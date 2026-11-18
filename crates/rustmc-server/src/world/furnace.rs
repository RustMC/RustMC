//! Milestone M4/M5 Furnace Smelting State Machine and Fuel Burning.
//!
//! Provides authoritative furnace block entity state progression, fuel registry burn time
//! evaluation, smelting recipe evaluation, experience points accumulation and withdrawal,
//! and furnace property sync helpers using [`SetContainerDataPacket`].
//!
//! Sources: furnace property indices from minecraft.wiki
//! `Java_Edition_protocol/Packets` revision 3868170; observed smelting behavior
//! from minecraft.wiki `Smelting` revision 3829803 and `Furnace` revision 3882524.

use super::container_data::SetContainerDataPacket;
use super::recipe_data::{CookingKind, RecipeData};
use crate::item::{DEFAULT_MAX_STACK, ItemStack, ItemStackError};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

/// Default total cooking duration in ticks for a standard smelting recipe (minecraft.wiki `Smelting` rev 3829803: 200 ticks = 10s).
pub const DEFAULT_COOKING_TOTAL_TIME: u16 = 200;

/// Burn duration in ticks for coal and charcoal (minecraft.wiki `Smelting` rev 3829803: 1600 ticks = 8 smelts).
const COAL_BURN_TIME: u16 = 1600;
/// Burn duration in ticks for a lava bucket (minecraft.wiki `Smelting` rev 3829803: 20000 ticks = 100 smelts).
const LAVA_BUCKET_BURN_TIME: u16 = 20_000;
/// Burn duration in ticks for wooden planks and flammable logs/wood (minecraft.wiki `Smelting` rev 3829803: 300 ticks = 1.5 smelts).
const WOOD_BURN_TIME: u16 = 300;
/// Burn duration in ticks for flammable wooden slabs (minecraft.wiki `Smelting` rev 3829803: 150 ticks = 0.75 smelts).
const WOODEN_SLAB_BURN_TIME: u16 = 150;
/// Burn duration in ticks for sticks (minecraft.wiki `Smelting` rev 3829803: 100 ticks = 0.5 smelts).
const STICK_BURN_TIME: u16 = 100;

/// Experience yielded by smelting raw iron or iron ore (minecraft.wiki `Smelting` rev 3829803).
const IRON_SMELT_XP: f32 = 0.7;
/// Experience yielded by smelting raw gold or gold ore (minecraft.wiki `Smelting` rev 3829803).
const GOLD_SMELT_XP: f32 = 1.0;
/// Experience yielded by smelting raw copper or copper ore (minecraft.wiki `Smelting` rev 3829803).
const COPPER_SMELT_XP: f32 = 0.7;
/// Experience yielded by smelting cobblestone or sand (minecraft.wiki `Smelting` rev 3829803).
const COMMON_SMELT_XP: f32 = 0.1;
/// Experience yielded by cooking beef or porkchop (minecraft.wiki `Smelting` rev 3829803).
const MEAT_SMELT_XP: f32 = 0.35;

/// Slot indices matching standard Minecraft furnace container layouts.
pub const INPUT_SLOT: usize = 0;
pub const FUEL_SLOT: usize = 1;
pub const OUTPUT_SLOT: usize = 2;
pub const TOTAL_SLOTS: usize = 3;

/// Strips optional `minecraft:` namespace prefix for canonical item matching.
#[inline]
pub fn normalize_item_id(item: &str) -> &str {
    item.strip_prefix("minecraft:").unwrap_or(item)
}

/// Checks if two item identifiers match canonically regardless of `minecraft:` prefix.
#[inline]
pub fn items_equal(a: &str, b: &str) -> bool {
    normalize_item_id(a) == normalize_item_id(b)
}

/// Returns the standard default maximum stack size for common items.
#[must_use]
pub fn default_max_stack(item: &str) -> u16 {
    match normalize_item_id(item) {
        "lava_bucket" | "water_bucket" | "milk_bucket" => 1,
        "bucket" | "snowball" | "ender_pearl" | "egg" => 16,
        _ => DEFAULT_MAX_STACK,
    }
}

/// Registered fuel entry with burn duration in ticks and optional residual item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuelEntry {
    burn_time: u16,
    remainder: Option<ItemStack>,
}

/// Evaluates fuel burn time and residual item for items according to official specifications:
/// - Coal / Charcoal: 1600 ticks (8 smelts).
/// - Wooden planks / logs: 300 ticks (1.5 smelts). Crimson and warped wood is
///   non-flammable and does not burn.
/// - Wooden slabs: 150 ticks.
/// - Sticks: 100 ticks (0.5 smelts).
/// - Lava bucket: 20000 ticks (100 smelts), leaves empty bucket (`minecraft:bucket`).
/// - Non-fuel items: 0 ticks (rejected).
#[derive(Debug, Clone, Default)]
pub struct FuelRegistry {
    custom: BTreeMap<String, FuelEntry>,
}

impl FuelRegistry {
    /// Creates an empty registry relying on default vanilla fuel specifications.
    pub fn new() -> Self {
        Self {
            custom: BTreeMap::new(),
        }
    }

    /// Registers a custom fuel burn time and optional remainder.
    pub fn register(
        &mut self,
        item_id: impl Into<String>,
        burn_time: u16,
        remainder: Option<String>,
    ) -> Result<(), ItemStackError> {
        let remainder = remainder
            .map(|id| {
                let max_stack = default_max_stack(&id);
                ItemStack::try_new(id, 1, max_stack)
            })
            .transpose()?;
        let item_id = item_id.into();
        let key = normalize_item_id(&item_id).to_string();
        self.custom.insert(
            key,
            FuelEntry {
                burn_time,
                remainder,
            },
        );
        Ok(())
    }

    /// Returns the burn duration in ticks for an item, or 0 if not a fuel.
    #[must_use]
    pub fn burn_time(&self, item_id: &str) -> u16 {
        let normalized = normalize_item_id(item_id);
        if let Some(entry) = self.custom.get(normalized) {
            return entry.burn_time;
        }
        fuel_burn_time(normalized)
    }

    /// Returns the remainder item left behind when consuming fuel, if any.
    #[must_use]
    pub fn remainder(&self, item_id: &str) -> Option<ItemStack> {
        let normalized = normalize_item_id(item_id);
        if let Some(entry) = self.custom.get(normalized) {
            return entry.remainder.clone();
        }
        fuel_remainder(normalized).map(|rem| {
            ItemStack::try_new(rem, 1, default_max_stack(rem))
                .expect("built-in fuel remainders use valid item ids")
        })
    }

