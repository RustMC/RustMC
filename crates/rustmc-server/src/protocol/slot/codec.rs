//! The slot encoder and decoder: item id, count and the component
//! patch.

use super::*;

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
