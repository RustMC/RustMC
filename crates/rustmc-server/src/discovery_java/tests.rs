//! Unit tests for `discovery_java`.

use super::*;
fn handshake(version: u32) -> Vec<u8> {
    let mut body = Vec::new();
    put_varint(version, &mut body);
    body.push(1);
    body.push(b'x');
    body.extend(25565u16.to_be_bytes());
    body.push(1);
    frame(0, &body)
}
#[test]
fn split_status_and_ping() {
    let mut s = Session::default();
    let h = handshake(PROTOCOL as u32);
    for b in h {
        assert!(s.receive(&[b], 4096).unwrap().is_empty());
    }
    assert_eq!(s.state(), State::Status);
    let answer = s.receive(&frame(0, &[]), 4096).unwrap();
    assert!(String::from_utf8_lossy(&answer[0]).contains("777"));
    let nonce = [9u8; 8];
    assert_eq!(
        s.receive(&frame(1, &nonce), 4096).unwrap()[0],
        frame(1, &nonce)
    );
    assert_eq!(s.state(), State::Done);
}
#[test]
fn older_version_gets_current_status() {
    let mut s = Session::default();
    s.receive(&handshake(766), 4096).unwrap();
    assert!(String::from_utf8_lossy(&s.receive(&frame(0, &[]), 4096).unwrap()[0]).contains("777"));
}
#[test]
fn malformed_and_oversized() {
    assert_eq!(
        Session::default().receive(&[0x80, 0x80, 0x80, 0x80, 0x80], 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        Session::default().receive(&[0x81, 0x20], 4096),
        Err(Error::Oversized)
    );
    assert_eq!(
        Session::default().receive(&frame(0, &[]), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        Session::default().receive(&handshake(777), 3),
        Err(Error::Oversized)
    );
}
fn hello(name: &[u8]) -> Vec<u8> {
    let mut body = vec![name.len() as u8];
    body.extend_from_slice(name);
    body.extend_from_slice(&[7; 16]);
    frame(login::serverbound::HELLO, &body)
}

fn assert_refused(mut session: Session, version: u32, reason: &str) {
    let mut h = handshake(version);
    *h.last_mut().unwrap() = intent::LOGIN as u8;
    assert!(session.receive(&h, 4096).unwrap().is_empty());
    let reply = session.receive(&hello(b"Test"), 4096).unwrap();
    assert_eq!(reply.len(), 1);
    assert_eq!(reply[0][1], login::clientbound::LOGIN_DISCONNECT as u8);
    let text = String::from_utf8_lossy(&reply[0]);
    assert!(text.contains(&format!("{{\"text\":\"{reason}\"}}")));
    assert_eq!(session.state(), State::Refused);
    // Nothing further is accepted after the refusal.
    assert_eq!(
        session.receive(&hello(b"Test"), 4096),
        Err(Error::WrongState)
    );
}

#[test]
fn discovery_only_login_is_refused_with_a_reason() {
    assert_refused(Session::default(), 777, DISCOVERY_ONLY_REASON);
}

#[test]
fn refused_login_still_validates_the_hello() {
    let mut h = handshake(777);
    *h.last_mut().unwrap() = intent::LOGIN as u8;
    let mut session = Session::default();
    session.receive(&h, 4096).unwrap();
    assert_eq!(session.receive(&hello(b"x!"), 4096), Err(Error::Malformed));
}

#[test]
fn local_preview_login_is_version_gated_and_unverified() {
    assert_refused(Session::new(true), 766, VERSION_REASON);
    let mut current = handshake(777);
    *current.last_mut().unwrap() = 2;
    let mut s = Session::new(true);
    s.receive(&current, 4096).unwrap();
    assert_eq!(s.state(), State::Login);
    let mut login = vec![4];
    login.extend_from_slice(b"Test");
    login.extend_from_slice(&[7; 16]);
    let reply = s.receive(&frame(0, &login), 4096).unwrap();
    assert_eq!(reply.len(), 1);
    assert!(reply[0].windows(16).any(|w| w == [7; 16]));
    assert_eq!(s.state(), State::LoginAcknowledgement);
    s.receive(&frame(3, &[]), 4096).unwrap();
    assert_eq!(s.state(), State::Configuration);
    let mut known = vec![1];
    for text in ["minecraft", "core", VERSION] {
        put_string(text, &mut known);
    }
    s.receive(&frame(7, &known), 4096).unwrap();
    assert_eq!(s.state(), State::ConfigurationData);
}

#[test]
fn known_pack_reply_rejects_oversized_and_malformed_lists() {
    assert_eq!(recognizes_core_pack(&[17]), Err(Error::Oversized));
    assert_eq!(recognizes_core_pack(&[1, 255, 255]), Err(Error::Malformed));
    assert_eq!(recognizes_core_pack(&[0, 1]), Err(Error::Malformed));
    assert_eq!(recognizes_core_pack(&[0]), Ok(false));
}

#[test]
fn invalid_host_text_and_out_of_order_ping_are_rejected() {
    let mut h = handshake(777);
    h[5] = 0xff;
    assert_eq!(Session::default().receive(&h, 4096), Err(Error::Malformed));
    let mut s = Session::default();
    s.receive(&handshake(777), 4096).unwrap();
    assert_eq!(s.receive(&frame(1, &[0; 8]), 4096), Err(Error::WrongState));
}

#[test]
fn test_unpack_and_pack_block_pos() {
    let test_coords = [
        (0, 0, 0),
        (1, 2, 3),
        (-1, -2, -3),
        (10, 20, 30),
        (-10, -20, -30),
        (12345, -64, -98765),
        (-30_000_000, -64, -30_000_000),
        (30_000_000, 319, 30_000_000),
        (33_554_431, 2047, 33_554_431),
        (-33_554_432, -2048, -33_554_432),
        (1, -1, 1),
        (-1, 1, -1),
    ];
    for (x, y, z) in test_coords {
        let packed = pack_block_pos(x, y, z);
        let unpacked = unpack_block_pos(packed);
        assert_eq!((x, y, z), unpacked, "round trip failed for ({x}, {y}, {z})");
    }
}

#[test]
fn test_player_action_decoding_positive_and_negative_coords() {
    // Positive coordinates
    let action_pos = PlayerAction {
        status: PlayerActionStatus::StartedDigging,
        pos: (1234, 100, 5678),
        face: 1, // Up
        sequence: 42,
    };
    let encoded = encode_player_action(&action_pos);
    let decoded = decode_player_action(&encoded).expect("valid positive coords player action");
    assert_eq!(decoded, action_pos);

    // Negative coordinates
    let action_neg = PlayerAction {
        status: PlayerActionStatus::FinishedDigging,
        pos: (-500, -64, -1200),
        face: 2, // North
        sequence: 99,
    };
    let encoded_neg = encode_player_action(&action_neg);
    let decoded_neg =
        decode_player_action(&encoded_neg).expect("valid negative coords player action");
    assert_eq!(decoded_neg, action_neg);

    // Test all status variants (0..=6)
    let statuses = [
        PlayerActionStatus::StartedDigging,
        PlayerActionStatus::CancelledDigging,
        PlayerActionStatus::FinishedDigging,
        PlayerActionStatus::DropItemStack,
        PlayerActionStatus::DropItem,
        PlayerActionStatus::ShootArrowOrFinishEating,
        PlayerActionStatus::SwapItemInHand,
    ];
    for (i, status) in statuses.into_iter().enumerate() {
        let action = PlayerAction {
            status,
            pos: (i as i32 * 10, (i as i32 * 10) - 20, -(i as i32 * 10)),
            face: (i % 6) as u8,
            sequence: i as u32 + 1,
        };
        let enc = encode_player_action(&action);
        let dec = decode_player_action(&enc).expect("status variant decode");
        assert_eq!(dec, action);
    }

    // Test all faces 0..=5
    for face in 0..=5 {
        let action = PlayerAction {
            status: PlayerActionStatus::StartedDigging,
            pos: (0, 0, 0),
            face,
            sequence: 1,
        };
        let enc = encode_player_action(&action);
        let dec = decode_player_action(&enc).expect("face variant decode");
        assert_eq!(dec.face, face);
    }
}

#[test]
fn test_use_item_on_decoding_cursor_and_faces() {
    // Positive & negative coords with main hand
    let use_item = UseItemOn {
        hand: 0,
        pos: (42, 64, -84),
        face: 1, // Up
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.25,
        inside_block: false,
        world_border_hit: false,
        sequence: 10,
    };
    let encoded = encode_use_item_on(&use_item);
    let decoded = decode_use_item_on(&encoded).expect("valid use_item_on");
    assert_eq!(decoded, use_item);
    assert_eq!(decoded.cursor(), (0.5, 1.0, 0.25));

    // Off-hand with negative coords and inside_block = true
    let use_item_off = UseItemOn {
        hand: 1,
        pos: (-100, -50, 200),
        face: 4, // West
        cursor_x: 0.0,
        cursor_y: 0.0,
        cursor_z: 0.0,
        inside_block: true,
        world_border_hit: false,
        sequence: 100,
    };
    let encoded_off = encode_use_item_on(&use_item_off);
    let decoded_off = decode_use_item_on(&encoded_off).expect("valid off-hand use_item_on");
    assert_eq!(decoded_off, use_item_off);

    // Edge cursor coordinates (1.0, 1.0, 1.0)
    let use_edge = UseItemOn {
        hand: 0,
        pos: (0, 0, 0),
        face: 5,
        cursor_x: 1.0,
        cursor_y: 1.0,
        cursor_z: 1.0,
        inside_block: false,
        world_border_hit: false,
        sequence: 55,
    };
    let enc_edge = encode_use_item_on(&use_edge);
    let dec_edge = decode_use_item_on(&enc_edge).expect("edge cursor use_item_on");
    assert_eq!(dec_edge, use_edge);

    // All faces 0..=5
    for face in 0..=5 {
        let action = UseItemOn {
            hand: 0,
            pos: (0, 0, 0),
            face,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside_block: false,
            world_border_hit: false,
            sequence: 1,
        };
        let enc = encode_use_item_on(&action);
        let dec = decode_use_item_on(&enc).expect("face decode");
        assert_eq!(dec.face, face);
    }

    // Test with world_border_hit: u8 representation
    let mut with_border = Vec::new();
    put_varint(0, &mut with_border); // hand
    let packed = pack_block_pos(10, 20, 30);
    with_border.extend_from_slice(&packed.to_be_bytes());
    put_varint(3, &mut with_border); // face: South
    with_border.extend_from_slice(&0.25f32.to_be_bytes());
    with_border.extend_from_slice(&0.5f32.to_be_bytes());
    with_border.extend_from_slice(&0.75f32.to_be_bytes());
    with_border.push(0); // inside_block
    with_border.push(1); // world_border_hit
    put_varint(77, &mut with_border); // sequence
    let dec_border = decode_use_item_on(&with_border).expect("use_item_on with world_border_hit");
    assert_eq!(dec_border.hand, 0);
    assert_eq!(dec_border.pos, (10, 20, 30));
    assert_eq!(dec_border.face, 3);
    assert_eq!(dec_border.cursor_x, 0.25);
    assert_eq!(dec_border.cursor_y, 0.5);
    assert_eq!(dec_border.cursor_z, 0.75);
    assert!(!dec_border.inside_block);
    assert!(dec_border.world_border_hit);
    assert_eq!(dec_border.sequence, 77);
}

#[test]
fn test_malformed_packet_rejection() {
    // --- PlayerAction malformed tests ---
    // 1. Truncated bytes
    assert_eq!(decode_player_action(&[]), Err(Error::Malformed));
    assert_eq!(decode_player_action(&[0]), Err(Error::Malformed)); // Status only, missing pos
    let mut partial = vec![0];
    partial.extend_from_slice(&[0; 4]); // Only 4 bytes of pos
    assert_eq!(decode_player_action(&partial), Err(Error::Malformed));
    let mut missing_face = vec![0];
    missing_face.extend_from_slice(&0i64.to_be_bytes());
    assert_eq!(decode_player_action(&missing_face), Err(Error::Malformed));
    let mut missing_seq = missing_face.clone();
    missing_seq.push(1); // Face present, missing sequence
    assert_eq!(decode_player_action(&missing_seq), Err(Error::Malformed));
    let mut truncated_varint = missing_seq.clone();
    truncated_varint.push(0x80); // Incomplete varint
    assert_eq!(
        decode_player_action(&truncated_varint),
        Err(Error::Malformed)
    );

    // 2. Invalid face > 5
    let mut bad_face = missing_face.clone();
    bad_face.push(6); // Invalid face
    put_varint(1, &mut bad_face);
    assert_eq!(decode_player_action(&bad_face), Err(Error::Malformed));
    bad_face[9] = 255;
    assert_eq!(decode_player_action(&bad_face), Err(Error::Malformed));

    // 3. Invalid status
    let mut bad_status = Vec::new();
    put_varint(7, &mut bad_status); // Status 7 is invalid (only 0..=6)
    bad_status.extend_from_slice(&0i64.to_be_bytes());
    bad_status.push(0);
    put_varint(1, &mut bad_status);
    assert_eq!(decode_player_action(&bad_status), Err(Error::Malformed));
    let mut bad_status2 = Vec::new();
    put_varint(100, &mut bad_status2);
    bad_status2.extend_from_slice(&0i64.to_be_bytes());
    bad_status2.push(0);
    put_varint(1, &mut bad_status2);
    assert_eq!(decode_player_action(&bad_status2), Err(Error::Malformed));

    // 4. Invalid coordinate range
    for (bx, by, bz) in [
        (0, -65, 0),
        (0, 320, 0),
        (30_000_001, 0, 0),
        (-30_000_001, 0, 0),
        (0, 0, 30_000_001),
        (0, 0, -30_000_001),
    ] {
        let mut bad_coord = vec![0];
        let packed = pack_block_pos(bx, by, bz);
        bad_coord.extend_from_slice(&packed.to_be_bytes());
        bad_coord.push(0);
        put_varint(1, &mut bad_coord);
        assert_eq!(
            decode_player_action(&bad_coord),
            Err(Error::Malformed),
            "coord ({bx}, {by}, {bz}) should be rejected"
        );
    }

    // 5. Surplus / trailing bytes
    let mut surplus = encode_player_action(&PlayerAction {
        status: PlayerActionStatus::StartedDigging,
        pos: (0, 0, 0),
        face: 0,
        sequence: 1,
    });
    surplus.push(0xff);
    assert_eq!(decode_player_action(&surplus), Err(Error::Malformed));

    // --- UseItemOn malformed tests ---
    // 1. Truncated bytes
    assert_eq!(decode_use_item_on(&[]), Err(Error::Malformed));
    assert_eq!(decode_use_item_on(&[0]), Err(Error::Malformed)); // Hand only
    let mut u_partial = vec![0];
    u_partial.extend_from_slice(&[0; 4]); // Partial pos
    assert_eq!(decode_use_item_on(&u_partial), Err(Error::Malformed));
    let mut u_missing_face = vec![0];
    u_missing_face.extend_from_slice(&0i64.to_be_bytes());
    assert_eq!(decode_use_item_on(&u_missing_face), Err(Error::Malformed));
    let mut u_missing_cursor = u_missing_face.clone();
    put_varint(1, &mut u_missing_cursor); // Face present
    assert_eq!(decode_use_item_on(&u_missing_cursor), Err(Error::Malformed));
    u_missing_cursor.extend_from_slice(&0.5f32.to_be_bytes()); // Only 4 bytes cursor
    assert_eq!(decode_use_item_on(&u_missing_cursor), Err(Error::Malformed));
    let mut u_missing_inside = u_missing_face.clone();
    put_varint(1, &mut u_missing_inside);
    u_missing_inside.extend_from_slice(&0.5f32.to_be_bytes());
    u_missing_inside.extend_from_slice(&0.5f32.to_be_bytes());
    u_missing_inside.extend_from_slice(&0.5f32.to_be_bytes());
    assert_eq!(decode_use_item_on(&u_missing_inside), Err(Error::Malformed));
    let mut u_missing_seq = u_missing_inside.clone();
    u_missing_seq.push(0); // inside_block present, missing sequence
    assert_eq!(decode_use_item_on(&u_missing_seq), Err(Error::Malformed));

    // 2. Invalid hand > 1
    let mut bad_hand = Vec::new();
    put_varint(2, &mut bad_hand);
    bad_hand.extend_from_slice(&0i64.to_be_bytes());
    put_varint(1, &mut bad_hand);
    bad_hand.extend_from_slice(&0.5f32.to_be_bytes());
    bad_hand.extend_from_slice(&0.5f32.to_be_bytes());
    bad_hand.extend_from_slice(&0.5f32.to_be_bytes());
    bad_hand.push(0);
    put_varint(1, &mut bad_hand);
    assert_eq!(decode_use_item_on(&bad_hand), Err(Error::Malformed));

    // 3. Invalid face > 5
    let mut u_bad_face = Vec::new();
    put_varint(0, &mut u_bad_face);
    u_bad_face.extend_from_slice(&0i64.to_be_bytes());
    put_varint(6, &mut u_bad_face); // Face 6 is invalid
    u_bad_face.extend_from_slice(&0.5f32.to_be_bytes());
    u_bad_face.extend_from_slice(&0.5f32.to_be_bytes());
    u_bad_face.extend_from_slice(&0.5f32.to_be_bytes());
    u_bad_face.push(0);
    put_varint(1, &mut u_bad_face);
    assert_eq!(decode_use_item_on(&u_bad_face), Err(Error::Malformed));

    // 4. NaN, infinity, and out-of-range cursor floats
    let test_bad_floats = [
        (f32::NAN, 0.5f32, 0.5f32),
        (0.5f32, f32::NAN, 0.5f32),
        (0.5f32, 0.5f32, f32::NAN),
        (f32::INFINITY, 0.5f32, 0.5f32),
        (0.5f32, f32::NEG_INFINITY, 0.5f32),
        (-0.01f32, 0.5f32, 0.5f32),
        (0.5f32, 1.01f32, 0.5f32),
        (0.5f32, 0.5f32, -1.0f32),
        (0.5f32, 0.5f32, 2.0f32),
    ];
    for (cx, cy, cz) in test_bad_floats {
        let mut bad_cursor = Vec::new();
        put_varint(0, &mut bad_cursor);
        bad_cursor.extend_from_slice(&0i64.to_be_bytes());
        put_varint(0, &mut bad_cursor);
        bad_cursor.extend_from_slice(&cx.to_be_bytes());
        bad_cursor.extend_from_slice(&cy.to_be_bytes());
        bad_cursor.extend_from_slice(&cz.to_be_bytes());
        bad_cursor.push(0);
        put_varint(1, &mut bad_cursor);
        assert_eq!(
            decode_use_item_on(&bad_cursor),
            Err(Error::Malformed),
            "cursor ({cx}, {cy}, {cz}) should be rejected"
        );
    }

    // 5. Invalid inside_block > 1
    let mut bad_inside = Vec::new();
    put_varint(0, &mut bad_inside);
    bad_inside.extend_from_slice(&0i64.to_be_bytes());
    put_varint(0, &mut bad_inside);
    bad_inside.extend_from_slice(&0.5f32.to_be_bytes());
    bad_inside.extend_from_slice(&0.5f32.to_be_bytes());
    bad_inside.extend_from_slice(&0.5f32.to_be_bytes());
    bad_inside.push(2); // Invalid inside_block
    put_varint(1, &mut bad_inside);
    assert_eq!(decode_use_item_on(&bad_inside), Err(Error::Malformed));

    // 6. Invalid coordinate range for UseItemOn
    for (bx, by, bz) in [(0, -65, 0), (0, 320, 0), (30_000_001, 0, 0)] {
        let mut bad_coord = Vec::new();
        put_varint(0, &mut bad_coord);
        let packed = pack_block_pos(bx, by, bz);
        bad_coord.extend_from_slice(&packed.to_be_bytes());
        put_varint(0, &mut bad_coord);
        bad_coord.extend_from_slice(&0.5f32.to_be_bytes());
        bad_coord.extend_from_slice(&0.5f32.to_be_bytes());
        bad_coord.extend_from_slice(&0.5f32.to_be_bytes());
        bad_coord.push(0);
        put_varint(1, &mut bad_coord);
        assert_eq!(decode_use_item_on(&bad_coord), Err(Error::Malformed));
    }

    // 7. Surplus bytes
    let mut u_surplus = encode_use_item_on(&UseItemOn {
        hand: 0,
        pos: (0, 0, 0),
        face: 0,
        cursor_x: 0.5,
        cursor_y: 0.5,
        cursor_z: 0.5,
        inside_block: false,
        world_border_hit: false,
        sequence: 1,
    });
    u_surplus.push(0xaa);
    assert_eq!(decode_use_item_on(&u_surplus), Err(Error::Malformed));
}

#[test]
fn test_acknowledge_block_change_round_trip() {
    let test_sequences = [0, 1, 42, 127, 128, 255, 1000, 65536, 1_000_000, 1 << 28];
    for seq in test_sequences {
        // Test framed packet round-trip
        let framed = acknowledge_block_change(seq);
        let decoded = decode_acknowledge_block_change(&framed).expect("decode framed ack");
        assert_eq!(decoded, seq);

        // Test unframed packet payload round-trip
        let unframed = acknowledge_block_change_packet(seq);
        assert_eq!(unframed[0], 0x04, "packet ID must be 0x04");
        let decoded_unframed =
            decode_acknowledge_block_change_packet(&unframed).expect("decode unframed ack");
        assert_eq!(decoded_unframed, seq);
    }

    // Exact byte boundary verification for sequence = 1 << 28:
    // unframed is [0x04, 0x80, 0x80, 0x80, 0x80, 0x01] (6 bytes total).
    // decode_acknowledge_block_change must reject this unframed payload rather than
    // misinterpreting 0x04 as frame length 5 and failing on packet ID.
    let large_seq_unframed = acknowledge_block_change_packet(1 << 28);
    assert_eq!(large_seq_unframed.len(), 6);
    assert_eq!(large_seq_unframed[0], 0x04);
    assert_eq!(
        decode_acknowledge_block_change_packet(&large_seq_unframed).unwrap(),
        1 << 28
    );
    assert_eq!(
        decode_acknowledge_block_change(&large_seq_unframed),
        Err(Error::Malformed)
    );

    // Test exact byte encoding for known sequences
    assert_eq!(acknowledge_block_change_packet(0), vec![0x04, 0x00]);
    assert_eq!(acknowledge_block_change_packet(1), vec![0x04, 0x01]);
    assert_eq!(acknowledge_block_change_packet(128), vec![0x04, 0x80, 0x01]);

    // Malformed AcknowledgeBlockChange
    assert_eq!(decode_acknowledge_block_change(&[]), Err(Error::Malformed));
    assert_eq!(
        decode_acknowledge_block_change_packet(&[]),
        Err(Error::Malformed)
    );
    assert_eq!(
        decode_acknowledge_block_change_packet(&[0x05, 0x01]),
        Err(Error::Malformed)
    ); // Wrong ID (block_destruction)
    assert_eq!(
        decode_acknowledge_block_change_packet(&[0x04, 0x80]),
        Err(Error::Malformed)
    ); // Truncated varint
    assert_eq!(
        decode_acknowledge_block_change_packet(&[0x04, 0x01, 0x99]),
        Err(Error::Malformed)
    ); // Trailing byte
}

fn test_manifest() -> Arc<RegistryManifest> {
    Arc::new(RegistryManifest {
        tags: Vec::new(),
        registries: vec![
            (
                "minecraft:dimension_type".into(),
                vec!["minecraft:overworld".into()],
            ),
            (
                "minecraft:world_clock".into(),
                vec!["minecraft:overworld".into()],
            ),
            (
                "minecraft:worldgen/biome".into(),
                crate::world::Biome::ALL
                    .iter()
                    .map(|b| b.identifier().into())
                    .collect(),
            ),
        ],
    })
}

fn play_session() -> Session {
    let mut s = enter_play_session();
    s.player_name = "Alice".to_string();
    s
}

#[test]
fn chat_and_command_packets_in_play_state() {
    let mut s = play_session();

    // Unsigned chat with the mandatory acknowledgment tail, written by hand.
    let mut chat = vec![0x09, 0x05, b'H', b'e', b'l', b'l', b'o'];
    chat.extend([0; 16]); // Timestamp and salt.
    chat.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // No signature, empty tail.
    let replies = s.receive(&frame_bytes(&chat), 4096).unwrap();
    let expected: &[u8] = b"<Alice> Hello";
    let mut system_chat = vec![0x7C, 0x08, 0x00, expected.len() as u8];
    system_chat.extend(expected);
    system_chat.push(0x00);
    assert_eq!(replies, vec![frame_bytes(&system_chat)]);
    assert_eq!(s.take_broadcasts(), vec![frame_bytes(&system_chat)]);
    assert!(s.take_broadcasts().is_empty());

    // An offline client sends a string-only mutating command, and all
    // world-mutating commands (tp, gamemode, time set, give) are refused
    // with a red system chat message while the preview is read-only.
    for cmd in [
        "time set day",
        "tp 1 2 3",
        "gamemode creative",
        "give Alice diamond 1",
    ] {
        let mut body = vec![0x07];
        put_string(cmd, &mut body);
        let replies = s.receive(&frame_bytes(&body), 4096).unwrap();
        assert_eq!(replies.len(), 1);
        let reply = chat::SystemChatMessagePacket::decode_frame(&replies[0]).unwrap();
        assert_eq!(reply.content().color, Some(chat::ChatColor::Red), "{cmd}");
        assert!(reply.content().text.contains("read-only"), "{cmd}");
        assert!(s.take_broadcasts().is_empty());

        // The signed form reaches the same `crate::commands` dispatcher.
        let mut signed_mut = vec![0x08];
        put_string(cmd, &mut signed_mut);
        signed_mut.extend([0; 16]); // Timestamp and salt.
        signed_mut.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // No arguments, empty tail.
        let replies = s.receive(&frame_bytes(&signed_mut), 4096).unwrap();
        assert_eq!(replies.len(), 1);
        let reply = chat::SystemChatMessagePacket::decode_frame(&replies[0]).unwrap();
        assert_eq!(reply.content().color, Some(chat::ChatColor::Red), "{cmd}");
        assert!(reply.content().text.contains("read-only"), "{cmd}");
    }

    // Read-only commands (/help and /seed) succeed through `crate::commands`.
    let mut signed = vec![0x08, 0x04, b'h', b'e', b'l', b'p'];
    signed.extend([0; 16]);
    signed.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // No arguments, empty tail.
    let replies = s.receive(&frame_bytes(&signed), 4096).unwrap();
    let reply = chat::SystemChatMessagePacket::decode_frame(&replies[0]).unwrap();
    assert!(reply.content().text.contains("/help"));

    let seed_cmd = [0x07, 0x04, b's', b'e', b'e', b'd'];
    let replies = s.receive(&frame_bytes(&seed_cmd), 4096).unwrap();
    let reply = chat::SystemChatMessagePacket::decode_frame(&replies[0]).unwrap();
    assert_eq!(reply.content().text, "Seed: [0]");

    // Acknowledgments and session announcements have no reply.
    assert!(
        s.receive(&frame_bytes(&[0x06, 0x00]), 4096)
            .unwrap()
            .is_empty()
    );
    assert!(
        s.receive(&frame_bytes(&[0x0A, 0x00]), 4096)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        s.receive(&frame_bytes(&[0x06, 0x00, 0x00]), 4096),
        Err(Error::Malformed)
    );
}

fn frame_bytes(packet: &[u8]) -> Vec<u8> {
    let mut out = vec![packet.len() as u8];
    assert!(packet.len() < 0x80);
    out.extend(packet);
    out
}

#[test]
fn queued_broadcasts_are_bounded() {
    let mut s = play_session();
    let mut chat = vec![0x09, 0x01, b'x'];
    chat.extend([0; 22]);
    let chat = frame_bytes(&chat);
    for _ in 0..MAX_QUEUED_BROADCASTS {
        s.receive(&chat, 1 << 20).unwrap();
    }
    assert_eq!(s.receive(&chat, 1 << 20), Err(Error::Oversized));
    assert_eq!(s.take_broadcasts().len(), MAX_QUEUED_BROADCASTS);
}

fn enter_play_session() -> Session {
    let mut current = handshake(777);
    *current.last_mut().unwrap() = 2;
    let mut s = Session::new(true).with_registry_manifest(test_manifest());
    s.receive(&current, 4096).unwrap();
    assert_eq!(s.state(), State::Login);

    let mut login = vec![4];
    login.extend_from_slice(b"Test");
    login.extend_from_slice(&[7; 16]);
    s.receive(&frame(0, &login), 4096).unwrap();
    assert_eq!(s.state(), State::LoginAcknowledgement);

    s.receive(&frame(3, &[]), 4096).unwrap();
    assert_eq!(s.state(), State::Configuration);

    let mut known = vec![1];
    for text in ["minecraft", "core", VERSION] {
        put_string(text, &mut known);
    }
    s.receive(&frame(7, &known), 4096).unwrap();
    assert_eq!(s.state(), State::ConfigurationData);

    s.receive(&frame(3, &[]), 4096).unwrap();
    assert_eq!(s.state(), State::Play);

    s
}

fn teleport_confirm(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
    let mut body = vec![1u8];
    body.extend(x.to_be_bytes());
    body.extend(y.to_be_bytes());
    body.extend(z.to_be_bytes());
    body.extend(yaw.to_be_bytes());
    body.extend(pitch.to_be_bytes());
    body
}

fn player_position(x: f64, y: f64, z: f64, on_ground: u8) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(x.to_be_bytes());
    body.extend(y.to_be_bytes());
    body.extend(z.to_be_bytes());
    body.push(on_ground);
    body
}

fn player_pos_rot(x: f64, y: f64, z: f64, yaw: f32, pitch: f32, on_ground: u8) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(x.to_be_bytes());
    body.extend(y.to_be_bytes());
    body.extend(z.to_be_bytes());
    body.extend(yaw.to_be_bytes());
    body.extend(pitch.to_be_bytes());
    body.push(on_ground);
    body
}

