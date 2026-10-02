//! The runtime aquifer adjustment layered over the density router.
//!
//! Vanilla 26.3 does not encode aquifers in the density graphs; the chunk
//! generator samples `final_density` per block and hands the value together
//! with the block coordinates to an aquifer runtime built from the
//! `noise_settings` `aquifers` section. That runtime decides whether a
//! density-empty position becomes solid stone (barrier pressure), water,
//! lava, or stays air, and any non-air block updates the stored heightmap.
//! Column heights therefore count the highest solid *or fluid* block, and
//! the aquifer can both raise terrain slightly above what density alone
//! indicates and move the water surface away from the dimension sea level.
//!
//! Numeric semantics were established by knowledge-only consultation under
//! ADR-0014 (as amended) and are recorded as facts in `docs/PROVENANCE.md`;
//! this implementation is written independently for RustMC's column query.
//! Where the reference used per-chunk arrays and a fluid-tick scheduling
//! flag, this module uses seed-pure grid lookups behind fixed-capacity
//! caches (a cell's center and fluid status are functions of its grid
//! coordinates and the world seed alone, so an evicted entry is just
//! recomputed) and omits the scheduling flag, which affects tick
//! post-processing but not the generated block substance a column query
//! observes.

use std::cell::RefCell;

use crate::vanilla::cache::BoundedCache;
use crate::vanilla::density::Density;
use crate::vanilla::random::PositionalRandomFactory;

/// The six compiled density graphs of a `noise_settings` `aquifers`
/// section. Field names follow the public datapack format.
#[derive(Clone)]
pub struct AquiferConfig {
    pub barrier: Density,
    pub fluid_level_floodedness: Density,
    pub fluid_level_spread: Density,
    pub lava: Density,
    pub exclusion: Density,
    pub surface_level: Density,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fluid {
    Water,
    Lava,
}

/// A fluid surface height with its fluid type; `fluid: None` fills
/// nothing. The deep `WAY_BELOW_MIN_Y` level is effectively "no fluid".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidStatus {
    pub level: i32,
    pub fluid: Option<Fluid>,
}

impl FluidStatus {
    /// The fluid occupying a block row: nothing at or above the surface.
    pub fn at(&self, block_y: i32) -> Option<Fluid> {
        if block_y < self.level {
            self.fluid
        } else {
            None
        }
    }
}

/// Dimension-wide fluid rule from the generator settings: lava below the
/// fixed deep-lava boundary, otherwise the configured global fluid at the
/// sea level. The deep-lava height is a documented 26.3 constant.
#[derive(Debug, Clone, Copy)]
pub struct GlobalFluid {
    pub sea_level: i32,
    pub sea_fluid: Fluid,
}

const DEEP_LAVA_LEVEL: i32 = -54;

impl GlobalFluid {
    fn pick(&self, block_y: i32) -> FluidStatus {
        if block_y < DEEP_LAVA_LEVEL.min(self.sea_level) {
            FluidStatus {
                level: DEEP_LAVA_LEVEL,
                fluid: Some(Fluid::Lava),
            }
        } else {
            FluidStatus {
                level: self.sea_level,
                fluid: Some(self.sea_fluid),
            }
        }
    }
}

/// What a block position holds after the aquifer adjustment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Substance {
    Solid,
    Fluid(Fluid),
    Air,
}

/// Occupancy of the aquifer's coordinate-keyed grid caches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AquiferOccupancy {
    /// Fluid cells with a memoised hashed center.
    pub centers: usize,
    /// Fluid cells with a memoised fluid status.
    pub statuses: usize,
    /// Quart-grid points with a memoised preliminary surface level.
    pub surface_levels: usize,
    /// Chunks with a memoised above-this-Y global-fluid shortcut.
    pub skip_bounds: usize,
}

fn substance_of(fluid: Option<Fluid>) -> Substance {
    match fluid {
        Some(f) => Substance::Fluid(f),
        None => Substance::Air,
    }
}

