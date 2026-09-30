# Vanilla-compatible world generation: feasibility spike

Status: **research spike** (30 September 2026). This evaluates what a
vanilla-compatible 26.3 generator in RustMC would require. It authorizes
nothing by itself; the milestone decision belongs to the owner. See the
proposed [ADR-0014](../decisions/ADR-0014.md) and the measured divergence in
the [seed-2026 comparison](java-26.3-seed-comparison.md).

## Why this is hard, stated honestly

The six-coordinate worksheet shows the current preview generator diverges
completely from vanilla 26.3 (0/6 biomes, height gaps up to 46 blocks).
Closing that gap is not a tuning task: modern vanilla generation is a stack of
interacting data-driven systems, and every one of them must agree before a
single column "matches seed 2026".

## Subsystem inventory (what parity actually means)

| Layer | What vanilla 2026 does | RustMC today |
| --- | --- | --- |
| Shape | Density functions combining noise (continentalness, erosion, ridges, pockets, needles) into 3D field | Single 2D lattice sample |
| Biomes | Climate-parameter routing (temperature, humidity, altitude, weirdness, continentalness, erosion, penetration) into weighted biome clusters | Nearest jittered site over 8 labels |
| Caves | Noise caves (spaghetti/noodle/cave pockets), aquifers, carvers | None |
| Surface | Per-biome rule trees (surface/material rules) choosing top/filler/underwater materials | Fixed per-label blocks |
| Decoration | Ore veins with per-biome count distributions, trees/flowers via seeded feature passes, structures (villages, strongholds, …) | Fixed-height trees only |
| Seeding | One world seed expanded into many independent sub-seeds (worldgen, carvers, features, structures) | One seed mixed per sample call |
| Protocol output | Heightmaps, 4×4×4 biome palettes, block-light arrays in 26.3 chunk format | Already implemented for preview chunks |

Version churn matters: 26.3 itself changed noise settings and material rules
([26.3 snapshot notes](https://www.minecraft.net/en-us/article/minecraft-26-3-snapshot-10),
[world-generation background](https://www.minecraft.net/en-us/article/new-world-generation-java-available-testing)),
so parity work is per-version, not one-time.

## Data and licensing landscape

- Since Java 26.1 the shipped jars are **deobfuscated by default**
  ([Mojang: Removing obfuscation in Java Edition](https://www.minecraft.net/en-us/article/removing-obfuscation-in-java-edition),
  [community how-to with license summary](https://minecraft.wiki/w/Tutorial:See_Minecraft%27s_code)).
  Reading vanilla code is now easy; Mojang's terms still **do not allow
  releasing exact copies of the code**, and the precise EULA clauses for
  derived constants and data must be quoted from
  [the official EULA](https://www.minecraft.net/eula) during a legal review
  before implementation starts (open item).
- Generation is driven by **data** (noise constants, density functions, biome
  parameters, surface rules) inside Mojang's jars. RustMC must not commit or
  redistribute Mojang data. Third-party servers handle this with user-side
  downloads/extracts; that precedent is recorded in
  [Paper's vanilla-data-files note](https://docs.papermc.io/paper/reference/vanilla-data-files/)
  (cited as a licensing practice, not an architecture source).
- The Anvil region format and chunk/NBT layout are publicly documented, so a
  comparison tool can read **user-generated** vanilla worlds without touching
  Mojang binaries in this repository
  ([region format](https://minecraft.wiki/w/Region_file_format)).

## Testing strategy: an oracle, not eyeballs

The worksheet method (manual F3 readings) does not scale. The spike proposes:

1. **Oracle harness (T0)** — the owner runs their own licensed vanilla 26.3
   server headless with a fixed seed; a RustMC tool reads the resulting region
   files and extracts, per sampled column: terrain height, top block, biome.
   No Mojang file is ever committed; the tool ships empty.
2. **Match-rate metric** — over a fixed deterministic sample (e.g. 4,096
   columns per quadrant set), report exact height match %, biome match %, and
   top-block match %. The six-point worksheet becomes one hand-checked subset.
3. **Regression pin** — keep the existing
   `seed_2026_comparison_points_match_the_published_worksheet` test as the
   drift guard for whichever generator is active.

## Staged plan and effort gates

| Stage | Scope | Exit gate |
| --- | --- | --- |
| T0 | Oracle + match-rate tooling only | Reproducible report vs a local vanilla world; no generator change |
| T1 | Terrain shape: density/noise field → exact heights | ≥95% exact height match on T0 sample |
| T2 | Biome placement + surface rules | ≥95% biome and top-block match where T1 height matched |
| T3 | Caves, aquifers, carvers | 3D block agreement threshold on sampled columns |
| T4 | Ores and vegetation decoration | Distribution-level (not per-block) agreement |
| T5 | Structures | Separate spike; explicitly deferred — highest churn, least reusable |

Performance: vanilla generation is CPU-heavy (parallel workers in the client);
each stage must record release-build chunk-generation timings in
`docs/BENCHMARKS.md` style before acceptance. No speed claims beforehand.

## Risks

- **Legal ambiguity** until EULA review: derived numeric constants are the
  gray zone; mitigation is black-box observation via the oracle where
  possible, and owner/legal sign-off where not.
- **Version churn**: 26.3-specific re-validation needed each update cycle.
- **Scope**: structures/features are a project of their own; the staged plan
  allows stopping after any tier with a still-honest "terrain parity" claim.
- **Preview displacement**: ADR-0013's synthetic preview stays as the default
  until a tier passes its gate; no half-ported vanilla mode.

## Recommendation

Do not start generator code now. If the owner approves the direction, the
first authorized slice is **T0 only** (oracle tooling — tests and tooling, no
worldgen), because every later decision should be driven by measured match
rates rather than opinions. T1+ requires explicit owner authorization as an
M5 workstream under the policy proposed in ADR-0014.
