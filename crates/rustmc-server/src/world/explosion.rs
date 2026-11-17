//! Vanilla explosion raycast attenuation, block destruction, and entity knockback.
//!
//! Implements Milestone M5 vanilla explosion mechanics according to Minecraft protocol
//! and data specifications:
//! - 1,352 raycasts originating from the explosion center directed toward all surface
//!   points of a 16x16x16 cube normalized to unit direction vectors.
//! - Stepwise power attenuation along each ray in 0.3-block increments: a non-air
//!   block costs `(blast_resistance + 0.3) * 0.3`, the block is destroyed if power
//!   remains, and every step then decays by a further 0.225.
//! - Aggregating destroyed non-air blocks into a unique coordinate set (`BTreeSet<[i32; 3]>`).
//! - Entity exposure factor calculation via 3D bounding box raycasting to explosion origin.
//! - Exposure-attenuated damage calculation:
//!   `floor((impact² + impact) / 2 * 7 * diameter + 1)` where
//!   `impact = (1 - dist / diameter) * exposure` and `diameter = power * 2`.
//!
//! These rules follow the published description of Java Edition explosions
//! (<https://minecraft.wiki/w/Explosion>); they have not yet been compared with
//! a 26.3 server.
//! - Exposure-attenuated knockback vector calculation.

use super::Block;
use crate::vanilla::random::RandomSource;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Number of surface rays shot from the 16x16x16 cube: 16^3 - 14^3 = 1,352 rays.
pub const RAY_COUNT: usize = 1352;

/// Ray step distance along ray direction (0.3 blocks).
pub const RAY_STEP_DISTANCE: f64 = 0.3;

/// Constant power decay per step along an explosion ray (`0.3 * 0.75` in `f32`).
pub const RAY_POWER_DECAY_PER_STEP: f32 = 0.225_000_01;

/// Extra resistance every non-air block adds before scaling by the step length.
const BLOCK_ATTENUATION_OFFSET: f32 = 0.3;

/// Damage scale applied to `(impact² + impact) / 2 * diameter`.
const DAMAGE_SCALE: f64 = 7.0;

/// Mode of block destruction caused by an explosion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExplosionDestructionMode {
    /// No blocks are destroyed (e.g. `mobGriefing = false` or harmless explosions).
    None,
    /// Blocks are destroyed and dropped with decay (standard vanilla TNT/creeper explosion).
    #[default]
    Destroy,
    /// Blocks are destroyed and all drops preserved (e.g. 100% item drop gamerule).
    Keep,
    /// Blocks are triggered rather than destroyed (e.g. wind charge interactions).
    Trigger,
}

impl ExplosionDestructionMode {
    /// Returns whether this mode destroys solid blocks.
    #[inline]
    pub const fn destroys_blocks(self) -> bool {
        matches!(self, Self::Destroy | Self::Keep)
    }
}

/// An axis-aligned 3D bounding box for entity exposure and collision calculations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisAlignedBB {
    pub min_x: f64,
    pub min_y: f64,
    pub min_z: f64,
    pub max_x: f64,
    pub max_y: f64,
    pub max_z: f64,
}

impl AxisAlignedBB {
    /// Constructs a bounding box from coordinate extrema.
    #[inline]
    pub fn new(min_x: f64, min_y: f64, min_z: f64, max_x: f64, max_y: f64, max_z: f64) -> Self {
        Self {
            min_x: min_x.min(max_x),
            min_y: min_y.min(max_y),
            min_z: min_z.min(max_z),
            max_x: min_x.max(max_x),
            max_y: min_y.max(max_y),
            max_z: min_z.max(max_z),
        }
    }

    /// Constructs an entity bounding box centered at `center` with width and height.
    #[inline]
    pub fn from_center_size(center: [f64; 3], width: f64, height: f64) -> Self {
        let half_w = width * 0.5;
        Self::new(
            center[0] - half_w,
            center[1],
            center[2] - half_w,
            center[0] + half_w,
            center[1] + height,
            center[2] + half_w,
        )
    }

