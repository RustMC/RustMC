//! Java Edition 26.3 (protocol 777) network `Slot` encoder and decoder with
//! structured item data component patches.
//!
//! # Wire Format Specification
//!
//! All field layouts follow `minecraft.wiki`:
//! - `Java Edition protocol/Slot data` (revision `3763756`)
//! - `Java Edition protocol/Data types` (revision `3855787`)
//!
//! ## Top-Level `Slot` (`Java Edition protocol/Slot data`, revision `3763756`, `== Format ==`)
//!
//! ```text
//! | Name                           | Type                     | Meaning
//! | Item Count                     | VarInt                   | The item count. Every following field is only present if this value is greater than zero.
//! | Item ID                        | Optional VarInt          | ID in the `minecraft:item` registry.
//! | Number of components to add    | Optional VarInt          | Number of elements present in the first data component array
//! | Number of components to remove | Optional VarInt          | Number of elements present in the second data component array.
//! | Components to add              | Optional Array           | (Component type: VarInt Enum, Component data: Varies)
//! | Components to remove           | Optional Array           | (Component type: VarInt Enum)
//! ```
//!
//! Component ordering note: `Java Edition protocol/Slot data` (revision `3763756`)
//! leaves the order of elements in `Components to add` unspecified. [`encode_slot`]
//! emits only components actually present on the stack, sorted in ascending
//! `minecraft:data_component_type` protocol ID order for deterministic wire output.
//!
//! ## Modeled Structured Components (`Java Edition protocol/Slot data`, revision `3763756`, `== Structured components ==`)
//!
//! Note: in 26.3 (revision `3763756`), per-component `Show in tooltip` flags are not
//! part of `enchantments`, `stored_enchantments`, or `trim` (tooltip hiding moved to
//! `minecraft:tooltip_display`).
//!
//! - `minecraft:damage` (see also `Data component format/damage`, revision `3536877`):
//!   ```text
//!   | Name   | Type   | Description
//!   | Damage | VarInt |
//!   ```
//! - `minecraft:custom_name` (see also `Data component format/custom name`, revision `3863465`,
//!   and `Java Edition protocol/Data types`, revision `3855787`, `Text Component`):
//!   ```text
//!   | Name | Type           | Description
//!   | Name | Text Component |
//!   ```
//!   A plain-text name is encoded as a `TAG_String` text component ("As a String Tag:
//!   For components only containing text (no styling, no events etc.)") via
//!   `java_wire::put_nbt_text`; a translatable name (`{"translate": key}`) is encoded
//!   as a `TAG_Compound` containing only `translate` ("As a Compound Tag: Every other
//!   case."); decoding also accepts a `{"text": …}` compound.
//! - `minecraft:item_name` (see also `Data component format/item name`, revision `3728022`,
//!   and `Java Edition protocol/Data types`, revision `3855787`, `Text Component`):
//!   ```text
//!   | Name | Type           | Description
//!   | Name | Text Component |
//!   ```
//!   Encoded and decoded with the same plain/translatable `TextComponent` layout as
//!   `minecraft:custom_name`.
//! - `minecraft:enchantments` (see also `Data component format/enchantments`, revision `3849825`):
//!   ```text
//!   | Name                  | Type                        | Description
//!   | Enchantment / Type ID | Prefixed Array: VarInt Enum | The ID of the enchantment in the `minecraft:enchantment` registry.
//!   | Enchantment / Level   | Prefixed Array: VarInt      | The level of the enchantment.
//!   ```
//! - `minecraft:repair_cost` (see also `Data component format/repair cost`, revision `2710067`):
//!   ```text
//!   | Name | Type   | Description
//!   | Cost | VarInt |
//!   ```
//! - `minecraft:stored_enchantments` (see also `Data component format/stored enchantments`, revision `3849830`):
//!   ```text
//!   | Name                  | Type                        | Description
//!   | Enchantment / Type ID | Prefixed Array: VarInt Enum | The ID of the enchantment in the `minecraft:enchantment` registry.
//!   | Enchantment / Level   | Prefixed Array: VarInt      | The level of the enchantment.
//!   ```
//! - `minecraft:potion_contents` (see also `Data component format/potion contents`, revision `3676971`):
//!   ```text
//!   | Name           | Type                            | Description
//!   | Potion ID      | Prefixed Optional VarInt        | The ID of the potion type in the `minecraft:potion` registry.
//!   | Custom Color   | Prefixed Optional Int           | The RGB components of the color, encoded as an integer.
//!   | Custom Effects | Prefixed Array of Potion Effect | Any custom effects the potion might have.
//!   | Custom Name    | Prefixed Optional String        |
//!   ```
//!   RustMC models the base `Potion ID` only (`Some(potion_id)` followed by `Custom Color = false`,
//!   `Custom Effects = []`, `Custom Name = false`).
//! - `minecraft:suspicious_stew_effects` (see also `Data component format/suspicious stew effects`, revision `3277335`):
//!   ```text
//!   | Name              | Type                        | Description
//!   | Effect / Type ID  | Prefixed Array: VarInt Enum | The ID of the effect in the `minecraft:mob_effect` registry.
//!   | Effect / Duration | Prefixed Array: VarInt      | The duration of the effect.
//!   ```
//! - `minecraft:trim` (see also `Data component format/trim`, revision `3685746`, and
//!   `Java Edition protocol/Data types`, revision `3855787`, `=== ID or X ===`):
//!   ```text
//!   | Name          | Type                | Description
//!   | Trim Material | ID or Trim Material | ID in the `minecraft:trim_material` registry, or an inline definition.
//!   | Trim Pattern  | ID or Trim Pattern  | ID in the `minecraft:trim_pattern` registry, or an inline definition.
//!   ```
//!   `ID or X` (`Java Edition protocol/Data types`, revision `3855787`):
//!   ```text
//!   | Field Name | Field Type | Meaning
//!   | ID         | VarInt     | 0 if value of type X is given inline; otherwise registry ID + 1.
//!   | Value      | Optional X | Only present if ID is 0.
//!   ```
//! - `minecraft:instrument` (see also `Data component format/instrument`, revision `3685633`):
//!   ```text
//!   | Name       | Type             | Description
//!   | Instrument | ID or Instrument | ID in the `minecraft:instrument` registry, or an inline definition.
//!   ```
//! - `minecraft:ominous_bottle_amplifier` (see also `Data component format/ominous bottle amplifier`, revision `3277333`):
//!   ```text
//!   | Name      | Type   | Description
//!   | Amplifier | VarInt | Between 0 and 4.
//!   ```

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use crate::item::{
    ArmorTrim, ItemComponents, ItemStack, ItemStackError, MAX_OMINOUS_BOTTLE_AMPLIFIER,
    MAX_STACK_LIMIT, MIN_ENCHANTMENT_LEVEL, MIN_OMINOUS_BOTTLE_AMPLIFIER, SuspiciousStewEffect,
    TextComponent, default_max_stack,
};
use crate::java_wire::{self, MAX_NBT_STRING_BYTES, NBT_TAG_STRING, WireError, put_varint};
use crate::protocol::ids::{DynamicRegistryIds, ProtocolIds};
use crate::world::block_entity::{BlockEntityError, NbtTag, read_anonymous_network_nbt};
use crate::world::enchantment::MAX_ENCHANTMENT_LEVEL;

