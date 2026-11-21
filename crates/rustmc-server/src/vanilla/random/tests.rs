//! Unit tests for `vanilla::random`.

use super::*;

/// Why: an exact-pin refresh of `md-5` must not change decoration seeds,
/// so the MD5 digests and the hash-named seed halves below were captured
/// from the 0.10.6 stack before the bump and are asserted unchanged. The
/// inputs cover the empty value, a partial block, one whole MD5 block, a
/// multi-block name, and a 4 KiB payload.
#[test]
fn digests_match_the_0_10_stack() {
    for (len, expected) in [
        (0usize, "d41d8cd98f00b204e9800998ecf8427e"),
        (1, "0cc175b9c0f1b6a831c399e269772661"),
        (64, "a2eaf6295c32adc403865fd96a2f182b"),
        (200, "32cce8c4f2bf6f04dbb71b5cb9e37c30"),
        (4_096, "ab6bc685710814791f9d9a4570514525"),
    ] {
        let input: Vec<u8> = (0..len).map(|i| b'a' + (i % 26) as u8).collect();
        let mut hasher = Md5::new();
        hasher.update(&input);
        assert_eq!(hex(&hasher.finalize()), expected, "{len} bytes");
    }
    for (name, expected_lo, expected_hi) in [
        ("", 0xd41d_8cd9_8f00_b204, 0xe980_0998_ecf8_427e),
        (
            "minecraft:continentalness",
            0x8388_6c9d_0ae3_a662,
            0xafa6_38a6_1b42_e8ad,
        ),
        (
            "minecraft:clay_bands",
            0x1656_8f44_636e_3bc9,
            0xc1fd_d673_95e0_296c,
        ),
        (
            "minecraft:terrain",
            0x1ee5_5522_2ef9_6f14,
            0xe2be_dfdb_ebe4_3d33,
        ),
    ] {
        let seed = seed_from_hash_of(name);
        assert_eq!(seed.lo, expected_lo, "hash seed lo for {name:?}");
        assert_eq!(seed.hi, expected_hi, "hash seed hi for {name:?}");
    }
    // The salted construction the positional factory performs: MD5 of the
    // name XOR-folded into the factory state of world seed 2026.
    let factory = RandomSource::from_world_seed(2026).fork_positional();
    assert_eq!(factory.lo, 0x3a6e_71c4_5539_8e64);
    assert_eq!(factory.hi, 0x7779_c4ab_560b_df3a);
    for (name, expected_lo, expected_hi) in [
        (
            "minecraft:continentalness",
            0xb9e6_1d59_5fda_2806,
            0xd8df_fc0d_4d49_3797,
        ),
        (
            "minecraft:clay_bands",
            0x2c38_fe80_3657_b5ad,
            0xb684_12d8_c3eb_f656,
        ),
    ] {
        let source = factory.from_hash_of(name);
        assert_eq!(source.core.lo, expected_lo, "salted lo for {name:?}");
        assert_eq!(source.core.hi, expected_hi, "salted hi for {name:?}");
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn xoroshiro_stream_matches_vanilla_parity_vector() {
    // Generated 2026-09-30 by executing the owner's local deobfuscated
    // Java 26.3 classes; see docs/PROVENANCE.md consultation log.
    let mut source = RandomSource::from_world_seed(2026);
    let expected: [u64; 5] = [
        0x3a6e_71c4_5539_8e64,
        0x7779_c4ab_560b_df3a,
        0xc27e_ab91_bfbd_337e,
        0x25f1_de73_2d18_faae,
        0x5366_bbc9_670a_a824,
    ];
    for value in expected {
        assert_eq!(source.next_long(), value);
    }
    let factory = source.fork_positional();
    assert_eq!(factory.lo, 2433090872870611415);
    assert_eq!(factory.hi, 6199706540704153528);
}

#[test]
fn zero_seed_falls_back_to_the_golden_silver_pair() {
    let source = RandomSource::from_seed_128bit(Seed128Bit { lo: 0, hi: 0 });
    assert_eq!(source.core.lo, GOLDEN_RATIO_64);
    assert_eq!(source.core.hi, SILVER_RATIO_64);
}

#[test]
fn bounded_draw_is_deterministic_and_in_range() {
    let mut source = RandomSource::from_world_seed(7);
    for _ in 0..10_000 {
        assert!(source.next_int_bounded(256) < 256);
    }
}

/// Raw-stream parity against the deobfuscated 26.3
/// `LegacyRandomSource`/`SingleThreadedRandomSource`, generated with
/// `java -cp inner.jar` on 2026-10-01 (facts recorded in
/// `docs/PROVENANCE.md` session 9). One continuous draw sequence.
#[test]
fn legacy_bounded_streams_match_parity_vectors() {
    let mut r = LegacyRandom::new(2026);
    let bound_16: Vec<i32> = (0..8).map(|_| r.next_int_bounded(16)).collect();
    assert_eq!(bound_16, [9, 3, 13, 2, 12, 3, 8, 9]);
    let bound_237: Vec<i32> = (0..5).map(|_| r.next_int_bounded(237)).collect();
    assert_eq!(bound_237, [28, 234, 177, 92, 9]);
    let bound_28: Vec<i32> = (0..5).map(|_| r.next_int_bounded(28)).collect();
    assert_eq!(bound_28, [10, 10, 6, 6, 0]);
    // A non-power-of-two bound exercises the rejection loop.
    let bound_15: Vec<i32> = (0..5).map(|_| r.next_int_bounded(15)).collect();
    assert_eq!(bound_15, [3, 7, 1, 6, 3]);
}

#[test]
fn legacy_float_and_long_streams_match_parity_vectors() {
    let mut r = LegacyRandom::new(2026);
    let floats: Vec<u32> = (0..4).map(|_| r.next_float().to_bits()).collect();
    assert_eq!(floats, [1058954935, 1045075720, 1062823368, 1043807924]);
    let mut r = LegacyRandom::new(2026);
    let longs: Vec<u64> = (0..4).map(|_| r.next_long() as u64).collect();
    assert_eq!(
        longs,
        [
            0x9e5e_b7ef_32a5_c2c5,
            0xd965_c8bb_2dcf_ad64,
            0xca56_9a92_32e5_1d6d,
            0x8794_36e9_9c38_92b9,
        ]
    );
}

#[test]
fn large_feature_seed_matches_parity_vectors() {
    for (index, expected) in [
        (0i64, [0xc137_b17b_u32, 0xee13_e7a9]),
        (1, [0xa06e_93c3, 0x483c_27c0]),
        (2, [0x64ec_13e2, 0x29aa_7d22]),
    ] {
        let mut r = LegacyRandom::new(1);
        r.set_large_feature_seed(2026 + index, -16, -13);
        assert_eq!(r.next_bits(32) as u32, expected[0], "seed {index} word 0");
        assert_eq!(r.next_bits(32) as u32, expected[1], "seed {index} word 1");
    }
    let mut r = LegacyRandom::new(1);
    r.set_large_feature_seed(2026, 40, -17);
    assert_eq!(r.next_bits(32) as u32, 0x0c64_b921);
    assert_eq!(r.next_bits(32) as u32, 0x61ae_4307);
}

/// Decoration-seed parity against the deobfuscated 26.3
/// `WorldgenRandom`/`XoroshiroRandomSource`, generated with
/// `java -cp server-26.3.jar:libraries/*` on 2026-10-05 and recorded in
/// `docs/PROVENANCE.md`. The chunk coordinates are the decoration
/// origin, i.e. the chunk position shifted left by four.
#[test]
fn decoration_seed_matches_parity_vectors() {
    for ((chunk_x, chunk_z), expected) in [
        ((0, 0), 2026_i64),
        ((-1, 2), -6_273_982_822_019_529_638),
        ((40, -17), 7_892_031_622_368_679_610),
        ((-128, -128), -7_560_045_051_374_004_246),
    ] {
        let mut random = DecorationRandom::new(12_345);
        let seed = random.set_decoration_seed(2026, chunk_x << 4, chunk_z << 4);
        assert_eq!(seed, expected, "decoration seed for {chunk_x},{chunk_z}");
    }
}

/// The draw sequence a single ore decoration makes: `nextFloat` for the
/// vein angle, `nextInt(16)` for the in-square offsets, `nextInt(3)` for
/// the two height jitters, then `nextDouble` for each vein point. Expected
/// values come from the same 2026-10-05 reference run as the seeds above and
/// are recorded in `docs/PROVENANCE.md`.
#[test]
fn decoration_draw_sequence_matches_parity_vectors() {
    let mut random = DecorationRandom::new(12_345);
    let seed = random.set_decoration_seed(2026, -16, 32);
    random.set_feature_seed(seed, 3, 6);
    // The reference run draws one of each shape per round, in this order.
    let mut floats = Vec::new();
    let mut square = Vec::new();
    let mut jitter = Vec::new();
    let mut doubles = Vec::new();
    for _ in 0..3 {
        floats.push(random.next_float());
        square.push(random.next_int_bounded(16));
        jitter.push(random.next_int_bounded(3));
        doubles.push(random.next_double().to_bits());
    }
    assert_eq!(floats, [0.665_943_2, 0.530_540_35, 0.203_523_93]);
    assert_eq!(square, [8, 0, 2]);
    assert_eq!(jitter, [1, 1, 0]);
    assert_eq!(
        doubles,
        [
            4_605_220_165_685_422_423_u64,
            4_605_330_769_301_753_330,
            4_599_212_402_979_728_424,
        ]
    );
}

/// The composite shapes: `nextInt`, the non-power-of-two bounded draw,
/// `nextLong` (two delegate longs) and `nextBoolean` (one). Expected values
/// come from the same 2026-10-05 reference run and are recorded in
/// `docs/PROVENANCE.md`.
#[test]
fn decoration_composite_draws_match_parity_vectors() {
    let mut random = DecorationRandom::new(12_345);
    let seed = random.set_decoration_seed(2026, -2048, -2048);
    random.set_feature_seed(seed, 17, 6);
    let mut ints = Vec::new();
    let mut bounded = Vec::new();
    let mut longs = Vec::new();
    let mut bools = Vec::new();
    for _ in 0..3 {
        ints.push(random.next_int());
        bounded.push(random.next_int_bounded(237));
        longs.push(random.next_long());
        bools.push(random.next_bool());
    }
    assert_eq!(ints, [1_389_737_797, -218_130_023, 1_811_641_268]);
    assert_eq!(bounded, [209, 53, 69]);
    assert_eq!(
        longs,
        [
            -7_926_701_679_838_353_624,
            -5_402_565_863_842_856_637,
            -1_590_638_501_234_764_267
        ]
    );
    assert_eq!(bools, [true, true, true]);
}

/// A feature's stream is a pure function of its ordinal and step, so
/// reseeding the same feature twice repeats the sequence exactly.
#[test]
fn feature_seed_streams_are_independent_and_repeatable() {
    for ordinal in [0, 3, 17] {
        let mut first = DecorationRandom::new(1);
        let mut second = DecorationRandom::new(-999);
        let seed = first.set_decoration_seed(2027, 64, -64);
        second.set_decoration_seed(2027, 64, -64);
        first.set_feature_seed(seed, ordinal, 6);
        second.set_feature_seed(seed, ordinal, 6);
        let left: Vec<i32> = (0..8).map(|_| first.next_int_bounded(16)).collect();
        let right: Vec<i32> = (0..8).map(|_| second.next_int_bounded(16)).collect();
        assert_eq!(left, right, "ordinal {ordinal}");
    }
    let mut a = DecorationRandom::new(1);
    let seed = a.set_decoration_seed(2027, 64, -64);
    a.set_feature_seed(seed, 4, 6);
    let mut b = DecorationRandom::new(1);
    b.set_decoration_seed(2027, 64, -64);
    b.set_feature_seed(seed, 5, 6);
    assert_ne!(
        (0..8).map(|_| a.next_float()).collect::<Vec<f32>>(),
        (0..8).map(|_| b.next_float()).collect::<Vec<f32>>()
    );
}
