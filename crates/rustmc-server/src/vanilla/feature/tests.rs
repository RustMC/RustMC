//! Unit tests for `vanilla::feature`.

use super::*;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_root(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("rustmc-feature-{label}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create scratch dir");
    path
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, text).expect("write");
}

#[test]
fn parses_the_ore_shape_and_its_targets() {
    let document = json!({
        "type": "minecraft:ore",
        "size": 64,
        "discard_chance_on_air_exposure": 0.0,
        "targets": [{
            "state": "minecraft:andesite",
            "target": {"predicate_type": "minecraft:tag_match", "tag": "minecraft:base_stone_overworld"}
        }]
    });
    let feature = parse_configured_feature(&document).expect("parses");
    let ConfiguredFeature::Ore {
        size,
        targets,
        discard_chance_on_air_exposure,
    } = feature
    else {
        panic!("expected an ore feature: {feature:?}");
    };
    assert_eq!(size, 64);
    assert_eq!(discard_chance_on_air_exposure, 0.0);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].state, "minecraft:andesite");
    assert_eq!(
        targets[0].predicate,
        RuleTest::Tag {
            tag: "minecraft:base_stone_overworld".to_string()
        }
    );
}

#[test]
fn unknown_feature_types_are_recorded_not_dropped() {
    let feature = parse_configured_feature(&json!({"type": "minecraft:tree"})).expect("parses");
    assert_eq!(
        feature,
        ConfiguredFeature::Unsupported {
            kind: "minecraft:tree".to_string()
        }
    );
    assert!(!feature.is_supported());
}

#[test]
fn an_out_of_budget_ore_size_is_rejected() {
    for size in [0, -8, 129, i64::MAX] {
        let error = parse_configured_feature(&json!({
            "type": "minecraft:ore",
            "size": size,
            "targets": [{
                "state": "minecraft:tuff",
                "target": {"predicate_type": "minecraft:matching_blocks", "blocks": ["minecraft:stone"]}
            }]
        }))
        .expect_err("rejected");
        assert!(
            error.to_string().contains("size must be in 1..=128"),
            "{error}"
        );
    }
}

#[test]
fn placement_rejects_overflowing_and_unbounded_inputs() {
    let configured = HashMap::from([(
        "minecraft:test".to_string(),
        Rc::new(ConfiguredFeature::Unsupported {
            kind: "minecraft:tree".to_string(),
        }),
    )]);
    for placement in [
        json!([{"type": "minecraft:count", "count": i64::MAX}]),
        json!([{"type": "minecraft:count", "count": 257}]),
        json!([{"type": "minecraft:count", "count": {
            "type": "minecraft:uniform", "min_inclusive": {"absolute": 0},
            "max_inclusive": {"absolute": 257}
        }}]),
        json!([{"type": "minecraft:height_range", "height": {
            "type": "minecraft:uniform", "min_inclusive": {"absolute": i64::MAX},
            "max_inclusive": {"absolute": 0}
        }}]),
        json!([{"type": "minecraft:count", "count": 256},
               {"type": "minecraft:count", "count": 256}]),
    ] {
        let document = json!({"feature": "minecraft:test", "placement": placement});
        assert!(
            parse_placed_feature("minecraft:test_placed", &document, &configured).is_err(),
            "oversized input must fail at load time: {document}"
        );
    }
}

#[test]
fn reads_the_count_rarity_square_height_and_biome_steps() {
    let configured = HashMap::from([(
        "minecraft:ore_granite".to_string(),
        Rc::new(ConfiguredFeature::Ore {
            size: 64,
            targets: vec![OreTarget {
                predicate: RuleTest::Tag {
                    tag: "minecraft:base_stone_overworld".to_string(),
                },
                state: "minecraft:granite".to_string(),
            }],
            discard_chance_on_air_exposure: 0.0,
        }),
    )]);
    let document = json!({
        "feature": "minecraft:ore_granite",
        "placement": [
            {"type": "minecraft:rarity_filter", "chance": 6},
            {"type": "minecraft:in_square"},
            {"type": "minecraft:height_range", "height": {
                "type": "minecraft:uniform",
                "max_inclusive": {"absolute": 128},
                "min_inclusive": {"absolute": 64}
            }},
            {"type": "minecraft:biome"}
        ]
    });
    let placed = parse_placed_feature("minecraft:ore_granite_upper", &document, &configured)
        .expect("parses");
    assert!(placed.is_modelable());
    assert_eq!(
        placed.modifiers,
        vec![
            PlacementModifier::RarityFilter { chance: 6 },
            PlacementModifier::InSquare,
            PlacementModifier::HeightRange {
                height: RangeProvider::Uniform {
                    min_inclusive: VerticalAnchor::Absolute(64),
                    max_inclusive: VerticalAnchor::Absolute(128),
                }
            },
            PlacementModifier::Biome,
        ]
    );
}

#[test]
fn an_above_bottom_anchor_resolves_against_the_dimension_bounds() {
    let document = json!({
        "feature": "minecraft:ore_tuff",
        "placement": [
            {"type": "minecraft:count", "count": 2},
            {"type": "minecraft:in_square"},
            {"type": "minecraft:height_range", "height": {
                "type": "minecraft:uniform",
                "max_inclusive": {"absolute": 0},
                "min_inclusive": {"above_bottom": 0}
            }},
            {"type": "minecraft:biome"}
        ]
    });
    let tuff = ConfiguredFeature::Ore {
        size: 64,
        targets: vec![OreTarget {
            predicate: RuleTest::Blocks {
                blocks: vec!["minecraft:deepslate".to_string()],
            },
            state: "minecraft:tuff".to_string(),
        }],
        discard_chance_on_air_exposure: 0.0,
    };
    let configured = HashMap::from([("minecraft:ore_tuff".to_string(), Rc::new(tuff.clone()))]);
    let placed =
        parse_placed_feature("minecraft:ore_tuff", &document, &configured).expect("parses");
    let PlacementModifier::HeightRange {
        height:
            RangeProvider::Uniform {
                min_inclusive,
                max_inclusive,
            },
    } = &placed.modifiers[2]
    else {
        panic!("expected a uniform height range: {:?}", placed.modifiers[2]);
    };
    // The overworld spans -64..=319.
    assert_eq!(min_inclusive.resolve(-64, 384), -64);
    assert_eq!(max_inclusive.resolve(-64, 384), 0);
    assert_eq!(VerticalAnchor::BelowTop(0).resolve(-64, 384), 319);
}

#[test]
fn an_unmodeled_decoration_makes_the_feature_unmodelable() {
    let ore = ConfiguredFeature::Ore {
        size: 10,
        targets: vec![OreTarget {
            predicate: RuleTest::Tag {
                tag: "minecraft:base_stone_overworld".to_string(),
            },
            state: "minecraft:coal_ore".to_string(),
        }],
        discard_chance_on_air_exposure: 0.0,
    };
    let configured =
        HashMap::from([("minecraft:ore_coal_upper".to_string(), Rc::new(ore.clone()))]);
    let document = json!({
        "feature": "minecraft:ore_coal_upper",
        "placement": [
            {"type": "minecraft:count", "count": {"type": "minecraft:clumped", "inner": 5}},
            {"type": "minecraft:random_offset", "xz_spread": 7, "y_spread": 3},
            {"type": "minecraft:biome"}
        ]
    });
    let placed = parse_placed_feature("test", &document, &configured).expect("parses");
    assert!(
        !placed.is_modelable(),
        "an unmodeled provider or decoration must not be silently run"
    );
}

/// A pack of one modeled ore feature, one placed feature whose
/// `placement` array the caller supplies verbatim, and one biome listing
/// it — so the only thing the report can be talking about is the
/// decorations.
fn report_pack(label: &str, placement: &str) -> PathBuf {
    let root = scratch_root(label);
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("feature/andesite.json"),
        r#"{"type": "minecraft:ore", "size": 8, "discard_chance_on_air_exposure": 0.0,
            "targets": [{"state": "minecraft:andesite",
                "target": {"predicate_type": "minecraft:matching_blocks",
                    "blocks": ["minecraft:stone"]}}]}"#,
    );
    write(
        &worldgen.join("placed_feature/one.json"),
        &format!(r#"{{"feature": "testns:andesite", "placement": {placement}}}"#),
    );
    write(
        &worldgen.join("biome/plains.json"),
        r#"{"features": [["testns:one"]]}"#,
    );
    root
}

