//! Measure process spawn to local listener bind. No cache or playable-readiness claim.
use std::{
    error::Error,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn measure(binary: &Path, config: &Path) -> Result<(f64, f64), Box<dyn Error>> {
    let started = Instant::now();
    let mut child = Command::new(binary)
        .args(["--run", config.to_str().ok_or("non-UTF8 config path")?])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let bound = (|| {
        loop {
            let line = receiver.recv_timeout(Duration::from_secs(5))??;
            if line.contains("event=listener_bound ") {
                let observed = started.elapsed().as_secs_f64() * 1000.0;
                let elapsed = line
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix("elapsed_us="))
                    .ok_or("bind event lacks elapsed_us")?
                    .parse::<f64>()?
                    / 1000.0;
                return Ok::<_, Box<dyn Error>>((elapsed, observed));
            }
        }
    })();
    if bound.is_ok() {
        let status = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()?;
        if !status.success() {
            child.kill()?;
        }
    } else {
        child.kill()?;
    }
    let exit = child.wait()?;
    reader.join().map_err(|_| "reader thread panicked")?;
    if !exit.success() {
        return Err(format!("listener exited with {exit}").into());
    }
    bound
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    let pairs = match args.as_slice() {
        [_] => 5,
        [_, flag, value] if flag == "--pairs" => value.parse::<usize>()?,
        _ => return Err("usage: measure_startup [--pairs POSITIVE_INTEGER]".into()),
    };
    if pairs == 0 {
        return Err("--pairs must be positive".into());
    }
    let binary = Path::new("target/release/rustmc-server");
    let config = Path::new("config/rustmc.example.toml");
    if !binary.is_file() || !config.is_file() {
        return Err("build the release server and provide its example config first".into());
    }
    println!("cycle,mode,code_entry_to_bound_ms,parent_spawn_to_bound_ms");
    for cycle in 1..=pairs {
        for mode in ["first", "warm_repeat"] {
            let (internal, observed) = measure(binary, config)?;
            println!("{cycle},{mode},{internal:.3},{observed:.3}");
        }
    }
    Ok(())
}