fn player_rotation(yaw: f32, pitch: f32, on_ground: u8) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(yaw.to_be_bytes());
    body.extend(pitch.to_be_bytes());
    body.push(on_ground);
    body
}

#[test]
fn play_state_packet_validation_guards() {
    // 1. NaN coordinate guards
    assert_eq!(
        enter_play_session().receive(
            &frame(0, &teleport_confirm(f64::NAN, 64.0, 0.0, 0.0, 0.0)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(0, &teleport_confirm(0.0, f64::NAN, 0.0, 0.0, 0.0)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(0, &teleport_confirm(0.0, 64.0, f64::NAN, 0.0, 0.0)),
            4096
        ),
        Err(Error::Malformed)
    );

    assert_eq!(
        enter_play_session().receive(&frame(30, &player_position(f64::NAN, 64.0, 0.0, 1)), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(30, &player_position(0.0, f64::NAN, 0.0, 1)), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(30, &player_position(0.0, 64.0, f64::NAN, 1)), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(31, &player_pos_rot(f64::NAN, 64.0, 0.0, 0.0, 0.0, 1)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(31, &player_pos_rot(0.0, f64::NAN, 0.0, 0.0, 0.0, 1)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(31, &player_pos_rot(0.0, 64.0, f64::NAN, 0.0, 0.0, 1)),
            4096
        ),
        Err(Error::Malformed)
    );

    // 2. Non-finite pitch guards
    assert_eq!(
        enter_play_session().receive(
            &frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, f32::NAN)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, f32::INFINITY)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, f32::NEG_INFINITY)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(31, &player_pos_rot(0.0, 64.0, 0.0, 0.0, f32::NAN, 1)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(31, &player_pos_rot(0.0, 64.0, 0.0, 0.0, f32::INFINITY, 1)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(32, &player_rotation(0.0, f32::NAN, 1)), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(32, &player_rotation(0.0, f32::INFINITY, 1)), 4096),
        Err(Error::Malformed)
    );

    // 3. Pitch > 90.0 (and < -90.0) guards
    assert_eq!(
        enter_play_session().receive(
            &frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, 90.001)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, -90.001)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(31, &player_pos_rot(0.0, 64.0, 0.0, 0.0, 91.0, 1)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(31, &player_pos_rot(0.0, 64.0, 0.0, 0.0, -91.0, 1)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(32, &player_rotation(0.0, 90.5, 1)), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(32, &player_rotation(0.0, -90.5, 1)), 4096),
        Err(Error::Malformed)
    );

    // 4. On-ground and horizontal-collision flag bit validation (0..=3 valid)
    for flag in [0, 1, 2, 3] {
        let mut s = enter_play_session();
        s.receive(&frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, 0.0)), 4096)
            .unwrap();
        assert!(
            s.receive(&frame(30, &player_position(0.0, 64.0, 0.0, flag)), 4096)
                .is_ok()
        );
        assert!(
            s.receive(
                &frame(31, &player_pos_rot(0.0, 64.0, 0.0, 0.0, 0.0, flag)),
                4096
            )
            .is_ok()
        );
        assert!(
            s.receive(&frame(32, &player_rotation(0.0, 0.0, flag)), 4096)
                .is_ok()
        );
        assert!(s.receive(&frame(33, &[flag]), 4096).is_ok());
    }

    // Invalid flags (> 3, unknown bits set) are rejected as malformed
    for invalid_flag in [4, 5, 8, 255] {
        assert_eq!(
            enter_play_session().receive(
                &frame(30, &player_position(0.0, 64.0, 0.0, invalid_flag)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(31, &player_pos_rot(0.0, 64.0, 0.0, 0.0, 0.0, invalid_flag)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session()
                .receive(&frame(32, &player_rotation(0.0, 0.0, invalid_flag)), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(33, &[invalid_flag]), 4096),
            Err(Error::Malformed)
        );
    }

    // player_abilities carries only the flying flag; player_input uses seven flags.
    for flags in [0x01, 0x04, 0x80, 0xff] {
        assert_eq!(
            enter_play_session().receive(&frame(40, &[flags]), 4096),
            Err(Error::Malformed)
        );
    }
    assert_eq!(
        enter_play_session().receive(&frame(43, &[0x80]), 4096),
        Err(Error::Malformed)
    );

    // 5. Trailing payload byte guards
    let mut p0 = teleport_confirm(0.0, 64.0, 0.0, 0.0, 0.0);
    p0.push(0);
    assert_eq!(
        enter_play_session().receive(&frame(0, &p0), 4096),
        Err(Error::Malformed)
    );

    let mut p11 = 1.0f32.to_be_bytes().to_vec();
    p11.push(0);
    assert_eq!(
        enter_play_session().receive(&frame(11, &p11), 4096),
        Err(Error::Malformed)
    );

    assert_eq!(
        enter_play_session().receive(&frame(13, &[0]), 4096),
        Err(Error::Malformed)
    );

    let mut p28 = 1u64.to_be_bytes().to_vec();
    p28.push(0);
    assert_eq!(
        enter_play_session().receive(&frame(28, &p28), 4096),
        Err(Error::Malformed)
    );

    let mut p30 = player_position(0.0, 64.0, 0.0, 1);
    p30.push(0);
    assert_eq!(
        enter_play_session().receive(&frame(30, &p30), 4096),
        Err(Error::Malformed)
    );

    let mut p31 = player_pos_rot(0.0, 64.0, 0.0, 0.0, 0.0, 1);
    p31.push(0);
    assert_eq!(
        enter_play_session().receive(&frame(31, &p31), 4096),
        Err(Error::Malformed)
    );

    let mut p32 = player_rotation(0.0, 0.0, 1);
    p32.push(0);
    assert_eq!(
        enter_play_session().receive(&frame(32, &p32), 4096),
        Err(Error::Malformed)
    );

    assert_eq!(
        enter_play_session().receive(&frame(33, &[1, 0]), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(40, &[0, 0]), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(43, &[1, 0]), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(44, &[0]), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(46, &[0]), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(54, &[0, 0, 0]), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(63, &[0, 0]), 4096),
        Err(Error::Malformed)
    );

    // 6. Extreme coordinate guards: vanilla only rejects positions beyond
    // +-30,000,000 horizontally and +-20,000,000 vertically.
    let teleport = |x, y, z| {
        enter_play_session().receive(&frame(0, &teleport_confirm(x, y, z, 0.0, 0.0)), 4096)
    };
    for (x, y, z) in [
        (30_000_000.5, 64.0, 0.0),
        (-30_000_000.5, 64.0, 0.0),
        (0.0, 20_000_000.5, 0.0),
        (0.0, -20_000_000.5, 0.0),
        (0.0, 64.0, 30_000_000.5),
    ] {
        assert_eq!(teleport(x, y, z), Err(Error::Malformed), "{x} {y} {z}");
    }
    assert!(teleport(5_000_000.0, 1000.0, -5_000_000.0).is_ok());
    assert_eq!(
        enter_play_session().receive(
            &frame(30, &player_position(30_000_001.0, 64.0, 0.0, 1)),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(
            &frame(31, &player_pos_rot(0.0, 64.0, -30_000_001.0, 0.0, 0.0, 1)),
            4096
        ),
        Err(Error::Malformed)
    );

    // 7. Invalid hotbar slot guards
    assert_eq!(
        enter_play_session().receive(&frame(54, &9i16.to_be_bytes()), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(54, &(-1i16).to_be_bytes()), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(54, &100i16.to_be_bytes()), 4096),
        Err(Error::Malformed)
    );
    assert_eq!(
        enter_play_session().receive(&frame(54, &(-10i16).to_be_bytes()), 4096),
        Err(Error::Malformed)
    );

    // Valid hotbar slots 0 and 8 succeed
    assert!(
        enter_play_session()
            .receive(&frame(54, &0i16.to_be_bytes()), 4096)
            .is_ok()
    );
    assert!(
        enter_play_session()
            .receive(&frame(54, &8i16.to_be_bytes()), 4096)
            .is_ok()
    );

    // 8. Batch rate guards
    let mut s_batch = enter_play_session();
    s_batch.preview.as_mut().unwrap().awaiting_batch = true;
    assert_eq!(
        s_batch.receive(&frame(11, &0.0f32.to_be_bytes()), 4096),
        Err(Error::Malformed)
    );
    let mut s_batch = enter_play_session();
    s_batch.preview.as_mut().unwrap().awaiting_batch = true;
    assert_eq!(
        s_batch.receive(&frame(11, &(-1.0f32).to_be_bytes()), 4096),
        Err(Error::Malformed)
    );
    // A fast client may report a high desired rate; it is not malformed.
    let mut s_batch = enter_play_session();
    s_batch.preview.as_mut().unwrap().awaiting_batch = true;
    assert!(
        s_batch
            .receive(&frame(11, &1000.0f32.to_be_bytes()), 4096)
            .is_ok()
    );
    let mut s_batch = enter_play_session();
    s_batch.preview.as_mut().unwrap().awaiting_batch = true;
    assert_eq!(
        s_batch.receive(&frame(11, &f32::NAN.to_be_bytes()), 4096),
        Err(Error::Malformed)
    );
    let mut s_batch = enter_play_session();
    s_batch.preview.as_mut().unwrap().awaiting_batch = true;
    assert!(
        s_batch
            .receive(&frame(11, &50.0f32.to_be_bytes()), 4096)
            .is_ok()
    );

    // 9. Keep alive guards
    let mut s_keepalive = enter_play_session();
    s_keepalive.preview.as_mut().unwrap().pending_keepalive = Some(12345);
    assert_eq!(
        s_keepalive.receive(&frame(28, &99999u64.to_be_bytes()), 4096),
        Err(Error::Malformed)
    );
    assert!(
        s_keepalive
            .receive(&frame(28, &12345u64.to_be_bytes()), 4096)
            .is_ok()
    );

    // 10. Unsupported unknown packet in Play state returns WrongState
    assert_eq!(
        enter_play_session().receive(&frame(99, &[]), 4096),
        Err(Error::WrongState)
    );

    // 11. Valid play packets flow smoothly
    let mut s = enter_play_session();
    s.receive(&frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, 0.0)), 4096)
        .unwrap();
    assert!(s.preview.as_ref().unwrap().teleport_acknowledged);

    assert!(
        s.receive(&frame(30, &player_position(10.0, 64.0, 10.0, 1)), 4096)
            .is_ok()
    );
    assert!(
        s.receive(
            &frame(31, &player_pos_rot(15.0, 64.0, 15.0, 90.0, 45.0, 0)),
            4096
        )
        .is_ok()
    );
    assert!(
        s.receive(&frame(32, &player_rotation(180.0, -45.0, 1)), 4096)
            .is_ok()
    );
    assert!(s.receive(&frame(33, &[1]), 4096).is_ok());
    // Start and stop flying (abilities flag 0x02).
    assert!(s.receive(&frame(40, &[0x02]), 4096).is_ok());
    assert!(s.receive(&frame(40, &[0]), 4096).is_ok());
    // Forward, backward, left, right, jump, shift, and sprint together.
    assert!(s.receive(&frame(43, &[0x7f]), 4096).is_ok());
    assert!(s.receive(&frame(43, &[0x02]), 4096).is_ok());
    assert!(s.receive(&frame(44, &[]), 4096).is_ok());
    assert!(s.preview.as_ref().unwrap().client_loaded);
    assert!(s.receive(&frame(46, &[]), 4096).is_ok());
    assert!(s.receive(&frame(54, &4i16.to_be_bytes()), 4096).is_ok());
    assert!(s.receive(&frame(63, &[0]), 4096).is_ok());
}

#[test]
fn test_session_play_state_action_packets() {
    let mut s = enter_play_session();
    // 1. Send valid PlayerAction (ID 0x29 / 41) -> should emit AcknowledgeBlockChange
    let action = PlayerAction {
        status: PlayerActionStatus::StartedDigging,
        pos: (10, 64, -20),
        face: 1,
        sequence: 12,
    };
    let responses = s
        .receive(
            &frame(PLAYER_ACTION_ID, &encode_player_action(&action)),
            4096,
        )
        .unwrap();
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0], acknowledge_block_change(12));

    // 2. Send valid UseItemOn (ID 0x42 / 66) -> should emit AcknowledgeBlockChange
    let use_item = UseItemOn {
        hand: 0,
        pos: (10, 64, -20),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 0.5,
        cursor_z: 0.5,
        inside_block: false,
        world_border_hit: false,
        sequence: 15,
    };
    let responses = s
        .receive(&frame(USE_ITEM_ON_ID, &encode_use_item_on(&use_item)), 4096)
        .unwrap();
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0], acknowledge_block_change(15));

    // 3. Send malformed PlayerAction in Play state -> Error::Malformed
    let mut malformed_action = encode_player_action(&action);
    malformed_action.push(0xff); // Trailing byte
    assert_eq!(
        s.receive(&frame(PLAYER_ACTION_ID, &malformed_action), 4096),
        Err(Error::Malformed)
    );

    // 4. Send malformed UseItemOn in Play state -> Error::Malformed
    let mut bad_cursor_use = Vec::new();
    put_varint(0, &mut bad_cursor_use);
    bad_cursor_use.extend_from_slice(&pack_block_pos(0, 0, 0).to_be_bytes());
    put_varint(0, &mut bad_cursor_use);
    bad_cursor_use.extend_from_slice(&f32::NAN.to_be_bytes()); // NaN cursor float
    bad_cursor_use.extend_from_slice(&0.5f32.to_be_bytes());
    bad_cursor_use.extend_from_slice(&0.5f32.to_be_bytes());
    bad_cursor_use.push(0);
    put_varint(1, &mut bad_cursor_use);
    assert_eq!(
        s.receive(&frame(USE_ITEM_ON_ID, &bad_cursor_use), 4096),
        Err(Error::Malformed)
    );
}

