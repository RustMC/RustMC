# Development

Linux/Fedora is the initially tested environment. Install Git and rustup from trusted sources; `rust-toolchain.toml` pins Rust 1.98.1 with rustfmt and clippy. Other platforms are unverified. Current direct libraries are `toml` for configuration parsing, `signal-hook` for Unix shutdown signals, and `uuid` for a random, connection-scoped preview session ID; see [provenance](PROVENANCE.md) for locked licenses and review scope. Do not update dependencies or toolchain merely to hide a failing check.

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
| `max_connection_lifetime_ms` | `10000` | `10..=60000` ms total; must be at least idle timeout |
| `local_java_preview` | `false` | Boolean; enables the unauthenticated local Java 26.3 Creative terrain preview when a matching manifest is supplied. |
| `preview_registry_manifest` | absent | Path to locally prepared, version-checked 26.3 registry identifier/tag metadata. Required for world entry. |
| `preview_seed` | `0` | Integer `0..=9223372036854775807`, used only for original preview generation. |
| `preview_view_distance` | `4` | Integer `2..=32` chunks. Radius 32 permits up to 4,225 loaded chunk coordinates per client; only one batch is in flight, containing at most 16 chunks within a 768 KiB encoding budget. |

Unknown fields, remote bind addresses, and invalid or conflicting values are rejected before startup. Loopback remains mandatory; no remote-access switch exists. Logs are line-oriented key-value events (`event`, `state`, `elapsed_ms`, and safe event-specific fields). Lifecycle control events are always emitted; `debug` or `trace` additionally emits connection admission/closure diagnostics. `elapsed_us` and `elapsed_ms` use a monotonic clock from the beginning of `main` to each event. `listener_bound` includes `protocol_ready=false world_ready=false`; `discovery_bound` names only the working discovery transports. A bound socket does not mean either client edition can play. No client payload or raw configuration is logged.

The preview flag is for local protocol testing only. It accepts an unverified name and client UUID, then sends a separate random session UUID. It does not authenticate a Microsoft account, grant permissions, or establish player identity. Keep it disabled when not testing. Its successful response does not imply configuration, play, chunk delivery, or gameplay support.

Exit codes: `0` clean stop or non-running success; `2` CLI usage; `3` configuration; `4` listener startup/bind or signal setup; `5` running listener I/O failure. Process tests use ephemeral ports and real signals. If the pinned toolchain or locked dependencies cannot be obtained, report the acquisition blocker rather than claiming tests passed.

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

Join `127.0.0.1:25565`. The server announces a maximum 32-chunk view; set the
client's render distance separately if desired. The client enters Creative for
terrain inspection, with a fly-speed value ten times the first preview value.
The landscape contains eight original, labelled surface regions. Creative
inventory, block changes, commands, entities, and saves have no server effect.
A single connection is bounded by one pending chunk batch, a 1 MiB inbound
budget during play, and a 10-minute lifetime. The bound listener does not
announce a persistent playable world. See [compatibility](COMPATIBILITY.md)
and [ADR-0013](decisions/ADR-0013.md).
