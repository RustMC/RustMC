# Java 26.3 Overworld validation and live preview

Status: planned. This Java-only work does not close the M3 dual-edition join gate or establish vanilla parity. The existing opt-in synthetic preview remains the default under [ADR-0014](../decisions/ADR-0014.md).

## Goal and boundaries

Render RustMC's independently implemented, data-driven Overworld in a real Java 26.3 client, then compare its saved and displayed results with an owner-generated vanilla 26.3 world at the same seed. Operator-provisioned game data stays local. The first live slice is a read-only Creative inspection world on loopback; it does not provide secure authentication, multiplayer, saves, block interaction, or a release.

## Live adapter probe plan (2 October 2026)

The current Java client still receives the original synthetic preview. Wire the
data-driven column adapter behind a separate, explicit local configuration flag.
Keep the synthetic path as the default. Before binding, validate the local
versioned ID table and generated data root. Generate one chunk at a time on a
bounded worker so that network polling and keepalives continue during slow
generation. A disconnected client must close its worker after its current
bounded chunk finishes. Start with a small view radius, inspect the actual
Java 26.3 display and F3 coordinates, then compare visible terrain and cave
blocks with the same-seed save. Record generation, encoding, and delivery
separately. This probe does not claim feature-stage ores, trees, structures,
block interaction, radius-32 delivery, or vanilla parity.

## Acceptance sequence

- [ ] **Ground truth:** Record the save's exact client version, seed, preset, datapacks, and mod effects. Sample loaded chunks across positive and negative coordinates. Compare height, biome, top block, and full 3D block categories; add exact block-state comparisons where the RustMC generator represents them. Report numerator, denominator, missing chunks, and differences. Keep the save and any extracted game data out of Git.
- [ ] **Finite generation:** Bound the research generator's caches and per-client chunk work. Reject unsupported or missing operator data at startup with actionable errors. Measure cold and warm generation per chunk and the memory/work retained at the configured view radius; do not promise 32-chunk real-time delivery from one sample.
- [ ] **Versioned chunk adapter:** Map every emitted block state and biome to an operator-provisioned Java 26.3 protocol ID. Fail on unknown states rather than substituting a plausible block. Encode all 24 vertical sections, heightmaps, fluids, light, and chunk boundaries correctly, with packet-size tests and negative-coordinate fixtures.
- [ ] **Opt-in client view:** Send the data-driven chunks through the existing bounded Java preview session. Confirm a real 26.3 client renders terrain above and below sea level, cave openings and interiors, biomes, and movement-driven chunk loading. Capture coordinates and known mismatches. Leave the synthetic preview as the default until the owner accepts a change.
- [ ] **Terrain completion:** Implement and independently verify remaining Overworld feature placement (ores, vegetation, trees) and structures in separate slices. Measure exact blocks and distribution against vanilla saves; do not label a partial generator vanilla-equivalent.
- [ ] **Authoritative editing:** Introduce ordered block intents and a bounded mutable world state before enabling placement or breaking. Verify state changes with at least two clients, chunk-edge updates, drops/inventory rules, save/reload, and crash recovery before claiming gameplay support. Creative inspection alone does not satisfy this item.

Every checked item needs focused tests, `cargo fmt`, `check`, Clippy with warnings denied, tests, build, rustdoc, the dependency license gate, diff review, and passing CI. Record client observations separately from automated evidence and keep edition/version scope explicit in the [compatibility matrix](../COMPATIBILITY.md).

## Integration status of the first slices (2 October 2026)

Four parallel slices landed on this branch together. None of the acceptance items
above is checked by them, and the sequence below says exactly where each one stops.

