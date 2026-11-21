//! Unit tests for `oracle`.

use super::*;
use crate::nbt::compound;

fn palette_entry(name: &str) -> Tag {
    compound(&[
        ("Name", Tag::String(name.into())),
        ("Properties", compound(&[])),
    ])
}

/// 16x16x16 section at Y=4 (blocks 64..=79): everything grass_block,
/// single biome, heightmap says surface top at 75 for all columns.
/// The chunk has no yPos, so the world minimum is -64 and heightmaps are
/// packed min_y-relative (140 = 75 - (-64) + 1).
fn synthetic_chunk(biome_single: bool) -> Tag {
    let mut height_data = vec![0i64; 36]; // ceil(256*9/64) = 36
    let value = 140u64;
    for i in 0..256 {
        let bit = i * 9;
        let long = bit / 64;
        let off = bit % 64;
        height_data[long] |= (value << off) as i64;
        if off + 9 > 64 {
            height_data[long + 1] |= (value >> (64 - off)) as i64;
        }
    }
    let block_states = compound(&[(
        "palette",
        Tag::List(vec![palette_entry("minecraft:grass_block")]),
    )]);
    let biomes = if biome_single {
        compound(&[(
            "palette",
            Tag::List(vec![Tag::String("minecraft:plains".into())]),
        )])
    } else {
        // Two-value palette stored at 3 bits: 4 longs is also
        // consistent with 4 bits, so the validator must pick 3.
        let mut data = vec![0i64; 4]; // 64 entries, 21 per long
        for i in 0..64 {
            let long = i / 21;
            let off = (i % 21) * 3;
            let slot = if i < 40 { 0 } else { 1 };
            data[long] |= (slot as i64) << off;
        }
        compound(&[
            (
                "palette",
                Tag::List(vec![
                    Tag::String("minecraft:plains".into()),
                    Tag::String("minecraft:forest".into()),
                ]),
            ),
            ("data", Tag::LongArray(data)),
        ])
    };
    let section = compound(&[
        ("Y", Tag::Byte(4)),
        ("block_states", block_states),
        ("biomes", biomes),
    ]);
    compound(&[
        (
            "Heightmaps",
            compound(&[("WORLD_SURFACE", Tag::LongArray(height_data))]),
        ),
        ("sections", Tag::List(vec![section])),
    ])
}

#[test]
fn extracts_surface_block_height_and_single_biome() {
    let root = synthetic_chunk(true);
    let column = column_truth(&root, 1, -2, 3, 9).unwrap().expect("column");
    assert_eq!(column.x, 19);
    assert_eq!(column.z, -23);
    assert_eq!(column.surface_y, 75);
    assert_eq!(column.top_block.as_deref(), Some("minecraft:grass_block"));
    assert_eq!(column.biome.as_deref(), Some("minecraft:plains"));
}

#[test]
fn decodes_multi_value_biome_palette() {
    let root = synthetic_chunk(false);
    // local_y 11 within section => y index 11/4=2; lz/4 chooses halves.
    let first = column_truth(&root, 0, 0, 0, 0).unwrap().unwrap();
    assert_eq!(first.biome.as_deref(), Some("minecraft:plains"));
    let second = column_truth(&root, 0, 0, 0, 15).unwrap().unwrap();
    assert_eq!(second.biome.as_deref(), Some("minecraft:forest"));
}

#[test]
fn saved_biome_cells_keep_quart_bottom_y_and_negative_chunk_origin() {
    let cells = biome_cells(&synthetic_chunk(false), 1, -2).expect("saved quart cells");
    assert_eq!(cells.len(), 64);
    assert_eq!(
        cells.first(),
        Some(&BiomeCell {
            x: 16,
            y: 64,
            z: -32,
            name: "minecraft:plains".to_owned(),
        })
    );
    assert!(cells.contains(&BiomeCell {
        x: 16,
        y: 72,
        z: -24,
        name: "minecraft:forest".to_owned(),
    }));
    assert_eq!(
        cells.last().map(|cell| (cell.x, cell.y, cell.z)),
        Some((28, 76, -20))
    );
}

