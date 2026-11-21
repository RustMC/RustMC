//! Data-driven vanilla enchantment definitions loaded from `data/<ns>/enchantment/*.json`.
//!
//! Schema and semantics follow minecraft.wiki `Enchantment definition` (revision 3843772):
//! - `supported_items`: "Items on which this enchantment can be applied using an anvil or
//!   using the `/enchant` command."
//! - `primary_items`: "Items for which this enchantment (if present in `in_enchanting_table`)
//!   appears in an enchanting table. If unspecified, defaults to being the same as
//!   `supported_items`."
//! - `exclusive_set`: "Enchantments that are incompatible with this enchantment. Defaults to
//!   an empty list."
//! - `anvil_cost`: "The base cost when applying this enchantment to another item using an
//!   anvil. Halved when adding using a book, multiplied by the level of the enchantment."
//!
//! The `effects` and `description` fields are deliberately ignored by this loader; any other
//! unknown field is rejected with an error naming the offending file.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::item::{items_equal, valid_item_id};
use crate::vanilla::feature::{EnchantmentTags, ItemTags};
use crate::vanilla::worldgen::{WorldgenError, read_dir_optional, read_json, walk_json};

/// Maximum valid enchantment `max_level` in Java Edition (`1..=255`;
/// minecraft.wiki `Enchantment definition`, revision 3843772).
pub const MAX_ENCHANTMENT_LEVEL: u8 = 255;

/// Maximum valid enchantment `weight` in Java Edition (`1..=1024`;
/// minecraft.wiki `Enchantment definition`, revision 3843772).
pub const MAX_ENCHANTMENT_WEIGHT: u32 = 1024;

// Valid `EquipmentSlotGroup` names per minecraft.wiki `Enchantment definition` (revision 3843772).
const VALID_SLOTS: &[&str] = &[
    "any", "hand", "mainhand", "offhand", "armor", "feet", "legs", "chest", "head", "body",
    "saddle",
];

const ALLOWED_TOP_LEVEL_FIELDS: &[&str] = &[
    "max_level",
    "anvil_cost",
    "weight",
    "min_cost",
    "max_cost",
    "slots",
    "supported_items",
    "primary_items",
    "exclusive_set",
    "effects",
    "description",
];

const ALLOWED_COST_FIELDS: &[&str] = &["base", "per_level_above_first"];

/// Errors returned while loading or constructing enchantment definitions.
#[derive(Debug)]
pub enum EnchantmentError {
    /// Filesystem or JSON error from shared worldgen I/O helpers.
    Worldgen(WorldgenError),
    /// An enchantment JSON file had missing, malformed, out-of-range, or unknown fields.
    Invalid { file: PathBuf, message: String },
}

impl fmt::Display for EnchantmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Worldgen(error) => write!(f, "{error}"),
            Self::Invalid { file, message } => {
                write!(f, "invalid enchantment {}: {message}", file.display())
            }
        }
    }
}

impl std::error::Error for EnchantmentError {}

impl From<WorldgenError> for EnchantmentError {
    fn from(error: WorldgenError) -> Self {
        Self::Worldgen(error)
    }
}

/// Linear enchanting-table cost bounds (`base + per_level_above_first * (level - 1)`).
///
/// Source: minecraft.wiki `Enchantment definition` (revision 3843772):
/// `base`: "The minimum cost for a level I enchantment."
/// `per_level_above_first`: "The amount of levels added to the minimum for each level above
/// level I."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnchantmentCost {
    /// Cost at enchantment level I.
    pub base: i32,
    /// Additional cost per level above level I.
    pub per_level_above_first: i32,
}

impl EnchantmentCost {
    /// Creates an [`EnchantmentCost`] from its base and per-level increment.
    #[must_use]
    pub const fn new(base: i32, per_level_above_first: i32) -> Self {
        Self {
            base,
            per_level_above_first,
        }
    }

