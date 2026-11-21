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
    let digest: [u8; 16] = hasher.finalize().into();
    let [
        a0,
        a1,
        a2,
        a3,
        a4,
        a5,
        a6,
        a7,
        b0,
        b1,
        b2,
        b3,
        b4,
        b5,
        b6,
        b7,
    ] = digest;
    let lo = u64::from_be_bytes([a0, a1, a2, a3, a4, a5, a6, a7]);
    let hi = u64::from_be_bytes([b0, b1, b2, b3, b4, b5, b6, b7]);
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
    /// the vanilla bounded `nextInt`, including its rejection path. Vanilla
    /// rejects a zero bound; here it yields 0 without a draw, so malformed
    /// data cannot stop the server.
    pub fn next_int_bounded(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
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
    /// with `int` wrapping arithmetic. Java rejects a bound below 1; here it
    /// yields 0 without a draw, so malformed data cannot stop the server.
    pub fn next_int_bounded(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            return 0;
        }
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
mod tests;
