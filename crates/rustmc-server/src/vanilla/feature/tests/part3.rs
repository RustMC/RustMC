//! Unit tests, part 3 of 5.

use super::*;

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
pub(super) struct OpenGround {
    pub(super) top: i32,
    pub(super) halo: &'static str,
    pub(super) written: HashMap<(i32, i32, i32), String>,
}

impl OpenGround {
    pub(super) fn new(top: i32) -> Self {
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

pub(super) fn vegetation_tags() -> BlockTags {
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

pub(super) fn data_with_vegetation_tags() -> FeatureData {
    FeatureData {
        tags: vegetation_tags(),
        ..FeatureData::default()
    }
}

pub(super) fn simple_block(state: &str) -> Rc<ConfiguredFeature> {
    Rc::new(ConfiguredFeature::SimpleBlock {
        to_place: StateProvider::Constant {
            state: state.to_string(),
        },
    })
}

pub(super) fn placed(
    feature: Rc<ConfiguredFeature>,
    modifiers: Vec<PlacementModifier>,
) -> PlacedFeature {
    PlacedFeature {
        id: "test:patch".to_string(),
        feature,
        modifiers,
    }
}

pub(super) fn patch(
    tries: i32,
    xz_spread: i32,
    y_spread: i32,
    child: PlacedFeature,
) -> PlacedFeature {
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
