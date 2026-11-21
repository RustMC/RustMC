//! Unit tests for `vanilla::carver`.

use super::*;

fn overworld_ctx() -> CarverContext {
    CarverContext {
        min_y: -64,
        gen_depth: 384,
        sea_level: 63,
    }
}

/// The parameter shape of the shipped `minecraft:cave` document,
/// self-authored numbers with the same provider kinds.
fn simple_cave() -> Carver {
    Carver::Cave(CaveCarver {
        probability: 0.15,
        y: HeightSpec {
            min: Anchor::AboveBottom(8),
            max: Anchor::Absolute(180),
        },
        count: IntSpec::VeryBiasedToBottom { min: 0, max: 14 },
        thickness: FloatSpec::Trapezoid {
            min: 0.0,
            max: 3.0,
            plateau: 1.0,
        },
        weird_thickness_bias: true,
        room_vertical: FloatSpec::Uniform { min: 0.1, max: 0.9 },
        horizontal_multiplier: FloatSpec::Uniform { min: 0.7, max: 1.4 },
        vertical_multiplier: FloatSpec::Uniform { min: 0.8, max: 1.3 },
        start_vertical: FloatSpec::Constant(1.0),
        floor: FloatSpec::Uniform {
            min: -1.0,
            max: -0.4,
        },
    })
}

#[test]
fn biome_carvers_preserve_source_indices_and_reject_bad_entries() {
    let known = HashSet::from(["test:unsupported".to_owned(), "test:cave".to_owned()]);
    let cave = Rc::new(simple_cave());
    let supported = HashMap::from([("test:cave".to_owned(), Rc::clone(&cave))]);
    let biome = serde_json::json!({"carvers": ["test:unsupported", "test:cave"]});
    let slots = resolve_biome_carvers("test:biome", &biome, &known, &supported).unwrap();
    assert_eq!(slots.len(), 2);
    assert!(slots[0].is_none());
    assert!(Rc::ptr_eq(slots[1].as_ref().unwrap(), &cave));
    let single = serde_json::json!({"carvers": "test:cave"});
    assert_eq!(
        resolve_biome_carvers("test:biome", &single, &known, &supported)
            .unwrap()
            .len(),
        1
    );
    for invalid in [
        serde_json::json!({"carvers": [17]}),
        serde_json::json!({"carvers": ["test:missing"]}),
    ] {
        assert!(resolve_biome_carvers("test:biome", &invalid, &known, &supported).is_err());
    }
}

#[test]
fn mask_window_is_the_orchestration_bitset() {
    // Overworld allocation: CarvingMask(minGenY + 1, minGenY +
    // genDepth - 1 - 7) = -63..=312, 256 columns x 376 levels.
    let mask = CarveMask::new(-63, 312);
    assert_eq!(mask.min_y(), -63);
    assert_eq!(mask.max_y(), 312);
    assert!(mask.is_empty());
    assert_eq!(mask.carved_count(), 0);
}

#[test]
fn mask_indexes_bits_by_y_then_column() {
    let mut mask = CarveMask::new(0, 9);
    mask.carve(0, 5, 0);
    mask.carve(1, 0, 0);
    assert!(mask.contains(0, 5, 0));
    assert!(mask.contains(1, 0, 0));
    assert!(!mask.contains(0, 4, 0));
    assert!(!mask.contains(0, 5, 1));
    // The Java index formula: y - minY + (z + (x << 4)) * height;
    // x=1 selects column 16.
    assert_eq!(mask.index(1, 0, 0), 160);
    assert_eq!(mask.index(0, 5, 0), 5);
    assert_eq!(mask.carved_count(), 2);
}

#[test]
fn ellipsoid_clips_to_the_target_chunk_and_y_window() {
    let mut mask = CarveMask::new(0, 40);
    let skip = Skip::Cave { floor: -1.0e9 };
    // Far outside the reach bail-out: nothing stamped.
    carve_ellipsoid(
        &mut mask,
        (0, 0),
        &Ellipsoid {
            x: 100.0,
            y: 20.0,
            z: 100.0,
            horizontal_radius: 3.0,
            vertical_radius: 3.0,
        },
        &skip,
    );
    assert!(mask.is_empty());
    // Centered in the chunk, partially below the mask floor: the
    // y window clips, the column clip keeps 0..=15.
    carve_ellipsoid(
        &mut mask,
        (0, 0),
        &Ellipsoid {
            x: 8.0,
            y: 1.0,
            z: 8.0,
            horizontal_radius: 3.0,
            vertical_radius: 3.0,
        },
        &skip,
    );
    assert!(!mask.is_empty());
    assert!(!mask.contains(0, 0, 0));
    for y in -5..0 {
        // Y below the window is never stamped (clip is a clamp).
        assert!(!mask.contains(8, y, 8), "stamped below window at {y}");
    }
    assert!(mask.contains(8, 1, 8));
}

