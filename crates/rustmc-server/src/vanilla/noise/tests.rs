//! Unit tests for `vanilla::noise`.

use super::*;

#[test]
fn floor_to_i32_matches_floor_cast_everywhere() {
    let mut values = vec![
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        0.0,
        -0.0,
        f64::from(i32::MAX) + 10.0,
        f64::from(i32::MIN) - 10.0,
        f64::from(i32::MIN) - 0.5,
        f64::from(i32::MIN),
        f64::from(i32::MAX),
    ];
    for integer in [-1_000_000, -4096, -2, -1, 0, 1, 2, 4096, 1_000_000] {
        for fraction in [0.0, 1.0e-9, 0.25, 0.5, 0.999_999_999] {
            values.push(f64::from(integer) + fraction);
            values.push(f64::from(integer) - fraction);
        }
    }
    for value in values {
        assert_eq!(floor_to_i32(value), value.floor() as i32, "{value}");
    }
}

#[test]
fn lattice_floor_matches_floor_inside_wrapped_coordinate_band() {
    for integer in [-16_777_216, -4096, -1, 0, 1, 4096, 16_777_216] {
        for fraction in [0.0, 0.125, 0.5, 0.999_999] {
            let value = f64::from(integer) + fraction;
            assert_eq!(lattice_floor(value), value.floor() as i32);
            let negative = f64::from(integer) - fraction;
            assert_eq!(lattice_floor(negative), negative.floor() as i32);
        }
    }
}

const POINTS: [(f64, f64, f64); 4] = [
    (0.5, 0.5, 0.5),
    (100.25, -20.75, 300.125),
    (12345.678, 64.0, -9876.543),
    (-5000.5, 10.25, 777.75),
];

fn harness_factory() -> crate::vanilla::random::PositionalRandomFactory {
    let mut source = RandomSource::from_world_seed(2026);
    for _ in 0..5 {
        let _ = source.next_long();
    }
    source.fork_positional()
}

fn continentalness() -> NoiseParameters {
    NoiseParameters {
        base_amplitude: 0.8880832896205223,
        base_octave: -9,
        octave_count: 9,
        normalize: true,
        amplitude_modifiers: vec![1.0, 1.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0],
    }
}

fn erosion() -> NoiseParameters {
    NoiseParameters {
        base_amplitude: 1.0,
        base_octave: -7,
        octave_count: 9,
        normalize: true,
        amplitude_modifiers: vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
    }
}

#[test]
fn continentalness_matches_vanilla_parity_vectors() {
    // Raw f32 bit patterns captured 2026-09-30 by executing the owner's
    // local deobfuscated Java 26.3 noise classes on these exact inputs;
    // see docs/PROVENANCE.md consultation log.
    let factory = harness_factory();
    let mut source = factory.from_hash_of("minecraft:continentalness");
    let noise = NormalNoise::new(&continentalness()).create(&mut source);
    let expected_3d: [u32; 4] = [0x3e7e_6fc2, 0xbe12_f36b, 0xbf31_2f9d, 0xbdc1_837a];
    let expected_2d: [u32; 4] = [0x3e78_fba6, 0xbc87_6a9e, 0xbf39_8cc7, 0xbdba_0345];
    for (point, (bits_3d, bits_2d)) in POINTS.iter().zip(expected_3d.iter().zip(expected_2d)) {
        let (x, y, z) = *point;
        assert_eq!(
            noise.get(x, y, z).to_bits(),
            *bits_3d,
            "3D sample at ({x}, {y}, {z})"
        );
        assert_eq!(
            noise.get_2d(x, z).to_bits(),
            bits_2d,
            "2D sample at ({x}, {z})"
        );
    }
}

#[test]
fn erosion_matches_vanilla_parity_vectors_with_zeroed_octaves() {
    // Same capture run as the continentalness vectors above.
    let factory = harness_factory();
    let mut source = factory.from_hash_of("minecraft:erosion");
    let noise = NormalNoise::new(&erosion()).create(&mut source);
    let expected_2d: [u32; 4] = [0xbdb6_9d34, 0x3c9f_f257, 0xbe57_c0a7, 0xbe35_b28f];
    for (point, bits) in POINTS.iter().zip(expected_2d) {
        let (x, _, z) = *point;
        assert_eq!(noise.get_2d(x, z).to_bits(), bits, "2D at ({x}, {z})");
    }
}

#[test]
fn stack_evaluation_is_independent_of_construction_order_in_one_run() {
    let factory = harness_factory();
    let mut source_a = factory.from_hash_of("minecraft:continentalness");
    let noise_a = NormalNoise::new(&continentalness()).create(&mut source_a);
    let mut source_b = factory.from_hash_of("minecraft:continentalness");
    let noise_b = NormalNoise::new(&continentalness()).create(&mut source_b);
    for (x, y, z) in POINTS {
        assert_eq!(noise_a.get(x, y, z), noise_b.get(x, y, z));
    }
}

