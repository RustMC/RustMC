//! Unit tests for `entity::item`.

use super::*;
use crate::item::DEFAULT_MAX_STACK;
use crate::world::physics::SimpleVoxelWorld;
use crate::world::{Block, Chunk};

fn stack(item: &str, count: u16) -> ItemStack {
    let max_stack = match item {
        "minecraft:ender_pearl" => 16,
        "minecraft:potion" => 1,
        _ => DEFAULT_MAX_STACK,
    };
    ItemStack::try_new(item, count, max_stack).unwrap()
}

#[test]
fn test_freefall_gravity_and_terminal_velocity() {
    let mut item = ItemEntity::new(1, [0.0, 100.0, 0.0], stack("minecraft:dirt", 1));
    assert_eq!(item.velocity(), [0.0, 0.0, 0.0]);

    item.tick(&());
    let expected_vy = (0.0 - ITEM_GRAVITY) * AIR_DRAG_VERTICAL;
    assert!((item.velocity()[1] - expected_vy).abs() < 1e-6);
    assert!(!item.on_ground());
    assert!(item.position()[1] < 100.0);

    let prev_vy = item.velocity()[1];
    item.tick(&());
    assert!(item.velocity()[1] < prev_vy);

    let mut fast_item = ItemEntity::new(2, [0.0, 100.0, 0.0], stack("minecraft:dirt", 1))
        .with_velocity([0.0, -10.0, 0.0]);
    fast_item.tick(&());
    assert!(fast_item.velocity()[1] >= -2.0);
    assert_eq!(fast_item.velocity()[1], -2.0 * AIR_DRAG_VERTICAL);

    for _ in 0..200 {
        item.tick(&());
        assert!(item.velocity()[1] >= -2.0);
    }
}

#[test]
fn test_landing_on_solid_floor_and_friction() {
    let mut world = SimpleVoxelWorld::new();
    for x in -2..=2 {
        for z in -2..=2 {
            world.add_solid_block(x, 0, z);
        }
    }

    let mut item = ItemEntity::new(1, [0.0, 1.5, 0.0], stack("minecraft:iron_ingot", 4))
        .with_velocity([1.0, -0.6, 0.0]);

    item.tick(&world);
    assert!(item.on_ground(), "Item should land on top of the block");
    assert_eq!(item.position()[1], 1.0);
    assert_eq!(item.velocity()[1], 0.0);

    // Ground friction derived from default block slipperiness 0.6 * air drag 0.98 = 0.588
    let vx_after_landing = item.velocity()[0];
    assert!(
        (vx_after_landing - (1.0 * GROUND_FRICTION)).abs() < 1e-6,
        "Expected ground friction {GROUND_FRICTION}"
    );
    assert!((GROUND_FRICTION - 0.588).abs() < 1e-12);

    item.tick(&world);
    assert!(item.on_ground());
    assert_eq!(item.position()[1], 1.0);
    assert_eq!(item.velocity()[1], 0.0);
    assert!(item.velocity()[0] < vx_after_landing);
}

#[test]
fn test_stack_merging_within_box_expansion() {
    let mut item1 = ItemEntity::new(1, [0.0, 10.0, 0.0], stack("minecraft:dirt", 25))
        .with_age(100)
        .with_pickup_delay(5);
    let mut item2 = ItemEntity::new(2, [0.5, 10.0, 0.0], stack("minecraft:dirt", 35))
        .with_age(50)
        .with_pickup_delay(15);

    assert!(item1.can_merge_with(&item2));
    assert!(item1.try_merge(&mut item2));

    assert_eq!(item1.count(), 60);
    assert_eq!(item1.age(), 50);
    assert_eq!(item1.pickup_delay(), 15);
    assert!(item1.is_alive());

    assert_eq!(item2.count(), 0);
    assert!(item2.is_dead());

    let mut item3 = ItemEntity::new(3, [0.2, 10.0, 0.0], stack("minecraft:dirt", 4));
    assert!(item1.try_merge(&mut item3));
    assert_eq!(item1.count(), 64);
    assert!(item3.is_dead());
}

