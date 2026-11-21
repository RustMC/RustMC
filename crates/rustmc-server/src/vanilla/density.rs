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

impl Density {
    pub fn constant(value: f32) -> Self {
        Self(Rc::new(Node::Constant(value)))
    }

    /// The literal value of a constant node, mirroring vanilla's
    /// `instanceof ConstantFunction` compile-time fold checks.
    fn as_constant(&self) -> Option<f32> {
        match &*self.0 {
            Node::Constant(value) => Some(*value),
            _ => None,
        }
    }

    /// Whether the value provably does not depend on `y` beyond its sign
    /// (see [`ColumnKey`]). Conservative: anything not proven independent
    /// counts as dependent, which only costs a missed memo.
    fn is_y_independent(&self) -> bool {
        match &*self.0 {
            // `find_top_surface` samples its bounds at y = 0 and its density at
            // fixed probe heights: it never reads the queried y.
            Node::Constant(_)
            | Node::BlendAlpha
            | Node::BlendOffset
            | Node::Beardifier
            | Node::FindTopSurface { .. } => true,
            Node::Noise {
                y_scale,
                shift_x,
                shift_y,
                shift_z,
                ..
            } => {
                *y_scale == 0.0
                    && shift_x.is_y_independent()
                    && shift_y.is_y_independent()
                    && shift_z.is_y_independent()
            }
            Node::ShiftNoise { kind, .. } => *kind != ShiftKind::X,
            Node::Axial { axis, .. } => *axis != Axis::Y,
            Node::Unary(_, input)
            | Node::BlendDensity(input)
            | Node::PowConstExponent { input, .. }
            | Node::Clamp { input, .. } => input.is_y_independent(),
            Node::PowConstBase { exponent, .. } => exponent.is_y_independent(),
            Node::Binary(_, left, right) | Node::Pow(left, right) => {
                left.is_y_independent() && right.is_y_independent()
            }
            Node::Round {
                input, multiple, ..
            } => input.is_y_independent() && multiple.is_y_independent(),
            Node::Lerp {
                alpha,
                first,
                second,
            } => alpha.is_y_independent() && first.is_y_independent() && second.is_y_independent(),
            Node::RangeChoice {
                input,
                when_in_range,
                when_out_of_range,
                ..
            } => {
                input.is_y_independent()
                    && when_in_range.is_y_independent()
                    && when_out_of_range.is_y_independent()
            }
            Node::IntervalSelect {
                input, functions, ..
            } => input.is_y_independent() && functions.iter().all(Density::is_y_independent),
            Node::Spline(spline) => spline_is_y_independent(spline),
            Node::Cache { input, .. } => input.is_y_independent(),
            Node::Slice { axis, input, .. } => *axis == Axis::Y || input.is_y_independent(),
            Node::Interpolated { .. } | Node::DistanceToPoint { .. } | Node::Blended(_) => false,
        }
    }

    /// Whether any descendant node is `Node::Interpolated`, used to decide
    /// when a branch of `range_choice` benefits from a volume pass over a
    /// sparse per-block sample.
    fn contains_interpolated(&self) -> bool {
        match &*self.0 {
            Node::Interpolated { .. } => true,
            Node::Unary(_, input)
            | Node::PowConstExponent { input, .. }
            | Node::PowConstBase {
                exponent: input, ..
            }
            | Node::Clamp { input, .. }
            | Node::Cache { input, .. }
            | Node::Slice { input, .. }
            | Node::BlendDensity(input) => input.contains_interpolated(),
            Node::Binary(_, left, right) | Node::Pow(left, right) => {
                left.contains_interpolated() || right.contains_interpolated()
            }
            Node::Round {
                input, multiple, ..
            } => input.contains_interpolated() || multiple.contains_interpolated(),
            Node::Lerp {
                alpha,
                first,
                second,
            } => {
                alpha.contains_interpolated()
                    || first.contains_interpolated()
                    || second.contains_interpolated()
            }
            Node::RangeChoice {
                input,
                when_in_range,
                when_out_of_range,
                ..
            } => {
                input.contains_interpolated()
                    || when_in_range.contains_interpolated()
                    || when_out_of_range.contains_interpolated()
            }
            Node::IntervalSelect {
                input, functions, ..
            } => input.contains_interpolated() || functions.iter().any(Self::contains_interpolated),
            _ => false,
        }
    }

