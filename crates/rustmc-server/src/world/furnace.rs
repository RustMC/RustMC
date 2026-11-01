//! Milestone M4/M5 Furnace Smelting State Machine, Fuel Burning, and Container Data Sync.
//!
//! Provides authoritative furnace block entity state progression, fuel registry burn time
//! evaluation, smelting recipe evaluation, experience points accumulation and withdrawal,
//! and Java Edition 26.3 `SetContainerData` (packet ID `0x14`) network encoding.

use crate::discovery_java::{frame, put_varint, varint};
use std::collections::BTreeMap;
use std::fmt;

/// Standard vanilla maximum item stack count.
pub const DEFAULT_MAX_STACK: u16 = 64;

/// Default total cooking duration in ticks for a standard smelting recipe (200 ticks = 10s).
pub const DEFAULT_COOKING_TOTAL_TIME: u16 = 200;

/// Slot indices matching standard Minecraft furnace container layouts.
pub const INPUT_SLOT: usize = 0;
pub const FUEL_SLOT: usize = 1;
pub const OUTPUT_SLOT: usize = 2;
pub const TOTAL_SLOTS: usize = 3;

/// Java 26.3 Clientbound SetContainerData packet ID in the Play state.
pub const SET_CONTAINER_DATA_PACKET_ID: u32 = 0x14;

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

/// An item stack occupying a furnace slot or transferred during window interactions.
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

    #[inline]
    pub fn item(&self) -> &str {
        &self.item
    }

    #[inline]
    pub fn item_id(&self) -> &str {
        &self.item
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
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    #[inline]
    pub fn matches(&self, other: &Self) -> bool {
        items_equal(&self.item, &other.item)
    }

    #[inline]
    pub fn can_stack_with(&self, other: &Self) -> bool {
        self.matches(other) && self.max_stack == other.max_stack
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

/// Registered fuel entry with burn duration in ticks and optional residual item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuelEntry {
    pub burn_time: u16,
    pub remainder: Option<String>,
}

/// Evaluates fuel burn time and residual item for items according to official specifications:
/// - Coal / Charcoal: 1600 ticks (8 smelts).
/// - Wooden planks / logs: 300 ticks (1.5 smelts).
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
    ) {
        let item_id = item_id.into();
        let key = normalize_item_id(&item_id).to_string();
        self.custom.insert(
            key,
            FuelEntry {
                burn_time,
                remainder,
            },
        );
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
    pub fn remainder(&self, item_id: &str) -> Option<&str> {
        let normalized = normalize_item_id(item_id);
        if let Some(entry) = self.custom.get(normalized) {
            return entry.remainder.as_deref();
        }
        fuel_remainder(normalized)
    }

    /// Whether an item is recognized as valid fuel.
    #[must_use]
    pub fn is_fuel(&self, item_id: &str) -> bool {
        self.burn_time(item_id) > 0
    }
}

