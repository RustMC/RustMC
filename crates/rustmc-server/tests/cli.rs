#![forbid(unsafe_code)]

use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustmc-test-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, contents: &str) -> PathBuf {
        let path = self.0.join("config.toml");
        fs::write(&path, contents).unwrap();
        path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rustmc-server"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_version_and_bootstrap_only() {
    let help = run(&["--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&help.stdout).contains("--check-config <path>"));
    let version = run(&["--version"]);
    assert_eq!(version.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&version.stdout).contains(env!("CARGO_PKG_VERSION")));
    let default = run(&[]);
    assert_eq!(default.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&default.stdout).contains("no listener or world started"));
}

#[test]
fn valid_and_invalid_configurations() {
    let dir = TempDir::new();
    let cases = [
        ("schema_version = 1\nlog_level = 'info'", 0, "valid"),
        ("schema_version = 1\nlog_level =", 3, "malformed TOML"),
        (
            "schema_version = 1\nlog_level = 'info'\nextra = 4",
            3,
            "unknown",
        ),
        ("schema_version = 9\nlog_level = 'info'", 3, "unsupported"),
        (
            "schema_version = 1\nlog_level = 'private-token'",
            3,
            "invalid",
        ),
        ("log_level = 'info'", 3, "missing"),
    ];
    for (contents, code, expected) in cases {
        let path = dir.file(contents);
        let output = run(&["--check-config", path.to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(code), "{contents}");
        let message = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(message.contains(expected), "{message}");
        assert!(!message.contains("private-token"));
        assert!(!message.contains("panicked"));
        assert_eq!(fs::read_to_string(path).unwrap(), contents);
    }
    let missing = dir.0.join("missing.toml");
    let output = run(&["--check-config", missing.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not read"));
}

#[test]
fn usage_errors() {
    for args in [
        vec!["--check-config"],
        vec!["--bad"],
        vec!["--help", "extra"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("usage error"));
    }
}
