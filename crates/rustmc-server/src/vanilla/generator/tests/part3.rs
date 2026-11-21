//! Unit tests, part 3 of 3.

use super::*;

/// Halo and eviction cannot move an answer. A carved, decorated pack is
/// queried in three orders — plain gate lookups over a block of chunks
/// including negative coordinates, then whole-chunk fills that overwrite
/// the lazily stored halo rows with fill-derived rows, then a sweep past
/// the map cache's bound so every original map rotates out and back. In
/// every phase the answer per column equals the reference descent.
#[test]
fn heightmap_halo_lookups_and_eviction_match_the_descent() {
    let root = scratch_root("heightmap-halo");
    bounded_pack(&root);
    // Same pack, but the carver actually carves: rows it stamps sit
    // inside the solid band, so descents cross post-carve substance.
    write(
        &root.join("data/testns/worldgen/carver/scarce.json"),
        r#"{"type": "minecraft:cave", "probability": 1.0,
            "y": {"type": "minecraft:uniform",
                "min_inclusive": {"absolute": 6},
                "max_inclusive": {"absolute": 22}},
            "count": 4, "thickness": 1.5,
            "room_vertical_radius_multiplier": 1.0,
            "horizontal_radius_multiplier": 1.0,
            "vertical_radius_multiplier": 1.0,
            "floor_level": -0.7}"#,
    );
    let generator =
        VanillaGenerator::new(&root, 2026, "testns:bounded").expect("carving pack generator");
    // A three-chunk block straddling the origin's negatives.
    let area: Vec<(i32, i32)> = (-3..=3)
        .flat_map(|chunk_x| (-2..=2).map(move |chunk_z| (chunk_x, chunk_z)))
        .collect();
    let mut carved_rows = 0usize;
    let mut reference = Vec::new();
    for &(chunk_x, chunk_z) in &area {
        for local_x in 0..16 {
            for local_z in 0..16 {
                let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                let descent = generator.ocean_floor_by_descent(x, z);
                for y in generator.min_y()..generator.min_y() + generator.height() {
                    if generator.carved(x, y, z) {
                        carved_rows += 1;
                    }
                }
                reference.push((x, z, descent));
            }
        }
    }
    assert!(carved_rows > 0, "the fixture must actually carve rows");
    // Phase 1: gate-style lookups (lazy per-column recording).
    for &(x, z, descent) in &reference {
        assert_eq!(generator.ocean_floor_height(x, z), descent, "({x}, {z})");
    }
    // Phase 2: fill the same chunks; the fill-derived rows must agree
    // with the descent exactly where the lazy rows sit.
    for &(chunk_x, chunk_z) in &area {
        for local_x in 0..16 {
            for local_z in 0..16 {
                let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                let _ = generator.column_ids(x, z);
            }
        }
    }
    for &(x, z, descent) in &reference {
        assert_eq!(
            generator.ocean_floor_height(x, z),
            descent,
            "({x}, {z}) after the fill overwrote the map"
        );
    }
    // Phase 3: rotate every one of these maps out of the cache with
    // far-apart chunk keys, then answer again from rebuilt maps.
    for chunk in 0..(4 * VanillaGenerator::OCEAN_FLOOR_MAP_CACHE_CAPACITY) as i32 {
        let _ = generator.ocean_floor_height(chunk * 512 + 7, -4 * chunk + 3);
    }
    let occupancy = generator.cache_occupancy();
    assert!(
        occupancy.ocean_floor_maps <= 2 * VanillaGenerator::OCEAN_FLOOR_MAP_CACHE_CAPACITY,
        "heightmap cache grew past its bound: {occupancy:?}"
    );
    assert!(
        occupancy.ocean_floor_maps
            < area.len() + 4 * VanillaGenerator::OCEAN_FLOOR_MAP_CACHE_CAPACITY,
        "the sweep touched one map per chunk of the area plus four per capacity, so a \
         growing map would pin all of them: {occupancy:?}"
    );
    for &(x, z, descent) in &reference {
        assert_eq!(
            generator.ocean_floor_height(x, z),
            descent,
            "({x}, {z}) after eviction"
        );
    }
    fs::remove_dir_all(root).expect("cleanup");
}

