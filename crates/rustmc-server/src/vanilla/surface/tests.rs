//! Unit tests for `vanilla::surface`.

use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_root(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("rustmc-surface-{label}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create scratch dir");
    path
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, text).expect("write");
}

/// A zero-amplitude noise definition: every stack sample is exactly
/// `0.0`, which makes the eager surface depth `(int)(0*2.75 + 3.0 +
/// [0, 0.25)) = 3` and turns threshold and band-offset comparisons
/// into fixed arithmetic.
const ZERO_NOISE: &str = r#"{"base_amplitude": 0.0, "base_octave": 4}"#;

/// The shared synthetic pack: the three system noises, a flat
/// `testns:flat` threshold noise, a solid density function, and one
/// dimension spanning y 0..=127 with `default_block` granite. Extra
/// files (rule and condition documents) are layered on top.
fn write_pack(label: &str, extra_files: &[(&str, &str)]) -> PathBuf {
    let root = scratch_root(label);
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("density_function/solid.json"),
        r#"{"type": "constant", "value": 1.0}"#,
    );
    write(&worldgen.join("noise/flat.json"), ZERO_NOISE);
    write(
        &worldgen.join("noise_settings/dim.json"),
        r#"{
            "noise": {"min_y": 0, "height": 128},
            "sea_level": 63,
            "default_block": "minecraft:granite",
            "noise_router": {
                "final_density": "testns:solid",
                "continents": 0.0,
                "erosion": 0.0,
                "depth": 0.0,
                "ridges": 0.0,
                "temperature": 0.0,
                "vegetation": 0.0
            }
        }"#,
    );
    let mc = root.join("data/minecraft/worldgen/noise");
    for name in ["surface", "surface_secondary", "clay_bands_offset"] {
        write(&mc.join(format!("{name}.json")), ZERO_NOISE);
    }
    for (rel, body) in extra_files {
        write(&worldgen.join(rel), body);
    }
    root
}

/// Compiles the `testns:root` rule of a fresh synthetic pack.
fn compile(label: &str, rules: &[(&str, &str)], conditions: &[(&str, &str)]) -> SurfaceRules {
    let mut entries: Vec<(String, String)> = Vec::with_capacity(rules.len() + conditions.len());
    for (stem, body) in rules {
        entries.push((format!("material_rule/{stem}.json"), (*body).to_owned()));
    }
    for (stem, body) in conditions {
        entries.push((
            format!("material_condition/{stem}.json"),
            (*body).to_owned(),
        ));
    }
    let files: Vec<(&str, &str)> = entries
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_str()))
        .collect();
    let root = write_pack(label, &files);
    let data = WorldgenData::load(&root).expect("load pack");
    let engine = data.engine(2026);
    let registry = data.registry(&engine);
    let router = data.router(&registry, "testns:dim").expect("router");
    SurfaceRules::compile(&data, &engine, &registry, &router, "testns:root").expect("compile")
}

/// A plains column context with fixed neighbor heights.
fn context(rules: &SurfaceRules, x: i32, z: i32) -> SurfaceContext<'_> {
    SurfaceContext::new(
        rules,
        None,
        x,
        z,
        |_, _| 70,
        |_, _, _| Some("minecraft:plains".to_owned()),
    )
}

