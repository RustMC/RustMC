//! Unit tests, part 1 of 3.

use super::*;

pub(super) static COUNTER: AtomicU32 = AtomicU32::new(0);

pub(super) fn scratch_root(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "rustmc-generator-{label}-{}-{n}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create scratch dir");
    path
}

pub(super) fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, text).expect("write");
}

/// Builds a fabricated pack whose final density is an interpolated
/// linear ramp: gradient value `1.0` at y=0 falling to `-1.0` at y=48,
/// scaled by a folded constant multiply. The refinement scan must
/// agree with an exhaustive block-resolution scan, and the surface
/// rule (with no `aquifers` section) reproduces the global fluid
/// picker: the highest non-air position, water below the sea.
#[test]
fn extracts_surface_from_interpolated_ramp() {
    let root = scratch_root("ramp");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("density_function/ramp.json"),
        r#"{
            "type": "mul",
            "left": {"type": "interpolated", "cell_size_xz": 4, "cell_size_y": 8, "input": {
                "type": "gradient", "axis": "y",
                "from_coordinate": 0, "from_value": 1.0,
                "to_coordinate": 48, "to_value": -1.0
            }},
            "right": 0.64
        }"#,
    );
    let settings = worldgen.join("noise_settings/ramp_dimension.json");
    let generator_at_sea = |sea_level: i32| -> VanillaGenerator {
        write(
            &settings,
            &format!(
                r#"{{
            "noise": {{"min_y": 0, "height": 128}},
            "sea_level": {sea_level},
            "noise_router": {{
                "final_density": "testns:ramp",
                "continents": 0.0,
                "erosion": 0.0,
                "depth": 0.0,
                "ridges": 0.0,
                "temperature": 0.0,
                "vegetation": 0.0
            }}
        }}"#
            ),
        );
        VanillaGenerator::new(&root, 2026, "testns:ramp_dimension").expect("generator")
    };
    // The inner gradient reaches zero at y = 24 and the interpolated
    // wrapper blends it linearly; the highest positive block must
    // equal what a full block-resolution scan finds.
    let generator = generator_at_sea(-1000);
    let x = 10;
    let z = -3;
    let mut exhaustive = 0i32;
    for y in (0..128).rev() {
        if generator.router.final_density.sample(x, y, z) > 0.0 {
            exhaustive = y;
            break;
        }
    }
    assert_eq!(generator.terrain_top(x, z), exhaustive);
    assert_eq!(generator.terrain_top_exhaustive(x, z), exhaustive);
    assert!(exhaustive > 0, "ramp must have a solid base");
    // No fluid anywhere in the column when the sea is below the floor.
    assert_eq!(generator.surface_height(x, z), exhaustive);
    // Water rule: terrain below the sea reports the water top, and a
    // sea above the volume caps at the world ceiling.
    assert_eq!(generator_at_sea(10).surface_height(x, z), 23);
    assert_eq!(generator_at_sea(63).surface_height(x, z), 62);
    assert_eq!(generator_at_sea(200).surface_height(x, z), 127);
    fs::remove_dir_all(&root).expect("cleanup");
}

/// With a fabricated `aquifers` section, empty positions can turn
/// solid via barrier pressure where the raw density is negative, and
/// the surface scan must report the substance that fills the top.
#[test]
fn aquifer_section_participates_in_surface_scan() {
    let root = scratch_root("aquifer");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("density_function/all_positive.json"),
        r#"{"type": "constant", "value": 1.0}"#,
    );
    write(
        &worldgen.join("noise_settings/aquifer_dimension.json"),
        r#"{
            "noise": {"min_y": 0, "height": 128},
            "sea_level": 63,
            "default_fluid": "minecraft:water",
            "noise_router": {
                "final_density": "testns:all_positive",
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
            }
        }"#,
    );
    let generator =
        VanillaGenerator::new(&root, 2026, "testns:aquifer_dimension").expect("generator");
    assert!(matches!(generator.aquifer, Aquifer::NoiseBased(_)));
    // All-density solid everywhere: the surface is the volume ceiling.
    let (y, substance) = generator.surface(5, 5).expect("surface");
    assert_eq!(y, 127);
    assert_eq!(substance, Substance::Solid);
    fs::remove_dir_all(&root).expect("cleanup");
}

