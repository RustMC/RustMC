# M2 — Versioned discovery and status

Status: planning. Exact Java and Bedrock client versions require owner approval before protocol implementation. M1's development socket is not a Minecraft endpoint; M2 cannot claim login, gameplay, or world readiness.

## Scope and implementation plan

1. Record exact target client versions, protocol/data sources, and observed vanilla discovery behavior in the compatibility ledger. Keep edition-specific transport and state separate.
2. Design bounded Java framing and handshake/status state transitions for the approved version. Validate lengths and state before allocating or replying; use a version-correct status response only after independent fixture and real-client checks.
3. Design bounded Bedrock discovery on its required transport for the approved version. Establish the datagram and session boundary without implying authentication or play support; document edition differences.
4. Retain loopback-only defaults and explicit readiness. Define admission, timeout, malformed-input, and shutdown behavior for each endpoint. A working status query may establish discovery readiness, never protocol login or playable-world readiness.
5. Add codec and process tests, malformed-input cases, and real-client discovery checks. Keep repeatable evidence and source attribution in public documentation.

## Acceptance criteria

- [ ] Owner approves exact Java and Bedrock target versions; primary specifications and vanilla observations are recorded with their version and scope.
- [ ] Java endpoint frames and validates versioned discovery input under explicit size and time limits, rejects invalid state transitions, and returns a tested status response.
- [ ] Bedrock endpoint handles versioned discovery on the appropriate transport with bounded datagrams and no session or gameplay claim.
- [ ] Both endpoints stay local by default, reject malformed and excessive input without unbounded work, and release sockets on shutdown.
- [ ] Real clients of both approved versions complete discovery; unsupported versions and differences are stated in the compatibility matrix.
- [ ] Formatting, check, Clippy with warnings denied, tests, build, rustdoc, license policy, and GitHub CI pass; reviewed commits and evidence are published.

M2 does not implement login, authenticated sessions, configuration/play packets, shared-world rules, or Minecraft assets. Those remain later milestones.
