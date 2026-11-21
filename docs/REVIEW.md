# Codebase review (Phase 0, 21 November 2026)

Purpose: map every module of the workspace, state whether a live server run
reaches it, and record what Phase 0 does with it, so that later phases start
from code that is either wired into play or deliberately kept for a named
phase. Deleted subsystems can be restored from the commit recorded next to
them (`git show <hash>^:<path>` or `git revert <hash>`).

## Method

- **Live play** is everything the `rustmc-server` binary can reach from
  `main` through `runtime::run_listener`. It was measured with the compiler,
  not by reading: a scratch build made every library module private except
  the binary's entry points, so rustc's `dead_code` lint reported each item no
  live path reaches. A module whose items are (almost) all reported is
  unreachable; the remainder were confirmed by tracing `use crate::…` edges
  from `runtime.rs`.
- Line counts include each file's unit tests.
- Actions: **keep** (live), **keep for phase N** (unreachable today, needed by
  the named Phase 1 track), **isolate** (the opt-in vanilla generator and its
  tools), **delete** (unreachable and not needed by Phases 1-2).

## What live play reaches today

`main` → `runtime` (TCP/UDP supervisor) → `discovery_java::Session` (status,
offline login, configuration, play: chat, commands, creative inventory,
movement) → `java_preview` (chunk streaming from `world::Generator` or the
opt-in `vanilla` generator, through `chunk_adapter` and `preview_cache`), plus
`discovery_bedrock` (Bedrock LAN ping/pong). Nothing persists, there is no
shared world, and other players are not visible.

## Server crate (`crates/rustmc-server/src`)

| Module | Lines | Live | Action |
| --- | ---: | --- | --- |
| `main.rs` | 111 | yes | keep |
| `lib.rs` (config parsing) | 698 | yes | keep; config grows in Phase 1 E |
| `runtime.rs` | 926 | yes | keep; becomes the server loop in Phase 1 A |
| `discovery_java.rs` | 3,045 | yes | keep; split (over 1,000 lines) |
| `discovery_java/keepalive.rs` | 239 | mostly no | keep for phase 1 A (keep-alive) |
| `java_wire.rs` | 1,125 | yes | keep; split |
| `java_packets.rs` | 117 | yes | keep |
| `java_preview.rs` | 1,701 | yes | keep; split |
| `preview_cache.rs` | 619 | yes | keep (opt-in) |
| `preview_data.rs` | 190 | yes | keep |
| `chunk_adapter/` | 1,629 + tests | yes | keep |
| `commands/` | 1,568 + tests | yes | keep for phase 1 E; split |
| `protocol/chat.rs` | 1,591 | yes | keep; split |
| `protocol/ids.rs` | 870 | yes | keep |
| `protocol/slot.rs` | 1,920 | yes | keep; split |
| `protocol/movement.rs` | 2,050 | ~5 % | keep for phase 1 D; split |
| `protocol/player_info.rs` | 2,413 | no | keep for phase 1 D (player list); split |
| `protocol/entity_spawn.rs` | 1,875 | no | keep for phase 1 D (other players); split |
| `item.rs` | 3,405 | partly (slot codec) | keep for phase 1 D (inventory, give); split |
| `world.rs` (default terrain) | 1,253 | yes | keep for phase 1 B; split |
| `world/inventory.rs` | 461 | partly | keep for phase 1 D |
| `world/container.rs` | 459 | no | keep for phase 1 D (window transactions) |
| `world/interaction.rs` | 640 | no | keep for phase 1 D (break/place validation) |
| `world/events.rs` | 1,806 | no | keep for phase 1 D; split |
| `world/journal.rs` | 778 | no | keep for phase 1 C (crash-safe journal) |
| `world/physics.rs` | 931 | no | keep for phase 1 D (collision) |
| `world/entity_tracker.rs` | 551 | no | keep for phase 1 D (visibility) |
| `world/time_weather.rs` | 305 | no | keep for phase 1 E (`time`) |
| `world/block_entity.rs` | 1,702 | no | keep (slot codec depends on it); split |
| `world/enchantment.rs` | 940 | partly (item components) | keep (item components) |
| `entity/item.rs` | 1,106 | no | keep for phase 1 D (drop, pickup) |
| `entity/ai.rs` | 1,813 | no | **delete** (mob AI) |
| `world/loot.rs` | 11,585 | no | **delete** |
| `world/recipe_data.rs` | 4,036 | no | **delete** |
| `world/crafting.rs` | 1,837 | no | **delete** (survival) |
| `world/furnace.rs` | 1,967 | no | **delete** (survival) |
| `world/brewing.rs` | 2,285 | no | **delete** |
| `world/anvil.rs` | 2,758 | no | **delete** |
| `world/enchanting.rs` | 1,026 | no | **delete** |
| `world/grindstone.rs` | 1,196 | no | **delete** |
| `world/container_data.rs` | 285 | no | **delete** (only furnace/brewing used it) |
| `world/experience.rs` | 228 | no | **delete** (survival) |
| `world/fluid.rs` | 709 | no | **delete** |
| `world/projectile.rs` | 1,226 | no | **delete** |
| `world/explosion.rs` | 879 | no | **delete** |
| `world/vanilla_rules.rs` | 859 | no | **delete** (bundles the deleted rule data) |
| `bedrock/` | 2,646 | re-exports only | **delete** (Bedrock sessions) |
| `bedrock_chunk.rs` | 1,801 | no | **delete** |
| `bedrock_framing.rs` | 740 | no | **delete** |
| `discovery_bedrock.rs` | 462 | yes | keep (LAN ping); owner decision below |
| `vanilla/` (13 files) | 24,780 | opt-in | **isolate**: opt-in only, split files |

