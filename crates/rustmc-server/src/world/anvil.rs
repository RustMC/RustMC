//! Milestone M4: Anvil Repairing, Renaming, and Item Durability Combination Logic.
//!
//! Provides authoritative anvil repair models for tool/armor durability combination,
//! material unit repairs (such as iron ingots or diamonds), item renaming with validation,
//! prior work penalty escalation (`2*penalty + 1`), and state degradation progression
//! (`Undamaged` -> `Chipped` -> `Damaged` -> `Broken`) with a 12% degradation chance.

use std::fmt;

/// Maximum allowable characters for an item's custom name in an anvil text box.
pub const MAX_NAME_LENGTH: usize = 50;

/// Experience level cost cap in Survival mode ("Too Expensive!" at 40 levels).
pub const SURVIVAL_LEVEL_CAP: u32 = 40;

/// Probability of anvil degradation upon completing a repair (12%).
pub const DEGRADATION_CHANCE: f64 = 0.12;

/// Bonus factor applied to max durability during same-item combination (12%).
pub const DURABILITY_BONUS_FACTOR: f64 = 0.12;

/// Fraction of max durability restored per unit of repair material (25%).
pub const MATERIAL_REPAIR_FRACTION: f64 = 0.25;

/// Cost in experience levels to rename an item when the name is modified.
pub const RENAME_LEVEL_COST: u32 = 1;

/// Cost in experience levels for tool durability combination.
pub const COMBINE_REPAIR_LEVEL_COST: u32 = 2;

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

/// Strips optional `minecraft:` namespace prefix for canonical item comparison.
#[inline]
pub fn normalize_item_id(item: &str) -> &str {
    item.strip_prefix("minecraft:").unwrap_or(item)
}

/// Returns true if two item identifiers are canonically identical.
#[inline]
pub fn items_equal(a: &str, b: &str) -> bool {
    normalize_item_id(a) == normalize_item_id(b)
}

/// Returns the standard vanilla max durability for a known tool, weapon, or armor piece.
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
        _ => None,
    }
}

/// Checks whether `material` is a valid repair ingredient for `tool`.
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
        return matches!(m, "turtle_scute" | "scute");
    }
    if t == "elytra" {
        return m == "phantom_membrane";
    }
    if t == "mace" {
        return m == "breeze_rod";
    }

    false
}

/// An item stack representing an item identifier, stack count, durability, prior work penalty,
/// and optional custom name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemStack {
    pub item: String,
    pub count: u32,
    pub durability: u32,
    pub max_durability: u32,
    pub repair_cost: u32,
    pub custom_name: Option<String>,
}

impl ItemStack {
    /// Creates a new `ItemStack`. If the item has a known default max durability, it is
    /// populated automatically with full durability and zero repair penalty.
    pub fn new(item: impl Into<String>, count: u32) -> Self {
        let item = item.into();
        let max_durability = default_max_durability(&item).unwrap_or(0);
        Self {
            item,
            count,
            durability: max_durability,
            max_durability,
            repair_cost: 0,
            custom_name: None,
        }
    }

    /// Sets the current remaining durability.
    #[must_use]
    pub fn with_durability(mut self, durability: u32) -> Self {
        if self.max_durability == 0 {
            self.max_durability = durability;
        }
        self.durability = durability.min(self.max_durability);
        self
    }

    /// Sets the maximum durability.
    #[must_use]
    pub fn with_max_durability(mut self, max_durability: u32) -> Self {
        self.max_durability = max_durability;
        self.durability = self.durability.min(max_durability);
        self
    }

    /// Sets the prior work penalty (`repair_cost`).
    #[must_use]
    pub fn with_repair_cost(mut self, repair_cost: u32) -> Self {
        self.repair_cost = repair_cost;
        self
    }

    /// Sets the custom display name.
    #[must_use]
    pub fn with_custom_name(mut self, name: impl Into<String>) -> Self {
        self.custom_name = Some(name.into());
        self
    }

    /// Returns damage taken (missing durability).
    #[must_use]
    pub fn damage(&self) -> u32 {
        self.max_durability.saturating_sub(self.durability)
    }

