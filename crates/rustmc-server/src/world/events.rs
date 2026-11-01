//! Authoritative block interaction event bus, event models, and rollback helpers.
//!
//! Milestone M4/M5: provides synchronous event dispatching with priority ordering,
//! cancellation propagation, and authoritative rollback packet generation for
//! optimistic client break/place actions.

use std::{
    any::{Any, TypeId},
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::{
    discovery_java::{frame, put_string, put_varint},
    world::{Block, Chunk},
};

/// Packet ID for `ClientboundBlockUpdatePacket` (protocol 777 / Java 26.3).
pub const BLOCK_UPDATE_PACKET_ID: u32 = 0x09;

/// Packet ID for `ClientboundBlockChangedAckPacket` (protocol 777 / Java 26.3).
pub const ACKNOWLEDGE_BLOCK_CHANGE_PACKET_ID: u32 = 0x05;

/// Packet ID for `ClientboundContainerSetSlotPacket` (protocol 777 / Java 26.3).
pub const CONTAINER_SET_SLOT_PACKET_ID: u32 = 0x15;

/// 3D integer coordinate identifying a block cell in world space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    /// Creates a new `BlockPos`.
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Packs coordinates into the standard 64-bit Java Edition representation:
    /// `((x & 0x3FFFFFF) << 38) | ((z & 0x3FFFFFF) << 12) | (y & 0xFFF)`.
    pub const fn pack(&self) -> u64 {
        let ux = (self.x as i64 & 0x3FFFFFF) as u64;
        let uz = (self.z as i64 & 0x3FFFFFF) as u64;
        let uy = (self.y as i64 & 0xFFF) as u64;
        (ux << 38) | (uz << 12) | uy
    }

    /// Unpacks coordinates from the standard 64-bit Java Edition representation.
    pub const fn unpack(packed: u64) -> Self {
        let x = ((packed as i64) >> 38) as i32;
        let y = (((packed as i64) << 52) >> 52) as i32;
        let z = (((packed as i64) << 26) >> 38) as i32;
        Self { x, y, z }
    }

    /// Chunk coordinates `(cx, cz)` owning this block position.
    pub fn chunk_coords(&self) -> (i32, i32) {
        (self.x.div_euclid(16), self.z.div_euclid(16))
    }

    /// Within-chunk local index coordinates `(local_x, y, local_z)`.
    pub fn local_coords(&self) -> (usize, usize, usize) {
        (
            self.x.rem_euclid(16) as usize,
            self.y as usize,
            self.z.rem_euclid(16) as usize,
        )
    }

    /// Offsets position by the step of one `BlockFace`.
    pub fn offset(&self, face: BlockFace) -> Self {
        let (dx, dy, dz) = face.offset();
        Self::new(self.x + dx, self.y + dy, self.z + dz)
    }
}

/// Six cardinal cube faces / directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BlockFace {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

/// Type alias aligning with `Direction`.
pub type Direction = BlockFace;

impl BlockFace {
    pub const BOTTOM: Self = Self::Down;
    pub const TOP: Self = Self::Up;
    #[allow(non_upper_case_globals)]
    pub const Bottom: Self = Self::Down;
    #[allow(non_upper_case_globals)]
    pub const Top: Self = Self::Up;

    /// Cardinal unit offset `(dx, dy, dz)` for this face.
    pub const fn offset(self) -> (i32, i32, i32) {
        match self {
            Self::Down => (0, -1, 0),
            Self::Up => (0, 1, 0),
            Self::North => (0, 0, -1),
            Self::South => (0, 0, 1),
            Self::West => (-1, 0, 0),
            Self::East => (1, 0, 0),
        }
    }

    /// Opposite face.
    pub const fn opposite(self) -> Self {
        match self {
            Self::Down => Self::Up,
            Self::Up => Self::Down,
            Self::North => Self::South,
            Self::South => Self::North,
            Self::West => Self::East,
            Self::East => Self::West,
        }
    }

    /// Protocol face index (0: Down, 1: Up, 2: North, 3: South, 4: West, 5: East).
    pub const fn index(self) -> u32 {
        match self {
            Self::Down => 0,
            Self::Up => 1,
            Self::North => 2,
            Self::South => 3,
            Self::West => 4,
            Self::East => 5,
        }
    }

    /// Resolves protocol face index to `BlockFace`.
    pub const fn from_index(index: u32) -> Option<Self> {
        match index {
            0 => Some(Self::Down),
            1 => Some(Self::Up),
            2 => Some(Self::North),
            3 => Some(Self::South),
            4 => Some(Self::West),
            5 => Some(Self::East),
            _ => None,
        }
    }
}

/// Hand used during interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InteractionHand {
    MainHand,
    OffHand,
}

/// Type of player interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InteractAction {
    RightClickBlock,
    LeftClickBlock,
    RightClickAir,
    LeftClickAir,
    Physical,
}

/// An inventory or dropped item stack.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemStack {
    pub item: String,
    pub count: u32,
    pub item_id: Option<u32>,
}

impl ItemStack {
    /// Creates a new `ItemStack`.
    pub fn new(item: impl Into<String>, count: u32) -> Self {
        Self {
            item: item.into(),
            count,
            item_id: None,
        }
    }

    /// Creates an `ItemStack` of count 1.
    pub fn single(item: impl Into<String>) -> Self {
        Self::new(item, 1)
    }

    /// Creates an `ItemStack` with a specific protocol numeric ID.
    pub fn with_id(item: impl Into<String>, count: u32, item_id: u32) -> Self {
        Self {
            item: item.into(),
            count,
            item_id: Some(item_id),
        }
    }
}

/// Common trait for events that can be intercepted and cancelled.
pub trait Cancellable {
    /// Cancels this event.
    fn cancel(&mut self);

