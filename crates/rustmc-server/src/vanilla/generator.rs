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
}

mod biomes;
mod columns;
mod terrain;
mod view;
use view::*;

#[cfg(test)]
mod tests;
