//! Deterministic entity velocity, gravity, terminal velocity, and AABB sweep collision loop.
//!
//! Milestone M5 implementation providing coordinate delta sweeping along axes,
//! step-height traversal over slabs/stairs, drag, gravity integration, and fall damage.
//!
//! # Sources
//!
//! The motion constants follow the "Motion of entities" table on the community
//! [Minecraft Wiki Entity page](https://minecraft.wiki/w/Entity), retrieved 13 November
//! 2026. That is a secondary source not pinned to Java 26.3; no primary source or
//! client observation has been recorded. For players and other living entities it gives
//! acceleration -0.08 blocks/tick², vertical drag 0.98, horizontal drag 0.91, terminal
//! velocity 3.92 blocks/tick, and the per-tick order "position, acceleration, drag".
//! Block slipperiness (ground friction), liquids, and Slow Falling are not modelled.

use crate::world::{Block, CHUNK_SIDE, Chunk, WORLD_HEIGHT};

/// Living entity gravity acceleration in blocks per tick² (Entity page, see module docs).
pub const GRAVITY: f64 = 0.08;

/// Horizontal drag factor per tick for living entities (Entity page: 0.91).
pub const HORIZONTAL_AIR_DRAG: f64 = 0.91;

/// Vertical drag factor per tick for living entities (Entity page: 0.98).
pub const VERTICAL_AIR_DRAG: f64 = 0.98;

/// Terminal downward velocity in blocks per tick (Entity page: 3.92). It is the fixed
/// point of `v = (v - GRAVITY) * VERTICAL_AIR_DRAG`, so the clamp only guards rounding.
pub const TERMINAL_VELOCITY: f64 = -3.92;

/// Default step height for living entities (0.6 blocks, allowing smooth movement up slabs).
pub const DEFAULT_STEP_HEIGHT: f64 = 0.6;

/// Default player hitbox width in blocks.
pub const DEFAULT_PLAYER_WIDTH: f64 = 0.6;

/// Default player hitbox height in blocks.
pub const DEFAULT_PLAYER_HEIGHT: f64 = 1.8;

/// Numerical tolerance for floating-point collision tests.
const EPSILON: f64 = 1e-7;

