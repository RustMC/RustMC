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

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::rc::Rc;

use crate::vanilla::aquifer::{
    Aquifer, AquiferOccupancy, Fluid, GlobalFluid, NoiseBasedAquifer, Substance,
};
use crate::vanilla::biome::{BiomePlacement, ClimateSampler};
use crate::vanilla::cache::BoundedCache;
use crate::vanilla::carver::{CarveMask, Carver, CarverContext, CarverData};
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
    /// Which palette entry each position holds, laid out y-outer so that one
    /// absolute Y row of the chunk is 256 contiguous entries: `row * 256 +
    /// local_x * 16 + local_z`, with `row` relative to `min_y`.
    blocks: Vec<u16>,
    /// Name to palette index, kept so interning is O(1) per write.
    ids: HashMap<String, u16>,
}

impl DecoratedChunk {
    fn new(height: usize) -> Self {
        Self {
            palette: vec![None],
            blocks: vec![0; height * 256],
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
}

/// The biomes present somewhere in one chunk's whole 4x4x4-cell volume,
/// ascending and deduplicated. The reference runtime collects the palette of
/// every section of a chunk (`LevelChunkSection.getBiomes().getAll(sink)`)
/// when it decides which features may run, so the set spans the full column
/// height rather than only the surface band.
type BiomeRegion = Vec<String>;

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
    }
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
    masks: RefCell<BoundedCache<(i32, i32), CarveMask>>,
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
    biomes: RefCell<BoundedCache<(i32, i32, i32), Option<String>>>,
    /// Chunks whose whole biome volume has been enumerated, for the union a
    /// decoration pass runs its ordinals over.
    biome_regions: RefCell<BoundedCache<(i32, i32), Rc<BiomeRegion>>>,
    /// Chunks whose columns have been decorated. The value is a pure
    /// function of the chunk position and the seed, so an eviction only
    /// costs a replay of the nine anchor chunks' placement passes.
    chunks: RefCell<BoundedCache<(i32, i32), Rc<DecoratedChunk>>>,
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