#[test]
fn continentalness_internals_match_vanilla_capture() {
    // Intermediate state captured 2026-09-30 from the owner's local
    // deobfuscated Java 26.3 classes on this exact construction path;
    // see docs/PROVENANCE.md consultation log.
    let factory = harness_factory();
    let mut source = factory.from_hash_of("minecraft:continentalness");
    let first_random = source.fork_positional();
    let mut octave_source = first_random.from_hash_of("octave_-9");
    let perlin = PerlinNoise::new(&mut octave_source);
    assert_eq!(perlin.offset_x.to_bits(), 0x4044_0bb9_0ecb_1348);
    assert_eq!(perlin.offset_y.to_bits(), 0x406b_1625_61f1_3097);
    assert_eq!(perlin.offset_z.to_bits(), 0x402c_5f36_d4c3_5fe0);
    assert_eq!(
        &perlin.perms[..16],
        &[
            161, 227, 19, 56, 126, 143, 100, 135, 171, 16, 114, 181, 125, 79, 3, 210
        ]
    );
    let freq = 2.0f64.powi(-9);
    assert_eq!(
        perlin.get(0.5 * freq, 0.5 * freq, 0.5 * freq).to_bits(),
        0x3e5d_dc2e
    );
    assert_eq!(perlin.get_2d(0.5 * freq, 0.5 * freq).to_bits(), 0x3e5f_89df);

    let nn = NormalNoise::new(&continentalness());
    let amps: Vec<u64> = nn.octaves.iter().map(|o| o.amplitude.to_bits()).collect();
    assert_eq!(
        amps,
        [
            0x3fdc_796a_5ace_d1d1,
            0x3fcc_796a_5ace_d1d1,
            0x3fcc_796a_5ace_d1d1,
            0x3fbc_796a_5ace_d1d1,
            0x3fac_796a_5ace_d1d1,
            0x3f8c_796a_5ace_d1d1,
            0x3f7c_796a_5ace_d1d1,
            0x3f6c_796a_5ace_d1d1,
            0x3f5c_796a_5ace_d1d1,
        ]
    );
    let freqs: Vec<u64> = nn.octaves.iter().map(|o| o.frequency.to_bits()).collect();
    assert_eq!(freqs[0], 0x3f60_0000_0000_0000); // 2^-9
    assert_eq!(freqs[8], 0x3fe0_0000_0000_0000); // 2^-1
    let target = stream_sum(nn.octaves.iter().map(|o| o.amplitude.abs()));
    assert_eq!(target.to_bits(), 0x3ff1_52de_74bf_5427);
    let mut variance = 0.0f64;
    for octave in &nn.octaves {
        let layer_deviation = PERLIN_STANDARD_DEVIATION * octave.amplitude.abs();
        variance += layer_deviation * layer_deviation;
    }
    assert_eq!(variance.sqrt().to_bits(), 0x3fc3_570b_ca8d_ef42);
    assert_eq!(nn.normalization_factor.to_bits(), 0x3ffb_0645_2030_9512);
}

#[test]
fn stream_sum_matches_jdk_compensated_accumulation() {
    // Ground truth from `DoubleStream.sum()` on a live Java runtime,
    // 2026-09-30 (docs/PROVENANCE.md consultation log).
    let cont_amps: Vec<f64> = [
        0x3fdc_796a_5ace_d1d1_u64,
        0x3fcc_796a_5ace_d1d1,
        0x3fcc_796a_5ace_d1d1,
        0x3fbc_796a_5ace_d1d1,
        0x3fac_796a_5ace_d1d1,
        0x3f8c_796a_5ace_d1d1,
        0x3f7c_796a_5ace_d1d1,
        0x3f6c_796a_5ace_d1d1,
        0x3f5c_796a_5ace_d1d1,
    ]
    .iter()
    .map(|&bits| f64::from_bits(bits))
    .collect();
    assert_eq!(
        stream_sum(cont_amps.iter().copied()).to_bits(),
        0x3ff1_52de_74bf_5427
    );
    let mut tiny = vec![1.0f64];
    tiny.extend(std::iter::repeat_n(1e-16, 20));
    assert_eq!(stream_sum(tiny).to_bits(), 0x3ff0_0000_0000_0009);
    assert_eq!(
        stream_sum([
            1.0, -1.0, 1e-16, -1.0, 1.0, 1e-16, 0.5, -0.5, 3e-17, 1.0, -1.0, 1e-16
        ])
        .to_bits(),
        0x3cb7_34ac_a5f6_226f
    );
    assert_eq!(
        stream_sum([0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]).to_bits(),
        0x4012_0000_0000_0000
    );
    assert_eq!(
        stream_sum([
            3.5e200, -1.2e199, 7.7e-300, 1.0, -1.0, 2.5, 1e-310, 5e-324, 123456.789
        ])
        .to_bits(),
        0x6991_a9ad_4ffc_664f
    );
    // Overflow-to-NaN path returns the cross checksum.
    assert_eq!(
        stream_sum([
            1e308,
            1.0,
            1e-308,
            1e308,
            -1e308,
            42.0,
            1e-16,
            3.0,
            f64::MAX
        ])
        .to_bits(),
        0x7ff0_0000_0000_0000
    );
}
