//! Unit tests for `vanilla::density`.

use super::*;
use serde_json::json;

fn engine() -> NoiseEngine {
    let mut root = RandomSource::from_world_seed(2026);
    let positional = root.fork_positional();
    NoiseEngine::new(positional, HashMap::new())
}

fn engine_with_noise() -> NoiseEngine {
    let mut root = RandomSource::from_world_seed(2026);
    let positional = root.fork_positional();
    let definitions = HashMap::from([("minecraft:shift".to_owned(), NoiseParameters::new(-2))]);
    NoiseEngine::new(positional, definitions)
}

fn registry(engine: &NoiseEngine) -> DensityRegistry<'_> {
    DensityRegistry::new(engine, HashMap::new())
}

#[test]
fn horizontal_shift_memo_matches_direct_noise_across_heights_and_eviction() {
    let engine = engine_with_noise();
    let stack = engine.stack(&"minecraft:shift".to_owned()).unwrap();
    for kind in [ShiftKind::Xy, ShiftKind::Z] {
        let node = Density(Rc::new(Node::ShiftNoise {
            kind,
            stack: Rc::clone(&stack),
            horizontal: RefCell::new(BoundedCache::new(512)),
        }));
        for x in -750..750 {
            let z = x * 3 - 17;
            let fx = f64::from(x) * 0.25;
            let fz = f64::from(z) * 0.25;
            let expected = match kind {
                ShiftKind::Xy => stack.get(fx, 0.0, fz),
                ShiftKind::Z => stack.get(fz, fx, 0.0),
                ShiftKind::X => unreachable!(),
            } * 4.0;
            for y in [-64, 0, 127, 319] {
                assert_eq!(node.sample(x, y, z).to_bits(), expected.to_bits());
            }
        }
        let Node::ShiftNoise { horizontal, .. } = &*node.0 else {
            unreachable!()
        };
        assert!(horizontal.borrow().entries() <= 1024);
    }
}

/// Exact coordinate passthrough for small coordinates: the Chebyshev
/// distance to a pivot is `pivot - coordinate` as long as the other
/// axes stay well below the pivot, and every intermediate stays an
/// exactly representable f32 integer below 2^24.
fn identity(axis: Axis) -> Density {
    let pivot = 1 << 23;
    let point = match axis {
        Axis::X => [pivot, 0, 0],
        Axis::Y => [0, pivot, 0],
        Axis::Z => [0, 0, pivot],
    };
    let distance = Density(Rc::new(Node::DistanceToPoint {
        point,
        metric: DistanceMetric::Chebyshev,
    }));
    let negated = Density(Rc::new(Node::Unary(UnaryKind::Negate, distance)));
    Density(Rc::new(Node::Binary(
        BinaryKind::Add,
        negated,
        Density::constant(pivot as f32),
    )))
}

#[test]
fn unary_ops_follow_java_float_semantics() {
    let engine = engine();
    let reg = registry(&engine);
    let squeeze = reg
        .compile_value(&json!({"type": "minecraft:squeeze", "input": 0.5}))
        .unwrap();
    let expected = 0.5f32 / 2.0 - (0.5f32 * 0.5 * 0.5) / 24.0;
    assert_eq!(squeeze.sample(0, 0, 0).to_bits(), expected.to_bits());

    let half = reg
        .compile_value(&json!({"type": "minecraft:half_negative", "input": -2.0}))
        .unwrap();
    assert_eq!(half.sample(0, 0, 0), -1.0);
    let quarter = reg
        .compile_value(&json!({"type": "minecraft:quarter_negative", "input": -2.0}))
        .unwrap();
    assert_eq!(quarter.sample(0, 0, 0), -0.5);
    let positive = reg
        .compile_value(&json!({"type": "minecraft:quarter_negative", "input": 3.0}))
        .unwrap();
    assert_eq!(positive.sample(0, 0, 0), 3.0);
    let reciprocal = reg
        .compile_value(&json!({"type": "minecraft:reciprocal", "input": 4.0}))
        .unwrap();
    assert_eq!(reciprocal.sample(0, 0, 0), 0.25);
    let sign = reg
        .compile_value(&json!({"type": "minecraft:sign", "input": -3.0}))
        .unwrap();
    assert_eq!(sign.sample(0, 0, 0), -1.0);
}