/// `DimensionType.WAY_BELOW_MIN_Y = MIN_Y << 4` with the 12-bit packed
/// coordinate bounds (`MIN_Y = -2032`); recorded in `docs/PROVENANCE.md`.
const WAY_BELOW_MIN_Y: i32 = -32512;

/// The aquifer runtime of a dimension.
pub enum Aquifer {
    /// No `aquifers` section: empty positions take the global fluid only.
    Disabled(GlobalFluid),
    NoiseBased(Box<NoiseBasedAquifer>),
}

impl Aquifer {
    /// The block substance at a position given its raw final density.
    pub fn substance(&self, x: i32, y: i32, z: i32, density: f32) -> Substance {
        match self {
            Self::Disabled(fluids) => {
                if density > 0.0 {
                    Substance::Solid
                } else {
                    substance_of(fluids.pick(y).at(y))
                }
            }
            Self::NoiseBased(aquifer) => aquifer.substance(x, y, z, density),
        }
    }
}

/// The noise-based aquifer: a hashed grid of fluid cells whose statuses
/// adjust density-empty positions through barrier pressure and per-cell
/// fluid levels.
pub struct NoiseBasedAquifer {
    config: AquiferConfig,
    fluids: GlobalFluid,
    /// Positional random factory fork for the aquifer cell grid, derived
    /// from the world seed before any cell is touched.
    factory: PositionalRandomFactory,
    centers: RefCell<BoundedCache<[i32; 3], [i32; 3]>>,
    statuses: RefCell<BoundedCache<[i32; 3], FluidStatus>>,
    surface_cache: RefCell<BoundedCache<[i32; 2], i32>>,
    skip_cache: RefCell<BoundedCache<[i32; 2], i32>>,
}

impl NoiseBasedAquifer {
    /// Fluid cells with a memoised hashed center. One chunk column spans
    /// 2x2 grid cells in x/z and one 12-block y cell per 12 rows, so the
    /// overworld's 384-row column touches 2x2x32 = 128 centers plus the
    /// 12-cell interpolation halo: about 140 per chunk. 1,024 keeps seven
    /// chunks of streaming resident (2,048 entries at 24 bytes plus
    /// control bytes is roughly 60 KiB) instead of one entry per cell of
    /// every chunk ever visited.
    pub const CENTER_CACHE_CAPACITY: usize = 1_024;
    /// Fluid cells with a memoised status. Same working set as the centers
    /// and the more expensive recompute (13 surface samples plus three
    /// density samples), so the same capacity.
    pub const STATUS_CACHE_CAPACITY: usize = 1_024;
    /// Quart-grid points with a memoised preliminary surface level. A
    /// chunk's `skip_sampling_above_y` scan covers a padded cell span of
    /// about 11x11 quart points, and one row of a radius-32 view is 65
    /// chunks wide, i.e. 65x16/4 + 16 = 276 quart columns by ~15 rows:
    /// 4,096 entries covers that row plus the 13-point ring of the cells
    /// inside it.
    pub const SURFACE_CACHE_CAPACITY: usize = 4_096;
    /// Chunks with a memoised aquifer sampling bound. Exactly one key per
    /// chunk and only ever read for positions inside that chunk, so two
    /// rows of a view is the real working set; 256 (2,048 entries, ~30 KiB
    /// including the hash table's control bytes) leaves plenty of slack.
    pub const SKIP_CACHE_CAPACITY: usize = 256;

    /// Cell spacing is 16 blocks in x/z (power of two) and 12 in y.
    const X_SPACING: i32 = 16;
    const Y_SPACING: i32 = 12;
    const Z_SPACING: i32 = 16;
    /// Random center offset ranges inside a cell: 10 in x/z, 9 in y.
    const X_RANGE: u32 = 10;
    const Y_RANGE: u32 = 9;
    const Z_RANGE: u32 = 10;
    /// Blocks sampled above the adjusted surface take the global fluid.
    const SURFACE_ADJUST: i32 = 8;

