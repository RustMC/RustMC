//! Raw generation measurements, separate from build, encoding, and delivery.
use rustmc_server::world::Generator;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [seed, count] = args.as_slice() else {
        eprintln!("usage: measure_generation <seed:u64> <count:1..=100>");
        std::process::exit(2);
    };
    let Ok(seed) = seed.parse::<u64>() else {
        eprintln!("invalid seed");
        std::process::exit(2);
    };
    let Ok(count) = count.parse::<u32>() else {
        eprintln!("invalid count");
        std::process::exit(2);
    };
    if !(1..=100).contains(&count) {
        eprintln!("count must be 1..=100");
        std::process::exit(2);
    }
    let generator = Generator::new(seed);
    for index in 0..count {
        let x = (index % 10) as i32 - 5;
        let z = (index / 10) as i32 - 5;
        let started = Instant::now();
        let chunk = std::hint::black_box(generator.generate(x, z));
        let elapsed = started.elapsed();
        println!(
            "seed={seed} chunk_x={x} chunk_z={z} generation_us={} sample={:?}",
            elapsed.as_micros(),
            chunk.block(0, 0, 0)
        );
    }
}
