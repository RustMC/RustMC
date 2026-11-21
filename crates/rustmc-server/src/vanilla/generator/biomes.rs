//! Biome lookup for the generator: point queries, per-chunk biome volumes
//! and the 3x3 region biome sets decoration needs.

use super::*;

impl VanillaGenerator {
    pub(super) fn can_serve_chunk_biomes(&self) -> bool {
        self.placement.is_some() && (self.router.min_y & 3) == 0 && (self.router.height & 3) == 0
    }

    pub(super) fn biome_arc(&self, x: i32, z: i32, surface_y: i32) -> Option<Arc<str>> {
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
    pub(super) fn chunk_biomes(&self, chunk_x: i32, chunk_z: i32) -> Arc<ChunkBiomes> {
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
    pub(super) fn resolve_biome_column(
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
    pub(super) fn region_biomes(&self, chunk_x: i32, chunk_z: i32) -> Vec<Arc<str>> {
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
}
