//! Column-height extraction from a compiled `noise_settings` router.
//!
//! The surface of a vanilla column is the highest block position that the
//! chunk filler would write: the first position from the top of the world
//! whose `Substance` (the raw `final_density` sample adjusted by the
//! runtime aquifer, or by the global sea/lava picker when the dimension
//! has no `aquifers` section) is not air. Below sea level this reproduces
//! the historical "water at `sea_level - 1` counts as the top" rule from
//! the picker alone, and the aquifer can additionally raise the terrain
//! with barrier pressure or move fluid surfaces up and down.
//!
//! The default scan walks the column block by block from the top. A
//! grid-refined alternative (`terrain_top`) exploits the fact that the
//! terrain branch of the 26.3 overworld `final_density` is an
//! `interpolated` wrapper (cell size 8 in y) followed only by
//! sign-preserving operations, so a positive block normally sits at most
//! seven above the first positive y-grid sample. That shortcut is *not*
//! exact for the real graph: the per-block `min` with block-resolution
//! carving (`noodle`) can dip the grid sample itself below zero while
//! the surface block between samples stays positive. Measured against
//! the owner's seed-2026 save, the shortcut missed 7 of 2,401 columns;
//! the exhaustive scan is therefore the default.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use crate::vanilla::aquifer::{
    Aquifer, AquiferOccupancy, Fluid, GlobalFluid, NoiseBasedAquifer, Substance,
};
use crate::vanilla::biome::{
    BiomePlacement, ClimateSampler, ColumnSearchScratch, fixed_climate_coords, fixed_run_len,
};
use crate::vanilla::cache::{BoundedCache, ShardedBoundedCache};
use crate::vanilla::carver::{CarveMask, Carver, CarverContext, CarverData};
use crate::vanilla::density::DensityVolume;
use crate::vanilla::feature::{DecorationTarget, FeatureData};
use crate::vanilla::random::LegacyRandom;
use crate::vanilla::surface::{SurfaceContext, SurfaceRules};
use crate::vanilla::worldgen::{NoiseRouter, WorldgenData, WorldgenError};

/// `DimensionType.WAY_BELOW_MIN_Y`: the descent ceiling sentinel when the
/// column is solid all the way to the floor (recorded in `PROVENANCE.md`).
const WAY_BELOW_MIN_Y: i32 = -32512;

/// A biome's resolved carver list, shared between the registry and the
/// per-source-chunk cache.
type ChunkCarvers = Rc<Vec<Option<Rc<Carver>>>>;

/// Cached `final_density` volume fill for one chunk (`16 x height x 16`).
type ChunkDensities = Arc<[f32]>;

/// Local memo of quantized 4x4x4-cell biome identifiers.
type BiomePointCache = BoundedCache<(i32, i32, i32), Option<Arc<str>>>;

/// One chunk after the decoration pass: the material-rule columns with
/// every vein that landed in them painted over.
///
/// The grid is a flat palette index per position rather than a `String`
/// per position: a chunk column is 256 x `height` positions, and holding a
/// name for each would cost more than the terrain it describes. The palette
/// stays small because one chunk holds a couple of dozen stone and surface
/// families plus the ores that decorated it.
struct DecoratedChunk {
    /// Interned block names; index 0 is the air slot a `None` column entry
    /// maps to.
    palette: Vec<Option<String>>,
    /// Whether each palette slot is a non-air block, kept so `top_non_air`
    /// maintenance and heightmap scans avoid repeated string lookups.
    palette_non_air: Vec<bool>,
    /// Which palette entry each position holds, laid out y-outer so that one
    /// absolute Y row of the chunk is 256 contiguous entries: `row * 256 +
    /// local_x * 16 + local_z`, with `row` relative to `min_y`.
    blocks: Vec<u16>,
    /// Highest non-air relative row index (`0..height`), or `-1` when a column
    /// is completely empty. Maintained on populate and `set`.
    top_non_air: [i16; 256],
    /// Name to palette index, kept so interning is O(1) per write.
    ids: HashMap<String, u16>,
}

impl DecoratedChunk {
    fn new(height: usize) -> Self {
        Self {
            palette: vec![None],
            palette_non_air: vec![false],
            blocks: vec![0; height * 256],
            top_non_air: [-1; 256],
            ids: HashMap::new(),
        }
    }

    fn index(&self, local_x: i32, row: usize, local_z: i32) -> usize {
        row * 256 + (local_x as usize) * 16 + local_z as usize
    }

    /// The palette slot for a name, adding it when first seen.
    fn intern(&mut self, name: Option<String>) -> u16 {
        let Some(name) = name else { return 0 };
        if let Some(&slot) = self.ids.get(name.as_str()) {
            return slot;
        }
        let slot = self.palette.len() as u16;
        self.palette_non_air.push(!AIR_IDS.contains(&name.as_str()));
        self.palette.push(Some(name.clone()));
        self.ids.insert(name, slot);
        slot
    }

    fn name(&self, slot: u16) -> Option<&str> {
        self.palette.get(slot as usize).and_then(Option::as_deref)
    }

    fn set(&mut self, local_x: i32, row: usize, local_z: i32, name: &str) {
        let at = self.index(local_x, row, local_z);
        let slot = self.intern(Some(name.to_owned()));
        self.blocks[at] = slot;
        let col = (local_x as usize) * 16 + local_z as usize;
        if self.palette_non_air[slot as usize] {
            if (row as i16) > self.top_non_air[col] {
                self.top_non_air[col] = row as i16;
            }
        } else if (row as i16) == self.top_non_air[col] {
            let mut scan = row as i16 - 1;
            while scan >= 0 {
                let idx = (scan as usize) * 256 + col;
                if self.palette_non_air[self.blocks[idx] as usize] {
                    break;
                }
                scan -= 1;
            }
            self.top_non_air[col] = scan;
        }
    }

    /// One absolute column of the chunk, dimension-relative from index 0.
    fn column(&self, local_x: i32, local_z: i32) -> Vec<Option<String>> {
        (0..self.blocks.len() / 256)
            .map(|row| {
                self.name(self.blocks[self.index(local_x, row, local_z)])
                    .map(str::to_owned)
            })
            .collect()
    }
}

/// The chunk grid as the decoration pass sees it: reads and writes inside
/// the chunk being decorated, with the six-neighbour air test falling back
/// to the underlying terrain for positions in a neighbouring chunk.
struct ChunkView<'a> {
    generator: &'a VanillaGenerator,
    chunk_x: i32,
    chunk_z: i32,
    chunk: &'a mut DecoratedChunk,
    /// Undecorated neighbour columns already asked for by this chunk's pass.
    halo: &'a RefCell<HaloColumns>,
}

/// The neighbour columns one chunk's decoration pass has read. A predicate
/// that crosses the border asks for one row of a column that costs a full
/// fill and carve, and the next candidate usually asks for the same column.
/// Emptied at [`HALO_COLUMN_LIMIT`] so a pack of wide offsets cannot grow it
/// past a fixed size.
type HaloColumns = HashMap<(i32, i32, i32), Rc<str>>;

/// The most neighbour columns one pass keeps, about a chunk's perimeter four
/// rows deep.
const HALO_COLUMN_LIMIT: usize = 256;

/// One chunk's full `4 x (height/4) x 4` quart biome grid together with its
/// ascending, deduplicated biome list (`region`).
///
/// The reference runtime collects the palette of every section of a chunk
/// (`LevelChunkSection.getBiomes().getAll(sink)`) when it decides which features
/// may run, so `region` spans the full column height rather than only the
/// surface band. Storing the `1,536` `u16` palette slots alongside `region`
/// also serves every surface, decoration, and packet biome lookup in the chunk
/// in `O(1)` without extra per-cell allocations.
struct ChunkBiomes {
    quart_height: usize,
    region: Arc<[Arc<str>]>,
    cells: Arc<[u16]>,
}

impl ChunkBiomes {
    fn empty() -> Self {
        Self {
            quart_height: 0,
            region: Arc::from([]),
            cells: Arc::from([]),
        }
    }

    fn at_local(&self, local_qx: usize, local_qy: usize, local_qz: usize) -> Option<Arc<str>> {
        if self.quart_height == 0 {
            return None;
        }
        // Matches `DensityVolume::index`: `local_qy + (local_qx + local_qz * 4) * quart_height`.
        let index = (local_qz * 4 + local_qx) * self.quart_height + local_qy;
        let slot = *self.cells.get(index)?;
        (slot != u16::MAX).then(|| Arc::clone(&self.region[slot as usize]))
    }
}

struct ChunkBiomeBuild {
    palette: Vec<Arc<str>>,
    cells: Vec<u16>,
}

impl ChunkBiomeBuild {
    fn intern(&mut self, name: &str) -> (u16, Arc<str>) {
        if let Some(pos) = self
            .palette
            .iter()
            .position(|existing| existing.as_ref() == name)
        {
            (pos as u16, Arc::clone(&self.palette[pos]))
        } else {
            let slot = self.palette.len() as u16;
            let arc: Arc<str> = Arc::from(name);
            self.palette.push(Arc::clone(&arc));
            (slot, arc)
        }
    }

