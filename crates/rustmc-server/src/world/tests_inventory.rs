use super::inventory::*;
use crate::item::{DEFAULT_MAX_STACK, ItemStack};

fn stack(item: &str, count: u16) -> ItemStack {
    ItemStack::try_new(item, count, DEFAULT_MAX_STACK).unwrap()
}

#[test]
fn test_inventory_slot_bounds_and_held_selection() {
    let mut inv = PlayerInventory::new();

    // Initial state assertions
    assert_eq!(inv.held_slot(), 0);
    assert_eq!(inv.revision(), 0);
    assert_eq!(inv.total_item_count(), 0);
    assert!(inv.held_item().is_none());

    // All 36 slots must be initialized to empty
    for slot in 0..TOTAL_SLOT_COUNT {
        assert!(
            inv.get_slot(slot as u8).is_none(),
            "slot {slot} must start empty"
        );
    }

    // Out-of-bounds slot access returns None
    for invalid_slot in [36u8, 37, 45, 100, 255] {
        assert!(
            inv.get_slot(invalid_slot).is_none(),
            "slot {invalid_slot} out of bounds must return None"
        );
    }

    // Test valid hotbar selections (0..=8) and monotonic revision counter
    let mut expected_revision = 0;
    for hotbar_slot in 0..=8 {
        let res = inv.set_held_slot(hotbar_slot);
        assert!(res.is_ok(), "hotbar slot {hotbar_slot} must be valid");
        assert_eq!(inv.held_slot(), hotbar_slot);
        expected_revision += 1;
        assert_eq!(inv.revision(), expected_revision);
    }

    // Test invalid hotbar selections (>= 9) and atomicity (no state mutation)
    let last_valid_held = inv.held_slot();
    for invalid_hotbar in [9u8, 10, 35, 36, 128, 255] {
        let res = inv.set_held_slot(invalid_hotbar);
        assert_eq!(
            res,
            Err(InventoryError::InvalidHotbarSlot(invalid_hotbar)),
            "slot {invalid_hotbar} must be rejected as invalid hotbar slot"
        );
        assert_eq!(
            inv.held_slot(),
            last_valid_held,
            "held slot must not change on rejection"
        );
        assert_eq!(
            inv.revision(),
            expected_revision,
            "revision must not increment on rejection"
        );
    }

    // Test set_slot bounds and atomicity
    let valid_stack = stack("minecraft:torch", 16);
    assert!(inv.set_slot(35, Some(valid_stack.clone())).is_ok());
    expected_revision += 1;
    assert_eq!(inv.revision(), expected_revision);
    assert_eq!(inv.get_slot(35), Some(&valid_stack));

    // Attempt set_slot out of bounds
    assert_eq!(
        inv.set_slot(36, Some(valid_stack.clone())),
        Err(InventoryError::InvalidSlot(36))
    );
    assert_eq!(inv.revision(), expected_revision);
}

