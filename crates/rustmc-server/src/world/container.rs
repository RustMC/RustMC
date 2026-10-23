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

use std::fmt;

/// Maximum item stack count in standard Minecraft inventory.
pub const DEFAULT_MAX_STACK: u16 = 64;

/// A stack of items occupying a container slot or floating on the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStack {
    item_id: String,
    count: u16,
    max_stack: u16,
}

impl ItemStack {
    pub fn new(item_id: impl Into<String>, count: u16, max_stack: u16) -> Option<Self> {
        let max_stack = if max_stack == 0 {
            DEFAULT_MAX_STACK
        } else {
            max_stack
        };
        if count == 0 {
            return None;
        }
        Some(Self {
            item_id: item_id.into(),
            count: count.min(max_stack),
            max_stack,
        })
    }

    #[inline]
    pub fn item_id(&self) -> &str {
        &self.item_id
    }

    #[inline]
    pub fn count(&self) -> u16 {
        self.count
    }

    #[inline]
    pub fn max_stack(&self) -> u16 {
        self.max_stack
    }

    #[inline]
    pub fn can_stack_with(&self, other: &Self) -> bool {
        self.item_id == other.item_id && self.max_stack == other.max_stack
    }

    /// Attempts to add items into this stack up to `max_stack`. Returns overflow count.
    pub fn add(&mut self, amount: u16) -> u16 {
        let space = self.max_stack.saturating_sub(self.count);
        let accepted = space.min(amount);
        self.count += accepted;
        amount - accepted
    }

    /// Splits off up to `amount` items into a new stack. Returns None if amount is 0.
    pub fn split(&mut self, amount: u16) -> Option<Self> {
        if amount == 0 || self.count == 0 {
            return None;
        }
        let take = amount.min(self.count);
        self.count -= take;
        Some(Self {
            item_id: self.item_id.clone(),
            count: take,
            max_stack: self.max_stack,
        })
    }
}

/// The kind of container window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerType {
    PlayerInventory,
    Chest9x3,
}