    /// Returns the geometric center of this bounding box.
    #[inline]
    pub fn center(&self) -> [f64; 3] {
        [
            (self.min_x + self.max_x) * 0.5,
            (self.min_y + self.max_y) * 0.5,
            (self.min_z + self.max_z) * 0.5,
        ]
    }

    /// Returns whether all coordinates are finite numbers.
    #[inline]
    pub fn is_finite(&self) -> bool {
        self.min_x.is_finite()
            && self.min_y.is_finite()
            && self.min_z.is_finite()
            && self.max_x.is_finite()
            && self.max_y.is_finite()
            && self.max_z.is_finite()
    }
}

/// The impact (exposure, damage, and knockback) inflicted on an entity by an explosion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityImpact {
    /// Fraction of sample points visible to the explosion origin, in `[0.0, 1.0]`.
    pub exposure: f32,
    /// Damage points inflicted on the entity.
    pub damage: f32,
    /// 3D knockback velocity vector applied to the entity.
    pub knockback: [f64; 3],
}

impl EntityImpact {
    /// Returns integer damage floored to an unsigned value.
    #[inline]
    pub fn damage_u32(&self) -> u32 {
        self.damage.floor().max(0.0) as u32
    }
}

/// Query interface for retrieving block properties during explosion processing.
pub trait ExplosionWorld {
    /// Returns the blast resistance of the block at the given coordinate.
    fn blast_resistance(&self, pos: [i32; 3]) -> f32;

    /// Returns whether the block at the given coordinate is air.
    fn is_air(&self, pos: [i32; 3]) -> bool;

    /// Returns whether the block at the given coordinate obstructs entity line of sight.
    /// Defaults to non-air blocks (`!self.is_air(pos)`).
    fn blocks_line_of_sight(&self, pos: [i32; 3]) -> bool {
        !self.is_air(pos)
    }
}

impl<F> ExplosionWorld for F
where
    F: Fn([i32; 3]) -> Block,
{
    fn blast_resistance(&self, pos: [i32; 3]) -> f32 {
        self(pos).blast_resistance()
    }

    fn is_air(&self, pos: [i32; 3]) -> bool {
        self(pos).is_air()
    }
}

impl ExplosionWorld for HashMap<[i32; 3], Block> {
    fn blast_resistance(&self, pos: [i32; 3]) -> f32 {
        self.get(&pos)
            .copied()
            .unwrap_or(Block::Air)
            .blast_resistance()
    }

    fn is_air(&self, pos: [i32; 3]) -> bool {
        matches!(self.get(&pos), None | Some(Block::Air))
    }
}

impl ExplosionWorld for BTreeMap<[i32; 3], Block> {
    fn blast_resistance(&self, pos: [i32; 3]) -> f32 {
        self.get(&pos)
            .copied()
            .unwrap_or(Block::Air)
            .blast_resistance()
    }

    fn is_air(&self, pos: [i32; 3]) -> bool {
        matches!(self.get(&pos), None | Some(Block::Air))
    }
}

/// In-memory block grid suitable for testing and localized explosion simulation.
#[derive(Debug, Clone, Default)]
pub struct BlockGrid {
    blocks: HashMap<[i32; 3], Block>,
    default_block: Block,
}

impl BlockGrid {
    /// Constructs a new empty grid with default Air blocks.
    pub fn new() -> Self {
        Self {
            blocks: HashMap::new(),
            default_block: Block::Air,
        }
    }

    /// Sets the fallback block returned for coordinates not explicitly stored.
    pub fn with_default_block(mut self, block: Block) -> Self {
        self.default_block = block;
        self
    }

    /// Sets the block at the given position.
    pub fn set_block(&mut self, pos: [i32; 3], block: Block) {
        if block == self.default_block {
            self.blocks.remove(&pos);
        } else {
            self.blocks.insert(pos, block);
        }
    }

    /// Retrieves the block at the given position.
    pub fn get_block(&self, pos: [i32; 3]) -> Block {
        self.blocks.get(&pos).copied().unwrap_or(self.default_block)
    }

