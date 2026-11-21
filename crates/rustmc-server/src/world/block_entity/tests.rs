//! Unit tests for `world::block_entity`.

use super::*;

/// Hand-built wire fixtures: the IDs are the 26.3 report values, written
/// out independently of the module constants.
#[test]
fn sign_packets_use_the_26_3_wire_ids() {
    let update = UpdateSignPacket::new(BlockPos::new(0, 64, 0), true, Default::default())
        .unwrap()
        .frame();
    // Length 14, sign_update 62, packed position 64, front, four empty lines.
    assert_eq!(update, [14, 62, 0, 0, 0, 0, 0, 0, 0, 64, 1, 0, 0, 0, 0]);

    let data =
        BlockEntityDataPacket::from_sign(BlockPos::new(0, 64, 0), &SignBlockEntity::new()).frame();
    // Two-byte length (the sign NBT exceeds 127 bytes), then block_entity_data 6,
    // packed position, and minecraft:sign type 7.
    assert!(data[0] & 0x80 != 0 && data[1] & 0x80 == 0);
    assert_eq!(&data[2..12], &[6, 0, 0, 0, 0, 0, 0, 0, 64, 7]);
    assert_eq!(BlockEntity::Chest.type_id(), 1);
    assert_eq!(BlockEntity::Furnace.type_id(), 0);
}

#[test]
fn test_block_pos_packing_roundtrip() {
    let test_cases = [
        BlockPos::new(0, 0, 0),
        BlockPos::new(100, 64, -200),
        BlockPos::new(-1, -64, -1),
        BlockPos::new(BlockPos::MIN_XZ, BlockPos::MIN_Y, BlockPos::MIN_XZ),
        BlockPos::new(BlockPos::MAX_XZ, BlockPos::MAX_Y, BlockPos::MAX_XZ),
        BlockPos::new(-30_000_000, 319, 30_000_000),
        BlockPos::new(1234567, -15, -7654321),
    ];

    for pos in test_cases {
        assert!(pos.is_valid(), "coordinate {pos:?} should be valid");
        let packed = pos.pack();
        let unpacked = BlockPos::unpack(packed);
        assert_eq!(pos, unpacked, "failed roundtrip for {pos:?}");
    }

    let invalid = BlockPos::new(0, 320, 0);
    assert!(!invalid.is_valid());
    let invalid_xz = BlockPos::new(30_000_001, 0, 0);
    assert!(!invalid_xz.is_valid());
}

#[test]
fn test_sign_initialization_defaults() {
    let sign = SignBlockEntity::new();
    assert!(!sign.is_waxed());

    // Front text defaults
    assert_eq!(
        sign.front_text.messages,
        [String::new(), String::new(), String::new(), String::new()]
    );
    assert_eq!(sign.front_text.color, DyeColor::Black);
    assert!(!sign.front_text.has_glowing_text);

    // Back text defaults
    assert_eq!(
        sign.back_text.messages,
        [String::new(), String::new(), String::new(), String::new()]
    );
    assert_eq!(sign.back_text.color, DyeColor::Black);
    assert!(!sign.back_text.has_glowing_text);
}

#[test]
fn test_sign_text_modification_front_and_back() {
    let mut sign = SignBlockEntity::new();

    // Modify front lines
    sign.set_line(true, 0, "Welcome").unwrap();
    sign.set_line(true, 1, "to").unwrap();
    sign.set_line(true, 2, "RustMC").unwrap();
    sign.set_line(true, 3, "!").unwrap();

    assert_eq!(sign.front_text.line(0), Some("Welcome"));
    assert_eq!(sign.front_text.line(1), Some("to"));
    assert_eq!(sign.front_text.line(2), Some("RustMC"));
    assert_eq!(sign.front_text.line(3), Some("!"));

    // Back lines remain unaffected
    assert_eq!(sign.back_text.line(0), Some(""));

    // Modify back lines
    sign.set_line(false, 0, "Rear Line 0").unwrap();
    sign.set_line(false, 3, "Rear Line 3").unwrap();

    assert_eq!(sign.back_text.line(0), Some("Rear Line 0"));
    assert_eq!(sign.back_text.line(1), Some(""));
    assert_eq!(sign.back_text.line(3), Some("Rear Line 3"));

    // Index out of bounds
    assert_eq!(
        sign.set_line(true, 4, "Invalid"),
        Err(SignError::InvalidLineIndex(4))
    );
}

