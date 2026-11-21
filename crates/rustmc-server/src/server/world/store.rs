//! The game loop's chunk store: loaded columns, their shared encoded packets
//! and outstanding generation requests.
//!
//! Why: every player near a column receives the same packet, so it is
//! encoded once and shared; edits invalidate it.
//! Done: request de-duplication, packet sharing, tracking which columns
//! changed since they were last saved, unloading of saved columns nobody
//! views. Not done: nothing known.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::column::Column;
use super::workers::{ChunkWorkers, encode};
use crate::chunk_adapter::RegistryTables;
use crate::net::framing::Packet;

/// A column a worker finished.
#[derive(Debug)]
pub struct ReadyChunk {
    pub column: Column,
    pub packet: Option<Arc<Packet>>,
    /// The saved copy could not be read; this column must never be saved.
    pub damaged: bool,
}

struct Stored {
    column: Column,
    packet: Option<Arc<Packet>>,
    /// Changed since it was last saved: kept loaded until it is saved.
    dirty: bool,
    damaged: bool,
}

/// Loaded columns and pending requests.
pub struct ChunkStore {
    columns: HashMap<(i32, i32), Stored>,
    requested: HashSet<(i32, i32)>,
    workers: ChunkWorkers,
    tables: Arc<RegistryTables>,
}

impl ChunkStore {
    pub fn new(workers: ChunkWorkers, tables: Arc<RegistryTables>) -> Self {
        Self {
            columns: HashMap::new(),
            requested: HashSet::new(),
            workers,
            tables,
        }
    }

    /// Asks for a column unless it is loaded or already requested.
    pub fn request(&mut self, position: (i32, i32)) {
        if self.columns.contains_key(&position) || self.requested.contains(&position) {
            return;
        }
        if self.workers.request(position) {
            self.requested.insert(position);
        }
    }

    /// Stores a finished column.
    pub fn insert(&mut self, ready: ReadyChunk) {
        let position = (ready.column.x, ready.column.z);
        self.requested.remove(&position);
        self.columns.entry(position).or_insert(Stored {
            column: ready.column,
            packet: ready.packet,
            dirty: false,
            damaged: ready.damaged,
        });
    }

    /// The shared chunk packet, re-encoding after an edit.
    pub fn packet(&mut self, position: (i32, i32)) -> Option<Arc<Packet>> {
        let stored = self.columns.get_mut(&position)?;
        if stored.packet.is_none() {
            stored.packet = encode(&stored.column, &self.tables);
        }
        stored.packet.clone()
    }

    pub fn column(&self, position: (i32, i32)) -> Option<&Column> {
        self.columns.get(&position).map(|s| &s.column)
    }

    /// Changes one block; returns the previous state, or `None` when the
    /// column is not loaded or the position is out of range.
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) -> Option<u32> {
        let stored = self
            .columns
            .get_mut(&(x.div_euclid(16), z.div_euclid(16)))?;
        let previous = stored.column.set(
            x.rem_euclid(16) as usize,
            y,
            z.rem_euclid(16) as usize,
            state,
        )?;
        if previous != state {
            stored.packet = None;
            stored.dirty = !stored.damaged;
        }
        Some(previous)
    }

    pub fn block(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.column((x.div_euclid(16), z.div_euclid(16)))?.get(
            x.rem_euclid(16) as usize,
            y,
            z.rem_euclid(16) as usize,
        )
    }

    /// Unloads saved columns for which `viewed` is false.
    pub fn unload_unviewed(&mut self, viewed: impl Fn((i32, i32)) -> bool) {
        self.columns
            .retain(|position, stored| stored.dirty || viewed(*position));
    }

    /// Whether a column is loaded or on its way.
    pub fn is_known(&self, position: (i32, i32)) -> bool {
        self.columns.contains_key(&position) || self.requested.contains(&position)
    }

    /// Columns changed since they were last saved.
    pub fn dirty_columns(&self) -> impl Iterator<Item = &Column> {
        self.columns
            .values()
            .filter(|stored| stored.dirty)
            .map(|stored| &stored.column)
    }

    /// Records that these columns now match their saved copies.
    pub fn mark_saved(&mut self, positions: &[(i32, i32)]) {
        for position in positions {
            if let Some(stored) = self.columns.get_mut(position) {
                stored.dirty = false;
            }
        }
    }

    /// Loaded columns and their memory, for the load test and logs.
    pub fn stats(&self) -> (usize, usize) {
        let bytes = self.columns.values().map(|s| s.column.memory_bytes()).sum();
        (self.columns.len(), bytes)
    }
}
