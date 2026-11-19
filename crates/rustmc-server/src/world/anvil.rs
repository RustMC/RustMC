//! Milestone M4: Anvil Repairing, Renaming, and Item Durability Combination Logic.
//!
//! Provides authoritative anvil repair models for tool/armor durability combination,
//! material unit repairs (such as iron ingots or diamonds), item renaming with validation,
//! prior work penalty escalation (`2*penalty + 1`), and state degradation progression
//! (`Undamaged` -> `Chipped` -> `Damaged` -> `Broken`) with a 12% degradation chance.
//!
//! Sources: the level costs, the 40-level "Too Expensive!" cap, the 12%
//! durability bonus, the 25%-per-unit material repair, the `2 * penalty + 1`
//! prior work penalty, the 12% degradation chance, the 50-character name
//! limit, and Java Edition enchantment combination follow the public
//! minecraft.wiki `Anvil_mechanics` (revision 3804134), `Anvil` (revision
//! 3902804), `Item_repair` (revision 3791643), and `Enchantment_definition`
//! (revision 3843772) articles; offline fallback item durabilities follow
//! the minecraft.wiki `Durability` article, while [`AnvilRules`] reads
//! `minecraft:max_damage`, `minecraft:repairable`, and enchantment rules
//! from [`ItemDefaults`], [`ItemTags`], and [`EnchantmentRegistry`].

use crate::item::{
    DEFAULT_MAX_STACK, ItemDefaults, ItemStack, ItemStackError, TextComponent, items_equal,
    normalize_item_id,
};
use crate::vanilla::feature::ItemTags;
use crate::vanilla::random::RandomSource;
use crate::world::enchantment::EnchantmentRegistry;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

/// Maximum allowable characters for an item's custom name in an anvil text box (minecraft.wiki `Anvil`).
pub const MAX_NAME_LENGTH: usize = 50;

/// Experience level cost cap in Survival mode ("Too Expensive!" at 40 levels; minecraft.wiki `Anvil_mechanics`).
pub const SURVIVAL_LEVEL_CAP: u32 = 40;

/// Probability of anvil degradation upon completing a repair (12%; minecraft.wiki `Anvil`).
pub const DEGRADATION_CHANCE: f64 = 0.12;

/// Bonus factor applied to max durability during same-item combination (12%; minecraft.wiki `Anvil_mechanics`).
pub const DURABILITY_BONUS_FACTOR: f64 = 0.12;

/// Fraction of max durability restored per unit of repair material (25%; minecraft.wiki `Anvil_mechanics`).
pub const MATERIAL_REPAIR_FRACTION: f64 = 0.25;

/// Maximum material units consumed in a single material repair (4 units; minecraft.wiki `Anvil_mechanics`).
const MAX_REPAIR_MATERIAL_UNITS: u32 = 4;

/// Cost in experience levels to rename an item when the name is modified (minecraft.wiki `Anvil_mechanics`).
pub const RENAME_LEVEL_COST: u32 = 1;

/// Cost in experience levels for tool durability combination (minecraft.wiki `Anvil_mechanics`).
pub const COMBINE_REPAIR_LEVEL_COST: u32 = 2;

/// Experience level surcharge per incompatible enchantment on the target item in Java Edition
/// (minecraft.wiki `Anvil mechanics`, revision 3804134: "Add one level for every incompatible
/// enchantment on the target (in Java Edition).").
pub const INCOMPATIBLE_ENCHANTMENT_LEVEL_COST: u32 = 1;

/// Divisor applied to an enchantment's `anvil_cost` when the sacrifice is an enchanted book
/// (minecraft.wiki `Enchantment definition`, revision 3843772: "Halved when adding using a
/// book, multiplied by the level of the enchantment.").
const BOOK_ANVIL_COST_DIVISOR: u32 = 2;

/// Minimum multiplier for applying an enchantment from an enchanted book in an anvil
/// (minecraft.wiki `Anvil mechanics`, revision 3804134: book multiplier is 1 when item
/// multiplier is 1 or 2).
const MIN_BOOK_ANVIL_COST_MULTIPLIER: u32 = 1;

/// Canonical item identifier for enchanted books (minecraft.wiki `Anvil mechanics`, revision 3804134).
pub const ENCHANTED_BOOK_ID: &str = "minecraft:enchanted_book";

/// Max durability of `minecraft:mace` in Java Edition (minecraft.wiki `Mace` / `Durability`: 500).
const MACE_MAX_DURABILITY: u32 = 500;

/// State of an anvil block in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AnvilState {
    /// Fully intact anvil.
    #[default]
    Undamaged,
    /// Slightly damaged anvil.
    Chipped,
    /// Heavily damaged anvil.
    Damaged,
    /// Destroyed anvil (cannot perform repairs).
    Broken,
}

impl AnvilState {
    /// Returns the next degraded state when this anvil wears down.
    #[must_use]
    pub const fn next_degraded(self) -> Self {
        match self {
            Self::Undamaged => Self::Chipped,
            Self::Chipped => Self::Damaged,
            Self::Damaged => Self::Broken,
            Self::Broken => Self::Broken,
        }
    }

    /// Whether this anvil is broken.
    #[must_use]
    pub const fn is_broken(self) -> bool {
        matches!(self, Self::Broken)
    }

    /// Rolls for degradation given a random draw in `[0.0, 1.0)`.
    ///
    /// If `roll < DEGRADATION_CHANCE` (12%), returns `Some(next_degraded)`.
    /// Otherwise returns `None`. Broken anvils never degrade further.
    #[must_use]
    pub fn roll_degradation(self, roll: f64) -> Option<Self> {
        if self.is_broken() {
            return None;
        }
        if roll < DEGRADATION_CHANCE {
            Some(self.next_degraded())
        } else {
            None
        }
    }
}

/// Returns the standard vanilla max durability for a known tool, weapon, or armor piece.
///
/// Offline fallback table used when no [`ItemDefaults`] report is loaded.
/// Prefer [`AnvilRules`] for data-backed durability lookups.
#[must_use]
pub fn default_max_durability(item: &str) -> Option<u32> {
    match normalize_item_id(item) {
        // Wooden tools (59)
        "wooden_sword" | "wooden_pickaxe" | "wooden_axe" | "wooden_shovel" | "wooden_hoe" => {
            Some(59)
        }
        // Stone tools (131)
        "stone_sword" | "stone_pickaxe" | "stone_axe" | "stone_shovel" | "stone_hoe" => Some(131),
        // Iron tools (250)
        "iron_sword" | "iron_pickaxe" | "iron_axe" | "iron_shovel" | "iron_hoe" => Some(250),
        // Golden tools (32)
        "golden_sword" | "golden_pickaxe" | "golden_axe" | "golden_shovel" | "golden_hoe" => {
            Some(32)
        }
        // Diamond tools (1561)
        "diamond_sword" | "diamond_pickaxe" | "diamond_axe" | "diamond_shovel" | "diamond_hoe" => {
            Some(1561)
        }
        // Netherite tools (2031)
        "netherite_sword" | "netherite_pickaxe" | "netherite_axe" | "netherite_shovel"
        | "netherite_hoe" => Some(2031),

        // Leather armor
        "leather_helmet" => Some(55),
        "leather_chestplate" => Some(80),
        "leather_leggings" => Some(75),
        "leather_boots" => Some(65),

        // Chainmail & Iron armor
        "chainmail_helmet" | "iron_helmet" => Some(165),
        "chainmail_chestplate" | "iron_chestplate" => Some(240),
        "chainmail_leggings" | "iron_leggings" => Some(225),
        "chainmail_boots" | "iron_boots" => Some(195),

        // Golden armor
        "golden_helmet" => Some(77),
        "golden_chestplate" => Some(112),
        "golden_leggings" => Some(105),
        "golden_boots" => Some(91),

        // Diamond armor
        "diamond_helmet" => Some(363),
        "diamond_chestplate" => Some(528),
        "diamond_leggings" => Some(495),
        "diamond_boots" => Some(429),

        // Netherite armor
        "netherite_helmet" => Some(407),
        "netherite_chestplate" => Some(592),
        "netherite_leggings" => Some(555),
        "netherite_boots" => Some(481),

        // Other utilities & armor
        "turtle_helmet" => Some(275),
        "elytra" => Some(432),
        "bow" => Some(384),
        "crossbow" => Some(465),
        "fishing_rod" => Some(64),
        "flint_and_steel" => Some(64),
        "shears" => Some(238),
        "shield" => Some(336),
        "trident" => Some(250),
        "brush" => Some(64),
        "carrot_on_a_stick" => Some(25),
        "warped_fungus_on_a_stick" => Some(100),
        "mace" => Some(MACE_MAX_DURABILITY),
        _ => None,
    }
}

/// Checks whether `material` is a valid repair ingredient for `tool`.
///
/// Offline fallback table used when no [`ItemDefaults`] / [`ItemTags`] data is loaded.
/// Prefer [`AnvilRules`] for data-backed repair-material lookups.
#[must_use]
pub fn is_repair_material(tool: &str, material: &str) -> bool {
    let t = normalize_item_id(tool);
    let m = normalize_item_id(material);

    if t.starts_with("iron_") || t.starts_with("chainmail_") {
        return m == "iron_ingot";
    }
    if t.starts_with("diamond_") {
        return m == "diamond";
    }
    if t.starts_with("golden_") {
        return m == "gold_ingot";
    }
    if t.starts_with("netherite_") {
        return m == "netherite_ingot";
    }
    if t.starts_with("wooden_") || t == "shield" {
        return m.ends_with("_planks");
    }
    if t.starts_with("stone_") {
        return matches!(m, "cobblestone" | "cobbled_deepslate" | "blackstone");
    }
    if t.starts_with("leather_") {
        return m == "leather";
    }
    if t == "turtle_helmet" {
        return m == "turtle_scute";
    }
    if t == "elytra" {
        return m == "phantom_membrane";
    }
    if t == "mace" {
        return m == "breeze_rod";
    }

    false
}

/// Validation failures when constructing or updating an [`AnvilItem`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnvilItemError {
    /// Underlying [`ItemStack`] rejected item identifier or stack count.
    InvalidStack(ItemStackError),
    /// A damageable item cannot have `max_durability == 0`.
    ZeroMaxDurability,
    /// Remaining durability cannot exceed `max_durability`.
    DurabilityExceedsMax {
        durability: u32,
        max_durability: u32,
    },
    /// Custom name exceeds [`MAX_NAME_LENGTH`] characters.
    NameTooLong { len: usize, max: usize },
}

impl fmt::Display for AnvilItemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStack(err) => write!(f, "{err}"),
            Self::ZeroMaxDurability => write!(f, "damageable item must have max_durability > 0"),
            Self::DurabilityExceedsMax {
                durability,
                max_durability,
            } => write!(
                f,
                "durability {durability} exceeds max_durability {max_durability}"
            ),
            Self::NameTooLong { len, max } => {
                write!(f, "custom name length {len} exceeds limit of {max} chars")
            }
        }
    }
}

impl std::error::Error for AnvilItemError {}

impl From<ItemStackError> for AnvilItemError {
    fn from(err: ItemStackError) -> Self {
        Self::InvalidStack(err)
    }
}

fn validate_anvil_item_invariants(
    is_known_damageable: bool,
    durability: u32,
    max_durability: u32,
    custom_name: Option<&str>,
) -> Result<(), AnvilItemError> {
    if is_known_damageable && max_durability == 0 {
        return Err(AnvilItemError::ZeroMaxDurability);
    }
    if durability > max_durability {
        return Err(AnvilItemError::DurabilityExceedsMax {
            durability,
            max_durability,
        });
    }
    if let Some(name) = custom_name {
        let len = name.chars().count();
        if len > MAX_NAME_LENGTH {
            return Err(AnvilItemError::NameTooLong {
                len,
                max: MAX_NAME_LENGTH,
            });
        }
    }
    Ok(())
}

fn max_durability_override_for(item: &str, max_durability: u32) -> Option<u32> {
    (max_durability != default_max_durability(item).unwrap_or(0)).then_some(max_durability)
}

fn damage_for_durability(durability: u32, max_durability: u32) -> Option<u32> {
    let damage = max_durability.saturating_sub(durability);
    (damage > 0).then_some(damage)
}

fn set_item_enchantments(
    stack: &mut ItemStack,
    is_enchanted_book: bool,
    enchantments: BTreeMap<String, u8>,
) -> Result<(), AnvilItemError> {
    let res = if is_enchanted_book {
        stack.components_mut().set_stored_enchantments(enchantments)
    } else {
        stack.components_mut().set_enchantments(enchantments)
    };
    res.map_err(|err| AnvilItemError::InvalidStack(ItemStackError::InvalidComponents(err)))
}

