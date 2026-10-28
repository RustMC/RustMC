//! Deterministic entity velocity, gravity, terminal velocity, and AABB sweep collision loop.
//!
//! Milestone M5 implementation providing coordinate delta sweeping along axes,
//! step-height traversal over slabs/stairs, drag, gravity integration, and fall damage.

use crate::world::{Block, CHUNK_SIDE, Chunk, WORLD_HEIGHT};

/// Standard living entity gravity acceleration in blocks per tick.
pub const GRAVITY: f64 = 0.08;

/// Horizontal air drag damping factor per tick (preserves 98% of horizontal speed).
pub const HORIZONTAL_AIR_DRAG: f64 = 0.98;

/// Vertical air drag damping factor per tick (preserves 98% of vertical speed).
pub const VERTICAL_AIR_DRAG: f64 = 0.98;

/// Terminal downward velocity bound in blocks per tick (-3.92 blocks/tick).
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
#[inline]
pub fn compute_fall_damage(fall_distance: f32) -> u32 {
    if !fall_distance.is_finite() || fall_distance <= 3.0 {
        0
    } else {
        (fall_distance - 3.0).floor() as u32
    }
}

/// Floating-point variation of `compute_fall_damage`.
#[inline]
pub fn compute_fall_damage_f32(fall_distance: f32) -> f32 {
    if !fall_distance.is_finite() || fall_distance <= 3.0 {
        0.0
    } else {
        (fall_distance - 3.0).floor()
    }
}

