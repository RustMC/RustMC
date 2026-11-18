//! A counted stack of one item kind, with invariants checked at construction.
//! Per-item components (durability, names, enchantments) are deliberately not
//! modelled yet.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::vanilla::feature::{BlockTags, ItemTags};
use crate::vanilla::worldgen::{WorldgenError, read_dir_optional, read_json, walk_json};

/// Default stack limit for standard items.
pub const DEFAULT_MAX_STACK: u16 = 64;

/// Upper bound of the item max_stack_size component since Java 1.20.5.
pub const MAX_STACK_LIMIT: u16 = 99;

/// Default mining speed multiplier when an item has no `minecraft:tool` component,
/// omits `default_mining_speed`, or matches no rule overriding `speed`.
///
/// Source: minecraft.wiki `Data component format` (revision 3903955,
/// `Data component format/tool` revision 2910648):
/// "`default_mining_speed`: The default mining speed of this tool, used if no rules override it. Defaults to 1.0."
pub const DEFAULT_TOOL_MINING_SPEED: f32 = 1.0;

/// Strips an optional `minecraft:` namespace prefix for canonical item comparison.
#[inline]
#[must_use]
pub fn normalize_item_id(item: &str) -> &str {
    item.strip_prefix("minecraft:").unwrap_or(item)
}

/// Returns true if two item identifiers match canonically regardless of `minecraft:` prefix.
#[inline]
#[must_use]
pub fn items_equal(a: &str, b: &str) -> bool {
    normalize_item_id(a) == normalize_item_id(b)
}

/// Returns the standard default maximum stack size for common items.
#[inline]
#[must_use]
pub fn default_max_stack(item: &str) -> u16 {
    match normalize_item_id(item) {
        "lava_bucket" | "water_bucket" | "milk_bucket" | "potion" | "splash_potion"
        | "lingering_potion" => 1,
        "bucket" | "snowball" | "ender_pearl" | "egg" => 16,
        _ => DEFAULT_MAX_STACK,
    }
}

/// Rejection reasons when constructing an [`ItemStack`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStackError {
    /// Item identifier is not a valid namespaced resource location.
    InvalidItem,
    /// Maximum stack size is outside `1..=MAX_STACK_LIMIT`.
    InvalidMaxStack(u16),
    /// Stack count is zero or exceeds the stack's maximum size.
    InvalidCount(u16),
}

impl std::fmt::Display for ItemStackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidItem => f.write_str("invalid namespaced item id"),
            Self::InvalidMaxStack(max) => {
                write!(f, "max stack size {max} must be in 1..={MAX_STACK_LIMIT}")
            }
            Self::InvalidCount(count) => write!(f, "invalid item stack count {count}"),
        }
    }
}

impl std::error::Error for ItemStackError {}

/// Non-empty, capacity-bounded stack of a single item kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemStack {
    item: String,
    count: u16,
    max_stack: u16,
}

// Java Edition resource-location character rules.
pub(crate) fn valid_item_id(id: &str) -> bool {
    let Some((namespace, path)) = id.split_once(':') else {
        return false;
    };
    let is_namespace_byte = |b: u8| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.');
    !namespace.is_empty()
        && !path.is_empty()
        && namespace.bytes().all(is_namespace_byte)
        && path.bytes().all(|b| is_namespace_byte(b) || b == b'/')
}

impl ItemStack {
    /// Constructs a validated stack, checking the item id, max stack size, and count in order.
    pub fn try_new(
        item: impl Into<String>,
        count: u16,
        max_stack: u16,
    ) -> Result<Self, ItemStackError> {
        let item = item.into();
        if !valid_item_id(&item) {
            return Err(ItemStackError::InvalidItem);
        }
        if !(1..=MAX_STACK_LIMIT).contains(&max_stack) {
            return Err(ItemStackError::InvalidMaxStack(max_stack));
        }
        if !(1..=max_stack).contains(&count) {
            return Err(ItemStackError::InvalidCount(count));
        }
        Ok(Self {
            item,
            count,
            max_stack,
        })
    }

    /// Returns the namespaced item identifier.
    pub fn item(&self) -> &str {
        &self.item
    }

    /// Returns the current item count in this stack.
    pub fn count(&self) -> u16 {
        self.count
    }

    /// Returns the maximum stack size for this stack.
    pub fn max_stack(&self) -> u16 {
        self.max_stack
    }

    /// Reports whether `other` shares the same item identifier and maximum stack size.
    pub fn can_stack_with(&self, other: &Self) -> bool {
        self.item == other.item && self.max_stack == other.max_stack
    }

    /// Adds as many items as fit under `max_stack` and returns the overflow.
    pub fn add(&mut self, amount: u16) -> u16 {
        let space = self.max_stack.saturating_sub(self.count);
        let accepted = amount.min(space);
        self.count = self.count.saturating_add(accepted);
        amount - accepted
    }

    /// Splits up to `amount` items off into a new stack and returns any remaining stack.
    pub fn split(self, amount: u16) -> Result<(Self, Option<Self>), Self> {
        if amount == 0 {
            return Err(self);
        }
        let take = amount.min(self.count);
        let remaining = self.count - take;
        if remaining == 0 {
            return Ok((self, None));
        }
        let taken = Self {
            item: self.item.clone(),
            count: take,
            max_stack: self.max_stack,
        };
        let rest = Self {
            item: self.item,
            count: remaining,
            max_stack: self.max_stack,
        };
        Ok((taken, Some(rest)))
    }
}

/// Errors encountered while loading default item components from vanilla server reports.
#[derive(Debug)]
pub enum ItemDefaultsError {
    /// The `reports` directory or its `minecraft/components/item` subdirectory is missing.
    MissingReports(PathBuf),
    /// Reading or parsing a report JSON file failed.
    Worldgen(WorldgenError),
    /// A report JSON file had missing, malformed, or out-of-range fields.
    Invalid { file: PathBuf, message: String },
}

impl fmt::Display for ItemDefaultsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingReports(path) => write!(
                f,
                "missing vanilla reports directory `{}` (generate with: java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports)",
                path.display()
            ),
            Self::Worldgen(error) => write!(f, "{error}"),
            Self::Invalid { file, message } => {
                write!(f, "invalid item report {}: {message}", file.display())
            }
        }
    }
}

impl std::error::Error for ItemDefaultsError {}

