# M1 — Runtime and startup baseline

Status: M1 implementation gate verified; owner review pending. GitHub [foundation CI run 36495576589](https://github.com/RustMC/RustMC/actions/runs/36495576589) passed for the M1 code and documentation after the license-check fix. M1 adds infrastructure for a development process, not a Minecraft server. The Java/Bedrock gateways, gameplay rules, world, plugins, and `protocol_ready`/`world_ready` states remain unimplemented. Proposed gameplay and protocol choices stay proposed.

## Implementation plan

1. Extend schema 1 configuration with only the listener and resource settings the runtime uses. Retain strict unknown-field rejection and the existing CLI validation command. Loopback is mandatory in M1; port `0` requests an ephemeral development port.
2. Add a single-process, single-threaded listener supervisor with explicit `configuring`, `starting`, `bound`, `stopping`, `stopped`, and `failed` states. Emit stable structured events and monotonic process-to-bind timing. No listener event means protocol or world readiness.
3. Admit at most the configured number of connections, bound bytes read per connection, apply an idle timeout, and close all accepted sockets on graceful shutdown. Reject saturated or invalid connections without a protocol response. A short polling interval bounds shutdown latency without orphaned workers.
4. Wire SIGINT/SIGTERM to graceful shutdown on supported Linux development hosts. Keep CLI/configuration, bind, and runtime failures on distinct exit codes.
5. Add unit and process integration tests using loopback and ephemeral ports. Record raw cold/warm process-to-bind observations separately from build time and explain their limits.
6. Review the dependency/license ledger, documentation, staged diff, and CI result before marking M1 complete.

## Acceptance criteria

- [x] `--check-config` accepts the example and rejects missing/malformed/unknown/unsupported fields, non-loopback addresses, invalid ports/limits/timeouts, and conflicting address-plus-port input without exposing raw values.
- [x] `--run <path>` reports `process_started`, configuration, starting, and bound events with a concrete local address and elapsed time; it never reports `protocol_ready` or `world_ready` as true.
- [x] A loopback listener accepts only bounded development connections, closes on read limit or timeout, and closes/rejects excess connections; no Minecraft packets are sent.
- [x] SIGINT/SIGTERM and injected pre-bind shutdown reach stopped state, release sockets/connections, and leave no worker tasks. Bind conflict and runtime failure paths produce documented nonzero exit codes.
- [x] Unit and isolated CLI/integration tests cover the above behavior with ephemeral ports and event-based synchronization, not fixed-port sleeps.
- [x] Logs expose lifecycle, admission, closure/rejection, and timings without config contents or secrets. Raw repeatable cold/warm measurements and method are documented as local observations, not a record.
- [x] Cargo format, check, clippy with denied warnings, tests, build, rustdoc, documentation links, and Git diff checks passed locally; the reviewed M1 commits were pushed and GitHub CI run 36495576589 passed.

Evidence: 10 Rust unit tests, 7 CLI/process integration tests, and 2 license-policy tests passed locally and in CI. The [benchmark record](../BENCHMARKS.md#m1-local-startup-observations-29-september-2026) contains ten raw startup observations with cache limitations. The [README checklist](../../README.md#development-checklist) remains the canonical feature status. M1 can satisfy only its infrastructure entries; every Java, Bedrock, world, gameplay, and playable-readiness entry stays unchecked.
