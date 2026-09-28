# RustMC requirements and decisions

## Confirmed direction
RustMC is an independently developed Rust server project. The long-term goal is a vanilla-compatible multiplayer world with Java and Bedrock clients in the first release advertised as dual-edition playable. Startup speed, capacity, and later plugins matter, but require reproducible evidence. M0 is a bootstrap scaffold only: no login, protocol, world, gameplay, or performance result exists.

## Decisions awaiting owner review
| Decision | Current proposal or state | Approval point |
| --- | --- | --- |
| Shared-world rules | Java-style rules with Bedrock client translation; edition behavior can differ | Before gameplay implementation |
| Protocol targets | One exact Java and one exact Bedrock version | Before M2 protocol implementation |
| Deployment | One process on one machine initially | Before runtime design is fixed |
| Development platform | Linux/Fedora first; others untested | Before claiming wider support |
| License | Undecided; no external contributions accepted yet | Before public launch |
| Security reporting | Private route undecided | Before public launch |
| Repository/visibility | Local repository only; intended remote and owner unverified | Before remote creation or push |
| Libraries | General-purpose dependencies allowed after provenance/license review | Before adding each dependency |

No proposed decision is recorded as approved. See the [architecture](ARCHITECTURE.md) and [ADRs](decisions/ADR-0001.md).
