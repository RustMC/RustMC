# Changelog

## Unreleased — opt-in Overworld terrain probe

- Added bounded parallel generation of operator-provisioned Java 26.3 Overworld chunks behind an explicit loopback preview setting. The original terrain preview remains the default.
- Added a local, size-bounded immutable chunk packet cache and a spawn prewarmer; it does not save block edits or player state.
- Reduced measured cold generation cost on a pinned four-chunk sweep by about 10.6% through equivalent noise arithmetic, and combined completed chunks into bounded acknowledged batches. Exact vanilla parity and full-client loading speed remain unverified.
- Added bounded X/Z memoization for horizontal shift noise during cold vanilla-preview generation; a local 4×4-chunk sweep improved, with nine encoded packets unchanged. Feature-stage generation and client-visible speed remain open.
- Corrected the read-only region oracle's chunk-length handling for unpadded Java 26.3 save files and recorded a seed-2027 comparison sample.
- Prefer the forward edge within square chunk shells during movement and let stationary players use the configured worker pool for outer cold chunks, while keeping the same bounded maximum.
- Use `rustc-hash` for bounded coordinate caches in the Overworld generator. A ten-run, cold four-chunk seed-2027 sample improved from a 1.134 s median to 1.062 s; packet bytes matched the prior build for 49 seed-2027 and nine seed-2026 chunks. Client loading speed remains unmeasured.
- Fill the three-chunk square around the player before sending farther chunks in the Java preview, preventing early far results from appearing beside nearby gaps. Cold chunks can still take time to generate.
- Added the placement stage's ground-cover runtime for the operator-provisioned 26.3 pack: the `heightmap`, `offset`, and `block_predicate_filter` decorations and the `simple_block` configured type with its constant and weighted providers, modelable in 46 of the pack's 64 ground-cover chains. Its measured effect was first recorded as zero to 77 of the save's 2,543 grass blocks and 1 of its 95 flowers over a fully generated 65,536-column square at +0.36% cold terrain cost; both figures are superseded by the next entry, which found the placement gate could not fire outside carved air. Tree canopy and structures remain absent and no per-position vegetation parity is claimed.
- Named open sky as the air it is for the decoration pass: a row the fill pass leaves unnamed now answers `minecraft:air` to `block_at`, which is what `is_air` had always answered, so the pack's `matching_block_tag minecraft:air` filter fires on the surface instead of only inside caves. 82 of the pack's 84 `matching_block_tag` documents test air. Ground cover over a fully generated 65,536-column square moves from 77 to 5,885 grasses against the save's 2,543 and from 1 to 222 flowers against its 95, and a 1,000-block control square shows the same shape (4,101 against 788), so the placement now over-counts by 2.1×–5.2× and that over-count is published unexplained rather than claimed as parity; two production tests pin the fixed behavior and the T4 vegetation gate stays unmet. Cold four-chunk cost is unchanged within its measured band (medians 1,403.86 against 1,391.08 ms/chunk over ten alternating runs per build) with 797 more written rows, and the modifier gap report grows from 18 to 24 ground-cover keys by naming the int providers `minecraft:count` and `minecraft:height_range` were silent about.

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
