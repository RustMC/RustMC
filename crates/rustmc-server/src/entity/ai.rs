//! Mob AI engine, GoalSelector, and behavioral goal execution loop.
//!
//! Implements Milestone M5 mob behavior systems adhering to vanilla Minecraft
//! AI mechanics: prioritized goal arbitration with flag-based mutual exclusion,
//! target acquisition, movement navigation, look orientation, and melee combat loops.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Add, AddAssign, BitAnd, BitOr, BitOrAssign, Div, Mul, Sub, SubAssign};

/// 3D coordinate vector with standard Euclidean arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    #[inline]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub fn length_squared(self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    #[inline]
    pub fn length(self) -> f64 {
        self.length_squared().sqrt()
    }

    #[inline]
    pub fn distance_squared(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        dx * dx + dy * dy + dz * dz
    }

    #[inline]
    pub fn distance(self, other: Self) -> f64 {
        self.distance_squared(other).sqrt()
    }

    #[inline]
    pub fn horizontal_distance_squared(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dz = self.z - other.z;
        dx * dx + dz * dz
    }

    #[inline]
    pub fn horizontal_distance(self, other: Self) -> f64 {
        self.horizontal_distance_squared(other).sqrt()
    }

    #[inline]
    pub fn normalize(self) -> Self {
        let len = self.length();
        if len > 1e-7 {
            Self {
                x: self.x / len,
                y: self.y / len,
                z: self.z / len,
            }
        } else {
            Self::ZERO
        }
    }
}

impl Add for Vec3 {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl AddAssign for Vec3 {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
        self.z += rhs.z;
    }
}

impl Sub for Vec3 {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

impl SubAssign for Vec3 {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
        self.z -= rhs.z;
    }
}

impl Mul<f64> for Vec3 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f64) -> Self {
        Self {
            x: self.x * rhs,
            y: self.y * rhs,
            z: self.z * rhs,
        }
    }
}

impl Div<f64> for Vec3 {
    type Output = Self;
    #[inline]
    fn div(self, rhs: f64) -> Self {
        Self {
            x: self.x / rhs,
            y: self.y / rhs,
            z: self.z / rhs,
        }
    }
}

/// Normalizes an angle in degrees into canonical Minecraft `[-180.0, 180.0)` bounds.
#[inline]
pub fn wrap_degrees(mut degrees: f32) -> f32 {
    degrees %= 360.0;
    if degrees >= 180.0 {
        degrees -= 360.0;
    }
    if degrees < -180.0 {
        degrees += 360.0;
    }
    degrees
}

/// Computes Minecraft yaw and pitch (in degrees) to look from observer eye to target eye.
///
/// In vanilla coordinate orientation:
/// - Yaw: 0 deg = South (+Z), 90 deg = West (-X), 180/-180 deg = North (-Z), -90 deg = East (+X).
/// - Pitch: -90 deg = looking straight up (+Y), 0 deg = level, +90 deg = looking straight down (-Y).
pub fn calculate_look_at(from: Vec3, to: Vec3) -> (f32, f32) {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let dz = to.z - from.z;
    let horizontal_dist = (dx * dx + dz * dz).sqrt();

    let yaw = wrap_degrees((-dx).atan2(dz).to_degrees() as f32);
    let pitch = ((-dy).atan2(horizontal_dist).to_degrees() as f32).clamp(-90.0, 90.0);

    (yaw, pitch)
}

/// Deterministic, reproducible pseudo-random number generator (SplitMix64).
#[derive(Debug, Clone)]
pub struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    pub const fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x517c_c1b7_2722_0a95
            } else {
                seed
            },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn next_i32_range(&mut self, min: i32, max: i32) -> i32 {
        if min >= max {
            return min;
        }
        let range = (max - min + 1) as u64;
        min + (self.next_u64() % range) as i32
    }

    pub fn next_u32_range(&mut self, min: u32, max: u32) -> u32 {
        if min >= max {
            return min;
        }
        let range = (max - min + 1) as u64;
        min + (self.next_u64() % range) as u32
    }
}

/// Bitflags representing behavioral subsystems required by a goal to resolve mutual exclusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct GoalFlags(u8);

impl GoalFlags {
    pub const NONE: Self = Self(0);
    pub const MOVE: Self = Self(1 << 0);
    pub const LOOK: Self = Self(1 << 1);
    pub const JUMP: Self = Self(1 << 2);
    pub const TARGET: Self = Self(1 << 3);

    #[inline]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline]
    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[inline]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    #[inline]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[inline]
    pub const fn from_bits_truncate(bits: u8) -> Self {
        Self(bits & 0b1111)
    }
}

impl BitOr for GoalFlags {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for GoalFlags {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for GoalFlags {
    type Output = Self;
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

/// Classification of entities recognized by the AI and targeting systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityKind {
    Player,
    Monster,
    Animal,
    Item,
    Other,
}

/// World entity snapshot or entry used for spatial queries, targeting, and combat.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityEntry {
    pub id: u32,
    pub kind: EntityKind,
    pub position: Vec3,
    pub width: f32,
    pub height: f32,
    pub eye_height: f32,
    pub health: f32,
    pub max_health: f32,
}

impl EntityEntry {
    pub fn new(id: u32, kind: EntityKind, position: Vec3) -> Self {
        Self {
            id,
            kind,
            position,
            width: 0.6,
            height: 1.8,
            eye_height: 1.62,
            health: 20.0,
            max_health: 20.0,
        }
    }

