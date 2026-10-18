//! Original coordinate-derived preview terrain; no vanilla generation parity.

pub mod journal;
pub use journal::WorldJournal;

#[cfg(test)]
mod tests_interaction;
#[cfg(test)]
mod tests_persistence;

pub const CHUNK_SIDE: usize = 16;
pub const WORLD_HEIGHT: usize = 128;
const TREE_CELL: i64 = 8;
const TREE_REACH: i64 = 3;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

impl Block {
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Air),
            1 => Some(Self::Bedrock),
            2 => Some(Self::Stone),
            3 => Some(Self::Dirt),
            4 => Some(Self::Grass),
            5 => Some(Self::Log),
            6 => Some(Self::Leaves),
            7 => Some(Self::Sand),
            8 => Some(Self::Sandstone),
            9 => Some(Self::Snow),
            10 => Some(Self::SpruceLog),
            11 => Some(Self::SpruceLeaves),
            12 => Some(Self::AcaciaLog),
            13 => Some(Self::AcaciaLeaves),
            14 => Some(Self::JungleLog),
            15 => Some(Self::JungleLeaves),
            16 => Some(Self::RedSand),
            17 => Some(Self::RedSandstone),
            18 => Some(Self::Terracotta),
            _ => None,
        }
    }
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
    pub fn set_block(&mut self, x: i64, y: i64, z: i64, block: Block) -> bool {
        if y < 0 || y >= WORLD_HEIGHT as i64 {
            return false;
        }
        let local_x = x - i64::from(self.x) * CHUNK_SIDE as i64;
        let local_z = z - i64::from(self.z) * CHUNK_SIDE as i64;
        if !(0..CHUNK_SIDE as i64).contains(&local_x) || !(0..CHUNK_SIDE as i64).contains(&local_z)
        {
            return false;
        }
        let index = (y as usize * CHUNK_SIDE + local_z as usize) * CHUNK_SIDE + local_x as usize;
        self.blocks[index] = block;
        true
    }
}

/// Which deterministic terrain field produces column heights. `Preview` is
/// the accepted ADR-0013 surface; `Experimental` is the T1 groundwork noise
/// stack (docs/research/vanilla-worldgen-feasibility.md) and is opt-in.
/// Neither claims vanilla generation parity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terrain {
    Preview,
    Experimental,
}

impl Terrain {
    pub const fn to_u8(self) -> u8 {
        match self {
            Self::Preview => 0,
            Self::Experimental => 1,
        }
    }

