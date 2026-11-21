//! A counted stack of one item kind, with invariants checked at construction
//! and typed per-item data components.

use std::collections::BTreeMap;
use std::fmt;

use crate::world::enchantment::MAX_ENCHANTMENT_LEVEL;

/// Minimum valid enchantment level in Java Edition (`1`;
/// minecraft.wiki `Enchantment` revision 3902233: "The minimum level of any
/// enchantment is I"; `Enchantment definition` revision 3843772: "Value
/// between 1 and 255 (inclusive)"; `Item modifier` revision 3896146:
/// "Decreasing an enchantment power below 1 remove the enchantment").
pub const MIN_ENCHANTMENT_LEVEL: u8 = 1;

/// Default stack limit for standard items.
pub const DEFAULT_MAX_STACK: u16 = 64;

/// Upper bound of the item max_stack_size component since Java 1.20.5.
pub const MAX_STACK_LIMIT: u16 = 99;

/// Default effect duration in ticks (`8 seconds`) for an entry in
/// `minecraft:suspicious_stew_effects` when `duration` is omitted.
///
/// Source: minecraft.wiki `Data component format/suspicious stew effects` (revision 3277335):
/// "`duration`: The duration of the effect in ticks. Defaults to 160."
pub const DEFAULT_STEW_EFFECT_DURATION_TICKS: u32 = 160;

/// Minimum valid amplifier for `minecraft:ominous_bottle_amplifier` (`0`, Bad Omen I).
///
/// Source: minecraft.wiki `Data component format/ominous bottle amplifier` (revision 3277333):
/// "`minecraft:ominous_bottle_amplifier`: ... Must be a non-negative integer from 0 to 4 (inclusive)."
pub const MIN_OMINOUS_BOTTLE_AMPLIFIER: u8 = 0;

/// Maximum valid amplifier for `minecraft:ominous_bottle_amplifier` (`4`, Bad Omen V).
///
/// Source: minecraft.wiki `Data component format/ominous bottle amplifier` (revision 3277333):
/// "`minecraft:ominous_bottle_amplifier`: ... Must be a non-negative integer from 0 to 4 (inclusive)."
pub const MAX_OMINOUS_BOTTLE_AMPLIFIER: u8 = 4;

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
        | "lingering_potion" | "enchanted_book" => 1,
        "bucket" | "snowball" | "ender_pearl" | "egg" => 16,
        _ => DEFAULT_MAX_STACK,
    }
}

/// Rejection reasons when constructing or validating [`ItemComponents`],
/// [`ArmorTrim`], or [`SuspiciousStewEffect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemComponentsError {
    /// Enchantment identifier is not a valid namespaced resource location.
    InvalidEnchantmentId,
    /// Enchantment level is outside `MIN_ENCHANTMENT_LEVEL..=MAX_ENCHANTMENT_LEVEL` (`1..=255`).
    InvalidEnchantmentLevel(u8),
    /// Potion identifier is not a valid namespaced resource location.
    InvalidPotionId,
    /// Armor trim pattern identifier is not a valid namespaced resource location.
    InvalidTrimPattern,
    /// Armor trim material identifier is not a valid namespaced resource location.
    InvalidTrimMaterial,
    /// Suspicious stew mob effect identifier is not a valid namespaced resource location.
    InvalidEffectId,
    /// Instrument identifier is not a valid namespaced resource location.
    InvalidInstrumentId,
    /// Ominous bottle amplifier is outside `MIN_OMINOUS_BOTTLE_AMPLIFIER..=MAX_OMINOUS_BOTTLE_AMPLIFIER` (`0..=4`).
    InvalidOminousBottleAmplifier(u8),
}

