# Java 26.3 fixed-seed comparison worksheet

Status: **reference observations pending** (29 September 2026). This is a
research procedure, not a vanilla-parity claim or a change to the M3/M5 gates.
The current RustMC generator is independent and intentionally differs from
vanilla. Start with observable surface results before proposing an algorithm.

## Reference setup

Use a clean, licensed **Minecraft Java 26.3** client with no world-generation
mods or data packs. Create a new Creative single-player world with the default
world preset and seed `2026`; enable commands for repeatable navigation.
Mojang documents [Java 26.3](https://www.minecraft.net/en-us/article/minecraft-java-edition-26-3),
[commands and their permission setting](https://www.minecraft.net/en-us/article/minecraft-commands),
and the [F3 debug options](https://feedback.minecraft.net/hc/en-us/articles/48913133328013-Minecraft-Java-Edition-26-3).

At each X/Z below, teleport high enough to see the location (for example,
`/tp @s 256 200 0`), descend to the ground, and record the F3 coordinates,
biome at the ground, top solid block, and any water above it. If the biome or
coordinates are hidden, use the 26.3 debug options to show them. Capture one
screenshot per point. The reference world must remain unchanged by RustMC.

## RustMC baseline

The values below come from RustMC's own `Generator::height` and
`Generator::biome` for seed 2026, using:

```sh
cargo run -p rustmc-server --example inspect_preview_seed --locked -- 2026 0 0 256 0 0 256 -256 0 0 -256 512 512
```

`preview_ground_y` is the generator's top terrain layer at X/Z, before trees;
it is not a client player Y, ocean surface, or vanilla heightmap. This tool
reads no Mojang world data and checks no assets into the repository.

| X | Z | RustMC preview ground Y | RustMC preview biome | Vanilla 26.3 ground Y / biome / top block |
| ---: | ---: | ---: | --- | --- |
| 0 | 0 | 71 | `minecraft:badlands` | Pending client observation |
| 256 | 0 | 61 | `minecraft:plains` | Pending client observation |
| 0 | 256 | 68 | `minecraft:forest` | Pending client observation |
| -256 | 0 | 73 | `minecraft:plains` | Pending client observation |
| 0 | -256 | 65 | `minecraft:badlands` | Pending client observation |
| 512 | 512 | 65 | `minecraft:taiga` | Pending client observation |

## What the comparison can establish

Each matched coordinate can show a specific height, surface-block, or biome
difference. Six points cannot prove whole-world parity; a matching point may
be coincidental. Once client observations exist, record each mismatch, select
one version-specific rule to investigate from official documentation or
observed vanilla behavior, implement it independently, and test unchanged
points plus neighboring chunk borders. Caves, oceans, structures, vegetation,
lighting, and all remaining biomes need separate evidence. See the
[compatibility matrix](../COMPATIBILITY.md) and [world-preview ADR](../decisions/ADR-0013.md).