    pub const fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Preview),
            1 => Some(Self::Experimental),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Generator {
    seed: u64,
    terrain: Terrain,
}
impl Generator {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            terrain: Terrain::Preview,
        }
    }
    pub fn with_terrain(seed: u64, terrain: Terrain) -> Self {
        Self { seed, terrain }
    }
    pub fn terrain(self) -> Terrain {
        self.terrain
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

    /// Smoothstep-interpolated lattice noise on [-1.0, 1.0]. Only integer
    /// divisions and IEEE add/multiply are used, so results are reproducible
    /// for identical inputs.
    fn eased(self, x: i64, z: i64, spacing: i64, salt: u64) -> f64 {
        let gx = x.div_euclid(spacing);
        let gz = z.div_euclid(spacing);
        let u = x.rem_euclid(spacing) as f64 / spacing as f64;
        let v = z.rem_euclid(spacing) as f64 / spacing as f64;
        let su = u * u * (3.0 - 2.0 * u);
        let sv = v * v * (3.0 - 2.0 * v);
        let node = |dx: i64, dz: i64| {
            (self.hash(gx + dx, gz + dz, salt) % 2_000_001) as f64 / 1_000_000.0 - 1.0
        };
        let n00 = node(0, 0);
        let n10 = node(1, 0);
        let n01 = node(0, 1);
        let n11 = node(1, 1);
        let a = n00 + (n10 - n00) * su;
        let b = n01 + (n11 - n01) * su;
        a + (b - a) * sv
    }

    pub fn height(self, x: i64, z: i64) -> i64 {
        match self.terrain {
            Terrain::Preview => (66
                + self.sample(x, z, 48, 0x5445525241494e) / 3
                + self.sample(x, z, 12, 0x48494c4c53) / 8)
                .clamp(42, 91),
            Terrain::Experimental => {
                // Broad continents, rolling hills, and ridged mountains that
                // only appear where a wide amplifier field is positive.
                let continents = self.eased(x, z, 512, 0x434f4e54_494e454e) * 26.0;
                let hills = self.eased(x, z, 96, 0x48494c4c535f4558) * 9.0;
                let ridge = 1.0 - self.eased(x, z, 256, 0x52494447455f4558).abs();
                let amplifier = self.eased(x, z, 384, 0x414d505f4558).max(0.0) * 2.0;
                let mountains = (ridge * amplifier).min(1.0) * 28.0;
                (64.0 + continents + hills + mountains)
                    .round()
                    .clamp(40.0, 108.0) as i64
            }
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockMutation {
    pub x: i64,
    pub y: i64,
    pub z: i64,
    pub block: Block,
}

impl BlockMutation {
    pub const fn new(x: i64, y: i64, z: i64, block: Block) -> Self {
        Self { x, y, z, block }
    }
}

#[derive(Debug)]
pub enum WorldError {
    Io(std::io::Error),
    Corrupted(String),
    SeedMismatch { expected: u64, found: u64 },
    TerrainMismatch { expected: Terrain, found: Terrain },
    InvalidMagic,
    OutOfBounds { x: i64, y: i64, z: i64 },
    InvalidBlockId(u8),
    InvalidTerrainId(u8),
}

impl std::fmt::Display for WorldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Corrupted(msg) => write!(f, "corrupted world data: {msg}"),
            Self::SeedMismatch { expected, found } => {
                write!(f, "seed mismatch: expected {expected}, found {found}")
            }
            Self::TerrainMismatch { expected, found } => {
                write!(
                    f,
                    "terrain mismatch: expected {expected:?}, found {found:?}"
                )
            }
            Self::InvalidMagic => write!(f, "invalid file magic header"),
            Self::OutOfBounds { x, y, z } => {
                write!(f, "coordinates out of bounds: ({x}, {y}, {z})")
            }
            Self::InvalidBlockId(id) => write!(f, "invalid block id: {id}"),
            Self::InvalidTerrainId(id) => write!(f, "invalid terrain id: {id}"),
        }
    }
}

impl std::error::Error for WorldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WorldError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

#[derive(Debug)]
pub struct World {
    generator: Generator,
    chunk_cache: std::collections::BTreeMap<(i32, i32), Chunk>,
    committed_mutations: std::collections::BTreeMap<(i64, i64, i64), Block>,
    journal: Option<WorldJournal>,
    current_tick: u64,
    pending_tick_mutations: Vec<BlockMutation>,
}

impl World {
    pub fn new(generator: Generator) -> Self {
        Self {
            generator,
            chunk_cache: std::collections::BTreeMap::new(),
            committed_mutations: std::collections::BTreeMap::new(),
            journal: None,
            current_tick: 0,
            pending_tick_mutations: Vec::new(),
        }
    }

    pub fn open(
        path: impl AsRef<std::path::Path>,
        generator: Generator,
    ) -> Result<Self, WorldError> {
        let (journal, committed_mutations, current_tick) =
            WorldJournal::open(path.as_ref(), generator.seed(), generator.terrain())?;
        Ok(Self {
            generator,
            chunk_cache: std::collections::BTreeMap::new(),
            committed_mutations,
            journal: Some(journal),
            current_tick,
            pending_tick_mutations: Vec::new(),
        })
    }

