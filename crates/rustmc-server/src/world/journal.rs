//! Authoritative block-mutation write-ahead journal and checkpoint persistence.
//!
//! # Durability boundary
//!
//! [`WorldJournal::append_tick`] acknowledges a tick only after its record has
//! been written and `fdatasync`ed, so an `Ok` return means the tick survives a
//! process crash or power loss. An `Err` return means the outcome is unknown
//! to the caller: the journal refuses further work until it is reopened, and
//! recovery then yields either the previous tick or that tick in full, never
//! a partial tick or a repeated one.
//!
//! # Files
//!
//! `journal.log` is a 32-byte header followed by records, and each record is:
//!
//! ```text
//! tick u64 | count u32 | header CRC32 u32 | count * mutation | record CRC32 u32
//! ```
//!
//! The header CRC covers `tick` and `count`, so recovery can trust a record's
//! length before it decides what the bytes after it mean. The record CRC covers
//! everything before it. Integers are big-endian. `checkpoint.bin` holds the
//! deduplicated state before the tick named in its header.
//!
//! Both files are only ever replaced by writing a synced temporary file and
//! renaming it into place. Compaction installs the checkpoint, syncs the
//! directory, and only then replaces the journal, so every crash point leaves
//! one of three states: old checkpoint with old journal, new checkpoint with
//! old journal, or new checkpoint with new journal. Recovery accepts all three.
//! A journal that starts after the checkpoint means acknowledged ticks are
//! missing, and recovery refuses it.
//!
//! # Torn writes and corruption
//!
//! Only the final record can be in flight when a crash happens, so only damage
//! that can be confined to it is truncated. Damage followed by a further valid
//! record, or by more bytes than one record can occupy, is reported as
//! corruption. A bit flip in the final record looks the same as a torn write
//! and drops that one tick; this format cannot distinguish them.

use super::{Block, BlockMutation, Terrain, WorldError};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const JOURNAL_MAGIC: &[u8; 8] = b"RMCJNL01";
pub const CHECKPOINT_MAGIC: &[u8; 8] = b"RMCCKP01";
pub const HEADER_SIZE: usize = 32;

/// Largest mutation batch one tick may commit. The writer enforces the same
/// bound the reader does, so every acknowledged record can be replayed.
pub const MAX_MUTATIONS_PER_TICK: usize = 100_000;

const MUTATION_SIZE: usize = 25;
const RECORD_HEADER_SIZE: usize = 16;
const RECORD_CRC_SIZE: usize = 4;
const MAX_RECORD_SIZE: usize =
    RECORD_HEADER_SIZE + MAX_MUTATIONS_PER_TICK * MUTATION_SIZE + RECORD_CRC_SIZE;
const CHECKPOINT_NAME: &str = "checkpoint.bin";
const JOURNAL_NAME: &str = "journal.log";
const CHECKPOINT_TEMP_PREFIX: &str = ".checkpoint.";
const JOURNAL_TEMP_PREFIX: &str = ".journal.";

/// Recovered journal state consisting of the active journal,
/// committed mutations map, and the recovered tick sequence ID.
pub type JournalRecovery = (WorldJournal, BTreeMap<(i64, i64, i64), Block>, u64);

/// Test-only fault injection for every durability-relevant I/O step.
#[cfg(test)]
pub(super) mod faults {
    use std::cell::RefCell;

    #[derive(Default)]
    struct Plan {
        named: Vec<&'static str>,
        countdown: Option<usize>,
        fired: Vec<&'static str>,
    }

    thread_local! {
        static PLAN: RefCell<Plan> = RefCell::new(Plan::default());
    }

    /// Fails the next occurrence of each named step, once.
    pub fn fail_steps(steps: &[&'static str]) {
        PLAN.with_borrow_mut(|plan| {
            *plan = Plan {
                named: steps.to_vec(),
                ..Plan::default()
            };
        });
    }

    /// Fails the step reached after `skipped` other steps have completed.
    pub fn fail_after(skipped: usize) {
        PLAN.with_borrow_mut(|plan| {
            *plan = Plan {
                countdown: Some(skipped),
                ..Plan::default()
            };
        });
    }

    /// Disarms every fault and returns the steps that actually failed.
    pub fn clear() -> Vec<&'static str> {
        PLAN.with_borrow_mut(|plan| std::mem::take(plan).fired)
    }