impl From<WorldgenError> for ItemDefaultsError {
    fn from(error: WorldgenError) -> Self {
        Self::Worldgen(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IdSet {
    Id(String),
    Tag(String),
    List(Vec<String>),
}

impl IdSet {
    fn matches_block(&self, block: &str, tags: &BlockTags) -> bool {
        match self {
            Self::Id(spec) => items_equal(spec, block),
            Self::Tag(raw_tag) => {
                let tag_id = raw_tag.strip_prefix('#').unwrap_or(raw_tag);
                tags.members(tag_id)
                    .is_some_and(|members| members.iter().any(|m| items_equal(m, block)))
            }
            Self::List(list) => list.iter().any(|spec| items_equal(spec, block)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ToolRule {
    blocks: IdSet,
    speed: Option<f32>,
    correct_for_drops: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
struct ToolComponent {
    default_mining_speed: f32,
    rules: Vec<ToolRule>,
}

#[derive(Debug, Clone, PartialEq)]
struct ItemDefaultEntry {
    max_stack_size: u8,
    max_damage: Option<u32>,
    repairable: Option<IdSet>,
    enchantability: Option<u32>,
    tool: Option<ToolComponent>,
}

/// Default per-item components read from `reports/minecraft/components/item/*.json`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemDefaults {
    entries: BTreeMap<String, ItemDefaultEntry>,
}

impl ItemDefaults {
    /// Loads default item components from every `reports/minecraft/components/item/*.json`
    /// under `root`.
    pub fn load(root: &Path) -> Result<Self, ItemDefaultsError> {
        let reports_dir = root.join("reports");
        if read_dir_optional(&reports_dir)?.is_none() {
            return Err(ItemDefaultsError::MissingReports(reports_dir));
        }
        let items_dir = reports_dir
            .join("minecraft")
            .join("components")
            .join("item");
        if read_dir_optional(&items_dir)?.is_none() {
            return Err(ItemDefaultsError::MissingReports(items_dir));
        }

        let mut entries = BTreeMap::new();
        for path in walk_json(&items_dir)? {
            let (id, entry) = parse_item_report(&path)?;
            entries.insert(id, entry);
        }
        Ok(Self { entries })
    }

    /// Returns the default `minecraft:max_stack_size` component for `item`, if known.
    /// Accepts identifiers with or without the `minecraft:` prefix.
    #[must_use]
    pub fn max_stack_size(&self, item: &str) -> Option<u8> {
        self.entries
            .get(normalize_item_id(item))
            .map(|entry| entry.max_stack_size)
    }

    /// Returns the default `minecraft:max_damage` component for `item`, if present.
    /// Accepts identifiers with or without the `minecraft:` prefix.
    #[must_use]
    pub fn max_damage(&self, item: &str) -> Option<u32> {
        self.entries
            .get(normalize_item_id(item))
            .and_then(|entry| entry.max_damage)
    }

    /// Returns the resolved repair material identifiers for `item` from its
    /// `minecraft:repairable` component using `tags`, or `None` if `item` has no
    /// `minecraft:repairable` entry (or references an unloaded tag).
    /// Accepts identifiers with or without the `minecraft:` prefix.
    #[must_use]
    pub fn repair_materials(&self, item: &str, tags: &ItemTags) -> Option<Vec<String>> {
        let entry = self.entries.get(normalize_item_id(item))?;
        match entry.repairable.as_ref()? {
            IdSet::Id(id) => Some(vec![id.clone()]),
            IdSet::Tag(raw_tag) => {
                let tag_id = raw_tag.strip_prefix('#').unwrap_or(raw_tag);
                tags.members(tag_id).map(|members| members.to_vec())
            }
            IdSet::List(list) => Some(list.clone()),
        }
    }

    /// Returns the default `minecraft:enchantable` value for `item`, if present.
    /// Accepts identifiers with or without the `minecraft:` prefix.
    ///
    /// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
    /// "In Java Edition, an item's enchantability is stored in the `minecraft:enchantable`
    /// item component."
    #[must_use]
    pub fn enchantability(&self, item: &str) -> Option<u32> {
        self.entries
            .get(normalize_item_id(item))
            .and_then(|entry| entry.enchantability)
    }

    /// Returns the mining speed multiplier for `item` breaking `block` from its
    /// `minecraft:tool` component, or [`DEFAULT_TOOL_MINING_SPEED`] (`1.0`) if `item`
    /// is unknown, has no `minecraft:tool` component, or has no matching rule
    /// overriding `speed` (falling back to `default_mining_speed` when `minecraft:tool`
    /// is present). Accepts item and block identifiers with or without the `minecraft:`
    /// prefix.
    ///
    /// Source: minecraft.wiki `Data component format` (revision 3903955,
    /// `Data component format/tool` revision 2910648):
    /// - `rules`: "If a field is overridden by multiple matched rules, the one that comes first in the list is chosen."
    /// - `speed`: "If the blocks match, overrides the default mining speed. Optional."
    /// - `default_mining_speed`: "The default mining speed of this tool, used if no rules override it. Defaults to 1.0."
    #[must_use]
    pub fn tool_speed(&self, item: &str, block: &str, tags: &BlockTags) -> f32 {
        let Some(tool) = self
            .entries
            .get(normalize_item_id(item))
            .and_then(|entry| entry.tool.as_ref())
        else {
            return DEFAULT_TOOL_MINING_SPEED;
        };
        tool.rules
            .iter()
            .filter(|rule| rule.blocks.matches_block(block, tags))
            .find_map(|rule| rule.speed)
            .unwrap_or(tool.default_mining_speed)
    }

    /// Returns whether `item` is considered correct to mine `block` at full efficiency
    /// and drop tool-gated items according to its `minecraft:tool` component, or `None`
    /// if `item` is unknown or lacks `minecraft:tool`.
    /// Accepts item and block identifiers with or without the `minecraft:` prefix.
    ///
    /// Source: minecraft.wiki `Data component format` (revision 3903955,
    /// `Data component format/tool` revision 2910648):
    /// - `rules`: "If a field is overridden by multiple matched rules, the one that comes first in the list is chosen."
    /// - `correct_for_drops`: "If the blocks match, overrides whether or not this tool is considered correct to mine at its most efficient speed, and to drop items if the block's loot table requires it. If not set by any rules, defaults to `false`. Optional."
    #[must_use]
    pub fn is_correct_for_drops(&self, item: &str, block: &str, tags: &BlockTags) -> Option<bool> {
        let tool = self
            .entries
            .get(normalize_item_id(item))
            .and_then(|entry| entry.tool.as_ref())?;
        let matched = tool
            .rules
            .iter()
            .filter(|rule| rule.blocks.matches_block(block, tags))
            .find_map(|rule| rule.correct_for_drops)
            .unwrap_or(false);
        Some(matched)
    }

    /// Returns the number of items loaded from the report.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when no items have been loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn item_report_invalid(file: &Path, message: impl Into<String>) -> ItemDefaultsError {
    ItemDefaultsError::Invalid {
        file: file.to_path_buf(),
        message: message.into(),
    }
}

fn parse_item_report(file: &Path) -> Result<(String, ItemDefaultEntry), ItemDefaultsError> {
    let item_stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| item_report_invalid(file, "missing or non-UTF-8 item filename stem"))?;
    let full_id = format!("minecraft:{item_stem}");
    if !valid_item_id(&full_id) {
        return Err(item_report_invalid(
            file,
            format!("invalid item identifier `{full_id}`"),
        ));
    }

    let document = read_json(file)?;
    let obj = document
        .as_object()
        .ok_or_else(|| item_report_invalid(file, "item report root must be a JSON object"))?;
    let components = obj
        .get("components")
        .and_then(Value::as_object)
        .ok_or_else(|| item_report_invalid(file, "missing or non-object `components` field"))?;

    let raw_stack = components
        .get("minecraft:max_stack_size")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            item_report_invalid(
                file,
                "missing or invalid integer `components[\"minecraft:max_stack_size\"]`",
            )
        })?;
    let max_stack_size = u8::try_from(raw_stack)
        .ok()
        .filter(|&n| (1..=MAX_STACK_LIMIT).contains(&u16::from(n)))
        .ok_or_else(|| {
            item_report_invalid(
                file,
                format!("`minecraft:max_stack_size` {raw_stack} must be in 1..={MAX_STACK_LIMIT}"),
            )
        })?;

    let max_damage = match components.get("minecraft:max_damage") {
        None => None,
        Some(value) => {
            let raw_damage = value
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|&n| n >= 1)
                .ok_or_else(|| {
                    item_report_invalid(
                        file,
                        "`components[\"minecraft:max_damage\"]` must be a positive 32-bit integer",
                    )
                })?;
            Some(raw_damage)
        }
    };

    let repairable = match components.get("minecraft:repairable") {
        None => None,
        Some(value) => Some(parse_repairable(value, file)?),
    };

    let enchantability = match components.get("minecraft:enchantable") {
        None => None,
        Some(value) => Some(parse_enchantable(value, file)?),
    };

    let tool = match components.get("minecraft:tool") {
        None => None,
        Some(value) => Some(parse_tool(value, file)?),
    };

    Ok((
        item_stem.to_owned(),
        ItemDefaultEntry {
            max_stack_size,
            max_damage,
            repairable,
            enchantability,
            tool,
        },
    ))
}

fn parse_id_set(value: &Value, file: &Path, context: &str) -> Result<IdSet, ItemDefaultsError> {
    match value {
        Value::String(raw) => {
            if let Some(tag) = raw.strip_prefix('#') {
                if !valid_item_id(tag) {
                    return Err(item_report_invalid(
                        file,
                        format!("invalid tag `{raw}` in {context}"),
                    ));
                }
                Ok(IdSet::Tag(raw.clone()))
            } else {
                if !valid_item_id(raw) {
                    return Err(item_report_invalid(
                        file,
                        format!("invalid id `{raw}` in {context}"),
                    ));
                }
                Ok(IdSet::Id(raw.clone()))
            }
        }
        Value::Array(arr) => {
            let mut list = Vec::with_capacity(arr.len());
            for elem in arr {
                let raw = elem.as_str().ok_or_else(|| {
                    item_report_invalid(file, format!("{context} list entries must be strings"))
                })?;
                if !valid_item_id(raw) {
                    return Err(item_report_invalid(
                        file,
                        format!("invalid id `{raw}` in {context} list"),
                    ));
                }
                if !list.iter().any(|existing| existing == raw) {
                    list.push(raw.to_owned());
                }
            }
            Ok(IdSet::List(list))
        }
        _ => Err(item_report_invalid(
            file,
            format!("{context} must be an id string, #tag string, or list"),
        )),
    }
}

fn parse_finite_f32(value: &Value, file: &Path, context: &str) -> Result<f32, ItemDefaultsError> {
    let raw = value
        .as_f64()
        .ok_or_else(|| item_report_invalid(file, format!("{context} must be a number")))?;
    let parsed = raw as f32;
    if !parsed.is_finite() {
        return Err(item_report_invalid(
            file,
            format!("{context} must be a finite 32-bit float"),
        ));
    }
    Ok(parsed)
}

fn parse_repairable(value: &Value, file: &Path) -> Result<IdSet, ItemDefaultsError> {
    let obj = value.as_object().ok_or_else(|| {
        item_report_invalid(
            file,
            "`components[\"minecraft:repairable\"]` must be a JSON object",
        )
    })?;
    let items_val = obj.get("items").ok_or_else(|| {
        item_report_invalid(
            file,
            "missing `items` field in `components[\"minecraft:repairable\"]`",
        )
    })?;
    parse_id_set(
        items_val,
        file,
        "`components[\"minecraft:repairable\"].items`",
    )
}

fn parse_enchantable(value: &Value, file: &Path) -> Result<u32, ItemDefaultsError> {
    let obj = value.as_object().ok_or_else(|| {
        item_report_invalid(
            file,
            "`components[\"minecraft:enchantable\"]` must be a JSON object",
        )
    })?;
    obj.get("value")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n >= 1)
        .ok_or_else(|| {
            item_report_invalid(
                file,
                "`components[\"minecraft:enchantable\"].value` must be a positive 32-bit integer",
            )
        })
}

fn parse_tool_rule(value: &Value, file: &Path) -> Result<ToolRule, ItemDefaultsError> {
    let obj = value.as_object().ok_or_else(|| {
        item_report_invalid(
            file,
            "`components[\"minecraft:tool\"].rules[]` entries must be JSON objects",
        )
    })?;
    for key in obj.keys() {
        if !matches!(key.as_str(), "blocks" | "speed" | "correct_for_drops") {
            return Err(item_report_invalid(
                file,
                format!("unknown key `{key}` in `components[\"minecraft:tool\"].rules[]`"),
            ));
        }
    }
    let blocks_val = obj.get("blocks").ok_or_else(|| {
        item_report_invalid(
            file,
            "missing `blocks` field in `components[\"minecraft:tool\"].rules[]`",
        )
    })?;
    let blocks = parse_id_set(
        blocks_val,
        file,
        "`components[\"minecraft:tool\"].rules[].blocks`",
    )?;
    let speed = match obj.get("speed") {
        None => None,
        Some(val) => Some(parse_finite_f32(
            val,
            file,
            "`components[\"minecraft:tool\"].rules[].speed`",
        )?),
    };
    let correct_for_drops = match obj.get("correct_for_drops") {
        None => None,
        Some(val) => {
            let b = val.as_bool().ok_or_else(|| {
                item_report_invalid(
                    file,
                    "`components[\"minecraft:tool\"].rules[].correct_for_drops` must be a boolean",
                )
            })?;
            Some(b)
        }
    };
    Ok(ToolRule {
        blocks,
        speed,
        correct_for_drops,
    })
}

fn parse_tool(value: &Value, file: &Path) -> Result<ToolComponent, ItemDefaultsError> {
    let obj = value.as_object().ok_or_else(|| {
        item_report_invalid(
            file,
            "`components[\"minecraft:tool\"]` must be a JSON object",
        )
    })?;
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "rules"
                | "default_mining_speed"
                | "damage_per_block"
                | "can_destroy_blocks_in_creative"
        ) {
            return Err(item_report_invalid(
                file,
                format!("unknown key `{key}` in `components[\"minecraft:tool\"]`"),
            ));
        }
    }
    let default_mining_speed = match obj.get("default_mining_speed") {
        None => DEFAULT_TOOL_MINING_SPEED,
        Some(val) => parse_finite_f32(
            val,
            file,
            "`components[\"minecraft:tool\"].default_mining_speed`",
        )?,
    };
    if let Some(val) = obj.get("damage_per_block") {
        val.as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| {
                item_report_invalid(
                    file,
                    "`components[\"minecraft:tool\"].damage_per_block` must be a non-negative 32-bit integer",
                )
            })?;
    }
    if let Some(val) = obj.get("can_destroy_blocks_in_creative")
        && !val.is_boolean()
    {
        return Err(item_report_invalid(
            file,
            "`components[\"minecraft:tool\"].can_destroy_blocks_in_creative` must be a boolean",
        ));
    }
    let raw_rules = obj.get("rules").and_then(Value::as_array).ok_or_else(|| {
        item_report_invalid(
            file,
            "missing or non-array `rules` field in `components[\"minecraft:tool\"]`",
        )
    })?;
    let mut rules = Vec::with_capacity(raw_rules.len());
    for raw_rule in raw_rules {
        rules.push(parse_tool_rule(raw_rule, file)?);
    }
    Ok(ToolComponent {
        default_mining_speed,
        rules,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_new_accepts_valid_stacks_and_default_max_stack() {
        let one = ItemStack::try_new("minecraft:stone", 1, 64).unwrap();
        assert_eq!(one.item(), "minecraft:stone");
        assert_eq!(one.count(), 1);
        assert_eq!(one.max_stack(), 64);

        let full = ItemStack::try_new("minecraft:stone", 64, DEFAULT_MAX_STACK).unwrap();
        assert_eq!(full.count(), 64);
        assert_eq!(full.max_stack(), DEFAULT_MAX_STACK);

        let potion = ItemStack::try_new("minecraft:potion", 1, 1).unwrap();
        assert_eq!(potion.item(), "minecraft:potion");
        assert_eq!(potion.count(), 1);
        assert_eq!(potion.max_stack(), 1);

        let max = ItemStack::try_new("a:b", 99, MAX_STACK_LIMIT).unwrap();
        assert_eq!(max.item(), "a:b");
        assert_eq!(max.count(), 99);
        assert_eq!(max.max_stack(), MAX_STACK_LIMIT);
    }

    #[test]
    fn validates_item_id_character_and_separator_rules() {
        for invalid in [
            "",
            "stone",
            ":stone",
            "minecraft:",
            "minecraft:stone:extra",
            "Minecraft:stone",
            "minecraft:Stone",
            "minecraft:sto ne",
            "my/mod:stone",
        ] {
            assert_eq!(
                ItemStack::try_new(invalid, 1, DEFAULT_MAX_STACK),
                Err(ItemStackError::InvalidItem),
                "expected InvalidItem for `{invalid}`"
            );
        }

        let custom = ItemStack::try_new("my_mod.x-1:block/a_b.c-2", 1, DEFAULT_MAX_STACK).unwrap();
        assert_eq!(custom.item(), "my_mod.x-1:block/a_b.c-2");
    }

    #[test]
    fn rejects_invalid_max_stack_and_prioritizes_earlier_checks() {
        assert_eq!(
            ItemStack::try_new("minecraft:stone", 1, 0),
            Err(ItemStackError::InvalidMaxStack(0))
        );
        assert_eq!(
            ItemStack::try_new("minecraft:stone", 1, 100),
            Err(ItemStackError::InvalidMaxStack(100))
        );
        assert_eq!(
            ItemStack::try_new("NotValid", 1, 0),
            Err(ItemStackError::InvalidItem)
        );
        assert_eq!(
            ItemStack::try_new("NotValid", 0, 100),
            Err(ItemStackError::InvalidItem)
        );
        assert_eq!(
            ItemStack::try_new("minecraft:stone", 0, 0),
            Err(ItemStackError::InvalidMaxStack(0))
        );
    }

    #[test]
    fn rejects_zero_or_over_limit_count() {
        assert_eq!(
            ItemStack::try_new("minecraft:stone", 0, 64),
            Err(ItemStackError::InvalidCount(0))
        );
        assert_eq!(
            ItemStack::try_new("minecraft:stone", 65, 64),
            Err(ItemStackError::InvalidCount(65))
        );
        assert_eq!(
            ItemStack::try_new("minecraft:potion", 2, 1),
            Err(ItemStackError::InvalidCount(2))
        );
    }

    #[test]
    fn error_display_is_lowercase_and_never_echoes_item_id() {
        let item_err = ItemStack::try_new("secret_ns:Bad_Item", 1, 64).unwrap_err();
        let msg = item_err.to_string();
        assert!(!msg.is_empty());
        assert_eq!(msg, msg.to_lowercase());
        assert!(!msg.contains("secret_ns"));
        assert!(!msg.contains("bad_item"));

        for err in [
            ItemStackError::InvalidMaxStack(0),
            ItemStackError::InvalidCount(65),
        ] {
            let text = err.to_string();
            assert!(!text.is_empty());
            assert_eq!(text, text.to_lowercase());
            let _: &dyn std::error::Error = &err;
        }
    }

    #[test]
    fn can_stack_with_requires_same_id_and_max_stack() {
        let stone_one = ItemStack::try_new("minecraft:stone", 1, 64).unwrap();
        let stone_ten = ItemStack::try_new("minecraft:stone", 10, 64).unwrap();
        let dirt = ItemStack::try_new("minecraft:dirt", 1, 64).unwrap();
        let stone_small_max = ItemStack::try_new("minecraft:stone", 1, 16).unwrap();

        assert!(stone_one.can_stack_with(&stone_ten));
        assert!(!stone_one.can_stack_with(&dirt));
        assert!(!stone_one.can_stack_with(&stone_small_max));
    }

    #[test]
    fn add_caps_at_max_stack_and_returns_overflow() {
        let mut stack = ItemStack::try_new("minecraft:stone", 60, 64).unwrap();
        assert_eq!(stack.add(0), 0);
        assert_eq!(stack.count(), 60);

        assert_eq!(stack.add(4), 0);
        assert_eq!(stack.count(), 64);

        let mut near_full = ItemStack::try_new("minecraft:stone", 60, 64).unwrap();
        assert_eq!(near_full.add(10), 6);
        assert_eq!(near_full.count(), 64);

        assert_eq!(near_full.add(25), 25);
        assert_eq!(near_full.count(), 64);

        let mut single = ItemStack::try_new("minecraft:stone", 1, 64).unwrap();
        assert_eq!(single.add(u16::MAX), u16::MAX - 63);
        assert_eq!(single.count(), 64);

        let mut pearl = ItemStack::try_new("minecraft:ender_pearl", 1, 16).unwrap();
        assert_eq!(pearl.add(20), 5);
        assert_eq!(pearl.count(), 16);
    }

    #[test]
    fn split_returns_err_on_zero_and_keeps_non_empty_stacks() {
        let original = ItemStack::try_new("minecraft:ender_pearl", 10, 16).unwrap();
        let returned = original.clone().split(0).unwrap_err();
        assert_eq!(returned, original);

        let (taken, rest) = original.clone().split(4).unwrap();
        assert_eq!(taken.item(), "minecraft:ender_pearl");
        assert_eq!(taken.count(), 4);
        assert_eq!(taken.max_stack(), 16);
        let rest = rest.expect("remainder should exist");
        assert_eq!(rest.item(), "minecraft:ender_pearl");
        assert_eq!(rest.count(), 6);
        assert_eq!(rest.max_stack(), 16);

        let (taken_exact, rest_exact) = original.clone().split(10).unwrap();
        assert_eq!(taken_exact.item(), "minecraft:ender_pearl");
        assert_eq!(taken_exact.count(), 10);
        assert_eq!(taken_exact.max_stack(), 16);
        assert_eq!(rest_exact, None);

        let (taken_more, rest_more) = original.split(50).unwrap();
        assert_eq!(taken_more.item(), "minecraft:ender_pearl");
        assert_eq!(taken_more.count(), 10);
        assert_eq!(taken_more.max_stack(), 16);
        assert_eq!(rest_more, None);
    }

    #[test]
    fn item_id_helpers_normalize_compare_and_default_max_stack() {
        assert_eq!(normalize_item_id("minecraft:stone"), "stone");
        assert_eq!(normalize_item_id("stone"), "stone");
        assert_eq!(normalize_item_id("custom:widget"), "custom:widget");

        assert!(items_equal("minecraft:oak_planks", "oak_planks"));
        assert!(items_equal("oak_planks", "oak_planks"));
        assert!(!items_equal("custom:oak_planks", "oak_planks"));

        for single in [
            "minecraft:lava_bucket",
            "water_bucket",
            "minecraft:milk_bucket",
            "minecraft:potion",
            "splash_potion",
            "minecraft:lingering_potion",
        ] {
            assert_eq!(default_max_stack(single), 1, "{single}");
        }
        for sixteen in ["bucket", "minecraft:snowball", "ender_pearl", "egg"] {
            assert_eq!(default_max_stack(sixteen), 16, "{sixteen}");
        }
        assert_eq!(default_max_stack("minecraft:stone"), DEFAULT_MAX_STACK);
    }

    fn scratch_root(label: &str) -> PathBuf {
        use std::fs;
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rustmc-item-{label}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create scratch dir");
        path
    }

    fn write_file(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        std::fs::write(path, text).expect("write");
    }

    #[test]
    fn item_defaults_loads_and_supports_prefix_insensitive_lookup() {
        let root = scratch_root("defaults-ok");
        write_file(
            &root.join("reports/minecraft/components/item/widget.json"),
            r#"{"components": {"minecraft:max_stack_size": 16, "minecraft:rarity": "common"}}"#,
        );
        write_file(
            &root.join("reports/minecraft/components/item/gadget.json"),
            r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:max_damage": 238}}"#,
        );

        let defaults = ItemDefaults::load(&root).expect("load item defaults");
        assert_eq!(defaults.len(), 2);
        assert!(!defaults.is_empty());

        assert_eq!(defaults.max_stack_size("minecraft:widget"), Some(16));
        assert_eq!(defaults.max_stack_size("widget"), Some(16));
        assert_eq!(defaults.max_damage("widget"), None);

        assert_eq!(defaults.max_stack_size("minecraft:gadget"), Some(1));
        assert_eq!(defaults.max_stack_size("gadget"), Some(1));
        assert_eq!(defaults.max_damage("minecraft:gadget"), Some(238));
        assert_eq!(defaults.max_damage("gadget"), Some(238));

        assert_eq!(defaults.max_stack_size("testns:widget"), None);
        assert_eq!(defaults.max_stack_size("missing"), None);
        assert_eq!(defaults.max_damage("missing"), None);

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn item_defaults_rejects_out_of_range_stack_size_naming_file() {
        for (name, value) in [("too_low.json", "0"), ("too_high.json", "100")] {
            let root = scratch_root(name);
            write_file(
                &root.join("reports/minecraft/components/item").join(name),
                &format!(r#"{{"components": {{"minecraft:max_stack_size": {value}}}}}"#),
            );
            let err = ItemDefaults::load(&root)
                .expect_err("out of range stack size must fail")
                .to_string();
            assert!(err.contains(name), "error must name {name}: {err}");
            std::fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn item_defaults_missing_reports_dir_mentions_reports_flag() {
        let root = scratch_root("missing-reports");
        let err = ItemDefaults::load(&root)
            .expect_err("missing reports dir must fail")
            .to_string();
        assert!(
            err.contains(
                "java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports"
            ),
            "error must describe how to generate reports: {err}"
        );
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_item_defaults_pin_vanilla_counts`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_item_defaults_pin_vanilla_counts() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let defaults = ItemDefaults::load(&root).expect("load vanilla item defaults");

        assert_eq!(defaults.len(), 1658);
        let count_64 = defaults
            .entries
            .values()
            .filter(|e| e.max_stack_size == 64)
            .count();
        let count_1 = defaults
            .entries
            .values()
            .filter(|e| e.max_stack_size == 1)
            .count();
        let count_16 = defaults
            .entries
            .values()
            .filter(|e| e.max_stack_size == 16)
            .count();
        assert_eq!(count_64, 1342);
        assert_eq!(count_1, 248);
        assert_eq!(count_16, 68);

        assert_eq!(defaults.max_stack_size("minecraft:shears"), Some(1));
        assert_eq!(defaults.max_stack_size("shears"), Some(1));
        assert_eq!(defaults.max_damage("minecraft:shears"), Some(238));
        assert_eq!(defaults.max_damage("shears"), Some(238));
        assert_eq!(defaults.max_stack_size("minecraft:ender_pearl"), Some(16));
        assert_eq!(defaults.max_damage("minecraft:ender_pearl"), None);
    }

    #[test]
    fn item_defaults_reads_repairable_variants_and_enchantability() {
        let root = scratch_root("repairable-enchantable");
        write_file(
            &root.join("data/testns/tags/item/tool_mats.json"),
            r##"{"values": ["testns:ore_a", "#testns:extra_mats"]}"##,
        );
        write_file(
            &root.join("data/testns/tags/item/extra_mats.json"),
            r#"{"values": ["testns:ore_b"]}"#,
        );
        write_file(
            &root.join("reports/minecraft/components/item/widget.json"),
            r#"{"components": {
                "minecraft:max_stack_size": 1,
                "minecraft:max_damage": 120,
                "minecraft:repairable": {"items": "testns:crystal"},
                "minecraft:enchantable": {"value": 14}
            }}"#,
        );
        write_file(
            &root.join("reports/minecraft/components/item/tagged_tool.json"),
            r##"{"components": {
                "minecraft:max_stack_size": 1,
                "minecraft:max_damage": 200,
                "minecraft:repairable": {"items": "#testns:tool_mats"},
                "minecraft:enchantable": {"value": 10}
            }}"##,
        );
        write_file(
            &root.join("reports/minecraft/components/item/multi_tool.json"),
            r#"{"components": {
                "minecraft:max_stack_size": 1,
                "minecraft:max_damage": 80,
                "minecraft:repairable": {"items": ["testns:alpha", "testns:beta", "testns:alpha"]}
            }}"#,
        );
        write_file(
            &root.join("reports/minecraft/components/item/plain.json"),
            r#"{"components": {"minecraft:max_stack_size": 64}}"#,
        );

        let tags = ItemTags::load(&root).expect("load synthetic item tags");
        let defaults = ItemDefaults::load(&root).expect("load synthetic item defaults");

        assert_eq!(
            defaults.repair_materials("minecraft:widget", &tags),
            Some(vec!["testns:crystal".to_owned()])
        );
        assert_eq!(
            defaults.repair_materials("widget", &tags),
            Some(vec!["testns:crystal".to_owned()])
        );
        assert_eq!(
            defaults.repair_materials("tagged_tool", &tags),
            Some(vec!["testns:ore_a".to_owned(), "testns:ore_b".to_owned()])
        );
        assert_eq!(
            defaults.repair_materials("tagged_tool", &ItemTags::default()),
            None
        );
        assert_eq!(
            defaults.repair_materials("minecraft:multi_tool", &tags),
            Some(vec!["testns:alpha".to_owned(), "testns:beta".to_owned()])
        );
        assert_eq!(defaults.repair_materials("plain", &tags), None);
        assert_eq!(defaults.repair_materials("missing", &tags), None);

        assert_eq!(defaults.enchantability("minecraft:widget"), Some(14));
        assert_eq!(defaults.enchantability("tagged_tool"), Some(10));
        assert_eq!(defaults.enchantability("multi_tool"), None);
        assert_eq!(defaults.enchantability("plain"), None);
        assert_eq!(defaults.enchantability("missing"), None);

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn item_defaults_rejects_malformed_repairable_and_enchantable_naming_file() {
        let cases = [
            (
                "bad_repair_type.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:repairable": "bad"}}"#,
            ),
            (
                "missing_repair_items.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:repairable": {}}}"#,
            ),
            (
                "bad_repair_id.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:repairable": {"items": "bad"}}}"#,
            ),
            (
                "bad_repair_tag.json",
                r##"{"components": {"minecraft:max_stack_size": 1, "minecraft:repairable": {"items": "#bad"}}}"##,
            ),
            (
                "bad_repair_list_entry.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:repairable": {"items": [42]}}}"#,
            ),
            (
                "bad_enchantable_type.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:enchantable": 10}}"#,
            ),
            (
                "zero_enchantable.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:enchantable": {"value": 0}}}"#,
            ),
        ];

        for (name, body) in cases {
            let root = scratch_root(name);
            write_file(
                &root.join("reports/minecraft/components/item").join(name),
                body,
            );
            let err = ItemDefaults::load(&root)
                .expect_err("malformed component must fail")
                .to_string();
            assert!(err.contains(name), "error must name {name}: {err}");
            std::fs::remove_dir_all(root).expect("cleanup");
        }
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_item_defaults_pin_repairable_and_enchantable`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_item_defaults_pin_repairable_and_enchantable() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let tags = ItemTags::load(&root).expect("load vanilla item tags");
        let defaults = ItemDefaults::load(&root).expect("load vanilla item defaults");

        assert_eq!(defaults.max_damage("minecraft:diamond_sword"), Some(1561));
        assert_eq!(
            defaults.repair_materials("minecraft:diamond_sword", &tags),
            Some(vec!["minecraft:diamond".to_owned()])
        );
        assert_eq!(defaults.enchantability("minecraft:diamond_sword"), Some(10));

        assert_eq!(defaults.max_damage("minecraft:mace"), Some(500));
        assert_eq!(
            defaults.repair_materials("minecraft:mace", &tags),
            Some(vec!["minecraft:breeze_rod".to_owned()])
        );
        assert_eq!(defaults.enchantability("minecraft:mace"), Some(15));

        assert_eq!(
            defaults.repair_materials("minecraft:copper_pickaxe", &tags),
            Some(vec!["minecraft:copper_ingot".to_owned()])
        );
        assert_eq!(defaults.max_damage("minecraft:copper_pickaxe"), Some(190));
        assert_eq!(
            defaults.enchantability("minecraft:copper_pickaxe"),
            Some(13)
        );

        assert_eq!(defaults.max_damage("minecraft:book"), None);
        assert_eq!(defaults.repair_materials("minecraft:book", &tags), None);
        assert_eq!(defaults.enchantability("minecraft:book"), Some(1));
    }

    #[test]
    fn item_defaults_evaluates_tool_rules_order_and_defaults() {
        let root = scratch_root("tool-rules");
        write_file(
            &root.join("data/testns/tags/block/mineable.json"),
            r#"{"values": ["testns:stone", "testns:hard_ore"]}"#,
        );
        write_file(
            &root.join("data/testns/tags/block/denied.json"),
            r#"{"values": ["testns:hard_ore", "testns:bedrock"]}"#,
        );
        write_file(
            &root.join("reports/minecraft/components/item/multi_tool.json"),
            r##"{"components": {
                "minecraft:max_stack_size": 1,
                "minecraft:tool": {
                    "default_mining_speed": 1.5,
                    "damage_per_block": 2,
                    "can_destroy_blocks_in_creative": false,
                    "rules": [
                        {"blocks": "#testns:denied", "correct_for_drops": false},
                        {"blocks": "#testns:mineable", "speed": 7.5, "correct_for_drops": true},
                        {"blocks": ["testns:alpha", "testns:beta"], "speed": 12.0, "correct_for_drops": true},
                        {"blocks": "testns:alpha", "speed": 99.0, "correct_for_drops": false}
                    ]
                }
            }}"##,
        );
        write_file(
            &root.join("reports/minecraft/components/item/speed_only.json"),
            r#"{"components": {
                "minecraft:max_stack_size": 1,
                "minecraft:tool": {
                    "rules": [
                        {"blocks": "minecraft:cobweb", "speed": 15.0}
                    ]
                }
            }}"#,
        );
        write_file(
            &root.join("reports/minecraft/components/item/empty_tool.json"),
            r#"{"components": {
                "minecraft:max_stack_size": 1,
                "minecraft:tool": {"damage_per_block": 0, "rules": []}
            }}"#,
        );
        write_file(
            &root.join("reports/minecraft/components/item/plain.json"),
            r#"{"components": {"minecraft:max_stack_size": 64}}"#,
        );

        let block_tags = BlockTags::load(&root).expect("load synthetic block tags");
        let defaults = ItemDefaults::load(&root).expect("load synthetic item defaults");

        // Denied tag overrides correct_for_drops first, while speed falls through to mineable rule.
        assert_eq!(
            defaults.tool_speed("minecraft:multi_tool", "testns:hard_ore", &block_tags),
            7.5
        );
        assert_eq!(
            defaults.is_correct_for_drops("multi_tool", "testns:hard_ore", &block_tags),
            Some(false)
        );

        // Mineable only.
        assert_eq!(
            defaults.tool_speed("multi_tool", "testns:stone", &block_tags),
            7.5
        );
        assert_eq!(
            defaults.is_correct_for_drops("minecraft:multi_tool", "testns:stone", &block_tags),
            Some(true)
        );

        // First matching rule wins over later rule for the same block.
        assert_eq!(
            defaults.tool_speed("multi_tool", "testns:alpha", &block_tags),
            12.0
        );
        assert_eq!(
            defaults.is_correct_for_drops("multi_tool", "testns:alpha", &block_tags),
            Some(true)
        );
        assert_eq!(
            defaults.tool_speed("multi_tool", "testns:beta", &block_tags),
            12.0
        );

        // Denied-only block falls back to default_mining_speed.
        assert_eq!(
            defaults.tool_speed("multi_tool", "testns:bedrock", &block_tags),
            1.5
        );
        assert_eq!(
            defaults.is_correct_for_drops("multi_tool", "testns:bedrock", &block_tags),
            Some(false)
        );

        // Unmatched block with minecraft:tool gets default_mining_speed and Some(false).
        assert_eq!(
            defaults.tool_speed("multi_tool", "testns:unlisted", &block_tags),
            1.5
        );
        assert_eq!(
            defaults.is_correct_for_drops("multi_tool", "testns:unlisted", &block_tags),
            Some(false)
        );

        // Speed-only rule supports prefix-insensitive minecraft: blocks and defaults correct_for_drops to false.
        assert_eq!(
            defaults.tool_speed("speed_only", "cobweb", &block_tags),
            15.0
        );
        assert_eq!(
            defaults.tool_speed("speed_only", "minecraft:cobweb", &block_tags),
            15.0
        );
        assert_eq!(
            defaults.is_correct_for_drops("speed_only", "cobweb", &block_tags),
            Some(false)
        );
        assert_eq!(
            defaults.tool_speed("speed_only", "testns:unlisted", &block_tags),
            DEFAULT_TOOL_MINING_SPEED
        );

        // Empty rules list still has minecraft:tool defaults.
        assert_eq!(
            defaults.tool_speed("empty_tool", "testns:stone", &block_tags),
            DEFAULT_TOOL_MINING_SPEED
        );
        assert_eq!(
            defaults.is_correct_for_drops("empty_tool", "testns:stone", &block_tags),
            Some(false)
        );

        // Items without minecraft:tool or unknown items get 1.0 and None.
        assert_eq!(
            defaults.tool_speed("plain", "testns:stone", &block_tags),
            DEFAULT_TOOL_MINING_SPEED
        );
        assert_eq!(
            defaults.is_correct_for_drops("plain", "testns:stone", &block_tags),
            None
        );
        assert_eq!(
            defaults.tool_speed("missing", "testns:stone", &block_tags),
            DEFAULT_TOOL_MINING_SPEED
        );
        assert_eq!(
            defaults.is_correct_for_drops("missing", "testns:stone", &block_tags),
            None
        );

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn item_defaults_rejects_malformed_tool_component_naming_file() {
        let cases = [
            (
                "bad_tool_type.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": "bad"}}"#,
            ),
            (
                "missing_tool_rules.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {}}}"#,
            ),
            (
                "bad_tool_rules_type.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"rules": "bad"}}}"#,
            ),
            (
                "bad_tool_rule_entry.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"rules": ["bad"]}}}"#,
            ),
            (
                "missing_rule_blocks.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"rules": [{"speed": 2.0}]}}}"#,
            ),
            (
                "bad_rule_blocks_id.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"rules": [{"blocks": "bad"}]}}}"#,
            ),
            (
                "bad_rule_blocks_tag.json",
                r##"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"rules": [{"blocks": "#bad"}]}}}"##,
            ),
            (
                "bad_rule_speed.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"rules": [{"blocks": "testns:a", "speed": "fast"}]}}}"#,
            ),
            (
                "overflow_rule_speed.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"rules": [{"blocks": "testns:a", "speed": 1e39}]}}}"#,
            ),
            (
                "bad_rule_correct.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"rules": [{"blocks": "testns:a", "correct_for_drops": 1}]}}}"#,
            ),
            (
                "bad_default_speed.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"default_mining_speed": "1.0", "rules": []}}}"#,
            ),
            (
                "negative_damage_per_block.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"damage_per_block": -1, "rules": []}}}"#,
            ),
            (
                "bad_creative_destroy.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"can_destroy_blocks_in_creative": 0, "rules": []}}}"#,
            ),
            (
                "unknown_tool_key.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"extra": true, "rules": []}}}"#,
            ),
            (
                "unknown_rule_key.json",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:tool": {"rules": [{"blocks": "testns:a", "extra": 1}]}}}"#,
            ),
        ];

        for (name, body) in cases {
            let root = scratch_root(name);
            write_file(
                &root.join("reports/minecraft/components/item").join(name),
                body,
            );
            let err = ItemDefaults::load(&root)
                .expect_err("malformed tool component must fail")
                .to_string();
            assert!(err.contains(name), "error must name {name}: {err}");
            std::fs::remove_dir_all(root).expect("cleanup");
        }
    }

    fn read_tool_rule_speed(root: &Path, item: &str, rule_index: usize) -> f32 {
        let report_path = root
            .join("reports/minecraft/components/item")
            .join(format!("{item}.json"));
        let doc = read_json(&report_path).expect("read item report");
        doc["components"]["minecraft:tool"]["rules"][rule_index]["speed"]
            .as_f64()
            .expect("rule speed in report") as f32
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_item_defaults_evaluate_tool_rules`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_item_defaults_evaluate_tool_rules() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let block_tags = BlockTags::load(&root).expect("load vanilla block tags");
        let defaults = ItemDefaults::load(&root).expect("load vanilla item defaults");

        let tool_count = defaults
            .entries
            .values()
            .filter(|entry| entry.tool.is_some())
            .count();
        assert_eq!(tool_count, 38);

        let diamond_pickaxe_speed = read_tool_rule_speed(&root, "diamond_pickaxe", 1);
        let iron_pickaxe_speed = read_tool_rule_speed(&root, "iron_pickaxe", 1);
        let wooden_shovel_speed = read_tool_rule_speed(&root, "wooden_shovel", 1);
        let shears_cobweb_speed = read_tool_rule_speed(&root, "shears", 0);

        assert_eq!(
            defaults.tool_speed("minecraft:diamond_pickaxe", "minecraft:stone", &block_tags),
            diamond_pickaxe_speed
        );
        assert_eq!(
            defaults.is_correct_for_drops(
                "minecraft:diamond_pickaxe",
                "minecraft:stone",
                &block_tags
            ),
            Some(true)
        );

        assert_eq!(
            defaults.tool_speed(
                "minecraft:diamond_pickaxe",
                "minecraft:obsidian",
                &block_tags
            ),
            diamond_pickaxe_speed
        );
        assert_eq!(
            defaults.is_correct_for_drops(
                "minecraft:diamond_pickaxe",
                "minecraft:obsidian",
                &block_tags
            ),
            Some(true)
        );

        assert_eq!(
            defaults.tool_speed("minecraft:iron_pickaxe", "minecraft:obsidian", &block_tags),
            iron_pickaxe_speed
        );
        assert_eq!(
            defaults.is_correct_for_drops(
                "minecraft:iron_pickaxe",
                "minecraft:obsidian",
                &block_tags
            ),
            Some(false)
        );

        assert_eq!(
            defaults.tool_speed("minecraft:wooden_shovel", "minecraft:dirt", &block_tags),
            wooden_shovel_speed
        );
        assert_eq!(
            defaults.is_correct_for_drops("minecraft:wooden_shovel", "minecraft:dirt", &block_tags),
            Some(true)
        );

        assert_eq!(shears_cobweb_speed, 15.0);
        assert_eq!(
            defaults.tool_speed("minecraft:shears", "minecraft:cobweb", &block_tags),
            shears_cobweb_speed
        );
        assert_eq!(
            defaults.is_correct_for_drops("minecraft:shears", "minecraft:cobweb", &block_tags),
            Some(true)
        );

        assert_eq!(
            defaults.tool_speed("minecraft:stick", "minecraft:stone", &block_tags),
            1.0
        );
        assert_eq!(
            defaults.is_correct_for_drops("minecraft:stick", "minecraft:stone", &block_tags),
            None
        );
    }
}
