//! Data-driven biome placement (T2 slice F).
//!
//! Vanilla 26.3 places overworld biomes by sampling six `noise_router`
//! densities on a 4-block grid, quantizing each `f32` coordinate to a
//! fixed-point integer (`value × 10 000` truncated toward zero), and
//! picking the parameter entry with the smallest squared-distance
//! "fitness" across the six inclusive ranges plus a constant offset term.
//! The parameter table itself lives in the game's code, not its datapack
//! JSON; under ADR-0014 the owner provisions it as a numeric capture
//! (`rustmc/biome_placement/<preset>.psv` beside the worldgen data root,
//! never committed) produced by the disposable extraction harness
//! documented in `docs/PROVENANCE.md`.
//!
//! The pipeline is structurally free of structure blending and
//! post-processing (same stance as the density and aquifer modules), so
//! the placement answer is the raw resolver value; blender-driven biome
//! borders near structures remain a known residual.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use crate::vanilla::density::{Density, DensityVolume};

/// Fixed-point scale of a quantized climate coordinate.
const QUANTIZATION_FACTOR: f32 = 10000.0;

/// One dimension's inclusive quantized interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Param {
    pub min: i64,
    pub max: i64,
}

impl Param {
    /// Distance from a target coordinate to the interval: zero inside,
    /// otherwise the gap to the nearer end.
    pub fn distance(&self, target: i64) -> i64 {
        let above = target - self.max;
        let below = self.min - target;
        if above > 0 { above } else { below.max(0) }
    }
}

/// One placement entry: six interval dimensions plus the constant offset
/// penalty (quantized like the coordinates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterPoint {
    pub temperature: Param,
    pub humidity: Param,
    pub continentalness: Param,
    pub erosion: Param,
    pub depth: Param,
    pub weirdness: Param,
    pub offset: i64,
}

impl ParameterPoint {
    /// Squared interval distances summed over all seven dimensions.
    pub fn fitness(&self, t: &[i64; 6]) -> i64 {
        let dims = [
            &self.temperature,
            &self.humidity,
            &self.continentalness,
            &self.erosion,
            &self.depth,
            &self.weirdness,
        ];
        dims.iter()
            .zip(t)
            .map(|(param, target)| square(param.distance(*target)))
            .sum::<i64>()
            + square(self.offset)
    }

    /// Squared interval distances summed over the five non-depth dimensions
    /// `[temperature, humidity, continentalness, erosion, weirdness]` plus the
    /// squared offset term.
    pub fn fixed_fitness(&self, fixed: &[i64; 5]) -> i64 {
        square(self.temperature.distance(fixed[0]))
            + square(self.humidity.distance(fixed[1]))
            + square(self.continentalness.distance(fixed[2]))
            + square(self.erosion.distance(fixed[3]))
            + square(self.weirdness.distance(fixed[4]))
            + square(self.offset)
    }
}

impl ParameterPoint {
    /// [`Self::fitness`], or `None` as soon as the partial sum proves this
    /// row (at index `row`) cannot beat `best` under the `(fitness, row)`
    /// order. Terms are summed in the same order as `fitness`, so a
    /// returned value is identical to it.
    fn fitness_if_better(
        &self,
        t: &[i64; 6],
        best: Option<(i64, usize)>,
        row: usize,
    ) -> Option<i64> {
        let loses = |partial: i64| {
            best.is_some_and(|(best_fitness, best_row)| {
                partial > best_fitness || (partial == best_fitness && row > best_row)
            })
        };
        let dims = [
            &self.temperature,
            &self.humidity,
            &self.continentalness,
            &self.erosion,
            &self.depth,
            &self.weirdness,
        ];
        let mut partial = 0i64;
        for (param, target) in dims.iter().zip(t) {
            partial += square(param.distance(*target));
            if loses(partial) {
                return None;
            }
        }
        partial += square(self.offset);
        (!loses(partial)).then_some(partial)
    }
}

