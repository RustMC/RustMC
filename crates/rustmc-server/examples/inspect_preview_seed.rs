//! Print RustMC preview ground and biome samples for a fixed-seed comparison.
//! These values are not vanilla Minecraft results.
use rustmc_server::world::Generator;

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 3 || args.len() % 2 == 0 {
        eprintln!("usage: inspect_preview_seed SEED_U64 X Z [X Z ...]");
        std::process::exit(2);
    }
    let Ok(seed) = args[0].parse::<u64>() else {
        eprintln!("invalid non-negative seed");
        std::process::exit(2);
    };
    let pairs: Result<Vec<_>, &str> = args[1..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let (Ok(x), Ok(z)) = (pair[0].parse::<i64>(), pair[1].parse::<i64>()) else {
                return Err("invalid coordinate pair");
            };
            if !(-1_000_000..=1_000_000).contains(&x) || !(-1_000_000..=1_000_000).contains(&z) {
                return Err("coordinates must be within one million blocks of origin");
            }
            Ok((x, z))
        })
        .collect();
    let pairs = pairs.unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    let generator = Generator::new(seed);
    println!("seed,x,z,preview_ground_y,preview_biome");
    for (x, z) in pairs {
        println!(
            "{seed},{x},{z},{},{}",
            generator.height(x, z),
            generator.biome(x, z).identifier()
        );
    }
}
