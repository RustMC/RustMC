//! Unit tests, part 2 of 2.

use super::*;

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
    let encoded_chunks = [(0_i32, 0_i32), (-1, 2)];
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
            // Veins are local: a stone column two chunks over can hold an ore
            // this chunk never emits. The invented vocabulary therefore has to
            // be collected from exactly the columns that are encoded below,
            // not from a sample elsewhere.
            for (chunk_x, chunk_z) in encoded_chunks {
                for local_z in 0..16 {
                    for local_x in 0..16 {
                        let x = chunk_x * 16 + local_x;
                        let z = chunk_z * 16 + local_z;
                        states.extend(generator.column_ids(x, z).into_iter().flatten());
                        for layer in 0..BIOME_LAYERS {
                            let y = OVERWORLD_MIN_Y + layer as i32 * SECTION_BIOME_SIDE as i32;
                            if let Some(biome) = generator.biome(x, z, y) {
                                biomes.insert(biome);
                            }
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
    for (chunk_x, chunk_z) in encoded_chunks {
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

#[test]
#[ignore = "requires operator-provisioned local data"]
fn shared_worker_caches_encode_identically_to_private_generator() {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use crate::vanilla::generator::SharedChunkCaches;

    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let registry_path = std::env::var("RUSTMC_CHUNK_REGISTRY")
        .or_else(|_| std::env::var("RUSTMC_REGISTRY_TABLE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/chunk-registry-26.3.json"));
    let text = std::fs::read_to_string(&registry_path).expect("provisioned chunk registry reads");
    let tables = RegistryTables::from_provisioned(&text).expect("provisioned table loads");

    let coords: Vec<(i32, i32)> = (-2..=2)
        .flat_map(|cx| (-2..=2).map(move |cz| (cx, cz)))
        .collect();
    let private =
        VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("private generator loads");
    let expected: Vec<Vec<u8>> = coords
        .iter()
        .map(|&(cx, cz)| {
            let chunk =
                chunk_from_generator(&private, cx, cz, &tables).expect("private chunk builds");
            encode_chunk(&chunk, &tables).expect("private chunk encodes")
        })
        .collect();

    let shared = Arc::new(SharedChunkCaches::new());
    let queue = Mutex::new(coords.iter().copied().enumerate());
    let actual = Mutex::new(vec![Vec::new(); coords.len()]);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let (root, text, shared, queue, actual) = (&root, &text, &shared, &queue, &actual);
            scope.spawn(move || {
                let worker = VanillaGenerator::with_shared_caches(
                    root,
                    2026,
                    "minecraft:overworld",
                    Arc::clone(shared),
                )
                .expect("shared generator loads");
                let worker_tables =
                    RegistryTables::from_provisioned(text).expect("worker tables load");
                loop {
                    let next = queue.lock().expect("queue").next();
                    let Some((idx, (cx, cz))) = next else {
                        break;
                    };
                    let chunk = chunk_from_generator(&worker, cx, cz, &worker_tables)
                        .expect("shared chunk builds");
                    let packet =
                        encode_chunk(&chunk, &worker_tables).expect("shared chunk encodes");
                    actual.lock().expect("actual")[idx] = packet;
                }
            });
        }
    });
    let actual = actual.into_inner().expect("actual");
    for (idx, &(cx, cz)) in coords.iter().enumerate() {
        assert_eq!(
            actual[idx], expected[idx],
            "packet mismatch for chunk ({cx}, {cz})"
        );
    }
}
