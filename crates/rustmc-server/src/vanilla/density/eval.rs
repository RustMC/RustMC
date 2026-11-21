//! Point evaluation of a compiled density function.

use super::*;

impl Density {
    pub fn constant(value: f32) -> Self {
        Self(Rc::new(Node::Constant(value)))
    }

    /// The literal value of a constant node, mirroring vanilla's
    /// `instanceof ConstantFunction` compile-time fold checks.
    pub(super) fn as_constant(&self) -> Option<f32> {
        match &*self.0 {
            Node::Constant(value) => Some(*value),
            _ => None,
        }
    }

    /// Whether the value provably does not depend on `y` beyond its sign
    /// (see [`ColumnKey`]). Conservative: anything not proven independent
    /// counts as dependent, which only costs a missed memo.
    pub(super) fn is_y_independent(&self) -> bool {
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
    pub(super) fn contains_interpolated(&self) -> bool {
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
