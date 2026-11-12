# M6 — Pulse–Parcel Deterministic Replay & Partition Equivalence Evaluation

Status: **Evaluation slice implemented**. Under [ADR-0005](../decisions/ADR-0005.md) and [ARCHITECTURE](../ARCHITECTURE.md), parallel partitioned simulation must be evaluated strictly against a single-authoritative-writer reference executor. The initial slice in `crates/rustmc-server/src/pulse_parcel.rs` proves bit-for-bit replay equivalence across spatial partitioning, dynamic repartitioning, player migration epochs, cross-parcel boundary envelopes, and competing same-pulse edits.

## 1. Scope & Core Types

The evaluation models discrete logical ticks (**Pulses**) and spatial ownership units (**Parcels**) grouping chunks under mutable owners:

- `PulseId`: Numbered logical tick identifier (`PulseId(u64)`).
- `CellCoord`: 3D integer world coordinate `(x, y, z)` in Euclidean chunk space.
- `ParcelId`: Spatial partition owner identifier (`ParcelId(u32)`).
- `PartitionLayout`: Deterministic mapping from chunk `(i32, i32)` to `ParcelId` using `BTreeMap`.
- `PlayerId`: Deterministic player session identifier (`PlayerId(u64)`).
- `PlayerState`: Authoritative position (`x, y, z`), rotation, `epoch`, `last_sequence`, and `parcel_id` using float bit representations for total ordering.
- `IntentKind`: Decoded gameplay intents (`Move`, `BreakBlock`, `PlaceBlock`).
- `AdmittedIntent`: Intent admitted at the session gate with a canonical global `admission_order`.
- `PulseInput`: Admitted intent batch and optional dynamic `PartitionLayout` for the pulse.
- `RejectionReason`: Explicit failure causes (`DuplicateSequence`, `PlayerNotFound`, `OutOfBounds`, `UnloadedChunk`, `OutOfReach`, `BodyBlocked`, `UnsupportedFoot`, `StaleClaim`, `WouldEmbed`).
- `PulseEvent`: Authoritative pulse event log (`PartitionApplied`, `IntentCommitted`, `IntentRejected`, `PlayerMigrated`, `PulseCompleted`).
- `CommittedEffect`: Immutable effect record (`PlayerMoved`, `BlockChange`).

Data structures strictly use `BTreeMap` and `BTreeSet` (zero `HashMap` or `HashSet`), ensuring deterministic iteration and replay independence from allocator layout.

## 2. Executors

### ReferenceExecutor
The reference executor maintains a single flat authoritative state (`BTreeMap<(i32, i32), Chunk>` and `BTreeMap<PlayerId, PlayerState>`). It applies admitted intents sequentially in canonical `admission_order`, serving as the deterministic oracle.

### ParcelExecutor
The parallel executor partitions chunks and player states across discrete `Parcel` instances. It supports:
- **Independent Parcel Stepping**: Intra-parcel actions execute within the parcel owner without synchronization overhead.
- **Deterministic Boundary Coordinator**: Resolves cross-parcel actions, checks envelope intersections across parcel borders (`WouldEmbed`), and resolves competing same-pulse edits in canonical `admission_order` (`StaleClaim`).
- **Committed Player Migration Transfer Point**: Transmits player ownership between parcels during cross-border movement, incrementing `epoch` and maintaining `PlayerId` so subsequent actions within the same pulse execute in the destination parcel.
- **Dynamic Repartitioning**: Chunks and resident players are migrated across parcels between pulses, preserving all states and ensuring sequence idempotence against retransmissions.

## 3. Equivalence Verification

The `EquivalenceReport` comparator verifies bit-for-bit equivalence between `ReferenceExecutor` and `ParcelExecutor` runs:
1. **Event Streams**: Exact equality of emitted `PulseEvent` logs.
2. **Player States**: Exact equality of player coordinates, migration epochs, sequence counters, and parcel assignments.
3. **Chunk Block Arrays**: Bit-for-bit equality of raw block arrays across all chunks.

## 4. Coordinate Bounds

Invalid coordinates are refused before they reach replay state, by the same checks in both executors:

- `is_valid_player_position` gates every `Move` and, with finite rotation, every `insert_player`. It refuses NaN, infinities, `|x|` or `|z|` above `MAX_HORIZONTAL_COORDINATE`, and envelopes outside the world height. Every float-to-integer cast then operates on a validated position.
- `CellCoord::is_addressable` gates block intents, so a distant target is `OutOfBounds` rather than `OutOfReach`.
- `CellCoord::chunk_coord` returns `None` beyond `MAX_CELL_COORDINATE`. It never clamps, because clamping would map distant cells onto one chunk.
- `crossed_cells` returns nothing for non-finite, over-wide, or out-of-range intervals, which bounds its allocation, and `validate_body_clear` treats that as `OutOfBounds` rather than a clear body.

These limits are evaluation policy, not vanilla claims.

## 5. Test Suite

Unit tests in `crates/rustmc-server/src/pulse_parcel.rs` pin the evaluation criteria, including:
- `single_parcel_and_multi_parcel_layouts_match_reference_bit_for_bit`: 4-chunk 2x2 grid executed under single-parcel and 4-parcel layouts compared to the reference oracle.
- `player_migration_across_chunk_and_parcel_boundary_preserves_id_epoch_and_same_pulse_actions`: Migration across parcel borders advancing epoch and enabling same-pulse actions in the destination parcel.
- `cross_parcel_border_envelope_and_would_embed_conflict_resolve_in_canonical_admission_order`: Envelope crossing border chunks rejecting placement with `WouldEmbed` and resolving ordering conflicts canonically.
- `same_pulse_competing_cross_parcel_block_edits_preserve_stale_claim_rejection`: Competing block breaks from different parcels resolving in canonical admission order with `StaleClaim` rejection.
- `dynamic_repartition_across_pulses_and_duplicate_sequence_idempotence`: Dynamic layout changes moving chunks and players between pulses, verifying duplicate sequence idempotence.
- `invalid_moves_leave_partitioned_replay_equivalent_through_a_repartition`: NaN, infinite, and overflowing moves rejected identically under a two-parcel layout, with replay still equivalent after a later repartition.
- `extreme_block_targets_cannot_alias_a_loaded_chunk`: Block intents at `i64` extremes rejected identically without touching a chunk that clamping would have selected.
- `players_with_unreplayable_state_are_refused_by_both_executors`: Invalid positions and rotations refused at insertion.