#[test]
fn test_sign_color_and_glowing_updates() {
    let mut sign = SignBlockEntity::new();

    // Front: red dye, glowing
    sign.set_color(true, DyeColor::Red).unwrap();
    sign.set_glowing(true, true).unwrap();

    // Back: blue dye, not glowing
    sign.set_color(false, DyeColor::Blue).unwrap();
    sign.set_glowing(false, false).unwrap();

    assert_eq!(sign.front_text.color, DyeColor::Red);
    assert!(sign.front_text.has_glowing_text);

    assert_eq!(sign.back_text.color, DyeColor::Blue);
    assert!(!sign.back_text.has_glowing_text);

    // Verify all 16 dye colors
    for color in DyeColor::ALL {
        assert_eq!(DyeColor::from_str_name(color.as_str()), Some(color));
    }
}

#[test]
fn test_line_length_validation_rejects_oversized_lines() {
    let mut sign = SignBlockEntity::new();

    // Exact bound 384 bytes should succeed
    let max_text = "a".repeat(MAX_LINE_BYTES);
    assert_eq!(max_text.len(), 384);
    assert!(sign.set_line(true, 0, max_text).is_ok());

    // 385 bytes should be rejected
    let too_long = "a".repeat(MAX_LINE_BYTES + 1);
    assert_eq!(too_long.len(), 385);
    assert_eq!(
        sign.set_line(true, 1, too_long.clone()),
        Err(SignError::LineTooLong {
            line: 1,
            bytes: 385
        })
    );

    // Batch lines rejection
    let batch = [
        "line 0".to_string(),
        "line 1".to_string(),
        too_long,
        "line 3".to_string(),
    ];
    assert_eq!(
        sign.set_lines(true, batch),
        Err(SignError::LineTooLong {
            line: 2,
            bytes: 385
        })
    );
}

#[test]
fn test_waxed_sign_rejects_further_edits() {
    let mut sign = SignBlockEntity::new();
    sign.set_line(true, 0, "Initial Text").unwrap();
    sign.set_waxed(true);
    assert!(sign.is_waxed());

    // Text mutation rejected
    assert_eq!(sign.set_line(true, 0, "Modified"), Err(SignError::Waxed));
    assert_eq!(
        sign.set_lines(false, ["a".into(), "b".into(), "c".into(), "d".into()]),
        Err(SignError::Waxed)
    );

    // Color & glowing updates rejected
    assert_eq!(sign.set_color(true, DyeColor::Green), Err(SignError::Waxed));
    assert_eq!(sign.set_glowing(true, true), Err(SignError::Waxed));

    // Original line unchanged
    assert_eq!(sign.front_text.line(0), Some("Initial Text"));
}

#[test]
fn test_block_entity_registry_operations() {
    let mut registry = BlockEntityRegistry::new();
    let pos1 = BlockPos::new(10, 64, 20);
    let pos2 = BlockPos::new(30, 70, 40);

    let mut sign1 = SignBlockEntity::new();
    sign1.set_line(true, 0, "Sign 1").unwrap();

    let mut sign2 = SignBlockEntity::new();
    sign2.set_line(true, 0, "Sign 2").unwrap();

    assert!(registry.insert_sign(pos1, sign1).is_none());
    assert!(registry.insert_sign(pos2, sign2).is_none());
    assert_eq!(registry.len(), 2);

    // Lookup
    assert_eq!(
        registry.get_sign(&pos1).unwrap().front_text.line(0),
        Some("Sign 1")
    );
    assert_eq!(
        registry.get_sign(&pos2).unwrap().front_text.line(0),
        Some("Sign 2")
    );

    // Mutable lookup
    let sign1_mut = registry.get_sign_mut(&pos1).unwrap();
    sign1_mut.set_line(true, 1, "Updated Line").unwrap();
    assert_eq!(
        registry.get_sign(&pos1).unwrap().front_text.line(1),
        Some("Updated Line")
    );

    // Removal on destruction
    let removed = registry.on_block_destruction(&pos1).unwrap();
    assert_eq!(removed.type_id(), TYPE_ID_SIGN);
    assert!(!registry.contains(&pos1));
    assert_eq!(registry.len(), 1);
    assert!(registry.contains(&pos2));
}