#[test]
fn an_unmodeled_decoration_is_named_in_the_report() {
    // The configured type here is `minecraft:ore`, which this runtime
    // models, so before this counter the feature places nothing and the
    // report says nothing about why.
    let root = report_pack(
        "report-modifier",
        r#"[{"type": "minecraft:count", "count": 2},
            {"type": "minecraft:random_offset", "xz_spread": 7, "y_spread": 3},
            {"type": "minecraft:biome"}]"#,
    );
    let data = FeatureData::load(&root).expect("loads");
    assert_eq!(
        data.unimplemented_features
            .get("placement:minecraft:random_offset"),
        Some(&1),
        "the decoration that gates the chain is named"
    );
    assert_eq!(
        data.unimplemented_features.len(),
        1,
        "every other decoration in the chain is modeled: {:?}",
        data.unimplemented_features
    );
    assert!(
        !data.step(0).expect("step 0")[0].is_modelable(),
        "and the feature is still gated out of the run"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_fully_modeled_chain_adds_nothing_to_the_report() {
    let root = report_pack(
        "report-clean",
        r#"[{"type": "minecraft:count", "count": 2},
            {"type": "minecraft:rarity_filter", "chance": 4},
            {"type": "minecraft:in_square"},
            {"type": "minecraft:height_range", "height": {"type": "minecraft:uniform",
                "min_inclusive": {"absolute": 10}, "max_inclusive": {"absolute": 60}}},
            {"type": "minecraft:biome"}]"#,
    );
    let data = FeatureData::load(&root).expect("loads");
    assert!(
        data.unimplemented_features.is_empty(),
        "a chain this runtime runs is not a gap: {:?}",
        data.unimplemented_features
    );
    assert!(data.step(0).expect("step 0")[0].is_modelable());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_repeated_decoration_counts_once_per_document() {
    let root = report_pack(
        "report-dedup",
        r#"[{"type": "minecraft:random_offset", "xz_spread": 7, "y_spread": 3},
            {"type": "minecraft:random_offset", "xz_spread": 2, "y_spread": 1},
            {"type": "minecraft:count_on_every_layer", "count": 1},
            {"type": "minecraft:biome"}]"#,
    );
    let data = FeatureData::load(&root).expect("loads");
    // One document, one kind, two occurrences: the counter counts the
    // feature the report is about, not the modifier's repetition.
    assert_eq!(
        data.unimplemented_features
            .get("placement:minecraft:random_offset"),
        Some(&1)
    );
    assert_eq!(
        data.unimplemented_features
            .get("placement:minecraft:count_on_every_layer"),
        Some(&1),
        "a second unmodeled kind in the same document gets its own key"
    );
    assert_eq!(data.unimplemented_features.len(), 2);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn the_configured_and_target_keys_keep_their_meaning() {
    let root = scratch_root("report-kinds");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("feature/swamp.json"),
        r#"{"type": "minecraft:tree"}"#,
    );
    write(
        &worldgen.join("feature/diorite.json"),
        r#"{"type": "minecraft:ore", "size": 8, "discard_chance_on_air_exposure": 0.0,
            "targets": [
                {"state": "minecraft:diorite",
                    "target": {"predicate_type": "minecraft:matching_blocks",
                        "blocks": ["minecraft:stone"]}},
                {"state": "minecraft:granite",
                    "target": {"predicate_type": "minecraft:random_block_match",
                        "block": "minecraft:stone", "probability": 0.5}}
            ]}"#,
    );
    write(
        &worldgen.join("placed_feature/one.json"),
        r#"{"feature": "testns:diorite", "placement": [
            {"type": "minecraft:count_on_every_layer", "count": 1}
        ]}"#,
    );
    write(
        &worldgen.join("biome/plains.json"),
        r#"{"features": [["testns:one"]]}"#,
    );
    let data = FeatureData::load(&root).expect("loads");
    assert_eq!(
        data.unimplemented_features.get("minecraft:tree"),
        Some(&1),
        "one increment per configured document of an unmodeled type"
    );
    assert_eq!(
        data.unimplemented_features
            .get("target:minecraft:random_block_match"),
        Some(&1),
        "one increment per ore target using an unmodeled predicate"
    );
    assert_eq!(
        data.unimplemented_features
            .get("placement:minecraft:count_on_every_layer"),
        Some(&1)
    );
    assert_eq!(
        data.unimplemented_features.len(),
        3,
        "the three namespaces stay separate: {:?}",
        data.unimplemented_features
    );
    assert!(
        !data
            .unimplemented_features
            .contains_key("minecraft:random_block_match"),
        "a rule or decoration kind never reads as a feature type"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_biome_reference_to_an_unknown_feature_is_an_error() {
    let data = FeatureData::default();
    let error = parse_biome_steps(
        "minecraft:plains",
        &json!({"features": [[], [], [], [], [], [], ["minecraft:ore_typo"]]}),
        &data.placed,
        &data.configured,
        &[],
        &[],
        &mut 0,
    )
    .expect_err("rejected");
    assert!(error.to_string().contains("references unknown"), "{error}");
}

#[test]
fn an_empty_biome_section_yields_no_steps() {
    let steps = parse_biome_steps(
        "minecraft:plains",
        &json!({}),
        &HashMap::new(),
        &HashMap::new(),
        &[],
        &[],
        &mut 0,
    )
    .expect("a biome without features contributes nothing");
    assert!(steps.is_empty());
}

// --- The decoration runtime -------------------------------------------

/// A terrain the runtime can paint on without a generator: an unlimited
/// volume of one base block, the writable zone pinned to one 16-wide
/// chunk, and one biome for the `minecraft:biome` filter. Reads answer
/// outside the writable chunk too, because a border block's six
/// neighbours are queried regardless; above the build volume the terrain
/// answer is air, which is what makes a vein's top rows exposed.
#[derive(Debug)]
struct TestTarget {
    min_y: i32,
    height: i32,
    chunk_x: i32,
    chunk_z: i32,
    base: String,
    write_anywhere: bool,
    ground: i32,
    written: HashMap<(i32, i32, i32), String>,
    biome: Option<String>,
}

impl TestTarget {
    fn new() -> Self {
        Self {
            min_y: 0,
            height: 128,
            chunk_x: 0,
            chunk_z: 0,
            base: "minecraft:stone".to_string(),
            write_anywhere: false,
            ground: 127,
            written: HashMap::new(),
            biome: Some("minecraft:plains".to_string()),
        }
    }

    /// The chunk the pass may write into.
    fn anchored_at(mut self, chunk_x: i32, chunk_z: i32) -> Self {
        self.chunk_x = chunk_x;
        self.chunk_z = chunk_z;
        self
    }

    /// Let the pass write anywhere in the build volume, so a test can
    /// measure a vein's true extent instead of its clipped one.
    fn write_anywhere(mut self) -> Self {
        self.write_anywhere = true;
        self
    }

    /// The row every column's ocean-floor heightmap reports. The fixture
    /// terrain fills the build volume, so the default is its top row and no
    /// attempt is ever unanchored; a lower row leaves the sky above it empty
    /// for the anchor gate to read.
    fn with_ground(mut self, y: i32) -> Self {
        self.ground = y;
        self
    }

    fn in_biome(mut self, biome: Option<&str>) -> Self {
        self.biome = biome.map(str::to_owned);
        self
    }

    fn max_y(&self) -> i32 {
        self.min_y + self.height - 1
    }

    /// How many written positions hold `name`.
    fn blocks(&self, name: &str) -> usize {
        self.written
            .values()
            .filter(|state| state.as_str() == name)
            .count()
    }

    fn total(&self) -> usize {
        self.written.len()
    }
}

impl DecorationTarget for TestTarget {
    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn height(&self) -> i32 {
        self.height
    }

    fn writable(&self, x: i32, y: i32, z: i32) -> bool {
        (self.write_anywhere || (x >> 4, z >> 4) == (self.chunk_x, self.chunk_z))
            && y >= self.min_y
            && y <= self.max_y()
    }

    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<&str> {
        if let Some(name) = self.written.get(&(x, y, z)) {
            return Some(name);
        }
        (y >= self.min_y && y <= self.max_y()).then_some(self.base.as_str())
    }

    fn is_air(&self, x: i32, y: i32, z: i32) -> bool {
        match self.block_at(x, y, z) {
            None => true,
            Some(name) => name == "minecraft:air",
        }
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, name: &str) {
        self.written.insert((x, y, z), name.to_string());
    }

    fn biome_at(&self, _x: i32, _y: i32, _z: i32) -> Option<String> {
        self.biome.clone()
    }

    fn ocean_floor_height(&self, _x: i32, _z: i32) -> i32 {
        self.ground
    }
}

/// An ore that replaces the fixture terrain's base block.
fn stone_replacement(state: &str, size: i32, discard: f32) -> Rc<ConfiguredFeature> {
    Rc::new(ConfiguredFeature::Ore {
        size,
        targets: vec![OreTarget {
            predicate: RuleTest::Blocks {
                blocks: vec!["minecraft:stone".to_string()],
            },
            state: state.to_string(),
        }],
        discard_chance_on_air_exposure: discard,
    })
}

fn wrapper(
    id: &str,
    feature: &Rc<ConfiguredFeature>,
    modifiers: Vec<PlacementModifier>,
) -> Rc<PlacedFeature> {
    Rc::new(PlacedFeature {
        id: id.to_string(),
        feature: Rc::clone(feature),
        modifiers,
    })
}

/// A registry running one step's schedule, listing everything it carries
/// in the fixture biome. A single biome whose only chain is that step's
/// list is numbered by position, which is what the derived ordering gives
/// when one biome is the whole source.
fn registry(step: Vec<Rc<PlacedFeature>>) -> FeatureData {
    let membership: HashSet<String> = step.iter().map(|feature| feature.id.clone()).collect();
    let mut ordinals = HashMap::new();
    let mut positions = Vec::new();
    for (index, feature) in step.iter().enumerate() {
        ordinals.insert(feature.id.clone(), index);
        positions.push(index);
    }
    FeatureData {
        steps: vec![StepSchedule {
            features: step,
            ordinals,
        }],
        biome_ordinals: HashMap::from([("minecraft:plains".to_string(), vec![positions])]),
        membership: HashMap::from([("minecraft:plains".to_string(), membership)]),
        ..Default::default()
    }
}

/// The fixture decoration region: one anchor chunk whose biome volume is
/// the fixture biome every `TestTarget` reports.
fn region() -> Vec<String> {
    vec!["minecraft:plains".to_string()]
}

/// The identifiers one generation step's schedule runs, in ordinal order.
fn step_ids(data: &FeatureData, step: usize) -> Vec<String> {
    data.step(step)
        .unwrap_or_else(|| panic!("the pack has no step {step}"))
        .iter()
        .map(|feature| feature.id.clone())
        .collect()
}

/// Runs one vein on `target` from the stream `seed`/`ordinal` gives it.
fn paint(
    mut target: TestTarget,
    seed: i64,
    ordinal: i32,
    origin: DecorationPos,
    feature: &ConfiguredFeature,
) -> TestTarget {
    let ConfiguredFeature::Ore {
        size,
        targets,
        discard_chance_on_air_exposure,
    } = feature
    else {
        panic!("the fixture ore is an ore: {feature:?}");
    };
    let mut random = DecorationRandom::new(seed);
    random.set_feature_seed(seed, ordinal, 0);
    let written = place_ore(
        &mut target,
        &mut random,
        &BlockTags::default(),
        *size,
        targets,
        *discard_chance_on_air_exposure,
        origin,
    );
    // The visited set makes each position of a vein a write, never a
    // repaint: the two counts have to agree.
    assert_eq!(
        written,
        target.total(),
        "one vein writes each position once"
    );
    target
}

/// Runs one ore attempt against a target that keeps its own stream, so a
/// test can watch what the attempts of one chain spend in order.
fn attempt(
    target: &mut TestTarget,
    random: &mut DecorationRandom,
    origin: DecorationPos,
    feature: &ConfiguredFeature,
) -> usize {
    let ConfiguredFeature::Ore {
        size,
        targets,
        discard_chance_on_air_exposure,
    } = feature
    else {
        panic!("the fixture ore is an ore: {feature:?}");
    };
    place_ore(
        target,
        random,
        &BlockTags::default(),
        *size,
        targets,
        *discard_chance_on_air_exposure,
        origin,
    )
}

/// The next three delegate draws of a stream, so it can be compared
/// against a reference stream that consumed a known number of longs.
fn signature(random: &DecorationRandom) -> [i32; 3] {
    let mut probe = random.clone();
    [probe.next_int(), probe.next_int(), probe.next_int()]
}

fn reference(baseline: &DecorationRandom, longs: u32) -> [i32; 3] {
    let mut probe = baseline.clone();
    for _ in 0..longs {
        probe.next_bits(32);
    }
    signature(&probe)
}

#[test]
fn a_vein_is_reproducible_and_seed_dependent() {
    let ore = stone_replacement("minecraft:granite", 64, 0.0);
    let origin = DecorationPos {
        x: 40,
        y: 64,
        z: -12,
    };
    let first = paint(TestTarget::new().write_anywhere(), 2026, 3, origin, &ore);
    let again = paint(TestTarget::new().write_anywhere(), 2026, 3, origin, &ore);
    assert!(
        first.total() > 8,
        "the fixture vein must actually place blocks, got {}",
        first.total()
    );
    assert_eq!(
        first.written, again.written,
        "the same seed, ordinal and origin must replay identically"
    );
    let other = paint(TestTarget::new().write_anywhere(), 2027, 3, origin, &ore);
    assert_ne!(
        first.written, other.written,
        "a different world seed must move the stream"
    );
}

/// The anchor gate: a vein's box is measured against the heightmap row of
/// the columns under its footprint, and an attempt whose base row clears
/// every one of them is abandoned before it draws a radius. The row that
/// decides is the box's own base — seven under the origin for a size-64
/// vein — so the boundary sits a whole box-height above where the shape
/// would first miss the ground, and what a dropped attempt costs the chain
/// is the three draws that fixed its axis.
#[test]
fn a_box_that_clears_the_terrain_costs_only_its_axis_draws() {
    let ore = stone_replacement("minecraft:granite", 64, 0.0);
    let ground = 40;
    let origin = DecorationPos {
        x: 4,
        y: ground + 7,
        z: 4,
    };

    let mut boundary = TestTarget::new().with_ground(ground);
    let mut random = DecorationRandom::new(2026);
    random.set_feature_seed(2026, 0, 0);
    let placed = attempt(&mut boundary, &mut random, origin, &ore);
    assert!(
        placed > 0,
        "a box whose base row reaches the terrain places, got {placed}"
    );

    let mut floating = TestTarget::new().with_ground(ground);
    let mut random = DecorationRandom::new(2026);
    random.set_feature_seed(2026, 0, 0);
    let dropped = attempt(
        &mut floating,
        &mut random,
        DecorationPos { y: 48, ..origin },
        &ore,
    );
    assert_eq!(dropped, 0, "the same box one row higher is dropped");
    assert_eq!(
        floating.total(),
        0,
        "and the abandoned box leaves nothing behind"
    );

    // The fixture's terrain fills the build volume unless a test lowers it,
    // so the row above is the gate's doing, not a vein that lands nowhere:
    // the identical attempt places into a full column.
    let mut full_column = TestTarget::new();
    let mut random = DecorationRandom::new(2026);
    random.set_feature_seed(2026, 0, 0);
    let unbounded = attempt(
        &mut full_column,
        &mut random,
        DecorationPos { y: 48, ..origin },
        &ore,
    );
    assert!(
        unbounded > 0,
        "without the heightmap floor the same attempt places, got {unbounded}"
    );

    // A dropped attempt is over the moment the gate answers, so the stream
    // it leaves behind is the stream three draws — the angle and the two
    // height jitters — would have left, with none of the vein's radii spent.
    let mut baseline = DecorationRandom::new(2026);
    baseline.set_feature_seed(2026, 0, 0);

    let mut dropped = TestTarget::new().with_ground(ground);
    let mut spent = baseline.clone();
    let skipped = attempt(
        &mut dropped,
        &mut spent,
        DecorationPos { y: 100, ..origin },
        &ore,
    );
    assert_eq!(skipped, 0, "the sky-high box is dropped");
    assert_eq!(
        signature(&spent),
        reference(&baseline, 3),
        "a dropped attempt costs its three axis draws, not its radii"
    );

    // Which is what the next attempt of the same chain is written against:
    // it lands where three draws leave the stream, not where a full vein
    // would have left it.
    let mut chained = TestTarget::new().with_ground(ground);
    let mut random = baseline.clone();
    attempt(
        &mut chained,
        &mut random,
        DecorationPos { y: 100, ..origin },
        &ore,
    );
    let after = attempt(&mut chained, &mut random, origin, &ore);

    let mut resumed = TestTarget::new().with_ground(ground);
    let mut random = baseline.clone();
    let _ = random.next_float();
    let _ = random.next_int_bounded(3);
    let _ = random.next_int_bounded(3);
    let direct = attempt(&mut resumed, &mut random, origin, &ore);

    assert!(direct > 0, "the anchored attempt places, got {direct}");
    assert_eq!(
        after, direct,
        "the attempt after a dropped one draws the numbers a three-draw gap leaves"
    );
    assert_eq!(
        chained.written, resumed.written,
        "and lands the same blocks at the same positions"
    );
}

/// The premise of the ring-one replay, measured from the geometry itself
/// rather than from the box arithmetic: a vein's blocks never lie more
/// than one chunk away from the chunk its attempt was anchored in, so
/// replaying the eight neighbours captures every write a wider radius
/// could add.
#[test]
fn a_vein_never_reaches_past_the_neighbour_chunk() {
    for size in [4, 10, 20, 64] {
        let ore = stone_replacement("minecraft:granite", size, 0.0);
        let mut total = 0usize;
        for ordinal in 0..6i32 {
            for seed in [2026i64, 7777] {
                // Origins spread across a chunk the way `in_square` does.
                let origin = DecorationPos {
                    x: (ordinal * 3 + seed as i32 % 16).rem_euclid(16),
                    y: 64,
                    z: ordinal * 5,
                };
                let target = paint(
                    TestTarget::new().write_anywhere(),
                    seed,
                    ordinal,
                    origin,
                    &ore,
                );
                let (chunk_x, chunk_z) = (origin.x >> 4, origin.z >> 4);
                for &(x, _y, z) in target.written.keys() {
                    assert!(
                        ((x >> 4) - chunk_x).abs() <= 1 && ((z >> 4) - chunk_z).abs() <= 1,
                        "a size-{size} vein at ({x}, {z}) reached chunk {} {}, outside the \
                         one-chunk reach of its anchor chunk {chunk_x}, {chunk_z}",
                        x >> 4,
                        z >> 4,
                    );
                }
                total += target.total();
            }
        }
        assert!(total > 0, "a size-{size} vein must place something");
    }
}

#[test]
fn writes_outside_the_decorated_chunk_are_refused() {
    let ore = stone_replacement("minecraft:granite", 64, 0.0);
    let target = paint(
        TestTarget::new().anchored_at(3, -2),
        2026,
        0,
        DecorationPos {
            x: 48,
            y: 64,
            z: -32,
        },
        &ore,
    );
    assert!(
        target.total() > 0,
        "the fixture chunk must be reached at all"
    );
    for &(x, _y, z) in target.written.keys() {
        assert_eq!(
            (x >> 4, z >> 4),
            (3, -2),
            "the pass clips to the chunk it decorates; the neighbour paints its half of a straddling vein"
        );
    }
}

/// A chunk-1 anchor reaches chunk 0 and a chunk-5 anchor cannot, which is
/// what makes the three-by-three replay both sufficient and the smallest
/// honest window.
#[test]
fn a_neighbour_anchor_reaches_the_target_chunk_and_a_far_one_does_not() {
    let ore = stone_replacement("minecraft:granite", 64, 0.0);
    let mut from_neighbour = 0usize;
    let mut from_far_chunk = 0usize;
    for seed in 2026..2034 {
        for ordinal in 0..4i32 {
            for offset in [0i32, 4, 8, 12] {
                for (anchor, reached) in [(1, &mut from_neighbour), (5, &mut from_far_chunk)] {
                    let target = paint(
                        TestTarget::new().anchored_at(0, 0),
                        seed,
                        ordinal,
                        DecorationPos {
                            x: (anchor << 4) + offset,
                            y: 64,
                            z: 8,
                        },
                        &ore,
                    );
                    *reached += target.total();
                }
            }
        }
    }
    assert!(
        from_neighbour > 0,
        "a vein seeded in the neighbour chunk must bleed into the target chunk"
    );
    assert_eq!(
        from_far_chunk, 0,
        "an anchor four chunks away is past any vein's reach"
    );
}

/// Both a zero and a certain discard cost no random draws, so the same
/// vein geometry is evaluated twice and only the exposure test differs:
/// the certain run is a strict subset of the free one, thinned exactly
/// where the terrain runs out.
#[test]
fn air_exposure_only_removes_blocks_that_touch_air() {
    let loose = paint(
        TestTarget::new().write_anywhere(),
        2026,
        1,
        DecorationPos {
            x: 20,
            y: 126,
            z: 20,
        },
        &stone_replacement("minecraft:granite", 64, 0.0),
    );
    let strict = paint(
        TestTarget::new().write_anywhere(),
        2026,
        1,
        DecorationPos {
            x: 20,
            y: 126,
            z: 20,
        },
        &stone_replacement("minecraft:granite", 64, 1.0),
    );
    assert!(
        loose.written.keys().any(|&(_x, y, _z)| y == loose.max_y()),
        "the fixture vein must reach the top of the volume to test exposure"
    );
    assert!(
        strict.total() > 0 && strict.total() < loose.total(),
        "the exposed rows must drop out and the buried core must stay: {} vs {}",
        strict.total(),
        loose.total()
    );
    for (position, name) in &strict.written {
        assert_eq!(
            loose.written.get(position),
            Some(name),
            "a discard roll must thin the vein, not move it, at {position:?}"
        );
        assert!(
            position.1 < strict.max_y(),
            "{position:?} is exposed above the volume and must have been rejected"
        );
    }
}

#[test]
fn each_range_provider_draws_its_documented_number_of_randoms() {
    let cases: [(RangeProvider, i32, u32); 5] = [
        (RangeProvider::Constant(64), 64, 0),
        (
            RangeProvider::Uniform {
                min_inclusive: VerticalAnchor::Absolute(0),
                max_inclusive: VerticalAnchor::Absolute(0),
            },
            0,
            1,
        ),
        (
            RangeProvider::Uniform {
                min_inclusive: VerticalAnchor::Absolute(0),
                max_inclusive: VerticalAnchor::Absolute(15),
            },
            15,
            1,
        ),
        (
            RangeProvider::Trapezoid {
                min_inclusive: VerticalAnchor::Absolute(3),
                max_inclusive: VerticalAnchor::Absolute(3),
            },
            3,
            1,
        ),
        (
            RangeProvider::Trapezoid {
                min_inclusive: VerticalAnchor::Absolute(0),
                max_inclusive: VerticalAnchor::Absolute(1),
            },
            1,
            2,
        ),
    ];
    let baseline = DecorationRandom::new(2026);
    for (provider, upper_bound, longs) in cases {
        let mut sampled = baseline.clone();
        let value = provider.sample(0, 128, &mut sampled);
        assert!(
            (0..=upper_bound).contains(&value),
            "{provider:?} sampled {value} outside its band"
        );
        if let RangeProvider::Constant(_) = provider {
            assert_eq!(value, 64);
        }
        assert_eq!(
            signature(&sampled),
            reference(&baseline, longs),
            "{provider:?} must cost exactly {longs} delegate longs"
        );
    }
    // Bounds the pack wrote the wrong way round: the reference runtime
    // reports the minimum and draws nothing at all.
    let inverted = RangeProvider::Uniform {
        min_inclusive: VerticalAnchor::Absolute(70),
        max_inclusive: VerticalAnchor::Absolute(20),
    };
    let mut sampled = baseline.clone();
    assert_eq!(inverted.sample(0, 128, &mut sampled), 70);
    assert_eq!(
        signature(&sampled),
        reference(&baseline, 0),
        "an inverted band must not draw"
    );
}

#[test]
fn a_rarity_filter_gates_the_whole_attempt_and_a_count_repeats_it() {
    let ore = stone_replacement("minecraft:granite", 20, 0.0);
    let chain = |chance: i64| {
        registry(vec![wrapper(
            "testns:ore",
            &ore,
            vec![
                PlacementModifier::Count {
                    count: RangeProvider::Constant(3),
                },
                PlacementModifier::HeightRange {
                    height: RangeProvider::Constant(64),
                },
                PlacementModifier::RarityFilter { chance },
            ],
        )])
    };
    let mut always = TestTarget::new().write_anywhere();
    let written = chain(1).decorate_step(&mut always, 0, 0, 0, 2026, &region());
    assert!(
        written > 0 && always.blocks("minecraft:granite") > 0,
        "a chance of one fires every time and the count multiplies the attempt"
    );
    let mut rare = TestTarget::new().write_anywhere();
    assert_eq!(
        chain(1_000_000).decorate_step(&mut rare, 0, 0, 0, 2026, &region()),
        0,
        "one attempt in a million does not fire on this stream"
    );
}

#[test]
fn the_biome_filter_keeps_only_a_biome_that_lists_the_feature() {
    let ore = stone_replacement("minecraft:granite", 20, 0.0);
    let data = registry(vec![wrapper(
        "testns:ore",
        &ore,
        vec![
            PlacementModifier::Count {
                count: RangeProvider::Constant(2),
            },
            PlacementModifier::HeightRange {
                height: RangeProvider::Constant(64),
            },
            PlacementModifier::Biome,
        ],
    )]);
    let mut listed = TestTarget::new().write_anywhere();
    assert!(
        data.decorate_step(&mut listed, 0, 0, 0, 2026, &region()) > 0,
        "the fixture biome lists the feature"
    );
    let mut elsewhere = TestTarget::new()
        .write_anywhere()
        .in_biome(Some("minecraft:desert"));
    assert_eq!(
        data.decorate_step(&mut elsewhere, 0, 0, 0, 2026, &region()),
        0,
        "a biome that does not list it places nothing"
    );
    let mut unknown = TestTarget::new().write_anywhere().in_biome(None);
    assert_eq!(
        data.decorate_step(&mut unknown, 0, 0, 0, 2026, &region()),
        0
    );
}

/// A feature with no runtime here places nothing and still costs no
/// draws, but it holds its ordinal, so the streams of the features after
/// it are the ones the pack seeded them with.
#[test]
fn an_unmodeled_feature_still_shifts_its_neighbours() {
    let ore = stone_replacement("minecraft:granite", 20, 0.0);
    let vein = wrapper(
        "testns:ore",
        &ore,
        vec![
            PlacementModifier::Count {
                count: RangeProvider::Constant(2),
            },
            PlacementModifier::HeightRange {
                height: RangeProvider::Constant(64),
            },
        ],
    );
    let forest = wrapper(
        "testns:forest",
        &Rc::new(ConfiguredFeature::Unsupported {
            kind: "minecraft:tree".to_string(),
        }),
        Vec::new(),
    );
    let mut alone = TestTarget::new().write_anywhere();
    registry(vec![Rc::clone(&vein)]).decorate_step(&mut alone, 0, 0, 0, 2026, &region());
    let mut shifted = TestTarget::new().write_anywhere();
    registry(vec![Rc::clone(&forest), Rc::clone(&vein)]).decorate_step(
        &mut shifted,
        0,
        0,
        0,
        2026,
        &region(),
    );
    assert!(!alone.written.is_empty() && !shifted.written.is_empty());
    assert!(shifted.blocks("minecraft:granite") > 0 && alone.blocks("minecraft:granite") > 0);
    assert_ne!(
        alone.written, shifted.written,
        "the unmodeled feature holds a slot, so the vein after it is seeded differently"
    );
}

#[test]
fn rule_trees_nest_any_all_and_not_tests() {
    let tags = BlockTags::default();
    let tree = RuleTest::AllOf(vec![
        RuleTest::Not(Box::new(RuleTest::Block {
            block: "minecraft:bedrock".to_string(),
        })),
        RuleTest::AnyOf(vec![
            RuleTest::Blocks {
                blocks: vec![
                    "minecraft:stone".to_string(),
                    "minecraft:deepslate".to_string(),
                ],
            },
            RuleTest::Tag {
                tag: "minecraft:base_stone_overworld".to_string(),
            },
        ]),
        RuleTest::Height {
            min_inclusive: -64,
            max_inclusive: 0,
        },
    ]);
    assert!(tree.is_modelable());
    assert!(tree.test(Some("minecraft:deepslate"), -40, &tags));
    assert!(
        tree.test(Some("minecraft:stone"), 0, &tags),
        "both bounds are inclusive"
    );
    assert!(
        !tree.test(Some("minecraft:stone"), 1, &tags),
        "the height test bounds the band"
    );
    assert!(
        !tree.test(Some("minecraft:bedrock"), -40, &tags),
        "the not-test holds"
    );
    assert!(
        !tree.test(Some("minecraft:granite"), -40, &tags),
        "an unknown tag matches nothing rather than assuming membership"
    );
    assert!(
        !tree.test(None, -40, &tags),
        "a position holding no block matches no block test"
    );
    let unsupported = RuleTest::AnyOf(vec![
        RuleTest::Block {
            block: "minecraft:stone".to_string(),
        },
        RuleTest::Unsupported {
            kind: "minecraft:random_block_match".to_string(),
        },
    ]);
    assert!(!unsupported.is_modelable());
    assert!(
        !RuleTest::Unsupported {
            kind: "minecraft:random_block_match".to_string(),
        }
        .test(Some("minecraft:stone"), 0, &tags),
        "an unmodeled test matches nothing when it is asked"
    );
}

#[test]
fn tag_membership_resolves_nested_references_and_cycles() {
    let root = scratch_root("tags");
    write(
        &root.join("data/minecraft/tags/block/base_stone_overworld.json"),
        r#"{"values": ["minecraft:stone", "minecraft:deepslate"]}"#,
    );
    // Filed away, so its id keeps the directory.
    write(
        &root.join("data/testns/tags/block/deep/deep_stone.json"),
        r##"{"values": ["minecraft:cobblestone", "#testns:base_stone"]}"##,
    );
    write(
        &root.join("data/testns/tags/block/base_stone.json"),
        r##"{"values": [{"id": "minecraft:stone", "required": true}, {"id": "#testns:deep/deep_stone"}, {"id": "#testns:optional", "required": false}]}"##,
    );
    let tags = BlockTags::load(&root).expect("loads");
    assert!(tags.contains("minecraft:base_stone_overworld", "minecraft:stone"));
    assert!(
        !tags.contains("minecraft:base_stone_overworld", "minecraft:granite"),
        "a member list is exactly what the pack declared"
    );
    assert!(tags.contains("testns:base_stone", "minecraft:stone"));
    assert!(
        tags.contains("testns:base_stone", "minecraft:cobblestone"),
        "a reference to another tag resolves into the member list"
    );
    assert!(
        tags.contains("testns:deep/deep_stone", "minecraft:stone"),
        "and so does the reference that reaches back through the cycle"
    );
    assert!(
        !tags.contains("testns:deep/deep_stone", "#testns:base_stone"),
        "a tag reference is not a literal block id"
    );
    assert_eq!(tags.len(), 3, "one entry per path under the category");

    let empty = scratch_root("no-tags");
    let tags = BlockTags::load(&empty).expect("a pack without tags loads");
    assert!(tags.is_empty() && !tags.contains("minecraft:base_stone_overworld", "minecraft:stone"));
    fs::remove_dir_all(root).expect("cleanup");
    fs::remove_dir_all(empty).expect("cleanup");
}

#[test]
fn missing_required_tag_reference_is_an_error() {
    let root = scratch_root("missing-required-tag");
    write(
        &root.join("data/testns/tags/block/parent.json"),
        r##"{"values": ["#testns:absent"]}"##,
    );
    let error = BlockTags::load(&root).unwrap_err().to_string();
    assert!(error.contains("testns:parent references missing required tag testns:absent"));
    write(
        &root.join("data/testns/tags/block/parent.json"),
        r##"{"values": [{"id":"#testns:absent","required":true}, {"id":"#testns:absent","required":false}]}"##,
    );
    let error = BlockTags::load(&root).unwrap_err().to_string();
    assert!(
        error.contains("testns:parent references missing required tag testns:absent"),
        "an optional duplicate must not hide a required reference"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
#[ignore = "requires operator-provisioned local data"]
fn pack_motion_heightmap_tags_resolve_string_references() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
    let tags = BlockTags::load(&root).expect("the provisioned block tags load");
    assert!(tags.contains("minecraft:blocks_motion_in_heightmap", "minecraft:stone"));
    assert!(tags.contains(
        "minecraft:blocks_motion_in_heightmap",
        "minecraft:oak_leaves"
    ));
    assert!(tags.contains(
        "minecraft:blocks_motion_in_heightmap_no_leaves",
        "minecraft:stone"
    ));
    assert!(!tags.contains(
        "minecraft:blocks_motion_in_heightmap_no_leaves",
        "minecraft:oak_leaves"
    ));
    assert!(!tags.contains(
        "minecraft:blocks_motion_in_heightmap",
        "minecraft:short_grass"
    ));
    assert!(!tags.contains(
        "minecraft:blocks_motion_in_heightmap_no_leaves",
        "minecraft:short_grass"
    ));
}

#[test]
fn item_tags_resolve_nested_optional_and_cycles() {
    let root = scratch_root("item-tags");
    write(
        &root.join("data/testns/tags/item/sub/widgets.json"),
        r##"{"values": ["testns:widget_a", "#testns:cycle_a"]}"##,
    );
    write(
        &root.join("data/testns/tags/item/cycle_a.json"),
        r##"{"values": ["testns:widget_b", "#testns:cycle_b"]}"##,
    );
    write(
        &root.join("data/testns/tags/item/cycle_b.json"),
        r##"{"values": ["#testns:cycle_a"]}"##,
    );
    write(
        &root.join("data/testns/tags/item/tools.json"),
        r##"{"values": ["testns:hammer", {"id": "#testns:sub/widgets"}, {"id": "#testns:optional_missing", "required": false}]}"##,
    );
    let tags = ItemTags::load(&root).expect("item tags load");
    assert_eq!(tags.len(), 4);
    assert!(!tags.is_empty());
    assert!(tags.contains("testns:tools", "testns:hammer"));
    assert!(tags.contains("testns:tools", "testns:widget_a"));
    assert!(tags.contains("testns:tools", "testns:widget_b"));
    assert_eq!(
        tags.members("testns:tools").expect("tools tag").len(),
        3,
        "optional missing entry is ignored and nested cycles terminate"
    );
    assert!(tags.contains("testns:cycle_b", "testns:widget_b"));
    assert_eq!(tags.members("testns:absent"), None);

    let empty = scratch_root("no-item-tags");
    let empty_tags = ItemTags::load(&empty).expect("pack without item tags loads");
    assert!(empty_tags.is_empty());
    fs::remove_dir_all(root).expect("cleanup");
    fs::remove_dir_all(empty).expect("cleanup");
}

#[test]
fn item_tags_honour_replace_across_pack_roots() {
    let base = scratch_root("item-tags-base");
    let overlay = scratch_root("item-tags-overlay");
    write(
        &base.join("data/testns/tags/item/widgets.json"),
        r#"{"values": ["testns:alpha", "testns:beta"]}"#,
    );
    write(
        &base.join("data/testns/tags/item/merged.json"),
        r#"{"values": ["testns:one"]}"#,
    );
    write(
        &base.join("data/testns/tags/item/parent.json"),
        r##"{"values": ["#testns:widgets"]}"##,
    );
    write(
        &overlay.join("data/testns/tags/item/widgets.json"),
        r#"{"replace": true, "values": ["testns:gamma"]}"#,
    );
    write(
        &overlay.join("data/testns/tags/item/merged.json"),
        r#"{"replace": false, "values": ["testns:two"]}"#,
    );

    let tags = ItemTags::load_from_roots(&[&base, &overlay]).expect("layered tags load");
    assert_eq!(
        tags.members("testns:widgets"),
        Some(["testns:gamma".to_string()].as_slice()),
        "replace: true discards earlier values"
    );
    assert!(tags.contains("testns:parent", "testns:gamma"));
    assert!(!tags.contains("testns:parent", "testns:alpha"));
    assert!(tags.contains("testns:merged", "testns:one"));
    assert!(tags.contains("testns:merged", "testns:two"));
    assert_eq!(tags.members("testns:merged").expect("merged tag").len(), 2);

    let invalid = scratch_root("item-tags-bad-replace");
    let bad_file = invalid.join("data/testns/tags/item/bad.json");
    write(&bad_file, r#"{"replace": "yes", "values": []}"#);
    let err = ItemTags::load(&invalid).unwrap_err().to_string();
    assert!(err.contains("bad.json"), "error names the file: {err}");

    fs::remove_dir_all(base).expect("cleanup");
    fs::remove_dir_all(overlay).expect("cleanup");
    fs::remove_dir_all(invalid).expect("cleanup");
}

/// Run with:
/// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_item_tags_pin_vanilla_wood_counts`
#[test]
#[ignore = "requires operator-provisioned local data"]
fn pack_item_tags_pin_vanilla_wood_counts() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
    let tags = ItemTags::load(&root).expect("the provisioned item tags load");
    assert_eq!(
        tags.members("minecraft:logs_that_burn")
            .expect("logs_that_burn")
            .len(),
        40
    );
    assert_eq!(tags.members("minecraft:planks").expect("planks").len(), 13);
    assert_eq!(
        tags.members("minecraft:wooden_slabs")
            .expect("wooden_slabs")
            .len(),
        13
    );
    assert_eq!(
        tags.members("minecraft:oak_logs").expect("oak_logs").len(),
        4
    );
    assert!(tags.contains("minecraft:oak_logs", "minecraft:stripped_oak_wood"));
}

#[test]
fn enchantment_tags_resolve_nested_directories_and_replace() {
    let base = scratch_root("ench-tags-base");
    let overlay = scratch_root("ench-tags-overlay");

    write(
        &base.join("data/testns/tags/enchantment/exclusive_set/weapon.json"),
        r#"{"values": ["testns:alpha", "testns:beta"]}"#,
    );
    write(
        &base.join("data/testns/tags/enchantment/group.json"),
        r##"{"values": ["#testns:exclusive_set/weapon", "testns:gamma"]}"##,
    );
    write(
        &overlay.join("data/testns/tags/enchantment/group.json"),
        r#"{"replace": true, "values": ["testns:delta"]}"#,
    );

    let base_tags = EnchantmentTags::load(&base).expect("base enchantment tags load");
    assert!(base_tags.contains("testns:exclusive_set/weapon", "testns:alpha"));
    assert!(base_tags.contains("testns:group", "testns:beta"));
    assert!(base_tags.contains("testns:group", "testns:gamma"));
    assert_eq!(base_tags.members("testns:absent"), None);

    let merged = EnchantmentTags::load_from_roots(&[&base, &overlay]).expect("layered tags load");
    assert_eq!(
        merged.members("testns:group"),
        Some(&["testns:delta".to_string()][..])
    );

    let empty = scratch_root("ench-tags-empty");
    let empty_tags = EnchantmentTags::load(&empty).expect("pack without enchantment tags loads");
    assert!(empty_tags.is_empty());

    fs::remove_dir_all(base).expect("cleanup");
    fs::remove_dir_all(overlay).expect("cleanup");
    fs::remove_dir_all(empty).expect("cleanup");
}

/// Run with:
/// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_enchantment_tags_pin_vanilla_counts`
#[test]
#[ignore = "requires operator-provisioned local data"]
fn pack_enchantment_tags_pin_vanilla_counts() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
    let tags = EnchantmentTags::load(&root).expect("the provisioned enchantment tags load");
    assert!(tags.contains("minecraft:exclusive_set/damage", "minecraft:sharpness"));
    assert!(tags.contains("minecraft:exclusive_set/damage", "minecraft:smite"));
    assert!(tags.contains("minecraft:exclusive_set/bow", "minecraft:infinity"));
    assert!(tags.contains("minecraft:exclusive_set/bow", "minecraft:mending"));
    assert!(tags.contains("minecraft:double_trade_price", "minecraft:mending"));
    assert!(!tags.contains("minecraft:double_trade_price", "minecraft:sharpness"));
}

/// A hand-built decorating pack: two ores and an unmodeled tree, placed
/// features that reference them, nested block tags, and two biomes whose
/// positional `features` arrays the caller supplies.
fn schedule_pack(label: &str, biome_one: &str, biome_two: &str) -> PathBuf {
    let root = scratch_root(label);
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("feature/granite.json"),
        r#"{"type": "minecraft:ore", "size": 64, "discard_chance_on_air_exposure": 0.0,
            "targets": [{"state": "minecraft:granite",
                "target": {"predicate_type": "minecraft:tag_match", "tag": "testns:base_stone"}}]}"#,
    );
    write(
        &worldgen.join("feature/tuff.json"),
        r#"{"type": "minecraft:ore", "size": 64, "discard_chance_on_air_exposure": 0.0,
            "targets": [{"state": "minecraft:tuff",
                "target": {"predicate_type": "minecraft:block_match", "block": "minecraft:stone"}}]}"#,
    );
    write(
        &worldgen.join("feature/swamp.json"),
        r#"{"type": "minecraft:tree"}"#,
    );
    write(
        &root.join("data/testns/tags/block/base_stone.json"),
        r#"{"values": ["minecraft:stone", {"tag": "testns:deep_stone"}]}"#,
    );
    write(
        &root.join("data/testns/tags/block/deep_stone.json"),
        r#"{"values": ["minecraft:dirt"]}"#,
    );
    write(
        &worldgen.join("placed_feature/ore_a.json"),
        r#"{"feature": "testns:granite", "placement": [
            {"type": "minecraft:count", "count": 2},
            {"type": "minecraft:height_range", "height": {"type": "minecraft:constant",
                "value": {"absolute": 60}}},
            {"type": "minecraft:biome"}
        ]}"#,
    );
    write(
        &worldgen.join("placed_feature/ore_b.json"),
        r#"{"feature": "testns:tuff", "placement": [
            {"type": "minecraft:count", "count": 2},
            {"type": "minecraft:height_range", "height": {"type": "minecraft:constant",
                "value": {"absolute": 60}}}
        ]}"#,
    );
    write(
        &worldgen.join("placed_feature/aa_forest.json"),
        r#"{"feature": "testns:swamp", "placement": [{"type": "minecraft:biome"}]}"#,
    );
    write(
        &worldgen.join("biome/one.json"),
        &format!(r#"{{"features": {biome_one}}}"#),
    );
    write(
        &worldgen.join("biome/two.json"),
        &format!(r#"{{"features": {biome_two}}}"#),
    );
    root
}

#[test]
fn the_step_schedule_orders_the_biome_chains_into_step_schedules() {
    // Biome `one` lists ore_b and the tree at step 0 and ore_a at step 2;
    // biome `two` lists ore_a at step 0 and ore_b at step 3. Neither
    // contradicts the other, so the walk has one answer: every biome's
    // own relative order holds, across the step boundaries too.
    let root = schedule_pack(
        "schedule",
        r#"[["testns:ore_b", "testns:aa_forest"], [], ["testns:ore_a"]]"#,
        r#"[["testns:ore_a"], [], [], ["testns:ore_b"]]"#,
    );
    let data = FeatureData::load(&root).expect("loads");
    let ids = |step: usize| -> Vec<&str> {
        data.step(step)
            .unwrap_or_else(|| panic!("step {step}"))
            .iter()
            .map(|feature| feature.id.as_str())
            .collect()
    };
    assert_eq!(
        ids(0),
        ["testns:ore_a", "testns:ore_b", "testns:aa_forest"],
        "the step order comes from the two chains, not from the identifiers"
    );
    assert!(
        !data.step(0).expect("step 0")[2].is_modelable(),
        "the tree holds its slot without being modeled"
    );
    assert_eq!(
        data.decorated_steps(),
        [0, 2, 3],
        "an empty step carries nothing and the schedule is as wide as the widest biome"
    );
    assert_eq!(ids(2), ["testns:ore_a"]);
    assert_eq!(ids(3), ["testns:ore_b"]);
    // A feature listed at two steps is numbered separately in each, and
    // the pair of step and ordinal is its random identity.
    assert_eq!(
        data.steps[0].ordinal_of("testns:ore_b"),
        Some(1),
        "ore_b is second in step 0's schedule"
    );
    assert_eq!(
        data.steps[3].ordinal_of("testns:ore_b"),
        Some(0),
        "and first in step 3's"
    );

    // The ordinals a chunk runs come from the biomes in its neighbourhood,
    // not from the biome a candidate position lands in.
    let one = "testns:one".to_string();
    let two = "testns:two".to_string();
    let only_one = vec![one.clone()];
    let only_two = vec![two.clone()];
    let both = vec![two.clone(), one.clone()];
    assert_eq!(data.region_ordinals(0, &only_one), vec![1, 2]);
    assert_eq!(data.region_ordinals(0, &only_two), vec![0]);
    assert_eq!(data.region_ordinals(0, &both), vec![0, 1, 2]);
    assert_eq!(data.region_ordinals(2, &only_one), vec![0]);
    assert_eq!(data.region_ordinals(3, &only_two), vec![0]);
    assert!(data.region_ordinals(3, &only_one).is_empty());
    assert!(data.region_ordinals(1, &only_two).is_empty());

    assert!(data.biome_lists("testns:one", "testns:ore_a"));
    assert!(
        data.biome_lists("testns:two", "testns:ore_b"),
        "membership is flat over the biome's whole positional array"
    );
    assert!(
        !data.biome_lists("testns:two", "testns:aa_forest"),
        "the second biome never lists the tree"
    );
    assert_eq!(
        data.unimplemented_features.get("minecraft:tree"),
        Some(&1),
        "the startup report names what it skipped"
    );
    assert!(data.tags().contains("testns:base_stone", "minecraft:dirt"));

    // And the runtime decorates from it: the granite ore needs the tag,
    // the biome filter needs the membership, and a region that leaves an
    // ordinal out runs nothing at all.
    let both = vec![one, two];
    let mut plains = TestTarget::new()
        .anchored_at(0, 0)
        .in_biome(Some("testns:one"));
    data.decorate_step(&mut plains, 0, 0, 0, 2026, &both);
    assert!(plains.blocks("minecraft:granite") > 0, "tag_match fired");
    assert!(plains.blocks("minecraft:tuff") > 0, "block_match fired");
    let mut desert = TestTarget::new()
        .anchored_at(0, 0)
        .in_biome(Some("minecraft:desert"));
    data.decorate_step(&mut desert, 0, 0, 0, 2026, &both);
    assert_eq!(
        desert.blocks("minecraft:granite"),
        0,
        "the biome filter gated it"
    );
    assert!(
        desert.blocks("minecraft:tuff") > 0,
        "ore_b has no biome filter"
    );
    let mut single = TestTarget::new()
        .anchored_at(0, 0)
        .in_biome(Some("testns:one"));
    data.decorate_step(&mut single, 0, 0, 0, 2026, &only_two);
    assert!(
        single.blocks("minecraft:granite") > 0,
        "the one ordinal that region contributes is ore_a"
    );
    assert_eq!(
        single.blocks("minecraft:tuff"),
        0,
        "ore_b is not listed at step 0 by that biome"
    );

    let empty = scratch_root("no-features");
    assert!(FeatureData::load(&empty).expect("loads").is_empty());
    fs::remove_dir_all(root).expect("cleanup");
    fs::remove_dir_all(empty).expect("cleanup");
}

/// Running one ordinal alone is the same as running it inside its step,
/// because each feature reseeds from the chunk's decoration seed with its
/// own ordinal and step and never borrows a neighbour's stream. This is
/// what lets a measurement attribute a written block to one feature.
#[test]
fn one_ordinal_alone_reproduces_its_share_of_the_step() {
    let root = schedule_pack(
        "attribution",
        r#"[["testns:ore_b", "testns:aa_forest"], [], ["testns:ore_a"]]"#,
        r#"[["testns:ore_a"], [], [], ["testns:ore_b"]]"#,
    );
    let data = FeatureData::load(&root).expect("loads");
    let both = vec!["testns:one".to_string(), "testns:two".to_string()];

    let mut step = TestTarget::new()
        .anchored_at(0, 0)
        .in_biome(Some("testns:one"));
    data.decorate_step(&mut step, 0, 0, 0, 2026, &both);
    let mut granite = TestTarget::new()
        .anchored_at(0, 0)
        .in_biome(Some("testns:one"));
    let granite_draws = data.decorate_ordinal(&mut granite, 0, 0, 0, 0, 2026);
    let mut tuff = TestTarget::new()
        .anchored_at(0, 0)
        .in_biome(Some("testns:one"));
    let tuff_draws = data.decorate_ordinal(&mut tuff, 0, 1, 0, 0, 2026);
    assert!(
        granite_draws > 0 && tuff_draws > 0,
        "both veins place when run alone"
    );

    // The step runs its ordinals ascending, so a position the granite
    // ordinal reached keeps granite: the later tuff ordinal reads stone
    // there, fails its own target test and does not overwrite it.
    let mut expected = granite.written.clone();
    for (position, name) in &tuff.written {
        expected.entry(*position).or_insert_with(|| name.clone());
    }
    assert_eq!(
        step.written, expected,
        "each ordinal draws alone exactly as it draws inside its step"
    );

    let mut unmodeled = TestTarget::new()
        .anchored_at(0, 0)
        .in_biome(Some("testns:one"));
    assert_eq!(
        data.decorate_ordinal(&mut unmodeled, 0, 2, 0, 0, 2026),
        0,
        "the tree keeps its slot in the schedule but places nothing here"
    );

    let mut past_end = TestTarget::new().anchored_at(0, 0);
    assert_eq!(
        data.decorate_ordinal(&mut past_end, 0, 9, 0, 0, 2026),
        0,
        "an ordinal past the step's schedule holds nothing"
    );
    let mut no_step = TestTarget::new().anchored_at(0, 0);
    assert_eq!(
        data.decorate_ordinal(&mut no_step, 9, 0, 0, 0, 2026),
        0,
        "a step the schedule never built contributes nothing"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

/// Two biomes that list the same pair of features in opposite orders give
/// the ordering nothing consistent to build: the reference runtime drops
/// one biome at a time, in source order, and rebuilds from scratch.
#[test]
fn the_step_schedule_recovers_from_a_biome_order_cycle() {
    let root = schedule_pack(
        "cycle",
        r#"[["testns:ore_b", "testns:aa_forest", "testns:ore_a"], [], ["testns:ore_a"]]"#,
        r#"[["testns:ore_a", "testns:ore_b"]]"#,
    );
    let data = FeatureData::load(&root).expect("loads");
    let ids: Vec<&str> = data
        .step(0)
        .expect("step 0")
        .iter()
        .map(|feature| feature.id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["testns:ore_a", "testns:ore_b"],
        "the surviving biome's own order is all that is left"
    );
    assert_eq!(
        data.decorated_steps(),
        [0],
        "the dropped biome was the only one listing steps 1 and 2"
    );
    assert_eq!(
        data.unimplemented_features.get("feature_order_cycle"),
        Some(&1),
        "the recovery is reported, not silent"
    );
    let one = "testns:one".to_string();
    let two = "testns:two".to_string();
    assert!(
        data.region_ordinals(0, &[one]).is_empty(),
        "a biome the schedule dropped contributes nothing"
    );
    assert_eq!(data.region_ordinals(0, &[two]), vec![0, 1]);
    assert!(
        data.biome_lists("testns:one", "testns:aa_forest"),
        "membership is read from the pack, not from the surviving order"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

/// Two biomes that share no features give the ordering nothing to
/// interleave by, so which step-0 list comes first is decided by the
/// source's own declaration order.
#[test]
fn incomparable_biomes_are_ordered_by_the_biome_source() {
    let ore = stone_replacement("minecraft:granite", 8, 0.0);
    let a = wrapper("testns:ore_a", &ore, Vec::new());
    let b = wrapper("testns:ore_b", &ore, Vec::new());
    let mut data = FeatureData {
        biomes: HashMap::from([
            ("testns:one".to_string(), vec![vec![Rc::clone(&a)]]),
            ("testns:two".to_string(), vec![vec![Rc::clone(&b)]]),
        ]),
        ..Default::default()
    };
    data.set_biome_order(&["testns:one", "testns:two"]);
    // The walk emits a node only after everything it points to, and the
    // schedule is the reversal of that emission, so the biome listed
    // later by the source ends up with the lower ordinal.
    assert_eq!(step_ids(&data, 0), ["testns:ore_b", "testns:ore_a"]);
    let one = "testns:one".to_string();
    let only_one = vec![one];
    assert_eq!(data.region_ordinals(0, &only_one), vec![1]);
    data.set_biome_order(&["testns:two", "testns:one"]);
    assert_eq!(step_ids(&data, 0), ["testns:ore_a", "testns:ore_b"]);
    assert_eq!(data.region_ordinals(0, &only_one), vec![0]);
}

/// Open ground for the patch tests: `grass_block` up to `top`, air above,
/// inside chunk (0, 0). Beyond the chunk `block_at` has no answer, as the
/// generator's own view behaves, and `read_block` reports `halo`.
struct OpenGround {
    top: i32,
    halo: &'static str,
    written: HashMap<(i32, i32, i32), String>,
}

impl OpenGround {
    fn new(top: i32) -> Self {
        Self {
            top,
            halo: "minecraft:stone",
            written: HashMap::new(),
        }
    }
}

impl DecorationTarget for OpenGround {
    fn min_y(&self) -> i32 {
        0
    }

    fn height(&self) -> i32 {
        128
    }

    fn writable(&self, x: i32, y: i32, z: i32) -> bool {
        (x >> 4, z >> 4) == (0, 0) && (0..128).contains(&y)
    }

    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<&str> {
        if !self.writable(x, y, z) {
            return None;
        }
        if let Some(name) = self.written.get(&(x, y, z)) {
            return Some(name);
        }
        Some(if y <= self.top {
            "minecraft:grass_block"
        } else {
            "minecraft:air"
        })
    }

    fn read_block(&self, x: i32, y: i32, z: i32) -> Option<Cow<'_, str>> {
        if !(0..128).contains(&y) {
            return None;
        }
        match self.block_at(x, y, z) {
            Some(name) => Some(Cow::Borrowed(name)),
            None => Some(Cow::Borrowed(self.halo)),
        }
    }

    fn is_air(&self, x: i32, y: i32, z: i32) -> bool {
        self.block_at(x, y, z) == Some("minecraft:air")
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, name: &str) {
        self.written.insert((x, y, z), name.to_string());
    }

    fn biome_at(&self, _x: i32, _y: i32, _z: i32) -> Option<String> {
        None
    }

    fn ocean_floor_height(&self, _x: i32, _z: i32) -> i32 {
        self.top
    }
}

fn vegetation_tags() -> BlockTags {
    BlockTags {
        entries: HashMap::from([
            (
                "minecraft:supports_vegetation".to_string(),
                vec!["minecraft:grass_block".to_string()],
            ),
            (
                "minecraft:air".to_string(),
                vec!["minecraft:air".to_string()],
            ),
        ]),
        membership: HashMap::new(),
    }
}

fn data_with_vegetation_tags() -> FeatureData {
    FeatureData {
        tags: vegetation_tags(),
        ..FeatureData::default()
    }
}

fn simple_block(state: &str) -> Rc<ConfiguredFeature> {
    Rc::new(ConfiguredFeature::SimpleBlock {
        to_place: StateProvider::Constant {
            state: state.to_string(),
        },
    })
}

fn placed(feature: Rc<ConfiguredFeature>, modifiers: Vec<PlacementModifier>) -> PlacedFeature {
    PlacedFeature {
        id: "test:patch".to_string(),
        feature,
        modifiers,
    }
}

fn patch(tries: i32, xz_spread: i32, y_spread: i32, child: PlacedFeature) -> PlacedFeature {
    placed(
        Rc::new(ConfiguredFeature::RandomPatch {
            tries,
            xz_spread,
            y_spread,
            feature: Box::new(child),
        }),
        Vec::new(),
    )
}

#[test]
fn flower_and_random_patch_share_one_configuration() {
    for kind in ["minecraft:random_patch", "minecraft:flower"] {
        let document = json!({
            "type": kind,
            "tries": 96,
            "xz_spread": 7,
            "y_spread": 3,
            "feature": {
                "feature": {
                    "type": "minecraft:simple_block",
                    "to_place": {"id": "minecraft:poppy"}
                },
                "placement": [{
                    "type": "minecraft:block_predicate_filter",
                    "predicate": {"type": "minecraft:matching_block_tag", "tag": "minecraft:air"}
                }]
            }
        });
        let feature = parse_configured_feature(&document).expect("parses");
        let ConfiguredFeature::RandomPatch {
            tries,
            xz_spread,
            y_spread,
            feature: child,
        } = &feature
        else {
            panic!("{kind} parsed as {feature:?}");
        };
        assert_eq!((*tries, *xz_spread, *y_spread), (96, 7, 3));
        assert!(matches!(
            *child.feature,
            ConfiguredFeature::SimpleBlock { .. }
        ));
        assert!(feature.is_supported(), "{kind} is modeled");
    }
}

#[test]
fn a_patch_field_without_a_source_or_over_budget_is_a_gap_not_a_clamp() {
    let child = json!({"type": "minecraft:simple_block", "to_place": {"id": "minecraft:poppy"}});
    for (document, gap) in [
        (
            json!({"type": "minecraft:random_patch", "xz_spread": 7, "y_spread": 3, "feature": child}),
            "minecraft:random_patch:tries",
        ),
        (
            json!({"type": "minecraft:random_patch", "tries": 4294967297_i64, "xz_spread": 7,
                "y_spread": 3, "feature": child}),
            "minecraft:random_patch:tries",
        ),
        (
            json!({"type": "minecraft:random_patch", "tries": 1, "xz_spread": 17, "y_spread": 3,
                "feature": child}),
            "minecraft:random_patch:xz_spread",
        ),
        (
            json!({"type": "minecraft:flower", "tries": 1, "xz_spread": 7, "y_spread": -1,
                "feature": child}),
            "minecraft:flower:y_spread",
        ),
        (
            json!({"type": "minecraft:flower", "tries": 1, "xz_spread": 7, "y_spread": 3}),
            "minecraft:flower:feature",
        ),
    ] {
        assert_eq!(
            parse_configured_feature(&document).expect("a gap does not abort the pack"),
            ConfiguredFeature::Unsupported {
                kind: gap.to_string()
            }
        );
    }
}

#[test]
fn nested_patches_beyond_the_replay_window_stay_unmodeled() {
    let inner = |spread: i32| {
        patch(
            1,
            spread,
            0,
            placed(simple_block("minecraft:poppy"), vec![]),
        )
    };
    let within = patch(1, 8, 0, inner(8));
    assert!(
        within.is_modelable(),
        "8 + 8 reaches the last replayed column"
    );
    let beyond = patch(1, 8, 0, inner(9));
    assert!(
        !beyond.is_modelable(),
        "8 + 9 reaches past the replay window"
    );
    let mut counts = HashMap::new();
    record_unmodeled_configured(&mut counts, &beyond.feature);
    assert_eq!(counts.get("minecraft:random_patch:reach"), Some(&1));
}

#[test]
fn an_unsupported_state_provider_keeps_the_pack_loading_and_names_the_gap() {
    let root = scratch_root("gap-provider");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("feature/noise_flowers.json"),
        r#"{"type": "minecraft:simple_block", "to_place": {"type": "minecraft:noise_provider"}}"#,
    );
    write(
        &worldgen.join("feature/weighted_flowers.json"),
        r#"{"type": "minecraft:simple_block", "to_place": {"type": "minecraft:weighted",
            "entries": [{"data": "minecraft:poppy", "weight": 2},
                {"data": "minecraft:brown_mushroom", "weight": 1}]}}"#,
    );
    write(
        &worldgen.join("placed_feature/noise_flowers.json"),
        r#"{"feature": "testns:noise_flowers", "placement": []}"#,
    );
    write(
        &worldgen.join("biome/plains.json"),
        r#"{"features": [["testns:noise_flowers"]]}"#,
    );
    let data = FeatureData::load(&root).expect("an unmodeled provider does not abort loading");
    assert_eq!(
        data.unimplemented_features
            .get("provider:minecraft:noise_provider"),
        Some(&1)
    );
    assert_eq!(
        data.unimplemented_features
            .get("state:minecraft:brown_mushroom"),
        Some(&1),
        "a state with no ground rule gates its feature by name"
    );
    assert!(!data.step(0).expect("step 0")[0].is_modelable());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn block_predicate_combinators_read_their_own_field_names() {
    let filter = |predicate: Value| {
        parse_placement_modifier(
            "test",
            0,
            &json!({"type": "minecraft:block_predicate_filter", "predicate": predicate}),
        )
        .expect("parses")
    };
    let stone = json!({"type": "minecraft:matching_blocks", "blocks": "minecraft:stone"});
    let air = json!({"type": "minecraft:matching_block_tag", "tag": "minecraft:air"});
    let tags = vegetation_tags();
    let at_air = DecorationPos { x: 5, y: 70, z: 5 };
    let target = OpenGround::new(63);
    let check = |modifier: &PlacementModifier| {
        let PlacementModifier::BlockPredicateFilter { predicate } = modifier else {
            panic!("expected a predicate filter, got {modifier:?}");
        };
        assert!(predicate.is_modelable());
        predicate.test(&target, at_air, &tags)
    };
    assert!(!check(&filter(json!({
        "type": "minecraft:all_of", "predicates": [stone.clone()]
    }))));
    assert!(check(&filter(json!({
        "type": "minecraft:any_of", "predicates": [stone.clone(), air.clone()]
    }))));
    assert!(check(&filter(json!({
        "type": "minecraft:not", "predicate": stone
    }))));
    assert!(!check(&filter(json!({
        "type": "minecraft:not", "predicate": air
    }))));
}

#[test]
fn a_predicate_offset_across_the_border_reads_the_neighbouring_terrain() {
    let air = BlockPredicate::Blocks {
        blocks: vec!["minecraft:air".to_string()],
        offset: (1, 0, 0),
    };
    let stone = BlockPredicate::Blocks {
        blocks: vec!["minecraft:stone".to_string()],
        offset: (1, 0, 0),
    };
    let tags = vegetation_tags();
    let edge = DecorationPos { x: 15, y: 70, z: 5 };
    let target = OpenGround::new(63);
    assert!(
        stone.test(&target, edge, &tags),
        "the neighbour holds stone"
    );
    assert!(!air.test(&target, edge, &tags), "and so is not air");
}

/// The expected scatter goes through the pack's own form instead of the
/// patch loop: `minecraft:trapezoid` offsets with no plateau, x then y then z.
#[test]
fn a_patch_draws_two_values_per_axis_in_x_y_z_order() {
    let (xz, y) = (7, 3);
    let tries = 12;
    let origin = DecorationPos { x: 8, y: 80, z: 8 };
    let trapezoid = |reach: i32| RangeProvider::Trapezoid {
        min_inclusive: VerticalAnchor::Absolute(-reach),
        max_inclusive: VerticalAnchor::Absolute(reach),
    };
    let mut expected = DecorationRandom::new(42);
    let mut positions = Vec::new();
    for _ in 0..tries {
        let dx = trapezoid(xz).sample(0, 128, &mut expected);
        let dy = trapezoid(y).sample(0, 128, &mut expected);
        let dz = trapezoid(xz).sample(0, 128, &mut expected);
        positions.push((origin.x + dx, origin.y + dy, origin.z + dz));
    }
    let after_patch = expected.next_int_bounded(1_000_000);

    let data = FeatureData::default();
    let mut target = TestTarget::new().write_anywhere();
    let mut random = DecorationRandom::new(42);
    let scatter = patch(
        tries,
        xz,
        y,
        placed(simple_block("minecraft:pumpkin"), vec![]),
    );
    target.base = "minecraft:air".to_string();
    data.run_chain(&mut target, &scatter, 0, origin, &mut random);
    let mut written: Vec<(i32, i32, i32)> = target.written.keys().copied().collect();
    written.sort_unstable();
    positions.sort_unstable();
    positions.dedup();
    assert_eq!(written, positions);
    assert_eq!(
        random.next_int_bounded(1_000_000),
        after_patch,
        "the patch consumed exactly two draws per axis per try"
    );
}

#[test]
fn a_zero_spread_still_consumes_its_draws() {
    let mut expected = DecorationRandom::new(7);
    for _ in 0..6 {
        expected.next_int_bounded(1);
    }
    let after = expected.next_int_bounded(1_000_000);
    let data = FeatureData::default();
    let mut target = TestTarget::new().write_anywhere();
    target.base = "minecraft:air".to_string();
    let mut random = DecorationRandom::new(7);
    let scatter = patch(1, 0, 0, placed(simple_block("minecraft:pumpkin"), vec![]));
    let origin = DecorationPos { x: 3, y: 9, z: 3 };
    assert_eq!(
        data.run_chain(&mut target, &scatter, 0, origin, &mut random),
        1
    );
    assert_eq!(random.next_int_bounded(1_000_000), after);
}

#[test]
fn a_plant_needs_its_ground_directly_below() {
    let data = data_with_vegetation_tags();
    let poppy = placed(simple_block("minecraft:poppy"), vec![]);
    let mut random = DecorationRandom::new(1);
    let mut target = OpenGround::new(63);
    let floating = DecorationPos { x: 4, y: 66, z: 4 };
    assert_eq!(
        data.run_chain(&mut target, &poppy, 0, floating, &mut random),
        0
    );
    let grounded = DecorationPos { x: 4, y: 64, z: 4 };
    assert_eq!(
        data.run_chain(&mut target, &poppy, 0, grounded, &mut random),
        1
    );
    assert_eq!(
        target.written.get(&(4, 64, 4)).map(String::as_str),
        Some("minecraft:poppy")
    );
}

#[test]
fn a_two_row_plant_writes_both_halves_or_neither() {
    let data = data_with_vegetation_tags();
    let tall = placed(simple_block("minecraft:tall_grass[half=lower]"), vec![]);
    let mut random = DecorationRandom::new(1);

    let mut target = OpenGround::new(63);
    let spot = DecorationPos { x: 4, y: 64, z: 4 };
    assert_eq!(data.run_chain(&mut target, &tall, 0, spot, &mut random), 2);
    assert_eq!(
        target.written.get(&(4, 65, 4)).map(String::as_str),
        Some("minecraft:tall_grass[half=upper]")
    );

    let mut roofed = OpenGround::new(63);
    roofed.set_block(4, 65, 4, "minecraft:stone");
    assert_eq!(data.run_chain(&mut roofed, &tall, 0, spot, &mut random), 0);
    assert_eq!(roofed.written.len(), 1, "no lower half under a roof");

    let mut ceiling = OpenGround::new(126);
    let top = DecorationPos { x: 4, y: 127, z: 4 };
    assert_eq!(data.run_chain(&mut ceiling, &tall, 0, top, &mut random), 0);
    assert!(
        ceiling.written.is_empty(),
        "no lower half at the build ceiling"
    );
}

/// The draw for the state happens before the chunk clip, so a chunk that
/// replays a neighbour's anchor stays in step with the chunk that owns it.
#[test]
fn a_candidate_outside_the_chunk_still_draws_its_state() {
    let weighted = Rc::new(ConfiguredFeature::SimpleBlock {
        to_place: StateProvider::Weighted {
            entries: vec![
                WeightedState {
                    state: "minecraft:poppy".to_string(),
                    weight: 2,
                },
                WeightedState {
                    state: "minecraft:dandelion".to_string(),
                    weight: 1,
                },
            ],
        },
    });
    let data = data_with_vegetation_tags();
    let mut random = DecorationRandom::new(11);
    let mut expected = DecorationRandom::new(11);
    expected.next_int_bounded(3);
    let after = expected.next_int_bounded(1_000_000);
    let mut target = OpenGround::new(63);
    let outside = DecorationPos { x: 40, y: 64, z: 4 };
    let chain = placed(weighted, vec![]);
    assert_eq!(
        data.run_chain(&mut target, &chain, 0, outside, &mut random),
        0
    );
    assert_eq!(random.next_int_bounded(1_000_000), after);
}

#[test]
fn test_random_patch_attempt_budget_and_nesting_limits() {
    let poppy = || json!({"type": "minecraft:simple_block", "to_place": {"id": "minecraft:poppy"}});
    let patch_document = |tries: i64, child: Value| {
        json!({"type": "minecraft:random_patch", "tries": tries, "xz_spread": 1,
            "y_spread": 0, "feature": child})
    };
    let mut configured = HashMap::new();
    configured.insert(
        "valid_patch".to_string(),
        Rc::new(parse_configured_feature(&patch_document(96, poppy())).unwrap()),
    );
    configured.insert(
        "huge_patch".to_string(),
        Rc::new(parse_configured_feature(&patch_document(256, poppy())).unwrap()),
    );

    let valid = json!({"feature": "valid_patch",
        "placement": [{"type": "minecraft:count", "count": 10}]});
    let placed_valid = parse_placed_feature("test:valid", &valid, &configured).unwrap();
    assert!(placed_valid.is_modelable());
    assert_eq!(placed_valid.validate_attempt_budget(0).unwrap(), 960);

    let huge = json!({"feature": "huge_patch",
        "placement": [{"type": "minecraft:count", "count": 256}]});
    let err = parse_placed_feature("test:huge", &huge, &configured).unwrap_err();
    assert!(
        err.to_string()
            .contains("exceed the 4096-attempt placement budget"),
        "Error was: {err}"
    );

    let nested = (0..3).fold(poppy(), |child, _| {
        patch_document(2, json!({"feature": child, "placement": []}))
    });
    let too_deep = patch_document(2, json!({"feature": nested, "placement": []}));
    let err = parse_configured_feature(&too_deep).unwrap_err();
    assert!(
        err.to_string().contains("nesting depth exceeds limit"),
        "Error was: {err}"
    );
}

#[test]
fn gap_ledger_reports_random_patch_without_false_unmodeled_drop() {
    let root = scratch_root("gap-random-patch");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("feature/patch_flowers.json"),
        r#"{
            "type": "minecraft:flower",
            "tries": 64,
            "xz_spread": 7,
            "y_spread": 3,
            "feature": {
                "feature": {"type": "minecraft:simple_block", "to_place": {"id": "minecraft:dandelion"}},
                "placement": [{"type": "minecraft:block_predicate_filter",
                    "predicate": {"type": "minecraft:matching_blocks", "blocks": ["minecraft:air"]}}]
            }
        }"#,
    );
    write(
        &worldgen.join("placed_feature/patch_flowers.json"),
        r#"{"feature": "testns:patch_flowers", "placement": [
            {"type": "minecraft:count", "count": 2},
            {"type": "minecraft:in_square"},
            {"type": "minecraft:biome"}
        ]}"#,
    );
    write(
        &worldgen.join("biome/plains.json"),
        r#"{"features": [["testns:patch_flowers"]]}"#,
    );

    let data = FeatureData::load(&root).expect("loads");
    assert!(
        data.unimplemented_features.is_empty(),
        "a fully modeled patch chain creates zero gap entries: {:?}",
        data.unimplemented_features
    );
    assert!(
        data.step(0).expect("step 0")[0].is_modelable(),
        "the placed patch is modelable"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn gap_ledger_reports_unmodeled_subfeature_inside_random_patch() {
    let root = scratch_root("gap-patch-unmodeled-child");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("feature/patch_tree.json"),
        r#"{
            "type": "minecraft:random_patch",
            "tries": 32,
            "xz_spread": 4,
            "y_spread": 2,
            "feature": {
                "feature": {
                    "type": "minecraft:tree"
                },
                "placement": []
            }
        }"#,
    );
    write(
        &worldgen.join("placed_feature/patch_tree.json"),
        r#"{"feature": "testns:patch_tree", "placement": []}"#,
    );
    write(
        &worldgen.join("biome/plains.json"),
        r#"{"features": [["testns:patch_tree"]]}"#,
    );

    let data = FeatureData::load(&root).expect("loads");
    assert_eq!(
        data.unimplemented_features.get("minecraft:tree"),
        Some(&1),
        "unmodeled child feature inside random_patch is reported"
    );
    assert!(
        !data
            .unimplemented_features
            .contains_key("minecraft:random_patch"),
        "random_patch container itself is modeled"
    );
    assert_eq!(data.unimplemented_features.len(), 1);
    assert!(
        !data.step(0).expect("step 0")[0].is_modelable(),
        "feature with unmodeled child is gated out"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

/// Requires the operator-provisioned data pack. The 26.3 pack writes no
/// `minecraft:random_patch` or `minecraft:flower` document: it spells
/// patches as `count` and `offset` on the placed feature, so this runtime
/// only serves operator packs and a real pack must still load through it.
#[test]
#[ignore = "requires operator-provisioned local data"]
fn the_real_pack_loads_and_has_no_patch_documents() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let data = FeatureData::load(&root).expect("the provisioned pack loads");
    assert!(
        !data
            .configured
            .values()
            .any(|feature| matches!(**feature, ConfiguredFeature::RandomPatch { .. })),
        "the pack gained a patch document; its fields can now be pinned to it"
    );
    let grass = data.configured.get("minecraft:grass").expect("grass");
    assert!(
        grass.is_supported(),
        "the pack's own grass state is placeable"
    );
}