    fn finish(mut self, quart_height: usize) -> ChunkBiomes {
        let mut order: Vec<usize> = (0..self.palette.len()).collect();
        order.sort_unstable_by(|&a, &b| self.palette[a].cmp(&self.palette[b]));
        let region: Arc<[Arc<str>]> = order
            .iter()
            .map(|&idx| Arc::clone(&self.palette[idx]))
            .collect();
        let mut remap = vec![0u16; self.palette.len()];
        for (new_idx, &old_idx) in order.iter().enumerate() {
            remap[old_idx] = new_idx as u16;
        }
        for slot in &mut self.cells {
            if *slot != u16::MAX {
                *slot = remap[*slot as usize];
            }
        }
        ChunkBiomes {
            quart_height,
            region,
            cells: Arc::from(self.cells),
        }
    }
}

/// Sentinel for a heightmap row neither the chunk's fill pass nor an
/// earlier anchor-gate lookup has answered yet. No real row value can
/// collide with it: absolute build heights sit far inside `i32`'s range.
const UNCOMPUTED_OCEAN_FLOOR: i32 = i32::MIN;

/// One chunk's decoration-time `OCEAN_FLOOR_WG` heightmap: the 16x16
/// column rows in the same `local_x * 16 + local_z` layout as the
/// decorated grid. The chunk fill pass writes all 256 rows from the
/// substances it already sampled while descending the column for its
/// block ids, and an anchor-gate lookup into a neighbour's halo adds the
/// columns it touches, so no column of the decoration window descends
/// the density graph a second time.
#[derive(Debug, Clone)]
struct OceanFloorMap {
    entries: [i32; 256],
}

impl OceanFloorMap {
    fn new() -> Self {
        Self {
            entries: [UNCOMPUTED_OCEAN_FLOOR; 256],
        }
    }

    /// Slot of one absolute column within its chunk's map.
    fn slot(x: i32, z: i32) -> usize {
        ((x & 15) * 16 + (z & 15)) as usize
    }
}

/// Bounded, sharded cross-worker store of immutable per-chunk and per-column
/// terrain products.
///
/// Workers generate scattered chunks in parallel while decoration touches a
/// `5x5` neighbourhood of densities, biome volumes, carve masks, and
/// heightmaps. Sharing those pure products in one `Arc<SharedChunkCaches>`
/// prevents N workers from recomputing the same neighbour chunks while keeping
/// every value immutable (`Arc<[...]>`, `Arc<str>`, `Arc<CarveMask>`, `i32`)
/// and never holding a shard lock across computation.
///
/// All generators sharing an instance must come from the same data root, world
/// seed, and dimension settings.
///
/// Memory budget across the 32 shards (worst-case 2x generation cap):
/// - `chunk_density`: `2 * 192 = 384` chunks x `384 KiB` = `144 MiB`
/// - `masks`: `2 * 256 = 512` chunks x `12 KiB` ≈ `6 MiB`
/// - `chunk_biomes`: `2 * 512 = 1,024` chunks x `~3.5 KiB` ≈ `3.5 MiB`
/// - `ocean_floor_maps`: `2 * 512 = 1,024` chunks x `1 KiB` = `1 MiB`
/// - `surface_height_maps`: `2 * 512 = 1,024` chunks x `1 KiB` = `1 MiB`
/// - `biomes`: `2 * 8,192 = 16,384` quart cells x `~32 B` ≈ `512 KiB`
/// - `ocean_floor_cols`: `2 * 16,384 = 32,768` columns x `~16 B` ≈ `512 KiB`
/// - `surface_heights`: `2 * 16,384 = 32,768` columns x `~16 B` ≈ `512 KiB`
///
/// Total upper bound: roughly `157 MiB`, bounded regardless of world size.
pub struct SharedChunkCaches {
    chunk_density: ShardedBoundedCache<(i32, i32), ChunkDensities>,
    chunk_biomes: ShardedBoundedCache<(i32, i32), Arc<ChunkBiomes>>,
    biomes: ShardedBoundedCache<(i32, i32, i32), Option<Arc<str>>>,
    masks: ShardedBoundedCache<(i32, i32), Arc<CarveMask>>,
    ocean_floor_maps: ShardedBoundedCache<(i32, i32), Arc<[i32; 256]>>,
    ocean_floor_cols: ShardedBoundedCache<(i32, i32), i32>,
    surface_height_maps: ShardedBoundedCache<(i32, i32), Arc<[i32; 256]>>,
    surface_heights: ShardedBoundedCache<(i32, i32), i32>,
}

impl SharedChunkCaches {
    /// Target active-window capacity for shared chunk `final_density` grids
    /// (max `384` entries across two generations, `144 MiB` at Overworld height).
    pub const CHUNK_DENSITY_CAPACITY: usize = 192;
    /// Target active-window capacity for shared chunk biome grids and regions
    /// (max `1,024` entries, `~3.5 MiB`).
    pub const CHUNK_BIOMES_CAPACITY: usize = 512;
    /// Target active-window capacity for shared point biome lookups such as
    /// carver source chunks (max `16,384` entries, `~512 KiB`).
    pub const BIOME_POINT_CAPACITY: usize = 8_192;
    /// Target active-window capacity for shared per-chunk carve masks (max
    /// `512` entries, `~6 MiB`).
    pub const MASK_CAPACITY: usize = 256;
    /// Target active-window capacity for completed chunk ocean-floor heightmaps
    /// (max `1,024` entries, `1 MiB`).
    pub const OCEAN_FLOOR_MAP_CAPACITY: usize = 512;
    /// Target active-window capacity for individual halo ocean-floor columns
    /// (max `32,768` entries, `~512 KiB`).
    pub const OCEAN_FLOOR_COL_CAPACITY: usize = 16_384;
    /// Target active-window capacity for completed chunk surface heightmaps
    /// (max `1,024` entries, `1 MiB`).
    pub const SURFACE_HEIGHT_MAP_CAPACITY: usize = 512;
    /// Target active-window capacity for individual halo surface-top columns
    /// (max `32,768` entries, `~512 KiB`).
    pub const SURFACE_HEIGHT_COL_CAPACITY: usize = 16_384;

    /// Creates an empty shared cache set whose shards allocate lazily.
    pub fn new() -> Self {
        Self {
            chunk_density: ShardedBoundedCache::new(Self::CHUNK_DENSITY_CAPACITY),
            chunk_biomes: ShardedBoundedCache::new(Self::CHUNK_BIOMES_CAPACITY),
            biomes: ShardedBoundedCache::new(Self::BIOME_POINT_CAPACITY),
            masks: ShardedBoundedCache::new(Self::MASK_CAPACITY),
            ocean_floor_maps: ShardedBoundedCache::new(Self::OCEAN_FLOOR_MAP_CAPACITY),
            ocean_floor_cols: ShardedBoundedCache::new(Self::OCEAN_FLOOR_COL_CAPACITY),
            surface_height_maps: ShardedBoundedCache::new(Self::SURFACE_HEIGHT_MAP_CAPACITY),
            surface_heights: ShardedBoundedCache::new(Self::SURFACE_HEIGHT_COL_CAPACITY),
        }
    }
}

impl Default for SharedChunkCaches {
    fn default() -> Self {
        Self::new()
    }
}

/// Entry counts of the generator's coordinate-keyed caches. Every one of
/// them is keyed by world coordinates, so the count grows with the volume
/// of world the generator has been asked about; `VanillaGenerator` bounds
/// each with a fixed-capacity cache and this view lets tests and the bench
/// binary check the bound holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheOccupancy {
    /// Target chunks with a replayed carve mask.
    pub masks: usize,
    /// Source chunks with a resolved biome carver list.
    pub chunk_carvers: usize,
    /// Columns with a memoised surface top.
    pub heights: usize,
    /// Chunks with a stored decoration-time ocean-floor heightmap.
    pub ocean_floor_maps: usize,
    /// Quart-grid biome choices retained for nearby samples.
    pub biomes: usize,
    /// Chunks whose full biome volume has been enumerated.
    pub biome_regions: usize,
    /// Aquifer fluid cells with a memoised center.
    pub aquifer_centers: usize,
    /// Aquifer fluid cells with a memoised status.
    pub aquifer_statuses: usize,
    /// Quart-grid points with a memoised preliminary surface level.
    pub aquifer_surface_levels: usize,
    /// Chunks with a memoised aquifer sampling bound.
    pub aquifer_skip_bounds: usize,
    /// Chunks with a decorated grid.
    pub decorated_chunks: usize,
    /// Chunks with a cached volume-filled `final_density` grid.
    pub chunk_densities: usize,
}

impl CacheOccupancy {
    /// Sum of all entries held.
    pub fn total(&self) -> usize {
        self.masks
            + self.chunk_carvers
            + self.heights
            + self.ocean_floor_maps
            + self.biomes
            + self.biome_regions
            + self.aquifer_centers
            + self.aquifer_statuses
            + self.aquifer_surface_levels
            + self.aquifer_skip_bounds
            + self.decorated_chunks
            + self.chunk_densities
    }
}

/// Reusable buffers for `chunk_biomes` climate volume fills and column-run
/// placement searches, preventing per-run and per-cell allocations.
#[derive(Default)]
struct ClimateScratch {
    density: Vec<f32>,
    targets: Vec<[i64; 6]>,
    column: ColumnRunScratch,
}

/// Per-run scratch buffers owned by [`ClimateScratch`].
#[derive(Default)]
struct ColumnRunScratch {
    search: ColumnSearchScratch,
    depths: Vec<i64>,
    keys: Vec<(i32, i32, i32)>,
    slots: Vec<usize>,
    rows: Vec<usize>,
}

