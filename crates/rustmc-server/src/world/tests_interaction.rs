//! Test-only gameplay-envelope probes over the existing `world` seams.
//!
//! Nothing in this file is production behavior: it is compiled only for tests
//! and adds no public API. It exists so the first authoritative
//! block-interaction slice (see
//! `docs/milestones/M3-authoritative-block-interaction.md`) starts from pinned,
//! executable facts about the terrain the current preview already serves: which
//! cells a standing envelope may occupy, whether every sampled column has
//! continuous support down to bedrock, and how a world coordinate maps to
//! exactly one chunk cell (including the negative quadrants and the chunk
//! border).
//!
//! The envelope numbers here are RustMC's *proposed* whole-cube approximation,
//! not a verified vanilla 26.3 mechanic. The 0.6-block width and 1.8-block
//! height are the player hitbox figures documented at
//! <https://minecraft.wiki/w/Player> (checked 2 October 2026). Per-block
//! collision shapes, leaves behavior, stairs, slabs, and fluid interaction are
//! recorded as OBSERVE tasks in that milestone document and are deliberately
//! not asserted here. Under [ADR-0014](../../../../docs/decisions/ADR-0014.md)
//! nothing was copied or translated from another server; these probes are
//! independently written checks against RustMC's own generator.

use super::{Block, CHUNK_SIDE, CellChange, Chunk, Generator, WORLD_HEIGHT};
use std::collections::BTreeMap;

/// Proposed player envelope in blocks, whole-cube approximation.
const PLAYER_WIDTH: f64 = 0.6;
const PLAYER_HEIGHT: f64 = 1.8;
/// Descent bound: the loaded preview index space is `WORLD_HEIGHT` cells tall,
/// so a full top-to-bottom sweep needs at most that many steps plus one.
const MAX_DESCENT_STEPS: u32 = WORLD_HEIGHT as u32 + 2;

/// Half the envelope width, used to center the AABB on a coordinate.
fn half_width() -> f64 {
    PLAYER_WIDTH / 2.0
}

/// Whether a block stops the body span. Air is the only cell a body may share,
/// so leaves, snow, and logs all block it. This is a RustMC envelope decision,
/// not a vanilla claim.
fn body_blocked(block: Option<Block>) -> bool {
    !matches!(block, Some(Block::Air))
}

/// Whether the feet may rest on this cell. Under the whole-cube approximation
/// any non-air, addressable block below the envelope supports it.
fn foot_supported(block: Option<Block>) -> bool {
    block.is_some() && !matches!(block, Some(Block::Air))
}

/// Integer block cells whose interior the half-open interval `[lo, hi)` crosses.
fn crossed_cells(lo: f64, hi: f64) -> Vec<i64> {
    let first = lo.floor();
    let last = if (hi - hi.floor()).abs() < 1e-9 {
        hi.floor() - 1.0
    } else {
        hi.floor()
    };
    (first as i64..=last as i64).collect()
}

/// Preview terrain assembled from the existing public seams:
/// `Generator::generate` plus `Chunk::block`. Coordinates are world block
/// coordinates in the preview's own index space; the world-Y mapping is a
/// decision recorded in the milestone document.
struct Columns {
    chunks: BTreeMap<(i32, i32), Chunk>,
}

impl Columns {
    /// Loads every chunk whose coordinates fall in the closed ranges.
    fn load(generator: Generator, x_range: (i32, i32), z_range: (i32, i32)) -> Self {
        let mut chunks = BTreeMap::new();
        for z in z_range.0..=z_range.1 {
            for x in x_range.0..=x_range.1 {
                chunks.insert((x, z), generator.generate(x, z));
            }
        }
        Self { chunks }
    }

    /// Maps a world block coordinate to the single chunk cell that owns it, or
    /// `None` when the chunk is not loaded or the coordinate is outside the
    /// loaded index space. The authoritative pipeline needs exactly this
    /// rejection rather than a defaulted air read.
    fn block(&self, x: i64, y: i64, z: i64) -> Option<Block> {
        let side = CHUNK_SIDE as i64;
        let owner_x = i32::try_from(x.div_euclid(side)).ok()?;
        let owner_z = i32::try_from(z.div_euclid(side)).ok()?;
        let chunk = self.chunks.get(&(owner_x, owner_z))?;
        if !(0..WORLD_HEIGHT as i64).contains(&y) {
            return None;
        }
        chunk.block(
            x.rem_euclid(side) as usize,
            y as usize,
            z.rem_euclid(side) as usize,
        )
    }