    pub fn block(&mut self, x: i64, y: i64, z: i64) -> Option<Block> {
        if y < 0 || y >= WORLD_HEIGHT as i64 {
            return None;
        }
        if let Some(m) = self
            .pending_tick_mutations
            .iter()
            .rev()
            .find(|m| m.x == x && m.y == y && m.z == z)
        {
            return Some(m.block);
        }
        if let Some(&block) = self.committed_mutations.get(&(x, y, z)) {
            return Some(block);
        }
        let side = CHUNK_SIDE as i64;
        let chunk_x = i32::try_from(x.div_euclid(side)).ok()?;
        let chunk_z = i32::try_from(z.div_euclid(side)).ok()?;
        let chunk = self.chunk(chunk_x, chunk_z);
        chunk.block(
            x.rem_euclid(side) as usize,
            y as usize,
            z.rem_euclid(side) as usize,
        )
    }

    pub fn get_block(&self, x: i64, y: i64, z: i64) -> Option<Block> {
        if y < 0 || y >= WORLD_HEIGHT as i64 {
            return None;
        }
        if let Some(m) = self
            .pending_tick_mutations
            .iter()
            .rev()
            .find(|m| m.x == x && m.y == y && m.z == z)
        {
            return Some(m.block);
        }
        if let Some(&block) = self.committed_mutations.get(&(x, y, z)) {
            return Some(block);
        }
        let side = CHUNK_SIDE as i64;
        let chunk_x = i32::try_from(x.div_euclid(side)).ok()?;
        let chunk_z = i32::try_from(z.div_euclid(side)).ok()?;
        if let Some(chunk) = self.chunk_cache.get(&(chunk_x, chunk_z)) {
            chunk.block(
                x.rem_euclid(side) as usize,
                y as usize,
                z.rem_euclid(side) as usize,
            )
        } else {
            let chunk = self.generator.generate(chunk_x, chunk_z);
            chunk.block(
                x.rem_euclid(side) as usize,
                y as usize,
                z.rem_euclid(side) as usize,
            )
        }
    }

    pub fn chunk(&mut self, chunk_x: i32, chunk_z: i32) -> &Chunk {
        if !self.chunk_cache.contains_key(&(chunk_x, chunk_z)) {
            let mut chunk = self.generator.generate(chunk_x, chunk_z);
            let min_x = i64::from(chunk_x) * CHUNK_SIDE as i64;
            let max_x = min_x + CHUNK_SIDE as i64;
            let min_z = i64::from(chunk_z) * CHUNK_SIDE as i64;
            let max_z = min_z + CHUNK_SIDE as i64;
            for (&(x, y, z), &block) in &self.committed_mutations {
                if x >= min_x && x < max_x && z >= min_z && z < max_z {
                    chunk.set_block(x, y, z, block);
                }
            }
            for m in &self.pending_tick_mutations {
                if m.x >= min_x && m.x < max_x && m.z >= min_z && m.z < max_z {
                    chunk.set_block(m.x, m.y, m.z, m.block);
                }
            }
            self.chunk_cache.insert((chunk_x, chunk_z), chunk);
        }
        self.chunk_cache
            .get(&(chunk_x, chunk_z))
            .expect("chunk was just inserted")
    }

    pub fn set_block(&mut self, x: i64, y: i64, z: i64, block: Block) -> bool {
        if y < 0 || y >= WORLD_HEIGHT as i64 {
            return false;
        }
        let side = CHUNK_SIDE as i64;
        let Ok(chunk_x) = i32::try_from(x.div_euclid(side)) else {
            return false;
        };
        let Ok(chunk_z) = i32::try_from(z.div_euclid(side)) else {
            return false;
        };
        self.pending_tick_mutations
            .push(BlockMutation { x, y, z, block });
        if let Some(chunk) = self.chunk_cache.get_mut(&(chunk_x, chunk_z)) {
            chunk.set_block(x, y, z, block);
        }
        true
    }