/// One dimension's column-height, biome, and top-block source, built from
/// operator-provisioned data (never committed) and a world seed. The
/// compiled router owns its graph, and the surface program owns every
/// noise stack and random factory it needs, so the engine is used only
/// while wiring.
///
/// The coordinate-keyed memos below are fixed-capacity caches, not
/// growing maps: every value they hold is a pure function of world
/// coordinates and the seed, so eviction only costs a recomputation. See
/// `vanilla::cache` and the capacity comments for the sizing reasoning.
pub struct VanillaGenerator {
    router: NoiseRouter,
    max_y: i32,
    aquifer: Aquifer,
    climate: ClimateSampler,
    placement: Option<BiomePlacement>,
    surface: Option<SurfaceRules>,
    world_seed: i64,
    carvers: Option<Rc<CarverData>>,
    carver_context: CarverContext,
    /// The pack's placement stage: `None` when the operator's data has no
    /// biome or placed-feature documents to decorate with.
    features: Option<FeatureData>,
    /// Per-target-chunk carving masks, built lazily on first probe.
    masks: RefCell<BoundedCache<(i32, i32), Arc<CarveMask>>>,
    /// Per-source-chunk biome carver lists, built lazily during replay.
    chunk_carvers: RefCell<BoundedCache<(i32, i32), ChunkCarvers>>,
    /// Column-top memo: the descent's steep-gradient lookups re-read
    /// neighbouring columns, whose top scan is otherwise O(height).
    heights: RefCell<BoundedCache<(i32, i32), i32>>,
    /// Ocean-floor heightmaps by chunk: the anchor gate's footprint reads
    /// are served from the rows the chunk fill pass derived from the
    /// substances it had already sampled, plus the halo columns it looked
    /// up. See `OceanFloorMap`.
    ocean_floor_maps: RefCell<BoundedCache<(i32, i32), OceanFloorMap>>,
    /// One immutable biome choice per quantized 4x4x4 world cell.
    biomes: RefCell<BiomePointCache>,
    /// Chunks whose whole biome volume has been enumerated, for the union a
    /// decoration pass runs its ordinals over and O(1) per-cell lookup.
    biome_regions: RefCell<BoundedCache<(i32, i32), Arc<ChunkBiomes>>>,
    /// Chunks whose columns have been decorated. The value is a pure
    /// function of the chunk position and the seed, so an eviction only
    /// costs a replay of the nine anchor chunks' placement passes.
    chunks: RefCell<BoundedCache<(i32, i32), Rc<DecoratedChunk>>>,
    /// Raw `final_density` for one chunk, `16 x height x 16` block values in
    /// `DensityVolume::index` order, filled by one `sample_volume` pass.
    chunk_density: RefCell<BoundedCache<(i32, i32), ChunkDensities>>,
    /// Cross-worker shared cache of immutable chunk/column products.
    shared: Arc<SharedChunkCaches>,
    /// Reusable buffer for `chunk_density` volume fills.
    density_scratch: RefCell<Vec<f32>>,
    /// Reusable buffers for `chunk_biomes` climate volume fills and column searches.
    climate_scratch: RefCell<ClimateScratch>,
}

impl VanillaGenerator {
    /// Target chunks with a replayed carve mask. A mask is only ever read
    /// for positions inside its own chunk (`carved` derives the target
    /// from the block coordinates), and building one replays the 17x17
    /// source window, so contiguous generation touches one mask at a time.
    /// 32 keeps a 4x4 chunk batch and the row it is streaming warm at
    /// 64 masks; the overworld mask is 12,032 bytes of bitset, so that is
    /// about 750 KiB instead of the 48 MiB a radius-32 view (4,225 chunks)
    /// would otherwise pin forever.
    pub const MASK_CACHE_CAPACITY: usize = 32;
    /// Source chunks with a resolved biome carver list. One mask replay
    /// resolves the 17x17 window (289 chunks) and consecutive targets
    /// overlap by 16 of 17 rows, so a row-major sweep of a view of radius
    /// R needs 17x(2R+17) entries: 1,377 at R=32. The values are shared
    /// references into the carver registry, so an entry is 16 bytes and
    /// 2,048 costs under 100 KiB. The recompute is not cheap (a climate
    /// sample plus a linear search of the placement table), which is why
    /// this cache is sized for a whole 32-chunk row rather than a few
    /// chunks.
    pub const CHUNK_CARVERS_CACHE_CAPACITY: usize = 2_048;
    /// Columns with a memoised surface top. The surface program's steep
    /// test reads the columns one block either side in x and z (clamped to
    /// the chunk), so a column's top is reused within its own chunk: 256
    /// entries per chunk plus the halo. 4,096 covers 16 chunks of
    /// streaming for a full-radius view's worth of 1,081,600 columns,
    /// i.e. tens of KiB instead of ~14 MB.
    pub const HEIGHT_CACHE_CAPACITY: usize = 4_096;
    /// Chunks with a stored ocean-floor heightmap. One map is 256 four-byte
    /// rows, a KiB. The anchor gate reads the footprint columns of boxes
    /// inside the nine anchor chunks of a decoration, and the widest box
    /// spills about thirteen blocks past its anchor chunk, so one target's
    /// gate touches a five-by-five band of maps and a row-major sweep keeps
    /// re-reading the band it has already stored. The previous 4,096-column
    /// memo thrashed on exactly that working set — a 4x4 batch touches some
    /// 4,800 distinct columns — recomputing each descent through the density
    /// graph; the same sweep stored chunk-keyed holds all 4,800 answers in
    /// at most 25 maps — the batch's sixteen target chunks plus the halo of
    /// anchor chunks around them — and every map carries its chunk's full
    /// 256 rows whether or not the gate ever reaches that chunk again.
    /// 32 keys, 64 maps at the doubled bound, costs 64 KiB per generator.
    pub const OCEAN_FLOOR_MAP_CACHE_CAPACITY: usize = 32;
    /// A full chunk has at most 4×4×96 quart cells; neighboring chunk and
    /// surface-rule probes reuse them before a streaming sweep evicts them.
    pub const BIOME_CACHE_CAPACITY: usize = 4_096;
    /// Chunks with a decorated grid. A column sweep visits a chunk 16 times
    /// before moving on and returns to it only after a whole row of 512
    /// columns, so the cache has to hold one row's worth of chunk keys — 32
    /// for a 512-block square — or every column rebuilds its chunk, which
    /// costs 256 columns plus nine placement replays each time. The grid is
    /// two bytes per position, so an overworld entry is 196 KiB and the
    /// bound costs 6.3 MiB; a smaller square or a chunk-at-a-time consumer
    /// simply never fills it.
    pub const DECORATED_CHUNK_CACHE_CAPACITY: usize = 32;
    /// Chunks whose whole biome volume has been enumerated, for the union a
    /// decoration pass runs one anchor's ordinals over. Moving the anchor one
    /// column adds three chunk volumes to the nine it already holds, so 16
    /// cover a straight sweep; 64 also cover a 4x4 target batch's nine
    /// anchor windows. An entry is the sorted list of the distinct biome
    /// identifiers in the chunk — a few hundred bytes — so the bound costs
    /// tens of KiB instead of pinning one volume per chunk a radius-32 view
    /// touches (1,089 chunks).
    pub const BIOME_REGION_CACHE_CAPACITY: usize = 64;
    /// Chunks with a volume-filled `final_density` array (`16 x height x 16`
    /// floats). Sixteen chunks per generation cover the 3x3 decoration
    /// neighbourhood plus a streaming margin (at most 32 chunks across the
    /// two-generation cache). At 384 blocks high, one entry is
    /// `16 * 384 * 16 * 4` = 384 KiB, so 32 entries cap at 12 MiB per
    /// generator.
    pub const CHUNK_DENSITY_CACHE_CAPACITY: usize = 16;

    /// Loads `data_root`, compiles the `settings_id` dimension, and binds
    /// the noise engine to `world_seed` using a private [`SharedChunkCaches`]
    /// instance.
    pub fn new(
        data_root: &Path,
        world_seed: i64,
        settings_id: &str,
    ) -> Result<Self, WorldgenError> {
        Self::with_shared_caches(
            data_root,
            world_seed,
            settings_id,
            Arc::new(SharedChunkCaches::new()),
        )
    }

