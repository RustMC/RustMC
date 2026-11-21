//! Unit tests for `protocol::slot`.

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
    encode_slot(&Some(stone.clone()), &ids, &dynamic_ids, &mut stone_bytes).expect("encode stone");
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
    encode_slot(&Some(sword.clone()), &ids, &dynamic_ids, &mut sword_bytes).expect("encode sword");
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
            0x00, 0x09, b't', b'r', b'a', b'n', b's', b'l', b'a', b't', b'e', // "translate"
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
        ItemComponents::new()
            .with_trim(ArmorTrim::try_new("minecraft:ward", "minecraft:amethyst").expect("trim")),
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
        0x08, 0x00, 0x09, b't', b'r', b'a', b'n', b's', b'l', b'a', b't', b'e', 0x00, 0x01, b'k',
        0x08, 0x00, 0x05, b'c', b'o', b'l', b'o', b'r', 0x00, 0x04, b'g', b'o', b'l', b'd',
        0x00, // TAG_End
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
