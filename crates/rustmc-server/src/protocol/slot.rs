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

fn read_slot_varint(input: &[u8]) -> Result<(u32, usize), SlotError> {
    match java_wire::read_varint(input) {
        Ok(Some(pair)) => Ok(pair),
        Ok(None) | Err(WireError::Truncated) => Err(SlotError::Truncated),
        Err(WireError::VarIntTooLong | WireError::NonCanonicalVarInt) => {
            Err(SlotError::InvalidVarInt)
        }
        Err(
            WireError::TooLong { .. }
            | WireError::InvalidText
            | WireError::BelowCompressionThreshold { .. }
            | WireError::InvalidCompression
            | WireError::DecompressedSizeMismatch { .. },
        ) => Err(SlotError::InvalidTextComponent {
            component: "slot",
            reason: "invalid wire text",
        }),
    }
}

fn read_bool(input: &[u8]) -> Result<(bool, usize), SlotError> {
    let (&byte, _) = input.split_first().ok_or(SlotError::Truncated)?;
    match byte {
        0 => Ok((false, 1)),
        1 => Ok((true, 1)),
        other => Err(SlotError::InvalidBoolean(other)),
    }
}

fn require_component_id(ids: &ProtocolIds, name: &'static str) -> Result<u32, SlotError> {
    ids.component_type_id(name)
        .ok_or(SlotError::UnknownComponentTypeName(name))
}

fn encode_text_component(
    component_name: &'static str,
    text: &TextComponent,
    out: &mut Vec<u8>,
) -> Result<(), SlotError> {
    let map_err = |_| SlotError::InvalidTextComponent {
        component: component_name,
        reason: "text exceeds maximum NBT string byte length",
    };
    match text {
        TextComponent::Literal(plain) => java_wire::put_nbt_text(plain, out).map_err(map_err),
        TextComponent::Translatable { key } => {
            let start = out.len();
            out.push(NBT_TAG_COMPOUND);
            out.push(NBT_TAG_STRING);
            if let Err(err) = java_wire::put_nbt_string_payload("translate", out)
                .and_then(|()| java_wire::put_nbt_string_payload(key, out))
            {
                out.truncate(start);
                return Err(map_err(err));
            }
            out.push(NBT_TAG_END);
            Ok(())
        }
    }
}

fn decode_text_component(
    input: &[u8],
    component_name: &'static str,
) -> Result<(TextComponent, usize), SlotError> {
    let (tag, consumed) = read_anonymous_network_nbt(input).map_err(|err| match err {
        BlockEntityError::UnexpectedEof => SlotError::Truncated,
        BlockEntityError::Malformed(msg) => SlotError::InvalidTextComponent {
            component: component_name,
            reason: msg,
        },
        _ => SlotError::InvalidTextComponent {
            component: component_name,
            reason: "invalid network NBT in text component",
        },
    })?;
    match tag {
        NbtTag::String(text) => Ok((TextComponent::Literal(text), consumed)),
        NbtTag::Compound(entries) => {
            if entries.len() != 1 {
                return Err(SlotError::InvalidTextComponent {
                    component: component_name,
                    reason: "text component compound must contain only `text` or `translate`",
                });
            }
            let (key, value) = &entries[0];
            match (key.as_str(), value) {
                ("text", NbtTag::String(text)) => {
                    Ok((TextComponent::Literal(text.clone()), consumed))
                }
                ("translate", NbtTag::String(tr_key)) => Ok((
                    TextComponent::Translatable {
                        key: tr_key.clone(),
                    },
                    consumed,
                )),
                ("text" | "translate", _) => Err(SlotError::InvalidTextComponent {
                    component: component_name,
                    reason: "text component field value must be a TAG_String",
                }),
                _ => Err(SlotError::InvalidTextComponent {
                    component: component_name,
                    reason: "unsupported field in text component compound",
                }),
            }
        }
        _ => Err(SlotError::InvalidTextComponent {
            component: component_name,
            reason: "text component root tag must be TAG_Compound or TAG_String",
        }),
    }
}

fn encode_enchantment_map(
    map: &BTreeMap<String, u8>,
    dynamic_ids: &DynamicRegistryIds,
    out: &mut Vec<u8>,
) -> Result<(), SlotError> {
    if map.is_empty() || map.len() > MAX_ENCHANTMENTS_PER_COMPONENT {
        return Err(SlotError::InvalidEnchantmentCount {
            count: u32::try_from(map.len()).unwrap_or(u32::MAX),
            max: MAX_ENCHANTMENTS_PER_COMPONENT,
        });
    }
    let mut resolved = Vec::with_capacity(map.len());
    for (name, &level) in map {
        let id = dynamic_ids
            .enchantment_id(name)
            .ok_or_else(|| SlotError::UnknownEnchantmentName(name.clone()))?;
        resolved.push((id, level));
    }
    resolved.sort_by_key(|&(id, _)| id);
    put_varint(resolved.len() as u32, out);
    for (id, level) in resolved {
        put_varint(id, out);
        put_varint(u32::from(level), out);
    }
    Ok(())
}

fn decode_enchantment_map(
    mut input: &[u8],
    dynamic_ids: &DynamicRegistryIds,
) -> Result<(BTreeMap<String, u8>, usize), SlotError> {
    let start_len = input.len();
    let (count, prefix) = read_slot_varint(input)?;
    input = &input[prefix..];
    if count == 0 || count as usize > MAX_ENCHANTMENTS_PER_COMPONENT {
        return Err(SlotError::InvalidEnchantmentCount {
            count,
            max: MAX_ENCHANTMENTS_PER_COMPONENT,
        });
    }
    let mut map = BTreeMap::new();
    let mut seen_ids = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let (ench_id, id_len) = read_slot_varint(input)?;
        input = &input[id_len..];
        if seen_ids.contains(&ench_id) {
            return Err(SlotError::DuplicateEnchantment(ench_id));
        }
        seen_ids.push(ench_id);
        let ench_name = dynamic_ids
            .enchantment_name(ench_id)
            .ok_or(SlotError::UnknownEnchantmentId(ench_id))?;
        let (raw_level, lvl_len) = read_slot_varint(input)?;
        input = &input[lvl_len..];
        let Ok(level) = u8::try_from(raw_level) else {
            return Err(SlotError::InvalidEnchantmentLevel(raw_level));
        };
        if !(MIN_ENCHANTMENT_LEVEL..=MAX_ENCHANTMENT_LEVEL).contains(&level) {
            return Err(SlotError::InvalidEnchantmentLevel(raw_level));
        }
        map.insert(ench_name.to_owned(), level);
    }
    Ok((map, start_len - input.len()))
}

fn encode_potion_contents(
    potion: &str,
    ids: &ProtocolIds,
    out: &mut Vec<u8>,
) -> Result<(), SlotError> {
    let potion_id = ids
        .potion_id(potion)
        .ok_or_else(|| SlotError::UnknownPotionName(potion.to_owned()))?;
    out.push(1); // Potion ID present (Prefixed Optional VarInt)
    put_varint(potion_id, out);
    out.push(0); // Custom Color absent (Prefixed Optional Int)
    put_varint(0, out); // Custom Effects empty (Prefixed Array of Potion Effect)
    out.push(0); // Custom Name absent (Prefixed Optional String)
    Ok(())
}

