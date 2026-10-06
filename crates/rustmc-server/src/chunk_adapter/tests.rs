//! Deterministic wire tests for [`super`]: every assertion decodes the encoded
//! packet back off the byte stream, so a field-order or bit-width regression
//! fails here rather than in front of a real client.
//!
//! The fixtures use locally invented registry ids inside a 65,536-state and
//! 128-biome registry. Those sizes are chosen so the derived direct-mode
//! widths equal the ones the provisioned 26.3 registries produce (16 and 7),
//! which keeps the assertions meaningful without committing Mojang data.

use super::*;

/// Locally invented registry sizes: 65,536 states derive a 16-bit direct
/// palette and 128 biomes derive a 7-bit one, like the provisioned 26.3 tables.
const STATE_COUNT: u32 = 65_536;
const BIOME_COUNT: u32 = 128;

const AIR: u32 = 0;
const STONE: u32 = 1;
const GRASS: u32 = 9;
const BEDROCK: u32 = 88;
const DIRT: u32 = 10;
const LEAVES: u32 = 300;
const CAVE_AIR: u32 = 1_024;
const WATER: u32 = 700;
const LAVA: u32 = 701;

const PLAINS: u32 = 0;
const DESERT: u32 = 12;
const FOREST: u32 = 4;

/// First id of the invented solid states, and how many exist.
const SOLID_BASE: u32 = 1_000;
const PROBE_STATES: u32 = 400;
/// First id of the invented biomes, and how many exist.
const BIOME_BASE: u32 = 20;
const PROBE_BIOMES: u32 = 20;

fn tables() -> RegistryTables {
    let states: Vec<(String, u32)> = [
        ("minecraft:air", AIR),
        ("minecraft:cave_air", CAVE_AIR),
        ("minecraft:stone", STONE),
        ("minecraft:dirt", DIRT),
        ("minecraft:grass_block[snowy=false]", GRASS),
        ("minecraft:bedrock", BEDROCK),
        ("minecraft:water", WATER),
        ("minecraft:lava", LAVA),
        ("minecraft:oak_leaves[persistent=false]", LEAVES),
    ]
    .into_iter()
    .map(|(name, id)| (name.to_string(), id))
    .chain((0..PROBE_STATES).map(|index| (format!("minecraft:probe_{index}"), SOLID_BASE + index)))
    .collect();
    let biomes: Vec<(String, u32)> = [
        ("minecraft:plains", PLAINS),
        ("minecraft:desert", DESERT),
        ("minecraft:forest", FOREST),
    ]
    .into_iter()
    .map(|(name, id)| (name.to_string(), id))
    .chain((0..PROBE_BIOMES).map(|index| (format!("minecraft:biome_{index}"), BIOME_BASE + index)))
    .collect();
    RegistryTables::new(
        SUPPORTED_VERSION,
        SUPPORTED_PROTOCOL,
        STATE_COUNT,
        BIOME_COUNT,
        states,
        biomes,
    )
    .expect("fixture tables")
}

fn air_chunk(chunk_x: i32, chunk_z: i32, tables: &RegistryTables) -> VanillaChunk {
    VanillaChunk::new(
        chunk_x,
        chunk_z,
        vec![tables.air_state(); CHUNK_CELLS],
        vec![PLAINS; CHUNK_BIOMES],
    )
    .expect("air column")
}

#[test]
fn absolute_queries_use_euclidean_chunk_coordinates_and_absolute_y() {
    let chunk = column_chunk(
        -2,
        1,
        |x, y, z| {
            if x == 0 && y == OVERWORLD_MIN_Y && z == 15 {
                BEDROCK
            } else if x == 15 && y == 319 && z == 0 {
                STONE
            } else {
                AIR
            }
        },
        |bx, _by, bz| if bx == 3 && bz == 0 { FOREST } else { PLAINS },
    );
    assert_eq!(chunk.state_at_world(-32, -64, 31), Some(BEDROCK));
    assert_eq!(chunk.state_at_world(-17, 319, 16), Some(STONE));
    assert_eq!(chunk.biome_at_world(-17, 319, 16), Some(FOREST));
    assert_eq!(chunk.biome_at_world(-32, -64, 31), Some(PLAINS));
    for (x, y, z) in [
        (-33, -64, 31),
        (-16, -64, 31),
        (-32, -64, 15),
        (-32, -64, 32),
        (-32, -65, 31),
        (-32, 320, 31),
    ] {
        assert_eq!(chunk.state_at_world(x, y, z), None, "{x},{y},{z}");
        assert_eq!(chunk.biome_at_world(x, y, z), None, "{x},{y},{z}");
    }
}

#[test]
fn chunk_origins_reject_positions_that_overflow_absolute_block_coordinates() {
    assert_eq!(chunk_origin(-2, 1), Ok((-32, 16)));
    assert_eq!(
        chunk_origin(i32::MIN / 16, i32::MAX / 16),
        Ok((i32::MIN, i32::MAX - 15))
    );
    for (chunk_x, chunk_z) in [(i32::MAX / 16 + 1, 0), (0, i32::MIN / 16 - 1)] {
        assert_eq!(
            chunk_origin(chunk_x, chunk_z),
            Err(EncodeError::ChunkCoordinateOverflow { chunk_x, chunk_z })
        );
    }
}

