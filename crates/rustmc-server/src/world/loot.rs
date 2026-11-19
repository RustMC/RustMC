//! Typed block, entity, chest, gameplay, and archaeology loot table model and
//! runtime loader for Java 26.3 datapacks.
//!
//! Reads `data/<namespace>/loot_table/<category>/**/*.json` (for `blocks`,
//! `entities`, `chests`, `gameplay`, and `archaeology`) from an
//! operator-provisioned vanilla data root and resolves named predicate strings
//! against `data/<namespace>/predicate/**/*.json` as well as `loot_table`
//! entry references against `data/<namespace>/loot_table/**/*.json`.
//!
//! Verified 26.3 block loot format facts (checked across all 1,201 vanilla
//! block loot tables and 3 predicate files):
//! - Uses the singular keys `condition` and `modifier` (each either a single
//!   value or a JSON array).
//! - Table `pools` is optional (14 block tables omit `pools`, yielding an empty
//!   pool list), and tables (9) and pools (7) may also carry top-level `modifier`.
//! - Modifiers (`set_count`, `apply_bonus`) may carry an optional `condition`.
//! - `match_block` uses the plural key `blocks` (a single block id string or
//!   a list of block id strings) plus optional `state` string properties.
//! - `entity_properties` occurs twice (`chorus_flower`, `snow`), and
//!   `location_check` occurs 4 times (`large_fern`, `tall_grass`, checking
//!   `offsetY` ±1 with `predicate.block` `blocks` + `state`).
//!
//! Verified 26.3 entity loot format facts (checked across all 109 JSON files
//! under `data/minecraft/loot_table/entities/**`: 93 top-level files plus 16
//! `sheep/<color>.json` sub-tables):
//! - Uses the same singular `condition` and `modifier` keys, optional `pools`
//!   (30 entity tables omit `pools`), and pool-level `modifier` (`ghast`).
//! - Entry types used: `item` (137), `loot_table` (18: `sheep` referencing
//!   `minecraft:entities/sheep/*` and `guardian`/`elder_guardian` referencing
//!   `minecraft:gameplay/fishing/fish` with type `minecraft:fishing`),
//!   `empty` (3), `tag` (1 in `creeper`, using `"items": "#minecraft:creeper_drop_music_discs"`
//!   and `"expand": true`), and `alternatives` (1). Singleton entries may carry `weight`.
//! - Modifiers used: `set_count` (89), `enchanted_count_increase` (80, with
//!   optional `limit`), `furnace_smelt` (19), `set_potion` (3), and
//!   `set_ominous_bottle_amplifier` (1 in `pillager`, left `Unsupported`).
//! - Conditions used: `entity_properties` (61, testing `this`, `attacker`,
//!   `direct_attacker`), `killed_by_player` (24), `any_of` (19), `all_of` (13),
//!   `random_chance_with_enchanted_bonus` (11, using `minecraft:linear`),
//!   `damage_source_properties` (8), `random_chance` (4), and `inverted` (2).
//!
//! Verified 26.3 chest, gameplay, and archaeology loot format facts (checked across
//! all 90 JSON files under `chests/` 58, `gameplay/` 26 including `fishing/` and
//! `hero_of_the_village/` subfolders, and `archaeology/` 6):
//! - `chests/` uses `type: "minecraft:chest"`, `archaeology/` uses
//!   `type: "minecraft:archaeology"`, and `gameplay/` spans `minecraft:gift` (21),
//!   `minecraft:fishing` (4), and `minecraft:barter` (1).
//! - `gameplay/fishing` uses `quality` on `loot_table` entries (`-2`, `2`, `-1`)
//!   and `entity_properties` with `minecraft:type_specific/fishing_hook`
//!   (`in_open_water`). `gameplay/chicken_lay` uses `minecraft:components`
//!   (`minecraft:chicken/variant`). Biome-based `location_check` stays `Unsupported`.
//! - Modifiers used: `set_count` (517), `enchant_randomly` (52),
//!   `enchant_with_levels` (34), `set_potion` (29), `set_damage` (28),
//!   `exploration_map` (18, left `Unsupported`), `filtered` (18, using
//!   `minecraft:map_id` in vanilla so those stay `Unsupported`), `set_name` (8,
//!   using `translate` + `target: "item_name"` in vanilla so those stay
//!   `Unsupported`), `set_stew_effect` (3, left `Unsupported`),
//!   `set_ominous_bottle_amplifier` (2, left `Unsupported`),
//!   `set_instrument` (1, left `Unsupported`), and `set_enchantments` (1).
//!
//! Schema definitions follow `minecraft.wiki`:
//! - `Loot table` (revision 3893957): root fields `type`, `modifier`, `pools`,
//!   `random_sequence`; pool fields `condition`, `modifier`, `rolls`, `entries`;
//!   entry types `item`, `loot_table`, `empty`, `tag`, `alternatives`, `dynamic`,
//!   `weight`, and `quality`.
//! - `Predicate` (revision 3888899): `all_of`, `any_of`, `inverted`,
//!   `location_check` (with `Advancement/Conditions/location` revision 3684921 &
//!   `Advancement/Conditions/block` revision 3684708), `match_block`,
//!   `match_tool`, `random_chance`, `random_chance_with_enchanted_bonus`,
//!   `killed_by_player`, `survives_explosion`, `table_bonus`,
//!   `entity_properties` (`Advancement/Conditions/entity` revision 3825655,
//!   including `minecraft:type_specific/fishing_hook.in_open_water`),
//!   and `damage_source_properties` (`Advancement/Conditions/damage type` revision 3778836).
//! - `Item modifier` (revision 3896146): `apply_bonus`, `copy_components`,
//!   `copy_state`, `discard`, `enchant_randomly`, `enchant_with_levels`,
//!   `enchanted_count_increase`, `explosion_decay`, `filtered` (with
//!   `Advancement/Conditions/item` revision 3286549 & `Data component predicate`
//!   revision 3826514), `furnace_smelt`, `limit_count`, `set_count`,
//!   `set_damage`, `set_enchantments`, `set_name`, `set_potion`.
//! - `Number provider` (revision 3869456): constant numbers, `uniform`, and
//!   `binomial`.
//! - `Enchantment definition` (revision 3843772): `Level-based value` (`linear`, etc.).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::item::{
    ItemComponents, ItemDefaults, ItemDefaultsError, ItemStack, MAX_STACK_LIMIT,
    MIN_ENCHANTMENT_LEVEL, default_max_stack, items_equal, normalize_item_id,
};
use crate::vanilla::feature::{BlockTags, EnchantmentTags, ItemTags, load_tag_entries};
use crate::vanilla::random::RandomSource;
use crate::vanilla::worldgen::{WorldgenError, read_dir_optional, read_json, walk_json};
use crate::world::enchanting::select_enchantments_for_loot;
use crate::world::enchantment::{
    EnchantmentDefinition, EnchantmentError, EnchantmentRegistry, MAX_ENCHANTMENT_LEVEL,
};
use crate::world::recipe_data::{CookingKind, RecipeData, RecipeDataError};

/// Maximum nesting depth allowed when resolving or evaluating `loot_table` entries.
pub const MAX_LOOT_TABLE_DEPTH: usize = 32;

/// Errors encountered while loading or parsing loot tables.
#[derive(Debug)]
pub enum LootError {
    /// Reading or parsing a JSON file failed.
    Worldgen(WorldgenError),
    /// Reading or parsing default item component reports failed.
    ItemDefaults(ItemDefaultsError),
    /// Loading smelting recipes failed.
    RecipeData(RecipeDataError),
    /// Loading enchantment definitions failed.
    Enchantment(EnchantmentError),
    /// A loot table referenced a named predicate file that does not exist.
    MissingPredicate {
        table_file: PathBuf,
        predicate: String,
        predicate_file: PathBuf,
    },
    /// A loot table referenced another loot table id that does not exist.
    MissingLootTable {
        table_file: PathBuf,
        loot_table: String,
        loot_table_file: PathBuf,
    },
    /// A loot table or predicate file had invalid syntax, types, or unknown shape.
    Invalid { file: PathBuf, message: String },
}

impl fmt::Display for LootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Worldgen(error) => write!(f, "{error}"),
            Self::ItemDefaults(error) => write!(f, "{error}"),
            Self::RecipeData(error) => write!(f, "{error}"),
            Self::Enchantment(error) => write!(f, "{error}"),
            Self::MissingPredicate {
                table_file,
                predicate,
                predicate_file,
            } => write!(
                f,
                "loot table {} references missing predicate `{predicate}` (expected {})",
                table_file.display(),
                predicate_file.display()
            ),
            Self::MissingLootTable {
                table_file,
                loot_table,
                loot_table_file,
            } => write!(
                f,
                "loot table {} references missing loot table `{loot_table}` (expected {})",
                table_file.display(),
                loot_table_file.display()
            ),
            Self::Invalid { file, message } => {
                write!(f, "invalid loot table {}: {message}", file.display())
            }
        }
    }
}

impl std::error::Error for LootError {}

impl From<WorldgenError> for LootError {
    fn from(error: WorldgenError) -> Self {
        Self::Worldgen(error)
    }
}

impl From<ItemDefaultsError> for LootError {
    fn from(error: ItemDefaultsError) -> Self {
        Self::ItemDefaults(error)
    }
}

impl From<RecipeDataError> for LootError {
    fn from(error: RecipeDataError) -> Self {
        Self::RecipeData(error)
    }
}

impl From<EnchantmentError> for LootError {
    fn from(error: EnchantmentError) -> Self {
        Self::Enchantment(error)
    }
}

/// Number provider used for pool `rolls` and `set_count` quantities.
///
/// Mirrors `minecraft.wiki` `Number provider` (revision 3869456): plain
/// constants, `uniform` (`min`/`max`), and `binomial` (`n`/`p`).
#[derive(Debug, Clone, PartialEq)]
pub enum NumberProvider {
    /// Fixed constant number (bare JSON number or `{"type":"minecraft:constant","value":...}`).
    Constant(f64),
    /// Uniform range between `min` and `max` (`minecraft:uniform`).
    Uniform { min: f64, max: f64 },
    /// Binomial distribution of `n` trials with success probability `p` (`minecraft:binomial`).
    Binomial { n: u32, p: f64 },
}

/// Enchantment level constraint inside a `match_tool` predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnchantmentPredicate {
    /// Accepted enchantment identifiers (empty matches any enchantment).
    pub enchantments: Vec<String>,
    /// Inclusive minimum enchantment level, if specified.
    pub min_level: Option<u32>,
    /// Inclusive maximum enchantment level, if specified.
    pub max_level: Option<u32>,
}

/// Item matcher inside a `minecraft:match_tool` condition.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolPredicate {
    /// Accepted item ids or `#`-prefixed item tags (empty imposes no item restriction).
    pub items: Vec<String>,
    /// Required enchantment component checks (`predicates.minecraft:enchantments`).
    pub enchantments: Vec<EnchantmentPredicate>,
}

/// Enchantment level-based float calculation (`minecraft.wiki` `Enchantment definition`,
/// revision `3843772`, `Level-based value`).
#[derive(Debug, Clone, PartialEq)]
pub enum LevelBasedValue {
    /// Fixed constant independent of enchantment level.
    Constant(f64),
    /// `minecraft:linear` — `base + per_level_above_first * (level - 1)`.
    Linear {
        base: f64,
        per_level_above_first: f64,
    },
    /// `minecraft:levels_squared` — `level^2 + added`.
    LevelsSquared { added: f64 },
    /// `minecraft:clamped` — clamps `value` to `[min, max]`.
    Clamped {
        value: Box<LevelBasedValue>,
        min: f64,
        max: f64,
    },
    /// `minecraft:fraction` — `numerator / denominator`.
    Fraction {
        numerator: Box<LevelBasedValue>,
        denominator: Box<LevelBasedValue>,
    },
    /// `minecraft:exponent` — `base ^ power`.
    Exponent {
        base: Box<LevelBasedValue>,
        power: Box<LevelBasedValue>,
    },
    /// `minecraft:lookup` — `values[level - 1]` or `fallback` when out of bounds.
    Lookup {
        values: Vec<f64>,
        fallback: Box<LevelBasedValue>,
    },
}

impl LevelBasedValue {
    /// Evaluates this level-based value for enchantment `level`.
    ///
    /// Follows `minecraft.wiki` `Enchantment definition` (revision `3843772`,
    /// `Level-based value`):
    /// - `Constant(v)` -> `v`
    /// - `Linear` -> `"The final value is base + per_level_above_first * (level - 1)."`
    /// - `LevelsSquared` -> `"The final value is level ^ 2 + added"`
    /// - `Clamped` -> clamps the inner value to `[min, max]`
    /// - `Fraction` -> `numerator / denominator` (`0.0` when denominator is `0.0`)
    /// - `Exponent` -> `base ^ power`
    /// - `Lookup` -> `values[level - 1]` when in range, else `fallback`
    #[must_use]
    pub fn evaluate(&self, level: u8) -> f64 {
        let lvl = f64::from(level);
        match self {
            Self::Constant(value) => *value,
            Self::Linear {
                base,
                per_level_above_first,
            } => *base + *per_level_above_first * (lvl - 1.0),
            Self::LevelsSquared { added } => lvl * lvl + *added,
            Self::Clamped { value, min, max } => {
                let inner = value.evaluate(level);
                if *min <= *max {
                    inner.clamp(*min, *max)
                } else {
                    *min
                }
            }
            Self::Fraction {
                numerator,
                denominator,
            } => {
                let num = numerator.evaluate(level);
                let den = denominator.evaluate(level);
                if den == 0.0 { 0.0 } else { num / den }
            }
            Self::Exponent { base, power } => base.evaluate(level).powf(power.evaluate(level)),
            Self::Lookup { values, fallback } => {
                if level >= 1 {
                    let idx = usize::from(level - 1);
                    if let Some(&val) = values.get(idx) {
                        return val;
                    }
                }
                fallback.evaluate(level)
            }
        }
    }
}

/// Inclusive integer range matcher (exact value or optional `min`/`max` bounds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IntRange {
    /// Inclusive minimum bound, if specified.
    pub min: Option<i32>,
    /// Inclusive maximum bound, if specified.
    pub max: Option<i32>,
}

impl IntRange {
    /// Returns `true` when `value` lies within the optional inclusive `[min, max]` bounds.
    #[must_use]
    pub fn matches(self, value: i32) -> bool {
        self.min.is_none_or(|lo| value >= lo) && self.max.is_none_or(|hi| value <= hi)
    }
}

/// Entity boolean flags checked by `minecraft:flags` inside `entity_properties`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntityFlagsPredicate {
    /// Matches whether the entity is on fire (`is_on_fire`).
    pub is_on_fire: Option<bool>,
    /// Matches whether the entity is a baby (`is_baby`).
    pub is_baby: Option<bool>,
}

/// Entity equipment matcher checked by `minecraft:equipment` inside `entity_properties`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EntityEquipmentPredicate {
    /// Required predicate on the entity's `mainhand` item.
    pub mainhand: Option<ToolPredicate>,
}

/// Fishing hook properties checked by `minecraft:type_specific/fishing_hook`.
///
/// Mirrors `minecraft.wiki` `Advancement/Conditions/entity` (revision `3825655`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FishingHookPredicate {
    /// Matches whether the fishing bobber was in open water (`in_open_water`).
    pub in_open_water: Option<bool>,
}

/// Typed entity predicate for the fields tested by vanilla entity and gameplay loot tables.
///
/// Any unmodeled `entity_properties` predicate field causes the parser to emit
/// [`LootCondition::Unsupported`] so coverage gaps remain visible.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EntityPredicate {
    /// `minecraft:flags` (`is_on_fire`, `is_baby`).
    pub flags: Option<EntityFlagsPredicate>,
    /// `minecraft:entity_type` (single id, id list, or `#`-prefixed tag).
    pub entity_types: Vec<String>,
    /// `minecraft:vehicle` (recursive predicate on mounted vehicle).
    pub vehicle: Option<Box<EntityPredicate>>,
    /// `minecraft:equipment` (`mainhand`).
    pub equipment: Option<EntityEquipmentPredicate>,
    /// `minecraft:components` exact string component checks (e.g. `minecraft:sheep/color`).
    pub components: BTreeMap<String, String>,
    /// `minecraft:type_specific/sheep.sheared`.
    pub sheep_sheared: Option<bool>,
    /// `minecraft:type_specific/cube_mob.size`.
    pub cube_mob_size: Option<IntRange>,
    /// `minecraft:type_specific/raider.is_captain`.
    pub raider_is_captain: Option<bool>,
    /// `minecraft:type_specific/fishing_hook` (`in_open_water`).
    pub fishing_hook: Option<FishingHookPredicate>,
}

/// Damage type tag membership requirement inside `minecraft:damage_source_properties`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DamageTypeTagPredicate {
    /// Damage type id or `#`-prefixed damage type tag id (for example `"#minecraft:is_projectile"`).
    pub id: String,
    /// Whether the damage type is expected to match `id`.
    pub expected: bool,
}

/// Typed damage-source predicate for the fields tested by vanilla entity loot tables.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DamageSourcePredicate {
    /// `direct_entity` predicate.
    pub direct_entity: Option<EntityPredicate>,
    /// `source_entity` predicate.
    pub source_entity: Option<EntityPredicate>,
    /// `tags` membership checks (`id`, `expected`).
    pub tags: Vec<DamageTypeTagPredicate>,
}

/// Predicate condition attached to pools, entries, or modifiers.
///
/// Mirrors `minecraft.wiki` `Predicate` (revision 3888899). Unsupported
/// condition types or unsupported `location_check` / `entity_properties` /
/// `damage_source_properties` predicate fields are retained as
/// [`LootCondition::Unsupported`] so callers can inspect coverage gaps.
#[derive(Debug, Clone, PartialEq)]
pub enum LootCondition {
    /// `minecraft:survives_explosion` — survival test under an explosion radius.
    SurvivesExplosion,
    /// `minecraft:match_block` — matches block id(s) (`blocks`) and optional `state` properties.
    MatchBlock {
        blocks: Vec<String>,
        state: BTreeMap<String, String>,
    },
    /// `minecraft:location_check` — matches relative block at
    /// `(offsetX, offsetY, offsetZ)` against `predicate.block` (`blocks` and `state`).
    LocationCheck {
        offset_x: i32,
        offset_y: i32,
        offset_z: i32,
        blocks: Vec<String>,
        state: BTreeMap<String, String>,
    },
    /// `minecraft:table_bonus` — probability table indexed by `enchantment` level.
    TableBonus {
        enchantment: String,
        chances: Vec<f64>,
    },
    /// `minecraft:all_of` — logical conjunction of nested conditions.
    AllOf { terms: Vec<LootCondition> },
    /// `minecraft:any_of` — logical disjunction of nested conditions.
    AnyOf { terms: Vec<LootCondition> },
    /// `minecraft:inverted` — logical negation of `term`.
    Inverted { term: Box<LootCondition> },
    /// `minecraft:random_chance` — fixed Bernoulli pass probability in `0.0..=1.0`.
    RandomChance { chance: f64 },
    /// `minecraft:random_chance_with_enchanted_bonus` — Bernoulli pass probability
    /// scaled by an enchantment on `attacker`.
    RandomChanceWithEnchantedBonus {
        enchantment: String,
        unenchanted_chance: f64,
        enchanted_chance: LevelBasedValue,
    },
    /// `minecraft:killed_by_player` — passes when `attacking_player` is present.
    KilledByPlayer,
    /// `minecraft:match_tool` — checks breaking tool item/tag and enchantments.
    MatchTool { predicate: ToolPredicate },
    /// `minecraft:entity_properties` — checks entity context properties (`entity`, `predicate`).
    EntityProperties {
        entity: String,
        predicate: Box<EntityPredicate>,
    },
    /// `minecraft:damage_source_properties` — checks damage source properties (`predicate`).
    DamageSourceProperties {
        predicate: Box<DamageSourcePredicate>,
    },
    /// Condition type without a typed runtime model.
    Unsupported { what: String },
}

/// Bonus formula variants for [`LootModifier::ApplyBonus`].
///
/// Mirrors `minecraft.wiki` `Item modifier` (revision 3896146).
#[derive(Debug, Clone, PartialEq)]
pub enum BonusFormula {
    /// `minecraft:ore_drops`
    OreDrops,
    /// `minecraft:uniform_bonus_count` (`parameters.bonusMultiplier`)
    UniformBonusCount { bonus_multiplier: f64 },
    /// `minecraft:binomial_with_bonus_count` (`parameters.extra`, `parameters.probability`)
    BinomialWithBonusCount { extra: u32, probability: f64 },
}

/// Target component for [`LootModifier::SetName`].
///
/// Mirrors `minecraft.wiki` `Item modifier` (revision `3896146`):
/// `"Allowed values are custom_name or item_name, corresponding with the component to be set. Defaults to custom_name."`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetNameTarget {
    /// Sets `minecraft:custom_name` on [`ItemComponents`].
    CustomName,
    /// Targets `minecraft:item_name`, which [`ItemComponents`] does not model.
    ItemName,
}

/// Damage and remaining-durability bounds inside `predicates.minecraft:damage`.
///
/// Mirrors `minecraft.wiki` `Data component predicate` (revision `3826514`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DamagePredicate {
    /// Consumed durability range (`minecraft:damage`).
    pub damage: Option<IntRange>,
    /// Remaining durability range (`max_damage - damage`).
    pub durability: Option<IntRange>,
}

/// Item matcher inside `minecraft:filtered`'s `item_filter`.
///
/// Mirrors `minecraft.wiki` `Advancement/Conditions/item` (revision `3286549`)
/// and `Data component predicate` (revision `3826514`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemFilterPredicate {
    /// Accepted item ids or `#`-prefixed item tags (empty imposes no item restriction).
    pub items: Vec<String>,
    /// Optional stack size bounds (`count`).
    pub count: Option<IntRange>,
    /// Required active enchantment checks (`predicates.minecraft:enchantments`).
    pub enchantments: Vec<EnchantmentPredicate>,
    /// Required stored enchantment checks (`predicates.minecraft:stored_enchantments`).
    pub stored_enchantments: Vec<EnchantmentPredicate>,
    /// Required damage/durability check (`predicates.minecraft:damage`).
    pub damage: Option<DamagePredicate>,
}

/// Item modifier attached to a table, pool, or entry.
///
/// Mirrors `minecraft.wiki` `Item modifier` (revision 3896146).
#[derive(Debug, Clone, PartialEq)]
pub enum LootModifier {
    /// `minecraft:set_count` — sets or adds to stack size using `count`.
    SetCount {
        count: NumberProvider,
        add: bool,
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:set_damage` — sets or adjusts durability fraction (`1.0` undamaged, `0.0` zero durability left).
    SetDamage {
        damage: NumberProvider,
        add: bool,
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:set_name` — sets plain-text item name component (`target` defaults to `custom_name`).
    SetName {
        name: String,
        target: SetNameTarget,
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:set_enchantments` — sets or adds enchantment levels (`book` converts to `enchanted_book`).
    SetEnchantments {
        enchantments: BTreeMap<String, NumberProvider>,
        add: bool,
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:enchant_randomly` — applies one randomly chosen enchantment (`book` converts to `enchanted_book`).
    EnchantRandomly {
        options: Option<Vec<String>>,
        only_compatible: bool,
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:enchant_with_levels` — enchants the item at the given experience `levels` (`book` converts to `enchanted_book`).
    EnchantWithLevels {
        levels: NumberProvider,
        options: Option<Vec<String>>,
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:filtered` — tests `item_filter` and applies `on_pass` or `on_fail` modifiers.
    Filtered {
        item_filter: ItemFilterPredicate,
        on_pass: Vec<LootModifier>,
        on_fail: Vec<LootModifier>,
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:discard` — removes the item stack.
    Discard { conditions: Vec<LootCondition> },
    /// `minecraft:enchanted_count_increase` — adds `round(count * level)` items
    /// based on `enchantment` on `attacker`, capped at `limit` when `limit > 0`.
    EnchantedCountIncrease {
        enchantment: String,
        count: NumberProvider,
        limit: u32,
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:furnace_smelt` — smelts the dropped item via furnace smelting recipes.
    FurnaceSmelt { conditions: Vec<LootCondition> },
    /// `minecraft:set_potion` — sets the `minecraft:potion_contents` base potion id.
    SetPotion {
        id: String,
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:explosion_decay` — reduces stack count during explosions.
    ExplosionDecay { conditions: Vec<LootCondition> },
    /// `minecraft:apply_bonus` — applies an enchantment bonus formula.
    ApplyBonus {
        enchantment: String,
        formula: BonusFormula,
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:limit_count` — clamps stack count to optional `min`/`max` bounds.
    LimitCount {
        min: Option<i32>,
        max: Option<i32>,
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:copy_components` — copies data components from `source`.
    CopyComponents {
        source: String,
        include: Vec<String>,
        exclude: Vec<String>,
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:copy_state` — copies `properties` of `block` onto the item state component.
    CopyState {
        block: String,
        properties: Vec<String>,
        conditions: Vec<LootCondition>,
    },
    /// Item modifier type without a typed runtime model.
    Unsupported { what: String },
}

impl LootModifier {
    fn conditions(&self) -> &[LootCondition] {
        match self {
            Self::SetCount { conditions, .. }
            | Self::SetDamage { conditions, .. }
            | Self::SetName { conditions, .. }
            | Self::SetEnchantments { conditions, .. }
            | Self::EnchantRandomly { conditions, .. }
            | Self::EnchantWithLevels { conditions, .. }
            | Self::Filtered { conditions, .. }
            | Self::Discard { conditions }
            | Self::EnchantedCountIncrease { conditions, .. }
            | Self::FurnaceSmelt { conditions }
            | Self::SetPotion { conditions, .. }
            | Self::ExplosionDecay { conditions }
            | Self::ApplyBonus { conditions, .. }
            | Self::LimitCount { conditions, .. }
            | Self::CopyComponents { conditions, .. }
            | Self::CopyState { conditions, .. } => conditions,
            Self::Unsupported { .. } => &[],
        }
    }
}

/// Target of a [`LootEntry::LootTable`] entry.
#[derive(Debug, Clone, PartialEq)]
pub enum LootTableSource {
    /// Reference to another loot table by namespaced id (resolved at load time).
    Reference {
        id: String,
        table: Option<Arc<LootTable>>,
    },
    /// Inline loot table embedded directly inside the entry.
    Inline(Box<LootTable>),
}

/// Entry inside a [`LootPool`].
///
/// Mirrors `minecraft.wiki` `Loot table` (revision 3893957): `"alternatives —
/// Extracts only the first successful (conditions are met) entry."`, `"item —
/// Drops a single item stack."`, `"loot_table — Drops the loot from another
/// loot table."`, `"empty — Drops nothing."`, `"tag"`, and `"dynamic — Drops
/// block-specific loot."`
#[derive(Debug, Clone, PartialEq)]
pub enum LootEntry {
    /// `minecraft:item` — drops a single item stack.
    Item {
        name: String,
        weight: u32,
        quality: i32,
        conditions: Vec<LootCondition>,
        modifiers: Vec<LootModifier>,
    },
    /// `minecraft:loot_table` — rolls another loot table (referenced by id or inline).
    LootTable {
        source: LootTableSource,
        weight: u32,
        quality: i32,
        conditions: Vec<LootCondition>,
        modifiers: Vec<LootModifier>,
    },
    /// `minecraft:empty` — drops nothing.
    Empty {
        weight: u32,
        quality: i32,
        conditions: Vec<LootCondition>,
        modifiers: Vec<LootModifier>,
    },
    /// `minecraft:tag` — expands into one entry per tag item (`expand == true`)
    /// or drops all tag items together (`expand == false`).
    Tag {
        tag: String,
        expand: bool,
        weight: u32,
        quality: i32,
        conditions: Vec<LootCondition>,
        modifiers: Vec<LootModifier>,
    },
    /// `minecraft:alternatives` — picks the first child entry whose conditions pass.
    Alternatives {
        children: Vec<LootEntry>,
        conditions: Vec<LootCondition>,
        modifiers: Vec<LootModifier>,
    },
    /// `minecraft:dynamic` — block-specific dynamic drop (such as `minecraft:sherds`).
    Dynamic {
        name: String,
        weight: u32,
        quality: i32,
        conditions: Vec<LootCondition>,
        modifiers: Vec<LootModifier>,
    },
}

/// One independent roll group within a [`LootTable`].
///
/// Mirrors `minecraft.wiki` `Loot table` (revision 3893957): `"Each roll of a
/// pool is independent."`
#[derive(Debug, Clone, PartialEq)]
pub struct LootPool {
    /// Number of rolls executed when the pool's conditions pass.
    pub rolls: NumberProvider,
    /// Candidate entries in this pool.
    pub entries: Vec<LootEntry>,
    /// Optional pool-level conditions (all must pass for the pool to roll).
    pub conditions: Vec<LootCondition>,
    /// Optional pool-level modifiers applied to items dropped by this pool.
    pub modifiers: Vec<LootModifier>,
}

impl LootEntry {
    fn conditions(&self) -> &[LootCondition] {
        match self {
            Self::Item { conditions, .. }
            | Self::LootTable { conditions, .. }
            | Self::Empty { conditions, .. }
            | Self::Tag { conditions, .. }
            | Self::Alternatives { conditions, .. }
            | Self::Dynamic { conditions, .. } => conditions,
        }
    }
}

/// Tool context supplied to [`LootTable::roll`] when breaking a block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolContext {
    /// Item identifier of the breaking tool (for example `"minecraft:diamond_pickaxe"`).
    pub item: String,
    /// Active enchantment levels on the tool keyed by enchantment id.
    pub enchantments: BTreeMap<String, u8>,
}

/// Relative block lookup supplied to [`LootContext`] for `minecraft:location_check`.
///
/// Receives offsets `(dx, dy, dz)` relative to the broken block's origin and
/// returns `Some((block_id, properties))` for loaded locations, or `None` when
/// unloaded or unavailable.
pub trait BlockLookup: Send + Sync {
    /// Returns `(block_id, state_properties)` at relative offset `(dx, dy, dz)`,
    /// or `None` if unavailable/unloaded.
    fn block_at(&self, dx: i32, dy: i32, dz: i32) -> Option<(String, BTreeMap<String, String>)>;
}

impl<F> BlockLookup for F
where
    F: Fn(i32, i32, i32) -> Option<(String, BTreeMap<String, String>)> + Send + Sync,
{
    fn block_at(&self, dx: i32, dy: i32, dz: i32) -> Option<(String, BTreeMap<String, String>)> {
        self(dx, dy, dz)
    }
}

/// Description of an entity supplied to [`LootContext`] for evaluating
/// `minecraft:entity_properties` and `damage_source_properties` entity sub-predicates.
///
/// Captures only the fields tested across `entity_properties` uses (and
/// `damage_source_properties` sub-predicates) in the vanilla 26.3 entity and
/// gameplay tables: `entity_type`, `is_on_fire`, `is_baby`, `mainhand`
/// equipment, `vehicle`, `components` (`minecraft:sheep/color`,
/// `minecraft:frog/variant`, `minecraft:chicken/variant`),
/// `type_specific/sheep.sheared`, `type_specific/cube_mob.size`,
/// `type_specific/raider.is_captain`, and
/// `type_specific/fishing_hook.in_open_water`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntityContext {
    /// Namespaced entity type id (for example `"minecraft:cow"` or `"minecraft:frog"`).
    pub entity_type: String,
    /// Explicit entity-type tag ids present on this entity (with or without `#`),
    /// checked alongside any datapack `data/<ns>/tags/entity_type` definitions.
    pub entity_type_tags: Vec<String>,
    /// `minecraft:flags.is_on_fire`.
    pub is_on_fire: bool,
    /// `minecraft:flags.is_baby`.
    pub is_baby: bool,
    /// `minecraft:equipment.mainhand` item/enchantments.
    pub mainhand: Option<ToolContext>,
    /// Mounted vehicle entity tested by `minecraft:vehicle`.
    pub vehicle: Option<Box<EntityContext>>,
    /// String-valued components (for example `"minecraft:sheep/color"`,
    /// `"minecraft:frog/variant"`, or `"minecraft:chicken/variant"`).
    pub components: BTreeMap<String, String>,
    /// `minecraft:type_specific/sheep.sheared`, if applicable.
    pub sheep_sheared: Option<bool>,
    /// `minecraft:type_specific/cube_mob.size`, if applicable.
    pub cube_mob_size: Option<i32>,
    /// `minecraft:type_specific/raider.is_captain`, if applicable.
    pub raider_is_captain: Option<bool>,
    /// `minecraft:type_specific/fishing_hook.in_open_water`, if this entity is a fishing bobber.
    pub fishing_hook_in_open_water: Option<bool>,
}

impl EntityContext {
    /// Constructs an [`EntityContext`] with `entity_type` and default flags.
    #[must_use]
    pub fn new(entity_type: impl Into<String>) -> Self {
        Self {
            entity_type: entity_type.into(),
            ..Self::default()
        }
    }

    /// Sets `minecraft:flags.is_on_fire`.
    #[must_use]
    pub fn with_on_fire(mut self, is_on_fire: bool) -> Self {
        self.is_on_fire = is_on_fire;
        self
    }

    /// Sets `minecraft:flags.is_baby`.
    #[must_use]
    pub fn with_baby(mut self, is_baby: bool) -> Self {
        self.is_baby = is_baby;
        self
    }

    /// Sets `minecraft:equipment.mainhand`.
    #[must_use]
    pub fn with_mainhand(mut self, mainhand: ToolContext) -> Self {
        self.mainhand = Some(mainhand);
        self
    }

    /// Sets `minecraft:vehicle`.
    #[must_use]
    pub fn with_vehicle(mut self, vehicle: EntityContext) -> Self {
        self.vehicle = Some(Box::new(vehicle));
        self
    }

    /// Inserts a component value for `minecraft:components` checks.
    #[must_use]
    pub fn with_component(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.components.insert(key.into(), value.into());
        self
    }

    /// Sets `minecraft:type_specific/sheep.sheared`.
    #[must_use]
    pub fn with_sheep_sheared(mut self, sheared: bool) -> Self {
        self.sheep_sheared = Some(sheared);
        self
    }

    /// Sets `minecraft:type_specific/cube_mob.size`.
    #[must_use]
    pub fn with_cube_mob_size(mut self, size: i32) -> Self {
        self.cube_mob_size = Some(size);
        self
    }

    /// Sets `minecraft:type_specific/raider.is_captain`.
    #[must_use]
    pub fn with_raider_captain(mut self, is_captain: bool) -> Self {
        self.raider_is_captain = Some(is_captain);
        self
    }

    /// Sets `minecraft:type_specific/fishing_hook.in_open_water` (marks entity as a fishing bobber).
    #[must_use]
    pub fn with_fishing_hook_in_open_water(mut self, in_open_water: bool) -> Self {
        self.fishing_hook_in_open_water = Some(in_open_water);
        self
    }

    /// Appends an entity-type tag matching this entity (for example `"#minecraft:skeletons"`).
    #[must_use]
    pub fn with_entity_type_tag(mut self, tag: impl Into<String>) -> Self {
        self.entity_type_tags.push(tag.into());
        self
    }
}

/// Description of the lethal damage source supplied to [`LootContext`] for
/// evaluating `minecraft:damage_source_properties`.
///
/// Captures only the fields tested across the 8 `damage_source_properties` uses
/// in the vanilla 26.3 entity tables: `direct_entity`, `source_entity`, and
/// damage-type `tags` (`#minecraft:is_projectile`, `#minecraft:is_lightning`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DamageSourceContext {
    /// Optional namespaced damage type id checked against loaded `damage_type` tags.
    pub damage_type: Option<String>,
    /// Explicit damage-type tags present on this damage source (with or without `#`).
    pub tags: Vec<String>,
    /// `direct_entity` — the entity that directly collided with the victim (e.g. fireball).
    pub direct_entity: Option<EntityContext>,
    /// `source_entity` — the originating entity (e.g. frog).
    pub source_entity: Option<EntityContext>,
}

impl DamageSourceContext {
    /// Sets `direct_entity`.
    #[must_use]
    pub fn with_direct_entity(mut self, entity: EntityContext) -> Self {
        self.direct_entity = Some(entity);
        self
    }

    /// Sets `source_entity`.
    #[must_use]
    pub fn with_source_entity(mut self, entity: EntityContext) -> Self {
        self.source_entity = Some(entity);
        self
    }

    /// Sets the damage type identifier.
    #[must_use]
    pub fn with_damage_type(mut self, damage_type: impl Into<String>) -> Self {
        self.damage_type = Some(damage_type.into());
        self
    }

    /// Appends a damage-type tag present on this damage source.
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }
}

/// Execution context for evaluating a [`LootTable`].
#[derive(Clone, Default)]
pub struct LootContext {
    /// Broken block state as `(block_id, properties)`.
    pub block_state: (String, BTreeMap<String, String>),
    /// Breaking tool or fishing rod (`tool`), or `None` when absent.
    pub tool: Option<ToolContext>,
    /// Explosion radius when destroyed by an explosion, or `None` otherwise.
    pub explosion_radius: Option<f32>,
    /// Optional relative block lookup `(dx, dy, dz) -> Option<(block_id, properties)>`
    /// used by `minecraft:location_check`. When `None`, `location_check` evaluates
    /// to `false`.
    pub neighbor_blocks: Option<Arc<dyn BlockLookup>>,
    /// Whether the entity was killed by a player (`minecraft:killed_by_player`).
    pub killed_by_player: bool,
    /// Attacker weapon used for `random_chance_with_enchanted_bonus` and
    /// `enchanted_count_increase` (such as Looting). If `None`, falls back to
    /// `attacker.as_ref().and_then(|e| e.mainhand.as_ref())`.
    pub attacker_weapon: Option<ToolContext>,
    /// Target/victim/hook entity (`"this"` in `minecraft:entity_properties`).
    pub entity: Option<EntityContext>,
    /// Attacking entity (`"attacker"` in `minecraft:entity_properties`).
    pub attacker: Option<EntityContext>,
    /// Direct attacking entity (`"direct_attacker"` in `minecraft:entity_properties`).
    pub direct_attacker: Option<EntityContext>,
    /// Lethal damage source (`minecraft:damage_source_properties`).
    pub damage_source: Option<DamageSourceContext>,
    /// Player luck modifier applied to singleton entry `quality` weights.
    pub luck: f32,
}

impl fmt::Debug for LootContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LootContext")
            .field("block_state", &self.block_state)
            .field("tool", &self.tool)
            .field("explosion_radius", &self.explosion_radius)
            .field("has_neighbor_blocks", &self.neighbor_blocks.is_some())
            .field("killed_by_player", &self.killed_by_player)
            .field("attacker_weapon", &self.attacker_weapon)
            .field("entity", &self.entity)
            .field("attacker", &self.attacker)
            .field("direct_attacker", &self.direct_attacker)
            .field("damage_source", &self.damage_source)
            .field("luck", &self.luck)
            .finish()
    }
}

impl LootContext {
    /// Builds a bare-handed, non-explosion context for `block_id` with empty properties.
    #[must_use]
    pub fn for_block(block_id: impl Into<String>) -> Self {
        Self {
            block_state: (block_id.into(), BTreeMap::new()),
            ..Self::default()
        }
    }

    /// Builds an entity death context for `entity_id` with default flags.
    #[must_use]
    pub fn for_entity(entity_id: impl Into<String>) -> Self {
        Self {
            entity: Some(EntityContext::new(entity_id)),
            ..Self::default()
        }
    }

    /// Sets whether the entity was killed by a player (`killed_by_player`).
    #[must_use]
    pub fn with_killed_by_player(mut self, killed_by_player: bool) -> Self {
        self.killed_by_player = killed_by_player;
        self
    }

    /// Sets the attacker's weapon used for looting / enchanted bonus rolls.
    #[must_use]
    pub fn with_attacker_weapon(mut self, weapon: ToolContext) -> Self {
        self.attacker_weapon = Some(weapon);
        self
    }

    /// Sets the primary `"this"` entity description.
    #[must_use]
    pub fn with_entity(mut self, entity: EntityContext) -> Self {
        self.entity = Some(entity);
        self
    }

    /// Sets the `"attacker"` entity description.
    #[must_use]
    pub fn with_attacker(mut self, attacker: EntityContext) -> Self {
        self.attacker = Some(attacker);
        self
    }

    /// Sets the `"direct_attacker"` entity description.
    #[must_use]
    pub fn with_direct_attacker(mut self, direct_attacker: EntityContext) -> Self {
        self.direct_attacker = Some(direct_attacker);
        self
    }

    /// Sets the lethal damage source description (`damage_source_properties`).
    #[must_use]
    pub fn with_damage_source(mut self, damage_source: DamageSourceContext) -> Self {
        self.damage_source = Some(damage_source);
        self
    }

    /// Sets the player luck level used with singleton entry `quality`.
    #[must_use]
    pub fn with_luck(mut self, luck: f32) -> Self {
        self.luck = luck;
        self
    }

    /// Attaches an optional relative block lookup used by `minecraft:location_check`.
    #[must_use]
    pub fn with_block_lookup(mut self, lookup: impl BlockLookup + 'static) -> Self {
        self.neighbor_blocks = Some(Arc::new(lookup));
        self
    }

    /// Looks up the block id and state properties at offset `(dx, dy, dz)`,
    /// returning `None` if no lookup is attached or the location is unloaded.
    #[must_use]
    pub fn block_at(
        &self,
        dx: i32,
        dy: i32,
        dz: i32,
    ) -> Option<(String, BTreeMap<String, String>)> {
        self.neighbor_blocks
            .as_ref()
            .and_then(|lookup| lookup.block_at(dx, dy, dz))
    }
}

/// Typed note emitted by [`LootTable::roll_with_notes`] when component data or
/// dynamic entries could not be materialized without block-entity state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LootNote {
    /// `minecraft:copy_components` kept the item without copying block-entity/tool components.
    CopyComponentsNotApplied {
        item: String,
        source: String,
        include: Vec<String>,
        exclude: Vec<String>,
    },
    /// `minecraft:copy_state` kept the item without copying block-state properties.
    CopyStateNotApplied {
        item: String,
        block: String,
        properties: Vec<String>,
    },
    /// `minecraft:dynamic` matched and yielded no stacks because dynamic block-entity loot is unmodeled.
    DynamicNotApplied { name: String },
    /// An unsupported modifier kept the item stack unchanged.
    UnsupportedModifier { item: String, what: String },
}

/// Full outcome of rolling a [`LootTable`], containing dropped stacks and typed notes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LootRoll {
    /// Dropped item stacks in roll order.
    pub items: Vec<ItemStack>,
    /// Typed notes for unapplied component modifiers, dynamic entries, or unsupported modifiers.
    pub notes: Vec<LootNote>,
}

#[derive(Debug, Clone, Default)]
struct LootRegistries {
    item_tags: Arc<ItemTags>,
    block_tags: Arc<BlockTags>,
    enchantment_tags: Arc<EnchantmentTags>,
    enchantments: Arc<EnchantmentRegistry>,
    entity_type_tags: Arc<HashMap<String, Vec<String>>>,
    damage_type_tags: Arc<HashMap<String, Vec<String>>>,
    recipes: Option<Arc<RecipeData>>,
    item_defaults: Option<Arc<ItemDefaults>>,
}

/// Parsed loot table loaded from `data/<ns>/loot_table/<category>/**/*.json`.
#[derive(Debug, Clone, Default)]
pub struct LootTable {
    /// Optional loot context type string (for example `"minecraft:block"`, `"minecraft:entity"`, or `"minecraft:chest"`).
    pub table_type: Option<String>,
    /// Optional deterministic random sequence identifier.
    pub random_sequence: Option<String>,
    /// Ordered loot pools (may be empty if `pools` is omitted).
    pub pools: Vec<LootPool>,
    /// Optional table-level modifiers applied to all drops from this table.
    pub modifiers: Vec<LootModifier>,
    registries: LootRegistries,
}

impl PartialEq for LootTable {
    fn eq(&self, other: &Self) -> bool {
        self.table_type == other.table_type
            && self.random_sequence == other.random_sequence
            && self.pools == other.pools
            && self.modifiers == other.modifiers
    }
}

impl LootTable {
    /// Constructs a standalone [`LootTable`] with empty tag, recipe, and item-default registries.
    #[must_use]
    pub fn new(
        table_type: Option<String>,
        random_sequence: Option<String>,
        pools: Vec<LootPool>,
        modifiers: Vec<LootModifier>,
    ) -> Self {
        Self {
            table_type,
            random_sequence,
            pools,
            modifiers,
            registries: LootRegistries::default(),
        }
    }

    /// Attaches tag and item-default registries used during [`Self::roll`].
    #[must_use]
    pub fn with_registries(
        mut self,
        item_tags: Arc<ItemTags>,
        block_tags: Arc<BlockTags>,
        item_defaults: Option<Arc<ItemDefaults>>,
    ) -> Self {
        self.registries.item_tags = item_tags;
        self.registries.block_tags = block_tags;
        self.registries.item_defaults = item_defaults;
        self
    }

    /// Attaches enchantment tags used by `#tag` enchantment predicates and modifier `options`.
    #[must_use]
    pub fn with_enchantment_tags(mut self, enchantment_tags: Arc<EnchantmentTags>) -> Self {
        self.registries.enchantment_tags = enchantment_tags;
        self
    }

    /// Attaches loaded [`EnchantmentRegistry`] definitions used by enchantment modifiers.
    #[must_use]
    pub fn with_enchantments(mut self, enchantments: Arc<EnchantmentRegistry>) -> Self {
        self.registries.enchantments = enchantments;
        self
    }

    /// Attaches loaded [`RecipeData`] used by `minecraft:furnace_smelt`.
    #[must_use]
    pub fn with_recipes(mut self, recipes: Arc<RecipeData>) -> Self {
        self.registries.recipes = Some(recipes);
        self
    }

    /// Returns the `what` labels of every [`LootCondition::Unsupported`] and
    /// [`LootModifier::Unsupported`] node anywhere in this table.
    #[must_use]
    pub fn unsupported(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for modifier in &self.modifiers {
            collect_unsupported_modifier(modifier, &mut out);
        }
        for pool in &self.pools {
            for condition in &pool.conditions {
                collect_unsupported_condition(condition, &mut out);
            }
            for modifier in &pool.modifiers {
                collect_unsupported_modifier(modifier, &mut out);
            }
            for entry in &pool.entries {
                collect_unsupported_entry(entry, &mut out);
            }
        }
        out
    }

    /// Evaluates this block or entity loot table under `ctx` using `random` and returns dropped stacks.
    ///
    /// Uses real maximum stack sizes from [`ItemDefaults`] when loaded from the data root's
    /// `reports/minecraft/components/item/*.json`, falling back to [`default_max_stack`]
    /// otherwise.
    ///
    /// Semantics follow `minecraft.wiki` and the task's binding reviewer decisions:
    /// - **Pool & entry expansion** (`Loot table` rev 3893957):
    ///   - `"Each roll of a pool is independent."`
    ///   - `"composite entries first expand their children, and then singleton entries that do not meet their conditions are removed from the pool, then entries are rolled afterward."`
    ///   - `weight`: `"Determines how often this singleton entry is chosen out of all the singleton entries in the pool."` / `"The chance of an entry being chosen is [this entry's weight ÷ total weight of all entries in the pool]."`
    ///   - `alternatives`: `"Extracts only the first successful (conditions are met) entry. Conditions are checked in order."`
    ///   - `loot_table`: `"Drops the loot from another loot table."` Rolls the referenced or inline table up to [`MAX_LOOT_TABLE_DEPTH`] and passes its dropped stacks through outer entry, pool, and table modifiers.
    ///   - `empty`: `"Drops nothing."` Participates in weighted pool selection with its weight and emits no items when chosen.
    ///   - `tag`: `"If expand is false, it is a singleton entry: When selected in a roll, generates item stacks for all items in the specified item tag."` / `"If expand is true, it is a composite entry: Expands into multiple singleton entries with the specified weight and conditions, for each item in the item tag."` (`"Item modifiers do not work"` on expanded tags, Bug MC-212671.)
    ///   - `dynamic`: `"Drops block-specific loot."` Kept unmaterialized and recorded in [`LootNote::DynamicNotApplied`] via [`Self::roll_with_notes`].
    ///   - Table `modifier`: `"Applies an item modifier, onto all item stacks dropped by this table."`
    /// - **Conditions** (`Predicate` rev 3888899, `Data component predicate` rev 3826514, `Advancement/Conditions/damage type` rev 3778836, `Enchantment definition` rev 3843772):
    ///   - `survives_explosion`: `"Returns success with 1 ÷ explosion radius probability. Requires explosion radius provided by loot context, and always success if not provided."`
    ///   - `table_bonus`: `"Passes with probability picked from a list, indexed by enchantment power. Requires tool provided by loot context. If not provided, the enchantment level is regarded as 0."`
    ///     Reviewer decision: levels beyond the list use `chances[min(level, len - 1)]`, backed by `Fortune` (rev 3864999), where `"Fortune IV+"` for `"Flint"` from `"Gravel"` shares `"100%"` with Fortune III.
    ///   - `random_chance`: `"Generates a random number between 0.0 and 1.0, and checks if it is less than a specified value."`
    ///   - `random_chance_with_enchanted_bonus`: `"Generates a random number between 0.0 and 1.0, and checks if it is less than the value determined using the level of a given enchantment. Requires attacker entity provided by loot context, and if not provided, the enchantment level is regarded as 0."` Uses `unenchanted_chance` at level 0 and `enchanted_chance` (`Level-based value`, e.g. `linear`: `"The final value is base + per_level_above_first * (level - 1)."`) when present.
    ///   - `killed_by_player`: `"Checks if there is a attacking_player entity provided by loot context. Requires attacking_player entity provided by loot context, and always fails if not provided."`
    ///   - `match_tool`: `"Checks tool used to mine the block. Requires tool provided by loot context, and always fails if not provided."` (`minecraft:enchantments`: `"Array of conditions. The sub-predicate succeeds when all the conditions succeed."`)
    ///   - `location_check` (`Predicate` rev 3888899, `Advancement/Conditions/location` rev 3684921, `Advancement/Conditions/block` rev 3684708): `"Checks the current location against location criteria. Requires origin provided by loot context, and always fails if not provided."` Queries `LootContext::block_at(offsetX, offsetY, offsetZ)` and matches `predicate.block` (`blocks` and `state`); evaluates to `false` when no neighbour lookup is attached or `block_at` returns `None`. Any other location or block predicate field remains `Unsupported` (`false`).
    ///   - `entity_properties`: `"Checks properties of an entity. Invokable from any context."` Evaluates `this`, `attacker`, or `direct_attacker` from [`LootContext`], returning `false` if absent; only tested fields (`minecraft:flags`, `minecraft:entity_type`, `minecraft:vehicle`, `minecraft:equipment.mainhand`, `minecraft:components`, `minecraft:type_specific/sheep.sheared`, `minecraft:type_specific/cube_mob.size`, `minecraft:type_specific/raider.is_captain`) are modeled and all others stay `Unsupported`.
    ///   - `damage_source_properties`: `"Checks properties of the damage source. Requires origin and damage source provided by loot context, and always fails if not provided."` Matches `direct_entity`, `source_entity`, and `tags` (`id`, `expected`); unmodeled fields stay `Unsupported`.
    ///   - `Unsupported`: evaluates to `false`.
    /// - **Modifiers** (`Item modifier` rev 3896146, `Fortune` rev 3864999, `Explosion` rev 3903416, `Drops` rev 3884009):
    ///   - `enchanted_count_increase`: `"Adjusts the stack size based on the level of the specified enchantment on the killer entity provided by loot context."` / `count`: `"Specifies the number of additional items per level of the enchantment. Note the number may be fractional, rounded after multiplying by the enchantment level."` / `limit`: `"Specifies the maximum amount of items in the stack after the enchantment calculation. If the value is 0, no limit is applied. Defaults to 0."`
    ///   - `furnace_smelt`: `"Smelts the item as it would be in a furnace without changing its count."` Looks up `CookingKind::Smelting` in loaded `RecipeData`; when no recipe matches, leaves the item unchanged.
    ///   - `set_potion`: `"Sets the potion_contents component of an item."` (`"The potion ID. Set to empty to remove the Potion tag."`)
    ///   - `set_damage` (`Item modifier` rev 3896146; `Data component format/damage` rev 3536877): `"Sets the item's damage value (durability)."` `damage`: `"Specifies the durability fraction to set (1.0 is undamaged, 0.0 is zero durability left)."` `add`: `"If true, change is relative to current damage. Defaults to false."` Converts remaining durability fraction to `minecraft:damage` uses consumed from `ItemDefaults::max_damage`, leaving non-damageable items unchanged.
    ///   - `set_name` (`Item modifier` rev 3896146; `Data component format/custom name` rev 3863465): `"Adds or changes the item's custom name."` `target`: `"Allowed values are custom_name or item_name, corresponding with the component to be set. Defaults to custom_name."` Plain-text `custom_name` sets `ItemComponents::set_custom_name`; `target: "item_name"` and non-plain-text text components stay [`LootModifier::Unsupported`] with [`LootNote::UnsupportedModifier`].
    ///   - `set_enchantments` (`Item modifier` rev 3896146): `"Modifies the item's enchantments. A book converts to an enchanted book."` `"Powers are clamped at a maximum of 255. Decreasing an enchantment power below 1 remove the enchantment, through a relative negative add or through a direct value assignemnt."` `add`: `"If true, change is relative to current level. A nonexistent enchantment is presumed to start at 0. Defaults to false."`
    ///   - `enchant_randomly` (`Item modifier` rev 3896146): `"Enchants the item with one randomly-selected enchantment. The level of the enchantment, if applicable, is random. A book converts to an enchanted book when enchanted."` `only_compatible`: `"Whether only enchantments that are compatible with the item should be chosen (i.e. the item is listed in the enchantments supported_items). Books are always considered compatible."`
    ///   - `enchant_with_levels` (`Item modifier` rev 3896146; `Enchanting table mechanics` rev 3881519): `"Enchants the item, with the specified enchantment level (roughly equivalent to using an enchanting table at that level). A book converts to an enchanted book."` Reuses [`select_enchantments_for_loot`], keeping multiple selected enchantments on books (`"if the item is a book, an enchanting table is used, and multiple enchantments were selected, a random one is removed"`).
    ///   - `filtered` & `discard` (`Item modifier` rev 3896146; `Advancement/Conditions/item` rev 3286549; `Data component predicate` rev 3826514): `"Applies an Item Predicate to the target item, and runs modifiers according to whether the predicate passes or fails."` `discard`: `"Removes the item stack."` Unmodeled item predicates (such as `minecraft:map_id`) keep `filtered` as [`LootModifier::Unsupported`].
    ///   - `set_ominous_bottle_amplifier`, `exploration_map`, `set_stew_effect`, `set_instrument`: kept as [`LootModifier::Unsupported`] because `ItemComponents` does not model those components.
    ///   - `apply_bonus` (`ore_drops`): `"Count *= (max(1; randomInt(0(inclusive) .. (Level + 2)(exclusive))))"` — reviewer decision: `count *= max(1, random_int(0 ..= level + 1))`.
    ///   - `apply_bonus` (`uniform_bonus_count`): `"uniform distribution (from 0 to level * bonusMultiplier)"` — reviewer decision: `count += random_int(0 ..= level * bonusMultiplier)`.
    ///   - `apply_bonus` (`binomial_with_bonus_count`): `Item modifier` rev 3896146 writes `"for 1 bonus item plus bonus determined by binomial distribution with n=level + extra, p=probability"`, whereas `Fortune` rev 3864999 shows `"the binomial distribution in the default case is created by rolling three times (n=3) with a drop probability of 57% (p=4/7), then an additional seed is added"` (`"Average drops"` = `"2.71 (2 5/7)"` = `1 + 3 * 4/7`). Reviewer decision: `count += Binomial(n = level + extra, p = probability)` with **no extra +1**, and missing enchantment = level 0.
    ///   - `explosion_decay`: `Explosion` rev 3903416 states `"Other explosions have a 1/power chance of dropping items."` and `Predicate` rev 3888899 states `"1 ÷ explosion radius probability"`, contradicting `Item modifier` rev 3896146 (`"Each item in the item stack has a chance of 1/explosion radius to be lost."`). Reviewer decision: each item in the stack is **kept** with probability `1 / explosion_radius`, and no explosion radius in context means no decay.
    ///   - `limit_count`: `"Limits the count between a minimum and maximum value."` Reviewer decision: clamps to bounds; counts `<= 0` after all modifiers produce no stack.
    ///   - `copy_components` / `copy_state`: keep the item without inventing component data and record a typed [`LootNote`].
    /// - **Number providers** (`Number provider` rev 3869456):
    ///   - `uniform`: `"The values are inclusive, meaning the provided number can be either min or max, except if the provided number is a float, in which case it must be strictly less than max. If max <= min, the provided number is min."` Reviewer decision: integer `min`/`max` draws an integer uniformly from `min ..= max` inclusive.
    ///   - `round`: `"Returns the rounded (towards the nearest integer, with ties rounding towards positive infinity) value of the input."`
    ///   - `binomial`: `"A random number following a binomial distribution."` (`n` = `"The amount of trials"`, `p` = `"The probability of success on an individual trial"`).
    pub fn roll(&self, ctx: &LootContext, random: &mut RandomSource) -> Vec<ItemStack> {
        self.roll_with_notes(ctx, random).items
    }

    /// Evaluates this block or entity loot table under `ctx` and returns both dropped stacks and
    /// typed [`LootNote`] records for unapplied component/dynamic entries.
    pub fn roll_with_notes(&self, ctx: &LootContext, random: &mut RandomSource) -> LootRoll {
        self.roll_with(
            ctx,
            random,
            &self.registries.item_tags,
            &self.registries.block_tags,
            self.registries.item_defaults.as_deref(),
        )
    }

    /// Evaluates this loot table using explicit `item_tags`, `block_tags`, and `item_defaults`.
    pub fn roll_with(
        &self,
        ctx: &LootContext,
        random: &mut RandomSource,
        item_tags: &ItemTags,
        block_tags: &BlockTags,
        item_defaults: Option<&ItemDefaults>,
    ) -> LootRoll {
        let eval_ctx = EvalCtx {
            loot: ctx,
            item_tags,
            block_tags,
            enchantment_tags: &self.registries.enchantment_tags,
            enchantments: &self.registries.enchantments,
            entity_type_tags: &self.registries.entity_type_tags,
            damage_type_tags: &self.registries.damage_type_tags,
            recipes: self.registries.recipes.as_deref(),
            item_defaults,
        };
        let mut outcome = LootRoll::default();
        let mut drops = Vec::new();
        self.roll_into_drops(&eval_ctx, random, &mut outcome.notes, &mut drops, 0);
        for drop in drops {
            push_item_stacks(drop, item_defaults, &mut outcome.items);
        }
        outcome
    }

    fn roll_into_drops(
        &self,
        eval_ctx: &EvalCtx<'_>,
        random: &mut RandomSource,
        notes: &mut Vec<LootNote>,
        out_drops: &mut Vec<PendingDrop>,
        depth: usize,
    ) {
        if depth > MAX_LOOT_TABLE_DEPTH {
            return;
        }
        let is_fishing = self
            .table_type
            .as_deref()
            .is_some_and(|kind| normalize_item_id(kind) == "fishing");
        let base_luck = if eval_ctx.loot.luck.is_finite() {
            f64::from(eval_ctx.loot.luck)
        } else {
            0.0
        };
        let effective_luck = base_luck
            + if is_fishing {
                f64::from(tool_enchantment_level(
                    eval_ctx.loot.tool.as_ref(),
                    "minecraft:luck_of_the_sea",
                ))
            } else {
                0.0
            };

        for pool in &self.pools {
            if !eval_conditions(&pool.conditions, eval_ctx, random) {
                continue;
            }
            let rolls = sample_count_provider(&pool.rolls, random);
            if rolls <= 0 {
                continue;
            }

            for _ in 0..rolls {
                let mut candidates = Vec::new();
                for entry in &pool.entries {
                    expand_entry(entry, eval_ctx, random, &mut candidates);
                }
                let Some(selected) =
                    pick_weighted_candidate(&mut candidates, effective_luck, random)
                else {
                    continue;
                };

                let mut generated = Vec::new();
                match selected.kind {
                    SelectedEntryKind::Dynamic(name) => {
                        notes.push(LootNote::DynamicNotApplied {
                            name: name.to_owned(),
                        });
                    }
                    SelectedEntryKind::Empty => {}
                    SelectedEntryKind::Item(item_name) => {
                        generated.push(PendingDrop::new(ensure_namespaced(item_name)));
                    }
                    SelectedEntryKind::Tag(tag_id) => {
                        if let Some(members) = eval_ctx.item_tags.members(tag_id) {
                            for member in members {
                                generated.push(PendingDrop::new(ensure_namespaced(member)));
                            }
                        }
                    }
                    SelectedEntryKind::LootTable(source) => {
                        let sub_table = match source {
                            LootTableSource::Reference { table, .. } => table.as_deref(),
                            LootTableSource::Inline(table) => Some(table.as_ref()),
                        };
                        if let Some(sub) = sub_table {
                            sub.roll_into_drops(eval_ctx, random, notes, &mut generated, depth + 1);
                        }
                    }
                }

                for mut drop in generated {
                    let all_modifiers = selected
                        .modifiers
                        .iter()
                        .copied()
                        .chain(pool.modifiers.iter())
                        .chain(self.modifiers.iter());
                    for modifier in all_modifiers {
                        if drop.discarded {
                            break;
                        }
                        if eval_conditions(modifier.conditions(), eval_ctx, random) {
                            apply_modifier(modifier, &mut drop, eval_ctx, random, notes);
                        }
                    }
                    if !drop.discarded && drop.count > 0 {
                        out_drops.push(drop);
                    }
                }
            }
        }
    }
}

/// Loaded collection of loot tables for one or more categories (for example `blocks`,
/// `entities`, `chests`, `gameplay`, or `archaeology`).
#[derive(Debug, Clone, Default)]
pub struct LootTables {
    tables: BTreeMap<String, LootTable>,
    external_tables: BTreeMap<String, LootTable>,
    registries: LootRegistries,
}

impl PartialEq for LootTables {
    fn eq(&self, other: &Self) -> bool {
        self.tables == other.tables && self.external_tables == other.external_tables
    }
}

fn expected_type_for_category(category: &str) -> Option<&'static str> {
    match category {
        "blocks" => Some("block"),
        "entities" => Some("entity"),
        "chests" => Some("chest"),
        "archaeology" => Some("archaeology"),
        _ => None,
    }
}

impl LootTables {
    /// Loads every `data/<ns>/loot_table/blocks/**/*.json` file under `root`,
    /// resolves string condition references through `data/<ns>/predicate/**`,
    /// resolves any `loot_table` entry references, and loads `ItemTags`,
    /// `BlockTags`, `EnchantmentRegistry`, and optional `ItemDefaults` (when
    /// `reports/minecraft/components/item` is present).
    pub fn load_blocks(root: &Path) -> Result<Self, LootError> {
        Self::load_categories(root, &["blocks"])
    }

    /// Loads every `data/<ns>/loot_table/entities/**/*.json` file under `root`
    /// with table ids `<ns>:entities/<path>`, resolving predicate and
    /// `loot_table` references (including external sub-tables such as
    /// `minecraft:gameplay/fishing/fish`).
    pub fn load_entities(root: &Path) -> Result<Self, LootError> {
        Self::load_categories(root, &["entities"])
    }

    /// Loads every `data/<ns>/loot_table/chests/**/*.json` file under `root`
    /// with table ids `<ns>:chests/<path>`.
    pub fn load_chests(root: &Path) -> Result<Self, LootError> {
        Self::load_categories(root, &["chests"])
    }

    /// Loads every `data/<ns>/loot_table/gameplay/**/*.json` file under `root`
    /// (including subfolders such as `fishing/` and `hero_of_the_village/`)
    /// with table ids `<ns>:gameplay/<path>`.
    pub fn load_gameplay(root: &Path) -> Result<Self, LootError> {
        Self::load_categories(root, &["gameplay"])
    }

    /// Loads every `data/<ns>/loot_table/archaeology/**/*.json` file under `root`
    /// with table ids `<ns>:archaeology/<path>`.
    pub fn load_archaeology(root: &Path) -> Result<Self, LootError> {
        Self::load_categories(root, &["archaeology"])
    }

    /// Loads all specified `loot_table/<category>/**/*.json` categories under `root`
    /// with ids `<ns>:<category>/<path>`, resolving `loot_table` references across
    /// all loaded categories (as well as external categories under `data/<ns>/loot_table`).
    pub fn load_categories(root: &Path, categories: &[&str]) -> Result<Self, LootError> {
        let data_dir = root.join("data");
        let Some(namespaces) = read_dir_optional(&data_dir)? else {
            return Ok(Self::default());
        };

        let item_tags = Arc::new(ItemTags::load(root)?);
        let block_tags = Arc::new(BlockTags::load(root)?);
        let enchantment_tags = Arc::new(EnchantmentTags::load(root)?);
        let entity_type_tags = Arc::new(load_tag_entries(root, "entity_type")?);
        let damage_type_tags = Arc::new(load_tag_entries(root, "damage_type")?);
        let item_defaults = match ItemDefaults::load(root) {
            Ok(defaults) => Some(Arc::new(defaults)),
            Err(ItemDefaultsError::MissingReports(_)) => None,
            Err(error) => return Err(LootError::ItemDefaults(error)),
        };
        let recipes = Some(Arc::new(RecipeData::load_with_registries(
            root,
            &item_tags,
            item_defaults.as_deref(),
        )?));

        let mut ns_dirs = Vec::new();
        for entry in namespaces {
            let entry = entry.map_err(|error| WorldgenError::Io(data_dir.clone(), error))?;
            let path = entry.path();
            if entry
                .file_type()
                .map_err(|error| WorldgenError::Io(path.clone(), error))?
                .is_dir()
            {
                ns_dirs.push(path);
            }
        }
        ns_dirs.sort();

        let mut predicate_paths: HashMap<String, PathBuf> = HashMap::new();
        for ns_path in &ns_dirs {
            let ns = dir_name(ns_path);
            let pred_dir = ns_path.join("predicate");
            for file in walk_json(&pred_dir)? {
                let id = format!("{ns}:{}", relative_stem(&file, &pred_dir));
                predicate_paths.insert(id, file);
            }
        }

        let mut pred_resolver = PredicateResolver {
            root,
            paths: predicate_paths,
            cache: HashMap::new(),
        };

        let mut unique_categories = Vec::new();
        for &category in categories {
            if !unique_categories.contains(&category) {
                unique_categories.push(category);
            }
        }

        let mut raw_tables: HashMap<String, (PathBuf, LootTable)> = HashMap::new();
        let mut ordered_ids = Vec::new();
        for &category in &unique_categories {
            let expected_type = expected_type_for_category(category);
            for ns_path in &ns_dirs {
                let ns = dir_name(ns_path);
                let cat_dir = ns_path.join("loot_table").join(category);
                for file in walk_json(&cat_dir)? {
                    let id = format!("{ns}:{category}/{}", relative_stem(&file, &cat_dir));
                    let value = read_json(&file)?;
                    let table =
                        parse_loot_table_inner(&value, &file, &mut pred_resolver, expected_type)?;
                    ordered_ids.push(id.clone());
                    raw_tables.insert(id, (file, table));
                }
            }
        }

        let needs_enchantments = unique_categories.iter().any(|&c| c != "blocks")
            || raw_tables.values().any(|(_, t)| table_uses_enchantments(t));
        let enchantments = if needs_enchantments {
            Arc::new(EnchantmentRegistry::load(
                root,
                &item_tags,
                &enchantment_tags,
            )?)
        } else {
            Arc::new(EnchantmentRegistry::default())
        };
        let registries = LootRegistries {
            item_tags,
            block_tags,
            enchantment_tags,
            enchantments,
            entity_type_tags,
            damage_type_tags,
            recipes,
            item_defaults,
        };
        for (_, table) in raw_tables.values_mut() {
            table.registries = registries.clone();
        }

        let mut table_resolver = LootTableResolver {
            root,
            registries: &registries,
            pred_resolver: &mut pred_resolver,
            raw_category: raw_tables,
            resolved_category: BTreeMap::new(),
            external_cache: BTreeMap::new(),
        };
        let mut visiting = HashSet::new();
        for id in ordered_ids {
            table_resolver.resolve_category_id(&id, &mut visiting, 0)?;
        }

        let tables = table_resolver
            .resolved_category
            .into_iter()
            .map(|(id, arc)| (id, (*arc).clone()))
            .collect();
        let external_tables = table_resolver
            .external_cache
            .into_iter()
            .map(|(id, arc)| (id, (*arc).clone()))
            .collect();

        Ok(Self {
            tables,
            external_tables,
            registries,
        })
    }

    /// Looks up a loot table by full `<ns>:<category>/<path>` identifier (defaulting
    /// missing namespace to `minecraft:`).
    #[must_use]
    pub fn get_table(&self, table_id: &str) -> Option<&LootTable> {
        let key = ensure_namespaced(table_id);
        self.tables.get(&key)
    }

    /// Looks up the block loot table for `block_id`.
    ///
    /// Accepts `"minecraft:stone"`, `"stone"`, or `"minecraft:blocks/stone"`
    /// (and similarly `"ns:block"` or `"ns:blocks/block"` for custom namespaces).
    #[must_use]
    pub fn get(&self, block_id: &str) -> Option<&LootTable> {
        let key = normalize_category_table_id(block_id, "blocks");
        self.tables.get(&key)
    }

    /// Looks up the entity loot table for `entity_id`.
    ///
    /// Accepts `"minecraft:cow"`, `"cow"`, or `"minecraft:entities/cow"`
    /// (and similarly `"ns:mob"` or `"ns:entities/mob"` for custom namespaces).
    #[must_use]
    pub fn get_entity(&self, entity_id: &str) -> Option<&LootTable> {
        let key = normalize_category_table_id(entity_id, "entities");
        self.tables.get(&key)
    }

    /// Looks up the chest loot table for `chest_id`.
    ///
    /// Accepts `"minecraft:simple_dungeon"`, `"simple_dungeon"`, or `"minecraft:chests/simple_dungeon"`.
    #[must_use]
    pub fn get_chest(&self, chest_id: &str) -> Option<&LootTable> {
        let key = normalize_category_table_id(chest_id, "chests");
        self.tables.get(&key)
    }

    /// Looks up the gameplay loot table for `gameplay_id`.
    ///
    /// Accepts `"minecraft:piglin_bartering"`, `"fishing/fish"`, or `"minecraft:gameplay/fishing/fish"`.
    #[must_use]
    pub fn get_gameplay(&self, gameplay_id: &str) -> Option<&LootTable> {
        let key = normalize_category_table_id(gameplay_id, "gameplay");
        self.tables.get(&key)
    }

    /// Looks up the archaeology loot table for `archaeology_id`.
    ///
    /// Accepts `"minecraft:desert_pyramid"`, `"desert_pyramid"`, or `"minecraft:archaeology/desert_pyramid"`.
    #[must_use]
    pub fn get_archaeology(&self, archaeology_id: &str) -> Option<&LootTable> {
        let key = normalize_category_table_id(archaeology_id, "archaeology");
        self.tables.get(&key)
    }

    /// Rolls the loot table identified by full `<ns>:<category>/<path>` id if present.
    pub fn roll_table(
        &self,
        table_id: &str,
        ctx: &LootContext,
        random: &mut RandomSource,
    ) -> Option<LootRoll> {
        self.get_table(table_id)
            .map(|table| table.roll_with_notes(ctx, random))
    }

    /// Rolls the block loot table for `block_id` if present, returning stacks and typed notes.
    pub fn roll(
        &self,
        block_id: &str,
        ctx: &LootContext,
        random: &mut RandomSource,
    ) -> Option<LootRoll> {
        self.get(block_id)
            .map(|table| table.roll_with_notes(ctx, random))
    }

    /// Rolls the entity loot table for `entity_id` if present, returning stacks and typed notes.
    pub fn roll_entity(
        &self,
        entity_id: &str,
        ctx: &LootContext,
        random: &mut RandomSource,
    ) -> Option<LootRoll> {
        self.get_entity(entity_id)
            .map(|table| table.roll_with_notes(ctx, random))
    }

    /// Rolls the chest loot table for `chest_id` if present, returning stacks and typed notes.
    pub fn roll_chest(
        &self,
        chest_id: &str,
        ctx: &LootContext,
        random: &mut RandomSource,
    ) -> Option<LootRoll> {
        self.get_chest(chest_id)
            .map(|table| table.roll_with_notes(ctx, random))
    }

    /// Rolls the gameplay loot table for `gameplay_id` if present, returning stacks and typed notes.
    pub fn roll_gameplay(
        &self,
        gameplay_id: &str,
        ctx: &LootContext,
        random: &mut RandomSource,
    ) -> Option<LootRoll> {
        self.get_gameplay(gameplay_id)
            .map(|table| table.roll_with_notes(ctx, random))
    }

    /// Rolls the archaeology loot table for `archaeology_id` if present, returning stacks and typed notes.
    pub fn roll_archaeology(
        &self,
        archaeology_id: &str,
        ctx: &LootContext,
        random: &mut RandomSource,
    ) -> Option<LootRoll> {
        self.get_archaeology(archaeology_id)
            .map(|table| table.roll_with_notes(ctx, random))
    }

    /// Returns the loaded item tag registry used during condition matching.
    #[must_use]
    pub fn item_tags(&self) -> &ItemTags {
        &self.registries.item_tags
    }

    /// Returns the loaded block tag registry used during condition matching.
    #[must_use]
    pub fn block_tags(&self) -> &BlockTags {
        &self.registries.block_tags
    }

    /// Returns the loaded item defaults, when present under `reports/`.
    #[must_use]
    pub fn item_defaults(&self) -> Option<&ItemDefaults> {
        self.registries.item_defaults.as_deref()
    }

    /// Returns the loaded enchantment tag registry used during condition matching and enchantment options.
    #[must_use]
    pub fn enchantment_tags(&self) -> &EnchantmentTags {
        &self.registries.enchantment_tags
    }

    /// Returns the loaded enchantment definition registry.
    #[must_use]
    pub fn enchantments(&self) -> &EnchantmentRegistry {
        &self.registries.enchantments
    }

    /// Returns the loaded recipe registry used by `minecraft:furnace_smelt`.
    #[must_use]
    pub fn recipes(&self) -> Option<&RecipeData> {
        self.registries.recipes.as_deref()
    }

    /// Returns the number of loaded primary loot tables.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tables.len()
    }

    /// Returns `true` when no primary loot tables were loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    /// Iterates over `(table_id, table)` pairs in deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &LootTable)> {
        self.tables.iter().map(|(id, table)| (id.as_str(), table))
    }

    /// Lists `(table_id, what)` for every [`LootCondition::Unsupported`] and
    /// [`LootModifier::Unsupported`] node across all loaded tables (including
    /// any externally referenced sub-tables).
    #[must_use]
    pub fn unsupported(&self) -> Vec<(&str, &str)> {
        let mut out = Vec::new();
        for (id, table) in self.tables.iter().chain(self.external_tables.iter()) {
            for what in table.unsupported() {
                out.push((id.as_str(), what));
            }
        }
        out
    }
}

struct LootTableResolver<'a> {
    root: &'a Path,
    registries: &'a LootRegistries,
    pred_resolver: &'a mut PredicateResolver<'a>,
    raw_category: HashMap<String, (PathBuf, LootTable)>,
    resolved_category: BTreeMap<String, Arc<LootTable>>,
    external_cache: BTreeMap<String, Arc<LootTable>>,
}

impl LootTableResolver<'_> {
    fn resolve_category_id(
        &mut self,
        id: &str,
        visiting: &mut HashSet<String>,
        depth: usize,
    ) -> Result<Arc<LootTable>, LootError> {
        if let Some(existing) = self.resolved_category.get(id) {
            return Ok(Arc::clone(existing));
        }
        let Some((file, mut table)) = self.raw_category.remove(id) else {
            return Err(invalid(
                Path::new(id),
                format!("internal error resolving loot table `{id}`"),
            ));
        };
        if depth > MAX_LOOT_TABLE_DEPTH {
            return Err(invalid(
                &file,
                format!("loot_table nesting depth exceeds maximum ({MAX_LOOT_TABLE_DEPTH})"),
            ));
        }
        visiting.insert(id.to_owned());
        let res = self.resolve_table(&mut table, &file, visiting, depth);
        visiting.remove(id);
        res?;
        let arc = Arc::new(table);
        self.resolved_category
            .insert(id.to_owned(), Arc::clone(&arc));
        Ok(arc)
    }

    fn resolve_reference(
        &mut self,
        target_id: &str,
        referrer_file: &Path,
        visiting: &mut HashSet<String>,
        depth: usize,
    ) -> Result<Arc<LootTable>, LootError> {
        let normalized = ensure_namespaced(target_id);
        validate_resource_id(&normalized, "loot_table", referrer_file)?;
        if depth > MAX_LOOT_TABLE_DEPTH {
            return Err(invalid(
                referrer_file,
                format!("loot_table nesting depth exceeds maximum ({MAX_LOOT_TABLE_DEPTH})"),
            ));
        }
        if visiting.contains(&normalized) {
            return Err(invalid(
                referrer_file,
                format!("cyclic loot_table reference `{normalized}`"),
            ));
        }
        if let Some(existing) = self.resolved_category.get(&normalized) {
            return Ok(Arc::clone(existing));
        }
        if let Some(existing) = self.external_cache.get(&normalized) {
            return Ok(Arc::clone(existing));
        }
        if self.raw_category.contains_key(&normalized) {
            return self.resolve_category_id(&normalized, visiting, depth);
        }

        let target_file = expected_loot_table_path(self.root, &normalized);
        if !target_file.exists() {
            return Err(LootError::MissingLootTable {
                table_file: referrer_file.to_path_buf(),
                loot_table: normalized,
                loot_table_file: target_file,
            });
        }
        let value = read_json(&target_file)?;
        let mut table = parse_loot_table_inner(&value, &target_file, self.pred_resolver, None)?;
        table.registries = self.registries.clone();
        visiting.insert(normalized.clone());
        let res = self.resolve_table(&mut table, &target_file, visiting, depth);
        visiting.remove(&normalized);
        res?;
        let arc = Arc::new(table);
        self.external_cache.insert(normalized, Arc::clone(&arc));
        Ok(arc)
    }

    fn resolve_table(
        &mut self,
        table: &mut LootTable,
        file: &Path,
        visiting: &mut HashSet<String>,
        depth: usize,
    ) -> Result<(), LootError> {
        for pool in &mut table.pools {
            for entry in &mut pool.entries {
                self.resolve_entry(entry, file, visiting, depth)?;
            }
        }
        Ok(())
    }

    fn resolve_entry(
        &mut self,
        entry: &mut LootEntry,
        file: &Path,
        visiting: &mut HashSet<String>,
        depth: usize,
    ) -> Result<(), LootError> {
        match entry {
            LootEntry::Alternatives { children, .. } => {
                for child in children {
                    self.resolve_entry(child, file, visiting, depth)?;
                }
            }
            LootEntry::LootTable { source, .. } => match source {
                LootTableSource::Reference { id, table } => {
                    let resolved = self.resolve_reference(id, file, visiting, depth + 1)?;
                    *id = ensure_namespaced(id);
                    *table = Some(resolved);
                }
                LootTableSource::Inline(inline) => {
                    if depth + 1 > MAX_LOOT_TABLE_DEPTH {
                        return Err(invalid(
                            file,
                            format!(
                                "loot_table nesting depth exceeds maximum ({MAX_LOOT_TABLE_DEPTH})"
                            ),
                        ));
                    }
                    inline.registries = self.registries.clone();
                    self.resolve_table(inline, file, visiting, depth + 1)?;
                }
            },
            LootEntry::Item { .. }
            | LootEntry::Dynamic { .. }
            | LootEntry::Empty { .. }
            | LootEntry::Tag { .. } => {}
        }
        Ok(())
    }
}

struct PredicateResolver<'a> {
    root: &'a Path,
    paths: HashMap<String, PathBuf>,
    cache: HashMap<String, LootCondition>,
}

impl PredicateResolver<'_> {
    fn resolve(
        &mut self,
        id: &str,
        table_file: &Path,
        stack: &mut HashSet<String>,
    ) -> Result<LootCondition, LootError> {
        let normalized = ensure_namespaced(id);
        validate_resource_id(&normalized, "predicate", table_file)?;
        if let Some(cached) = self.cache.get(&normalized) {
            return Ok(cached.clone());
        }
        if !stack.insert(normalized.clone()) {
            return Err(invalid(
                table_file,
                format!("cyclic predicate reference `{normalized}`"),
            ));
        }

        let pred_file = match self.paths.get(&normalized) {
            Some(path) => path.clone(),
            None => {
                stack.remove(&normalized);
                return Err(LootError::MissingPredicate {
                    table_file: table_file.to_path_buf(),
                    predicate: normalized.clone(),
                    predicate_file: expected_predicate_path(self.root, &normalized),
                });
            }
        };

        let value = read_json(&pred_file)?;
        let parsed = parse_single_condition(&value, &pred_file, self, stack);
        stack.remove(&normalized);
        let cond = parsed?;
        self.cache.insert(normalized, cond.clone());
        Ok(cond)
    }
}

fn dir_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

fn relative_stem(path: &Path, base: &Path) -> String {
    let relative = path.strip_prefix(base).unwrap_or(path);
    relative
        .with_extension("")
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn ensure_namespaced(id: &str) -> String {
    if id.contains(':') {
        id.to_owned()
    } else {
        format!("minecraft:{id}")
    }
}

fn normalize_category_table_id(id: &str, category: &str) -> String {
    let prefix = format!("{category}/");
    if let Some((ns, path)) = id.split_once(':') {
        if path.starts_with(&prefix) {
            format!("{ns}:{path}")
        } else {
            format!("{ns}:{prefix}{path}")
        }
    } else {
        format!("minecraft:{prefix}{id}")
    }
}

fn validate_resource_id(id: &str, kind: &str, file: &Path) -> Result<(), LootError> {
    let Some((ns, path)) = id.split_once(':') else {
        return Err(invalid(file, format!("invalid {kind} id `{id}`")));
    };
    if ns.is_empty()
        || path.is_empty()
        || ns.contains("..")
        || path.split('/').any(|seg| seg.is_empty() || seg == "..")
    {
        return Err(invalid(file, format!("invalid {kind} id `{id}`")));
    }
    Ok(())
}

fn expected_resource_path(root: &Path, subdir: &str, id: &str) -> PathBuf {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    let mut out = root.join("data").join(ns).join(subdir);
    for segment in path.split('/') {
        out.push(segment);
    }
    out.set_extension("json");
    out
}

fn expected_predicate_path(root: &Path, id: &str) -> PathBuf {
    expected_resource_path(root, "predicate", id)
}

fn expected_loot_table_path(root: &Path, id: &str) -> PathBuf {
    expected_resource_path(root, "loot_table", id)
}

fn invalid(file: &Path, message: impl Into<String>) -> LootError {
    LootError::Invalid {
        file: file.to_path_buf(),
        message: message.into(),
    }
}

fn check_allowed_keys(
    obj: &Map<String, Value>,
    allowed: &[&str],
    context: &str,
    file: &Path,
) -> Result<(), LootError> {
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(invalid(
                file,
                format!("unexpected key `{key}` in {context}"),
            ));
        }
    }
    Ok(())
}

fn required_str<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    context: &str,
    file: &Path,
) -> Result<&'a str, LootError> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(file, format!("missing or non-string `{key}` in {context}")))
}

fn optional_str<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    context: &str,
    file: &Path,
) -> Result<Option<&'a str>, LootError> {
    match obj.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| invalid(file, format!("non-string `{key}` in {context}"))),
    }
}

fn parse_string_or_list(
    value: &Value,
    context: &str,
    file: &Path,
) -> Result<Vec<String>, LootError> {
    match value {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid(file, format!("non-string element in {context}")))
            })
            .collect(),
        _ => Err(invalid(
            file,
            format!("expected string or array in {context}"),
        )),
    }
}

fn parse_finite_f64(value: &Value, context: &str, file: &Path) -> Result<f64, LootError> {
    let number = value
        .as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| invalid(file, format!("expected finite number in {context}")))?;
    Ok(number)
}

fn parse_probability(value: &Value, context: &str, file: &Path) -> Result<f64, LootError> {
    let p = parse_finite_f64(value, context, file)?;
    if !(0.0..=1.0).contains(&p) {
        return Err(invalid(
            file,
            format!("probability {p} out of 0.0..=1.0 range in {context}"),
        ));
    }
    Ok(p)
}

fn parse_u32(value: &Value, context: &str, file: &Path) -> Result<u32, LootError> {
    let raw = value
        .as_u64()
        .ok_or_else(|| invalid(file, format!("expected non-negative integer in {context}")))?;
    u32::try_from(raw)
        .map_err(|_| invalid(file, format!("integer {raw} overflows u32 in {context}")))
}

fn parse_i32(value: &Value, context: &str, file: &Path) -> Result<i32, LootError> {
    let raw = value
        .as_i64()
        .ok_or_else(|| invalid(file, format!("expected integer in {context}")))?;
    i32::try_from(raw)
        .map_err(|_| invalid(file, format!("integer {raw} overflows i32 in {context}")))
}

fn parse_loot_table_inner(
    value: &Value,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
    expected_type: Option<&str>,
) -> Result<LootTable, LootError> {
    let obj = value
        .as_object()
        .ok_or_else(|| invalid(file, "loot table root must be a JSON object"))?;
    check_allowed_keys(
        obj,
        &["type", "pools", "random_sequence", "modifier"],
        "loot table",
        file,
    )?;

    let table_type = optional_str(obj, "type", "loot table", file)?.map(str::to_owned);
    if let Some(expected) = expected_type
        && let Some(ref kind) = table_type
        && normalize_item_id(kind) != expected
    {
        return Err(invalid(
            file,
            format!("expected {expected} loot table type, found `{kind}`"),
        ));
    }
    let random_sequence =
        optional_str(obj, "random_sequence", "loot table", file)?.map(str::to_owned);

    let pools = match obj.get("pools") {
        None => Vec::new(),
        Some(raw_pools) => {
            let arr = raw_pools
                .as_array()
                .ok_or_else(|| invalid(file, "`pools` must be a JSON array"))?;
            arr.iter()
                .map(|pool| parse_pool(pool, file, resolver))
                .collect::<Result<Vec<_>, _>>()?
        }
    };

    let modifiers = parse_optional_modifiers(obj.get("modifier"), file, resolver)?;

    Ok(LootTable::new(
        table_type,
        random_sequence,
        pools,
        modifiers,
    ))
}

fn parse_pool(
    value: &Value,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
) -> Result<LootPool, LootError> {
    let obj = value
        .as_object()
        .ok_or_else(|| invalid(file, "loot pool must be a JSON object"))?;
    check_allowed_keys(
        obj,
        &["rolls", "entries", "condition", "modifier"],
        "loot pool",
        file,
    )?;

    let rolls_val = obj
        .get("rolls")
        .ok_or_else(|| invalid(file, "loot pool missing `rolls`"))?;
    let rolls = parse_number_provider(rolls_val, file)?;

    let raw_entries = obj
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(file, "loot pool missing or non-array `entries`"))?;
    let entries = raw_entries
        .iter()
        .map(|entry| parse_entry(entry, file, resolver, 0))
        .collect::<Result<Vec<_>, _>>()?;

    let conditions = parse_optional_conditions(obj.get("condition"), file, resolver)?;
    let modifiers = parse_optional_modifiers(obj.get("modifier"), file, resolver)?;

    Ok(LootPool {
        rolls,
        entries,
        conditions,
        modifiers,
    })
}

fn parse_optional_weight(
    obj: &Map<String, Value>,
    context: &str,
    file: &Path,
) -> Result<u32, LootError> {
    match obj.get("weight") {
        None => Ok(1),
        Some(v) => parse_u32(v, &format!("{context}.weight"), file),
    }
}

fn parse_optional_quality(
    obj: &Map<String, Value>,
    context: &str,
    file: &Path,
) -> Result<i32, LootError> {
    match obj.get("quality") {
        None => Ok(0),
        Some(v) => parse_i32(v, &format!("{context}.quality"), file),
    }
}

fn parse_entry(
    value: &Value,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
    depth: usize,
) -> Result<LootEntry, LootError> {
    if depth > MAX_LOOT_TABLE_DEPTH {
        return Err(invalid(
            file,
            format!("loot entry nesting depth exceeds maximum ({MAX_LOOT_TABLE_DEPTH})"),
        ));
    }
    let obj = value
        .as_object()
        .ok_or_else(|| invalid(file, "loot entry must be a JSON object"))?;
    let entry_type = required_str(obj, "type", "loot entry", file)?;
    let conditions = parse_optional_conditions(obj.get("condition"), file, resolver)?;
    let modifiers = parse_optional_modifiers(obj.get("modifier"), file, resolver)?;

    match normalize_item_id(entry_type) {
        "item" => {
            check_allowed_keys(
                obj,
                &["type", "name", "weight", "quality", "condition", "modifier"],
                "item entry",
                file,
            )?;
            let name = required_str(obj, "name", "item entry", file)?.to_owned();
            let weight = parse_optional_weight(obj, "item entry", file)?;
            let quality = parse_optional_quality(obj, "item entry", file)?;
            Ok(LootEntry::Item {
                name,
                weight,
                quality,
                conditions,
                modifiers,
            })
        }
        "loot_table" => {
            check_allowed_keys(
                obj,
                &[
                    "type",
                    "value",
                    "weight",
                    "quality",
                    "condition",
                    "modifier",
                ],
                "loot_table entry",
                file,
            )?;
            let weight = parse_optional_weight(obj, "loot_table entry", file)?;
            let quality = parse_optional_quality(obj, "loot_table entry", file)?;
            let raw_value = obj
                .get("value")
                .ok_or_else(|| invalid(file, "loot_table entry missing `value`"))?;
            let source = match raw_value {
                Value::String(id) => {
                    let normalized = ensure_namespaced(id);
                    validate_resource_id(&normalized, "loot_table", file)?;
                    LootTableSource::Reference {
                        id: normalized,
                        table: None,
                    }
                }
                Value::Object(_) => LootTableSource::Inline(Box::new(parse_loot_table_inner(
                    raw_value, file, resolver, None,
                )?)),
                _ => {
                    return Err(invalid(
                        file,
                        "loot_table `value` must be a string id or inline loot table object",
                    ));
                }
            };
            Ok(LootEntry::LootTable {
                source,
                weight,
                quality,
                conditions,
                modifiers,
            })
        }
        "empty" => {
            check_allowed_keys(
                obj,
                &["type", "weight", "quality", "condition", "modifier"],
                "empty entry",
                file,
            )?;
            let weight = parse_optional_weight(obj, "empty entry", file)?;
            let quality = parse_optional_quality(obj, "empty entry", file)?;
            Ok(LootEntry::Empty {
                weight,
                quality,
                conditions,
                modifiers,
            })
        }
        "tag" => {
            check_allowed_keys(
                obj,
                &[
                    "type",
                    "items",
                    "name",
                    "expand",
                    "weight",
                    "quality",
                    "condition",
                    "modifier",
                ],
                "tag entry",
                file,
            )?;
            let raw_tag = match (obj.get("items"), obj.get("name")) {
                (Some(v), None) | (None, Some(v)) => v
                    .as_str()
                    .ok_or_else(|| invalid(file, "tag entry item tag must be a string"))?,
                (Some(_), Some(_)) => {
                    return Err(invalid(
                        file,
                        "tag entry cannot specify both `items` and `name`",
                    ));
                }
                (None, None) => {
                    return Err(invalid(file, "tag entry missing `items` or `name`"));
                }
            };
            let stripped = raw_tag.strip_prefix('#').unwrap_or(raw_tag);
            let tag = ensure_namespaced(stripped);
            validate_resource_id(&tag, "tag", file)?;
            let expand = obj
                .get("expand")
                .and_then(Value::as_bool)
                .ok_or_else(|| invalid(file, "tag entry missing or non-boolean `expand`"))?;
            let weight = parse_optional_weight(obj, "tag entry", file)?;
            let quality = parse_optional_quality(obj, "tag entry", file)?;
            Ok(LootEntry::Tag {
                tag,
                expand,
                weight,
                quality,
                conditions,
                modifiers,
            })
        }
        "alternatives" => {
            check_allowed_keys(
                obj,
                &["type", "children", "condition", "modifier"],
                "alternatives entry",
                file,
            )?;
            let raw_children = obj
                .get("children")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    invalid(file, "alternatives entry missing or non-array `children`")
                })?;
            let children = raw_children
                .iter()
                .map(|child| parse_entry(child, file, resolver, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LootEntry::Alternatives {
                children,
                conditions,
                modifiers,
            })
        }
        "dynamic" => {
            check_allowed_keys(
                obj,
                &["type", "name", "weight", "quality", "condition", "modifier"],
                "dynamic entry",
                file,
            )?;
            let name = required_str(obj, "name", "dynamic entry", file)?.to_owned();
            let weight = parse_optional_weight(obj, "dynamic entry", file)?;
            let quality = parse_optional_quality(obj, "dynamic entry", file)?;
            Ok(LootEntry::Dynamic {
                name,
                weight,
                quality,
                conditions,
                modifiers,
            })
        }
        _ => Err(invalid(
            file,
            format!("unknown loot entry type `{entry_type}`"),
        )),
    }
}

fn parse_number_provider(value: &Value, file: &Path) -> Result<NumberProvider, LootError> {
    if value.is_number() {
        return Ok(NumberProvider::Constant(parse_finite_f64(
            value,
            "constant number provider",
            file,
        )?));
    }

    let obj = value
        .as_object()
        .ok_or_else(|| invalid(file, "number provider must be a number or JSON object"))?;
    let kind = required_str(obj, "type", "number provider", file)?;

    match normalize_item_id(kind) {
        "constant" => {
            check_allowed_keys(obj, &["type", "value"], "constant number provider", file)?;
            let val = obj
                .get("value")
                .ok_or_else(|| invalid(file, "constant number provider missing `value`"))?;
            Ok(NumberProvider::Constant(parse_finite_f64(
                val,
                "constant.value",
                file,
            )?))
        }
        "uniform" => {
            check_allowed_keys(
                obj,
                &["type", "min", "max"],
                "uniform number provider",
                file,
            )?;
            let min = obj
                .get("min")
                .ok_or_else(|| invalid(file, "uniform number provider missing `min`"))?;
            let max = obj
                .get("max")
                .ok_or_else(|| invalid(file, "uniform number provider missing `max`"))?;
            Ok(NumberProvider::Uniform {
                min: parse_finite_f64(min, "uniform.min", file)?,
                max: parse_finite_f64(max, "uniform.max", file)?,
            })
        }
        "binomial" => {
            check_allowed_keys(obj, &["type", "n", "p"], "binomial number provider", file)?;
            let n = obj
                .get("n")
                .ok_or_else(|| invalid(file, "binomial number provider missing `n`"))?;
            let p = obj
                .get("p")
                .ok_or_else(|| invalid(file, "binomial number provider missing `p`"))?;
            Ok(NumberProvider::Binomial {
                n: parse_u32(n, "binomial.n", file)?,
                p: parse_probability(p, "binomial.p", file)?,
            })
        }
        _ => Err(invalid(
            file,
            format!("unknown number provider type `{kind}`"),
        )),
    }
}

fn parse_optional_conditions(
    value: Option<&Value>,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
) -> Result<Vec<LootCondition>, LootError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut stack = HashSet::new();
    parse_condition_list_or_single(value, file, resolver, &mut stack)
}

fn parse_condition_list_or_single(
    value: &Value,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
    stack: &mut HashSet<String>,
) -> Result<Vec<LootCondition>, LootError> {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| parse_single_condition(item, file, resolver, stack))
            .collect(),
        _ => Ok(vec![parse_single_condition(value, file, resolver, stack)?]),
    }
}

fn parse_single_condition(
    value: &Value,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
    stack: &mut HashSet<String>,
) -> Result<LootCondition, LootError> {
    if let Some(pred_id) = value.as_str() {
        return resolver.resolve(pred_id, file, stack);
    }

    let obj = value
        .as_object()
        .ok_or_else(|| invalid(file, "condition must be a string or JSON object"))?;
    let kind = required_str(obj, "type", "loot condition", file)?;

    match normalize_item_id(kind) {
        "survives_explosion" => {
            check_allowed_keys(obj, &["type"], "survives_explosion condition", file)?;
            Ok(LootCondition::SurvivesExplosion)
        }
        "match_block" => parse_match_block(obj, file),
        "table_bonus" => parse_table_bonus(obj, file),
        "all_of" | "any_of" => parse_composite_condition(obj, kind, file, resolver, stack),
        "inverted" => {
            check_allowed_keys(obj, &["type", "term"], "inverted condition", file)?;
            let raw_term = obj
                .get("term")
                .ok_or_else(|| invalid(file, "inverted condition missing `term`"))?;
            let term = parse_single_condition(raw_term, file, resolver, stack)?;
            Ok(LootCondition::Inverted {
                term: Box::new(term),
            })
        }
        "random_chance" => {
            check_allowed_keys(obj, &["type", "chance"], "random_chance condition", file)?;
            let raw_chance = obj
                .get("chance")
                .ok_or_else(|| invalid(file, "random_chance condition missing `chance`"))?;
            let chance = parse_probability(raw_chance, "random_chance.chance", file)?;
            Ok(LootCondition::RandomChance { chance })
        }
        "random_chance_with_enchanted_bonus" => parse_random_chance_with_enchanted_bonus(obj, file),
        "killed_by_player" => {
            check_allowed_keys(obj, &["type"], "killed_by_player condition", file)?;
            Ok(LootCondition::KilledByPlayer)
        }
        "match_tool" => parse_match_tool(obj, file),
        "location_check" => parse_location_check(obj, kind, file),
        "entity_properties" => parse_entity_properties(obj, kind, file),
        "damage_source_properties" => parse_damage_source_properties(obj, kind, file),
        _ => Ok(LootCondition::Unsupported {
            what: kind.to_owned(),
        }),
    }
}

fn parse_block_match_fields(
    obj: &Map<String, Value>,
    context: &str,
    file: &Path,
) -> Result<(Vec<String>, BTreeMap<String, String>), LootError> {
    let blocks = match obj.get("blocks") {
        None => Vec::new(),
        Some(raw) => parse_string_or_list(raw, &format!("{context}.blocks"), file)?,
    };
    let mut state = BTreeMap::new();
    if let Some(raw_state) = obj.get("state") {
        let state_obj = raw_state
            .as_object()
            .ok_or_else(|| invalid(file, format!("{context} `state` must be a JSON object")))?;
        for (prop, val) in state_obj {
            let val_str = val.as_str().ok_or_else(|| {
                invalid(
                    file,
                    format!("{context} state `{prop}` value must be a string"),
                )
            })?;
            state.insert(prop.clone(), val_str.to_owned());
        }
    }
    Ok((blocks, state))
}

fn parse_match_block(obj: &Map<String, Value>, file: &Path) -> Result<LootCondition, LootError> {
    check_allowed_keys(
        obj,
        &["type", "blocks", "state"],
        "match_block condition",
        file,
    )?;
    let (blocks, state) = parse_block_match_fields(obj, "match_block", file)?;
    Ok(LootCondition::MatchBlock { blocks, state })
}

fn parse_optional_offset(
    obj: &Map<String, Value>,
    key: &str,
    file: &Path,
) -> Result<i32, LootError> {
    match obj.get(key) {
        None => Ok(0),
        Some(v) => parse_i32(v, &format!("location_check.{key}"), file),
    }
}

fn parse_location_check(
    obj: &Map<String, Value>,
    kind: &str,
    file: &Path,
) -> Result<LootCondition, LootError> {
    check_allowed_keys(
        obj,
        &["type", "offsetX", "offsetY", "offsetZ", "predicate"],
        "location_check condition",
        file,
    )?;
    let offset_x = parse_optional_offset(obj, "offsetX", file)?;
    let offset_y = parse_optional_offset(obj, "offsetY", file)?;
    let offset_z = parse_optional_offset(obj, "offsetZ", file)?;
    let pred_obj = obj
        .get("predicate")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(file, "location_check missing or non-object `predicate`"))?;
    if pred_obj.keys().any(|k| k != "block") {
        return Ok(LootCondition::Unsupported {
            what: kind.to_owned(),
        });
    }
    let (blocks, state) = match pred_obj.get("block") {
        None => (Vec::new(), BTreeMap::new()),
        Some(raw_block) => {
            let block_obj = raw_block.as_object().ok_or_else(|| {
                invalid(
                    file,
                    "location_check `predicate.block` must be a JSON object",
                )
            })?;
            if block_obj.keys().any(|k| k != "blocks" && k != "state") {
                return Ok(LootCondition::Unsupported {
                    what: kind.to_owned(),
                });
            }
            parse_block_match_fields(block_obj, "location_check.predicate.block", file)?
        }
    };
    Ok(LootCondition::LocationCheck {
        offset_x,
        offset_y,
        offset_z,
        blocks,
        state,
    })
}

fn parse_table_bonus(obj: &Map<String, Value>, file: &Path) -> Result<LootCondition, LootError> {
    check_allowed_keys(
        obj,
        &["type", "enchantment", "chances"],
        "table_bonus condition",
        file,
    )?;
    let enchantment = required_str(obj, "enchantment", "table_bonus condition", file)?.to_owned();
    let raw_chances = obj
        .get("chances")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(file, "table_bonus missing or non-array `chances`"))?;
    if raw_chances.is_empty() {
        return Err(invalid(file, "table_bonus `chances` must not be empty"));
    }
    let chances = raw_chances
        .iter()
        .map(|item| parse_probability(item, "table_bonus.chances", file))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LootCondition::TableBonus {
        enchantment,
        chances,
    })
}

fn parse_composite_condition(
    obj: &Map<String, Value>,
    kind: &str,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
    stack: &mut HashSet<String>,
) -> Result<LootCondition, LootError> {
    check_allowed_keys(obj, &["type", "terms"], "composite condition", file)?;
    let raw_terms = obj
        .get("terms")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(file, format!("`{kind}` missing or non-array `terms`")))?;
    let terms = raw_terms
        .iter()
        .map(|term| parse_single_condition(term, file, resolver, stack))
        .collect::<Result<Vec<_>, _>>()?;
    if normalize_item_id(kind) == "all_of" {
        Ok(LootCondition::AllOf { terms })
    } else {
        Ok(LootCondition::AnyOf { terms })
    }
}

fn parse_tool_predicate_fields(
    pred_obj: &Map<String, Value>,
    context: &str,
    reject_unknown_keys: bool,
    file: &Path,
) -> Result<Option<ToolPredicate>, LootError> {
    for key in pred_obj.keys() {
        if key != "items" && key != "predicates" {
            if reject_unknown_keys {
                return Err(invalid(
                    file,
                    format!("unexpected key `{key}` in {context}"),
                ));
            }
            return Ok(None);
        }
    }

    let items = match pred_obj.get("items") {
        None => Vec::new(),
        Some(raw_items) => parse_string_or_list(raw_items, &format!("{context}.items"), file)?,
    };

    let mut enchantments = Vec::new();
    if let Some(raw_preds) = pred_obj.get("predicates") {
        let preds_obj = raw_preds
            .as_object()
            .ok_or_else(|| invalid(file, format!("{context}.predicates must be a JSON object")))?;
        for key in preds_obj.keys() {
            if key != "minecraft:enchantments" {
                if reject_unknown_keys {
                    return Err(invalid(
                        file,
                        format!("unexpected key `{key}` in {context}.predicates"),
                    ));
                }
                return Ok(None);
            }
        }
        if let Some(raw_enchs) = preds_obj.get("minecraft:enchantments") {
            enchantments = parse_enchantment_predicate_list(
                raw_enchs,
                &format!("{context}.predicates.minecraft:enchantments"),
                file,
            )?;
        }
    }

    Ok(Some(ToolPredicate {
        items,
        enchantments,
    }))
}

fn parse_enchantment_predicate_list(
    raw: &Value,
    context: &str,
    file: &Path,
) -> Result<Vec<EnchantmentPredicate>, LootError> {
    let ench_arr = raw
        .as_array()
        .ok_or_else(|| invalid(file, format!("{context} must be an array")))?;
    ench_arr
        .iter()
        .map(|entry| parse_enchantment_predicate(entry, file))
        .collect()
}

fn parse_match_tool(obj: &Map<String, Value>, file: &Path) -> Result<LootCondition, LootError> {
    check_allowed_keys(obj, &["type", "predicate"], "match_tool condition", file)?;
    let pred_obj = obj
        .get("predicate")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(file, "match_tool missing or non-object `predicate`"))?;
    let predicate = parse_tool_predicate_fields(pred_obj, "match_tool.predicate", true, file)?
        .ok_or_else(|| invalid(file, "unexpected key in match_tool.predicate"))?;
    Ok(LootCondition::MatchTool { predicate })
}

fn parse_enchantment_predicate(
    value: &Value,
    file: &Path,
) -> Result<EnchantmentPredicate, LootError> {
    let obj = value
        .as_object()
        .ok_or_else(|| invalid(file, "enchantment predicate must be a JSON object"))?;
    check_allowed_keys(
        obj,
        &["enchantments", "levels"],
        "enchantment predicate",
        file,
    )?;

    let enchantments = match obj.get("enchantments") {
        None => Vec::new(),
        Some(raw) => parse_string_or_list(raw, "enchantment predicate `enchantments`", file)?,
    };

    let (min_level, max_level) = match obj.get("levels") {
        None => (None, None),
        Some(raw_levels) if raw_levels.is_u64() => {
            let exact = parse_u32(raw_levels, "enchantment predicate `levels`", file)?;
            (Some(exact), Some(exact))
        }
        Some(raw_levels) => {
            let lvl_obj = raw_levels.as_object().ok_or_else(|| {
                invalid(
                    file,
                    "enchantment predicate `levels` must be an int or object",
                )
            })?;
            check_allowed_keys(
                lvl_obj,
                &["min", "max"],
                "enchantment predicate `levels`",
                file,
            )?;
            let min = match lvl_obj.get("min") {
                None => None,
                Some(v) => Some(parse_u32(v, "levels.min", file)?),
            };
            let max = match lvl_obj.get("max") {
                None => None,
                Some(v) => Some(parse_u32(v, "levels.max", file)?),
            };
            (min, max)
        }
    };

    Ok(EnchantmentPredicate {
        enchantments,
        min_level,
        max_level,
    })
}

fn parse_int_range(value: &Value, context: &str, file: &Path) -> Result<IntRange, LootError> {
    if value.is_i64() {
        let exact = parse_i32(value, context, file)?;
        return Ok(IntRange {
            min: Some(exact),
            max: Some(exact),
        });
    }
    let obj = value
        .as_object()
        .ok_or_else(|| invalid(file, format!("{context} must be an integer or object")))?;
    check_allowed_keys(obj, &["min", "max"], context, file)?;
    let min = match obj.get("min") {
        None => None,
        Some(v) => Some(parse_i32(v, &format!("{context}.min"), file)?),
    };
    let max = match obj.get("max") {
        None => None,
        Some(v) => Some(parse_i32(v, &format!("{context}.max"), file)?),
    };
    Ok(IntRange { min, max })
}

fn parse_bool_field(
    obj: &Map<String, Value>,
    key: &str,
    context: &str,
    file: &Path,
) -> Result<Option<bool>, LootError> {
    match obj.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| invalid(file, format!("{context}.{key} must be a boolean"))),
    }
}

fn parse_entity_predicate(
    pred_obj: &Map<String, Value>,
    file: &Path,
    depth: usize,
) -> Result<Option<EntityPredicate>, LootError> {
    if depth > MAX_LOOT_TABLE_DEPTH {
        return Err(invalid(
            file,
            format!("entity predicate nesting depth exceeds maximum ({MAX_LOOT_TABLE_DEPTH})"),
        ));
    }
    let mut out = EntityPredicate::default();
    for (key, value) in pred_obj {
        match normalize_item_id(key) {
            "flags" => {
                let flags_obj = value
                    .as_object()
                    .ok_or_else(|| invalid(file, "entity predicate `flags` must be an object"))?;
                if flags_obj
                    .keys()
                    .any(|k| k != "is_on_fire" && k != "is_baby")
                {
                    return Ok(None);
                }
                out.flags = Some(EntityFlagsPredicate {
                    is_on_fire: parse_bool_field(flags_obj, "is_on_fire", "flags", file)?,
                    is_baby: parse_bool_field(flags_obj, "is_baby", "flags", file)?,
                });
            }
            "entity_type" => {
                out.entity_types = parse_string_or_list(value, "entity_type", file)?;
            }
            "vehicle" => {
                let veh_obj = value.as_object().ok_or_else(|| {
                    invalid(file, "entity predicate `vehicle` must be a JSON object")
                })?;
                let Some(veh_pred) = parse_entity_predicate(veh_obj, file, depth + 1)? else {
                    return Ok(None);
                };
                out.vehicle = Some(Box::new(veh_pred));
            }
            "equipment" => {
                let eq_obj = value.as_object().ok_or_else(|| {
                    invalid(file, "entity predicate `equipment` must be a JSON object")
                })?;
                if eq_obj.keys().any(|k| k != "mainhand") {
                    return Ok(None);
                }
                let mainhand = match eq_obj.get("mainhand") {
                    None => None,
                    Some(raw_hand) => {
                        let hand_obj = raw_hand.as_object().ok_or_else(|| {
                            invalid(file, "equipment `mainhand` must be a JSON object")
                        })?;
                        let Some(tool_pred) = parse_tool_predicate_fields(
                            hand_obj,
                            "equipment.mainhand",
                            false,
                            file,
                        )?
                        else {
                            return Ok(None);
                        };
                        Some(tool_pred)
                    }
                };
                out.equipment = Some(EntityEquipmentPredicate { mainhand });
            }
            "components" => {
                let comp_obj = value.as_object().ok_or_else(|| {
                    invalid(file, "entity predicate `components` must be a JSON object")
                })?;
                for (comp_key, comp_val) in comp_obj {
                    let Some(val_str) = comp_val.as_str() else {
                        return Ok(None);
                    };
                    out.components
                        .insert(ensure_namespaced(comp_key), val_str.to_owned());
                }
            }
            "type_specific/sheep" => {
                let sheep_obj = value
                    .as_object()
                    .ok_or_else(|| invalid(file, "type_specific/sheep must be a JSON object"))?;
                if sheep_obj.keys().any(|k| k != "sheared") {
                    return Ok(None);
                }
                out.sheep_sheared =
                    parse_bool_field(sheep_obj, "sheared", "type_specific/sheep", file)?;
            }
            "type_specific/cube_mob" => {
                let cube_obj = value
                    .as_object()
                    .ok_or_else(|| invalid(file, "type_specific/cube_mob must be a JSON object"))?;
                if cube_obj.keys().any(|k| k != "size") {
                    return Ok(None);
                }
                if let Some(size_val) = cube_obj.get("size") {
                    out.cube_mob_size = Some(parse_int_range(
                        size_val,
                        "type_specific/cube_mob.size",
                        file,
                    )?);
                }
            }
            "type_specific/raider" => {
                let raider_obj = value
                    .as_object()
                    .ok_or_else(|| invalid(file, "type_specific/raider must be a JSON object"))?;
                if raider_obj.keys().any(|k| k != "is_captain") {
                    return Ok(None);
                }
                out.raider_is_captain =
                    parse_bool_field(raider_obj, "is_captain", "type_specific/raider", file)?;
            }
            "type_specific/fishing_hook" => {
                let hook_obj = value.as_object().ok_or_else(|| {
                    invalid(file, "type_specific/fishing_hook must be a JSON object")
                })?;
                if hook_obj.keys().any(|k| k != "in_open_water") {
                    return Ok(None);
                }
                out.fishing_hook = Some(FishingHookPredicate {
                    in_open_water: parse_bool_field(
                        hook_obj,
                        "in_open_water",
                        "type_specific/fishing_hook",
                        file,
                    )?,
                });
            }
            _ => return Ok(None),
        }
    }
    Ok(Some(out))
}

fn parse_entity_properties(
    obj: &Map<String, Value>,
    kind: &str,
    file: &Path,
) -> Result<LootCondition, LootError> {
    check_allowed_keys(
        obj,
        &["type", "entity", "predicate"],
        "entity_properties condition",
        file,
    )?;
    let entity = required_str(obj, "entity", "entity_properties condition", file)?;
    if !matches!(
        entity,
        "this"
            | "attacker"
            | "direct_attacker"
            | "attacking_player"
            | "target_entity"
            | "interacting_entity"
    ) {
        return Err(invalid(
            file,
            format!("unknown entity target `{entity}` in entity_properties"),
        ));
    }
    let pred_obj = obj
        .get("predicate")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(file, "entity_properties missing or non-object `predicate`"))?;
    let Some(predicate) = parse_entity_predicate(pred_obj, file, 0)? else {
        return Ok(LootCondition::Unsupported {
            what: kind.to_owned(),
        });
    };
    Ok(LootCondition::EntityProperties {
        entity: entity.to_owned(),
        predicate: Box::new(predicate),
    })
}

fn parse_damage_source_properties(
    obj: &Map<String, Value>,
    kind: &str,
    file: &Path,
) -> Result<LootCondition, LootError> {
    check_allowed_keys(
        obj,
        &["type", "predicate"],
        "damage_source_properties condition",
        file,
    )?;
    let pred_obj = obj
        .get("predicate")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            invalid(
                file,
                "damage_source_properties missing or non-object `predicate`",
            )
        })?;
    let mut out = DamageSourcePredicate::default();
    for (key, value) in pred_obj {
        match key.as_str() {
            "direct_entity" => {
                let ent_obj = value.as_object().ok_or_else(|| {
                    invalid(
                        file,
                        "damage_source_properties `direct_entity` must be an object",
                    )
                })?;
                let Some(ent_pred) = parse_entity_predicate(ent_obj, file, 0)? else {
                    return Ok(LootCondition::Unsupported {
                        what: kind.to_owned(),
                    });
                };
                out.direct_entity = Some(ent_pred);
            }
            "source_entity" => {
                let ent_obj = value.as_object().ok_or_else(|| {
                    invalid(
                        file,
                        "damage_source_properties `source_entity` must be an object",
                    )
                })?;
                let Some(ent_pred) = parse_entity_predicate(ent_obj, file, 0)? else {
                    return Ok(LootCondition::Unsupported {
                        what: kind.to_owned(),
                    });
                };
                out.source_entity = Some(ent_pred);
            }
            "tags" => {
                let arr = value.as_array().ok_or_else(|| {
                    invalid(file, "damage_source_properties `tags` must be an array")
                })?;
                for item in arr {
                    let tag_obj = item.as_object().ok_or_else(|| {
                        invalid(
                            file,
                            "damage_source_properties tag entry must be a JSON object",
                        )
                    })?;
                    check_allowed_keys(
                        tag_obj,
                        &["id", "expected"],
                        "damage_source_properties tag",
                        file,
                    )?;
                    let id = required_str(tag_obj, "id", "damage_source_properties tag", file)?
                        .to_owned();
                    let expected = tag_obj
                        .get("expected")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| {
                            invalid(
                                file,
                                "damage_source_properties tag missing or non-boolean `expected`",
                            )
                        })?;
                    out.tags.push(DamageTypeTagPredicate { id, expected });
                }
            }
            _ => {
                return Ok(LootCondition::Unsupported {
                    what: kind.to_owned(),
                });
            }
        }
    }
    Ok(LootCondition::DamageSourceProperties {
        predicate: Box::new(out),
    })
}

fn parse_level_based_value(
    value: &Value,
    file: &Path,
    depth: usize,
) -> Result<LevelBasedValue, LootError> {
    if depth > MAX_LOOT_TABLE_DEPTH {
        return Err(invalid(
            file,
            format!("level-based value nesting depth exceeds maximum ({MAX_LOOT_TABLE_DEPTH})"),
        ));
    }
    if value.is_number() {
        return Ok(LevelBasedValue::Constant(parse_finite_f64(
            value,
            "constant level-based value",
            file,
        )?));
    }
    let obj = value
        .as_object()
        .ok_or_else(|| invalid(file, "level-based value must be a number or JSON object"))?;
    let kind = required_str(obj, "type", "level-based value", file)?;
    match normalize_item_id(kind) {
        "constant" => {
            check_allowed_keys(obj, &["type", "value"], "constant level-based value", file)?;
            let raw = obj
                .get("value")
                .ok_or_else(|| invalid(file, "constant level-based value missing `value`"))?;
            Ok(LevelBasedValue::Constant(parse_finite_f64(
                raw,
                "constant.value",
                file,
            )?))
        }
        "linear" => {
            check_allowed_keys(
                obj,
                &["type", "base", "per_level_above_first"],
                "linear level-based value",
                file,
            )?;
            let raw_base = obj
                .get("base")
                .ok_or_else(|| invalid(file, "linear level-based value missing `base`"))?;
            let raw_step = obj.get("per_level_above_first").ok_or_else(|| {
                invalid(
                    file,
                    "linear level-based value missing `per_level_above_first`",
                )
            })?;
            Ok(LevelBasedValue::Linear {
                base: parse_finite_f64(raw_base, "linear.base", file)?,
                per_level_above_first: parse_finite_f64(
                    raw_step,
                    "linear.per_level_above_first",
                    file,
                )?,
            })
        }
        "levels_squared" => {
            check_allowed_keys(
                obj,
                &["type", "added"],
                "levels_squared level-based value",
                file,
            )?;
            let raw_added = obj
                .get("added")
                .ok_or_else(|| invalid(file, "levels_squared level-based value missing `added`"))?;
            Ok(LevelBasedValue::LevelsSquared {
                added: parse_finite_f64(raw_added, "levels_squared.added", file)?,
            })
        }
        "clamped" => {
            check_allowed_keys(
                obj,
                &["type", "value", "min", "max"],
                "clamped level-based value",
                file,
            )?;
            let raw_val = obj
                .get("value")
                .ok_or_else(|| invalid(file, "clamped level-based value missing `value`"))?;
            let raw_min = obj
                .get("min")
                .ok_or_else(|| invalid(file, "clamped level-based value missing `min`"))?;
            let raw_max = obj
                .get("max")
                .ok_or_else(|| invalid(file, "clamped level-based value missing `max`"))?;
            Ok(LevelBasedValue::Clamped {
                value: Box::new(parse_level_based_value(raw_val, file, depth + 1)?),
                min: parse_finite_f64(raw_min, "clamped.min", file)?,
                max: parse_finite_f64(raw_max, "clamped.max", file)?,
            })
        }
        "fraction" => {
            check_allowed_keys(
                obj,
                &["type", "numerator", "denominator"],
                "fraction level-based value",
                file,
            )?;
            let raw_num = obj
                .get("numerator")
                .ok_or_else(|| invalid(file, "fraction level-based value missing `numerator`"))?;
            let raw_den = obj
                .get("denominator")
                .ok_or_else(|| invalid(file, "fraction level-based value missing `denominator`"))?;
            Ok(LevelBasedValue::Fraction {
                numerator: Box::new(parse_level_based_value(raw_num, file, depth + 1)?),
                denominator: Box::new(parse_level_based_value(raw_den, file, depth + 1)?),
            })
        }
        "exponent" => {
            check_allowed_keys(
                obj,
                &["type", "base", "power"],
                "exponent level-based value",
                file,
            )?;
            let raw_base = obj
                .get("base")
                .ok_or_else(|| invalid(file, "exponent level-based value missing `base`"))?;
            let raw_power = obj
                .get("power")
                .ok_or_else(|| invalid(file, "exponent level-based value missing `power`"))?;
            Ok(LevelBasedValue::Exponent {
                base: Box::new(parse_level_based_value(raw_base, file, depth + 1)?),
                power: Box::new(parse_level_based_value(raw_power, file, depth + 1)?),
            })
        }
        "lookup" => {
            check_allowed_keys(
                obj,
                &["type", "values", "fallback"],
                "lookup level-based value",
                file,
            )?;
            let raw_vals = obj.get("values").and_then(Value::as_array).ok_or_else(|| {
                invalid(
                    file,
                    "lookup level-based value missing or non-array `values`",
                )
            })?;
            let values = raw_vals
                .iter()
                .map(|v| parse_finite_f64(v, "lookup.values", file))
                .collect::<Result<Vec<_>, _>>()?;
            let raw_fallback = obj
                .get("fallback")
                .ok_or_else(|| invalid(file, "lookup level-based value missing `fallback`"))?;
            Ok(LevelBasedValue::Lookup {
                values,
                fallback: Box::new(parse_level_based_value(raw_fallback, file, depth + 1)?),
            })
        }
        _ => Err(invalid(
            file,
            format!("unknown level-based value type `{kind}`"),
        )),
    }
}

fn parse_random_chance_with_enchanted_bonus(
    obj: &Map<String, Value>,
    file: &Path,
) -> Result<LootCondition, LootError> {
    check_allowed_keys(
        obj,
        &[
            "type",
            "enchantment",
            "unenchanted_chance",
            "enchanted_chance",
        ],
        "random_chance_with_enchanted_bonus condition",
        file,
    )?;
    let enchantment = required_str(
        obj,
        "enchantment",
        "random_chance_with_enchanted_bonus condition",
        file,
    )?
    .to_owned();
    let raw_unenchanted = obj.get("unenchanted_chance").ok_or_else(|| {
        invalid(
            file,
            "random_chance_with_enchanted_bonus missing `unenchanted_chance`",
        )
    })?;
    let unenchanted_chance = parse_probability(
        raw_unenchanted,
        "random_chance_with_enchanted_bonus.unenchanted_chance",
        file,
    )?;
    let raw_enchanted = obj.get("enchanted_chance").ok_or_else(|| {
        invalid(
            file,
            "random_chance_with_enchanted_bonus missing `enchanted_chance`",
        )
    })?;
    let enchanted_chance = parse_level_based_value(raw_enchanted, file, 0)?;
    Ok(LootCondition::RandomChanceWithEnchantedBonus {
        enchantment,
        unenchanted_chance,
        enchanted_chance,
    })
}

fn parse_optional_modifiers(
    value: Option<&Value>,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
) -> Result<Vec<LootModifier>, LootError> {
    parse_optional_modifiers_depth(value, file, resolver, 0)
}

fn parse_optional_modifiers_depth(
    value: Option<&Value>,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
    depth: usize,
) -> Result<Vec<LootModifier>, LootError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| parse_single_modifier(item, file, resolver, depth))
            .collect(),
        Value::Object(_) => Ok(vec![parse_single_modifier(value, file, resolver, depth)?]),
        _ => Err(invalid(
            file,
            "modifier must be a JSON object or array of objects",
        )),
    }
}

fn parse_optional_bool(
    obj: &Map<String, Value>,
    key: &str,
    default: bool,
    context: &str,
    file: &Path,
) -> Result<bool, LootError> {
    match obj.get(key) {
        None => Ok(default),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| invalid(file, format!("{context} `{key}` must be a boolean"))),
    }
}

fn parse_single_modifier(
    value: &Value,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
    depth: usize,
) -> Result<LootModifier, LootError> {
    if depth > MAX_LOOT_TABLE_DEPTH {
        return Err(invalid(
            file,
            format!("loot modifier nesting depth exceeds maximum ({MAX_LOOT_TABLE_DEPTH})"),
        ));
    }
    let obj = value
        .as_object()
        .ok_or_else(|| invalid(file, "modifier must be a JSON object"))?;
    let kind = required_str(obj, "type", "loot modifier", file)?;
    let conditions = parse_optional_conditions(obj.get("condition"), file, resolver)?;

    match normalize_item_id(kind) {
        "set_count" => {
            check_allowed_keys(
                obj,
                &["type", "count", "add", "condition"],
                "set_count modifier",
                file,
            )?;
            let raw_count = obj
                .get("count")
                .ok_or_else(|| invalid(file, "set_count modifier missing `count`"))?;
            let count = parse_number_provider(raw_count, file)?;
            let add = parse_optional_bool(obj, "add", false, "set_count", file)?;
            Ok(LootModifier::SetCount {
                count,
                add,
                conditions,
            })
        }
        "set_damage" => parse_set_damage(obj, conditions, file),
        "set_name" => parse_set_name(obj, kind, conditions, file),
        "set_enchantments" => parse_set_enchantments(obj, conditions, file),
        "enchant_randomly" => parse_enchant_randomly(obj, kind, conditions, file),
        "enchant_with_levels" => parse_enchant_with_levels(obj, kind, conditions, file),
        "filtered" => parse_filtered(obj, kind, conditions, file, resolver, depth),
        "discard" => {
            check_allowed_keys(obj, &["type", "condition"], "discard modifier", file)?;
            Ok(LootModifier::Discard { conditions })
        }
        "enchanted_count_increase" => {
            check_allowed_keys(
                obj,
                &["type", "enchantment", "count", "limit", "condition"],
                "enchanted_count_increase modifier",
                file,
            )?;
            let enchantment = required_str(
                obj,
                "enchantment",
                "enchanted_count_increase modifier",
                file,
            )?
            .to_owned();
            let raw_count = obj.get("count").ok_or_else(|| {
                invalid(file, "enchanted_count_increase modifier missing `count`")
            })?;
            let count = parse_number_provider(raw_count, file)?;
            let limit = match obj.get("limit") {
                None => 0,
                Some(v) => parse_u32(v, "enchanted_count_increase.limit", file)?,
            };
            Ok(LootModifier::EnchantedCountIncrease {
                enchantment,
                count,
                limit,
                conditions,
            })
        }
        "furnace_smelt" => {
            check_allowed_keys(obj, &["type", "condition"], "furnace_smelt modifier", file)?;
            Ok(LootModifier::FurnaceSmelt { conditions })
        }
        "set_potion" => {
            check_allowed_keys(
                obj,
                &["type", "id", "condition"],
                "set_potion modifier",
                file,
            )?;
            let id = required_str(obj, "id", "set_potion modifier", file)?.to_owned();
            Ok(LootModifier::SetPotion { id, conditions })
        }
        "explosion_decay" => {
            check_allowed_keys(
                obj,
                &["type", "condition"],
                "explosion_decay modifier",
                file,
            )?;
            Ok(LootModifier::ExplosionDecay { conditions })
        }
        "apply_bonus" => parse_apply_bonus(obj, conditions, file),
        "limit_count" => parse_limit_count(obj, conditions, file),
        "copy_components" => parse_copy_components(obj, conditions, file),
        "copy_state" => parse_copy_state(obj, conditions, file),
        _ => Ok(LootModifier::Unsupported {
            what: kind.to_owned(),
        }),
    }
}

fn parse_set_damage(
    obj: &Map<String, Value>,
    conditions: Vec<LootCondition>,
    file: &Path,
) -> Result<LootModifier, LootError> {
    check_allowed_keys(
        obj,
        &["type", "damage", "add", "condition"],
        "set_damage modifier",
        file,
    )?;
    let raw_damage = obj
        .get("damage")
        .ok_or_else(|| invalid(file, "set_damage modifier missing `damage`"))?;
    let damage = parse_number_provider(raw_damage, file)?;
    let add = parse_optional_bool(obj, "add", false, "set_damage", file)?;
    Ok(LootModifier::SetDamage {
        damage,
        add,
        conditions,
    })
}

fn extract_plain_text_name(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Object(obj) if obj.len() == 1 => {
            obj.get("text").and_then(Value::as_str).map(str::to_owned)
        }
        _ => None,
    }
}

fn parse_set_name(
    obj: &Map<String, Value>,
    kind: &str,
    conditions: Vec<LootCondition>,
    file: &Path,
) -> Result<LootModifier, LootError> {
    check_allowed_keys(
        obj,
        &["type", "name", "entity", "target", "condition"],
        "set_name modifier",
        file,
    )?;
    let raw_name = obj
        .get("name")
        .ok_or_else(|| invalid(file, "set_name modifier missing `name`"))?;
    if !matches!(
        raw_name,
        Value::String(_) | Value::Object(_) | Value::Array(_)
    ) {
        return Err(invalid(
            file,
            "set_name `name` must be a JSON text component",
        ));
    }
    let entity = optional_str(obj, "entity", "set_name modifier", file)?;
    if let Some(target_entity) = entity
        && !matches!(
            target_entity,
            "this"
                | "attacker"
                | "direct_attacker"
                | "attacking_player"
                | "target_entity"
                | "interacting_entity"
        )
    {
        return Err(invalid(
            file,
            format!("unknown entity target `{target_entity}` in set_name"),
        ));
    }
    let target = match optional_str(obj, "target", "set_name modifier", file)? {
        None | Some("custom_name") => SetNameTarget::CustomName,
        Some("item_name") => SetNameTarget::ItemName,
        Some(other) => {
            return Err(invalid(
                file,
                format!(
                    "unknown set_name `target` `{other}` (expected `custom_name` or `item_name`)"
                ),
            ));
        }
    };
    if target == SetNameTarget::CustomName
        && entity.is_none()
        && let Some(name) = extract_plain_text_name(raw_name)
    {
        return Ok(LootModifier::SetName {
            name,
            target,
            conditions,
        });
    }
    Ok(LootModifier::Unsupported {
        what: kind.to_owned(),
    })
}

fn parse_set_enchantments(
    obj: &Map<String, Value>,
    conditions: Vec<LootCondition>,
    file: &Path,
) -> Result<LootModifier, LootError> {
    check_allowed_keys(
        obj,
        &["type", "enchantments", "add", "condition"],
        "set_enchantments modifier",
        file,
    )?;
    let raw_enchs = obj
        .get("enchantments")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            invalid(
                file,
                "set_enchantments missing or non-object `enchantments`",
            )
        })?;
    let mut enchantments = BTreeMap::new();
    for (ench_id, level_val) in raw_enchs {
        let normalized = ensure_namespaced(ench_id);
        validate_resource_id(&normalized, "enchantment", file)?;
        let provider = parse_number_provider(level_val, file)?;
        enchantments.insert(normalized, provider);
    }
    let add = parse_optional_bool(obj, "add", false, "set_enchantments", file)?;
    Ok(LootModifier::SetEnchantments {
        enchantments,
        add,
        conditions,
    })
}

fn normalize_enchant_option_spec(raw: &str, file: &Path) -> Result<String, LootError> {
    if let Some(tag) = raw.strip_prefix('#') {
        let normalized = ensure_namespaced(tag);
        validate_resource_id(&normalized, "enchantment tag", file)?;
        Ok(format!("#{normalized}"))
    } else {
        let normalized = ensure_namespaced(raw);
        validate_resource_id(&normalized, "enchantment", file)?;
        Ok(normalized)
    }
}

fn parse_enchant_options(
    obj: &Map<String, Value>,
    context: &str,
    file: &Path,
) -> Result<Option<Vec<String>>, LootError> {
    let Some(raw) = obj.get("options") else {
        return Ok(None);
    };
    let entries = parse_string_or_list(raw, &format!("{context}.options"), file)?;
    let normalized = entries
        .into_iter()
        .map(|entry| normalize_enchant_option_spec(&entry, file))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(normalized))
}

fn parse_enchant_randomly(
    obj: &Map<String, Value>,
    kind: &str,
    conditions: Vec<LootCondition>,
    file: &Path,
) -> Result<LootModifier, LootError> {
    check_allowed_keys(
        obj,
        &[
            "type",
            "options",
            "only_compatible",
            "include_additional_cost_component",
            "condition",
        ],
        "enchant_randomly modifier",
        file,
    )?;
    let options = parse_enchant_options(obj, "enchant_randomly", file)?;
    let only_compatible =
        parse_optional_bool(obj, "only_compatible", true, "enchant_randomly", file)?;
    let include_additional_cost = parse_optional_bool(
        obj,
        "include_additional_cost_component",
        false,
        "enchant_randomly",
        file,
    )?;
    if include_additional_cost {
        return Ok(LootModifier::Unsupported {
            what: kind.to_owned(),
        });
    }
    Ok(LootModifier::EnchantRandomly {
        options,
        only_compatible,
        conditions,
    })
}

fn parse_enchant_with_levels(
    obj: &Map<String, Value>,
    kind: &str,
    conditions: Vec<LootCondition>,
    file: &Path,
) -> Result<LootModifier, LootError> {
    check_allowed_keys(
        obj,
        &[
            "type",
            "levels",
            "options",
            "include_additional_cost_component",
            "condition",
        ],
        "enchant_with_levels modifier",
        file,
    )?;
    let raw_levels = obj
        .get("levels")
        .ok_or_else(|| invalid(file, "enchant_with_levels modifier missing `levels`"))?;
    let levels = parse_number_provider(raw_levels, file)?;
    let options = parse_enchant_options(obj, "enchant_with_levels", file)?;
    let include_additional_cost = parse_optional_bool(
        obj,
        "include_additional_cost_component",
        false,
        "enchant_with_levels",
        file,
    )?;
    if include_additional_cost {
        return Ok(LootModifier::Unsupported {
            what: kind.to_owned(),
        });
    }
    Ok(LootModifier::EnchantWithLevels {
        levels,
        options,
        conditions,
    })
}

fn parse_damage_predicate(
    value: &Value,
    file: &Path,
) -> Result<Option<DamagePredicate>, LootError> {
    let obj = value.as_object().ok_or_else(|| {
        invalid(
            file,
            "item_filter.predicates.minecraft:damage must be a JSON object",
        )
    })?;
    let mut out = DamagePredicate::default();
    for (key, val) in obj {
        match key.as_str() {
            "damage" => {
                out.damage = Some(parse_int_range(
                    val,
                    "item_filter.predicates.minecraft:damage.damage",
                    file,
                )?);
            }
            "durability" => {
                out.durability = Some(parse_int_range(
                    val,
                    "item_filter.predicates.minecraft:damage.durability",
                    file,
                )?);
            }
            _ => return Ok(None),
        }
    }
    Ok(Some(out))
}

fn parse_item_filter(
    filter_obj: &Map<String, Value>,
    file: &Path,
) -> Result<Option<ItemFilterPredicate>, LootError> {
    let mut out = ItemFilterPredicate::default();
    for (key, value) in filter_obj {
        match key.as_str() {
            "items" => {
                out.items = parse_string_or_list(value, "filtered.item_filter.items", file)?;
            }
            "count" => {
                out.count = Some(parse_int_range(value, "filtered.item_filter.count", file)?);
            }
            "predicates" => {
                let preds_obj = value.as_object().ok_or_else(|| {
                    invalid(
                        file,
                        "filtered.item_filter.predicates must be a JSON object",
                    )
                })?;
                for (pred_key, pred_val) in preds_obj {
                    match pred_key.as_str() {
                        "minecraft:enchantments" => {
                            out.enchantments = parse_enchantment_predicate_list(
                                pred_val,
                                "filtered.item_filter.predicates.minecraft:enchantments",
                                file,
                            )?;
                        }
                        "minecraft:stored_enchantments" => {
                            out.stored_enchantments = parse_enchantment_predicate_list(
                                pred_val,
                                "filtered.item_filter.predicates.minecraft:stored_enchantments",
                                file,
                            )?;
                        }
                        "minecraft:damage" => {
                            let Some(dmg) = parse_damage_predicate(pred_val, file)? else {
                                return Ok(None);
                            };
                            out.damage = Some(dmg);
                        }
                        _ => return Ok(None),
                    }
                }
            }
            _ => return Ok(None),
        }
    }
    Ok(Some(out))
}

fn parse_filtered(
    obj: &Map<String, Value>,
    kind: &str,
    conditions: Vec<LootCondition>,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
    depth: usize,
) -> Result<LootModifier, LootError> {
    check_allowed_keys(
        obj,
        &["type", "item_filter", "on_pass", "on_fail", "condition"],
        "filtered modifier",
        file,
    )?;
    let filter_obj = obj
        .get("item_filter")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            invalid(
                file,
                "filtered modifier missing or non-object `item_filter`",
            )
        })?;
    let on_pass = parse_optional_modifiers_depth(obj.get("on_pass"), file, resolver, depth + 1)?;
    let on_fail = parse_optional_modifiers_depth(obj.get("on_fail"), file, resolver, depth + 1)?;
    let Some(item_filter) = parse_item_filter(filter_obj, file)? else {
        return Ok(LootModifier::Unsupported {
            what: kind.to_owned(),
        });
    };
    Ok(LootModifier::Filtered {
        item_filter,
        on_pass,
        on_fail,
        conditions,
    })
}

fn parse_apply_bonus(
    obj: &Map<String, Value>,
    conditions: Vec<LootCondition>,
    file: &Path,
) -> Result<LootModifier, LootError> {
    check_allowed_keys(
        obj,
        &["type", "enchantment", "formula", "parameters", "condition"],
        "apply_bonus modifier",
        file,
    )?;
    let enchantment = required_str(obj, "enchantment", "apply_bonus modifier", file)?.to_owned();
    let formula_name = required_str(obj, "formula", "apply_bonus modifier", file)?;
    let params = match obj.get("parameters") {
        None => None,
        Some(raw) => Some(
            raw.as_object()
                .ok_or_else(|| invalid(file, "apply_bonus `parameters` must be a JSON object"))?,
        ),
    };

    let formula = match normalize_item_id(formula_name) {
        "ore_drops" => {
            if let Some(map) = params {
                check_allowed_keys(map, &[], "apply_bonus ore_drops parameters", file)?;
            }
            BonusFormula::OreDrops
        }
        "uniform_bonus_count" => {
            let map = params
                .ok_or_else(|| invalid(file, "uniform_bonus_count requires `parameters` object"))?;
            check_allowed_keys(
                map,
                &["bonusMultiplier"],
                "apply_bonus uniform_bonus_count parameters",
                file,
            )?;
            let raw_mult = map
                .get("bonusMultiplier")
                .ok_or_else(|| invalid(file, "uniform_bonus_count missing `bonusMultiplier`"))?;
            let bonus_multiplier = parse_finite_f64(raw_mult, "bonusMultiplier", file)?;
            if bonus_multiplier < 0.0 {
                return Err(invalid(
                    file,
                    format!(
                        "uniform_bonus_count `bonusMultiplier` must be >= 0, got {bonus_multiplier}"
                    ),
                ));
            }
            BonusFormula::UniformBonusCount { bonus_multiplier }
        }
        "binomial_with_bonus_count" => {
            let map = params.ok_or_else(|| {
                invalid(
                    file,
                    "binomial_with_bonus_count requires `parameters` object",
                )
            })?;
            check_allowed_keys(
                map,
                &["extra", "probability"],
                "apply_bonus binomial_with_bonus_count parameters",
                file,
            )?;
            let raw_extra = map
                .get("extra")
                .ok_or_else(|| invalid(file, "binomial_with_bonus_count missing `extra`"))?;
            let raw_prob = map
                .get("probability")
                .ok_or_else(|| invalid(file, "binomial_with_bonus_count missing `probability`"))?;
            BonusFormula::BinomialWithBonusCount {
                extra: parse_u32(raw_extra, "apply_bonus.extra", file)?,
                probability: parse_probability(raw_prob, "apply_bonus.probability", file)?,
            }
        }
        _ => {
            return Err(invalid(
                file,
                format!("unknown apply_bonus formula `{formula_name}`"),
            ));
        }
    };

    Ok(LootModifier::ApplyBonus {
        enchantment,
        formula,
        conditions,
    })
}

fn parse_limit_count(
    obj: &Map<String, Value>,
    conditions: Vec<LootCondition>,
    file: &Path,
) -> Result<LootModifier, LootError> {
    check_allowed_keys(
        obj,
        &["type", "limit", "condition"],
        "limit_count modifier",
        file,
    )?;
    let raw_limit = obj
        .get("limit")
        .ok_or_else(|| invalid(file, "limit_count modifier missing `limit`"))?;
    let (min, max) = if raw_limit.is_i64() {
        let exact = parse_i32(raw_limit, "limit_count.limit", file)?;
        (Some(exact), Some(exact))
    } else {
        let limit_obj = raw_limit
            .as_object()
            .ok_or_else(|| invalid(file, "limit_count `limit` must be an int or object"))?;
        check_allowed_keys(limit_obj, &["min", "max"], "limit_count.limit", file)?;
        let min = match limit_obj.get("min") {
            None => None,
            Some(v) => Some(parse_i32(v, "limit_count.limit.min", file)?),
        };
        let max = match limit_obj.get("max") {
            None => None,
            Some(v) => Some(parse_i32(v, "limit_count.limit.max", file)?),
        };
        if min.is_none() && max.is_none() {
            return Err(invalid(
                file,
                "limit_count `limit` object must set `min`, `max`, or both",
            ));
        }
        (min, max)
    };
    Ok(LootModifier::LimitCount {
        min,
        max,
        conditions,
    })
}

fn parse_copy_components(
    obj: &Map<String, Value>,
    conditions: Vec<LootCondition>,
    file: &Path,
) -> Result<LootModifier, LootError> {
    check_allowed_keys(
        obj,
        &["type", "source", "include", "exclude", "condition"],
        "copy_components modifier",
        file,
    )?;
    let source = required_str(obj, "source", "copy_components modifier", file)?.to_owned();
    let include = match obj.get("include") {
        None => Vec::new(),
        Some(v) => parse_string_list(v, "copy_components.include", file)?,
    };
    let exclude = match obj.get("exclude") {
        None => Vec::new(),
        Some(v) => parse_string_list(v, "copy_components.exclude", file)?,
    };
    Ok(LootModifier::CopyComponents {
        source,
        include,
        exclude,
        conditions,
    })
}

fn parse_copy_state(
    obj: &Map<String, Value>,
    conditions: Vec<LootCondition>,
    file: &Path,
) -> Result<LootModifier, LootError> {
    check_allowed_keys(
        obj,
        &["type", "block", "properties", "condition"],
        "copy_state modifier",
        file,
    )?;
    let block = required_str(obj, "block", "copy_state modifier", file)?.to_owned();
    let raw_props = obj
        .get("properties")
        .ok_or_else(|| invalid(file, "copy_state modifier missing `properties`"))?;
    let properties = parse_string_list(raw_props, "copy_state.properties", file)?;
    Ok(LootModifier::CopyState {
        block,
        properties,
        conditions,
    })
}

fn parse_string_list(value: &Value, context: &str, file: &Path) -> Result<Vec<String>, LootError> {
    let arr = value
        .as_array()
        .ok_or_else(|| invalid(file, format!("expected array in {context}")))?;
    arr.iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid(file, format!("non-string element in {context}")))
        })
        .collect()
}

fn collect_unsupported_condition<'a>(condition: &'a LootCondition, out: &mut Vec<&'a str>) {
    match condition {
        LootCondition::AllOf { terms } | LootCondition::AnyOf { terms } => {
            for term in terms {
                collect_unsupported_condition(term, out);
            }
        }
        LootCondition::Inverted { term } => collect_unsupported_condition(term, out),
        LootCondition::Unsupported { what } => out.push(what.as_str()),
        LootCondition::SurvivesExplosion
        | LootCondition::MatchBlock { .. }
        | LootCondition::LocationCheck { .. }
        | LootCondition::TableBonus { .. }
        | LootCondition::RandomChance { .. }
        | LootCondition::RandomChanceWithEnchantedBonus { .. }
        | LootCondition::KilledByPlayer
        | LootCondition::MatchTool { .. }
        | LootCondition::EntityProperties { .. }
        | LootCondition::DamageSourceProperties { .. } => {}
    }
}

fn modifier_uses_enchantments(modifier: &LootModifier) -> bool {
    match modifier {
        LootModifier::EnchantRandomly { .. } | LootModifier::EnchantWithLevels { .. } => true,
        LootModifier::Filtered {
            on_pass, on_fail, ..
        } => {
            on_pass.iter().any(modifier_uses_enchantments)
                || on_fail.iter().any(modifier_uses_enchantments)
        }
        _ => false,
    }
}

fn entry_uses_enchantments(entry: &LootEntry) -> bool {
    match entry {
        LootEntry::Item { modifiers, .. }
        | LootEntry::Empty { modifiers, .. }
        | LootEntry::Tag { modifiers, .. }
        | LootEntry::Dynamic { modifiers, .. } => modifiers.iter().any(modifier_uses_enchantments),
        LootEntry::LootTable {
            source, modifiers, ..
        } => {
            modifiers.iter().any(modifier_uses_enchantments)
                || matches!(source, LootTableSource::Inline(inline) if table_uses_enchantments(inline))
        }
        LootEntry::Alternatives {
            children,
            modifiers,
            ..
        } => {
            modifiers.iter().any(modifier_uses_enchantments)
                || children.iter().any(entry_uses_enchantments)
        }
    }
}

fn table_uses_enchantments(table: &LootTable) -> bool {
    table.modifiers.iter().any(modifier_uses_enchantments)
        || table.pools.iter().any(|pool| {
            pool.modifiers.iter().any(modifier_uses_enchantments)
                || pool.entries.iter().any(entry_uses_enchantments)
        })
}

fn collect_unsupported_modifier<'a>(modifier: &'a LootModifier, out: &mut Vec<&'a str>) {
    for condition in modifier.conditions() {
        collect_unsupported_condition(condition, out);
    }
    match modifier {
        LootModifier::Unsupported { what } => out.push(what.as_str()),
        LootModifier::Filtered {
            on_pass, on_fail, ..
        } => {
            for inner in on_pass {
                collect_unsupported_modifier(inner, out);
            }
            for inner in on_fail {
                collect_unsupported_modifier(inner, out);
            }
        }
        _ => {}
    }
}

fn collect_unsupported_entry<'a>(entry: &'a LootEntry, out: &mut Vec<&'a str>) {
    match entry {
        LootEntry::Item {
            conditions,
            modifiers,
            ..
        }
        | LootEntry::Empty {
            conditions,
            modifiers,
            ..
        }
        | LootEntry::Tag {
            conditions,
            modifiers,
            ..
        }
        | LootEntry::Dynamic {
            conditions,
            modifiers,
            ..
        } => {
            for condition in conditions {
                collect_unsupported_condition(condition, out);
            }
            for modifier in modifiers {
                collect_unsupported_modifier(modifier, out);
            }
        }
        LootEntry::LootTable {
            source,
            conditions,
            modifiers,
            ..
        } => {
            for condition in conditions {
                collect_unsupported_condition(condition, out);
            }
            for modifier in modifiers {
                collect_unsupported_modifier(modifier, out);
            }
            if let LootTableSource::Inline(inline) = source {
                out.extend(inline.unsupported());
            }
        }
        LootEntry::Alternatives {
            children,
            conditions,
            modifiers,
        } => {
            for condition in conditions {
                collect_unsupported_condition(condition, out);
            }
            for modifier in modifiers {
                collect_unsupported_modifier(modifier, out);
            }
            for child in children {
                collect_unsupported_entry(child, out);
            }
        }
    }
}

struct EvalCtx<'a> {
    loot: &'a LootContext,
    item_tags: &'a ItemTags,
    block_tags: &'a BlockTags,
    enchantment_tags: &'a EnchantmentTags,
    enchantments: &'a EnchantmentRegistry,
    entity_type_tags: &'a HashMap<String, Vec<String>>,
    damage_type_tags: &'a HashMap<String, Vec<String>>,
    recipes: Option<&'a RecipeData>,
    item_defaults: Option<&'a ItemDefaults>,
}

struct PendingDrop {
    item: String,
    count: i32,
    components: ItemComponents,
    discarded: bool,
}

impl PendingDrop {
    fn new(item: String) -> Self {
        Self {
            item,
            count: 1,
            components: ItemComponents::default(),
            discarded: false,
        }
    }
}

enum SelectedEntryKind<'a> {
    Item(&'a str),
    Dynamic(&'a str),
    LootTable(&'a LootTableSource),
    Tag(&'a str),
    Empty,
}

struct SelectedEntry<'a> {
    kind: SelectedEntryKind<'a>,
    weight: u32,
    quality: i32,
    modifiers: Vec<&'a LootModifier>,
}

fn effective_weight(weight: u32, quality: i32, luck: f64) -> u64 {
    let raw = (f64::from(weight) + f64::from(quality) * luck).floor();
    if !raw.is_finite() || raw <= 0.0 {
        0
    } else if raw >= f64::from(u32::MAX) {
        u64::from(u32::MAX)
    } else {
        raw as u64
    }
}

fn pick_weighted_candidate<'a>(
    candidates: &mut Vec<SelectedEntry<'a>>,
    luck: f64,
    random: &mut RandomSource,
) -> Option<SelectedEntry<'a>> {
    let mut total_weight = 0_u64;
    let mut positive_count = 0_usize;
    let mut single_positive_idx = 0_usize;
    for (idx, candidate) in candidates.iter().enumerate() {
        let eff = effective_weight(candidate.weight, candidate.quality, luck);
        if eff > 0 {
            total_weight = total_weight.saturating_add(eff);
            positive_count += 1;
            single_positive_idx = idx;
        }
    }
    if positive_count == 0 || total_weight == 0 {
        return None;
    }
    if positive_count == 1 {
        return Some(candidates.swap_remove(single_positive_idx));
    }
    let bound = u32::try_from(total_weight).unwrap_or(u32::MAX);
    let mut remaining = u64::from(random.next_int_bounded(bound));
    let mut chosen_idx = single_positive_idx;
    for (idx, candidate) in candidates.iter().enumerate() {
        let weight = effective_weight(candidate.weight, candidate.quality, luck);
        if weight == 0 {
            continue;
        }
        if remaining < weight {
            chosen_idx = idx;
            break;
        }
        remaining -= weight;
    }
    Some(candidates.swap_remove(chosen_idx))
}

fn expand_entry<'a>(
    entry: &'a LootEntry,
    ctx: &EvalCtx<'a>,
    random: &mut RandomSource,
    out: &mut Vec<SelectedEntry<'a>>,
) -> bool {
    if !eval_conditions(entry.conditions(), ctx, random) {
        return false;
    }
    match entry {
        LootEntry::Item {
            name,
            weight,
            quality,
            modifiers,
            ..
        } => {
            out.push(SelectedEntry {
                kind: SelectedEntryKind::Item(name.as_str()),
                weight: *weight,
                quality: *quality,
                modifiers: modifiers.iter().collect(),
            });
            true
        }
        LootEntry::Dynamic {
            name,
            weight,
            quality,
            modifiers,
            ..
        } => {
            out.push(SelectedEntry {
                kind: SelectedEntryKind::Dynamic(name.as_str()),
                weight: *weight,
                quality: *quality,
                modifiers: modifiers.iter().collect(),
            });
            true
        }
        LootEntry::LootTable {
            source,
            weight,
            quality,
            modifiers,
            ..
        } => {
            out.push(SelectedEntry {
                kind: SelectedEntryKind::LootTable(source),
                weight: *weight,
                quality: *quality,
                modifiers: modifiers.iter().collect(),
            });
            true
        }
        LootEntry::Empty {
            weight,
            quality,
            modifiers,
            ..
        } => {
            out.push(SelectedEntry {
                kind: SelectedEntryKind::Empty,
                weight: *weight,
                quality: *quality,
                modifiers: modifiers.iter().collect(),
            });
            true
        }
        LootEntry::Tag {
            tag,
            expand,
            weight,
            quality,
            modifiers,
            ..
        } => {
            if *expand {
                let Some(members) = ctx.item_tags.members(tag) else {
                    return false;
                };
                if members.is_empty() {
                    return false;
                }
                for member in members {
                    out.push(SelectedEntry {
                        kind: SelectedEntryKind::Item(member.as_str()),
                        weight: *weight,
                        quality: *quality,
                        // minecraft.wiki `Loot table` rev 3893957: expanded tag entries
                        // do not apply entry-level item modifiers (Bug MC-212671).
                        modifiers: Vec::new(),
                    });
                }
                true
            } else {
                out.push(SelectedEntry {
                    kind: SelectedEntryKind::Tag(tag.as_str()),
                    weight: *weight,
                    quality: *quality,
                    modifiers: modifiers.iter().collect(),
                });
                true
            }
        }
        LootEntry::Alternatives {
            children,
            modifiers,
            ..
        } => {
            let start = out.len();
            for child in children {
                if expand_entry(child, ctx, random, out) {
                    for selected in &mut out[start..] {
                        selected.modifiers.extend(modifiers.iter());
                    }
                    return true;
                }
            }
            false
        }
    }
}

fn eval_conditions(
    conditions: &[LootCondition],
    ctx: &EvalCtx<'_>,
    random: &mut RandomSource,
) -> bool {
    conditions
        .iter()
        .all(|condition| eval_condition(condition, ctx, random))
}

fn eval_condition(condition: &LootCondition, ctx: &EvalCtx<'_>, random: &mut RandomSource) -> bool {
    match condition {
        LootCondition::SurvivesExplosion => ctx
            .loot
            .explosion_radius
            .is_none_or(|radius| survives_explosion_radius(radius, random)),
        LootCondition::MatchBlock { blocks, state } => match_block(blocks, state, ctx),
        LootCondition::LocationCheck {
            offset_x,
            offset_y,
            offset_z,
            blocks,
            state,
        } => {
            let Some((block_id, properties)) = ctx.loot.block_at(*offset_x, *offset_y, *offset_z)
            else {
                return false;
            };
            match_block_state(blocks, state, &block_id, &properties, ctx.block_tags)
        }
        LootCondition::TableBonus {
            enchantment,
            chances,
        } => {
            let Some(&last_chance) = chances.last() else {
                return false;
            };
            let level = usize::from(tool_enchantment_level(ctx.loot.tool.as_ref(), enchantment));
            let chance = chances.get(level).copied().unwrap_or(last_chance);
            sample_probability(chance, random)
        }
        LootCondition::AllOf { terms } => eval_conditions(terms, ctx, random),
        LootCondition::AnyOf { terms } => {
            terms.iter().any(|term| eval_condition(term, ctx, random))
        }
        LootCondition::Inverted { term } => !eval_condition(term, ctx, random),
        LootCondition::RandomChance { chance } => sample_probability(*chance, random),
        LootCondition::RandomChanceWithEnchantedBonus {
            enchantment,
            unenchanted_chance,
            enchanted_chance,
        } => {
            let level = attacker_enchantment_level(ctx.loot, enchantment);
            let chance = if level == 0 {
                *unenchanted_chance
            } else {
                enchanted_chance.evaluate(level)
            };
            sample_probability(chance, random)
        }
        LootCondition::KilledByPlayer => ctx.loot.killed_by_player,
        LootCondition::MatchTool { predicate } => {
            let Some(tool) = ctx.loot.tool.as_ref() else {
                return false;
            };
            match_tool_predicate(predicate, tool, ctx)
        }
        LootCondition::EntityProperties { entity, predicate } => {
            let target = match entity.as_str() {
                "this" => ctx.loot.entity.as_ref(),
                "attacker" => ctx.loot.attacker.as_ref(),
                "direct_attacker" => ctx.loot.direct_attacker.as_ref(),
                _ => None,
            };
            let Some(entity_ctx) = target else {
                return false;
            };
            match_entity_predicate(predicate, entity_ctx, ctx, 0)
        }
        LootCondition::DamageSourceProperties { predicate } => {
            let Some(damage_source) = ctx.loot.damage_source.as_ref() else {
                return false;
            };
            match_damage_source_predicate(predicate, damage_source, ctx)
        }
        LootCondition::Unsupported { .. } => false,
    }
}

fn survives_explosion_radius(radius: f32, random: &mut RandomSource) -> bool {
    if !radius.is_finite() || radius <= 0.0 {
        return false;
    }
    let survival_chance = 1.0 / f64::from(radius);
    sample_probability(survival_chance, random)
}

fn sample_probability(chance: f64, random: &mut RandomSource) -> bool {
    f64::from(random.next_float()) < chance
}

fn normalize_tag_id(raw: &str) -> String {
    let stripped = raw.strip_prefix('#').unwrap_or(raw);
    ensure_namespaced(stripped)
}

fn component_value_matches(actual: &str, expected: &str) -> bool {
    if actual.contains(':') || expected.contains(':') {
        items_equal(actual, expected)
    } else {
        actual == expected
    }
}

fn match_block(blocks: &[String], state: &BTreeMap<String, String>, ctx: &EvalCtx<'_>) -> bool {
    let (block_id, properties) = &ctx.loot.block_state;
    match_block_state(blocks, state, block_id, properties, ctx.block_tags)
}

fn match_block_state(
    blocks: &[String],
    state: &BTreeMap<String, String>,
    block_id: &str,
    properties: &BTreeMap<String, String>,
    block_tags: &BlockTags,
) -> bool {
    if !blocks.is_empty() {
        if block_id.is_empty() {
            return false;
        }
        let id_matches = blocks.iter().any(|spec| {
            if let Some(tag_id) = spec.strip_prefix('#') {
                let tag = ensure_namespaced(tag_id);
                block_tags
                    .members(&tag)
                    .is_some_and(|members| members.iter().any(|m| items_equal(m, block_id)))
            } else {
                items_equal(spec, block_id)
            }
        });
        if !id_matches {
            return false;
        }
    }
    state
        .iter()
        .all(|(prop, expected)| properties.get(prop) == Some(expected))
}

fn item_matches_spec_list(specs: &[String], item_id: &str, item_tags: &ItemTags) -> bool {
    if item_id.is_empty() {
        return false;
    }
    specs.iter().any(|spec| {
        if let Some(tag_id) = spec.strip_prefix('#') {
            let tag = ensure_namespaced(tag_id);
            item_tags
                .members(&tag)
                .is_some_and(|members| members.iter().any(|m| items_equal(m, item_id)))
        } else {
            items_equal(spec, item_id)
        }
    })
}

fn match_tool_predicate(predicate: &ToolPredicate, tool: &ToolContext, ctx: &EvalCtx<'_>) -> bool {
    if !predicate.items.is_empty()
        && !item_matches_spec_list(&predicate.items, &tool.item, ctx.item_tags)
    {
        return false;
    }
    predicate
        .enchantments
        .iter()
        .all(|ench_pred| match_enchantment_map(ench_pred, &tool.enchantments, ctx.enchantment_tags))
}

fn match_enchantment_map(
    pred: &EnchantmentPredicate,
    enchantments: &BTreeMap<String, u8>,
    enchantment_tags: &EnchantmentTags,
) -> bool {
    let matches_level = |lvl: u8| -> bool {
        lvl > 0
            && pred.min_level.is_none_or(|min| u32::from(lvl) >= min)
            && pred.max_level.is_none_or(|max| u32::from(lvl) <= max)
    };
    if pred.enchantments.is_empty() {
        enchantments.values().copied().any(matches_level)
    } else {
        pred.enchantments.iter().any(|target| {
            if let Some(tag_id) = target.strip_prefix('#') {
                let tag = ensure_namespaced(tag_id);
                enchantment_tags.members(&tag).is_some_and(|members| {
                    members.iter().any(|member| {
                        enchantments
                            .iter()
                            .any(|(id, &lvl)| items_equal(id, member) && matches_level(lvl))
                    })
                })
            } else {
                enchantments
                    .iter()
                    .any(|(id, &lvl)| items_equal(id, target) && matches_level(lvl))
            }
        })
    }
}

fn match_item_filter(filter: &ItemFilterPredicate, drop: &PendingDrop, ctx: &EvalCtx<'_>) -> bool {
    if !filter.items.is_empty() && !item_matches_spec_list(&filter.items, &drop.item, ctx.item_tags)
    {
        return false;
    }
    if let Some(count_range) = filter.count
        && !count_range.matches(drop.count)
    {
        return false;
    }
    if !filter.enchantments.iter().all(|pred| {
        match_enchantment_map(pred, drop.components.enchantments(), ctx.enchantment_tags)
    }) {
        return false;
    }
    if !filter.stored_enchantments.iter().all(|pred| {
        match_enchantment_map(
            pred,
            drop.components.stored_enchantments(),
            ctx.enchantment_tags,
        )
    }) {
        return false;
    }
    if let Some(dmg_pred) = filter.damage {
        let max_damage = ctx
            .item_defaults
            .and_then(|defaults| defaults.max_damage(&drop.item));
        if max_damage.is_none() && drop.components.damage().is_none() {
            return false;
        }
        let dmg = drop.components.damage().unwrap_or(0);
        if let Some(dmg_range) = dmg_pred.damage
            && !i32::try_from(dmg).is_ok_and(|v| dmg_range.matches(v))
        {
            return false;
        }
        if let Some(dur_range) = dmg_pred.durability {
            let Some(max_dmg) = max_damage else {
                return false;
            };
            let remaining = max_dmg.saturating_sub(dmg);
            if !i32::try_from(remaining).is_ok_and(|v| dur_range.matches(v)) {
                return false;
            }
        }
    }
    true
}

fn match_entity_predicate(
    pred: &EntityPredicate,
    entity: &EntityContext,
    ctx: &EvalCtx<'_>,
    depth: usize,
) -> bool {
    if depth > MAX_LOOT_TABLE_DEPTH {
        return false;
    }
    if let Some(flags) = &pred.flags {
        if let Some(expected) = flags.is_on_fire
            && entity.is_on_fire != expected
        {
            return false;
        }
        if let Some(expected) = flags.is_baby
            && entity.is_baby != expected
        {
            return false;
        }
    }
    if !pred.entity_types.is_empty() {
        let matched = pred.entity_types.iter().any(|spec| {
            if let Some(raw_tag) = spec.strip_prefix('#') {
                let tag = ensure_namespaced(raw_tag);
                let explicit = entity
                    .entity_type_tags
                    .iter()
                    .any(|present| normalize_tag_id(present) == tag);
                let registry = !entity.entity_type.is_empty()
                    && ctx.entity_type_tags.get(&tag).is_some_and(|members| {
                        members.iter().any(|m| items_equal(m, &entity.entity_type))
                    });
                explicit || registry
            } else {
                !entity.entity_type.is_empty() && items_equal(spec, &entity.entity_type)
            }
        });
        if !matched {
            return false;
        }
    }
    if let Some(vehicle_pred) = &pred.vehicle {
        let Some(vehicle) = entity.vehicle.as_deref() else {
            return false;
        };
        if !match_entity_predicate(vehicle_pred, vehicle, ctx, depth + 1) {
            return false;
        }
    }
    if let Some(equip_pred) = &pred.equipment
        && let Some(mainhand_pred) = &equip_pred.mainhand
    {
        let Some(mainhand) = entity.mainhand.as_ref() else {
            return false;
        };
        if !match_tool_predicate(mainhand_pred, mainhand, ctx) {
            return false;
        }
    }
    for (comp_key, expected_val) in &pred.components {
        let found = entity
            .components
            .iter()
            .find(|(actual_key, _)| items_equal(actual_key, comp_key));
        let Some((_, actual_val)) = found else {
            return false;
        };
        if !component_value_matches(actual_val, expected_val) {
            return false;
        }
    }
    if let Some(expected) = pred.sheep_sheared
        && entity.sheep_sheared != Some(expected)
    {
        return false;
    }
    if let Some(range) = pred.cube_mob_size {
        let Some(size) = entity.cube_mob_size else {
            return false;
        };
        if !range.matches(size) {
            return false;
        }
    }
    if let Some(expected) = pred.raider_is_captain
        && entity.raider_is_captain != Some(expected)
    {
        return false;
    }
    if let Some(hook_pred) = pred.fishing_hook {
        let is_fishing_bobber = entity.fishing_hook_in_open_water.is_some()
            || items_equal(&entity.entity_type, "minecraft:fishing_bobber");
        if !is_fishing_bobber {
            return false;
        }
        if let Some(expected) = hook_pred.in_open_water
            && entity.fishing_hook_in_open_water != Some(expected)
        {
            return false;
        }
    }
    true
}

fn match_damage_source_predicate(
    pred: &DamageSourcePredicate,
    damage_source: &DamageSourceContext,
    ctx: &EvalCtx<'_>,
) -> bool {
    if let Some(direct_pred) = &pred.direct_entity {
        let Some(direct) = damage_source.direct_entity.as_ref() else {
            return false;
        };
        if !match_entity_predicate(direct_pred, direct, ctx, 0) {
            return false;
        }
    }
    if let Some(source_pred) = &pred.source_entity {
        let Some(source) = damage_source.source_entity.as_ref() else {
            return false;
        };
        if !match_entity_predicate(source_pred, source, ctx, 0) {
            return false;
        }
    }
    for tag_pred in &pred.tags {
        let present = if let Some(raw_tag) = tag_pred.id.strip_prefix('#') {
            let tag = ensure_namespaced(raw_tag);
            let explicit = damage_source
                .tags
                .iter()
                .any(|present| normalize_tag_id(present) == tag);
            let registry = damage_source.damage_type.as_ref().is_some_and(|dt| {
                ctx.damage_type_tags
                    .get(&tag)
                    .is_some_and(|members| members.iter().any(|m| items_equal(m, dt)))
            });
            explicit || registry
        } else {
            damage_source
                .damage_type
                .as_ref()
                .is_some_and(|dt| items_equal(&tag_pred.id, dt))
                || damage_source
                    .tags
                    .iter()
                    .any(|present| items_equal(present, &tag_pred.id))
        };
        if present != tag_pred.expected {
            return false;
        }
    }
    true
}

fn tool_enchantment_level(tool: Option<&ToolContext>, enchantment: &str) -> u8 {
    let Some(tool) = tool else {
        return 0;
    };
    tool.enchantments
        .iter()
        .filter(|(id, _)| items_equal(id, enchantment))
        .map(|(_, &lvl)| lvl)
        .max()
        .unwrap_or(0)
}

fn attacker_enchantment_level(loot: &LootContext, enchantment: &str) -> u8 {
    let weapon = loot
        .attacker_weapon
        .as_ref()
        .or_else(|| loot.attacker.as_ref().and_then(|e| e.mainhand.as_ref()));
    tool_enchantment_level(weapon, enchantment)
}

fn apply_modifier(
    modifier: &LootModifier,
    drop: &mut PendingDrop,
    ctx: &EvalCtx<'_>,
    random: &mut RandomSource,
    notes: &mut Vec<LootNote>,
) {
    match modifier {
        LootModifier::SetCount { count: np, add, .. } => {
            let delta = sample_count_provider(np, random);
            if *add {
                drop.count = drop.count.saturating_add(delta);
            } else {
                drop.count = delta;
            }
        }
        LootModifier::EnchantedCountIncrease {
            enchantment,
            count: np,
            limit,
            ..
        } => {
            let level = attacker_enchantment_level(ctx.loot, enchantment);
            if level > 0 {
                let per_level = sample_float_provider(np, random);
                let delta = round_f64_to_i32(per_level * f64::from(level));
                drop.count = drop.count.saturating_add(delta);
                if *limit > 0 {
                    let cap = i32::try_from(*limit).unwrap_or(i32::MAX);
                    drop.count = drop.count.min(cap);
                }
            }
        }
        LootModifier::FurnaceSmelt { .. } => {
            if let Some(recipes) = ctx.recipes
                && let Some(cooked) = recipes.find_cooking(CookingKind::Smelting, &drop.item)
            {
                drop.item = ensure_namespaced(cooked.result().item());
            }
        }
        LootModifier::SetPotion { id, .. } => {
            if id.is_empty() || id == "empty" || id == "minecraft:empty" {
                let _ = drop.components.set_potion(None);
            } else {
                let _ = drop.components.set_potion(Some(ensure_namespaced(id)));
            }
        }
        LootModifier::ExplosionDecay { .. } => {
            if let Some(radius) = ctx.loot.explosion_radius
                && drop.count > 0
            {
                let mut kept = 0_i32;
                for _ in 0..drop.count {
                    if survives_explosion_radius(radius, random) {
                        kept += 1;
                    }
                }
                drop.count = kept;
            }
        }
        LootModifier::ApplyBonus {
            enchantment,
            formula,
            ..
        } => {
            apply_bonus(
                enchantment,
                formula,
                &mut drop.count,
                ctx.loot.tool.as_ref(),
                random,
            );
        }
        LootModifier::LimitCount { min, max, .. } => {
            if let Some(lo) = min {
                drop.count = drop.count.max(*lo);
            }
            if let Some(hi) = max {
                drop.count = drop.count.min(*hi);
            }
        }
        LootModifier::CopyComponents {
            source,
            include,
            exclude,
            ..
        } => {
            notes.push(LootNote::CopyComponentsNotApplied {
                item: ensure_namespaced(&drop.item),
                source: source.clone(),
                include: include.clone(),
                exclude: exclude.clone(),
            });
        }
        LootModifier::CopyState {
            block, properties, ..
        } => {
            notes.push(LootNote::CopyStateNotApplied {
                item: ensure_namespaced(&drop.item),
                block: block.clone(),
                properties: properties.clone(),
            });
        }
        LootModifier::SetDamage { damage, add, .. } => {
            apply_set_damage(damage, *add, drop, ctx.item_defaults, random);
        }
        LootModifier::SetName { name, target, .. } => match target {
            SetNameTarget::CustomName => drop.components.set_custom_name(Some(name.clone())),
            SetNameTarget::ItemName => {
                notes.push(LootNote::UnsupportedModifier {
                    item: ensure_namespaced(&drop.item),
                    what: "minecraft:set_name".to_owned(),
                });
            }
        },
        LootModifier::SetEnchantments {
            enchantments, add, ..
        } => {
            apply_set_enchantments(enchantments, *add, drop, random);
        }
        LootModifier::EnchantRandomly {
            options,
            only_compatible,
            ..
        } => {
            apply_enchant_randomly(options.as_deref(), *only_compatible, drop, ctx, random);
        }
        LootModifier::EnchantWithLevels {
            levels, options, ..
        } => {
            apply_enchant_with_levels(levels, options.as_deref(), drop, ctx, random);
        }
        LootModifier::Filtered {
            item_filter,
            on_pass,
            on_fail,
            ..
        } => {
            let branch = if match_item_filter(item_filter, drop, ctx) {
                on_pass
            } else {
                on_fail
            };
            for inner in branch {
                if drop.discarded {
                    break;
                }
                if eval_conditions(inner.conditions(), ctx, random) {
                    apply_modifier(inner, drop, ctx, random, notes);
                }
            }
        }
        LootModifier::Discard { .. } => {
            drop.count = 0;
            drop.discarded = true;
        }
        LootModifier::Unsupported { what } => {
            notes.push(LootNote::UnsupportedModifier {
                item: ensure_namespaced(&drop.item),
                what: what.clone(),
            });
        }
    }
}

const BOOK_ITEM_ID: &str = "minecraft:book";
const ENCHANTED_BOOK_ITEM_ID: &str = "minecraft:enchanted_book";

fn is_book_item(item: &str) -> bool {
    items_equal(item, BOOK_ITEM_ID) || items_equal(item, ENCHANTED_BOOK_ITEM_ID)
}

fn apply_set_damage(
    damage: &NumberProvider,
    add: bool,
    drop: &mut PendingDrop,
    item_defaults: Option<&ItemDefaults>,
    random: &mut RandomSource,
) {
    let Some(max_damage) = item_defaults
        .and_then(|defaults| defaults.max_damage(&drop.item))
        .filter(|&max| max > 0)
    else {
        return;
    };
    let max_f = f64::from(max_damage);
    let sampled = sample_float_provider(damage, random);
    let base_fraction = if add {
        1.0 - f64::from(drop.components.damage().unwrap_or(0)) / max_f
    } else {
        0.0
    };
    let remaining_fraction = (base_fraction + sampled).clamp(0.0, 1.0);
    let consumed_fraction = 1.0 - remaining_fraction;
    let new_damage = (consumed_fraction * max_f).round().clamp(0.0, max_f) as u32;
    drop.components
        .set_damage((new_damage > 0).then_some(new_damage));
}

fn apply_set_enchantments(
    enchantments: &BTreeMap<String, NumberProvider>,
    add: bool,
    drop: &mut PendingDrop,
    random: &mut RandomSource,
) {
    let is_book = is_book_item(&drop.item);
    for (ench_id, provider) in enchantments {
        let id = ensure_namespaced(ench_id);
        let delta = sample_count_provider(provider, random);
        let current = if add {
            let map = if is_book {
                drop.components.stored_enchantments()
            } else {
                drop.components.enchantments()
            };
            i32::from(map.get(&id).copied().unwrap_or(0))
        } else {
            0
        };
        let next = current.saturating_add(delta);
        if next < i32::from(MIN_ENCHANTMENT_LEVEL) {
            if is_book {
                drop.components.remove_stored_enchantment(&id);
            } else {
                drop.components.remove_enchantment(&id);
            }
        } else {
            let level = next.min(i32::from(MAX_ENCHANTMENT_LEVEL)) as u8;
            if is_book {
                let _ = drop.components.set_stored_enchantment(id, level);
            } else {
                let _ = drop.components.set_enchantment(id, level);
            }
        }
    }
    if items_equal(&drop.item, BOOK_ITEM_ID) && !drop.components.stored_enchantments().is_empty() {
        drop.item = ENCHANTED_BOOK_ITEM_ID.to_owned();
    }
}

fn resolve_enchant_option_ids(options: Option<&[String]>, ctx: &EvalCtx<'_>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    match options {
        None => {
            for (id, _) in ctx.enchantments.iter() {
                if seen.insert(id.to_owned()) {
                    out.push(id.to_owned());
                }
            }
        }
        Some(specs) => {
            for spec in specs {
                if let Some(tag) = spec.strip_prefix('#') {
                    if let Some(members) = ctx.enchantment_tags.members(tag) {
                        for member in members {
                            let normalized = ensure_namespaced(member);
                            if seen.insert(normalized.clone()) {
                                out.push(normalized);
                            }
                        }
                    }
                } else {
                    let normalized = ensure_namespaced(spec);
                    if seen.insert(normalized.clone()) {
                        out.push(normalized);
                    }
                }
            }
        }
    }
    out
}

fn apply_selected_enchantments(drop: &mut PendingDrop, selected: Vec<(String, u8)>) {
    if selected.is_empty() {
        return;
    }
    if is_book_item(&drop.item) {
        if items_equal(&drop.item, BOOK_ITEM_ID) {
            drop.item = ENCHANTED_BOOK_ITEM_ID.to_owned();
        }
        for (id, level) in selected {
            let _ = drop.components.set_stored_enchantment(id, level);
        }
    } else {
        for (id, level) in selected {
            let _ = drop.components.set_enchantment(id, level);
        }
    }
}

fn apply_enchant_randomly(
    options: Option<&[String]>,
    only_compatible: bool,
    drop: &mut PendingDrop,
    ctx: &EvalCtx<'_>,
    random: &mut RandomSource,
) {
    let candidate_ids = resolve_enchant_option_ids(options, ctx);
    let is_book = is_book_item(&drop.item);
    let candidates: Vec<&EnchantmentDefinition> = candidate_ids
        .iter()
        .filter_map(|id| ctx.enchantments.get(id))
        .filter(|def| {
            !only_compatible || is_book || ctx.enchantments.can_apply(def.id(), &drop.item)
        })
        .collect();
    if candidates.is_empty() {
        return;
    }
    let idx = random.next_int_bounded(u32::try_from(candidates.len()).unwrap_or(u32::MAX)) as usize;
    let chosen = candidates[idx];
    let level = 1 + random.next_int_bounded(u32::from(chosen.max_level())) as u8;
    apply_selected_enchantments(drop, vec![(chosen.id().to_owned(), level)]);
}

fn apply_enchant_with_levels(
    levels: &NumberProvider,
    options: Option<&[String]>,
    drop: &mut PendingDrop,
    ctx: &EvalCtx<'_>,
    random: &mut RandomSource,
) {
    let level = u32::try_from(sample_count_provider(levels, random).max(0)).unwrap_or(u32::MAX);
    let allowed = options.map(|specs| resolve_enchant_option_ids(Some(specs), ctx));
    let fallback_defaults = ItemDefaults::default();
    let defaults = ctx.item_defaults.unwrap_or(&fallback_defaults);
    let selected = select_enchantments_for_loot(
        &drop.item,
        level,
        allowed.as_deref(),
        ctx.enchantments,
        defaults,
        random,
    );
    apply_selected_enchantments(drop, selected);
}

fn apply_bonus(
    enchantment: &str,
    formula: &BonusFormula,
    count: &mut i32,
    tool: Option<&ToolContext>,
    random: &mut RandomSource,
) {
    let level = tool_enchantment_level(tool, enchantment);
    match formula {
        BonusFormula::OreDrops => {
            if level > 0 {
                let bound = u32::from(level) + 2;
                let multiplier = (random.next_int_bounded(bound) as i32).max(1);
                *count = count.saturating_mul(multiplier);
            }
        }
        BonusFormula::UniformBonusCount { bonus_multiplier } => {
            let max_bonus = (f64::from(level) * bonus_multiplier).floor();
            if max_bonus >= 1.0 {
                let bound = if max_bonus >= f64::from(u32::MAX) {
                    u32::MAX
                } else {
                    (max_bonus as u32).saturating_add(1)
                };
                let bonus = random.next_int_bounded(bound) as i32;
                *count = count.saturating_add(bonus);
            }
        }
        BonusFormula::BinomialWithBonusCount { extra, probability } => {
            let trials = u32::from(level).saturating_add(*extra);
            let bonus = sample_binomial(trials, *probability, random);
            *count = count.saturating_add(bonus);
        }
    }
}

fn round_f64_to_i32(value: f64) -> i32 {
    (value + 0.5)
        .floor()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

fn sample_float_provider(provider: &NumberProvider, random: &mut RandomSource) -> f64 {
    match provider {
        NumberProvider::Constant(value) => *value,
        NumberProvider::Uniform { min, max } => {
            if *max <= *min {
                *min
            } else {
                *min + f64::from(random.next_float()) * (*max - *min)
            }
        }
        NumberProvider::Binomial { n, p } => f64::from(sample_binomial(*n, *p, random)),
    }
}

fn sample_count_provider(provider: &NumberProvider, random: &mut RandomSource) -> i32 {
    match provider {
        NumberProvider::Constant(value) => round_f64_to_i32(*value),
        NumberProvider::Uniform { min, max } => {
            if *max <= *min {
                return round_f64_to_i32(*min);
            }
            if min.fract() == 0.0
                && max.fract() == 0.0
                && *min >= f64::from(i32::MIN)
                && *max <= f64::from(i32::MAX)
            {
                let min_i = *min as i32;
                let max_i = *max as i32;
                let span = i64::from(max_i) - i64::from(min_i) + 1;
                if let Ok(bound) = u32::try_from(span) {
                    let offset = i64::from(random.next_int_bounded(bound));
                    return (i64::from(min_i) + offset) as i32;
                }
            }
            let sample = *min + f64::from(random.next_float()) * (*max - *min);
            round_f64_to_i32(sample)
        }
        NumberProvider::Binomial { n, p } => sample_binomial(*n, *p, random),
    }
}

fn sample_binomial(n: u32, p: f64, random: &mut RandomSource) -> i32 {
    let mut successes = 0_i32;
    for _ in 0..n {
        if sample_probability(p, random) {
            successes = successes.saturating_add(1);
        }
    }
    successes
}

fn resolve_max_stack(item_name: &str, item_defaults: Option<&ItemDefaults>) -> u16 {
    item_defaults
        .and_then(|defaults| defaults.max_stack_size(item_name))
        .map(u16::from)
        .unwrap_or_else(|| default_max_stack(item_name))
        .clamp(1, MAX_STACK_LIMIT)
}

fn push_item_stacks(
    drop: PendingDrop,
    item_defaults: Option<&ItemDefaults>,
    out: &mut Vec<ItemStack>,
) {
    let item_name = ensure_namespaced(&drop.item);
    let max_stack = resolve_max_stack(&item_name, item_defaults);
    let has_components = !drop.components.is_empty();
    let mut remaining = drop.count;
    while remaining > 0 {
        let chunk = remaining.min(i32::from(max_stack)) as u16;
        if let Ok(stack) = ItemStack::try_new(item_name.as_str(), chunk, max_stack) {
            let final_stack = if has_components {
                match stack.clone().with_components(drop.components.clone()) {
                    Ok(with_comp) => with_comp,
                    Err(_) => stack,
                }
            } else {
                stack
            };
            out.push(final_stack);
        }
        remaining -= i32::from(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn scratch_root(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rustmc-loot-{label}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create scratch dir");
        path
    }

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().expect("parent dir")).expect("create parent dirs");
        fs::write(path, body).expect("write test json");
    }

    #[test]
    fn parses_all_entry_condition_modifier_and_number_types() {
        let root = scratch_root("all_types");
        write(
            &root.join("data/testns/loot_table/blocks/widget.json"),
            r##"{
                "type": "minecraft:block",
                "modifier": {
                    "type": "minecraft:explosion_decay"
                },
                "pools": [
                    {
                        "rolls": 1,
                        "condition": {
                            "type": "minecraft:all_of",
                            "terms": [
                                {
                                    "type": "minecraft:match_block",
                                    "blocks": ["testns:widget", "testns:other_widget"],
                                    "state": {"active": "true"}
                                },
                                {
                                    "type": "minecraft:inverted",
                                    "term": {
                                        "type": "minecraft:entity_properties",
                                        "entity": "this",
                                        "predicate": {}
                                    }
                                }
                            ]
                        },
                        "modifier": [
                            {
                                "type": "minecraft:limit_count",
                                "limit": {"min": 1, "max": 8}
                            }
                        ],
                        "entries": [
                            {
                                "type": "minecraft:alternatives",
                                "children": [
                                    {
                                        "type": "minecraft:item",
                                        "name": "testns:widget",
                                        "condition": {
                                            "type": "minecraft:any_of",
                                            "terms": [
                                                {
                                                    "type": "minecraft:match_tool",
                                                    "predicate": {
                                                        "items": "#testns:cutters",
                                                        "predicates": {
                                                            "minecraft:enchantments": [
                                                                {
                                                                    "enchantments": "testns:delicate",
                                                                    "levels": {"min": 1}
                                                                }
                                                            ]
                                                        }
                                                    }
                                                },
                                                {
                                                    "type": "minecraft:table_bonus",
                                                    "enchantment": "testns:luck",
                                                    "chances": [0.1, 0.25, 0.5]
                                                }
                                            ]
                                        },
                                        "modifier": [
                                            {
                                                "type": "minecraft:copy_components",
                                                "source": "block_entity",
                                                "include": ["testns:custom"]
                                            },
                                            {
                                                "type": "minecraft:copy_state",
                                                "block": "testns:widget",
                                                "properties": ["active"]
                                            }
                                        ]
                                    },
                                    {
                                        "type": "minecraft:dynamic",
                                        "name": "testns:bits",
                                        "condition": {
                                            "type": "minecraft:survives_explosion"
                                        }
                                    },
                                    {
                                        "type": "minecraft:item",
                                        "name": "testns:crumb",
                                        "condition": {
                                            "type": "minecraft:random_chance",
                                            "chance": 0.5
                                        },
                                        "modifier": [
                                            {
                                                "type": "minecraft:set_count",
                                                "count": {"type": "minecraft:uniform", "min": 1, "max": 3}
                                            },
                                            {
                                                "type": "minecraft:set_count",
                                                "add": true,
                                                "condition": {
                                                    "type": "minecraft:match_block",
                                                    "blocks": "testns:widget"
                                                },
                                                "count": {"type": "minecraft:binomial", "n": 2, "p": 0.25}
                                            },
                                            {
                                                "type": "minecraft:apply_bonus",
                                                "enchantment": "testns:luck",
                                                "formula": "minecraft:ore_drops"
                                            },
                                            {
                                                "type": "minecraft:apply_bonus",
                                                "enchantment": "testns:luck",
                                                "formula": "minecraft:uniform_bonus_count",
                                                "parameters": {"bonusMultiplier": 2}
                                            },
                                            {
                                                "type": "minecraft:apply_bonus",
                                                "enchantment": "testns:luck",
                                                "formula": "minecraft:binomial_with_bonus_count",
                                                "parameters": {"extra": 3, "probability": 0.5}
                                            }
                                        ]
                                    }
                                ]
                            }
                        ]
                    }
                ],
                "random_sequence": "testns:blocks/widget"
            }"##,
        );
        write(
            &root.join("data/testns/loot_table/blocks/sub/sprocket.json"),
            r#"{
                "type": "minecraft:block",
                "random_sequence": "testns:blocks/sub/sprocket"
            }"#,
        );

        let tables = LootTables::load_blocks(&root).expect("load synthetic loot tables");
        assert_eq!(tables.len(), 2);
        assert!(!tables.is_empty());
        assert!(tables.unsupported().is_empty());

        let widget = tables.get("testns:widget").expect("testns:widget");
        assert_eq!(tables.get("testns:blocks/widget"), Some(widget));
        assert_eq!(widget.modifiers.len(), 1);
        assert_eq!(widget.pools.len(), 1);
        assert_eq!(widget.pools[0].rolls, NumberProvider::Constant(1.0));
        assert_eq!(widget.pools[0].modifiers.len(), 1);

        let sprocket = tables.get("testns:sub/sprocket").expect("nested table");
        assert!(sprocket.pools.is_empty());

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn resolves_string_predicate_files() {
        let root = scratch_root("pred_resolve");
        write(
            &root.join("data/testns/predicate/tool/can_harvest.json"),
            r#"{
                "type": "minecraft:match_tool",
                "predicate": {
                    "items": "testns:wrench"
                }
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/blocks/gadget.json"),
            r#"{
                "type": "minecraft:block",
                "pools": [
                    {
                        "rolls": {"type": "minecraft:constant", "value": 2},
                        "entries": [
                            {
                                "type": "minecraft:item",
                                "name": "testns:gadget",
                                "condition": "testns:tool/can_harvest"
                            }
                        ]
                    }
                ]
            }"#,
        );

        let tables = LootTables::load_blocks(&root).expect("load with predicate");
        let gadget = tables.get("testns:gadget").expect("gadget table");
        assert_eq!(gadget.pools[0].rolls, NumberProvider::Constant(2.0));
        assert_eq!(
            gadget.pools[0].entries[0],
            LootEntry::Item {
                name: "testns:gadget".to_owned(),
                weight: 1,
                quality: 0,
                conditions: vec![LootCondition::MatchTool {
                    predicate: ToolPredicate {
                        items: vec!["testns:wrench".to_owned()],
                        enchantments: Vec::new(),
                    },
                }],
                modifiers: Vec::new(),
            }
        );

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn missing_predicate_file_errors_naming_table_file() {
        let root = scratch_root("missing_pred");
        write(
            &root.join("data/testns/loot_table/blocks/bad_ref.json"),
            r#"{
                "type": "minecraft:block",
                "pools": [
                    {
                        "rolls": 1,
                        "entries": [
                            {
                                "type": "minecraft:item",
                                "name": "testns:widget",
                                "condition": "testns:tool/nonexistent"
                            }
                        ]
                    }
                ]
            }"#,
        );

        let err = LootTables::load_blocks(&root)
            .expect_err("missing predicate must error")
            .to_string();
        assert!(
            err.contains("bad_ref.json"),
            "error must name table file: {err}"
        );
        assert!(
            err.contains("testns:tool/nonexistent"),
            "error must name missing predicate: {err}"
        );

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn bad_json_and_unknown_shapes_error_naming_file() {
        let cases = [
            ("corrupt_json.json", "{ not valid json"),
            (
                "unknown_entry.json",
                r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"testns:bogus"}]}]}"#,
            ),
            (
                "unknown_provider.json",
                r#"{"type":"minecraft:block","pools":[{"rolls":{"type":"testns:bogus"},"entries":[]}]}"#,
            ),
            (
                "bad_table_bonus.json",
                r#"{"type":"minecraft:block","pools":[{"rolls":1,"condition":{"type":"minecraft:table_bonus","enchantment":"testns:e"},"entries":[]}]}"#,
            ),
            (
                "bad_chance.json",
                r#"{"type":"minecraft:block","pools":[{"rolls":1,"condition":{"type":"minecraft:random_chance","chance":1.5},"entries":[]}]}"#,
            ),
            (
                "unknown_formula.json",
                r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"testns:x","modifier":{"type":"minecraft:apply_bonus","enchantment":"testns:e","formula":"testns:mystery"}}]}]}"#,
            ),
        ];

        for (filename, body) in cases {
            let root = scratch_root(filename);
            write(
                &root.join("data/testns/loot_table/blocks").join(filename),
                body,
            );
            let err = LootTables::load_blocks(&root)
                .expect_err("bad loot table must fail")
                .to_string();
            assert!(
                err.contains(filename),
                "error for {filename} must name the file: {err}"
            );
            fs::remove_dir_all(&root).expect("cleanup");
        }
    }

    #[test]
    fn unsupported_nodes_listed_across_table_and_nested_conditions() {
        let root = scratch_root("unsupported");
        write(
            &root.join("data/testns/loot_table/blocks/gap.json"),
            r#"{
                "type": "minecraft:block",
                "modifier": {"type": "testns:table_mod"},
                "pools": [
                    {
                        "rolls": 1,
                        "condition": {
                            "type": "minecraft:all_of",
                            "terms": [
                                {"type": "testns:pool_cond"}
                            ]
                        },
                        "entries": [
                            {
                                "type": "minecraft:alternatives",
                                "children": [
                                    {
                                        "type": "minecraft:item",
                                        "name": "testns:widget",
                                        "condition": {"type": "testns:entry_cond"},
                                        "modifier": {
                                            "type": "minecraft:set_count",
                                            "count": 2,
                                            "condition": {"type": "testns:mod_cond"}
                                        }
                                    }
                                ]
                            }
                        ]
                    }
                ]
            }"#,
        );

        let tables = LootTables::load_blocks(&root).expect("load unsupported table");
        assert_eq!(
            tables.unsupported(),
            vec![
                ("testns:blocks/gap", "testns:table_mod"),
                ("testns:blocks/gap", "testns:pool_cond"),
                ("testns:blocks/gap", "testns:entry_cond"),
                ("testns:blocks/gap", "testns:mod_cond"),
            ]
        );

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn list_condition_scalar_limit_cycle_and_empty_root() {
        let empty_root = scratch_root("empty_root");
        let empty = LootTables::load_blocks(&empty_root).expect("empty root loads");
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        fs::remove_dir_all(&empty_root).expect("cleanup");

        let root = scratch_root("list_and_limit");
        write(
            &root.join("data/testns/predicate/check.json"),
            r#"{"type":"minecraft:survives_explosion"}"#,
        );
        write(
            &root.join("data/testns/loot_table/blocks/listy.json"),
            r#"{
                "type": "minecraft:block",
                "pools": [
                    {
                        "rolls": 1,
                        "condition": ["testns:check", {"type": "minecraft:random_chance", "chance": 0.25}],
                        "entries": [
                            {
                                "type": "minecraft:item",
                                "name": "testns:listy",
                                "modifier": {"type": "minecraft:limit_count", "limit": 3}
                            }
                        ]
                    }
                ]
            }"#,
        );
        let tables = LootTables::load_blocks(&root).expect("load list condition");
        let listy = tables.get("testns:listy").expect("listy");
        assert_eq!(
            listy.pools[0].conditions,
            vec![
                LootCondition::SurvivesExplosion,
                LootCondition::RandomChance { chance: 0.25 },
            ]
        );
        assert_eq!(
            listy.pools[0].entries[0],
            LootEntry::Item {
                name: "testns:listy".to_owned(),
                weight: 1,
                quality: 0,
                conditions: Vec::new(),
                modifiers: vec![LootModifier::LimitCount {
                    min: Some(3),
                    max: Some(3),
                    conditions: Vec::new(),
                }],
            }
        );
        fs::remove_dir_all(&root).expect("cleanup");

        let cycle_root = scratch_root("pred_cycle");
        write(
            &cycle_root.join("data/testns/predicate/loop.json"),
            r#"{"type":"minecraft:inverted","term":"testns:loop"}"#,
        );
        write(
            &cycle_root.join("data/testns/loot_table/blocks/cyclic.json"),
            r#"{"type":"minecraft:block","pools":[{"rolls":1,"condition":"testns:loop","entries":[]}]}"#,
        );
        let err = LootTables::load_blocks(&cycle_root)
            .expect_err("cyclic predicate must fail")
            .to_string();
        assert!(
            err.contains("cyclic predicate"),
            "error should report cycle: {err}"
        );
        fs::remove_dir_all(&cycle_root).expect("cleanup");
    }

    fn load_single_table(label: &str, table_name: &str, body: &str) -> LootTable {
        let root = scratch_root(label);
        write(
            &root.join(format!("data/testns/loot_table/blocks/{table_name}.json")),
            body,
        );
        let tables = LootTables::load_blocks(&root).expect("load synthetic table");
        let table = tables
            .get(&format!("testns:{table_name}"))
            .expect("table present")
            .clone();
        fs::remove_dir_all(&root).expect("cleanup");
        table
    }

    #[test]
    fn alternatives_pick_first_passing_child() {
        let table = load_single_table(
            "alt_first",
            "widget",
            r#"{
                "type": "minecraft:block",
                "pools": [
                    {
                        "rolls": 1,
                        "entries": [
                            {
                                "type": "minecraft:alternatives",
                                "children": [
                                    {
                                        "type": "minecraft:item",
                                        "name": "testns:silk_widget",
                                        "condition": {
                                            "type": "minecraft:match_tool",
                                            "predicate": {
                                                "predicates": {
                                                    "minecraft:enchantments": [
                                                        {"enchantments": "testns:delicate", "levels": {"min": 1}}
                                                    ]
                                                }
                                            }
                                        }
                                    },
                                    {
                                        "type": "minecraft:item",
                                        "name": "testns:tool_widget",
                                        "condition": {
                                            "type": "minecraft:match_tool",
                                            "predicate": {"items": "testns:hammer"}
                                        }
                                    },
                                    {
                                        "type": "minecraft:item",
                                        "name": "testns:fallback_widget"
                                    }
                                ]
                            }
                        ]
                    }
                ]
            }"#,
        );

        let mut rng = RandomSource::from_world_seed(42);
        let mut hammer_delicate = BTreeMap::new();
        hammer_delicate.insert("testns:delicate".to_owned(), 1);
        let ctx_first = LootContext {
            block_state: ("testns:widget".to_owned(), BTreeMap::new()),
            tool: Some(ToolContext {
                item: "testns:hammer".to_owned(),
                enchantments: hammer_delicate,
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let drops_first = table.roll(&ctx_first, &mut rng);
        assert_eq!(drops_first.len(), 1);
        assert_eq!(drops_first[0].item(), "testns:silk_widget");
        assert_eq!(drops_first[0].count(), 1);

        let ctx_second = LootContext {
            block_state: ("testns:widget".to_owned(), BTreeMap::new()),
            tool: Some(ToolContext {
                item: "testns:hammer".to_owned(),
                enchantments: BTreeMap::new(),
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let drops_second = table.roll(&ctx_second, &mut rng);
        assert_eq!(drops_second.len(), 1);
        assert_eq!(drops_second[0].item(), "testns:tool_widget");

        let ctx_third = LootContext::for_block("testns:widget");
        let drops_third = table.roll(&ctx_third, &mut rng);
        assert_eq!(drops_third.len(), 1);
        assert_eq!(drops_third[0].item(), "testns:fallback_widget");
    }

    #[test]
    fn survives_explosion_with_and_without_explosion() {
        let table = load_single_table(
            "survives_exp",
            "widget",
            r#"{
                "type": "minecraft:block",
                "pools": [
                    {
                        "rolls": 1,
                        "entries": [
                            {
                                "type": "minecraft:item",
                                "name": "testns:widget",
                                "condition": {"type": "minecraft:survives_explosion"}
                            }
                        ]
                    }
                ]
            }"#,
        );

        let no_explosion = LootContext::for_block("testns:widget");
        for seed in 0..50_u64 {
            let mut rng = RandomSource::from_world_seed(seed);
            let drops = table.roll(&no_explosion, &mut rng);
            assert_eq!(drops.len(), 1, "without explosion must always drop");
            assert_eq!(drops[0].item(), "testns:widget");
        }

        let exploded = LootContext {
            block_state: ("testns:widget".to_owned(), BTreeMap::new()),
            tool: None,
            explosion_radius: Some(4.0),
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let mut survived = 0_u32;
        for seed in 0..10_000_u64 {
            let mut rng = RandomSource::from_world_seed(seed);
            let drops = table.roll(&exploded, &mut rng);
            assert!(drops.len() <= 1);
            survived += drops.len() as u32;
        }
        assert!(
            (2300..=2700).contains(&survived),
            "expected ~2500 survivors at radius 4, got {survived}"
        );
    }

    #[test]
    fn table_bonus_indexes_by_level_and_clamps_above_list() {
        let table = load_single_table(
            "table_bonus",
            "widget",
            r#"{
                "type": "minecraft:block",
                "pools": [
                    {
                        "rolls": 1,
                        "entries": [
                            {
                                "type": "minecraft:item",
                                "name": "testns:bonus_drop",
                                "condition": {
                                    "type": "minecraft:table_bonus",
                                    "enchantment": "testns:luck",
                                    "chances": [0.0, 0.5, 1.0]
                                }
                            }
                        ]
                    }
                ]
            }"#,
        );

        let ctx_level = |level: Option<u8>| LootContext {
            block_state: ("testns:widget".to_owned(), BTreeMap::new()),
            tool: level.map(|lvl| {
                let mut enchantments = BTreeMap::new();
                enchantments.insert("testns:luck".to_owned(), lvl);
                ToolContext {
                    item: "testns:pick".to_owned(),
                    enchantments,
                }
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };

        // Missing tool -> level 0 -> chance 0.0 -> never drops.
        for seed in 0..200_u64 {
            let mut rng = RandomSource::from_world_seed(seed);
            assert!(table.roll(&ctx_level(None), &mut rng).is_empty());
        }

        // Level 1 -> chance 0.5 -> roughly half of 2000 trials.
        let mut level1_hits = 0_u32;
        for seed in 0..2000_u64 {
            let mut rng = RandomSource::from_world_seed(seed);
            level1_hits += table.roll(&ctx_level(Some(1)), &mut rng).len() as u32;
        }
        assert!(
            (900..=1100).contains(&level1_hits),
            "expected ~1000 hits at level 1, got {level1_hits}"
        );

        // Level 2 -> chance 1.0 -> always drops.
        for seed in 0..200_u64 {
            let mut rng = RandomSource::from_world_seed(seed);
            assert_eq!(table.roll(&ctx_level(Some(2)), &mut rng).len(), 1);
        }

        // Level 5 (above chances len 3) -> uses last entry (1.0) -> always drops.
        for seed in 0..200_u64 {
            let mut rng = RandomSource::from_world_seed(seed);
            assert_eq!(table.roll(&ctx_level(Some(5)), &mut rng).len(), 1);
        }
    }

    #[test]
    fn apply_bonus_formulas_pin_exact_counts_for_fixed_seeds() {
        let ore_table = load_single_table(
            "bonus_ore",
            "ore",
            r#"{
                "type": "minecraft:block",
                "pools": [{
                    "rolls": 1,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "testns:gem",
                        "modifier": [
                            {"type": "minecraft:set_count", "count": 2},
                            {
                                "type": "minecraft:apply_bonus",
                                "enchantment": "testns:luck",
                                "formula": "minecraft:ore_drops"
                            }
                        ]
                    }]
                }]
            }"#,
        );
        let uniform_table = load_single_table(
            "bonus_uniform",
            "bush",
            r#"{
                "type": "minecraft:block",
                "pools": [{
                    "rolls": 1,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "testns:berry",
                        "modifier": {
                            "type": "minecraft:apply_bonus",
                            "enchantment": "testns:luck",
                            "formula": "minecraft:uniform_bonus_count",
                            "parameters": {"bonusMultiplier": 2}
                        }
                    }]
                }]
            }"#,
        );
        let binomial_table = load_single_table(
            "bonus_binomial",
            "crop",
            r#"{
                "type": "minecraft:block",
                "pools": [{
                    "rolls": 1,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "testns:seed",
                        "modifier": {
                            "type": "minecraft:apply_bonus",
                            "enchantment": "testns:luck",
                            "formula": "minecraft:binomial_with_bonus_count",
                            "parameters": {"extra": 3, "probability": 0.5}
                        }
                    }]
                }]
            }"#,
        );

        let mut enchs = BTreeMap::new();
        enchs.insert("testns:luck".to_owned(), 3);
        let ctx_luck3 = LootContext {
            block_state: ("testns:block".to_owned(), BTreeMap::new()),
            tool: Some(ToolContext {
                item: "testns:tool".to_owned(),
                enchantments: enchs,
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let ctx_bare = LootContext::for_block("testns:block");

        // Compute exact expected results from independent RandomSource clones with the same formula.
        let ore_counts: Vec<u16> = (0..8_u64)
            .map(|seed| {
                let mut rng = RandomSource::from_world_seed(seed);
                ore_table.roll(&ctx_luck3, &mut rng)[0].count()
            })
            .collect();
        let expected_ore: Vec<u16> = (0..8_u64)
            .map(|seed| {
                let mut rng = RandomSource::from_world_seed(seed);
                let mult = (rng.next_int_bounded(5) as u16).max(1);
                2 * mult
            })
            .collect();
        assert_eq!(ore_counts, expected_ore);
        assert_eq!(ore_counts, vec![8, 4, 4, 8, 4, 6, 2, 6]);
        // Missing enchantment -> level 0 -> multiplier 1 -> base count 2.
        assert_eq!(
            ore_table.roll(&ctx_bare, &mut RandomSource::from_world_seed(1))[0].count(),
            2
        );

        let uniform_counts: Vec<u16> = (0..8_u64)
            .map(|seed| {
                let mut rng = RandomSource::from_world_seed(seed);
                uniform_table.roll(&ctx_luck3, &mut rng)[0].count()
            })
            .collect();
        let expected_uniform: Vec<u16> = (0..8_u64)
            .map(|seed| {
                let mut rng = RandomSource::from_world_seed(seed);
                1 + rng.next_int_bounded(7) as u16
            })
            .collect();
        assert_eq!(uniform_counts, expected_uniform);
        assert_eq!(uniform_counts, vec![7, 3, 4, 7, 4, 5, 1, 5]);
        // Missing enchantment -> level 0 -> bonus 0 -> base count 1.
        assert_eq!(
            uniform_table.roll(&ctx_bare, &mut RandomSource::from_world_seed(1))[0].count(),
            1
        );

        let binomial_counts: Vec<u16> = (0..8_u64)
            .map(|seed| {
                let mut rng = RandomSource::from_world_seed(seed);
                binomial_table.roll(&ctx_luck3, &mut rng)[0].count()
            })
            .collect();
        let expected_binomial: Vec<u16> = (0..8_u64)
            .map(|seed| {
                let mut rng = RandomSource::from_world_seed(seed);
                let mut successes = 0_u16;
                for _ in 0..6 {
                    if f64::from(rng.next_float()) < 0.5 {
                        successes += 1;
                    }
                }
                1 + successes
            })
            .collect();
        assert_eq!(binomial_counts, expected_binomial);
        assert_eq!(binomial_counts, vec![5, 5, 4, 4, 3, 4, 3, 2]);

        // Verify no extra +1 on binomial_with_bonus_count: with level 0, extra 3, p = 0.5,
        // mean over 4000 seeds must be ~ 1 + 3 * 0.5 = 2.5 (never ~3.5).
        let total: u32 = (0..4000_u64)
            .map(|seed| {
                let mut rng = RandomSource::from_world_seed(seed);
                u32::from(binomial_table.roll(&ctx_bare, &mut rng)[0].count())
            })
            .sum();
        let mean = f64::from(total) / 4000.0;
        assert!(
            (2.4..=2.6).contains(&mean),
            "expected mean ~2.5, got {mean}"
        );
    }

    #[test]
    fn limit_count_clamps_and_zero_or_negative_drops_nothing() {
        let clamped_table = load_single_table(
            "limit_clamp",
            "widget",
            r#"{
                "type": "minecraft:block",
                "pools": [{
                    "rolls": 1,
                    "entries": [
                        {
                            "type": "minecraft:item",
                            "name": "testns:low",
                            "modifier": [
                                {"type": "minecraft:set_count", "count": -3},
                                {"type": "minecraft:limit_count", "limit": {"min": 2, "max": 5}}
                            ]
                        }
                    ]
                }]
            }"#,
        );
        let high_table = load_single_table(
            "limit_high",
            "widget",
            r#"{
                "type": "minecraft:block",
                "pools": [{
                    "rolls": 1,
                    "entries": [
                        {
                            "type": "minecraft:item",
                            "name": "testns:high",
                            "modifier": [
                                {"type": "minecraft:set_count", "count": 9},
                                {"type": "minecraft:limit_count", "limit": {"min": 2, "max": 5}}
                            ]
                        }
                    ]
                }]
            }"#,
        );
        let zero_table = load_single_table(
            "limit_zero",
            "widget",
            r#"{
                "type": "minecraft:block",
                "pools": [{
                    "rolls": 1,
                    "entries": [
                        {
                            "type": "minecraft:item",
                            "name": "testns:zero",
                            "modifier": [
                                {"type": "minecraft:set_count", "count": -2},
                                {"type": "minecraft:limit_count", "limit": {"min": 0}}
                            ]
                        }
                    ]
                }]
            }"#,
        );

        let ctx = LootContext::for_block("testns:widget");
        let mut rng = RandomSource::from_world_seed(7);

        let low = clamped_table.roll(&ctx, &mut rng);
        assert_eq!(low.len(), 1);
        assert_eq!(low[0].count(), 2);

        let high = high_table.roll(&ctx, &mut rng);
        assert_eq!(high.len(), 1);
        assert_eq!(high[0].count(), 5);

        let zero = zero_table.roll(&ctx, &mut rng);
        assert!(zero.is_empty(), "count <= 0 must drop nothing");
    }

    #[test]
    fn explosion_decay_radius_four_keeps_around_quarter() {
        let table = load_single_table(
            "exp_decay",
            "widget",
            r#"{
                "type": "minecraft:block",
                "pools": [{
                    "rolls": 1,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "testns:widget",
                        "modifier": {"type": "minecraft:explosion_decay"}
                    }]
                }]
            }"#,
        );

        // No explosion radius -> no decay.
        let bare = LootContext::for_block("testns:widget");
        assert_eq!(
            table.roll(&bare, &mut RandomSource::from_world_seed(1))[0].count(),
            1
        );

        let exploded = LootContext {
            block_state: ("testns:widget".to_owned(), BTreeMap::new()),
            tool: None,
            explosion_radius: Some(4.0),
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let mut kept = 0_u32;
        for seed in 0..10_000_u64 {
            let mut rng = RandomSource::from_world_seed(seed);
            let drops = table.roll(&exploded, &mut rng);
            assert!(drops.len() <= 1);
            kept += drops.len() as u32;
        }
        assert!(
            (2300..=2700).contains(&kept),
            "expected 2300..=2700 kept items at radius 4, got {kept}"
        );
    }

    #[test]
    fn conditions_tags_modifiers_notes_and_item_defaults() {
        let root = scratch_root("full_eval");
        write(
            &root.join("data/testns/tags/item/cutters.json"),
            r#"{"values": ["testns:shears_x", "testns:shears_y"]}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/synthetic_widget.json"),
            r#"{"components": {"minecraft:max_stack_size": 16}}"#,
        );
        write(
            &root.join("data/testns/loot_table/blocks/gadget.json"),
            r##"{
                "type": "minecraft:block",
                "pools": [
                    {
                        "rolls": 2,
                        "condition": {
                            "type": "minecraft:all_of",
                            "terms": [
                                {
                                    "type": "minecraft:match_block",
                                    "blocks": "testns:gadget",
                                    "state": {"active": "true"}
                                },
                                {
                                    "type": "minecraft:inverted",
                                    "term": {
                                        "type": "minecraft:entity_properties",
                                        "entity": "this",
                                        "predicate": {}
                                    }
                                }
                            ]
                        },
                        "entries": [{
                            "type": "minecraft:alternatives",
                            "children": [
                                {
                                    "type": "minecraft:dynamic",
                                    "name": "testns:bits",
                                    "condition": {
                                        "type": "minecraft:match_block",
                                        "blocks": "testns:gadget",
                                        "state": {"cracked": "true"}
                                    }
                                },
                                {
                                    "type": "minecraft:item",
                                    "name": "minecraft:synthetic_widget",
                                    "condition": {
                                        "type": "minecraft:match_tool",
                                        "predicate": {"items": "#testns:cutters"}
                                    },
                                    "modifier": [
                                        {"type": "minecraft:set_count", "count": 10},
                                        {
                                            "type": "minecraft:copy_components",
                                            "source": "block_entity",
                                            "include": ["testns:custom"]
                                        },
                                        {
                                            "type": "minecraft:copy_state",
                                            "block": "testns:gadget",
                                            "properties": ["active"]
                                        }
                                    ]
                                },
                                {
                                    "type": "minecraft:item",
                                    "name": "testns:crumb",
                                    "modifier": [
                                        {"type": "minecraft:set_count", "count": {"type": "minecraft:uniform", "min": -2, "max": 2}},
                                        {
                                            "type": "minecraft:set_count",
                                            "add": true,
                                            "condition": {
                                                "type": "minecraft:match_block",
                                                "blocks": "testns:gadget",
                                                "state": {"bonus": "true"}
                                            },
                                            "count": 3
                                        }
                                    ]
                                }
                            ]
                        }]
                    },
                    {
                        "rolls": 1,
                        "condition": {"type": "testns:unsupported_cond"},
                        "entries": [{"type": "minecraft:item", "name": "testns:never"}]
                    }
                ]
            }"##,
        );

        let tables = LootTables::load_blocks(&root).expect("load full eval");
        assert!(tables.item_defaults().is_some());
        let gadget = tables.get("testns:gadget").expect("gadget");

        // Cracked state hits dynamic entry twice (rolls = 2): no items invented, two notes emitted.
        let mut props_cracked = BTreeMap::new();
        props_cracked.insert("active".to_owned(), "true".to_owned());
        props_cracked.insert("cracked".to_owned(), "true".to_owned());
        let ctx_cracked = LootContext {
            block_state: ("testns:gadget".to_owned(), props_cracked),
            tool: None,
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let mut rng = RandomSource::from_world_seed(99);
        let cracked_roll = gadget.roll_with_notes(&ctx_cracked, &mut rng);
        assert!(cracked_roll.items.is_empty());
        assert_eq!(
            cracked_roll.notes,
            vec![
                LootNote::DynamicNotApplied {
                    name: "testns:bits".to_owned()
                },
                LootNote::DynamicNotApplied {
                    name: "testns:bits".to_owned()
                },
            ]
        );

        // Tag-matched tool drops minecraft:synthetic_widget with ItemDefaults max_stack = 16 and component notes.
        let mut props_active = BTreeMap::new();
        props_active.insert("active".to_owned(), "true".to_owned());
        let ctx_cutter = LootContext {
            block_state: ("testns:gadget".to_owned(), props_active),
            tool: Some(ToolContext {
                item: "testns:shears_x".to_owned(),
                enchantments: BTreeMap::new(),
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let cutter_roll = tables
            .roll("testns:gadget", &ctx_cutter, &mut rng)
            .expect("table exists");
        assert_eq!(cutter_roll.items.len(), 2);
        assert_eq!(cutter_roll.items[0].item(), "minecraft:synthetic_widget");
        assert_eq!(cutter_roll.items[0].count(), 10);
        assert_eq!(cutter_roll.items[0].max_stack(), 16);
        assert_eq!(cutter_roll.notes.len(), 4);
        assert!(matches!(
            &cutter_roll.notes[0],
            LootNote::CopyComponentsNotApplied { item, source, .. }
                if item == "minecraft:synthetic_widget" && source == "block_entity"
        ));
        assert!(matches!(
            &cutter_roll.notes[1],
            LootNote::CopyStateNotApplied { item, block, properties }
                if item == "minecraft:synthetic_widget"
                    && block == "testns:gadget"
                    && properties == &vec!["active".to_owned()]
        ));

        // Inactive block state fails match_block -> empty drops; unsupported pool condition also fails.
        let ctx_inactive = LootContext::for_block("testns:gadget");
        assert!(gadget.roll(&ctx_inactive, &mut rng).is_empty());

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn binomial_provider_random_chance_any_of_and_stack_splitting() {
        let table = load_single_table(
            "split_and_chance",
            "widget",
            r#"{
                "type": "minecraft:block",
                "pools": [
                    {
                        "rolls": 1,
                        "condition": {
                            "type": "minecraft:any_of",
                            "terms": [
                                {"type": "testns:missing_cond"},
                                {
                                    "type": "minecraft:match_tool",
                                    "predicate": {
                                        "predicates": {
                                            "minecraft:enchantments": [
                                                {"enchantments": "testns:delicate", "levels": {"min": 1, "max": 2}}
                                            ]
                                        }
                                    }
                                }
                            ]
                        },
                        "entries": [
                            {
                                "type": "minecraft:item",
                                "name": "minecraft:snowball",
                                "modifier": {"type": "minecraft:set_count", "count": 20}
                            }
                        ]
                    },
                    {
                        "rolls": 1,
                        "entries": [
                            {
                                "type": "minecraft:item",
                                "name": "testns:seed",
                                "condition": {"type": "minecraft:random_chance", "chance": 0.25},
                                "modifier": {
                                    "type": "minecraft:set_count",
                                    "count": {"type": "minecraft:binomial", "n": 4, "p": 0.5}
                                }
                            }
                        ]
                    }
                ]
            }"#,
        );

        // No ItemDefaults present -> minecraft:snowball falls back to default_max_stack (16),
        // splitting 20 items into 16 + 4.
        let mut enchs = BTreeMap::new();
        enchs.insert("testns:delicate".to_owned(), 2);
        let ctx_delicate = LootContext {
            block_state: ("testns:widget".to_owned(), BTreeMap::new()),
            tool: Some(ToolContext {
                item: "testns:wand".to_owned(),
                enchantments: enchs,
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let mut rng = RandomSource::from_world_seed(123);
        let drops = table.roll(&ctx_delicate, &mut rng);
        assert!(drops.len() >= 2);
        assert_eq!(drops[0].item(), "minecraft:snowball");
        assert_eq!(drops[0].count(), 16);
        assert_eq!(drops[0].max_stack(), 16);
        assert_eq!(drops[1].item(), "minecraft:snowball");
        assert_eq!(drops[1].count(), 4);
        assert_eq!(drops[1].max_stack(), 16);

        // Enchantment level 0 is treated as absent -> fails min: 1.
        let mut zero_enchs = BTreeMap::new();
        zero_enchs.insert("testns:delicate".to_owned(), 0);
        let ctx_zero = LootContext {
            block_state: ("testns:widget".to_owned(), BTreeMap::new()),
            tool: Some(ToolContext {
                item: "testns:wand".to_owned(),
                enchantments: zero_enchs,
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let mut seed_hits = 0_u32;
        let mut seed_total = 0_u32;
        let mut r = RandomSource::from_world_seed(777);
        for _ in 0..4000_u32 {
            let res = table.roll(&ctx_zero, &mut r);
            assert!(res.iter().all(|s| s.item() == "testns:seed"));
            if let Some(s) = res.first() {
                seed_hits += 1;
                seed_total += u32::from(s.count());
                assert!(s.count() <= 4);
            }
        }
        // 0.25 pass rate * (1 - 0.5^4) non-zero binomial rate = 0.234375 -> ~938 out of 4000.
        assert!((830..=1050).contains(&seed_hits), "seed_hits={seed_hits}");
        let conditional_mean = f64::from(seed_total) / f64::from(seed_hits);
        // Conditional mean of Binomial(4, 0.5) given > 0 is 2.0 / (1 - 0.0625) = 2.133...
        assert!(
            (1.95..=2.30).contains(&conditional_mean),
            "conditional_mean={conditional_mean}"
        );
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored loads_all_vanilla_block_loot_tables`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn loads_all_vanilla_block_loot_tables() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let tables = LootTables::load_blocks(&root).expect("load vanilla block loot tables");
        assert_eq!(tables.len(), 1201);
        assert!(tables.get("minecraft:stone").is_some());
        assert!(tables.get("stone").is_some());
        assert!(tables.get("minecraft:blocks/stone").is_some());

        let unsupported = tables.unsupported();
        assert!(
            unsupported.is_empty(),
            "expected no unsupported nodes across 1201 block loot tables, got {unsupported:?}"
        );
    }

    fn vanilla_data_root() -> PathBuf {
        std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root")
    }

    fn read_vanilla_block_table_json(root: &Path, block_name: &str) -> Value {
        let path = root
            .join("data")
            .join("minecraft")
            .join("loot_table")
            .join("blocks")
            .join(format!("{block_name}.json"));
        read_json(&path).expect("read vanilla block loot table json")
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_stone_bare_and_silk_touch`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn vanilla_stone_bare_and_silk_touch() {
        let root = vanilla_data_root();
        let tables = LootTables::load_blocks(&root).expect("load vanilla block loot tables");
        let stone = tables.get("minecraft:stone").expect("stone loot table");
        let raw = read_vanilla_block_table_json(&root, "stone");
        let children = raw["pools"][0]["entries"][0]["children"]
            .as_array()
            .expect("stone alternatives children");
        let silk_item = children[0]["name"].as_str().expect("silk stone drop");
        let bare_item = children[1]["name"].as_str().expect("bare stone drop");

        let bare_ctx = LootContext::for_block("minecraft:stone");
        let mut rng = RandomSource::from_world_seed(1);
        let bare_drops = stone.roll(&bare_ctx, &mut rng);
        assert_eq!(bare_drops.len(), 1);
        assert_eq!(bare_drops[0].item(), bare_item);
        assert_eq!(bare_drops[0].count(), 1);

        let mut silk_enchs = BTreeMap::new();
        silk_enchs.insert("minecraft:silk_touch".to_owned(), 1);
        let silk_ctx = LootContext {
            block_state: ("minecraft:stone".to_owned(), BTreeMap::new()),
            tool: Some(ToolContext {
                item: "minecraft:diamond_pickaxe".to_owned(),
                enchantments: silk_enchs,
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let silk_drops = stone.roll(&silk_ctx, &mut rng);
        assert_eq!(silk_drops.len(), 1);
        assert_eq!(silk_drops[0].item(), silk_item);
        assert_eq!(silk_drops[0].count(), 1);
    }

    /// Verifies `minecraft:diamond_ore` drops without enchantments and with Fortune III.
    ///
    /// Mirrors `minecraft.wiki` `Fortune` (revision 3864999):
    /// `"The average drops multiplier is 1/(2+level) + (1+level)/2, so Fortune I gives 1.33 × drops on average, Fortune II gives 1.75 × drops on average, and Fortune III gives 2.2 × drops on average."`
    ///
    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_diamond_ore_no_enchant_and_fortune_three`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn vanilla_diamond_ore_no_enchant_and_fortune_three() {
        let root = vanilla_data_root();
        let tables = LootTables::load_blocks(&root).expect("load vanilla block loot tables");
        let diamond_ore = tables
            .get("minecraft:diamond_ore")
            .expect("diamond_ore loot table");
        let raw = read_vanilla_block_table_json(&root, "diamond_ore");
        let expected_item = raw["pools"][0]["entries"][0]["children"][1]["name"]
            .as_str()
            .expect("diamond_ore normal drop");

        let bare_pick = LootContext {
            block_state: ("minecraft:diamond_ore".to_owned(), BTreeMap::new()),
            tool: Some(ToolContext {
                item: "minecraft:iron_pickaxe".to_owned(),
                enchantments: BTreeMap::new(),
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        for seed in 0..100_u64 {
            let mut rng = RandomSource::from_world_seed(seed);
            let drops = diamond_ore.roll(&bare_pick, &mut rng);
            assert_eq!(drops.len(), 1);
            assert_eq!(drops[0].item(), expected_item);
            assert_eq!(drops[0].count(), 1);
        }

        let mut fortune_enchs = BTreeMap::new();
        fortune_enchs.insert("minecraft:fortune".to_owned(), 3);
        let fortune_ctx = LootContext {
            block_state: ("minecraft:diamond_ore".to_owned(), BTreeMap::new()),
            tool: Some(ToolContext {
                item: "minecraft:diamond_pickaxe".to_owned(),
                enchantments: fortune_enchs,
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let total: u64 = (0..10_000_u64)
            .map(|seed| {
                let mut rng = RandomSource::from_world_seed(seed);
                let drops = diamond_ore.roll(&fortune_ctx, &mut rng);
                assert_eq!(drops.len(), 1);
                assert_eq!(drops[0].item(), expected_item);
                u64::from(drops[0].count())
            })
            .sum();
        let mean = total as f64 / 10_000.0;
        assert!(
            (2.1..=2.3).contains(&mean),
            "expected diamond_ore Fortune III mean in 2.1..=2.3 (wiki 2.2), got {mean}"
        );
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_gravel_flint_rates_match_table_bonus`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn vanilla_gravel_flint_rates_match_table_bonus() {
        let root = vanilla_data_root();
        let tables = LootTables::load_blocks(&root).expect("load vanilla block loot tables");
        let gravel = tables.get("minecraft:gravel").expect("gravel loot table");
        let raw = read_vanilla_block_table_json(&root, "gravel");
        let inner_alt = &raw["pools"][0]["entries"][0]["children"][1];
        let flint_entry = &inner_alt["children"][0];
        let gravel_entry = &inner_alt["children"][1];
        let flint_item = flint_entry["name"].as_str().expect("flint name");
        let gravel_item = gravel_entry["name"].as_str().expect("gravel name");
        let chances: Vec<f64> = flint_entry["condition"]["chances"]
            .as_array()
            .expect("table_bonus chances array")
            .iter()
            .map(|v| v.as_f64().expect("chance float"))
            .collect();
        assert_eq!(chances.len(), 4);

        for (fortune_level, expected_rate) in chances.iter().copied().enumerate() {
            let mut enchantments = BTreeMap::new();
            if fortune_level > 0 {
                enchantments.insert("minecraft:fortune".to_owned(), fortune_level as u8);
            }
            let ctx = LootContext {
                block_state: ("minecraft:gravel".to_owned(), BTreeMap::new()),
                tool: Some(ToolContext {
                    item: "minecraft:diamond_shovel".to_owned(),
                    enchantments,
                }),
                explosion_radius: None,
                neighbor_blocks: None,
                ..LootContext::default()
            };

            let mut flint_count = 0_u64;
            for seed in 0..20_000_u64 {
                let mut rng = RandomSource::from_world_seed(seed);
                let drops = gravel.roll(&ctx, &mut rng);
                assert_eq!(drops.len(), 1);
                assert_eq!(drops[0].count(), 1);
                if drops[0].item() == flint_item {
                    flint_count += 1;
                } else {
                    assert_eq!(drops[0].item(), gravel_item);
                }
            }
            let observed_rate = flint_count as f64 / 20_000.0;
            assert!(
                (observed_rate - expected_rate).abs() <= 0.01,
                "fortune {fortune_level}: expected {expected_rate} +- 0.01, got {observed_rate}"
            );
        }
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_oak_leaves_shears_and_bare_hand`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn vanilla_oak_leaves_shears_and_bare_hand() {
        let root = vanilla_data_root();
        let tables = LootTables::load_blocks(&root).expect("load vanilla block loot tables");
        let oak_leaves = tables
            .get("minecraft:oak_leaves")
            .expect("oak_leaves loot table");
        let raw = read_vanilla_block_table_json(&root, "oak_leaves");
        let leaves_item = raw["pools"][0]["entries"][0]["children"][0]["name"]
            .as_str()
            .expect("oak_leaves drop name");

        let shears_ctx = LootContext {
            block_state: ("minecraft:oak_leaves".to_owned(), BTreeMap::new()),
            tool: Some(ToolContext {
                item: "minecraft:shears".to_owned(),
                enchantments: BTreeMap::new(),
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let mut rng = RandomSource::from_world_seed(7);
        let sheared = oak_leaves.roll(&shears_ctx, &mut rng);
        assert_eq!(sheared.len(), 1);
        assert_eq!(sheared[0].item(), leaves_item);
        assert_eq!(sheared[0].count(), 1);

        let bare_ctx = LootContext::for_block("minecraft:oak_leaves");
        for seed in 0..500_u64 {
            let mut bare_rng = RandomSource::from_world_seed(seed);
            let bare_drops = oak_leaves.roll(&bare_ctx, &mut bare_rng);
            assert!(
                bare_drops.iter().all(|stack| stack.item() != leaves_item),
                "oak_leaves must never drop leaves without shears/silk_touch (seed {seed})"
            );
        }
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_stone_explosion_radius_four`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn vanilla_stone_explosion_radius_four() {
        let root = vanilla_data_root();
        let tables = LootTables::load_blocks(&root).expect("load vanilla block loot tables");
        let stone = tables.get("minecraft:stone").expect("stone loot table");
        let raw = read_vanilla_block_table_json(&root, "stone");
        let cobblestone_item = raw["pools"][0]["entries"][0]["children"][1]["name"]
            .as_str()
            .expect("cobblestone drop name");

        let ctx = LootContext {
            block_state: ("minecraft:stone".to_owned(), BTreeMap::new()),
            tool: None,
            explosion_radius: Some(4.0),
            neighbor_blocks: None,
            ..LootContext::default()
        };
        let mut drops_count = 0_u32;
        for seed in 0..10_000_u64 {
            let mut rng = RandomSource::from_world_seed(seed);
            let drops = stone.roll(&ctx, &mut rng);
            if let Some(first) = drops.first() {
                assert_eq!(drops.len(), 1);
                assert_eq!(first.item(), cobblestone_item);
                assert_eq!(first.count(), 1);
                drops_count += 1;
            }
        }
        assert!(
            (2300..=2700).contains(&drops_count),
            "expected 2300..=2700 cobblestone drops at radius 4, got {drops_count}"
        );
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_all_tables_roll_without_panic_in_empty_context`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn vanilla_all_tables_roll_without_panic_in_empty_context() {
        let root = vanilla_data_root();
        let tables = LootTables::load_blocks(&root).expect("load vanilla block loot tables");
        assert_eq!(tables.len(), 1201);
        let item_defaults = tables
            .item_defaults()
            .expect("vanilla data root includes ItemDefaults reports");

        // Verify ItemDefaults max_stack_size is applied (for example shulker box = 1, sign = 16).
        let shulker = tables
            .get("minecraft:white_shulker_box")
            .expect("white_shulker_box");
        let shulker_roll = shulker.roll_with_notes(
            &LootContext::for_block("minecraft:white_shulker_box"),
            &mut RandomSource::from_world_seed(1),
        );
        assert_eq!(shulker_roll.items.len(), 1);
        assert_eq!(shulker_roll.items[0].max_stack(), 1);
        assert!(!shulker_roll.notes.is_empty());

        let empty_ctx = LootContext::default();
        let mut rng = RandomSource::from_world_seed(202_603);
        let mut returned_nothing = 0_usize;
        for (_id, table) in tables.iter() {
            let outcome = table.roll_with_notes(&empty_ctx, &mut rng);
            for stack in &outcome.items {
                let expected_max = u16::from(
                    item_defaults
                        .max_stack_size(stack.item())
                        .expect("every vanilla block drop exists in ItemDefaults"),
                );
                assert_eq!(stack.max_stack(), expected_max);
            }
            if outcome.items.is_empty() {
                returned_nothing += 1;
            }
        }
        println!(
            "rolled {} vanilla tables in empty context; {} returned nothing",
            tables.len(),
            returned_nothing
        );
    }

    #[test]
    fn location_check_evaluates_offsets_block_state_and_missing_lookup() {
        let root = scratch_root("location_check");
        write(
            &root.join("data/testns/tags/block/double_tops.json"),
            r#"{"values": ["testns:tall_plant"]}"#,
        );
        write(
            &root.join("data/testns/loot_table/blocks/tall_plant.json"),
            r##"{
                "type": "minecraft:block",
                "pools": [
                    {
                        "rolls": 1,
                        "condition": {
                            "type": "minecraft:all_of",
                            "terms": [
                                {
                                    "type": "minecraft:match_block",
                                    "blocks": "testns:tall_plant",
                                    "state": {"half": "lower"}
                                },
                                {
                                    "type": "minecraft:location_check",
                                    "offsetX": 1,
                                    "offsetY": 1,
                                    "offsetZ": -2,
                                    "predicate": {
                                        "block": {
                                            "blocks": "#testns:double_tops",
                                            "state": {"half": "upper"}
                                        }
                                    }
                                }
                            ]
                        },
                        "entries": [
                            {
                                "type": "minecraft:item",
                                "name": "testns:frond",
                                "modifier": {"type": "minecraft:set_count", "count": 2}
                            }
                        ]
                    }
                ]
            }"##,
        );

        let tables = LootTables::load_blocks(&root).expect("load synthetic location_check");
        assert!(tables.unsupported().is_empty());
        let plant = tables.get("testns:tall_plant").expect("tall_plant table");

        let mut lower_props = BTreeMap::new();
        lower_props.insert("half".to_owned(), "lower".to_owned());

        // No neighbor lookup -> location_check evaluates to false.
        let no_lookup_ctx = LootContext {
            block_state: ("testns:tall_plant".to_owned(), lower_props.clone()),
            tool: None,
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        assert!(
            plant
                .roll(&no_lookup_ctx, &mut RandomSource::from_world_seed(1))
                .is_empty()
        );

        // Matching neighbor at exact offset (1, 1, -2) -> drops 2 testns:frond.
        let matching_ctx = LootContext {
            block_state: ("testns:tall_plant".to_owned(), lower_props.clone()),
            tool: None,
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        }
        .with_block_lookup(|dx, dy, dz| {
            if (dx, dy, dz) == (1, 1, -2) {
                let mut props = BTreeMap::new();
                props.insert("half".to_owned(), "upper".to_owned());
                Some(("testns:tall_plant".to_owned(), props))
            } else {
                None
            }
        });
        let drops = plant.roll(&matching_ctx, &mut RandomSource::from_world_seed(1));
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item(), "testns:frond");
        assert_eq!(drops[0].count(), 2);

        // Wrong state property at offset -> false.
        let wrong_state_ctx = LootContext {
            block_state: ("testns:tall_plant".to_owned(), lower_props.clone()),
            tool: None,
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        }
        .with_block_lookup(|dx, dy, dz| {
            if (dx, dy, dz) == (1, 1, -2) {
                let mut props = BTreeMap::new();
                props.insert("half".to_owned(), "lower".to_owned());
                Some(("testns:tall_plant".to_owned(), props))
            } else {
                None
            }
        });
        assert!(
            plant
                .roll(&wrong_state_ctx, &mut RandomSource::from_world_seed(1))
                .is_empty()
        );

        // Wrong block id at offset -> false.
        let wrong_block_ctx = LootContext {
            block_state: ("testns:tall_plant".to_owned(), lower_props),
            tool: None,
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        }
        .with_block_lookup(|dx, dy, dz| {
            if (dx, dy, dz) == (1, 1, -2) {
                let mut props = BTreeMap::new();
                props.insert("half".to_owned(), "upper".to_owned());
                Some(("testns:other".to_owned(), props))
            } else {
                None
            }
        });
        assert!(
            plant
                .roll(&wrong_block_ctx, &mut RandomSource::from_world_seed(1))
                .is_empty()
        );

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn location_check_unsupported_predicate_fields_stay_unsupported_and_false() {
        let root = scratch_root("location_check_unsupported");
        write(
            &root.join("data/testns/loot_table/blocks/biome_plant.json"),
            r#"{
                "type": "minecraft:block",
                "pools": [
                    {
                        "rolls": 1,
                        "condition": {
                            "type": "minecraft:location_check",
                            "offsetY": 1,
                            "predicate": {
                                "block": {"blocks": "testns:top"},
                                "biomes": "minecraft:plains"
                            }
                        },
                        "entries": [{"type": "minecraft:item", "name": "testns:drop"}]
                    },
                    {
                        "rolls": 1,
                        "condition": {
                            "type": "minecraft:location_check",
                            "predicate": {
                                "block": {"blocks": "testns:top", "nbt": "{}"}
                            }
                        },
                        "entries": [{"type": "minecraft:item", "name": "testns:drop"}]
                    }
                ]
            }"#,
        );

        let tables = LootTables::load_blocks(&root).expect("load unsupported location_check");
        assert_eq!(
            tables.unsupported(),
            vec![
                ("testns:blocks/biome_plant", "minecraft:location_check"),
                ("testns:blocks/biome_plant", "minecraft:location_check"),
            ]
        );

        let table = tables.get("testns:biome_plant").expect("table present");
        let ctx = LootContext::for_block("testns:biome_plant")
            .with_block_lookup(|_, _, _| Some(("testns:top".to_owned(), BTreeMap::new())));
        assert!(
            table
                .roll(&ctx, &mut RandomSource::from_world_seed(5))
                .is_empty()
        );

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn location_check_rejects_malformed_shapes_naming_file() {
        let cases = [
            (
                "bad_offset.json",
                r#"{"type": "minecraft:block", "pools": [{"rolls": 1, "condition": {"type": "minecraft:location_check", "offsetY": "up", "predicate": {}}, "entries": []}]}"#,
            ),
            (
                "missing_predicate.json",
                r#"{"type": "minecraft:block", "pools": [{"rolls": 1, "condition": {"type": "minecraft:location_check"}, "entries": []}]}"#,
            ),
            (
                "bad_block_pred.json",
                r#"{"type": "minecraft:block", "pools": [{"rolls": 1, "condition": {"type": "minecraft:location_check", "predicate": {"block": "bad"}}, "entries": []}]}"#,
            ),
            (
                "unknown_loc_key.json",
                r#"{"type": "minecraft:block", "pools": [{"rolls": 1, "condition": {"type": "minecraft:location_check", "extra": 1, "predicate": {}}, "entries": []}]}"#,
            ),
        ];

        for (name, body) in cases {
            let root = scratch_root(name);
            write(&root.join("data/testns/loot_table/blocks").join(name), body);
            let err = LootTables::load_blocks(&root)
                .expect_err("malformed location_check must fail")
                .to_string();
            assert!(err.contains(name), "error must name {name}: {err}");
            fs::remove_dir_all(&root).expect("cleanup");
        }
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_double_plant_location_check_and_unsupported_empty`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn vanilla_double_plant_location_check_and_unsupported_empty() {
        let root = vanilla_data_root();
        let tables = LootTables::load_blocks(&root).expect("load vanilla block loot tables");
        assert_eq!(tables.len(), 1201);
        assert!(
            tables.unsupported().is_empty(),
            "all 1201 vanilla block loot tables must have empty unsupported(), got {:?}",
            tables.unsupported()
        );

        let tall_grass = tables
            .get("minecraft:tall_grass")
            .expect("tall_grass loot table");
        let raw_tall_grass = read_vanilla_block_table_json(&root, "tall_grass");
        let expected_seed_chance = raw_tall_grass["pools"][0]["entries"][0]["children"][1]
            ["condition"]["terms"][1]["chance"]
            .as_f64()
            .expect("tall_grass random_chance from file");

        let mut lower_props = BTreeMap::new();
        lower_props.insert("half".to_owned(), "lower".to_owned());

        // Lower half of tall_grass with upper half above and shears -> 2 short_grass.
        let shears_with_upper = LootContext {
            block_state: ("minecraft:tall_grass".to_owned(), lower_props.clone()),
            tool: Some(ToolContext {
                item: "minecraft:shears".to_owned(),
                enchantments: BTreeMap::new(),
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        }
        .with_block_lookup(|dx, dy, dz| {
            if (dx, dy, dz) == (0, 1, 0) {
                let mut upper = BTreeMap::new();
                upper.insert("half".to_owned(), "upper".to_owned());
                Some(("minecraft:tall_grass".to_owned(), upper))
            } else {
                None
            }
        });
        let sheared_drops =
            tall_grass.roll(&shears_with_upper, &mut RandomSource::from_world_seed(1));
        assert_eq!(sheared_drops.len(), 1);
        assert_eq!(sheared_drops[0].item(), "minecraft:short_grass");
        assert_eq!(sheared_drops[0].count(), 2);

        // Same without the upper half above -> nothing (both air above and missing lookup).
        let shears_without_upper = LootContext {
            block_state: ("minecraft:tall_grass".to_owned(), lower_props.clone()),
            tool: Some(ToolContext {
                item: "minecraft:shears".to_owned(),
                enchantments: BTreeMap::new(),
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        }
        .with_block_lookup(|dx, dy, dz| {
            if (dx, dy, dz) == (0, 1, 0) {
                Some(("minecraft:air".to_owned(), BTreeMap::new()))
            } else {
                None
            }
        });
        assert!(
            tall_grass
                .roll(&shears_without_upper, &mut RandomSource::from_world_seed(1))
                .is_empty()
        );
        let shears_no_lookup = LootContext {
            block_state: ("minecraft:tall_grass".to_owned(), lower_props.clone()),
            tool: Some(ToolContext {
                item: "minecraft:shears".to_owned(),
                enchantments: BTreeMap::new(),
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        };
        assert!(
            tall_grass
                .roll(&shears_no_lookup, &mut RandomSource::from_world_seed(1))
                .is_empty()
        );

        // Without shears over 20_000 seeds -> wheat_seeds rate within +-0.01 of random_chance.
        let bare_with_upper = LootContext {
            block_state: ("minecraft:tall_grass".to_owned(), lower_props),
            tool: None,
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        }
        .with_block_lookup(|dx, dy, dz| {
            if (dx, dy, dz) == (0, 1, 0) {
                let mut upper = BTreeMap::new();
                upper.insert("half".to_owned(), "upper".to_owned());
                Some(("minecraft:tall_grass".to_owned(), upper))
            } else {
                None
            }
        });
        let mut seed_drops = 0_u64;
        for seed in 0..20_000_u64 {
            let mut rng = RandomSource::from_world_seed(seed);
            let drops = tall_grass.roll(&bare_with_upper, &mut rng);
            if let Some(first) = drops.first() {
                assert_eq!(drops.len(), 1);
                assert_eq!(first.item(), "minecraft:wheat_seeds");
                assert_eq!(first.count(), 1);
                seed_drops += 1;
            }
        }
        let observed_rate = seed_drops as f64 / 20_000.0;
        assert!(
            (observed_rate - expected_seed_chance).abs() <= 0.01,
            "expected wheat_seeds rate {expected_seed_chance} +- 0.01, got {observed_rate}"
        );

        // Also check upper half of large_fern with lower half below and shears -> 2 fern.
        let large_fern = tables
            .get("minecraft:large_fern")
            .expect("large_fern loot table");
        let mut upper_props = BTreeMap::new();
        upper_props.insert("half".to_owned(), "upper".to_owned());
        let fern_ctx = LootContext {
            block_state: ("minecraft:large_fern".to_owned(), upper_props),
            tool: Some(ToolContext {
                item: "minecraft:shears".to_owned(),
                enchantments: BTreeMap::new(),
            }),
            explosion_radius: None,
            neighbor_blocks: None,
            ..LootContext::default()
        }
        .with_block_lookup(|dx, dy, dz| {
            if (dx, dy, dz) == (0, -1, 0) {
                let mut lower = BTreeMap::new();
                lower.insert("half".to_owned(), "lower".to_owned());
                Some(("minecraft:large_fern".to_owned(), lower))
            } else {
                None
            }
        });
        let fern_drops = large_fern.roll(&fern_ctx, &mut RandomSource::from_world_seed(1));
        assert_eq!(fern_drops.len(), 1);
        assert_eq!(fern_drops[0].item(), "minecraft:fern");
        assert_eq!(fern_drops[0].count(), 2);
    }

    #[test]
    fn parses_entity_loot_entries_conditions_modifiers_and_resolves_references() {
        let root = scratch_root("entity_model");
        write(
            &root.join("data/testns/loot_table/gameplay/bonus_fish.json"),
            r#"{
                "type": "minecraft:fishing",
                "pools": [{
                    "rolls": 1,
                    "entries": [
                        {"type": "minecraft:item", "name": "testns:cod", "weight": 3},
                        {"type": "minecraft:empty", "weight": 2}
                    ]
                }]
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/entities/sheep/red.json"),
            r#"{
                "type": "minecraft:entity",
                "pools": [{
                    "rolls": 1,
                    "entries": [{"type": "minecraft:item", "name": "testns:red_wool"}]
                }]
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/entities/mob.json"),
            r##"{
                "type": "minecraft:entity",
                "pools": [
                    {
                        "rolls": 1,
                        "condition": {
                            "type": "minecraft:all_of",
                            "terms": [
                                {"type": "minecraft:killed_by_player"},
                                {
                                    "type": "minecraft:random_chance_with_enchanted_bonus",
                                    "enchantment": "minecraft:looting",
                                    "unenchanted_chance": 0.025,
                                    "enchanted_chance": {
                                        "type": "minecraft:linear",
                                        "base": 0.035,
                                        "per_level_above_first": 0.01
                                    }
                                }
                            ]
                        },
                        "entries": [
                            {
                                "type": "minecraft:item",
                                "name": "testns:raw_meat",
                                "weight": 2,
                                "modifier": [
                                    {"type": "minecraft:set_count", "count": {"type": "minecraft:uniform", "min": 1, "max": 3}},
                                    {
                                        "type": "minecraft:enchanted_count_increase",
                                        "enchantment": "minecraft:looting",
                                        "count": {"type": "minecraft:uniform", "min": 0.0, "max": 1.0},
                                        "limit": 4
                                    },
                                    {
                                        "type": "minecraft:furnace_smelt",
                                        "condition": {
                                            "type": "minecraft:any_of",
                                            "terms": [
                                                {"type": "minecraft:entity_properties", "entity": "this", "predicate": {"minecraft:flags": {"is_on_fire": true}}},
                                                {"type": "minecraft:entity_properties", "entity": "direct_attacker", "predicate": {"minecraft:equipment": {"mainhand": {"predicates": {"minecraft:enchantments": [{"enchantments": "#minecraft:smelts_loot"}]}}}}}
                                            ]
                                        }
                                    }
                                ]
                            },
                            {
                                "type": "minecraft:item",
                                "name": "minecraft:tipped_arrow",
                                "modifier": {"type": "minecraft:set_potion", "id": "minecraft:poison"}
                            },
                            {
                                "type": "minecraft:tag",
                                "items": "#testns:discs",
                                "expand": true
                            },
                            {
                                "type": "minecraft:loot_table",
                                "value": "testns:gameplay/bonus_fish",
                                "condition": {
                                    "type": "minecraft:damage_source_properties",
                                    "predicate": {
                                        "direct_entity": {"minecraft:entity_type": "minecraft:fireball"},
                                        "source_entity": {"minecraft:entity_type": "minecraft:frog", "minecraft:components": {"minecraft:frog/variant": "minecraft:warm"}},
                                        "tags": [{"id": "#minecraft:is_projectile", "expected": true}]
                                    }
                                }
                            },
                            {
                                "type": "minecraft:loot_table",
                                "value": {
                                    "pools": [{
                                        "rolls": 1,
                                        "entries": [{"type": "minecraft:empty"}]
                                    }]
                                }
                            },
                            {
                                "type": "minecraft:alternatives",
                                "children": [{
                                    "type": "minecraft:loot_table",
                                    "value": "testns:entities/sheep/red",
                                    "condition": {
                                        "type": "minecraft:entity_properties",
                                        "entity": "this",
                                        "predicate": {
                                            "minecraft:components": {"minecraft:sheep/color": "red"},
                                            "minecraft:type_specific/sheep": {"sheared": false},
                                            "minecraft:type_specific/cube_mob": {"size": {"min": 2}},
                                            "minecraft:type_specific/raider": {"is_captain": true},
                                            "minecraft:flags": {"is_baby": true},
                                            "minecraft:vehicle": {"minecraft:entity_type": "minecraft:chicken"}
                                        }
                                    }
                                }]
                            }
                        ]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:ominous_bottle",
                            "modifier": {"type": "minecraft:set_ominous_bottle_amplifier", "amplifier": {"type": "minecraft:uniform", "min": 0, "max": 4}}
                        }]
                    }
                ]
            }"##,
        );

        let tables = LootTables::load_entities(&root).expect("load entity tables");
        assert_eq!(tables.len(), 2);
        assert!(tables.get_entity("testns:mob").is_some());
        assert!(tables.get_entity("testns:entities/mob").is_some());
        assert!(tables.get_entity("testns:sheep/red").is_some());
        assert!(tables.get_entity("testns:entities/sheep/red").is_some());
        assert_eq!(
            tables.unsupported(),
            vec![(
                "testns:entities/mob",
                "minecraft:set_ominous_bottle_amplifier"
            )]
        );

        let mob = tables.get_entity("testns:mob").expect("mob table");
        match &mob.pools[0].entries[2] {
            LootEntry::Tag { tag, expand, .. } => {
                assert_eq!(tag, "testns:discs");
                assert!(*expand);
            }
            other => panic!("expected Tag entry, got {other:?}"),
        }
        match &mob.pools[0].entries[3] {
            LootEntry::LootTable {
                source: LootTableSource::Reference { id, table },
                ..
            } => {
                assert_eq!(id, "testns:gameplay/bonus_fish");
                assert!(
                    table.is_some(),
                    "external loot_table reference must resolve"
                );
            }
            other => panic!("expected resolved LootTable Reference entry, got {other:?}"),
        }
        match &mob.pools[0].entries[4] {
            LootEntry::LootTable {
                source: LootTableSource::Inline(inline),
                ..
            } => {
                assert!(matches!(
                    inline.pools[0].entries[0],
                    LootEntry::Empty { .. }
                ));
            }
            other => panic!("expected Inline LootTable entry, got {other:?}"),
        }

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn missing_and_cyclic_loot_table_references_error_naming_file() {
        let missing_root = scratch_root("missing_lt_ref");
        write(
            &missing_root.join("data/testns/loot_table/entities/broken.json"),
            r#"{
                "type": "minecraft:entity",
                "pools": [{
                    "rolls": 1,
                    "entries": [{"type": "minecraft:loot_table", "value": "testns:entities/nope"}]
                }]
            }"#,
        );
        let err = LootTables::load_entities(&missing_root)
            .expect_err("missing loot_table reference must fail")
            .to_string();
        assert!(
            err.contains("broken.json"),
            "error must name referring file: {err}"
        );
        assert!(
            err.contains("testns:entities/nope"),
            "error must name missing target id: {err}"
        );
        fs::remove_dir_all(&missing_root).expect("cleanup");

        let cycle_root = scratch_root("cyclic_lt_ref");
        write(
            &cycle_root.join("data/testns/loot_table/entities/a.json"),
            r#"{
                "type": "minecraft:entity",
                "pools": [{
                    "rolls": 1,
                    "entries": [{"type": "minecraft:loot_table", "value": "testns:entities/b"}]
                }]
            }"#,
        );
        write(
            &cycle_root.join("data/testns/loot_table/entities/b.json"),
            r#"{
                "type": "minecraft:entity",
                "pools": [{
                    "rolls": 1,
                    "entries": [{"type": "minecraft:loot_table", "value": "testns:entities/a"}]
                }]
            }"#,
        );
        let err = LootTables::load_entities(&cycle_root)
            .expect_err("cyclic loot_table reference must fail")
            .to_string();
        assert!(
            err.contains("cyclic loot_table reference"),
            "error must report cycle: {err}"
        );
        fs::remove_dir_all(&cycle_root).expect("cleanup");
    }

    fn assert_entry_loot_table_refs_resolved(entry: &LootEntry) {
        match entry {
            LootEntry::LootTable { source, .. } => match source {
                LootTableSource::Reference { id, table } => {
                    assert!(table.is_some(), "unresolved loot_table reference `{id}`");
                }
                LootTableSource::Inline(inline) => {
                    for pool in &inline.pools {
                        for sub in &pool.entries {
                            assert_entry_loot_table_refs_resolved(sub);
                        }
                    }
                }
            },
            LootEntry::Alternatives { children, .. } => {
                for child in children {
                    assert_entry_loot_table_refs_resolved(child);
                }
            }
            LootEntry::Item { .. }
            | LootEntry::Dynamic { .. }
            | LootEntry::Empty { .. }
            | LootEntry::Tag { .. } => {}
        }
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored loads_all_vanilla_entity_loot_tables`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn loads_all_vanilla_entity_loot_tables() {
        let root = vanilla_data_root();
        let tables = LootTables::load_entities(&root).expect("load vanilla entity loot tables");
        // 93 top-level entity files + 16 `sheep/<color>.json` sub-tables = 109 files.
        assert_eq!(tables.len(), 109);
        assert!(tables.get_entity("minecraft:cow").is_some());
        assert!(tables.get_entity("cow").is_some());
        assert!(tables.get_entity("minecraft:entities/cow").is_some());
        assert!(tables.get_entity("minecraft:sheep/white").is_some());

        for (_id, table) in tables.iter() {
            for pool in &table.pools {
                for entry in &pool.entries {
                    assert_entry_loot_table_refs_resolved(entry);
                }
            }
        }

        let unsupported = tables.unsupported();
        println!("entity loot unsupported nodes: {unsupported:?}");
        assert_eq!(
            unsupported,
            vec![(
                "minecraft:entities/pillager",
                "minecraft:set_ominous_bottle_amplifier",
            )]
        );
    }

    #[test]
    fn killed_by_player_and_random_chance_with_enchanted_bonus_eval() {
        let root = scratch_root("killed_by_player_and_bonus");
        write(
            &root.join("data/testns/loot_table/entities/rare_mob.json"),
            r#"{
                "type": "minecraft:entity",
                "pools": [{
                    "rolls": 1,
                    "condition": [
                        {"type": "minecraft:killed_by_player"},
                        {
                            "type": "minecraft:random_chance_with_enchanted_bonus",
                            "enchantment": "minecraft:looting",
                            "unenchanted_chance": 0.25,
                            "enchanted_chance": {
                                "type": "minecraft:linear",
                                "base": 0.5,
                                "per_level_above_first": 0.25
                            }
                        }
                    ],
                    "entries": [{"type": "minecraft:item", "name": "testns:rare_gem"}]
                }]
            }"#,
        );
        let tables = LootTables::load_entities(&root).expect("load");
        let table = tables.get_entity("testns:rare_mob").expect("rare_mob");

        // Never drops when killed_by_player is false, even with Looting III.
        let mut looting3 = BTreeMap::new();
        looting3.insert("minecraft:looting".to_owned(), 3);
        let non_player = LootContext::for_entity("testns:rare_mob")
            .with_killed_by_player(false)
            .with_attacker_weapon(ToolContext {
                item: "testns:sword".to_owned(),
                enchantments: looting3.clone(),
            });
        for seed in 0..500_u64 {
            assert!(
                table
                    .roll(&non_player, &mut RandomSource::from_world_seed(seed))
                    .is_empty()
            );
        }

        // Unenchanted player kill uses unenchanted_chance (0.25).
        let unenchanted = LootContext::for_entity("testns:rare_mob").with_killed_by_player(true);
        let mut hits = 0_u32;
        let trials = 4_000_u64;
        for seed in 0..trials {
            if !table
                .roll(&unenchanted, &mut RandomSource::from_world_seed(seed))
                .is_empty()
            {
                hits += 1;
            }
        }
        let rate = f64::from(hits) / trials as f64;
        assert!(
            (rate - 0.25).abs() <= 0.02,
            "expected unenchanted rate 0.25 +/- 0.02, got {rate}"
        );

        // Looting III evaluates linear(0.5, 0.25) at level 3 = 1.0 -> always drops.
        let loot3_ctx = LootContext::for_entity("testns:rare_mob")
            .with_killed_by_player(true)
            .with_attacker_weapon(ToolContext {
                item: "testns:sword".to_owned(),
                enchantments: looting3,
            });
        for seed in 0..100_u64 {
            let drops = table.roll(&loot3_ctx, &mut RandomSource::from_world_seed(seed));
            assert_eq!(drops.len(), 1);
            assert_eq!(drops[0].item(), "testns:rare_gem");
        }

        // Also cover LevelBasedValue::evaluate variants.
        assert_eq!(LevelBasedValue::Constant(0.4).evaluate(5), 0.4);
        assert_eq!(
            LevelBasedValue::LevelsSquared { added: 2.0 }.evaluate(3),
            11.0
        );
        let clamped = LevelBasedValue::Clamped {
            value: Box::new(LevelBasedValue::Linear {
                base: 0.2,
                per_level_above_first: 0.5,
            }),
            min: 0.1,
            max: 0.9,
        };
        assert_eq!(clamped.evaluate(3), 0.9);
        let frac = LevelBasedValue::Fraction {
            numerator: Box::new(LevelBasedValue::Constant(3.0)),
            denominator: Box::new(LevelBasedValue::LevelsSquared { added: 1.0 }),
        };
        assert!((frac.evaluate(2) - 0.6).abs() < 1e-12);
        let exp = LevelBasedValue::Exponent {
            base: Box::new(LevelBasedValue::Constant(2.0)),
            power: Box::new(LevelBasedValue::Linear {
                base: 1.0,
                per_level_above_first: 1.0,
            }),
        };
        assert_eq!(exp.evaluate(3), 8.0);
        let lookup = LevelBasedValue::Lookup {
            values: vec![0.1, 0.2],
            fallback: Box::new(LevelBasedValue::Constant(0.9)),
        };
        assert_eq!(lookup.evaluate(2), 0.2);
        assert_eq!(lookup.evaluate(3), 0.9);

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn entity_properties_eval_all_tested_fields_and_tags() {
        let root = scratch_root("entity_props_eval");
        write(
            &root.join("data/testns/tags/enchantment/smelts_loot.json"),
            r#"{"values": ["minecraft:fire_aspect"]}"#,
        );
        write(
            &root.join("data/testns/tags/entity_type/skeletons.json"),
            r#"{"values": ["testns:bog_archer"]}"#,
        );
        write(
            &root.join("data/testns/loot_table/entities/complex_mob.json"),
            r##"{
                "type": "minecraft:entity",
                "pools": [
                    {
                        "rolls": 1,
                        "condition": [
                            {
                                "type": "minecraft:entity_properties",
                                "entity": "this",
                                "predicate": {
                                    "minecraft:flags": {"is_on_fire": true, "is_baby": true},
                                    "minecraft:entity_type": "testns:complex_mob",
                                    "minecraft:vehicle": {"minecraft:entity_type": "testns:chicken"},
                                    "minecraft:components": {"minecraft:sheep/color": "cyan"},
                                    "minecraft:type_specific/sheep": {"sheared": false},
                                    "minecraft:type_specific/cube_mob": {"size": {"min": 2, "max": 4}},
                                    "minecraft:type_specific/raider": {"is_captain": true}
                                }
                            },
                            {
                                "type": "minecraft:entity_properties",
                                "entity": "attacker",
                                "predicate": {"minecraft:entity_type": "#testns:skeletons"}
                            },
                            {
                                "type": "minecraft:entity_properties",
                                "entity": "direct_attacker",
                                "predicate": {
                                    "minecraft:equipment": {
                                        "mainhand": {
                                            "predicates": {
                                                "minecraft:enchantments": [{"enchantments": "#testns:smelts_loot"}]
                                            }
                                        }
                                    }
                                }
                            }
                        ],
                        "entries": [{"type": "minecraft:item", "name": "testns:matched"}]
                    }
                ]
            }"##,
        );
        let tables = LootTables::load_entities(&root).expect("load");
        assert!(
            tables
                .enchantment_tags()
                .members("testns:smelts_loot")
                .is_some()
        );
        let table = tables
            .get_entity("testns:complex_mob")
            .expect("complex_mob");

        let mut fire_aspect = BTreeMap::new();
        fire_aspect.insert("minecraft:fire_aspect".to_owned(), 1);
        let matching_ctx = LootContext::default()
            .with_entity(
                EntityContext::new("testns:complex_mob")
                    .with_on_fire(true)
                    .with_baby(true)
                    .with_vehicle(EntityContext::new("testns:chicken"))
                    .with_component("minecraft:sheep/color", "cyan")
                    .with_sheep_sheared(false)
                    .with_cube_mob_size(3)
                    .with_raider_captain(true),
            )
            .with_attacker(EntityContext::new("testns:bog_archer"))
            .with_direct_attacker(EntityContext::new("minecraft:player").with_mainhand(
                ToolContext {
                    item: "minecraft:diamond_sword".to_owned(),
                    enchantments: fire_aspect,
                },
            ));

        let mut rng = RandomSource::from_world_seed(42);
        let drops = table.roll(&matching_ctx, &mut rng);
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item(), "testns:matched");

        // Changing any one field fails the condition cleanly.
        let mut wrong_size = matching_ctx.clone();
        wrong_size.entity.as_mut().expect("entity").cube_mob_size = Some(1);
        assert!(table.roll(&wrong_size, &mut rng).is_empty());

        let mut wrong_vehicle = matching_ctx.clone();
        wrong_vehicle.entity.as_mut().expect("entity").vehicle = None;
        assert!(table.roll(&wrong_vehicle, &mut rng).is_empty());

        let mut no_direct_attacker = matching_ctx.clone();
        no_direct_attacker.direct_attacker = None;
        assert!(table.roll(&no_direct_attacker, &mut rng).is_empty());

        let mut missing_this = matching_ctx;
        missing_this.entity = None;
        assert!(table.roll(&missing_this, &mut rng).is_empty());

        // Explicit entity_type_tags on EntityContext also satisfy `#tag` without pack files.
        assert!(
            table
                .roll(
                    &LootContext::default()
                        .with_entity(
                            EntityContext::new("testns:complex_mob")
                                .with_on_fire(true)
                                .with_baby(true)
                                .with_vehicle(EntityContext::new("testns:chicken"))
                                .with_component("minecraft:sheep/color", "cyan")
                                .with_sheep_sheared(false)
                                .with_cube_mob_size(2)
                                .with_raider_captain(true),
                        )
                        .with_attacker(
                            EntityContext::new("testns:custom_mob")
                                .with_entity_type_tag("#testns:skeletons"),
                        )
                        .with_direct_attacker(
                            EntityContext::new("minecraft:player").with_mainhand(ToolContext {
                                item: String::new(),
                                enchantments: {
                                    let mut m = BTreeMap::new();
                                    m.insert("fire_aspect".to_owned(), 2);
                                    m
                                },
                            },)
                        ),
                    &mut rng,
                )
                .len()
                == 1
        );

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn damage_source_properties_eval_entities_and_tags() {
        let root = scratch_root("damage_source_eval");
        write(
            &root.join("data/testns/tags/damage_type/is_lightning.json"),
            r#"{"values": ["testns:zap"]}"#,
        );
        write(
            &root.join("data/testns/loot_table/entities/magma_like.json"),
            r##"{
                "type": "minecraft:entity",
                "pools": [
                    {
                        "rolls": 1,
                        "condition": {
                            "type": "minecraft:damage_source_properties",
                            "predicate": {
                                "direct_entity": {"minecraft:entity_type": "minecraft:fireball"},
                                "tags": [{"id": "#minecraft:is_projectile", "expected": true}]
                            }
                        },
                        "entries": [{"type": "minecraft:item", "name": "testns:tear"}]
                    },
                    {
                        "rolls": 1,
                        "condition": {
                            "type": "minecraft:damage_source_properties",
                            "predicate": {
                                "source_entity": {
                                    "minecraft:entity_type": "minecraft:frog",
                                    "minecraft:components": {"minecraft:frog/variant": "minecraft:warm"}
                                }
                            }
                        },
                        "entries": [{"type": "minecraft:item", "name": "testns:pearlescent_froglight"}]
                    },
                    {
                        "rolls": 1,
                        "condition": {
                            "type": "minecraft:damage_source_properties",
                            "predicate": {
                                "tags": [{"id": "#testns:is_lightning", "expected": true}]
                            }
                        },
                        "entries": [{"type": "minecraft:item", "name": "testns:bowl"}]
                    }
                ]
            }"##,
        );
        let tables = LootTables::load_entities(&root).expect("load");
        let table = tables.get_entity("testns:magma_like").expect("magma_like");
        let mut rng = RandomSource::from_world_seed(5);

        // No damage_source -> nothing.
        assert!(table.roll(&LootContext::default(), &mut rng).is_empty());

        // Projectile fireball hit.
        let projectile_ctx = LootContext::default().with_damage_source(
            DamageSourceContext::default()
                .with_direct_entity(EntityContext::new("minecraft:fireball"))
                .with_tag("#minecraft:is_projectile"),
        );
        let drops = table.roll(&projectile_ctx, &mut rng);
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item(), "testns:tear");

        // Frog eat hit.
        let frog_ctx = LootContext::default().with_damage_source(
            DamageSourceContext::default().with_source_entity(
                EntityContext::new("minecraft:frog")
                    .with_component("minecraft:frog/variant", "minecraft:warm"),
            ),
        );
        let drops = table.roll(&frog_ctx, &mut rng);
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item(), "testns:pearlescent_froglight");

        // Lightning hit resolved via damage_type tag registry.
        let lightning_ctx = LootContext::default()
            .with_damage_source(DamageSourceContext::default().with_damage_type("testns:zap"));
        let drops = table.roll(&lightning_ctx, &mut rng);
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item(), "testns:bowl");

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn enchanted_count_increase_float_rounding_and_limit() {
        let root = scratch_root("looting_float_rounding");
        write(
            &root.join("data/testns/loot_table/entities/cow_like.json"),
            r#"{
                "type": "minecraft:entity",
                "pools": [
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "testns:hide",
                            "modifier": [
                                {"type": "minecraft:set_count", "count": 1},
                                {
                                    "type": "minecraft:enchanted_count_increase",
                                    "enchantment": "minecraft:looting",
                                    "count": {"type": "minecraft:uniform", "min": 0.0, "max": 1.0}
                                }
                            ]
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "testns:capped_arrow",
                            "modifier": [
                                {"type": "minecraft:set_count", "count": {"type": "minecraft:uniform", "min": 0, "max": 1}},
                                {
                                    "type": "minecraft:enchanted_count_increase",
                                    "enchantment": "minecraft:looting",
                                    "limit": 1,
                                    "count": {"type": "minecraft:uniform", "min": 0.0, "max": 1.0}
                                }
                            ]
                        }]
                    }
                ]
            }"#,
        );
        let tables = LootTables::load_entities(&root).expect("load");
        let table = tables.get_entity("testns:cow_like").expect("cow_like");

        let bare = LootContext::for_entity("testns:cow_like");
        for seed in 0..100_u64 {
            let drops = table.roll(&bare, &mut RandomSource::from_world_seed(seed));
            let hide = drops
                .iter()
                .find(|s| s.item() == "testns:hide")
                .expect("hide always drops");
            assert_eq!(hide.count(), 1);
        }

        let mut looting3 = BTreeMap::new();
        looting3.insert("minecraft:looting".to_owned(), 3);
        let loot3_ctx =
            LootContext::for_entity("testns:cow_like").with_attacker_weapon(ToolContext {
                item: "minecraft:diamond_sword".to_owned(),
                enchantments: looting3,
            });

        let mut hide_counts = [0_u32; 4]; // bonus 0..=3 => total count 1..=4
        let mut arrow_drops = 0_u32;
        let trials = 12_000_u64;
        for seed in 0..trials {
            let drops = table.roll(&loot3_ctx, &mut RandomSource::from_world_seed(seed));
            let hide = drops
                .iter()
                .find(|s| s.item() == "testns:hide")
                .expect("hide");
            let bonus = usize::from(hide.count() - 1);
            assert!(bonus <= 3, "looting bonus must stay in 0..=3, got {bonus}");
            hide_counts[bonus] += 1;
            if let Some(arrow) = drops.iter().find(|s| s.item() == "testns:capped_arrow") {
                assert_eq!(arrow.count(), 1, "limit: 1 must cap tipped arrows at 1");
                arrow_drops += 1;
            }
        }
        let expected_hide = [1.0 / 6.0, 1.0 / 3.0, 1.0 / 3.0, 1.0 / 6.0];
        for (bonus, expected) in expected_hide.iter().enumerate() {
            let observed = f64::from(hide_counts[bonus]) / trials as f64;
            assert!(
                (observed - expected).abs() <= 0.02,
                "looting III bonus {bonus}: expected {expected:.4}, got {observed:.4}"
            );
        }
        let arrow_rate = f64::from(arrow_drops) / trials as f64;
        let expected_arrow_rate = 11.0 / 12.0;
        assert!(
            (arrow_rate - expected_arrow_rate).abs() <= 0.02,
            "expected capped arrow rate ~{expected_arrow_rate:.4}, got {arrow_rate:.4}"
        );

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn furnace_smelt_set_potion_and_unsupported_amplifier() {
        let root = scratch_root("smelt_potion_amp");
        write(
            &root.join("data/testns/recipe/cook_raw_meat.json"),
            r#"{
                "type": "minecraft:smelting",
                "ingredient": "testns:raw_meat",
                "result": {"id": "testns:cooked_meat"},
                "experience": 0.35,
                "cookingtime": 200
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/entities/cook_potion_mob.json"),
            r#"{
                "type": "minecraft:entity",
                "pools": [
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "testns:raw_meat",
                            "modifier": [
                                {"type": "minecraft:set_count", "count": 3},
                                {
                                    "type": "minecraft:furnace_smelt",
                                    "condition": {
                                        "type": "minecraft:entity_properties",
                                        "entity": "this",
                                        "predicate": {"minecraft:flags": {"is_on_fire": true}}
                                    }
                                }
                            ]
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "testns:bone",
                            "modifier": [
                                {"type": "minecraft:set_count", "count": 2},
                                {"type": "minecraft:furnace_smelt"}
                            ]
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:tipped_arrow",
                            "modifier": {"type": "minecraft:set_potion", "id": "minecraft:poison"}
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:ominous_bottle",
                            "modifier": {
                                "type": "minecraft:set_ominous_bottle_amplifier",
                                "amplifier": {"type": "minecraft:uniform", "min": 0, "max": 4}
                            }
                        }]
                    }
                ]
            }"#,
        );
        let tables = LootTables::load_entities(&root).expect("load");
        let recipes = tables.recipes().expect("recipes loaded");
        assert!(
            recipes
                .find_cooking(CookingKind::Smelting, "testns:raw_meat")
                .is_some()
        );
        assert!(
            recipes
                .find_cooking(CookingKind::Smelting, "testns:bone")
                .is_none()
        );
        let table = tables
            .get_entity("testns:cook_potion_mob")
            .expect("cook_potion_mob");

        let fire_ctx = LootContext::default()
            .with_entity(EntityContext::new("testns:cook_potion_mob").with_on_fire(true));
        let roll = table.roll_with_notes(&fire_ctx, &mut RandomSource::from_world_seed(1));
        assert_eq!(roll.items.len(), 4);
        assert_eq!(roll.items[0].item(), "testns:cooked_meat");
        assert_eq!(roll.items[0].count(), 3);
        assert_eq!(roll.items[1].item(), "testns:bone");
        assert_eq!(roll.items[1].count(), 2);
        assert_eq!(roll.items[2].item(), "minecraft:tipped_arrow");
        assert_eq!(
            roll.items[2].components().potion(),
            Some("minecraft:poison")
        );
        assert_eq!(roll.items[3].item(), "minecraft:ominous_bottle");
        assert_eq!(
            roll.notes,
            vec![LootNote::UnsupportedModifier {
                item: "minecraft:ominous_bottle".to_owned(),
                what: "minecraft:set_ominous_bottle_amplifier".to_owned(),
            }]
        );

        // Cold kill leaves meat raw.
        let cold_ctx = LootContext::for_entity("testns:cook_potion_mob");
        let cold_drops = table.roll(&cold_ctx, &mut RandomSource::from_world_seed(1));
        assert_eq!(cold_drops[0].item(), "testns:raw_meat");
        assert_eq!(cold_drops[0].count(), 3);

        // `set_potion` with `"empty"` clears a previously set potion component.
        let clear_potion = LootTable::new(
            Some("minecraft:entity".to_owned()),
            None,
            vec![LootPool {
                rolls: NumberProvider::Constant(1.0),
                entries: vec![LootEntry::Item {
                    name: "minecraft:potion".to_owned(),
                    weight: 1,
                    quality: 0,
                    conditions: vec![],
                    modifiers: vec![
                        LootModifier::SetPotion {
                            id: "minecraft:healing".to_owned(),
                            conditions: vec![],
                        },
                        LootModifier::SetPotion {
                            id: "empty".to_owned(),
                            conditions: vec![],
                        },
                    ],
                }],
                conditions: vec![],
                modifiers: vec![],
            }],
            vec![],
        );
        let cleared = clear_potion.roll(
            &LootContext::default(),
            &mut RandomSource::from_world_seed(1),
        );
        assert_eq!(cleared.len(), 1);
        assert_eq!(cleared[0].components().potion(), None);

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn weighted_entries_empty_tag_and_nested_loot_table_eval() {
        let root = scratch_root("entries_weighted_tag_subtable");
        write(
            &root.join("data/testns/tags/item/music_discs.json"),
            r#"{"values": ["testns:disc_a", "testns:disc_b"]}"#,
        );
        write(
            &root.join("data/testns/recipe/cook_fish.json"),
            r#"{
                "type": "minecraft:smelting",
                "ingredient": "testns:raw_fish",
                "result": {"id": "testns:cooked_fish"},
                "experience": 0.35,
                "cookingtime": 200
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/gameplay/fish.json"),
            r#"{
                "type": "minecraft:fishing",
                "pools": [{
                    "rolls": 1,
                    "entries": [{"type": "minecraft:item", "name": "testns:raw_fish", "modifier": {"type": "minecraft:set_count", "count": 2}}]
                }]
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/entities/entry_showcase.json"),
            r##"{
                "type": "minecraft:entity",
                "pools": [
                    {
                        "rolls": 1,
                        "entries": [
                            {"type": "minecraft:item", "name": "testns:common", "weight": 3},
                            {"type": "minecraft:item", "name": "testns:rare", "weight": 1},
                            {"type": "minecraft:empty", "weight": 2}
                        ]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:tag",
                            "items": "#testns:music_discs",
                            "expand": false,
                            "modifier": {"type": "minecraft:set_count", "count": 2}
                        }]
                    },
                    {
                        "rolls": 1,
                        "modifier": {"type": "minecraft:set_count", "count": 3},
                        "entries": [{
                            "type": "minecraft:tag",
                            "items": "#testns:music_discs",
                            "expand": true,
                            "modifier": {"type": "minecraft:set_count", "count": 9}
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:loot_table",
                            "value": "testns:gameplay/fish",
                            "modifier": {"type": "minecraft:furnace_smelt"}
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:loot_table",
                            "value": {
                                "type": "minecraft:entity",
                                "pools": [{
                                    "rolls": 1,
                                    "entries": [{"type": "minecraft:item", "name": "testns:inline_drop"}]
                                }]
                            }
                        }]
                    }
                ]
            }"##,
        );
        let tables = LootTables::load_entities(&root).expect("load");
        let table = tables
            .get_entity("testns:entry_showcase")
            .expect("entry_showcase");

        let mut common = 0_u32;
        let mut rare = 0_u32;
        let mut empty = 0_u32;
        let mut saw_expanded_a = false;
        let mut saw_expanded_b = false;
        let trials = 3_000_u64;
        for seed in 0..trials {
            let drops = table.roll(
                &LootContext::default(),
                &mut RandomSource::from_world_seed(seed),
            );
            if drops.iter().any(|s| s.item() == "testns:common") {
                common += 1;
            } else if drops.iter().any(|s| s.item() == "testns:rare") {
                rare += 1;
            } else {
                empty += 1;
            }

            // expand: false drops BOTH disc_a and disc_b with count 2; expand: true drops
            // one of them with pool modifier count 3 (entry modifier 9 is ignored).
            let count2_a = drops
                .iter()
                .filter(|s| s.item() == "testns:disc_a" && s.count() == 2)
                .count();
            let count2_b = drops
                .iter()
                .filter(|s| s.item() == "testns:disc_b" && s.count() == 2)
                .count();
            assert_eq!(count2_a, 1);
            assert_eq!(count2_b, 1);

            for s in &drops {
                if s.item() == "testns:disc_a" && s.count() == 3 {
                    saw_expanded_a = true;
                }
                if s.item() == "testns:disc_b" && s.count() == 3 {
                    saw_expanded_b = true;
                }
                assert_ne!(s.count(), 9, "expanded tag must ignore entry modifier");
            }

            let cooked = drops
                .iter()
                .find(|s| s.item() == "testns:cooked_fish")
                .expect("outer furnace_smelt applies to sub-table drop");
            assert_eq!(cooked.count(), 2);
            assert!(drops.iter().any(|s| s.item() == "testns:inline_drop"));
        }
        assert!(saw_expanded_a && saw_expanded_b);
        let r_common = f64::from(common) / trials as f64;
        let r_rare = f64::from(rare) / trials as f64;
        let r_empty = f64::from(empty) / trials as f64;
        assert!((r_common - 0.5).abs() <= 0.03, "common rate {r_common}");
        assert!((r_rare - 1.0 / 6.0).abs() <= 0.03, "rare rate {r_rare}");
        assert!((r_empty - 1.0 / 3.0).abs() <= 0.03, "empty rate {r_empty}");

        // Exercise with_recipes / with_enchantment_tags builders + depth bound guard.
        let mut deep_table = LootTable::new(
            Some("minecraft:entity".to_owned()),
            None,
            vec![LootPool {
                rolls: NumberProvider::Constant(1.0),
                entries: vec![LootEntry::Item {
                    name: "testns:deep_item".to_owned(),
                    weight: 1,
                    quality: 0,
                    conditions: vec![],
                    modifiers: vec![],
                }],
                conditions: vec![],
                modifiers: vec![],
            }],
            vec![],
        );
        for _ in 0..=(MAX_LOOT_TABLE_DEPTH + 2) {
            deep_table = LootTable::new(
                Some("minecraft:entity".to_owned()),
                None,
                vec![LootPool {
                    rolls: NumberProvider::Constant(1.0),
                    entries: vec![LootEntry::LootTable {
                        source: LootTableSource::Inline(Box::new(deep_table)),
                        weight: 1,
                        quality: 0,
                        conditions: vec![],
                        modifiers: vec![],
                    }],
                    conditions: vec![],
                    modifiers: vec![],
                }],
                vec![],
            );
        }
        let configured = deep_table
            .with_enchantment_tags(Arc::new(tables.enchantment_tags().clone()))
            .with_recipes(Arc::new(tables.recipes().expect("recipes").clone()));
        assert!(
            configured
                .roll(
                    &LootContext::default(),
                    &mut RandomSource::from_world_seed(1)
                )
                .is_empty(),
            "depth > MAX_LOOT_TABLE_DEPTH must terminate without dropping"
        );

        fs::remove_dir_all(&root).expect("cleanup");
    }

    fn count_item_drops(stacks: &[ItemStack], item_id: &str) -> u32 {
        stacks
            .iter()
            .filter(|stack| stack.item() == item_id)
            .map(|stack| u32::from(stack.count()))
            .sum()
    }

    /// Pin vanilla `minecraft:cow` drops against `entities/cow.json` and `minecraft.wiki` `Cow`
    /// (revision `3903124`):
    /// - Raw Beef (`not_burn`) / Steak (`burn`, `minecraft:cooked_beef`): `quantity=1-3|lootingquantity=0-1`
    ///   (base mean 2.0, Looting III mean 3.5).
    /// - Leather: `quantity=0-2|lootingquantity=0-1` (base mean 1.0, Looting III mean 2.5, max `2 + 3 = 5`).
    ///
    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_cow_drops_looting_and_fire`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn vanilla_cow_drops_looting_and_fire() {
        let root = vanilla_data_root();
        let tables = LootTables::load_entities(&root).expect("load vanilla entity loot tables");
        let cow = tables.get_entity("minecraft:cow").expect("cow loot table");

        // Read expected ranges directly from `entities/cow.json`.
        let raw_json: Value = read_json(&root.join("data/minecraft/loot_table/entities/cow.json"))
            .expect("read cow.json");
        let pools = raw_json["pools"].as_array().expect("cow pools");
        let leather_entry = &pools[0]["entries"][0];
        assert_eq!(leather_entry["name"].as_str(), Some("minecraft:leather"));
        let leather_mods = leather_entry["modifier"].as_array().expect("leather mods");
        let leather_min = leather_mods[0]["count"]["min"].as_i64().expect("min") as u32;
        let leather_max = leather_mods[0]["count"]["max"].as_i64().expect("max") as u32;
        let leather_looting_max = leather_mods[1]["count"]["max"]
            .as_f64()
            .expect("looting max")
            .round() as u32;

        let beef_entry = &pools[1]["entries"][0];
        assert_eq!(beef_entry["name"].as_str(), Some("minecraft:beef"));
        let beef_mods = beef_entry["modifier"].as_array().expect("beef mods");
        let beef_min = beef_mods[0]["count"]["min"].as_i64().expect("beef min") as u32;
        let beef_max = beef_mods[0]["count"]["max"].as_i64().expect("beef max") as u32;
        let beef_looting_max = beef_mods[2]["count"]["max"]
            .as_f64()
            .expect("beef looting max")
            .round() as u32;

        // Without looting, leather stays in [leather_min, leather_max] and beef is raw [beef_min, beef_max].
        let normal_ctx = LootContext::for_entity("minecraft:cow");
        let mut bare_leather_sum = 0_u64;
        let mut bare_leather_min = u32::MAX;
        let mut bare_leather_max = 0_u32;
        let mut bare_beef_sum = 0_u64;
        let trials = 6_000_u64;
        for seed in 0..trials {
            let drops = cow.roll(&normal_ctx, &mut RandomSource::from_world_seed(seed));
            let leather = count_item_drops(&drops, "minecraft:leather");
            let beef = count_item_drops(&drops, "minecraft:beef");
            let steak = count_item_drops(&drops, "minecraft:cooked_beef");
            assert_eq!(steak, 0, "cold kill must not drop cooked_beef");
            assert!(
                (leather_min..=leather_max).contains(&leather),
                "bare leather {leather} outside {leather_min}..={leather_max}"
            );
            assert!(
                (beef_min..=beef_max).contains(&beef),
                "bare beef {beef} outside {beef_min}..={beef_max}"
            );
            bare_leather_min = bare_leather_min.min(leather);
            bare_leather_max = bare_leather_max.max(leather);
            bare_leather_sum += u64::from(leather);
            bare_beef_sum += u64::from(beef);
        }
        assert_eq!(bare_leather_min, leather_min);
        assert_eq!(bare_leather_max, leather_max);
        let bare_leather_avg = bare_leather_sum as f64 / trials as f64;
        let expected_bare_leather_avg = f64::from(leather_min + leather_max) / 2.0;
        assert!(
            (bare_leather_avg - expected_bare_leather_avg).abs() <= 0.05,
            "bare leather mean {bare_leather_avg} vs {expected_bare_leather_avg}"
        );
        let bare_beef_avg = bare_beef_sum as f64 / trials as f64;
        let expected_bare_beef_avg = f64::from(beef_min + beef_max) / 2.0;
        assert!(
            (bare_beef_avg - expected_bare_beef_avg).abs() <= 0.05,
            "bare beef mean {bare_beef_avg} vs {expected_bare_beef_avg}"
        );

        // With Looting III, max leather equals leather_max + 3 * leather_looting_max.
        let mut looting3 = BTreeMap::new();
        looting3.insert("minecraft:looting".to_owned(), 3);
        let loot3_ctx = LootContext::for_entity("minecraft:cow")
            .with_killed_by_player(true)
            .with_attacker_weapon(ToolContext {
                item: "minecraft:diamond_sword".to_owned(),
                enchantments: looting3,
            });
        let expected_loot3_leather_max = leather_max + 3 * leather_looting_max;
        let expected_loot3_beef_max = beef_max + 3 * beef_looting_max;
        let mut loot3_leather_sum = 0_u64;
        let mut loot3_leather_max = 0_u32;
        let mut loot3_beef_sum = 0_u64;
        let mut loot3_beef_max = 0_u32;
        for seed in 0..trials {
            let drops = cow.roll(&loot3_ctx, &mut RandomSource::from_world_seed(seed));
            let leather = count_item_drops(&drops, "minecraft:leather");
            let beef = count_item_drops(&drops, "minecraft:beef");
            assert!(leather <= expected_loot3_leather_max);
            assert!(beef >= beef_min && beef <= expected_loot3_beef_max);
            loot3_leather_max = loot3_leather_max.max(leather);
            loot3_beef_max = loot3_beef_max.max(beef);
            loot3_leather_sum += u64::from(leather);
            loot3_beef_sum += u64::from(beef);
        }
        assert_eq!(loot3_leather_max, expected_loot3_leather_max);
        assert_eq!(loot3_beef_max, expected_loot3_beef_max);
        let loot3_leather_avg = loot3_leather_sum as f64 / trials as f64;
        assert!(
            (loot3_leather_avg - 2.5).abs() <= 0.06,
            "Looting III leather mean {loot3_leather_avg} vs 2.5"
        );
        let loot3_beef_avg = loot3_beef_sum as f64 / trials as f64;
        assert!(
            (loot3_beef_avg - 3.5).abs() <= 0.06,
            "Looting III beef mean {loot3_beef_avg} vs 3.5"
        );

        // When cow is on fire, beef smelts to `minecraft:cooked_beef`.
        let fire_ctx = LootContext::default()
            .with_entity(EntityContext::new("minecraft:cow").with_on_fire(true));
        for seed in 0..200_u64 {
            let drops = cow.roll(&fire_ctx, &mut RandomSource::from_world_seed(seed));
            assert_eq!(
                count_item_drops(&drops, "minecraft:beef"),
                0,
                "burning cow must not drop raw beef"
            );
            let cooked = count_item_drops(&drops, "minecraft:cooked_beef");
            assert!((beef_min..=beef_max).contains(&cooked));
        }

        // Also confirm melee direct_attacker holding Fire Aspect (#minecraft:smelts_loot) smelts beef.
        let mut fire_aspect = BTreeMap::new();
        fire_aspect.insert("minecraft:fire_aspect".to_owned(), 2);
        let fire_aspect_ctx = LootContext::for_entity("minecraft:cow")
            .with_killed_by_player(true)
            .with_direct_attacker(EntityContext::new("minecraft:player").with_mainhand(
                ToolContext {
                    item: "minecraft:diamond_sword".to_owned(),
                    enchantments: fire_aspect,
                },
            ));
        let fa_drops = cow.roll(&fire_aspect_ctx, &mut RandomSource::from_world_seed(42));
        assert_eq!(count_item_drops(&fa_drops, "minecraft:beef"), 0);
        assert!(count_item_drops(&fa_drops, "minecraft:cooked_beef") >= beef_min);
    }

    /// Pin vanilla `minecraft:zombie` rare drops (`minecraft:iron_ingot`, `minecraft:carrot`,
    /// `minecraft:potato`, or `minecraft:baked_potato` when on fire) against `entities/zombie.json`
    /// and `minecraft.wiki` `Zombie` (revision `3902006`):
    /// `player_or_pet` (`killed_by_player`), `dropchance=0.025|lootingchance=0.01|multiplychance=1/3`
    /// (`0.025` base, `0.055` with Looting III).
    ///
    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_zombie_rare_drop_player_kill_and_rate`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn vanilla_zombie_rare_drop_player_kill_and_rate() {
        let root = vanilla_data_root();
        let tables = LootTables::load_entities(&root).expect("load vanilla entity loot tables");
        let zombie = tables
            .get_entity("minecraft:zombie")
            .expect("zombie loot table");

        // Extract rare-drop probabilities and item set directly from `entities/zombie.json`.
        let raw_json: Value =
            read_json(&root.join("data/minecraft/loot_table/entities/zombie.json"))
                .expect("read zombie.json");
        let rare_pool = &raw_json["pools"][2];
        let chance_cond = &rare_pool["condition"]["terms"][1];
        assert_eq!(
            chance_cond["type"].as_str(),
            Some("minecraft:random_chance_with_enchanted_bonus")
        );
        let unenchanted_rate = chance_cond["unenchanted_chance"]
            .as_f64()
            .expect("unenchanted_chance");
        let enchant_base = chance_cond["enchanted_chance"]["base"]
            .as_f64()
            .expect("base");
        let enchant_per_level = chance_cond["enchanted_chance"]["per_level_above_first"]
            .as_f64()
            .expect("per_level_above_first");
        let expected_loot3_rate = enchant_base + enchant_per_level * 2.0;

        let is_rare_drop = |item: &str| -> bool {
            matches!(
                item,
                "minecraft:iron_ingot"
                    | "minecraft:carrot"
                    | "minecraft:potato"
                    | "minecraft:baked_potato"
            )
        };

        // Never drops when not killed by a player (even with Looting III attached).
        let mut looting3 = BTreeMap::new();
        looting3.insert("minecraft:looting".to_owned(), 3);
        let non_player = LootContext::for_entity("minecraft:zombie")
            .with_killed_by_player(false)
            .with_attacker_weapon(ToolContext {
                item: "minecraft:diamond_sword".to_owned(),
                enchantments: looting3.clone(),
            });
        let seeds = 20_000_u64;
        for seed in 0..seeds {
            let drops = zombie.roll(&non_player, &mut RandomSource::from_world_seed(seed));
            assert!(
                drops.iter().all(|s| !is_rare_drop(s.item())),
                "non-player zombie kill must never yield rare drop on seed {seed}: {drops:?}"
            );
        }

        // Player kill without looting matches unenchanted_chance (0.025 +/- 0.01 over 20,000 seeds).
        let player_ctx = LootContext::for_entity("minecraft:zombie").with_killed_by_player(true);
        let mut rare_hits = 0_u32;
        for seed in 0..seeds {
            let drops = zombie.roll(&player_ctx, &mut RandomSource::from_world_seed(seed));
            if drops.iter().any(|s| is_rare_drop(s.item())) {
                rare_hits += 1;
            }
        }
        let observed_rate = f64::from(rare_hits) / seeds as f64;
        assert!(
            (observed_rate - unenchanted_rate).abs() <= 0.01,
            "expected zombie rare drop rate {unenchanted_rate} +/- 0.01, got {observed_rate}"
        );

        // Player kill with Looting III matches 0.055 +/- 0.01 over 20,000 seeds,
        // and burning zombies smelt potato rare drops to `minecraft:baked_potato`.
        let loot3_burning = LootContext::default()
            .with_entity(EntityContext::new("minecraft:zombie").with_on_fire(true))
            .with_killed_by_player(true)
            .with_attacker_weapon(ToolContext {
                item: "minecraft:diamond_sword".to_owned(),
                enchantments: looting3,
            });
        let mut loot3_hits = 0_u32;
        let mut baked_potato_hits = 0_u32;
        for seed in 0..seeds {
            let drops = zombie.roll(&loot3_burning, &mut RandomSource::from_world_seed(seed));
            assert_eq!(
                count_item_drops(&drops, "minecraft:potato"),
                0,
                "burning zombie must drop baked_potato instead of raw potato"
            );
            if drops.iter().any(|s| s.item() == "minecraft:baked_potato") {
                baked_potato_hits += 1;
            }
            if drops.iter().any(|s| is_rare_drop(s.item())) {
                loot3_hits += 1;
            }
        }
        let observed_loot3 = f64::from(loot3_hits) / seeds as f64;
        assert!(
            (observed_loot3 - expected_loot3_rate).abs() <= 0.01,
            "expected zombie Looting III rare drop rate {expected_loot3_rate} +/- 0.01, got {observed_loot3}"
        );
        assert!(
            baked_potato_hits > 0,
            "expected at least one smelted baked_potato rare drop"
        );
    }

    /// Roll every vanilla entity loot table once with an empty context and fixed seed
    /// without error or panic, and report how many return nothing.
    ///
    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_all_entity_tables_roll_without_panic_in_empty_context`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn vanilla_all_entity_tables_roll_without_panic_in_empty_context() {
        let root = vanilla_data_root();
        let tables = LootTables::load_entities(&root).expect("load vanilla entity loot tables");
        // 93 top-level entity files + 16 `sheep/<color>.json` sub-tables = 109 files.
        assert_eq!(tables.len(), 109);
        let item_defaults = tables
            .item_defaults()
            .expect("vanilla data root includes ItemDefaults reports");

        let empty_ctx = LootContext::default();
        let mut rng = RandomSource::from_world_seed(202_603);
        let mut returned_nothing = 0_usize;
        let mut top_level_total = 0_usize;
        let mut top_level_nothing = 0_usize;
        for (id, table) in tables.iter() {
            let outcome = table.roll_with_notes(&empty_ctx, &mut rng);
            for stack in &outcome.items {
                let expected_max = u16::from(
                    item_defaults
                        .max_stack_size(stack.item())
                        .expect("every vanilla entity drop exists in ItemDefaults"),
                );
                assert_eq!(stack.max_stack(), expected_max);
            }
            let is_top_level = !id.starts_with("minecraft:entities/sheep/");
            if is_top_level {
                top_level_total += 1;
            }
            if outcome.items.is_empty() {
                returned_nothing += 1;
                if is_top_level {
                    top_level_nothing += 1;
                }
            }
        }
        println!(
            "rolled {} vanilla entity tables in empty context ({} top-level + {} sheep color sub-tables); {} returned nothing ({} of {} top-level)",
            tables.len(),
            top_level_total,
            tables.len() - top_level_total,
            returned_nothing,
            top_level_nothing,
            top_level_total
        );
    }

    #[test]
    fn loads_chests_gameplay_archaeology_and_parses_new_modifiers() {
        let root = scratch_root("more_categories");
        write(
            &root.join("data/testns/enchantment/sharp.json"),
            r#"{
                "max_level": 5,
                "anvil_cost": 1,
                "weight": 10,
                "min_cost": {"base": 1, "per_level_above_first": 11},
                "max_cost": {"base": 21, "per_level_above_first": 11},
                "slots": ["mainhand"],
                "supported_items": "testns:sword"
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/archaeology/pot.json"),
            r#"{
                "type": "minecraft:archaeology",
                "pools": [{
                    "rolls": 1,
                    "entries": [
                        {"type": "minecraft:item", "name": "testns:shard"},
                        {
                            "type": "minecraft:item",
                            "name": "testns:stew",
                            "modifier": {
                                "type": "minecraft:set_stew_effect",
                                "effects": [{"type": "minecraft:night_vision", "duration": 8}]
                            }
                        }
                    ]
                }]
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/gameplay/fishing/fish.json"),
            r#"{
                "type": "minecraft:fishing",
                "pools": [{
                    "rolls": 1,
                    "entries": [{"type": "minecraft:item", "name": "testns:cod"}]
                }]
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/gameplay/fishing/treasure.json"),
            r#"{
                "type": "minecraft:fishing",
                "pools": [{
                    "rolls": 1,
                    "entries": [{"type": "minecraft:item", "name": "testns:pearl"}]
                }]
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/gameplay/fishing.json"),
            r##"{
                "type": "minecraft:fishing",
                "pools": [{
                    "rolls": 1,
                    "entries": [
                        {
                            "type": "minecraft:loot_table",
                            "value": "testns:gameplay/fishing/fish",
                            "weight": 1,
                            "quality": -1
                        },
                        {
                            "type": "minecraft:loot_table",
                            "value": "testns:gameplay/fishing/treasure",
                            "weight": 1,
                            "quality": 2,
                            "condition": {
                                "type": "minecraft:entity_properties",
                                "entity": "this",
                                "predicate": {
                                    "minecraft:type_specific/fishing_hook": {"in_open_water": true}
                                }
                            }
                        }
                    ]
                }]
            }"##,
        );
        write(
            &root.join("data/testns/loot_table/chests/sub/stash.json"),
            r##"{
                "type": "minecraft:chest",
                "pools": [{
                    "rolls": 1,
                    "entries": [
                        {
                            "type": "minecraft:item",
                            "name": "testns:sword",
                            "modifier": [
                                {"type": "minecraft:set_damage", "damage": {"type": "minecraft:uniform", "min": 0.25, "max": 0.75}, "add": true},
                                {"type": "minecraft:set_name", "name": {"text": "Blade"}, "target": "custom_name"},
                                {"type": "minecraft:set_enchantments", "enchantments": {"testns:sharp": 2}, "add": true},
                                {"type": "minecraft:enchant_randomly", "options": ["testns:sharp"], "only_compatible": false},
                                {"type": "minecraft:enchant_with_levels", "levels": 20, "options": "#minecraft:on_random_loot"},
                                {
                                    "type": "minecraft:filtered",
                                    "item_filter": {
                                        "items": "testns:sword",
                                        "count": {"min": 1, "max": 1},
                                        "predicates": {
                                            "minecraft:enchantments": [{"enchantments": "testns:sharp", "levels": {"min": 1}}],
                                            "minecraft:damage": {"damage": {"min": 0}}
                                        }
                                    },
                                    "on_pass": {"type": "minecraft:set_count", "count": 2},
                                    "on_fail": {"type": "minecraft:discard"}
                                },
                                {"type": "minecraft:set_name", "name": {"translate": "item.blade"}, "target": "item_name"},
                                {"type": "minecraft:set_name", "name": "Ignored", "target": "item_name"},
                                {"type": "minecraft:exploration_map", "destination": "#minecraft:on_treasure_maps"},
                                {"type": "minecraft:filtered", "item_filter": {"predicates": {"minecraft:map_id": {}}}, "on_fail": {"type": "minecraft:discard"}},
                                {"type": "minecraft:set_instrument", "options": "#minecraft:regular_goat_horns"},
                                {"type": "minecraft:set_ominous_bottle_amplifier", "amplifier": 1}
                            ]
                        },
                        {"type": "minecraft:loot_table", "value": "testns:gameplay/fishing"},
                        {"type": "minecraft:loot_table", "value": "testns:archaeology/pot"}
                    ]
                }]
            }"##,
        );

        let chests = LootTables::load_chests(&root).expect("load chests");
        assert_eq!(chests.len(), 1);
        assert!(chests.get_chest("testns:sub/stash").is_some());
        assert!(chests.get_chest("testns:chests/sub/stash").is_some());
        assert_eq!(chests.enchantments().len(), 1);

        let gameplay = LootTables::load_gameplay(&root).expect("load gameplay");
        assert_eq!(gameplay.len(), 3);
        assert!(gameplay.get_gameplay("testns:fishing").is_some());
        assert!(gameplay.get_gameplay("testns:fishing/fish").is_some());
        assert!(
            gameplay
                .get_gameplay("testns:gameplay/fishing/treasure")
                .is_some()
        );

        let arch = LootTables::load_archaeology(&root).expect("load archaeology");
        assert_eq!(arch.len(), 1);
        assert!(arch.get_archaeology("testns:pot").is_some());

        let combined = LootTables::load_categories(&root, &["chests", "gameplay", "archaeology"])
            .expect("load combined categories");
        assert_eq!(combined.len(), 5);
        assert!(combined.get_table("testns:chests/sub/stash").is_some());
        assert!(combined.get_table("testns:gameplay/fishing/fish").is_some());
        assert!(combined.get_table("testns:archaeology/pot").is_some());

        for (_id, table) in combined.iter() {
            for pool in &table.pools {
                for entry in &pool.entries {
                    assert_entry_loot_table_refs_resolved(entry);
                }
            }
        }

        let stash = combined
            .get_chest("testns:sub/stash")
            .expect("stash chest table");
        match &stash.pools[0].entries[0] {
            LootEntry::Item { modifiers, .. } => {
                assert!(matches!(
                    &modifiers[0],
                    LootModifier::SetDamage {
                        damage: NumberProvider::Uniform { min, max },
                        add: true,
                        ..
                    } if (*min - 0.25).abs() < f64::EPSILON && (*max - 0.75).abs() < f64::EPSILON
                ));
                assert_eq!(
                    modifiers[1],
                    LootModifier::SetName {
                        name: "Blade".to_owned(),
                        target: SetNameTarget::CustomName,
                        conditions: Vec::new(),
                    }
                );
                assert!(matches!(
                    &modifiers[2],
                    LootModifier::SetEnchantments { enchantments, add: true, .. }
                        if enchantments.get("testns:sharp") == Some(&NumberProvider::Constant(2.0))
                ));
                assert_eq!(
                    modifiers[3],
                    LootModifier::EnchantRandomly {
                        options: Some(vec!["testns:sharp".to_owned()]),
                        only_compatible: false,
                        conditions: Vec::new(),
                    }
                );
                assert_eq!(
                    modifiers[4],
                    LootModifier::EnchantWithLevels {
                        levels: NumberProvider::Constant(20.0),
                        options: Some(vec!["#minecraft:on_random_loot".to_owned()]),
                        conditions: Vec::new(),
                    }
                );
                assert!(matches!(
                    &modifiers[5],
                    LootModifier::Filtered {
                        item_filter,
                        on_pass,
                        on_fail,
                        ..
                    } if item_filter.items == vec!["testns:sword".to_owned()]
                        && item_filter.count == Some(IntRange { min: Some(1), max: Some(1) })
                        && item_filter.enchantments.len() == 1
                        && item_filter.damage.is_some()
                        && on_pass.len() == 1
                        && matches!(on_fail.as_slice(), [LootModifier::Discard { .. }])
                ));
            }
            other => panic!("expected Item entry, got {other:?}"),
        }

        assert_eq!(
            combined.unsupported(),
            vec![
                ("testns:archaeology/pot", "minecraft:set_stew_effect"),
                ("testns:chests/sub/stash", "minecraft:set_name"),
                ("testns:chests/sub/stash", "minecraft:set_name"),
                ("testns:chests/sub/stash", "minecraft:exploration_map"),
                ("testns:chests/sub/stash", "minecraft:filtered"),
                ("testns:chests/sub/stash", "minecraft:set_instrument"),
                (
                    "testns:chests/sub/stash",
                    "minecraft:set_ominous_bottle_amplifier"
                ),
            ]
        );

        // Verify fishing_hook in_open_water + quality/luck evaluation on fishing table.
        let open_hook = LootContext::default().with_entity(
            EntityContext::new("minecraft:fishing_bobber").with_fishing_hook_in_open_water(true),
        );
        let closed_hook = LootContext::default().with_entity(
            EntityContext::new("minecraft:fishing_bobber").with_fishing_hook_in_open_water(false),
        );
        let non_hook = LootContext::for_entity("minecraft:cod");
        let mut saw_treasure_open = false;
        for seed in 0..200_u64 {
            let open_res = gameplay
                .roll_gameplay(
                    "testns:fishing",
                    &open_hook,
                    &mut RandomSource::from_world_seed(seed),
                )
                .expect("fishing table");
            if open_res.items.iter().any(|s| s.item() == "testns:pearl") {
                saw_treasure_open = true;
            }
            let closed_res = gameplay
                .roll_gameplay(
                    "testns:fishing",
                    &closed_hook,
                    &mut RandomSource::from_world_seed(seed),
                )
                .expect("fishing table");
            assert!(
                closed_res.items.iter().all(|s| s.item() == "testns:cod"),
                "closed water must never yield treasure"
            );
            let non_hook_res = gameplay
                .roll_gameplay(
                    "testns:fishing",
                    &non_hook,
                    &mut RandomSource::from_world_seed(seed),
                )
                .expect("fishing table");
            assert!(
                non_hook_res.items.iter().all(|s| s.item() == "testns:cod"),
                "non-bobber entity must fail fishing_hook check"
            );
        }
        assert!(saw_treasure_open, "open water fishing must allow treasure");

        // Luck 1 reduces fish weight (1 + (-1)*1 = 0) and raises treasure weight (1 + 2*1 = 3).
        let lucky_hook = open_hook.with_luck(1.0);
        for seed in 0..50_u64 {
            let res = gameplay
                .roll_gameplay(
                    "testns:fishing",
                    &lucky_hook,
                    &mut RandomSource::from_world_seed(seed),
                )
                .expect("fishing table");
            assert!(
                res.items.iter().all(|s| s.item() == "testns:pearl"),
                "luck 1 zeros out fish (weight 1 + quality -1 * 1 = 0)"
            );
        }

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn rejects_invalid_new_modifier_and_quality_shapes() {
        let cases = [
            (
                "bad_set_name_target.json",
                r#"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"testns:x","modifier":{"type":"minecraft:set_name","name":"A","target":"lore"}}]}]}"#,
            ),
            (
                "missing_set_damage.json",
                r#"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"testns:x","modifier":{"type":"minecraft:set_damage"}}]}]}"#,
            ),
            (
                "bad_set_enchantments.json",
                r#"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"testns:x","modifier":{"type":"minecraft:set_enchantments","enchantments":["minecraft:sharpness"]}}]}]}"#,
            ),
            (
                "missing_filtered_predicate.json",
                r#"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"testns:x","modifier":{"type":"minecraft:filtered"}}]}]}"#,
            ),
            (
                "bad_quality.json",
                r#"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"testns:x","quality":"high"}]}]}"#,
            ),
        ];
        for (filename, body) in cases {
            let root = scratch_root(filename);
            write(
                &root.join("data/testns/loot_table/chests").join(filename),
                body,
            );
            let err = LootTables::load_chests(&root)
                .expect_err("invalid table must fail")
                .to_string();
            assert!(
                err.contains(filename),
                "error for {filename} must name the file: {err}"
            );
            fs::remove_dir_all(&root).expect("cleanup");
        }
    }

    #[test]
    fn set_damage_modifier_sets_and_adds_durability_fraction_from_max_damage() {
        let root = scratch_root("mod_set_damage");
        write(
            &root.join("reports/minecraft/components/item/test_blade.json"),
            r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:max_damage": 100}}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/stick.json"),
            r#"{"components": {"minecraft:max_stack_size": 64}}"#,
        );
        write(
            &root.join("data/testns/loot_table/chests/damage.json"),
            r#"{
                "type": "minecraft:chest",
                "pools": [
                    {
                        "rolls": 1,
                        "entries": [{"type": "minecraft:item", "name": "minecraft:test_blade", "modifier": {"type": "minecraft:set_damage", "damage": 0.25}}]
                    },
                    {
                        "rolls": 1,
                        "entries": [{"type": "minecraft:item", "name": "minecraft:test_blade", "modifier": {"type": "minecraft:set_damage", "damage": 1.0}}]
                    },
                    {
                        "rolls": 1,
                        "entries": [{"type": "minecraft:item", "name": "minecraft:test_blade", "modifier": {"type": "minecraft:set_damage", "damage": 0.0}}]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:test_blade",
                            "modifier": [
                                {"type": "minecraft:set_damage", "damage": 0.8},
                                {"type": "minecraft:set_damage", "damage": -0.3, "add": true}
                            ]
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:test_blade",
                            "modifier": [
                                {"type": "minecraft:set_damage", "damage": 0.5},
                                {"type": "minecraft:set_damage", "damage": 0.75, "add": true}
                            ]
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{"type": "minecraft:item", "name": "minecraft:stick", "modifier": {"type": "minecraft:set_damage", "damage": 0.5}}]
                    }
                ]
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/chests/uniform_damage.json"),
            r#"{
                "type": "minecraft:chest",
                "pools": [{
                    "rolls": 1,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "minecraft:test_blade",
                        "modifier": {"type": "minecraft:set_damage", "damage": {"type": "minecraft:uniform", "min": 0.1, "max": 0.9}}
                    }]
                }]
            }"#,
        );

        let tables = LootTables::load_chests(&root).expect("load set_damage tables");
        let items = tables
            .roll_chest(
                "testns:damage",
                &LootContext::default(),
                &mut RandomSource::from_world_seed(7),
            )
            .expect("damage table")
            .items;
        assert_eq!(items.len(), 6);
        assert_eq!(items[0].components().damage(), Some(75));
        assert_eq!(items[1].components().damage(), None);
        assert_eq!(items[2].components().damage(), Some(100));
        assert_eq!(items[3].components().damage(), Some(50));
        assert_eq!(items[4].components().damage(), None);
        assert_eq!(items[5].components().damage(), None);

        let uniform_table = tables
            .get_chest("testns:uniform_damage")
            .expect("uniform_damage");
        let mut saw_low = false;
        let mut saw_high = false;
        for seed in 0..200_u64 {
            let rolled = uniform_table.roll(
                &LootContext::default(),
                &mut RandomSource::from_world_seed(seed),
            );
            assert_eq!(rolled.len(), 1);
            let dmg = rolled[0].components().damage().expect("damaged");
            assert!((10..=90).contains(&dmg), "damage {dmg} out of 10..=90");
            if dmg < 50 {
                saw_low = true;
            } else {
                saw_high = true;
            }
        }
        assert!(saw_low && saw_high);

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn set_name_modifier_sets_plain_custom_name_and_leaves_unmodeled_targets_unsupported() {
        let root = scratch_root("mod_set_name");
        write(
            &root.join("data/testns/loot_table/chests/names.json"),
            r#"{
                "type": "minecraft:chest",
                "pools": [
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:iron_sword",
                            "modifier": [
                                {"type": "minecraft:set_name", "name": "Old Blade"},
                                {"type": "minecraft:set_name", "name": {"text": "Excalibur"}, "target": "custom_name"}
                            ]
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:compass",
                            "modifier": {"type": "minecraft:set_name", "name": "Wayfinder", "target": "item_name"}
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:map",
                            "modifier": {"type": "minecraft:set_name", "name": {"translate": "filled_map.buried_treasure"}}
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:stick",
                            "modifier": {"type": "minecraft:set_name", "name": {"text": "Styled", "color": "gold"}}
                        }]
                    }
                ]
            }"#,
        );

        let tables = LootTables::load_chests(&root).expect("load set_name");
        let roll = tables
            .roll_chest(
                "testns:names",
                &LootContext::default(),
                &mut RandomSource::from_world_seed(1),
            )
            .expect("names table");
        assert_eq!(roll.items.len(), 4);
        assert_eq!(roll.items[0].components().custom_name(), Some("Excalibur"));
        assert_eq!(roll.items[1].components().custom_name(), None);
        assert_eq!(roll.items[2].components().custom_name(), None);
        assert_eq!(roll.items[3].components().custom_name(), None);
        assert_eq!(
            roll.notes,
            vec![
                LootNote::UnsupportedModifier {
                    item: "minecraft:compass".to_owned(),
                    what: "minecraft:set_name".to_owned(),
                },
                LootNote::UnsupportedModifier {
                    item: "minecraft:map".to_owned(),
                    what: "minecraft:set_name".to_owned(),
                },
                LootNote::UnsupportedModifier {
                    item: "minecraft:stick".to_owned(),
                    what: "minecraft:set_name".to_owned(),
                },
            ]
        );

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn set_enchantments_modifier_modifies_clamps_removes_and_converts_books() {
        let root = scratch_root("mod_set_enchantments");
        write(
            &root.join("data/testns/loot_table/chests/enchs.json"),
            r#"{
                "type": "minecraft:chest",
                "pools": [
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:iron_sword",
                            "modifier": [
                                {"type": "minecraft:set_enchantments", "enchantments": {"minecraft:sharpness": 3, "minecraft:looting": 2}},
                                {"type": "minecraft:set_enchantments", "enchantments": {"minecraft:sharpness": 2, "minecraft:fire_aspect": 1}, "add": true},
                                {"type": "minecraft:set_enchantments", "enchantments": {"minecraft:looting": -5}, "add": true},
                                {"type": "minecraft:set_enchantments", "enchantments": {"minecraft:fire_aspect": 0}},
                                {"type": "minecraft:set_enchantments", "enchantments": {"minecraft:unbreaking": 400}, "add": true}
                            ]
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:book",
                            "modifier": [
                                {"type": "minecraft:set_enchantments", "enchantments": {"minecraft:wind_burst": 1}},
                                {"type": "minecraft:set_enchantments", "enchantments": {"minecraft:wind_burst": 2}, "add": true}
                            ]
                        }]
                    }
                ]
            }"#,
        );

        let tables = LootTables::load_chests(&root).expect("load set_enchantments");
        let items = tables
            .roll_chest(
                "testns:enchs",
                &LootContext::default(),
                &mut RandomSource::from_world_seed(11),
            )
            .expect("enchs")
            .items;
        assert_eq!(items.len(), 2);

        let mut expected_sword = BTreeMap::new();
        expected_sword.insert("minecraft:sharpness".to_owned(), 5);
        expected_sword.insert("minecraft:unbreaking".to_owned(), 255);
        assert_eq!(items[0].item(), "minecraft:iron_sword");
        assert_eq!(items[0].components().enchantments(), &expected_sword);
        assert!(items[0].components().stored_enchantments().is_empty());

        let mut expected_book = BTreeMap::new();
        expected_book.insert("minecraft:wind_burst".to_owned(), 3);
        assert_eq!(items[1].item(), "minecraft:enchanted_book");
        assert_eq!(items[1].max_stack(), 1);
        assert!(items[1].components().enchantments().is_empty());
        assert_eq!(items[1].components().stored_enchantments(), &expected_book);

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn enchant_randomly_modifier_respects_options_compatibility_levels_and_books() {
        let root = scratch_root("mod_enchant_randomly");
        write(
            &root.join("data/testns/tags/enchantment/loot_pool.json"),
            r#"{"values": ["testns:sharpness", "testns:soul_speed", "testns:mending"]}"#,
        );
        write(
            &root.join("data/testns/enchantment/sharpness.json"),
            r#"{
                "max_level": 5,
                "anvil_cost": 1,
                "weight": 10,
                "min_cost": {"base": 1, "per_level_above_first": 11},
                "max_cost": {"base": 21, "per_level_above_first": 11},
                "slots": ["mainhand"],
                "supported_items": "minecraft:diamond_sword"
            }"#,
        );
        write(
            &root.join("data/testns/enchantment/soul_speed.json"),
            r#"{
                "max_level": 3,
                "anvil_cost": 4,
                "weight": 1,
                "min_cost": {"base": 10, "per_level_above_first": 10},
                "max_cost": {"base": 25, "per_level_above_first": 10},
                "slots": ["feet"],
                "supported_items": "minecraft:iron_boots"
            }"#,
        );
        write(
            &root.join("data/testns/enchantment/mending.json"),
            r#"{
                "max_level": 1,
                "anvil_cost": 4,
                "weight": 2,
                "min_cost": {"base": 25, "per_level_above_first": 25},
                "max_cost": {"base": 75, "per_level_above_first": 25},
                "slots": ["any"],
                "supported_items": ["minecraft:diamond_sword", "minecraft:iron_boots"]
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/chests/rnd_sword.json"),
            r#"{
                "type": "minecraft:chest",
                "pools": [{
                    "rolls": 1,
                    "entries": [{"type": "minecraft:item", "name": "minecraft:diamond_sword", "modifier": {"type": "minecraft:enchant_randomly"}}]
                }]
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/chests/rnd_sword_any.json"),
            r##"{
                "type": "minecraft:chest",
                "pools": [{
                    "rolls": 1,
                    "entries": [{"type": "minecraft:item", "name": "minecraft:diamond_sword", "modifier": {"type": "minecraft:enchant_randomly", "options": "#testns:loot_pool", "only_compatible": false}}]
                }]
            }"##,
        );
        write(
            &root.join("data/testns/loot_table/chests/rnd_sword_incompatible.json"),
            r#"{
                "type": "minecraft:chest",
                "pools": [{
                    "rolls": 1,
                    "entries": [{"type": "minecraft:item", "name": "minecraft:diamond_sword", "modifier": {"type": "minecraft:enchant_randomly", "options": "testns:soul_speed"}}]
                }]
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/chests/rnd_book.json"),
            r#"{
                "type": "minecraft:chest",
                "pools": [{
                    "rolls": 1,
                    "entries": [{"type": "minecraft:item", "name": "minecraft:book", "modifier": {"type": "minecraft:enchant_randomly", "options": "testns:soul_speed"}}]
                }]
            }"#,
        );

        let tables = LootTables::load_chests(&root).expect("load enchant_randomly");

        // `only_compatible: true` on sword never picks `soul_speed`, picks `sharpness` 1..=5 or `mending` 1.
        let mut sharp_levels = HashSet::new();
        let mut saw_mending = false;
        for seed in 0..200_u64 {
            let items = tables
                .roll_chest(
                    "testns:rnd_sword",
                    &LootContext::default(),
                    &mut RandomSource::from_world_seed(seed),
                )
                .expect("rnd_sword")
                .items;
            assert_eq!(items.len(), 1);
            let enchs = items[0].components().enchantments();
            assert_eq!(enchs.len(), 1);
            assert!(!enchs.contains_key("testns:soul_speed"));
            if let Some(&lvl) = enchs.get("testns:sharpness") {
                assert!((1..=5).contains(&lvl));
                sharp_levels.insert(lvl);
            }
            if let Some(&lvl) = enchs.get("testns:mending") {
                assert_eq!(lvl, 1);
                saw_mending = true;
            }
        }
        assert_eq!(sharp_levels.len(), 5, "expected all sharpness levels 1..=5");
        assert!(saw_mending);

        // `only_compatible: false` allows `soul_speed` on `diamond_sword`.
        let mut saw_incompat_soul_speed = false;
        for seed in 0..100_u64 {
            let items = tables
                .roll_chest(
                    "testns:rnd_sword_any",
                    &LootContext::default(),
                    &mut RandomSource::from_world_seed(seed),
                )
                .expect("rnd_sword_any")
                .items;
            if items[0]
                .components()
                .enchantments()
                .contains_key("testns:soul_speed")
            {
                saw_incompat_soul_speed = true;
                break;
            }
        }
        assert!(saw_incompat_soul_speed);

        // Compatible filter with zero matching candidates leaves sword unenchanted.
        let empty_incompat = tables
            .roll_chest(
                "testns:rnd_sword_incompatible",
                &LootContext::default(),
                &mut RandomSource::from_world_seed(1),
            )
            .expect("rnd_sword_incompatible")
            .items;
        assert!(empty_incompat[0].components().enchantments().is_empty());

        // Books are always compatible, convert to `minecraft:enchanted_book` (`max_stack = 1`),
        // and store enchantments in `stored_enchantments`.
        let mut book_levels = HashSet::new();
        for seed in 0..100_u64 {
            let items = tables
                .roll_chest(
                    "testns:rnd_book",
                    &LootContext::default(),
                    &mut RandomSource::from_world_seed(seed),
                )
                .expect("rnd_book")
                .items;
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].item(), "minecraft:enchanted_book");
            assert_eq!(items[0].max_stack(), 1);
            assert!(items[0].components().enchantments().is_empty());
            let stored = items[0].components().stored_enchantments();
            assert_eq!(stored.len(), 1);
            let &lvl = stored.get("testns:soul_speed").expect("soul_speed stored");
            assert!((1..=3).contains(&lvl));
            book_levels.insert(lvl);
        }
        assert_eq!(book_levels.len(), 3);

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn enchant_with_levels_modifier_reuses_select_enchantments_and_keeps_book_multi_enchants() {
        let root = scratch_root("mod_enchant_with_levels");
        write(
            &root.join("reports/minecraft/components/item/diamond_sword.json"),
            r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:enchantable": {"value": 10}}}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/book.json"),
            r#"{"components": {"minecraft:max_stack_size": 64, "minecraft:enchantable": {"value": 1}}}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/enchanted_book.json"),
            r#"{"components": {"minecraft:max_stack_size": 1}}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/stick.json"),
            r#"{"components": {"minecraft:max_stack_size": 64}}"#,
        );
        write(
            &root.join("data/minecraft/tags/enchantment/in_enchanting_table.json"),
            r#"{"values": ["testns:sharpness", "testns:power"]}"#,
        );
        write(
            &root.join("data/testns/tags/enchantment/on_random_loot.json"),
            r#"{"values": ["testns:sharpness", "testns:mending", "testns:power"]}"#,
        );
        write(
            &root.join("data/testns/enchantment/sharpness.json"),
            r#"{
                "max_level": 5,
                "anvil_cost": 1,
                "weight": 10,
                "min_cost": {"base": 1, "per_level_above_first": 10},
                "max_cost": {"base": 60, "per_level_above_first": 10},
                "slots": ["mainhand"],
                "supported_items": "minecraft:diamond_sword"
            }"#,
        );
        write(
            &root.join("data/testns/enchantment/mending.json"),
            r#"{
                "max_level": 1,
                "anvil_cost": 4,
                "weight": 5,
                "min_cost": {"base": 1, "per_level_above_first": 0},
                "max_cost": {"base": 100, "per_level_above_first": 0},
                "slots": ["any"],
                "supported_items": ["minecraft:diamond_sword", "minecraft:bow"]
            }"#,
        );
        write(
            &root.join("data/testns/enchantment/power.json"),
            r#"{
                "max_level": 5,
                "anvil_cost": 1,
                "weight": 10,
                "min_cost": {"base": 1, "per_level_above_first": 10},
                "max_cost": {"base": 60, "per_level_above_first": 10},
                "slots": ["mainhand"],
                "supported_items": "minecraft:bow"
            }"#,
        );
        write(
            &root.join("data/testns/loot_table/chests/lvl_sword.json"),
            r##"{
                "type": "minecraft:chest",
                "pools": [{
                    "rolls": 1,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "minecraft:diamond_sword",
                        "modifier": {
                            "type": "minecraft:enchant_with_levels",
                            "levels": {"type": "minecraft:uniform", "min": 25, "max": 35},
                            "options": "#testns:on_random_loot"
                        }
                    }]
                }]
            }"##,
        );
        write(
            &root.join("data/testns/loot_table/chests/lvl_book.json"),
            r##"{
                "type": "minecraft:chest",
                "pools": [{
                    "rolls": 1,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "minecraft:book",
                        "modifier": {
                            "type": "minecraft:enchant_with_levels",
                            "levels": 80,
                            "options": "#testns:on_random_loot"
                        }
                    }]
                }]
            }"##,
        );
        write(
            &root.join("data/testns/loot_table/chests/lvl_stick.json"),
            r#"{
                "type": "minecraft:chest",
                "pools": [{
                    "rolls": 1,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "minecraft:stick",
                        "modifier": {
                            "type": "minecraft:enchant_with_levels",
                            "levels": 30
                        }
                    }]
                }]
            }"#,
        );

        let tables = LootTables::load_chests(&root).expect("load enchant_with_levels");

        let mut saw_mending_on_sword = false;
        for seed in 0..200_u64 {
            let items = tables
                .roll_chest(
                    "testns:lvl_sword",
                    &LootContext::default(),
                    &mut RandomSource::from_world_seed(seed),
                )
                .expect("lvl_sword")
                .items;
            assert_eq!(items.len(), 1);
            let enchs = items[0].components().enchantments();
            assert!(!enchs.is_empty());
            assert!(
                !enchs.contains_key("testns:power"),
                "bow-only enchantment must not appear on diamond_sword"
            );
            if enchs.contains_key("testns:mending") {
                saw_mending_on_sword = true;
            }
        }
        assert!(
            saw_mending_on_sword,
            "on_random_loot must include treasure enchantments outside in_enchanting_table"
        );

        // Unlike the enchanting table, `enchant_with_levels` keeps multiple enchantments on books.
        let mut saw_multi_book = false;
        let mut saw_power_on_book = false;
        for seed in 0..50_u64 {
            let items = tables
                .roll_chest(
                    "testns:lvl_book",
                    &LootContext::default(),
                    &mut RandomSource::from_world_seed(seed),
                )
                .expect("lvl_book")
                .items;
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].item(), "minecraft:enchanted_book");
            assert_eq!(items[0].max_stack(), 1);
            let stored = items[0].components().stored_enchantments();
            assert!(!stored.is_empty());
            if stored.len() >= 2 {
                saw_multi_book = true;
            }
            if stored.contains_key("testns:power") {
                saw_power_on_book = true;
            }
        }
        assert!(saw_multi_book, "loot books keep multiple enchantments");
        assert!(saw_power_on_book, "books treat all options as primary");

        let stick = tables
            .roll_chest(
                "testns:lvl_stick",
                &LootContext::default(),
                &mut RandomSource::from_world_seed(1),
            )
            .expect("lvl_stick")
            .items;
        assert!(stick[0].components().enchantments().is_empty());

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn filtered_modifier_evaluates_item_filter_and_runs_pass_fail_or_discard() {
        let root = scratch_root("mod_filtered");
        write(
            &root.join("data/testns/tags/item/swords.json"),
            r#"{"values": ["minecraft:iron_sword"]}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/iron_sword.json"),
            r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:max_damage": 100}}"#,
        );
        write(
            &root.join("data/testns/loot_table/chests/filter.json"),
            r##"{
                "type": "minecraft:chest",
                "pools": [
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:iron_sword",
                            "modifier": [
                                {"type": "minecraft:set_damage", "damage": 0.6},
                                {"type": "minecraft:set_enchantments", "enchantments": {"minecraft:sharpness": 3}},
                                {
                                    "type": "minecraft:filtered",
                                    "item_filter": {
                                        "items": "#testns:swords",
                                        "count": {"min": 1, "max": 1},
                                        "predicates": {
                                            "minecraft:enchantments": [{"enchantments": "minecraft:sharpness", "levels": {"min": 2}}],
                                            "minecraft:damage": {"damage": {"min": 30, "max": 50}, "durability": {"min": 50, "max": 70}}
                                        }
                                    },
                                    "on_pass": {"type": "minecraft:set_name", "name": "FilteredMatch"},
                                    "on_fail": {"type": "minecraft:discard"}
                                }
                            ]
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:iron_sword",
                            "modifier": [
                                {"type": "minecraft:set_damage", "damage": 0.9},
                                {
                                    "type": "minecraft:filtered",
                                    "item_filter": {
                                        "predicates": {
                                            "minecraft:damage": {"durability": {"max": 50}}
                                        }
                                    },
                                    "on_pass": {"type": "minecraft:set_name", "name": "Worn"},
                                    "on_fail": {"type": "minecraft:set_name", "name": "Fresh"}
                                }
                            ]
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:book",
                            "modifier": [
                                {"type": "minecraft:set_enchantments", "enchantments": {"minecraft:mending": 1}},
                                {
                                    "type": "minecraft:filtered",
                                    "item_filter": {
                                        "items": "minecraft:enchanted_book",
                                        "predicates": {
                                            "minecraft:stored_enchantments": [{"enchantments": "minecraft:mending"}]
                                        }
                                    },
                                    "on_pass": {"type": "minecraft:set_name", "name": "MendingBook"},
                                    "on_fail": {"type": "minecraft:discard"}
                                }
                            ]
                        }]
                    },
                    {
                        "rolls": 1,
                        "entries": [{
                            "type": "minecraft:item",
                            "name": "minecraft:stick",
                            "modifier": {
                                "type": "minecraft:filtered",
                                "item_filter": {"items": "minecraft:diamond"},
                                "on_fail": {"type": "minecraft:discard"}
                            }
                        }]
                    }
                ]
            }"##,
        );

        let tables = LootTables::load_chests(&root).expect("load filtered");
        let items = tables
            .roll_chest(
                "testns:filter",
                &LootContext::default(),
                &mut RandomSource::from_world_seed(5),
            )
            .expect("filter")
            .items;
        assert_eq!(items.len(), 3, "stick must be discarded on_fail");
        assert_eq!(items[0].components().custom_name(), Some("FilteredMatch"));
        assert_eq!(items[0].components().damage(), Some(40));
        assert_eq!(items[1].components().custom_name(), Some("Fresh"));
        assert_eq!(items[2].item(), "minecraft:enchanted_book");
        assert_eq!(items[2].components().custom_name(), Some("MendingBook"));

        fs::remove_dir_all(&root).expect("cleanup");
    }

    /// Pin vanilla Java 26.3 `chests/` (58), `gameplay/` (26, including `fishing/` and
    /// `hero_of_the_village/` subfolders), and `archaeology/` (6) loot tables and
    /// their unsupported modifier/condition nodes.
    ///
    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored loads_all_vanilla_chest_gameplay_and_archaeology_loot_tables`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn loads_all_vanilla_chest_gameplay_and_archaeology_loot_tables() {
        let root = vanilla_data_root();
        let lt_dir = root.join("data/minecraft/loot_table");
        let chest_files = walk_json(&lt_dir.join("chests")).expect("walk chests");
        let gameplay_files = walk_json(&lt_dir.join("gameplay")).expect("walk gameplay");
        let archaeology_files = walk_json(&lt_dir.join("archaeology")).expect("walk archaeology");
        assert_eq!(chest_files.len(), 58);
        assert_eq!(gameplay_files.len(), 26);
        assert_eq!(archaeology_files.len(), 6);

        let chests = LootTables::load_chests(&root).expect("load vanilla chest loot tables");
        let gameplay = LootTables::load_gameplay(&root).expect("load vanilla gameplay loot tables");
        let archaeology =
            LootTables::load_archaeology(&root).expect("load vanilla archaeology loot tables");
        assert_eq!(chests.len(), 58);
        assert_eq!(gameplay.len(), 26);
        assert_eq!(archaeology.len(), 6);

        let tables = LootTables::load_categories(&root, &["chests", "gameplay", "archaeology"])
            .expect("load vanilla chest, gameplay, and archaeology loot tables");
        assert_eq!(tables.len(), 58 + 26 + 6);
        assert!(tables.get_chest("simple_dungeon").is_some());
        assert!(tables.get_chest("trial_chambers/reward").is_some());
        assert!(tables.get_gameplay("piglin_bartering").is_some());
        assert!(tables.get_gameplay("fishing").is_some());
        assert!(tables.get_gameplay("fishing/fish").is_some());
        assert!(
            tables
                .get_gameplay("hero_of_the_village/fletcher_gift")
                .is_some()
        );
        assert!(tables.get_archaeology("desert_pyramid").is_some());
        assert!(
            tables
                .get_table("minecraft:chests/simple_dungeon")
                .is_some()
        );

        for (_id, table) in tables.iter() {
            for pool in &table.pools {
                for entry in &pool.entries {
                    assert_entry_loot_table_refs_resolved(entry);
                }
            }
        }

        let unsupported = tables.unsupported();
        println!(
            "chest/gameplay/archaeology unsupported nodes ({}): {unsupported:?}",
            unsupported.len()
        );
        let mut expected_unsupported = vec![(
            "minecraft:archaeology/desert_well",
            "minecraft:set_stew_effect",
        )];
        for _ in 0..8 {
            expected_unsupported.extend([
                (
                    "minecraft:chests/abandoned_camp_common_chest",
                    "minecraft:location_check",
                ),
                (
                    "minecraft:chests/abandoned_camp_common_chest",
                    "minecraft:set_name",
                ),
                (
                    "minecraft:chests/abandoned_camp_common_chest",
                    "minecraft:exploration_map",
                ),
                (
                    "minecraft:chests/abandoned_camp_common_chest",
                    "minecraft:filtered",
                ),
            ]);
        }
        for _ in 0..7 {
            expected_unsupported.extend([
                (
                    "minecraft:chests/abandoned_camp_secret_chest",
                    "minecraft:exploration_map",
                ),
                (
                    "minecraft:chests/abandoned_camp_secret_chest",
                    "minecraft:filtered",
                ),
            ]);
        }
        expected_unsupported.extend([
            (
                "minecraft:chests/abandoned_mineshaft",
                "minecraft:location_check",
            ),
            (
                "minecraft:chests/ancient_city_ice_box",
                "minecraft:set_stew_effect",
            ),
            (
                "minecraft:chests/pillager_outpost",
                "minecraft:set_instrument",
            ),
            (
                "minecraft:chests/shipwreck_map",
                "minecraft:exploration_map",
            ),
            ("minecraft:chests/shipwreck_map", "minecraft:filtered"),
            (
                "minecraft:chests/shipwreck_supply",
                "minecraft:set_stew_effect",
            ),
            (
                "minecraft:chests/trial_chambers/reward_common",
                "minecraft:set_ominous_bottle_amplifier",
            ),
            (
                "minecraft:chests/trial_chambers/reward_ominous_common",
                "minecraft:set_ominous_bottle_amplifier",
            ),
            (
                "minecraft:chests/underwater_ruin_big",
                "minecraft:exploration_map",
            ),
            ("minecraft:chests/underwater_ruin_big", "minecraft:filtered"),
            (
                "minecraft:chests/underwater_ruin_small",
                "minecraft:exploration_map",
            ),
            (
                "minecraft:chests/underwater_ruin_small",
                "minecraft:filtered",
            ),
            (
                "minecraft:gameplay/fishing/junk",
                "minecraft:location_check",
            ),
        ]);
        assert_eq!(unsupported, expected_unsupported);
    }
}
