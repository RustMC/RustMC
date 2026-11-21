//! Unit tests for `protocol::chat`.

use super::*;

// Wire fixtures below are written by hand from the protocol tables, not
// produced by this module's encoders.

const TIMESTAMP: [u8; 8] = [0x00, 0x00, 0x01, 0x8B, 0xCF, 0xE5, 0x68, 0x00];
const SALT: [u8; 8] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];

fn unsigned_chat_fixture() -> Vec<u8> {
    let mut bytes = vec![0x09, 0x02, b'h', b'i'];
    bytes.extend(TIMESTAMP);
    bytes.extend(SALT);
    bytes.push(0x00); // No signature.
    bytes.extend([0x00, 0x00, 0x00, 0x00, 0x00]); // Count, 3-byte bitset, checksum.
    bytes
}

#[test]
fn packet_ids_match_the_26_3_report() {
    assert_eq!(CHAT_ACK_PACKET_ID, 6);
    assert_eq!(CHAT_COMMAND_PACKET_ID, 7);
    assert_eq!(CHAT_COMMAND_SIGNED_PACKET_ID, 8);
    assert_eq!(CHAT_PACKET_ID, 9);
    assert_eq!(CHAT_SESSION_UPDATE_PACKET_ID, 10);
    assert_eq!(SYSTEM_CHAT_MESSAGE_PACKET_ID, 124);
}

#[test]
fn unsigned_chat_parses_the_mandatory_acknowledgment_tail() {
    let packet = ChatPacket::decode_packet(&unsigned_chat_fixture()).unwrap();
    assert_eq!(packet.message, "hi");
    assert_eq!(packet.timestamp, 0x0000_018B_CFE5_6800);
    assert_eq!(packet.salt, 0x1122_3344_5566_7788);
    assert_eq!(packet.signature, None);
    assert_eq!(packet.last_seen, LastSeenUpdate::default());
    assert_eq!(packet.encode_packet(), unsigned_chat_fixture());
}

#[test]
fn signed_chat_keeps_signature_and_tail_separate() {
    let mut bytes = vec![0x09, 0x01, b'x'];
    bytes.extend(TIMESTAMP);
    bytes.extend(SALT);
    bytes.push(0x01);
    bytes.extend([0xAB; 256]);
    bytes.extend([0x03, 0x05, 0x00, 0x80, 0x7F]);

    let packet = ChatPacket::decode_packet(&bytes).unwrap();
    assert_eq!(packet.signature, Some([0xAB; 256]));
    assert_eq!(
        packet.last_seen,
        LastSeenUpdate {
            message_count: 3,
            acknowledged: [0x05, 0x00, 0x80],
            checksum: 0x7F,
        }
    );
    assert_eq!(packet.encode_packet(), bytes);
}

#[test]
fn chat_rejects_missing_tail_trailing_bytes_and_invalid_flag() {
    let fixture = unsigned_chat_fixture();
    assert_eq!(
        ChatPacket::decode_packet(&fixture[..fixture.len() - 1]),
        Err(ProtocolError::UnexpectedEof)
    );

    let mut trailing = fixture.clone();
    trailing.push(0);
    assert!(matches!(
        ChatPacket::decode_packet(&trailing),
        Err(ProtocolError::Malformed(_))
    ));

    let mut bad_flag = fixture;
    bad_flag[20] = 0x02;
    assert!(matches!(
        ChatPacket::decode_packet(&bad_flag),
        Err(ProtocolError::Malformed(_))
    ));

    // A signature shorter than the fixed 256 bytes cannot be absorbed.
    let mut short = vec![0x09, 0x01, b'x'];
    short.extend(TIMESTAMP);
    short.extend(SALT);
    short.push(0x01);
    short.extend([0xAB; 200]);
    assert_eq!(
        ChatPacket::decode_packet(&short),
        Err(ProtocolError::UnexpectedEof)
    );
}