/// Rows per search-tree leaf; small leaves keep the bounding boxes tight.
const SEARCH_LEAF_ROWS: usize = 8;

/// Maximum rows [`BiomePlacement::find_column`] examines per cell before
/// falling back to [`BiomePlacement::find_row`].
const COLUMN_SCAN_FALLBACK_ROWS: usize = 1024;

/// One node of the placement table's bounding-box tree: the union of its
/// rows' six intervals, their smallest offset penalty, and their lowest row
/// index (for the earlier-row tie rule).
#[derive(Debug, Clone)]
struct SearchNode {
    bounds: [Param; 6],
    min_offset_squared: i64,
    first_row: usize,
    kind: SearchKind,
}

#[derive(Debug, Clone)]
enum SearchKind {
    Leaf(Box<[usize]>),
    Branch(usize, usize),
}

impl SearchNode {
    /// No row in this subtree has a smaller fitness for `target`: each
    /// row's intervals lie inside `bounds`, so each per-dimension distance is
    /// at least the box's, and its offset is at least the minimum.
    fn lower_bound(&self, target: &[i64; 6]) -> i64 {
        self.bounds
            .iter()
            .zip(target)
            .map(|(param, value)| square(param.distance(*value)))
            .sum::<i64>()
            + self.min_offset_squared
    }
}

/// `(fitness, row)` strictly precedes `best`: lower fitness, or equal
/// fitness at an earlier row.
fn beats(fitness: i64, row: usize, best: Option<(i64, usize)>) -> bool {
    best.is_none_or(|(best_fitness, best_row)| {
        fitness < best_fitness || (fitness == best_fitness && row < best_row)
    })
}

/// No row with fitness at least `lower` and index at least `first_row` can
/// precede `best`.
fn cannot_beat(lower: i64, first_row: usize, best: Option<(i64, usize)>) -> bool {
    best.is_some_and(|(best_fitness, best_row)| {
        lower > best_fitness || (lower == best_fitness && first_row > best_row)
    })
}

fn point_params(point: &ParameterPoint) -> [Param; 6] {
    [
        point.temperature,
        point.humidity,
        point.continentalness,
        point.erosion,
        point.depth,
        point.weirdness,
    ]
}

/// Builds the subtree for `rows` into `tree` and returns its index. Rows
/// are split at the median of the widest dimension's interval midpoints.
fn build_node(
    entries: &[(String, ParameterPoint)],
    rows: &mut [usize],
    tree: &mut Vec<SearchNode>,
) -> usize {
    let mut bounds = [Param {
        min: i64::MAX,
        max: i64::MIN,
    }; 6];
    let mut min_offset_squared = i64::MAX;
    let mut first_row = usize::MAX;
    for &row in rows.iter() {
        let Some((_, point)) = entries.get(row) else {
            continue;
        };
        for (bound, param) in bounds.iter_mut().zip(point_params(point)) {
            bound.min = bound.min.min(param.min);
            bound.max = bound.max.max(param.max);
        }
        min_offset_squared = min_offset_squared.min(square(point.offset));
        first_row = first_row.min(row);
    }
    let index = tree.len();
    tree.push(SearchNode {
        bounds,
        min_offset_squared,
        first_row,
        kind: SearchKind::Leaf(Box::new([])),
    });
    if rows.len() <= SEARCH_LEAF_ROWS {
        if let Some(node) = tree.get_mut(index) {
            node.kind = SearchKind::Leaf(rows.to_vec().into_boxed_slice());
        }
        return index;
    }
    let axis = (0..6)
        .max_by_key(|&axis| bounds[axis].max.saturating_sub(bounds[axis].min))
        .unwrap_or(0);
    let midpoint = |row: &usize| {
        entries
            .get(*row)
            .map(|(_, point)| {
                let param = point_params(point)[axis];
                param.min / 2 + param.max / 2
            })
            .unwrap_or(0)
    };
    rows.sort_unstable_by_key(midpoint);
    let (low, high) = rows.split_at_mut(rows.len() / 2);
    let left = build_node(entries, low, tree);
    let right = build_node(entries, high, tree);
    if let Some(node) = tree.get_mut(index) {
        node.kind = SearchKind::Branch(left, right);
    }
    index
}

