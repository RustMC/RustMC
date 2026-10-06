# Independent development and provenance

RustMC core design and code are developed independently. Do not copy competing server implementations or architecture as a template, use decompiled proprietary code, or redistribute unapproved game assets. Primary technical documentation and reviewed general-purpose libraries are allowed. This is a process policy, not proof that every concept is unprecedented. Contributions must identify source, usage, license, and fixture/data origin; do not invent cryptography. One scoped exception exists: [ADR-0014 as amended](decisions/ADR-0014.md) lets terrain work **read** deobfuscated vanilla or PaperMC generation code for understanding only; nothing consulted may enter git in any form, and each session is logged below.

| Item | Origin | Use and review |
| --- | --- | --- |
| RustMC architecture and M0 scaffold | Project requirements and independent design | Proposed; owner review pending |
| Rust toolchain/Cargo | Rust project documentation | Build behavior; toolchain pinned |
| `toml` crate and transitive dependencies | crates.io packages in `Cargo.lock` | Configuration parsing; locked license metadata reviewed below; advisory review pending |
| `rustc-hash` 2.1.3 | [crate source](https://crates.io/crates/rustc-hash/2.1.3) | Faster hashing of internal, bounded world-coordinate caches only. Declares `Apache-2.0 OR MIT`; no protocol, identity, or unbounded user-input map uses it. |
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

### Session 1 (30 September – 1 October 2026): random source and gradient-noise core

Under the amended policy, terrain-generation semantics were established by executing the owner's locally deobfuscated Java 26.3 classes in `/tmp` and recording numeric behavior facts. No deobfuscated source, Mojang code, or Mojang data entered this repository; the Rust modules under `crates/rustmc-server/src/vanilla/` were written independently against the recorded facts and are pinned by the parity tests in `vanilla::random` and `vanilla::noise`.

Facts captured on world seed 2026 (raw IEEE bit patterns, all reproduced bit-for-bit by Rust tests):

- Xoroshiro128++ stream: five successive `nextLong` outputs and the resulting 128-bit positional-fork state (`seedLo=2433090872870611415`, `seedHi=6199706540704153528`).
- Seed upgrade constants: golden ratio `-7046029254386353131` and silver ratio `7640891576956012809` (`0x6A09E667F3BCC909`), Stafford13 mix, zero-seed fallback pair, and the MD5-of-UTF-8 hash-seed layout (big-endian 8-byte halves, XOR-folded into the positional factory).
- Random-source widths: `nextDouble` is a `f64` multiply of 53 high bits by a double field holding the `f32` value `1.110223e-16`; `nextFloat` is an `f32` multiply of 24 bits by `5.9604645e-8f32`; bounded `nextInt` uses Lemire reduction.
- Perlin/normal-noise construction: gradient table, `wrap` band `±nextDown(2^24)` with modulus `2^25`, permutation shuffle order, `f32` evaluation chain (smoothstep, gradient-dot associativity, `lerp3` nesting), octave frequency/amplitude bit patterns and the amplitude-modifier skip rule, the two-fork `create` layering with input factor `1.0181268882175227`, Perlin standard deviation `0.2702247831245211`, target deviation `0.3333333333333333`, and normalization factor `(target/3)/(deviation·√2)` with a plain-loop variance estimate.
- Samples for `minecraft:continentalness` (3D and 2D) and `minecraft:erosion` (2D) at four test points, plus first-octave internals (noise offsets, first 16 permutation entries, single-octave `f32` samples) as intermediate checkpoints.
- Target-amplitude summation: vanilla totals octave amplitudes with `List.stream().mapToDouble(...).sum()`, and the JDK 26 runtime in use implements `DoubleStream.sum()` as Kahan compensated accumulation with a cross-checksum fallback — established by `javap` disassembly of the JDK's own `java.util.stream` classes (`DoublePipeline.sum`, `Collectors.sumWithCompensation`, `Collectors.computeFinalSum`), which is a JDK-behavior fact, not Mojang material. A naive left fold differs from the JDK sum by one ulp on the real continentalness amplitudes, so RustMC reproduces the JDK algorithm exactly and pins it with eight adversarial ground-truth vectors (including the overflow-to-NaN path).

The consultation adds the direct dependency `md-5 0.10.6` (RustCrypto, `MIT OR Apache-2.0`), used only to compute the MD5 hash defined by the documented vanilla seed-hash format above — an established library, no first-party or custom cryptography, and no security use. Locked transitives are `digest 0.10.7` (`MIT OR Apache-2.0`), `block-buffer 0.10.4` (`MIT OR Apache-2.0`), `crypto-common 0.1.7` (`MIT OR Apache-2.0`), `generic-array 0.14.7` (`MIT`), `typenum 1.20.1` (`MIT OR Apache-2.0`), and `version_check 0.9.5` (`MIT/Apache-2.0`); each declares an Apache-2.0-compatible option and all were added to the policy gate's reviewed ledger on 1 October 2026. Binary distribution still requires notice collection.

### Session 2 (1 October 2026): density-function graph semantics

Under the same policy, the vanilla 26.3 density-function layer was consulted by reading the owner's local deobfuscated sources in `/tmp` (never committed) plus `javap` on the runtime jar for anonymous wiring classes. Goal: the exact per-node evaluation semantics that the Rust `vanilla::density` module reimplements independently. No vendor code, files, or data entered the repository. Facts captured:

- Pipeline shape: a density graph is a tree of typed nodes; each node compiles once against a per-world context into a sampler whose point form returns `f32` from integer block coordinates. Node kind tags and JSON field names match the public datapack format (constant, noise, gradient, shift/shift_a/shift_b, abs/square/cube/sqrt/half_negative/quarter_negative/reciprocal/negate/squeeze/log/sign, add/sub/mul/div/min/max, pow, round, clamp, lerp, range_choice, interval_select, spline, interpolated, cache, blend_density, blend_alpha/blend_offset/beardifier, find_top_surface, slice, distance_to_point, end_island, old_blended_noise); a node's child may be a bare string that resolves as a registry reference.
- Evaluation order matters bit-exactly and was recorded per node: all combining arithmetic is `f32` except noise input coordinates, which are `f64` (block coordinate times an `f64` scale, plus the `f32` shift widened). Lerp is `first + alpha·(second − first)` with hard branches at `alpha == 0` and `alpha == 1`. Squeeze is `c/2 − c³/24` after `c = clamp(x, −1, 1)`. Half/quarter-negative leak `0.5`/`0.25` only for values `≤ 0`. Mul short-circuits to `0` and div short-circuits to `0` when the left operand is `0`. Min/max short-circuit on range bounds, and ranges are computed interval algebra; the short-circuits preserve values, so a pure point evaluator is bit-identical without them.
- Shift nodes: coordinate factor `0.25`, value factor `4.0F`; `shift_a` evaluates the offset noise at `(x·0.25, y·0.0, z·0.25)` while `shift_b` is transposed — `(z·0.25, x·0.25, 0.0)`; plain `shift` also scales y by `0.25`.
- Gradient (axial) nodes: slope = `(to_value − from_value) / (to_coordinate − from_coordinate)` in `f32`; tiling clamp/repeat/mirrored, with mirrored folding by tile-index parity.
- Range choice is `min_inclusive ≤ v < max_exclusive`; interval select picks the first threshold strictly above `v` (thresholds ascending).
- Splines are piecewise cubic Hermite in `f32`: with `t` the fraction between adjacent locations, value = `lerp(t, y1, y2) + t·(1 − t)·lerp(t, a, b)` where `a = d1·Δx − (y2 − y1)` and `b = −d2·Δx + (y2 − y1)`; outside the location range, linear extension `y + d·(input − loc)`; interval lookup by binary search. Point values and nested sub-splines both appear as JSON `value`s.
- Interpolated nodes downsample then trilinear-fill with `cell_size_xz` and (new in 26.3) `cell_size_y`; reciprocals `1/cell` are `f32`; within a cell the vertical fill is repeated addition of `valueStep = (top − bottom)·cellY⁻¹`, not multiplication — an exactness-relevant detail. The overworld surface-level wrapper uses `xz = 16, y = 1`; noise-limiting wrappers use `xz = 4, y = 8`.
- Find-top-surface samples its upper bound, starts at `floor(upper/cell)·cell` and steps down by `cell` until density `> 0`, else `lower_bound`; the result is constant along y (compiled as a y-slice at 0).
- End-island: legacy random from the world seed, 17 292 stream consumptions, then simplex; per 8-block section it scans neighbor chunk coordinates over `±12`, keeps chunks with self-distance² `> 4096` and noise `< −0.9`, size `(|cx|·3439 + |cz|·147) mod 13 + 9`, `d = clamp(100 − dist·size, −100, 80)`, max-seeded at `−100`, final `(d − 8)/128`; declared range `−0.84375 … 0.5625`.
- Old blended noise (base 3D): five `f64` parameters (overworld `xz_scale 0.25, y_scale 0.125, xz_factor 80, y_factor 160, smear_scale_multiplier 8`); base scale `684.412`; limit FBM first octave `−15` with value factor `(double)(f32)0.99998474`, main FBM `−7` with `12.75`; per-FBM normalization divides by `2^octaves − 1`; smeared Perlin layers quantize the y fraction by subtracting `floor(relY/smear + (f32)1.0E-7)·smear`; the composite is `lerp(clamp(main + 0.5, 0, 1), minLimit, maxLimit)`.
- Distance-to-point metrics in `f32`: euclidean = `sqrt(x² + y² + z²)` with f32 accumulation; squared, manhattan, chebyshev variants.
- Round node modes floor/round/ceil/truncate apply to `input/multiple` and multiply back, passing through when `multiple == 0`; pow folds exponents `0.5/1/2/3` (sign selecting reciprocal wrap) else `(f32)Math.pow((double)…, (double)…)`.
- Caching nodes are value-preserving memoization (last-volume buffer plus last-point entry, deduplicated by input identity); a plain tree evaluator is equivalent.
- World-level wiring (from `javap` of the runtime jar's anonymous classes): noise instances are created once per world, seeded from the positional fork of the world seed by the identifier hash (`minecraft:<name>`), and sampled at absolute block coordinates — there is no per-chunk noise re-fork; `createRandom(terrain)` is the positional hash fork of `minecraft:terrain`; structure/or/vein randoms are hash-named positional factories; blend alpha/offset/beardifier read optional context with defaults `1.0F`/`0.0F`/`0.0F`, so an aquifer-free, structure-free, blend-free pipeline treats them as those constants.
- The built-in registry seeds `zero = 0`, `y = clamped identity gradient over the doubled coordinate bounds (2·MIN_Y, 2·MAX_Y)`, `shift_x = cached shift_a(noise "minecraft:shift")`, `shift_z = cached shift_b(noise "minecraft:shift")`; base-3D noise per dimension: overworld `(0.25, 0.125, 80, 160, 8)`, nether `(0.25, 0.375, 80, 60, 8)`, end `(0.25, 0.25, 80, 160, 4)`; global surface offset constant `−0.50375F`, surface-density cutoff `1.5625F`, cheese target `−0.703125F`, zero-noise constant `0.390625F`, end-island chunk distance `64` (squared `4096`).
- Coordinate packing (verified by `javap` bytecode and a runtime probe against the owner's 26.3 jar): `BlockPos.PACKED_HORIZONTAL_LENGTH = 26`, `PACKED_Y_LENGTH = 64 − 2·26 = 12`; `DimensionType.BITS_FOR_Y = 12`, `Y_SIZE = (1 << 12) − 32 = 4064`, `MAX_Y = (Y_SIZE >> 1) − 1 = 2031`, `MIN_Y = MAX_Y − Y_SIZE + 1 = −2032`. The built-in `y` gradient therefore spans coordinates and values `−4064 … 4062` (the doubled bounds), independent of any dimension's height. The `find_top_surface` codec accepts `lower_bound` in that same doubled range.
- Gradient samplers per tiling, statement-level: clamp-to-edge computes `from_value + (clamp(coordinate, min(from, to), max(from, to)) − from) · factor`; repeat uses `Math.floorMod(coordinate − from, range)`; mirrored-repeat folds by `floorDiv` parity with `range − local` on odd tiles. Factor and final add are `f32`.
- Find-top-surface loop form (code-confirmed): `topY = floor(upper / cell) · cell` (f32 divide, int floor); if `topY ≤ lower` return `lower`; else `for (probe = topY; probe ≥ lower; probe −= cell) if density(x, probe, z) > 0 return probe`; return `lower`. The descent tests `lower_bound` itself inclusively, and the whole computation is compiled behind a y-slice at 0.

No new dependencies were needed beyond the already-locked, already-reviewed `serde_json` (used for parsing operator-provisioned datapack JSON at runtime; the JSON files themselves are never committed).

### Session 3 (1 October 2026): bit-for-bit parity vectors for the density pipeline

Under the same policy, the Rust `vanilla::density` point evaluator was measured against the real 26.3 density engine by *executing* the owner's locally deobfuscated classes headlessly in `/tmp` (a throwaway Java probe harness — tooling only, never committed; all project code remains Rust). The harness bootstraps the version and server registries, parses self-authored JSON trees through the game's own density-function codec, compiles samplers against a trivial compile context, and prints raw `f32` bit patterns. No vendor code, files, or data entered the repository; only numeric facts and this process record are kept.

Facts captured and pinned as regression data:

- 62 pure-tree vectors covering arithmetic folds, signed-zero corners, all unary modes, round modes, clamp/lerp, range/interval boundaries, the four distance metrics, the interpolated point path, slice axis substitution and nested-xz fusion, find-top-surface, the blend-context defaults, splines (linear and nested), and gradient tilings were evaluated by the real 26.3 engine; the resolved vectors plus expected `f32` bit patterns are checked in under `crates/rustmc-server/src/vanilla/testdata/` (RustMC-authored data) and the Rust evaluator now reproduces every recorded bit (`vanilla::density::tests::vanilla_parity_vectors_match_bit_for_bit`).
- 26.3 folds compile-time-known constant operands into dedicated samplers, and the folded forms are *not* value-identical to the general samplers: `mul` with either side constant multiplies directly with no left-zero guard (`0 × ∞ → NaN`), while the general form short-circuits `left == 0 → +0.0`; `div` with a constant left operand divides directly (no guard, `0/0 → NaN`), and `div` by a constant right operand becomes multiplication by the precomputed `f32` reciprocal `1.0F/c` — which rounds differently from a division (e.g. `7 ÷ 3 → 2.3333335`, raw bits `1075139926`, not the naive-division `1075139925`). The left-constant check runs before the right-constant check. RustMC's evaluator mirrors these folds exactly.
- Java `Math.signum(float)` maps `+0.0 → +0.0`, `−0.0 → −0.0` (and NaN → NaN), unlike the usual IEEE signum returning `±1.0` at zero; the `sign` unary node pins this (`sign_negzero` expected bits `0x80000000`).
- 26.3 node layout confirmed: operator nodes live in a dedicated subpackage with kind records (binary/unary/clamp/lerp/range-choice/interval-select/pow/round/cache/slice/interpolated/find-top-surface/blend/spline) and generator nodes elsewhere; the runtime density-function type registry holds exactly 44 ids including `minecraft:end_outer_islands` (the earlier `end_island` naming in these notes corresponds to that id).
- JSON decode specifics pinned by execution: the distance-to-point `point` field decodes only as a three-element integer array (the vec3 codec rejects an object form); spline point `value` entries are a plain float or a nested `{coordinate, points}` object rather than a density-function tree; a bare JSON number parses directly as a constant function.
- Cache nodes are rejected by the engine when compiled without the graph-level deduplication pass ("Cannot compile cache before it has been deduplicated"), so no standalone golden vector exists for them; RustMC keeps its value-preserving passthrough, which is equivalent whenever caching is disabled — the same configuration the vanilla probe used (an uncached empty context evaluates cache nodes transparently).
- Pow folds base-constant before exponent-constant: a constant base evaluates `(float)Math.pow((double)c, (double)e)` per sample, and a constant exponent `2/3/0.5/1` folds to square/cube/sqrt/identity with negative exponents wrapping in a reciprocal — matching the evaluator behavior recorded in Session 2 and now exercised by the vectors.
- The block-volume path is not value-identical to the point path, and vanilla world generation fills its density caches through the volume path, so RustMC replicates it exactly. Captured with the same harness over full buffer dumps (7 volume vectors, 568 slots, pinned in the same testdata directory): the general volume loops for `mul` and `div` apply the raw operation per slot with no left-zero guard (so `0 × ∞ → NaN` and `0 ÷ 0 → NaN` in volume buffers where point sampling returns `0`, and `0 × −0.5 → −0.0` keeps its sign); volume `min`/`max` replace only on a strict comparison, so `min(+0.0, −0.0)` keeps `+0.0` in a buffer where the point path returns `−0.0`.
- Interpolated volume semantics pinned by the buffer captures: an aligned volume whose per-axis step equals the cell size (or whose size is 1) passes straight through to the input's volume path; otherwise a cell-corner grid is filled through the input's *volume* path and each cell is interpolated with alphas computed as `index · (1.0F/cell)` (reciprocal multiply, unlike the point path's division) with the vertical direction filled by repeated addition of `valueStep = (top − bottom) · (1.0F/cellY)` starting at `bottom + valueStep · firstLocalY`; non-unit-step volumes are filled at unit stride and then gathered; the point path itself samples its 2×2×2 corner grid through the input's volume path on an aligned cell-stepped volume. Nested interpolated wrappers recurse through these dispatches, and the corner-grid carry reuses the previous cell's top values as the next cell's bottom values.

No new dependencies were needed beyond the already-locked, already-reviewed `serde_json` (used for parsing operator-provisioned datapack JSON at runtime; the JSON files themselves are never committed). The parity harnesses in `/tmp` are disposable consultation tooling under the ADR-0014 amendment and are not part of the repository; the project's implementation and tests are Rust-only.

### Session 4 (1 October 2026): runtime loading of operator-provisioned worldgen data

This session consulted no deobfuscated code. It exercised the Rust loader `crates/rustmc-server/src/vanilla/worldgen.rs` against the operator's own local copy of the 26.3 data files (untracked under `.rustmc-local/`; no Mojang file, snippet, or noise value is committed or reproduced here). What follows are shape facts about the public datapack format as observed in those files, plus one process record.

- Datapack layout consumed: `data/<namespace>/worldgen/noise/*.json`, `data/<namespace>/worldgen/density_function/**/*.json` (directory nesting is part of the identifier), and `data/<namespace>/worldgen/noise_settings/*.json`, keyed as `<namespace>:<path>` — the same string form used by registry references inside density JSON and by the `noise_router` section.
- 26.3 `worldgen/noise` files store the *resolved* octave form: `base_amplitude` and `base_octave` are always present; `octave_count` (implicit 1), `amplitude_modifiers` (implicit all-`1.0`), and `normalize` (implicit `true`) are omitted at their defaults, so single-octave definitions carry only the two required fields. The loader applies these defaults and pads modifier lists to the octave count with identity.
- 26.3 `noise_settings` documents are slim: the generator fields the loader reads are `noise.min_y`/`noise.height`, `sea_level`, and `noise_router` entries that are either registry-id strings or inline density-function objects (`chunk_surface_level` optional). The remaining sections (`aquifers`, `debug_functions`, `spawn_target`, material rules, etc.) belong to later phases. As concrete numbers, the overworld bounds asserted by the smoke test (`min_y = -64`, `height = 384`, `sea_level = 63`) are the publicly documented 26.x overworld height facts, not vendor-data excerpts.
- Data-side density specifics observed: `zero.json` is a bare JSON number (`0.0`) accepted by the codec as a constant function — the registry document compiler now mirrors that; `shift_x.json` is `{"type": "cache", "input": {"type": "shift_a", "noise": "minecraft:offset"}}`, i.e. 26.3 data names the shift noise `minecraft:offset` (the built-in fallback recorded in Session 2 under the name `minecraft:shift` is only used when no datapack document exists); `y.json` is the gradient spanning `-4064 … 4062`, matching the coordinate-packing fact from Session 2.
- Process: an `#[ignore]`-gated smoke test loads the operator root (path from `RUSTMC_VANILLA_DATA`, defaulting to the project-local directory), compiles the full overworld `noise_router` graph — every reference resolves against the loaded documents plus the four built-ins — and samples the real `final_density` across the documented vertical range, asserting finiteness. It prints pass/fail only; no data values are emitted. Loader unit tests run entirely on synthetic self-authored numbers under a temporary directory.

Continuation of the same day (slice D, measurement facts — no deobfuscated code consulted):

- Surveying which density-function node types the operator's 26.3 datapack actually uses found 42 distinct type names, including `blend_density`, `blend_alpha`, `blend_offset`, `beardifier`, `shift_a`, `shift_b`, `find_top_surface`, `end_outer_islands`, and `cache`; no aquifer-related node type appears anywhere in the density graphs. Together with the `aquifers` section of the slim `noise_settings` documents this confirms the split recorded in Session 2: aquifer and structure effects are applied by the runtime context, not the JSON graphs, so a structure-free, aquifer-free pipeline evaluates them at their documented defaults (`1.0F` alpha, `0.0F` offset/beardifier).
- Column-height extraction from the compiled router is a per-block descending scan for the first `final_density` sample strictly greater than zero, with the sea rule reporting `sea_level − 1` where the scan finds terrain below it. A grid-refined shortcut (scan the y-cell grid, then rescan the cell above the first positive grid sample) is implemented but *not* exact for the overworld graph: the outer `min` with the per-block `noodle` carving can push a y-grid sample itself to ≤ 0 while the surface block between samples stays positive. Measured against the operator's own save, the shortcut missed 7 of 2,401 columns and the exhaustive scan is the default. This is an observed RustMC behavior fact, not a vendor claim: vanilla's own block filling evaluates density per block as well.
- The first end-to-end measurement against the owner's seed-2026 save (2,401 columns, every 16th in the explored square): 94.59% exact height match; all six published worksheet points reproduce; the residuals break down as 42 tree-trunk columns and 11 ruin-block columns (features — outside a density-only surface by construction), 13 water-surface columns (fluid finalization), and 64 terrain-topped columns concentrated near structure footprints and shallow-aquifer bands (runtime-context effects the pipeline currently evaluates at their no-context defaults). Aggregate numbers only; the save itself stays local.

### Session 5 (1 October 2026): aquifer runtime semantics (T1 slice E)

Under the same policy, the 26.3 runtime aquifer was consulted by decompiling the owner's local server jar classes in `/tmp` (throwaway tooling; the 26.3 jar ships unobfuscated class and member names in this era, so consultation was a vineflower decompile only — no file, snippet, or vendor data entered the repository). The Rust `vanilla::aquifer` module was then written independently from these recorded facts. Facts captured:

- The aquifer is a runtime consumer, not a density-graph node. The chunk constructor builds it from the `noise_settings` `aquifers` section — six required density fields: `barrier`, `fluid_level_floodedness`, `fluid_level_spread`, `lava`, `exclusion`, `surface_level` — plus a random factory derived as the positional hash fork of the name `minecraft:aquifer` from the world-seeded positional factory, itself forked once more. A dimension without the section uses the global fluid picker alone. The settings document also carries `default_fluid` (the overworld names its flooding fluid).
- Block filling scans each column top-down; the block substance at a position is the aquifer's answer given the raw density sample, and the stored heightmap tracks the highest position that is *solid or fluid* — so the fluid surface is part of the top by construction, and the aquifer can additionally raise terrain through barrier pressure or move fluid surfaces.
- Cell grid: spacing 16 blocks in x/z and 12 in y; the cell containing a position is anchored at `((x − 5) >> 4, floorDiv(y + 1, 12), (z − 5) >> 4)`. Each cell's random center draws three bounded integers in x, y, z order from the factory seeded at the cell: ranges 10, 9, 10, giving center `(gx·16 + rx, gy·12 + ry, gz·16 + rz)`.
- Substance decision for a density-empty position: the global picker is `y < min(−54, sea_level) ? lava-at−54 : sea-fluid-at-sea_level`, and a status fills fluid strictly below its level. Above a chunk-scoped skip height (`y > skipSamplingAboveY`) positions take the global fluid without neighborhood work. Otherwise the 12-cell neighborhood (anchors with `dx ∈ {0,1}`, `dy ∈ {−1,0,1}`, `dz ∈ {0,1}` in that loop order) keeps the four nearest cell centers (insertion replacing on `>=` displacement); pairwise similarity is `1 − (d2 − d1)/25`. Branch order: similarity of the two nearest `≤ 0` → the nearer status; water status with global lava below → water; otherwise a first barrier adjustment `sim₁₂ · pressure(status₁, status₂)` at the position — solid when `density + barrier > 0`; then if the third-nearest is within similarity, a second adjustment multiplied by `sim₁₂·sim₁₃`, else a third multiplied by `sim₁₂·sim₂₃`; failing all, the nearer status alone. The barrier noise is sampled at most once per block across those branches.
- The pressure between two cell statuses at a block y: a mixed lava/water pair gives the constant `2.0`; equal fluid levels give `0.0`; otherwise with `diff = |l1 − l2|`, `avg = 0.5·(l1 + l2)`, `above = y + 0.5 − avg`, `edge = diff/2 − |above|`: above the midpoint the gradient is `edge/1.5` when `edge > 0` else `edge/2.5`; below it, `3.0 + edge` divided by `3.0` when positive else `10.0`. The barrier noise is consulted only when the gradient is within `±2.0`, and the result is `2.0·(noise + gradient)`.
- Cell status: a preliminary surface height per cell comes from sampling the `surface_level` density at quart-quantized (4-block) x/z coordinates, y fixed at 0, floored to an integer. A candidate's fluid level is decided by sampling the preliminary surfaces of a 13-entry chunk ring (offsets `[(0,0), (−2,−1), (−1,−1), (0,−1), (1,−1), (−3,0), (−2,0), (−1,0), (1,0), (−2,1), (−1,1), (0,1), (1,1)]` in 16-block units, each surface raised by `+8`): the center sample against a ±12-block window can short-circuit to the global level; the `exclusion` density above zero at the position forces "no flooding"; otherwise flooding uses `fluid_level_floodedness` clamped to `[-1, 1]` mapped through `(1, 0) → (−0.3, 0.8)` for full flooding and `(1, 0) → (−0.8, 0.4)` for partial, where the interpolation factor counts depth below the lowest neighboring surface up to 64 blocks and only applies to positions the preliminary surfaces mark as under fluid. A partially flooded level is randomized per `(16, 40, 16)` cell: an offset from the `fluid_level_spread` density scaled by `10.0` and quantized down to multiples of 3, centered on the cell's middle height, taking the minimum with the lowest neighbor surface `+8`. A status becomes lava when the level is at or below `−10`, is not the sentinel `−32512` (`DimensionType.WAY_BELOW_MIN_Y`, the packed-coordinate floor shifted into block space), the global fluid there is not lava, and the `lava` density sampled per `(64, 40, 64)` cell exceeds `0.3` in absolute value.
- The chunk-scoped skip height bounds neighborhood sampling above every plausible fluid surface in the chunk: it scans the quart grid of preliminary surfaces over the chunk's sampled x/z span (`min·16` to `max·16 + 9`), takes the maximum, adds `+8`, and rounds up to the cell above, minus one block (`skipGridY = floorDiv(max + 8 + 12, 12) + 1`, value `skipGridY·12 + 11 − 1`).
- Two debug switches (fluid-generation disabling, aquifer visualization) are constant-off in production, and the per-block fluid *update scheduling* flags are tick-post-processing only — neither affects the filled heightmap, so extraction ignores them.

Post-implementation measurement (same save and sample as slice D): with the runtime aquifer wired into the surface scan, the exact-height match on the 2,401-column sample rose from 94.59% to 96.00% (2,305 columns) — above the 95% acceptance gate — and all six published worksheet points still reproduce exactly. The remaining 96 mismatches concentrate in the tree/ruin feature columns already attributed to features and in slope/lake-edge residuals. Aggregate numbers only; the save and the vendor data root stay local.

No new dependencies were needed; the slice uses only the already-locked `rustmc-server` internals.

### Session 6 (1 October 2026): biome placement semantics and table capture (T2 slice F)

Under the same policy, 26.3 biome placement was consulted by decompiling the owner's local (unobfuscated) server jar classes in `/tmp` — throwaway tooling; nothing consulted entered the repository except the numeric and structural facts below. The Rust `vanilla::biome` module was then written independently from those facts. Facts captured:

- Climate parameter math: coordinates are quantized with `QUANTIZATION_FACTOR = 10000.0F`, i.e. `quantizeCoord(v) = (long)(v · 10000.0F)` — an `f32` multiply then Java truncation toward zero (so e.g. `1.1F` lands at `11000` after `f32` rounding). Each dimension of a placement entry is an inclusive quantized integer interval `[min, max]`; the distance from a target to an interval is zero inside it, else the gap to the nearer end. An entry is six intervals (temperature, humidity, continentalness, erosion, depth, weirdness) plus a quantized scalar `offset`. "Fitness" against a target is the sum of the squared interval distances over the six dimensions plus the squared offset — the offset enters as a constant penalty, not a distance.
- Selection: the game searches a deterministic 7-dimensional R-tree (19 children per node; build sorts/buckets by parameter-space magnitude and cost; search replaces only on a *strictly* closer comparison, with a thread-local warm start from the previous cell). The class also ships a brute-force variant used for testing, which keeps the *earlier* entry on exact fitness ties. Because ties between the R-tree and brute-force orders agree whenever the first minimum in table order is strictly nearer than every later entry, RustMC starts from brute force in table order; the 2,401-column measurement below shows no tie-order pathology.
- Wiring (from `javap` of `NoiseRouter.createClimateSampler` and `MultiNoiseBiomeSource`): the six climate axes come straight from the dimension's compiled `noise_router` fields, with `vegetation` feeding the humidity axis and `ridges` feeding the weirdness axis. Placement is resolved on a 4-block quart grid: the per-chunk resolver buffers all six densities over a `DensityVolume` at unit-quart stride and each cell's target is sampled at its bottom block coordinates (`quart · 4`), depth included. The stored chunk biome state is this resolver's answer wrapped by the world blender, so structure-border blending is a known residual of a structure-free pipeline.
- The overworld placement table is *code-side*: the datapack biome JSON documents carry no placement parameters (their keys are attributes, carvers, downfall, effects, features, has_precipitation, temperature), and `worldgen/noise_settings/overworld.json` only names the preset. `MultiNoiseBiomeSourceParameterList.Preset.OVERWORLD` builds the list by invoking `OverworldBiomeBuilder.addBiomes`, which emits static entries plus spline-computed families (e.g. desert/badlands alternating at the locations of the erosion-offset spline, snowy-taiga variants at offset-spline locations; plains-family mid entries pin depth to the point `0.0F`).
- Capture process: a throwaway Java harness bootstrapped the owner's 26.3 version and server registries headlessly, built the `OVERWORLD` preset's parameter list at runtime, and dumped it in table order as quantized integers, one line per entry (`idx|biome_id|t=[a-b]|h=…|c=…|e=…|d=…|w=…|off=N`). The capture contains 7,594 entries and every overworld offset is `0`. Consistent with the ADR-0014 amendment's data stance, the bulk table is treated like datapack content: it is provisioned by the operator beside the worldgen data root (`rustmc/biome_placement/overworld.psv`, untracked) and is *never* committed; the repository keeps only the loader, the format, and self-authored synthetic tables in tests.

Post-implementation measurement against the owner's seed-2026 save (same 2,401-column sample; aggregate numbers only): all six published worksheet points now reproduce their stored biomes exactly (`forest`, `taiga`, `old_growth_birch_forest`, `forest`, `plains`, `old_growth_pine_taiga`), and biome identity matched 2,399 of 2,401 columns (99.92%) — far above the 95% T2 gate — while the exact-height match remained 96.00% as recorded in session 5. The two biome residuals sit at blender-affected borders. No new dependencies were needed; the slice uses only `std` inside `rustmc-server`.

### Session 7 (1 October 2026): surface material rule semantics (T2 slice G)

Under the same policy, the 26.3 surface material system was consulted by decompiling the owner's local (unobfuscated) server jar classes in `/tmp` — throwaway tooling; nothing consulted entered the repository except the numeric and structural facts below. The Rust `vanilla::surface` module was then written independently from those facts. Facts captured:

- Wiring (from `RandomState`, `MaterialSystem`, `NoiseBasedChunkGenerator`): the material system is constructed with the settings `default_block` (the overworld names stone — the filler block the surface rules replace), the sea level, the router's `chunk_surface_level` density, and the world-seeded positional random. `getOrCreateNoise(key)` instantiates a `NormalNoise` from the positional hash of the key (`fromHashOf(identifier)`), cached per name; `getOrCreateRandomFactory(name)` is `fromHashOf(name)` forked once more. The chunk generator runs `buildSurface` *before* carvers, and rules only ever recolor solid positions.
- The per-column descent: start at the world surface height + 1 and walk down; an air position resets the stone-above counter and clears the water height to a `MIN_VALUE` sentinel; a fluid position latches the water height to `y + 1` once per run; a solid position lazily recomputes the next ceiling-stone row when reached (a downward lookahead for the first non-solid, else the `WAY_BELOW_MIN_Y` floor), increments the stone-above counter, sets stone-below as `y − ceiling + 1`, and applies the rule at that row.
- Per-XZ quantities: the surface depth is eager — `(int)(surfaceNoise(x, 0, z) · 2.75 + 3.0 + positional.at(x, 0, z).nextDouble() · 0.25)`; the secondary surface and the `floor(chunkSurfaceLevel(x, 0, z)) + surfaceDepth − 8` preliminary floor are lazy; the steep gradient uses WORLD_SURFACE_WG differences with the chunk-local clamp `min(lx + 1, 15)`/`max(lx − 1, 0)` (zero at clamped chunk edges).
- Condition semantics (every predicate verified against its class): `biome` matches a holder set of plain ids (string or list; the overworld data uses no tags); `noise_threshold` is an inclusive `min ≤ v ≤ max` on a raw noise widened from `f32` to `f64`, sampled at `(x, 0, z)` or `(x, y, z)` by its `is_3d` flag; `vertical_gradient` is certain at or below the true anchor, never at or above the false anchor, and between them holds with probability `Mth.map(y, below, above, 1.0, 0.0)` decided by the first `nextFloat` of a named random factory at the block position (Java `Mth.map` = `(v − inMin)·(outMax − outMin)/(inMax − inMin) + outMin`, operand order preserved); `y_above` is `y + (addStoneDepth ? stoneAbove : 0) ≥ anchorY + surfaceDepth · multiplier`; `water` holds trivially when no water column was latched, else the analogous comparison against `waterHeight + offset + surfaceDepth · multiplier`; `stone_depth` is `depth ≤ 1 + offset + (addSurfaceDepth ? surfaceDepth : 0) + secondary` with `secondary = range == 0 ? 0 : (int)Mth.map(surfaceSecondary, −1, 1, 0, range)`, `floor` reading the above-depth and `ceiling` the below-depth; `steep` is `gradX ≤ −4 || gradZ ≥ 4`; `hole` is `surfaceDepth ≤ 0`; `above_preliminary_surface` is `y ≥ minSurfaceLevel`; `not` inverts. A `temperature` condition is registered in code but unused by the overworld data.
- Vertical anchors (verified by decompiling the three record classes): `absolute: n` → `n`; `above_bottom: n` → `minGenY + n`; `below_top: n` → `minGenY + height − 1 − n`; a bare integer is absolute.
- Rule semantics: `sequence` returns the first non-null child (a size-one sequence is a passthrough; empty is rejected); `condition` gates `then_run`; `block` emits its `result_state` (an id string or an object whose `Name` is kept); `bandlands` returns `clayBands[(y + round(clayBandsOffset(x, 0, z) · 4.0F) + 192) % 192]` with the `·4.0F` multiply staying `f32` and `Math.round(float) = floor(x + 0.5)`; `ore_vein` {ore/raw/filler blocks, `raw_ore_chance` (0.02 in the overworld), `density`/`richness`/`filler_gap` density references} evaluates: `density ≤ 0` → skip; then three ordered draws on the factory named `minecraft:ore` at the position — `nextFloat() > density` → skip; `nextFloat() < richness && fillerGap < 0` → (`nextFloat() < rawOreChance` ? raw : ore) : filler — all `f32` comparisons; the prefill half of the rule cannot change a point sample of the top row.
- The badlands band table program (192 entries, `minecraft:clay_bands` positional stream): fill terracotta; an orange loop that increments twice per pass (the decompiled body advances `i += nextInt(5) + 1` and paints, and the loop header advances again by one); `makeBands` for yellow (base width 1), brown (2), red (1), each drawing a band count `nextIntBetweenInclusive(6, 15) = nextInt(10) + 6`, per band a width `base + nextInt(3)` and start `nextInt(192)`, painting clipped at the table end; then a white pass with count `nextIntBetweenInclusive(9, 15)` painting at `start` and optional neighbors gated by `nextBoolean`, stepping `start += nextInt(16) + 4`. `nextIntBetweenInclusive(min, max) = nextInt(max − min + 1) + min` was verified.
- Random primitive verification (decompiled `RandomSource`/`XoroshiroRandomSource`): `nextFloat = (float)nextBits(24) · 5.9604645E-8` and `nextDouble = nextBits(53) · 1.110223E-16` match the existing Rust implementation, and `nextBoolean` is `(nextLong() & 1) != 0` — the low bit of the *next long*, not a one-bit draw; the Rust random module gained a matching `next_bool` used by the band table.
- Datapack census of the operator's own 26.3 data (shape facts only, files stay local): the overworld root `material_rule` is a sequence of the bedrock floor, copper and iron `ore_vein` rules, the `above_preliminary_surface`-gated `overworld/surface` subtree, and the underground subtree; condition references resolve to `worldgen/material_condition` documents; the vein densities reference `worldgen/density_function/overworld/ore_vein/*`; every noise the material system samples (`surface`, `surface_secondary`, `clay_bands_offset`, plus the condition noises) exists as a `worldgen/noise` document.
- Deliberately out of slice G scope (documented residuals): the hardcoded eroded-badlands pillar/roof and frozen-ocean iceberg column extensions inside the material system (their six extra noises were observed in the constructor but not implemented), and feature-driven top blocks (podzol, snow, water plants), which belong to T4.

Post-implementation measurement against the owner's seed-2026 save (same 2,401-column sample; aggregate numbers only): with the data-driven surface-rule tree evaluating the topmost solid row of every height-matched column, top-block identity matched 2,200 of 2,305 columns — **95.44%**, above the ≥95% T2 gate — while exact height (96.00%) and biome (99.92%) were unchanged. Five of the six published worksheet points reproduce their stored top block; the sixth is a podzol column under old-growth pines (a feature cap, not a surface rule). The 105 residuals are dominated by that same class: 65 podzol-topped columns the rule tree correctly leaves as grass/coarse dirt, plus shore and sub-fluid bookkeeping swaps (water↔grass, 9), noise-patch edges (gravel/sand/mossy/coarse, ~25), and a long tail of stone-type flips. The podzol, dirt_path, and mossy_cobblestone classes all sit at feature-decorated positions and are capped until T4. No new dependencies were needed; the slice uses only `std` inside `rustmc-server` and `serde_json` already locked for the density loader.

### Session 8 (1 October 2026): 3D substance baseline and carver data census (T3 slice H, measurement half)

This half-slice consulted no new vanilla runtime code; it adds measurement
tooling and records what the existing pipeline already reproduces. Facts:

- Cave shapes in 26.3 are composed inside the density graph: the operator's
  datapack resolves `minecraft:overworld/final_density` through
  `overworld/caves/{entrances,noodle,pillars,spaghetti_2d,spaghetti_2d_thickness_modulator,spaghetti_roughness_function}`
  and the `cave_layer`/`cave_cheese`/`base_3d_noise` terms, so the graph
  compiled for T1 already samples them. There is no `caves` section in the
  26.3 `noise_settings` document (router slots are exactly
  `chunk_surface_level`, `continents`, `depth`, `erosion`, `final_density`,
  `ridges`, `temperature`, `vegetation`).
- The remaining cave machinery is the registry carver runtime: biome
  documents list `carvers` `["minecraft:cave", "minecraft:cave_extra_underground", "minecraft:canyon"]`,
  and the `worldgen/carver` registry documents record (structure facts):
  the cave type starts at y from `above_bottom 8` to `absolute 180` with a
  `very_biased_to_bottom` count 0..14, probability 0.15 (0.07 for the
  extra-underground variant, capped at y 47), thickness trapezoid 0..3
  plateau 1, floor level uniform −1.0..−0.4, horizontal radius ×0.7..1.4,
  vertical ×0.8..1.3, room-vertical ×0.1..0.9, with the
  `weird_thickness_bias` flag; the canyon type runs y 10..67, probability
  0.01, with shape parameters (thickness trapezoid 0..6 plateau 2,
  `y_scale` 3.0, `width_smoothness` 3, vertical rotation ±0.125, distance
  and horizontal-radius factors 0.75..1.0).
- Baseline (owner's seed-2026 save, same 2,401-column stride-16 grid,
  categories air-family/fluid/solid, `vanilla_oracle substance`):
  **96.44% exact over 338,217 sampled positions** (near-surface band
  96.76%, 8–63 below surface 97.15%, deeper 95.89%). Residuals: 9,186
  vanilla-air/ours-solid, 1,771 vanilla-fluid/ours-solid, 842
  ours-air-bonus, plus small fluid bookkeeping tails.
- Attribution by probing failing columns (e.g. (−256, −256)): vanilla air
  occurs where our `final_density` sample is clearly positive (+0.019 to
  +0.13), and the disagreement bands are deepest-heavy where carver counts
  are biased (`very_biased_to_bottom`), so the gap is the registry carvers
  carving through solid density, not a density-graph divergence. The
  bonus-carve tail (842) sits at cheese-graph sign boundaries and is
  attributed to the documented `f32` noise approximations.
- The oracle gained a per-column substance profile reader (sections decode
  palettes per Y; unsaved spans are air), a category classifier for the
  air family (`air`, `cave_air`, `void_air`, `structure_void`) and the two
  fluids, `compare_substance` with depth bands, per-band marginals, and a
  capped fail-position dump, and single-column `column` and `substance`
  CLI modes. `VanillaGenerator` exposes `raw_density`/`substance`.
  No new dependencies; no generator behavior changed.


### Session 9 (1 October 2026): carver runtime consultation (T3 slice H, implementation half)

Deobfuscated 26.3 classes consulted knowledge-only (nothing recorded here
enters the repository as code): `WorldCarver`, `CaveWorldCarver`,
`CanyonWorldCarver`, `NoiseBasedChunkGenerator.generateCarvers/
applyCarvingMask`, `CarvingMask`, `CarverOutput`, `WorldgenRandom`,
`LegacyRandomSource`, `SingleThreadedRandomSource`, `BitRandomSource`,
`XoroshiroRandomSource`, `RandomSource`, value providers (`UniformInt`,
`VeryBiasedToBottomInt`, `UniformFloat`, `TrapezoidFloat`, `ConstantFloat`,
`IntProviders`, `FloatProviders`), `UniformHeight`, `VerticalAnchor`,
`WorldGenerationContext`, `BiomeGenerationSettings`, `Aquifer`, `Mth`.
Numeric/structural facts:

- Orchestration: one `WorldgenRandom` over a legacy source; for each of
  the 17×17 source-chunk offsets (dx outer, dz inner, both −8..8) the
  carver list is taken from the biome at the source chunk's corner quart
  `(4·x, 0, 4·z)`; for carver index i: `setLargeFeatureSeed(worldSeed + i,
  srcX, srcZ)` then `isStartChunk` (`nextFloat() <= probability`) gates
  `carve`, which stamps into a per-target-chunk `CarvingMask` spanning
  `minGenY + 1` to `minGenY + genDepth − 1 − 7` (overworld: −63..312).
  `setLargeFeatureSeed`: `setSeed(s); a = nextLong(); b = nextLong();
  setSeed(x·a ^ z·b ^ s)`. Set-bit visits run top-down per column; each
  masked position not tagged uncarvable is replaced by
  `aquifer.computeSubstance(x, y, z, 0.0)` (null is impossible at
  density 0), so carved solids become aquifer air/water/lava; a sticky
  grass flag can recolor the dirt below via `topMaterial` (category-
  neutral; not modeled for the substance metric).
- `RandomSource.createThreadLocalInstance(seed)` returns
  `SingleThreadedRandomSource`: the 48-bit legacy LCG
  (`seed = (seed ^ 0x5DEECE66D) & (2^48−1)`, advance
  `seed·0x5DEECE66D + 0xB`, output `seed >> (48 − bits)`), NOT xoroshiro.
  All carver randomness is therefore legacy-LCG: the outer per-source-chunk
  stream and each tunnel's private stream. `nextInt(bound)` uses the
  power-of-two shortcut `(bound · next(31)) >> 31` else the rejection loop
  `sample % bound` while `sample − modulo + bound − 1 < 0`; `nextLong =
  (next(32) << 32) + next(32)`; `nextFloat = next(24) · 5.9604645E-8F`;
  `nextDouble = ((next(26) << 27) + next(27)) · 1.110223E-16F`.
- `carveEllipsoid(chunk, x, y, z, hR, vR, mask, skip)`: bail if
  `|x − cx−8| > 16 + 2·hR` or same for z; x indices
  `max(floor(x−hR) − minX − 1, 0)..=min(floor(x+hR) − minX, 15)` (same for
  z); `yLo = max(floor(y−vR) − 1, mask.minY)`, `yHi = min(floor(y+vR) + 1,
  mask.maxY)`; for each column with `xd² + zd² < 1` where
  `xd = (wx + 0.5 − x)/hR`, `zd = (wz + 0.5 − z)/hR`, walk `worldY` from
  `yHi` down while `worldY > yLo` and carve when
  `!skip(xd, (worldY − 0.5 − y)/vR, zd, worldY)`.
  `canReach(chunk, x, z, step, total, thickness)`:
  `xd² + zd² − (total − step)² ≤ (thickness + 2 + 16)²` (float add, then
  double).
- Cave carver (`minecraft:cave`) draw order per start source chunk, outer
  stream: `count` (`very_biased_to_bottom`: `min +
  nextInt(nextInt(nextInt(span+1)+1)+1)`); then per cave: `x =
  src·16 + nextInt(16)`, `y = uniform height` (`min>max → min` else
  `nextInt(max−min+1) + min`; anchors `absolute n`, `above_bottom n →
  minGenY + n`), `z`, `horizontalRadiusMultiplier`, `verticalRadiusMultiplier`,
  `startVerticalRadiusMultiplier` (default constant 1.0), `floorLevel`
  (uniform float `nf·(max−min)+min`); skip test
  `yd ≤ floorLevel || xd²+yd²+zd² ≥ 1`; `tunnels = 1`; if
  `nextInt(4) == 0`: room (`yScale = roomVerticalRadiusMultiplier`,
  `thickness = 1 + nf·6`, ellipsoid at `(x+1, y, z)` with radii
  `1.5 + sin(π/2)·thickness` and `·yScale`), then `tunnels += nextInt(4)`.
  Per tunnel: `hRot = nf·2π`, `vRot = (nf − 0.5)/4`,
  `thickness = provider` then `weird_thickness_bias`: extra
  `nextInt(10) == 0` → `·(nf·nf·3 + 1)`; `distance = 112 − nextInt(28)`;
  fork with `seed = nextLong()` into a fresh legacy LCG.
- Tunnel walk (private stream): `splitPoint = nextInt(dist/2) + dist/4`;
  `steep = nextInt(6) == 0`; per step `hR = 1.5 + sin(π·step/dist)·thickness`,
  `vR = hR·yScale`; move `x += cos(hRot)·cos(vRot)`, `y += sin(vRot)`,
  `z += sin(hRot)·cos(vRot)`; then `vRot = vRot·(0.92 if steep else 0.7)`,
  `vRot += xRota·0.1`, `hRot += yRota·0.1`, `xRota·=0.9`, `yRota·=0.75`,
  `xRota += (nf−nf)·nf·2`, `yRota += (nf−nf)·nf·4`; at `splitPoint` with
  `thickness > 1` two recursive forks (`thickness = nf·0.5 + 0.5` each,
  `hRot ∓ π/2`, `vRot/3`, yScale 1.0, seeds `nextLong()`) and return;
  else if `nextInt(4) != 0`: `canReach` else return, carve ellipsoid with
  radii `hR·horizontalMultiplier`, `vR·verticalMultiplier`.
- Canyon carver (`minecraft:canyon`): one walk per start chunk; outer
  draws: `x` (+`nextInt(16)`), `y` int height, `z`, `hRot = nf·2π`,
  `vRot = verticalRotation provider`, `yScale`, `thickness`,
  `distanceFactor`, then tunnel seed `nextLong()`. Private stream:
  width factors (per genDepth entry: index 0 or `nextInt(widthSmoothness)
  == 0` → `wf = 1 + nf·nf`; store `wf·wf`); per step
  `hR = 1.5 + sin(step·π/dist)·thickness`, `vR = hR·yScale`,
  `hR ·= horizontalRadiusFactor` draw, then
  `vR = (defaultFactor + centerFactor·(1 − |0.5 − step/dist|·2)) · vR ·
  randomBetween(0.75, 1.0)` (a draw); move as above with 0.05/0.8/0.5
  rotation damping; skip `nextInt(4) == 0`; `canReach` abort; ellipsoid
  skip test `(xd²+zd²)·widthFactor[y − minGenY − 1] + yd²/6 ≥ 1`
  (index is `worldY − minGenY − 1`, floor-divided like Java).
- `BiomeGenerationSettings` in 26.3 stores a single flat `carvers`
  `HolderSet` in JSON list order; the plains document lists cave,
  cave_extra_underground, canyon — carver index for the seed is the list
  position. Provider registries: int `constant/uniform/biased_to_bottom/
  very_biased_to_bottom/clamped/weighted_list/clamped_normal/trapezoid`,
  float `constant/uniform/clamped_normal/trapezoid`; trapezoid float
  sample `min + nf·plateauEnd + nf·plateauStart` with
  `plateauStart = (max−min−plateau)/2`. `CarvingMask` storage is a bitset
  indexed `y − minY + (z + 16·x)·height`; terrain-only replay never
  encounters the uncarvable tag (barrier-family blocks), and the data
  root ships no block tags, so the tag check is out of scope for the
  oracle metric.

### Session 10 (1 October 2026): vein-rate ground truth and census attribution (T4 slice J)

Vein-rate semantics were established two ways under the ADR-0014 knowledge-only
amendment: (a) by executing the owner's locally fetched/deobfuscated official Java
26.3 server classes headlessly in `/tmp` via a throwaway Java probe harness (tooling
only, never committed; all project code remains Rust), and (b) by running that
official server headless with structures disabled at seed 2026 to generate a 100-chunk
ground-truth world, censused read-only by RustMC's own Rust oracle. Classes exercised
for (a): `NormalNoise` (codec, `create`, normalization), `NoiseStack`,
`Noises.instantiate`, the `NoiseFunction` node, `InterpolatedFunction`,
`CacheFunction` and `SamplerContext`, the density `getDensitiesInChunk` entry point,
`OreVeinRule`, and `VeinType`. No vendor code, files, or data entered the repository;
only numeric/structural facts and this process record are kept. Noise construction
semantics (26.3):

- NormalNoise codec: `base_amplitude` optional default `1.0`, range `[1e-5, 1e6]`;
  `base_octave` required int `[-32, 32]`; `octave_count` optional default `1`;
  `normalize` optional default the `ENABLED` mode; `amplitude_modifiers` optional
  default empty. LEGACY normalization is reachable only via the `"legacy"` string
  (the old-parity path) and is NOT used by ore-vein data.
- Normalize amplitude formula: `base · (0.5^−(n−1) / (0.5^−n − 1))` for `n =
  base_octave`; target amplitude = sum of `|octave amplitudes|` via `DoubleStream`
  sum (Kahan, per Session 1); `estimateDeviation = sqrt(sum (0.2702247831245211·
  |amp|)²)`; `normFactor = (target·(1/3))/(deviation·√2)`. Single-octave σ of output
  = `base_amplitude/3`, so base `0.955388882960065` gives σ = 0.3185.
- `NoiseStack.get`: per layer `value += layer.amplitude · layer.noise.get(x·
  frequency, y·frequency, z·frequency)`; the second layer of a two-layer stack has
  frequency × input factor `1.0181268882175227`; `NormalNoise.create` forks two
  positional randoms and seeds octave i by `fromHashOf("octave_i")`.
- `Noises.instantiate`: each noise instance is created from the registry holder and
  seeded `positional.fromHashOf(name.identifier()).…` — i.e. by the hash of the
  registry name (e.g. `minecraft:ore_vein_a`) against the world's single positional
  factory. RustMC's port matches this exactly.
- `NoiseFunction` in 26.3: the sampler computes `noise.get(blockX·xzScale,
  blockY·yScale, blockZ·xzScale)` — multiplicative scales at absolute block
  coordinates (vein toggles use scale `4.0` with `base_octave −7`; veininess scale
  `1.5`; `ore_gap` scale `1.0` with `base_octave −5`; all vein noises
  `base_amplitude 0.955388882960065`).
- `InterpolatedFunction`: the point path trilinearly lerps (`Mth.lerp3`) over 8
  absolute `floorMod`-aligned grid corners with `cell_size_xz`/`cell_size_y`; the
  volume path aligns the corner grid to absolute cell multiples. `CacheFunction` is
  pure memoization (`SamplerContext.sampleVolumeCached`/`sampleValueCached`) with no
  value change. Both confirmed value-identical to RustMC's `density.rs`
  implementations (`Node::Interpolated` uses the same absolute-corner scheme).
- `getDensitiesInChunk(function, prefill)`: `prefill=true` fills a buffer via the
  volume path over absolute block coordinates, `prefill=false` uses point sampling;
  for value comparison these are equivalent to RustMC's per-column descent.
- `OreVeinRule` draw order (per block): `density ≤ 0` → keep; `randomFactory =
  getOrCreateRandomFactory("minecraft:ore")` (positional
  `fromHashOf("minecraft:ore").forkPositional()`); `nextFloat > density` → keep;
  `richness = richnessGetter`; `nextFloat < richness && fillerGapGetter < 0` →
  (`nextFloat < 0.02` ? raw : ore) else filler. `VeinType` constants: copper
  (`copper_ore`, `raw_copper_block`, granite, y `0..50`), iron
  (`deepslate_iron_ore`, `raw_iron_block`, tuff, y `−60..−8`); thresholds richness
  `0.4` clamp band `0.4..0.6`, edge roundoff begin `20`, max roundoff `0.2`,
  solidness `0.7`. RustMC's `surface.rs` ore-vein rule matches this order.
- Material-rule root binding: noise settings JSONs name the root material rule —
  overworld/amplified/large_biomes → `minecraft:overworld`; caves →
  `minecraft:overworld_caves` (a separate root preset, not a second underground
  pass); floating_islands → `minecraft:overworld_floating_islands`. The field
  `ore_veins_enabled` does NOT exist anywhere in 26.3 data or classes — veins are
  purely material-rule + density-function driven. Root `overworld.json` sequence
  order: [`bedrock_floor`, `overworld/copper_ore_vein`, `overworld/iron_ore_vein`,
  condition `above_preliminary_surface` → `surface`, `underground`] — veins precede
  the surface condition, first-match-wins, as implemented.
- Vein density graphs (operator datapack compared to jar-bundled data: diffed
  identical), with `t` the toggle: toggle = `cache(interp 4×8 of
  squeeze(veininess noise, scale 1.5)` clamped to the y window, yielding 0 outside);
  mask = the y-window gate and `range_choice(toggle, [−0.4, 0.4) → −1 else 0.08 −
  max(|interp(abs(vein_a))|, |interp(abs(vein_b))|))`; iron gate = `−t − 0.4 +
  (clamp(min(−8−y, y+60), 0, 20)·0.01) − 0.2 ≥ 0 → 0.7 else −1`; copper gate =
  `t − 0.4 + (clamp(min(50−y, y), 0, 20)·0.01) − 0.2 ≥ 0`; richness = `clamp(|t|,
  0.4..0.6)·1.0 − 0.3` (∈ `[0.1, 0.3]`); gap = `−0.3 − noise(ore_gap)`.

Vanilla-class probe marginals (`VeinProbe` harness, seed 2026, vein windows of the
ground-truth chunks):

- Raw veininess: n=44376 mean −0.0027 sd 0.3059; raw vein_a: n=44376 mean 0.0027 sd
  0.3120, `P(|a| ≤ 0.08)` = 0.1968; interpolated window samples n=736372 mean 0.0220
  sd 0.3143.
- IRON: gate fires 5.368% of window rows; mask ≥ 0 at 0.825%; final density > 0 at
  0.188%. COPPER: gate 4.095%; density > 0 at 0.130%.
- These match RustMC's independently written port (mask ≥ 0 ≈ 0.81%; iron ≈ 0.11%,
  copper ≈ 0.24% at the coarser comparison grid) within grid resolution — i.e. no
  vein-rate discrepancy exists between vanilla's own classes and RustMC.

Ground-truth census and the attribution correction (the key finding):

- A fresh official-server world (seed 2026, `generate-structures=false`,
  view-distance 4) was forceloaded over a 10×10 chunk square (blocks −64..95) and
  censused read-only by the Rust oracle: tuff 114,729 (bands −64: 50,882, −32:
  62,849, 0: 998; 1,147/chunk ≈ 8.6% of iron-window rows), granite 105,811, diorite
  107,543, andesite 108,791, deepslate_iron_ore 3,071, iron_ore 5,882,
  deepslate_copper_ore 734, copper_ore 15,025, raw_iron_block 20, raw_copper_block
  1, stone 1,661,812, deepslate 1,381,153.
- Explanation: tuff/granite/diorite/andesite census counts in the vein windows are
  dominated by placement-stage features — notably the `ore_tuff` feature (type
  `minecraft:ore`, size 64, target the `base_stone_overworld` tag → tuff) and the
  stone-blob features — NOT by the vein material rules. Raw-metal block counts (20
  `raw_iron_block` and 1 `raw_copper_block` across 100 chunks) are consistent with
  the veins' own low rates (≈0.02 raw chance on ~17.6 vein draws/chunk). The earlier
  slices' apparent "25–100× vein-rate gap" was therefore a census attribution error,
  not a generator mismatch; closing the remaining census delta requires the feature
  runtime (the next slice), and no vein parameter was re-tuned to chase the
  contaminated numbers.
- Also recorded: an earlier analytical slip in this investigation (`P(|x| ≤ 0.08) ≈
  0.599` for a Gaussian with σ 0.3185) was corrected to `2Φ(0.2512) − 1 = 0.198`,
  which is what the vanilla probe measures (0.1968) and what RustMC's port already
  produces.
- No new dependencies were introduced; the `/tmp` harnesses (the Java probe plus the
  headless server instance) are disposable consultation tooling under the ADR-0014
  amendment and the project's implementation and tests remain Rust-only.

### Session 11 (2 October 2026): chunk-adapter registry tables, protocol-777 wire shapes, and the four-slice integration record

Method. This entry closes the provenance record that the four integrated slices owed
(bounded generator caches, the protocol-777 chunk adapter, the M3 block-interaction
design, and the full-column block-identity oracle). No terrain generation code was
consulted for it: the generator-side facts are those of Sessions 1–10, re-used
unchanged. The adapter needed one thing no earlier slice had — numeric registry
identities — and it takes them **only at runtime from an operator-provisioned versioned
table** (`chunk_adapter::registry::RegistryTables`, validated for 26.3/777); no id list,
data file, or registry size from any Mojang source is in this repository, and the unit
fixtures use locally invented ids inside invented registry sizes. What *was* consulted
for the adapter is shape only: the field order and palette rules of the section
containers, the heightmap and light encodings, and the block-state classification each
one depends on, read from the 26.3 classes under the ADR-0014 knowledge-only amendment.
Nothing was copied or translated; every consulted number is restated below and
re-derived in Rust from the provisioned table rather than hardcoded.

A deletion made traceable: a predecessor of the adapter slice left the real 26.3
registry sizes — **35,723 block states and 67 biomes** — in a tracked test comment.
Those counts are Mojang-derived numeric facts, so they were removed from the code before
the commit and are logged here instead, with the two derived figures that depend on them:
`ceillog2(35723) = 16` bits for a direct-mode block palette and `ceillog2(67) = 7` bits
for a direct-mode biome palette. The committed fixtures reach the same two widths from
invented sizes (65,536 states and 128 biomes), which is why the tests stay meaningful
without carrying the counts. Any future change to a provisioned table recomputes both
widths from the table's declared sizes; no width is a constant in the code.

Wire facts the adapter depends on, recorded here because the log did not carry them
before (protocol 777, Java 26.3; the chunk packet id `46` is the one the accepted
synthetic preview already uses and a real client has rendered with):

- Heightmaps are sent as a VarInt entry count, then per entry a VarInt type id followed
  by a VarInt-prefixed big-endian long array. The type ids in use are WORLD_SURFACE `1`,
  MOTION_BLOCKING `4`, MOTION_BLOCKING_NO_LEAVES `5`. Entries are
  `ceillog2(dimension height)` bits wide, so the Overworld's 384-row columns pack at 9
  bits, 256 columns per chunk; stored values are relative to the dimension `min_y` (the
  T0 save-format fact) as `absolute_y + 1 − min_y`, so the Overworld's lowest buildable
  row (absolute `−64`) reports `1` and a column with nothing captured reports `0`.
- Section field order is `block_count` (short), `fluid_count` (short), block container,
  biome container. Both counts are `u16` big-endian and are keyed on `isAir()` and on a
  non-empty fluid state, which is why carved rows (whose saved air is `cave_air`) count
  as air and only the dimension fluid feeds the fluid count.
- Paletted containers: block palettes are indirect from 4 through 8 bits per entry,
  biome palettes from 1 through 3 bits; beyond those limits the container switches to
  global/direct mode whose width is `ceillog2(declared registry size)` and no palette
  entries are written. A single-distinct-value section writes width byte `0` plus one
  bare id and no long array.
- Light: a full column sends `sections + 2 = 26` skylight layers (the two boundary
  sections included), then four `BIT_SET` masks in the order skyYMask / blockYMask /
  emptySkyYMask / emptyBlockYMask, then the sky-light layer list and an empty block-light
  list. For a complete column skyYMask and emptyBlockYMask are all-set over the 26 layers
  and the other two are clear. A `BIT_SET` here is a VarInt byte count over little-endian
  bytes truncated at the highest set bit, not a long array. Skylight itself is a purely
  vertical model — level `15` above the top, attenuated per state downward with air `0`,
  fluid and leaves `1`, solid closing the column to `0` — nibble-packed two cells per
  byte, low nibble first. This is the same vertical model the accepted preview path uses;
  it is **not** a lit world: no block light, no sky-edge propagation from neighbours, and
  no border transfer is modelled (stated in the module docs).
- Block-state classification into air / fluid / leaves / solid is derived from the
  documented behaviour of the block families the generator can emit; the provisioned
  table's optional `state_kinds` section overrides any single state. An id the table does
  not classify is a typed error, never a fallback, because substituting an id moves a
  heightmap entry.

The M3 block-interaction design and its test-only probes cite one external figure: the
0.6 x 1.8-block player envelope, from the public wiki page for the player (checked
2 October 2026). It is recorded as RustMC's *proposed* whole-cube approximation, not as
a verified 26.3 mechanic; per-block collision, reach, and correction values remain
OBSERVE/BIND tasks in that document, and no gameplay number from Mojang data was
consulted or committed.

Measurements. Every number below is RustMC's own run against RustMC code and the owner's
own save — none of it is consulted or vendor-supplied data, and none of it is a parity
claim:

- Block identity vs the seed-2026 save (the oracle's `block_compare`, 2,401-column
  grid, 338,217 scored positions): 80.86% exact base block, 82.78% with the air family
  collapsed, against the 98.64% substance figure from Session 9. The `3,228` uncarved
  deep rows reproduce the recorded 3,226 carver residual. Four vein/blob stone pairs
  (tuff→deepslate, diorite/andesite/granite→stone) cover 35,223 positions, 10.4% of the
  sample — the placement-stage decoration runtime already isolated as the T4 target, not a
  consulted difference.
- Generator cache bounds: the overworld carve mask is `12,032` bytes
  (`ceil(256 x 376 / 64) x 8`), and the seven coordinate-keyed memos are capped at a
  fixed 1,407 KiB per generator (753 masks, 96 chunk carvers, 160 column tops,
  80 + 80 aquifer centers/statuses, 224 aquifer surfaces, 14 skip bounds), replacing
  48.5 MiB of masks plus 12.4 MiB of tops that a radius-32 view would otherwise pin.
- Full-descent cost, release build, seed 2026, re-measured by the lead on the integrated
  tree (2x2 chunk cold sweep): 13,954 ms per chunk, 54.5 ms per column, projected
  16.38 hours single-threaded for one 4,225-chunk radius-32 view; peak RSS 9,924 KiB.
  The slice's own 4x4 measurement gave 12,682 ms per chunk and a 14.88-hour projection.
  Either figure is the live-preview blocker; the cache bound is not what makes a view
  fast.
- Carve-path cost of the bound: 1.39 -> 2.27 ms per chunk on the adversarial 64x64
  full-volume scan, with peak RSS 54,528 -> 9,792 KiB (5.6x) and identical carved-block
  counts, which is the eviction-is-recomputation evidence.
- Adapter payload worst case: a fully direct-mode Overworld column encodes to 251,457
  bytes against the preview path's own 786,432-byte (768 KiB) batch budget, so 16 such
  chunks overflow it and the budget binds at batch level.

No new Cargo dependency, no save file, region file, or game data entered the repository
in any of the four slices; the operator-provisioned data roots stay untracked and are
reached only through `RUSTMC_VANILLA_DATA` / `RUSTMC_VANILLA_SAVE` in `#[ignore]`d
smokes.

### Session 12 (4–5 October 2026): placement-stage feature runtime (T4 slice K)

The ore/blob stone slice consulted the owner's locally fetched, deobfuscated
official Java 26.3 server classes, read in `/tmp` under the ADR-0014 amendment:
`OreFeature`/`AbstractOreFeature`, the `FeaturePlacer` placement chain and the
placement-modifier type registry, the `IntProvider`/`HeightProvider` family,
`VerticalAnchor` and `WorldGenerationContext`, `WorldgenRandom`,
`LegacyRandomSource`, `BitRandomSource` and `XoroshiroRandomSource`, `Mth`,
`WorldGenRegion` and `BulkSectionAccess`, `GenerationStep.Decoration`,
`FeatureSorter`, `ChunkGenerator.applyBiomeDecoration`, the `Heightmap` types
and the rule-test/target classes. Nothing consulted entered the repository: no
dump, decompiled text, translation or derived file is tracked, and the design,
implementation and tests are RustMC's own. The operator datapack documents and
the owner's saves were read only through `RUSTMC_VANILLA_DATA` /
`RUSTMC_VANILLA_SAVE` paths and were never copied.

Behaviour facts established (26.3):

- `OreFeature.place` draws exactly three times before any block work: `ang =
  nextFloat() * π`, then two independent `nextInt(3) − 2` for the two endpoint
  Y values. The axis endpoints use `java.lang.Math.sin`/`Math.cos` on the
  widened float — a 26.3 divergence from the table-based `Mth.sin` the in-vein
  wobble uses. With `f = size/8` the box is `2·(ceil(f) + extent)` wide and
  `2·(extent + 2)` tall, `extent = ceil((size/16·2 + 1)/2)`.
- The anchor gate scans the box footprint x-outer/z-inner, inclusive of
  `base + size`, accepts the first column with `baseY ≤ getHeight(
  OCEAN_FLOOR_WG, x, z)` (that getter returns the column height plus one) and
  calls `doPlace` at most once. With no passing column it returns `false`
  having drawn nothing further. The type is the world-gen heightmap, not
  motion blocking.
- `doPlace` takes one `nextDouble` per `size` axis point for that point's
  ellipsoid radius, prunes a point whose radius difference exceeds its squared
  distance by writing `−1.0`, then walks the inclusive box with a `BitSet`
  sized `sizeX·sizeY·sizeX` so each candidate index is painted at most once,
  and the strict `dx² + dy² + dz² < 1` test decides inclusion. For each
  accepted position the target list is evaluated in order and the first passing
  replacement wins; the air-exposure discard roll happens only when `0 <
  discard < 1`, and the adjacency test reads six neighbours with a missing
  section counting as air.
- `WorldgenRandom`, the decoration stream, forwards every `next(bits)` to its
  xoroshiro delegate, so one request costs exactly one delegate long while
  `nextLong` and `nextDouble` cost two — a different contract from the derived
  draws on `RandomSource` itself, and the decoration order depends on it.
  `setDecorationSeed(worldSeed, x, z)` reseeds, takes two `nextLong | 1`
  scales, mixes `(x·a + z·b) ^ worldSeed` and reseeds, ignoring Y;
  `setFeatureSeed` is `seed + ordinal + step·10000` with no draws at all.
  Observed output: running the 26.3 server classes on 5 October 2026 produced
  the decoration seeds and draw sequences now pinned as golden vectors by
  `vanilla::random::tests::decoration_seed_matches_parity_vectors`,
  `decoration_draw_sequence_matches_parity_vectors` and
  `decoration_composite_draws_match_parity_vectors` — the numbers are black-box
  observations, no reference source was transcribed.
- Placement modifiers run in list order with these per-attempt costs: `count`
  is one provider sample (constant 0 draws, uniform 1, trapezoid 1–2) that
  reuses the *same* position for each copy; `rarity_filter` is one `nextFloat`
  kept iff `< 1/chance`; `in_square` is two `nextInt(16)`; `height_range` is
  one draw for a uniform provider (even when min equals max) and two for a
  trapezoid unless the span is 0, while an inverted bound resolves to the
  minimum without drawing; `biome` draws nothing and is a flat membership
  test. 26.3's `TrapezoidInt` has no `base`/`fold` terms.
- `FeatureSorter.buildFeaturesPerStep` numbers features with one global
  sequential counter, uses that only to topologically order the graph built
  from each biome's per-step lists, and hands the index inside the step's
  sorted list to `setFeatureSeed` as the ordinal.
  `GenerationStep.Decoration` has 11 constants with `UNDERGROUND_ORES` at 6.
- Chunk-border behaviour: `WorldGenRegion.isWithinWriteZone` compares only the
  section x/z of a position against `ChunkStep.blockStateWriteRadius()` (Y is
  not considered), an out-of-zone write is silently skipped (log plus IDE
  pause, no exception) while reads stay unrestricted. A border vein therefore
  loses its overhang in one chunk's pass and the neighbour's own pass paints
  the rest from its own per-feature seed. `ChunkSteps` was absent from the
  dumps, so the concrete integer radius for the terrain and feature steps is
  unverified.
- `applyBiomeDecoration` builds one nondeterministically seeded stream per
  chunk and immediately reseeds it, collects the biome holders appearing in
  the 3×3 chunk range intersected with the generator's possible biomes, and
  iterates the decoration steps in order.

Shape facts measured from the owner's seed-2026 save (read-only generated
output; the counting probe was throwaway tooling in a disposable worktree and
is not committed):

- `size` is a scale, not a voxel budget: across the twelve single-`size` ore
  features the mean component follows `blocks ≈ 0.045 · size^2.09` (R² = 0.949,
  n = 12), so a `size = 64` stone placement is worth roughly ten times its
  nominal size in blocks (granite 265 blocks at 6-connectivity, 585 at
  26-connectivity).
- Stone blobs are oblate and sparse: at `size = 64` the median `dx`/`dz` is
  13–15 and `dy` is 8, horizontal reach median 8.5–9.0 (`size/7`) with p90
  13–14, fill fraction 0.32–0.35. Ore clumps are small and near-solid (2.3–18.3
  blocks, fill 0.54–1.00, reach ≤ 2.1).
- Veins straddle chunk borders constantly: 26-connected stone components cross
  a border in either axis 0.761–0.858 of the time and occupy 2.86–3.07 chunks;
  the widest merged components span `dx = 95` (granite) and `dx = 181` (tuff)
  blocks. Those wide components are several adjacent anchors merging, so
  per-anchor reach stays inside one neighbouring chunk ring: a feature placed
  at one anchor can only affect the 3×3 ring around it, and a target chunk sees
  the union of its own and its eight neighbours' anchors.
- 6-connectivity fragments real veins (granite median component 4 blocks at
  6-connectivity against 510 at 26-connectivity), and the 26-connected
  component count per chunk is 0.72–0.84 of the datapack's attempts per chunk —
  the paint step advances diagonally and adjacent placements merge, which a
  face-only walk would not reproduce.
- Air contact is low and filtered: 3.8–4.4% of stone blob blocks touch air, and
  the families carrying a discard chance sit at the bottom of that range
  (diamond 0.74%, lapis 1.20%, coal 1.73–1.98%, gold 2.17%) against copper
  3.94–5.27% and iron 4.39–5.21% with discard 0.
- The Y bands match the datapack windows, with the leak between bands
  attributable to vertical reach rather than to the height providers (tuff
  10,343 of 1,231,339 blocks, 0.84%, in the band above its window).

Documented deviations in RustMC's implementation of this stage:

- **Write zone.** RustMC clips a decorated chunk's writes to its own 16×16
  column area and replays the 3×3 ring of anchor chunks, which the measured
  per-anchor reach bounds exactly. Vanilla's own `blockStateWriteRadius`
  integer could not be verified from the dumps, so the ring rests on the
  geometry and the save measurement rather than on a consulted constant.
  Overlapping writes from two anchors resolve in RustMC's deterministic
  row-major replay order whereas vanilla lets each chunk's own pass write last;
  the difference is confined to positions two veins both claim, and tests pin
  that the assembled grid is independent of the order chunks are built in and
  of cache eviction. Session 13 measured the alternative — letting one chunk's
  replay write past its border — and it answers no better, so this deviation
  stands.
- **Ordinals.** *(closed by session 13)* RustMC numbered one global per-step
  schedule sorted by feature identifier instead of reproducing the reference
  runtime's dependency-graph ordering, so a feature's reseed ordinal could
  differ from vanilla's whenever that graph reorders a step. Features RustMC
  does not model still consume their slot, so their neighbours kept stable
  ordinals. The measured consequence, recorded in
  `docs/research/vanilla-worldgen-feasibility.md` under slice K: because the
  ordinal is part of the feature's seed, an ordinal that differs moves every
  anchor that feature draws, so family counts land close to the save while the
  sampled per-coordinate block identity falls from 81.28% to 70.83% on the
  seed-2026 `-384..384` grid. `StepSchedule` now builds the graph ordering and
  the measured fall has reversed; see session 13.
- **Anchor gate.** *(closed by session 13)* The `OCEAN_FLOOR_WG` footprint gate
  was not reproduced. It is a fast path rather than a placement rule — a box
  with no terrain in it can only hold air and fluid, which every stone-family
  target test rejects — but skipping it lets a floating vein draw its `size`
  radii here, which shifts the stream for the attempts that follow it in the
  same chain. `vanilla::feature::anchored` now reproduces it; session 13
  measures what the gate is worth and records the heightmap source that is
  still an approximation.
- **Heightmap.** *(partly closed by session 13)* With the gate absent, the
  world-gen heightmap was not captured for decoration. Reproducing the gate
  needs a height to compare against, and a finished save cannot supply one:
  `keepAfterWorldgen()` drops the `*_WG` types, so RustMC descends its own
  computed surface to the first solid block. A vegetation slice will need the
  same state for the surface types. Both the stored-layout fact and the
  remaining approximation are recorded in session 13 and in
  `docs/research/vanilla-worldgen-feasibility.md`.

No parity claim rests on this slice. The measured census and base-block deltas
are in `docs/research/vanilla-worldgen-feasibility.md`, the vegetation families
and structures remain absent, and the ore/blob placement is verified against
aggregate save counts plus self-consistency invariants — not against a
block-for-block vanilla reproduction. No new Cargo dependency was introduced,
and no game data or consulted source entered the repository.

### Session 13 (5–6 October 2026): decoration-time heightmaps, the step ordinal graph and the ore draw budget (T4 slice L)

The slice consulted the owner's locally fetched, deobfuscated official Java 26.3
server classes, read outside the repository under the ADR-0014 amendment:
`Heightmap`, `Heightmap$Types` and `Heightmap$Usage`, `ChunkAccess` and the
`ProtoChunk` block-write path, `AbstractOreFeature` and `OreFeature`
re-consulted for the per-cell discard cost, `FeatureSorter`,
`BiomeGenerationSettings`' per-step feature lists, the `blocks_motion*`
block-tag chain, and the placement-chain classes already listed under session
12. The operator datapack's `worldgen/feature`, `worldgen/placed_feature` and
`tags/block` documents and the owner's seed-2026 and seed-2027 saves were read
only through the `RUSTMC_VANILLA_DATA` / `RUSTMC_VANILLA_SAVE` paths and were
never copied. Nothing consulted entered the repository: no dump, decompiled
text, translation or derived file is tracked, and the design, implementation
and tests are RustMC's own. The measurement tooling this session used — a
standalone Anvil/NBT column and heightmap decoder, a vein cluster matcher, and
a single-chunk decoration replay harness — is throwaway and lives outside the
worktree.

Behaviour facts established (26.3):

- **Heightmaps are the non-spanning bit layout.** `Heightmap`'s constructor
  builds a `SimpleBitStorage(ceillog2(chunk height + 1), 256)` per type: the
  overworld's 385 levels need 9 bits, that storage puts `64 / 9 = 7` values in a
  long and wastes the remaining 6 bits, and 256 columns therefore occupy 37
  longs where a spanning read would need 36.
- **A world-gen heightmap is a running maximum, not a snapshot of finished
  terrain.** `ProtoChunk.setBlockState` walks the types in
  `ChunkStatus.heightmapsAfter()` and calls `Heightmap.update(x, y, z, state)`
  for each, and priming scans a column downward from the top, so the stored
  value is the highest Y so far passing that type's predicate.
  `keepAfterWorldgen()` drops the `*_WG` types, so the `OCEAN_FLOOR_WG` the ore
  anchor gate reads is not present in a finished save: the nearest stored key,
  `OCEAN_FLOOR`, describes terrain after decoration.
- **`OCEAN_FLOOR_WG`'s predicate** is the `blocks_motion_in_heightmap` tag test
  with no fluid clause, so water does not raise it and a carved ceiling does
  lower it.
- **Per-cell discard cost.** `AbstractOreFeature.canPlaceOre` evaluates the
  target rules, then `shouldSkipAirCheck(random, discard)`, then the six
  neighbour air test, in that order, and `shouldSkipAirCheck` draws one
  `nextFloat` only when `0 < discard < 1`. A feature carrying a fractional
  discard chance therefore consumes one draw per cell its target rules accepted,
  so how far it moves the decoration stream depends on terrain and not only on
  geometry, and every later attempt of that feature resumes from wherever the
  accepted cells left it.
- **Where the fractional chances sit** in 26.3's `worldgen/feature` documents:
  `ore_coal_buried`, `ore_gold_buried`, `ore_diamond_small` and
  `ore_diamond_medium` at 0.5, `ore_diamond_large` at 0.7, `ore_diamond_buried`,
  `ore_lapis_buried` and both ancient-debris features at 1.0, against 0.0 for
  every stone family (`ore_dirt`, `ore_gravel`, `ore_granite`, `ore_diorite`,
  `ore_andesite`, `ore_tuff`) and for the plain coal, iron, gold, redstone,
  lapis, copper and emerald features. The placed features choose between them:
  `ore_coal_lower` runs `ore_coal_buried` while `ore_coal_upper` runs
  `ore_coal`, and `ore_gold_lower` runs `ore_gold_buried`.

Save-format facts measured independently of RustMC, from the owner's seed-2026
world (2,048 chunks over two region files, 524,288 columns, decoded by the
standalone reader):

- Every stored heightmap is 37 longs — all four kept keys, in every chunk
  surveyed — and the decoded values span Y 32..195.
- Decoded non-spanning, the values agree with the column tops read straight from
  the section palettes: `WORLD_SURFACE` exact in 524,288 of 524,288 columns,
  `MOTION_BLOCKING` 75.32% exact and 99.17% within one block,
  `MOTION_BLOCKING_NO_LEAVES` 57.10% and 81.50% (that type ignores leaves, so it
  legitimately reads lower), `OCEAN_FLOOR` 64.16% and 89.27% (fluids excluded).
- Decoded spanning, the same arrays answer 3.86–5.69% exact and produce values
  from −65 to 446, outside the world's −64..319 span — the signature of a
  wrong-layout read rather than of a noisy measurement.
- Section palettes use the packed layout too, including the power-of-two sizes:
  a 32-entry palette at 5 bits is stored in 342 longs where spanning would need
  320, and a 56-entry palette at 6 bits in 410 where spanning would need 384.
  RustMC's palette decode already followed the packed rule and was left alone.

RustMC's own defect, found by that measurement and fixed: `oracle::surface_top`
decoded stored heightmaps as spanning, which is right only for the first seven
columns of each chunk. The scope of the damage is measurable: the M3 exact-grid
samplers stride 16 blocks, so they read local column index 0, and their
denominators (347,483, 338,217 and 308,379 positions) are identical before and
after the fix — every earlier A/B comparison in this research stood on correct
heights by that accident of stride. Any other stride, and every per-column
inspection, was reading garbage. The decode now selects the layout from the
stored long count, which is unambiguous because the two counts never coincide,
and `oracle::tests::heightmaps_decode_from_their_stored_long_count` packs
1..=256 both ways and checks columns 0, 6, 7, 135 and 255.

RustMC changes this slice makes, each one demonstrated rather than assumed:

- **Step ordinal graph.** `vanilla::feature` builds a `StepSchedule` per
  decoration step by the documented topological descent over each biome's
  per-step list, with a bounded retry when a biome pair yields a cyclic order,
  and `region_ordinals` hands a feature the index inside the step's sorted list
  — the value `setFeatureSeed` mixes into the seed. The biome set feeding the
  graph is `generator::region_biomes`, the biomes appearing in the 3×3 chunk
  range intersected with `biome::possible_biomes()`, replacing the earlier
  identifier-sorted stand-in.
  That bounded retry (`MAX_CYCLE_ATTEMPTS = 8`, against the reference's
  unbounded recursion, which is exponential in the number of biomes) is a
  deviation, and the pack this slice measures is shown not to reach it:
  `vanilla::generator::tests::smoke_operator_feature_schedule_needs_no_cycle_recovery`
  builds the real schedule, asserts neither `feature_order_cycle` nor
  `feature_order_cycle_unresolved` is reported, and prints the shape it found —
  9 generation steps carrying 171 placed features over the 56 biomes the
  dimension's biome source declares. A pack that did need more than eight prunes
  would get an empty schedule and that counter instead, so the bound cannot fail
  silently.
- **Anchor gate.** `vanilla::feature::anchored` reproduces the
  `OCEAN_FLOOR_WG` footprint scan over the vein box's columns, so an attempt
  whose box floats entirely above the terrain costs only its three axis draws,
  and the stream after it lands where vanilla's does. The heightmap it consults
  is `VanillaGenerator::ocean_floor_height`, the computed surface descended to the
  first solid block — served since session 13's follow-up slice from a heightmap
  stored per chunk, whose rows the column fill derives from the substances it had
  already sampled and a halo lookup adds one column at a time. That is RustMC's
  own representation, not a claim about how the reference builds or mutates its
  `*_WG` heightmaps; the documented observable, the row answered for a column, is
  what is held fixed and tested.

Measured, on the M3 `block_compare` grids (seed 2026 over `-384..384`, seed 2026
over the milestone's `-256..512`, seed 2027 over `-384..384`; each round re-run
whole, successive intermediate builds, exact base-block agreement):

| build | `-384..384` | `-256..512` | seed 2027 |
| --- | --- | --- | --- |
| pre-feature terrain only | 81.28% | 80.86% | 83.91% |
| placement with identifier-sorted ordinals | 70.83% | 70.08% | 74.54% |
| step ordinal graph, region biome set included | 91.19% | 91.27% | 92.65% |
| and the anchor gate (this slice's tree) | 94.84% | 95.02% | 96.29% |

Two notes on what those rows are, because the tree was edited in place between
rounds and a label is then not evidence. First, the rounds are identified by
their own output, not by their names: the `-128..127` family census over seed
2026 paints `coal_ore` 38,512 blocks on the pre-gate graph build and 37,951 on
the gated one, which is what separates the third row from the fourth.
Second, that fourth row was mis-labelled when this section was first written —
the grids it calls "the decoration region's biome set" were run on a tree that
already carried the anchor gate too. The ablation settles which change earned
the movement. This tree with `anchored` patched to report every box as anchored
answers the pre-gate census byte-for-byte, every family line included, and on the
grids it answers 316,872, 308,680 and 285,714 exact positions — the pre-gate
round's three numbers again, to the position. So the region biome set carries no
measurable gain of its own and the whole movement is the anchor gate's: 12,687,
12,685 and 11,239 positions, 3.65, 3.75 and 3.65 points. Every one of those is a
solid-row identity, since the air-collapsed counts move by exactly the same
numbers. The row label is corrected rather than the numbers, because the numbers
were never in question.

A third round then re-ran all three grids after the stored-heightmap decode fix
described above, which changes the oracle's read of the save rather than the
generator. Every report came back byte-identical to the gated round, exactly as
the stride argument predicts: those grids sample local column index 0, the one
column the two decoders agree on. So the ladder is not a comparison across a
measurement change, and the fix's value is to the per-column inspections, which
had no valid earlier reading.

The terrain answer is untouched by all of it: `substance` over the same window
reads 342,641 of 347,483 positions exact (98.61%) and its whole report is
byte-identical between the session-12 baseline build and every round of this
slice, so the movement above is placement, not terrain. The census moves with
it: over the `-128..127` square the aggregate family agreement (the lesser of
the two sides over the save's sum, the 24 families slice K tabulated) reaches
99.99%, with `granite` from 114.1% to 100.6% of the save's count, `coal_ore`
from 104.7% to 99.2%, `copper_ore` from 102.1% to 99.7%, `lapis_ore` from 105.2%
to exactly the save's 2,559 blocks, and the four vegetation families still at
zero because this slice does not place them.

Cost, measured in one machine state — release build, single thread, seed 2026,
the bench's default 4×4 `column_ids` sweep, every binary run within the same
hour with three oracle grids busy on other cores — and attributed with an
ablation, this same tree patched so `anchored` reports every box as anchored
(which is the placement work the no-gate tree does, minus the footprint scan):

| | `f86ef84` | this tree, gate ablated | this tree |
| --- | --- | --- | --- |
| cold pass | 570.47 ms/chunk | 784.84 ms/chunk | 1,750.75 ms/chunk |
| warm cache-read pass | 1.34 ms/chunk | 1.55 ms/chunk | 1.54 ms/chunk |
| radius-32 view, single thread | 0.67 h | 0.92 h | 1.99 h |
| peak resident set | 17,780 KiB | 17,556 KiB | 17,524 KiB |

The ablation is checked rather than trusted: its `-128..127` family census
reproduces the pre-gate round's report byte-for-byte, every family line included
(`coal_ore` 38,512 against the gated round's 37,951), so what is measured is the
footprint scan and not some other difference between the builds.

The terrain modes do not move (`heights` 203.17 → 204.65, `carve` 2.42 → 2.54,
`substance` 433.98 → 435.23 ms per chunk, with the substance answer
byte-identical between the trees), so the increase is entirely on the decorated
path, and the ablation splits it: +214 ms per chunk for the ordinal graph and
the decoration region's biome union together, then +966 ms per chunk for the
anchor gate on top — 55% of the tree's cost. The cause is the missing piece
session 12's deviation list already names: RustMC holds no decoration-time
heightmap, so the gate's per-column question is answered by computation, and the
sweep ends with 4,843 ocean-floor columns memoised over a cache bounded at
4,096, each descent walking the density graph row by row. Peak resident set is
flat across all three columns, so this is recomputation and not growth. Keeping
the heightmap per chunk through the fill and carve passes would turn the largest
cost of this slice into an array read, which makes that work a cost item as well
as a correctness one. Absolute milliseconds are conditioned on the state of the
machine — the same `f86ef84` binary answers 570.47 today against the 388.76
recorded for it under session 12 — which is why the table is read as ratios and
not as levels; repeated runs of one binary agreed to within 0.5%.

One consequence of that cost is a test change rather than a generator change. The
operator smoke `java_preview::tests::local_vanilla_worker_delivers_a_framed_chunk_without_blocking_network_poll`
carried a 20-second bound on the first delivered chunk, which held while a cold
column descent was pre-placement work. With the gate in, the same run from a
debug build delivers its first chunk in 91 s measured — a debug build is far
slower than the 1,750 ms per chunk the release bench reports, and each of the
smoke's eight workers compiles the provisioned pack before drawing anything — so
the bound is now 600 s. It is a hang detector, not a performance gate: what this
path costs is what `bench_vanilla_chunks` measures and the table above reports,
and the smoke's actual claim, that the network poll never blocks on generation,
is unchanged and still asserted at 100 ms. The whole nine-smoke operator set now
takes 475 s in a debug build, which is the same cost seen from the other side;
that figure is machine-state conditioned (three oracle grids were busy through
it) and is reported as why the bound had to move, not as a measurement.

Follow-up slice, same day: the decoration-time heightmap the gate asks for is now
stored per chunk and derived inside the column fill, from the substances that fill
had already sampled. This is a RustMC-side representation decision, and the only
vanilla-shaped fact it touches is the documented observable — the row the
`OCEAN_FLOOR_WG` footprint test reads for a pre-decoration column — which it holds
fixed. Evidence that it holds fixed: `ocean_floor_height` equals
`ocean_floor_by_descent` (the pre-change body, kept as the fallback) on every
column of the four new tests and of the new operator-data smoke, including after
whole-chunk fills overwrite lazily stored rows and after a sweep rotates every
original map out of the cache; the 3D `substance` report is byte-identical between
the builds (md5 28f153fd93ebb5d299544a3447f195c1); the `-128..127` family census
report is byte-identical, all 24 placed families and `coal_ore` 37,951 included,
which is the feature stream and the gate's decisions; and the three
`block_compare` grids re-measure to the same lines and the same numerators —
329,559 of 347,483, 321,365 of 338,217, 296,953 of 308,379. The baseline side of
those grid and census comparisons is the round this worktree produced before the
slice's edits, whose figures are the ones committed above; the candidate side ran
with `fail-cap` 5 against that round's 20, and the oracle truncates that list by
prefix (`if report.fail_positions.len() < fail_cap`), so the compared lines are
the same lines. What the slice does not claim: that the reference stores or mutates
a heightmap this way, and that the remaining ore displacement is anything other
than what the trace localises, since the gate's answers did not move.

Decoration replay evidence for one chunk (seed 2026, chunk `(-8, -8)`, the
step-6 chain, ordinals 0..33, our replay reading that chunk's stored terrain):

- With the anchor gate forced never to pass, the chain paints nothing: the
  footprint test is load-bearing, not decorative.
- With it forced to pass, the chain paints 4,365 ore positions of which 2,953
  (67.65%) sit on a block the save holds — floating boxes draw their radii and
  shift the chain behind them.
- With it as documented, 3,362 positions, 2,953 of them (87.83%) on a save ore
  block.
- Allowing writes past the decorated chunk's border gives 3,417 positions and
  the same 2,953 (86.42%), so the clipped 3×3 ring replay stays.
- Per feature the split is sharp. The discard-0 stone families reproduce
  positionally: `ore_tuff` 398 of 398, `ore_diorite_lower` 1,067 of 1,071,
  `ore_andesite_lower` 787 of 812, `ore_granite_lower` 701 of 896, and their
  misses are host-block disagreements rather than misplaced veins. `ore_coal_lower`
  — the first feature in the chain carrying a fractional discard — places 159
  positions and matches none, and so does every small ore after it
  (`ore_iron_upper` 6, `ore_gold` 14, `ore_diamond_medium` 6, all unmatched).
- Matching our coal veins to the save's over a 4-chunk window: the ten clusters
  of five blocks or more that RustMC paints in the traced chunk each face a save
  cluster between 3.8 and 8.6 blocks from their centroid, and none within 2;
  cluster sizes run 9–48 blocks against the save's 5–27. The rate, the box and
  the Y band are right, the anchors are shifted.

What that demonstrates, and what it does not:

- The seed, the ordinal and the vein geometry reproduce. The discard-0 families
  match to within a few percent of their painted positions under the same
  ordinals and the same streams that miss for coal, iron, gold and diamond, so
  the residual is no longer an ordinal or seed defect — session 12's ordinal
  deviation is closed by the graph, and the numbers above are its measurement.
- The remaining gap is terrain-dependent draw consumption. A fractional-discard
  feature spends a draw per accepted cell, so how many draws a chain has
  consumed before the next feature reseeds depends on which cells the terrain
  let through. Two differences in that terrain are documented here. The replay
  those numbers come from reads the save's *final* blocks, which carry the later
  features' writes and not the state the decoration pass had. And the gate
  compares against `ocean_floor_height` — RustMC's own descent from the computed
  surface through carved air — where vanilla compares against a heightmap its
  write path maintained as the chunk was built. The two agree more often than
  not, and the distance is measured: over the traced chunk's 256 columns the
  generator answers the save's topmost non-air row exactly in 231 of them, and
  in the remaining 25 it sits 2 to 8 rows *below* it — never above — which is
  the direction the semantics predict, since a saved column top can be a block
  the world-gen gate never saw. An earlier reading of that same comparison,
  which had the generator answering 87 to 171 rows too high, was the
  spanning-decode artifact described above rather than a generator defect.
- Either difference moves the anchors of every feature downstream, and after
  that the veins are simply somewhere else. What is *not* demonstrated is any
  per-vein attribution between the two: a chain shifted by an accepted-cell roll
  and a chain shifted by a gate decision are the same observable — a different
  number of draws before the next feature reseeds. The next piece of work for
  this stage is therefore a decoration-time heightmap maintained through the
  carver stage, not more ordinal work, and that is recorded in
  `docs/research/vanilla-worldgen-feasibility.md`.

No parity claim rests on this slice. The table above is a measured improvement
over the pre-feature baseline on three grids and two seeds, not parity: 17,924 of
the traced grid's 347,483 scored positions still disagree (5.16%), and 11,159 of
them (3.21%) once the two sides' air families collapse to one name. The shape of
what is left is recorded rather than smoothed over. 6,740 of the exact residual
is one air position named two ways — `cave_air` where the save holds `air`.
2,422 is a row the save leaves void that RustMC fills with deepslate, which is
the deep-carver residual the T3 baseline already attributed. The vein families
disagree as a near-symmetric pair (703 positions where the save holds coal ore
and RustMC holds stone, against 690 the other way), which is the anchor
displacement described above rather than a wrong amount. The vegetation families
and structures are absent, and the deviation list carried forward is the
write-zone radius (vanilla's `blockStateWriteRadius` integer is still unverified
from the dumps) and the decoration-time heightmap. No new Cargo
dependency was introduced, no game data or consulted source entered the
repository, and the operator preview's packet-cache identity moved to
`rustmc-preview-cache-v3` because the decorated generator it caches changed.

## Vanilla research review hardening (2 October 2026)

No new external generator source or game data was consulted for this change.
Review of the RustMC research implementation found that cyclic density
references and out-of-range gradient coordinates could crash a local oracle
run. Compilation now rejects reference cycles, deep reference chains, and
gradient ranges outside the supported 32-bit span; evaluation widens the
coordinate arithmetic before tiling. Biome interval documentation now matches
the inclusive implementation. Unsupported carver types retain their position
in a biome's carver list, preserving later seed indices, while malformed or
missing references fail with an error. Tests cover these cases with synthetic
data. The provisioned Overworld graph was checked locally; no private data is
committed. Volume wrappers, cache bounds, feature placement, and live chunk
integration remain open before any client-visible parity claim.

## Java 26.3 motion heightmap review (2 October 2026)

The owner-local Java 26.3 server jar was inspected with `javap` in `/tmp` to
check the predicates for `MOTION_BLOCKING` and `MOTION_BLOCKING_NO_LEAVES`.
Both capture a block when its matching heightmap tag applies **or** its fluid
state is nonempty. RustMC's independent chunk adapter had omitted fluid states
from both motion heightmaps; the classification and synthetic round-trip tests
now include them. No game code, class file, or asset was added to the repository.
The adapter still uses a coarse state classification for the block tags and
needs per-state validation against versioned data before claiming exact
heightmap parity.

## Interpolated density cache (2 October 2026)

This optimization consulted no new game source or third-party implementation.
Linux `perf` on RustMC's own release binary attributed the largest CPU share
to repeated Perlin sampling while evaluating interpolated density nodes. The
node now memoizes its eight corner values by aligned world cell in a bounded
cache. It retains the existing volume evaluation and interpolation arithmetic;
eviction recomputes the same immutable values. The published seed-2026 save
comparison counts remained identical after the change. Timing and limits are
recorded in the M3 Overworld milestone note.

## Local Java 26.3 chunk ID table preparation (2 October 2026)

`prepare_chunk_registry` reads the owner-generated official `blocks.json`
report and RustMC's existing local biome manifest. It derives protocol state
IDs from the report, aliases bare material-rule block names to each reported
default state, and writes a versioned JSON table outside Git. The table and
game report are local data, not redistributed RustMC source. A smoke test
encoded two generated columns with the derived IDs. No competing server code
or architecture was consulted for this tool.

## Second-seed save observation (2 October 2026)

The read-only `inspect_vanilla_save` tool parses the owner's local Java 26.3
`level.dat` and `data/minecraft/world_gen_settings.dat` as compressed NBT.
Their observed metadata supplies version, seed, Overworld generator/settings,
modded flag, enabled packs, and spawn coordinates without printing player
records. The source is an owner-generated save; it remains untracked. A second
default-preset seed was compared through the existing oracle, and its
denominators and residuals are recorded in the M3 Overworld milestone note.
No competing server source was used for this observation.
## Operator-local preview packet cache (2 October 2026)

`sha2 0.10.9` (RustCrypto, `MIT OR Apache-2.0`) hashes the preview cache's
operator-provisioned inputs and packet bytes for invalidation and corruption
detection. Its new locked transitive `cpufeatures 0.2.17` has the same license
declaration. This is a local integrity check, not authentication or custom
cryptography. The cache is an immutable rendering artifact and does not save
player edits. Both packages are recorded in the dependency license policy.
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