    /// Loads `data_root`, compiles the `settings_id` dimension, binds the
    /// noise engine to `world_seed`, and attaches a shared cross-worker cache.
    pub fn with_shared_caches(
        data_root: &Path,
        world_seed: i64,
        settings_id: &str,
        shared: Arc<SharedChunkCaches>,
    ) -> Result<Self, WorldgenError> {
        let data = WorldgenData::load(data_root)?;
        let engine = data.engine(world_seed);
        let registry = data.registry(&engine);
        let router = data.router(&registry, settings_id)?;
        let max_y = router.min_y + router.height - 1;
        let fluids = GlobalFluid {
            sea_level: router.sea_level,
            sea_fluid: router.default_fluid,
        };
        let aquifer = match &router.aquifers {
            Some(config) => {
                let mut named = engine.positional().from_hash_of("minecraft:aquifer");
                Aquifer::NoiseBased(Box::new(NoiseBasedAquifer::new(
                    config.clone(),
                    fluids,
                    named.fork_positional(),
                )))
            }
            None => Aquifer::Disabled(fluids),
        };
        let climate = ClimateSampler {
            temperature: router.temperature.clone(),
            vegetation: router.vegetation.clone(),
            continents: router.continents.clone(),
            erosion: router.erosion.clone(),
            depth: router.depth.clone(),
            ridges: router.ridges.clone(),
        };
        let preset = settings_id.rsplit(':').next().unwrap_or(settings_id);
        let placement = BiomePlacement::load(data_root, preset);
        // The surface program compiles against the still-borrowed engine
        // and registry; the result owns everything it needs.
        let surface = match router.material_rule.clone() {
            Some(root_id) => Some(SurfaceRules::compile(
                &data, &engine, &registry, &router, &root_id,
            )?),
            None => None,
        };
        // Carver documents are optional: a pack without them (or without
        // a placement table to select biome carver lists) leaves the
        // replay inert and `substance` purely density-driven.
        let carver_data = CarverData::load(data_root)?;
        let carvers = (!carver_data.is_empty()).then_some(Rc::new(carver_data));
        let carver_context = CarverContext {
            min_y: router.min_y,
            gen_depth: router.height,
            sea_level: router.sea_level,
        };
        // Placement documents are optional in the same sense the carver
        // documents are: a pack without them leaves the generator with the
        // terrain, carving, and surface passes only.
        let mut feature_data = FeatureData::load(data_root)?;
        // The step schedules number their features over the biome source's
        // possible-biome set, and for this pack that set is the placement
        // table's own declaration order with repeats dropped.
        if let Some(placement) = placement.as_ref() {
            feature_data.set_biome_order(&placement.possible_biomes());
        }
        let features = (!feature_data.is_empty()).then_some(feature_data);
        Ok(Self {
            router,
            max_y,
            aquifer,
            climate,
            placement,
            surface,
            world_seed,
            carvers,
            carver_context,
            features,
            masks: RefCell::new(BoundedCache::new(Self::MASK_CACHE_CAPACITY)),
            chunk_carvers: RefCell::new(BoundedCache::new(Self::CHUNK_CARVERS_CACHE_CAPACITY)),
            heights: RefCell::new(BoundedCache::new(Self::HEIGHT_CACHE_CAPACITY)),
            ocean_floor_maps: RefCell::new(BoundedCache::new(Self::OCEAN_FLOOR_MAP_CACHE_CAPACITY)),
            biomes: RefCell::new(BoundedCache::new(Self::BIOME_CACHE_CAPACITY)),
            biome_regions: RefCell::new(BoundedCache::new(Self::BIOME_REGION_CACHE_CAPACITY)),
            chunks: RefCell::new(BoundedCache::new(Self::DECORATED_CHUNK_CACHE_CAPACITY)),
            chunk_density: RefCell::new(BoundedCache::new(Self::CHUNK_DENSITY_CACHE_CAPACITY)),
            shared,
            density_scratch: RefCell::new(Vec::new()),
            climate_scratch: RefCell::new(ClimateScratch::default()),
        })
    }

    fn can_serve_chunk_biomes(&self) -> bool {
        self.placement.is_some() && (self.router.min_y & 3) == 0 && (self.router.height & 3) == 0
    }

    fn biome_arc(&self, x: i32, z: i32, surface_y: i32) -> Option<Arc<str>> {
        let placement = self.placement.as_ref()?;
        if self.can_serve_chunk_biomes()
            && surface_y >= self.router.min_y
            && surface_y <= self.max_y
        {
            let chunk_key = (x >> 4, z >> 4);
            let local_qx = ((x >> 2) & 3) as usize;
            let local_qz = ((z >> 2) & 3) as usize;
            let local_qy = ((surface_y >> 2) - (self.router.min_y >> 2)) as usize;
            if let Some(chunk) = self.biome_regions.borrow_mut().get_mut(&chunk_key) {
                return chunk.at_local(local_qx, local_qy, local_qz);
            }
            if let Some(chunk) = self.shared.chunk_biomes.get(&chunk_key) {
                let found = chunk.at_local(local_qx, local_qy, local_qz);
                self.biome_regions.borrow_mut().insert(chunk_key, chunk);
                return found;
            }
        }
        let key = (x >> 2, surface_y >> 2, z >> 2);
        if let Some(value) = self.biomes.borrow_mut().get_mut(&key) {
            return value.clone();
        }
        if let Some(value) = self.shared.biomes.get(&key) {
            self.biomes.borrow_mut().insert(key, value.clone());
            return value;
        }
        let value = self
            .climate
            .biome(placement, x, surface_y, z)
            .map(Arc::<str>::from);
        self.shared.biomes.insert(key, value.clone());
        self.biomes.borrow_mut().insert(key, value.clone());
        value
    }

    /// Biome identifier for the column at `(x, z)`, resolved from the
    /// climate target at the surface cell. `None` when the operator has
    /// not provisioned a placement table for this preset.
    pub fn biome(&self, x: i32, z: i32, surface_y: i32) -> Option<String> {
        self.biome_arc(x, z, surface_y)
            .map(|name| name.as_ref().to_owned())
    }

    /// The distinct biomes and full `4 x (height/4) x 4` quart grid of one
    /// chunk, consulted from local L1 then shared L2 before computing.
    fn chunk_biomes(&self, chunk_x: i32, chunk_z: i32) -> Arc<ChunkBiomes> {
        let chunk_key = (chunk_x, chunk_z);
        if let Some(region) = self.biome_regions.borrow_mut().get_mut(&chunk_key) {
            return Arc::clone(region);
        }
        if let Some(shared) = self.shared.chunk_biomes.get(&chunk_key) {
            self.biome_regions
                .borrow_mut()
                .insert(chunk_key, Arc::clone(&shared));
            return shared;
        }
        let Some(placement) = self.placement.as_ref() else {
            let empty = Arc::new(ChunkBiomes::empty());
            self.shared
                .chunk_biomes
                .insert(chunk_key, Arc::clone(&empty));
            self.biome_regions
                .borrow_mut()
                .insert(chunk_key, Arc::clone(&empty));
            return empty;
        };
        let min_y = self.router.min_y;
        let quart_height = self.router.height / 4;
        let Some(volume) = DensityVolume::new(
            [4, quart_height, 4],
            [chunk_x * 16, min_y, chunk_z * 16],
            [4, 4, 4],
        ) else {
            return Arc::new(ChunkBiomes::empty());
        };
        let mut scratch_guard = self.climate_scratch.borrow_mut();
        let ClimateScratch {
            density,
            targets,
            column,
        } = &mut *scratch_guard;
        self.climate.targets_for_volume(&volume, density, targets);
        let mut built_grid = ChunkBiomeBuild {
            palette: Vec::new(),
            cells: vec![u16::MAX; volume.slot_count()],
        };
        let mut biomes = self.biomes.borrow_mut();
        // Iterate column by column with y innermost, grouping runs whose five
        // non-depth targets are identical so placement searches sort once per run.
        for local_z in 0..4 {
            let z = chunk_z * 16 + local_z * 4;
            let qz = z >> 2;
            for local_x in 0..4 {
                let x = chunk_x * 16 + local_x * 4;
                let qx = x >> 2;
                let base_slot = volume.index(local_x as usize, 0, local_z as usize);
                let col_targets = &targets[base_slot..base_slot + quart_height as usize];
                Self::resolve_biome_column(
                    placement,
                    col_targets,
                    (qx, qz, min_y, base_slot),
                    column,
                    &mut biomes,
                    &mut built_grid,
                );
            }
        }
        drop(biomes);
        drop(scratch_guard);
        let built = Arc::new(built_grid.finish(quart_height as usize));
        self.shared
            .chunk_biomes
            .insert(chunk_key, Arc::clone(&built));
        self.biome_regions
            .borrow_mut()
            .insert(chunk_key, Arc::clone(&built));
        built
    }

    /// Resolves one quart column's biome cells by grouping runs of equal
    /// non-depth targets and populating `biomes` and `built_grid` on misses.
    fn resolve_biome_column(
        placement: &BiomePlacement,
        col_targets: &[[i64; 6]],
        (qx, qz, min_y, base_slot): (i32, i32, i32, usize),
        scratch: &mut ColumnRunScratch,
        biomes: &mut BiomePointCache,
        built_grid: &mut ChunkBiomeBuild,
    ) {
        let mut offset = 0usize;
        while offset < col_targets.len() {
            let run_len = fixed_run_len(&col_targets[offset..]);
            let fixed = fixed_climate_coords(&col_targets[offset]);
            scratch.depths.clear();
            scratch.keys.clear();
            scratch.slots.clear();
            for (in_run, target) in col_targets[offset..offset + run_len].iter().enumerate() {
                let qy = (min_y + (offset + in_run) as i32 * 4) >> 2;
                let key = (qx, qy, qz);
                let cell_slot = base_slot + offset + in_run;
                if let Some(cached) = biomes.get_mut(&key) {
                    if let Some(name) = cached {
                        let (slot, _) = built_grid.intern(name.as_ref());
                        built_grid.cells[cell_slot] = slot;
                    }
                } else {
                    scratch.depths.push(target[4]);
                    scratch.keys.push(key);
                    scratch.slots.push(cell_slot);
                }
            }
            if !scratch.depths.is_empty() {
                placement.find_column(
                    fixed,
                    &scratch.depths,
                    &mut scratch.search,
                    &mut scratch.rows,
                );
                for ((&key, &cell_slot), &row_idx) in scratch
                    .keys
                    .iter()
                    .zip(scratch.slots.iter())
                    .zip(scratch.rows.iter())
                {
                    let found = placement.entry_id(row_idx);
                    if let Some(name) = found {
                        let (slot, arc_name) = built_grid.intern(name);
                        built_grid.cells[cell_slot] = slot;
                        biomes.insert(key, Some(arc_name));
                    } else {
                        biomes.insert(key, None);
                    }
                }
            }
            offset += run_len;
        }
    }