    pub(super) fn should_fail(step: &'static str) -> bool {
        PLAN.with_borrow_mut(|plan| {
            let fire = if let Some(index) = plan.named.iter().position(|name| *name == step) {
                plan.named.remove(index);
                true
            } else if let Some(remaining) = plan.countdown {
                plan.countdown = remaining.checked_sub(1);
                remaining == 0
            } else {
                false
            };
            if fire {
                plan.fired.push(step);
            }
            fire
        })
    }
}

#[cfg(test)]
fn io_step(step: &'static str) -> std::io::Result<()> {
    if faults::should_fail(step) {
        return Err(std::io::Error::other(format!("injected fault at {step}")));
    }
    Ok(())
}

#[cfg(not(test))]
#[inline(always)]
fn io_step(_step: &'static str) -> std::io::Result<()> {
    Ok(())
}

/// Makes a rename or file creation in `dir` durable. Failure is returned
/// because callers must not discard older data on an unconfirmed rename.
/// Directories cannot be synced this way outside Unix, so there the rename's
/// durability is left to the platform.
fn sync_dir(dir: &Path, step: &'static str) -> std::io::Result<()> {
    io_step(step)?;
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize()
}

fn be_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("slice is four bytes"))
}

fn be_u64(bytes: &[u8]) -> u64 {
    u64::from_be_bytes(bytes.try_into().expect("slice is eight bytes"))
}

fn be_i64(bytes: &[u8]) -> i64 {
    i64::from_be_bytes(bytes.try_into().expect("slice is eight bytes"))
}

fn next_tick_after(tick: u64) -> Result<u64, WorldError> {
    tick.checked_add(1)
        .ok_or_else(|| WorldError::Corrupted("tick sequence ID overflow".into()))
}

/// Decodes a record header, or `None` when its CRC does not match.
fn parse_record_header(header: &[u8]) -> Option<(u64, u32)> {
    if crc32(&[&header[0..12]]) != be_u32(&header[12..16]) {
        return None;
    }
    Some((be_u64(&header[0..8]), be_u32(&header[8..12])))
}

/// Whether a record with an intact header starts anywhere after the first byte
/// of `bytes`. Used on a damaged header: a later record proves the damage is
/// not a torn tail.
fn contains_later_record(bytes: &[u8]) -> bool {
    bytes.windows(RECORD_HEADER_SIZE).skip(1).any(|window| {
        parse_record_header(window)
            .is_some_and(|(_, count)| count as usize <= MAX_MUTATIONS_PER_TICK)
    })
}

fn read_checkpoint(
    path: &Path,
    seed: u64,
    terrain: Terrain,
) -> Result<(BTreeMap<(i64, i64, i64), Block>, u64), WorldError> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < HEADER_SIZE + 4 {
        return Err(WorldError::Corrupted(
            "checkpoint file is too small to contain valid header and CRC".into(),
        ));
    }
    if &bytes[0..8] != CHECKPOINT_MAGIC {
        return Err(WorldError::InvalidMagic);
    }
    let payload_len = bytes.len() - 4;
    let computed_crc = crc32(&[&bytes[0..payload_len]]);
    let stored_crc = be_u32(&bytes[payload_len..]);
    if computed_crc != stored_crc {
        return Err(WorldError::Corrupted(format!(
            "checkpoint CRC32 mismatch: computed {computed_crc:08x}, stored {stored_crc:08x}"
        )));
    }

    check_identity(be_u64(&bytes[8..16]), bytes[16], seed, terrain)?;

    let next_tick = be_u64(&bytes[20..28]);
    let count = be_u32(&bytes[28..32]) as usize;
    let expected_len = HEADER_SIZE + count * MUTATION_SIZE + 4;
    if bytes.len() != expected_len {
        return Err(WorldError::Corrupted(format!(
            "checkpoint length mismatch: expected {expected_len}, got {}",
            bytes.len()
        )));
    }

    let mut mutations = BTreeMap::new();
    for entry in bytes[HEADER_SIZE..payload_len].chunks_exact(MUTATION_SIZE) {
        let block = Block::from_u8(entry[24]).ok_or(WorldError::InvalidBlockId(entry[24]))?;
        mutations.insert(
            (
                be_i64(&entry[0..8]),
                be_i64(&entry[8..16]),
                be_i64(&entry[16..24]),
            ),
            block,
        );
    }
    Ok((mutations, next_tick))
}

