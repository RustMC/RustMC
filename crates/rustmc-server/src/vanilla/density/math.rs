//! Arithmetic helpers with vanilla rounding and the bounded per-column
//! memo of cache nodes, plus spline sampling.

use super::*;

/// `Mth.clamp(float, min, max)`: comparison-chain form, so NaN passes
/// through exactly like the Java implementation.
pub(super) fn clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// `Math.min(float, float)`: NaN-propagating, and -0.0 compares below 0.0.
pub(super) fn java_min(a: f32, b: f32) -> f32 {
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
pub(super) fn java_max(a: f32, b: f32) -> f32 {
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

pub(super) fn leaky(input: f32, negative_factor: f32) -> f32 {
    if input > 0.0 {
        input
    } else {
        input * negative_factor
    }
}

/// `Mth.lerp(float, a, b)`.
pub(super) fn lerp(alpha: f32, from: f32, to: f32) -> f32 {
    from + alpha * (to - from)
}

pub(super) fn lerp2(a1: f32, a2: f32, x00: f32, x10: f32, x01: f32, x11: f32) -> f32 {
    lerp(a2, lerp(a1, x00, x10), lerp(a1, x01, x11))
}

pub(super) struct Lerp3Values {
    pub(super) x000: f32,
    pub(super) x100: f32,
    pub(super) x010: f32,
    pub(super) x110: f32,
    pub(super) x001: f32,
    pub(super) x101: f32,
    pub(super) x011: f32,
    pub(super) x111: f32,
}

/// `Mth.lerp3` with the same nesting order as the Java helper.
pub(super) fn lerp3(a1: f32, a2: f32, a3: f32, values: Lerp3Values) -> f32 {
    lerp(
        a3,
        lerp2(a1, a2, values.x000, values.x100, values.x010, values.x110),
        lerp2(a1, a2, values.x001, values.x101, values.x011, values.x111),
    )
}

pub(super) fn floor_mod(value: i32, divisor: i32) -> i32 {
    value.rem_euclid(divisor)
}

/// `a - floorDiv(a, b) * b` for the mirrored-repeat tiling (Java int ops).
/// Index selection of `interval_select`: first threshold strictly above the
/// input, else the last function.
pub(super) fn select_index(thresholds: &[f32], input: f32) -> usize {
    for (index, threshold) in thresholds.iter().enumerate() {
        if input < *threshold {
            return index;
        }
    }
    thresholds.len()
}

/// `Mth.binarySearch(0, len, i -> input < locations[i]) - 1`, reproduced
/// statement for statement including the halving arithmetic.
pub(super) fn find_interval_start(locations: &[f32], input: f32) -> i32 {
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

pub(super) fn linear_extend(
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
pub(super) type ColumnKey = (i32, i32, bool);

/// Columns each cached node remembers per generation of its two-generation
/// [`BoundedCache`]: a chunk plus a one-cell interpolation margin
/// (`(16 + 2 * 4)^2` = 576) rounded up.
pub(super) const CACHE_NODE_COLUMNS: usize = 1024;

pub(super) fn column_key(x: i32, y: i32, z: i32) -> ColumnKey {
    (x, z, y < 0)
}

/// Builds a `minecraft:cache` node, giving it a per-column memo when its
/// input cannot depend on `y`.
pub(super) fn cache_node(input: Density) -> Node {
    let columns = input
        .is_y_independent()
        .then(|| RefCell::new(BoundedCache::new(CACHE_NODE_COLUMNS)));
    Node::Cache { input, columns }
}

pub(super) fn spline_is_y_independent(spline: &Spline) -> bool {
    match spline {
        Spline::Constant(_) => true,
        Spline::Multipoint {
            coordinate, values, ..
        } => coordinate.is_y_independent() && values.iter().all(spline_is_y_independent),
    }
}

pub(super) fn sample_spline(spline: &Spline, x: i32, y: i32, z: i32) -> f32 {
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
