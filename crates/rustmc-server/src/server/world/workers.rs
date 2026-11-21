//! The chunk worker pool: loads or generates and encodes columns off the game
//! thread.
//!
//! Why: reading, generating and encoding are the expensive part of streaming;
//! the game loop only asks for positions and receives finished columns.
//! Done: a fixed pool sized by configuration, a bounded request queue, saved
//! columns loaded before anything is generated (a damaged one is served as
//! generated terrain and never saved), results delivered to the game loop's
//! inbox. Not done: nothing known.

use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use super::column::Column;
use super::store::ReadyChunk;
use super::terrain::{TerrainPalette, generate};
use crate::chunk_adapter::{RegistryTables, encode_chunk_body};
use crate::java_packets::play::clientbound::LEVEL_CHUNK_WITH_LIGHT;
use crate::net::framing::Packet;
use crate::server::connection::Event;
use crate::server::log;
use crate::storage::world_files::{StorageError, WorldFiles};
use crate::world::Generator;

/// Requests that may wait for a worker before `request` reports the queue
/// full; the store retries those positions on a later tick.
pub const REQUEST_QUEUE: usize = 1_024;

/// Where columns come from: the world's saved chunks, else the generator.
pub struct ColumnSource {
    pub files: WorldFiles,
    pub generator: Generator,
    pub palette: TerrainPalette,
    pub tables: Arc<RegistryTables>,
}

impl ColumnSource {
    /// The saved column, or a freshly generated one if it was never saved.
    pub fn column(&self, x: i32, z: i32) -> Result<Column, StorageError> {
        match self.files.load_column(x, z, &self.tables)? {
            Some(column) => Ok(column),
            None => Ok(self.generated(x, z)),
        }
    }

    /// The generated column, ignoring any saved copy.
    pub fn generated(&self, x: i32, z: i32) -> Column {
        generate(self.generator, &self.palette, x, z)
    }
}

/// Handle to the worker pool.
pub struct ChunkWorkers {
    requests: SyncSender<(i32, i32)>,
}

impl ChunkWorkers {
    /// Starts `count` workers that send finished columns to `results`.
    pub fn spawn(
        count: usize,
        source: Arc<ColumnSource>,
        results: SyncSender<Event>,
    ) -> std::io::Result<Self> {
        let (requests, queue) = sync_channel(REQUEST_QUEUE);
        let queue = Arc::new(Mutex::new(queue));
        for index in 0..count {
            let queue = Arc::clone(&queue);
            let source = Arc::clone(&source);
            let results = results.clone();
            thread::Builder::new()
                .name(format!("chunk-worker-{index}"))
                .spawn(move || work(&queue, &source, &results))?;
        }
        Ok(Self { requests })
    }

    /// Queues a column; `false` when the queue is full.
    pub fn request(&self, position: (i32, i32)) -> bool {
        !matches!(
            self.requests.try_send(position),
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_))
        )
    }
}

fn work(queue: &Mutex<Receiver<(i32, i32)>>, source: &ColumnSource, results: &SyncSender<Event>) {
    loop {
        // The receiver holds no invariant a panicking holder could break.
        let next = queue.lock().unwrap_or_else(PoisonError::into_inner).recv();
        let Ok((x, z)) = next else {
            return;
        };
        // A damaged saved chunk is served as generated terrain so players are
        // not stuck, but marked so it is never saved over the damaged data.
        let (column, damaged) = match source.column(x, z) {
            Ok(column) => (column, false),
            Err(error) => {
                log::event(
                    "chunk_load_failed",
                    &[("x", &x), ("z", &z), ("error", &error)],
                );
                (source.generated(x, z), true)
            }
        };
        let packet = encode(&column, &source.tables);
        let ready = ReadyChunk {
            column,
            packet,
            damaged,
        };
        if results.send(Event::ChunkReady(Box::new(ready))).is_err() {
            return;
        }
    }
}

/// Encodes a column as a Level Chunk With Light packet.
pub fn encode(column: &Column, tables: &RegistryTables) -> Option<Arc<Packet>> {
    let adapter = column.to_adapter().ok()?;
    let body = encode_chunk_body(&adapter, tables).ok()?;
    Some(Arc::new(Packet {
        id: LEVEL_CHUNK_WITH_LIGHT,
        body,
    }))
}