    /// Evaluates the cost bound at `level` (clamping `level == 0` to 1).
    #[must_use]
    pub const fn at_level(self, level: u8) -> i32 {
        let extra_levels = if level > 1 { (level - 1) as i32 } else { 0 };
        self.base
            .saturating_add(self.per_level_above_first.saturating_mul(extra_levels))
    }
}

/// Parameters required to construct an [`EnchantmentDefinition`] in code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnchantmentSpec {
    /// Namespaced enchantment identifier (e.g. `"minecraft:sharpness"`).
    pub id: String,
    /// Maximum level (`1..=255`).
    pub max_level: u8,
    /// Base anvil multiplier (`>= 0`).
    pub anvil_cost: u32,
    /// Enchanting table weight (`1..=1024`).
    pub weight: u32,
    /// Minimum enchanting cost curve.
    pub min_cost: EnchantmentCost,
    /// Maximum enchanting cost curve.
    pub max_cost: EnchantmentCost,
    /// Equipment slot groups in which the enchantment activates.
    pub slots: Vec<String>,
    /// Resolved item identifiers accepted by this enchantment on an anvil or `/enchant`.
    pub supported_items: Vec<String>,
    /// Optional resolved item identifiers eligible in an enchanting table.
    pub primary_items: Option<Vec<String>>,
    /// Resolved enchantment identifiers mutually exclusive with this enchantment.
    pub exclusive_set: Vec<String>,
}

/// Parsed definition of a single vanilla or data-pack enchantment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnchantmentDefinition {
    id: String,
    max_level: u8,
    anvil_cost: u32,
    weight: u32,
    min_cost: EnchantmentCost,
    max_cost: EnchantmentCost,
    slots: Vec<String>,
    supported_items: Vec<String>,
    primary_items: Option<Vec<String>>,
    exclusive_set: Vec<String>,
}

impl EnchantmentDefinition {
    /// Constructs a validated [`EnchantmentDefinition`] from an [`EnchantmentSpec`].
    pub fn try_new(spec: EnchantmentSpec) -> Result<Self, EnchantmentError> {
        let file = PathBuf::from(&spec.id);
        if !valid_item_id(&spec.id) {
            return Err(invalid(
                &file,
                format!("invalid enchantment identifier `{}`", spec.id),
            ));
        }
        if spec.max_level == 0 {
            return Err(invalid(&file, "`max_level` must be in 1..=255"));
        }
        if !(1..=MAX_ENCHANTMENT_WEIGHT).contains(&spec.weight) {
            return Err(invalid(
                &file,
                format!(
                    "`weight` {} must be in 1..={MAX_ENCHANTMENT_WEIGHT}",
                    spec.weight
                ),
            ));
        }
        for slot in &spec.slots {
            if !VALID_SLOTS.contains(&slot.as_str()) {
                return Err(invalid(&file, format!("unknown slot `{slot}`")));
            }
        }
        Ok(Self {
            id: spec.id,
            max_level: spec.max_level,
            anvil_cost: spec.anvil_cost,
            weight: spec.weight,
            min_cost: spec.min_cost,
            max_cost: spec.max_cost,
            slots: spec.slots,
            supported_items: spec.supported_items,
            primary_items: spec.primary_items,
            exclusive_set: spec.exclusive_set,
        })
    }

    /// Namespaced enchantment identifier (e.g. `"minecraft:sharpness"`).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Maximum enchantment level (`1..=255`).
    #[must_use]
    pub const fn max_level(&self) -> u8 {
        self.max_level
    }

    /// Base anvil cost multiplier before book halving.
    #[must_use]
    pub const fn anvil_cost(&self) -> u32 {
        self.anvil_cost
    }

    /// Relative selection weight (`1..=1024`).
    #[must_use]
    pub const fn weight(&self) -> u32 {
        self.weight
    }

    /// Minimum level-cost curve.
    #[must_use]
    pub const fn min_cost(&self) -> EnchantmentCost {
        self.min_cost
    }

    /// Maximum level-cost curve.
    #[must_use]
    pub const fn max_cost(&self) -> EnchantmentCost {
        self.max_cost
    }

