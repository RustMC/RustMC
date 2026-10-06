//! Bit-exact reimplementation of the vanilla worldgen random source.
//!
//! Behavior was established by knowledge-only consultation under ADR-0014
//! (as amended) and is pinned by parity vectors recorded in
//! `docs/PROVENANCE.md`; the tests in this module assert raw bit patterns.

use md5::{Digest, Md5};

/// Java: `RandomSupport.GOLDEN_RATIO_64` (`-7046029254386353131` as `i64`).
const GOLDEN_RATIO_64: u64 = 0x9E37_79B9_7F4A_7C15;
/// Java: `RandomSupport.SILVER_RATIO_64` (`7640891576956012809` as `i64`).
const SILVER_RATIO_64: u64 = 0x6A09_E667_F3BC_C909;

/// A 128-bit worldgen seed split into two halves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seed128Bit {
    pub lo: u64,
    pub hi: u64,
}

impl Seed128Bit {
    /// Stafford13 finalizer (Murmur3-style splitmix avalanche).
    fn mix_stafford13(mut z: u64) -> u64 {
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn mixed(self) -> Self {
        Self {
            lo: Self::mix_stafford13(self.lo),
            hi: Self::mix_stafford13(self.hi),
        }
    }

    fn xor(self, other: Self) -> Self {
        Self {
            lo: self.lo ^ other.lo,
            hi: self.hi ^ other.hi,
        }
    }
}

/// Expand a signed 64-bit world seed into a mixed 128-bit seed.
pub fn upgrade_seed_to_128bit(legacy_seed: u64) -> Seed128Bit {
    let lo = legacy_seed ^ SILVER_RATIO_64;
    let hi = lo.wrapping_add(GOLDEN_RATIO_64);
    Seed128Bit { lo, hi }.mixed()
}

/// Derive a 128-bit seed from the MD5 digest of a UTF-8 name; the first
/// digest half becomes `lo` and the second becomes `hi`, both big-endian.
pub fn seed_from_hash_of(input: &str) -> Seed128Bit {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let lo = u64::from_be_bytes(digest[0..8].try_into().expect("md5 prefix"));
    let hi = u64::from_be_bytes(digest[8..16].try_into().expect("md5 suffix"));
    Seed128Bit { lo, hi }
}

#[derive(Clone)]
struct Xoroshiro128PlusPlus {
    lo: u64,
    hi: u64,
}

impl Xoroshiro128PlusPlus {
    fn new(seed: Seed128Bit) -> Self {
        let mut state = Self {
            lo: seed.lo,
            hi: seed.hi,
        };
        if state.lo | state.hi == 0 {
            state.lo = GOLDEN_RATIO_64;
            state.hi = SILVER_RATIO_64;
        }
        state
    }

    fn next_long(&mut self) -> u64 {
        let s0 = self.lo;
        let mut s1 = self.hi;
        let result = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
        s1 ^= s0;
        self.lo = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
        self.hi = s1.rotate_left(28);
        result
    }
}

/// The xoroshiro128++-backed random source used by modern worldgen.
#[derive(Clone)]
pub struct RandomSource {
    core: Xoroshiro128PlusPlus,
}

/// Java declares `DOUBLE_UNIT` as a `double` field initialized from the
/// `1.110223E-16F` float literal; `nextBits(53) * DOUBLE_UNIT` then runs in
/// `double`, so the float constant must be widened, not the multiply.
const DOUBLE_UNIT: f64 = 1.110223e-16_f32 as f64;
const FLOAT_UNIT: f32 = 5.9604645e-8;

impl RandomSource {
    pub fn from_world_seed(seed: u64) -> Self {
        Self::from_seed_128bit(upgrade_seed_to_128bit(seed))
    }

    pub fn from_seed_128bit(seed: Seed128Bit) -> Self {
        Self {
            core: Xoroshiro128PlusPlus::new(seed),
        }
    }

    pub fn next_long(&mut self) -> u64 {
        self.core.next_long()
    }

    /// Low 32 bits of the next long, matching Java's `(int) nextLong()`.
    pub fn next_int(&mut self) -> u32 {
        (self.next_long() & 0xFFFF_FFFF) as u32
    }

    /// Lemire's multiply-shift-with-rejection draw; bit-for-bit identical to
    /// the vanilla bounded `nextInt`, including its rejection path.
    pub fn next_int_bounded(&mut self, bound: u32) -> u32 {
        assert!(bound > 0, "bound must be positive");
        let bound = u64::from(bound);
        let mut product = u64::from(self.next_int()).wrapping_mul(bound);
        let mut fractional = product & 0xFFFF_FFFF;
        if fractional < bound {
            let threshold = (1u64 << 32).wrapping_sub(bound) % bound;
            while fractional < threshold {
                product = u64::from(self.next_int()).wrapping_mul(bound);
                fractional = product & 0xFFFF_FFFF;
            }
        }
        (product >> 32) as u32
    }