    /// The biome set one anchor chunk decorates with: the union of its own
    /// volume and the eight around it. The reference runtime retains only the
    /// biome source's possible biomes from that union, and every identifier
    /// the placement table can return is in its own possible set, so the
    /// retention is a no-op here.
    fn region_biomes(&self, chunk_x: i32, chunk_z: i32) -> Vec<Arc<str>> {
        let mut names = Vec::new();
        for anchor_z in chunk_z - 1..=chunk_z + 1 {
            for anchor_x in chunk_x - 1..=chunk_x + 1 {
                names.extend(self.chunk_biomes(anchor_x, anchor_z).region.iter().cloned());
            }
        }
        names.sort_unstable();
        names.dedup();
        names
    }

    /// The loaded feature registry for this dimension, or `None` when the
    /// data root carries no worldgen feature documents.
    pub fn features(&self) -> Option<&FeatureData> {
        self.features.as_ref()
    }

    /// Distinct biomes present in the 4x4x4 quart-cell volume of the chunks
    /// covering the inclusive block rectangle `[min_x..=max_x] x [min_z..=max_z]`,
    /// sorted ascending and deduplicated.
    pub fn biomes_in_square(&self, min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> Vec<String> {
        if min_x > max_x || min_z > max_z {
            return Vec::new();
        }
        let mut names = Vec::new();
        for chunk_z in (min_z >> 4)..=(max_z >> 4) {
            for chunk_x in (min_x >> 4)..=(max_x >> 4) {
                names.extend(
                    self.chunk_biomes(chunk_x, chunk_z)
                        .region
                        .iter()
                        .map(|name| name.as_ref().to_owned()),
                );
            }
        }
        names.sort_unstable();
        names.dedup();
        names
    }

    /// The block id left at the column's highest non-air position by the
    /// dimension's surface rules. Fluid tops report the dimension fluid;
    /// a solid top that matches no rule keeps the filler default block
    /// (the settings `default_block`, stone in the overworld). `None`
    /// when the dimension has no material rules or the column is empty.
    pub fn top_block(&self, x: i32, z: i32) -> Option<String> {
        let rules = self.surface.as_ref()?;
        let (top_y, substance) = self.surface(x, z)?;
        match substance {
            Substance::Air => None,
            Substance::Fluid(fluid) => Some(Self::fluid_name(fluid).to_owned()),
            Substance::Solid => {
                // The documented descent reaches the top solid with a
                // stone depth of one, no water column above, and the
                // below-depth from the first non-solid position down.
                let chunk = self.chunk_density(x >> 4, z >> 4);
                let col = chunk.get(self.column_density_range(x, z)).unwrap_or(&[]);
                let mut ceiling = WAY_BELOW_MIN_Y;
                for lookahead in (self.router.min_y..top_y).rev() {
                    let value = self.column_density_value(col, x, lookahead, z);
                    let below = self.aquifer.substance(x, lookahead, z, value);
                    if below != Substance::Solid {
                        ceiling = lookahead + 1;
                        break;
                    }
                }
                let stone_below = top_y - ceiling + 1;
                let mut ctx = SurfaceContext::new(
                    rules,
                    self.router.chunk_surface_level.as_ref(),
                    x,
                    z,
                    |hx, hz| self.surface_height(hx, hz),
                    |bx, by, bz| self.biome(bx, bz, by),
                );
                ctx.set_y(1, stone_below, None, top_y);
                Some(
                    rules
                        .apply(&mut ctx)
                        .unwrap_or_else(|| self.router.default_block.clone()),
                )
            }
        }
    }

    /// The dimension fluid's block id (the settings `default_fluid`
    /// family: water or lava in the overworld).
    fn fluid_name(fluid: Fluid) -> &'static str {
        match fluid {
            Fluid::Lava => "minecraft:lava",
            Fluid::Water => "minecraft:water",
        }
    }

    /// The full material-rule descent of one column plus everything the
    /// placement stage painted over it: the generator's block-id answer, and
    /// index 0 of the result is the dimension's minimum build Y.
    ///
    /// Terrain, carving, and surface stay as the earlier passes left them
    /// (`substance`, `carved`, and `top_block` all answer pre-feature by
    /// design, so the 3D substance metric keeps comparing the same thing);
    /// only the ids a chunk is shipped with carry veins.
    pub fn column_ids(&self, x: i32, z: i32) -> Vec<Option<String>> {
        let chunk = self.decorated_chunk(x >> 4, z >> 4);
        chunk.column(x & 15, z & 15)
    }

    /// The decorated grid of one chunk, built once and cached: 256 base
    /// columns, then every placement pass that can reach into it.
    fn decorated_chunk(&self, chunk_x: i32, chunk_z: i32) -> Rc<DecoratedChunk> {
        let key = (chunk_x, chunk_z);
        if let Some(existing) = self.chunks.borrow_mut().get_mut(&key) {
            return Rc::clone(existing);
        }
        let built = Rc::new(self.build_decorated_chunk(chunk_x, chunk_z));
        self.chunks.borrow_mut().insert(key, Rc::clone(&built));
        built
    }

    /// Builds one chunk's grid.
    ///
    /// A vein's own blocks can lie up to twelve or thirteen blocks from the
    /// chunk it was seeded in — measured from the owner's save, three quarters
    /// of stone blobs cross a chunk border and a blob covers about three
    /// chunks — so a chunk is decorated by replaying the placement passes of
    /// its own chunk *and* the eight around it, keeping only the writes that
    /// land inside. Every one of those replays is a pure function of the
    /// anchor chunk's coordinates, the seed, and the step and ordinal numbers
    /// the pack gives it, so the answer does not depend on which chunk was
    /// asked for first: two neighbouring chunks agree about the border because
    /// each replays the same anchor with the same seed and clips to itself.
    ///
    /// Within one replay the order is ours: anchor chunks row-major from the
    /// northwest of the 3×3, then the pack's generation steps, then the step's
    /// feature ordinals. The reference runtime mutates a shared region and
    /// leaves the cross-chunk order to whatever the worker queue did, so where
    /// two veins compete for one position the winner can differ; `docs/
    /// PROVENANCE.md` records that as a deviation rather than a parity claim.
    fn build_decorated_chunk(&self, chunk_x: i32, chunk_z: i32) -> DecoratedChunk {
        if self.can_serve_chunk_biomes() {
            let _ = self.chunk_biomes(chunk_x, chunk_z);
        }
        let height = self.router.height as usize;
        let mut chunk = DecoratedChunk::new(height);
        let mut surface_map = [self.router.min_y; 256];
        let mut floor_map = [self.router.min_y; 256];
        for local_x in 0..16 {
            for local_z in 0..16 {
                let x = chunk_x * 16 + local_x;
                let z = chunk_z * 16 + local_z;
                let (ids, top_y, floor_y) = self.base_column_ids_with_heights(x, z);
                let map_slot = OceanFloorMap::slot(x, z);
                surface_map[map_slot] = top_y;
                floor_map[map_slot] = floor_y;
                let mut col_top = -1i16;
                for (row, name) in ids.into_iter().enumerate() {
                    let at = chunk.index(local_x, row, local_z);
                    let slot = chunk.intern(name);
                    chunk.blocks[at] = slot;
                    if chunk.palette_non_air[slot as usize] {
                        col_top = row as i16;
                    }
                }
                chunk.top_non_air[(local_x as usize) * 16 + local_z as usize] = col_top;
            }
        }
        let chunk_key = (chunk_x, chunk_z);
        self.shared
            .surface_height_maps
            .insert(chunk_key, Arc::new(surface_map));
        self.shared
            .ocean_floor_maps
            .insert(chunk_key, Arc::new(floor_map));
        let Some(features) = self.features.as_ref() else {
            return chunk;
        };
        let steps = features.decorated_steps();
        if steps.is_empty() {
            return chunk;
        }
        let halo = RefCell::new(HaloColumns::new());
        for anchor_z in chunk_z - 1..=chunk_z + 1 {
            for anchor_x in chunk_x - 1..=chunk_x + 1 {
                let region = self.region_biomes(anchor_x, anchor_z);
                let mut view = ChunkView {
                    generator: self,
                    chunk_x,
                    chunk_z,
                    chunk: &mut chunk,
                    halo: &halo,
                };
                for &step in &steps {
                    features.decorate_step(
                        &mut view,
                        step,
                        anchor_x,
                        anchor_z,
                        self.world_seed,
                        &region,
                    );
                }
            }
        }
        chunk
    }

    #[cfg(test)]
    fn base_column_ids(&self, x: i32, z: i32) -> Vec<Option<String>> {
        self.base_column_ids_with_heights(x, z).0
    }

