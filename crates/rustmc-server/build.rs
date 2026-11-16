//! Fingerprint the sources that determine cached preview chunk packets.
//!
//! The operator-local packet cache must never serve bytes produced by an older
//! generator or encoder. Hashing those sources at build time makes every change
//! to them select a fresh cache directory without a hand-maintained version.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// Directories and files whose contents shape preview chunk packets.
const PACKET_SOURCES: &[&str] = &[
    "src/vanilla",
    "src/chunk_adapter",
    "src/java_packets.rs",
    "src/java_wire.rs",
];

fn collect(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<PathBuf> = fs::read_dir(path)
            .unwrap_or_else(|error| panic!("cannot list {}: {error}", path.display()))
            .map(|entry| entry.expect("readable directory entry").path())
            .collect();
        entries.sort();
        for entry in entries {
            collect(&entry, files);
        }
    } else if path.extension().is_some_and(|ext| ext == "rs") {
        files.push(path.to_path_buf());
    }
}

/// 64-bit FNV-1a. This only detects source changes; it is not a security hash.
fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by Cargo"));
    let mut files = Vec::new();
    for source in PACKET_SOURCES {
        println!("cargo::rerun-if-changed={source}");
        collect(&root.join(source), &mut files);
    }
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for file in files {
        let relative = file.strip_prefix(&root).expect("collected under the crate");
        // Normalize separators so the fingerprint matches across platforms.
        let name = relative.to_string_lossy().replace('\\', "/");
        let contents = fs::read(&file)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
        fnv1a(&mut hash, &(name.len() as u64).to_le_bytes());
        fnv1a(&mut hash, name.as_bytes());
        fnv1a(&mut hash, &(contents.len() as u64).to_le_bytes());
        fnv1a(&mut hash, &contents);
    }
    println!("cargo::rustc-env=RUSTMC_PACKET_SOURCE_FINGERPRINT={hash:016x}");
}