    /// Uncancels this event.
    fn uncancel(&mut self);

    /// Checks if this event is currently cancelled.
    fn is_cancelled(&self) -> bool;

    /// Sets explicit cancellation status.
    fn set_cancelled(&mut self, cancelled: bool);
}

/// Event marker trait with downcasting capability.
pub trait Event: Any + Send {
    /// Checks cancellation status.
    fn is_cancelled(&self) -> bool;

    /// Sets cancellation status.
    fn set_cancelled(&mut self, cancelled: bool);
}

impl<T: Cancellable + Any + Send> Event for T {
    fn is_cancelled(&self) -> bool {
        Cancellable::is_cancelled(self)
    }

    fn set_cancelled(&mut self, cancelled: bool) {
        Cancellable::set_cancelled(self, cancelled);
    }
}

/// Fired when a player attempts to break a block in the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockBreakEvent {
    pub player_id: u32,
    pub pos: BlockPos,
    pub block: Block,
    pub tool: Option<ItemStack>,
    pub drops: Vec<ItemStack>,
    pub is_cancelled: bool,
}

impl BlockBreakEvent {
    /// Creates a new `BlockBreakEvent`.
    pub fn new(
        player_id: u32,
        pos: BlockPos,
        block: Block,
        tool: Option<ItemStack>,
        drops: Vec<ItemStack>,
    ) -> Self {
        Self {
            player_id,
            pos,
            block,
            tool,
            drops,
            is_cancelled: false,
        }
    }

    /// Cancels the break action.
    pub fn cancel(&mut self) {
        self.is_cancelled = true;
    }

    /// Uncancels the break action.
    pub fn uncancel(&mut self) {
        self.is_cancelled = false;
    }

    /// Whether this break action was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.is_cancelled
    }

    /// Sets cancellation status.
    pub fn set_cancelled(&mut self, cancelled: bool) {
        self.is_cancelled = cancelled;
    }

    /// Updates the item stacks dropped by this break.
    pub fn set_drops(&mut self, drops: Vec<ItemStack>) {
        self.drops = drops;
    }

    /// Generates rollback packets if cancelled.
    pub fn rollback(&self) -> Option<BreakRollback> {
        rollback_break(self)
    }

    /// Generates rollback packets with sequence acknowledgement if cancelled.
    pub fn rollback_with_ack(&self, sequence: i32) -> Option<BreakRollback> {
        rollback_break_with_ack(self, sequence)
    }
}

impl Cancellable for BlockBreakEvent {
    fn cancel(&mut self) {
        self.cancel();
    }

    fn uncancel(&mut self) {
        self.uncancel();
    }

    fn is_cancelled(&self) -> bool {
        self.is_cancelled()
    }

    fn set_cancelled(&mut self, cancelled: bool) {
        self.set_cancelled(cancelled);
    }
}

/// Fired when a player attempts to place a block in the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPlaceEvent {
    pub player_id: u32,
    pub placed_pos: BlockPos,
    pub placed_against: BlockPos,
    pub placed_block: Block,
    pub item_in_hand: ItemStack,
    pub is_cancelled: bool,
}

impl BlockPlaceEvent {
    /// Creates a new `BlockPlaceEvent`.
    pub fn new(
        player_id: u32,
        placed_pos: BlockPos,
        placed_against: BlockPos,
        placed_block: Block,
        item_in_hand: ItemStack,
    ) -> Self {
        Self {
            player_id,
            placed_pos,
            placed_against,
            placed_block,
            item_in_hand,
            is_cancelled: false,
        }
    }

    /// Cancels the placement action.
    pub fn cancel(&mut self) {
        self.is_cancelled = true;
    }

    /// Uncancels the placement action.
    pub fn uncancel(&mut self) {
        self.is_cancelled = false;
    }

    /// Whether this placement action was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.is_cancelled
    }

    /// Sets cancellation status.
    pub fn set_cancelled(&mut self, cancelled: bool) {
        self.is_cancelled = cancelled;
    }

    /// Generates rollback packets if cancelled, restoring client inventory slot.
    pub fn rollback(&self, inventory_slot: i16) -> Option<PlaceRollback> {
        rollback_place(self, inventory_slot)
    }

    /// Generates rollback packets with sequence acknowledgement if cancelled.
    pub fn rollback_with_ack(&self, inventory_slot: i16, sequence: i32) -> Option<PlaceRollback> {
        rollback_place_with_ack(self, inventory_slot, sequence)
    }
}

impl Cancellable for BlockPlaceEvent {
    fn cancel(&mut self) {
        self.cancel();
    }

    fn uncancel(&mut self) {
        self.uncancel();
    }

    fn is_cancelled(&self) -> bool {
        self.is_cancelled()
    }

    fn set_cancelled(&mut self, cancelled: bool) {
        self.set_cancelled(cancelled);
    }
}

/// Fired when a player interacts with a block or the air.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInteractEvent {
    pub player_id: u32,
    pub clicked_pos: Option<BlockPos>,
    pub face: Option<BlockFace>,
    pub hand: InteractionHand,
    pub action: InteractAction,
    pub is_cancelled: bool,
}

impl PlayerInteractEvent {
    /// Creates a new `PlayerInteractEvent`.
    pub fn new(
        player_id: u32,
        clicked_pos: Option<BlockPos>,
        face: Option<BlockFace>,
        hand: InteractionHand,
        action: InteractAction,
    ) -> Self {
        Self {
            player_id,
            clicked_pos,
            face,
            hand,
            action,
            is_cancelled: false,
        }
    }

    /// Cancels the interaction.
    pub fn cancel(&mut self) {
        self.is_cancelled = true;
    }

    /// Uncancels the interaction.
    pub fn uncancel(&mut self) {
        self.is_cancelled = false;
    }

