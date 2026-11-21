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
mod tests;
