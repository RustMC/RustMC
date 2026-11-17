//! Projectile trajectory physics and swept raycast impact detection.
//!
//! Provides kinematics simulation for Minecraft projectiles (Arrows, Snowballs, Ender Pearls),
//! including air/water drag, gravity, continuous swept raycasts across voxel blocks and entity
//! bounding boxes, impact resolution, and damage scaling.
//!
//! # Sources
//!
//! Gravity and air drag follow the "Motion of entities" table on the community
//! [Minecraft Wiki Entity page](https://minecraft.wiki/w/Entity), retrieved 13 November
//! 2026, a secondary source not pinned to Java 26.3: arrows 0.05 and 0.99 (terminal
//! 5.00 blocks/tick); eggs, snowballs, and ender pearls 0.03 and 0.99 (terminal 2.97).
//! Arrows apply drag then gravity after moving; snowballs and ender pearls follow the
//! table's Java 1.21.2 "Acceleration, Drag, Position" ordering (gravity, then drag, then
//! moving). Water drag, the damage and critical formulas, knockback strengths, shake
//! ticks, and the five-tick shooter grace period have no recorded source.

pub use super::physics::Aabb;
use std::collections::{BTreeSet, HashSet};

/// Longest path, in blocks, one tick may move a projectile; longer velocities are
/// scaled down to it. Real projectiles move a few blocks per tick, so this only
/// bounds corrupt or hostile input.
pub const MAX_TICK_DISTANCE: f64 = 64.0;

/// Cells one raycast may visit: a segment of length L crosses at most ceil(L)
/// cell boundaries per axis, plus its start cell.
const MAX_RAYCAST_STEPS: usize = 3 * (MAX_TICK_DISTANCE as usize + 1) + 1;

/// Air drag multiplier applied to velocity each tick (`v *= 0.99`).
pub const AIR_DRAG: f64 = 0.99;

/// Water drag multiplier applied to velocity each tick (`v *= 0.60`).
pub const WATER_DRAG: f64 = 0.60;

/// Vertical gravity acceleration for arrows in blocks/tick.
pub const ARROW_GRAVITY: f64 = 0.05;

/// Vertical gravity acceleration for snowballs in blocks/tick.
pub const SNOWBALL_GRAVITY: f64 = 0.03;

/// Vertical gravity acceleration for ender pearls in blocks/tick.
pub const ENDER_PEARL_GRAVITY: f64 = 0.03;

/// Standard arrow ground shake duration upon impact (ticks).
pub const DEFAULT_GROUND_SHAKE_TICKS: u8 = 7;

/// Vanilla base arrow damage multiplier (damage = speed * multiplier).
pub const ARROW_BASE_DAMAGE_MULTIPLIER: f64 = 2.0;

/// Base impulse strength for arrow knockback along movement direction.
pub const ARROW_BASE_KNOCKBACK: f64 = 0.6;

/// Base impulse strength for snowball knockback along movement direction.
pub const SNOWBALL_BASE_KNOCKBACK: f64 = 0.4;

/// Supported projectile classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectileKind {
    Arrow,
    Snowball,
    EnderPearl,
}

impl ProjectileKind {
    /// Downward gravity acceleration in blocks per tick squared.
    pub const fn gravity(self) -> f64 {
        match self {
            Self::Arrow => ARROW_GRAVITY,
            Self::Snowball => SNOWBALL_GRAVITY,
            Self::EnderPearl => ENDER_PEARL_GRAVITY,
        }
    }

    /// Default base damage multiplier.
    pub const fn default_damage_multiplier(self) -> f64 {
        match self {
            Self::Arrow => ARROW_BASE_DAMAGE_MULTIPLIER,
            Self::Snowball | Self::EnderPearl => 0.0,
        }
    }

    /// Default knockback impulse strength along normalized velocity.
    pub const fn default_knockback_strength(self) -> f64 {
        match self {
            Self::Arrow => ARROW_BASE_KNOCKBACK,
            Self::Snowball => SNOWBALL_BASE_KNOCKBACK,
            Self::EnderPearl => 0.0,
        }
    }

    /// Default ground stick shake duration in ticks.
    pub const fn default_shake_ticks(self) -> u8 {
        match self {
            Self::Arrow => DEFAULT_GROUND_SHAKE_TICKS,
            Self::Snowball | Self::EnderPearl => 0,
        }
    }

    /// Whether gravity and drag apply before moving (changed for thrown items in Java 1.21.2).
    pub const fn accelerates_before_moving(self) -> bool {
        match self {
            Self::Arrow => false,
            Self::Snowball | Self::EnderPearl => true,
        }
    }
}

/// Six cardinal directions of an axis-aligned voxel block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockFace {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl BlockFace {
    /// Unit normal vector pointing outwards from the face.
    pub const fn normal(self) -> [f64; 3] {
        match self {
            Self::Down => [0.0, -1.0, 0.0],
            Self::Up => [0.0, 1.0, 0.0],
            Self::North => [0.0, 0.0, -1.0],
            Self::South => [0.0, 0.0, 1.0],
            Self::West => [-1.0, 0.0, 0.0],
            Self::East => [1.0, 0.0, 0.0],
        }
    }
}

/// Result of a projectile collision during swept trajectory update.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectileHit {
    /// Collision with a solid voxel world block.
    BlockHit {
        block_pos: [i32; 3],
        face: BlockFace,
    },
    /// Collision with an entity bounding box.
    EntityHit {
        entity_id: i32,
        damage: f64,
        knockback: [f64; 3],
    },
}