    /// Whether the body span with feet at `feet` is free of blocks.
    fn body_clear(&self, center_x: f64, center_z: f64, feet: f64) -> bool {
        let offset = half_width();
        for cx in crossed_cells(center_x - offset, center_x + offset) {
            for cz in crossed_cells(center_z - offset, center_z + offset) {
                for cy in crossed_cells(feet, feet + PLAYER_HEIGHT) {
                    if body_blocked(self.block(cx, cy, cz)) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Whether at least one cell under the footprint rests on a block.
    fn has_support(&self, center_x: f64, center_z: f64, feet: f64) -> bool {
        let offset = half_width();
        let below = feet.floor() as i64 - 1;
        for cx in crossed_cells(center_x - offset, center_x + offset) {
            for cz in crossed_cells(center_z - offset, center_z + offset) {
                if foot_supported(self.block(cx, below, cz)) {
                    return true;
                }
            }
        }
        false
    }

    /// Highest standable feet level at or below `start`, found by descending one
    /// block at a time within a bounded step count. `None` means nothing
    /// standable exists inside the bound, which the pipeline must reject.
    fn stand_highest(&self, center_x: f64, center_z: f64, start: f64) -> Option<f64> {
        let mut feet = start.floor();
        for _ in 0..MAX_DESCENT_STEPS {
            if feet < 0.0 {
                return None;
            }
            if self.body_clear(center_x, center_z, feet)
                && self.has_support(center_x, center_z, feet)
            {
                return Some(feet);
            }
            feet -= 1.0;
        }
        None
    }
}

/// Surface top for a world column in the preview index space.
fn surface(generator: Generator, x: i64, z: i64) -> i64 {
    generator.height(x, z)
}

/// One sampled column center: the descent walk must return a supported,
/// body-clear position, and repeating it must return the same position.
fn stand_in(columns: &Columns, x: i64, z: i64) -> i64 {
    let center_x = x as f64 + 0.5;
    let center_z = z as f64 + 0.5;
    let sky = WORLD_HEIGHT as f64 - 1.0;
    let feet = columns
        .stand_highest(center_x, center_z, sky)
        .unwrap_or_else(|| panic!("no standable position in column ({x}, {z})"));
    assert!(
        columns.body_clear(center_x, center_z, feet),
        "body span must be clear at ({x}, {z}) feet {feet}"
    );
    assert!(
        columns.has_support(center_x, center_z, feet),
        "feet must be supported at ({x}, {z}) feet {feet}"
    );
    assert_eq!(
        Some(feet),
        columns.stand_highest(center_x, center_z, sky),
        "standing check must be deterministic at ({x}, {z})"
    );
    feet as i64
}

#[test]
fn standing_envelope_agrees_with_the_surface_rule_on_bare_ground_and_lifts_on_canopy() {
    let generator = Generator::new(2026);
    // Chunk square (0, 0) to (2, 2) at this seed carries no above-surface
    // terrain, so the resting level must agree with the height rule exactly: a
    // spawn placed from `Generator::height` is already a valid standing
    // position, and only a changed block may move it.
    let bare_area = Columns::load(generator, (0, 2), (0, 2));
    let mut sampled = 0usize;
    for z in (0..48).step_by(3) {
        for x in (0..48).step_by(3) {
            let top = surface(generator, x, z);
            let feet = stand_in(&bare_area, x, z);
            assert_eq!(
                feet,
                top + 1,
                "bare column ({x}, {z}) rests at {feet}, surface top is {top}"
            );
            sampled += 1;
        }
    }
    assert_eq!(sampled, 16 * 16);

    // A forest strip of the same seed (world z 224..256, observed tree trunks at
    // (3, 229), (19, 229), (21, 235), and (27, 235)) puts trunks and canopy above
    // the surface. The same walk must still land on a supported position, and at
    // least one column must be lifted above its bare surface, which is what the
    // later collision rules have to reproduce.
    let forest = Columns::load(generator, (0, 2), (14, 16));
    let mut lifted = 0usize;
    let mut sampled = 0usize;
    for z in 224..256 {
        for x in 0..32 {
            let top = surface(generator, x, z);
            let feet = stand_in(&forest, x, z);
            assert!(
                feet > top,
                "column ({x}, {z}) rests at {feet}, at or below its surface top {top}"
            );
            // Canopies stack: a neighbor tree's leaves can overhang a column with
            // a lower surface, so the climb bound is generous rather than exact.
            assert!(
                feet - top <= 16,
                "lift {} at ({x}, {z}) exceeds the bounded canopy climb",
                feet - top
            );
            sampled += 1;
            if feet > top + 1 {
                lifted += 1;
            }
        }
    }
    assert_eq!(sampled, 32 * 32);
    assert!(
        lifted > 0,
        "no sampled forest column was lifted above its bare surface"
    );
}

#[test]
fn envelope_footprint_crosses_chunk_borders_and_rejects_unaddressable_targets() {
    let generator = Generator::new(2026);
    let columns = Columns::load(generator, (0, 1), (0, 1));
    // A center at x = 15.8 puts the 0.6-wide envelope across local cells 15 and
    // 16, which belong to chunk (0, 0) and chunk (1, 0). The standing check must
    // consult both chunks and still land on terrain. An envelope that only
    // touches a cell face (center 15.7, max edge exactly 16.0) does not overlap
    // it, which is the convention the collision rules must state.
    let center_x = 15.8;
    let center_z = 0.5;
    assert_eq!(
        crossed_cells(center_x - half_width(), center_x + half_width()),
        vec![15, 16]
    );
    assert_eq!(crossed_cells(15.4, 16.0), vec![15]);
    let top = surface(generator, 15, 0).max(surface(generator, 16, 0));
    let feet = columns
        .stand_highest(center_x, center_z, WORLD_HEIGHT as f64 - 1.0)
        .expect("border column pair must be standable");
    assert!(
        feet as i64 > top,
        "border standing position {feet} must be above the higher neighbor surface {top}"
    );

    // Exactly one chunk cell owns each world coordinate: local index 16 is not
    // addressable inside chunk (0, 0), and out-of-space reads are `None`.
    let left = generator.generate(0, 0);
    assert_eq!(left.block(CHUNK_SIDE, 60, 0), None);
    assert_eq!(left.block(0, WORLD_HEIGHT, 0), None);
    assert_eq!(columns.block(0, -1, 0), None);
    assert_eq!(columns.block(0, WORLD_HEIGHT as i64, 0), None);
    // A coordinate in a chunk that was never loaded is a reject, not air.
    assert_eq!(columns.block(-1, 60, 0), None);
    // World column 15 and world column 16 are different cells in different
    // chunks; each is addressable only through its own mapping, and each is
    // solid at its own surface with open sky above it.
    let right = generator.generate(1, 0);
    let left_top = surface(generator, 15, 0);
    let right_top = surface(generator, 16, 0);
    assert!(left.block(15, left_top as usize, 0).is_some());
    assert_ne!(left.block(15, left_top as usize, 0), Some(Block::Air));
    assert_ne!(right.block(0, right_top as usize, 0), Some(Block::Air));
    assert_eq!(left.block(15, (left_top + 1) as usize, 0), Some(Block::Air));
    assert_eq!(
        right.block(0, (right_top + 1) as usize, 0),
        Some(Block::Air)
    );
}

#[test]
fn sampled_preview_columns_are_solid_from_the_surface_down_to_bedrock() {
    let generator = Generator::new(2026);
    let mut checked_cells = 0usize;
    for (chunk_x, chunk_z) in [(0, 0), (-1, 0), (0, -1), (-1, -1), (2, 1)] {
        let chunk = generator.generate(chunk_x, chunk_z);
        for local_z in 0..CHUNK_SIDE {
            for local_x in 0..CHUNK_SIDE {
                let world_x = i64::from(chunk_x) * CHUNK_SIDE as i64 + local_x as i64;
                let world_z = i64::from(chunk_z) * CHUNK_SIDE as i64 + local_z as i64;
                let top = surface(generator, world_x, world_z) as usize;
                assert_eq!(
                    chunk.block(local_x, 0, local_z),
                    Some(Block::Bedrock),
                    "column ({world_x}, {world_z}) has no bedrock floor"
                );
                // Continuous support: no air pocket between the surface and the
                // floor, so a position the standing walk reports is reachable
                // and breaking a block must not leave a floating column.
                for y in 1..=top {
                    assert_ne!(
                        chunk.block(local_x, y, local_z),
                        Some(Block::Air),
                        "air pocket at ({world_x}, {y}, {world_z})"
                    );
                    checked_cells += 1;
                }
                assert_eq!(
                    chunk.block(local_x, WORLD_HEIGHT - 1, local_z),
                    Some(Block::Air),
                    "column ({world_x}, {world_z}) is not capped by open sky"
                );
            }
        }
    }
    assert!(checked_cells > 10_000);
}

/// Reads at most one region header; never loads whole multi-megabyte files.
fn region_header(path: &std::path::Path) -> Vec<u8> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(8192).read_to_end(&mut bytes))
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    assert_eq!(
        bytes.len(),
        8192,
        "a region header is two 4 KiB tables: {}",
        path.display()
    );
    bytes
}

/// Characterization smoke against the operator's own seed-2026 26.3 save, read
/// strictly read-only and never committed. Same operator-data pattern as the
/// `RUSTMC_VANILLA_DATA` smokes: run with
/// `RUSTMC_VANILLA_SAVE=<world directory> cargo test -p rustmc-server --lib -- --ignored`.
///
/// It pins only the publicly documented on-disk facts the persistence and
/// rejoin criteria depend on: the 26.3 per-dimension region layout, the region
/// location-table entry for the chunk cell our coordinate mapping selects, and
/// sector alignment. See <https://minecraft.wiki/w/Region_file_format>.
#[test]
#[ignore = "requires the operator-provisioned local 26.3 save"]
fn smoke_operator_save_layout_matches_the_chunk_coordinate_mapping() {
    use std::io::{Read, Seek, SeekFrom};
    let root = std::env::var("RUSTMC_VANILLA_SAVE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(".rustmc-local/vanilla-save"));
    let region_dir = root.join("dimensions/minecraft/overworld/region");
    assert!(
        region_dir.is_dir(),
        "26.3 saves keep overworld region files under dimensions/minecraft/overworld/region"
    );

    // Around the preview spawn area: each chunk coordinate must land in the
    // region file and location-table slot that the operator's own save actually
    // uses, under the same euclidean mapping `Columns::block` applies. Truncating
    // division would name a different file or slot and find nothing stored.
    let cases: [(i32, i32, (i32, i32), usize); 5] = [
        (0, 0, (0, 0), 0),
        (-1, -1, (-1, -1), 31 + 31 * 32),
        (1, -1, (0, -1), 1 + 31 * 32),
        (-1, 32, (-1, 1), 31),
        (15, 15, (0, 0), 15 + 15 * 32),
    ];
    for (chunk_x, chunk_z, region, slot) in cases {
        assert_eq!(chunk_x.div_euclid(32), region.0);
        assert_eq!(chunk_z.div_euclid(32), region.1);
        assert_eq!(
            (chunk_x.rem_euclid(32) + chunk_z.rem_euclid(32) * 32) as usize,
            slot
        );
        let path = region_dir.join(format!("r.{}.{}.mca", region.0, region.1));
        let bytes = region_header(&path);
        let location = i32::from_be_bytes(bytes[slot * 4..slot * 4 + 4].try_into().unwrap());
        assert_ne!(
            location,
            0,
            "{} stores no chunk {chunk_x},{chunk_z} at slot {slot}",
            path.display()
        );
        let sector = (location >> 8) as u64;
        let run = (location & 0xff) as u64;
        assert!(
            sector >= 2,
            "chunk {chunk_x},{chunk_z} overlaps the header tables"
        );
        assert!(run > 0, "chunk {chunk_x},{chunk_z} has an empty sector run");
        let file_len = std::fs::metadata(&path).expect("region metadata").len();
        assert_eq!(
            file_len % 4096,
            0,
            "{} is not sector aligned",
            path.display()
        );
        assert!(
            (sector + run) * 4096 <= file_len,
            "chunk {chunk_x},{chunk_z} runs past the end of {}",
            path.display()
        );
        // Chunk stream prefix: 4-byte length, then the documented compression
        // type: 1 gzip, 2 zlib, or 3 none.
        let mut stream = [0u8; 5];
        std::fs::File::open(&path)
            .and_then(|mut file| {
                file.seek(SeekFrom::Start(sector * 4096))?;
                file.read_exact(&mut stream)
            })
            .expect("chunk stream header");
        assert!(
            matches!(stream[4], 1..=3),
            "unexpected chunk compression type {}",
            stream[4]
        );
    }
}

#[test]
fn breaking_supporting_block_collapses_resting_foliage_snow_and_carpet_at_y_plus_1() {
    let mut chunk = Chunk::empty(0, 0);

    // ShortGrass (foliage) resting on soil (Grass).
    let (x1, y1, z1) = (2, 10, 3);
    chunk.set_and_update(x1, y1, z1, Block::Grass).unwrap();
    chunk
        .set_and_update(x1, y1 + 1, z1, Block::ShortGrass)
        .unwrap();
    assert_eq!(chunk.block(x1, y1 + 1, z1), Some(Block::ShortGrass));

    let changes1 = chunk.set_and_update(x1, y1, z1, Block::Air).unwrap();
    assert_eq!(chunk.block(x1, y1, z1), Some(Block::Air));
    assert_eq!(chunk.block(x1, y1 + 1, z1), Some(Block::Air));
    assert!(changes1.contains(&CellChange {
        x: x1,
        y: y1,
        z: z1,
        block: Block::Air
    }));
    assert!(changes1.contains(&CellChange {
        x: x1,
        y: y1 + 1,
        z: z1,
        block: Block::Air
    }));

    // SnowLayer resting on solid block (Stone).
    let (x2, y2, z2) = (5, 10, 5);
    chunk.set_and_update(x2, y2, z2, Block::Stone).unwrap();
    chunk
        .set_and_update(x2, y2 + 1, z2, Block::SnowLayer)
        .unwrap();
    assert_eq!(chunk.block(x2, y2 + 1, z2), Some(Block::SnowLayer));

    let changes2 = chunk.set_and_update(x2, y2, z2, Block::Air).unwrap();
    assert_eq!(chunk.block(x2, y2, z2), Some(Block::Air));
    assert_eq!(chunk.block(x2, y2 + 1, z2), Some(Block::Air));
    assert!(changes2.contains(&CellChange {
        x: x2,
        y: y2,
        z: z2,
        block: Block::Air
    }));
    assert!(changes2.contains(&CellChange {
        x: x2,
        y: y2 + 1,
        z: z2,
        block: Block::Air
    }));

    // Carpet resting on solid block (Stone).
    let (x3, y3, z3) = (8, 10, 8);
    chunk.set_and_update(x3, y3, z3, Block::Stone).unwrap();
    chunk.set_and_update(x3, y3 + 1, z3, Block::Carpet).unwrap();
    assert_eq!(chunk.block(x3, y3 + 1, z3), Some(Block::Carpet));

    let changes3 = chunk.set_and_update(x3, y3, z3, Block::Air).unwrap();
    assert_eq!(chunk.block(x3, y3, z3), Some(Block::Air));
    assert_eq!(chunk.block(x3, y3 + 1, z3), Some(Block::Air));
    assert!(changes3.contains(&CellChange {
        x: x3,
        y: y3,
        z: z3,
        block: Block::Air
    }));
    assert!(changes3.contains(&CellChange {
        x: x3,
        y: y3 + 1,
        z: z3,
        block: Block::Air
    }));
}

#[test]
fn breaking_either_half_of_double_plant_destroys_the_other_half_deterministically() {
    let mut chunk = Chunk::empty(0, 0);

    // Case 1: Breaking the upper half destroys the lower half.
    let (x1, y1, z1) = (3, 20, 3);
    chunk.set_and_update(x1, y1, z1, Block::Grass).unwrap();
    chunk
        .set_and_update(x1, y1 + 1, z1, Block::TallGrassLower)
        .unwrap();
    chunk
        .set_and_update(x1, y1 + 2, z1, Block::TallGrassUpper)
        .unwrap();

    assert_eq!(chunk.block(x1, y1 + 1, z1), Some(Block::TallGrassLower));
    assert_eq!(chunk.block(x1, y1 + 2, z1), Some(Block::TallGrassUpper));

    let changes_upper = chunk.set_and_update(x1, y1 + 2, z1, Block::Air).unwrap();
    assert_eq!(chunk.block(x1, y1 + 2, z1), Some(Block::Air));
    assert_eq!(chunk.block(x1, y1 + 1, z1), Some(Block::Air));
    assert_eq!(chunk.block(x1, y1, z1), Some(Block::Grass));
    assert!(changes_upper.contains(&CellChange {
        x: x1,
        y: y1 + 2,
        z: z1,
        block: Block::Air
    }));
    assert!(changes_upper.contains(&CellChange {
        x: x1,
        y: y1 + 1,
        z: z1,
        block: Block::Air
    }));

    // Case 2: Breaking the lower half destroys the upper half.
    let (x2, y2, z2) = (6, 20, 6);
    chunk.set_and_update(x2, y2, z2, Block::Grass).unwrap();
    chunk
        .set_and_update(x2, y2 + 1, z2, Block::TallGrassLower)
        .unwrap();
    chunk
        .set_and_update(x2, y2 + 2, z2, Block::TallGrassUpper)
        .unwrap();

    assert_eq!(chunk.block(x2, y2 + 1, z2), Some(Block::TallGrassLower));
    assert_eq!(chunk.block(x2, y2 + 2, z2), Some(Block::TallGrassUpper));

    let changes_lower = chunk.set_and_update(x2, y2 + 1, z2, Block::Air).unwrap();
    assert_eq!(chunk.block(x2, y2 + 1, z2), Some(Block::Air));
    assert_eq!(chunk.block(x2, y2 + 2, z2), Some(Block::Air));
    assert_eq!(chunk.block(x2, y2, z2), Some(Block::Grass));
    assert!(changes_lower.contains(&CellChange {
        x: x2,
        y: y2 + 1,
        z: z2,
        block: Block::Air
    }));
    assert!(changes_lower.contains(&CellChange {
        x: x2,
        y: y2 + 2,
        z: z2,
        block: Block::Air
    }));
}

#[test]
fn replacing_soil_with_non_soil_solid_breaks_plants_but_preserves_snow_layer_and_carpet() {
    let mut chunk = Chunk::empty(0, 0);

    // ShortGrass on Dirt: replacing Dirt with Stone breaks ShortGrass.
    let (x1, y1, z1) = (1, 15, 1);
    chunk.set_and_update(x1, y1, z1, Block::Dirt).unwrap();
    chunk
        .set_and_update(x1, y1 + 1, z1, Block::ShortGrass)
        .unwrap();
    assert_eq!(chunk.block(x1, y1 + 1, z1), Some(Block::ShortGrass));

    let changes_plant = chunk.set_and_update(x1, y1, z1, Block::Stone).unwrap();
    assert_eq!(chunk.block(x1, y1, z1), Some(Block::Stone));
    assert_eq!(chunk.block(x1, y1 + 1, z1), Some(Block::Air));
    assert!(changes_plant.contains(&CellChange {
        x: x1,
        y: y1 + 1,
        z: z1,
        block: Block::Air
    }));

    // TallGrass on Grass: replacing Grass with Stone breaks both halves.
    let (x2, y2, z2) = (3, 15, 3);
    chunk.set_and_update(x2, y2, z2, Block::Grass).unwrap();
    chunk
        .set_and_update(x2, y2 + 1, z2, Block::TallGrassLower)
        .unwrap();
    chunk
        .set_and_update(x2, y2 + 2, z2, Block::TallGrassUpper)
        .unwrap();

    let changes_double = chunk.set_and_update(x2, y2, z2, Block::Stone).unwrap();
    assert_eq!(chunk.block(x2, y2, z2), Some(Block::Stone));
    assert_eq!(chunk.block(x2, y2 + 1, z2), Some(Block::Air));
    assert_eq!(chunk.block(x2, y2 + 2, z2), Some(Block::Air));
    assert!(changes_double.contains(&CellChange {
        x: x2,
        y: y2 + 1,
        z: z2,
        block: Block::Air
    }));
    assert!(changes_double.contains(&CellChange {
        x: x2,
        y: y2 + 2,
        z: z2,
        block: Block::Air
    }));

    // SnowLayer on Grass: replacing Grass with Stone preserves SnowLayer.
    let (x3, y3, z3) = (5, 15, 5);
    chunk.set_and_update(x3, y3, z3, Block::Grass).unwrap();
    chunk
        .set_and_update(x3, y3 + 1, z3, Block::SnowLayer)
        .unwrap();
    assert_eq!(chunk.block(x3, y3 + 1, z3), Some(Block::SnowLayer));

    let changes_snow = chunk.set_and_update(x3, y3, z3, Block::Stone).unwrap();
    assert_eq!(chunk.block(x3, y3, z3), Some(Block::Stone));
    assert_eq!(chunk.block(x3, y3 + 1, z3), Some(Block::SnowLayer));
    assert_eq!(
        changes_snow,
        vec![CellChange {
            x: x3,
            y: y3,
            z: z3,
            block: Block::Stone
        }]
    );

    // Carpet on Dirt: replacing Dirt with Stone preserves Carpet.
    let (x4, y4, z4) = (7, 15, 7);
    chunk.set_and_update(x4, y4, z4, Block::Dirt).unwrap();
    chunk.set_and_update(x4, y4 + 1, z4, Block::Carpet).unwrap();
    assert_eq!(chunk.block(x4, y4 + 1, z4), Some(Block::Carpet));

    let changes_carpet = chunk.set_and_update(x4, y4, z4, Block::Stone).unwrap();
    assert_eq!(chunk.block(x4, y4, z4), Some(Block::Stone));
    assert_eq!(chunk.block(x4, y4 + 1, z4), Some(Block::Carpet));
    assert_eq!(
        changes_carpet,
        vec![CellChange {
            x: x4,
            y: y4,
            z: z4,
            block: Block::Stone
        }]
    );
}

#[test]
fn unsupported_placement_and_out_of_bounds_coordinates_are_rejected_without_mutation() {
    let mut chunk = Chunk::empty(0, 0);

    // 1. Out-of-bounds coordinates must return None without mutation.
    assert!(
        chunk
            .set_and_update(CHUNK_SIDE, 10, 0, Block::Stone)
            .is_none()
    );
    assert!(
        chunk
            .set_and_update(0, WORLD_HEIGHT, 0, Block::Stone)
            .is_none()
    );
    assert!(
        chunk
            .set_and_update(0, 10, CHUNK_SIDE, Block::Stone)
            .is_none()
    );
    assert!(chunk.set_and_update(100, 200, 300, Block::Dirt).is_none());

    // 2. Unsupported placement must return None without mutation.
    // ShortGrass on Air:
    assert!(chunk.set_and_update(0, 10, 0, Block::ShortGrass).is_none());
    assert_eq!(chunk.block(0, 10, 0), Some(Block::Air));

    // ShortGrass on Stone (Stone is not soil):
    chunk.set_and_update(1, 10, 1, Block::Stone).unwrap();
    assert!(chunk.set_and_update(1, 11, 1, Block::ShortGrass).is_none());
    assert_eq!(chunk.block(1, 11, 1), Some(Block::Air));

    // ShortGrass at y = 0:
    assert!(chunk.set_and_update(2, 0, 2, Block::ShortGrass).is_none());
    assert_eq!(chunk.block(2, 0, 2), Some(Block::Air));

    // Carpet on Air:
    assert!(chunk.set_and_update(3, 10, 3, Block::Carpet).is_none());
    assert_eq!(chunk.block(3, 10, 3), Some(Block::Air));

    // Carpet on ShortGrass (ShortGrass does not provide top support):
    chunk.set_and_update(4, 10, 4, Block::Grass).unwrap();
    chunk.set_and_update(4, 11, 4, Block::ShortGrass).unwrap();
    assert!(chunk.set_and_update(4, 12, 4, Block::Carpet).is_none());
    assert_eq!(chunk.block(4, 12, 4), Some(Block::Air));

    // SnowLayer on Air:
    assert!(chunk.set_and_update(5, 10, 5, Block::SnowLayer).is_none());
    assert_eq!(chunk.block(5, 10, 5), Some(Block::Air));

    // TallGrassUpper on Grass (must be placed on TallGrassLower):
    chunk.set_and_update(6, 10, 6, Block::Grass).unwrap();
    assert!(
        chunk
            .set_and_update(6, 11, 6, Block::TallGrassUpper)
            .is_none()
    );
    assert_eq!(chunk.block(6, 11, 6), Some(Block::Air));

    // TallGrassLower at WORLD_HEIGHT - 1 (no space for upper half):
    chunk
        .set_and_update(7, WORLD_HEIGHT - 2, 7, Block::Grass)
        .unwrap();
    assert!(
        chunk
            .set_and_update(7, WORLD_HEIGHT - 1, 7, Block::TallGrassLower)
            .is_none()
    );
    assert_eq!(chunk.block(7, WORLD_HEIGHT - 1, 7), Some(Block::Air));
}

#[test]
fn survival_pass_is_bounded_and_stops_at_first_self_supporting_block_above() {
    let mut chunk = Chunk::empty(0, 0);

    // Set up a vertical column:
    // y = 10: Grass (soil supporting ShortGrass at 11)
    // y = 11: ShortGrass (dependent on 10)
    // y = 12: Stone (self-supporting block that stops the survival pass!)
    // y = 13: Carpet (dependent on Stone at 12)
    chunk.set_and_update(4, 10, 4, Block::Grass).unwrap();
    chunk.set_and_update(4, 11, 4, Block::ShortGrass).unwrap();
    chunk.set_block(4, 12, 4, Block::Stone);
    chunk.set_and_update(4, 13, 4, Block::Carpet).unwrap();

    assert_eq!(chunk.block(4, 10, 4), Some(Block::Grass));
    assert_eq!(chunk.block(4, 11, 4), Some(Block::ShortGrass));
    assert_eq!(chunk.block(4, 12, 4), Some(Block::Stone));
    assert_eq!(chunk.block(4, 13, 4), Some(Block::Carpet));

    // Break Grass at y = 10:
    let changes = chunk.set_and_update(4, 10, 4, Block::Air).unwrap();

    // Grass at 10 broke to Air.
    assert_eq!(chunk.block(4, 10, 4), Some(Block::Air));
    // ShortGrass at 11 collapsed to Air.
    assert_eq!(chunk.block(4, 11, 4), Some(Block::Air));
    // Stone at 12 is self-supporting, so it survived and stopped the pass.
    assert_eq!(chunk.block(4, 12, 4), Some(Block::Stone));
    // Carpet at 13 was never reached and continues to rest on Stone at 12.
    assert_eq!(chunk.block(4, 13, 4), Some(Block::Carpet));

    assert_eq!(
        changes,
        vec![
            CellChange {
                x: 4,
                y: 10,
                z: 4,
                block: Block::Air
            },
            CellChange {
                x: 4,
                y: 11,
                z: 4,
                block: Block::Air
            },
        ]
    );

    // Also verify that the survival pass stops when Air is hit.
    // Set up:
    // y = 20: Grass
    // y = 21: ShortGrass
    // y = 22: Air
    // y = 23: SnowLayer
    chunk.set_and_update(8, 20, 8, Block::Grass).unwrap();
    chunk.set_and_update(8, 21, 8, Block::ShortGrass).unwrap();
    chunk.set_block(8, 22, 8, Block::Air);
    chunk.set_block(8, 23, 8, Block::SnowLayer);

    let changes_air = chunk.set_and_update(8, 20, 8, Block::Air).unwrap();
    assert_eq!(chunk.block(8, 20, 8), Some(Block::Air));
    assert_eq!(chunk.block(8, 21, 8), Some(Block::Air));
    assert_eq!(chunk.block(8, 22, 8), Some(Block::Air));
    // SnowLayer at 23 was not touched because pass stopped at Air at 22.
    assert_eq!(chunk.block(8, 23, 8), Some(Block::SnowLayer));
    assert_eq!(
        changes_air,
        vec![
            CellChange {
                x: 8,
                y: 20,
                z: 8,
                block: Block::Air
            },
            CellChange {
                x: 8,
                y: 21,
                z: 8,
                block: Block::Air
            },
        ]
    );
}

#[test]
fn atomic_double_plant_placement_and_rejection_when_blocked() {
    let mut chunk = Chunk::empty(0, 0);

    // Place Grass soil at (5, 10, 5)
    chunk.set_and_update(5, 10, 5, Block::Grass).unwrap();

    // 1. Placing TallGrassLower at (5, 11, 5) atomically places TallGrassUpper at (5, 12, 5)
    let changes = chunk
        .set_and_update(5, 11, 5, Block::TallGrassLower)
        .unwrap();
    assert_eq!(chunk.block(5, 11, 5), Some(Block::TallGrassLower));
    assert_eq!(chunk.block(5, 12, 5), Some(Block::TallGrassUpper));
    assert!(changes.contains(&CellChange {
        x: 5,
        y: 11,
        z: 5,
        block: Block::TallGrassLower,
    }));
    assert!(changes.contains(&CellChange {
        x: 5,
        y: 12,
        z: 5,
        block: Block::TallGrassUpper,
    }));

    // 2. Placing TallGrassLower when above block is occupied by solid block (Stone) is rejected
    let mut blocked_chunk = Chunk::empty(0, 0);
    blocked_chunk
        .set_and_update(5, 10, 5, Block::Grass)
        .unwrap();
    blocked_chunk.set_block(5, 12, 5, Block::Stone);
    assert!(
        blocked_chunk
            .set_and_update(5, 11, 5, Block::TallGrassLower)
            .is_none()
    );
    assert_eq!(blocked_chunk.block(5, 11, 5), Some(Block::Air));
}
