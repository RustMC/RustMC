//! Authoritative block-mutation write-ahead journal and checkpoint persistence.

use super::{Block, BlockMutation, Terrain, WorldError};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const JOURNAL_MAGIC: &[u8; 8] = b"RMCJNL01";
pub const CHECKPOINT_MAGIC: &[u8; 8] = b"RMCCKP01";
pub const HEADER_SIZE: usize = 32;

/// Recovered journal state consisting of the active journal,
/// committed mutations map, and the recovered tick sequence ID.
pub type JournalRecovery = (WorldJournal, BTreeMap<(i64, i64, i64), Block>, u64);

fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        if let Ok(f) = std::fs::File::open(dir) {
            let _ = f.sync_all();
        }
    }
    let _ = dir;
    Ok(())
}

/// Manages `journal.log` and `checkpoint.bin` with CRC32 checksums,
/// crash recovery, and atomic checkpoint compaction.
#[derive(Debug)]
pub struct WorldJournal {
    dir: PathBuf,
    log_file: std::fs::File,
    seed: u64,
    terrain: Terrain,
}

impl WorldJournal {
    /// Opens the world journal in `dir`. Recovers any existing checkpoint and
    /// replays valid WAL records from `journal.log`, truncating any partial
    /// trailing writes left by an unclean shutdown.
    pub fn open(dir: &Path, seed: u64, terrain: Terrain) -> Result<JournalRecovery, WorldError> {
        std::fs::create_dir_all(dir)?;

        let mut mutations = BTreeMap::new();
        let mut recovered_tick = 0u64;

        // 1. Recover from checkpoint.bin if it exists.
        // Orphaned temporary files (.checkpoint.*) are ignored.
        let checkpoint_path = dir.join("checkpoint.bin");
        if checkpoint_path.is_file() {
            let bytes = std::fs::read(&checkpoint_path)?;
            if bytes.len() < 36 {
                return Err(WorldError::Corrupted(
                    "checkpoint file is too small to contain valid header and CRC".into(),
                ));
            }
            if &bytes[0..8] != CHECKPOINT_MAGIC {
                return Err(WorldError::InvalidMagic);
            }
            let payload_len = bytes.len() - 4;
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(&bytes[0..payload_len]);
            let computed_crc = hasher.finalize();
            let stored_crc = u32::from_be_bytes(bytes[payload_len..].try_into().unwrap());
            if computed_crc != stored_crc {
                return Err(WorldError::Corrupted(format!(
                    "checkpoint CRC32 mismatch: computed {computed_crc:08x}, stored {stored_crc:08x}"
                )));
            }

            let cp_seed = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
            if cp_seed != seed {
                return Err(WorldError::SeedMismatch {
                    expected: seed,
                    found: cp_seed,
                });
            }

            let cp_terrain_byte = bytes[16];
            let cp_terrain = Terrain::from_u8(cp_terrain_byte)
                .ok_or(WorldError::InvalidTerrainId(cp_terrain_byte))?;
            if cp_terrain != terrain {
                return Err(WorldError::TerrainMismatch {
                    expected: terrain,
                    found: cp_terrain,
                });
            }

            let last_tick = u64::from_be_bytes(bytes[20..28].try_into().unwrap());
            let count = u32::from_be_bytes(bytes[28..32].try_into().unwrap()) as usize;
            let expected_len = 32 + count * 25 + 4;
            if bytes.len() != expected_len {
                return Err(WorldError::Corrupted(format!(
                    "checkpoint length mismatch: expected {expected_len}, got {}",
                    bytes.len()
                )));
            }

            let mut offset = 32;
            for _ in 0..count {
                let x = i64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
                let y = i64::from_be_bytes(bytes[offset + 8..offset + 16].try_into().unwrap());
                let z = i64::from_be_bytes(bytes[offset + 16..offset + 24].try_into().unwrap());
                let block_byte = bytes[offset + 24];
                offset += 25;
                let block =
                    Block::from_u8(block_byte).ok_or(WorldError::InvalidBlockId(block_byte))?;
                mutations.insert((x, y, z), block);
            }
            recovered_tick = last_tick;
        }

        // 2. Open or create journal.log and replay records.
        let log_path = dir.join("journal.log");
        let (mut file, valid_offset) = if log_path.is_file() {
            let mut f = OpenOptions::new().read(true).write(true).open(&log_path)?;
            let file_len = f.metadata()?.len();
            if file_len < HEADER_SIZE as u64 {
                // Incomplete header due to crash before header write finished.
                write_log_header(&mut f, seed, terrain, recovered_tick)?;
                f.flush()?;
                (f, HEADER_SIZE as u64)
            } else {
                let mut header = [0u8; HEADER_SIZE];
                f.seek(SeekFrom::Start(0))?;
                f.read_exact(&mut header)?;

                if &header[0..8] != JOURNAL_MAGIC {
                    return Err(WorldError::InvalidMagic);
                }

                let mut hasher = crc32fast::Hasher::new();
                hasher.update(&header[0..28]);
                let computed_header_crc = hasher.finalize();
                let stored_header_crc = u32::from_be_bytes(header[28..32].try_into().unwrap());
                if computed_header_crc != stored_header_crc {
                    return Err(WorldError::Corrupted(
                        "journal header CRC32 mismatch".into(),
                    ));
                }

                let jnl_seed = u64::from_be_bytes(header[8..16].try_into().unwrap());
                if jnl_seed != seed {
                    return Err(WorldError::SeedMismatch {
                        expected: seed,
                        found: jnl_seed,
                    });
                }

                let jnl_terrain_byte = header[16];
                let jnl_terrain = Terrain::from_u8(jnl_terrain_byte)
                    .ok_or(WorldError::InvalidTerrainId(jnl_terrain_byte))?;
                if jnl_terrain != terrain {
                    return Err(WorldError::TerrainMismatch {
                        expected: terrain,
                        found: jnl_terrain,
                    });
                }

                // Replay WAL records starting at offset 32.
                f.seek(SeekFrom::Start(HEADER_SIZE as u64))?;
                let mut valid_offset = HEADER_SIZE as u64;

                loop {
                    let remaining = file_len.saturating_sub(valid_offset);
                    if remaining < 12 {
                        // Incomplete record header at EOF (truncated trailing write)
                        break;
                    }

                    let mut record_hdr = [0u8; 12];
                    match f.read_exact(&mut record_hdr) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                            break;
                        }
                        Err(e) => return Err(WorldError::Io(e)),
                    }

                    let tick = u64::from_be_bytes(record_hdr[0..8].try_into().unwrap());
                    if tick < recovered_tick {
                        return Err(WorldError::Corrupted(format!(
                            "non-monotonic sequence ID {tick} < {recovered_tick} at offset {valid_offset}"
                        )));
                    }

                    let count = u32::from_be_bytes(record_hdr[8..12].try_into().unwrap()) as usize;
                    if count > 100_000 {
                        let needed = 12u64
                            .saturating_add((count as u64).saturating_mul(25))
                            .saturating_add(4);
                        if remaining < needed {
                            // Incomplete/torn header at EOF
                            break;
                        }
                        return Err(WorldError::Corrupted(format!(
                            "unreasonable mutation count {count} at offset {valid_offset}"
                        )));
                    }

                    let body_len = count * 25 + 4;
                    if remaining < 12 + body_len as u64 {
                        // Incomplete record payload at EOF (truncated trailing write)
                        break;
                    }

                    let mut body = vec![0u8; body_len];
                    match f.read_exact(&mut body) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                            break;
                        }
                        Err(e) => return Err(WorldError::Io(e)),
                    }

