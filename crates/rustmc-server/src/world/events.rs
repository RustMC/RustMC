//! Authoritative block interaction event bus, event models, and rollback helpers.
//!
//! Milestone M4/M5: provides synchronous event dispatching with priority ordering,
//! re-entrancy rejection, cancellation propagation, and authoritative rollback
//! packet generation for optimistic client break/place actions.

use std::{
    any::{Any, TypeId},
    collections::BTreeMap,
    fmt,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    thread::ThreadId,
};

use crate::{
    item::ItemStack,
    java_packets::play::clientbound::{BLOCK_CHANGED_ACK, BLOCK_UPDATE, CONTAINER_SET_SLOT},
    java_preview::block_default_state_id,
    java_wire::{frame, pack_block_position, put_varint},
    world::{Block, BlockPos, CHUNK_SIDE, Chunk, Direction, WORLD_HEIGHT},
};

// Play-state IDs below come from `crate::java_packets` (official Java 26.3
// server packet report, protocol 777); field layouts follow the public protocol
// documentation at minecraft.wiki, Java_Edition_protocol/Packets revision
// 3868170 and Java_Edition_protocol/Slot_data.

/// Packet ID for `minecraft:block_update` (official 26.3 packets report: 8).
pub const BLOCK_UPDATE_PACKET_ID: u32 = BLOCK_UPDATE;

/// Packet ID for `minecraft:block_changed_ack` (official 26.3 packets report: 4).
pub const ACKNOWLEDGE_BLOCK_CHANGE_PACKET_ID: u32 = BLOCK_CHANGED_ACK;

/// Packet ID for `minecraft:container_set_slot` (official 26.3 packets report: 20).
pub const CONTAINER_SET_SLOT_PACKET_ID: u32 = CONTAINER_SET_SLOT;

fn chunk_coords_of(pos: BlockPos) -> (i32, i32) {
    (
        pos.x.div_euclid(CHUNK_SIDE as i32),
        pos.z.div_euclid(CHUNK_SIDE as i32),
    )
}

fn local_coords_of(pos: BlockPos) -> (usize, usize, usize) {
    (
        pos.x.rem_euclid(CHUNK_SIDE as i32) as usize,
        pos.y as usize,
        pos.z.rem_euclid(CHUNK_SIDE as i32) as usize,
    )
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
    is_cancelled: bool,
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
    pub item_in_hand: Option<ItemStack>,
    is_cancelled: bool,
}

impl BlockPlaceEvent {
    /// Creates a new `BlockPlaceEvent`.
    pub fn new(
        player_id: u32,
        placed_pos: BlockPos,
        placed_against: BlockPos,
        placed_block: Block,
        item_in_hand: Option<ItemStack>,
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
    pub face: Option<Direction>,
    pub hand: InteractionHand,
    pub action: InteractAction,
    is_cancelled: bool,
}

impl PlayerInteractEvent {
    /// Creates a new `PlayerInteractEvent`.
    pub fn new(
        player_id: u32,
        clicked_pos: Option<BlockPos>,
        face: Option<Direction>,
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

/// Failure returned when event dispatch cannot proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchError {
    /// A listener attempted to dispatch an event on the same bus from inside an active dispatch.
    Reentrant,
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reentrant => f.write_str("re-entrant event dispatch is not permitted"),
        }
    }
}

impl std::error::Error for DispatchError {}

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
    active_threads: Mutex<Vec<ThreadId>>,
}

struct DispatchGuard<'a> {
    active_threads: &'a Mutex<Vec<ThreadId>>,
    thread_id: ThreadId,
}