    fn next_bits(&mut self, bits: u32) -> u64 {
        self.next_long() >> (64 - bits)
    }

    pub fn next_double(&mut self) -> f64 {
        self.next_bits(53) as f64 * DOUBLE_UNIT
    }

    /// 26.3 `nextBoolean()` is `(nextLong() & 1) != 0`: the low bit of the
    /// next long, not a one-bit draw.
    pub fn next_bool(&mut self) -> bool {
        self.next_long() & 1 != 0
    }

    pub fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 * FLOAT_UNIT
    }

    /// Derive an independent stream, consuming two longs.
    pub fn fork(&mut self) -> Self {
        Self::from_seed_128bit(Seed128Bit {
            lo: self.next_long(),
            hi: self.next_long(),
        })
    }

    /// Derive a positional factory, consuming two longs.
    pub fn fork_positional(&mut self) -> PositionalRandomFactory {
        PositionalRandomFactory {
            lo: self.next_long(),
            hi: self.next_long(),
        }
    }
}

/// Seed factory that derives per-position and per-name streams without
/// advancing a shared cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionalRandomFactory {
    lo: u64,
    hi: u64,
}

impl PositionalRandomFactory {
    pub fn from_hash_of(&self, name: &str) -> RandomSource {
        let factory_seed = Seed128Bit {
            lo: self.lo,
            hi: self.hi,
        };
        RandomSource::from_seed_128bit(seed_from_hash_of(name).xor(factory_seed))
    }

    pub fn from_seed(&self, seed: u64) -> RandomSource {
        RandomSource::from_seed_128bit(Seed128Bit {
            lo: seed ^ self.lo,
            hi: seed ^ self.hi,
        })
    }

    /// Position-mixed stream for a block position (Java `Mth.getSeed`).
    pub fn at(&self, x: i32, y: i32, z: i32) -> RandomSource {
        RandomSource::from_seed_128bit(Seed128Bit {
            lo: position_seed(x, y, z) ^ self.lo,
            hi: self.hi,
        })
    }
}

/// Java: `Mth.getSeed(int x, int y, int z)`; the `x` product is a wrapping
/// `int` multiply before sign extension.
fn position_seed(x: i32, y: i32, z: i32) -> u64 {
    let seed = (x.wrapping_mul(3129871) as i64) ^ (z as i64).wrapping_mul(116129781) ^ (y as i64);
    let seed = seed
        .wrapping_mul(seed)
        .wrapping_mul(42317861)
        .wrapping_add(seed.wrapping_mul(11));
    (seed >> 16) as u64
}

/// The 48-bit legacy LCG behind Java's `LegacyRandomSource`,
/// `SingleThreadedRandomSource`, and `WorldgenRandom` — the stream every
/// registry carver draws from (see `docs/PROVENANCE.md` session 9).
#[derive(Clone)]
pub struct LegacyRandom {
    seed: u64,
}

const SEED_MASK: u64 = (1 << 48) - 1;
const SEED_UNA: u64 = 0x0005_DEEC_E66D;
const SEED_STEP: u64 = 11;

impl LegacyRandom {
    pub fn new(seed: i64) -> Self {
        let mut this = Self { seed: 0 };
        this.set_seed(seed);
        this
    }

    /// `setSeed`: mix with the multiplier constant, keep 48 bits.
    pub fn set_seed(&mut self, seed: i64) {
        self.seed = (seed as u64 ^ SEED_UNA) & SEED_MASK;
    }

    /// Advance and return the top `bits` (Java `next(int)`; results for
    /// `bits < 32` are non-negative, `bits == 32` may be negative).
    pub fn next_bits(&mut self, bits: u32) -> i32 {
        self.seed = self.seed.wrapping_mul(SEED_UNA).wrapping_add(SEED_STEP) & SEED_MASK;
        (self.seed >> (48 - bits)) as i32
    }

    pub fn next_int(&mut self) -> i32 {
        self.next_bits(32)
    }

    /// Java's bounded draw: power-of-two shortcut else the rejection loop
    /// with `int` wrapping arithmetic.
    pub fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");
        if bound & (bound - 1) == 0 {
            return ((bound as i64 * i64::from(self.next_bits(31))) >> 31) as i32;
        }
        loop {
            let sample = self.next_bits(31);
            let modulo = sample % bound;
            if !sample
                .wrapping_sub(modulo)
                .wrapping_add(bound - 1)
                .is_negative()
            {
                return modulo;
            }
        }
    }

    /// Java: `(next(32) << 32) + next(32)` with the halves sign-extended.
    pub fn next_long(&mut self) -> i64 {
        let upper = i64::from(self.next_bits(32)) << 32;
        let lower = i64::from(self.next_bits(32));
        upper.wrapping_add(lower)
    }

    pub fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 * FLOAT_UNIT
    }

    /// `WorldgenRandom.setLargeFeatureSeed`: reseed, draw two scale longs,
    /// reseed from `x·a ^ z·b ^ seed`. Returns nothing; the stream is left
    /// exactly where Java leaves it.
    pub fn set_large_feature_seed(&mut self, seed: i64, chunk_x: i32, chunk_z: i32) {
        self.set_seed(seed);
        let x_scale = self.next_long();
        let z_scale = self.next_long();
        let mixed = (i64::from(chunk_x)).wrapping_mul(x_scale)
            ^ (i64::from(chunk_z)).wrapping_mul(z_scale)
            ^ seed;
        self.set_seed(mixed);
    }
}