impl fmt::Display for ItemComponentsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEnchantmentId => f.write_str("invalid namespaced enchantment id"),
            Self::InvalidEnchantmentLevel(level) => write!(
                f,
                "enchantment level {level} must be in {MIN_ENCHANTMENT_LEVEL}..={MAX_ENCHANTMENT_LEVEL}"
            ),
            Self::InvalidPotionId => f.write_str("invalid namespaced potion id"),
            Self::InvalidTrimPattern => f.write_str("invalid namespaced trim pattern id"),
            Self::InvalidTrimMaterial => f.write_str("invalid namespaced trim material id"),
            Self::InvalidEffectId => f.write_str("invalid namespaced mob effect id"),
            Self::InvalidInstrumentId => f.write_str("invalid namespaced instrument id"),
            Self::InvalidOminousBottleAmplifier(amplifier) => write!(
                f,
                "ominous bottle amplifier {amplifier} must be in {MIN_OMINOUS_BOTTLE_AMPLIFIER}..={MAX_OMINOUS_BOTTLE_AMPLIFIER}"
            ),
        }
    }
}

impl std::error::Error for ItemComponentsError {}

/// Rejection reasons when constructing an [`ItemStack`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStackError {
    /// Item identifier is not a valid namespaced resource location.
    InvalidItem,
    /// Maximum stack size is outside `1..=MAX_STACK_LIMIT`.
    InvalidMaxStack(u16),
    /// Stack count is zero or exceeds the stack's maximum size.
    InvalidCount(u16),
    /// Data component values failed validation.
    InvalidComponents(ItemComponentsError),
}

impl std::fmt::Display for ItemStackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidItem => f.write_str("invalid namespaced item id"),
            Self::InvalidMaxStack(max) => {
                write!(f, "max stack size {max} must be in 1..={MAX_STACK_LIMIT}")
            }
            Self::InvalidCount(count) => write!(f, "invalid item stack count {count}"),
            Self::InvalidComponents(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ItemStackError {}

impl From<ItemComponentsError> for ItemStackError {
    fn from(err: ItemComponentsError) -> Self {
        Self::InvalidComponents(err)
    }
}

/// Armor trim pattern and material stored in an item's `minecraft:trim` component.
///
/// Source: minecraft.wiki `Data component format` (revision 3903955,
/// `Data component format/trim` revision 3685746):
/// "`minecraft:trim`: Contains the trim applied to this armor piece."
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArmorTrim {
    pattern: String,
    material: String,
}

impl ArmorTrim {
    /// Constructs a validated [`ArmorTrim`], ensuring `pattern` and `material`
    /// are non-empty namespaced resource locations.
    pub fn try_new(
        pattern: impl Into<String>,
        material: impl Into<String>,
    ) -> Result<Self, ItemComponentsError> {
        let pattern = pattern.into();
        let material = material.into();
        let trim = Self { pattern, material };
        trim.validate()?;
        Ok(trim)
    }

    /// Returns the namespaced trim pattern identifier (for example `"minecraft:coast"`).
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Returns the namespaced trim material identifier.
    #[must_use]
    pub fn material(&self) -> &str {
        &self.material
    }

    fn validate(&self) -> Result<(), ItemComponentsError> {
        if !valid_item_id(&self.pattern) {
            return Err(ItemComponentsError::InvalidTrimPattern);
        }
        if !valid_item_id(&self.material) {
            return Err(ItemComponentsError::InvalidTrimMaterial);
        }
        Ok(())
    }
}

fn validate_enchantment_entry(id: &str, level: u8) -> Result<(), ItemComponentsError> {
    if !valid_item_id(id) {
        return Err(ItemComponentsError::InvalidEnchantmentId);
    }
    if !(MIN_ENCHANTMENT_LEVEL..=MAX_ENCHANTMENT_LEVEL).contains(&level) {
        return Err(ItemComponentsError::InvalidEnchantmentLevel(level));
    }
    Ok(())
}

fn validate_enchantment_map(map: &BTreeMap<String, u8>) -> Result<(), ItemComponentsError> {
    for (id, &level) in map {
        validate_enchantment_entry(id, level)?;
    }
    Ok(())
}

fn insert_enchantment(
    map: &mut BTreeMap<String, u8>,
    id: String,
    level: u8,
) -> Result<(), ItemComponentsError> {
    validate_enchantment_entry(&id, level)?;
    map.insert(id, level);
    Ok(())
}

fn replace_enchantments(
    target: &mut BTreeMap<String, u8>,
    enchantments: BTreeMap<String, u8>,
) -> Result<(), ItemComponentsError> {
    validate_enchantment_map(&enchantments)?;
    *target = enchantments;
    Ok(())
}

/// Minimal text component for `minecraft:custom_name` and `minecraft:item_name`,
/// modeling plain literal strings and unparameterized translation keys.
///
/// Sources (minecraft.wiki `Text component format` revision 3862222):
/// - String / `{"text": ...}`: `"A string containing plain text to display directly."`
/// - `{"translate": ...}`: `"A translation identifier, corresponding to the identifiers
///   found in loaded language files."`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TextComponent {
    /// Plain literal text (`"..."` or `{"text": "..."}`).
    Literal(String),
    /// Translation key (`{"translate": key}`).
    Translatable { key: String },
}

