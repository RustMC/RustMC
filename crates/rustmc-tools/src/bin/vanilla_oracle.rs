//! Vanilla oracle (T0): read ground truth from an owner-provided 26.3 world
//! save and report how far RustMC's current generator is from it.
//!
//! usage:
//!
//! ```text
//! vanilla_oracle inspect <world-dir> [X Z ...]
//! vanilla_oracle worksheet <world-dir> <seed> [preview|experimental|vanilla[:settings-id] [data-root]]
//! vanilla_oracle compare <world-dir> <seed> <min> <max> <stride> [preview|experimental|vanilla[:settings-id] [data-root] [mismatch-cap]]
//! vanilla_oracle substance <world-dir> <seed> <min> <max> <stride> [preview|experimental|vanilla[:settings-id] [data-root] [fail-cap]]
//! vanilla_oracle block_compare <world-dir> <seed> <min> <max> <stride> [data-root [fail-cap]]
//! vanilla_oracle biome_volume <world-dir> <seed> <min-chunk> <max-chunk> [data-root]
//! vanilla_oracle census <world-dir> <min> <max>
//! vanilla_oracle census_names <world-dir> <min> <max> [cap]
//! vanilla_oracle census_compare <world-dir> <seed> <min> <max> [vanilla[:settings-id]] [data-root]
//! vanilla_oracle column <world-dir> <seed> <x> <z> [vanilla[:settings-id] [data-root]]
//! ```
//!
//! `--allow-partial` may be given in any mode above; it is the only flag.
//!
//! Measurement guards. Every mode that takes a `<seed>` first reads that
//! save's own seed from `level.dat` plus `data/minecraft/world_gen_settings.dat`
//! (see `rustmc_tools::save`) and exits non-zero when the argument differs,
//! naming both values: the oracle can only measure the save it is pointed at,
//! and an unreadable `level.dat` is an error rather than a skipped check. That
//! reader also carries the version pin it inherited from `inspect_vanilla_save`:
//! a save whose stored `Version.Name` is not `26.3` is refused outright, so a
//! version is never measured silently. What the attestation does *not* compare
//! is the dimension and the active data packs.
//!
//! Every scoring mode then prints exactly one machine-readable denominator
//! line, `coverage scored=N expected=M missing=K unaccounted=U source=S`, on
//! standard error next to the report it qualifies, and exits non-zero unless
//! the run is complete (`scored == expected`, `missing == 0`,
//! `unaccounted == 0`). It goes to stderr deliberately: the report itself stays
//! byte-for-byte the published measurement, so the recorded `substance` md5 and
//! the line-for-line `block_compare` comparisons in `docs/PROVENANCE.md` remain
//! reproducible. `expected` is
//! the sample grid or chunk square the mode asked the save for, `missing` the
//! units the save could not supply (absent chunks, ungenerated columns,
//! declined or out-of-domain columns), `unaccounted` anything else unexplained,
//! and `source` names the generator that produced the RustMC-side answers:
//! `vanilla`, `preview-not-vanilla-generator`,
//! `experimental-not-vanilla-generator`, or `save-only` when nothing is scored
//! against us. The `preview` and `experimental` terrain tokens legitimately
//! answer no per-position substance at all, so their zero is labelled instead
//! of being comparable; their exit status is unchanged.
//!
//! `--allow-partial` downgrades only the non-zero exit: the coverage line is
//! printed identically, followed by a `warning:` line, both on stderr. It exists
//! for exploratory runs on a save that is still generating, and its output is
//! not comparable with a published baseline. One shortfall it does not rescue: a
//! run that scored *nothing* — the save holds none of the units asked for — exits
//! non-zero with or without the flag, because zeros over an empty sample are
//! exactly the error-shaped result this guard exists to catch.
//!
//! The world directory is a single-player save root (contains region/).
//! Nothing from the save is copied into the repository; only aggregate
//! numbers and per-column public facts (height, biome, block name) are
//! printed. The `vanilla` terrain mode samples the data-driven density
//! pipeline from an operator-provisioned worldgen datapack root (second
//! trailing argument, else `$RUSTMC_VANILLA_DATA`, else
//! `.rustmc-local/vanilla-data`); those data files are likewise never
//! committed.

