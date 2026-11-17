//! A counted stack of one item kind, with invariants checked at construction.
//! Per-item components (durability, names, enchantments) are deliberately not
//! modelled yet.

/// Default stack limit for standard items.
pub const DEFAULT_MAX_STACK: u16 = 64;

/// Upper bound of the item max_stack_size component since Java 1.20.5.
pub const MAX_STACK_LIMIT: u16 = 99;

/// Rejection reasons when constructing an [`ItemStack`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStackError {
    /// Item identifier is not a valid namespaced resource location.
    InvalidItem,
    /// Maximum stack size is outside `1..=MAX_STACK_LIMIT`.
    InvalidMaxStack(u16),
    /// Stack count is zero or exceeds the stack's maximum size.
    InvalidCount(u16),
}

impl std::fmt::Display for ItemStackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidItem => f.write_str("invalid namespaced item id"),
            Self::InvalidMaxStack(max) => {
                write!(f, "max stack size {max} must be in 1..={MAX_STACK_LIMIT}")
            }
            Self::InvalidCount(count) => write!(f, "invalid item stack count {count}"),
        }
    }
}

impl std::error::Error for ItemStackError {}

/// Non-empty, capacity-bounded stack of a single item kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemStack {
    item: String,
    count: u16,
    max_stack: u16,
}

// Java Edition resource-location character rules.
fn valid_item_id(id: &str) -> bool {
    let Some((namespace, path)) = id.split_once(':') else {
        return false;
    };
    let is_namespace_byte = |b: u8| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.');
    !namespace.is_empty()
        && !path.is_empty()
        && namespace.bytes().all(is_namespace_byte)
        && path.bytes().all(|b| is_namespace_byte(b) || b == b'/')
}

impl ItemStack {
    /// Constructs a validated stack, checking the item id, max stack size, and count in order.
    pub fn try_new(
        item: impl Into<String>,
        count: u16,
        max_stack: u16,
    ) -> Result<Self, ItemStackError> {
        let item = item.into();
        if !valid_item_id(&item) {
            return Err(ItemStackError::InvalidItem);
        }
        if !(1..=MAX_STACK_LIMIT).contains(&max_stack) {
            return Err(ItemStackError::InvalidMaxStack(max_stack));
        }
        if !(1..=max_stack).contains(&count) {
            return Err(ItemStackError::InvalidCount(count));
        }
        Ok(Self {
            item,
            count,
            max_stack,
        })
    }

    /// Returns the namespaced item identifier.
    pub fn item(&self) -> &str {
        &self.item
    }

    /// Returns the current item count in this stack.
    pub fn count(&self) -> u16 {
        self.count
    }

    /// Returns the maximum stack size for this stack.
    pub fn max_stack(&self) -> u16 {
        self.max_stack
    }

    /// Reports whether `other` shares the same item identifier and maximum stack size.
    pub fn can_stack_with(&self, other: &Self) -> bool {
        self.item == other.item && self.max_stack == other.max_stack
    }

    /// Adds as many items as fit under `max_stack` and returns the overflow.
    pub fn add(&mut self, amount: u16) -> u16 {
        let space = self.max_stack.saturating_sub(self.count);
        let accepted = amount.min(space);
        self.count = self.count.saturating_add(accepted);
        amount - accepted
    }

