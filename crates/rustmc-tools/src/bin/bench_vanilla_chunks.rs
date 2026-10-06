//! Release-build measurement of `VanillaGenerator` chunk and column work.
//!
//! ```text
//! bench_vanilla_chunks [--mode columns|heights|carve|substance|all ...]
//!                      [--data-root PATH] [--seed N] [--settings ID]
//!                      [--origin-x N] [--origin-z N]
//!                      [--side N] [--repeat N] [--view-radius N]
//! ```
//!
//! Defaults measure the overworld the oracle gates run against: seed 2026, a
//! 4x4 chunk square at chunk origin (0, 0), two passes so the second shows
//! the cost with the coordinate caches warm. Each mode walks the whole
//! square with the documented generator call and reports milliseconds per
//! chunk.
//!
//! Each mode gets a freshly constructed generator: the coordinate caches
//! persist across calls, so sharing one generator between modes would
//! measure cache hits instead of generation. After each mode the binary
//! prints how many entries every coordinate-keyed cache holds, projects the
//! cold (first-pass) measured rate onto one radius-32 chunk view (4,225
//! chunks, 1,081,600 columns), and the last lines state the fixed cache
//! budget and the process peak resident set from `/proc/self/status`
//! (`VmHWM`). A repeat pass is a warm-cache read rate, never a projection
//! basis: a fresh view is cold work.
//!
//! Before/after comparison is deliberately out-of-product: the pre-change
//! tree has no occupancy surface to query, so the "before" numbers in the
//! commit message were taken by extracting the previous commit to a scratch
//! directory and running the same sweep there (`--side 64 --mode carve`
//! matches byte-for-byte carved-block counts between the two trees, which
//! is the eviction-is-recomputation check). This binary measures the
//! bounded state only.
//!
//! The data root is operator-provisioned worldgen data and is never
//! committed; this binary prints aggregate numbers only.

use std::path::PathBuf;
use std::time::Instant;

use rustmc_server::vanilla::aquifer::{NoiseBasedAquifer, Substance};
use rustmc_server::vanilla::generator::{CacheOccupancy, VanillaGenerator};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(message) = run(&args) {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

struct Config {
    data_root: PathBuf,
    seed: i64,
    settings: String,
    origin: (i32, i32),
    side: usize,
    repeat: usize,
    view_radius: i32,
    modes: Vec<String>,
}

fn default_data_root() -> PathBuf {
    std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"))
}

fn integer(field: &str, value: &str) -> Result<i64, String> {
    value
        .parse::<i64>()
        .map_err(|error| format!("bad {field} {value}: {error}"))
}

fn parse(args: &[String]) -> Result<Config, String> {
    let mut config = Config {
        data_root: default_data_root(),
        seed: 2026,
        settings: "minecraft:overworld".to_owned(),
        origin: (0, 0),
        side: 4,
        repeat: 2,
        view_radius: 32,
        modes: vec!["columns".to_owned()],
    };
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let mut take_number = |name: &str| -> Result<i64, String> {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| format!("missing value for {name}"))?;
            integer(name, value)
        };
        match flag {
            "--mode" => {
                let mut modes = Vec::new();
                while let Some(next) = args.get(index + 1) {
                    if next.starts_with("--") {
                        break;
                    }
                    modes.push(next.clone());
                    index += 1;
                }
                if modes.is_empty() {
                    return Err("missing value for --mode".to_owned());
                }
                config.modes = modes;
            }
            "--data-root" => {
                index += 1;
                config.data_root = PathBuf::from(
                    args.get(index)
                        .ok_or("missing value for --data-root".to_owned())?,
                );
            }
            "--settings" => {
                index += 1;
                config.settings = args
                    .get(index)
                    .cloned()
                    .ok_or("missing value for --settings".to_owned())?;
            }
            "--seed" => config.seed = take_number("--seed")?,
            "--origin-x" => config.origin.0 = take_number("--origin-x")? as i32,
            "--origin-z" => config.origin.1 = take_number("--origin-z")? as i32,
            "--side" => config.side = take_number("--side")? as usize,
            "--repeat" => config.repeat = take_number("--repeat")? as usize,
            "--view-radius" => config.view_radius = take_number("--view-radius")? as i32,
            other => return Err(format!("unknown argument {other}")),
        }
        index += 1;
    }
    if config.side == 0 || config.repeat == 0 {
        return Err("--side and --repeat must be positive".to_owned());
    }
    if config.modes.iter().any(|mode| mode == "all") {
        config.modes = ["columns", "heights", "carve", "substance"]
            .iter()
            .map(|mode| (*mode).to_owned())
            .collect();
    }
    Ok(config)
}