/// Requires the operator-provisioned data pack. Prints the gap ledger the
/// loader builds from it — the honest gap list, per key and per count;
/// this smoke, not the server, is what surfaces it today.
/// `FeatureData::load` has no dimension filter (the overworld
/// biome order only shapes the schedule), so the report covers every
/// placed and configured document in the pack. Run with
/// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib --
/// --ignored --nocapture`.
#[test]
#[ignore = "requires operator-provisioned local data"]
fn smoke_prints_the_pack_wide_gap_report() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let data = FeatureData::load(&root).expect("the provisioned pack loads");
    assert!(
        !data.unimplemented_features.is_empty(),
        "the provisioned pack asks for nothing this generator lacks"
    );
    let mut entries: Vec<(&String, &usize)> = data.unimplemented_features.iter().collect();
    entries.sort_unstable_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));
    let placements = entries
        .iter()
        .filter(|(key, _)| key.starts_with("placement:"))
        .count();
    println!(
        "gap report: {} keys ({} of them placement modifiers), root {}",
        entries.len(),
        placements,
        root.display()
    );
    for (key, count) in entries {
        println!("  {key} {count}");
    }
}

#[test]
fn test_disk_feature_placement() {
    // 1. Parsing official data pack disk schema
    let disk_doc = json!({
        "type": "minecraft:disk",
        "half_height": 1,
        "radius": {
            "type": "minecraft:uniform",
            "min_inclusive": 2,
            "max_inclusive": 2
        },
        "state_provider": {
            "id": "minecraft:clay"
        },
        "target": {
            "type": "minecraft:matching_blocks",
            "blocks": ["minecraft:dirt", "minecraft:clay"]
        }
    });
    let parsed = parse_configured_feature(&disk_doc).expect("parses disk feature");
    assert!(parsed.is_supported());
    let ConfiguredFeature::Disk {
        state_provider,
        target_predicate,
        radius,
        half_height,
    } = &parsed
    else {
        panic!("expected ConfiguredFeature::Disk");
    };
    assert_eq!(*half_height, 1);
    assert_eq!(
        *state_provider,
        StateProvider::Constant {
            state: "minecraft:clay".to_string()
        }
    );
    assert!(target_predicate.is_modelable());
    assert_eq!(
        *radius,
        RangeProvider::Uniform {
            min_inclusive: VerticalAnchor::Absolute(2),
            max_inclusive: VerticalAnchor::Absolute(2)
        }
    );

    // 2. Placing disk in world
    let mut target = TestTarget::new().write_anywhere();
    // Fill an 11x11x5 volume around (8, 64, 8) with dirt
    for y in 60..=68 {
        for x in 3..=13 {
            for z in 3..=13 {
                target.set_block(x, y, z, "minecraft:dirt");
            }
        }
    }
    // Place some stone inside the cylinder to ensure target predicate filters it out
    target.set_block(8, 64, 8, "minecraft:stone");

    let placed_feature = PlacedFeature {
        id: "test:disk".to_string(),
        feature: Rc::new(parsed),
        modifiers: vec![],
    };
    let data = FeatureData::default();
    let mut random = DecorationRandom::new(12345);
    let origin = DecorationPos { x: 8, y: 64, z: 8 };
    let written = data.run_chain(&mut target, &placed_feature, 0, origin, &mut random);

    // Cylinder radius=2 (sq=4), half_height=1:
    // dy in [-1, 0, 1] (3 layers).
    // On each layer, dx in [-2..=2], dz in [-2..=2] with dx*dx + dz*dz <= 4:
    // dx=0: dz in -2..=2 (5)
    // dx=1, -1: dz in -1..=1 (3 each => 6)
    // dx=2, -2: dz=0 (1 each => 2)
    // Total points per layer: 5 + 6 + 2 = 13 points.
    // For 3 layers: 13 * 3 = 39 candidate positions.
    // But (8, 64, 8) is stone, so it is not replaced!
    // Expected placed: 39 - 1 = 38!
    assert_eq!(written, 38);
    assert_eq!(target.block_at(8, 64, 8), Some("minecraft:stone"));
    assert_eq!(target.block_at(8, 64, 9), Some("minecraft:clay"));
    assert_eq!(target.block_at(8, 64, 10), Some("minecraft:clay"));
    // (8, 64, 11) is dz=3 (outside radius=2), should still be dirt
    assert_eq!(target.block_at(8, 64, 11), Some("minecraft:dirt"));
    // y=66 is dy=2 (outside half_height=1), should still be dirt
    assert_eq!(target.block_at(8, 66, 8), Some("minecraft:dirt"));

    // 3. Rule-based disk placement
    let rule_disk_doc = json!({
        "type": "minecraft:disk",
        "half_height": 0,
        "radius": 1,
        "state_provider": {
            "type": "minecraft:rule_based",
            "fallback": { "id": "minecraft:sand" },
            "rules": [{
                "if_true": {
                    "type": "minecraft:matching_blocks",
                    "blocks": ["minecraft:air"],
                    "offset": [0, -1, 0]
                },
                "then": { "id": "minecraft:sandstone" }
            }]
        },
        "target": {
            "type": "minecraft:matching_blocks",
            "blocks": ["minecraft:dirt"]
        }
    });
    let rule_feature = parse_configured_feature(&rule_disk_doc).expect("parses rule disk");
    let placed_rule = PlacedFeature {
        id: "test:rule_disk".to_string(),
        feature: Rc::new(rule_feature),
        modifiers: vec![],
    };
    // Setup target: (8, 64, 8) has air below it (8, 63, 8)
    target.set_block(8, 64, 8, "minecraft:dirt");
    target.set_block(8, 63, 8, "minecraft:air");
    // (9, 64, 8) has dirt below it (9, 63, 8)
    target.set_block(9, 64, 8, "minecraft:dirt");
    target.set_block(9, 63, 8, "minecraft:stone");

    let written_rule = data.run_chain(&mut target, &placed_rule, 0, origin, &mut random);
    assert!(written_rule > 0);
    // Position above air got sandstone:
    assert_eq!(target.block_at(8, 64, 8), Some("minecraft:sandstone"));
    // Position above stone got sand:
    assert_eq!(target.block_at(9, 64, 8), Some("minecraft:sand"));
}