use std::path::{Path, PathBuf};

use rustmc_server::vanilla::aquifer::Substance;
use rustmc_server::vanilla::generator::VanillaGenerator;
use rustmc_server::world::{Generator, Terrain};
use rustmc_tools::oracle::{self, ColumnSource, Coverage, CoverageOutcome, ScoredSource};
use rustmc_tools::region::RegionStore;
use rustmc_tools::save;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => {}
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
    }
}

/// Modes whose second positional argument is the world seed. Each of them is
/// attested against the save's stored seed before any measurement runs.
const SEED_MODES: [&str; 7] = [
    "worksheet",
    "compare",
    "substance",
    "block_compare",
    "biome_volume",
    "census_compare",
    "column",
];

fn run(raw_args: &[String]) -> Result<(), String> {
    let allow_partial = raw_args.iter().any(|arg| arg == "--allow-partial");
    let args: Vec<String> = raw_args
        .iter()
        .filter(|arg| *arg != "--allow-partial")
        .cloned()
        .collect();
    if let Some(flag) = args.iter().find(|arg| arg.starts_with("--")) {
        return Err(format!(
            "unknown flag {flag}; the only flag this tool accepts is --allow-partial"
        ));
    }
    let Some(mode) = args.first().map(String::as_str) else {
        return Err(
            "usage: vanilla_oracle inspect|worksheet|compare|substance|block_compare|biome_volume|census|census_compare|census_names|column <world-dir> ..."
                .to_string(),
        );
    };
    let world = Path::new(args.get(1).ok_or("missing <world-dir>")?);
    if SEED_MODES.contains(&mode) {
        attest_seed(world, parse_i64(args.get(2).ok_or("missing <seed>")?)?)?;
    }
    let mut store = RegionStore::new(world);
    match mode {
        "inspect" => {
            let mut points = Vec::new();
            let rest = &args[2..];
            if !rest.len().is_multiple_of(2) {
                return Err("inspect needs coordinate pairs".to_string());
            }
            for pair in rest.chunks(2) {
                points.push((parse_i64(&pair[0])?, parse_i64(&pair[1])?));
            }
            println!("x,z,surface_y,top_block,biome");
            for (x, z, result) in oracle::read_columns(&mut store, &points)? {
                match result? {
                    None => println!("{x},{z},<not generated>,<none>,<none>"),
                    Some(c) => println!(
                        "{},{},{},{},{}",
                        c.x,
                        c.z,
                        c.surface_y,
                        c.top_block.unwrap_or_else(|| "<unreadable>".into()),
                        c.biome.unwrap_or_else(|| "<unreadable>".into())
                    ),
                }
            }
        }
        "worksheet" => {
            let seed = parse_i64(args.get(2).ok_or("missing <seed>")?)?;
            let points = oracle::worksheet_columns();
            println!(
                "x,z,vanilla_surface_y,vanilla_top_block,vanilla_biome,rustmc_height,rustmc_biome,rustmc_top_block"
            );
            let (source, origin) = build_source(seed, args.get(3), args.get(4))?;
            for (x, z, result) in oracle::read_columns(&mut store, &points)? {
                let (sy, tb, bi) = match result? {
                    None => return Err(format!("worksheet point ({x}, {z}) is not in the save")),
                    Some(c) => (
                        c.surface_y,
                        c.top_block.unwrap_or_else(|| "<unreadable>".into()),
                        c.biome.unwrap_or_else(|| "<unreadable>".into()),
                    ),
                };
                println!(
                    "{x},{z},{sy},{tb},{bi},{},{},{}",
                    source.column_height(x, z),
                    source.column_biome(x, z),
                    source.column_top_block(x, z)
                );
            }
            report_coverage(
                Coverage::new(points.len(), points.len(), 0, origin),
                allow_partial,
            )?;
        }
        "compare" => {
            let seed = parse_i64(args.get(2).ok_or("missing <seed>")?)?;
            let min = parse_i64(args.get(3).ok_or("missing <min>")?)?;
            let max = parse_i64(args.get(4).ok_or("missing <max>")?)?;
            let stride = parse_i64(args.get(5).ok_or("missing <stride>")?)?;
            if min > max {
                return Err("min must not exceed max".to_string());
            }
            let (columns, gaps) = oracle::sample_columns(&mut store, min, max, min, max, stride)?;
            let (source, origin) = build_source(seed, args.get(6), args.get(7))?;
            let cap = args
                .get(8)
                .map(|v| parse_i64(v).map(|n| n.max(0) as usize))
                .transpose()?
                .unwrap_or(20);
            let report = oracle::compare_columns(columns.iter().cloned(), &*source, cap);
            println!(
                "columns={} missing_chunks={} ungenerated_columns={}",
                report.columns, gaps.missing_chunks, gaps.ungenerated_columns
            );
            report_coverage(
                Coverage::new(
                    report.columns,
                    grid_square(min, max, stride)?,
                    gaps.total(),
                    origin,
                ),
                allow_partial,
            )?;
            println!(
                "height_exact={} ({:.2}%) biome={} ({:.2}%)",
                report.height_matches,
                oracle::percent(report.height_matches, report.columns),
                report.biome_matches,
                oracle::percent(report.biome_matches, report.columns)
            );
            println!(
                "topblock_on_height_matched={} ({:.2}%)",
                report.topblock_matches,
                oracle::percent(report.topblock_matches, report.height_matches)
            );
            let mut pairs: Vec<_> = report.topblock_residuals.iter().collect();
            pairs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            for ((vanilla, rustmc), count) in pairs.iter().take(15) {
                println!("topblock_residual {count}x vanilla={vanilla} rustmc={rustmc}");
            }
            println!("x,z,vanilla_surface_y,vanilla_biome,rustmc_height,rustmc_biome");
            for m in &report.mismatches {
                println!(
                    "{},{},{},{},{},{}",
                    m.x,
                    m.z,
                    m.vanilla_surface_y,
                    m.vanilla_biome.as_deref().unwrap_or("<unreadable>"),
                    m.rustmc_height,
                    m.rustmc_biome
                );
            }
            if report.columns == 0 {
                return Err("no generated chunks found in the sampled range".to_string());
            }
        }
        "substance" => {
            let seed = parse_i64(args.get(2).ok_or("missing <seed>")?)?;
            let min = parse_i64(args.get(3).ok_or("missing <min>")?)?;
            let max = parse_i64(args.get(4).ok_or("missing <max>")?)?;
            let stride = parse_i64(args.get(5).ok_or("missing <stride>")?)?;
            if min > max {
                return Err("min must not exceed max".to_string());
            }
            let (profiles, gaps) = oracle::sample_profiles(&mut store, min, max, min, max, stride)?;
            let (source, origin) = build_source(seed, args.get(6), args.get(7))?;
            let cap = args
                .get(8)
                .map(|v| parse_i64(v).map(|n| n.max(0) as usize))
                .transpose()?
                .unwrap_or(20);
            let report = oracle::compare_substance(profiles, &*source, cap);
            println!(
                "columns={} missing_chunks={} ungenerated_columns={}",
                report.columns, gaps.missing_chunks, gaps.ungenerated_columns
            );
            report_coverage(
                Coverage::new(
                    report.columns,
                    grid_square(min, max, stride)?,
                    gaps.total(),
                    origin,
                ),
                allow_partial,
            )?;
            println!(
                "substance_positions={} exact={} ({:.2}%)",
                report.positions,
                report.matches,
                oracle::percent(report.matches, report.positions)
            );
            for (label, positions, matches) in [
                (
                    "near_surface_0_7",
                    report.near_positions,
                    report.near_matches,
                ),
                (
                    "middle_8_63",
                    report.middle_positions,
                    report.middle_matches,
                ),
                ("deep_64_plus", report.deep_positions, report.deep_matches),
            ] {
                println!(
                    "band {label}: positions={positions} exact={matches} ({:.2}%)",
                    oracle::percent(matches, positions)
                );
            }
            if report.positions == 0 {
                println!(
                    "note: the selected source answers no per-position substance (pending T3)."
                );
            }
            let mut pairs: Vec<_> = report.residuals.iter().collect();
            pairs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            for ((vanilla, rustmc), count) in pairs.iter().take(10) {
                println!(
                    "substance_residual {count}x vanilla={} rustmc={}",
                    vanilla.name(),
                    rustmc.name()
                );
            }
            // Depth profile of the two dominant residual shapes.
            for (from, to) in pairs.iter().take(2).map(|((from, to), _)| (*from, *to)) {
                let mut bands: Vec<_> = report
                    .residual_bands
                    .iter()
                    .filter(|((pair, _), _)| *pair == (from, to))
                    .map(|((_, band), count)| (*band, *count))
                    .collect();
                bands.sort_by_key(|(band, _)| *band);
                let name = format!("{}_to_{}", from.name(), to.name());
                let rendered: Vec<String> = bands
                    .iter()
                    .map(|(band, count)| format!("{band}:{count}"))
                    .collect();
                println!("bands {name} {}", rendered.join(" "));
            }
            for (band, stats) in &report.marginals {
                println!(
                    "marginal y={band}: vanilla air={} fluid={} solid={} | rustmc air={} fluid={} solid={}",
                    stats.vanilla[0],
                    stats.vanilla[1],
                    stats.vanilla[2],
                    stats.rustmc[0],
                    stats.rustmc[1],
                    stats.rustmc[2]
                );
            }
            println!("fail_x,fail_z,fail_y");
            for (x, z, y) in &report.fail_positions {
                println!("{x},{z},{y}");
            }
            if report.columns == 0 {
                return Err("no generated chunks found in the sampled range".to_string());
            }
        }
        "block_compare" => {
            let seed = parse_i64(args.get(2).ok_or("missing <seed>")?)?;
            let min = parse_i64(args.get(3).ok_or("missing <min>")?)?;
            let max = parse_i64(args.get(4).ok_or("missing <max>")?)?;
            let stride = parse_i64(args.get(5).ok_or("missing <stride>")?)?;
            if min > max {
                return Err("min must not exceed max".to_string());
            }
            let (profiles, gaps) = oracle::sample_profiles(&mut store, min, max, min, max, stride)?;
            let generator =
                VanillaGenerator::new(&resolve_data_root(args.get(6)), seed, "minecraft:overworld")
                    .map_err(|error| error.to_string())?;
            let cap = args
                .get(7)
                .map(|v| parse_i64(v).map(|n| n.max(0) as usize))
                .transpose()?
                .unwrap_or(20);
            let report = oracle::compare_block_ids(profiles, &generator, cap);
            // M3 ground truth: the requested sample size and every grid
            // point that could not be scored are printed before any
            // percentage, so the denominator is never implied.
            let grid_points = grid_square(min, max, stride)?;
            let unaccounted = grid_points
                .saturating_sub(report.columns)
                .saturating_sub(gaps.total())
                .saturating_sub(report.out_of_domain_columns)
                .saturating_sub(report.declined_columns);
            println!("requested={min}..{max} stride={stride} grid_points={grid_points}");
            println!(
                "columns={} missing_chunks={} ungenerated_columns={} declined_columns={} out_of_domain_columns={} unaccounted_grid_points={} out_of_range_rows={}",
                report.columns,
                gaps.missing_chunks,
                gaps.ungenerated_columns,
                report.declined_columns,
                report.out_of_domain_columns,
                unaccounted,
                report.out_of_range_rows
            );
            report_coverage(
                Coverage {
                    scored: report.columns,
                    expected: grid_points,
                    missing: gaps.total() + report.declined_columns + report.out_of_domain_columns,
                    unaccounted,
                    source: ScoredSource::Vanilla,
                },
                allow_partial,
            )?;
            println!(
                "base_block_positions={} exact={} ({:.2}%) air_family_normalised_exact={} ({:.2}%)",
                report.positions,
                report.exact_base,
                oracle::percent(report.exact_base, report.positions),
                report.exact_base_air_family,
                oracle::percent(report.exact_base_air_family, report.positions)
            );
            for (band, agreement) in &report.bands {
                println!(
                    "band y={band}: positions={} exact={} ({:.2}%) air_family_exact={} ({:.2}%)",
                    agreement.positions,
                    agreement.matches,
                    oracle::percent(agreement.matches, agreement.positions),
                    agreement.matches_air_family,
                    oracle::percent(agreement.matches_air_family, agreement.positions)
                );
            }
            for (context, agreement) in &report.cave {
                println!(
                    "cave {}: positions={} exact={} ({:.2}%) air_family_exact={} ({:.2}%)",
                    context.name(),
                    agreement.positions,
                    agreement.matches,
                    oracle::percent(agreement.matches, agreement.positions),
                    agreement.matches_air_family,
                    oracle::percent(agreement.matches_air_family, agreement.positions)
                );
            }
            println!(
                "save_void_ours_block={} save_block_ours_air={} of which near_saved_void={}",
                report.missed_carves, report.extra_carves, report.extra_carves_near_void
            );
            for (label, bands) in [
                ("missed", &report.missed_carves_bands),
                ("extra", &report.extra_carves_bands),
            ] {
                let rendered: Vec<String> = bands
                    .iter()
                    .map(|(band, count)| format!("{band}:{count}"))
                    .collect();
                println!("carve_bands {label} {}", rendered.join(" "));
            }
            let mut pairs: Vec<_> = report.residuals.iter().collect();
            pairs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            for ((saved, generated), count) in pairs.into_iter().take(20) {
                println!("residual {count}x vanilla={saved} rustmc={generated}");
            }
            let mut pairs: Vec<_> = report.residuals_air_family.iter().collect();
            pairs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            for ((saved, generated), count) in pairs.into_iter().take(20) {
                println!("residual_air_norm {count}x vanilla={saved} rustmc={generated}");
            }
            let mut pairs: Vec<_> = report.residuals_cave.iter().collect();
            pairs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            for ((saved, generated), count) in pairs.into_iter().take(10) {
                println!("residual_cave {count}x vanilla={saved} rustmc={generated}");
            }
            println!("fail_x,fail_z,fail_y");
            for (x, z, y) in &report.fail_positions {
                println!("{x},{z},{y}");
            }
            if report.columns == 0 {
                return Err("no generated chunks found in the sampled range".to_string());
            }
        }
        "biome_volume" => {
            let seed = parse_i64(args.get(2).ok_or("missing <seed>")?)?;
            let min = parse_i64(args.get(3).ok_or("missing <min-chunk>")?)?
                .try_into()
                .map_err(|_| "min chunk does not fit i32")?;
            let max = parse_i64(args.get(4).ok_or("missing <max-chunk>")?)?
                .try_into()
                .map_err(|_| "max chunk does not fit i32")?;
            let generator =
                VanillaGenerator::new(&resolve_data_root(args.get(5)), seed, "minecraft:overworld")
                    .map_err(|error| error.to_string())?;
            let report = oracle::compare_biome_volume(&mut store, &generator, min, max)?;
            if report.chunks == 0 {
                return Err("no generated chunks found in the sampled range".to_owned());
            }
            println!(
                "chunks={} missing_chunks={} cells={} exact={} ({:.2}%) unresolved={}",
                report.chunks,
                report.missing_chunks,
                report.cells,
                report.matches,
                oracle::percent(report.matches, report.cells),
                report.unresolved
            );
            report_coverage(
                Coverage::new(
                    report.chunks,
                    chunk_square_of(min, max)?,
                    report.missing_chunks,
                    ScoredSource::Vanilla,
                ),
                allow_partial,
            )?;
            for (band, (cells, matches)) in report.bands {
                println!(
                    "band_y={band} cells={cells} exact={matches} ({:.2}%)",
                    oracle::percent(matches, cells)
                );
            }
            let mut residuals: Vec<_> = report.residuals.into_iter().collect();
            residuals
                .sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
            for ((saved, generated), count) in residuals.into_iter().take(15) {
                println!("residual {count}x vanilla={saved} rustmc={generated}");
            }
        }
        "census" => {
            let min = parse_i64(args.get(2).ok_or("missing <min>")?)?;
            let max = parse_i64(args.get(3).ok_or("missing <max>")?)?;
            if min > max {
                return Err("min must not exceed max".to_string());
            }
            // T4 baseline: the save's decoration census (ore features
            // and vegetation families) over the chunk rectangle covering
            // the block range. Counts include structure-placed blocks;
            // the current pipeline places no features, so every counted
            // family is a quantified T4 target, not a scored mismatch.
            let census = oracle::census_blocks(&mut store, min, max)?;
            println!(
                "chunks={} missing_chunks={}",
                census.chunks, census.missing_chunks
            );
            report_coverage(
                Coverage::new(
                    census.chunks,
                    chunk_square(min, max)?,
                    census.missing_chunks,
                    ScoredSource::Save,
                ),
                allow_partial,
            )?;
            for family in oracle::CensusFamily::all() {
                let total = census.totals.get(&family).copied().unwrap_or(0);
                if total == 0 {
                    continue;
                }
                let mut band_pairs: Vec<_> = census
                    .bands
                    .iter()
                    .filter(|((f, _), _)| *f == family)
                    .map(|((_, band), count)| (*band, *count))
                    .collect();
                band_pairs.sort_by_key(|(band, _)| *band);
                let rendered: Vec<String> = band_pairs
                    .iter()
                    .map(|(band, count)| format!("{band}:{count}"))
                    .collect();
                println!(
                    "census {} total={} bands {}",
                    family.name(),
                    total,
                    rendered.join(" ")
                );
            }
            if census.chunks == 0 {
                return Err("no generated chunks found in the sampled range".to_string());
            }
        }
        "census_compare" => {
            let seed = parse_i64(args.get(2).ok_or("missing <seed>")?)?;
            let min = parse_i64(args.get(3).ok_or("missing <min>")?)?;
            let max = parse_i64(args.get(4).ok_or("missing <max>")?)?;
            if min > max {
                return Err("min must not exceed max".to_string());
            }
            // T4 comparison: the same census computed from RustMC's full
            // material-rule descent against the save's stored blocks over
            // the inclusive block square. Feature-placed families (coal,
            // diamond, vegetation) still read rustmc=0 until the feature
            // runtime lands.
            let terrain = args.get(5).cloned().unwrap_or_else(|| "vanilla".to_owned());
            if terrain != "vanilla" && !terrain.starts_with("vanilla:") {
                return Err("census_compare requires the vanilla surface pipeline".to_string());
            }
            let settings = terrain
                .split_once(':')
                .map_or("minecraft:overworld", |(_, id)| id);
            let generator = VanillaGenerator::new(&resolve_data_root(args.get(6)), seed, settings)
                .map_err(|error| error.to_string())?;
            let save = oracle::census_blocks(&mut store, min, max)?;
            let ours = oracle::census_generated(&generator, min, max)?;
            println!(
                "region={min}..{max} save_chunks={} missing_chunks={} rustmc_columns={}",
                save.chunks,
                save.missing_chunks,
                (max - min + 1) * (max - min + 1)
            );
            report_coverage(
                Coverage::new(
                    save.chunks,
                    chunk_square(min, max)?,
                    save.missing_chunks,
                    ScoredSource::Vanilla,
                ),
                allow_partial,
            )?;
            for family in oracle::CensusFamily::all() {
                let save_total = save.totals.get(&family).copied().unwrap_or(0);
                let ours_total = ours.totals.get(&family).copied().unwrap_or(0);
                if save_total == 0 && ours_total == 0 {
                    continue;
                }
                println!(
                    "compare {} save={} rustmc={}",
                    family.name(),
                    save_total,
                    ours_total
                );
                if save_total > 0 && ours_total > 0 {
                    for (label, census) in [("save", &save), ("rustmc", &ours)] {
                        let mut band_pairs: Vec<_> = census
                            .bands
                            .iter()
                            .filter(|((f, _), _)| *f == family)
                            .map(|((_, band), count)| (*band, *count))
                            .collect();
                        band_pairs.sort_by_key(|(band, _)| *band);
                        let rendered: Vec<String> = band_pairs
                            .iter()
                            .map(|(band, count)| format!("{band}:{count}"))
                            .collect();
                        println!("bands {label} {} {}", family.name(), rendered.join(" "));
                    }
                }
            }
            if save.chunks == 0 {
                return Err("no generated chunks found in the sampled range".to_string());
            }
        }
        "census_names" => {
            let min = parse_i64(args.get(2).ok_or("missing <min>")?)?;
            let max = parse_i64(args.get(3).ok_or("missing <max>")?)?;
            if min > max {
                return Err("min must not exceed max".to_string());
            }
            // Classification audit: every stored block-state name in the
            // rectangle with its count and the census family it maps to
            // (`none` when the census ignores it).
            let to_chunk = |v: i64| -> Result<i32, String> {
                i32::try_from(v.div_euclid(16)).map_err(|_| "coordinate too large".to_string())
            };
            let (min_cx, max_cx) = (to_chunk(min)?, to_chunk(max)?);
            let (chunks, names) =
                oracle::census_name_histogram(&mut store, min_cx, max_cx, min_cx, max_cx)?;
            println!("chunks={chunks}");
            let expected = chunk_square_of(min_cx, max_cx)?;
            report_coverage(
                Coverage::new(
                    chunks,
                    expected,
                    expected.saturating_sub(chunks),
                    ScoredSource::Save,
                ),
                allow_partial,
            )?;
            let mut pairs: Vec<_> = names.iter().collect();
            pairs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            let cap = args
                .get(4)
                .map(|v| parse_i64(v).map(|n| n.max(1) as usize))
                .transpose()?
                .unwrap_or(pairs.len());
            for (name, count) in pairs.iter().take(cap) {
                let family = oracle::census_family(oracle::base_block_name(name))
                    .map(|f| f.name().to_owned())
                    .unwrap_or_else(|| "none".to_owned());
                println!("name {name} count {count} family {family}");
            }
            if chunks == 0 {
                return Err("no generated chunks found in the sampled range".to_string());
            }
        }
        "column" => {
            let seed = parse_i64(args.get(2).ok_or("missing <seed>")?)?;
            let x = parse_i64(args.get(3).ok_or("missing <x>")?)?;
            let z = parse_i64(args.get(4).ok_or("missing <z>")?)?;
            let terrain = args.get(5).cloned().unwrap_or_else(|| "vanilla".to_owned());
            if terrain != "vanilla" && !terrain.starts_with("vanilla:") {
                return Err("column mode requires the vanilla density pipeline".to_string());
            }
            let settings = terrain
                .split_once(':')
                .map_or("minecraft:overworld", |(_, id)| id);
            let generator = VanillaGenerator::new(&resolve_data_root(args.get(6)), seed, settings)
                .map_err(|error| error.to_string())?;
            let (Ok(x32), Ok(z32)) = (i32::try_from(x), i32::try_from(z)) else {
                return Err("coordinate too large".to_string());
            };
            let profile = oracle::read_profile(&mut store, x, z)?
                .ok_or_else(|| format!("column ({x}, {z}) is not in the save"))?;
            println!("y,vanilla,rustmc,rustmc_density");
            for (offset, vanilla) in profile.categories.iter().copied().enumerate() {
                let y = profile.min_y + offset as i32;
                let density = generator.raw_density(x32, y, z32);
                let rustmc = match generator.substance(x32, y, z32) {
                    Substance::Air => oracle::Category::Air,
                    Substance::Fluid(_) => oracle::Category::Fluid,
                    Substance::Solid => oracle::Category::Solid,
                };
                println!("{y},{},{},{density}", vanilla.name(), rustmc.name());
            }
            report_coverage(Coverage::new(1, 1, 0, ScoredSource::Vanilla), allow_partial)?;
        }
        other => return Err(format!("unknown mode {other:?}")),
    }
    Ok(())
}

