# Benchmark methodology

M0 has no server-performance results. Later startup reporting must separate `process_started`, `listeners_bound`, `protocol_ready`, `world_ready`, first playable join, first chunk, and dirty/crash recovery. A bound socket is not playable readiness. Compare any generated Boot Image with simple loading, including generation and corruption behavior.

Capacity workloads must include spread-out players, clustered players, movement-heavy sessions, and entity/simulation-heavy worlds. Record hardware, OS, toolchain, build profile, configuration, world seed/data, client mix, warm/cold state, sample count, raw timings, and error/overload behavior. Report latency distribution only with adequate samples; do not infer high percentiles from a handful of trials. Measure tick critical path, barrier wait, queue depth, disk backpressure, first join/chunk latency, and correctness. Replay and partition-independence failures reject an optimization even if throughput improves. Idle bot connections do not prove playable capacity.

## M1 local startup observations (29 September 2026)

These are **development-listener bind timings, not Minecraft server startup or first playable join**. The release binary was built from commit `7df14d7ed9ba161092002815cd8b15e0c212d7fa` with Rust 1.98.1 on Linux 7.0.12-201.fc44.x86_64, x86_64, 13th Gen Intel Core i7-13650HX (20 logical CPUs). Configuration: `config/rustmc.example.toml`, loopback port 0, no clients or world. Build time is separate: Cargo reported 0.44 s for an incremental release rebuild after the last code change; an earlier first release build after M1 code changes took 3.43 s. Neither is a clean-build benchmark.

Historical method: the M1 measurement script requested `POSIX_FADV_DONTNEED` for the executable before each `cold_hint` launch, then immediately ran a `warm_repeat` launch. The kernel may retain cached pages, so `cold_hint` is **not a verified cold OS-cache condition**. `code_entry_to_bound_ms` is the monotonic interval from the beginning of Rust `main` to successful local bind, reported by the process. `parent_spawn_to_bound_ms` was measured by the parent process from just before process creation until its `listener_bound` line was read; it includes process creation, scheduling, and pipe delivery. Each child received SIGTERM and exited cleanly. No high-percentile or capacity claim follows from these ten samples. The historical script was retired when developer tools moved to Rust; do not treat the new method as a reproduction of its cache hint.

For new warm-only samples, run `cargo build --release --workspace --locked`, then `cargo run -p rustmc-tools --bin measure_startup --locked -- --pairs 5`. This Rust tool emits first/warm-repeat pairs without a cache-drop request. Record its raw output and host details separately before comparing runs.

| Cycle | Mode | Cache-drop requested | Code entry to bind (ms) | Parent spawn to bound (ms) |
| --- | --- | --- | ---: | ---: |
| 1 | cold_hint | yes | 0.100 | 0.715 |
| 1 | warm_repeat | no | 0.115 | 0.727 |
| 2 | cold_hint | yes | 0.083 | 0.594 |
| 2 | warm_repeat | no | 0.092 | 0.638 |
| 3 | cold_hint | yes | 0.088 | 1.532 |
| 3 | warm_repeat | no | 0.086 | 0.702 |
| 4 | cold_hint | yes | 0.097 | 4.136 |
| 4 | warm_repeat | no | 0.080 | 0.865 |
| 5 | cold_hint | yes | 0.079 | 4.369 |
| 5 | warm_repeat | no | 0.100 | 0.786 |

The large difference in parent-observed times reflects process/page-cache and host noise, not game readiness. Future measurements must add actual protocol and world readiness, client join, first chunk, dirty recovery, and representative workloads before any server-performance claim.

## Original terrain prototype observations (29 September 2026)

These numbers measure **generation only** for the independent `Generator` prototype; no Minecraft chunk encoding, network delivery, lighting, client rendering, or disk I/O exists in this path. On the same Fedora host and Rust 1.98.1 toolchain described above, `cargo build --release -p rustmc-server --example measure_generation --locked` reported 0.12 s for the final incremental example rebuild (an earlier build reported 0.44 s). Those build durations are separate from the measurements. Then `target/release/examples/measure_generation 2026 25` generated 25 adjacent 16×16×128 chunks in process order from coordinates (-5,-5) through (-1,-3), with seed 2026. Each number is the monotonic interval around one call to `Generator::generate`, including its block allocation; `black_box` keeps the complete result observable to the optimizer. No cache control, repetitions, or load isolation was attempted; these are raw development observations, not a throughput or latency claim.