impl TextComponent {
    /// Constructs a plain literal text component.
    #[must_use]
    pub fn literal(text: impl Into<String>) -> Self {
        Self::Literal(text.into())
    }

    /// Constructs a translatable text component from a translation key.
    #[must_use]
    pub fn translatable(key: impl Into<String>) -> Self {
        Self::Translatable { key: key.into() }
    }

    /// Returns the plain text if this component is [`TextComponent::Literal`].
    #[must_use]
    pub fn as_literal(&self) -> Option<&str> {
        match self {
            Self::Literal(text) => Some(text),
            Self::Translatable { .. } => None,
        }
    }

    /// Returns the translation key if this component is [`TextComponent::Translatable`].
    #[must_use]
    pub fn translation_key(&self) -> Option<&str> {
        match self {
            Self::Translatable { key } => Some(key),
            Self::Literal(_) => None,
        }
    }
}

impl From<String> for TextComponent {
    fn from(text: String) -> Self {
        Self::Literal(text)
    }
}

impl From<&str> for TextComponent {
    fn from(text: &str) -> Self {
        Self::Literal(text.to_owned())
    }
}

/// Single status effect entry stored in `minecraft:suspicious_stew_effects`.
///
/// Source: minecraft.wiki `Data component format/suspicious stew effects` (revision 3277335):
/// "`minecraft:suspicious_stew_effects`: A list of unamplified mob effects given to the player
/// or mob upon consuming this item (normally a suspicious stew)."
/// "`id`: The ID of the effect. `duration`: The duration of the effect in ticks. Defaults to 160."
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SuspiciousStewEffect {
    id: String,
    duration_ticks: u32,
}

impl SuspiciousStewEffect {
    /// Constructs a validated [`SuspiciousStewEffect`], ensuring `id` is a
    /// non-empty namespaced resource location.
    pub fn try_new(
        id: impl Into<String>,
        duration_ticks: u32,
    ) -> Result<Self, ItemComponentsError> {
        let entry = Self {
            id: id.into(),
            duration_ticks,
        };
        entry.validate()?;
        Ok(entry)
    }

    /// Constructs a validated [`SuspiciousStewEffect`] using
    /// [`DEFAULT_STEW_EFFECT_DURATION_TICKS`] (`160`).
    pub fn with_default_duration(id: impl Into<String>) -> Result<Self, ItemComponentsError> {
        Self::try_new(id, DEFAULT_STEW_EFFECT_DURATION_TICKS)
    }

    /// Returns the namespaced mob effect identifier (for example `"minecraft:night_vision"`).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the effect duration in ticks.
    #[must_use]
    pub const fn duration_ticks(&self) -> u32 {
        self.duration_ticks
    }

    fn validate(&self) -> Result<(), ItemComponentsError> {
        if !valid_item_id(&self.id) {
            return Err(ItemComponentsError::InvalidEffectId);
        }
        Ok(())
    }
}