    /// Whether an item is recognized as valid fuel.
    #[must_use]
    pub fn is_fuel(&self, item_id: &str) -> bool {
        self.burn_time(item_id) > 0
    }
}

/// Returns the standard vanilla fuel burn duration in ticks for a given item identifier.
///
/// Only real Java 26.3 item IDs from `registries.json` and the server's
/// `logs_that_burn`, `planks`, `wooden_slabs`, and `bamboo_blocks` tags are
/// accepted; crimson/warped non-flammable woods and unknown IDs return 0.
#[must_use]
pub fn fuel_burn_time(item_id: &str) -> u16 {
    let name = normalize_item_id(item_id);
    match name {
        // Coal and Charcoal: 1600 ticks (8 smelts)
        "coal" | "charcoal" => COAL_BURN_TIME,

        // Lava Bucket: 20000 ticks (100 smelts)
        "lava_bucket" => LAVA_BUCKET_BURN_TIME,

        // Sticks: 100 ticks (0.5 smelts)
        "stick" => STICK_BURN_TIME,

        // Flammable wooden slabs (`#minecraft:wooden_slabs` excluding crimson/warped, plus bamboo_mosaic_slab): 150 ticks
        "oak_slab" | "spruce_slab" | "birch_slab" | "jungle_slab" | "acacia_slab"
        | "dark_oak_slab" | "pale_oak_slab" | "mangrove_slab" | "cherry_slab" | "bamboo_slab"
        | "bamboo_mosaic_slab" | "poplar_slab" => WOODEN_SLAB_BURN_TIME,

        // Flammable wooden planks (`#minecraft:planks` excluding crimson/warped): 300 ticks (1.5 smelts)
        "oak_planks" | "spruce_planks" | "birch_planks" | "jungle_planks" | "acacia_planks"
        | "dark_oak_planks" | "pale_oak_planks" | "mangrove_planks" | "cherry_planks"
        | "bamboo_planks" | "poplar_planks" => WOOD_BURN_TIME,

        // Flammable logs, wood, and bamboo blocks (`#minecraft:logs_that_burn` and `#minecraft:bamboo_blocks`): 300 ticks
        "oak_log"
        | "spruce_log"
        | "birch_log"
        | "jungle_log"
        | "acacia_log"
        | "dark_oak_log"
        | "pale_oak_log"
        | "mangrove_log"
        | "cherry_log"
        | "poplar_log"
        | "stripped_oak_log"
        | "stripped_spruce_log"
        | "stripped_birch_log"
        | "stripped_jungle_log"
        | "stripped_acacia_log"
        | "stripped_dark_oak_log"
        | "stripped_pale_oak_log"
        | "stripped_mangrove_log"
        | "stripped_cherry_log"
        | "stripped_poplar_log"
        | "oak_wood"
        | "spruce_wood"
        | "birch_wood"
        | "jungle_wood"
        | "acacia_wood"
        | "dark_oak_wood"
        | "pale_oak_wood"
        | "mangrove_wood"
        | "cherry_wood"
        | "poplar_wood"
        | "stripped_oak_wood"
        | "stripped_spruce_wood"
        | "stripped_birch_wood"
        | "stripped_jungle_wood"
        | "stripped_acacia_wood"
        | "stripped_dark_oak_wood"
        | "stripped_pale_oak_wood"
        | "stripped_mangrove_wood"
        | "stripped_cherry_wood"
        | "stripped_poplar_wood"
        | "bamboo_block"
        | "stripped_bamboo_block" => WOOD_BURN_TIME,

        _ => 0,
    }
}

/// Returns the standard vanilla residual item left when burning a fuel item.
#[must_use]
pub fn fuel_remainder(item_id: &str) -> Option<&'static str> {
    match normalize_item_id(item_id) {
        "lava_bucket" => Some("minecraft:bucket"),
        _ => None,
    }
}

/// Smelting recipe definition matching an input item type to an output item and experience yield.
#[derive(Debug, Clone, PartialEq)]
pub struct SmeltingRecipe {
    input: String,
    output: ItemStack,
    experience: f32,
    cooking_time: u16,
}

impl SmeltingRecipe {
    /// Creates a new smelting recipe, validating a single-item output stack.
    pub fn new(
        input: impl Into<String>,
        output: impl Into<String>,
        experience: f32,
        cooking_time: u16,
    ) -> Result<Self, ItemStackError> {
        let output = output.into();
        let max_stack = default_max_stack(&output);
        let output = ItemStack::try_new(output, 1, max_stack)?;
        Ok(Self::with_output(input, output, experience, cooking_time))
    }

    /// Creates a new smelting recipe with an explicit output stack (keeping counts above 1).
    #[must_use]
    pub fn with_output(
        input: impl Into<String>,
        output: ItemStack,
        experience: f32,
        cooking_time: u16,
    ) -> Self {
        Self {
            input: input.into(),
            output,
            experience,
            cooking_time,
        }
    }

    /// Returns the input item identifier.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Returns the output stack produced by this recipe.
    #[must_use]
    pub fn output(&self) -> &ItemStack {
        &self.output
    }

    /// Returns the experience yielded per smelted item.
    #[must_use]
    pub fn experience(&self) -> f32 {
        self.experience
    }

    /// Returns the cooking duration in ticks.
    #[must_use]
    pub fn cooking_time(&self) -> u16 {
        self.cooking_time
    }
}

/// Registry of smelting recipes for furnace processing.
#[derive(Debug, Clone)]
pub struct SmeltingRecipeRegistry {
    recipes: BTreeMap<String, SmeltingRecipe>,
}

impl Default for SmeltingRecipeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SmeltingRecipeRegistry {
    fn register_builtin(&mut self, input: &str, output: &str, experience: f32, cooking_time: u16) {
        let recipe = SmeltingRecipe::new(input, output, experience, cooking_time)
            .expect("built-in smelting recipes use valid item ids");
        self.register(recipe);
    }