| Chunk coordinate order | Generation time (microseconds) |
| --- | --- |
| (-5,-5) through (4,-5) | 46, 24, 25, 24, 24, 24, 23, 23, 24, 23 |
| (-5,-4) through (4,-4) | 22, 23, 23, 22, 21, 22, 21, 21, 22, 23 |
| (-5,-3) through (-1,-3) | 21, 22, 23, 23, 23 |

**Encoding and delivery were unavailable for this prototype measurement.** Later preview observations are recorded below.

## Java 26.3 local Creative preview observations (29 September 2026)

A real Java 26.3 client joined the opt-in loopback preview with seed 2026 and
server view radius 32. The development build was used; the client was also
rendering and other desktop work was running. RustMC logged separate monotonic
intervals for the first ten generated chunks. `generation_us` spans the pure
terrain call; `encoding_us` spans protocol encoding; `queue_and_write_us`
starts after encoding and ends when bytes are handed to the local TCP socket.
Socket write completion is **not** client receipt, rendering, or a network
latency measurement. There was no cache control or repetition, so these are
raw diagnostic samples, not a startup, throughput, speed, or capacity claim.

| Stage (microseconds, first ten chunks) | Raw observations |
| --- | --- |
| Generation | 1218, 1005, 1194, 1198, 772, 1228, 700, 756, 763, 705 |
| Encoding | 9821, 12849, 12713, 10884, 8311, 9663, 7961, 8069, 8100, 9126 |
| Queue plus socket write | 84, 106, 116, 88, 96, 93, 73, 79, 81, 100 |

The user and agent visually observed chunks and several biome surfaces, but no
client timestamp or first-render interval was captured. The current encoder
sends direct vertical skylight and has no full light propagation. A proper
performance study must use a release build, repeated runs, controlled client
movement, profiling, and end-to-end delivery timestamps before comparison.

## Data-driven Overworld generation speed (19 November 2026)

The September client recording above used the original preview terrain,
which generates a chunk in about a millisecond. The opt-in data-driven
Overworld generator was far slower: in a real Java 26.3 session with eight
generation workers the server logged a median of 6.7 s (maximum 15 s) of
`generation_us` per chunk, so a radius-32 view could not fill in play.

Method: release build of `bench_vanilla_chunks`, seed 2026, the default 4x4
chunk square at origin, one pass with a freshly constructed generator (cold
caches), single thread, operator-provisioned 26.3 data root. The machine is a
shared 20-thread desktop; each before/after pair was run back to back, so the
numbers are diagnostic, not a capacity claim. A CPU profile (`perf`) of the
full-chunk mode guided each change.

| Mode (ms per chunk) | Before | After |
| --- | --- | --- |
| `columns` (full chunk) | 2,795 | 509 |
| `substance` | 822 | 162 |
| `heights` | 381 | 72 |

What changed:

- `minecraft:cache` density nodes now memoise values per column when the
  wrapped function provably does not depend on `y` (zero-`y_scale` noise,
  y-slices, the horizontal shift noises, `find_top_surface`, and expressions
  built only from those). Before, the climate and terrain-shaping trees were
  re-evaluated for every block of a column. The memo key keeps the sign of
  `y`, because a zero-`y_scale` noise multiplies `y` by `0.0`, which is `-0.0`
  below zero.
- Biome selection searches a bounding-box tree over the 7,594-row
  multi-noise table instead of scanning every row. Subtrees are skipped only
  when their distance lower bound proves no row inside can beat the current
  best `(fitness, row)`, so ties still resolve to the earlier table row.
- One libm `floor` call per smeared noise sample became an exact
  truncate-and-correct conversion.