#[test]
fn test_block_entity_data_packet_encoding_and_decoding() {
    let pos = BlockPos::new(-5, 65, 42);
    let mut sign = SignBlockEntity::new();
    sign.set_line(true, 0, "RustMC Front").unwrap();
    sign.set_line(true, 3, "Front Line 3").unwrap();
    sign.set_color(true, DyeColor::Cyan).unwrap();
    sign.set_glowing(true, true).unwrap();

    sign.set_line(false, 1, "RustMC Back").unwrap();
    sign.set_color(false, DyeColor::Yellow).unwrap();
    sign.set_waxed(true);

    // Create packet from sign
    let packet = BlockEntityDataPacket::from_sign(pos, &sign);
    assert_eq!(packet.block_entity_type, TYPE_ID_SIGN);

    // Unframed payload round-trip
    let encoded = packet.encode();
    let decoded = BlockEntityDataPacket::decode(&encoded).expect("decode packet payload");
    assert_eq!(decoded.pos, pos);
    assert_eq!(decoded.block_entity_type, TYPE_ID_SIGN);

    let decoded_sign = decoded.to_sign().expect("convert decoded to sign");
    assert_eq!(decoded_sign, sign);

    // Framed round-trip
    let framed = packet.frame();
    let decoded_framed =
        BlockEntityDataPacket::decode_framed(&framed).expect("decode framed packet");
    assert_eq!(decoded_framed.pos, pos);
    assert_eq!(decoded_framed.to_sign().unwrap(), sign);
}

#[test]
fn test_update_sign_packet_roundtrip_and_validation() {
    let pos = BlockPos::new(100, 64, -200);
    let lines = [
        "First Line".to_string(),
        "Second Line".to_string(),
        "Third Line".to_string(),
        "Fourth Line".to_string(),
    ];

    let packet = UpdateSignPacket::new(pos, true, lines.clone()).unwrap();

    // Unframed round-trip
    let encoded = packet.encode();
    let decoded = UpdateSignPacket::decode(&encoded).expect("decode UpdateSignPacket");
    assert_eq!(decoded, packet);

    // Framed round-trip
    let framed = packet.frame();
    let decoded_framed =
        UpdateSignPacket::decode_framed(&framed).expect("decode framed UpdateSignPacket");
    assert_eq!(decoded_framed, packet);

    // Apply packet to sign
    let mut sign = SignBlockEntity::new();
    decoded.apply_to(&mut sign).expect("apply packet to sign");
    assert_eq!(sign.front_text.messages, lines);

    // Apply to waxed sign fails
    sign.set_waxed(true);
    assert_eq!(decoded.apply_to(&mut sign), Err(SignError::Waxed));

    // Malformed: invalid position
    let bad_pos = BlockPos::new(0, 400, 0); // Out of world bounds
    assert!(UpdateSignPacket::new(bad_pos, true, lines.clone()).is_err());

    // Malformed: oversized line
    let bad_lines = ["a".repeat(385), String::new(), String::new(), String::new()];
    assert!(UpdateSignPacket::new(pos, true, bad_lines).is_err());

    // Decode rejects trailing garbage
    let mut bad_encoded = encoded.clone();
    bad_encoded.push(0xff);
    assert!(matches!(
        UpdateSignPacket::decode(&bad_encoded),
        Err(BlockEntityError::Malformed(_))
    ));

    // Decode rejects invalid is_front_text byte
    let mut bad_bool = encoded.clone();
    bad_bool[8] = 2; // Not 0 or 1
    assert!(matches!(
        UpdateSignPacket::decode(&bad_bool),
        Err(BlockEntityError::Malformed(_))
    ));
}

#[test]
fn test_sign_synchronization_roundtrip_flow() {
    let mut registry = BlockEntityRegistry::new();
    let pos = BlockPos::new(12, 65, -88);

    // 1. Initial sign placement in registry
    let sign = SignBlockEntity::new();
    registry.insert_sign(pos, sign);

    // 2. Client submits UpdateSignPacket for front text
    let front_lines = [
        "Town Square".to_string(),
        "Market Ahead".to_string(),
        "Beware of Phantoms".to_string(),
        "[Shop Open]".to_string(),
    ];
    let client_packet = UpdateSignPacket::new(pos, true, front_lines.clone()).unwrap();
    let wire_update_bytes = client_packet.frame();

    // 3. Server decodes client packet
    let server_received = UpdateSignPacket::decode_framed(&wire_update_bytes).unwrap();
    assert_eq!(server_received.pos, pos);
    assert!(server_received.is_front_text);

    // 4. Server applies update to registered sign entity
    let sign_entry = registry.get_sign_mut(&pos).expect("sign in registry");
    server_received
        .apply_to(sign_entry)
        .expect("apply text update");
    sign_entry.set_color(true, DyeColor::Yellow).unwrap();
    sign_entry.set_glowing(true, true).unwrap();

    // 5. Server encodes BlockEntityDataPacket (0x06) for observers
    let sync_packet = BlockEntityDataPacket::from_sign(pos, sign_entry);
    let wire_sync_bytes = sync_packet.frame();

    // 6. Observing client decodes BlockEntityDataPacket
    let client_received = BlockEntityDataPacket::decode_framed(&wire_sync_bytes).unwrap();
    assert_eq!(client_received.pos, pos);
    assert_eq!(client_received.block_entity_type, TYPE_ID_SIGN);

    let synchronized_sign = client_received.to_sign().unwrap();
    assert_eq!(synchronized_sign.front_text.messages, front_lines);
    assert_eq!(synchronized_sign.front_text.color, DyeColor::Yellow);
    assert!(synchronized_sign.front_text.has_glowing_text);
    assert!(!synchronized_sign.is_waxed());

    // 7. Waxing the sign prevents further text edits
    let sign_entry = registry.get_sign_mut(&pos).unwrap();
    sign_entry.set_waxed(true);

    let malicious_edit = UpdateSignPacket::new(
        pos,
        true,
        ["Hacked".into(), "Text".into(), String::new(), String::new()],
    )
    .unwrap();
    assert_eq!(malicious_edit.apply_to(sign_entry), Err(SignError::Waxed));
    assert_eq!(sign_entry.front_text.messages, front_lines);
}

