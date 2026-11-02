//! Player inventory container and deterministic slot transactions.
//!
//! Manages 36 slots:
//! - 9 hotbar slots (`0..=8`)
//! - 27 main inventory slots (`9..=35`)
//! - Active hotbar index (`0..=8`)
//! - Monotonic transaction revision counter (`revision: u64`)
//!
//! All slot transactions guarantee strict conservation of item counts. Any
//! transaction that fails or is rejected is strictly atomic and leaves the
//! inventory unmodified.

use std::fmt;

/// Maximum number of items in a single stack.
pub const MAX_STACK_SIZE: u8 = 64;

/// Number of hotbar slots (slots 0..=8).
pub const HOTBAR_SLOT_COUNT: usize = 9;

/// Number of main inventory slots (slots 9..=35).
pub const MAIN_SLOT_COUNT: usize = 27;

/// Total number of inventory slots (slots 0..=35).
pub const TOTAL_SLOT_COUNT: usize = 36;

/// Closed range of valid hotbar slot indices.
pub const HOTBAR_SLOTS: std::ops::RangeInclusive<u8> = 0..=8;

/// Closed range of valid main inventory slot indices.
pub const MAIN_SLOTS: std::ops::RangeInclusive<u8> = 9..=35;

/// Closed range of all valid inventory slot indices.
pub const TOTAL_SLOTS: std::ops::RangeInclusive<u8> = 0..=35;

/// A counted stack of items with max stack size bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStack {
    item: String,
    count: u8,
}

impl ItemStack {
    /// Creates a new item stack, validating that `count` is within `1..=MAX_STACK_SIZE`
    /// and that `item` is non-empty.
    pub fn new(item: impl Into<String>, count: u8) -> Result<Self, InventoryError> {
        let item = item.into();
        if item.trim().is_empty() {
            return Err(InventoryError::InvalidItem);
        }
        if count == 0 || count > MAX_STACK_SIZE {
            return Err(InventoryError::InvalidCount(count));
        }
        Ok(Self { item, count })
    }

    /// The item identifier.
    pub fn item(&self) -> &str {
        &self.item
    }

    /// The item stack count, guaranteed to be within `1..=MAX_STACK_SIZE`.
    pub fn count(&self) -> u8 {
        self.count
    }
}

/// Errors returned by inventory transactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryError {
    /// Slot index is outside the valid inventory range `0..=35`.
    InvalidSlot(u8),
    /// Slot index is outside the valid hotbar range `0..=8`.
    InvalidHotbarSlot(u8),
    /// Item count is invalid (e.g. zero or exceeding bounds).
    InvalidCount(u8),
    /// Item identifier is empty or invalid.
    InvalidItem,
    /// Held slot does not contain enough items to satisfy the requested consumption.
    InsufficientItems { requested: u8, available: u8 },
    /// Inventory has no available space to accept items.
    InventoryFull,
}

impl fmt::Display for InventoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlot(slot) => {
                write!(f, "inventory slot {slot} is out of bounds (0..=35)")
            }
            Self::InvalidHotbarSlot(slot) => {
                write!(f, "hotbar slot {slot} is out of bounds (0..=8)")
            }
            Self::InvalidCount(count) => write!(f, "invalid item count {count}"),
            Self::InvalidItem => write!(f, "item identifier cannot be empty"),
            Self::InsufficientItems {
                requested,
                available,
            } => write!(
                f,
                "insufficient items: requested {requested}, but only {available} available"
            ),
            Self::InventoryFull => write!(f, "inventory has no available space"),
        }
    }
}

impl std::error::Error for InventoryError {}