    pub fn with_dimensions(mut self, width: f32, height: f32, eye_height: f32) -> Self {
        self.width = width;
        self.height = height;
        self.eye_height = eye_height;
        self
    }

    pub fn with_health(mut self, health: f32, max_health: f32) -> Self {
        self.health = health;
        self.max_health = max_health;
        self
    }

    #[inline]
    pub fn eye_position(&self) -> Vec3 {
        Vec3::new(
            self.position.x,
            self.position.y + self.eye_height as f64,
            self.position.z,
        )
    }

    #[inline]
    pub fn is_alive(&self) -> bool {
        self.health > 0.0
    }
}

/// Recorded combat event when damage is delivered by a mob attack goal.
#[derive(Debug, Clone, PartialEq)]
pub struct DamageEvent {
    pub target_id: u32,
    pub damage: f32,
    pub remaining_health: f32,
}

/// View of the game world providing spatial entity indexing, terrain queries, and combat outcomes.
#[derive(Default)]
pub struct WorldView {
    entities: RefCell<BTreeMap<u32, EntityEntry>>,
    solid_blocks: RefCell<BTreeSet<(i32, i32, i32)>>,
    flat_ground_y: Option<i32>,
    damage_events: RefCell<Vec<DamageEvent>>,
}

impl WorldView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a world view where all coordinates at or below `ground_y` are solid ground.
    pub fn with_flat_ground(ground_y: i32) -> Self {
        Self {
            entities: RefCell::new(BTreeMap::new()),
            solid_blocks: RefCell::new(BTreeSet::new()),
            flat_ground_y: Some(ground_y),
            damage_events: RefCell::new(Vec::new()),
        }
    }

    pub fn add_entity(&self, entity: EntityEntry) {
        self.entities.borrow_mut().insert(entity.id, entity);
    }

    pub fn remove_entity(&self, id: u32) -> Option<EntityEntry> {
        self.entities.borrow_mut().remove(&id)
    }

    pub fn get_entity(&self, id: u32) -> Option<EntityEntry> {
        self.entities.borrow().get(&id).cloned()
    }

    pub fn update_entity_position(&self, id: u32, pos: Vec3) -> bool {
        if let Some(entry) = self.entities.borrow_mut().get_mut(&id) {
            entry.position = pos;
            true
        } else {
            false
        }
    }

    pub fn find_nearest_player(&self, pos: Vec3, max_range: f64) -> Option<EntityEntry> {
        self.find_nearest_target(pos, max_range, EntityKind::Player)
    }

    pub fn find_nearest_target(
        &self,
        pos: Vec3,
        max_range: f64,
        kind: EntityKind,
    ) -> Option<EntityEntry> {
        let max_range_sq = max_range * max_range;
        let entities = self.entities.borrow();
        entities
            .values()
            .filter(|e| e.kind == kind && e.is_alive())
            .filter_map(|e| {
                let dist_sq = pos.distance_squared(e.position);
                if dist_sq <= max_range_sq {
                    Some((dist_sq, e))
                } else {
                    None
                }
            })
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, e)| e.clone())
    }

    pub fn set_solid(&self, x: i32, y: i32, z: i32, solid: bool) {
        let mut blocks = self.solid_blocks.borrow_mut();
        if solid {
            blocks.insert((x, y, z));
        } else {
            blocks.remove(&(x, y, z));
        }
    }

    pub fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
        if self.solid_blocks.borrow().contains(&(x, y, z)) {
            return true;
        }
        if let Some(gy) = self.flat_ground_y {
            y <= gy
        } else {
            false
        }
    }

    /// Validates whether standing at block `(x, y, z)` has solid ground support
    /// beneath (`y - 1`) and passable clearance at body (`y`) and head (`y + 1`).
    pub fn has_ground_support(&self, x: i32, y: i32, z: i32) -> bool {
        self.is_solid(x, y - 1, z) && !self.is_solid(x, y, z) && !self.is_solid(x, y + 1, z)
    }

    /// Delivers melee damage to a target entity, recording damage and updating entity health.
    pub fn apply_damage(&self, target_id: u32, amount: f32) -> bool {
        if !amount.is_finite() || amount < 0.0 {
            return false;
        }
        let mut entities = self.entities.borrow_mut();
        if let Some(entry) = entities.get_mut(&target_id) {
            entry.health = (entry.health - amount).max(0.0);
            let remaining = entry.health;
            self.damage_events.borrow_mut().push(DamageEvent {
                target_id,
                damage: amount,
                remaining_health: remaining,
            });
            true
        } else {
            false
        }
    }

    pub fn damage_events(&self) -> Vec<DamageEvent> {
        self.damage_events.borrow().clone()
    }

    pub fn clear_damage_events(&self) {
        self.damage_events.borrow_mut().clear();
    }
}

/// Behavioral AI goal interface evaluated periodically by a `GoalSelector`.
pub trait Goal {
    /// Evaluates whether the goal can begin execution.
    fn can_use(&mut self, mob: &MobEntity, world: &WorldView) -> bool;

    /// Evaluates whether an already active goal can continue execution.
    fn can_continue_to_use(&mut self, mob: &MobEntity, world: &WorldView) -> bool {
        self.can_use(mob, world)
    }

    /// Invoked once when the goal transitions from inactive to active.
    fn start(&mut self, _mob: &mut MobEntity) {}

    /// Stepped every tick while the goal is active.
    fn tick(&mut self, _mob: &mut MobEntity, _world: &WorldView) {}

