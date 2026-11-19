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

/// Reusable scratch buffers for [`BiomePlacement::find_column`], holding one
/// run's per-row fixed-dimension fitness `A_i` and sorted row entries across
/// runs and chunks without per-cell allocations.
#[derive(Debug, Default, Clone)]
pub struct ColumnSearchScratch {
    fixed_costs: Vec<i64>,
    order: Vec<SortedRow>,
}

/// A preset's placement table, in the exact order the game builds it:
/// fitness ties keep the earlier entry.
#[derive(Debug, Clone)]
pub struct BiomePlacement {
    entries: Vec<(String, ParameterPoint)>,
    /// Bounding-box tree over `entries` for exact nearest-row search.
    tree: Vec<SearchNode>,
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
        scratch.fixed_costs.clear();
        scratch.fixed_costs.reserve(n);
        scratch.order.clear();
        scratch.order.reserve(n);
        for (row, (_, point)) in self.entries.iter().enumerate() {
            let fixed_cost = point.fixed_fitness(&fixed);
            scratch.fixed_costs.push(fixed_cost);
            scratch.order.push(SortedRow {
                fixed_cost,
                row,
                depth: point.depth,
            });
        }
        scratch.order.sort_unstable_by(|lhs, rhs| {
            lhs.fixed_cost
                .cmp(&rhs.fixed_cost)
                .then_with(|| lhs.row.cmp(&rhs.row))
        });