/// The stream the decoration loop draws through: Java's `WorldgenRandom`
/// keeps the derived primitive shapes of `LegacyRandomSource` but forwards
/// every `next(bits)` to an xoroshiro delegate, so each `bits` request costs
/// exactly one delegate long while the composite draws (`nextLong`,
/// `nextDouble`) cost two. That is a different contract from
/// [`RandomSource`]'s own derived draws, which take one long each, and the
/// decoration order depends on it (`docs/PROVENANCE.md`, the feature-runtime
/// consultation).
#[derive(Clone)]
pub struct DecorationRandom {
    delegate: RandomSource,
}

impl DecorationRandom {
    /// A stream whose delegate is reseeded from `seed`, matching the
    /// constructor's state being irrelevant after `set_decoration_seed`.
    pub fn new(seed: i64) -> Self {
        Self {
            delegate: RandomSource::from_world_seed(seed as u64),
        }
    }

    /// Java `setSeed`: the xoroshiro delegate re-derives both halves from the
    /// single 64-bit value.
    pub fn set_seed(&mut self, seed: i64) {
        self.delegate = RandomSource::from_world_seed(seed as u64);
    }

    /// Java `next(int)`: one delegate long, logically shifted so the top
    /// `bits` bits are the value, then truncated to `int`.
    pub fn next_bits(&mut self, bits: u32) -> i32 {
        debug_assert!(bits > 0 && bits <= 32, "bits must be in 1..=32");
        (self.delegate.next_long() >> (64 - bits)) as u32 as i32
    }

    pub fn next_int(&mut self) -> i32 {
        self.next_bits(32)
    }

    /// `BitRandomSource`'s bounded draw: the power-of-two shortcut, else the
    /// rejection loop, both expressed through `next`.
    pub fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");
        if bound & (bound - 1) == 0 {
            return ((bound as i64 * i64::from(self.next_bits(31))) >> 31) as i32;
        }
        loop {
            let sample = self.next_bits(31);
            let modulo = sample % bound;
            if !sample
                .wrapping_sub(modulo)
                .wrapping_add(bound - 1)
                .is_negative()
            {
                return modulo;
            }
        }
    }

    /// Java: `(next(32) << 32) + next(32)` with both halves sign-extended,
    /// i.e. two delegate longs.
    pub fn next_long(&mut self) -> i64 {
        let upper = i64::from(self.next_bits(32)) << 32;
        upper.wrapping_add(i64::from(self.next_bits(32)))
    }

    pub fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 * FLOAT_UNIT
    }

    /// Java: `(next(26) << 27) + next(27)` scaled by the double multiplier;
    /// two delegate longs, unlike [`RandomSource::next_double`].
    pub fn next_double(&mut self) -> f64 {
        let upper = (i64::from(self.next_bits(26))) << 27;
        upper.wrapping_add(i64::from(self.next_bits(27))) as f64 * DOUBLE_UNIT
    }

    pub fn next_bool(&mut self) -> bool {
        self.next_bits(1) != 0
    }

    /// `WorldgenRandom.setDecorationSeed`: two odd scale longs mixed with the
    /// chunk origin and the world seed. Returns the seed the per-feature
    /// reseeds are based on.
    pub fn set_decoration_seed(&mut self, world_seed: i64, x: i32, z: i32) -> i64 {
        self.set_seed(world_seed);
        let x_scale = self.next_long() | 1;
        let z_scale = self.next_long() | 1;
        let mixed = (i64::from(x))
            .wrapping_mul(x_scale)
            .wrapping_add(i64::from(z).wrapping_mul(z_scale))
            ^ world_seed;
        self.set_seed(mixed);
        mixed
    }

    /// `WorldgenRandom.setFeatureSeed`: the feature's own stream starts from
    /// the decoration seed plus its ordinal and step, without drawing.
    pub fn set_feature_seed(&mut self, decoration_seed: i64, ordinal: i32, step: i32) {
        self.set_seed(
            decoration_seed
                .wrapping_add(i64::from(ordinal))
                .wrapping_add(i64::from(step).wrapping_mul(10_000)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