    /// Returns whether this item is damageable (`max_durability > 0`).
    #[must_use]
    pub fn is_damageable(&self) -> bool {
        self.max_durability > 0
    }

    /// Returns whether this item has taken damage (`durability < max_durability`).
    #[must_use]
    pub fn is_damaged(&self) -> bool {
        self.is_damageable() && self.durability < self.max_durability
    }

    /// Returns whether this item stack is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0 || self.item.trim().is_empty()
    }
}

/// Input parameters for an anvil operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnvilRepairInput {
    pub base_item: ItemStack,
    pub sacrifice_item: Option<ItemStack>,
    pub custom_name: Option<String>,
}

impl AnvilRepairInput {
    /// Creates a new `AnvilRepairInput`, validating custom name length (max 50 chars).
    pub fn new(
        base_item: ItemStack,
        sacrifice_item: Option<ItemStack>,
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
        base_item: ItemStack,
        custom_name: impl Into<String>,
    ) -> Result<Self, AnvilError> {
        Self::new(base_item, None, Some(custom_name.into()))
    }

    /// Helper to create a same-item or material combination anvil input.
    pub fn combine(base_item: ItemStack, sacrifice_item: ItemStack) -> Self {
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
    pub result_item: ItemStack,
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
    /// Base item stack is empty or missing.
    EmptyBaseItem,
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
            Self::EmptyBaseItem => write!(f, "base item stack is empty"),
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

/// Calculates and executes the anvil operation with full validation, durability combination math,
/// level calculation, and deterministic degradation roll.
fn calculate_repair_internal(
    input: &AnvilRepairInput,
    anvil_state: AnvilState,
    is_creative: bool,
    roll: Option<f64>,
) -> Result<AnvilRepairResult, AnvilError> {
    if anvil_state.is_broken() {
        return Err(AnvilError::AnvilBroken);
    }
    if input.base_item.is_empty() {
        return Err(AnvilError::EmptyBaseItem);
    }

    // Custom rename validation (max 50 chars, trimmed, blank string reset)
    let mut rename_changed = false;
    let mut final_custom_name = input.base_item.custom_name.clone();

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
            if input.base_item.custom_name.is_some() {
                final_custom_name = None;
                rename_changed = true;
            }
        } else if input.base_item.custom_name.as_deref() != Some(trimmed) {
            final_custom_name = Some(trimmed.to_string());
            rename_changed = true;
        }
    }

    let rename_level_cost = if rename_changed { RENAME_LEVEL_COST } else { 0 };

    // Evaluate repair operation
    let mut result_durability = input.base_item.durability;
    let mut repair_cost_levels = 0;
    let mut material_cost = 0;
    let mut sacrifice_penalty = 0;