impl ContainerType {
    pub const fn slot_count(self) -> usize {
        match self {
            Self::PlayerInventory => 46,
            Self::Chest9x3 => 63, // 27 chest slots + 36 player inventory slots
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
        }
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

    pub fn set_cursor(&mut self, cursor: Option<ItemStack>) {
        self.cursor = cursor;
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

        // Slot -999 indicates clicking outside the window (drops cursor items).
        if slot_index == -999 {
            match mode {
                ClickMode::LeftClick => {
                    let _dropped = self.cursor.take();
                    self.state_id = self.state_id.wrapping_add(1);
                    return Ok(WindowUpdate {
                        window_id: self.window_id,
                        state_id: self.state_id,
                        modified_slots: modified,
                        cursor: None,
                    });
                }
                ClickMode::RightClick => {
                    if let Some(ref mut cur) = self.cursor {
                        let _dropped = cur.split(1);
                        if cur.count() == 0 {
                            self.cursor = None;
                        }
                    }
                    self.state_id = self.state_id.wrapping_add(1);
                    return Ok(WindowUpdate {
                        window_id: self.window_id,
                        state_id: self.state_id,
                        modified_slots: modified,
                        cursor: self.cursor.clone(),
                    });
                }
                _ => return Err(ContainerError::InvalidSlotIndex),
            }
        }

        if slot_index < 0 || (slot_index as usize) >= self.slots.len() {
            return Err(ContainerError::SlotOutOfRange);
        }

        let idx = slot_index as usize;

        match mode {
            ClickMode::LeftClick => {
                match (self.cursor.take(), self.slots[idx].take()) {
                    (None, None) => {}
                    (Some(cur), None) => {
                        // Place cursor stack into empty slot
                        self.slots[idx] = Some(cur);
                        modified.push((idx as u16, self.slots[idx].clone()));
                    }
                    (None, Some(slot_stack)) => {
                        // Pick up entire slot stack into cursor
                        self.cursor = Some(slot_stack);
                        self.slots[idx] = None;
                        modified.push((idx as u16, None));
                    }
                    (Some(mut cur), Some(mut slot_stack)) => {
                        if cur.can_stack_with(&slot_stack) {
                            // Merge into slot stack up to max_stack
                            let overflow = slot_stack.add(cur.count());
                            if overflow > 0 {
                                cur.count = overflow;
                                self.cursor = Some(cur);
                            } else {
                                self.cursor = None;
                            }
                            self.slots[idx] = Some(slot_stack);
                        } else {
                            // Swap cursor stack and slot stack
                            self.cursor = Some(slot_stack);
                            self.slots[idx] = Some(cur);
                        }
                        modified.push((idx as u16, self.slots[idx].clone()));
                    }
                }
            }
            ClickMode::RightClick => {
                match (self.cursor.take(), self.slots[idx].take()) {
                    (None, None) => {}
                    (Some(mut cur), None) => {
                        // Place 1 item from cursor into empty slot
                        let placed = cur.split(1).unwrap();
                        self.slots[idx] = Some(placed);
                        if cur.count() > 0 {
                            self.cursor = Some(cur);
                        }
                        modified.push((idx as u16, self.slots[idx].clone()));
                    }
                    (None, Some(mut slot_stack)) => {
                        // Pick up half of slot stack into cursor
                        let half = slot_stack.count().div_ceil(2);
                        let picked = slot_stack.split(half);
                        self.cursor = picked;
                        if slot_stack.count() > 0 {
                            self.slots[idx] = Some(slot_stack);
                        } else {
                            self.slots[idx] = None;
                        }
                        modified.push((idx as u16, self.slots[idx].clone()));
                    }
                    (Some(mut cur), Some(mut slot_stack)) => {
                        if cur.can_stack_with(&slot_stack)
                            && slot_stack.count() < slot_stack.max_stack()
                        {
                            // Place 1 item from cursor into slot
                            cur.count -= 1;
                            slot_stack.count += 1;
                            self.slots[idx] = Some(slot_stack);
                            if cur.count() > 0 {
                                self.cursor = Some(cur);
                            }
                        } else {
                            // Swap cursor stack and slot stack
                            self.cursor = Some(slot_stack);
                            self.slots[idx] = Some(cur);
                        }
                        modified.push((idx as u16, self.slots[idx].clone()));
                    }
                }
            }
            ClickMode::ShiftClick => {
                // Quick-move between zones (container top slots <-> player inventory bottom slots)
                if let Some(mut stack) = self.slots[idx].take() {
                    let total = self.slots.len();
                    let (target_start, target_end) =
                        if self.container_type == ContainerType::PlayerInventory {
                            // In player inventory: hotbar (36..45) <-> main inventory (9..36)
                            if idx >= 36 { (9, 36) } else { (36, 45) }
                        } else {
                            // In chest: chest (0..27) <-> player inventory (27..63)
                            if idx < 27 { (27, total) } else { (0, 27) }
                        };

                    // First pass: try merging into existing matching stacks
                    for target_idx in target_start..target_end {
                        if stack.count() == 0 {
                            break;
                        }
                        if let Some(target_stack) = &mut self.slots[target_idx]
                            && target_stack.can_stack_with(&stack)
                        {
                            let rem = target_stack.add(stack.count());
                            stack.count = rem;
                            modified.push((target_idx as u16, Some(target_stack.clone())));
                        }
                    }

                    // Second pass: place remainder into first empty slot
                    let mut remainder = if stack.count() > 0 {
                        let mut rem = Some(stack);
                        for target_idx in target_start..target_end {
                            if self.slots[target_idx].is_none() {
                                let placed = rem.take().unwrap();
                                modified.push((target_idx as u16, Some(placed.clone())));
                                self.slots[target_idx] = Some(placed);
                                break;
                            }
                        }
                        rem
                    } else {
                        None
                    };

                    self.slots[idx] = remainder.take();
                    modified.push((idx as u16, self.slots[idx].clone()));
                }
            }
            ClickMode::DropSingle => {
                if let Some(mut stack) = self.slots[idx].take() {
                    let _dropped = stack.split(1);
                    if stack.count() > 0 {
                        self.slots[idx] = Some(stack);
                    }
                    modified.push((idx as u16, self.slots[idx].clone()));
                }
            }
            ClickMode::DropStack => {
                if self.slots[idx].is_some() {
                    let _dropped = self.slots[idx].take();
                    modified.push((idx as u16, None));
                }
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
}