fn is_finite_vec3(v: [f64; 3]) -> bool {
    v[0].is_finite() && v[1].is_finite() && v[2].is_finite()
}

fn is_in_water<W: WorldCollision>(world: &W, pos: [f64; 3]) -> bool {
    world.is_water_block([
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    ])
}

/// Computes intersection of the ray segment `origin + t * dir` for `t in [0.0, 1.0]`.
/// Returns `Some((t, face))` with the entry parameter and face if an intersection occurs.
fn clip_ray(aabb: &Aabb, origin: [f64; 3], dir: [f64; 3]) -> Option<(f64, BlockFace)> {
    let mut t_near = f64::NEG_INFINITY;
    let mut t_far = f64::INFINITY;
    let mut hit_face = None;

    for axis in 0..3 {
        let o = origin[axis];
        let d = dir[axis];
        let b_min = aabb.min[axis];
        let b_max = aabb.max[axis];

        if d.abs() < 1e-12 {
            // Ray is parallel to slab
            if o < b_min || o > b_max {
                return None;
            }
        } else {
            let mut t1 = (b_min - o) / d;
            let mut t2 = (b_max - o) / d;
            let (entry_face, _exit_face) = match axis {
                0 => {
                    if d > 0.0 {
                        (BlockFace::West, BlockFace::East)
                    } else {
                        (BlockFace::East, BlockFace::West)
                    }
                }
                1 => {
                    if d > 0.0 {
                        (BlockFace::Down, BlockFace::Up)
                    } else {
                        (BlockFace::Up, BlockFace::Down)
                    }
                }
                _ => {
                    if d > 0.0 {
                        (BlockFace::North, BlockFace::South)
                    } else {
                        (BlockFace::South, BlockFace::North)
                    }
                }
            };

            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }

            if t1 > t_near {
                t_near = t1;
                hit_face = Some(entry_face);
            }
            if t2 < t_far {
                t_far = t2;
            }

            if t_near > t_far {
                return None;
            }
        }
    }

    if t_far < 0.0 || t_near > 1.0 {
        return None;
    }

    if t_near < 0.0 {
        // Origin is inside the bounding box
        Some((0.0, hit_face.unwrap_or(BlockFace::Up)))
    } else {
        Some((t_near, hit_face.unwrap_or(BlockFace::Up)))
    }
}

/// Targetable entity with an ID and bounding box for raycast intersection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityCollider {
    pub id: i32,
    pub bounding_box: Aabb,
}

impl EntityCollider {
    /// Creates a collider with the specified ID and bounding box.
    pub fn new(id: i32, bounding_box: Aabb) -> Self {
        Self { id, bounding_box }
    }

    /// Creates a standard player collider (0.6 width, 1.8 height) with feet at `feet`.
    pub fn player(id: i32, feet: [f64; 3]) -> Self {
        Self {
            id,
            bounding_box: Aabb::from_entity_pos(feet, 0.6, 1.8),
        }
    }
}

/// World query seam for solid voxel obstacles and fluids.
pub trait WorldCollision {
    /// Returns true if the block at `pos` has solid collision.
    fn is_solid_block(&self, pos: [i32; 3]) -> bool;

    /// Returns true if the block at `pos` is water or fluid inducing water drag.
    fn is_water_block(&self, _pos: [i32; 3]) -> bool {
        false
    }
}

impl<F> WorldCollision for F
where
    F: Fn([i32; 3]) -> bool,
{
    fn is_solid_block(&self, pos: [i32; 3]) -> bool {
        self(pos)
    }
}

impl WorldCollision for BTreeSet<[i32; 3]> {
    fn is_solid_block(&self, pos: [i32; 3]) -> bool {
        self.contains(&pos)
    }
}

impl WorldCollision for HashSet<[i32; 3]> {
    fn is_solid_block(&self, pos: [i32; 3]) -> bool {
        self.contains(&pos)
    }
}

/// In-memory testing world with sets of solid and water blocks.
#[derive(Debug, Default, Clone)]
pub struct SimpleWorld {
    pub solid_blocks: BTreeSet<[i32; 3]>,
    pub water_blocks: BTreeSet<[i32; 3]>,
}

impl SimpleWorld {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_solid(&mut self, pos: [i32; 3]) {
        self.solid_blocks.insert(pos);
    }

    pub fn add_water(&mut self, pos: [i32; 3]) {
        self.water_blocks.insert(pos);
    }
}

impl WorldCollision for SimpleWorld {
    fn is_solid_block(&self, pos: [i32; 3]) -> bool {
        self.solid_blocks.contains(&pos)
    }

    fn is_water_block(&self, pos: [i32; 3]) -> bool {
        self.water_blocks.contains(&pos)
    }
}

impl WorldCollision for super::Chunk {
    fn is_solid_block(&self, pos: [i32; 3]) -> bool {
        if pos[1] < 0 || pos[1] >= super::WORLD_HEIGHT as i32 {
            return false;
        }
        let chunk_min_x = self.x * super::CHUNK_SIDE as i32;
        let chunk_min_z = self.z * super::CHUNK_SIDE as i32;
        let local_x = pos[0] - chunk_min_x;
        let local_z = pos[2] - chunk_min_z;
        if local_x < 0
            || local_x >= super::CHUNK_SIDE as i32
            || local_z < 0
            || local_z >= super::CHUNK_SIDE as i32
        {
            return false;
        }
        matches!(
            self.block(local_x as usize, pos[1] as usize, local_z as usize),
            Some(b) if b != super::Block::Air
        )
    }
}