#[test]
fn test_merge_box_0_5_x_0_25_x_0_5_boundaries() {
    // Item boxes are 0.25 x 0.25 x 0.25, so horizontal reach is 0.25 + 0.5 = 0.75
    // center-to-center and vertical reach is 0.25 + 0.25 = 0.50 center-to-center.
    let center = ItemEntity::new(1, [0.0, 10.0, 0.0], stack("minecraft:dirt", 1));
    let inside_x = ItemEntity::new(2, [0.74, 10.0, 0.0], stack("minecraft:dirt", 1));
    let outside_x = ItemEntity::new(3, [0.76, 10.0, 0.0], stack("minecraft:dirt", 1));
    let inside_y = ItemEntity::new(4, [0.0, 10.49, 0.0], stack("minecraft:dirt", 1));
    let outside_y = ItemEntity::new(5, [0.0, 10.51, 0.0], stack("minecraft:dirt", 1));

    assert!(center.can_merge_with(&inside_x));
    assert!(!center.can_merge_with(&outside_x));
    assert!(center.can_merge_with(&inside_y));
    assert!(!center.can_merge_with(&outside_y));
}

#[test]
fn test_per_item_max_stack_respected_for_merge_and_pickup() {
    // Ender pearls cap at 16, not 64
    let mut pearl1 = ItemEntity::new(1, [0.0, 10.0, 0.0], stack("minecraft:ender_pearl", 10));
    let mut pearl2 = ItemEntity::new(2, [0.2, 10.0, 0.0], stack("minecraft:ender_pearl", 7));
    assert!(!pearl1.can_merge_with(&pearl2));
    assert!(!pearl1.try_merge(&mut pearl2));

    let mut pearl3 = ItemEntity::new(3, [0.2, 10.0, 0.0], stack("minecraft:ender_pearl", 6));
    assert!(pearl1.try_merge(&mut pearl3));
    assert_eq!(pearl1.count(), 16);

    let mut inv = PlayerInventory::new();
    let mut drop1 = ItemEntity::new(4, [0.0, 10.0, 0.0], stack("minecraft:ender_pearl", 12));
    let mut drop2 = ItemEntity::new(5, [0.0, 10.0, 0.0], stack("minecraft:ender_pearl", 10));
    drop1.try_collect(
        1,
        &Aabb::from_entity_pos([0.0, 10.0, 0.0], 0.6, 1.8),
        &mut inv,
    );
    drop2.try_collect(
        1,
        &Aabb::from_entity_pos([0.0, 10.0, 0.0], 0.6, 1.8),
        &mut inv,
    );
    assert_eq!(inv.get_slot(0).unwrap().count(), 16);
    assert_eq!(inv.get_slot(1).unwrap().count(), 6);
}

#[test]
fn test_rejection_of_stack_merging() {
    let item1 = ItemEntity::new(1, [0.0, 10.0, 0.0], stack("minecraft:dirt", 10));
    let mut item2 = ItemEntity::new(2, [1.1, 10.0, 0.0], stack("minecraft:dirt", 10));
    assert!(!item1.can_merge_with(&item2));
    let mut item1_mut = item1.clone();
    assert!(!item1_mut.try_merge(&mut item2));
    assert_eq!(item1_mut.count(), 10);
    assert_eq!(item2.count(), 10);

    let mut item3 = ItemEntity::new(3, [0.2, 10.0, 0.0], stack("minecraft:stone", 10));
    assert!(!item1.can_merge_with(&item3));
    assert!(!item1_mut.try_merge(&mut item3));

    let mut item4 = ItemEntity::new(4, [0.0, 10.0, 0.0], stack("minecraft:dirt", 40));
    let mut item5 = ItemEntity::new(5, [0.2, 10.0, 0.0], stack("minecraft:dirt", 30));
    assert!(!item4.can_merge_with(&item5));
    assert!(!item4.try_merge(&mut item5));
    assert_eq!(item4.count(), 40);
    assert_eq!(item5.count(), 30);

    let mut item6 = ItemEntity::new(6, [0.1, 10.0, 0.0], stack("minecraft:dirt", 5));
    item6.mark_dead();
    assert!(!item1.can_merge_with(&item6));

    assert!(!item1.can_merge_with(&item1));
}

fn total_count_of(inv: &PlayerInventory, item: &str) -> u32 {
    inv.slots()
        .iter()
        .flatten()
        .filter(|s| s.item() == item)
        .map(|s| u32::from(s.count()))
        .sum()
}