    match &input.sacrifice_item {
        None => {
            if !rename_changed {
                return Err(AnvilError::NoOperation);
            }
        }
        Some(sacrifice) if sacrifice.is_empty() => {
            if !rename_changed {
                return Err(AnvilError::NoOperation);
            }
        }
        Some(sacrifice) => {
            let is_same_item = items_equal(&input.base_item.item, &sacrifice.item);
            let is_mat = is_repair_material(&input.base_item.item, &sacrifice.item);

            if !is_same_item && !is_mat {
                return Err(AnvilError::IncompatibleSacrificeItem);
            }

            if is_same_item {
                if !input.base_item.is_damageable() {
                    return Err(AnvilError::NotDamageable);
                }

                let max_durability = input.base_item.max_durability;
                let d1 = input.base_item.durability.min(max_durability);
                let d2 = sacrifice.durability.min(sacrifice.max_durability);
                let bonus = ((f64::from(max_durability)) * DURABILITY_BONUS_FACTOR).floor() as u32;
                let combined = d1
                    .saturating_add(d2)
                    .saturating_add(bonus)
                    .min(max_durability);

                if combined > d1 {
                    result_durability = combined;
                    repair_cost_levels = COMBINE_REPAIR_LEVEL_COST;
                    material_cost = 1;
                } else if !rename_changed {
                    return Err(AnvilError::NoRepairNeeded);
                }

                sacrifice_penalty = sacrifice.repair_cost;
            } else if is_mat {
                if !input.base_item.is_damageable() {
                    return Err(AnvilError::NotDamageable);
                }

                let max_durability = input.base_item.max_durability;
                let d1 = input.base_item.durability.min(max_durability);
                let missing = max_durability.saturating_sub(d1);

                if missing == 0 {
                    if !rename_changed {
                        return Err(AnvilError::NoRepairNeeded);
                    }
                } else {
                    let unit_repair =
                        ((f64::from(max_durability)) * MATERIAL_REPAIR_FRACTION).floor() as u32;
                    let unit_repair = unit_repair.max(1);

                    let mut mat_used = 0;
                    let mut cur_d = d1;
                    while cur_d < max_durability && mat_used < sacrifice.count && mat_used < 4 {
                        cur_d = cur_d.saturating_add(unit_repair).min(max_durability);
                        mat_used += 1;
                    }
                    if mat_used == 4 {
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
    let base_penalty = input.base_item.repair_cost;
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

    let mut result_item = input.base_item.clone();
    result_item.count = 1;
    result_item.durability = result_durability;
    result_item.repair_cost = next_prior_work_penalty;
    result_item.custom_name = final_custom_name;

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
pub fn calculate_repair(
    input: &AnvilRepairInput,
    is_creative: bool,
) -> Result<AnvilRepairResult, AnvilError> {
    calculate_repair_internal(input, AnvilState::Undamaged, is_creative, None)
}

/// Executes an anvil repair with a specified degradation roll `[0.0, 1.0)`.
pub fn repair_with_roll(
    input: &AnvilRepairInput,
    anvil_state: AnvilState,
    is_creative: bool,
    roll: f64,
) -> Result<AnvilRepairResult, AnvilError> {
    calculate_repair_internal(input, anvil_state, is_creative, Some(roll))
}

/// Executes an anvil repair with a caller-provided random source.
pub fn repair_with_rng(
    input: &AnvilRepairInput,
    anvil_state: AnvilState,
    is_creative: bool,
    rng: &mut crate::vanilla::random::RandomSource,
) -> Result<AnvilRepairResult, AnvilError> {
    let roll = rng.next_double();
    calculate_repair_internal(input, anvil_state, is_creative, Some(roll))
}

/// Executes an anvil repair on an `Undamaged` anvil, sampling degradation using a time-seeded random source.
pub fn repair(
    input: &AnvilRepairInput,
    is_creative: bool,
) -> Result<AnvilRepairResult, AnvilError> {
    repair_with_state(input, AnvilState::Undamaged, is_creative)
}

/// Executes an anvil repair with the given `anvil_state`, sampling degradation using a time-seeded random source.
pub fn repair_with_state(
    input: &AnvilRepairInput,
    anvil_state: AnvilState,
    is_creative: bool,
) -> Result<AnvilRepairResult, AnvilError> {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut rng = crate::vanilla::random::RandomSource::from_world_seed(seed);
    repair_with_rng(input, anvil_state, is_creative, &mut rng)
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

    /// Performs a repair, updating the anvil's state if degradation occurs.
    pub fn repair_with_roll(
        &mut self,
        input: &AnvilRepairInput,
        is_creative: bool,
        roll: f64,
    ) -> Result<AnvilRepairResult, AnvilError> {
        let result = repair_with_roll(input, self.state, is_creative, roll)?;
        if let Some(new_state) = result.degraded_state {
            self.state = new_state;
        }
        Ok(result)
    }

    /// Performs a repair with a random source, updating the anvil's state if degradation occurs.
    pub fn repair(
        &mut self,
        input: &AnvilRepairInput,
        is_creative: bool,
        rng: &mut crate::vanilla::random::RandomSource,
    ) -> Result<AnvilRepairResult, AnvilError> {
        let result = repair_with_rng(input, self.state, is_creative, rng)?;
        if let Some(new_state) = result.degraded_state {
            self.state = new_state;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vanilla::random::RandomSource;

    #[test]
    fn test_same_item_combination_durability_bonus() {
        // Iron sword has max durability 250.
        // 12% bonus = floor(250 * 0.12) = 30.
        let sword1 = ItemStack::new("minecraft:iron_sword", 1).with_durability(50);
        let sword2 = ItemStack::new("minecraft:iron_sword", 1).with_durability(50);

        let input = AnvilRepairInput::combine(sword1, sword2);
        let result = calculate_repair(&input, false).unwrap();

        // 50 + 50 + 30 = 130
        assert_eq!(result.result_item.durability, 130);
        assert_eq!(result.result_item.max_durability, 250);
        // Combining tools costs 2 levels
        assert_eq!(result.repair_cost_levels, 2);
        assert_eq!(result.material_cost, 1);
        // Prior work penalty: 0 -> next is 2*0 + 1 = 1
        assert_eq!(result.next_prior_work_penalty, 1);
        assert_eq!(result.result_item.repair_cost, 1);
        assert!(result.degraded_state.is_none());
    }

    #[test]
    fn test_same_item_combination_durability_capping() {
        // Capping at max_durability
        let pick1 = ItemStack::new("minecraft:diamond_pickaxe", 1).with_durability(1400);
        let pick2 = ItemStack::new("minecraft:diamond_pickaxe", 1).with_durability(300);
        // Max durability = 1561. Bonus = floor(1561 * 0.12) = 187.
        // 1400 + 300 + 187 = 1887 > 1561 -> capped at 1561.
        let input = AnvilRepairInput::combine(pick1, pick2);
        let result = calculate_repair(&input, false).unwrap();

        assert_eq!(result.result_item.durability, 1561);
        assert_eq!(result.repair_cost_levels, 2);
        assert_eq!(result.material_cost, 1);
    }

    #[test]
    fn test_material_repair_iron_chestplate() {
        // Iron chestplate max durability = 240.
        // 25% per ingot = floor(240 * 0.25) = 60.
        // Starting durability: 100 (missing 140).
        let chestplate = ItemStack::new("minecraft:iron_chestplate", 1).with_durability(100);
        let ingots = ItemStack::new("minecraft:iron_ingot", 5);

        let input = AnvilRepairInput::combine(chestplate, ingots);
        let result = calculate_repair(&input, false).unwrap();

        // 1 ingot: 100 + 60 = 160
        // 2 ingots: 160 + 60 = 220
        // 3 ingots: 220 + 60 = 280 -> capped at 240.
        // Total ingots consumed = 3.
        assert_eq!(result.result_item.durability, 240);
        assert_eq!(result.material_cost, 3);
        assert_eq!(result.repair_cost_levels, 3);
        assert_eq!(result.next_prior_work_penalty, 1);
    }

    #[test]
    fn test_material_repair_diamond_sword() {
        // Diamond sword max durability = 1561.
        // 25% per diamond = floor(1561 * 0.25) = 390.
        // Starting durability: 1 (missing 1560).
        let sword = ItemStack::new("minecraft:diamond_sword", 1).with_durability(1);
        let diamonds = ItemStack::new("minecraft:diamond", 10);

        let input = AnvilRepairInput::combine(sword, diamonds);
        let result = calculate_repair(&input, false).unwrap();

        // 4 diamonds repair full durability: 4 * 390 = 1560, total = 1561.
        assert_eq!(result.result_item.durability, 1561);
        assert_eq!(result.material_cost, 4);
        assert_eq!(result.repair_cost_levels, 4);
    }

    #[test]
    fn test_material_repair_insufficient_materials() {
        // Iron chestplate missing 140 durability, but player only has 2 ingots.
        let chestplate = ItemStack::new("minecraft:iron_chestplate", 1).with_durability(100);
        let ingots = ItemStack::new("minecraft:iron_ingot", 2);

        let input = AnvilRepairInput::combine(chestplate, ingots);
        let result = calculate_repair(&input, false).unwrap();

        // Consumes 2 ingots, restores 120 durability -> 220
        assert_eq!(result.result_item.durability, 220);
        assert_eq!(result.material_cost, 2);
        assert_eq!(result.repair_cost_levels, 2);
    }

    #[test]
    fn test_rename_handling_and_validation() {
        let sword = ItemStack::new("minecraft:diamond_sword", 1);

        // Rename costs 1 level
        let input = AnvilRepairInput::rename_only(sword.clone(), "Excalibur").unwrap();
        let result = calculate_repair(&input, false).unwrap();
        assert_eq!(result.result_item.custom_name.as_deref(), Some("Excalibur"));
        assert_eq!(result.repair_cost_levels, 1);
        assert_eq!(result.material_cost, 0);
        assert_eq!(result.next_prior_work_penalty, 1);

        // Trimmed whitespace
        let input_trimmed =
            AnvilRepairInput::rename_only(sword.clone(), "   Doom Blade   ").unwrap();
        let res_trimmed = calculate_repair(&input_trimmed, false).unwrap();
        assert_eq!(
            res_trimmed.result_item.custom_name.as_deref(),
            Some("Doom Blade")
        );

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
        let named_sword = sword.with_custom_name("Old Name");
        let reset_input = AnvilRepairInput::new(named_sword, None, Some("   ".into())).unwrap();
        let reset_res = calculate_repair(&reset_input, false).unwrap();
        assert_eq!(reset_res.result_item.custom_name, None);
        assert_eq!(reset_res.repair_cost_levels, 1); // un-renaming costs 1 level

        // Unchanged name does not cost levels
        let sword_same = ItemStack::new("minecraft:iron_sword", 1).with_custom_name("Sword");
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
            let sword1 = ItemStack::new("minecraft:iron_sword", 1)
                .with_durability(100)
                .with_repair_cost(penalty);
            let sword2 = ItemStack::new("minecraft:iron_sword", 1)
                .with_durability(100)
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
        let sword1 = ItemStack::new("minecraft:iron_sword", 1)
            .with_durability(100)
            .with_repair_cost(15);
        let sword2 = ItemStack::new("minecraft:iron_sword", 1)
            .with_durability(100)
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
        let sword1 = ItemStack::new("minecraft:iron_sword", 1).with_durability(50);
        let sword2 = ItemStack::new("minecraft:iron_sword", 1).with_durability(50);
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
    fn test_anvil_struct_state_mutation() {
        let mut anvil = Anvil::undamaged();
        let sword1 = ItemStack::new("minecraft:iron_sword", 1).with_durability(50);
        let sword2 = ItemStack::new("minecraft:iron_sword", 1).with_durability(50);
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
        let sword = ItemStack::new("minecraft:diamond_sword", 1).with_durability(100);
        let ingot = ItemStack::new("minecraft:iron_ingot", 1);
        let input = AnvilRepairInput::combine(sword, ingot);
        assert_eq!(
            calculate_repair(&input, false),
            Err(AnvilError::IncompatibleSacrificeItem)
        );

        // Diamond sword + Iron sword is incompatible
        let iron_sword = ItemStack::new("minecraft:iron_sword", 1);
        let sword = ItemStack::new("minecraft:diamond_sword", 1).with_durability(100);
        let input2 = AnvilRepairInput::combine(sword, iron_sword);
        assert_eq!(
            calculate_repair(&input2, false),
            Err(AnvilError::IncompatibleSacrificeItem)
        );

        // Empty base item
        let empty_item = ItemStack::new("", 0);
        let input_empty = AnvilRepairInput::new(empty_item, None, None).unwrap();
        assert_eq!(
            calculate_repair(&input_empty, false),
            Err(AnvilError::EmptyBaseItem)
        );

        // Fully repaired item with no rename
        let full_sword1 = ItemStack::new("minecraft:iron_sword", 1);
        let full_sword2 = ItemStack::new("minecraft:iron_sword", 1);
        let input_full = AnvilRepairInput::combine(full_sword1, full_sword2);
        assert_eq!(
            calculate_repair(&input_full, false),
            Err(AnvilError::NoRepairNeeded)
        );
    }
}
