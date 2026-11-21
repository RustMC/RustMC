//! The decoration view of one chunk: block reads and writes, biomes and
//! heightmaps as feature placement sees them.

use super::*;

/// The air families a decoration pass's neighbour test treats as air: the
/// filler writes the named cave air at carved rows, and the pre-carve `None`
/// entry stands for the air above the terrain.
pub(super) const AIR_IDS: [&str; 3] = ["minecraft:air", "minecraft:cave_air", "minecraft:void_air"];

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