/// The generator resolves biomes through the provisioned placement
/// table when one exists beside the data root and reports `None`
/// otherwise. Climate slots are constants here so the expected
/// nearest entry is fully determined by the self-authored table.
#[test]
fn biome_requires_a_provisioned_placement_table() {
    let root = scratch_root("biome");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("density_function/solid.json"),
        r#"{"type": "constant", "value": 1.0}"#,
    );
    let settings_path = worldgen.join("noise_settings/biome_dimension.json");
    let write_settings = |temperature: &str| {
        write(
            &settings_path,
            &format!(
                r#"{{
            "noise": {{"min_y": 0, "height": 128}},
            "sea_level": 63,
            "noise_router": {{
                "final_density": "testns:solid",
                "continents": 0.0,
                "erosion": 0.0,
                "depth": 0.0,
                "ridges": 0.0,
                "temperature": {temperature},
                "vegetation": 0.0
            }}
        }}"#
            ),
        );
    };
    write_settings("0.5");
    // Without a placement file the same dimension reports no biome.
    let bare = VanillaGenerator::new(&root, 2026, "testns:biome_dimension").expect("bare");
    assert!(bare.placement.is_none());
    assert_eq!(bare.biome(0, 0, 64), None);

    let psv = root.join("rustmc/biome_placement/biome_dimension.psv");
    write(
        &psv,
        concat!(
            "0|minecraft:plains|t=[-2000-2000]|h=[-10000-10000]|c=[-10000-10000]|",
            "e=[-10000-10000]|d=[-10000-10000]|w=[-10000-10000]|off=0\n",
            "1|minecraft:desert|t=[3000-10000]|h=[-10000-10000]|c=[-10000-10000]|",
            "e=[-10000-10000]|d=[-10000-10000]|w=[-10000-10000]|off=0\n",
        ),
    );
    let generator =
        VanillaGenerator::new(&root, 2026, "testns:biome_dimension").expect("generator");
    assert_eq!(
        generator.biome(0, 0, 64).as_deref(),
        Some("minecraft:desert"),
        "temperature 0.5 quantizes to 5000, inside the desert interval"
    );
    // A colder router selects the other entry instead.
    write_settings("-1.0");
    let cold = VanillaGenerator::new(&root, 2026, "testns:biome_dimension").expect("cold");
    assert_eq!(cold.biome(0, 0, 64).as_deref(), Some("minecraft:plains"));
    fs::remove_dir_all(&root).expect("cleanup");
}

/// Requires the operator-provisioned 26.3 worldgen data plus the
/// captured overworld placement table (see `docs/PROVENANCE.md`).
#[test]
#[ignore = "requires operator-provisioned local data"]
fn smoke_resolves_overworld_biomes_from_provisioned_table() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let generator =
        VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
    let placement = generator.placement.as_ref().expect("provisioned psv table");
    assert!(placement.len() > 1000, "captured table should be large");
    let surface_y = generator.surface_height(0, 0);
    let biome = generator.biome(0, 0, surface_y).expect("biome at origin");
    assert!(
        biome.starts_with("minecraft:"),
        "expected a namespaced biome, got {biome}"
    );
}

/// Requires the operator-provisioned 26.3 worldgen data. The full
/// overworld rule tree must compile and produce a namespaced block
/// id for every sampled surface column.
#[test]
#[ignore = "requires operator-provisioned local data"]
fn smoke_top_blocks_resolve_from_provisioned_surface_rules() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let generator =
        VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
    assert!(generator.surface.is_some(), "overworld has material rules");
    for (x, z) in [(0, 0), (32, -17), (-128, 96), (1000, 1000)] {
        let top = generator
            .top_block(x, z)
            .unwrap_or_else(|| panic!("column ({x}, {z}) has no top block at all"));
        assert!(
            top.starts_with("minecraft:"),
            "expected a namespaced top block, got {top}"
        );
    }
}

