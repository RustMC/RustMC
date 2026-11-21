//! Vanilla ground truth extraction and match-rate reporting for the T0
//! oracle slice (docs/research/vanilla-worldgen-feasibility.md). Reads only
//! chunk data the owner's own licensed client generated; compares against
//! RustMC's current generator and reports exact-match percentages.

use rustmc_server::vanilla::aquifer::Substance;
use rustmc_server::vanilla::generator::VanillaGenerator;
use rustmc_server::world::Generator;

use crate::nbt::Tag;
use crate::region::RegionStore;

#[derive(Debug, Clone, PartialEq)]
pub struct VanillaColumn {
    pub x: i64,
    pub z: i64,
    /// Absolute Y of the top terrain block (heightmap value − 1 + world min Y).
    pub surface_y: i32,
    pub top_block: Option<String>,
    pub biome: Option<String>,
}

/// One sampled comparison: vanilla truth vs the RustMC generator.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnVerdict {
    pub x: i64,
    pub z: i64,
    pub vanilla_surface_y: i32,
    pub vanilla_biome: Option<String>,
    pub rustmc_height: i64,
    pub rustmc_biome: String,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct MatchReport {
    pub columns: usize,
    pub height_matches: usize,
    pub biome_matches: usize,
    /// Top-block matches restricted to height-matched columns (the
    /// documented T2 gate population).
    pub topblock_matches: usize,
    /// Residual top-block disagreements as `(vanilla, rustmc)` base-id
    /// pair counts, for attributing the gap without dumping columns.
    pub topblock_residuals: std::collections::BTreeMap<(String, String), usize>,
    pub mismatches: Vec<ColumnVerdict>,
}

/// Coarse filler class of a block position, the T3 comparison unit:
/// caves and terrain agree or not at the substance level, before block
/// identities (T2 surface rules, T4 features) are considered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Air,
    Fluid,
    Solid,
}

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Self::Air => "air",
            Self::Fluid => "fluid",
            Self::Solid => "solid",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Air => 0,
            Self::Fluid => 1,
            Self::Solid => 2,
        }
    }
}

/// Classify a saved block state id. The air family and the two fluids
/// are named by the public block list; everything else is solid.
pub fn block_category(name: &str) -> Category {
    match base_block_name(name) {
        "minecraft:air"
        | "minecraft:cave_air"
        | "minecraft:void_air"
        | "minecraft:structure_void" => Category::Air,
        "minecraft:water" | "minecraft:lava" => Category::Fluid,
        _ => Category::Solid,
    }
}

/// Any source of per-column RustMC answers: the current generator or the
/// data-driven vanilla density pipeline.
pub trait ColumnSource {
    fn column_height(&self, x: i64, z: i64) -> i64;
    fn column_biome(&self, x: i64, z: i64) -> String;
    /// Base block id at the column surface. Sources without surface rules
    /// answer with the pending marker so the metric is never inflated.
    fn column_top_block(&self, _x: i64, _z: i64) -> String {
        "<pending-T2>".to_owned()
    }
    /// Filler substance at one absolute position. Sources without a 3D
    /// answer return `None`, keeping the T3 metric honest.
    fn column_substance(&self, _x: i64, _z: i64, _y: i32) -> Option<Category> {
        None
    }
}

/// The block id without its property suffix (`minecraft:k[v=1]` ->
/// `minecraft:k`); surface rules and result states are recorded by base
/// id while saved palettes may carry properties.
pub fn base_block_name(name: &str) -> &str {
    name.split_once('[').map_or(name, |(base, _)| base)
}

impl ColumnSource for Generator {
    fn column_height(&self, x: i64, z: i64) -> i64 {
        self.height(x, z)
    }
    fn column_biome(&self, x: i64, z: i64) -> String {
        self.biome(x, z).identifier().to_string()
    }
}

impl ColumnSource for VanillaGenerator {
    fn column_height(&self, x: i64, z: i64) -> i64 {
        let (Ok(x), Ok(z)) = (i32::try_from(x), i32::try_from(z)) else {
            return i64::MIN;
        };
        i64::from(self.surface_height(x, z))
    }
    fn column_biome(&self, x: i64, z: i64) -> String {
        let (Ok(x), Ok(z)) = (i32::try_from(x), i32::try_from(z)) else {
            return "<unknown>".to_owned();
        };
        let surface_y = self.surface_height(x, z);
        self.biome(x, z, surface_y)
            .unwrap_or_else(|| "<unknown>".to_owned())
    }
    fn column_top_block(&self, x: i64, z: i64) -> String {
        let (Ok(x), Ok(z)) = (i32::try_from(x), i32::try_from(z)) else {
            return "<unknown>".to_owned();
        };
        self.top_block(x, z)
            .unwrap_or_else(|| "<unknown>".to_owned())
    }
    fn column_substance(&self, x: i64, z: i64, y: i32) -> Option<Category> {
        let (Ok(x), Ok(z)) = (i32::try_from(x), i32::try_from(z)) else {
            return None;
        };
        Some(match self.substance(x, y, z) {
            Substance::Solid => Category::Solid,
            Substance::Fluid(_) => Category::Fluid,
            Substance::Air => Category::Air,
        })
    }
}

