# M2 — Versioned discovery and status

Status: Java 26.3 discovery verified with a real client; Bedrock real-client check remains open. GitHub [CI run 36544198221](https://github.com/RustMC/RustMC/actions/runs/36544198221) passed for the published M2 discovery commits. The owner authorized initial version selection. Java 26.3 / protocol 777 and Bedrock 1.26.51 / network protocol 2193 were selected on 29 September 2026 from the primary sources and rationale in [compatibility](../COMPATIBILITY.md#m2-discovery-targets-checked-29-september-2026). M2 cannot claim login, gameplay, or world readiness.

## Scope and implementation plan

1. Record exact target client versions, protocol/data sources, and observed vanilla discovery behavior in the compatibility ledger. Keep edition-specific transport and state separate.
2. Design bounded Java framing and handshake/status state transitions for the approved version. Validate lengths and state before allocating or replying; use a version-correct status response only after independent fixture and real-client checks.
3. Design bounded Bedrock discovery on its required transport for the approved version. Establish the datagram and session boundary without implying authentication or play support; document edition differences.
4. Retain loopback-only defaults and explicit readiness. Define admission, timeout, malformed-input, and shutdown behavior for each endpoint. A working status query may establish discovery readiness, never protocol login or playable-world readiness.
5. Add codec and process tests, malformed-input cases, and real-client discovery checks. Keep repeatable evidence and source attribution in public documentation.

## Acceptance criteria

- [x] Owner authorizes exact-version selection; Java and Bedrock targets and their official release/protocol sources are recorded. Wire behavior still needs real-client observation.
- [x] Java endpoint frames and validates versioned discovery input under explicit size and time limits, rejects invalid state transitions, and returns a synthetic-test status response.
- [x] Bedrock endpoint handles versioned discovery on UDP with bounded datagrams and no session or gameplay claim in synthetic tests.
- [x] Both endpoints stay local by default, reject malformed and excessive input without unbounded work, and release sockets on shutdown in process tests.
- [ ] Real clients of both approved versions complete discovery; unsupported versions and differences are stated in the compatibility matrix.
- [x] Formatting, check, Clippy with warnings denied, tests, build, rustdoc, license policy, and GitHub CI pass; reviewed discovery commits and evidence are published.

M2 does not implement login, authenticated sessions, configuration/play packets, shared-world rules, or Minecraft assets. Those remain later milestones.

The M2 test probes are independently written byte sequences for the documented field layouts. They contain no client, server, or game assets. Unit tests cover split frames, older Java protocol number, ping/pong, wrong state, invalid magic, oversized frames and datagrams. Process tests exercise both sockets, malformed input, and SIGINT/SIGTERM cleanup. A Java 26.3 client in the owner-provided Modrinth profile visibly displayed RustMC's status entry and green connection indicator on 29 September 2026; the server logged `status_complete`. A join attempt correctly ended at the unimplemented login boundary. No Bedrock client was observed, so the combined real-client criterion remains unchecked.