fn square(value: i64) -> i64 {
    value * value
}

/// Java `(long)(float)` truncation toward zero after the f32 multiply.
pub fn quantize_coord(value: f32) -> i64 {
    (value * QUANTIZATION_FACTOR) as i64
}

/// Five non-depth climate coordinates `[temperature, humidity, continentalness,
/// erosion, weirdness]` held constant across one column run.
pub fn fixed_climate_coords(target: &[i64; 6]) -> [i64; 5] {
    [target[0], target[1], target[2], target[3], target[5]]
}

/// Number of leading cells in `targets` whose five non-depth coordinates equal
/// `targets[0]`'s.
pub fn fixed_run_len(targets: &[[i64; 6]]) -> usize {
    let Some(first) = targets.first() else {
        return 0;
    };
    let fixed = fixed_climate_coords(first);
    targets
        .iter()
        .skip(1)
        .position(|target| fixed_climate_coords(target) != fixed)
        .map_or(targets.len(), |offset| offset + 1)
}

#[derive(Debug, Clone, Copy)]
struct SortedRow {
    fixed_cost: i64,
    row: usize,
    depth: Param,
}

fn cmp_sorted_row(lhs: &SortedRow, rhs: &SortedRow) -> std::cmp::Ordering {
    lhs.fixed_cost
        .cmp(&rhs.fixed_cost)
        .then_with(|| lhs.row.cmp(&rhs.row))
}

/// Builds the sorted, merged, disjoint union of all rows' `depth` intervals.
fn merge_depth_ranges(entries: &[(String, ParameterPoint)]) -> Vec<Param> {
    let mut ranges: Vec<Param> = entries.iter().map(|(_, point)| point.depth).collect();
    ranges.sort_unstable_by(|lhs, rhs| lhs.min.cmp(&rhs.min).then_with(|| lhs.max.cmp(&rhs.max)));
    let mut merged: Vec<Param> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.min <= last.max
        {
            last.max = last.max.max(range.max);
        } else {
            merged.push(range);
        }
    }
    merged
}

/// Reusable scratch buffers for [`BiomePlacement::find_column`], holding one
/// run's sorted row entries across runs and chunks without per-cell
/// allocations.
#[derive(Debug, Default, Clone)]
pub struct ColumnSearchScratch {
    order: Vec<SortedRow>,
}

/// A preset's placement table, in the exact order the game builds it:
/// fitness ties keep the earlier entry.
#[derive(Debug, Clone)]
pub struct BiomePlacement {
    entries: Vec<(String, ParameterPoint)>,
    /// Bounding-box tree over `entries` for exact nearest-row search.
    tree: Vec<SearchNode>,
    /// Union of all rows' `depth` intervals, sorted and merged into disjoint
    /// ranges for exact `D(t4)` lower bounds.
    depth_ranges: Vec<Param>,
    /// Row that won the previous lookup. Neighbouring quart cells usually
    /// select the same biome, so testing it first gives a tight bound for
    /// pruning; it never changes which row wins (see [`Self::find`]).
    last_winner: Cell<Option<usize>>,
}