    /// Evaluates the function at absolute block coordinates.
    pub fn sample(&self, x: i32, y: i32, z: i32) -> f32 {
        match &*self.0 {
            Node::Constant(value) => *value,
            Node::Noise {
                stack,
                xz_scale,
                y_scale,
                shift_x,
                shift_y,
                shift_z,
            } => {
                let noise_x = f64::from(x) * xz_scale + f64::from(shift_x.sample(x, y, z));
                let noise_y = f64::from(y) * y_scale + f64::from(shift_y.sample(x, y, z));
                let noise_z = f64::from(z) * xz_scale + f64::from(shift_z.sample(x, y, z));
                stack.get(noise_x, noise_y, noise_z)
            }
            Node::ShiftNoise {
                kind,
                stack,
                horizontal,
            } => {
                if *kind != ShiftKind::X
                    && let Some(cached) = horizontal.borrow_mut().get_mut(&[x, z])
                {
                    return *cached;
                }
                let fx = f64::from(x) * 0.25;
                let fy = f64::from(y) * 0.25;
                let fz = f64::from(z) * 0.25;
                let raw = match kind {
                    // shift_a: horizontal noise, y coordinate zeroed.
                    ShiftKind::Xy => stack.get(fx, 0.0, fz),
                    // shift_b: transposed — evaluated at (z, x, 0).
                    ShiftKind::Z => stack.get(fz, fx, 0.0),
                    // plain shift: full 3D.
                    ShiftKind::X => stack.get(fx, fy, fz),
                };
                let result = raw * 4.0;
                if *kind != ShiftKind::X {
                    horizontal.borrow_mut().insert([x, z], result);
                }
                result
            }
            Node::Axial {
                axis,
                tiling,
                from_coordinate,
                coordinate_range,
                from_value,
                coordinate_factor,
            } => {
                let coordinate = i64::from(axis.choose(x, y, z));
                let from_coordinate = i64::from(*from_coordinate);
                let coordinate_range = i64::from(*coordinate_range);
                let relative = match tiling {
                    Tiling::ClampToEdge => {
                        let min = from_coordinate.min(from_coordinate + coordinate_range);
                        let max = from_coordinate.max(from_coordinate + coordinate_range);
                        coordinate.clamp(min, max) - from_coordinate
                    }
                    Tiling::Repeat => (coordinate - from_coordinate).rem_euclid(coordinate_range),
                    Tiling::MirroredRepeat => {
                        let shifted = coordinate - from_coordinate;
                        let tile = shifted.div_euclid(coordinate_range);
                        let local = shifted - tile * coordinate_range;
                        if tile & 1 == 0 {
                            local
                        } else {
                            coordinate_range - local
                        }
                    }
                };
                *from_value + (relative as f32) * *coordinate_factor
            }
            Node::Unary(kind, input) => kind.apply(input.sample(x, y, z)),
            Node::Binary(kind, left, right) => {
                // Vanilla folds compile-time-known constant operands into
                // dedicated samplers: the folded mul/div forms drop the
                // left-operand zero guard, division by a constant becomes a
                // multiplication by the f32 reciprocal (which rounds
                // differently), and the left-constant check runs first.
                let folded = match kind {
                    BinaryKind::Mul => match (left.as_constant(), right.as_constant()) {
                        (Some(c), _) => Some(right.sample(x, y, z) * c),
                        (None, Some(c)) => Some(left.sample(x, y, z) * c),
                        (None, None) => None,
                    },
                    BinaryKind::Div => match (left.as_constant(), right.as_constant()) {
                        (Some(c), _) => Some(c / right.sample(x, y, z)),
                        (None, Some(c)) => Some(left.sample(x, y, z) * (1.0f32 / c)),
                        (None, None) => None,
                    },
                    _ => None,
                };
                folded.unwrap_or_else(|| kind.apply(left.sample(x, y, z), right.sample(x, y, z)))
            }
            Node::PowConstExponent { input, exponent } => {
                let base = input.sample(x, y, z);
                (base as f64).powf(*exponent) as f32
            }
            Node::PowConstBase { base, exponent } => {
                let exponent = exponent.sample(x, y, z);
                base.powf(f64::from(exponent)) as f32
            }
            Node::Pow(base, exponent) => {
                let base = base.sample(x, y, z);
                let exponent = exponent.sample(x, y, z);
                (base as f64).powf(f64::from(exponent)) as f32
            }
            Node::Round {
                kind,
                input,
                multiple,
            } => {
                let input = input.sample(x, y, z);
                let multiple = multiple.sample(x, y, z);
                if multiple == 0.0 {
                    input
                } else {
                    kind.apply(input / multiple) * multiple
                }
            }
            Node::Clamp { input, min, max } => clamp(input.sample(x, y, z), *min, *max),
            Node::Lerp {
                alpha,
                first,
                second,
            } => {
                let alpha = alpha.sample(x, y, z);
                if alpha == 0.0 {
                    first.sample(x, y, z)
                } else if alpha == 1.0 {
                    second.sample(x, y, z)
                } else {
                    lerp(alpha, first.sample(x, y, z), second.sample(x, y, z))
                }
            }
            Node::RangeChoice {
                input,
                min_inclusive,
                max_exclusive,
                when_in_range,
                when_out_of_range,
            } => {
                let value = input.sample(x, y, z);
                if value >= *min_inclusive && value < *max_exclusive {
                    when_in_range.sample(x, y, z)
                } else {
                    when_out_of_range.sample(x, y, z)
                }
            }
            Node::IntervalSelect {
                input,
                thresholds,
                functions,
            } => {
                let value = input.sample(x, y, z);
                let index = select_index(&thresholds[..], value);
                functions[index].sample(x, y, z)
            }
            Node::Spline(spline) => sample_spline(spline, x, y, z),
            Node::Cache { input, columns } => match columns {
                Some(columns) => {
                    let key = column_key(x, y, z);
                    if let Some(value) = columns.borrow_mut().get_mut(&key) {
                        return *value;
                    }
                    let value = input.sample(x, y, z);
                    columns.borrow_mut().insert(key, value);
                    value
                }
                None => input.sample(x, y, z),
            },
            Node::Interpolated {
                input,
                cell_size_xz,
                cell_size_y,
                corners,
            } => {
                let x_in_cell = floor_mod(x, *cell_size_xz);
                let y_in_cell = floor_mod(y, *cell_size_y);
                let z_in_cell = floor_mod(z, *cell_size_xz);
                if x_in_cell == 0 && y_in_cell == 0 && z_in_cell == 0 {
                    return input.sample(x, y, z);
                }
                let origin_x = x - x_in_cell;
                let origin_y = y - y_in_cell;
                let origin_z = z - z_in_cell;
                // Vanilla samples the corner grid through the input's
                // volume path (an aligned 2×2×2 cell-step volume), not
                // point-wise.
                let corner_volume = DensityVolume {
                    size: [2, 2, 2],
                    min: [origin_x, origin_y, origin_z],
                    step: [*cell_size_xz, *cell_size_y, *cell_size_xz],
                };
                let key = [origin_x, origin_y, origin_z];
                let cached = corners.borrow_mut().get_mut(&key).copied();
                let corner = cached.unwrap_or_else(|| {
                    let mut values = [0.0f32; 8];
                    input.sample_volume(&corner_volume, &mut values);
                    corners.borrow_mut().insert(key, values);
                    values
                });
                let at = |cx: usize, cy: usize, cz: usize| corner[corner_volume.index(cx, cy, cz)];
                lerp3(
                    x_in_cell as f32 / *cell_size_xz as f32,
                    y_in_cell as f32 / *cell_size_y as f32,
                    z_in_cell as f32 / *cell_size_xz as f32,
                    Lerp3Values {
                        x000: at(0, 0, 0),
                        x100: at(1, 0, 0),
                        x010: at(0, 1, 0),
                        x110: at(1, 1, 0),
                        x001: at(0, 0, 1),
                        x101: at(1, 0, 1),
                        x011: at(0, 1, 1),
                        x111: at(1, 1, 1),
                    },
                )
            }
            Node::Slice {
                axis,
                coordinate,
                input,
            } => match axis {
                Axis::X => input.sample(*coordinate, y, z),
                Axis::Y => input.sample(x, *coordinate, z),
                Axis::Z => input.sample(x, y, *coordinate),
            },
            Node::FindTopSurface {
                density,
                upper_bound,
                lower_bound,
                cell_height,
            } => {
                // Compiled with a y-slice at 0 in vanilla: the search is a
                // function of (x, z) only.
                let upper_bound = upper_bound.sample(x, 0, z);
                let cell = *cell_height;
                let mut probe_y = (upper_bound / cell as f32).floor() as i32 * cell;
                if probe_y <= *lower_bound {
                    return *lower_bound as f32;
                }
                loop {
                    if density.sample(x, probe_y, z) > 0.0 {
                        return probe_y as f32;
                    }
                    probe_y -= cell;
                    if probe_y < *lower_bound {
                        break;
                    }
                }
                *lower_bound as f32
            }
            Node::DistanceToPoint { point, metric } => {
                let dx = (point[0] - x) as f32;
                let dy = (point[1] - y) as f32;
                let dz = (point[2] - z) as f32;
                metric.apply(dx, dy, dz)
            }
            Node::Blended(set) => {
                let min = set.min.get(
                    f64::from(x) * set.xz_multiplier,
                    f64::from(y) * set.y_multiplier,
                    f64::from(z) * set.xz_multiplier,
                );
                let max = set.max.get(
                    f64::from(x) * set.xz_multiplier,
                    f64::from(y) * set.y_multiplier,
                    f64::from(z) * set.xz_multiplier,
                );
                let main = set.main.get(
                    f64::from(x) * set.main_xz_multiplier,
                    f64::from(y) * set.main_y_multiplier,
                    f64::from(z) * set.main_xz_multiplier,
                );
                let alpha = clamp(main + 0.5, 0.0, 1.0);
                if alpha == 0.0 {
                    min
                } else if alpha == 1.0 {
                    max
                } else {
                    lerp(alpha, min, max)
                }
            }
            Node::BlendAlpha => 1.0,
            Node::BlendOffset => 0.0,
            Node::Beardifier => 0.0,
            Node::BlendDensity(input) => input.sample(x, y, z),
        }
    }