/// Projectile entity state and kinematics simulation.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectileEntity {
    pub kind: ProjectileKind,
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub shooter_id: Option<i32>,
    pub in_ground: bool,
    pub shake_ticks: u8,
    pub ticks_in_air: u32,
    pub ticks_in_ground: u32,
    pub is_critical: bool,
    pub pierce_level: u8,
    pub damage_multiplier: f64,
    pub knockback_strength: f64,
    pub pierced_entities: Vec<i32>,
}

impl Default for ProjectileEntity {
    fn default() -> Self {
        Self::new(ProjectileKind::Arrow, [0.0; 3], [0.0; 3])
    }
}

impl ProjectileEntity {
    /// Creates a new projectile entity with default kind-specific multipliers.
    pub fn new(kind: ProjectileKind, position: [f64; 3], velocity: [f64; 3]) -> Self {
        Self {
            kind,
            position,
            velocity,
            shooter_id: None,
            in_ground: false,
            shake_ticks: 0,
            ticks_in_air: 0,
            ticks_in_ground: 0,
            is_critical: false,
            pierce_level: 0,
            damage_multiplier: kind.default_damage_multiplier(),
            knockback_strength: kind.default_knockback_strength(),
            pierced_entities: Vec::new(),
        }
    }

    /// Sets the entity ID of the shooter.
    pub fn with_shooter(mut self, shooter_id: i32) -> Self {
        self.shooter_id = Some(shooter_id);
        self
    }

    /// Marks the projectile as a critical shot.
    pub fn with_critical(mut self, is_critical: bool) -> Self {
        self.is_critical = is_critical;
        self
    }

    /// Sets the piercing level (e.g. Crossbow Piercing enchantment).
    pub fn with_pierce_level(mut self, pierce_level: u8) -> Self {
        self.pierce_level = pierce_level;
        self
    }

    /// Sets a custom damage multiplier.
    pub fn with_damage_multiplier(mut self, damage_multiplier: f64) -> Self {
        self.damage_multiplier = damage_multiplier;
        self
    }

    /// Sets custom knockback impulse strength.
    pub fn with_knockback_strength(mut self, knockback_strength: f64) -> Self {
        self.knockback_strength = knockback_strength;
        self
    }

    /// Current scalar speed magnitude `|velocity|`.
    pub fn speed(&self) -> f64 {
        self.speed_sq().sqrt()
    }

    /// Squared scalar speed magnitude `|velocity|^2`.
    pub fn speed_sq(&self) -> f64 {
        self.velocity[0] * self.velocity[0]
            + self.velocity[1] * self.velocity[1]
            + self.velocity[2] * self.velocity[2]
    }

    /// Computes damage on impact: `|velocity| * damage_multiplier` plus critical bonus.
    ///
    /// When `is_critical` is true, follows deterministic vanilla arrow bonus scaling:
    /// `base + 1.0 + floor(base / 2.0)`.
    pub fn compute_damage(&self) -> f64 {
        let speed = self.speed();
        let base = speed * self.damage_multiplier;
        if self.is_critical {
            let bonus = (base / 2.0).max(0.0).floor();
            base + 1.0 + bonus
        } else {
            base
        }
    }

    /// Computes damage with a caller-supplied critical bonus.
    pub fn compute_damage_with_bonus(&self, bonus: f64) -> f64 {
        let speed = self.speed();
        let base = speed * self.damage_multiplier;
        if self.is_critical {
            base + 1.0 + bonus
        } else {
            base
        }
    }

    /// Computes knockback impulse vector along normalized velocity.
    pub fn compute_knockback(&self) -> [f64; 3] {
        let speed = self.speed();
        if speed < 1e-9 {
            return [0.0, 0.0, 0.0];
        }
        let norm = [
            self.velocity[0] / speed,
            self.velocity[1] / speed,
            self.velocity[2] / speed,
        ];
        [
            norm[0] * self.knockback_strength,
            norm[1] * self.knockback_strength,
            norm[2] * self.knockback_strength,
        ]
    }

    fn apply_drag(&mut self, in_water: bool) {
        let drag = if in_water { WATER_DRAG } else { AIR_DRAG };
        self.velocity[0] *= drag;
        self.velocity[1] *= drag;
        self.velocity[2] *= drag;
    }

    fn apply_drag_then_gravity(&mut self, in_water: bool) {
        self.apply_drag(in_water);
        self.velocity[1] -= self.kind.gravity();
    }

    fn apply_gravity_then_drag(&mut self, in_water: bool) {
        self.velocity[1] -= self.kind.gravity();
        self.apply_drag(in_water);
    }