Output is unchanged: 81 chunk packets around spawn (radius 4) produced by
the new build are byte-identical to the previous build's cached packets; a
unit test and an ignored data-root test compare the tree search with the
full scan on 5,000 synthetic and 50,000 random targets against the vanilla
table; all ignored data-root tests pass. Still open: the remaining time is
spread between Perlin sampling (about half) and biome search (about a
third); end-to-end client delivery timings are not yet recorded.

### Per-chunk density volume fill and local hot caches

Method: same single-threaded release measurement as above (seed 2026, cold
first pass on a freshly constructed generator, 4×4 chunk square at origin plus
one isolated chunk at `(500, 500)`).

| Mode (ms per chunk) | Before | After per-chunk density fill | After climate fill + local aquifer cache |
| --- | ---: | ---: | ---: |
| `columns` (4×4) | 528.06 | 260.96 | 221.55 |
| `substance` (4×4) | 178.52 | 26.24 | 17.15 |
| `heights` (4×4) | 73.68 | 17.72 | 12.20 |
| `columns` isolated `(500, 500)` | 3,233.18 | 1,027.94 | 901.95 |

What changed:

- `VanillaGenerator` now fills `final_density` once per chunk (`16 × height × 16`)
  via `Density::sample_volume` into a bounded 16-chunk (two-generation, 32-entry
  / 12 MiB cap) cache, and all per-chunk column readers (`base_column_ids`,
  `surface`, `top_block`, `substance`, `terrain_top`, `terrain_top_exhaustive`)
  index into that array instead of calling `Density::sample` block-by-block.
- `Density::sample_volume` also dispatches `Unary`, `Clamp`, `BlendDensity`,
  `RangeChoice`, and `y`-independent leaves, and `fill_interpolated_cell`
  evaluates in `lerp3` order so volume-filled `final_density` values are
  bit-identical to point samples.
- `chunk_biomes` fills the six climate densities over a chunk's `4 × (height/4) × 4`
  quart grid via `sample_volume` and populates the quart biome cache column by
  column (`y` innermost).
- `NoiseBasedAquifer` keeps a single-chunk flat `3 × 48 × 3` cell-center and
  fluid-status array plus cached skip bound so per-block queries avoid repeated
  hash lookups while keeping the cross-chunk `BoundedCache` stores intact.

Evidence: 81/81 chunk packets around spawn (radius 4) are byte-identical to
the baseline build, oracle `substance` and `census_compare` outputs against the
seed-2026 save are byte-identical, and all 63 ignored data-root tests pass
(including bit-identical volume-vs-point `final_density` and climate-target
checks over four chunks). In a CPU profile of `columns`, the remaining time is
dominated by `BiomePlacement::search` (58.51 %), `PerlinNoise` sampling (17.06 %
across `sample_and_lerp`, `get_smeared`, and `get`), and `Density::sample`
(5.52 %); a flat compiled density program was skipped because `Density::sample`
dispatch is below 10 %.

### Column-coherent biome search

Method: same single-threaded release measurement as above (seed 2026, cold
first pass on a freshly constructed generator, 4×4 chunk square at origin plus
one isolated chunk at `(500, 500)`, operator-provisioned 26.3 data root),
comparing the pre-change baseline against the updated build.

| Mode (ms per chunk) | Before | After |
| --- | ---: | ---: |
| `columns` (4×4) | 241.27 | 169.78 |
| `substance` (4×4) | 15.65 | 16.13 |
| `heights` (4×4) | 10.64 | 12.15 |
| `columns` isolated `(500, 500)` | 862.87 | 714.36 |

What changed:

- `BiomePlacement::find_column` groups each quart column in
  `VanillaGenerator::chunk_biomes` into runs whose five non-depth quantized
  targets (`temperature`, `humidity`, `continentalness`, `erosion`, `weirdness`)
  are equal (`fixed_run_len`), computes each row's fixed-part fitness `A_i`
  once per run, partitions the `COLUMN_SCAN_FALLBACK_ROWS = 1024` smallest
  `(A_i, row)` pairs with `select_nth_unstable_by` (tuned across
  `128..2048` by measurement), and sorts only that prefix by `(A_i, row)`
  ascending so ties keep the earlier table row.
