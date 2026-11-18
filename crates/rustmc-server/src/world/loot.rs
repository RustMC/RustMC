//! Typed block loot table model and runtime loader for Java 26.3 datapacks.
//!
//! Reads `data/<namespace>/loot_table/blocks/**/*.json` from an
//! operator-provisioned vanilla data root and resolves named predicate strings
//! against `data/<namespace>/predicate/**/*.json`.
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
//!   `location_check` occurs 4 times (`large_fern`, `tall_grass`, parsed as
//!   [`LootCondition::Unsupported`]).
//!
//! Schema definitions follow `minecraft.wiki`:
//! - `Loot table` (revision 3893957): root fields `type`, `modifier`, `pools`,
//!   `random_sequence`; pool fields `condition`, `modifier`, `rolls`, `entries`;
//!   entry types `item`, `alternatives`, `dynamic`.
//! - `Predicate` (revision 3888899): `all_of`, `any_of`, `inverted`,
//!   `match_block`, `match_tool`, `random_chance`, `survives_explosion`,
//!   `table_bonus`, `entity_properties`.
//! - `Item modifier` (revision 3896146): `apply_bonus`, `copy_components`,
//!   `copy_state`, `explosion_decay`, `limit_count`, `set_count`.
//! - `Number provider` (revision 3869456): constant numbers, `uniform`, and
//!   `binomial`.

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

/// Errors encountered while loading or parsing block loot tables.
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

/// Predicate condition attached to pools, entries, or modifiers.
///
/// Mirrors `minecraft.wiki` `Predicate` (revision 3888899). Unsupported
/// condition types (such as `minecraft:location_check`) are retained as
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
    /// `minecraft:match_tool` — checks breaking tool item/tag and enchantments.
    MatchTool { predicate: ToolPredicate },
    /// `minecraft:entity_properties` — checks entity context properties (`entity`, `predicate`).
    EntityProperties {
        entity: String,
        predicate: BTreeMap<String, Value>,
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
            | Self::ExplosionDecay { conditions }
            | Self::ApplyBonus { conditions, .. }
            | Self::LimitCount { conditions, .. }
            | Self::CopyComponents { conditions, .. }
            | Self::CopyState { conditions, .. } => conditions,
            Self::Unsupported { .. } => &[],
        }
    }
}