#[test]
fn test_pickup_delay_guard() {
    let mut inv = PlayerInventory::new();
    let mut item = ItemEntity::dropped_by_player(
        1,
        [0.0, 10.0, 0.0],
        [0.0, 0.0, 0.0],
        stack("minecraft:gold_ingot", 8),
    );

    assert_eq!(item.pickup_delay(), PLAYER_DROP_PICKUP_DELAY);
    assert!(!item.can_pickup());

    let player_pos = [0.0, 10.0, 0.0];
    let res = item.try_collect_at(100, player_pos, &mut inv);
    assert_eq!(res.collected_count, 0);
    assert_eq!(res.remaining_count, 8);
    assert!(res.event.is_none());
    assert_eq!(total_count_of(&inv, "minecraft:gold_ingot"), 0);

    for _ in 0..PLAYER_DROP_PICKUP_DELAY {
        item.tick_lifecycle();
    }
    assert_eq!(item.pickup_delay(), 0);
    assert!(item.can_pickup());

    let res = item.try_collect_at(100, player_pos, &mut inv);
    assert_eq!(res.collected_count, 8);
    assert_eq!(res.remaining_count, 0);
    assert!(item.is_dead());
    assert_eq!(total_count_of(&inv, "minecraft:gold_ingot"), 8);
}

#[test]
fn test_player_pickup_collection_full_transfer() {
    let mut inv = PlayerInventory::new();
    let mut item = ItemEntity::new(1, [5.0, 64.0, 5.0], stack("minecraft:diamond", 16));

    let player_pos = [5.1, 64.0, 5.0];
    let res = item.try_collect_at(42, player_pos, &mut inv);

    assert_eq!(res.collected_count, 16);
    assert_eq!(res.remaining_count, 0);
    assert!(item.is_dead());
    assert_eq!(inv.get_slot(0).unwrap().count(), 16);
    assert_eq!(inv.get_slot(0).unwrap().item(), "minecraft:diamond");

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
    for slot in 0..35 {
        inv.set_slot(slot, Some(stack("minecraft:cobblestone", 64)))
            .unwrap();
    }
    inv.set_slot(35, Some(stack("minecraft:diamond", 60)))
        .unwrap();

    let mut item = ItemEntity::new(5, [0.0, 1.0, 0.0], stack("minecraft:diamond", 10));

    let res = item.try_collect_at(99, [0.0, 1.0, 0.0], &mut inv);
    assert_eq!(res.collected_count, 4);
    assert_eq!(res.remaining_count, 6);
    assert_eq!(item.count(), 6);
    assert!(item.is_alive());

    assert_eq!(inv.get_slot(35).unwrap().count(), 64);
    let event = res.event.unwrap();
    assert_eq!(event.packet.pickup_item_count, 4);

    let res2 = item.try_collect_at(99, [0.0, 1.0, 0.0], &mut inv);
    assert_eq!(res2.collected_count, 0);
    assert_eq!(res2.remaining_count, 6);
    assert!(res2.event.is_none());
    assert!(item.is_alive());

    inv.set_slot(0, None).unwrap();
    let res3 = item.try_collect_at(99, [0.0, 1.0, 0.0], &mut inv);
    assert_eq!(res3.collected_count, 6);
    assert_eq!(res3.remaining_count, 0);
    assert!(item.is_dead());
    assert_eq!(inv.get_slot(0).unwrap().count(), 6);
}

#[test]
fn test_age_progression_and_despawn_threshold() {
    let mut item = ItemEntity::new(1, [0.0, 10.0, 0.0], stack("minecraft:apple", 1)).with_age(5998);

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
    for z in -5..=5 {
        world.add_solid_block(1, 0, z);
        world.add_solid_block(1, 1, z);
    }

    let mut item = ItemEntity::new(1, [0.5, 0.5, 0.0], stack("minecraft:dirt", 1))
        .with_velocity([0.8, 0.0, 0.5]);

    item.tick_physics(&world);

    assert!((item.position()[0] - 0.875).abs() < 1e-5);
    assert_eq!(item.velocity()[0], 0.0);

    assert!(item.position()[2] > 0.4);
    assert!(item.velocity()[2] > 0.0);
}

