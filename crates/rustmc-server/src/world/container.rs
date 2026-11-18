//! Authoritative container window and slot synchronization engine.
//!
//! Provides transactional container operations matching Minecraft Java 26.3
//! window click protocols (`ClickContainerPacket`, Window IDs, Slot mappings).
//!
//! # Protocol Specification & Slot Mapping
//!
//! In Minecraft Java 26.3:
//! - **Window ID 0**: The player inventory window (46 slots total):
//!   - Slot 0: Crafting result
//!   - Slots 1..=4: 2x2 Crafting matrix
//!   - Slots 5..=8: Armor slots (Head, Chest, Legs, Feet)
//!   - Slots 9..=35: Main storage (3 rows of 9)
//!   - Slots 36..=44: Hotbar (9 slots)
//!   - Slot 45: Offhand slot
//! - **Generic 9x3 Container (Chest)**: 27 container slots + 36 player storage slots.
//! - **Cursor**: Floating item stack held during window interactions.
//!
//! All operations enforce stack size limits, transactional rollback on error,
//! and state ID versioning to detect out-of-order client packet sequences.

use std::collections::BTreeMap;
use std::fmt;

use crate::item::ItemStack;

/// Armor slot an item can be worn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentSlot {
    Head,
    Chest,
    Legs,
    Feet,
}

/// The kind of container window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerType {
    PlayerInventory,
    Chest9x3,
}

/// Player inventory window slot layout.
mod player {
    use std::ops::Range;
    pub const RESULT: usize = 0;
    pub const CRAFT_GRID: Range<usize> = 1..5;
    pub const ARMOR: Range<usize> = 5..9;
    pub const MAIN: Range<usize> = 9..36;
    pub const HOTBAR: Range<usize> = 36..45;
    pub const OFFHAND: usize = 45;
    pub const STORAGE: Range<usize> = 9..45;
}

/// Slots in a 9x3 chest window before the 36 player inventory slots.
const CHEST_SLOTS: usize = 27;

impl ContainerType {
    pub const fn slot_count(self) -> usize {
        match self {
            Self::PlayerInventory => player::OFFHAND + 1,
            Self::Chest9x3 => CHEST_SLOTS + 36,
        }
    }
}

/// What a slot accepts when the player places items into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotRule {
    /// Output only, such as the crafting result: items can be taken, not placed.
    TakeOnly,
    /// Holds one item wearable in this equipment slot.
    Equipment(EquipmentSlot),
    Any,
}

impl SlotRule {
    fn accepts(self, equipment: Option<EquipmentSlot>) -> bool {
        match self {
            Self::TakeOnly => false,
            Self::Equipment(slot) => equipment == Some(slot),
            Self::Any => true,
        }
    }

    /// Most items of `stack` this slot holds.
    fn limit(self, stack: &ItemStack) -> u16 {
        match self {
            Self::Equipment(_) => 1,
            Self::TakeOnly | Self::Any => stack.max_stack(),
        }
    }
}

/// Interaction click mode sent by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickMode {
    /// Left click: pickup entire stack or place entire cursor stack.
    LeftClick,
    /// Right click: pickup half stack or place one item from cursor.
    RightClick,
    /// Shift-click: transfer items between container zones.
    ShiftClick,
    /// Drop item: Q (drop one) or Ctrl-Q (drop stack).
    DropSingle,
    DropStack,
}

/// Error encountered during container interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerError {
    InvalidSlotIndex,
    SlotOutOfRange,
    StateMismatch,
    EmptyCursor,
}

impl fmt::Display for ContainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlotIndex => write!(f, "slot index is invalid or malformed"),
            Self::SlotOutOfRange => write!(f, "slot index exceeds container boundary"),
            Self::StateMismatch => write!(f, "transaction state id mismatch"),
            Self::EmptyCursor => write!(f, "operation requires non-empty cursor"),
        }
    }
}

impl std::error::Error for ContainerError {}

/// State change delta produced by a container click transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowUpdate {
    pub window_id: u8,
    pub state_id: i32,
    pub modified_slots: Vec<(u16, Option<ItemStack>)>,
    pub cursor: Option<ItemStack>,
}

/// Authoritative container window managing slots, cursor, and transactions.
#[derive(Debug, Clone)]
pub struct ContainerWindow {
    window_id: u8,
    container_type: ContainerType,
    slots: Vec<Option<ItemStack>>,
    cursor: Option<ItemStack>,
    state_id: i32,
    equipment: BTreeMap<String, EquipmentSlot>,
}

impl ContainerWindow {
    pub fn new(window_id: u8, container_type: ContainerType) -> Self {
        let count = container_type.slot_count();
        Self {
            window_id,
            container_type,
            slots: vec![None; count],
            cursor: None,
            state_id: 0,
            equipment: BTreeMap::new(),
        }
    }

