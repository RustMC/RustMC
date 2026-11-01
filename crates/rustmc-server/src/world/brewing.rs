//! Milestone M4: Brewing Stand State Machine, Potion Recipes, and Container Synchronization.
//!
//! Provides authoritative brewing stand block entity state progression, fuel consumption
//! and replenishment, canonical vanilla potion recipe resolution and modifiers, multi-bottle
//! simultaneous brewing, and Java Edition 26.3 `SetContainerData` (packet ID `0x14`) network encoding.

use crate::discovery_java::{frame, put_varint, varint};
use std::fmt;

/// Potion bottle slot 0 (left slot).
pub const POTION_SLOT_0: usize = 0;
/// Potion bottle slot 1 (middle slot).
pub const POTION_SLOT_1: usize = 1;
/// Potion bottle slot 2 (right slot).
pub const POTION_SLOT_2: usize = 2;
/// Range of potion bottle slots (0..=2).
pub const POTION_SLOTS: std::ops::RangeInclusive<usize> = 0..=2;

/// Ingredient slot index (top center slot).
pub const INGREDIENT_SLOT: usize = 3;

/// Blaze powder fuel slot index (top left slot).
pub const FUEL_SLOT: usize = 4;

/// Total number of inventory slots in a brewing stand.
pub const TOTAL_SLOTS: usize = 5;

/// Total brew cycle duration in game ticks (400 ticks = 20 seconds).
pub const BREW_TIME_MAX: u16 = 400;

/// Maximum blaze powder fuel charges a brewing stand can hold.
pub const MAX_FUEL_CHARGES: u8 = 20;

/// Number of fuel charges replenished per Blaze Powder consumed.
pub const FUEL_PER_BLAZE_POWDER: u8 = 20;

/// Default maximum stack size for standard inventory items.
pub const DEFAULT_MAX_STACK: u16 = 64;

/// Default maximum stack size for potion bottles (unstackable in vanilla).
pub const POTION_MAX_STACK: u16 = 1;

/// Java 26.3 Clientbound SetContainerData packet ID in Play state.
pub const SET_CONTAINER_DATA_PACKET_ID: u32 = 0x14;

/// Strips optional `minecraft:` namespace prefix for canonical item matching.
#[inline]
#[must_use]
pub fn normalize_item_id(item: &str) -> &str {
    item.strip_prefix("minecraft:").unwrap_or(item)
}

