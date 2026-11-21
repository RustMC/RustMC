//! Unit tests, part 4 of 5.

use super::*;

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
pub(super) struct CornerPocket;

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

pub(super) fn disk_of(provider: Value, radius: Value, half_height: i32) -> PlacedFeature {
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
