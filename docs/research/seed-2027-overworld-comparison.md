# Seed 2027 Overworld sample — Java 26.3

This is a read-only comparison against an owner-provided Java 26.3 single-player
save. Its `world_gen_settings.dat` reports seed 2027, the default Overworld noise
settings, and the vanilla and Fabric convention-tags packs. The save is marked
modded, so this sample does not prove stock-client equivalence. No world files
are committed or modified by these tools.

The grid contains 2,401 X/Z columns from -384 through 384 on each axis at a
16-block stride. All 2,401 columns were present in the save; no missing or
declined samples were scored as agreement. The RustMC generator used the same
seed and operator-provisioned Java 26.3 worldgen data.

| Measure | Agreement | Denominator |
| --- | ---: | ---: |
| Surface height | 2,375 (98.92%) | 2,401 columns |
| Biome | 2,399 (99.92%) | 2,401 columns |
| Top block, where surface height matched | 2,357 (99.24%) | 2,375 columns |
| Exact sampled base block | 258,769 (83.91%) | 308,379 positions |
| Exact sampled base block, air family collapsed | 262,872 (85.24%) | 308,379 positions |

The block comparison samples selected Y bands in each column; it is not a census
of every block in the region. Missing feature placement, carver differences, and
unimplemented block states remain. This sample says nothing about other seeds,
all biomes, structures, or client-visible chunk loading speed.

The comparison initially failed on chunks at the physical end of several
region files. The reader treated the stored length as excluding the compression
byte; the owner's 26.3 files showed that it includes that byte and can end at
the final payload byte without sector padding. The corrected reader validates
the complete payload against the file and the allocated sector count. A focused
exact-EOF regression test covers the observed layout.

Reproduce with an operator-local save and data root:

```text
vanilla_oracle compare WORLD 2027 -384 384 16 vanilla DATA_ROOT
vanilla_oracle block_compare WORLD 2027 -384 384 16 DATA_ROOT 0
```
