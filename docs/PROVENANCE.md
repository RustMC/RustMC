# Independent development and provenance

RustMC core design and code are developed independently. Do not copy competing server implementations or architecture as a template, use decompiled proprietary code, or redistribute unapproved game assets. Primary technical documentation and reviewed general-purpose libraries are allowed. This is a process policy, not proof that every concept is unprecedented. Contributions must identify source, usage, license, and fixture/data origin; do not invent cryptography.

| Item | Origin | Use and review |
| --- | --- | --- |
| RustMC architecture and M0 scaffold | Project requirements and independent design | Proposed; owner review pending |
| Rust toolchain/Cargo | Rust project documentation | Build behavior; toolchain pinned |
| `toml` crate and transitive dependencies | crates.io packages in `Cargo.lock` | Configuration parsing; locked license metadata reviewed below; advisory review pending |
| Java/Bedrock differences | [Microsoft Learn](https://learn.microsoft.com/en-us/minecraft/creator/documents/differencesbetweenbedrockandjava?view=minecraft-bedrock-stable) | Motivation for separate compatibility claims; not a protocol specification |
| GitHub Actions security | [GitHub Docs](https://docs.github.com/en/actions/reference/security/secure-use) | CI permissions and action pinning |

No game data, protocol fixtures, or generated assets are distributed in M0. Dependency updates require a new license and advisory review; unavailable advisory databases must be reported as unavailable.

## License and locked Cargo dependency review

The owner adopted [Apache License 2.0](../LICENSE) for RustMC's original source. This does not relicense third-party crates, Minecraft assets, client data, or contributed material without the contributor's rights. The standard license text came from the [Apache Software Foundation](https://www.apache.org/licenses/LICENSE-2.0.txt). The package remains `publish = false`.

On 29 September 2026, `cargo metadata --locked` reported the following license expressions for every third-party package in the M0 lockfile. `toml` is the only direct dependency, used for bootstrap configuration parsing. All other rows are transitive parsing or build dependencies. The listed expressions provide an Apache-2.0 or MIT option; `unicode-ident` also includes Unicode-3.0. The [Apache third-party policy](https://www.apache.org/legal/resolved.html) treats MIT as a compatible permissive license, and [Unicode describes Unicode-3.0](https://unicode.org/policies/licensing_policy.html) as permissive. No incompatible declared license was found. This metadata review does not replace checking shipped license texts and notices when packaging binaries.

| Package(s) in `Cargo.lock` | Declared SPDX expression | Purpose |
| --- | --- | --- |
| `toml 0.8.23` | MIT OR Apache-2.0 | Direct TOML parser |
| `equivalent 1.0.2`, `hashbrown 0.17.1`, `indexmap 2.14.2` | Apache-2.0 OR MIT / MIT OR Apache-2.0 | Parser map storage |
| `memchr 2.8.3`, `winnow 0.7.15` | Unlicense OR MIT / MIT | Parser scanning |
| `serde 1.0.229`, `serde_core 1.0.229`, `serde_derive 1.0.229`, `serde_spanned 0.6.9` | MIT OR Apache-2.0 | Parser serialization support |
| `proc-macro2 1.0.107`, `quote 1.0.47`, `syn 3.0.6` | MIT OR Apache-2.0 | Transitive macro/build support |
| `toml_datetime 0.6.11`, `toml_edit 0.22.27`, `toml_write 0.1.2` | MIT OR Apache-2.0 | TOML parsing and formatting support |
| `unicode-ident 1.0.26` | (MIT OR Apache-2.0) AND Unicode-3.0 | Identifier parsing in macro/build support |

Dependencies retain their own license terms and attribution. Recheck this ledger when the lockfile changes, and collect required third-party license texts and notices before distributing a binary. No Minecraft assets are included in the M0 source tree.

## M1 dependency addition

`signal-hook 0.4.4` is a direct dependency used only to set an atomic shutdown flag on Unix SIGINT/SIGTERM. It avoids first-party unsafe signal handlers; the development runtime remains Linux-first. Its declared license is `MIT OR Apache-2.0`. The new locked transitive packages are `errno 0.3.14`, `libc 0.2.189`, `signal-hook-registry 1.4.8`, `windows-link 0.2.1`, and `windows-sys 0.61.2`; each declares `MIT OR Apache-2.0` in Cargo metadata. The Windows packages are target-specific transitive entries, not a claim of tested Windows support. At the 29 September 2026 review, `cargo info` identified 0.4.4 as the current published `signal-hook` release, and its [upstream repository](https://github.com/vorner/signal-hook) was not archived and showed an April 2026 push. This is a maintenance signal, not a guarantee of future support. The current declared licenses are compatible with RustMC's Apache-2.0 source policy; future binary packages must collect applicable third-party notices. The [CI license policy](../scripts/check_dependency_licenses.py) fails on new or changed declarations until reviewed here.