#[test]
fn test_modified_utf8_supplementary_characters_surrogate_pairs() {
    let text = "Hello 😀 world \0 test!";
    let encoded = encode_modified_utf8(text);
    let decoded = decode_modified_utf8(&encoded).expect("decode modified utf-8");
    assert_eq!(decoded, text);

    // Verify that 😀 (U+1F600) is encoded as a 6-byte surrogate pair
    let emoji_encoded = encode_modified_utf8("😀");
    assert_eq!(emoji_encoded.len(), 6);
    assert_eq!(emoji_encoded, &[0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80]);

    // Verify NBT string round-trip with emoji
    let tag = NbtTag::Compound(vec![(
        "emoji".to_string(),
        NbtTag::String("😀".to_string()),
    )]);
    let mut nbt_bytes = Vec::new();
    write_network_nbt(&tag, &mut nbt_bytes);
    let parsed = read_network_nbt(&nbt_bytes).expect("parse NBT with emoji");
    assert_eq!(parsed, tag);
}

#[test]
fn test_reject_nonempty_tag_end_list() {
    // Tag type 9 (List), element type 0 (TAG_End), declared count 1000
    let malformed_list = vec![
        10, // TAG_Compound
        9,  // TAG_List
        0, 4, b'l', b'i', b's', b't', // Name "list"
        0,    // elem_type: TAG_End
        0, 0, 0x03, 0xe8, // count: 1000 in BE
        0,    // TAG_End of root
    ];
    let res = read_network_nbt(&malformed_list);
    assert!(
        matches!(res, Err(BlockEntityError::Malformed(_))),
        "must reject nonempty TAG_End list, got {res:?}"
    );
}

#[test]
fn test_decode_named_root_compound_preserves_data() {
    // Standard NBT root named "root" containing string field "foo" = "bar"
    let tag = NbtTag::Compound(vec![("foo".to_string(), NbtTag::String("bar".to_string()))]);
    let mut named_bytes = Vec::new();
    write_named_network_nbt("root", &tag, &mut named_bytes);

    // Decode with read_network_nbt
    let decoded = read_network_nbt(&named_bytes).expect("read_network_nbt named root");
    assert_eq!(decoded, tag);

    // Decode with read_named_network_nbt
    let (root_name, decoded_named) =
        read_named_network_nbt(&named_bytes).expect("read_named_network_nbt");
    assert_eq!(root_name, "root");
    assert_eq!(decoded_named, tag);
}

#[test]
fn test_modified_utf8_rejects_overlong_encodings() {
    assert_eq!(decode_modified_utf8(&[0xC0, 0x80]), Ok("\0".to_string()));
    for overlong in [
        &[0xC1, 0x81][..],
        &[0xC0, 0xAF][..],
        &[0xE0, 0x80, 0x80][..],
        &[0xE0, 0x81, 0x81][..],
        &[0xE0, 0x9F, 0xBF][..],
    ] {
        assert!(
            matches!(
                decode_modified_utf8(overlong),
                Err(BlockEntityError::Malformed(_))
            ),
            "expected overlong modified UTF-8 {overlong:02X?} to be rejected"
        );
    }

    let overlong_nbt = [
        10, // TAG_Compound
        8,  // TAG_String
        0, 1, b'k', // key "k"
        0, 2, 0xC1, 0x81, // overlong 'A'
        0,    // TAG_End
    ];
    assert!(matches!(
        read_network_nbt(&overlong_nbt),
        Err(BlockEntityError::Malformed(_))
    ));
}