#[test]
fn test_inventory_atomic_swap_and_count_conservation() {
    let mut inv = PlayerInventory::new();

    // Populate test items across hotbar (0..=8) and main inventory (9..=35)
    let stone = stack("minecraft:cobblestone", 64);
    let torches = stack("minecraft:torch", 16);
    let sword = stack("minecraft:diamond_sword", 1);
    let planks = stack("minecraft:oak_planks", 32);
    let apples = stack("minecraft:apple", 5);

    inv.set_slot(0, Some(stone.clone())).unwrap();
    inv.set_slot(1, Some(torches.clone())).unwrap();
    inv.set_slot(8, Some(sword.clone())).unwrap();
    inv.set_slot(9, Some(planks.clone())).unwrap();
    inv.set_slot(35, Some(apples.clone())).unwrap();

    let initial_total: u64 = 64 + 16 + 1 + 32 + 5; // 118
    assert_eq!(inv.total_item_count(), initial_total);

    // 1. Swap within hotbar (0 and 1)
    let rev_before = inv.revision();
    inv.swap_slots(0, 1).unwrap();
    assert_eq!(inv.revision(), rev_before + 1);
    assert_eq!(inv.get_slot(0), Some(&torches));
    assert_eq!(inv.get_slot(1), Some(&stone));
    assert_eq!(
        inv.total_item_count(),
        initial_total,
        "total item count must be conserved across hotbar swap"
    );

    // 2. Swap between hotbar and main inventory (slot 1 [stone 64] and slot 35 [apple 5])
    inv.swap_slots(1, 35).unwrap();
    assert_eq!(inv.get_slot(1), Some(&apples));
    assert_eq!(inv.get_slot(35), Some(&stone));
    assert_eq!(
        inv.total_item_count(),
        initial_total,
        "total item count must be conserved across main/hotbar swap"
    );

    // 3. Swap non-empty slot with an empty slot (slot 2 [empty] and slot 0 [torches 16])
    assert!(inv.get_slot(2).is_none());
    inv.swap_slots(2, 0).unwrap();
    assert_eq!(inv.get_slot(2), Some(&torches));
    assert!(inv.get_slot(0).is_none());
    assert_eq!(
        inv.total_item_count(),
        initial_total,
        "total item count must be conserved when swapping with empty slot"
    );

    // 4. Swap slot with itself (slot 2 with slot 2)
    inv.swap_slots(2, 2).unwrap();
    assert_eq!(inv.get_slot(2), Some(&torches));
    assert_eq!(inv.total_item_count(), initial_total);

    // 5. Atomic failure rejection: invalid slot indices must NOT modify any state or count
    let snapshot_slots: Vec<Option<ItemStack>> = inv.slots().to_vec();
    let snapshot_rev = inv.revision();

    // slot_b out of bounds
    assert_eq!(inv.swap_slots(0, 36), Err(InventoryError::InvalidSlot(36)));
    assert_eq!(inv.revision(), snapshot_rev);
    assert_eq!(inv.slots(), snapshot_slots.as_slice());
    assert_eq!(inv.total_item_count(), initial_total);

    // slot_a out of bounds
    assert_eq!(inv.swap_slots(40, 2), Err(InventoryError::InvalidSlot(40)));
    assert_eq!(inv.revision(), snapshot_rev);
    assert_eq!(inv.slots(), snapshot_slots.as_slice());
    assert_eq!(inv.total_item_count(), initial_total);

    // Both slots out of bounds
    assert_eq!(
        inv.swap_slots(50, 100),
        Err(InventoryError::InvalidSlot(50))
    );
    assert_eq!(inv.revision(), snapshot_rev);
    assert_eq!(inv.slots(), snapshot_slots.as_slice());
    assert_eq!(inv.total_item_count(), initial_total);
}

