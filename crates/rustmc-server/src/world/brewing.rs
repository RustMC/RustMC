//! Milestone M4: Brewing Stand State Machine, Potion Recipes, and Container Synchronization.
//!
//! Provides authoritative brewing stand block entity state progression, fuel consumption
//! and replenishment, canonical vanilla potion recipe resolution and modifiers, multi-bottle
//! simultaneous brewing, and Java Edition 26.3 `SetContainerData` (packet ID 19) encoding
//! via [`crate::world::container_data::SetContainerDataPacket`].
//!
//! Sources: `SetContainerData` property indices from minecraft.wiki
//! `Java_Edition_protocol/Packets` revision 3868170; recipes and timings from
//! minecraft.wiki `Brewing` revision 3885554 (excluding Bedrock-only entries);
//! `minecraft:potion` registry entries from the official 26.3 server report
//! `registries.json`.

use super::container_data::SetContainerDataPacket;
use crate::item::{DEFAULT_MAX_STACK, ItemStack, ItemStackError};
use std::fmt;

/// Potion bottle slot 0 (left slot).
pub const POTION_SLOT_0: usize = 0;
/// Potion bottle slot 1 (middle slot).
pub const POTION_SLOT_1: usize = 1;
/// Potion bottle slot 2 (right slot).
pub const POTION_SLOT_2: usize = 2;
/// Range of potion bottle slots (0..=2).
pub const POTION_SLOTS: std::ops::RangeInclusive<usize> = 0..=2;

/// Ingredient slot index (top center slot; minecraft.wiki `Java_Edition_protocol/Inventory`).
pub const INGREDIENT_SLOT: usize = 3;

/// Blaze powder fuel slot index (top left slot; minecraft.wiki `Java_Edition_protocol/Inventory`).
pub const FUEL_SLOT: usize = 4;

/// Total number of inventory slots in a brewing stand (minecraft.wiki `Brewing` rev 3885554).
pub const TOTAL_SLOTS: usize = 5;

/// Total brew cycle duration in game ticks (minecraft.wiki `Brewing` rev 3885554: 400 ticks = 20s).
pub const BREW_TIME_MAX: u16 = 400;

/// Maximum blaze powder fuel charges a brewing stand can hold (minecraft.wiki `Brewing` rev 3885554).
pub const MAX_FUEL_CHARGES: u8 = 20;

/// Number of fuel charges replenished per Blaze Powder consumed (minecraft.wiki `Brewing` rev 3885554).
pub const FUEL_PER_BLAZE_POWDER: u8 = 20;

/// Maximum stack size for potion bottles in vanilla Java Edition (minecraft.wiki `Potion`).
pub const POTION_MAX_STACK: u16 = 1;

/// Valid `minecraft:potion` registry IDs from the official 26.3 server report `registries.json`.
const VANILLA_POTION_TYPES: [&str; 46] = [
    "minecraft:awkward",
    "minecraft:fire_resistance",
    "minecraft:harming",
    "minecraft:healing",
    "minecraft:infested",
    "minecraft:invisibility",
    "minecraft:leaping",
    "minecraft:long_fire_resistance",
    "minecraft:long_invisibility",
    "minecraft:long_leaping",
    "minecraft:long_night_vision",
    "minecraft:long_poison",
    "minecraft:long_regeneration",
    "minecraft:long_slow_falling",
    "minecraft:long_slowness",
    "minecraft:long_strength",
    "minecraft:long_swiftness",
    "minecraft:long_turtle_master",
    "minecraft:long_water_breathing",
    "minecraft:long_weakness",
    "minecraft:luck",
    "minecraft:mundane",
    "minecraft:night_vision",
    "minecraft:oozing",
    "minecraft:poison",
    "minecraft:regeneration",
    "minecraft:slow_falling",
    "minecraft:slowness",
    "minecraft:strength",
    "minecraft:strong_harming",
    "minecraft:strong_healing",
    "minecraft:strong_leaping",
    "minecraft:strong_poison",
    "minecraft:strong_regeneration",
    "minecraft:strong_slowness",
    "minecraft:strong_strength",
    "minecraft:strong_swiftness",
    "minecraft:strong_turtle_master",
    "minecraft:swiftness",
    "minecraft:thick",
    "minecraft:turtle_master",
    "minecraft:water",
    "minecraft:water_breathing",
    "minecraft:weakness",
    "minecraft:weaving",
    "minecraft:wind_charged",
];

/// Strips optional `minecraft:` namespace prefix for canonical item or potion matching.
#[inline]
#[must_use]
pub fn normalize_item_id(item: &str) -> &str {
    item.strip_prefix("minecraft:").unwrap_or(item)
}

/// Returns whether `item` is one of the three Java 26.3 potion container items.
#[inline]
#[must_use]
pub fn is_potion_container_item(item: &str) -> bool {
    matches!(
        normalize_item_id(item),
        "potion" | "splash_potion" | "lingering_potion"
    )
}

/// Canonicalizes a potion type from the `minecraft:potion` registry if valid.
#[must_use]
pub fn canonical_potion_type(potion_type: &str) -> Option<&'static str> {
    let bare = normalize_item_id(potion_type);
    VANILLA_POTION_TYPES
        .iter()
        .copied()
        .find(|entry| normalize_item_id(entry) == bare)
}

/// Checks if two item identifiers match canonically in the `minecraft` namespace.
#[must_use]
pub fn items_equal(a: &str, b: &str) -> bool {
    normalize_item_id(a) == normalize_item_id(b)
}

/// Returns the standard default maximum stack size for brewing items.
#[must_use]
pub fn default_max_stack(item: &str) -> u16 {
    if is_potion_container_item(item) {
        POTION_MAX_STACK
    } else {
        DEFAULT_MAX_STACK
    }
}

/// Potion effect kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PotionEffect {
    Water,
    Awkward,
    Healing,
    Harming,
    Swiftness,
    Slowness,
    Strength,
    FireResistance,
    Weakness,
}

/// Potion potency tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum PotionTier {
    #[default]
    Normal,
    Strong, // Level II
}

/// Potion duration modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum PotionDuration {
    #[default]
    Normal,
    Long, // Extended
}

/// Structured descriptor for the subset of `minecraft:potion` registry entries with active recipes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Potion {
    effect: PotionEffect,
    tier: PotionTier,
    duration: PotionDuration,
}

impl Potion {
    /// Returns the water potion descriptor (`minecraft:water`).
    pub const fn water() -> Self {
        Self {
            effect: PotionEffect::Water,
            tier: PotionTier::Normal,
            duration: PotionDuration::Normal,
        }
    }

    /// Returns the awkward potion descriptor (`minecraft:awkward`).
    pub const fn awkward() -> Self {
        Self {
            effect: PotionEffect::Awkward,
            tier: PotionTier::Normal,
            duration: PotionDuration::Normal,
        }
    }

    /// Constructs a structured potion descriptor.
    pub const fn new(effect: PotionEffect, tier: PotionTier, duration: PotionDuration) -> Self {
        Self {
            effect,
            tier,
            duration,
        }
    }

    /// Returns the potion effect kind.
    #[must_use]
    pub const fn effect(self) -> PotionEffect {
        self.effect
    }

    /// Returns the potion potency tier.
    #[must_use]
    pub const fn tier(self) -> PotionTier {
        self.tier
    }

    /// Returns the potion duration modifier.
    #[must_use]
    pub const fn duration(self) -> PotionDuration {
        self.duration
    }