    fn clamp_velocity(&mut self) {
        let max_abs = self.velocity[0]
            .abs()
            .max(self.velocity[1].abs())
            .max(self.velocity[2].abs());
        if max_abs == 0.0 {
            return;
        }
        // Divide by the largest component first so squaring cannot overflow on huge inputs.
        let scaled = [
            self.velocity[0] / max_abs,
            self.velocity[1] / max_abs,
            self.velocity[2] / max_abs,
        ];
        let unit_norm =
            (scaled[0] * scaled[0] + scaled[1] * scaled[1] + scaled[2] * scaled[2]).sqrt();
        if max_abs > MAX_TICK_DISTANCE / unit_norm {
            let scale = MAX_TICK_DISTANCE / unit_norm;
            self.velocity[0] = scaled[0] * scale;
            self.velocity[1] = scaled[1] * scale;
            self.velocity[2] = scaled[2] * scale;
        }
    }

    /// Integrates drag and gravity over one logical tick without position movement.
    pub fn apply_drag_and_gravity(&mut self, in_water: bool) {
        if self.kind.accelerates_before_moving() {
            self.apply_gravity_then_drag(in_water);
        } else {
            self.apply_drag_then_gravity(in_water);
        }
    }

    /// Advances kinematics in free air without collision detection.
    pub fn tick_air(&mut self) -> Option<ProjectileHit> {
        struct EmptyWorld;
        impl WorldCollision for EmptyWorld {
            fn is_solid_block(&self, _pos: [i32; 3]) -> bool {
                false
            }
        }
        self.tick(&EmptyWorld, &[])
    }

    /// Advances projectile simulation over one server tick.
    ///
    /// Performs continuous swept raycasting between `position` and `position + velocity`,
    /// testing voxel blocks and entity bounding boxes. Resolves impacts, ground sticks,
    /// and kinematic drag/gravity.
    pub fn tick<W: WorldCollision>(
        &mut self,
        world: &W,
        entities: &[EntityCollider],
    ) -> Option<ProjectileHit> {
        if !is_finite_vec3(self.position) || !is_finite_vec3(self.velocity) {
            return None;
        }

        if self.in_ground {
            self.ticks_in_ground = self.ticks_in_ground.saturating_add(1);
            if self.shake_ticks > 0 {
                self.shake_ticks -= 1;
            }
            return None;
        }

        self.ticks_in_air = self.ticks_in_air.saturating_add(1);
        if self.shake_ticks > 0 {
            self.shake_ticks -= 1;
        }

        self.clamp_velocity();
        if self.kind.accelerates_before_moving() {
            let in_water = is_in_water(world, self.position);
            self.apply_gravity_then_drag(in_water);
        }

        let start = self.position;
        let vel = self.velocity;

        // Swept raycast against blocks
        let block_hit = self.raycast_blocks(world, start, vel);

        // Swept raycast against entities
        let mut best_entity_hit: Option<(f64, i32)> = None;
        for entity in entities {
            // Prevent hitting shooter during initial flight ticks
            if Some(entity.id) == self.shooter_id && self.ticks_in_air < 5 {
                continue;
            }
            // Ignore already pierced entities
            if self.pierced_entities.contains(&entity.id) {
                continue;
            }

            if let Some((t, _face)) = clip_ray(&entity.bounding_box, start, vel)
                && (0.0..=1.0).contains(&t)
            {
                match best_entity_hit {
                    None => best_entity_hit = Some((t, entity.id)),
                    Some((best_t, _)) if t < best_t => {
                        best_entity_hit = Some((t, entity.id));
                    }
                    _ => {}
                }
            }
        }

        match (block_hit, best_entity_hit) {
            (Some((t_b, block_pos, face)), Some((t_e, entity_id))) => {
                if t_e < t_b {
                    self.resolve_entity_hit(t_e, entity_id, vel, world)
                } else {
                    self.resolve_block_hit(t_b, block_pos, face, vel)
                }
            }
            (Some((t_b, block_pos, face)), None) => {
                self.resolve_block_hit(t_b, block_pos, face, vel)
            }
            (None, Some((t_e, entity_id))) => self.resolve_entity_hit(t_e, entity_id, vel, world),
            (None, None) => {
                self.position[0] += vel[0];
                self.position[1] += vel[1];
                self.position[2] += vel[2];

                if !self.kind.accelerates_before_moving() {
                    let in_water = is_in_water(world, self.position);
                    self.apply_drag_then_gravity(in_water);
                }
                None
            }
        }
    }

    fn resolve_block_hit(
        &mut self,
        t: f64,
        block_pos: [i32; 3],
        face: BlockFace,
        vel: [f64; 3],
    ) -> Option<ProjectileHit> {
        self.position[0] += t * vel[0];
        self.position[1] += t * vel[1];
        self.position[2] += t * vel[2];

        self.in_ground = true;
        self.velocity = [0.0, 0.0, 0.0];
        self.shake_ticks = self.kind.default_shake_ticks();
        self.ticks_in_ground = 0;

        Some(ProjectileHit::BlockHit { block_pos, face })
    }

    fn resolve_entity_hit<W: WorldCollision>(
        &mut self,
        t: f64,
        entity_id: i32,
        vel: [f64; 3],
        world: &W,
    ) -> Option<ProjectileHit> {
        self.position[0] += t * vel[0];
        self.position[1] += t * vel[1];
        self.position[2] += t * vel[2];

        let damage = self.compute_damage();
        let knockback = self.compute_knockback();

        if self.pierce_level > 0 {
            self.pierce_level -= 1;
            self.pierced_entities.push(entity_id);

            if !self.kind.accelerates_before_moving() {
                let in_water = is_in_water(world, self.position);
                self.apply_drag_then_gravity(in_water);
            }
        } else {
            self.velocity = [0.0, 0.0, 0.0];
        }

        Some(ProjectileHit::EntityHit {
            entity_id,
            damage,
            knockback,
        })
    }