- **Ground truth** — evidence landed (this document's measurement section, plus the
  oracle's `block_compare` mode). Item stays unchecked: base-block ids only, one seed,
  one preset, no block-state properties, no client involved.
- **Finite generation** — partially landed. The seven coordinate-keyed generator memos
  are now fixed-capacity (`vanilla::cache`, 1,407 KiB total per generator, verified by
  occupancy tests that sweep far past each bound and by an eviction-recomputes-identically
  test). Cold full-descent cost is *not* satisfied: measured 13,954 ms per chunk on a 2×2
  cold sweep and 12,682 ms on a 4×4 sweep, which projects to roughly 15–16 hours
  single-threaded for one 4,225-chunk radius-32 view, with peak RSS ~10 MiB. A 32-chunk
  live view cannot be sustained from this yet, and nothing here promises it.
- **Versioned chunk adapter** — landed as an encoding seam, not a live path.
  `chunk_adapter::registry` validates an operator-provisioned 26.3/777 id table and makes
  any unclassified id a typed error; `chunk_adapter` encodes all 24 sections, three
  heightmaps, per-section fluid counts, and 26 skylight layers, with the tests decoding
  the bytes back off the wire. Deferred and stated in the module docs: block entities,
  neighbour-chunk border blocks, per-layer vertical biome selection, and block light. A
  real 26.3 client has **not** been shown an adapter-built column, and the worst-case
  251,457-byte column against the preview's own 786,432-byte batch budget means the
  budget binds at batch level.
- **Opt-in client view** — not started: no session code sends adapter output, and the
  synthetic preview remains the default.
- **Terrain completion** — not started. The measurement below attributes 10.4% of the
  scored positions (35,223) to the four vein/blob stones the save stores where we still
  write plain stone or deepslate — the placement-stage blob and ore runtime, which is the
  next generator lever.
- **Authoritative editing** — design landed
  (`docs/milestones/M3-authoritative-block-interaction.md`) with its numeric mechanics
  deferred to BIND/OBSERVE tasks; no gameplay code.

One cross-cutting blocker recorded by the design slice and unresolved here: the preview
`world::Chunk` index space is `0..WORLD_HEIGHT` while the adapter emits absolute Overworld
rows `-64..319`, so the two chunk representations in this tree are not the same coordinate
model. Settling one authoritative world-Y mapping (decision **D1** / requirement **V6** in
the block-interaction milestone) is a prerequisite for both live wiring and gameplay, and
no mapping has been chosen or implemented.

## Interpolated-corner cache follow-up (2 October 2026)

Profiling a release-build, one-chunk column descent showed repeated Perlin
evaluation in the point path of the `interpolated` density node. Neighboring
block positions recomputed the same eight aligned cell corners. A per-node,
two-generation cache now retains at most 128 corner sets; keys are absolute
aligned cell origins and eviction only recomputes immutable input. The volume
path and density formulas are unchanged. A cache-eviction test and the existing
bit-exact density vectors pass. Re-running the seed-2026, 2,401-column save
comparison gave the same 273,498 / 338,217 exact base-block positions (80.86%)
and the same per-band counts as the pre-cache worksheet below.

On this machine, a fresh release-build one-chunk run of `bench_vanilla_chunks
--side 1 --repeat 1 --mode columns` took 14,549 ms before and 1,547 ms after
the cache. A separate 2×2 cold sweep after the change took 6,030.5 ms total,
or 1,507.6 ms/chunk. These are local measurements with a warm filesystem and
one seed, not a throughput or client-visible latency guarantee. At that rate,
4,225 chunks still project to about 1.77 hours of single-threaded work. The
finite-generation and live-client acceptance items remain unchecked.

The local `prepare_chunk_registry` tool now derives a versioned ID table from
operator-generated official 26.3 reports. Using that table, an ignored adapter
smoke encoded generated chunks `(0,0)` and `(-1,2)` to 74,943 and 77,001 byte
frames. This verifies ID lookup and encoding on those two columns only. No
client has received them; state-kind tags, lighting, and live delivery still
need validation.

## Biome lookup profiling and bounded cache (2 October 2026)

A release-build `perf` run on four seed-2026 chunks found repeated linear
biome-placement searches as the largest sampled cost before this change.
RustMC now reuses a biome result within its quantized 4×4×4 coordinate cell,
with at most 8,192 cells retained per generator (two generations of 4,096).
The cache stores only results of the same climate sampler; a test compares
cached and uncached answers over 9,000 cells and checks eviction. No biome
placement values or sampling rules changed.

On this machine, the same release-build 2×2 cold column sweep measured
6,259.6 ms before and 2,088.6 ms after this cache, including a concurrently
running local preview in the latter measurement. The corresponding per-chunk
means were 1,564.9 and 522.2 ms. These are generator timings, not client
render latency. A four-worker local client probe is still visibly slow at a
radius-2 view, and a radius-32 view remains unverified. A second `perf`
sample after the cache attributes about half of sampled CPU time to RustMC's
Perlin gradient/interpolation code; this is the next measured hotspot.

The local client delivery path now keeps its bounded vanilla workers generating
while the client acknowledges the previous one-chunk batch. Chunk unloading
waits until an acknowledged batch can carry the forget packets. An
operator-data smoke confirms that a worker remains in flight during this
interval. This removes idle generation time between batches; it does not
change the measured cost of generating an individual chunk or establish a
client-visible speed result.

For the radius-32 probe, RustMC now sorts the player's 4,225 chunk positions
only when the player crosses a chunk boundary, rather than on every 10 ms
network poll. When all workers are occupied and a client batch is awaiting
acknowledgement, the poll returns without scanning the view. A release-build
CPU profile before this change assigned about 9% of samples to view-set search
and sorting, alongside roughly half to Perlin sampling. This change leaves
terrain values untouched.

On this 20-thread machine, a separate-process release benchmark of 16 cold
one-chunk tasks took 3.36 s at four-way concurrency and 1.96 s at eight-way
concurrency while the local server was also active. This is a throughput
probe, not a same-client latency measurement; it motivated increasing the
opt-in worker pool to eight. An individual chunk still takes hundreds of
milliseconds, so a radius-32 view will continue to fill over time.

## Independent seed check (2 October 2026)

`inspect_vanilla_save` verified an additional owner-local Java 26.3
single-player save with a different seed (kept in private progress notes),
Overworld generator `minecraft:noise`, and settings `minecraft:overworld`.
Its metadata reports
`WasModded=true` and enabled packs `vanilla,fabric-convention-tags-v2`; this
is a default-preset observation from a modded client, not a clean vanilla
fixture. The world remains local and untracked.

On a stride-16 square from `-640..640` on each axis, 6,561 grid points were
requested: 1,662 generated columns, 4,052 missing chunks, and 847 stored but
ungenerated columns. Among the 1,662 scored columns, exact surface height was
1,636 (98.44%), biome identity 1,662 (100%), and top block 1,621 of the 1,636
height-matched columns (99.08%). The full-column base-block comparison scored
213,871 positions: 178,414 exact (83.42%) and 181,341 exact after collapsing
the air family (84.79%). Dominant remaining substitutions were tuff, andesite,
diorite, and granite in the save versus plain deepslate or stone in RustMC.
Only loaded/generated save columns enter the numerator. This is a second-seed
measurement, not proof across arbitrary seeds or vanilla client rendering.

## Ground truth: first block-identity measurement (2 October 2026)

This records automated evidence for the first acceptance item. The item stays
unchecked: the comparison is by base block id, so the exact block-state
comparison it asks for has not been made, and no vanilla-parity or
client-visible claim follows from these numbers.

`vanilla_oracle block_compare` (`crates/rustmc-tools`) scores RustMC's
full-column `vanilla::VanillaGenerator` descent — density, aquifer, material
rules including vein rules, and the registry carvers — against the block names
the owner's Java 26.3 save stores at the same absolute `(x, y, z)` positions,
Overworld only, read-only. Saved rows above the chunk
`MOTION_BLOCKING_NO_LEAVES` height are outside the compared volume.

Sample A is the published seed-2026 grid (stride 16 over `-256..512` on both
axes): 2,401 requested grid points, 2,401 columns read, 0 missing chunks,
0 ungenerated or unscorable columns, 338,217 scored positions — the same
denominator as the T3 substance baseline in `docs/PROVENANCE.md`.

- Exact base-block agreement: 273,498 / 338,217 = **80.86%**. Collapsing the
  air family (`air`, `cave_air`, `void_air`, `structure_void`) to one name:
  279,991 = 82.78%. Substance (air/fluid/solid) agreement on this grid is
  98.64%, so the gap is block identity rather than terrain shape.
- Per 32-block absolute Y band (exact): `-64` 86.61%, `-32` 84.66%, `0` 72.52%,
  `32` 75.61%, `64` 90.35%, `96` 91.55%, `128` 75.93% (54 positions). The
  sea-level bands are the weakest.
- Dominant confusion pairs (save → RustMC): tuff→deepslate 9,804;
  diorite→stone 8,757; andesite→stone 8,393; granite→stone 8,269;
  air→cave_air 6,456; dirt→stone 2,763; air→deepslate 2,493; gravel→stone
  2,291; gravel→deepslate 2,194. The four vein/blob stones alone are 35,223
  positions (10.4% of the sample), which is the placement-stage blob and ore
  runtime already isolated as the T4 target.
- Cave context, keyed on the air rows the save itself stores: saved voids
  28,723 positions (66.16% exact, 88.76% with the air family collapsed), cave
  wall within four rows of a void 28,251 (76.73%), intact rock 281,243 (82.78%).
  Two-thirds of the raw void disagreement is only which air block was written.
  Solid-versus-air: 3,228 void rows the save leaves air that we fill (2,063 in
  the `-64` band and 896 in the `-32` band, matching the deep-carver residual
  attribution of the T3 baseline), and 1,167 rows we open that the save fills —
  652 beside an existing void (over-wide carving) and 515 in intact rock
  (invented caves).

Sample B, a sparser negative-coordinate grid (stride 32 over `-1024..-512`):
289 requested grid points, 61 scored columns, 120 missing chunks, and 108
stored-but-ungenerated columns. The sampler now reports that last case as a
denominator bucket instead of aborting the run, which is what previously made
wide-area measurement impossible. Agreement 85.14% over 12,221 positions, with
the same residual shape: 430 void rows left filled (325 at `-64`) and 20 rows
over-carved.

Caveats: one seed, one default preset, and one hand-travelled world, so chunk
generation states are uneven across the sample; feature- and
structure-placed blocks count as mismatches by design; block-state properties
are stripped from both sides; and no client rendering is involved in these
numbers.
