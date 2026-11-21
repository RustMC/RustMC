//! Encoders and decoders for the item data components the slot codec
//! carries (text, enchantments, potions, trims, stew effects, instruments).

use super::*;

pub(super) fn read_slot_varint(input: &[u8]) -> Result<(u32, usize), SlotError> {
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

pub(super) fn read_bool(input: &[u8]) -> Result<(bool, usize), SlotError> {
    let (&byte, _) = input.split_first().ok_or(SlotError::Truncated)?;
    match byte {
        0 => Ok((false, 1)),
        1 => Ok((true, 1)),
        other => Err(SlotError::InvalidBoolean(other)),
    }
}

pub(super) fn require_component_id(
    ids: &ProtocolIds,
    name: &'static str,
) -> Result<u32, SlotError> {
    ids.component_type_id(name)
        .ok_or(SlotError::UnknownComponentTypeName(name))
}

pub(super) fn encode_text_component(
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

pub(super) fn decode_text_component(
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

pub(super) fn encode_enchantment_map(
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

pub(super) fn decode_enchantment_map(
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

pub(super) fn encode_potion_contents(
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

pub(super) fn decode_potion_contents(
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

pub(super) fn encode_registry_holder(id: u32, out: &mut Vec<u8>) -> Result<(), SlotError> {
    // `ID or X` (Java Edition protocol/Data types, revision 3855787):
    // 0 if inline value follows; otherwise registry ID + 1.
    let shifted = id
        .checked_add(1)
        .filter(|&v| v <= i32::MAX as u32)
        .ok_or(SlotError::UnsupportedInlineHolder)?;
    put_varint(shifted, out);
    Ok(())
}

pub(super) fn decode_registry_holder(input: &[u8]) -> Result<(u32, usize), SlotError> {
    let (raw, n) = read_slot_varint(input)?;
    if raw == 0 || raw > i32::MAX as u32 {
        return Err(SlotError::UnsupportedInlineHolder);
    }
    Ok((raw - 1, n))
}

pub(super) fn encode_trim(
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

pub(super) fn decode_trim(
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

pub(super) fn encode_suspicious_stew_effects(
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

pub(super) fn decode_suspicious_stew_effects(
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

pub(super) fn encode_instrument(
    instrument: &str,
    dynamic_ids: &DynamicRegistryIds,
    out: &mut Vec<u8>,
) -> Result<(), SlotError> {
    let id = dynamic_ids
        .instrument_id(instrument)
        .ok_or_else(|| SlotError::UnknownInstrumentName(instrument.to_owned()))?;
    encode_registry_holder(id, out)
}

pub(super) fn decode_instrument(
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