fn decode_potion_contents(
    mut input: &[u8],
    ids: &ProtocolIds,
) -> Result<(String, usize), SlotError> {
    let start_len = input.len();
    let (has_potion, n) = read_bool(input)?;
    input = &input[n..];
    if !has_potion {
        return Err(SlotError::UnsupportedPotionContents);
    }
    let (potion_id, n) = read_slot_varint(input)?;
    input = &input[n..];
    let potion_name = ids
        .potion_name(potion_id)
        .ok_or(SlotError::UnknownPotionId(potion_id))?;

    let (has_custom_color, n) = read_bool(input)?;
    input = &input[n..];
    if has_custom_color {
        return Err(SlotError::UnsupportedPotionContents);
    }

    let (custom_effects_count, n) = read_slot_varint(input)?;
    input = &input[n..];
    if custom_effects_count != 0 {
        return Err(SlotError::UnsupportedPotionContents);
    }

    let (has_custom_name, n) = read_bool(input)?;
    input = &input[n..];
    if has_custom_name {
        return Err(SlotError::UnsupportedPotionContents);
    }

    Ok((potion_name.to_owned(), start_len - input.len()))
}

fn encode_registry_holder(id: u32, out: &mut Vec<u8>) -> Result<(), SlotError> {
    // `ID or X` (Java Edition protocol/Data types, revision 3855787):
    // 0 if inline value follows; otherwise registry ID + 1.
    let shifted = id
        .checked_add(1)
        .filter(|&v| v <= i32::MAX as u32)
        .ok_or(SlotError::UnsupportedInlineHolder)?;
    put_varint(shifted, out);
    Ok(())
}

fn decode_registry_holder(input: &[u8]) -> Result<(u32, usize), SlotError> {
    let (raw, n) = read_slot_varint(input)?;
    if raw == 0 || raw > i32::MAX as u32 {
        return Err(SlotError::UnsupportedInlineHolder);
    }
    Ok((raw - 1, n))
}

fn encode_trim(
    trim: &ArmorTrim,
    dynamic_ids: &DynamicRegistryIds,
    out: &mut Vec<u8>,
) -> Result<(), SlotError> {
    let material_id = dynamic_ids
        .trim_material_id(trim.material())
        .ok_or_else(|| SlotError::UnknownTrimMaterialName(trim.material().to_owned()))?;
    let pattern_id = dynamic_ids
        .trim_pattern_id(trim.pattern())
        .ok_or_else(|| SlotError::UnknownTrimPatternName(trim.pattern().to_owned()))?;
    encode_registry_holder(material_id, out)?;
    encode_registry_holder(pattern_id, out)?;
    Ok(())
}

fn decode_trim(
    mut input: &[u8],
    dynamic_ids: &DynamicRegistryIds,
) -> Result<(ArmorTrim, usize), SlotError> {
    let start_len = input.len();
    let (material_id, n) = decode_registry_holder(input)?;
    input = &input[n..];
    let material_name = dynamic_ids
        .trim_material_name(material_id)
        .ok_or(SlotError::UnknownTrimMaterialId(material_id))?;

    let (pattern_id, n) = decode_registry_holder(input)?;
    input = &input[n..];
    let pattern_name = dynamic_ids
        .trim_pattern_name(pattern_id)
        .ok_or(SlotError::UnknownTrimPatternId(pattern_id))?;

    let trim = ArmorTrim::try_new(pattern_name, material_name)
        .map_err(|err| SlotError::InvalidStack(ItemStackError::from(err)))?;
    Ok((trim, start_len - input.len()))
}

fn encode_suspicious_stew_effects(
    effects: &[SuspiciousStewEffect],
    ids: &ProtocolIds,
    out: &mut Vec<u8>,
) -> Result<(), SlotError> {
    if effects.len() > MAX_STEW_EFFECTS_PER_COMPONENT {
        return Err(SlotError::TooManyStewEffects {
            count: u32::try_from(effects.len()).unwrap_or(u32::MAX),
            max: MAX_STEW_EFFECTS_PER_COMPONENT,
        });
    }
    let mut resolved = Vec::with_capacity(effects.len());
    for effect in effects {
        let duration = effect.duration_ticks();
        if duration > i32::MAX as u32 {
            return Err(SlotError::NegativeComponentInt {
                component: COMPONENT_SUSPICIOUS_STEW_EFFECTS,
                raw: duration,
            });
        }
        let effect_id = ids
            .mob_effect_id(effect.id())
            .ok_or_else(|| SlotError::UnknownMobEffectName(effect.id().to_owned()))?;
        resolved.push((effect_id, duration));
    }
    put_varint(resolved.len() as u32, out);
    for (effect_id, duration) in resolved {
        put_varint(effect_id, out);
        put_varint(duration, out);
    }
    Ok(())
}

fn decode_suspicious_stew_effects(
    mut input: &[u8],
    ids: &ProtocolIds,
) -> Result<(Vec<SuspiciousStewEffect>, usize), SlotError> {
    let start_len = input.len();
    let (count, prefix) = read_slot_varint(input)?;
    input = &input[prefix..];
    if count as usize > MAX_STEW_EFFECTS_PER_COMPONENT {
        return Err(SlotError::TooManyStewEffects {
            count,
            max: MAX_STEW_EFFECTS_PER_COMPONENT,
        });
    }
    let mut effects = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let (effect_id, id_len) = read_slot_varint(input)?;
        input = &input[id_len..];
        let effect_name = ids
            .mob_effect_name(effect_id)
            .ok_or(SlotError::UnknownMobEffectId(effect_id))?;
        let (duration, dur_len) = read_slot_varint(input)?;
        input = &input[dur_len..];
        if duration > i32::MAX as u32 {
            return Err(SlotError::NegativeComponentInt {
                component: COMPONENT_SUSPICIOUS_STEW_EFFECTS,
                raw: duration,
            });
        }
        let entry = SuspiciousStewEffect::try_new(effect_name, duration)
            .map_err(|err| SlotError::InvalidStack(ItemStackError::from(err)))?;
        effects.push(entry);
    }
    Ok((effects, start_len - input.len()))
}

fn encode_instrument(
    instrument: &str,
    dynamic_ids: &DynamicRegistryIds,
    out: &mut Vec<u8>,
) -> Result<(), SlotError> {
    let id = dynamic_ids
        .instrument_id(instrument)
        .ok_or_else(|| SlotError::UnknownInstrumentName(instrument.to_owned()))?;
    encode_registry_holder(id, out)
}

fn decode_instrument(
    mut input: &[u8],
    dynamic_ids: &DynamicRegistryIds,
) -> Result<(String, usize), SlotError> {
    let start_len = input.len();
    let (id, n) = decode_registry_holder(input)?;
    input = &input[n..];
    let name = dynamic_ids
        .instrument_name(id)
        .ok_or(SlotError::UnknownInstrumentId(id))?;
    Ok((name.to_owned(), start_len - input.len()))
}