    /// Equipment slot groups where this enchantment is active.
    #[must_use]
    pub fn slots(&self) -> &[String] {
        &self.slots
    }

    /// Resolved list of item identifiers supporting this enchantment (e.g. via anvil).
    #[must_use]
    pub fn supported_items(&self) -> &[String] {
        &self.supported_items
    }

    /// Resolved list of primary item identifiers (defaults to [`Self::supported_items`]
    /// when `primary_items` is omitted in JSON, per minecraft.wiki `Enchantment definition`,
    /// revision 3843772).
    #[must_use]
    pub fn primary_items(&self) -> &[String] {
        self.primary_items
            .as_deref()
            .unwrap_or(&self.supported_items)
    }

    /// Returns the explicit `primary_items` list if specified in the JSON definition,
    /// or `None` if it defaulted to [`Self::supported_items`].
    #[must_use]
    pub fn explicit_primary_items(&self) -> Option<&[String]> {
        self.primary_items.as_deref()
    }

    /// Resolved list of enchantment identifiers mutually exclusive with this enchantment.
    #[must_use]
    pub fn exclusive_set(&self) -> &[String] {
        &self.exclusive_set
    }
}

/// Registry of loaded enchantment definitions keyed by namespaced identifier.
///
/// Ignores `effects` and `description` fields in enchantment JSON files while rejecting
/// any other unknown top-level or cost field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnchantmentRegistry {
    entries: BTreeMap<String, EnchantmentDefinition>,
}

impl EnchantmentRegistry {
    /// Loads every `data/<ns>/enchantment/**/*.json` under `root`, resolving item tags
    /// against `item_tags` and enchantment tags against `enchantment_tags`.
    ///
    /// Ignores `effects` and `description`; any other unknown field is returned as
    /// [`EnchantmentError::Invalid`] naming the file.
    pub fn load(
        root: &Path,
        item_tags: &ItemTags,
        enchantment_tags: &EnchantmentTags,
    ) -> Result<Self, EnchantmentError> {
        let mut registry = Self::default();
        let data_dir = root.join("data");
        let Some(namespaces) = read_dir_optional(&data_dir)? else {
            return Ok(registry);
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

        for ns_path in ns_dirs {
            let ns = ns_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ench_dir = ns_path.join("enchantment");
            for file in walk_json(&ench_dir)? {
                let relative = file
                    .strip_prefix(&ench_dir)
                    .unwrap_or(file.as_path())
                    .with_extension("");
                let id = format!("{ns}:{}", relative.to_string_lossy().replace('\\', "/"));
                let def = parse_enchantment(&id, &file, item_tags, enchantment_tags)?;
                registry.entries.insert(id, def);
            }
        }

        Ok(registry)
    }

    /// Inserts or replaces an [`EnchantmentDefinition`] in this registry.
    pub fn register(&mut self, def: EnchantmentDefinition) {
        self.entries.insert(def.id.clone(), def);
    }

    /// Looks up an enchantment definition by identifier.
    /// Accepts identifiers with or without an explicit `minecraft:` namespace prefix.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&EnchantmentDefinition> {
        if let Some(def) = self.entries.get(id) {
            return Some(def);
        }
        if !id.contains(':') {
            return self.entries.get(&format!("minecraft:{id}"));
        }
        None
    }