/// End-to-end `top_block` on a fabricated pack: a rule that always
/// matches replaces the filler block, and a rule gated on a biome
/// the column cannot resolve falls back to the settings
/// `default_block`.
#[test]
fn top_block_applies_material_rules_or_keeps_default_block() {
    let root = scratch_root("topblock");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("density_function/solid.json"),
        r#"{"type": "constant", "value": 1.0}"#,
    );
    let zero_noise = r#"{"base_amplitude": 0.0, "base_octave": 4}"#;
    let mc = root.join("data/minecraft/worldgen/noise");
    for name in ["surface", "surface_secondary", "clay_bands_offset"] {
        write(&mc.join(format!("{name}.json")), zero_noise);
    }
    write(
        &worldgen.join("material_rule/always.json"),
        r#"{"type": "block", "result_state": "minecraft:sandstone"}"#,
    );
    write(
        &worldgen.join("material_rule/never.json"),
        r#"{"type": "condition",
            "if_true": {"type": "biome", "biome_is": ["minecraft:badlands"]},
            "then_run": {"type": "block", "result_state": "minecraft:red_sand"}}"#,
    );
    let settings = |id: &str, material_rule: &str| {
        write(
            &worldgen.join(format!("noise_settings/{id}.json")),
            &format!(
                r#"{{
            "noise": {{"min_y": 0, "height": 128}},
            "sea_level": 63,
            "default_block": "minecraft:granite",
            "noise_router": {{
                "final_density": "testns:solid",
                "continents": 0.0,
                "erosion": 0.0,
                "depth": 0.0,
                "ridges": 0.0,
                "temperature": 0.0,
                "vegetation": 0.0
            }},
            "material_rule": "{material_rule}"
        }}"#
            ),
        );
    };
    settings("hit", "testns:always");
    settings("miss", "testns:never");
    // A fully solid constant-density column tops out at the ceiling.
    let hitting = VanillaGenerator::new(&root, 2026, "testns:hit").expect("hit generator");
    assert_eq!(hitting.surface_height(4, 7), 127);
    assert_eq!(
        hitting.top_block(4, 7).as_deref(),
        Some("minecraft:sandstone"),
        "an unconditional rule recolors the topmost solid"
    );
    // No placement table: the biome-gated rule never holds and the
    // column keeps the filler default block.
    let missing = VanillaGenerator::new(&root, 2026, "testns:miss").expect("miss generator");
    assert_eq!(
        missing.top_block(4, 7).as_deref(),
        Some("minecraft:granite")
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

/// The rule program shared by the descent tests: floor and ceiling
/// `stone_depth` gates (a zero-noise pack makes the depth ranges
/// deterministic), a vertical-gradient rule coloring the deep rows,
/// and an unconditional fallthrough.
pub(super) const DESCENT_ROOT: &str = r#"{"type": "sequence", "sequence": [
    {"type": "condition",
        "if_true": {"type": "stone_depth", "surface_type": "floor",
            "offset": 1, "add_surface_depth": false,
            "secondary_depth_range": 0},
        "then_run": {"type": "block", "result_state": "minecraft:dirt"}},
    {"type": "condition",
        "if_true": {"type": "stone_depth", "surface_type": "ceiling",
            "offset": 1, "add_surface_depth": false,
            "secondary_depth_range": 0},
        "then_run": {"type": "block", "result_state": "minecraft:moss_block"}},
    {"type": "condition",
        "if_true": {"type": "vertical_gradient",
            "random_name": "minecraft:rustmc_test_deep",
            "true_at_and_below": {"absolute": 10},
            "false_at_and_above": {"absolute": 11}},
        "then_run": {"type": "block", "result_state": "minecraft:deepslate"}},
    {"type": "block", "result_state": "minecraft:stone"}
]}"#;

/// Writes the zero-amplitude noise documents and the constant
/// noise-router shell the surface program needs for a fabricated
/// overworld-like pack.
pub(super) fn descent_pack_noise(root: &Path) {
    let mc = root.join("data/minecraft/worldgen/noise");
    let zero_noise = r#"{"base_amplitude": 0.0, "base_octave": 4}"#;
    for name in ["surface", "surface_secondary", "clay_bands_offset"] {
        write(&mc.join(format!("{name}.json")), zero_noise);
    }
}

