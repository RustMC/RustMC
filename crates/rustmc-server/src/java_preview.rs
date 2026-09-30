//! Independent Java 26.3 wire adapter for the immutable local terrain preview.
//! IDs and field layouts: official 26.3 packet/block reports and codec metadata.

use crate::{
    discovery_java::{frame, put_string, put_varint},
    preview_data::RegistryManifest,
    world::{Biome, Block, Chunk, Generator},
};
use std::collections::BTreeSet;

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
    sent: BTreeSet<(i32, i32)>,
    biomes: [u32; 8],
    pub teleport_acknowledged: bool,
    pub client_loaded: bool,
    pub awaiting_batch: bool,
    keepalive_at: std::time::Instant,
    keepalive_id: u64,
    pub pending_keepalive: Option<u64>,
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
            sent: BTreeSet::new(),
            biomes,
            teleport_acknowledged: false,
            client_loaded: false,
            awaiting_batch: false,
            keepalive_at: std::time::Instant::now(),
            keepalive_id: 0,
            pending_keepalive: None,
        })
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
            self.generator.height(0, 0) as f64 + 10.0,
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
        self.center = (
            (x.floor() as i32).div_euclid(16),
            (z.floor() as i32).div_euclid(16),
        );
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
        if !self.teleport_acknowledged || self.awaiting_batch {
            return None;
        }
        let mut output = cache_center(self.center);
        let (cx, cz) = self.center;
        let radius = self.radius;
        let expired: Vec<_> = self
            .sent
            .iter()
            .copied()
            .filter(|(x, z)| (x - cx).abs() > radius || (z - cz).abs() > radius)
            .collect();
        for (x, z) in expired {
            let packed = ((z as u32 as u64) << 32) | x as u32 as u64;
            output.extend(frame(FORGET_CHUNK, &packed.to_be_bytes()));
            self.sent.remove(&(x, z));
        }
        let mut candidates: Vec<_> = (-radius..=radius)
            .flat_map(|z| (-radius..=radius).map(move |x| (cx + x, cz + z)))
            .filter(|pos| !self.sent.contains(pos))
            .collect();
        if candidates.is_empty() {
            return None;
        }
        candidates.sort_unstable_by_key(|(x, z)| ((x - cx).abs().max((z - cz).abs()), *z, *x));
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
        put_varint(count, &mut count_data);
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
}