    /// Splits up to `amount` items off into a new stack and returns any remaining stack.
    pub fn split(self, amount: u16) -> Result<(Self, Option<Self>), Self> {
        if amount == 0 {
            return Err(self);
        }
        let take = amount.min(self.count);
        let remaining = self.count - take;
        if remaining == 0 {
            return Ok((self, None));
        }
        let taken = Self {
            item: self.item.clone(),
            count: take,
            max_stack: self.max_stack,
        };
        let rest = Self {
            item: self.item,
            count: remaining,
            max_stack: self.max_stack,
        };
        Ok((taken, Some(rest)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_new_accepts_valid_stacks_and_default_max_stack() {
        let one = ItemStack::try_new("minecraft:stone", 1, 64).unwrap();
        assert_eq!(one.item(), "minecraft:stone");
        assert_eq!(one.count(), 1);
        assert_eq!(one.max_stack(), 64);

        let full = ItemStack::try_new("minecraft:stone", 64, DEFAULT_MAX_STACK).unwrap();
        assert_eq!(full.count(), 64);
        assert_eq!(full.max_stack(), DEFAULT_MAX_STACK);

        let potion = ItemStack::try_new("minecraft:potion", 1, 1).unwrap();
        assert_eq!(potion.item(), "minecraft:potion");
        assert_eq!(potion.count(), 1);
        assert_eq!(potion.max_stack(), 1);

        let max = ItemStack::try_new("a:b", 99, MAX_STACK_LIMIT).unwrap();
        assert_eq!(max.item(), "a:b");
        assert_eq!(max.count(), 99);
        assert_eq!(max.max_stack(), MAX_STACK_LIMIT);
    }

    #[test]
    fn validates_item_id_character_and_separator_rules() {
        for invalid in [
            "",
            "stone",
            ":stone",
            "minecraft:",
            "minecraft:stone:extra",
            "Minecraft:stone",
            "minecraft:Stone",
            "minecraft:sto ne",
            "my/mod:stone",
        ] {
            assert_eq!(
                ItemStack::try_new(invalid, 1, DEFAULT_MAX_STACK),
                Err(ItemStackError::InvalidItem),
                "expected InvalidItem for `{invalid}`"
            );
        }

        let custom = ItemStack::try_new("my_mod.x-1:block/a_b.c-2", 1, DEFAULT_MAX_STACK).unwrap();
        assert_eq!(custom.item(), "my_mod.x-1:block/a_b.c-2");
    }

    #[test]
    fn rejects_invalid_max_stack_and_prioritizes_earlier_checks() {
        assert_eq!(
            ItemStack::try_new("minecraft:stone", 1, 0),
            Err(ItemStackError::InvalidMaxStack(0))
        );
        assert_eq!(
            ItemStack::try_new("minecraft:stone", 1, 100),
            Err(ItemStackError::InvalidMaxStack(100))
        );
        assert_eq!(
            ItemStack::try_new("NotValid", 1, 0),
            Err(ItemStackError::InvalidItem)
        );
        assert_eq!(
            ItemStack::try_new("NotValid", 0, 100),
            Err(ItemStackError::InvalidItem)
        );
        assert_eq!(
            ItemStack::try_new("minecraft:stone", 0, 0),
            Err(ItemStackError::InvalidMaxStack(0))
        );
    }

    #[test]
    fn rejects_zero_or_over_limit_count() {
        assert_eq!(
            ItemStack::try_new("minecraft:stone", 0, 64),
            Err(ItemStackError::InvalidCount(0))
        );
        assert_eq!(
            ItemStack::try_new("minecraft:stone", 65, 64),
            Err(ItemStackError::InvalidCount(65))
        );
        assert_eq!(
            ItemStack::try_new("minecraft:potion", 2, 1),
            Err(ItemStackError::InvalidCount(2))
        );
    }

    #[test]
    fn error_display_is_lowercase_and_never_echoes_item_id() {
        let item_err = ItemStack::try_new("secret_ns:Bad_Item", 1, 64).unwrap_err();
        let msg = item_err.to_string();
        assert!(!msg.is_empty());
        assert_eq!(msg, msg.to_lowercase());
        assert!(!msg.contains("secret_ns"));
        assert!(!msg.contains("bad_item"));

        for err in [
            ItemStackError::InvalidMaxStack(0),
            ItemStackError::InvalidCount(65),
        ] {
            let text = err.to_string();
            assert!(!text.is_empty());
            assert_eq!(text, text.to_lowercase());
            let _: &dyn std::error::Error = &err;
        }
    }

    #[test]
    fn can_stack_with_requires_same_id_and_max_stack() {
        let stone_one = ItemStack::try_new("minecraft:stone", 1, 64).unwrap();
        let stone_ten = ItemStack::try_new("minecraft:stone", 10, 64).unwrap();
        let dirt = ItemStack::try_new("minecraft:dirt", 1, 64).unwrap();
        let stone_small_max = ItemStack::try_new("minecraft:stone", 1, 16).unwrap();

        assert!(stone_one.can_stack_with(&stone_ten));
        assert!(!stone_one.can_stack_with(&dirt));
        assert!(!stone_one.can_stack_with(&stone_small_max));
    }

    #[test]
    fn add_caps_at_max_stack_and_returns_overflow() {
        let mut stack = ItemStack::try_new("minecraft:stone", 60, 64).unwrap();
        assert_eq!(stack.add(0), 0);
        assert_eq!(stack.count(), 60);

        assert_eq!(stack.add(4), 0);
        assert_eq!(stack.count(), 64);

        let mut near_full = ItemStack::try_new("minecraft:stone", 60, 64).unwrap();
        assert_eq!(near_full.add(10), 6);
        assert_eq!(near_full.count(), 64);

        assert_eq!(near_full.add(25), 25);
        assert_eq!(near_full.count(), 64);

        let mut single = ItemStack::try_new("minecraft:stone", 1, 64).unwrap();
        assert_eq!(single.add(u16::MAX), u16::MAX - 63);
        assert_eq!(single.count(), 64);

        let mut pearl = ItemStack::try_new("minecraft:ender_pearl", 1, 16).unwrap();
        assert_eq!(pearl.add(20), 5);
        assert_eq!(pearl.count(), 16);
    }

    #[test]
    fn split_returns_err_on_zero_and_keeps_non_empty_stacks() {
        let original = ItemStack::try_new("minecraft:ender_pearl", 10, 16).unwrap();
        let returned = original.clone().split(0).unwrap_err();
        assert_eq!(returned, original);

        let (taken, rest) = original.clone().split(4).unwrap();
        assert_eq!(taken.item(), "minecraft:ender_pearl");
        assert_eq!(taken.count(), 4);
        assert_eq!(taken.max_stack(), 16);
        let rest = rest.expect("remainder should exist");
        assert_eq!(rest.item(), "minecraft:ender_pearl");
        assert_eq!(rest.count(), 6);
        assert_eq!(rest.max_stack(), 16);

        let (taken_exact, rest_exact) = original.clone().split(10).unwrap();
        assert_eq!(taken_exact.item(), "minecraft:ender_pearl");
        assert_eq!(taken_exact.count(), 10);
        assert_eq!(taken_exact.max_stack(), 16);
        assert_eq!(rest_exact, None);

        let (taken_more, rest_more) = original.split(50).unwrap();
        assert_eq!(taken_more.item(), "minecraft:ender_pearl");
        assert_eq!(taken_more.count(), 10);
        assert_eq!(taken_more.max_stack(), 16);
        assert_eq!(rest_more, None);
    }
}