#[test]
fn test_spring_feature_placement() {
    // 1. Parsing official data pack spring schema
    let spring_doc = json!({
        "type": "minecraft:spring_feature",
        "state": {
            "id": "minecraft:water",
            "properties": {
                "falling": "true"
            }
        },
        "valid_blocks": [
            "minecraft:stone",
            "minecraft:granite"
        ],
        "rock_count": 4,
        "hole_count": 1,
        "requires_block_below": true
    });
    let parsed = parse_configured_feature(&spring_doc).expect("parses spring feature");
    assert!(parsed.is_supported());
    let ConfiguredFeature::Spring {
        state,
        rock_count,
        hole_count,
        requires_block_below,
        valid_blocks,
    } = &parsed
    else {
        panic!("expected ConfiguredFeature::Spring");
    };
    assert_eq!(
        state, "minecraft:water",
        "the fluid state becomes its block state"
    );
    assert_eq!(*rock_count, 4);
    assert_eq!(*hole_count, 1);
    assert!(*requires_block_below);
    assert_eq!(valid_blocks.len(), 2);

    let placed_spring = PlacedFeature {
        id: "test:spring".to_string(),
        feature: Rc::new(parsed),
        modifiers: vec![],
    };
    let data = FeatureData::default();
    let mut random = DecorationRandom::new(999);
    let origin = DecorationPos { x: 8, y: 64, z: 8 };
    let pocket = |sides: [&str; 4], above: &str, below: &str| {
        let mut target = TestTarget::new().write_anywhere();
        target.set_block(8, 64, 8, "minecraft:air");
        target.set_block(8, 65, 8, above);
        target.set_block(8, 63, 8, below);
        target.set_block(7, 64, 8, sides[0]);
        target.set_block(9, 64, 8, sides[1]);
        target.set_block(8, 64, 7, sides[2]);
        target.set_block(8, 64, 9, sides[3]);
        target
    };
    let stone = "minecraft:stone";
    let air = "minecraft:air";
    let obsidian = "minecraft:obsidian";
    let mut run = |mut target: TestTarget| {
        data.run_chain(&mut target, &placed_spring, 0, origin, &mut random)
    };

    // Three stone sides, the block below, and one opening.
    assert_eq!(run(pocket([air, stone, stone, stone], stone, stone)), 1);
    // A granite side counts through `valid_blocks`.
    assert_eq!(
        run(pocket(
            [air, "minecraft:granite", stone, stone],
            stone,
            stone
        )),
        1
    );
    // The ceiling is a gate, not a count: open sky never passes, even
    // though the five counted cells are exactly four rocks and one hole.
    assert_eq!(run(pocket([air, stone, stone, stone], air, stone)), 0);
    // A foreign ceiling fails the same way.
    assert_eq!(run(pocket([air, stone, stone, stone], obsidian, stone)), 0);
    // Only three valid cells below the ceiling: the ceiling does not top
    // the count back up to four.
    assert_eq!(run(pocket([air, obsidian, stone, stone], stone, stone)), 0);
    // Counts are exact in both directions.
    assert_eq!(run(pocket([stone, stone, stone, stone], stone, stone)), 0);
    assert_eq!(run(pocket([air, air, stone, stone], stone, stone)), 0);
    // The block below is required by default.
    assert_eq!(run(pocket([air, stone, stone, stone], stone, air)), 0);
}

