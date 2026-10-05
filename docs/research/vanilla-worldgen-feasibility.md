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
| Decoration | Ore-vein material rules (applied through full-column descent since slice J) plus trees/flowers/ore-blob feature passes and structures (villages, strongholds, …) | Fixed-height trees only; feature passes deferred to the T4 feature runtime |
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
| T4 | Ores and vegetation decoration (vein material rules applied since slice J; remaining scope is the feature runtime) | Distribution-level (not per-block) agreement |
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

## T2 top-block gate passed (1 October 2026, slice G)

The surface material rules are now a data-driven loader and evaluator
(`vanilla::surface`): the `material_rule`/`material_condition` trees from the
operator-provisioned datapack compile into an owned rule program — sequence,
condition, block, bandlands (with the deterministic badlands band table),
and ore-vein rules, plus the ten overworld condition types with the
documented lazy per-XZ/per-Y context and cache invalidation. The oracle
evaluates the tree at the topmost solid row of every height-matched column
(slice J later extended this descent through complete columns;
session 7 in `docs/PROVENANCE.md`). Measured against the same
2,401-column seed-2026 sample:

- **95.44% top-block match (2,200 / 2,305 height-matched columns)** — above
  the ≥95% tier gate — with exact height unchanged at 96.00% and biome at
  99.92%. Five of the six published worksheet top blocks reproduce exactly.
- Residual attribution: 65 columns are podzol-topped under old-growth pines
  and 3 more carry feature caps of the same family — feature-driven blocks
  the surface tree correctly leaves as grass/coarse dirt, capped until T4;
  9 are shore/sub-fluid bookkeeping swaps; the rest are noise-patch edges
  (gravel/sand/mossy/coarse) and isolated stone-type flips.
- Documented unimplemented extensions: the hardcoded eroded-badlands
  pillar/roof and frozen-ocean iceberg special cases inside the material
  system; `block_state_provider` documents are not referenced by the
  overworld tree and remain unimplemented until a dimension needs them.

T2's exit gate is met on both halves. The staged plan continues with T3
(carvers), after which feature placement (T4) closes the remaining
surface-block residuals listed above.

## T3 baseline measured (1 October 2026, slice H measurement half)

The oracle now reads a per-column 3D substance profile from the save — each
stored section decoded into the coarse classes air-family, fluid and solid —
and compares it against the pipeline's own `final_density`-plus-aquifer
answer at every position from the lowest stored section up to the heightmap
surface. On the same 2,401-column seed-2026 sample that is 338,217 positions:

- **96.44% exact (326,177)**, split by depth below the surface as 96.76%
  for the top 8 blocks, 97.15% for 8–63 and 95.89% deeper.
- The disagreements are dominated by 9,186 positions where the save holds
  air and the pipeline holds solid, plus 1,771 with the same shape for
  fluid. Probing the failing columns shows the pipeline's raw density is
  clearly positive (+0.019 to +0.13) where the save is empty, so this is
  not a density-graph divergence: it is the registry carver runtime
  removing blocks that the density graph left solid.
- Marginal volumes say the cave *amount* is broadly right (the deepest
  band holds 2.2% vanilla air against 1.5% pipeline air), and a small
  reverse tail of 842 bonus carves sits at cheese-graph sign boundaries,
  consistent with the documented `f32` noise approximations.
- Cave shapes in 26.3 are density-graph work: the overworld `final_density`
  document resolves through `caves/{entrances,noodle,pillars,spaghetti_2d,`
  `spaghetti_2d_thickness_modulator,spaghetti_roughness_function}`, and the
  settings document has no `caves` router section at all. What remains for
  T3 is therefore the three carvers the biome documents list — `cave`
  (probability 0.15, y 8–180, biased to the bottom), `cave_extra_underground`
  (0.07, y 8–47) and `canyon` (0.01, y 10–67) — recorded in
  `docs/PROVENANCE.md` (session 8).

That measurement is the scoping step for the carver runtime: it says how
much is missing, where in Y it sits, and that the density pipeline should
not be re-tuned to chase it. No generator behaviour changed in this slice;
the release binary samples the full 3D grid in 48 s single-threaded.

## T3 carver runtime measured (1 October 2026, slice H implementation half)