- Each `BiomePlacement` precomputes the sorted, merged, disjoint union of all
  rows' `depth` intervals (`depth_ranges`) so `min_depth_fitness` yields the
  exact lower bound `D(t4) = min_i distance_depth(t4)^2` in `O(log |depth_ranges|)`
  (0 inside any interval). The column scan stops as soon as
  `A_i + D(t4) > best_fitness` (or `A_i + D(t4) == best_fitness` with
  `row > best_row`), avoiding full-table scans at negative depth targets when
  all placement rows have `depth.min >= 0`.
- When a cell has not terminated after examining `COLUMN_SCAN_FALLBACK_ROWS`
  rows and the `(K + 1)`-th row's bound does not prove termination,
  `find_column` stops the scan and resolves that cell through the bounding-box
  tree (`find_row_with_seed`, seeded with the better of the scanned prefix
  winner and `last_winner`), updating `last_winner` across both paths to keep
  the vertical warm start tight.
- Reusable scratch buffers (`ColumnSearchScratch` inside `ClimateScratch`)
  keep the 7,594-row order buffer across runs and chunks without per-run or
  per-cell allocations, while `BiomePlacement::find` retains the bounding-box
  tree for out-of-grid point lookups (`biome(x, z, y)` misses, point queries,
  and tests).

Evidence: 289/289 chunk packets around spawn (radius 8) are byte-identical to
the baseline build, oracle `census_compare` and `biome_volume` outputs against
the seed-2026 save are byte-identical, all 975 workspace tests pass (with 64
ignored), and all 64 ignored data-root tests pass in release mode (including
exact `find_column == find == brute force` checks across all 12,288 quart
cells of 8 chunks, 3,000 random synthetic runs with deliberate ties and
mid-column splits, 2,000 negative-depth runs above non-negative depth
intervals, a fallback-limit test verifying winners beyond
`COLUMN_SCAN_FALLBACK_ROWS`, and a quantized overflow-bound check asserting
`|A_i| + max_depth_term <= 7.27e12` against `i64::MAX`, with an observed
maximum of `1,516,900,650` across the 8 chunks). Compared with the baseline,
`columns` (4×4) improves by 29.6 % (`241.27` to `169.78 ms/chunk`) and
`columns` isolated `(500, 500)` improves by 17.2 % (`862.87` to
`714.36 ms/chunk`). In a CPU profile of `columns` (2×2 square),
`BiomePlacement::search` (cache-miss `biome` lookups plus tree fallbacks
beyond 1,024 rows) accounts for 28.58 %, `PerlinNoise` sampling accounts for
26.09 % (`15.86 %` `sample_and_lerp`, `8.37 %` `get_smeared`, `1.86 %` `get`),
`Density::sample` accounts for 7.49 %, `NoiseBasedAquifer::substance` accounts
for 4.89 %, `BiomePlacement::find_column` accounts for 4.04 %,
`FeatureData::run_chain` accounts for 3.84 %, `partition_at_index` plus
`small_sort_general` accounts for 3.58 % (`2.29 %` + `1.29 %`),
`sample_interpolated_block_step` accounts for 2.68 %, and
`Density::sample_volume` accounts for 2.42 %.

### Shared cross-worker chunk products (19 November 2026)

Throughput method: release build of `bench_vanilla_chunks --mode throughput --workers 16 --radius 8`, seed 2026, `minecraft:overworld`, operator-provisioned 26.3 data root and chunk registry table. Sixteen worker threads—each owning a `VanillaGenerator` and `RegistryTables` compiled before the timer starts—pull chunk positions from one shared queue in `java_preview::ordered_view` order around `(0, 0)` with direction `(0, 0)` over the radius-8 square (`17 × 17 = 289` chunks) and run `chunk_from_generator` plus `encode_chunk` cold with no packet cache. Single-thread `columns` mode (`--repeat 1`) was also measured back to back at `(500, 500)` (`1 × 1`) and `(0, 0)` (`4 × 4`).