/// A whole-column chunk from a closure over absolute positions, the shape most
/// fixtures want: they think in world coordinates, not flat indices.
fn column_chunk(
    chunk_x: i32,
    chunk_z: i32,
    state_at: impl Fn(usize, i32, usize) -> u32,
    biome_at: impl Fn(usize, i32, usize) -> u32,
) -> VanillaChunk {
    let mut states = vec![AIR; CHUNK_CELLS];
    let mut biomes = vec![PLAINS; CHUNK_BIOMES];
    for y in OVERWORLD_MIN_Y..OVERWORLD_MIN_Y + OVERWORLD_HEIGHT as i32 {
        for z in 0..CHUNK_SIDE {
            for x in 0..CHUNK_SIDE {
                states[state_index(x, y, z)] = state_at(x, y, z);
            }
        }
    }
    for layer in 0..BIOME_LAYERS {
        let by = OVERWORLD_MIN_Y + layer as i32 * SECTION_BIOME_SIDE as i32;
        for bz in 0..SECTION_BIOME_SIDE {
            for bx in 0..SECTION_BIOME_SIDE {
                biomes[biome_index(bx, by, bz)] = biome_at(bx, by, bz);
            }
        }
    }
    VanillaChunk::new(chunk_x, chunk_z, states, biomes).expect("fixture column")
}

/// Byte cursor over a 26.3 packet, reading exactly the codecs the adapter writes.
struct Wire<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Wire<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take(&mut self, count: usize) -> &'a [u8] {
        assert!(
            self.pos + count <= self.bytes.len(),
            "wire under-run at {} wanting {count} bytes",
            self.pos
        );
        let slice = &self.bytes[self.pos..self.pos + count];
        self.pos += count;
        slice
    }

    fn byte(&mut self) -> u8 {
        self.take(1)[0]
    }

    fn short(&mut self) -> u16 {
        u16::from_be_bytes(self.take(2).try_into().expect("2 bytes"))
    }

    fn int(&mut self) -> i32 {
        i32::from_be_bytes(self.take(4).try_into().expect("4 bytes"))
    }

    fn long(&mut self) -> u64 {
        u64::from_be_bytes(self.take(8).try_into().expect("8 bytes"))
    }

    fn varint(&mut self) -> u32 {
        let mut value = 0u32;
        for shift in 0..5 {
            let byte = self.byte();
            value |= u32::from(byte & 0x7f) << (shift * 7);
            if byte & 0x80 == 0 {
                return value;
            }
        }
        panic!("test VarInt is longer than five bytes");
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }
}

/// Unpacks a `SimpleBitStorage` word array: `valuesPerLong = 64 / bits`, and
/// values never straddle a word.
fn unpack(words: &[u64], count: usize, bits: usize) -> Vec<u32> {
    let per_word = 64 / bits;
    (0..count)
        .map(|index| {
            let word = words[index / per_word];
            ((word >> (index % per_word * bits)) & ((1u64 << bits) - 1)) as u32
        })
        .collect()
}

fn read_container(wire: &mut Wire, cells: usize, max_indirect: usize) -> (usize, Vec<u32>) {
    let bits = usize::from(wire.byte());
    if bits == 0 {
        let id = wire.varint();
        return (0, vec![id; cells]);
    }
    let palette: Vec<u32> = if bits <= max_indirect {
        let size = wire.varint() as usize;
        (0..size).map(|_| wire.varint()).collect()
    } else {
        Vec::new()
    };
    let per_word = 64 / bits;
    let words: Vec<u64> = (0..cells.div_ceil(per_word)).map(|_| wire.long()).collect();
    let values = unpack(&words, cells, bits);
    if palette.is_empty() {
        return (bits, values);
    }
    (
        bits,
        values
            .iter()
            .map(|index| {
                palette
                    .get(*index as usize)
                    .copied()
                    .unwrap_or_else(|| panic!("palette index {index} is out of range"))
            })
            .collect(),
    )
}

fn read_heightmap(wire: &mut Wire) -> (u32, Vec<u32>) {
    let kind = wire.varint();
    let words: Vec<u64> = (0..wire.varint() as usize).map(|_| wire.long()).collect();
    (kind, unpack(&words, HEIGHTMAP_COLUMNS, HEIGHTMAP_BITS))
}

/// `BIT_SET` in 26.3 is `byteArray`: a VarInt byte count over the little-endian
/// bytes of `BitSet.toByteArray()`, truncated at the highest set bit.
fn read_bit_set(wire: &mut Wire) -> u64 {
    let length = wire.varint() as usize;
    wire.take(length)
        .iter()
        .rev()
        .fold(0u64, |mask, byte| (mask << 8) | u64::from(*byte))
}

fn read_byte_list(wire: &mut Wire<'_>) -> Vec<Vec<u8>> {
    (0..wire.varint())
        .map(|_| {
            let length = wire.varint() as usize;
            wire.take(length).to_vec()
        })
        .collect()
}

#[derive(Debug)]
struct Section {
    non_air: u16,
    fluid: u16,
    block_bits: usize,
    biome_bits: usize,
    states: Vec<u32>,
    biomes: Vec<u32>,
}

#[derive(Debug)]
struct Decoded {
    x: i32,
    z: i32,
    heightmaps: Vec<(u32, Vec<u32>)>,
    sections: Vec<Section>,
    block_entities: u32,
    masks: [u64; 4],
    sky: Vec<Vec<u8>>,
    block_light: Vec<Vec<u8>>,
}

impl Decoded {
    /// Section cells mapped back into whole-column [`state_index`] order.
    fn states(&self) -> Vec<u32> {
        let mut out = vec![AIR; CHUNK_CELLS];
        for (section, cells) in self.sections.iter().enumerate() {
            for (index, id) in cells.states.iter().copied().enumerate() {
                let local_y = index / (CHUNK_SIDE * CHUNK_SIDE);
                let plane = index % (CHUNK_SIDE * CHUNK_SIDE);
                let y = OVERWORLD_MIN_Y + (section * SECTION_HEIGHT + local_y) as i32;
                out[state_index(plane % CHUNK_SIDE, y, plane / CHUNK_SIDE)] = id;
            }
        }
        out
    }