#[test]
fn truncated_player_action_is_malformed() {
    let valid = encode_player_action(&PlayerAction {
        status: PlayerActionStatus::StartedDigging,
        pos: (10, 64, -20),
        face: 1,
        sequence: 7,
    });
    for len in 0..valid.len() {
        assert_eq!(decode_player_action(&valid[..len]), Err(Error::Malformed));
        assert_eq!(
            enter_play_session().receive(
                &frame(play::serverbound::PLAYER_ACTION, &valid[..len]),
                4096
            ),
            Err(Error::Malformed)
        );
    }
}

#[test]
fn truncated_use_item_on_is_malformed() {
    let valid = encode_use_item_on(&UseItemOn {
        hand: 0,
        pos: (10, 64, -20),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 0.5,
        cursor_z: 0.5,
        inside_block: false,
        world_border_hit: false,
        sequence: 7,
    });
    for len in 0..valid.len() {
        assert_eq!(decode_use_item_on(&valid[..len]), Err(Error::Malformed));
        assert_eq!(
            enter_play_session()
                .receive(&frame(play::serverbound::USE_ITEM_ON, &valid[..len]), 4096),
            Err(Error::Malformed)
        );
    }
}

#[test]
fn truncated_accept_teleportation_is_malformed() {
    let valid = teleport_confirm(0.0, 64.0, 0.0, 0.0, 0.0);
    for len in 0..valid.len() {
        assert_eq!(
            enter_play_session().receive(
                &frame(play::serverbound::ACCEPT_TELEPORTATION, &valid[..len]),
                4096
            ),
            Err(Error::Malformed)
        );
    }
}