    /// Whether this interaction was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.is_cancelled
    }

    /// Sets cancellation status.
    pub fn set_cancelled(&mut self, cancelled: bool) {
        self.is_cancelled = cancelled;
    }
}

impl Cancellable for PlayerInteractEvent {
    fn cancel(&mut self) {
        self.cancel();
    }

    fn uncancel(&mut self) {
        self.uncancel();
    }

    fn is_cancelled(&self) -> bool {
        self.is_cancelled()
    }

    fn set_cancelled(&mut self, cancelled: bool) {
        self.set_cancelled(cancelled);
    }
}

/// Execution priority for event listeners.
///
/// Handlers run in ascending order: `Lowest` -> `Low` -> `Normal` -> `High` -> `Highest` -> `Monitor`.
/// If an event is cancelled, subsequent non-monitor handlers are bypassed.
/// Monitor handlers are always invoked and cannot mutate the cancellation outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventPriority {
    Lowest,
    Low,
    Normal,
    High,
    Highest,
    Monitor,
}

impl EventPriority {
    /// All priorities in execution order.
    pub const ALL: [Self; 6] = [
        Self::Lowest,
        Self::Low,
        Self::Normal,
        Self::High,
        Self::Highest,
        Self::Monitor,
    ];

    /// Whether this priority is monitor-only.
    pub const fn is_monitor(self) -> bool {
        matches!(self, Self::Monitor)
    }
}

type AnyListener = Arc<Mutex<dyn FnMut(&mut dyn Any) + Send>>;

struct RegisteredListener {
    id: u64,
    priority: EventPriority,
    handler: AnyListener,
}

#[derive(Default)]
struct EventBusInner {
    next_id: AtomicU64,
    listeners: Mutex<BTreeMap<TypeId, Vec<RegisteredListener>>>,
}

/// Synchronous, priority-ordered event dispatching pipeline.
#[derive(Clone, Default)]
pub struct EventBus {
    inner: Arc<EventBusInner>,
}

impl EventBus {
    /// Creates a fresh, empty `EventBus`.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(EventBusInner::default()),
        }
    }

    /// Registers a listener for event type `E` at the specified priority.
    ///
    /// Returns a listener ID for unregistration.
    pub fn register<E: Event + 'static, F>(&self, priority: EventPriority, mut handler: F) -> u64
    where
        F: FnMut(&mut E) + Send + 'static,
    {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let wrapper = Arc::new(Mutex::new(move |any: &mut dyn Any| {
            if let Some(event) = any.downcast_mut::<E>() {
                handler(event);
            }
        }));

        let listener = RegisteredListener {
            id,
            priority,
            handler: wrapper,
        };

        let type_id = TypeId::of::<E>();
        let mut map = self
            .inner
            .listeners
            .lock()
            .expect("event bus lock poisoned");
        let list = map.entry(type_id).or_default();
        list.push(listener);
        list.sort_by_key(|l| l.priority);
        id
    }

    /// Unregisters a listener by ID.
    pub fn unregister(&self, listener_id: u64) -> bool {
        let mut map = self
            .inner
            .listeners
            .lock()
            .expect("event bus lock poisoned");
        for list in map.values_mut() {
            if let Some(pos) = list.iter().position(|l| l.id == listener_id) {
                list.remove(pos);
                return true;
            }
        }
        false
    }

    /// Clears all registered listeners.
    pub fn clear(&self) {
        let mut map = self
            .inner
            .listeners
            .lock()
            .expect("event bus lock poisoned");
        map.clear();
    }

    /// Returns the number of registered listeners for event type `E`.
    pub fn listener_count<E: Event + 'static>(&self) -> usize {
        let type_id = TypeId::of::<E>();
        let map = self
            .inner
            .listeners
            .lock()
            .expect("event bus lock poisoned");
        map.get(&type_id).map_or(0, |l| l.len())
    }

    /// Dispatches an event through listeners in priority order.
    ///
    /// If an event is cancelled by an earlier listener, subsequent non-monitor
    /// listeners are bypassed. Monitor listeners always observe, and any attempt
    /// to alter cancellation status within a monitor listener is reverted.
    pub fn dispatch<E: Event + 'static>(&self, event: &mut E) {
        let type_id = TypeId::of::<E>();
        let listeners_snapshot: Vec<(EventPriority, AnyListener)> = {
            let map = self
                .inner
                .listeners
                .lock()
                .expect("event bus lock poisoned");
            let Some(list) = map.get(&type_id) else {
                return;
            };
            list.iter()
                .map(|l| (l.priority, Arc::clone(&l.handler)))
                .collect()
        };

        for (priority, handler_arc) in listeners_snapshot {
            // Cancelled events bypass non-monitor listeners
            if event.is_cancelled() && priority != EventPriority::Monitor {
                continue;
            }

            let cancelled_before = event.is_cancelled();

            if let Ok(mut handler) = handler_arc.lock() {
                (*handler)(event);
            }

            // Monitor handlers observe without mutating cancellation status
            if priority == EventPriority::Monitor && event.is_cancelled() != cancelled_before {
                event.set_cancelled(cancelled_before);
            }
        }
    }

    /// Convenience registration for `BlockBreakEvent`.
    pub fn register_break<F>(&self, priority: EventPriority, handler: F) -> u64
    where
        F: FnMut(&mut BlockBreakEvent) + Send + 'static,
    {
        self.register::<BlockBreakEvent, F>(priority, handler)
    }

    /// Convenience registration for `BlockPlaceEvent`.
    pub fn register_place<F>(&self, priority: EventPriority, handler: F) -> u64
    where
        F: FnMut(&mut BlockPlaceEvent) + Send + 'static,
    {
        self.register::<BlockPlaceEvent, F>(priority, handler)
    }

    /// Convenience registration for `PlayerInteractEvent`.
    pub fn register_interact<F>(&self, priority: EventPriority, handler: F) -> u64
    where
        F: FnMut(&mut PlayerInteractEvent) + Send + 'static,
    {
        self.register::<PlayerInteractEvent, F>(priority, handler)
    }

    /// Convenience dispatch for `BlockBreakEvent`.
    pub fn dispatch_break(&self, event: &mut BlockBreakEvent) {
        self.dispatch::<BlockBreakEvent>(event);
    }

    /// Convenience dispatch for `BlockPlaceEvent`.
    pub fn dispatch_place(&self, event: &mut BlockPlaceEvent) {
        self.dispatch::<BlockPlaceEvent>(event);
    }

    /// Convenience dispatch for `PlayerInteractEvent`.
    pub fn dispatch_interact(&self, event: &mut PlayerInteractEvent) {
        self.dispatch::<PlayerInteractEvent>(event);
    }
}