pub fn compare_columns(
    columns: impl IntoIterator<Item = VanillaColumn>,
    source: &dyn ColumnSource,
    mismatch_cap: usize,
) -> MatchReport {
    let mut report = MatchReport::default();
    for column in columns {
        let rustmc_height = source.column_height(column.x, column.z);
        let rustmc_biome = source.column_biome(column.x, column.z);
        report.columns += 1;
        let height_ok = i64::from(column.surface_y) == rustmc_height;
        let biome_ok = column.biome.as_deref() == Some(rustmc_biome.as_str());
        report.height_matches += usize::from(height_ok);
        report.biome_matches += usize::from(biome_ok);
        if height_ok && let Some(vanilla_top) = column.top_block.as_deref() {
            let rustmc_top = source.column_top_block(column.x, column.z);
            let vanilla_base = base_block_name(vanilla_top).to_owned();
            let rustmc_base = base_block_name(&rustmc_top).to_owned();
            let matched = vanilla_base == rustmc_base;
            report.topblock_matches += usize::from(matched);
            if !matched {
                *report
                    .topblock_residuals
                    .entry((vanilla_base, rustmc_base))
                    .or_default() += 1;
            }
        }
        // The capped detail list tracks height gaps only; biome misses are
        // tier T2 work and would otherwise drown the diagnostic (the
        // aggregate biome count above still reports them).
        if !height_ok && report.mismatches.len() < mismatch_cap {
            report.mismatches.push(ColumnVerdict {
                x: column.x,
                z: column.z,
                vanilla_surface_y: column.surface_y,
                vanilla_biome: column.biome.clone(),
                rustmc_height,
                rustmc_biome,
            });
        }
    }
    report
}

/// Category volumes of one 32-block absolute Y band over the compared
/// population, in `[air, fluid, solid]` order.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BandStats {
    pub vanilla: [usize; 3],
    pub rustmc: [usize; 3],
}

/// Per-position substance agreement over sampled column profiles (the
/// T3 measurement). Bands split each column by depth below its vanilla
/// surface: the near-surface crust, the middle cave belt, and the deep
/// interior below 64 blocks of cover.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SubstanceReport {
    pub columns: usize,
    pub positions: usize,
    pub matches: usize,
    pub near_positions: usize,
    pub near_matches: usize,
    pub middle_positions: usize,
    pub middle_matches: usize,
    pub deep_positions: usize,
    pub deep_matches: usize,
    /// Disagreements as `(vanilla, rustmc)` category pair counts.
    pub residuals: std::collections::BTreeMap<(Category, Category), usize>,
    /// Disagreements per 32-block absolute Y band, so a residual can be
    /// attributed to a carver height range without dumping columns.
    pub residual_bands: std::collections::BTreeMap<((Category, Category), i32), usize>,
    /// Marginal category volumes per 32-block absolute Y band.
    pub marginals: std::collections::BTreeMap<i32, BandStats>,
    /// First capped disagreement positions for follow-up single-column
    /// probes (`column` mode).
    pub fail_positions: Vec<(i64, i64, i32)>,
}

pub fn compare_substance(
    profiles: impl IntoIterator<Item = ColumnProfile>,
    source: &dyn ColumnSource,
    fail_cap: usize,
) -> SubstanceReport {
    let mut report = SubstanceReport::default();
    for profile in profiles {
        report.columns += 1;
        for (offset, vanilla) in profile.categories.iter().copied().enumerate() {
            let y = profile.min_y + offset as i32;
            let Some(rustmc) = source.column_substance(profile.x, profile.z, y) else {
                continue;
            };
            report.positions += 1;
            let stats = report.marginals.entry(y.div_euclid(32) * 32).or_default();
            stats.vanilla[vanilla.index()] += 1;
            stats.rustmc[rustmc.index()] += 1;
            let matched = vanilla == rustmc;
            report.matches += usize::from(matched);
            if !matched {
                *report.residuals.entry((vanilla, rustmc)).or_default() += 1;
                *report
                    .residual_bands
                    .entry(((vanilla, rustmc), y.div_euclid(32) * 32))
                    .or_default() += 1;
                if report.fail_positions.len() < fail_cap {
                    report.fail_positions.push((profile.x, profile.z, y));
                }
            }
            let depth = profile.surface_y - y;
            let (positions, matches) = if depth < 8 {
                (&mut report.near_positions, &mut report.near_matches)
            } else if depth < 64 {
                (&mut report.middle_positions, &mut report.middle_matches)
            } else {
                (&mut report.deep_positions, &mut report.deep_matches)
            };
            *positions += 1;
            *matches += usize::from(matched);
        }
    }
    report
}

/// Numerator and denominator for one subgroup of a sampled population.
/// `matches` counts strict base ids, `matches_air_family` the same rows
/// with the air family collapsed to one name, so a subgroup dominated by
/// air bookkeeping is visible instead of depressing the headline score.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Agreement {
    pub positions: usize,
    pub matches: usize,
    pub matches_air_family: usize,
}

impl Agreement {
    fn add(&mut self, matched: bool, matched_air_family: bool) {
        self.positions += 1;
        self.matches += usize::from(matched);
        self.matches_air_family += usize::from(matched_air_family);
    }
}

mod blocks;
pub use blocks::*;
mod save_read;
pub use save_read::*;
mod census;
pub use census::*;

#[cfg(test)]
mod tests;
