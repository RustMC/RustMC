# Independent development and provenance

RustMC core design and code are developed independently. Do not copy competing server implementations or architecture as a template, use decompiled proprietary code, or redistribute unapproved game assets. Primary technical documentation and reviewed general-purpose libraries are allowed. This is a process policy, not proof that every concept is unprecedented. Contributions must identify source, usage, license, and fixture/data origin; do not invent cryptography.

| Item | Origin | Use and review |
| --- | --- | --- |
| RustMC architecture and M0 scaffold | Project requirements and independent design | Proposed; owner review pending |
| Rust toolchain/Cargo | Rust project documentation | Build behavior; toolchain pinned |
| `toml` crate and transitive dependencies | crates.io packages in `Cargo.lock` | Configuration parsing; dependency license/advisory review pending |
| Java/Bedrock differences | [Microsoft Learn](https://learn.microsoft.com/en-us/minecraft/creator/documents/differencesbetweenbedrockandjava?view=minecraft-bedrock-stable) | Motivation for separate compatibility claims; not a protocol specification |
| GitHub Actions security | [GitHub Docs](https://docs.github.com/en/actions/reference/security/secure-use) | CI permissions and action pinning |

No game data, protocol fixtures, or generated assets are distributed in M0. Dependency updates require a new license and advisory review; unavailable advisory databases must be reported as unavailable.