    pub fn commit_tick(&mut self) -> Result<(), WorldError> {
        if let Some(journal) = &mut self.journal {
            journal.append_tick(self.current_tick, &self.pending_tick_mutations)?;
        }
        for m in self.pending_tick_mutations.drain(..) {
            self.committed_mutations.insert((m.x, m.y, m.z), m.block);
            let side = CHUNK_SIDE as i64;
            let Ok(chunk_x) = i32::try_from(m.x.div_euclid(side)) else {
                continue;
            };
            let Ok(chunk_z) = i32::try_from(m.z.div_euclid(side)) else {
                continue;
            };
            if let Some(chunk) = self.chunk_cache.get_mut(&(chunk_x, chunk_z)) {
                chunk.set_block(m.x, m.y, m.z, m.block);
            }
        }
        self.current_tick += 1;
        Ok(())
    }

    pub fn compact(&mut self) -> Result<(), WorldError> {
        if let Some(journal) = &mut self.journal {
            journal.compact(self.current_tick, &self.committed_mutations)?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), WorldError> {
        if let Some(journal) = &mut self.journal {
            journal.flush()?;
        }
        Ok(())
    }

    pub fn current_tick(&self) -> u64 {
        self.current_tick
    }

    pub fn generator(&self) -> Generator {
        self.generator
    }

    pub fn mutations(&self) -> &std::collections::BTreeMap<(i64, i64, i64), Block> {
        &self.committed_mutations
    }

    pub fn pending_mutations(&self) -> &[BlockMutation] {
        &self.pending_tick_mutations
    }

    pub fn journal(&self) -> Option<&WorldJournal> {
        self.journal.as_ref()
    }

    pub fn journal_mut(&mut self) -> Option<&mut WorldJournal> {
        self.journal.as_mut()
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
    fn experimental_terrain_is_stable_varied_and_continuous() {
        let g = Generator::with_terrain(2026, Terrain::Experimental);
        assert_eq!(g.height(1234, -5678), g.height(1234, -5678));
        assert_ne!(
            g.height(0, 0),
            Generator::with_terrain(2027, Terrain::Experimental).height(0, 0)
        );
        let mut min = i64::MAX;
        let mut max = i64::MIN;
        let mut prev = g.height(0, 37);
        for x in 1..4096 {
            let h = g.height(x, 37);
            min = min.min(h);
            max = max.max(h);
            assert!((40..=108).contains(&h), "height {h} at x={x}");
            assert!((h - prev).abs() <= 3, "cliff at x={x}: {prev} -> {h}");
            prev = h;
        }
        assert!(max - min > 25, "relief too flat: {min}..{max}");
        let negative: std::collections::BTreeSet<_> =
            (-512..512).step_by(64).map(|x| g.height(x, -64)).collect();
        assert!(negative.len() > 4);
    }

    #[test]
    fn preview_remains_the_default_terrain_field() {
        let a = Generator::new(2026);
        let b = Generator::with_terrain(2026, Terrain::Preview);
        assert_eq!(a.terrain(), Terrain::Preview);
        assert_eq!(a.height(999, -1001), b.height(999, -1001));
        assert_eq!(a.biome(999, -1001), b.biome(999, -1001));
        assert_eq!(a.generate(3, -4), b.generate(3, -4));
    }

    #[test]
    fn seed_2026_comparison_points_match_the_published_worksheet() {
        // Pins the RustMC column of docs/research/java-26.3-seed-comparison.md.
        // Changing the generator must update that worksheet (and its recorded
        // vanilla 26.3 comparison) in the same change.
        let generator = Generator::new(2026);
        const POINTS: [(i64, i64, i64, &str); 6] = [
            (0, 0, 71, "minecraft:badlands"),
            (256, 0, 61, "minecraft:plains"),
            (0, 256, 68, "minecraft:forest"),
            (-256, 0, 73, "minecraft:plains"),
            (0, -256, 65, "minecraft:badlands"),
            (512, 512, 65, "minecraft:taiga"),
        ];
        for (x, z, height, biome) in POINTS {
            assert_eq!(generator.height(x, z), height, "height at ({x}, {z})");
            assert_eq!(
                generator.biome(x, z).identifier(),
                biome,
                "biome at ({x}, {z})"
            );
        }
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
