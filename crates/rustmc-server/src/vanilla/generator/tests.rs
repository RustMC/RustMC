//! Unit tests for `vanilla::generator`.

use super::*;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_root(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "rustmc-generator-{label}-{}-{n}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create scratch dir");
    path
}

fn write(path: &Path, text: &str) {
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
const DESCENT_ROOT: &str = r#"{"type": "sequence", "sequence": [
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
fn descent_pack_noise(root: &Path) {
    let mc = root.join("data/minecraft/worldgen/noise");
    let zero_noise = r#"{"base_amplitude": 0.0, "base_octave": 4}"#;
    for name in ["surface", "surface_secondary", "clay_bands_offset"] {
        write(&mc.join(format!("{name}.json")), zero_noise);
    }
}

fn descent_pack_settings(root: &Path, id: &str, density: &str, sea_level: i32, rule: &str) {
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
fn bounded_pack(root: &Path) {
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

fn bounded_generator(label: &str) -> (PathBuf, VanillaGenerator) {
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
fn decorating_pack(root: &Path) {
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
fn chunk_columns(
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

fn decorating_generator(label: &str) -> (PathBuf, VanillaGenerator) {
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
fn region_pack(root: &Path) {
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

fn region_generator(label: &str) -> (PathBuf, VanillaGenerator) {
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