    /// Invoked once when the goal stops executing or is preempted.
    fn stop(&mut self, _mob: &mut MobEntity) {}

    /// Subsystem flags required by this goal for mutual exclusion resolution.
    fn flags(&self) -> GoalFlags {
        GoalFlags::empty()
    }
}

/// Wrapped goal container with an assigned priority rank and active execution state.
pub struct PrioritizedGoal {
    pub priority: u32,
    pub goal: Box<dyn Goal>,
    pub is_running: bool,
}

impl std::fmt::Debug for PrioritizedGoal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrioritizedGoal")
            .field("priority", &self.priority)
            .field("is_running", &self.is_running)
            .field("flags", &self.goal.flags())
            .finish()
    }
}

/// Manages prioritized behavioral goals, resolving flag mutual exclusion and step transitions.
#[derive(Default)]
pub struct GoalSelector {
    goals: Vec<PrioritizedGoal>,
}

impl std::fmt::Debug for GoalSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalSelector")
            .field("goals", &self.goals)
            .finish()
    }
}

impl GoalSelector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a prioritized goal (lower integer indicates higher execution priority).
    pub fn add_goal(&mut self, priority: u32, goal: Box<dyn Goal>) {
        self.goals.push(PrioritizedGoal {
            priority,
            goal,
            is_running: false,
        });
        self.goals.sort_by_key(|g| g.priority);
    }

    pub fn goals(&self) -> &[PrioritizedGoal] {
        &self.goals
    }

    pub fn running_count(&self) -> usize {
        self.goals.iter().filter(|g| g.is_running).count()
    }

    pub fn is_goal_running(&self, index: usize) -> bool {
        self.goals.get(index).is_some_and(|g| g.is_running)
    }

    /// Ticks active goals, stops ineligible goals, and arbitrates new goals in priority order.
    pub fn tick(&mut self, mob: &mut MobEntity, world: &WorldView) {
        // Step 1: Stop running goals that can no longer continue
        for pg in &mut self.goals {
            if pg.is_running && !pg.goal.can_continue_to_use(mob, world) {
                pg.goal.stop(mob);
                pg.is_running = false;
            }
        }

        // Step 2: Goal arbitration in ascending priority order (1 runs before 2)
        let mut claimed_flags = GoalFlags::empty();

        for i in 0..self.goals.len() {
            let flags = self.goals[i].goal.flags();

            // Conflict with a higher-priority goal that already claimed flags
            if claimed_flags.intersects(flags) {
                if self.goals[i].is_running {
                    self.goals[i].goal.stop(mob);
                    self.goals[i].is_running = false;
                }
                continue;
            }

            if self.goals[i].is_running {
                claimed_flags |= flags;
            } else if self.goals[i].goal.can_use(mob, world) {
                // Preempt any lower-priority running goals that conflict with this goal's flags
                let mut to_preempt = Vec::new();
                for j in (i + 1)..self.goals.len() {
                    if self.goals[j].is_running && self.goals[j].goal.flags().intersects(flags) {
                        to_preempt.push(j);
                    }
                }
                for j in to_preempt {
                    self.goals[j].goal.stop(mob);
                    self.goals[j].is_running = false;
                }

                self.goals[i].goal.start(mob);
                self.goals[i].is_running = true;
                claimed_flags |= flags;
            }
        }

        // Step 3: Tick all active goals
        for pg in &mut self.goals {
            if pg.is_running {
                pg.goal.tick(mob, world);
            }
        }
    }
}

/// Autonomous mob entity holding kinematic transform, health, targeting, and goal selectors.
pub struct MobEntity {
    pub id: u32,
    pub kind: EntityKind,
    pub position: Vec3,
    pub velocity: Vec3,
    pub navigation_velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub max_health: f32,
    pub target_id: Option<u32>,
    pub width: f32,
    pub height: f32,
    pub eye_height: f32,
    pub goal_selector: GoalSelector,
    pub target_selector: GoalSelector,
}

impl std::fmt::Debug for MobEntity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobEntity")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("position", &self.position)
            .field("velocity", &self.velocity)
            .field("navigation_velocity", &self.navigation_velocity)
            .field("yaw", &self.yaw)
            .field("pitch", &self.pitch)
            .field("health", &self.health)
            .field("max_health", &self.max_health)
            .field("target_id", &self.target_id)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("eye_height", &self.eye_height)
            .finish()
    }
}

impl MobEntity {
    pub fn new(id: u32, kind: EntityKind, position: Vec3) -> Self {
        Self {
            id,
            kind,
            position,
            velocity: Vec3::ZERO,
            navigation_velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            health: 20.0,
            max_health: 20.0,
            target_id: None,
            width: 0.6,
            height: 1.8,
            eye_height: 1.62,
            goal_selector: GoalSelector::new(),
            target_selector: GoalSelector::new(),
        }
    }

    pub fn with_dimensions(mut self, width: f32, height: f32, eye_height: f32) -> Self {
        self.width = width;
        self.height = height;
        self.eye_height = eye_height;
        self
    }

    pub fn with_health(mut self, health: f32, max_health: f32) -> Self {
        self.health = health;
        self.max_health = max_health;
        self
    }

    #[inline]
    pub fn eye_position(&self) -> Vec3 {
        Vec3::new(
            self.position.x,
            self.position.y + self.eye_height as f64,
            self.position.z,
        )
    }

    pub fn add_goal(&mut self, priority: u32, goal: Box<dyn Goal>) {
        self.goal_selector.add_goal(priority, goal);
    }

