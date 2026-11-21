//! Unit tests, part 1 of 3.

use super::*;

pub(super) fn handshake(version: u32) -> Vec<u8> {
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
pub(super) fn hello(name: &[u8]) -> Vec<u8> {
    let mut body = vec![name.len() as u8];
    body.extend_from_slice(name);
    body.extend_from_slice(&[7; 16]);
    frame(login::serverbound::HELLO, &body)
}

pub(super) fn assert_refused(mut session: Session, version: u32, reason: &str) {
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
