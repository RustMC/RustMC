//! Unit tests, part 2 of 3.

use super::*;

/// Requires the operator-provisioned 26.3 worldgen data. A feature's
/// ordinal is part of its seed, and the ordinal graph has a bounded cycle
/// recovery (`MAX_CYCLE_ATTEMPTS`) where the reference runtime recurses
/// without a bound. That bound is only safe while the pack never needs it,
/// so this checks the schedule really is the graph over the biome source's
/// own list and that neither cycle counter was reported, and prints the
/// shape it found.
#[test]
#[ignore = "requires operator-provisioned local data"]
fn smoke_operator_feature_schedule_needs_no_cycle_recovery() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let generator =
        VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
    let data = generator
        .features
        .as_ref()
        .expect("the provisioned pack decorates");
    assert!(
        !data.biome_order().is_empty(),
        "the schedule fell back to the pack's identifier order"
    );
    for counter in ["feature_order_cycle", "feature_order_cycle_unresolved"] {
        assert!(
            !data.unimplemented_features.contains_key(counter),
            "the provisioned pack needed the bounded cycle recovery ({counter})"
        );
    }
    let steps = data.decorated_steps();
    let slots: usize = steps
        .iter()
        .map(|step| data.step(*step).map_or(0, |list| list.len()))
        .sum();
    println!(
        "smoke OK: schedule over {} biomes, {} steps carrying {} placed features, root {}",
        data.biome_order().len(),
        steps.len(),
        slots,
        root.display()
    );
}

/// A fabricated pack that drives every coordinate-keyed cache of the
/// generator and the aquifer without touching the operator's data: a
/// y-only density band (solid on y = 5..=23, identical in all
/// columns), the shared descent rule program, an all-constant aquifer
/// section, a one-entry placement table selecting a biome whose only
/// carver has probability 0. That carver never stamps a mask, yet
/// `build_carve_mask` still resolves the 17x17 window of source chunk
/// carver lists, so the replay path and both carving caches run.
pub(super) fn bounded_pack(root: &Path) {
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("density_function/band.json"),
        r#"{"type": "mul",
            "left": {"type": "gradient", "axis": "y",
                "from_coordinate": 0, "from_value": -1.0,
                "to_coordinate": 8, "to_value": 1.0},
            "right": {"type": "gradient", "axis": "y",
                "from_coordinate": 0, "from_value": 1.0,
                "to_coordinate": 48, "to_value": -1.0}}"#,
    );
    write(&worldgen.join("material_rule/root.json"), DESCENT_ROOT);
    write(
        &worldgen.join("carver/scarce.json"),
        r#"{"type": "minecraft:cave", "probability": 0.0,
            "y": {"type": "minecraft:uniform",
                "min_inclusive": {"absolute": -20},
                "max_inclusive": {"absolute": 60}},
            "count": 1, "thickness": 1.5,
            "room_vertical_radius_multiplier": 1.0,
            "horizontal_radius_multiplier": 1.0,
            "vertical_radius_multiplier": 1.0,
            "floor_level": -0.7}"#,
    );
    write(
        &worldgen.join("biome/flat.json"),
        r#"{"carvers": ["testns:scarce"]}"#,
    );
    descent_pack_noise(root);
    write(
        &worldgen.join("noise_settings/bounded.json"),
        r#"{
            "noise": {"min_y": 0, "height": 128},
            "sea_level": -1000,
            "default_fluid": "minecraft:water",
            "default_block": "minecraft:granite",
            "noise_router": {
                "final_density": "testns:band",
                "continents": 0.0,
                "erosion": 0.0,
                "depth": 0.0,
                "ridges": 0.0,
                "temperature": 0.0,
                "vegetation": 0.0
            },
            "aquifers": {
                "barrier": 0.0,
                "fluid_level_floodedness": 0.0,
                "fluid_level_spread": 0.0,
                "lava": 0.0,
                "exclusion": 0.0,
                "surface_level": 0.0
            },
            "material_rule": "testns:root"
        }"#,
    );
    // The preset of `testns:bounded` is `bounded`, so the placement
    // table beside the data root must be named accordingly. A single
    // entry covering the whole climate volume keeps every chunk on
    // the `testns:flat` carver list.
    write(
        &root.join("rustmc/biome_placement/bounded.psv"),
        concat!(
            "0|testns:flat|t=[-2000-2000]|h=[-10000-10000]|c=[-10000-10000]|",
            "e=[-10000-10000]|d=[-10000-10000]|w=[-10000-10000]|off=0\n"
        ),
    );
}