/// Compares the command-line `<seed>` with the seed the save stores, so a
/// measurement can never be attributed to the wrong world. Both an unreadable
/// save and a mismatch are errors.
fn attest_seed(world: &Path, argument: i64) -> Result<(), String> {
    let stored = save::seed(world)?;
    save::attest_seed(stored, argument).map_err(|error| format!("{error} ({})", world.display()))
}

/// Prints one mode's coverage line and applies its verdict. The line goes to
/// standard error so a mode's own report stays byte-identical to the
/// measurements already recorded in `docs/PROVENANCE.md`.
fn report_coverage(coverage: Coverage, allow_partial: bool) -> Result<(), String> {
    match coverage.outcome(allow_partial) {
        CoverageOutcome::Complete { line } => {
            eprintln!("{line}");
            Ok(())
        }
        CoverageOutcome::PartialAccepted { line, warning } => {
            eprintln!("{line}");
            eprintln!("warning: {warning}");
            Ok(())
        }
        CoverageOutcome::PartialRejected { line, message } => {
            eprintln!("{line}");
            Err(message)
        }
    }
}

/// Grid points a strided sample of the inclusive block square asks for.
fn grid_square(min: i64, max: i64, stride: i64) -> Result<usize, String> {
    let per_axis = (max - min)
        .checked_div(stride)
        .and_then(|n| n.checked_add(1))
        .ok_or("invalid sample grid")?;
    usize::try_from(
        per_axis
            .checked_mul(per_axis)
            .ok_or("invalid sample grid")?,
    )
    .map_err(|_| "sample grid too large".to_string())
}

