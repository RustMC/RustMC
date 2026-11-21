//! Unit tests for `storage::chunk`.

use super::*;
use crate::server::world::terrain::{TerrainPalette, generate, tests::tables};
use crate::storage::nbt::{parse_root, write_root};
use crate::world::{Block, Generator};

#[test]
fn a_generated_column_with_edits_round_trips_through_nbt_bytes() {
    let tables = tables();
    let palette = TerrainPalette::resolve(&tables).expect("palette");
    let mut column = generate(Generator::new(9), &palette, -2, 5);
    column.set(3, 100, 4, Block::Obsidian as u32);
    column.set(15, -64, 15, Block::Sand as u32);
    let tag = encode(&column, &tables, 1234).expect("encode");
    let mut bytes = Vec::new();
    write_root(&tag, &mut bytes);
    let parsed = parse_root(&bytes).expect("parse");
    assert_eq!(
        parsed.get("DataVersion").and_then(Tag::as_i32),
        Some(DATA_VERSION)
    );
    assert_eq!(parsed.get("yPos").and_then(Tag::as_i32), Some(-4));
    // Compare contents: decoding stores single-state sections compactly.
    assert_eq!(
        decode(&parsed, &tables).expect("decode").to_adapter(),
        column.to_adapter()
    );
}

#[test]
fn packing_never_spans_longs_and_round_trips_every_width() {
    for bits in 1..=15 {
        let count = 4096;
        let indices: Vec<usize> = (0..count).map(|i| i % (1 << bits)).collect();
        let packed = pack(&indices, bits);
        assert_eq!(packed.len(), count.div_ceil(64 / bits), "{bits} bits");
        assert_eq!(unpack(&packed, bits, count), Some(indices));
    }
    assert_eq!(unpack(&[0; 3], 4, 4096), None, "wrong length is rejected");
}

#[test]
fn palettes_use_the_26_3_entry_forms_and_parse_back() {
    let plain = block_palette(&["minecraft:stone", "minecraft:dirt"]);
    assert_eq!(
        plain,
        Tag::List(vec![
            Tag::String("minecraft:stone".to_owned()),
            Tag::String("minecraft:dirt".to_owned())
        ])
    );
    let mixed = block_palette(&["minecraft:stone", "minecraft:oak_log[axis=y]"]);
    let entries = mixed.as_list().expect("list");
    assert_eq!(
        entries[0].get("").and_then(Tag::as_str),
        Some("minecraft:stone")
    );
    assert_eq!(
        entries[1].get("id").and_then(Tag::as_str),
        Some("minecraft:oak_log")
    );
    assert_eq!(
        entries.iter().map(entry_key).collect::<Vec<_>>(),
        vec![
            Some("minecraft:stone".to_owned()),
            Some("minecraft:oak_log[axis=y]".to_owned())
        ]
    );
    let older = compound(&[
        ("Name", Tag::String("minecraft:oak_log".to_owned())),
        (
            "Properties",
            compound(&[("axis", Tag::String("x".to_owned()))]),
        ),
    ]);
    assert_eq!(
        entry_key(&older).as_deref(),
        Some("minecraft:oak_log[axis=x]")
    );
}

#[test]
fn unknown_blocks_fail_instead_of_turning_into_air() {
    let tables = tables();
    let mut tag = encode(&Column::filled(0, 0, 0, 0), &tables, 0).expect("encode");
    if let Tag::Compound(map) = &mut tag
        && let Some(Tag::List(sections)) = map.get_mut("sections")
        && let Some(Tag::Compound(section)) = sections.first_mut()
    {
        section.insert(
            "block_states".to_owned(),
            compound(&[(
                "palette",
                Tag::List(vec![Tag::String("minecraft:not_a_block".to_owned())]),
            )]),
        );
    }
    assert_eq!(
        decode(&tag, &tables),
        Err(ChunkError::UnknownName("minecraft:not_a_block".to_owned()))
    );
}

#[test]
#[ignore = "requires operator-provisioned local data"]
fn vanilla_save_chunks_decode_with_the_real_registry_table() {
    use crate::storage::region::RegionStore;
    let table = std::env::var("RUSTMC_CHUNK_REGISTRY").expect("RUSTMC_CHUNK_REGISTRY");
    let save = std::env::var("RUSTMC_VANILLA_SAVE").expect("RUSTMC_VANILLA_SAVE");
    let tables = RegistryTables::from_provisioned(&std::fs::read_to_string(table).expect("table"))
        .expect("tables");
    let mut store = RegionStore::new(std::path::Path::new(&save));
    let mut decoded = 0;
    for z in 0..32 {
        for x in 0..32 {
            let Some(root) = store.chunk_root(x, z).expect("region") else {
                continue;
            };
            let column = decode(&root, &tables).expect("decode");
            let again = decode(&encode(&column, &tables, 0).expect("encode"), &tables)
                .expect("decode again");
            assert_eq!(again.to_adapter(), column.to_adapter(), "chunk {x} {z}");
            decoded += 1;
        }
    }
    assert!(decoded > 0, "the save has chunks in region 0 0");
}