pub(super) fn bounded_generator(label: &str) -> (PathBuf, VanillaGenerator) {
    let root = scratch_root(label);
    bounded_pack(&root);
    let generator =
        VanillaGenerator::new(&root, 2026, "testns:bounded").expect("bounded pack generator");
    (root, generator)
}

#[test]
fn quart_biome_cache_is_bounded_and_matches_uncached_sampler() {
    let (root, generator) = bounded_generator("cache-biomes");
    let placement = generator.placement.as_ref().expect("fixture placement");
    for quart in 0..9_000i32 {
        let x = quart * 4;
        let expected = generator.climate.biome(placement, x, 8, -4);
        assert_eq!(generator.biome(x, -4, 8), expected);
        assert_eq!(generator.biome(x + 3, -1, 11), expected);
    }
    let occupancy = generator.cache_occupancy();
    assert!(occupancy.biomes <= 2 * VanillaGenerator::BIOME_CACHE_CAPACITY);
    assert!(occupancy.biomes < 9_000);
    fs::remove_dir_all(root).expect("cleanup");
}

/// The carving caches must stop growing long before the world volume
/// queried: 300 chunk targets along the x = z diagonal touch 300
/// distinct mask keys and (each replaying its 17x17 window) more than
/// 10,000 distinct source chunk keys, while the capacities allow 64
/// and 4,096 entries at most.
#[test]
fn carve_caches_stay_bounded_across_target_chunks() {
    let (root, generator) = bounded_generator("cache-carve");
    let mut distinct_sources = std::collections::HashSet::new();
    for target in 0..300i32 {
        // One column per target chunk: the descent's carve pass is
        // what builds and reads the mask.
        let _ = generator.column_ids(target * 16 + 8, target * 16 + 8);
        for dx in -8..=8 {
            for dz in -8..=8 {
                distinct_sources.insert((target + dx, target + dz));
            }
        }
    }
    let occupancy = generator.cache_occupancy();
    assert!(
        distinct_sources.len() > 10_000,
        "the fixture must probe a source-window union far past the cache bound, \
         got {}",
        distinct_sources.len()
    );
    assert!(
        occupancy.masks <= 2 * VanillaGenerator::MASK_CACHE_CAPACITY,
        "mask cache grew past its bound: {occupancy:?}"
    );
    assert!(
        occupancy.masks < 300,
        "every one of the 300 target masks is still pinned: {occupancy:?}"
    );
    assert!(
        occupancy.chunk_carvers <= 2 * VanillaGenerator::CHUNK_CARVERS_CACHE_CAPACITY,
        "carver cache grew past its bound: {occupancy:?}"
    );
    assert!(
        occupancy.chunk_carvers < distinct_sources.len(),
        "every source chunk of the sweep is still pinned: {occupancy:?}"
    );
    assert!(
        occupancy.heights <= 2 * VanillaGenerator::HEIGHT_CACHE_CAPACITY,
        "height cache grew past its bound: {occupancy:?}"
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

/// The column-top memo is the largest of the seven caches by entry
/// count: a sweep of 36 whole chunks queries 9,216 distinct columns,
/// and the memo must hold at most 8,192 of them.
#[test]
fn height_cache_stays_bounded_across_column_sweeps() {
    let (root, generator) = bounded_generator("cache-heights");
    // 96x96 blocks is 36 chunks, i.e. 9,216 distinct columns; the
    // neighbour reads of the surface program land on columns inside
    // the same sweep.
    let mut columns = 0usize;
    for x in 0..96i32 {
        for z in 0..96i32 {
            let _ = generator.surface_height(x, z);
            columns += 1;
        }
    }
    assert_eq!(columns, 9_216);
    let occupancy = generator.cache_occupancy();
    assert!(
        occupancy.heights <= 2 * VanillaGenerator::HEIGHT_CACHE_CAPACITY,
        "height cache grew past its bound after {columns} columns: {occupancy:?}"
    );
    assert!(
        occupancy.heights < columns,
        "the memo still holds one entry per column queried: {occupancy:?}"
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

/// The aquifer's four grid caches are keyed by fluid cell, quart
/// point, and chunk, so they grow with the area queried even when the
/// columns themselves are not cached. Probing one low block per chunk
/// of a 60x60 chunk sweep touches 3,600 distinct chunk keys (bound:
/// 512) and, because each sampling consults the 2x3x2 cell
/// neighbourhood of its anchor, the full 61x3x61 = 11,163 cell grid
/// (bound: 2,048), plus tens of thousands of quart surface points
/// (bound: 8,192).
#[test]
fn aquifer_caches_stay_bounded_across_chunk_sweeps() {
    let (root, generator) = bounded_generator("cache-aquifer");
    for chunk_x in 0..60i32 {
        for chunk_z in 0..60i32 {
            // y = 4 is density-empty below the aquifer sampling bound
            // of this pack, which is the branch that consults cells.
            let _ = generator.substance(chunk_x * 16 + 3, 4, chunk_z * 16 + 7);
        }
    }
    let occupancy = generator.cache_occupancy();
    assert!(
        occupancy.aquifer_skip_bounds <= 2 * NoiseBasedAquifer::SKIP_CACHE_CAPACITY,
        "chunk sampling bounds grew past their bound: {occupancy:?}"
    );
    assert!(
        occupancy.aquifer_skip_bounds < 3_600,
        "one key per chunk and the sweep spans 3,600 chunks, so a growing map would pin \
         all of them: {occupancy:?}"
    );
    assert!(
        occupancy.aquifer_centers <= 2 * NoiseBasedAquifer::CENTER_CACHE_CAPACITY,
        "fluid cell centers grew past their bound: {occupancy:?}"
    );
    assert!(
        occupancy.aquifer_centers < 11_163,
        "the sweep consults 61x3x61 fluid cells, so a growing map would pin all of them: \
         {occupancy:?}"
    );
    assert!(
        occupancy.aquifer_statuses <= 2 * NoiseBasedAquifer::STATUS_CACHE_CAPACITY,
        "fluid cell statuses grew past their bound: {occupancy:?}"
    );
    assert!(
        occupancy.aquifer_surface_levels <= 2 * NoiseBasedAquifer::SURFACE_CACHE_CAPACITY,
        "preliminary surface levels grew past their bound: {occupancy:?}"
    );
    assert!(
        occupancy.aquifer_centers > 0 && occupancy.aquifer_surface_levels > 0,
        "the sweep did not populate the aquifer caches, so the bounds above are vacuous: \
         {occupancy:?}"
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

/// Eviction is a recomputation, not a change of result: after the
/// caches have rotated through a wide sweep, the same column must
/// still report the same top, the same rule descent, and the same
/// carve state, which is what makes the bounds safe to apply to
/// coordinate-keyed memos at all.
#[test]
fn evicted_entries_recompute_identically() {
    let (root, generator) = bounded_generator("cache-invariance");
    let (x, z) = (6i32, -2i32);
    let before_height = generator.surface_height(x, z);
    let before_floor = generator.ocean_floor_height(x, z);
    let before_ids = generator.column_ids(x, z);
    let before_top = generator.top_block(x, z);
    let before_biome = generator.biome(x, z, 8);
    let before_carved = generator.carved(x, 8, z);
    assert!(
        before_height > 0 && before_ids.iter().any(Option::is_some) && before_top.is_some(),
        "the fixture column must actually have terrain and rule results to compare"
    );

    // Enough distinct keys to rotate every generation of every cache,
    // none of which re-reads the column captured above: 300 new mask
    // targets (capacity 32), 9,216 new columns (capacity 4,096), and
    // some 19,000 new fluid cells (capacity 1,024).
    for target in 0..300i32 {
        let _ = generator.column_ids(target * 16 + 8, target * 16 + 8);
    }
    for x in 0..96i32 {
        for z in 0..96i32 {
            let _ = generator.surface_height(x, z);
        }
    }
    for chunk_x in 0..40i32 {
        for chunk_z in 0..40i32 {
            let _ = generator.substance(chunk_x * 16 + 3, 4, chunk_z * 16 + 7);
        }
    }
    let occupancy = generator.cache_occupancy();
    assert!(
        occupancy.masks <= 2 * VanillaGenerator::MASK_CACHE_CAPACITY
            && occupancy.heights <= 2 * VanillaGenerator::HEIGHT_CACHE_CAPACITY
            && occupancy.ocean_floor_maps <= 2 * VanillaGenerator::OCEAN_FLOOR_MAP_CACHE_CAPACITY
            && occupancy.biomes <= 2 * VanillaGenerator::BIOME_CACHE_CAPACITY
            && occupancy.aquifer_centers <= 2 * NoiseBasedAquifer::CENTER_CACHE_CAPACITY,
        "the sweeps above must have rotated the caches while keeping them bounded: \
         {occupancy:?}"
    );

    assert_eq!(generator.surface_height(x, z), before_height);
    assert_eq!(generator.ocean_floor_height(x, z), before_floor);
    assert_eq!(generator.column_ids(x, z), before_ids);
    assert_eq!(generator.top_block(x, z), before_top);
    assert_eq!(generator.biome(x, z, 8), before_biome);
    assert_eq!(generator.carved(x, 8, z), before_carved);
    fs::remove_dir_all(&root).expect("cleanup");
}

/// The bounded pack plus a placement stage: one configured ore whose
/// target is a block tag, placed with count, square, band and biome
/// decorations, and listed by the fixture biome at the underground step.
/// The pack's solid band is y = 5..=23 and its descent ends in an
/// unconditional stone fallthrough, so granite anywhere in a column can
/// only come from a vein.
pub(super) fn decorating_pack(root: &Path) {
    bounded_pack(root);
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("feature/vein.json"),
        r#"{"type": "minecraft:ore", "size": 32, "discard_chance_on_air_exposure": 0.0,
            "targets": [{"state": "minecraft:granite",
                "target": {"predicate_type": "minecraft:tag_match", "tag": "testns:filler"}}]}"#,
    );
    write(
        &root.join("data/testns/tags/block/filler.json"),
        r#"{"values": ["minecraft:stone", {"tag": "testns:deep_filler"}]}"#,
    );
    write(
        &root.join("data/testns/tags/block/deep_filler.json"),
        r#"{"values": ["minecraft:deepslate"]}"#,
    );
    write(
        &worldgen.join("placed_feature/vein.json"),
        r#"{"feature": "testns:vein", "placement": [
            {"type": "minecraft:count", "count": 3},
            {"type": "minecraft:in_square"},
            {"type": "minecraft:height_range", "height": {"type": "minecraft:uniform",
                "min_inclusive": {"absolute": 5}, "max_inclusive": {"absolute": 23}}},
            {"type": "minecraft:biome"}
        ]}"#,
    );
    // The eleventh-step slot the overworld pack uses for ore veins.
    write(
        &worldgen.join("biome/flat.json"),
        r#"{"carvers": ["testns:scarce"],
            "features": [[], [], [], [], [], [], [], [], ["testns:vein"], [], []]}"#,
    );
}

/// Every column of one chunk, in a fixed read order.
pub(super) fn chunk_columns(
    generator: &VanillaGenerator,
    chunk_x: i32,
    chunk_z: i32,
) -> Vec<Vec<Option<String>>> {
    let mut columns = Vec::with_capacity(256);
    for local_x in 0..16 {
        for local_z in 0..16 {
            columns.push(generator.column_ids(chunk_x * 16 + local_x, chunk_z * 16 + local_z));
        }
    }
    columns
}

pub(super) fn decorating_generator(label: &str) -> (PathBuf, VanillaGenerator) {
    let root = scratch_root(label);
    decorating_pack(&root);
    let generator =
        VanillaGenerator::new(&root, 2026, "testns:bounded").expect("decorating pack generator");
    (root, generator)
}

/// The decorating pack plus a second dimension whose placement table
/// splits the world along x: `testns:low` west of x = 32, `testns:peak`
/// east of it. Only `peak` lists a placed vein, and that vein carries no
/// `minecraft:biome` filter, so the only thing that can keep it out of a
/// chunk is the biome union the pass runs its ordinals over. The
/// `testns:flat` biome of the decorating dimension stays out of this
/// dimension's possible set, so its own granite vein must not run here.
pub(super) fn region_pack(root: &Path) {
    decorating_pack(root);
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("density_function/slope.json"),
        r#"{"type": "gradient", "axis": "x", "from_coordinate": 0,
            "to_coordinate": 64, "from_value": -1.0, "to_value": 1.0}"#,
    );
    write(
        &worldgen.join("feature/raw.json"),
        r#"{"type": "minecraft:ore", "size": 32, "discard_chance_on_air_exposure": 0.0,
            "targets": [{"state": "minecraft:diorite",
                "target": {"predicate_type": "minecraft:block_match", "block": "minecraft:stone"}}]}"#,
    );
    write(
        &worldgen.join("placed_feature/raw.json"),
        r#"{"feature": "testns:raw", "placement": [
            {"type": "minecraft:count", "count": 3},
            {"type": "minecraft:in_square"},
            {"type": "minecraft:height_range", "height": {"type": "minecraft:uniform",
                "min_inclusive": {"absolute": 5}, "max_inclusive": {"absolute": 23}}}
        ]}"#,
    );
    write(
        &worldgen.join("biome/low.json"),
        r#"{"carvers": [], "features": [[], [], [], [], [], [], [], [], [], [], []]}"#,
    );
    write(
        &worldgen.join("biome/peak.json"),
        r#"{"carvers": [], "features": [[], [], [], [], [], [], [], [], ["testns:raw"], [], []]}"#,
    );
    write(
        &worldgen.join("noise_settings/split.json"),
        r#"{
            "noise": {"min_y": 0, "height": 128},
            "sea_level": -1000,
            "default_fluid": "minecraft:water",
            "default_block": "minecraft:granite",
            "noise_router": {
                "final_density": "testns:band",
                "continents": 0.0,
                "erosion": 0.0,
                "depth": 0.0,
                "ridges": 0.0,
                "temperature": "testns:slope",
                "vegetation": 0.0
            },
            "aquifers": {
                "barrier": 0.0,
                "fluid_level_floodedness": 0.0,
                "fluid_level_spread": 0.0,
                "lava": 0.0,
                "exclusion": 0.0,
                "surface_level": 0.0
            },
            "material_rule": "testns:root"
        }"#,
    );
    write(
        &root.join("rustmc/biome_placement/split.psv"),
        concat!(
            "0|testns:low|t=[-10000-0]|h=[-10000-10000]|c=[-10000-10000]|",
            "e=[-10000-10000]|d=[-10000-10000]|w=[-10000-10000]|off=0\n",
            "1|testns:peak|t=[0-10000]|h=[-10000-10000]|c=[-10000-10000]|",
            "e=[-10000-10000]|d=[-10000-10000]|w=[-10000-10000]|off=0\n"
        ),
    );
}

