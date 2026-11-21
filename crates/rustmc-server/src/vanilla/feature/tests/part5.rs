//! Unit tests, part 5 of 5.

use super::*;

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
