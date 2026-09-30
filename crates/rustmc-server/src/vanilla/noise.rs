//! Bit-exact reimplementation of the vanilla gradient-noise core.
//!
//! Octave stacking, normalization, and positional seeding follow the
//! behavior established under the ADR-0014 (as amended) consultation and
//! are pinned by parity vectors recorded in `docs/PROVENANCE.md`.

use crate::vanilla::random::RandomSource;

/// Java: `GradientNoise.HALF_ROUND_OFF == Math.nextDown(1.6777216E7)`.
const HALF_ROUND_OFF: f64 = 1.6777215999999996e7;
/// Java: `GradientNoise.ROUND_OFF == 33554432` (2^25).
const ROUND_OFF: f64 = 3.3554432e7;

const GRADIENT: [(i32, i32, i32); 16] = [
    (1, 1, 0),
    (-1, 1, 0),
    (1, -1, 0),
    (-1, -1, 0),
    (1, 0, 1),
    (-1, 0, 1),
    (1, 0, -1),
    (-1, 0, -1),
    (0, 1, 1),
    (0, -1, 1),
    (0, 1, -1),
    (0, -1, -1),
    (1, 1, 0),
    (0, -1, 1),
    (-1, 1, 0),
    (0, -1, -1),
];

/// Keeps coordinates inside the exact-integer band of double arithmetic.
fn wrap(x: f64) -> f64 {
    if (-HALF_ROUND_OFF..HALF_ROUND_OFF).contains(&x) {
        x
    } else {
        x - (x / ROUND_OFF + 0.5).floor() * ROUND_OFF
    }
}

fn grad_dot(hash: i32, x: f32, y: f32, z: f32) -> f32 {
    let g = GRADIENT[(hash & 15) as usize];
    g.0 as f32 * x + g.1 as f32 * y + g.2 as f32 * z
}