/// Outgoing `BlockUpdatePacket` emitted to restore or update an authoritative block cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockUpdatePacket {
    pub pos: BlockPos,
    pub block_state_id: u32,
}

impl BlockUpdatePacket {
    /// Creates a new `BlockUpdatePacket`.
    pub const fn new(pos: BlockPos, block_state_id: u32) -> Self {
        Self {
            pos,
            block_state_id,
        }
    }

    /// Encodes into standard Java 26.3 framed packet bytes:
    /// `VarInt(len) + VarInt(packet_id=0x09) + 8-byte packed Position + VarInt(block_state_id)`.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&self.pos.pack().to_be_bytes());
        put_varint(self.block_state_id, &mut body);
        frame(BLOCK_UPDATE_PACKET_ID, &body)
    }
}

/// Outgoing `AcknowledgeBlockChangePacket` acknowledging client interaction sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcknowledgeBlockChangePacket {
    pub sequence: i32,
}

impl AcknowledgeBlockChangePacket {
    /// Creates a new `AcknowledgeBlockChangePacket`.
    pub const fn new(sequence: i32) -> Self {
        Self { sequence }
    }

    /// Encodes into standard Java 26.3 framed packet bytes:
    /// `VarInt(len) + VarInt(packet_id=0x05) + VarInt(sequence)`.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        put_varint(self.sequence as u32, &mut body);
        frame(ACKNOWLEDGE_BLOCK_CHANGE_PACKET_ID, &body)
    }
}

/// Outgoing `ContainerSetSlotPacket` syncing an item slot into the client inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSetSlotPacket {
    pub window_id: i8,
    pub state_id: i32,
    pub slot: i16,
    pub item: ItemStack,
}

impl ContainerSetSlotPacket {
    /// Creates a new `ContainerSetSlotPacket`.
    pub const fn new(window_id: i8, state_id: i32, slot: i16, item: ItemStack) -> Self {
        Self {
            window_id,
            state_id,
            slot,
            item,
        }
    }

    /// Player inventory slot update (`window_id = 0`).
    pub fn player_inventory(slot: i16, item: ItemStack) -> Self {
        Self::new(0, 0, slot, item)
    }

    /// Encodes into standard Java 26.3 framed packet bytes:
    /// `VarInt(len) + VarInt(packet_id=0x15) + window_id (1B) + state_id (VarInt) + slot (2B) + count (VarInt) + item (String)`.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(self.window_id as u8);
        put_varint(self.state_id as u32, &mut body);
        body.extend_from_slice(&self.slot.to_be_bytes());
        put_varint(self.item.count, &mut body);
        put_string(&self.item.item, &mut body);
        frame(CONTAINER_SET_SLOT_PACKET_ID, &body)
    }
}

/// Rollback packet set emitted when a `BlockBreakEvent` is cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakRollback {
    pub block_update: BlockUpdatePacket,
    pub ack: Option<AcknowledgeBlockChangePacket>,
}

impl BreakRollback {
    /// Encodes all rollback packets to wire frames.
    pub fn to_packets(&self) -> Vec<Vec<u8>> {
        let mut packets = vec![self.block_update.encode()];
        if let Some(ack) = &self.ack {
            packets.push(ack.encode());
        }
        packets
    }
}

/// Rollback packet set emitted when a `BlockPlaceEvent` is cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceRollback {
    pub block_update: BlockUpdatePacket,
    pub inventory_sync: ContainerSetSlotPacket,
    pub ack: Option<AcknowledgeBlockChangePacket>,
}

impl PlaceRollback {
    /// Encodes all rollback packets to wire frames.
    pub fn to_packets(&self) -> Vec<Vec<u8>> {
        let mut packets = vec![self.block_update.encode(), self.inventory_sync.encode()];
        if let Some(ack) = &self.ack {
            packets.push(ack.encode());
        }
        packets
    }
}

/// Produces authoritative break rollback packets restoring the original block state.
pub fn rollback_break(event: &BlockBreakEvent) -> Option<BreakRollback> {
    rollback_break_explicit(event, event.block.default_state_id(), None)
}

/// Produces authoritative break rollback packets with sequence acknowledgement.
pub fn rollback_break_with_ack(event: &BlockBreakEvent, sequence: i32) -> Option<BreakRollback> {
    rollback_break_explicit(event, event.block.default_state_id(), Some(sequence))
}

/// Produces authoritative break rollback with explicit block state ID and sequence.
pub fn rollback_break_explicit(
    event: &BlockBreakEvent,
    state_id: u32,
    sequence: Option<i32>,
) -> Option<BreakRollback> {
    if !event.is_cancelled() {
        return None;
    }
    Some(BreakRollback {
        block_update: BlockUpdatePacket::new(event.pos, state_id),
        ack: sequence.map(AcknowledgeBlockChangePacket::new),
    })
}

