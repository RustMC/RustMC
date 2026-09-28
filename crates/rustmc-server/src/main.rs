#![forbid(unsafe_code)]

use std::{
    io::{self, Write},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

use rustmc_server::runtime::{LifecycleState, RuntimeEvent, run_listener};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    flag,
};

const HELP: &str = "RustMC local development runtime (no Minecraft protocol)\n\nUsage:\n  rustmc-server --help\n  rustmc-server --version\n  rustmc-server --check-config <path>\n  rustmc-server --run <path>\n\nExit codes: 0 clean stop/success, 2 usage error, 3 configuration error, 4 listener startup error, 5 running listener error.";

fn print_event(event: RuntimeEvent) {
    println!("{}", event.log_line());
    let _ = io::stdout().flush();
}

fn control_event(kind: &'static str, state: LifecycleState, started: Instant) {
    print_event(RuntimeEvent {
        kind,
        state,
        elapsed_ms: started.elapsed().as_millis(),
        address: None,
        active_connections: None,
        reason: None,
    });
}

fn run(path: &Path, started: Instant) -> i32 {
    let shutdown = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM] {
        if let Err(error) = flag::register(signal, Arc::clone(&shutdown)) {
            control_event("failed", LifecycleState::Failed, started);
            eprintln!("startup error: could not register shutdown signal: {error}");
            return 4;
        }
    }
    control_event("process_started", LifecycleState::Configuring, started);
    let config = match rustmc_server::check_config(path) {
        Ok(config) => config,
        Err(error) => {
            control_event("failed", LifecycleState::Failed, started);
            eprintln!("configuration error: {error}");
            return 3;
        }
    };
    control_event("config_validated", LifecycleState::Configuring, started);
    match run_listener(&config.listener, &shutdown, started, |event| {
        if !event.kind.starts_with("connection_") || config.log_level.allows_connection_events() {
            print_event(event);
        }
    }) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("startup/runtime error: {error}");
            error.exit_code()
        }
    }
}

fn main() {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.as_slice() {
        [] => {
            println!(
                "RustMC development runtime was not started; no listener or world started. Use --help."
            );
            0
        }
        [flag] if flag == "--help" => {
            println!("{HELP}");
            0
        }
        [flag] if flag == "--version" => {
            println!(
                "RustMC {} (development runtime; no Minecraft protocol)",
                env!("CARGO_PKG_VERSION")
            );
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
        [flag, path] if flag == "--run" => run(Path::new(path), started),
        _ => {
            eprintln!(
                "usage error: expected --help, --version, --check-config <path>, or --run <path>. Use --help."
            );
            2
        }
    };
    std::process::exit(code);
}