    /// Offline fallback registry used when no operator-provisioned data root is provisioned.
    ///
    /// Constructs a new recipe registry pre-populated with standard vanilla smelting recipes:
    /// - Raw Iron / Iron Ore -> Iron Ingot (0.7 XP).
    /// - Raw Gold / Gold Ore -> Gold Ingot (1.0 XP).
    /// - Raw Copper -> Copper Ingot (0.7 XP).
    /// - Cobblestone -> Stone (0.1 XP).
    /// - Sand -> Glass (0.1 XP).
    /// - Raw Beef -> Cooked Beef (0.35 XP).
    /// - Raw Porkchop -> Cooked Porkchop (0.35 XP).
    pub fn new() -> Self {
        const BUILTINS: [(&str, &str, f32); 15] = [
            // Iron recipes
            ("minecraft:raw_iron", "minecraft:iron_ingot", IRON_SMELT_XP),
            ("minecraft:iron_ore", "minecraft:iron_ingot", IRON_SMELT_XP),
            (
                "minecraft:deepslate_iron_ore",
                "minecraft:iron_ingot",
                IRON_SMELT_XP,
            ),
            // Gold recipes
            ("minecraft:raw_gold", "minecraft:gold_ingot", GOLD_SMELT_XP),
            ("minecraft:gold_ore", "minecraft:gold_ingot", GOLD_SMELT_XP),
            (
                "minecraft:deepslate_gold_ore",
                "minecraft:gold_ingot",
                GOLD_SMELT_XP,
            ),
            (
                "minecraft:nether_gold_ore",
                "minecraft:gold_ingot",
                GOLD_SMELT_XP,
            ),
            // Copper recipes
            (
                "minecraft:raw_copper",
                "minecraft:copper_ingot",
                COPPER_SMELT_XP,
            ),
            (
                "minecraft:copper_ore",
                "minecraft:copper_ingot",
                COPPER_SMELT_XP,
            ),
            (
                "minecraft:deepslate_copper_ore",
                "minecraft:copper_ingot",
                COPPER_SMELT_XP,
            ),
            // Stone recipes
            ("minecraft:cobblestone", "minecraft:stone", COMMON_SMELT_XP),
            // Glass recipes
            ("minecraft:sand", "minecraft:glass", COMMON_SMELT_XP),
            ("minecraft:red_sand", "minecraft:glass", COMMON_SMELT_XP),
            // Food recipes (26.3 uses `minecraft:beef` and `minecraft:porkchop`, not `raw_*`)
            ("minecraft:beef", "minecraft:cooked_beef", MEAT_SMELT_XP),
            (
                "minecraft:porkchop",
                "minecraft:cooked_porkchop",
                MEAT_SMELT_XP,
            ),
        ];

        let mut registry = Self {
            recipes: BTreeMap::new(),
        };

        for (input, output, experience) in BUILTINS {
            registry.register_builtin(input, output, experience, DEFAULT_COOKING_TOTAL_TIME);
        }

        registry
    }

    /// Builds a cooking recipe registry for `kind` from loaded [`RecipeData`].
    #[must_use]
    pub fn from_recipe_data(data: &RecipeData, kind: CookingKind) -> Self {
        let mut registry = Self {
            recipes: BTreeMap::new(),
        };
        for recipe in data.cooking(kind) {
            for input in recipe.ingredient().allowed_items() {
                registry.register(SmeltingRecipe::with_output(
                    input,
                    recipe.result().clone(),
                    recipe.experience(),
                    recipe.cooking_time(),
                ));
            }
        }
        registry
    }

    /// Registers or replaces a smelting recipe.
    pub fn register(&mut self, recipe: SmeltingRecipe) {
        let key = normalize_item_id(&recipe.input).to_string();
        self.recipes.insert(key, recipe);
    }

    /// Finds a smelting recipe for a given input item identifier.
    #[must_use]
    pub fn find_recipe(&self, input_item: &str) -> Option<&SmeltingRecipe> {
        let normalized = normalize_item_id(input_item);
        self.recipes.get(normalized)
    }
}

/// Errors returned during furnace slot or container interactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FurnaceError {
    InvalidSlot(usize),
}

impl fmt::Display for FurnaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlot(slot) => write!(f, "invalid furnace slot index: {slot}"),
        }
    }
}

impl std::error::Error for FurnaceError {}

/// Authoritative furnace block entity state machine.
///
/// Contains 3 item slots:
/// - Slot 0: Input item stack (`Option<ItemStack>`).
/// - Slot 1: Fuel item stack (`Option<ItemStack>`).
/// - Slot 2: Output item stack (`Option<ItemStack>`).
///
/// State and timers:
/// - `lit_time: u16`: remaining ticks current fuel will burn.
/// - `lit_duration: u16`: total burn duration of the current fuel item.
/// - `cooking_progress: u16`: current cook progress ticks ($0 \le \text{cooking\_progress} \le \text{cooking\_total\_time}$).
/// - `cooking_total_time: u16`: total ticks required to smelt (default 200 ticks = 10 seconds).
/// - `stored_experience: f32`: accumulated experience points waiting for withdrawal.
#[derive(Debug, Clone, PartialEq)]
pub struct FurnaceBlockEntity {
    input: Option<ItemStack>,
    fuel: Option<ItemStack>,
    output: Option<ItemStack>,
    lit_time: u16,
    lit_duration: u16,
    cooking_progress: u16,
    cooking_total_time: u16,
    stored_experience: f32,
    /// Item the current progress belongs to; replacing it restarts the cook.
    cooking_input: Option<String>,
}

impl Default for FurnaceBlockEntity {
    fn default() -> Self {
        Self::new()
    }
}

impl FurnaceBlockEntity {
    /// Creates a new unlit `FurnaceBlockEntity` with empty slots and default cook total time (200).
    pub fn new() -> Self {
        Self {
            input: None,
            fuel: None,
            output: None,
            lit_time: 0,
            lit_duration: 0,
            cooking_progress: 0,
            cooking_total_time: DEFAULT_COOKING_TOTAL_TIME,
            stored_experience: 0.0,
            cooking_input: None,
        }
    }

    /// Whether the furnace is currently burning fuel.
    #[inline]
    pub fn is_lit(&self) -> bool {
        self.lit_time > 0
    }

    /// Retrieves a reference to an item stack in the given slot (0: input, 1: fuel, 2: output).
    #[must_use]
    pub fn slot(&self, index: usize) -> Option<&ItemStack> {
        match index {
            INPUT_SLOT => self.input.as_ref(),
            FUEL_SLOT => self.fuel.as_ref(),
            OUTPUT_SLOT => self.output.as_ref(),
            _ => None,
        }
    }

    /// Retrieves a mutable reference to an item stack in the given slot.
    pub fn slot_mut(&mut self, index: usize) -> Option<&mut Option<ItemStack>> {
        match index {
            INPUT_SLOT => Some(&mut self.input),
            FUEL_SLOT => Some(&mut self.fuel),
            OUTPUT_SLOT => Some(&mut self.output),
            _ => None,
        }
    }

    /// Sets or clears the item stack in the given slot.
    pub fn set_slot(&mut self, index: usize, stack: Option<ItemStack>) -> Result<(), FurnaceError> {
        match index {
            INPUT_SLOT => self.input = stack,
            FUEL_SLOT => self.fuel = stack,
            OUTPUT_SLOT => self.output = stack,
            _ => return Err(FurnaceError::InvalidSlot(index)),
        }
        Ok(())
    }