#[test]
fn clay_bands_program_is_deterministic_and_plausible() {
    let build = |seed: u64| {
        generate_clay_bands(
            RandomSource::from_world_seed(seed)
                .fork_positional()
                .from_hash_of("minecraft:clay_bands"),
        )
    };
    let first = build(2026);
    assert_eq!(first, build(2026), "same seed, same table");
    assert_ne!(first, build(7), "different seed, different table");
    assert_eq!(first.len(), BAND_COUNT);
    let allowed: HashSet<&str> = [
        "minecraft:terracotta",
        "minecraft:orange_terracotta",
        "minecraft:yellow_terracotta",
        "minecraft:brown_terracotta",
        "minecraft:red_terracotta",
        "minecraft:white_terracotta",
        "minecraft:light_gray_terracotta",
    ]
    .into_iter()
    .collect();
    assert!(
        first.iter().all(|band| allowed.contains(band)),
        "the band table contains only badlands terracotta states"
    );
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for band in &first {
        *counts.entry(band).or_default() += 1;
    }
    let terracotta = counts["minecraft:terracotta"];
    assert_eq!(
        Some(terracotta),
        counts.values().max().copied(),
        "base terracotta is the most frequent band"
    );
    for painted in [
        "minecraft:orange_terracotta",
        "minecraft:yellow_terracotta",
        "minecraft:brown_terracotta",
        "minecraft:red_terracotta",
        "minecraft:white_terracotta",
    ] {
        assert!(
            counts.contains_key(painted),
            "the draw program paints at least one {painted} band"
        );
    }
}

#[test]
fn biome_condition_selects_by_context_biome() {
    let rules = compile(
        "biome",
        &[
            (
                "root",
                r#"{"type": "sequence", "sequence": [
                    {"type": "condition", "if_true": "testns:is_plains",
                     "then_run": "testns:grass"},
                    {"type": "block", "result_state": "minecraft:stone"}
                ]}"#,
            ),
            (
                "grass",
                r#"{"type": "block", "result_state": "minecraft:grass_block"}"#,
            ),
        ],
        &[(
            "is_plains",
            r#"{"type": "biome", "biome_is": ["minecraft:plains"]}"#,
        )],
    );
    let mut ctx = context(&rules, 4, 7);
    ctx.set_y(1, 1, None, 80);
    assert_eq!(
        rules.apply(&mut ctx).as_deref(),
        Some("minecraft:grass_block")
    );
    // A forest column falls through the sequence to the stone rule.
    // Re-setting the row clears the per-Y biome cache first.
    ctx.biome_at = Box::new(|_, _, _| Some("minecraft:forest".to_owned()));
    ctx.set_y(1, 1, None, 79);
    assert_eq!(rules.apply(&mut ctx).as_deref(), Some("minecraft:stone"));
    // No biome at all (no placement table) fails a biome condition.
    ctx.biome_at = Box::new(|_, _, _| None);
    ctx.set_y(1, 1, None, 78);
    assert_eq!(rules.apply(&mut ctx).as_deref(), Some("minecraft:stone"));
}

#[test]
fn y_above_anchor_forms_resolve_through_dimension_bounds() {
    // absolute, above_bottom, below_top (128 - 1 - 7 = 120) and the
    // bare-int form; multiplier and stone depth add the surface term.
    let cases = [
        (r#"{"absolute": 33}"#, 33),
        (r#"{"above_bottom": 8}"#, 8),
        (r#"{"below_top": 7}"#, 120),
        ("33", 33),
    ];
    for (anchor_json, anchor_y) in cases {
        let root_rule = format!(
            r#"{{"type": "condition", "if_true":
                    {{"type": "y_above", "anchor": {anchor_json},
                      "surface_depth_multiplier": 0, "add_stone_depth": false}},
                "then_run": "testns:at_anchor"}}"#
        );
        let rules = compile(
            "anchor",
            &[
                ("root", root_rule.as_str()),
                (
                    "at_anchor",
                    r#"{"type": "block", "result_state": "minecraft:stone"}"#,
                ),
            ],
            &[],
        );
        let mut ctx = context(&rules, 4, 7);
        ctx.set_y(0, 1, None, anchor_y);
        assert_eq!(
            rules.apply(&mut ctx).as_deref(),
            Some("minecraft:stone"),
            "row {anchor_y} passes for anchor {anchor_json}"
        );
        ctx.set_y(0, 1, None, anchor_y - 1);
        assert_eq!(rules.apply(&mut ctx).as_deref(), None);
    }
}

#[test]
fn y_above_applies_surface_depth_and_stone_term() {
    let rules = compile(
        "yabove",
        &[
            (
                "root",
                r#"{"type": "sequence", "sequence": [
                    {"type": "condition",
                     "if_true": {"type": "y_above", "anchor": {"above_bottom": 8},
                                 "surface_depth_multiplier": 1,
                                 "add_stone_depth": true},
                     "then_run": "testns:high"},
                    {"type": "block", "result_state": "minecraft:dirt"}
                ]}"#,
            ),
            (
                "high",
                r#"{"type": "block", "result_state": "minecraft:coarse_dirt"}"#,
            ),
        ],
        &[],
    );
    let mut ctx = context(&rules, 4, 7);
    // Threshold: y + stone_above(1) >= 8 + surface_depth(3) -> y >= 10.
    ctx.set_y(1, 1, None, 10);
    assert_eq!(
        rules.apply(&mut ctx).as_deref(),
        Some("minecraft:coarse_dirt")
    );
    ctx.set_y(1, 1, None, 9);
    assert_eq!(rules.apply(&mut ctx).as_deref(), Some("minecraft:dirt"));
}