/// 3D Axis-Aligned Bounding Box represented by minimum and maximum extents `[x, y, z]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb {
    /// Construct a new `Aabb` from minimum and maximum coordinate arrays.
    #[inline]
    pub const fn new(min: [f64; 3], max: [f64; 3]) -> Self {
        Self { min, max }
    }

    /// Construct a new `Aabb` from separate min and max coordinate scalars.
    #[inline]
    pub const fn from_min_max(
        min_x: f64,
        min_y: f64,
        min_z: f64,
        max_x: f64,
        max_y: f64,
        max_z: f64,
    ) -> Self {
        Self {
            min: [min_x, min_y, min_z],
            max: [max_x, max_y, max_z],
        }
    }

    /// Construct an entity bounding box centered horizontally at `(pos[0], pos[2])` with base at `pos[1]`.
    #[inline]
    pub fn from_entity_pos(pos: [f64; 3], width: f64, height: f64) -> Self {
        let half_width = width / 2.0;
        Self {
            min: [pos[0] - half_width, pos[1], pos[2] - half_width],
            max: [pos[0] + half_width, pos[1] + height, pos[2] + half_width],
        }
    }

    /// Construct a full 1x1x1 cube bounding box for a voxel at integer coordinates `(x, y, z)`.
    #[inline]
    pub fn from_block(x: i64, y: i64, z: i64) -> Self {
        Self {
            min: [x as f64, y as f64, z as f64],
            max: [x as f64 + 1.0, y as f64 + 1.0, z as f64 + 1.0],
        }
    }

    /// Check if this bounding box strictly intersects another bounding box.
    ///
    /// Boxes that merely touch along faces, edges, or vertices without overlapping volume do not intersect.
    #[inline]
    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min[0] < other.max[0]
            && self.max[0] > other.min[0]
            && self.min[1] < other.max[1]
            && self.max[1] > other.min[1]
            && self.min[2] < other.max[2]
            && self.max[2] > other.min[2]
    }

    /// Expand the bounding box in the direction of motion `(dx, dy, dz)`.
    ///
    /// For each axis, if displacement is positive, maximum extent is increased;
    /// if negative, minimum extent is decreased. This creates the swept volume.
    #[inline]
    pub fn expand(&self, dx: f64, dy: f64, dz: f64) -> Self {
        let min_x = if dx < 0.0 {
            self.min[0] + dx
        } else {
            self.min[0]
        };
        let max_x = if dx > 0.0 {
            self.max[0] + dx
        } else {
            self.max[0]
        };
        let min_y = if dy < 0.0 {
            self.min[1] + dy
        } else {
            self.min[1]
        };
        let max_y = if dy > 0.0 {
            self.max[1] + dy
        } else {
            self.max[1]
        };
        let min_z = if dz < 0.0 {
            self.min[2] + dz
        } else {
            self.min[2]
        };
        let max_z = if dz > 0.0 {
            self.max[2] + dz
        } else {
            self.max[2]
        };
        Self {
            min: [min_x, min_y, min_z],
            max: [max_x, max_y, max_z],
        }
    }

    /// Translate this bounding box by a 3D offset `(dx, dy, dz)`.
    #[inline]
    pub fn offset(&self, dx: f64, dy: f64, dz: f64) -> Self {
        Self {
            min: [self.min[0] + dx, self.min[1] + dy, self.min[2] + dz],
            max: [self.max[0] + dx, self.max[1] + dy, self.max[2] + dz],
        }
    }

    /// Uniformly inflate (grow) or deflate (shrink) the bounding box by `(dx, dy, dz)` around its center.
    #[inline]
    pub fn inflate(&self, dx: f64, dy: f64, dz: f64) -> Self {
        Self {
            min: [self.min[0] - dx, self.min[1] - dy, self.min[2] - dz],
            max: [self.max[0] + dx, self.max[1] + dy, self.max[2] + dz],
        }
    }

    /// Check if this bounding box contains a point `[x, y, z]`.
    #[inline]
    pub fn contains_point(&self, point: [f64; 3]) -> bool {
        point[0] >= self.min[0]
            && point[0] <= self.max[0]
            && point[1] >= self.min[1]
            && point[1] <= self.max[1]
            && point[2] >= self.min[2]
            && point[2] <= self.max[2]
    }

    /// Calculate the maximum Y offset `self` can move before colliding with `other`.
    pub fn calculate_y_offset(&self, other: &Aabb, mut dy: f64) -> f64 {
        if other.max[0] > self.min[0] + EPSILON
            && other.min[0] < self.max[0] - EPSILON
            && other.max[2] > self.min[2] + EPSILON
            && other.min[2] < self.max[2] - EPSILON
        {
            if dy > 0.0 && other.min[1] >= self.max[1] - EPSILON {
                let limit = (other.min[1] - self.max[1]).max(0.0);
                if limit < dy {
                    dy = limit;
                }
            } else if dy < 0.0 && other.max[1] <= self.min[1] + EPSILON {
                let limit = (other.max[1] - self.min[1]).min(0.0);
                if limit > dy {
                    dy = limit;
                }
            }
        }
        dy
    }

    /// Calculate the maximum X offset `self` can move before colliding with `other`.
    pub fn calculate_x_offset(&self, other: &Aabb, mut dx: f64) -> f64 {
        if other.max[1] > self.min[1] + EPSILON
            && other.min[1] < self.max[1] - EPSILON
            && other.max[2] > self.min[2] + EPSILON
            && other.min[2] < self.max[2] - EPSILON
        {
            if dx > 0.0 && other.min[0] >= self.max[0] - EPSILON {
                let limit = (other.min[0] - self.max[0]).max(0.0);
                if limit < dx {
                    dx = limit;
                }
            } else if dx < 0.0 && other.max[0] <= self.min[0] + EPSILON {
                let limit = (other.max[0] - self.min[0]).min(0.0);
                if limit > dx {
                    dx = limit;
                }
            }
        }
        dx
    }

    /// Calculate the maximum Z offset `self` can move before colliding with `other`.
    pub fn calculate_z_offset(&self, other: &Aabb, mut dz: f64) -> f64 {
        if other.max[0] > self.min[0] + EPSILON
            && other.min[0] < self.max[0] - EPSILON
            && other.max[1] > self.min[1] + EPSILON
            && other.min[1] < self.max[1] - EPSILON
        {
            if dz > 0.0 && other.min[2] >= self.max[2] - EPSILON {
                let limit = (other.min[2] - self.max[2]).max(0.0);
                if limit < dz {
                    dz = limit;
                }
            } else if dz < 0.0 && other.max[2] <= self.min[2] + EPSILON {
                let limit = (other.max[2] - self.min[2]).min(0.0);
                if limit > dz {
                    dz = limit;
                }
            }
        }
        dz
    }
}