/// The chunk-and-slot split of an absolute column is the Euclidean one
/// the decorated grid already uses, at negative coordinates too.
#[test]
fn heightmap_slots_split_absolute_columns_euclideanly() {
    for x in [
        -4_194_321i32,
        -70_824,
        -300,
        -17,
        -16,
        -1,
        0,
        1,
        15,
        16,
        303,
    ] {
        for z in [-70_824i32, -33, -16, -1, 0, 7, 16, 511] {
            let slot = OceanFloorMap::slot(x, z);
            assert!(slot < 256, "({x}, {z}) → {slot}");
            let (chunk_x, chunk_z) = (x >> 4, z >> 4);
            // Euclidean round trip: chunk base plus local offset is back.
            assert_eq!(
                (
                    chunk_x * 16 + (slot as i32 / 16),
                    chunk_z * 16 + (slot as i32 % 16)
                ),
                (x, z),
                "({x}, {z}) → chunk ({chunk_x}, {chunk_z}) slot {slot}"
            );
        }
    }
}

#[test]
fn chunk_density_cache_stays_bounded_and_matches_raw_density() {
    let (root, generator) = bounded_generator("cache-density");
    for chunk_x in 0..40i32 {
        let chunk = generator.chunk_density(chunk_x, 0);
        assert_eq!(chunk.len(), 16 * generator.height() as usize * 16);
    }
    let occupancy = generator.cache_occupancy();
    assert!(occupancy.chunk_densities <= 2 * VanillaGenerator::CHUNK_DENSITY_CACHE_CAPACITY);
    assert!(occupancy.chunk_densities < 40);
    for (x, y, z) in [
        (-1, 0, -1),
        (0, 22, 15),
        (17, 127, -3),
        (5, -10, 5),
        (5, 200, 5),
    ] {
        assert_eq!(
            generator.density_at(x, y, z).to_bits(),
            generator.raw_density(x, y, z).to_bits(),
            "mismatch at ({x},{y},{z})"
        );
    }
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn shared_caches_match_private_generator_across_threads() {
    let (root, private) = decorating_generator("shared-caches");
    let shared = Arc::new(SharedChunkCaches::new());
    let coords: Vec<(i32, i32)> = (-1..=1)
        .flat_map(|cx| (-1..=1).map(move |cz| (cx, cz)))
        .collect();
    let expected: Vec<_> = coords
        .iter()
        .map(|&(cx, cz)| chunk_columns(&private, cx, cz))
        .collect();
    let queue = std::sync::Mutex::new(coords.iter().copied().enumerate());
    let results = std::sync::Mutex::new(vec![Vec::new(); coords.len()]);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let (root, shared, queue, results) = (&root, &shared, &queue, &results);
            scope.spawn(move || {
                let worker = VanillaGenerator::with_shared_caches(
                    root,
                    2026,
                    "testns:bounded",
                    Arc::clone(shared),
                )
                .expect("shared worker");
                loop {
                    let next = queue.lock().expect("queue").next();
                    let Some((idx, (cx, cz))) = next else {
                        break;
                    };
                    let cols = chunk_columns(&worker, cx, cz);
                    results.lock().expect("results")[idx] = cols;
                }
            });
        }
    });
    let got = results.into_inner().expect("results");
    assert_eq!(got, expected);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn chunk_biomes_at_local_preserves_xz_axes() {
    let (root, generator) = region_generator("chunk-biomes-axes");
    // In `region_generator`, temperature varies only along x (`0..64`),
    // splitting at x = 32 (chunk 2). Chunk (1, 0) vs (2, 0): probe chunk
    // straddling x = 32 by testing a x-varying pack inside one chunk (0, 0).
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("density_function/slope.json"),
        r#"{"type": "gradient", "axis": "x", "from_coordinate": 0,
            "to_coordinate": 16, "from_value": -1.0, "to_value": 1.0}"#,
    );
    let split = VanillaGenerator::new(&root, 2026, "testns:split").expect("split generator");
    let placement = split.placement.as_ref().expect("placement");
    let _ = split.chunk_biomes(0, 0);
    // After populating `chunk_biomes(0, 0)`, every asymmetric (x, z) quart
    // cell in chunk (0, 0) must still match the uncached point sampler.
    let mut saw_low = false;
    let mut saw_peak = false;
    for local_qz in 0..4 {
        for local_qx in 0..4 {
            let x = local_qx * 4;
            let z = local_qz * 4;
            let expected = split.climate.biome(placement, x, 8, z);
            let served = split.biome(x, z, 8);
            assert_eq!(served, expected, "mismatch at ({x}, 8, {z})");
            if served.as_deref() == Some("testns:low") {
                saw_low = true;
            } else if served.as_deref() == Some("testns:peak") {
                saw_peak = true;
            }
        }
    }
    assert!(saw_low && saw_peak, "chunk must contain both x halves");
    drop(generator);
    fs::remove_dir_all(root).expect("cleanup");
}

