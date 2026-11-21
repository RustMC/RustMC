//! Small repository tools; never linked into the RustMC server.

pub mod nbt;
pub mod oracle;
pub mod region;
pub mod save;

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const REVIEWED: &[(&str, &str)] = &[
    ("adler2", "0BSD OR MIT OR Apache-2.0"),
    ("aes", "MIT OR Apache-2.0"),
    ("autocfg", "Apache-2.0 OR MIT"),
    ("base16ct", "Apache-2.0 OR MIT"),
    ("base64", "MIT OR Apache-2.0"),
    ("base64ct", "Apache-2.0 OR MIT"),
    ("block-buffer", "MIT OR Apache-2.0"),
    ("bytes", "MIT"),
    ("cc", "MIT OR Apache-2.0"),
    ("cfb8", "MIT OR Apache-2.0"),
    ("cfg-if", "MIT OR Apache-2.0"),
    ("cipher", "MIT OR Apache-2.0"),
    ("cmov", "Apache-2.0 OR MIT"),
    ("const-oid", "Apache-2.0 OR MIT"),
    ("cpubits", "MIT OR Apache-2.0"),
    ("cpufeatures", "MIT OR Apache-2.0"),
    ("crc32fast", "MIT OR Apache-2.0"),
    ("crypto-bigint", "Apache-2.0 OR MIT"),
    ("crypto-common", "MIT OR Apache-2.0"),
    ("crypto-primes", "Apache-2.0 OR MIT"),
    ("ctutils", "Apache-2.0 OR MIT"),
    ("der", "Apache-2.0 OR MIT"),
    ("digest", "MIT OR Apache-2.0"),
    ("equivalent", "Apache-2.0 OR MIT"),
    ("errno", "MIT OR Apache-2.0"),
    ("find-msvc-tools", "MIT OR Apache-2.0"),
    ("flate2", "MIT OR Apache-2.0"),
    ("getrandom", "MIT OR Apache-2.0"),
    ("hashbrown", "MIT OR Apache-2.0"),
    ("hmac", "MIT OR Apache-2.0"),
    ("http", "MIT OR Apache-2.0"),
    ("httparse", "MIT OR Apache-2.0"),
    ("hybrid-array", "MIT OR Apache-2.0"),
    ("indexmap", "Apache-2.0 OR MIT"),
    ("inout", "MIT OR Apache-2.0"),
    ("itoa", "MIT OR Apache-2.0"),
    ("libc", "MIT OR Apache-2.0"),
    ("log", "MIT OR Apache-2.0"),
    ("md-5", "MIT OR Apache-2.0"),
    ("memchr", "Unlicense OR MIT"),
    ("miniz_oxide", "MIT OR Zlib OR Apache-2.0"),
    ("num-traits", "MIT OR Apache-2.0"),
    ("once_cell", "MIT OR Apache-2.0"),
    ("pem-rfc7468", "Apache-2.0 OR MIT"),
    ("percent-encoding", "MIT OR Apache-2.0"),
    ("pkcs1", "Apache-2.0 OR MIT"),
    ("pkcs8", "Apache-2.0 OR MIT"),
    ("proc-macro2", "MIT OR Apache-2.0"),
    ("quote", "MIT OR Apache-2.0"),
    ("r-efi", "MIT OR Apache-2.0 OR LGPL-2.1-or-later"),
    ("rand_core", "MIT OR Apache-2.0"),
    ("ring", "Apache-2.0 AND ISC"),
    ("rsa", "MIT OR Apache-2.0"),
    ("rustc-hash", "Apache-2.0 OR MIT"),
    ("rustls", "Apache-2.0 OR ISC OR MIT"),
    ("rustls-pki-types", "MIT OR Apache-2.0"),
    ("rustls-webpki", "ISC"),
    ("serde", "MIT OR Apache-2.0"),
    ("serde_core", "MIT OR Apache-2.0"),
    ("serde_derive", "MIT OR Apache-2.0"),
    ("serde_json", "MIT OR Apache-2.0"),
    ("serde_spanned", "MIT OR Apache-2.0"),
    ("serdect", "Apache-2.0 OR MIT"),
    ("sha1", "MIT OR Apache-2.0"),
    ("sha2", "MIT OR Apache-2.0"),
    ("shlex", "MIT OR Apache-2.0"),
    ("signal-hook", "MIT OR Apache-2.0"),
    ("signal-hook-registry", "MIT OR Apache-2.0"),
    ("signature", "Apache-2.0 OR MIT"),
    ("simd-adler32", "MIT"),
    ("spki", "Apache-2.0 OR MIT"),
    ("subtle", "BSD-3-Clause"),
    ("syn", "MIT OR Apache-2.0"),
    ("toml", "MIT OR Apache-2.0"),
    ("toml_datetime", "MIT OR Apache-2.0"),
    ("toml_parser", "MIT OR Apache-2.0"),
    ("toml_writer", "MIT OR Apache-2.0"),
    ("typenum", "MIT OR Apache-2.0"),
    ("unicode-ident", "(MIT OR Apache-2.0) AND Unicode-3.0"),
    ("untrusted", "ISC"),
    ("ureq", "MIT OR Apache-2.0"),
    ("ureq-proto", "MIT OR Apache-2.0"),
    ("utf8-zero", "MIT OR Apache-2.0"),
    ("uuid", "Apache-2.0 OR MIT"),
    (
        "wasi",
        "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
    ),
    ("webpki-roots", "CDLA-Permissive-2.0"),
    ("windows-link", "MIT OR Apache-2.0"),
    ("windows-sys", "MIT OR Apache-2.0"),
    ("windows-targets", "MIT OR Apache-2.0"),
    ("windows_aarch64_gnullvm", "MIT OR Apache-2.0"),
    ("windows_aarch64_msvc", "MIT OR Apache-2.0"),
    ("windows_i686_gnu", "MIT OR Apache-2.0"),
    ("windows_i686_gnullvm", "MIT OR Apache-2.0"),
    ("windows_i686_msvc", "MIT OR Apache-2.0"),
    ("windows_x86_64_gnu", "MIT OR Apache-2.0"),
    ("windows_x86_64_gnullvm", "MIT OR Apache-2.0"),
    ("windows_x86_64_msvc", "MIT OR Apache-2.0"),
    ("winnow", "MIT"),
    ("zeroize", "Apache-2.0 OR MIT"),
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