fn check_identity(
    found_seed: u64,
    found_terrain: u8,
    seed: u64,
    terrain: Terrain,
) -> Result<(), WorldError> {
    if found_seed != seed {
        return Err(WorldError::SeedMismatch {
            expected: seed,
            found: found_seed,
        });
    }
    let found_terrain =
        Terrain::from_u8(found_terrain).ok_or(WorldError::InvalidTerrainId(found_terrain))?;
    if found_terrain != terrain {
        return Err(WorldError::TerrainMismatch {
            expected: terrain,
            found: found_terrain,
        });
    }
    Ok(())
}

/// Validates the journal header and returns its first tick.
fn read_log_header(file: &mut File, seed: u64, terrain: Terrain) -> Result<u64, WorldError> {
    let mut header = [0u8; HEADER_SIZE];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            // The header is installed atomically, so a short file lost data.
            WorldError::Corrupted("journal header is truncated".into())
        } else {
            WorldError::Io(error)
        }
    })?;

    if &header[0..8] != JOURNAL_MAGIC {
        return Err(WorldError::InvalidMagic);
    }
    if crc32(&[&header[0..28]]) != be_u32(&header[28..32]) {
        return Err(WorldError::Corrupted(
            "journal header CRC32 mismatch".into(),
        ));
    }
    check_identity(be_u64(&header[8..16]), header[16], seed, terrain)?;
    Ok(be_u64(&header[20..28]))
}

/// Outcome of scanning `journal.log`.
struct Replay {
    /// Tick the next record in this file would carry.
    next_tick: u64,
    /// End of the last valid record.
    valid_end: u64,
}

/// Replays records from `file`, applying those at or after `checkpoint_tick`.
/// Earlier records are checked the same way but are already in the checkpoint.
fn replay_log(
    file: &File,
    file_len: u64,
    start_tick: u64,
    checkpoint_tick: u64,
    mutations: &mut BTreeMap<(i64, i64, i64), Block>,
) -> Result<Replay, WorldError> {
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(HEADER_SIZE as u64))?;
    let mut next_tick = start_tick;
    let mut valid_end = HEADER_SIZE as u64;

    loop {
        let remaining = file_len - valid_end;
        if remaining < RECORD_HEADER_SIZE as u64 {
            // A partial header cannot hold a further record, so it is torn.
            break;
        }

        let mut header = [0u8; RECORD_HEADER_SIZE];
        reader.read_exact(&mut header)?;
        let Some((tick, count)) = parse_record_header(&header) else {
            if remaining > MAX_RECORD_SIZE as u64 {
                return Err(WorldError::Corrupted(format!(
                    "record header CRC32 mismatch at offset {valid_end}, \
                     {remaining} bytes exceed one record"
                )));
            }
            let mut tail = header.to_vec();
            reader.read_to_end(&mut tail)?;
            if contains_later_record(&tail) {
                return Err(WorldError::Corrupted(format!(
                    "record header CRC32 mismatch at offset {valid_end} before a valid record"
                )));
            }
            break;
        };

        if tick != next_tick {
            return Err(WorldError::Corrupted(format!(
                "non-monotonic sequence ID {tick}, expected {next_tick} at offset {valid_end}"
            )));
        }
        let count = count as usize;
        if count > MAX_MUTATIONS_PER_TICK {
            return Err(WorldError::Corrupted(format!(
                "unreasonable mutation count {count} at offset {valid_end}"
            )));
        }

        let payload_len = count * MUTATION_SIZE;
        let record_len = (RECORD_HEADER_SIZE + payload_len + RECORD_CRC_SIZE) as u64;
        if remaining < record_len {
            // The authenticated length runs past EOF: an incomplete final write.
            break;
        }

        let mut body = vec![0u8; payload_len + RECORD_CRC_SIZE];
        reader.read_exact(&mut body)?;
        let computed_crc = crc32(&[&header, &body[..payload_len]]);
        let stored_crc = be_u32(&body[payload_len..]);
        if computed_crc != stored_crc {
            if remaining == record_len {
                // File size can reach disk before the data does.
                break;
            }
            return Err(WorldError::Corrupted(format!(
                "checksum mismatch at offset {valid_end}: \
                 computed {computed_crc:#x}, stored {stored_crc:#x}"
            )));
        }

        let mut decoded = Vec::with_capacity(count);
        for entry in body[..payload_len].chunks_exact(MUTATION_SIZE) {
            let Some(block) = Block::from_u8(entry[24]) else {
                return Err(WorldError::Corrupted(format!(
                    "invalid block ID {} at offset {valid_end}",
                    entry[24]
                )));
            };
            decoded.push((
                (
                    be_i64(&entry[0..8]),
                    be_i64(&entry[8..16]),
                    be_i64(&entry[16..24]),
                ),
                block,
            ));
        }
        if tick >= checkpoint_tick {
            mutations.extend(decoded);
        }

        next_tick = next_tick_after(tick)?;
        valid_end += record_len;
    }

    Ok(Replay {
        next_tick,
        valid_end,
    })
}