fn validate_optional_id(
    value: Option<&str>,
    err: ItemComponentsError,
) -> Result<(), ItemComponentsError> {
    if let Some(id) = value
        && !valid_item_id(id)
    {
        return Err(err);
    }
    Ok(())
}

fn validate_ominous_bottle_amplifier(amplifier: Option<u8>) -> Result<(), ItemComponentsError> {
    if let Some(amp) = amplifier
        && !(MIN_OMINOUS_BOTTLE_AMPLIFIER..=MAX_OMINOUS_BOTTLE_AMPLIFIER).contains(&amp)
    {
        return Err(ItemComponentsError::InvalidOminousBottleAmplifier(amp));
    }
    Ok(())
}

/// Typed per-item data components modeled by RustMC.
///
/// Sources (minecraft.wiki `Data component format` revision 3903955 and subpages):
/// - `Data component format/damage` (revision 3536877):
///   "`minecraft:damage`: The number of uses consumed (not remaining) of the item's
///   durability. Must be a non-negative integer, defaults to 0."
/// - `Data component format/custom name` (revision 3863465):
///   "`minecraft:custom_name`: The player-assigned name of this item, block, or entity,
///   typically assigned with an anvil or a name tag."
/// - `Data component format/item name` (revision 3728022):
///   "`minecraft:item_name`: The default name of this item, as a text component, present on
///   all items by default. Unlike the `minecraft:custom_name` component, this name cannot be
///   erased using an anvil, is not italicized, and does not appear in some labels, such as
///   banner markers and item frames."
/// - `Data component format/enchantments` (revision 3849825):
///   "`minecraft:enchantments`: Contains a map of each of this item's enchantments to
///   its enchantment level."
/// - `Data component format/stored enchantments` (revision 3849830):
///   "`minecraft:stored_enchantments`: Contains a map of inactive enchantments and
///   their levels."
/// - `Data component format/repair cost` (revision 2710067):
///   "`minecraft:repair_cost`: The number of experience levels to add to the base
///   level cost when repairing, combining, or renaming this item with an anvil. Must
///   be a non-negative integer, defaults to 0."
/// - `Data component format/potion contents` (revision 3676971) and
///   `Entity format/potion contents` (revision 3620348):
///   "`potion`: The ID of a potion type. Optional."
/// - `Data component format/trim` (revision 3685746):
///   "`minecraft:trim`: Contains the trim applied to this armor piece."
/// - `Data component format/suspicious stew effects` (revision 3277335):
///   "`minecraft:suspicious_stew_effects`: A list of unamplified mob effects given to the
///   player or mob upon consuming this item (normally a suspicious stew)."
/// - `Data component format/instrument` (revision 3685633):
///   "`minecraft:instrument`: ... If present on an item, the description of the instrument is
///   shown in the item's tooltip as gray text."
/// - `Data component format/ominous bottle amplifier` (revision 3277333):
///   "`minecraft:ominous_bottle_amplifier`: The amplifier of the Bad Omen effect given to the
///   player or mob upon consuming this item (normally an ominous bottle). Must be a
///   non-negative integer from 0 to 4 (inclusive)."
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ItemComponents {
    damage: Option<u32>,
    custom_name: Option<TextComponent>,
    item_name: Option<TextComponent>,
    enchantments: BTreeMap<String, u8>,
    stored_enchantments: BTreeMap<String, u8>,
    repair_cost: Option<u32>,
    potion: Option<String>,
    trim: Option<ArmorTrim>,
    suspicious_stew_effects: Vec<SuspiciousStewEffect>,
    instrument: Option<String>,
    ominous_bottle_amplifier: Option<u8>,
}

static EMPTY_COMPONENTS: ItemComponents = ItemComponents::new();

