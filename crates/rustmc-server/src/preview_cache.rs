//! Operator-local cache of immutable Java 26.3 preview chunk packets.
//!
//! This is a rendering accelerator, not a saved world: it contains no player
//! edits, entities, or authoritative state. The key includes every provisioned
//! worldgen file, the registry table, seed, protocol, and cache format version.

use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const MAGIC: &[u8; 8] = b"RMCPC001";
const HEADER_BYTES: usize = 8 + 4 + 4 + 4 + 32;
const MAX_PACKET_BYTES: usize = crate::chunk_adapter::MAX_CHUNK_PACKET_BYTES;
const MAX_FILES: usize = 8_192;
const MAX_BYTES: u64 = 1_073_741_824;
const MAX_INPUT_FILES: usize = 4_096;
const MAX_INPUT_BYTES: u64 = 67_108_864;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default)]
struct Budget {
    files: VecDeque<(PathBuf, u64)>,
    bytes: u64,
}

/// Shared, bounded packet store; clones share the same write/eviction lock.
#[derive(Debug, Clone)]
pub struct PreviewCache {
    directory: Arc<PathBuf>,
    budget: Arc<Mutex<Budget>>,
}

impl PreviewCache {
    pub fn open(
        root: &Path,
        data_root: &Path,
        registry_table: &Path,
        seed: i64,
    ) -> io::Result<Self> {
        let identity = input_identity(data_root, registry_table, seed)?;
        let directory = root.join(hex(&identity));
        fs::create_dir_all(&directory)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "pkt") {
                let metadata = entry.metadata()?;
                if metadata.is_file() {
                    entries.push((
                        path,
                        metadata.len(),
                        metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
                    ));
                }
            }
        }
        entries.sort_unstable_by_key(|(_, _, modified)| *modified);
        let mut budget = Budget::default();
        for (path, size, _) in entries {
            budget.bytes += size;
            budget.files.push_back((path, size));
        }
        prune(&mut budget)?;
        Ok(Self {
            directory: Arc::new(directory),
            budget: Arc::new(Mutex::new(budget)),
        })
    }

    fn path(&self, x: i32, z: i32) -> PathBuf {
        self.directory.join(format!("{x}.{z}.pkt"))
    }

    pub fn read(&self, x: i32, z: i32) -> io::Result<Option<Vec<u8>>> {
        let path = self.path(x, z);
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() > (HEADER_BYTES + MAX_PACKET_BYTES) as u64 {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Ok(None);
        }
        let stored_x = i32::from_be_bytes(bytes[8..12].try_into().expect("fixed slice"));
        let stored_z = i32::from_be_bytes(bytes[12..16].try_into().expect("fixed slice"));
        let length = u32::from_be_bytes(bytes[16..20].try_into().expect("fixed slice")) as usize;
        if stored_x != x
            || stored_z != z
            || length == 0
            || length > MAX_PACKET_BYTES
            || bytes.len() != HEADER_BYTES + length
        {
            return Ok(None);
        }
        let packet = &bytes[HEADER_BYTES..];
        if Sha256::digest(packet).as_slice() != &bytes[20..52] {
            return Ok(None);
        }
        Ok(Some(packet.to_vec()))
    }

    pub fn write(&self, x: i32, z: i32, packet: &[u8]) -> io::Result<()> {
        if packet.is_empty() || packet.len() > MAX_PACKET_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid packet size",
            ));
        }
        let mut budget = self.budget.lock().expect("preview cache lock poisoned");
        let path = self.path(x, z);
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary =
            self.directory
                .join(format!(".{x}.{z}.{}.{}.tmp", std::process::id(), sequence));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(MAGIC)?;
            file.write_all(&x.to_be_bytes())?;
            file.write_all(&z.to_be_bytes())?;
            file.write_all(&(packet.len() as u32).to_be_bytes())?;
            file.write_all(&Sha256::digest(packet))?;
            file.write_all(packet)?;
            file.sync_all()?;
            fs::rename(&temporary, &path)?;
            Ok::<_, io::Error>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
            return result;
        }
        if let Some(index) = budget.files.iter().position(|(stored, _)| *stored == path) {
            let (_, old_size) = budget.files.remove(index).expect("index exists");
            budget.bytes -= old_size;
        }
        let size = (HEADER_BYTES + packet.len()) as u64;
        budget.files.push_back((path, size));
        budget.bytes += size;
        prune(&mut budget)
    }

    pub fn cached_count(&self) -> usize {
        self.budget
            .lock()
            .expect("preview cache lock poisoned")
            .files
            .len()
    }
}