    /// The chunk ring (in 16-block units) whose preliminary surfaces a
    /// cell consults; the center sample is first and special-cased.
    const SURFACE_SAMPLING_OFFSETS: [[i32; 2]; 13] = [
        [0, 0],
        [-2, -1],
        [-1, -1],
        [0, -1],
        [1, -1],
        [-3, 0],
        [-2, 0],
        [-1, 0],
        [1, 0],
        [-2, 1],
        [-1, 1],
        [0, 1],
        [1, 1],
    ];

    pub fn new(
        config: AquiferConfig,
        fluids: GlobalFluid,
        factory: PositionalRandomFactory,
    ) -> Self {
        Self {
            config,
            fluids,
            factory,
            centers: RefCell::new(BoundedCache::new(Self::CENTER_CACHE_CAPACITY)),
            statuses: RefCell::new(BoundedCache::new(Self::STATUS_CACHE_CAPACITY)),
            surface_cache: RefCell::new(BoundedCache::new(Self::SURFACE_CACHE_CAPACITY)),
            skip_cache: RefCell::new(BoundedCache::new(Self::SKIP_CACHE_CAPACITY)),
        }
    }

    /// Entry counts of the four grid caches. Every key is a world grid
    /// coordinate, so each cache would grow with the volume of world
    /// queried; the capacities above bound them and this view lets the
    /// generator tests and the bench binary check the bound holds.
    pub fn cache_occupancy(&self) -> AquiferOccupancy {
        AquiferOccupancy {
            centers: self.centers.borrow().entries(),
            statuses: self.statuses.borrow().entries(),
            surface_levels: self.surface_cache.borrow().entries(),
            skip_bounds: self.skip_cache.borrow().entries(),
        }
    }

    fn grid_x(block_x: i32) -> i32 {
        // A 5-block sample offset before the 16-block cell shift.
        (block_x + -5) >> 4
    }

    fn grid_z(block_z: i32) -> i32 {
        (block_z + -5) >> 4
    }

    fn grid_y(block_y: i32) -> i32 {
        (block_y + 1).div_euclid(Self::Y_SPACING)
    }

    /// The hashed fluid-cell center of a grid cell (seed-pure function of
    /// its coordinates), cached.
    fn cell_center(&self, cell: [i32; 3]) -> [i32; 3] {
        if let Some(center) = self.centers.borrow_mut().get_mut(&cell) {
            return *center;
        }
        let mut random = self.factory.at(cell[0], cell[1], cell[2]);
        let center = [
            cell[0] * Self::X_SPACING + random.next_int_bounded(Self::X_RANGE) as i32,
            cell[1] * Self::Y_SPACING + random.next_int_bounded(Self::Y_RANGE) as i32,
            cell[2] * Self::Z_SPACING + random.next_int_bounded(Self::Z_RANGE) as i32,
        ];
        self.centers.borrow_mut().insert(cell, center);
        center
    }

    /// The fluid status of a grid cell, computed from its center once.
    fn cell_status(&self, cell: [i32; 3]) -> FluidStatus {
        if let Some(status) = self.statuses.borrow_mut().get_mut(&cell) {
            return *status;
        }
        let center = self.cell_center(cell);
        let status = self.compute_fluid(center[0], center[1], center[2]);
        self.statuses.borrow_mut().insert(cell, status);
        status
    }

    fn surface_level(&self, block_x: i32, block_z: i32) -> i32 {
        let key = [quantize_quart(block_x), quantize_quart(block_z)];
        if let Some(value) = self.surface_cache.borrow_mut().get_mut(&key) {
            return *value;
        }
        let value = self.config.surface_level.sample(key[0], 0, key[1]).floor() as i32;
        self.surface_cache.borrow_mut().insert(key, value);
        value
    }