#[test]
fn binary_ops_short_circuit_and_propagate_nan_like_java() {
    // 0 / 0 collapses to positive zero, matching the Java left-is-zero skip.
    assert_eq!(BinaryKind::Div.apply(0.0, 0.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(BinaryKind::Div.apply(-0.0, 3.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(BinaryKind::Div.apply(6.0, 3.0), 2.0);
    assert!(BinaryKind::Min.apply(f32::NAN, 1.0).is_nan());
    assert!(BinaryKind::Max.apply(1.0, f32::NAN).is_nan());
    assert_eq!(java_min(-0.0, 0.0).to_bits(), (-0.0f32).to_bits());
    assert_eq!(java_max(-0.0, 0.0).to_bits(), 0.0f32.to_bits());
}

#[test]
fn round_modes_match_math_round() {
    assert_eq!(RoundKind::Round.apply(-0.5), 0.0);
    assert_eq!(RoundKind::Round.apply(-0.51), -1.0);
    assert_eq!(RoundKind::Floor.apply(-0.01), -1.0);
    assert_eq!(RoundKind::Ceil.apply(0.01), 1.0);
    assert_eq!(
        RoundKind::Truncate.apply(-0.9).to_bits(),
        (-0.0f32).to_bits()
    );
}

#[test]
fn lerp_alpha_shortcuts_skip_the_other_branch() {
    let first = Density::constant(3.0);
    let nan = Density::constant(f32::NAN);
    let zero = Density::constant(0.0);
    let one = Density::constant(1.0);
    let low = Density(Rc::new(Node::Lerp {
        alpha: zero.clone(),
        first: first.clone(),
        second: nan.clone(),
    }));
    assert_eq!(low.sample(0, 0, 0), 3.0);
    let high = Density(Rc::new(Node::Lerp {
        alpha: one,
        first: nan,
        second: first,
    }));
    assert_eq!(high.sample(0, 0, 0), 3.0);
}

#[test]
fn range_choice_and_interval_select_boundaries() {
    let engine = engine();
    let reg = registry(&engine);
    let choice = reg
        .compile_value(&json!({"type": "minecraft:range_choice", "input": 0.5,
                    "min_inclusive": 0.5, "max_exclusive": 1.5,
                    "when_in_range": 1, "when_out_of_range": 2}))
        .unwrap();
    assert_eq!(choice.sample(0, 0, 0), 1.0);
    let upper = reg
        .compile_value(&json!({"type": "minecraft:range_choice", "input": 1.5,
                    "min_inclusive": 0.5, "max_exclusive": 1.5,
                    "when_in_range": 1, "when_out_of_range": 2}))
        .unwrap();
    assert_eq!(upper.sample(0, 0, 0), 2.0);

    let select = reg
        .compile_value(&json!({"type": "minecraft:interval_select", "input": 0.0,
                    "thresholds": [0.0, 1.0], "functions": [10, 20, 30]}))
        .unwrap();
    // Java picks the first threshold strictly above the input.
    assert_eq!(select.sample(0, 0, 0), 20.0);
    let last = reg
        .compile_value(&json!({"type": "minecraft:interval_select", "input": 5.0,
                    "thresholds": [0.0, 1.0], "functions": [10, 20, 30]}))
        .unwrap();
    assert_eq!(last.sample(0, 0, 0), 30.0);
}

#[test]
fn find_interval_start_reproduces_the_java_binary_search() {
    let locations: Vec<f32> = vec![0.0, 1.0, 2.0, 3.0];
    for probe in -8..12 {
        let input = probe as f32 / 2.0;
        let expected = locations
            .iter()
            .position(|location| input < *location)
            .unwrap_or(locations.len()) as i32
            - 1;
        assert_eq!(
            find_interval_start(&locations, input),
            expected,
            "input {input}"
        );
    }
}

#[test]
fn spline_hermite_and_linear_extension() {
    // Straight segment with matching endpoint derivatives stays linear,
    // including outside the node range via the derivative extension.
    let spline = Spline::Multipoint {
        coordinate: identity(Axis::X),
        locations: vec![0.0, 2.0].into(),
        derivatives: vec![1.0, 1.0].into(),
        values: vec![Spline::Constant(1.0), Spline::Constant(3.0)].into(),
    };
    let node = Density(Rc::new(Node::Spline(spline)));
    assert_eq!(node.sample(0, 0, 0), 1.0);
    assert_eq!(node.sample(1, 0, 0), 2.0);
    assert_eq!(node.sample(2, 0, 0), 3.0);
    assert_eq!(node.sample(-5, 0, 0), -4.0);
    assert_eq!(node.sample(10, 0, 0), 11.0);

    // Zero derivative at the last node makes the extension constant.
    let flat = Spline::Multipoint {
        coordinate: Density::constant(-1.0),
        locations: vec![0.0, 1.0].into(),
        derivatives: vec![0.0, 3.0].into(),
        values: vec![Spline::Constant(2.0), Spline::Constant(5.0)].into(),
    };
    let flat = Density(Rc::new(Node::Spline(flat)));
    assert_eq!(flat.sample(0, 0, 0), 2.0);

    let above = Spline::Multipoint {
        coordinate: Density::constant(2.0),
        locations: vec![0.0, 1.0].into(),
        derivatives: vec![0.0, 3.0].into(),
        values: vec![Spline::Constant(2.0), Spline::Constant(5.0)].into(),
    };
    let above = Density(Rc::new(Node::Spline(above)));
    assert_eq!(above.sample(0, 0, 0), 8.0);
}

#[test]
fn interpolated_point_path_matches_trilinear_lerp() {
    let inner = identity(Axis::X);
    let interpolated = Density(Rc::new(Node::Interpolated {
        input: inner,
        cell_size_xz: 2,
        cell_size_y: 2,
        corners: RefCell::new(BoundedCache::new(64)),
    }));
    // Cell corners pass through the direct sample.
    assert_eq!(interpolated.sample(0, 0, 0), 0.0);
    assert_eq!(interpolated.sample(2, 2, 2), 2.0);
    // Cell centre: x corners are 0 and 2, so the trilinear blend is 1.
    assert_eq!(interpolated.sample(1, 1, 1), 1.0);
    // Negative coordinates use floor-modulated cell offsets.
    assert_eq!(interpolated.sample(-1, -1, -1), -1.0);
}

#[test]
fn interpolated_corner_cache_is_bounded_and_eviction_is_value_neutral() {
    let input = Density(Rc::new(Node::Binary(
        BinaryKind::Add,
        identity(Axis::X),
        identity(Axis::Y),
    )));
    let node = Density(Rc::new(Node::Interpolated {
        input,
        cell_size_xz: 4,
        cell_size_y: 8,
        corners: RefCell::new(BoundedCache::new(64)),
    }));
    let first = node.sample(-3, 5, -2).to_bits();
    for x in -120..120 {
        let _ = node.sample(x * 4 + 1, 5, 1);
    }
    let Node::Interpolated { corners, .. } = &*node.0 else {
        unreachable!()
    };
    assert!(corners.borrow().entries() <= 128);
    assert_eq!(node.sample(-3, 5, -2).to_bits(), first);
}

#[test]
fn json_child_slots_accept_refs_and_bare_numbers() {
    let mut root = RandomSource::from_world_seed(2026);
    let positional = root.fork_positional();
    let engine = NoiseEngine::new(positional, HashMap::new());
    let documents = HashMap::from([(
        "minecraft:half".to_owned(),
        json!({"type": "minecraft:constant", "value": 0.5}),
    )]);
    let reg = DensityRegistry::new(&engine, documents);
    let compiled = reg
        .compile_value(&json!({"type": "minecraft:mul", "left": "minecraft:half", "right": 4}))
        .unwrap();
    assert_eq!(compiled.sample(0, 0, 0), 2.0);
    // The referenced document resolves to the same compiled subtree.
    let direct = reg.compile(&"minecraft:half".to_owned()).unwrap();
    assert_eq!(direct.sample(0, 0, 0), 0.5);
}

#[test]
fn cyclic_and_excessively_deep_density_references_are_rejected() {
    let engine = engine();
    let mut documents = HashMap::from([
        (
            "test:a".to_owned(),
            json!({"type": "minecraft:add", "left": "test:b", "right": 1}),
        ),
        (
            "test:b".to_owned(),
            json!({"type": "minecraft:add", "left": "test:a", "right": 1}),
        ),
        ("test:good".to_owned(), json!(3)),
    ]);
    for index in 0..=MAX_REFERENCE_DEPTH {
        documents.insert(
            format!("test:depth_{index}"),
            json!({"type": "minecraft:add", "left": format!("test:depth_{}", index + 1), "right": 0}),
        );
    }
    let registry = DensityRegistry::new(&engine, documents);
    assert!(matches!(
        registry.compile(&"test:a".to_owned()),
        Err(DensityError::Parse(message)) if message.contains("cyclic")
    ));
    assert!(matches!(
        registry.compile(&"test:depth_0".to_owned()),
        Err(DensityError::Parse(message)) if message.contains("depth")
    ));
    assert_eq!(
        registry
            .compile(&"test:good".to_owned())
            .unwrap()
            .sample(0, 0, 0),
        3.0
    );
}

#[test]
fn gradient_coordinates_reject_truncation_and_extreme_samples_do_not_overflow() {
    let engine = engine();
    let registry = registry(&engine);
    let gradient = |from: i64, to: i64| {
        json!({"type": "minecraft:gradient", "axis": "x",
               "from_coordinate": from, "to_coordinate": to,
               "from_value": 0.0, "to_value": 4.0})
    };
    for invalid in [
        gradient(0, 4_294_967_296),
        gradient(i64::from(i32::MIN), i64::from(i32::MAX)),
        gradient(0, 0),
    ] {
        assert!(matches!(
            registry.compile_value(&invalid),
            Err(DensityError::Parse(_))
        ));
    }
    let valid = registry.compile_value(&gradient(-2, 2)).unwrap();
    assert_eq!(valid.sample(i32::MIN, 0, 0), 0.0);
    assert_eq!(valid.sample(i32::MAX, 0, 0), 4.0);
}

#[test]
fn builtin_y_gradient_and_shift_wiring() {
    let engine = engine_with_noise();
    let reg = registry(&engine);
    let y = reg.compile(&"minecraft:y".to_owned()).unwrap();
    // Identity within the packed-Y range, clamped outside it.
    assert_eq!(y.sample(0, 100, 0), 100.0);
    assert_eq!(y.sample(0, -100, 0), -100.0);
    assert_eq!(y.sample(0, 4062, 0), 4062.0);
    assert_eq!(y.sample(0, 5000, 0), 4062.0);
    assert_eq!(y.sample(0, -5000, 0), -4064.0);

    let shift_x = reg.compile(&"minecraft:shift_x".to_owned()).unwrap();
    let first = shift_x.sample(64, 100, -64);
    assert!(first.is_finite());
    // World-level caching: repeated evaluation is deterministic.
    assert_eq!(first, shift_x.sample(64, 100, -64));
    let shift_z = reg.compile(&"minecraft:shift_z".to_owned()).unwrap();
    assert!(shift_z.sample(64, 100, -64).is_finite());
}

#[test]
fn noise_node_shares_one_stack_per_id() {
    let engine = engine_with_noise();
    let reg = registry(&engine);
    let first = reg
        .compile_value(
            &json!({"type": "minecraft:noise", "noise": "minecraft:shift",
                    "xz_scale": 0.25, "y_scale": 0.125}),
        )
        .unwrap();
    let second = reg
        .compile_value(
            &json!({"type": "minecraft:noise", "noise": "minecraft:shift",
                    "xz_scale": 0.5, "y_scale": 0.25}),
        )
        .unwrap();
    let value = first.sample(100, 60, -100);
    assert!(value.is_finite());
    // World-level caching: the second graph reuses the same noise stack.
    assert_eq!(engine.stacks.borrow().len(), 1);
    let shared = Rc::clone(engine.stacks.borrow().get("minecraft:shift").unwrap());
    assert_eq!(second.sample(100, 60, -100), shared.get(50.0, 15.0, -50.0));
}

#[test]
fn pow_folds_constant_exponents_at_parse_time() {
    let engine = engine();
    let reg = registry(&engine);
    let square = reg
        .compile_value(&json!({"type": "minecraft:pow", "base": 4.0, "exponent": 2.0}))
        .unwrap();
    assert_eq!(square.sample(0, 0, 0), 16.0);
    let negative = reg
        .compile_value(&json!({"type": "minecraft:pow", "base": 4.0, "exponent": -2.0}))
        .unwrap();
    assert_eq!(negative.sample(0, 0, 0), 1.0 / 16.0);
    let sqrt = reg
        .compile_value(&json!({"type": "minecraft:pow", "base": 9.0, "exponent": 0.5}))
        .unwrap();
    assert_eq!(sqrt.sample(0, 0, 0), 3.0);
    let general = reg
        .compile_value(&json!({"type": "minecraft:pow", "base": 4.0, "exponent": 2.5}))
        .unwrap();
    assert_eq!(general.sample(0, 0, 0), (4.0f64).powf(2.5) as f32);
    let pass = reg
        .compile_value(&json!({"type": "minecraft:pow", "base": 7.0, "exponent": 1.0}))
        .unwrap();
    assert_eq!(pass.sample(0, 0, 0), 7.0);
}

#[test]
fn old_blended_noise_builds_and_validates_parameters() {
    let engine = engine();
    let reg = registry(&engine);
    let blended = reg
        .compile_value(
            &json!({"type": "minecraft:old_blended_noise", "xz_scale": 0.25, "y_scale": 0.125,
                    "xz_factor": 80.0, "y_factor": 160.0, "smear_scale_multiplier": 8.0}),
        )
        .unwrap();
    let value = blended.sample(100, 50, -100);
    assert!(value.is_finite());

    let rejected = reg.compile_value(
        &json!({"type": "minecraft:old_blended_noise", "xz_scale": 0.0005, "y_scale": 0.125,
                "xz_factor": 80.0, "y_factor": 160.0, "smear_scale_multiplier": 8.0}),
    );
    assert!(matches!(rejected, Err(DensityError::Parse(_))));
}

#[test]
fn context_nodes_use_the_blend_free_defaults() {
    let engine = engine();
    let reg = registry(&engine);
    assert_eq!(
        reg.compile_value(&json!({"type": "minecraft:blend_alpha"}))
            .unwrap()
            .sample(0, 0, 0),
        1.0
    );
    assert_eq!(
        reg.compile_value(&json!({"type": "minecraft:blend_offset"}))
            .unwrap()
            .sample(0, 0, 0),
        0.0
    );
    assert_eq!(
        reg.compile_value(&json!({"type": "minecraft:beardifier"}))
            .unwrap()
            .sample(0, 0, 0),
        0.0
    );
    let through = reg
        .compile_value(&json!({"type": "minecraft:blend_density", "input": 1.25}))
        .unwrap();
    assert_eq!(through.sample(0, 0, 0), 1.25);
}

#[test]
fn find_top_surface_descends_to_the_lower_bound_exactly() {
    // Density is positive only at the lowest cell position, one above
    // lower_bound; the search must reach it, not stop one cell early.
    let positive_at = Density(Rc::new(Node::RangeChoice {
        input: identity(Axis::Y),
        min_inclusive: 1.0,
        max_exclusive: 2.0,
        when_in_range: Density::constant(1.0),
        when_out_of_range: Density::constant(-1.0),
    }));
    let top = Density(Rc::new(Node::FindTopSurface {
        density: positive_at,
        upper_bound: Density::constant(10.0),
        lower_bound: 0,
        cell_height: 1,
    }));
    assert_eq!(top.sample(0, 0, 0), 1.0);

    // Never positive: the search bottoms out at lower_bound itself.
    let none = Density(Rc::new(Node::FindTopSurface {
        density: Density::constant(-1.0),
        upper_bound: Density::constant(10.0),
        lower_bound: 3,
        cell_height: 2,
    }));
    assert_eq!(none.sample(0, 0, 0), 3.0);

    // upper_bound at or below the lower bound short-circuits.
    let floor = Density(Rc::new(Node::FindTopSurface {
        density: Density::constant(1.0),
        upper_bound: Density::constant(2.0),
        lower_bound: 3,
        cell_height: 2,
    }));
    assert_eq!(floor.sample(0, 0, 0), 3.0);
}

/// Bit-for-bit parity against the real vanilla 26.3 density-function
/// engine. Each vector is a pure (noise-free) density JSON tree plus
/// sample points; the expected column is the raw f32 bit pattern
/// (as signed i32) produced by vanilla's own compile+sample pipeline
/// with caches disabled. Vectors and expectations are generated by the
/// local knowledge-only consultation harness recorded in docs/PROVENANCE.md.
/// Bit-for-bit equality, except that any two NaNs match. IEEE 754 leaves
/// the sign and payload of a NaN from an invalid operation (`0/0`,
/// `sqrt(-1)`) to the hardware: x86-64 produces `0xffc00000`, AArch64
/// `0x7fc00000`. The vanilla vectors were recorded on x86-64, and nothing in
/// generation inspects NaN bits, so only "is NaN" is part of parity.
fn same_density_bits(got: i32, want: i32) -> bool {
    got == want || (f32::from_bits(got as u32).is_nan() && f32::from_bits(want as u32).is_nan())
}

#[test]
fn density_bits_match_across_nan_encodings_only() {
    let x86_nan = 0xffc0_0000_u32 as i32;
    let arm_nan = 0x7fc0_0000_u32 as i32;
    assert!(same_density_bits(x86_nan, arm_nan));
    assert!(same_density_bits(
        1.5f32.to_bits() as i32,
        1.5f32.to_bits() as i32
    ));
    // Every non-NaN value, including signed zero, must still match exactly.
    assert!(!same_density_bits(
        0.0f32.to_bits() as i32,
        (-0.0f32).to_bits() as i32
    ));
    assert!(!same_density_bits(arm_nan, 0.0f32.to_bits() as i32));
    assert!(!same_density_bits(
        1.0f32.to_bits() as i32,
        (1.0f32.to_bits() + 1) as i32
    ));
}

#[test]
fn vanilla_parity_vectors_match_bit_for_bit() {
    let vectors: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../testdata/density_parity_vectors.json")).unwrap();
    let mut expected = HashMap::<(String, i32, i32, i32), i32>::new();
    for line in include_str!("../testdata/density_parity_expected.txt").lines() {
        let mut it = line.split_whitespace();
        let name = it.next().unwrap().to_owned();
        let x: i32 = it.next().unwrap().parse().unwrap();
        let y: i32 = it.next().unwrap().parse().unwrap();
        let z: i32 = it.next().unwrap().parse().unwrap();
        let bits: i32 = it.next().unwrap().parse().unwrap();
        expected.insert((name, x, y, z), bits);
    }

    let engine = engine();
    let reg = registry(&engine);
    let mut mismatches = Vec::new();
    let mut checked = 0usize;
    for vector in &vectors {
        let name = vector["name"].as_str().unwrap().to_owned();
        let density = reg
            .compile_value(&vector["function"])
            .unwrap_or_else(|error| panic!("{name}: rust parse: {error}"));
        for point in vector["points"].as_array().unwrap() {
            let p = point.as_array().unwrap();
            let x = p[0].as_i64().unwrap() as i32;
            let y = p[1].as_i64().unwrap() as i32;
            let z = p[2].as_i64().unwrap() as i32;
            let got = density.sample(x, y, z).to_bits() as i32;
            let want = *expected
                .get(&(name.clone(), x, y, z))
                .unwrap_or_else(|| panic!("no expectation for {name} at ({x},{y},{z})"));
            checked += 1;
            if !same_density_bits(got, want) {
                mismatches.push(format!(
                    "{name} ({x},{y},{z}): rust {:08x}, vanilla {:08x}",
                    got as u32, want as u32
                ));
            }
        }
    }
    assert_eq!(
        checked,
        expected.len(),
        "expectation table not fully consumed"
    );
    assert!(
        mismatches.is_empty(),
        "{} parity mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

/// Bit-for-bit parity for the block-volume evaluation path, captured
/// from the real 26.3 `sampleVolume` pipeline by the same knowledge-only
/// consultation harness as the point vectors. Pins the interpolated
/// fast path, block-step fill (reciprocal-multiply alphas and the
/// vertical repeated-addition accumulation), the non-unit-step gather,
/// nested interpolated recursion, and the volume forms of mul/div/min
/// that intentionally differ from the point samplers.
#[test]
fn vanilla_volume_parity_vectors_match_bit_for_bit() {
    let vectors: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../testdata/density_volume_parity_vectors.json"
    ))
    .unwrap();
    let mut expected = HashMap::<(String, i32, i32, i32), i32>::new();
    for line in include_str!("../testdata/density_volume_parity_expected.txt").lines() {
        let mut it = line.split_whitespace();
        let name = it.next().unwrap().to_owned();
        let x: i32 = it.next().unwrap().parse().unwrap();
        let y: i32 = it.next().unwrap().parse().unwrap();
        let z: i32 = it.next().unwrap().parse().unwrap();
        let bits: i32 = it.next().unwrap().parse().unwrap();
        expected.insert((name, x, y, z), bits);
    }

    let engine = engine();
    let reg = registry(&engine);
    let mut mismatches = Vec::new();
    let mut checked = 0usize;
    for vector in &vectors {
        let name = vector["name"].as_str().unwrap().to_owned();
        let density = reg
            .compile_value(&vector["function"])
            .unwrap_or_else(|error| panic!("{name}: rust parse: {error}"));
        let volume_spec = &vector["volume"];
        let ints = |key: &str| {
            volume_spec[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap() as i32)
                .collect::<Vec<_>>()
                .try_into()
                .unwrap()
        };
        let volume = DensityVolume::new(ints("size"), ints("min"), ints("step"))
            .unwrap_or_else(|| panic!("{name}: invalid volume"));
        let mut buffer = vec![0.0f32; volume.slot_count()];
        density.sample_volume(&volume, &mut buffer);
        for z in 0..volume.size[2] {
            for x in 0..volume.size[0] {
                for y in 0..volume.size[1] {
                    let got =
                        buffer[volume.index(x as usize, y as usize, z as usize)].to_bits() as i32;
                    let want = *expected
                        .get(&(name.clone(), x, y, z))
                        .unwrap_or_else(|| panic!("no expectation for {name} at ({x},{y},{z})"));
                    checked += 1;
                    if !same_density_bits(got, want) {
                        mismatches.push(format!(
                            "{name} ({x},{y},{z}): rust {:08x}, vanilla {:08x}",
                            got as u32, want as u32
                        ));
                    }
                }
            }
        }
    }
    assert_eq!(
        checked,
        expected.len(),
        "expectation table not fully consumed"
    );
    assert!(
        mismatches.is_empty(),
        "{} volume parity mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn volume_matches_point_sample_bit_for_bit_on_synthetic_graph() {
    let engine = engine_with_noise();
    let reg = registry(&engine);
    let graph = reg
        .compile_value(&json!({
            "type": "minecraft:min",
            "left": {
                "type": "minecraft:max",
                "left": {
                    "type": "minecraft:add",
                    "left": {
                        "type": "minecraft:interpolated",
                        "cell_size_xz": 4,
                        "cell_size_y": 8,
                        "input": {
                            "type": "minecraft:mul",
                            "left": {
                                "type": "minecraft:cache",
                                "input": {
                                    "type": "minecraft:noise",
                                    "noise": "minecraft:shift",
                                    "xz_scale": 0.25,
                                    "y_scale": 0.0
                                }
                            },
                            "right": {
                                "type": "minecraft:noise",
                                "noise": "minecraft:shift",
                                "xz_scale": 0.5,
                                "y_scale": 0.125
                            }
                        }
                    },
                    "right": 0.25
                },
                "right": -0.5
            },
            "right": 0.75
        }))
        .unwrap();
    let volume = DensityVolume::new([16, 32, 16], [-16, -16, 16], [1, 1, 1]).unwrap();
    let mut buffer = vec![0.0f32; volume.slot_count()];
    graph.sample_volume(&volume, &mut buffer);
    for z in 0..16 {
        for x in 0..16 {
            for y in 0..32 {
                let wx = -16 + x;
                let wy = -16 + y;
                let wz = 16 + z;
                let got = buffer[volume.index(x as usize, y as usize, z as usize)];
                let want = graph.sample(wx, wy, wz);
                assert_eq!(
                    got.to_bits(),
                    want.to_bits(),
                    "mismatch at ({wx},{wy},{wz}): volume={got} ({:08x}) point={want} ({:08x})",
                    got.to_bits(),
                    want.to_bits()
                );
            }
        }
    }
}