#[test]
fn chat_length_counts_utf16_code_units() {
    assert!(ChatPacket::unsigned("A".repeat(256), 0, 0).is_ok());
    assert_eq!(
        ChatPacket::unsigned("A".repeat(257), 0, 0),
        Err(ProtocolError::StringTooLong {
            max: 256,
            actual: 257
        })
    );
    // 129 supplementary characters are 258 code units.
    assert_eq!(
        ChatPacket::unsigned("\u{1F600}".repeat(129), 0, 0),
        Err(ProtocolError::StringTooLong {
            max: 256,
            actual: 258
        })
    );
}

#[test]
fn unsigned_command_is_only_a_string() {
    let bytes = [
        0x07, 0x0C, b't', b'i', b'm', b'e', b' ', b's', b'e', b't', b' ', b'd', b'a', b'y',
    ];
    let packet = ChatCommandPacket::decode_packet(&bytes).unwrap();
    assert_eq!(packet.command, "time set day");
    assert_eq!(packet.encode_packet(), bytes);
    assert_eq!(
        ChatCommandPacket::new("/time set day").unwrap().command,
        "time set day"
    );

    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(matches!(
        ChatCommandPacket::decode_packet(&trailing),
        Err(ProtocolError::Malformed(_))
    ));
    assert!(ChatCommandPacket::new("a".repeat(32767)).is_ok());
    assert!(ChatCommandPacket::new("a".repeat(32768)).is_err());
}

#[test]
fn signed_command_uses_fixed_signatures_and_acknowledgment_tail() {
    let mut bytes = vec![0x08, 0x03, b's', b'a', b'y'];
    bytes.extend(TIMESTAMP);
    bytes.extend(SALT);
    bytes.push(0x01); // One argument.
    bytes.extend([0x07, b'm', b'e', b's', b's', b'a', b'g', b'e']);
    bytes.extend([0xCD; 256]);
    bytes.extend([0x00, 0x00, 0x00, 0x00, 0x00]);

    let packet = SignedChatCommandPacket::decode_packet(&bytes).unwrap();
    assert_eq!(packet.command, "say");
    assert_eq!(
        packet.argument_signatures,
        vec![ArgumentSignature {
            name: "message".into(),
            signature: [0xCD; 256],
        }]
    );
    assert_eq!(packet.encode_packet(), bytes);

    let mut too_many = vec![0x08, 0x00];
    too_many.extend(TIMESTAMP);
    too_many.extend(SALT);
    too_many.push(0x09);
    assert!(matches!(
        SignedChatCommandPacket::decode_packet(&too_many),
        Err(ProtocolError::Oversized(_))
    ));
}

#[test]
fn system_chat_plain_text_is_a_string_tag() {
    let bytes = [0x7C, 0x08, 0x00, 0x02, b'h', b'i', 0x00];
    let packet = SystemChatMessagePacket::chat_text("hi").unwrap();
    assert_eq!(packet.encode_packet(), bytes);
    assert_eq!(
        SystemChatMessagePacket::decode_packet(&bytes).unwrap(),
        packet
    );
    assert_eq!(
        packet.to_frame(),
        [0x07, 0x7C, 0x08, 0x00, 0x02, b'h', b'i', 0x00]
    );
    assert!(
        SystemChatMessagePacket::overlay_text("hi")
            .unwrap()
            .is_overlay()
    );
}

#[test]
fn system_chat_styled_text_is_an_anonymous_compound() {
    #[rustfmt::skip]
    let bytes = [
        0x7C,
        0x0A, // Root compound, no name.
        0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x01, b'a',
        0x08, 0x00, 0x05, b'c', b'o', b'l', b'o', b'r', 0x00, 0x03, b'r', b'e', b'd',
        0x01, 0x00, 0x04, b'b', b'o', b'l', b'd', 0x01,
        0x09, 0x00, 0x05, b'e', b'x', b't', b'r', b'a', 0x0A, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x01, b'b',
        0x00, // End of the child compound.
        0x00, // End of the root compound.
        0x01, // Overlay.
    ];
    let component = ChatComponent::colored("a", ChatColor::Red)
        .with_bold(true)
        .add_extra(ChatComponent::text("b"));
    let packet = SystemChatMessagePacket::overlay(component).unwrap();
    assert_eq!(packet.encode_packet(), bytes);
    assert_eq!(
        SystemChatMessagePacket::decode_packet(&bytes).unwrap(),
        packet
    );
}