    /// Fills `out` (length `volume.slot_count()`) with this function's
    /// values over a strided block volume, reproducing vanilla's
    /// `sampleVolume` operation order for the nodes whose volume path is
    /// not value-identical to the point path. Other nodes fall back to a
    /// naive per-block fill, which vanilla also uses with the same result.
    pub fn sample_volume(&self, volume: &DensityVolume, out: &mut [f32]) {
        debug_assert_eq!(out.len(), volume.slot_count());
        match &*self.0 {
            Node::Constant(value) => out.fill(*value),
            Node::BlendAlpha => out.fill(1.0),
            Node::BlendOffset | Node::Beardifier => out.fill(0.0),
            Node::BlendDensity(input) => input.sample_volume(volume, out),
            Node::Unary(kind, input) => {
                input.sample_volume(volume, out);
                for slot in out.iter_mut() {
                    *slot = kind.apply(*slot);
                }
            }
            Node::Clamp { input, min, max } => {
                input.sample_volume(volume, out);
                for slot in out.iter_mut() {
                    *slot = clamp(*slot, *min, *max);
                }
            }
            Node::Binary(kind, left, right) => {
                sample_binary_volume(*kind, left, right, volume, out)
            }
            Node::Interpolated {
                input,
                cell_size_xz,
                cell_size_y,
                ..
            } => sample_interpolated_volume(input, *cell_size_xz, *cell_size_y, volume, out),
            Node::RangeChoice {
                input,
                min_inclusive,
                max_exclusive,
                when_in_range,
                when_out_of_range,
            } => sample_range_choice_volume(
                input,
                *min_inclusive,
                *max_exclusive,
                when_in_range,
                when_out_of_range,
                volume,
                out,
            ),
            _ if self.is_y_independent() => {
                let first_slot = |x: usize, z: usize| volume.index(x, 0, z);
                let height = volume.size[1] as usize;
                let neg_count = (0..volume.size[1])
                    .take_while(|&y| volume.block(1, y) < 0)
                    .count();
                for z in 0..volume.size[2] {
                    let block_z = volume.block(2, z);
                    for x in 0..volume.size[0] {
                        let block_x = volume.block(0, x);
                        let start = first_slot(x as usize, z as usize);
                        if neg_count > 0 {
                            let val = self.sample(block_x, volume.block(1, 0), block_z);
                            out[start..start + neg_count].fill(val);
                        }
                        if neg_count < height {
                            let val =
                                self.sample(block_x, volume.block(1, neg_count as i32), block_z);
                            out[start + neg_count..start + height].fill(val);
                        }
                    }
                }
            }
            _ => {
                for z in 0..volume.size[2] {
                    for x in 0..volume.size[0] {
                        let block_x = volume.block(0, x);
                        let block_z = volume.block(2, z);
                        for y in 0..volume.size[1] {
                            let index = volume.index(x as usize, y as usize, z as usize);
                            out[index] = self.sample(block_x, volume.block(1, y), block_z);
                        }
                    }
                }
            }
        }
    }
}

/// Vanilla's `DensityVolume`: a strided sampling grid over block
/// coordinates with the same y-fastest buffer layout (`index =
/// y + (x + z·sizeX)·sizeY`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DensityVolume {
    size: [i32; 3],
    min: [i32; 3],
    step: [i32; 3],
}

impl DensityVolume {
    /// Mirrors the vanilla constructor validation: sizes and steps must
    /// all be positive.
    pub fn new(size: [i32; 3], min: [i32; 3], step: [i32; 3]) -> Option<Self> {
        if size.iter().any(|s| *s <= 0) || step.iter().any(|s| *s <= 0) {
            return None;
        }
        Some(Self { size, min, step })
    }

    pub fn slot_count(&self) -> usize {
        (self.size[0] * self.size[1] * self.size[2]) as usize
    }

    /// `indexUnchecked` for in-grid indices.
    pub fn index(&self, x: usize, y: usize, z: usize) -> usize {
        y + (x + z * self.size[0] as usize) * self.size[1] as usize
    }

    fn block(&self, axis: usize, index: i32) -> i32 {
        self.min[axis] + index * self.step[axis]
    }

    /// `maxBlock*`: one past the last covered block along the axis.
    fn max_block(&self, axis: usize) -> i32 {
        self.min[axis] + self.size[axis] * self.step[axis] - 1
    }
}

fn volume_scratch(volume: &DensityVolume) -> Vec<f32> {
    vec![0.0f32; volume.slot_count()]
}

/// The volume-path forms of the binary nodes, matching vanilla's
/// specialized sampler volume loops: constant folds operate on the
/// dynamic side in place, and the general `mul`/`div` loops apply the
/// raw operation per slot without the point sampler's left-zero guard.
fn sample_binary_volume(
    kind: BinaryKind,
    left: &Density,
    right: &Density,
    volume: &DensityVolume,
    out: &mut [f32],
) {
    let left_const = left.as_constant();
    let right_const = right.as_constant();
    match kind {
        BinaryKind::Add => match (left_const, right_const) {
            (Some(c), _) => {
                right.sample_volume(volume, out);
                for slot in out.iter_mut() {
                    *slot += c;
                }
            }
            (None, Some(c)) => {
                left.sample_volume(volume, out);
                for slot in out.iter_mut() {
                    *slot += c;
                }
            }
            (None, None) => {
                left.sample_volume(volume, out);
                let mut scratch = volume_scratch(volume);
                right.sample_volume(volume, &mut scratch);
                for (slot, other) in out.iter_mut().zip(scratch) {
                    *slot += other;
                }
            }
        },
        BinaryKind::Sub => match (left_const, right_const) {
            (Some(c), _) => {
                right.sample_volume(volume, out);
                for slot in out.iter_mut() {
                    *slot = c - *slot;
                }
            }
            (None, Some(c)) => {
                left.sample_volume(volume, out);
                let negated = -c;
                for slot in out.iter_mut() {
                    *slot += negated;
                }
            }
            (None, None) => {
                left.sample_volume(volume, out);
                let mut scratch = volume_scratch(volume);
                right.sample_volume(volume, &mut scratch);
                for (slot, other) in out.iter_mut().zip(scratch) {
                    *slot += -other;
                }
            }
        },
        BinaryKind::Mul => match (left_const, right_const) {
            (Some(c), _) => {
                right.sample_volume(volume, out);
                for slot in out.iter_mut() {
                    *slot *= c;
                }
            }
            (None, Some(c)) => {
                left.sample_volume(volume, out);
                for slot in out.iter_mut() {
                    *slot *= c;
                }
            }
            (None, None) => {
                left.sample_volume(volume, out);
                let mut scratch = volume_scratch(volume);
                right.sample_volume(volume, &mut scratch);
                for (slot, other) in out.iter_mut().zip(scratch) {
                    *slot *= other;
                }
            }
        },
        BinaryKind::Div => match (left_const, right_const) {
            (Some(c), _) => {
                right.sample_volume(volume, out);
                for slot in out.iter_mut() {
                    *slot = c / *slot;
                }
            }
            (None, Some(c)) => {
                left.sample_volume(volume, out);
                let reciprocal = 1.0f32 / c;
                for slot in out.iter_mut() {
                    *slot *= reciprocal;
                }
            }
            (None, None) => {
                left.sample_volume(volume, out);
                let mut scratch = volume_scratch(volume);
                right.sample_volume(volume, &mut scratch);
                for (slot, other) in out.iter_mut().zip(scratch) {
                    *slot /= other;
                }
            }
        },
        BinaryKind::Min => {
            let const_replacement = match (left_const, right_const) {
                (Some(c), _) => {
                    right.sample_volume(volume, out);
                    Some(c)
                }
                (None, Some(c)) => {
                    left.sample_volume(volume, out);
                    Some(c)
                }
                (None, None) => None,
            };
            match const_replacement {
                Some(c) => {
                    for slot in out.iter_mut() {
                        if c < *slot {
                            *slot = c;
                        }
                    }
                }
                None => {
                    left.sample_volume(volume, out);
                    let mut scratch = volume_scratch(volume);
                    right.sample_volume(volume, &mut scratch);
                    for (slot, other) in out.iter_mut().zip(scratch) {
                        if other < *slot {
                            *slot = other;
                        }
                    }
                }
            }
        }
        BinaryKind::Max => {
            let const_replacement = match (left_const, right_const) {
                (Some(c), _) => {
                    right.sample_volume(volume, out);
                    Some(c)
                }
                (None, Some(c)) => {
                    left.sample_volume(volume, out);
                    Some(c)
                }
                (None, None) => None,
            };
            match const_replacement {
                Some(c) => {
                    for slot in out.iter_mut() {
                        if c > *slot {
                            *slot = c;
                        }
                    }
                }
                None => {
                    left.sample_volume(volume, out);
                    let mut scratch = volume_scratch(volume);
                    right.sample_volume(volume, &mut scratch);
                    for (slot, other) in out.iter_mut().zip(scratch) {
                        if other > *slot {
                            *slot = other;
                        }
                    }
                }
            }
        }
    }
}

