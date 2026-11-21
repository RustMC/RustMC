//! The block-change journal: every edit since the last save, on disk.
//!
//! Why: region files are rewritten only at autosave and shutdown; the journal
//! makes each tick's edits durable in between, so a crash loses at most the
//! tick in progress.
//! Done: fixed-size records with a CRC each, appended and synced once per
//! tick; replay that stops at the first incomplete or damaged record (the
//! tail a crash can leave); reset after a successful save. Not done: edits
//! other than block states (block entities, entities).

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use super::region_file::{create_dir_durably, replace_atomically};

/// Bytes per record: x, y, z (i32), state (u32) and a CRC-32 of those 16.
pub const RECORD_BYTES: usize = 20;

/// One block change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEdit {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub state: u32,
}

impl BlockEdit {
    fn encode(self) -> [u8; RECORD_BYTES] {
        let mut record = [0u8; RECORD_BYTES];
        record[0..4].copy_from_slice(&self.x.to_be_bytes());
        record[4..8].copy_from_slice(&self.y.to_be_bytes());
        record[8..12].copy_from_slice(&self.z.to_be_bytes());
        record[12..16].copy_from_slice(&self.state.to_be_bytes());
        let crc = crc32fast::hash(&record[..16]);
        record[16..20].copy_from_slice(&crc.to_be_bytes());
        record
    }

    fn decode(record: &[u8; RECORD_BYTES]) -> Option<Self> {
        let word = |at: usize| [record[at], record[at + 1], record[at + 2], record[at + 3]];
        if crc32fast::hash(&record[..16]) != u32::from_be_bytes(word(16)) {
            return None;
        }
        Some(Self {
            x: i32::from_be_bytes(word(0)),
            y: i32::from_be_bytes(word(4)),
            z: i32::from_be_bytes(word(8)),
            state: u32::from_be_bytes(word(12)),
        })
    }
}

/// The open journal of one world.
pub struct Journal {
    path: PathBuf,
    writer: BufWriter<File>,
    pending: bool,
}

impl Journal {
    /// Opens (creating if needed) the journal at `path` and returns it with
    /// every complete edit already in it, oldest first.
    pub fn open(path: &Path) -> io::Result<(Self, Vec<BlockEdit>)> {
        if let Some(directory) = path.parent() {
            create_dir_durably(directory)?;
        }
        let mut bytes = Vec::new();
        match File::open(path) {
            Ok(mut file) => {
                file.read_to_end(&mut bytes)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // A new journal is created through an atomic replace, which
                // syncs its directory (itself created durably above).
                replace_atomically(path, &[])?;
            }
            Err(error) => return Err(error),
        }
        let mut edits = Vec::new();
        let (records, _torn) = bytes.as_chunks::<RECORD_BYTES>();
        for record in records {
            match BlockEdit::decode(record) {
                Some(edit) => edits.push(edit),
                None => break,
            }
        }
        // Rewrite without any damaged tail so new records follow valid ones.
        let valid: Vec<u8> = edits.iter().flat_map(|edit| edit.encode()).collect();
        if valid.len() != bytes.len() {
            replace_atomically(path, &[&valid])?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok((
            Self {
                path: path.to_path_buf(),
                writer: BufWriter::new(file),
                pending: false,
            },
            edits,
        ))
    }

    /// Queues one edit; it is durable after the next `sync`.
    pub fn append(&mut self, edit: BlockEdit) -> io::Result<()> {
        self.pending = true;
        self.writer.write_all(&edit.encode())
    }

    /// Flushes and syncs queued edits, if any (once per tick).
    pub fn sync(&mut self) -> io::Result<()> {
        if !self.pending {
            return Ok(());
        }
        self.writer.flush()?;
        self.writer.get_ref().sync_data()?;
        self.pending = false;
        Ok(())
    }

    /// Empties the journal after every journaled edit reached the region
    /// files.
    pub fn reset(&mut self) -> io::Result<()> {
        self.writer.flush()?;
        replace_atomically(&self.path, &[])?;
        self.writer = BufWriter::new(OpenOptions::new().append(true).open(&self.path)?);
        self.pending = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
