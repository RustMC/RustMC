//! Entity mechanics, dropped item physics, and world interactions.

pub mod item;

pub use item::{
    AIR_DRAG_HORIZONTAL, AIR_DRAG_VERTICAL, Aabb, BLOCK_BREAK_PICKUP_DELAY, DESPAWN_TICKS,
    GROUND_FRICTION, ITEM_GRAVITY, ITEM_HEIGHT, ITEM_TERMINAL_VELOCITY, ITEM_WIDTH, ItemEntity,
    ItemEntityManager, ItemPickupEvent, ItemPickupResult, ItemStack, MAX_STACK_SIZE,
    PERMANENT_PICKUP_DELAY, PICKUP_REACH_HORIZONTAL, PICKUP_REACH_VERTICAL,
    PLAYER_DROP_PICKUP_DELAY, PlayerInventory, STACK_MERGE_RADIUS, SimpleVoxelWorld,
    TAKE_ITEM_ENTITY_PACKET_ID, TakeItemEntityPacket, VOID_DESPAWN_Y, WorldCollisionView,
    merge_nearby_stacks,
};