/// Apply horizontal air drag, vertical air drag, gravity acceleration, and terminal velocity clamp.
///
/// Drag factors:
/// - Horizontal velocity multiplied by 0.98.
/// - Vertical velocity multiplied by 0.98, then subtracted by 0.08 blocks/tick.
/// - Terminal velocity clamped: `v_y = max(v_y, -3.92)`.
pub fn apply_air_drag_and_gravity(velocity: &mut [f64; 3]) {
    velocity[0] *= HORIZONTAL_AIR_DRAG;
    velocity[2] *= HORIZONTAL_AIR_DRAG;
    velocity[1] = (velocity[1] * VERTICAL_AIR_DRAG) - GRAVITY;
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
        true
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

        if (resolved[0] - delta[0]).abs() > EPSILON && !stepped_up {
            self.velocity[0] = 0.0;
        }
        if (resolved[2] - delta[2]).abs() > EPSILON && !stepped_up {
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
    /// Applies drag, gravity, swept collision resolution, and fall damage calculations.
    pub fn step<W: WorldCollisionView>(&mut self, world: &W) -> StepResult {
        self.apply_drag_and_gravity();
        self.move_collide(self.velocity, world)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freefall_in_empty_space_accelerates_and_clamps_at_terminal_velocity() {
        let mut body = PhysicsBody::player([0.0, 1000.0, 0.0]);
        let empty_world = Vec::<Aabb>::new();

        // Tick 1: v_y = (0 * 0.98) - 0.08 = -0.08
        let res = body.step(&empty_world);
        assert!((body.velocity[1] - -0.08).abs() < 1e-9);
        assert!((body.position[1] - 999.92).abs() < 1e-9);
        assert!(!res.on_ground);

        // Run until terminal velocity is reached
        for _ in 0..250 {
            body.step(&empty_world);
            assert!(body.velocity[1] >= TERMINAL_VELOCITY);
        }

        // Must stay clamped at TERMINAL_VELOCITY (-3.92)
        assert!((body.velocity[1] - TERMINAL_VELOCITY).abs() < 1e-6);
        assert!(!body.on_ground);
        assert!(body.fall_distance > 500.0);
    }

    #[test]
    fn landing_on_solid_floor_stops_vertical_motion_and_sets_on_ground_and_fall_damage() {
        let mut body = PhysicsBody::player([0.5, 5.0, 0.5]);
        // Solid block floor at y = 0 covering [0, 0, 0] to [1, 1, 1], so floor top is y = 1.0.
        let floor = Aabb::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let world = vec![floor];

        let mut landed = false;
        let mut recorded_damage = 0;
        for _ in 0..100 {
            let res = body.step(&world);
            if res.on_ground {
                landed = true;
                recorded_damage = res.fall_damage;
                break;
            }
        }

        assert!(landed);
        assert!((body.position[1] - 1.0).abs() < 1e-9);
        assert_eq!(body.velocity[1], 0.0);
        assert!(body.on_ground);
        // Fall from 5.0 to 1.0 is exactly 4.0 blocks: 4.0 - 3.0 = 1.0 damage
        assert_eq!(recorded_damage, 1);
        assert_eq!(body.fall_distance, 0.0);

        // Subsequent tick on ground stays resting at 1.0 with 0 damage
        let next_res = body.step(&world);
        assert!((body.position[1] - 1.0).abs() < 1e-9);
        assert_eq!(body.velocity[1], 0.0);
        assert!(next_res.on_ground);
        assert_eq!(next_res.fall_damage, 0);
    }

    #[test]
    fn fall_damage_values_3_blocks_4_blocks_and_24_blocks() {
        assert_eq!(compute_fall_damage(0.0), 0);
        assert_eq!(compute_fall_damage(2.9), 0);
        assert_eq!(compute_fall_damage(3.0), 0);
        assert_eq!(compute_fall_damage(3.5), 0);
        assert_eq!(compute_fall_damage(4.0), 1);
        assert_eq!(compute_fall_damage(4.9), 1);
        assert_eq!(compute_fall_damage(24.0), 21);
        assert_eq!(compute_fall_damage(24.9), 21);

        // Physical drop test: exactly 3 blocks (4.0 to 1.0)
        let mut body3 = PhysicsBody::player([0.5, 4.0, 0.5]);
        let world = vec![Aabb::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])];
        let mut dmg3 = 0;
        while !body3.on_ground {
            let r = body3.step(&world);
            if r.on_ground {
                dmg3 = r.fall_damage;
            }
        }
        assert_eq!(dmg3, 0);

        // Physical drop test: exactly 4 blocks (5.0 to 1.0)
        let mut body4 = PhysicsBody::player([0.5, 5.0, 0.5]);
        let mut dmg4 = 0;
        while !body4.on_ground {
            let r = body4.step(&world);
            if r.on_ground {
                dmg4 = r.fall_damage;
            }
        }
        assert_eq!(dmg4, 1);

        // Physical drop test: exactly 24 blocks (25.0 to 1.0)
        let mut body24 = PhysicsBody::player([0.5, 25.0, 0.5]);
        let mut dmg24 = 0;
        while !body24.on_ground {
            let r = body24.step(&world);
            if r.on_ground {
                dmg24 = r.fall_damage;
            }
        }
        assert_eq!(dmg24, 21);
    }

    #[test]
    fn wall_collision_slides_along_z_while_stopping_along_x() {
        let mut body = PhysicsBody::player([0.5, 0.0, 0.0]);
        body.on_ground = true;

        // Ground floor at y = [-1, 0]
        let floor = Aabb::new([-10.0, -1.0, -10.0], [10.0, 0.0, 10.0]);
        // Wall along X at x in [1.0, 2.0]
        let wall = Aabb::new([1.0, 0.0, -10.0], [2.0, 3.0, 10.0]);
        let world = vec![floor, wall];

        // Diagonal velocity into the wall along X and along Z
        body.velocity = [0.5, 0.0, 0.5];

        let res = body.step(&world);
        // Player width is 0.6, so max_x reaches wall at 1.0, meaning pos[0] stops at 0.7
        assert!((body.position[0] - 0.7).abs() < 1e-6);
        assert_eq!(body.velocity[0], 0.0);
        // Z movement continues without obstruction
        assert!(body.position[2] > 0.4);
        assert!(body.velocity[2] > 0.0);
        assert!(res.collided_horizontal);
        assert!(res.on_ground);

        // Continuing another tick: still blocked along X, still sliding along Z
        body.velocity[0] = 0.5;
        let res2 = body.step(&world);
        assert!((body.position[0] - 0.7).abs() < 1e-6);
        assert_eq!(body.velocity[0], 0.0);
        assert!(body.position[2] > 0.8);
        assert!(res2.collided_horizontal);
    }

    #[test]
    fn step_height_entity_walking_forward_into_slab_steps_up_smoothly() {
        let mut body = PhysicsBody::player([0.5, 0.0, 0.5]);
        body.on_ground = true;

        // Ground floor at y = [-1, 0]
        let floor = Aabb::new([-5.0, -1.0, -5.0], [5.0, 0.0, 5.0]);
        // 0.5-block slab at x in [1.0, 2.0], y in [0.0, 0.5], z in [0.0, 1.0]
        let slab = Aabb::new([1.0, 0.0, 0.0], [2.0, 0.5, 1.0]);
        let world = vec![floor, slab];

        // Walk forward in +X towards the slab
        body.velocity = [0.4, 0.0, 0.0];

        let res = body.step(&world);
        assert!(res.stepped_up);
        // Stepped onto the top of the slab at y = 0.5
        assert!((body.position[1] - 0.5).abs() < 1e-6);
        assert!(body.on_ground);
        // Advanced forward horizontally onto the slab
        assert!(body.position[0] > 0.8);
        // Velocity along X was not stopped because step-up cleared the barrier
        assert!(body.velocity[0] > 0.0);
    }

    #[test]
    fn full_wall_exceeding_step_height_blocks_movement() {
        let mut body = PhysicsBody::player([0.5, 0.0, 0.5]);
        body.on_ground = true;

        let floor = Aabb::new([-5.0, -1.0, -5.0], [5.0, 0.0, 5.0]);
        // 1.0-block tall wall (step height is 0.6, so cannot step up)
        let wall = Aabb::new([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]);
        let world = vec![floor, wall];

        body.velocity = [0.4, 0.0, 0.0];
        let res = body.step(&world);

        assert!(!res.stepped_up);
        assert_eq!(body.position[1], 0.0);
        assert!((body.position[0] - 0.7).abs() < 1e-6);
        assert_eq!(body.velocity[0], 0.0);
        assert!(res.collided_horizontal);
    }

    #[test]
    fn jumping_into_ceiling_stops_rising_motion() {
        let mut body = PhysicsBody::player([0.0, 1.0, 0.0]);
        // Ceiling at y = 3.0. Player height = 1.8, so head is at 2.8.
        let ceiling = Aabb::new([-5.0, 3.0, -5.0], [5.0, 4.0, 5.0]);
        let world = vec![ceiling];

        // Jump upward with high velocity
        body.velocity = [0.0, 1.0, 0.0];

        let res = body.step(&world);
        assert!(res.ceiling_collision);
        // Player top stopped at ceiling y = 3.0, so base pos[1] stopped at 3.0 - 1.8 = 1.2
        assert!((body.position[1] - 1.2).abs() < 1e-6);
        assert_eq!(body.velocity[1], 0.0);
        assert!(!body.on_ground);
    }

    #[test]
    fn aabb_intersects_expand_offset_contracts() {
        let a = Aabb::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let b = Aabb::new([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]);
        let c = Aabb::new([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]); // touching face
        let d = Aabb::new([2.0, 2.0, 2.0], [3.0, 3.0, 3.0]); // disjoint

        assert!(a.intersects(&b));
        assert!(b.intersects(&a));
        assert!(!a.intersects(&c)); // face touching is not intersecting
        assert!(!a.intersects(&d));

        let expanded = a.expand(1.0, -2.0, 0.5);
        assert_eq!(expanded.min, [0.0, -2.0, 0.0]);
        assert_eq!(expanded.max, [2.0, 1.0, 1.5]);

        let offset = a.offset(2.0, 3.0, 4.0);
        assert_eq!(offset.min, [2.0, 3.0, 4.0]);
        assert_eq!(offset.max, [3.0, 4.0, 5.0]);

        assert!(a.contains_point([0.5, 0.5, 0.5]));
        assert!(!a.contains_point([1.5, 0.5, 0.5]));
    }

    #[test]
    fn simple_voxel_world_and_chunk_collision_view() {
        let mut voxel_world = SimpleVoxelWorld::new();
        voxel_world.add_solid_block(0, 0, 0);
        voxel_world.add_solid_block(0, 1, 0);

        let query = Aabb::new([-0.5, -0.5, -0.5], [1.5, 2.5, 1.5]);
        let boxes = voxel_world.get_colliding_boxes(&query);
        assert_eq!(boxes.len(), 2);

        // Chunk query test
        let mut chunk = Chunk {
            x: 0,
            z: 0,
            blocks: vec![Block::Air; CHUNK_SIDE * CHUNK_SIDE * WORLD_HEIGHT],
        };
        // Place stone at (2, 5, 2)
        chunk.set(2, 5, 2, Block::Stone);

        let chunk_query = Aabb::new([1.5, 4.5, 1.5], [2.5, 5.5, 2.5]);
        let chunk_boxes = chunk.get_colliding_boxes(&chunk_query);
        assert_eq!(chunk_boxes.len(), 1);
        assert_eq!(chunk_boxes[0], Aabb::from_block(2, 5, 2));

        // Air only query returns nothing
        let air_query = Aabb::new([10.0, 10.0, 10.0], [11.0, 11.0, 11.0]);
        assert!(chunk.get_colliding_boxes(&air_query).is_empty());
    }
}