fn apply_anvil_components(
    mut stack: ItemStack,
    durability: u32,
    max_durability: u32,
    repair_cost: u32,
    custom_name: Option<String>,
    enchantments: BTreeMap<String, u8>,
    is_enchanted_book: bool,
) -> Result<ItemStack, AnvilItemError> {
    {
        let components = stack.components_mut();
        components.set_damage(damage_for_durability(durability, max_durability));
        components.set_repair_cost((repair_cost > 0).then_some(repair_cost));
        components.set_custom_name(custom_name.map(TextComponent::Literal));
    }
    set_item_enchantments(&mut stack, is_enchanted_book, enchantments)?;
    Ok(stack)
}

/// An anvil slot item wrapping the shared [`ItemStack`], storing damage,
/// prior work penalty (`repair_cost`), custom name, active `enchantments`,
/// and enchanted-book `stored_enchantments` in the stack's [`crate::item::ItemComponents`].
///
/// Sources (minecraft.wiki `Data component format` revision 3903955):
/// - `Data component format/damage` (revision 3536877):
///   "`minecraft:damage`: The number of uses consumed (not remaining) of the item's
///   durability. Must be a non-negative integer, defaults to 0."
/// - `Data component format/max damage` (revision 3389861):
///   "`minecraft:max_damage`: The maximum amount of damage that this item can take."
/// - `Data component format/stored enchantments` (revision 3849830):
///   "`minecraft:stored_enchantments`: Contains a map of inactive enchantments and
///   their levels."
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AnvilItem {
    stack: ItemStack,
    max_durability_override: Option<u32>,
    is_enchanted_book: bool,
}

impl AnvilItem {
    /// Creates a validated [`AnvilItem`] from a shared [`ItemStack`] and explicit metadata.
    ///
    /// Uses the offline [`default_max_durability`] table to reject zero max durability on
    /// known fallback damageable items; for data-backed validation, use [`AnvilRules::new_item`].
    pub fn new(
        stack: ItemStack,
        durability: u32,
        max_durability: u32,
        repair_cost: u32,
        custom_name: Option<String>,
    ) -> Result<Self, AnvilItemError> {
        let is_enchanted_book = items_equal(stack.item(), ENCHANTED_BOOK_ID);
        Self::new_with_enchantments(
            stack,
            durability,
            max_durability,
            repair_cost,
            custom_name,
            BTreeMap::new(),
            is_enchanted_book,
        )
    }

    /// Creates a validated [`AnvilItem`] with an explicit enchantment map and enchanted-book flag.
    ///
    /// Uses the offline [`default_max_durability`] table for zero-durability checks;
    /// for data-backed validation, use [`AnvilRules::new_item`] combined with
    /// [`AnvilItem::with_enchantments`] and [`AnvilItem::with_enchanted_book`].
    pub fn new_with_enchantments(
        stack: ItemStack,
        durability: u32,
        max_durability: u32,
        repair_cost: u32,
        custom_name: Option<String>,
        enchantments: BTreeMap<String, u8>,
        is_enchanted_book: bool,
    ) -> Result<Self, AnvilItemError> {
        validate_anvil_item_invariants(
            default_max_durability(stack.item()).is_some(),
            durability,
            max_durability,
            custom_name.as_deref(),
        )?;
        let max_durability_override = max_durability_override_for(stack.item(), max_durability);
        let stack = apply_anvil_components(
            stack,
            durability,
            max_durability,
            repair_cost,
            custom_name,
            enchantments,
            is_enchanted_book,
        )?;
        Ok(Self {
            stack,
            max_durability_override,
            is_enchanted_book,
        })
    }

    /// Creates an [`AnvilItem`] from an existing shared [`ItemStack`], populating
    /// default durability if the item is a known damageable tool, weapon, or armor piece.
    ///
    /// Offline fallback constructor using [`default_max_durability`]; prefer
    /// [`AnvilRules::item_from_stack`] when item report data is loaded.
    #[must_use]
    pub fn from_stack(stack: ItemStack) -> Self {
        let is_enchanted_book = items_equal(stack.item(), ENCHANTED_BOOK_ID)
            || !stack.components().stored_enchantments().is_empty();
        Self {
            stack,
            max_durability_override: None,
            is_enchanted_book,
        }
    }

    /// Creates a validated [`AnvilItem`], checking item ID and count via [`ItemStack::try_new`].
    ///
    /// Offline fallback constructor using [`default_max_durability`]; prefer
    /// [`AnvilRules::try_new_item`] when item report data is loaded.
    pub fn try_new(item: impl Into<String>, count: u16) -> Result<Self, ItemStackError> {
        let item = item.into();
        let max_durability = default_max_durability(&item).unwrap_or(0);
        let max_stack = if max_durability > 0 {
            1
        } else {
            DEFAULT_MAX_STACK
        };
        let is_enchanted_book = items_equal(&item, ENCHANTED_BOOK_ID);
        let stack = ItemStack::try_new(item, count, max_stack)?;
        Ok(Self {
            stack,
            max_durability_override: None,
            is_enchanted_book,
        })
    }

    /// Returns the underlying shared [`ItemStack`].
    #[must_use]
    pub fn stack(&self) -> &ItemStack {
        &self.stack
    }

    /// Returns the namespaced item identifier.
    #[must_use]
    pub fn item(&self) -> &str {
        self.stack.item()
    }

    /// Returns the stack count.
    #[must_use]
    pub fn count(&self) -> u16 {
        self.stack.count()
    }

    /// Returns current remaining durability (`max_durability - damage`).
    #[must_use]
    pub fn durability(&self) -> u32 {
        self.max_durability()
            .saturating_sub(self.stack.components().damage().unwrap_or(0))
    }

    /// Returns maximum durability (`0` for non-damageable items).
    #[must_use]
    pub fn max_durability(&self) -> u32 {
        self.max_durability_override
            .unwrap_or_else(|| default_max_durability(self.stack.item()).unwrap_or(0))
    }

    /// Returns prior work penalty (`repair_cost`).
    #[must_use]
    pub fn repair_cost(&self) -> u32 {
        self.stack.components().repair_cost().unwrap_or(0)
    }

    /// Returns the custom display name, if set.
    #[must_use]
    pub fn custom_name(&self) -> Option<&str> {
        self.stack.components().custom_name()
    }

    /// Returns the enchantments on this item (or stored on an enchanted book).
    #[must_use]
    pub fn enchantments(&self) -> &BTreeMap<String, u8> {
        if self.is_enchanted_book {
            self.stack.components().stored_enchantments()
        } else {
            self.stack.components().enchantments()
        }
    }

    /// Returns `true` if this item is treated as an enchanted book (`minecraft:enchanted_book`)
    /// when combining enchantments in an anvil.
    #[must_use]
    pub const fn is_enchanted_book(&self) -> bool {
        self.is_enchanted_book
    }

    /// Replaces the item's enchantment map.
    #[must_use]
    pub fn with_enchantments(mut self, enchantments: BTreeMap<String, u8>) -> Self {
        let filtered: BTreeMap<String, u8> = enchantments
            .into_iter()
            .filter(|(_, level)| *level > 0)
            .collect();
        let _ = set_item_enchantments(&mut self.stack, self.is_enchanted_book, filtered);
        self
    }

    /// Inserts or updates a single enchantment level on this item.
    #[must_use]
    pub fn with_enchantment(mut self, id: impl Into<String>, level: u8) -> Self {
        let id = id.into();
        let components = self.stack.components_mut();
        if level == 0 {
            if self.is_enchanted_book {
                components.remove_stored_enchantment(&id);
            } else {
                components.remove_enchantment(&id);
            }
        } else if self.is_enchanted_book {
            let _ = components.set_stored_enchantment(id, level);
        } else {
            let _ = components.set_enchantment(id, level);
        }
        self
    }

    /// Explicitly sets whether this item acts as an enchanted book in anvil combinations.
    #[must_use]
    pub const fn with_enchanted_book(mut self, is_enchanted_book: bool) -> Self {
        self.is_enchanted_book = is_enchanted_book;
        self
    }

    /// Updates remaining durability, enforcing `durability <= max_durability`
    /// and `max_durability > 0`.
    pub fn with_durability(mut self, durability: u32) -> Result<Self, AnvilItemError> {
        let max_durability = self.max_durability();
        if max_durability == 0 {
            return Err(AnvilItemError::ZeroMaxDurability);
        }
        if durability > max_durability {
            return Err(AnvilItemError::DurabilityExceedsMax {
                durability,
                max_durability,
            });
        }
        self.stack
            .components_mut()
            .set_damage(damage_for_durability(durability, max_durability));
        Ok(self)
    }

    /// Updates maximum durability, enforcing invariants for damageable items and current durability.
    pub fn with_max_durability(mut self, max_durability: u32) -> Result<Self, AnvilItemError> {
        let cur_durability = self.durability();
        validate_anvil_item_invariants(
            default_max_durability(self.stack.item()).is_some(),
            cur_durability,
            max_durability,
            self.custom_name(),
        )?;
        self.max_durability_override =
            max_durability_override_for(self.stack.item(), max_durability);
        self.stack
            .components_mut()
            .set_damage(damage_for_durability(cur_durability, max_durability));
        Ok(self)
    }

    /// Sets the prior work penalty (`repair_cost`).
    #[must_use]
    pub fn with_repair_cost(mut self, repair_cost: u32) -> Self {
        self.stack
            .components_mut()
            .set_repair_cost((repair_cost > 0).then_some(repair_cost));
        self
    }

    /// Updates the custom display name, enforcing [`MAX_NAME_LENGTH`].
    pub fn with_custom_name(mut self, name: impl Into<String>) -> Result<Self, AnvilItemError> {
        let name = name.into();
        let max_durability = self.max_durability();
        validate_anvil_item_invariants(
            default_max_durability(self.stack.item()).is_some(),
            self.durability(),
            max_durability,
            Some(&name),
        )?;
        self.stack
            .components_mut()
            .set_custom_name(Some(TextComponent::Literal(name)));
        Ok(self)
    }

    /// Returns damage taken (missing durability).
    #[must_use]
    pub fn damage(&self) -> u32 {
        self.max_durability().saturating_sub(self.durability())
    }

    /// Returns whether this item is damageable (`max_durability > 0`).
    #[must_use]
    pub fn is_damageable(&self) -> bool {
        self.max_durability() > 0
    }

    /// Returns whether this item has taken damage (`durability < max_durability`).
    #[must_use]
    pub fn is_damaged(&self) -> bool {
        self.is_damageable() && self.durability() < self.max_durability()
    }
}

/// Input parameters for an anvil operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnvilRepairInput {
    pub base_item: AnvilItem,
    pub sacrifice_item: Option<AnvilItem>,
    pub custom_name: Option<String>,
}

impl AnvilRepairInput {
    /// Creates a new `AnvilRepairInput`, validating custom name length (max 50 chars).
    pub fn new(
        base_item: AnvilItem,
        sacrifice_item: Option<AnvilItem>,
        custom_name: Option<String>,
    ) -> Result<Self, AnvilError> {
        if let Some(ref name) = custom_name {
            let len = name.chars().count();
            if len > MAX_NAME_LENGTH {
                return Err(AnvilError::NameTooLong {
                    len,
                    max: MAX_NAME_LENGTH,
                });
            }
        }
        Ok(Self {
            base_item,
            sacrifice_item,
            custom_name,
        })
    }

    /// Helper to create a rename-only anvil input.
    pub fn rename_only(
        base_item: AnvilItem,
        custom_name: impl Into<String>,
    ) -> Result<Self, AnvilError> {
        Self::new(base_item, None, Some(custom_name.into()))
    }

    /// Helper to create a same-item or material combination anvil input.
    pub fn combine(base_item: AnvilItem, sacrifice_item: AnvilItem) -> Self {
        Self {
            base_item,
            sacrifice_item: Some(sacrifice_item),
            custom_name: None,
        }
    }
}

/// Result of an anvil repair or renaming operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnvilRepairResult {
    /// The resulting repaired and/or renamed item stack.
    pub result_item: AnvilItem,
    /// Total experience level cost to perform the operation.
    pub repair_cost_levels: u32,
    /// Count of items consumed from the sacrifice slot (0 for rename-only, 1 for tools, 1-4 for materials).
    pub material_cost: u32,
    /// Next prior work penalty applied to the result item (`2 * penalty + 1`).
    pub next_prior_work_penalty: u32,
    /// New anvil state if the anvil degraded during repair completion (12% chance), or `None` if intact.
    pub degraded_state: Option<AnvilState>,
}

impl AnvilRepairResult {
    /// Returns true if the operation costs 40 or more levels (the "Too Expensive!" limit in Survival).
    #[must_use]
    pub fn is_too_expensive(&self) -> bool {
        self.repair_cost_levels >= SURVIVAL_LEVEL_CAP
    }
}

