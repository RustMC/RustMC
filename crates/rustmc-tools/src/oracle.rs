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

/// Which generator the scored RustMC-side answers came from. Recorded in every
/// coverage line so a zero can never be read as a parity result: `preview` and
/// `experimental` are RustMC's own terrain models, not the vanilla pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoredSource {
    /// The data-driven vanilla density and surface pipeline.
    Vanilla,
    /// `Terrain::Preview`, the placeholder terrain model.
    Preview,
    /// `Terrain::Experimental`, the experimental terrain model.
    Experimental,
    /// Save-side accounting only: no RustMC answer is scored in this mode.
    Save,
}

impl ScoredSource {
    pub fn name(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Preview => "preview-not-vanilla-generator",
            Self::Experimental => "experimental-not-vanilla-generator",
            Self::Save => "save-only",
        }
    }
}

/// Denominator accounting for one oracle run, printed as one machine-readable
/// line by every scoring mode. The denominators are save-driven, so a wrong
/// world directory or a partially generated save shrinks the population instead
/// of raising an error: `expected` records the grid or chunk square the mode
/// asked for, `missing` the units the save could not supply, and `unaccounted`
/// any unit that is neither scored nor explained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coverage {
    pub scored: usize,
    pub expected: usize,
    pub missing: usize,
    pub unaccounted: usize,
    pub source: ScoredSource,
}

impl Coverage {
    /// Builds the account, deriving `unaccounted` from the other three so no
    /// mode can forget to state the residual.
    pub fn new(scored: usize, expected: usize, missing: usize, source: ScoredSource) -> Self {
        Self {
            scored,
            expected,
            missing,
            unaccounted: expected.saturating_sub(scored).saturating_sub(missing),
            source,
        }
    }

    /// Every requested unit is scored and none is unexplained.
    pub fn is_complete(self) -> bool {
        self.scored == self.expected && self.missing == 0 && self.unaccounted == 0
    }

    /// The machine-readable line, identical whatever follows from it.
    pub fn line(self) -> String {
        format!(
            "coverage scored={} expected={} missing={} unaccounted={} source={}",
            self.scored,
            self.expected,
            self.missing,
            self.unaccounted,
            self.source.name()
        )
    }

    /// The diagnosis, shared by both verdicts so the accepted warning and the
    /// rejected error describe the same shortfall in the same words.
    fn shortfall_reason(self) -> String {
        format!(
            "incomplete measurement: scored={} of expected={} with missing={} unaccounted={} (source={})",
            self.scored,
            self.expected,
            self.missing,
            self.unaccounted,
            self.source.name()
        )
    }

    /// Why this run is not comparable with a full-coverage baseline, and the
    /// flag that would have accepted it.
    fn shortfall(self) -> String {
        format!(
            "{}; a partial population is not comparable with a published baseline, and this mode exits non-zero unless the run is complete (rerun with --allow-partial to accept a partial measurement)",
            self.shortfall_reason()
        )
    }

    /// The one shortfall `--allow-partial` does not rescue: nothing at all was
    /// scored, so there is no partial measurement to read.
    fn empty(self) -> String {
        format!(
            "nothing was scored: expected={} with missing={} unaccounted={} (source={}); the save holds none of the units this run asked for, and --allow-partial accepts a partially populated save, not an empty measurement",
            self.expected,
            self.missing,
            self.unaccounted,
            self.source.name()
        )
    }

    /// The lines to print and the exit status: `allow_partial` downgrades a
    /// shortfall to a warning and never changes the coverage line — except a
    /// run that scored nothing, which fails whatever the flag says, because
    /// zeros over an empty sample are the failure this guard exists to catch.
    pub fn outcome(self, allow_partial: bool) -> CoverageOutcome {
        if self.is_complete() {
            return CoverageOutcome::Complete { line: self.line() };
        }
        if allow_partial && self.scored > 0 {
            return CoverageOutcome::PartialAccepted {
                line: self.line(),
                warning: format!(
                    "{}; accepted because --allow-partial is present, so this output is not comparable with a published baseline",
                    self.shortfall_reason()
                ),
            };
        }
        let message = if self.scored == 0 {
            self.empty()
        } else {
            self.shortfall()
        };
        CoverageOutcome::PartialRejected {
            line: self.line(),
            message,
        }
    }
}

/// What a mode does with one coverage report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageOutcome {
    /// Full coverage: print the line and keep the exit status.
    Complete { line: String },
    /// Shortfall tolerated by `--allow-partial`: print the line and a warning.
    PartialAccepted { line: String, warning: String },
    /// Shortfall: print the line, then fail with this message.
    PartialRejected { line: String, message: String },
}

