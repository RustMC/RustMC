//! Dropped item entity physics, stack merging, and player pickup collection.
//!
//! Implements Milestone M5 dropped item mechanics:
//! - Item entities with unique IDs, AABB ($0.25 \times 0.25 \times 0.25$),
//!   age tracking (despawning at 6000 ticks = 5 minutes), and pickup delay.
//! - Deterministic motion physics: gravity acceleration ($v_y - 0.04$ blocks/tick),
//!   terminal downward velocity clamp ($v_y \ge -2.0$), airborne drag ($0.98$),
//!   and ground friction ($0.58$).
//! - Swept AABB collision resolution with world blocks (solid floor landing, wall sliding).
//! - Nearby stack merging within $1.0$ block Euclidean radius up to max stack size ($64$).
//! - Player contact pickup collection into inventory (filling matching stacks then empty slots),
//!   emitting collection events and `TakeItemEntity` packet payloads (ID 0x67).

use crate::world::{Block, CHUNK_SIDE, Chunk, WORLD_HEIGHT};
use std::collections::HashSet;
use std::fmt;

/// Maximum number of items in a single item stack.
pub const MAX_STACK_SIZE: u8 = 64;

/// Lifetime of a dropped item in ticks before despawning (5 minutes at 20 ticks/sec).
pub const DESPAWN_TICKS: u32 = 6000;

/// Default pickup delay when dropped by a player (40 ticks = 2 seconds).
pub const PLAYER_DROP_PICKUP_DELAY: u32 = 40;

/// Default pickup delay when dropped from a broken block (10 ticks = 0.5 seconds).
pub const BLOCK_BREAK_PICKUP_DELAY: u32 = 10;

/// Width and depth of dropped item entity bounding box in blocks.
pub const ITEM_WIDTH: f64 = 0.25;

/// Height of dropped item entity bounding box in blocks.
pub const ITEM_HEIGHT: f64 = 0.25;

/// Downward gravity acceleration applied to item entities per tick in blocks/tick.
pub const ITEM_GRAVITY: f64 = 0.04;

/// Terminal downward velocity clamp for item entities in blocks/tick.
pub const ITEM_TERMINAL_VELOCITY: f64 = -2.0;

/// Horizontal velocity damping factor per tick while airborne.
pub const AIR_DRAG_HORIZONTAL: f64 = 0.98;

/// Vertical velocity damping factor per tick while airborne.
pub const AIR_DRAG_VERTICAL: f64 = 0.98;

/// Horizontal velocity damping factor per tick while on the ground.
pub const GROUND_FRICTION: f64 = 0.58;

/// Maximum Euclidean distance for two item entities to merge stacks (1.0 block).
pub const STACK_MERGE_RADIUS: f64 = 1.0;

/// Total number of player inventory slots (9 hotbar + 27 main inventory).
pub const TOTAL_SLOT_COUNT: usize = 36;

/// Number of hotbar slots (slots 0..=8).
pub const HOTBAR_SLOT_COUNT: usize = 9;

/// Number of main inventory slots (slots 9..=35).
pub const MAIN_SLOT_COUNT: usize = 27;

/// Java Edition clientbound play packet ID for Take Item Entity (0x67 in 26.3).
pub const TAKE_ITEM_ENTITY_PACKET_ID: u32 = 0x67;

/// Tolerance threshold for floating-point collision tests.
const EPSILON: f64 = 1e-7;

/// Velocity threshold below which near-zero speeds are clamped to zero.
const VELOCITY_REST_THRESHOLD: f64 = 1e-6;

/// Errors returned by inventory transactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryError {
    /// Slot index is outside the valid inventory range `0..=35`.
    InvalidSlot(u8),
    /// Slot index is outside the valid hotbar range `0..=8`.
    InvalidHotbarSlot(u8),
    /// Item count is invalid (zero or exceeds max stack size).
    InvalidCount(u8),
    /// Item identifier is empty or whitespace-only.
    InvalidItem,
    /// Held slot does not contain enough items.
    InsufficientItems { requested: u8, available: u8 },
    /// Inventory has no available space to accept items.
    InventoryFull,
}

impl fmt::Display for InventoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlot(slot) => write!(f, "inventory slot {slot} is out of bounds (0..=35)"),
            Self::InvalidHotbarSlot(slot) => {
                write!(f, "hotbar slot {slot} is out of bounds (0..=8)")
            }
            Self::InvalidCount(count) => write!(f, "invalid item count {count}"),
            Self::InvalidItem => write!(f, "item identifier cannot be empty"),
            Self::InsufficientItems {
                requested,
                available,
            } => {
                write!(
                    f,
                    "insufficient items: requested {requested}, but only {available} available"
                )
            }
            Self::InventoryFull => write!(f, "inventory has no available space"),
        }
    }
}

impl std::error::Error for InventoryError {}

/// A counted stack of items bounded by `MAX_STACK_SIZE` (64).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStack {
    item: String,
    count: u8,
}

impl ItemStack {
    /// Create a new item stack with validation.
    pub fn new(item: impl Into<String>, count: u8) -> Result<Self, InventoryError> {
        let item = item.into();
        if item.trim().is_empty() {
            return Err(InventoryError::InvalidItem);
        }
        if count == 0 || count > MAX_STACK_SIZE {
            return Err(InventoryError::InvalidCount(count));
        }
        Ok(Self { item, count })
    }

    /// Construct an item stack without returning a Result, clamping count to `0..=MAX_STACK_SIZE`.
    pub fn new_unchecked(item: impl Into<String>, count: u8) -> Self {
        Self {
            item: item.into(),
            count: count.clamp(0, MAX_STACK_SIZE),
        }
    }

    /// The item identifier (e.g. `"minecraft:diamond"`).
    #[inline]
    pub fn item(&self) -> &str {
        &self.item
    }

    /// The item count, within `0..=MAX_STACK_SIZE`.
    #[inline]
    pub const fn count(&self) -> u8 {
        self.count
    }