    /// Section biome cells mapped back into whole-column [`biome_index`] order.
    fn biomes(&self) -> Vec<u32> {
        let mut out = vec![PLAINS; CHUNK_BIOMES];
        for (section, cells) in self.sections.iter().enumerate() {
            for (index, id) in cells.biomes.iter().copied().enumerate() {
                let local = index / BIOME_LAYER_CELLS;
                let plane = index % BIOME_LAYER_CELLS;
                let by = OVERWORLD_MIN_Y
                    + (section * SECTION_BIOME_SIDE + local) as i32 * SECTION_BIOME_SIDE as i32;
                out[biome_index(plane % SECTION_BIOME_SIDE, by, plane / SECTION_BIOME_SIDE)] = id;
            }
        }
        out
    }

    fn heightmap(&self, kind: HeightmapKind) -> &[u32] {
        &self
            .heightmaps
            .iter()
            .find(|(id, _)| *id == kind.protocol_id())
            .unwrap_or_else(|| panic!("heightmap {} is missing", kind.protocol_id()))
            .1
    }

    /// Skylight nibble at one absolute position, including the two boundary
    /// layers above and below the buildable range.
    fn sky_at(&self, x: usize, y: i32, z: usize) -> u8 {
        let offset = (y - (OVERWORLD_MIN_Y - SECTION_HEIGHT as i32)) as usize;
        let cell = x + z * CHUNK_SIDE + offset % SECTION_HEIGHT * CHUNK_SIDE * CHUNK_SIDE;
        let byte = self.sky[offset / SECTION_HEIGHT][cell / 2];
        if cell.is_multiple_of(2) {
            byte & 0x0f
        } else {
            byte >> 4
        }
    }
}

fn decode_chunk(packet: &[u8]) -> Decoded {
    let mut outer = Wire::new(packet);
    let length = outer.varint() as usize;
    let framed = outer.take(length);
    assert_eq!(
        outer.remaining(),
        0,
        "the VarInt prefix must cover the packet"
    );
    let mut wire = Wire::new(framed);
    assert_eq!(wire.varint(), CHUNK_PACKET, "packet id");
    let x = wire.int();
    let z = wire.int();
    let kinds = wire.varint();
    assert_eq!(kinds as usize, HEIGHTMAP_TYPES, "heightmap type count");
    let heightmaps = (0..kinds).map(|_| read_heightmap(&mut wire)).collect();
    let buffer_length = wire.varint() as usize;
    let section_bytes = wire.take(buffer_length);
    let mut cursor = Wire::new(section_bytes);
    let sections = (0..SECTIONS)
        .map(|_| {
            let non_air = cursor.short();
            let fluid = cursor.short();
            let (block_bits, states) =
                read_container(&mut cursor, SECTION_CELLS, BLOCK_PALETTE_MAX_BITS);
            let (biome_bits, biomes) =
                read_container(&mut cursor, SECTION_BIOME_CELLS, BIOME_PALETTE_MAX_BITS);
            Section {
                non_air,
                fluid,
                block_bits,
                biome_bits,
                states,
                biomes,
            }
        })
        .collect();
    assert_eq!(cursor.remaining(), 0, "the section buffer must be consumed");
    let block_entities = wire.varint();
    let masks = [
        read_bit_set(&mut wire),
        read_bit_set(&mut wire),
        read_bit_set(&mut wire),
        read_bit_set(&mut wire),
    ];
    let sky = read_byte_list(&mut wire);
    let block_light = read_byte_list(&mut wire);
    assert_eq!(
        wire.remaining(),
        0,
        "the packet must end after the light data"
    );
    Decoded {
        x,
        z,
        heightmaps,
        sections,
        block_entities,
        masks,
        sky,
        block_light,
    }
}

#[test]
fn full_overworld_column_round_trips_a_synthetic_pattern() {
    let tables = tables();
    let pattern = [STONE, DIRT, GRASS, WATER, CAVE_AIR];
    let chunk = column_chunk(
        2,
        -1,
        |x, y, z| pattern[(x + z + (y - OVERWORLD_MIN_Y) as usize) % pattern.len()],
        |bx, by, bz| [PLAINS, DESERT, FOREST][(bx + bz + (by - OVERWORLD_MIN_Y) as usize / 4) % 3],
    );
    let decoded = decode_chunk(&encode_chunk(&chunk, &tables).expect("encode a full column"));
    assert_eq!((decoded.x, decoded.z), (2, -1));
    assert_eq!(
        decoded.states(),
        chunk.states().to_vec(),
        "every cell survives"
    );
    assert_eq!(
        decoded.biomes(),
        chunk.biomes().to_vec(),
        "every biome survives"
    );
    assert_eq!(decoded.block_entities, 0, "block entities are deferred");
    for (index, section) in decoded.sections.iter().enumerate() {
        let cells = chunk.section_states(index);
        let expected_air = cells
            .iter()
            .filter(|id| tables.kind(**id) == Some(StateKind::Air))
            .count();
        let expected_fluid = cells
            .iter()
            .filter(|id| tables.kind(**id) == Some(StateKind::Fluid))
            .count();
        assert_eq!(
            section.non_air as usize,
            SECTION_CELLS - expected_air,
            "section {index} counts cave_air as air"
        );
        assert_eq!(
            section.fluid as usize, expected_fluid,
            "section {index} counts the dimension fluid"
        );
        assert_eq!(section.block_bits, 4, "five states fit the 4-bit palette");
        assert_eq!(section.biome_bits, 2, "three biomes need 2 bits");
    }
    // Heightmaps pack 9-bit values across 256 columns in x-fastest order; the
    // capture rules themselves are covered by `cave_profile_...` below.
    for kind in [
        HeightmapKind::WorldSurface,
        HeightmapKind::MotionBlocking,
        HeightmapKind::MotionBlockingNoLeaves,
    ] {
        let expected: Vec<u32> = (0..HEIGHTMAP_COLUMNS)
            .map(|column| {
                let x = column % CHUNK_SIDE;
                let z = column / CHUNK_SIDE;
                (0..OVERWORLD_HEIGHT)
                    .rev()
                    .find(|row| {
                        let id = pattern[(x + z + row) % pattern.len()];
                        tables.kind(id).expect("fixture id").captured_by(kind)
                    })
                    .map_or(0, |row| row as u32 + 1)
            })
            .collect();
        assert_eq!(
            decoded.heightmap(kind),
            expected.as_slice(),
            "heightmap {:?}",
            kind.protocol_id()
        );
    }
}

