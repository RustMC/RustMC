//! Original coordinate-derived preview terrain; no vanilla generation parity.

pub mod container;

#[cfg(test)]
mod tests_container;

pub mod physics;
pub mod time_weather;

pub mod block_entity;
pub mod interaction;

pub use block_entity::*;

pub mod entity_tracker;

pub use entity_tracker::{
    EntityId, EntityType, SpatialEntityTracker, TrackedEntity, TrackingError, TrackingEvent,
    try_chunk_coords,
};

#[cfg(test)]
mod tests_entity_tracker;

pub mod journal;
pub use journal::{MAX_MUTATIONS_PER_TICK, WorldJournal};
#[cfg(test)]
mod tests_interaction;
#[cfg(test)]
mod tests_inventory;
#[cfg(test)]
mod tests_persistence;
#[cfg(test)]
mod tests_player_interaction;
#[cfg(test)]
mod tests_time_weather;

pub use interaction::*;

pub use physics::{
    Aabb, PhysicsBody, SimpleVoxelWorld, StepResult, WorldCollisionView, compute_fall_damage,
    compute_fall_damage_f32,
};

pub mod events;
pub use events::{
    AcknowledgeBlockChangePacket, BlockBreakEvent, BlockBreakOutcome, BlockPlaceEvent,
    BlockPlaceOutcome, BlockPlaceRequest, BlockUpdatePacket, BreakRollback, Cancellable,
    ContainerSetSlotPacket, DispatchError, Event, EventBus, EventPriority, InteractAction,
    InteractionHand, PlaceRollback, PlayerInteractEvent, UnresolvedItemId, apply_block_break,
    apply_block_place, rollback_break, rollback_break_explicit, rollback_break_with_ack,
    rollback_place, rollback_place_explicit, rollback_place_with_ack,
};
pub mod enchantment;

pub mod inventory;
pub use inventory::{
    HOTBAR_SLOT_COUNT, HOTBAR_SLOTS, InventoryError, MAIN_SLOT_COUNT, MAIN_SLOTS, PlayerInventory,
    TOTAL_SLOT_COUNT, TOTAL_SLOTS, WINDOW_HOTBAR_SLOTS, WINDOW_MAIN_SLOTS,
};

pub const CHUNK_SIDE: usize = 16;
pub const WORLD_HEIGHT: usize = 128;
const TREE_CELL: i64 = 8;
const TREE_REACH: i64 = 3;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Block {
    #[default]
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
    ShortGrass,
    SnowLayer,
    Carpet,
    TallGrassLower,
    TallGrassUpper,
    Obsidian,
}

impl Block {
    /// Java Edition blast resistance of this block, as listed in the
    /// blast-resistance table at <https://minecraft.wiki/w/Explosion>.
    pub const fn blast_resistance(self) -> f32 {
        match self {
            Self::Air => 0.0,
            Self::Dirt => 0.5,
            Self::Stone => 6.0,
            Self::Obsidian => 1200.0,
            Self::Bedrock => 3_600_000.0,
            Self::Grass => 0.6,
            Self::Log | Self::SpruceLog | Self::AcaciaLog | Self::JungleLog => 2.0,
            Self::Leaves | Self::SpruceLeaves | Self::AcaciaLeaves | Self::JungleLeaves => 0.2,
            Self::Sand | Self::RedSand => 0.5,
            Self::Sandstone | Self::RedSandstone => 0.8,
            Self::Snow => 0.2,
            Self::Terracotta => 4.2,
            Self::ShortGrass | Self::TallGrassLower | Self::TallGrassUpper => 0.0,
            Self::SnowLayer | Self::Carpet => 0.1,
        }
    }