fn sample_range_choice_volume(
    input: &Density,
    min_inclusive: f32,
    max_exclusive: f32,
    when_in_range: &Density,
    when_out_of_range: &Density,
    volume: &DensityVolume,
    out: &mut [f32],
) {
    input.sample_volume(volume, out);
    let mut any_in = false;
    let mut any_out = false;
    for &value in out.iter() {
        if value >= min_inclusive && value < max_exclusive {
            any_in = true;
        } else {
            any_out = true;
        }
        if any_in && any_out {
            break;
        }
    }
    if !any_out {
        when_in_range.sample_volume(volume, out);
        return;
    }
    if !any_in {
        when_out_of_range.sample_volume(volume, out);
        return;
    }
    let prepare_branch = |branch: &Density| -> (Option<f32>, Option<Vec<f32>>) {
        if let Some(c) = branch.as_constant() {
            return (Some(c), None);
        }
        let buf = branch.contains_interpolated().then(|| {
            let mut buf = volume_scratch(volume);
            branch.sample_volume(volume, &mut buf);
            buf
        });
        (None, buf)
    };
    let (in_const, in_scratch) = prepare_branch(when_in_range);
    let (out_const, out_scratch) = prepare_branch(when_out_of_range);
    for z in 0..volume.size[2] {
        let block_z = volume.block(2, z);
        for x in 0..volume.size[0] {
            let block_x = volume.block(0, x);
            for y in 0..volume.size[1] {
                let index = volume.index(x as usize, y as usize, z as usize);
                let v = out[index];
                let branch_value = if v >= min_inclusive && v < max_exclusive {
                    if let Some(c) = in_const {
                        c
                    } else if let Some(buf) = in_scratch.as_deref() {
                        buf[index]
                    } else {
                        when_in_range.sample(block_x, volume.block(1, y), block_z)
                    }
                } else if let Some(c) = out_const {
                    c
                } else if let Some(buf) = out_scratch.as_deref() {
                    buf[index]
                } else {
                    when_out_of_range.sample(block_x, volume.block(1, y), block_z)
                };
                out[index] = branch_value;
            }
        }
    }
}

/// `InterpolatedFunction.Sampler.sampleVolume`: aligned cell-stepped
/// volumes pass straight through to the input; block-stepped volumes
/// fill cell corner grids and interpolate each cell; volumes with a
/// non-unit step are filled at unit stride first, then gathered.
fn sample_interpolated_volume(
    input: &Density,
    cell_size_xz: i32,
    cell_size_y: i32,
    volume: &DensityVolume,
    out: &mut [f32],
) {
    let aligned = (volume.step[0] == cell_size_xz || volume.size[0] == 1)
        && (volume.step[1] == cell_size_y || volume.size[1] == 1)
        && (volume.step[2] == cell_size_xz || volume.size[2] == 1)
        && volume.min[0].rem_euclid(cell_size_xz) == 0
        && volume.min[1].rem_euclid(cell_size_y) == 0
        && volume.min[2].rem_euclid(cell_size_xz) == 0;
    if aligned {
        input.sample_volume(volume, out);
        return;
    }
    if volume.step == [1, 1, 1] {
        sample_interpolated_block_step(input, cell_size_xz, cell_size_y, volume, out);
        return;
    }
    let block_volume = DensityVolume {
        size: [
            volume.size[0] * volume.step[0],
            volume.size[1] * volume.step[1],
            volume.size[2] * volume.step[2],
        ],
        min: volume.min,
        step: [1, 1, 1],
    };
    let mut block_buffer = volume_scratch(&block_volume);
    sample_interpolated_block_step(
        input,
        cell_size_xz,
        cell_size_y,
        &block_volume,
        &mut block_buffer,
    );
    for z in 0..volume.size[2] {
        for x in 0..volume.size[0] {
            for y in 0..volume.size[1] {
                let value = block_buffer[block_volume.index(
                    (x * volume.step[0]) as usize,
                    (y * volume.step[1]) as usize,
                    (z * volume.step[2]) as usize,
                )];
                let index = volume.index(x as usize, y as usize, z as usize);
                out[index] = value;
            }
        }
    }
}

fn sample_interpolated_block_step(
    input: &Density,
    cell_size_xz: i32,
    cell_size_y: i32,
    volume: &DensityVolume,
    out: &mut [f32],
) {
    let min_cell = [
        volume.min[0].div_euclid(cell_size_xz),
        volume.min[1].div_euclid(cell_size_y),
        volume.min[2].div_euclid(cell_size_xz),
    ];
    let max_block = [
        volume.max_block(0),
        volume.max_block(1),
        volume.max_block(2),
    ];
    let max_cell = [
        max_block[0].div_euclid(cell_size_xz),
        max_block[1].div_euclid(cell_size_y),
        max_block[2].div_euclid(cell_size_xz),
    ];
    let cell_count = [
        max_cell[0] - min_cell[0] + 1,
        max_cell[1] - min_cell[1] + 1,
        max_cell[2] - min_cell[2] + 1,
    ];
    // The corner grid gains one extra sample on each axis unless the
    // volume ends exactly on a cell boundary.
    let cell_size = [
        if max_block[0].rem_euclid(cell_size_xz) == 0 {
            cell_count[0]
        } else {
            cell_count[0] + 1
        },
        if max_block[1].rem_euclid(cell_size_y) == 0 {
            cell_count[1]
        } else {
            cell_count[1] + 1
        },
        if max_block[2].rem_euclid(cell_size_xz) == 0 {
            cell_count[2]
        } else {
            cell_count[2] + 1
        },
    ];
    let cell_volume = DensityVolume {
        size: cell_size,
        min: [
            min_cell[0] * cell_size_xz,
            min_cell[1] * cell_size_y,
            min_cell[2] * cell_size_xz,
        ],
        step: [cell_size_xz, cell_size_y, cell_size_xz],
    };
    let mut cell_buffer = volume_scratch(&cell_volume);
    input.sample_volume(&cell_volume, &mut cell_buffer);

    let xz_inv = 1.0f32 / cell_size_xz as f32;
    let y_inv = 1.0f32 / cell_size_y as f32;
    let cell_params = InterpolatedCellParams {
        volume,
        cell_volume: &cell_volume,
        xz_inv,
        y_inv,
        cell_size_xz,
        cell_size_y,
    };
    for cell_z in 0..cell_count[2] {
        let next_z = (cell_z + 1).min(cell_size[2] - 1);
        for cell_x in 0..cell_count[0] {
            let next_x = (cell_x + 1).min(cell_size[0] - 1);
            let mut v000 = cell_buffer[cell_volume.index(cell_x as usize, 0, cell_z as usize)];
            let mut v100 = cell_buffer[cell_volume.index(next_x as usize, 0, cell_z as usize)];
            let mut v001 = cell_buffer[cell_volume.index(cell_x as usize, 0, next_z as usize)];
            let mut v101 = cell_buffer[cell_volume.index(next_x as usize, 0, next_z as usize)];
            for cell_y in 0..cell_count[1] {
                let next_y = (cell_y + 1).min(cell_size[1] - 1);
                let v010 = cell_buffer
                    [cell_volume.index(cell_x as usize, next_y as usize, cell_z as usize)];
                let v110 = cell_buffer
                    [cell_volume.index(next_x as usize, next_y as usize, cell_z as usize)];
                let v011 = cell_buffer
                    [cell_volume.index(cell_x as usize, next_y as usize, next_z as usize)];
                let v111 = cell_buffer
                    [cell_volume.index(next_x as usize, next_y as usize, next_z as usize)];
                fill_interpolated_cell(
                    &cell_params,
                    out,
                    [cell_x, cell_y, cell_z],
                    CellCorners {
                        v000,
                        v100,
                        v010,
                        v110,
                        v001,
                        v101,
                        v011,
                        v111,
                    },
                );
                v000 = v010;
                v100 = v110;
                v001 = v011;
                v101 = v111;
            }
        }
    }
}

