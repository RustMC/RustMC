# RustMC

An independently developed Minecraft-compatible server project written in Rust. RustMC aims to provide a vanilla-style multiplayer world for Java and Bedrock clients, with measured startup and capacity and a plugin boundary later.

> **Foundation / pre-alpha:** RustMC is currently a bootstrap CLI and configuration validator, not a playable Minecraft server. No client login, world, gameplay, or performance result exists. Do not use it for production worlds.

| Capability | Current state |
| --- | --- |
| Bootstrap CLI and configuration validation | Implemented and tested in M0 |
| Java and Bedrock login/gameplay | Planned |
| Vanilla survival and compatible storage | Planned |
| Pulse–Parcel parallel execution | Experimental design only |
| Plugin runtime/API | Deferred |
| Server performance results | None |

The proposed shared world uses Java-style rules with Bedrock client translation, pending owner approval. Bedrock support would not imply identical edition behavior. See [compatibility](docs/COMPATIBILITY.md).

## Contents

- [Developer quick start](#developer-quick-start)
- [Architecture and project guidance](#architecture-and-project-guidance)
- [Project checklist](#project-checklist)
  - [M0 — Foundation](#m0--foundation)
  - [M1 — Runtime and startup](#m1--runtime-and-startup)
  - [M2 — Protocol discovery and status](#m2--protocol-discovery-and-status)
  - [M3 — First dual-edition join](#m3--first-dual-edition-join)
  - [M4 — Persistent world basics](#m4--persistent-world-basics)
  - [M5 — Vanilla behavior and usable multiplayer](#m5--vanilla-behavior-and-usable-multiplayer)
  - [M6 — Parallel execution evaluation](#m6--parallel-execution-evaluation)
  - [M7 — Plugin support prototype](#m7--plugin-support-prototype)
  - [M8 — Public alpha release](#m8--public-alpha-release)
- [Contributing, security, and license](#contributing-security-and-license)

## Developer quick start

The tested toolchain is pinned in `rust-toolchain.toml`; Linux is the initially tested platform. From a clone:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo run -p rustmc-server --locked -- --help
cargo run -p rustmc-server --locked -- --version
cargo run -p rustmc-server --locked -- --check-config config/rustmc.example.toml
```

These commands build and validate the scaffold. Running with no arguments reports bootstrap-only status and exits without opening sockets or creating a world. See [development](docs/DEVELOPMENT.md).

## Architecture and project guidance

The proposed [architecture](docs/ARCHITECTURE.md) keeps Java and Bedrock gateways separate from an authoritative gameplay core. [Decisions](docs/PROJECT_DECISIONS.md), [testing](docs/TESTING.md), [benchmark methodology](docs/BENCHMARKS.md), [provenance](docs/PROVENANCE.md), and the [roadmap](ROADMAP.md) give the contracts and exit gates. The checklist below is the canonical task status; check a box only after its evidence exists.

## Project checklist

These are outcome gates, not dates. Checked M0 items are supported by the [M0 commits and acceptance record](docs/milestones/M0-foundation.md) and a passing [foundation CI run](https://github.com/RustMC/RustMC/actions/runs/36492068809). All later work remains planned and requires separate authorization. A feature is not usable merely because its design is documented.

### M0 — Foundation

- [x] Define requirements, non-goals, and versioned Java/Bedrock compatibility statuses.
- [x] Document proposed architecture, separate gateways, reference-executor contract, and Pulse–Parcel rejection tests in ADRs.
- [x] Publish README, roadmap, development/testing/benchmark guides, contributor and provenance policies, and security policy draft.
- [x] Build one pinned, non-publishable Rust package with `--help`, `--version`, and validated bootstrap-only configuration.
- [x] Test CLI success and failure paths, malformed/missing/unknown/unsupported configuration, and non-sensitive error handling.
- [x] Run formatting, lint, build, tests, rustdoc, and a passing GitHub foundation CI job; publish the three reviewed M0 commits.
- [ ] Select a project license and review dependency license compatibility before accepting external contributions or releasing binaries.
- [ ] Establish a working private vulnerability-reporting route before inviting security reports or public release.
- [ ] Approve the shared-world gameplay baseline before implementing game rules.
- [ ] Select exact Java and Bedrock protocol versions and source data before M2 implementation.

### M1 — Runtime and startup

- [ ] Specify lifecycle, failure states, bounded queues, shutdown, and M1 acceptance tests before implementation.
- [ ] Add validated runtime configuration and local development listeners without claiming playable readiness.
- [ ] Instrument process start, listener bind, protocol readiness, world readiness, first join, and recovery as distinct future events.
- [ ] Test startup failures, cancellation, resource cleanup, and shutdown; record raw startup measurements under stated conditions.

### M2 — Protocol discovery and status

- [ ] Approve exact Java and Bedrock targets, official protocol references, and data provenance.
- [ ] Implement separate versioned transport/framing, limits, state validation, and status/discovery responses for both editions.
- [ ] Test malformed packets, size/rate limits, timeouts, and real-client discovery; document unsupported versions and behavior.
- [ ] Review authentication and protocol attack surfaces before enabling login work.

### M3 — First dual-edition join

- [ ] Implement authenticated Java and Bedrock sessions with validated identities, bounded input admission, and permission checks.
- [ ] Translate both clients into one small authoritative world without equating their game rules.
- [ ] Synchronize minimal position and visibility; test join, disconnect, reconnect, and two-client interaction with real clients.
- [ ] Record edition-specific limitations and ensure the join gate requires evidence from both editions.

### M4 — Persistent world basics

- [ ] Implement basic block interaction, inventory transaction consistency, and multiplayer state synchronization.
- [ ] Define save ordering, durable acknowledgment, bounded disk backpressure, and recovery behavior.
- [ ] Test restart/crash recovery and no lost or duplicated covered inventory/world actions.
- [ ] Document world-format support and import/export limits rather than assuming vanilla save compatibility.

### M5 — Vanilla behavior and usable multiplayer

- [ ] Build a single-authoritative-writer reference executor and versioned replay fixtures before parallel gameplay.
- [ ] Grow the [compatibility matrix](docs/COMPATIBILITY.md) through independently observed, version-specific rules for commands, recipes, interactions, scheduled updates, entities, and world generation.
- [ ] Test Java behavior and Bedrock translation differences with real multiplayer sessions, including same-tick ordering and recovery.
- [ ] Define a scoped first usable vanilla multiplayer release gate: supported versions, playable world behavior, persistence, known gaps, security review, and documented operating limits.
- [ ] Mark only tested features as supported; defer release if core survival or dual-edition evidence is incomplete.

### M6 — Parallel execution evaluation

- [ ] Record deterministic replay inputs and compare reference outcomes across workers, partitions, and migrations.
- [ ] Test same-tick boundary mechanics, randomness, ordering, ownership transfer, barriers, and disk backpressure.
- [ ] Benchmark spread-out, clustered, movement-heavy, and simulation-heavy workloads with raw timings and full environment details.
- [ ] Adopt, revise, or reject Pulse–Parcel based on correctness and measured benefit; publish no capacity record without reproducible evidence.

### M7 — Plugin support prototype

- [ ] Choose an isolation/runtime model and versioned capability API after reviewing security and license implications.
- [ ] Route plugins through validated, permission-controlled intents; prohibit unrestricted mutable world access.
- [ ] Test permission denial, timeouts, crashes, ordering, resource budgets, and version compatibility.
- [ ] Document plugin limitations and review third-party provenance before accepting extensions.

### M8 — Public alpha release

- [ ] Resolve license and private security reporting, review dependencies and third-party data, and complete threat and recovery guidance.
- [ ] Publish an evidence-backed Java/Bedrock support matrix, known issues, upgrade path, and operator documentation.
- [ ] Reproduce packaging/builds and mixed-client load tests; report hardware, workload, raw data, latency, and overload limits.
- [ ] Obtain owner approval for release scope, then publish binaries and release notes only for verified capabilities.

## Contributing, security, and license

Read [contribution guidance](CONTRIBUTING.md) and [provenance policy](docs/PROVENANCE.md). External contributions await a selected license. RustMC is experimental; [security guidance](SECURITY.md) records the pending private route. Do not post secrets or vulnerability details in public issues.

The project license is awaiting maintainer selection; see [decisions](docs/PROJECT_DECISIONS.md). RustMC is independent, unofficial, and neither approved by nor associated with Mojang or Microsoft.
