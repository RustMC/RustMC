//! Prepare immutable preview packets around spawn before a player joins.
//! Usage: `prepare_preview_cache DATA_ROOT REGISTRY_TABLE CACHE_ROOT SEED RADIUS [WORKERS]`

use rustmc_server::chunk_adapter::{chunk_from_generator, encode_chunk, registry::RegistryTables};
use rustmc_server::preview_cache::PreviewCache;
use rustmc_server::vanilla::generator::VanillaGenerator;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Instant;

fn main() {
    if let Err(error) = run() {
        eprintln!("preview preparation failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !(5..=6).contains(&args.len()) {
        return Err(
            "usage: prepare_preview_cache DATA_ROOT REGISTRY_TABLE CACHE_ROOT SEED RADIUS [WORKERS]".into(),
        );
    }
    let data_root = PathBuf::from(&args[0]);
    let registry_path = PathBuf::from(&args[1]);
    let cache_root = PathBuf::from(&args[2]);
    let seed: i64 = args[3]
        .to_string_lossy()
        .parse()
        .map_err(|_| "invalid seed")?;
    let radius: i32 = args[4]
        .to_string_lossy()
        .parse()
        .map_err(|_| "invalid radius")?;
    if !(1..=32).contains(&radius) {
        return Err("radius must be 1..=32".into());
    }
    let workers: usize = args.get(5).map_or(Ok(8), |value| {
        value
            .to_string_lossy()
            .parse()
            .map_err(|_| "invalid workers")
    })?;
    if !(1..=20).contains(&workers) {
        return Err("workers must be 1..=20".into());
    }
    let text = std::fs::read_to_string(&registry_path).map_err(|error| error.to_string())?;
    RegistryTables::from_provisioned(&text).map_err(|error| error.to_string())?;
    let cache = PreviewCache::open(&cache_root, &data_root, &registry_path, seed)
        .map_err(|error| error.to_string())?;
    let mut positions = Vec::new();
    for z in -radius..=radius {
        for x in -radius..=radius {
            positions.push((x, z));
        }
    }
    positions.sort_by_key(|(x, z)| (x.abs().max(z.abs()), *z, *x));
    let total = positions.len();
    let work = Arc::new(Mutex::new(positions.into_iter()));
    let built = AtomicUsize::new(0);
    let hits = AtomicUsize::new(0);
    let started = Instant::now();
    std::thread::scope(|scope| -> Result<(), String> {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let work = Arc::clone(&work);
                let cache = cache.clone();
                let data_root = &data_root;
                let text = &text;
                let built = &built;
                let hits = &hits;
                scope.spawn(move || -> Result<(), String> {
                    let generator = VanillaGenerator::new(data_root, seed, "minecraft:overworld")
                        .map_err(|error| error.to_string())?;
                    let tables = RegistryTables::from_provisioned(text)
                        .map_err(|error| error.to_string())?;
                    loop {
                        let Some((x, z)) = work.lock().map_err(|error| error.to_string())?.next()
                        else {
                            break;
                        };
                        if cache
                            .read(x, z)
                            .map_err(|error| error.to_string())?
                            .is_some()
                        {
                            hits.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        let chunk = chunk_from_generator(&generator, x, z, &tables)
                            .map_err(|error| error.to_string())?;
                        let packet =
                            encode_chunk(&chunk, &tables).map_err(|error| error.to_string())?;
                        cache
                            .write(x, z, &packet)
                            .map_err(|error| error.to_string())?;
                        let count = built.fetch_add(1, Ordering::Relaxed) + 1;
                        if count.is_multiple_of(32) {
                            eprintln!("prepared {count}/{total} chunks");
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        for handle in handles {
            handle.join().map_err(|_| "worker panic".to_owned())??;
        }
        Ok(())
    })?;
    println!(
        "preview cache: workers={workers} total={total} built={} hits={} elapsed_ms={}",
        built.load(Ordering::Relaxed),
        hits.load(Ordering::Relaxed),
        started.elapsed().as_millis()
    );
    Ok(())
}
