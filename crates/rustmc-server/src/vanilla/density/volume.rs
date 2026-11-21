//! Volume evaluation: filling a whole cell grid at once with results
//! identical to point evaluation.

use super::*;

/// Vanilla's `DensityVolume`: a strided sampling grid over block
/// coordinates with the same y-fastest buffer layout (`index =
/// y + (x + z·sizeX)·sizeY`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DensityVolume {
    pub(super) size: [i32; 3],
    pub(super) min: [i32; 3],
    pub(super) step: [i32; 3],
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

    pub(super) fn block(&self, axis: usize, index: i32) -> i32 {
        self.min[axis] + index * self.step[axis]
    }

    /// `maxBlock*`: one past the last covered block along the axis.
    pub(super) fn max_block(&self, axis: usize) -> i32 {
        self.min[axis] + self.size[axis] * self.step[axis] - 1
    }
}

pub(super) fn volume_scratch(volume: &DensityVolume) -> Vec<f32> {
    vec![0.0f32; volume.slot_count()]
}

/// The volume-path forms of the binary nodes, matching vanilla's
/// specialized sampler volume loops: constant folds operate on the
/// dynamic side in place, and the general `mul`/`div` loops apply the
/// raw operation per slot without the point sampler's left-zero guard.
pub(super) fn sample_binary_volume(
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

pub(super) fn sample_range_choice_volume(
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
pub(super) fn sample_interpolated_volume(
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

pub(super) fn sample_interpolated_block_step(
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

pub(super) struct InterpolatedCellParams<'a> {
    pub(super) volume: &'a DensityVolume,
    pub(super) cell_volume: &'a DensityVolume,
    pub(super) xz_inv: f32,
    pub(super) y_inv: f32,
    pub(super) cell_size_xz: i32,
    pub(super) cell_size_y: i32,
}

pub(super) struct CellCorners {
    pub(super) v000: f32,
    pub(super) v100: f32,
    pub(super) v010: f32,
    pub(super) v110: f32,
    pub(super) v001: f32,
    pub(super) v101: f32,
    pub(super) v011: f32,
    pub(super) v111: f32,
}

/// `InterpolatedFunction.Sampler.fillCell`: horizontal axes interpolate
/// with the reciprocal-cell multiply, and the vertical direction is a
/// repeated addition of `valueStep` — not per-point lerp evaluation.
pub(super) fn fill_interpolated_cell(
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
