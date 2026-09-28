# RustMC roadmap

These are outcome gates, not dates or promises. M0 is the only implemented milestone.

| Milestone | Outcome and exit evidence |
| --- | --- |
| M0 Foundation | Architecture, documentation, bootstrap CLI, tests, and local CI definition; see [acceptance](docs/milestones/M0-foundation.md) |
| M1 Runtime | Validated lifecycle, local development listeners, shutdown, startup instrumentation; socket bind is not playable readiness |
| M2 Discovery/status | Pin both edition versions; validate framing, limits, states, and real client discovery |
| M3 First dual-edition join | Authenticated Java and Bedrock clients enter one small world with tested identity/rejoin behavior |
| M4 Persistent basics | Block/inventory consistency, saves, recovery, and restart/crash tests |
| M5 Reference behavior | Incremental versioned survival parity observations and fixtures |
| M6 Pulse–Parcel evaluation | Reference-equivalent replay across partitions plus realistic measured benefit, or reject it |
| M7 Plugin prototype | Capability, permission, timeout, failure, ordering, and isolation tests |
| M8 Public alpha | Packaging, security review, recovery guidance, supported matrix, and approved release |

Java-first implementation within a future milestone does not close a dual-edition exit gate. No milestone beyond M0 is authorized here.