| Workload | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| `throughput` (16 workers, radius 8, chunks/s) | 27.52 | 115.44 | 4.20× |
| `throughput` elapsed time (289 chunks, ms) | 10,503.0 | 2,503.4 | 4.20× |
| `throughput` per-chunk `p50` (ms) | 569.6 | 126.7 | 4.50× |
| `throughput` per-chunk `p95` (ms) | 815.0 | 208.2 | 3.91× |
| `throughput` per-chunk `max` (ms) | 942.6 | 377.1 | 2.50× |
| `columns` isolated `(500, 500)` (ms/chunk) | 746.10 | 651.94 | 1.14× |
| `columns` `4 × 4` at `(0, 0)` (ms/chunk) | 170.06 | 100.54 | 1.69× |

On a repeat back-to-back `throughput` pass after the final edit, the updated build measured `111.46 chunks/s` (`2,592.9 ms` total, `p50 = 128.9 ms`, `p95 = 217.4 ms`, `max = 334.5 ms`), and peak RSS across the 16 workers dropped from `595,336 KiB` before to `544,560 KiB` (`537,248 KiB` on the repeat) after.

What changed:

- Added `ShardedBoundedCache` (`32` shards, two-generation FIFO eviction, shard locks released before any computation) and `SharedChunkCaches` (`Arc`-shared across `VanillaWorker::new` workers and `bench_vanilla_chunks --mode throughput`, with a private instance for single-generator tools and tests). Total worst-case shared capacity across all eight caches is bounded at roughly `157 MiB`: `chunk_density` (`192` chunks / `144 MiB` max), `masks` (`256` chunks / `~6 MiB` max), `chunk_biomes` (`512` chunks / `~3.5 MiB` max), `ocean_floor_maps` (`512` chunks / `1 MiB` max), `surface_height_maps` (`512` chunks / `1 MiB` max), `biomes` point cache (`8,192` cells / `~512 KiB` max), `ocean_floor_cols` (`16,384` columns / `~512 KiB` max), and `surface_heights` (`16,384` columns / `~512 KiB` max).
- Each generator keeps its lock-free thread-local L1 `BoundedCache`s and `Rc`-based density trees while consulting `SharedChunkCaches` on L1 misses before computing.
- `ChunkBiomes` stores the `4 × (height/4) × 4` `u16` quart palette slots alongside the sorted `region` slice (`Arc<[Arc<str>]>`) so `VanillaGenerator::biome` serves in-range chunk queries in `O(1)`, and `build_decorated_chunk` populates `chunk_biomes` before `base_column_ids_with_heights` and publishes completed `256`-column `surface_height_maps` and `ocean_floor_maps` once the base columns finish.

Evidence and remaining profile: `81/81` chunk packets around spawn (`prepare_preview_cache`, seed `2026`, radius `4`) are byte-identical to the `upstream/main` golden set; the ignored `shared_worker_caches_encode_identically_to_private_generator` test confirms `8` shared-cache workers produce byte-identical packets across a `5 × 5` square (`25` chunks) compared with a private `VanillaGenerator::new`; all `978` non-ignored workspace tests pass (`65` ignored), and all `64` ignored data-root tests pass in release mode (the one remaining ignored test requires the operator's local 26.3 save). Before this change, a CPU profile of the 16-worker throughput run (`218B` cycles) showed `44.78 %` in `chunk_density` (driven by neighbour `ocean_floor_height` -> `surface` lookups at `53.67 %` inclusive), `19.16 %` in `BiomePlacement::search`, `14.23 %` in `BiomePlacement::find_column`, `7.32 %` in `NoiseBasedAquifer::substance`, and `3.62 %` in `VanillaGenerator::carved`. After sharing (`52.5B` cycles), the remaining time is spread across `chunk_density` (`31.16 %`, mainly outer halo chunks and startup races on `ocean_floor_height`), `FeatureData::run_chain` (`20.76 %`), `base_column_ids_with_heights` (`16.38 %`, split across `eval_rule` `9.53 %` and `NoiseBasedAquifer::substance` `7.82 %`), `chunk_biomes` (`8.48 %`), and `chunk_adapter` state lookup and packet encoding (`RegistryTables::state` `6.61 %`, `encode_chunk` `6.12 %`).