#[test]
fn test_inventory_grant_and_consume_held() {
    let mut inv = PlayerInventory::new();
    assert_eq!(inv.held_slot(), 0);

    // 1. Invalid input validation and atomicity
    assert_eq!(inv.consume_held(0), Err(InventoryError::InvalidCount(0)));
    assert_eq!(
        inv.consume_held(1),
        Err(InventoryError::InsufficientItems {
            requested: 1,
            available: 0
        })
    );
    assert_eq!(inv.revision(), 0);
    assert_eq!(inv.total_item_count(), 0);

    // 2. Grant items into empty inventory (places into slot 0, which is held_slot)
    let remainder = inv
        .grant_item(stack("minecraft:iron_ingot", 20))
        .expect("grant should succeed");
    assert_eq!(remainder, 0, "all 20 items should fit");
    assert_eq!(inv.revision(), 1);
    assert_eq!(inv.total_item_count(), 20);
    assert_eq!(inv.held_item(), Some(&stack("minecraft:iron_ingot", 20)));

    // 3. Stacking into existing slot
    let remainder = inv
        .grant_item(stack("minecraft:iron_ingot", 30))
        .expect("stacking grant should succeed");
    assert_eq!(remainder, 0);
    assert_eq!(inv.revision(), 2);
    assert_eq!(inv.total_item_count(), 50);
    assert_eq!(inv.get_slot(0), Some(&stack("minecraft:iron_ingot", 50)));

    // 4. Consume held items partially
    let consumed = inv.consume_held(15).expect("partial consumption");
    assert_eq!(consumed, stack("minecraft:iron_ingot", 15));
    assert_eq!(inv.revision(), 3);
    assert_eq!(inv.total_item_count(), 35);
    assert_eq!(inv.get_slot(0), Some(&stack("minecraft:iron_ingot", 35)));

    // 5. Consume remaining held items completely (clearing slot to None)
    let consumed = inv.consume_held(35).expect("complete consumption");
    assert_eq!(consumed, stack("minecraft:iron_ingot", 35));
    assert_eq!(inv.revision(), 4);
    assert_eq!(inv.total_item_count(), 0);
    assert!(inv.get_slot(0).is_none());
    assert!(inv.held_item().is_none());

    // 6. Attempt consume on now-empty slot (rejection)
    assert_eq!(
        inv.consume_held(1),
        Err(InventoryError::InsufficientItems {
            requested: 1,
            available: 0
        })
    );
    assert_eq!(inv.revision(), 4);

    // 7. Over-consume rejection and conservation
    inv.grant_item(stack("minecraft:gold_ingot", 5)).unwrap();
    assert_eq!(inv.revision(), 5);
    assert_eq!(inv.total_item_count(), 5);

    let err = inv.consume_held(10);
    assert_eq!(
        err,
        Err(InventoryError::InsufficientItems {
            requested: 10,
            available: 5
        })
    );
    // Atomicity: held slot must still have exactly 5 gold ingots
    assert_eq!(inv.revision(), 5);
    assert_eq!(inv.held_item(), Some(&stack("minecraft:gold_ingot", 5)));
    assert_eq!(inv.total_item_count(), 5);

    // 8. Switching held slot and consuming from newly active slot
    inv.set_held_slot(2).unwrap();
    assert_eq!(inv.held_slot(), 2);
    assert!(inv.held_item().is_none());
    assert_eq!(
        inv.consume_held(1),
        Err(InventoryError::InsufficientItems {
            requested: 1,
            available: 0
        })
    );

    // Grant bread: slot 0 has gold, so bread goes into slot 1
    inv.grant_item(stack("minecraft:bread", 8)).unwrap();
    assert_eq!(inv.get_slot(1), Some(&stack("minecraft:bread", 8)));

    // Select slot 1 and consume 3 bread
    inv.set_held_slot(1).unwrap();
    let consumed_bread = inv.consume_held(3).unwrap();
    assert_eq!(consumed_bread, stack("minecraft:bread", 3));
    assert_eq!(inv.get_slot(1), Some(&stack("minecraft:bread", 5)));

    // 9. Stacking past max_stack (spill into next slots)
    // Select slot 0 (which has 5 gold ingots)
    inv.set_held_slot(0).unwrap();
    // Grant 60 gold ingots: 59 top up slot 0 to 64, 1 spills into next empty slot (slot 2)
    let rem = inv.grant_item(stack("minecraft:gold_ingot", 60)).unwrap();
    assert_eq!(rem, 0);
    assert_eq!(inv.get_slot(0), Some(&stack("minecraft:gold_ingot", 64)));
    assert_eq!(inv.get_slot(2), Some(&stack("minecraft:gold_ingot", 1)));

    // 10. Full inventory rejection
    let mut full_inv = PlayerInventory::new();
    for slot in 0..TOTAL_SLOT_COUNT {
        full_inv
            .set_slot(slot as u8, Some(stack("minecraft:cobblestone", 64)))
            .unwrap();
    }
    let full_rev = full_inv.revision();
    assert_eq!(full_inv.total_item_count(), 36 * 64);

    assert_eq!(
        full_inv.grant_item(stack("minecraft:diamond", 1)),
        Err(InventoryError::InventoryFull)
    );
    assert_eq!(full_inv.revision(), full_rev);
    assert_eq!(full_inv.total_item_count(), 36 * 64);

    // 11. Partial grant remainder when only partial capacity remains
    let mut partial_inv = PlayerInventory::new();
    for slot in 0..35 {
        partial_inv
            .set_slot(slot as u8, Some(stack("minecraft:cobblestone", 64)))
            .unwrap();
    }
    // Slot 35 has 60 cobblestone (capacity remaining = 4)
    partial_inv
        .set_slot(35, Some(stack("minecraft:cobblestone", 60)))
        .unwrap();

    let partial_rev = partial_inv.revision();
    let rem = partial_inv
        .grant_item(stack("minecraft:cobblestone", 10))
        .unwrap();
    assert_eq!(rem, 6, "4 cobblestone should fit and 6 should remain");
    assert_eq!(partial_inv.revision(), partial_rev + 1);
    assert_eq!(
        partial_inv.get_slot(35),
        Some(&stack("minecraft:cobblestone", 64))
    );
    // Count conservation: (35 * 64 + 60) + (10 - 6) = 36 * 64
    assert_eq!(partial_inv.total_item_count(), 36 * 64);
}

#[test]
fn test_inventory_respects_per_item_max_stack_limit() {
    let mut inv = PlayerInventory::new();
    let pearl = |count| ItemStack::try_new("minecraft:ender_pearl", count, 16).unwrap();

    // ItemStack::try_new rejects count > max_stack (20 > 16), so grant 20 across two stacks.
    assert_eq!(inv.grant_item(pearl(10)).unwrap(), 0);
    assert_eq!(inv.grant_item(pearl(10)).unwrap(), 0);
    assert_eq!(inv.get_slot(0), Some(&pearl(16)));
    assert_eq!(inv.get_slot(1), Some(&pearl(4)));

    // Granting 10 more tops up slot 1 to 14 and leaves slot 2 empty.
    assert_eq!(inv.grant_item(pearl(10)).unwrap(), 0);
    assert_eq!(inv.get_slot(0), Some(&pearl(16)));
    assert_eq!(inv.get_slot(1), Some(&pearl(14)));
    assert!(inv.get_slot(2).is_none());
}