/// Encodes `slot` into `out` using the Java Edition 26.3 (protocol 777) `Slot` format.
///
/// Present data components on the stack are emitted in ascending
/// `minecraft:data_component_type` protocol ID order. If an error occurs, `out`
/// is left unmodified.
pub fn encode_slot(
    slot: &Option<ItemStack>,
    ids: &ProtocolIds,
    dynamic_ids: &DynamicRegistryIds,
    out: &mut Vec<u8>,
) -> Result<(), SlotError> {
    let Some(stack) = slot else {
        put_varint(0, out);
        return Ok(());
    };
    let count = u32::from(stack.count());
    if count == 0 || count > u32::from(MAX_STACK_LIMIT) {
        return Err(SlotError::InvalidItemCount(count));
    }
    let item_id = ids
        .item_id(stack.item())
        .ok_or_else(|| SlotError::UnknownItemName(stack.item().to_owned()))?;

    let components = stack.components();
    let mut encoded_components: Vec<(u32, Vec<u8>)> = Vec::new();

    if let Some(damage) = components.damage() {
        if damage > i32::MAX as u32 {
            return Err(SlotError::NegativeComponentInt {
                component: COMPONENT_DAMAGE,
                raw: damage,
            });
        }
        let type_id = require_component_id(ids, COMPONENT_DAMAGE)?;
        let mut data = Vec::new();
        put_varint(damage, &mut data);
        encoded_components.push((type_id, data));
    }

    if let Some(custom_name) = components.custom_name_component() {
        let type_id = require_component_id(ids, COMPONENT_CUSTOM_NAME)?;
        let mut data = Vec::new();
        encode_text_component(COMPONENT_CUSTOM_NAME, custom_name, &mut data)?;
        encoded_components.push((type_id, data));
    }

    if let Some(item_name) = components.item_name_component() {
        let type_id = require_component_id(ids, COMPONENT_ITEM_NAME)?;
        let mut data = Vec::new();
        encode_text_component(COMPONENT_ITEM_NAME, item_name, &mut data)?;
        encoded_components.push((type_id, data));
    }

    if !components.enchantments().is_empty() {
        let type_id = require_component_id(ids, COMPONENT_ENCHANTMENTS)?;
        let mut data = Vec::new();
        encode_enchantment_map(components.enchantments(), dynamic_ids, &mut data)?;
        encoded_components.push((type_id, data));
    }

    if !components.stored_enchantments().is_empty() {
        let type_id = require_component_id(ids, COMPONENT_STORED_ENCHANTMENTS)?;
        let mut data = Vec::new();
        encode_enchantment_map(components.stored_enchantments(), dynamic_ids, &mut data)?;
        encoded_components.push((type_id, data));
    }

    if let Some(repair_cost) = components.repair_cost() {
        if repair_cost > i32::MAX as u32 {
            return Err(SlotError::NegativeComponentInt {
                component: COMPONENT_REPAIR_COST,
                raw: repair_cost,
            });
        }
        let type_id = require_component_id(ids, COMPONENT_REPAIR_COST)?;
        let mut data = Vec::new();
        put_varint(repair_cost, &mut data);
        encoded_components.push((type_id, data));
    }

    if let Some(potion) = components.potion() {
        let type_id = require_component_id(ids, COMPONENT_POTION_CONTENTS)?;
        let mut data = Vec::new();
        encode_potion_contents(potion, ids, &mut data)?;
        encoded_components.push((type_id, data));
    }

    if !components.suspicious_stew_effects().is_empty() {
        let type_id = require_component_id(ids, COMPONENT_SUSPICIOUS_STEW_EFFECTS)?;
        let mut data = Vec::new();
        encode_suspicious_stew_effects(components.suspicious_stew_effects(), ids, &mut data)?;
        encoded_components.push((type_id, data));
    }

    if let Some(trim) = components.trim() {
        let type_id = require_component_id(ids, COMPONENT_TRIM)?;
        let mut data = Vec::new();
        encode_trim(trim, dynamic_ids, &mut data)?;
        encoded_components.push((type_id, data));
    }

    if let Some(instrument) = components.instrument() {
        let type_id = require_component_id(ids, COMPONENT_INSTRUMENT)?;
        let mut data = Vec::new();
        encode_instrument(instrument, dynamic_ids, &mut data)?;
        encoded_components.push((type_id, data));
    }

    if let Some(amplifier) = components.ominous_bottle_amplifier() {
        if amplifier > MAX_OMINOUS_BOTTLE_AMPLIFIER {
            return Err(SlotError::InvalidOminousBottleAmplifier(u32::from(
                amplifier,
            )));
        }
        let type_id = require_component_id(ids, COMPONENT_OMINOUS_BOTTLE_AMPLIFIER)?;
        let mut data = Vec::new();
        put_varint(u32::from(amplifier), &mut data);
        encoded_components.push((type_id, data));
    }

    encoded_components.sort_by_key(|&(type_id, _)| type_id);

    put_varint(count, out);
    put_varint(item_id, out);
    put_varint(encoded_components.len() as u32, out);
    put_varint(0, out); // Number of components to remove
    for (type_id, data) in encoded_components {
        put_varint(type_id, out);
        out.extend_from_slice(&data);
    }
    Ok(())
}