struct InterpolatedCellParams<'a> {
    volume: &'a DensityVolume,
    cell_volume: &'a DensityVolume,
    xz_inv: f32,
    y_inv: f32,
    cell_size_xz: i32,
    cell_size_y: i32,
}

struct CellCorners {
    v000: f32,
    v100: f32,
    v010: f32,
    v110: f32,
    v001: f32,
    v101: f32,
    v011: f32,
    v111: f32,
}

/// `InterpolatedFunction.Sampler.fillCell`: horizontal axes interpolate
/// with the reciprocal-cell multiply, and the vertical direction is a
/// repeated addition of `valueStep` — not per-point lerp evaluation.
fn fill_interpolated_cell(
    params: &InterpolatedCellParams<'_>,
    out: &mut [f32],
    [cell_x, cell_y, cell_z]: [i32; 3],
    corners: CellCorners,
) {
    let &InterpolatedCellParams {
        volume,
        cell_volume,
        xz_inv,
        y_inv,
        cell_size_xz,
        cell_size_y,
    } = params;
    let CellCorners {
        v000,
        v100,
        v010,
        v110,
        v001,
        v101,
        v011,
        v111,
    } = corners;
    let cell_output_x = cell_volume.block(0, cell_x) - volume.min[0];
    let cell_output_y = cell_volume.block(1, cell_y) - volume.min[1];
    let cell_output_z = cell_volume.block(2, cell_z) - volume.min[2];
    let x0 = (-cell_output_x).max(0);
    let y0 = (-cell_output_y).max(0);
    let z0 = (-cell_output_z).max(0);
    let x1 = cell_size_xz.min(volume.size[0] - cell_output_x) - 1;
    let y1 = cell_size_y.min(volume.size[1] - cell_output_y) - 1;
    let z1 = cell_size_xz.min(volume.size[2] - cell_output_z) - 1;

    let xz_f = cell_size_xz as f32;
    let y_f = cell_size_y as f32;
    for z in z0..=z1 {
        let output_z = cell_output_z + z;
        let alpha_z = z as f32 / xz_f;
        for x in x0..=x1 {
            let output_x = cell_output_x + x;
            let alpha_x = x as f32 / xz_f;
            let lx00 = lerp(alpha_x, v000, v100);
            let lx01 = lerp(alpha_x, v010, v110);
            let lx10 = lerp(alpha_x, v001, v101);
            let lx11 = lerp(alpha_x, v011, v111);
            // The volume layout is y-fastest, so the cell's vertical run
            // is a contiguous slot range.
            let first_slot = volume.index(
                output_x as usize,
                (cell_output_y + y0) as usize,
                output_z as usize,
            );
            for (offset, slot) in out[first_slot..first_slot + (y1 - y0 + 1) as usize]
                .iter_mut()
                .enumerate()
            {
                let alpha_y = (y0 + offset as i32) as f32 / y_f;
                *slot = lerp(
                    alpha_z,
                    lerp(alpha_y, lx00, lx01),
                    lerp(alpha_y, lx10, lx11),
                );
            }
        }
    }
    let _ = (xz_inv, y_inv);
}

/// `Mth.clamp(float, min, max)`: comparison-chain form, so NaN passes
/// through exactly like the Java implementation.
fn clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// `Math.min(float, float)`: NaN-propagating, and -0.0 compares below 0.0.
fn java_min(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}

/// `Math.max(float, float)`: NaN-propagating, and 0.0 wins over -0.0.
fn java_max(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else if a > b || (a == b && !a.is_sign_negative()) {
        a
    } else {
        b
    }
}

fn leaky(input: f32, negative_factor: f32) -> f32 {
    if input > 0.0 {
        input
    } else {
        input * negative_factor
    }
}

/// `Mth.lerp(float, a, b)`.
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

/// `Mth.lerp3` with the same nesting order as the Java helper.
fn lerp3(a1: f32, a2: f32, a3: f32, values: Lerp3Values) -> f32 {
    lerp(
        a3,
        lerp2(a1, a2, values.x000, values.x100, values.x010, values.x110),
        lerp2(a1, a2, values.x001, values.x101, values.x011, values.x111),
    )
}

fn floor_mod(value: i32, divisor: i32) -> i32 {
    value.rem_euclid(divisor)
}

/// `a - floorDiv(a, b) * b` for the mirrored-repeat tiling (Java int ops).
/// Index selection of `interval_select`: first threshold strictly above the
/// input, else the last function.
fn select_index(thresholds: &[f32], input: f32) -> usize {
    for (index, threshold) in thresholds.iter().enumerate() {
        if input < *threshold {
            return index;
        }
    }
    thresholds.len()
}

/// `Mth.binarySearch(0, len, i -> input < locations[i]) - 1`, reproduced
/// statement for statement including the halving arithmetic.
fn find_interval_start(locations: &[f32], input: f32) -> i32 {
    let mut from = 0i32;
    let mut len = locations.len() as i32;
    while len > 0 {
        let half = len / 2;
        let middle = from + half;
        if input < locations[middle as usize] {
            len = half;
        } else {
            from = middle + 1;
            len -= half + 1;
        }
    }
    from - 1
}

fn linear_extend(
    input: f32,
    locations: &[f32],
    value: f32,
    derivatives: &[f32],
    index: usize,
) -> f32 {
    let derivative = derivatives[index];
    if derivative == 0.0 {
        value
    } else {
        value + derivative * (input - locations[index])
    }
}

/// Memo key for a `y`-independent cached node: the column plus the sign of
/// `y`. A zero-`y_scale` noise multiplies `y` by `0.0`, which is `-0.0` for
/// negative `y`; keeping the sign in the key returns exactly the bits the
/// direct evaluation would produce.
type ColumnKey = (i32, i32, bool);

/// Columns each cached node remembers per generation of its two-generation
/// [`BoundedCache`]: a chunk plus a one-cell interpolation margin
/// (`(16 + 2 * 4)^2` = 576) rounded up.
const CACHE_NODE_COLUMNS: usize = 1024;

fn column_key(x: i32, y: i32, z: i32) -> ColumnKey {
    (x, z, y < 0)
}

/// Builds a `minecraft:cache` node, giving it a per-column memo when its
/// input cannot depend on `y`.
fn cache_node(input: Density) -> Node {
    let columns = input
        .is_y_independent()
        .then(|| RefCell::new(BoundedCache::new(CACHE_NODE_COLUMNS)));
    Node::Cache { input, columns }
}

fn spline_is_y_independent(spline: &Spline) -> bool {
    match spline {
        Spline::Constant(_) => true,
        Spline::Multipoint {
            coordinate, values, ..
        } => coordinate.is_y_independent() && values.iter().all(spline_is_y_independent),
    }
}

fn sample_spline(spline: &Spline, x: i32, y: i32, z: i32) -> f32 {
    match spline {
        Spline::Constant(value) => *value,
        Spline::Multipoint {
            coordinate,
            locations,
            derivatives,
            values,
        } => {
            let input = coordinate.sample(x, y, z);
            let start = find_interval_start(locations, input);
            let last_index = locations.len() - 1;
            if start < 0 {
                let first = sample_spline(&values[0], x, y, z);
                linear_extend(input, locations, first, derivatives, 0)
            } else if start as usize == last_index {
                let last = sample_spline(&values[last_index], x, y, z);
                linear_extend(input, locations, last, derivatives, last_index)
            } else {
                let index = start as usize;
                let x1 = locations[index];
                let x2 = locations[index + 1];
                let delta_x = x2 - x1;
                let t = (input - x1) / delta_x;
                let y1 = sample_spline(&values[index], x, y, z);
                let y2 = sample_spline(&values[index + 1], x, y, z);
                let a = derivatives[index] * delta_x - (y2 - y1);
                let b = -(derivatives[index + 1]) * delta_x + (y2 - y1);
                lerp(t, y1, y2) + t * (1.0 - t) * lerp(t, a, b)
            }
        }
    }
}