#[test]
fn negative_chunk_coordinates_keep_their_fixed_width_header() {
    let tables = tables();
    let chunk = air_chunk(-3, -7, &tables);
    let packet = encode_chunk(&chunk, &tables).expect("encode at negative coords");
    // Frame: VarInt length, VarInt id, then INT x and INT z (not VarInts).
    let mut outer = Wire::new(&packet);
    let framed_length = outer.varint() as usize;
    let framed = outer.take(framed_length);
    assert_eq!(outer.remaining(), 0);
    let mut wire = Wire::new(framed);
    assert_eq!(wire.varint(), CHUNK_PACKET);
    assert_eq!(wire.take(4), (-3i32).to_be_bytes(), "x is a fixed int");
    assert_eq!(wire.take(4), (-7i32).to_be_bytes(), "z is a fixed int");
    let decoded = decode_chunk(&packet);
    assert_eq!(
        (decoded.x, decoded.z),
        (-3, -7),
        "the ints read back signed"
    );
    // The header carries the position, so a same-shaped chunk elsewhere differs.
    assert_ne!(
        packet,
        encode_chunk(&air_chunk(3, 7, &tables), &tables).expect("encode")
    );
    assert_eq!(
        packet,
        encode_chunk(&air_chunk(-3, -7, &tables), &tables).expect("encode")
    );
}

#[test]
fn cave_profile_keeps_air_pockets_and_fluid_in_motion_heightmaps() {
    let tables = tables();
    // A solid run with carved air inside and fluid above leaves. The fluid
    // surface is included in both motion heightmaps.
    let top = 106i32;
    let chunk = column_chunk(
        0,
        0,
        |_, y, _| {
            if (105..=top).contains(&y) {
                WATER
            } else if y == 100 {
                LEAVES
            } else if y == 99 {
                GRASS
            } else if (69..=98).contains(&y) && !(94..=95).contains(&y) {
                STONE
            } else if y == OVERWORLD_MIN_Y {
                BEDROCK
            } else {
                AIR
            }
        },
        |_, _, _| PLAINS,
    );
    let packet = encode_chunk(&chunk, &tables).expect("encode the cave profile");
    let decoded = decode_chunk(&packet);
    // Air pockets stay air inside the solid run rather than being filled in.
    for y in [94, 95] {
        assert_eq!(
            decoded.states()[state_index(4, y, 7)],
            AIR,
            "pocket at y={y}"
        );
        assert_eq!(chunk.state_at(4, y, 7), Some(AIR));
    }
    assert_eq!(decoded.states()[state_index(4, 93, 7)], STONE);
    assert_eq!(decoded.states()[state_index(4, 105, 7)], WATER);

    let captured = |y: i32| (y + 1 - OVERWORLD_MIN_Y) as u32;
    assert_eq!(
        decoded.heightmap(HeightmapKind::WorldSurface)[255],
        captured(top)
    );
    assert_eq!(
        decoded.heightmap(HeightmapKind::MotionBlocking)[255],
        captured(top),
        "a non-empty fluid state is captured by the motion heightmap"
    );
    assert_eq!(
        decoded.heightmap(HeightmapKind::MotionBlockingNoLeaves)[255],
        captured(top),
        "fluid is captured even where leaves below are excluded"
    );
    assert!(
        decoded
            .heightmap(HeightmapKind::WorldSurface)
            .iter()
            .all(|value| *value == captured(top)),
        "the profile spans every column"
    );

    // Section 9 holds the carved pocket rows: 14 solid layers of 16.
    assert_eq!(
        decoded.sections[9].non_air as usize,
        14 * CHUNK_SIDE * CHUNK_SIDE
    );
    assert_eq!(
        decoded.sections[9].fluid, 0,
        "no fluid in the carved section"
    );
    // Section 10 spans y=96..=111: leaves, grass, three stone rows, two water rows.
    assert_eq!(
        decoded.sections[10].non_air as usize,
        7 * CHUNK_SIDE * CHUNK_SIDE
    );
    assert_eq!(
        decoded.sections[10].fluid as usize,
        2 * CHUNK_SIDE * CHUNK_SIDE
    );
    assert_eq!(
        decoded.sections[11].non_air, 0,
        "above the profile is empty"
    );
    assert_eq!(decoded.sections[11].block_bits, 0, "and single-valued");

    // Vertical skylight: air passes, each fluid and the leaf row cost one
    // level, and the grass closes the column.
    assert_eq!(decoded.sky_at(4, 107, 7), 15, "above the water");
    assert_eq!(decoded.sky_at(4, 106, 7), 14, "inside the water column");
    assert_eq!(decoded.sky_at(4, 105, 7), 13);
    assert_eq!(decoded.sky_at(4, 101, 7), 13, "the air gap keeps the level");
    assert_eq!(decoded.sky_at(4, 100, 7), 12, "leaves cost one level");
    assert_eq!(decoded.sky_at(4, 99, 7), 0, "a solid closes the column");
    assert_eq!(decoded.sky_at(4, 94, 7), 0, "the pocket stays dark");
    assert_eq!(
        decoded.sky_at(4, 320, 7),
        15,
        "the boundary layer above is lit"
    );
    assert_eq!(
        decoded.sky_at(4, -80, 7),
        0,
        "nothing lights below the solid run"
    );
}