    /// Highest adjusted preliminary surface over the padded cell grid of
    /// the chunk containing the block, converted to the Y above which the
    /// global fluid shortcut applies. Reproduces the documented per-chunk
    /// bound of the reference implementation.
    fn skip_sampling_above_y(&self, block_x: i32, block_z: i32) -> i32 {
        let chunk = [block_x >> 4, block_z >> 4];
        if let Some(value) = self.skip_cache.borrow_mut().get_mut(&chunk) {
            return *value;
        }
        let min_block_x = chunk[0] * 16;
        let max_block_x = min_block_x + 15;
        let min_block_z = chunk[1] * 16;
        let max_block_z = min_block_z + 15;
        let min_grid_x = Self::grid_x(min_block_x);
        let max_grid_x = Self::grid_x(max_block_x) + 1;
        let min_grid_z = Self::grid_z(min_block_z);
        let max_grid_z = Self::grid_z(max_block_z) + 1;
        let max_surface = self.max_surface_level(
            min_grid_x * Self::X_SPACING,
            max_grid_x * Self::X_SPACING + (Self::X_RANGE as i32 - 1),
            min_grid_z * Self::Z_SPACING,
            max_grid_z * Self::Z_SPACING + (Self::Z_RANGE as i32 - 1),
        );
        let max_adjusted = max_surface + Self::SURFACE_ADJUST;
        let skip_grid_y = (max_adjusted + 12).div_euclid(Self::Y_SPACING) - -1;
        let value = skip_grid_y * Self::Y_SPACING + 11 - 1;
        self.skip_cache.borrow_mut().insert(chunk, value);
        value
    }

    /// Maximum preliminary surface over the quart-quantized grid span.
    fn max_surface_level(
        &self,
        min_block_x: i32,
        max_block_x: i32,
        min_block_z: i32,
        max_block_z: i32,
    ) -> i32 {
        let mut max = i32::MIN;
        for quart_z in (min_block_z >> 2)..=(max_block_z >> 2) {
            for quart_x in (min_block_x >> 2)..=(max_block_x >> 2) {
                let level = self.surface_level(quart_x << 2, quart_z << 2);
                if level > max {
                    max = level;
                }
            }
        }
        max
    }

    /// The block substance at a position given its raw final density.
    pub fn substance(&self, x: i32, y: i32, z: i32, density: f32) -> Substance {
        if density > 0.0 {
            return Substance::Solid;
        }
        let global = self.fluids.pick(y);
        if y > self.skip_sampling_above_y(x, z) {
            return substance_of(global.at(y));
        }
        if global.at(y) == Some(Fluid::Lava) {
            return Substance::Fluid(Fluid::Lava);
        }

        let anchor = [Self::grid_x(x), Self::grid_y(y), Self::grid_z(z)];
        // The 12 surrounding cells, keeping the four nearest centers in
        // arrival order with displacement on ties.
        let mut nearest: [([i32; 3], i32); 4] = [
            ([0, 0, 0], i32::MAX),
            ([0, 0, 0], i32::MAX),
            ([0, 0, 0], i32::MAX),
            ([0, 0, 0], i32::MAX),
        ];
        for dx in 0..=1 {
            for dy in -1..=1 {
                for dz in 0..=1 {
                    let cell = [anchor[0] + dx, anchor[1] + dy, anchor[2] + dz];
                    let center = self.cell_center(cell);
                    let dx = center[0] - x;
                    let dy = center[1] - y;
                    let dz = center[2] - z;
                    let distance = dx * dx + dy * dy + dz * dz;
                    insert_nearest(&mut nearest, cell, distance);
                }
            }
        }

        let (cell_one, distance_one) = nearest[0];
        let (cell_two, distance_two) = nearest[1];
        let (cell_three, distance_three) = nearest[2];
        let status_one = self.cell_status(cell_one);
        let similarity_one_two = similarity(distance_one, distance_two);
        let fluid_state = status_one.at(y);
        if similarity_one_two <= 0.0 {
            return substance_of(fluid_state);
        }
        // Water directly above the deep-lava boundary stays water.
        if fluid_state == Some(Fluid::Water)
            && self.fluids.pick(y - 1).at(y - 1) == Some(Fluid::Lava)
        {
            return Substance::Fluid(Fluid::Water);
        }

        let density = f64::from(density);
        let mut barrier_noise = f64::NAN;
        let status_two = self.cell_status(cell_two);
        let barrier_one_two = similarity_one_two
            * self.pressure(x, y, z, &mut barrier_noise, &status_one, &status_two);
        if density + barrier_one_two > 0.0 {
            return Substance::Solid;
        }
        let status_three = self.cell_status(cell_three);
        let similarity_one_three = similarity(distance_one, distance_three);
        if similarity_one_three > 0.0 {
            let barrier_one_three = similarity_one_two
                * similarity_one_three
                * self.pressure(x, y, z, &mut barrier_noise, &status_one, &status_three);
            if density + barrier_one_three > 0.0 {
                return Substance::Solid;
            }
        }
        let similarity_two_three = similarity(distance_two, distance_three);
        if similarity_two_three > 0.0 {
            let barrier_two_three = similarity_one_two
                * similarity_two_three
                * self.pressure(x, y, z, &mut barrier_noise, &status_two, &status_three);
            if density + barrier_two_three > 0.0 {
                return Substance::Solid;
            }
        }
        substance_of(fluid_state)
    }

