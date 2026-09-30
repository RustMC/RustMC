//! Small repository tools; never linked into the RustMC server.

pub mod nbt;
pub mod oracle;
pub mod region;

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const REVIEWED: &[(&str, &str)] = &[
    ("adler2", "0BSD OR MIT OR Apache-2.0"),
    ("block-buffer", "MIT OR Apache-2.0"),
    ("cfg-if", "MIT OR Apache-2.0"),
    ("crc32fast", "MIT OR Apache-2.0"),
    ("crypto-common", "MIT OR Apache-2.0"),
    ("digest", "MIT OR Apache-2.0"),
    ("equivalent", "Apache-2.0 OR MIT"),
    ("errno", "MIT OR Apache-2.0"),
    ("flate2", "MIT OR Apache-2.0"),
    ("generic-array", "MIT"),
    ("getrandom", "MIT OR Apache-2.0"),
    ("hashbrown", "MIT OR Apache-2.0"),
    ("indexmap", "Apache-2.0 OR MIT"),
    ("itoa", "MIT OR Apache-2.0"),
    ("libc", "MIT OR Apache-2.0"),
    ("md-5", "MIT OR Apache-2.0"),
    ("memchr", "Unlicense OR MIT"),
    ("miniz_oxide", "MIT OR Zlib OR Apache-2.0"),
    ("proc-macro2", "MIT OR Apache-2.0"),
    ("quote", "MIT OR Apache-2.0"),
    ("r-efi", "MIT OR Apache-2.0 OR LGPL-2.1-or-later"),
    ("serde", "MIT OR Apache-2.0"),
    ("serde_core", "MIT OR Apache-2.0"),
    ("serde_derive", "MIT OR Apache-2.0"),
    ("serde_json", "MIT OR Apache-2.0"),
    ("serde_spanned", "MIT OR Apache-2.0"),
    ("signal-hook", "MIT OR Apache-2.0"),
    ("signal-hook-registry", "MIT OR Apache-2.0"),
    ("simd-adler32", "MIT"),
    ("syn", "MIT OR Apache-2.0"),
    ("toml", "MIT OR Apache-2.0"),
    ("toml_datetime", "MIT OR Apache-2.0"),
    ("toml_edit", "MIT OR Apache-2.0"),
    ("toml_write", "MIT OR Apache-2.0"),
    ("typenum", "MIT OR Apache-2.0"),
    ("unicode-ident", "(MIT OR Apache-2.0) AND Unicode-3.0"),
    ("uuid", "Apache-2.0 OR MIT"),
    ("version_check", "MIT/Apache-2.0"),
    ("windows-link", "MIT OR Apache-2.0"),
    ("windows-sys", "MIT OR Apache-2.0"),
    ("winnow", "MIT"),
    ("zmij", "MIT"),
];

pub fn review_packages(packages: &[Value], reviewed: &[(&str, &str)]) -> Vec<String> {
    let expected: BTreeMap<_, _> = reviewed.iter().copied().collect();
    let mut seen = BTreeSet::new();
    let mut first_party = BTreeSet::new();
    let mut problems = Vec::new();
    for package in packages {
        let name = package["name"].as_str().unwrap_or("<missing name>");
        let version = package["version"].as_str().unwrap_or("<missing version>");
        let license = package["license"].as_str();
        if matches!(name, "rustmc-server" | "rustmc-tools") {
            first_party.insert(name);
            if license != Some("Apache-2.0") {
                problems.push(format!("{name}: expected Apache-2.0, found {license:?}"));
            }
            continue;
        }
        seen.insert(name);
        match expected.get(name) {
            None => problems.push(format!(
                "{name} {version}: unreviewed dependency, license {license:?}"
            )),
            Some(wanted) if license != Some(*wanted) => problems.push(format!(
                "{name} {version}: license changed from {wanted:?} to {license:?}"
            )),
            _ => {}
        }
    }
    for name in ["rustmc-server", "rustmc-tools"] {
        if !first_party.contains(name) {
            problems.push(format!("{name}: first-party package missing from metadata"));
        }
    }
    for name in expected.keys() {
        if !seen.contains(name) {
            problems.push(format!("{name}: reviewed entry is no longer in Cargo.lock"));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_unknown_and_changed_licenses() {
        let packages = vec![
            json!({"name":"rustmc-server","license":"Apache-2.0"}),
            json!({"name":"rustmc-tools","license":"Apache-2.0"}),
            json!({"name":"known","version":"1","license":"GPL-3.0"}),
            json!({"name":"new","version":"1","license":null}),
        ];
        let problems = review_packages(&packages, &[("known", "MIT")]);
        assert!(problems.iter().any(|p| p.contains("license changed")));
        assert!(problems.iter().any(|p| p.contains("unreviewed dependency")));
    }

    #[test]
    fn accepts_reviewed_metadata() {
        let packages = vec![
            json!({"name":"rustmc-server","license":"Apache-2.0"}),
            json!({"name":"rustmc-tools","license":"Apache-2.0"}),
            json!({"name":"known","version":"1","license":"MIT"}),
        ];
        assert!(review_packages(&packages, &[("known", "MIT")]).is_empty());
    }
}