#[test]
fn fluid_levels_are_counted_per_section_and_captured_by_motion_heightmaps() {
    // Water and lava reach the wire as one registry id per level, all from the
    // same base family, so the section counters, heightmaps, and skylight must
    // treat every level alike.
    const WATER_L1: u32 = 702;
    const WATER_L7: u32 = 703;
    const LAVA_L0: u32 = 704;
    let tables = RegistryTables::new(
        SUPPORTED_VERSION,
        SUPPORTED_PROTOCOL,
        STATE_COUNT,
        BIOME_COUNT,
        [
            ("minecraft:air".to_string(), AIR),
            ("minecraft:stone".to_string(), STONE),
            ("minecraft:bedrock".to_string(), BEDROCK),
            ("minecraft:water".to_string(), WATER),
            ("minecraft:water[level=1]".to_string(), WATER_L1),
            ("minecraft:water[level=7]".to_string(), WATER_L7),
            ("minecraft:lava[level=0]".to_string(), LAVA_L0),
        ],
        [("minecraft:plains".to_string(), PLAINS)],
    )
    .expect("fluid fixture tables");
    for (name, id) in [
        ("minecraft:water[level=1]", WATER_L1),
        ("minecraft:water[level=7]", WATER_L7),
        ("minecraft:lava[level=0]", LAVA_L0),
    ] {
        let entry = tables
            .state(name)
            .unwrap_or_else(|_| panic!("{name} must resolve"));
        assert_eq!(entry.id, id, "{name} keeps its own registry id");
        assert_eq!(
            entry.kind,
            StateKind::Fluid,
            "{name} is a level of the dimension fluid family"
        );
        assert_eq!(tables.kind(id), Some(StateKind::Fluid));
    }

    let chunk = column_chunk(
        -2,
        3,
        |_, y, _| match y {
            OVERWORLD_MIN_Y => BEDROCK,
            -63..=47 => STONE,
            48..=53 => WATER,
            54..=57 => WATER_L1,
            58..=63 => WATER_L7,
            64..=67 => LAVA_L0,
            _ => AIR,
        },
        |_, _, _| PLAINS,
    );
    let packet = encode_chunk(&chunk, &tables).expect("encode the fluid column");
    let decoded = decode_chunk(&packet);
    assert_eq!((decoded.x, decoded.z), (-2, 3));
    assert_eq!(decoded.states(), chunk.states().to_vec());
    assert_eq!(decoded.states()[state_index(3, 55, 3)], WATER_L1);
    assert_eq!(decoded.states()[state_index(3, 67, 3)], LAVA_L0);

    // Section 7 spans y=48..=63: sixteen fluid rows, so the non-air and fluid
    // counts are both the whole section.
    assert_eq!(decoded.sections[7].non_air as usize, SECTION_CELLS);
    assert_eq!(decoded.sections[7].fluid as usize, SECTION_CELLS);
    assert_eq!(
        decoded.sections[7].block_bits, BLOCK_PALETTE_MIN_BITS,
        "three water levels stay on the indirect palette"
    );
    // Everything below the fluid, bedrock floor and stone run alike, is solid.
    for section in 0..7 {
        assert_eq!(
            decoded.sections[section].non_air as usize, SECTION_CELLS,
            "section {section} is fully filled"
        );
        assert_eq!(
            decoded.sections[section].fluid, 0,
            "section {section} holds no fluid"
        );
    }
    // Section 8 above carries the four lava rows.
    assert_eq!(
        decoded.sections[8].non_air as usize,
        4 * CHUNK_SIDE * CHUNK_SIDE
    );
    assert_eq!(
        decoded.sections[8].fluid as usize,
        4 * CHUNK_SIDE * CHUNK_SIDE,
        "lava counts exactly like water"
    );
    assert_eq!(
        decoded.sections[9].non_air, 0,
        "nothing above the fluid surface"
    );
    assert_eq!(decoded.sections[9].fluid, 0);
    assert!(
        decoded
            .sections
            .iter()
            .all(|section| section.biome_bits == 0),
        "one biome per column keeps every section single-valued"
    );

    let captured = |y: i32| (y + 1 - OVERWORLD_MIN_Y) as u32;
    assert_eq!(
        decoded.heightmap(HeightmapKind::WorldSurface)[0],
        captured(67),
        "the fluid surface is the world surface"
    );
    assert_eq!(
        decoded.heightmap(HeightmapKind::MotionBlocking)[0],
        captured(67),
        "the fluid surface is included in MOTION_BLOCKING"
    );
    assert_eq!(
        decoded.heightmap(HeightmapKind::MotionBlockingNoLeaves)[0],
        captured(67)
    );
    assert!(
        decoded
            .heightmap(HeightmapKind::WorldSurface)
            .iter()
            .all(|value| *value == captured(67)),
        "the profile spans every column"
    );

    // Vertical skylight: each fluid level costs one degree, water and lava alike.
    assert_eq!(decoded.sky_at(0, 68, 0), 15, "air above the fluid");
    assert_eq!(decoded.sky_at(0, 67, 0), 14, "the top lava row");
    assert_eq!(
        decoded.sky_at(0, 64, 0),
        11,
        "four lava rows cost four degrees"
    );
    assert_eq!(
        decoded.sky_at(0, 63, 0),
        10,
        "the water column continues it"
    );
    assert_eq!(decoded.sky_at(0, 58, 0), 5);
    assert_eq!(decoded.sky_at(0, 54, 0), 1);
    assert_eq!(
        decoded.sky_at(0, 53, 0),
        0,
        "the fluid run exhausts the light"
    );
    assert_eq!(
        decoded.sky_at(0, 47, 0),
        0,
        "and the stone closes the column"
    );
}