                    let mut hasher = crc32fast::Hasher::new();
                    hasher.update(&record_hdr);
                    hasher.update(&body[0..count * 25]);
                    let computed_crc = hasher.finalize();
                    let stored_crc =
                        u32::from_be_bytes(body[count * 25..count * 25 + 4].try_into().unwrap());
                    if computed_crc != stored_crc {
                        let record_end = valid_offset + 12 + body_len as u64;
                        if record_end == file_len {
                            // Checksum failure on the trailing record at the very end of file
                            // (torn write during power loss)
                            break;
                        } else {
                            // Checksum mismatch inside the journal prior to EOF
                            return Err(WorldError::Corrupted(format!(
                                "checksum mismatch at offset {valid_offset}: computed {computed_crc:#x}, stored {stored_crc:#x}"
                            )));
                        }
                    }

                    let mut record_muts = Vec::with_capacity(count);
                    let mut offset = 0;
                    for _ in 0..count {
                        let x = i64::from_be_bytes(body[offset..offset + 8].try_into().unwrap());
                        let y =
                            i64::from_be_bytes(body[offset + 8..offset + 16].try_into().unwrap());
                        let z =
                            i64::from_be_bytes(body[offset + 16..offset + 24].try_into().unwrap());
                        let block_byte = body[offset + 24];
                        offset += 25;
                        if let Some(block) = Block::from_u8(block_byte) {
                            record_muts.push((x, y, z, block));
                        } else {
                            return Err(WorldError::Corrupted(format!(
                                "invalid block ID {block_byte} at offset {valid_offset}"
                            )));
                        }
                    }

                    for (x, y, z, block) in record_muts {
                        mutations.insert((x, y, z), block);
                    }