    /// Resolves one solid pre-decoration block id at `(x, y, z)` using the
    /// same top-down descent counters (`stone_above`, `stone_below`, `water_height`)
    /// as [`Self::base_column_ids_with_heights`], without evaluating surface rules
    /// on lower rows or allocating a full column vector.
    fn base_solid_block_at(&self, x: i32, y: i32, z: i32) -> String {
        let Some(rules) = self.surface.as_ref() else {
            return self.router.default_block.clone();
        };
        let min_y = self.router.min_y;
        let top = self.surface_height(x, z);
        if y > top {
            return self.router.default_block.clone();
        }
        let chunk = self.chunk_density(x >> 4, z >> 4);
        let col = chunk.get(self.column_density_range(x, z)).unwrap_or(&[]);
        let mut stone_above = 0i32;
        let mut water_height: Option<i32> = None;
        for cy in (y..=top).rev() {
            let value = self.column_density_value(col, x, cy, z);
            match self.aquifer.substance(x, cy, z, value) {
                Substance::Air => {
                    stone_above = 0;
                    water_height = None;
                }
                Substance::Fluid(_) => {
                    if water_height.is_none() {
                        water_height = Some(cy + 1);
                    }
                }
                Substance::Solid => {
                    stone_above += 1;
                }
            }
        }
        let mut run_bottom = WAY_BELOW_MIN_Y;
        let mut lookahead = y - 1;
        while lookahead >= min_y {
            let value = self.column_density_value(col, x, lookahead, z);
            if self.aquifer.substance(x, lookahead, z, value) != Substance::Solid {
                run_bottom = lookahead + 1;
                break;
            }
            lookahead -= 1;
        }
        let mut ctx = SurfaceContext::new(
            rules,
            self.router.chunk_surface_level.as_ref(),
            x,
            z,
            |hx, hz| self.surface_height(hx, hz),
            |bx, by, bz| self.biome(bx, bz, by),
        );
        ctx.set_y(stone_above, y - run_bottom + 1, water_height, y);
        rules
            .apply(&mut ctx)
            .unwrap_or_else(|| self.router.default_block.clone())
    }

    /// The full material-rule descent of one column, the documented
    /// `buildSurface` pass (`PROVENANCE.md` session 7): from the highest
    /// non-air row walk down, reset the stone-above counter and the water
    /// latch at air, latch the water height at the first fluid row below
    /// an air gap, and evaluate the rule program at every solid row with
    /// the current counters and the below-depth from the contiguous solid
    /// run. Registry carvers run after this pass in the documented order,
    /// so carved rows report their post-carve fluid or cave air and are
    /// never recolored by a rule.
    ///
    /// Index 0 of the result is the dimension's minimum build Y. Rows
    /// above the column top and pre-carve air rows are `None`; solid rows
    /// with no matching rule keep the filler `default_block`. Also returns
    /// `(surface_height, ocean_floor_height)` for chunk-level map publishing.
    fn base_column_ids_with_heights(&self, x: i32, z: i32) -> (Vec<Option<String>>, i32, i32) {
        let min_y = self.router.min_y;
        let mut ids: Vec<Option<String>> = vec![None; self.router.height as usize];
        // The column as the surface pass sees it: the registry carvers
        // have not stamped it yet. Keep the first non-air row found by the
        // top-down scan and retain the rest; this avoids sampling the first
        // non-air row a second time.
        let chunk = self.chunk_density(x >> 4, z >> 4);
        let col = chunk.get(self.column_density_range(x, z)).unwrap_or(&[]);
        let mut top = None;
        let mut filled = Vec::with_capacity(self.router.height as usize);
        for y in (min_y..=self.max_y).rev() {
            let value = self.column_density_value(col, x, y, z);
            let substance = self.aquifer.substance(x, y, z, value);
            if top.is_none() && substance != Substance::Air {
                top = Some(y);
            }
            if top.is_some() {
                filled.push(substance);
            }
        }
        let Some(top) = top else {
            self.record_ocean_floor(x, z, min_y);
            return (ids, min_y, min_y);
        };
        filled.reverse();
        self.heights.borrow_mut().insert((x, z), top);
        let mut ctx = self.surface.as_ref().map(|rules| {
            (
                rules,
                SurfaceContext::new(
                    rules,
                    self.router.chunk_surface_level.as_ref(),
                    x,
                    z,
                    |hx, hz| self.surface_height(hx, hz),
                    |bx, by, bz| self.biome(bx, bz, by),
                ),
            )
        });
        let mut stone_above = 0i32;
        let mut water_height: Option<i32> = None;
        let mut run_bottom = WAY_BELOW_MIN_Y;
        let mut in_run = false;
        for offset in (0..filled.len()).rev() {
            let y = min_y + offset as i32;
            match filled[offset] {
                Substance::Air => {
                    stone_above = 0;
                    water_height = None;
                    in_run = false;
                }
                Substance::Fluid(fluid) => {
                    if water_height.is_none() {
                        water_height = Some(y + 1);
                    }
                    in_run = false;
                    ids[offset] = Some(Self::fluid_name(fluid).to_owned());
                }
                Substance::Solid => {
                    if !in_run {
                        in_run = true;
                        // Downward lookahead to the first non-solid row;
                        // the sentinel floor when the run reaches the
                        // bottom of the world.
                        run_bottom = WAY_BELOW_MIN_Y;
                        let mut lookahead = y - 1;
                        while lookahead >= min_y {
                            if filled[(lookahead - min_y) as usize] != Substance::Solid {
                                run_bottom = lookahead + 1;
                                break;
                            }
                            lookahead -= 1;
                        }
                    }
                    stone_above += 1;
                    ids[offset] = Some(match &mut ctx {
                        Some((rules, ctx)) => {
                            ctx.set_y(stone_above, y - run_bottom + 1, water_height, y);
                            rules
                                .apply(ctx)
                                .unwrap_or_else(|| self.router.default_block.clone())
                        }
                        None => self.router.default_block.clone(),
                    });
                }
            }
        }
        // The registry carving pass overwrites whatever the surface pass
        // left at its positions. The density-0 sample is never solid in
        // the overworld aquifer, so a solid post-carve answer keeps the
        // surface result untouched.
        for (offset, id) in ids.iter_mut().enumerate().take(filled.len()) {
            let y = min_y + offset as i32;
            if self.carved(x, y, z) {
                *id = match self.aquifer.substance(x, y, z, 0.0) {
                    Substance::Air => Some("minecraft:cave_air".to_owned()),
                    Substance::Fluid(fluid) => Some(Self::fluid_name(fluid).to_owned()),
                    Substance::Solid => id.take(),
                };
            }
        }
        // The decoration-time ocean-floor heightmap row for this column,
        // read off the substances this pass already sampled: from the same
        // surface top, down past every row the post-carve substance does
        // not count as terrain — the walk `ocean_floor_by_descent` makes
        // from `substance`, which for a column fill has in hand is exactly
        // `filled` where the carvers left the row and the density-0
        // aquifer answer where they stamped it. The anchor gate then reads
        // this column's row from the chunk map instead of descending the
        // density graph over the fluid above its sea floor a second time.
        let mut floor = top;
        while floor > min_y {
            let solid = if self.carved(x, floor, z) {
                matches!(self.aquifer.substance(x, floor, z, 0.0), Substance::Solid)
            } else {
                filled[(floor - min_y) as usize] == Substance::Solid
            };
            if solid {
                break;
            }
            floor -= 1;
        }
        self.record_ocean_floor(x, z, floor);
        (ids, top, floor)
    }