    /// Update the item count, clamping to `MAX_STACK_SIZE`.
    #[inline]
    pub fn set_count(&mut self, count: u8) {
        self.count = count.min(MAX_STACK_SIZE);
    }

    /// Maximum number of items this stack can hold (64).
    #[inline]
    pub const fn max_stack_size(&self) -> u8 {
        MAX_STACK_SIZE
    }

    /// Whether this stack has reached count 0.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
}

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

    /// Construct an item entity bounding box ($0.25 \times 0.25 \times 0.25$).
    #[inline]
    pub fn from_item_pos(pos: [f64; 3]) -> Self {
        Self::from_entity_pos(pos, ITEM_WIDTH, ITEM_HEIGHT)
    }

    /// Construct a standard player entity bounding box ($0.6 \times 1.8$).
    #[inline]
    pub fn from_player_pos(pos: [f64; 3]) -> Self {
        Self::from_entity_pos(pos, 0.6, 1.8)
    }

    /// Construct a full 1x1x1 cube bounding box for a voxel at integer coordinates `(x, y, z)`.
    #[inline]
    pub fn from_block(x: i64, y: i64, z: i64) -> Self {
        Self {
            min: [x as f64, y as f64, z as f64],
            max: [x as f64 + 1.0, y as f64 + 1.0, z as f64 + 1.0],
        }
    }

    /// Check if this bounding box strictly overlaps another bounding box.
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

    /// Inflate the bounding box outward by `(dx, dy, dz)`.
    #[inline]
    pub fn inflate(&self, dx: f64, dy: f64, dz: f64) -> Self {
        Self {
            min: [self.min[0] - dx, self.min[1] - dy, self.min[2] - dz],
            max: [self.max[0] + dx, self.max[1] + dy, self.max[2] + dz],
        }
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

/// Trait for querying obstacle bounding boxes for collision sweeping.
pub trait WorldCollisionView {
    /// Retrieve all solid bounding boxes intersecting the query box.
    fn get_colliding_boxes(&self, aabb: &Aabb) -> Vec<Aabb>;
}

impl WorldCollisionView for () {
    fn get_colliding_boxes(&self, _aabb: &Aabb) -> Vec<Aabb> {
        Vec::new()
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

impl WorldCollisionView for Vec<Aabb> {
    fn get_colliding_boxes(&self, aabb: &Aabb) -> Vec<Aabb> {
        self.as_slice().get_colliding_boxes(aabb)
    }
}

impl<T: WorldCollisionView + ?Sized> WorldCollisionView for &T {
    fn get_colliding_boxes(&self, aabb: &Aabb) -> Vec<Aabb> {
        (**self).get_colliding_boxes(aabb)
    }
}

/// Helper function to extract voxel collision boxes where `is_solid` returns true.
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

/// A simple in-memory voxel world suitable for physics testing and custom scenarios.
#[derive(Debug, Default, Clone)]
pub struct SimpleVoxelWorld {
    solid_blocks: HashSet<(i64, i64, i64)>,
    custom_boxes: Vec<Aabb>,
}

impl SimpleVoxelWorld {
    /// Create an empty voxel world.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a solid 1x1x1 cube block at integer coordinates `(x, y, z)`.
    pub fn add_solid_block(&mut self, x: i64, y: i64, z: i64) {
        self.solid_blocks.insert((x, y, z));
    }

    /// Add a custom collision box (e.g. slabs or stairs).
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

/// Swept axis collision resolution for item entities.
///
/// Sweeps Y, then X, then Z against obstacles.
/// Returns `(resolved_displacement, on_ground, collided_horizontal, hit_ceiling)`.
pub fn collide_and_sweep(
    start_box: &Aabb,
    velocity: [f64; 3],
    colliding_boxes: &[Aabb],
) -> ([f64; 3], bool, bool, bool) {
    // 1. Sweep Y axis
    let mut resolved_dy = velocity[1];
    for b in colliding_boxes {
        resolved_dy = start_box.calculate_y_offset(b, resolved_dy);
    }
    let box_y = start_box.offset(0.0, resolved_dy, 0.0);
    let hit_floor = (resolved_dy - velocity[1]).abs() > EPSILON && velocity[1] < 0.0;
    let hit_ceiling = (resolved_dy - velocity[1]).abs() > EPSILON && velocity[1] > 0.0;

    // 2. Sweep X axis
    let mut resolved_dx = velocity[0];
    for b in colliding_boxes {
        resolved_dx = box_y.calculate_x_offset(b, resolved_dx);
    }
    let box_yx = box_y.offset(resolved_dx, 0.0, 0.0);
    let hit_x = (resolved_dx - velocity[0]).abs() > EPSILON;

    // 3. Sweep Z axis
    let mut resolved_dz = velocity[2];
    for b in colliding_boxes {
        resolved_dz = box_yx.calculate_z_offset(b, resolved_dz);
    }
    let hit_z = (resolved_dz - velocity[2]).abs() > EPSILON;

    let on_ground = hit_floor;
    let collided_horizontal = hit_x || hit_z;

    (
        [resolved_dx, resolved_dy, resolved_dz],
        on_ground,
        collided_horizontal,
        hit_ceiling,
    )
}

/// Player inventory managing 36 slots (9 hotbar + 27 main inventory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInventory {
    slots: [Option<ItemStack>; TOTAL_SLOT_COUNT],
    held_slot: u8,
    revision: u64,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerInventory {
    /// Construct an empty player inventory.
    pub const fn new() -> Self {
        Self {
            slots: [const { None }; TOTAL_SLOT_COUNT],
            held_slot: 0,
            revision: 0,
        }
    }

    /// The currently active held hotbar slot (`0..=8`).
    pub const fn held_slot(&self) -> u8 {
        self.held_slot
    }

    /// Select an active hotbar slot (`0..=8`).
    pub fn set_held_slot(&mut self, slot: u8) -> Result<(), InventoryError> {
        if slot >= HOTBAR_SLOT_COUNT as u8 {
            return Err(InventoryError::InvalidHotbarSlot(slot));
        }
        self.held_slot = slot;
        Ok(())
    }

    /// Monotonic revision counter tracking inventory mutations.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Reference to all 36 slots.
    pub fn slots(&self) -> &[Option<ItemStack>] {
        &self.slots
    }

    /// Mutable reference to all 36 slots.
    pub fn slots_mut(&mut self) -> &mut [Option<ItemStack>] {
        &mut self.slots
    }

    /// Reference to a specific slot if valid.
    pub fn get_slot(&self, slot: u8) -> Option<&ItemStack> {
        if (slot as usize) < TOTAL_SLOT_COUNT {
            self.slots[slot as usize].as_ref()
        } else {
            None
        }
    }

    /// Assign an item stack directly to a specific slot.
    pub fn set_slot(&mut self, slot: u8, stack: Option<ItemStack>) -> Result<(), InventoryError> {
        if (slot as usize) >= TOTAL_SLOT_COUNT {
            return Err(InventoryError::InvalidSlot(slot));
        }
        self.slots[slot as usize] = stack;
        self.revision = self
            .revision
            .checked_add(1)
            .expect("inventory revision overflow");
        Ok(())
    }

    /// Attempt to insert an item stack into inventory slots following official rules:
    /// 1. Top up existing matching partial stacks (`count < 64`) across slots 0..36.
    /// 2. Place remainder into empty slots across slots 0..36.
    ///
    /// Returns the number of items successfully accepted by the inventory.
    pub fn insert_stack(&mut self, stack: &ItemStack) -> u8 {
        let mut remaining = stack.count();
        let item_name = stack.item();

        // Pass 1: top up compatible partial stacks
        for slot in self.slots.iter_mut() {
            if remaining == 0 {
                break;
            }
            if let Some(existing) = slot
                && existing.item() == item_name
                && existing.count() < MAX_STACK_SIZE
            {
                let space = MAX_STACK_SIZE - existing.count();
                let to_add = remaining.min(space);
                existing.set_count(existing.count() + to_add);
                remaining -= to_add;
            }
        }

        // Pass 2: fill empty slots
        for slot in self.slots.iter_mut() {
            if remaining == 0 {
                break;
            }
            if slot.is_none() {
                let to_add = remaining.min(MAX_STACK_SIZE);
                *slot = Some(ItemStack::new_unchecked(item_name, to_add));
                remaining -= to_add;
            }
        }

        let collected = stack.count() - remaining;
        if collected > 0 {
            self.revision = self
                .revision
                .checked_add(1)
                .expect("inventory revision overflow");
        }
        collected
    }

    /// Transactional method to grant items, returning the unplaced remainder count or `InventoryFull`.
    pub fn grant_item(&mut self, item: String, count: u8) -> Result<u8, InventoryError> {
        if item.trim().is_empty() {
            return Err(InventoryError::InvalidItem);
        }
        if count == 0 {
            return Err(InventoryError::InvalidCount(0));
        }

        let stack = ItemStack::new(item, count)?;
        let placed = self.insert_stack(&stack);
        if placed == 0 {
            return Err(InventoryError::InventoryFull);
        }
        Ok(count - placed)
    }

    /// Total count of matching items held across all slots.
    pub fn total_count_of(&self, item: &str) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.item() == item)
            .map(|s| s.count() as u32)
            .sum()
    }

    /// Check if all 36 inventory slots are currently empty.
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.is_none())
    }
}