    /// Fast 3D digital differential analysis (DDA) raycast across voxel blocks.
    pub fn raycast_blocks<W: WorldCollision>(
        &self,
        world: &W,
        start: [f64; 3],
        vel: [f64; 3],
    ) -> Option<(f64, [i32; 3], BlockFace)> {
        if !is_finite_vec3(start) || !is_finite_vec3(vel) {
            return None;
        }

        let end = [start[0] + vel[0], start[1] + vel[1], start[2] + vel[2]];
        let start_cell = [
            start[0].floor() as i32,
            start[1].floor() as i32,
            start[2].floor() as i32,
        ];
        let end_cell = [
            end[0].floor() as i32,
            end[1].floor() as i32,
            end[2].floor() as i32,
        ];

        // Check starting cell immediately
        if world.is_solid_block(start_cell) {
            let box_aabb = Aabb::from_block(
                i64::from(start_cell[0]),
                i64::from(start_cell[1]),
                i64::from(start_cell[2]),
            );
            if let Some((t, face)) = clip_ray(&box_aabb, start, vel)
                && t <= 1.0
            {
                return Some((t, start_cell, face));
            }
        }

        let mut step = [0i32; 3];
        let mut t_max = [0.0f64; 3];
        let mut t_delta = [0.0f64; 3];

        for i in 0..3 {
            let s = start[i];
            let v = vel[i];
            let c = start_cell[i];

            if v > 1e-12 {
                step[i] = 1;
                t_max[i] = (f64::from(c) + 1.0 - s) / v;
                t_delta[i] = 1.0 / v;
            } else if v < -1e-12 {
                step[i] = -1;
                t_max[i] = (f64::from(c) - s) / v;
                t_delta[i] = -1.0 / v;
            } else {
                step[i] = 0;
                t_max[i] = f64::INFINITY;
                t_delta[i] = f64::INFINITY;
            }
        }

        let mut current_cell = start_cell;
        let cell_distance = (i64::from(end_cell[0]) - i64::from(start_cell[0])).abs()
            + (i64::from(end_cell[1]) - i64::from(start_cell[1])).abs()
            + (i64::from(end_cell[2]) - i64::from(start_cell[2])).abs()
            + 4;
        let max_steps = cell_distance.min(MAX_RAYCAST_STEPS as i64) as usize;

        for _ in 0..max_steps {
            if current_cell == end_cell {
                break;
            }

            if t_max[0] < t_max[1] {
                if t_max[0] < t_max[2] {
                    current_cell[0] = current_cell[0].saturating_add(step[0]);
                    t_max[0] += t_delta[0];
                } else {
                    current_cell[2] = current_cell[2].saturating_add(step[2]);
                    t_max[2] += t_delta[2];
                }
            } else if t_max[1] < t_max[2] {
                current_cell[1] = current_cell[1].saturating_add(step[1]);
                t_max[1] += t_delta[1];
            } else {
                current_cell[2] = current_cell[2].saturating_add(step[2]);
                t_max[2] += t_delta[2];
            }

            if world.is_solid_block(current_cell) {
                let box_aabb = Aabb::from_block(
                    i64::from(current_cell[0]),
                    i64::from(current_cell[1]),
                    i64::from(current_cell[2]),
                );
                if let Some((t, face)) = clip_ray(&box_aabb, start, vel)
                    && (0.0..=1.0).contains(&t)
                {
                    return Some((t, current_cell, face));
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projectile_kind_parameters() {
        assert_eq!(ProjectileKind::Arrow.gravity(), 0.05);
        assert_eq!(ProjectileKind::Snowball.gravity(), 0.03);
        assert_eq!(ProjectileKind::EnderPearl.gravity(), 0.03);

        assert_eq!(ProjectileKind::Arrow.default_damage_multiplier(), 2.0);
        assert_eq!(ProjectileKind::Snowball.default_damage_multiplier(), 0.0);
        assert_eq!(ProjectileKind::EnderPearl.default_damage_multiplier(), 0.0);

        assert_eq!(ProjectileKind::Arrow.default_shake_ticks(), 7);
        assert_eq!(ProjectileKind::Snowball.default_shake_ticks(), 0);
        assert_eq!(ProjectileKind::EnderPearl.default_shake_ticks(), 0);
    }

    #[test]
    fn gravity_and_air_drag_integration() {
        let mut arrow =
            ProjectileEntity::new(ProjectileKind::Arrow, [0.0, 100.0, 0.0], [1.0, 0.0, 0.0]);

        // Tick 1
        let hit = arrow.tick_air();
        assert_eq!(hit, None);
        assert_eq!(arrow.position, [1.0, 100.0, 0.0]);
        assert_eq!(arrow.ticks_in_air, 1);

        // Velocity after drag & gravity:
        // vx = 1.0 * 0.99 = 0.99
        // vy = 0.0 * 0.99 - 0.05 = -0.05
        assert!((arrow.velocity[0] - 0.99).abs() < 1e-9);
        assert!((arrow.velocity[1] - (-0.05)).abs() < 1e-9);
        assert_eq!(arrow.velocity[2], 0.0);

        // Tick 2
        let hit2 = arrow.tick_air();
        assert_eq!(hit2, None);
        assert!((arrow.position[0] - 1.99).abs() < 1e-9);
        assert!((arrow.position[1] - 99.95).abs() < 1e-9);

        // vx = 0.99 * 0.99 = 0.9801
        // vy = -0.05 * 0.99 - 0.05 = -0.0995
        assert!((arrow.velocity[0] - 0.9801).abs() < 1e-9);
        assert!((arrow.velocity[1] - (-0.0995)).abs() < 1e-9);
    }

    #[test]
    fn water_drag_integration() {
        let mut arrow =
            ProjectileEntity::new(ProjectileKind::Arrow, [0.0, 10.0, 0.0], [1.0, 0.0, 0.0]);

        let mut world = SimpleWorld::new();
        // Water at destination (1, 10, 0)
        world.add_water([1, 10, 0]);

        let hit = arrow.tick(&world, &[]);
        assert_eq!(hit, None);
        assert_eq!(arrow.position, [1.0, 10.0, 0.0]);

        // Water drag 0.60 applied
        // vx = 1.0 * 0.60 = 0.60
        // vy = 0.0 * 0.60 - 0.05 = -0.05
        assert!((arrow.velocity[0] - 0.60).abs() < 1e-9);
        assert!((arrow.velocity[1] - (-0.05)).abs() < 1e-9);
    }

    #[test]
    fn terminal_velocity_convergence() {
        // Asymptotically, vy = vy * 0.99 - 0.05 converges to vy_t = -0.05 / 0.01 = -5.0
        let mut arrow =
            ProjectileEntity::new(ProjectileKind::Arrow, [0.0, 1000.0, 0.0], [0.0, 0.0, 0.0]);

        for _ in 0..1000 {
            arrow.tick_air();
        }

        assert!((arrow.velocity[1] - (-5.0)).abs() < 1e-3);

        // Snowball terminal velocity converges to -0.03 * 0.99 / 0.01 = -2.97
        let mut snowball = ProjectileEntity::new(
            ProjectileKind::Snowball,
            [0.0, 1000.0, 0.0],
            [0.0, 0.0, 0.0],
        );

        for _ in 0..1000 {
            snowball.tick_air();
        }

        assert!((snowball.velocity[1] - (-2.97)).abs() < 1e-3);
    }

    #[test]
    fn swept_block_collision_and_ground_stick() {
        let mut arrow =
            ProjectileEntity::new(ProjectileKind::Arrow, [0.5, 64.0, 0.5], [2.0, 0.0, 0.0]);

        let mut world = SimpleWorld::new();
        world.add_solid([2, 64, 0]);

        let hit = arrow.tick(&world, &[]);
        assert_eq!(
            hit,
            Some(ProjectileHit::BlockHit {
                block_pos: [2, 64, 0],
                face: BlockFace::West,
            })
        );

        // Arrow hits West face of block at x = 2.0
        assert_eq!(arrow.position, [2.0, 64.0, 0.5]);
        assert!(arrow.in_ground);
        assert_eq!(arrow.velocity, [0.0, 0.0, 0.0]);
        assert_eq!(arrow.shake_ticks, 7);
        assert_eq!(arrow.ticks_in_ground, 0);

        // Next tick: sticks in ground, timer updates
        let hit_ground = arrow.tick(&world, &[]);
        assert_eq!(hit_ground, None);
        assert_eq!(arrow.position, [2.0, 64.0, 0.5]);
        assert_eq!(arrow.ticks_in_ground, 1);
        assert_eq!(arrow.shake_ticks, 6);
    }

    #[test]
    fn swept_high_velocity_no_tunneling() {
        let mut arrow = ProjectileEntity::new(
            ProjectileKind::Arrow,
            [0.0, 64.5, 0.5],
            [10.0, 0.0, 0.0], // Moves 10 blocks in one tick
        );

        let mut world = SimpleWorld::new();
        // Thin 1-block wall at x = 4
        world.add_solid([4, 64, 0]);

        let hit = arrow.tick(&world, &[]);
        assert_eq!(
            hit,
            Some(ProjectileHit::BlockHit {
                block_pos: [4, 64, 0],
                face: BlockFace::West,
            })
        );
        assert_eq!(arrow.position, [4.0, 64.5, 0.5]);
        assert!(arrow.in_ground);
    }

    #[test]
    fn floor_landing_face_up() {
        let mut arrow =
            ProjectileEntity::new(ProjectileKind::Arrow, [0.5, 2.0, 0.5], [0.0, -1.5, 0.0]);

        let mut world = SimpleWorld::new();
        world.add_solid([0, 0, 0]); // Top is at y = 1.0

        let hit = arrow.tick(&world, &[]);
        assert_eq!(
            hit,
            Some(ProjectileHit::BlockHit {
                block_pos: [0, 0, 0],
                face: BlockFace::Up,
            })
        );
        assert_eq!(arrow.position, [0.5, 1.0, 0.5]);
        assert!(arrow.in_ground);
    }

    #[test]
    fn entity_hit_detection_and_damage() {
        let mut arrow =
            ProjectileEntity::new(ProjectileKind::Arrow, [0.0, 64.5, 0.0], [4.0, 0.0, 0.0]);

        // Player standing at feet x=2.0, y=64.0, z=0.0
        // Hitbox: x: [1.7, 2.3], y: [64.0, 65.8], z: [-0.3, 0.3]
        let player = EntityCollider::player(100, [2.0, 64.0, 0.0]);

        let hit = arrow.tick(&SimpleWorld::new(), &[player]);
        match hit {
            Some(ProjectileHit::EntityHit {
                entity_id,
                damage,
                knockback,
            }) => {
                assert_eq!(entity_id, 100);
                // speed = 4.0, damage = 4.0 * 2.0 = 8.0
                assert!((damage - 8.0).abs() < 1e-9);
                // knockback along [1.0, 0.0, 0.0] with default strength 0.6
                assert!((knockback[0] - 0.6).abs() < 1e-9);
                assert_eq!(knockback[1], 0.0);
                assert_eq!(knockback[2], 0.0);
            }
            other => panic!("expected EntityHit, got {other:?}"),
        }

        // Without pierce, velocity is zeroed
        assert_eq!(arrow.velocity, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn critical_damage_bonus_scaling() {
        let non_crit =
            ProjectileEntity::new(ProjectileKind::Arrow, [0.0, 0.0, 0.0], [3.0, 0.0, 0.0]);
        // speed = 3.0, base = 3.0 * 2.0 = 6.0
        assert_eq!(non_crit.compute_damage(), 6.0);

        let crit = ProjectileEntity::new(ProjectileKind::Arrow, [0.0, 0.0, 0.0], [3.0, 0.0, 0.0])
            .with_critical(true);

        // base = 6.0, critical bonus = base + 1.0 + floor(base / 2.0) = 6.0 + 1.0 + 3.0 = 10.0
        assert_eq!(crit.compute_damage(), 10.0);

        // Custom bonus test
        assert_eq!(crit.compute_damage_with_bonus(4.0), 11.0);
    }

    #[test]
    fn snowball_knockback_zero_damage() {
        let mut snowball =
            ProjectileEntity::new(ProjectileKind::Snowball, [0.0, 1.0, 0.0], [2.0, 0.0, 0.0]);

        let target = EntityCollider::new(200, Aabb::new([1.0, 0.0, -0.5], [2.0, 2.0, 0.5]));

        let hit = snowball.tick(&SimpleWorld::new(), &[target]);
        match hit {
            Some(ProjectileHit::EntityHit {
                entity_id,
                damage,
                knockback,
            }) => {
                assert_eq!(entity_id, 200);
                assert_eq!(damage, 0.0);
                let vx = 2.0 * AIR_DRAG;
                let vy = -SNOWBALL_GRAVITY * AIR_DRAG;
                let expected_kx = SNOWBALL_BASE_KNOCKBACK * vx / (vx * vx + vy * vy).sqrt();
                assert!((knockback[0] - expected_kx).abs() < 1e-9);
            }
            other => panic!("expected EntityHit, got {other:?}"),
        }
    }

    #[test]
    fn shooter_immunity_for_five_ticks() {
        let shooter_id = 42;
        let mut arrow =
            ProjectileEntity::new(ProjectileKind::Arrow, [0.0, 65.0, 0.0], [0.0, 0.0, 0.0])
                .with_shooter(shooter_id);

        let shooter = EntityCollider::player(shooter_id, [0.0, 64.0, 0.0]);

        // First 4 ticks: inside shooter's hitbox, but immune because ticks_in_air < 5
        for tick_idx in 1..=4 {
            let hit = arrow.tick(&SimpleWorld::new(), &[shooter]);
            assert_eq!(hit, None, "should be immune at tick {tick_idx}");
        }

        // On tick 5 (ticks_in_air reaches 5): shooter can be struck
        let hit = arrow.tick(&SimpleWorld::new(), &[shooter]);
        assert!(matches!(
            hit,
            Some(ProjectileHit::EntityHit { entity_id: 42, .. })
        ));
    }

    #[test]
    fn wall_obstruction_precedence() {
        let mut arrow =
            ProjectileEntity::new(ProjectileKind::Arrow, [0.0, 64.5, 0.5], [6.0, 0.0, 0.0]);

        let mut world = SimpleWorld::new();
        // Wall at x = 2
        world.add_solid([2, 64, 0]);

        // Entity at x = 4 (behind the wall)
        let enemy = EntityCollider::player(99, [4.0, 64.0, 0.5]);

        let hit = arrow.tick(&world, &[enemy]);
        // Must hit the wall at x = 2, NOT the enemy behind it
        assert_eq!(
            hit,
            Some(ProjectileHit::BlockHit {
                block_pos: [2, 64, 0],
                face: BlockFace::West,
            })
        );
        assert_eq!(arrow.position, [2.0, 64.5, 0.5]);
        assert!(arrow.in_ground);
    }

    #[test]
    fn piercing_crossbow_arrow_passes_through_entities() {
        let mut arrow =
            ProjectileEntity::new(ProjectileKind::Arrow, [0.0, 64.5, 0.0], [3.0, 0.0, 0.0])
                .with_pierce_level(2);

        let e1 = EntityCollider::player(1, [2.0, 64.0, 0.0]);
        let e2 = EntityCollider::player(2, [4.0, 64.0, 0.0]);

        // Tick 1: hits entity 1, pierce decreases to 1, velocity retained
        let hit1 = arrow.tick(&SimpleWorld::new(), &[e1, e2]);
        assert!(matches!(
            hit1,
            Some(ProjectileHit::EntityHit { entity_id: 1, .. })
        ));
        assert_eq!(arrow.pierce_level, 1);
        assert!(arrow.velocity[0] > 0.0);

        // Tick 2: hits entity 2, pierce decreases to 0
        let hit2 = arrow.tick(&SimpleWorld::new(), &[e1, e2]);
        assert!(matches!(
            hit2,
            Some(ProjectileHit::EntityHit { entity_id: 2, .. })
        ));
        assert_eq!(arrow.pierce_level, 0);
    }

    #[test]
    fn thrown_items_accelerate_before_moving() {
        assert!(!ProjectileKind::Arrow.accelerates_before_moving());
        assert!(ProjectileKind::Snowball.accelerates_before_moving());
        assert!(ProjectileKind::EnderPearl.accelerates_before_moving());

        let expected_vy = -0.03 * 0.99;
        let expected_y = 100.0 - 0.03 * 0.99;

        let mut snowball =
            ProjectileEntity::new(ProjectileKind::Snowball, [0.0, 100.0, 0.0], [0.0, 0.0, 0.0]);
        assert_eq!(snowball.tick_air(), None);
        assert!((snowball.velocity[1] - expected_vy).abs() < 1e-12);
        assert!((snowball.position[1] - expected_y).abs() < 1e-12);

        let mut pearl = ProjectileEntity::new(
            ProjectileKind::EnderPearl,
            [0.0, 100.0, 0.0],
            [0.0, 0.0, 0.0],
        );
        assert_eq!(pearl.tick_air(), None);
        assert!((pearl.velocity[1] - expected_vy).abs() < 1e-12);
        assert!((pearl.position[1] - expected_y).abs() < 1e-12);
    }

    #[test]
    fn ender_pearl_terminal_velocity_convergence() {
        let mut pearl = ProjectileEntity::new(
            ProjectileKind::EnderPearl,
            [0.0, 1000.0, 0.0],
            [0.0, 0.0, 0.0],
        );

        for _ in 0..1000 {
            pearl.tick_air();
        }

        assert!((pearl.velocity[1] - (-2.97)).abs() < 1e-3);
    }

    #[test]
    fn non_finite_tick_leaves_state_unchanged() {
        let world = SimpleWorld::new();

        let mut nan_vel = ProjectileEntity::new(
            ProjectileKind::Arrow,
            [0.0, 64.0, 0.0],
            [f64::NAN, 0.0, 0.0],
        );
        assert_eq!(nan_vel.tick(&world, &[]), None);
        assert_eq!(nan_vel.position, [0.0, 64.0, 0.0]);
        assert!(nan_vel.velocity[0].is_nan());
        assert_eq!(nan_vel.velocity[1], 0.0);
        assert_eq!(nan_vel.velocity[2], 0.0);
        assert_eq!(nan_vel.ticks_in_air, 0);

        let mut inf_pos = ProjectileEntity::new(
            ProjectileKind::Snowball,
            [f64::INFINITY, 64.0, 0.0],
            [1.0, 0.0, 0.0],
        );
        assert_eq!(inf_pos.tick(&world, &[]), None);
        assert_eq!(inf_pos.position, [f64::INFINITY, 64.0, 0.0]);
        assert_eq!(inf_pos.velocity, [1.0, 0.0, 0.0]);
        assert_eq!(inf_pos.ticks_in_air, 0);
    }

    #[test]
    fn huge_velocity_scales_without_overflow() {
        let mut arrow =
            ProjectileEntity::new(ProjectileKind::Arrow, [0.5, 64.5, 0.5], [1e200, 0.0, 1e200]);
        let mut world = SimpleWorld::new();
        world.add_solid([10, 64, 10]);

        let hit = arrow.tick(&world, &[]);
        assert!(matches!(
            hit,
            Some(ProjectileHit::BlockHit {
                block_pos: [10, 64, 10],
                ..
            })
        ));
    }

    #[test]
    fn raycast_blocks_bounds_and_non_finite() {
        let arrow = ProjectileEntity::default();
        let world = SimpleWorld::new();

        assert_eq!(
            arrow.raycast_blocks(&world, [0.0, 0.0, 0.0], [1e300, 0.0, 0.0]),
            None
        );
        assert_eq!(
            arrow.raycast_blocks(&world, [1e15, 0.0, 0.0], [1.0, 0.0, 0.0]),
            None
        );
        assert_eq!(
            arrow.raycast_blocks(&world, [1e15, 0.0, 0.0], [1.0, 1.0, 0.0]),
            None
        );
        assert_eq!(
            arrow.raycast_blocks(&world, [0.0, 0.0, 0.0], [f64::NAN, 0.0, 0.0]),
            None
        );
    }

    #[test]
    fn piercing_thrown_item_accelerates_only_before_moving() {
        let mut snowball =
            ProjectileEntity::new(ProjectileKind::Snowball, [0.0, 64.5, 0.0], [4.0, 0.0, 0.0])
                .with_pierce_level(1);
        let target = EntityCollider::player(1, [2.0, 64.0, 0.0]);

        let hit = snowball.tick(&SimpleWorld::new(), &[target]);
        assert!(matches!(
            hit,
            Some(ProjectileHit::EntityHit { entity_id: 1, .. })
        ));
        assert!((snowball.velocity[0] - 4.0 * AIR_DRAG).abs() < 1e-12);
        assert!((snowball.velocity[1] - (-SNOWBALL_GRAVITY * AIR_DRAG)).abs() < 1e-12);
    }
}