    /// Number of enchantments in the registry.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when the registry contains no enchantments.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterates over `(id, definition)` pairs sorted by identifier.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &EnchantmentDefinition)> {
        self.entries.iter().map(|(id, def)| (id.as_str(), def))
    }

    /// Returns `true` if enchantments `a` and `b` are both known, are not the same
    /// enchantment, and neither appears in the other's resolved `exclusive_set`.
    #[must_use]
    pub fn are_compatible(&self, a: &str, b: &str) -> bool {
        let (Some(def_a), Some(def_b)) = (self.get(a), self.get(b)) else {
            return false;
        };
        if def_a.id == def_b.id {
            return false;
        }
        let a_excludes_b = def_a.exclusive_set.iter().any(|ex| ex == &def_b.id);
        let b_excludes_a = def_b.exclusive_set.iter().any(|ex| ex == &def_a.id);
        !a_excludes_b && !b_excludes_a
    }

    /// Returns `true` if `enchantment` is known and `item` is in its resolved
    /// `supported_items` list (per minecraft.wiki `Enchantment definition`,
    /// revision 3843772, `supported_items` governs anvil application).
    #[must_use]
    pub fn can_apply(&self, enchantment: &str, item: &str) -> bool {
        let Some(def) = self.get(enchantment) else {
            return false;
        };
        def.supported_items
            .iter()
            .any(|supported| items_equal(supported, item))
    }
}

fn invalid(file: &Path, message: impl Into<String>) -> EnchantmentError {
    EnchantmentError::Invalid {
        file: file.to_path_buf(),
        message: message.into(),
    }
}

fn reject_unknown_keys(
    obj: &serde_json::Map<String, Value>,
    allowed: &[&str],
    context: &str,
    file: &Path,
) -> Result<(), EnchantmentError> {
    if let Some(unknown) = obj.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid(
            file,
            format!("unknown field `{unknown}` in {context}"),
        ));
    }
    Ok(())
}

fn parse_enchantment(
    id: &str,
    file: &Path,
    item_tags: &ItemTags,
    enchantment_tags: &EnchantmentTags,
) -> Result<EnchantmentDefinition, EnchantmentError> {
    if !valid_item_id(id) {
        return Err(invalid(
            file,
            format!("invalid enchantment identifier `{id}`"),
        ));
    }

    let document = read_json(file)?;
    let obj = document
        .as_object()
        .ok_or_else(|| invalid(file, "enchantment root must be a JSON object"))?;

    reject_unknown_keys(
        obj,
        ALLOWED_TOP_LEVEL_FIELDS,
        "enchantment definition",
        file,
    )?;

    let raw_max_level = required_u64(obj, "max_level", file)?;
    let max_level = u8::try_from(raw_max_level)
        .ok()
        .filter(|&level| level >= 1)
        .ok_or_else(|| {
            invalid(
                file,
                format!("`max_level` {raw_max_level} must be in 1..={MAX_ENCHANTMENT_LEVEL}"),
            )
        })?;

    let raw_anvil_cost = required_u64(obj, "anvil_cost", file)?;
    let anvil_cost = u32::try_from(raw_anvil_cost).map_err(|_| {
        invalid(
            file,
            format!("`anvil_cost` {raw_anvil_cost} must fit in an unsigned 32-bit integer"),
        )
    })?;

    let raw_weight = required_u64(obj, "weight", file)?;
    let weight = u32::try_from(raw_weight)
        .ok()
        .filter(|&w| (1..=MAX_ENCHANTMENT_WEIGHT).contains(&w))
        .ok_or_else(|| {
            invalid(
                file,
                format!("`weight` {raw_weight} must be in 1..={MAX_ENCHANTMENT_WEIGHT}"),
            )
        })?;

    let min_cost = parse_cost(obj, "min_cost", file)?;
    let max_cost = parse_cost(obj, "max_cost", file)?;
    let slots = parse_slots(obj, file)?;

    let supported_val = obj
        .get("supported_items")
        .ok_or_else(|| invalid(file, "missing required `supported_items` field"))?;
    let supported_items =
        resolve_holder_set(supported_val, "supported_items", "item", file, |tag| {
            item_tags.members(tag)
        })?;

    let primary_items = match obj.get("primary_items") {
        None => None,
        Some(value) => Some(resolve_holder_set(
            value,
            "primary_items",
            "item",
            file,
            |tag| item_tags.members(tag),
        )?),
    };

    let exclusive_set = match obj.get("exclusive_set") {
        None => Vec::new(),
        Some(value) => resolve_holder_set(value, "exclusive_set", "enchantment", file, |tag| {
            enchantment_tags.members(tag)
        })?,
    };

    Ok(EnchantmentDefinition {
        id: id.to_owned(),
        max_level,
        anvil_cost,
        weight,
        min_cost,
        max_cost,
        slots,
        supported_items,
        primary_items,
        exclusive_set,
    })
}