impl BiomePlacement {
    /// Looks for `rustmc/biome_placement/<preset>.psv` under the worldgen
    /// data root; `None` when the operator has not provisioned it.
    pub fn load(data_root: &Path, preset: &str) -> Option<Self> {
        let path: PathBuf = data_root
            .join("rustmc")
            .join("biome_placement")
            .join(format!("{preset}.psv"));
        let text = fs::read_to_string(path).ok()?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut entries = Vec::new();
        for line in text.lines() {
            let mut fields = line.split('|');
            let (
                Some(_index),
                Some(id),
                Some(t),
                Some(h),
                Some(c),
                Some(e),
                Some(d),
                Some(w),
                Some(off),
            ) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            )
            else {
                return None;
            };
            let point = ParameterPoint {
                temperature: parse_param(t, "t")?,
                humidity: parse_param(h, "h")?,
                continentalness: parse_param(c, "c")?,
                erosion: parse_param(e, "e")?,
                depth: parse_param(d, "d")?,
                weirdness: parse_param(w, "w")?,
                offset: off
                    .strip_prefix("off=")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default(),
            };
            entries.push((id.to_owned(), point));
        }
        (!entries.is_empty()).then(|| Self::from_entries(entries))
    }

    /// Nearest entry for a quantized target; ties resolve to the earlier
    /// table entry, matching the reference (brute-force) implementation
    /// the game itself uses for testing.
    ///
    /// The scan is exact but pruned: a row's fitness is a sum of
    /// non-negative terms, so once its partial sum shows it cannot beat the
    /// best `(fitness, row)` so far under that order it is skipped. The
    /// previous lookup's winner is evaluated first to start with a tight
    /// bound. Both only skip rows that cannot win, so the result is the
    /// row the full scan returns.
    pub fn find(&self, target: &[i64; 6]) -> Option<&str> {
        let row = self.find_row(target)?;
        self.entry_id(row)
    }

    /// Biome identifier for table row `row`, or `None` if out of range.
    pub fn entry_id(&self, row: usize) -> Option<&str> {
        self.entries.get(row).map(|(id, _)| id.as_str())
    }

    /// Minimum squared depth distance `D(t4)` across every row in the table:
    /// zero when `depth` lies inside any row's depth interval, otherwise the
    /// squared distance to the nearest interval endpoint.
    fn min_depth_fitness(&self, depth: i64) -> i64 {
        let idx = self.depth_ranges.partition_point(|range| range.max < depth);
        let mut best_dist = match self.depth_ranges.get(idx) {
            Some(right) => right.distance(depth),
            None => i64::MAX,
        };
        if best_dist > 0
            && let Some(left) = idx.checked_sub(1).and_then(|i| self.depth_ranges.get(i))
        {
            best_dist = best_dist.min(depth - left.max);
        }
        if best_dist == i64::MAX {
            0
        } else {
            square(best_dist)
        }
    }

    /// Resolves a run of cells sharing the five non-depth coordinates
    /// `fixed = [temperature, humidity, continentalness, erosion, weirdness]`
    /// across `depths`, writing the winning table row index for each depth to
    /// `out` (after clearing `out`).
    ///
    /// Every cell yields the exact same winning row as [`Self::find`] (lowest
    /// `fitness`, ties resolved to the earlier table row).
    pub fn find_column(
        &self,
        fixed: [i64; 5],
        depths: &[i64],
        scratch: &mut ColumnSearchScratch,
        out: &mut Vec<usize>,
    ) {
        out.clear();
        if self.entries.is_empty() || depths.is_empty() {
            return;
        }
        let n = self.entries.len();
        scratch.order.clear();
        scratch.order.reserve(n);
        for (row, (_, point)) in self.entries.iter().enumerate() {
            scratch.order.push(SortedRow {
                fixed_cost: point.fixed_fitness(&fixed),
                row,
                depth: point.depth,
            });
        }
        let (scan_len, next_beyond_limit) = if n > COLUMN_SCAN_FALLBACK_ROWS {
            scratch
                .order
                .select_nth_unstable_by(COLUMN_SCAN_FALLBACK_ROWS, cmp_sorted_row);
            scratch.order[..COLUMN_SCAN_FALLBACK_ROWS].sort_unstable_by(cmp_sorted_row);
            (
                COLUMN_SCAN_FALLBACK_ROWS,
                Some(scratch.order[COLUMN_SCAN_FALLBACK_ROWS]),
            )
        } else {
            scratch.order.sort_unstable_by(cmp_sorted_row);
            (n, None)
        };

        out.reserve(depths.len());
        let scan_rows = &scratch.order[..scan_len];
        let Some((&first, rest)) = scan_rows.split_first() else {
            return;
        };
        for &depth in depths {
            let min_depth_term = self.min_depth_fitness(depth);
            let mut best_fitness = first.fixed_cost + square(first.depth.distance(depth));
            let mut best_row = first.row;
            let mut terminated = next_beyond_limit.is_none();

            for item in rest {
                if cannot_beat(
                    item.fixed_cost + min_depth_term,
                    item.row,
                    Some((best_fitness, best_row)),
                ) {
                    terminated = true;
                    break;
                }
                let fitness = item.fixed_cost + square(item.depth.distance(depth));
                if beats(fitness, item.row, Some((best_fitness, best_row))) {
                    best_fitness = fitness;
                    best_row = item.row;
                }
            }
            if !terminated
                && let Some(next) = next_beyond_limit
                && cannot_beat(
                    next.fixed_cost + min_depth_term,
                    next.row,
                    Some((best_fitness, best_row)),
                )
            {
                terminated = true;
            }
            let winner = if terminated {
                self.last_winner.set(Some(best_row));
                best_row
            } else {
                let target = [fixed[0], fixed[1], fixed[2], fixed[3], depth, fixed[4]];
                self.find_row_with_seed(&target, Some((best_fitness, best_row)))
                    .unwrap_or(best_row)
            };
            out.push(winner);
        }
    }

    fn find_row(&self, target: &[i64; 6]) -> Option<usize> {
        self.find_row_with_seed(target, None)
    }

    fn find_row_with_seed(&self, target: &[i64; 6], seed: Option<(i64, usize)>) -> Option<usize> {
        let mut best = seed;
        if let Some(row) = self.last_winner.get()
            && let Some((_, point)) = self.entries.get(row)
        {
            let fitness = point.fitness(target);
            if beats(fitness, row, best) {
                best = Some((fitness, row));
            }
        }
        if !self.tree.is_empty() {
            self.search(0, target, &mut best);
        }
        let (_, row) = best?;
        self.last_winner.set(Some(row));
        Some(row)
    }

    #[cfg(test)]
    pub(crate) fn find_row_for_test(&self, target: &[i64; 6]) -> Option<usize> {
        self.find_row(target)
    }

    /// Builds the table and its search tree.
    fn from_entries(entries: Vec<(String, ParameterPoint)>) -> Self {
        let mut tree = Vec::new();
        let mut rows: Vec<usize> = (0..entries.len()).collect();
        if !rows.is_empty() {
            build_node(&entries, &mut rows, &mut tree);
        }
        let depth_ranges = merge_depth_ranges(&entries);
        Self {
            entries,
            tree,
            depth_ranges,
            last_winner: Cell::new(None),
        }
    }

    /// Depth-first search below `node`, updating `best` with any row that
    /// beats it under the `(fitness, row)` order. A subtree is skipped only
    /// when its lower bound proves none of its rows can.
    fn search(&self, node: usize, target: &[i64; 6], best: &mut Option<(i64, usize)>) {
        let Some(current) = self.tree.get(node) else {
            return;
        };
        match &current.kind {
            SearchKind::Leaf(rows) => {
                for &row in rows.iter() {
                    if best.is_some_and(|(_, best_row)| best_row == row) {
                        continue;
                    }
                    let Some((_, point)) = self.entries.get(row) else {
                        continue;
                    };
                    if let Some(fitness) = point.fitness_if_better(target, *best, row)
                        && beats(fitness, row, *best)
                    {
                        *best = Some((fitness, row));
                    }
                }
            }
            SearchKind::Branch(left, right) => {
                let bound = |child: usize| {
                    self.tree
                        .get(child)
                        .map(|node| (node.lower_bound(target), node.first_row))
                };
                let mut children = [(*left, bound(*left)), (*right, bound(*right))];
                if children[1].1 < children[0].1 {
                    children.swap(0, 1);
                }
                for (child, child_bound) in children {
                    let Some((lower, first_row)) = child_bound else {
                        continue;
                    };
                    if cannot_beat(lower, first_row, *best) {
                        continue;
                    }
                    self.search(child, target, best);
                }
            }
        }
    }

    /// The biomes this table can select, in the order the table declares
    /// them and with repeats dropped. A biome with several parameter ranges
    /// occupies several rows, so the reference runtime's set of possible
    /// biomes is this same first-appearance dedupe of the table, and the
    /// placement stage numbers its features over exactly that list.
    pub fn possible_biomes(&self) -> Vec<&str> {
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        for (id, _) in &self.entries {
            if seen.insert(id.as_str()) {
                names.push(id.as_str());
            }
        }
        names
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn parse_param(field: &str, name: &str) -> Option<Param> {
    let value = field.strip_prefix(&format!("{name}="))?;
    if let Some(range) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        // Both bounds are integers and either may be negative, so the
        // separator is the first dash whose two sides both parse.
        for split in 1..range.len() {
            if range.as_bytes()[split] != b'-' {
                continue;
            }
            if let (Ok(min), Ok(max)) = (
                range[..split].parse::<i64>(),
                range[split + 1..].parse::<i64>(),
            ) {
                return Some(Param { min, max });
            }
        }
        None
    } else {
        let point = value.parse().ok()?;
        Some(Param {
            min: point,
            max: point,
        })
    }
}