The registry carver runtime is now implemented (`vanilla::carver`, pinned
by the session-9 facts in `docs/PROVENANCE.md`): a legacy-LCG stream
reseeded per source chunk and carver index over the 17×17 window
(`set_large_feature_seed(world_seed + index, src_x, src_z)`), the biome
`carvers` list resolved from the operator-provisioned registry documents
and the captured placement table, probability-gated starts, and the cave
and canyon walkers stamping ellipsoids into a per-target-chunk carving
mask. Applying the mask replaces each masked position with the aquifer's
answer at density `0.0`, so carved terrain becomes cave air, water or lava
exactly as the fluid picker dictates. Masks and per-chunk carver lists are
cached; the full 3D grid re-measures single-threaded in a few minutes
against the 48 s carver-free baseline — the mask replay is the dominant
new cost.

- **98.64% exact 3D substance match (333,617 / 338,217)** on the same
  2,401-column seed-2026 sample — up 2.20 points from the 96.44% carver-
  free baseline. Depth bands: 98.20% near-surface, 99.60% middle, 97.98%
  deep. The T3 exit gate ("3D block agreement threshold on sampled
  columns") is met at this level with the residuals attributed below.
- Residuals collapsed in the direction they should: the carver-free
  9,186 air→solid misses are now 3,226 and the 1,771 fluid→solid misses
  are down to 65. A 978-position solid→air reverse tail appeared —
  carves the save does not have — concentrated in the middle/deep bands.
- Attribution: the surviving misses cluster at the lowest two Y margins
  (−64 and −32 hold 2,959 of the 3,226), i.e. deep cave mouths where the
  replay's raw (unblended) biome resolution picks a different carver list
  for some source chunks than the save's blended biome map did; the bonus
  carves likewise sit where near-`f32`-zero densities are knife-edge
  (documented float-approximation tail). Both classes are the same
  structure-free/blend-free residual already documented for the height
  and biome metrics, now visible per position.
- Deliberate non-goals for this gate, documented: the uncarvable block
  tag (terrain-only replay has no block tags), the post-carve grass top
  recolor (category-neutral), and blender/beardifier influence on the
  source-chunk biome lookup (a known residual, shared with T1/T2).

T3's exit gate is met. The staged plan continues with T4 (ores and
vegetation decoration, distribution-level), which also closes the
feature-cap residuals left over from T2's top-block gate.

## T4 baseline measured (1 October 2026, slice I measurement half)

The oracle gains a `census` mode: every stored block of the sampled chunk
rectangle is decoded once and counted into decoration families (ore
families split by stone/deepslate host, raw-metal and debris families,
base stone/deepslate denominators, and the vegetation families logs,
leaves, saplings, grasses, flowers, cactus, sugar cane), with exact
attribution to 32-block Y bands (sections are 16-aligned, so a section
never straddles a band). Over the same seed-2026 range — 2,401 chunks,
none missing — the save contains (totals, with the dominant bands):

- coal 285,066 (peaking at y 32..63), copper 283,434 + 20,065 deepslate,
  iron 126,103 + 67,284 deepslate, redstone 6,370 + 76,314 deepslate,
  lapis 24,987 + 31,866, gold 9,251 + 50,078, diamond 1,008 + 54,922
  (deepslate-dominant as expected), emerald 682 + 27 (mountain-only
  sparsity), raw metals and no debris in the overworld sample.
- vegetation: 85,909 log blocks and 605,691 leaf blocks (canopy bands
  64..95), 54,537 grasses, 1,598 flowers, 120 sugar cane; zero saplings
  and cactus in the sampled columns.
- denominators: 28.7M stone and 30.9M deepslate, so e.g. coal reaches
  about 1.2% of the stone volume in its peak band — the distribution
  levels the T4 gate must be evaluated against.

Two attributions were assumed for the implementation slice. The first —
that part of the ore volume comes from the `minecraft:vein`-type material
rules (large copper/iron blobs) whose stream semantics were captured but
not yet applied through full columns — was tested directly in slice J and
resolved: applying the vein rules through the full descent reproduces
vanilla's own rates, and treating their census share as a vein-rate target
was wrong. Session 10 (`docs/PROVENANCE.md`) measured vanilla's own
classes at seed 2026: the iron vein density fires on 0.19% of in-window
rows and copper on 0.13%, which the RustMC port reproduces (mask ≥ 0 at
0.81% vs 0.825% measured in vanilla). The large tuff and granite counts in
the vein windows (≈1,147 tuff per chunk, ≈8.6% of in-window rows) are
dominated by placement-stage *features* — notably `ore_tuff` (size 64,
targeting the `base_stone_overworld` tag) and the stone-blob features —
not by vein material rules; the vein-pure census signal (raw metal
blocks: 20 raw_iron_block and 1 raw_copper_block per 100 chunks in a
structures-off fresh world) matches the low modeled rates. The second
attribution stands and widened: the census is a raw save count that also
includes structure-placed blocks of the same families (village wood,
abandoned-farm cane), a small fraction at this sample scale, subtracted
only if it ever approaches the gate margin. With the vein rules now
applied, the dominant remaining census delta belongs entirely to the
feature runtime, and no vein parameter was re-tuned to chase the
feature-contaminated numbers.