#[test]
fn noise_threshold_window_is_inclusive() {
    let rules = compile(
        "threshold",
        &[
            (
                "root",
                r#"{"type": "condition", "if_true":
                        {"type": "noise_threshold", "noise": "testns:flat",
                         "min_threshold": 0.0, "max_threshold": 0.0, "is_3d": true},
                    "then_run": "testns:match"}"#,
            ),
            (
                "match",
                r#"{"type": "block", "result_state": "minecraft:grass_block"}"#,
            ),
        ],
        &[],
    );
    let mut ctx = context(&rules, 4, 7);
    // The zero-amplitude stack samples exactly 0.0 and the inclusive
    // [0, 0] window therefore matches.
    ctx.set_y(1, 1, None, 80);
    assert_eq!(
        rules.apply(&mut ctx).as_deref(),
        Some("minecraft:grass_block"),
        "flat noise inside a degenerate inclusive window"
    );
}

#[test]
fn water_condition_uses_height_offset_and_absent_water() {
    let rules = compile(
        "water",
        &[
            (
                "root",
                r#"{"type": "sequence", "sequence": [
                    {"type": "condition",
                     "if_true": {"type": "water", "offset": -1,
                                 "surface_depth_multiplier": 0,
                                 "add_stone_depth": false},
                     "then_run": "testns:wet"},
                    {"type": "block", "result_state": "minecraft:dirt"}
                ]}"#,
            ),
            (
                "wet",
                r#"{"type": "block", "result_state": "minecraft:clay"}"#,
            ),
        ],
        &[],
    );
    let mut ctx = context(&rules, 4, 7);
    // Water height 64: y >= 64 - 1 selects clay at 63, dirt below.
    ctx.set_y(1, 1, Some(64), 63);
    assert_eq!(rules.apply(&mut ctx).as_deref(), Some("minecraft:clay"));
    ctx.set_y(1, 1, Some(64), 62);
    assert_eq!(rules.apply(&mut ctx).as_deref(), Some("minecraft:dirt"));
    // No water column above: the condition holds unconditionally.
    ctx.set_y(1, 1, None, 5);
    assert_eq!(rules.apply(&mut ctx).as_deref(), Some("minecraft:clay"));
}

#[test]
fn stone_depth_floor_reads_above_depth_plus_surface_term() {
    let rules = compile(
        "stonedepth",
        &[
            (
                "root",
                r#"{"type": "sequence", "sequence": [
                    {"type": "condition",
                     "if_true": {"type": "stone_depth", "offset": 1,
                                 "add_surface_depth": true,
                                 "secondary_depth_range": 0,
                                 "surface_type": "floor"},
                     "then_run": "testns:near"},
                    {"type": "block", "result_state": "minecraft:dirt"}
                ]}"#,
            ),
            (
                "near",
                r#"{"type": "block", "result_state": "minecraft:grass_block"}"#,
            ),
        ],
        &[],
    );
    let mut ctx = context(&rules, 4, 7);
    // Threshold: stone_above <= 1 + offset(1) + surface_depth(3) = 5.
    ctx.set_y(5, 1, None, 80);
    assert_eq!(
        rules.apply(&mut ctx).as_deref(),
        Some("minecraft:grass_block")
    );
    ctx.set_y(6, 1, None, 80);
    assert_eq!(rules.apply(&mut ctx).as_deref(), Some("minecraft:dirt"));
}

