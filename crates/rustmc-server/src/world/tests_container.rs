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

    // Shift-click on chest slot 0 -> moves into player inventory zone (starts at slot 27)
    let update = win.handle_click(0, ClickMode::ShiftClick, 0).unwrap();
    assert_eq!(update.state_id, 1);
    assert_eq!(win.slot(0), None);
    assert_eq!(win.slot(27).unwrap().item_id(), "minecraft:iron_ingot");
    assert_eq!(win.slot(27).unwrap().count(), 20);

    // Shift-click back on player slot 27 -> moves back to chest slot 0
    let update2 = win.handle_click(27, ClickMode::ShiftClick, 1).unwrap();
    assert_eq!(update2.state_id, 2);
    assert_eq!(win.slot(27), None);
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
