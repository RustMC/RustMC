//! Typed block and entity loot table model and runtime loader for Java 26.3 datapacks.
//!
//! Reads `data/<namespace>/loot_table/blocks/**/*.json` or
//! `data/<namespace>/loot_table/entities/**/*.json` from an
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
//! Schema definitions follow `minecraft.wiki`:
//! - `Loot table` (revision 3893957): root fields `type`, `modifier`, `pools`,
//!   `random_sequence`; pool fields `condition`, `modifier`, `rolls`, `entries`;
//!   entry types `item`, `loot_table`, `empty`, `tag`, `alternatives`, `dynamic`,
//!   and `weight`.
//! - `Predicate` (revision 3888899): `all_of`, `any_of`, `inverted`,
//!   `location_check` (with `Advancement/Conditions/location` revision 3684921 &
//!   `Advancement/Conditions/block` revision 3684708), `match_block`,
//!   `match_tool`, `random_chance`, `random_chance_with_enchanted_bonus`,
//!   `killed_by_player`, `survives_explosion`, `table_bonus`,
//!   `entity_properties` (`Advancement/Conditions/entity` revision 3825655),
//!   and `damage_source_properties` (`Advancement/Conditions/damage type` revision 3778836).
//! - `Item modifier` (revision 3896146): `apply_bonus`, `copy_components`,
//!   `copy_state`, `enchanted_count_increase`, `explosion_decay`,
//!   `furnace_smelt`, `limit_count`, `set_count`, `set_potion`.
//! - `Number provider` (revision 3869456): constant numbers, `uniform`, and
//!   `binomial`.
//! - `Enchantment definition` (revision 3843772): `Level-based value` (`linear`, etc.).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::item::{
    ItemDefaults, ItemDefaultsError, ItemStack, MAX_STACK_LIMIT, default_max_stack, items_equal,
    normalize_item_id,
};
use crate::vanilla::feature::{BlockTags, ItemTags};
use crate::vanilla::random::RandomSource;
use crate::vanilla::worldgen::{WorldgenError, read_dir_optional, read_json, walk_json};

/// Maximum nesting depth allowed when resolving or evaluating `loot_table` entries.
pub const MAX_LOOT_TABLE_DEPTH: usize = 32;

/// Errors encountered while loading or parsing loot tables.
#[derive(Debug)]
pub enum LootError {
    /// Reading or parsing a JSON file failed.
    Worldgen(WorldgenError),
    /// Reading or parsing default item component reports failed.
    ItemDefaults(ItemDefaultsError),
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

/// Inclusive integer range matcher (exact value or optional `min`/`max` bounds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IntRange {
    /// Inclusive minimum bound, if specified.
    pub min: Option<i32>,
    /// Inclusive maximum bound, if specified.
    pub max: Option<i32>,
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

/// Typed entity predicate for the fields tested by vanilla entity loot tables.
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
        conditions: Vec<LootCondition>,
        modifiers: Vec<LootModifier>,
    },
    /// `minecraft:loot_table` — rolls another loot table (referenced by id or inline).
    LootTable {
        source: LootTableSource,
        weight: u32,
        conditions: Vec<LootCondition>,
        modifiers: Vec<LootModifier>,
    },
    /// `minecraft:empty` — drops nothing.
    Empty {
        weight: u32,
        conditions: Vec<LootCondition>,
        modifiers: Vec<LootModifier>,
    },
    /// `minecraft:tag` — expands into one entry per tag item (`expand == true`)
    /// or drops all tag items together (`expand == false`).
    Tag {
        tag: String,
        expand: bool,
        weight: u32,
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

/// Execution context for evaluating a block [`LootTable`].
#[derive(Clone, Default)]
pub struct LootContext {
    /// Broken block state as `(block_id, properties)`.
    pub block_state: (String, BTreeMap<String, String>),
    /// Breaking tool, or `None` when broken bare-handed or without a tool.
    pub tool: Option<ToolContext>,
    /// Explosion radius when destroyed by an explosion, or `None` otherwise.
    pub explosion_radius: Option<f32>,
    /// Optional relative block lookup `(dx, dy, dz) -> Option<(block_id, properties)>`
    /// used by `minecraft:location_check`. When `None`, `location_check` evaluates
    /// to `false`.
    pub neighbor_blocks: Option<Arc<dyn BlockLookup>>,
}

impl fmt::Debug for LootContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LootContext")
            .field("block_state", &self.block_state)
            .field("tool", &self.tool)
            .field("explosion_radius", &self.explosion_radius)
            .field("has_neighbor_blocks", &self.neighbor_blocks.is_some())
            .finish()
    }
}