/// Produces authoritative place rollback packets restoring air and syncing client inventory.
pub fn rollback_place(event: &BlockPlaceEvent, inventory_slot: i16) -> Option<PlaceRollback> {
    rollback_place_explicit(event, Block::Air.default_state_id(), inventory_slot, None)
}

/// Produces authoritative place rollback packets with sequence acknowledgement.
pub fn rollback_place_with_ack(
    event: &BlockPlaceEvent,
    inventory_slot: i16,
    sequence: i32,
) -> Option<PlaceRollback> {
    rollback_place_explicit(
        event,
        Block::Air.default_state_id(),
        inventory_slot,
        Some(sequence),
    )
}

/// Produces authoritative place rollback with explicit air state ID, inventory slot, and sequence.
pub fn rollback_place_explicit(
    event: &BlockPlaceEvent,
    air_state_id: u32,
    inventory_slot: i16,
    sequence: Option<i32>,
) -> Option<PlaceRollback> {
    if !event.is_cancelled() {
        return None;
    }
    Some(PlaceRollback {
        block_update: BlockUpdatePacket::new(event.placed_pos, air_state_id),
        inventory_sync: ContainerSetSlotPacket::player_inventory(
            inventory_slot,
            event.item_in_hand.clone(),
        ),
        ack: sequence.map(AcknowledgeBlockChangePacket::new),
    })
}

/// Result of an accepted world block break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockBreakOutcome {
    pub player_id: u32,
    pub pos: BlockPos,
    pub original_block: Block,
    pub drops: Vec<ItemStack>,
}

/// Result of an accepted world block placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPlaceOutcome {
    pub player_id: u32,
    pub placed_pos: BlockPos,
    pub placed_against: BlockPos,
    pub placed_block: Block,
    pub remaining_item: ItemStack,
}

/// Authoritative block break execution pipeline.
///
/// 1. Reads target cell from chunk; returns rollback if unaddressable.
/// 2. Dispatches `BlockBreakEvent` through the event bus.
/// 3. If cancelled, world is NOT mutated and `Err(BreakRollback)` is returned.
/// 4. If admitted, replaces block with `Block::Air` and returns `Ok(BlockBreakOutcome)`.
pub fn apply_block_break(
    bus: &EventBus,
    chunk: &mut Chunk,
    player_id: u32,
    pos: BlockPos,
    tool: Option<ItemStack>,
    default_drops: Vec<ItemStack>,
) -> Result<BlockBreakOutcome, BreakRollback> {
    let (chunk_x, chunk_z) = pos.chunk_coords();
    if chunk.x != chunk_x || chunk.z != chunk_z {
        let mut dummy = BlockBreakEvent::new(player_id, pos, Block::Air, tool, default_drops);
        dummy.cancel();
        return Err(rollback_break_explicit(&dummy, 0, None).expect("rollback exists"));
    }
    let (lx, y, lz) = pos.local_coords();
    let current_block = match chunk.block(lx, y, lz) {
        Some(b) => b,
        None => {
            let mut dummy = BlockBreakEvent::new(player_id, pos, Block::Air, tool, default_drops);
            dummy.cancel();
            return Err(rollback_break_explicit(&dummy, 0, None).expect("rollback exists"));
        }
    };

    let mut event = BlockBreakEvent::new(player_id, pos, current_block, tool, default_drops);
    bus.dispatch(&mut event);

    if event.is_cancelled() {
        let rollback = rollback_break(&event).expect("rollback for cancelled break");
        Err(rollback)
    } else {
        chunk.set_world_block(pos.x as i64, pos.y as i64, pos.z as i64, Block::Air);
        Ok(BlockBreakOutcome {
            player_id,
            pos,
            original_block: current_block,
            drops: event.drops,
        })
    }
}