#[test]
fn steep_condition_uses_clamped_world_surface_gradients() {
    let rules = compile(
        "steep",
        &[
            (
                "root",
                r#"{"type": "sequence", "sequence": [
                    {"type": "condition", "if_true": {"type": "steep"},
                     "then_run": "testns:cliff"},
                    {"type": "block", "result_state": "minecraft:dirt"}
                ]}"#,
            ),
            (
                "cliff",
                r#"{"type": "block", "result_state": "minecraft:stone"}"#,
            ),
        ],
        &[],
    );
    // At chunk-local x=4 the clamped neighbors are 3 and 5, and the
    // z samples coincide because the height closure ignores z.
    let mut falling = SurfaceContext::new(
        &rules,
        None,
        4,
        7,
        |cx, _| if cx == 3 { 10 } else { 0 },
        |_, _, _| Some("minecraft:plains".to_owned()),
    );
    falling.set_y(1, 1, None, 80);
    assert_eq!(
        rules.apply(&mut falling).as_deref(),
        Some("minecraft:stone"),
        "height(5) - height(3) = -10 is steep"
    );
    let mut flat = SurfaceContext::new(
        &rules,
        None,
        4,
        7,
        |_, _| 0,
        |_, _, _| Some("minecraft:plains".to_owned()),
    );
    flat.set_y(1, 1, None, 80);
    assert_eq!(rules.apply(&mut flat).as_deref(), Some("minecraft:dirt"));
}

#[test]
fn hole_condition_false_for_the_eager_surface_depth() {
    let rules = compile(
        "hole",
        &[
            (
                "root",
                r#"{"type": "sequence", "sequence": [
                    {"type": "condition", "if_true": {"type": "hole"},
                     "then_run": "testns:cave"},
                    {"type": "block", "result_state": "minecraft:dirt"}
                ]}"#,
            ),
            (
                "cave",
                r#"{"type": "block", "result_state": "minecraft:air"}"#,
            ),
        ],
        &[],
    );
    let mut ctx = context(&rules, 4, 7);
    assert_eq!(ctx.surface_depth, 3, "zero noise plus the 3.0 base");
    ctx.set_y(1, 1, None, 80);
    assert_eq!(rules.apply(&mut ctx).as_deref(), Some("minecraft:dirt"));
}

#[test]
fn vertical_gradient_branches_and_probability_stream() {
    let rules = compile(
        "gradient",
        &[
            (
                "root",
                r#"{"type": "sequence", "sequence": [
                    {"type": "condition",
                     "if_true": {"type": "vertical_gradient",
                                 "random_name": "minecraft:deepslate",
                                 "true_at_and_below": {"absolute": 20},
                                 "false_at_and_above": {"absolute": 40}},
                     "then_run": "testns:deep"},
                    {"type": "block", "result_state": "minecraft:stone"}
                ]}"#,
            ),
            (
                "deep",
                r#"{"type": "block", "result_state": "minecraft:deepslate"}"#,
            ),
        ],
        &[],
    );
    let mut ctx = context(&rules, 4, 7);
    ctx.set_y(1, 1, None, 20);
    assert_eq!(
        rules.apply(&mut ctx).as_deref(),
        Some("minecraft:deepslate"),
        "at or below the true anchor the predicate is certain"
    );
    ctx.set_y(1, 1, None, 40);
    assert_eq!(
        rules.apply(&mut ctx).as_deref(),
        Some("minecraft:stone"),
        "at or above the false anchor the predicate never holds"
    );
    // Mid-band: the mapped probability is exactly 0.5, decided by the
    // first draw of the named stream at the block position.
    let positional = RandomSource::from_world_seed(2026).fork_positional();
    let mirror = positional
        .from_hash_of("minecraft:deepslate")
        .fork_positional();
    let draw = f64::from(mirror.at(4, 30, 7).next_float());
    ctx.set_y(1, 1, None, 30);
    let got_deep = rules.apply(&mut ctx).as_deref() == Some("minecraft:deepslate");
    assert_eq!(got_deep, draw < 0.5);
}

