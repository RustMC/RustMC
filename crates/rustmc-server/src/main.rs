#![forbid(unsafe_code)]

use std::path::Path;

const HELP: &str = "RustMC bootstrap CLI (no game server)\n\nUsage:\n  rustmc-server --help\n  rustmc-server --version\n  rustmc-server --check-config <path>\n\nExit codes: 0 success, 2 usage error, 3 configuration error.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.as_slice() {
        [] => {
            println!(
                "RustMC foundation is bootstrap-only; no listener or world started. Use --help."
            );
            0
        }
        [flag] if flag == "--help" => {
            println!("{HELP}");
            0
        }
        [flag] if flag == "--version" => {
            println!("RustMC {} (bootstrap-only)", env!("CARGO_PKG_VERSION"));
            0
        }
        [flag, path] if flag == "--check-config" => {
            match rustmc_server::check_config(Path::new(path)) {
                Ok(_) => {
                    println!("RustMC configuration is valid (schema version 1).");
                    0
                }
                Err(error) => {
                    eprintln!("configuration error: {error}");
                    3
                }
            }
        }
        _ => {
            eprintln!(
                "usage error: expected --help, --version, or --check-config <path>. Use --help."
            );
            2
        }
    };
    std::process::exit(code);
}