                    recovered_tick = tick + 1;
                    valid_offset += 12 + body_len as u64;
                }

                // Truncate back to the last valid boundary if partial writes existed
                if file_len > valid_offset {
                    f.set_len(valid_offset)?;
                }
                f.seek(SeekFrom::Start(valid_offset))?;
                (f, valid_offset)
            }
        } else {
            let mut f = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(true)
                .open(&log_path)?;
            write_log_header(&mut f, seed, terrain, recovered_tick)?;
            f.flush()?;
            (f, HEADER_SIZE as u64)
        };

        // Ensure file write offset is at valid_offset
        file.seek(SeekFrom::Start(valid_offset))?;

        let journal = Self {
            dir: dir.to_path_buf(),
            log_file: file,
            seed,
            terrain,
        };

        Ok((journal, mutations, recovered_tick))
    }

    /// Appends a tick batch WAL record with CRC32 checksum.
    pub fn append_tick(
        &mut self,
        tick: u64,
        mutations: &[BlockMutation],
    ) -> Result<(), WorldError> {
        let count = mutations.len() as u32;
        let mut record = Vec::with_capacity(12 + mutations.len() * 25 + 4);
        record.extend_from_slice(&tick.to_be_bytes());
        record.extend_from_slice(&count.to_be_bytes());
        for m in mutations {
            record.extend_from_slice(&m.x.to_be_bytes());
            record.extend_from_slice(&m.y.to_be_bytes());
            record.extend_from_slice(&m.z.to_be_bytes());
            record.push(m.block as u8);
        }

        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&record);
        let crc = hasher.finalize();
        record.extend_from_slice(&crc.to_be_bytes());

        self.log_file.seek(SeekFrom::End(0))?;
        self.log_file.write_all(&record)?;
        self.log_file.flush()?;
        self.log_file.sync_data()?;
        Ok(())
    }

    /// Flushes any buffered writes and syncs to disk.
    pub fn flush(&mut self) -> Result<(), WorldError> {
        self.log_file.flush()?;
        self.log_file.sync_data().map_err(WorldError::Io)?;
        Ok(())
    }

    /// Atomically compacts all committed mutations into `checkpoint.bin`
    /// via a temporary file and atomic rename, then truncates `journal.log`.
    pub fn compact(
        &mut self,
        current_tick: u64,
        committed_mutations: &BTreeMap<(i64, i64, i64), Block>,
    ) -> Result<(), WorldError> {
        let tmp_path = self
            .dir
            .join(format!(".checkpoint.{}", uuid::Uuid::new_v4()));
        let count = committed_mutations.len() as u32;
        let mut data = Vec::with_capacity(32 + (count as usize) * 25 + 4);

        data.extend_from_slice(CHECKPOINT_MAGIC);
        data.extend_from_slice(&self.seed.to_be_bytes());
        data.push(self.terrain.to_u8());
        data.extend_from_slice(&[0u8; 3]);
        data.extend_from_slice(&current_tick.to_be_bytes());
        data.extend_from_slice(&count.to_be_bytes());

        for (&(x, y, z), &block) in committed_mutations {
            data.extend_from_slice(&x.to_be_bytes());
            data.extend_from_slice(&y.to_be_bytes());
            data.extend_from_slice(&z.to_be_bytes());
            data.push(block as u8);
        }

        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&data);
        let crc = hasher.finalize();
        data.extend_from_slice(&crc.to_be_bytes());

        {
            let mut tmp_file = std::fs::File::create(&tmp_path)?;
            tmp_file.write_all(&data)?;
            tmp_file.sync_all()?;
        }

        let checkpoint_path = self.dir.join("checkpoint.bin");
        std::fs::rename(&tmp_path, &checkpoint_path)?;
        sync_dir(&self.dir)?;

        // Reset journal.log with current_tick as start sequence.
        write_log_header(&mut self.log_file, self.seed, self.terrain, current_tick)?;
        self.log_file.flush()?;
        self.log_file.sync_data()?;
        sync_dir(&self.dir)?;

        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn terrain(&self) -> Terrain {
        self.terrain
    }
}

fn write_log_header(
    file: &mut std::fs::File,
    seed: u64,
    terrain: Terrain,
    start_seq: u64,
) -> Result<(), std::io::Error> {
    let mut header = [0u8; HEADER_SIZE];
    header[0..8].copy_from_slice(JOURNAL_MAGIC);
    header[8..16].copy_from_slice(&seed.to_be_bytes());
    header[16] = terrain.to_u8();
    header[20..28].copy_from_slice(&start_seq.to_be_bytes());

    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&header[0..28]);
    let crc = hasher.finalize();
    header[28..32].copy_from_slice(&crc.to_be_bytes());

    file.seek(SeekFrom::Start(0))?;
    file.write_all(&header)?;
    file.set_len(HEADER_SIZE as u64)?;
    file.flush()?;
    file.sync_data()?;
    file.seek(SeekFrom::Start(HEADER_SIZE as u64))?;
    Ok(())
}