/// Errors returned by anvil repair operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnvilError {
    /// Custom name exceeds the 50-character limit.
    NameTooLong { len: usize, max: usize },
    /// Sacrifice item cannot be combined with or used to repair the base item.
    IncompatibleSacrificeItem,
    /// Base item is not damageable and cannot be repaired.
    NotDamageable,
    /// Base item is already at full durability and no other modifications were made.
    NoRepairNeeded,
    /// No valid anvil operation requested (e.g. no sacrifice and name unchanged).
    NoOperation,
    /// Total experience level cost meets or exceeds 40 in Survival mode ("Too Expensive!").
    TooExpensive { cost: u32, limit: u32 },
    /// The anvil is broken and cannot perform repairs.
    AnvilBroken,
}

impl fmt::Display for AnvilError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NameTooLong { len, max } => {
                write!(f, "custom name length {len} exceeds limit of {max} chars")
            }
            Self::IncompatibleSacrificeItem => {
                write!(f, "sacrifice item is incompatible with base item")
            }
            Self::NotDamageable => write!(f, "item is not damageable"),
            Self::NoRepairNeeded => {
                write!(f, "item is already fully repaired and cannot be combined")
            }
            Self::NoOperation => write!(f, "no valid repair or rename operation"),
            Self::TooExpensive { cost, limit } => {
                write!(
                    f,
                    "Too Expensive! Cost {cost} exceeds survival limit {limit}"
                )
            }
            Self::AnvilBroken => write!(f, "anvil is broken"),
        }
    }
}

impl std::error::Error for AnvilError {}

/// Data-backed anvil durability, repair-material, and enchantment-combining rules
/// built from [`ItemDefaults`], [`ItemTags`], and [`EnchantmentRegistry`].
///
/// Uses the official item report (`minecraft:max_damage` and resolved
/// `minecraft:repairable` components) instead of the offline fallback tables
/// [`default_max_durability`] and [`is_repair_material`], and applies Java Edition
/// enchantment combination semantics using loaded enchantment definitions.
///
/// Sources:
/// - minecraft.wiki `Anvil` (revision 3902804): "one material repairs 1/4 of
///   the item's maximum durability."
/// - minecraft.wiki `Anvil mechanics` (revision 3804134):
///   - "Each unit restores up to 25% total durability of the item (rounded down)
///     and costs 1 level per unit of material used in addition to any applicable
///     prior work penalties."
///   - "The anvil can be used to combine two of the same item, or an item with
///     an enchanted book."
///   - "If the sacrifice is a book, there is no repair, but the anvil tries to
///     combine the book's enchantments onto the target."
///   - "For each enchantment on the sacrifice: Ignore any enchantment that cannot
///     be applied to the target (e.g. Protection on a sword). Add one level for
///     every incompatible enchantment on the target (in Java Edition). If the
///     enchantment is compatible with the existing enchantments on the target:
///     For Java Edition, add the final level of the enchantment on the resulting
///     item multiplied by the multiplier from the table below."
///   - "Once the cost exceeds 39 levels, the anvil menu refuses the operation
///     with the message \"Too Expensive!\", no matter how high the player's
///     experience level is."
/// - minecraft.wiki `Enchantment definition` (revision 3843772):
///   - "`anvil_cost`: The base cost when applying this enchantment to another item
///     using an anvil. Halved when adding using a book, multiplied by the level
///     of the enchantment."
#[derive(Debug, Clone, Default)]
pub struct AnvilRules {
    defaults: ItemDefaults,
    tags: ItemTags,
    enchantments: EnchantmentRegistry,
}

impl AnvilRules {
    /// Builds data-backed anvil rules from loaded [`ItemDefaults`] and [`ItemTags`].
    #[must_use]
    pub fn new(defaults: &ItemDefaults, tags: &ItemTags) -> Self {
        Self {
            defaults: defaults.clone(),
            tags: tags.clone(),
            enchantments: EnchantmentRegistry::default(),
        }
    }

    /// Builds data-backed anvil rules with loaded item defaults, item tags, and
    /// [`EnchantmentRegistry`].
    #[must_use]
    pub fn with_enchantments(
        defaults: &ItemDefaults,
        tags: &ItemTags,
        enchantments: &EnchantmentRegistry,
    ) -> Self {
        Self {
            defaults: defaults.clone(),
            tags: tags.clone(),
            enchantments: enchantments.clone(),
        }
    }

    /// Builds anvil rules from an [`EnchantmentRegistry`] without item report defaults.
    #[must_use]
    pub fn from_enchantments(enchantments: &EnchantmentRegistry) -> Self {
        Self {
            defaults: ItemDefaults::default(),
            tags: ItemTags::default(),
            enchantments: enchantments.clone(),
        }
    }

    /// Attaches an [`EnchantmentRegistry`] to these [`AnvilRules`].
    #[must_use]
    pub fn with_enchantment_registry(mut self, enchantments: &EnchantmentRegistry) -> Self {
        self.enchantments = enchantments.clone();
        self
    }

    /// Returns the underlying [`ItemDefaults`] report.
    #[must_use]
    pub fn defaults(&self) -> &ItemDefaults {
        &self.defaults
    }

    /// Returns the underlying [`ItemTags`] registry.
    #[must_use]
    pub fn tags(&self) -> &ItemTags {
        &self.tags
    }

    /// Returns the underlying [`EnchantmentRegistry`].
    #[must_use]
    pub fn enchantments(&self) -> &EnchantmentRegistry {
        &self.enchantments
    }

    /// Returns the report `minecraft:max_damage` for `item`, if damageable.
    #[must_use]
    pub fn max_durability(&self, item: &str) -> Option<u32> {
        self.defaults.max_damage(item)
    }

    /// Returns the report `minecraft:max_stack_size` for `item`, if known.
    #[must_use]
    pub fn max_stack_size(&self, item: &str) -> Option<u8> {
        self.defaults.max_stack_size(item)
    }

    /// Returns the resolved repair material identifiers for `item`, if repairable.
    #[must_use]
    pub fn repair_materials(&self, item: &str) -> Option<Vec<String>> {
        self.defaults.repair_materials(item, &self.tags)
    }

    /// Checks whether `material` is a valid repair ingredient for `tool` in the report.
    #[must_use]
    pub fn is_repair_material(&self, tool: &str, material: &str) -> bool {
        self.repair_materials(tool).is_some_and(|materials| {
            materials
                .iter()
                .any(|candidate| items_equal(candidate, material))
        })
    }

    /// Returns the report `minecraft:enchantable` value for `item`, if present.
    #[must_use]
    pub fn enchantability(&self, item: &str) -> Option<u32> {
        self.defaults.enchantability(item)
    }

    /// Creates a validated [`AnvilItem`] from a shared [`ItemStack`] and explicit metadata,
    /// enforcing non-zero `max_durability` when `item` has `minecraft:max_damage` in the report.
    pub fn new_item(
        &self,
        stack: ItemStack,
        durability: u32,
        max_durability: u32,
        repair_cost: u32,
        custom_name: Option<String>,
    ) -> Result<AnvilItem, AnvilItemError> {
        validate_anvil_item_invariants(
            self.max_durability(stack.item()).is_some(),
            durability,
            max_durability,
            custom_name.as_deref(),
        )?;
        let is_enchanted_book = items_equal(stack.item(), ENCHANTED_BOOK_ID);
        let max_durability_override = max_durability_override_for(stack.item(), max_durability);
        let stack = apply_anvil_components(
            stack,
            durability,
            max_durability,
            repair_cost,
            custom_name,
            BTreeMap::new(),
            is_enchanted_book,
        )?;
        Ok(AnvilItem {
            stack,
            max_durability_override,
            is_enchanted_book,
        })
    }

    /// Creates an [`AnvilItem`] from an existing [`ItemStack`], populating
    /// `max_durability` from `minecraft:max_damage` in the report.
    #[must_use]
    pub fn item_from_stack(&self, stack: ItemStack) -> AnvilItem {
        let max_durability = self.max_durability(stack.item()).unwrap_or(0);
        let max_durability_override = max_durability_override_for(stack.item(), max_durability);
        let is_enchanted_book = items_equal(stack.item(), ENCHANTED_BOOK_ID)
            || !stack.components().stored_enchantments().is_empty();
        AnvilItem {
            stack,
            max_durability_override,
            is_enchanted_book,
        }
    }

    /// Creates a validated [`AnvilItem`] using `minecraft:max_damage` and
    /// `minecraft:max_stack_size` from the report.
    pub fn try_new_item(
        &self,
        item: impl Into<String>,
        count: u16,
    ) -> Result<AnvilItem, ItemStackError> {
        let item = item.into();
        let max_durability = self.max_durability(&item).unwrap_or(0);
        let max_stack = self
            .max_stack_size(&item)
            .map(u16::from)
            .unwrap_or_else(|| {
                if max_durability > 0 {
                    1
                } else {
                    DEFAULT_MAX_STACK
                }
            });
        let is_enchanted_book = items_equal(&item, ENCHANTED_BOOK_ID);
        let max_durability_override = max_durability_override_for(&item, max_durability);
        let stack = ItemStack::try_new(item, count, max_stack)?;
        Ok(AnvilItem {
            stack,
            max_durability_override,
            is_enchanted_book,
        })
    }

    /// Calculates the anvil repair result in preview mode (`degraded_state = None`)
    /// using the report durability and repair-material rules.
    pub fn calculate_repair(
        &self,
        input: &AnvilRepairInput,
        is_creative: bool,
    ) -> Result<AnvilRepairResult, AnvilError> {
        calculate_repair_internal(
            input,
            AnvilState::Undamaged,
            is_creative,
            None,
            RepairSource::Data(self),
        )
    }

    /// Executes an anvil repair with a specified degradation roll `[0.0, 1.0)`
    /// using the report durability and repair-material rules.
    pub fn repair_with_roll(
        &self,
        input: &AnvilRepairInput,
        anvil_state: AnvilState,
        is_creative: bool,
        roll: f64,
    ) -> Result<AnvilRepairResult, AnvilError> {
        calculate_repair_internal(
            input,
            anvil_state,
            is_creative,
            Some(roll),
            RepairSource::Data(self),
        )
    }

    /// Executes an anvil repair on an `Undamaged` anvil using the caller's random source
    /// and the report durability and repair-material rules.
    pub fn repair(
        &self,
        input: &AnvilRepairInput,
        is_creative: bool,
        rng: &mut RandomSource,
    ) -> Result<AnvilRepairResult, AnvilError> {
        self.repair_with_state(input, AnvilState::Undamaged, is_creative, rng)
    }

    /// Executes an anvil repair with the given `anvil_state` using the caller's random
    /// source and the report durability and repair-material rules.
    pub fn repair_with_state(
        &self,
        input: &AnvilRepairInput,
        anvil_state: AnvilState,
        is_creative: bool,
        rng: &mut RandomSource,
    ) -> Result<AnvilRepairResult, AnvilError> {
        let roll = rng.next_double();
        calculate_repair_internal(
            input,
            anvil_state,
            is_creative,
            Some(roll),
            RepairSource::Data(self),
        )
    }
}

#[derive(Clone, Copy)]
enum RepairSource<'a> {
    Fallback,
    Data(&'a AnvilRules),
}

impl<'a> RepairSource<'a> {
    fn is_repair_material(self, tool: &str, material: &str) -> bool {
        match self {
            Self::Fallback => is_repair_material(tool, material),
            Self::Data(rules) => rules.is_repair_material(tool, material),
        }
    }

    fn is_known_damageable(self, item: &str) -> bool {
        match self {
            Self::Fallback => default_max_durability(item).is_some(),
            Self::Data(rules) => rules.max_durability(item).is_some(),
        }
    }

    fn enchantments(self) -> Option<&'a EnchantmentRegistry> {
        match self {
            Self::Fallback => None,
            Self::Data(rules) => Some(&rules.enchantments),
        }
    }
}

struct EnchantmentCombineOutcome {
    enchantments: BTreeMap<String, u8>,
    enchant_cost: u32,
    any_applied: bool,
    any_incompatible: bool,
}

fn canonical_enchantment_id(registry: &EnchantmentRegistry, id: &str) -> String {
    registry
        .get(id)
        .map_or_else(|| id.to_owned(), |def| def.id().to_owned())
}