#[test]
fn test_take_item_entity_packet_wire_roundtrip() {
    let packet = TakeItemEntityPacket::new(1234, 5678, 32);
    let payload = packet.encode_payload();

    let decoded = TakeItemEntityPacket::decode_payload(&payload).unwrap();
    assert_eq!(decoded, packet);

    assert_eq!(
        packet.encode_packet(),
        [0x06, 0x7F, 0xD2, 0x09, 0xAE, 0x2C, 0x20]
    );
    assert!(TakeItemEntityPacket::decode_payload(&[0xFF, 0xFF, 0xFF, 0xFF, 0x1F]).is_err());
}

#[test]
fn test_pickup_box_extends_past_the_hitbox() {
    let hitbox = Aabb::from_min_max(0.0, 0.0, 0.0, 1.0, 2.0, 1.0);
    let item_at = |position| ItemEntity::new(1, position, stack("minecraft:dirt", 1));

    assert!(item_at([2.125, 1.0, 0.5]).in_pickup_reach(&hitbox));
    assert!(!item_at([2.25, 1.0, 0.5]).in_pickup_reach(&hitbox));
    assert!(item_at([0.5, 2.25, 0.5]).in_pickup_reach(&hitbox));
    assert!(!item_at([0.5, 2.5, 0.5]).in_pickup_reach(&hitbox));
}

#[test]
fn test_permanent_pickup_delay_never_counts_down() {
    let mut item = ItemEntity::new(1, [0.0, 10.0, 0.0], stack("minecraft:dirt", 1))
        .with_pickup_delay(PERMANENT_PICKUP_DELAY);
    item.tick_lifecycle();
    assert_eq!(item.pickup_delay(), PERMANENT_PICKUP_DELAY);
}

#[test]
fn test_items_below_the_void_limit_are_removed() {
    let mut item = ItemEntity::new(
        1,
        [0.0, VOID_DESPAWN_Y + 0.01, 0.0],
        stack("minecraft:dirt", 1),
    );
    item.tick(&());
    assert!(item.is_dead());
}

#[test]
fn test_the_larger_stack_survives_a_merge() {
    let mut items = [
        ItemEntity::new(1, [0.0, 10.0, 0.0], stack("minecraft:dirt", 3)),
        ItemEntity::new(2, [0.5, 10.0, 0.0], stack("minecraft:dirt", 10)),
    ];
    assert_eq!(merge_nearby_stacks(&mut items), 1);
    assert!(items[0].is_dead());
    assert_eq!(items[1].count(), 13);
}

#[test]
fn test_item_entity_manager_loop() {
    let mut manager = ItemEntityManager::new();

    manager.spawn(ItemEntity::new(
        1,
        [0.0, 10.0, 0.0],
        stack("minecraft:redstone", 10),
    ));
    manager.spawn(ItemEntity::new(
        2,
        [0.3, 10.0, 0.0],
        stack("minecraft:redstone", 15),
    ));
    assert_eq!(manager.count(), 2);

    manager.tick(&());
    assert_eq!(manager.count(), 1);
    assert_eq!(manager.items()[0].count(), 25);

    let mut inv = PlayerInventory::new();
    let events = manager.handle_player_pickup(10, [0.0, 10.0, 0.0], &mut inv);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].count, 25);
    assert_eq!(manager.count(), 0);
    assert_eq!(total_count_of(&inv, "minecraft:redstone"), 25);
}

#[test]
fn test_chunk_collision_view() {
    let mut chunk = Chunk::empty(0, 0);
    chunk.set_block(0, 0, 0, Block::Stone);

    let mut item = ItemEntity::new(1, [0.5, 1.2, 0.5], stack("minecraft:dirt", 1))
        .with_velocity([0.0, -0.5, 0.0]);

    item.tick(&chunk);
    assert!(item.on_ground());
    assert_eq!(item.position()[1], 1.0);
}

#[test]
fn test_block_break_pickup_delay() {
    let item = ItemEntity::dropped_by_block_break(
        1,
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        stack("minecraft:cobblestone", 1),
    );
    assert_eq!(item.pickup_delay(), BLOCK_BREAK_PICKUP_DELAY);
}