/// RustMC policy bound on component additions or removals in a single network `Slot`.
///
/// RustMC models 11 item data components (and vanilla 26.3 defines 122
/// `minecraft:data_component_type` entries in total).
pub const MAX_SLOT_COMPONENTS: usize = 32;

/// RustMC policy bound on enchantment entries in a single `minecraft:enchantments`
/// or `minecraft:stored_enchantments` component.
///
/// Vanilla 26.3 defines 43 enchantments in `data/minecraft/enchantment/`.
pub const MAX_ENCHANTMENTS_PER_COMPONENT: usize = 64;

/// RustMC policy bound on entries in a single `minecraft:suspicious_stew_effects` component.
///
/// Vanilla 26.3 defines 40 mob effects in `minecraft:mob_effect`.
pub const MAX_STEW_EFFECTS_PER_COMPONENT: usize = 64;

/// Maximum modified UTF-8 byte length of a `minecraft:custom_name` or `minecraft:item_name`
/// string value, matching the `u16::MAX` (`65,535`) NBT `TAG_String` length prefix limit
/// (`Java Edition protocol/Data types` revision `3855787`).
pub const MAX_CUSTOM_NAME_BYTES: usize = MAX_NBT_STRING_BYTES;

const COMPONENT_DAMAGE: &str = "minecraft:damage";
const COMPONENT_CUSTOM_NAME: &str = "minecraft:custom_name";
const COMPONENT_ITEM_NAME: &str = "minecraft:item_name";
const COMPONENT_ENCHANTMENTS: &str = "minecraft:enchantments";
const COMPONENT_REPAIR_COST: &str = "minecraft:repair_cost";
const COMPONENT_STORED_ENCHANTMENTS: &str = "minecraft:stored_enchantments";
const COMPONENT_POTION_CONTENTS: &str = "minecraft:potion_contents";
const COMPONENT_SUSPICIOUS_STEW_EFFECTS: &str = "minecraft:suspicious_stew_effects";
const COMPONENT_TRIM: &str = "minecraft:trim";
const COMPONENT_INSTRUMENT: &str = "minecraft:instrument";
const COMPONENT_OMINOUS_BOTTLE_AMPLIFIER: &str = "minecraft:ominous_bottle_amplifier";