/// Combines enchantments from `sacrifice` onto `base` following Java Edition
/// rules (minecraft.wiki `Anvil mechanics`, revision 3804134, and
/// `Enchantment definition`, revision 3843772).
fn combine_enchantments(
    base: &AnvilItem,
    sacrifice: &AnvilItem,
    registry: &EnchantmentRegistry,
) -> EnchantmentCombineOutcome {
    let mut result_enchantments = BTreeMap::new();
    for (id, &level) in base.enchantments() {
        if level > 0 {
            result_enchantments.insert(canonical_enchantment_id(registry, id), level);
        }
    }

    let mut enchant_cost = 0u32;
    let mut any_applied = false;
    let mut any_incompatible = false;

    for (raw_id, &sac_level) in sacrifice.enchantments() {
        if sac_level == 0 {
            continue;
        }
        let Some(def) = registry.get(raw_id) else {
            continue;
        };
        let ench_id = def.id().to_owned();

        // minecraft.wiki `Anvil mechanics` (revision 3804134):
        // "Ignore any enchantment that cannot be applied to the target (e.g. Protection on a sword)."
        // Enchanted books accept all valid enchantments for book-book merging.
        if !base.is_enchanted_book() && !registry.can_apply(&ench_id, base.item()) {
            continue;
        }

        // minecraft.wiki `Anvil mechanics` (revision 3804134):
        // "Add one level for every incompatible enchantment on the target (in Java Edition)."
        let incompatible_count = result_enchantments
            .keys()
            .filter(|existing| {
                existing.as_str() != ench_id.as_str()
                    && registry.get(existing).is_some()
                    && !registry.are_compatible(&ench_id, existing)
            })
            .count() as u32;

        if incompatible_count > 0 {
            enchant_cost = enchant_cost.saturating_add(
                incompatible_count.saturating_mul(INCOMPATIBLE_ENCHANTMENT_LEVEL_COST),
            );
            any_incompatible = true;
            continue;
        }

        // minecraft.wiki `Anvil mechanics` (revision 3804134):
        // "and the level of the enchantment on the sacrifice item is greater, the enchantment
        // level on the target item is raised to match it; and the level of the enchantment on
        // the sacrifice item is equal AND the enchantment is not already at the maximum level,
        // the enchantment on the target item gains one level; and the level of the enchantment
        // on the sacrifice item is less, the enchantment level on the target item is unaffected."
        let merged_level = match result_enchantments.get(&ench_id).copied() {
            Some(target_level) => match sac_level.cmp(&target_level) {
                Ordering::Greater => sac_level,
                Ordering::Equal => {
                    if target_level < def.max_level() {
                        target_level.saturating_add(1)
                    } else {
                        target_level
                    }
                }
                Ordering::Less => target_level,
            },
            None => sac_level,
        };

        // minecraft.wiki `Enchantment definition` (revision 3843772):
        // "`anvil_cost`: The base cost when applying this enchantment to another item using an
        // anvil. Halved when adding using a book, multiplied by the level of the enchantment."
        // minecraft.wiki `Anvil mechanics` (revision 3804134):
        // "For Java Edition, add the final level of the enchantment on the resulting item
        // multiplied by the multiplier from the table below."
        let multiplier = if sacrifice.is_enchanted_book() {
            (def.anvil_cost() / BOOK_ANVIL_COST_DIVISOR).max(MIN_BOOK_ANVIL_COST_MULTIPLIER)
        } else {
            def.anvil_cost()
        };
        enchant_cost =
            enchant_cost.saturating_add(multiplier.saturating_mul(u32::from(merged_level)));
        result_enchantments.insert(ench_id, merged_level);
        any_applied = true;
    }

    EnchantmentCombineOutcome {
        enchantments: result_enchantments,
        enchant_cost,
        any_applied,
        any_incompatible,
    }
}

/// Calculates and executes the anvil operation with full validation, durability combination math,
/// level calculation, and deterministic degradation roll.
fn calculate_repair_internal(
    input: &AnvilRepairInput,
    anvil_state: AnvilState,
    is_creative: bool,
    roll: Option<f64>,
    source: RepairSource<'_>,
) -> Result<AnvilRepairResult, AnvilError> {
    if anvil_state.is_broken() {
        return Err(AnvilError::AnvilBroken);
    }

    // Custom rename validation (max 50 chars, trimmed, blank string reset)
    let mut rename_changed = false;
    let base_custom_name = input.base_item.custom_name();
    let mut final_custom_name = base_custom_name.map(str::to_owned);

    if let Some(ref raw_name) = input.custom_name {
        let char_count = raw_name.chars().count();
        if char_count > MAX_NAME_LENGTH {
            return Err(AnvilError::NameTooLong {
                len: char_count,
                max: MAX_NAME_LENGTH,
            });
        }
        let trimmed = raw_name.trim();
        if trimmed.is_empty() {
            // Blank string reset
            if base_custom_name.is_some() {
                final_custom_name = None;
                rename_changed = true;
            }
        } else if base_custom_name != Some(trimmed) {
            final_custom_name = Some(trimmed.to_string());
            rename_changed = true;
        }
    }

    let rename_level_cost = if rename_changed { RENAME_LEVEL_COST } else { 0 };

    // Evaluate repair and enchantment combination
    let mut result_durability = input.base_item.durability();
    let mut result_enchantments = input.base_item.enchantments().clone();
    let mut repair_cost_levels = 0;
    let mut material_cost = 0;
    let mut sacrifice_penalty = 0;

    match &input.sacrifice_item {
        None => {
            if !rename_changed {
                return Err(AnvilError::NoOperation);
            }
        }
        Some(sacrifice) => {
            let sacrifice_is_book = sacrifice.is_enchanted_book();
            let both_books = input.base_item.is_enchanted_book() && sacrifice_is_book;
            let book_onto_item = !input.base_item.is_enchanted_book() && sacrifice_is_book;
            let is_same_item = !input.base_item.is_enchanted_book()
                && !sacrifice_is_book
                && items_equal(input.base_item.item(), sacrifice.item());
            let is_mat = !input.base_item.is_enchanted_book()
                && !sacrifice_is_book
                && source.is_repair_material(input.base_item.item(), sacrifice.item());

            if !both_books && !book_onto_item && !is_same_item && !is_mat {
                return Err(AnvilError::IncompatibleSacrificeItem);
            }

            if both_books || book_onto_item {
                let Some(registry) = source.enchantments() else {
                    return Err(AnvilError::IncompatibleSacrificeItem);
                };
                if sacrifice.enchantments().is_empty() {
                    return Err(AnvilError::IncompatibleSacrificeItem);
                }

                let outcome = combine_enchantments(&input.base_item, sacrifice, registry);
                if !outcome.any_applied && !outcome.any_incompatible {
                    return Err(AnvilError::IncompatibleSacrificeItem);
                }

                result_enchantments = outcome.enchantments;
                repair_cost_levels = outcome.enchant_cost;
                material_cost = 1;
                sacrifice_penalty = sacrifice.repair_cost();
            } else if is_same_item {
                if !input.base_item.is_damageable() {
                    return Err(AnvilError::NotDamageable);
                }

                let max_durability = input.base_item.max_durability();
                let d1 = input.base_item.durability().min(max_durability);
                let d2 = sacrifice.durability().min(sacrifice.max_durability());
                let bonus = ((f64::from(max_durability)) * DURABILITY_BONUS_FACTOR).floor() as u32;
                let combined = d1
                    .saturating_add(d2)
                    .saturating_add(bonus)
                    .min(max_durability);

                let repaired = combined > d1;
                if repaired {
                    result_durability = combined;
                    repair_cost_levels = COMBINE_REPAIR_LEVEL_COST;
                }

                let mut any_enchant_change = false;
                if !sacrifice.enchantments().is_empty() {
                    let Some(registry) = source.enchantments() else {
                        return Err(AnvilError::IncompatibleSacrificeItem);
                    };
                    let outcome = combine_enchantments(&input.base_item, sacrifice, registry);
                    result_enchantments = outcome.enchantments;
                    repair_cost_levels = repair_cost_levels.saturating_add(outcome.enchant_cost);
                    any_enchant_change = outcome.any_applied || outcome.any_incompatible;
                }

                if !repaired && !any_enchant_change && !rename_changed {
                    return Err(AnvilError::NoRepairNeeded);
                }

                material_cost = 1;
                sacrifice_penalty = sacrifice.repair_cost();
            } else if is_mat {
                if !input.base_item.is_damageable() {
                    return Err(AnvilError::NotDamageable);
                }

                let max_durability = input.base_item.max_durability();
                let d1 = input.base_item.durability().min(max_durability);
                let missing = max_durability.saturating_sub(d1);

                if missing == 0 {
                    if !rename_changed {
                        return Err(AnvilError::NoRepairNeeded);
                    }
                } else {
                    let unit_repair =
                        ((f64::from(max_durability)) * MATERIAL_REPAIR_FRACTION).floor() as u32;
                    let unit_repair = unit_repair.max(1);

                    let mut mat_used = 0u32;
                    let mut cur_d = d1;
                    let sacrifice_count = u32::from(sacrifice.count());
                    while cur_d < max_durability
                        && mat_used < sacrifice_count
                        && mat_used < MAX_REPAIR_MATERIAL_UNITS
                    {
                        cur_d = cur_d.saturating_add(unit_repair).min(max_durability);
                        mat_used += 1;
                    }
                    // When max_durability % 4 != 0 (e.g. 59 or 1561), 4 * floor(max * 0.25) < max;
                    // this branch tops up the 4th material unit to full durability.
                    if mat_used == MAX_REPAIR_MATERIAL_UNITS {
                        cur_d = max_durability;
                    }

                    result_durability = cur_d;
                    material_cost = mat_used;
                    repair_cost_levels = mat_used; // 1 level per unit of material (1-4)
                }
            }
        }
    }

    // Prior work penalty calculation
    let base_penalty = input.base_item.repair_cost();
    let prior_penalty_cost = base_penalty.saturating_add(sacrifice_penalty);
    let total_level_cost = prior_penalty_cost
        .saturating_add(rename_level_cost)
        .saturating_add(repair_cost_levels);

    // Result prior work penalty doubles: 2 * penalty + 1
    let max_prior = base_penalty.max(sacrifice_penalty);
    let next_prior_work_penalty = max_prior.saturating_mul(2).saturating_add(1);

    // Maximum penalty cap / "Too Expensive!" threshold (40 levels in Survival, ignored in Creative)
    if !is_creative && total_level_cost >= SURVIVAL_LEVEL_CAP {
        return Err(AnvilError::TooExpensive {
            cost: total_level_cost,
            limit: SURVIVAL_LEVEL_CAP,
        });
    }

    let Ok(base_result_stack) = ItemStack::try_new(
        input.base_item.item(),
        1,
        input.base_item.stack().max_stack(),
    ) else {
        return Err(AnvilError::NoOperation);
    };
    let Ok(result_stack) =
        base_result_stack.with_components(input.base_item.stack().components().clone())
    else {
        return Err(AnvilError::NoOperation);
    };
    let result_max_durability = input.base_item.max_durability();
    validate_anvil_item_invariants(
        source.is_known_damageable(result_stack.item()),
        result_durability,
        result_max_durability,
        final_custom_name.as_deref(),
    )
    .map_err(|err| match err {
        AnvilItemError::NameTooLong { len, max } => AnvilError::NameTooLong { len, max },
        _ => AnvilError::NoOperation,
    })?;
    let is_enchanted_book = input.base_item.is_enchanted_book();
    let result_stack = apply_anvil_components(
        result_stack,
        result_durability,
        result_max_durability,
        next_prior_work_penalty,
        final_custom_name,
        result_enchantments,
        is_enchanted_book,
    )
    .map_err(|_| AnvilError::NoOperation)?;
    let result_item = AnvilItem {
        stack: result_stack,
        max_durability_override: max_durability_override_for(
            input.base_item.item(),
            result_max_durability,
        ),
        is_enchanted_book,
    };

    let degraded_state = roll.and_then(|r| anvil_state.roll_degradation(r));

    Ok(AnvilRepairResult {
        result_item,
        repair_cost_levels: total_level_cost,
        material_cost,
        next_prior_work_penalty,
        degraded_state,
    })
}

/// Calculates the anvil repair result in preview mode without degrading the anvil (`degraded_state = None`).
///
/// Offline fallback function using [`default_max_durability`] and [`is_repair_material`];
/// prefer [`AnvilRules::calculate_repair`] when item report data is loaded.
pub fn calculate_repair(
    input: &AnvilRepairInput,
    is_creative: bool,
) -> Result<AnvilRepairResult, AnvilError> {
    calculate_repair_internal(
        input,
        AnvilState::Undamaged,
        is_creative,
        None,
        RepairSource::Fallback,
    )
}

/// Executes an anvil repair with a specified degradation roll `[0.0, 1.0)`.
///
/// Offline fallback function using [`default_max_durability`] and [`is_repair_material`];
/// prefer [`AnvilRules::repair_with_roll`] when item report data is loaded.
pub fn repair_with_roll(
    input: &AnvilRepairInput,
    anvil_state: AnvilState,
    is_creative: bool,
    roll: f64,
) -> Result<AnvilRepairResult, AnvilError> {
    calculate_repair_internal(
        input,
        anvil_state,
        is_creative,
        Some(roll),
        RepairSource::Fallback,
    )
}