/// Requires the operator-provisioned 26.3 worldgen data. Lists the biomes
/// present in the measured `0..=31` square on seed 2026, prints the
/// `vegetal_decoration` step table (each placed feature, whether RustMC
/// runs it, and its first and remaining unsupported pieces), and pins the
/// set of unsupported pieces.
#[test]
#[ignore = "requires operator-provisioned local data"]
fn pack_measured_square_vegetation_diagnosis_pins_unsupported_pieces() {
    use crate::vanilla::feature::VEGETAL_DECORATION_STEP;

    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let generator =
        VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");

    let square_biomes = generator.biomes_in_square(0, 31, 0, 31);
    assert_eq!(
        square_biomes,
        vec![
            "minecraft:deep_dark".to_string(),
            "minecraft:dripstone_caves".to_string(),
            "minecraft:forest".to_string(),
        ]
    );

    let features = generator.features().expect("overworld feature registry");
    let rows = features.diagnose_step_for_biomes(VEGETAL_DECORATION_STEP, &square_biomes);
    assert_eq!(rows.len(), 15);

    println!("square 0..=31 biomes: {square_biomes:?}");
    println!(
        "{:<44} | {:<5} | {:<40} | all unsupported pieces",
        "placed_feature", "runs", "first_unsupported"
    );
    let mut first_pieces = BTreeSet::new();
    let mut all_pieces = BTreeSet::new();
    for row in &rows {
        let first = row.first_unsupported.as_deref().unwrap_or("-");
        if let Some(piece) = &row.first_unsupported {
            first_pieces.insert(piece.clone());
        }
        for piece in &row.unsupported_pieces {
            all_pieces.insert(piece.clone());
        }
        println!(
            "{:<44} | {:<5} | {:<40} | {}",
            row.id,
            row.modelable,
            first,
            row.unsupported_pieces.join(", ")
        );
    }

    let expected_table: [(&str, bool, Option<&str>); 15] = [
        (
            "minecraft:glow_lichen",
            false,
            Some("minecraft:multiface_growth"),
        ),
        ("minecraft:forest_flowers", true, None),
        (
            "minecraft:trees_birch_and_oak_leaf_litter",
            false,
            Some("placement:minecraft:surface_water_depth_filter"),
        ),
        (
            "minecraft:patch_tall_grass_2",
            false,
            Some("placement:minecraft:noise_threshold_count"),
        ),
        ("minecraft:patch_bush", true, None),
        (
            "minecraft:trees_plains",
            false,
            Some("placement:minecraft:surface_water_depth_filter"),
        ),
        (
            "minecraft:flower_plains",
            false,
            Some("placement:minecraft:noise_threshold_count"),
        ),
        (
            "minecraft:patch_grass_plain",
            false,
            Some("placement:minecraft:noise_threshold_count"),
        ),
        ("minecraft:flower_default", true, None),
        ("minecraft:patch_grass_forest", true, None),
        (
            "minecraft:brown_mushroom_normal",
            false,
            Some("state:minecraft:brown_mushroom"),
        ),
        (
            "minecraft:red_mushroom_normal",
            false,
            Some("state:minecraft:red_mushroom"),
        ),
        ("minecraft:patch_pumpkin", true, None),
        ("minecraft:patch_sugar_cane", true, None),
        ("minecraft:patch_firefly_bush_near_water", true, None),
    ];

    for (row, (expected_id, expected_runs, expected_first)) in
        rows.iter().zip(expected_table.iter())
    {
        assert_eq!(row.id, *expected_id);
        assert_eq!(row.modelable, *expected_runs, "modelable for {}", row.id);
        assert_eq!(
            row.first_unsupported.as_deref(),
            *expected_first,
            "first unsupported piece for {}",
            row.id
        );
    }

    assert_eq!(
        first_pieces,
        BTreeSet::from([
            "minecraft:multiface_growth".to_string(),
            "placement:minecraft:noise_threshold_count".to_string(),
            "placement:minecraft:surface_water_depth_filter".to_string(),
            "state:minecraft:brown_mushroom".to_string(),
            "state:minecraft:red_mushroom".to_string(),
        ])
    );
    assert_eq!(
        all_pieces,
        BTreeSet::from([
            "minecraft:fallen_tree".to_string(),
            "minecraft:multiface_growth".to_string(),
            "minecraft:tree".to_string(),
            "placement:minecraft:noise_threshold_count".to_string(),
            "placement:minecraft:surface_water_depth_filter".to_string(),
            "predicate:minecraft:would_survive".to_string(),
            "provider:minecraft:noise_threshold".to_string(),
            "state:minecraft:brown_mushroom".to_string(),
            "state:minecraft:red_mushroom".to_string(),
        ])
    );
}