#[test]
fn truncated_chunk_batch_received_is_malformed() {
    let valid = 20.0f32.to_be_bytes();
    for len in 0..valid.len() {
        let mut s = enter_play_session();
        s.preview.as_mut().unwrap().awaiting_batch = true;
        assert_eq!(
            s.receive(
                &frame(play::serverbound::CHUNK_BATCH_RECEIVED, &valid[..len]),
                4096
            ),
            Err(Error::Malformed)
        );
    }
}

#[test]
fn truncated_keep_alive_is_malformed() {
    let valid = 12345u64.to_be_bytes();
    for len in 0..valid.len() {
        let mut s = enter_play_session();
        s.preview.as_mut().unwrap().pending_keepalive = Some(12345);
        assert_eq!(
            s.receive(&frame(play::serverbound::KEEP_ALIVE, &valid[..len]), 4096),
            Err(Error::Malformed)
        );
    }
}

#[test]
fn truncated_move_player_pos_is_malformed() {
    let valid = player_position(0.0, 64.0, 0.0, 1);
    for len in 0..valid.len() {
        let mut s = enter_play_session();
        s.preview.as_mut().unwrap().teleport_acknowledged = true;
        assert_eq!(
            s.receive(
                &frame(play::serverbound::MOVE_PLAYER_POS, &valid[..len]),
                4096
            ),
            Err(Error::Malformed)
        );
    }
}