/// One field of `/proc/self/status`, in KiB.
fn proc_kib(field: &str) -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    text.lines().find_map(|line| {
        let rest = line.strip_prefix(field)?.strip_prefix(':')?;
        rest.split_whitespace()
            .next()
            .and_then(|value| value.parse::<u64>().ok())
    })
}

fn run(args: &[String]) -> Result<(), String> {
    let config = parse(args)?;
    if !config.data_root.join("data").is_dir() {
        return Err(format!(
            "data root {} has no data/ directory",
            config.data_root.display()
        ));
    }
    let chunks = (config.side * config.side) as f64;
    println!(
        "config,seed,{},settings,{},origin,{},{}",
        config.seed, config.settings, config.origin.0, config.origin.1
    );
    println!(
        "config,data_root,{},side,{}x{},repeat,{}",
        config.data_root.display(),
        config.side,
        config.side,
        config.repeat
    );
    println!("mode,pass,total_ms,ms_per_chunk,ms_per_column,units,units_per_ms");
    let mut mask_bytes = 0;
    for mode in &config.modes {
        // A fresh generator per mode: the coordinate caches persist across
        // calls, so sharing one generator between modes would measure cache
        // hits rather than generation.
        let started = Instant::now();
        let generator = VanillaGenerator::new(&config.data_root, config.seed, &config.settings)
            .map_err(|error| format!("{error}"))?;
        let load_ms = started.elapsed().as_secs_f64() * 1000.0;
        mask_bytes = generator.carve_mask_bytes();
        println!("phase,{mode},load_and_compile_ms,{load_ms:.1}");
        let mut cold_column_ms = 0.0;
        for pass in 1..=config.repeat {
            let started = Instant::now();
            let units = match mode.as_str() {
                "columns" => measure_columns(&generator, &config),
                "heights" => measure_heights(&generator, &config),
                "carve" => measure_carve(&generator, &config),
                "substance" => measure_substance(&generator, &config),
                other => return Err(format!("unknown mode {other}")),
            };
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            let column_ms = total_ms / (chunks * 256.0);
            if pass == 1 {
                cold_column_ms = column_ms;
            }
            println!(
                "{mode},{pass},{total_ms:.1},{per_chunk:.2},{per_column:.4},{units},{throughput:.1}",
                per_chunk = total_ms / chunks,
                per_column = column_ms,
                throughput = units as f64 / total_ms,
            );
        }
        report_occupancy(mode, &generator.cache_occupancy());
        // The projection uses the cold first-pass rate: one radius-32 view
        // is fresh work, so pricing it with a repeat pass that mostly hits
        // the coordinate memos would report seconds for what is hours.
        report_projection(mode, mask_bytes, &config, cold_column_ms);
    }
    report_budget(mask_bytes);
    match (proc_kib("VmHWM"), proc_kib("VmRSS")) {
        (Some(peak), Some(current)) => println!("rss_kib,peak,{peak},current,{current}"),
        _ => println!("rss_kib,peak,unavailable,current,unavailable"),
    }
    Ok(())
}

/// Entries held by every coordinate-keyed cache after one mode's sweep.
fn report_occupancy(mode: &str, occupancy: &CacheOccupancy) {
    println!(
        "occupancy,{mode},masks,{},heights,{},ocean_floor_maps,{},biomes,{},biome_regions,{},chunk_carvers,{}",
        occupancy.masks,
        occupancy.heights,
        occupancy.ocean_floor_maps,
        occupancy.biomes,
        occupancy.biome_regions,
        occupancy.chunk_carvers
    );
    println!(
        "occupancy,{mode},aquifer_centers,{},aquifer_statuses,{},aquifer_surface_levels,{},aquifer_skip_bounds,{},total,{}",
        occupancy.aquifer_centers,
        occupancy.aquifer_statuses,
        occupancy.aquifer_surface_levels,
        occupancy.aquifer_skip_bounds,
        occupancy.total()
    );
}

/// The measured rate applied to one full 32-chunk view: the work limit the
/// streaming path has to respect, plus what the same view would have left
/// cached if the coordinate memos were unbounded.
fn report_projection(mode: &str, mask_bytes: usize, config: &Config, column_ms: f64) {
    let side = i64::from(2 * config.view_radius + 1);
    let view_chunks = side * side;
    let view_columns = view_chunks * 256;
    let source_chunks = (side + 16) * (side + 16);
    let seconds = column_ms * view_columns as f64 / 1000.0;
    println!(
        "projection,{mode},basis,cold_pass1,view_radius,{},view_chunks,{view_chunks},view_columns,{view_columns},view_seconds_single_thread,{seconds:.0},hours_single_thread,{:.2}",
        config.view_radius,
        seconds / 3600.0
    );
    println!(
        "growth,{mode},unbounded_masks_mib,{:.1},unbounded_heights_mib,{:.1},unbounded_source_chunks,{source_chunks}",
        view_chunks as f64 * mask_bytes as f64 / (1024.0 * 1024.0),
        view_columns as f64 * size_of::<((i32, i32), i32)>() as f64 / (1024.0 * 1024.0)
    );
}

