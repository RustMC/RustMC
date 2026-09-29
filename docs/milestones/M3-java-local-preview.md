# M3 slice — local Java 26.3 world preview

Status: in development; opt-in login and known-pack exchange were observed with a Java 26.3 client on 29 September 2026. The client then timed out waiting for registry data. This is a Java-only preview slice, not the M3 dual-edition join gate in [ROADMAP](../../ROADMAP.md). The first observable goal is one Java 26.3 client entering a generated world and rendering nearby chunks. A successful status ping or login packet is insufficient.

## Scope and sequence

1. Source packet IDs from the official 26.3 server's generated `reports/packets.json` and version metadata. Treat packet field layouts as provisional until the running Java 26.3 client accepts them. Keep the report and any client traces local; record source and observations publicly without checking in Mojang assets or code.
2. Add an explicitly enabled, loopback-only development offline identity path. It cannot authenticate a Microsoft account or prove player identity. Reject remote binds and unsupported protocol versions. Bound packet sizes, queue depth, work per iteration, lifetime, and view distance.
3. Enter login, configuration, and play in protocol order. Do not emit world readiness until the client has accepted the initial join and chunk stream.
4. Map the independent seeded terrain prototype into 26.3 block/biome/chunk/heightmap/light packets with sourced IDs and independently tested encoders. Load nearby chunks as movement crosses chunk boundaries, with finite pending and retained sets.
5. Verify a real client rendering original chunks, movement into newly generated chunks, and disconnect/rejoin. Measure generation, encoding, queue wait, and delivery separately; leave the visual checklist unchecked without direct evidence.

## Acceptance criteria

- [x] Explicit local development identity setting and clear security warnings; remote access remains disallowed. This is unauthenticated and has no permissions or playable world.
- [ ] Java 26.3 client completes login, configuration, and play transitions with malformed/version mismatch tests.
- [ ] Client visibly renders independently generated grass terrain and trees from a configured seed; chunk borders and negative coordinates are deterministic.
- [ ] Bounded view loading follows movement; shutdown and rejoin preserve correctness in covered cases.
- [ ] Full local checks and GitHub CI pass, with generation/encoding/delivery evidence separated.

Bedrock world join, secure online authentication, vanilla generation/parity, persistence, multiplayer visibility, and release readiness are outside this slice.
