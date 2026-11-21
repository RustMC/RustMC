//! Unit tests for `server::world::terrain`.

use super::*;
use crate::chunk_adapter::{SUPPORTED_PROTOCOL, SUPPORTED_VERSION};

/// A registry table holding exactly the default-terrain names, with block
/// ids in `Block` order and biome ids in `Biome` order.
pub(crate) fn tables() -> RegistryTables {
    let states: Vec<(String, u32)> = Block::ALL
        .iter()
        .map(|block| (block.state_name().to_owned(), *block as u32))
        .collect();
    let biomes: Vec<(String, u32)> = Biome::ALL
        .iter()
        .enumerate()
        .map(|(id, biome)| (biome.identifier().to_owned(), id as u32))
        .collect();
    RegistryTables::new(
        SUPPORTED_VERSION,
        SUPPORTED_PROTOCOL,
        states.len() as u32,
        biomes.len() as u32,
        states,
        biomes,
    )
    .expect("tables")
}

#[test]
fn columns_have_a_bedrock_floor_solid_base_and_open_sky() {
    let palette = TerrainPalette::resolve(&tables()).expect("palette");
    let generator = Generator::new(7);
    let column = generate(generator, &palette, 0, 0);
    assert_eq!(
        column.get(3, OVERWORLD_MIN_Y, 4),
        Some(Block::Bedrock as u32)
    );
    assert_eq!(column.get(3, -10, 4), Some(Block::Stone as u32));
    assert_eq!(
        column.get(3, 0, 4),
        Some(Block::Stone as u32),
        "old bedrock row"
    );
    assert_eq!(column.get(3, 300, 4), Some(Block::Air as u32));
    let surface = surface_y(generator, 3, 4);
    assert_ne!(column.get(3, surface - 1, 4), Some(Block::Air as u32));
    assert!(
        column.memory_bytes() < 16 * 16 * 384 * 4,
        "uniform sections stay compact"
    );
}

#[test]
fn generation_is_deterministic_and_encodable() {
    let tables = tables();
    let palette = TerrainPalette::resolve(&tables).expect("palette");
    let a = generate(Generator::new(42), &palette, -3, 9);
    let b = generate(Generator::new(42), &palette, -3, 9);
    assert_eq!(a, b);
    let body = crate::chunk_adapter::encode_chunk_body(&a.to_adapter().expect("column"), &tables)
        .expect("encode");
    assert_eq!(body[..8], [0xff, 0xff, 0xff, 0xfd, 0, 0, 0, 9]);
}
