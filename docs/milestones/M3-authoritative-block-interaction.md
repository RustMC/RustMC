# M3 follow-on slice — authoritative block interaction (design)

Status: **design only, not implemented.** The owner requested a vanilla-matching
Overworld with standing, breaking, and placement on 2 October 2026. Live
Overworld chunk delivery, an authoritative Y-coordinate model, and durable
world state remain prerequisites; this document does not claim those are met.
This is the design
gate for the first gameplay slice *after* the [local Java 26.3 preview
slice](M3-java-local-preview.md). That preview is a read-only,
single-observer Creative inspection of RustMC-generated chunks whose surface
rules come from [ADR-0013](../decisions/ADR-0013.md); the preview slice itself
records that "Full play behavior remains unverified" and that the world is
read-only, so no preview behavior is treated here as an authoritative rule.
[ROADMAP](../../ROADMAP.md) places block interaction, save ordering, recovery,
and multiplayer synchronization under **M4 — Persistent basics**; this file is
named for the milestone it follows, and as a design document it closes no M3 or
M4 gate.
Nothing here may be claimed as vanilla 26.3 behavior until the OBSERVE tasks
below return measurements recorded in `docs/research/` and
[PROVENANCE](../PROVENANCE.md).

## 1. Scope

In scope for the implementation slice:

1. One authoritative, single-writer path that turns a validated client block
   action into a world mutation, one broadcast set, and one durable record.
2. Server-side validation that never trusts client-supplied state.
3. Collision/standing integrity for the actions this slice supports.
4. Client synchronization of the resulting block state to every client that can
   see the target cell, plus a corrective path for rejected actions.
5. Save, restart, and rejoin recovery for covered actions.