impl ItemComponents {
    /// Creates an empty component set (all optional components unset).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            damage: None,
            custom_name: None,
            item_name: None,
            enchantments: BTreeMap::new(),
            stored_enchantments: BTreeMap::new(),
            repair_cost: None,
            potion: None,
            trim: None,
            suspicious_stew_effects: Vec::new(),
            instrument: None,
            ominous_bottle_amplifier: None,
        }
    }

    /// Returns `true` when no non-default data components are set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.damage.is_none()
            && self.custom_name.is_none()
            && self.item_name.is_none()
            && self.enchantments.is_empty()
            && self.stored_enchantments.is_empty()
            && self.repair_cost.is_none()
            && self.potion.is_none()
            && self.trim.is_none()
            && self.suspicious_stew_effects.is_empty()
            && self.instrument.is_none()
            && self.ominous_bottle_amplifier.is_none()
    }

    /// Returns the `minecraft:damage` component (uses consumed), if set.
    #[must_use]
    pub fn damage(&self) -> Option<u32> {
        self.damage
    }

    /// Returns the plain-text `minecraft:custom_name` component, if set as literal text.
    #[must_use]
    pub fn custom_name(&self) -> Option<&str> {
        self.custom_name
            .as_ref()
            .and_then(TextComponent::as_literal)
    }

    /// Returns the `minecraft:custom_name` text component, if set.
    #[must_use]
    pub fn custom_name_component(&self) -> Option<&TextComponent> {
        self.custom_name.as_ref()
    }

    /// Returns the plain-text `minecraft:item_name` component, if set as literal text.
    #[must_use]
    pub fn item_name(&self) -> Option<&str> {
        self.item_name.as_ref().and_then(TextComponent::as_literal)
    }

    /// Returns the `minecraft:item_name` text component, if set.
    #[must_use]
    pub fn item_name_component(&self) -> Option<&TextComponent> {
        self.item_name.as_ref()
    }

    /// Returns the active `minecraft:enchantments` map.
    #[must_use]
    pub fn enchantments(&self) -> &BTreeMap<String, u8> {
        &self.enchantments
    }

    /// Returns the inactive `minecraft:stored_enchantments` map (used on enchanted books).
    #[must_use]
    pub fn stored_enchantments(&self) -> &BTreeMap<String, u8> {
        &self.stored_enchantments
    }

    /// Returns the `minecraft:repair_cost` prior-work penalty, if set.
    #[must_use]
    pub fn repair_cost(&self) -> Option<u32> {
        self.repair_cost
    }

    /// Returns the base potion ID from `minecraft:potion_contents`, if set.
    #[must_use]
    pub fn potion(&self) -> Option<&str> {
        self.potion.as_deref()
    }

    /// Returns the `minecraft:trim` armor trim component, if set.
    #[must_use]
    pub fn trim(&self) -> Option<&ArmorTrim> {
        self.trim.as_ref()
    }

    /// Returns the `minecraft:suspicious_stew_effects` list.
    #[must_use]
    pub fn suspicious_stew_effects(&self) -> &[SuspiciousStewEffect] {
        &self.suspicious_stew_effects
    }

    /// Returns the `minecraft:instrument` identifier, if set.
    #[must_use]
    pub fn instrument(&self) -> Option<&str> {
        self.instrument.as_deref()
    }

    /// Returns the `minecraft:ominous_bottle_amplifier` level (`0..=4`), if set.
    #[must_use]
    pub fn ominous_bottle_amplifier(&self) -> Option<u8> {
        self.ominous_bottle_amplifier
    }

    /// Sets `minecraft:damage` (uses consumed) on this component set.
    #[must_use]
    pub fn with_damage(mut self, damage: u32) -> Self {
        self.damage = Some(damage);
        self
    }

    /// Sets `minecraft:custom_name` on this component set.
    #[must_use]
    pub fn with_custom_name(mut self, custom_name: impl Into<TextComponent>) -> Self {
        self.custom_name = Some(custom_name.into());
        self
    }

    /// Sets `minecraft:item_name` on this component set.
    #[must_use]
    pub fn with_item_name(mut self, item_name: impl Into<TextComponent>) -> Self {
        self.item_name = Some(item_name.into());
        self
    }

    /// Inserts or updates a validated active enchantment in `minecraft:enchantments`.
    pub fn with_enchantment(
        mut self,
        id: impl Into<String>,
        level: u8,
    ) -> Result<Self, ItemComponentsError> {
        self.set_enchantment(id, level)?;
        Ok(self)
    }

    /// Replaces `minecraft:enchantments` after validating every ID and level (`1..=255`).
    pub fn with_enchantments(
        mut self,
        enchantments: BTreeMap<String, u8>,
    ) -> Result<Self, ItemComponentsError> {
        self.set_enchantments(enchantments)?;
        Ok(self)
    }

    /// Inserts or updates a validated stored enchantment in `minecraft:stored_enchantments`.
    pub fn with_stored_enchantment(
        mut self,
        id: impl Into<String>,
        level: u8,
    ) -> Result<Self, ItemComponentsError> {
        self.set_stored_enchantment(id, level)?;
        Ok(self)
    }

    /// Replaces `minecraft:stored_enchantments` after validating every ID and level (`1..=255`).
    pub fn with_stored_enchantments(
        mut self,
        stored_enchantments: BTreeMap<String, u8>,
    ) -> Result<Self, ItemComponentsError> {
        self.set_stored_enchantments(stored_enchantments)?;
        Ok(self)
    }

    /// Sets `minecraft:repair_cost` on this component set.
    #[must_use]
    pub fn with_repair_cost(mut self, repair_cost: u32) -> Self {
        self.repair_cost = Some(repair_cost);
        self
    }

    /// Sets the `minecraft:potion_contents` base potion ID after validating its resource location.
    pub fn with_potion(mut self, potion: impl Into<String>) -> Result<Self, ItemComponentsError> {
        self.set_potion(Some(potion.into()))?;
        Ok(self)
    }

    /// Sets `minecraft:trim` on this component set.
    #[must_use]
    pub fn with_trim(mut self, trim: ArmorTrim) -> Self {
        self.trim = Some(trim);
        self
    }

    /// Appends a validated entry to `minecraft:suspicious_stew_effects`.
    pub fn with_suspicious_stew_effect(
        mut self,
        id: impl Into<String>,
        duration_ticks: u32,
    ) -> Result<Self, ItemComponentsError> {
        self.add_suspicious_stew_effect(id, duration_ticks)?;
        Ok(self)
    }

    /// Replaces `minecraft:suspicious_stew_effects` after validating every entry.
    pub fn with_suspicious_stew_effects(
        mut self,
        effects: Vec<SuspiciousStewEffect>,
    ) -> Result<Self, ItemComponentsError> {
        self.set_suspicious_stew_effects(effects)?;
        Ok(self)
    }

    /// Sets `minecraft:instrument` after validating its resource location.
    pub fn with_instrument(
        mut self,
        instrument: impl Into<String>,
    ) -> Result<Self, ItemComponentsError> {
        self.set_instrument(Some(instrument.into()))?;
        Ok(self)
    }

    /// Sets `minecraft:ominous_bottle_amplifier` after validating it is in `0..=4`.
    pub fn with_ominous_bottle_amplifier(
        mut self,
        amplifier: u8,
    ) -> Result<Self, ItemComponentsError> {
        self.set_ominous_bottle_amplifier(Some(amplifier))?;
        Ok(self)
    }

    /// Sets or clears `minecraft:damage`.
    pub fn set_damage(&mut self, damage: Option<u32>) {
        self.damage = damage;
    }

    /// Sets or clears `minecraft:custom_name`.
    pub fn set_custom_name(&mut self, custom_name: Option<TextComponent>) {
        self.custom_name = custom_name;
    }

    /// Sets or clears `minecraft:item_name`.
    pub fn set_item_name(&mut self, item_name: Option<TextComponent>) {
        self.item_name = item_name;
    }

    /// Inserts or updates a validated enchantment in `minecraft:enchantments`.
    pub fn set_enchantment(
        &mut self,
        id: impl Into<String>,
        level: u8,
    ) -> Result<(), ItemComponentsError> {
        insert_enchantment(&mut self.enchantments, id.into(), level)
    }

    /// Removes an enchantment from `minecraft:enchantments` if present.
    pub fn remove_enchantment(&mut self, id: &str) -> Option<u8> {
        self.enchantments.remove(id)
    }

    /// Replaces `minecraft:enchantments` after validating every entry.
    pub fn set_enchantments(
        &mut self,
        enchantments: BTreeMap<String, u8>,
    ) -> Result<(), ItemComponentsError> {
        replace_enchantments(&mut self.enchantments, enchantments)
    }

    /// Inserts or updates a validated enchantment in `minecraft:stored_enchantments`.
    pub fn set_stored_enchantment(
        &mut self,
        id: impl Into<String>,
        level: u8,
    ) -> Result<(), ItemComponentsError> {
        insert_enchantment(&mut self.stored_enchantments, id.into(), level)
    }

    /// Removes an enchantment from `minecraft:stored_enchantments` if present.
    pub fn remove_stored_enchantment(&mut self, id: &str) -> Option<u8> {
        self.stored_enchantments.remove(id)
    }

    /// Replaces `minecraft:stored_enchantments` after validating every entry.
    pub fn set_stored_enchantments(
        &mut self,
        stored_enchantments: BTreeMap<String, u8>,
    ) -> Result<(), ItemComponentsError> {
        replace_enchantments(&mut self.stored_enchantments, stored_enchantments)
    }

    /// Sets or clears `minecraft:repair_cost`.
    pub fn set_repair_cost(&mut self, repair_cost: Option<u32>) {
        self.repair_cost = repair_cost;
    }

    /// Sets or clears the `minecraft:potion_contents` base potion ID, validating resource location syntax.
    pub fn set_potion(&mut self, potion: Option<String>) -> Result<(), ItemComponentsError> {
        validate_optional_id(potion.as_deref(), ItemComponentsError::InvalidPotionId)?;
        self.potion = potion;
        Ok(())
    }

    /// Sets or clears `minecraft:trim`.
    pub fn set_trim(&mut self, trim: Option<ArmorTrim>) {
        self.trim = trim;
    }

    /// Appends a validated effect entry to `minecraft:suspicious_stew_effects`.
    pub fn add_suspicious_stew_effect(
        &mut self,
        id: impl Into<String>,
        duration_ticks: u32,
    ) -> Result<(), ItemComponentsError> {
        let entry = SuspiciousStewEffect::try_new(id, duration_ticks)?;
        self.suspicious_stew_effects.push(entry);
        Ok(())
    }

    /// Replaces `minecraft:suspicious_stew_effects` after validating every entry.
    pub fn set_suspicious_stew_effects(
        &mut self,
        effects: Vec<SuspiciousStewEffect>,
    ) -> Result<(), ItemComponentsError> {
        for effect in &effects {
            effect.validate()?;
        }
        self.suspicious_stew_effects = effects;
        Ok(())
    }

    /// Sets or clears `minecraft:instrument`, validating resource location syntax.
    pub fn set_instrument(
        &mut self,
        instrument: Option<String>,
    ) -> Result<(), ItemComponentsError> {
        validate_optional_id(
            instrument.as_deref(),
            ItemComponentsError::InvalidInstrumentId,
        )?;
        self.instrument = instrument;
        Ok(())
    }

    /// Sets or clears `minecraft:ominous_bottle_amplifier`, validating `0..=4`.
    pub fn set_ominous_bottle_amplifier(
        &mut self,
        amplifier: Option<u8>,
    ) -> Result<(), ItemComponentsError> {
        validate_ominous_bottle_amplifier(amplifier)?;
        self.ominous_bottle_amplifier = amplifier;
        Ok(())
    }

    /// Validates all component fields against Java Edition rules.
    pub fn validate(&self) -> Result<(), ItemComponentsError> {
        validate_enchantment_map(&self.enchantments)?;
        validate_enchantment_map(&self.stored_enchantments)?;
        validate_optional_id(self.potion.as_deref(), ItemComponentsError::InvalidPotionId)?;
        if let Some(ref trim) = self.trim {
            trim.validate()?;
        }
        for effect in &self.suspicious_stew_effects {
            effect.validate()?;
        }
        validate_optional_id(
            self.instrument.as_deref(),
            ItemComponentsError::InvalidInstrumentId,
        )?;
        validate_ominous_bottle_amplifier(self.ominous_bottle_amplifier)?;
        Ok(())
    }
}