/// Removes temporary files left by an interrupted install. They were never
/// renamed into place, so they hold nothing recovery depends on.
fn remove_orphaned_temporaries(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(CHECKPOINT_TEMP_PREFIX) || name.starts_with(JOURNAL_TEMP_PREFIX) {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Atomically replaces `journal.log` with an empty journal starting at
/// `start_tick` and returns a handle positioned for appending. The handle is
/// only safe to use once the returned result is `Ok`: until the directory sync
/// succeeds a power loss can restore the previous file.
fn install_empty_log(
    dir: &Path,
    seed: u64,
    terrain: Terrain,
    start_tick: u64,
) -> std::io::Result<File> {
    let temp_path = dir.join(format!("{JOURNAL_TEMP_PREFIX}{}", uuid::Uuid::new_v4()));
    let installed = (|| -> std::io::Result<File> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&temp_path)?;
        io_step("log.write")?;
        file.write_all(&log_header(seed, terrain, start_tick))?;
        io_step("log.sync")?;
        file.sync_all()?;
        io_step("log.rename")?;
        std::fs::rename(&temp_path, dir.join(JOURNAL_NAME))?;
        Ok(file)
    })();
    let file = installed.inspect_err(|_: &std::io::Error| {
        let _ = std::fs::remove_file(&temp_path);
    })?;
    sync_dir(dir, "log.dir_sync")?;
    Ok(file)
}

/// Manages `journal.log` and `checkpoint.bin` with CRC32 checksums,
/// crash recovery, and atomic checkpoint compaction.
#[derive(Debug)]
pub struct WorldJournal {
    dir: PathBuf,
    log_file: File,
    seed: u64,
    terrain: Terrain,
    /// End of the last acknowledged record.
    log_end: u64,
    /// Tick the next appended record must carry.
    next_tick: u64,
    /// Set after a failure that leaves the on-disk state unknown. Retrying
    /// could acknowledge data the kernel has already dropped, so only
    /// reopening, which rereads the file, clears it.
    unusable: bool,
}