/// The fixed cache budget: every capacity times its doubled generation limit
/// and entry size. This is all the per-generator coordinate state there is,
/// however much world the generator is asked about.
fn report_budget(mask_bytes: usize) {
    let entries = |capacity: usize, payload: usize| 2 * capacity * (16 + payload);
    let masks = entries(VanillaGenerator::MASK_CACHE_CAPACITY, mask_bytes);
    let carvers = entries(VanillaGenerator::CHUNK_CARVERS_CACHE_CAPACITY, 8);
    let heights = entries(VanillaGenerator::HEIGHT_CACHE_CAPACITY, 4);
    let floors = entries(VanillaGenerator::OCEAN_FLOOR_MAP_CACHE_CAPACITY, 256 * 4);
    let biomes = entries(VanillaGenerator::BIOME_CACHE_CAPACITY, 32);
    let centers = entries(NoiseBasedAquifer::CENTER_CACHE_CAPACITY, 24);
    let statuses = entries(NoiseBasedAquifer::STATUS_CACHE_CAPACITY, 24);
    let surfaces = entries(NoiseBasedAquifer::SURFACE_CACHE_CAPACITY, 12);
    let skips = entries(NoiseBasedAquifer::SKIP_CACHE_CAPACITY, 12);
    println!(
        "budget_kib,carve_masks,{},chunk_carvers,{},heights,{},ocean_floor_maps,{},biomes,{},aquifer_centers,{}",
        masks / 1024,
        carvers / 1024,
        heights / 1024,
        floors / 1024,
        biomes / 1024,
        centers / 1024
    );
    println!(
        "budget_kib,aquifer_statuses,{},aquifer_surfaces,{},aquifer_skips,{},total,{}",
        statuses / 1024,
        surfaces / 1024,
        skips / 1024,
        (masks + carvers + heights + floors + biomes + centers + statuses + surfaces + skips)
            / 1024
    );
}

/// The documented chunk-fill path: the full material-rule descent of every
/// column in the square, counting written rows.
fn measure_columns(generator: &VanillaGenerator, config: &Config) -> usize {
    let mut rows = 0usize;
    for (base_x, base_z) in square(config) {
        for x in base_x..base_x + 16 {
            for z in base_z..base_z + 16 {
                rows += generator
                    .column_ids(x, z)
                    .iter()
                    .filter(|id| id.is_some())
                    .count();
            }
        }
    }
    rows
}

/// Column-top extraction only: the descent scan with its aquifer and carving
/// lookups, without the material-rule program.
fn measure_heights(generator: &VanillaGenerator, config: &Config) -> usize {
    let mut columns = 0usize;
    for (base_x, base_z) in square(config) {
        for x in base_x..base_x + 16 {
            for z in base_z..base_z + 16 {
                let _ = generator.surface_height(x, z);
                columns += 1;
            }
        }
    }
    columns
}

/// The carving pass alone: every block position of every chunk queried
/// against the replayed carve mask.
fn measure_carve(generator: &VanillaGenerator, config: &Config) -> usize {
    let mut carved = 0usize;
    let min_y = generator.min_y();
    for (base_x, base_z) in square(config) {
        for x in base_x..base_x + 16 {
            for z in base_z..base_z + 16 {
                for y in min_y..min_y + generator.height() {
                    if generator.carved(x, y, z) {
                        carved += 1;
                    }
                }
            }
        }
    }
    carved
}

/// The 3D substance metric: aquifer-adjusted density at every block.
fn measure_substance(generator: &VanillaGenerator, config: &Config) -> usize {
    let mut solid = 0usize;
    let min_y = generator.min_y();
    for (base_x, base_z) in square(config) {
        for x in base_x..base_x + 16 {
            for z in base_z..base_z + 16 {
                for y in min_y..min_y + generator.height() {
                    if matches!(generator.substance(x, y, z), Substance::Solid) {
                        solid += 1;
                    }
                }
            }
        }
    }
    solid
}

/// Chunk block origins of the configured square, in row-major order.
fn square(config: &Config) -> Vec<(i32, i32)> {
    let mut origins = Vec::with_capacity(config.side * config.side);
    for cz in 0..config.side {
        for cx in 0..config.side {
            origins.push((
                config.origin.0 + cx as i32 * 16,
                config.origin.1 + cz as i32 * 16,
            ));
        }
    }
    origins
}
