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

/// Vertical distance in stored rows at which a block is still considered
/// cave wall rather than intact rock.
pub const CAVE_WALL_ROWS: u32 = 4;

/// Where a sampled row sits relative to the cave voids the save itself
/// stores, used to attribute solid-vs-air disagreement to the carving
/// pass instead of the density graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CaveContext {
    /// The save stores an air-family block below its heightmap surface:
    /// a vanilla cave void (or a span the chunk never wrote).
    Void,
    /// The save stores a block within [`CAVE_WALL_ROWS`] rows of a void:
    /// cave floor, ceiling, or wall.
    CaveWall,
    /// The save stores a block further from any void in the column.
    IntactRock,
}

impl CaveContext {
    pub fn name(self) -> &'static str {
        match self {
            Self::Void => "void",
            Self::CaveWall => "cave_wall",
            Self::IntactRock => "intact_rock",
        }
    }
}

/// Any source of a full per-column block descent: index 0 of the answer
/// is the dimension's minimum build Y, `None` entries are air. Sources
/// without a material-rule descent answer `None` for every row, which the
/// report counts as a declined column instead of scoring as air.
pub trait ColumnDescent {
    fn column_block_ids(&self, x: i32, z: i32) -> Vec<Option<String>>;
    fn column_min_y(&self) -> i32;
}

impl ColumnDescent for VanillaGenerator {
    fn column_block_ids(&self, x: i32, z: i32) -> Vec<Option<String>> {
        self.column_ids(x, z)
    }
    fn column_min_y(&self) -> i32 {
        self.min_y()
    }
}

/// Exact base-block agreement for a sampled generated column volume, the
/// M3 ground-truth measurement. This deliberately scores feature and
/// structure blocks too: a high substance score alone cannot establish a
/// vanilla-looking or editable world. Two scores are reported: strict base
/// ids, and ids with the air family (`air`, `cave_air`, `void_air`,
/// `structure_void`) collapsed to one name, which separates identity gaps
/// from air bookkeeping.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BlockReport {
    /// Columns contributing at least one scored position.
    pub columns: usize,
    /// Sampled columns whose absolute coordinates leave the generator's
    /// `i32` domain: reported, never silently dropped.
    pub out_of_domain_columns: usize,
    /// Sampled columns where the generator answered no block at all.
    pub declined_columns: usize,
    /// Stored rows outside the generator's build range: not scored.
    pub out_of_range_rows: usize,
    pub positions: usize,
    pub exact_base: usize,
    pub exact_base_air_family: usize,
    /// Agreement per 32-block absolute Y band, the generator-side
    /// counterpart of the save census banding.
    pub bands: std::collections::BTreeMap<i32, Agreement>,
    /// Agreement per cave context of the saved column.
    pub cave: std::collections::BTreeMap<CaveContext, Agreement>,
    /// Rows the save leaves air where we keep a block: carving the density
    /// graph does not open the cave.
    pub missed_carves: usize,
    /// `missed_carves` per 32-block absolute Y band: where the carvers are
    /// still missing.
    pub missed_carves_bands: std::collections::BTreeMap<i32, usize>,
    /// Rows the save fills with a block where we answer air.
    pub extra_carves: usize,
    /// `extra_carves` per 32-block absolute Y band: where we carve too much.
    pub extra_carves_bands: std::collections::BTreeMap<i32, usize>,
    /// `extra_carves` within [`CAVE_WALL_ROWS`] of a saved void: over-wide
    /// carving of an existing cave. The remainder are invented caves in
    /// rock the save leaves solid.
    pub extra_carves_near_void: usize,
    /// Confusion pairs as `(save, rustmc)` base-id counts.
    pub residuals: std::collections::BTreeMap<(String, String), usize>,
    /// The same pairs after collapsing the air family, dropping the pairs
    /// that only differ by which air block was written.
    pub residuals_air_family: std::collections::BTreeMap<(String, String), usize>,
    /// The same pairs restricted to saved voids and cave walls: how the
    /// carving neighbourhood disagrees, block by block.
    pub residuals_cave: std::collections::BTreeMap<(String, String), usize>,
    /// First capped disagreement positions for follow-up single-column
    /// probes (`column` mode).
    pub fail_positions: Vec<(i64, i64, i32)>,
}

/// Collapse the air family to one name so residual reporting can separate
/// real identity differences from air bookkeeping. Expects a base id.
pub fn air_family_name(name: &str) -> &str {
    if block_category(name) == Category::Air {
        "minecraft:air"
    } else {
        name
    }
}

/// Distance in rows from each position to the nearest `true` entry,
/// `u32::MAX` when the column has none.
fn void_distances(flags: &[bool]) -> Vec<u32> {
    let mut distances = vec![u32::MAX; flags.len()];
    let mut previous: Option<usize> = None;
    for (index, flag) in flags.iter().enumerate() {
        if *flag {
            previous = Some(index);
        }
        let Some(nearest) = previous else { continue };
        let Ok(distance) = u32::try_from(index - nearest) else {
            continue;
        };
        distances[index] = distance;
    }
    let mut next: Option<usize> = None;
    for index in (0..flags.len()).rev() {
        if flags[index] {
            next = Some(index);
        }
        let Some(nearest) = next else { continue };
        let Ok(distance) = u32::try_from(nearest - index) else {
            continue;
        };
        if distance < distances[index] {
            distances[index] = distance;
        }
    }
    distances
}

/// Score one generator's full-column descent against sampled saved
/// columns. Rows are matched by absolute Y; a saved row outside the
/// generator's build range is counted in `out_of_range_rows` and never
/// scored, and `fail_cap` bounds the follow-up position dump.
pub fn compare_block_ids(
    profiles: impl IntoIterator<Item = ColumnProfile>,
    source: &dyn ColumnDescent,
    fail_cap: usize,
) -> BlockReport {
    let mut report = BlockReport::default();
    let generated_min_y = source.column_min_y();
    for profile in profiles {
        let (Ok(x), Ok(z)) = (i32::try_from(profile.x), i32::try_from(profile.z)) else {
            report.out_of_domain_columns += 1;
            continue;
        };
        let generated = source.column_block_ids(x, z);
        if generated.iter().all(Option::is_none) {
            report.declined_columns += 1;
            continue;
        }
        report.columns += 1;
        let voids: Vec<bool> = profile
            .block_ids
            .iter()
            .map(|name| block_category(name) == Category::Air)
            .collect();
        let nearest_void = void_distances(&voids);
        for (offset, (saved, is_void)) in profile.block_ids.iter().zip(&voids).enumerate() {
            let y = profile.min_y + offset as i32;
            let Some(index) = usize::try_from(y - generated_min_y).ok() else {
                report.out_of_range_rows += 1;
                continue;
            };
            let ours: Option<&str> = match generated.get(index) {
                None => {
                    report.out_of_range_rows += 1;
                    continue;
                }
                Some(None) => None,
                Some(Some(name)) => Some(name.as_str()),
            };
            let saved = base_block_name(saved);
            let ours = ours.map_or("minecraft:air", base_block_name);
            let saved_family = air_family_name(saved);
            let ours_family = air_family_name(ours);
            let distance = nearest_void[offset];
            let matched = saved == ours;
            let matched_family = saved_family == ours_family;
            report.positions += 1;
            report.exact_base += usize::from(matched);
            report.exact_base_air_family += usize::from(matched_family);
            let band = y.div_euclid(32) * 32;
            report
                .bands
                .entry(band)
                .or_default()
                .add(matched, matched_family);
            let context = if *is_void {
                CaveContext::Void
            } else if distance <= CAVE_WALL_ROWS {
                CaveContext::CaveWall
            } else {
                CaveContext::IntactRock
            };
            report
                .cave
                .entry(context)
                .or_default()
                .add(matched, matched_family);
            let ours_is_air = block_category(ours) == Category::Air;
            if *is_void && !ours_is_air {
                report.missed_carves += 1;
                *report.missed_carves_bands.entry(band).or_default() += 1;
            } else if !*is_void && ours_is_air {
                report.extra_carves += 1;
                *report.extra_carves_bands.entry(band).or_default() += 1;
                if distance <= CAVE_WALL_ROWS {
                    report.extra_carves_near_void += 1;
                }
            }
            if !matched {
                *report
                    .residuals
                    .entry((saved.to_owned(), ours.to_owned()))
                    .or_default() += 1;
                if context != CaveContext::IntactRock {
                    *report
                        .residuals_cave
                        .entry((saved.to_owned(), ours.to_owned()))
                        .or_default() += 1;
                }
                if !matched_family {
                    *report
                        .residuals_air_family
                        .entry((saved_family.to_owned(), ours_family.to_owned()))
                        .or_default() += 1;
                }
                if report.fail_positions.len() < fail_cap {
                    report.fail_positions.push((profile.x, profile.z, y));
                }
            }
        }
    }
    report
}