#[test]
fn truncated_move_player_pos_rot_is_malformed() {
    let valid = player_pos_rot(0.0, 64.0, 0.0, 0.0, 0.0, 1);
    for len in 0..valid.len() {
        let mut s = enter_play_session();
        s.preview.as_mut().unwrap().teleport_acknowledged = true;
        assert_eq!(
            s.receive(
                &frame(play::serverbound::MOVE_PLAYER_POS_ROT, &valid[..len]),
                4096
            ),
            Err(Error::Malformed)
        );
    }
}

#[test]
fn truncated_move_player_rot_is_malformed() {
    let valid = player_rotation(0.0, 0.0, 1);
    for len in 0..valid.len() {
        assert_eq!(
            enter_play_session().receive(
                &frame(play::serverbound::MOVE_PLAYER_ROT, &valid[..len]),
                4096
            ),
            Err(Error::Malformed)
        );
    }
}

#[test]
fn truncated_set_carried_item_is_malformed() {
    let valid = 4i16.to_be_bytes();
    for len in 0..valid.len() {
        assert_eq!(
            enter_play_session().receive(
                &frame(play::serverbound::SET_CARRIED_ITEM, &valid[..len]),
                4096
            ),
            Err(Error::Malformed)
        );
    }
}

fn test_protocol_ids() -> Arc<ProtocolIds> {
    const SYNTHETIC_REGISTRIES: &str = r#"{
        "minecraft:item": {
            "entries": {
                "minecraft:air": { "protocol_id": 0 },
                "minecraft:stone": { "protocol_id": 1 },
                "minecraft:cobblestone": { "protocol_id": 14 },
                "minecraft:diamond_sword": { "protocol_id": 851 }
            }
        },
        "minecraft:data_component_type": {
            "entries": {
                "minecraft:damage": { "protocol_id": 3 },
                "minecraft:custom_name": { "protocol_id": 6 },
                "minecraft:enchantments": { "protocol_id": 13 }
            }
        },
        "minecraft:potion": {
            "entries": {
                "minecraft:water": { "protocol_id": 0 }
            }
        },
        "minecraft:mob_effect": {
            "entries": {
                "minecraft:speed": { "protocol_id": 0 }
            }
        }
    }"#;
    Arc::new(
        ProtocolIds::from_json_str(
            std::path::Path::new("reports/registries.json"),
            SYNTHETIC_REGISTRIES,
        )
        .expect("synthetic protocol ids"),
    )
}