    /// Fills an axis-aligned box of blocks from min (inclusive) to max (inclusive).
    pub fn fill_box(&mut self, min: [i32; 3], max: [i32; 3], block: Block) {
        let x_start = min[0].min(max[0]);
        let x_end = min[0].max(max[0]);
        let y_start = min[1].min(max[1]);
        let y_end = min[1].max(max[1]);
        let z_start = min[2].min(max[2]);
        let z_end = min[2].max(max[2]);

        for x in x_start..=x_end {
            for y in y_start..=y_end {
                for z in z_start..=z_end {
                    self.set_block([x, y, z], block);
                }
            }
        }
    }
}

impl ExplosionWorld for BlockGrid {
    fn blast_resistance(&self, pos: [i32; 3]) -> f32 {
        self.get_block(pos).blast_resistance()
    }

    fn is_air(&self, pos: [i32; 3]) -> bool {
        self.get_block(pos).is_air()
    }
}

/// Generates the 1,352 normalized ray direction vectors directed toward all
/// surface points of a 16x16x16 cube centered at origin with coordinates on [-1.0, 1.0].
pub fn generate_ray_directions() -> Vec<[f64; 3]> {
    let mut rays = Vec::with_capacity(RAY_COUNT);
    for x in 0..16 {
        for y in 0..16 {
            for z in 0..16 {
                let is_surface = x == 0 || x == 15 || y == 0 || y == 15 || z == 0 || z == 15;
                if is_surface {
                    let dx = (x as f64) / 15.0 * 2.0 - 1.0;
                    let dy = (y as f64) / 15.0 * 2.0 - 1.0;
                    let dz = (z as f64) / 15.0 * 2.0 - 1.0;
                    let length = (dx * dx + dy * dy + dz * dz).sqrt();
                    rays.push([dx / length, dy / length, dz / length]);
                }
            }
        }
    }
    rays
}

/// Performs 3D voxel traversal from `start` to `end`, checking whether solid blocks
/// obstruct the line of sight. Returns `true` if unobstructed, `false` if blocked.
pub fn raycast_line_of_sight<W: ExplosionWorld>(world: &W, start: [f64; 3], end: [f64; 3]) -> bool {
    let mut x = start[0].floor() as i32;
    let mut y = start[1].floor() as i32;
    let mut z = start[2].floor() as i32;
    if world.blocks_line_of_sight([x, y, z]) {
        return false;
    }

    let dx = end[0] - start[0];
    let dy = end[1] - start[1];
    let dz = end[2] - start[2];

    let step_x = if dx > 1e-9 {
        1
    } else if dx < -1e-9 {
        -1
    } else {
        0
    };
    let step_y = if dy > 1e-9 {
        1
    } else if dy < -1e-9 {
        -1
    } else {
        0
    };
    let step_z = if dz > 1e-9 {
        1
    } else if dz < -1e-9 {
        -1
    } else {
        0
    };

    if step_x == 0 && step_y == 0 && step_z == 0 {
        return true;
    }

    let t_delta_x = if step_x != 0 {
        (1.0 / dx).abs()
    } else {
        f64::INFINITY
    };
    let t_delta_y = if step_y != 0 {
        (1.0 / dy).abs()
    } else {
        f64::INFINITY
    };
    let t_delta_z = if step_z != 0 {
        (1.0 / dz).abs()
    } else {
        f64::INFINITY
    };

    let mut t_max_x = if step_x > 0 {
        ((x as f64 + 1.0) - start[0]) * t_delta_x
    } else if step_x < 0 {
        (start[0] - (x as f64)) * t_delta_x
    } else {
        f64::INFINITY
    };

    let mut t_max_y = if step_y > 0 {
        ((y as f64 + 1.0) - start[1]) * t_delta_y
    } else if step_y < 0 {
        (start[1] - (y as f64)) * t_delta_y
    } else {
        f64::INFINITY
    };

    let mut t_max_z = if step_z > 0 {
        ((z as f64 + 1.0) - start[2]) * t_delta_z
    } else if step_z < 0 {
        (start[2] - (z as f64)) * t_delta_z
    } else {
        f64::INFINITY
    };

    loop {
        let min_t = t_max_x.min(t_max_y).min(t_max_z);
        if min_t >= 1.0 - 1e-9 {
            return true;
        }

        if t_max_x <= t_max_y && t_max_x <= t_max_z {
            x += step_x;
            t_max_x += t_delta_x;
        } else if t_max_y <= t_max_z {
            y += step_y;
            t_max_y += t_delta_y;
        } else {
            z += step_z;
            t_max_z += t_delta_z;
        }

        if world.blocks_line_of_sight([x, y, z]) {
            return false;
        }
    }
}