#[test]
fn spanning_and_non_spanning_unpackers_agree_with_manual_packing() {
    // 9-bit spanning: values 500.. pattern crossing long boundaries.
    let mut longs = vec![0i64; 36];
    let values: Vec<u32> = (0..256).map(|i| (i % 512) as u32).collect();
    for (i, v) in values.iter().enumerate() {
        let bit = i * 9;
        let long = bit / 64;
        let off = bit % 64;
        longs[long] |= (*v as i64) << off;
        if off + 9 > 64 {
            longs[long + 1] |= (*v as i64) >> (64 - off);
        }
    }
    assert_eq!(unpack_spanning(&longs, 9, 256), values);
    assert_eq!(unpack_non_spanning(&[0b1010_0101], 4, 4), vec![5, 10, 0, 0]);
}

/// 26.3 stores heightmaps non-spanning (7 values per long, 37 longs);
/// reading that layout as spanning only gets the first seven columns
/// right, so every column must be checked here.
#[test]
fn heightmaps_decode_from_their_stored_long_count() {
    let values: Vec<u32> = (0..256).map(|i| 1 + i as u32).collect();
    let mut non_spanning = vec![0i64; 37];
    let mut spanning = vec![0i64; 36];
    for (i, v) in values.iter().enumerate() {
        let long = i / 7;
        non_spanning[long] |= (*v as i64) << ((i % 7) * 9);
        let bit = i * 9;
        spanning[bit / 64] |= (*v as i64) << (bit % 64);
        if bit % 64 + 9 > 64 {
            spanning[bit / 64 + 1] |= (*v as i64) >> (64 - bit % 64);
        }
    }
    assert_eq!(heightmap_heights(&non_spanning, 256), values);
    assert_eq!(heightmap_heights(&spanning, 256), values);
    for index in [0usize, 6, 7, 135, 255] {
        let mut heightmaps = std::collections::BTreeMap::new();
        heightmaps.insert(
            "WORLD_SURFACE".to_owned(),
            Tag::LongArray(non_spanning.clone()),
        );
        let root = compound(&[
            ("yPos", Tag::Int(-4)),
            ("Heightmaps", Tag::Compound(heightmaps)),
            ("sections", Tag::List(vec![])),
        ]);
        let (lx, lz) = ((index % 16) as u8, (index / 16) as u8);
        assert_eq!(
            surface_top(&root, lx, lz).unwrap(),
            Some(index as i32 - 64),
            "column {lx},{lz}"
        );
    }
}

#[test]
fn state_name_handles_26_3_palette_forms() {
    let bare = compound(&[("", Tag::String("minecraft:stone".into()))]);
    assert_eq!(state_name(&bare).as_deref(), Some("minecraft:stone"));
    let with_props = compound(&[
        ("id", Tag::String("minecraft:water".into())),
        (
            "properties",
            compound(&[("level", Tag::String("3".into()))]),
        ),
    ]);
    assert_eq!(
        state_name(&with_props).as_deref(),
        Some("minecraft:water[level=3]")
    );
    assert_eq!(
        state_name(&Tag::String("minecraft:dirt".into())).as_deref(),
        Some("minecraft:dirt")
    );
}

#[test]
fn ambiguous_palette_width_prefers_slot_consistency() {
    // Two-entry biome palette stored at 4 bits: at 3 bits the second
    // slot decodes to 2 (out of range), at 4 bits both slots are valid.
    let palette = vec![
        Tag::String("minecraft:plains".into()),
        Tag::String("minecraft:forest".into()),
    ];
    let data = Tag::LongArray(vec![0x11, 0, 0, 0]);
    let value = palette_value(&palette, Some(&data), 1, 64).unwrap();
    assert_eq!(value.unwrap().as_str(), Some("minecraft:forest"));
}

#[test]
fn heightmaps_are_relative_to_stored_y_pos() {
    // yPos 0 with value 1 => absolute top block y = 0.
    let mut root = synthetic_chunk(true);
    let Tag::Compound(map) = &mut root else {
        unreachable!()
    };
    map.insert("yPos".to_string(), Tag::Int(0));
    let column = column_truth(&root, 0, 0, 0, 0).unwrap().unwrap();
    assert_eq!(column.surface_y, 140 - 1);
}