fn test_manifest_with_enchantments() -> Arc<RegistryManifest> {
    let mut manifest = (*test_manifest()).clone();
    manifest.registries.push((
        "minecraft:enchantment".into(),
        vec!["minecraft:sharpness".into(), "minecraft:unbreaking".into()],
    ));
    Arc::new(manifest)
}

fn enter_play_session_with_protocol_ids() -> (Session, Arc<ProtocolIds>, DynamicRegistryIds) {
    let ids = test_protocol_ids();
    let manifest = test_manifest_with_enchantments();
    let dynamic_ids = DynamicRegistryIds::from_manifest(&manifest);

    let mut current = handshake(777);
    *current.last_mut().unwrap() = 2;
    let mut s = Session::new(true)
        .with_registry_manifest(manifest)
        .with_protocol_ids(Arc::clone(&ids));
    s.receive(&current, 4096).unwrap();
    let mut login = vec![4];
    login.extend_from_slice(b"Test");
    login.extend_from_slice(&[7; 16]);
    s.receive(&frame(0, &login), 4096).unwrap();
    s.receive(&frame(3, &[]), 4096).unwrap();
    let mut known = vec![1];
    for text in ["minecraft", "core", VERSION] {
        put_string(text, &mut known);
    }
    s.receive(&frame(7, &known), 4096).unwrap();
    s.receive(&frame(3, &[]), 4096).unwrap();
    assert_eq!(s.state(), State::Play);
    (s, ids, dynamic_ids)
}

fn creative_slot_frame(
    window_slot: i16,
    stack: &Option<crate::item::ItemStack>,
    ids: &ProtocolIds,
    dynamic_ids: &DynamicRegistryIds,
) -> Vec<u8> {
    let mut body = window_slot.to_be_bytes().to_vec();
    slot::encode_slot(stack, ids, dynamic_ids, &mut body).expect("encode creative slot");
    frame(play::serverbound::SET_CREATIVE_MODE_SLOT, &body)
}

#[test]
fn creative_slot_stores_and_clears_stacks_and_tracks_carried_item() {
    use crate::item::{DEFAULT_MAX_STACK, ItemComponents, ItemStack};

    let (mut s, ids, dynamic_ids) = enter_play_session_with_protocol_ids();
    assert_eq!(s.inventory().held_slot(), 0);
    assert!(s.inventory().held_item().is_none());

    // Window slot 36 is hotbar slot 0 (`Java Edition protocol/Inventory` rev 3201379).
    let stone = ItemStack::try_new("minecraft:stone", 64, DEFAULT_MAX_STACK).unwrap();
    let replies = s
        .receive(
            &creative_slot_frame(36, &Some(stone.clone()), &ids, &dynamic_ids),
            4096,
        )
        .unwrap();
    assert!(replies.is_empty());
    assert_eq!(s.inventory().get_slot(0), Some(&stone));
    assert_eq!(s.inventory().get_window_slot(36), Some(&stone));
    assert_eq!(s.inventory().held_item(), Some(&stone));

    // Window slot 44 is hotbar slot 8; store an enchanted named sword with damage.
    let sword = ItemStack::try_new("minecraft:diamond_sword", 1, DEFAULT_MAX_STACK)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_damage(12)
                .with_custom_name("Excalibur")
                .with_enchantment("minecraft:sharpness", 5)
                .unwrap(),
        )
        .unwrap();
    let replies = s
        .receive(
            &creative_slot_frame(44, &Some(sword.clone()), &ids, &dynamic_ids),
            4096,
        )
        .unwrap();
    assert!(replies.is_empty());
    assert_eq!(s.inventory().get_slot(8), Some(&sword));
    assert_eq!(s.inventory().get_window_slot(44), Some(&sword));

    // Window slot 9 is the first main inventory slot (`9..=35`).
    let cobble = ItemStack::try_new("minecraft:cobblestone", 32, DEFAULT_MAX_STACK).unwrap();
    let replies = s
        .receive(
            &creative_slot_frame(9, &Some(cobble.clone()), &ids, &dynamic_ids),
            4096,
        )
        .unwrap();
    assert!(replies.is_empty());
    assert_eq!(s.inventory().get_slot(9), Some(&cobble));
    assert_eq!(s.inventory().get_window_slot(9), Some(&cobble));

    // `SET_CARRIED_ITEM` selects hotbar slot 8 (window slot 44).
    let replies = s
        .receive(
            &frame(play::serverbound::SET_CARRIED_ITEM, &8i16.to_be_bytes()),
            4096,
        )
        .unwrap();
    assert!(replies.is_empty());
    assert_eq!(s.inventory().held_slot(), 8);
    assert_eq!(s.inventory().held_item(), Some(&sword));

    // Clearing hotbar slot 0 (window slot 36) and hotbar slot 8 (window slot 44) removes them.
    assert!(
        s.receive(&creative_slot_frame(36, &None, &ids, &dynamic_ids), 4096)
            .unwrap()
            .is_empty()
    );
    assert!(s.inventory().get_slot(0).is_none());
    assert!(
        s.receive(&creative_slot_frame(44, &None, &ids, &dynamic_ids), 4096)
            .unwrap()
            .is_empty()
    );
    assert!(s.inventory().get_slot(8).is_none());
    assert!(s.inventory().held_item().is_none());
    assert_eq!(s.inventory().get_slot(9), Some(&cobble));
}

