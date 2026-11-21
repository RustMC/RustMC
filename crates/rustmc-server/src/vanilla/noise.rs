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

const GRADIENT: [(f32, f32, f32); 16] = [
    (1.0, 1.0, 0.0),
    (-1.0, 1.0, 0.0),
    (1.0, -1.0, 0.0),
    (-1.0, -1.0, 0.0),
    (1.0, 0.0, 1.0),
    (-1.0, 0.0, 1.0),
    (1.0, 0.0, -1.0),
    (-1.0, 0.0, -1.0),
    (0.0, 1.0, 1.0),
    (0.0, -1.0, 1.0),
    (0.0, 1.0, -1.0),
    (0.0, -1.0, -1.0),
    (1.0, 1.0, 0.0),
    (0.0, -1.0, 1.0),
    (-1.0, 1.0, 0.0),
    (0.0, -1.0, -1.0),
];

/// Keeps coordinates inside the exact-integer band of double arithmetic.
fn wrap(x: f64) -> f64 {
    if (-HALF_ROUND_OFF..HALF_ROUND_OFF).contains(&x) {
        x
    } else {
        x - (x / ROUND_OFF + 0.5).floor() * ROUND_OFF
    }
}

/// Perlin coordinates are wrapped into the exact integer range before this
/// conversion. Truncation plus a negative-fraction correction avoids the
/// platform libm `floor` call on every noise sample.
#[inline]
fn lattice_floor(x: f64) -> i32 {
    let truncated = x as i32;
    truncated - i32::from(x < f64::from(truncated))
}

/// `value.floor() as i32` without the libm call: truncate, then step down
/// when truncation rounded a negative fraction up. Saturation and NaN match
/// the `as` cast (NaN gives 0; out-of-range values give `i32::MIN`/`MAX`).
#[inline]
fn floor_to_i32(value: f64) -> i32 {
    let truncated = value as i32;
    if f64::from(truncated) > value {
        truncated.saturating_sub(1)
    } else {
        truncated
    }
}

fn grad_dot(hash: i32, x: f32, y: f32, z: f32) -> f32 {
    let g = GRADIENT[(hash & 15) as usize];
    g.0 * x + g.1 * y + g.2 * z
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

struct Lerp3Values {
    x000: f32,
    x100: f32,
    x010: f32,
    x110: f32,
    x001: f32,
    x101: f32,
    x011: f32,
    x111: f32,
}

fn lerp3(a1: f32, a2: f32, a3: f32, values: Lerp3Values) -> f32 {
    lerp(
        a3,
        lerp2(a1, a2, values.x000, values.x100, values.x010, values.x110),
        lerp2(a1, a2, values.x001, values.x101, values.x011, values.x111),
    )
}

struct SamplePoint {
    x: i32,
    y: i32,
    z: i32,
    relative_x: f32,
    relative_y: f32,
    relative_z: f32,
    original_relative_y: f32,
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
        let floor_x = lattice_floor(x);
        let floor_y = lattice_floor(y);
        let floor_z = lattice_floor(z);
        let relative_x = (x - floor_x as f64) as f32;
        let relative_y = (y - floor_y as f64) as f32;
        let relative_z = (z - floor_z as f64) as f32;
        self.sample_and_lerp(SamplePoint {
            x: floor_x,
            y: floor_y,
            z: floor_z,
            relative_x,
            relative_y,
            relative_z,
            original_relative_y: relative_y,
        })
    }

    /// Point evaluation of the y-smeared Perlin variant used by the base-3D
    /// blended noise: the gradient corners use a quantized y fraction while
    /// the interpolation alpha keeps the original one.
    pub fn get_smeared(&self, _x: f64, _y: f64, _z: f64, fudge_y_scale: f64) -> f32 {
        let x = wrap(_x) + self.offset_x;
        let y = wrap(_y) + self.offset_y;
        let z = wrap(_z) + self.offset_z;
        let floor_x = lattice_floor(x);
        let floor_y = lattice_floor(y);
        let floor_z = lattice_floor(z);
        let relative_x = (x - floor_x as f64) as f32;
        let relative_y = y - floor_y as f64;
        let relative_z = (z - floor_z as f64) as f32;
        let fudged = (relative_y - compute_fudge_y(_y, relative_y, fudge_y_scale)) as f32;
        self.sample_and_lerp(SamplePoint {
            x: floor_x,
            y: floor_y,
            z: floor_z,
            relative_x,
            relative_y: fudged,
            relative_z,
            original_relative_y: relative_y as f32,
        })
    }

    pub fn get_2d(&self, x: f64, y: f64) -> f32 {
        self.get(wrap(x), 0.0, wrap(y))
    }

    fn sample_and_lerp(&self, point: SamplePoint) -> f32 {
        let SamplePoint {
            x,
            y,
            z,
            relative_x,
            relative_y,
            relative_z,
            original_relative_y,
        } = point;
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
            smoothstep(original_relative_y),
            smoothstep(relative_z),
            Lerp3Values {
                x000: d000,
                x100: d100,
                x010: d010,
                x110: d110,
                x001: d001,
                x101: d101,
                x011: d011,
                x111: d111,
            },
        )
    }
}