impl CoverageOutcome {
    /// The machine-readable line, which every variant prints unchanged.
    pub fn line(&self) -> &str {
        match self {
            Self::Complete { line }
            | Self::PartialAccepted { line, .. }
            | Self::PartialRejected { line, .. } => line,
        }
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
    let heights = heightmap_heights(packed, 256);
    let index = usize::from(lx) + usize::from(lz) * 16;
    Ok(match heights.get(index).copied() {
        Some(h) if h > 0 => Some(h as i32 - 1 + min_y),
        _ => None,
    })
}

/// Heightmap bit width: enough for `max_y - min_y + 1` up to 512 levels.
const HEIGHTMAP_BITS: usize = 9;

/// Decode a stored heightmap's 16x16 column values.
///
/// Heightmaps are 9-bit values, and 9 does not divide 64, so the two
/// documented bit-storage layouts need different long counts: spanning
/// (`ceil(256*9/64) = 36` longs) and non-spanning, where each long holds 7
/// values and wastes the remaining 6 bits (`ceil(256/7) = 37` longs).
/// Observed in the owner's 26.3 world (30 September 2026): every stored
/// heightmap is the 37-long non-spanning form, and decoding it as spanning
/// reads correct values only for the first seven columns. The long count
/// selects the layout because the two counts never coincide.
fn heightmap_heights(data: &[i64], count: usize) -> Vec<u32> {
    let non_spanning_len = count.div_ceil(64 / HEIGHTMAP_BITS);
    if data.len() == non_spanning_len {
        unpack_non_spanning(data, HEIGHTMAP_BITS, count)
    } else {
        unpack_spanning(data, HEIGHTMAP_BITS, count)
    }
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

/// One stored quart-grid biome cell at its bottom block coordinate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiomeCell {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub name: String,
}

/// Decode the saved 4×4×4 biome cells of a chunk, preserving their Y levels.
/// Missing sections are not scored: the save provides no biome answer there.
pub fn biome_cells(root: &Tag, chunk_x: i32, chunk_z: i32) -> Result<Vec<BiomeCell>, String> {
    let origin_x = chunk_x
        .checked_mul(16)
        .ok_or("chunk X overflows block coordinates")?;
    let origin_z = chunk_z
        .checked_mul(16)
        .ok_or("chunk Z overflows block coordinates")?;
    let mut cells = Vec::new();
    for section in chunk_sections(root)? {
        let section_y = section
            .get("Y")
            .and_then(Tag::as_i32)
            .ok_or("section without Y")?;
        let base_y = section_y
            .checked_mul(16)
            .ok_or("section Y overflows block coordinates")?;
        for by in 0..4 {
            for bz in 0..4 {
                for bx in 0..4 {
                    let y = base_y
                        .checked_add(by * 4)
                        .ok_or("section Y overflows block coordinates")?;
                    let Some(name) = biome_at(section, (bx * 4) as u8, y, (bz * 4) as u8)? else {
                        return Err("saved biome cell has no palette entry".to_owned());
                    };
                    cells.push(BiomeCell {
                        x: origin_x + bx * 4,
                        y,
                        z: origin_z + bz * 4,
                        name,
                    });
                }
            }
        }
    }
    Ok(cells)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BiomeVolumeReport {
    pub chunks: usize,
    pub missing_chunks: usize,
    pub cells: usize,
    pub matches: usize,
    pub unresolved: usize,
    /// 32-block Y band -> (scored cells, exact matches).
    pub bands: std::collections::BTreeMap<i32, (usize, usize)>,
    pub residuals: std::collections::BTreeMap<(String, String), usize>,
}

/// Compare every stored quart cell in an inclusive square of saved chunks
/// against the generated biome at the same bottom block coordinate.
pub fn compare_biome_volume(
    store: &mut RegionStore,
    generator: &VanillaGenerator,
    min_chunk: i32,
    max_chunk: i32,
) -> Result<BiomeVolumeReport, String> {
    if min_chunk > max_chunk || i64::from(max_chunk) - i64::from(min_chunk) > 31 {
        return Err("chunk range must be ordered and at most 32 wide".to_owned());
    }
    let mut report = BiomeVolumeReport::default();
    for chunk_z in min_chunk..=max_chunk {
        for chunk_x in min_chunk..=max_chunk {
            let Some(root) = store.chunk_root(chunk_x, chunk_z)? else {
                report.missing_chunks += 1;
                continue;
            };
            let cells = biome_cells(&root, chunk_x, chunk_z)?;
            if cells.is_empty() {
                report.missing_chunks += 1;
                continue;
            }
            report.chunks += 1;
            for cell in cells {
                report.cells += 1;
                let band = cell.y.div_euclid(32) * 32;
                let entry = report.bands.entry(band).or_default();
                entry.0 += 1;
                let Some(ours) = generator.biome(cell.x, cell.z, cell.y) else {
                    report.unresolved += 1;
                    continue;
                };
                let matched = ours == cell.name;
                report.matches += usize::from(matched);
                entry.1 += usize::from(matched);
                if !matched {
                    *report.residuals.entry((cell.name, ours)).or_default() += 1;
                }
            }
        }
    }
    Ok(report)
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

fn chunk_cell_coords(x: i64, z: i64) -> Result<(i32, i32, u8, u8), String> {
    let chunk_x =
        i32::try_from(x.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
    let chunk_z =
        i32::try_from(z.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
    let lx = (x & 0x0f) as u8;
    let lz = (z & 0x0f) as u8;
    Ok((chunk_x, chunk_z, lx, lz))
}

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
            let (chunk_x, chunk_z, lx, lz) = chunk_cell_coords(x, z)?;
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
    let (chunk_x, chunk_z, lx, lz) = chunk_cell_coords(x, z)?;
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
    /// Selected small-flower names; the legacy report also included pitcher plants.
    Flowers,
    Wildflowers,
    Bushes,
    TallFlowers,
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
            Self::Wildflowers => "wildflowers",
            Self::Bushes => "bushes",
            Self::TallFlowers => "tall_flowers",
            Self::Cactus => "cactus",
            Self::SugarCane => "sugar_cane",
        }
    }

    pub fn all() -> [Self; 36] {
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
            Self::Wildflowers,
            Self::Bushes,
            Self::TallFlowers,
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
        "wildflowers" => CensusFamily::Wildflowers,
        "bush" | "firefly_bush" => CensusFamily::Bushes,
        "rose_bush" | "lilac" | "peony" | "sunflower" | "pitcher_plant" => {
            CensusFamily::TallFlowers
        }
        "cactus" => CensusFamily::Cactus,
        "sugar_cane" => CensusFamily::SugarCane,
        "dandelion" | "poppy" | "blue_orchid" | "allium" | "azure_bluet" | "red_tulip"
        | "orange_tulip" | "white_tulip" | "pink_tulip" | "oxeye_daisy" | "cornflower"
        | "lily_of_the_valley" | "wither_rose" | "torchflower" => CensusFamily::Flowers,
        other => {
            if other.ends_with("_log")
                || matches!(
                    other,
                    "crimson_stem"
                        | "warped_stem"
                        | "stripped_crimson_stem"
                        | "stripped_warped_stem"
                )
                || other.ends_with("hyphae")
            {
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
        let mut counts = vec![0usize; palette.len()];
        let mut valid = true;
        for slot in slots {
            let Some(entry) = usize::try_from(slot)
                .ok()
                .and_then(|idx| counts.get_mut(idx))
            else {
                valid = false;
                break;
            };
            *entry += 1;
        }
        if valid {
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
        let y_min = section
            .get("Y")
            .and_then(Tag::as_i32)
            .ok_or("section without Y")?
            .checked_mul(16)
            .ok_or("section Y overflows block coordinates")?;
        let band = y_min.div_euclid(32) * 32;
        for (name, count) in section_name_counts(section)? {
            if let Some(family) = census_family(base_block_name(&name)) {
                *self.totals.entry(family).or_default() += count;
                *self.bands.entry((family, band)).or_default() += count;
            }
        }
        Ok(())
    }

    fn record_chunk(&mut self, root: &Tag) -> Result<bool, String> {
        let sections = chunk_sections(root)?;
        if sections.is_empty() {
            return Ok(false);
        }
        for section in sections {
            self.record_section(section)?;
        }
        self.chunks += 1;
        Ok(true)
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
            let sections =
                chunk_sections(&root).map_err(|e| format!("chunk ({chunk_x}, {chunk_z}): {e}"))?;
            if sections.is_empty() {
                continue;
            }
            for section in sections {
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
                Some(root) => {
                    let scored = census
                        .record_chunk(&root)
                        .map_err(|e| format!("chunk ({chunk_x}, {chunk_z}): {e}"))?;
                    if !scored {
                        census.missing_chunks += 1;
                    }
                }
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
            let (chunk_x, chunk_z, lx, lz) = chunk_cell_coords(x, z)?;
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
mod tests;
