//! What one player sees: the chunks around them, sent in priority order and
//! paced by the client's batch acknowledgements.
//!
//! Why: streaming must keep up with a moving player without flooding a slow
//! client or the network.
//! Done: square view of the configured radius, nearest-first order with the
//! direction of travel first, one batch in flight at a time sized by the
//! client's reported rate, Forget Level Chunk for chunks that leave the view.
//! Not done: per-player view distance changes after join.
//!
//! Source: minecraft.wiki `Java Edition protocol/Packets` — Chunk Batch Start
//! (no fields), Chunk Batch Finished (Batch Size VarInt), Chunk Batch
//! Received (Chunks per tick Float), Unload Chunk (chunk Z Int, chunk X Int).

use std::collections::HashSet;
use std::sync::Arc;

use super::store::ChunkStore;
use crate::java_packets::play::clientbound;
use crate::java_preview::ordered_view;
use crate::java_wire::put_varint;
use crate::net::framing::Packet;
use crate::server::connection::Outgoing;
use crate::server::join::chunk_cache_center;

/// Chunks in the first batch, before the client reports a rate.
pub const INITIAL_BATCH: usize = 9;
/// Most chunks in one batch, whatever rate the client reports; bounds the
/// burst a single player can put on the outbound queue.
pub const MAX_BATCH: usize = 64;
/// Unsent positions examined per batch, in priority order: ready ones are
/// sent and missing ones requested, so generation runs ahead of sending.
pub const REQUEST_AHEAD: usize = 128;

/// One player's view of the world.
#[derive(Debug)]
pub struct View {
    center: (i32, i32),
    radius: i32,
    order: Vec<(i32, i32)>,
    sent: HashSet<(i32, i32)>,
    batch_in_flight: bool,
    batch_size: usize,
}

/// The chunk a block position is in.
pub fn chunk_of(x: f64, z: f64) -> (i32, i32) {
    (
        (x.floor() as i32).div_euclid(16),
        (z.floor() as i32).div_euclid(16),
    )
}

impl View {
    pub fn new(center: (i32, i32), radius: u8) -> Self {
        let radius = i32::from(radius);
        Self {
            center,
            radius,
            order: ordered_view(center, radius, (0, 0)),
            sent: HashSet::new(),
            batch_in_flight: false,
            batch_size: INITIAL_BATCH,
        }
    }

    pub fn center(&self) -> (i32, i32) {
        self.center
    }

    /// Whether `position` is inside the view square.
    pub fn contains(&self, (x, z): (i32, i32)) -> bool {
        (x - self.center.0).abs() <= self.radius && (z - self.center.1).abs() <= self.radius
    }

    /// Whether the client already has `position`.
    pub fn has_sent(&self, position: (i32, i32)) -> bool {
        self.sent.contains(&position)
    }

    /// Moves the view; returns the centre update and the forgets for chunks
    /// that left it.
    pub fn recenter(&mut self, center: (i32, i32)) -> Vec<Outgoing> {
        if center == self.center {
            return Vec::new();
        }
        let direction = (
            (center.0 - self.center.0).signum(),
            (center.1 - self.center.1).signum(),
        );
        self.center = center;
        self.order = ordered_view(center, self.radius, direction);
        let mut out = vec![Outgoing::Packet(chunk_cache_center(center.0, center.1))];
        let gone: Vec<(i32, i32)> = self
            .sent
            .iter()
            .copied()
            .filter(|p| !self.contains(*p))
            .collect();
        for (x, z) in gone {
            self.sent.remove(&(x, z));
            let mut body = z.to_be_bytes().to_vec();
            body.extend(x.to_be_bytes());
            out.push(Outgoing::Packet(Packet {
                id: clientbound::FORGET_LEVEL_CHUNK,
                body,
            }));
        }
        out
    }

    /// The client acknowledged a batch and asks for `chunks_per_tick`.
    pub fn batch_received(&mut self, chunks_per_tick: f32) {
        self.batch_in_flight = false;
        if chunks_per_tick.is_finite() && chunks_per_tick > 0.0 {
            self.batch_size = (chunks_per_tick.ceil() as usize).clamp(1, MAX_BATCH);
        }
    }

    /// The next batch to send, if the client is ready for one. Missing
    /// columns ahead of the cursor are requested from `store`.
    pub fn next_batch(&mut self, store: &mut ChunkStore) -> Option<Vec<Outgoing>> {
        if self.batch_in_flight {
            return None;
        }
        let mut chunks: Vec<Arc<Packet>> = Vec::new();
        // Look at the next unsent positions in priority order: send the
        // ready ones (up to the batch size) and request the rest.
        let unsent: Vec<(i32, i32)> = self
            .order
            .iter()
            .copied()
            .filter(|p| !self.sent.contains(p))
            .take(REQUEST_AHEAD)
            .collect();
        for position in unsent {
            match store.packet(position) {
                Some(packet) if chunks.len() < self.batch_size => {
                    chunks.push(packet);
                    self.sent.insert(position);
                }
                Some(_) => {}
                None => store.request(position),
            }
        }
        if chunks.is_empty() {
            return None;
        }
        self.batch_in_flight = true;
        let count = chunks.len();
        let mut out = Vec::with_capacity(count + 2);
        out.push(Outgoing::Packet(Packet {
            id: clientbound::CHUNK_BATCH_START,
            body: Vec::new(),
        }));
        out.extend(chunks.into_iter().map(Outgoing::Shared));
        let mut size = Vec::new();
        put_varint(count as u32, &mut size);
        out.push(Outgoing::Packet(Packet {
            id: clientbound::CHUNK_BATCH_FINISHED,
            body: size,
        }));
        Some(out)
    }
}

#[cfg(test)]
mod tests;