#[test]
fn can_reach_uses_remaining_walk_plus_thickness_margin() {
    // Standing at the target center with steps left: reachable.
    assert!(can_reach((0, 0), 8.0, 8.0, 0, 112, 3.0));
    // Far along x beyond the remaining reach: bails.
    assert!(!can_reach((0, 0), 8.0 + 60.0, 8.0, 110, 112, 3.0));
    // The margin is (thickness + 18) as a float sum widened to double.
    let reach = f64::from(3.0f32 + 18.0f32);
    assert_eq!(reach, 21.0);
}

#[test]
fn very_biased_to_bottom_draws_three_sequential_ints() {
    let spec = IntSpec::VeryBiasedToBottom { min: 0, max: 14 };
    let mut reference = LegacyRandom::new(2026);
    let expected = {
        let span = reference.next_int_bounded(15);
        let once = reference.next_int_bounded(span + 1);
        reference.next_int_bounded(once + 1)
    };
    let mut walker = LegacyRandom::new(2026);
    assert_eq!(spec.sample(&mut walker), expected);
}

#[test]
fn trapezoid_draws_two_floats_and_blends_plateau_edges() {
    let spec = FloatSpec::Trapezoid {
        min: 0.0,
        max: 3.0,
        plateau: 1.0,
    };
    let mut reference = LegacyRandom::new(99);
    let a = reference.next_float();
    let b = reference.next_float();
    // range 3, plateau_start 1, plateau_end 2.
    let expected = 0.0 + a * 2.0 + b * 1.0;
    let mut walker = LegacyRandom::new(99);
    assert_eq!(spec.sample(&mut walker), expected);
}

#[test]
fn height_spec_empty_range_skips_the_draw() {
    let spec = HeightSpec {
        min: Anchor::Absolute(70),
        max: Anchor::Absolute(60),
    };
    let mut walker = LegacyRandom::new(7);
    assert_eq!(spec.sample(&overworld_ctx(), &mut walker), 70);
    let after = walker.next_int_bounded(2);
    // An untouched stream must produce the same next draw: the
    // empty range consumed nothing.
    let mut untouched = LegacyRandom::new(7);
    assert_eq!(untouched.next_int_bounded(2), after);
}

#[test]
fn anchors_resolve_against_the_dimension_bounds() {
    let ctx = overworld_ctx();
    assert_eq!(Anchor::Absolute(10).resolve(&ctx), 10);
    assert_eq!(Anchor::AboveBottom(8).resolve(&ctx), -56);
    assert_eq!(Anchor::BelowTop(24).resolve(&ctx), 295);
    assert_eq!(Anchor::RelativeToSeaLevel(-1).resolve(&ctx), 62);
}

#[test]
fn cave_carver_is_deterministic_across_replays() {
    let carver = simple_cave();
    let ctx = overworld_ctx();
    let mut first = LegacyRandom::new(0);
    first.set_large_feature_seed(2026, 3, -4);
    let mut first_mask = CarveMask::new(-63, 312);
    carver.carve(&ctx, &mut first, (0, 0), (3, -4), &mut first_mask);
    let mut second = LegacyRandom::new(0);
    second.set_large_feature_seed(2026, 3, -4);
    let mut second_mask = CarveMask::new(-63, 312);
    carver.carve(&ctx, &mut second, (0, 0), (3, -4), &mut second_mask);
    assert_eq!(first_mask.carved_count(), second_mask.carved_count());
}

#[test]
fn forced_cave_start_carves_near_its_origin() {
    // A constant single cave at probability 1 must stamp the chunk
    // containing its source position for seeds where the walk stays
    // inside reach.
    let carver = Carver::Cave(CaveCarver {
        probability: 1.0,
        y: HeightSpec {
            min: Anchor::Absolute(60),
            max: Anchor::Absolute(60),
        },
        count: IntSpec::Constant(1),
        thickness: FloatSpec::Constant(2.0),
        weird_thickness_bias: false,
        room_vertical: FloatSpec::Constant(1.0),
        horizontal_multiplier: FloatSpec::Constant(1.0),
        vertical_multiplier: FloatSpec::Constant(1.0),
        start_vertical: FloatSpec::Constant(1.0),
        floor: FloatSpec::Constant(-1.0),
    });
    let ctx = overworld_ctx();
    let mut total = 0;
    for seed in 0..8 {
        let mut random = LegacyRandom::new(0);
        random.set_large_feature_seed(seed, 0, 0);
        assert!(carver.is_start_chunk(&mut random));
        let mut mask = CarveMask::new(-63, 312);
        carver.carve(&ctx, &mut random, (0, 0), (0, 0), &mut mask);
        total += mask.carved_count();
    }
    assert!(total > 1000, "cave walks carved only {total} blocks");
}

