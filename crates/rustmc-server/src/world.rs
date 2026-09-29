//! Original coordinate-derived preview terrain; no vanilla generation parity.

pub const CHUNK_SIDE: usize = 16;
pub const WORLD_HEIGHT: usize = 128;
const TREE_CELL: i64 = 8;
const TREE_REACH: i64 = 3;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Air,
    Bedrock,
    Stone,
    Dirt,
    Grass,
    Log,
    Leaves,
    Sand,
    Sandstone,
    Snow,
    SpruceLog,
    SpruceLeaves,
    AcaciaLog,
    AcaciaLeaves,
    JungleLog,
    JungleLeaves,
    RedSand,
    RedSandstone,
    Terracotta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Biome {
    Plains,
    Forest,
    Desert,
    SnowyPlains,
    Taiga,
    Savanna,
    Jungle,
    Badlands,
}

impl Biome {
    pub const ALL: [Self; 8] = [
        Self::Plains,
        Self::Forest,
        Self::Desert,
        Self::SnowyPlains,
        Self::Taiga,
        Self::Savanna,
        Self::Jungle,
        Self::Badlands,
    ];

    pub const fn identifier(self) -> &'static str {
        match self {
            Self::Plains => "minecraft:plains",
            Self::Forest => "minecraft:forest",
            Self::Desert => "minecraft:desert",
            Self::SnowyPlains => "minecraft:snowy_plains",
            Self::Taiga => "minecraft:taiga",
            Self::Savanna => "minecraft:savanna",
            Self::Jungle => "minecraft:jungle",
            Self::Badlands => "minecraft:badlands",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub x: i32,
    pub z: i32,
    blocks: Vec<Block>,
}
impl Chunk {
    pub fn block(&self, x: usize, y: usize, z: usize) -> Option<Block> {
        if x >= CHUNK_SIDE || y >= WORLD_HEIGHT || z >= CHUNK_SIDE {
            return None;
        }
        Some(self.blocks[(y * CHUNK_SIDE + z) * CHUNK_SIDE + x])
    }
    fn set(&mut self, x: i64, y: i64, z: i64, block: Block) {
        if y < 0 || y >= WORLD_HEIGHT as i64 {
            return;
        }
        let local_x = x - i64::from(self.x) * CHUNK_SIDE as i64;
        let local_z = z - i64::from(self.z) * CHUNK_SIDE as i64;
        if !(0..CHUNK_SIDE as i64).contains(&local_x) || !(0..CHUNK_SIDE as i64).contains(&local_z)
        {
            return;
        }
        let index = (y as usize * CHUNK_SIDE + local_z as usize) * CHUNK_SIDE + local_x as usize;
        if matches!(
            block,
            Block::Leaves | Block::SpruceLeaves | Block::AcaciaLeaves | Block::JungleLeaves
        ) && self.blocks[index] != Block::Air
        {
            return;
        }
        self.blocks[index] = block;
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Generator {
    seed: u64,
}
impl Generator {
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }
    pub fn seed(self) -> u64 {
        self.seed
    }

    pub fn biome(self, x: i64, z: i64) -> Biome {
        // Smoothly bend cell boundaries, then choose the closest jittered site.
        // All inputs are world coordinates, including the negative quadrants.
        const REGION: i64 = 192;
        let warped_x = x + self.sample(x, z, 96, 0x42494f4d4558) * 2;
        let warped_z = z + self.sample(x, z, 96, 0x42494f4d455a) * 2;
        let cell_x = warped_x.div_euclid(REGION);
        let cell_z = warped_z.div_euclid(REGION);
        let mut nearest = (i128::MAX, 0usize);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let cx = cell_x + dx;
                let cz = cell_z + dz;
                let site_x = cx * REGION + 48 + (self.hash(cx, cz, 31) % 96) as i64;
                let site_z = cz * REGION + 48 + (self.hash(cx, cz, 37) % 96) as i64;
                let a = i128::from(warped_x - site_x);
                let b = i128::from(warped_z - site_z);
                let distance = a * a + b * b;
                let kind = (self.hash(cx, cz, 41) % Biome::ALL.len() as u64) as usize;
                if distance < nearest.0 {
                    nearest = (distance, kind);
                }
            }
        }
        Biome::ALL[nearest.1]
    }