#[test]
fn unknown_or_unmapped_ids_are_rejected_not_substituted() {
    let tables = tables();
    let mut states = vec![AIR; CHUNK_CELLS];
    // Inside the registry but described by nothing: no heightmap or light class
    // can be derived, so encoding must fail instead of guessing.
    states[state_index(0, 200, 0)] = 60_000;
    let chunk = VanillaChunk::new(0, 0, states.clone(), vec![PLAINS; CHUNK_BIOMES]).expect("chunk");
    assert_eq!(
        encode_chunk(&chunk, &tables),
        Err(EncodeError::UnregisteredState { id: 60_000 })
    );

    states[state_index(0, 200, 0)] = STATE_COUNT;
    let chunk = VanillaChunk::new(0, 0, states, vec![PLAINS; CHUNK_BIOMES]).expect("chunk");
    assert_eq!(
        encode_chunk(&chunk, &tables),
        Err(EncodeError::StateOutOfRange {
            id: STATE_COUNT,
            count: STATE_COUNT
        })
    );

    let chunk = VanillaChunk::new(
        0,
        0,
        vec![AIR; CHUNK_CELLS],
        vec![BIOME_COUNT; CHUNK_BIOMES],
    )
    .expect("chunk");
    assert_eq!(
        encode_chunk(&chunk, &tables),
        Err(EncodeError::BiomeOutOfRange {
            id: BIOME_COUNT,
            count: BIOME_COUNT
        })
    );

    // A name the generator can emit but the table does not carry is the same
    // hard error on the way in, never a fallback id.
    let error = tables
        .state("minecraft:glow_lichen")
        .expect_err("unmapped state");
    assert_eq!(
        error,
        RegistryError::UnknownState {
            name: "minecraft:glow_lichen".to_string()
        }
    );
    assert_eq!(
        EncodeError::from(error.clone()).to_string(),
        "registry: no protocol id for block state `minecraft:glow_lichen`"
    );
    assert!(matches!(
        tables.biome("minecraft:deep_dark"),
        Err(RegistryError::UnknownBiome { .. })
    ));
}

#[test]
fn truncated_or_misshaped_columns_are_rejected() {
    let tables = tables();
    assert_eq!(
        VanillaChunk::new(0, 0, vec![AIR; CHUNK_CELLS - 1], vec![PLAINS; CHUNK_BIOMES]),
        Err(EncodeError::WrongStateCount {
            expected: CHUNK_CELLS,
            got: CHUNK_CELLS - 1
        })
    );
    assert_eq!(
        VanillaChunk::new(0, 0, vec![AIR; CHUNK_CELLS], vec![PLAINS; CHUNK_BIOMES - 2]),
        Err(EncodeError::WrongBiomeCount {
            expected: CHUNK_BIOMES,
            got: CHUNK_BIOMES - 2
        })
    );
    // Coordinates outside the column are an error, not a silent air read.
    let chunk = air_chunk(0, 0, &tables);
    assert_eq!(chunk.state_at(16, OVERWORLD_MIN_Y, 0), None);
    assert_eq!(
        chunk.state_at(0, OVERWORLD_MIN_Y - 1, 0),
        None,
        "below the buildable range"
    );
    assert_eq!(chunk.biome_at(4, OVERWORLD_MIN_Y, 0), None);
}

#[test]
fn block_palette_widths_transition_at_the_registry_limits() {
    let tables = tables();
    // Air plus `size - 1` invented solids, so the palette size is exact.
    for (size, expected_bits) in [
        (1usize, 0usize),
        (2, BLOCK_PALETTE_MIN_BITS),
        (16, BLOCK_PALETTE_MIN_BITS),
        (17, 5),
        (256, 8),
        (257, 16),
    ] {
        let mut states = vec![AIR; CHUNK_CELLS];
        for (index, cell) in states.iter_mut().enumerate().take(size).skip(1) {
            *cell = SOLID_BASE + index as u32;
        }
        let chunk =
            VanillaChunk::new(0, 0, states, vec![PLAINS; CHUNK_BIOMES]).expect("fixture column");
        let decoded = decode_chunk(&encode_chunk(&chunk, &tables).expect("encode"));
        let section = &decoded.sections[0];
        assert_eq!(
            section.block_bits, expected_bits,
            "{size} distinct states must use {expected_bits} bits"
        );
        assert_eq!(
            section.non_air as usize,
            size - 1,
            "the {size}-state section counts only non-air cells"
        );
        let expected: Vec<u32> = (0..size)
            .map(|index| {
                if index == 0 {
                    AIR
                } else {
                    SOLID_BASE + index as u32
                }
            })
            .collect();
        assert_eq!(
            &decoded.sections[0].states[..size],
            expected.as_slice(),
            "the {size}-state palette round-trips"
        );
    }
}

#[test]
fn biome_palette_widths_transition_at_the_registry_limits() {
    let tables = tables();
    for (size, expected_bits) in [(1usize, 0usize), (2, 1), (4, 2), (8, 3), (9, 7)] {
        let mut biomes = vec![PLAINS; CHUNK_BIOMES];
        for (index, cell) in biomes.iter_mut().enumerate().take(size).skip(1) {
            *cell = BIOME_BASE + index as u32;
        }
        let chunk = VanillaChunk::new(0, 0, vec![AIR; CHUNK_CELLS], biomes).expect("fixture");
        let decoded = decode_chunk(&encode_chunk(&chunk, &tables).expect("encode"));
        assert_eq!(
            decoded.sections[0].biome_bits, expected_bits,
            "{size} distinct biomes must use {expected_bits} bits"
        );
        assert_eq!(
            decoded.sections[1].biome_bits, 0,
            "the untouched sections stay single-valued at {size} distinct"
        );
        let expected: Vec<u32> = (0..size)
            .map(|index| {
                if index == 0 {
                    PLAINS
                } else {
                    BIOME_BASE + index as u32
                }
            })
            .collect();
        assert_eq!(
            &decoded.biomes()[..size],
            expected.as_slice(),
            "the {size}-biome palette round-trips"
        );
    }
}

