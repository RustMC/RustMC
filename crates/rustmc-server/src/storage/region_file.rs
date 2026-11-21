//! Reading and atomically rewriting Anvil region files (`r.X.Z.mca`) for the
//! server's own world.
//!
//! Why: the server must load a single chunk quickly while it is running and
//! save changed chunks without ever leaving a half-written region behind.
//! Done: a 4 KiB-sector header read and seek per chunk; saving merges changed
//! chunks with the file's untouched ones (their compressed bytes are kept
//! as-is) and replaces the file by writing a temporary file, syncing it,
//! renaming it over the old one and syncing the directory. Not done: chunks
//! over 255 sectors (about 1 MiB compressed), which vanilla moves to separate
//! `.mcc` files; RustMC refuses to save them.
//!
//! Format: minecraft.wiki `Region file format` — an 8 KiB header of 1,024
//! big-endian locations (3-byte sector offset, 1-byte sector count) and 1,024
//! timestamps, then chunks as a 4-byte length, a compression byte and the
//! payload, each padded to whole 4 KiB sectors.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

/// Bytes per region sector.
pub const SECTOR: usize = 4096;
/// Chunks along one side of a region.
pub const REGION_SIDE: usize = 32;
/// Header size: one sector of locations and one of timestamps.
const HEADER: usize = 2 * SECTOR;
/// Most sectors one chunk may use (the location's 1-byte count).
const MAX_CHUNK_SECTORS: usize = 255;
/// Compression byte for zlib, the format vanilla 26.3 writes.
const ZLIB: u8 = 2;
/// Largest decompressed chunk accepted when reading, so a damaged file
/// cannot exhaust memory: well above any 384-block column.
const MAX_CHUNK_NBT: u64 = 16 * 1024 * 1024;

/// One stored chunk as it sits in the file: compression byte and payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredChunk {
    pub compression: u8,
    pub payload: Vec<u8>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

fn index(local_x: usize, local_z: usize) -> usize {
    local_x + local_z * REGION_SIDE
}

/// Reads one chunk's decompressed NBT bytes, or `None` when the region file
/// or the chunk does not exist.
pub fn read_chunk(path: &Path, local_x: usize, local_z: usize) -> io::Result<Option<Vec<u8>>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut location = [0u8; 4];
    file.seek(SeekFrom::Start((index(local_x, local_z) * 4) as u64))?;
    file.read_exact(&mut location)?;
    let sector = u32::from_be_bytes([0, location[0], location[1], location[2]]) as usize;
    let sectors = usize::from(location[3]);
    if sector == 0 && sectors == 0 {
        return Ok(None);
    }
    if sector < HEADER / SECTOR || sectors == 0 {
        return Err(invalid("chunk location points into the header"));
    }
    file.seek(SeekFrom::Start((sector * SECTOR) as u64))?;
    let mut prefix = [0u8; 5];
    file.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
    if length == 0 || length + 4 > sectors * SECTOR {
        return Err(invalid("chunk length does not fit its sectors"));
    }
    let mut payload = vec![0u8; length - 1];
    file.read_exact(&mut payload)?;
    decompress(&StoredChunk {
        compression: prefix[4],
        payload,
    })
    .map(Some)
}

fn decompress(chunk: &StoredChunk) -> io::Result<Vec<u8>> {
    match chunk.compression {
        ZLIB => {
            let mut out = Vec::new();
            ZlibDecoder::new(chunk.payload.as_slice())
                .take(MAX_CHUNK_NBT + 1)
                .read_to_end(&mut out)?;
            if out.len() as u64 > MAX_CHUNK_NBT {
                return Err(invalid("chunk is larger than any column"));
            }
            Ok(out)
        }
        _ => Err(invalid("unsupported chunk compression")),
    }
}

