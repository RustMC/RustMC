//! Block-identity comparison: cave context, per-block reports, coverage
//! accounting and the scored-source label.

use super::*;

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
pub(super) fn void_distances(flags: &[bool]) -> Vec<u32> {
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
    pub(super) fn shortfall_reason(self) -> String {
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
    pub(super) fn shortfall(self) -> String {
        format!(
            "{}; a partial population is not comparable with a published baseline, and this mode exits non-zero unless the run is complete (rerun with --allow-partial to accept a partial measurement)",
            self.shortfall_reason()
        )
    }

    /// The one shortfall `--allow-partial` does not rescue: nothing at all was
    /// scored, so there is no partial measurement to read.
    pub(super) fn empty(self) -> String {
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