pub(super) fn descent_pack_settings(
    root: &Path,
    id: &str,
    density: &str,
    sea_level: i32,
    rule: &str,
) {
    write(
        &root.join(format!("data/testns/worldgen/noise_settings/{id}.json")),
        &format!(
            r#"{{
            "noise": {{"min_y": 0, "height": 128}},
            "sea_level": {sea_level},
            "default_fluid": "minecraft:water",
            "default_block": "minecraft:granite",
            "noise_router": {{
                "final_density": "testns:{density}",
                "continents": 0.0,
                "erosion": 0.0,
                "depth": 0.0,
                "ridges": 0.0,
                "temperature": 0.0,
                "vegetation": 0.0
            }},
            "material_rule": "testns:{rule}"
        }}"#
        ),
    );
}

/// A read past the chunk edge returns the neighbour's own terrain rather
/// than nothing, and the store of those columns stays bounded.
#[test]
fn read_block_sees_the_neighbouring_terrain_within_a_bounded_store() {
    let root = scratch_root("halo");
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
    let generator = VanillaGenerator::new(&root, 2026, "testns:dry").expect("generator");
    let mut chunk = DecoratedChunk::new(generator.router.height as usize);
    let halo = RefCell::new(HaloColumns::new());
    let view = ChunkView {
        generator: &generator,
        chunk_x: 0,
        chunk_z: 0,
        chunk: &mut chunk,
        halo: &halo,
    };
    let expected = generator.column_ids(16, 3);
    assert_eq!(
        view.block_at(16, 22, 3),
        None,
        "the writable view stops here"
    );
    assert_eq!(
        view.read_block(16, 22, 3).as_deref(),
        expected[22].as_deref(),
        "the neighbour's filled row"
    );
    assert_eq!(
        view.read_block(16, 100, 3).as_deref(),
        Some("minecraft:air"),
        "the neighbour's open sky"
    );
    assert_eq!(
        view.read_block(16, 100_000, 3),
        None,
        "outside the build volume"
    );
    for z in 0..(HALO_COLUMN_LIMIT as i32 + 8) {
        view.read_block(16, 22, z);
    }
    assert!(halo.borrow().len() <= HALO_COLUMN_LIMIT);
    fs::remove_dir_all(root).expect("cleanup");
}

/// End-to-end material-rule descent on a fabricated pack whose
/// density is the product of two y-gradients: one solid run from
/// y=5 to y=23 with nothing solid below or above. Floor rules must
/// catch the two topmost rows of the run, ceiling rules its two
/// bottommost rows, the gradient rule the rows at or below 10, and
/// the sea variant must stamp the fluid rows without disturbing the
/// solid results; a biome-gated-only program keeps the filler
/// default block on every solid row.
#[test]
fn column_ids_descends_rules_through_solid_runs() {
    let root = scratch_root("descent");
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
        &worldgen.join("material_rule/never.json"),
        r#"{"type": "condition",
            "if_true": {"type": "biome", "biome_is": ["minecraft:badlands"]},
            "then_run": {"type": "block", "result_state": "minecraft:red_sand"}}"#,
    );
    descent_pack_noise(&root);
    descent_pack_settings(&root, "dry", "band", -1000, "root");
    descent_pack_settings(&root, "wet", "band", 200, "root");
    descent_pack_settings(&root, "fallback", "band", -1000, "never");

    // The gradient product is positive exactly on 4 < y < 24, so the
    // run spans y = 5..=23 with stone_above 1 at y = 23 counting down
    // and stone_below 1 at y = 5 counting up.
    let dry = VanillaGenerator::new(&root, 2026, "testns:dry").expect("dry generator");
    let ids = dry.column_ids(6, -2);
    assert_eq!(ids.len(), 128);
    for (y, id) in ids[..5].iter().enumerate() {
        assert_eq!(*id, None, "row {y} below the run is pre-carve air");
    }
    assert_eq!(ids[5].as_deref(), Some("minecraft:moss_block"));
    assert_eq!(ids[6].as_deref(), Some("minecraft:moss_block"));
    for id in &ids[7..=10] {
        assert_eq!(id.as_deref(), Some("minecraft:deepslate"));
    }
    for id in &ids[11..=21] {
        assert_eq!(id.as_deref(), Some("minecraft:stone"));
    }
    assert_eq!(ids[22].as_deref(), Some("minecraft:dirt"));
    assert_eq!(ids[23].as_deref(), Some("minecraft:dirt"));
    for (i, id) in ids[24..128].iter().enumerate() {
        assert_eq!(*id, None, "row {} above the column top stays unset", i + 24);
    }
    assert_eq!(
        dry.top_block(6, -2).as_deref(),
        ids[23].as_deref(),
        "the descent's top solid row agrees with the top-block pass"
    );

    // A sea above the volume fills every non-solid row: the descent
    // still walks from the fluid top and colors the run identically.
    let wet = VanillaGenerator::new(&root, 2026, "testns:wet").expect("wet generator");
    let ids = wet.column_ids(6, -2);
    assert_eq!(ids[127].as_deref(), Some("minecraft:water"));
    assert_eq!(ids[24].as_deref(), Some("minecraft:water"));
    assert_eq!(ids[4].as_deref(), Some("minecraft:water"));
    assert_eq!(ids[0].as_deref(), Some("minecraft:water"));
    assert_eq!(ids[23].as_deref(), Some("minecraft:dirt"));
    assert_eq!(ids[8].as_deref(), Some("minecraft:deepslate"));
    assert_eq!(ids[5].as_deref(), Some("minecraft:moss_block"));

    // No placement table: the only rule never holds and every solid
    // row keeps the settings default block.
    let fallback =
        VanillaGenerator::new(&root, 2026, "testns:fallback").expect("fallback generator");
    let ids = fallback.column_ids(6, -2);
    for id in &ids[5..=23] {
        assert_eq!(id.as_deref(), Some("minecraft:granite"));
    }
    assert_eq!(ids[4], None);
    assert_eq!(ids[24], None);
    fs::remove_dir_all(&root).expect("cleanup");
}