    fn hash(self, x: i64, z: i64, salt: u64) -> u64 {
        let mut v = self.seed
            ^ salt
            ^ (x as u64).wrapping_mul(0x9e3779b185ebca87)
            ^ (z as u64).wrapping_mul(0xc2b2ae3d27d4eb4f);
        v ^= v >> 30;
        v = v.wrapping_mul(0xbf58476d1ce4e5b9);
        v ^= v >> 27;
        v = v.wrapping_mul(0x94d049bb133111eb);
        v ^ (v >> 31)
    }
    fn sample(self, x: i64, z: i64, spacing: i64, salt: u64) -> i64 {
        let x0 = x.div_euclid(spacing);
        let z0 = z.div_euclid(spacing);
        let fx = x.rem_euclid(spacing);
        let fz = z.rem_euclid(spacing);
        let node = |dx, dz| (self.hash(x0 + dx, z0 + dz, salt) % 65) as i64 - 32;
        let a = node(0, 0) * (spacing - fx) + node(1, 0) * fx;
        let b = node(0, 1) * (spacing - fx) + node(1, 1) * fx;
        (a * (spacing - fz) + b * fz) / (spacing * spacing)
    }
    pub fn height(self, x: i64, z: i64) -> i64 {
        (66 + self.sample(x, z, 48, 0x5445525241494e) / 3 + self.sample(x, z, 12, 0x48494c4c53) / 8)
            .clamp(42, 91)
    }
    fn clearing(self, x: i64, z: i64) -> bool {
        self.sample(x, z, 32, 0x434c454152) > 13
    }
    fn tree_root(self, cell_x: i64, cell_z: i64) -> Option<(i64, i64)> {
        let x = cell_x * TREE_CELL + 2 + (self.hash(cell_x, cell_z, 1) % 4) as i64;
        let z = cell_z * TREE_CELL + 2 + (self.hash(cell_x, cell_z, 2) % 4) as i64;
        let threshold = match self.biome(x, z) {
            Biome::Plains => 12,
            Biome::Forest => 58,
            Biome::SnowyPlains => 6,
            Biome::Taiga => 48,
            Biome::Savanna => 20,
            Biome::Jungle => 65,
            Biome::Desert | Biome::Badlands => 0,
        };
        (self.hash(cell_x, cell_z, 3) % 100 < threshold && !self.clearing(x, z)).then_some((x, z))
    }
    pub fn generate(self, chunk_x: i32, chunk_z: i32) -> Chunk {
        let mut chunk = Chunk {
            x: chunk_x,
            z: chunk_z,
            blocks: vec![Block::Air; CHUNK_SIDE * CHUNK_SIDE * WORLD_HEIGHT],
        };
        let min_x = i64::from(chunk_x) * CHUNK_SIDE as i64;
        let min_z = i64::from(chunk_z) * CHUNK_SIDE as i64;
        for local_z in 0..CHUNK_SIDE {
            for local_x in 0..CHUNK_SIDE {
                let x = min_x + local_x as i64;
                let z = min_z + local_z as i64;
                let top = self.height(x, z) as usize;
                let biome = self.biome(x, z);
                for y in 0..=top {
                    let block = if y == 0 {
                        Block::Bedrock
                    } else if biome == Biome::Desert {
                        if y + 4 >= top {
                            Block::Sand
                        } else {
                            Block::Sandstone
                        }
                    } else if biome == Biome::Badlands {
                        if y + 4 >= top {
                            Block::RedSand
                        } else {
                            Block::Terracotta
                        }
                    } else if y == top {
                        if biome == Biome::SnowyPlains {
                            Block::Snow
                        } else {
                            Block::Grass
                        }
                    } else if y + 4 >= top {
                        Block::Dirt
                    } else {
                        Block::Stone
                    };
                    chunk.set(x, y as i64, z, block);
                }
            }
        }
        for cell_z in (min_z - TREE_REACH).div_euclid(TREE_CELL)
            ..=(min_z + CHUNK_SIDE as i64 - 1 + TREE_REACH).div_euclid(TREE_CELL)
        {
            for cell_x in (min_x - TREE_REACH).div_euclid(TREE_CELL)
                ..=(min_x + CHUNK_SIDE as i64 - 1 + TREE_REACH).div_euclid(TREE_CELL)
            {
                let Some((x, z)) = self.tree_root(cell_x, cell_z) else {
                    continue;
                };
                let (log, leaves) = match self.biome(x, z) {
                    Biome::Taiga | Biome::SnowyPlains => (Block::SpruceLog, Block::SpruceLeaves),
                    Biome::Savanna => (Block::AcaciaLog, Block::AcaciaLeaves),
                    Biome::Jungle => (Block::JungleLog, Block::JungleLeaves),
                    _ => (Block::Log, Block::Leaves),
                };
                let base = self.height(x, z) + 1;
                for y in base..base + 5 {
                    chunk.set(x, y, z, log);
                }
                for dy in 3..=6 {
                    let radius: i64 = if dy == 6 { 1 } else { 2 };
                    for dz in -radius..=radius {
                        for dx in -radius..=radius {
                            if dx.abs() == radius && dz.abs() == radius && dy == 3 {
                                continue;
                            }
                            chunk.set(x + dx, base + dy, z + dz, leaves);
                        }
                    }
                }
            }
        }
        chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_seed_and_coordinates_are_stable() {
        let g = Generator::new(42);
        assert_eq!(g.generate(-1, 2), g.generate(-1, 2));
        assert_ne!(g.generate(-1, 2), Generator::new(43).generate(-1, 2));
    }
    #[test]
    fn request_order_does_not_change_chunks() {
        let g = Generator::new(1234);
        let first = g.generate(0, 0);
        let neighbor = g.generate(1, 0);
        assert_eq!(neighbor, g.generate(1, 0));
        assert_eq!(first, g.generate(0, 0));
    }
    #[test]
    fn terrain_layers_and_bounded_height() {
        let g = Generator::new(9);
        let c = g.generate(0, 0);
        for z in 0..16 {
            for x in 0..16 {
                let h = g.height(x as i64, z as i64) as usize;
                assert_eq!(c.block(x, 0, z), Some(Block::Bedrock));
                let biome = g.biome(x as i64, z as i64);
                let top = match biome {
                    Biome::Desert => Block::Sand,
                    Biome::Badlands => Block::RedSand,
                    Biome::SnowyPlains => Block::Snow,
                    _ => Block::Grass,
                };
                let below = match biome {
                    Biome::Desert => Block::Sand,
                    Biome::Badlands => Block::RedSand,
                    _ => Block::Dirt,
                };
                assert_eq!(c.block(x, h, z), Some(top));
                assert_eq!(c.block(x, h - 1, z), Some(below));
                assert_eq!(c.block(x, WORLD_HEIGHT - 1, z), Some(Block::Air));
            }
        }
    }
    #[test]
    fn trees_can_cross_chunk_edges_without_order_dependence() {
        let mut found = false;
        for seed in 0..500 {
            let g = Generator::new(seed);
            let a = g.generate(0, 0);
            let b = g.generate(1, 0);
            for z in 0..16 {
                for y in 55..100 {
                    if matches!(
                        a.block(15, y, z),
                        Some(
                            Block::Leaves
                                | Block::SpruceLeaves
                                | Block::AcaciaLeaves
                                | Block::JungleLeaves
                        )
                    ) && matches!(
                        b.block(0, y, z),
                        Some(
                            Block::Leaves
                                | Block::SpruceLeaves
                                | Block::AcaciaLeaves
                                | Block::JungleLeaves
                        )
                    ) {
                        found = true;
                        assert_eq!(a, g.generate(0, 0));
                        assert_eq!(b, g.generate(1, 0));
                        break;
                    }
                }
            }
            if found {
                break;
            }
        }
        assert!(found);
    }
    #[test]
    fn heights_vary_and_clearings_exclude_tree_roots() {
        let g = Generator::new(2026);
        let heights: std::collections::BTreeSet<_> = (-64..64)
            .step_by(8)
            .flat_map(|z| (-64..64).step_by(8).map(move |x| g.height(x, z)))
            .collect();
        assert!(heights.len() > 5);
        let mut clearings = 0;
        for z in -10..10 {
            for x in -10..10 {
                if let Some((root_x, root_z)) = g.tree_root(x, z) {
                    assert!(!g.clearing(root_x, root_z));
                }
                let candidate_x = x * TREE_CELL + 2 + (g.hash(x, z, 1) % 4) as i64;
                let candidate_z = z * TREE_CELL + 2 + (g.hash(x, z, 2) % 4) as i64;
                if g.clearing(candidate_x, candidate_z) {
                    clearings += 1;
                    assert!(g.tree_root(x, z).is_none());
                }
            }
        }
        assert!(clearings > 0);
    }

    #[test]
    fn biomes_cover_all_preview_regions_and_are_coordinate_stable() {
        let generator = Generator::new(2026);
        let mut seen = std::collections::BTreeSet::new();
        for z in (-2048..2048).step_by(96) {
            for x in (-2048..2048).step_by(96) {
                let biome = generator.biome(x, z);
                assert_eq!(biome, generator.biome(x, z));
                seen.insert(biome.identifier());
            }
        }
        assert_eq!(seen.len(), Biome::ALL.len());
        assert_ne!(
            generator.biome(-192, -192),
            Generator::new(2027).biome(-192, -192)
        );
    }
}
