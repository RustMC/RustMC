//! Block columns: top blocks, decorated chunks and the pre-decoration
//! surface-rule descent of one column.

use super::*;

impl VanillaGenerator {
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
    pub(super) fn fluid_name(fluid: Fluid) -> &'static str {
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
    pub(super) fn decorated_chunk(&self, chunk_x: i32, chunk_z: i32) -> Rc<DecoratedChunk> {
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
    pub(super) fn build_decorated_chunk(&self, chunk_x: i32, chunk_z: i32) -> DecoratedChunk {
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
    pub(super) fn base_column_ids(&self, x: i32, z: i32) -> Vec<Option<String>> {
        self.base_column_ids_with_heights(x, z).0
    }

    /// Resolves one solid pre-decoration block id at `(x, y, z)` using the
    /// same top-down descent counters (`stone_above`, `stone_below`, `water_height`)
    /// as [`Self::base_column_ids_with_heights`], without evaluating surface rules
    /// on lower rows or allocating a full column vector.
    pub(super) fn base_solid_block_at(&self, x: i32, y: i32, z: i32) -> String {
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
    pub(super) fn base_column_ids_with_heights(
        &self,
        x: i32,
        z: i32,
    ) -> (Vec<Option<String>>, i32, i32) {
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
}