## Tools crate (`crates/rustmc-tools`)

| Tool | Action |
| --- | --- |
| `vanilla_oracle`, `oracle.rs`, `region.rs`, `nbt.rs`, `save.rs`, `inspect_vanilla_save` | isolate (vanilla generator measurement) |
| `bench_vanilla_chunks`, `prepare_preview_cache`, `prepare_chunk_registry`, `prepare_preview_registry` | keep |
| `measure_startup`, `check_dependency_licenses` | keep |
| `check_vanilla_data` | **delete** (checks the deleted rule data) |

## Deletions

41,074 lines removed in total. To restore a subsystem, revert its commit
(`git revert <commit>`) or read a file back with `git show <commit>^:<path>`.

| Subsystem | Files | Commit |
| --- | --- | --- |
| Vanilla rule bundle and `check_vanilla_data` tool | `world/vanilla_rules.rs`, tool and its test | `4bae815` |
| Loot tables (block, entity, chest, gameplay, archaeology) | `world/loot.rs` | `c3f1e62` |
| Anvil, grindstone, enchanting table | `world/anvil.rs`, `world/grindstone.rs`, `world/enchanting.rs` | `cacf3fb` |
| Crafting, recipe data, furnace, brewing, container data | `world/crafting.rs`, `world/recipe_data.rs`, `world/furnace.rs`, `world/brewing.rs`, `world/container_data.rs` | `4172655` |
| Experience | `world/experience.rs` and tests | `13d8d13` |
| Fluids | `world/fluid.rs` and tests | `79848c1` |
| Projectiles | `world/projectile.rs` | `23bb1dd` |
| Explosions | `world/explosion.rs` | `de0f525` |
| Mob AI | `entity/ai.rs` | `12e397d` |
| Bedrock session, chunk and batch codecs | `bedrock/`, `bedrock_chunk.rs`, `bedrock_framing.rs` | `53b3438` |
| Item default components (tool, food, consumable rules) | part of `item.rs` | `425e2cc` |

## Owner decisions

- `discovery_bedrock` answers Bedrock LAN pings today. The plan defers
  Bedrock; this review keeps the live ping (small, tested) and deletes the
  unreachable Bedrock session code. Removing the ping too is one more commit.

## Step 3: structure, data and panics

- **File size.** Every Rust file is now at most 1,000 lines. Large modules
  were split along their existing sections as pure moves (moved private items
  became `pub(super)`; public paths are unchanged), and inline unit tests moved
  to sibling `tests.rs` files. The vanilla generator split was checked for
  byte identity: 81 chunk packets (seed 2026, radius 4) from the build before
  and after are identical, and all 27 ignored data-root tests pass.
- **Panics.** Production `unwrap`/`expect`/`assert!` were removed from chunk
  caching, journal integer reads, bounded random draws and the registry tool.
  Remaining `debug_assert!`s state internal invariants and compile out of
  release builds; `build.rs` panics are the build-script error channel.
- **Hard-coded limits.** Listener bounds are named range constants in
  `lib.rs`, and `ListenerConfig::validate` replaces the runtime's duplicate
  literal checks.
- **Headers.** Every source file starts with a `//!` header.

## Known items left for Phase 1

These are not fixed in Phase 0 because the Phase 1 track that owns them
replaces the code:

| Item | Where | Owner track |
| --- | --- | --- |
| 25 protocol block-state IDs embedded for the synthetic terrain | `java_preview.rs` `BLOCK_STATES` | B (world) needs the full 26.3 block registry from the provisioned reports |
| Wire helpers duplicated per packet module (VarInt, strings, framing) | `protocol/movement.rs`, `protocol/player_info/wire.rs`, `protocol/entity_spawn.rs`, `protocol/chat.rs`, `world/block_entity/nbt.rs` | A (network) moves them onto `java_wire` |
| Vanilla plant and solidity block lists in source | `vanilla/plant_support.rs`, `vanilla/feature/state.rs` | vanilla-accurate terrain phase (opt-in, isolated) |
| File headers state purpose; "done / not done" notes are added as each Phase 1 track rewrites a module | all | each track |
