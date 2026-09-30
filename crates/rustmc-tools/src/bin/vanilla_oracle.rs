//! Vanilla oracle (T0): read ground truth from an owner-provided 26.3 world
//! save and report how far RustMC's current generator is from it.
//!
//! usage:
//!
//! ```text
//! vanilla_oracle inspect <world-dir> [X Z ...]
//! vanilla_oracle worksheet <world-dir> <seed> [preview|experimental]
//! vanilla_oracle compare <world-dir> <seed> <min> <max> <stride> [preview|experimental]
//! ```
//!
//! The world directory is a single-player save root (contains region/).
//! Nothing from the save is copied into the repository; only aggregate
//! numbers and per-column public facts (height, biome, block name) are
//! printed.

use std::path::Path;

use rustmc_server::world::{Generator, Terrain};
use rustmc_tools::oracle;
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
        return Err("usage: vanilla_oracle inspect|worksheet|compare <world-dir> ...".to_string());
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
            let seed = parse_u64(args.get(2).ok_or("missing <seed>")?)?;
            let points = oracle::worksheet_columns();
            println!(
                "x,z,vanilla_surface_y,vanilla_top_block,vanilla_biome,rustmc_height,rustmc_biome"
            );
            let terrain = parse_terrain(args.get(3))?;
            let generator = Generator::with_terrain(seed, terrain);
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
                    "{x},{z},{sy},{tb},{bi},{},{}",
                    generator.height(x, z),
                    generator.biome(x, z).identifier()
                );
            }
        }
        "compare" => {
            let seed = parse_u64(args.get(2).ok_or("missing <seed>")?)?;
            let min = parse_i64(args.get(3).ok_or("missing <min>")?)?;
            let max = parse_i64(args.get(4).ok_or("missing <max>")?)?;
            let stride = parse_i64(args.get(5).ok_or("missing <stride>")?)?;
            let terrain = parse_terrain(args.get(6))?;
            if min > max {
                return Err("min must not exceed max".to_string());
            }
            let (columns, missing_chunks) =
                oracle::sample_columns(&mut store, min, max, min, max, stride)?;
            let generator = Generator::with_terrain(seed, terrain);
            let report = oracle::compare_columns(columns.iter().cloned(), &generator, 20);
            println!("columns={} missing_chunks={missing_chunks}", report.columns);
            println!(
                "height_exact={} ({:.2}%) biome={} ({:.2}%)",
                report.height_matches,
                oracle::percent(report.height_matches, report.columns),
                report.biome_matches,
                oracle::percent(report.biome_matches, report.columns)
            );
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
        other => return Err(format!("unknown mode {other:?}")),
    }
    Ok(())
}

fn parse_terrain(value: Option<&String>) -> Result<Terrain, String> {
    match value.map(String::as_str) {
        None | Some("preview") => Ok(Terrain::Preview),
        Some("experimental") => Ok(Terrain::Experimental),
        Some(other) => Err(format!(
            "unknown terrain {other:?}; use preview or experimental"
        )),
    }
}

fn parse_i64(value: &str) -> Result<i64, String> {
    value
        .parse::<i64>()
        .map_err(|_| format!("invalid integer: {value}"))
}

fn parse_u64(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid non-negative integer: {value}"))
}