/// Player inventory managing 36 slots (9 hotbar, 27 main inventory) and active hotbar selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInventory {
    slots: [Option<ItemStack>; TOTAL_SLOT_COUNT],
    held_slot: u8,
    revision: u64,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerInventory {
    /// Creates an empty player inventory with held slot 0 and revision 0.
    pub const fn new() -> Self {
        Self {
            slots: [const { None }; TOTAL_SLOT_COUNT],
            held_slot: 0,
            revision: 0,
        }
    }

    /// Returns the currently active hotbar slot index (`0..=8`).
    pub const fn held_slot(&self) -> u8 {
        self.held_slot
    }

    /// Returns the monotonic transaction revision counter.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns a slice over all 36 inventory slots.
    pub fn slots(&self) -> &[Option<ItemStack>] {
        &self.slots
    }

    /// Returns a reference to the item stack in the currently active held hotbar slot, if any.
    pub fn held_item(&self) -> Option<&ItemStack> {
        self.get_slot(self.held_slot)
    }

    /// Returns the total count of all items across all 36 inventory slots.
    pub fn total_item_count(&self) -> u64 {
        self.slots
            .iter()
            .filter_map(|s| s.as_ref())
            .map(|s| s.count as u64)
            .sum()
    }

    /// Sets the active hotbar slot.
    ///
    /// The slot index must be within the hotbar range `0..=8`.
    /// On success, the monotonic revision counter is incremented.
    /// On rejection, the held slot and revision are unchanged.
    pub fn set_held_slot(&mut self, slot: u8) -> Result<(), InventoryError> {
        if !HOTBAR_SLOTS.contains(&slot) {
            return Err(InventoryError::InvalidHotbarSlot(slot));
        }
        self.held_slot = slot;
        self.revision = self
            .revision
            .checked_add(1)
            .expect("inventory revision overflow");
        Ok(())
    }

    /// Gets a reference to the item stack in the given slot, or `None` if empty
    /// or if `slot` is out of bounds (`slot >= 36`).
    pub fn get_slot(&self, slot: u8) -> Option<&ItemStack> {
        if (slot as usize) < TOTAL_SLOT_COUNT {
            self.slots[slot as usize].as_ref()
        } else {
            None
        }
    }

    /// Sets the item stack in a specific slot (`0..=35`), validating count bounds.
    ///
    /// On success, the revision counter increments.
    /// On rejection (slot out of bounds or invalid stack), state and revision are untouched.
    pub fn set_slot(&mut self, slot: u8, stack: Option<ItemStack>) -> Result<(), InventoryError> {
        if (slot as usize) >= TOTAL_SLOT_COUNT {
            return Err(InventoryError::InvalidSlot(slot));
        }
        if let Some(ref s) = stack {
            if s.item.trim().is_empty() {
                return Err(InventoryError::InvalidItem);
            }
            if s.count == 0 || s.count > MAX_STACK_SIZE {
                return Err(InventoryError::InvalidCount(s.count));
            }
        }
        self.slots[slot as usize] = stack;
        self.revision = self
            .revision
            .checked_add(1)
            .expect("inventory revision overflow");
        Ok(())
    }

    /// Deterministically swaps the contents of two slots (`0..=35`).
    ///
    /// Strictly atomic: if either slot index is out of bounds, no modification is
    /// performed, no items are lost or duplicated, and an error is returned.
    /// On success, revision is incremented.
    pub fn swap_slots(&mut self, slot_a: u8, slot_b: u8) -> Result<(), InventoryError> {
        if (slot_a as usize) >= TOTAL_SLOT_COUNT {
            return Err(InventoryError::InvalidSlot(slot_a));
        }
        if (slot_b as usize) >= TOTAL_SLOT_COUNT {
            return Err(InventoryError::InvalidSlot(slot_b));
        }

        self.slots.swap(slot_a as usize, slot_b as usize);
        self.revision = self
            .revision
            .checked_add(1)
            .expect("inventory revision overflow");
        Ok(())
    }

    /// Grants `count` of `item` to the inventory, stacking with existing compatible stacks
    /// first, then filling empty slots in slot order (`0..=35`).
    ///
    /// Returns `Ok(remaining)` where `remaining` is the count of items that could
    /// not fit into the inventory (`0` if all items were granted).
    ///
    /// Conservation of item counts: any placed items are accounted for, and any
    /// items that cannot fit are returned in `remaining`. If no items can fit because
    /// the inventory has no capacity, `Err(InventoryError::InventoryFull)` is returned
    /// and the inventory is completely unmodified.
    ///
    /// If rejected (empty item name, count == 0, or inventory full), the operation
    /// is strictly atomic: no state is modified and revision is not incremented.
    pub fn grant_item(&mut self, item: String, count: u8) -> Result<u8, InventoryError> {
        if item.trim().is_empty() {
            return Err(InventoryError::InvalidItem);
        }
        if count == 0 {
            return Err(InventoryError::InvalidCount(0));
        }

        // Calculate available capacity for this item across all slots.
        let mut available_capacity: usize = 0;
        for slot in &self.slots {
            match slot {
                Some(stack) if stack.item == item => {
                    available_capacity += (MAX_STACK_SIZE.saturating_sub(stack.count)) as usize;
                }
                None => {
                    available_capacity += MAX_STACK_SIZE as usize;
                }
                _ => {}
            }
        }

        if available_capacity == 0 {
            return Err(InventoryError::InventoryFull);
        }

        let to_place = (count as usize).min(available_capacity) as u8;
        let remainder = count - to_place;

        let mut remaining_to_place = to_place;

        // Pass 1: Top up existing compatible stacks in deterministic slot order (0..=35)
        for slot in 0..TOTAL_SLOT_COUNT {
            if remaining_to_place == 0 {
                break;
            }
            if let Some(stack) = &mut self.slots[slot]
                && stack.item == item
                && stack.count < MAX_STACK_SIZE
            {
                let space = MAX_STACK_SIZE - stack.count;
                let add = remaining_to_place.min(space);
                stack.count += add;
                remaining_to_place -= add;
            }
        }

        // Pass 2: Place remainder into empty slots in deterministic slot order (0..=35)
        for slot in 0..TOTAL_SLOT_COUNT {
            if remaining_to_place == 0 {
                break;
            }
            if self.slots[slot].is_none() {
                let add = remaining_to_place.min(MAX_STACK_SIZE);
                self.slots[slot] = Some(ItemStack {
                    item: item.clone(),
                    count: add,
                });
                remaining_to_place -= add;
            }
        }

        self.revision = self
            .revision
            .checked_add(1)
            .expect("inventory revision overflow");
        Ok(remainder)
    }

    /// Consumes `count` items from the currently active held hotbar slot (`held_slot`).
    ///
    /// Strictly atomic: if the held slot is empty or contains fewer items than `count`,
    /// or if `count == 0`, the transaction fails, no items are consumed, and revision
    /// is not incremented.
    ///
    /// On success, returns the consumed `ItemStack`, updates the held slot (clearing
    /// to `None` if all items were consumed), and increments the revision counter.
    pub fn consume_held(&mut self, count: u8) -> Result<ItemStack, InventoryError> {
        if count == 0 {
            return Err(InventoryError::InvalidCount(0));
        }

        let held_idx = self.held_slot as usize;
        let Some(stack) = &mut self.slots[held_idx] else {
            return Err(InventoryError::InsufficientItems {
                requested: count,
                available: 0,
            });
        };

        if stack.count < count {
            return Err(InventoryError::InsufficientItems {
                requested: count,
                available: stack.count,
            });
        }

        let item_name = stack.item.clone();
        if stack.count == count {
            self.slots[held_idx] = None;
        } else {
            stack.count -= count;
        }

        self.revision = self
            .revision
            .checked_add(1)
            .expect("inventory revision overflow");

        Ok(ItemStack {
            item: item_name,
            count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_stack_validation() {
        assert_eq!(ItemStack::new("", 1), Err(InventoryError::InvalidItem));
        assert_eq!(ItemStack::new("   ", 1), Err(InventoryError::InvalidItem));
        assert_eq!(
            ItemStack::new("minecraft:dirt", 0),
            Err(InventoryError::InvalidCount(0))
        );
        assert_eq!(
            ItemStack::new("minecraft:dirt", 65),
            Err(InventoryError::InvalidCount(65))
        );
        let valid = ItemStack::new("minecraft:dirt", 64).unwrap();
        assert_eq!(valid.item(), "minecraft:dirt");
        assert_eq!(valid.count(), 64);
    }

    #[test]
    fn error_display_coverage() {
        let errs = [
            InventoryError::InvalidSlot(40),
            InventoryError::InvalidHotbarSlot(12),
            InventoryError::InvalidCount(0),
            InventoryError::InvalidItem,
            InventoryError::InsufficientItems {
                requested: 10,
                available: 2,
            },
            InventoryError::InventoryFull,
        ];
        for err in &errs {
            let msg = err.to_string();
            assert!(!msg.is_empty());
        }
    }

    #[test]
    fn inventory_transactional_invariants_cannot_be_bypassed() {
        let mut inv = PlayerInventory::default();
        assert_eq!(inv, PlayerInventory::new());
        let initial_rev = inv.revision();

        // 1. Invalid slot rejected without modifying revision
        assert_eq!(
            inv.set_slot(36, Some(ItemStack::new("minecraft:apple", 2).unwrap())),
            Err(InventoryError::InvalidSlot(36))
        );
        assert_eq!(inv.revision(), initial_rev);

        // 2. Valid set_slot increments revision
        inv.set_slot(5, Some(ItemStack::new("minecraft:apple", 2).unwrap()))
            .unwrap();
        assert_eq!(inv.revision(), initial_rev + 1);
        assert_eq!(inv.get_slot(5).unwrap().count(), 2);
        assert_eq!(inv.get_slot(5).unwrap().item(), "minecraft:apple");

        // 3. Transactions are strictly checked; no direct mutable slot reference
        assert!(inv.get_slot(36).is_none());
    }
}
