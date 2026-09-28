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
