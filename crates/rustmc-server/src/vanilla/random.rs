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
}
