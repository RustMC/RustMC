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

## T0 outcome (30 September 2026)

`cargo run -p rustmc-tools --bin vanilla_oracle` now provides the oracle:
`inspect` reads selected columns, `worksheet` re-checks the six comparison
points, and `compare` samples a strided grid and prints exact-match
percentages with capped mismatch detail. It reads only saves the owner's
licensed client generated (26.3 places them under
`dimensions/minecraft/overworld/region/`, which the tool detects
automatically); no Mojang file is committed and no generator code changed.

Validated against the owner's seed-2026 world: all six biomes reproduce the
manual F3 readings, and the six heights reproduce the published worksheet
(117/64/73/84/71/71; the (-256, 0) cell reads 84 from the save versus the
hand-noted 83, so the worksheet's sharpest single height carries a ±1
observation caveat). Reverse-engineered 26.3 save facts, all from the owner's
own data: heightmaps are packed relative to the world minimum Y; blockstate
palettes mix string and compound entries (`{"": name}` /
`{id, properties}`); biome palettes are stored at their exact bit width
(even one bit), which the old "minimum 3 bits" assumption broke on — the
decoder now infers the width from the data length and verifies slots against
the palette.

First aggregate report over every 16th column in the loaded −256..512 square
(2,401 columns): **4.50% exact height match, 2.87% biome match**. These
measure one strided sample of one world; they quantify the gap and imply no
parity claim. T1 and later tiers remain unauthorized pending the ADR-0014
decision.

## T1 groundwork (30 September 2026, black-box only per accepted ADR-0014)

`preview_terrain = "experimental"` is an opt-in configuration for the local
preview. It switches column heights to an independently designed octave
noise stack (smoothstep-interpolated lattice fields for continents, hills,
and ridged mountains, all derived from seed and world coordinates). The
default preview generator, its ADR-0013 worksheet pin, and every existing
wire path are unchanged; biome placement and surface blocks still come from
the preview rules, so this is terrain-shape groundwork, not vanilla
generation. Measured against the same 2,401-column oracle sample, the
experimental field reaches 2.75% exact height match (preview: 4.50%) while
showing substantially wider relief — confirming that seed-exact parity needs
the full density-function pipeline, not a tuned noise field. No vanilla
constants were copied or consulted.

## T1 measurement (1 October 2026, data-driven density pipeline)

With the owner's authorization for the knowledge-consultation route (ADR-0014
as amended), the Rust `vanilla` modules now implement the full density path:
the world-seeded noise core, the density-function evaluator (point and
block-volume, pinned bit-for-bit against 26.3 runtime vectors), a runtime
loader for operator-provisioned worldgen datapacks (Mojang files are never
committed), per-dimension `noise_router` wiring, and column-height extraction
(`final_density > 0` surface scan plus the sea-level water rule).

Measured against the same 2,401-column seed-2026 oracle sample with
`vanilla_oracle compare ... vanilla`:

- **94.59% exact height match (2,271 / 2,401)**, up from 4.50% (preview) and
  2.75% (experimental groundwork). All six published worksheet heights
  reproduce exactly (117 / 64 / 73 / 84 / 71 / 71).
- Categorizing the 130 remaining columns by the vanilla save's top block:
  42 tree trunks and 11 village-ruin blocks (feature/structure placement —
  tiers T2/T3, absent from a density-only surface by definition; every
  sample tree column mismatches and 3 of 14 ruin columns match), 13 water
  surfaces where the saved heightmap counts partial water levels (fluid
  finalization, not density), and 64 terrain-topped columns whose small
  offsets concentrate near structure footprints and shallow aquifer bands —
  effects of the runtime blender/beardifier and the aquifer adjustment,
  which the current pipeline evaluates at their structure-free/aquifer-free
  defaults.
- The ≥95% tier gate is therefore not yet passed on the raw sample, and the
  raw sample structurally cannot be passed by density-only generation:
  tree and ruin columns above the true surface count as mismatches by
  construction. Excluding the 56 tree/ruin-top columns, the same scan reads
  **96.7% exact (2,268 / 2,345)**. Closing the remaining terrain-topped gap
  requires the runtime-context slice (aquifers, then structure blending
  with T3); the owner should also decide whether to re-scope the gate to
  terrain-surface columns.

The oracle's mismatch detail list now tracks height gaps only (biome misses
are T2 work and would drown the diagnostic); the aggregate biome count is
unchanged. A grid-refined fast scan is implemented and documented as an
approximation: it under-scanned 7 of 2,401 columns because the per-block
cave-carving `min` can dip a y-grid sample below zero while the surface
block between samples stays positive, so the exhaustive per-block scan is
the default.

## T1 gate passed (1 October 2026, runtime aquifer slice)

The surface rule was replaced with the fill rule the density pipeline is
designed to feed: the highest position whose block *substance* is not air,
where the substance is the raw `final_density` sample adjusted by an
independently implemented runtime aquifer (`vanilla::aquifer`), and a
dimension without an `aquifers` settings section degrades exactly to the
previous sea rule. The consulted semantics and the recorded numeric facts
behind the implementation are in `docs/PROVENANCE.md` (session 5); no
vendor code or data entered the repository.

Measured against the same 2,401-column seed-2026 oracle sample:

- **96.00% exact height match (2,305 / 2,401)** — above the ≥95% tier
  gate — with all six published worksheet heights still reproducing
  exactly (117 / 64 / 73 / 84 / 71 / 71). The aquifer slice gained 34
  columns over the density-only 94.59%.
- The remaining 96 mismatches are dominated by the feature columns
  (tree trunks, village-ruin blocks) that a density+aquifer surface
  structurally cannot cover, plus lake-edge and mountain-slope offsets of
  ±1–7 attributable to structure-footprint blending (T3 machinery) and
  to cave carving below the surface interacting with the heightmap.
- Timing record for this stage: the release build scans the whole sample
  single-threaded in 57.1 s wall (≈24 ms/column including datapack load
  and save reads), reproducible run-to-run.

T1's exit gate is met on the raw sample. The owner's standing instruction is
to continue through the staged plan milestone by milestone, so T2 (biome
placement and surface rules — the router already exposes the
continents/erosion/depth/ridges/temperature/vegetation fields it needs) is
the next workstream.

## T2 biome placement passed (1 October 2026, slice F)

The chunk biome is now resolved the way the game resolves it: the six
router densities sampled per 4-block quart cell, quantized to fixed-point
integers, and matched against the preset's parameter table by minimum
squared-distance fitness (`vanilla::biome`). The table itself is code-side
in 26.3, so the owner provisions it as a numeric capture beside the local
data root; the repository keeps only the loader and the format (session 6
in `docs/PROVENANCE.md`). Measured against the same 2,401-column seed-2026
sample:

- **99.92% biome match (2,399 / 2,401)** — far above the ≥95% tier gate —
  with all six published worksheet biomes reproducing exactly. The two
  residuals sit at blender-affected borders, the known structural
  exception of a structure-free pipeline.
- Exact-height match is unchanged at 96.00%; biome lookup rides the same
  surface scan and did not regress timing materially.

The remaining T2 half is surface rules: 26.3 materializes top blocks from
`worldgen/material_rule`, `material_condition`, and `block_state_provider`
datapack graphs evaluated over the column — a data-driven loader and
evaluator (slice G), the next workstream.

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