/// Every chunk stored in a region file, still compressed.
fn read_all(path: &Path) -> io::Result<BTreeMap<usize, StoredChunk>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error),
    };
    if bytes.len() < HEADER {
        return Err(invalid("region file is smaller than its header"));
    }
    let mut chunks = BTreeMap::new();
    for slot in 0..REGION_SIDE * REGION_SIDE {
        let entry = &bytes[slot * 4..slot * 4 + 4];
        let sector = u32::from_be_bytes([0, entry[0], entry[1], entry[2]]) as usize;
        if sector == 0 {
            continue;
        }
        let start = sector * SECTOR;
        let prefix = bytes
            .get(start..start + 5)
            .ok_or_else(|| invalid("chunk points past the end of the file"))?;
        let length = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
        let payload = bytes
            .get(start + 5..start + 4 + length)
            .ok_or_else(|| invalid("chunk extends past the end of the file"))?;
        chunks.insert(
            slot,
            StoredChunk {
                compression: prefix[4],
                payload: payload.to_vec(),
            },
        );
    }
    Ok(chunks)
}

/// Compresses chunk NBT the way vanilla stores it.
pub fn compress(nbt: &[u8]) -> io::Result<StoredChunk> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(nbt)?;
    Ok(StoredChunk {
        compression: ZLIB,
        payload: encoder.finish()?,
    })
}

/// Writes `changed` chunks (keyed by local x, z) into the region at `path`,
/// keeping every other chunk already there, and replaces the file atomically.
pub fn save_chunks(
    path: &Path,
    changed: BTreeMap<(usize, usize), StoredChunk>,
    timestamp: u32,
) -> io::Result<()> {
    let mut chunks = read_all(path)?;
    let mut stamped = Vec::with_capacity(changed.len());
    for ((x, z), chunk) in changed {
        if x >= REGION_SIDE || z >= REGION_SIDE {
            return Err(invalid("chunk position outside the region"));
        }
        stamped.push(index(x, z));
        chunks.insert(index(x, z), chunk);
    }
    let mut header = vec![0u8; HEADER];
    let mut body = Vec::new();
    let mut next_sector = HEADER / SECTOR;
    for (slot, chunk) in &chunks {
        let length = chunk.payload.len() + 1;
        let sectors = (length + 4).div_ceil(SECTOR);
        if sectors > MAX_CHUNK_SECTORS {
            return Err(invalid("chunk needs more than 255 sectors"));
        }
        let location = ((next_sector as u32) << 8) | sectors as u32;
        header[slot * 4..slot * 4 + 4].copy_from_slice(&location.to_be_bytes());
        if stamped.contains(slot) {
            header[SECTOR + slot * 4..SECTOR + slot * 4 + 4]
                .copy_from_slice(&timestamp.to_be_bytes());
        }
        body.extend_from_slice(&(length as u32).to_be_bytes());
        body.push(chunk.compression);
        body.extend_from_slice(&chunk.payload);
        body.resize(body.len().next_multiple_of(SECTOR), 0);
        next_sector += sectors;
    }
    // Timestamps of chunks kept from the old file are preserved.
    if let Ok(old) = fs::read(path)
        && old.len() >= HEADER
    {
        for slot in chunks.keys().filter(|slot| !stamped.contains(slot)) {
            let at = SECTOR + slot * 4;
            header[at..at + 4].copy_from_slice(&old[at..at + 4]);
        }
    }
    replace_atomically(path, &[&header, &body])
}

/// Writes `parts` to `path` so that a crash leaves either the old file or the
/// new one, never a mix.
pub fn replace_atomically(path: &Path, parts: &[&[u8]]) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| invalid("path has no directory"))?;
    create_dir_durably(directory)?;
    let temporary = path.with_extension("tmp");
    {
        let mut file = File::create(&temporary)?;
        for part in parts {
            file.write_all(part)?;
        }
        file.sync_all()?;
    }
    fs::rename(&temporary, path)?;
    sync_directory(directory)
}

/// Creates `directory` and any missing ancestors, syncing each new one's
/// entry into its parent so a fresh world's directories survive a crash.
pub fn create_dir_durably(directory: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut current = directory;
    while !current.as_os_str().is_empty() && !current.exists() {
        missing.push(current);
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    fs::create_dir_all(directory)?;
    for created in missing.iter().rev() {
        // A relative top-level directory's parent is the working directory.
        let parent = match created.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        sync_directory(parent)?;
    }
    Ok(())
}

/// Makes the rename durable. Windows cannot open a directory as a file; its
/// rename is already flushed with the file's metadata.
#[cfg(unix)]
pub fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
pub fn sync_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