    pub fn add_target_goal(&mut self, priority: u32, goal: Box<dyn Goal>) {
        self.target_selector.add_goal(priority, goal);
    }

    /// Steps active behavioral goals and integrates kinematic movement into entity position.
    pub fn tick(&mut self, world: &WorldView) {
        // Step active goals without borrow conflicts
        let mut target_selector = std::mem::take(&mut self.target_selector);
        target_selector.tick(self, world);
        self.target_selector = target_selector;

        let mut goal_selector = std::mem::take(&mut self.goal_selector);
        goal_selector.tick(self, world);
        self.goal_selector = goal_selector;

        self.apply_movement();
        world.update_entity_position(self.id, self.position);
    }

    pub fn apply_movement(&mut self) {
        self.velocity = self.navigation_velocity;
        self.position += self.velocity;
    }
}

/// Vanilla Goal: Detects nearest player within range and rotates mob yaw and pitch toward player head.
pub struct LookAtPlayerGoal {
    range: f64,
    look_time: u32,
    min_look_time: u32,
    max_look_time: u32,
    target_player_id: Option<u32>,
    rng: SimpleRng,
}

impl LookAtPlayerGoal {
    pub fn new(range: f64) -> Self {
        Self {
            range,
            look_time: 0,
            min_look_time: 40,
            max_look_time: 80,
            target_player_id: None,
            rng: SimpleRng::new(0x1004_a791),
        }
    }

    pub fn with_duration(mut self, min_ticks: u32, max_ticks: u32) -> Self {
        self.min_look_time = min_ticks;
        self.max_look_time = max_ticks;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.rng = SimpleRng::new(seed);
        self
    }

    pub fn target_player_id(&self) -> Option<u32> {
        self.target_player_id
    }

    pub fn look_time(&self) -> u32 {
        self.look_time
    }
}

impl Goal for LookAtPlayerGoal {
    fn flags(&self) -> GoalFlags {
        GoalFlags::LOOK
    }

    fn can_use(&mut self, mob: &MobEntity, world: &WorldView) -> bool {
        if let Some(player) = world.find_nearest_player(mob.position, self.range) {
            self.target_player_id = Some(player.id);
            true
        } else {
            false
        }
    }

    fn can_continue_to_use(&mut self, mob: &MobEntity, world: &WorldView) -> bool {
        if self.look_time == 0 {
            return false;
        }
        let Some(target_id) = self.target_player_id else {
            return false;
        };
        let Some(player) = world.get_entity(target_id) else {
            return false;
        };
        if !player.is_alive() {
            return false;
        }
        mob.position.distance_squared(player.position) <= self.range * self.range
    }

    fn start(&mut self, _mob: &mut MobEntity) {
        self.look_time = self
            .rng
            .next_u32_range(self.min_look_time, self.max_look_time);
    }

    fn tick(&mut self, mob: &mut MobEntity, world: &WorldView) {
        self.orient_towards_target(mob, world);
        if self.look_time > 0 {
            self.look_time -= 1;
        }
    }

    fn stop(&mut self, _mob: &mut MobEntity) {
        self.target_player_id = None;
        self.look_time = 0;
    }
}

impl LookAtPlayerGoal {
    fn orient_towards_target(&self, mob: &mut MobEntity, world: &WorldView) {
        if let Some(player) = self
            .target_player_id
            .and_then(|target_id| world.get_entity(target_id))
        {
            let (yaw, pitch) = calculate_look_at(mob.eye_position(), player.eye_position());
            mob.yaw = yaw;
            mob.pitch = pitch;
        }
    }
}

/// Vanilla Goal: Selects a ground-supported coordinate in range and navigates entity toward it.
pub struct RandomStrollGoal {
    speed_modifier: f64,
    horizontal_radius: i32,
    vertical_radius: i32,
    target_pos: Option<Vec3>,
    stroll_duration: u32,
    max_duration: u32,
    interval_ticks: u32,
    cooldown: u32,
    rng: SimpleRng,
}

impl RandomStrollGoal {
    pub fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            horizontal_radius: 10,
            vertical_radius: 3,
            target_pos: None,
            stroll_duration: 0,
            max_duration: 120,
            interval_ticks: 60,
            cooldown: 0,
            rng: SimpleRng::new(0x5780_11aa),
        }
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.rng = SimpleRng::new(seed);
        self
    }

    pub fn with_radius(mut self, horizontal: i32, vertical: i32) -> Self {
        self.horizontal_radius = horizontal;
        self.vertical_radius = vertical;
        self
    }

    pub fn with_interval(mut self, interval_ticks: u32) -> Self {
        self.interval_ticks = interval_ticks;
        self
    }

    pub fn target_pos(&self) -> Option<Vec3> {
        self.target_pos
    }

    fn find_ground_target(&mut self, mob: &MobEntity, world: &WorldView) -> Option<Vec3> {
        let base_x = mob.position.x.floor() as i32;
        let base_y = mob.position.y.floor() as i32;
        let base_z = mob.position.z.floor() as i32;

        for _ in 0..16 {
            let dx = self
                .rng
                .next_i32_range(-self.horizontal_radius, self.horizontal_radius);
            let dz = self
                .rng
                .next_i32_range(-self.horizontal_radius, self.horizontal_radius);
            let dy = self
                .rng
                .next_i32_range(-self.vertical_radius, self.vertical_radius);

            let cx = base_x + dx;
            let cy = base_y + dy;
            let cz = base_z + dz;

            if world.has_ground_support(cx, cy, cz) {
                return Some(Vec3::new(cx as f64 + 0.5, cy as f64, cz as f64 + 0.5));
            }
        }
        None
    }
}