/// Authoritative block place execution pipeline.
///
/// 1. Dispatches `BlockPlaceEvent` through the event bus.
/// 2. If cancelled, world is NOT mutated and `Err(PlaceRollback)` is returned.
/// 3. If admitted, mutates chunk with `placed_block`, decrements hand item, and returns `Ok(BlockPlaceOutcome)`.
#[allow(clippy::too_many_arguments)]
pub fn apply_block_place(
    bus: &EventBus,
    chunk: &mut Chunk,
    player_id: u32,
    placed_pos: BlockPos,
    placed_against: BlockPos,
    placed_block: Block,
    item_in_hand: ItemStack,
    inventory_slot: i16,
) -> Result<BlockPlaceOutcome, PlaceRollback> {
    let mut event = BlockPlaceEvent::new(
        player_id,
        placed_pos,
        placed_against,
        placed_block,
        item_in_hand.clone(),
    );
    bus.dispatch(&mut event);

    if event.is_cancelled() {
        let rollback =
            rollback_place(&event, inventory_slot).expect("rollback for cancelled place");
        Err(rollback)
    } else {
        chunk.set_world_block(
            placed_pos.x as i64,
            placed_pos.y as i64,
            placed_pos.z as i64,
            placed_block,
        );
        let mut remaining_item = item_in_hand;
        remaining_item.count = remaining_item.count.saturating_sub(1);
        Ok(BlockPlaceOutcome {
            player_id,
            placed_pos,
            placed_against,
            placed_block,
            remaining_item,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{CHUNK_SIDE, WORLD_HEIGHT};
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn block_pos_packing_roundtrip_all_quadrants() {
        let cases = [
            BlockPos::new(0, 0, 0),
            BlockPos::new(1, 64, 1),
            BlockPos::new(-1, -1, -1),
            BlockPos::new(100, -64, -200),
            BlockPos::new(-33_554_432, 2047, 33_554_431),
            BlockPos::new(33_554_431, -2048, -33_554_432),
            BlockPos::new(1234, 128, -5678),
        ];

        for pos in cases {
            let packed = pos.pack();
            let unpacked = BlockPos::unpack(packed);
            assert_eq!(pos, unpacked, "Roundtrip failed for {pos:?}");
        }
    }

    #[test]
    fn block_pos_chunk_and_local_coords() {
        let pos1 = BlockPos::new(0, 64, 0);
        assert_eq!(pos1.chunk_coords(), (0, 0));
        assert_eq!(pos1.local_coords(), (0, 64, 0));

        let pos2 = BlockPos::new(15, 64, 15);
        assert_eq!(pos2.chunk_coords(), (0, 0));
        assert_eq!(pos2.local_coords(), (15, 64, 15));

        let pos3 = BlockPos::new(16, 64, 16);
        assert_eq!(pos3.chunk_coords(), (1, 1));
        assert_eq!(pos3.local_coords(), (0, 64, 0));

        let pos_neg = BlockPos::new(-1, 64, -1);
        assert_eq!(pos_neg.chunk_coords(), (-1, -1));
        assert_eq!(pos_neg.local_coords(), (15, 64, 15));

        let pos_neg16 = BlockPos::new(-16, 64, -16);
        assert_eq!(pos_neg16.chunk_coords(), (-1, -1));
        assert_eq!(pos_neg16.local_coords(), (0, 64, 0));
    }

    #[test]
    fn block_face_cardinal_offsets_and_opposites() {
        let pos = BlockPos::new(10, 20, 30);
        assert_eq!(pos.offset(BlockFace::Down), BlockPos::new(10, 19, 30));
        assert_eq!(pos.offset(BlockFace::Up), BlockPos::new(10, 21, 30));
        assert_eq!(pos.offset(BlockFace::North), BlockPos::new(10, 20, 29));
        assert_eq!(pos.offset(BlockFace::South), BlockPos::new(10, 20, 31));
        assert_eq!(pos.offset(BlockFace::West), BlockPos::new(9, 20, 30));
        assert_eq!(pos.offset(BlockFace::East), BlockPos::new(11, 20, 30));

        for face in [
            BlockFace::Down,
            BlockFace::Up,
            BlockFace::North,
            BlockFace::South,
            BlockFace::West,
            BlockFace::East,
        ] {
            assert_eq!(face.opposite().opposite(), face);
            assert_eq!(BlockFace::from_index(face.index()), Some(face));
        }
    }

    #[test]
    fn block_break_event_registration_and_dispatch() {
        let bus = EventBus::new();
        let called = Arc::new(AtomicUsize::new(0));
        let called_clone = Arc::clone(&called);

        bus.register_break(EventPriority::Normal, move |event| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            assert_eq!(event.player_id, 42);
            assert_eq!(event.pos, BlockPos::new(10, 64, 10));
            assert_eq!(event.block, Block::Stone);
            assert_eq!(
                event.tool,
                Some(ItemStack::single("minecraft:iron_pickaxe"))
            );
            assert_eq!(event.drops.len(), 1);

            // Modify drops
            event.set_drops(vec![
                ItemStack::new("minecraft:cobblestone", 2),
                ItemStack::single("minecraft:raw_iron"),
            ]);
        });

        let mut event = BlockBreakEvent::new(
            42,
            BlockPos::new(10, 64, 10),
            Block::Stone,
            Some(ItemStack::single("minecraft:iron_pickaxe")),
            vec![ItemStack::single("minecraft:cobblestone")],
        );

        assert!(!event.is_cancelled());
        bus.dispatch_break(&mut event);

        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_eq!(event.drops.len(), 2);
        assert_eq!(event.drops[0].count, 2);
        assert_eq!(event.drops[1].item, "minecraft:raw_iron");
        assert!(!event.is_cancelled());
    }

    #[test]
    fn priority_execution_order_verification() {
        let bus = EventBus::new();
        let log = Arc::new(Mutex::new(Vec::new()));

        // Register in intentionally scrambled order
        let priorities = [
            EventPriority::Monitor,
            EventPriority::High,
            EventPriority::Lowest,
            EventPriority::Normal,
            EventPriority::Highest,
            EventPriority::Low,
        ];

        for &priority in &priorities {
            let log_clone = Arc::clone(&log);
            bus.register_break(priority, move |_| {
                log_clone.lock().unwrap().push(priority);
            });
        }

        let mut event =
            BlockBreakEvent::new(1, BlockPos::new(0, 0, 0), Block::Dirt, None, Vec::new());
        bus.dispatch_break(&mut event);

        let execution = log.lock().unwrap().clone();
        assert_eq!(
            execution,
            vec![
                EventPriority::Lowest,
                EventPriority::Low,
                EventPriority::Normal,
                EventPriority::High,
                EventPriority::Highest,
                EventPriority::Monitor,
            ],
            "Execution order must strictly follow priority Lowest to Monitor"
        );
    }

    #[test]
    fn cancellation_stopping_world_mutation_and_bypassing_lower_priority_handlers() {
        let bus = EventBus::new();
        let executed = Arc::new(Mutex::new(Vec::new()));

        let exec1 = Arc::clone(&executed);
        bus.register_break(EventPriority::Lowest, move |_| {
            exec1.lock().unwrap().push(EventPriority::Lowest);
        });

        let exec2 = Arc::clone(&executed);
        bus.register_break(EventPriority::Low, move |event| {
            exec2.lock().unwrap().push(EventPriority::Low);
            event.cancel();
        });

        let exec3 = Arc::clone(&executed);
        bus.register_break(EventPriority::Normal, move |_| {
            exec3.lock().unwrap().push(EventPriority::Normal);
        });

        let exec4 = Arc::clone(&executed);
        bus.register_break(EventPriority::High, move |_| {
            exec4.lock().unwrap().push(EventPriority::High);
        });

        let exec5 = Arc::clone(&executed);
        bus.register_break(EventPriority::Highest, move |_| {
            exec5.lock().unwrap().push(EventPriority::Highest);
        });

        let exec6 = Arc::clone(&executed);
        bus.register_break(EventPriority::Monitor, move |event| {
            exec6.lock().unwrap().push(EventPriority::Monitor);
            assert!(event.is_cancelled(), "Monitor must observe cancelled state");
        });

        let mut event =
            BlockBreakEvent::new(1, BlockPos::new(0, 0, 0), Block::Dirt, None, Vec::new());
        bus.dispatch_break(&mut event);

        assert!(event.is_cancelled());
        let records = executed.lock().unwrap().clone();
        assert_eq!(
            records,
            vec![
                EventPriority::Lowest,
                EventPriority::Low,
                EventPriority::Monitor
            ],
            "Subsequent non-monitor listeners (Normal, High, Highest) must be bypassed"
        );
    }

    #[test]
    fn monitor_handler_observing_cancelled_events_without_mutating_cancellation_status() {
        let bus = EventBus::new();
        let observed_cancelled = Arc::new(AtomicUsize::new(0));
        let obs_clone = Arc::clone(&observed_cancelled);

        // Handler cancels at Normal
        bus.register_break(EventPriority::Normal, |event| {
            event.cancel();
        });

        // Monitor attempts to uncancel
        bus.register_break(EventPriority::Monitor, move |event| {
            if event.is_cancelled() {
                obs_clone.fetch_add(1, Ordering::SeqCst);
            }
            // Rogue monitor attempts mutation:
            event.uncancel();
        });

        let mut event =
            BlockBreakEvent::new(1, BlockPos::new(0, 0, 0), Block::Stone, None, Vec::new());
        bus.dispatch_break(&mut event);

        assert_eq!(
            observed_cancelled.load(Ordering::SeqCst),
            1,
            "Monitor must have observed cancellation"
        );
        assert!(
            event.is_cancelled(),
            "Event must remain cancelled despite rogue monitor mutation attempt"
        );
    }

    #[test]
    fn block_place_event_dispatch_and_cancellation() {
        let bus = EventBus::new();
        let was_cancelled = Arc::new(AtomicUsize::new(0));
        let cancel_clone = Arc::clone(&was_cancelled);

        bus.register_place(EventPriority::High, move |event| {
            if event.placed_block == Block::Bedrock {
                event.cancel();
                cancel_clone.fetch_add(1, Ordering::SeqCst);
            }
        });

        let mut event = BlockPlaceEvent::new(
            7,
            BlockPos::new(1, 65, 1),
            BlockPos::new(1, 64, 1),
            Block::Bedrock,
            ItemStack::single("minecraft:bedrock"),
        );

        assert!(!event.is_cancelled());
        bus.dispatch_place(&mut event);
        assert!(event.is_cancelled());
        assert_eq!(was_cancelled.load(Ordering::SeqCst), 1);

        // Rollback generation
        let rollback = event.rollback(36).expect("rollback packet set");
        assert_eq!(rollback.block_update.pos, BlockPos::new(1, 65, 1));
        assert_eq!(rollback.block_update.block_state_id, 0); // Air state id
        assert_eq!(rollback.inventory_sync.slot, 36);
        assert_eq!(rollback.inventory_sync.item.item, "minecraft:bedrock");

        let packets = rollback.to_packets();
        assert_eq!(packets.len(), 2);
    }

    #[test]
    fn player_interact_event_dispatch_and_cancellation() {
        let bus = EventBus::new();
        let interact_seen = Arc::new(AtomicUsize::new(0));
        let seen_clone = Arc::clone(&interact_seen);

        bus.register_interact(EventPriority::Normal, move |event| {
            if event.action == InteractAction::RightClickBlock {
                assert_eq!(event.hand, InteractionHand::MainHand);
                assert_eq!(event.clicked_pos, Some(BlockPos::new(5, 64, 5)));
                assert_eq!(event.face, Some(BlockFace::Up));
                event.cancel();
                seen_clone.fetch_add(1, Ordering::SeqCst);
            }
        });

        let mut event = PlayerInteractEvent::new(
            123,
            Some(BlockPos::new(5, 64, 5)),
            Some(BlockFace::Up),
            InteractionHand::MainHand,
            InteractAction::RightClickBlock,
        );

        assert!(!event.is_cancelled());
        bus.dispatch_interact(&mut event);
        assert!(event.is_cancelled());
        assert_eq!(interact_seen.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rollback_packets_wire_encoding() {
        let update = BlockUpdatePacket::new(BlockPos::new(1, 64, 1), 9); // Grass block
        let encoded_update = update.encode();
        assert!(!encoded_update.is_empty());

        let ack = AcknowledgeBlockChangePacket::new(42);
        let encoded_ack = ack.encode();
        assert!(!encoded_ack.is_empty());

        let set_slot = ContainerSetSlotPacket::player_inventory(
            36,
            ItemStack::new("minecraft:diamond_pickaxe", 1),
        );
        let encoded_slot = set_slot.encode();
        assert!(!encoded_slot.is_empty());
    }

    #[test]
    fn world_mutation_integration_break_allowed_and_cancelled() {
        let bus = EventBus::new();
        let mut chunk = Chunk {
            x: 0,
            z: 0,
            blocks: vec![Block::Air; CHUNK_SIDE * CHUNK_SIDE * WORLD_HEIGHT],
        };
        chunk.set_world_block(5, 64, 5, Block::Stone);
        assert_eq!(chunk.block(5, 64, 5), Some(Block::Stone));

        // 1. Allowed break
        let outcome = apply_block_break(
            &bus,
            &mut chunk,
            1,
            BlockPos::new(5, 64, 5),
            None,
            vec![ItemStack::single("minecraft:cobblestone")],
        )
        .expect("break allowed");

        assert_eq!(outcome.original_block, Block::Stone);
        assert_eq!(chunk.block(5, 64, 5), Some(Block::Air));

        // Reset to Stone
        chunk.set_world_block(5, 64, 5, Block::Stone);

        // 2. Cancelled break
        bus.register_break(EventPriority::Normal, |e| {
            e.cancel();
        });

        let err = apply_block_break(
            &bus,
            &mut chunk,
            1,
            BlockPos::new(5, 64, 5),
            None,
            vec![ItemStack::single("minecraft:cobblestone")],
        )
        .expect_err("break cancelled");

        // World must NOT have mutated
        assert_eq!(chunk.block(5, 64, 5), Some(Block::Stone));
        assert_eq!(err.block_update.pos, BlockPos::new(5, 64, 5));
        assert_eq!(
            err.block_update.block_state_id,
            Block::Stone.default_state_id()
        );
    }

    #[test]
    fn world_mutation_integration_place_allowed_and_cancelled() {
        let bus = EventBus::new();
        let mut chunk = Chunk {
            x: 0,
            z: 0,
            blocks: vec![Block::Air; CHUNK_SIDE * CHUNK_SIDE * WORLD_HEIGHT],
        };
        assert_eq!(chunk.block(2, 65, 2), Some(Block::Air));

        // 1. Allowed placement
        let outcome = apply_block_place(
            &bus,
            &mut chunk,
            1,
            BlockPos::new(2, 65, 2),
            BlockPos::new(2, 64, 2),
            Block::Sandstone,
            ItemStack::new("minecraft:sandstone", 5),
            37,
        )
        .expect("place allowed");

        assert_eq!(outcome.remaining_item.count, 4);
        assert_eq!(chunk.block(2, 65, 2), Some(Block::Sandstone));

        // 2. Cancelled placement
        bus.register_place(EventPriority::Normal, |e| {
            e.cancel();
        });

        let err = apply_block_place(
            &bus,
            &mut chunk,
            1,
            BlockPos::new(3, 65, 3),
            BlockPos::new(3, 64, 3),
            Block::Sandstone,
            ItemStack::new("minecraft:sandstone", 4),
            37,
        )
        .expect_err("place cancelled");

        // World cell must remain air
        assert_eq!(chunk.block(3, 65, 3), Some(Block::Air));
        assert_eq!(err.block_update.pos, BlockPos::new(3, 65, 3));
        assert_eq!(err.block_update.block_state_id, 0); // Air
        assert_eq!(err.inventory_sync.item.count, 4);
        assert_eq!(err.inventory_sync.slot, 37);
    }

    #[test]
    fn unregister_and_clear_listeners() {
        let bus = EventBus::new();
        let count = Arc::new(AtomicUsize::new(0));

        let c1 = Arc::clone(&count);
        let id1 = bus.register_break(EventPriority::Normal, move |_| {
            c1.fetch_add(1, Ordering::SeqCst);
        });

        let c2 = Arc::clone(&count);
        let _id2 = bus.register_break(EventPriority::Normal, move |_| {
            c2.fetch_add(10, Ordering::SeqCst);
        });

        assert_eq!(bus.listener_count::<BlockBreakEvent>(), 2);

        let mut event = BlockBreakEvent::new(1, BlockPos::default(), Block::Stone, None, vec![]);
        bus.dispatch_break(&mut event);
        assert_eq!(count.load(Ordering::SeqCst), 11);

        // Unregister id1
        assert!(bus.unregister(id1));
        assert!(!bus.unregister(id1)); // Idempotent false
        assert_eq!(bus.listener_count::<BlockBreakEvent>(), 1);

        count.store(0, Ordering::SeqCst);
        bus.dispatch_break(&mut event);
        assert_eq!(count.load(Ordering::SeqCst), 10);

        // Clear all
        bus.clear();
        assert_eq!(bus.listener_count::<BlockBreakEvent>(), 0);
        count.store(0, Ordering::SeqCst);
        bus.dispatch_break(&mut event);
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn event_bus_clone_shares_listeners() {
        let bus1 = EventBus::new();
        let bus2 = bus1.clone();
        let executed = Arc::new(AtomicUsize::new(0));

        let exec = Arc::clone(&executed);
        bus1.register_break(EventPriority::High, move |_| {
            exec.fetch_add(1, Ordering::SeqCst);
        });

        let mut event = BlockBreakEvent::new(1, BlockPos::default(), Block::Dirt, None, vec![]);
        bus2.dispatch_break(&mut event);

        assert_eq!(executed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn interact_actions_coverage() {
        let bus = EventBus::new();
        let actions_seen = Arc::new(Mutex::new(Vec::new()));

        let actions = [
            InteractAction::RightClickBlock,
            InteractAction::LeftClickBlock,
            InteractAction::RightClickAir,
            InteractAction::LeftClickAir,
            InteractAction::Physical,
        ];

        let seen = Arc::clone(&actions_seen);
        bus.register_interact(EventPriority::Normal, move |event| {
            seen.lock().unwrap().push(event.action);
        });

        for &action in &actions {
            let mut event = PlayerInteractEvent::new(
                1,
                Some(BlockPos::new(0, 64, 0)),
                Some(BlockFace::Up),
                InteractionHand::OffHand,
                action,
            );
            bus.dispatch_interact(&mut event);
        }

        assert_eq!(actions_seen.lock().unwrap().clone(), actions.to_vec());
    }
}