#[test]
fn nbt_strings_use_modified_utf8() {
    // U+1F600 is the surrogate pair D83D DE00, three bytes each.
    let emoji = [
        0x7C, 0x08, 0x00, 0x06, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80, 0x00,
    ];
    let packet = SystemChatMessagePacket::chat_text("\u{1F600}").unwrap();
    assert_eq!(packet.encode_packet(), emoji);
    assert_eq!(
        SystemChatMessagePacket::decode_packet(&emoji).unwrap(),
        packet
    );

    let nul = [0x7C, 0x08, 0x00, 0x02, 0xC0, 0x80, 0x00];
    let packet = SystemChatMessagePacket::chat_text("\0").unwrap();
    assert_eq!(packet.encode_packet(), nul);
    assert_eq!(
        SystemChatMessagePacket::decode_packet(&nul).unwrap(),
        packet
    );

    // Four-byte UTF-8 is not modified UTF-8.
    let standard = [0x7C, 0x08, 0x00, 0x04, 0xF0, 0x9F, 0x98, 0x80, 0x00];
    assert_eq!(
        SystemChatMessagePacket::decode_packet(&standard),
        Err(ProtocolError::InvalidUtf8)
    );
}

#[test]
fn text_component_bounds_are_enforced_both_ways() {
    let mut nested = ChatComponent::text("leaf");
    for _ in 0..MAX_TEXT_COMPONENT_DEPTH {
        nested = ChatComponent::text("").add_extra(nested);
    }
    assert!(matches!(
        SystemChatMessagePacket::chat(nested),
        Err(ProtocolError::Oversized(_))
    ));
    assert!(matches!(
        SystemChatMessagePacket::chat_text("a".repeat(65536)),
        Err(ProtocolError::Oversized(_))
    ));

    // A deeply nested hostile payload fails without exhausting the stack.
    let mut hostile = vec![0x0A];
    for _ in 0..100_000 {
        hostile.extend([0x09, 0x00, 0x05, b'e', b'x', b't', b'r', b'a', 0x0A]);
        hostile.extend([0x00, 0x00, 0x00, 0x01]);
    }
    assert!(matches!(
        SystemChatMessagePacket::decode_payload(&hostile),
        Err(ProtocolError::Oversized(_))
    ));

    // Content this model cannot represent is rejected, never dropped.
    #[rustfmt::skip]
    let translated = [
        0x0A,
        0x08, 0x00, 0x09, b't', b'r', b'a', b'n', b's', b'l', b'a', b't', b'e',
        0x00, 0x01, b'k',
        0x00, 0x00,
    ];
    assert!(matches!(
        SystemChatMessagePacket::decode_payload(&translated),
        Err(ProtocolError::Malformed(_))
    ));
}

#[test]
fn framed_and_unframed_entry_points_are_explicit() {
    // 120 characters make an unframed packet whose leading ID 0x7C equals
    // its remaining length, so guessing the format would misread it.
    let packet = SystemChatMessagePacket::chat_text("a".repeat(120)).unwrap();
    let unframed = packet.encode_packet();
    assert_eq!(unframed[0] as usize, unframed.len() - 1);
    assert_eq!(
        SystemChatMessagePacket::decode_packet(&unframed).unwrap(),
        packet
    );
    assert!(SystemChatMessagePacket::decode_frame(&unframed).is_err());
    assert_eq!(
        SystemChatMessagePacket::decode_frame(&packet.to_frame()).unwrap(),
        packet
    );
}