/// Vanilla explosion configuration and calculation engine.
#[derive(Debug, Clone, PartialEq)]
pub struct Explosion {
    /// World coordinates of the explosion center [x, y, z].
    pub center: [f64; 3],
    /// Blast power (e.g. 4.0 for TNT, 3.0 for Creeper, 1.0 for small fireball).
    pub power: f32,
    /// Whether the explosion ignites fires on destroyed block tops.
    pub create_fire: bool,
    /// Destruction mode specifying how blocks interact with the blast.
    pub destruction_mode: ExplosionDestructionMode,
}

impl Explosion {
    /// Constructs a new explosion with default DestructionMode::Destroy and create_fire = false.
    #[inline]
    pub const fn new(center: [f64; 3], power: f32) -> Self {
        Self {
            center,
            power,
            create_fire: false,
            destruction_mode: ExplosionDestructionMode::Destroy,
        }
    }

    /// Sets whether this explosion ignites fire.
    #[inline]
    pub const fn with_fire(mut self, create_fire: bool) -> Self {
        self.create_fire = create_fire;
        self
    }

    /// Sets the block destruction mode.
    #[inline]
    pub const fn with_destruction_mode(mut self, mode: ExplosionDestructionMode) -> Self {
        self.destruction_mode = mode;
        self
    }

    /// Blast radius in blocks: `power * 2.0`.
    #[inline]
    pub fn radius(&self) -> f64 {
        f64::from(self.power) * 2.0
    }

    /// Derives a deterministic seed from the explosion center and power parameters.
    pub fn deterministic_seed(&self) -> u64 {
        let x = self.center[0].to_bits();
        let y = self.center[1].to_bits();
        let z = self.center[2].to_bits();
        let p = (self.power as f64).to_bits();
        x.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ y.wrapping_mul(0x6A09_E667_F3BC_C909)
            ^ z.wrapping_mul(0xBF58_476D_1CE4_E5B9)
            ^ p.wrapping_mul(0x94D0_49BB_1331_11EB)
    }

    /// Computes the unique set of destroyed block coordinates using a deterministic PRNG stream.
    pub fn compute_block_destruction<W: ExplosionWorld>(&self, world: &W) -> BTreeSet<[i32; 3]> {
        self.compute_block_destruction_seeded(world, self.deterministic_seed())
    }

    /// Computes the unique set of destroyed block coordinates using a specific random seed.
    pub fn compute_block_destruction_seeded<W: ExplosionWorld>(
        &self,
        world: &W,
        seed: u64,
    ) -> BTreeSet<[i32; 3]> {
        let mut rng = RandomSource::from_world_seed(seed);
        self.compute_block_destruction_with_jitter(world, |_ray_idx| rng.next_float() * 0.6)
    }

    /// Computes the unique set of destroyed block coordinates using a custom jitter callback.
    ///
    /// For each of the 1,352 rays, starting from `power * (0.7 + jitter)`:
    /// - a non-air block at the current position costs `(resistance + 0.3) * 0.3`;
    /// - if power remains, that block is destroyed;
    /// - the position advances 0.3 blocks and power decays by a further 0.225.
    pub fn compute_block_destruction_with_jitter<W: ExplosionWorld, J: FnMut(usize) -> f32>(
        &self,
        world: &W,
        mut jitter_fn: J,
    ) -> BTreeSet<[i32; 3]> {
        if !self.destruction_mode.destroys_blocks()
            || !self.power.is_finite()
            || self.power <= 0.0
            || !self.center.iter().all(|v| v.is_finite())
        {
            return BTreeSet::new();
        }

        let mut destroyed = BTreeSet::new();
        for (i, dir) in generate_ray_directions().into_iter().enumerate() {
            let jitter = jitter_fn(i).clamp(0.0, 0.6);
            let mut ray_power = self.power * (0.7 + jitter);
            let mut position = self.center;
            while ray_power > 0.0 {
                let block = position.map(|v| v.floor() as i32);
                if !world.is_air(block) {
                    let resistance = world.blast_resistance(block).max(0.0);
                    ray_power -= (resistance + BLOCK_ATTENUATION_OFFSET) * RAY_STEP_DISTANCE as f32;
                    if ray_power > 0.0 {
                        destroyed.insert(block);
                    }
                }
                for axis in 0..3 {
                    position[axis] += dir[axis] * RAY_STEP_DISTANCE;
                }
                ray_power -= RAY_POWER_DECAY_PER_STEP;
            }
        }
        destroyed
    }

