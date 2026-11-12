use super::container::*;

#[test]
fn test_item_stack_creation_and_stacking() {
    let mut stack = ItemStack::new("minecraft:cobblestone", 32, 64).unwrap();
    assert_eq!(stack.item_id(), "minecraft:cobblestone");
    assert_eq!(stack.count(), 32);
    assert_eq!(stack.max_stack(), 64);

    let overflow = stack.add(40);
    assert_eq!(stack.count(), 64);
    assert_eq!(overflow, 8);

    let split = stack.split(10).unwrap();
    assert_eq!(stack.count(), 54);
    assert_eq!(split.count(), 10);
    assert_eq!(split.item_id(), "minecraft:cobblestone");
}

#[test]
fn test_player_inventory_left_click_pickup_and_place() {
    let mut win = ContainerWindow::new(0, ContainerType::PlayerInventory);

    let stone = ItemStack::new("minecraft:stone", 16, 64).unwrap();
    win.set_slot(36, Some(stone.clone())).unwrap();

    // Left click on slot 36 with empty cursor -> picks up entire stack into cursor
    let update = win.handle_click(36, ClickMode::LeftClick, 0).unwrap();
    assert_eq!(update.state_id, 1);
    assert_eq!(win.slot(36), None);
    assert_eq!(win.cursor(), Some(&stone));

    // Left click on empty slot 37 with cursor -> places stack into slot 37
    let update2 = win.handle_click(37, ClickMode::LeftClick, 1).unwrap();
    assert_eq!(update2.state_id, 2);
    assert_eq!(win.slot(37), Some(&stone));
    assert_eq!(win.cursor(), None);
}

#[test]
fn test_right_click_split_and_place_single() {
    let mut win = ContainerWindow::new(0, ContainerType::PlayerInventory);
    let wood = ItemStack::new("minecraft:oak_planks", 10, 64).unwrap();
    win.set_slot(9, Some(wood)).unwrap();

    // Right click on slot 9 -> picks up half (5) into cursor
    let update = win.handle_click(9, ClickMode::RightClick, 0).unwrap();
    assert_eq!(update.state_id, 1);
    assert_eq!(win.slot(9).unwrap().count(), 5);
    assert_eq!(win.cursor().unwrap().count(), 5);

    // Right click on slot 10 (empty) -> places 1 item, leaving 4 on cursor
    let update2 = win.handle_click(10, ClickMode::RightClick, 1).unwrap();
    assert_eq!(update2.state_id, 2);
    assert_eq!(win.slot(10).unwrap().count(), 1);
    assert_eq!(win.cursor().unwrap().count(), 4);
}

#[test]
fn test_shift_click_transfer_between_zones() {
    let mut win = ContainerWindow::new(1, ContainerType::Chest9x3);

    // Chest slot 0 has 20 iron ingots
    let iron = ItemStack::new("minecraft:iron_ingot", 20, 64).unwrap();
    win.set_slot(0, Some(iron)).unwrap();

    // Shift-click on chest slot 0 fills the player inventory from its last
    // slot (the rightmost hotbar slot, 62) backwards.
    let update = win.handle_click(0, ClickMode::ShiftClick, 0).unwrap();
    assert_eq!(update.state_id, 1);
    assert_eq!(win.slot(0), None);
    assert_eq!(win.slot(62).unwrap().item_id(), "minecraft:iron_ingot");
    assert_eq!(win.slot(62).unwrap().count(), 20);

    // Shift-click back on player slot 62 -> moves to the first chest slot
    let update2 = win.handle_click(62, ClickMode::ShiftClick, 1).unwrap();
    assert_eq!(update2.state_id, 2);
    assert_eq!(win.slot(62), None);
    assert_eq!(win.slot(0).unwrap().count(), 20);
}

#[test]
fn test_state_id_mismatch_rejection() {
    let mut win = ContainerWindow::new(0, ContainerType::PlayerInventory);
    // Client sends state_id 5 when server is at state_id 0
    let err = win.handle_click(36, ClickMode::LeftClick, 5).unwrap_err();
    assert_eq!(err, ContainerError::StateMismatch);
}

#[test]
fn test_out_of_bounds_slot_rejection() {
    let mut win = ContainerWindow::new(0, ContainerType::PlayerInventory);
    // Slot index 100 exceeds 46 player inventory slots
    let err = win.handle_click(100, ClickMode::LeftClick, 0).unwrap_err();
    assert_eq!(err, ContainerError::SlotOutOfRange);
}