    /// Resets cooking progress back to 0.
    pub fn reset_cooking_progress(&mut self) {
        self.cooking_progress = 0;
    }

    /// Returns the accumulated experience points waiting for withdrawal.
    #[must_use]
    pub fn stored_experience(&self) -> f32 {
        self.stored_experience
    }

    /// Withdraws accumulated experience points.
    ///
    /// Returns integer experience points to drop/give player and the total accumulated
    /// float experience, then resets `stored_experience` to 0.0.
    pub fn withdraw_experience(&mut self) -> (u32, f32) {
        let total = self.stored_experience;
        let points = total.floor() as u32;
        self.stored_experience = 0.0;
        (points, total)
    }

    /// Executes one authoritative tick of furnace smelting using default fuel and recipe registries.
    pub fn tick(&mut self) {
        static DEFAULTS: OnceLock<(FuelRegistry, SmeltingRecipeRegistry)> = OnceLock::new();
        let (fuels, recipes) =
            DEFAULTS.get_or_init(|| (FuelRegistry::new(), SmeltingRecipeRegistry::new()));
        self.tick_with_registries(fuels, recipes);
    }

    /// Executes one authoritative tick using custom fuel and recipe registries.
    pub fn tick_with_registries(&mut self, fuels: &FuelRegistry, recipes: &SmeltingRecipeRegistry) {
        // Step 1: Lit time decrements each tick if burning.
        if self.lit_time > 0 {
            self.lit_time -= 1;
        }

        // Progress belongs to one item: replacing or removing it restarts the cook.
        let current_input = self
            .input
            .as_ref()
            .map(|inp| normalize_item_id(inp.item()).to_owned());
        if self.cooking_input.is_some() && self.cooking_input != current_input {
            self.cooking_progress = 0;
        }
        self.cooking_input = current_input;

        // Step 2: Evaluate matching cookable recipe and output slot capacity.
        let recipe = self
            .input
            .as_ref()
            .and_then(|inp| recipes.find_recipe(inp.item()));

        let cookable_recipe = if let Some(rec) = recipe {
            self.cooking_total_time = rec.cooking_time();
            let output_ready = match &self.output {
                None => true,
                Some(out) => {
                    out.can_stack_with(rec.output())
                        && out.count().saturating_add(rec.output().count()) <= out.max_stack()
                }
            };
            output_ready.then_some(rec)
        } else {
            None
        };

        if let Some(rec) = cookable_recipe {
            // Step 3a: If unlit and cookable, attempt to consume 1 fuel from slot 1.
            if self.lit_time == 0
                && let Some(burn) = self
                    .fuel
                    .as_ref()
                    .map(|fuel_stack| fuels.burn_time(fuel_stack.item()))
                    .filter(|&burn| burn > 0)
            {
                if let Some(consumed) = consume_one(&mut self.fuel)
                    && self.fuel.is_none()
                {
                    self.fuel = fuels.remainder(consumed.item());
                }

                self.lit_time = burn;
                self.lit_duration = burn;
            }

            // Step 3b: If lit, advance cooking progress.
            if self.lit_time > 0 {
                self.cooking_progress = self.cooking_progress.saturating_add(1);

                // Completion check
                if self.cooking_progress >= self.cooking_total_time {
                    // Decrement 1 input from slot 0
                    consume_one(&mut self.input);

                    // Increment output stack by recipe result count in slot 2
                    match &mut self.output {
                        Some(out) => {
                            let overflow = out.add(rec.output().count());
                            debug_assert_eq!(overflow, 0);
                        }
                        None => {
                            self.output = Some(rec.output().clone());
                        }
                    }

                    // Accumulate XP yield
                    self.stored_experience += rec.experience();

                    // Reset cooking progress
                    self.cooking_progress = 0;
                }
            } else {
                // Out of fuel: partial progress is undone at double speed.
                self.cooking_progress = self.cooking_progress.saturating_sub(2);
            }
        } else if self.lit_time > 0 {
            // Step 4: Burning with no valid recipe or a blocked output resets progress.
            self.cooking_progress = 0;
        } else {
            self.cooking_progress = self.cooking_progress.saturating_sub(2);
        }
    }

    /// Gets the property value corresponding to a `FurnaceProperty`.
    #[must_use]
    pub fn property_value(&self, prop: FurnaceProperty) -> u16 {
        match prop {
            FurnaceProperty::LitTime => self.lit_time,
            FurnaceProperty::LitDuration => self.lit_duration,
            FurnaceProperty::CookingProgress => self.cooking_progress,
            FurnaceProperty::CookingTotalTime => self.cooking_total_time,
        }
    }

    /// Sets the property value corresponding to a `FurnaceProperty`.
    pub fn set_property_value(&mut self, prop: FurnaceProperty, value: u16) {
        match prop {
            FurnaceProperty::LitTime => self.lit_time = value,
            FurnaceProperty::LitDuration => self.lit_duration = value,
            FurnaceProperty::CookingProgress => self.cooking_progress = value,
            FurnaceProperty::CookingTotalTime => self.cooking_total_time = value,
        }
    }

    /// Creates a `SetContainerDataPacket` for the given property.
    #[must_use]
    pub fn create_property_packet(
        &self,
        window_id: u8,
        prop: FurnaceProperty,
    ) -> SetContainerDataPacket {
        SetContainerDataPacket::new(
            window_id,
            prop.index(),
            // The wire field is a signed Short; custom fuels may burn for longer.
            self.property_value(prop).min(i16::MAX as u16) as i16,
        )
    }

    /// Encodes an unframed `SetContainerData` (0x13) packet payload for a given property.
    #[must_use]
    pub fn encode_container_data(&self, window_id: u8, prop: FurnaceProperty) -> Vec<u8> {
        self.create_property_packet(window_id, prop).encode_packet()
    }

    /// Encodes a framed `SetContainerData` (0x13) packet for a given property.
    #[must_use]
    pub fn encode_container_data_framed(&self, window_id: u8, prop: FurnaceProperty) -> Vec<u8> {
        self.create_property_packet(window_id, prop).encode_framed()
    }

    /// Encodes unframed packets for all 4 furnace container properties.
    #[must_use]
    pub fn encode_all_container_data(&self, window_id: u8) -> [Vec<u8>; 4] {
        [
            self.encode_container_data(window_id, FurnaceProperty::LitTime),
            self.encode_container_data(window_id, FurnaceProperty::LitDuration),
            self.encode_container_data(window_id, FurnaceProperty::CookingProgress),
            self.encode_container_data(window_id, FurnaceProperty::CookingTotalTime),
        ]
    }

