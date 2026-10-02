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
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use crate::vanilla::aquifer::{Aquifer, Fluid, GlobalFluid, NoiseBasedAquifer, Substance};
use crate::vanilla::biome::{BiomePlacement, ClimateSampler};
use crate::vanilla::carver::{CarveMask, Carver, CarverContext, CarverData};
use crate::vanilla::random::LegacyRandom;
use crate::vanilla::surface::{SurfaceContext, SurfaceRules};
use crate::vanilla::worldgen::{NoiseRouter, WorldgenData, WorldgenError};

/// `DimensionType.WAY_BELOW_MIN_Y`: the descent ceiling sentinel when the
/// column is solid all the way to the floor (recorded in `PROVENANCE.md`).
const WAY_BELOW_MIN_Y: i32 = -32512;

/// A biome's resolved carver list, shared between the registry and the
/// per-source-chunk cache.
type ChunkCarvers = Rc<Vec<Option<Rc<Carver>>>>;

/// One dimension's column-height, biome, and top-block source, built from
/// operator-provisioned data (never committed) and a world seed. The
/// compiled router owns its graph, and the surface program owns every
/// noise stack and random factory it needs, so the engine is used only
/// while wiring.
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
    /// Per-target-chunk carving masks, built lazily on first probe.
    masks: RefCell<HashMap<(i32, i32), CarveMask>>,
    /// Per-source-chunk biome carver lists, built lazily during replay.
    chunk_carvers: RefCell<HashMap<(i32, i32), ChunkCarvers>>,
    /// Column-top memo: the descent's steep-gradient lookups re-read
    /// neighbouring columns, whose top scan is otherwise O(height).
    heights: RefCell<HashMap<(i32, i32), i32>>,
}

impl VanillaGenerator {
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
            masks: RefCell::new(HashMap::new()),
            chunk_carvers: RefCell::new(HashMap::new()),
            heights: RefCell::new(HashMap::new()),
        })
    }

    /// Biome identifier for the column at `(x, z)`, resolved from the
    /// climate target at the surface cell. `None` when the operator has
    /// not provisioned a placement table for this preset.
    pub fn biome(&self, x: i32, z: i32, surface_y: i32) -> Option<String> {
        let placement = self.placement.as_ref()?;
        self.climate.biome(placement, x, surface_y, z)
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
    pub fn column_ids(&self, x: i32, z: i32) -> Vec<Option<String>> {
        let min_y = self.router.min_y;
        let mut ids: Vec<Option<String>> = vec![None; self.router.height as usize];
        let Some(top) = self.surface(x, z).map(|(y, _)| y) else {
            return ids;
        };
        self.heights.borrow_mut().insert((x, z), top);
        // The column as the surface pass sees it: the registry carvers
        // have not stamped it yet.
        let density = &self.router.final_density;
        let filled: Vec<Substance> = (min_y..=top)
            .map(|y| self.aquifer.substance(x, y, z, density.sample(x, y, z)))
            .collect();
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
        ids
    }

    /// Absolute Y of the top written block: terrain, or the aquifer/sea
    /// fluid surface where terrain does not reach above it. Memoised
    /// because the descent's steep gradients re-read neighbours.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        if let Some(&y) = self.heights.borrow().get(&(x, z)) {
            return y;
        }
        let y = self.surface(x, z).map_or(self.router.min_y, |(y, _)| y);
        self.heights.borrow_mut().insert((x, z), y);
        y
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
            let cached = self.masks.borrow();
            if let Some(mask) = cached.get(&target) {
                return mask.contains(relative_x, y, relative_z);
            }
        }
        let mask = self.build_carve_mask(target);
        let hit = mask.contains(relative_x, y, relative_z);
        self.masks.borrow_mut().insert(target, mask);
        hit
    }

    /// Replays the orchestration for one target chunk: one legacy stream
    /// reseeded per source chunk and carver index over the 17×17 window,
    /// the probability gate, then the walk stamping into the mask.
    fn build_carve_mask(&self, target: (i32, i32)) -> CarveMask {
        let mut mask = CarveMask::new(
            self.router.min_y + 1,
            self.router.min_y + self.router.height - 1 - 7,
        );
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
            let cached = self.chunk_carvers.borrow();
            if let Some(carvers) = cached.get(&chunk) {
                return Some(Rc::clone(carvers));
            }
        }
        let placement = self.placement.as_ref()?;
        let data = self.carvers.as_ref()?;
        let biome = self
            .climate
            .biome(placement, chunk.0 << 4, 0, chunk.1 << 4)?;
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
    /// must reproduce the deepslate split below the transition band and
    /// fire both ore-vein rules: granite and tuff are vein fillers the
    /// rest of the program never writes, so any occurrence proves the
    /// copper and iron veins evaluated, and each must stay inside its
    /// documented window. The two 16×16 block squares sit on vein
    /// clusters found by scanning the density graphs at this seed.
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn smoke_column_ids_splits_deepslate_and_fires_veins() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
        let generator =
            VanillaGenerator::new(&root, 2026, "minecraft:overworld").expect("overworld generator");
        let min_y = generator.min_y();
        let mut granite = 0usize;
        let mut tuff = 0usize;
        let mut unexpected_deep: Vec<(i32, i32, i32, String)> = Vec::new();
        for (origin_x, origin_z) in [(-1024, -880), (-824, -149)] {
            for x in origin_x..origin_x + 16 {
                for z in origin_z..origin_z + 16 {
                    let ids = generator.column_ids(x, z);
                    for (index, id) in ids.iter().enumerate() {
                        let Some(id) = id else { continue };
                        let y = min_y + index as i32;
                        match id.as_str() {
                            "minecraft:granite" => {
                                assert!(y < 50, "granite at y={y} outside the copper window");
                                granite += 1;
                            }
                            "minecraft:tuff" => {
                                assert!(y < -8, "tuff at y={y} outside the iron window");
                                tuff += 1;
                            }
                            _ => {}
                        }
                        // Between the iron window and the deepslate/stone
                        // transition only the underground rule, carving and
                        // the dimension fluids can have written a row.
                        if (-8..=-2).contains(&y)
                            && !matches!(
                                id.as_str(),
                                "minecraft:deepslate"
                                    | "minecraft:cave_air"
                                    | "minecraft:water"
                                    | "minecraft:lava"
                            )
                        {
                            unexpected_deep.push((x, z, y, id.clone()));
                        }
                    }
                }
            }
        }
        assert!(granite > 0, "copper veins must leave granite filler");
        assert!(tuff > 0, "iron veins must leave tuff filler");
        assert!(
            unexpected_deep.is_empty(),
            "unexpected rows in the deep band: {unexpected_deep:?}"
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
}