/// Returns the standard vanilla fuel burn duration in ticks for a given item identifier.
#[must_use]
pub fn fuel_burn_time(item_id: &str) -> u16 {
    let name = normalize_item_id(item_id);
    match name {
        // Coal and Charcoal: 1600 ticks (8 smelts)
        "coal" | "charcoal" => 1600,

        // Lava Bucket: 20000 ticks (100 smelts)
        "lava_bucket" => 20000,

        // Sticks: 100 ticks (0.5 smelts)
        "stick" => 100,

        // Wooden Slabs: 150 ticks
        "oak_slab" | "spruce_slab" | "birch_slab" | "jungle_slab" | "acacia_slab"
        | "dark_oak_slab" | "mangrove_slab" | "cherry_slab" | "bamboo_slab"
        | "bamboo_mosaic_slab" | "crimson_slab" | "warped_slab" | "wooden_slab" => 150,

        // Wooden Planks: 300 ticks (1.5 smelts)
        "oak_planks" | "spruce_planks" | "birch_planks" | "jungle_planks" | "acacia_planks"
        | "dark_oak_planks" | "mangrove_planks" | "cherry_planks" | "bamboo_planks"
        | "crimson_planks" | "warped_planks" | "planks" => 300,

        // Wooden Logs & Wood: 300 ticks (1.5 smelts)
        "oak_log"
        | "spruce_log"
        | "birch_log"
        | "jungle_log"
        | "acacia_log"
        | "dark_oak_log"
        | "mangrove_log"
        | "cherry_log"
        | "stripped_oak_log"
        | "stripped_spruce_log"
        | "stripped_birch_log"
        | "stripped_jungle_log"
        | "stripped_acacia_log"
        | "stripped_dark_oak_log"
        | "stripped_mangrove_log"
        | "stripped_cherry_log"
        | "oak_wood"
        | "spruce_wood"
        | "birch_wood"
        | "jungle_wood"
        | "acacia_wood"
        | "dark_oak_wood"
        | "mangrove_wood"
        | "cherry_wood"
        | "stripped_oak_wood"
        | "stripped_spruce_wood"
        | "stripped_birch_wood"
        | "stripped_jungle_wood"
        | "stripped_acacia_wood"
        | "stripped_dark_oak_wood"
        | "stripped_mangrove_wood"
        | "stripped_cherry_wood"
        | "bamboo_block"
        | "stripped_bamboo_block"
        | "crimson_stem"
        | "warped_stem"
        | "stripped_crimson_stem"
        | "stripped_warped_stem"
        | "crimson_hyphae"
        | "warped_hyphae"
        | "stripped_crimson_hyphae"
        | "stripped_warped_hyphae"
        | "log" => 300,

        _ => {
            // General wooden slab check (excluding stone/sandstone/brick slabs)
            if name.ends_with("_slab")
                && (name.starts_with("oak_")
                    || name.starts_with("spruce_")
                    || name.starts_with("birch_")
                    || name.starts_with("jungle_")
                    || name.starts_with("acacia_")
                    || name.starts_with("dark_oak_")
                    || name.starts_with("mangrove_")
                    || name.starts_with("cherry_")
                    || name.starts_with("bamboo_")
                    || name.starts_with("crimson_")
                    || name.starts_with("warped_")
                    || name.contains("wood"))
            {
                150
            } else if name.ends_with("_planks")
                || ((name.ends_with("_log") || name.ends_with("_wood"))
                    && !name.contains("stone")
                    && !name.contains("copper"))
            {
                300
            } else {
                0
            }
        }
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
    pub input: String,
    pub output: String,
    pub experience: f32,
    pub cooking_time: u16,
}

impl SmeltingRecipe {
    /// Creates a new smelting recipe.
    pub fn new(
        input: impl Into<String>,
        output: impl Into<String>,
        experience: f32,
        cooking_time: u16,
    ) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
            experience,
            cooking_time,
        }
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
    /// Constructs a new recipe registry pre-populated with standard vanilla smelting recipes:
    /// - Raw Iron / Iron Ore -> Iron Ingot (0.7 XP).
    /// - Raw Gold / Gold Ore -> Gold Ingot (1.0 XP).
    /// - Raw Copper -> Copper Ingot (0.7 XP).
    /// - Cobblestone -> Stone (0.1 XP).
    /// - Sand -> Glass (0.1 XP).
    /// - Raw Beef -> Cooked Beef (0.35 XP).
    /// - Raw Porkchop -> Cooked Porkchop (0.35 XP).
    pub fn new() -> Self {
        let mut registry = Self {
            recipes: BTreeMap::new(),
        };

        // Iron recipes
        registry.register(SmeltingRecipe::new(
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
            0.7,
            DEFAULT_COOKING_TOTAL_TIME,
        ));
        registry.register(SmeltingRecipe::new(
            "minecraft:iron_ore",
            "minecraft:iron_ingot",
            0.7,
            DEFAULT_COOKING_TOTAL_TIME,
        ));
        registry.register(SmeltingRecipe::new(
            "minecraft:deepslate_iron_ore",
            "minecraft:iron_ingot",
            0.7,
            DEFAULT_COOKING_TOTAL_TIME,
        ));

        // Gold recipes
        registry.register(SmeltingRecipe::new(
            "minecraft:raw_gold",
            "minecraft:gold_ingot",
            1.0,
            DEFAULT_COOKING_TOTAL_TIME,
        ));
        registry.register(SmeltingRecipe::new(
            "minecraft:gold_ore",
            "minecraft:gold_ingot",
            1.0,
            DEFAULT_COOKING_TOTAL_TIME,
        ));
        registry.register(SmeltingRecipe::new(
            "minecraft:deepslate_gold_ore",
            "minecraft:gold_ingot",
            1.0,
            DEFAULT_COOKING_TOTAL_TIME,
        ));
        registry.register(SmeltingRecipe::new(
            "minecraft:nether_gold_ore",
            "minecraft:gold_ingot",
            1.0,
            DEFAULT_COOKING_TOTAL_TIME,
        ));

        // Copper recipes
        registry.register(SmeltingRecipe::new(
            "minecraft:raw_copper",
            "minecraft:copper_ingot",
            0.7,
            DEFAULT_COOKING_TOTAL_TIME,
        ));
        registry.register(SmeltingRecipe::new(
            "minecraft:copper_ore",
            "minecraft:copper_ingot",
            0.7,
            DEFAULT_COOKING_TOTAL_TIME,
        ));
        registry.register(SmeltingRecipe::new(
            "minecraft:deepslate_copper_ore",
            "minecraft:copper_ingot",
            0.7,
            DEFAULT_COOKING_TOTAL_TIME,
        ));

        // Stone recipes
        registry.register(SmeltingRecipe::new(
            "minecraft:cobblestone",
            "minecraft:stone",
            0.1,
            DEFAULT_COOKING_TOTAL_TIME,
        ));

        // Glass recipes
        registry.register(SmeltingRecipe::new(
            "minecraft:sand",
            "minecraft:glass",
            0.1,
            DEFAULT_COOKING_TOTAL_TIME,
        ));
        registry.register(SmeltingRecipe::new(
            "minecraft:red_sand",
            "minecraft:glass",
            0.1,
            DEFAULT_COOKING_TOTAL_TIME,
        ));

        // Food recipes (supports canonical beef and raw_beef alias)
        registry.register(SmeltingRecipe::new(
            "minecraft:beef",
            "minecraft:cooked_beef",
            0.35,
            DEFAULT_COOKING_TOTAL_TIME,
        ));
        registry.register(SmeltingRecipe::new(
            "minecraft:raw_beef",
            "minecraft:cooked_beef",
            0.35,
            DEFAULT_COOKING_TOTAL_TIME,
        ));
        registry.register(SmeltingRecipe::new(
            "minecraft:porkchop",
            "minecraft:cooked_porkchop",
            0.35,
            DEFAULT_COOKING_TOTAL_TIME,
        ));
        registry.register(SmeltingRecipe::new(
            "minecraft:raw_porkchop",
            "minecraft:cooked_porkchop",
            0.35,
            DEFAULT_COOKING_TOTAL_TIME,
        ));

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
    pub input: Option<ItemStack>,
    pub fuel: Option<ItemStack>,
    pub output: Option<ItemStack>,
    pub lit_time: u16,
    pub lit_duration: u16,
    pub cooking_progress: u16,
    pub cooking_total_time: u16,
    pub stored_experience: f32,
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
        let fuels = FuelRegistry::new();
        let recipes = SmeltingRecipeRegistry::new();
        self.tick_with_registries(&fuels, &recipes);
    }

    /// Executes one authoritative tick using custom fuel and recipe registries.
    pub fn tick_with_registries(&mut self, fuels: &FuelRegistry, recipes: &SmeltingRecipeRegistry) {
        // Step 1: Lit time decrements each tick if burning.
        if self.lit_time > 0 {
            self.lit_time -= 1;
        }

        // Step 2: Evaluate matching cookable recipe and output slot capacity.
        let recipe = self
            .input
            .as_ref()
            .and_then(|inp| recipes.find_recipe(inp.item()));

        let can_cook = if let Some(rec) = recipe {
            self.cooking_total_time = rec.cooking_time;
            match &self.output {
                None => true,
                Some(out) => items_equal(out.item(), &rec.output) && out.count < out.max_stack,
            }
        } else {
            false
        };

        if can_cook {
            // Step 3a: If unlit and cookable, attempt to consume 1 fuel from slot 1.
            if self.lit_time == 0
                && let Some(fuel_stack) = &mut self.fuel
            {
                let burn = fuels.burn_time(fuel_stack.item());
                if burn > 0 {
                    let remainder = fuels.remainder(fuel_stack.item()).map(str::to_owned);

                    if fuel_stack.count <= 1 {
                        if let Some(rem) = remainder {
                            self.fuel = Some(ItemStack::new(rem, 1));
                        } else {
                            self.fuel = None;
                        }
                    } else {
                        fuel_stack.count -= 1;
                    }

                    self.lit_time = burn;
                    self.lit_duration = burn;
                }
            }

            // Step 3b: If lit, advance cooking progress.
            if self.lit_time > 0 {
                self.cooking_progress = self.cooking_progress.saturating_add(1);

                // Completion check
                if self.cooking_progress >= self.cooking_total_time {
                    let rec = recipe.unwrap();

                    // Decrement 1 input from slot 0
                    if let Some(inp) = &mut self.input {
                        if inp.count <= 1 {
                            self.input = None;
                        } else {
                            inp.count -= 1;
                        }
                    }

                    // Increment output stack by 1 in slot 2
                    match &mut self.output {
                        Some(out) => {
                            out.count = out.count.saturating_add(1);
                        }
                        None => {
                            self.output = Some(ItemStack::new(&rec.output, 1));
                        }
                    }

                    // Accumulate XP yield
                    self.stored_experience += rec.experience;

                    // Reset cooking progress
                    self.cooking_progress = 0;
                }
            } else {
                // If fuel could not be consumed (or ran out), reset cooking progress
                self.cooking_progress = 0;
            }
        } else {
            // Step 4: If no valid recipe or output is blocked, reset cooking progress
            self.cooking_progress = 0;
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
        SetContainerDataPacket {
            window_id,
            property: prop.index(),
            value: self.property_value(prop) as i16,
        }
    }

    /// Encodes an unframed `SetContainerData` (0x14) packet payload for a given property.
    #[must_use]
    pub fn encode_container_data(&self, window_id: u8, prop: FurnaceProperty) -> Vec<u8> {
        self.create_property_packet(window_id, prop).encode_packet()
    }

    /// Encodes a framed `SetContainerData` (0x14) packet for a given property.
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

/// Java 26.3 `SetContainerData` (0x14) Clientbound packet representation.
///
/// Contains:
/// - `window_id`: Unsigned Byte (`u8`).
/// - `property`: Short (`u16`).
/// - `value`: Short (`i16`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
/// `[0x14, window_id, property (2 bytes BE), value (2 bytes BE)]`.
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

    let payload = &framed[len_prefix..total];
    decode_set_container_data(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuel_burning_progression_and_decrementing_fuel_item() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(ItemStack::new("minecraft:raw_iron", 5));
        furnace.fuel = Some(ItemStack::new("minecraft:coal", 2));

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
        furnace.input = Some(ItemStack::new("minecraft:raw_iron", 1));
        furnace.fuel = Some(ItemStack::new("minecraft:lava_bucket", 1));

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
        furnace.input = Some(ItemStack::new("minecraft:raw_iron", 1));
        furnace.fuel = Some(ItemStack::new("minecraft:coal", 1));

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
    }

    #[test]
    fn test_cook_progress_reset_when_input_is_removed_mid_cook() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(ItemStack::new("minecraft:raw_iron", 4));
        furnace.fuel = Some(ItemStack::new("minecraft:coal", 1));

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
        furnace.input = Some(ItemStack::new("minecraft:raw_iron", 5));
        furnace.fuel = Some(ItemStack::new("minecraft:coal", 1));
        // Fill output slot to max stack (64)
        furnace.output = Some(ItemStack::new("minecraft:iron_ingot", 64));

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
        furnace.input = Some(ItemStack::new("minecraft:raw_iron", 2));
        furnace.fuel = Some(ItemStack::new("minecraft:coal", 2));

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
        furnace.input = Some(ItemStack::new("minecraft:raw_gold", 1));
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
        assert_eq!(pkt0.window_id, 2);
        assert_eq!(pkt0.property, 0);
        assert_eq!(pkt0.value, 1200);

        let encoded0 = pkt0.encode_packet();
        // Expect: [0x14, window_id: 2, property: 0x0000, value: 0x04b0 (1200)]
        assert_eq!(encoded0, vec![0x14, 2, 0, 0, 0x04, 0xb0]);
        let decoded0 = SetContainerDataPacket::decode_packet(&encoded0).unwrap();
        assert_eq!(decoded0, pkt0);

        // Property 1: LitDuration
        let pkt1 = furnace.create_property_packet(window_id, FurnaceProperty::LitDuration);
        assert_eq!(pkt1.property, 1);
        assert_eq!(pkt1.value, 1600);
        let encoded1 = pkt1.encode_packet();
        // 1600 = 0x0640
        assert_eq!(encoded1, vec![0x14, 2, 0, 1, 0x06, 0x40]);
        let decoded1 = SetContainerDataPacket::decode_packet(&encoded1).unwrap();
        assert_eq!(decoded1, pkt1);

        // Property 2: CookingProgress
        let pkt2 = furnace.create_property_packet(window_id, FurnaceProperty::CookingProgress);
        assert_eq!(pkt2.property, 2);
        assert_eq!(pkt2.value, 75);
        let encoded2 = pkt2.encode_packet();
        assert_eq!(encoded2, vec![0x14, 2, 0, 2, 0x00, 75]);
        let decoded2 = SetContainerDataPacket::decode_packet(&encoded2).unwrap();
        assert_eq!(decoded2, pkt2);

        // Property 3: CookingTotalTime
        let pkt3 = furnace.create_property_packet(window_id, FurnaceProperty::CookingTotalTime);
        assert_eq!(pkt3.property, 3);
        assert_eq!(pkt3.value, 200);
        let encoded3 = pkt3.encode_packet();
        assert_eq!(encoded3, vec![0x14, 2, 0, 3, 0x00, 200]);
        let decoded3 = SetContainerDataPacket::decode_packet(&encoded3).unwrap();
        assert_eq!(decoded3, pkt3);

        // Framed packet round-trip
        let framed0 = pkt0.encode_framed();
        // Framed has length prefix (6 bytes payload -> length = 6)
        assert_eq!(framed0, vec![6, 0x14, 2, 0, 0, 0x04, 0xb0]);
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

        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(ItemStack::new("minecraft:raw_iron", 1));
        furnace.fuel = Some(ItemStack::new("minecraft:dirt", 64));

        furnace.tick();
        assert_eq!(furnace.lit_time, 0);
        assert_eq!(furnace.cooking_progress, 0);
        assert_eq!(furnace.fuel.as_ref().unwrap().count(), 64); // Untouched
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
            ("raw_beef", "minecraft:cooked_beef", 0.35),
            ("porkchop", "minecraft:cooked_porkchop", 0.35),
            ("raw_porkchop", "minecraft:cooked_porkchop", 0.35),
        ];

        for (input, expected_out, expected_xp) in recipes_to_test {
            let rec = registry.find_recipe(input).expect(input);
            assert_eq!(rec.output, expected_out);
            assert!((rec.experience - expected_xp).abs() < 1e-4);
            assert_eq!(rec.cooking_time, 200);
        }

        assert!(registry.find_recipe("diamond").is_none());
    }

    #[test]
    fn test_output_type_mismatch_blocks_smelting() {
        let mut furnace = FurnaceBlockEntity::new();
        furnace.input = Some(ItemStack::new("minecraft:raw_gold", 2));
        furnace.fuel = Some(ItemStack::new("minecraft:coal", 1));
        furnace.output = Some(ItemStack::new("minecraft:iron_ingot", 1)); // Gold cannot go into iron stack

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
    fn test_packet_decoder_error_cases() {
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
}