#[test]
fn compare_report_counts_matches_and_caps_mismatches() {
    let generator = Generator::new(2026);
    let columns: Vec<VanillaColumn> = [(0i64, 0i64), (256, 0)]
        .into_iter()
        .map(|(x, z)| VanillaColumn {
            x,
            z,
            surface_y: generator.height(x, z) as i32,
            top_block: None,
            biome: Some(generator.biome(x, z).identifier().to_string()),
        })
        .collect();
    let report = compare_columns(columns.iter().cloned(), &generator, 5);
    assert_eq!(report.columns, 2);
    assert_eq!(report.height_matches, 2);
    assert_eq!(report.biome_matches, 2);
    assert!(report.mismatches.is_empty());
    assert_eq!(percent(2, 2), 100.0);
    assert_eq!(percent(0, 0), 0.0);
}

#[test]
fn categories_classify_air_family_and_fluids() {
    assert_eq!(block_category("minecraft:cave_air"), Category::Air);
    assert_eq!(block_category("minecraft:structure_void"), Category::Air);
    assert_eq!(block_category("minecraft:water[level=3]"), Category::Fluid);
    assert_eq!(
        block_category("minecraft:deepslate_gold_ore"),
        Category::Solid
    );
}

fn solid_section(y: i32, block: &str) -> Tag {
    compound(&[
        ("Y", Tag::Byte(y as i8)),
        (
            "block_states",
            compound(&[("palette", Tag::List(vec![palette_entry(block)]))]),
        ),
    ])
}

/// Same packed heightmap as `synthetic_chunk`: top block 75 over the
/// implicit -64 world floor.
fn heightmap_root(sections: Vec<Tag>) -> Tag {
    let mut height_data = vec![0i64; 36];
    let value = 140u64;
    for i in 0..256 {
        let bit = i * 9;
        let long = bit / 64;
        let off = bit % 64;
        height_data[long] |= (value << off) as i64;
        if off + 9 > 64 {
            height_data[long + 1] |= (value >> (64 - off)) as i64;
        }
    }
    compound(&[
        (
            "Heightmaps",
            compound(&[("WORLD_SURFACE", Tag::LongArray(height_data))]),
        ),
        ("sections", Tag::List(sections)),
    ])
}

#[test]
fn profiles_store_categories_with_air_gaps() {
    let root = heightmap_root(vec![
        solid_section(0, "minecraft:water"),
        solid_section(4, "minecraft:stone"),
    ]);
    let profile = column_profile(&root, 0, 0, 3, 9).unwrap().expect("profile");
    assert_eq!((profile.x, profile.z, profile.surface_y), (3, 9, 75));
    assert_eq!(profile.min_y, 0);
    assert_eq!(profile.categories.len(), 76);
    assert_eq!(profile.categories[0], Category::Fluid);
    assert_eq!(profile.categories[15], Category::Fluid);
    // The unsaved 16..=63 span is empty: recorded as air.
    assert_eq!(profile.categories[16], Category::Air);
    assert_eq!(profile.categories[63], Category::Air);
    assert_eq!(profile.categories[64], Category::Solid);
    assert_eq!(profile.categories[75], Category::Solid);
    assert_eq!(profile.block_ids[16], "minecraft:air");
    assert_eq!(profile.block_ids[64], "minecraft:stone");
}

#[test]
fn stored_chunks_without_heightmaps_are_ungenerated() {
    // A chunk the client wrote before the height pass: no Heightmaps
    // compound at all. That is an ungenerated column, not a decode
    // failure that would abort a sample.
    let root = compound(&[(
        "sections",
        Tag::List(vec![solid_section(0, "minecraft:stone")]),
    )]);
    assert_eq!(column_truth(&root, 0, 0, 3, 9).unwrap(), None);
    assert_eq!(column_profile(&root, 0, 0, 3, 9).unwrap(), None);
}

#[derive(Clone, Copy)]
struct HalfAir;

impl ColumnSource for HalfAir {
    fn column_height(&self, _x: i64, _z: i64) -> i64 {
        0
    }
    fn column_biome(&self, _x: i64, _z: i64) -> String {
        "test".to_owned()
    }
    fn column_substance(&self, _x: i64, _z: i64, y: i32) -> Option<Category> {
        Some(if y < 32 {
            Category::Solid
        } else {
            Category::Air
        })
    }
}

