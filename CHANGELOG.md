# Changelog

## Unreleased — opt-in Overworld terrain probe

- Added bounded parallel generation of operator-provisioned Java 26.3 Overworld chunks behind an explicit loopback preview setting. The original terrain preview remains the default.
- Added a local, size-bounded immutable chunk packet cache and a spawn prewarmer; it does not save block edits or player state.
- Reduced measured cold generation cost on a pinned four-chunk sweep by about 10.6% through equivalent noise arithmetic, and combined completed chunks into bounded acknowledged batches. Exact vanilla parity and full-client loading speed remain unverified.
- Added bounded X/Z memoization for horizontal shift noise during cold vanilla-preview generation; a local 4×4-chunk sweep improved, with nine encoded packets unchanged. Feature-stage generation and client-visible speed remain open.
- Corrected the read-only region oracle's chunk-length handling for unpadded Java 26.3 save files and recorded a seed-2027 comparison sample.

## Unreleased — Milestone 2 discovery

- Added bounded Java 26.3 status/ping and Bedrock 1.26.51 UDP discovery paths with independent codec and process tests. Java server-list status was observed in a matching client; Bedrock real-client evidence remains open. Login and world remain absent.
- Added an independent, deterministic seeded terrain prototype and raw generation-only measurement utility. It is not yet sent to a client.
- Added an opt-in, loopback-only Java login and known-pack exchange experiment for 26.3. A matching client acknowledged the known pack, then timed out awaiting registry data. It is unauthenticated and stops before complete configuration/play; no playable world exists.

## Milestone 1

- Added a bounded loopback-only development listener, strict runtime configuration, lifecycle events, signal shutdown, and process integration tests.
- Added raw startup measurement tooling and a locked dependency-license CI gate. No playable world exists.

## Milestone 0 foundation

- Drafted independent architecture, compatibility boundaries, decisions, and contributor policies.
- Added one-package Rust bootstrap CLI with configuration validation, tests, and CI configuration.

No playable server or release exists.