#[test]
fn a_spring_state_is_its_block_state_in_the_block_registry() {
    let doc = |id: &str| {
        json!({"type": "minecraft:spring_feature", "valid_blocks": "minecraft:stone",
            "state": {"id": id, "properties": {"falling": "true"}}})
    };
    let state_of = |id: &str| match parse_configured_feature(&doc(id)).expect("parses") {
        ConfiguredFeature::Spring { state, .. } => state,
        other => panic!("expected a spring, got {other:?}"),
    };
    assert_eq!(state_of("minecraft:water"), "minecraft:water");
    assert_eq!(state_of("minecraft:lava"), "minecraft:lava");
    assert_eq!(
        parse_configured_feature(&doc("minecraft:flowing_water")).expect("parses"),
        ConfiguredFeature::Unsupported {
            kind: "minecraft:spring_feature:state".to_string()
        }
    );
}

#[test]
fn spring_counts_outside_the_five_cells_are_gaps_not_defaults() {
    let doc = json!({"type": "minecraft:spring_feature", "valid_blocks": "minecraft:stone",
        "state": {"id": "minecraft:water"}, "rock_count": 4294967297_i64});
    assert_eq!(
        parse_configured_feature(&doc).expect("parses"),
        ConfiguredFeature::Unsupported {
            kind: "minecraft:spring_feature:rock_count".to_string()
        }
    );
}