/// A solid run that reaches the bottom of the world keeps the
/// documented below-depth sentinel, so the ceiling gate (depth <= 2)
/// never fires inside the run and the floor row itself falls through
/// to the gradient rule.
#[test]
fn column_ids_floor_run_keeps_below_depth_sentinel() {
    let root = scratch_root("sentinel");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("density_function/ramp.json"),
        r#"{"type": "mul",
            "left": {"type": "interpolated", "cell_size_xz": 4, "cell_size_y": 8, "input": {
                "type": "gradient", "axis": "y",
                "from_coordinate": 0, "from_value": 1.0,
                "to_coordinate": 48, "to_value": -1.0
            }},
            "right": 0.64}"#,
    );
    write(&worldgen.join("material_rule/root.json"), DESCENT_ROOT);
    descent_pack_noise(&root);
    descent_pack_settings(&root, "floor", "ramp", -1000, "root");

    let generator = VanillaGenerator::new(&root, 2026, "testns:floor").expect("floor generator");
    let ids = generator.column_ids(10, -3);
    // Same ramp top as the ramp scan test: y = 23.
    assert_eq!(ids[23].as_deref(), Some("minecraft:dirt"));
    assert_eq!(ids[22].as_deref(), Some("minecraft:dirt"));
    assert_eq!(ids[21].as_deref(), Some("minecraft:stone"));
    for id in &ids[11..=20] {
        assert_eq!(id.as_deref(), Some("minecraft:stone"));
    }
    // The run reaches min_y: sentinel below-depths are far above the
    // ceiling gate's range, and rows at or below 10 color deepslate.
    for id in &ids[0..=10] {
        assert_eq!(id.as_deref(), Some("minecraft:deepslate"));
    }
    for id in &ids[24..128] {
        assert_eq!(*id, None);
    }
    assert_eq!(
        generator.top_block(10, -3).as_deref(),
        ids[23].as_deref(),
        "the descent's top solid row agrees with the top-block pass"
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

/// Requires the operator-provisioned 26.3 worldgen data. The descent
/// must reproduce the deepslate split below the transition band and fire
/// both ore-vein rules: granite and tuff are vein states the rest of the
/// program never writes, so any occurrence proves the copper and iron
/// veins evaluated. A vein may only replace a block its target test
/// accepts, and every replaceable tag in the pack (`stone_ore_replaceables`,
/// `deepslate_ore_replaceables`, `height_specific_ore_replaceables`) is a
/// subset of `base_stone_overworld`, so that one tag answers for the whole
/// placement stage: a row the pass changed must have held one of its
/// members before, and no row may appear or disappear. Both checks cover
/// the rows under each column's highest solid base block, because the
/// vegetation step legitimately places plants on and above the terrain.
///
/// The four 16×16 squares sit at fixed origins on a power-of-two grid,
/// with one off-grid pair so at least one square straddles chunk borders
/// and exercises the neighbour replay. Veins are dense enough that the
/// families appear at any underground position: the coordinates are a
/// sample, not a tuned pick.
#[test]
#[ignore = "requires operator-provisioned local data"]
fn smoke_column_ids_splits_deepslate_and_fires_veins() {
    const SPRING_WATER: &str = "minecraft:water";
    const SPRING_LAVA: &str = "minecraft:lava";
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let generator =
        VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
    let min_y = generator.min_y();
    let tags = generator
        .features
        .as_ref()
        .expect("the provisioned pack decorates")
        .tags();
    let mut granite = 0usize;
    let mut tuff = 0usize;
    let mut ores = 0usize;
    let mut unexpected_deep: Vec<(i32, i32, i32, String)> = Vec::new();
    for (origin_x, origin_z) in [(0, 0), (512, -512), (-1024, 1024), (255, -97)] {
        for x in origin_x..origin_x + 16 {
            for z in origin_z..origin_z + 16 {
                let base = generator.base_column_ids(x, z);
                let ids = generator.column_ids(x, z);
                // The vegetation step places plants on and above the
                // terrain (grass on air, seagrass and kelp in water), so
                // the invariants below hold under the column's highest
                // solid base row only.
                let solid_rows = base
                    .iter()
                    .rposition(|id| {
                        id.as_deref().is_some_and(|id| {
                            !matches!(id, "minecraft:cave_air" | SPRING_WATER | SPRING_LAVA)
                        })
                    })
                    .map_or(0, |row| row + 1);
                for (index, id) in ids.iter().enumerate().take(solid_rows) {
                    let y = min_y + index as i32;
                    let before = base[index].as_deref();
                    if let Some(id) = id {
                        match id.as_str() {
                            "minecraft:granite" => granite += 1,
                            "minecraft:tuff" => tuff += 1,
                            "minecraft:deepslate_iron_ore" | "minecraft:deepslate_copper_ore" => {
                                ores += 1
                            }
                            _ => {}
                        }
                    }
                    // Between the iron band and the deepslate/stone
                    // transition only the underground rule, carving and
                    // the dimension fluids can have written a row. An
                    // unset row is the cave air the carvers left in the
                    // column, so it is expected here too.
                    if (-8..=-2).contains(&y)
                        && !matches!(
                            before,
                            None | Some("minecraft:deepslate")
                                | Some("minecraft:cave_air")
                                | Some("minecraft:water")
                                | Some("minecraft:lava")
                        )
                        && unexpected_deep.len() < 20
                    {
                        unexpected_deep.push((x, z, y, before.unwrap_or_default().to_string()));
                    }
                    if before == id.as_deref() {
                        continue;
                    }
                    // A row the pass did change: a vein recolors only rows its
                    // target test accepts, and a spring is the one feature that
                    // fills an unset (cave air) row, with its fluid's block.
                    let spring_fill = before.is_none()
                        && matches!(id.as_deref(), Some(SPRING_WATER | SPRING_LAVA));
                    assert!(
                        spring_fill
                            || before
                                .is_some_and(|before| tags
                                    .contains("minecraft:base_stone_overworld", before)),
                        "({x}, {z}) row {y}: the placement stage wrote {id:?} onto {before:?}"
                    );
                }
                assert_eq!(
                    ids.iter()
                        .zip(base.iter())
                        .take(solid_rows)
                        .filter(|(after, before)| {
                            after.is_some() != before.is_some()
                                && !(before.is_none()
                                    && matches!(after.as_deref(), Some(SPRING_WATER | SPRING_LAVA)))
                        })
                        .count(),
                    0,
                    "({x}, {z}): only a spring may create a row, and nothing may remove one"
                );
            }
        }
    }
    assert!(granite > 0, "copper veins must leave granite filler");
    assert!(tuff > 0, "the tuff band must fire");
    assert!(ores > 0, "the iron or copper target rules must place ore");
    assert!(
        unexpected_deep.is_empty(),
        "unexpected pre-feature rows in the deep band: {unexpected_deep:?}"
    );
}

/// Requires the operator-provisioned 26.3 worldgen data. On columns
/// whose top row the carvers left alone, the descent's top solid row
/// must equal the single-row top-block pass.
#[test]
#[ignore = "requires operator-provisioned local data"]
fn smoke_column_ids_top_row_matches_top_block() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let generator =
        VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
    let min_y = generator.min_y();
    for (x, z) in [(0, 0), (32, -17), (-128, 96)] {
        let top = generator.surface_height(x, z);
        if generator.carved(x, top, z) {
            continue;
        }
        let ids = generator.column_ids(x, z);
        let index = (top - min_y) as usize;
        let row = ids[index].as_deref().expect("top row is written");
        assert_eq!(
            generator.top_block(x, z).as_deref(),
            Some(row),
            "column ({x}, {z}) top row at y={top}"
        );
    }
}