    /// Barrier pressure between two cell statuses at a block row. The
    /// barrier noise is sampled at block resolution at most once per
    /// position; the caller threads the per-position cache through.
    fn pressure(
        &self,
        x: i32,
        y: i32,
        z: i32,
        barrier_noise: &mut f64,
        first: &FluidStatus,
        second: &FluidStatus,
    ) -> f64 {
        let first_at = first.at(y);
        let second_at = second.at(y);
        let mixed = matches!(
            (first_at, second_at),
            (Some(Fluid::Lava), Some(Fluid::Water)) | (Some(Fluid::Water), Some(Fluid::Lava))
        );
        if mixed {
            return 2.0;
        }
        let level_difference = (first.level - second.level).abs();
        if level_difference == 0 {
            return 0.0;
        }
        let average_level = 0.5 * (first.level + second.level) as f64;
        let above_average = f64::from(y) + 0.5 - average_level;
        let edge_distance = (level_difference as f64) / 2.0 - above_average.abs();
        // Above the mean fluid level the barrier crest starts at the edge
        // itself; below it, a 3-block pocket widens the crest downwards.
        let gradient = if above_average > 0.0 {
            if edge_distance > 0.0 {
                edge_distance / 1.5
            } else {
                edge_distance / 2.5
            }
        } else {
            let pocket = 3.0 + edge_distance;
            if pocket > 0.0 {
                pocket / 3.0
            } else {
                pocket / 10.0
            }
        };
        let noise = if (-2.0..=2.0).contains(&gradient) {
            if barrier_noise.is_nan() {
                *barrier_noise = f64::from(self.config.barrier.sample(x, y, z));
            }
            *barrier_noise
        } else {
            0.0
        };
        2.0 * (noise + gradient)
    }

    /// The fluid status of the cell centered at a block position.
    fn compute_fluid(&self, x: i32, y: i32, z: i32) -> FluidStatus {
        let global = self.fluids.pick(y);
        let cell_top = y + Self::Y_SPACING;
        let cell_bottom = y - Self::Y_SPACING;
        let mut lowest_preliminary = i32::MAX;
        let mut center_under_fluid = false;
        for offset in Self::SURFACE_SAMPLING_OFFSETS {
            let sample_x = x + offset[0] * 16;
            let sample_z = z + offset[1] * 16;
            let preliminary = self.surface_level(sample_x, sample_z);
            let adjusted = preliminary + Self::SURFACE_ADJUST;
            let is_center = offset[0] == 0 && offset[1] == 0;
            if is_center && cell_bottom > adjusted {
                return global;
            }
            let pokes_above_surface = cell_top > adjusted;
            if pokes_above_surface || is_center {
                let at_surface = self.fluids.pick(adjusted);
                if at_surface.at(adjusted).is_some() {
                    if is_center {
                        center_under_fluid = true;
                    }
                    if pokes_above_surface {
                        return at_surface;
                    }
                }
            }
            if preliminary < lowest_preliminary {
                lowest_preliminary = preliminary;
            }
        }
        let level = self.compute_fluid_surface_level(
            x,
            y,
            z,
            global.level,
            lowest_preliminary,
            center_under_fluid,
        );
        let fluid = self.compute_fluid_type(x, y, z, global.fluid, level);
        FluidStatus { level, fluid }
    }