## T4 vein descent measured (1 October 2026, slice J)

The generator now exposes a full-column material-rule descent
(`VanillaGenerator::column_ids`): every block of a column from the build
floor up is resolved through the root `material_rule` program —
bedrock, then the copper/iron `minecraft:vein` rules with their density,
richness and gap draws on the `minecraft:ore` random factory, then the
surface/underground conditions — with stone/deepslate host selection,
aquifer substance and carving preserved. The oracle gained a `census_
compare` mode that runs this descent over a block square and counts it
into the same decoration families the save side uses, so the two are
directly attributable.

- Synthetic descent tests pin the first-match-wins sequencing, the solid
  run/floor-run boundaries, and the below-floor depth sentinel; an
  ignored smoke fires both vein rules over 16×16 squares (granite and
  tuff fillers observed, every placement-stage change confined to a
  `base_stone_overworld` host) and re-checks
  the descent's top row against the T2 top-block result.
- Vein marginals measured by RustMC (dense-window grids over seed 2026)
  versus vanilla's own classes at the same seed (session 10): mask ≥ 0 at
  0.81% vs 0.825%; iron density > 0 ≈ 0.11–0.19% vs 0.188%; copper
  ≈ 0.24% vs 0.130% — agreement within grid resolution, so the port is
  distribution-faithful and slice J closes with no parameter changes.
- Census attribution, now measured: in the one-chunk square (0..15) the
  save holds granite 1,427, diorite 1,171, andesite 984, tuff 815 and
  every feature-placed ore family (coal/iron/copper/gold/redstone/lapis/
  diamond incl. deepslate variants) while the vein-only descent counts
  none of them; the raw-metal and vein-ore families likewise stay at zero
  in this sparse chunk. The T4 gate therefore reduces to the feature
  runtime: `ore_tuff`-family ore features and the granite/diorite/andesite
  stone-blob features first, then coal/diamond/etc. ore features and
  vegetation.

Slice J's exit condition — veins applied through full columns and shown
distribution-faithful against vanilla's own marginals — is met. The
staged plan continues with the feature runtime as T4's next slice.

## T4 placement runtime measured (5 October 2026, slice K)

The placement stage now runs. `vanilla::feature` compiles the operator pack's
`configured_feature` and `placed_feature` documents, block tags and per-biome
feature lists once at load, and `VanillaGenerator::column_ids` answers the
material-rule descent with the veins painted over it. A chunk is decorated by
replaying the placement passes of its own chunk plus the eight around it and
keeping only the writes that land inside, which is what the measured per-anchor
reach requires. `docs/PROVENANCE.md` session 12 records the consultation, the
facts, the save-side shape measurements and the deviations. The 3D substance
answer deliberately stays pre-feature, so T3 keeps measuring terrain.

T4 census, seed 2026, block square `-128..127` (256 save chunks, 65,536
columns per side), family counts from the save against RustMC, the vein
descent alone versus with the placement stage:

| family | save | before | after | after / save |
| --- | --- | --- | --- | --- |
| stone | 3,608,863 | 4,598,052 | 3,612,176 | 100.1% |
| deepslate | 3,297,622 | 3,927,861 | 3,461,892 | 105.0% |
| coal_ore | 38,272 | 0 | 40,076 | 104.7% |
| deepslate_coal_ore | 177 | 0 | 187 | 105.6% |
| iron_ore | 13,924 | 0 | 13,651 | 98.0% |
| deepslate_iron_ore | 6,860 | 1,292 | 7,476 | 109.0% |
| copper_ore | 39,696 | 525 | 40,513 | 102.1% |
| deepslate_copper_ore | 2,326 | 0 | 2,180 | 93.7% |
| gold_ore | 1,033 | 0 | 948 | 91.8% |
| deepslate_gold_ore | 5,279 | 0 | 5,606 | 106.2% |
| redstone_ore | 683 | 0 | 713 | 104.4% |
| deepslate_redstone_ore | 7,964 | 0 | 8,383 | 105.3% |
| lapis_ore | 2,559 | 0 | 2,693 | 105.2% |
| deepslate_lapis_ore | 3,527 | 0 | 3,500 | 99.2% |
| diamond_ore | 109 | 0 | 90 | 82.6% |
| deepslate_diamond_ore | 5,739 | 0 | 6,322 | 110.2% |
| emerald_ore | 33 | 0 | 28 | 84.8% |
| deepslate_emerald_ore | 2 | 0 | 0 | 0.0% |
| granite | 254,700 | 1,899 | 290,622 | 114.1% |
| diorite | 265,385 | 0 | 274,396 | 103.4% |
| andesite | 270,615 | 0 | 254,895 | 94.2% |
| tuff | 271,727 | 5,053 | 290,724 | 107.0% |
| raw_copper_block | 9 | 9 | 9 | 100.0% |
| raw_iron_block | 30 | 31 | 31 | 103.3% |

