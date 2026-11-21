//! Data-driven vanilla density-function pipeline (point evaluation).
//!
//! The node kinds, JSON field names, and numeric semantics follow the
//! publicly documented datapack density-function format; per-operation
//! evaluation order was established under the ADR-0014 (as amended)
//! consultation and is recorded in `docs/PROVENANCE.md` (session 2).
//! Everything here was written independently against those recorded facts.
//!
//! Evaluation has two paths, mirroring vanilla's samplers: point
//! evaluation (`sample` at block coordinates) and block-volume filling
//! (`sample_volume`). The paths are NOT always value-identical: the
//! volume forms of `mul`/`div` skip the point path's left-zero guard,
//! the volume forms of `min`/`max` replace only on a strict comparison
//! (so `min(+0.0, -0.0)` keeps `+0.0` where the point path returns
//! `-0.0`), and the interpolated volume path lerps per axis and fills
//! the vertical direction by repeated addition of a value step. Vanilla
//! world generation fills density caches through the volume path, so
//! the tested node shapes are pinned bit-for-bit by the parity vectors in
//! `docs/PROVENANCE.md` (session 3). Nested volume wrappers around an
//! `interpolated` node still need independent vectors; the fallback point
//! path must not be used to claim complete volume parity.
//! Context nodes (`blend_alpha`, `blend_offset`, `beardifier`,
//! `blend_density`) currently evaluate to their context-free defaults
//! (1, 0, 0, and pass-through); slice C will attach real blender and
//! structure providers.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::OnceLock;

use serde_json::Value;

use crate::vanilla::cache::BoundedCache;
use crate::vanilla::noise::{NoiseParameters, NoiseStack, NormalNoise, create_blended_fbm};
use crate::vanilla::random::{PositionalRandomFactory, RandomSource};

#[derive(Debug)]
pub enum DensityError {
    /// The JSON node is malformed for the documented density format.
    Parse(String),
    /// A string reference did not resolve in the registry.
    Unresolved(String),
    /// The node kind is not implemented yet.
    Unsupported(String),
}

impl std::fmt::Display for DensityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(message) => write!(f, "invalid density JSON: {message}"),
            Self::Unresolved(id) => write!(f, "unresolved density reference: {id}"),
            Self::Unsupported(id) => write!(f, "unsupported density node: {id}"),
        }
    }
}

impl std::error::Error for DensityError {}

/// An identifier in the `minecraft` namespace as used by JSON references.
type Id = String;