#[test]
fn substance_report_counts_positions_bands_and_residuals() {
    let profile = ColumnProfile {
        x: 0,
        z: 0,
        surface_y: 3,
        min_y: 0,
        categories: vec![
            Category::Solid,
            Category::Air,
            Category::Air,
            Category::Solid,
        ],
        block_ids: vec![
            "minecraft:stone".to_owned(),
            "minecraft:air".to_owned(),
            "minecraft:air".to_owned(),
            "minecraft:stone".to_owned(),
        ],
    };
    // The legacy generator has no 3D answer: nothing is counted.
    let pending = compare_substance([profile.clone()], &Generator::new(2026), 5);
    assert_eq!((pending.columns, pending.positions), (1, 0));
    let report = compare_substance([profile], &HalfAir, 5);
    assert_eq!((report.positions, report.matches), (4, 2));
    assert_eq!(report.fail_positions, vec![(0, 0, 1), (0, 0, 2)]);
    assert_eq!((report.near_positions, report.near_matches), (4, 2));
    assert_eq!(report.middle_positions, 0);
    assert_eq!(report.deep_positions, 0);
    assert_eq!(
        report
            .residuals
            .get(&(Category::Air, Category::Solid))
            .copied(),
        Some(2)
    );
    assert_eq!(
        report
            .residual_bands
            .get(&((Category::Air, Category::Solid), 0))
            .copied(),
        Some(2)
    );
    assert_eq!(
        report.marginals.get(&0),
        Some(&BandStats {
            vanilla: [2, 0, 2],
            rustmc: [0, 0, 4],
        })
    );
}

/// Descent stub with an explicit `min_y` and per-row answers, so the
/// block report is testable without operator worldgen data.
struct StubDescent {
    min_y: i32,
    ids: Vec<Option<String>>,
}

impl ColumnDescent for StubDescent {
    fn column_block_ids(&self, _x: i32, _z: i32) -> Vec<Option<String>> {
        self.ids.clone()
    }
    fn column_min_y(&self) -> i32 {
        self.min_y
    }
}

/// Saved column from Y 30 to Y 40 with two void rows (36 and 38) and
/// one property-carrying state, the fixture for the block report.
fn block_profile(x: i64) -> ColumnProfile {
    let saved: [&str; 11] = [
        "minecraft:stone",
        "minecraft:stone",
        "minecraft:stone",
        "minecraft:stone",
        "minecraft:stone",
        "minecraft:grass_block[snowy=false]",
        "minecraft:cave_air",
        "minecraft:stone",
        "minecraft:air",
        "minecraft:stone",
        "minecraft:stone",
    ];
    ColumnProfile {
        x,
        z: 0,
        surface_y: 40,
        min_y: 30,
        categories: saved
            .iter()
            .map(|name| block_category(name))
            .collect::<Vec<_>>(),
        block_ids: saved.iter().map(|name| (*name).to_owned()).collect(),
    }
}

fn stub_descent() -> StubDescent {
    let ids: [&str; 10] = [
        "minecraft:cave_air",
        "minecraft:cave_air",
        "minecraft:stone",
        "minecraft:granite",
        "minecraft:grass_block[snowy=true]",
        "minecraft:stone",
        "minecraft:stone",
        "minecraft:cave_air",
        "minecraft:air",
        "minecraft:stone",
    ];
    StubDescent {
        // Index 0 is Y 31: the saved Y 30 row is out of the generator
        // build range for this sample.
        min_y: 31,
        ids: ids.iter().map(|name| Some((*name).to_owned())).collect(),
    }
}

