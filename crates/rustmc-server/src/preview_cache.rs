//! Operator-local cache of immutable Java 26.3 preview chunk packets.
//!
//! This is a rendering accelerator, not a saved world: it contains no player
//! edits, entities, or authoritative state. The key includes every provisioned
//! worldgen file, the registry table, seed, protocol, and cache format version.

use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

const MAGIC: &[u8; 8] = b"RMCPC002";
const HEADER_BYTES: usize = 8 + 4 + 4 + 4 + 32;
const MAX_PACKET_BYTES: usize = crate::chunk_adapter::MAX_CHUNK_PACKET_BYTES;
const MAX_FILES: usize = 8_192;
const MAX_BYTES: u64 = 1_073_741_824;
const MAX_INPUT_FILES: usize = 4_096;
const MAX_INPUT_BYTES: u64 = 67_108_864;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
/// Build-time hash of the sources that shape cached packets (see `build.rs`).
const PACKET_SOURCE_FINGERPRINT: &str = env!("RUSTMC_PACKET_SOURCE_FINGERPRINT");

#[derive(Debug, Default)]
struct Budget {
    files: VecDeque<(PathBuf, u64)>,
    positions: BTreeSet<(i32, i32)>,
    bytes: u64,
}

impl Budget {
    fn remove_file_entry(&mut self, path: &Path) {
        if let Some(index) = self.files.iter().position(|(stored, _)| stored == path)
            && let Some((_, old_size)) = self.files.remove(index)
        {
            self.bytes -= old_size;
        }
    }
}

fn packet_position(path: &Path) -> Option<(i32, i32)> {
    let (x, z) = path.file_stem()?.to_str()?.split_once('.')?;
    Some((x.parse().ok()?, z.parse().ok()?))
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
            if let Some(position) = packet_position(&path) {
                budget.positions.insert(position);
            }
            budget.files.push_back((path, size));
        }
        let evicted = prune(&mut budget);
        remove_evicted(evicted)?;
        Ok(Self {
            directory: Arc::new(directory),
            budget: Arc::new(Mutex::new(budget)),
        })
    }

    fn lock_budget(&self) -> MutexGuard<'_, Budget> {
        // The budget only tracks in-memory cache paths and byte totals; `read` re-validates files on disk.
        self.budget.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn path(&self, x: i32, z: i32) -> PathBuf {
        self.directory.join(format!("{x}.{z}.pkt"))
    }

    pub fn read(&self, x: i32, z: i32) -> io::Result<Option<Vec<u8>>> {
        let packet = read_packet(&self.path(x, z), x, z)?;
        if packet.is_none() {
            self.forget(x, z);
        }
        Ok(packet)
    }

    fn forget(&self, x: i32, z: i32) {
        let path = self.path(x, z);
        {
            let mut budget = self.lock_budget();
            budget.remove_file_entry(&path);
            budget.positions.remove(&(x, z));
        }
        // The entry is already out of the budget, so a failed delete only wastes disk space.
        let _ = fs::remove_file(&path);
    }

    /// A cheap hint for scheduling cached packets; `read` still validates them.
    pub fn contains(&self, x: i32, z: i32) -> bool {
        self.lock_budget().positions.contains(&(x, z))
    }

    pub fn write(&self, x: i32, z: i32, packet: &[u8]) -> io::Result<()> {
        if packet.is_empty() || packet.len() > MAX_PACKET_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid packet size",
            ));
        }
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
        // File I/O and fsync above, and eviction unlinks below, run without the
        // lock, so concurrent workers and scheduling hints (`contains`) never
        // wait on disk. Each write uses a unique temporary file and an atomic
        // rename; only the bookkeeping in this block is serialized.
        let evicted = {
            let mut budget = self.lock_budget();
            budget.remove_file_entry(&path);
            let size = (HEADER_BYTES + packet.len()) as u64;
            budget.files.push_back((path, size));
            budget.positions.insert((x, z));
            budget.bytes += size;
            prune(&mut budget)
        };
        remove_evicted(evicted)
    }

    pub fn cached_count(&self) -> usize {
        self.lock_budget().files.len()
    }
}

