# Development

Linux/Fedora is the initially tested environment. Install Git and rustup from trusted sources; `rust-toolchain.toml` pins Rust 1.98.1 with rustfmt and clippy. Other platforms are unverified. Direct libraries include `toml` for configuration parsing, `signal-hook` for Unix shutdown signals, `uuid` for a random, connection-scoped preview session ID, and `rustc-hash` for bounded generator coordinate caches; see [provenance](PROVENANCE.md) for locked licenses and review scope. Do not update dependencies or toolchain merely to hide a failing check.

From the repository root:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
cargo doc --workspace --no-deps --locked
cargo run -p rustmc-tools --bin check_dependency_licenses --locked
cargo run -p rustmc-server --locked -- --check-config config/rustmc.example.toml
cargo run -p rustmc-server --locked -- --run config/rustmc.example.toml
```

`--run` is a **loopback-only discovery listener**, not a playable world endpoint. It binds TCP for Java 26.3 status and UDP for Bedrock 1.26.51 discovery on the same port. The example uses port `0`; `listener_bound` reports the chosen local port, and `discovery_bound` confirms both transports are ready while login and world readiness remain false. Press Ctrl-C or send SIGTERM for graceful shutdown. No arguments print status and exit without listening. `--help` and `--version` also exit without listening. `--check-config PATH` reads and validates without modifying its input or binding a socket.

Schema 1 requires `schema_version = 1` and `log_level` (`error`, `warn`, `info`, `debug`, or `trace`). The optional `[listener]` table has these defaults and allowed ranges. The preview-specific fields are disabled unless `local_java_preview = true`:

| Field | Default | Allowed value |
| --- | --- | --- |
| `bind_address` | `127.0.0.1` | Loopback IP literal only; port belongs in `port` |
| `port` | `0` | `0..=65535`; zero requests an ephemeral port |
| `max_connections` | `8` | `1..=64` simultaneous accepted sockets |
| `max_bytes_per_connection` | `4096` | `1..=65536` bytes read before closure |
| `idle_timeout_ms` | `1000` | `10..=60000` ms without received data |
| `max_connection_lifetime_ms` | `10000` | `10..=60000` ms for discovery, or up to `3600000` ms for the opt-in local Java preview; must be at least idle timeout |
| `local_java_preview` | `false` | Boolean; enables the unauthenticated local Java 26.3 Creative terrain preview when a matching manifest is supplied. |
| `preview_registry_manifest` | absent | Path to locally prepared, version-checked 26.3 registry identifier/tag metadata. Required for world entry. |
| `preview_seed` | `0` | Integer `0..=9223372036854775807`, used only for original preview generation. |
| `preview_view_distance` | `4` | Integer `2..=32` chunks. Radius 32 permits up to 4,225 loaded chunk coordinates per client; only one batch is in flight, containing at most 16 chunks within a 768 KiB encoding budget. |
| `preview_terrain` | `"preview"` | `"preview"` or `"experimental"`. Experimental selects the opt-in octave-noise terrain field (T1 groundwork); biome labels, surface blocks, and trees stay on the ADR-0013 preview rules. Neither option is vanilla generation. |
| `vanilla_data_root` | absent | Operator-local Java 26.3 worldgen data root for a data-driven Overworld probe; requires the registry table, local preview, manifest, and `max_connections = 1`. |
| `vanilla_registry_table` | absent | Operator-local Java 26.3 block-state and biome ID table produced by `prepare_chunk_registry`; never commit the table or game data. |
| `vanilla_cache_root` | absent | Optional disk directory for immutable preview chunk packets. Improves repeat visits only; contains no authoritative world edits. Cache is capped at 8,192 packets or 1 GiB for the current seed/data identity. |
| `vanilla_generation_workers` | `8` | Integer `1..=20`; number of independent cold chunk generators for the local vanilla preview. More workers use more CPU and memory. |

When both `vanilla_*` paths are set, RustMC validates them before binding and
starts the configured worker pool. Each worker owns a generator; at most that many
chunks are under construction for the one permitted local client. The initial
recommended first test radius is 2; the operator can set up to 32 for a
long-running load test. For a repeat visit, set `vanilla_cache_root` and run
`cargo run --release -p rustmc-tools --bin prepare_preview_cache --locked -- DATA_ROOT REGISTRY_TABLE CACHE_ROOT SEED 12`
before joining. This prepares a radius-12 disk view around chunk (0,0); cold
generation still takes time. The cache identity includes the seed, input files,
registry, protocol, and format version. Delete the cache if generator semantics
change without a format bump. This is a slow, incomplete terrain probe: no structures,
feature-stage ores or trees, authoritative edits, lateral cave lighting, or
radius-32 throughput claim. Remove both paths to return to the original
synthetic preview.

A local release-build cold-cache comparison on a 20-thread Intel i7-13650HX
generated the same 49 chunks at seed 2026 (radius 3, no cache hits). Eight workers
took 3,911 and 4,332 ms with 156–157 MiB peak resident memory; 16 workers took
2,705 and 2,771 ms with 291–292 MiB peak. The preview server was stopped during
these four runs. This measures preparation throughput, not client-visible latency;
the default remains eight workers. The optional last argument to
`prepare_preview_cache` selects `WORKERS` in `1..=20` for the same local test.

In a separate single-worker, cold 4×4-chunk sweep at chunk origin (32,32),
memoizing the two horizontal shift-noise signals by X/Z reduced the measured
column-descent time from 6,585 ms to 4,925–5,273 ms across three candidate
runs. The memo retains at most 1,024 coordinate entries per shift node.
Nine seed-2026 encoded packets matched the preceding build byte-for-byte.
These are local generation timings, not a client render or all-seeds parity
result; future feature placement will add work to cold chunks.

During movement across chunk coordinates, the vanilla preview orders square
chunk shells from the player's current chunk and prefers the forward edge of
each shell. Cold work within three chunks of the player has priority for
750 ms after each change.
Generation of farther uncached chunks resumes when movement pauses, using up to
the configured worker count. A far chunk already under construction
is discarded at a column boundary if it falls outside the new near area.
Previously delivered chunks remain visible until they leave the view. The
current three-chunk square fills before farther chunks are scheduled or sent;
an early far result from the previous center is skipped and may be requested
again. This makes nearby gaps less likely to sit beside distant rendered
islands, but can delay the outer view while a cold nearby chunk finishes.
The worker and queue limits remain bounded. A player
moving continuously can still outrun cold generation; the advertised view
distance is not a guarantee that every outer chunk is ready at once.

For a cold seed-2027 radius-3 preparation (49 chunks, no cache hits), two
workers took 5,933 ms and eight took 2,057 ms on the local test machine;
all 49 encoded packets matched byte-for-byte across both runs. This measures
generation and cache preparation, not client receipt or rendering. Using the
full configured pool while stationary can increase CPU and memory use until
the view is filled.

Unknown fields, remote bind addresses, and invalid or conflicting values are rejected before startup. Loopback remains mandatory; no remote-access switch exists. Logs are line-oriented key-value events (`event`, `state`, `elapsed_ms`, and safe event-specific fields). Lifecycle control events are always emitted; `debug` or `trace` additionally emits connection admission/closure diagnostics. `elapsed_us` and `elapsed_ms` use a monotonic clock from the beginning of `main` to each event. `listener_bound` includes `protocol_ready=false world_ready=false`; `discovery_bound` names only the working discovery transports. A bound socket does not mean either client edition can play. No client payload or raw configuration is logged.

The preview flag is for local protocol testing only. It accepts an unverified name and client UUID, then sends a separate random session UUID. It does not authenticate a Microsoft account, grant permissions, or establish player identity. Keep it disabled when not testing. Its successful response does not imply configuration, play, chunk delivery, or gameplay support.

Exit codes: `0` clean stop or non-running success; `2` CLI usage; `3` configuration; `4` listener startup/bind or signal setup; `5` running listener I/O failure. Process tests use ephemeral ports and real signals. If the pinned toolchain or locked dependencies cannot be obtained, report the acquisition blocker rather than claiming tests passed.

## Continuous integration

`.github/workflows/` holds four workflows; third-party actions are pinned to commit SHAs and run with `contents: read` except where noted.

| Workflow | Trigger | Checks |
| --- | --- | --- |
| `foundation` (`ci.yml`) | every branch push, PR, manual | format, check, dependency license gate, clippy `-D warnings`, tests, build, rustdoc on the pinned toolchain |
| `security-audit` | pushes, PRs, weekly schedule, manual | `cargo-deny check advisories bans sources` against `deny.toml`; the schedule re-tests advisories because the vulnerability database changes over time |
| `spellcheck` | pushes, PRs, manual | `typos` across the repository |
| `release` | `v*` tag push | builds `rustmc-server` release binaries (Linux and Windows) with SHA-256 checksums and attaches them to a **draft** GitHub Release; publishing the draft is a manual owner action (`contents: write` is limited to this workflow) |

`.github/dependabot.yml` opens weekly grouped PRs for Cargo and GitHub Actions updates.

## Opt-in Java 26.3 terrain preview

Use a licensed matching Java 26.3 client. Java 25 is required to run Mojang's
26.3 data generator. The following setup obtains the official server archive
only to prepare a **local** identifier/tag manifest. The preparation script
checks Mojang's published SHA-1 and protocol version. The archive, generated
reports, and manifest stay outside the repository and are not RustMC source
or redistributed assets.

```sh
rustmc_preview_dir="$HOME/.cache/rustmc-preview"
mkdir -p "$rustmc_preview_dir/reports"
curl --fail --location --output "$rustmc_preview_dir/server-26.3.jar"   https://piston-data.mojang.com/v1/objects/33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c/server.jar
(cd "$rustmc_preview_dir/reports" &&   java -DbundlerMainClass=net.minecraft.data.Main -jar "$rustmc_preview_dir/server-26.3.jar" --reports)
cargo run -p rustmc-tools --bin prepare_preview_registry --locked --   "$rustmc_preview_dir/server-26.3.jar"   "$rustmc_preview_dir/preview-registries.toml"   "$rustmc_preview_dir/reports/generated/reports/registries.json"
cat > "$rustmc_preview_dir/preview.toml" <<EOF
schema_version = 1
log_level = "info"
[listener]
bind_address = "127.0.0.1"
port = 25565
max_connections = 2
max_bytes_per_connection = 4096
idle_timeout_ms = 60000
max_connection_lifetime_ms = 60000
local_java_preview = true
preview_registry_manifest = "$rustmc_preview_dir/preview-registries.toml"
preview_seed = 2026
preview_view_distance = 32
EOF
cargo run -p rustmc-server --locked -- --run "$rustmc_preview_dir/preview.toml"
```

For the separate, **not yet live** vanilla chunk adapter, prepare its local
state/biome ID table from the same official reports and manifest:

```sh
cargo run -p rustmc-tools --bin prepare_chunk_registry --locked -- \
  "$rustmc_preview_dir/reports/generated/reports/blocks.json" \
  "$rustmc_preview_dir/preview-registries.toml" \
  "$rustmc_preview_dir/chunk-registry-26.3.json"
```

Keep this generated JSON outside Git. It maps material-rule block names to
the report's default block states and retains explicit property-bearing states.
It is an input to adapter tests and future opt-in integration; the command
above does not make the current preview serve vanilla chunks.

Before comparing an owner-generated 26.3 save, inspect its seed and preset:

```sh
cargo run -p rustmc-tools --bin inspect_vanilla_save --locked -- /path/to/world
```

The tool reads `level.dat` and `data/minecraft/world_gen_settings.dat` locally
and prints only comparison metadata. A world with a nondefault preset or
generation-changing packs needs its own reference data and cannot be scored as
the default preset merely because its client version matches.

Join `127.0.0.1:25565`. The server announces a maximum 32-chunk view; set the
client's render distance separately if desired. The client enters Creative for
terrain inspection, with a fly-speed value ten times the first preview value.
The landscape contains eight original, labelled surface regions. Creative
inventory, block changes, commands, entities, and saves have no server effect.
A single connection is bounded by one pending chunk batch, a 1 MiB inbound
budget during play, and a 10-minute lifetime. The bound listener does not
announce a persistent playable world. See [compatibility](COMPATIBILITY.md)
and [ADR-0013](decisions/ADR-0013.md).