#[test]
fn block_report_scores_bands_air_family_and_cave_context() {
    let report = compare_block_ids([block_profile(0)], &stub_descent(), 20);
    // One stored row below the generator's build range is reported,
    // not silently scored.
    assert_eq!((report.columns, report.positions), (1, 10));
    assert_eq!(report.out_of_range_rows, 1);
    assert_eq!(report.out_of_domain_columns, 0);
    assert_eq!(report.declined_columns, 0);
    // Y 31 and 32 (we opened rock the save fills), 34 (granite vs
    // stone), 36 (stone in a saved void), 38 (cave_air vs air), and
    // 39 (air vs stone) disagree on base ids; only the 38 pair survives
    // air-family normalisation.
    assert_eq!((report.exact_base, report.exact_base_air_family), (4, 5));
    assert_eq!(
        report.bands.get(&0),
        Some(&Agreement {
            positions: 1,
            matches: 0,
            matches_air_family: 0
        })
    );
    assert_eq!(
        report.bands.get(&32),
        Some(&Agreement {
            positions: 9,
            matches: 4,
            matches_air_family: 5
        })
    );
    // Void rows are 36 and 38; Y 31 is five rows from the nearest one,
    // everything else in the sample is cave wall.
    assert_eq!(
        report.cave,
        [
            (
                CaveContext::Void,
                Agreement {
                    positions: 2,
                    matches: 0,
                    matches_air_family: 1
                }
            ),
            (
                CaveContext::CaveWall,
                Agreement {
                    positions: 7,
                    matches: 4,
                    matches_air_family: 4
                }
            ),
            (
                CaveContext::IntactRock,
                Agreement {
                    positions: 1,
                    matches: 0,
                    matches_air_family: 0
                }
            ),
        ]
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>()
    );
    // Three rows we opened that the save fills (Y 31 far from any void,
    // Y 32 and Y 39 against a cave wall) and one row the save leaves
    // air that we filled.
    assert_eq!(
        (
            report.extra_carves,
            report.extra_carves_near_void,
            report.missed_carves
        ),
        (3, 2, 1)
    );
    assert_eq!(
        report
            .missed_carves_bands
            .iter()
            .map(|(band, count)| (*band, *count))
            .collect::<Vec<_>>(),
        vec![(32, 1)]
    );
    assert_eq!(
        report
            .extra_carves_bands
            .iter()
            .map(|(band, count)| (*band, *count))
            .collect::<Vec<_>>(),
        vec![(0, 1), (32, 2)]
    );
    assert_eq!(
        report.residuals.get(&(
            "minecraft:stone".to_owned(),
            "minecraft:cave_air".to_owned()
        )),
        Some(&2)
    );
    assert_eq!(
        report
            .residuals
            .get(&("minecraft:air".to_owned(), "minecraft:cave_air".to_owned())),
        Some(&1)
    );
    assert_eq!(
        report.residuals.get(&(
            "minecraft:cave_air".to_owned(),
            "minecraft:stone".to_owned()
        )),
        Some(&1)
    );
    assert_eq!(report.residuals.values().sum::<usize>(), 6);
    // Air-name bookkeeping collapses to one air key: the 38 pair
    // disappears and the 36 pair is keyed as air, not cave_air. The
    // property-carrying grass_block pair is never a residual at all.
    assert_eq!(report.residuals_air_family.values().sum::<usize>(), 5);
    // The cave view is the same population minus the intact-rock row:
    // what goes wrong next to a saved void.
    assert_eq!(
        report
            .residuals_cave
            .iter()
            .map(|(pair, count)| (pair.clone(), *count))
            .collect::<Vec<_>>(),
        vec![
            (
                ("minecraft:air".to_owned(), "minecraft:cave_air".to_owned()),
                1
            ),
            (
                (
                    "minecraft:cave_air".to_owned(),
                    "minecraft:stone".to_owned()
                ),
                1
            ),
            (
                ("minecraft:stone".to_owned(), "minecraft:air".to_owned()),
                1
            ),
            (
                (
                    "minecraft:stone".to_owned(),
                    "minecraft:cave_air".to_owned()
                ),
                1
            ),
            (
                ("minecraft:stone".to_owned(), "minecraft:granite".to_owned()),
                1
            ),
        ]
    );
    assert_eq!(
        report
            .residuals_air_family
            .get(&("minecraft:stone".to_owned(), "minecraft:air".to_owned())),
        Some(&3)
    );
    assert_eq!(
        report
            .residuals_air_family
            .get(&("minecraft:air".to_owned(), "minecraft:stone".to_owned())),
        Some(&1)
    );
    assert_eq!(
        report.residuals_air_family.get(&(
            "minecraft:cave_air".to_owned(),
            "minecraft:stone".to_owned()
        )),
        None
    );
    assert_eq!(
        report.fail_positions,
        vec![
            (0, 0, 31),
            (0, 0, 32),
            (0, 0, 34),
            (0, 0, 36),
            (0, 0, 38),
            (0, 0, 39)
        ]
    );
}

