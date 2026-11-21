//! Terrain queries: surface and ocean-floor heights, density,
//! substance, carving masks and cache occupancy.

use super::*;

impl VanillaGenerator {
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
    pub(super) fn ocean_floor_by_descent(&self, x: i32, z: i32) -> i32 {
        let mut y = self.surface_height(x, z);
        while y > self.min_y() && self.substance(x, y, z) != Substance::Solid {
            y -= 1;
        }
        y
    }

    /// Writes one column's heightmap row into its chunk's map, creating the
    /// map when this is the chunk's first stored row.
    pub(super) fn record_ocean_floor(&self, x: i32, z: i32, y: i32) {
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
    pub(super) fn chunk_density(&self, chunk_x: i32, chunk_z: i32) -> ChunkDensities {
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
    pub(super) fn column_density_range(&self, x: i32, z: i32) -> std::ops::Range<usize> {
        let height = self.router.height as usize;
        let local_x = (x & 15) as usize;
        let local_z = (z & 15) as usize;
        let start = (local_x + local_z * 16) * height;
        start..start + height
    }

    /// Indexes a column slice from [`Self::chunk_density`] for in-range `y`
    /// and falls back to point evaluation outside `[min_y, max_y]`.
    pub(super) fn column_density_value(&self, col: &[f32], x: i32, y: i32, z: i32) -> f32 {
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
    pub(super) fn density_at(&self, x: i32, y: i32, z: i32) -> f32 {
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
    pub(super) fn mask_window(&self) -> (i32, i32) {
        (
            self.router.min_y + 1,
            self.router.min_y + self.router.height - 1 - 7,
        )
    }

    /// Replays the orchestration for one target chunk: one legacy stream
    /// reseeded per source chunk and carver index over the 17×17 window,
    /// the probability gate, then the walk stamping into the mask.
    pub(super) fn build_carve_mask(&self, target: (i32, i32)) -> CarveMask {
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
    pub(super) fn carvers_for_chunk(&self, chunk: (i32, i32)) -> Option<ChunkCarvers> {
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