    fn compute_fluid_surface_level(
        &self,
        x: i32,
        y: i32,
        z: i32,
        global_level: i32,
        lowest_preliminary: i32,
        center_under_fluid: bool,
    ) -> i32 {
        let (partially_flooded, fully_flooded) = if self.config.exclusion.sample(x, y, z) > 0.0 {
            (-1.0, -1.0)
        } else {
            let distance_below_surface = (lowest_preliminary + Self::SURFACE_ADJUST) - y;
            let floodedness_factor = if center_under_fluid {
                clamped_map(f64::from(distance_below_surface), 0.0, 64.0, 1.0, 0.0)
            } else {
                0.0
            };
            let floodedness = clamp(
                f64::from(self.config.fluid_level_floodedness.sample(x, y, z)),
                -1.0,
                1.0,
            );
            (
                floodedness - map(floodedness_factor, 1.0, 0.0, -0.8, 0.4),
                floodedness - map(floodedness_factor, 1.0, 0.0, -0.3, 0.8),
            )
        };
        if fully_flooded > 0.0 {
            global_level
        } else if partially_flooded > 0.0 {
            self.randomized_surface_level(x, y, z, lowest_preliminary)
        } else {
            WAY_BELOW_MIN_Y
        }
    }

    /// A partially flooded cell drains to a hashed local surface: the
    /// middle of its 40-block fluid column, spread by noise up to ±10 and
    /// quantized to 3, never above the lowest sampled surface.
    fn randomized_surface_level(&self, x: i32, y: i32, z: i32, lowest_preliminary: i32) -> i32 {
        let cell_x = x.div_euclid(16);
        let cell_y = y.div_euclid(40);
        let cell_z = z.div_euclid(16);
        let column_middle = cell_y * 40 + 20;
        let spread = f64::from(
            self.config
                .fluid_level_spread
                .sample(cell_x, cell_y, cell_z)
                * 10.0f32,
        );
        let quantized = spread / 3.0;
        let quantized = quantized.floor() as i32 * 3;
        lowest_preliminary.min(column_middle + quantized)
    }

    fn compute_fluid_type(
        &self,
        x: i32,
        y: i32,
        z: i32,
        global_fluid: Option<Fluid>,
        level: i32,
    ) -> Option<Fluid> {
        let mut fluid = global_fluid;
        if level <= -10
            && level != WAY_BELOW_MIN_Y
            && global_fluid != Some(Fluid::Lava)
            && f64::from(self.config.lava.sample(
                x.div_euclid(64),
                y.div_euclid(40),
                z.div_euclid(64),
            ))
            .abs()
                > 0.3
        {
            fluid = Some(Fluid::Lava);
        }
        fluid
    }
}

/// Keeps the four nearest cells, displacing on ties like the reference.
fn insert_nearest(nearest: &mut [([i32; 3], i32); 4], cell: [i32; 3], distance: i32) {
    if nearest[0].1 >= distance {
        nearest[3] = nearest[2];
        nearest[2] = nearest[1];
        nearest[1] = nearest[0];
        nearest[0] = (cell, distance);
    } else if nearest[1].1 >= distance {
        nearest[3] = nearest[2];
        nearest[2] = nearest[1];
        nearest[1] = (cell, distance);
    } else if nearest[2].1 >= distance {
        nearest[3] = nearest[2];
        nearest[2] = (cell, distance);
    } else if nearest[3].1 >= distance {
        nearest[3] = (cell, distance);
    }
}

