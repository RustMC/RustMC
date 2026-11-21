//! Unit tests, part 1 of 5.

use super::*;

pub(super) static COUNTER: AtomicU32 = AtomicU32::new(0);

pub(super) fn scratch_root(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("rustmc-feature-{label}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create scratch dir");
    path
}

pub(super) fn write(path: &Path, text: &str) {
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
pub(super) fn report_pack(label: &str, placement: &str) -> PathBuf {
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
pub(super) struct TestTarget {
    pub(super) min_y: i32,
    pub(super) height: i32,
    pub(super) chunk_x: i32,
    pub(super) chunk_z: i32,
    pub(super) base: String,
    pub(super) write_anywhere: bool,
    pub(super) ground: i32,
    pub(super) written: HashMap<(i32, i32, i32), String>,
    pub(super) biome: Option<String>,
}

impl TestTarget {
    pub(super) fn new() -> Self {
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
    pub(super) fn anchored_at(mut self, chunk_x: i32, chunk_z: i32) -> Self {
        self.chunk_x = chunk_x;
        self.chunk_z = chunk_z;
        self
    }

    /// Let the pass write anywhere in the build volume, so a test can
    /// measure a vein's true extent instead of its clipped one.
    pub(super) fn write_anywhere(mut self) -> Self {
        self.write_anywhere = true;
        self
    }

    /// The row every column's ocean-floor heightmap reports. The fixture
    /// terrain fills the build volume, so the default is its top row and no
    /// attempt is ever unanchored; a lower row leaves the sky above it empty
    /// for the anchor gate to read.
    pub(super) fn with_ground(mut self, y: i32) -> Self {
        self.ground = y;
        self
    }

    pub(super) fn in_biome(mut self, biome: Option<&str>) -> Self {
        self.biome = biome.map(str::to_owned);
        self
    }

    pub(super) fn max_y(&self) -> i32 {
        self.min_y + self.height - 1
    }

    /// How many written positions hold `name`.
    pub(super) fn blocks(&self, name: &str) -> usize {
        self.written
            .values()
            .filter(|state| state.as_str() == name)
            .count()
    }

    pub(super) fn total(&self) -> usize {
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
pub(super) fn stone_replacement(state: &str, size: i32, discard: f32) -> Rc<ConfiguredFeature> {
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

pub(super) fn wrapper(
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
pub(super) fn registry(step: Vec<Rc<PlacedFeature>>) -> FeatureData {
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
pub(super) fn region() -> Vec<String> {
    vec!["minecraft:plains".to_string()]
}

/// The identifiers one generation step's schedule runs, in ordinal order.
pub(super) fn step_ids(data: &FeatureData, step: usize) -> Vec<String> {
    data.step(step)
        .unwrap_or_else(|| panic!("the pack has no step {step}"))
        .iter()
        .map(|feature| feature.id.clone())
        .collect()
}

/// Runs one vein on `target` from the stream `seed`/`ordinal` gives it.
pub(super) fn paint(
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
pub(super) fn attempt(
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
pub(super) fn signature(random: &DecorationRandom) -> [i32; 3] {
    let mut probe = random.clone();
    [probe.next_int(), probe.next_int(), probe.next_int()]
}

pub(super) fn reference(baseline: &DecorationRandom, longs: u32) -> [i32; 3] {
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