fn prune(budget: &mut Budget) -> io::Result<()> {
    while budget.files.len() > MAX_FILES || budget.bytes > MAX_BYTES {
        let (path, size) = budget.files.pop_front().expect("over budget has files");
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        budget.bytes -= size;
    }
    Ok(())
}

fn input_identity(data_root: &Path, registry_table: &Path, seed: i64) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(b"rustmc-preview-cache-v1:java-26.3:protocol-777");
    hasher.update(seed.to_be_bytes());
    let mut paths = Vec::new();
    collect_files(data_root, &mut paths)?;
    if paths.len() > MAX_INPUT_FILES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many worldgen files",
        ));
    }
    paths.sort();
    let mut total = 0u64;
    for path in paths {
        let relative = path.strip_prefix(data_root).expect("collected under root");
        let name = relative.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "non-UTF-8 worldgen path")
        })?;
        let bytes = fs::read(&path)?;
        total += bytes.len() as u64;
        if total > MAX_INPUT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "worldgen input too large",
            ));
        }
        hasher.update((name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    let registry = fs::read(registry_table)?;
    if registry.len() as u64 > MAX_INPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "registry input too large",
        ));
    }
    hasher.update((registry.len() as u64).to_be_bytes());
    hasher.update(registry);
    Ok(hasher.finalize().into())
}

fn collect_files(directory: &Path, paths: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "worldgen symlink",
            ));
        }
        if kind.is_dir() {
            collect_files(&entry.path(), paths)?;
        } else if kind.is_file() {
            paths.push(entry.path());
            if paths.len() > MAX_INPUT_FILES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "too many worldgen files",
                ));
            }
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_corruption_and_input_invalidation() {
        let root = std::env::temp_dir().join(format!(
            "rustmc-preview-cache-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let data = root.join("data");
        fs::create_dir_all(&data).unwrap();
        let registry = root.join("registry.json");
        fs::write(data.join("world.json"), b"alpha").unwrap();
        fs::write(&registry, b"registry-a").unwrap();
        let cache = PreviewCache::open(&root.join("cache"), &data, &registry, 2026).unwrap();
        assert_eq!(cache.read(-2, 3).unwrap(), None);
        cache.write(-2, 3, b"framed-packet").unwrap();
        assert_eq!(cache.read(-2, 3).unwrap(), Some(b"framed-packet".to_vec()));
        let reopened = PreviewCache::open(&root.join("cache"), &data, &registry, 2026).unwrap();
        assert_eq!(reopened.cached_count(), 1);
        assert_eq!(
            reopened.read(-2, 3).unwrap(),
            Some(b"framed-packet".to_vec())
        );
        let mut corrupt = fs::read(cache.path(-2, 3)).unwrap();
        *corrupt.last_mut().unwrap() ^= 1;
        fs::write(cache.path(-2, 3), corrupt).unwrap();
        assert_eq!(cache.read(-2, 3).unwrap(), None);
        assert_eq!(
            PreviewCache::open(&root.join("cache"), &data, &registry, 2027)
                .unwrap()
                .cached_count(),
            0
        );
        fs::write(data.join("world.json"), b"beta").unwrap();
        assert_eq!(
            PreviewCache::open(&root.join("cache"), &data, &registry, 2026)
                .unwrap()
                .cached_count(),
            0
        );
        fs::write(&registry, b"registry-b").unwrap();
        assert_eq!(
            PreviewCache::open(&root.join("cache"), &data, &registry, 2026)
                .unwrap()
                .cached_count(),
            0
        );
        fs::remove_dir_all(root).unwrap();
    }
}
