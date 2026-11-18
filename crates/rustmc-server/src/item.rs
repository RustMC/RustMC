//! A counted stack of one item kind, with invariants checked at construction.
//! Per-item components (durability, names, enchantments) are deliberately not
//! modelled yet.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::vanilla::worldgen::{WorldgenError, read_dir_optional, read_json, walk_json};

/// Default stack limit for standard items.
pub const DEFAULT_MAX_STACK: u16 = 64;

/// Upper bound of the item max_stack_size component since Java 1.20.5.
pub const MAX_STACK_LIMIT: u16 = 99;

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
fn valid_item_id(id: &str) -> bool {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ItemDefaultEntry {
    max_stack_size: u8,
    max_damage: Option<u32>,
}

/// Default per-item components read from `reports/minecraft/components/item/*.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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

    Ok((
        item_stem.to_owned(),
        ItemDefaultEntry {
            max_stack_size,
            max_damage,
        },
    ))
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
}
