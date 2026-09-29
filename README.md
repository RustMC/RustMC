# RustMC

RustMC is an independently developed Minecraft-compatible server project written in Rust. The goal is a vanilla-style multiplayer world for Java and Bedrock clients, with compatibility and performance claims backed by tests and measurements.

> **Discovery runtime / pre-alpha:** RustMC validates configuration and answers bounded Java status and Bedrock UDP discovery probes on loopback. It cannot log in a game client or run a world. Do not use it for production worlds.

The proposed shared-world baseline is Java-style gameplay with Bedrock client translation. That choice still needs owner approval; Bedrock support would not mean identical edition rules. Initial discovery targets are Java 26.3 and Bedrock 1.26.51. See the [compatibility matrix](docs/COMPATIBILITY.md).

## Contents

- [Current status and quick start](#current-status-and-quick-start)
- [World generation and client join work — 29 September 2026](#world-generation-and-client-join-work--29-september-2026)
- [Development checklist](#development-checklist)
  - [Completed M0 foundation](#completed-m0-foundation)
  - [Decisions](#decisions)
  - [Configuration and server operations](#configuration-and-server-operations)
  - [Java protocol](#java-protocol)
  - [Bedrock protocol](#bedrock-protocol)
  - [Shared-world core](#shared-world-core)
  - [Players and multiplayer](#players-and-multiplayer)
  - [World, dimensions, chunks, and saving](#world-dimensions-chunks-and-saving)
  - [Blocks, fluids, redstone, and scheduled updates](#blocks-fluids-redstone-and-scheduled-updates)
  - [Items, inventories, crafting, and containers](#items-inventories-crafting-and-containers)
  - [Entities, mobs, pathfinding, and AI](#entities-mobs-pathfinding-and-ai)
  - [Combat, survival, effects, and progression](#combat-survival-effects-and-progression)
  - [Commands, permissions, and administration](#commands-permissions-and-administration)
  - [Performance, concurrency, and recovery](#performance-concurrency-and-recovery)
  - [Security, compatibility, documentation, and release](#security-compatibility-documentation-and-release)
  - [Plugin API (future)](#plugin-api-future)
- [Architecture, roadmap, and contribution](#architecture-roadmap-and-contribution)

## Current status and quick start

| Capability | Status |
| --- | --- |
| `--help`, `--version`, and `--check-config` | Implemented and tested; schema 1 now includes M1 listener settings |
| `--run` local discovery listener | Bounded loopback TCP and UDP; process tests pass on Linux |
| Java 26.3 status and Bedrock 1.26.51 discovery | Partial: synthetic socket tests pass; real clients not yet checked |
| Java or Bedrock login and play | Planned; no playable endpoint exists |
| World, chunks, players, inventory, and survival | Planned; no gameplay exists |
| Pulse–Parcel parallel execution | Experimental design only |
| Plugins | Deferred |
| Server performance or player capacity | No results |

The tested Rust toolchain is pinned in `rust-toolchain.toml`; Linux is the initially tested platform. From a clone:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo run -p rustmc-server --locked -- --help
cargo run -p rustmc-server --locked -- --version
cargo run -p rustmc-server --locked -- --check-config config/rustmc.example.toml
cargo run -p rustmc-server --locked -- --run config/rustmc.example.toml
```

The final command binds loopback TCP and UDP on the reported ephemeral port, answering only discovery probes. Stop it with Ctrl-C. Running without arguments exits without opening sockets or creating world data. See [development](docs/DEVELOPMENT.md).

## World generation and client join work — 29 September 2026

This dated slice tracks the next evidence gate; it does not declare vanilla parity or multiplayer support. The [development checklist](#development-checklist) remains the canonical feature status.

- [ ] Finish bounded Java 26.3 status and Bedrock 1.26.51 UDP discovery, including malformed input, shutdown, and source evidence.
- [ ] Establish a documented local development identity path and complete Java 26.3 login, configuration, and play transitions with a real client.
- [ ] Send version-correct initial position, chunk, biome, heightmap, and lighting data and observe original chunks rendered in a matching client.
- [ ] Generate deterministic seeded grass terrain, ground layers, clearings, and trees by world coordinate, including seam-free chunk borders.
- [ ] Load nearby chunks as the player moves, within documented view, work, memory, and queue limits; test disconnect and rejoin.
- [ ] Pass protocol, generation, malformed-input, process, shutdown, and repository CI checks; record generation, encoding, and delivery timings separately.
- [ ] Record direct evidence of a Java 26.3 client joining and visibly rendering RustMC chunks. Leave this unchecked until the real client has been observed.

## Development checklist

This is the canonical **feature coverage map**, separate from the [M0–M8 roadmap](ROADMAP.md). Every unchecked item is planned or not yet fully verified, even if partial code exists. A checked feature must be implemented and tested for a stated edition/version and scope; partial or unknown behavior stays unchecked and is recorded in the [compatibility matrix](docs/COMPATIBILITY.md). Each future feature needs independent acceptance tests and source provenance. Version-dependent behavior must be pinned before implementation. M1 infrastructure passed its local and GitHub CI gates; M2 discovery has local process evidence but no real-client evidence yet.

### Completed M0 foundation

- [x] Document requirements, proposed architecture, ADRs, compatibility statuses, testing and benchmark methods, and contribution/provenance policies.
- [x] Build the one-package bootstrap CLI with a pinned toolchain, example TOML configuration, unit tests, and isolated CLI failure tests. Its configuration check does not start a server.
- [x] Publish the reviewed M0 commits and pass formatting, lint, build, test, rustdoc, and [foundation CI](https://github.com/RustMC/RustMC/actions/runs/36492068809).

### Decisions

- [x] Adopt Apache-2.0 for RustMC original source and review current Cargo dependency license metadata; preserve third-party notices for future binaries.
- [ ] Establish a working private security-reporting route before inviting reports or a public release.
- [ ] Approve shared-world gameplay rules, including how Bedrock differences are represented, before gameplay implementation.
- [x] Select exact Java and Bedrock discovery targets and record primary version/protocol sources: Java 26.3 / 777 and Bedrock 1.26.51 / 2193. Gameplay data and rule sources remain undecided.

### Configuration and server operations

- **M1 runtime; M8 operating guidance**
  - [x] Validate schema 1 runtime settings, loopback address, port, connection/read/time limits, unknown fields, and conflicting timeouts without echoing raw values.
  - [x] Start a local development listener after configuration validation; report process start and socket bind while explicitly marking protocol and world readiness false.
  - [x] Expose configuring, starting, bound, stopping, stopped, and failed states; test startup failure, injected cancellation, SIGINT/SIGTERM, socket cleanup, and exit codes.
  - [x] Emit structured lifecycle and connection diagnostics plus monotonic process-to-bind timings without client payloads or raw configuration.
  - [x] Bound development connections, bytes read, idle time, and total connection life; test capacity rejection and closure.
  - [ ] Add tick, queue, disk, and gameplay metrics when those subsystems exist.
  - [ ] Define process-wide memory, file, and work budgets beyond the current connection limits before wider deployment.

### Java protocol

- **M2 discovery; M3 login/play; later versioned coverage**
  - [ ] Implement transport accept/read/write limits and disconnect handling for the selected Java version; test partial, slow, and oversized input.
  - [ ] Implement packet framing, bounded decoding, encoding, and compression negotiation for the selected version; round-trip and malformed-input tests must pass.
  - [ ] Enforce protocol-state transitions through handshake, status, login, configuration, and play as applicable to the selected version; reject out-of-state packets.
  - [ ] Return version-correct status/discovery responses and verify them with a real Java client.
  - [ ] Authenticate login sessions using an approved identity flow; test invalid, expired, and replayed credentials without inventing cryptography.
  - [ ] Translate validated play packets into sequenced core intents and committed outputs; test permissions, ordering, and disconnect/rejoin behavior.
  - [ ] Record exact packet/data sources and unsupported Java versions in [compatibility](docs/COMPATIBILITY.md).

### Bedrock protocol

- **M2 discovery; M3 sessions/play; later translation coverage**
  - [ ] Implement the selected Bedrock transport and discovery path with bounded datagrams, sessions, timeouts, and malformed-input tests.
  - [ ] Validate session establishment and authentication for the selected version; test invalid identities and session replay/expiry cases.
  - [ ] Decode and encode versioned packets with limits and state checks; test fragmentation or reliability behavior where the approved protocol requires it.
  - [ ] Translate Bedrock player intents into the shared-world model and committed effects back to Bedrock clients; test identity, ordering, and rejection paths.
  - [ ] Decide and test translation for edition differences in commands, redstone/update rules, inventory/UI, world representation, and other observed mechanics; document unsupported cases.
  - [ ] Verify discovery, join, interaction, disconnect, and rejoin with a real Bedrock client; do not infer parity from Java tests.

### Shared-world core

- **M3–M5 authoritative behavior; M6 optimization evaluation**
  - [ ] Define numbered ticks, phase ordering, admission cutoffs, and authoritative state ownership; test same-tick effects and input order.
  - [ ] Validate and sequence player/admin actions at an intent boundary; reject forged ownership, permissions, duplicate actions, and out-of-order input.
  - [ ] Implement a single-authoritative-writer reference executor before parallel gameplay; compare its results with versioned black-box observations.
  - [ ] Record initial state, rules/data version, seed/random state, admitted intents, and external results for deterministic replay; test repeatability.
  - [ ] Define spatial ownership and cross-boundary action semantics without adding an extra tick or partition-dependent order; test dependent neighbors.
  - [ ] Publish only committed immutable effects to gateways, interest management, and persistence; test that no consumer sees partial state.
  - [ ] Define save checkpoints, durable records, and recovery boundaries; test that required writes are neither silently dropped nor acknowledged early.

### Players and multiplayer

- **M3 first joins; M4–M5 playable behavior**
  - [ ] Test authenticated Java and Bedrock join, leave, reconnect, and rejoin in the same fixed world with distinct identities.
  - [ ] Validate movement, collision, teleportation, and position correction against authoritative state; test invalid speed and impossible paths.
  - [ ] Synchronize player visibility, nearby entities, and chunk interest across clients; test entry, exit, and dense-player cases.
  - [ ] Implement health, hunger, damage, death, respawn, and experience for the approved rule set; test state transitions and persistence.
  - [ ] Implement version-dependent game modes and permissions; test that clients cannot grant themselves abilities or bypass restrictions.
  - [ ] Persist player data and equipment across disconnects and crashes without duplication or loss in covered scenarios.

### World, dimensions, chunks, and saving

- **M4 persistence; M5 versioned world behavior**
  - [ ] Define block/biome registries and versioned data provenance; test IDs and translation for both clients.
  - [ ] Load, generate, retain, and unload chunks under bounded memory and I/O; test concurrent requests and lifecycle races.
  - [ ] Implement version-dependent terrain generation, biome placement, and structures with reproducible seed fixtures; mark unsupported parity explicitly.
  - [ ] Compute and update lighting and heightmaps after generation and block changes; test chunk-edge propagation.
  - [ ] Store and restore block entities and scheduled state; test restart and crash recovery.
  - [ ] Implement supported dimensions, portals, time, weather, and environment transitions; test cross-dimension consistency.
  - [ ] Define save format, checkpoint ordering, upgrade/import/export limits, and corruption handling; do not claim vanilla file compatibility without fixtures.

### Blocks, fluids, redstone, and scheduled updates

- **M4 basic interaction; M5 version-dependent mechanics**
  - [ ] Validate placement and breaking against reach, permissions, collision, game mode, tools, and inventory; test multiplayer conflicts.
  - [ ] Apply version-dependent hardness, tool suitability, drops, and block state transitions; compare with observed vanilla behavior.
  - [ ] Schedule and run deterministic block and random ticks with required same-tick dependencies; replay across chunk boundaries.
  - [ ] Implement water/lava flow, interaction, and containment with versioned fixtures and bounded update work.
  - [ ] Implement fire spread, crops, growth, and environmental updates with reproducible random-state tests.
  - [ ] Implement the approved edition's redstone components, power propagation, and update ordering; document Bedrock translation differences.
  - [ ] Implement portal activation and travel only after dimension and persistence rules are tested.

### Items, inventories, crafting, and containers

- **M4 consistency; M5 survival systems**
  - [ ] Define versioned item registries, stack sizes, metadata, and slot rules; reject invalid client-supplied items.
  - [ ] Make pickup, drop, move, split, merge, and consumption atomic transactions; test concurrent players and disconnects.
  - [ ] Synchronize player inventories, equipment, and container viewers after committed changes; test reopen/rejoin consistency.
  - [ ] Validate item use, durability, repair, and equipment effects against approved rules and world state.
  - [ ] Implement versioned recipe matching and crafting outputs; test shaped/shapeless cases and item conservation.
  - [ ] Implement furnace-style processing, fuel, progress, output claims, and persistence; test restart and multi-viewer conflicts.

### Entities, mobs, pathfinding, and AI

- **M5 behavior workstreams**
  - [ ] Define entity IDs, lifecycle, spawn/despawn rules, serialization, and interest visibility; test no duplicate IDs after recovery.
  - [ ] Implement movement, collision, gravity, attributes, and version-dependent physics with replay fixtures.
  - [ ] Implement spawning caps and conditions for supported passive and hostile mobs; test chunk and player proximity effects.
  - [ ] Implement pathfinding over changing terrain with bounded work; test stale paths and unreachable goals.
  - [ ] Implement sensing and prioritized goals/behaviors for each supported mob; test ordering and deterministic outcomes.
  - [ ] Implement mob combat, projectiles, vehicles, breeding, drops, and persistence in separately tested slices.
  - [ ] Keep unsupported entity types marked `planned` or `partial` in the compatibility matrix until client and behavior tests pass.

### Combat, survival, effects, and progression

- **M5 behavior workstreams**
  - [ ] Implement version-dependent attack timing, damage, armor, knockback, projectiles, and PvP interaction; test multiplayer outcomes.
  - [ ] Implement food, hunger, regeneration, exhaustion, drowning, fall damage, and other supported survival hazards.
  - [ ] Apply and expire status effects, enchantments, and equipment modifiers in deterministic order; test stacking and persistence.
  - [ ] Implement loot tables and drops with sourced versioned data and replayable randomness; review asset/data rights.
  - [ ] Implement experience gains, levels, death loss, and supported advancements or achievements where applicable; document edition differences.
  - [ ] Test a scoped first usable vanilla multiplayer world with real Java and Bedrock clients, persistence, known gaps, and security review before any release claim.

### Commands, permissions, and administration

- **M5 operator and gameplay behavior**
  - [ ] Parse and authorize player/admin commands for approved versions; reject forged or over-privileged requests.
  - [ ] Implement supported world rules, time/weather controls, and player management with audit-safe results.
  - [ ] Implement version-dependent scoreboards, teams, and command feedback only with client and behavior tests.
  - [ ] Add administration interfaces through validated intents, never unrestricted mutable world access; test ordering with gameplay.
  - [ ] Document command differences and unsupported behavior separately for Java and Bedrock.

### Performance, concurrency, and recovery

- **M1 measurements; M4 recovery; M6 experiment; M8 release evidence**
  - [x] Record raw repeated M1 code-entry-to-bind and parent-observed bind measurements with method and cache limitations; keep build time separate.
  - [ ] Measure protocol/world readiness, first playable join/chunk, and dirty recovery when those features exist.
  - [ ] Benchmark spread-out, clustered, movement-heavy, and entity-heavy player scenarios with hardware, build, versions, raw timings, and adequate samples.
  - [ ] Profile tick critical path, visibility/output, queue depth, barrier wait, disk backpressure, and overload behavior; publish limits rather than a single unsupported player count.
  - [ ] Compare any Boot Image or other startup optimization with simple loading, including generation cost and first-join latency.
  - [ ] Evaluate Pulse–Parcel only against the reference executor across workers, partition changes, migration epochs, same-tick boundaries, and random-state replay.
  - [ ] Reject or revise parallel execution on semantic divergence, unsafe recovery, or lack of measured benefit; keep a reference fallback.
  - [ ] Crash-test checkpoints and durable records under full queues, slow disks, and process termination; verify no acknowledged data loss in covered cases.

### Security, compatibility, documentation, and release

- **M2–M8 quality gates**
  - [ ] Fuzz versioned protocol codecs and test malformed lengths, state transitions, decompression limits, and abuse throttling for both editions.
  - [ ] Review authentication, session handling, permissions, secrets, dependency advisories, and third-party licenses before accepting contributions or binaries.
  - [ ] Compare supported behavior with observed vanilla Java and Bedrock clients; record source, version, scope, fixture, and test evidence for every `tested` matrix entry.
  - [ ] Keep README, roadmap, ADRs, compatibility status, operator docs, and changelog aligned with actual features and limits.
  - [ ] Document installation, configuration, backup, recovery, upgrade, and known incompatibilities for a scoped public alpha.
  - [ ] Reproduce release builds and mixed-client load tests; publish raw results and obtain owner approval before packaging or announcing a release.

### Plugin API (future)

- **M7 prototype; not a prerequisite for a correct first world**
  - [ ] Define versioned capabilities, events/hooks, and intent-based APIs with explicit ordering and compatibility rules.
  - [ ] Choose and test an isolation model; in-process native code must not be described as crash-isolated.
  - [ ] Enforce permissions and resource budgets; test denial, timeout, failure, and recovery without corrupting world state.
  - [ ] Test plugin upgrade/version mismatch and deterministic interaction with gameplay and persistence.
  - [ ] Review plugin dependency licenses and provenance before inviting third-party extensions.

## Architecture, roadmap, and contribution

The [architecture](docs/ARCHITECTURE.md) and [ADRs](docs/decisions/ADR-0001.md) describe proposed boundaries; [testing](docs/TESTING.md), [benchmark methodology](docs/BENCHMARKS.md), and the [roadmap](ROADMAP.md) define evidence and milestone gates. Read [contribution guidance](CONTRIBUTING.md), [provenance](docs/PROVENANCE.md), and [security guidance](SECURITY.md) before proposing work. External contributions remain paused until a private reporting route and contribution process are approved.

RustMC original source code is licensed under [Apache License 2.0](LICENSE). Dependencies keep their own licenses; this grant does not cover Minecraft assets or third-party code. RustMC is an independent, unofficial project, neither approved by nor associated with Mojang or Microsoft.