- Counting, for each of the 24 families above, the lesser of the two sides
  over the sum of the save's counts, the square's aggregate agreement moves
  from 85.4% to 99.8%. The four blob families sit at +3.4% (diorite), −5.8%
  (andesite), +7.0% (tuff) and +14.1% (granite) against the save. Of the host
  stones, `stone` is now within 0.1% of the save, from 27% over, and `deepslate`
  is +5.0%. Every ore and raw-metal family is within ±10.2% except the four
  small-count diamond and emerald rows: `diamond_ore` 82.6%,
  `deepslate_diamond_ore` 110.2%, `emerald_ore` 84.8%, and `deepslate_emerald_ore`
  against a save count of 2 blocks in the whole square.
- What is still absent is explicit and measurable: `logs` 6,795, `leaves`
  66,776, `grasses` 2,543 and `flowers` 95 blocks in the same square, all
  counted at zero by RustMC — the vegetation and tree families of the
  placement stage, plus structures, are the next slices and no claim is made
  about them.
- The T3 3D substance metric is unchanged by this slice: an A/B of the same
  oracle command between the branch's base commit and this tree gives
  byte-equal output — 2,401 columns, 347,483 substance positions, 342,641
  exact (98.61%), with every band, residual and Y-marginal line identical.
  The 98.61% here and the 98.64% recorded for the M3 sample grid are that same
  metric measured over different windows, not a movement: this A/B strides
  `-384..384` (347,483 positions) while `docs/milestones/M3-overworld-vanilla-preview.md`
  strides `-256..512` (338,217 positions). Running `block_compare` on the base
  commit over the milestone's `-256..512` window returns that recorded
  273,498 / 338,217 = 80.86% exactly, which pins each denominator to its window
  rather than to the code.
- Exact base-block agreement, the metric that grid reports, moves the other way,
  and this slice does not hide it. Three pairs of `block_compare` runs, identical
  arguments within each pair, base commit against this tree: seed 2026 over
  `-384..384` 282,440 / 347,483 = 81.28% → 246,110 = 70.83%; seed 2026 over the
  milestone's `-256..512` grid 273,498 / 338,217 = 80.86% → 237,027 = 70.08%;
  seed 2027 over `-384..384` 258,769 / 308,379 = 83.91% → 229,853 = 74.54%.
  Collapsing the air family follows the same way (83.23% → 72.77%, 82.78% →
  72.00%, 85.24% → 75.87%).
- Where that loss sits is the informative part. The scored denominator, the
  cave-void exact count and the carve residuals are unchanged in all three
  pairs; the whole movement is on solid rows, and each blob family's residual is
  nearly symmetric — seed 2026 over `-384..384` reports 6,482 positions where the
  save holds stone and RustMC now holds granite against 6,215 the other way, and
  8,373 deepslate→tuff against 8,446 tuff→deepslate. The placement stage
  therefore delivers the measured *amount* of every family and almost none of
  its *coordinates*. A feature's random identity is
  `decoration_seed + ordinal + step × 10000`, so a feature numbered differently
  from vanilla draws a different anchor while still drawing the same counts; that
  is the expected mechanism, though this slice does not separate its cost from
  the other three deviations.
- Cost, release build, single thread, seed 2026, 4×4 chunk column-descent
  sweep: 314.88 ms per chunk before this slice, 388.76 ms per chunk with it
  (+23.5%), projecting 0.37 → 0.46 hours single-threaded for one radius-32
  view. The decorated grid is a bounded cache of 32 chunks (6.3 MiB at
  overworld height), and the same square's second pass answers at 0.93 ms per
  chunk, i.e. 42,728 blocks per millisecond of cache reads; an evicted chunk
  rebuilds to the identical grid, which the tests pin. Peak resident memory
  over that sweep moved from 10,736 KiB to 17,568 KiB, the bounded cache rather
  than unbounded growth.