/// Ids the registry resolves from code even without a datapack document,
/// matching the built-ins the vanilla bootstrap registers.
pub fn builtin_density_ids() -> &'static [&'static str] {
    &[
        "minecraft:zero",
        "minecraft:y",
        "minecraft:shift_x",
        "minecraft:shift_z",
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    fn parse(value: &str) -> Result<Self, DensityError> {
        match value {
            "x" => Ok(Self::X),
            "y" => Ok(Self::Y),
            "z" => Ok(Self::Z),
            other => Err(DensityError::Parse(format!("unknown axis {other:?}"))),
        }
    }

    fn choose(self, x: i32, y: i32, z: i32) -> i32 {
        match self {
            Self::X => x,
            Self::Y => y,
            Self::Z => z,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tiling {
    ClampToEdge,
    Repeat,
    MirroredRepeat,
}

impl Tiling {
    fn parse(value: &str) -> Result<Self, DensityError> {
        match value {
            "clamp_to_edge" => Ok(Self::ClampToEdge),
            "repeat" => Ok(Self::Repeat),
            "mirrored_repeat" => Ok(Self::MirroredRepeat),
            other => Err(DensityError::Parse(format!("unknown tiling {other:?}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnaryKind {
    Abs,
    Square,
    Cube,
    Sqrt,
    HalfNegative,
    QuarterNegative,
    Reciprocal,
    Negate,
    Squeeze,
    Log,
    Sign,
}

impl UnaryKind {
    fn apply(self, input: f32) -> f32 {
        match self {
            // Java `Math.abs(float)`.
            Self::Abs => input.abs(),
            Self::Square => input * input,
            // Java evaluates `x * x * x` left to right.
            Self::Cube => input * input * input,
            Self::Sqrt => input.sqrt(),
            Self::HalfNegative => leaky(input, 0.5),
            Self::QuarterNegative => leaky(input, 0.25),
            Self::Reciprocal => 1.0 / input,
            Self::Negate => -input,
            Self::Squeeze => {
                let clamped = clamp(input, -1.0, 1.0);
                clamped / 2.0 - clamped * clamped * clamped / 24.0
            }
            Self::Log => input.ln(),
            // Java `Math.signum(float)`: NaN stays NaN and signed zeros
            // map to themselves, unlike Rust's `signum` which returns ±1.
            Self::Sign if input.is_nan() || input == 0.0 => input,
            Self::Sign => input.signum(),
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "abs" => Self::Abs,
            "square" => Self::Square,
            "cube" => Self::Cube,
            "sqrt" => Self::Sqrt,
            "half_negative" => Self::HalfNegative,
            "quarter_negative" => Self::QuarterNegative,
            "reciprocal" => Self::Reciprocal,
            "negate" => Self::Negate,
            "squeeze" => Self::Squeeze,
            "log" => Self::Log,
            "sign" => Self::Sign,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BinaryKind {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
}

impl BinaryKind {
    fn apply(self, left: f32, right: f32) -> f32 {
        match self {
            Self::Add => left + right,
            Self::Sub => left - right,
            // Java's general `mul` sampler short-circuits to +0.0 when the
            // left operand is zero (either sign), so `0 * infinity` is 0.
            Self::Mul => {
                if left == 0.0 {
                    0.0
                } else {
                    left * right
                }
            }
            // The general `div` sampler likewise returns +0.0 for a zero
            // left operand (including 0/0); constant folds bypass it.
            Self::Div => {
                if left == 0.0 {
                    0.0
                } else {
                    left / right
                }
            }
            // Java `Math.min`/`Math.max` propagate NaN and pick -0.0.
            Self::Min => java_min(left, right),
            Self::Max => java_max(left, right),
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "add" => Self::Add,
            "sub" => Self::Sub,
            "mul" => Self::Mul,
            "div" => Self::Div,
            "min" => Self::Min,
            "max" => Self::Max,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RoundKind {
    Floor,
    Round,
    Ceil,
    Truncate,
}

impl RoundKind {
    fn apply(self, input: f32) -> f32 {
        match self {
            Self::Floor => input.floor(),
            // Java `Math.round(float)` is `(int) floor(a + 0.5f)` with the
            // saturation and NaN-to-zero of the narrowing int conversion.
            Self::Round => {
                if input.is_nan() {
                    0.0
                } else {
                    (input + 0.5)
                        .floor()
                        .max(i32::MIN as f32)
                        .min(i32::MAX as f32)
                }
            }
            Self::Ceil => input.ceil(),
            Self::Truncate => {
                if input > 0.0 {
                    input.floor()
                } else {
                    input.ceil()
                }
            }
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "floor" => Self::Floor,
            "round" => Self::Round,
            "ceil" => Self::Ceil,
            "truncate" => Self::Truncate,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DistanceMetric {
    Euclidean,
    EuclideanSquared,
    Manhattan,
    Chebyshev,
}

impl DistanceMetric {
    fn apply(self, dx: f32, dy: f32, dz: f32) -> f32 {
        match self {
            Self::Euclidean => (dx * dx + dy * dy + dz * dz).sqrt(),
            Self::EuclideanSquared => dx * dx + dy * dy + dz * dz,
            Self::Manhattan => dx.abs() + dy.abs() + dz.abs(),
            Self::Chebyshev => dx.abs().max(dy.abs().max(dz.abs())),
        }
    }

    fn parse(value: &str) -> Result<Self, DensityError> {
        match value {
            "euclidean" => Ok(Self::Euclidean),
            "euclidean_squared" => Ok(Self::EuclideanSquared),
            "manhattan" => Ok(Self::Manhattan),
            "chebyshev" => Ok(Self::Chebyshev),
            other => Err(DensityError::Parse(format!(
                "unknown distance metric {other:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShiftKind {
    Xy,
    X,
    Z,
}

/// The three FBM layers of an `old_blended_noise` node.
struct BlendedSet {
    min: NoiseStack,
    max: NoiseStack,
    main: NoiseStack,
    xz_multiplier: f64,
    y_multiplier: f64,
    main_xz_multiplier: f64,
    main_y_multiplier: f64,
}

/// A node in a compiled density graph. Children are shared through `Rc` so
/// registry references collapse to one subtree, mirroring how the vanilla
/// compiler deduplicates resolved function references.
enum Node {
    Constant(f32),
    Noise {
        stack: Rc<NoiseStack>,
        xz_scale: f64,
        y_scale: f64,
        shift_x: Density,
        shift_y: Density,
        shift_z: Density,
    },
    ShiftNoise {
        kind: ShiftKind,
        stack: Rc<NoiseStack>,
        horizontal: RefCell<BoundedCache<[i32; 2], f32>>,
    },
    Axial {
        axis: Axis,
        tiling: Tiling,
        from_coordinate: i32,
        coordinate_range: i32,
        from_value: f32,
        coordinate_factor: f32,
    },
    Unary(UnaryKind, Density),
    Binary(BinaryKind, Density, Density),
    /// `pow` with the constant-exponent special cases folded at compile
    /// time into the same operations Java folds them into.
    PowConstExponent {
        input: Density,
        exponent: f64,
    },
    PowConstBase {
        base: f64,
        exponent: Density,
    },
    Pow(Density, Density),
    Round {
        kind: RoundKind,
        input: Density,
        multiple: Density,
    },
    Clamp {
        input: Density,
        min: f32,
        max: f32,
    },
    Lerp {
        alpha: Density,
        first: Density,
        second: Density,
    },
    RangeChoice {
        input: Density,
        min_inclusive: f32,
        max_exclusive: f32,
        when_in_range: Density,
        when_out_of_range: Density,
    },
    IntervalSelect {
        input: Density,
        thresholds: Rc<[f32]>,
        functions: Rc<[Density]>,
    },
    Spline(Spline),
    /// `minecraft:cache`: the input's values are memoised. When the input is
    /// provably independent of `y` (see [`Density::is_y_independent`]) the
    /// memo is per column; otherwise the node evaluates its input directly.
    Cache {
        input: Density,
        columns: Option<RefCell<BoundedCache<ColumnKey, f32>>>,
    },
    Interpolated {
        input: Density,
        cell_size_xz: i32,
        cell_size_y: i32,
        // Each corner volume is a pure function of its aligned origin and
        // this immutable node. Adjacent block samples share it. Two 64-entry
        // generations cap retained values at 128 cells per node.
        corners: RefCell<BoundedCache<[i32; 3], [f32; 8]>>,
    },
    Slice {
        axis: Axis,
        coordinate: i32,
        input: Density,
    },
    FindTopSurface {
        density: Density,
        upper_bound: Density,
        lower_bound: i32,
        cell_height: i32,
    },
    DistanceToPoint {
        point: [i32; 3],
        metric: DistanceMetric,
    },
    Blended(BlendedSet),
    /// Context node: the blender alpha (default 1 without a blender).
    BlendAlpha,
    /// Context node: the blender offset (default 0 without a blender).
    BlendOffset,
    /// Context node: the structure beard contribution (default 0).
    Beardifier,
    /// Context node: applies the world blender to a density (identity
    /// without a blender).
    BlendDensity(Density),
}

/// Cubic-spline trees: a constant value or a multipoint spline whose
/// per-point values are themselves splines, with a coordinate function.
enum Spline {
    Constant(f32),
    Multipoint {
        coordinate: Density,
        locations: Rc<[f32]>,
        derivatives: Rc<[f32]>,
        values: Rc<[Spline]>,
    },
}

/// A compiled density function ready for point evaluation.
#[derive(Clone)]
pub struct Density(Rc<Node>);

mod eval;
mod volume;
pub use volume::*;
mod math;
use math::*;
mod registry;
pub use registry::*;

#[cfg(test)]
mod tests;