impl LootContext {
    /// Builds a bare-handed, non-explosion context for `block_id` with empty properties.
    #[must_use]
    pub fn for_block(block_id: impl Into<String>) -> Self {
        Self {
            block_state: (block_id.into(), BTreeMap::new()),
            tool: None,
            explosion_radius: None,
            neighbor_blocks: None,
        }
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
    item_defaults: Option<Arc<ItemDefaults>>,
}

/// Parsed block loot table loaded from `data/<ns>/loot_table/blocks/**/*.json`.
#[derive(Debug, Clone, Default)]
pub struct LootTable {
    /// Optional loot context type string (for example `"minecraft:block"`).
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
    /// Constructs a standalone [`LootTable`] with empty tag and item-default registries.
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
        self.registries = LootRegistries {
            item_tags,
            block_tags,
            item_defaults,
        };
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

    /// Evaluates this block loot table under `ctx` using `random` and returns dropped stacks.
    ///
    /// Uses real maximum stack sizes from [`ItemDefaults`] when loaded from the data root's
    /// `reports/minecraft/components/item/*.json`, falling back to [`default_max_stack`]
    /// otherwise.
    ///
    /// Semantics follow `minecraft.wiki` and the task's binding reviewer decisions:
    /// - **Pool & entry expansion** (`Loot table` rev 3893957):
    ///   - `"Each roll of a pool is independent."`
    ///   - `"composite entries first expand their children, and then singleton entries that do not meet their conditions are removed from the pool, then entries are rolled afterward."`
    ///   - `alternatives`: `"Extracts only the first successful (conditions are met) entry. Conditions are checked in order."`
    ///   - `dynamic`: `"Drops block-specific loot."` Kept unmaterialized and recorded in [`LootNote::DynamicNotApplied`] via [`Self::roll_with_notes`].
    ///   - Table `modifier`: `"Applies an item modifier, onto all item stacks dropped by this table."`
    /// - **Conditions** (`Predicate` rev 3888899 & `Data component predicate` rev 3826514):
    ///   - `survives_explosion`: `"Returns success with 1 ÷ explosion radius probability. Requires explosion radius provided by loot context, and always success if not provided."`
    ///   - `table_bonus`: `"Passes with probability picked from a list, indexed by enchantment power. Requires tool provided by loot context. If not provided, the enchantment level is regarded as 0."`
    ///     Reviewer decision: levels beyond the list use `chances[min(level, len - 1)]`, backed by `Fortune` (rev 3864999), where `"Fortune IV+"` for `"Flint"` from `"Gravel"` shares `"100%"` with Fortune III.
    ///   - `random_chance`: `"Generates a random number between 0.0 and 1.0, and checks if it is less than a specified value."`
    ///   - `match_tool`: `"Checks tool used to mine the block. Requires tool provided by loot context, and always fails if not provided."` (`minecraft:enchantments`: `"Array of conditions. The sub-predicate succeeds when all the conditions succeed."`)
    ///   - `location_check` (`Predicate` rev 3888899, `Advancement/Conditions/location` rev 3684921, `Advancement/Conditions/block` rev 3684708): `"Checks the current location against location criteria. Requires origin provided by loot context, and always fails if not provided."` Queries `LootContext::block_at(offsetX, offsetY, offsetZ)` and matches `predicate.block` (`blocks` and `state`); evaluates to `false` when no neighbour lookup is attached or `block_at` returns `None`. Any other location or block predicate field remains `Unsupported` (`false`).
    ///   - `entity_properties`: reviewer decision — with no entity in [`LootContext`], evaluates to `false`.
    ///   - `Unsupported`: evaluates to `false`.
    /// - **Modifiers** (`Item modifier` rev 3896146, `Fortune` rev 3864999, `Explosion` rev 3903416):
    ///   - `apply_bonus` (`ore_drops`): `"Count *= (max(1; randomInt(0(inclusive) .. (Level + 2)(exclusive))))"` — reviewer decision: `count *= max(1, random_int(0 ..= level + 1))`.
    ///   - `apply_bonus` (`uniform_bonus_count`): `"uniform distribution (from 0 to level * bonusMultiplier)"` — reviewer decision: `count += random_int(0 ..= level * bonusMultiplier)`.
    ///   - `apply_bonus` (`binomial_with_bonus_count`): `Item modifier` rev 3896146 writes `"for 1 bonus item plus bonus determined by binomial distribution with n=level + extra, p=probability"`, whereas `Fortune` rev 3864999 shows `"the binomial distribution in the default case is created by rolling three times (n=3) with a drop probability of 57% (p=4/7), then an additional seed is added"` (`"Average drops"` = `"2.71 (2 5/7)"` = `1 + 3 * 4/7`). Reviewer decision: `count += Binomial(n = level + extra, p = probability)` with **no extra +1**, and missing enchantment = level 0.
    ///   - `explosion_decay`: `Explosion` rev 3903416 states `"Other explosions have a 1/power chance of dropping items."` and `Predicate` rev 3888899 states `"1 ÷ explosion radius probability"`, contradicting `Item modifier` rev 3896146 (`"Each item in the item stack has a chance of 1/explosion radius to be lost."`). Reviewer decision: each item in the stack is **kept** with probability `1 / explosion_radius`, and no explosion radius in context means no decay.
    ///   - `limit_count`: `"Limits the count between a minimum and maximum value."` Reviewer decision: clamps to bounds; counts `<= 0` after all modifiers produce no stack.
    ///   - `copy_components` / `copy_state`: keep the item without inventing component data and record a typed [`LootNote`].
    /// - **Number providers** (`Number provider` rev 3869456):
    ///   - `uniform`: `"The values are inclusive, meaning the provided number can be either min or max, except if the provided number is a float, in which case it must be strictly less than max. If max <= min, the provided number is min."` Reviewer decision: integer `min`/`max` draws an integer uniformly from `min ..= max` inclusive.
    ///   - `binomial`: `"A random number following a binomial distribution."` (`n` = `"The amount of trials"`, `p` = `"The probability of success on an individual trial"`).
    pub fn roll(&self, ctx: &LootContext, random: &mut RandomSource) -> Vec<ItemStack> {
        self.roll_with_notes(ctx, random).items
    }

