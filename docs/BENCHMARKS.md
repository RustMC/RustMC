# Benchmark methodology

M0 has no server-performance results. Later startup reporting must separate `process_started`, `listeners_bound`, `protocol_ready`, `world_ready`, first playable join, first chunk, and dirty/crash recovery. A bound socket is not playable readiness. Compare any generated Boot Image with simple loading, including generation and corruption behavior.

Capacity workloads must include spread-out players, clustered players, movement-heavy sessions, and entity/simulation-heavy worlds. Record hardware, OS, toolchain, build profile, configuration, world seed/data, client mix, warm/cold state, sample count, raw timings, and error/overload behavior. Report latency distribution only with adequate samples; do not infer high percentiles from a handful of trials. Measure tick critical path, barrier wait, queue depth, disk backpressure, first join/chunk latency, and correctness. Replay and partition-independence failures reject an optimization even if throughput improves. Idle bot connections do not prove playable capacity.

## M1 local startup observations (29 September 2026)

These are **development-listener bind timings, not Minecraft server startup or first playable join**. The release binary was built from commit `7df14d7ed9ba161092002815cd8b15e0c212d7fa` with Rust 1.98.1 on Linux 7.0.12-201.fc44.x86_64, x86_64, 13th Gen Intel Core i7-13650HX (20 logical CPUs). Configuration: `config/rustmc.example.toml`, loopback port 0, no clients or world. Build time is separate: Cargo reported 0.44 s for an incremental release rebuild after the last code change; an earlier first release build after M1 code changes took 3.43 s. Neither is a clean-build benchmark.

Method: `cargo build --release --workspace --locked`, then `python3 scripts/measure_startup.py --pairs 5`. Each cycle requests `POSIX_FADV_DONTNEED` for the executable before a `cold_hint` launch, then immediately runs a `warm_repeat` launch. The kernel may retain cached pages, so `cold_hint` is **not a verified cold OS-cache condition**. `code_entry_to_bound_ms` is the monotonic interval from the beginning of Rust `main` to successful local bind, reported by the process. `parent_spawn_to_bound_ms` is measured by Python from just before process creation until its `listener_bound` line is read; it includes process creation, scheduling, and pipe delivery. Each child receives SIGTERM and must emit `stopped` with exit code 0. No high-percentile or capacity claim follows from these ten samples.

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

**Encoding: unavailable. Delivery: unavailable.** Both require a version-correct client join and chunk packet path. Future evidence must report generation, encoding, queue wait, socket delivery, and client-visible timing separately.