pub fn percent(matches: usize, columns: usize) -> f64 {
    if columns == 0 {
        0.0
    } else {
        100.0 * matches as f64 / columns as f64
    }
}

/// Absolute Y of the top terrain block from the stored heightmaps, or
/// `Ok(None)` when the column is not filled yet.
///
/// 26.3 saves (DataVersion 5023, observed 30 September 2026 in the owner's
/// world) store heightmaps relative to the world minimum Y, so the absolute
/// top block is `value - 1 + yPos * 16`. `MOTION_BLOCKING_NO_LEAVES` is the
/// closest stored analogue of a hand-cleared F3 ground reading.
///
/// A stored chunk with no heightmaps at all is a chunk the client wrote
/// before the height pass, not a decode failure: it answers `Ok(None)` so
/// samplers can report it as ungenerated instead of losing the run.
fn surface_top(root: &Tag, lx: u8, lz: u8) -> Result<Option<i32>, String> {
    let heightmaps = root.get("Heightmaps").or_else(|| root.get("heightmaps"));
    let Some(packed) = heightmaps
        .and_then(|h| {
            h.get("MOTION_BLOCKING_NO_LEAVES")
                .or_else(|| h.get("WORLD_SURFACE"))
                .or_else(|| h.get("MOTION_BLOCKING"))
        })
        .and_then(Tag::as_long_array)
    else {
        return Ok(None);
    };
    let min_y = root.get("yPos").and_then(Tag::as_i32).unwrap_or(-4) * 16;
    let heights = unpack_spanning(packed, 9, 256);
    let index = usize::from(lx) + usize::from(lz) * 16;
    Ok(match heights.get(index).copied() {
        Some(h) if h > 0 => Some(h as i32 - 1 + min_y),
        _ => None,
    })
}

/// Extract ground truth for one column of one stored chunk.
pub fn column_truth(
    root: &Tag,
    chunk_x: i32,
    chunk_z: i32,
    lx: u8,
    lz: u8,
) -> Result<Option<VanillaColumn>, String> {
    let surface_top = match surface_top(root, lx, lz)? {
        Some(y) => y,
        None => return Ok(None),
    };
    let sections = root
        .get("sections")
        .or_else(|| root.get("Sections"))
        .and_then(Tag::as_list)
        .ok_or("chunk has no sections list")?;
    let mut top_block = None;
    let mut biome = None;
    for section in sections {
        let Some(y_min) = section.get("Y").and_then(Tag::as_i32).map(|y| y * 16) else {
            continue;
        };
        if surface_top < y_min || surface_top >= y_min + 16 {
            continue;
        }
        top_block = block_at(section, lx, lz, surface_top - y_min)?;
        biome = biome_at(section, lx, surface_top, lz)?;
        break;
    }
    Ok(Some(VanillaColumn {
        x: i64::from(chunk_x) * 16 + i64::from(lx),
        z: i64::from(chunk_z) * 16 + i64::from(lz),
        surface_y: surface_top,
        top_block,
        biome,
    }))
}

/// One column's stored substance profile: the air/fluid/solid category
/// of every absolute Y from the lowest stored section up to the heightmap
/// surface. Chunks store sections only where content exists, so a Y not
/// covered by any section is empty: it is recorded as air, matching how
/// the filler would have written it.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnProfile {
    pub x: i64,
    pub z: i64,
    pub surface_y: i32,
    pub min_y: i32,
    pub categories: Vec<Category>,
    /// Saved block-state names at each absolute Y. Missing sections are air.
    pub block_ids: Vec<String>,
}

pub fn column_profile(
    root: &Tag,
    chunk_x: i32,
    chunk_z: i32,
    lx: u8,
    lz: u8,
) -> Result<Option<ColumnProfile>, String> {
    let surface_y = match surface_top(root, lx, lz)? {
        Some(y) => y,
        None => return Ok(None),
    };
    let sections = root
        .get("sections")
        .or_else(|| root.get("Sections"))
        .and_then(Tag::as_list)
        .ok_or("chunk has no sections list")?;
    let stored: Vec<(i32, &Tag)> = sections
        .iter()
        .filter_map(|s| s.get("Y").and_then(Tag::as_i32).map(|y| (y * 16, s)))
        .filter(|(y_min, _)| *y_min <= surface_y)
        .collect();
    let min_y = stored
        .iter()
        .map(|(y_min, _)| *y_min)
        .min()
        .unwrap_or(surface_y);
    let mut categories = vec![Category::Air; (surface_y - min_y + 1) as usize];
    let mut block_ids = vec!["minecraft:air".to_owned(); categories.len()];
    for (y_min, section) in &stored {
        for local_y in 0..16 {
            let y = y_min + local_y;
            if y > surface_y {
                break;
            }
            if let Some(name) = block_at(section, lx, lz, local_y)? {
                categories[(y - min_y) as usize] = block_category(&name);
                block_ids[(y - min_y) as usize] = name;
            }
        }
    }
    Ok(Some(ColumnProfile {
        x: i64::from(chunk_x) * 16 + i64::from(lx),
        z: i64::from(chunk_z) * 16 + i64::from(lz),
        surface_y,
        min_y,
        categories,
        block_ids,
    }))
}

fn block_at(section: &Tag, lx: u8, lz: u8, local_y: i32) -> Result<Option<String>, String> {
    let states = section
        .get("block_states")
        .or_else(|| section.get("BlockStates"))
        .ok_or("section without block_states")?;
    let palette = states
        .get("palette")
        .and_then(Tag::as_list)
        .ok_or("block_states without palette")?;
    let index = usize::from(lx) + usize::from(lz) * 16 + (local_y as usize) * 256;
    let value = palette_value(palette, states.get("data"), index, 16 * 16 * 16)?;
    Ok(value.and_then(state_name))
}

/// Blockstate palette entry names: 26.3 saves use `{"": "name"}` for
/// property-less states and `{"id": "name", "properties": {…}}` otherwise;
/// older/legacy entries use `{"Name": …, "Properties": …}`.
fn state_name(entry: &Tag) -> Option<String> {
    match entry {
        Tag::String(s) => Some(s.clone()),
        Tag::Compound(_) => {
            let name = entry
                .get("Name")
                .or_else(|| entry.get(""))
                .or_else(|| entry.get("id"))
                .and_then(Tag::as_str)?;
            let props = entry
                .get("Properties")
                .or_else(|| entry.get("properties"))
                .and_then(|p| match p {
                    Tag::Compound(map) => Some(map),
                    _ => None,
                });
            let Some(props) = props.filter(|p| !p.is_empty()) else {
                return Some(name.to_string());
            };
            let inner: Vec<String> = props
                .iter()
                .map(|(k, v)| {
                    let value = match v {
                        Tag::String(s) => s.clone(),
                        other => format!("{other:?}"),
                    };
                    format!("{k}={value}")
                })
                .collect();
            Some(format!("{name}[{}]", inner.join(",")))
        }
        _ => None,
    }
}