fn required_u64(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    file: &Path,
) -> Result<u64, EnchantmentError> {
    obj.get(key).and_then(Value::as_u64).ok_or_else(|| {
        invalid(
            file,
            format!("missing or non-negative-integer `{key}` field"),
        )
    })
}

fn parse_cost(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    file: &Path,
) -> Result<EnchantmentCost, EnchantmentError> {
    let cost_obj = obj
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(file, format!("missing or non-object `{key}` field")))?;
    reject_unknown_keys(cost_obj, ALLOWED_COST_FIELDS, key, file)?;

    let base_raw = cost_obj
        .get("base")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(file, format!("missing or non-integer `{key}.base`")))?;
    let base = i32::try_from(base_raw)
        .map_err(|_| invalid(file, format!("`{key}.base` {base_raw} out of i32 range")))?;

    let per_raw = cost_obj
        .get("per_level_above_first")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            invalid(
                file,
                format!("missing or non-integer `{key}.per_level_above_first`"),
            )
        })?;
    let per_level_above_first = i32::try_from(per_raw).map_err(|_| {
        invalid(
            file,
            format!("`{key}.per_level_above_first` {per_raw} out of i32 range"),
        )
    })?;

    Ok(EnchantmentCost::new(base, per_level_above_first))
}

fn parse_slots(
    obj: &serde_json::Map<String, Value>,
    file: &Path,
) -> Result<Vec<String>, EnchantmentError> {
    let arr = obj
        .get("slots")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(file, "missing or non-array `slots` field"))?;
    let mut slots = Vec::with_capacity(arr.len());
    for elem in arr {
        let slot = elem
            .as_str()
            .ok_or_else(|| invalid(file, "`slots` entries must be strings"))?;
        if !VALID_SLOTS.contains(&slot) {
            return Err(invalid(file, format!("unknown slot `{slot}` in `slots`")));
        }
        slots.push(slot.to_owned());
    }
    Ok(slots)
}

fn resolve_holder_set<'a>(
    value: &Value,
    field: &str,
    tag_kind: &str,
    file: &Path,
    tag_lookup: impl Fn(&str) -> Option<&'a [String]>,
) -> Result<Vec<String>, EnchantmentError> {
    let mut resolved = Vec::new();
    let mut seen = HashSet::new();

    let mut push_id = |id: &str| -> Result<(), EnchantmentError> {
        if !valid_item_id(id) {
            return Err(invalid(
                file,
                format!("invalid {tag_kind} id `{id}` in `{field}`"),
            ));
        }
        if seen.insert(id.to_owned()) {
            resolved.push(id.to_owned());
        }
        Ok(())
    };

    match value {
        Value::String(raw) => {
            if let Some(tag) = raw.strip_prefix('#') {
                if !valid_item_id(tag) {
                    return Err(invalid(
                        file,
                        format!("invalid {tag_kind} tag `{raw}` in `{field}`"),
                    ));
                }
                let members = tag_lookup(tag).ok_or_else(|| {
                    invalid(file, format!("unknown {tag_kind} tag `{raw}` in `{field}`"))
                })?;
                for member in members {
                    push_id(member)?;
                }
            } else {
                push_id(raw)?;
            }
        }
        Value::Array(arr) => {
            for elem in arr {
                let raw = elem.as_str().ok_or_else(|| {
                    invalid(file, format!("`{field}` list entries must be strings"))
                })?;
                push_id(raw)?;
            }
        }
        _ => {
            return Err(invalid(
                file,
                format!("`{field}` must be an id string, #tag string, or list of id strings"),
            ));
        }
    }

    resolved.sort();
    Ok(resolved)
}

#[cfg(test)]
mod tests;
