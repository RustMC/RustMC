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
            let (columns, missing_chunks) =
                oracle::sample_columns(&mut store, min, max, min, max, stride)?;
            let source = build_source(seed, args.get(6), args.get(7))?;
            let cap = args
                .get(8)
                .map(|v| parse_i64(v).map(|n| n.max(0) as usize))
                .transpose()?
                .unwrap_or(20);
            let report = oracle::compare_columns(columns.iter().cloned(), &*source, cap);
            println!("columns={} missing_chunks={missing_chunks}", report.columns);
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
            let (profiles, missing_chunks) =
                oracle::sample_profiles(&mut store, min, max, min, max, stride)?;
            let source = build_source(seed, args.get(6), args.get(7))?;
            let cap = args
                .get(8)
                .map(|v| parse_i64(v).map(|n| n.max(0) as usize))
                .transpose()?
                .unwrap_or(20);
            let report = oracle::compare_substance(profiles, &*source, cap);
            println!("columns={} missing_chunks={missing_chunks}", report.columns);
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
