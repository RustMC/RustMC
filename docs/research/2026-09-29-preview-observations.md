# Research note: first Java 26.3 terrain preview

Date: 29 September 2026. Scope: one local Java 26.3 Creative client and RustMC's
pre-alpha, unauthenticated loopback preview. This is **not** a vanilla world or a
server performance comparison.

## What we observed

The owner's [34-second client recording](../../assets/demos/rustmc-java-26.3-preview-2026-09-29.mp4)
shows original terrain appearing during Creative flight, with a visible loading
delay. The [README preview](../../README.md#world-generation-and-client-join-work--29-september-2026)
is an inline animated excerpt of that same recording. The client was Java 26.3;
the server used a configured view radius of 32 chunks. The recording is visual
evidence of rendering, not a timed throughput measurement.

The first stream admitted one chunk and waited for an acknowledgement before
sending the next. RustMC now sends up to 16 chunks per acknowledged batch within
a 768 KiB encoded budget, updates the target center from validated movement,
and forgets coordinates outside the 32-chunk radius. The [bounded-view unit
tests](../../crates/rustmc-server/src/java_preview.rs) check a distant movement,
then a one-chunk move that emits five forget packets and five replacement chunk
packets for a radius-two view. A fresh session reproduces the same first batch.
Real-client disconnect/rejoin evidence is still open. We have **not** measured before/after
client-visible loading time, so no speedup factor is claimed.

The generator currently offers eight independently derived preview surface
regions. Their visual labels do not establish vanilla 26.3 terrain, cave,
structure, biome, or gameplay parity. Bedrock world join and secure Java login
remain absent; see the [compatibility matrix](../COMPATIBILITY.md).

## Version and source trail

- Java 26.3 and protocol 777: [Mojang version manifest](https://piston-meta.mojang.com/mc/game/version_manifest_v2.json),
  [official server archive](https://piston-data.mojang.com/v1/objects/33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c/server.jar),
  and [release notes](https://www.minecraft.net/en-us/article/minecraft-java-edition-26-3).
- Preview packet identifiers and static registry IDs: reports generated locally
  from that official archive; no archive or generated game data is checked in.
  Field behavior was checked against the owner's running Java 26.3 client.
- Biome/terrain context: [Mojang's Overworld generation overview](https://www.minecraft.net/en-us/article/new-world-generation-java-available-testing)
  and [biome overview](https://help.minecraft.net/hc/en-us/articles/360046470431-Minecraft-Types-of-Biomes).
  RustMC's algorithm and code are independent; see [ADR-0013](../decisions/ADR-0013.md).
- Dependency license evidence and limitations: [provenance](../PROVENANCE.md).
- Raw generation, encoding, socket-flush, and startup observations:
  [benchmark notes](../BENCHMARKS.md). Socket flush does not prove client receipt.

## Publishable statement with present evidence

“Today RustMC's local Java 26.3 preview rendered original, seeded terrain in a
real client while flying. I found that waiting for an acknowledgement after
each chunk limited the stream, so the server now sends bounded batches of up
to 16 chunks and unloads chunks that leave its 32-chunk view. Tests and the
client recording show this early slice works; vanilla parity and a measured
before/after speedup are still open.”

Before publishing a speed comparison, repeat controlled tests on the same
machine and client with both builds, fixed seed/path/view settings, separate
generation/encoding/network/client-render timing, and multiple runs. Record
client FPS and stalls as well as server work and correctness.