/// Trait for querying collision bounding boxes in a world.
pub trait WorldCollisionView {
    /// Retrieve all solid bounding boxes colliding with the given query box.
    fn get_colliding_boxes(&self, aabb: &Aabb) -> Vec<Aabb>;
}

impl WorldCollisionView for Vec<Aabb> {
    fn get_colliding_boxes(&self, aabb: &Aabb) -> Vec<Aabb> {
        self.iter()
            .copied()
            .filter(|b| b.intersects(aabb))
            .collect()
    }
}

impl WorldCollisionView for [Aabb] {
    fn get_colliding_boxes(&self, aabb: &Aabb) -> Vec<Aabb> {
        self.iter()
            .copied()
            .filter(|b| b.intersects(aabb))
            .collect()
    }
}

impl<T: WorldCollisionView + ?Sized> WorldCollisionView for &T {
    fn get_colliding_boxes(&self, aabb: &Aabb) -> Vec<Aabb> {
        (**self).get_colliding_boxes(aabb)
    }
}

impl WorldCollisionView for () {
    fn get_colliding_boxes(&self, _aabb: &Aabb) -> Vec<Aabb> {
        Vec::new()
    }
}

/// Helper to extract voxel collision boxes for integer coordinates where `is_solid` returns true.
pub fn get_voxel_colliding_boxes<F>(aabb: &Aabb, is_solid: F) -> Vec<Aabb>
where
    F: Fn(i64, i64, i64) -> bool,
{
    let min_x = aabb.min[0].floor() as i64;
    let max_x = aabb.max[0].ceil() as i64;
    let min_y = aabb.min[1].floor() as i64;
    let max_y = aabb.max[1].ceil() as i64;
    let min_z = aabb.min[2].floor() as i64;
    let max_z = aabb.max[2].ceil() as i64;

    let mut colliding = Vec::new();
    for y in min_y..max_y {
        for z in min_z..max_z {
            for x in min_x..max_x {
                if is_solid(x, y, z) {
                    let block_box = Aabb::from_block(x, y, z);
                    if block_box.intersects(aabb) {
                        colliding.push(block_box);
                    }
                }
            }
        }
    }
    colliding
}

impl WorldCollisionView for Chunk {
    fn get_colliding_boxes(&self, aabb: &Aabb) -> Vec<Aabb> {
        let origin_x = i64::from(self.x) * CHUNK_SIDE as i64;
        let origin_z = i64::from(self.z) * CHUNK_SIDE as i64;
        get_voxel_colliding_boxes(aabb, |x, y, z| {
            if y < 0 || y >= WORLD_HEIGHT as i64 {
                return false;
            }
            let lx = x - origin_x;
            let lz = z - origin_z;
            if !(0..CHUNK_SIDE as i64).contains(&lx) || !(0..CHUNK_SIDE as i64).contains(&lz) {
                return false;
            }
            self.block(lx as usize, y as usize, lz as usize)
                .is_some_and(Block::is_solid)
        })
    }
}

/// A simple in-memory voxel world suitable for physics testing and scenarios.
#[derive(Debug, Default, Clone)]
pub struct SimpleVoxelWorld {
    solid_blocks: std::collections::HashSet<(i64, i64, i64)>,
    custom_boxes: Vec<Aabb>,
}

impl SimpleVoxelWorld {
    /// Create a new empty `SimpleVoxelWorld`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a 1x1x1 solid block at voxel coordinates `(x, y, z)`.
    pub fn add_solid_block(&mut self, x: i64, y: i64, z: i64) {
        self.solid_blocks.insert((x, y, z));
    }