/// Executes an anvil repair on an `Undamaged` anvil using the caller's random source.
///
/// Offline fallback function using [`default_max_durability`] and [`is_repair_material`];
/// prefer [`AnvilRules::repair`] when item report data is loaded.
pub fn repair(
    input: &AnvilRepairInput,
    is_creative: bool,
    rng: &mut RandomSource,
) -> Result<AnvilRepairResult, AnvilError> {
    repair_with_state(input, AnvilState::Undamaged, is_creative, rng)
}

/// Executes an anvil repair with the given `anvil_state` using the caller's random source.
///
/// Offline fallback function using [`default_max_durability`] and [`is_repair_material`];
/// prefer [`AnvilRules::repair_with_state`] when item report data is loaded.
pub fn repair_with_state(
    input: &AnvilRepairInput,
    anvil_state: AnvilState,
    is_creative: bool,
    rng: &mut RandomSource,
) -> Result<AnvilRepairResult, AnvilError> {
    let roll = rng.next_double();
    calculate_repair_internal(
        input,
        anvil_state,
        is_creative,
        Some(roll),
        RepairSource::Fallback,
    )
}

/// Anvil block instance tracking its current state in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Anvil {
    pub state: AnvilState,
}

impl Anvil {
    /// Creates a new anvil with the specified state.
    pub const fn new(state: AnvilState) -> Self {
        Self { state }
    }

    /// Creates an undamaged anvil.
    pub const fn undamaged() -> Self {
        Self {
            state: AnvilState::Undamaged,
        }
    }

    /// Performs a repair using offline fallback rules, updating the anvil's state if degradation occurs.
    pub fn repair_with_roll(
        &mut self,
        input: &AnvilRepairInput,
        is_creative: bool,
        roll: f64,
    ) -> Result<AnvilRepairResult, AnvilError> {
        self.apply_repair(repair_with_roll(input, self.state, is_creative, roll))
    }

    /// Performs a repair using offline fallback rules and a random source, updating the anvil's state if degradation occurs.
    pub fn repair(
        &mut self,
        input: &AnvilRepairInput,
        is_creative: bool,
        rng: &mut RandomSource,
    ) -> Result<AnvilRepairResult, AnvilError> {
        self.apply_repair(repair_with_state(input, self.state, is_creative, rng))
    }

    /// Performs a repair using data-backed [`AnvilRules`] and a roll, updating the anvil's state if degradation occurs.
    pub fn repair_with_rules_and_roll(
        &mut self,
        rules: &AnvilRules,
        input: &AnvilRepairInput,
        is_creative: bool,
        roll: f64,
    ) -> Result<AnvilRepairResult, AnvilError> {
        self.apply_repair(rules.repair_with_roll(input, self.state, is_creative, roll))
    }

    /// Performs a repair using data-backed [`AnvilRules`] and a random source, updating the anvil's state if degradation occurs.
    pub fn repair_with_rules(
        &mut self,
        rules: &AnvilRules,
        input: &AnvilRepairInput,
        is_creative: bool,
        rng: &mut RandomSource,
    ) -> Result<AnvilRepairResult, AnvilError> {
        self.apply_repair(rules.repair_with_state(input, self.state, is_creative, rng))
    }