/// Requires the operator-provisioned 26.3 worldgen data. On real terrain
/// the stored heightmap must be the descent: the target chunk is filled
/// through the block-id pass, which derives its 256 rows, and then the
/// chunk's rows and the halo band the anchor gate's footprints spill into
/// are served from the maps and compared against `ocean_floor_by_descent`,
/// which never reads a map. The sampled band must reach rows above ocean
/// floors that the filler left as fluid and rows a carver stamped, so a
/// pack that only ever answered "the top is solid" could not pass.
#[test]
#[ignore = "requires operator-provisioned local data"]
fn smoke_ocean_floor_heightmap_serves_the_descent_on_real_terrain() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let generator =
        VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
    let mut fluid_rows = 0usize;
    let mut carved_rows = 0usize;
    let mut columns = 0usize;
    for (chunk_x, chunk_z) in [(-20, -8), (4, -20), (-4, 5)] {
        for local_x in 0..16 {
            for local_z in 0..16 {
                let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                let _ = generator.base_column_ids(x, z);
            }
        }
        let stored = generator
            .ocean_floor_maps
            .borrow_mut()
            .get_mut(&(chunk_x, chunk_z))
            .expect("the fill pass stored the target chunk map")
            .entries;
        assert!(
            stored.iter().all(|row| *row != UNCOMPUTED_OCEAN_FLOOR),
            "chunk ({chunk_x}, {chunk_z}): the fill pass left a row uncomputed"
        );
        // The widest box in the pack spills about thirteen blocks past its
        // anchor chunk, so the gate can read this band without the band's
        // own chunk ever being filled. The target chunk is checked column
        // by column; outside it a one-in-four lattice is enough, since the
        // answer of every column is computed the same way and the descent
        // — not the coverage — is what a smoke can afford to pay.
        for x in chunk_x * 16 - 13..chunk_x * 16 + 16 + 13 {
            for z in chunk_z * 16 - 13..chunk_z * 16 + 16 + 13 {
                let in_target = x >> 4 == chunk_x && z >> 4 == chunk_z;
                if !in_target && ((x & 3) | (z & 3)) != 0 {
                    continue;
                }
                let floor = generator.ocean_floor_height(x, z);
                assert_eq!(
                    floor,
                    generator.ocean_floor_by_descent(x, z),
                    "column ({x}, {z}): stored row and descent disagree"
                );
                let top = generator.surface_height(x, z);
                // Split the rows the walk left above the floor by what
                // they are: a row a carver stamped, and a row the filler
                // itself left as fluid. Both have to appear for the smoke
                // to have crossed the terrain the gate is asked about.
                for y in floor..top {
                    if generator.carved(x, y, z) {
                        carved_rows += 1;
                    } else if matches!(generator.substance(x, y, z), Substance::Fluid(_)) {
                        fluid_rows += 1;
                    }
                }
                columns += 1;
            }
        }
    }
    assert!(columns > 1_000, "the smoke sampled {columns} columns");
    assert!(
        carved_rows > 0 && fluid_rows > 0,
        "the sampled band crossed {fluid_rows} filler-fluid rows and {carved_rows} \
         carve-stamped rows above ocean floors"
    );
    println!(
        "smoke OK: {columns} columns, {fluid_rows} filler-fluid rows and {carved_rows} \
         carved rows above their floors, root {}",
        root.display()
    );
}