/// How much two cell distances still count as one neighborhood: the
/// farther one may exceed the nearest by at most 25 squared blocks.
fn similarity(nearest: i32, other: i32) -> f64 {
    1.0 - f64::from(other - nearest) / 25.0
}

/// Quart-grid quantization (`>> 2` then `<< 2`) for surface sampling.
fn quantize_quart(block: i32) -> i32 {
    (block >> 2) << 2
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

fn map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    to_min + (value - from_min) / (from_max - from_min) * (to_max - to_min)
}

fn clamped_map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    let factor = (value - from_min) / (from_max - from_min);
    if factor < 0.0 {
        to_min
    } else if factor > 1.0 {
        to_max
    } else {
        to_min + (to_max - to_min) * factor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_fluid_bounds_sea_and_lava() {
        let fluids = GlobalFluid {
            sea_level: 63,
            sea_fluid: Fluid::Water,
        };
        assert_eq!(fluids.pick(100).at(100), None);
        assert_eq!(fluids.pick(63).at(63), None);
        assert_eq!(fluids.pick(62).at(62), Some(Fluid::Water));
        assert_eq!(fluids.pick(-54).at(-54), Some(Fluid::Water));
        assert_eq!(fluids.pick(-55).at(-55), Some(Fluid::Lava));
    }

    #[test]
    fn similarity_fades_over_25_squared_blocks() {
        assert_eq!(similarity(10, 10), 1.0);
        assert_eq!(similarity(10, 35), 0.0);
        assert!(similarity(10, 40) < 0.0);
    }

    #[test]
    fn nearest_four_keep_ties_by_displacement() {
        let mut nearest = [([0, 0, 0], i32::MAX); 4];
        for (cell, distance) in [
            ([1, 1, 1], 30),
            ([2, 2, 2], 10),
            ([3, 3, 3], 10),
            ([4, 4, 4], 20),
            ([5, 5, 5], 40),
        ] {
            insert_nearest(&mut nearest, cell, distance);
        }
        // Equal distances displace earlier entries into the next slot.
        assert_eq!(nearest[0].0, [3, 3, 3]);
        assert_eq!(nearest[1].0, [2, 2, 2]);
        assert_eq!(nearest[2].0, [4, 4, 4]);
        assert_eq!(nearest[3].0, [1, 1, 1]);
    }

    #[test]
    fn helper_maps_match_the_documented_shapes() {
        assert_eq!(clamped_map(0.0, 0.0, 64.0, 1.0, 0.0), 1.0);
        assert_eq!(clamped_map(32.0, 0.0, 64.0, 1.0, 0.0), 0.5);
        assert_eq!(clamped_map(64.0, 0.0, 64.0, 1.0, 0.0), 0.0);
        assert_eq!(clamped_map(-10.0, 0.0, 64.0, 1.0, 0.0), 1.0);
        assert_eq!(clamped_map(100.0, 0.0, 64.0, 1.0, 0.0), 0.0);
        assert_eq!(map(1.0, 1.0, 0.0, -0.3, 0.8), -0.3);
        assert_eq!(map(0.0, 1.0, 0.0, -0.3, 0.8), 0.8);
        assert_eq!(clamp(-2.0, -1.0, 1.0), -1.0);
        assert_eq!(clamp(2.0, -1.0, 1.0), 1.0);
    }

    #[test]
    fn quantization_snaps_to_the_quart_grid() {
        assert_eq!(quantize_quart(7), 4);
        assert_eq!(quantize_quart(-1), -4);
        assert_eq!(quantize_quart(-4), -4);
        assert_eq!(quantize_quart(4), 4);
    }
}