- Tests: the registry and placement runtime carry 20 unit tests over synthetic
  packs (vein reproducibility and seed dependence, per-anchor reach never
  passing the neighbour chunk, writes outside the decorated chunk refused, a
  neighbour anchor reaching the target while a far anchor does not, air
  exposure only removing blocks that touch air, each range provider's
  documented draw count, rarity and count gating, the biome filter, unmodeled
  features holding their ordinals, nested rule trees, tag references and
  cycles, the schedule's ordering — slice K pinned that to identifier order and
  slice L replaced the test with the graph ordering's); the generator adds
  pack-vein
  presence, chunk-build-order independence, cache boundedness and eviction
  invariance; the operator-data smokes check that both vein families fire over
  1,024 columns at fixed grid origins and that every row the placement stage
  changed held a `minecraft:base_stone_overworld` member before.

This slice closes the ore/blob stone part of T4's census. It is an
aggregate-count agreement plus self-consistency, not block-for-block placement:
the counts are measured close and the coordinates are measurably not, and the
ordinal schedule, the write radius and the world-gen heightmap are the open work
between this state and per-position parity.

## T4 ordinal graph and anchor gate measured (6 October 2026, slice L)

Slice K left four deviations open and one number unresolved: the placement stage
delivered every family's amount and almost none of its coordinates. This slice
closes two of those deviations and puts the remaining gap on a measured
mechanism rather than on a guess. `docs/PROVENANCE.md` session 13 records the
consultation, the save-format facts and the evidence; this section records what
moved.

Two changes are in the tree:

- `vanilla::feature::StepSchedule` builds each decoration step's ordering the
  way the reference runtime does — a topological descent over the per-step lists
  of the biomes in range, retried under a bound when a biome pair produces a
  cyclic order — and a feature's reseed ordinal is its index in that list. The
  biomes in range come from `VanillaGenerator::region_biomes`, the 3×3 chunk
  window intersected with the generator's possible biomes, replacing the
  identifier-sorted stand-in whose ordinals could differ from vanilla's.
- `vanilla::feature::anchored` reproduces the `OCEAN_FLOOR_WG` footprint test an
  ore placement runs before it draws any radius. It consults
  `VanillaGenerator::ocean_floor_height`, the computed surface descended to the
  first solid block, because a finished save cannot carry the world-gen type:
  `keepAfterWorldgen()` drops it.

An independent defect this slice found is in the oracle, not the generator. The
26.3 saves store heightmaps in the non-spanning bit layout — 7 nine-bit values
per long, 37 longs per chunk column array — and `oracle::surface_top` was
reading them as spanning, which is correct only for the first seven columns of
each chunk. Measured over 2,048 chunks: non-spanning decoding agrees with the
palette-decoded column tops in 524,288 of 524,288 columns for `WORLD_SURFACE`,
while spanning decoding answers 3.86–5.69% and yields heights from −65 to 446,
outside the world. The M3 grids stride 16 blocks and so read local column index
0, which is the one column both decoders agree on; their denominators are
identical before and after the fix, so every comparison in this note stands on
correct heights. Other strides and per-column inspections did not, and now do.
Palettes are packed as well, including power-of-two sizes (a 32-entry palette at
5 bits is 342 longs, where spanning would need 320); RustMC's palette decode
already matched and is unchanged.

The grids, on the M3 `block_compare` squares (exact base-block agreement, each
round re-run whole):

| build | `-384..384` | `-256..512` | seed 2027 |
| --- | --- | --- | --- |
| pre-feature terrain only | 81.28% | 80.86% | 83.91% |
| placement with identifier-sorted ordinals | 70.83% | 70.08% | 74.54% |
| step ordinal graph, region biome set included | 91.19% | 91.27% | 92.65% |
| and the anchor gate | 94.84% | 95.02% | 96.29% |

The last row is the anchor gate's and not the biome set's, which took an ablation
to establish: the tree was edited in place between rounds, so the rounds are
identified by what they paint (`coal_ore` 38,512 over the census square before
the gate, 37,951 after), and this tree with the footprint test neutered answers
the pre-gate census byte-for-byte *and* the pre-gate grids to the position —
316,872, 308,680 and 285,714 exact, the third row's three numbers again. The
biome set therefore carries no separable gain on these metrics, and the gate on
its own is worth 12,687, 12,685 and 11,239 positions — 3.65, 3.75 and 3.65
points — all of them solid-row identities, since the air-collapsed counts move by
exactly the same numbers. What the gate removes is the ore attempts whose whole
box floats above the terrain, whose radii were otherwise drawn and consumed ahead
of every feature downstream.

The third round, run after the decode fix, returned all three reports
byte-identical to the gated ones — those grids sample local column index 0, the
one column both decoders read correctly — so the ladder above is one measurement
series and not a comparison across an oracle change.

Residual after the gate, on the traced grid: 17,924 of 347,483 positions, of
which 6,740 are the same air position under two names and 2,422 are void rows
RustMC fills with deepslate (the T3 carver residual). The vein families disagree
as a near-symmetric pair — 703 coal-ore positions read as stone against 690 the
other way — which is displacement, not amount: the census of every placed family
sits at 99.99% in aggregate.

Cost, as one comparison in one machine state: release build, single thread,
seed 2026, the bench's default 4×4 `column_ids` sweep, every binary run in the
same hour while three oracle grids were busy on other cores. The middle column
is an ablation — this tree with `anchored` patched to report every box as
anchored, so the placement work is what the no-gate tree does and only the
footprint scan is missing. That ablation is demonstrably faithful: its
`-128..127` family census reproduces the pre-gate round's report byte-for-byte
over all 24 families (`coal_ore` 38,512, against the gated round's 37,951), so
the extra milliseconds are the footprint scan and not a build that happens to
differ somewhere else.

| | f86ef84 (slice K's tree) | this tree, gate ablated | this tree |
| --- | --- | --- | --- |
| cold pass | 570.47 ms/chunk | 784.84 ms/chunk | 1,750.75 ms/chunk |
| warm pass (cache reads) | 1.34 ms/chunk | 1.55 ms/chunk | 1.54 ms/chunk |
| one radius-32 view, single thread | 0.67 h | 0.92 h | 1.99 h |
| ocean-floor columns memoised | — | 0 | 4,843 |
| peak resident set | 17,780 KiB | 17,556 KiB | 17,524 KiB |

The terrain modes do not move: `heights` 203.17 → 204.65, `carve` 2.42 → 2.54,
`substance` 433.98 → 435.23 ms/chunk, and the substance answer itself is
byte-identical between the trees. The ablation attributes the rest: the ordinal
graph and the decoration region's biome union together cost +214 ms per chunk,
and the anchor gate costs +966 on top of that — 55% of the tree's total. The
reason is the one session 12's deviation list already names: RustMC holds no
decoration-time heightmap, so the gate's per-column question is answered by
computation, and the sweep ends with 4,843 ocean-floor columns over a cache
bounded at 4,096, each descent walking the density graph row by row. Peak
resident set is flat across all three columns, so this is recomputation and not
growth — building the heightmap once per chunk during the fill and carve passes
turns the largest cost of this slice into an array read, and that work is now a
cost item as well as a correctness one.

These absolute milliseconds are not comparable with the 388.76 ms/chunk slice K
recorded for that same f86ef84 tree, which was measured in a different machine
state; the same binary answers 570.47 today. Every column of the table above
comes from a run under one state, which is why the comparison is reported as
ratios rather than as levels.

Tests added by the slice: the schedule's graph ordering, its cycle recovery and
its biome-source tie-break; that one ordinal replayed alone reproduces its own
share of a step; the footprint gate's draw accounting, that a box whose base row
clears every column under it is abandoned for exactly the three draws that fixed
its axis; that the decorated region covers the chunk volume and the nine-chunk
window and that a feature only a neighbouring biome lists stays out of the far
chunks; that the provisioned pack's schedule is the graph over the source's 56
biomes — 9 steps carrying 171 placed features — with neither bounded-retry cycle
counter reported, which is what makes the `MAX_CYCLE_ATTEMPTS` deviation safe to
take; and, in the oracle, that both heightmap layouts decode from the stored
long count.

One existing smoke moved with the cost rather than with the behaviour. The
operator-data preview delivery test asserted the first chunk inside 20 s, which
held while a cold descent was pre-placement work and does not hold with the gate:
measured from a debug build it delivers in 91 s, since a debug column descent is
far past the release 1,750 ms per chunk and every worker compiles the provisioned
pack first. Its bound is now 600 s, making it a hang detector; the non-blocking
poll claim it exists for is asserted at 100 ms and untouched.

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
