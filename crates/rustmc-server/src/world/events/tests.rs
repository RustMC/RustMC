//! Unit tests for `world::events`.

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

    let mut event = BlockBreakEvent::new(1, BlockPos::new(0, 0, 0), Block::Dirt, None, Vec::new());
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

    let mut event = BlockBreakEvent::new(1, BlockPos::new(0, 0, 0), Block::Dirt, None, Vec::new());
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

    let mut event = BlockBreakEvent::new(1, BlockPos::new(0, 0, 0), Block::Stone, None, Vec::new());
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
        ContainerSetSlotPacket::player_inventory(36, Some(stack("minecraft:stone", 1)), Some(7),)
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