        out.reserve(depths.len());
        let Some((&first, rest)) = scratch.order.split_first() else {
            return;
        };
        for &depth in depths {
            let mut best_fitness = first.fixed_cost + square(first.depth.distance(depth));
            let mut best_row = first.row;

            for item in rest {
                let a = item.fixed_cost;
                if a > best_fitness {
                    break;
                }
                if a == best_fitness && item.row > best_row {
                    continue;
                }
                let fitness = a + square(item.depth.distance(depth));
                if fitness < best_fitness || (fitness == best_fitness && item.row < best_row) {
                    best_fitness = fitness;
                    best_row = item.row;
                }
            }
            out.push(best_row);
        }
    }

    fn find_row(&self, target: &[i64; 6]) -> Option<usize> {
        let mut best: Option<(i64, usize)> = self
            .last_winner
            .get()
            .and_then(|row| Some((self.entries.get(row)?.1.fitness(target), row)));
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
        Self {
            entries,
            tree,
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
mod tests {
    use super::*;

    /// The reference rule `find` must reproduce: the first row with the
    /// smallest fitness.
    fn brute_force_row(table: &BiomePlacement, target: &[i64; 6]) -> Option<usize> {
        let mut best: Option<(i64, usize)> = None;
        for (row, (_, point)) in table.entries.iter().enumerate() {
            let fitness = point.fitness(target);
            if best.is_none_or(|(previous, _)| fitness < previous) {
                best = Some((fitness, row));
            }
        }
        best.map(|(_, row)| row)
    }

    fn brute_force<'a>(table: &'a BiomePlacement, target: &[i64; 6]) -> Option<&'a str> {
        let row = brute_force_row(table, target)?;
        table.entry_id(row)
    }

    fn lcg_targets(count: usize, spread: i64) -> Vec<[i64; 6]> {
        let mut state = 0x2026_u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as i64 % (2 * spread + 1)) - spread
        };
        (0..count)
            .map(|_| [next(), next(), next(), next(), next(), next()])
            .collect()
    }

    #[test]
    fn pruned_search_matches_brute_force_including_ties() {
        // Identical rows and rows that tie at many targets: the earlier row
        // must keep winning however the previous lookup primed the search.
        let row = |name: &str, min: i64, max: i64, offset: i64| {
            let param = Param { min, max };
            (
                name.to_owned(),
                ParameterPoint {
                    temperature: param,
                    humidity: param,
                    continentalness: param,
                    erosion: param,
                    depth: param,
                    weirdness: param,
                    offset,
                },
            )
        };
        let table = BiomePlacement::from_entries(vec![
            row("a", -100, 100, 0),
            row("b", -100, 100, 0),
            row("c", 0, 500, 0),
            row("d", -500, 0, 0),
            row("e", 0, 500, 0),
            row("f", -50, 50, 10),
        ]);
        for target in lcg_targets(5_000, 800) {
            assert_eq!(
                table.find(&target),
                brute_force(&table, &target),
                "{target:?}"
            );
        }
        // Prime with a later tying row, then ask for a point where an earlier
        // row ties with it.
        table.last_winner.set(Some(4));
        assert_eq!(table.find(&[300; 6]), Some("c"));
        table.last_winner.set(Some(1));
        assert_eq!(table.find(&[0; 6]), Some("a"));
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pruned_search_matches_brute_force_on_the_vanilla_table`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pruned_search_matches_brute_force_on_the_vanilla_table() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let table = BiomePlacement::load(&root, "overworld").expect("overworld placement table");
        // Quantized climate values span roughly +/-2 * 10_000.
        for target in lcg_targets(50_000, 20_000) {
            assert_eq!(
                table.find(&target),
                brute_force(&table, &target),
                "{target:?}"
            );
        }
    }

    #[test]
    fn quantization_truncates_toward_zero() {
        assert_eq!(quantize_coord(0.7), 7000);
        assert_eq!(quantize_coord(-0.375), -3750);
        // 1.1F * 10000 rounds in f32 just under 11000 and truncates.
        assert_eq!(quantize_coord(1.1), 11000);
    }

    #[test]
    fn interval_distance_is_zero_inside() {
        let param = Param { min: -4, max: 7 };
        assert_eq!(param.distance(-10), 6);
        assert_eq!(param.distance(0), 0);
        assert_eq!(param.distance(10), 3);
    }

    /// The dimension's possible-biome set is the table read in order with
    /// repeats dropped: a biome with two parameter ranges occupies two rows
    /// but one slot, and the slot is its first row's.
    #[test]
    fn possible_biomes_follow_declaration_order_and_drop_repeats() {
        let text = concat!(
            "0|minecraft:plains|t=[-2000-2000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
            "1|minecraft:forest|t=[2000-10000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
            "2|minecraft:plains|t=[-10000--2000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
        );
        let table = BiomePlacement::parse(text).expect("test table parses");
        assert_eq!(table.len(), 3);
        assert_eq!(
            table.possible_biomes(),
            vec!["minecraft:plains", "minecraft:forest"],
        );
    }

    #[test]
    fn parsing_handles_ranges_points_and_negative_bounds() {
        let table = test_table();
        assert_eq!(table.len(), 2);
        assert_eq!(table.entries[0].1.humidity, Param::default_full());
        assert_eq!(table.entries[1].1.temperature.min, 2000);
    }

    #[test]
    fn nearest_entry_wins_and_ties_keep_the_earlier() {
        let table = test_table();
        assert_eq!(table.find(&[5000, 0, 0, 0, 0, 0]), Some("minecraft:forest"));
        assert_eq!(
            table.find(&[-1000, 0, 0, 0, 0, 0]),
            Some("minecraft:plains")
        );
        // Exactly tied fitness between the two entries keeps entry 0.
        assert_eq!(table.find(&[0, 0, 0, 0, 0, 0]), Some("minecraft:plains"));
    }

    fn resolve_column_by_runs(
        table: &BiomePlacement,
        column: &[[i64; 6]],
        scratch: &mut ColumnSearchScratch,
        depths: &mut Vec<i64>,
        rows: &mut Vec<usize>,
        out: &mut Vec<usize>,
    ) {
        out.clear();
        let mut start = 0;
        while start < column.len() {
            let len = fixed_run_len(&column[start..]);
            let fixed = fixed_climate_coords(&column[start]);
            depths.clear();
            depths.extend(column[start..start + len].iter().map(|target| target[4]));
            table.find_column(fixed, depths, scratch, rows);
            assert_eq!(rows.len(), len);
            out.extend_from_slice(rows);
            start += len;
        }
    }

    #[test]
    fn column_search_matches_find_and_brute_force_including_ties() {
        let entry =
            |name: &str, t: Param, h: Param, c: Param, e: Param, d: Param, w: Param, off| {
                (
                    name.to_owned(),
                    ParameterPoint {
                        temperature: t,
                        humidity: h,
                        continentalness: c,
                        erosion: e,
                        depth: d,
                        weirdness: w,
                        offset: off,
                    },
                )
            };
        let p = |min, max| Param { min, max };
        let zero = p(0, 0);
        // Deliberately craft equal rows, equal fixed parts A_i, non-zero
        // offsets, and late-sorted earlier-index ties (row 0 has A=10000, d=0
        // whereas row 1 has A=0, d=100: at target [0;6], row 1 sets best=(10000,1)
        // first, then row 0 at A==best_fitness must still beat it by index).
        let table = BiomePlacement::from_entries(vec![
            entry("early_high_a", p(100, 100), zero, zero, zero, zero, zero, 0),
            entry("late_low_a", zero, zero, zero, zero, p(100, 100), zero, 0),
            entry("dup_low_a", zero, zero, zero, zero, p(100, 100), zero, 0),
            entry("dup_high_a", p(100, 100), zero, zero, zero, zero, zero, 0),
            entry(
                "equal_a_mid",
                p(-50, 50),
                p(-50, 50),
                zero,
                zero,
                p(-40, 40),
                zero,
                120,
            ),
            entry(
                "equal_a_alt",
                p(-50, 50),
                p(-50, 50),
                zero,
                zero,
                p(20, 80),
                zero,
                120,
            ),
            entry(
                "offset_tie_1",
                p(-100, 100),
                p(-100, 100),
                p(-100, 100),
                zero,
                p(-100, 100),
                zero,
                100,
            ),
            entry(
                "offset_tie_2",
                p(-100, 100),
                p(-100, 100),
                p(-100, 100),
                zero,
                p(-100, 100),
                zero,
                -100,
            ),
            entry(
                "wide_a",
                p(200, 400),
                p(-400, 400),
                p(-400, 400),
                p(-400, 400),
                p(-200, 200),
                p(-400, 400),
                25,
            ),
            entry(
                "wide_b",
                p(200, 400),
                p(-400, 400),
                p(-400, 400),
                p(-400, 400),
                p(100, 300),
                p(-400, 400),
                25,
            ),
            entry(
                "identical_1",
                p(200, 500),
                p(-200, 200),
                p(-100, 100),
                p(0, 300),
                p(-300, 300),
                p(-500, 500),
                0,
            ),
            entry(
                "identical_2",
                p(200, 500),
                p(-200, 200),
                p(-100, 100),
                p(0, 300),
                p(-300, 300),
                p(-500, 500),
                0,
            ),
        ]);

        let mut scratch = ColumnSearchScratch::default();
        let mut depths = Vec::new();
        let mut rows = Vec::new();
        let mut resolved = Vec::new();

        // Empty depths leaves `out` empty.
        rows.push(999);
        table.find_column([0; 5], &[], &mut scratch, &mut rows);
        assert!(rows.is_empty());

        // Direct tie cases including A == best_fitness and later/earlier indices.
        let edge_targets = [
            [0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 50, 0],
            [0, 0, 0, 0, 100, 0],
            [100, 0, 0, 0, 0, 0],
            [300, 0, 0, 150, -100, 0],
        ];
        resolve_column_by_runs(
            &table,
            &edge_targets,
            &mut scratch,
            &mut depths,
            &mut rows,
            &mut resolved,
        );
        for (target, &row) in edge_targets.iter().zip(&resolved) {
            assert_eq!(Some(row), table.find_row_for_test(target), "{target:?}");
            assert_eq!(Some(row), brute_force_row(&table, target), "{target:?}");
            assert_eq!(
                table.entry_id(row),
                brute_force(&table, target),
                "{target:?}"
            );
        }
        assert_eq!(resolved[0], 0);

        // Reuse the same scratch across a different-sized table and back.
        let small = test_table();
        small.find_column([0; 5], &[0, 1000], &mut scratch, &mut rows);
        assert_eq!(rows, vec![0, 0]);

        // 3,000 random column runs: half uniform 5D fixed coords, half with
        // mid-column fixed-coord splits (including single-cell runs).
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next_i64 = |spread: i64| -> i64 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as i64 % (2 * spread + 1)) - spread
        };
        let mut column = Vec::with_capacity(24);
        for run in 0..3_000 {
            column.clear();
            let mut fixed = [
                next_i64(600),
                next_i64(600),
                next_i64(600),
                next_i64(600),
                next_i64(600),
            ];
            let len = 16 + (run % 17);
            for cell in 0..len {
                if run % 2 == 1 && (cell == 3 || cell == 4 || cell == 9 || cell == 13) {
                    fixed = [
                        next_i64(600),
                        next_i64(600),
                        next_i64(600),
                        next_i64(600),
                        next_i64(600),
                    ];
                }
                let depth = next_i64(600);
                column.push([fixed[0], fixed[1], fixed[2], fixed[3], depth, fixed[4]]);
            }
            resolve_column_by_runs(
                &table,
                &column,
                &mut scratch,
                &mut depths,
                &mut rows,
                &mut resolved,
            );
            for (target, &row) in column.iter().zip(&resolved) {
                assert_eq!(
                    Some(row),
                    table.find_row_for_test(target),
                    "run {run} {target:?}"
                );
                assert_eq!(
                    Some(row),
                    brute_force_row(&table, target),
                    "run {run} {target:?}"
                );
                assert_eq!(
                    table.entry_id(row),
                    table.find(target),
                    "run {run} {target:?}"
                );
            }
        }
    }

    #[test]
    fn column_search_fixed_and_depth_terms_stay_far_below_i64_max() {
        // Documented bound for quantized coordinates (`value * 10_000`):
        // Overworld climate noises stay roughly within [-2.0, 2.0] (+/-20_000)
        // and depth within [-3.0, 3.0] (+/-30_000), while the 26.3 overworld
        // placement table bounds lie within [-12_000, 11_000] with offset = 0.
        // Even with a conservative 50x margin on targets (|t_d| <= 1_000_000,
        // i.e., |density| <= 100.0) and parameter/offset bounds within
        // [-100_000, 100_000]:
        //   max_distance = 1_100_000
        //   max_term     = 1_100_000^2 = 1_210_000_000_000 (1.21e12)
        //   |A_i|       <= 5 * 1.21e12 + 100_000^2 = 6_060_000_000_000
        //   |A_i| + max_depth_term <= 7_270_000_000_000 (7.27e12),
        // which is over 1,260,000x smaller than i64::MAX (~9.22e18).
        const MAX_QUANTIZED_COORD: i64 = 1_000_000;
        const MAX_PARAM_BOUND: i64 = 100_000;
        const DOCUMENTED_TOTAL_BOUND: i64 = 7_270_000_000_000;

        let corner = Param {
            min: -MAX_PARAM_BOUND,
            max: -MAX_PARAM_BOUND,
        };
        let table = BiomePlacement::from_entries(vec![(
            "extreme".to_owned(),
            ParameterPoint {
                temperature: corner,
                humidity: corner,
                continentalness: corner,
                erosion: corner,
                depth: corner,
                weirdness: corner,
                offset: MAX_PARAM_BOUND,
            },
        )]);
        let target = [MAX_QUANTIZED_COORD; 6];
        let fixed = fixed_climate_coords(&target);
        let point = &table.entries[0].1;

        let a_i = point.fixed_fitness(&fixed);
        let dist4 = point.depth.distance(target[4]);
        let depth_term = dist4.checked_mul(dist4).expect("depth term fits i64");
        let total = a_i
            .checked_abs()
            .and_then(|abs_a| abs_a.checked_add(depth_term))
            .expect("|A_i| + depth_term fits i64");

        let exact_i128 = (5_i128 * 1_100_000_i128 * 1_100_000_i128)
            + (MAX_PARAM_BOUND as i128).pow(2)
            + (1_100_000_i128 * 1_100_000_i128);
        assert_eq!(total as i128, exact_i128);
        assert!(total <= DOCUMENTED_TOTAL_BOUND);
        assert!(total < i64::MAX / 1_000_000);

        let mut scratch = ColumnSearchScratch::default();
        let mut rows = Vec::new();
        table.find_column(fixed, &[target[4]], &mut scratch, &mut rows);
        assert_eq!(rows, vec![0]);
        assert_eq!(brute_force_row(&table, &target), Some(0));
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test --release -p rustmc-server --lib -- --ignored column_search_matches_find_and_brute_force_on_eight_chunks`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn column_search_matches_find_and_brute_force_on_eight_chunks() {
        use crate::vanilla::worldgen::WorldgenData;

        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let table = BiomePlacement::load(&root, "overworld").expect("overworld placement table");
        let data = WorldgenData::load(&root).expect("operator data loads");
        let engine = data.engine(2026);
        let registry = data.registry(&engine);
        let router = data
            .router(&registry, "minecraft:overworld")
            .expect("overworld router");
        let climate = ClimateSampler {
            temperature: router.temperature.clone(),
            vegetation: router.vegetation.clone(),
            continents: router.continents.clone(),
            erosion: router.erosion.clone(),
            depth: router.depth.clone(),
            ridges: router.ridges.clone(),
        };

        let quart_height = router.height / 4;
        let mut density_scratch = Vec::new();
        let mut targets = Vec::new();
        let mut col_scratch = ColumnSearchScratch::default();
        let mut depths = Vec::new();
        let mut rows = Vec::new();
        let mut resolved = Vec::new();
        let mut checked_cells = 0usize;
        let mut max_total_fitness = 0i64;

        for (cx, cz) in [
            (0, 0),
            (-3, 2),
            (17, -9),
            (40, 40),
            (-120, 7),
            (300, -300),
            (1, 1),
            (2, 2),
        ] {
            let volume = DensityVolume::new(
                [4, quart_height, 4],
                [cx * 16, router.min_y, cz * 16],
                [4, 4, 4],
            )
            .expect("quart volume");
            climate.targets_for_volume(&volume, &mut density_scratch, &mut targets);
            for lz in 0..4usize {
                for lx in 0..4usize {
                    let base = volume.index(lx, 0, lz);
                    let col = &targets[base..base + quart_height as usize];
                    resolve_column_by_runs(
                        &table,
                        col,
                        &mut col_scratch,
                        &mut depths,
                        &mut rows,
                        &mut resolved,
                    );
                    for (target, &row) in col.iter().zip(&resolved) {
                        assert_eq!(
                            Some(row),
                            table.find_row_for_test(target),
                            "chunk ({cx}, {cz}) cell ({lx}, {lz}) {target:?}"
                        );
                        assert_eq!(
                            Some(row),
                            brute_force_row(&table, target),
                            "chunk ({cx}, {cz}) cell ({lx}, {lz}) {target:?}"
                        );
                        assert_eq!(
                            table.entry_id(row),
                            table.find(target),
                            "chunk ({cx}, {cz}) cell ({lx}, {lz}) {target:?}"
                        );
                        checked_cells += 1;
                    }
                    let max_a = col_scratch
                        .fixed_costs
                        .iter()
                        .copied()
                        .map(i64::abs)
                        .max()
                        .unwrap_or(0);
                    let max_d = col
                        .iter()
                        .flat_map(|t| {
                            table
                                .entries
                                .iter()
                                .map(move |(_, p)| square(p.depth.distance(t[4])))
                        })
                        .max()
                        .unwrap_or(0);
                    max_total_fitness = max_total_fitness.max(max_a + max_d);
                }
            }
        }
        assert_eq!(checked_cells, 8 * 4 * 4 * quart_height as usize);
        assert!(
            max_total_fitness < i64::MAX / 1_000_000,
            "max |A_i| + depth term {max_total_fitness} too close to i64::MAX"
        );
        println!("verified {checked_cells} quart cells; max |A_i|+depth_term={max_total_fitness}");
    }

    impl Param {
        fn default_full() -> Self {
            Self {
                min: -10000,
                max: 10000,
            }
        }
    }
}