/// Entry inside a [`LootPool`].
///
/// Mirrors `minecraft.wiki` `Loot table` (revision 3893957): `"alternatives —
/// Extracts only the first successful (conditions are met) entry."`, `"item —
/// Drops a single item stack."`, and `"dynamic — Drops block-specific loot."`
#[derive(Debug, Clone, PartialEq)]
pub enum LootEntry {
    /// `minecraft:item` — drops a single item stack.
    Item {
        name: String,
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

/// Execution context for evaluating a block [`LootTable`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LootContext {
    /// Broken block state as `(block_id, properties)`.
    pub block_state: (String, BTreeMap<String, String>),
    /// Breaking tool, or `None` when broken bare-handed or without a tool.
    pub tool: Option<ToolContext>,
    /// Explosion radius when destroyed by an explosion, or `None` otherwise.
    pub explosion_radius: Option<f32>,
}

impl LootContext {
    /// Builds a bare-handed, non-explosion context for `block_id` with empty properties.
    #[must_use]
    pub fn for_block(block_id: impl Into<String>) -> Self {
        Self {
            block_state: (block_id.into(), BTreeMap::new()),
            tool: None,
            explosion_radius: None,
        }
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
    ///   - `entity_properties`: reviewer decision — with no entity in [`LootContext`], evaluates to `false`.
    ///   - `Unsupported` (including `minecraft:location_check`): evaluates to `false`.
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

/// Loaded collection of block loot tables keyed by `<ns>:blocks/<path>`.
#[derive(Debug, Clone, Default)]
pub struct LootTables {
    tables: BTreeMap<String, LootTable>,
    registries: LootRegistries,
}

impl PartialEq for LootTables {
    fn eq(&self, other: &Self) -> bool {
        self.tables == other.tables
    }
}

impl LootTables {
    /// Loads every `data/<ns>/loot_table/blocks/**/*.json` file under `root`,
    /// resolves string condition references through `data/<ns>/predicate/**`,
    /// and loads `ItemTags`, `BlockTags`, and optional `ItemDefaults` (when
    /// `reports/minecraft/components/item` is present).
    pub fn load_blocks(root: &Path) -> Result<Self, LootError> {
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

        let mut resolver = PredicateResolver {
            root,
            paths: predicate_paths,
            cache: HashMap::new(),
        };

        let mut tables = BTreeMap::new();
        for ns_path in ns_dirs {
            let ns = dir_name(&ns_path);
            let blocks_dir = ns_path.join("loot_table").join("blocks");
            for file in walk_json(&blocks_dir)? {
                let id = format!("{ns}:blocks/{}", relative_stem(&file, &blocks_dir));
                let value = read_json(&file)?;
                let mut table = parse_loot_table(&value, &file, &mut resolver)?;
                table.registries = registries.clone();
                tables.insert(id, table);
            }
        }

        Ok(Self { tables, registries })
    }

    /// Looks up the loot table for `block_id`.
    ///
    /// Accepts `"minecraft:stone"`, `"stone"`, or `"minecraft:blocks/stone"`
    /// (and similarly `"ns:block"` or `"ns:blocks/block"` for custom namespaces).
    #[must_use]
    pub fn get(&self, block_id: &str) -> Option<&LootTable> {
        let key = normalize_block_table_id(block_id);
        self.tables.get(&key)
    }

    /// Rolls the loot table for `block_id` if present, returning stacks and typed notes.
    pub fn roll(
        &self,
        block_id: &str,
        ctx: &LootContext,
        random: &mut RandomSource,
    ) -> Option<LootRoll> {
        self.get(block_id)
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

    /// Returns the number of loaded block loot tables.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tables.len()
    }

    /// Returns `true` when no block loot tables were loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    /// Iterates over `(table_id, table)` pairs in deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &LootTable)> {
        self.tables.iter().map(|(id, table)| (id.as_str(), table))
    }

    /// Lists `(table_id, what)` for every [`LootCondition::Unsupported`] and
    /// [`LootModifier::Unsupported`] node across all loaded tables.
    #[must_use]
    pub fn unsupported(&self) -> Vec<(&str, &str)> {
        let mut out = Vec::new();
        for (id, table) in &self.tables {
            for what in table.unsupported() {
                out.push((id.as_str(), what));
            }
        }
        out
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
        validate_predicate_id(&normalized, table_file)?;
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

fn normalize_block_table_id(block_id: &str) -> String {
    if let Some((ns, path)) = block_id.split_once(':') {
        if path.starts_with("blocks/") {
            format!("{ns}:{path}")
        } else {
            format!("{ns}:blocks/{path}")
        }
    } else {
        format!("minecraft:blocks/{block_id}")
    }
}

fn validate_predicate_id(id: &str, file: &Path) -> Result<(), LootError> {
    let Some((ns, path)) = id.split_once(':') else {
        return Err(invalid(file, format!("invalid predicate id `{id}`")));
    };
    if ns.is_empty()
        || path.is_empty()
        || ns.contains("..")
        || path.split('/').any(|seg| seg.is_empty() || seg == "..")
    {
        return Err(invalid(file, format!("invalid predicate id `{id}`")));
    }
    Ok(())
}

fn expected_predicate_path(root: &Path, id: &str) -> PathBuf {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    let mut out = root.join("data").join(ns).join("predicate");
    for segment in path.split('/') {
        out.push(segment);
    }
    out.set_extension("json");
    out
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

fn parse_loot_table(
    value: &Value,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
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
    if let Some(ref kind) = table_type
        && normalize_item_id(kind) != "block"
    {
        return Err(invalid(
            file,
            format!("expected block loot table type, found `{kind}`"),
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
        .map(|entry| parse_entry(entry, file, resolver))
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

fn parse_entry(
    value: &Value,
    file: &Path,
    resolver: &mut PredicateResolver<'_>,
) -> Result<LootEntry, LootError> {
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
                &["type", "name", "condition", "modifier"],
                "item entry",
                file,
            )?;
            let name = required_str(obj, "name", "item entry", file)?.to_owned();
            Ok(LootEntry::Item {
                name,
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
                .map(|child| parse_entry(child, file, resolver))
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
                &["type", "name", "condition", "modifier"],
                "dynamic entry",
                file,
            )?;
            let name = required_str(obj, "name", "dynamic entry", file)?.to_owned();
            Ok(LootEntry::Dynamic {
                name,
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
        "match_tool" => parse_match_tool(obj, file),
        "entity_properties" => parse_entity_properties(obj, file),
        _ => Ok(LootCondition::Unsupported {
            what: kind.to_owned(),
        }),
    }
}

fn parse_match_block(obj: &Map<String, Value>, file: &Path) -> Result<LootCondition, LootError> {
    check_allowed_keys(
        obj,
        &["type", "blocks", "state"],
        "match_block condition",
        file,
    )?;
    let blocks = match obj.get("blocks") {
        None => Vec::new(),
        Some(raw) => parse_string_or_list(raw, "match_block.blocks", file)?,
    };
    let mut state = BTreeMap::new();
    if let Some(raw_state) = obj.get("state") {
        let state_obj = raw_state
            .as_object()
            .ok_or_else(|| invalid(file, "match_block `state` must be a JSON object"))?;
        for (prop, val) in state_obj {
            let val_str = val.as_str().ok_or_else(|| {
                invalid(
                    file,
                    format!("match_block state `{prop}` value must be a string"),
                )
            })?;
            state.insert(prop.clone(), val_str.to_owned());
        }
    }
    Ok(LootCondition::MatchBlock { blocks, state })
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

fn parse_match_tool(obj: &Map<String, Value>, file: &Path) -> Result<LootCondition, LootError> {
    check_allowed_keys(obj, &["type", "predicate"], "match_tool condition", file)?;
    let pred_obj = obj
        .get("predicate")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(file, "match_tool missing or non-object `predicate`"))?;
    check_allowed_keys(
        pred_obj,
        &["items", "predicates"],
        "match_tool.predicate",
        file,
    )?;

    let items = match pred_obj.get("items") {
        None => Vec::new(),
        Some(raw_items) => parse_string_or_list(raw_items, "match_tool.predicate.items", file)?,
    };

    let mut enchantments = Vec::new();
    if let Some(raw_preds) = pred_obj.get("predicates") {
        let preds_obj = raw_preds.as_object().ok_or_else(|| {
            invalid(
                file,
                "match_tool.predicate.predicates must be a JSON object",
            )
        })?;
        check_allowed_keys(
            preds_obj,
            &["minecraft:enchantments"],
            "match_tool.predicate.predicates",
            file,
        )?;
        if let Some(raw_enchs) = preds_obj.get("minecraft:enchantments") {
            let ench_arr = raw_enchs.as_array().ok_or_else(|| {
                invalid(
                    file,
                    "match_tool.predicate.predicates.minecraft:enchantments must be an array",
                )
            })?;
            for entry in ench_arr {
                enchantments.push(parse_enchantment_predicate(entry, file)?);
            }
        }
    }

    Ok(LootCondition::MatchTool {
        predicate: ToolPredicate {
            items,
            enchantments,
        },
    })
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

fn parse_entity_properties(
    obj: &Map<String, Value>,
    file: &Path,
) -> Result<LootCondition, LootError> {
    check_allowed_keys(
        obj,
        &["type", "entity", "predicate"],
        "entity_properties condition",
        file,
    )?;
    let entity = required_str(obj, "entity", "entity_properties condition", file)?.to_owned();
    let pred_obj = obj
        .get("predicate")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(file, "entity_properties missing or non-object `predicate`"))?;
    let predicate = pred_obj
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Ok(LootCondition::EntityProperties { entity, predicate })
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
        | LootCondition::TableBonus { .. }
        | LootCondition::RandomChance { .. }
        | LootCondition::MatchTool { .. }
        | LootCondition::EntityProperties { .. } => {}
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
        LootCondition::EntityProperties { .. } | LootCondition::Unsupported { .. } => false,
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
    if !blocks.is_empty() {
        if block_id.is_empty() {
            return false;
        }
        let id_matches = blocks.iter().any(|spec| {
            if let Some(tag_id) = spec.strip_prefix('#') {
                let tag = ensure_namespaced(tag_id);
                ctx.block_tags
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
        for (table_id, what) in &unsupported {
            println!("unsupported: {table_id} -> {what}");
        }
        assert_eq!(
            unsupported,
            vec![
                ("minecraft:blocks/large_fern", "minecraft:location_check"),
                ("minecraft:blocks/large_fern", "minecraft:location_check"),
                ("minecraft:blocks/tall_grass", "minecraft:location_check"),
                ("minecraft:blocks/tall_grass", "minecraft:location_check"),
            ]
        );
    }
}