    /// Marks every stack of `item` as wearable in `slot`; only such items enter
    /// armor slots. Kept per item id because the shared stack has no components yet.
    pub fn set_equipment_slot(&mut self, item: impl Into<String>, slot: EquipmentSlot) {
        self.equipment.insert(item.into(), slot);
    }

    fn equipment_slot(&self, stack: &ItemStack) -> Option<EquipmentSlot> {
        self.equipment.get(stack.item()).copied()
    }

    #[inline]
    pub fn window_id(&self) -> u8 {
        self.window_id
    }

    #[inline]
    pub fn container_type(&self) -> ContainerType {
        self.container_type
    }

    #[inline]
    pub fn state_id(&self) -> i32 {
        self.state_id
    }

    #[inline]
    pub fn cursor(&self) -> Option<&ItemStack> {
        self.cursor.as_ref()
    }

    pub fn slot(&self, index: usize) -> Option<&ItemStack> {
        self.slots.get(index).and_then(|s| s.as_ref())
    }

    /// Sets a slot directly.
    pub fn set_slot(
        &mut self,
        index: usize,
        stack: Option<ItemStack>,
    ) -> Result<(), ContainerError> {
        if index >= self.slots.len() {
            return Err(ContainerError::SlotOutOfRange);
        }
        self.slots[index] = stack;
        Ok(())
    }

    /// Sets the cursor directly.
    pub fn set_cursor(&mut self, cursor: Option<ItemStack>) {
        self.cursor = cursor;
    }

    fn rule(&self, index: usize) -> SlotRule {
        if self.container_type != ContainerType::PlayerInventory {
            return SlotRule::Any;
        }
        match index {
            player::RESULT => SlotRule::TakeOnly,
            5 => SlotRule::Equipment(EquipmentSlot::Head),
            6 => SlotRule::Equipment(EquipmentSlot::Chest),
            7 => SlotRule::Equipment(EquipmentSlot::Legs),
            8 => SlotRule::Equipment(EquipmentSlot::Feet),
            _ => SlotRule::Any,
        }
    }

    /// Handles an authoritative window click transaction.
    ///
    /// Increments `state_id` on successful application and returns the updated slots.
    pub fn handle_click(
        &mut self,
        slot_index: i16,
        mode: ClickMode,
        client_state_id: i32,
    ) -> Result<WindowUpdate, ContainerError> {
        if client_state_id != self.state_id {
            return Err(ContainerError::StateMismatch);
        }
        let mut modified = Vec::new();
        if slot_index == OUTSIDE_WINDOW {
            match mode {
                ClickMode::LeftClick => self.cursor = None,
                ClickMode::RightClick => drop_one(&mut self.cursor),
                _ => return Err(ContainerError::InvalidSlotIndex),
            }
        } else {
            let index = usize::try_from(slot_index)
                .ok()
                .filter(|index| *index < self.slots.len())
                .ok_or(ContainerError::SlotOutOfRange)?;
            match mode {
                ClickMode::LeftClick => self.click(index, false),
                ClickMode::RightClick => self.click(index, true),
                ClickMode::ShiftClick => self.quick_move(index, &mut modified),
                ClickMode::DropSingle => drop_one(&mut self.slots[index]),
                ClickMode::DropStack => self.slots[index] = None,
            }
            if !matches!(mode, ClickMode::ShiftClick) {
                modified.push((index as u16, self.slots[index].clone()));
            }
        }
        self.state_id = self.state_id.wrapping_add(1);
        Ok(WindowUpdate {
            window_id: self.window_id,
            state_id: self.state_id,
            modified_slots: modified,
            cursor: self.cursor.clone(),
        })
    }

    /// A left (whole stack) or right (half or one item) click on a slot.
    fn click(&mut self, index: usize, single: bool) {
        let rule = self.rule(index);
        match (self.cursor.take(), self.slots[index].take()) {
            (None, None) => {}
            (None, Some(stack)) => {
                // Right-click takes the larger half; output slots give everything.
                let take = if single && rule != SlotRule::TakeOnly {
                    stack.count().div_ceil(2)
                } else {
                    stack.count()
                };
                match stack.split(take) {
                    Ok((taken, rest)) => {
                        self.cursor = Some(taken);
                        self.slots[index] = rest;
                    }
                    Err(stack) => self.slots[index] = Some(stack),
                }
            }
            (Some(cursor), None) => {
                if rule.accepts(self.equipment_slot(&cursor)) {
                    let amount = if single { 1 } else { cursor.count() };
                    let place = amount.min(rule.limit(&cursor));
                    match cursor.split(place) {
                        Ok((placed, rest)) => {
                            self.slots[index] = Some(placed);
                            self.cursor = rest;
                        }
                        Err(cursor) => self.cursor = Some(cursor),
                    }
                } else {
                    self.cursor = Some(cursor);
                }
            }
            (Some(mut cursor), Some(mut stack)) => {
                if rule == SlotRule::TakeOnly {
                    // Collect the output only when all of it fits on the cursor.
                    if cursor.can_stack_with(&stack)
                        && stack.count() <= cursor.max_stack().saturating_sub(cursor.count())
                    {
                        let overflow = cursor.add(stack.count());
                        debug_assert_eq!(overflow, 0);
                    } else {
                        self.slots[index] = Some(stack);
                    }
                    self.cursor = Some(cursor);
                } else if cursor.can_stack_with(&stack) {
                    let room = rule.limit(&stack).saturating_sub(stack.count());
                    let moved = room.min(if single { 1 } else { cursor.count() });
                    if moved > 0 {
                        match move_to_existing(cursor, &mut stack, moved) {
                            Ok(rest) => self.cursor = rest,
                            Err(cursor) => self.cursor = Some(cursor),
                        }
                    } else {
                        self.cursor = Some(cursor);
                    }
                    self.slots[index] = Some(stack);
                } else if rule.accepts(self.equipment_slot(&cursor))
                    && cursor.count() <= rule.limit(&cursor)
                {
                    self.slots[index] = Some(cursor);
                    self.cursor = Some(stack);
                } else {
                    self.slots[index] = Some(stack);
                    self.cursor = Some(cursor);
                }
            }
        }
    }