fn biome_at(section: &Tag, lx: u8, y: i32, lz: u8) -> Result<Option<String>, String> {
    let biomes = section
        .get("biomes")
        .or_else(|| section.get("Biomes"))
        .ok_or("section without biomes")?;
    let palette = biomes
        .get("palette")
        .and_then(Tag::as_list)
        .ok_or("biomes without palette")?;
    let local_y = y - section
        .get("Y")
        .and_then(Tag::as_i32)
        .map(|v| v * 16)
        .unwrap_or(i32::MIN);
    let index = usize::from(lx) / 4 + (usize::from(lz) / 4) * 4 + (local_y as usize / 4) * 16;
    let value = palette_value(palette, biomes.get("data"), index, 4 * 4 * 4)?;
    Ok(value.and_then(|tag| tag.as_str().map(str::to_string)))
}

fn palette_value<'a>(
    palette: &'a [Tag],
    data: Option<&'a Tag>,
    index: usize,
    count: usize,
) -> Result<Option<&'a Tag>, String> {
    if palette.len() == 1 {
        return Ok(palette.first());
    }
    let computed = (palette.len() - 1).ilog2() as usize + 1;
    let packed = data
        .and_then(Tag::as_long_array)
        .ok_or("multi-value palette without data array")?;
    // 26.3 disk sections use the exact bit width the palette needs (biome
    // palettes with two entries are stored at one bit, observed in the
    // owner's world), and stored data can outlive palette changes. Derive
    // candidate widths from the data length; where several widths share a
    // long count (64 entries: 3 or 4 bits), accept the first whose decoded
    // slots all fit the palette, preferring the palette-derived width.
    let mut widths: Vec<usize> = (1..=16)
        .filter(|&bits| {
            let per_long = 64 / bits;
            per_long > 0 && packed.len() == count.div_ceil(per_long)
        })
        .collect();
    widths.sort_by_key(|bits| (bits != &computed, *bits));
    if widths.is_empty() {
        widths.push(computed);
    }
    for bits in widths {
        let slots = unpack_non_spanning(packed, bits, index + 1);
        if slots.iter().all(|slot| (*slot as usize) < palette.len()) {
            return Ok(palette.get(slots[index] as usize));
        }
    }
    Err(format!(
        "no consistent palette width for {} entries",
        palette.len()
    ))
}

pub fn unpack_spanning(data: &[i64], bits: usize, count: usize) -> Vec<u32> {
    let mask = (1u64 << bits) - 1;
    (0..count)
        .map(|i| {
            let bit = i * bits;
            let long = bit / 64;
            let off = bit % 64;
            let cur = data.get(long).copied().unwrap_or(0) as u64;
            let low = cur >> off;
            let value = if off + bits <= 64 {
                low
            } else {
                let next = data.get(long + 1).copied().unwrap_or(0) as u64;
                low | (next << (64 - off))
            };
            (value & mask) as u32
        })
        .collect()
}

fn unpack_non_spanning(data: &[i64], bits: usize, count: usize) -> Vec<u32> {
    let mask = (1u64 << bits) - 1;
    let per_long = 64 / bits;
    (0..count)
        .map(|i| {
            let long = i / per_long;
            let off = (i % per_long) * bits;
            let value = data.get(long).copied().unwrap_or(0) as u64;
            ((value >> off) & mask) as u32
        })
        .collect()
}

/// Grid points a sampler could not score, split by cause so a report can
/// state its denominator completely.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SampleGaps {
    /// No chunk is stored in the region file at the grid point.
    pub missing_chunks: usize,
    /// A chunk is stored but holds no filled surface column yet: the
    /// client wrote it before the height pass, or it is a not-generated
    /// placeholder.
    pub ungenerated_columns: usize,
}

impl SampleGaps {
    pub fn total(self) -> usize {
        self.missing_chunks + self.ungenerated_columns
    }
}

/// Sample every `stride`-th column in the inclusive block range, skipping
/// chunks the world has not generated. Deterministic order (z then x).
pub fn sample_columns(
    store: &mut RegionStore,
    min_x: i64,
    max_x: i64,
    min_z: i64,
    max_z: i64,
    stride: i64,
) -> Result<(Vec<VanillaColumn>, SampleGaps), String> {
    sample_grid(store, min_x, max_x, min_z, max_z, stride, column_truth)
}

/// Like [`sample_columns`], but reading full substance profiles.
pub fn sample_profiles(
    store: &mut RegionStore,
    min_x: i64,
    max_x: i64,
    min_z: i64,
    max_z: i64,
    stride: i64,
) -> Result<(Vec<ColumnProfile>, SampleGaps), String> {
    sample_grid(store, min_x, max_x, min_z, max_z, stride, column_profile)
}

/// One column read from a stored chunk root, shared by the samplers.
type ColumnReader<T> = fn(&Tag, i32, i32, u8, u8) -> Result<Option<T>, String>;