#[test]
fn worst_case_direct_palette_column_fits_one_batch() {
    let tables = tables();
    // 301 distinct states per section: past 256 the container goes direct with
    // the registry-derived width, the largest legal payload per section.
    let distinct = (PROBE_STATES as usize).min(300);
    let chunk = column_chunk(
        5,
        -9,
        |x, y, z| {
            let cell = x + z * CHUNK_SIDE + (y - OVERWORLD_MIN_Y) as usize % SECTION_HEIGHT * 256;
            if cell.is_multiple_of(13) {
                AIR
            } else {
                SOLID_BASE + (cell % distinct) as u32
            }
        },
        |bx, by, bz| BIOME_BASE + ((bx + bz + (by - OVERWORLD_MIN_Y) as usize / 4) % 3) as u32,
    );
    let packet = encode_chunk(&chunk, &tables).expect("encode the worst case");
    let decoded = decode_chunk(&packet);
    assert_eq!(decoded.states(), chunk.states().to_vec());
    for (index, section) in decoded.sections.iter().enumerate() {
        assert_eq!(section.block_bits, 16, "section {index} must be direct");
        assert_eq!(section.biome_bits, 2, "three biomes need 2 bits");
    }
    assert_eq!(decoded.masks, [0x03ff_ffff, 0, 0, 0x03ff_ffff]);
    assert_eq!(decoded.sky.len(), LIGHT_SECTIONS);
    assert!(decoded.block_light.is_empty(), "block light is deferred");
    println!("worst-case full-column packet: {} bytes", packet.len());
    assert!(
        packet.len() > 240_000,
        "a direct column should be near a quarter megabyte, got {}",
        packet.len()
    );
    assert!(
        packet.len() <= MAX_CHUNK_PACKET_BYTES,
        "one chunk must fit the preview's batch budget of {MAX_CHUNK_PACKET_BYTES} bytes, got {}",
        packet.len()
    );
    assert!(
        16 * packet.len() > MAX_CHUNK_PACKET_BYTES,
        "the budget must actually bind at the batch level"
    );
}

#[test]
fn chunk_border_cells_index_at_the_end_of_their_plane() {
    let tables = tables();
    // Only the far border column (x=15, z=15) is filled, so a wrong stride
    // would move the marks to another neighbour's slot.
    let chunk = column_chunk(
        0,
        0,
        |x, y, z| {
            if x == 15 && z == 15 && (100..=102).contains(&y) {
                STONE
            } else if x == 15 || z == 15 {
                DIRT
            } else {
                AIR
            }
        },
        |_, _, _| PLAINS,
    );
    let decoded = decode_chunk(&encode_chunk(&chunk, &tables).expect("encode border column"));
    for y in 100..=102 {
        // Local plane index of the last cell of the last row.
        let plane = CHUNK_SIDE * CHUNK_SIDE - 1;
        let local_y = (y - (OVERWORLD_MIN_Y + 10 * SECTION_HEIGHT as i32)) as usize;
        assert_eq!(
            decoded.sections[10].states[local_y * CHUNK_SIDE * CHUNK_SIDE + plane],
            STONE,
            "x=15,z=15 at y={y} sits at the end of its plane"
        );
        assert_eq!(decoded.states()[state_index(15, y, 15)], STONE);
    }
    assert_eq!(decoded.states()[state_index(0, 101, 0)], AIR);
    assert_eq!(
        decoded.states()[state_index(15, 101, 0)],
        DIRT,
        "x edge only"
    );
    assert_eq!(
        decoded.states()[state_index(0, 101, 15)],
        DIRT,
        "z edge only"
    );
    // Heightmap columns are ordered x-fastest as well: only the border columns
    // are captured at all, and the last entry is the (15, 15) corner.
    let surface = decoded.heightmap(HeightmapKind::WorldSurface);
    assert_eq!(surface[HEIGHTMAP_COLUMNS - 1], OVERWORLD_HEIGHT as u32);
    assert_eq!(surface[0], 0, "the interior column is empty");
    for (index, value) in surface.iter().enumerate() {
        let x = index % CHUNK_SIDE;
        let z = index / CHUNK_SIDE;
        let expected = if x == 15 || z == 15 {
            OVERWORLD_HEIGHT as u32
        } else {
            0
        };
        assert_eq!(*value, expected, "column {x},{z} heightmap");
    }
}

#[test]
fn encoding_is_byte_identical_and_position_dependent() {
    let tables = tables();
    let first = column_chunk(
        -1,
        2,
        |x, y, z| {
            if y == OVERWORLD_MIN_Y {
                BEDROCK
            } else if (60..70).contains(&y) && x == z % CHUNK_SIDE {
                LAVA
            } else {
                AIR
            }
        },
        |bx, _, bz| if bx + bz > 3 { FOREST } else { DESERT },
    );
    let other = column_chunk(
        14,
        2,
        |x, y, z| {
            if y == OVERWORLD_MIN_Y {
                BEDROCK
            } else if (60..70).contains(&y) && x == z % CHUNK_SIDE {
                LAVA
            } else {
                AIR
            }
        },
        |bx, _, bz| if bx + bz > 3 { FOREST } else { DESERT },
    );
    let encoded = encode_chunk(&first, &tables).expect("encode");
    assert_eq!(
        encoded,
        encode_chunk(&first, &tables).expect("re-encode"),
        "the adapter holds no state"
    );
    let rebuilt = VanillaChunk::new(-1, 2, first.states().to_vec(), first.biomes().to_vec())
        .expect("rebuilt column");
    assert_eq!(
        encoded,
        encode_chunk(&rebuilt, &tables).expect("encode rebuild")
    );
    assert_ne!(
        encoded,
        encode_chunk(&other, &tables).expect("encode neighbour"),
        "a different chunk position must produce a different packet"
    );
}