fn smoothstep(x: f32) -> f32 {
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

fn lerp(alpha: f32, from: f32, to: f32) -> f32 {
    from + alpha * (to - from)
}

fn lerp2(a1: f32, a2: f32, x00: f32, x10: f32, x01: f32, x11: f32) -> f32 {
    lerp(a2, lerp(a1, x00, x10), lerp(a1, x01, x11))
}

#[allow(clippy::too_many_arguments)]
fn lerp3(
    a1: f32,
    a2: f32,
    a3: f32,
    x000: f32,
    x100: f32,
    x010: f32,
    x110: f32,
    x001: f32,
    x101: f32,
    x011: f32,
    x111: f32,
) -> f32 {
    lerp(
        a3,
        lerp2(a1, a2, x000, x100, x010, x110),
        lerp2(a1, a2, x001, x101, x011, x111),
    )
}

#[derive(Clone)]
pub struct PerlinNoise {
    perms: [u8; 256],
    offset_x: f64,
    offset_y: f64,
    offset_z: f64,
}

impl PerlinNoise {
    pub fn new(random: &mut RandomSource) -> Self {
        let offset_x = random.next_double() * 256.0;
        let offset_y = random.next_double() * 256.0;
        let offset_z = random.next_double() * 256.0;
        let mut perms = [0u8; 256];
        for (i, slot) in perms.iter_mut().enumerate() {
            *slot = i as u8;
        }
        for i in 0..256 {
            let offset = random.next_int_bounded(256 - i as u32) as usize;
            perms.swap(i, i + offset);
        }
        Self {
            perms,
            offset_x,
            offset_y,
            offset_z,
        }
    }

    fn permute(&self, x: i32) -> i32 {
        self.perms[(x & 0xFF) as usize] as i32
    }

    pub fn get(&self, _x: f64, _y: f64, _z: f64) -> f32 {
        let x = wrap(_x) + self.offset_x;
        let y = wrap(_y) + self.offset_y;
        let z = wrap(_z) + self.offset_z;
        let floor_x = x.floor() as i32;
        let floor_y = y.floor() as i32;
        let floor_z = z.floor() as i32;
        let relative_x = (x - floor_x as f64) as f32;
        let relative_y = (y - floor_y as f64) as f32;
        let relative_z = (z - floor_z as f64) as f32;
        self.sample_and_lerp(
            floor_x, floor_y, floor_z, relative_x, relative_y, relative_z,
        )
    }

    pub fn get_2d(&self, x: f64, y: f64) -> f32 {
        self.get(wrap(x), 0.0, wrap(y))
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_and_lerp(
        &self,
        x: i32,
        y: i32,
        z: i32,
        relative_x: f32,
        relative_y: f32,
        relative_z: f32,
    ) -> f32 {
        let x0 = self.permute(x);
        let x1 = self.permute(x.wrapping_add(1));
        let xy00 = self.permute(x0.wrapping_add(y));
        let xy01 = self.permute(x0.wrapping_add(y).wrapping_add(1));
        let xy10 = self.permute(x1.wrapping_add(y));
        let xy11 = self.permute(x1.wrapping_add(y).wrapping_add(1));
        let d000 = grad_dot(
            self.permute(xy00.wrapping_add(z)),
            relative_x,
            relative_y,
            relative_z,
        );
        let d100 = grad_dot(
            self.permute(xy10.wrapping_add(z)),
            relative_x - 1.0,
            relative_y,
            relative_z,
        );
        let d010 = grad_dot(
            self.permute(xy01.wrapping_add(z)),
            relative_x,
            relative_y - 1.0,
            relative_z,
        );
        let d110 = grad_dot(
            self.permute(xy11.wrapping_add(z)),
            relative_x - 1.0,
            relative_y - 1.0,
            relative_z,
        );
        let d001 = grad_dot(
            self.permute(xy00.wrapping_add(z).wrapping_add(1)),
            relative_x,
            relative_y,
            relative_z - 1.0,
        );
        let d101 = grad_dot(
            self.permute(xy10.wrapping_add(z).wrapping_add(1)),
            relative_x - 1.0,
            relative_y,
            relative_z - 1.0,
        );
        let d011 = grad_dot(
            self.permute(xy01.wrapping_add(z).wrapping_add(1)),
            relative_x,
            relative_y - 1.0,
            relative_z - 1.0,
        );
        let d111 = grad_dot(
            self.permute(xy11.wrapping_add(z).wrapping_add(1)),
            relative_x - 1.0,
            relative_y - 1.0,
            relative_z - 1.0,
        );
        lerp3(
            smoothstep(relative_x),
            smoothstep(relative_y),
            smoothstep(relative_z),
            d000,
            d100,
            d010,
            d110,
            d001,
            d101,
            d011,
            d111,
        )
    }
}

#[derive(Clone)]
struct NoiseLayer {
    noise: PerlinNoise,
    frequency: f64,
    amplitude: f32,
}

/// A weighted sum of Perlin layers, evaluated with `f32` accumulation.
#[derive(Clone)]
pub struct NoiseStack {
    layers: Vec<NoiseLayer>,
}

impl NoiseStack {
    pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        let mut value = 0.0f32;
        for layer in &self.layers {
            value += layer.amplitude
                * layer.noise.get(
                    x * layer.frequency,
                    y * layer.frequency,
                    z * layer.frequency,
                );
        }
        value
    }

    pub fn get_2d(&self, x: f64, y: f64) -> f32 {
        let mut value = 0.0f32;
        for layer in &self.layers {
            value += layer.amplitude * layer.noise.get_2d(x * layer.frequency, y * layer.frequency);
        }
        value
    }
}

/// The worldgen noise definition parsed from a `worldgen/noise` data file.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseParameters {
    pub base_amplitude: f64,
    pub base_octave: i32,
    pub octave_count: usize,
    pub normalize: bool,
    pub amplitude_modifiers: Vec<f64>,
}

impl NoiseParameters {
    /// Defaults mirror the data-file codec: base amplitude 1.0, one octave,
    /// normalization enabled, no explicit modifiers.
    pub fn new(base_octave: i32) -> Self {
        Self {
            base_amplitude: 1.0,
            base_octave,
            octave_count: 1,
            normalize: true,
            amplitude_modifiers: Vec::new(),
        }
    }
}

struct OctaveInfo {
    index: i32,
    frequency: f64,
    amplitude: f64,
}

