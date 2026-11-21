# Status

Last updated: 21 November 2026.

Progress: 1 of 9 plan phases complete (11%).

The plan has nine phases: 0 review and cleanup, 1 playable core, 2 proof and
hardening, 3 survival basics, 4 mobs and combat, 5 vanilla-accurate terrain, 6
Bedrock, 7 release hardening, 8 plugin API. A phase counts as complete only when
its exit criteria pass; the percentage above is that count, not an estimate of
effort.

## Done

- **Phase 0, review and cleanup** (merged in pull request #140). Every module is
  mapped in [REVIEW.md](REVIEW.md); unreachable subsystems were removed with
  restore points; every source file is under 1,000 lines; production panics
  and duplicated limits were removed.
- **Discovery and preview** (earlier milestones). Java 26.3 status responses, an
  opt-in loopback terrain preview observed in a real 26.3 client, and an opt-in
  data-driven Overworld generator measured against a vanilla save (see
  [PARITY.md](PARITY.md)).

## In progress

- **Phase 1 A, network and sessions** (pull request #141, not merged). Offline
  and online-mode login with encryption and session-server verification,
  compression, keep-alive, bounded queues and per-connection limits, covered
  by end-to-end socket tests. Not yet exercised with a real client or the real
  session server.
- **Phase 1 B, world streaming** (branch `feat/world-streaming`, no pull
  request yet). Default terrain stored as 26.3 block states, a chunk worker
  pool and per-player streaming with forward priority.

## Not done

- Phase 1 C (persistence), D (play: movement checks, block break and place,
  other players, inventory) and E (chat, commands with permissions, server
  config, logging).
- Phases 2 to 8.
- There is no playable world on `main` yet: the server answers status pings
  and runs the opt-in preview, which is read-only and single-player.

## Known bugs

- The CodeQL check on pull request #141 reports hard-coded keys in test code
  (fixed test vectors); they need triage before that request can merge.

## Next

Finish Phase 1 (tracks C, D and E), then Phase 2: bot-client restart,
two-client and load tests.