/// Y-slice quantization used by the smeared Perlin variant: the largest
/// multiple of `fudge_y_scale` that fits below the fraction (bounded by the
/// original absolute y when it lies inside `[0, relative_y)`), with the
/// `(float)1.0E-7` guard offset.
fn compute_fudge_y(original_y: f64, relative_y: f64, fudge_y_scale: f64) -> f64 {
    let fudge_limit = if original_y >= 0.0 && original_y < relative_y {
        original_y
    } else {
        relative_y
    };
    f64::from(floor_to_i32(
        fudge_limit / fudge_y_scale + f64::from(1.0e-7_f32),
    )) * fudge_y_scale
}

#[derive(Clone)]
struct NoiseLayer {
    noise: PerlinNoise,
    frequency: f64,
    amplitude: f32,
    /// When set, the layer is a y-smeared Perlin with this fudge scale
    /// (used by the base-3D blended noise only).
    smear_y: Option<f64>,
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
            let sample = match layer.smear_y {
                None => layer.noise.get(
                    x * layer.frequency,
                    y * layer.frequency,
                    z * layer.frequency,
                ),
                Some(fudge) => layer.noise.get_smeared(
                    x * layer.frequency,
                    y * layer.frequency,
                    z * layer.frequency,
                    fudge,
                ),
            };
            value += layer.amplitude * sample;
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
                smear_y: None,
            });
            let mut second_source = second_random.from_hash_of(&seed);
            layers.push(NoiseLayer {
                noise: PerlinNoise::new(&mut second_source),
                frequency: octave.frequency * INPUT_FACTOR,
                amplitude: value_factor,
                smear_y: None,
            });
        }
        NoiseStack { layers }
    }
}

/// Builds one blended-noise FBM: `octave_count = -first_octave + 1` smeared
/// Perlin layers whose spatial factor halves and value factor doubles per
/// step, consumed sequentially from `random`. The value factor is normalized
/// by `2^octave_count − 1` before iteration; all layer coefficients follow
/// the documented vanilla construction captured in `docs/PROVENANCE.md`.
pub fn create_blended_fbm(
    random: &mut RandomSource,
    first_octave: i32,
    smear_scale_y: f64,
    mut value_factor: f64,
) -> NoiseStack {
    let octave_count = -first_octave + 1;
    value_factor /= 2.0f64.powi(octave_count) - 1.0;
    let mut layers = Vec::with_capacity(octave_count as usize);
    let mut factor = 1.0f64;
    for _ in (0..octave_count).rev() {
        layers.push(NoiseLayer {
            noise: PerlinNoise::new(random),
            frequency: factor,
            amplitude: value_factor as f32,
            smear_y: Some(smear_scale_y * factor),
        });
        factor /= 2.0;
        value_factor *= 2.0;
    }
    NoiseStack { layers }
}

#[cfg(test)]
mod tests;