/// Java Edition clientbound `TakeItemEntity` packet payload (packet ID 0x67).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeItemEntityPacket {
    pub collected_entity_id: u32,
    pub collector_entity_id: u32,
    pub pickup_item_count: u32,
}

impl TakeItemEntityPacket {
    /// Construct a new `TakeItemEntityPacket`.
    pub const fn new(
        collected_entity_id: u32,
        collector_entity_id: u32,
        pickup_item_count: u32,
    ) -> Self {
        Self {
            collected_entity_id,
            collector_entity_id,
            pickup_item_count,
        }
    }

    /// Encode the body payload (without length prefix or packet ID framing).
    pub fn encode_payload(&self) -> Vec<u8> {
        let mut body = Vec::new();
        put_varint(self.collected_entity_id, &mut body);
        put_varint(self.collector_entity_id, &mut body);
        put_varint(self.pickup_item_count, &mut body);
        body
    }

    /// Encode the fully framed packet: `[VarInt length] [VarInt 0x67] [payload]`.
    pub fn encode_packet(&self) -> Vec<u8> {
        let body = self.encode_payload();
        let mut packet = Vec::new();
        put_varint(TAKE_ITEM_ENTITY_PACKET_ID, &mut packet);
        packet.extend_from_slice(&body);
        let mut out = Vec::new();
        put_varint(packet.len() as u32, &mut out);
        out.extend(packet);
        out
    }

    /// Decode the packet payload from a slice of bytes.
    pub fn decode_payload(input: &[u8]) -> Result<Self, String> {
        let (collected_entity_id, rest) = read_varint(input)?;
        let (collector_entity_id, rest) = read_varint(rest)?;
        let (pickup_item_count, rest) = read_varint(rest)?;
        if !rest.is_empty() {
            return Err("trailing bytes in TakeItemEntity packet payload".to_string());
        }
        Ok(Self {
            collected_entity_id,
            collector_entity_id,
            pickup_item_count,
        })
    }
}

/// Event emitted when an item is successfully picked up by a player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemPickupEvent {
    pub collected_entity_id: u32,
    pub collector_entity_id: u32,
    pub item: String,
    pub count: u8,
    pub packet: TakeItemEntityPacket,
}