    /// Fraction of sample points on an entity's box with a clear line to the centre.
    ///
    /// The grid spacing comes from the box size: `1 / (2 * extent + 1)` per axis,
    /// stepping from 0 to 1 inclusive, with the X and Z grids centred in the box.
    /// Returns a factor in `[0.0, 1.0]`.
    pub fn compute_exposure<W: ExplosionWorld>(&self, world: &W, aabb: &AxisAlignedBB) -> f32 {
        if !aabb.is_finite() || !self.center.iter().all(|v| v.is_finite()) {
            return 0.0;
        }
        let step_x = 1.0 / ((aabb.max_x - aabb.min_x) * 2.0 + 1.0);
        let step_y = 1.0 / ((aabb.max_y - aabb.min_y) * 2.0 + 1.0);
        let step_z = 1.0 / ((aabb.max_z - aabb.min_z) * 2.0 + 1.0);
        let offset_x = (1.0 - (1.0 / step_x).floor() * step_x) / 2.0;
        let offset_z = (1.0 - (1.0 / step_z).floor() * step_z) / 2.0;
        let lerp = |t: f64, min: f64, max: f64| min + t * (max - min);

        let mut total = 0u32;
        let mut visible = 0u32;
        // Accumulated like the reference loop so the sample count matches it.
        let mut fx = 0.0;
        while fx <= 1.0 {
            let mut fy = 0.0;
            while fy <= 1.0 {
                let mut fz = 0.0;
                while fz <= 1.0 {
                    let point = [
                        lerp(fx, aabb.min_x, aabb.max_x) + offset_x,
                        lerp(fy, aabb.min_y, aabb.max_y),
                        lerp(fz, aabb.min_z, aabb.max_z) + offset_z,
                    ];
                    if raycast_line_of_sight(world, point, self.center) {
                        visible += 1;
                    }
                    total += 1;
                    fz += step_z;
                }
                fy += step_y;
            }
            fx += step_x;
        }
        if total == 0 {
            0.0
        } else {
            visible as f32 / total as f32
        }
    }

    /// Computes damage and knockback applied to an entity position with a known exposure factor.
    ///
    /// Formulas:
    /// - `diameter = power * 2.0`
    /// - `impact = (1.0 - dist / diameter) * exposure`
    /// - `damage = floor((impact² + impact) / 2 * 7 * diameter + 1)` if `exposure > 0`
    ///   and `dist < diameter`, else `0`.
    /// - `knockback = normalized_dir * impact`.
    pub fn compute_impact_at(&self, exposure: f32, entity_pos: [f64; 3]) -> EntityImpact {
        let diameter = self.radius();
        let none = |exposure: f32| EntityImpact {
            exposure,
            damage: 0.0,
            knockback: [0.0, 0.0, 0.0],
        };
        if !exposure.is_finite() || exposure <= 0.0 || !self.power.is_finite() || self.power <= 0.0
        {
            return none(exposure.clamp(0.0, 1.0));
        }
        let delta = [0, 1, 2].map(|axis| entity_pos[axis] - self.center[axis]);
        let dist = delta.iter().map(|d| d * d).sum::<f64>().sqrt();
        if !dist.is_finite() || dist >= diameter {
            return none(exposure);
        }
        let impact = (1.0 - dist / diameter) * f64::from(exposure);
        let damage = ((impact * impact + impact) / 2.0 * DAMAGE_SCALE * diameter + 1.0).floor();
        let knockback = if dist > 1e-9 {
            delta.map(|d| d / dist * impact)
        } else {
            [0.0, 0.0, 0.0]
        };
        EntityImpact {
            exposure,
            damage: damage.max(0.0) as f32,
            knockback,
        }
    }

