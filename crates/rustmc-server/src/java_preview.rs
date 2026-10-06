//! Independent Java 26.3 wire adapter for the immutable local terrain preview.
//! IDs and field layouts: official 26.3 packet/block reports and codec metadata.

use crate::{
    discovery_java::{frame, put_string, put_varint},
    preview_data::RegistryManifest,
    world::{Biome, Block, Chunk, Generator},
};
use std::collections::BTreeSet;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const MOVING_NEAR_RADIUS: i32 = 3;
const MOVING_PRIORITY_WINDOW: Duration = Duration::from_millis(750);

fn packed_center((x, z): (i32, i32)) -> u64 {
    (u64::from(z as u32) << 32) | u64::from(x as u32)
}

fn unpacked_center(value: u64) -> (i32, i32) {
    (value as u32 as i32, (value >> 32) as u32 as i32)
}

/// Validated operator-local input. Runtime preflight checks the files before binding.
#[derive(Debug, Clone)]
pub struct VanillaSource {
    pub data_root: PathBuf,
    pub registry_table: PathBuf,
    pub seed: i64,
    pub spawn_y: i32,
    pub cache: Option<crate::preview_cache::PreviewCache>,
    pub workers: usize,
}

struct BuiltChunk {
    position: (i32, i32),
    packet: Result<Option<Vec<u8>>, String>,
    generation_us: u128,
    encoding_us: u128,
}

struct ChunkRequest {
    position: (i32, i32),
    center: (i32, i32),
}

/// A fixed pool with one chunk per worker. Each generator stays on its owning thread.
struct VanillaWorker {
    requests: Option<mpsc::SyncSender<ChunkRequest>>,
    results: mpsc::Receiver<BuiltChunk>,
    handles: Vec<thread::JoinHandle<()>>,
    in_flight: BTreeSet<(i32, i32)>,
    pending: Option<BuiltChunk>,
    capacity: usize,
    current_center: Arc<AtomicU64>,
    cache: Option<crate::preview_cache::PreviewCache>,
}