#[test]
fn canyon_width_factors_reroll_at_index_zero_without_a_draw() {
    let carver = CanyonCarver {
        probability: 1.0,
        y: HeightSpec {
            min: Anchor::Absolute(30),
            max: Anchor::Absolute(40),
        },
        vertical_rotation: FloatSpec::Constant(0.0),
        shape: CanyonShape {
            distance_factor: FloatSpec::Constant(1.0),
            thickness: FloatSpec::Constant(1.0),
            width_smoothness: 3,
            horizontal_factor: FloatSpec::Constant(1.0),
            vertical_default: 1.0,
            vertical_center: 0.0,
            y_scale: FloatSpec::Constant(3.0),
        },
    };
    let ctx = overworld_ctx();
    let mut walker = LegacyRandom::new(5);
    let factors = carver.init_width_factors(&ctx, &mut walker);
    assert_eq!(factors.len(), ctx.gen_depth as usize);
    // Index 0 re-rolls unconditionally: at least one draw pair was
    // consumed, and every factor is a squared value >= 1.
    assert!(factors.iter().all(|factor| *factor >= 1.0));
    // The factor stays sticky until a 1-in-3 reroll, so changes
    // must exist but stay well below the slot count.
    let changes = factors.windows(2).filter(|pair| pair[0] != pair[1]).count();
    assert!(changes > 0, "stream never rerolled");
    assert!(changes < ctx.gen_depth as usize / 2);
}

#[test]
fn parses_cave_document_with_default_start_vertical() {
    let document: Value = serde_json::from_str(
        r#"{
            "type": "minecraft:cave",
            "probability": 0.15,
            "y": {"type": "minecraft:uniform",
                  "min_inclusive": {"above_bottom": 8},
                  "max_inclusive": {"absolute": 180}},
            "count": {"type": "minecraft:very_biased_to_bottom",
                      "min_inclusive": 0, "max_inclusive": 14},
            "thickness": {"type": "minecraft:trapezoid",
                          "min": 0.0, "max": 3.0, "plateau": 1.0},
            "weird_thickness_bias": true,
            "room_vertical_radius_multiplier": {"type": "minecraft:uniform",
                          "min_inclusive": 0.1, "max_exclusive": 0.9},
            "horizontal_radius_multiplier": {"type": "minecraft:uniform",
                          "min_inclusive": 0.7, "max_exclusive": 1.4},
            "vertical_radius_multiplier": {"type": "minecraft:uniform",
                          "min_inclusive": 0.8, "max_exclusive": 1.3},
            "floor_level": {"type": "minecraft:uniform",
                          "min_inclusive": -1.0, "max_exclusive": -0.4}
        }"#,
    )
    .expect("json");
    let parsed = parse_carver("minecraft:cave", &document)
        .expect("parses")
        .expect("cave type");
    let Carver::Cave(carver) = parsed else {
        panic!("expected the cave family");
    };
    assert!((carver.probability - 0.15).abs() < 1e-6);
    assert_eq!(carver.y.min, Anchor::AboveBottom(8));
    assert_eq!(carver.y.max, Anchor::Absolute(180));
    assert_eq!(
        carver.count,
        IntSpec::VeryBiasedToBottom { min: 0, max: 14 }
    );
    assert!(carver.weird_thickness_bias);
    // The absent field defaults to the constant 1.0 multiplier.
    assert_eq!(carver.start_vertical, FloatSpec::Constant(1.0));
    assert_eq!(
        carver.thickness,
        FloatSpec::Trapezoid {
            min: 0.0,
            max: 3.0,
            plateau: 1.0
        }
    );
}

#[test]
fn unsupported_carver_types_are_skipped() {
    let document: Value =
        serde_json::from_str(r#"{"type": "minecraft:nether_cave"}"#).expect("json");
    assert!(
        parse_carver("minecraft:nether_cave", &document)
            .expect("ok")
            .is_none()
    );
}
