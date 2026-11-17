//! Player inventory container and deterministic slot transactions.
//!
//! Manages 36 slots:
//! - 9 hotbar slots (`0..=8`)
//! - 27 main inventory slots (`9..=35`)
//! - Active hotbar index (`0..=8`)
//! - Monotonic transaction revision counter (`revision: u64`)
//!
//! Each occupied slot holds a validated `crate::item::ItemStack` whose own
//! `max_stack()` defines its stack limit. All slot transactions guarantee strict
//! conservation of item counts. Any transaction that fails or is rejected is
//! strictly atomic and leaves the inventory unmodified.

use std::fmt;

use crate::item::ItemStack;

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

/// Errors returned by inventory transactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryError {
    /// Slot index is outside the valid inventory range `0..=35`.
    InvalidSlot(u8),
    /// Slot index is outside the valid hotbar range `0..=8`.
    InvalidHotbarSlot(u8),
    /// Item count is invalid (e.g. zero or exceeding bounds).
    InvalidCount(u16),
    /// Held slot does not contain enough items to satisfy the requested consumption.
    InsufficientItems { requested: u16, available: u16 },
    /// Inventory has no available space to accept items.
    InventoryFull,
    /// Transaction revision counter reached `u64::MAX` and cannot advance.
    RevisionExhausted,
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
            Self::InsufficientItems {
                requested,
                available,
            } => write!(
                f,
                "insufficient items: requested {requested}, but only {available} available"
            ),
            Self::InventoryFull => write!(f, "inventory has no available space"),
            Self::RevisionExhausted => write!(f, "inventory revision counter exhausted"),
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

    /// Computes the next revision without mutating state so failed checks remain atomic.
    const fn next_revision(&self) -> Result<u64, InventoryError> {
        match self.revision.checked_add(1) {
            Some(next) => Ok(next),
            None => Err(InventoryError::RevisionExhausted),
        }
    }

    /// Returns the total count of all items across all 36 inventory slots.
    pub fn total_item_count(&self) -> u64 {
        self.slots
            .iter()
            .filter_map(|s| s.as_ref())
            .map(|s| u64::from(s.count()))
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
        let next_revision = self.next_revision()?;
        self.held_slot = slot;
        self.revision = next_revision;
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

    /// Sets the item stack in a specific slot (`0..=35`).
    ///
    /// On success, the revision counter increments.
    /// On rejection (slot out of bounds or revision exhausted), state and revision are untouched.
    pub fn set_slot(&mut self, slot: u8, stack: Option<ItemStack>) -> Result<(), InventoryError> {
        if (slot as usize) >= TOTAL_SLOT_COUNT {
            return Err(InventoryError::InvalidSlot(slot));
        }
        let next_revision = self.next_revision()?;
        self.slots[slot as usize] = stack;
        self.revision = next_revision;
        Ok(())
    }

    /// Deterministically swaps the contents of two slots (`0..=35`).
    ///
    /// Strictly atomic: if either slot index is out of bounds or revision is exhausted,
    /// no modification is performed, no items are lost or duplicated, and an error is returned.
    /// On success, revision is incremented.
    pub fn swap_slots(&mut self, slot_a: u8, slot_b: u8) -> Result<(), InventoryError> {
        if (slot_a as usize) >= TOTAL_SLOT_COUNT {
            return Err(InventoryError::InvalidSlot(slot_a));
        }
        if (slot_b as usize) >= TOTAL_SLOT_COUNT {
            return Err(InventoryError::InvalidSlot(slot_b));
        }
        let next_revision = self.next_revision()?;

        self.slots.swap(slot_a as usize, slot_b as usize);
        self.revision = next_revision;
        Ok(())
    }

    /// Grants `stack` to the inventory, stacking with existing compatible stacks
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
    /// If rejected (inventory full or revision exhausted), the operation is
    /// strictly atomic: no state is modified and revision is not incremented.
    pub fn grant_item(&mut self, stack: ItemStack) -> Result<u16, InventoryError> {
        // Calculate available capacity for this stack across all slots.
        let mut available_capacity: usize = 0;
        for slot in &self.slots {
            match slot {
                Some(existing) if existing.can_stack_with(&stack) => {
                    available_capacity +=
                        usize::from(existing.max_stack().saturating_sub(existing.count()));
                }
                None => {
                    available_capacity += usize::from(stack.max_stack());
                }
                _ => {}
            }
        }

        if available_capacity == 0 {
            return Err(InventoryError::InventoryFull);
        }

        let next_revision = self.next_revision()?;
        let mut pending = Some(stack);

        // Pass 1: Top up existing compatible stacks in deterministic slot order (0..=35).
        for slot in &mut self.slots {
            let Some(incoming) = pending.take() else {
                break;
            };
            if let Some(existing) = slot
                && existing.can_stack_with(&incoming)
            {
                let space = existing.max_stack().saturating_sub(existing.count());
                if space > 0 {
                    match incoming.split(space) {
                        Ok((taken, rest)) => {
                            let overflow = existing.add(taken.count());
                            debug_assert_eq!(overflow, 0);
                            pending = rest;
                        }
                        Err(returned) => {
                            pending = Some(returned);
                        }
                    }
                    continue;
                }
            }
            pending = Some(incoming);
        }

        // Pass 2: Place remainder into empty slots in deterministic slot order (0..=35).
        for slot in &mut self.slots {
            let Some(incoming) = pending.take() else {
                break;
            };
            if slot.is_none() {
                let limit = incoming.max_stack();
                match incoming.split(limit) {
                    Ok((placed, rest)) => {
                        *slot = Some(placed);
                        pending = rest;
                    }
                    Err(returned) => {
                        pending = Some(returned);
                    }
                }
            } else {
                pending = Some(incoming);
            }
        }

        let remainder = pending.map_or(0, |rem| rem.count());
        self.revision = next_revision;
        Ok(remainder)
    }

    /// Consumes `count` items from the currently active held hotbar slot (`held_slot`).
    ///
    /// Strictly atomic: if the held slot is empty or contains fewer items than `count`,
    /// `count == 0`, or revision is exhausted, the transaction fails, no items are
    /// consumed, and revision is not incremented.
    ///
    /// On success, returns the consumed `ItemStack`, updates the held slot (clearing
    /// to `None` if all items were consumed), and increments the revision counter.
    pub fn consume_held(&mut self, count: u16) -> Result<ItemStack, InventoryError> {
        if count == 0 {
            return Err(InventoryError::InvalidCount(0));
        }

        let held_idx = usize::from(self.held_slot);
        let available = self.slots[held_idx].as_ref().map_or(0, ItemStack::count);
        if available < count {
            return Err(InventoryError::InsufficientItems {
                requested: count,
                available,
            });
        }

        let next_revision = self.next_revision()?;
        let Some(stack) = self.slots[held_idx].take() else {
            return Err(InventoryError::InsufficientItems {
                requested: count,
                available: 0,
            });
        };

        match stack.split(count) {
            Ok((taken, remainder)) => {
                self.slots[held_idx] = remainder;
                self.revision = next_revision;
                Ok(taken)
            }
            Err(original) => {
                self.slots[held_idx] = Some(original);
                Err(InventoryError::InvalidCount(count))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::DEFAULT_MAX_STACK;

    #[test]
    fn error_display_coverage() {
        let errs = [
            InventoryError::InvalidSlot(40),
            InventoryError::InvalidHotbarSlot(12),
            InventoryError::InvalidCount(0),
            InventoryError::InsufficientItems {
                requested: 10,
                available: 2,
            },
            InventoryError::InventoryFull,
            InventoryError::RevisionExhausted,
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
            inv.set_slot(
                36,
                Some(ItemStack::try_new("minecraft:apple", 2, DEFAULT_MAX_STACK).unwrap())
            ),
            Err(InventoryError::InvalidSlot(36))
        );
        assert_eq!(inv.revision(), initial_rev);

        // 2. Valid set_slot increments revision
        inv.set_slot(
            5,
            Some(ItemStack::try_new("minecraft:apple", 2, DEFAULT_MAX_STACK).unwrap()),
        )
        .unwrap();
        assert_eq!(inv.revision(), initial_rev + 1);
        assert_eq!(inv.get_slot(5).unwrap().count(), 2);
        assert_eq!(inv.get_slot(5).unwrap().item(), "minecraft:apple");

        // 3. Transactions are strictly checked; no direct mutable slot reference
        assert!(inv.get_slot(36).is_none());
    }

    #[test]
    fn revision_exhaustion_is_atomic_for_all_transactions() {
        let mut inv = PlayerInventory::new();
        let apples = ItemStack::try_new("minecraft:apple", 5, DEFAULT_MAX_STACK).unwrap();
        let sword = ItemStack::try_new("minecraft:diamond_sword", 1, 1).unwrap();
        inv.set_slot(0, Some(apples.clone())).unwrap();
        inv.set_slot(1, Some(sword)).unwrap();
        inv.revision = u64::MAX;

        let snapshot_slots = inv.slots().to_vec();
        let snapshot_held = inv.held_slot();

        let assert_unchanged = |inv: &PlayerInventory| {
            assert_eq!(inv.slots(), snapshot_slots.as_slice());
            assert_eq!(inv.held_slot(), snapshot_held);
            assert_eq!(inv.revision(), u64::MAX);
        };

        assert_eq!(
            inv.set_slot(2, Some(apples.clone())),
            Err(InventoryError::RevisionExhausted)
        );
        assert_unchanged(&inv);

        assert_eq!(inv.swap_slots(0, 1), Err(InventoryError::RevisionExhausted));
        assert_unchanged(&inv);

        assert_eq!(inv.set_held_slot(1), Err(InventoryError::RevisionExhausted));
        assert_unchanged(&inv);

        assert_eq!(
            inv.grant_item(apples),
            Err(InventoryError::RevisionExhausted)
        );
        assert_unchanged(&inv);

        assert_eq!(inv.consume_held(1), Err(InventoryError::RevisionExhausted));
        assert_unchanged(&inv);
    }
}
