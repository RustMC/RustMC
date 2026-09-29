# RustMC roadmap

This is the professional M0–M8 sequence of outcome gates, separate from the [README development checklist](README.md#development-checklist). Dates and performance targets are not promises. M0 is complete, M1 infrastructure passed its local and GitHub CI gates, and M2 discovery has local codec/process tests plus Java 26.3 client status evidence; Bedrock client discovery remains open. A separate Java-only Creative terrain preview has been observed in a 26.3 client; it does not close the M3 dual-edition gate. The first usable vanilla multiplayer scope must be demonstrated before any release claim, and plugin work follows the core gameplay work.

| Milestone | Outcome | Exit evidence |
| --- | --- | --- |
| M0 Foundation | Architecture, compatibility boundaries, bootstrap CLI, tests, and CI | [M0 acceptance](docs/milestones/M0-foundation.md) and passing CI; no playable server |
| M1 Runtime and startup | Validated lifecycle, local development listeners, bounded work, shutdown, observability | Failure and overload tests plus raw startup measurements; binding is not playable readiness |
| M2 Discovery and status | Select both edition versions; separate framing, limits, protocol states, and discovery | Real Java and Bedrock client discovery, malformed-input tests, documented limits |
| M3 First dual-edition join | Authenticated sessions enter the same small fixed world with minimal visibility | Two real clients, identity and join/disconnect/rejoin tests, translation limits |
| M4 Persistent basics | Block interaction, inventory consistency, save ordering, recovery, multiplayer synchronization | Restart/crash tests with no lost or duplicated covered transactions |
| M5 Vanilla behavior and usable multiplayer | Incremental workstreams for world/chunks, players, blocks, items, entities/AI, survival, commands, and versioned parity | Scoped playable world with real clients, reference observations, persistence, and explicit known gaps; not one blanket vanilla claim |
| M6 Pulse–Parcel evaluation | Compare reference and parallel execution under migration and dense/dependent workloads | Replay-equivalent outcomes across supported partitions/workers plus measured benefit, or reject the experiment |
| M7 Plugin API prototype | Versioned capabilities, hooks, permission and isolation model | Failure, timeout, ordering, resource, and compatibility tests; not required for the first usable world |
| M8 Public alpha hardening | Packaging, upgrade/recovery guidance, security review, realistic mixed-client load testing | Reproducible build, documented support matrix and limits, known issues, owner-approved release |

M5 is a collection of substantial behavior workstreams, not a single ticket. Java-first work inside a milestone does not close a dual-edition gate without Bedrock evidence. A first playable world does not establish complete vanilla parity or a public release. The experimental scheduler and plugin runtime can be rejected or deferred without changing that correctness requirement.