fn read_packet(path: &Path, x: i32, z: i32) -> io::Result<Option<Vec<u8>>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() > (HEADER_BYTES + MAX_PACKET_BYTES) as u64 {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let Some((magic, rest)) = bytes.split_first_chunk::<8>() else {
        return Ok(None);
    };
    if magic != MAGIC {
        return Ok(None);
    }
    let Some((stored_x_bytes, rest)) = rest.split_first_chunk::<4>() else {
        return Ok(None);
    };
    let Some((stored_z_bytes, rest)) = rest.split_first_chunk::<4>() else {
        return Ok(None);
    };
    let Some((length_bytes, rest)) = rest.split_first_chunk::<4>() else {
        return Ok(None);
    };
    let Some((digest, packet)) = rest.split_first_chunk::<32>() else {
        return Ok(None);
    };
    let stored_x = i32::from_be_bytes(*stored_x_bytes);
    let stored_z = i32::from_be_bytes(*stored_z_bytes);
    let length = u32::from_be_bytes(*length_bytes) as usize;
    if stored_x != x
        || stored_z != z
        || length == 0
        || length > MAX_PACKET_BYTES
        || packet.len() != length
    {
        return Ok(None);
    }
    if Sha256::digest(packet).as_slice() != digest {
        return Ok(None);
    }
    Ok(Some(packet.to_vec()))
}

fn prune(budget: &mut Budget) -> Vec<PathBuf> {
    let mut evicted = Vec::new();
    while budget.files.len() > MAX_FILES || budget.bytes > MAX_BYTES {
        let Some((path, size)) = budget.files.pop_front() else {
            break;
        };
        budget.bytes -= size;
        if let Some(position) = packet_position(&path) {
            budget.positions.remove(&position);
        }
        evicted.push(path);
    }
    evicted
}

fn remove_evicted(paths: Vec<PathBuf>) -> io::Result<()> {
    for path in paths {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn input_identity(data_root: &Path, registry_table: &Path, seed: i64) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(b"rustmc-preview-cache:java-26.3:protocol-777");
    // Generator and encoder sources, so code changes never reuse old packets.
    hasher.update(PACKET_SOURCE_FINGERPRINT.as_bytes());
    hasher.update(seed.to_be_bytes());
    let mut paths = Vec::new();
    for directory in generator_input_dirs(data_root)? {
        collect_files(&directory, &mut paths)?;
    }
    if paths.len() > MAX_INPUT_FILES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many worldgen files",
        ));
    }
    paths.sort();
    let mut total = 0u64;
    for path in paths {
        let relative = path.strip_prefix(data_root).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "worldgen path outside data root",
            )
        })?;
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

/// The data-root subtrees the vanilla generator reads: `data/<ns>/worldgen`,
/// `data/<ns>/tags/block`, and RustMC's `rustmc/` biome placement tables.
/// Recipes, loot tables, reports and other rule data provisioned in the same
/// root do not shape generated chunks, so they neither count toward
/// [`MAX_INPUT_FILES`] nor invalidate cached packets.
fn generator_input_dirs(data_root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut directories = Vec::new();
    let data = data_root.join("data");
    match fs::read_dir(&data) {
        Ok(namespaces) => {
            for namespace in namespaces {
                let namespace = namespace?;
                if !namespace.file_type()?.is_dir() {
                    continue;
                }
                for sub in [
                    namespace.path().join("worldgen"),
                    namespace.path().join("tags").join("block"),
                ] {
                    if sub.is_dir() {
                        directories.push(sub);
                    }
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let placement = data_root.join("rustmc");
    if placement.is_dir() {
        directories.push(placement);
    }
    directories.sort();
    Ok(directories)
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
mod tests;