/// A chunk corner: `block_at` stops at the chunk edge as the generator's
/// own view does, and `read_block` answers past it.
struct CornerPocket;

impl DecorationTarget for CornerPocket {
    fn min_y(&self) -> i32 {
        0
    }
    fn height(&self) -> i32 {
        128
    }
    fn writable(&self, x: i32, _y: i32, z: i32) -> bool {
        (x >> 4, z >> 4) == (0, 0)
    }
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<&str> {
        if !self.writable(x, y, z) {
            return None;
        }
        Some(match (x, y, z) {
            (15, 64, 15) | (15, 64, 14) => "minecraft:air",
            _ => "minecraft:stone",
        })
    }
    fn read_block(&self, x: i32, y: i32, z: i32) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(
            self.block_at(x, y, z).unwrap_or("minecraft:stone"),
        ))
    }
    fn is_air(&self, x: i32, y: i32, z: i32) -> bool {
        self.block_at(x, y, z) == Some("minecraft:air")
    }
    fn set_block(&mut self, _x: i32, _y: i32, _z: i32, _name: &str) {}
    fn biome_at(&self, _x: i32, _y: i32, _z: i32) -> Option<String> {
        None
    }
    fn ocean_floor_height(&self, _x: i32, _z: i32) -> i32 {
        0
    }
}

#[test]
fn a_pocket_on_a_chunk_corner_reads_its_neighbours() {
    let valid = vec!["minecraft:stone".to_string()];
    let tags = BlockTags::default();
    let corner = DecorationPos {
        x: 15,
        y: 64,
        z: 15,
    };
    let placed = place_spring(
        &mut CornerPocket,
        &tags,
        SpringFeature {
            state: "minecraft:water",
            rock_count: 4,
            hole_count: 1,
            requires_block_below: true,
            valid_blocks: &valid,
        },
        corner,
    );
    assert_eq!(placed, 1, "east and south are stone in the next chunks");
}

fn disk_of(provider: Value, radius: Value, half_height: i32) -> PlacedFeature {
    let document = json!({"type": "minecraft:disk", "half_height": half_height,
        "radius": radius, "state_provider": provider,
        "target": {"type": "minecraft:matching_blocks", "blocks": ["minecraft:stone"]}});
    PlacedFeature {
        id: "test:disk".to_string(),
        feature: Rc::new(parse_configured_feature(&document).expect("parses")),
        modifiers: vec![],
    }
}

/// Stone below the candidate picks sand, anything else clay. Written in
/// column order, top first, every row still has the original stone under
/// it; a bottom-up walk would turn the upper rows to clay.
#[test]
fn a_disk_column_runs_top_down_before_the_next_column() {
    let provider = json!({"type": "minecraft:rule_based",
        "fallback": {"id": "minecraft:clay"},
        "rules": [{"if_true": {"type": "minecraft:matching_blocks",
            "blocks": ["minecraft:stone"], "offset": [0, -1, 0]},
            "then": {"id": "minecraft:sand"}}]});
    let disk = disk_of(provider, json!(0), 1);
    let mut target = TestTarget::new().write_anywhere();
    let data = FeatureData::default();
    let mut random = DecorationRandom::new(5);
    let origin = DecorationPos { x: 8, y: 64, z: 8 };
    assert_eq!(
        data.run_chain(&mut target, &disk, 0, origin, &mut random),
        3
    );
    for y in 63..=65 {
        assert_eq!(target.block_at(8, y, 8), Some("minecraft:sand"), "row {y}");
    }
}

#[test]
fn a_zero_radius_disk_fills_its_centre_column() {
    let disk = disk_of(json!({"id": "minecraft:clay"}), json!(0), 2);
    let mut target = TestTarget::new().write_anywhere();
    let data = FeatureData::default();
    let mut random = DecorationRandom::new(5);
    let origin = DecorationPos { x: 8, y: 64, z: 8 };
    assert_eq!(
        data.run_chain(&mut target, &disk, 0, origin, &mut random),
        5
    );
    assert_eq!(target.blocks("minecraft:clay"), 5);
    assert_eq!(target.block_at(9, 64, 8), Some("minecraft:stone"));
}

#[test]
fn a_radius_past_the_replay_window_is_a_gap() {
    let doc = |radius: Value| {
        json!({"type": "minecraft:disk", "half_height": 1, "radius": radius,
            "state_provider": {"id": "minecraft:clay"},
            "target": {"type": "minecraft:matching_blocks", "blocks": ["minecraft:stone"]}})
    };
    let gap = ConfiguredFeature::Unsupported {
        kind: "minecraft:disk:radius".to_string(),
    };
    assert_eq!(parse_configured_feature(&doc(json!(50_000))).unwrap(), gap);
    assert_eq!(
        parse_configured_feature(&doc(
            json!({"type": "minecraft:uniform", "min_inclusive": 2, "max_inclusive": 17})
        ))
        .unwrap(),
        gap
    );
    assert_eq!(parse_configured_feature(&doc(json!(-1))).unwrap(), gap);
    assert!(
        parse_configured_feature(&doc(json!(16)))
            .unwrap()
            .is_supported()
    );
}

/// Requires the operator-provisioned data pack. `spring_water` reaches the
/// runtime through modifiers this branch has, and the disks now also model
/// `minecraft:heightmap`, `minecraft:matching_fluids`, and `minecraft:rule_based`.
#[test]
#[ignore = "requires operator-provisioned local data"]
fn the_real_pack_reaches_springs_and_gates_disks() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let data = FeatureData::load(&root).expect("the provisioned pack loads");
    for id in ["spring_water", "spring_lava", "spring_lava_frozen"] {
        let placed = data
            .placed
            .get(&format!("minecraft:{id}"))
            .expect("spring placement");
        println!("{id} modelable: {}", placed.is_modelable());
    }
    assert!(data.placed["minecraft:spring_water"].is_modelable());
    for id in ["disk_clay", "disk_gravel", "disk_sand"] {
        let name = format!("minecraft:{id}");
        assert!(data.configured[&name].is_supported(), "{id} is modeled");
        assert!(
            data.placed[&name].is_modelable(),
            "{id} is modelable now that heightmap and rule_based predicates are modeled"
        );
    }
}

#[test]
fn matching_fluids_derives_from_properties_and_tags_without_plant_list() {
    let mut tags = BlockTags::default();
    tags.entries.insert(
        "minecraft:water".to_string(),
        vec![
            "minecraft:water".to_string(),
            "minecraft:flowing_water".to_string(),
        ],
    );

    let mut target = TestTarget::new().with_ground(60).write_anywhere();
    target.set_block(0, 60, 0, "minecraft:stone");
    target.set_block(0, 61, 0, "minecraft:water");
    target.set_block(0, 62, 0, "minecraft:water[level=3]");
    target.set_block(0, 63, 0, "minecraft:lava[level=0]");
    target.set_block(0, 64, 0, "minecraft:lava[level=5]");
    target.set_block(0, 65, 0, "minecraft:chest[facing=north,waterlogged=true]");
    target.set_block(0, 66, 0, "minecraft:chest[facing=north,waterlogged=false]");
    target.set_block(0, 67, 0, "minecraft:seagrass");
    target.set_block(0, 68, 0, "minecraft:tall_seagrass[half=lower]");
    target.set_block(0, 69, 0, "minecraft:kelp[age=5]");
    target.set_block(0, 70, 0, "minecraft:kelp_plant");
    target.set_block(0, 71, 0, "minecraft:bubble_column[drag=true]");
    target.set_block(0, 72, 0, "minecraft:air");

    let still_water = BlockPredicate::MatchingFluids {
        fluids: vec!["minecraft:water".to_string()],
        offset: (0, 0, 0),
    };
    let flowing_water = BlockPredicate::MatchingFluids {
        fluids: vec!["minecraft:flowing_water".to_string()],
        offset: (0, 0, 0),
    };
    let still_lava = BlockPredicate::MatchingFluids {
        fluids: vec!["minecraft:lava".to_string()],
        offset: (0, 0, 0),
    };
    let flowing_lava = BlockPredicate::MatchingFluids {
        fluids: vec!["minecraft:flowing_lava".to_string()],
        offset: (0, 0, 0),
    };
    let empty = BlockPredicate::MatchingFluids {
        fluids: vec!["minecraft:empty".to_string()],
        offset: (0, 0, 0),
    };
    let tagged_water = BlockPredicate::MatchingFluids {
        fluids: vec!["#minecraft:water".to_string()],
        offset: (0, 0, 0),
    };

    assert!(still_water.test(&target, DecorationPos { x: 0, y: 61, z: 0 }, &tags));
    assert!(!still_water.test(&target, DecorationPos { x: 0, y: 62, z: 0 }, &tags));
    assert!(flowing_water.test(&target, DecorationPos { x: 0, y: 62, z: 0 }, &tags));
    assert!(tagged_water.test(&target, DecorationPos { x: 0, y: 61, z: 0 }, &tags));
    assert!(tagged_water.test(&target, DecorationPos { x: 0, y: 62, z: 0 }, &tags));

    // Waterlogged property drives matching without special-casing block IDs.
    assert!(still_water.test(&target, DecorationPos { x: 0, y: 65, z: 0 }, &tags));
    assert!(!still_water.test(&target, DecorationPos { x: 0, y: 66, z: 0 }, &tags));
    assert!(empty.test(&target, DecorationPos { x: 0, y: 66, z: 0 }, &tags));

    // Always-waterlogged plants/bubble_column hold a water source without a `waterlogged` property.
    for y in 67..=71 {
        let p = DecorationPos { x: 0, y, z: 0 };
        assert!(
            still_water.test(&target, p, &tags),
            "y={y} must match water"
        );
        assert!(
            !still_lava.test(&target, p, &tags),
            "y={y} must not match lava"
        );
        assert!(
            !empty.test(&target, p, &tags),
            "y={y} must not count as dry"
        );
    }
    assert!(empty.test(&target, DecorationPos { x: 0, y: 72, z: 0 }, &tags));
    assert!(empty.test(&target, DecorationPos { x: 0, y: 60, z: 0 }, &tags));

    assert!(still_lava.test(&target, DecorationPos { x: 0, y: 63, z: 0 }, &tags));
    assert!(!still_lava.test(&target, DecorationPos { x: 0, y: 64, z: 0 }, &tags));
    assert!(flowing_lava.test(&target, DecorationPos { x: 0, y: 64, z: 0 }, &tags));

    let offset_water = parse_block_predicate(Some(&json!({
        "type": "minecraft:matching_fluids",
        "offset": [0, 1, 0],
        "fluids": "minecraft:water"
    })));
    assert_eq!(
        offset_water,
        BlockPredicate::MatchingFluids {
            fluids: vec!["minecraft:water".to_string()],
            offset: (0, 1, 0),
        }
    );
    assert!(offset_water.test(&target, DecorationPos { x: 0, y: 60, z: 0 }, &tags));
}