#[test]
fn test_chat_component_json_serialization() {
    // Plain text
    let plain = ChatComponent::text("Hello, world!");
    assert_eq!(plain.to_json(), "{\"text\":\"Hello, world!\"}");

    // Colored text
    let colored = ChatComponent::colored("Red text", ChatColor::Red);
    assert_eq!(
        colored.to_json(),
        "{\"color\":\"red\",\"text\":\"Red text\"}"
    );

    // Formatted sender message
    let player_msg = ChatComponent::player_message("Alice", "Hello there");
    assert_eq!(player_msg.to_json(), "{\"text\":\"<Alice> Hello there\"}");

    // Complex styled component with extra children
    let complex = ChatComponent::text("Prefix: ")
        .with_color(ChatColor::Gold)
        .with_bold(true)
        .add_extra(ChatComponent::colored("Suffix", ChatColor::Green).with_italic(true));
    let json = complex.to_json();
    assert!(json.contains("\"color\":\"gold\""));
    assert!(json.contains("\"bold\":true"));
    assert!(json.contains("\"extra\":["));
    assert!(json.contains("\"color\":\"green\""));
    assert!(json.contains("\"italic\":true"));

    // Round trip from_json
    let parsed = ChatComponent::from_json(&json).unwrap();
    assert_eq!(complex.text, parsed.text);
    assert_eq!(complex.color, parsed.color);
    assert_eq!(complex.bold, parsed.bold);
    assert_eq!(complex.extra.len(), 1);
    assert_eq!(complex.extra[0].text, "Suffix");
}

#[test]
fn test_broadcast_chat_helper() {
    let packet = broadcast_chat("Bob", "testing 123").unwrap();
    assert!(!packet.is_overlay());
    assert_eq!(packet.content().text, "<Bob> testing 123");
}

#[test]
fn test_packet_codec_malformed_and_errors() {
    assert_eq!(
        ChatPacket::decode_frame(&[]),
        Err(ProtocolError::UnexpectedEof)
    );
    assert_eq!(
        ChatCommandPacket::decode_frame(&[]),
        Err(ProtocolError::UnexpectedEof)
    );
    assert_eq!(
        SystemChatMessagePacket::decode_frame(&[]),
        Err(ProtocolError::UnexpectedEof)
    );

    // Length 3, packet ID 0x7F, two payload bytes.
    let wrong_id_packet = [0x03, 0x7F, 0x00, 0x00];
    assert_eq!(
        ChatPacket::decode_frame(&wrong_id_packet),
        Err(ProtocolError::WrongPacketId {
            expected: CHAT_PACKET_ID,
            actual: 0x7F
        })
    );
    assert_eq!(
        ChatCommandPacket::decode_frame(&wrong_id_packet),
        Err(ProtocolError::WrongPacketId {
            expected: CHAT_COMMAND_PACKET_ID,
            actual: 0x7F
        })
    );
    assert_eq!(
        SystemChatMessagePacket::decode_frame(&wrong_id_packet),
        Err(ProtocolError::WrongPacketId {
            expected: SYSTEM_CHAT_MESSAGE_PACKET_ID,
            actual: 0x7F
        })
    );

    // Timestamp present, salt missing.
    let mut truncated = vec![0x09, 0x02, b'H', b'i'];
    truncated.extend_from_slice(&[0; 8]);
    assert_eq!(
        ChatPacket::decode_packet(&truncated),
        Err(ProtocolError::UnexpectedEof)
    );

    let bad_overlay = [0x7C, 0x08, 0x00, 0x00, 0x02];
    assert!(matches!(
        SystemChatMessagePacket::decode_packet(&bad_overlay),
        Err(ProtocolError::Malformed(_))
    ));
}

#[test]
fn test_chat_component_styling_and_hex_colors() {
    // Hex color
    let custom_color = ChatColor::parse("#1234AB").unwrap();
    assert_eq!(custom_color.as_str(), "#1234ab");
    let comp = ChatComponent::colored("Custom", custom_color.clone())
        .with_underlined(true)
        .with_strikethrough(true)
        .with_obfuscated(true);
    let json = comp.to_json();
    assert!(json.contains("\"color\":\"#1234ab\""));
    assert!(json.contains("\"underlined\":true"));
    assert!(json.contains("\"strikethrough\":true"));
    assert!(json.contains("\"obfuscated\":true"));

    // Plain text extraction
    let parent = ChatComponent::text("First ")
        .add_extra(ChatComponent::text("Second "))
        .add_extra(ChatComponent::text("Third"));
    assert_eq!(parent.to_plain_text(), "First Second Third");

    // Display trait output matches to_json
    assert_eq!(format!("{comp}"), comp.to_json());
}
