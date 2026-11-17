# Entity tracking limits

Separates RustMC's policy values in `crates/rustmc-server/src/world/entity_tracker.rs` from facts about Java 26.3. Checked 7 November 2026. Public statements below come from the community Minecraft Wiki, a secondary source not pinned to 26.3. No value has been observed against a real client or official server.

## RustMC policy

These are chosen by RustMC to bound work and are not vanilla values. Changing them changes no compatibility claim.

| Constant | Value | Purpose | Relation to vanilla |
| --- | --- | --- | --- |
| `EntityType::tracking_range` | Player 64, Monster 48, Animal 32, Item 16, Projectile 32 blocks | Bounds who receives an entity | Unverified. No public per-type table was found. [server.properties](https://minecraft.wiki/w/Server.properties) describes only `entity-broadcast-range-percentage` (default 100), which RustMC does not model |
| `MAX_TRACKING_RANGE` | 64 blocks | Bounds the chunk query radius; must equal the widest type, which a test enforces | Derived from the row above |
| Spherical range test | Euclidean distance in 3D | Simple deterministic visibility rule | Unverified |
| `MAX_VERTICAL_COORDINATE` | ±2048 | Rejects absurd input before indexing | Stricter than vanilla: [World boundary](https://minecraft.wiki/w/World_boundary) says build limits do not apply to entities. A legitimate entity beyond this is refused |
| `MAX_HORIZONTAL_COORDINATE` | ±30,000,000 | Keeps chunk coordinates inside `i32` | Rounded from the fact below |

## Facts these rest on

| Fact | Source | Status |
| --- | --- | --- |
| Chunks are 16 blocks wide on X and Z | Already relied on by the protocol paths in this repository | Established by the existing chunk work |
| Horizontal teleport limit near ±30,000,000; world border at ±29,999,984 | [World boundary](https://minecraft.wiki/w/World_boundary) | Secondary only |

## Movement cost

`move_entity` and `remove_entity` find existing observers through a reverse index rather than scanning every observer. Cost still grows with the number of entities near the position and the number of observers of the entity, so a dense crowd remains expensive. That case has not been measured.

`movement_cost_report` in `tests_entity_tracker.rs` moves one mob that no observer can see, with distant observers on a 256-block grid. Release build, 200,000 moves per row, Rust 1.98.1, Intel i7-13650HX, on a shared machine with other builds running, so treat these as indicative only:

| Distant observers | Nanoseconds per move |
| ---: | ---: |
| 100 | 337 |
| 1,000 | 390 |
| 10,000 | 454 |
| 100,000 | 514 |

A 1,000-fold population increase costs about 1.5 times as much per move, consistent with the logarithmic map lookups rather than a scan. This measures the spread-out case only and is not a player capacity claim.