/// Manual verification against operator-provisioned world data, which is never
/// committed (`docs/PROVENANCE.md`, ADR-0014 as amended). Run with:
/// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored`
///
/// Protocol ids come from `RUSTMC_REGISTRY_TABLE` when the operator has a
/// provisioned versioned table; that variable may hold the JSON document
/// itself or the path of the table file (a complete 26.3 state registry is far
/// too large to carry inline). Otherwise the vocabulary the generator actually
/// emits is collected and given deterministic positional ids: those ids are
/// invented, but the palette choice, heightmaps, light, and field order are the
/// real ones, which is what this smoke is for.
#[test]
#[ignore = "requires operator-provisioned local data"]
fn smoke_encodes_a_provisioned_overworld_column() {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let generator =
        VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("operator data loads");
    assert_eq!(generator.min_y(), OVERWORLD_MIN_Y, "overworld profile");
    let tables = match std::env::var("RUSTMC_REGISTRY_TABLE") {
        Ok(table) => {
            let text = if table.trim_start().starts_with('{') {
                table
            } else {
                std::fs::read_to_string(&table)
                    .unwrap_or_else(|error| panic!("{table} is not readable: {error}"))
            };
            RegistryTables::from_provisioned(&text).expect("provisioned table loads")
        }
        Err(_) => {
            let mut states: BTreeSet<String> = BTreeSet::new();
            let mut biomes: BTreeSet<String> = BTreeSet::new();
            for z in 0..4 {
                for x in 0..32 {
                    states.extend(generator.column_ids(x, z).into_iter().flatten());
                    for layer in 0..BIOME_LAYERS {
                        let y = OVERWORLD_MIN_Y + layer as i32 * SECTION_BIOME_SIDE as i32;
                        if let Some(biome) = generator.biome(x, z, y) {
                            biomes.insert(biome);
                        }
                    }
                }
            }
            states.insert("minecraft:air".to_string());
            assert!(
                !biomes.is_empty(),
                "operator data must place at least one biome"
            );
            let position = |index: usize| u32::try_from(index).expect("vocabulary fits in u32");
            RegistryTables::new(
                SUPPORTED_VERSION,
                SUPPORTED_PROTOCOL,
                STATE_COUNT,
                BIOME_COUNT,
                states
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, name)| (name, position(index))),
                biomes
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, name)| (name, position(index))),
            )
            .expect("positional table from the emitted vocabulary")
        }
    };
    let mut visited_columns = 0;
    let cancelled = chunk_from_generator_cancellable(&generator, 0, 0, &tables, || {
        visited_columns += 1;
        visited_columns > 8
    })
    .expect("cancel after a few complete columns");
    assert!(cancelled.is_none());
    assert_eq!(visited_columns, 9);
    let mut vertically_distinct_columns = 0;
    for (chunk_x, chunk_z) in [(0, 0), (-1, 2)] {
        let chunk =
            chunk_from_generator(&generator, chunk_x, chunk_z, &tables).expect("resolve a column");
        let packet = encode_chunk(&chunk, &tables).expect("encode a real column");
        let decoded = decode_chunk(&packet);
        assert_eq!((decoded.x, decoded.z), (chunk_x, chunk_z), "header");
        assert_eq!(
            decoded.states(),
            chunk.states().to_vec(),
            "real column survives"
        );
        assert_eq!(
            decoded.biomes(),
            chunk.biomes().to_vec(),
            "real biomes survive"
        );
        let origin_x = chunk_x * CHUNK_SIDE as i32;
        let origin_z = chunk_z * CHUNK_SIDE as i32;
        for bz in 0..SECTION_BIOME_SIDE {
            for bx in 0..SECTION_BIOME_SIDE {
                let x = origin_x + (bx * SECTION_BIOME_SIDE) as i32;
                let z = origin_z + (bz * SECTION_BIOME_SIDE) as i32;
                let mut column_biomes = BTreeSet::new();
                for layer in 0..BIOME_LAYERS {
                    let y = OVERWORLD_MIN_Y + (layer * SECTION_BIOME_SIDE) as i32;
                    let expected = generator.biome(x, z, y).expect("placed biome");
                    column_biomes.insert(expected.clone());
                    assert_eq!(
                        chunk.biome_at_world(x, y, z),
                        Some(tables.biome(&expected).expect("registered biome")),
                        "quart cell at ({x}, {y}, {z})"
                    );
                }
                vertically_distinct_columns += usize::from(column_biomes.len() > 1);
            }
        }
        let surface = decoded.heightmap(HeightmapKind::WorldSurface);
        assert!(
            surface.iter().any(|value| *value > 0),
            "a real overworld column must have a surface"
        );
        assert!(
            surface
                .iter()
                .all(|value| *value <= OVERWORLD_HEIGHT as u32),
            "heightmaps stay inside the 9-bit range"
        );
        println!(
            "provisioned smoke OK: chunk {},{} is {} bytes",
            chunk_x,
            chunk_z,
            packet.len()
        );
    }
    assert!(
        vertically_distinct_columns > 0,
        "operator data must exercise biome changes with height"
    );
}