    /// Computes exposure, damage, and knockback for an entity given its bounding box.
    pub fn compute_entity_impact<W: ExplosionWorld>(
        &self,
        world: &W,
        aabb: &AxisAlignedBB,
    ) -> EntityImpact {
        let exposure = self.compute_exposure(world, aabb);
        self.compute_impact_at(exposure, aabb.center())
    }

    /// Computes exposure from the bounding box and calculates damage/knockback at an explicit position.
    pub fn compute_entity_impact_with_pos<W: ExplosionWorld>(
        &self,
        world: &W,
        aabb: &AxisAlignedBB,
        entity_pos: [f64; 3],
    ) -> EntityImpact {
        let exposure = self.compute_exposure(world, aabb);
        self.compute_impact_at(exposure, entity_pos)
    }

    /// Evaluates fire candidate positions on top of destroyed blocks when `create_fire` is enabled.
    pub fn compute_fire_positions<W: ExplosionWorld, R: FnMut([i32; 3]) -> bool>(
        &self,
        world: &W,
        destroyed_blocks: &BTreeSet<[i32; 3]>,
        mut fire_chance: R,
    ) -> Vec<[i32; 3]> {
        if !self.create_fire {
            return Vec::new();
        }

        let mut fire_positions = Vec::new();
        for &pos in destroyed_blocks {
            let [x, y, z] = pos;
            let below = [x, y - 1, z];
            if world.is_air(pos)
                && !world.is_air(below)
                && world.blast_resistance(below) > 0.0
                && fire_chance(pos)
            {
                fire_positions.push(pos);
            }
        }
        fire_positions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_count_is_exactly_1352_and_normalized() {
        let rays = generate_ray_directions();
        assert_eq!(rays.len(), RAY_COUNT);

        for ray in rays {
            let len_sq = ray[0] * ray[0] + ray[1] * ray[1] + ray[2] * ray[2];
            assert!(
                (len_sq - 1.0).abs() < 1e-9,
                "ray was not normalized: {len_sq}"
            );
        }
    }

    #[test]
    fn blast_resistance_values_match_vanilla_specification() {
        assert_eq!(Block::Air.blast_resistance(), 0.0);
        assert_eq!(Block::Dirt.blast_resistance(), 0.5);
        assert_eq!(Block::Stone.blast_resistance(), 6.0);
        assert_eq!(Block::Obsidian.blast_resistance(), 1200.0);
        assert_eq!(Block::Bedrock.blast_resistance(), 3600000.0);
    }

    #[test]
    fn air_blast_destroys_no_blocks_and_terminates_cleanly() {
        let grid = BlockGrid::new(); // all air
        let explosion = Explosion::new([0.0, 64.0, 0.0], 4.0);

        let destroyed = explosion.compute_block_destruction(&grid);
        assert!(
            destroyed.is_empty(),
            "air blast should destroy 0 blocks, found: {}",
            destroyed.len()
        );
    }

    #[test]
    fn dirt_vs_stone_crater_shows_blast_resistance_difference() {
        let dirt_grid = BlockGrid::new().with_default_block(Block::Dirt);
        let stone_grid = BlockGrid::new().with_default_block(Block::Stone);

        let explosion = Explosion::new([0.5, 64.5, 0.5], 4.0);

        let dirt_destroyed = explosion.compute_block_destruction(&dirt_grid);
        let stone_destroyed = explosion.compute_block_destruction(&stone_grid);

        assert!(
            !dirt_destroyed.is_empty(),
            "dirt explosion must destroy blocks"
        );
        assert!(
            !stone_destroyed.is_empty(),
            "stone explosion must destroy blocks"
        );

        // Dirt blast resistance (0.5) allows significantly greater penetration than stone (6.0)
        assert!(
            dirt_destroyed.len() > stone_destroyed.len() * 2,
            "dirt crater ({}) must be substantially larger than stone crater ({})",
            dirt_destroyed.len(),
            stone_destroyed.len()
        );
    }

    #[test]
    fn obsidian_shielding_destroys_zero_blocks() {
        let mut grid = BlockGrid::new(); // all air
        let center = [0.5, 64.5, 0.5];

        // Surround the center with an obsidian shell
        grid.fill_box([-1, 63, -1], [2, 66, 2], Block::Obsidian);
        // Leave the explosion origin block air
        grid.set_block([0, 64, 0], Block::Air);

        let explosion = Explosion::new(center, 4.0);
        let destroyed = explosion.compute_block_destruction(&grid);

        assert!(
            destroyed.is_empty(),
            "obsidian (resistance 1200) must stop all blast rays on first step without block destruction, found: {:?}",
            destroyed
        );
    }

    #[test]
    fn entity_direct_exposure_vs_full_occlusion() {
        let mut grid = BlockGrid::new(); // all air
        let center = [0.0, 64.0, 0.0];
        let explosion = Explosion::new(center, 4.0);

        // Entity centered at [4.0, 64.0, 0.0]
        let entity_aabb = AxisAlignedBB::new(3.7, 63.1, -0.3, 4.3, 64.9, 0.3);

        // Direct exposure test
        let direct_impact = explosion.compute_entity_impact(&grid, &entity_aabb);
        assert_eq!(direct_impact.exposure, 1.0);
        assert!(
            direct_impact.damage > 0.0,
            "visible entity must receive damage"
        );
        assert!(
            direct_impact.knockback[0] > 0.0,
            "visible entity must receive positive knockback away from center"
        );

        // Expected damage for dist = 4.0, diameter = 8.0, power = 4.0, exposure = 1.0:
        // impact = (1.0 - 4.0 / 8.0) * 1.0 = 0.5
        // damage = floor((0.25 + 0.5) / 2 * 7 * 8 + 1) = floor(22.0) = 22.0
        assert_eq!(direct_impact.damage, 22.0);

        // Place an obsidian wall between the explosion and the entity
        grid.fill_box([2, 60, -5], [2, 70, 5], Block::Obsidian);

        let occluded_impact = explosion.compute_entity_impact(&grid, &entity_aabb);
        assert_eq!(
            occluded_impact.exposure, 0.0,
            "entity behind obsidian wall must have 0.0 exposure"
        );
        assert_eq!(
            occluded_impact.damage, 0.0,
            "entity behind obsidian wall must take 0 damage"
        );
        assert_eq!(
            occluded_impact.knockback,
            [0.0, 0.0, 0.0],
            "entity behind obsidian wall must take 0 knockback"
        );
    }

    #[test]
    fn partial_occlusion_calculates_fractional_exposure() {
        let mut grid = BlockGrid::new();
        let center = [0.0, 64.0, 0.0];
        let explosion = Explosion::new(center, 4.0);

        // Entity centered at [3.0, 64.0, 0.0] spanning z from -0.5 to 0.5
        let entity_aabb = AxisAlignedBB::new(2.7, 63.5, -0.5, 3.3, 64.5, 0.5);

        // Place a wall at x = 1.0 covering only positive z (z from 0 to 5), leaving negative z open
        grid.fill_box([1, 60, 0], [1, 70, 5], Block::Obsidian);

        let impact = explosion.compute_entity_impact(&grid, &entity_aabb);
        assert_eq!(
            impact.exposure, 0.5,
            "half-occluded entity must have exposure 0.5, got {}",
            impact.exposure
        );
        assert!(
            impact.damage > 0.0,
            "partially exposed entity should take damage"
        );
    }

    #[test]
    fn destruction_modes_govern_block_destruction() {
        let grid = BlockGrid::new().with_default_block(Block::Dirt);

        let none_exp = Explosion::new([0.0, 64.0, 0.0], 4.0)
            .with_destruction_mode(ExplosionDestructionMode::None);
        assert!(none_exp.compute_block_destruction(&grid).is_empty());

        let trigger_exp = Explosion::new([0.0, 64.0, 0.0], 4.0)
            .with_destruction_mode(ExplosionDestructionMode::Trigger);
        assert!(trigger_exp.compute_block_destruction(&grid).is_empty());

        let destroy_exp = Explosion::new([0.0, 64.0, 0.0], 4.0)
            .with_destruction_mode(ExplosionDestructionMode::Destroy);
        assert!(!destroy_exp.compute_block_destruction(&grid).is_empty());

        let keep_exp = Explosion::new([0.0, 64.0, 0.0], 4.0)
            .with_destruction_mode(ExplosionDestructionMode::Keep);
        assert!(!keep_exp.compute_block_destruction(&grid).is_empty());
    }

    #[test]
    fn non_finite_and_negative_parameters_are_safely_rejected() {
        let grid = BlockGrid::new().with_default_block(Block::Dirt);

        let nan_exp = Explosion::new([f64::NAN, 64.0, 0.0], 4.0);
        assert!(nan_exp.compute_block_destruction(&grid).is_empty());

        let neg_exp = Explosion::new([0.0, 64.0, 0.0], -4.0);
        assert!(neg_exp.compute_block_destruction(&grid).is_empty());

        let zero_exp = Explosion::new([0.0, 64.0, 0.0], 0.0);
        assert!(zero_exp.compute_block_destruction(&grid).is_empty());

        let aabb = AxisAlignedBB::from_center_size([4.0, 64.0, 0.0], 0.6, 1.8);
        assert_eq!(nan_exp.compute_entity_impact(&grid, &aabb).damage, 0.0);
        assert_eq!(neg_exp.compute_entity_impact(&grid, &aabb).damage, 0.0);
    }

    #[test]
    fn deterministic_seed_produces_repeatable_destruction() {
        let grid = BlockGrid::new().with_default_block(Block::Dirt);
        let exp1 = Explosion::new([12.5, 64.5, -34.5], 4.0);
        let exp2 = Explosion::new([12.5, 64.5, -34.5], 4.0);

        let res1 = exp1.compute_block_destruction(&grid);
        let res2 = exp2.compute_block_destruction(&grid);

        assert_eq!(res1, res2);
    }

    #[test]
    fn a_blast_inside_solid_stone_destroys_only_its_own_block() {
        let grid = BlockGrid::new().with_default_block(Block::Stone);
        let blast = Explosion::new([0.5, 64.5, 0.5], 4.0);
        // Jitter 0.3 gives every ray exactly the nominal power of 4.0.
        let destroyed = blast.compute_block_destruction_with_jitter(&grid, |_| 0.3);
        assert_eq!(destroyed, BTreeSet::from([[0, 64, 0]]));
    }

    #[test]
    fn air_only_decays_by_the_fixed_step_cost() {
        // Dirt 3.5 blocks away: with the fixed 0.225 per air step, rays still
        // carry about 1.3 power when they arrive and dirt costs 0.24.
        let mut grid = BlockGrid::new();
        grid.set_block([4, 64, 0], Block::Dirt);
        let blast = Explosion::new([0.5, 64.5, 0.5], 4.0);
        let destroyed = blast.compute_block_destruction_with_jitter(&grid, |_| 0.3);
        assert!(destroyed.contains(&[4, 64, 0]));
    }

    #[test]
    fn a_ray_with_power_left_after_a_block_destroys_it_before_decay() {
        // Power 0.2 at the origin: leaves (resistance 0.2) cost 0.15 and leave
        // 0.05, less than the 0.225 step decay. The block is still destroyed
        // because the hit is recorded before the decay.
        let mut grid = BlockGrid::new();
        grid.set_block([0, 64, 0], Block::Leaves);
        let blast = Explosion::new([0.5, 64.5, 0.5], 0.2 / 0.7);
        let destroyed = blast.compute_block_destruction_with_jitter(&grid, |_| 0.0);
        assert_eq!(destroyed, BTreeSet::from([[0, 64, 0]]));
    }

    #[test]
    fn bedrock_prevents_destruction_completely() {
        let grid = BlockGrid::new().with_default_block(Block::Bedrock);
        let exp = Explosion::new([0.5, 64.5, 0.5], 10.0);
        let destroyed = exp.compute_block_destruction(&grid);
        assert!(destroyed.is_empty(), "bedrock must never be destroyed");
    }
}
