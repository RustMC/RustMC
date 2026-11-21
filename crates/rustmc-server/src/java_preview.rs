//! Independent Java 26.3 wire adapter for the immutable local terrain preview.
//! IDs and field layouts: official 26.3 packet/block reports and codec metadata.

use crate::{
    java_wire::{frame, frame_compressed, put_string, put_varint, reframe_compressed},
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
        // `in_flight` never exceeds `capacity`, so neither channel can grow past it.
        let (requests, rx) = mpsc::sync_channel::<ChunkRequest>(capacity);
        let (tx, results) = mpsc::sync_channel(capacity);
        let rx = Arc::new(Mutex::new(rx));
        let current_center = Arc::new(AtomicU64::new(packed_center((0, 0))));
        let shared_caches = Arc::new(crate::vanilla::generator::SharedChunkCaches::new());
        let handles = (0..capacity)
            .map(|_| {
                let rx = Arc::clone(&rx);
                let tx = tx.clone();
                let source = source.clone();
                let current_center = Arc::clone(&current_center);
                let shared_caches = Arc::clone(&shared_caches);
                thread::spawn(move || {
                    let Some((generator, tables)) = load_worker_inputs(&source, shared_caches)
                    else {
                        // Dropping this worker's senders lets the preview observe
                        // a disconnected pool and close the session explicitly.
                        return;
                    };
                    loop {
                        let ChunkRequest {
                            position,
                            center: requested_center,
                        } = match rx
                            .lock()
                            // Workers only call `recv()` under this lock; a panic elsewhere leaves the channel receiver valid.
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .recv()
                        {
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

/// Load one worker's generator and registry tables. Runtime preflight already
/// validated these inputs, so a failure here means they changed on disk.
fn load_worker_inputs(
    source: &VanillaSource,
    shared_caches: Arc<crate::vanilla::generator::SharedChunkCaches>,
) -> Option<(
    crate::vanilla::generator::VanillaGenerator,
    crate::chunk_adapter::registry::RegistryTables,
)> {
    let loaded = crate::vanilla::generator::VanillaGenerator::with_shared_caches(
        &source.data_root,
        source.seed,
        "minecraft:overworld",
        shared_caches,
    )
    .map_err(|error| error.to_string())
    .and_then(|generator| {
        let text =
            std::fs::read_to_string(&source.registry_table).map_err(|error| error.to_string())?;
        crate::chunk_adapter::registry::RegistryTables::from_provisioned(&text)
            .map(|tables| (generator, tables))
            .map_err(|error| error.to_string())
    });
    match loaded {
        Ok(inputs) => Some(inputs),
        Err(error) => {
            eprintln!("event=vanilla_worker_start_failed error={error:?}");
            None
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

use crate::java_packets::play::clientbound::{
    CHUNK_BATCH_FINISHED as BATCH_END, CHUNK_BATCH_START as BATCH_START,
    FORGET_LEVEL_CHUNK as FORGET_CHUNK, GAME_EVENT, KEEP_ALIVE, LEVEL_CHUNK_WITH_LIGHT as CHUNK,
    LOGIN, PLAYER_ABILITIES as ABILITIES, PLAYER_POSITION as POSITION,
    SET_CHUNK_CACHE_CENTER as CACHE_CENTER, SET_TIME as TIME,
};

const MAX_CHUNKS_PER_BATCH: usize = 16;
const MAX_BATCH_BYTES: usize = 768 * 1024;
/// Seconds between server keep-alive challenges.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
/// Default block state of each preview block in the official 26.3 block
/// report, in `Block` discriminant order (checked by a unit test).
pub(crate) const BLOCK_STATES: [(Block, u32); 25] = [
    (Block::Air, 0),                // minecraft:air
    (Block::Bedrock, 88),           // minecraft:bedrock
    (Block::Stone, 1),              // minecraft:stone
    (Block::Dirt, 10),              // minecraft:dirt
    (Block::Grass, 9),              // minecraft:grass_block[snowy=false]
    (Block::Log, 140),              // minecraft:oak_log[axis=y]
    (Block::Leaves, 294),           // minecraft:oak_leaves (default state)
    (Block::Sand, 121),             // minecraft:sand
    (Block::Sandstone, 677),        // minecraft:sandstone
    (Block::Snow, 8598),            // minecraft:snow_block
    (Block::SpruceLog, 143),        // minecraft:spruce_log[axis=y]
    (Block::SpruceLeaves, 322),     // minecraft:spruce_leaves (default state)
    (Block::AcaciaLog, 152),        // minecraft:acacia_log[axis=y]
    (Block::AcaciaLeaves, 406),     // minecraft:acacia_leaves (default state)
    (Block::JungleLog, 149),        // minecraft:jungle_log[axis=y]
    (Block::JungleLeaves, 378),     // minecraft:jungle_leaves (default state)
    (Block::RedSand, 126),          // minecraft:red_sand
    (Block::RedSandstone, 15094),   // minecraft:red_sandstone
    (Block::Terracotta, 14759),     // minecraft:terracotta
    (Block::ShortGrass, 2363),      // minecraft:short_grass
    (Block::SnowLayer, 8589),       // minecraft:snow[layers=1]
    (Block::Carpet, 14743),         // minecraft:white_carpet
    (Block::TallGrassLower, 14771), // minecraft:tall_grass[half=lower]
    (Block::TallGrassUpper, 14770), // minecraft:tall_grass[half=upper]
    (Block::Obsidian, 4925),        // minecraft:obsidian
];

fn block_state(index: u16) -> u32 {
    BLOCK_STATES[usize::from(index)].1
}

/// Returns the Java 26.3 protocol-777 default block state ID for a preview [`Block`].
pub(crate) fn block_default_state_id(block: Block) -> u32 {
    BLOCK_STATES[block as usize].1
}
const MIN_Y: i32 = -64;
const SECTION_COUNT: usize = 24;
const LIGHT_SECTIONS: usize = SECTION_COUNT + 2;
const CREATIVE: u32 = 1;
// Vanilla Java rejects move packets beyond these magnitudes.
pub(crate) const MAX_HORIZONTAL_POSITION: f64 = 30_000_000.0;
pub(crate) const MAX_VERTICAL_POSITION: f64 = 20_000_000.0;

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
    compression_threshold: Option<usize>,
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
            compression_threshold: None,
            failed: false,
        })
    }

    pub fn with_compression_threshold(mut self, threshold: Option<usize>) -> Self {
        self.compression_threshold = threshold;
        self
    }

    fn frame_packet(&self, id: u32, body: &[u8]) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        push_wire_frame(self.compression_threshold, id, body, &mut out)?;
        Some(out)
    }

    fn push_frame(&self, id: u32, body: &[u8], out: &mut Vec<u8>) -> Option<()> {
        push_wire_frame(self.compression_threshold, id, body, out)
    }

    fn cache_center_frame(&self, (x, z): (i32, i32)) -> Option<Vec<u8>> {
        let mut body = Vec::new();
        put_varint(x as u32, &mut body);
        put_varint(z as u32, &mut body);
        self.frame_packet(CACHE_CENTER, &body)
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
            self.frame_packet(LOGIN, &login)?,
            self.frame_packet(ABILITIES, &abilities)?,
            self.frame_packet(TIME, &time)?,
            self.cache_center_frame(self.center)?,
            self.frame_packet(GAME_EVENT, &start_chunks)?,
            self.frame_packet(POSITION, &position)?,
        ])
    }

    /// Validate only observer coordinates; this is not survival movement validation.
    pub fn move_to(&mut self, x: f64, y: f64, z: f64) -> Result<(), &'static str> {
        if ![x, y, z].iter().all(|v| v.is_finite())
            || x.abs() > MAX_HORIZONTAL_POSITION
            || z.abs() > MAX_HORIZONTAL_POSITION
            || y.abs() > MAX_VERTICAL_POSITION
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
        if self.keepalive_at.elapsed() < KEEPALIVE_INTERVAL || self.pending_keepalive.is_some() {
            return None;
        }
        self.keepalive_id += 1;
        self.pending_keepalive = Some(self.keepalive_id);
        self.keepalive_at = std::time::Instant::now();
        self.frame_packet(KEEP_ALIVE, &self.keepalive_id.to_be_bytes())
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
        let mut output = self.cache_center_frame(self.center)?;
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
                self.push_frame(FORGET_CHUNK, &packed.to_be_bytes(), &mut output)?;
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
        self.push_frame(BATCH_START, &[], &mut output)?;
        let mut generation_us = 0;
        let mut encoding_us = 0;
        let mut count = 0;
        for (x, z) in candidates.into_iter().take(MAX_CHUNKS_PER_BATCH) {
            let started = std::time::Instant::now();
            let chunk = self.generator.generate(x, z);
            generation_us += started.elapsed().as_micros();
            let started = std::time::Instant::now();
            let encoded = encode_chunk(&chunk, self.generator, &self.biomes);
            let wire = match self.compression_threshold {
                None => encoded,
                Some(threshold) => {
                    let mut out = Vec::new();
                    if reframe_compressed(&encoded, threshold, &mut out).is_err() {
                        self.failed = true;
                        return None;
                    }
                    out
                }
            };
            encoding_us += started.elapsed().as_micros();
            if count > 0 && output.len() + wire.len() > MAX_BATCH_BYTES {
                break;
            }
            output.extend(wire);
            self.sent.insert((x, z));
            count += 1;
        }
        let mut count_data = Vec::new();
        put_varint(count as u32, &mut count_data);
        self.push_frame(BATCH_END, &count_data, &mut output)?;
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
        let compression = self.compression_threshold;
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
            let wire_packet = match (&built.packet, self.compression_threshold) {
                (Ok(Some(packet)), None) if packet.len() < MAX_BATCH_BYTES => None,
                (Ok(Some(packet)), Some(threshold)) => {
                    let mut out = Vec::new();
                    if reframe_compressed(packet, threshold, &mut out).is_err()
                        || out.len() >= MAX_BATCH_BYTES
                    {
                        self.failed = true;
                        return None;
                    }
                    Some(out)
                }
                _ => {
                    self.failed = true;
                    return None;
                }
            };
            let size = match (&wire_packet, &built.packet) {
                (Some(wire), _) => wire.len(),
                (None, Ok(Some(packet))) => packet.len(),
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
            let packet = match (wire_packet, built.packet) {
                (Some(wire), _) => wire,
                (None, Ok(Some(packet))) => packet,
                _ => {
                    self.failed = true;
                    return None;
                }
            };
            if count == 0 {
                push_wire_frame(compression, BATCH_START, &[], &mut output)?;
            }
            output.extend(packet);
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
        push_wire_frame(compression, BATCH_END, &count_data, &mut output)?;
        self.awaiting_batch = true;
        Some((output, generation_us, encoding_us))
    }
}

fn push_wire_frame(
    compression_threshold: Option<usize>,
    id: u32,
    body: &[u8],
    out: &mut Vec<u8>,
) -> Option<()> {
    match compression_threshold {
        None => {
            out.extend(frame(id, body));
            Some(())
        }
        Some(threshold) => frame_compressed(id, body, threshold, out).ok(),
    }
}

/// Square shells from the player, with each shell's forward edge first.
///
/// This is the order the preview requests chunks in; a join at spawn uses
/// direction `(0, 0)`. Public so the generation benchmarks can replay it.
pub fn ordered_view((cx, cz): (i32, i32), radius: i32, (dx, dz): (i32, i32)) -> Vec<(i32, i32)> {
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

/// Plants with no collision, which only `WORLD_SURFACE` captures.
fn is_passable_plant(block: Block) -> bool {
    matches!(
        block,
        Block::ShortGrass | Block::TallGrassLower | Block::TallGrassUpper
    )
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

fn biome_index(biome: Biome) -> u16 {
    match biome {
        Biome::Plains => 0,
        Biome::Forest => 1,
        Biome::Desert => 2,
        Biome::SnowyPlains => 3,
        Biome::Taiga => 4,
        Biome::Savanna => 5,
        Biome::Jungle => 6,
        Biome::Badlands => 7,
    }
}

fn build_palette<T: Copy + Ord>(values: &[T]) -> (Vec<T>, Vec<u16>) {
    let mut entries: Vec<(T, usize)> = values
        .iter()
        .copied()
        .enumerate()
        .map(|(index, value)| (value, index))
        .collect();
    entries.sort_unstable_by_key(|&(value, _)| value);

    let mut palette = Vec::new();
    let mut indices = vec![0u16; values.len()];
    let mut palette_index = 0u16;
    for (value, original_index) in entries {
        match palette.last() {
            Some(&prev) if prev == value => {}
            _ => {
                palette_index = palette.len() as u16;
                palette.push(value);
            }
        }
        indices[original_index] = palette_index;
    }
    (palette, indices)
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
                                && (kind == 1 || !is_passable_plant(b))
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
            biome_index(generator.biome(wx, wz))
        })
        .collect();
    let (biome_palette, biome_indices) = build_palette(&biome_values);
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
        let (palette, indices) = build_palette(&values);
        if palette.len() == 1 {
            sections.push(0);
            put_varint(block_state(values[0]), &mut sections);
        } else {
            let bits = (usize::BITS - (palette.len() - 1).leading_zeros()).max(4) as usize;
            sections.push(bits as u8);
            put_varint(palette.len() as u32, &mut sections);
            for value in &palette {
                put_varint(block_state(*value), &mut sections);
            }
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
            for word in packed(&biome_indices, bits) {
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
    // Direct vertical skylight, attenuated by leaves; plants and thin covers
    // pass it unchanged. No emitted light or full lateral solver yet.
    let mut lights = vec![[0u8; 2048]; LIGHT_SECTIONS];
    for z in 0..16 {
        for x in 0..16 {
            let mut sky = 15u8;
            for y in (-80..336).rev() {
                match block(chunk, x, y, z) {
                    Block::Air
                    | Block::ShortGrass
                    | Block::SnowLayer
                    | Block::Carpet
                    | Block::TallGrassLower
                    | Block::TallGrassUpper => {}
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
mod tests;