fn sample_grid<T>(
    store: &mut RegionStore,
    min_x: i64,
    max_x: i64,
    min_z: i64,
    max_z: i64,
    stride: i64,
    read: ColumnReader<T>,
) -> Result<(Vec<T>, SampleGaps), String> {
    if stride < 1 {
        return Err("stride must be at least 1".to_string());
    }
    let mut columns = Vec::new();
    let mut gaps = SampleGaps::default();
    let stride = u8::try_from(stride).map_err(|_| "stride above 255 unsupported".to_string())?;
    let mut z = min_z;
    while z <= max_z {
        let mut x = min_x;
        while x <= max_x {
            let chunk_x =
                i32::try_from(x.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
            let chunk_z =
                i32::try_from(z.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
            let lx = u8::try_from(x.rem_euclid(16)).expect("rem_euclid(16) fits u8");
            let lz = u8::try_from(z.rem_euclid(16)).expect("rem_euclid(16) fits u8");
            match store.chunk_root(chunk_x, chunk_z) {
                Ok(None) => gaps.missing_chunks += 1,
                Ok(Some(root)) => {
                    let column = (read)(&root, chunk_x, chunk_z, lx, lz)
                        .map_err(|e| format!("chunk ({chunk_x}, {chunk_z}) at ({x}, {z}): {e}"))?;
                    match column {
                        Some(column) => columns.push(column),
                        None => gaps.ungenerated_columns += 1,
                    }
                }
                Err(e) => return Err(format!("chunk ({chunk_x}, {chunk_z}): {e}")),
            }
            x += i64::from(stride);
        }
        z += i64::from(stride);
    }
    Ok((columns, gaps))
}

pub fn worksheet_columns() -> [(i64, i64); 6] {
    [(0, 0), (256, 0), (0, 256), (-256, 0), (0, -256), (512, 512)]
}

/// Read one absolute column's stored substance profile, or `None` when
/// the chunk is not generated.
pub fn read_profile(
    store: &mut RegionStore,
    x: i64,
    z: i64,
) -> Result<Option<ColumnProfile>, String> {
    let chunk_x =
        i32::try_from(x.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
    let chunk_z =
        i32::try_from(z.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
    let lx = u8::try_from(x.rem_euclid(16)).expect("rem_euclid(16) fits u8");
    let lz = u8::try_from(z.rem_euclid(16)).expect("rem_euclid(16) fits u8");
    match store.chunk_root(chunk_x, chunk_z)? {
        None => Ok(None),
        Some(root) => column_profile(&root, chunk_x, chunk_z, lx, lz),
    }
}

/// One requested column read: absolute coordinates plus the extracted
/// column, or a per-column error.
pub type ColumnRead = (i64, i64, Result<Option<VanillaColumn>, String>);

/// Block families the T4 decoration baseline counts: ore feature
/// outputs (stone and deepslate variants kept apart because they are
/// placed by different distributions), the raw-metal vein outputs,
/// the vein/blob filler stones, the base stones that ores replace,
/// and the vegetation families trees and surface patches produce.
/// Everything outside this list is ignored by the census.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CensusFamily {
    Stone,
    Deepslate,
    Coal,
    DeepslateCoal,
    Iron,
    DeepslateIron,
    Copper,
    DeepslateCopper,
    Gold,
    DeepslateGold,
    Redstone,
    DeepslateRedstone,
    Lapis,
    DeepslateLapis,
    Diamond,
    DeepslateDiamond,
    Emerald,
    DeepslateEmerald,
    RawCopper,
    RawIron,
    RawGold,
    AncientDebris,
    Granite,
    Diorite,
    Andesite,
    Tuff,
    Logs,
    Leaves,
    Saplings,
    Grasses,
    Flowers,
    Cactus,
    SugarCane,
}

impl CensusFamily {
    pub fn name(self) -> &'static str {
        match self {
            Self::Stone => "stone",
            Self::Deepslate => "deepslate",
            Self::Coal => "coal_ore",
            Self::DeepslateCoal => "deepslate_coal_ore",
            Self::Iron => "iron_ore",
            Self::DeepslateIron => "deepslate_iron_ore",
            Self::Copper => "copper_ore",
            Self::DeepslateCopper => "deepslate_copper_ore",
            Self::Gold => "gold_ore",
            Self::DeepslateGold => "deepslate_gold_ore",
            Self::Redstone => "redstone_ore",
            Self::DeepslateRedstone => "deepslate_redstone_ore",
            Self::Lapis => "lapis_ore",
            Self::DeepslateLapis => "deepslate_lapis_ore",
            Self::Diamond => "diamond_ore",
            Self::DeepslateDiamond => "deepslate_diamond_ore",
            Self::Emerald => "emerald_ore",
            Self::DeepslateEmerald => "deepslate_emerald_ore",
            Self::RawCopper => "raw_copper_block",
            Self::RawIron => "raw_iron_block",
            Self::RawGold => "raw_gold_block",
            Self::AncientDebris => "ancient_debris",
            Self::Granite => "granite",
            Self::Diorite => "diorite",
            Self::Andesite => "andesite",
            Self::Tuff => "tuff",
            Self::Logs => "logs",
            Self::Leaves => "leaves",
            Self::Saplings => "saplings",
            Self::Grasses => "grasses",
            Self::Flowers => "flowers",
            Self::Cactus => "cactus",
            Self::SugarCane => "sugar_cane",
        }
    }

    pub fn all() -> [Self; 33] {
        [
            Self::Stone,
            Self::Deepslate,
            Self::Coal,
            Self::DeepslateCoal,
            Self::Iron,
            Self::DeepslateIron,
            Self::Copper,
            Self::DeepslateCopper,
            Self::Gold,
            Self::DeepslateGold,
            Self::Redstone,
            Self::DeepslateRedstone,
            Self::Lapis,
            Self::DeepslateLapis,
            Self::Diamond,
            Self::DeepslateDiamond,
            Self::Emerald,
            Self::DeepslateEmerald,
            Self::RawCopper,
            Self::RawIron,
            Self::RawGold,
            Self::AncientDebris,
            Self::Granite,
            Self::Diorite,
            Self::Andesite,
            Self::Tuff,
            Self::Logs,
            Self::Leaves,
            Self::Saplings,
            Self::Grasses,
            Self::Flowers,
            Self::Cactus,
            Self::SugarCane,
        ]
    }
}

/// Classify one base block id (no property suffix) into a census family.
pub fn census_family(base_name: &str) -> Option<CensusFamily> {
    let name = base_name.strip_prefix("minecraft:")?;
    let family = match name {
        "stone" => CensusFamily::Stone,
        "deepslate" => CensusFamily::Deepslate,
        "coal_ore" => CensusFamily::Coal,
        "deepslate_coal_ore" => CensusFamily::DeepslateCoal,
        "iron_ore" => CensusFamily::Iron,
        "deepslate_iron_ore" => CensusFamily::DeepslateIron,
        "copper_ore" => CensusFamily::Copper,
        "deepslate_copper_ore" => CensusFamily::DeepslateCopper,
        "gold_ore" => CensusFamily::Gold,
        "deepslate_gold_ore" => CensusFamily::DeepslateGold,
        "redstone_ore" => CensusFamily::Redstone,
        "deepslate_redstone_ore" => CensusFamily::DeepslateRedstone,
        "lapis_ore" => CensusFamily::Lapis,
        "deepslate_lapis_ore" => CensusFamily::DeepslateLapis,
        "diamond_ore" => CensusFamily::Diamond,
        "deepslate_diamond_ore" => CensusFamily::DeepslateDiamond,
        "emerald_ore" => CensusFamily::Emerald,
        "deepslate_emerald_ore" => CensusFamily::DeepslateEmerald,
        "raw_copper_block" => CensusFamily::RawCopper,
        "raw_iron_block" => CensusFamily::RawIron,
        "raw_gold_block" => CensusFamily::RawGold,
        "ancient_debris" => CensusFamily::AncientDebris,
        "granite" => CensusFamily::Granite,
        "diorite" => CensusFamily::Diorite,
        "andesite" => CensusFamily::Andesite,
        "tuff" => CensusFamily::Tuff,
        "short_grass" | "tall_grass" | "grass" | "fern" | "large_fern" => CensusFamily::Grasses,
        "cactus" => CensusFamily::Cactus,
        "sugar_cane" => CensusFamily::SugarCane,
        "dandelion" | "poppy" | "blue_orchid" | "allium" | "azure_bluet" | "red_tulip"
        | "orange_tulip" | "white_tulip" | "pink_tulip" | "oxeye_daisy" | "cornflower"
        | "lily_of_the_valley" | "with_rose" | "torchflower" | "pitcher_plant" => {
            CensusFamily::Flowers
        }
        other => {
            if other.ends_with("_log") || other.ends_with("_stem") || other.ends_with("hyphae") {
                CensusFamily::Logs
            } else if other.ends_with("_leaves") || other.ends_with("foliage") {
                CensusFamily::Leaves
            } else if other.ends_with("_sapling") {
                CensusFamily::Saplings
            } else {
                return None;
            }
        }
    };
    Some(family)
}

/// Decode every slot of one section's block palette into name counts.
/// The width selection mirrors `palette_value`'s candidate ordering but
/// validates all 4096 slots, which a whole-section decode can afford.
pub fn section_name_counts(section: &Tag) -> Result<Vec<(String, usize)>, String> {
    const SLOTS: usize = 16 * 16 * 16;
    let states = section
        .get("block_states")
        .or_else(|| section.get("BlockStates"))
        .ok_or("section without block_states")?;
    let palette = states
        .get("palette")
        .and_then(Tag::as_list)
        .ok_or("block_states without palette")?;
    if palette.len() == 1 {
        let name = state_name(&palette[0]).ok_or("unusable palette entry")?;
        return Ok(vec![(name, SLOTS)]);
    }
    let packed = states
        .get("data")
        .and_then(Tag::as_long_array)
        .ok_or("multi-value palette without data array")?;
    let computed = (palette.len() - 1).ilog2() as usize + 1;
    let mut widths: Vec<usize> = (1..=16)
        .filter(|&bits| {
            let per_long = 64 / bits;
            per_long > 0 && packed.len() == SLOTS.div_ceil(per_long)
        })
        .collect();
    widths.sort_by_key(|bits| (bits != &computed, *bits));
    if widths.is_empty() {
        widths.push(computed);
    }
    for bits in widths {
        let slots = unpack_non_spanning(packed, bits, SLOTS);
        if slots.iter().all(|slot| (*slot as usize) < palette.len()) {
            let mut counts = vec![0usize; palette.len()];
            for slot in slots {
                counts[usize::try_from(slot).expect("slot width <= 16 bits")] += 1;
            }
            let mut out = Vec::new();
            for (entry, count) in palette.iter().zip(counts) {
                if count == 0 {
                    continue;
                }
                out.push((state_name(entry).ok_or("unusable palette entry")?, count));
            }
            return Ok(out);
        }
    }
    Err(format!(
        "no consistent palette width for {} entries",
        palette.len()
    ))
}

/// Aggregate census over a chunk rectangle: family totals and per
/// 32-block Y-band counts. A sections Y window always sits inside one
/// band (sections are 16-aligned, bands 32-wide), so band attribution
/// is exact.
#[derive(Debug, Default)]
pub struct Census {
    pub chunks: usize,
    pub missing_chunks: usize,
    pub totals: std::collections::BTreeMap<CensusFamily, usize>,
    pub bands: std::collections::BTreeMap<(CensusFamily, i32), usize>,
}

impl Census {
    fn record_section(&mut self, section: &Tag) -> Result<(), String> {
        let Some(y_min) = section.get("Y").and_then(Tag::as_i32).map(|y| y * 16) else {
            return Ok(());
        };
        let band = y_min.div_euclid(32) * 32;
        for (name, count) in section_name_counts(section)? {
            if let Some(family) = census_family(base_block_name(&name)) {
                *self.totals.entry(family).or_default() += count;
                *self.bands.entry((family, band)).or_default() += count;
            }
        }
        Ok(())
    }

    fn record_chunk(&mut self, root: &Tag) -> Result<(), String> {
        for section in chunk_sections(root)? {
            self.record_section(section)?;
        }
        self.chunks += 1;
        Ok(())
    }

    /// Fold one generated column (as produced by
    /// `VanillaGenerator::column_ids`, indexed from the dimension's
    /// minimum build Y) into the census with the same band windows as
    /// the stored decode.
    pub fn record_column_ids(&mut self, min_y: i32, ids: &[Option<String>]) {
        for (offset, id) in ids.iter().enumerate() {
            let Some(name) = id else { continue };
            let band = (min_y + offset as i32).div_euclid(32) * 32;
            if let Some(family) = census_family(base_block_name(name)) {
                *self.totals.entry(family).or_default() += 1;
                *self.bands.entry((family, band)).or_default() += 1;
            }
        }
    }
}

/// The stored section list of a chunk root, accepting both NBT spelling
/// variants used across versions.
fn chunk_sections(root: &Tag) -> Result<&[Tag], String> {
    root.get("sections")
        .or_else(|| root.get("Sections"))
        .and_then(Tag::as_list)
        .ok_or_else(|| "chunk has no sections list".to_owned())
}

/// Histogram of every block-state name stored in the inclusive chunk
/// rectangle, with the number of census chunks read. This is the
/// classification audit tool: it lists what the save actually contains
/// so `census_family` can be checked against real ids rather than
/// assumed names.
pub fn census_name_histogram(
    store: &mut RegionStore,
    min_cx: i32,
    max_cx: i32,
    min_cz: i32,
    max_cz: i32,
) -> Result<(usize, std::collections::BTreeMap<String, usize>), String> {
    let mut chunks = 0usize;
    let mut names = std::collections::BTreeMap::new();
    for chunk_z in min_cz..=max_cz {
        for chunk_x in min_cx..=max_cx {
            let Some(root) = store.chunk_root(chunk_x, chunk_z)? else {
                continue;
            };
            for section in
                chunk_sections(&root).map_err(|e| format!("chunk ({chunk_x}, {chunk_z}): {e}"))?
            {
                for (name, count) in section_name_counts(section)? {
                    *names.entry(name).or_default() += count;
                }
            }
            chunks += 1;
        }
    }
    Ok((chunks, names))
}

/// Census every stored chunk with coordinates in the inclusive chunk
/// rectangle; ungenerated chunks are counted as missing.
pub fn census_chunks(
    store: &mut RegionStore,
    min_cx: i32,
    max_cx: i32,
    min_cz: i32,
    max_cz: i32,
) -> Result<Census, String> {
    let mut census = Census::default();
    for chunk_z in min_cz..=max_cz {
        for chunk_x in min_cx..=max_cx {
            match store.chunk_root(chunk_x, chunk_z)? {
                None => census.missing_chunks += 1,
                Some(root) => census
                    .record_chunk(&root)
                    .map_err(|e| format!("chunk ({chunk_x}, {chunk_z}): {e}"))?,
            }
        }
    }
    Ok(census)
}

/// Convenience: chunk rectangle covering the inclusive block range.
pub fn census_blocks(store: &mut RegionStore, min: i64, max: i64) -> Result<Census, String> {
    let to_chunk = |v: i64| -> Result<i32, String> {
        i32::try_from(v.div_euclid(16)).map_err(|_| "coordinate too large".to_string())
    };
    census_chunks(
        store,
        to_chunk(min)?,
        to_chunk(max)?,
        to_chunk(min)?,
        to_chunk(max)?,
    )
}

/// Census the generator's full material-rule descent over the inclusive
/// block square: every column from `min` to `max` in both axes, counted
/// in the same family/band shape as the save census so the two line up
/// row by row. `chunks` reports the 16-aligned chunk square covered.
pub fn census_generated(
    generator: &VanillaGenerator,
    min: i64,
    max: i64,
) -> Result<Census, String> {
    let (Ok(min), Ok(max)) = (i32::try_from(min), i32::try_from(max)) else {
        return Err("coordinate too large".to_string());
    };
    let mut census = Census::default();
    for x in min..=max {
        for z in min..=max {
            let ids = generator.column_ids(x, z);
            census.record_column_ids(generator.min_y(), &ids);
        }
    }
    let side = max - min + 1;
    census.chunks = (side.div_euclid(16) * side.div_euclid(16)) as usize;
    Ok(census)
}

/// Convenience for `inspect`: read specific absolute columns.
pub fn read_columns(
    store: &mut RegionStore,
    points: &[(i64, i64)],
) -> Result<Vec<ColumnRead>, String> {
    let mut out = Vec::new();
    for &(x, z) in points {
        let result = (|| {
            let chunk_x =
                i32::try_from(x.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
            let chunk_z =
                i32::try_from(z.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
            let lx = u8::try_from(x.rem_euclid(16)).expect("rem_euclid(16) fits u8");
            let lz = u8::try_from(z.rem_euclid(16)).expect("rem_euclid(16) fits u8");
            match store.chunk_root(chunk_x, chunk_z)? {
                None => Ok(None),
                Some(root) => column_truth(&root, chunk_x, chunk_z, lx, lz),
            }
        })();
        out.push((x, z, result));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::compound;

    fn palette_entry(name: &str) -> Tag {
        compound(&[
            ("Name", Tag::String(name.into())),
            ("Properties", compound(&[])),
        ])
    }

    /// 16x16x16 section at Y=4 (blocks 64..=79): everything grass_block,
    /// single biome, heightmap says surface top at 75 for all columns.
    /// The chunk has no yPos, so the world minimum is -64 and heightmaps are
    /// packed min_y-relative (140 = 75 - (-64) + 1).
    fn synthetic_chunk(biome_single: bool) -> Tag {
        let mut height_data = vec![0i64; 36]; // ceil(256*9/64) = 36
        let value = 140u64;
        for i in 0..256 {
            let bit = i * 9;
            let long = bit / 64;
            let off = bit % 64;
            height_data[long] |= (value << off) as i64;
            if off + 9 > 64 {
                height_data[long + 1] |= (value >> (64 - off)) as i64;
            }
        }
        let block_states = compound(&[(
            "palette",
            Tag::List(vec![palette_entry("minecraft:grass_block")]),
        )]);
        let biomes = if biome_single {
            compound(&[(
                "palette",
                Tag::List(vec![Tag::String("minecraft:plains".into())]),
            )])
        } else {
            // Two-value palette stored at 3 bits: 4 longs is also
            // consistent with 4 bits, so the validator must pick 3.
            let mut data = vec![0i64; 4]; // 64 entries, 21 per long
            for i in 0..64 {
                let long = i / 21;
                let off = (i % 21) * 3;
                let slot = if i < 40 { 0 } else { 1 };
                data[long] |= (slot as i64) << off;
            }
            compound(&[
                (
                    "palette",
                    Tag::List(vec![
                        Tag::String("minecraft:plains".into()),
                        Tag::String("minecraft:forest".into()),
                    ]),
                ),
                ("data", Tag::LongArray(data)),
            ])
        };
        let section = compound(&[
            ("Y", Tag::Byte(4)),
            ("block_states", block_states),
            ("biomes", biomes),
        ]);
        compound(&[
            (
                "Heightmaps",
                compound(&[("WORLD_SURFACE", Tag::LongArray(height_data))]),
            ),
            ("sections", Tag::List(vec![section])),
        ])
    }

    #[test]
    fn extracts_surface_block_height_and_single_biome() {
        let root = synthetic_chunk(true);
        let column = column_truth(&root, 1, -2, 3, 9).unwrap().expect("column");
        assert_eq!(column.x, 19);
        assert_eq!(column.z, -23);
        assert_eq!(column.surface_y, 75);
        assert_eq!(column.top_block.as_deref(), Some("minecraft:grass_block"));
        assert_eq!(column.biome.as_deref(), Some("minecraft:plains"));
    }

    #[test]
    fn decodes_multi_value_biome_palette() {
        let root = synthetic_chunk(false);
        // local_y 11 within section => y index 11/4=2; lz/4 chooses halves.
        let first = column_truth(&root, 0, 0, 0, 0).unwrap().unwrap();
        assert_eq!(first.biome.as_deref(), Some("minecraft:plains"));
        let second = column_truth(&root, 0, 0, 0, 15).unwrap().unwrap();
        assert_eq!(second.biome.as_deref(), Some("minecraft:forest"));
    }

    #[test]
    fn spanning_and_non_spanning_unpackers_agree_with_manual_packing() {
        // 9-bit spanning: values 500.. pattern crossing long boundaries.
        let mut longs = vec![0i64; 36];
        let values: Vec<u32> = (0..256).map(|i| (i % 512) as u32).collect();
        for (i, v) in values.iter().enumerate() {
            let bit = i * 9;
            let long = bit / 64;
            let off = bit % 64;
            longs[long] |= (*v as i64) << off;
            if off + 9 > 64 {
                longs[long + 1] |= (*v as i64) >> (64 - off);
            }
        }
        assert_eq!(unpack_spanning(&longs, 9, 256), values);
        assert_eq!(unpack_non_spanning(&[0b1010_0101], 4, 4), vec![5, 10, 0, 0]);
    }

    #[test]
    fn state_name_handles_26_3_palette_forms() {
        let bare = compound(&[("", Tag::String("minecraft:stone".into()))]);
        assert_eq!(state_name(&bare).as_deref(), Some("minecraft:stone"));
        let with_props = compound(&[
            ("id", Tag::String("minecraft:water".into())),
            (
                "properties",
                compound(&[("level", Tag::String("3".into()))]),
            ),
        ]);
        assert_eq!(
            state_name(&with_props).as_deref(),
            Some("minecraft:water[level=3]")
        );
        assert_eq!(
            state_name(&Tag::String("minecraft:dirt".into())).as_deref(),
            Some("minecraft:dirt")
        );
    }

    #[test]
    fn ambiguous_palette_width_prefers_slot_consistency() {
        // Two-entry biome palette stored at 4 bits: at 3 bits the second
        // slot decodes to 2 (out of range), at 4 bits both slots are valid.
        let palette = vec![
            Tag::String("minecraft:plains".into()),
            Tag::String("minecraft:forest".into()),
        ];
        let data = Tag::LongArray(vec![0x11, 0, 0, 0]);
        let value = palette_value(&palette, Some(&data), 1, 64).unwrap();
        assert_eq!(value.unwrap().as_str(), Some("minecraft:forest"));
    }

    #[test]
    fn heightmaps_are_relative_to_stored_y_pos() {
        // yPos 0 with value 1 => absolute top block y = 0.
        let mut root = synthetic_chunk(true);
        let Tag::Compound(map) = &mut root else {
            unreachable!()
        };
        map.insert("yPos".to_string(), Tag::Int(0));
        let column = column_truth(&root, 0, 0, 0, 0).unwrap().unwrap();
        assert_eq!(column.surface_y, 140 - 1);
    }

    #[test]
    fn compare_report_counts_matches_and_caps_mismatches() {
        let generator = Generator::new(2026);
        let columns: Vec<VanillaColumn> = [(0i64, 0i64), (256, 0)]
            .into_iter()
            .map(|(x, z)| VanillaColumn {
                x,
                z,
                surface_y: generator.height(x, z) as i32,
                top_block: None,
                biome: Some(generator.biome(x, z).identifier().to_string()),
            })
            .collect();
        let report = compare_columns(columns.iter().cloned(), &generator, 5);
        assert_eq!(report.columns, 2);
        assert_eq!(report.height_matches, 2);
        assert_eq!(report.biome_matches, 2);
        assert!(report.mismatches.is_empty());
        assert_eq!(percent(2, 2), 100.0);
        assert_eq!(percent(0, 0), 0.0);
    }

    #[test]
    fn categories_classify_air_family_and_fluids() {
        assert_eq!(block_category("minecraft:cave_air"), Category::Air);
        assert_eq!(block_category("minecraft:structure_void"), Category::Air);
        assert_eq!(block_category("minecraft:water[level=3]"), Category::Fluid);
        assert_eq!(
            block_category("minecraft:deepslate_gold_ore"),
            Category::Solid
        );
    }

    fn solid_section(y: i32, block: &str) -> Tag {
        compound(&[
            ("Y", Tag::Byte(y as i8)),
            (
                "block_states",
                compound(&[("palette", Tag::List(vec![palette_entry(block)]))]),
            ),
        ])
    }

    /// Same packed heightmap as `synthetic_chunk`: top block 75 over the
    /// implicit -64 world floor.
    fn heightmap_root(sections: Vec<Tag>) -> Tag {
        let mut height_data = vec![0i64; 36];
        let value = 140u64;
        for i in 0..256 {
            let bit = i * 9;
            let long = bit / 64;
            let off = bit % 64;
            height_data[long] |= (value << off) as i64;
            if off + 9 > 64 {
                height_data[long + 1] |= (value >> (64 - off)) as i64;
            }
        }
        compound(&[
            (
                "Heightmaps",
                compound(&[("WORLD_SURFACE", Tag::LongArray(height_data))]),
            ),
            ("sections", Tag::List(sections)),
        ])
    }

    #[test]
    fn profiles_store_categories_with_air_gaps() {
        let root = heightmap_root(vec![
            solid_section(0, "minecraft:water"),
            solid_section(4, "minecraft:stone"),
        ]);
        let profile = column_profile(&root, 0, 0, 3, 9).unwrap().expect("profile");
        assert_eq!((profile.x, profile.z, profile.surface_y), (3, 9, 75));
        assert_eq!(profile.min_y, 0);
        assert_eq!(profile.categories.len(), 76);
        assert_eq!(profile.categories[0], Category::Fluid);
        assert_eq!(profile.categories[15], Category::Fluid);
        // The unsaved 16..=63 span is empty: recorded as air.
        assert_eq!(profile.categories[16], Category::Air);
        assert_eq!(profile.categories[63], Category::Air);
        assert_eq!(profile.categories[64], Category::Solid);
        assert_eq!(profile.categories[75], Category::Solid);
        assert_eq!(profile.block_ids[16], "minecraft:air");
        assert_eq!(profile.block_ids[64], "minecraft:stone");
    }

    #[test]
    fn stored_chunks_without_heightmaps_are_ungenerated() {
        // A chunk the client wrote before the height pass: no Heightmaps
        // compound at all. That is an ungenerated column, not a decode
        // failure that would abort a sample.
        let root = compound(&[(
            "sections",
            Tag::List(vec![solid_section(0, "minecraft:stone")]),
        )]);
        assert_eq!(column_truth(&root, 0, 0, 3, 9).unwrap(), None);
        assert_eq!(column_profile(&root, 0, 0, 3, 9).unwrap(), None);
    }

    #[derive(Clone, Copy)]
    struct HalfAir;

    impl ColumnSource for HalfAir {
        fn column_height(&self, _x: i64, _z: i64) -> i64 {
            0
        }
        fn column_biome(&self, _x: i64, _z: i64) -> String {
            "test".to_owned()
        }
        fn column_substance(&self, _x: i64, _z: i64, y: i32) -> Option<Category> {
            Some(if y < 32 {
                Category::Solid
            } else {
                Category::Air
            })
        }
    }

    #[test]
    fn substance_report_counts_positions_bands_and_residuals() {
        let profile = ColumnProfile {
            x: 0,
            z: 0,
            surface_y: 3,
            min_y: 0,
            categories: vec![
                Category::Solid,
                Category::Air,
                Category::Air,
                Category::Solid,
            ],
            block_ids: vec![
                "minecraft:stone".to_owned(),
                "minecraft:air".to_owned(),
                "minecraft:air".to_owned(),
                "minecraft:stone".to_owned(),
            ],
        };
        // The legacy generator has no 3D answer: nothing is counted.
        let pending = compare_substance([profile.clone()], &Generator::new(2026), 5);
        assert_eq!((pending.columns, pending.positions), (1, 0));
        let report = compare_substance([profile], &HalfAir, 5);
        assert_eq!((report.positions, report.matches), (4, 2));
        assert_eq!(report.fail_positions, vec![(0, 0, 1), (0, 0, 2)]);
        assert_eq!((report.near_positions, report.near_matches), (4, 2));
        assert_eq!(report.middle_positions, 0);
        assert_eq!(report.deep_positions, 0);
        assert_eq!(
            report
                .residuals
                .get(&(Category::Air, Category::Solid))
                .copied(),
            Some(2)
        );
        assert_eq!(
            report
                .residual_bands
                .get(&((Category::Air, Category::Solid), 0))
                .copied(),
            Some(2)
        );
        assert_eq!(
            report.marginals.get(&0),
            Some(&BandStats {
                vanilla: [2, 0, 2],
                rustmc: [0, 0, 4],
            })
        );
    }

    /// Descent stub with an explicit `min_y` and per-row answers, so the
    /// block report is testable without operator worldgen data.
    struct StubDescent {
        min_y: i32,
        ids: Vec<Option<String>>,
    }

    impl ColumnDescent for StubDescent {
        fn column_block_ids(&self, _x: i32, _z: i32) -> Vec<Option<String>> {
            self.ids.clone()
        }
        fn column_min_y(&self) -> i32 {
            self.min_y
        }
    }

    /// Saved column from Y 30 to Y 40 with two void rows (36 and 38) and
    /// one property-carrying state, the fixture for the block report.
    fn block_profile(x: i64) -> ColumnProfile {
        let saved: [&str; 11] = [
            "minecraft:stone",
            "minecraft:stone",
            "minecraft:stone",
            "minecraft:stone",
            "minecraft:stone",
            "minecraft:grass_block[snowy=false]",
            "minecraft:cave_air",
            "minecraft:stone",
            "minecraft:air",
            "minecraft:stone",
            "minecraft:stone",
        ];
        ColumnProfile {
            x,
            z: 0,
            surface_y: 40,
            min_y: 30,
            categories: saved
                .iter()
                .map(|name| block_category(name))
                .collect::<Vec<_>>(),
            block_ids: saved.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    fn stub_descent() -> StubDescent {
        let ids: [&str; 10] = [
            "minecraft:cave_air",
            "minecraft:cave_air",
            "minecraft:stone",
            "minecraft:granite",
            "minecraft:grass_block[snowy=true]",
            "minecraft:stone",
            "minecraft:stone",
            "minecraft:cave_air",
            "minecraft:air",
            "minecraft:stone",
        ];
        StubDescent {
            // Index 0 is Y 31: the saved Y 30 row is out of the generator
            // build range for this sample.
            min_y: 31,
            ids: ids.iter().map(|name| Some((*name).to_owned())).collect(),
        }
    }

    #[test]
    fn block_report_scores_bands_air_family_and_cave_context() {
        let report = compare_block_ids([block_profile(0)], &stub_descent(), 20);
        // One stored row below the generator's build range is reported,
        // not silently scored.
        assert_eq!((report.columns, report.positions), (1, 10));
        assert_eq!(report.out_of_range_rows, 1);
        assert_eq!(report.out_of_domain_columns, 0);
        assert_eq!(report.declined_columns, 0);
        // Y 31 and 32 (we opened rock the save fills), 34 (granite vs
        // stone), 36 (stone in a saved void), 38 (cave_air vs air), and
        // 39 (air vs stone) disagree on base ids; only the 38 pair survives
        // air-family normalisation.
        assert_eq!((report.exact_base, report.exact_base_air_family), (4, 5));
        assert_eq!(
            report.bands.get(&0),
            Some(&Agreement {
                positions: 1,
                matches: 0,
                matches_air_family: 0
            })
        );
        assert_eq!(
            report.bands.get(&32),
            Some(&Agreement {
                positions: 9,
                matches: 4,
                matches_air_family: 5
            })
        );
        // Void rows are 36 and 38; Y 31 is five rows from the nearest one,
        // everything else in the sample is cave wall.
        assert_eq!(
            report.cave,
            [
                (
                    CaveContext::Void,
                    Agreement {
                        positions: 2,
                        matches: 0,
                        matches_air_family: 1
                    }
                ),
                (
                    CaveContext::CaveWall,
                    Agreement {
                        positions: 7,
                        matches: 4,
                        matches_air_family: 4
                    }
                ),
                (
                    CaveContext::IntactRock,
                    Agreement {
                        positions: 1,
                        matches: 0,
                        matches_air_family: 0
                    }
                ),
            ]
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>()
        );
        // Three rows we opened that the save fills (Y 31 far from any void,
        // Y 32 and Y 39 against a cave wall) and one row the save leaves
        // air that we filled.
        assert_eq!(
            (
                report.extra_carves,
                report.extra_carves_near_void,
                report.missed_carves
            ),
            (3, 2, 1)
        );
        assert_eq!(
            report
                .missed_carves_bands
                .iter()
                .map(|(band, count)| (*band, *count))
                .collect::<Vec<_>>(),
            vec![(32, 1)]
        );
        assert_eq!(
            report
                .extra_carves_bands
                .iter()
                .map(|(band, count)| (*band, *count))
                .collect::<Vec<_>>(),
            vec![(0, 1), (32, 2)]
        );
        assert_eq!(
            report.residuals.get(&(
                "minecraft:stone".to_owned(),
                "minecraft:cave_air".to_owned()
            )),
            Some(&2)
        );
        assert_eq!(
            report
                .residuals
                .get(&("minecraft:air".to_owned(), "minecraft:cave_air".to_owned())),
            Some(&1)
        );
        assert_eq!(
            report.residuals.get(&(
                "minecraft:cave_air".to_owned(),
                "minecraft:stone".to_owned()
            )),
            Some(&1)
        );
        assert_eq!(report.residuals.values().sum::<usize>(), 6);
        // Air-name bookkeeping collapses to one air key: the 38 pair
        // disappears and the 36 pair is keyed as air, not cave_air. The
        // property-carrying grass_block pair is never a residual at all.
        assert_eq!(report.residuals_air_family.values().sum::<usize>(), 5);
        // The cave view is the same population minus the intact-rock row:
        // what goes wrong next to a saved void.
        assert_eq!(
            report
                .residuals_cave
                .iter()
                .map(|(pair, count)| (pair.clone(), *count))
                .collect::<Vec<_>>(),
            vec![
                (
                    ("minecraft:air".to_owned(), "minecraft:cave_air".to_owned()),
                    1
                ),
                (
                    (
                        "minecraft:cave_air".to_owned(),
                        "minecraft:stone".to_owned()
                    ),
                    1
                ),
                (
                    ("minecraft:stone".to_owned(), "minecraft:air".to_owned()),
                    1
                ),
                (
                    (
                        "minecraft:stone".to_owned(),
                        "minecraft:cave_air".to_owned()
                    ),
                    1
                ),
                (
                    ("minecraft:stone".to_owned(), "minecraft:granite".to_owned()),
                    1
                ),
            ]
        );
        assert_eq!(
            report
                .residuals_air_family
                .get(&("minecraft:stone".to_owned(), "minecraft:air".to_owned())),
            Some(&3)
        );
        assert_eq!(
            report
                .residuals_air_family
                .get(&("minecraft:air".to_owned(), "minecraft:stone".to_owned())),
            Some(&1)
        );
        assert_eq!(
            report.residuals_air_family.get(&(
                "minecraft:cave_air".to_owned(),
                "minecraft:stone".to_owned()
            )),
            None
        );
        assert_eq!(
            report.fail_positions,
            vec![
                (0, 0, 31),
                (0, 0, 32),
                (0, 0, 34),
                (0, 0, 36),
                (0, 0, 38),
                (0, 0, 39)
            ]
        );
    }

    #[test]
    fn block_report_caps_fails_and_reports_unscorable_columns() {
        let report = compare_block_ids([block_profile(0)], &stub_descent(), 2);
        assert_eq!(report.fail_positions.len(), 2);
        // A descent with no answer anywhere is a declined column, not a
        // column of air disagreements.
        let empty = StubDescent {
            min_y: 31,
            ids: vec![None; 10],
        };
        let report = compare_block_ids([block_profile(0)], &empty, 5);
        assert_eq!(
            (report.columns, report.positions, report.declined_columns),
            (0, 0, 1)
        );
        // Coordinates outside the generator's domain are counted, not dropped.
        let report =
            compare_block_ids([block_profile(i64::from(i32::MAX) + 1)], &stub_descent(), 5);
        assert_eq!(
            (
                report.columns,
                report.positions,
                report.out_of_domain_columns
            ),
            (0, 0, 1)
        );
    }

    #[test]
    fn air_family_name_collapses_only_the_air_family() {
        assert_eq!(air_family_name("minecraft:cave_air"), "minecraft:air");
        assert_eq!(air_family_name("minecraft:void_air"), "minecraft:air");
        assert_eq!(air_family_name("minecraft:stone"), "minecraft:stone");
        assert_eq!(air_family_name("minecraft:water"), "minecraft:water");
    }

    #[test]
    fn census_family_classification_follows_public_block_names() {
        assert_eq!(
            census_family("minecraft:coal_ore"),
            Some(CensusFamily::Coal)
        );
        assert_eq!(
            census_family("minecraft:deepslate_iron_ore"),
            Some(CensusFamily::DeepslateIron)
        );
        assert_eq!(
            census_family("minecraft:raw_gold_block"),
            Some(CensusFamily::RawGold)
        );
        assert_eq!(
            census_family("minecraft:raw_copper_block"),
            Some(CensusFamily::RawCopper)
        );
        assert_eq!(
            census_family("minecraft:raw_iron_block"),
            Some(CensusFamily::RawIron)
        );
        assert_eq!(census_family("minecraft:oak_log"), Some(CensusFamily::Logs));
        assert_eq!(
            census_family("minecraft:flowering_azalea_leaves"),
            Some(CensusFamily::Leaves)
        );
        assert_eq!(
            census_family("minecraft:short_grass"),
            Some(CensusFamily::Grasses)
        );
        assert_eq!(
            census_family("minecraft:poppy"),
            Some(CensusFamily::Flowers)
        );
        assert_eq!(census_family("minecraft:stone"), Some(CensusFamily::Stone));
        assert_eq!(
            census_family("minecraft:granite"),
            Some(CensusFamily::Granite)
        );
        assert_eq!(census_family("minecraft:tuff"), Some(CensusFamily::Tuff));
        // Surface and crafted materials are not decoration families.
        assert_eq!(census_family("minecraft:grass_block"), None);
        assert_eq!(census_family("minecraft:cobblestone"), None);
        assert_eq!(census_family("minecraft:tuff_bricks"), None);
    }

    #[test]
    fn section_name_counts_decodes_single_and_multi_entry_palettes() {
        let single = compound(&[(
            "block_states",
            compound(&[("palette", Tag::List(vec![palette_entry("minecraft:stone")]))]),
        )]);
        assert_eq!(
            section_name_counts(&single).unwrap(),
            vec![("minecraft:stone".to_owned(), 4096)]
        );
        // Two-entry palette at the minimal 1-bit width: the first 100
        // slots are coal, the rest air.
        let mut data = vec![0i64; 64];
        for i in 0..100 {
            data[i / 64] |= 1i64 << (i % 64);
        }
        let multi = compound(&[(
            "block_states",
            compound(&[
                (
                    "palette",
                    Tag::List(vec![
                        Tag::String("minecraft:air".into()),
                        Tag::String("minecraft:coal_ore".into()),
                    ]),
                ),
                ("data", Tag::LongArray(data)),
            ]),
        )]);
        let counts = section_name_counts(&multi).unwrap();
        assert!(counts.contains(&("minecraft:coal_ore".to_owned(), 100)));
        assert!(counts.contains(&("minecraft:air".to_owned(), 3996)));
    }

    #[test]
    fn census_bands_attribute_sections_to_their_32_block_window() {
        fn diamond_section(y: i32) -> Tag {
            compound(&[
                ("Y", Tag::Byte(y as i8)),
                (
                    "block_states",
                    compound(&[(
                        "palette",
                        Tag::List(vec![palette_entry("minecraft:deepslate_diamond_ore")]),
                    )]),
                ),
            ])
        }
        let root = compound(&[(
            "sections",
            Tag::List(vec![diamond_section(4), diamond_section(6)]),
        )]);
        let mut census = Census::default();
        census.record_chunk(&root).expect("census");
        assert_eq!(census.chunks, 1);
        assert_eq!(
            census.totals.get(&CensusFamily::DeepslateDiamond),
            Some(&8192)
        );
        // Y=4 -> 64..79 in band 64; Y=6 -> 96..111 in band 96.
        assert_eq!(
            census.bands.get(&(CensusFamily::DeepslateDiamond, 64)),
            Some(&4096)
        );
        assert_eq!(
            census.bands.get(&(CensusFamily::DeepslateDiamond, 96)),
            Some(&4096)
        );
    }
}