fn stone(count: u16) -> ItemStack {
    ItemStack::new("minecraft:stone", count, 64).unwrap()
}

#[test]
fn right_click_onto_a_full_matching_stack_changes_nothing() {
    let mut win = ContainerWindow::new(0, ContainerType::PlayerInventory);
    win.set_slot(9, Some(stone(64))).unwrap();
    win.set_cursor(Some(stone(1)));
    win.handle_click(9, ClickMode::RightClick, 0).unwrap();
    assert_eq!(win.slot(9), Some(&stone(64)));
    assert_eq!(win.cursor(), Some(&stone(1)));
}

#[test]
fn crafting_result_and_armor_slots_refuse_ordinary_items() {
    let mut win = ContainerWindow::new(0, ContainerType::PlayerInventory);
    win.set_cursor(Some(stone(64)));
    for (state, slot) in [0, 5, 6, 7, 8].into_iter().enumerate() {
        win.handle_click(slot, ClickMode::LeftClick, state as i32)
            .unwrap();
        assert_eq!(win.slot(slot as usize), None, "slot {slot}");
        assert_eq!(win.cursor(), Some(&stone(64)));
    }
}

#[test]
fn armor_slots_hold_one_matching_item() {
    let mut win = ContainerWindow::new(0, ContainerType::PlayerInventory);
    let helmet = ItemStack::new("minecraft:iron_helmet", 2, 64)
        .unwrap()
        .with_equipment(EquipmentSlot::Head);
    win.set_cursor(Some(helmet.clone()));
    // A helmet does not fit the chest slot, and only one enters the head slot.
    win.handle_click(6, ClickMode::LeftClick, 0).unwrap();
    assert_eq!(win.slot(6), None);
    win.handle_click(5, ClickMode::LeftClick, 1).unwrap();
    assert_eq!(win.slot(5).map(ItemStack::count), Some(1));
    assert_eq!(win.cursor().map(ItemStack::count), Some(1));
}

#[test]
fn crafting_result_is_collected_onto_a_matching_cursor() {
    let mut win = ContainerWindow::new(0, ContainerType::PlayerInventory);
    win.set_slot(0, Some(stone(4))).unwrap();
    win.set_cursor(Some(stone(10)));
    win.handle_click(0, ClickMode::LeftClick, 0).unwrap();
    assert_eq!(win.slot(0), None);
    assert_eq!(win.cursor(), Some(&stone(14)));
}

#[test]
fn offhand_shift_click_reaches_the_hotbar_when_storage_is_full() {
    let mut win = ContainerWindow::new(0, ContainerType::PlayerInventory);
    for slot in 9..36 {
        win.set_slot(
            slot,
            Some(ItemStack::new("minecraft:dirt", 64, 64).unwrap()),
        )
        .unwrap();
    }
    win.set_slot(45, Some(stone(8))).unwrap();
    win.handle_click(45, ClickMode::ShiftClick, 0).unwrap();
    assert_eq!(win.slot(45), None);
    assert_eq!(win.slot(36), Some(&stone(8)));
}

#[test]
fn shift_clicking_armor_from_storage_equips_it() {
    let mut win = ContainerWindow::new(0, ContainerType::PlayerInventory);
    let boots = ItemStack::new("minecraft:iron_boots", 1, 1)
        .unwrap()
        .with_equipment(EquipmentSlot::Feet);
    win.set_slot(20, Some(boots.clone())).unwrap();
    win.handle_click(20, ClickMode::ShiftClick, 0).unwrap();
    assert_eq!(win.slot(8), Some(&boots));
    assert_eq!(win.slot(20), None);
}

#[test]
fn exhausted_stacks_never_occupy_the_cursor_or_a_slot() {
    let mut win = ContainerWindow::new(0, ContainerType::PlayerInventory);
    let mut source = stone(3);
    let moved = source.split(3).unwrap();
    assert_eq!(source.count(), 0);
    win.set_cursor(Some(source.clone()));
    assert_eq!(win.cursor(), None);
    win.set_slot(9, Some(source)).unwrap();
    assert_eq!(win.slot(9), None);
    // Placing the real stack still works afterwards.
    win.set_cursor(Some(moved));
    win.handle_click(9, ClickMode::LeftClick, 0).unwrap();
    assert_eq!(win.slot(9), Some(&stone(3)));
}
