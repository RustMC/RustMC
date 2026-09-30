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

use std::path::Path;

use crate::vanilla::aquifer::{Aquifer, GlobalFluid, NoiseBasedAquifer, Substance};
use crate::vanilla::worldgen::{NoiseRouter, WorldgenData, WorldgenError};

/// One dimension's column-height source, built from operator-provisioned
/// data (never committed) and a world seed. The compiled router owns its
/// graph, so the loaded pack and engine may be dropped once wired.
pub struct VanillaGenerator {
    router: NoiseRouter,
    max_y: i32,
    aquifer: Aquifer,
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
        Ok(Self {
            router,
            max_y,
            aquifer,
        })
    }

    /// Absolute Y of the top written block: terrain, or the aquifer/sea
    /// fluid surface where terrain does not reach above it.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        self.surface(x, z).map_or(self.router.min_y, |(y, _)| y)
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
}