#[test]
fn surface_relative_threshold_filter_enforces_ocean_floor_bounds() {
    let ore = stone_replacement("minecraft:gold_ore", 16, 0.0);
    let modifier = parse_placement_modifier(
        "test:surface",
        0,
        &json!({
            "type": "minecraft:surface_relative_threshold_filter",
            "heightmap": "OCEAN_FLOOR_WG",
            "min_inclusive": -13,
            "max_inclusive": -5
        }),
    )
    .unwrap();
    let placed = PlacedFeature {
        id: "test:surface".to_string(),
        feature: Rc::clone(&ore),
        modifiers: vec![modifier],
    };
    assert!(placed.is_modelable());
    let data = FeatureData::default();
    let mut random = DecorationRandom::new(2026);

    for (y, should_place) in [(46, false), (47, true), (50, true), (55, true), (56, false)] {
        let mut target = TestTarget::new().with_ground(60).write_anywhere();
        let count = data.run_chain(
            &mut target,
            &placed,
            0,
            DecorationPos { x: 0, y, z: 0 },
            &mut random,
        );
        assert_eq!(
            count > 0,
            should_place,
            "y={y} against ocean_floor=60: count={count}"
        );
    }

    // Other heightmaps are gated as unmodeled instead of reading ocean_floor_height.
    let other = parse_placement_modifier(
        "test:other",
        0,
        &json!({
            "type": "minecraft:surface_relative_threshold_filter",
            "heightmap": "WORLD_SURFACE_WG",
            "max_inclusive": -5
        }),
    )
    .unwrap();
    let gated = PlacedFeature {
        id: "test:other".to_string(),
        feature: ore,
        modifiers: vec![other],
    };
    assert!(!gated.is_modelable());
}

#[test]
fn environment_scan_walks_direction_and_enforces_allowed_and_max_steps() {
    let ore = stone_replacement("minecraft:diamond_ore", 16, 0.0);
    let tags = vegetation_tags();
    let data = FeatureData {
        tags,
        ..FeatureData::default()
    };
    let mut random = DecorationRandom::new(42);

    let scan_up = parse_placement_modifier(
        "test:scan_up",
        0,
        &json!({
            "type": "minecraft:environment_scan",
            "direction_of_search": "up",
            "max_steps": 4,
            "target_condition": {
                "type": "minecraft:matching_blocks",
                "blocks": "minecraft:stone"
            },
            "allowed_search_condition": {
                "type": "minecraft:matching_block_tag",
                "tag": "minecraft:air"
            }
        }),
    )
    .unwrap();
    let placed_up = PlacedFeature {
        id: "test:scan_up".to_string(),
        feature: Rc::clone(&ore),
        modifiers: vec![scan_up],
    };
    assert!(placed_up.is_modelable());

    let mut target = TestTarget::new().write_anywhere();
    target.set_block(0, 50, 0, "minecraft:air");
    target.set_block(0, 51, 0, "minecraft:air");
    target.set_block(0, 52, 0, "minecraft:air");
    target.set_block(0, 53, 0, "minecraft:stone");
    assert!(
        data.run_chain(
            &mut target,
            &placed_up,
            0,
            DecorationPos { x: 0, y: 50, z: 0 },
            &mut random
        ) > 0
    );
    assert!(target.blocks("minecraft:diamond_ore") > 0);

    // Fails immediately if start position violates `allowed_search_condition`.
    let mut blocked_start = TestTarget::new().write_anywhere();
    blocked_start.set_block(0, 50, 0, "minecraft:water");
    blocked_start.set_block(0, 51, 0, "minecraft:stone");
    assert_eq!(
        data.run_chain(
            &mut blocked_start,
            &placed_up,
            0,
            DecorationPos { x: 0, y: 50, z: 0 },
            &mut random
        ),
        0
    );

    // Unmodeled target predicate inside environment_scan gates the placement and reports a gap.
    let unmodeled_scan = parse_placement_modifier(
        "test:scan_gap",
        0,
        &json!({
            "type": "minecraft:environment_scan",
            "direction_of_search": "down",
            "max_steps": 4,
            "target_condition": { "type": "minecraft:solid" }
        }),
    )
    .unwrap();
    let placed_gap = PlacedFeature {
        id: "test:scan_gap".to_string(),
        feature: ore,
        modifiers: vec![unmodeled_scan],
    };
    assert!(!placed_gap.is_modelable());
    let mut gaps = HashMap::new();
    record_unmodeled_modifiers(&mut gaps, &placed_gap);
    assert_eq!(gaps.get("placement:minecraft:environment_scan"), Some(&1));
    assert_eq!(gaps.get("predicate:minecraft:minecraft:solid"), None);
    assert_eq!(gaps.get("predicate:minecraft:solid"), Some(&1));
}

#[test]
fn randomized_int_samples_property_uniformly_and_reports_gaps_without_air_fallback() {
    let doc = json!({
        "type": "minecraft:randomized_int",
        "property": "pickles",
        "source": {
            "id": "minecraft:sea_pickle",
            "properties": {
                "pickles": "1",
                "waterlogged": "true"
            }
        },
        "values": {
            "type": "minecraft:uniform",
            "min_inclusive": 1,
            "max_inclusive": 4
        }
    });
    let provider = parse_state_provider(Some(&doc));
    assert!(provider.is_modelable());
    assert!(matches!(provider, StateProvider::RandomizedInt { .. }));

    let target = TestTarget::new();
    let tags = BlockTags::default();
    let pos = DecorationPos { x: 0, y: 64, z: 0 };
    let mut random = DecorationRandom::new(987654321);
    let mut counts: HashMap<String, usize> = HashMap::new();
    for _ in 0..10_000 {
        let sampled = provider
            .sample(&target, pos, &tags, &mut random)
            .expect("modelable randomized_int provider must yield a state")
            .into_owned();
        *counts.entry(sampled).or_insert(0) += 1;
    }
    assert_eq!(counts.len(), 4);
    for i in 1..=4 {
        let expected = format!("minecraft:sea_pickle[pickles={i},waterlogged=true]");
        let count = counts.get(&expected).copied().unwrap_or_default();
        assert!(
            (2100..=2900).contains(&count),
            "count for {expected} was {count}, expected roughly 2500"
        );
    }

    // Un-namespaced identifiers are not aliased and report as unsupported gaps.
    let bare_type = json!({
        "type": "randomized_int",
        "property": "pickles",
        "source": { "id": "minecraft:sea_pickle" },
        "values": { "type": "minecraft:uniform", "min_inclusive": 1, "max_inclusive": 4 }
    });
    assert!(!parse_state_provider(Some(&bare_type)).is_modelable());

    // Missing source or state is recorded as an unsupported gap, never defaulted to air.
    let missing_source = json!({
        "type": "minecraft:randomized_int",
        "property": "pickles",
        "source": {},
        "values": { "type": "minecraft:uniform", "min_inclusive": 1, "max_inclusive": 4 }
    });
    let parsed_missing = parse_state_provider(Some(&missing_source));
    assert!(!parsed_missing.is_modelable());
    let mut gap_counts = HashMap::new();
    record_unmodeled_provider(&mut gap_counts, &parsed_missing);
    assert_eq!(gap_counts.get("provider:minecraft:missing_state"), Some(&1));
    assert!(!parsed_missing.states().contains(&"minecraft:air"));
}

#[test]
fn simple_random_selector_picks_uniformly_and_enforces_exact_type_and_gaps() {
    let granite = wrapper(
        "testns:child_granite",
        &stone_replacement("minecraft:granite", 16, 0.0),
        vec![
            PlacementModifier::Count {
                count: RangeProvider::Constant(1),
            },
            PlacementModifier::HeightRange {
                height: RangeProvider::Constant(64),
            },
        ],
    );
    let diorite = wrapper(
        "testns:child_diorite",
        &stone_replacement("minecraft:diorite", 16, 0.0),
        vec![
            PlacementModifier::Count {
                count: RangeProvider::Constant(1),
            },
            PlacementModifier::HeightRange {
                height: RangeProvider::Constant(64),
            },
        ],
    );
    let selector = Rc::new(ConfiguredFeature::SimpleRandomSelector {
        features: vec![Rc::clone(&granite), Rc::clone(&diorite)],
    });
    assert!(selector.is_supported());
    let placed_selector = wrapper("testns:selector_placed", &selector, vec![]);
    assert!(placed_selector.is_modelable());
    let data = registry(vec![placed_selector]);

    let mut granite_chosen = 0usize;
    let mut diorite_chosen = 0usize;
    for seed in 0..100i64 {
        let mut target = TestTarget::new().write_anywhere();
        let written = data.decorate_step(&mut target, 0, 0, 0, seed, &region());
        assert!(written > 0);
        let g = target.blocks("minecraft:granite");
        let d = target.blocks("minecraft:diorite");
        assert!((g > 0 && d == 0) || (d > 0 && g == 0));
        if g > 0 {
            granite_chosen += 1;
        } else {
            diorite_chosen += 1;
        }
    }
    assert!(granite_chosen > 30 && diorite_chosen > 30);

    // Un-namespaced selector identifier stays Unsupported.
    let unnamespaced = json!({
        "type": "simple_random_selector",
        "features": []
    });
    assert!(matches!(
        parse_configured_feature(&unnamespaced).unwrap(),
        ConfiguredFeature::Unsupported { ref kind } if kind == "simple_random_selector"
    ));

    // Real configured feature with unsupported inner feature reports the inner gap.
    let with_gap = parse_configured_feature(&json!({
        "type": "minecraft:simple_random_selector",
        "features": [
            {
                "feature": { "type": "minecraft:coral_tree" },
                "placement": []
            }
        ]
    }))
    .unwrap();
    assert!(!with_gap.is_supported());
    let mut gaps = HashMap::new();
    record_unmodeled_configured(&mut gaps, &with_gap);
    assert_eq!(gaps.get("minecraft:coral_tree"), Some(&1));
}

