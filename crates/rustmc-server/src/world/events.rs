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
        Arc, Mutex, MutexGuard, PoisonError,
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

    fn lock_listeners(&self) -> MutexGuard<'_, BTreeMap<TypeId, Vec<RegisteredListener>>> {
        // Callers only insert, remove, or clone listener Arcs under this lock, so the map remains valid if a listener panics elsewhere.
        self.inner
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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
        let mut map = self.lock_listeners();
        let list = map.entry(type_id).or_default();
        list.push(listener);
        list.sort_by_key(|l| l.priority);
        id
    }

    /// Unregisters a listener by ID.
    pub fn unregister(&self, listener_id: u64) -> bool {
        let mut map = self.lock_listeners();
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
        self.lock_listeners().clear();
    }

    /// Returns the number of registered listeners for event type `E`.
    pub fn listener_count<E: Event + 'static>(&self) -> usize {
        let type_id = TypeId::of::<E>();
        let map = self.lock_listeners();
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
            let map = self.lock_listeners();
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

mod rollback;
pub use rollback::*;

#[cfg(test)]
mod tests;