/// Normalizes ingredient identifiers to handle common aliases (e.g. `glistering_melon` vs `glistering_melon_slice`).
#[must_use]
pub fn normalize_ingredient_id(item: &str) -> &str {
    let s = normalize_item_id(item);
    match s {
        "glistering_melon" | "glistering_melon_slice" => "glistering_melon_slice",
        "redstone" | "redstone_dust" => "redstone",
        "glowstone" | "glowstone_dust" => "glowstone_dust",
        _ => s,
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

/// Structured potion descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Potion {
    pub effect: PotionEffect,
    pub tier: PotionTier,
    pub duration: PotionDuration,
}

impl Potion {
    pub const fn water() -> Self {
        Self {
            effect: PotionEffect::Water,
            tier: PotionTier::Normal,
            duration: PotionDuration::Normal,
        }
    }

    pub const fn awkward() -> Self {
        Self {
            effect: PotionEffect::Awkward,
            tier: PotionTier::Normal,
            duration: PotionDuration::Normal,
        }
    }

    pub const fn new(effect: PotionEffect, tier: PotionTier, duration: PotionDuration) -> Self {
        Self {
            effect,
            tier,
            duration,
        }
    }

    /// Attempts to parse a potion from a string identifier (case-insensitive, optional `minecraft:` prefix).
    #[must_use]
    pub fn from_item_id(item: &str) -> Option<Self> {
        let clean = normalize_item_id(item)
            .to_ascii_lowercase()
            .replace(' ', "_");
        match clean.as_str() {
            "water_bottle" | "water" => Some(Self::water()),
            "awkward_potion" | "awkward" => Some(Self::awkward()),

            // Healing
            "potion_of_healing" | "healing" | "healing_potion" => Some(Self::new(
                PotionEffect::Healing,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "strong_potion_of_healing"
            | "potion_of_strong_healing"
            | "potion_of_healing_ii"
            | "potion_of_healing_2"
            | "healing_ii"
            | "healing_2" => Some(Self::new(
                PotionEffect::Healing,
                PotionTier::Strong,
                PotionDuration::Normal,
            )),

            // Harming
            "potion_of_harming" | "harming" | "harming_potion" => Some(Self::new(
                PotionEffect::Harming,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "strong_potion_of_harming"
            | "potion_of_strong_harming"
            | "potion_of_harming_ii"
            | "potion_of_harming_2"
            | "harming_ii"
            | "harming_2" => Some(Self::new(
                PotionEffect::Harming,
                PotionTier::Strong,
                PotionDuration::Normal,
            )),

            // Swiftness / Speed
            "potion_of_swiftness" | "swiftness" | "speed" | "potion_of_speed" => Some(Self::new(
                PotionEffect::Swiftness,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "long_potion_of_swiftness"
            | "potion_of_long_swiftness"
            | "long_swiftness"
            | "long_speed" => Some(Self::new(
                PotionEffect::Swiftness,
                PotionTier::Normal,
                PotionDuration::Long,
            )),
            "strong_potion_of_swiftness"
            | "potion_of_strong_swiftness"
            | "potion_of_swiftness_ii"
            | "potion_of_swiftness_2"
            | "strong_swiftness"
            | "strong_speed" => Some(Self::new(
                PotionEffect::Swiftness,
                PotionTier::Strong,
                PotionDuration::Normal,
            )),

            // Slowness
            "potion_of_slowness" | "slowness" => Some(Self::new(
                PotionEffect::Slowness,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "long_potion_of_slowness" | "potion_of_long_slowness" | "long_slowness" => {
                Some(Self::new(
                    PotionEffect::Slowness,
                    PotionTier::Normal,
                    PotionDuration::Long,
                ))
            }
            "strong_potion_of_slowness"
            | "potion_of_strong_slowness"
            | "potion_of_slowness_ii"
            | "potion_of_slowness_2"
            | "strong_slowness" => Some(Self::new(
                PotionEffect::Slowness,
                PotionTier::Strong,
                PotionDuration::Normal,
            )),

            // Strength
            "potion_of_strength" | "strength" => Some(Self::new(
                PotionEffect::Strength,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "long_potion_of_strength" | "potion_of_long_strength" | "long_strength" => {
                Some(Self::new(
                    PotionEffect::Strength,
                    PotionTier::Normal,
                    PotionDuration::Long,
                ))
            }
            "strong_potion_of_strength"
            | "potion_of_strong_strength"
            | "potion_of_strength_ii"
            | "potion_of_strength_2"
            | "strong_strength" => Some(Self::new(
                PotionEffect::Strength,
                PotionTier::Strong,
                PotionDuration::Normal,
            )),

            // Fire Resistance
            "potion_of_fire_resistance" | "fire_resistance" => Some(Self::new(
                PotionEffect::FireResistance,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "long_potion_of_fire_resistance"
            | "potion_of_long_fire_resistance"
            | "long_fire_resistance" => Some(Self::new(
                PotionEffect::FireResistance,
                PotionTier::Normal,
                PotionDuration::Long,
            )),

            // Weakness
            "potion_of_weakness" | "weakness" => Some(Self::new(
                PotionEffect::Weakness,
                PotionTier::Normal,
                PotionDuration::Normal,
            )),
            "long_potion_of_weakness" | "potion_of_long_weakness" | "long_weakness" => {
                Some(Self::new(
                    PotionEffect::Weakness,
                    PotionTier::Normal,
                    PotionDuration::Long,
                ))
            }

            _ => None,
        }
    }

    /// Returns the canonical namespaced item identifier for this potion.
    #[must_use]
    pub fn canonical_item_id(&self) -> &'static str {
        match (self.effect, self.tier, self.duration) {
            (PotionEffect::Water, _, _) => "minecraft:water_bottle",
            (PotionEffect::Awkward, _, _) => "minecraft:awkward_potion",

            (PotionEffect::Healing, PotionTier::Normal, _) => "minecraft:potion_of_healing",
            (PotionEffect::Healing, PotionTier::Strong, _) => "minecraft:strong_potion_of_healing",

            (PotionEffect::Harming, PotionTier::Normal, _) => "minecraft:potion_of_harming",
            (PotionEffect::Harming, PotionTier::Strong, _) => "minecraft:strong_potion_of_harming",

            (PotionEffect::Swiftness, PotionTier::Normal, PotionDuration::Normal) => {
                "minecraft:potion_of_swiftness"
            }
            (PotionEffect::Swiftness, PotionTier::Normal, PotionDuration::Long) => {
                "minecraft:long_potion_of_swiftness"
            }
            (PotionEffect::Swiftness, PotionTier::Strong, _) => {
                "minecraft:strong_potion_of_swiftness"
            }

            (PotionEffect::Slowness, PotionTier::Normal, PotionDuration::Normal) => {
                "minecraft:potion_of_slowness"
            }
            (PotionEffect::Slowness, PotionTier::Normal, PotionDuration::Long) => {
                "minecraft:long_potion_of_slowness"
            }
            (PotionEffect::Slowness, PotionTier::Strong, _) => {
                "minecraft:strong_potion_of_slowness"
            }

            (PotionEffect::Strength, PotionTier::Normal, PotionDuration::Normal) => {
                "minecraft:potion_of_strength"
            }
            (PotionEffect::Strength, PotionTier::Normal, PotionDuration::Long) => {
                "minecraft:long_potion_of_strength"
            }
            (PotionEffect::Strength, PotionTier::Strong, _) => {
                "minecraft:strong_potion_of_strength"
            }

            (PotionEffect::FireResistance, _, PotionDuration::Normal) => {
                "minecraft:potion_of_fire_resistance"
            }
            (PotionEffect::FireResistance, _, PotionDuration::Long) => {
                "minecraft:long_potion_of_fire_resistance"
            }

            (PotionEffect::Weakness, _, PotionDuration::Normal) => "minecraft:potion_of_weakness",
            (PotionEffect::Weakness, _, PotionDuration::Long) => {
                "minecraft:long_potion_of_weakness"
            }
        }
    }
}

/// Checks if two item identifiers match canonically, supporting namespaces, case-insensitivity, and potion aliases.
#[must_use]
pub fn items_equal(a: &str, b: &str) -> bool {
    let na = normalize_item_id(a);
    let nb = normalize_item_id(b);
    if na.eq_ignore_ascii_case(nb) {
        return true;
    }
    if let (Some(pa), Some(pb)) = (Potion::from_item_id(a), Potion::from_item_id(b)) {
        return pa == pb;
    }
    false
}

/// Returns the standard default maximum stack size for common items.
#[must_use]
pub fn default_max_stack(item: &str) -> u16 {
    let normalized = normalize_item_id(item);
    if Potion::from_item_id(normalized).is_some()
        || normalized.contains("potion")
        || normalized == "water_bottle"
    {
        POTION_MAX_STACK
    } else {
        DEFAULT_MAX_STACK
    }
}

/// Evaluates dynamic potion modifier transformations for items that might not have explicit static recipe entries.
#[must_use]
pub fn evaluate_potion_modifier(ingredient: &str, input_potion: &str) -> Option<String> {
    let ing = normalize_ingredient_id(ingredient);
    let potion = Potion::from_item_id(input_potion)?;

    match ing {
        // Redstone Dust: extends duration (normal duration -> long duration)
        "redstone" => {
            if potion.duration == PotionDuration::Normal && potion.tier == PotionTier::Normal {
                match potion.effect {
                    PotionEffect::Swiftness
                    | PotionEffect::Strength
                    | PotionEffect::FireResistance
                    | PotionEffect::Slowness
                    | PotionEffect::Weakness => Some(
                        Potion::new(potion.effect, PotionTier::Normal, PotionDuration::Long)
                            .canonical_item_id()
                            .to_string(),
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
                            .canonical_item_id()
                            .to_string(),
                    ),
                    _ => None,
                }
            } else {
                None
            }
        }

        // Fermented Spider Eye: inverts effect
        "fermented_spider_eye" => match potion.effect {
            PotionEffect::Healing => Some(
                Potion::new(PotionEffect::Harming, potion.tier, potion.duration)
                    .canonical_item_id()
                    .to_string(),
            ),
            PotionEffect::Swiftness => Some(
                Potion::new(PotionEffect::Slowness, potion.tier, potion.duration)
                    .canonical_item_id()
                    .to_string(),
            ),
            PotionEffect::FireResistance => Some(
                Potion::new(PotionEffect::Slowness, PotionTier::Normal, potion.duration)
                    .canonical_item_id()
                    .to_string(),
            ),
            PotionEffect::Water | PotionEffect::Awkward | PotionEffect::Strength => Some(
                Potion::new(
                    PotionEffect::Weakness,
                    PotionTier::Normal,
                    PotionDuration::Normal,
                )
                .canonical_item_id()
                .to_string(),
            ),
            _ => None,
        },

        _ => None,
    }
}

/// An item stack occupying a brewing stand slot or transferred during window interactions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemStack {
    pub item: String,
    pub count: u16,
    pub max_stack: u16,
}

impl ItemStack {
    /// Constructs a new `ItemStack` with default maximum stack bounds.
    pub fn new(item: impl Into<String>, count: u16) -> Self {
        let item = item.into();
        let max_stack = default_max_stack(&item);
        Self {
            item,
            count,
            max_stack,
        }
    }

    /// Constructs a new `ItemStack` with explicit `max_stack` limits.
    pub fn with_max_stack(item: impl Into<String>, count: u16, max_stack: u16) -> Self {
        Self {
            item: item.into(),
            count,
            max_stack: if max_stack == 0 {
                DEFAULT_MAX_STACK
            } else {
                max_stack
            },
        }
    }

    /// Helper constructor for a water bottle.
    pub fn water_bottle() -> Self {
        Self::new("minecraft:water_bottle", 1)
    }

    /// Helper constructor for an awkward potion.
    pub fn awkward_potion() -> Self {
        Self::new("minecraft:awkward_potion", 1)
    }

    /// Helper constructor for blaze powder fuel.
    pub fn blaze_powder(count: u16) -> Self {
        Self::new("minecraft:blaze_powder", count)
    }

    /// Helper constructor from a structured `Potion`.
    pub fn from_potion(potion: Potion) -> Self {
        Self::new(potion.canonical_item_id(), 1)
    }

    #[inline]
    #[must_use]
    pub fn item(&self) -> &str {
        &self.item
    }

    #[inline]
    #[must_use]
    pub fn item_id(&self) -> &str {
        &self.item
    }

    #[inline]
    #[must_use]
    pub fn count(&self) -> u16 {
        self.count
    }

    #[inline]
    #[must_use]
    pub fn max_stack(&self) -> u16 {
        self.max_stack
    }

    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    #[inline]
    #[must_use]
    pub fn potion(&self) -> Option<Potion> {
        Potion::from_item_id(&self.item)
    }

    #[inline]
    #[must_use]
    pub fn is_potion(&self) -> bool {
        self.potion().is_some()
    }

    #[inline]
    #[must_use]
    pub fn is_water_bottle(&self) -> bool {
        matches!(self.potion(), Some(p) if p.effect == PotionEffect::Water)
    }

    #[inline]
    #[must_use]
    pub fn is_awkward_potion(&self) -> bool {
        matches!(self.potion(), Some(p) if p.effect == PotionEffect::Awkward)
    }

    #[inline]
    #[must_use]
    pub fn is_potion_of(&self, effect: PotionEffect) -> bool {
        matches!(self.potion(), Some(p) if p.effect == effect)
    }

    #[inline]
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        items_equal(&self.item, &other.item)
    }

    #[inline]
    #[must_use]
    pub fn matches_item(&self, item_name: &str) -> bool {
        items_equal(&self.item, item_name)
    }

    /// Attempts to add count up to `max_stack`. Returns excess overflow count.
    pub fn add(&mut self, amount: u16) -> u16 {
        let space = self.max_stack.saturating_sub(self.count);
        let accepted = space.min(amount);
        self.count += accepted;
        amount - accepted
    }
}

impl<S: Into<String>> From<(S, u16)> for ItemStack {
    fn from((item, count): (S, u16)) -> Self {
        Self::new(item, count)
    }
}

/// A brewing recipe mapping an ingredient item and input potion item to an output potion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrewingRecipe {
    pub ingredient: String,
    pub input_potion: String,
    pub output_potion: String,
}

impl BrewingRecipe {
    /// Constructs a new brewing recipe.
    pub fn new(
        ingredient: impl Into<String>,
        input_potion: impl Into<String>,
        output_potion: impl Into<String>,
    ) -> Self {
        Self {
            ingredient: ingredient.into(),
            input_potion: input_potion.into(),
            output_potion: output_potion.into(),
        }
    }

    /// Checks if this recipe matches the given ingredient and bottle stack identifiers.
    #[must_use]
    pub fn matches(&self, ingredient: &str, input_potion: &str) -> bool {
        let ing_a = normalize_ingredient_id(&self.ingredient);
        let ing_b = normalize_ingredient_id(ingredient);
        if !ing_a.eq_ignore_ascii_case(ing_b) {
            return false;
        }
        items_equal(&self.input_potion, input_potion)
    }
}

/// Registry of brewing recipes for the brewing stand state machine.
#[derive(Debug, Clone)]
pub struct BrewingRecipeRegistry {
    recipes: Vec<BrewingRecipe>,
}

impl BrewingRecipeRegistry {
    /// Creates a registry initialized with all vanilla Minecraft brewing recipes.
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

    /// Registers a brewing recipe by component string names.
    pub fn register_recipe(
        &mut self,
        ingredient: impl Into<String>,
        input: impl Into<String>,
        output: impl Into<String>,
    ) {
        self.register(BrewingRecipe::new(ingredient, input, output));
    }

    /// Finds a matching recipe in the registry.
    #[must_use]
    pub fn find_recipe(&self, ingredient: &str, input_potion: &str) -> Option<&BrewingRecipe> {
        self.recipes
            .iter()
            .rev()
            .find(|r| r.matches(ingredient, input_potion))
    }

    /// Evaluates the output potion item identifier for a given ingredient and input potion.
    #[must_use]
    pub fn find_output(&self, ingredient: &str, input_potion: &str) -> Option<String> {
        if let Some(r) = self.find_recipe(ingredient, input_potion) {
            return Some(r.output_potion.clone());
        }
        evaluate_potion_modifier(ingredient, input_potion)
    }

    /// Checks whether an ingredient and input potion form a valid recipe.
    #[must_use]
    pub fn has_recipe(&self, ingredient: &str, input_potion: &str) -> bool {
        self.find_output(ingredient, input_potion).is_some()
    }

    /// Populates standard vanilla potion brewing recipes.
    fn register_defaults(&mut self) {
        // Base potion recipes
        self.register_recipe(
            "minecraft:nether_wart",
            "minecraft:water_bottle",
            "minecraft:awkward_potion",
        );
        self.register_recipe(
            "minecraft:glistering_melon_slice",
            "minecraft:awkward_potion",
            "minecraft:potion_of_healing",
        );
        self.register_recipe(
            "minecraft:sugar",
            "minecraft:awkward_potion",
            "minecraft:potion_of_swiftness",
        );
        self.register_recipe(
            "minecraft:blaze_powder",
            "minecraft:awkward_potion",
            "minecraft:potion_of_strength",
        );
        self.register_recipe(
            "minecraft:magma_cream",
            "minecraft:awkward_potion",
            "minecraft:potion_of_fire_resistance",
        );

        // Redstone Dust duration modifier (extends duration)
        self.register_recipe(
            "minecraft:redstone",
            "minecraft:potion_of_swiftness",
            "minecraft:long_potion_of_swiftness",
        );
        self.register_recipe(
            "minecraft:redstone",
            "minecraft:potion_of_strength",
            "minecraft:long_potion_of_strength",
        );
        self.register_recipe(
            "minecraft:redstone",
            "minecraft:potion_of_fire_resistance",
            "minecraft:long_potion_of_fire_resistance",
        );
        self.register_recipe(
            "minecraft:redstone",
            "minecraft:potion_of_slowness",
            "minecraft:long_potion_of_slowness",
        );
        self.register_recipe(
            "minecraft:redstone",
            "minecraft:potion_of_weakness",
            "minecraft:long_potion_of_weakness",
        );

        // Glowstone Dust potency modifier (tier II)
        self.register_recipe(
            "minecraft:glowstone_dust",
            "minecraft:potion_of_healing",
            "minecraft:strong_potion_of_healing",
        );
        self.register_recipe(
            "minecraft:glowstone_dust",
            "minecraft:potion_of_swiftness",
            "minecraft:strong_potion_of_swiftness",
        );
        self.register_recipe(
            "minecraft:glowstone_dust",
            "minecraft:potion_of_strength",
            "minecraft:strong_potion_of_strength",
        );
        self.register_recipe(
            "minecraft:glowstone_dust",
            "minecraft:potion_of_harming",
            "minecraft:strong_potion_of_harming",
        );
        self.register_recipe(
            "minecraft:glowstone_dust",
            "minecraft:potion_of_slowness",
            "minecraft:strong_potion_of_slowness",
        );

        // Fermented Spider Eye effect inversion
        self.register_recipe(
            "minecraft:fermented_spider_eye",
            "minecraft:potion_of_healing",
            "minecraft:potion_of_harming",
        );
        self.register_recipe(
            "minecraft:fermented_spider_eye",
            "minecraft:strong_potion_of_healing",
            "minecraft:strong_potion_of_harming",
        );
        self.register_recipe(
            "minecraft:fermented_spider_eye",
            "minecraft:potion_of_swiftness",
            "minecraft:potion_of_slowness",
        );
        self.register_recipe(
            "minecraft:fermented_spider_eye",
            "minecraft:long_potion_of_swiftness",
            "minecraft:long_potion_of_slowness",
        );
        self.register_recipe(
            "minecraft:fermented_spider_eye",
            "minecraft:strong_potion_of_swiftness",
            "minecraft:strong_potion_of_slowness",
        );
        self.register_recipe(
            "minecraft:fermented_spider_eye",
            "minecraft:potion_of_fire_resistance",
            "minecraft:potion_of_slowness",
        );
        self.register_recipe(
            "minecraft:fermented_spider_eye",
            "minecraft:long_potion_of_fire_resistance",
            "minecraft:long_potion_of_slowness",
        );
        self.register_recipe(
            "minecraft:fermented_spider_eye",
            "minecraft:water_bottle",
            "minecraft:potion_of_weakness",
        );
        self.register_recipe(
            "minecraft:fermented_spider_eye",
            "minecraft:awkward_potion",
            "minecraft:potion_of_weakness",
        );
        self.register_recipe(
            "minecraft:fermented_spider_eye",
            "minecraft:potion_of_strength",
            "minecraft:potion_of_weakness",
        );
    }
}

impl Default for BrewingRecipeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Java 26.3 brewing stand container data property indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum BrewingStandProperty {
    /// Property 0: Remaining brew time ticks (400 down to 0).
    BrewTime = 0,
    /// Property 1: Remaining fuel charges (0 up to 20).
    FuelCharges = 1,
}

impl BrewingStandProperty {
    #[inline]
    #[must_use]
    pub const fn index(self) -> u16 {
        self as u16
    }

    #[must_use]
    pub const fn from_index(index: u16) -> Option<Self> {
        match index {
            0 => Some(Self::BrewTime),
            1 => Some(Self::FuelCharges),
            _ => None,
        }
    }
}

/// Errors related to brewing stand operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrewingError {
    InvalidSlotIndex(usize),
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
        }
    }
}

impl std::error::Error for BrewingError {}

/// Authoritative state machine for a Minecraft brewing stand block entity.
#[derive(Debug, Clone)]
pub struct BrewingStandBlockEntity {
    /// 5 slots: slots 0..=2 = potions, slot 3 = ingredient, slot 4 = blaze powder fuel.
    pub slots: [Option<ItemStack>; TOTAL_SLOTS],
    /// Countdown brew timer from 400 ticks (20s) down to 0.
    pub brew_time: u16,
    /// Remaining fuel charges (0..=20).
    pub fuel_charges: u8,
    /// Recipe registry for resolving brewing transitions.
    pub recipe_registry: BrewingRecipeRegistry,
}

impl BrewingStandBlockEntity {
    /// Creates a new empty brewing stand with 0 fuel and default recipes.
    pub fn new() -> Self {
        Self {
            slots: [None, None, None, None, None],
            brew_time: 0,
            fuel_charges: 0,
            recipe_registry: BrewingRecipeRegistry::new(),
        }
    }

    /// Creates a brewing stand with a custom recipe registry.
    pub fn with_registry(recipe_registry: BrewingRecipeRegistry) -> Self {
        Self {
            slots: [None, None, None, None, None],
            brew_time: 0,
            fuel_charges: 0,
            recipe_registry,
        }
    }

    /// Returns a reference to the item stack in the given slot, if any.
    #[inline]
    #[must_use]
    pub fn slot(&self, index: usize) -> Option<&ItemStack> {
        self.slots.get(index).and_then(Option::as_ref)
    }

    /// Returns a mutable reference to the item stack in the given slot, if any.
    #[inline]
    pub fn slot_mut(&mut self, index: usize) -> Option<&mut ItemStack> {
        self.slots.get_mut(index).and_then(Option::as_mut)
    }

    /// Sets the item stack in the given slot, validating slot bounds.
    pub fn set_slot(&mut self, index: usize, stack: Option<ItemStack>) -> Result<(), BrewingError> {
        if index >= TOTAL_SLOTS {
            return Err(BrewingError::InvalidSlotIndex(index));
        }
        self.slots[index] = stack;
        Ok(())
    }

    /// Returns a reference to the potion stack in one of the 3 potion slots (0..=2).
    #[inline]
    #[must_use]
    pub fn potion_slot(&self, index: usize) -> Option<&ItemStack> {
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
        stack: Option<ItemStack>,
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
    pub fn ingredient(&self) -> Option<&ItemStack> {
        self.slot(INGREDIENT_SLOT)
    }

    /// Sets the ingredient in slot 3.
    pub fn set_ingredient(&mut self, stack: Option<ItemStack>) {
        self.slots[INGREDIENT_SLOT] = stack;
    }

    /// Returns a reference to the fuel stack in slot 4.
    #[inline]
    #[must_use]
    pub fn fuel(&self) -> Option<&ItemStack> {
        self.slot(FUEL_SLOT)
    }

    /// Sets the fuel stack in slot 4.
    pub fn set_fuel(&mut self, stack: Option<ItemStack>) {
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

        if let Some(ref mut fuel) = self.slots[FUEL_SLOT]
            && items_equal(&fuel.item, "blaze_powder")
            && fuel.count > 0
        {
            fuel.count -= 1;
            if fuel.count == 0 {
                self.slots[FUEL_SLOT] = None;
            }
            self.fuel_charges = MAX_FUEL_CHARGES;
            return true;
        }

        false
    }

    /// Checks if the current ingredient in slot 3 and at least one bottle in slots 0..=2 form a valid brewing recipe.
    #[must_use]
    pub fn can_brew(&self) -> bool {
        let Some(ref ingredient) = self.slots[INGREDIENT_SLOT] else {
            return false;
        };
        if ingredient.count == 0 {
            return false;
        }

        for slot_idx in POTION_SLOTS {
            if let Some(ref bottle) = self.slots[slot_idx]
                && bottle.count > 0
                && self
                    .recipe_registry
                    .has_recipe(&ingredient.item, &bottle.item)
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
        let Some(ingredient_item) = self.slots[INGREDIENT_SLOT].as_ref().map(|s| s.item.clone())
        else {
            self.brew_time = 0;
            return;
        };

        // Transform all matching potion slots
        for slot_idx in POTION_SLOTS {
            if let Some(ref bottle) = self.slots[slot_idx]
                && let Some(output) = self
                    .recipe_registry
                    .find_output(&ingredient_item, &bottle.item)
            {
                let count = bottle.count;
                self.slots[slot_idx] = Some(ItemStack::new(output, count));
            }
        }

        // Decrement ingredient count by 1
        if let Some(ref mut ing) = self.slots[INGREDIENT_SLOT] {
            ing.count = ing.count.saturating_sub(1);
            if ing.count == 0 {
                self.slots[INGREDIENT_SLOT] = None;
            }
        }

        // Reset brew timer
        self.brew_time = 0;
    }

    /// Advances the brewing stand state machine by one server tick (1/20 second).
    ///
    /// State machine steps:
    /// 1. Replenishes fuel charges from Blaze Powder if empty.
    /// 2. Checks if ingredient and at least one bottle form a valid brewing recipe.
    ///    If no valid recipe exists, any active brewing countdown is cancelled/reset to 0.
    /// 3. If starting a new brew cycle (`brew_time == 0`), consumes 1 fuel charge and initializes `brew_time` to 400.
    /// 4. Decrements `brew_time`.
    /// 5. On reaching 0, completes the brew cycle, transforming potions, decrementing ingredient count, and resetting `brew_time`.
    ///
    /// Returns true if any state change occurred.
    pub fn tick(&mut self) -> bool {
        // Step 1: replenish fuel charges from blaze powder slot if fuel is exhausted
        if self.fuel_charges == 0 {
            self.replenish_fuel();
        }

        // Step 2: verify valid recipe
        let can_brew = self.can_brew();
        if !can_brew {
            if self.brew_time > 0 {
                self.brew_time = 0;
                return true;
            }
            return false;
        }

        // Step 3: consume fuel charge when starting a brew cycle
        if self.brew_time == 0 {
            if self.fuel_charges == 0 {
                return false;
            }
            self.fuel_charges -= 1;
            self.brew_time = BREW_TIME_MAX;
        }

        // Step 4: decrement brew_time
        self.brew_time = self.brew_time.saturating_sub(1);

        // Step 5: on reaching 0, transform potions and decrement ingredient
        if self.brew_time == 0 {
            self.complete_brew();
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

    /// Creates a `SetContainerDataPacket` for the given property.
    #[must_use]
    pub fn create_property_packet(
        &self,
        window_id: u8,
        prop: BrewingStandProperty,
    ) -> SetContainerDataPacket {
        SetContainerDataPacket {
            window_id,
            property: prop.index(),
            value: self.property_value(prop),
        }
    }

    /// Encodes an unframed `SetContainerData` (0x14) packet payload for a given property.
    #[must_use]
    pub fn encode_container_data(&self, window_id: u8, prop: BrewingStandProperty) -> Vec<u8> {
        self.create_property_packet(window_id, prop).encode_packet()
    }

    /// Encodes a framed `SetContainerData` (0x14) packet for a given property.
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

/// Clientbound SetContainerData packet (ID 0x14) for container property synchronization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetContainerDataPacket {
    pub window_id: u8,
    pub property: u16,
    pub value: i16,
}

impl SetContainerDataPacket {
    /// Constructs a new packet.
    pub fn new(window_id: u8, property: u16, value: i16) -> Self {
        Self {
            window_id,
            property,
            value,
        }
    }

    /// Encodes the unframed packet payload `[0x14, window_id, property_be, value_be]`.
    #[must_use]
    pub fn encode_packet(&self) -> Vec<u8> {
        encode_set_container_data(self.window_id, self.property, self.value)
    }

    /// Encodes the packet body `[window_id, property_be, value_be]` without packet ID.
    #[must_use]
    pub fn encode_body(&self) -> Vec<u8> {
        encode_set_container_data_body(self.window_id, self.property, self.value)
    }

    /// Encodes the packet as a framed packet with length VarInt prefix.
    #[must_use]
    pub fn encode_framed(&self) -> Vec<u8> {
        encode_set_container_data_framed(self.window_id, self.property, self.value)
    }

    /// Decodes an unframed packet payload `[0x14, window_id, property_be, value_be]`.
    pub fn decode_packet(payload: &[u8]) -> Result<Self, SetContainerDataError> {
        decode_set_container_data(payload)
    }

    /// Decodes a packet body `[window_id, property_be, value_be]`.
    pub fn decode_body(body: &[u8]) -> Result<Self, SetContainerDataError> {
        decode_set_container_data_body(body)
    }

    /// Decodes a framed packet `[length_varint, 0x14, window_id, property_be, value_be]`.
    pub fn decode_framed(framed: &[u8]) -> Result<Self, SetContainerDataError> {
        decode_set_container_data_framed(framed)
    }
}

/// Errors during `SetContainerData` packet decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetContainerDataError {
    Malformed,
    WrongPacketId(u32),
    Oversized,
    Truncated,
}

impl fmt::Display for SetContainerDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => write!(f, "malformed SetContainerData packet"),
            Self::WrongPacketId(id) => write!(
                f,
                "wrong packet ID: expected {:#04x}, found {:#04x}",
                SET_CONTAINER_DATA_PACKET_ID, id
            ),
            Self::Oversized => write!(f, "oversized SetContainerData packet"),
            Self::Truncated => write!(f, "truncated SetContainerData packet"),
        }
    }
}

impl std::error::Error for SetContainerDataError {}

/// Encodes an unframed `SetContainerData` (0x14) packet payload:
/// `[0x14 (VarInt), window_id (u8), property (i16 BE), value (i16 BE)]`.
#[must_use]
pub fn encode_set_container_data(window_id: u8, property: u16, value: i16) -> Vec<u8> {
    let mut packet = Vec::with_capacity(6);
    put_varint(SET_CONTAINER_DATA_PACKET_ID, &mut packet);
    packet.push(window_id);
    packet.extend_from_slice(&property.to_be_bytes());
    packet.extend_from_slice(&value.to_be_bytes());
    packet
}

/// Encodes the 5-byte body of a `SetContainerData` packet:
/// `[window_id, property (2 bytes BE), value (2 bytes BE)]`.
#[must_use]
pub fn encode_set_container_data_body(window_id: u8, property: u16, value: i16) -> Vec<u8> {
    let mut body = Vec::with_capacity(5);
    body.push(window_id);
    body.extend_from_slice(&property.to_be_bytes());
    body.extend_from_slice(&value.to_be_bytes());
    body
}

/// Encodes a framed Java 26.3 `SetContainerData` (0x14) packet with length prefix.
#[must_use]
pub fn encode_set_container_data_framed(window_id: u8, property: u16, value: i16) -> Vec<u8> {
    let body = encode_set_container_data_body(window_id, property, value);
    frame(SET_CONTAINER_DATA_PACKET_ID, &body)
}

/// Decodes an unframed `SetContainerData` packet payload:
/// `[0x14 (VarInt), window_id (u8), property (i16 BE), value (i16 BE)]`.
pub fn decode_set_container_data(
    payload: &[u8],
) -> Result<SetContainerDataPacket, SetContainerDataError> {
    let Some((packet_id, prefix_len)) =
        varint(payload).map_err(|_| SetContainerDataError::Malformed)?
    else {
        return Err(SetContainerDataError::Truncated);
    };

    if packet_id != SET_CONTAINER_DATA_PACKET_ID {
        return Err(SetContainerDataError::WrongPacketId(packet_id));
    }

    let body = &payload[prefix_len..];
    decode_set_container_data_body(body)
}

/// Decodes the 5-byte body of `SetContainerData`:
/// `[window_id (u8), property (i16 BE), value (i16 BE)]`.
pub fn decode_set_container_data_body(
    body: &[u8],
) -> Result<SetContainerDataPacket, SetContainerDataError> {
    if body.len() < 5 {
        return Err(SetContainerDataError::Truncated);
    }
    if body.len() > 5 {
        return Err(SetContainerDataError::Oversized);
    }

    let window_id = body[0];
    let property = u16::from_be_bytes([body[1], body[2]]);
    let value = i16::from_be_bytes([body[3], body[4]]);

    Ok(SetContainerDataPacket {
        window_id,
        property,
        value,
    })
}

/// Decodes a framed Java 26.3 `SetContainerData` packet:
/// `[length (VarInt), 0x14 (VarInt), window_id, property, value]`.
pub fn decode_set_container_data_framed(
    framed: &[u8],
) -> Result<SetContainerDataPacket, SetContainerDataError> {
    let Some((length, len_prefix)) =
        varint(framed).map_err(|_| SetContainerDataError::Malformed)?
    else {
        return Err(SetContainerDataError::Truncated);
    };

    let total = len_prefix + length as usize;
    if framed.len() < total {
        return Err(SetContainerDataError::Truncated);
    }
    if framed.len() > total {
        return Err(SetContainerDataError::Oversized);
    }

    let inner = &framed[len_prefix..total];
    decode_set_container_data(inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuel_replenishment_from_blaze_powder_in_slot_4() {
        let mut stand = BrewingStandBlockEntity::new();
        assert_eq!(stand.fuel_charges, 0);

        // Put 2 Blaze Powder in slot 4
        stand.slots[FUEL_SLOT] = Some(ItemStack::new("minecraft:blaze_powder", 2));

        // Fuel charges == 0 -> replenish succeeds
        assert!(stand.replenish_fuel());
        assert_eq!(stand.fuel_charges, 20);
        assert_eq!(stand.slots[FUEL_SLOT].as_ref().unwrap().count, 1);

        // Fuel charges > 0 -> cannot replenish again
        assert!(!stand.replenish_fuel());
        assert_eq!(stand.fuel_charges, 20);
        assert_eq!(stand.slots[FUEL_SLOT].as_ref().unwrap().count, 1);

        // Simulate consuming all fuel charges
        stand.fuel_charges = 0;
        assert!(stand.replenish_fuel());
        assert_eq!(stand.fuel_charges, 20);
        // Slot 4 now exhausted
        assert!(stand.slots[FUEL_SLOT].is_none());

        // Further replenishment fails with no fuel left
        stand.fuel_charges = 0;
        assert!(!stand.replenish_fuel());
        assert_eq!(stand.fuel_charges, 0);
    }

    #[test]
    fn test_fuel_consumption_on_brew_cycle_start() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.fuel_charges = 1;
        stand.slots[INGREDIENT_SLOT] = Some(ItemStack::new("minecraft:nether_wart", 1));
        stand.slots[POTION_SLOT_0] = Some(ItemStack::water_bottle());

        assert_eq!(stand.brew_time, 0);
        assert_eq!(stand.fuel_charges, 1);

        // Tick 1: starts brew cycle -> consumes 1 fuel charge, initializes brew_time to 400, decrements to 399
        assert!(stand.tick());
        assert_eq!(stand.fuel_charges, 0);
        assert_eq!(stand.brew_time, 399);

        // Ticks 2..=399: brew continues, fuel charges remain 0 (already consumed)
        for expected in (1..=398).rev() {
            assert!(stand.tick());
            assert_eq!(stand.brew_time, expected);
            assert_eq!(stand.fuel_charges, 0);
        }

        // Tick 400: reaches 0, transforms bottle, ingredient consumed, brew_time reset to 0
        assert_eq!(stand.brew_time, 1);
        assert!(stand.tick());
        assert_eq!(stand.brew_time, 0);
        assert!(
            stand.slots[POTION_SLOT_0]
                .as_ref()
                .unwrap()
                .is_awkward_potion()
        );
        assert!(stand.slots[INGREDIENT_SLOT].is_none());

        // Tick 401: with 0 fuel and no blaze powder, cannot start new cycle
        stand.slots[INGREDIENT_SLOT] = Some(ItemStack::new("minecraft:sugar", 1));
        assert!(!stand.tick());
        assert_eq!(stand.brew_time, 0);
    }

    #[test]
    fn test_auto_replenish_fuel_when_starting_brew() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.fuel_charges = 0;
        stand.slots[FUEL_SLOT] = Some(ItemStack::new("minecraft:blaze_powder", 1));
        stand.slots[INGREDIENT_SLOT] = Some(ItemStack::new("minecraft:nether_wart", 1));
        stand.slots[POTION_SLOT_0] = Some(ItemStack::water_bottle());

        // Tick 1 should replenish fuel from slot 4 (20 charges), consume 1 to start brew (19 charges), brew_time = 399
        assert!(stand.tick());
        assert_eq!(stand.fuel_charges, 19);
        assert_eq!(stand.brew_time, 399);
        assert!(stand.slots[FUEL_SLOT].is_none());
    }

    #[test]
    fn test_no_fuel_blocks_brewing() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.fuel_charges = 0;
        stand.slots[INGREDIENT_SLOT] = Some(ItemStack::new("minecraft:nether_wart", 1));
        stand.slots[POTION_SLOT_0] = Some(ItemStack::water_bottle());

        assert!(!stand.tick());
        assert_eq!(stand.brew_time, 0);
        assert!(
            stand.slots[POTION_SLOT_0]
                .as_ref()
                .unwrap()
                .is_water_bottle()
        );
        assert_eq!(stand.slots[INGREDIENT_SLOT].as_ref().unwrap().count, 1);
    }

    #[test]
    fn test_interrupted_brew_resets_brew_time() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.fuel_charges = 5;
        stand.slots[INGREDIENT_SLOT] = Some(ItemStack::new("minecraft:nether_wart", 1));
        stand.slots[POTION_SLOT_0] = Some(ItemStack::water_bottle());

        stand.tick(); // brew_time -> 399, fuel_charges -> 4
        assert_eq!(stand.brew_time, 399);
        assert_eq!(stand.fuel_charges, 4);

        // Remove ingredient during brewing
        stand.slots[INGREDIENT_SLOT] = None;
        stand.tick();

        // Brew cycle aborted, brew_time reset to 0, fuel NOT refunded
        assert_eq!(stand.brew_time, 0);
        assert_eq!(stand.fuel_charges, 4);
        assert!(
            stand.slots[POTION_SLOT_0]
                .as_ref()
                .unwrap()
                .is_water_bottle()
        );
    }

    #[test]
    fn test_multi_bottle_simultaneous_brewing() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.fuel_charges = 5;
        stand.slots[POTION_SLOT_0] = Some(ItemStack::water_bottle());
        stand.slots[POTION_SLOT_1] = Some(ItemStack::water_bottle());
        stand.slots[POTION_SLOT_2] = Some(ItemStack::water_bottle());
        stand.slots[INGREDIENT_SLOT] = Some(ItemStack::new("minecraft:nether_wart", 1));

        // Fast-forward to tick 1 remaining
        stand.brew_time = 1;
        stand.tick();

        assert_eq!(stand.brew_time, 0);
        assert!(stand.slots[INGREDIENT_SLOT].is_none());

        for i in 0..=2 {
            let stack = stand.slots[i].as_ref().expect("bottle transformed");
            assert!(stack.is_awkward_potion());
            assert_eq!(stack.count, 1);
        }
    }

    #[test]
    fn test_multi_bottle_partial_matching() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.fuel_charges = 5;
        stand.slots[POTION_SLOT_0] = Some(ItemStack::awkward_potion());
        stand.slots[POTION_SLOT_1] = Some(ItemStack::water_bottle()); // Water does not match glistering melon
        stand.slots[POTION_SLOT_2] = Some(ItemStack::awkward_potion());
        stand.slots[INGREDIENT_SLOT] = Some(ItemStack::new("minecraft:glistering_melon_slice", 1));

        stand.brew_time = 1;
        stand.tick();

        assert_eq!(stand.brew_time, 0);
        assert!(
            stand.slots[POTION_SLOT_0]
                .as_ref()
                .unwrap()
                .is_potion_of(PotionEffect::Healing)
        );
        assert!(
            stand.slots[POTION_SLOT_1]
                .as_ref()
                .unwrap()
                .is_water_bottle()
        );
        assert!(
            stand.slots[POTION_SLOT_2]
                .as_ref()
                .unwrap()
                .is_potion_of(PotionEffect::Healing)
        );
    }

    #[test]
    fn test_vanilla_base_potion_recipes() {
        let recipes = [
            (
                "minecraft:nether_wart",
                "minecraft:water_bottle",
                "minecraft:awkward_potion",
            ),
            (
                "minecraft:glistering_melon_slice",
                "minecraft:awkward_potion",
                "minecraft:potion_of_healing",
            ),
            (
                "minecraft:sugar",
                "minecraft:awkward_potion",
                "minecraft:potion_of_swiftness",
            ),
            (
                "minecraft:blaze_powder",
                "minecraft:awkward_potion",
                "minecraft:potion_of_strength",
            ),
            (
                "minecraft:magma_cream",
                "minecraft:awkward_potion",
                "minecraft:potion_of_fire_resistance",
            ),
        ];

        let reg = BrewingRecipeRegistry::new();
        for (ing, input, expected_out) in recipes {
            assert!(
                reg.has_recipe(ing, input),
                "missing recipe for {ing} + {input}"
            );
            let out = reg.find_output(ing, input).expect("output exists");
            assert!(
                items_equal(&out, expected_out),
                "expected {expected_out}, got {out}"
            );

            // Verify with BrewingStand state machine
            let mut stand = BrewingStandBlockEntity::new();
            stand.fuel_charges = 5;
            stand.slots[INGREDIENT_SLOT] = Some(ItemStack::new(ing, 1));
            stand.slots[POTION_SLOT_0] = Some(ItemStack::new(input, 1));
            stand.brew_time = 1;
            stand.tick();

            let result = stand.slots[POTION_SLOT_0].as_ref().expect("transformed");
            assert!(
                result.matches_item(expected_out),
                "brew result {result:?} did not match {expected_out}"
            );
        }
    }

    #[test]
    fn test_redstone_duration_modifiers() {
        let reg = BrewingRecipeRegistry::new();

        let extendables = [
            (
                "minecraft:potion_of_swiftness",
                "minecraft:long_potion_of_swiftness",
            ),
            (
                "minecraft:potion_of_strength",
                "minecraft:long_potion_of_strength",
            ),
            (
                "minecraft:potion_of_fire_resistance",
                "minecraft:long_potion_of_fire_resistance",
            ),
            (
                "minecraft:potion_of_slowness",
                "minecraft:long_potion_of_slowness",
            ),
        ];

        for (input, expected) in extendables {
            assert!(
                reg.has_recipe("minecraft:redstone", input),
                "redstone must extend {input}"
            );
            let out = reg.find_output("minecraft:redstone", input).unwrap();
            assert!(
                items_equal(&out, expected),
                "expected {expected}, got {out}"
            );

            // Test execution in state machine
            let mut stand = BrewingStandBlockEntity::new();
            stand.fuel_charges = 5;
            stand.slots[INGREDIENT_SLOT] = Some(ItemStack::new("minecraft:redstone", 1));
            stand.slots[POTION_SLOT_0] = Some(ItemStack::new(input, 1));
            stand.brew_time = 1;
            stand.tick();

            let res = stand.slots[POTION_SLOT_0].as_ref().unwrap();
            assert!(res.matches_item(expected));
            assert_eq!(res.potion().unwrap().duration, PotionDuration::Long);
        }

        // Instant effects (Healing, Harming) cannot be extended
        assert!(!reg.has_recipe("minecraft:redstone", "minecraft:potion_of_healing"));
        assert!(!reg.has_recipe("minecraft:redstone", "minecraft:potion_of_harming"));
        assert!(!reg.has_recipe("minecraft:redstone", "minecraft:awkward_potion"));
    }

    #[test]
    fn test_glowstone_potency_modifiers() {
        let reg = BrewingRecipeRegistry::new();

        let upgradeables = [
            (
                "minecraft:potion_of_healing",
                "minecraft:strong_potion_of_healing",
            ),
            (
                "minecraft:potion_of_swiftness",
                "minecraft:strong_potion_of_swiftness",
            ),
            (
                "minecraft:potion_of_strength",
                "minecraft:strong_potion_of_strength",
            ),
            (
                "minecraft:potion_of_harming",
                "minecraft:strong_potion_of_harming",
            ),
        ];

        for (input, expected) in upgradeables {
            assert!(
                reg.has_recipe("minecraft:glowstone_dust", input),
                "glowstone must upgrade {input}"
            );
            let out = reg.find_output("minecraft:glowstone_dust", input).unwrap();
            assert!(
                items_equal(&out, expected),
                "expected {expected}, got {out}"
            );

            let mut stand = BrewingStandBlockEntity::new();
            stand.fuel_charges = 5;
            stand.slots[INGREDIENT_SLOT] = Some(ItemStack::new("minecraft:glowstone_dust", 1));
            stand.slots[POTION_SLOT_0] = Some(ItemStack::new(input, 1));
            stand.brew_time = 1;
            stand.tick();

            let res = stand.slots[POTION_SLOT_0].as_ref().unwrap();
            assert!(res.matches_item(expected));
            assert_eq!(res.potion().unwrap().tier, PotionTier::Strong);
        }

        // Fire Resistance has no tier II
        assert!(!reg.has_recipe(
            "minecraft:glowstone_dust",
            "minecraft:potion_of_fire_resistance"
        ));
        assert!(!reg.has_recipe("minecraft:glowstone_dust", "minecraft:awkward_potion"));
    }

    #[test]
    fn test_fermented_spider_eye_inversions() {
        let reg = BrewingRecipeRegistry::new();

        let inversions = [
            ("minecraft:potion_of_healing", "minecraft:potion_of_harming"),
            (
                "minecraft:strong_potion_of_healing",
                "minecraft:strong_potion_of_harming",
            ),
            (
                "minecraft:potion_of_swiftness",
                "minecraft:potion_of_slowness",
            ),
            (
                "minecraft:long_potion_of_swiftness",
                "minecraft:long_potion_of_slowness",
            ),
            (
                "minecraft:potion_of_fire_resistance",
                "minecraft:potion_of_slowness",
            ),
        ];

        for (input, expected) in inversions {
            assert!(
                reg.has_recipe("minecraft:fermented_spider_eye", input),
                "spider eye must invert {input}"
            );
            let out = reg
                .find_output("minecraft:fermented_spider_eye", input)
                .unwrap();
            assert!(
                items_equal(&out, expected),
                "expected {expected}, got {out}"
            );

            let mut stand = BrewingStandBlockEntity::new();
            stand.fuel_charges = 5;
            stand.slots[INGREDIENT_SLOT] =
                Some(ItemStack::new("minecraft:fermented_spider_eye", 1));
            stand.slots[POTION_SLOT_0] = Some(ItemStack::new(input, 1));
            stand.brew_time = 1;
            stand.tick();

            let res = stand.slots[POTION_SLOT_0].as_ref().unwrap();
            assert!(res.matches_item(expected));
        }
    }

    #[test]
    fn test_container_data_packet_encoding_and_decoding() {
        let mut stand = BrewingStandBlockEntity::new();
        stand.brew_time = 250;
        stand.fuel_charges = 15;
        let window_id = 3;

        // Property 0: BrewTime
        let pkt0 = stand.create_property_packet(window_id, BrewingStandProperty::BrewTime);
        assert_eq!(pkt0.property, 0);
        assert_eq!(pkt0.value, 250);
        let encoded0 = pkt0.encode_packet();
        assert_eq!(encoded0, vec![0x14, 3, 0, 0, 0x00, 0xfa]);
        let decoded0 = SetContainerDataPacket::decode_packet(&encoded0).unwrap();
        assert_eq!(decoded0, pkt0);

        // Property 1: FuelCharges
        let pkt1 = stand.create_property_packet(window_id, BrewingStandProperty::FuelCharges);
        assert_eq!(pkt1.property, 1);
        assert_eq!(pkt1.value, 15);
        let encoded1 = pkt1.encode_packet();
        assert_eq!(encoded1, vec![0x14, 3, 0, 1, 0x00, 0x0f]);
        let decoded1 = SetContainerDataPacket::decode_packet(&encoded1).unwrap();
        assert_eq!(decoded1, pkt1);

        // Framed packet round-trip
        let framed0 = pkt0.encode_framed();
        assert_eq!(framed0, vec![6, 0x14, 3, 0, 0, 0x00, 0xfa]);
        let decoded_framed = SetContainerDataPacket::decode_framed(&framed0).unwrap();
        assert_eq!(decoded_framed, pkt0);

        // Batch encode
        let all = stand.encode_all_container_data(window_id);
        assert_eq!(all[0], encoded0);
        assert_eq!(all[1], encoded1);
    }

    #[test]
    fn test_malformed_and_edge_case_packets() {
        // Truncated packet
        assert_eq!(
            SetContainerDataPacket::decode_packet(&[0x14, 1, 0]),
            Err(SetContainerDataError::Truncated)
        );

        // Wrong packet ID
        assert_eq!(
            SetContainerDataPacket::decode_packet(&[0x15, 1, 0, 0, 0, 1]),
            Err(SetContainerDataError::WrongPacketId(0x15))
        );

        // Oversized body
        assert_eq!(
            SetContainerDataPacket::decode_body(&[1, 0, 0, 0, 1, 0xff]),
            Err(SetContainerDataError::Oversized)
        );
    }

    #[test]
    fn test_custom_recipe_registration() {
        let mut reg = BrewingRecipeRegistry::new();
        reg.register_recipe(
            "minecraft:apple",
            "minecraft:water_bottle",
            "minecraft:potion_of_swiftness",
        );

        let mut stand = BrewingStandBlockEntity::with_registry(reg);
        stand.fuel_charges = 1;
        stand.slots[INGREDIENT_SLOT] = Some(ItemStack::new("minecraft:apple", 1));
        stand.slots[POTION_SLOT_0] = Some(ItemStack::water_bottle());

        stand.brew_time = 1;
        stand.tick();

        assert_eq!(stand.brew_time, 0);
        let res = stand.slots[POTION_SLOT_0].as_ref().unwrap();
        assert!(res.is_potion_of(PotionEffect::Swiftness));
    }

    #[test]
    fn test_slot_bounds_and_accessors() {
        let mut stand = BrewingStandBlockEntity::new();
        assert!(stand.set_slot(0, Some(ItemStack::water_bottle())).is_ok());
        assert!(stand.set_slot(4, Some(ItemStack::blaze_powder(10))).is_ok());
        assert!(stand.set_slot(5, Some(ItemStack::water_bottle())).is_err());

        assert!(stand.set_potion(0, Some(ItemStack::water_bottle())).is_ok());
        assert!(
            stand
                .set_potion(3, Some(ItemStack::water_bottle()))
                .is_err()
        );

        assert!(stand.potion_slot(0).is_some());
        assert!(stand.potion_slot(3).is_none());
    }
}