/// Non-empty, capacity-bounded stack of a single item kind with typed data components.
#[derive(Debug, Clone)]
pub struct ItemStack {
    item: String,
    count: u16,
    max_stack: u16,
    components: Option<Box<ItemComponents>>,
}

impl PartialEq for ItemStack {
    fn eq(&self, other: &Self) -> bool {
        self.item == other.item
            && self.count == other.count
            && self.max_stack == other.max_stack
            && self.components() == other.components()
    }
}

impl Eq for ItemStack {}

impl std::hash::Hash for ItemStack {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.item.hash(state);
        self.count.hash(state);
        self.max_stack.hash(state);
        self.components().hash(state);
    }
}

const fn is_namespace_byte(b: u8) -> bool {
    matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.')
}

// Java Edition resource-location character rules.
pub(crate) const fn valid_item_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    let mut colon = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            colon = Some(i);
            break;
        }
        i += 1;
    }
    let Some(colon) = colon else {
        return false;
    };
    if colon == 0 || colon + 1 >= bytes.len() {
        return false;
    }
    let mut n = 0;
    while n < colon {
        if !is_namespace_byte(bytes[n]) {
            return false;
        }
        n += 1;
    }
    let mut p = colon + 1;
    while p < bytes.len() {
        if !is_namespace_byte(bytes[p]) && bytes[p] != b'/' {
            return false;
        }
        p += 1;
    }
    true
}