    /// Loads `data_root`, compiles the `settings_id` dimension, and binds
    /// the noise engine to `world_seed`.
    pub fn new(
        data_root: &Path,
        world_seed: i64,
        settings_id: &str,
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
        })
    }

    /// Biome identifier for the column at `(x, z)`, resolved from the
    /// climate target at the surface cell. `None` when the operator has
    /// not provisioned a placement table for this preset.
    pub fn biome(&self, x: i32, z: i32, surface_y: i32) -> Option<String> {
        let placement = self.placement.as_ref()?;
        let key = (x >> 2, surface_y >> 2, z >> 2);
        if let Some(value) = self.biomes.borrow_mut().get_mut(&key) {
            return value.clone();
        }
        let value = self.climate.biome(placement, x, surface_y, z);
        self.biomes.borrow_mut().insert(key, value.clone());
        value
    }

    /// The distinct biomes anywhere in one chunk's 4x4x4-cell volume,
    /// ascending and deduplicated. The reference runtime collects the biome
    /// palette of every section of the chunk, so the walk covers the whole
    /// build height rather than only the surface band; one chunk is
    /// 4x4x(height/4) quart cells, 1,536 in an overworld-sized dimension.
    fn chunk_biomes(&self, chunk_x: i32, chunk_z: i32) -> Rc<BiomeRegion> {
        if let Some(region) = self.biome_regions.borrow_mut().get_mut(&(chunk_x, chunk_z)) {
            return Rc::clone(region);
        }
        let min_y = self.router.min_y;
        let mut names = BTreeSet::new();
        for local_x in 0..4 {
            for local_z in 0..4 {
                let x = chunk_x * 16 + local_x * 4;
                let z = chunk_z * 16 + local_z * 4;
                for row in 0..self.router.height / 4 {
                    if let Some(name) = self.biome(x, z, min_y + row * 4) {
                        names.insert(name);
                    }
                }
            }
        }
        let region = Rc::new(names.into_iter().collect::<BiomeRegion>());
        self.biome_regions
            .borrow_mut()
            .insert((chunk_x, chunk_z), Rc::clone(&region));
        region
    }

    /// The biome set one anchor chunk decorates with: the union of its own
    /// volume and the eight around it. The reference runtime retains only the
    /// biome source's possible biomes from that union, and every identifier
    /// the placement table can return is in its own possible set, so the
    /// retention is a no-op here.
    fn region_biomes(&self, chunk_x: i32, chunk_z: i32) -> Vec<String> {
        let mut names = Vec::new();
        for anchor_z in chunk_z - 1..=chunk_z + 1 {
            for anchor_x in chunk_x - 1..=chunk_x + 1 {
                names.extend(self.chunk_biomes(anchor_x, anchor_z).iter().cloned());
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
                let density = &self.router.final_density;
                let mut ceiling = WAY_BELOW_MIN_Y;
                for lookahead in (self.router.min_y..top_y).rev() {
                    let below =
                        self.aquifer
                            .substance(x, lookahead, z, density.sample(x, lookahead, z));
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
        let height = self.router.height as usize;
        let mut chunk = DecoratedChunk::new(height);
        for local_x in 0..16 {
            for local_z in 0..16 {
                let x = chunk_x * 16 + local_x;
                let z = chunk_z * 16 + local_z;
                let ids = self.base_column_ids(x, z);
                for (row, name) in ids.into_iter().enumerate() {
                    let at = chunk.index(local_x, row, local_z);
                    let slot = chunk.intern(name);
                    chunk.blocks[at] = slot;
                }
            }
        }
        let Some(features) = self.features.as_ref() else {
            return chunk;
        };
        let steps = features.decorated_steps();
        if steps.is_empty() {
            return chunk;
        }
        for anchor_z in chunk_z - 1..=chunk_z + 1 {
            for anchor_x in chunk_x - 1..=chunk_x + 1 {
                let region = self.region_biomes(anchor_x, anchor_z);
                let mut view = ChunkView {
                    generator: self,
                    chunk_x,
                    chunk_z,
                    chunk: &mut chunk,
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
    /// with no matching rule keep the filler `default_block`.
    fn base_column_ids(&self, x: i32, z: i32) -> Vec<Option<String>> {
        let min_y = self.router.min_y;
        let mut ids: Vec<Option<String>> = vec![None; self.router.height as usize];
        // The column as the surface pass sees it: the registry carvers
        // have not stamped it yet. Keep the first non-air row found by the
        // top-down scan and retain the rest; this avoids sampling the first
        // non-air row a second time.
        let density = &self.router.final_density;
        let mut top = None;
        let mut filled = Vec::with_capacity(self.router.height as usize);
        for y in (min_y..=self.max_y).rev() {
            let substance = self.aquifer.substance(x, y, z, density.sample(x, y, z));
            if top.is_none() && substance != Substance::Air {
                top = Some(y);
            }
            if top.is_some() {
                filled.push(substance);
            }
        }
        let Some(top) = top else {
            self.record_ocean_floor(x, z, min_y);
            return ids;
        };
        filled.reverse();
        self.heights.borrow_mut().insert((x, z), top);
        let rules = self.surface.as_ref();
        let mut ctx = rules.map(|rules| {
            SurfaceContext::new(
                rules,
                self.router.chunk_surface_level.as_ref(),
                x,
                z,
                |hx, hz| self.surface_height(hx, hz),
                |bx, by, bz| self.biome(bx, bz, by),
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
                        Some(ctx) => {
                            ctx.set_y(stone_above, y - run_bottom + 1, water_height, y);
                            // `rules` is `Some` whenever the context is.
                            rules
                                .expect("surface rules")
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
        ids
    }

    /// Absolute Y of the top written block: terrain, or the aquifer/sea
    /// fluid surface where terrain does not reach above it. Memoised
    /// because the descent's steep gradients re-read neighbours.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        if let Some(y) = self.heights.borrow_mut().get_mut(&(x, z)) {
            return *y;
        }
        let y = self.surface(x, z).map_or(self.router.min_y, |(y, _)| y);
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
        let y = self.ocean_floor_by_descent(x, z);
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

    /// The filler substance at one absolute block position: the raw
    /// `final_density` sample through the runtime aquifer, replaced by
    /// the aquifer's answer at density `0.0` where a registry carver
    /// carved the position (the apply-carving-mask rewrite). This is
    /// what the 3D agreement metric compares against the save.
    pub fn substance(&self, x: i32, y: i32, z: i32) -> Substance {
        if self.carved(x, y, z) {
            return self.aquifer.substance(x, y, z, 0.0);
        }
        let density = self.raw_density(x, y, z);
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
        let mask = self.build_carve_mask(target);
        let hit = mask.contains(relative_x, y, relative_z);
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
        let biome = self.biome(chunk.0 << 4, chunk.1 << 4, 0)?;
        let carvers = data.carvers_for_biome(&biome)?;
        self.chunk_carvers
            .borrow_mut()
            .insert(chunk, Rc::clone(&carvers));
        Some(carvers)
    }

    /// The highest non-air position in the column and what fills it.
    pub fn surface(&self, x: i32, z: i32) -> Option<(i32, Substance)> {
        let density = &self.router.final_density;
        for y in (self.router.min_y..=self.max_y).rev() {
            let substance = self.aquifer.substance(x, y, z, density.sample(x, y, z));
            if substance != Substance::Air {
                return Some((y, substance));
            }
        }
        None
    }

    /// Absolute Y of the top *terrain* (solid) block, ignoring fluids:
    /// the density-only scan kept for diagnostics.
    pub fn terrain_top_exhaustive(&self, x: i32, z: i32) -> i32 {
        let density = &self.router.final_density;
        for y in (self.router.min_y..=self.max_y).rev() {
            if density.sample(x, y, z) > 0.0 {
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
        }
    }

    /// Grid-refined alternative scan (see module docs: an approximation
    /// that is value-exact only for graphs whose top-level `min` operands
    /// are also interpolated at the same y cell).
    pub fn terrain_top(&self, x: i32, z: i32) -> i32 {
        let density = &self.router.final_density;
        let bottom_grid = self.router.min_y.div_euclid(8) * 8;
        let mut grid_y = self.max_y.div_euclid(8) * 8;
        while grid_y >= bottom_grid {
            if density.sample(x, grid_y, z) > 0.0 {
                // Refine the cell above the first positive grid sample.
                let mut probe = (grid_y + 7).min(self.max_y);
                while probe > grid_y {
                    if density.sample(x, probe, z) > 0.0 {
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

    fn biome_at(&self, _x: i32, y: i32, z: i32) -> Option<String> {
        self.generator.biome(_x, z, y)
    }

    fn ocean_floor_height(&self, x: i32, z: i32) -> i32 {
        self.generator.ocean_floor_height(x, z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn scratch_root(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustmc-generator-{label}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create scratch dir");
        path
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, text).expect("write");
    }

    /// Builds a fabricated pack whose final density is an interpolated
    /// linear ramp: gradient value `1.0` at y=0 falling to `-1.0` at y=48,
    /// scaled by a folded constant multiply. The refinement scan must
    /// agree with an exhaustive block-resolution scan, and the surface
    /// rule (with no `aquifers` section) reproduces the global fluid
    /// picker: the highest non-air position, water below the sea.
    #[test]
    fn extracts_surface_from_interpolated_ramp() {
        let root = scratch_root("ramp");
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("density_function/ramp.json"),
            r#"{
                "type": "mul",
                "left": {"type": "interpolated", "cell_size_xz": 4, "cell_size_y": 8, "input": {
                    "type": "gradient", "axis": "y",
                    "from_coordinate": 0, "from_value": 1.0,
                    "to_coordinate": 48, "to_value": -1.0
                }},
                "right": 0.64
            }"#,
        );
        let settings = worldgen.join("noise_settings/ramp_dimension.json");
        let generator_at_sea = |sea_level: i32| -> VanillaGenerator {
            write(
                &settings,
                &format!(
                    r#"{{
                "noise": {{"min_y": 0, "height": 128}},
                "sea_level": {sea_level},
                "noise_router": {{
                    "final_density": "testns:ramp",
                    "continents": 0.0,
                    "erosion": 0.0,
                    "depth": 0.0,
                    "ridges": 0.0,
                    "temperature": 0.0,
                    "vegetation": 0.0
                }}
            }}"#
                ),
            );
            VanillaGenerator::new(&root, 2026, "testns:ramp_dimension").expect("generator")
        };
        // The inner gradient reaches zero at y = 24 and the interpolated
        // wrapper blends it linearly; the highest positive block must
        // equal what a full block-resolution scan finds.
        let generator = generator_at_sea(-1000);
        let x = 10;
        let z = -3;
        let mut exhaustive = 0i32;
        for y in (0..128).rev() {
            if generator.router.final_density.sample(x, y, z) > 0.0 {
                exhaustive = y;
                break;
            }
        }
        assert_eq!(generator.terrain_top(x, z), exhaustive);
        assert_eq!(generator.terrain_top_exhaustive(x, z), exhaustive);
        assert!(exhaustive > 0, "ramp must have a solid base");
        // No fluid anywhere in the column when the sea is below the floor.
        assert_eq!(generator.surface_height(x, z), exhaustive);
        // Water rule: terrain below the sea reports the water top, and a
        // sea above the volume caps at the world ceiling.
        assert_eq!(generator_at_sea(10).surface_height(x, z), 23);
        assert_eq!(generator_at_sea(63).surface_height(x, z), 62);
        assert_eq!(generator_at_sea(200).surface_height(x, z), 127);
        fs::remove_dir_all(&root).expect("cleanup");
    }

    /// With a fabricated `aquifers` section, empty positions can turn
    /// solid via barrier pressure where the raw density is negative, and
    /// the surface scan must report the substance that fills the top.
    #[test]
    fn aquifer_section_participates_in_surface_scan() {
        let root = scratch_root("aquifer");
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("density_function/all_positive.json"),
            r#"{"type": "constant", "value": 1.0}"#,
        );
        write(
            &worldgen.join("noise_settings/aquifer_dimension.json"),
            r#"{
                "noise": {"min_y": 0, "height": 128},
                "sea_level": 63,
                "default_fluid": "minecraft:water",
                "noise_router": {
                    "final_density": "testns:all_positive",
                    "continents": 0.0,
                    "erosion": 0.0,
                    "depth": 0.0,
                    "ridges": 0.0,
                    "temperature": 0.0,
                    "vegetation": 0.0
                },
                "aquifers": {
                    "barrier": 0.0,
                    "fluid_level_floodedness": 0.0,
                    "fluid_level_spread": 0.0,
                    "lava": 0.0,
                    "exclusion": 0.0,
                    "surface_level": 0.0
                }
            }"#,
        );
        let generator =
            VanillaGenerator::new(&root, 2026, "testns:aquifer_dimension").expect("generator");
        assert!(matches!(generator.aquifer, Aquifer::NoiseBased(_)));
        // All-density solid everywhere: the surface is the volume ceiling.
        let (y, substance) = generator.surface(5, 5).expect("surface");
        assert_eq!(y, 127);
        assert_eq!(substance, Substance::Solid);
        fs::remove_dir_all(&root).expect("cleanup");
    }

    /// The generator resolves biomes through the provisioned placement
    /// table when one exists beside the data root and reports `None`
    /// otherwise. Climate slots are constants here so the expected
    /// nearest entry is fully determined by the self-authored table.
    #[test]
    fn biome_requires_a_provisioned_placement_table() {
        let root = scratch_root("biome");
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("density_function/solid.json"),
            r#"{"type": "constant", "value": 1.0}"#,
        );
        let settings_path = worldgen.join("noise_settings/biome_dimension.json");
        let write_settings = |temperature: &str| {
            write(
                &settings_path,
                &format!(
                    r#"{{
                "noise": {{"min_y": 0, "height": 128}},
                "sea_level": 63,
                "noise_router": {{
                    "final_density": "testns:solid",
                    "continents": 0.0,
                    "erosion": 0.0,
                    "depth": 0.0,
                    "ridges": 0.0,
                    "temperature": {temperature},
                    "vegetation": 0.0
                }}
            }}"#
                ),
            );
        };
        write_settings("0.5");
        // Without a placement file the same dimension reports no biome.
        let bare = VanillaGenerator::new(&root, 2026, "testns:biome_dimension").expect("bare");
        assert!(bare.placement.is_none());
        assert_eq!(bare.biome(0, 0, 64), None);

        let psv = root.join("rustmc/biome_placement/biome_dimension.psv");
        write(
            &psv,
            concat!(
                "0|minecraft:plains|t=[-2000-2000]|h=[-10000-10000]|c=[-10000-10000]|",
                "e=[-10000-10000]|d=[-10000-10000]|w=[-10000-10000]|off=0\n",
                "1|minecraft:desert|t=[3000-10000]|h=[-10000-10000]|c=[-10000-10000]|",
                "e=[-10000-10000]|d=[-10000-10000]|w=[-10000-10000]|off=0\n",
            ),
        );
        let generator =
            VanillaGenerator::new(&root, 2026, "testns:biome_dimension").expect("generator");
        assert_eq!(
            generator.biome(0, 0, 64).as_deref(),
            Some("minecraft:desert"),
            "temperature 0.5 quantizes to 5000, inside the desert interval"
        );
        // A colder router selects the other entry instead.
        write_settings("-1.0");
        let cold = VanillaGenerator::new(&root, 2026, "testns:biome_dimension").expect("cold");
        assert_eq!(cold.biome(0, 0, 64).as_deref(), Some("minecraft:plains"));
        fs::remove_dir_all(&root).expect("cleanup");
    }

    /// Requires the operator-provisioned 26.3 worldgen data plus the
    /// captured overworld placement table (see `docs/PROVENANCE.md`).
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn smoke_resolves_overworld_biomes_from_provisioned_table() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
        let generator =
            VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
        let placement = generator.placement.as_ref().expect("provisioned psv table");
        assert!(placement.len() > 1000, "captured table should be large");
        let surface_y = generator.surface_height(0, 0);
        let biome = generator.biome(0, 0, surface_y).expect("biome at origin");
        assert!(
            biome.starts_with("minecraft:"),
            "expected a namespaced biome, got {biome}"
        );
    }

    /// Requires the operator-provisioned 26.3 worldgen data. The full
    /// overworld rule tree must compile and produce a namespaced block
    /// id for every sampled surface column.
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn smoke_top_blocks_resolve_from_provisioned_surface_rules() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
        let generator =
            VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
        assert!(generator.surface.is_some(), "overworld has material rules");
        for (x, z) in [(0, 0), (32, -17), (-128, 96), (1000, 1000)] {
            let top = generator
                .top_block(x, z)
                .unwrap_or_else(|| panic!("column ({x}, {z}) has no top block at all"));
            assert!(
                top.starts_with("minecraft:"),
                "expected a namespaced top block, got {top}"
            );
        }
    }

    /// End-to-end `top_block` on a fabricated pack: a rule that always
    /// matches replaces the filler block, and a rule gated on a biome
    /// the column cannot resolve falls back to the settings
    /// `default_block`.
    #[test]
    fn top_block_applies_material_rules_or_keeps_default_block() {
        let root = scratch_root("topblock");
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("density_function/solid.json"),
            r#"{"type": "constant", "value": 1.0}"#,
        );
        let zero_noise = r#"{"base_amplitude": 0.0, "base_octave": 4}"#;
        let mc = root.join("data/minecraft/worldgen/noise");
        for name in ["surface", "surface_secondary", "clay_bands_offset"] {
            write(&mc.join(format!("{name}.json")), zero_noise);
        }
        write(
            &worldgen.join("material_rule/always.json"),
            r#"{"type": "block", "result_state": "minecraft:sandstone"}"#,
        );
        write(
            &worldgen.join("material_rule/never.json"),
            r#"{"type": "condition",
                "if_true": {"type": "biome", "biome_is": ["minecraft:badlands"]},
                "then_run": {"type": "block", "result_state": "minecraft:red_sand"}}"#,
        );
        let settings = |id: &str, material_rule: &str| {
            write(
                &worldgen.join(format!("noise_settings/{id}.json")),
                &format!(
                    r#"{{
                "noise": {{"min_y": 0, "height": 128}},
                "sea_level": 63,
                "default_block": "minecraft:granite",
                "noise_router": {{
                    "final_density": "testns:solid",
                    "continents": 0.0,
                    "erosion": 0.0,
                    "depth": 0.0,
                    "ridges": 0.0,
                    "temperature": 0.0,
                    "vegetation": 0.0
                }},
                "material_rule": "{material_rule}"
            }}"#
                ),
            );
        };
        settings("hit", "testns:always");
        settings("miss", "testns:never");
        // A fully solid constant-density column tops out at the ceiling.
        let hitting = VanillaGenerator::new(&root, 2026, "testns:hit").expect("hit generator");
        assert_eq!(hitting.surface_height(4, 7), 127);
        assert_eq!(
            hitting.top_block(4, 7).as_deref(),
            Some("minecraft:sandstone"),
            "an unconditional rule recolors the topmost solid"
        );
        // No placement table: the biome-gated rule never holds and the
        // column keeps the filler default block.
        let missing = VanillaGenerator::new(&root, 2026, "testns:miss").expect("miss generator");
        assert_eq!(
            missing.top_block(4, 7).as_deref(),
            Some("minecraft:granite")
        );
        fs::remove_dir_all(&root).expect("cleanup");
    }

    /// The rule program shared by the descent tests: floor and ceiling
    /// `stone_depth` gates (a zero-noise pack makes the depth ranges
    /// deterministic), a vertical-gradient rule coloring the deep rows,
    /// and an unconditional fallthrough.
    const DESCENT_ROOT: &str = r#"{"type": "sequence", "sequence": [
        {"type": "condition",
            "if_true": {"type": "stone_depth", "surface_type": "floor",
                "offset": 1, "add_surface_depth": false,
                "secondary_depth_range": 0},
            "then_run": {"type": "block", "result_state": "minecraft:dirt"}},
        {"type": "condition",
            "if_true": {"type": "stone_depth", "surface_type": "ceiling",
                "offset": 1, "add_surface_depth": false,
                "secondary_depth_range": 0},
            "then_run": {"type": "block", "result_state": "minecraft:moss_block"}},
        {"type": "condition",
            "if_true": {"type": "vertical_gradient",
                "random_name": "minecraft:rustmc_test_deep",
                "true_at_and_below": {"absolute": 10},
                "false_at_and_above": {"absolute": 11}},
            "then_run": {"type": "block", "result_state": "minecraft:deepslate"}},
        {"type": "block", "result_state": "minecraft:stone"}
    ]}"#;

    /// Writes the zero-amplitude noise documents and the constant
    /// noise-router shell the surface program needs for a fabricated
    /// overworld-like pack.
    fn descent_pack_noise(root: &Path) {
        let mc = root.join("data/minecraft/worldgen/noise");
        let zero_noise = r#"{"base_amplitude": 0.0, "base_octave": 4}"#;
        for name in ["surface", "surface_secondary", "clay_bands_offset"] {
            write(&mc.join(format!("{name}.json")), zero_noise);
        }
    }

    fn descent_pack_settings(root: &Path, id: &str, density: &str, sea_level: i32, rule: &str) {
        write(
            &root.join(format!("data/testns/worldgen/noise_settings/{id}.json")),
            &format!(
                r#"{{
                "noise": {{"min_y": 0, "height": 128}},
                "sea_level": {sea_level},
                "default_fluid": "minecraft:water",
                "default_block": "minecraft:granite",
                "noise_router": {{
                    "final_density": "testns:{density}",
                    "continents": 0.0,
                    "erosion": 0.0,
                    "depth": 0.0,
                    "ridges": 0.0,
                    "temperature": 0.0,
                    "vegetation": 0.0
                }},
                "material_rule": "testns:{rule}"
            }}"#
            ),
        );
    }

    /// End-to-end material-rule descent on a fabricated pack whose
    /// density is the product of two y-gradients: one solid run from
    /// y=5 to y=23 with nothing solid below or above. Floor rules must
    /// catch the two topmost rows of the run, ceiling rules its two
    /// bottommost rows, the gradient rule the rows at or below 10, and
    /// the sea variant must stamp the fluid rows without disturbing the
    /// solid results; a biome-gated-only program keeps the filler
    /// default block on every solid row.
    #[test]
    fn column_ids_descends_rules_through_solid_runs() {
        let root = scratch_root("descent");
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("density_function/band.json"),
            r#"{"type": "mul",
                "left": {"type": "gradient", "axis": "y",
                    "from_coordinate": 0, "from_value": -1.0,
                    "to_coordinate": 8, "to_value": 1.0},
                "right": {"type": "gradient", "axis": "y",
                    "from_coordinate": 0, "from_value": 1.0,
                    "to_coordinate": 48, "to_value": -1.0}}"#,
        );
        write(&worldgen.join("material_rule/root.json"), DESCENT_ROOT);
        write(
            &worldgen.join("material_rule/never.json"),
            r#"{"type": "condition",
                "if_true": {"type": "biome", "biome_is": ["minecraft:badlands"]},
                "then_run": {"type": "block", "result_state": "minecraft:red_sand"}}"#,
        );
        descent_pack_noise(&root);
        descent_pack_settings(&root, "dry", "band", -1000, "root");
        descent_pack_settings(&root, "wet", "band", 200, "root");
        descent_pack_settings(&root, "fallback", "band", -1000, "never");

        // The gradient product is positive exactly on 4 < y < 24, so the
        // run spans y = 5..=23 with stone_above 1 at y = 23 counting down
        // and stone_below 1 at y = 5 counting up.
        let dry = VanillaGenerator::new(&root, 2026, "testns:dry").expect("dry generator");
        let ids = dry.column_ids(6, -2);
        assert_eq!(ids.len(), 128);
        for (y, id) in ids[..5].iter().enumerate() {
            assert_eq!(*id, None, "row {y} below the run is pre-carve air");
        }
        assert_eq!(ids[5].as_deref(), Some("minecraft:moss_block"));
        assert_eq!(ids[6].as_deref(), Some("minecraft:moss_block"));
        for id in &ids[7..=10] {
            assert_eq!(id.as_deref(), Some("minecraft:deepslate"));
        }
        for id in &ids[11..=21] {
            assert_eq!(id.as_deref(), Some("minecraft:stone"));
        }
        assert_eq!(ids[22].as_deref(), Some("minecraft:dirt"));
        assert_eq!(ids[23].as_deref(), Some("minecraft:dirt"));
        for (i, id) in ids[24..128].iter().enumerate() {
            assert_eq!(*id, None, "row {} above the column top stays unset", i + 24);
        }
        assert_eq!(
            dry.top_block(6, -2).as_deref(),
            ids[23].as_deref(),
            "the descent's top solid row agrees with the top-block pass"
        );

        // A sea above the volume fills every non-solid row: the descent
        // still walks from the fluid top and colors the run identically.
        let wet = VanillaGenerator::new(&root, 2026, "testns:wet").expect("wet generator");
        let ids = wet.column_ids(6, -2);
        assert_eq!(ids[127].as_deref(), Some("minecraft:water"));
        assert_eq!(ids[24].as_deref(), Some("minecraft:water"));
        assert_eq!(ids[4].as_deref(), Some("minecraft:water"));
        assert_eq!(ids[0].as_deref(), Some("minecraft:water"));
        assert_eq!(ids[23].as_deref(), Some("minecraft:dirt"));
        assert_eq!(ids[8].as_deref(), Some("minecraft:deepslate"));
        assert_eq!(ids[5].as_deref(), Some("minecraft:moss_block"));

        // No placement table: the only rule never holds and every solid
        // row keeps the settings default block.
        let fallback =
            VanillaGenerator::new(&root, 2026, "testns:fallback").expect("fallback generator");
        let ids = fallback.column_ids(6, -2);
        for id in &ids[5..=23] {
            assert_eq!(id.as_deref(), Some("minecraft:granite"));
        }
        assert_eq!(ids[4], None);
        assert_eq!(ids[24], None);
        fs::remove_dir_all(&root).expect("cleanup");
    }

    /// A solid run that reaches the bottom of the world keeps the
    /// documented below-depth sentinel, so the ceiling gate (depth <= 2)
    /// never fires inside the run and the floor row itself falls through
    /// to the gradient rule.
    #[test]
    fn column_ids_floor_run_keeps_below_depth_sentinel() {
        let root = scratch_root("sentinel");
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("density_function/ramp.json"),
            r#"{"type": "mul",
                "left": {"type": "interpolated", "cell_size_xz": 4, "cell_size_y": 8, "input": {
                    "type": "gradient", "axis": "y",
                    "from_coordinate": 0, "from_value": 1.0,
                    "to_coordinate": 48, "to_value": -1.0
                }},
                "right": 0.64}"#,
        );
        write(&worldgen.join("material_rule/root.json"), DESCENT_ROOT);
        descent_pack_noise(&root);
        descent_pack_settings(&root, "floor", "ramp", -1000, "root");

        let generator =
            VanillaGenerator::new(&root, 2026, "testns:floor").expect("floor generator");
        let ids = generator.column_ids(10, -3);
        // Same ramp top as the ramp scan test: y = 23.
        assert_eq!(ids[23].as_deref(), Some("minecraft:dirt"));
        assert_eq!(ids[22].as_deref(), Some("minecraft:dirt"));
        assert_eq!(ids[21].as_deref(), Some("minecraft:stone"));
        for id in &ids[11..=20] {
            assert_eq!(id.as_deref(), Some("minecraft:stone"));
        }
        // The run reaches min_y: sentinel below-depths are far above the
        // ceiling gate's range, and rows at or below 10 color deepslate.
        for id in &ids[0..=10] {
            assert_eq!(id.as_deref(), Some("minecraft:deepslate"));
        }
        for id in &ids[24..128] {
            assert_eq!(*id, None);
        }
        assert_eq!(
            generator.top_block(10, -3).as_deref(),
            ids[23].as_deref(),
            "the descent's top solid row agrees with the top-block pass"
        );
        fs::remove_dir_all(&root).expect("cleanup");
    }

    /// Requires the operator-provisioned 26.3 worldgen data. The descent
    /// must reproduce the deepslate split below the transition band and fire
    /// both ore-vein rules: granite and tuff are vein states the rest of the
    /// program never writes, so any occurrence proves the copper and iron
    /// veins evaluated. A vein may only replace a block its target test
    /// accepts, and every replaceable tag in the pack (`stone_ore_replaceables`,
    /// `deepslate_ore_replaceables`, `height_specific_ore_replaceables`) is a
    /// subset of `base_stone_overworld`, so that one tag answers for the whole
    /// placement stage: a row the pass changed must have held one of its
    /// members before, and no row may appear or disappear.
    ///
    /// The four 16×16 squares sit at fixed origins on a power-of-two grid,
    /// with one off-grid pair so at least one square straddles chunk borders
    /// and exercises the neighbour replay. Veins are dense enough that the
    /// families appear at any underground position: the coordinates are a
    /// sample, not a tuned pick.
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn smoke_column_ids_splits_deepslate_and_fires_veins() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
        let generator =
            VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
        let min_y = generator.min_y();
        let tags = generator
            .features
            .as_ref()
            .expect("the provisioned pack decorates")
            .tags();
        let mut granite = 0usize;
        let mut tuff = 0usize;
        let mut ores = 0usize;
        let mut unexpected_deep: Vec<(i32, i32, i32, String)> = Vec::new();
        for (origin_x, origin_z) in [(0, 0), (512, -512), (-1024, 1024), (255, -97)] {
            for x in origin_x..origin_x + 16 {
                for z in origin_z..origin_z + 16 {
                    let base = generator.base_column_ids(x, z);
                    let ids = generator.column_ids(x, z);
                    for (index, id) in ids.iter().enumerate() {
                        let y = min_y + index as i32;
                        let before = base[index].as_deref();
                        if let Some(id) = id {
                            match id.as_str() {
                                "minecraft:granite" => granite += 1,
                                "minecraft:tuff" => tuff += 1,
                                "minecraft:deepslate_iron_ore"
                                | "minecraft:deepslate_copper_ore" => ores += 1,
                                _ => {}
                            }
                        }
                        // Between the iron band and the deepslate/stone
                        // transition only the underground rule, carving and
                        // the dimension fluids can have written a row. An
                        // unset row is the cave air the carvers left in the
                        // column, so it is expected here too.
                        if (-8..=-2).contains(&y)
                            && !matches!(
                                before,
                                None | Some("minecraft:deepslate")
                                    | Some("minecraft:cave_air")
                                    | Some("minecraft:water")
                                    | Some("minecraft:lava")
                            )
                            && unexpected_deep.len() < 20
                        {
                            unexpected_deep.push((x, z, y, before.unwrap_or_default().to_string()));
                        }
                        if before == id.as_deref() {
                            continue;
                        }
                        // A row the pass did change: it can only change one its
                        // target test accepts.
                        assert!(
                            before
                                .is_some_and(|before| tags
                                    .contains("minecraft:base_stone_overworld", before)),
                            "({x}, {z}) row {y}: the placement stage wrote {id:?} onto {before:?}"
                        );
                    }
                    assert_eq!(
                        ids.iter()
                            .zip(base.iter())
                            .filter(|(after, before)| after.is_some() != before.is_some())
                            .count(),
                        0,
                        "({x}, {z}): a vein cannot create or remove a row, only recolor one"
                    );
                }
            }
        }
        assert!(granite > 0, "copper veins must leave granite filler");
        assert!(tuff > 0, "the tuff band must fire");
        assert!(ores > 0, "the iron or copper target rules must place ore");
        assert!(
            unexpected_deep.is_empty(),
            "unexpected pre-feature rows in the deep band: {unexpected_deep:?}"
        );
    }

    /// Requires the operator-provisioned 26.3 worldgen data. On columns
    /// whose top row the carvers left alone, the descent's top solid row
    /// must equal the single-row top-block pass.
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn smoke_column_ids_top_row_matches_top_block() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
        let generator =
            VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
        let min_y = generator.min_y();
        for (x, z) in [(0, 0), (32, -17), (-128, 96)] {
            let top = generator.surface_height(x, z);
            if generator.carved(x, top, z) {
                continue;
            }
            let ids = generator.column_ids(x, z);
            let index = (top - min_y) as usize;
            let row = ids[index].as_deref().expect("top row is written");
            assert_eq!(
                generator.top_block(x, z).as_deref(),
                Some(row),
                "column ({x}, {z}) top row at y={top}"
            );
        }
    }

    /// Requires the operator-provisioned 26.3 worldgen data. On real terrain
    /// the stored heightmap must be the descent: the target chunk is filled
    /// through the block-id pass, which derives its 256 rows, and then the
    /// chunk's rows and the halo band the anchor gate's footprints spill into
    /// are served from the maps and compared against `ocean_floor_by_descent`,
    /// which never reads a map. The sampled band must reach rows above ocean
    /// floors that the filler left as fluid and rows a carver stamped, so a
    /// pack that only ever answered "the top is solid" could not pass.
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn smoke_ocean_floor_heightmap_serves_the_descent_on_real_terrain() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
        let generator =
            VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
        let mut fluid_rows = 0usize;
        let mut carved_rows = 0usize;
        let mut columns = 0usize;
        for (chunk_x, chunk_z) in [(-20, -8), (4, -20), (-4, 5)] {
            for local_x in 0..16 {
                for local_z in 0..16 {
                    let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                    let _ = generator.base_column_ids(x, z);
                }
            }
            let stored = generator
                .ocean_floor_maps
                .borrow_mut()
                .get_mut(&(chunk_x, chunk_z))
                .expect("the fill pass stored the target chunk map")
                .entries;
            assert!(
                stored.iter().all(|row| *row != UNCOMPUTED_OCEAN_FLOOR),
                "chunk ({chunk_x}, {chunk_z}): the fill pass left a row uncomputed"
            );
            // The widest box in the pack spills about thirteen blocks past its
            // anchor chunk, so the gate can read this band without the band's
            // own chunk ever being filled. The target chunk is checked column
            // by column; outside it a one-in-four lattice is enough, since the
            // answer of every column is computed the same way and the descent
            // — not the coverage — is what a smoke can afford to pay.
            for x in chunk_x * 16 - 13..chunk_x * 16 + 16 + 13 {
                for z in chunk_z * 16 - 13..chunk_z * 16 + 16 + 13 {
                    let in_target = x >> 4 == chunk_x && z >> 4 == chunk_z;
                    if !in_target && ((x & 3) | (z & 3)) != 0 {
                        continue;
                    }
                    let floor = generator.ocean_floor_height(x, z);
                    assert_eq!(
                        floor,
                        generator.ocean_floor_by_descent(x, z),
                        "column ({x}, {z}): stored row and descent disagree"
                    );
                    let top = generator.surface_height(x, z);
                    // Split the rows the walk left above the floor by what
                    // they are: a row a carver stamped, and a row the filler
                    // itself left as fluid. Both have to appear for the smoke
                    // to have crossed the terrain the gate is asked about.
                    for y in floor..top {
                        if generator.carved(x, y, z) {
                            carved_rows += 1;
                        } else if matches!(generator.substance(x, y, z), Substance::Fluid(_)) {
                            fluid_rows += 1;
                        }
                    }
                    columns += 1;
                }
            }
        }
        assert!(columns > 1_000, "the smoke sampled {columns} columns");
        assert!(
            carved_rows > 0 && fluid_rows > 0,
            "the sampled band crossed {fluid_rows} filler-fluid rows and {carved_rows} \
             carve-stamped rows above ocean floors"
        );
        println!(
            "smoke OK: {columns} columns, {fluid_rows} filler-fluid rows and {carved_rows} \
             carved rows above their floors, root {}",
            root.display()
        );
    }

    /// Requires the operator-provisioned 26.3 worldgen data. A feature's
    /// ordinal is part of its seed, and the ordinal graph has a bounded cycle
    /// recovery (`MAX_CYCLE_ATTEMPTS`) where the reference runtime recurses
    /// without a bound. That bound is only safe while the pack never needs it,
    /// so this checks the schedule really is the graph over the biome source's
    /// own list and that neither cycle counter was reported, and prints the
    /// shape it found.
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn smoke_operator_feature_schedule_needs_no_cycle_recovery() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
        let generator =
            VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
        let data = generator
            .features
            .as_ref()
            .expect("the provisioned pack decorates");
        assert!(
            !data.biome_order().is_empty(),
            "the schedule fell back to the pack's identifier order"
        );
        for counter in ["feature_order_cycle", "feature_order_cycle_unresolved"] {
            assert!(
                !data.unimplemented_features.contains_key(counter),
                "the provisioned pack needed the bounded cycle recovery ({counter})"
            );
        }
        let steps = data.decorated_steps();
        let slots: usize = steps
            .iter()
            .map(|step| data.step(*step).map_or(0, |list| list.len()))
            .sum();
        println!(
            "smoke OK: schedule over {} biomes, {} steps carrying {} placed features, root {}",
            data.biome_order().len(),
            steps.len(),
            slots,
            root.display()
        );
    }

    /// A fabricated pack that drives every coordinate-keyed cache of the
    /// generator and the aquifer without touching the operator's data: a
    /// y-only density band (solid on y = 5..=23, identical in all
    /// columns), the shared descent rule program, an all-constant aquifer
    /// section, a one-entry placement table selecting a biome whose only
    /// carver has probability 0. That carver never stamps a mask, yet
    /// `build_carve_mask` still resolves the 17x17 window of source chunk
    /// carver lists, so the replay path and both carving caches run.
    fn bounded_pack(root: &Path) {
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("density_function/band.json"),
            r#"{"type": "mul",
                "left": {"type": "gradient", "axis": "y",
                    "from_coordinate": 0, "from_value": -1.0,
                    "to_coordinate": 8, "to_value": 1.0},
                "right": {"type": "gradient", "axis": "y",
                    "from_coordinate": 0, "from_value": 1.0,
                    "to_coordinate": 48, "to_value": -1.0}}"#,
        );
        write(&worldgen.join("material_rule/root.json"), DESCENT_ROOT);
        write(
            &worldgen.join("carver/scarce.json"),
            r#"{"type": "minecraft:cave", "probability": 0.0,
                "y": {"type": "minecraft:uniform",
                    "min_inclusive": {"absolute": -20},
                    "max_inclusive": {"absolute": 60}},
                "count": 1, "thickness": 1.5,
                "room_vertical_radius_multiplier": 1.0,
                "horizontal_radius_multiplier": 1.0,
                "vertical_radius_multiplier": 1.0,
                "floor_level": -0.7}"#,
        );
        write(
            &worldgen.join("biome/flat.json"),
            r#"{"carvers": ["testns:scarce"]}"#,
        );
        descent_pack_noise(root);
        write(
            &worldgen.join("noise_settings/bounded.json"),
            r#"{
                "noise": {"min_y": 0, "height": 128},
                "sea_level": -1000,
                "default_fluid": "minecraft:water",
                "default_block": "minecraft:granite",
                "noise_router": {
                    "final_density": "testns:band",
                    "continents": 0.0,
                    "erosion": 0.0,
                    "depth": 0.0,
                    "ridges": 0.0,
                    "temperature": 0.0,
                    "vegetation": 0.0
                },
                "aquifers": {
                    "barrier": 0.0,
                    "fluid_level_floodedness": 0.0,
                    "fluid_level_spread": 0.0,
                    "lava": 0.0,
                    "exclusion": 0.0,
                    "surface_level": 0.0
                },
                "material_rule": "testns:root"
            }"#,
        );
        // The preset of `testns:bounded` is `bounded`, so the placement
        // table beside the data root must be named accordingly. A single
        // entry covering the whole climate volume keeps every chunk on
        // the `testns:flat` carver list.
        write(
            &root.join("rustmc/biome_placement/bounded.psv"),
            concat!(
                "0|testns:flat|t=[-2000-2000]|h=[-10000-10000]|c=[-10000-10000]|",
                "e=[-10000-10000]|d=[-10000-10000]|w=[-10000-10000]|off=0\n"
            ),
        );
    }

    fn bounded_generator(label: &str) -> (PathBuf, VanillaGenerator) {
        let root = scratch_root(label);
        bounded_pack(&root);
        let generator =
            VanillaGenerator::new(&root, 2026, "testns:bounded").expect("bounded pack generator");
        (root, generator)
    }

    #[test]
    fn quart_biome_cache_is_bounded_and_matches_uncached_sampler() {
        let (root, generator) = bounded_generator("cache-biomes");
        let placement = generator.placement.as_ref().expect("fixture placement");
        for quart in 0..9_000i32 {
            let x = quart * 4;
            let expected = generator.climate.biome(placement, x, 8, -4);
            assert_eq!(generator.biome(x, -4, 8), expected);
            assert_eq!(generator.biome(x + 3, -1, 11), expected);
        }
        let occupancy = generator.cache_occupancy();
        assert!(occupancy.biomes <= 2 * VanillaGenerator::BIOME_CACHE_CAPACITY);
        assert!(occupancy.biomes < 9_000);
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// The carving caches must stop growing long before the world volume
    /// queried: 300 chunk targets along the x = z diagonal touch 300
    /// distinct mask keys and (each replaying its 17x17 window) more than
    /// 10,000 distinct source chunk keys, while the capacities allow 64
    /// and 4,096 entries at most.
    #[test]
    fn carve_caches_stay_bounded_across_target_chunks() {
        let (root, generator) = bounded_generator("cache-carve");
        let mut distinct_sources = std::collections::HashSet::new();
        for target in 0..300i32 {
            // One column per target chunk: the descent's carve pass is
            // what builds and reads the mask.
            let _ = generator.column_ids(target * 16 + 8, target * 16 + 8);
            for dx in -8..=8 {
                for dz in -8..=8 {
                    distinct_sources.insert((target + dx, target + dz));
                }
            }
        }
        let occupancy = generator.cache_occupancy();
        assert!(
            distinct_sources.len() > 10_000,
            "the fixture must probe a source-window union far past the cache bound, \
             got {}",
            distinct_sources.len()
        );
        assert!(
            occupancy.masks <= 2 * VanillaGenerator::MASK_CACHE_CAPACITY,
            "mask cache grew past its bound: {occupancy:?}"
        );
        assert!(
            occupancy.masks < 300,
            "every one of the 300 target masks is still pinned: {occupancy:?}"
        );
        assert!(
            occupancy.chunk_carvers <= 2 * VanillaGenerator::CHUNK_CARVERS_CACHE_CAPACITY,
            "carver cache grew past its bound: {occupancy:?}"
        );
        assert!(
            occupancy.chunk_carvers < distinct_sources.len(),
            "every source chunk of the sweep is still pinned: {occupancy:?}"
        );
        assert!(
            occupancy.heights <= 2 * VanillaGenerator::HEIGHT_CACHE_CAPACITY,
            "height cache grew past its bound: {occupancy:?}"
        );
        fs::remove_dir_all(&root).expect("cleanup");
    }

    /// The column-top memo is the largest of the seven caches by entry
    /// count: a sweep of 36 whole chunks queries 9,216 distinct columns,
    /// and the memo must hold at most 8,192 of them.
    #[test]
    fn height_cache_stays_bounded_across_column_sweeps() {
        let (root, generator) = bounded_generator("cache-heights");
        // 96x96 blocks is 36 chunks, i.e. 9,216 distinct columns; the
        // neighbour reads of the surface program land on columns inside
        // the same sweep.
        let mut columns = 0usize;
        for x in 0..96i32 {
            for z in 0..96i32 {
                let _ = generator.surface_height(x, z);
                columns += 1;
            }
        }
        assert_eq!(columns, 9_216);
        let occupancy = generator.cache_occupancy();
        assert!(
            occupancy.heights <= 2 * VanillaGenerator::HEIGHT_CACHE_CAPACITY,
            "height cache grew past its bound after {columns} columns: {occupancy:?}"
        );
        assert!(
            occupancy.heights < columns,
            "the memo still holds one entry per column queried: {occupancy:?}"
        );
        fs::remove_dir_all(&root).expect("cleanup");
    }

    /// The aquifer's four grid caches are keyed by fluid cell, quart
    /// point, and chunk, so they grow with the area queried even when the
    /// columns themselves are not cached. Probing one low block per chunk
    /// of a 60x60 chunk sweep touches 3,600 distinct chunk keys (bound:
    /// 512) and, because each sampling consults the 2x3x2 cell
    /// neighbourhood of its anchor, the full 61x3x61 = 11,163 cell grid
    /// (bound: 2,048), plus tens of thousands of quart surface points
    /// (bound: 8,192).
    #[test]
    fn aquifer_caches_stay_bounded_across_chunk_sweeps() {
        let (root, generator) = bounded_generator("cache-aquifer");
        for chunk_x in 0..60i32 {
            for chunk_z in 0..60i32 {
                // y = 4 is density-empty below the aquifer sampling bound
                // of this pack, which is the branch that consults cells.
                let _ = generator.substance(chunk_x * 16 + 3, 4, chunk_z * 16 + 7);
            }
        }
        let occupancy = generator.cache_occupancy();
        assert!(
            occupancy.aquifer_skip_bounds <= 2 * NoiseBasedAquifer::SKIP_CACHE_CAPACITY,
            "chunk sampling bounds grew past their bound: {occupancy:?}"
        );
        assert!(
            occupancy.aquifer_skip_bounds < 3_600,
            "one key per chunk and the sweep spans 3,600 chunks, so a growing map would pin \
             all of them: {occupancy:?}"
        );
        assert!(
            occupancy.aquifer_centers <= 2 * NoiseBasedAquifer::CENTER_CACHE_CAPACITY,
            "fluid cell centers grew past their bound: {occupancy:?}"
        );
        assert!(
            occupancy.aquifer_centers < 11_163,
            "the sweep consults 61x3x61 fluid cells, so a growing map would pin all of them: \
             {occupancy:?}"
        );
        assert!(
            occupancy.aquifer_statuses <= 2 * NoiseBasedAquifer::STATUS_CACHE_CAPACITY,
            "fluid cell statuses grew past their bound: {occupancy:?}"
        );
        assert!(
            occupancy.aquifer_surface_levels <= 2 * NoiseBasedAquifer::SURFACE_CACHE_CAPACITY,
            "preliminary surface levels grew past their bound: {occupancy:?}"
        );
        assert!(
            occupancy.aquifer_centers > 0 && occupancy.aquifer_surface_levels > 0,
            "the sweep did not populate the aquifer caches, so the bounds above are vacuous: \
             {occupancy:?}"
        );
        fs::remove_dir_all(&root).expect("cleanup");
    }

    /// Eviction is a recomputation, not a change of result: after the
    /// caches have rotated through a wide sweep, the same column must
    /// still report the same top, the same rule descent, and the same
    /// carve state, which is what makes the bounds safe to apply to
    /// coordinate-keyed memos at all.
    #[test]
    fn evicted_entries_recompute_identically() {
        let (root, generator) = bounded_generator("cache-invariance");
        let (x, z) = (6i32, -2i32);
        let before_height = generator.surface_height(x, z);
        let before_floor = generator.ocean_floor_height(x, z);
        let before_ids = generator.column_ids(x, z);
        let before_top = generator.top_block(x, z);
        let before_biome = generator.biome(x, z, 8);
        let before_carved = generator.carved(x, 8, z);
        assert!(
            before_height > 0 && before_ids.iter().any(Option::is_some) && before_top.is_some(),
            "the fixture column must actually have terrain and rule results to compare"
        );

        // Enough distinct keys to rotate every generation of every cache,
        // none of which re-reads the column captured above: 300 new mask
        // targets (capacity 32), 9,216 new columns (capacity 4,096), and
        // some 19,000 new fluid cells (capacity 1,024).
        for target in 0..300i32 {
            let _ = generator.column_ids(target * 16 + 8, target * 16 + 8);
        }
        for x in 0..96i32 {
            for z in 0..96i32 {
                let _ = generator.surface_height(x, z);
            }
        }
        for chunk_x in 0..40i32 {
            for chunk_z in 0..40i32 {
                let _ = generator.substance(chunk_x * 16 + 3, 4, chunk_z * 16 + 7);
            }
        }
        let occupancy = generator.cache_occupancy();
        assert!(
            occupancy.masks <= 2 * VanillaGenerator::MASK_CACHE_CAPACITY
                && occupancy.heights <= 2 * VanillaGenerator::HEIGHT_CACHE_CAPACITY
                && occupancy.ocean_floor_maps
                    <= 2 * VanillaGenerator::OCEAN_FLOOR_MAP_CACHE_CAPACITY
                && occupancy.biomes <= 2 * VanillaGenerator::BIOME_CACHE_CAPACITY
                && occupancy.aquifer_centers <= 2 * NoiseBasedAquifer::CENTER_CACHE_CAPACITY,
            "the sweeps above must have rotated the caches while keeping them bounded: \
             {occupancy:?}"
        );

        assert_eq!(generator.surface_height(x, z), before_height);
        assert_eq!(generator.ocean_floor_height(x, z), before_floor);
        assert_eq!(generator.column_ids(x, z), before_ids);
        assert_eq!(generator.top_block(x, z), before_top);
        assert_eq!(generator.biome(x, z, 8), before_biome);
        assert_eq!(generator.carved(x, 8, z), before_carved);
        fs::remove_dir_all(&root).expect("cleanup");
    }

    /// The bounded pack plus a placement stage: one configured ore whose
    /// target is a block tag, placed with count, square, band and biome
    /// decorations, and listed by the fixture biome at the underground step.
    /// The pack's solid band is y = 5..=23 and its descent ends in an
    /// unconditional stone fallthrough, so granite anywhere in a column can
    /// only come from a vein.
    fn decorating_pack(root: &Path) {
        bounded_pack(root);
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("feature/vein.json"),
            r#"{"type": "minecraft:ore", "size": 32, "discard_chance_on_air_exposure": 0.0,
                "targets": [{"state": "minecraft:granite",
                    "target": {"predicate_type": "minecraft:tag_match", "tag": "testns:filler"}}]}"#,
        );
        write(
            &root.join("data/testns/tags/block/filler.json"),
            r#"{"values": ["minecraft:stone", {"tag": "testns:deep_filler"}]}"#,
        );
        write(
            &root.join("data/testns/tags/block/deep_filler.json"),
            r#"{"values": ["minecraft:deepslate"]}"#,
        );
        write(
            &worldgen.join("placed_feature/vein.json"),
            r#"{"feature": "testns:vein", "placement": [
                {"type": "minecraft:count", "count": 3},
                {"type": "minecraft:in_square"},
                {"type": "minecraft:height_range", "height": {"type": "minecraft:uniform",
                    "min_inclusive": {"absolute": 5}, "max_inclusive": {"absolute": 23}}},
                {"type": "minecraft:biome"}
            ]}"#,
        );
        // The eleventh-step slot the overworld pack uses for ore veins.
        write(
            &worldgen.join("biome/flat.json"),
            r#"{"carvers": ["testns:scarce"],
                "features": [[], [], [], [], [], [], [], [], ["testns:vein"], [], []]}"#,
        );
    }

    /// Every column of one chunk, in a fixed read order.
    fn chunk_columns(
        generator: &VanillaGenerator,
        chunk_x: i32,
        chunk_z: i32,
    ) -> Vec<Vec<Option<String>>> {
        let mut columns = Vec::with_capacity(256);
        for local_x in 0..16 {
            for local_z in 0..16 {
                columns.push(generator.column_ids(chunk_x * 16 + local_x, chunk_z * 16 + local_z));
            }
        }
        columns
    }

    fn decorating_generator(label: &str) -> (PathBuf, VanillaGenerator) {
        let root = scratch_root(label);
        decorating_pack(&root);
        let generator = VanillaGenerator::new(&root, 2026, "testns:bounded")
            .expect("decorating pack generator");
        (root, generator)
    }

    /// The decorating pack plus a second dimension whose placement table
    /// splits the world along x: `testns:low` west of x = 32, `testns:peak`
    /// east of it. Only `peak` lists a placed vein, and that vein carries no
    /// `minecraft:biome` filter, so the only thing that can keep it out of a
    /// chunk is the biome union the pass runs its ordinals over. The
    /// `testns:flat` biome of the decorating dimension stays out of this
    /// dimension's possible set, so its own granite vein must not run here.
    fn region_pack(root: &Path) {
        decorating_pack(root);
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("density_function/slope.json"),
            r#"{"type": "gradient", "axis": "x", "from_coordinate": 0,
                "to_coordinate": 64, "from_value": -1.0, "to_value": 1.0}"#,
        );
        write(
            &worldgen.join("feature/raw.json"),
            r#"{"type": "minecraft:ore", "size": 32, "discard_chance_on_air_exposure": 0.0,
                "targets": [{"state": "minecraft:diorite",
                    "target": {"predicate_type": "minecraft:block_match", "block": "minecraft:stone"}}]}"#,
        );
        write(
            &worldgen.join("placed_feature/raw.json"),
            r#"{"feature": "testns:raw", "placement": [
                {"type": "minecraft:count", "count": 3},
                {"type": "minecraft:in_square"},
                {"type": "minecraft:height_range", "height": {"type": "minecraft:uniform",
                    "min_inclusive": {"absolute": 5}, "max_inclusive": {"absolute": 23}}}
            ]}"#,
        );
        write(
            &worldgen.join("biome/low.json"),
            r#"{"carvers": [], "features": [[], [], [], [], [], [], [], [], [], [], []]}"#,
        );
        write(
            &worldgen.join("biome/peak.json"),
            r#"{"carvers": [], "features": [[], [], [], [], [], [], [], [], ["testns:raw"], [], []]}"#,
        );
        write(
            &worldgen.join("noise_settings/split.json"),
            r#"{
                "noise": {"min_y": 0, "height": 128},
                "sea_level": -1000,
                "default_fluid": "minecraft:water",
                "default_block": "minecraft:granite",
                "noise_router": {
                    "final_density": "testns:band",
                    "continents": 0.0,
                    "erosion": 0.0,
                    "depth": 0.0,
                    "ridges": 0.0,
                    "temperature": "testns:slope",
                    "vegetation": 0.0
                },
                "aquifers": {
                    "barrier": 0.0,
                    "fluid_level_floodedness": 0.0,
                    "fluid_level_spread": 0.0,
                    "lava": 0.0,
                    "exclusion": 0.0,
                    "surface_level": 0.0
                },
                "material_rule": "testns:root"
            }"#,
        );
        write(
            &root.join("rustmc/biome_placement/split.psv"),
            concat!(
                "0|testns:low|t=[-10000-0]|h=[-10000-10000]|c=[-10000-10000]|",
                "e=[-10000-10000]|d=[-10000-10000]|w=[-10000-10000]|off=0\n",
                "1|testns:peak|t=[0-10000]|h=[-10000-10000]|c=[-10000-10000]|",
                "e=[-10000-10000]|d=[-10000-10000]|w=[-10000-10000]|off=0\n"
            ),
        );
    }

    fn region_generator(label: &str) -> (PathBuf, VanillaGenerator) {
        let root = scratch_root(label);
        region_pack(&root);
        let generator =
            VanillaGenerator::new(&root, 2026, "testns:split").expect("region pack generator");
        (root, generator)
    }

    /// End to end: the placement pass paints veins over the rule descent and
    /// nothing else. Every granite block sits where the tag test held, every
    /// other row keeps the descent's answer, and the base pass the cache is
    /// built from places none of it.
    #[test]
    fn a_decorated_chunk_carries_the_pack_veins() {
        let (root, generator) = decorating_generator("decorate");
        assert!(generator.features.is_some(), "the fixture pack decorates");
        let min_y = generator.min_y();
        let mut painted = 0usize;
        for local_x in 0..16 {
            for local_z in 0..16 {
                let (x, z) = (local_x, local_z);
                let base = generator.base_column_ids(x, z);
                assert_eq!(
                    base.iter()
                        .filter(|name| name.as_deref() == Some("minecraft:granite"))
                        .count(),
                    0,
                    "the descent alone never writes the vein's state at ({x}, {z})"
                );
                let decorated = generator.column_ids(x, z);
                for (index, name) in decorated.iter().enumerate() {
                    if name.as_deref() != Some("minecraft:granite") {
                        assert_eq!(
                            name,
                            &base[index],
                            "row {} of ({x}, {z}) changed without a vein claiming it",
                            min_y + index as i32
                        );
                        continue;
                    }
                    painted += 1;
                    assert!(
                        matches!(
                            base[index].as_deref(),
                            Some("minecraft:stone") | Some("minecraft:deepslate")
                        ),
                        "a vein only replaces what its target test accepts, saw {:?}",
                        base[index]
                    );
                }
            }
        }
        assert!(
            painted > 0,
            "the fixture must actually fire veins, or the assertions above are vacuous"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// The decoration pass runs the ordinals the biomes of a nine-chunk
    /// window contribute, and a window's biome set is enumerated over the
    /// whole chunk volume, not just the surface band.
    #[test]
    fn the_decoration_region_covers_the_chunk_volume_and_the_nine_window() {
        let (root, generator) = region_generator("region");
        let low = "testns:low".to_string();
        let peak = "testns:peak".to_string();
        // One chunk's enumeration visits every 4x4x4 cell of its volume: 4x4
        // quart columns over height/4 rows, all distinct sampler keys.
        let cells = 4 * 4 * (generator.router.height / 4) as usize;
        assert_eq!(*generator.chunk_biomes(1, 0), vec![low.clone()]);
        let occupancy = generator.cache_occupancy();
        assert_eq!(occupancy.biomes, cells, "the quart scan came up short");
        assert_eq!(occupancy.biome_regions, 1);
        assert_eq!(*generator.chunk_biomes(3, 0), vec![peak.clone()]);
        let occupancy = generator.cache_occupancy();
        assert_eq!(occupancy.biomes, 2 * cells);
        assert_eq!(occupancy.biome_regions, 2);
        // A cached volume is not re-sampled.
        let _ = generator.chunk_biomes(1, 0);
        assert_eq!(generator.cache_occupancy().biomes, 2 * cells);

        // The fixture's climate split runs at x = 32, so chunk 1 sits wholly
        // west of it and chunk 3 wholly east, and the nine-chunk union
        // reaches across the split as soon as the window does: each biome
        // once, in identifier order.
        assert_eq!(
            generator.region_biomes(2, 0),
            vec![low.clone(), peak.clone()]
        );
        assert_eq!(generator.region_biomes(0, 0), vec![low.clone()]);
        assert_eq!(generator.region_biomes(8, 0), vec![peak.clone()]);

        // The volume cache is bounded like the others: a sweep along a row
        // touches far more chunk volumes than it may hold.
        for chunk_x in 0..200i32 {
            let _ = generator.region_biomes(chunk_x, 0);
        }
        let occupancy = generator.cache_occupancy();
        assert!(
            occupancy.biome_regions <= 2 * VanillaGenerator::BIOME_REGION_CACHE_CAPACITY,
            "the region cache grew past its bound: {occupancy:?}"
        );
        assert!(occupancy.biome_regions < 220);
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// A placed feature with no biome filter of its own is still gated by the
    /// region: the chunk whose nine-chunk window holds only the biome that
    /// does not list it runs none of its ordinals at all.
    #[test]
    fn a_feature_only_the_neighbouring_biome_lists_stays_out_of_the_far_chunks() {
        let (root, generator) = region_generator("region-reach");
        let count = |generator: &VanillaGenerator, chunk_x: i32, name: &str| -> usize {
            chunk_columns(generator, chunk_x, 0)
                .iter()
                .map(|column| {
                    column
                        .iter()
                        .filter(|state| state.as_deref() == Some(name))
                        .count()
                })
                .sum()
        };
        let west = count(&generator, 0, "minecraft:diorite");
        let east = count(&generator, 5, "minecraft:diorite");
        let border = count(&generator, 2, "minecraft:diorite");
        assert_eq!(
            west, 0,
            "the chunks west of the split list the vein nowhere"
        );
        assert!(east > 0, "the peak biome's own chunk must be decorated");
        assert!(
            border > 0,
            "a window that reaches one chunk into the peak half is decorated too"
        );
        assert_eq!(
            count(&generator, 5, "minecraft:granite"),
            0,
            "a biome the dimension cannot produce contributes no ordinals"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// A chunk's grid is a pure function of its coordinates and the seed, so
    /// the order chunks are asked for in cannot change a block: the answer a
    /// neighbour reaches by replaying this anchor is the answer this chunk
    /// reaches by clipping that replay to itself.
    #[test]
    fn decoration_does_not_depend_on_the_chunk_query_order() {
        let (root, forward) = decorating_generator("decorate-order");
        let reverse =
            VanillaGenerator::new(&root, 2026, "testns:bounded").expect("second generator");
        let block = [(0, 0), (0, 1), (1, 0), (1, 1)];
        // Visit the chunk block in one order, then read it back in a fixed
        // order: only the build order differs between the two generators.
        let visit = |generator: &VanillaGenerator, order: &[(i32, i32)]| {
            for &(chunk_x, chunk_z) in order {
                let _ = chunk_columns(generator, chunk_x, chunk_z);
            }
            block
                .iter()
                .flat_map(|&(chunk_x, chunk_z)| chunk_columns(generator, chunk_x, chunk_z))
                .collect::<Vec<_>>()
        };
        let mut backwards = block;
        backwards.reverse();
        let built_first = visit(&forward, &block);
        let built_last = visit(&reverse, &backwards);
        assert_eq!(built_first.len(), built_last.len());
        assert!(
            built_first
                .iter()
                .any(|column| column.iter().any(|name| name.is_some())),
            "the fixture must produce terrain"
        );
        assert!(
            built_first.iter().any(|column| column
                .iter()
                .any(|name| name.is_some() && name.as_deref() == Some("minecraft:granite"))),
            "and veins to compare"
        );
        assert_eq!(
            built_first, built_last,
            "a chunk decorated after its neighbours differs from one decorated before them"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// Eviction of a decorated grid is a recomputation, and the write zone
    /// stays bounded: 300 chunk keys through `column_ids` may not pin 300
    /// grids of 196 KiB each.
    #[test]
    fn decorated_chunk_cache_stays_bounded_across_chunk_sweeps() {
        let (root, generator) = bounded_generator("cache-decorated");
        for chunk in 0..300i32 {
            let _ = generator.column_ids(chunk * 16 + 8, 3);
        }
        let occupancy = generator.cache_occupancy();
        assert!(
            occupancy.decorated_chunks > 0,
            "the sweep did not populate the decorated-chunk memo: {occupancy:?}"
        );
        assert!(
            occupancy.decorated_chunks <= 2 * VanillaGenerator::DECORATED_CHUNK_CACHE_CAPACITY,
            "decorated grids grew past their bound: {occupancy:?}"
        );
        assert!(
            occupancy.decorated_chunks < 300,
            "one key per chunk and the sweep spans 300, so a growing map would pin all of \
             them: {occupancy:?}"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// An evicted grid rebuilds to the same blocks, veins included, which is
    /// what makes the bound above safe to apply.
    #[test]
    fn an_evicted_decorated_chunk_rebuilds_identically() {
        let (root, generator) = decorating_generator("decorate-eviction");
        let before = chunk_columns(&generator, 0, 0);
        assert!(
            before
                .iter()
                .flatten()
                .any(|name| name.as_deref() == Some("minecraft:granite")),
            "the fixture chunk must hold vein blocks to compare"
        );
        // More chunk keys than the memo holds, none of them this chunk's.
        for chunk in 0..60i32 {
            let _ = generator.column_ids(chunk * 16 + 8, 91);
        }
        assert_eq!(
            chunk_columns(&generator, 0, 0),
            before,
            "eviction must be a recomputation"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// The fill pass seeds the whole chunk heightmap: after the 256 columns
    /// of a chunk are built, every row of its ocean-floor map is stored, and
    /// each stored row equals what the reference descent — surface top, then
    /// down past every row the post-carve substance does not count as
    /// terrain — answers for that column. The dry pack exercises rows whose
    /// top is already solid (the walk stops at once); the wet pack, a sea
    /// above the volume, exercises columns whose descent crosses more than a
    /// hundred fluid rows before the sea floor, and both at negative and
    /// positive chunk coordinates.
    #[test]
    fn the_fill_pass_seeds_the_heightmap_and_matches_the_descent() {
        let root = scratch_root("heightmap-fill");
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("density_function/band.json"),
            r#"{"type": "mul",
                "left": {"type": "gradient", "axis": "y",
                    "from_coordinate": 0, "from_value": -1.0,
                    "to_coordinate": 8, "to_value": 1.0},
                "right": {"type": "gradient", "axis": "y",
                    "from_coordinate": 0, "from_value": 1.0,
                    "to_coordinate": 48, "to_value": -1.0}}"#,
        );
        write(&worldgen.join("material_rule/root.json"), DESCENT_ROOT);
        descent_pack_noise(&root);
        descent_pack_settings(&root, "dry", "band", -1000, "root");
        descent_pack_settings(&root, "wet", "band", 200, "root");
        for (id, floor, fluid_above) in [
            ("testns:dry", 23i32, false),
            // A sea above the volume: the surface top is the water ceiling
            // and the ocean floor is still the run's top solid row.
            ("testns:wet", 23, true),
        ] {
            let generator = VanillaGenerator::new(&root, 2026, id).expect("generator");
            for (chunk_x, chunk_z) in [(1, 0), (-2, 3)] {
                for local_x in 0..16 {
                    for local_z in 0..16 {
                        let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                        let _ = generator.column_ids(x, z);
                    }
                }
                let map = generator
                    .ocean_floor_maps
                    .borrow_mut()
                    .get_mut(&(chunk_x, chunk_z))
                    .expect("the fill pass seeded the chunk map")
                    .clone();
                for local_x in 0..16 {
                    for local_z in 0..16 {
                        let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                        let stored = map.entries[OceanFloorMap::slot(x, z)];
                        assert_ne!(
                            stored, UNCOMPUTED_OCEAN_FLOOR,
                            "({x}, {z}): the fill pass must seed every row"
                        );
                        assert_eq!(
                            stored,
                            generator.ocean_floor_by_descent(x, z),
                            "({x}, {z}): fill-derived row against the descent"
                        );
                        assert_eq!(
                            generator.ocean_floor_height(x, z),
                            stored,
                            "({x}, {z}): the served row equals the stored row"
                        );
                        assert_eq!(stored, floor, "({x}, {z}): the sea floor row");
                        if fluid_above {
                            assert!(
                                generator.surface_height(x, z) > floor,
                                "({x}, {z}): the wet column descends through fluid"
                            );
                        }
                    }
                }
            }
            assert_eq!(
                generator.cache_occupancy().ocean_floor_maps,
                2,
                "two target chunks filled, two maps: the fill pass adds no halo keys"
            );
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// Halo and eviction cannot move an answer. A carved, decorated pack is
    /// queried in three orders — plain gate lookups over a block of chunks
    /// including negative coordinates, then whole-chunk fills that overwrite
    /// the lazily stored halo rows with fill-derived rows, then a sweep past
    /// the map cache's bound so every original map rotates out and back. In
    /// every phase the answer per column equals the reference descent.
    #[test]
    fn heightmap_halo_lookups_and_eviction_match_the_descent() {
        let root = scratch_root("heightmap-halo");
        bounded_pack(&root);
        // Same pack, but the carver actually carves: rows it stamps sit
        // inside the solid band, so descents cross post-carve substance.
        write(
            &root.join("data/testns/worldgen/carver/scarce.json"),
            r#"{"type": "minecraft:cave", "probability": 1.0,
                "y": {"type": "minecraft:uniform",
                    "min_inclusive": {"absolute": 6},
                    "max_inclusive": {"absolute": 22}},
                "count": 4, "thickness": 1.5,
                "room_vertical_radius_multiplier": 1.0,
                "horizontal_radius_multiplier": 1.0,
                "vertical_radius_multiplier": 1.0,
                "floor_level": -0.7}"#,
        );
        let generator =
            VanillaGenerator::new(&root, 2026, "testns:bounded").expect("carving pack generator");
        // A three-chunk block straddling the origin's negatives.
        let area: Vec<(i32, i32)> = (-3..=3)
            .flat_map(|chunk_x| (-2..=2).map(move |chunk_z| (chunk_x, chunk_z)))
            .collect();
        let mut carved_rows = 0usize;
        let mut reference = Vec::new();
        for &(chunk_x, chunk_z) in &area {
            for local_x in 0..16 {
                for local_z in 0..16 {
                    let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                    let descent = generator.ocean_floor_by_descent(x, z);
                    for y in generator.min_y()..generator.min_y() + generator.height() {
                        if generator.carved(x, y, z) {
                            carved_rows += 1;
                        }
                    }
                    reference.push((x, z, descent));
                }
            }
        }
        assert!(carved_rows > 0, "the fixture must actually carve rows");
        // Phase 1: gate-style lookups (lazy per-column recording).
        for &(x, z, descent) in &reference {
            assert_eq!(generator.ocean_floor_height(x, z), descent, "({x}, {z})");
        }
        // Phase 2: fill the same chunks; the fill-derived rows must agree
        // with the descent exactly where the lazy rows sit.
        for &(chunk_x, chunk_z) in &area {
            for local_x in 0..16 {
                for local_z in 0..16 {
                    let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                    let _ = generator.column_ids(x, z);
                }
            }
        }
        for &(x, z, descent) in &reference {
            assert_eq!(
                generator.ocean_floor_height(x, z),
                descent,
                "({x}, {z}) after the fill overwrote the map"
            );
        }
        // Phase 3: rotate every one of these maps out of the cache with
        // far-apart chunk keys, then answer again from rebuilt maps.
        for chunk in 0..(4 * VanillaGenerator::OCEAN_FLOOR_MAP_CACHE_CAPACITY) as i32 {
            let _ = generator.ocean_floor_height(chunk * 512 + 7, -4 * chunk + 3);
        }
        let occupancy = generator.cache_occupancy();
        assert!(
            occupancy.ocean_floor_maps <= 2 * VanillaGenerator::OCEAN_FLOOR_MAP_CACHE_CAPACITY,
            "heightmap cache grew past its bound: {occupancy:?}"
        );
        assert!(
            occupancy.ocean_floor_maps
                < area.len() + 4 * VanillaGenerator::OCEAN_FLOOR_MAP_CACHE_CAPACITY,
            "the sweep touched one map per chunk of the area plus four per capacity, so a \
             growing map would pin all of them: {occupancy:?}"
        );
        for &(x, z, descent) in &reference {
            assert_eq!(
                generator.ocean_floor_height(x, z),
                descent,
                "({x}, {z}) after eviction"
            );
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// The chunk-and-slot split of an absolute column is the Euclidean one
    /// the decorated grid already uses, at negative coordinates too.
    #[test]
    fn heightmap_slots_split_absolute_columns_euclideanly() {
        for x in [
            -4_194_321i32,
            -70_824,
            -300,
            -17,
            -16,
            -1,
            0,
            1,
            15,
            16,
            303,
        ] {
            for z in [-70_824i32, -33, -16, -1, 0, 7, 16, 511] {
                let slot = OceanFloorMap::slot(x, z);
                assert!(slot < 256, "({x}, {z}) → {slot}");
                let (chunk_x, chunk_z) = (x >> 4, z >> 4);
                // Euclidean round trip: chunk base plus local offset is back.
                assert_eq!(
                    (
                        chunk_x * 16 + (slot as i32 / 16),
                        chunk_z * 16 + (slot as i32 % 16)
                    ),
                    (x, z),
                    "({x}, {z}) → chunk ({chunk_x}, {chunk_z}) slot {slot}"
                );
            }
        }
    }
}
