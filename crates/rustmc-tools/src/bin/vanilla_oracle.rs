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
//! vanilla_oracle census <world-dir> <min> <max>
//! vanilla_oracle census_names <world-dir> <min> <max> [cap]
//! vanilla_oracle column <world-dir> <seed> <x> <z> [vanilla[:settings-id] [data-root]]
//! ```
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
use rustmc_tools::oracle::{self, ColumnSource};
use rustmc_tools::region::RegionStore;

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

fn run(args: &[String]) -> Result<(), String> {
    let Some(mode) = args.first().map(String::as_str) else {
        return Err(
            "usage: vanilla_oracle inspect|worksheet|compare|substance|column <world-dir> ..."
                .to_string(),
        );
    };
    let world = Path::new(args.get(1).ok_or("missing <world-dir>")?);
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
            let source = build_source(seed, args.get(3), args.get(4))?;
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
            let source = build_source(seed, args.get(6), args.get(7))?;
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
            let source = build_source(seed, args.get(6), args.get(7))?;
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
            let per_axis = (max - min)
                .checked_div(stride)
                .and_then(|n| n.checked_add(1))
                .ok_or("invalid sample grid")?;
            let grid_points = usize::try_from(
                per_axis
                    .checked_mul(per_axis)
                    .ok_or("invalid sample grid")?,
            )
            .map_err(|_| "sample grid too large".to_string())?;
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
        }
        other => return Err(format!("unknown mode {other:?}")),
    }
    Ok(())
}

/// Builds the RustMC-side column source. `vanilla` mode takes an optional
/// `:settings-id` suffix (default the overworld) and reads the datapack
/// root from the trailing argument, `$RUSTMC_VANILLA_DATA`, or the
/// project-local default.
fn build_source(
    seed: i64,
    terrain: Option<&String>,
    data_root: Option<&String>,
) -> Result<Box<dyn ColumnSource>, String> {
    let value = terrain.map(String::as_str);
    match value {
        None | Some("preview") => Ok(Box::new(Generator::with_terrain(
            seed as u64,
            Terrain::Preview,
        ))),
        Some("experimental") => Ok(Box::new(Generator::with_terrain(
            seed as u64,
            Terrain::Experimental,
        ))),
        Some(t) if t == "vanilla" || t.starts_with("vanilla:") => {
            let settings = t
                .split_once(':')
                .map_or("minecraft:overworld", |(_, id)| id);
            Ok(Box::new(
                VanillaGenerator::new(&resolve_data_root(data_root), seed, settings)
                    .map_err(|error| error.to_string())?,
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