    /// Encodes framed packets for all 4 furnace container properties.
    #[must_use]
    pub fn encode_all_container_data_framed(&self, window_id: u8) -> [Vec<u8>; 4] {
        [
            self.encode_container_data_framed(window_id, FurnaceProperty::LitTime),
            self.encode_container_data_framed(window_id, FurnaceProperty::LitDuration),
            self.encode_container_data_framed(window_id, FurnaceProperty::CookingProgress),
            self.encode_container_data_framed(window_id, FurnaceProperty::CookingTotalTime),
        ]
    }
}

fn consume_one(slot: &mut Option<ItemStack>) -> Option<ItemStack> {
    let stack = slot.take()?;
    match stack.split(1) {
        Ok((consumed, rest)) => {
            *slot = rest;
            Some(consumed)
        }
        Err(unchanged) => {
            *slot = Some(unchanged);
            None
        }
    }
}

/// Java 26.3 furnace container data property indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum FurnaceProperty {
    /// Remaining ticks the current fuel will burn.
    LitTime = 0,
    /// Total burn duration of the current fuel item.
    LitDuration = 1,
    /// Current cook progress ticks ($0 \le \text{cooking\_progress} \le \text{cooking\_total\_time}$).
    CookingProgress = 2,
    /// Total ticks required to smelt (default 200 ticks = 10s).
    CookingTotalTime = 3,
}

impl FurnaceProperty {
    pub const ALL: [Self; 4] = [
        Self::LitTime,
        Self::LitDuration,
        Self::CookingProgress,
        Self::CookingTotalTime,
    ];

    #[inline]
    pub fn index(self) -> u16 {
        self as u16
    }

    #[must_use]
    pub fn from_index(index: u16) -> Option<Self> {
        match index {
            0 => Some(Self::LitTime),
            1 => Some(Self::LitDuration),
            2 => Some(Self::CookingProgress),
            3 => Some(Self::CookingTotalTime),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(item: &str, count: u16) -> ItemStack {
        ItemStack::try_new(item, count, default_max_stack(item)).unwrap()
    }

    #[test]
    fn test_fuel_burning_progression_and_decrementing_fuel_item() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(stack("minecraft:raw_iron", 5));
        furnace.fuel = Some(stack("minecraft:coal", 2));

        assert_eq!(furnace.lit_time, 0);
        assert_eq!(furnace.lit_duration, 0);
        assert_eq!(furnace.cooking_progress, 0);

        // Tick 1: Coal consumed, remaining count drops to 1, lit_time initialized to 1600.
        furnace.tick();
        assert_eq!(furnace.fuel.as_ref().unwrap().count(), 1);
        assert_eq!(furnace.fuel.as_ref().unwrap().item(), "minecraft:coal");
        assert_eq!(furnace.lit_duration, 1600);
        assert_eq!(furnace.lit_time, 1600);
        assert_eq!(furnace.cooking_progress, 1);
        assert!(furnace.is_lit());

        // Tick 2: lit_time decrements by 1 each tick, cooking progress advances.
        furnace.tick();
        assert_eq!(furnace.lit_time, 1599);
        assert_eq!(furnace.cooking_progress, 2);
        assert_eq!(furnace.fuel.as_ref().unwrap().count(), 1);
    }

    #[test]
    fn test_lava_bucket_burning_leaving_empty_bucket_in_slot_1() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(stack("minecraft:raw_iron", 1));
        furnace.fuel = Some(stack("minecraft:lava_bucket", 1));

        assert_eq!(furnace.lit_time, 0);

        // Tick 1: Lava bucket consumed. Burns for 20000 ticks.
        // Remainder item "minecraft:bucket" is placed in fuel slot.
        furnace.tick();

        assert_eq!(furnace.lit_time, 20000);
        assert_eq!(furnace.lit_duration, 20000);
        assert_eq!(furnace.cooking_progress, 1);
        assert_eq!(furnace.fuel.as_ref().unwrap().item(), "minecraft:bucket");
        assert_eq!(furnace.fuel.as_ref().unwrap().count(), 1);
    }

    #[test]
    fn test_complete_200_tick_smelting_cycle_producing_output_ingot_and_clearing_input() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(stack("minecraft:raw_iron", 1));
        furnace.fuel = Some(stack("minecraft:coal", 1));

        assert!(furnace.output.is_none());

        // Run 200 ticks
        for tick in 1..=200 {
            furnace.tick();
            if tick < 200 {
                assert!(furnace.output.is_none());
                assert_eq!(furnace.cooking_progress, tick as u16);
            }
        }

        // After 200 ticks:
        // Input cleared (count was 1)
        assert!(furnace.input.is_none());

        // Output produced: 1 iron ingot
        let output = furnace.output.as_ref().expect("output ingot present");
        assert_eq!(output.item(), "minecraft:iron_ingot");
        assert_eq!(output.count(), 1);

        // Progress reset to 0
        assert_eq!(furnace.cooking_progress, 0);

        // Lit time: 1600 - 199 = 1401 (consumed on tick 1, decremented on ticks 2..=200)
        assert_eq!(furnace.lit_time, 1401);

        // Fuel stack cleared (count was 1 coal)
        assert!(furnace.fuel.is_none());