#[test]
fn bandlands_rule_reads_the_offset_wrapped_band_table() {
    let rules = compile("bandlands", &[("root", r#"{"type": "bandlands"}"#)], &[]);
    let mut ctx = context(&rules, 4, 7);
    // The zero clay-bands offset noise rounds to shift 0, so row 80
    // selects exactly table entry 80.
    ctx.set_y(1, 1, None, 80);
    assert_eq!(
        rules.apply(&mut ctx).as_deref(),
        Some(rules.clay_bands[80]),
        "bandlands selects the wrapped table entry"
    );
}

#[test]
fn not_condition_inverts_above_preliminary_surface() {
    let rules = compile(
        "preliminary",
        &[
            (
                "root",
                r#"{"type": "sequence", "sequence": [
                    {"type": "condition",
                     "if_true": {"type": "not", "invert":
                        {"type": "above_preliminary_surface"}},
                     "then_run": "testns:below"},
                    {"type": "block", "result_state": "minecraft:stone"}
                ]}"#,
            ),
            (
                "below",
                r#"{"type": "block", "result_state": "minecraft:dripstone_block"}"#,
            ),
        ],
        &[],
    );
    let mut ctx = context(&rules, 4, 7);
    // No chunk_surface_level router slot: the preliminary surface is
    // the zero constant, so the floor sits at floor(0) + 3 - 8 = -5.
    ctx.set_y(1, 1, None, -6);
    assert_eq!(
        rules.apply(&mut ctx).as_deref(),
        Some("minecraft:dripstone_block")
    );
    ctx.set_y(1, 1, None, -5);
    assert_eq!(rules.apply(&mut ctx).as_deref(), Some("minecraft:stone"));
}

#[test]
fn ore_vein_draws_follow_the_documented_stream_order() {
    let rules = compile(
        "vein",
        &[(
            "root",
            r#"{"type": "ore_vein", "ore_block": "minecraft:iron_ore",
                "raw_ore_block": "minecraft:raw_iron_block",
                "filler_block": "minecraft:tuff", "raw_ore_chance": 0.02,
                "density": "testns:solid",
                "richness": {"type": "constant", "value": 1.0},
                "filler_gap": {"type": "constant", "value": -1.0}}"#,
        )],
        &[],
    );
    let positional = RandomSource::from_world_seed(2026).fork_positional();
    let mut ctx = context(&rules, 4, 7);
    // Density 1.0 always admits the first draw, richness 1.0 the
    // second, and a negative gap always holds, so the third draw
    // decides raw versus ordinary ore at each position.
    for y in 20..28 {
        let mirror = positional.from_hash_of("minecraft:ore").fork_positional();
        let mut column = mirror.at(4, y, 7);
        let density_draw = column.next_float();
        let richness_draw = column.next_float();
        let raw_draw = column.next_float();
        assert!(density_draw <= 1.0 && richness_draw < 1.0);
        let expect = if raw_draw < 0.02 {
            "minecraft:raw_iron_block"
        } else {
            "minecraft:iron_ore"
        };
        ctx.set_y(1, 1, None, y);
        assert_eq!(rules.apply(&mut ctx).as_deref(), Some(expect));
    }
}