#[test]
fn creative_slot_accepts_drop_crafting_armor_and_offhand_slots_without_storing() {
    use crate::item::{DEFAULT_MAX_STACK, ItemStack};

    let (mut s, ids, dynamic_ids) = enter_play_session_with_protocol_ids();
    let stone = ItemStack::try_new("minecraft:stone", 1, DEFAULT_MAX_STACK).unwrap();
    // -1 drops the item; 0..=8 are crafting output/input and armor; 45 is the offhand.
    for slot in [-1, 0, 1, 4, 5, 8, 45] {
        let packet = creative_slot_frame(slot, &Some(stone.clone()), &ids, &dynamic_ids);
        assert!(
            s.receive(&packet, 4096).is_ok(),
            "window slot {slot} is valid creative input"
        );
    }
    assert_eq!(s.state(), State::Play);
    assert_eq!(s.inventory().total_item_count(), 0);
}

#[test]
fn creative_slot_with_unmodelled_component_keeps_the_session_and_untracks_the_slot() {
    use crate::item::{DEFAULT_MAX_STACK, ItemStack};

    let (mut s, ids, dynamic_ids) = enter_play_session_with_protocol_ids();
    let stone = ItemStack::try_new("minecraft:stone", 1, DEFAULT_MAX_STACK).unwrap();
    s.receive(
        &creative_slot_frame(36, &Some(stone), &ids, &dynamic_ids),
        4096,
    )
    .unwrap();
    assert_eq!(s.inventory().total_item_count(), 1);

    // Component type 99 is not modelled (for example fireworks or banner patterns).
    let mut body = 36i16.to_be_bytes().to_vec();
    put_varint(1, &mut body); // count
    put_varint(1, &mut body); // minecraft:stone
    put_varint(1, &mut body); // one component to add
    put_varint(0, &mut body); // none to remove
    put_varint(99, &mut body); // unmodelled component type
    body.push(0);
    assert!(
        s.receive(
            &frame(play::serverbound::SET_CREATIVE_MODE_SLOT, &body),
            4096
        )
        .is_ok()
    );
    assert_eq!(s.state(), State::Play);
    assert!(s.inventory().get_window_slot(36).is_none());
}