/// Result returned from an attempted item collection by a player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemPickupResult {
    pub collected_count: u8,
    pub remaining_count: u8,
    pub event: Option<ItemPickupEvent>,
}

impl ItemPickupResult {
    /// Helper for no collection occurring.
    pub const fn none(remaining_count: u8) -> Self {
        Self {
            collected_count: 0,
            remaining_count,
            event: None,
        }
    }

    /// Whether any items were successfully collected.
    pub const fn is_collected(&self) -> bool {
        self.collected_count > 0
    }
}

/// A dropped item entity floating in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemEntity {
    pub id: u32,
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub stack: ItemStack,
    pub age: u32,
    pub pickup_delay: u32,
    pub on_ground: bool,
    dead: bool,
}

impl ItemEntity {
    /// Construct a new `ItemEntity` at the given position with zero initial velocity and delay.
    pub fn new(id: u32, position: [f64; 3], stack: ItemStack) -> Self {
        Self {
            id,
            position,
            velocity: [0.0, 0.0, 0.0],
            stack,
            age: 0,
            pickup_delay: 0,
            on_ground: false,
            dead: false,
        }
    }

    /// Construct an `ItemEntity` dropped by a player (pickup delay 40 ticks).
    pub fn dropped_by_player(
        id: u32,
        position: [f64; 3],
        velocity: [f64; 3],
        stack: ItemStack,
    ) -> Self {
        Self {
            id,
            position,
            velocity,
            stack,
            age: 0,
            pickup_delay: PLAYER_DROP_PICKUP_DELAY,
            on_ground: false,
            dead: false,
        }
    }

    /// Construct an `ItemEntity` dropped from a broken block (pickup delay 10 ticks).
    pub fn dropped_by_block_break(
        id: u32,
        position: [f64; 3],
        velocity: [f64; 3],
        stack: ItemStack,
    ) -> Self {
        Self {
            id,
            position,
            velocity,
            stack,
            age: 0,
            pickup_delay: BLOCK_BREAK_PICKUP_DELAY,
            on_ground: false,
            dead: false,
        }
    }

    /// Set initial velocity.
    pub const fn with_velocity(mut self, velocity: [f64; 3]) -> Self {
        self.velocity = velocity;
        self
    }

    /// Set initial pickup delay in ticks.
    pub const fn with_pickup_delay(mut self, pickup_delay: u32) -> Self {
        self.pickup_delay = pickup_delay;
        self
    }

    /// Set initial age in ticks.
    pub const fn with_age(mut self, age: u32) -> Self {
        self.age = age;
        self
    }

    /// Unique entity ID.
    #[inline]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// Current entity position `[x, y, z]`.
    #[inline]
    pub const fn position(&self) -> [f64; 3] {
        self.position
    }

    /// Current entity velocity `[vx, vy, vz]`.
    #[inline]
    pub const fn velocity(&self) -> [f64; 3] {
        self.velocity
    }

    /// Reference to the underlying item stack.
    #[inline]
    pub const fn stack(&self) -> &ItemStack {
        &self.stack
    }

    /// Number of items remaining in this entity's stack.
    #[inline]
    pub const fn count(&self) -> u8 {
        self.stack.count()
    }

    /// Entity age in ticks.
    #[inline]
    pub const fn age(&self) -> u32 {
        self.age
    }

    /// Pickup delay remaining in ticks.
    #[inline]
    pub const fn pickup_delay(&self) -> u32 {
        self.pickup_delay
    }

    /// Whether this entity is resting on the ground.
    #[inline]
    pub const fn on_ground(&self) -> bool {
        self.on_ground
    }

    /// Whether this entity is alive (not despawned, not marked dead, and stack count > 0).
    #[inline]
    pub fn is_alive(&self) -> bool {
        !self.dead && self.age < DESPAWN_TICKS && !self.stack.is_empty()
    }

    /// Whether this entity has been removed, despawned, or depleted.
    #[inline]
    pub fn is_dead(&self) -> bool {
        !self.is_alive()
    }

    /// Mark this entity as dead/removed.
    #[inline]
    pub fn mark_dead(&mut self) {
        self.dead = true;
    }

    /// Whether this entity can currently be picked up (`pickup_delay == 0` and alive).
    #[inline]
    pub fn can_pickup(&self) -> bool {
        self.is_alive() && self.pickup_delay == 0
    }

    /// The $0.25 \times 0.25 \times 0.25$ bounding box centered at entity position.
    #[inline]
    pub fn aabb(&self) -> Aabb {
        Aabb::from_item_pos(self.position)
    }