    /// Parses a potion type from a `minecraft:potion` registry identifier.
    #[must_use]
    pub fn from_registry_id(potion_type: &str) -> Option<Self> {
        match normalize_item_id(potion_type) {
            "water" => Some(Self::water()),
            "awkward" => Some(Self::awkward()),
            "healing" => Some(Self::new(
                PotionEffect::Healing,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "strong_healing" => Some(Self::new(
                PotionEffect::Healing,
                PotionTier::Strong,
                PotionDuration::Normal,
            )),
            "harming" => Some(Self::new(
                PotionEffect::Harming,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "strong_harming" => Some(Self::new(
                PotionEffect::Harming,
                PotionTier::Strong,
                PotionDuration::Normal,
            )),
            "swiftness" => Some(Self::new(
                PotionEffect::Swiftness,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "long_swiftness" => Some(Self::new(
                PotionEffect::Swiftness,
                PotionTier::Normal,
                PotionDuration::Long,
            )),
            "strong_swiftness" => Some(Self::new(
                PotionEffect::Swiftness,
                PotionTier::Strong,
                PotionDuration::Normal,
            )),
            "slowness" => Some(Self::new(
                PotionEffect::Slowness,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "long_slowness" => Some(Self::new(
                PotionEffect::Slowness,
                PotionTier::Normal,
                PotionDuration::Long,
            )),
            "strong_slowness" => Some(Self::new(
                PotionEffect::Slowness,
                PotionTier::Strong,
                PotionDuration::Normal,
            )),
            "strength" => Some(Self::new(
                PotionEffect::Strength,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "long_strength" => Some(Self::new(
                PotionEffect::Strength,
                PotionTier::Normal,
                PotionDuration::Long,
            )),
            "strong_strength" => Some(Self::new(
                PotionEffect::Strength,
                PotionTier::Strong,
                PotionDuration::Normal,
            )),
            "fire_resistance" => Some(Self::new(
                PotionEffect::FireResistance,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "long_fire_resistance" => Some(Self::new(
                PotionEffect::FireResistance,
                PotionTier::Normal,
                PotionDuration::Long,
            )),
            "weakness" => Some(Self::new(
                PotionEffect::Weakness,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "long_weakness" => Some(Self::new(
                PotionEffect::Weakness,
                PotionTier::Normal,
                PotionDuration::Long,
            )),
            _ => None,
        }
    }

    /// Returns the canonical `minecraft:potion` registry identifier.
    #[must_use]
    pub const fn registry_id(&self) -> &'static str {
        match (self.effect, self.tier, self.duration) {
            (PotionEffect::Water, _, _) => "minecraft:water",
            (PotionEffect::Awkward, _, _) => "minecraft:awkward",

            (PotionEffect::Healing, PotionTier::Normal, _) => "minecraft:healing",
            (PotionEffect::Healing, PotionTier::Strong, _) => "minecraft:strong_healing",

            (PotionEffect::Harming, PotionTier::Normal, _) => "minecraft:harming",
            (PotionEffect::Harming, PotionTier::Strong, _) => "minecraft:strong_harming",

            (PotionEffect::Swiftness, PotionTier::Normal, PotionDuration::Normal) => {
                "minecraft:swiftness"
            }
            (PotionEffect::Swiftness, PotionTier::Normal, PotionDuration::Long) => {
                "minecraft:long_swiftness"
            }
            (PotionEffect::Swiftness, PotionTier::Strong, _) => "minecraft:strong_swiftness",

            (PotionEffect::Slowness, PotionTier::Normal, PotionDuration::Normal) => {
                "minecraft:slowness"
            }
            (PotionEffect::Slowness, PotionTier::Normal, PotionDuration::Long) => {
                "minecraft:long_slowness"
            }
            (PotionEffect::Slowness, PotionTier::Strong, _) => "minecraft:strong_slowness",

            (PotionEffect::Strength, PotionTier::Normal, PotionDuration::Normal) => {
                "minecraft:strength"
            }
            (PotionEffect::Strength, PotionTier::Normal, PotionDuration::Long) => {
                "minecraft:long_strength"
            }
            (PotionEffect::Strength, PotionTier::Strong, _) => "minecraft:strong_strength",

            (PotionEffect::FireResistance, _, PotionDuration::Normal) => {
                "minecraft:fire_resistance"
            }
            (PotionEffect::FireResistance, _, PotionDuration::Long) => {
                "minecraft:long_fire_resistance"
            }

            (PotionEffect::Weakness, _, PotionDuration::Normal) => "minecraft:weakness",
            (PotionEffect::Weakness, _, PotionDuration::Long) => "minecraft:long_weakness",
        }
    }
}

/// Evaluates Java Edition potion modifier transformations (minecraft.wiki `Brewing` rev 3885554).
#[must_use]
pub fn evaluate_potion_modifier(ingredient: &str, input_potion_type: &str) -> Option<&'static str> {
    let ing = normalize_item_id(ingredient);
    let potion = Potion::from_registry_id(input_potion_type)?;

    match ing {
        // Redstone: extends duration (normal duration -> long duration)
        "redstone" => {
            if potion.duration == PotionDuration::Normal && potion.tier == PotionTier::Normal {
                match potion.effect {
                    PotionEffect::Swiftness
                    | PotionEffect::Strength
                    | PotionEffect::FireResistance
                    | PotionEffect::Slowness
                    | PotionEffect::Weakness => Some(
                        Potion::new(potion.effect, PotionTier::Normal, PotionDuration::Long)
                            .registry_id(),
                    ),
                    _ => None,
                }
            } else {
                None
            }
        }

        // Glowstone Dust: upgrades potency tier (normal tier -> strong tier II)
        "glowstone_dust" => {
            if potion.tier == PotionTier::Normal && potion.duration == PotionDuration::Normal {
                match potion.effect {
                    PotionEffect::Healing
                    | PotionEffect::Harming
                    | PotionEffect::Swiftness
                    | PotionEffect::Strength
                    | PotionEffect::Slowness => Some(
                        Potion::new(potion.effect, PotionTier::Strong, PotionDuration::Normal)
                            .registry_id(),
                    ),
                    _ => None,
                }
            } else {
                None
            }
        }

        // Fermented Spider Eye: inverts effect (Java Edition rules only)
        "fermented_spider_eye" => match potion.effect {
            PotionEffect::Healing => {
                Some(Potion::new(PotionEffect::Harming, potion.tier, potion.duration).registry_id())
            }
            PotionEffect::Swiftness if potion.tier == PotionTier::Normal => Some(
                Potion::new(PotionEffect::Slowness, potion.tier, potion.duration).registry_id(),
            ),
            PotionEffect::Water => Some(
                Potion::new(
                    PotionEffect::Weakness,
                    PotionTier::Normal,
                    PotionDuration::Normal,
                )
                .registry_id(),
            ),
            _ => None,
        },

        _ => None,
    }
}

/// An item stack in a brewing stand slot, paired with an optional `minecraft:potion`
/// registry entry because [`ItemStack`] does not model data components yet.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BrewingSlotItem {
    stack: ItemStack,
    potion_type: Option<String>,
}

impl BrewingSlotItem {
    /// Constructs a validated brewing slot item from a shared [`ItemStack`] and optional potion type.
    pub fn try_new(stack: ItemStack, potion_type: Option<&str>) -> Result<Self, BrewingError> {
        if is_potion_container_item(stack.item()) {
            let Some(raw_potion_type) = potion_type else {
                return Err(BrewingError::MissingPotionType);
            };
            let Some(canonical) = canonical_potion_type(raw_potion_type) else {
                return Err(BrewingError::InvalidPotionType(raw_potion_type.to_owned()));
            };
            if stack.max_stack() != POTION_MAX_STACK {
                return Err(BrewingError::InvalidStack(ItemStackError::InvalidMaxStack(
                    stack.max_stack(),
                )));
            }
            Ok(Self {
                stack,
                potion_type: Some(canonical.to_owned()),
            })
        } else if potion_type.is_some() {
            Err(BrewingError::UnexpectedPotionType)
        } else {
            Ok(Self {
                stack,
                potion_type: None,
            })
        }
    }

    /// Constructs a non-potion item stack using default stack limits.
    pub fn item(item: &str, count: u16) -> Result<Self, BrewingError> {
        if is_potion_container_item(item) {
            return Err(BrewingError::MissingPotionType);
        }
        let stack = ItemStack::try_new(item, count, default_max_stack(item))
            .map_err(BrewingError::InvalidStack)?;
        Self::try_new(stack, None)
    }

    /// Constructs a potion stack (`minecraft:potion`, `minecraft:splash_potion`, or
    /// `minecraft:lingering_potion`) with a validated `minecraft:potion` registry type.
    pub fn potion_with_container(
        container_item: &str,
        potion_type: &str,
    ) -> Result<Self, BrewingError> {
        if !is_potion_container_item(container_item) {
            return Err(BrewingError::InvalidPotionContainer(
                container_item.to_owned(),
            ));
        }
        let stack = ItemStack::try_new(container_item, 1, POTION_MAX_STACK)
            .map_err(BrewingError::InvalidStack)?;
        Self::try_new(stack, Some(potion_type))
    }

    /// Constructs a standard drinkable `minecraft:potion` with the given registry potion type.
    pub fn potion(potion_type: &str) -> Result<Self, BrewingError> {
        Self::potion_with_container("minecraft:potion", potion_type)
    }

    /// Constructs a `minecraft:splash_potion` with the given registry potion type.
    pub fn splash_potion(potion_type: &str) -> Result<Self, BrewingError> {
        Self::potion_with_container("minecraft:splash_potion", potion_type)
    }

    /// Constructs a `minecraft:lingering_potion` with the given registry potion type.
    pub fn lingering_potion(potion_type: &str) -> Result<Self, BrewingError> {
        Self::potion_with_container("minecraft:lingering_potion", potion_type)
    }

    /// Helper constructor for a water bottle (`minecraft:potion` + `minecraft:water`).
    #[must_use]
    pub fn water_bottle() -> Self {
        Self::potion("minecraft:water").expect("minecraft:water is in the 26.3 potion registry")
    }

    /// Helper constructor for an awkward potion (`minecraft:potion` + `minecraft:awkward`).
    #[must_use]
    pub fn awkward_potion() -> Self {
        Self::potion("minecraft:awkward").expect("minecraft:awkward is in the 26.3 potion registry")
    }

    /// Helper constructor for blaze powder fuel (`minecraft:blaze_powder`).
    pub fn blaze_powder(count: u16) -> Result<Self, BrewingError> {
        Self::item("minecraft:blaze_powder", count)
    }

    /// Helper constructor from a structured [`Potion`].
    #[must_use]
    pub fn from_potion(potion: Potion) -> Self {
        Self::potion(potion.registry_id()).expect("Potion variants map to 26.3 potion registry IDs")
    }

    /// Returns the underlying shared [`ItemStack`].
    #[inline]
    #[must_use]
    pub fn stack(&self) -> &ItemStack {
        &self.stack
    }

    /// Returns the `minecraft:potion` registry type if this slot holds a potion bottle.
    #[inline]
    #[must_use]
    pub fn potion_type(&self) -> Option<&str> {
        self.potion_type.as_deref()
    }

    /// Returns the item identifier from the underlying [`ItemStack`].
    #[inline]
    #[must_use]
    pub fn item_id(&self) -> &str {
        self.stack.item()
    }

    /// Returns the stack count from the underlying [`ItemStack`].
    #[inline]
    #[must_use]
    pub fn count(&self) -> u16 {
        self.stack.count()
    }

    /// Returns the parsed structured [`Potion`] if this slot holds a supported potion type.
    #[inline]
    #[must_use]
    pub fn potion_descriptor(&self) -> Option<Potion> {
        self.potion_type
            .as_deref()
            .and_then(Potion::from_registry_id)
    }

    /// Whether this slot holds a potion item with a potion type.
    #[inline]
    #[must_use]
    pub fn is_potion(&self) -> bool {
        is_potion_container_item(self.stack.item()) && self.potion_type.is_some()
    }

    /// Whether this slot holds a water bottle (`minecraft:water`).
    #[inline]
    #[must_use]
    pub fn is_water_bottle(&self) -> bool {
        self.potion_type.as_deref() == Some("minecraft:water")
    }

    /// Whether this slot holds an awkward potion (`minecraft:awkward`).
    #[inline]
    #[must_use]
    pub fn is_awkward_potion(&self) -> bool {
        self.potion_type.as_deref() == Some("minecraft:awkward")
    }

    /// Whether this slot holds a potion with the given [`PotionEffect`].
    #[inline]
    #[must_use]
    pub fn is_potion_of(&self, effect: PotionEffect) -> bool {
        matches!(self.potion_descriptor(), Some(p) if p.effect == effect)
    }
}

/// A brewing recipe mapping an ingredient item and input `minecraft:potion` registry type
/// to an output `minecraft:potion` registry type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrewingRecipe {
    ingredient: String,
    input_potion: String,
    output_potion: String,
}

impl BrewingRecipe {
    /// Constructs a validated brewing recipe using real item and `minecraft:potion` registry IDs.
    pub fn new(
        ingredient: impl Into<String>,
        input_potion: impl AsRef<str>,
        output_potion: impl AsRef<str>,
    ) -> Result<Self, BrewingError> {
        let ingredient = ingredient.into();
        ItemStack::try_new(ingredient.clone(), 1, DEFAULT_MAX_STACK)
            .map_err(BrewingError::InvalidStack)?;
        let input_potion = canonical_potion_type(input_potion.as_ref())
            .ok_or_else(|| BrewingError::InvalidPotionType(input_potion.as_ref().to_owned()))?;
        let output_potion = canonical_potion_type(output_potion.as_ref())
            .ok_or_else(|| BrewingError::InvalidPotionType(output_potion.as_ref().to_owned()))?;
        Ok(Self {
            ingredient,
            input_potion: input_potion.to_owned(),
            output_potion: output_potion.to_owned(),
        })
    }

    /// Returns the ingredient item identifier.
    #[must_use]
    pub fn ingredient(&self) -> &str {
        &self.ingredient
    }

    /// Returns the input `minecraft:potion` registry identifier.
    #[must_use]
    pub fn input_potion(&self) -> &str {
        &self.input_potion
    }

    /// Returns the output `minecraft:potion` registry identifier.
    #[must_use]
    pub fn output_potion(&self) -> &str {
        &self.output_potion
    }

    /// Checks if this recipe matches the given ingredient and input `minecraft:potion` registry type.
    #[must_use]
    pub fn matches(&self, ingredient: &str, input_potion_type: &str) -> bool {
        items_equal(&self.ingredient, ingredient)
            && canonical_potion_type(input_potion_type) == Some(self.input_potion.as_str())
    }
}

/// Registry of brewing recipes for the brewing stand state machine.
#[derive(Debug, Clone)]
pub struct BrewingRecipeRegistry {
    recipes: Vec<BrewingRecipe>,
}

impl BrewingRecipeRegistry {
    /// Creates a registry initialized with the supported Java brewing recipes.
    pub fn new() -> Self {
        let mut reg = Self {
            recipes: Vec::new(),
        };
        reg.register_defaults();
        reg
    }

    /// Registers a custom brewing recipe.
    pub fn register(&mut self, recipe: BrewingRecipe) {
        self.recipes.push(recipe);
    }

    /// Registers a brewing recipe by ingredient item ID and `minecraft:potion` registry IDs.
    pub fn register_recipe(
        &mut self,
        ingredient: impl Into<String>,
        input_potion: impl AsRef<str>,
        output_potion: impl AsRef<str>,
    ) -> Result<(), BrewingError> {
        let recipe = BrewingRecipe::new(ingredient, input_potion, output_potion)?;
        self.register(recipe);
        Ok(())
    }

    /// Finds a matching recipe in the registry.
    #[must_use]
    pub fn find_recipe(&self, ingredient: &str, input_potion_type: &str) -> Option<&BrewingRecipe> {
        self.recipes
            .iter()
            .rev()
            .find(|r| r.matches(ingredient, input_potion_type))
    }

    /// Evaluates the output `minecraft:potion` registry type for an ingredient and input potion type.
    #[must_use]
    pub fn find_output_potion_type(
        &self,
        ingredient: &str,
        input_potion_type: &str,
    ) -> Option<String> {
        if let Some(r) = self.find_recipe(ingredient, input_potion_type) {
            return Some(r.output_potion.clone());
        }
        evaluate_potion_modifier(ingredient, input_potion_type).map(str::to_owned)
    }

    /// Evaluates the resulting `(container_item, potion_type)` for a bottle slot item.
    ///
    /// Includes Gunpowder (`minecraft:potion` -> `minecraft:splash_potion`) and
    /// Dragon's Breath (`minecraft:splash_potion` -> `minecraft:lingering_potion`)
    /// from minecraft.wiki `Brewing` rev 3885554.
    #[must_use]
    pub fn find_output_item(
        &self,
        ingredient: &str,
        bottle: &BrewingSlotItem,
    ) -> Option<(String, String)> {
        let potion_type = bottle.potion_type()?;
        if !is_potion_container_item(bottle.item_id()) {
            return None;
        }
        let ing = normalize_item_id(ingredient);
        let container = normalize_item_id(bottle.item_id());
        if ing == "gunpowder" && container == "potion" {
            return Some(("minecraft:splash_potion".to_owned(), potion_type.to_owned()));
        }
        if ing == "dragon_breath" && container == "splash_potion" {
            return Some((
                "minecraft:lingering_potion".to_owned(),
                potion_type.to_owned(),
            ));
        }
        let next_potion_type = self.find_output_potion_type(ingredient, potion_type)?;
        Some((bottle.item_id().to_owned(), next_potion_type))
    }

    /// Checks whether an ingredient and bottle item form a valid brewing transformation.
    #[must_use]
    pub fn has_recipe_for_item(&self, ingredient: &str, bottle: &BrewingSlotItem) -> bool {
        self.find_output_item(ingredient, bottle).is_some()
    }

    /// Checks whether an ingredient and input `minecraft:potion` type form a valid recipe.
    #[must_use]
    pub fn has_recipe(&self, ingredient: &str, input_potion_type: &str) -> bool {
        self.find_output_potion_type(ingredient, input_potion_type)
            .is_some()
    }

    fn register_builtin(&mut self, ingredient: &str, input_potion: &str, output_potion: &str) {
        self.register_recipe(ingredient, input_potion, output_potion)
            .expect("built-in brewing recipes use valid 26.3 item and potion registry IDs");
    }

    /// Populates the supported Java Edition potion recipes from minecraft.wiki `Brewing` rev 3885554.
    fn register_defaults(&mut self) {
        // Base potion recipes
        self.register_builtin(
            "minecraft:nether_wart",
            "minecraft:water",
            "minecraft:awkward",
        );
        self.register_builtin(
            "minecraft:glistering_melon_slice",
            "minecraft:awkward",
            "minecraft:healing",
        );
        self.register_builtin(
            "minecraft:sugar",
            "minecraft:awkward",
            "minecraft:swiftness",
        );
        self.register_builtin(
            "minecraft:blaze_powder",
            "minecraft:awkward",
            "minecraft:strength",
        );
        self.register_builtin(
            "minecraft:magma_cream",
            "minecraft:awkward",
            "minecraft:fire_resistance",
        );

        // Redstone duration modifier (extends duration)
        self.register_builtin(
            "minecraft:redstone",
            "minecraft:swiftness",
            "minecraft:long_swiftness",
        );
        self.register_builtin(
            "minecraft:redstone",
            "minecraft:strength",
            "minecraft:long_strength",
        );
        self.register_builtin(
            "minecraft:redstone",
            "minecraft:fire_resistance",
            "minecraft:long_fire_resistance",
        );
        self.register_builtin(
            "minecraft:redstone",
            "minecraft:slowness",
            "minecraft:long_slowness",
        );
        self.register_builtin(
            "minecraft:redstone",
            "minecraft:weakness",
            "minecraft:long_weakness",
        );

        // Glowstone Dust potency modifier (tier II)
        self.register_builtin(
            "minecraft:glowstone_dust",
            "minecraft:healing",
            "minecraft:strong_healing",
        );
        self.register_builtin(
            "minecraft:glowstone_dust",
            "minecraft:swiftness",
            "minecraft:strong_swiftness",
        );
        self.register_builtin(
            "minecraft:glowstone_dust",
            "minecraft:strength",
            "minecraft:strong_strength",
        );
        self.register_builtin(
            "minecraft:glowstone_dust",
            "minecraft:harming",
            "minecraft:strong_harming",
        );
        self.register_builtin(
            "minecraft:glowstone_dust",
            "minecraft:slowness",
            "minecraft:strong_slowness",
        );

        // Fermented Spider Eye effect inversion (Java Edition only)
        self.register_builtin(
            "minecraft:fermented_spider_eye",
            "minecraft:healing",
            "minecraft:harming",
        );
        self.register_builtin(
            "minecraft:fermented_spider_eye",
            "minecraft:strong_healing",
            "minecraft:strong_harming",
        );
        self.register_builtin(
            "minecraft:fermented_spider_eye",
            "minecraft:swiftness",
            "minecraft:slowness",
        );
        self.register_builtin(
            "minecraft:fermented_spider_eye",
            "minecraft:long_swiftness",
            "minecraft:long_slowness",
        );
        self.register_builtin(
            "minecraft:fermented_spider_eye",
            "minecraft:water",
            "minecraft:weakness",
        );
    }
}

impl Default for BrewingRecipeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Java 26.3 brewing stand container data property indices
/// (minecraft.wiki `Java_Edition_protocol/Packets` rev 3868170).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum BrewingStandProperty {
    /// Property 0: Remaining brew time ticks (400 down to 0).
    BrewTime = 0,
    /// Property 1: Remaining fuel charges (0 up to 20).
    FuelCharges = 1,
}

impl BrewingStandProperty {
    /// Returns the numeric window property index.
    #[inline]
    #[must_use]
    pub const fn index(self) -> u16 {
        self as u16
    }

    /// Parses a numeric window property index into [`BrewingStandProperty`].
    #[must_use]
    pub const fn from_index(index: u16) -> Option<Self> {
        match index {
            0 => Some(Self::BrewTime),
            1 => Some(Self::FuelCharges),
            _ => None,
        }
    }
}

/// Errors related to brewing stand slots, potions, and recipes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrewingError {
    InvalidSlotIndex(usize),
    InvalidStack(ItemStackError),
    InvalidPotionContainer(String),
    InvalidPotionType(String),
    MissingPotionType,
    UnexpectedPotionType,
}

impl fmt::Display for BrewingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlotIndex(idx) => {
                write!(
                    f,
                    "invalid slot index {idx}; brewing stand has {TOTAL_SLOTS} slots"
                )
            }
            Self::InvalidStack(err) => write!(f, "invalid brewing stack: {err}"),
            Self::InvalidPotionContainer(item) => {
                write!(f, "item '{item}' is not a valid potion container")
            }
            Self::InvalidPotionType(potion_type) => {
                write!(f, "unknown 26.3 potion registry type '{potion_type}'")
            }
            Self::MissingPotionType => {
                f.write_str("potion container requires a minecraft:potion registry type")
            }
            Self::UnexpectedPotionType => {
                f.write_str("non-potion item cannot carry a potion registry type")
            }
        }
    }
}

impl std::error::Error for BrewingError {}

/// Authoritative state machine for a Minecraft brewing stand block entity.
#[derive(Debug, Clone)]
pub struct BrewingStandBlockEntity {
    /// 5 slots: slots 0..=2 = potions, slot 3 = ingredient, slot 4 = blaze powder fuel.
    slots: [Option<BrewingSlotItem>; TOTAL_SLOTS],
    /// Countdown brew timer from 400 ticks (20s) down to 0.
    brew_time: u16,
    /// Remaining fuel charges (0..=20).
    fuel_charges: u8,
    /// Recipe registry for resolving brewing transitions.
    recipe_registry: BrewingRecipeRegistry,
    /// Ingredient the running cycle started with; changing it cancels the cycle.
    brewing_ingredient: Option<String>,
}

impl BrewingStandBlockEntity {
    /// Creates a new empty brewing stand with 0 fuel and default recipes.
    pub fn new() -> Self {
        Self {
            slots: [None, None, None, None, None],
            brew_time: 0,
            fuel_charges: 0,
            recipe_registry: BrewingRecipeRegistry::new(),
            brewing_ingredient: None,
        }
    }

    /// Creates a brewing stand with a custom recipe registry.
    pub fn with_registry(recipe_registry: BrewingRecipeRegistry) -> Self {
        Self {
            slots: [None, None, None, None, None],
            brew_time: 0,
            fuel_charges: 0,
            recipe_registry,
            brewing_ingredient: None,
        }
    }

    /// Returns the remaining brew timer in ticks (0 when idle).
    #[inline]
    #[must_use]
    pub fn brew_time(&self) -> u16 {
        self.brew_time
    }

    /// Sets the remaining brew timer in ticks.
    pub fn set_brew_time(&mut self, ticks: u16) {
        self.brew_time = ticks.min(BREW_TIME_MAX);
    }

    /// Returns the remaining blaze powder fuel charges (0..=20).
    #[inline]
    #[must_use]
    pub fn fuel_charges(&self) -> u8 {
        self.fuel_charges
    }

    /// Sets the remaining blaze powder fuel charges (clamped to 0..=20).
    pub fn set_fuel_charges(&mut self, charges: u8) {
        self.fuel_charges = charges.min(MAX_FUEL_CHARGES);
    }

    /// Returns a reference to the recipe registry.
    #[inline]
    #[must_use]
    pub fn recipe_registry(&self) -> &BrewingRecipeRegistry {
        &self.recipe_registry
    }

    /// Returns a mutable reference to the recipe registry.
    #[inline]
    pub fn recipe_registry_mut(&mut self) -> &mut BrewingRecipeRegistry {
        &mut self.recipe_registry
    }

    /// Returns a reference to the item in the given slot, if any.
    #[inline]
    #[must_use]
    pub fn slot(&self, index: usize) -> Option<&BrewingSlotItem> {
        self.slots.get(index).and_then(|s| s.as_ref())
    }

    /// Sets or clears an item in the given slot (0..=4).
    pub fn set_slot(
        &mut self,
        index: usize,
        stack: Option<BrewingSlotItem>,
    ) -> Result<(), BrewingError> {
        if index >= TOTAL_SLOTS {
            return Err(BrewingError::InvalidSlotIndex(index));
        }
        self.slots[index] = stack;
        Ok(())
    }

    /// Returns a reference to the potion stack in one of the 3 potion slots (0..=2).
    #[inline]
    #[must_use]
    pub fn potion_slot(&self, index: usize) -> Option<&BrewingSlotItem> {
        if index <= POTION_SLOT_2 {
            self.slot(index)
        } else {
            None
        }
    }

    /// Sets a potion stack in one of the 3 potion slots (0..=2).
    pub fn set_potion(
        &mut self,
        index: usize,
        stack: Option<BrewingSlotItem>,
    ) -> Result<(), BrewingError> {
        if index > POTION_SLOT_2 {
            return Err(BrewingError::InvalidSlotIndex(index));
        }
        self.slots[index] = stack;
        Ok(())
    }

    /// Returns a reference to the ingredient in slot 3.
    #[inline]
    #[must_use]
    pub fn ingredient(&self) -> Option<&BrewingSlotItem> {
        self.slot(INGREDIENT_SLOT)
    }

    /// Sets the ingredient in slot 3.
    pub fn set_ingredient(&mut self, stack: Option<BrewingSlotItem>) {
        self.slots[INGREDIENT_SLOT] = stack;
    }

    /// Returns a reference to the fuel stack in slot 4.
    #[inline]
    #[must_use]
    pub fn fuel(&self) -> Option<&BrewingSlotItem> {
        self.slot(FUEL_SLOT)
    }

    /// Sets the fuel stack in slot 4.
    pub fn set_fuel(&mut self, stack: Option<BrewingSlotItem>) {
        self.slots[FUEL_SLOT] = stack;
    }

    /// Whether the stand is currently actively brewing a potion cycle.
    #[inline]
    #[must_use]
    pub fn is_brewing(&self) -> bool {
        self.brew_time > 0
    }

    /// Whether the brewing stand has at least one active fuel charge.
    #[inline]
    #[must_use]
    pub fn has_fuel(&self) -> bool {
        self.fuel_charges > 0
    }

    /// Replenishes fuel charges from Blaze Powder in slot 4 if the fuel gauge is empty.
    ///
    /// Consumes 1 Blaze Powder and sets `fuel_charges` to 20. Returns true if fuel was replenished.
    pub fn replenish_fuel(&mut self) -> bool {
        if self.fuel_charges > 0 {
            return false;
        }

        let Some(fuel) = self.slots[FUEL_SLOT].take() else {
            return false;
        };

        if !items_equal(fuel.item_id(), "minecraft:blaze_powder") {
            self.slots[FUEL_SLOT] = Some(fuel);
            return false;
        }

        let (_, rest) = fuel
            .stack
            .split(1)
            .expect("splitting 1 from a non-empty ItemStack succeeds");
        self.slots[FUEL_SLOT] = rest.map(|stack| BrewingSlotItem {
            stack,
            potion_type: None,
        });
        self.fuel_charges = FUEL_PER_BLAZE_POWDER.min(MAX_FUEL_CHARGES);
        true
    }

    /// Checks if the current ingredient in slot 3 and at least one bottle in slots 0..=2 form a valid recipe.
    #[must_use]
    pub fn can_brew(&self) -> bool {
        let Some(ref ingredient) = self.slots[INGREDIENT_SLOT] else {
            return false;
        };

        for slot_idx in POTION_SLOTS {
            if let Some(ref bottle) = self.slots[slot_idx]
                && self
                    .recipe_registry
                    .has_recipe_for_item(ingredient.item_id(), bottle)
            {
                return true;
            }
        }

        false
    }

    /// Completes the current brewing cycle:
    /// - Transforms matching input potions in slots 0..=2 into recipe output potions.
    /// - Decrements the ingredient in slot 3 by 1.
    /// - Resets `brew_time` to 0.
    pub fn complete_brew(&mut self) {
        let Some(ingredient_item) = self.slots[INGREDIENT_SLOT]
            .as_ref()
            .map(|s| s.item_id().to_owned())
        else {
            self.brew_time = 0;
            return;
        };

        // Transform all matching potion slots
        for slot_idx in POTION_SLOTS {
            if let Some(ref bottle) = self.slots[slot_idx]
                && let Some((container_item, output_potion_type)) = self
                    .recipe_registry
                    .find_output_item(&ingredient_item, bottle)
            {
                self.slots[slot_idx] = Some(
                    BrewingSlotItem::potion_with_container(&container_item, &output_potion_type)
                        .expect("recipe outputs produce valid 26.3 potion container and type"),
                );
            }
        }

        // Decrement ingredient count by 1 via ItemStack::split
        if let Some(ing) = self.slots[INGREDIENT_SLOT].take() {
            let (_, rest) = ing
                .stack
                .split(1)
                .expect("splitting 1 from a non-empty ItemStack succeeds");
            self.slots[INGREDIENT_SLOT] = rest.map(|stack| BrewingSlotItem {
                stack,
                potion_type: None,
            });
        }

        // Reset brew timer
        self.brew_time = 0;
    }

    /// Advances the brewing stand state machine by one server tick (1/20 second).
    ///
    /// Returns true if any state change occurred.
    pub fn tick(&mut self) -> bool {
        // Step 1: replenish fuel charges from blaze powder slot if fuel is exhausted
        let replenished = self.fuel_charges == 0 && self.replenish_fuel();

        // Step 2: verify valid recipe and that the running cycle kept its ingredient
        let ingredient = self.slots[INGREDIENT_SLOT]
            .as_ref()
            .map(|stack| normalize_item_id(stack.item_id()).to_owned());
        let ingredient_changed =
            self.brewing_ingredient.is_some() && self.brewing_ingredient != ingredient;
        if !self.can_brew() || ingredient_changed {
            self.brewing_ingredient = None;
            if self.brew_time > 0 {
                self.brew_time = 0;
                return true;
            }
            return replenished;
        }

        // Step 3: consume fuel charge when starting a brew cycle
        if self.brew_time == 0 {
            if self.fuel_charges == 0 {
                return replenished;
            }
            self.fuel_charges -= 1;
            self.brew_time = BREW_TIME_MAX;
            self.brewing_ingredient = ingredient;
        }

        // Step 4: decrement brew_time
        self.brew_time = self.brew_time.saturating_sub(1);

        // Step 5: on reaching 0, transform potions and decrement ingredient
        if self.brew_time == 0 {
            self.complete_brew();
            self.brewing_ingredient = None;
        }

        true
    }

    /// Returns the signed 16-bit value of a container property.
    #[must_use]
    pub fn property_value(&self, prop: BrewingStandProperty) -> i16 {
        match prop {
            BrewingStandProperty::BrewTime => self.brew_time as i16,
            BrewingStandProperty::FuelCharges => self.fuel_charges as i16,
        }
    }

    /// Creates a shared `SetContainerDataPacket` for the given property.
    #[must_use]
    pub fn create_property_packet(
        &self,
        window_id: u8,
        prop: BrewingStandProperty,
    ) -> SetContainerDataPacket {
        SetContainerDataPacket::new(window_id, prop.index(), self.property_value(prop))
    }

    /// Encodes an unframed `SetContainerData` (19 / 0x13) packet payload for a given property.
    #[must_use]
    pub fn encode_container_data(&self, window_id: u8, prop: BrewingStandProperty) -> Vec<u8> {
        self.create_property_packet(window_id, prop).encode_packet()
    }

    /// Encodes a framed `SetContainerData` (19 / 0x13) packet for a given property.
    #[must_use]
    pub fn encode_container_data_framed(
        &self,
        window_id: u8,
        prop: BrewingStandProperty,
    ) -> Vec<u8> {
        self.create_property_packet(window_id, prop).encode_framed()
    }

    /// Encodes unframed packets for both brewing stand container properties.
    #[must_use]
    pub fn encode_all_container_data(&self, window_id: u8) -> [Vec<u8>; 2] {
        [
            self.encode_container_data(window_id, BrewingStandProperty::BrewTime),
            self.encode_container_data(window_id, BrewingStandProperty::FuelCharges),
        ]
    }

    /// Encodes framed packets for both brewing stand container properties.
    #[must_use]
    pub fn encode_all_container_data_framed(&self, window_id: u8) -> [Vec<u8>; 2] {
        [
            self.encode_container_data_framed(window_id, BrewingStandProperty::BrewTime),
            self.encode_container_data_framed(window_id, BrewingStandProperty::FuelCharges),
        ]
    }
}

impl Default for BrewingStandBlockEntity {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::container_data::SET_CONTAINER_DATA_PACKET_ID;

    fn item(id: &str, count: u16) -> BrewingSlotItem {
        BrewingSlotItem::item(id, count).unwrap()
    }

    fn potion(potion_type: &str) -> BrewingSlotItem {
        BrewingSlotItem::potion(potion_type).unwrap()
    }

    #[test]
    fn test_potion_item_rejects_made_up_ids_and_validates_registry_types() {
        assert_eq!(
            BrewingSlotItem::item("minecraft:potion", 1),
            Err(BrewingError::MissingPotionType)
        );
        assert_eq!(
            BrewingSlotItem::potion("minecraft:potion_of_healing"),
            Err(BrewingError::InvalidPotionType(
                "minecraft:potion_of_healing".to_owned()
            ))
        );
        assert_eq!(
            BrewingSlotItem::potion("minecraft:water_bottle"),
            Err(BrewingError::InvalidPotionType(
                "minecraft:water_bottle".to_owned()
            ))
        );
        let non_potion = ItemStack::try_new("minecraft:sugar", 1, 64).unwrap();
        assert_eq!(
            BrewingSlotItem::try_new(non_potion, Some("minecraft:healing")),
            Err(BrewingError::UnexpectedPotionType)
        );

        let heal = potion("healing");
        assert_eq!(heal.item_id(), "minecraft:potion");
        assert_eq!(heal.potion_type(), Some("minecraft:healing"));
        assert_eq!(heal.count(), 1);
        assert_eq!(heal.stack().max_stack(), 1);
    }

    #[test]
    fn test_fuel_replenishment_from_blaze_powder_in_slot_4() {
        let mut stand = BrewingStandBlockEntity::new();
        assert_eq!(stand.fuel_charges(), 0);

        stand.set_fuel(Some(item("minecraft:blaze_powder", 2)));

        assert!(stand.replenish_fuel());
        assert_eq!(stand.fuel_charges(), 20);
        assert_eq!(stand.fuel().unwrap().count(), 1);

        assert!(!stand.replenish_fuel());
        assert_eq!(stand.fuel_charges(), 20);
        assert_eq!(stand.fuel().unwrap().count(), 1);

        stand.set_fuel_charges(0);
        assert!(stand.replenish_fuel());
        assert_eq!(stand.fuel_charges(), 20);
        assert!(stand.fuel().is_none());

        stand.set_fuel_charges(0);
        assert!(!stand.replenish_fuel());
        assert_eq!(stand.fuel_charges(), 0);
    }

    #[test]
    fn test_fuel_consumption_on_brew_cycle_start() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.set_fuel_charges(1);
        stand.set_ingredient(Some(item("minecraft:nether_wart", 1)));
        stand
            .set_potion(POTION_SLOT_0, Some(BrewingSlotItem::water_bottle()))
            .unwrap();

        assert_eq!(stand.brew_time(), 0);
        assert_eq!(stand.fuel_charges(), 1);

        assert!(stand.tick());
        assert_eq!(stand.fuel_charges(), 0);
        assert_eq!(stand.brew_time(), 399);

        for expected in (1..=398).rev() {
            assert!(stand.tick());
            assert_eq!(stand.brew_time(), expected);
            assert_eq!(stand.fuel_charges(), 0);
        }

        assert_eq!(stand.brew_time(), 1);
        assert!(stand.tick());
        assert_eq!(stand.brew_time(), 0);
        let bottle = stand.potion_slot(POTION_SLOT_0).unwrap();
        assert_eq!(bottle.item_id(), "minecraft:potion");
        assert_eq!(bottle.potion_type(), Some("minecraft:awkward"));
        assert!(bottle.is_awkward_potion());
        assert!(stand.ingredient().is_none());

        stand.set_ingredient(Some(item("minecraft:sugar", 1)));
        assert!(!stand.tick());
        assert_eq!(stand.brew_time(), 0);
    }

    #[test]
    fn test_auto_replenish_fuel_when_starting_brew() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.set_fuel(Some(item("minecraft:blaze_powder", 1)));
        stand.set_ingredient(Some(item("minecraft:nether_wart", 1)));
        stand
            .set_potion(POTION_SLOT_0, Some(BrewingSlotItem::water_bottle()))
            .unwrap();

        assert!(stand.tick());
        assert_eq!(stand.fuel_charges(), 19);
        assert_eq!(stand.brew_time(), 399);
        assert!(stand.fuel().is_none());
    }

    #[test]
    fn test_no_fuel_blocks_brewing() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.set_ingredient(Some(item("minecraft:nether_wart", 1)));
        stand
            .set_potion(POTION_SLOT_0, Some(BrewingSlotItem::water_bottle()))
            .unwrap();

        assert!(!stand.tick());
        assert_eq!(stand.brew_time(), 0);
        assert!(stand.potion_slot(POTION_SLOT_0).unwrap().is_water_bottle());
        assert_eq!(stand.ingredient().unwrap().count(), 1);
    }

    #[test]
    fn test_interrupted_brew_resets_brew_time() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.set_fuel_charges(5);
        stand.set_ingredient(Some(item("minecraft:nether_wart", 1)));
        stand
            .set_potion(POTION_SLOT_0, Some(BrewingSlotItem::water_bottle()))
            .unwrap();

        stand.tick();
        assert_eq!(stand.brew_time(), 399);
        assert_eq!(stand.fuel_charges(), 4);

        stand.set_ingredient(None);
        stand.tick();

        assert_eq!(stand.brew_time(), 0);
        assert_eq!(stand.fuel_charges(), 4);
        assert!(stand.potion_slot(POTION_SLOT_0).unwrap().is_water_bottle());
    }

    #[test]
    fn test_multi_bottle_simultaneous_brewing() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.set_fuel_charges(5);
        for slot in POTION_SLOTS {
            stand
                .set_potion(slot, Some(BrewingSlotItem::water_bottle()))
                .unwrap();
        }
        stand.set_ingredient(Some(item("minecraft:nether_wart", 1)));

        stand.set_brew_time(1);
        stand.tick();

        assert_eq!(stand.brew_time(), 0);
        assert!(stand.ingredient().is_none());

        for i in POTION_SLOTS {
            let stack = stand.potion_slot(i).expect("bottle transformed");
            assert!(stack.is_awkward_potion());
            assert_eq!(stack.item_id(), "minecraft:potion");
            assert_eq!(stack.potion_type(), Some("minecraft:awkward"));
            assert_eq!(stack.count(), 1);
        }
    }

