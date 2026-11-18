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

use serde_json::{Map, Value};

use crate::item::normalize_item_id;
use crate::vanilla::worldgen::{WorldgenError, read_dir_optional, read_json, walk_json};

/// Errors encountered while loading or parsing block loot tables.
#[derive(Debug)]
pub enum LootError {
    /// Reading or parsing a JSON file failed.
    Worldgen(WorldgenError),
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

/// Parsed block loot table loaded from `data/<ns>/loot_table/blocks/**/*.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct LootTable {
    /// Optional loot context type string (for example `"minecraft:block"`).
    pub table_type: Option<String>,
    /// Optional deterministic random sequence identifier.
    pub random_sequence: Option<String>,
    /// Ordered loot pools (may be empty if `pools` is omitted).
    pub pools: Vec<LootPool>,
    /// Optional table-level modifiers applied to all drops from this table.
    pub modifiers: Vec<LootModifier>,
}

impl LootTable {
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
}

/// Loaded collection of block loot tables keyed by `<ns>:blocks/<path>`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LootTables {
    tables: BTreeMap<String, LootTable>,
}

impl LootTables {
    /// Loads every `data/<ns>/loot_table/blocks/**/*.json` file under `root`
    /// and resolves string condition references through `data/<ns>/predicate/**`.
    pub fn load_blocks(root: &Path) -> Result<Self, LootError> {
        let data_dir = root.join("data");
        let Some(namespaces) = read_dir_optional(&data_dir)? else {
            return Ok(Self::default());
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
                let table = parse_loot_table(&value, &file, &mut resolver)?;
                tables.insert(id, table);
            }
        }

        Ok(Self { tables })
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

    Ok(LootTable {
        table_type,
        random_sequence,
        pools,
        modifiers,
    })
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