impl VanillaWorker {
    fn new(source: VanillaSource) -> Self {
        let capacity = source.workers;
        let (requests, rx) = mpsc::sync_channel::<ChunkRequest>(0);
        let (tx, results) = mpsc::sync_channel(capacity);
        let rx = Arc::new(Mutex::new(rx));
        let current_center = Arc::new(AtomicU64::new(packed_center((0, 0))));
        let handles = (0..capacity)
            .map(|_| {
                let rx = Arc::clone(&rx);
                let tx = tx.clone();
                let source = source.clone();
                let current_center = Arc::clone(&current_center);
                thread::spawn(move || {
                    let Ok(generator) = crate::vanilla::generator::VanillaGenerator::new(
                        &source.data_root,
                        source.seed,
                        "minecraft:overworld",
                    ) else {
                        return;
                    };
                    let Ok(text) = std::fs::read_to_string(&source.registry_table) else {
                        return;
                    };
                    let Ok(tables) =
                        crate::chunk_adapter::registry::RegistryTables::from_provisioned(&text)
                    else {
                        return;
                    };
                    loop {
                        let ChunkRequest {
                            position,
                            center: requested_center,
                        } = match rx.lock().expect("worker request lock poisoned").recv() {
                            Ok(request) => request,
                            Err(_) => break,
                        };
                        if let Some(cache) = &source.cache {
                            match cache.read(position.0, position.1) {
                                Ok(Some(packet)) => {
                                    if tx
                                        .send(BuiltChunk {
                                            position,
                                            packet: Ok(Some(packet)),
                                            generation_us: 0,
                                            encoding_us: 0,
                                        })
                                        .is_err()
                                    {
                                        break;
                                    }
                                    continue;
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    if tx
                                        .send(BuiltChunk {
                                            position,
                                            packet: Err(error.to_string()),
                                            generation_us: 0,
                                            encoding_us: 0,
                                        })
                                        .is_err()
                                    {
                                        break;
                                    }
                                    continue;
                                }
                            }
                        }
                        let started = Instant::now();
                        let chunk = crate::chunk_adapter::chunk_from_generator_cancellable(
                            &generator,
                            position.0,
                            position.1,
                            &tables,
                            || {
                                let center =
                                    unpacked_center(current_center.load(Ordering::Relaxed));
                                center != requested_center
                                    && (position.0 - center.0)
                                        .abs()
                                        .max((position.1 - center.1).abs())
                                        > MOVING_NEAR_RADIUS
                            },
                        );
                        let generation_us = started.elapsed().as_micros();
                        let started = Instant::now();
                        let packet = chunk
                            .and_then(|chunk| {
                                chunk
                                    .map(|chunk| {
                                        crate::chunk_adapter::encode_chunk(&chunk, &tables)
                                    })
                                    .transpose()
                            })
                            .map_err(|error| error.to_string());
                        let packet = packet.and_then(|packet| {
                            if let (Some(cache), Some(packet)) = (&source.cache, &packet) {
                                cache
                                    .write(position.0, position.1, packet)
                                    .map_err(|error| error.to_string())?;
                            }
                            Ok(packet)
                        });
                        let encoding_us = started.elapsed().as_micros();
                        if tx
                            .send(BuiltChunk {
                                position,
                                packet,
                                generation_us,
                                encoding_us,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                })
            })
            .collect();
        drop(tx);
        Self {
            requests: Some(requests),
            results,
            handles,
            in_flight: BTreeSet::new(),
            pending: None,
            capacity,
            current_center,
            cache: source.cache,
        }
    }
}

impl Drop for VanillaWorker {
    fn drop(&mut self) {
        self.requests.take();
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

const LOGIN: u32 = 50;
const POSITION: u32 = 73;
const CHUNK: u32 = 46;
const CACHE_CENTER: u32 = 96;
const FORGET_CHUNK: u32 = 38;
const GAME_EVENT: u32 = 39;
const ABILITIES: u32 = 65;
const TIME: u32 = 115;
const BATCH_START: u32 = 12;
const BATCH_END: u32 = 11;
const MAX_CHUNKS_PER_BATCH: usize = 16;
const MAX_BATCH_BYTES: usize = 768 * 1024;
// Air, bedrock, stone, dirt, non-snowy grass, vertical oak log, dry oak leaves.
const BLOCK_STATES: [u32; 19] = [
    0, 88, 1, 10, 9, 140, 294, 121, 677, 8598, 143, 322, 152, 406, 149, 378, 126, 15094, 14759,
];
const MIN_Y: i32 = -64;
const SECTION_COUNT: usize = 24;
const LIGHT_SECTIONS: usize = SECTION_COUNT + 2;
const CREATIVE: u32 = 1;

fn registry_id(manifest: &RegistryManifest, registry: &str, entry: &str) -> Option<u32> {
    manifest
        .registries
        .iter()
        .find(|(key, _)| key == registry)?
        .1
        .iter()
        .position(|key| key == entry)
        .map(|id| id as u32)
}

pub struct Preview {
    generator: Generator,
    radius: i32,
    center: (i32, i32),
    view_order: Vec<(i32, i32)>,
    next_view_position: usize,
    next_cached_position: usize,
    forget_dirty: bool,
    last_center_change: Instant,
    sent: BTreeSet<(i32, i32)>,
    biomes: [u32; 8],
    pub teleport_acknowledged: bool,
    pub client_loaded: bool,
    pub awaiting_batch: bool,
    keepalive_at: std::time::Instant,
    keepalive_id: u64,
    pub pending_keepalive: Option<u64>,
    vanilla: Option<VanillaWorker>,
    vanilla_spawn_y: Option<i32>,
    pub failed: bool,
}

impl Preview {
    pub fn new(
        seed: u64,
        radius: u8,
        terrain: crate::world::Terrain,
        manifest: &RegistryManifest,
    ) -> Option<Self> {
        if !(2..=32).contains(&radius) {
            return None;
        }
        let biomes: Vec<u32> = Biome::ALL
            .iter()
            .map(|b| registry_id(manifest, "minecraft:worldgen/biome", b.identifier()))
            .collect::<Option<Vec<_>>>()?;
        let biomes: [u32; 8] = biomes.try_into().ok()?;
        Some(Self {
            generator: Generator::with_terrain(seed, terrain),
            radius: radius.into(),
            center: (0, 0),
            view_order: ordered_view((0, 0), radius.into(), (0, 0)),
            next_view_position: 0,
            next_cached_position: 0,
            forget_dirty: false,
            last_center_change: Instant::now() - MOVING_PRIORITY_WINDOW,
            sent: BTreeSet::new(),
            biomes,
            teleport_acknowledged: false,
            client_loaded: false,
            awaiting_batch: false,
            keepalive_at: std::time::Instant::now(),
            keepalive_id: 0,
            pending_keepalive: None,
            vanilla: None,
            vanilla_spawn_y: None,
            failed: false,
        })
    }

    pub fn new_vanilla(
        radius: u8,
        manifest: &RegistryManifest,
        source: VanillaSource,
    ) -> Option<Self> {
        if !(1..=20).contains(&source.workers) {
            return None;
        }
        let mut preview = Self::new(
            source.seed as u64,
            radius,
            crate::world::Terrain::Preview,
            manifest,
        )?;
        preview.vanilla_spawn_y = Some(source.spawn_y);
        preview.vanilla = Some(VanillaWorker::new(source));
        Some(preview)
    }

    pub fn initial(&self, manifest: &RegistryManifest) -> Option<Vec<Vec<u8>>> {
        let mut login = 1i32.to_be_bytes().to_vec(); // One isolated observer, no shared player entities.
        login.push(0); // Not hardcore.
        put_varint(1, &mut login);
        put_string("minecraft:overworld", &mut login);
        for value in [1, self.radius as u32, self.radius as u32] {
            put_varint(value, &mut login);
        }
        login.extend([0, 1, 0]); // Debug/death/crafting flags.
        put_varint(
            registry_id(manifest, "minecraft:dimension_type", "minecraft:overworld")?,
            &mut login,
        );
        put_string("minecraft:overworld", &mut login);
        login.extend(0i64.to_be_bytes()); // No biome seed obfuscation claim; biome fixed to plains.
        put_varint(CREATIVE, &mut login);
        put_varint(0, &mut login); // Absent previous mode (optional VarInt).
        login.extend([0, 0, 0]); // Not debug, not flat, no death location.
        put_varint(0, &mut login); // Portal cooldown.
        put_varint(63, &mut login); // Sea level of negotiated overworld.
        login.extend([0, 0]); // Explicit offline mode, no secure chat.
        let mut abilities = vec![0x0f]; // Local Creative observer: invulnerable, flying, may fly, instant build.
        abilities.extend(0.5f32.to_be_bytes()); // Ten times the earlier local preview fly speed.
        abilities.extend(0.1f32.to_be_bytes());
        let mut position = vec![1]; // Teleport ID.
        for value in [
            0.5,
            self.vanilla_spawn_y
                .map_or_else(|| self.generator.height(0, 0), i64::from) as f64
                + 10.0,
            0.5,
            0.0,
            0.0,
            0.0,
        ] {
            position.extend(value.to_be_bytes());
        }
        position.extend(0f32.to_be_bytes());
        position.extend(20f32.to_be_bytes());
        position.extend(0u32.to_be_bytes()); // All coordinates absolute.
        let mut time = 0i64.to_be_bytes().to_vec();
        put_varint(1, &mut time);
        put_varint(
            registry_id(manifest, "minecraft:world_clock", "minecraft:overworld")?,
            &mut time,
        );
        put_varint(6000, &mut time); // Fixed noon preview clock; not a simulated day cycle.
        time.extend(0f32.to_be_bytes());
        time.extend(0f32.to_be_bytes());
        let mut start_chunks = vec![13]; // LEVEL_CHUNKS_LOAD_START.
        start_chunks.extend(0f32.to_be_bytes());
        Some(vec![
            frame(LOGIN, &login),
            frame(ABILITIES, &abilities),
            frame(TIME, &time),
            cache_center(self.center),
            frame(GAME_EVENT, &start_chunks),
            frame(POSITION, &position),
        ])
    }

    /// Validate only observer coordinates; this is not survival movement validation.
    pub fn move_to(&mut self, x: f64, y: f64, z: f64) -> Result<(), &'static str> {
        if ![x, y, z].iter().all(|v| v.is_finite())
            || x.abs() > 1_000_000.0
            || z.abs() > 1_000_000.0
            || !(-64.0..=512.0).contains(&y)
        {
            return Err("invalid preview position");
        }
        if !self.teleport_acknowledged {
            return Err("position before teleport acknowledgement");
        }
        let center = (
            (x.floor() as i32).div_euclid(16),
            (z.floor() as i32).div_euclid(16),
        );
        if center != self.center {
            let direction = (
                (center.0 - self.center.0).signum(),
                (center.1 - self.center.1).signum(),
            );
            self.center = center;
            self.view_order = ordered_view(center, self.radius, direction);
            self.next_view_position = 0;
            self.next_cached_position = 0;
            self.forget_dirty = true;
            self.last_center_change = Instant::now();
            if let Some(worker) = &self.vanilla {
                worker
                    .current_center
                    .store(packed_center(center), Ordering::Relaxed);
            }
        }
        Ok(())
    }

    /// Keepalive emission is independent of chunk admission.
    pub fn keepalive(&mut self) -> Option<Vec<u8>> {
        if self.keepalive_at.elapsed() < std::time::Duration::from_secs(5)
            || self.pending_keepalive.is_some()
        {
            return None;
        }
        self.keepalive_id += 1;
        self.pending_keepalive = Some(self.keepalive_id);
        self.keepalive_at = std::time::Instant::now();
        Some(frame(45, &self.keepalive_id.to_be_bytes()))
    }

    pub fn next_chunk(&mut self) -> Option<(Vec<u8>, u128, u128)> {
        if !self.teleport_acknowledged {
            return None;
        }
        if self.awaiting_batch && self.vanilla.is_none() {
            return None;
        }
        if self.awaiting_batch
            && self
                .vanilla
                .as_ref()
                .is_some_and(|worker| worker.in_flight.len() >= worker.capacity)
        {
            return None;
        }
        let mut output = cache_center(self.center);
        let mut had_forgets = false;
        let (cx, cz) = self.center;
        let radius = self.radius;
        if !self.awaiting_batch && self.forget_dirty {
            let expired: Vec<_> = self
                .sent
                .iter()
                .copied()
                .filter(|(x, z)| (x - cx).abs() > radius || (z - cz).abs() > radius)
                .collect();
            had_forgets = !expired.is_empty();
            for (x, z) in expired {
                let packed = ((z as u32 as u64) << 32) | x as u32 as u64;
                output.extend(frame(FORGET_CHUNK, &packed.to_be_bytes()));
                self.sent.remove(&(x, z));
            }
            self.forget_dirty = false;
        }
        if self.vanilla.is_some() {
            return self.next_vanilla_chunk(output, had_forgets);
        }
        let candidates: Vec<_> = self
            .view_order
            .iter()
            .copied()
            .filter(|pos| !self.sent.contains(pos))
            .collect();
        if candidates.is_empty() {
            return had_forgets.then_some((output, 0, 0));
        }
        output.extend(frame(BATCH_START, &[]));
        let mut generation_us = 0;
        let mut encoding_us = 0;
        let mut count = 0;
        for (x, z) in candidates.into_iter().take(MAX_CHUNKS_PER_BATCH) {
            let started = std::time::Instant::now();
            let chunk = self.generator.generate(x, z);
            generation_us += started.elapsed().as_micros();
            let started = std::time::Instant::now();
            let encoded = encode_chunk(&chunk, self.generator, &self.biomes);
            encoding_us += started.elapsed().as_micros();
            if count > 0 && output.len() + encoded.len() > MAX_BATCH_BYTES {
                break;
            }
            output.extend(encoded);
            self.sent.insert((x, z));
            count += 1;
        }
        let mut count_data = Vec::new();
        put_varint(count as u32, &mut count_data);
        output.extend(frame(BATCH_END, &count_data));
        self.awaiting_batch = true;
        Some((output, generation_us, encoding_us))
    }

    fn next_vanilla_chunk(
        &mut self,
        mut output: Vec<u8>,
        had_forgets: bool,
    ) -> Option<(Vec<u8>, u128, u128)> {
        let mut near_unsent = self
            .view_order
            .iter()
            .take_while(|&&(x, z)| {
                (x - self.center.0).abs().max((z - self.center.1).abs()) <= MOVING_NEAR_RADIUS
            })
            .filter(|position| !self.sent.contains(position))
            .count();
        let worker = self.vanilla.as_mut()?;
        let moving = self.last_center_change.elapsed() < MOVING_PRIORITY_WINDOW;
        while let Some(&position) = self.view_order.get(self.next_view_position) {
            let distance = (position.0 - self.center.0)
                .abs()
                .max((position.1 - self.center.1).abs());
            if distance > MOVING_NEAR_RADIUS && (moving || near_unsent > 0) {
                break;
            }
            if worker.in_flight.len() >= worker.capacity {
                break;
            }
            if self.sent.contains(&position) || worker.in_flight.contains(&position) {
                self.next_view_position += 1;
                continue;
            }
            let Some(requests) = &worker.requests else {
                break;
            };
            match requests.try_send(ChunkRequest {
                position,
                center: self.center,
            }) {
                Ok(()) => {
                    worker.in_flight.insert(position);
                    self.next_view_position += 1;
                }
                Err(mpsc::TrySendError::Full(_)) => break,
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    self.failed = true;
                    return None;
                }
            }
        }
        if moving
            && near_unsent == 0
            && let Some(cache) = &worker.cache
        {
            while worker.in_flight.len() < worker.capacity
                && let Some(&position) = self.view_order.get(self.next_cached_position)
            {
                if (position.0 - self.center.0)
                    .abs()
                    .max((position.1 - self.center.1).abs())
                    <= MOVING_NEAR_RADIUS
                    || self.sent.contains(&position)
                    || worker.in_flight.contains(&position)
                    || !cache.contains(position.0, position.1)
                {
                    self.next_cached_position += 1;
                    continue;
                }
                let Some(requests) = &worker.requests else {
                    break;
                };
                match requests.try_send(ChunkRequest {
                    position,
                    center: self.center,
                }) {
                    Ok(()) => {
                        worker.in_flight.insert(position);
                        self.next_cached_position += 1;
                    }
                    Err(mpsc::TrySendError::Full(_)) => break,
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        self.failed = true;
                        return None;
                    }
                }
            }
        }
        // Keep generation busy while the client processes the previous batch.
        // Results stay in the bounded channel until its acknowledgement arrives.
        if self.awaiting_batch {
            return None;
        }
        let mut count = 0;
        let mut generation_us = 0;
        let mut encoding_us = 0;
        let mut packet_bytes = 0;
        while count < MAX_CHUNKS_PER_BATCH {
            let built = match worker.pending.take() {
                Some(built) => built,
                None => match worker.results.try_recv() {
                    Ok(built) => built,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.failed = true;
                        if count == 0 {
                            return None;
                        }
                        break;
                    }
                },
            };
            if matches!(built.packet, Ok(None)) {
                worker.in_flight.remove(&built.position);
                self.next_view_position = 0;
                continue;
            }
            let distance = (built.position.0 - self.center.0)
                .abs()
                .max((built.position.1 - self.center.1).abs());
            if distance > MOVING_NEAR_RADIUS && near_unsent > 0 {
                // A result from the previous center may finish after we move.
                // It can be requested again once the nearby view is filled;
                // sending it now creates distant islands.
                worker.in_flight.remove(&built.position);
                self.next_view_position = 0;
                continue;
            }
            let size = match &built.packet {
                Ok(Some(packet)) if packet.len() < MAX_BATCH_BYTES => packet.len(),
                _ => {
                    self.failed = true;
                    return None;
                }
            };
            if count > 0 && packet_bytes + size > MAX_BATCH_BYTES {
                worker.pending = Some(built);
                break;
            }
            worker.in_flight.remove(&built.position);
            if (built.position.0 - self.center.0).abs() > self.radius
                || (built.position.1 - self.center.1).abs() > self.radius
                || self.sent.contains(&built.position)
            {
                continue;
            }
            if count == 0 {
                output.extend(frame(BATCH_START, &[]));
            }
            output.extend(
                built
                    .packet
                    .expect("checked packet")
                    .expect("checked non-cancelled"),
            );
            self.sent.insert(built.position);
            if distance <= MOVING_NEAR_RADIUS {
                near_unsent -= 1;
            }
            generation_us += built.generation_us;
            encoding_us += built.encoding_us;
            packet_bytes += size;
            count += 1;
        }
        if count == 0 {
            return had_forgets.then_some((output, 0, 0));
        }
        let mut count_data = Vec::new();
        put_varint(count as u32, &mut count_data);
        output.extend(frame(BATCH_END, &count_data));
        self.awaiting_batch = true;
        Some((output, generation_us, encoding_us))
    }
}

fn cache_center((x, z): (i32, i32)) -> Vec<u8> {
    let mut body = Vec::new();
    put_varint(x as u32, &mut body);
    put_varint(z as u32, &mut body);
    frame(CACHE_CENTER, &body)
}

/// Square shells from the player, with each shell's forward edge first.
fn ordered_view((cx, cz): (i32, i32), radius: i32, (dx, dz): (i32, i32)) -> Vec<(i32, i32)> {
    let mut positions: Vec<_> = (-radius..=radius)
        .flat_map(|z| (-radius..=radius).map(move |x| (cx + x, cz + z)))
        .collect();
    positions.sort_unstable_by_key(|(x, z)| {
        let relative_x = x - cx;
        let relative_z = z - cz;
        (
            relative_x.abs().max(relative_z.abs()),
            -(relative_x * dx + relative_z * dz),
            *z,
            *x,
        )
    });
    positions
}

fn block(chunk: &Chunk, x: usize, y: i32, z: usize) -> Block {
    if y < 0 {
        Block::Air
    } else {
        chunk.block(x, y as usize, z).unwrap_or(Block::Air)
    }
}

/// Pack fixed-width values without crossing a 64-bit word boundary.
fn packed(values: &[u16], bits: usize) -> Vec<u64> {
    let per_word = 64 / bits;
    values
        .chunks(per_word)
        .map(|part| {
            part.iter()
                .enumerate()
                .fold(0, |word, (i, v)| word | (u64::from(*v) << (i * bits)))
        })
        .collect()
}

pub fn encode_chunk(chunk: &Chunk, generator: Generator, biome_ids: &[u32; 8]) -> Vec<u8> {
    let mut body = chunk.x.to_be_bytes().to_vec();
    body.extend(chunk.z.to_be_bytes());
    // 26.3 heightmaps are a map of enum VarInt to long array, not named NBT.
    put_varint(3, &mut body);
    for kind in [1, 4, 5] {
        // WORLD_SURFACE, MOTION_BLOCKING, MOTION_BLOCKING_NO_LEAVES.
        put_varint(kind, &mut body);
        let heights: Vec<u16> = (0..16)
            .flat_map(|z| {
                (0..16).map(move |x| {
                    (0..128)
                        .rev()
                        .find(|y| {
                            let b = block(chunk, x, *y, z);
                            b != Block::Air
                                && (kind != 5
                                    || !matches!(
                                        b,
                                        Block::Leaves
                                            | Block::SpruceLeaves
                                            | Block::AcaciaLeaves
                                            | Block::JungleLeaves
                                    ))
                        })
                        .map_or(0, |y| (y + 1 - MIN_Y) as u16)
                })
            })
            .collect();
        let words = packed(&heights, 9);
        put_varint(words.len() as u32, &mut body);
        for word in words {
            body.extend(word.to_be_bytes());
        }
    }
    let mut sections = Vec::new();
    let biome_values: Vec<u16> = (0..4)
        .flat_map(|_| (0..4).flat_map(|z| (0..4).map(move |x| (x, z))))
        .map(|(x, z)| {
            let wx = i64::from(chunk.x) * 16 + x * 4 + 2;
            let wz = i64::from(chunk.z) * 16 + z * 4 + 2;
            let biome = generator.biome(wx, wz);
            Biome::ALL.iter().position(|value| *value == biome).unwrap() as u16
        })
        .collect();
    let mut biome_palette = biome_values.clone();
    biome_palette.sort_unstable();
    biome_palette.dedup();
    for section in 0..SECTION_COUNT {
        let values: Vec<u16> = (0..16)
            .flat_map(|y| {
                (0..16).flat_map(move |z| {
                    (0..16).map(move |x| block(chunk, x, MIN_Y + section as i32 * 16 + y, z) as u16)
                })
            })
            .collect();
        let non_air = values.iter().filter(|v| **v != 0).count() as u16;
        sections.extend(non_air.to_be_bytes());
        sections.extend(0u16.to_be_bytes()); // Fluid count added in 26.3.
        let mut palette = values.clone();
        palette.sort_unstable();
        palette.dedup();
        if palette.len() == 1 {
            sections.push(0);
            put_varint(BLOCK_STATES[values[0] as usize], &mut sections);
        } else {
            let bits = (usize::BITS - (palette.len() - 1).leading_zeros()).max(4) as usize;
            sections.push(bits as u8);
            put_varint(palette.len() as u32, &mut sections);
            for value in &palette {
                put_varint(BLOCK_STATES[*value as usize], &mut sections);
            }
            let indices: Vec<u16> = values
                .iter()
                .map(|v| palette.binary_search(v).unwrap() as u16)
                .collect();
            // 26.3 has a fixed-size long array, with no array-length prefix.
            for word in packed(&indices, bits) {
                sections.extend(word.to_be_bytes());
            }
        }
        if biome_palette.len() == 1 {
            sections.push(0);
            put_varint(biome_ids[biome_palette[0] as usize], &mut sections);
        } else {
            let bits = (usize::BITS - (biome_palette.len() - 1).leading_zeros()) as usize;
            sections.push(bits as u8);
            put_varint(biome_palette.len() as u32, &mut sections);
            for index in &biome_palette {
                put_varint(biome_ids[*index as usize], &mut sections);
            }
            let indices: Vec<u16> = biome_values
                .iter()
                .map(|v| biome_palette.binary_search(v).unwrap() as u16)
                .collect();
            for word in packed(&indices, bits) {
                sections.extend(word.to_be_bytes());
            }
        }
    }
    put_varint(sections.len() as u32, &mut body);
    body.extend(sections);
    put_varint(0, &mut body); // No block entities.
    let all_light = (1u64 << LIGHT_SECTIONS) - 1;
    for mask in [all_light, 0, 0, all_light] {
        if mask == 0 {
            put_varint(0, &mut body);
        } else {
            put_varint(4, &mut body);
            body.extend((mask as u32).to_le_bytes());
        }
    }
    put_varint(LIGHT_SECTIONS as u32, &mut body);
    // Direct vertical skylight, attenuated by leaves. No emitted light or full lateral solver yet.
    let mut lights = vec![[0u8; 2048]; LIGHT_SECTIONS];
    for z in 0..16 {
        for x in 0..16 {
            let mut sky = 15u8;
            for y in (-80..336).rev() {
                match block(chunk, x, y, z) {
                    Block::Air => {}
                    Block::Leaves
                    | Block::SpruceLeaves
                    | Block::AcaciaLeaves
                    | Block::JungleLeaves => sky = sky.saturating_sub(1),
                    _ => sky = 0,
                }
                let section = ((y + 80) / 16) as usize;
                let i = (y.rem_euclid(16) as usize * 16 + z) * 16 + x;
                lights[section][i / 2] |= sky << ((i % 2) * 4);
            }
        }
    }
    for layer in lights {
        put_varint(2048, &mut body);
        body.extend(layer);
    }
    put_varint(0, &mut body); // No emitted block light.
    frame(CHUNK, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Terrain;

    fn read_varint(input: &[u8]) -> (u32, usize) {
        let mut value = 0;
        for (index, byte) in input.iter().take(5).enumerate() {
            value |= u32::from(byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 {
                return (value, index + 1);
            }
        }
        panic!("invalid test packet VarInt");
    }

    fn packet_ids(mut stream: &[u8]) -> Vec<u32> {
        let mut ids = Vec::new();
        while !stream.is_empty() {
            let (length, prefix) = read_varint(stream);
            let packet = &stream[prefix..prefix + length as usize];
            ids.push(read_varint(packet).0);
            stream = &stream[prefix + length as usize..];
        }
        ids
    }

    fn manifest() -> RegistryManifest {
        RegistryManifest {
            tags: Vec::new(),
            registries: vec![
                (
                    "minecraft:dimension_type".into(),
                    vec!["minecraft:overworld".into()],
                ),
                (
                    "minecraft:world_clock".into(),
                    vec!["minecraft:overworld".into()],
                ),
                (
                    "minecraft:worldgen/biome".into(),
                    Biome::ALL.iter().map(|b| b.identifier().into()).collect(),
                ),
            ],
        }
    }

    #[test]
    fn vanilla_ready_chunks_share_a_bounded_acknowledged_batch() {
        let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
        preview.teleport_acknowledged = true;
        let (tx, rx) = mpsc::sync_channel(8);
        let positions = [(0, 0), (1, 0), (0, 1)];
        for position in positions {
            tx.send(BuiltChunk {
                position,
                packet: Ok(Some(frame(CHUNK, b"test"))),
                generation_us: 4,
                encoding_us: 2,
            })
            .unwrap();
        }
        preview.vanilla = Some(VanillaWorker {
            requests: None,
            results: rx,
            handles: Vec::new(),
            in_flight: positions.into_iter().collect(),
            pending: None,
            capacity: 8,
            current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
            cache: None,
        });
        let (stream, generation_us, encoding_us) = preview.next_chunk().unwrap();
        let ids = packet_ids(&stream);
        assert_eq!(ids.iter().filter(|id| **id == CHUNK).count(), 3);
        assert_eq!(ids.iter().filter(|id| **id == BATCH_START).count(), 1);
        assert_eq!(ids.iter().filter(|id| **id == BATCH_END).count(), 1);
        assert_eq!((generation_us, encoding_us), (12, 6));
        assert!(preview.awaiting_batch);
        assert_eq!(preview.sent.len(), 3);
    }

    #[test]
    fn vanilla_batch_defers_packet_when_byte_budget_is_full() {
        let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
        preview.teleport_acknowledged = true;
        let (tx, rx) = mpsc::sync_channel(8);
        for position in [(0, 0), (1, 0)] {
            tx.send(BuiltChunk {
                position,
                packet: Ok(Some(frame(CHUNK, &vec![0; 400_000]))),
                generation_us: 0,
                encoding_us: 0,
            })
            .unwrap();
        }
        preview.vanilla = Some(VanillaWorker {
            requests: None,
            results: rx,
            handles: Vec::new(),
            in_flight: [(0, 0), (1, 0)].into_iter().collect(),
            pending: None,
            capacity: 8,
            current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
            cache: None,
        });
        let first = preview.next_chunk().unwrap().0;
        assert_eq!(
            packet_ids(&first).iter().filter(|id| **id == CHUNK).count(),
            1
        );
        assert!(first.len() < MAX_BATCH_BYTES + 32);
        assert!(preview.vanilla.as_ref().unwrap().pending.is_some());
        preview.awaiting_batch = false;
        let second = preview.next_chunk().unwrap().0;
        assert_eq!(
            packet_ids(&second)
                .iter()
                .filter(|id| **id == CHUNK)
                .count(),
            1
        );
        assert!(preview.vanilla.as_ref().unwrap().pending.is_none());
        assert_eq!(preview.sent.len(), 2);
    }

    #[test]
    fn vanilla_move_sends_unload_without_waiting_for_generated_chunk() {
        let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
        preview.teleport_acknowledged = true;
        preview.sent.insert((0, 0));
        let (_tx, results) = mpsc::sync_channel(1);
        preview.vanilla = Some(VanillaWorker {
            requests: None,
            results,
            handles: Vec::new(),
            in_flight: BTreeSet::new(),
            pending: None,
            capacity: 1,
            current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
            cache: None,
        });
        preview.move_to(48.5, 80.0, 0.5).unwrap();
        let (stream, _, _) = preview.next_chunk().expect("unload-only stream");
        assert_eq!(packet_ids(&stream), vec![CACHE_CENTER, FORGET_CHUNK]);
        assert!(!preview.sent.contains(&(0, 0)));
        assert!(!preview.awaiting_batch);
    }

    #[test]
    fn cancelled_far_chunk_releases_slot_without_failing_preview() {
        let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
        preview.teleport_acknowledged = true;
        let (tx, results) = mpsc::sync_channel(1);
        tx.send(BuiltChunk {
            position: (2, 2),
            packet: Ok(None),
            generation_us: 1,
            encoding_us: 0,
        })
        .unwrap();
        preview.vanilla = Some(VanillaWorker {
            requests: None,
            results,
            handles: Vec::new(),
            in_flight: [(2, 2)].into_iter().collect(),
            pending: None,
            capacity: 1,
            current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
            cache: None,
        });
        assert!(preview.next_chunk().is_none());
        assert!(!preview.failed);
        assert!(preview.vanilla.as_ref().unwrap().in_flight.is_empty());
        assert_eq!(preview.next_view_position, 0);
    }

    #[test]
    fn completed_view_is_not_rescanned_until_center_changes() {
        let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
        preview.teleport_acknowledged = true;
        preview.sent.extend(preview.view_order.iter().copied());
        let (_tx, results) = mpsc::sync_channel(1);
        preview.vanilla = Some(VanillaWorker {
            requests: None,
            results,
            handles: Vec::new(),
            in_flight: BTreeSet::new(),
            pending: None,
            capacity: 1,
            current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
            cache: None,
        });
        assert!(preview.next_chunk().is_none());
        assert_eq!(preview.next_view_position, preview.view_order.len());
        assert!(preview.next_chunk().is_none());
        assert_eq!(preview.next_view_position, preview.view_order.len());
        preview.move_to(16.5, 80.0, 0.5).unwrap();
        assert_eq!(preview.next_view_position, 0);
        let stream = preview.next_chunk().expect("old edge unload").0;
        assert!(packet_ids(&stream).contains(&FORGET_CHUNK));
        assert!(preview.next_view_position < preview.view_order.len());
    }

    #[test]
    fn stationary_view_uses_full_bounded_pool_for_outer_chunks() {
        let mut preview = Preview::new(2027, 5, Terrain::Preview, &manifest()).unwrap();
        preview.teleport_acknowledged = true;
        preview.last_center_change = Instant::now() - MOVING_PRIORITY_WINDOW;
        preview.sent.extend(
            preview
                .view_order
                .iter()
                .copied()
                .filter(|(x, z)| x.abs().max(z.abs()) <= MOVING_NEAR_RADIUS),
        );
        let (requests, queued) = mpsc::sync_channel(8);
        let (_results_sender, results) = mpsc::sync_channel(8);
        preview.vanilla = Some(VanillaWorker {
            requests: Some(requests),
            results,
            handles: Vec::new(),
            in_flight: BTreeSet::new(),
            pending: None,
            capacity: 8,
            current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
            cache: None,
        });
        assert!(preview.next_chunk().is_none());
        assert_eq!(preview.vanilla.as_ref().unwrap().in_flight.len(), 8);
        let queued: Vec<_> = queued.try_iter().map(|request| request.position).collect();
        assert_eq!(queued.len(), 8);
        assert!(queued.iter().all(|(x, z)| x.abs().max(z.abs()) == 4));
    }

    #[test]
    fn nearby_hole_blocks_outer_requests_and_discards_early_far_result() {
        let mut preview = Preview::new(2027, 5, Terrain::Preview, &manifest()).unwrap();
        preview.teleport_acknowledged = true;
        preview.last_center_change = Instant::now() - MOVING_PRIORITY_WINDOW;
        preview.sent.extend(
            preview
                .view_order
                .iter()
                .copied()
                .filter(|(x, z)| x.abs().max(z.abs()) <= MOVING_NEAR_RADIUS && (*x, *z) != (0, 0)),
        );
        let (requests, queued) = mpsc::sync_channel(8);
        let (results_sender, results) = mpsc::sync_channel(8);
        results_sender
            .send(BuiltChunk {
                position: (4, 0),
                packet: Ok(Some(frame(CHUNK, &[4]))),
                generation_us: 0,
                encoding_us: 0,
            })
            .unwrap();
        results_sender
            .send(BuiltChunk {
                position: (0, 0),
                packet: Ok(Some(frame(CHUNK, &[0]))),
                generation_us: 0,
                encoding_us: 0,
            })
            .unwrap();
        preview.vanilla = Some(VanillaWorker {
            requests: Some(requests),
            results,
            handles: Vec::new(),
            in_flight: [(4, 0), (0, 0)].into_iter().collect(),
            pending: None,
            capacity: 8,
            current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
            cache: None,
        });
        let (stream, _, _) = preview.next_chunk().expect("nearby chunk is ready");
        assert_eq!(
            packet_ids(&stream),
            vec![CACHE_CENTER, BATCH_START, CHUNK, BATCH_END]
        );
        assert!(preview.sent.contains(&(0, 0)));
        assert!(!preview.sent.contains(&(4, 0)));
        assert!(queued.try_iter().next().is_none());
        preview.awaiting_batch = false;
        preview.next_chunk();
        assert!(
            preview
                .vanilla
                .as_ref()
                .unwrap()
                .in_flight
                .iter()
                .any(|(x, z)| x.abs().max(z.abs()) == 4)
        );
    }

    #[test]
    fn moving_view_keeps_square_shells_and_prefers_travel_direction() {
        let east = ordered_view((0, 0), 3, (1, 0));
        let west = ordered_view((0, 0), 3, (-1, 0));
        assert_eq!(east.len(), 49);
        assert_eq!(east[0], (0, 0));
        assert_eq!(east[1], (1, -1));
        assert_eq!(west[1], (-1, -1));
        assert!(east.windows(2).all(
            |pair| pair[0].0.abs().max(pair[0].1.abs()) <= pair[1].0.abs().max(pair[1].1.abs())
        ));
        assert_eq!(east.iter().copied().collect::<BTreeSet<_>>().len(), 49);
        assert_eq!(west.iter().copied().collect::<BTreeSet<_>>().len(), 49);
    }

    #[test]
    fn maximum_view_uses_bounded_acknowledged_batches_and_unloads_old_view() {
        assert!(Preview::new(2026, 33, Terrain::Preview, &manifest()).is_none());
        let mut preview = Preview::new(2026, 32, Terrain::Preview, &manifest()).unwrap();
        assert!(preview.next_chunk().is_none());
        preview.teleport_acknowledged = true;
        let (first, _, _) = preview.next_chunk().unwrap();
        assert!(first.len() <= MAX_BATCH_BYTES + 16);
        assert!(preview.sent.len() > 1);
        assert!(preview.sent.len() <= MAX_CHUNKS_PER_BATCH);
        assert!(preview.next_chunk().is_none());
        preview.awaiting_batch = false;
        preview.move_to(1024.5, 80.0, -1024.5).unwrap();
        assert_eq!(preview.center, (64, -65));
        let (moved, _, _) = preview.next_chunk().unwrap();
        assert_ne!(first, moved);
        assert!(!preview.sent.contains(&(0, 0))); // Old view was unloaded.
        assert!(preview.sent.len() <= MAX_CHUNKS_PER_BATCH);
        assert!(preview.move_to(f64::NAN, 80.0, 0.0).is_err());
    }

    #[test]
    fn walking_one_chunk_forgets_old_edge_and_rejoin_starts_fresh() {
        let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
        preview.teleport_acknowledged = true;
        let first = preview.next_chunk().unwrap().0;
        preview.awaiting_batch = false;
        while preview.sent.len() < 25 {
            preview.next_chunk().unwrap();
            preview.awaiting_batch = false;
        }
        assert!(preview.next_chunk().is_none());
        preview.move_to(16.5, 80.0, 0.5).unwrap();
        let moved = preview.next_chunk().unwrap().0;
        let ids = packet_ids(&moved);
        assert_eq!(ids.iter().filter(|id| **id == FORGET_CHUNK).count(), 5);
        assert_eq!(ids.iter().filter(|id| **id == CHUNK).count(), 5);
        assert_eq!(preview.sent.len(), 25);
        assert!(!preview.sent.contains(&(-2, 0)));
        assert!(preview.sent.contains(&(3, 0)));

        let mut rejoined = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
        rejoined.teleport_acknowledged = true;
        assert_eq!(rejoined.next_chunk().unwrap().0, first);
    }

    #[test]
    fn chunk_wire_output_is_repeatable_across_request_order() {
        let generator = Generator::new(42);
        let biomes = [0, 1, 2, 3, 4, 5, 6, 7];
        let first = encode_chunk(&generator.generate(-1, 2), generator, &biomes);
        let _unrelated = generator.generate(17, -3);
        assert_eq!(
            first,
            encode_chunk(&generator.generate(-1, 2), generator, &biomes)
        );
        assert_ne!(
            first,
            encode_chunk(&generator.generate(0, 2), generator, &biomes)
        );
    }

    #[test]
    #[ignore = "requires operator-provisioned 26.3 worldgen data and registry IDs"]
    fn local_vanilla_worker_delivers_a_framed_chunk_without_blocking_network_poll() {
        let source = VanillaSource {
            data_root: std::env::var_os("RUSTMC_VANILLA_DATA")
                .map(PathBuf::from)
                .expect("set RUSTMC_VANILLA_DATA"),
            registry_table: std::env::var_os("RUSTMC_CHUNK_REGISTRY")
                .map(PathBuf::from)
                .expect("set RUSTMC_CHUNK_REGISTRY"),
            seed: 2026,
            spawn_y: 117,
            cache: None,
            workers: 8,
        };
        let mut preview = Preview::new_vanilla(2, &manifest(), source).expect("preview");
        preview.teleport_acknowledged = true;
        let started = Instant::now();
        assert!(preview.next_chunk().is_none());
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        let delivered = loop {
            if let Some((stream, _, _)) = preview.next_chunk() {
                break stream;
            }
            assert!(!preview.failed, "worker failed before returning a chunk");
            // A bound on delivery, not on speed. This smoke runs the decorated
            // generator from a debug build, where one cold column descent costs
            // tens of times the 1,750 ms per chunk the release bench measures
            // (91 s to first delivery observed here, on a machine also running
            // three oracle grids), and each worker compiles the provisioned pack
            // before it draws anything. So this is a hang detector; the measured
            // cost of this path is `bench_vanilla_chunks` and
            // `docs/PROVENANCE.md`.
            assert!(started.elapsed() < std::time::Duration::from_secs(600));
            thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(packet_ids(&delivered).contains(&CHUNK));
        assert_eq!(preview.sent.len(), 1);
        preview.move_to(16.5, 80.0, 0.5).unwrap();
        assert!(
            preview.next_chunk().is_none(),
            "the first batch still awaits acknowledgement"
        );
        assert!(
            !preview.vanilla.as_ref().unwrap().in_flight.is_empty(),
            "generation should continue while awaiting the client"
        );
    }
}