#[test]
fn block_report_caps_fails_and_reports_unscorable_columns() {
    let report = compare_block_ids([block_profile(0)], &stub_descent(), 2);
    assert_eq!(report.fail_positions.len(), 2);
    // A descent with no answer anywhere is a declined column, not a
    // column of air disagreements.
    let empty = StubDescent {
        min_y: 31,
        ids: vec![None; 10],
    };
    let report = compare_block_ids([block_profile(0)], &empty, 5);
    assert_eq!(
        (report.columns, report.positions, report.declined_columns),
        (0, 0, 1)
    );
    // Coordinates outside the generator's domain are counted, not dropped.
    let report = compare_block_ids([block_profile(i64::from(i32::MAX) + 1)], &stub_descent(), 5);
    assert_eq!(
        (
            report.columns,
            report.positions,
            report.out_of_domain_columns
        ),
        (0, 0, 1)
    );
}

#[test]
fn air_family_name_collapses_only_the_air_family() {
    assert_eq!(air_family_name("minecraft:cave_air"), "minecraft:air");
    assert_eq!(air_family_name("minecraft:void_air"), "minecraft:air");
    assert_eq!(air_family_name("minecraft:stone"), "minecraft:stone");
    assert_eq!(air_family_name("minecraft:water"), "minecraft:water");
}

#[test]
fn census_family_classification_follows_public_block_names() {
    assert_eq!(
        census_family("minecraft:coal_ore"),
        Some(CensusFamily::Coal)
    );
    assert_eq!(
        census_family("minecraft:deepslate_iron_ore"),
        Some(CensusFamily::DeepslateIron)
    );
    assert_eq!(
        census_family("minecraft:raw_gold_block"),
        Some(CensusFamily::RawGold)
    );
    assert_eq!(
        census_family("minecraft:raw_copper_block"),
        Some(CensusFamily::RawCopper)
    );
    assert_eq!(
        census_family("minecraft:raw_iron_block"),
        Some(CensusFamily::RawIron)
    );
    assert_eq!(census_family("minecraft:oak_log"), Some(CensusFamily::Logs));
    assert_eq!(
        census_family("minecraft:flowering_azalea_leaves"),
        Some(CensusFamily::Leaves)
    );
    assert_eq!(
        census_family("minecraft:short_grass"),
        Some(CensusFamily::Grasses)
    );
    assert_eq!(
        census_family("minecraft:poppy"),
        Some(CensusFamily::Flowers)
    );
    for (name, family) in [
        ("minecraft:wildflowers", CensusFamily::Wildflowers),
        ("minecraft:bush", CensusFamily::Bushes),
        ("minecraft:firefly_bush", CensusFamily::Bushes),
        ("minecraft:rose_bush", CensusFamily::TallFlowers),
        ("minecraft:lilac", CensusFamily::TallFlowers),
        ("minecraft:peony", CensusFamily::TallFlowers),
        ("minecraft:sunflower", CensusFamily::TallFlowers),
        ("minecraft:pitcher_plant", CensusFamily::TallFlowers),
        ("minecraft:wither_rose", CensusFamily::Flowers),
        ("minecraft:crimson_stem", CensusFamily::Logs),
        ("minecraft:stripped_warped_stem", CensusFamily::Logs),
    ] {
        assert_eq!(census_family(name), Some(family), "{name}");
    }
    assert_eq!(
        census_family(base_block_name("minecraft:wildflowers[flower_amount=4]")),
        Some(CensusFamily::Wildflowers)
    );
    assert_eq!(
        census_family(base_block_name("minecraft:rose_bush[half=upper]")),
        Some(CensusFamily::TallFlowers)
    );
    assert_eq!(census_family("minecraft:stone"), Some(CensusFamily::Stone));
    assert_eq!(
        census_family("minecraft:granite"),
        Some(CensusFamily::Granite)
    );
    assert_eq!(census_family("minecraft:tuff"), Some(CensusFamily::Tuff));
    // Surface and crafted materials are not decoration families.
    assert_eq!(census_family("minecraft:grass_block"), None);
    assert_eq!(census_family("minecraft:cobblestone"), None);
    assert_eq!(census_family("minecraft:tuff_bricks"), None);
    assert_eq!(census_family("minecraft:big_dripleaf_stem"), None);
    assert_eq!(census_family("minecraft:mushroom_stem"), None);
    assert_eq!(census_family("minecraft:pumpkin_stem"), None);
    assert_eq!(census_family("minecraft:attached_melon_stem"), None);
    assert_eq!(census_family("minecraft:azalea"), None);
}

