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
python3 scripts/check_dependency_licenses.py
cargo run -p rustmc-server --locked -- --check-config config/rustmc.example.toml
cargo run -p rustmc-server --locked -- --run config/rustmc.example.toml
```

`--run` is a **loopback-only discovery listener**, not a playable world endpoint. It binds TCP for Java 26.3 status and UDP for Bedrock 1.26.51 discovery on the same port. The example uses port `0`; `listener_bound` reports the chosen local port, and `discovery_bound` confirms both transports are ready while login and world readiness remain false. Press Ctrl-C or send SIGTERM for graceful shutdown. No arguments print status and exit without listening. `--help` and `--version` also exit without listening. `--check-config PATH` reads and validates without modifying its input or binding a socket.

Schema 1 requires `schema_version = 1` and `log_level` (`error`, `warn`, `info`, `debug`, or `trace`). The optional `[listener]` table has these M1 defaults and allowed ranges:

| Field | Default | Allowed value |
| --- | --- | --- |
| `bind_address` | `127.0.0.1` | Loopback IP literal only; port belongs in `port` |
| `port` | `0` | `0..=65535`; zero requests an ephemeral port |
| `max_connections` | `8` | `1..=64` simultaneous accepted sockets |
| `max_bytes_per_connection` | `4096` | `1..=65536` bytes read before closure |
| `idle_timeout_ms` | `1000` | `10..=60000` ms without received data |
| `max_connection_lifetime_ms` | `10000` | `10..=60000` ms total; must be at least idle timeout |
| `local_java_preview` | `false` | Boolean; when true, only a Java 26.3 unauthenticated login response is attempted on loopback. Configuration and play remain unsupported. |

Unknown fields, remote bind addresses, and invalid or conflicting values are rejected before startup. Loopback remains mandatory; no remote-access switch exists. Logs are line-oriented key-value events (`event`, `state`, `elapsed_ms`, and safe event-specific fields). Lifecycle control events are always emitted; `debug` or `trace` additionally emits connection admission/closure diagnostics. `elapsed_us` and `elapsed_ms` use a monotonic clock from the beginning of `main` to each event. `listener_bound` includes `protocol_ready=false world_ready=false`; `discovery_bound` names only the working discovery transports. A bound socket does not mean either client edition can play. No client payload or raw configuration is logged.

The preview flag is for local protocol testing only. It accepts an unverified name and client UUID, then sends a separate random session UUID. It does not authenticate a Microsoft account, grant permissions, or establish player identity. Keep it disabled when not testing. Its successful response does not imply configuration, play, chunk delivery, or gameplay support.

Exit codes: `0` clean stop or non-running success; `2` CLI usage; `3` configuration; `4` listener startup/bind or signal setup; `5` running listener I/O failure. Process tests use ephemeral ports and real signals. If the pinned toolchain or locked dependencies cannot be obtained, report the acquisition blocker rather than claiming tests passed.