impl ItemStack {
    /// Constructs a validated stack with empty components, checking the item id,
    /// max stack size, and count in order.
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
            components: None,
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

    /// Returns the stack's typed data components.
    #[must_use]
    pub fn components(&self) -> &ItemComponents {
        self.components.as_deref().unwrap_or(&EMPTY_COMPONENTS)
    }

    /// Returns a mutable reference to the stack's validated data components.
    pub fn components_mut(&mut self) -> &mut ItemComponents {
        self.components.get_or_insert_with(Default::default)
    }

    /// Replaces the stack's data components after validating them.
    pub fn with_components(mut self, components: ItemComponents) -> Result<Self, ItemStackError> {
        components.validate()?;
        self.components = if components.is_empty() {
            None
        } else {
            Some(Box::new(components))
        };
        Ok(self)
    }

    /// Reports whether `other` shares the same item identifier, maximum stack size,
    /// and exact data components.
    ///
    /// Source: minecraft.wiki `Java Edition 1.21.9` (revision 3880956) /
    /// `Java Edition 25w34a` (revision 3601201):
    /// "Items stack based on exact contents, so even if a dynamic component is
    /// eventually resolved to the same profile as a static one, they won't stack together."
    /// Also minecraft.wiki `Anvil` (revision 3902804):
    /// "Named items do not stack with unnamed or differently-named items of the same type."
    pub fn can_stack_with(&self, other: &Self) -> bool {
        self.item == other.item
            && self.max_stack == other.max_stack
            && self.components() == other.components()
    }

    /// Adds as many items as fit under `max_stack` and returns the overflow.
    pub fn add(&mut self, amount: u16) -> u16 {
        let space = self.max_stack.saturating_sub(self.count);
        let accepted = amount.min(space);
        self.count = self.count.saturating_add(accepted);
        amount - accepted
    }

    /// Splits up to `amount` items off into a new stack and returns any remaining stack,
    /// preserving data components on both halves.
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
            components: self.components.clone(),
        };
        let rest = Self {
            item: self.item,
            count: remaining,
            max_stack: self.max_stack,
            components: self.components,
        };
        Ok((taken, Some(rest)))
    }
}

#[cfg(test)]
mod tests;
