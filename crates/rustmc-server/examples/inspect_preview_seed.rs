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
    let generator = Generator::new(seed);
    println!("seed,x,z,preview_ground_y,preview_biome");
    for pair in args[1..].as_chunks::<2>().0 {
        let (Ok(x), Ok(z)) = (pair[0].parse::<i64>(), pair[1].parse::<i64>()) else {
            eprintln!("invalid coordinate pair");
            std::process::exit(2);
        };
        if !(-1_000_000..=1_000_000).contains(&x) || !(-1_000_000..=1_000_000).contains(&z) {
            eprintln!("coordinates must be within one million blocks of origin");
            std::process::exit(2);
        }
        println!(
            "{seed},{x},{z},{},{}",
            generator.height(x, z),
            generator.biome(x, z).identifier()
        );
    }
}