    /// Absolute Y of the top written block: terrain, or the aquifer/sea
    /// fluid surface where terrain does not reach above it. Memoised
    /// because the descent's steep gradients re-read neighbours.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        if let Some(y) = self.heights.borrow_mut().get_mut(&(x, z)) {
            return *y;
        }
        let chunk_key = (x >> 4, z >> 4);
        if let Some(map) = self.shared.surface_height_maps.get(&chunk_key) {
            let y = map[OceanFloorMap::slot(x, z)];
            self.heights.borrow_mut().insert((x, z), y);
            return y;
        }
        if let Some(y) = self.shared.surface_heights.get(&(x, z)) {
            self.heights.borrow_mut().insert((x, z), y);
            return y;
        }
        let y = self.surface(x, z).map_or(self.router.min_y, |(y, _)| y);
        self.shared.surface_heights.insert((x, z), y);
        self.heights.borrow_mut().insert((x, z), y);
        y
    }

    /// Absolute Y of the highest terrain row of the column as its world-gen
    /// ocean-floor heightmap has it: the surface top, walked down past the
    /// fluid and air the filler and the carvers leave above a sea floor. Rows
    /// the reference counts are the ones its `blocks_motion_in_heightmap` tag
    /// holds, which for a pre-decoration column is every solid the density
    /// filler and surface rules can write and neither water nor lava.
    ///
    /// Answers come from the column's chunk heightmap: the chunk fill pass
    /// derives all 256 rows from the substances it sampled for the column's
    /// block ids, and a gate lookup into a chunk nobody has filled yet adds
    /// just the column it needs, by the `ocean_floor_by_descent` walk.
    /// Every row is therefore computed once while the density graph has to
    /// be read for that column anyway, and the anchor gate's footprint
    /// scans are map reads. Evicting a map costs only recomputation: each
    /// row is a pure function of the column's coordinates and the seed.
    pub fn ocean_floor_height(&self, x: i32, z: i32) -> i32 {
        let key = (x >> 4, z >> 4);
        let slot = OceanFloorMap::slot(x, z);
        let served = {
            let mut maps = self.ocean_floor_maps.borrow_mut();
            maps.get_mut(&key).and_then(|map| {
                (map.entries[slot] != UNCOMPUTED_OCEAN_FLOOR).then_some(map.entries[slot])
            })
        };
        if let Some(y) = served {
            return y;
        }
        if let Some(full) = self.shared.ocean_floor_maps.get(&key) {
            let y = full[slot];
            self.ocean_floor_maps
                .borrow_mut()
                .insert(key, OceanFloorMap { entries: *full });
            return y;
        }
        if let Some(y) = self.shared.ocean_floor_cols.get(&(x, z)) {
            self.record_ocean_floor(x, z, y);
            return y;
        }
        let y = self.ocean_floor_by_descent(x, z);
        self.shared.ocean_floor_cols.insert((x, z), y);
        self.record_ocean_floor(x, z, y);
        y
    }

    /// The heightmap row of one column recomputed from scratch: the top the
    /// surface reports, walked down past every row the post-carve substance
    /// does not count as terrain. This is what a gate lookup into an
    /// uncomputed heightmap slot falls back to, and the identity reference
    /// the fill pass's derivation is tested against.
    fn ocean_floor_by_descent(&self, x: i32, z: i32) -> i32 {
        let mut y = self.surface_height(x, z);
        while y > self.min_y() && self.substance(x, y, z) != Substance::Solid {
            y -= 1;
        }
        y
    }

    /// Writes one column's heightmap row into its chunk's map, creating the
    /// map when this is the chunk's first stored row.
    fn record_ocean_floor(&self, x: i32, z: i32, y: i32) {
        let key = (x >> 4, z >> 4);
        let slot = OceanFloorMap::slot(x, z);
        let mut maps = self.ocean_floor_maps.borrow_mut();
        if let Some(map) = maps.get_mut(&key) {
            map.entries[slot] = y;
        } else {
            let mut map = OceanFloorMap::new();
            map.entries[slot] = y;
            maps.insert(key, map);
        }
    }

    /// The dimension's minimum build height: index 0 of `column_ids`.
    pub fn min_y(&self) -> i32 {
        self.router.min_y
    }

    /// Raw `final_density` sample before the aquifer adjustment: the
    /// single-column probe prints it to reason about graph branches.
    pub fn raw_density(&self, x: i32, y: i32, z: i32) -> f32 {
        self.router.final_density.sample(x, y, z)
    }

    /// Raw `final_density` values for one chunk (`16 x height x 16` floats in
    /// `DensityVolume::index` order), filled once by `sample_volume` and
    /// cached across workers.
    fn chunk_density(&self, chunk_x: i32, chunk_z: i32) -> ChunkDensities {
        let key = (chunk_x, chunk_z);
        if let Some(existing) = self.chunk_density.borrow_mut().get_mut(&key) {
            return Arc::clone(existing);
        }
        if let Some(existing) = self.shared.chunk_density.get(&key) {
            self.chunk_density
                .borrow_mut()
                .insert(key, Arc::clone(&existing));
            return existing;
        }
        let Some(volume) = DensityVolume::new(
            [16, self.router.height, 16],
            [chunk_x * 16, self.router.min_y, chunk_z * 16],
            [1, 1, 1],
        ) else {
            return Arc::from([]);
        };
        let mut scratch = self.density_scratch.borrow_mut();
        scratch.resize(volume.slot_count(), 0.0);
        self.router
            .final_density
            .sample_volume(&volume, &mut scratch);
        let filled: ChunkDensities = Arc::from(scratch.as_slice());
        self.shared.chunk_density.insert(key, Arc::clone(&filled));
        self.chunk_density
            .borrow_mut()
            .insert(key, Arc::clone(&filled));
        filled
    }

    /// Slice range inside a chunk's density array for the column at `(x, z)`.
    fn column_density_range(&self, x: i32, z: i32) -> std::ops::Range<usize> {
        let height = self.router.height as usize;
        let local_x = (x & 15) as usize;
        let local_z = (z & 15) as usize;
        let start = (local_x + local_z * 16) * height;
        start..start + height
    }

    /// Indexes a column slice from [`Self::chunk_density`] for in-range `y`
    /// and falls back to point evaluation outside `[min_y, max_y]`.
    fn column_density_value(&self, col: &[f32], x: i32, y: i32, z: i32) -> f32 {
        if y >= self.router.min_y
            && let Some(&value) = col.get((y - self.router.min_y) as usize)
        {
            return value;
        }
        self.router.final_density.sample(x, y, z)
    }

    /// Cached `final_density` at `(x, y, z)`, indexing the per-chunk volume
    /// fill inside `[min_y, max_y]` and falling back to point evaluation
    /// outside the dimension build height.
    fn density_at(&self, x: i32, y: i32, z: i32) -> f32 {
        if y < self.router.min_y || y > self.max_y {
            return self.router.final_density.sample(x, y, z);
        }
        let chunk = self.chunk_density(x >> 4, z >> 4);
        let height = self.router.height as usize;
        let local_x = (x & 15) as usize;
        let local_y = (y - self.router.min_y) as usize;
        let local_z = (z & 15) as usize;
        let index = local_y + (local_x + local_z * 16) * height;
        chunk
            .get(index)
            .copied()
            .unwrap_or_else(|| self.router.final_density.sample(x, y, z))
    }

    /// The filler substance at one absolute block position: the raw
    /// `final_density` sample through the runtime aquifer, replaced by
    /// the aquifer's answer at density `0.0` where a registry carver
    /// carved the position (the apply-carving-mask rewrite). This is
    /// what the 3D agreement metric compares against the save.
    pub fn substance(&self, x: i32, y: i32, z: i32) -> Substance {
        if self.carved(x, y, z) {
            return self.aquifer.substance(x, y, z, 0.0);
        }
        let density = self.density_at(x, y, z);
        self.aquifer.substance(x, y, z, density)
    }

    /// Whether a registry carver stamped this absolute position in its
    /// target chunk's carving mask. The mask is replayed once per chunk
    /// over the 17×17 source window and cached.
    pub fn carved(&self, x: i32, y: i32, z: i32) -> bool {
        if self.carvers.is_none() || self.placement.is_none() {
            return false;
        }
        let target = (x >> 4, z >> 4);
        let (relative_x, relative_z) = (x - (target.0 << 4), z - (target.1 << 4));
        {
            let mut cached = self.masks.borrow_mut();
            if let Some(mask) = cached.get_mut(&target) {
                return mask.contains(relative_x, y, relative_z);
            }
        }
        if let Some(mask) = self.shared.masks.get(&target) {
            let hit = mask.contains(relative_x, y, relative_z);
            self.masks.borrow_mut().insert(target, mask);
            return hit;
        }
        let mask = Arc::new(self.build_carve_mask(target));
        let hit = mask.contains(relative_x, y, relative_z);
        self.shared.masks.insert(target, Arc::clone(&mask));
        self.masks.borrow_mut().insert(target, mask);
        hit
    }

    /// The Y window a target chunk's mask covers, as block rows
    /// `(min_y, max_y)`. Single source of truth because `carve_mask_bytes`
    /// has to price exactly the bitset `build_carve_mask` allocates.
    fn mask_window(&self) -> (i32, i32) {
        (
            self.router.min_y + 1,
            self.router.min_y + self.router.height - 1 - 7,
        )
    }

    /// Replays the orchestration for one target chunk: one legacy stream
    /// reseeded per source chunk and carver index over the 17×17 window,
    /// the probability gate, then the walk stamping into the mask.
    fn build_carve_mask(&self, target: (i32, i32)) -> CarveMask {
        let (mask_min_y, mask_max_y) = self.mask_window();
        let mut mask = CarveMask::new(mask_min_y, mask_max_y);
        let mut random = LegacyRandom::new(0);
        for dx in -8..=8 {
            for dz in -8..=8 {
                let source = (target.0 + dx, target.1 + dz);
                let Some(carvers) = self.carvers_for_chunk(source) else {
                    continue;
                };
                for (index, carver) in carvers.iter().enumerate() {
                    let Some(carver) = carver else {
                        continue;
                    };
                    random.set_large_feature_seed(
                        self.world_seed + index as i64,
                        source.0,
                        source.1,
                    );
                    if carver.is_start_chunk(&mut random) {
                        carver.carve(&self.carver_context, &mut random, target, source, &mut mask);
                    }
                }
            }
        }
        mask
    }

    /// The biome's flat carver list for one source chunk, cached; `None`
    /// when the data root has no carvers or placement table, or the
    /// biome resolves without a carver list.
    fn carvers_for_chunk(&self, chunk: (i32, i32)) -> Option<ChunkCarvers> {
        {
            let mut cached = self.chunk_carvers.borrow_mut();
            if let Some(carvers) = cached.get_mut(&chunk) {
                return Some(Rc::clone(carvers));
            }
        }
        let data = self.carvers.as_ref()?;
        let biome = self.biome_arc(chunk.0 << 4, chunk.1 << 4, 0)?;
        let carvers = data.carvers_for_biome(&biome)?;
        self.chunk_carvers
            .borrow_mut()
            .insert(chunk, Rc::clone(&carvers));
        Some(carvers)
    }

    /// The highest non-air position in the column and what fills it.
    pub fn surface(&self, x: i32, z: i32) -> Option<(i32, Substance)> {
        let chunk = self.chunk_density(x >> 4, z >> 4);
        let col = chunk.get(self.column_density_range(x, z)).unwrap_or(&[]);
        for y in (self.router.min_y..=self.max_y).rev() {
            let value = self.column_density_value(col, x, y, z);
            let substance = self.aquifer.substance(x, y, z, value);
            if substance != Substance::Air {
                return Some((y, substance));
            }
        }
        None
    }

    /// Absolute Y of the top *terrain* (solid) block, ignoring fluids:
    /// the density-only scan kept for diagnostics.
    pub fn terrain_top_exhaustive(&self, x: i32, z: i32) -> i32 {
        let chunk = self.chunk_density(x >> 4, z >> 4);
        let col = chunk.get(self.column_density_range(x, z)).unwrap_or(&[]);
        for y in (self.router.min_y..=self.max_y).rev() {
            if self.column_density_value(col, x, y, z) > 0.0 {
                return y;
            }
        }
        self.router.min_y
    }

    /// The dimension's build height: the `noise.height` setting, the
    /// number of rows `column_ids` indexes.
    pub fn height(&self) -> i32 {
        self.router.height
    }

    /// Heap bytes of one chunk's carve mask bitset: the per-target-chunk
    /// value the mask cache stores, used to price the cache bound.
    pub fn carve_mask_bytes(&self) -> usize {
        let (mask_min_y, mask_max_y) = self.mask_window();
        CarveMask::heap_bytes(mask_min_y, mask_max_y)
    }

    /// Current occupancy of every coordinate-keyed cache of this generator.
    /// Diagnostic and test surface: the bounded caches must stay inside the
    /// capacities documented on each field.
    pub fn cache_occupancy(&self) -> CacheOccupancy {
        let aquifer = match &self.aquifer {
            Aquifer::NoiseBased(aquifer) => aquifer.cache_occupancy(),
            Aquifer::Disabled(_) => AquiferOccupancy::default(),
        };
        CacheOccupancy {
            masks: self.masks.borrow().entries(),
            chunk_carvers: self.chunk_carvers.borrow().entries(),
            heights: self.heights.borrow().entries(),
            ocean_floor_maps: self.ocean_floor_maps.borrow().entries(),
            biomes: self.biomes.borrow().entries(),
            biome_regions: self.biome_regions.borrow().entries(),
            aquifer_centers: aquifer.centers,
            aquifer_statuses: aquifer.statuses,
            aquifer_surface_levels: aquifer.surface_levels,
            aquifer_skip_bounds: aquifer.skip_bounds,
            decorated_chunks: self.chunks.borrow().entries(),
            chunk_densities: self.chunk_density.borrow().entries(),
        }
    }

    /// Grid-refined alternative scan (see module docs: an approximation
    /// that is value-exact only for graphs whose top-level `min` operands
    /// are also interpolated at the same y cell).
    pub fn terrain_top(&self, x: i32, z: i32) -> i32 {
        let chunk = self.chunk_density(x >> 4, z >> 4);
        let col = chunk.get(self.column_density_range(x, z)).unwrap_or(&[]);
        let bottom_grid = self.router.min_y.div_euclid(8) * 8;
        let mut grid_y = self.max_y.div_euclid(8) * 8;
        while grid_y >= bottom_grid {
            if self.column_density_value(col, x, grid_y, z) > 0.0 {
                // Refine the cell above the first positive grid sample.
                let mut probe = (grid_y + 7).min(self.max_y);
                while probe > grid_y {
                    if self.column_density_value(col, x, probe, z) > 0.0 {
                        return probe;
                    }
                    probe -= 1;
                }
                return grid_y;
            }
            grid_y -= 8;
        }
        // No solid grid sample: the floor is the only guaranteed block.
        self.router.min_y
    }
}