impl Drop for DispatchGuard<'_> {
    fn drop(&mut self) {
        let mut active = self
            .active_threads
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(pos) = active.iter().position(|id| *id == self.thread_id) {
            active.swap_remove(pos);
        }
    }
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
    /// Rejects re-entrant dispatch from inside an active listener on the same thread
    /// with [`DispatchError::Reentrant`] instead of deadlocking.
    pub fn dispatch<E: Event + 'static>(&self, event: &mut E) -> Result<(), DispatchError> {
        let thread_id = std::thread::current().id();
        {
            let mut active = self
                .inner
                .active_threads
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if active.contains(&thread_id) {
                return Err(DispatchError::Reentrant);
            }
            active.push(thread_id);
        }
        let _guard = DispatchGuard {
            active_threads: &self.inner.active_threads,
            thread_id,
        };

        let type_id = TypeId::of::<E>();
        let listeners_snapshot: Vec<(EventPriority, AnyListener)> = {
            let map = self
                .inner
                .listeners
                .lock()
                .expect("event bus lock poisoned");
            let Some(list) = map.get(&type_id) else {
                return Ok(());
            };
            list.iter()
                .map(|l| (l.priority, Arc::clone(&l.handler)))
                .collect()
        };

        for (priority, handler_arc) in listeners_snapshot {
            if event.is_cancelled() && priority != EventPriority::Monitor {
                continue;
            }

            let cancelled_before = event.is_cancelled();

            {
                let mut handler = handler_arc.lock().unwrap_or_else(PoisonError::into_inner);
                (*handler)(event);
            }

            if priority == EventPriority::Monitor && event.is_cancelled() != cancelled_before {
                event.set_cancelled(cancelled_before);
            }
        }

        Ok(())
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
    pub fn dispatch_break(&self, event: &mut BlockBreakEvent) -> Result<(), DispatchError> {
        self.dispatch::<BlockBreakEvent>(event)
    }

    /// Convenience dispatch for `BlockPlaceEvent`.
    pub fn dispatch_place(&self, event: &mut BlockPlaceEvent) -> Result<(), DispatchError> {
        self.dispatch::<BlockPlaceEvent>(event)
    }

    /// Convenience dispatch for `PlayerInteractEvent`.
    pub fn dispatch_interact(&self, event: &mut PlayerInteractEvent) -> Result<(), DispatchError> {
        self.dispatch::<PlayerInteractEvent>(event)
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
    /// `VarInt(len) + VarInt(packet_id=8) + 8-byte packed Position + VarInt(block_state_id)`.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(
            &pack_block_position(self.pos.x, self.pos.y, self.pos.z).to_be_bytes(),
        );
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
    /// `VarInt(len) + VarInt(packet_id=4) + VarInt(sequence)`.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        put_varint(self.sequence as u32, &mut body);
        frame(ACKNOWLEDGE_BLOCK_CHANGE_PACKET_ID, &body)
    }
}

/// A non-empty stack cannot be written to the wire without its numeric
/// `minecraft:item` registry ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedItemId {
    pub item: String,
}

impl fmt::Display for UnresolvedItemId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "no protocol item ID for {}", self.item)
    }
}

impl std::error::Error for UnresolvedItemId {}

/// Outgoing `ContainerSetSlotPacket` syncing an item slot into the client inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSetSlotPacket {
    pub window_id: i8,
    pub state_id: i32,
    pub slot: i16,
    pub item: Option<ItemStack>,
    pub item_id: Option<u32>,
}

impl ContainerSetSlotPacket {
    /// Creates a new `ContainerSetSlotPacket`.
    pub const fn new(
        window_id: i8,
        state_id: i32,
        slot: i16,
        item: Option<ItemStack>,
        item_id: Option<u32>,
    ) -> Self {
        Self {
            window_id,
            state_id,
            slot,
            item,
            item_id,
        }
    }

    /// Player inventory slot update (`window_id = 0`).
    pub fn player_inventory(slot: i16, item: Option<ItemStack>, item_id: Option<u32>) -> Self {
        Self::new(0, 0, slot, item, item_id)
    }

    /// Encodes into standard Java 26.3 framed packet bytes:
    /// `VarInt(len) + VarInt(packet_id=20) + VarInt(window_id) + VarInt(state_id)
    /// + Short(slot) + Slot`.
    pub fn encode(&self) -> Result<Vec<u8>, UnresolvedItemId> {
        let mut body = Vec::new();
        put_varint(i32::from(self.window_id) as u32, &mut body);
        put_varint(self.state_id as u32, &mut body);
        body.extend_from_slice(&self.slot.to_be_bytes());
        match &self.item {
            None => put_varint(0, &mut body),
            Some(stack) => {
                put_varint(u32::from(stack.count()), &mut body);
                let item_id = self.item_id.ok_or_else(|| UnresolvedItemId {
                    item: stack.item().to_owned(),
                })?;
                put_varint(item_id, &mut body);
                put_varint(0, &mut body);
                put_varint(0, &mut body);
            }
        }
        Ok(frame(CONTAINER_SET_SLOT_PACKET_ID, &body))
    }
}

/// Rollback packet set emitted when a `BlockBreakEvent` is cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakRollback {
    /// `None` when this chunk does not own the position, so the server holds no
    /// authoritative state to restore.
    pub block_update: Option<BlockUpdatePacket>,
    pub ack: Option<AcknowledgeBlockChangePacket>,
}

