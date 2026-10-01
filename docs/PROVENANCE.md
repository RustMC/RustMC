# Independent development and provenance

RustMC core design and code are developed independently. Do not copy competing server implementations or architecture as a template, use decompiled proprietary code, or redistribute unapproved game assets. Primary technical documentation and reviewed general-purpose libraries are allowed. This is a process policy, not proof that every concept is unprecedented. Contributions must identify source, usage, license, and fixture/data origin; do not invent cryptography. One scoped exception exists: [ADR-0014 as amended](decisions/ADR-0014.md) lets terrain work **read** deobfuscated vanilla or PaperMC generation code for understanding only; nothing consulted may enter git in any form, and each session is logged below.

| Item | Origin | Use and review |
| --- | --- | --- |
| RustMC architecture and M0 scaffold | Project requirements and independent design | Proposed; owner review pending |
| Rust toolchain/Cargo | Rust project documentation | Build behavior; toolchain pinned |
| `toml` crate and transitive dependencies | crates.io packages in `Cargo.lock` | Configuration parsing; locked license metadata reviewed below; advisory review pending |
| Java/Bedrock differences | [Microsoft Learn](https://learn.microsoft.com/en-us/minecraft/creator/documents/differencesbetweenbedrockandjava?view=minecraft-bedrock-stable) | Motivation for separate compatibility claims; not a protocol specification |
| GitHub Actions security | [GitHub Docs](https://docs.github.com/en/actions/reference/security/secure-use) | CI permissions and action pinning |
| M2 Java 26.3 release and protocol 777 | [Mojang version manifest](https://piston-meta.mojang.com/mc/game/version_manifest_v2.json) and `version.json` in the [official server archive](https://piston-data.mojang.com/v1/objects/33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c/server.jar) | Version metadata only; no server code or assets copied |
| Java 26.3 packet IDs for the preview investigation | `generated/reports/packets.json` emitted by the [official server archive](https://piston-data.mojang.com/v1/objects/33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c/server.jar) using `java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports` | Generated report inspected locally in `/tmp`, not copied into RustMC; IDs must still be paired with client-observed field behavior |
| Java 26.3 login-finished fields | `ClientboundLoginFinishedPacket` and its `STREAM_CODEC` in the [official 26.3 server archive](https://piston-data.mojang.com/v1/objects/33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c/server.jar), inspected with `javap` | Confirmed the 26.3 response includes a game profile followed by a session UUID. RustMC independently encodes the fields; no Mojang bytecode or assets are checked in. |
| Java 26.3 known-pack fields | `ClientboundSelectKnownPacks` and `KnownPack.STREAM_CODEC` in the same official archive, inspected with `javap`; version from its `version.json` | Confirmed a list of namespace, ID, and version strings. The 26.3 client acknowledged RustMC's `minecraft:core` pack on 29 September 2026. No Mojang class or pack data is checked in. |
| M2 Bedrock 1.26.51 protocol 2193 | [Mojang protocol release](https://github.com/Mojang/bedrock-protocol-docs/releases/tag/v1.26.51) | Network identifier for discovery response; 26.52 hotfix not claimed |
| Unconnected UDP ping/pong envelope | [RakNet message identifiers](https://github.com/facebookarchive/RakNet/blob/master/Source/MessageIdentifiers.h) | Primary transport reference; RustMC codec and tests written independently |

No game data, protocol fixtures, or generated assets are distributed in M0. Dependency updates require a new license and advisory review; unavailable advisory databases must be reported as unavailable.

M2's checked-in test bytes are independently constructed protocol probes, not captured client assets. Java 26.3 status was observed in the owner's running client on 29 September 2026. Bedrock discovery text still needs real-client observation before compatibility is marked tested. No new Cargo dependency was added for M2 discovery; the existing locked license policy remains unchanged.

The opt-in Java preview adds `uuid` 1.26.1 to assign a fresh connection-scoped v4 session ID without implementing custom randomness. Its `getrandom` 0.4.3 dependency obtains entropy from the operating system; `cfg-if` 1.0.5 and target-specific `r-efi` 6.0.0 are transitive support packages. Their locked metadata declares `Apache-2.0 OR MIT`, `MIT OR Apache-2.0`, `MIT OR Apache-2.0`, and `MIT OR Apache-2.0 OR LGPL-2.1-or-later` respectively, so an Apache-2.0 option is available for each. The policy gate checks exact declarations and requires review on change. This is a metadata review, not a binary-distribution notice audit or security audit.

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

`signal-hook 0.4.4` is a direct dependency used only to set an atomic shutdown flag on Unix SIGINT/SIGTERM. It avoids first-party unsafe signal handlers; the development runtime remains Linux-first. Its declared license is `MIT OR Apache-2.0`. The new locked transitive packages are `errno 0.3.14`, `libc 0.2.189`, `signal-hook-registry 1.4.8`, `windows-link 0.2.1`, and `windows-sys 0.61.2`; each declares `MIT OR Apache-2.0` in Cargo metadata. The Windows packages are target-specific transitive entries, not a claim of tested Windows support. At the 29 September 2026 review, `cargo info` identified 0.4.4 as the current published `signal-hook` release, and its [upstream repository](https://github.com/vorner/signal-hook) was not archived and showed an April 2026 push. This is a maintenance signal, not a guarantee of future support. The current declared licenses are compatible with RustMC's Apache-2.0 source policy; future binary packages must collect applicable third-party notices. The [CI license policy](../crates/rustmc-tools/src/bin/check_dependency_licenses.rs) fails on new or changed declarations until reviewed here.

## Rust development utilities (29 September 2026)

The `rustmc-tools` workspace package replaces the earlier developer Python scripts; it is never linked into the server binary. Its direct `serde_json 1.0.151` dependency parses Cargo metadata and official version/registry reports (`MIT OR Apache-2.0`). Locked transitive `itoa 1.0.18` (`MIT OR Apache-2.0`) and `zmij 1.0.23` (`MIT`) support JSON serialization. These declared licenses are compatible with RustMC's Apache-2.0 source policy; binary distribution still needs notice review. The local registry preparation tool calls the system `sha1sum` and `unzip` utilities for the owner-supplied official archive. It verifies the official archive hash and version, then writes identifier metadata only to a local path. The startup timing tool is warm-only; it does not make cold-cache claims.

## Vanilla oracle (T0, 30 September 2026)

The `vanilla_oracle` tool reads Anvil region files from single-player saves the owner's licensed client generated, so RustMC can measure its generator against vanilla ground truth. Nothing from any save is copied into the repository; the tool prints only derived column facts (height, block name, biome name) and aggregate match percentages. Format facts about 26.3 saves — heightmaps relative to the world minimum Y, mixed string/compound blockstate palettes, exact-width biome palettes stored as low as one bit — were established by parsing the owner's own world and are documented in `docs/research/`. Region decompression uses `flate2 1.1.5` (`MIT OR Apache-2.0`) with its locked transitive packages `crc32fast 1.5.2` (`MIT OR Apache-2.0`), `miniz_oxide 0.8.9` (`MIT OR Zlib OR Apache-2.0`), `adler2 2.0.1` (`0BSD OR MIT OR Apache-2.0`), and `simd-adler32 0.3.10` (`MIT`); each has an Apache-2.0-compatible option and was reviewed against the policy gate. `rustmc-tools` now depends on `rustmc-server` read-only to compare the live generator; this links no new third-party code. The NBT and region readers in `crates/rustmc-tools/src/nbt.rs` and `region.rs` were written from the public Anvil/NBT format descriptions and verified against the owner's save, not translated from any server implementation.

## Terrain generation consultation log (ADR-0014, knowledge only)

Per the owner's 30 September 2026 direction, ADR-0014 was amended so terrain work may read deobfuscated vanilla or PaperMC generation code **for understanding only**. Nothing consulted enters git: no files, code fragments, or vendor references in tracked content. Each session appends an entry here: what was read, what was learned, and how RustMC implemented it independently.

- 2026-09-30 — Policy amendment recorded; no code consulted yet. The T0 oracle results and the experimental noise field (both pre-amendment, black-box) are documented above and in `docs/research/`.

## TOML 1.1 dependency review (2 October 2026)

The configuration parser upgrade to `toml 1.1.6+spec-1.1.0` changes the
whole-document entry point: RustMC now uses `toml::from_str` in its two TOML
document readers. `toml::Value::from_str` in this release parses one value,
which broke valid configuration and registry manifests until corrected.

The updated lockfile contains `serde_spanned 1.1.1`, `toml_datetime
1.1.1+spec-1.1.0`, `toml_parser 1.1.3+spec-1.1.0`, `toml_writer
1.1.2+spec-1.1.0`, and `winnow 1.0.4`. Their declared licenses are `MIT OR
Apache-2.0` except `winnow`, which declares `MIT`; each has a compatible
option under RustMC's Apache-2.0 policy. `toml_edit` and `toml_write` leave
the lockfile. The locked license gate checks these declarations, while a
binary release still needs the required third-party notice review.