/// World-seeded noise provider: the positional factory derived from the
/// world seed, plus the noise registry. Noise instances are created once
/// per identifier and shared, matching the documented world-level caching.
pub struct NoiseEngine {
    positional: PositionalRandomFactory,
    definitions: HashMap<Id, NoiseParameters>,
    stacks: RefCell<HashMap<Id, Rc<NoiseStack>>>,
}

impl NoiseEngine {
    pub fn new(
        positional: PositionalRandomFactory,
        definitions: HashMap<Id, NoiseParameters>,
    ) -> Self {
        Self {
            positional,
            definitions,
            stacks: RefCell::new(HashMap::new()),
        }
    }

    /// The root positional random factory derived from the world seed.
    /// Later phases derive named factories from it exactly as the
    /// documented world-seed chain does (for example the aquifer grid).
    pub fn positional(&self) -> PositionalRandomFactory {
        self.positional
    }

    /// Raw noise for a datapack `worldgen/noise` identifier, seeded from
    /// the world factory exactly as the documented `getOrCreateNoise`
    /// caching does. Surface rules sample these directly.
    pub fn noise_stack(&self, id: &str) -> Result<Rc<NoiseStack>, DensityError> {
        self.stack(&id.to_owned())
    }

    fn stack(&self, id: &Id) -> Result<Rc<NoiseStack>, DensityError> {
        if let Some(stack) = self.stacks.borrow().get(id) {
            return Ok(Rc::clone(stack));
        }
        let parameters = self
            .definitions
            .get(id)
            .ok_or_else(|| DensityError::Unresolved(id.clone()))?
            .clone();
        let mut random = self.positional.from_hash_of(id);
        let stack = Rc::new(NormalNoise::new(&parameters).create(&mut random));
        self.stacks
            .borrow_mut()
            .insert(id.clone(), Rc::clone(&stack));
        Ok(stack)
    }

    fn random(&self, id: &str) -> RandomSource {
        self.positional.from_hash_of(id)
    }
}

/// Density JSON documents keyed by identifier.
pub struct DensityRegistry<'e> {
    engine: &'e NoiseEngine,
    documents: HashMap<Id, Value>,
    compiled: RefCell<HashMap<Id, Density>>,
    compiling: RefCell<HashSet<Id>>,
}

// A reference chain also nests Rust parser frames. Keep the limit below the
// default test-thread stack while accepting the provisioned 26.3 graph.
const MAX_REFERENCE_DEPTH: usize = 32;

impl<'e> DensityRegistry<'e> {
    pub fn new(engine: &'e NoiseEngine, documents: HashMap<Id, Value>) -> Self {
        Self {
            engine,
            documents,
            compiled: RefCell::new(HashMap::new()),
            compiling: RefCell::new(HashSet::new()),
        }
    }

    /// Resolves a documented density-function identifier (built-in or
    /// datapack-provided) into a compiled graph.
    pub fn compile(&self, id: &Id) -> Result<Density, DensityError> {
        if let Some(existing) = self.compiled.borrow().get(id) {
            return Ok(existing.clone());
        }
        if self.compiling.borrow().len() >= MAX_REFERENCE_DEPTH {
            return Err(DensityError::Parse(format!(
                "density reference depth exceeds {MAX_REFERENCE_DEPTH} at {id}"
            )));
        }
        if !self.compiling.borrow_mut().insert(id.clone()) {
            return Err(DensityError::Parse(format!(
                "cyclic density reference: {id}"
            )));
        }
        let result = if let Some(document) = self.documents.get(id) {
            self.compile_value(document)
        } else {
            self.compile_builtin(id)
        };
        self.compiling.borrow_mut().remove(id);
        let density = result?;
        self.compiled
            .borrow_mut()
            .insert(id.clone(), density.clone());
        Ok(density)
    }

    /// Compiles one child-slot value: an object node, a reference
    /// string resolved through the registry, or a bare number constant.
    pub fn compile_slot(&self, value: &Value) -> Result<Density, DensityError> {
        self.child(value)
    }

    /// Compiles a detached JSON document (not stored in the registry).
    pub fn compile_value(&self, value: &Value) -> Result<Density, DensityError> {
        // Registry document roots may be bare numbers: the datapack codec
        // reads them as constant functions (26.3 `zero.json` is `0.0`).
        if let Value::Number(_) = value {
            return Ok(Density::constant(noise_value(value)?));
        }
        let node = self.parse(value)?;
        Ok(Density(Rc::new(node)))
    }

    fn compile_builtin(&self, id: &Id) -> Result<Density, DensityError> {
        let node = match id.as_str() {
            "minecraft:zero" => Node::Constant(0.0),
            // Registered by the vanilla bootstrap as a clamped identity
            // gradient over the doubled block-position range; independent
            // of any dimension (`PROVENANCE.md`: 26.3 packs Y in 12 bits,
            // `Y_SIZE = (1 << 12) - 32 = 4064`, `MIN_Y = -2032`,
            // `MAX_Y = 2031`).
            "minecraft:y" => Node::Axial {
                axis: Axis::Y,
                tiling: Tiling::ClampToEdge,
                from_coordinate: BUILTIN_Y_FROM,
                coordinate_range: BUILTIN_Y_TO - BUILTIN_Y_FROM,
                from_value: BUILTIN_Y_FROM as f32,
                coordinate_factor: 1.0,
            },
            "minecraft:shift_x" => cache_node(Density(Rc::new(Node::ShiftNoise {
                kind: ShiftKind::Xy,
                stack: self.engine.stack(&"minecraft:shift".to_owned())?,
                horizontal: RefCell::new(BoundedCache::new(512)),
            }))),
            "minecraft:shift_z" => cache_node(Density(Rc::new(Node::ShiftNoise {
                kind: ShiftKind::Z,
                stack: self.engine.stack(&"minecraft:shift".to_owned())?,
                horizontal: RefCell::new(BoundedCache::new(512)),
            }))),
            _ => return Err(DensityError::Unresolved(id.clone())),
        };
        Ok(Density(Rc::new(node)))
    }

    /// Parses one child slot: an object node, a `"namespace:path"`
    /// reference, or a bare number constant.
    fn child(&self, value: &Value) -> Result<Density, DensityError> {
        match value {
            Value::Object(_) => self.compile_value(value),
            Value::String(id) => self.compile(id),
            Value::Number(_) => Ok(Density::constant(noise_value(value)?)),
            other => Err(DensityError::Parse(format!(
                "expected density node, got {other}"
            ))),
        }
    }