impl WorldJournal {
    /// Opens the world journal in `dir`. Recovers any existing checkpoint,
    /// replays valid WAL records from `journal.log`, truncates a torn final
    /// write, and finishes a compaction that was interrupted by a crash.
    pub fn open(dir: &Path, seed: u64, terrain: Terrain) -> Result<JournalRecovery, WorldError> {
        if !dir.is_dir() {
            std::fs::create_dir_all(dir)?;
            let parent = dir.parent().filter(|path| !path.as_os_str().is_empty());
            sync_dir(parent.unwrap_or(Path::new(".")), "open.parent_dir_sync")?;
        }
        remove_orphaned_temporaries(dir)?;

        let checkpoint_path = dir.join(CHECKPOINT_NAME);
        let log_path = dir.join(JOURNAL_NAME);
        let has_checkpoint = checkpoint_path.is_file();
        let (mut mutations, checkpoint_tick) = if has_checkpoint {
            read_checkpoint(&checkpoint_path, seed, terrain)?
        } else {
            (BTreeMap::new(), 0)
        };

        if !log_path.is_file() {
            if has_checkpoint {
                // The journal is created before any checkpoint and is only
                // ever replaced atomically, so ticks may be missing.
                return Err(WorldError::Corrupted(
                    "checkpoint exists but journal.log is missing".into(),
                ));
            }
            let file = install_empty_log(dir, seed, terrain, 0)?;
            return Ok((
                Self::new(dir, file, seed, terrain, HEADER_SIZE as u64, 0),
                mutations,
                0,
            ));
        }

        let mut file = OpenOptions::new().read(true).write(true).open(&log_path)?;
        let file_len = file.metadata()?.len();
        let start_tick = read_log_header(&mut file, seed, terrain)?;
        if start_tick > checkpoint_tick {
            return Err(WorldError::Corrupted(format!(
                "journal starts at tick {start_tick} but the checkpoint ends at \
                 {checkpoint_tick}; acknowledged ticks are missing"
            )));
        }

        let replay = replay_log(&file, file_len, start_tick, checkpoint_tick, &mut mutations)?;

        if replay.next_tick <= checkpoint_tick && start_tick != checkpoint_tick {
            // The checkpoint covers the whole journal: a crash stopped
            // compaction before it replaced the file. Confirm the rename first
            // because the journal must outlive an unconfirmed checkpoint.
            sync_dir(dir, "open.checkpoint_dir_sync")?;
            let file = install_empty_log(dir, seed, terrain, checkpoint_tick)?;
            return Ok((
                Self::new(
                    dir,
                    file,
                    seed,
                    terrain,
                    HEADER_SIZE as u64,
                    checkpoint_tick,
                ),
                mutations,
                checkpoint_tick,
            ));
        }

        if file_len > replay.valid_end {
            io_step("open.truncate")?;
            file.set_len(replay.valid_end)?;
        }
        // Replayed records may only have reached the page cache before a
        // process crash, so make them durable before exposing them.
        io_step("open.sync")?;
        file.sync_all()?;

        Ok((
            Self::new(dir, file, seed, terrain, replay.valid_end, replay.next_tick),
            mutations,
            replay.next_tick,
        ))
    }

    fn new(
        dir: &Path,
        log_file: File,
        seed: u64,
        terrain: Terrain,
        log_end: u64,
        next_tick: u64,
    ) -> Self {
        Self {
            dir: dir.to_path_buf(),
            log_file,
            seed,
            terrain,
            log_end,
            next_tick,
            unusable: false,
        }
    }

    fn ensure_usable(&self) -> Result<(), WorldError> {
        if self.unusable {
            return Err(WorldError::JournalUnusable);
        }
        Ok(())
    }

    /// Appends a tick batch WAL record and syncs it to disk. `Ok` is the
    /// durability acknowledgement. After `Err` from an I/O step the journal is
    /// unusable until reopened; see the module documentation.
    pub fn append_tick(
        &mut self,
        tick: u64,
        mutations: &[BlockMutation],
    ) -> Result<(), WorldError> {
        self.ensure_usable()?;
        if tick != self.next_tick {
            return Err(WorldError::TickOutOfSequence {
                expected: self.next_tick,
                found: tick,
            });
        }
        if mutations.len() > MAX_MUTATIONS_PER_TICK {
            return Err(WorldError::TooManyMutations {
                count: mutations.len(),
                limit: MAX_MUTATIONS_PER_TICK,
            });
        }
        let next_tick = next_tick_after(tick)?;

        let count = mutations.len() as u32;
        let mut record =
            Vec::with_capacity(RECORD_HEADER_SIZE + mutations.len() * MUTATION_SIZE + 4);
        record.extend_from_slice(&tick.to_be_bytes());
        record.extend_from_slice(&count.to_be_bytes());
        let header_crc = crc32(&[&record]);
        record.extend_from_slice(&header_crc.to_be_bytes());
        for m in mutations {
            record.extend_from_slice(&m.x.to_be_bytes());
            record.extend_from_slice(&m.y.to_be_bytes());
            record.extend_from_slice(&m.z.to_be_bytes());
            record.push(m.block as u8);
        }
        let crc = crc32(&[&record]);
        record.extend_from_slice(&crc.to_be_bytes());

        if let Err(error) = self.write_record(&record) {
            self.unusable = true;
            // Best effort: if this fails too, recovery keeps or truncates the
            // leftover bytes and the unusable flag prevents a duplicate tick.
            let _ = self.roll_back_append();
            return Err(error.into());
        }

        self.log_end += record.len() as u64;
        self.next_tick = next_tick;
        Ok(())
    }