    /// Evaluates this block loot table under `ctx` and returns both dropped stacks and
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

    /// Evaluates this block loot table using explicit `item_tags`, `block_tags`, and `item_defaults`.
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
        };
        let mut outcome = LootRoll::default();

        for pool in &self.pools {
            if !eval_conditions(&pool.conditions, &eval_ctx, random) {
                continue;
            }
            let rolls = sample_count_provider(&pool.rolls, random);
            if rolls <= 0 {
                continue;
            }

            for _ in 0..rolls {
                let mut candidates = Vec::new();
                for entry in &pool.entries {
                    expand_entry(entry, &eval_ctx, random, &mut candidates);
                }
                if candidates.is_empty() {
                    continue;
                }
                let selected = if candidates.len() == 1 {
                    candidates.swap_remove(0)
                } else {
                    let idx = random.next_int_bounded(candidates.len() as u32) as usize;
                    candidates.swap_remove(idx)
                };

                match selected.kind {
                    SelectedEntryKind::Dynamic(name) => {
                        outcome.notes.push(LootNote::DynamicNotApplied {
                            name: name.to_owned(),
                        });
                    }
                    SelectedEntryKind::Item(item_name) => {
                        let mut count = 1_i32;
                        let all_modifiers = selected
                            .modifiers
                            .into_iter()
                            .chain(pool.modifiers.iter())
                            .chain(self.modifiers.iter());
                        for modifier in all_modifiers {
                            if eval_conditions(modifier.conditions(), &eval_ctx, random) {
                                apply_modifier(
                                    modifier,
                                    item_name,
                                    &mut count,
                                    &eval_ctx,
                                    random,
                                    &mut outcome.notes,
                                );
                            }
                        }
                        if count > 0 {
                            push_item_stacks(item_name, count, item_defaults, &mut outcome.items);
                        }
                    }
                }
            }
        }