#[test]
fn ore_vein_density_gate_and_filler_branch() {
    let dead = compile(
        "vein_dead",
        &[(
            "root",
            r#"{"type": "ore_vein", "ore_block": "minecraft:iron_ore",
                "raw_ore_block": "minecraft:raw_iron_block",
                "filler_block": "minecraft:tuff", "raw_ore_chance": 0.02,
                "density": {"type": "constant", "value": -1.0},
                "richness": {"type": "constant", "value": 1.0},
                "filler_gap": {"type": "constant", "value": -1.0}}"#,
        )],
        &[],
    );
    let mut ctx = context(&dead, 4, 7);
    ctx.set_y(1, 1, None, 80);
    assert_eq!(dead.apply(&mut ctx), None, "density below zero skips");

    let richless = compile(
        "vein_filler",
        &[(
            "root",
            r#"{"type": "ore_vein", "ore_block": "minecraft:iron_ore",
                "raw_ore_block": "minecraft:raw_iron_block",
                "filler_block": "minecraft:tuff", "raw_ore_chance": 0.02,
                "density": "testns:solid",
                "richness": {"type": "constant", "value": -1.0},
                "filler_gap": {"type": "constant", "value": -1.0}}"#,
        )],
        &[],
    );
    let mut ctx = context(&richless, 4, 7);
    ctx.set_y(1, 1, None, 80);
    assert_eq!(
        richless.apply(&mut ctx).as_deref(),
        Some("minecraft:tuff"),
        "failing the richness draw places only the filler"
    );
}

#[test]
fn reference_cycles_and_unknown_ids_are_rejected() {
    let cycle = write_pack(
        "cycle",
        &[
            (
                "material_rule/root.json",
                r#"{"type": "sequence", "sequence": ["testns:other"]}"#,
            ),
            (
                "material_rule/other.json",
                r#"{"type": "block", "result_state": "minecraft:stone"}"#,
            ),
            (
                "material_condition/is_plains.json",
                r#"{"type": "biome", "biome_is": ["minecraft:plains"]}"#,
            ),
        ],
    );
    // A rule reference cannot cycle; but a condition reference can
    // self-reference through a sequence, and an unknown id must fail
    // at compile rather than fall back silently.
    let data = WorldgenData::load(&cycle).expect("load");
    let engine = data.engine(2026);
    let registry = data.registry(&engine);
    let router = data.router(&registry, "testns:dim").expect("router");
    let error = SurfaceRules::compile(&data, &engine, &registry, &router, "testns:missing")
        .expect_err("unknown root id must be rejected");
    assert!(
        error.to_string().contains("unknown material rule"),
        "unexpected error: {error}"
    );
    drop(router);
    drop(registry);
    drop(engine);
    drop(data);

    let dangling = write_pack(
        "dangling",
        &[(
            "material_rule/root.json",
            r#"{"type": "condition", "if_true": "testns:nowhere",
                "then_run": {"type": "block", "result_state": "minecraft:stone"}}"#,
        )],
    );
    let data = WorldgenData::load(&dangling).expect("load");
    let engine = data.engine(2026);
    let registry = data.registry(&engine);
    let router = data.router(&registry, "testns:dim").expect("router");
    let error = SurfaceRules::compile(&data, &engine, &registry, &router, "testns:root")
        .expect_err("dangling condition reference must be rejected");
    assert!(
        error.to_string().contains("unknown material condition"),
        "unexpected error: {error}"
    );
}

/// A condition that references itself through a `not` invert is a
/// document-level cycle the registry guard must reject.
#[test]
fn condition_reference_cycle_is_rejected() {
    let root = write_pack(
        "cond-cycle",
        &[
            (
                "material_rule/root.json",
                r#"{"type": "condition", "if_true": "testns:loop",
                    "then_run": {"type": "block", "result_state": "minecraft:stone"}}"#,
            ),
            (
                "material_condition/loop.json",
                r#"{"type": "not", "invert": "testns:loop"}"#,
            ),
        ],
    );
    let data = WorldgenData::load(&root).expect("load");
    let engine = data.engine(2026);
    let registry = data.registry(&engine);
    let router = data.router(&registry, "testns:dim").expect("router");
    let error = SurfaceRules::compile(&data, &engine, &registry, &router, "testns:root")
        .expect_err("self-referencing condition must be rejected");
    assert!(
        error.to_string().contains("cycle"),
        "unexpected error: {error}"
    );
}