        // XP accumulated for iron ingot (0.7 XP)
        assert!((furnace.stored_experience - 0.7).abs() < 1e-4);
        assert!((furnace.stored_experience() - 0.7).abs() < 1e-4);
    }

    #[test]
    fn test_cook_progress_reset_when_input_is_removed_mid_cook() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(stack("minecraft:raw_iron", 4));
        furnace.fuel = Some(stack("minecraft:coal", 1));

        // Advance 50 ticks
        for _ in 0..50 {
            furnace.tick();
        }
        assert_eq!(furnace.cooking_progress, 50);
        assert!(furnace.lit_time > 0);

        // Remove input mid-cook
        furnace.input = None;

        // Next tick
        furnace.tick();

        // Cooking progress reset
        assert_eq!(furnace.cooking_progress, 0);
        // Furnace is still burning remaining fuel
        assert!(furnace.lit_time > 0);
    }

    #[test]
    fn test_output_slot_full_blocking_smelting_until_cleared() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(stack("minecraft:raw_iron", 5));
        furnace.fuel = Some(stack("minecraft:coal", 1));
        // Fill output slot to max stack (64)
        furnace.output = Some(stack("minecraft:iron_ingot", 64));

        // Attempt tick: output cannot receive product, so it must not smelt or consume fuel
        furnace.tick();
        assert_eq!(furnace.cooking_progress, 0);
        assert_eq!(furnace.lit_time, 0); // No fuel consumed
        assert_eq!(furnace.fuel.as_ref().unwrap().count(), 1);
        assert_eq!(furnace.output.as_ref().unwrap().count(), 64);

        // Clear output slot
        furnace.output = None;

        // Smelting can now start
        furnace.tick();
        assert_eq!(furnace.cooking_progress, 1);
        assert_eq!(furnace.lit_time, 1600);
        assert_eq!(furnace.fuel, None); // Consumed the single coal
    }

    #[test]
    fn test_stored_experience_accumulation_and_complete_withdrawal() {
        let mut furnace = FurnaceBlockEntity::new();
        // Smelt 2 raw iron (0.7 * 2 = 1.4) + 1 raw gold (1.0) = 2.4 XP
        furnace.input = Some(stack("minecraft:raw_iron", 2));
        furnace.fuel = Some(stack("minecraft:coal", 2));

        // Smelt first iron
        for _ in 0..200 {
            furnace.tick();
        }
        assert_eq!(furnace.output.as_ref().unwrap().count(), 1);
        assert!((furnace.stored_experience - 0.7).abs() < 1e-4);

        // Smelt second iron
        for _ in 0..200 {
            furnace.tick();
        }
        assert_eq!(furnace.output.as_ref().unwrap().count(), 2);
        assert!((furnace.stored_experience - 1.4).abs() < 1e-4);

        // Now place 1 raw gold into input
        furnace.input = Some(stack("minecraft:raw_gold", 1));
        // Clear output so gold ingot can be produced
        furnace.output = None;

        for _ in 0..200 {
            furnace.tick();
        }
        assert_eq!(furnace.output.as_ref().unwrap().count(), 1);
        assert_eq!(
            furnace.output.as_ref().unwrap().item(),
            "minecraft:gold_ingot"
        );
        // Total experience: 1.4 + 1.0 = 2.4
        assert!((furnace.stored_experience - 2.4).abs() < 1e-4);

        // Withdraw experience
        let (points, total_float) = furnace.withdraw_experience();
        assert_eq!(points, 2);
        assert!((total_float - 2.4).abs() < 1e-4);
        assert_eq!(furnace.stored_experience, 0.0);

        // Second withdrawal yields 0
        let (p2, f2) = furnace.withdraw_experience();
        assert_eq!(p2, 0);
        assert_eq!(f2, 0.0);
    }

    #[test]
    fn test_set_container_data_packet_encoding_for_all_4_properties() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.lit_time = 1200;
        furnace.lit_duration = 1600;
        furnace.cooking_progress = 75;
        furnace.cooking_total_time = 200;

        let window_id = 2;

        // Property 0: LitTime
        let pkt0 = furnace.create_property_packet(window_id, FurnaceProperty::LitTime);
        assert_eq!(pkt0.window_id(), 2);
        assert_eq!(pkt0.property(), 0);
        assert_eq!(pkt0.value(), 1200);

        let encoded0 = pkt0.encode_packet();
        // Expect: [0x13, window_id: 2, property: 0x0000, value: 0x04b0 (1200)]
        assert_eq!(encoded0, vec![0x13, 2, 0, 0, 0x04, 0xb0]);
        let decoded0 = SetContainerDataPacket::decode_packet(&encoded0).unwrap();
        assert_eq!(decoded0, pkt0);

        // Property 1: LitDuration
        let pkt1 = furnace.create_property_packet(window_id, FurnaceProperty::LitDuration);
        assert_eq!(pkt1.property(), 1);
        assert_eq!(pkt1.value(), 1600);
        let encoded1 = pkt1.encode_packet();
        // 1600 = 0x0640
        assert_eq!(encoded1, vec![0x13, 2, 0, 1, 0x06, 0x40]);
        let decoded1 = SetContainerDataPacket::decode_packet(&encoded1).unwrap();
        assert_eq!(decoded1, pkt1);

        // Property 2: CookingProgress
        let pkt2 = furnace.create_property_packet(window_id, FurnaceProperty::CookingProgress);
        assert_eq!(pkt2.property(), 2);
        assert_eq!(pkt2.value(), 75);
        let encoded2 = pkt2.encode_packet();
        assert_eq!(encoded2, vec![0x13, 2, 0, 2, 0x00, 75]);
        let decoded2 = SetContainerDataPacket::decode_packet(&encoded2).unwrap();
        assert_eq!(decoded2, pkt2);

        // Property 3: CookingTotalTime
        let pkt3 = furnace.create_property_packet(window_id, FurnaceProperty::CookingTotalTime);
        assert_eq!(pkt3.property(), 3);
        assert_eq!(pkt3.value(), 200);
        let encoded3 = pkt3.encode_packet();
        assert_eq!(encoded3, vec![0x13, 2, 0, 3, 0x00, 200]);
        let decoded3 = SetContainerDataPacket::decode_packet(&encoded3).unwrap();
        assert_eq!(decoded3, pkt3);

        // Framed packet round-trip
        let framed0 = pkt0.encode_framed();
        // Framed has length prefix (6 bytes payload -> length = 6)
        assert_eq!(framed0, vec![6, 0x13, 2, 0, 0, 0x04, 0xb0]);
        let decoded_framed = SetContainerDataPacket::decode_framed(&framed0).unwrap();
        assert_eq!(decoded_framed, pkt0);
    }

    #[test]
    fn test_non_fuel_rejection_and_fuel_burn_times() {
        assert_eq!(fuel_burn_time("coal"), 1600);
        assert_eq!(fuel_burn_time("charcoal"), 1600);
        assert_eq!(fuel_burn_time("oak_planks"), 300);
        assert_eq!(fuel_burn_time("spruce_log"), 300);
        assert_eq!(fuel_burn_time("oak_slab"), 150);
        assert_eq!(fuel_burn_time("stick"), 100);
        assert_eq!(fuel_burn_time("lava_bucket"), 20000);

        // Non-fuels
        assert_eq!(fuel_burn_time("dirt"), 0);
        assert_eq!(fuel_burn_time("stone"), 0);
        assert_eq!(fuel_burn_time("stone_slab"), 0);
        assert_eq!(fuel_burn_time("iron_ingot"), 0);
        for non_flammable in [
            "crimson_planks",
            "warped_slab",
            "crimson_stem",
            "stripped_warped_hyphae",
        ] {
            assert_eq!(fuel_burn_time(non_flammable), 0, "{non_flammable}");
        }
        assert_eq!(fuel_burn_time("pale_oak_slab"), 150);
        assert_eq!(fuel_burn_time("poplar_log"), 300);

        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(stack("minecraft:raw_iron", 1));
        furnace.fuel = Some(stack("minecraft:dirt", 64));

        furnace.tick();
        assert_eq!(furnace.lit_time, 0);
        assert_eq!(furnace.cooking_progress, 0);
        assert_eq!(furnace.fuel.as_ref().unwrap().count(), 64); // Untouched
    }

    #[test]
    fn test_non_26_3_generic_wood_ids_are_not_fuel() {
        for fake in [
            "planks",
            "log",
            "wooden_slab",
            "fake_planks",
            "fake_log",
            "fake_wood",
            "petrified_oak_slab",
        ] {
            assert_eq!(fuel_burn_time(fake), 0, "{fake}");
        }
        assert_eq!(fuel_burn_time("oak_planks"), 300);
        assert_eq!(fuel_burn_time("oak_log"), 300);
        assert_eq!(fuel_burn_time("oak_slab"), 150);
        assert_eq!(fuel_burn_time("pale_oak_planks"), 300);
        assert_eq!(fuel_burn_time("poplar_planks"), 300);
        assert_eq!(fuel_burn_time("stripped_poplar_wood"), 300);
    }

    #[test]
    fn test_fuel_registry_validates_remainder_before_inserting() {
        let mut registry = FuelRegistry::new();
        let previous = registry.burn_time("minecraft:magma_block");
        assert_eq!(
            registry.register("minecraft:magma_block", 100, Some("not an id".into())),
            Err(ItemStackError::InvalidItem)
        );
        assert_eq!(registry.burn_time("minecraft:magma_block"), previous);

        assert_eq!(
            registry.register(
                "minecraft:magma_block",
                100,
                Some("minecraft:bucket".into())
            ),
            Ok(())
        );
        assert_eq!(registry.burn_time("minecraft:magma_block"), 100);
        assert_eq!(
            registry.remainder("minecraft:magma_block"),
            Some(stack("minecraft:bucket", 1))
        );
        assert_eq!(
            registry.register("minecraft:magma_block", 999, Some("not an id".into())),
            Err(ItemStackError::InvalidItem)
        );
        assert_eq!(registry.burn_time("minecraft:magma_block"), 100);
        assert_eq!(
            registry.remainder("minecraft:magma_block"),
            Some(stack("minecraft:bucket", 1))
        );
        assert_eq!(
            registry.remainder("minecraft:lava_bucket"),
            Some(stack("minecraft:bucket", 1))
        );
    }

    #[test]
    fn test_multiple_smelting_recipes_evaluation() {
        let registry = SmeltingRecipeRegistry::new();

        let recipes_to_test = [
            ("raw_iron", "minecraft:iron_ingot", 0.7),
            ("iron_ore", "minecraft:iron_ingot", 0.7),
            ("raw_gold", "minecraft:gold_ingot", 1.0),
            ("gold_ore", "minecraft:gold_ingot", 1.0),
            ("raw_copper", "minecraft:copper_ingot", 0.7),
            ("cobblestone", "minecraft:stone", 0.1),
            ("sand", "minecraft:glass", 0.1),
            ("beef", "minecraft:cooked_beef", 0.35),
            ("porkchop", "minecraft:cooked_porkchop", 0.35),
        ];

        for (input, expected_out, expected_xp) in recipes_to_test {
            let rec = registry.find_recipe(input).expect(input);
            assert_eq!(rec.output.item(), expected_out);
            assert!((rec.experience - expected_xp).abs() < 1e-4);
            assert_eq!(rec.cooking_time, 200);
            assert!(rec.input().contains(':'));
            assert_eq!(rec.output().count(), 1);
            assert!((rec.experience() - expected_xp).abs() < 1e-4);
            assert_eq!(rec.cooking_time(), 200);
        }

        assert!(registry.find_recipe("diamond").is_none());
        assert!(registry.find_recipe("raw_beef").is_none());
        assert!(registry.find_recipe("raw_porkchop").is_none());
    }

    #[test]
    fn test_builtin_recipes_produce_valid_one_item_stacks() {
        let registry = SmeltingRecipeRegistry::new();
        assert!(!registry.recipes.is_empty());
        for recipe in registry.recipes.values() {
            assert_eq!(recipe.output().count(), 1);
            assert_eq!(
                recipe.output().max_stack(),
                default_max_stack(recipe.output().item())
            );
            assert_eq!(
                ItemStack::try_new(
                    recipe.output().item(),
                    recipe.output().count(),
                    recipe.output().max_stack(),
                ),
                Ok(recipe.output().clone())
            );
        }
    }

    #[test]
    fn test_smelting_recipe_rejects_unnamespaced_output() {
        assert_eq!(
            SmeltingRecipe::new("minecraft:a", "no_namespace", 0.1, 200),
            Err(ItemStackError::InvalidItem)
        );
    }

    #[test]
    fn test_output_type_mismatch_blocks_smelting() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(stack("minecraft:raw_gold", 2));
        furnace.fuel = Some(stack("minecraft:coal", 1));
        furnace.output = Some(stack("minecraft:iron_ingot", 1)); // Gold cannot go into iron stack

        furnace.tick();
        assert_eq!(furnace.cooking_progress, 0);
        assert_eq!(furnace.lit_time, 0);
        assert_eq!(furnace.output.as_ref().unwrap().count(), 1);
        assert_eq!(
            furnace.output.as_ref().unwrap().item(),
            "minecraft:iron_ingot"
        );
    }

    #[test]
    fn test_property_values_saturate_at_the_short_range() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.lit_time = 40_000;
        let packet = furnace.create_property_packet(1, FurnaceProperty::LitTime);
        assert_eq!(packet.value(), i16::MAX);
    }

    #[test]
    fn test_progress_is_undone_at_double_speed_when_fuel_runs_out() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(stack("minecraft:raw_iron", 1));
        furnace.lit_time = 6;
        furnace.lit_duration = 100;
        for _ in 0..5 {
            furnace.tick();
        }
        assert_eq!(furnace.cooking_progress, 5);

        furnace.tick();
        assert_eq!(furnace.lit_time, 0);
        assert_eq!(furnace.cooking_progress, 3);
        furnace.tick();
        assert_eq!(furnace.cooking_progress, 1);
        furnace.tick();
        assert_eq!(furnace.cooking_progress, 0);
    }

    #[test]
    fn test_swapping_the_input_restarts_the_cook() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(stack("minecraft:raw_iron", 4));
        furnace.fuel = Some(stack("minecraft:coal", 1));
        for _ in 0..50 {
            furnace.tick();
        }
        assert_eq!(furnace.cooking_progress, 50);

        furnace.input = Some(stack("minecraft:raw_gold", 4));
        furnace.tick();
        assert_eq!(furnace.cooking_progress, 1);
    }

    #[test]
    fn test_full_cook_pins_output_item_count_and_xp() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(stack("minecraft:beef", 2));
        furnace.fuel = Some(stack("minecraft:coal", 1));

        for _ in 0..(DEFAULT_COOKING_TOTAL_TIME * 2) {
            furnace.tick();
        }

        assert!(furnace.input.is_none());
        let output = furnace.output.as_ref().expect("cooked beef output");
        assert_eq!(output.item(), "minecraft:cooked_beef");
        assert_eq!(output.count(), 2);
        assert!((furnace.stored_experience() - (MEAT_SMELT_XP * 2.0)).abs() < 1e-6);
    }

    #[test]
    fn test_smelting_registry_from_recipe_data_keeps_count_above_one_and_isolates_kinds() {
        use crate::vanilla::feature::ItemTags;
        use std::fs;
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("rustmc-furnace-recipes-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |rel: &str, text: &str| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            fs::write(p, text).expect("write");
        };

        write(
            "data/testns/tags/item/ores.json",
            r#"{"values": ["testns:ore_a", "testns:ore_b"]}"#,
        );
        write(
            "data/testns/recipe/double_ingot.json",
            r##"{
                "type": "minecraft:smelting",
                "ingredient": "#testns:ores",
                "result": {"id": "testns:ingot", "count": 2},
                "experience": 0.4,
                "cookingtime": 5
            }"##,
        );
        write(
            "data/testns/recipe/smoke_only.json",
            r#"{
                "type": "minecraft:smoking",
                "ingredient": "testns:raw_food",
                "result": {"id": "testns:cooked_food"},
                "experience": 0.25,
                "cookingtime": 4
            }"#,
        );

        let tags = ItemTags::load(&root).expect("load item tags");
        let data = RecipeData::load(&root, &tags).expect("load recipe data");
        let smelting = SmeltingRecipeRegistry::from_recipe_data(&data, CookingKind::Smelting);
        let smoking = SmeltingRecipeRegistry::from_recipe_data(&data, CookingKind::Smoking);
        let blasting = SmeltingRecipeRegistry::from_recipe_data(&data, CookingKind::Blasting);

        assert!(smelting.find_recipe("testns:ore_a").is_some());
        assert!(smelting.find_recipe("testns:ore_b").is_some());
        assert!(smelting.find_recipe("testns:raw_food").is_none());
        assert!(smoking.find_recipe("testns:raw_food").is_some());
        assert!(blasting.find_recipe("testns:ore_a").is_none());
        assert!(blasting.find_recipe("testns:raw_food").is_none());

        let fuels = FuelRegistry::new();
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(stack("testns:ore_a", 2));
        furnace.fuel = Some(stack("minecraft:coal", 1));

        for _ in 0..5 {
            furnace.tick_with_registries(&fuels, &smelting);
        }
        assert_eq!(furnace.output.as_ref().expect("first cook").count(), 2);
        assert!((furnace.stored_experience() - 0.4).abs() < 1e-6);

        for _ in 0..5 {
            furnace.tick_with_registries(&fuels, &smelting);
        }
        assert_eq!(furnace.output.as_ref().expect("second cook").count(), 4);
        assert!((furnace.stored_experience() - 0.8).abs() < 1e-6);

        // Output stack at 63 cannot fit a 2-item recipe result (63 + 2 > 64).
        furnace.input = Some(stack("testns:ore_b", 1));
        furnace.output = Some(stack("testns:ingot", 63));
        furnace.tick_with_registries(&fuels, &smelting);
        assert_eq!(furnace.cooking_progress, 0);
        assert_eq!(furnace.output.as_ref().expect("blocked").count(), 63);

        fs::remove_dir_all(root).expect("cleanup");
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_furnace_registries_cook_vanilla_recipes`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_furnace_registries_cook_vanilla_recipes() {
        use crate::vanilla::feature::ItemTags;
        use crate::vanilla::worldgen::read_json;
        use std::path::PathBuf;

        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let tags = ItemTags::load(&root).expect("load item tags");
        let data = RecipeData::load(&root, &tags).expect("load recipes");
        let fuels = FuelRegistry::new();

        let gold_json =
            read_json(&root.join("data/minecraft/recipe/gold_ingot_from_smelting_raw_gold.json"))
                .expect("read gold smelting json");
        let gold_time = gold_json["cookingtime"].as_u64().expect("gold cookingtime") as u16;
        let gold_xp = gold_json["experience"].as_f64().expect("gold xp") as f32;
        let gold_out = gold_json["result"]["id"].as_str().expect("gold result id");

        let smelting = SmeltingRecipeRegistry::from_recipe_data(&data, CookingKind::Smelting);
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(stack("minecraft:raw_gold", 1));
        furnace.fuel = Some(stack("minecraft:coal", 1));
        for _ in 0..(gold_time - 1) {
            furnace.tick_with_registries(&fuels, &smelting);
            assert!(furnace.output.is_none());
        }
        furnace.tick_with_registries(&fuels, &smelting);
        let out = furnace.output.as_ref().expect("smelted gold");
        assert_eq!(out.item(), gold_out);
        assert_eq!(out.count(), 1);
        assert!((furnace.stored_experience() - gold_xp).abs() < 1e-6);

        let beef_json =
            read_json(&root.join("data/minecraft/recipe/cooked_beef_from_smoking.json"))
                .expect("read beef smoking json");
        let beef_time = beef_json["cookingtime"].as_u64().expect("beef cookingtime") as u16;
        let beef_xp = beef_json["experience"].as_f64().expect("beef xp") as f32;
        let beef_out = beef_json["result"]["id"].as_str().expect("beef result id");

        let smoking = SmeltingRecipeRegistry::from_recipe_data(&data, CookingKind::Smoking);
        let mut smoker = FurnaceBlockEntity::new();
        smoker.input = Some(stack("minecraft:beef", 1));
        smoker.fuel = Some(stack("minecraft:coal", 1));
        for _ in 0..(beef_time - 1) {
            smoker.tick_with_registries(&fuels, &smoking);
            assert!(smoker.output.is_none());
        }
        smoker.tick_with_registries(&fuels, &smoking);
        let beef_stack = smoker.output.as_ref().expect("smoked beef");
        assert_eq!(beef_stack.item(), beef_out);
        assert_eq!(beef_stack.count(), 1);
        assert!((smoker.stored_experience() - beef_xp).abs() < 1e-6);

        let blasting = SmeltingRecipeRegistry::from_recipe_data(&data, CookingKind::Blasting);
        assert!(blasting.find_recipe("minecraft:beef").is_none());
    }
}
