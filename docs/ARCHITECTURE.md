# RustMC architecture v0.2 — proposed

M0 implements only bootstrap argument and configuration validation. Every other boundary here is a proposed contract, subject to owner review and test results. The initial deployment proposal is one process on one machine.

## Ownership and flow
A future Java gateway and a separate Bedrock gateway own edition-specific transport, sessions, authentication, and packet validation. They cannot mutate authoritative state. After authentication, an intent boundary will attach identity and per-session sequence, validate ownership and permissions, and admit input through bounded queues. The gameplay core will own authoritative rules, state, and tick ordering without packet IDs or socket types. An executor will apply the same transitions in reference or experimental mode. Only committed immutable effects flow to per-player output/interest and persistence. Administration and future plugins enter through validated, permission-controlled intents; they receive no unrestricted mutable world reference. Logging and metrics observe, without determining gameplay.

```
Java gateway ─┐
              ├─> authenticated intent boundary ─> gameplay core/executor ─> committed effects ─> output gateways
Bedrock gateway ┘                                                       └─> persistence
```

Failures must be explicit: invalid packets/intents are rejected; bounded admission defines overload responses; storage cannot acknowledge durable state before its durability condition holds. No M0 network, recovery, or gameplay lifecycle exists. The CLI exits after reporting bootstrap status or config validation.

## Correctness contract
Start future simulation with a single-authoritative-writer reference executor. It is an oracle for parallel equivalence, not proof of vanilla correctness; vanilla behavior needs separate versioned observations. Replay records initial state, rules/data versions, seed and random state, tick admission, ordered player/admin intents, and recorded external results. Wall-clock reads, unordered iteration, worker scheduling, and asynchronous completion cannot decide authoritative outcomes. Independent network, compression, and disk work may run outside the serial critical path when safe.

## Pulse–Parcel hypothesis
A **Pulse** is a numbered logical tick with a proposed 50 ms budget when not overloaded. A **Cell** is a candidate ownership unit of undecided size. A **Parcel** temporarily groups cells under one mutable owner. A **Transfer** crosses owners with ordering and visibility that must preserve gameplay semantics. None is implemented in M0 or an established speed claim.

- Stale prior-pulse boundary snapshots may delay same-tick mechanics. Prove permitted reads per subsystem; coordinate dependent actions in phase or group them.
- Ordering by source parcel changes with repartition. Define stable semantic order independent of parcel and worker; test against reference execution.
- Parcel-local random streams can change outcomes. Preserve target rule random consumption and replay state through partition changes.
- Dense hotspots may remain serial. Measure before splitting; serialize, group, or coordinate dependencies.
- Global barriers can make one slow parcel stall all. Measure wait and critical path; keep reference fallback.
- Disk queues are finite. Define capacity, admission/pause/fail-safe behavior and never silently drop required saves.
- Migration needs ownership epoch and committed transfer point, with no lost/duplicate actions or changed entity IDs.
- In-process native plugins do not guarantee fault isolation. Runtime and isolation are deferred.

Reject or revise Pulse–Parcel if same-tick, partition-independent replay, migration, recovery, or realistic load evidence fails. See [ADR-0005](decisions/ADR-0005.md).

## Readiness and capacity
Future events are `process_started`, `listeners_bound`, `protocol_ready`, and `world_ready`/first playable join. A bound port proves no playable readiness. Recovery, authentication, spawn preparation, and enabled plugins must be ready before claiming world readiness. A generated immutable Boot Image is a candidate optimization only; compare against simple loading and measure generation, corruption, first-join, and first-chunk costs.

Every future queue needs capacity, admission rule, timeout, and overload behavior. Only proven safe events may be coalesced; required actions, inventory transactions, and durable records cannot be silently dropped. Capacity must be evaluated for spread-out, clustered, movement-heavy, and entity-heavy worlds; no single player maximum is claimed.