#[test]
fn section_name_counts_decodes_single_and_multi_entry_palettes() {
    let single = compound(&[(
        "block_states",
        compound(&[("palette", Tag::List(vec![palette_entry("minecraft:stone")]))]),
    )]);
    assert_eq!(
        section_name_counts(&single).unwrap(),
        vec![("minecraft:stone".to_owned(), 4096)]
    );
    // Two-entry palette at the minimal 1-bit width: the first 100
    // slots are coal, the rest air.
    let mut data = vec![0i64; 64];
    for i in 0..100 {
        data[i / 64] |= 1i64 << (i % 64);
    }
    let multi = compound(&[(
        "block_states",
        compound(&[
            (
                "palette",
                Tag::List(vec![
                    Tag::String("minecraft:air".into()),
                    Tag::String("minecraft:coal_ore".into()),
                ]),
            ),
            ("data", Tag::LongArray(data)),
        ]),
    )]);
    let counts = section_name_counts(&multi).unwrap();
    assert!(counts.contains(&("minecraft:coal_ore".to_owned(), 100)));
    assert!(counts.contains(&("minecraft:air".to_owned(), 3996)));
}

#[test]
fn census_bands_attribute_sections_to_their_32_block_window() {
    fn diamond_section(y: i32) -> Tag {
        compound(&[
            ("Y", Tag::Byte(y as i8)),
            (
                "block_states",
                compound(&[(
                    "palette",
                    Tag::List(vec![palette_entry("minecraft:deepslate_diamond_ore")]),
                )]),
            ),
        ])
    }
    let root = compound(&[(
        "sections",
        Tag::List(vec![diamond_section(4), diamond_section(6)]),
    )]);
    let mut census = Census::default();
    census.record_chunk(&root).expect("census");
    assert_eq!(census.chunks, 1);
    assert_eq!(
        census.totals.get(&CensusFamily::DeepslateDiamond),
        Some(&8192)
    );
    // Y=4 -> 64..79 in band 64; Y=6 -> 96..111 in band 96.
    assert_eq!(
        census.bands.get(&(CensusFamily::DeepslateDiamond, 64)),
        Some(&4096)
    );
    assert_eq!(
        census.bands.get(&(CensusFamily::DeepslateDiamond, 96)),
        Some(&4096)
    );
}

#[test]
fn stored_placeholder_chunk_cannot_complete_a_census() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "rustmc-empty-census-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let region_dir = dir.join("region");
    std::fs::create_dir_all(&region_dir).expect("create synthetic region directory");

    // One stored chunk with an empty sections list, encoded as raw NBT in
    // a minimal Anvil region. It has a location entry but no block data.
    let mut nbt = vec![10, 0, 0, 9, 0, 8];
    nbt.extend_from_slice(b"sections");
    nbt.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let mut region = vec![0u8; 3 * 4096];
    region[0..4].copy_from_slice(&[0, 0, 2, 1]);
    region[8192..8196].copy_from_slice(&((nbt.len() + 1) as u32).to_be_bytes());
    region[8196] = 3; // uncompressed chunk NBT
    region[8197..8197 + nbt.len()].copy_from_slice(&nbt);
    std::fs::write(region_dir.join("r.0.0.mca"), region).expect("write synthetic region");

    let mut store = RegionStore::new(&dir);
    let census = census_chunks(&mut store, 0, 0, 0, 0).expect("census placeholder");
    assert_eq!((census.chunks, census.missing_chunks), (0, 1));
    let (named_chunks, names) =
        census_name_histogram(&mut store, 0, 0, 0, 0).expect("name placeholder");
    assert_eq!(named_chunks, 0);
    assert!(names.is_empty());
    let coverage = Coverage::new(census.chunks, 1, census.missing_chunks, ScoredSource::Save);
    assert!(matches!(
        coverage.outcome(false),
        CoverageOutcome::PartialRejected { .. }
    ));
    assert!(matches!(
        coverage.outcome(true),
        CoverageOutcome::PartialRejected { .. }
    ));

    std::fs::remove_dir_all(dir).expect("remove synthetic region");
}

