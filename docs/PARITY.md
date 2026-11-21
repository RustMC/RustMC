# Vanilla parity

How closely each RustMC feature behaves like vanilla Java Edition 26.3, and the
evidence. "Matches" is used only with a measured match rate; a feature without
a recorded comparison is "not started", whatever code exists.

Comparisons use the official 26.3 data reports and data-generator output, and
observations of real vanilla worlds; the methods and full numbers are in
[PROVENANCE.md](PROVENANCE.md) and [BENCHMARKS.md](BENCHMARKS.md).

| Feature | Status | Evidence | Known differences |
| --- | --- | --- | --- |
| Overworld terrain shape (opt-in vanilla generator) | partial | One seed-2026 vanilla save, 2,401 sampled columns: exact surface height 96.00%, biome 99.92%, top block 95.44%; 3D substance 98.61% of 347,483 sampled positions (PROVENANCE.md sessions 5 to 13) | One seed and one preset measured; structures are not generated |
| Ores and stone blobs (opt-in vanilla generator) | partial | Aggregate per-family block counts against the same save (PROVENANCE.md sessions 10 to 13) | Measured as counts per family, not position by position |
| Ground vegetation (opt-in vanilla generator) | partial | Per-family counts against the same save (PROVENANCE.md, simple Overworld ground vegetation) | Trees are not placed, so ground plants exceed vanilla counts where trees would stand |
| Default terrain of the playable server | not applicable | Original coordinate-derived terrain, not a vanilla imitation | Intentionally different from vanilla |
| Items and data components | not started | Item stacks and the slot codec exist; no comparison recorded | |
| Crafting, smelting, brewing | not started | Removed in Phase 0 until rebuilt against live play ([REVIEW.md](REVIEW.md)) | |
| Food, health, damage | not started | | |
| Tools and block drops | not started | | |
| Enchantments | not started | | |
| Loot tables | not started | Removed in Phase 0 ([REVIEW.md](REVIEW.md)) | |
| Mob and animal AI, spawning | not started | Removed in Phase 0 ([REVIEW.md](REVIEW.md)) | |
| Fluids | not started | Removed in Phase 0 ([REVIEW.md](REVIEW.md)) | |
| Redstone | not started | | |
| Commands | not started | Parsing only; see [COMMANDS.md](COMMANDS.md) | |