    #[test]
    fn test_multi_bottle_partial_matching() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.set_fuel_charges(5);
        stand
            .set_potion(POTION_SLOT_0, Some(BrewingSlotItem::awkward_potion()))
            .unwrap();
        stand
            .set_potion(POTION_SLOT_1, Some(BrewingSlotItem::water_bottle()))
            .unwrap();
        stand
            .set_potion(POTION_SLOT_2, Some(BrewingSlotItem::awkward_potion()))
            .unwrap();
        stand.set_ingredient(Some(item("minecraft:glistering_melon_slice", 1)));

        stand.set_brew_time(1);
        stand.tick();

        assert_eq!(stand.brew_time(), 0);
        assert_eq!(
            stand.potion_slot(POTION_SLOT_0).unwrap().potion_type(),
            Some("minecraft:healing")
        );
        assert!(stand.potion_slot(POTION_SLOT_1).unwrap().is_water_bottle());
        assert_eq!(
            stand.potion_slot(POTION_SLOT_2).unwrap().potion_type(),
            Some("minecraft:healing")
        );
    }

    #[test]
    fn test_vanilla_base_potion_recipes() {
        let recipes = [
            (
                "minecraft:nether_wart",
                "minecraft:water",
                "minecraft:awkward",
            ),
            (
                "minecraft:glistering_melon_slice",
                "minecraft:awkward",
                "minecraft:healing",
            ),
            (
                "minecraft:sugar",
                "minecraft:awkward",
                "minecraft:swiftness",
            ),
            (
                "minecraft:blaze_powder",
                "minecraft:awkward",
                "minecraft:strength",
            ),
            (
                "minecraft:magma_cream",
                "minecraft:awkward",
                "minecraft:fire_resistance",
            ),
        ];

        let reg = BrewingRecipeRegistry::new();
        for (ing, input, expected_out) in recipes {
            assert!(
                reg.has_recipe(ing, input),
                "missing recipe for {ing} + {input}"
            );
            let out = reg
                .find_output_potion_type(ing, input)
                .expect("output exists");
            assert_eq!(out, expected_out);

            let mut stand = BrewingStandBlockEntity::new();
            stand.set_fuel_charges(5);
            stand.set_ingredient(Some(item(ing, 1)));
            stand
                .set_potion(POTION_SLOT_0, Some(potion(input)))
                .unwrap();
            stand.set_brew_time(1);
            stand.tick();

            let result = stand.potion_slot(POTION_SLOT_0).expect("transformed");
            assert_eq!(result.item_id(), "minecraft:potion");
            assert_eq!(result.potion_type(), Some(expected_out));
        }

        // Non-existent alias ingredient IDs must not match
        assert!(!reg.has_recipe("minecraft:glistering_melon", "minecraft:awkward"));
        assert!(!reg.has_recipe("minecraft:redstone_dust", "minecraft:swiftness"));
        assert!(!reg.has_recipe("minecraft:glowstone", "minecraft:healing"));
    }

    #[test]
    fn test_redstone_duration_modifiers() {
        let reg = BrewingRecipeRegistry::new();

        let extendables = [
            ("minecraft:swiftness", "minecraft:long_swiftness"),
            ("minecraft:strength", "minecraft:long_strength"),
            (
                "minecraft:fire_resistance",
                "minecraft:long_fire_resistance",
            ),
            ("minecraft:slowness", "minecraft:long_slowness"),
            ("minecraft:weakness", "minecraft:long_weakness"),
        ];

        for (input, expected) in extendables {
            assert!(
                reg.has_recipe("minecraft:redstone", input),
                "redstone must extend {input}"
            );
            assert_eq!(
                reg.find_output_potion_type("minecraft:redstone", input)
                    .as_deref(),
                Some(expected)
            );

            let mut stand = BrewingStandBlockEntity::new();
            stand.set_fuel_charges(5);
            stand.set_ingredient(Some(item("minecraft:redstone", 1)));
            stand
                .set_potion(POTION_SLOT_0, Some(potion(input)))
                .unwrap();
            stand.set_brew_time(1);
            stand.tick();

            let res = stand.potion_slot(POTION_SLOT_0).unwrap();
            assert_eq!(res.potion_type(), Some(expected));
            assert_eq!(
                res.potion_descriptor().unwrap().duration(),
                PotionDuration::Long
            );
        }

        assert!(!reg.has_recipe("minecraft:redstone", "minecraft:healing"));
        assert!(!reg.has_recipe("minecraft:redstone", "minecraft:harming"));
        assert!(!reg.has_recipe("minecraft:redstone", "minecraft:awkward"));
    }

    #[test]
    fn test_glowstone_potency_modifiers() {
        let reg = BrewingRecipeRegistry::new();

        let upgradeables = [
            ("minecraft:healing", "minecraft:strong_healing"),
            ("minecraft:swiftness", "minecraft:strong_swiftness"),
            ("minecraft:strength", "minecraft:strong_strength"),
            ("minecraft:harming", "minecraft:strong_harming"),
            ("minecraft:slowness", "minecraft:strong_slowness"),
        ];

        for (input, expected) in upgradeables {
            assert!(
                reg.has_recipe("minecraft:glowstone_dust", input),
                "glowstone must upgrade {input}"
            );
            assert_eq!(
                reg.find_output_potion_type("minecraft:glowstone_dust", input)
                    .as_deref(),
                Some(expected)
            );

            let mut stand = BrewingStandBlockEntity::new();
            stand.set_fuel_charges(5);
            stand.set_ingredient(Some(item("minecraft:glowstone_dust", 1)));
            stand
                .set_potion(POTION_SLOT_0, Some(potion(input)))
                .unwrap();
            stand.set_brew_time(1);
            stand.tick();

            let res = stand.potion_slot(POTION_SLOT_0).unwrap();
            assert_eq!(res.potion_type(), Some(expected));
            assert_eq!(res.potion_descriptor().unwrap().tier(), PotionTier::Strong);
        }

        assert!(!reg.has_recipe("minecraft:glowstone_dust", "minecraft:fire_resistance"));
        assert!(!reg.has_recipe("minecraft:glowstone_dust", "minecraft:awkward"));
    }

    #[test]
    fn test_fermented_spider_eye_inversions() {
        let reg = BrewingRecipeRegistry::new();

        let inversions = [
            ("minecraft:healing", "minecraft:harming"),
            ("minecraft:strong_healing", "minecraft:strong_harming"),
            ("minecraft:swiftness", "minecraft:slowness"),
            ("minecraft:long_swiftness", "minecraft:long_slowness"),
        ];

        for (input, expected) in inversions {
            assert!(
                reg.has_recipe("minecraft:fermented_spider_eye", input),
                "spider eye must invert {input}"
            );
            assert_eq!(
                reg.find_output_potion_type("minecraft:fermented_spider_eye", input)
                    .as_deref(),
                Some(expected)
            );

            let mut stand = BrewingStandBlockEntity::new();
            stand.set_fuel_charges(5);
            stand.set_ingredient(Some(item("minecraft:fermented_spider_eye", 1)));
            stand
                .set_potion(POTION_SLOT_0, Some(potion(input)))
                .unwrap();
            stand.set_brew_time(1);
            stand.tick();

            let res = stand.potion_slot(POTION_SLOT_0).unwrap();
            assert_eq!(res.potion_type(), Some(expected));
        }
    }

    #[test]
    fn test_splash_and_lingering_potion_brewing_preserves_potion_type() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.set_fuel_charges(5);
        stand
            .set_potion(POTION_SLOT_0, Some(potion("minecraft:healing")))
            .unwrap();
        stand.set_ingredient(Some(item("minecraft:gunpowder", 1)));
        stand.set_brew_time(1);
        stand.tick();

        let splash = stand.potion_slot(POTION_SLOT_0).unwrap();
        assert_eq!(splash.item_id(), "minecraft:splash_potion");
        assert_eq!(splash.potion_type(), Some("minecraft:healing"));

        stand.set_ingredient(Some(item("minecraft:dragon_breath", 1)));
        stand.set_brew_time(1);
        stand.tick();

        let lingering = stand.potion_slot(POTION_SLOT_0).unwrap();
        assert_eq!(lingering.item_id(), "minecraft:lingering_potion");
        assert_eq!(lingering.potion_type(), Some("minecraft:healing"));

        // Modifiers preserve container item (splash/lingering)
        stand.set_ingredient(Some(item("minecraft:glowstone_dust", 1)));
        stand.set_brew_time(1);
        stand.tick();
        let strong_lingering = stand.potion_slot(POTION_SLOT_0).unwrap();
        assert_eq!(strong_lingering.item_id(), "minecraft:lingering_potion");
        assert_eq!(
            strong_lingering.potion_type(),
            Some("minecraft:strong_healing")
        );
    }

    #[test]
    fn test_container_data_packet_encoding_and_decoding() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.set_brew_time(250);
        stand.set_fuel_charges(15);
        let window_id = 3;

        let pkt0 = stand.create_property_packet(window_id, BrewingStandProperty::BrewTime);
        assert_eq!(pkt0.property(), 0);
        assert_eq!(pkt0.value(), 250);
        let encoded0 = pkt0.encode_packet();
        assert_eq!(
            encoded0,
            vec![SET_CONTAINER_DATA_PACKET_ID as u8, 3, 0, 0, 0x00, 0xfa]
        );
        let decoded0 = SetContainerDataPacket::decode_packet(&encoded0).unwrap();
        assert_eq!(decoded0, pkt0);

        let pkt1 = stand.create_property_packet(window_id, BrewingStandProperty::FuelCharges);
        assert_eq!(pkt1.property(), 1);
        assert_eq!(pkt1.value(), 15);
        let encoded1 = pkt1.encode_packet();
        assert_eq!(encoded1, vec![0x13, 3, 0, 1, 0x00, 0x0f]);
        let decoded1 = SetContainerDataPacket::decode_packet(&encoded1).unwrap();
        assert_eq!(decoded1, pkt1);

        let framed0 = pkt0.encode_framed();
        assert_eq!(framed0, vec![6, 0x13, 3, 0, 0, 0x00, 0xfa]);
        let decoded_framed = SetContainerDataPacket::decode_framed(&framed0).unwrap();
        assert_eq!(decoded_framed, pkt0);

        let all = stand.encode_all_container_data(window_id);
        assert_eq!(all[0], encoded0);
        assert_eq!(all[1], encoded1);
    }

    #[test]
    fn test_custom_recipe_registration() {
        let mut reg = BrewingRecipeRegistry::new();
        reg.register_recipe("minecraft:apple", "minecraft:water", "minecraft:swiftness")
            .unwrap();

        let mut stand = BrewingStandBlockEntity::with_registry(reg);
        stand.set_fuel_charges(1);
        stand.set_ingredient(Some(item("minecraft:apple", 1)));
        stand
            .set_potion(POTION_SLOT_0, Some(BrewingSlotItem::water_bottle()))
            .unwrap();

        stand.set_brew_time(1);
        stand.tick();

        assert_eq!(stand.brew_time(), 0);
        let res = stand.potion_slot(POTION_SLOT_0).unwrap();
        assert!(res.is_potion_of(PotionEffect::Swiftness));
        assert_eq!(res.potion_type(), Some("minecraft:swiftness"));
    }

    #[test]
    fn test_slot_bounds_and_accessors() {
        let mut stand = BrewingStandBlockEntity::new();
        assert!(
            stand
                .set_slot(0, Some(BrewingSlotItem::water_bottle()))
                .is_ok()
        );
        assert!(
            stand
                .set_slot(4, Some(BrewingSlotItem::blaze_powder(10).unwrap()))
                .is_ok()
        );
        assert!(
            stand
                .set_slot(5, Some(BrewingSlotItem::water_bottle()))
                .is_err()
        );

        assert!(
            stand
                .set_potion(0, Some(BrewingSlotItem::water_bottle()))
                .is_ok()
        );
        assert!(
            stand
                .set_potion(3, Some(BrewingSlotItem::water_bottle()))
                .is_err()
        );

        assert!(stand.potion_slot(0).is_some());
        assert!(stand.potion_slot(3).is_none());
    }

    #[test]
    fn test_bedrock_only_corruptions_are_not_java_recipes() {
        let registry = BrewingRecipeRegistry::new();
        let eye = "minecraft:fermented_spider_eye";
        for input in [
            "minecraft:awkward",
            "minecraft:strength",
            "minecraft:long_strength",
            "minecraft:fire_resistance",
            "minecraft:long_fire_resistance",
            "minecraft:strong_swiftness",
        ] {
            assert_eq!(
                registry.find_output_potion_type(eye, input),
                None,
                "{input}"
            );
        }
        assert_eq!(
            registry
                .find_output_potion_type(eye, "minecraft:water")
                .as_deref(),
            Some("minecraft:weakness")
        );
    }

    #[test]
    fn test_changing_the_ingredient_cancels_the_running_cycle() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.set_fuel_charges(2);
        stand
            .set_potion(0, Some(BrewingSlotItem::awkward_potion()))
            .unwrap();
        stand.set_ingredient(Some(item("minecraft:sugar", 1)));
        stand.tick();
        assert_eq!(stand.brew_time(), BREW_TIME_MAX - 1);

        stand.set_ingredient(Some(item("minecraft:magma_cream", 1)));
        assert!(stand.tick());
        assert_eq!(stand.brew_time(), 0);
        assert_eq!(stand.fuel_charges(), 1);

        stand.tick();
        assert_eq!(stand.brew_time(), BREW_TIME_MAX - 1);
        assert_eq!(stand.fuel_charges(), 0);
    }

    #[test]
    fn test_tick_reports_fuel_replenished_while_idle() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.set_fuel(Some(BrewingSlotItem::blaze_powder(1).unwrap()));
        assert!(stand.tick());
        assert_eq!(stand.fuel_charges(), MAX_FUEL_CHARGES);
        assert!(!stand.tick());
    }
}
