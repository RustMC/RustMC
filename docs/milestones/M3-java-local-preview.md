# M3 slice — local Java 26.3 world preview

Status: local Java preview acceptance criteria met on 29 September 2026; the broader M3 dual-edition join gate remains open. A real Java 26.3 client completed login and configuration, rendered opt-in Creative preview chunks, moved across chunk boundaries, disconnected, and rejoined. This is a Java-only preview slice, not the M3 dual-edition join gate in [ROADMAP](../../ROADMAP.md). A successful status ping or login packet alone is insufficient.

## Scope and sequence

1. Source packet IDs from the official 26.3 server's generated `reports/packets.json` and version metadata. Treat packet field layouts as provisional until the running Java 26.3 client accepts them. Keep the report and any client traces local; record source and observations publicly without checking in Mojang assets or code.
2. Add an explicitly enabled, loopback-only development offline identity path. It cannot authenticate a Microsoft account or prove player identity. Reject remote binds and unsupported protocol versions. Bound packet sizes, queue depth, work per iteration, lifetime, and view distance.
3. Enter login, configuration, and play in protocol order. Do not emit world readiness until the client has accepted the initial join and chunk stream.
4. Map the independent seeded terrain prototype into 26.3 block/biome/chunk/heightmap/light packets with sourced IDs and independently tested encoders. Load nearby chunks as movement crosses chunk boundaries, with finite pending and retained sets.
5. Verify a real client rendering original chunks, movement into newly generated chunks, and disconnect/rejoin. Measure generation, encoding, queue wait, and delivery separately; leave the visual checklist unchecked without direct evidence.

## Acceptance criteria

- [x] Explicit local development identity setting and clear security warnings; remote access remains disallowed. This is unauthenticated and has no permissions or playable world.
- [x] A real Java 26.3 client completes local offline login, configuration, and initial Creative play transition; version mismatch and malformed input have bounded tests. Authentication is absent.
- [x] Client visibly renders original terrain and trees at a configured seed and 32-chunk view; deterministic seed, negative coordinates, and chunk borders have unit tests. Full vanilla generation is absent.
- [x] Bounded view loading follows movement; shutdown and rejoin preserve correctness in covered cases. A real client rendered terrain after moving over 30 chunk widths and after rejoining; unit and process tests cover bounded streaming and shutdown. Full-view completion time is unmeasured.
- [x] Full local checks and GitHub CI pass, with generation, encoding, and socket-flush evidence separated. Socket flush is not proof of client receipt or render timing.

Bedrock world join, secure online authentication, vanilla generation/parity, persistence, multiplayer visibility, and release readiness are outside this slice.

## Configuration evidence — 29 September 2026

The real Java 26.3 client accepted the local registry/tag manifest, acknowledged
finish-configuration, entered the Creative preview, and visibly rendered chunks.
Initial experiments
failed with missing world-clock/timeline references, then missing static block
and item tags. Sending resolved dynamic and static tag IDs fixed that boundary.
See [ADR-0012](../decisions/ADR-0012.md). Full play behavior remains unverified.

The preview now uses eight labelled surface regions with coordinate-derived irregular borders, a 32-chunk maximum view, and Creative observer movement. The 26.3 client displayed grass, snow, sand, and red-sand/terracotta terrain after joining; the world remains read-only. See [ADR-0013](../decisions/ADR-0013.md).

## Movement and rejoin observation — 29 September 2026

In the owner's running Java 26.3 client, an assistant-controlled Creative flight moved from approximately X=96, Z=707 to X=625, Z=779 while new terrain remained visible. The client then disconnected through its Multiplayer menu and rejoined the same loopback server. Its F3 display showed approximately X=0.5, Y=81.0, Z=0.5 with RustMC terrain visible after rejoin. These are direct visual observations from one local client and session. They establish neither completion of all 4,225 view coordinates nor a client-visible loading time. The [research note](../research/2026-09-29-preview-observations.md) records the evidence boundary.