#[test]
fn creative_slot_rejects_out_of_range_slot_and_unknown_item_id() {
    use crate::item::{DEFAULT_MAX_STACK, ItemStack};

    let (mut s, ids, dynamic_ids) = enter_play_session_with_protocol_ids();
    let stone = ItemStack::try_new("minecraft:stone", 1, DEFAULT_MAX_STACK).unwrap();

    // Only numbers outside the player inventory window (`0..=45`) and the creative
    // drop slot (`-1`) are rejected.
    for bad_slot in [i16::MIN, -999, -2, 46, 100, i16::MAX] {
        let packet = creative_slot_frame(bad_slot, &Some(stone.clone()), &ids, &dynamic_ids);
        assert_eq!(
            s.receive(&packet, 4096),
            Err(Error::Malformed),
            "window slot {bad_slot} should be rejected"
        );
    }
    assert_eq!(s.inventory().total_item_count(), 0);

    // Unknown item protocol ID (999) in a valid window slot (36) is rejected as Malformed.
    let mut unknown_item = 36i16.to_be_bytes().to_vec();
    put_varint(1, &mut unknown_item); // count = 1
    put_varint(999, &mut unknown_item); // unknown item id
    put_varint(0, &mut unknown_item); // 0 components to add
    put_varint(0, &mut unknown_item); // 0 components to remove
    assert_eq!(
        s.receive(
            &frame(play::serverbound::SET_CREATIVE_MODE_SLOT, &unknown_item),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(s.inventory().total_item_count(), 0);

    // Truncated creative slot body and trailing bytes after the slot are rejected.
    assert_eq!(
        s.receive(
            &frame(play::serverbound::SET_CREATIVE_MODE_SLOT, &[0x00]),
            4096
        ),
        Err(Error::Malformed)
    );
    assert_eq!(
        s.receive(
            &frame(
                play::serverbound::SET_CREATIVE_MODE_SLOT,
                &36i16.to_be_bytes()
            ),
            4096
        ),
        Err(Error::Malformed)
    );
    let mut trailing = 36i16.to_be_bytes().to_vec();
    trailing.push(0); // Empty slot
    trailing.push(0); // Trailing byte
    assert_eq!(
        s.receive(
            &frame(play::serverbound::SET_CREATIVE_MODE_SLOT, &trailing),
            4096
        ),
        Err(Error::Malformed)
    );
}

#[test]
fn creative_slot_without_protocol_ids_stays_bounded_and_ignored() {
    let mut s = enter_play_session();
    assert_eq!(
        s.receive(&frame(play::serverbound::SET_CREATIVE_MODE_SLOT, &[]), 4096),
        Err(Error::Malformed)
    );
    for payload in [&[0u8][..], &[0, 36, 1, 99, 0, 0][..], &[0xff; 32][..]] {
        assert!(
            s.receive(
                &frame(play::serverbound::SET_CREATIVE_MODE_SLOT, payload),
                4096
            )
            .unwrap()
            .is_empty()
        );
    }
    assert_eq!(s.inventory().total_item_count(), 0);
}

#[test]
fn compressed_session_completes_login_configuration_and_play_round_trip() {
    let mut manifest_val = (*test_manifest()).clone();
    // Add enough registry entries so `REGISTRY_DATA` exceeds threshold=256 and compresses.
    manifest_val.registries.push((
        "minecraft:painting_variant".into(),
        (0..20)
            .map(|i| format!("minecraft:variant_{i:02}"))
            .collect(),
    ));
    let manifest = Arc::new(manifest_val);

    let mut hs = handshake(777);
    *hs.last_mut().unwrap() = intent::LOGIN as u8;
    let mut login_body = vec![4];
    login_body.extend_from_slice(b"Test");
    login_body.extend_from_slice(&[7; 16]);
    let hello_pkt = frame(login::serverbound::HELLO, &login_body);

    let mut s = Session::new(true)
        .with_registry_manifest(Arc::clone(&manifest))
        .with_compression_threshold(Some(256));
    assert!(s.receive(&hs, 4096).unwrap().is_empty());
    let login_replies = s.receive(&hello_pkt, 4096).unwrap();
    assert_eq!(login_replies.len(), 2);
    // Set Compression is sent uncompressed before Login Success.
    assert_eq!(login_replies[0], [0x03, 0x03, 0x80, 0x02]);
    // Login Success immediately follows in the compressed format.
    let (login_id, login_success_body, rest) =
        read_compressed_frame(&login_replies[1], 256, 4096).unwrap();
    assert!(rest.is_empty());
    assert_eq!(login_id, login::clientbound::LOGIN_FINISHED);
    assert!(login_success_body.starts_with(&[7; 16]));

    let mut ack_buf = Vec::new();
    frame_compressed(
        login::serverbound::LOGIN_ACKNOWLEDGED,
        &[],
        256,
        &mut ack_buf,
    )
    .unwrap();
    let cfg_replies = s.receive(&ack_buf, 4096).unwrap();
    assert_eq!(cfg_replies.len(), 1);
    let (kp_id, kp_body, rest) = read_compressed_frame(&cfg_replies[0], 256, 4096).unwrap();
    assert!(rest.is_empty());
    assert_eq!(kp_id, configuration::clientbound::SELECT_KNOWN_PACKS);
    assert!(String::from_utf8_lossy(&kp_body).contains("minecraft"));

    let mut known_body = vec![1];
    for text in ["minecraft", "core", VERSION] {
        put_string(text, &mut known_body);
    }
    let mut known_buf = Vec::new();
    frame_compressed(
        configuration::serverbound::SELECT_KNOWN_PACKS,
        &known_body,
        256,
        &mut known_buf,
    )
    .unwrap();
    let cfg_data_replies = s.receive(&known_buf, 4096).unwrap();
    assert!(cfg_data_replies.len() >= 3);
    let mut saw_compressed_cfg = false;
    for reply in &cfg_data_replies {
        let (_, prefix) = read_varint(reply).unwrap().unwrap();
        let (dl, _) = read_varint(&reply[prefix..]).unwrap().unwrap();
        if dl > 0 {
            saw_compressed_cfg = true;
        }
        let (_, _, rest) = read_compressed_frame(reply, 256, 65_536).unwrap();
        assert!(rest.is_empty());
    }
    assert!(saw_compressed_cfg);

    let mut finish_buf = Vec::new();
    frame_compressed(
        configuration::serverbound::FINISH_CONFIGURATION,
        &[],
        256,
        &mut finish_buf,
    )
    .unwrap();
    let play_initial = s.receive(&finish_buf, 4096).unwrap();
    assert_eq!(s.state(), State::Play);
    assert_eq!(play_initial.len(), 6);
    for reply in &play_initial {
        let (_, _, rest) = read_compressed_frame(reply, 256, 4096).unwrap();
        assert!(rest.is_empty());
    }

    // Compressed teleport confirmation and play chunk delivery.
    let mut tp_buf = Vec::new();
    frame_compressed(
        play::serverbound::ACCEPT_TELEPORTATION,
        &teleport_confirm(0.0, 64.0, 0.0, 0.0, 0.0),
        256,
        &mut tp_buf,
    )
    .unwrap();
    assert!(s.receive(&tp_buf, 4096).unwrap().is_empty());

    // Compare compressed chunk batch against an uncompressed session's chunk batch.
    let mut uncomp_s = Session::new(true).with_registry_manifest(manifest);
    uncomp_s.receive(&hs, 4096).unwrap();
    uncomp_s.receive(&hello_pkt, 4096).unwrap();
    uncomp_s
        .receive(&frame(login::serverbound::LOGIN_ACKNOWLEDGED, &[]), 4096)
        .unwrap();
    uncomp_s
        .receive(
            &frame(configuration::serverbound::SELECT_KNOWN_PACKS, &known_body),
            4096,
        )
        .unwrap();
    uncomp_s
        .receive(
            &frame(configuration::serverbound::FINISH_CONFIGURATION, &[]),
            4096,
        )
        .unwrap();
    uncomp_s
        .receive(
            &frame(
                play::serverbound::ACCEPT_TELEPORTATION,
                &teleport_confirm(0.0, 64.0, 0.0, 0.0, 0.0),
            ),
            4096,
        )
        .unwrap();

    let mut comp_chunks = Vec::new();
    while let Some((comp_batch, _, _)) = s.preview.as_mut().unwrap().next_chunk() {
        let mut comp_cursor = comp_batch.as_slice();
        while !comp_cursor.is_empty() {
            let (pkt_len, prefix) = read_varint(comp_cursor).unwrap().unwrap();
            let (data_len, _) = read_varint(&comp_cursor[prefix..]).unwrap().unwrap();
            let (id, body, rest) = read_compressed_frame(comp_cursor, 256, 1_048_576).unwrap();
            if id == play::clientbound::LEVEL_CHUNK_WITH_LIGHT {
                assert!(data_len > 0, "chunk packet must be zlib-compressed");
                assert!(
                    (pkt_len as usize) < data_len as usize,
                    "compressed chunk should be smaller than uncompressed"
                );
                comp_chunks.push(body);
            }
            comp_cursor = rest;
        }
        s.preview.as_mut().unwrap().awaiting_batch = false;
    }
    assert_eq!(comp_chunks.len(), 25);

    let mut uncomp_chunks = Vec::new();
    while let Some((uncomp_batch, _, _)) = uncomp_s.preview.as_mut().unwrap().next_chunk() {
        let mut uncomp_cursor = uncomp_batch.as_slice();
        while !uncomp_cursor.is_empty() {
            let (pkt_len, prefix) = read_varint(uncomp_cursor).unwrap().unwrap();
            let packet = &uncomp_cursor[prefix..prefix + pkt_len as usize];
            let (id, id_len) = read_varint(packet).unwrap().unwrap();
            if id == play::clientbound::LEVEL_CHUNK_WITH_LIGHT {
                uncomp_chunks.push(packet[id_len..].to_vec());
            }
            uncomp_cursor = &uncomp_cursor[prefix + pkt_len as usize..];
        }
        uncomp_s.preview.as_mut().unwrap().awaiting_batch = false;
    }
    assert_eq!(comp_chunks, uncomp_chunks);

    // Compressed play action -> compressed AcknowledgeBlockChange reply.
    let action = PlayerAction {
        status: PlayerActionStatus::StartedDigging,
        pos: (10, 64, -20),
        face: 1,
        sequence: 42,
    };
    let mut act_buf = Vec::new();
    frame_compressed(
        PLAYER_ACTION_ID,
        &encode_player_action(&action),
        256,
        &mut act_buf,
    )
    .unwrap();
    let act_replies = s.receive(&act_buf, 4096).unwrap();
    assert_eq!(act_replies.len(), 1);
    let (ack_id, ack_body, rest) = read_compressed_frame(&act_replies[0], 256, 4096).unwrap();
    assert!(rest.is_empty());
    assert_eq!(ack_id, BLOCK_CHANGED_ACK_ID);
    assert_eq!(ack_body, [42]);

    // Compressed chat -> compressed SystemChat reply + broadcast.
    let mut chat_body = Vec::new();
    put_string("hi compressed", &mut chat_body);
    chat_body.extend([0; 16]);
    chat_body.extend([0; 6]);
    let mut chat_buf = Vec::new();
    frame_compressed(play::serverbound::CHAT, &chat_body, 256, &mut chat_buf).unwrap();
    let chat_replies = s.receive(&chat_buf, 4096).unwrap();
    assert_eq!(chat_replies.len(), 1);
    let (sys_id, _, _) = read_compressed_frame(&chat_replies[0], 256, 4096).unwrap();
    assert_eq!(sys_id, play::clientbound::SYSTEM_CHAT);
    assert_eq!(s.take_broadcasts(), chat_replies);
}

#[test]
fn compressed_session_rejects_bad_client_frames_and_supports_pipelining() {
    let mut hs = handshake(777);
    *hs.last_mut().unwrap() = intent::LOGIN as u8;
    let mut login_body = vec![4];
    login_body.extend_from_slice(b"Test");
    login_body.extend_from_slice(&[7; 16]);
    let hello_pkt = frame(login::serverbound::HELLO, &login_body);

    // Pipeline Hello (uncompressed) and Login Acknowledged (compressed) in one buffer.
    let mut pipelined = Vec::new();
    pipelined.extend_from_slice(&hs);
    pipelined.extend_from_slice(&hello_pkt);
    frame_compressed(
        login::serverbound::LOGIN_ACKNOWLEDGED,
        &[],
        256,
        &mut pipelined,
    )
    .unwrap();

    let mut s = Session::new(true)
        .with_registry_manifest(test_manifest())
        .with_compression_threshold(Some(256));
    let replies = s.receive(&pipelined, 4096).unwrap();
    assert_eq!(s.state(), State::Configuration);
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[0], encode_set_compression(256));
    assert_eq!(
        read_compressed_frame(&replies[1], 256, 4096).unwrap().0,
        login::clientbound::LOGIN_FINISHED
    );
    assert_eq!(
        read_compressed_frame(&replies[2], 256, 4096).unwrap().0,
        configuration::clientbound::SELECT_KNOWN_PACKS
    );

    // Reject an invalid compressed frame whose declared length is below threshold (256).
    let mut bad_below = Session::new(true).with_compression_threshold(Some(256));
    bad_below.receive(&hs, 4096).unwrap();
    bad_below.receive(&hello_pkt, 4096).unwrap();
    let mut tiny_compressed = Vec::new();
    frame_compressed(
        login::serverbound::LOGIN_ACKNOWLEDGED,
        &[],
        0,
        &mut tiny_compressed,
    )
    .unwrap();
    assert_eq!(
        bad_below.receive(&tiny_compressed, 4096),
        Err(Error::Malformed)
    );

    // Reject a compressed frame whose declared Data Length exceeds MAX_FRAME (4096).
    let mut bad_oversized = Session::new(true).with_compression_threshold(Some(256));
    bad_oversized.receive(&hs, 4096).unwrap();
    bad_oversized.receive(&hello_pkt, 4096).unwrap();
    let mut forged = Vec::new();
    let mut inner = Vec::new();
    put_varint((MAX_FRAME + 1) as u32, &mut inner);
    inner.extend_from_slice(&[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
    put_varint(inner.len() as u32, &mut forged);
    forged.extend_from_slice(&inner);
    assert_eq!(bad_oversized.receive(&forged, 4096), Err(Error::Oversized));
}