    fn parse(&self, value: &Value) -> Result<Node, DensityError> {
        let object = value
            .as_object()
            .ok_or_else(|| DensityError::Parse("density node must be an object".to_owned()))?;
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| DensityError::Parse("missing \"type\"".to_owned()))?;
        let kind = kind.strip_prefix("minecraft:").unwrap_or(kind);
        let get = |key: &str| {
            object
                .get(key)
                .ok_or_else(|| DensityError::Parse(format!("{kind} missing {key:?}")))
        };
        let node = match kind {
            "constant" => Node::Constant(noise_value(get("value")?)?),
            "noise" => Node::Noise {
                stack: self.engine.stack(&referenced_id(get("noise")?)?)?,
                xz_scale: get("xz_scale")?
                    .as_f64()
                    .ok_or_else(|| DensityError::Parse("xz_scale must be a number".to_owned()))?,
                y_scale: get("y_scale")?
                    .as_f64()
                    .ok_or_else(|| DensityError::Parse("y_scale must be a number".to_owned()))?,
                shift_x: self
                    .child(object.get("shift_x").unwrap_or(&Value::Null).default_zero())?,
                shift_y: self
                    .child(object.get("shift_y").unwrap_or(&Value::Null).default_zero())?,
                shift_z: self
                    .child(object.get("shift_z").unwrap_or(&Value::Null).default_zero())?,
            },
            name @ ("shift" | "shift_a" | "shift_b") => Node::ShiftNoise {
                kind: match name {
                    "shift" => ShiftKind::X,
                    "shift_a" => ShiftKind::Xy,
                    _ => ShiftKind::Z,
                },
                stack: self.engine.stack(&referenced_id(get("noise")?)?)?,
                horizontal: RefCell::new(BoundedCache::new(512)),
            },
            "gradient" => {
                let axis = Axis::parse(
                    get("axis")?
                        .as_str()
                        .ok_or_else(|| DensityError::Parse("axis must be a string".to_owned()))?,
                )?;
                let tiling = match get("tiling") {
                    Ok(Value::String(name)) => Tiling::parse(name)?,
                    Ok(Value::Null) | Err(_) => Tiling::ClampToEdge,
                    Ok(other) => return Err(DensityError::Parse(format!("bad tiling {other}"))),
                };
                let from_coordinate = get("from_coordinate")?
                    .as_i64()
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| {
                        DensityError::Parse("from_coordinate must be a 32-bit int".to_owned())
                    })?;
                let to_coordinate = get("to_coordinate")?
                    .as_i64()
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| {
                        DensityError::Parse("to_coordinate must be a 32-bit int".to_owned())
                    })?;
                let from_value = noise_value(get("from_value")?)?;
                let to_value = noise_value(get("to_value")?)?;
                let coordinate_range = to_coordinate
                    .checked_sub(from_coordinate)
                    .filter(|value| *value != 0)
                    .ok_or_else(|| {
                        DensityError::Parse(
                            "gradient coordinate range must be nonzero and fit a 32-bit int"
                                .to_owned(),
                        )
                    })?;
                Node::Axial {
                    axis,
                    tiling,
                    from_coordinate,
                    coordinate_range,
                    from_value,
                    coordinate_factor: (to_value - from_value) / coordinate_range as f32,
                }
            }
            "blend_alpha" => Node::BlendAlpha,
            "blend_offset" => Node::BlendOffset,
            "beardifier" => Node::Beardifier,
            _ => self.parse_with_input(kind, get, object)?,
        };
        Ok(node)
    }

    fn parse_with_input<'v>(
        &self,
        kind: &str,
        get: impl Fn(&str) -> Result<&'v Value, DensityError>,
        object: &'v serde_json::Map<String, Value>,
    ) -> Result<Node, DensityError> {
        let node = match kind {
            "cache" => cache_node(self.child(get("input")?)?),
            "interpolated" => {
                let cell_size_xz = positive_int(get("cell_size_xz")?)?;
                let cell_size_y = positive_int(get("cell_size_y")?)?;
                Node::Interpolated {
                    input: self.child(get("input")?)?,
                    cell_size_xz,
                    cell_size_y,
                    corners: RefCell::new(BoundedCache::new(64)),
                }
            }
            "slice" => Node::Slice {
                axis: Axis::parse(
                    get("axis")?
                        .as_str()
                        .ok_or_else(|| DensityError::Parse("axis must be a string".to_owned()))?,
                )?,
                coordinate: get("coordinate")?
                    .as_i64()
                    .ok_or_else(|| DensityError::Parse("coordinate must be an int".to_owned()))?
                    as i32,
                input: self.child(get("input")?)?,
            },
            "clamp" => Node::Clamp {
                input: self.child(get("input")?)?,
                min: noise_value(get("min")?)?,
                max: noise_value(get("max")?)?,
            },
            "lerp" => Node::Lerp {
                alpha: self.child(get("alpha")?)?,
                first: self.child(get("first")?)?,
                second: self.child(get("second")?)?,
            },
            "range_choice" => Node::RangeChoice {
                input: self.child(get("input")?)?,
                min_inclusive: noise_value(get("min_inclusive")?)?,
                max_exclusive: noise_value(get("max_exclusive")?)?,
                when_in_range: self.child(get("when_in_range")?)?,
                when_out_of_range: self.child(get("when_out_of_range")?)?,
            },
            "interval_select" => {
                let thresholds_value = get("thresholds")?;
                let thresholds: Vec<f32> = thresholds_value
                    .as_array()
                    .ok_or_else(|| DensityError::Parse("thresholds must be an array".to_owned()))?
                    .iter()
                    .map(noise_value)
                    .collect::<Result<_, _>>()?;
                let functions: Vec<Density> = get("functions")?
                    .as_array()
                    .ok_or_else(|| DensityError::Parse("functions must be an array".to_owned()))?
                    .iter()
                    .map(|item| self.child(item))
                    .collect::<Result<_, _>>()?;
                if functions.len() < 2 || thresholds.len() + 1 != functions.len() {
                    return Err(DensityError::Parse(format!(
                        "interval_select wants {} thresholds for {} functions, got {}",
                        functions.len() - 1,
                        functions.len(),
                        thresholds.len()
                    )));
                }
                if thresholds.windows(2).any(|pair| pair[0] > pair[1]) {
                    return Err(DensityError::Parse(
                        "interval_select thresholds must be ordered".to_owned(),
                    ));
                }
                Node::IntervalSelect {
                    input: self.child(get("input")?)?,
                    thresholds: thresholds.into(),
                    functions: functions.into(),
                }
            }
            "spline" => Node::Spline(self.parse_spline(get("spline")?)?),
            "find_top_surface" => Node::FindTopSurface {
                density: self.child(get("density")?)?,
                upper_bound: self.child(get("upper_bound")?)?,
                lower_bound: get("lower_bound")?
                    .as_i64()
                    .ok_or_else(|| DensityError::Parse("lower_bound must be an int".to_owned()))?
                    as i32,
                cell_height: positive_int(get("cell_height")?)?,
            },
            "distance_to_point" => {
                // Vanilla decodes `point` with the vec3 codec, which only
                // accepts the three-element array form.
                let coords = get("point")?
                    .as_array()
                    .ok_or_else(|| DensityError::Parse("point must be a json array".to_owned()))?;
                if coords.len() != 3 {
                    return Err(DensityError::Parse(
                        "point must have exactly three coordinates".to_owned(),
                    ));
                }
                let mut point = [0i32; 3];
                for (slot, value) in point.iter_mut().zip(coords) {
                    *slot = value.as_i64().ok_or_else(|| {
                        DensityError::Parse("point coordinates must be ints".to_owned())
                    })? as i32;
                }
                let metric =
                    DistanceMetric::parse(get("metric")?.as_str().ok_or_else(|| {
                        DensityError::Parse("metric must be a string".to_owned())
                    })?)?;
                Node::DistanceToPoint { point, metric }
            }
            "blend_density" => Node::BlendDensity(self.child(get("input")?)?),
            "old_blended_noise" => Node::Blended(self.blended_set(get)?),
            "end_outer_islands" => {
                return Err(DensityError::Unsupported(
                    "end_outer_islands (needs the simplex provider; slice C)".to_owned(),
                ));
            }
            other => {
                if let Some(unary) = UnaryKind::parse(other) {
                    Node::Unary(unary, self.child(get("input")?)?)
                } else if let Some(binary) = BinaryKind::parse(other) {
                    Node::Binary(
                        binary,
                        self.child(get("left")?)?,
                        self.child(get("right")?)?,
                    )
                } else if let Some(round) = RoundKind::parse(other) {
                    let multiple = match object.get("multiple") {
                        None | Some(Value::Null) => Density::constant(1.0),
                        Some(value) => self.child(value)?,
                    };
                    Node::Round {
                        kind: round,
                        input: self.child(get("input")?)?,
                        multiple,
                    }
                } else if other == "pow" {
                    let base = self.child(get("base")?)?;
                    let exponent = self.child(get("exponent")?)?;
                    fold_pow(base, exponent)
                } else {
                    return Err(DensityError::Unsupported(other.to_owned()));
                }
            }
        };
        Ok(node)
    }

    fn parse_spline(&self, value: &Value) -> Result<Spline, DensityError> {
        match value {
            Value::Number(_) => Ok(Spline::Constant(noise_value(value)?)),
            Value::Object(object) => {
                let coordinate = object.get("coordinate").ok_or_else(|| {
                    DensityError::Parse("spline missing \"coordinate\"".to_owned())
                })?;
                let points = object
                    .get("points")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        DensityError::Parse("spline missing non-empty \"points\"".to_owned())
                    })?;
                if points.is_empty() {
                    return Err(DensityError::Parse(
                        "spline points must be non-empty".to_owned(),
                    ));
                }
                let mut locations = Vec::with_capacity(points.len());
                let mut values = Vec::with_capacity(points.len());
                let mut derivatives = Vec::with_capacity(points.len());
                for point in points {
                    let point = point.as_object().ok_or_else(|| {
                        DensityError::Parse("spline point must be an object".to_owned())
                    })?;
                    let read = |key: &str| {
                        point.get(key).ok_or_else(|| {
                            DensityError::Parse(format!("spline point missing {key:?}"))
                        })
                    };
                    locations.push(float_field(read("location")?)?);
                    values.push(self.parse_spline(read("value")?)?);
                    derivatives.push(float_field(read("derivative")?)?);
                }
                Ok(Spline::Multipoint {
                    coordinate: self.child(coordinate)?,
                    locations: locations.into(),
                    derivatives: derivatives.into(),
                    values: values.into(),
                })
            }
            other => Err(DensityError::Parse(format!(
                "spline value must be a number or object, got {other}"
            ))),
        }
    }

    fn blended_set<'v>(
        &self,
        get: impl Fn(&str) -> Result<&'v Value, DensityError>,
    ) -> Result<BlendedSet, DensityError> {
        let xz_scale = get("xz_scale")?
            .as_f64()
            .ok_or_else(|| DensityError::Parse("xz_scale must be a number".to_owned()))?;
        let y_scale = get("y_scale")?
            .as_f64()
            .ok_or_else(|| DensityError::Parse("y_scale must be a number".to_owned()))?;
        let xz_factor = get("xz_factor")?
            .as_f64()
            .ok_or_else(|| DensityError::Parse("xz_factor must be a number".to_owned()))?;
        let y_factor = get("y_factor")?
            .as_f64()
            .ok_or_else(|| DensityError::Parse("y_factor must be a number".to_owned()))?;
        let smear_scale_multiplier = get("smear_scale_multiplier")?.as_f64().ok_or_else(|| {
            DensityError::Parse("smear_scale_multiplier must be a number".to_owned())
        })?;
        for (name, scale) in [
            ("xz_scale", xz_scale),
            ("y_scale", y_scale),
            ("xz_factor", xz_factor),
            ("y_factor", y_factor),
        ] {
            if !(0.001..=1000.0).contains(&scale) {
                return Err(DensityError::Parse(format!("{name} {scale} out of range")));
            }
        }
        if !(1.0..=8.0).contains(&smear_scale_multiplier) {
            return Err(DensityError::Parse(
                "smear_scale_multiplier out of range".to_owned(),
            ));
        }
        // The three FBMs consume one sequential stream, min limit first.
        let mut random = self.engine.random("minecraft:terrain");
        let xz_multiplier = BASE_SCALE * xz_scale;
        let y_multiplier = BASE_SCALE * y_scale;
        let limit_smear = y_multiplier * smear_scale_multiplier;
        let min = create_blended_fbm(&mut random, -15, limit_smear, f64::from(LIMIT_FACTOR));
        let max = create_blended_fbm(&mut random, -15, limit_smear, f64::from(LIMIT_FACTOR));
        let main = create_blended_fbm(&mut random, -7, limit_smear / y_factor, MAIN_FACTOR);
        Ok(BlendedSet {
            min,
            max,
            main,
            xz_multiplier,
            y_multiplier,
            main_xz_multiplier: xz_multiplier / xz_factor,
            main_y_multiplier: y_multiplier / y_factor,
        })
    }
}