    /// Add a custom collision bounding box (e.g., slab or stair shape).
    pub fn add_box(&mut self, aabb: Aabb) {
        self.custom_boxes.push(aabb);
    }
}

impl WorldCollisionView for SimpleVoxelWorld {
    fn get_colliding_boxes(&self, aabb: &Aabb) -> Vec<Aabb> {
        let mut boxes =
            get_voxel_colliding_boxes(aabb, |x, y, z| self.solid_blocks.contains(&(x, y, z)));
        for custom in &self.custom_boxes {
            if custom.intersects(aabb) {
                boxes.push(*custom);
            }
        }
        boxes
    }
}

/// Calculate fall damage for an entity landing after falling `fall_distance` blocks.
///
/// Vanilla rule: `max(0, floor(fall_distance - 3.0))`.
/// A small epsilon `1e-4` is applied to prevent numerical roundoff during discrete per-tick
/// integration (e.g. 14.999999 vs 15.0) from removing a whole damage point.
#[inline]
pub fn compute_fall_damage(fall_distance: f32) -> u32 {
    if !fall_distance.is_finite() || fall_distance <= (3.0 - 1e-4) {
        0
    } else {
        ((fall_distance - 3.0 + 1e-4).floor()).max(0.0) as u32
    }
}

/// Floating-point variation of `compute_fall_damage`.
#[inline]
pub fn compute_fall_damage_f32(fall_distance: f32) -> f32 {
    if !fall_distance.is_finite() || fall_distance <= (3.0 - 1e-4) {
        0.0
    } else {
        ((fall_distance - 3.0 + 1e-4).floor()).max(0.0)
    }
}

/// Apply gravity acceleration, then drag, then the terminal velocity clamp.
///
/// Acceleration precedes drag, as the Entity page orders it for living entities:
/// - Horizontal velocity multiplied by [`HORIZONTAL_AIR_DRAG`].
/// - Vertical velocity reduced by [`GRAVITY`], then multiplied by [`VERTICAL_AIR_DRAG`].
/// - Terminal velocity clamped: `v_y = max(v_y, TERMINAL_VELOCITY)`.
pub fn apply_air_drag_and_gravity(velocity: &mut [f64; 3]) {
    velocity[0] *= HORIZONTAL_AIR_DRAG;
    velocity[2] *= HORIZONTAL_AIR_DRAG;
    velocity[1] = (velocity[1] - GRAVITY) * VERTICAL_AIR_DRAG;
    if velocity[1] < TERMINAL_VELOCITY {
        velocity[1] = TERMINAL_VELOCITY;
    }
}