Out of scope: inventory, item use, block hardness/break timing beyond instant
Creative removal, drops and loot, redstone, scheduled/random ticks, fluid
simulation, block entities, structures, entity movement and AI, authenticated
identity, Bedrock, multiplayer fairness studies, and any performance claim. Each
excluded area stays unchecked in the [README
checklist](../../README.md#development-checklist).

## 2. Requirements this slice must satisfy

From [ARCHITECTURE](../ARCHITECTURE.md) and the milestone gates, restated as
constraints on this design rather than new goals:

- R1 Gateways never mutate authoritative state. The Java session decodes a
  packet into an *intent*; only the gameplay core mutates.
- R2 Single authoritative writer. Every accepted action is applied in one
  numbered logical tick, in admission order. Wall-clock and packet arrival order
  outside the queue cannot decide outcomes.
- R3 Bounded admission. Per-session and global pending-action caps, per-tick
  work budget, and an explicit overload response. No unbounded queue.
- R4 Explicit failures. Rejected intents are named, counted, and answered; they
  never leave a partially mutated cell.
- R5 Persistence cannot acknowledge durable state before its durability
  condition holds. A committed mutation is either durably recorded or reported
  as not durable; it is never silently dropped.
- R6 Readiness stays honest. `world_ready` remains false until spawn, load, and
  recovery paths exist; a successful block change does not make the server
  playable.
- R7 Loopback-only development identity remains the only session type, so this
  slice is a *protocol and rules* milestone, not a security boundary. Owner
  authorization is required before any non-loopback exposure.
- R8 Provenance: behavior is defined by RustMC requirements plus official
  documentation and black-box observation. See section 9.

## 3. Terms

- **Cell**: one addressable world block position `(x, y, z)` in a loaded chunk,
  plus its block-state identity.
- **Action intent**: a decoded request from a session — kind (place or remove),
  target cell, the cell's face/adjacency used for placement, claimed previous
  state, session sequence number.
- **Effect**: the committed, immutable result of one accepted intent: the new
  cell state, affected neighbor cells, and the target-cell chunk coordinate.
- **Rejection**: a named decision that no state changed, with the corrective
  response the actor receives.
- **Interest set**: the chunk coordinates a session is currently being sent.
  The preview keeps exactly this set privately as `sent` in
  `crates/rustmc-server/src/java_preview.rs`.

## 4. Ordered block-update pipeline

Stages run in this order inside one tick. A later stage may not read state a
previous stage has not committed. Each stage is separately testable and emits
its own counter.

| # | Stage | Input | Output | Failure exit |
| --- | --- | --- | --- | --- |
| S1 | Decode | bytes in play state | typed action intent | malformed packet: close or drop per the M2/M1 admission rules |
| S2 | Session/state gate | intent + session | admitted intent with sequence | wrong protocol state, no loaded terrain, no teleport acknowledgement |
| S3 | Addressability | target cell | owning chunk + local cell, or reject | unloaded or unowned chunk, Y outside the dimension range |
| S4 | Authority | intent + session policy | permitted or denied | game mode without build permission, protected region |
| S5 | Reach and view | player position/eye, target cell | permitted or denied | target outside the documented interaction range, target outside the session interest set |
| S6 | World precondition | current cell state | consistent or reject | claimed previous state differs, unknown block identity, result would embed an entity envelope, fluid rule required |
| S7 | Mutation | accepted intent | new cell state plus affected neighbors, marked dirty | none: reaching S7 means the mutation is defined; a storage-side failure is reported at S9 |
| S8 | Broadcast | effects | packets to each interested session | session gone or interest lost: skip that session, never abort the effect |
| S9 | Durability | dirty cells | write-ahead record, then acknowledgement of durability | durability failure recorded against the effect; the session answer is the broadcast, not disk |

Ordering rules:

- O1 S7 through S9 run for the whole admitted batch of a tick; the tick's
  effects are broadcast in mutation order with the tick number.
- O2 A rejection at S2 to S6 changes nothing and produces exactly one
  correction (section 6), so a client cannot leave a ghost block predicted from
  an action the server refused.
- O3 Idempotence: a duplicate intent — same session sequence and same target
  cell already in the requested state — is a no-op that is answered, not
  reapplied. Client retransmission and double packet reads must not double-drop
  or double-place.
- O4 Cross-chunk effects (a neighbor cell in another chunk) require both chunks
  to be loaded and addressable; otherwise S3 rejects the whole intent, never
  half of it.
- O5 A block-change packet must never precede the chunk-data packet that
  creates its target cell for the same session. Per-session output is ordered by
  (chunk admission, effect).

## 5. Validation rules (authoritative)

- V1 A client may name which cell it wants to touch, and that target cell comes
  from client coordinates. Nothing else is taken from the client: the resulting
  block identity is computed from server-side rules, and the client's claimed
  previous state is only a consistency check (S6), never a source of truth.
- V2 Range and shape limits reuse the existing bounded-session discipline:
  fixed packet size caps, fixed per-tick action budget, fixed pending-action cap
  per session. Default proposals for the first slice: 8 pending actions per
  session, 32 actions applied per tick, 4 chunks touched per tick per session.
  These are RustMC limits, not vanilla facts, and need owner review.
- V3 Interaction distance is measured server-side from the session's
  authoritative position and eye position, to the target cell's block volume.
  The documented player-facing values are 4.5 blocks in normal modes and 5
  blocks in Creative ([Interaction range](https://minecraft.wiki/w/Interaction_range),
  checked 2 October 2026), controlled by the
  `minecraft:block_interaction_range` player attribute
  ([Attribute](https://minecraft.wiki/w/Attribute)). RustMC must confirm the
  26.3 defaults by OBSERVE-1 before asserting them in a test, and must decide
  whether an out-of-range action is silently ignored or corrected, per
  OBSERVE-2.
- V4 A target outside the session's interest set is rejected: a client cannot
  change a chunk it has not been sent. This also bounds the work one session can
  trigger.
- V5 Position encoding is the protocol `Position`/VarPos form (see
  [Java Edition protocol/Packets](https://minecraft.wiki/w/Java_Edition_protocol/Packets));
  RustMC must map it onto world coordinates with the same euclidean chunk/local
  split the region layout uses, including negative quadrants. This is the
  mapping pinned by
  `envelope_footprint_crosses_chunk_borders_and_rejects_unaddressable_targets`
  and `smoke_operator_save_layout_matches_the_chunk_coordinate_mapping`.
- V6 Y-space is an explicit, single definition. The preview `Chunk` array is
  indexed `0..WORLD_HEIGHT` (128 cells) while the wire adapter writes sections
  from `MIN_Y = -64`, so an array index and a world Y are **not** the same
  number today. The gameplay core must introduce one authoritative world-Y
  range, taken from the negotiated 26.3 dimension type rather than a constant,
  and the chunk adapter must translate at its edge. Owner decision D1 below.
- V7 Protected spawn: RustMC policy, not a vanilla mechanic. The first slice
  keeps a configurable protected box around the session spawn point in which
  block actions are denied (S4) and logged. Radius default `0` (disabled) for
  the loopback development profile so behavior stays opt-in.
- V8 Fluid interaction is not simulated in this slice. An action whose declared
  result requires fluid rules (placing into a cell the state model calls fluid,
  or removing a cell whose documented behavior spawns flow) is rejected at S6
  with `fluid-rule-required`, and the fluid state is never partially written.
  Water and lava behavior stays under M5 in the roadmap.

## 6. Collision, standing, and correction

- C1 Envelope. The player hitbox is documented as 0.6 blocks wide and 1.8 blocks
  tall, with eye height 1.62 ([Player](https://minecraft.wiki/w/Player)), and
  1.5 tall while sneaking ([Sneaking](https://minecraft.wiki/w/Sneaking)).
  RustMC uses those figures as its proposed envelope and must re-verify them for
  26.3 via OBSERVE-3 before any claim.
- C2 First approximation is whole-cube: a cell is body-blocking or supporting
  based on a block-property table, not a per-block shape. In RustMC's preview
  model every block is one addressable cube cell, so a whole-cube column walk is
  exact for that model; leaves are the visible place where that model can
  diverge from vanilla, which is exactly what OBSERVE-4 must settle.
  Until it returns, no test may assert shape-level behavior (stairs, slabs,
  fences, partial heights).
- C3 Standing invariant. For any cell the server reports as the player's feet
  position, every cell the body span crosses must be non-blocking and at least
  one cell under the footprint must be blocking. Under the 1.8 height a
  whole-block feet position crosses exactly two body cells; a sub-block feet
  position can cross three, which the implementation must handle. The new probes
  in `crates/rustmc-server/src/world/tests_interaction.rs` pin the invariant
  against sampled preview columns, including a bare-ground case where the
  resting level equals the generator's surface height plus one, a canopy case
  where it is lifted, and a chunk-border footprint that spans two chunks.
- C4 No embedding. A placement must not occupy a cell that intersects a player
  envelope (or, later, any entity). The check runs at S6 against the
  authoritative positions, and the action is rejected with `would-embed` rather
  than pushed through with a client-side suffocation state.
- C5 Support removal. Removing the cell a player stands on is **allowed** (this
  is what digging means); the server does not refuse it, and the player's next
  movement update resolves the fall. A later slice must decide whether movement
  validation accepts gravity, which is why movement rules are out of scope here.
- C6 Correction path. On any rejection the server sends the authoritative state
  of the target cell (and of every cell in the rejected effect set) back to the
  acting session only. There is no acknowledgement field in the block-change
  direction, so the correction is a state restatement, and it must be idempotent
  when the client already shows that state.

## 7. Client synchronization

- P1 One cell change is expressed with the clientbound block-change packet: a
  block position plus a block state id from the global block-state palette
  ([Java Edition protocol/Packets](https://minecraft.wiki/w/Java_Edition_protocol/Packets),
  section "Block Update"). RustMC must bind its 26.3 packet id from the
  operator's own `reports/packets.json`, exactly as the preview slice bound its
  login/chunk/teleport ids, and must never invent one.
- P2 Several cells in the same tick use the batched block-update form (the
  versioned "Block Updates" packet carrying an array of position/state actions),
  which is the shape that also carries block-breaking *status* and particle
  values. The batch size cap and the split rule are BIND-2.
- P3 Breaking animation is not this slice. Instant Creative removal is a single
  state change. The destruction-progress status values are needed only with
  hardness and are deferred with the item/inventory work.
- P4 Block state ids need a registry. The preview encodes 19 hard-coded state
  ids. Authoritative placement needs a versioned cell-state table mapping
  RustMC block/state names to 26.3 palette ids, prepared offline from the
  operator's own registry/block reports, kept out of git, and covered by a
  provenance entry (BIND-1). Until BIND-1 lands, only the 19 known states are
  placeable, and every other identity is rejected at S6 as `unknown-block`.
- P5 Recipients: each session whose interest set contains the target chunk,
  including the actor. Sessions that lose interest mid-tick get the state through
  the next chunk-data send, which must therefore reflect committed mutations —
  the mutation set must live in the authoritative chunk, not only in the
  encoder path.
- P6 Heightmaps and lighting must be updated for every mutation, because the
  preview encoder derives both from column content at send time. The first slice
  must at least recompute the three heightmap kinds already encoded
  (`WORLD_SURFACE`, `MOTION_BLOCKING`, `MOTION_BLOCKING_NO_LEAVES`) for the
  touched column and record a bounded re-light task; full light propagation is
  out of scope, and the current vertical-only skylight model is a preview
  simplification, not a rule.
- P7 Ordering and visibility: two clients are required to prove broadcast. The
  preview currently supports one isolated observer per connection and no shared
  player entities, so shared-session plumbing is a prerequisite task, not an
  acceptance shortcut.

## 8. Persistence and recovery

- Q1 Model: authoritative cells are mutated in memory, tagged dirty per chunk,
  then written by a bounded, ordered save worker. A chunk is never serialized
  from a half-applied tick; the tick's effect set is the atomic unit.
- Q2 Write-ahead ordering: a durable record for a tick is appended before that
  tick's effects are counted as durable. Clean shutdown flushes all dirty
  chunks in chunk-coordinate order and logs the count; a crash may lose the
  unsaved tail, which the recovery test must quantify rather than hide.
- Q3 Recovery target: after restart or rejoin, every action accepted before the
  interruption is visible, and no action rejected or never sent appears. The
  observable statement is per action: exactly once, or not at all with a logged
  reason.
- Q4 RustMC's own save format is a RustMC decision. Vanilla 26.3 file
  compatibility is **not** claimed. The only facts this slice pins about the
  operator's own save are the publicly documented layout ones: 26.3 stores
  overworld chunks under `dimensions/minecraft/overworld/region/r.<x>.<z>.mca`,
  a region is two 4 KiB tables plus sector-aligned chunk streams, and chunk
  coordinates map to region and location-table slot by euclidean division —
  including negative quadrants, where truncating division names the wrong file.
  [Region file format](https://minecraft.wiki/w/Region_file_format) is the
  format reference; the ignored smoke
  `smoke_operator_save_layout_matches_the_chunk_coordinate_mapping` verifies
  these invariants against the operator's own save, reading only each selected
  region's 4 KiB location table plus a 5-byte chunk-stream header. Save/import/
  export format remains an owner decision (D3).
- Q5 Bounded I/O: dirty-chunk queue depth, per-tick write budget, and back
  pressure are finite and tested. A full queue pauses admission of new actions
  (R3) instead of dropping records, per the architecture rule that required
  durable records are never silently dropped.

## 9. Provenance discipline (continues ADR-0014)

- No mechanic, table, or code is copied or translated from another server,
  and no Mojang code or asset enters git. Knowledge
  consultation, if used, is logged in [PROVENANCE](../PROVENANCE.md) with what
  was read, what was learned, and how the result was independently written.
- Compatibility facts enter the repository in three forms only: a citation to
  official or publicly documented sources, a recorded observation with method
  and date in `docs/research/`, or a RustMC-authored constant declared as a
  design choice with owner approval. Numbers that only make sense as generated
  values (packet ids, block-state palette ids) are bound from the operator's own
  locally generated reports and never committed.
- This slice's terrain input remains the ADR-0013 preview generator until a
  vanilla generation stage passes its own gate; gameplay rules must not be
  presented as vanilla parity in either direction.

## 10. Task list

Bind/derive tasks (no code claim until done):

- BIND-1 Build the versioned cell-state table (name plus state properties to 26.3
  global block-state palette id) from the operator's locally generated 26.3
  reports; record the report path and generation command in PROVENANCE; keep the
  data untracked.
  Landed so far: `chunk_adapter::registry::RegistryTables` is the provisioning seam
  for this shape (version-validated `26.3`/`777` name→id maps, canonical state-key
  normalization, unknown identity as a typed error, direct-mode widths derived from the
  declared registry sizes). It covers the states the terrain generator emits, is fed by
  an operator-provisioned file that stays untracked, and its provenance is Session 11.
  It does **not** yet carry the interactive cell states this slice needs, and no entry in
  it has been confirmed by a client, so BIND-1 is extended rather than closed.
- BIND-2 Bind the 26.3 ids and field orders for the clientbound block-change and
  batched block-update packets, the serverbound block-action packets, and the
  block-placement interaction packet from the same report; confirm each with a
  live client acceptance test.
- BIND-3 Bind the dimension range (`min_y`, height) from the negotiated 26.3
  dimension type in the registry manifest, replacing the preview's `MIN_Y` and
  `WORLD_HEIGHT` constants in the authoritative path (V6, D1).

Observe tasks (method must be written down before the number is used):

- OBSERVE-1 26.3 block interaction range: measure the maximum distance at which
  a real 26.3 client can place and break, in Creative and in non-Creative, from
  the owner's client against a vanilla server and against RustMC, recording
  F3 coordinates. Confirm whether the attribute defaults documented on the wiki
  still hold in 26.3.
- OBSERVE-2 Server response to an out-of-range or disallowed action in vanilla
  26.3: does the client receive a corrective block change, a different result,
  or silence? RustMC's rejection matrix must name its own choice either way.
- OBSERVE-3 Player envelope in 26.3: hitbox, eye height, sneaking height, and
  whether the reported values change the standable-position result on the same
  sampled columns.
- OBSERVE-4 Per-block collision behavior for leaves, stairs, slabs, and fences;
  which blocks are fluid-bearing. Until this returns, C2 stays whole-cube and
  V8 stays "reject".
- OBSERVE-5 Persistence timing in vanilla 26.3 (autosave cadence and what a hard
  kill loses) used only as a comparison point, never as a copied format.

Design decisions needing owner sign-off:

- D1 Authoritative world-Y range and the index-to-world mapping (V6).
- D2 Whether the first slice is Creative-instant only, or includes break timing.
- D3 Save format: RustMC-native journal first versus Anvil-compatible output;
  no compatibility claim until fixtures exist (Q4).
- D4 Spawn protection radius and defaults for the development profile (V7).
- D5 Whether an out-of-range action is corrected or silently dropped (OBSERVE-2).
- D6 Approval that shared-session plumbing may precede authentication, keeping
  the loopback-only rule (R7).

## 11. Acceptance criteria

Each item is either checked with named evidence or left unchecked; a green unit
test is not client evidence.

- [ ] A1 Single-writer pipeline: S1 to S9 exist as separate, testable stages with
  per-stage counters; no mutation path bypasses them; a duplicate intent is a
  no-op (O3).
- [ ] A2 A real Java 26.3 client on loopback removes a block it can see and the
  cell is gone in that client's view without rejoining, and it reappears after a
  rejected placement attempt (correction path C6 observed).
- [ ] A3 Two real 26.3 clients in the same world: a change by one is visible to
  the other within one bounded broadcast, and invisible to a client whose
  interest set excludes that chunk. Requires shared-session plumbing (P7).
- [ ] A4 Rejection matrix: every row of section 12 has a passing test that shows
  no state change, exactly one named log event, and the specified client answer.
- [ ] A5 Reach and view validation (V3, V4) measured with OBSERVE-1 numbers,
  including negative coordinates and the chunk-border footprint case.
- [ ] A6 Standing/collision integrity: C3 and C4 hold after mutations; the
  envelope probes are extended to mutated columns, no accepted action embeds a
  player, and the envelope constants used by the probes are the OBSERVE-3 values
  with the observation recorded in `docs/research/`.
- [ ] A7 Heightmaps and derived column state update on mutation (P6) and are
  consistent with the next chunk send for the same cell.
- [ ] A8 Persistence: an accepted action survives a clean shutdown, a SIGKILL
  with a bounded documented loss window, and a client disconnect/rejoin; the
  exactly-once statement in Q3 holds for the covered set.
- [ ] A9 Bounded resources: pending-action, per-tick, and save-queue caps hold
  under a flood test; overload produces the documented response with no
  unbounded allocation and no dropped durable record.
- [ ] A10 Gates: format, check, Clippy with warnings denied, tests, build,
  rustdoc, license policy, spellcheck, and GitHub CI pass; PROVENANCE,
  COMPATIBILITY, README checklist, and CHANGELOG are updated in the same change.
- [ ] A11 Documentation honesty: no vanilla-parity, player-capacity, or
  performance claim; the compatibility matrix records the tested scope and the
  remaining OBSERVE gaps.

## 12. Failure and rejection matrix

| Trigger | Detect | State effect | Answer to actor | Others | Event and metric |
| --- | --- | --- | --- | --- | --- |
| Malformed or oversized action packet | S1 | none | per M1/M2 admission rule, bounded | none | `action_malformed`, session byte counter |
| Wrong protocol state, terrain not loaded, teleport not acknowledged | S2 | none | corrective no-op after load | none | `action_wrong_state` |
| Target chunk unloaded or outside the interest set | S3/S4 | none | restatement of the cell the server actually has | none | `action_unloaded_target` |
| Y outside the dimension range | S3 | none | restatement | none | `action_out_of_range_y` |
| Target outside interaction distance | S5 (V3) | none | choice per D5/OBSERVE-2 | none | `action_out_of_reach` |
| No build permission for the session's mode | S4 | none | restatement | none | `action_denied_mode` |
| Protected spawn region | S4 (V7) | none | restatement | none | `action_denied_protection` |
| Block name or state not in the bound table | S6 (P4) | none | restatement of the unchanged cell | none | `action_unknown_block` |
| Claimed previous state differs from authoritative state | S6 (V1) | none | restatement of the true cell | none | `action_stale_claim` |
| Placement would intersect a player envelope | S6 (C4) | none | restatement; nothing placed | none | `action_would_embed` |
| Result requires fluid rules | S6 (V8) | none | restatement; no partial fluid write | none | `action_fluid_rule_required` |
| Duplicate intent, already applied | S6 (O3) | none | answered, not reapplied | none | `action_duplicate` |
| Pending-action or per-tick budget exhausted | S2 (R3) | none | explicit overload response | none | `action_queue_full`, depth gauge |
| Save queue full or write failed | S9 (Q5, R5) | in-memory state stands, durability not achieved | no client-visible change; documented | unchanged, flagged not-durable | `persistence_backpressure`, `persistence_failed` |
| Chunk half applied | impossible by Q1 | n/a | n/a | n/a | assertion failure in test builds |

## 13. Test seams added with this design

These are characterization probes only; they add no production code path, no
behavior, and no public API. They exist so the slice above starts from executable
facts about the terrain RustMC already generates.

| Probe | Existing seam exercised | Claim pinned |
| --- | --- | --- |
| `standing_envelope_agrees_with_the_surface_rule_on_bare_ground_and_lifts_on_canopy` | `Generator::new`, `Generator::height`, `Generator::generate`, `Chunk::block` | The proposed 0.6 x 1.8 whole-cube envelope lands exactly one block above the generator's surface on bare sampled columns, lands higher where canopy covers them, and is deterministic per column |
| `envelope_footprint_crosses_chunk_borders_and_rejects_unaddressable_targets` | same, plus `CHUNK_SIDE` and `WORLD_HEIGHT` | A footprint spanning local cells 15 and 16 reads two chunks; a local index of 16 inside one chunk, a Y outside `0..WORLD_HEIGHT`, and an unloaded chunk are all rejects rather than silent air |
| `sampled_preview_columns_are_solid_from_the_surface_down_to_bedrock` | same | Every sampled column is continuous from its surface down to the bedrock floor and capped by open sky, so a standable position is reachable and one broken cell cannot leave a floating column |
| `smoke_operator_save_layout_matches_the_chunk_coordinate_mapping` (ignored, `RUSTMC_VANILLA_SAVE`, read-only) | region layout of the operator's own 26.3 save | The 26.3 per-dimension region path, region plus location-slot mapping including negative quadrants, sector alignment, and stored chunk-stream header are as publicly documented |

Not asserted, on purpose: hardness and break timing, drops, tool suitability,
block-state transitions of neighbors (shape updates), fluid flow, block
properties per id (no block-property registry exists in the repository yet), the
26.3 numeric defaults for interaction range and hitbox, and any claim that RustMC
terrain equals vanilla terrain. The corresponding gaps are OBSERVE or BIND tasks
above, and the first implementation slice must add a real block-property and
block-state registry seam instead of the current hard-coded state list.