#[test]
fn diagnose_reports_first_and_all_unsupported_pieces_in_evaluation_order() {
    let root = scratch_root("diagnose-step");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("feature/grass.json"),
        r#"{"type": "minecraft:simple_block", "to_place": {"id": "minecraft:short_grass"}}"#,
    );
    write(
        &worldgen.join("feature/cane.json"),
        r#"{"type": "minecraft:block_column", "direction": "up", "allowed_placement": {"type": "minecraft:true"}, "prioritize_tip": false, "layers": []}"#,
    );
    write(
        &worldgen.join("feature/blocked_patch.json"),
        r#"{"type": "minecraft:vegetation_patch"}"#,
    );
    write(
        &worldgen.join("feature/plain_flower.json"),
        r#"{"type": "minecraft:simple_block", "to_place": "minecraft:flower_plain"}"#,
    );
    write(
        &worldgen.join("placed_feature/ready_grass.json"),
        r#"{"feature": "testns:grass", "placement": [
            {"type": "minecraft:count", "count": 2},
            {"type": "minecraft:in_square"},
            {"type": "minecraft:biome"}
        ]}"#,
    );
    write(
        &worldgen.join("placed_feature/ready_cane.json"),
        r#"{"feature": "testns:cane", "placement": [
            {"type": "minecraft:heightmap", "heightmap": "MOTION_BLOCKING"},
            {"type": "minecraft:offset", "x": 1, "y": 0, "z": 1},
            {"type": "minecraft:block_predicate_filter",
                "predicate": {"type": "minecraft:would_survive", "state": "minecraft:sugar_cane"}}
        ]}"#,
    );
    write(
        &worldgen.join("placed_feature/blocked_patch.json"),
        r#"{"feature": "testns:blocked_patch", "placement": [
            {"type": "minecraft:noise_threshold_count", "noise_level": -0.8, "below_noise": 5, "above_noise": 10},
            {"type": "minecraft:block_predicate_filter",
                "predicate": {"type": "minecraft:has_sturdy_face", "direction": "up"}}
        ]}"#,
    );
    write(
        &worldgen.join("placed_feature/blocked_provider.json"),
        r#"{"feature": "testns:plain_flower", "placement": [
            {"type": "minecraft:count", "count": {"type": "minecraft:clamped", "min_inclusive": 0, "max_inclusive": 1, "source": 1}}
        ]}"#,
    );
    write(
        &worldgen.join("biome/forest.json"),
        r#"{"features": [[], [], [], [], [], [], [], [], [], [
            "testns:ready_grass",
            "testns:ready_cane",
            "testns:blocked_patch",
            "testns:blocked_provider"
        ], []]}"#,
    );

    let data = FeatureData::load(&root).expect("loads synthetic pack");
    let report =
        data.diagnose_step_for_biomes(VEGETAL_DECORATION_STEP, &["testns:forest".to_string()]);
    assert_eq!(report.len(), 4);

    assert_eq!(
        report[0],
        PlacedFeatureDiagnosis {
            id: "testns:ready_grass".to_string(),
            modelable: true,
            first_unsupported: None,
            unsupported_pieces: Vec::new(),
        }
    );
    assert_eq!(
        report[1],
        PlacedFeatureDiagnosis {
            id: "testns:ready_cane".to_string(),
            modelable: true,
            first_unsupported: None,
            unsupported_pieces: Vec::new(),
        }
    );
    assert_eq!(
        report[2],
        PlacedFeatureDiagnosis {
            id: "testns:blocked_patch".to_string(),
            modelable: false,
            first_unsupported: Some("placement:minecraft:noise_threshold_count".to_string()),
            unsupported_pieces: vec![
                "placement:minecraft:noise_threshold_count".to_string(),
                "predicate:minecraft:has_sturdy_face".to_string(),
                "minecraft:vegetation_patch".to_string(),
            ],
        }
    );
    assert_eq!(
        report[3],
        PlacedFeatureDiagnosis {
            id: "testns:blocked_provider".to_string(),
            modelable: false,
            first_unsupported: Some(
                "provider:minecraft:block_state_provider_reference".to_string()
            ),
            unsupported_pieces: vec![
                "provider:minecraft:block_state_provider_reference".to_string(),
            ],
        }
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn heightmap_modifier_distinguishes_all_six_heightmaps() {
    let mut target = TestTarget::new().with_ground(60).write_anywhere();
    target.base = "minecraft:air".to_string();
    target.set_block(4, 60, 4, "minecraft:stone");
    target.set_block(4, 61, 4, "minecraft:water");
    target.set_block(4, 62, 4, "minecraft:water");
    target.set_block(4, 63, 4, "minecraft:oak_leaves");
    target.set_block(4, 64, 4, "minecraft:short_grass");

    target.set_block(5, 60, 5, "minecraft:stone");
    target.set_block(5, 61, 5, "minecraft:water");
    target.set_block(5, 62, 5, "minecraft:water");
    target.set_block(5, 63, 5, "minecraft:seagrass");

    let mut tags = BlockTags::default();
    tags.entries.insert(
        "minecraft:blocks_motion_in_heightmap".to_string(),
        vec![
            "minecraft:stone".to_string(),
            "minecraft:oak_leaves".to_string(),
        ],
    );
    tags.entries.insert(
        "minecraft:blocks_motion_in_heightmap_no_leaves".to_string(),
        vec!["minecraft:stone".to_string()],
    );
    tags.membership = build_tag_membership(&tags.entries);

    let check = |kind_str: &str, x: i32, z: i32| {
        let kind = HeightmapKind::parse(kind_str).expect("valid heightmap");
        target.heightmap_height(kind, x, z, &tags)
    };
    assert_eq!(check("WORLD_SURFACE_WG", 4, 4), 65);
    assert_eq!(check("WORLD_SURFACE", 4, 4), 65);
    assert_eq!(check("MOTION_BLOCKING", 4, 4), 64);
    assert_eq!(check("MOTION_BLOCKING_NO_LEAVES", 4, 4), 63);
    assert_eq!(check("OCEAN_FLOOR_WG", 4, 4), 64);
    assert_eq!(check("OCEAN_FLOOR", 5, 5), 61);
}

#[test]
fn range_providers_cover_26_1_fields_plateau_biased_clamped_and_weighted_list() {
    let trap_26_1 = RangeProvider::from_value(
        &json!({"type": "minecraft:trapezoid", "min": 2, "max": 6, "plateau": 2}),
        false,
    )
    .expect("parses 26.1 trapezoid");
    assert_eq!(trap_26_1.range(0, 0), Some((2, 6)));

    let uni_26_1 = RangeProvider::from_value(
        &json!({"type": "minecraft:uniform", "min": 1, "max": 4}),
        false,
    )
    .expect("parses 26.1 uniform");
    assert_eq!(uni_26_1.range(0, 0), Some((1, 4)));

    let biased = RangeProvider::from_value(
        &json!({"type": "minecraft:biased_to_bottom", "min_inclusive": 0, "max_inclusive": 2}),
        false,
    )
    .expect("parses biased_to_bottom");
    let clamped = RangeProvider::from_value(
        &json!({
            "type": "minecraft:clamped",
            "min_inclusive": 1,
            "max_inclusive": 3,
            "source": {"type": "minecraft:uniform", "min_inclusive": 0, "max_inclusive": 5}
        }),
        false,
    )
    .expect("parses clamped");
    let weighted = RangeProvider::from_value(
        &json!({
            "type": "minecraft:weighted_list",
            "distribution": [
                {"data": 2, "weight": 3},
                {"data": 5, "weight": 1}
            ]
        }),
        false,
    )
    .expect("parses weighted_list");

    let mut random = DecorationRandom::new(99);
    let mut counts = [0usize; 3];
    for _ in 0..9_000 {
        let v = biased.sample(0, 128, &mut random);
        assert!((0..=2).contains(&v));
        counts[v as usize] += 1;
        let cv = clamped.sample(0, 128, &mut random);
        assert!((1..=3).contains(&cv));
        let wv = weighted.sample(0, 128, &mut random);
        assert!(wv == 2 || wv == 5);
    }
    // Sugar Cane rev 3899892: 11/18, 5/18, 2/18 over 0..=2 (here 9000 samples).
    assert!(counts[0] > counts[1] && counts[1] > counts[2]);
    assert!((counts[0] as f64 / 9000.0 - 11.0 / 18.0).abs() < 0.03);
    assert!((counts[1] as f64 / 9000.0 - 5.0 / 18.0).abs() < 0.03);
    assert!((counts[2] as f64 / 9000.0 - 2.0 / 18.0).abs() < 0.03);
}

#[test]
fn offset_random_chance_and_reach_gating_work_as_specified() {
    let root = scratch_root("offset-chance");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &root.join("data/minecraft/tags/block/supports_vegetation.json"),
        r#"{"values": ["minecraft:grass_block", "minecraft:dirt"]}"#,
    );
    write(
        &worldgen.join("feature/grass.json"),
        r#"{"type": "minecraft:simple_block", "to_place": {"id": "minecraft:short_grass"}}"#,
    );
    write(
        &worldgen.join("placed_feature/near_patch.json"),
        r#"{"feature": "testns:grass", "placement": [
            {"type": "minecraft:random_chance", "chance": 1.0},
            {"type": "minecraft:offset", "x": 2, "y": 0, "z": -1},
            {"type": "minecraft:biome"}
        ]}"#,
    );
    write(
        &worldgen.join("placed_feature/wide_patch.json"),
        r#"{"feature": "testns:grass", "placement": [
            {"type": "minecraft:offset", "x": 10, "y": 0, "z": 0},
            {"type": "minecraft:offset", "x": 8, "y": 0, "z": 0}
        ]}"#,
    );
    write(
        &worldgen.join("biome/plains.json"),
        r#"{"features": [[], [], [], [], [], [], [], [], [], [
            "testns:near_patch",
            "testns:wide_patch"
        ], []]}"#,
    );
    let data = FeatureData::load(&root).expect("loads");
    assert!(data.placed["testns:near_patch"].is_modelable());
    assert!(!data.placed["testns:wide_patch"].is_modelable());

    let mut target = TestTarget::new()
        .with_ground(60)
        .in_biome(Some("testns:plains"))
        .write_anywhere();
    target.base = "minecraft:air".to_string();
    target.set_block(6, 60, 3, "minecraft:grass_block");
    let mut random = DecorationRandom::new(1);
    let written = data.run_chain(
        &mut target,
        &data.placed["testns:near_patch"],
        0,
        DecorationPos { x: 4, y: 61, z: 4 },
        &mut random,
    );
    assert_eq!(written, 1);
    assert_eq!(target.block_at(6, 61, 3), Some("minecraft:short_grass"));
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn predicates_replaceable_and_would_survive_enforce_plant_rules() {
    let mut tags = BlockTags::default();
    tags.entries.insert(
        "minecraft:replaceable".to_string(),
        vec![
            "minecraft:air".to_string(),
            "minecraft:short_grass".to_string(),
        ],
    );
    tags.entries.insert(
        "minecraft:supports_vegetation".to_string(),
        vec![
            "minecraft:grass_block".to_string(),
            "minecraft:dirt".to_string(),
        ],
    );
    tags.entries.insert(
        "minecraft:supports_sugar_cane".to_string(),
        vec![
            "minecraft:sand".to_string(),
            "minecraft:grass_block".to_string(),
        ],
    );
    tags.entries.insert(
        "minecraft:supports_cactus".to_string(),
        vec!["minecraft:sand".to_string()],
    );
    tags.entries.insert(
        "minecraft:water".to_string(),
        vec!["minecraft:water".to_string()],
    );
    tags.membership = build_tag_membership(&tags.entries);

    let mut target = TestTarget::new().with_ground(60).write_anywhere();
    target.base = "minecraft:air".to_string();
    target.set_block(0, 60, 0, "minecraft:grass_block");
    target.set_block(0, 61, 0, "minecraft:short_grass");
    target.set_block(5, 60, 5, "minecraft:sand");
    target.set_block(6, 60, 5, "minecraft:water");
    target.set_block(10, 60, 10, "minecraft:sand");

    let replaceable = BlockPredicate::Replaceable { offset: (0, 0, 0) };
    assert!(replaceable.test(&target, DecorationPos { x: 0, y: 61, z: 0 }, &tags));
    assert!(!replaceable.test(&target, DecorationPos { x: 0, y: 60, z: 0 }, &tags));

    let grass_survives = BlockPredicate::WouldSurvive {
        offset: (0, 0, 0),
        state: "minecraft:short_grass".to_string(),
    };
    assert!(grass_survives.test(&target, DecorationPos { x: 0, y: 61, z: 0 }, &tags));
    assert!(!grass_survives.test(&target, DecorationPos { x: 5, y: 61, z: 5 }, &tags));

    let cane_survives = BlockPredicate::WouldSurvive {
        offset: (0, 0, 0),
        state: "minecraft:sugar_cane".to_string(),
    };
    assert!(cane_survives.test(&target, DecorationPos { x: 5, y: 61, z: 5 }, &tags));
    assert!(!cane_survives.test(
        &target,
        DecorationPos {
            x: 10,
            y: 61,
            z: 10
        },
        &tags
    ));

    let cactus_survives = BlockPredicate::WouldSurvive {
        offset: (0, 0, 0),
        state: "minecraft:cactus".to_string(),
    };
    assert!(cactus_survives.test(
        &target,
        DecorationPos {
            x: 10,
            y: 61,
            z: 10
        },
        &tags
    ));
    target.set_block(11, 61, 10, "minecraft:stone");
    assert!(!cactus_survives.test(
        &target,
        DecorationPos {
            x: 10,
            y: 61,
            z: 10
        },
        &tags
    ));

    let sapling = BlockPredicate::WouldSurvive {
        offset: (0, 0, 0),
        state: "minecraft:oak_sapling".to_string(),
    };
    assert!(!sapling.is_modelable());
}

#[test]
fn state_provider_rotated_and_registry_reference_resolve_cleanly() {
    let root = scratch_root("provider-registry");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("block_state_provider/rotated_flower.json"),
        r#"{"type": "minecraft:rotated", "state": {"id": "minecraft:wildflowers", "properties": {"facing": "north", "flower_amount": "1"}}}"#,
    );
    write(
        &worldgen.join("feature/resolved.json"),
        r#"{"type": "minecraft:simple_block", "to_place": "testns:rotated_flower"}"#,
    );
    write(
        &worldgen.join("feature/unresolved.json"),
        r#"{"type": "minecraft:simple_block", "to_place": "testns:missing_provider"}"#,
    );
    write(
        &worldgen.join("placed_feature/resolved.json"),
        r#"{"feature": "testns:resolved", "placement": []}"#,
    );
    write(
        &worldgen.join("placed_feature/unresolved.json"),
        r#"{"feature": "testns:unresolved", "placement": []}"#,
    );
    write(
        &worldgen.join("biome/plains.json"),
        r#"{"features": [[], [], [], [], [], [], [], [], [], ["testns:resolved", "testns:unresolved"], []]}"#,
    );
    let data = FeatureData::load(&root).expect("loads");
    assert!(data.placed["testns:resolved"].is_modelable());
    assert!(!data.placed["testns:unresolved"].is_modelable());
    assert_eq!(
        data.unimplemented_features
            .get("provider:minecraft:block_state_provider_reference"),
        Some(&1)
    );
    if let ConfiguredFeature::SimpleBlock { to_place } = &*data.configured["testns:resolved"] {
        let target = TestTarget::new();
        let mut random = DecorationRandom::new(7);
        let picked = to_place
            .sample(
                &target,
                DecorationPos { x: 0, y: 64, z: 0 },
                &data.tags,
                &mut random,
            )
            .expect("sample");
        assert!(picked.starts_with("minecraft:wildflowers[facing="));
    } else {
        panic!("expected resolved simple_block");
    }
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn block_column_prioritize_tip_and_selectors_place_expected_plants() {
    let root = scratch_root("column-selectors");
    let worldgen = root.join("data/testns/worldgen");
    write(
        &root.join("data/minecraft/tags/block/supports_vegetation.json"),
        r#"{"values": ["minecraft:grass_block", "minecraft:sand"]}"#,
    );
    write(
        &root.join("data/minecraft/tags/block/replaceable.json"),
        r#"{"values": ["minecraft:air"]}"#,
    );
    write(
        &worldgen.join("feature/col_no_tip.json"),
        r#"{"type": "minecraft:block_column", "direction": "up", "prioritize_tip": false,
            "allowed_placement": {"type": "minecraft:matching_blocks", "blocks": ["minecraft:air"]},
            "layers": [
                {"height": 2, "provider": {"id": "minecraft:kelp_plant"}},
                {"height": 2, "provider": {"id": "minecraft:kelp"}}
            ]}"#,
    );
    write(
        &worldgen.join("feature/col_tip.json"),
        r#"{"type": "minecraft:block_column", "direction": "up", "prioritize_tip": true,
            "allowed_placement": {"type": "minecraft:matching_blocks", "blocks": ["minecraft:air"]},
            "layers": [
                {"height": 2, "provider": {"id": "minecraft:kelp_plant"}},
                {"height": 2, "provider": {"id": "minecraft:kelp"}}
            ]}"#,
    );
    write(
        &worldgen.join("feature/short_grass.json"),
        r#"{"type": "minecraft:simple_block", "to_place": {"id": "minecraft:short_grass"}}"#,
    );
    write(
        &worldgen.join("feature/fern.json"),
        r#"{"type": "minecraft:simple_block", "to_place": {"id": "minecraft:fern"}}"#,
    );
    write(
        &worldgen.join("placed_feature/short_grass_inline.json"),
        r#"{"feature": "testns:short_grass", "placement": []}"#,
    );
    write(
        &worldgen.join("placed_feature/fern_inline.json"),
        r#"{"feature": "testns:fern", "placement": []}"#,
    );
    write(
        &worldgen.join("feature/bool_sel.json"),
        r#"{"type": "minecraft:random_boolean_selector",
            "feature_true": "testns:short_grass_inline",
            "feature_false": "testns:fern_inline"}"#,
    );
    write(
        &worldgen.join("placed_feature/bool_sel_placed.json"),
        r#"{"feature": "testns:bool_sel", "placement": []}"#,
    );
    write(
        &worldgen.join("feature/weight_sel.json"),
        r#"{"type": "minecraft:weighted_random_selector",
            "features": [{"data": "testns:bool_sel_placed", "weight": 3}]}"#,
    );
    write(
        &worldgen.join("placed_feature/weight_sel_placed.json"),
        r#"{"feature": "testns:weight_sel", "placement": []}"#,
    );
    write(
        &worldgen.join("feature/prob_sel.json"),
        r#"{"type": "minecraft:random_selector",
            "default": "testns:weight_sel_placed",
            "features": [{"feature": "testns:fern_inline", "chance": 0.5}]}"#,
    );
    write(
        &worldgen.join("placed_feature/col_no_tip.json"),
        r#"{"feature": "testns:col_no_tip", "placement": []}"#,
    );
    write(
        &worldgen.join("placed_feature/col_tip.json"),
        r#"{"feature": "testns:col_tip", "placement": []}"#,
    );
    write(
        &worldgen.join("placed_feature/prob_sel.json"),
        r#"{"feature": "testns:prob_sel", "placement": []}"#,
    );
    write(
        &worldgen.join("biome/forest.json"),
        r#"{"features": [[], [], [], [], [], [], [], [], [], [
            "testns:col_no_tip",
            "testns:col_tip",
            "testns:prob_sel"
        ], []]}"#,
    );
    let data = FeatureData::load(&root).expect("loads");
    let mut target = TestTarget::new().with_ground(60).write_anywhere();
    target.base = "minecraft:air".to_string();
    target.set_block(2, 64, 2, "minecraft:stone");
    target.set_block(6, 64, 6, "minecraft:stone");
    let mut random = DecorationRandom::new(42);
    assert_eq!(
        data.run_chain(
            &mut target,
            &data.placed["testns:col_no_tip"],
            0,
            DecorationPos { x: 2, y: 61, z: 2 },
            &mut random,
        ),
        3
    );
    assert_eq!(target.block_at(2, 61, 2), Some("minecraft:kelp_plant"));
    assert_eq!(target.block_at(2, 62, 2), Some("minecraft:kelp_plant"));
    assert_eq!(target.block_at(2, 63, 2), Some("minecraft:kelp"));

    assert_eq!(
        data.run_chain(
            &mut target,
            &data.placed["testns:col_tip"],
            0,
            DecorationPos { x: 6, y: 61, z: 6 },
            &mut random,
        ),
        3
    );
    assert_eq!(target.block_at(6, 61, 6), Some("minecraft:kelp_plant"));
    assert_eq!(target.block_at(6, 62, 6), Some("minecraft:kelp"));
    assert_eq!(target.block_at(6, 63, 6), Some("minecraft:kelp"));

    target.set_block(10, 60, 10, "minecraft:grass_block");
    assert!(data.placed["testns:prob_sel"].is_modelable());
    assert_eq!(
        data.run_chain(
            &mut target,
            &data.placed["testns:prob_sel"],
            0,
            DecorationPos {
                x: 10,
                y: 61,
                z: 10
            },
            &mut random,
        ),
        1
    );
    let placed = target.block_at(10, 61, 10).expect("placed plant");
    assert!(placed == "minecraft:short_grass" || placed == "minecraft:fern");
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
#[ignore = "requires operator-provisioned local data"]
fn pack_simple_vegetation_features_are_modelable() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let data = FeatureData::load(&root).expect("the provisioned pack loads");
    for id in [
        "minecraft:patch_grass_forest",
        "minecraft:flower_default",
        "minecraft:forest_flowers",
        "minecraft:patch_bush",
        "minecraft:patch_pumpkin",
        "minecraft:patch_sugar_cane",
        "minecraft:patch_firefly_bush_near_water",
        "minecraft:patch_cactus_desert",
        "minecraft:patch_dead_bush",
        "minecraft:patch_grass_taiga",
        "minecraft:seagrass_normal",
        "minecraft:sea_pickle",
        "minecraft:patch_waterlily",
    ] {
        let placed = data
            .placed
            .get(id)
            .unwrap_or_else(|| panic!("missing placed feature {id}"));
        assert!(
            placed.is_modelable(),
            "expected {id} to be modelable, got diagnosis {:?}",
            placed.diagnose()
        );
    }
    for id in [
        "minecraft:trees_birch_and_oak_leaf_litter",
        "minecraft:patch_grass_plain",
        "minecraft:kelp_cold",
        "minecraft:brown_mushroom_normal",
    ] {
        assert!(
            !data.placed[id].is_modelable(),
            "{id} uses out-of-scope or unstated rules and must stay gated"
        );
    }
}