/// Swept collision resolution along axes with step-height support.
///
/// Sweeps Y, then X, then Z, and tests step-up over obstacles when on ground.
/// Returns `(resolved_delta, on_ground, collided_horizontal, collided_vertical, ceiling_hit, stepped_up)`.
pub fn collide_and_sweep(
    start_box: &Aabb,
    delta: [f64; 3],
    colliding_boxes: &[Aabb],
    step_height: f64,
    on_ground: bool,
) -> ([f64; 3], bool, bool, bool, bool, bool) {
    // 1. Resolve standard axis sweep: Y, then X, then Z.
    let mut resolved_dy = delta[1];
    for b in colliding_boxes {
        resolved_dy = start_box.calculate_y_offset(b, resolved_dy);
    }
    let box_y = start_box.offset(0.0, resolved_dy, 0.0);

    let mut resolved_dx = delta[0];
    for b in colliding_boxes {
        resolved_dx = box_y.calculate_x_offset(b, resolved_dx);
    }
    let box_yx = box_y.offset(resolved_dx, 0.0, 0.0);

    let mut resolved_dz = delta[2];
    for b in colliding_boxes {
        resolved_dz = box_yx.calculate_z_offset(b, resolved_dz);
    }

    let mut stepped_up = false;
    let collided_horiz =
        (resolved_dx - delta[0]).abs() > EPSILON || (resolved_dz - delta[2]).abs() > EPSILON;
    let ground_contact = on_ground || ((resolved_dy - delta[1]).abs() > EPSILON && delta[1] < 0.0);

    // 2. Attempt step-up if horizontal collision occurred while grounded.
    if collided_horiz && ground_contact && step_height > 0.0 {
        // Step up by step_height
        let mut step_up_dy = step_height;
        for b in colliding_boxes {
            step_up_dy = start_box.calculate_y_offset(b, step_up_dy);
        }
        let box_stepped = start_box.offset(0.0, step_up_dy, 0.0);

        // Path A: X then Z
        let mut step_dx_a = delta[0];
        for b in colliding_boxes {
            step_dx_a = box_stepped.calculate_x_offset(b, step_dx_a);
        }
        let box_a = box_stepped.offset(step_dx_a, 0.0, 0.0);
        let mut step_dz_a = delta[2];
        for b in colliding_boxes {
            step_dz_a = box_a.calculate_z_offset(b, step_dz_a);
        }
        let box_a = box_a.offset(0.0, 0.0, step_dz_a);
        let mut step_down_a = -step_up_dy + delta[1];
        for b in colliding_boxes {
            step_down_a = box_a.calculate_y_offset(b, step_down_a);
        }

        // Path B: Z then X
        let mut step_dz_b = delta[2];
        for b in colliding_boxes {
            step_dz_b = box_stepped.calculate_z_offset(b, step_dz_b);
        }
        let box_b = box_stepped.offset(0.0, 0.0, step_dz_b);
        let mut step_dx_b = delta[0];
        for b in colliding_boxes {
            step_dx_b = box_b.calculate_x_offset(b, step_dx_b);
        }
        let box_b = box_b.offset(step_dx_b, 0.0, 0.0);
        let mut step_down_b = -step_up_dy + delta[1];
        for b in colliding_boxes {
            step_down_b = box_b.calculate_y_offset(b, step_down_b);
        }

        let dist_sq_a = step_dx_a * step_dx_a + step_dz_a * step_dz_a;
        let dist_sq_b = step_dz_b * step_dz_b + step_dx_b * step_dx_b;
        let (step_dx, step_dz, step_down) = if dist_sq_b > dist_sq_a {
            (step_dx_b, step_dz_b, step_down_b)
        } else {
            (step_dx_a, step_dz_a, step_down_a)
        };
        let stepped_dist_sq = dist_sq_a.max(dist_sq_b);
        let normal_dist_sq = resolved_dx * resolved_dx + resolved_dz * resolved_dz;

        if stepped_dist_sq > normal_dist_sq {
            resolved_dx = step_dx;
            resolved_dz = step_dz;
            resolved_dy = step_up_dy + step_down;
            stepped_up = true;
        }
    }

    let final_collided_h = if stepped_up {
        (resolved_dx - delta[0]).abs() > EPSILON || (resolved_dz - delta[2]).abs() > EPSILON
    } else {
        collided_horiz
    };

    let floor_hit = (resolved_dy - delta[1]).abs() > EPSILON && delta[1] < 0.0;
    let ceiling_hit = (resolved_dy - delta[1]).abs() > EPSILON && delta[1] > 0.0;
    let final_on_ground = if stepped_up {
        let stepped_box = start_box.offset(resolved_dx, resolved_dy, resolved_dz);
        let check_depth = 0.05;
        let mut actual_support = false;
        for b in colliding_boxes {
            if b.max[1] <= stepped_box.min[1] + EPSILON
                && b.max[1] >= stepped_box.min[1] - check_depth - EPSILON
                && stepped_box.min[0] < b.max[0]
                && stepped_box.max[0] > b.min[0]
                && stepped_box.min[2] < b.max[2]
                && stepped_box.max[2] > b.min[2]
            {
                actual_support = true;
                break;
            }
        }
        actual_support
    } else {
        floor_hit || (on_ground && resolved_dy.abs() < EPSILON && delta[1].abs() < EPSILON)
    };

    (
        [resolved_dx, resolved_dy, resolved_dz],
        final_on_ground,
        final_collided_h,
        floor_hit || ceiling_hit,
        ceiling_hit,
        stepped_up,
    )
}