pub(super) fn region_generator(label: &str) -> (PathBuf, VanillaGenerator) {
    let root = scratch_root(label);
    region_pack(&root);
    let generator =
        VanillaGenerator::new(&root, 2026, "testns:split").expect("region pack generator");
    (root, generator)
}

/// End to end: the placement pass paints veins over the rule descent and
/// nothing else. Every granite block sits where the tag test held, every
/// other row keeps the descent's answer, and the base pass the cache is
/// built from places none of it.
#[test]
fn a_decorated_chunk_carries_the_pack_veins() {
    let (root, generator) = decorating_generator("decorate");
    assert!(generator.features.is_some(), "the fixture pack decorates");
    let min_y = generator.min_y();
    let mut painted = 0usize;
    for local_x in 0..16 {
        for local_z in 0..16 {
            let (x, z) = (local_x, local_z);
            let base = generator.base_column_ids(x, z);
            assert_eq!(
                base.iter()
                    .filter(|name| name.as_deref() == Some("minecraft:granite"))
                    .count(),
                0,
                "the descent alone never writes the vein's state at ({x}, {z})"
            );
            let decorated = generator.column_ids(x, z);
            for (index, name) in decorated.iter().enumerate() {
                if name.as_deref() != Some("minecraft:granite") {
                    assert_eq!(
                        name,
                        &base[index],
                        "row {} of ({x}, {z}) changed without a vein claiming it",
                        min_y + index as i32
                    );
                    continue;
                }
                painted += 1;
                assert!(
                    matches!(
                        base[index].as_deref(),
                        Some("minecraft:stone") | Some("minecraft:deepslate")
                    ),
                    "a vein only replaces what its target test accepts, saw {:?}",
                    base[index]
                );
            }
        }
    }
    assert!(
        painted > 0,
        "the fixture must actually fire veins, or the assertions above are vacuous"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

/// The decoration pass runs the ordinals the biomes of a nine-chunk
/// window contribute, and a window's biome set is enumerated over the
/// whole chunk volume, not just the surface band.
#[test]
fn the_decoration_region_covers_the_chunk_volume_and_the_nine_window() {
    let (root, generator) = region_generator("region");
    let low: Arc<str> = Arc::from("testns:low");
    let peak: Arc<str> = Arc::from("testns:peak");
    // One chunk's enumeration visits every 4x4x4 cell of its volume: 4x4
    // quart columns over height/4 rows, all distinct sampler keys.
    let cells = 4 * 4 * (generator.router.height / 4) as usize;
    assert_eq!(
        generator.chunk_biomes(1, 0).region.as_ref(),
        &[Arc::clone(&low)]
    );
    let occupancy = generator.cache_occupancy();
    assert_eq!(occupancy.biomes, cells, "the quart scan came up short");
    assert_eq!(occupancy.biome_regions, 1);
    assert_eq!(
        generator.chunk_biomes(3, 0).region.as_ref(),
        &[Arc::clone(&peak)]
    );
    let occupancy = generator.cache_occupancy();
    assert_eq!(occupancy.biomes, 2 * cells);
    assert_eq!(occupancy.biome_regions, 2);
    // A cached volume is not re-sampled.
    let _ = generator.chunk_biomes(1, 0);
    assert_eq!(generator.cache_occupancy().biomes, 2 * cells);

    // The fixture's climate split runs at x = 32, so chunk 1 sits wholly
    // west of it and chunk 3 wholly east, and the nine-chunk union
    // reaches across the split as soon as the window does: each biome
    // once, in identifier order.
    assert_eq!(
        generator.region_biomes(2, 0),
        vec![Arc::clone(&low), Arc::clone(&peak)]
    );
    assert_eq!(generator.region_biomes(0, 0), vec![Arc::clone(&low)]);
    assert_eq!(generator.region_biomes(8, 0), vec![Arc::clone(&peak)]);

    // The volume cache is bounded like the others: a sweep along a row
    // touches far more chunk volumes than it may hold.
    for chunk_x in 0..200i32 {
        let _ = generator.region_biomes(chunk_x, 0);
    }
    let occupancy = generator.cache_occupancy();
    assert!(
        occupancy.biome_regions <= 2 * VanillaGenerator::BIOME_REGION_CACHE_CAPACITY,
        "the region cache grew past its bound: {occupancy:?}"
    );
    assert!(occupancy.biome_regions < 220);
    fs::remove_dir_all(root).expect("cleanup");
}

/// A placed feature with no biome filter of its own is still gated by the
/// region: the chunk whose nine-chunk window holds only the biome that
/// does not list it runs none of its ordinals at all.
#[test]
fn a_feature_only_the_neighbouring_biome_lists_stays_out_of_the_far_chunks() {
    let (root, generator) = region_generator("region-reach");
    let count = |generator: &VanillaGenerator, chunk_x: i32, name: &str| -> usize {
        chunk_columns(generator, chunk_x, 0)
            .iter()
            .map(|column| {
                column
                    .iter()
                    .filter(|state| state.as_deref() == Some(name))
                    .count()
            })
            .sum()
    };
    let west = count(&generator, 0, "minecraft:diorite");
    let east = count(&generator, 5, "minecraft:diorite");
    let border = count(&generator, 2, "minecraft:diorite");
    assert_eq!(
        west, 0,
        "the chunks west of the split list the vein nowhere"
    );
    assert!(east > 0, "the peak biome's own chunk must be decorated");
    assert!(
        border > 0,
        "a window that reaches one chunk into the peak half is decorated too"
    );
    assert_eq!(
        count(&generator, 5, "minecraft:granite"),
        0,
        "a biome the dimension cannot produce contributes no ordinals"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

/// A chunk's grid is a pure function of its coordinates and the seed, so
/// the order chunks are asked for in cannot change a block: the answer a
/// neighbour reaches by replaying this anchor is the answer this chunk
/// reaches by clipping that replay to itself.
#[test]
fn decoration_does_not_depend_on_the_chunk_query_order() {
    let (root, forward) = decorating_generator("decorate-order");
    let reverse = VanillaGenerator::new(&root, 2026, "testns:bounded").expect("second generator");
    let block = [(0, 0), (0, 1), (1, 0), (1, 1)];
    // Visit the chunk block in one order, then read it back in a fixed
    // order: only the build order differs between the two generators.
    let visit = |generator: &VanillaGenerator, order: &[(i32, i32)]| {
        for &(chunk_x, chunk_z) in order {
            let _ = chunk_columns(generator, chunk_x, chunk_z);
        }
        block
            .iter()
            .flat_map(|&(chunk_x, chunk_z)| chunk_columns(generator, chunk_x, chunk_z))
            .collect::<Vec<_>>()
    };
    let mut backwards = block;
    backwards.reverse();
    let built_first = visit(&forward, &block);
    let built_last = visit(&reverse, &backwards);
    assert_eq!(built_first.len(), built_last.len());
    assert!(
        built_first
            .iter()
            .any(|column| column.iter().any(|name| name.is_some())),
        "the fixture must produce terrain"
    );
    assert!(
        built_first.iter().any(|column| column
            .iter()
            .any(|name| name.is_some() && name.as_deref() == Some("minecraft:granite"))),
        "and veins to compare"
    );
    assert_eq!(
        built_first, built_last,
        "a chunk decorated after its neighbours differs from one decorated before them"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

/// Eviction of a decorated grid is a recomputation, and the write zone
/// stays bounded: 300 chunk keys through `column_ids` may not pin 300
/// grids of 196 KiB each.
#[test]
fn decorated_chunk_cache_stays_bounded_across_chunk_sweeps() {
    let (root, generator) = bounded_generator("cache-decorated");
    for chunk in 0..300i32 {
        let _ = generator.column_ids(chunk * 16 + 8, 3);
    }
    let occupancy = generator.cache_occupancy();
    assert!(
        occupancy.decorated_chunks > 0,
        "the sweep did not populate the decorated-chunk memo: {occupancy:?}"
    );
    assert!(
        occupancy.decorated_chunks <= 2 * VanillaGenerator::DECORATED_CHUNK_CACHE_CAPACITY,
        "decorated grids grew past their bound: {occupancy:?}"
    );
    assert!(
        occupancy.decorated_chunks < 300,
        "one key per chunk and the sweep spans 300, so a growing map would pin all of \
         them: {occupancy:?}"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

/// An evicted grid rebuilds to the same blocks, veins included, which is
/// what makes the bound above safe to apply.
#[test]
fn an_evicted_decorated_chunk_rebuilds_identically() {
    let (root, generator) = decorating_generator("decorate-eviction");
    let before = chunk_columns(&generator, 0, 0);
    assert!(
        before
            .iter()
            .flatten()
            .any(|name| name.as_deref() == Some("minecraft:granite")),
        "the fixture chunk must hold vein blocks to compare"
    );
    // More chunk keys than the memo holds, none of them this chunk's.
    for chunk in 0..60i32 {
        let _ = generator.column_ids(chunk * 16 + 8, 91);
    }
    assert_eq!(
        chunk_columns(&generator, 0, 0),
        before,
        "eviction must be a recomputation"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

/// The fill pass seeds the whole chunk heightmap: after the 256 columns
/// of a chunk are built, every row of its ocean-floor map is stored, and
/// each stored row equals what the reference descent — surface top, then
/// down past every row the post-carve substance does not count as
/// terrain — answers for that column. The dry pack exercises rows whose
/// top is already solid (the walk stops at once); the wet pack, a sea
/// above the volume, exercises columns whose descent crosses more than a
/// hundred fluid rows before the sea floor, and both at negative and
/// positive chunk coordinates.
#[test]
fn the_fill_pass_seeds_the_heightmap_and_matches_the_descent() {
    let root = scratch_root("heightmap-fill");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("density_function/band.json"),
        r#"{"type": "mul",
            "left": {"type": "gradient", "axis": "y",
                "from_coordinate": 0, "from_value": -1.0,
                "to_coordinate": 8, "to_value": 1.0},
            "right": {"type": "gradient", "axis": "y",
                "from_coordinate": 0, "from_value": 1.0,
                "to_coordinate": 48, "to_value": -1.0}}"#,
    );
    write(&worldgen.join("material_rule/root.json"), DESCENT_ROOT);
    descent_pack_noise(&root);
    descent_pack_settings(&root, "dry", "band", -1000, "root");
    descent_pack_settings(&root, "wet", "band", 200, "root");
    for (id, floor, fluid_above) in [
        ("testns:dry", 23i32, false),
        // A sea above the volume: the surface top is the water ceiling
        // and the ocean floor is still the run's top solid row.
        ("testns:wet", 23, true),
    ] {
        let generator = VanillaGenerator::new(&root, 2026, id).expect("generator");
        for (chunk_x, chunk_z) in [(1, 0), (-2, 3)] {
            for local_x in 0..16 {
                for local_z in 0..16 {
                    let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                    let _ = generator.column_ids(x, z);
                }
            }
            let map = generator
                .ocean_floor_maps
                .borrow_mut()
                .get_mut(&(chunk_x, chunk_z))
                .expect("the fill pass seeded the chunk map")
                .clone();
            for local_x in 0..16 {
                for local_z in 0..16 {
                    let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                    let stored = map.entries[OceanFloorMap::slot(x, z)];
                    assert_ne!(
                        stored, UNCOMPUTED_OCEAN_FLOOR,
                        "({x}, {z}): the fill pass must seed every row"
                    );
                    assert_eq!(
                        stored,
                        generator.ocean_floor_by_descent(x, z),
                        "({x}, {z}): fill-derived row against the descent"
                    );
                    assert_eq!(
                        generator.ocean_floor_height(x, z),
                        stored,
                        "({x}, {z}): the served row equals the stored row"
                    );
                    assert_eq!(stored, floor, "({x}, {z}): the sea floor row");
                    if fluid_above {
                        assert!(
                            generator.surface_height(x, z) > floor,
                            "({x}, {z}): the wet column descends through fluid"
                        );
                    }
                }
            }
        }
        assert_eq!(
            generator.cache_occupancy().ocean_floor_maps,
            2,
            "two target chunks filled, two maps: the fill pass adds no halo keys"
        );
    }
    fs::remove_dir_all(root).expect("cleanup");
}