    fn write_record(&mut self, record: &[u8]) -> std::io::Result<()> {
        self.log_file.seek(SeekFrom::Start(self.log_end))?;
        #[cfg(test)]
        if faults::should_fail("append.partial_write") {
            self.log_file.write_all(&record[..record.len() / 2])?;
            return Err(std::io::Error::other("injected partial write"));
        }
        io_step("append.write")?;
        self.log_file.write_all(record)?;
        io_step("append.sync")?;
        self.log_file.sync_data()
    }

    fn roll_back_append(&mut self) -> std::io::Result<()> {
        io_step("append.rollback")?;
        self.log_file.set_len(self.log_end)?;
        self.log_file.sync_all()
    }

    /// Syncs the journal to disk. Every acknowledged tick is already durable,
    /// so this only reports whether the file can still be synced.
    pub fn flush(&mut self) -> Result<(), WorldError> {
        self.ensure_usable()?;
        let synced = io_step("flush.sync").and_then(|()| self.log_file.sync_data());
        if let Err(error) = synced {
            self.unusable = true;
            return Err(error.into());
        }
        Ok(())
    }

    /// Atomically compacts all committed mutations into `checkpoint.bin`, then
    /// replaces `journal.log` with an empty journal starting at `current_tick`.
    ///
    /// The old journal is discarded only after the checkpoint rename is
    /// confirmed durable. A failure before that point leaves the journal
    /// usable, because it still holds everything; a failure while replacing
    /// the journal makes it unusable until reopened.
    pub fn compact(
        &mut self,
        current_tick: u64,
        committed_mutations: &BTreeMap<(i64, i64, i64), Block>,
    ) -> Result<(), WorldError> {
        self.ensure_usable()?;
        if current_tick != self.next_tick {
            return Err(WorldError::TickOutOfSequence {
                expected: self.next_tick,
                found: current_tick,
            });
        }

        self.install_checkpoint(current_tick, committed_mutations)?;

        match install_empty_log(&self.dir, self.seed, self.terrain, current_tick) {
            Ok(file) => {
                self.log_file = file;
                self.log_end = HEADER_SIZE as u64;
                Ok(())
            }
            Err(error) => {
                self.unusable = true;
                Err(error.into())
            }
        }
    }

    fn install_checkpoint(
        &self,
        current_tick: u64,
        committed_mutations: &BTreeMap<(i64, i64, i64), Block>,
    ) -> Result<(), WorldError> {
        let count = u32::try_from(committed_mutations.len()).map_err(|_| {
            WorldError::Corrupted("checkpoint mutation count exceeds the file format".into())
        })?;
        let mut data =
            Vec::with_capacity(HEADER_SIZE + committed_mutations.len() * MUTATION_SIZE + 4);

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

        let crc = crc32(&[&data]);
        data.extend_from_slice(&crc.to_be_bytes());

        let temp_path = self
            .dir
            .join(format!("{CHECKPOINT_TEMP_PREFIX}{}", uuid::Uuid::new_v4()));
        let installed = (|| -> std::io::Result<()> {
            let mut temp_file = File::create(&temp_path)?;
            io_step("checkpoint.write")?;
            temp_file.write_all(&data)?;
            io_step("checkpoint.sync")?;
            temp_file.sync_all()?;
            io_step("checkpoint.rename")?;
            std::fs::rename(&temp_path, self.dir.join(CHECKPOINT_NAME))
        })();
        installed.inspect_err(|_: &std::io::Error| {
            let _ = std::fs::remove_file(&temp_path);
        })?;
        sync_dir(&self.dir, "checkpoint.dir_sync")?;
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

fn log_header(seed: u64, terrain: Terrain, start_tick: u64) -> [u8; HEADER_SIZE] {
    let mut header = [0u8; HEADER_SIZE];
    header[0..8].copy_from_slice(JOURNAL_MAGIC);
    header[8..16].copy_from_slice(&seed.to_be_bytes());
    header[16] = terrain.to_u8();
    header[20..28].copy_from_slice(&start_tick.to_be_bytes());
    let crc = crc32(&[&header[0..28]]);
    header[28..32].copy_from_slice(&crc.to_be_bytes());
    header
}