    /// Euclidean distance to another item entity.
    pub fn distance_to(&self, other: &ItemEntity) -> f64 {
        let dx = self.position[0] - other.position[0];
        let dy = self.position[1] - other.position[1];
        let dz = self.position[2] - other.position[2];
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    /// Euclidean squared distance to another item entity.
    pub fn distance_sq_to(&self, other: &ItemEntity) -> f64 {
        let dx = self.position[0] - other.position[0];
        let dy = self.position[1] - other.position[1];
        let dz = self.position[2] - other.position[2];
        dx * dx + dy * dy + dz * dz
    }

    /// Advance physics motion and collision sweep by one tick.
    pub fn tick_physics<W: WorldCollisionView>(&mut self, world: &W) {
        if !self.is_alive() {
            return;
        }

        // 1. Gravity acceleration and terminal velocity clamp
        self.velocity[1] -= ITEM_GRAVITY;
        if self.velocity[1] < ITEM_TERMINAL_VELOCITY {
            self.velocity[1] = ITEM_TERMINAL_VELOCITY;
        }

        // 2. Query colliding boxes for sweep
        let start_box = self.aabb();
        let query_box = start_box
            .expand(self.velocity[0], self.velocity[1], self.velocity[2])
            .inflate(0.1, 0.1, 0.1);
        let colliding_boxes = world.get_colliding_boxes(&query_box);

        // 3. Collision sweep along Y, X, Z
        let (resolved_delta, on_ground, collided_horiz, hit_ceiling) =
            collide_and_sweep(&start_box, self.velocity, &colliding_boxes);

        // 4. Update position
        self.position[0] += resolved_delta[0];
        self.position[1] += resolved_delta[1];
        self.position[2] += resolved_delta[2];

        // 5. Update ground contact and zero velocity on obstacle impacts
        self.on_ground = on_ground;

        if on_ground || hit_ceiling {
            self.velocity[1] = 0.0;
        }
        if collided_horiz {
            if (resolved_delta[0] - self.velocity[0]).abs() > EPSILON {
                self.velocity[0] = 0.0;
            }
            if (resolved_delta[2] - self.velocity[2]).abs() > EPSILON {
                self.velocity[2] = 0.0;
            }
        }

        // 6. Apply drag / friction
        if self.on_ground {
            self.velocity[0] *= GROUND_FRICTION;
            self.velocity[2] *= GROUND_FRICTION;
            self.velocity[1] *= AIR_DRAG_VERTICAL;
        } else {
            self.velocity[0] *= AIR_DRAG_HORIZONTAL;
            self.velocity[1] *= AIR_DRAG_VERTICAL;
            self.velocity[2] *= AIR_DRAG_HORIZONTAL;
        }

        // Rest clamp for tiny sub-epsilon speeds
        if self.velocity[0].abs() < VELOCITY_REST_THRESHOLD {
            self.velocity[0] = 0.0;
        }
        if self.velocity[1].abs() < VELOCITY_REST_THRESHOLD {
            self.velocity[1] = 0.0;
        }
        if self.velocity[2].abs() < VELOCITY_REST_THRESHOLD {
            self.velocity[2] = 0.0;
        }
    }

    /// Advance age and decrement pickup delay by one tick.
    pub fn tick_lifecycle(&mut self) {
        if !self.is_alive() {
            return;
        }
        self.age = self.age.saturating_add(1);
        if self.age >= DESPAWN_TICKS {
            self.mark_dead();
        }
        if self.pickup_delay > 0 {
            self.pickup_delay -= 1;
        }
    }

    /// Execute a complete simulation tick: physics sweep followed by lifecycle progression.
    pub fn tick<W: WorldCollisionView>(&mut self, world: &W) {
        self.tick_physics(world);
        self.tick_lifecycle();
    }

    /// Check if another item entity can merge into `self`.
    ///
    /// Conditions:
    /// - Both entities are alive.
    /// - Different entity IDs.
    /// - Euclidean distance $\le 1.0$ block.
    /// - Item types match.
    /// - Combined stack count $\le 64$ (or item's max stack size).
    pub fn can_merge_with(&self, other: &ItemEntity) -> bool {
        if !self.is_alive() || !other.is_alive() {
            return false;
        }
        if self.id == other.id {
            return false;
        }
        if self.stack.item() != other.stack.item() {
            return false;
        }
        if self.distance_sq_to(other) > STACK_MERGE_RADIUS * STACK_MERGE_RADIUS {
            return false;
        }
        let combined = self.stack.count() as u16 + other.stack.count() as u16;
        if combined > self.stack.max_stack_size() as u16 {
            return false;
        }
        true
    }

    /// Attempt to merge `other` into `self`.
    ///
    /// On success:
    /// - `self` receives the sum of both counts.
    /// - `other` has its count set to 0 and is marked dead.
    /// - Age is reset to `min(age1, age2)`.
    /// - Pickup delay is reset to `max(delay1, delay2)`.
    /// - Returns `true`.
    pub fn try_merge(&mut self, other: &mut ItemEntity) -> bool {
        if !self.can_merge_with(other) {
            return false;
        }
        let combined = self.stack.count() + other.stack.count();
        self.stack.set_count(combined);
        other.stack.set_count(0);
        other.mark_dead();
        self.age = self.age.min(other.age);
        self.pickup_delay = self.pickup_delay.max(other.pickup_delay);
        true
    }

    /// Attempt player pickup using a player's axis-aligned bounding box.
    pub fn try_collect(
        &mut self,
        player_id: u32,
        player_box: &Aabb,
        inventory: &mut PlayerInventory,
    ) -> ItemPickupResult {
        if !self.can_pickup() {
            return ItemPickupResult::none(self.count());
        }
        if !player_box.intersects(&self.aabb()) {
            return ItemPickupResult::none(self.count());
        }

        let collected = inventory.insert_stack(&self.stack);
        if collected == 0 {
            return ItemPickupResult::none(self.count());
        }

        let remaining = self.stack.count() - collected;
        self.stack.set_count(remaining);
        if remaining == 0 {
            self.mark_dead();
        }

        let packet = TakeItemEntityPacket::new(self.id, player_id, collected as u32);
        let event = ItemPickupEvent {
            collected_entity_id: self.id,
            collector_entity_id: player_id,
            item: self.stack.item().to_string(),
            count: collected,
            packet,
        };

        ItemPickupResult {
            collected_count: collected,
            remaining_count: remaining,
            event: Some(event),
        }
    }

    /// Attempt player pickup using player world coordinates (constructs $0.6 \times 1.8$ player AABB).
    pub fn try_collect_at(
        &mut self,
        player_id: u32,
        player_pos: [f64; 3],
        inventory: &mut PlayerInventory,
    ) -> ItemPickupResult {
        let player_box = Aabb::from_player_pos(player_pos);
        self.try_collect(player_id, &player_box, inventory)
    }
}

/// Merge all nearby matching item entities in a slice within $1.0$ block radius up to max stack size.
///
/// Returns the number of successful merges performed.
pub fn merge_nearby_stacks(items: &mut [ItemEntity]) -> usize {
    let mut merges = 0;
    let len = items.len();
    for i in 0..len {
        if !items[i].is_alive() {
            continue;
        }
        for j in (i + 1)..len {
            if !items[i].is_alive() {
                break;
            }
            if !items[j].is_alive() {
                continue;
            }
            let (left, right) = items.split_at_mut(j);
            let item_i = &mut left[i];
            let item_j = &mut right[0];
            if item_i.try_merge(item_j) {
                merges += 1;
            }
        }
    }
    merges
}

/// World manager container coordinating item physics, stack merging, and pickup events.
#[derive(Debug, Default, Clone)]
pub struct ItemEntityManager {
    items: Vec<ItemEntity>,
}

impl ItemEntityManager {
    /// Create an empty `ItemEntityManager`.
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Spawn an item entity into the world.
    pub fn spawn(&mut self, item: ItemEntity) {
        self.items.push(item);
    }

