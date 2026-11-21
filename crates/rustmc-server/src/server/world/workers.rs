//! The chunk worker pool: generates and encodes columns off the game thread.
//!
//! Why: generation and encoding are the expensive part of streaming; the game
//! loop only asks for positions and receives finished columns.
//! Done: a fixed pool sized by configuration, a bounded request queue,
//! results delivered to the game loop's inbox. Not done: loading saved
//! columns (track C reads them before generating).

use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use super::store::ReadyChunk;
use super::terrain::{TerrainPalette, generate};
use crate::chunk_adapter::{RegistryTables, encode_chunk_body};
use crate::java_packets::play::clientbound::LEVEL_CHUNK_WITH_LIGHT;
use crate::net::framing::Packet;
use crate::server::connection::Event;
use crate::world::Generator;

/// Requests that may wait for a worker before `request` reports the queue
/// full; the store retries those positions on a later tick.
pub const REQUEST_QUEUE: usize = 1_024;

/// Handle to the worker pool.
pub struct ChunkWorkers {
    requests: SyncSender<(i32, i32)>,
}

impl ChunkWorkers {
    /// Starts `count` workers that send finished columns to `results`.
    pub fn spawn(
        count: usize,
        generator: Generator,
        palette: Arc<TerrainPalette>,
        tables: Arc<RegistryTables>,
        results: SyncSender<Event>,
    ) -> std::io::Result<Self> {
        let (requests, queue) = sync_channel(REQUEST_QUEUE);
        let queue = Arc::new(Mutex::new(queue));
        for index in 0..count {
            let queue = Arc::clone(&queue);
            let palette = Arc::clone(&palette);
            let tables = Arc::clone(&tables);
            let results = results.clone();
            thread::Builder::new()
                .name(format!("chunk-worker-{index}"))
                .spawn(move || work(&queue, generator, &palette, &tables, &results))?;
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

fn work(
    queue: &Mutex<Receiver<(i32, i32)>>,
    generator: Generator,
    palette: &TerrainPalette,
    tables: &RegistryTables,
    results: &SyncSender<Event>,
) {
    loop {
        // The receiver holds no invariant a panicking holder could break.
        let next = queue.lock().unwrap_or_else(PoisonError::into_inner).recv();
        let Ok((x, z)) = next else {
            return;
        };
        let column = generate(generator, palette, x, z);
        let packet = encode(&column, tables);
        let ready = ReadyChunk { column, packet };
        if results.send(Event::ChunkReady(Box::new(ready))).is_err() {
            return;
        }
    }
}

/// Encodes a column as a Level Chunk With Light packet.
pub fn encode(column: &super::column::Column, tables: &RegistryTables) -> Option<Arc<Packet>> {
    let adapter = column.to_adapter().ok()?;
    let body = encode_chunk_body(&adapter, tables).ok()?;
    Some(Arc::new(Packet {
        id: LEVEL_CHUNK_WITH_LIGHT,
        body,
    }))
}