    /// Shift-click: move a stack to the other zone of the window.
    fn quick_move(&mut self, index: usize, modified: &mut Vec<(u16, Option<ItemStack>)>) {
        let Some(stack) = self.slots[index].take() else {
            return;
        };
        let total = self.slots.len();
        let routes: Vec<(std::ops::Range<usize>, bool)> = match self.container_type {
            ContainerType::Chest9x3 if index < CHEST_SLOTS => vec![(CHEST_SLOTS..total, true)],
            ContainerType::Chest9x3 => vec![(0..CHEST_SLOTS, false)],
            ContainerType::PlayerInventory => {
                let armor = self.equipment_slot(&stack).map(|slot| {
                    let at = player::ARMOR.start + slot as usize;
                    (at..at + 1, false)
                });
                if index == player::RESULT {
                    vec![(player::STORAGE, true)]
                } else if player::CRAFT_GRID.contains(&index)
                    || player::ARMOR.contains(&index)
                    || index == player::OFFHAND
                {
                    vec![(player::STORAGE, false)]
                } else if player::MAIN.contains(&index) {
                    armor.into_iter().chain([(player::HOTBAR, false)]).collect()
                } else {
                    armor.into_iter().chain([(player::MAIN, false)]).collect()
                }
            }
        };
        let mut remaining = Some(stack);
        for (range, reverse) in routes {
            let Some(current) = remaining else {
                break;
            };
            remaining = self.move_into(current, range, reverse, modified);
        }
        self.slots[index] = remaining;
        modified.push((index as u16, self.slots[index].clone()));
    }

    /// Merge into matching stacks, then fill empty slots, in slot order or reversed.
    fn move_into(
        &mut self,
        mut stack: ItemStack,
        range: std::ops::Range<usize>,
        reverse: bool,
        modified: &mut Vec<(u16, Option<ItemStack>)>,
    ) -> Option<ItemStack> {
        let order: Vec<usize> = if reverse {
            range.rev().collect()
        } else {
            range.collect()
        };
        for &target in &order {
            let rule = self.rule(target);
            if let Some(existing) = &mut self.slots[target]
                && existing.can_stack_with(&stack)
            {
                let moved = rule
                    .limit(existing)
                    .saturating_sub(existing.count())
                    .min(stack.count());
                if moved > 0 {
                    match move_to_existing(stack, existing, moved) {
                        Ok(rest) => {
                            modified.push((target as u16, Some(existing.clone())));
                            stack = rest?;
                        }
                        Err(unchanged) => stack = unchanged,
                    }
                }
            }
        }
        for &target in &order {
            let rule = self.rule(target);
            if self.slots[target].is_none() && rule.accepts(self.equipment_slot(&stack)) {
                let limit = rule.limit(&stack);
                match stack.split(limit) {
                    Ok((placed, rest)) => {
                        self.slots[target] = Some(placed);
                        modified.push((target as u16, self.slots[target].clone()));
                        stack = rest?;
                    }
                    Err(unchanged) => stack = unchanged,
                }
            }
        }
        Some(stack)
    }
}

/// Slot index the client sends for a click outside the window.
const OUTSIDE_WINDOW: i16 = -999;

/// Removes one item from `slot`, clearing it if that was the last item.
fn drop_one(slot: &mut Option<ItemStack>) {
    if let Some(stack) = slot.take() {
        *slot = match stack.split(1) {
            Ok((_, rest)) => rest,
            Err(stack) => Some(stack),
        };
    }
}

/// Splits `amount` (caller ensures `> 0`) from `source` and adds it into `dest`.
fn move_to_existing(
    source: ItemStack,
    dest: &mut ItemStack,
    amount: u16,
) -> Result<Option<ItemStack>, ItemStack> {
    let (taken, rest) = source.split(amount)?;
    let overflow = dest.add(taken.count());
    debug_assert_eq!(overflow, 0);
    Ok(rest)
}
