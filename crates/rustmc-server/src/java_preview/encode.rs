//! Chunk packet encoding for the preview: palettes, packed arrays,
//! biomes and the view ordering around a player.

use super::*;

pub(super) fn push_wire_frame(
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

pub(super) fn block(chunk: &Chunk, x: usize, y: i32, z: usize) -> Block {
    if y < 0 {
        Block::Air
    } else {
        chunk.block(x, y as usize, z).unwrap_or(Block::Air)
    }
}

/// Plants with no collision, which only `WORLD_SURFACE` captures.
pub(super) fn is_passable_plant(block: Block) -> bool {
    matches!(
        block,
        Block::ShortGrass | Block::TallGrassLower | Block::TallGrassUpper
    )
}

/// Pack fixed-width values without crossing a 64-bit word boundary.
pub(super) fn packed(values: &[u16], bits: usize) -> Vec<u64> {
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

pub(super) fn biome_index(biome: Biome) -> u16 {
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

pub(super) fn build_palette<T: Copy + Ord>(values: &[T]) -> (Vec<T>, Vec<u16>) {
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