/// The air families a decoration pass's neighbour test treats as air: the
/// filler writes the named cave air at carved rows, and the pre-carve `None`
/// entry stands for the air above the terrain.
const AIR_IDS: [&str; 3] = ["minecraft:air", "minecraft:cave_air", "minecraft:void_air"];

impl DecorationTarget for ChunkView<'_> {
    fn min_y(&self) -> i32 {
        self.generator.router.min_y
    }

    fn height(&self) -> i32 {
        self.generator.router.height
    }

    fn writable(&self, x: i32, y: i32, z: i32) -> bool {
        (x >> 4, z >> 4) == (self.chunk_x, self.chunk_z)
            && y >= self.generator.router.min_y
            && y <= self.generator.max_y
    }

    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<&str> {
        if !self.writable(x, y, z) {
            // Never asked: the pass tests `writable` before it reads.
            return None;
        }
        let row = (y - self.generator.router.min_y) as usize;
        let at = self.chunk.index(x & 15, row, z & 15);
        self.chunk.name(self.chunk.blocks[at])
    }

    fn read_block(&self, x: i32, y: i32, z: i32) -> Option<Cow<'_, str>> {
        if y < self.generator.router.min_y || y > self.generator.max_y {
            return None;
        }
        if self.writable(x, y, z) {
            let named = self.block_at(x, y, z).unwrap_or("minecraft:air");
            return Some(Cow::Borrowed(named));
        }
        if let Some(cached) = self.halo.borrow().get(&(x, y, z)) {
            return Some(Cow::Owned(cached.as_ref().to_owned()));
        }
        match self.generator.substance(x, y, z) {
            Substance::Air => {
                let named = if self.generator.carved(x, y, z) {
                    "minecraft:cave_air"
                } else {
                    "minecraft:air"
                };
                Some(Cow::Borrowed(named))
            }
            Substance::Fluid(fluid) => Some(Cow::Borrowed(VanillaGenerator::fluid_name(fluid))),
            Substance::Solid => {
                let mut halo = self.halo.borrow_mut();
                if halo.len() >= HALO_COLUMN_LIMIT && !halo.contains_key(&(x, y, z)) {
                    halo.clear();
                }
                let entry = halo.entry((x, y, z)).or_insert_with(|| {
                    Rc::<str>::from(self.generator.base_solid_block_at(x, y, z))
                });
                Some(Cow::Owned(entry.as_ref().to_owned()))
            }
        }
    }

    fn is_air(&self, x: i32, y: i32, z: i32) -> bool {
        if self.writable(x, y, z) {
            return match self.block_at(x, y, z) {
                None => true,
                Some(name) => AIR_IDS.contains(&name),
            };
        }
        // A border block's neighbour lies in the next chunk, whose veins this
        // pass has not replayed. The underlying terrain answers instead: the
        // air test only ever rejects a placement, and a cave that a neighbour
        // vein opened is not a stone target either way.
        self.generator.substance(x, y, z) == Substance::Air
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, name: &str) {
        let row = (y - self.generator.router.min_y) as usize;
        self.chunk.set(x & 15, row, z & 15, name);
    }

    fn writable_bounds(&self) -> Option<(i32, i32, i32, i32)> {
        let min_x = self.chunk_x << 4;
        let min_z = self.chunk_z << 4;
        Some((min_x, min_x + 15, min_z, min_z + 15))
    }

    fn biome_at(&self, _x: i32, y: i32, z: i32) -> Option<String> {
        self.generator.biome(_x, z, y)
    }

    fn has_biome(&self, x: i32, y: i32, z: i32, allowed: &dyn Fn(&str) -> bool) -> bool {
        self.generator
            .biome_arc(x, z, y)
            .is_some_and(|biome| allowed(biome.as_ref()))
    }

    fn ocean_floor_height(&self, x: i32, z: i32) -> i32 {
        self.generator.ocean_floor_height(x, z)
    }

    fn heightmap_height(
        &self,
        kind: crate::vanilla::feature::HeightmapKind,
        x: i32,
        z: i32,
        tags: &crate::vanilla::feature::BlockTags,
    ) -> i32 {
        use crate::vanilla::feature::HeightmapKind;
        let min_y = self.generator.router.min_y;
        if (x >> 4, z >> 4) == (self.chunk_x, self.chunk_z) {
            let col = ((x & 15) as usize) * 16 + (z & 15) as usize;
            let top_row = self.chunk.top_non_air[col];
            if top_row < 0 {
                return min_y;
            }
            if matches!(
                kind,
                HeightmapKind::WorldSurface | HeightmapKind::WorldSurfaceWg
            ) {
                return min_y + i32::from(top_row) + 1;
            }
            for row in (0..=top_row as usize).rev() {
                let slot = self.chunk.blocks[row * 256 + col];
                if slot == 0 {
                    continue;
                }
                if let Some(name) = self.chunk.name(slot)
                    && kind.matches_block(name, tags)
                {
                    return min_y + row as i32 + 1;
                }
            }
            return min_y;
        }
        // Neighbour columns outside this chunk hold only pre-decoration terrain
        // and fluids, so heightmaps resolve directly from cached floor heights
        // and substance descent without building surface-rule string columns.
        match kind {
            HeightmapKind::OceanFloor | HeightmapKind::OceanFloorWg => {
                let floor = self.generator.ocean_floor_height(x, z);
                if floor > min_y || self.generator.substance(x, min_y, z) == Substance::Solid {
                    floor + 1
                } else {
                    min_y
                }
            }
            HeightmapKind::WorldSurface
            | HeightmapKind::WorldSurfaceWg
            | HeightmapKind::MotionBlocking
            | HeightmapKind::MotionBlockingNoLeaves => {
                let mut y = self.generator.surface_height(x, z);
                while y >= min_y && self.generator.substance(x, y, z) == Substance::Air {
                    y -= 1;
                }
                if y >= min_y { y + 1 } else { min_y }
            }
        }
    }
}

#[cfg(test)]
mod tests;