const NBT_TAG_END: u8 = 0;
const NBT_TAG_COMPOUND: u8 = 10;

/// Errors returned while encoding or decoding a Java Edition 26.3 network `Slot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotError {
    /// Input ended before the full slot payload could be decoded.
    Truncated,
    /// A VarInt exceeded 5 bytes or 32 bits, or used a non-canonical encoding.
    InvalidVarInt,
    /// A boolean prefix byte was neither `0x00` nor `0x01`.
    InvalidBoolean(u8),
    /// Item count is outside `1..=MAX_STACK_LIMIT` (`1..=99`).
    InvalidItemCount(u32),
    /// Added or removed component count exceeds [`MAX_SLOT_COMPONENTS`].
    TooManyComponents { count: u32, max: usize },
    /// Enchantment count in a component is zero or exceeds [`MAX_ENCHANTMENTS_PER_COMPONENT`].
    InvalidEnchantmentCount { count: u32, max: usize },
    /// Suspicious stew effect count exceeds [`MAX_STEW_EFFECTS_PER_COMPONENT`].
    TooManyStewEffects { count: u32, max: usize },
    /// An added component type ID or removed component entry is unknown or not modeled by RustMC.
    UnsupportedComponent(u32),
    /// The same data component type ID appeared more than once in `Components to add`.
    DuplicateComponent(u32),
    /// An item name could not be resolved in [`ProtocolIds`] during encoding.
    UnknownItemName(String),
    /// An item numeric ID could not be resolved in [`ProtocolIds`] during decoding.
    UnknownItemId(u32),
    /// A required data component type name is missing from [`ProtocolIds`] during encoding.
    UnknownComponentTypeName(&'static str),
    /// An enchantment name could not be resolved in [`DynamicRegistryIds`] during encoding.
    UnknownEnchantmentName(String),
    /// An enchantment numeric ID could not be resolved in [`DynamicRegistryIds`] during decoding.
    UnknownEnchantmentId(u32),
    /// The same enchantment ID appeared more than once in an enchantment component array.
    DuplicateEnchantment(u32),
    /// An enchantment level on the wire is outside `1..=255`.
    InvalidEnchantmentLevel(u32),
    /// A potion name could not be resolved in [`ProtocolIds`] during encoding.
    UnknownPotionName(String),
    /// A potion numeric ID could not be resolved in [`ProtocolIds`] during decoding.
    UnknownPotionId(u32),
    /// `minecraft:potion_contents` omitted the base potion or set unmodeled custom fields.
    UnsupportedPotionContents,
    /// A mob effect name could not be resolved in [`ProtocolIds`] during encoding.
    UnknownMobEffectName(String),
    /// A mob effect numeric ID could not be resolved in [`ProtocolIds`] during decoding.
    UnknownMobEffectId(u32),
    /// A trim pattern name could not be resolved in [`DynamicRegistryIds`] during encoding.
    UnknownTrimPatternName(String),
    /// A trim pattern numeric ID could not be resolved in [`DynamicRegistryIds`] during decoding.
    UnknownTrimPatternId(u32),
    /// A trim material name could not be resolved in [`DynamicRegistryIds`] during encoding.
    UnknownTrimMaterialName(String),
    /// A trim material numeric ID could not be resolved in [`DynamicRegistryIds`] during decoding.
    UnknownTrimMaterialId(u32),
    /// An instrument name could not be resolved in [`DynamicRegistryIds`] during encoding.
    UnknownInstrumentName(String),
    /// An instrument numeric ID could not be resolved in [`DynamicRegistryIds`] during decoding.
    UnknownInstrumentId(u32),
    /// An `ID or X` field used an inline value (`ID == 0`) or exceeded `i32::MAX`.
    UnsupportedInlineHolder,
    /// `minecraft:ominous_bottle_amplifier` on the wire is outside `0..=4`.
    InvalidOminousBottleAmplifier(u32),
    /// Integer component field is negative as a signed 32-bit VarInt (`> i32::MAX`).
    NegativeComponentInt { component: &'static str, raw: u32 },
    /// `minecraft:custom_name` or `minecraft:item_name` exceeds [`MAX_CUSTOM_NAME_BYTES`] or is unmodeled.
    InvalidTextComponent {
        component: &'static str,
        reason: &'static str,
    },
    /// Constructing the validated [`ItemStack`] or [`ItemComponents`] failed.
    InvalidStack(ItemStackError),
}

impl fmt::Display for SlotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("unexpected end of slot payload"),
            Self::InvalidVarInt => f.write_str("invalid or non-canonical VarInt in slot payload"),
            Self::InvalidBoolean(byte) => {
                write!(f, "invalid boolean byte 0x{byte:02x} in slot payload")
            }
            Self::InvalidItemCount(count) => {
                write!(
                    f,
                    "slot item count {count} must be 0 or in 1..={MAX_STACK_LIMIT}"
                )
            }
            Self::TooManyComponents { count, max } => {
                write!(f, "slot component count {count} exceeds limit of {max}")
            }
            Self::InvalidEnchantmentCount { count, max } => {
                write!(f, "slot enchantment count {count} must be in 1..={max}")
            }
            Self::TooManyStewEffects { count, max } => {
                write!(
                    f,
                    "slot suspicious stew effect count {count} exceeds limit of {max}"
                )
            }
            Self::UnsupportedComponent(id) => {
                write!(f, "unsupported or removed data component type id {id}")
            }
            Self::DuplicateComponent(id) => {
                write!(f, "duplicate data component type id {id} in slot")
            }
            Self::UnknownItemName(name) => {
                write!(f, "item `{name}` is not present in protocol ids")
            }
            Self::UnknownItemId(id) => {
                write!(f, "item protocol id {id} is not present in protocol ids")
            }
            Self::UnknownComponentTypeName(name) => {
                write!(
                    f,
                    "data component type `{name}` is not present in protocol ids"
                )
            }
            Self::UnknownEnchantmentName(name) => {
                write!(
                    f,
                    "enchantment `{name}` is not present in dynamic registry ids"
                )
            }
            Self::UnknownEnchantmentId(id) => {
                write!(
                    f,
                    "enchantment network id {id} is not present in dynamic registry ids"
                )
            }
            Self::DuplicateEnchantment(id) => {
                write!(f, "duplicate enchantment network id {id} in slot component")
            }
            Self::InvalidEnchantmentLevel(level) => {
                write!(
                    f,
                    "enchantment level {level} must be in {MIN_ENCHANTMENT_LEVEL}..={MAX_ENCHANTMENT_LEVEL}"
                )
            }
            Self::UnknownPotionName(name) => {
                write!(f, "potion `{name}` is not present in protocol ids")
            }
            Self::UnknownPotionId(id) => {
                write!(f, "potion protocol id {id} is not present in protocol ids")
            }
            Self::UnsupportedPotionContents => {
                f.write_str("potion_contents must carry a base potion id and no custom fields")
            }
            Self::UnknownMobEffectName(name) => {
                write!(f, "mob effect `{name}` is not present in protocol ids")
            }
            Self::UnknownMobEffectId(id) => {
                write!(
                    f,
                    "mob effect protocol id {id} is not present in protocol ids"
                )
            }
            Self::UnknownTrimPatternName(name) => {
                write!(
                    f,
                    "trim pattern `{name}` is not present in dynamic registry ids"
                )
            }
            Self::UnknownTrimPatternId(id) => {
                write!(
                    f,
                    "trim pattern network id {id} is not present in dynamic registry ids"
                )
            }
            Self::UnknownTrimMaterialName(name) => {
                write!(
                    f,
                    "trim material `{name}` is not present in dynamic registry ids"
                )
            }
            Self::UnknownTrimMaterialId(id) => {
                write!(
                    f,
                    "trim material network id {id} is not present in dynamic registry ids"
                )
            }
            Self::UnknownInstrumentName(name) => {
                write!(
                    f,
                    "instrument `{name}` is not present in dynamic registry ids"
                )
            }
            Self::UnknownInstrumentId(id) => {
                write!(
                    f,
                    "instrument network id {id} is not present in dynamic registry ids"
                )
            }
            Self::UnsupportedInlineHolder => f.write_str(
                "inline registry holder definitions are not supported in slot component",
            ),
            Self::InvalidOminousBottleAmplifier(amp) => {
                write!(
                    f,
                    "ominous bottle amplifier {amp} must be in {MIN_OMINOUS_BOTTLE_AMPLIFIER}..={MAX_OMINOUS_BOTTLE_AMPLIFIER}"
                )
            }
            Self::NegativeComponentInt { component, raw } => {
                write!(
                    f,
                    "component `{component}` value {raw} exceeds non-negative i32::MAX"
                )
            }
            Self::InvalidTextComponent { component, reason } => {
                write!(f, "invalid {component} component: {reason}")
            }
            Self::InvalidStack(err) => write!(f, "{err}"),
        }
    }
}

impl Error for SlotError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidStack(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ItemStackError> for SlotError {
    fn from(err: ItemStackError) -> Self {
        Self::InvalidStack(err)
    }
}

mod components;
use components::*;
mod codec;
pub use codec::*;

#[cfg(test)]
mod tests;