impl BreakRollback {
    /// Encodes all rollback packets to wire frames.
    pub fn to_packets(&self) -> Vec<Vec<u8>> {
        let mut packets: Vec<_> = self.block_update.iter().map(|u| u.encode()).collect();
        if let Some(ack) = &self.ack {
            packets.push(ack.encode());
        }
        packets
    }
}

/// Rollback packet set emitted when a `BlockPlaceEvent` is cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceRollback {
    /// `None` when this chunk does not own the position, so the server holds no
    /// authoritative state to restore.
    pub block_update: Option<BlockUpdatePacket>,
    pub inventory_sync: ContainerSetSlotPacket,
    pub ack: Option<AcknowledgeBlockChangePacket>,
}

impl PlaceRollback {
    /// Encodes all rollback packets to wire frames.
    pub fn to_packets(&self) -> Result<Vec<Vec<u8>>, UnresolvedItemId> {
        let mut packets: Vec<_> = self.block_update.iter().map(|u| u.encode()).collect();
        packets.push(self.inventory_sync.encode()?);
        if let Some(ack) = &self.ack {
            packets.push(ack.encode());
        }
        Ok(packets)
    }
}

/// Produces authoritative break rollback packets restoring the original block state.
pub fn rollback_break(event: &BlockBreakEvent) -> Option<BreakRollback> {
    rollback_break_explicit(event, block_default_state_id(event.block), None)
}

/// Produces authoritative break rollback packets with sequence acknowledgement.
pub fn rollback_break_with_ack(event: &BlockBreakEvent, sequence: i32) -> Option<BreakRollback> {
    rollback_break_explicit(event, block_default_state_id(event.block), Some(sequence))
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
        block_update: Some(BlockUpdatePacket::new(event.pos, state_id)),
        ack: sequence.map(AcknowledgeBlockChangePacket::new),
    })
}

/// Produces authoritative place rollback packets restoring air and syncing client inventory.
pub fn rollback_place(event: &BlockPlaceEvent, inventory_slot: i16) -> Option<PlaceRollback> {
    rollback_place_explicit(
        event,
        block_default_state_id(Block::Air),
        inventory_slot,
        None,
    )
}

