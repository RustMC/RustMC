//! Unit tests, part 2 of 5.

use super::*;

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
pub(super) fn schedule_pack(label: &str, biome_one: &str, biome_two: &str) -> PathBuf {
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