#[test]
fn census_rejects_a_section_without_a_y_coordinate() {
    let section = compound(&[(
        "block_states",
        compound(&[("palette", Tag::List(vec![palette_entry("minecraft:stone")]))]),
    )]);
    let root = compound(&[("sections", Tag::List(vec![section]))]);
    let mut census = Census::default();
    assert_eq!(
        census.record_chunk(&root),
        Err("section without Y".to_owned())
    );
    assert_eq!(census.chunks, 0);
}

/// The published seed-2026 substance grid: 49 x 49 sampled columns, all
/// present. Anything else is a shortfall by construction.
#[test]
fn full_coverage_is_complete_and_shortfalls_are_not() {
    let full = Coverage::new(2401, 2401, 0, ScoredSource::Vanilla);
    assert!(full.is_complete());
    assert_eq!(full.unaccounted, 0);
    // The partial run recorded on the wrong world directory: 208 missing
    // chunks plus 153 ungenerated columns over the same 2401 grid.
    let partial = Coverage::new(2040, 2401, 208 + 153, ScoredSource::Vanilla);
    assert!(!partial.is_complete());
    assert_eq!(partial.unaccounted, 0);
    // Units that vanished without being scored or explained.
    let unaccounted = Coverage::new(2040, 2401, 0, ScoredSource::Vanilla);
    assert_eq!(unaccounted.unaccounted, 361);
    assert!(!unaccounted.is_complete());
    // A score above the requested grid cannot pass either.
    let over = Coverage::new(2402, 2401, 0, ScoredSource::Vanilla);
    assert!(!over.is_complete());
}

#[test]
fn coverage_line_names_the_scored_source() {
    assert!(
        Coverage::new(0, 2401, 0, ScoredSource::Preview)
            .line()
            .contains("source=preview-not-vanilla-generator")
    );
    assert!(
        Coverage::new(0, 2401, 0, ScoredSource::Experimental)
            .line()
            .contains("source=experimental-not-vanilla-generator")
    );
    assert_eq!(
        Coverage::new(1, 1, 0, ScoredSource::Vanilla).line(),
        "coverage scored=1 expected=1 missing=0 unaccounted=0 source=vanilla"
    );
}

#[test]
fn allow_partial_changes_the_exit_status_not_the_printed_line() {
    let partial = Coverage::new(2040, 2401, 361, ScoredSource::Vanilla);
    let rejected = partial.outcome(false);
    let accepted = partial.outcome(true);
    assert_eq!(rejected.line(), accepted.line());
    assert_eq!(rejected.line(), partial.line());
    assert!(matches!(rejected, CoverageOutcome::PartialRejected { .. }));
    assert!(matches!(accepted, CoverageOutcome::PartialAccepted { .. }));
    if let CoverageOutcome::PartialRejected { message, .. } = &rejected {
        assert!(message.contains("--allow-partial"), "{message}");
        assert!(message.contains("scored=2040"), "{message}");
    }
    if let CoverageOutcome::PartialAccepted { warning, .. } = &accepted {
        assert!(warning.contains("--allow-partial is present"), "{warning}");
        assert!(
            !warning.contains("rerun with --allow-partial"),
            "{warning} must not offer a flag the operator already gave"
        );
    }
    let full = Coverage::new(2401, 2401, 0, ScoredSource::Vanilla);
    assert!(matches!(
        full.outcome(false),
        CoverageOutcome::Complete { .. }
    ));
    assert!(matches!(
        full.outcome(true),
        CoverageOutcome::Complete { .. }
    ));
}

#[test]
fn allow_partial_never_rescues_a_run_that_scored_nothing() {
    let empty = Coverage::new(0, 1, 1, ScoredSource::Vanilla);
    for allow_partial in [false, true] {
        let CoverageOutcome::PartialRejected { line, message } = empty.outcome(allow_partial)
        else {
            panic!("an empty measurement must fail even with --allow-partial");
        };
        assert_eq!(line, empty.line());
        assert!(message.contains("nothing was scored"), "{message}");
        assert!(
            !message.contains("rerun with --allow-partial"),
            "{message} must not offer a flag that will not help"
        );
    }
    assert!(matches!(
        Coverage::new(2040, 2401, 361, ScoredSource::Vanilla).outcome(true),
        CoverageOutcome::PartialAccepted { .. }
    ));
}