/// Produces authoritative place rollback packets with sequence acknowledgement.
pub fn rollback_place_with_ack(
    event: &BlockPlaceEvent,
    inventory_slot: i16,
    sequence: i32,
) -> Option<PlaceRollback> {
    rollback_place_explicit(
        event,
        block_default_state_id(Block::Air),
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
        block_update: Some(BlockUpdatePacket::new(event.placed_pos, air_state_id)),
        inventory_sync: ContainerSetSlotPacket::player_inventory(
            inventory_slot,
            event.item_in_hand.clone(),
            None,
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
    pub remaining_item: Option<ItemStack>,
}

/// Reads the block at `pos`, or `None` when this chunk does not own that cell.
fn owned_cell(chunk: &Chunk, pos: BlockPos) -> Option<Block> {
    if chunk_coords_of(pos) != (chunk.x, chunk.z) || !(0..WORLD_HEIGHT as i32).contains(&pos.y) {
        return None;
    }
    let (lx, y, lz) = local_coords_of(pos);
    debug_assert!(lx < CHUNK_SIDE && lz < CHUNK_SIDE);
    chunk.block(lx, y, lz)
}

/// Authoritative block break execution pipeline.
pub fn apply_block_break(
    bus: &EventBus,
    chunk: &mut Chunk,
    player_id: u32,
    pos: BlockPos,
    sequence: Option<i32>,
    tool: Option<ItemStack>,
    default_drops: Vec<ItemStack>,
) -> Result<BlockBreakOutcome, BreakRollback> {
    let ack = sequence.map(AcknowledgeBlockChangePacket::new);
    let Some(current_block) = owned_cell(chunk, pos) else {
        return Err(BreakRollback {
            block_update: None,
            ack,
        });
    };
    if current_block == Block::Air {
        return Err(BreakRollback {
            block_update: Some(BlockUpdatePacket::new(
                pos,
                block_default_state_id(Block::Air),
            )),
            ack,
        });
    }

    let mut event = BlockBreakEvent::new(player_id, pos, current_block, tool, default_drops);
    if bus.dispatch(&mut event).is_err() {
        event.cancel();
    }

    if event.is_cancelled() {
        Err(
            rollback_break_explicit(&event, block_default_state_id(current_block), sequence)
                .expect("rollback for cancelled break"),
        )
    } else {
        chunk.set_world_block(
            i64::from(pos.x),
            i64::from(pos.y),
            i64::from(pos.z),
            Block::Air,
        );
        Ok(BlockBreakOutcome {
            player_id,
            pos,
            original_block: current_block,
            drops: event.drops,
        })
    }
}

/// Inputs for [`apply_block_place`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPlaceRequest {
    pub player_id: u32,
    pub placed_pos: BlockPos,
    pub placed_against: BlockPos,
    pub placed_block: Block,
    pub item_in_hand: Option<ItemStack>,
    pub inventory_slot: i16,
    pub sequence: Option<i32>,
}

/// Authoritative block place execution pipeline.
pub fn apply_block_place(
    bus: &EventBus,
    chunk: &mut Chunk,
    request: BlockPlaceRequest,
) -> Result<BlockPlaceOutcome, PlaceRollback> {
    let BlockPlaceRequest {
        player_id,
        placed_pos,
        placed_against,
        placed_block,
        item_in_hand,
        inventory_slot,
        sequence,
    } = request;
    let Some(existing) = owned_cell(chunk, placed_pos) else {
        return Err(PlaceRollback {
            block_update: None,
            inventory_sync: ContainerSetSlotPacket::player_inventory(
                inventory_slot,
                item_in_hand,
                None,
            ),
            ack: sequence.map(AcknowledgeBlockChangePacket::new),
        });
    };

    let mut event = BlockPlaceEvent::new(
        player_id,
        placed_pos,
        placed_against,
        placed_block,
        item_in_hand,
    );
    if existing != Block::Air || event.item_in_hand.is_none() || bus.dispatch(&mut event).is_err() {
        event.cancel();
    }

    if event.is_cancelled() {
        Err(rollback_place_explicit(
            &event,
            block_default_state_id(existing),
            inventory_slot,
            sequence,
        )
        .expect("rollback for cancelled place"))
    } else {
        chunk.set_world_block(
            i64::from(placed_pos.x),
            i64::from(placed_pos.y),
            i64::from(placed_pos.z),
            placed_block,
        );
        let (_, remaining_item) = event
            .item_in_hand
            .expect("non-empty hand validated above")
            .split(1)
            .expect("splitting 1 from non-empty ItemStack succeeds");
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
    use crate::item::DEFAULT_MAX_STACK;
    use crate::java_wire::unpack_block_position;
    use std::sync::atomic::AtomicUsize;

    fn stack(item: &str, count: u16) -> ItemStack {
        ItemStack::try_new(item, count, DEFAULT_MAX_STACK).unwrap()
    }

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
            let packed = pack_block_position(pos.x, pos.y, pos.z);
            let (x, y, z) = unpack_block_position(packed);
            assert_eq!(pos, BlockPos::new(x, y, z), "Roundtrip failed for {pos:?}");
        }
    }

    #[test]
    fn block_pos_chunk_and_local_coords() {
        let pos1 = BlockPos::new(0, 64, 0);
        assert_eq!(chunk_coords_of(pos1), (0, 0));
        assert_eq!(local_coords_of(pos1), (0, 64, 0));

        let pos2 = BlockPos::new(15, 64, 15);
        assert_eq!(chunk_coords_of(pos2), (0, 0));
        assert_eq!(local_coords_of(pos2), (15, 64, 15));

        let pos3 = BlockPos::new(16, 64, 16);
        assert_eq!(chunk_coords_of(pos3), (1, 1));
        assert_eq!(local_coords_of(pos3), (0, 64, 0));

        let pos_neg = BlockPos::new(-1, 64, -1);
        assert_eq!(chunk_coords_of(pos_neg), (-1, -1));
        assert_eq!(local_coords_of(pos_neg), (15, 64, 15));

        let pos_neg16 = BlockPos::new(-16, 64, -16);
        assert_eq!(chunk_coords_of(pos_neg16), (-1, -1));
        assert_eq!(local_coords_of(pos_neg16), (0, 64, 0));
    }

    #[test]
    fn direction_offsets_and_opposites() {
        assert_eq!(Direction::Down.offset(10, 20, 30), (10, 19, 30));
        assert_eq!(Direction::Up.offset(10, 20, 30), (10, 21, 30));
        assert_eq!(Direction::North.offset(10, 20, 30), (10, 20, 29));
        assert_eq!(Direction::South.offset(10, 20, 30), (10, 20, 31));
        assert_eq!(Direction::West.offset(10, 20, 30), (9, 20, 30));
        assert_eq!(Direction::East.offset(10, 20, 30), (11, 20, 30));

        for dir in Direction::ALL {
            assert_eq!(dir.opposite().opposite(), dir);
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
            assert_eq!(event.tool, Some(stack("minecraft:iron_pickaxe", 1)));
            assert_eq!(event.drops.len(), 1);

            event.set_drops(vec![
                stack("minecraft:cobblestone", 2),
                stack("minecraft:raw_iron", 1),
            ]);
        });

        let mut event = BlockBreakEvent::new(
            42,
            BlockPos::new(10, 64, 10),
            Block::Stone,
            Some(stack("minecraft:iron_pickaxe", 1)),
            vec![stack("minecraft:cobblestone", 1)],
        );

        assert!(!event.is_cancelled());
        bus.dispatch_break(&mut event).unwrap();

        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_eq!(event.drops.len(), 2);
        assert_eq!(event.drops[0].count(), 2);
        assert_eq!(event.drops[1].item(), "minecraft:raw_iron");
        assert!(!event.is_cancelled());
    }

    #[test]
    fn priority_execution_order_verification() {
        let bus = EventBus::new();
        let log = Arc::new(Mutex::new(Vec::new()));

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
        bus.dispatch_break(&mut event).unwrap();

        let executed = log.lock().unwrap().clone();
        assert_eq!(
            executed,
            vec![
                EventPriority::Lowest,
                EventPriority::Low,
                EventPriority::Normal,
                EventPriority::High,
                EventPriority::Highest,
                EventPriority::Monitor,
            ]
        );
    }

    #[test]
    fn cancellation_stops_subsequent_non_monitor_handlers() {
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
        bus.dispatch_break(&mut event).unwrap();

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

        bus.register_break(EventPriority::Normal, |event| {
            event.cancel();
        });

        bus.register_break(EventPriority::Monitor, move |event| {
            if event.is_cancelled() {
                obs_clone.fetch_add(1, Ordering::SeqCst);
            }
            event.uncancel();
        });

        let mut event =
            BlockBreakEvent::new(1, BlockPos::new(0, 0, 0), Block::Stone, None, Vec::new());
        bus.dispatch_break(&mut event).unwrap();

        assert_eq!(observed_cancelled.load(Ordering::SeqCst), 1);
        assert!(event.is_cancelled());
    }

    #[test]
    fn reentrant_dispatch_is_rejected_without_deadlock() {
        let bus = EventBus::new();
        let bus_in_listener = bus.clone();
        let nested_errors = Arc::new(Mutex::new(Vec::new()));
        let errs_clone = Arc::clone(&nested_errors);

        bus.register_break(EventPriority::Normal, move |_| {
            let mut nested_same =
                BlockBreakEvent::new(2, BlockPos::new(1, 64, 1), Block::Stone, None, Vec::new());
            errs_clone
                .lock()
                .unwrap()
                .push(bus_in_listener.dispatch_break(&mut nested_same));

            let mut nested_other = PlayerInteractEvent::new(
                2,
                None,
                None,
                InteractionHand::MainHand,
                InteractAction::LeftClickAir,
            );
            errs_clone
                .lock()
                .unwrap()
                .push(bus_in_listener.dispatch_interact(&mut nested_other));
        });

        let mut outer = BlockBreakEvent::new(1, BlockPos::new(0, 64, 0), Block::Dirt, None, vec![]);
        assert_eq!(bus.dispatch_break(&mut outer), Ok(()));
        assert_eq!(
            *nested_errors.lock().unwrap(),
            vec![Err(DispatchError::Reentrant), Err(DispatchError::Reentrant)]
        );

        // Guard was released after outer dispatch, so subsequent dispatches succeed.
        let mut after = PlayerInteractEvent::new(
            1,
            None,
            None,
            InteractionHand::MainHand,
            InteractAction::RightClickAir,
        );
        assert_eq!(bus.dispatch_interact(&mut after), Ok(()));
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
            Some(stack("minecraft:bedrock", 1)),
        );

        assert!(!event.is_cancelled());
        bus.dispatch_place(&mut event).unwrap();
        assert!(event.is_cancelled());
        assert_eq!(was_cancelled.load(Ordering::SeqCst), 1);

        let rollback = event.rollback(36).expect("rollback packet set");
        let block_update = rollback.block_update.as_ref().expect("owned cell");
        assert_eq!(block_update.pos, BlockPos::new(1, 65, 1));
        assert_eq!(block_update.block_state_id, 0);
        assert_eq!(rollback.inventory_sync.slot, 36);
        assert_eq!(
            rollback.inventory_sync.item.as_ref().unwrap().item(),
            "minecraft:bedrock"
        );

        assert!(rollback.to_packets().is_err());
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
                assert_eq!(event.face, Some(Direction::Up));
                event.cancel();
                seen_clone.fetch_add(1, Ordering::SeqCst);
            }
        });

        let mut event = PlayerInteractEvent::new(
            123,
            Some(BlockPos::new(5, 64, 5)),
            Some(Direction::Up),
            InteractionHand::MainHand,
            InteractAction::RightClickBlock,
        );

        assert!(!event.is_cancelled());
        bus.dispatch_interact(&mut event).unwrap();
        assert!(event.is_cancelled());
        assert_eq!(interact_seen.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rollback_packets_wire_encoding() {
        assert_eq!(
            BlockUpdatePacket::new(BlockPos::new(1, 64, 1), 9).encode(),
            [
                0x0A, 0x08, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x10, 0x40, 0x09
            ]
        );
        assert_eq!(
            AcknowledgeBlockChangePacket::new(42).encode(),
            [0x02, 0x04, 0x2A]
        );
        assert_eq!(
            AcknowledgeBlockChangePacket::new(300).encode(),
            [0x03, 0x04, 0xAC, 0x02]
        );
        assert_eq!(
            ContainerSetSlotPacket::player_inventory(
                36,
                Some(stack("minecraft:stone", 1)),
                Some(7),
            )
            .encode(),
            Ok(vec![
                0x09, 0x14, 0x00, 0x00, 0x00, 0x24, 0x01, 0x07, 0x00, 0x00
            ])
        );
        assert_eq!(
            ContainerSetSlotPacket::new(3, 300, 1, Some(stack("minecraft:stone", 2)), Some(300),)
                .encode(),
            Ok(vec![
                0x0B, 0x14, 0x03, 0xAC, 0x02, 0x00, 0x01, 0x02, 0xAC, 0x02, 0x00, 0x00
            ])
        );
    }

    #[test]
    fn empty_slot_stops_after_the_count() {
        assert_eq!(
            ContainerSetSlotPacket::player_inventory(36, None, None).encode(),
            Ok(vec![0x06, 0x14, 0x00, 0x00, 0x00, 0x24, 0x00])
        );
    }

    #[test]
    fn nonempty_slot_without_an_item_id_is_not_written() {
        assert_eq!(
            ContainerSetSlotPacket::player_inventory(36, Some(stack("minecraft:stone", 1)), None)
                .encode(),
            Err(UnresolvedItemId {
                item: "minecraft:stone".to_owned()
            })
        );
    }

    #[test]
    fn position_matches_the_documented_example() {
        let pos = BlockPos::new(18_357_644, 831, -20_882_616);
        assert_eq!(
            pack_block_position(pos.x, pos.y, pos.z),
            0x4607_632C_15B4_833F
        );
        let (x, y, z) = unpack_block_position(0x4607_632C_15B4_833F);
        assert_eq!(BlockPos::new(x, y, z), pos);
    }

    #[test]
    fn default_state_ids_match_the_26_3_registry_report() {
        for (block, state) in [
            (Block::Air, 0),
            (Block::Stone, 1),
            (Block::Grass, 9),
            (Block::Dirt, 10),
            (Block::Bedrock, 88),
            (Block::Log, 140),
            (Block::Snow, 8598),
            (Block::Terracotta, 14759),
            (Block::ShortGrass, 2363),
            (Block::SnowLayer, 8589),
            (Block::Carpet, 14743),
            (Block::TallGrassLower, 14771),
            (Block::TallGrassUpper, 14770),
        ] {
            assert_eq!(block_default_state_id(block), state, "{block:?}");
        }
        for block in Block::ALL {
            let _ = block_default_state_id(block);
        }
    }

    #[test]
    fn panicked_listener_keeps_vetoing_later_events() {
        let bus = EventBus::new();
        let first_call = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&first_call);
        bus.register_break(EventPriority::Normal, move |event| {
            event.cancel();
            assert_ne!(calls.fetch_add(1, Ordering::SeqCst), 0, "first call panics");
        });

        let mut event = BlockBreakEvent::new(1, BlockPos::new(0, 0, 0), Block::Dirt, None, vec![]);
        let bus_clone = bus.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = bus_clone.dispatch_break(&mut event);
        }));
        assert!(result.is_err());

        let mut event = BlockBreakEvent::new(1, BlockPos::new(0, 0, 0), Block::Dirt, None, vec![]);
        bus.dispatch_break(&mut event).unwrap();
        assert!(event.is_cancelled());
    }

    fn stone_at_origin() -> Chunk {
        let mut chunk = Chunk::empty(0, 0);
        chunk.set_world_block(0, 64, 0, Block::Stone);
        chunk
    }

    #[test]
    fn place_outside_the_chunk_keeps_the_item_and_acknowledges() {
        let bus = EventBus::new();
        let mut chunk = stone_at_origin();
        for pos in [
            BlockPos::new(16, 65, 0),
            BlockPos::new(0, -1, 0),
            BlockPos::new(0, WORLD_HEIGHT as i32, 0),
        ] {
            let rollback = apply_block_place(
                &bus,
                &mut chunk,
                BlockPlaceRequest {
                    player_id: 1,
                    placed_pos: pos,
                    placed_against: BlockPos::new(0, 64, 0),
                    placed_block: Block::Dirt,
                    item_in_hand: Some(stack("minecraft:dirt", 5)),
                    inventory_slot: 36,
                    sequence: Some(9),
                },
            )
            .expect_err("unowned cell");
            assert_eq!(rollback.block_update, None);
            assert_eq!(rollback.inventory_sync.item.as_ref().unwrap().count(), 5);
            assert_eq!(rollback.ack, Some(AcknowledgeBlockChangePacket::new(9)));
        }
    }

    #[test]
    fn place_on_an_occupied_cell_restores_it_without_dispatching() {
        let bus = EventBus::new();
        let dispatched = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&dispatched);
        bus.register_place(EventPriority::Monitor, move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        let mut chunk = stone_at_origin();

        let rollback = apply_block_place(
            &bus,
            &mut chunk,
            BlockPlaceRequest {
                player_id: 1,
                placed_pos: BlockPos::new(0, 64, 0),
                placed_against: BlockPos::new(0, 63, 0),
                placed_block: Block::Dirt,
                item_in_hand: Some(stack("minecraft:dirt", 5)),
                inventory_slot: 36,
                sequence: None,
            },
        )
        .expect_err("occupied cell");

        assert_eq!(chunk.block(0, 64, 0), Some(Block::Stone));
        assert_eq!(
            rollback.block_update,
            Some(BlockUpdatePacket::new(
                BlockPos::new(0, 64, 0),
                block_default_state_id(Block::Stone)
            ))
        );
        assert_eq!(dispatched.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn place_with_an_empty_hand_is_rejected() {
        let bus = EventBus::new();
        let mut chunk = stone_at_origin();
        apply_block_place(
            &bus,
            &mut chunk,
            BlockPlaceRequest {
                player_id: 1,
                placed_pos: BlockPos::new(0, 65, 0),
                placed_against: BlockPos::new(0, 64, 0),
                placed_block: Block::Dirt,
                item_in_hand: None,
                inventory_slot: 36,
                sequence: None,
            },
        )
        .expect_err("nothing to place");
        assert_eq!(chunk.block(0, 65, 0), Some(Block::Air));
    }

    #[test]
    fn break_rejections_acknowledge_the_sequence() {
        let bus = EventBus::new();
        let mut chunk = stone_at_origin();

        let foreign = apply_block_break(
            &bus,
            &mut chunk,
            1,
            BlockPos::new(16, 64, 0),
            Some(3),
            None,
            vec![],
        )
        .expect_err("unowned cell");
        assert_eq!(foreign.to_packets(), vec![vec![0x02, 0x04, 0x03]]);

        let air = apply_block_break(
            &bus,
            &mut chunk,
            1,
            BlockPos::new(0, 65, 0),
            Some(4),
            None,
            vec![stack("minecraft:stone", 1)],
        )
        .expect_err("nothing to break");
        assert_eq!(air.ack, Some(AcknowledgeBlockChangePacket::new(4)));

        bus.register_break(EventPriority::Normal, |event| event.cancel());
        let cancelled = apply_block_break(
            &bus,
            &mut chunk,
            1,
            BlockPos::new(0, 64, 0),
            Some(5),
            None,
            vec![],
        )
        .expect_err("cancelled");
        assert_eq!(cancelled.ack, Some(AcknowledgeBlockChangePacket::new(5)));
    }

    #[test]
    fn world_mutation_integration_break_allowed_and_cancelled() {
        let bus = EventBus::new();
        let mut chunk = Chunk::empty(0, 0);
        chunk.set_world_block(5, 64, 5, Block::Stone);

        let outcome = apply_block_break(
            &bus,
            &mut chunk,
            1,
            BlockPos::new(5, 64, 5),
            None,
            None,
            vec![stack("minecraft:cobblestone", 1)],
        )
        .expect("break allowed");
        assert_eq!(outcome.original_block, Block::Stone);
        assert_eq!(chunk.block(5, 64, 5), Some(Block::Air));

        chunk.set_world_block(5, 64, 5, Block::Stone);
        bus.register_break(EventPriority::Normal, |e| e.cancel());

        let err = apply_block_break(
            &bus,
            &mut chunk,
            1,
            BlockPos::new(5, 64, 5),
            None,
            None,
            vec![],
        )
        .expect_err("break cancelled");

        assert_eq!(chunk.block(5, 64, 5), Some(Block::Stone));
        assert_eq!(
            err.block_update,
            Some(BlockUpdatePacket::new(
                BlockPos::new(5, 64, 5),
                block_default_state_id(Block::Stone)
            ))
        );
    }

    #[test]
    fn world_mutation_integration_place_allowed_and_cancelled() {
        let bus = EventBus::new();
        let mut chunk = Chunk::empty(0, 0);
        assert_eq!(chunk.block(2, 65, 2), Some(Block::Air));

        let outcome = apply_block_place(
            &bus,
            &mut chunk,
            BlockPlaceRequest {
                player_id: 1,
                placed_pos: BlockPos::new(2, 65, 2),
                placed_against: BlockPos::new(2, 64, 2),
                placed_block: Block::Sandstone,
                item_in_hand: Some(stack("minecraft:sandstone", 5)),
                inventory_slot: 37,
                sequence: None,
            },
        )
        .expect("place allowed");

        assert_eq!(outcome.remaining_item.unwrap().count(), 4);
        assert_eq!(chunk.block(2, 65, 2), Some(Block::Sandstone));

        bus.register_place(EventPriority::Normal, |e| {
            e.cancel();
        });

        let err = apply_block_place(
            &bus,
            &mut chunk,
            BlockPlaceRequest {
                player_id: 1,
                placed_pos: BlockPos::new(3, 65, 3),
                placed_against: BlockPos::new(3, 64, 3),
                placed_block: Block::Sandstone,
                item_in_hand: Some(stack("minecraft:sandstone", 4)),
                inventory_slot: 37,
                sequence: None,
            },
        )
        .expect_err("place cancelled");

        assert_eq!(chunk.block(3, 65, 3), Some(Block::Air));
        assert_eq!(
            err.block_update,
            Some(BlockUpdatePacket::new(BlockPos::new(3, 65, 3), 0))
        );
        assert_eq!(err.inventory_sync.item.as_ref().unwrap().count(), 4);
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

        let mut event = BlockBreakEvent::new(1, BlockPos::new(0, 0, 0), Block::Stone, None, vec![]);
        bus.dispatch_break(&mut event).unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 11);

        assert!(bus.unregister(id1));
        assert!(!bus.unregister(id1));
        assert_eq!(bus.listener_count::<BlockBreakEvent>(), 1);

        count.store(0, Ordering::SeqCst);
        bus.dispatch_break(&mut event).unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 10);

        bus.clear();
        assert_eq!(bus.listener_count::<BlockBreakEvent>(), 0);
        count.store(0, Ordering::SeqCst);
        bus.dispatch_break(&mut event).unwrap();
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

        let mut event = BlockBreakEvent::new(1, BlockPos::new(0, 0, 0), Block::Dirt, None, vec![]);
        bus2.dispatch_break(&mut event).unwrap();

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
                Some(Direction::Up),
                InteractionHand::OffHand,
                action,
            );
            bus.dispatch_interact(&mut event).unwrap();
        }

        assert_eq!(actions_seen.lock().unwrap().clone(), actions.to_vec());
    }

    #[test]
    fn cancelled_place_event_cannot_be_uncancelled_by_later_or_monitor_listeners() {
        let bus = EventBus::new();
        let later_non_monitor_ran = Arc::new(AtomicUsize::new(0));

        bus.register_place(EventPriority::Low, |event| {
            event.cancel();
        });

        let same_priority_ran = Arc::clone(&later_non_monitor_ran);
        bus.register_place(EventPriority::Low, move |event| {
            same_priority_ran.fetch_add(1, Ordering::SeqCst);
            event.uncancel();
        });

        let highest_ran = Arc::clone(&later_non_monitor_ran);
        bus.register_place(EventPriority::Highest, move |event| {
            highest_ran.fetch_add(1, Ordering::SeqCst);
            event.set_cancelled(false);
        });

        bus.register_place(EventPriority::Monitor, |event| {
            assert!(event.is_cancelled());
            event.uncancel();
        });

        let mut event = BlockPlaceEvent::new(
            1,
            BlockPos::new(0, 65, 0),
            BlockPos::new(0, 64, 0),
            Block::Dirt,
            Some(stack("minecraft:dirt", 1)),
        );
        bus.dispatch_place(&mut event).unwrap();

        assert_eq!(later_non_monitor_ran.load(Ordering::SeqCst), 0);
        assert!(event.is_cancelled());
    }
}