        outcome
    }
}

/// Loaded collection of loot tables for one category (for example `blocks` or `entities`).
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

impl LootTables {
    /// Loads every `data/<ns>/loot_table/blocks/**/*.json` file under `root`,
    /// resolves string condition references through `data/<ns>/predicate/**`,
    /// resolves any `loot_table` entry references, and loads `ItemTags`,
    /// `BlockTags`, and optional `ItemDefaults` (when
    /// `reports/minecraft/components/item` is present).
    pub fn load_blocks(root: &Path) -> Result<Self, LootError> {
        Self::load_category(root, "blocks", Some("block"))
    }

    /// Loads every `data/<ns>/loot_table/entities/**/*.json` file under `root`
    /// with table ids `<ns>:entities/<path>`, resolves predicate and
    /// `loot_table` references (including external sub-tables such as
    /// `minecraft:gameplay/fishing/fish`), and loads `ItemTags`, `BlockTags`,
    /// and optional `ItemDefaults`.
    pub fn load_entities(root: &Path) -> Result<Self, LootError> {
        Self::load_category(root, "entities", Some("entity"))
    }

    fn load_category(
        root: &Path,
        category: &str,
        expected_type: Option<&str>,
    ) -> Result<Self, LootError> {
        let data_dir = root.join("data");
        let Some(namespaces) = read_dir_optional(&data_dir)? else {
            return Ok(Self::default());
        };

        let item_tags = Arc::new(ItemTags::load(root)?);
        let block_tags = Arc::new(BlockTags::load(root)?);
        let item_defaults = match ItemDefaults::load(root) {
            Ok(defaults) => Some(Arc::new(defaults)),
            Err(ItemDefaultsError::MissingReports(_)) => None,
            Err(error) => return Err(LootError::ItemDefaults(error)),
        };
        let registries = LootRegistries {
            item_tags,
            block_tags,
            item_defaults,
        };

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

        let mut raw_tables: HashMap<String, (PathBuf, LootTable)> = HashMap::new();
        let mut ordered_ids = Vec::new();
        for ns_path in &ns_dirs {
            let ns = dir_name(ns_path);
            let cat_dir = ns_path.join("loot_table").join(category);
            for file in walk_json(&cat_dir)? {
                let id = format!("{ns}:{category}/{}", relative_stem(&file, &cat_dir));
                let value = read_json(&file)?;
                let mut table =
                    parse_loot_table_inner(&value, &file, &mut pred_resolver, expected_type)?;
                table.registries = registries.clone();
                ordered_ids.push(id.clone());
                raw_tables.insert(id, (file, table));
            }
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

    /// Returns the loaded vanilla item defaults, when present under `reports/`.
    #[must_use]
    pub fn item_defaults(&self) -> Option<&ItemDefaults> {
        self.registries.item_defaults.as_deref()
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
                &["type", "name", "weight", "condition", "modifier"],
                "item entry",
                file,
            )?;
            let name = required_str(obj, "name", "item entry", file)?.to_owned();
            let weight = parse_optional_weight(obj, "item entry", file)?;
            Ok(LootEntry::Item {
                name,
                weight,
                conditions,
                modifiers,
            })
        }
        "loot_table" => {
            check_allowed_keys(
                obj,
                &["type", "value", "weight", "condition", "modifier"],
                "loot_table entry",
                file,
            )?;
            let weight = parse_optional_weight(obj, "loot_table entry", file)?;
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
                conditions,
                modifiers,
            })
        }
        "empty" => {
            check_allowed_keys(
                obj,
                &["type", "weight", "condition", "modifier"],
                "empty entry",
                file,
            )?;
            let weight = parse_optional_weight(obj, "empty entry", file)?;
            Ok(LootEntry::Empty {
                weight,
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
            Ok(LootEntry::Tag {
                tag,
                expand,
                weight,
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
                &["type", "name", "weight", "condition", "modifier"],
                "dynamic entry",
                file,
            )?;
            let name = required_str(obj, "name", "dynamic entry", file)?.to_owned();
            let weight = parse_optional_weight(obj, "dynamic entry", file)?;
            Ok(LootEntry::Dynamic {
                name,
                weight,
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
            let ench_arr = raw_enchs.as_array().ok_or_else(|| {
                invalid(
                    file,
                    format!("{context}.predicates.minecraft:enchantments must be an array"),
                )
            })?;
            for entry in ench_arr {
                enchantments.push(parse_enchantment_predicate(entry, file)?);
            }
        }
    }

    Ok(Some(ToolPredicate {
        items,
        enchantments,
    }))
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
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| parse_single_modifier(item, file, resolver))
            .collect(),
        Value::Object(_) => Ok(vec![parse_single_modifier(value, file, resolver)?]),
        _ => Err(invalid(
            file,
            "modifier must be a JSON object or array of objects",
        )),
    }
}

fn parse_single_modifier(
    value: &Value,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
) -> Result<LootModifier, LootError> {
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
            let add = match obj.get("add") {
                None => false,
                Some(v) => v
                    .as_bool()
                    .ok_or_else(|| invalid(file, "set_count `add` must be a boolean"))?,
            };
            Ok(LootModifier::SetCount {
                count,
                add,
                conditions,
            })
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

fn collect_unsupported_modifier<'a>(modifier: &'a LootModifier, out: &mut Vec<&'a str>) {
    for condition in modifier.conditions() {
        collect_unsupported_condition(condition, out);
    }
    if let LootModifier::Unsupported { what } = modifier {
        out.push(what.as_str());
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
}

enum SelectedEntryKind<'a> {
    Item(&'a str),
    Dynamic(&'a str),
}

struct SelectedEntry<'a> {
    kind: SelectedEntryKind<'a>,
    modifiers: Vec<&'a LootModifier>,
}

fn expand_entry<'a>(
    entry: &'a LootEntry,
    ctx: &EvalCtx<'_>,
    random: &mut RandomSource,
    out: &mut Vec<SelectedEntry<'a>>,
) -> bool {
    if !eval_conditions(entry.conditions(), ctx, random) {
        return false;
    }
    match entry {
        LootEntry::Item {
            name, modifiers, ..
        } => {
            out.push(SelectedEntry {
                kind: SelectedEntryKind::Item(name.as_str()),
                modifiers: modifiers.iter().collect(),
            });
            true
        }
        LootEntry::Dynamic {
            name, modifiers, ..
        } => {
            out.push(SelectedEntry {
                kind: SelectedEntryKind::Dynamic(name.as_str()),
                modifiers: modifiers.iter().collect(),
            });
            true
        }
        LootEntry::LootTable { .. } | LootEntry::Empty { .. } | LootEntry::Tag { .. } => false,
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
        LootCondition::MatchTool { predicate } => match_tool(predicate, ctx),
        // Reviewer decision: with no entity in LootContext, entity_properties is false.
        LootCondition::EntityProperties { .. }
        | LootCondition::DamageSourceProperties { .. }
        | LootCondition::RandomChanceWithEnchantedBonus { .. }
        | LootCondition::KilledByPlayer
        | LootCondition::Unsupported { .. } => false,
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

fn match_tool(predicate: &ToolPredicate, ctx: &EvalCtx<'_>) -> bool {
    let Some(tool) = ctx.loot.tool.as_ref() else {
        return false;
    };
    if !predicate.items.is_empty() {
        if tool.item.is_empty() {
            return false;
        }
        let item_matches = predicate.items.iter().any(|spec| {
            if let Some(tag_id) = spec.strip_prefix('#') {
                let tag = ensure_namespaced(tag_id);
                ctx.item_tags
                    .members(&tag)
                    .is_some_and(|members| members.iter().any(|m| items_equal(m, &tool.item)))
            } else {
                items_equal(spec, &tool.item)
            }
        });
        if !item_matches {
            return false;
        }
    }
    predicate
        .enchantments
        .iter()
        .all(|ench_pred| match_enchantment_predicate(ench_pred, tool))
}

fn match_enchantment_predicate(pred: &EnchantmentPredicate, tool: &ToolContext) -> bool {
    let matches_level = |lvl: u8| -> bool {
        lvl > 0
            && pred.min_level.is_none_or(|min| u32::from(lvl) >= min)
            && pred.max_level.is_none_or(|max| u32::from(lvl) <= max)
    };
    if pred.enchantments.is_empty() {
        tool.enchantments.values().copied().any(matches_level)
    } else {
        pred.enchantments.iter().any(|target| {
            tool.enchantments
                .iter()
                .any(|(id, &lvl)| items_equal(id, target) && matches_level(lvl))
        })
    }
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

fn apply_modifier(
    modifier: &LootModifier,
    item_name: &str,
    count: &mut i32,
    ctx: &EvalCtx<'_>,
    random: &mut RandomSource,
    notes: &mut Vec<LootNote>,
) {
    match modifier {
        LootModifier::SetCount { count: np, add, .. } => {
            let delta = sample_count_provider(np, random);
            if *add {
                *count = count.saturating_add(delta);
            } else {
                *count = delta;
            }
        }
        LootModifier::ExplosionDecay { .. } => {
            if let Some(radius) = ctx.loot.explosion_radius
                && *count > 0
            {
                let mut kept = 0_i32;
                for _ in 0..*count {
                    if survives_explosion_radius(radius, random) {
                        kept += 1;
                    }
                }
                *count = kept;
            }
        }
        LootModifier::ApplyBonus {
            enchantment,
            formula,
            ..
        } => {
            apply_bonus(enchantment, formula, count, ctx.loot.tool.as_ref(), random);
        }
        LootModifier::LimitCount { min, max, .. } => {
            if let Some(lo) = min {
                *count = (*count).max(*lo);
            }
            if let Some(hi) = max {
                *count = (*count).min(*hi);
            }
        }
        LootModifier::CopyComponents {
            source,
            include,
            exclude,
            ..
        } => {
            notes.push(LootNote::CopyComponentsNotApplied {
                item: ensure_namespaced(item_name),
                source: source.clone(),
                include: include.clone(),
                exclude: exclude.clone(),
            });
        }
        LootModifier::CopyState {
            block, properties, ..
        } => {
            notes.push(LootNote::CopyStateNotApplied {
                item: ensure_namespaced(item_name),
                block: block.clone(),
                properties: properties.clone(),
            });
        }
        LootModifier::EnchantedCountIncrease { .. }
        | LootModifier::FurnaceSmelt { .. }
        | LootModifier::SetPotion { .. } => {}
        LootModifier::Unsupported { what } => {
            notes.push(LootNote::UnsupportedModifier {
                item: ensure_namespaced(item_name),
                what: what.clone(),
            });
        }
    }
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
    raw_name: &str,
    count: i32,
    item_defaults: Option<&ItemDefaults>,
    out: &mut Vec<ItemStack>,
) {
    let item_name = ensure_namespaced(raw_name);
    let max_stack = resolve_max_stack(&item_name, item_defaults);
    let mut remaining = count;
    while remaining > 0 {
        let chunk = remaining.min(i32::from(max_stack)) as u16;
        if let Ok(stack) = ItemStack::try_new(item_name.as_str(), chunk, max_stack) {
            out.push(stack);
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
}