/// The six climate coordinate samplers of a dimension, wired from the
/// compiled `noise_router` exactly as the chunk generator wires them:
/// temperature and vegetation feed the humidity axis, ridges feed the
/// weirdness axis.
pub struct ClimateSampler {
    pub temperature: Density,
    pub vegetation: Density,
    pub continents: Density,
    pub erosion: Density,
    pub depth: Density,
    pub ridges: Density,
}

impl ClimateSampler {
    /// Samples the target at block coordinates (the chunk resolver calls
    /// this at quart-cell bottoms, `quart * 4`).
    pub fn target_at_block(&self, x: i32, y: i32, z: i32) -> [i64; 6] {
        [
            quantize_coord(self.temperature.sample(x, y, z)),
            quantize_coord(self.vegetation.sample(x, y, z)),
            quantize_coord(self.continents.sample(x, y, z)),
            quantize_coord(self.erosion.sample(x, y, z)),
            quantize_coord(self.depth.sample(x, y, z)),
            quantize_coord(self.ridges.sample(x, y, z)),
        ]
    }

    /// Fills `out` with quantized climate targets over `volume` in
    /// `DensityVolume::index` order using `sample_volume` for each of the
    /// six climate densities.
    pub fn targets_for_volume(
        &self,
        volume: &DensityVolume,
        scratch: &mut Vec<f32>,
        out: &mut Vec<[i64; 6]>,
    ) {
        let count = volume.slot_count();
        scratch.resize(count, 0.0);
        out.resize(count, [0; 6]);
        let samplers = [
            &self.temperature,
            &self.vegetation,
            &self.continents,
            &self.erosion,
            &self.depth,
            &self.ridges,
        ];
        for (dim, density) in samplers.iter().enumerate() {
            density.sample_volume(volume, scratch);
            for (target, &raw) in out.iter_mut().zip(scratch.iter()) {
                target[dim] = quantize_coord(raw);
            }
        }
    }

    /// Biome identifier for a block position given its placement table.
    pub fn biome(&self, placement: &BiomePlacement, x: i32, y: i32, z: i32) -> Option<String> {
        let quart = (x >> 2, y >> 2, z >> 2);
        let target = self.target_at_block(quart.0 << 2, quart.1 << 2, quart.2 << 2);
        Some(placement.find(&target)?.to_owned())
    }
}

/// Small table used by unit tests; shapes mirror the captured file but
/// the numbers are self-authored.
#[cfg(test)]
fn test_table() -> BiomePlacement {
    let text = concat!(
        "0|minecraft:plains|t=[-2000-2000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
        "1|minecraft:forest|t=[2000-10000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
    );
    BiomePlacement::parse(text).expect("test table parses")
}

#[cfg(test)]
mod tests;