    fn apply_repair(
        &mut self,
        outcome: Result<AnvilRepairResult, AnvilError>,
    ) -> Result<AnvilRepairResult, AnvilError> {
        let result = outcome?;
        if let Some(new_state) = result.degraded_state {
            self.state = new_state;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, count: u16) -> AnvilItem {
        AnvilItem::try_new(id, count).unwrap()
    }

    #[test]
    fn test_same_item_combination_durability_bonus() {
        // Iron sword has max durability 250.
        // 12% bonus = floor(250 * 0.12) = 30.
        let sword1 = item("minecraft:iron_sword", 1).with_durability(50).unwrap();
        let sword2 = item("minecraft:iron_sword", 1).with_durability(50).unwrap();

        let input = AnvilRepairInput::combine(sword1, sword2);
        let result = calculate_repair(&input, false).unwrap();

        // 50 + 50 + 30 = 130
        assert_eq!(result.result_item.durability(), 130);
        assert_eq!(result.result_item.max_durability(), 250);
        assert_eq!(result.result_item.item(), "minecraft:iron_sword");
        assert_eq!(result.result_item.count(), 1);
        // Combining tools costs 2 levels
        assert_eq!(result.repair_cost_levels, 2);
        assert_eq!(result.material_cost, 1);
        // Prior work penalty: 0 -> next is 2*0 + 1 = 1
        assert_eq!(result.next_prior_work_penalty, 1);
        assert_eq!(result.result_item.repair_cost(), 1);
        assert!(result.degraded_state.is_none());
    }

    #[test]
    fn test_same_item_combination_durability_capping() {
        // Capping at max_durability
        let pick1 = item("minecraft:diamond_pickaxe", 1)
            .with_durability(1400)
            .unwrap();
        let pick2 = item("minecraft:diamond_pickaxe", 1)
            .with_durability(300)
            .unwrap();
        // Max durability = 1561. Bonus = floor(1561 * 0.12) = 187.
        // 1400 + 300 + 187 = 1887 > 1561 -> capped at 1561.
        let input = AnvilRepairInput::combine(pick1, pick2);
        let result = calculate_repair(&input, false).unwrap();

        assert_eq!(result.result_item.durability(), 1561);
        assert_eq!(result.repair_cost_levels, 2);
        assert_eq!(result.material_cost, 1);
    }

    #[test]
    fn test_material_repair_iron_chestplate() {
        // Iron chestplate max durability = 240.
        // 25% per ingot = floor(240 * 0.25) = 60.
        // Starting durability: 100 (missing 140).
        let chestplate = item("minecraft:iron_chestplate", 1)
            .with_durability(100)
            .unwrap();
        let ingots = item("minecraft:iron_ingot", 5);

        let input = AnvilRepairInput::combine(chestplate, ingots);
        let result = calculate_repair(&input, false).unwrap();

        // 1 ingot: 100 + 60 = 160
        // 2 ingots: 160 + 60 = 220
        // 3 ingots: 220 + 60 = 280 -> capped at 240.
        // Total ingots consumed = 3.
        assert_eq!(result.result_item.durability(), 240);
        assert_eq!(result.material_cost, 3);
        assert_eq!(result.repair_cost_levels, 3);
        assert_eq!(result.next_prior_work_penalty, 1);
    }

    #[test]
    fn test_material_repair_diamond_sword() {
        // Diamond sword max durability = 1561.
        // 25% per diamond = floor(1561 * 0.25) = 390.
        // Starting durability: 1 (missing 1560).
        let sword = item("minecraft:diamond_sword", 1)
            .with_durability(1)
            .unwrap();
        let diamonds = item("minecraft:diamond", 10);

        let input = AnvilRepairInput::combine(sword, diamonds);
        let result = calculate_repair(&input, false).unwrap();

        // 4 diamonds repair full durability: 4 * 390 = 1560, total = 1561.
        assert_eq!(result.result_item.durability(), 1561);
        assert_eq!(result.material_cost, 4);
        assert_eq!(result.repair_cost_levels, 4);
    }

    #[test]
    fn test_four_material_units_top_up_non_divisible_durability() {
        // Wooden sword has max durability 59; 25% = floor(59 * 0.25) = 14, so 4 * 14 = 56 < 59.
        // The `mat_used == 4` branch changes the outcome from 56 to 59.
        let wooden = item("minecraft:wooden_sword", 1)
            .with_durability(0)
            .unwrap();
        let planks = item("minecraft:oak_planks", 4);
        let res_wood = calculate_repair(&AnvilRepairInput::combine(wooden, planks), false).unwrap();
        assert_eq!(res_wood.material_cost, 4);
        assert_eq!(res_wood.result_item.durability(), 59);

        // Diamond sword from 0 durability: 4 * 390 = 1560 < 1561; `mat_used == 4` restores 1561.
        let diamond = item("minecraft:diamond_sword", 1)
            .with_durability(0)
            .unwrap();
        let diamonds = item("minecraft:diamond", 4);
        let res_diamond =
            calculate_repair(&AnvilRepairInput::combine(diamond, diamonds), false).unwrap();
        assert_eq!(res_diamond.material_cost, 4);
        assert_eq!(res_diamond.result_item.durability(), 1561);
    }

    #[test]
    fn test_mace_durability_and_breeze_rod_repair() {
        // Java Edition mace has 500 durability and repairs with breeze_rod (125 per rod).
        assert_eq!(default_max_durability("minecraft:mace"), Some(500));
        let mace = item("minecraft:mace", 1).with_durability(100).unwrap();
        let rods = item("minecraft:breeze_rod", 3);

        let result = calculate_repair(&AnvilRepairInput::combine(mace, rods), false).unwrap();
        // 100 + 3 * 125 = 475
        assert_eq!(result.result_item.durability(), 475);
        assert_eq!(result.result_item.max_durability(), 500);
        assert_eq!(result.material_cost, 3);
        assert_eq!(result.repair_cost_levels, 3);
    }

    #[test]
    fn test_material_repair_insufficient_materials() {
        // Iron chestplate missing 140 durability, but player only has 2 ingots.
        let chestplate = item("minecraft:iron_chestplate", 1)
            .with_durability(100)
            .unwrap();
        let ingots = item("minecraft:iron_ingot", 2);

        let input = AnvilRepairInput::combine(chestplate, ingots);
        let result = calculate_repair(&input, false).unwrap();

        // Consumes 2 ingots, restores 120 durability -> 220
        assert_eq!(result.result_item.durability(), 220);
        assert_eq!(result.material_cost, 2);
        assert_eq!(result.repair_cost_levels, 2);
    }

    #[test]
    fn test_rename_handling_and_validation() {
        let sword = item("minecraft:diamond_sword", 1);

        // Rename costs 1 level
        let input = AnvilRepairInput::rename_only(sword.clone(), "Excalibur").unwrap();
        let result = calculate_repair(&input, false).unwrap();
        assert_eq!(result.result_item.custom_name(), Some("Excalibur"));
        assert_eq!(result.repair_cost_levels, 1);
        assert_eq!(result.material_cost, 0);
        assert_eq!(result.next_prior_work_penalty, 1);

        // Trimmed whitespace
        let input_trimmed =
            AnvilRepairInput::rename_only(sword.clone(), "   Doom Blade   ").unwrap();
        let res_trimmed = calculate_repair(&input_trimmed, false).unwrap();
        assert_eq!(res_trimmed.result_item.custom_name(), Some("Doom Blade"));

        // Name longer than 50 characters is rejected
        let long_name = "a".repeat(51);
        assert!(matches!(
            AnvilRepairInput::rename_only(sword.clone(), long_name),
            Err(AnvilError::NameTooLong { len: 51, max: 50 })
        ));

        // Name exactly 50 characters is accepted
        let max_name = "a".repeat(50);
        assert!(AnvilRepairInput::rename_only(sword.clone(), max_name).is_ok());

        // Blank string reset on an item that has an existing custom name
        let named_sword = sword.with_custom_name("Old Name").unwrap();
        let reset_input = AnvilRepairInput::new(named_sword, None, Some("   ".into())).unwrap();
        let reset_res = calculate_repair(&reset_input, false).unwrap();
        assert_eq!(reset_res.result_item.custom_name(), None);
        assert_eq!(reset_res.repair_cost_levels, 1); // un-renaming costs 1 level

        // Unchanged name does not cost levels
        let sword_same = item("minecraft:iron_sword", 1)
            .with_custom_name("Sword")
            .unwrap();
        let same_input =
            AnvilRepairInput::new(sword_same.clone(), None, Some("Sword".into())).unwrap();
        assert_eq!(
            calculate_repair(&same_input, false),
            Err(AnvilError::NoOperation)
        );
    }

    #[test]
    fn test_prior_work_penalty_escalation() {
        // Penalty doubles: 2*penalty + 1
        // Sequence: 0 -> 1 -> 3 -> 7 -> 15 -> 31 -> 63
        let mut penalty = 0;
        let expected = [1, 3, 7, 15, 31, 63];
        for exp in expected {
            let sword1 = item("minecraft:iron_sword", 1)
                .with_durability(100)
                .unwrap()
                .with_repair_cost(penalty);
            let sword2 = item("minecraft:iron_sword", 1)
                .with_durability(100)
                .unwrap()
                .with_repair_cost(penalty);

            let input = AnvilRepairInput::combine(sword1, sword2);
            let result = calculate_repair(&input, true).unwrap();
            assert_eq!(result.next_prior_work_penalty, exp);
            penalty = exp;
        }
    }

    #[test]
    fn test_too_expensive_survival_vs_creative() {
        // Base penalty 15 + sacrifice penalty 31 = 46.
        // Combined with repair cost (2) = 48 >= 40 limit.
        let sword1 = item("minecraft:iron_sword", 1)
            .with_durability(100)
            .unwrap()
            .with_repair_cost(15);
        let sword2 = item("minecraft:iron_sword", 1)
            .with_durability(100)
            .unwrap()
            .with_repair_cost(31);

        let input = AnvilRepairInput::combine(sword1, sword2);

        // In survival (is_creative = false), rejected as Too Expensive!
        let survival_err = calculate_repair(&input, false).unwrap_err();
        assert!(matches!(
            survival_err,
            AnvilError::TooExpensive {
                cost: 48,
                limit: 40
            }
        ));

        // In creative (is_creative = true), permitted!
        let creative_res = calculate_repair(&input, true).unwrap();
        assert_eq!(creative_res.repair_cost_levels, 48);
        assert_eq!(creative_res.next_prior_work_penalty, 63); // 31 * 2 + 1 = 63
    }

    #[test]
    fn test_degradation_state_transitions_deterministic() {
        let sword1 = item("minecraft:iron_sword", 1).with_durability(50).unwrap();
        let sword2 = item("minecraft:iron_sword", 1).with_durability(50).unwrap();
        let input = AnvilRepairInput::combine(sword1, sword2);

        // Roll < 0.12 triggers degradation
        let res1 = repair_with_roll(&input, AnvilState::Undamaged, false, 0.05).unwrap();
        assert_eq!(res1.degraded_state, Some(AnvilState::Chipped));

        let res2 = repair_with_roll(&input, AnvilState::Chipped, false, 0.119).unwrap();
        assert_eq!(res2.degraded_state, Some(AnvilState::Damaged));

        let res3 = repair_with_roll(&input, AnvilState::Damaged, false, 0.0).unwrap();
        assert_eq!(res3.degraded_state, Some(AnvilState::Broken));

        // Roll >= 0.12 does NOT degrade
        let res_no_deg = repair_with_roll(&input, AnvilState::Undamaged, false, 0.12).unwrap();
        assert_eq!(res_no_deg.degraded_state, None);

        let res_no_deg2 = repair_with_roll(&input, AnvilState::Chipped, false, 0.50).unwrap();
        assert_eq!(res_no_deg2.degraded_state, None);

        // Broken anvil cannot be used
        assert_eq!(
            repair_with_roll(&input, AnvilState::Broken, false, 0.0),
            Err(AnvilError::AnvilBroken)
        );
    }

    #[test]
    fn test_repair_and_repair_with_state_use_caller_rng_deterministically() {
        let sword1 = item("minecraft:iron_sword", 1).with_durability(50).unwrap();
        let sword2 = item("minecraft:iron_sword", 1).with_durability(50).unwrap();
        let input = AnvilRepairInput::combine(sword1, sword2);

        // Pin deterministic outcomes for fixed world seeds (18 degrades, 0 stays intact).
        let mut rng_a = RandomSource::from_world_seed(18);
        let res_degraded = repair(&input, false, &mut rng_a).unwrap();
        assert_eq!(res_degraded.degraded_state, Some(AnvilState::Chipped));

        let mut rng_b = RandomSource::from_world_seed(0);
        let res_intact = repair_with_state(&input, AnvilState::Chipped, false, &mut rng_b).unwrap();
        assert_eq!(res_intact.degraded_state, None);
    }

    #[test]
    fn test_anvil_struct_state_mutation() {
        let mut anvil = Anvil::undamaged();
        let sword1 = item("minecraft:iron_sword", 1).with_durability(50).unwrap();
        let sword2 = item("minecraft:iron_sword", 1).with_durability(50).unwrap();
        let input = AnvilRepairInput::combine(sword1, sword2);

        // Step 1: degrades to Chipped
        anvil.repair_with_roll(&input, false, 0.01).unwrap();
        assert_eq!(anvil.state, AnvilState::Chipped);

        // Step 2: does not degrade
        anvil.repair_with_roll(&input, false, 0.50).unwrap();
        assert_eq!(anvil.state, AnvilState::Chipped);

        // Step 3: degrades to Damaged
        anvil.repair_with_roll(&input, false, 0.05).unwrap();
        assert_eq!(anvil.state, AnvilState::Damaged);

        // Step 4: degrades to Broken
        anvil.repair_with_roll(&input, false, 0.02).unwrap();
        assert_eq!(anvil.state, AnvilState::Broken);

        // Step 5: broken anvil fails
        assert_eq!(
            anvil.repair_with_roll(&input, false, 0.0),
            Err(AnvilError::AnvilBroken)
        );
    }

    #[test]
    fn test_degradation_probability_distribution() {
        // Run 10,000 trials with deterministic RandomSource and measure degradation frequency.
        // Target: 12% probability (1200 expected). Allowed tolerance: +/- 1.5% (10.5% - 13.5%).
        let mut rng = RandomSource::from_world_seed(2026);
        let trials = 10_000;
        let mut degraded_count = 0;

        for _ in 0..trials {
            let roll = rng.next_double();
            if AnvilState::Undamaged.roll_degradation(roll).is_some() {
                degraded_count += 1;
            }
        }

        let rate = (degraded_count as f64) / (trials as f64);
        assert!(
            (0.105..=0.135).contains(&rate),
            "degradation rate {rate} was outside expected [0.105, 0.135]"
        );
    }

    #[test]
    fn test_incompatible_items_and_error_cases() {
        // Diamond sword + Iron ingot is incompatible
        let sword = item("minecraft:diamond_sword", 1)
            .with_durability(100)
            .unwrap();
        let ingot = item("minecraft:iron_ingot", 1);
        let input = AnvilRepairInput::combine(sword, ingot);
        assert_eq!(
            calculate_repair(&input, false),
            Err(AnvilError::IncompatibleSacrificeItem)
        );

        // Diamond sword + Iron sword is incompatible
        let iron_sword = item("minecraft:iron_sword", 1);
        let sword = item("minecraft:diamond_sword", 1)
            .with_durability(100)
            .unwrap();
        let input2 = AnvilRepairInput::combine(sword, iron_sword);
        assert_eq!(
            calculate_repair(&input2, false),
            Err(AnvilError::IncompatibleSacrificeItem)
        );

        // Shared ItemStack rejects invalid item ids and zero counts
        assert_eq!(AnvilItem::try_new("", 1), Err(ItemStackError::InvalidItem));
        assert_eq!(
            AnvilItem::try_new("minecraft:iron_sword", 0),
            Err(ItemStackError::InvalidCount(0))
        );

        // Fully repaired item with no rename
        let full_sword1 = item("minecraft:iron_sword", 1);
        let full_sword2 = item("minecraft:iron_sword", 1);
        let input_full = AnvilRepairInput::combine(full_sword1, full_sword2);
        assert_eq!(
            calculate_repair(&input_full, false),
            Err(AnvilError::NoRepairNeeded)
        );
    }

    #[test]
    fn anvil_item_enforces_invariants_through_validating_constructor() {
        let sword_stack = ItemStack::try_new("minecraft:iron_sword", 1, 1).unwrap();

        assert_eq!(
            AnvilItem::new(sword_stack.clone(), 0, 0, 0, None),
            Err(AnvilItemError::ZeroMaxDurability)
        );
        assert_eq!(
            AnvilItem::new(sword_stack.clone(), 251, 250, 0, None),
            Err(AnvilItemError::DurabilityExceedsMax {
                durability: 251,
                max_durability: 250,
            })
        );
        assert_eq!(
            AnvilItem::new(sword_stack.clone(), 100, 250, 0, Some("a".repeat(51))),
            Err(AnvilItemError::NameTooLong {
                len: 51,
                max: MAX_NAME_LENGTH,
            })
        );

        let valid = AnvilItem::new(sword_stack, 100, 250, 3, Some("a".repeat(50))).unwrap();
        assert_eq!(valid.durability(), 100);
        assert_eq!(valid.max_durability(), 250);
        assert_eq!(valid.repair_cost(), 3);
        assert_eq!(valid.custom_name().map(str::len), Some(50));

        assert_eq!(
            valid.clone().with_max_durability(0),
            Err(AnvilItemError::ZeroMaxDurability)
        );
        assert_eq!(
            valid.clone().with_durability(251),
            Err(AnvilItemError::DurabilityExceedsMax {
                durability: 251,
                max_durability: 250,
            })
        );
        assert_eq!(
            valid.with_custom_name("b".repeat(51)),
            Err(AnvilItemError::NameTooLong {
                len: 51,
                max: MAX_NAME_LENGTH,
            })
        );
    }

    fn scratch_root(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rustmc-anvil-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch dir");
        path
    }

    fn write_scratch_file(path: &std::path::Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        std::fs::write(path, text).expect("write");
    }

    #[test]
    fn anvil_rules_uses_report_durability_and_repair_materials_instead_of_fallback() {
        let root = scratch_root("rules");
        write_scratch_file(
            &root.join("data/testns/tags/item/ingots.json"),
            r#"{"values": ["testns:ingot_a"]}"#,
        );
        // Intentionally diverge from offline fallback (42 vs 1561, testns:gem vs diamond)
        // to prove AnvilRules reads from the report rather than default tables.
        write_scratch_file(
            &root.join("reports/minecraft/components/item/diamond_sword.json"),
            r#"{"components": {
                "minecraft:max_stack_size": 1,
                "minecraft:max_damage": 42,
                "minecraft:repairable": {"items": "testns:gem"}
            }}"#,
        );
        write_scratch_file(
            &root.join("reports/minecraft/components/item/copper_blade.json"),
            r##"{"components": {
                "minecraft:max_stack_size": 1,
                "minecraft:max_damage": 100,
                "minecraft:repairable": {"items": "#testns:ingots"}
            }}"##,
        );
        write_scratch_file(
            &root.join("reports/minecraft/components/item/diamond_horse_armor.json"),
            r#"{"components": {"minecraft:max_stack_size": 1}}"#,
        );

        let tags = ItemTags::load(&root).expect("load synthetic item tags");
        let defaults = ItemDefaults::load(&root).expect("load synthetic item defaults");
        let rules = AnvilRules::new(&defaults, &tags);

        assert_eq!(rules.max_durability("minecraft:diamond_sword"), Some(42));
        assert_eq!(rules.max_durability("minecraft:iron_sword"), None);
        assert!(!rules.is_repair_material("minecraft:diamond_sword", "minecraft:diamond"));
        assert!(rules.is_repair_material("minecraft:diamond_sword", "testns:gem"));

        let sword = rules
            .try_new_item("minecraft:diamond_sword", 1)
            .expect("construct report sword")
            .with_durability(10)
            .expect("damage sword");
        assert_eq!(sword.max_durability(), 42);
        let gem = rules.try_new_item("testns:gem", 1).expect("gem");
        let sword_rep = rules
            .calculate_repair(&AnvilRepairInput::combine(sword, gem), false)
            .expect("repair with report material");
        // floor(42 * 0.25) = 10 -> 10 + 10 = 20
        assert_eq!(sword_rep.result_item.durability(), 20);
        assert_eq!(sword_rep.material_cost, 1);

        let blade = rules
            .try_new_item("minecraft:copper_blade", 1)
            .expect("construct copper_blade")
            .with_durability(50)
            .expect("damage blade");
        let ingot = rules.try_new_item("testns:ingot_a", 1).expect("ingot");
        let blade_rep = rules
            .calculate_repair(&AnvilRepairInput::combine(blade, ingot), false)
            .expect("repair with tagged material");
        // floor(100 * 0.25) = 25 -> 50 + 25 = 75
        assert_eq!(blade_rep.result_item.durability(), 75);
        assert_eq!(blade_rep.material_cost, 1);

        let horse_armor = rules
            .try_new_item("minecraft:diamond_horse_armor", 1)
            .expect("horse armor");
        let diamond = AnvilItem::try_new("minecraft:diamond", 1).expect("diamond");
        assert_eq!(
            rules.calculate_repair(&AnvilRepairInput::combine(horse_armor, diamond), false),
            Err(AnvilError::IncompatibleSacrificeItem)
        );

        let mut anvil = Anvil::undamaged();
        let blade2 = rules
            .try_new_item("minecraft:copper_blade", 1)
            .expect("blade2")
            .with_durability(50)
            .expect("damage");
        let ingot2 = rules.try_new_item("testns:ingot_a", 1).expect("ingot2");
        let mut rng = RandomSource::from_world_seed(18);
        let res = anvil
            .repair_with_rules(
                &rules,
                &AnvilRepairInput::combine(blade2, ingot2),
                false,
                &mut rng,
            )
            .expect("repair through Anvil struct");
        assert_eq!(res.result_item.durability(), 75);
        assert_eq!(anvil.state, AnvilState::Chipped);

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_anvil_rules_diamond_sword_mace_and_all_max_damage_items`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_anvil_rules_diamond_sword_mace_and_all_max_damage_items() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(std::path::PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let tags = ItemTags::load(&root).expect("load vanilla item tags");
        let defaults = ItemDefaults::load(&root).expect("load vanilla item defaults");
        let rules = AnvilRules::new(&defaults, &tags);

        assert_eq!(rules.max_durability("minecraft:diamond_sword"), Some(1561));
        assert!(rules.is_repair_material("minecraft:diamond_sword", "minecraft:diamond"));
        assert!(!rules.is_repair_material("minecraft:diamond_sword", "minecraft:iron_ingot"));

        assert_eq!(rules.max_durability("minecraft:mace"), Some(500));
        assert!(rules.is_repair_material("minecraft:mace", "minecraft:breeze_rod"));
        assert!(!rules.is_repair_material("minecraft:mace", "minecraft:diamond"));

        let items_dir = root.join("reports/minecraft/components/item");
        let report_files = crate::vanilla::worldgen::walk_json(&items_dir).expect("walk reports");
        let mut checked_max_damage = 0usize;
        for file in report_files {
            let stem = file
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("valid item stem");
            let doc = crate::vanilla::worldgen::read_json(&file).expect("read item report");
            let components = doc["components"].as_object().expect("components object");
            let full_id = format!("minecraft:{stem}");
            if let Some(expected_damage) = components
                .get("minecraft:max_damage")
                .and_then(serde_json::Value::as_u64)
            {
                let expected = u32::try_from(expected_damage).expect("u32 max_damage");
                assert_eq!(rules.max_durability(&full_id), Some(expected), "{full_id}");
                let built = rules.try_new_item(&full_id, 1).expect("build anvil item");
                assert_eq!(built.max_durability(), expected, "{full_id}");
                checked_max_damage += 1;
            } else {
                assert_eq!(rules.max_durability(&full_id), None, "{full_id}");
            }
        }
        assert_eq!(checked_max_damage, 84);
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_anvil_rules_full_material_repair_diamond_pickaxe`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_anvil_rules_full_material_repair_diamond_pickaxe() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(std::path::PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let tags = ItemTags::load(&root).expect("load vanilla item tags");
        let defaults = ItemDefaults::load(&root).expect("load vanilla item defaults");
        let rules = AnvilRules::new(&defaults, &tags);

        // Diamond pickaxe max durability = 1561; 25% per diamond = floor(1561 * 0.25) = 390.
        // Starting at 1171 (missing exactly 390), 1 diamond restores full durability (1561).
        let pickaxe = rules
            .try_new_item("minecraft:diamond_pickaxe", 1)
            .expect("diamond_pickaxe")
            .with_durability(1171)
            .expect("damaged pickaxe");
        let diamond = rules.try_new_item("minecraft:diamond", 1).expect("diamond");

        let result = rules
            .calculate_repair(&AnvilRepairInput::combine(pickaxe, diamond), false)
            .expect("full repair with 1 diamond");
        assert_eq!(result.result_item.durability(), 1561);
        assert_eq!(result.result_item.max_durability(), 1561);
        assert_eq!(result.material_cost, 1);
        assert_eq!(result.repair_cost_levels, 1);
        assert_eq!(result.next_prior_work_penalty, 1);
    }

    fn synthetic_enchantment_registry() -> EnchantmentRegistry {
        use crate::world::enchantment::{EnchantmentCost, EnchantmentDefinition, EnchantmentSpec};

        let mut reg = EnchantmentRegistry::default();
        let cost = EnchantmentCost::new(1, 10);
        let reg_def =
            |id: &str, max_level: u8, anvil_cost: u32, supported: &[&str], exclusive: &[&str]| {
                EnchantmentDefinition::try_new(EnchantmentSpec {
                    id: id.to_owned(),
                    max_level,
                    anvil_cost,
                    weight: 10,
                    min_cost: cost,
                    max_cost: cost,
                    slots: vec!["mainhand".to_owned()],
                    supported_items: supported.iter().map(|s| (*s).to_owned()).collect(),
                    primary_items: None,
                    exclusive_set: exclusive.iter().map(|s| (*s).to_owned()).collect(),
                })
                .expect("valid synthetic enchantment")
            };

        reg.register(reg_def(
            "minecraft:sharpness",
            5,
            1,
            &["minecraft:diamond_sword", "minecraft:diamond_axe"],
            &["minecraft:sharpness", "minecraft:smite"],
        ));
        reg.register(reg_def(
            "minecraft:smite",
            5,
            2,
            &["minecraft:diamond_sword", "minecraft:diamond_axe"],
            &["minecraft:sharpness", "minecraft:smite"],
        ));
        reg.register(reg_def(
            "minecraft:looting",
            3,
            4,
            &["minecraft:diamond_sword"],
            &[],
        ));
        reg.register(reg_def(
            "minecraft:unbreaking",
            3,
            2,
            &["minecraft:diamond_sword", "minecraft:bow"],
            &[],
        ));
        reg.register(reg_def(
            "minecraft:protection",
            4,
            1,
            &["minecraft:diamond_helmet"],
            &[],
        ));
        reg.register(reg_def(
            "minecraft:depth_strider",
            3,
            4,
            &["minecraft:diamond_boots"],
            &[],
        ));
        reg
    }

    #[test]
    fn existing_anvil_item_constructors_preserve_empty_enchantments_and_book_flag() {
        let sword = AnvilItem::try_new("minecraft:diamond_sword", 1).expect("sword");
        assert!(sword.enchantments().is_empty());
        assert!(!sword.is_enchanted_book());

        let stack = ItemStack::try_new("minecraft:diamond_sword", 1, 1).expect("stack");
        let built = AnvilItem::new(stack, 100, 1561, 1, None).expect("new");
        assert!(built.enchantments().is_empty());
        assert!(!built.is_enchanted_book());

        let book = AnvilItem::try_new("minecraft:enchanted_book", 1).expect("book");
        assert!(book.enchantments().is_empty());
        assert!(book.is_enchanted_book());

        let custom_book = AnvilItem::try_new("testns:tome", 1)
            .expect("tome")
            .with_enchanted_book(true)
            .with_enchantment("minecraft:sharpness", 4);
        assert!(custom_book.is_enchanted_book());
        assert_eq!(
            custom_book.enchantments().get("minecraft:sharpness"),
            Some(&4)
        );
    }

    #[test]
    fn anvil_combines_sword_sharpness_iv_with_book_sharpness_iv_at_exact_wiki_cost() {
        let rules = AnvilRules::from_enchantments(&synthetic_enchantment_registry());
        let sword = item("minecraft:diamond_sword", 1).with_enchantment("minecraft:sharpness", 4);
        let book = item("minecraft:enchanted_book", 1).with_enchantment("minecraft:sharpness", 4);

        let result = rules
            .calculate_repair(&AnvilRepairInput::combine(sword, book), false)
            .expect("combine Sharpness IV + IV");

        assert_eq!(
            result.result_item.enchantments().get("minecraft:sharpness"),
            Some(&5)
        );
        assert_eq!(result.result_item.durability(), 1561);
        assert_eq!(result.material_cost, 1);
        // Sharpness V (5) * book multiplier max(1, 1/2) = 1 -> 5 levels
        assert_eq!(result.repair_cost_levels, 5);
        assert_eq!(result.next_prior_work_penalty, 1);
    }

    #[test]
    fn anvil_drops_incompatible_smite_from_book_and_adds_java_penalty() {
        let rules = AnvilRules::from_enchantments(&synthetic_enchantment_registry());
        let sword = item("minecraft:diamond_sword", 1).with_enchantment("minecraft:sharpness", 5);
        let book = item("minecraft:enchanted_book", 1).with_enchantment("minecraft:smite", 1);

        let result = rules
            .calculate_repair(&AnvilRepairInput::combine(sword, book), false)
            .expect("Sharpness V sword + Smite I book");

        assert_eq!(
            result.result_item.enchantments().get("minecraft:sharpness"),
            Some(&5)
        );
        assert_eq!(
            result.result_item.enchantments().get("minecraft:smite"),
            None
        );
        assert_eq!(result.repair_cost_levels, 1);
        assert_eq!(result.material_cost, 1);
    }

    #[test]
    fn anvil_merges_enchanted_books_and_respects_item_vs_book_multipliers() {
        let rules = AnvilRules::from_enchantments(&synthetic_enchantment_registry());

        // Wiki example: Depth Strider II book + Depth Strider II book -> Depth Strider III, cost 6
        let ds_book_1 =
            item("minecraft:enchanted_book", 1).with_enchantment("minecraft:depth_strider", 2);
        let ds_book_2 =
            item("minecraft:enchanted_book", 1).with_enchantment("minecraft:depth_strider", 2);
        let ds_res = rules
            .calculate_repair(&AnvilRepairInput::combine(ds_book_1, ds_book_2), false)
            .expect("book + book Depth Strider");
        assert!(ds_res.result_item.is_enchanted_book());
        assert_eq!(
            ds_res
                .result_item
                .enchantments()
                .get("minecraft:depth_strider"),
            Some(&3)
        );
        // anvil_cost 4 -> book mult 2 * level 3 = 6
        assert_eq!(ds_res.repair_cost_levels, 6);
        assert_eq!(ds_res.next_prior_work_penalty, 1);

        // Book + book merging distinct enchantments + prior work penalties
        let book_a = item("minecraft:enchanted_book", 1)
            .with_enchantment("minecraft:sharpness", 4)
            .with_repair_cost(1);
        let book_b = item("minecraft:enchanted_book", 1)
            .with_enchantment("minecraft:unbreaking", 3)
            .with_repair_cost(1);
        let merged_book = rules
            .calculate_repair(&AnvilRepairInput::combine(book_a, book_b), false)
            .expect("book + book distinct");
        assert_eq!(merged_book.result_item.enchantments().len(), 2);
        // Unbreaking III (3 * max(1, 2/2)=1 = 3) + penalties (1 + 1) = 5
        assert_eq!(merged_book.repair_cost_levels, 5);
        assert_eq!(merged_book.next_prior_work_penalty, 3);

        // Same-item vs book multiplier comparison for Looting II -> Looting III (anvil_cost = 4)
        let sword_a = item("minecraft:diamond_sword", 1).with_enchantment("minecraft:looting", 2);
        let sword_b = item("minecraft:diamond_sword", 1).with_enchantment("minecraft:looting", 2);
        let item_combine = rules
            .calculate_repair(&AnvilRepairInput::combine(sword_a.clone(), sword_b), false)
            .expect("item + item Looting");
        // Item multiplier 4 * level 3 = 12
        assert_eq!(item_combine.repair_cost_levels, 12);

        let book_looting =
            item("minecraft:enchanted_book", 1).with_enchantment("minecraft:looting", 2);
        let book_combine = rules
            .calculate_repair(&AnvilRepairInput::combine(sword_a, book_looting), false)
            .expect("item + book Looting");
        // Book multiplier 2 * level 3 = 6
        assert_eq!(book_combine.repair_cost_levels, 6);
    }

    #[test]
    fn anvil_handles_unequal_levels_max_level_cap_and_wiki_mixed_book() {
        let rules = AnvilRules::from_enchantments(&synthetic_enchantment_registry());

        // Wiki example: sword (Looting II) + book (Protection III, Sharpness I, Looting II) -> 7 levels
        // Protection III cannot apply to sword and is ignored.
        let sword = item("minecraft:diamond_sword", 1).with_enchantment("minecraft:looting", 2);
        let mixed_book = item("minecraft:enchanted_book", 1)
            .with_enchantment("minecraft:protection", 3)
            .with_enchantment("minecraft:sharpness", 1)
            .with_enchantment("minecraft:looting", 2);
        let res = rules
            .calculate_repair(&AnvilRepairInput::combine(sword, mixed_book), false)
            .expect("mixed book");
        assert_eq!(
            res.result_item.enchantments().get("minecraft:protection"),
            None
        );
        assert_eq!(
            res.result_item.enchantments().get("minecraft:sharpness"),
            Some(&1)
        );
        assert_eq!(
            res.result_item.enchantments().get("minecraft:looting"),
            Some(&3)
        );
        assert_eq!(res.repair_cost_levels, 7);

        // Unequal levels keep the higher, and Java charges final level * multiplier;
        // Equal levels at max_level stay capped at max_level.
        let target = item("minecraft:diamond_sword", 1)
            .with_enchantment("minecraft:sharpness", 5)
            .with_enchantment("minecraft:looting", 1);
        let sacrifice = item("minecraft:diamond_sword", 1)
            .with_enchantment("minecraft:sharpness", 5)
            .with_enchantment("minecraft:looting", 3);
        let unequal = rules
            .calculate_repair(&AnvilRepairInput::combine(target, sacrifice), false)
            .expect("unequal and capped");
        assert_eq!(
            unequal
                .result_item
                .enchantments()
                .get("minecraft:sharpness"),
            Some(&5)
        );
        assert_eq!(
            unequal.result_item.enchantments().get("minecraft:looting"),
            Some(&3)
        );
        // Sharpness V (5 * 1) + Looting III (3 * 4) = 17
        assert_eq!(unequal.repair_cost_levels, 17);
    }

    #[test]
    fn anvil_rejects_item_that_cannot_take_enchantment_and_book_on_left_of_tool() {
        let rules = AnvilRules::from_enchantments(&synthetic_enchantment_registry());

        // Bow cannot take Sharpness
        let bow = item("minecraft:bow", 1);
        let sharp_book =
            item("minecraft:enchanted_book", 1).with_enchantment("minecraft:sharpness", 1);
        assert_eq!(
            rules.calculate_repair(&AnvilRepairInput::combine(bow, sharp_book), false),
            Err(AnvilError::IncompatibleSacrificeItem)
        );

        // Enchanted book in left slot with sword in right slot is invalid
        let book_left =
            item("minecraft:enchanted_book", 1).with_enchantment("minecraft:sharpness", 1);
        let sword_right = item("minecraft:diamond_sword", 1);
        assert_eq!(
            rules.calculate_repair(&AnvilRepairInput::combine(book_left, sword_right), false),
            Err(AnvilError::IncompatibleSacrificeItem)
        );
    }

    #[test]
    fn anvil_combines_repair_rename_penalties_and_rejects_over_39_in_survival() {
        let rules = AnvilRules::from_enchantments(&synthetic_enchantment_registry());

        // Damaged sword1 (1000/1561, Sharpness IV, penalty 3)
        // + damaged sword2 (500/1561, Sharpness IV, penalty 1) + rename "Blade"
        // Durability repairs (2) + Sharpness V from item (5*1 = 5) + rename (1) + penalties (3 + 1) = 12
        let sword1 = item("minecraft:diamond_sword", 1)
            .with_durability(1000)
            .expect("dur1")
            .with_enchantment("minecraft:sharpness", 4)
            .with_repair_cost(3);
        let sword2 = item("minecraft:diamond_sword", 1)
            .with_durability(500)
            .expect("dur2")
            .with_enchantment("minecraft:sharpness", 4)
            .with_repair_cost(1);
        let input =
            AnvilRepairInput::new(sword1, Some(sword2), Some("Blade".to_owned())).expect("input");
        let combined = rules
            .calculate_repair(&input, false)
            .expect("repair + enchant + rename");
        assert_eq!(combined.result_item.durability(), 1561);
        assert_eq!(combined.result_item.custom_name(), Some("Blade"));
        assert_eq!(
            combined
                .result_item
                .enchantments()
                .get("minecraft:sharpness"),
            Some(&5)
        );
        assert_eq!(combined.repair_cost_levels, 12);
        assert_eq!(combined.next_prior_work_penalty, 7);

        // Survival cap boundary: 39 succeeds, 40 returns TooExpensive, Creative permits 40.
        // Sharpness V (5) + Looting III from book (3*2=6) + Unbreaking III from book (3*1=3) = 14 enchant cost
        // Base penalty 15 + book penalty 10 = 25 -> total 39 (succeeds);
        // With rename (+1) -> total 40 (TooExpensive in survival, succeeds in creative).
        let sword_39 = item("minecraft:diamond_sword", 1).with_repair_cost(15);
        let book_39 = item("minecraft:enchanted_book", 1)
            .with_enchantment("minecraft:sharpness", 5)
            .with_enchantment("minecraft:looting", 3)
            .with_enchantment("minecraft:unbreaking", 3)
            .with_repair_cost(10);
        let ok_39 = rules
            .calculate_repair(
                &AnvilRepairInput::combine(sword_39.clone(), book_39.clone()),
                false,
            )
            .expect("39 levels allowed in survival");
        assert_eq!(ok_39.repair_cost_levels, 39);

        let too_high = AnvilRepairInput::new(sword_39, Some(book_39), Some("OverCap".to_owned()))
            .expect("overcap input");
        assert_eq!(
            rules.calculate_repair(&too_high, false),
            Err(AnvilError::TooExpensive {
                cost: 40,
                limit: SURVIVAL_LEVEL_CAP,
            })
        );
        let creative_ok = rules
            .calculate_repair(&too_high, true)
            .expect("40 levels allowed in creative");
        assert_eq!(creative_ok.repair_cost_levels, 40);
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_anvil_enchant_combine_vanilla_scenarios`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_anvil_enchant_combine_vanilla_scenarios() {
        use crate::vanilla::feature::EnchantmentTags;

        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(std::path::PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let item_tags = ItemTags::load(&root).expect("load vanilla item tags");
        let ench_tags = EnchantmentTags::load(&root).expect("load vanilla enchantment tags");
        let defaults = ItemDefaults::load(&root).expect("load vanilla item defaults");
        let enchantments = EnchantmentRegistry::load(&root, &item_tags, &ench_tags)
            .expect("load vanilla enchantments");
        let rules = AnvilRules::with_enchantments(&defaults, &item_tags, &enchantments);

        // 1. Sword Sharpness IV + Book Sharpness IV -> Sharpness V, cost 5
        let sword_iv = rules
            .try_new_item("minecraft:diamond_sword", 1)
            .expect("diamond_sword")
            .with_enchantment("minecraft:sharpness", 4);
        let book_iv = rules
            .try_new_item("minecraft:enchanted_book", 1)
            .expect("enchanted_book")
            .with_enchantment("minecraft:sharpness", 4);
        let sharp_v = rules
            .calculate_repair(&AnvilRepairInput::combine(sword_iv, book_iv), false)
            .expect("Sharpness IV + IV");
        assert_eq!(
            sharp_v
                .result_item
                .enchantments()
                .get("minecraft:sharpness"),
            Some(&5)
        );
        assert_eq!(sharp_v.repair_cost_levels, 5);

        // 2. Sword Sharpness V + Book Smite I -> Smite dropped with Java cost +1
        let book_smite = rules
            .try_new_item("minecraft:enchanted_book", 1)
            .expect("enchanted_book")
            .with_enchantment("minecraft:smite", 1);
        let conflict = rules
            .calculate_repair(
                &AnvilRepairInput::combine(sharp_v.result_item.clone(), book_smite),
                false,
            )
            .expect("Sharpness V + Smite I");
        assert_eq!(
            conflict
                .result_item
                .enchantments()
                .get("minecraft:sharpness"),
            Some(&5)
        );
        assert_eq!(
            conflict.result_item.enchantments().get("minecraft:smite"),
            None
        );
        // 1 incompatible + 1 prior work penalty on sharp_v = 2
        assert_eq!(conflict.repair_cost_levels, 2);

        // 3. Book + Book merge (Wiki: Thorns III + Feather Falling IV -> cost 4;
        //    and Mending + Infinity -> Infinity dropped, cost 1)
        let thorns_book = rules
            .try_new_item("minecraft:enchanted_book", 1)
            .expect("book")
            .with_enchantment("minecraft:thorns", 3);
        let ff_book = rules
            .try_new_item("minecraft:enchanted_book", 1)
            .expect("book")
            .with_enchantment("minecraft:feather_falling", 4);
        let merged = rules
            .calculate_repair(&AnvilRepairInput::combine(thorns_book, ff_book), false)
            .expect("Thorns + Feather Falling books");
        assert!(merged.result_item.is_enchanted_book());
        assert_eq!(
            merged.result_item.enchantments().get("minecraft:thorns"),
            Some(&3)
        );
        assert_eq!(
            merged
                .result_item
                .enchantments()
                .get("minecraft:feather_falling"),
            Some(&4)
        );
        assert_eq!(merged.repair_cost_levels, 4);

        let mending_book = rules
            .try_new_item("minecraft:enchanted_book", 1)
            .expect("book")
            .with_enchantment("minecraft:mending", 1);
        let infinity_book = rules
            .try_new_item("minecraft:enchanted_book", 1)
            .expect("book")
            .with_enchantment("minecraft:infinity", 1);
        let bow_conflict = rules
            .calculate_repair(
                &AnvilRepairInput::combine(mending_book, infinity_book),
                false,
            )
            .expect("Mending + Infinity books");
        assert_eq!(
            bow_conflict
                .result_item
                .enchantments()
                .get("minecraft:mending"),
            Some(&1)
        );
        assert_eq!(
            bow_conflict
                .result_item
                .enchantments()
                .get("minecraft:infinity"),
            None
        );
        assert_eq!(bow_conflict.repair_cost_levels, 1);

        // 4. Item that cannot take the enchantment: bow + Sharpness I book
        let bow = rules.try_new_item("minecraft:bow", 1).expect("bow");
        let sharp_book = rules
            .try_new_item("minecraft:enchanted_book", 1)
            .expect("book")
            .with_enchantment("minecraft:sharpness", 1);
        assert_eq!(
            rules.calculate_repair(&AnvilRepairInput::combine(bow, sharp_book), false),
            Err(AnvilError::IncompatibleSacrificeItem)
        );

        // 5. Too Expensive (> 39) in survival vs permitted in creative
        let boots = rules
            .try_new_item("minecraft:diamond_boots", 1)
            .expect("boots")
            .with_repair_cost(31);
        let soul_speed_book = rules
            .try_new_item("minecraft:enchanted_book", 1)
            .expect("book")
            .with_enchantment("minecraft:soul_speed", 3)
            .with_repair_cost(1);
        // Soul Speed III from book (3 * 4 = 12) + penalties (31 + 1) = 44 >= 40
        assert_eq!(
            rules.calculate_repair(
                &AnvilRepairInput::combine(boots.clone(), soul_speed_book.clone()),
                false,
            ),
            Err(AnvilError::TooExpensive {
                cost: 44,
                limit: SURVIVAL_LEVEL_CAP,
            })
        );
        let creative = rules
            .calculate_repair(&AnvilRepairInput::combine(boots, soul_speed_book), true)
            .expect("allowed in creative");
        assert_eq!(creative.repair_cost_levels, 44);
        assert_eq!(
            creative
                .result_item
                .enchantments()
                .get("minecraft:soul_speed"),
            Some(&3)
        );
    }

    #[test]
    fn anvil_rename_and_enchant_writes_item_stack_components() {
        let rules = AnvilRules::from_enchantments(&synthetic_enchantment_registry());
        let damaged_sword = item("minecraft:diamond_sword", 1)
            .with_durability(1000)
            .expect("durability")
            .with_enchantment("minecraft:sharpness", 4)
            .with_repair_cost(3);
        let book = item("minecraft:enchanted_book", 1)
            .with_enchantment("minecraft:sharpness", 4)
            .with_repair_cost(1);
        assert_eq!(
            book.stack()
                .components()
                .stored_enchantments()
                .get("minecraft:sharpness"),
            Some(&4)
        );
        assert!(book.stack().components().enchantments().is_empty());

        let input = AnvilRepairInput::new(damaged_sword, Some(book), Some("Excalibur".to_owned()))
            .expect("rename + enchant input");
        let result = rules
            .calculate_repair(&input, false)
            .expect("rename + enchant succeeds");

        let stack = result.result_item.stack();
        let components = stack.components();
        assert_eq!(components.custom_name(), Some("Excalibur"));
        assert_eq!(
            components.enchantments().get("minecraft:sharpness"),
            Some(&5)
        );
        assert!(components.stored_enchantments().is_empty());
        assert_eq!(components.repair_cost(), Some(7));
        assert_eq!(components.damage(), Some(1561 - 1000));

        // Merging two enchanted books writes to `stored_enchantments` rather than `enchantments`.
        let book_a = item("minecraft:enchanted_book", 1).with_enchantment("minecraft:sharpness", 3);
        let book_b =
            item("minecraft:enchanted_book", 1).with_enchantment("minecraft:unbreaking", 2);
        let merged_books = rules
            .calculate_repair(&AnvilRepairInput::combine(book_a, book_b), false)
            .expect("book merge succeeds");
        let book_components = merged_books.result_item.stack().components();
        assert!(book_components.enchantments().is_empty());
        assert_eq!(
            book_components
                .stored_enchantments()
                .get("minecraft:sharpness"),
            Some(&3)
        );
        assert_eq!(
            book_components
                .stored_enchantments()
                .get("minecraft:unbreaking"),
            Some(&2)
        );
        assert_eq!(book_components.repair_cost(), Some(1));
        assert_eq!(book_components.damage(), None);
    }
}