#[test]
fn chunk_view_heightmap_and_halo_solid_match_base_columns() {
    use crate::vanilla::feature::{BlockTags, HeightmapKind};
    let (root, generator) = decorating_generator("heightmap-halo");
    let tags = BlockTags::default();
    let mut chunk = DecoratedChunk::new(generator.router.height as usize);
    for lx in 0..16 {
        for lz in 0..16 {
            let ids = generator.base_column_ids(lx, lz);
            let mut top = -1i16;
            for (row, name) in ids.into_iter().enumerate() {
                let at = chunk.index(lx, row, lz);
                let slot = chunk.intern(name);
                chunk.blocks[at] = slot;
                if chunk.palette_non_air[slot as usize] {
                    top = row as i16;
                }
            }
            chunk.top_non_air[(lx as usize) * 16 + lz as usize] = top;
        }
    }
    let top_row = generator.height() as usize - 1;
    let top_y = generator.min_y() + top_row as i32;
    chunk.set(2, top_row - 1, 2, "minecraft:stone");
    chunk.set(2, top_row, 2, "minecraft:oak_leaves");
    chunk.set(3, top_row - 1, 3, "minecraft:stone");
    chunk.set(3, top_row, 3, "minecraft:short_grass");
    let halo = RefCell::new(HaloColumns::new());
    let view = ChunkView {
        generator: &generator,
        chunk_x: 0,
        chunk_z: 0,
        chunk: &mut chunk,
        halo: &halo,
    };
    assert_eq!(
        view.heightmap_height(HeightmapKind::WorldSurface, 2, 2, &tags),
        top_y + 1
    );
    assert_eq!(
        view.heightmap_height(HeightmapKind::MotionBlocking, 2, 2, &tags),
        top_y + 1
    );
    assert_eq!(
        view.heightmap_height(HeightmapKind::MotionBlockingNoLeaves, 2, 2, &tags),
        top_y
    );
    assert_eq!(
        view.heightmap_height(HeightmapKind::WorldSurface, 3, 3, &tags),
        top_y + 1
    );
    assert_eq!(
        view.heightmap_height(HeightmapKind::MotionBlocking, 3, 3, &tags),
        top_y,
        "short_grass does not block motion"
    );
    let expected_halo = generator.base_column_ids(17, 5);
    for (row, expected_name) in expected_halo.iter().enumerate() {
        let y = generator.min_y() + row as i32;
        let got = view.read_block(17, y, 5);
        let want = expected_name.as_deref().unwrap_or("minecraft:air");
        assert_eq!(got.as_deref(), Some(want), "halo mismatch at y={y}");
    }
    assert!(
        view.heightmap_height(HeightmapKind::OceanFloorWg, 17, 5, &tags)
            <= view.heightmap_height(HeightmapKind::WorldSurfaceWg, 17, 5, &tags)
    );
    fs::remove_dir_all(root).expect("cleanup");
}