/// Java: `NormalNoise.INPUT_FACTOR`.
const INPUT_FACTOR: f64 = 1.0181268882175227;
/// Java: `PerlinNoise.STANDARD_DEVIATION`.
const PERLIN_STANDARD_DEVIATION: f64 = 0.2702247831245211;
/// Java: `NormalNoise.TARGET_DEVIATION`.
const TARGET_DEVIATION: f64 = 0.3333333333333333;

/// Reproduces the exact summation of a sequential `DoubleStream.sum()`
/// (Kahan compensated accumulation with a cross-checksum fallback for the
/// NaN-plus-infinite case), matching how vanilla totals octave amplitudes.
/// Verified bit-for-bit against a live Java runtime on eight adversarial
/// vectors; see the `docs/PROVENANCE.md` consultation log.
fn stream_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut sum = 0.0f64;
    let mut compensation = 0.0f64;
    let mut cross_checksum = 0.0f64;
    for value in values {
        let residual_input = value - compensation;
        let new_sum = sum + residual_input;
        compensation = (new_sum - sum) - residual_input;
        sum = new_sum;
        cross_checksum += value;
    }
    let result = sum - compensation;
    if result.is_nan() && cross_checksum.is_infinite() {
        cross_checksum
    } else {
        result
    }
}

pub struct NormalNoise {
    octaves: Vec<OctaveInfo>,
    normalization_factor: f64,
}

impl NormalNoise {
    pub fn new(parameters: &NoiseParameters) -> Self {
        let octaves = Self::build_octaves(parameters);
        let target_amplitude = stream_sum(octaves.iter().map(|octave| octave.amplitude.abs()));
        let normalization_factor = Self::compute_normalization_factor(target_amplitude, &octaves);
        Self {
            octaves,
            normalization_factor,
        }
    }

    fn build_octaves(parameters: &NoiseParameters) -> Vec<OctaveInfo> {
        let count = parameters.octave_count;
        let mut frequency = 2.0f64.powi(parameters.base_octave);
        let mut amplitude = if parameters.normalize {
            let octaves = count as i32;
            parameters.base_amplitude
                * (0.5f64.powi(-(octaves - 1)) / (0.5f64.powi(-octaves) - 1.0))
        } else {
            parameters.base_amplitude
        };
        let mut octaves = Vec::with_capacity(count);
        for index in 0..count {
            let modifier = if parameters.amplitude_modifiers.is_empty() {
                1.0
            } else {
                parameters.amplitude_modifiers[index]
            };
            if modifier != 0.0 {
                octaves.push(OctaveInfo {
                    index: parameters.base_octave + index as i32,
                    frequency,
                    amplitude: amplitude * modifier,
                });
            }
            frequency *= 2.0;
            amplitude *= 0.5;
        }
        octaves
    }

    fn compute_normalization_factor(target_amplitude: f64, octaves: &[OctaveInfo]) -> f64 {
        let mut variance = 0.0;
        for octave in octaves {
            let layer_deviation = PERLIN_STANDARD_DEVIATION * octave.amplitude.abs();
            variance += layer_deviation * layer_deviation;
        }
        let input_deviation = variance.sqrt();
        if input_deviation == 0.0 {
            return 0.0;
        }
        (target_amplitude * TARGET_DEVIATION) / (input_deviation * 2.0f64.sqrt())
    }

    /// Instantiates the two-octave-layer Perlin stack for this noise; the
    /// caller's stream is advanced by the two positional forks.
    pub fn create(&self, random: &mut RandomSource) -> NoiseStack {
        let first_random = random.fork_positional();
        let second_random = random.fork_positional();
        let mut layers = Vec::with_capacity(self.octaves.len() * 2);
        for octave in &self.octaves {
            let seed = format!("octave_{}", octave.index);
            let value_factor = (self.normalization_factor * octave.amplitude) as f32;
            let mut first_source = first_random.from_hash_of(&seed);
            layers.push(NoiseLayer {
                noise: PerlinNoise::new(&mut first_source),
                frequency: octave.frequency,
                amplitude: value_factor,
            });
            let mut second_source = second_random.from_hash_of(&seed);
            layers.push(NoiseLayer {
                noise: PerlinNoise::new(&mut second_source),
                frequency: octave.frequency * INPUT_FACTOR,
                amplitude: value_factor,
            });
        }
        NoiseStack { layers }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