    /// Whether this block represents air.
    pub const fn is_air(self) -> bool {
        matches!(self, Self::Air)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportRequirement {
    None,
    Soil,
    TopSupport,
    DoublePlantLower,
    DoublePlantUpper,
}

impl SupportRequirement {
    pub const fn is_self_supporting(self) -> bool {
        matches!(self, Self::None)
    }
}

impl Block {
    pub const fn provides_top_support(self) -> bool {
        !matches!(
            self,
            Self::Air
                | Self::ShortGrass
                | Self::SnowLayer
                | Self::Carpet
                | Self::TallGrassLower
                | Self::TallGrassUpper
        )
    }

    pub const fn is_soil(self) -> bool {
        matches!(self, Self::Dirt | Self::Grass)
    }

    pub const fn support_requirement(self) -> SupportRequirement {
        match self {
            Self::ShortGrass => SupportRequirement::Soil,
            Self::SnowLayer | Self::Carpet => SupportRequirement::TopSupport,
            Self::TallGrassLower => SupportRequirement::DoublePlantLower,
            Self::TallGrassUpper => SupportRequirement::DoublePlantUpper,
            _ => SupportRequirement::None,
        }
    }

    pub const fn can_survive_with(self, below: Option<Block>, above: Option<Block>) -> bool {
        match self.support_requirement() {
            SupportRequirement::None => true,
            SupportRequirement::Soil => match below {
                Some(b) => b.is_soil(),
                None => false,
            },
            SupportRequirement::TopSupport => match below {
                Some(b) => b.provides_top_support(),
                None => false,
            },
            SupportRequirement::DoublePlantLower => match (below, above) {
                (Some(b), Some(Self::TallGrassUpper)) => b.is_soil(),
                _ => false,
            },
            SupportRequirement::DoublePlantUpper => matches!(below, Some(Self::TallGrassLower)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellChange {
    pub x: usize,
    pub y: usize,
    pub z: usize,
    pub block: Block,
}

impl CellChange {
    pub const fn new(x: usize, y: usize, z: usize, block: Block) -> Self {
        Self { x, y, z, block }
    }
}

impl Block {
    /// Whether this block represents a solid obstacle for collision sweeps.
    #[inline]
    pub const fn is_solid(self) -> bool {
        !matches!(self, Block::Air)
    }
}

impl Block {
    /// Every block, in discriminant order (checked by a unit test).
    pub const ALL: [Self; 25] = [
        Self::Air,
        Self::Bedrock,
        Self::Stone,
        Self::Dirt,
        Self::Grass,
        Self::Log,
        Self::Leaves,
        Self::Sand,
        Self::Sandstone,
        Self::Snow,
        Self::SpruceLog,
        Self::SpruceLeaves,
        Self::AcaciaLog,
        Self::AcaciaLeaves,
        Self::JungleLog,
        Self::JungleLeaves,
        Self::RedSand,
        Self::RedSandstone,
        Self::Terracotta,
        Self::ShortGrass,
        Self::SnowLayer,
        Self::Carpet,
        Self::TallGrassLower,
        Self::TallGrassUpper,
        Self::Obsidian,
    ];

    /// Decode a journalled block discriminant; unknown values are corrupt input.
    pub const fn from_u8(value: u8) -> Option<Self> {
        if (value as usize) < Self::ALL.len() {
            Some(Self::ALL[value as usize])
        } else {
            None
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
    pub fn empty(x: i32, z: i32) -> Self {
        Self {
            x,
            z,
            blocks: vec![Block::Air; CHUNK_SIDE * CHUNK_SIDE * WORLD_HEIGHT],
        }
    }

    pub fn set_block(&mut self, x: usize, y: usize, z: usize, block: Block) {
        if x < CHUNK_SIDE && y < WORLD_HEIGHT && z < CHUNK_SIDE {
            self.blocks[(y * CHUNK_SIDE + z) * CHUNK_SIDE + x] = block;
        }
    }

    pub fn block(&self, x: usize, y: usize, z: usize) -> Option<Block> {
        if x >= CHUNK_SIDE || y >= WORLD_HEIGHT || z >= CHUNK_SIDE {
            return None;
        }
        Some(self.blocks[(y * CHUNK_SIDE + z) * CHUNK_SIDE + x])
    }

    pub fn set_and_update(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        block: Block,
    ) -> Option<Vec<CellChange>> {
        if x >= CHUNK_SIDE || y >= WORLD_HEIGHT || z >= CHUNK_SIDE {
            return None;
        }

        // Gate placement support if placing dependent blocks.
        let below = if y > 0 { self.block(x, y - 1, z) } else { None };
        let above = if y + 1 < WORLD_HEIGHT {
            self.block(x, y + 1, z)
        } else {
            None
        };

        match block.support_requirement() {
            SupportRequirement::None => {}
            SupportRequirement::Soil => {
                if !below.is_some_and(|b| b.is_soil()) {
                    return None;
                }
            }
            SupportRequirement::TopSupport => {
                if !below.is_some_and(|b| b.provides_top_support()) {
                    return None;
                }
            }
            SupportRequirement::DoublePlantLower => {
                if !below.is_some_and(|b| b.is_soil())
                    || y + 1 >= WORLD_HEIGHT
                    || !matches!(above, Some(Block::Air) | Some(Block::TallGrassUpper))
                {
                    return None;
                }
            }
            SupportRequirement::DoublePlantUpper => {
                if below != Some(Block::TallGrassLower) {
                    return None;
                }
            }
        }

        let mut changes = Vec::new();

        // Primary write at (x, y, z).
        let index = (y * CHUNK_SIDE + z) * CHUNK_SIDE + x;
        self.blocks[index] = block;
        changes.push(CellChange { x, y, z, block });

        // Atomic double plant placement: placing TallGrassLower atomically places TallGrassUpper at y + 1.
        if block == Block::TallGrassLower && y + 1 < WORLD_HEIGHT {
            let upper_index = ((y + 1) * CHUNK_SIDE + z) * CHUNK_SIDE + x;
            if self.blocks[upper_index] != Block::TallGrassUpper {
                self.blocks[upper_index] = Block::TallGrassUpper;
                changes.push(CellChange {
                    x,
                    y: y + 1,
                    z,
                    block: Block::TallGrassUpper,
                });
            }
        }

        // Step A: downward coupled-half check at y - 1 (breaks lower half if upper half broken).
        if y > 0 {
            let prev_y = y - 1;
            let prev_index = (prev_y * CHUNK_SIDE + z) * CHUNK_SIDE + x;
            if self.blocks[prev_index] == Block::TallGrassLower && block != Block::TallGrassUpper {
                self.blocks[prev_index] = Block::Air;
                changes.push(CellChange {
                    x,
                    y: prev_y,
                    z,
                    block: Block::Air,
                });
            }
        }

        // Step B: bounded upward pass (cy = y + 1 .. WORLD_HEIGHT), breaking unsupported dependent blocks until Air or a surviving block is hit.
        for cy in (y + 1)..WORLD_HEIGHT {
            let curr_index = (cy * CHUNK_SIDE + z) * CHUNK_SIDE + x;
            let current = self.blocks[curr_index];
            if current == Block::Air {
                break;
            }
            let curr_below = Some(self.blocks[((cy - 1) * CHUNK_SIDE + z) * CHUNK_SIDE + x]);
            let curr_above = if cy + 1 < WORLD_HEIGHT {
                Some(self.blocks[((cy + 1) * CHUNK_SIDE + z) * CHUNK_SIDE + x])
            } else {
                None
            };
            if current.can_survive_with(curr_below, curr_above) {
                break;
            } else {
                self.blocks[curr_index] = Block::Air;
                changes.push(CellChange {
                    x,
                    y: cy,
                    z,
                    block: Block::Air,
                });
            }
        }

        Some(changes)
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

    /// Set a block by world coordinates; false when they fall outside this chunk.
    pub fn set_world_block(&mut self, x: i64, y: i64, z: i64, block: Block) -> bool {
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

mod state;
pub use state::*;

#[cfg(test)]
mod tests;