impl Goal for RandomStrollGoal {
    fn flags(&self) -> GoalFlags {
        GoalFlags::MOVE
    }

    fn can_use(&mut self, mob: &MobEntity, world: &WorldView) -> bool {
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return false;
        }

        if let Some(target) = self.find_ground_target(mob, world) {
            self.target_pos = Some(target);
            true
        } else {
            self.cooldown = 10;
            false
        }
    }

    fn can_continue_to_use(&mut self, mob: &MobEntity, _world: &WorldView) -> bool {
        let Some(target) = self.target_pos else {
            return false;
        };
        if self.stroll_duration >= self.max_duration {
            return false;
        }
        // Continue while entity has not reached target center
        let dist_sq = mob.position.horizontal_distance_squared(target);
        dist_sq > 0.25
    }

    fn start(&mut self, mob: &mut MobEntity) {
        self.stroll_duration = 0;
        if let Some(target) = self.target_pos {
            let delta = target - mob.position;
            mob.navigation_velocity = delta.normalize() * self.speed_modifier;
        }
    }

    fn tick(&mut self, mob: &mut MobEntity, _world: &WorldView) {
        self.stroll_duration += 1;
        if let Some(target) = self.target_pos {
            let delta = target - mob.position;
            mob.navigation_velocity = delta.normalize() * self.speed_modifier;
        }
    }

    fn stop(&mut self, mob: &mut MobEntity) {
        self.target_pos = None;
        self.stroll_duration = 0;
        self.cooldown = self.interval_ticks;
        mob.navigation_velocity = Vec3::ZERO;
    }
}

/// Vanilla Goal: Scans spatial index for nearest enemy targets and sets mob target entity ID.
pub struct NearestAttackableTargetGoal {
    target_kind: EntityKind,
    range: f64,
    target_id: Option<u32>,
}

impl NearestAttackableTargetGoal {
    pub fn new(target_kind: EntityKind, range: f64) -> Self {
        Self {
            target_kind,
            range,
            target_id: None,
        }
    }

    pub fn for_player(range: f64) -> Self {
        Self::new(EntityKind::Player, range)
    }

    pub fn target_id(&self) -> Option<u32> {
        self.target_id
    }
}

impl Goal for NearestAttackableTargetGoal {
    fn flags(&self) -> GoalFlags {
        GoalFlags::TARGET
    }

    fn can_use(&mut self, mob: &MobEntity, world: &WorldView) -> bool {
        if let Some(target) = world.find_nearest_target(mob.position, self.range, self.target_kind)
        {
            self.target_id = Some(target.id);
            true
        } else {
            false
        }
    }

    fn can_continue_to_use(&mut self, mob: &MobEntity, world: &WorldView) -> bool {
        let Some(target_id) = self.target_id else {
            return false;
        };
        let Some(target) = world.get_entity(target_id) else {
            return false;
        };
        if !target.is_alive() {
            return false;
        }
        mob.position.distance_squared(target.position) <= (self.range * 1.5) * (self.range * 1.5)
    }

    fn start(&mut self, mob: &mut MobEntity) {
        mob.target_id = self.target_id;
    }

    fn stop(&mut self, mob: &mut MobEntity) {
        if mob.target_id == self.target_id {
            mob.target_id = None;
        }
        self.target_id = None;
    }
}

/// Vanilla Goal: Pursues current target entity, checking reach distance and delivering melee damage on cooldown.
pub struct MeleeAttackGoal {
    speed_modifier: f64,
    damage: f32,
    attack_interval: u32,
    cooldown_ticks: u32,
}

impl MeleeAttackGoal {
    pub fn new(speed_modifier: f64, damage: f32, attack_interval: u32) -> Self {
        Self {
            speed_modifier,
            damage,
            attack_interval,
            cooldown_ticks: 0,
        }
    }

    pub fn for_hostile_mob(damage: f32) -> Self {
        Self::new(0.3, damage, 20)
    }

    pub fn cooldown_ticks(&self) -> u32 {
        self.cooldown_ticks
    }

    /// Vanilla Minecraft attack reach distance squared: `(mob_width + target_width)^2 + 1.0`.
    #[inline]
    pub fn attack_reach_squared(mob_width: f32, target_width: f32) -> f64 {
        let sum = (mob_width + target_width) as f64;
        sum * sum + 1.0
    }
}

impl Goal for MeleeAttackGoal {
    fn flags(&self) -> GoalFlags {
        GoalFlags::MOVE | GoalFlags::LOOK
    }

    fn can_use(&mut self, mob: &MobEntity, world: &WorldView) -> bool {
        let Some(target_id) = mob.target_id else {
            return false;
        };
        let Some(target) = world.get_entity(target_id) else {
            return false;
        };
        target.is_alive()
    }

    fn can_continue_to_use(&mut self, mob: &MobEntity, world: &WorldView) -> bool {
        self.can_use(mob, world)
    }

    fn start(&mut self, _mob: &mut MobEntity) {}