/// `BlendedNoise` construction constants captured in `docs/PROVENANCE.md`.
const BASE_SCALE: f64 = 684.412;
const LIMIT_FACTOR: f32 = 0.99998474;
const MAIN_FACTOR: f64 = 12.75;

/// Built-in `minecraft:y` gradient bounds, `DimensionType.MIN_Y * 2` and
/// `MAX_Y * 2` as established in `docs/PROVENANCE.md`.
const BUILTIN_Y_FROM: i32 = -4064;
const BUILTIN_Y_TO: i32 = 4062;

trait DefaultZero {
    fn default_zero(&self) -> &Value;
}

impl DefaultZero for Value {
    fn default_zero(&self) -> &Value {
        if matches!(self, Value::Null) {
            // The absent shift is a constant zero node; keep one shared
            // instance through the caller's parse path by returning a
            // numeric literal JSON value.
            ZERO_LITERAL
                .get_or_init(|| serde_json::json!({ "type": "minecraft:constant", "value": 0.0 }))
        } else {
            self
        }
    }
}

static ZERO_LITERAL: OnceLock<Value> = OnceLock::new();

fn referenced_id(value: &Value) -> Result<Id, DensityError> {
    value
        .as_str()
        .map(|id| {
            if id.contains(':') {
                id.to_owned()
            } else {
                format!("minecraft:{id}")
            }
        })
        .ok_or_else(|| DensityError::Parse("expected a resource id string".to_owned()))
}

/// `NOISE_VALUE_CODEC`: float values limited to |v| <= 1e6.
fn noise_value(value: &Value) -> Result<f32, DensityError> {
    let raw = value
        .as_f64()
        .ok_or_else(|| DensityError::Parse("expected a number".to_owned()))?;
    let as_float = raw as f32;
    if as_float.abs() > 1.0e6 {
        return Err(DensityError::Parse(format!(
            "noise value {raw} exceeds the codec limit"
        )));
    }
    Ok(as_float)
}

fn float_field(value: &Value) -> Result<f32, DensityError> {
    value
        .as_f64()
        .map(|raw| raw as f32)
        .ok_or_else(|| DensityError::Parse("expected a float".to_owned()))
}

fn positive_int(value: &Value) -> Result<i32, DensityError> {
    let raw = value
        .as_i64()
        .ok_or_else(|| DensityError::Parse("expected an int".to_owned()))?;
    if raw <= 0 || raw > i64::from(i32::MAX) {
        return Err(DensityError::Parse(format!("{raw} is not positive")));
    }
    Ok(raw as i32)
}

/// Java folds constant-exponent `pow` into dedicated operations at compile
/// time: sqrt/passthrough/square/cube, wrapped in a reciprocal for negative
/// exponents. Anything else keeps the general two-operand `Math.pow`.
fn fold_pow(base: Density, exponent: Density) -> Node {
    if let Node::Constant(value) = *exponent.0 {
        let absolute = value.abs();
        let folded = if absolute == 0.5 {
            UnaryKind::Sqrt
        } else if absolute == 1.0 {
            if value > 0.0 {
                return cache_node(base);
            }
            UnaryKind::Reciprocal
        } else if absolute == 2.0 {
            UnaryKind::Square
        } else if absolute == 3.0 {
            UnaryKind::Cube
        } else {
            return Node::PowConstExponent {
                input: base,
                exponent: f64::from(value),
            };
        };
        return if value < 0.0 && folded != UnaryKind::Reciprocal {
            Node::Unary(
                UnaryKind::Reciprocal,
                Density(Rc::new(Node::Unary(folded, base))),
            )
        } else {
            Node::Unary(folded, base)
        };
    }
    if let Node::Constant(value) = *base.0 {
        return Node::PowConstBase {
            base: f64::from(value),
            exponent,
        };
    }
    Node::Pow(base, exponent)
}

#[cfg(test)]
mod tests;