/// Result of a discrete physics movement step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepResult {
    /// Actual coordinate displacement resolved during the tick.
    pub delta: [f64; 3],
    /// Whether the entity is resting on the ground after the tick.
    pub on_ground: bool,
    /// Whether a horizontal collision occurred against a wall/barrier.
    pub collided_horizontal: bool,
    /// Whether a vertical collision occurred against floor or ceiling.
    pub collided_vertical: bool,
    /// Whether the entity collided with a ceiling while moving upward.
    pub ceiling_collision: bool,
    /// Fall damage suffered upon landing this tick.
    pub fall_damage: u32,
    /// Whether step-height elevated the entity over an obstacle.
    pub stepped_up: bool,
}

/// Kinematic physics body for an entity.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsBody {
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub width: f64,
    pub height: f64,
    pub step_height: f64,
    pub on_ground: bool,
    pub fall_distance: f32,
}

impl PhysicsBody {
    /// Create a new `PhysicsBody` with given position and hitbox dimensions.
    pub fn new(position: [f64; 3], width: f64, height: f64) -> Self {
        Self {
            position,
            velocity: [0.0, 0.0, 0.0],
            width,
            height,
            step_height: DEFAULT_STEP_HEIGHT,
            on_ground: false,
            fall_distance: 0.0,
        }
    }

    /// Create a standard player physics body (width 0.6, height 1.8, step height 0.6).
    pub fn player(position: [f64; 3]) -> Self {
        Self::new(position, DEFAULT_PLAYER_WIDTH, DEFAULT_PLAYER_HEIGHT)
    }

    /// Return the entity's current axis-aligned bounding box.
    #[inline]
    pub fn aabb(&self) -> Aabb {
        Aabb::from_entity_pos(self.position, self.width, self.height)
    }

    /// Apply drag and gravity to the entity's velocity.
    pub fn apply_drag_and_gravity(&mut self) {
        apply_air_drag_and_gravity(&mut self.velocity);
    }

    /// Move the physics body by an explicit coordinate delta without applying gravity or drag.
    pub fn move_collide<W: WorldCollisionView>(
        &mut self,
        delta: [f64; 3],
        world: &W,
    ) -> StepResult {
        let was_in_air = !self.on_ground;
        let current_aabb = self.aabb();
        let query_box =
            current_aabb
                .expand(delta[0], delta[1], delta[2])
                .expand(0.0, self.step_height, 0.0);
        let colliding = world.get_colliding_boxes(&query_box);

        let (resolved, on_ground, collided_h, collided_v, ceiling_hit, stepped_up) =
            collide_and_sweep(
                &current_aabb,
                delta,
                &colliding,
                self.step_height,
                self.on_ground,
            );

        self.position[0] += resolved[0];
        self.position[1] += resolved[1];
        self.position[2] += resolved[2];
        self.on_ground = on_ground;

        if (resolved[0] - delta[0]).abs() > EPSILON {
            self.velocity[0] = 0.0;
        }
        if (resolved[2] - delta[2]).abs() > EPSILON {
            self.velocity[2] = 0.0;
        }
        if on_ground || ceiling_hit {
            self.velocity[1] = 0.0;
        }

        let mut fall_damage = 0;
        if self.on_ground {
            if resolved[1] < 0.0 {
                self.fall_distance += (-resolved[1]) as f32;
            }
            if was_in_air {
                fall_damage = compute_fall_damage(self.fall_distance);
                self.fall_distance = 0.0;
            } else {
                self.fall_distance = 0.0;
            }
        } else if resolved[1] < 0.0 {
            self.fall_distance += (-resolved[1]) as f32;
        }

        StepResult {
            delta: resolved,
            on_ground: self.on_ground,
            collided_horizontal: collided_h,
            collided_vertical: collided_v,
            ceiling_collision: ceiling_hit,
            fall_damage,
            stepped_up,
        }
    }

    /// Execute a discrete physics tick against the given collision world.
    ///
    /// Moves by the current velocity with swept collision and fall damage, then applies
    /// gravity and drag for the next tick: the Entity page's "position, acceleration,
    /// drag" order for living entities.
    pub fn step<W: WorldCollisionView>(&mut self, world: &W) -> StepResult {
        let result = self.move_collide(self.velocity, world);
        self.apply_drag_and_gravity();
        result
    }
}

#[cfg(test)]
mod tests;