/// Chunk square covering the inclusive block range.
fn chunk_square(min: i64, max: i64) -> Result<usize, String> {
    let to_chunk = |v: i64| -> Result<i32, String> {
        i32::try_from(v.div_euclid(16)).map_err(|_| "coordinate too large".to_string())
    };
    chunk_square_of(to_chunk(min)?, to_chunk(max)?)
}

/// Chunk square over an inclusive chunk range.
fn chunk_square_of(min: i32, max: i32) -> Result<usize, String> {
    if min > max {
        return Err("min must not exceed max".to_string());
    }
    let side = i64::from(max) - i64::from(min) + 1;
    usize::try_from(side * side).map_err(|_| "chunk square too large".to_string())
}

/// Builds the RustMC-side column source and reports which terrain model it
/// actually is, so the coverage line can label the number. `vanilla` mode takes
/// an optional `:settings-id` suffix (default the overworld) and reads the
/// datapack root from the trailing argument, `$RUSTMC_VANILLA_DATA`, or the
/// project-local default.
fn build_source(
    seed: i64,
    terrain: Option<&String>,
    data_root: Option<&String>,
) -> Result<(Box<dyn ColumnSource>, ScoredSource), String> {
    let value = terrain.map(String::as_str);
    match value {
        None | Some("preview") => Ok((
            Box::new(Generator::with_terrain(seed as u64, Terrain::Preview)),
            ScoredSource::Preview,
        )),
        Some("experimental") => Ok((
            Box::new(Generator::with_terrain(seed as u64, Terrain::Experimental)),
            ScoredSource::Experimental,
        )),
        Some(t) if t == "vanilla" || t.starts_with("vanilla:") => {
            let settings = t
                .split_once(':')
                .map_or("minecraft:overworld", |(_, id)| id);
            Ok((
                Box::new(
                    VanillaGenerator::new(&resolve_data_root(data_root), seed, settings)
                        .map_err(|error| error.to_string())?,
                ),
                ScoredSource::Vanilla,
            ))
        }
        Some(other) => Err(format!(
            "unknown terrain {other:?}; use preview, experimental, or vanilla[:settings-id]"
        )),
    }
}

/// Datapack root: trailing argument, else `$RUSTMC_VANILLA_DATA`, else the
/// project-local default.
fn resolve_data_root(data_root: Option<&String>) -> PathBuf {
    match data_root {
        Some(path) => PathBuf::from(path),
        None => std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data")),
    }
}

fn parse_i64(value: &str) -> Result<i64, String> {
    value
        .parse::<i64>()
        .map_err(|_| format!("invalid integer: {value}"))
}