    /// Slice of tracked item entities.
    pub fn items(&self) -> &[ItemEntity] {
        &self.items
    }

    /// Mutable slice of tracked item entities.
    pub fn items_mut(&mut self) -> &mut [ItemEntity] {
        &mut self.items
    }

    /// Total number of tracked item entities.
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// Whether there are zero tracked item entities.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Advance the simulation by one tick:
    /// 1. Ticks physics and lifecycle for all items.
    /// 2. Merges nearby compatible stacks.
    /// 3. Removes despawned and dead entities.
    pub fn tick<W: WorldCollisionView>(&mut self, world: &W) {
        for item in &mut self.items {
            item.tick(world);
        }
        merge_nearby_stacks(&mut self.items);
        self.items.retain(|item| item.is_alive());
    }

    /// Attempt player pickup for all items intersecting the player bounding box.
    pub fn handle_player_pickup(
        &mut self,
        player_id: u32,
        player_pos: [f64; 3],
        inventory: &mut PlayerInventory,
    ) -> Vec<ItemPickupEvent> {
        let player_box = Aabb::from_player_pos(player_pos);
        let mut events = Vec::new();
        for item in &mut self.items {
            if !item.is_alive() {
                continue;
            }
            let result = item.try_collect(player_id, &player_box, inventory);
            if let Some(event) = result.event {
                events.push(event);
            }
        }
        self.items.retain(|item| item.is_alive());
        events
    }
}

/// Encode a `u32` value as a variable-length quantity (VarInt) into a byte buffer.
pub fn put_varint(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Read a VarInt from a slice of bytes, returning `(value, remaining_slice)`.
pub fn read_varint(mut input: &[u8]) -> Result<(u32, &[u8]), String> {
    let mut value: u32 = 0;
    let mut position = 0;
    while position < 35 {
        if input.is_empty() {
            return Err("unexpected EOF while reading VarInt".to_string());
        }
        let byte = input[0];
        input = &input[1..];
        value |= ((byte & 0x7f) as u32) << position;
        if (byte & 0x80) == 0 {
            return Ok((value, input));
        }
        position += 7;
    }
    Err("VarInt too wide".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_freefall_gravity_and_terminal_velocity() {
        let stack = ItemStack::new("minecraft:dirt", 1).unwrap();
        let mut item = ItemEntity::new(1, [0.0, 100.0, 0.0], stack);
        assert_eq!(item.velocity(), [0.0, 0.0, 0.0]);

        // Tick 1 in freefall: gravity -0.04, then air drag * 0.98
        item.tick(&());
        let expected_vy = (0.0 - ITEM_GRAVITY) * AIR_DRAG_VERTICAL;
        assert!((item.velocity()[1] - expected_vy).abs() < 1e-6);
        assert!(!item.on_ground());
        assert!(item.position()[1] < 100.0);

        // Tick 2: downward speed increases further
        let prev_vy = item.velocity()[1];
        item.tick(&());
        assert!(item.velocity()[1] < prev_vy);

        // Terminal velocity clamp: large negative velocity cannot exceed -2.0 blocks/tick
        let stack2 = ItemStack::new("minecraft:dirt", 1).unwrap();
        let mut fast_item =
            ItemEntity::new(2, [0.0, 100.0, 0.0], stack2).with_velocity([0.0, -10.0, 0.0]);
        fast_item.tick(&());
        // Clamped to -2.0, then multiplied by 0.98 = -1.96
        assert!(fast_item.velocity()[1] >= -2.0);
        assert_eq!(fast_item.velocity()[1], -2.0 * AIR_DRAG_VERTICAL);

        // After 200 ticks of freefall, vertical velocity never drops below -2.0
        for _ in 0..200 {
            item.tick(&());
            assert!(item.velocity()[1] >= -2.0);
        }
    }

    #[test]
    fn test_landing_on_solid_floor_and_friction() {
        let mut world = SimpleVoxelWorld::new();
        // Floor blocks at y = 0
        for x in -2..=2 {
            for z in -2..=2 {
                world.add_solid_block(x, 0, z);
            }
        }

        // Top surface of floor is at y = 1.0. Item starts at y = 1.5 falling downwards.
        let stack = ItemStack::new("minecraft:iron_ingot", 4).unwrap();
        let mut item = ItemEntity::new(1, [0.0, 1.5, 0.0], stack).with_velocity([1.0, -0.6, 0.0]); // moving forward in X while falling

        // Tick 1: reaches floor top at y = 1.0
        item.tick(&world);
        assert!(item.on_ground(), "Item should land on top of the block");
        assert_eq!(item.position()[1], 1.0);
        assert_eq!(item.velocity()[1], 0.0);

        // Ground friction dampens horizontal velocity by 0.58
        let vx_after_landing = item.velocity()[0];
        assert!(
            (vx_after_landing - (1.0 * GROUND_FRICTION)).abs() < 1e-5,
            "Expected ground friction 0.58"
        );

        // Tick 2: remains resting on the floor
        item.tick(&world);
        assert!(item.on_ground());
        assert_eq!(item.position()[1], 1.0);
        assert_eq!(item.velocity()[1], 0.0);
        assert!(item.velocity()[0] < vx_after_landing);
    }

    #[test]
    fn test_stack_merging_within_radius_up_to_64() {
        let s1 = ItemStack::new("minecraft:dirt", 25).unwrap();
        let s2 = ItemStack::new("minecraft:dirt", 35).unwrap();
        let mut item1 = ItemEntity::new(1, [0.0, 10.0, 0.0], s1)
            .with_age(100)
            .with_pickup_delay(5);
        let mut item2 = ItemEntity::new(2, [0.5, 10.0, 0.0], s2)
            .with_age(50)
            .with_pickup_delay(15);

        assert!(item1.can_merge_with(&item2));
        assert!(item1.try_merge(&mut item2));

        // Combined count is 60 (25 + 35)
        assert_eq!(item1.count(), 60);
        assert_eq!(item1.age(), 50); // min(100, 50)
        assert_eq!(item1.pickup_delay(), 15); // max(5, 15)
        assert!(item1.is_alive());

        // Merged item is dead and count 0
        assert_eq!(item2.count(), 0);
        assert!(item2.is_dead());

        // Exact 64 merge
        let s3 = ItemStack::new("minecraft:dirt", 4).unwrap();
        let mut item3 = ItemEntity::new(3, [0.2, 10.0, 0.0], s3);
        assert!(item1.try_merge(&mut item3));
        assert_eq!(item1.count(), 64);
        assert!(item3.is_dead());
    }

    #[test]
    fn test_rejection_of_stack_merging() {
        // 1. Distance > 1.0 block (1.1 blocks apart)
        let s1 = ItemStack::new("minecraft:dirt", 10).unwrap();
        let s2 = ItemStack::new("minecraft:dirt", 10).unwrap();
        let mut item1 = ItemEntity::new(1, [0.0, 10.0, 0.0], s1);
        let mut item2 = ItemEntity::new(2, [1.1, 10.0, 0.0], s2);
        assert!(!item1.can_merge_with(&item2));
        assert!(!item1.try_merge(&mut item2));
        assert_eq!(item1.count(), 10);
        assert_eq!(item2.count(), 10);

        // 2. Different item types
        let s3 = ItemStack::new("minecraft:stone", 10).unwrap();
        let mut item3 = ItemEntity::new(3, [0.2, 10.0, 0.0], s3);
        assert!(!item1.can_merge_with(&item3));
        assert!(!item1.try_merge(&mut item3));

        // 3. Combined count exceeds max stack size (40 + 30 = 70 > 64)
        let s4 = ItemStack::new("minecraft:dirt", 40).unwrap();
        let s5 = ItemStack::new("minecraft:dirt", 30).unwrap();
        let mut item4 = ItemEntity::new(4, [0.0, 10.0, 0.0], s4);
        let mut item5 = ItemEntity::new(5, [0.2, 10.0, 0.0], s5);
        assert!(!item4.can_merge_with(&item5));
        assert!(!item4.try_merge(&mut item5));
        assert_eq!(item4.count(), 40);
        assert_eq!(item5.count(), 30);

        // 4. Dead item cannot merge
        let s6 = ItemStack::new("minecraft:dirt", 5).unwrap();
        let mut item6 = ItemEntity::new(6, [0.1, 10.0, 0.0], s6);
        item6.mark_dead();
        assert!(!item1.can_merge_with(&item6));

        // 5. Self-merge rejected
        assert!(!item1.can_merge_with(&item1));
    }

    #[test]
    fn test_pickup_delay_guard() {
        let mut inv = PlayerInventory::new();
        let stack = ItemStack::new("minecraft:gold_ingot", 8).unwrap();
        let mut item = ItemEntity::dropped_by_player(1, [0.0, 10.0, 0.0], [0.0, 0.0, 0.0], stack);

        assert_eq!(item.pickup_delay(), PLAYER_DROP_PICKUP_DELAY);
        assert!(!item.can_pickup());

        // Player collides with item at exact position
        let player_pos = [0.0, 10.0, 0.0];
        let res = item.try_collect_at(100, player_pos, &mut inv);
        assert_eq!(res.collected_count, 0);
        assert_eq!(res.remaining_count, 8);
        assert!(res.event.is_none());
        assert_eq!(inv.total_count_of("minecraft:gold_ingot"), 0);

        // Tick down pickup delay to 0
        for _ in 0..PLAYER_DROP_PICKUP_DELAY {
            item.tick_lifecycle();
        }
        assert_eq!(item.pickup_delay(), 0);
        assert!(item.can_pickup());

        // Now pickup succeeds
        let res = item.try_collect_at(100, player_pos, &mut inv);
        assert_eq!(res.collected_count, 8);
        assert_eq!(res.remaining_count, 0);
        assert!(item.is_dead());
        assert_eq!(inv.total_count_of("minecraft:gold_ingot"), 8);
    }

    #[test]
    fn test_player_pickup_collection_full_transfer() {
        let mut inv = PlayerInventory::new();
        let stack = ItemStack::new("minecraft:diamond", 16).unwrap();
        let mut item = ItemEntity::new(1, [5.0, 64.0, 5.0], stack);

        let player_pos = [5.1, 64.0, 5.0];
        let res = item.try_collect_at(42, player_pos, &mut inv);

        assert_eq!(res.collected_count, 16);
        assert_eq!(res.remaining_count, 0);
        assert!(item.is_dead());
        assert_eq!(inv.get_slot(0).unwrap().count(), 16);
        assert_eq!(inv.get_slot(0).unwrap().item(), "minecraft:diamond");

        // Verify emitted packet
        let event = res.event.expect("Expected pickup event");
        assert_eq!(event.collected_entity_id, 1);
        assert_eq!(event.collector_entity_id, 42);
        assert_eq!(event.count, 16);
        assert_eq!(event.packet.collected_entity_id, 1);
        assert_eq!(event.packet.collector_entity_id, 42);
        assert_eq!(event.packet.pickup_item_count, 16);
    }

    #[test]
    fn test_player_pickup_collection_partial_transfer() {
        let mut inv = PlayerInventory::new();
        // Fill slots 0..35 with full stacks of cobblestone, except slot 35 with 60 diamonds
        for slot in 0..35 {
            inv.set_slot(
                slot,
                Some(ItemStack::new("minecraft:cobblestone", 64).unwrap()),
            )
            .unwrap();
        }
        inv.set_slot(35, Some(ItemStack::new("minecraft:diamond", 60).unwrap()))
            .unwrap();

        let stack = ItemStack::new("minecraft:diamond", 10).unwrap();
        let mut item = ItemEntity::new(5, [0.0, 1.0, 0.0], stack);

        // Player picks up: only 4 diamonds fit into slot 35 (to reach 64)
        let res = item.try_collect_at(99, [0.0, 1.0, 0.0], &mut inv);
        assert_eq!(res.collected_count, 4);
        assert_eq!(res.remaining_count, 6);
        assert_eq!(item.count(), 6);
        assert!(item.is_alive());

        assert_eq!(inv.get_slot(35).unwrap().count(), 64);
        let event = res.event.unwrap();
        assert_eq!(event.packet.pickup_item_count, 4);

        // Subsequent pickup attempt collects 0 because inventory is now completely full
        let res2 = item.try_collect_at(99, [0.0, 1.0, 0.0], &mut inv);
        assert_eq!(res2.collected_count, 0);
        assert_eq!(res2.remaining_count, 6);
        assert!(res2.event.is_none());
    }

    #[test]
    fn test_age_progression_and_despawn_threshold() {
        let stack = ItemStack::new("minecraft:apple", 1).unwrap();
        let mut item = ItemEntity::new(1, [0.0, 10.0, 0.0], stack).with_age(5998);

        assert!(item.is_alive());
        item.tick_lifecycle();
        assert_eq!(item.age(), 5999);
        assert!(item.is_alive());

        item.tick_lifecycle();
        assert_eq!(item.age(), 6000);
        assert!(item.is_dead());
    }

    #[test]
    fn test_wall_sliding_and_axis_independence() {
        let mut world = SimpleVoxelWorld::new();
        // Solid wall at x = 1, y in 0..2, z in -10..10
        for z in -5..=5 {
            world.add_solid_block(1, 0, z);
            world.add_solid_block(1, 1, z);
        }

        // Item at x = 0.5, y = 0.5, z = 0.0. Moving diagonally into wall in X while travelling in Z.
        let stack = ItemStack::new("minecraft:dirt", 1).unwrap();
        let mut item = ItemEntity::new(1, [0.5, 0.5, 0.0], stack).with_velocity([0.8, 0.0, 0.5]);

        item.tick_physics(&world);

        // X collision stopped at wall: item width is 0.25, half-width 0.125.
        // Wall min_x = 1.0. Max item x = 1.0 - 0.125 = 0.875.
        assert!((item.position()[0] - 0.875).abs() < 1e-5);
        assert_eq!(item.velocity()[0], 0.0);

        // Z motion was unhindered and continues sliding!
        assert!(item.position()[2] > 0.4);
        assert!(item.velocity()[2] > 0.0);
    }

    #[test]
    fn test_take_item_entity_packet_wire_roundtrip() {
        let packet = TakeItemEntityPacket::new(1234, 5678, 32);
        let payload = packet.encode_payload();

        let decoded = TakeItemEntityPacket::decode_payload(&payload).unwrap();
        assert_eq!(decoded, packet);

        let framed = packet.encode_packet();
        // Verify framed packet: [VarInt length] [VarInt 0x67] [payload]
        let (len, body) = read_varint(&framed).unwrap();
        let (packet_id, payload_read) = read_varint(body).unwrap();
        assert_eq!(packet_id, TAKE_ITEM_ENTITY_PACKET_ID);
        assert_eq!(len as usize, body.len());
        assert_eq!(payload_read, &payload);
    }

    #[test]
    fn test_item_entity_manager_loop() {
        let mut manager = ItemEntityManager::new();

        // Spawn 2 items that should merge
        let s1 = ItemStack::new("minecraft:redstone", 10).unwrap();
        let s2 = ItemStack::new("minecraft:redstone", 15).unwrap();
        manager.spawn(ItemEntity::new(1, [0.0, 10.0, 0.0], s1));
        manager.spawn(ItemEntity::new(2, [0.3, 10.0, 0.0], s2));
        assert_eq!(manager.count(), 2);

        // Tick manager without obstacles: items merge and dead item is purged
        manager.tick(&());
        assert_eq!(manager.count(), 1);
        assert_eq!(manager.items()[0].count(), 25);

        // Player pickup via manager
        let mut inv = PlayerInventory::new();
        let events = manager.handle_player_pickup(10, [0.0, 10.0, 0.0], &mut inv);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].count, 25);
        assert_eq!(manager.count(), 0);
        assert_eq!(inv.total_count_of("minecraft:redstone"), 25);
    }

    #[test]
    fn test_chunk_collision_view() {
        let mut chunk = Chunk::new(0, 0);
        chunk.set_block(0, 0, 0, Block::Stone);

        // Item falling directly onto chunk block at (0, 0, 0)
        let stack = ItemStack::new("minecraft:dirt", 1).unwrap();
        let mut item = ItemEntity::new(1, [0.5, 1.2, 0.5], stack).with_velocity([0.0, -0.5, 0.0]);

        item.tick(&chunk);
        assert!(item.on_ground());
        assert_eq!(item.position()[1], 1.0);
    }

    #[test]
    fn test_block_break_pickup_delay() {
        let stack = ItemStack::new("minecraft:cobblestone", 1).unwrap();
        let item = ItemEntity::dropped_by_block_break(1, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0], stack);
        assert_eq!(item.pickup_delay(), BLOCK_BREAK_PICKUP_DELAY);
    }
}