/// Decodes one Java Edition 26.3 (protocol 777) `Slot` from the prefix of `input`,
/// returning `(slot, bytes_consumed)`.
///
/// Unknown component type IDs or any removed-component entry return
/// [`SlotError::UnsupportedComponent`].
pub fn decode_slot(
    mut input: &[u8],
    ids: &ProtocolIds,
    dynamic_ids: &DynamicRegistryIds,
) -> Result<(Option<ItemStack>, usize), SlotError> {
    let start_len = input.len();
    let (raw_count, n) = read_slot_varint(input)?;
    input = &input[n..];
    if raw_count == 0 {
        return Ok((None, start_len - input.len()));
    }
    if raw_count > u32::from(MAX_STACK_LIMIT) {
        return Err(SlotError::InvalidItemCount(raw_count));
    }
    let count = raw_count as u16;

    let (item_id, n) = read_slot_varint(input)?;
    input = &input[n..];
    let item_name = ids
        .item_name(item_id)
        .ok_or(SlotError::UnknownItemId(item_id))?;

    let (add_count, n) = read_slot_varint(input)?;
    input = &input[n..];
    if add_count as usize > MAX_SLOT_COMPONENTS {
        return Err(SlotError::TooManyComponents {
            count: add_count,
            max: MAX_SLOT_COMPONENTS,
        });
    }

    let (remove_count, n) = read_slot_varint(input)?;
    input = &input[n..];
    if remove_count as usize > MAX_SLOT_COMPONENTS {
        return Err(SlotError::TooManyComponents {
            count: remove_count,
            max: MAX_SLOT_COMPONENTS,
        });
    }

    let mut components = ItemComponents::new();
    let mut seen_types = Vec::with_capacity(add_count as usize);

    for _ in 0..add_count {
        let (type_id, n) = read_slot_varint(input)?;
        input = &input[n..];
        if seen_types.contains(&type_id) {
            return Err(SlotError::DuplicateComponent(type_id));
        }
        seen_types.push(type_id);

        let Some(type_name) = ids.component_type_name(type_id) else {
            return Err(SlotError::UnsupportedComponent(type_id));
        };

        match type_name {
            COMPONENT_DAMAGE => {
                let (damage, n) = read_slot_varint(input)?;
                input = &input[n..];
                if damage > i32::MAX as u32 {
                    return Err(SlotError::NegativeComponentInt {
                        component: COMPONENT_DAMAGE,
                        raw: damage,
                    });
                }
                components.set_damage(Some(damage));
            }
            COMPONENT_CUSTOM_NAME => {
                let (name, n) = decode_text_component(input, COMPONENT_CUSTOM_NAME)?;
                input = &input[n..];
                components.set_custom_name(Some(name));
            }
            COMPONENT_ITEM_NAME => {
                let (name, n) = decode_text_component(input, COMPONENT_ITEM_NAME)?;
                input = &input[n..];
                components.set_item_name(Some(name));
            }
            COMPONENT_ENCHANTMENTS => {
                let (map, n) = decode_enchantment_map(input, dynamic_ids)?;
                input = &input[n..];
                components
                    .set_enchantments(map)
                    .map_err(|err| SlotError::InvalidStack(ItemStackError::from(err)))?;
            }
            COMPONENT_REPAIR_COST => {
                let (cost, n) = read_slot_varint(input)?;
                input = &input[n..];
                if cost > i32::MAX as u32 {
                    return Err(SlotError::NegativeComponentInt {
                        component: COMPONENT_REPAIR_COST,
                        raw: cost,
                    });
                }
                components.set_repair_cost(Some(cost));
            }
            COMPONENT_STORED_ENCHANTMENTS => {
                let (map, n) = decode_enchantment_map(input, dynamic_ids)?;
                input = &input[n..];
                components
                    .set_stored_enchantments(map)
                    .map_err(|err| SlotError::InvalidStack(ItemStackError::from(err)))?;
            }
            COMPONENT_POTION_CONTENTS => {
                let (potion, n) = decode_potion_contents(input, ids)?;
                input = &input[n..];
                components
                    .set_potion(Some(potion))
                    .map_err(|err| SlotError::InvalidStack(ItemStackError::from(err)))?;
            }
            COMPONENT_SUSPICIOUS_STEW_EFFECTS => {
                let (effects, n) = decode_suspicious_stew_effects(input, ids)?;
                input = &input[n..];
                components
                    .set_suspicious_stew_effects(effects)
                    .map_err(|err| SlotError::InvalidStack(ItemStackError::from(err)))?;
            }
            COMPONENT_TRIM => {
                let (trim, n) = decode_trim(input, dynamic_ids)?;
                input = &input[n..];
                components.set_trim(Some(trim));
            }
            COMPONENT_INSTRUMENT => {
                let (instrument, n) = decode_instrument(input, dynamic_ids)?;
                input = &input[n..];
                components
                    .set_instrument(Some(instrument))
                    .map_err(|err| SlotError::InvalidStack(ItemStackError::from(err)))?;
            }
            COMPONENT_OMINOUS_BOTTLE_AMPLIFIER => {
                let (raw_amp, n) = read_slot_varint(input)?;
                input = &input[n..];
                let Ok(amp) = u8::try_from(raw_amp) else {
                    return Err(SlotError::InvalidOminousBottleAmplifier(raw_amp));
                };
                components
                    .set_ominous_bottle_amplifier(Some(amp))
                    .map_err(|_| SlotError::InvalidOminousBottleAmplifier(raw_amp))?;
            }
            _ => return Err(SlotError::UnsupportedComponent(type_id)),
        }
    }

    if remove_count > 0 {
        let (removed_type_id, _) = read_slot_varint(input)?;
        return Err(SlotError::UnsupportedComponent(removed_type_id));
    }

    let max_stack = default_max_stack(item_name);
    let stack = ItemStack::try_new(item_name, count, max_stack)?.with_components(components)?;
    Ok((Some(stack), start_len - input.len()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::preview_data::RegistryManifest;

    const SYNTHETIC_REGISTRIES: &str = r#"{
        "minecraft:item": {
            "entries": {
                "minecraft:air": { "protocol_id": 0 },
                "minecraft:stone": { "protocol_id": 1 },
                "minecraft:diamond_sword": { "protocol_id": 851 },
                "minecraft:enchanted_book": { "protocol_id": 1120 },
                "minecraft:potion": { "protocol_id": 1044 },
                "minecraft:iron_chestplate": { "protocol_id": 883 },
                "minecraft:suspicious_stew": { "protocol_id": 1200 },
                "minecraft:goat_horn": { "protocol_id": 1250 },
                "minecraft:ominous_bottle": { "protocol_id": 1300 },
                "minecraft:map": { "protocol_id": 1100 }
            }
        },
        "minecraft:data_component_type": {
            "entries": {
                "minecraft:custom_data": { "protocol_id": 0 },
                "minecraft:damage": { "protocol_id": 3 },
                "minecraft:unbreakable": { "protocol_id": 4 },
                "minecraft:custom_name": { "protocol_id": 6 },
                "minecraft:item_name": { "protocol_id": 9 },
                "minecraft:enchantments": { "protocol_id": 13 },
                "minecraft:repair_cost": { "protocol_id": 19 },
                "minecraft:stored_enchantments": { "protocol_id": 45 },
                "minecraft:potion_contents": { "protocol_id": 53 },
                "minecraft:suspicious_stew_effects": { "protocol_id": 55 },
                "minecraft:trim": { "protocol_id": 58 },
                "minecraft:instrument": { "protocol_id": 63 },
                "minecraft:ominous_bottle_amplifier": { "protocol_id": 65 }
            }
        },
        "minecraft:potion": {
            "entries": {
                "minecraft:water": { "protocol_id": 0 },
                "minecraft:awkward": { "protocol_id": 3 },
                "minecraft:healing": { "protocol_id": 21 }
            }
        },
        "minecraft:mob_effect": {
            "entries": {
                "minecraft:speed": { "protocol_id": 0 },
                "minecraft:night_vision": { "protocol_id": 15 },
                "minecraft:poison": { "protocol_id": 18 },
                "minecraft:saturation": { "protocol_id": 22 }
            }
        }
    }"#;

    fn synthetic_ids() -> (ProtocolIds, DynamicRegistryIds) {
        let ids =
            ProtocolIds::from_json_str(Path::new("reports/registries.json"), SYNTHETIC_REGISTRIES)
                .expect("parse synthetic registries");
        let manifest = RegistryManifest {
            tags: Vec::new(),
            registries: vec![
                (
                    "minecraft:enchantment".to_owned(),
                    vec![
                        "minecraft:sharpness".to_owned(),
                        "minecraft:unbreaking".to_owned(),
                        "minecraft:mending".to_owned(),
                    ],
                ),
                (
                    "minecraft:trim_pattern".to_owned(),
                    vec!["minecraft:coast".to_owned(), "minecraft:ward".to_owned()],
                ),
                (
                    "minecraft:trim_material".to_owned(),
                    vec![
                        "minecraft:amethyst".to_owned(),
                        "minecraft:diamond".to_owned(),
                    ],
                ),
                (
                    "minecraft:instrument".to_owned(),
                    vec![
                        "minecraft:admire_goat_horn".to_owned(),
                        "minecraft:call_goat_horn".to_owned(),
                        "minecraft:dream_goat_horn".to_owned(),
                        "minecraft:feel_goat_horn".to_owned(),
                        "minecraft:ponder_goat_horn".to_owned(),
                        "minecraft:seek_goat_horn".to_owned(),
                        "minecraft:sing_goat_horn".to_owned(),
                        "minecraft:yearn_goat_horn".to_owned(),
                    ],
                ),
            ],
        };
        let dynamic_ids = DynamicRegistryIds::from_manifest(&manifest);
        (ids, dynamic_ids)
    }

    #[test]
    fn pins_exact_bytes_for_empty_slot_plain_stone_and_damaged_diamond_sword() {
        let (ids, dynamic_ids) = synthetic_ids();

        // 1. Empty slot -> single VarInt(0).
        let mut empty_bytes = Vec::new();
        encode_slot(&None, &ids, &dynamic_ids, &mut empty_bytes).expect("encode empty");
        assert_eq!(empty_bytes, [0x00]);
        assert_eq!(decode_slot(&empty_bytes, &ids, &dynamic_ids), Ok((None, 1)));

        // 2. minecraft:stone x1 with no components.
        let stone_id = ids.item_id("minecraft:stone").expect("stone id");
        let mut expected_stone = Vec::new();
        put_varint(1, &mut expected_stone); // count = 1
        put_varint(stone_id, &mut expected_stone); // item_id = 1
        put_varint(0, &mut expected_stone); // add_count = 0
        put_varint(0, &mut expected_stone); // remove_count = 0
        assert_eq!(expected_stone, [0x01, 0x01, 0x00, 0x00]);

        let stone = ItemStack::try_new("minecraft:stone", 1, default_max_stack("minecraft:stone"))
            .expect("stone stack");
        let mut stone_bytes = Vec::new();
        encode_slot(&Some(stone.clone()), &ids, &dynamic_ids, &mut stone_bytes)
            .expect("encode stone");
        assert_eq!(stone_bytes, expected_stone);
        assert_eq!(
            decode_slot(&stone_bytes, &ids, &dynamic_ids),
            Ok((Some(stone), stone_bytes.len()))
        );

        // 3. minecraft:diamond_sword x1 with damage 5.
        let sword_id = ids
            .item_id("minecraft:diamond_sword")
            .expect("diamond_sword id");
        let damage_type_id = ids
            .component_type_id("minecraft:damage")
            .expect("damage component id");
        let mut expected_sword = Vec::new();
        put_varint(1, &mut expected_sword); // count = 1
        put_varint(sword_id, &mut expected_sword); // item_id = 851 ([0xd3, 0x06])
        put_varint(1, &mut expected_sword); // add_count = 1
        put_varint(0, &mut expected_sword); // remove_count = 0
        put_varint(damage_type_id, &mut expected_sword); // component_type = 3
        put_varint(5, &mut expected_sword); // damage = 5
        assert_eq!(expected_sword, [0x01, 0xd3, 0x06, 0x01, 0x00, 0x03, 0x05]);

        let sword = ItemStack::try_new(
            "minecraft:diamond_sword",
            1,
            default_max_stack("minecraft:diamond_sword"),
        )
        .expect("sword stack")
        .with_components(ItemComponents::new().with_damage(5))
        .expect("with damage");
        let mut sword_bytes = Vec::new();
        encode_slot(&Some(sword.clone()), &ids, &dynamic_ids, &mut sword_bytes)
            .expect("encode sword");
        assert_eq!(sword_bytes, expected_sword);
        assert_eq!(
            decode_slot(&sword_bytes, &ids, &dynamic_ids),
            Ok((Some(sword), sword_bytes.len()))
        );
    }

    #[test]
    fn round_trips_each_component_alone_and_all_components_together_in_ascending_id_order() {
        let (ids, dynamic_ids) = synthetic_ids();

        let make_stack = |item: &str, count: u16, components: ItemComponents| {
            ItemStack::try_new(item, count, default_max_stack(item))
                .expect("valid item stack")
                .with_components(components)
                .expect("valid components")
        };

        // Each modeled component alone:
        let cases = vec![
            // damage
            make_stack(
                "minecraft:diamond_sword",
                1,
                ItemComponents::new().with_damage(42),
            ),
            // custom_name (literal and translatable)
            make_stack(
                "minecraft:diamond_sword",
                1,
                ItemComponents::new().with_custom_name("Excalibur \u{2694}"),
            ),
            make_stack(
                "minecraft:diamond_sword",
                1,
                ItemComponents::new().with_custom_name(TextComponent::translatable("item.blade")),
            ),
            // item_name (literal and translatable)
            make_stack(
                "minecraft:map",
                1,
                ItemComponents::new().with_item_name("Wayfinder"),
            ),
            make_stack(
                "minecraft:map",
                1,
                ItemComponents::new()
                    .with_item_name(TextComponent::translatable("filled_map.buried_treasure")),
            ),
            // enchantments
            make_stack(
                "minecraft:diamond_sword",
                1,
                ItemComponents::new()
                    .with_enchantment("minecraft:sharpness", 5)
                    .expect("sharpness")
                    .with_enchantment("minecraft:unbreaking", 3)
                    .expect("unbreaking"),
            ),
            // repair_cost
            make_stack(
                "minecraft:diamond_sword",
                1,
                ItemComponents::new().with_repair_cost(15),
            ),
            // stored_enchantments
            make_stack(
                "minecraft:enchanted_book",
                1,
                ItemComponents::new()
                    .with_stored_enchantment("minecraft:mending", 1)
                    .expect("mending"),
            ),
            // potion_contents
            make_stack(
                "minecraft:potion",
                1,
                ItemComponents::new()
                    .with_potion("minecraft:awkward")
                    .expect("awkward"),
            ),
            // suspicious_stew_effects
            make_stack(
                "minecraft:suspicious_stew",
                1,
                ItemComponents::new()
                    .with_suspicious_stew_effect("minecraft:night_vision", 160)
                    .expect("night_vision")
                    .with_suspicious_stew_effect("minecraft:saturation", 7)
                    .expect("saturation"),
            ),
            // trim
            make_stack(
                "minecraft:iron_chestplate",
                1,
                ItemComponents::new().with_trim(
                    ArmorTrim::try_new("minecraft:ward", "minecraft:diamond").expect("trim"),
                ),
            ),
            // instrument
            make_stack(
                "minecraft:goat_horn",
                1,
                ItemComponents::new()
                    .with_instrument("minecraft:ponder_goat_horn")
                    .expect("instrument"),
            ),
            // ominous_bottle_amplifier
            make_stack(
                "minecraft:ominous_bottle",
                1,
                ItemComponents::new()
                    .with_ominous_bottle_amplifier(4)
                    .expect("amplifier"),
            ),
        ];

        for stack in cases {
            let mut bytes = Vec::new();
            encode_slot(&Some(stack.clone()), &ids, &dynamic_ids, &mut bytes)
                .expect("encode single-component stack");
            let mut with_trailing = bytes.clone();
            with_trailing.extend_from_slice(&[0xfe, 0xff]);
            let (decoded, consumed) =
                decode_slot(&with_trailing, &ids, &dynamic_ids).expect("decode stack");
            assert_eq!(consumed, bytes.len());
            assert_eq!(decoded, Some(stack));
        }

        // Pin exact component bytes for custom_name, item_name (literal & translatable),
        // potion_contents, suspicious_stew_effects, trim, instrument, and ominous_bottle_amplifier.
        let named = make_stack(
            "minecraft:stone",
            2,
            ItemComponents::new().with_custom_name("Hi"),
        );
        let mut named_bytes = Vec::new();
        encode_slot(&Some(named), &ids, &dynamic_ids, &mut named_bytes).expect("encode named");
        assert_eq!(
            named_bytes,
            [
                0x02, // count = 2
                0x01, // item_id = 1 (stone)
                0x01, // add_count = 1
                0x00, // remove_count = 0
                0x06, // component_type = 6 (minecraft:custom_name)
                0x08, // TAG_String text component
                0x00, 0x02, b'H', b'i', // "Hi"
            ]
        );

        let translatable_item = make_stack(
            "minecraft:stone",
            1,
            ItemComponents::new().with_item_name(TextComponent::translatable("a.b")),
        );
        let mut tr_bytes = Vec::new();
        encode_slot(&Some(translatable_item), &ids, &dynamic_ids, &mut tr_bytes)
            .expect("encode translatable item_name");
        assert_eq!(
            tr_bytes,
            [
                0x01, // count = 1
                0x01, // item_id = 1 (stone)
                0x01, // add_count = 1
                0x00, // remove_count = 0
                0x09, // component_type = 9 (minecraft:item_name)
                0x0a, // TAG_Compound
                0x08, // TAG_String field
                0x00, 0x09, b't', b'r', b'a', b'n', b's', b'l', b'a', b't',
                b'e', // "translate"
                0x00, 0x03, b'a', b'.', b'b', // "a.b"
                0x00, // TAG_End
            ]
        );

        let stew = make_stack(
            "minecraft:suspicious_stew",
            1,
            ItemComponents::new()
                .with_suspicious_stew_effect("minecraft:night_vision", 140)
                .expect("night_vision")
                .with_suspicious_stew_effect("minecraft:saturation", 7)
                .expect("saturation"),
        );
        let mut stew_bytes = Vec::new();
        encode_slot(&Some(stew.clone()), &ids, &dynamic_ids, &mut stew_bytes).expect("encode stew");
        assert_eq!(
            &stew_bytes[stew_bytes.len() - 7..],
            &[
                55, // component_type = 55 (minecraft:suspicious_stew_effects)
                2,  // count = 2
                15, // minecraft:night_vision = 15
                0x8c, 0x01, // 140 ticks (VarInt)
                22,   // minecraft:saturation = 22
                7,    // 7 ticks
            ]
        );
        assert_eq!(
            decode_slot(&stew_bytes, &ids, &dynamic_ids),
            Ok((Some(stew), stew_bytes.len()))
        );

        let horn = make_stack(
            "minecraft:goat_horn",
            1,
            ItemComponents::new()
                .with_instrument("minecraft:sing_goat_horn")
                .expect("sing_goat_horn"),
        );
        let mut horn_bytes = Vec::new();
        encode_slot(&Some(horn.clone()), &ids, &dynamic_ids, &mut horn_bytes).expect("encode horn");
        // sing_goat_horn is index 6 -> ID or X encodes 6 + 1 = 7
        assert_eq!(&horn_bytes[horn_bytes.len() - 2..], &[63, 7]);
        assert_eq!(
            decode_slot(&horn_bytes, &ids, &dynamic_ids),
            Ok((Some(horn), horn_bytes.len()))
        );

        let bottle = make_stack(
            "minecraft:ominous_bottle",
            1,
            ItemComponents::new()
                .with_ominous_bottle_amplifier(3)
                .expect("amp 3"),
        );
        let mut bottle_bytes = Vec::new();
        encode_slot(&Some(bottle.clone()), &ids, &dynamic_ids, &mut bottle_bytes)
            .expect("encode ominous bottle");
        assert_eq!(&bottle_bytes[bottle_bytes.len() - 2..], &[65, 3]);
        assert_eq!(
            decode_slot(&bottle_bytes, &ids, &dynamic_ids),
            Ok((Some(bottle), bottle_bytes.len()))
        );

        let potion = make_stack(
            "minecraft:potion",
            1,
            ItemComponents::new()
                .with_potion("minecraft:awkward")
                .expect("awkward"),
        );
        let mut potion_bytes = Vec::new();
        encode_slot(&Some(potion), &ids, &dynamic_ids, &mut potion_bytes).expect("encode potion");
        let potion_item_id = ids.item_id("minecraft:potion").expect("potion item id");
        let potion_comp_id = ids
            .component_type_id("minecraft:potion_contents")
            .expect("potion_contents id");
        let awkward_id = ids.potion_id("minecraft:awkward").expect("awkward id");
        let mut expected_potion = Vec::new();
        put_varint(1, &mut expected_potion);
        put_varint(potion_item_id, &mut expected_potion);
        put_varint(1, &mut expected_potion);
        put_varint(0, &mut expected_potion);
        put_varint(potion_comp_id, &mut expected_potion);
        expected_potion.extend_from_slice(&[
            0x01, // has_potion = true
            awkward_id as u8,
            0x00, // has_custom_color = false
            0x00, // custom_effects length = 0
            0x00, // has_custom_name = false
        ]);
        assert_eq!(potion_bytes, expected_potion);

        let trimmed = make_stack(
            "minecraft:iron_chestplate",
            1,
            ItemComponents::new().with_trim(
                ArmorTrim::try_new("minecraft:ward", "minecraft:amethyst").expect("trim"),
            ),
        );
        let mut trim_bytes = Vec::new();
        encode_slot(&Some(trimmed), &ids, &dynamic_ids, &mut trim_bytes).expect("encode trim");
        // trim_material "minecraft:amethyst" has dynamic id 0 -> holder VarInt(1)
        // trim_pattern "minecraft:ward" has dynamic id 1 -> holder VarInt(2)
        assert_eq!(&trim_bytes[trim_bytes.len() - 3..], &[58, 1, 2]);

        // All 11 components together on one stack:
        let all_components = ItemComponents::new()
            .with_damage(17)
            .with_custom_name("Relic")
            .with_item_name(TextComponent::translatable("item.relic"))
            .with_enchantment("minecraft:unbreaking", 3)
            .expect("unbreaking")
            .with_enchantment("minecraft:sharpness", 4)
            .expect("sharpness")
            .with_repair_cost(7)
            .with_stored_enchantment("minecraft:mending", 1)
            .expect("mending")
            .with_potion("minecraft:healing")
            .expect("healing")
            .with_suspicious_stew_effect("minecraft:poison", 200)
            .expect("poison")
            .with_trim(ArmorTrim::try_new("minecraft:coast", "minecraft:diamond").expect("trim"))
            .with_instrument("minecraft:ponder_goat_horn")
            .expect("instrument")
            .with_ominous_bottle_amplifier(2)
            .expect("amplifier");
        let all_stack = make_stack("minecraft:diamond_sword", 1, all_components);

        let mut all_bytes = Vec::new();
        encode_slot(&Some(all_stack.clone()), &ids, &dynamic_ids, &mut all_bytes)
            .expect("encode all components");
        let (decoded_all, consumed_all) =
            decode_slot(&all_bytes, &ids, &dynamic_ids).expect("decode all components");
        assert_eq!(consumed_all, all_bytes.len());
        assert_eq!(decoded_all, Some(all_stack.clone()));

        // Verify ascending component type id order even when synthetic IDs are inverted.
        let inverted_json = r#"{
            "minecraft:item": {
                "entries": {
                    "minecraft:diamond_sword": { "protocol_id": 10 }
                }
            },
            "minecraft:data_component_type": {
                "entries": {
                    "minecraft:damage": { "protocol_id": 90 },
                    "minecraft:repair_cost": { "protocol_id": 12 }
                }
            },
            "minecraft:potion": {
                "entries": {}
            },
            "minecraft:mob_effect": {
                "entries": {}
            }
        }"#;
        let inverted_ids =
            ProtocolIds::from_json_str(Path::new("reports/registries.json"), inverted_json)
                .expect("inverted ids");
        let two_comp_stack = make_stack(
            "minecraft:diamond_sword",
            1,
            ItemComponents::new().with_damage(5).with_repair_cost(2),
        );
        let mut inverted_bytes = Vec::new();
        encode_slot(
            &Some(two_comp_stack.clone()),
            &inverted_ids,
            &dynamic_ids,
            &mut inverted_bytes,
        )
        .expect("encode with inverted ids");
        // repair_cost (id 12, value 2) must precede damage (id 90, value 5):
        assert_eq!(inverted_bytes, [1, 10, 2, 0, 12, 2, 90, 5]);
        assert_eq!(
            decode_slot(&inverted_bytes, &inverted_ids, &dynamic_ids),
            Ok((Some(two_comp_stack), inverted_bytes.len()))
        );
    }

    #[test]
    fn rejects_unsupported_or_removed_components_truncated_input_and_oversized_counts() {
        let (ids, dynamic_ids) = synthetic_ids();

        // 1. Truncated inputs at every stage of a multi-component slot.
        let full_stack = ItemStack::try_new("minecraft:diamond_sword", 1, 64)
            .expect("stack")
            .with_components(
                ItemComponents::new()
                    .with_damage(5)
                    .with_custom_name("Hero")
                    .with_enchantment("minecraft:sharpness", 2)
                    .expect("enchant")
                    .with_potion("minecraft:awkward")
                    .expect("potion")
                    .with_trim(
                        ArmorTrim::try_new("minecraft:coast", "minecraft:amethyst").expect("trim"),
                    ),
            )
            .expect("components");
        let mut full_bytes = Vec::new();
        encode_slot(&Some(full_stack), &ids, &dynamic_ids, &mut full_bytes).expect("encode");
        for cut in 0..full_bytes.len() {
            assert_eq!(
                decode_slot(&full_bytes[..cut], &ids, &dynamic_ids),
                Err(SlotError::Truncated),
                "prefix of length {cut} must report Truncated"
            );
        }

        // 2. Unknown component type ID -> UnsupportedComponent(id).
        let unknown_component_packet = [
            0x01, // count = 1
            0x01, // item_id = 1 (stone)
            0x01, // add_count = 1
            0x00, // remove_count = 0
            0x7f, // component_type = 127 (not in registries.json)
            0x00,
        ];
        assert_eq!(
            decode_slot(&unknown_component_packet, &ids, &dynamic_ids),
            Err(SlotError::UnsupportedComponent(127))
        );

        // 3. Known but unmodeled component type ID (minecraft:unbreakable = 4) -> UnsupportedComponent(4).
        let unmodeled_component_packet = [
            0x01, // count = 1
            0x01, // item_id = 1 (stone)
            0x01, // add_count = 1
            0x00, // remove_count = 0
            0x04, // component_type = 4 (minecraft:unbreakable)
        ];
        assert_eq!(
            decode_slot(&unmodeled_component_packet, &ids, &dynamic_ids),
            Err(SlotError::UnsupportedComponent(4))
        );

        // 4. Removed-component entry -> UnsupportedComponent(removed_id).
        let removed_component_packet = [
            0x01, // count = 1
            0x01, // item_id = 1 (stone)
            0x00, // add_count = 0
            0x01, // remove_count = 1
            0x03, // removed component_type = 3 (minecraft:damage)
        ];
        assert_eq!(
            decode_slot(&removed_component_packet, &ids, &dynamic_ids),
            Err(SlotError::UnsupportedComponent(3))
        );

        // 5. Removed-component entry after valid added component -> UnsupportedComponent(removed_id).
        let added_then_removed_packet = [
            0x01, // count = 1
            0x01, // item_id = 1 (stone)
            0x01, // add_count = 1
            0x01, // remove_count = 1
            0x03, 0x05, // added damage = 5
            0x13, // removed repair_cost = 19
        ];
        assert_eq!(
            decode_slot(&added_then_removed_packet, &ids, &dynamic_ids),
            Err(SlotError::UnsupportedComponent(19))
        );

        // 6. Oversized item count (> MAX_STACK_LIMIT = 99).
        let mut oversized_item_count = Vec::new();
        put_varint(100, &mut oversized_item_count);
        put_varint(1, &mut oversized_item_count);
        put_varint(0, &mut oversized_item_count);
        put_varint(0, &mut oversized_item_count);
        assert_eq!(
            decode_slot(&oversized_item_count, &ids, &dynamic_ids),
            Err(SlotError::InvalidItemCount(100))
        );

        // 7. Oversized added component count (> MAX_SLOT_COMPONENTS).
        let mut oversized_add_count = Vec::new();
        put_varint(1, &mut oversized_add_count);
        put_varint(1, &mut oversized_add_count);
        put_varint((MAX_SLOT_COMPONENTS + 1) as u32, &mut oversized_add_count);
        put_varint(0, &mut oversized_add_count);
        assert_eq!(
            decode_slot(&oversized_add_count, &ids, &dynamic_ids),
            Err(SlotError::TooManyComponents {
                count: (MAX_SLOT_COMPONENTS + 1) as u32,
                max: MAX_SLOT_COMPONENTS,
            })
        );

        // 8. Oversized removed component count (> MAX_SLOT_COMPONENTS).
        let mut oversized_remove_count = Vec::new();
        put_varint(1, &mut oversized_remove_count);
        put_varint(1, &mut oversized_remove_count);
        put_varint(0, &mut oversized_remove_count);
        put_varint(
            (MAX_SLOT_COMPONENTS + 1) as u32,
            &mut oversized_remove_count,
        );
        assert_eq!(
            decode_slot(&oversized_remove_count, &ids, &dynamic_ids),
            Err(SlotError::TooManyComponents {
                count: (MAX_SLOT_COMPONENTS + 1) as u32,
                max: MAX_SLOT_COMPONENTS,
            })
        );

        // 9. Oversized enchantment count (> MAX_ENCHANTMENTS_PER_COMPONENT).
        let mut oversized_enchant_count = Vec::new();
        put_varint(1, &mut oversized_enchant_count);
        put_varint(1, &mut oversized_enchant_count);
        put_varint(1, &mut oversized_enchant_count);
        put_varint(0, &mut oversized_enchant_count);
        put_varint(13, &mut oversized_enchant_count); // minecraft:enchantments
        put_varint(
            (MAX_ENCHANTMENTS_PER_COMPONENT + 1) as u32,
            &mut oversized_enchant_count,
        );
        assert_eq!(
            decode_slot(&oversized_enchant_count, &ids, &dynamic_ids),
            Err(SlotError::InvalidEnchantmentCount {
                count: (MAX_ENCHANTMENTS_PER_COMPONENT + 1) as u32,
                max: MAX_ENCHANTMENTS_PER_COMPONENT,
            })
        );

        // 10. Duplicate component type ID in Components to add.
        let duplicate_comp_packet = [
            0x01, // count = 1
            0x01, // item_id = 1
            0x02, // add_count = 2
            0x00, // remove_count = 0
            0x03, 0x01, // damage = 1
            0x03, 0x02, // duplicate damage = 2
        ];
        assert_eq!(
            decode_slot(&duplicate_comp_packet, &ids, &dynamic_ids),
            Err(SlotError::DuplicateComponent(3))
        );

        // 11. Inline trim/instrument holders (ID == 0) are rejected.
        for comp_id in [58_u8, 63_u8] {
            let inline_packet = [
                0x01,    // count = 1
                0x01,    // item_id = 1
                0x01,    // add_count = 1
                0x00,    // remove_count = 0
                comp_id, // minecraft:trim or minecraft:instrument
                0x00,    // inline ID = 0
            ];
            assert_eq!(
                decode_slot(&inline_packet, &ids, &dynamic_ids),
                Err(SlotError::UnsupportedInlineHolder)
            );
        }

        // 12. Out-of-range ominous_bottle_amplifier (> 4) is rejected.
        let bad_amp_packet = [
            0x01, // count = 1
            0x01, // item_id = 1
            0x01, // add_count = 1
            0x00, // remove_count = 0
            65,   // minecraft:ominous_bottle_amplifier
            5,    // > 4
        ];
        assert_eq!(
            decode_slot(&bad_amp_packet, &ids, &dynamic_ids),
            Err(SlotError::InvalidOminousBottleAmplifier(5))
        );

        // 13. Oversized suspicious_stew_effects count (> MAX_STEW_EFFECTS_PER_COMPONENT).
        let mut oversized_stew_count = Vec::new();
        put_varint(1, &mut oversized_stew_count);
        put_varint(1, &mut oversized_stew_count);
        put_varint(1, &mut oversized_stew_count);
        put_varint(0, &mut oversized_stew_count);
        put_varint(55, &mut oversized_stew_count); // minecraft:suspicious_stew_effects
        put_varint(
            (MAX_STEW_EFFECTS_PER_COMPONENT + 1) as u32,
            &mut oversized_stew_count,
        );
        assert_eq!(
            decode_slot(&oversized_stew_count, &ids, &dynamic_ids),
            Err(SlotError::TooManyStewEffects {
                count: (MAX_STEW_EFFECTS_PER_COMPONENT + 1) as u32,
                max: MAX_STEW_EFFECTS_PER_COMPONENT,
            })
        );

        // 14. Unknown mob effect ID and richer text compound (with extra field) are rejected.
        let bad_effect_packet = [
            0x01, 0x01, 0x01, 0x00, 55, // suspicious_stew_effects
            1,  // count = 1
            99, // unknown mob_effect id
            10, // duration
        ];
        assert_eq!(
            decode_slot(&bad_effect_packet, &ids, &dynamic_ids),
            Err(SlotError::UnknownMobEffectId(99))
        );

        let styled_item_name_packet = [
            0x01, 0x01, 0x01, 0x00, 9,    // item_name
            0x0a, // TAG_Compound
            0x08, 0x00, 0x09, b't', b'r', b'a', b'n', b's', b'l', b'a', b't', b'e', 0x00, 0x01,
            b'k', 0x08, 0x00, 0x05, b'c', b'o', b'l', b'o', b'r', 0x00, 0x04, b'g', b'o', b'l',
            b'd', 0x00, // TAG_End
        ];
        assert!(matches!(
            decode_slot(&styled_item_name_packet, &ids, &dynamic_ids),
            Err(SlotError::InvalidTextComponent {
                component: "minecraft:item_name",
                ..
            })
        ));
    }

    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_registries_round_trip_slot_with_all_components`
    #[test]
    #[ignore]
    fn vanilla_registries_round_trip_slot_with_all_components() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let ids = ProtocolIds::load(Path::new(&root)).expect("load reports/registries.json");
        let (_, dynamic_ids) = synthetic_ids();

        let stack = ItemStack::try_new(
            "minecraft:diamond_sword",
            1,
            default_max_stack("minecraft:diamond_sword"),
        )
        .expect("sword stack")
        .with_components(
            ItemComponents::new()
                .with_damage(5)
                .with_custom_name("Vanilla Blade")
                .with_item_name(TextComponent::translatable("item.minecraft.diamond_sword"))
                .with_enchantment("minecraft:sharpness", 5)
                .expect("sharpness")
                .with_repair_cost(3)
                .with_stored_enchantment("minecraft:unbreaking", 3)
                .expect("unbreaking")
                .with_potion("minecraft:awkward")
                .expect("awkward")
                .with_suspicious_stew_effect("minecraft:night_vision", 160)
                .expect("night_vision")
                .with_suspicious_stew_effect("minecraft:saturation", 8)
                .expect("saturation")
                .with_trim(ArmorTrim::try_new("minecraft:ward", "minecraft:diamond").expect("trim"))
                .with_instrument("minecraft:ponder_goat_horn")
                .expect("instrument")
                .with_ominous_bottle_amplifier(4)
                .expect("amplifier"),
        )
        .expect("components");

        let mut bytes = Vec::new();
        encode_slot(&Some(stack.clone()), &ids, &dynamic_ids, &mut bytes)
            .expect("encode with vanilla registries.json");
        let (decoded, consumed) =
            decode_slot(&bytes, &ids, &dynamic_ids).expect("decode with vanilla registries.json");
        assert_eq!(consumed, bytes.len());
        assert_eq!(decoded, Some(stack));

        for real_stack in [
            ItemStack::try_new(
                "minecraft:goat_horn",
                1,
                default_max_stack("minecraft:goat_horn"),
            )
            .expect("goat horn")
            .with_components(
                ItemComponents::new()
                    .with_instrument("minecraft:sing_goat_horn")
                    .expect("sing"),
            )
            .expect("horn comps"),
            ItemStack::try_new(
                "minecraft:suspicious_stew",
                1,
                default_max_stack("minecraft:suspicious_stew"),
            )
            .expect("stew")
            .with_components(
                ItemComponents::new()
                    .with_suspicious_stew_effect("minecraft:poison", 240)
                    .expect("poison"),
            )
            .expect("stew comps"),
            ItemStack::try_new(
                "minecraft:ominous_bottle",
                1,
                default_max_stack("minecraft:ominous_bottle"),
            )
            .expect("bottle")
            .with_components(
                ItemComponents::new()
                    .with_ominous_bottle_amplifier(0)
                    .expect("amp 0"),
            )
            .expect("bottle comps"),
            ItemStack::try_new(
                "minecraft:filled_map",
                1,
                default_max_stack("minecraft:filled_map"),
            )
            .expect("map")
            .with_components(
                ItemComponents::new()
                    .with_item_name(TextComponent::translatable("filled_map.buried_treasure")),
            )
            .expect("map comps"),
        ] {
            let mut item_bytes = Vec::new();
            encode_slot(
                &Some(real_stack.clone()),
                &ids,
                &dynamic_ids,
                &mut item_bytes,
            )
            .expect("encode item");
            let (decoded_item, len) =
                decode_slot(&item_bytes, &ids, &dynamic_ids).expect("decode item");
            assert_eq!(len, item_bytes.len());
            assert_eq!(decoded_item, Some(real_stack));
        }
    }
}