    fn tick(&mut self, mob: &mut MobEntity, world: &WorldView) {
        let Some(target_id) = mob.target_id else {
            return;
        };
        let Some(target) = world.get_entity(target_id) else {
            return;
        };

        let (yaw, pitch) = calculate_look_at(mob.eye_position(), target.eye_position());
        mob.yaw = yaw;
        mob.pitch = pitch;

        let dist_sq = mob.position.distance_squared(target.position);
        let reach_sq = Self::attack_reach_squared(mob.width, target.width);

        if dist_sq <= reach_sq {
            // Target is in reach
            mob.navigation_velocity = Vec3::ZERO;
            if self.cooldown_ticks == 0 {
                world.apply_damage(target.id, self.damage);
                self.cooldown_ticks = self.attack_interval;
            }
        } else {
            // Pursue target
            let delta = target.position - mob.position;
            mob.navigation_velocity = delta.normalize() * self.speed_modifier;
        }

        if self.cooldown_ticks > 0 {
            self.cooldown_ticks -= 1;
        }
    }

    fn stop(&mut self, mob: &mut MobEntity) {
        mob.navigation_velocity = Vec3::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test dummy goal for arbitration testing.
    struct TestGoal {
        flags: GoalFlags,
        can_run: bool,
        was_started: bool,
        was_ticked: bool,
        was_stopped: bool,
    }

    impl TestGoal {
        fn new(flags: GoalFlags) -> Self {
            Self {
                flags,
                can_run: true,
                was_started: false,
                was_ticked: false,
                was_stopped: false,
            }
        }
    }

    impl Goal for TestGoal {
        fn flags(&self) -> GoalFlags {
            self.flags
        }

        fn can_use(&mut self, _mob: &MobEntity, _world: &WorldView) -> bool {
            self.can_run
        }

        fn can_continue_to_use(&mut self, _mob: &MobEntity, _world: &WorldView) -> bool {
            self.can_run
        }

        fn start(&mut self, _mob: &mut MobEntity) {
            self.was_started = true;
        }

        fn tick(&mut self, _mob: &mut MobEntity, _world: &WorldView) {
            self.was_ticked = true;
        }

        fn stop(&mut self, _mob: &mut MobEntity) {
            self.was_stopped = true;
        }
    }

    #[test]
    fn prioritized_goal_arbitration_preempts_conflicting_lower_priority() {
        let mut selector = GoalSelector::new();
        let mut mob = MobEntity::new(1, EntityKind::Monster, Vec3::new(0.0, 60.0, 0.0));
        let world = WorldView::new();

        // Goal A: Priority 1, MOVE
        // Goal B: Priority 2, MOVE (conflicts with A)
        // Goal C: Priority 3, LOOK (no conflict with MOVE)
        selector.add_goal(1, Box::new(TestGoal::new(GoalFlags::MOVE)));
        selector.add_goal(2, Box::new(TestGoal::new(GoalFlags::MOVE)));
        selector.add_goal(3, Box::new(TestGoal::new(GoalFlags::LOOK)));

        // Ticking should start Goal A (priority 1) and Goal C (priority 3, different flags).
        // Goal B (priority 2) conflicts on MOVE with Goal A, so it must NOT start.
        selector.tick(&mut mob, &world);

        assert_eq!(selector.running_count(), 2);
        assert!(selector.is_goal_running(0)); // Goal A
        assert!(!selector.is_goal_running(1)); // Goal B
        assert!(selector.is_goal_running(2)); // Goal C
    }

    #[test]
    fn higher_priority_goal_preempts_already_running_lower_priority_goal() {
        let mut selector = GoalSelector::new();
        let mut mob = MobEntity::new(1, EntityKind::Monster, Vec3::new(0.0, 60.0, 0.0));
        let world = WorldView::new();

        let mut high_goal = TestGoal::new(GoalFlags::MOVE);
        high_goal.can_run = false; // Initially inactive
        let low_goal = TestGoal::new(GoalFlags::MOVE);

        selector.add_goal(1, Box::new(high_goal));
        selector.add_goal(2, Box::new(low_goal));

        // Tick 1: High priority goal cannot run; low priority goal starts
        selector.tick(&mut mob, &world);
        assert!(!selector.is_goal_running(0));
        assert!(selector.is_goal_running(1));

        // Now high priority goal becomes eligible
        // To simulate this, we re-add or mutate; let's verify via selector manipulation:
        let mut selector2 = GoalSelector::new();
        let mut high2 = TestGoal::new(GoalFlags::MOVE);
        high2.can_run = true;
        let mut low2 = TestGoal::new(GoalFlags::MOVE);
        low2.can_run = true;
        selector2.add_goal(1, Box::new(high2));
        selector2.add_goal(2, Box::new(low2));

        // Force low2 to be running initially
        selector2.goals[1].is_running = true;

        // On tick, high2 (priority 1) preempts low2 (priority 2)
        selector2.tick(&mut mob, &world);
        assert!(selector2.is_goal_running(0));
        assert!(!selector2.is_goal_running(1));
    }

    #[test]
    fn look_at_player_goal_facing_calculation_matches_target() {
        // Observer at (0, 60, 0) with eye height 1.62 -> eye at (0, 61.62, 0)
        let mob_eye = Vec3::new(0.0, 61.62, 0.0);

        // Target 1: South (+Z)
        let target_south = Vec3::new(0.0, 61.62, 10.0);
        let (yaw, pitch) = calculate_look_at(mob_eye, target_south);
        assert!(
            (yaw - 0.0).abs() < 1e-3,
            "Expected South yaw 0.0, got {yaw}"
        );
        assert!(
            (pitch - 0.0).abs() < 1e-3,
            "Expected level pitch 0.0, got {pitch}"
        );

        // Target 2: West (-X)
        let target_west = Vec3::new(-10.0, 61.62, 0.0);
        let (yaw, pitch) = calculate_look_at(mob_eye, target_west);
        assert!(
            (yaw - 90.0).abs() < 1e-3,
            "Expected West yaw 90.0, got {yaw}"
        );
        assert!((pitch - 0.0).abs() < 1e-3);

        // Target 3: North (-Z)
        let target_north = Vec3::new(0.0, 61.62, -10.0);
        let (yaw, pitch) = calculate_look_at(mob_eye, target_north);
        assert!(
            (yaw.abs() - 180.0).abs() < 1e-3,
            "Expected North yaw 180 or -180, got {yaw}"
        );
        assert!((pitch - 0.0).abs() < 1e-3);

        // Target 4: East (+X)
        let target_east = Vec3::new(10.0, 61.62, 0.0);
        let (yaw, pitch) = calculate_look_at(mob_eye, target_east);
        assert!(
            (yaw - (-90.0)).abs() < 1e-3,
            "Expected East yaw -90.0, got {yaw}"
        );
        assert!((pitch - 0.0).abs() < 1e-3);

        // Target 5: Elevated target looking upward (pitch < 0)
        let target_up = Vec3::new(0.0, 71.62, 10.0);
        let (_, pitch_up) = calculate_look_at(mob_eye, target_up);
        assert!(
            pitch_up < -40.0 && pitch_up > -50.0,
            "Expected ~ -45 deg pitch, got {pitch_up}"
        );

        // Target 6: Depressed target looking downward (pitch > 0)
        let target_down = Vec3::new(0.0, 51.62, 10.0);
        let (_, pitch_down) = calculate_look_at(mob_eye, target_down);
        assert!(
            pitch_down > 40.0 && pitch_down < 50.0,
            "Expected ~ +45 deg pitch, got {pitch_down}"
        );
    }

    #[test]
    fn look_at_player_goal_orients_mob_towards_player() {
        let world = WorldView::new();
        let player = EntityEntry::new(10, EntityKind::Player, Vec3::new(5.0, 60.0, 0.0));
        world.add_entity(player);

        let mut mob = MobEntity::new(1, EntityKind::Monster, Vec3::new(0.0, 60.0, 0.0));
        let look_goal = LookAtPlayerGoal::new(8.0).with_duration(20, 20);
        mob.add_goal(1, Box::new(look_goal));

        // Before tick, yaw is 0
        assert_eq!(mob.yaw, 0.0);

        // Tick steps the goal, finding player at +X (East, yaw = -90.0)
        mob.tick(&world);
        assert!(
            (mob.yaw - (-90.0)).abs() < 1e-2,
            "Mob yaw should face player at East, got {}",
            mob.yaw
        );
    }

    #[test]
    fn random_stroll_goal_selects_reachable_coordinates_and_moves_entity() {
        // Flat ground at y = 60
        let world = WorldView::with_flat_ground(60);
        let mut mob = MobEntity::new(1, EntityKind::Monster, Vec3::new(0.0, 61.0, 0.0));

        let stroll_goal = RandomStrollGoal::new(0.5).with_seed(42);
        mob.add_goal(1, Box::new(stroll_goal));

        let initial_pos = mob.position;
        mob.tick(&world);

        // Entity should have acquired navigation velocity and moved
        assert!(
            mob.velocity.length_squared() > 0.0,
            "Mob velocity should be non-zero during stroll"
        );
        assert_ne!(
            mob.position, initial_pos,
            "Mob position should change after tick"
        );
    }

    #[test]
    fn nearest_attackable_target_and_melee_attack_goal_deal_damage_in_reach() {
        let world = WorldView::with_flat_ground(60);
        let player = EntityEntry::new(100, EntityKind::Player, Vec3::new(1.0, 61.0, 0.0))
            .with_dimensions(0.6, 1.8, 1.62)
            .with_health(20.0, 20.0);
        world.add_entity(player);

        let mut mob = MobEntity::new(1, EntityKind::Monster, Vec3::new(0.0, 61.0, 0.0))
            .with_dimensions(0.6, 1.8, 1.62);

        // Target goal acquires player
        mob.add_target_goal(1, Box::new(NearestAttackableTargetGoal::for_player(16.0)));
        // Melee attack goal delivers 4.0 damage with 20 tick cooldown
        mob.add_goal(1, Box::new(MeleeAttackGoal::new(0.3, 4.0, 20)));

        // Tick 0: Target acquired and melee damage delivered immediately since distance (1.0) <= reach
        // Reach squared: (0.6 + 0.6)^2 + 1.0 = 2.44. Distance squared: 1.0 <= 2.44.
        mob.tick(&world);

        assert_eq!(mob.target_id, Some(100));
        let target = world.get_entity(100).expect("Player must exist in world");
        assert_eq!(
            target.health, 16.0,
            "Player should have taken 4.0 damage, leaving 16.0 health"
        );
    }

    #[test]
    fn melee_attack_cooldown_enforcement_between_consecutive_attacks() {
        let world = WorldView::with_flat_ground(60);
        let player = EntityEntry::new(100, EntityKind::Player, Vec3::new(1.0, 61.0, 0.0))
            .with_dimensions(0.6, 1.8, 1.62)
            .with_health(20.0, 20.0);
        world.add_entity(player);

        let mut mob = MobEntity::new(1, EntityKind::Monster, Vec3::new(0.0, 61.0, 0.0))
            .with_dimensions(0.6, 1.8, 1.62);

        mob.target_id = Some(100);
        mob.add_goal(1, Box::new(MeleeAttackGoal::new(0.3, 4.0, 20)));

        // Tick 0: Initial attack delivers 4.0 damage (health 20.0 -> 16.0)
        mob.tick(&world);
        assert_eq!(world.get_entity(100).unwrap().health, 16.0);

        // Ticks 1..=19: On cooldown, NO additional damage should be dealt
        for _ in 1..20 {
            mob.tick(&world);
            assert_eq!(
                world.get_entity(100).unwrap().health,
                16.0,
                "No damage during cooldown"
            );
        }

        // Tick 20: Cooldown has elapsed, second attack delivers 4.0 damage (health 16.0 -> 12.0)
        mob.tick(&world);
        assert_eq!(
            world.get_entity(100).unwrap().health,
            12.0,
            "Second attack should deliver damage after exactly 20 ticks"
        );
    }

    #[test]
    fn melee_attack_goal_pursues_out_of_reach_target_then_attacks() {
        let world = WorldView::with_flat_ground(60);
        let player = EntityEntry::new(100, EntityKind::Player, Vec3::new(3.0, 61.0, 0.0))
            .with_dimensions(0.6, 1.8, 1.62)
            .with_health(20.0, 20.0);
        world.add_entity(player);

        let mut mob = MobEntity::new(1, EntityKind::Monster, Vec3::new(0.0, 61.0, 0.0))
            .with_dimensions(0.6, 1.8, 1.62);

        mob.target_id = Some(100);
        // Speed = 1.0 block/tick, damage = 5.0, cooldown = 20
        mob.add_goal(1, Box::new(MeleeAttackGoal::new(1.0, 5.0, 20)));

        // Tick 1: Out of reach (dist = 3.0, reach_sq = 2.44, reach ~ 1.56).
        // Mob navigates toward player at (3.0, 61.0, 0.0). Velocity = (1.0, 0.0, 0.0).
        mob.tick(&world);
        assert!(mob.velocity.x > 0.9);
        assert!((mob.position.x - 1.0).abs() < 1e-3);
        assert_eq!(world.get_entity(100).unwrap().health, 20.0);

        // Tick 2: Moves from 1.0 to 2.0. Dist was 2.0 > reach, so navigates again.
        mob.tick(&world);
        assert!((mob.position.x - 2.0).abs() < 1e-3);
        assert_eq!(world.get_entity(100).unwrap().health, 20.0);

        // Tick 3: At position 2.0, distance to target at 3.0 is 1.0 <= reach (~1.56).
        // Mob reaches player, stops moving, and attacks!
        mob.tick(&world);
        assert_eq!(
            world.get_entity(100).unwrap().health,
            15.0,
            "Target damaged upon reaching attack range"
        );
        assert_eq!(mob.velocity, Vec3::ZERO);
    }

    #[test]
    fn look_at_player_goal_stops_when_player_leaves_range() {
        let world = WorldView::new();
        let player = EntityEntry::new(10, EntityKind::Player, Vec3::new(4.0, 60.0, 0.0));
        world.add_entity(player);

        let mut mob = MobEntity::new(1, EntityKind::Monster, Vec3::new(0.0, 60.0, 0.0));
        let look_goal = LookAtPlayerGoal::new(8.0).with_duration(100, 100);
        mob.add_goal(1, Box::new(look_goal));

        // Start looking
        mob.tick(&world);
        assert_eq!(mob.goal_selector.running_count(), 1);

        // Player teleports far away (out of 8.0 block range)
        world.update_entity_position(10, Vec3::new(50.0, 60.0, 0.0));

        // Ticking again should stop the goal
        mob.tick(&world);
        assert_eq!(
            mob.goal_selector.running_count(),
            0,
            "Goal must stop when player is out of range"
        );
    }

    #[test]
    fn nearest_attackable_target_goal_clears_target_when_target_dies() {
        let world = WorldView::new();
        let player = EntityEntry::new(10, EntityKind::Player, Vec3::new(4.0, 60.0, 0.0))
            .with_health(5.0, 20.0);
        world.add_entity(player);

        let mut mob = MobEntity::new(1, EntityKind::Monster, Vec3::new(0.0, 60.0, 0.0));
        mob.add_target_goal(1, Box::new(NearestAttackableTargetGoal::for_player(16.0)));

        // Tick 1: Acquires alive player
        mob.tick(&world);
        assert_eq!(mob.target_id, Some(10));
        assert_eq!(mob.target_selector.running_count(), 1);

        // Kill player
        world.apply_damage(10, 10.0);
        assert_eq!(world.get_entity(10).unwrap().health, 0.0);

        // Tick 2: Target is dead, goal should stop and clear target
        mob.tick(&world);
        assert_eq!(
            mob.target_id, None,
            "Target ID must be cleared when target is no longer alive"
        );
        assert_eq!(mob.target_selector.running_count(), 0);
    }

    #[test]
    fn goal_flags_operations_and_properties() {
        let f1 = GoalFlags::MOVE;
        let f2 = GoalFlags::LOOK;
        let f3 = GoalFlags::MOVE | GoalFlags::LOOK;

        assert!(f3.contains(f1));
        assert!(f3.contains(f2));
        assert!(!f1.contains(f2));
        assert!(f1.intersects(f3));
        assert!(f2.intersects(f3));
        assert!(!f1.intersects(f2));
        assert_eq!(f1.union(f2), f3);
        assert_eq!(f3.difference(f1), f2);
        assert_eq!(GoalFlags::empty().bits(), 0);
    }
}
