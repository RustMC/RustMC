//! Unit tests for `protocol::player_info`.

use super::*;

fn sample_uuid(val: u8) -> [u8; 16] {
    [val; 16]
}

fn sample_entry(id: u8, name: &str) -> PlayerInfoEntry {
    PlayerInfoEntry {
        uuid: sample_uuid(id),
        name: name.to_owned(),
        properties: vec![
            ProfileProperty::signed("textures", "sample_texture_payload", "sample_sig"),
            ProfileProperty::unsigned("custom_badge", "developer"),
        ],
        chat_session: Some(ChatSessionData::new(
            sample_uuid(0xAA),
            1_700_000_000_000,
            vec![0x01, 0x02, 0x03, 0x04],
            vec![0x0A, 0x0B, 0x0C, 0x0D],
        )),
        game_mode: GameMode::Creative,
        listed: true,
        latency: 42,
        display_name: Some(format!("§6{name}§r").into()),
        list_order: 10,
        show_hat: true,
    }
}

#[test]
fn test_action_bitflags_constants_and_operations() {
    assert_eq!(PlayerInfoAction::AddPlayer.flag(), 0x01);
    assert_eq!(PlayerInfoAction::InitializeChat.flag(), 0x02);
    assert_eq!(PlayerInfoAction::UpdateGameMode.flag(), 0x04);
    assert_eq!(PlayerInfoAction::UpdateListed.flag(), 0x08);
    assert_eq!(PlayerInfoAction::UpdateLatency.flag(), 0x10);
    assert_eq!(PlayerInfoAction::UpdateDisplayName.flag(), 0x20);
    assert_eq!(PlayerInfoAction::UpdateListOrder.flag(), 0x40);
    assert_eq!(PlayerInfoAction::UpdateHat.flag(), 0x80);

    let combined = PlayerInfoAction::AddPlayer | PlayerInfoAction::UpdateLatency;
    assert_eq!(combined, 0x11);
    assert!(PlayerInfoAction::is_set(
        combined,
        PlayerInfoAction::AddPlayer
    ));
    assert!(PlayerInfoAction::is_set(
        combined,
        PlayerInfoAction::UpdateLatency
    ));
    assert!(!PlayerInfoAction::is_set(
        combined,
        PlayerInfoAction::UpdateGameMode
    ));

    let all = PlayerInfoAction::ALL;
    assert_eq!(all, 0xFF);
    for action in [
        PlayerInfoAction::AddPlayer,
        PlayerInfoAction::InitializeChat,
        PlayerInfoAction::UpdateGameMode,
        PlayerInfoAction::UpdateListed,
        PlayerInfoAction::UpdateLatency,
        PlayerInfoAction::UpdateDisplayName,
        PlayerInfoAction::UpdateListOrder,
        PlayerInfoAction::UpdateHat,
    ] {
        assert!(PlayerInfoAction::is_set(all, action));
        assert_eq!(PlayerInfoAction::try_from(action.flag()).unwrap(), action);
    }

    assert_eq!(
        PlayerInfoAction::try_from(0x00).unwrap_err(),
        ProtocolError::InvalidActionBitmask(0x00)
    );
}

#[test]
fn test_game_mode_mapping() {
    assert_eq!(GameMode::from_id(0).unwrap(), GameMode::Survival);
    assert_eq!(GameMode::from_id(1).unwrap(), GameMode::Creative);
    assert_eq!(GameMode::from_id(2).unwrap(), GameMode::Adventure);
    assert_eq!(GameMode::from_id(3).unwrap(), GameMode::Spectator);
    assert_eq!(
        GameMode::from_id(4).unwrap_err(),
        ProtocolError::InvalidGameMode(4)
    );
    assert_eq!(
        GameMode::from_id(-1).unwrap_err(),
        ProtocolError::InvalidGameMode(-1)
    );
}

#[test]
fn test_roundtrip_all_actions_full_entry() {
    let entry1 = sample_entry(1, "Alice");
    let mut entry2 = sample_entry(2, "Bob");
    entry2.chat_session = None;
    entry2.display_name = None;
    entry2.properties.clear();
    entry2.game_mode = GameMode::Spectator;
    entry2.listed = false;
    entry2.latency = 120;

    let packet = PlayerInfoUpdatePacket::new(PlayerInfoAction::ALL, vec![entry1, entry2]);
    let encoded = packet.encode().expect("encode should succeed");
    let decoded = PlayerInfoUpdatePacket::decode(&encoded).expect("decode should succeed");
    assert_eq!(packet, decoded);
}

#[test]
fn test_roundtrip_individual_actions() {
    let uuid = sample_uuid(7);

    // 1. AddPlayer only
    let entry = PlayerInfoEntry {
        uuid,
        name: "Steve".to_owned(),
        properties: vec![ProfileProperty::unsigned("tex", "val")],
        ..Default::default()
    };
    let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::AddPlayer as u8, entry);
    let encoded = packet.encode().unwrap();
    let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
    assert_eq!(decoded.actions, PlayerInfoAction::AddPlayer as u8);
    assert_eq!(decoded.entries.len(), 1);
    assert_eq!(decoded.entries[0].name, "Steve");
    assert_eq!(decoded.entries[0].properties.len(), 1);

    // 2. InitializeChat only (with chat session)
    let entry = PlayerInfoEntry {
        uuid,
        chat_session: Some(ChatSessionData::new(
            sample_uuid(99),
            987654321,
            vec![1, 2, 3],
            vec![4, 5, 6],
        )),
        ..Default::default()
    };
    let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::InitializeChat as u8, entry);
    let encoded = packet.encode().unwrap();
    let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
    assert_eq!(decoded.actions, PlayerInfoAction::InitializeChat as u8);
    assert!(decoded.entries[0].chat_session.is_some());

    // 3. InitializeChat only (with None)
    let entry = PlayerInfoEntry {
        uuid,
        chat_session: None,
        ..Default::default()
    };
    let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::InitializeChat as u8, entry);
    let encoded = packet.encode().unwrap();
    let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
    assert_eq!(decoded.actions, PlayerInfoAction::InitializeChat as u8);
    assert!(decoded.entries[0].chat_session.is_none());

    // 4. UpdateGameMode only
    for mode in [
        GameMode::Survival,
        GameMode::Creative,
        GameMode::Adventure,
        GameMode::Spectator,
    ] {
        let entry = PlayerInfoEntry {
            uuid,
            game_mode: mode,
            ..Default::default()
        };
        let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateGameMode as u8, entry);
        let encoded = packet.encode().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
        assert_eq!(decoded.entries[0].game_mode, mode);
    }

    // 5. UpdateListed only
    for listed in [true, false] {
        let entry = PlayerInfoEntry {
            uuid,
            listed,
            ..Default::default()
        };
        let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateListed as u8, entry);
        let encoded = packet.encode().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
        assert_eq!(decoded.entries[0].listed, listed);
    }

    // 6. UpdateLatency only
    for lat in [0, 50, 999, -1, 3500] {
        let entry = PlayerInfoEntry {
            uuid,
            latency: lat,
            ..Default::default()
        };
        let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateLatency as u8, entry);
        let encoded = packet.encode().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
        assert_eq!(decoded.entries[0].latency, lat);
    }

    // 7. UpdateDisplayName only (Some and None)
    let entry = PlayerInfoEntry {
        uuid,
        display_name: Some("Custom Nick".into()),
        ..Default::default()
    };
    let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateDisplayName as u8, entry);
    let encoded = packet.encode().unwrap();
    let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
    assert_eq!(
        decoded.entries[0].display_name.as_deref(),
        Some("Custom Nick")
    );

    let entry = PlayerInfoEntry {
        uuid,
        display_name: None,
        ..Default::default()
    };
    let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateDisplayName as u8, entry);
    let encoded = packet.encode().unwrap();
    let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
    assert_eq!(decoded.entries[0].display_name, None);

    // 8. UpdateListOrder only
    for order in [0, 1, 999, -50] {
        let entry = PlayerInfoEntry {
            uuid,
            list_order: order,
            ..Default::default()
        };
        let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateListOrder as u8, entry);
        let encoded = packet.encode().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
        assert_eq!(decoded.entries[0].list_order, order);
    }

    // 9. UpdateHat only
    for hat in [true, false] {
        let entry = PlayerInfoEntry {
            uuid,
            show_hat: hat,
            ..Default::default()
        };
        let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateHat as u8, entry);
        let encoded = packet.encode().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
        assert_eq!(decoded.entries[0].show_hat, hat);
    }
}

#[test]
fn test_all_256_bitmask_combinations_roundtrip() {
    for mask in 0u8..=0xFF {
        let entry = sample_entry(mask, "Tester");
        let packet = PlayerInfoUpdatePacket::new(mask, vec![entry]);
        let encoded = packet.encode().expect("encoding must succeed");
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).expect("decoding must succeed");
        assert_eq!(decoded.actions, mask);
        assert_eq!(decoded.entries.len(), 1);
        assert_eq!(decoded.entries[0].uuid, sample_uuid(mask));

        if PlayerInfoAction::is_set(mask, PlayerInfoAction::AddPlayer) {
            assert_eq!(decoded.entries[0].name, "Tester");
            assert_eq!(decoded.entries[0].properties.len(), 2);
        }
        if PlayerInfoAction::is_set(mask, PlayerInfoAction::InitializeChat) {
            assert!(decoded.entries[0].chat_session.is_some());
        }
        if PlayerInfoAction::is_set(mask, PlayerInfoAction::UpdateGameMode) {
            assert_eq!(decoded.entries[0].game_mode, GameMode::Creative);
        }
        if PlayerInfoAction::is_set(mask, PlayerInfoAction::UpdateListed) {
            assert!(decoded.entries[0].listed);
        }
        if PlayerInfoAction::is_set(mask, PlayerInfoAction::UpdateLatency) {
            assert_eq!(decoded.entries[0].latency, 42);
        }
        if PlayerInfoAction::is_set(mask, PlayerInfoAction::UpdateDisplayName) {
            assert_eq!(
                decoded.entries[0].display_name.as_deref(),
                Some("§6Tester§r")
            );
        }
        if PlayerInfoAction::is_set(mask, PlayerInfoAction::UpdateListOrder) {
            assert_eq!(decoded.entries[0].list_order, 10);
        }
        if PlayerInfoAction::is_set(mask, PlayerInfoAction::UpdateHat) {
            assert!(decoded.entries[0].show_hat);
        }
    }
}

#[test]
fn test_non_canonical_and_malformed_inputs() {
    // 1. Non-canonical VarInt with trailing zero [0x80, 0x00]
    let non_canonical = [0x80, 0x00];
    assert_eq!(
        read_varint(&non_canonical).unwrap_err(),
        ProtocolError::NonCanonicalVarInt
    );

    // 2. Malformed 6-byte VarInt
    let malformed = [0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
    assert_eq!(
        read_varint(&malformed).unwrap_err(),
        ProtocolError::MalformedVarInt
    );

    // 3. VarInt 5th byte with overflow bits
    let overflow = [0x80, 0x80, 0x80, 0x80, 0xF0];
    assert_eq!(
        read_varint(&overflow).unwrap_err(),
        ProtocolError::MalformedVarInt
    );

    // 4. Unexpected EOF on empty input
    assert_eq!(
        PlayerInfoUpdatePacket::decode(&[]).unwrap_err(),
        ProtocolError::UnexpectedEof
    );

    // 5. Unexpected packet ID (0x3F instead of 0x47)
    let mut wrong_packet = Vec::new();
    write_varint(2, &mut wrong_packet); // Frame len = 2
    write_varint(0x3F, &mut wrong_packet); // Wrong packet ID 0x3F
    wrong_packet.push(0x00); // Empty actions
    assert_eq!(
        PlayerInfoUpdatePacket::decode(&wrong_packet).unwrap_err(),
        ProtocolError::UnexpectedPacketId {
            expected: 0x47,
            received: 0x3F,
        }
    );

    // 6. Action bitmask TryFrom rejects invalid single flags (0x00, 0x43)
    assert_eq!(
        PlayerInfoAction::try_from(0x00).unwrap_err(),
        ProtocolError::InvalidActionBitmask(0x00)
    );
    assert_eq!(
        PlayerInfoAction::try_from(0x43).unwrap_err(),
        ProtocolError::InvalidActionBitmask(0x43)
    );

    // 7. Invalid boolean (byte 0x02)
    assert_eq!(
        read_bool(&[0x02]).unwrap_err(),
        ProtocolError::InvalidBoolean(0x02)
    );

    // 8. Trailing data after framed packet
    let valid_packet = PlayerInfoUpdatePacket::new(0, vec![]);
    let mut framed_with_trailing = valid_packet.encode().unwrap();
    framed_with_trailing.push(0xFF);
    assert_eq!(
        PlayerInfoUpdatePacket::decode(&framed_with_trailing).unwrap_err(),
        ProtocolError::TrailingData { remaining: 1 }
    );

    // 9. Name string exceeding 16 chars
    let long_name_entry = PlayerInfoEntry {
        uuid: sample_uuid(1),
        name: "SuperLongPlayerNameExceeding16".to_owned(),
        ..Default::default()
    };
    let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::AddPlayer as u8, long_name_entry);
    assert!(matches!(
        packet.encode().unwrap_err(),
        ProtocolError::StringTooLong { max: 16, .. }
    ));
}

#[test]
fn test_tab_list_tracker_state_transitions() {
    let mut tracker = TabListTracker::new();
    assert_eq!(tracker.len(), 0);
    assert!(tracker.is_empty());

    let uuid1 = sample_uuid(1);
    let uuid2 = sample_uuid(2);

    // 1. Add player 1
    let entry1 = sample_entry(1, "PlayerOne");
    let add_packet1 = tracker.add_player(entry1);
    assert_eq!(tracker.len(), 1);
    assert!(!tracker.is_empty());
    assert!(tracker.contains_player(&uuid1));
    assert_eq!(add_packet1.entries.len(), 1);
    assert_eq!(add_packet1.entries[0].name, "PlayerOne");
    assert!(PlayerInfoAction::is_set(
        add_packet1.actions,
        PlayerInfoAction::AddPlayer
    ));

    // 2. Add player 2
    let mut entry2 = sample_entry(2, "PlayerTwo");
    entry2.chat_session = None;
    let add_packet2 = tracker.add_player(entry2);
    assert_eq!(tracker.len(), 2);
    assert!(tracker.contains_player(&uuid2));
    assert_eq!(add_packet2.entries[0].name, "PlayerTwo");
    assert!(!PlayerInfoAction::is_set(
        add_packet2.actions,
        PlayerInfoAction::InitializeChat
    ));

    // 3. Update latency
    let lat_packet = tracker
        .update_latency(&uuid1, 95)
        .expect("update latency should succeed");
    assert_eq!(lat_packet.actions, PlayerInfoAction::UpdateLatency as u8);
    assert_eq!(lat_packet.entries[0].uuid, uuid1);
    assert_eq!(lat_packet.entries[0].latency, 95);
    assert_eq!(tracker.get_player(&uuid1).unwrap().latency, 95);

    // Latency update for unknown player fails gracefully
    assert!(tracker.update_latency(&sample_uuid(99), 10).is_none());

    // 4. Update game mode
    let gm_packet = tracker
        .update_game_mode(&uuid1, GameMode::Adventure)
        .expect("update game mode should succeed");
    assert_eq!(gm_packet.actions, PlayerInfoAction::UpdateGameMode as u8);
    assert_eq!(gm_packet.entries[0].game_mode, GameMode::Adventure);
    assert_eq!(
        tracker.get_player(&uuid1).unwrap().game_mode,
        GameMode::Adventure
    );

    // Game mode update for unknown player fails
    assert!(
        tracker
            .update_game_mode(&sample_uuid(99), GameMode::Survival)
            .is_none()
    );

    // 5. Update display name
    let dn_packet = tracker
        .update_display_name(&uuid1, Some("MasterOne".into()))
        .expect("update display name should succeed");
    assert_eq!(dn_packet.actions, PlayerInfoAction::UpdateDisplayName as u8);
    assert_eq!(
        dn_packet.entries[0].display_name.as_deref(),
        Some("MasterOne")
    );
    assert_eq!(
        tracker.get_player(&uuid1).unwrap().display_name.as_deref(),
        Some("MasterOne")
    );

    // Clear display name to None
    let dn_clear_packet = tracker
        .update_display_name(&uuid1, None)
        .expect("clearing display name should succeed");
    assert_eq!(dn_clear_packet.entries[0].display_name, None);
    assert_eq!(tracker.get_player(&uuid1).unwrap().display_name, None);

    // 6. Update listed status
    let listed_packet = tracker
        .update_listed(&uuid2, false)
        .expect("update listed should succeed");
    assert_eq!(listed_packet.actions, PlayerInfoAction::UpdateListed as u8);
    assert!(!listed_packet.entries[0].listed);
    assert!(!tracker.get_player(&uuid2).unwrap().listed);

    // 7. Full tab list packet
    let full_sync = tracker.full_tab_list_packet();
    assert_eq!(full_sync.actions, PlayerInfoAction::ALL);
    assert_eq!(full_sync.entries.len(), 2);
    // Deterministic order by UUID (uuid1 < uuid2)
    assert_eq!(full_sync.entries[0].uuid, uuid1);
    assert_eq!(full_sync.entries[1].uuid, uuid2);

    // Full sync can be encoded and decoded cleanly
    let encoded_sync = full_sync.encode().unwrap();
    let decoded_sync = PlayerInfoUpdatePacket::decode(&encoded_sync).unwrap();
    assert_eq!(full_sync, decoded_sync);

    // 8. Remove player
    let removed1 = tracker
        .remove_player(&uuid1)
        .expect("player 1 should be removed");
    assert_eq!(removed1.uuid, uuid1);
    assert_eq!(tracker.len(), 1);
    assert!(!tracker.contains_player(&uuid1));

    // Removing already-removed player returns None
    assert!(tracker.remove_player(&uuid1).is_none());

    // Remove player 2
    let removed2 = tracker
        .remove_player(&uuid2)
        .expect("player 2 should be removed");
    assert_eq!(removed2.uuid, uuid2);
    assert_eq!(tracker.len(), 0);
    assert!(tracker.is_empty());
}

#[test]
fn test_unframed_payload_codec_direct() {
    let entry = sample_entry(42, "DirectCodec");
    let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::ALL, entry);
    let payload = packet.encode_payload().unwrap();
    let decoded = PlayerInfoUpdatePacket::decode_payload(&payload).unwrap();
    assert_eq!(packet, decoded);
}

#[test]
fn test_empty_entries_packet_roundtrip() {
    let packet = PlayerInfoUpdatePacket::new(PlayerInfoAction::ALL, vec![]);
    let encoded = packet.encode().unwrap();
    let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
    assert_eq!(packet, decoded);
    assert_eq!(decoded.entries.len(), 0);
}

#[test]
fn test_string_boundary_length_16_vs_17() {
    let name_16 = "1234567890123456";
    assert_eq!(name_16.len(), 16);
    let entry_ok = PlayerInfoEntry::new(sample_uuid(1), name_16);
    let packet_ok = PlayerInfoUpdatePacket::single(PlayerInfoAction::AddPlayer as u8, entry_ok);
    assert!(packet_ok.encode().is_ok());

    let name_17 = "12345678901234567";
    assert_eq!(name_17.len(), 17);
    let entry_err = PlayerInfoEntry::new(sample_uuid(2), name_17);
    let packet_err = PlayerInfoUpdatePacket::single(PlayerInfoAction::AddPlayer as u8, entry_err);
    assert!(matches!(
        packet_err.encode().unwrap_err(),
        ProtocolError::StringTooLong {
            length: 17,
            max: 16
        }
    ));
}

#[test]
fn test_invalid_utf8_in_string_rejected() {
    // Construct payload with an invalid UTF-8 byte sequence (0xFF, 0xFE)
    let invalid_utf8_bytes = [0xFF, 0xFE];
    let mut raw_str = Vec::new();
    write_varint(invalid_utf8_bytes.len() as i32, &mut raw_str);
    raw_str.extend_from_slice(&invalid_utf8_bytes);

    let res = read_string(&raw_str, 16);
    assert_eq!(res.unwrap_err(), ProtocolError::InvalidUtf8);
}

#[test]
fn test_tracker_getters_and_clear() {
    let mut tracker = TabListTracker::new();
    let e1 = sample_entry(10, "User10");
    let e2 = sample_entry(20, "User20");
    tracker.add_player(e1);
    tracker.add_player(e2);

    assert_eq!(tracker.len(), 2);
    assert_eq!(tracker.get_player(&sample_uuid(10)).unwrap().name, "User10");
    assert_eq!(tracker.get_player(&sample_uuid(20)).unwrap().name, "User20");

    // Mutate player in-place
    if let Some(p) = tracker.get_player_mut(&sample_uuid(10)) {
        p.latency = 999;
    }
    assert_eq!(tracker.get_player(&sample_uuid(10)).unwrap().latency, 999);

    // Iterate over players
    let names: Vec<&str> = tracker.players().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["User10", "User20"]);

    // Clear
    tracker.clear();
    assert_eq!(tracker.len(), 0);
    assert!(tracker.is_empty());
}

#[test]
fn test_chat_session_key_and_signature_exact_preservation() {
    let pub_key = vec![0x30, 0x82, 0x01, 0x22, 0x30, 0x0D, 0x06, 0x09];
    let key_sig = vec![0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00, 0x11];
    let chat = ChatSessionData::new(
        sample_uuid(0x55),
        1_234_567_890_123,
        pub_key.clone(),
        key_sig.clone(),
    );

    let mut entry = PlayerInfoEntry::new(sample_uuid(1), "CryptoUser");
    entry.chat_session = Some(chat);

    let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::InitializeChat as u8, entry);
    let encoded = packet.encode().unwrap();
    let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();

    let decoded_chat = decoded.entries[0].chat_session.as_ref().unwrap();
    assert_eq!(decoded_chat.session_id, sample_uuid(0x55));
    assert_eq!(decoded_chat.expires_at, 1_234_567_890_123);
    assert_eq!(decoded_chat.public_key, pub_key);
    assert_eq!(decoded_chat.key_signature, key_sig);
}

#[test]
fn test_independent_protocol_777_player_info_update_fixture() {
    // Independently constructed framed fixture for Java 26.3 protocol 777 PlayerInfoUpdate:
    // Frame length: 27 bytes (0x1B)
    // Packet ID: VarInt 0x47 (clientbound PlayerInfoUpdate)
    // Actions bitmask: 0x20 (UpdateDisplayName)
    // Number of entries: VarInt 1 (0x01)
    // UUID: 16 bytes [0x07; 16]
    // has_display_name: 0x01 (true)
    // Network NBT TAG_String (0x08):
    //   tag type: 0x08
    //   length (u16 BE): 0x00, 0x04
    //   string: b"Nick"
    let mut fixture = Vec::new();
    // Frame length = 1 (packet ID 0x47) + 1 (action 0x20) + 1 (count 1) + 16 (UUID) + 1 (has_dn) + 1 (tag 0x08) + 2 (len 4) + 4 (Nick) = 27 bytes
    write_varint(27, &mut fixture);
    write_varint(0x47, &mut fixture); // Packet ID 0x47
    fixture.push(0x20); // Action: UpdateDisplayName
    write_varint(1, &mut fixture); // 1 entry
    fixture.extend_from_slice(&[0x07; 16]); // UUID
    fixture.push(0x01); // has_display_name = true
    fixture.push(0x08); // TAG_String
    fixture.extend_from_slice(&[0x00, 0x04]); // length 4
    fixture.extend_from_slice(b"Nick"); // string "Nick"

    let packet = PlayerInfoUpdatePacket::decode(&fixture)
        .expect("fixture decoding must succeed with packet ID 0x47");
    assert_eq!(packet.actions, 0x20);
    assert_eq!(packet.entries.len(), 1);
    assert_eq!(packet.entries[0].uuid, [0x07; 16]);
    assert_eq!(packet.entries[0].display_name.as_deref(), Some("Nick"));
}

#[test]
fn test_network_nbt_compound_display_name_decoding() {
    // Independently constructed framed fixture with anonymous TAG_Compound display name:
    // Actions: 0x20
    // has_display_name: true
    // Anonymous TAG_Compound (0x0A):
    //   TAG_String (0x08)
    //   name length (u16 BE): 0x00, 0x04
    //   name: "text"
    //   string length (u16 BE): 0x00, 0x05
    //   value: "Hello"
    //   TAG_End (0x00)
    let mut fixture = Vec::new();
    let mut compound = Vec::new();
    compound.push(0x0A); // TAG_Compound
    compound.push(0x08); // TAG_String
    compound.extend_from_slice(&4u16.to_be_bytes()); // "text" length
    compound.extend_from_slice(b"text");
    compound.extend_from_slice(&5u16.to_be_bytes()); // "Hello" length
    compound.extend_from_slice(b"Hello");
    compound.push(0x00); // TAG_End

    let payload_len = 1 + 1 + 16 + 1 + compound.len();
    let total_frame_len = 1 + payload_len; // 1 byte for VarInt 0x47
    write_varint(total_frame_len as i32, &mut fixture);
    write_varint(0x47, &mut fixture);
    fixture.push(0x20);
    write_varint(1, &mut fixture);
    fixture.extend_from_slice(&[0x08; 16]);
    fixture.push(0x01);
    fixture.extend_from_slice(&compound);

    let packet = PlayerInfoUpdatePacket::decode(&fixture)
        .expect("compound display name fixture must decode successfully");
    assert_eq!(packet.entries[0].display_name.as_deref(), Some("Hello"));
}

#[test]
fn test_actions_0x40_and_0x80_wire_fixture() {
    // Test independently constructed wire bytes for UpdateListOrder (0x40) and UpdateHat (0x80)
    let mut fixture = Vec::new();
    let mut payload = Vec::new();
    payload.push(0xC0); // 0x40 | 0x80
    write_varint(1, &mut payload); // 1 entry
    payload.extend_from_slice(&[0x09; 16]); // UUID
    write_varint(42, &mut payload); // list_order = 42
    payload.push(0x01); // show_hat = true

    let total_frame_len = 1 + payload.len();
    write_varint(total_frame_len as i32, &mut fixture);
    write_varint(0x47, &mut fixture);
    fixture.extend_from_slice(&payload);

    let packet = PlayerInfoUpdatePacket::decode(&fixture)
        .expect("0xC0 actions fixture must decode successfully");
    assert_eq!(packet.actions, 0xC0);
    assert_eq!(packet.entries[0].list_order, 42);
    assert!(packet.entries[0].show_hat);
}

fn too_long(packet: PlayerInfoUpdatePacket, length: usize, max: usize) {
    assert_eq!(
        packet.encode_payload().unwrap_err(),
        ProtocolError::StringTooLong { length, max }
    );
}

#[test]
fn test_encode_field_size_limit_validations() {
    let uuid = sample_uuid(1);
    let add = PlayerInfoAction::AddPlayer as u8;
    let chat = PlayerInfoAction::InitializeChat as u8;

    let mut entry = PlayerInfoEntry::new(uuid, "Steve");
    entry.properties = vec![ProfileProperty::unsigned("custom", "x".repeat(32768))];
    too_long(PlayerInfoUpdatePacket::single(add, entry), 32768, 32767);

    let mut entry = PlayerInfoEntry::new(uuid, "Steve");
    entry.properties = vec![ProfileProperty::unsigned("n".repeat(65), "val")];
    too_long(PlayerInfoUpdatePacket::single(add, entry), 65, 64);

    let mut entry = PlayerInfoEntry::new(uuid, "Steve");
    entry.properties = vec![ProfileProperty::signed("textures", "val", "s".repeat(1025))];
    too_long(PlayerInfoUpdatePacket::single(add, entry), 1025, 1024);

    let mut entry = PlayerInfoEntry::new(uuid, "Steve");
    entry.properties = vec![ProfileProperty::unsigned("a", "b"); 17];
    too_long(PlayerInfoUpdatePacket::single(add, entry), 17, 16);

    let mut entry = PlayerInfoEntry::new(uuid, "Steve");
    entry.chat_session = Some(ChatSessionData::new(
        sample_uuid(2),
        12345,
        vec![0x42; 513],
        vec![1, 2, 3],
    ));
    too_long(PlayerInfoUpdatePacket::single(chat, entry), 513, 512);

    let mut entry = PlayerInfoEntry::new(uuid, "Steve");
    entry.chat_session = Some(ChatSessionData::new(
        sample_uuid(2),
        12345,
        vec![1, 2, 3],
        vec![0x42; 4097],
    ));
    too_long(PlayerInfoUpdatePacket::single(chat, entry), 4097, 4096);

    let mut entry = PlayerInfoEntry::new(uuid, "Steve");
    entry.display_name = Some("d".repeat(32768).into());
    too_long(
        PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateDisplayName as u8, entry),
        32768,
        32767,
    );

    // 32767 code units fit the string limit, but not 65535 encoded NBT bytes.
    let mut entry = PlayerInfoEntry::new(uuid, "Steve");
    entry.display_name = Some("\u{20AC}".repeat(32767).into());
    too_long(
        PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateDisplayName as u8, entry),
        98301,
        65535,
    );
}

#[test]
fn test_decode_enforces_the_same_field_limits() {
    // Actions 0x01, one entry, name "ab", 17 properties.
    let mut payload = vec![0x01, 0x01];
    payload.extend([0x01; 16]);
    payload.extend([0x02, b'a', b'b', 0x11]);
    assert_eq!(
        PlayerInfoUpdatePacket::decode_payload(&payload).unwrap_err(),
        ProtocolError::StringTooLong {
            length: 17,
            max: 16
        }
    );
}

/// Wraps an anonymous NBT component in a display-name-only packet payload.
fn display_name_payload(component: &[u8]) -> Vec<u8> {
    let mut payload = vec![0x20, 0x01];
    payload.extend([0x07; 16]);
    payload.push(0x01);
    payload.extend_from_slice(component);
    payload
}

#[test]
fn test_display_name_uses_modified_utf8() {
    // U+1F600 is the surrogate pair D83D DE00, three bytes each.
    let component = [0x08, 0x00, 0x06, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80];
    let payload = display_name_payload(&component);

    let mut entry = PlayerInfoEntry::new([0x07; 16], "");
    entry.display_name = Some("\u{1F600}".into());
    let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateDisplayName as u8, entry);
    assert_eq!(packet.encode_payload().unwrap(), payload);

    let decoded = PlayerInfoUpdatePacket::decode_payload(&payload).unwrap();
    assert_eq!(
        decoded.entries[0].display_name.as_deref(),
        Some("\u{1F600}")
    );

    // U+0000 is C0 80 and never a zero byte.
    let nul = [0x08, 0x00, 0x02, 0xC0, 0x80];
    assert_eq!(
        PlayerDisplayName::from_raw_nbt(&nul).unwrap(),
        PlayerDisplayName::plain("\0")
    );
    for invalid in [
        &[0x08, 0x00, 0x01, 0x00][..],                   // Raw NUL.
        &[0x08, 0x00, 0x04, 0xF0, 0x9F, 0x98, 0x80][..], // Standard 4-byte UTF-8.
        &[0x08, 0x00, 0x03, 0xED, 0xA0, 0xBD][..],       // Lone high surrogate.
        &[0x08, 0x00, 0x03, 0xED, 0xB8, 0x80][..],       // Lone low surrogate.
    ] {
        assert_eq!(
            PlayerDisplayName::from_raw_nbt(invalid),
            Err(ProtocolError::InvalidUtf8)
        );
    }
}

#[test]
fn test_nested_component_is_preserved_byte_for_byte() {
    // {"text":"","extra":[{"text":"Nick"}],"color":"gold"}
    #[rustfmt::skip]
    let component = [
        0x0A,
        0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x00,
        0x09, 0x00, 0x05, b'e', b'x', b't', b'r', b'a', 0x0A, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x04, b'N', b'i', b'c', b'k',
        0x00,
        0x08, 0x00, 0x05, b'c', b'o', b'l', b'o', b'r', 0x00, 0x04, b'g', b'o', b'l', b'd',
        0x00,
    ];
    let payload = display_name_payload(&component);
    let decoded = PlayerInfoUpdatePacket::decode_payload(&payload).unwrap();
    let name = decoded.entries[0].display_name.as_ref().unwrap();
    assert_eq!(name.as_str(), "Nick");
    assert_eq!(name.raw_nbt(), Some(&component[..]));
    assert_eq!(decoded.encode_payload().unwrap(), payload);

    // {"translate":"k"} has no literal text and still survives a round trip.
    #[rustfmt::skip]
    let translated = [
        0x0A,
        0x08, 0x00, 0x09, b't', b'r', b'a', b'n', b's', b'l', b'a', b't', b'e',
        0x00, 0x01, b'k',
        0x00,
    ];
    let payload = display_name_payload(&translated);
    let decoded = PlayerInfoUpdatePacket::decode_payload(&payload).unwrap();
    assert_eq!(decoded.entries[0].display_name.as_deref(), Some(""));
    assert_eq!(decoded.encode_payload().unwrap(), payload);

    // Literal text keeps its order when `extra` precedes `text`, and an
    // `extra` list may hold plain strings.
    #[rustfmt::skip]
    let reversed = [
        0x0A,
        0x09, 0x00, 0x05, b'e', b'x', b't', b'r', b'a', 0x08, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x01, b'b',
        0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x01, b'a',
        0x00,
    ];
    let name = PlayerDisplayName::from_raw_nbt(&reversed).unwrap();
    assert_eq!(name.as_str(), "ab");

    assert_eq!(
        PlayerDisplayName::from_raw_nbt(&[0x0A, 0x00]),
        Err(ProtocolError::UnsupportedComponent)
    );
}

#[test]
fn test_hand_built_raw_component_is_validated_on_encode() {
    let mut entry = PlayerInfoEntry::new([0x07; 16], "");
    entry.display_name = Some(PlayerDisplayName::RawNbt {
        raw: vec![0x0A, 0x63],
        text: "x".into(),
    });
    let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateDisplayName as u8, entry);
    assert!(packet.encode_payload().is_err());
}

#[test]
fn test_nbt_nesting_is_bounded() {
    let max = MAX_NBT_DEPTH;

    // Nested compounds under a non-component field reach the skipper.
    let mut compounds = vec![0x0A];
    for _ in 0..100_000 {
        compounds.extend([0x0A, 0x00, 0x01, b'a']);
    }
    assert!(matches!(
        PlayerInfoUpdatePacket::decode_payload(&display_name_payload(&compounds)),
        Err(ProtocolError::NbtDepthExceeded { max: m, .. }) if m == max
    ));

    // Nested lists of lists.
    let mut lists = vec![0x0A, 0x09, 0x00, 0x01, b'a'];
    for _ in 0..100_000 {
        lists.extend([0x09, 0x00, 0x00, 0x00, 0x01]);
    }
    assert!(matches!(
        PlayerInfoUpdatePacket::decode_payload(&display_name_payload(&lists)),
        Err(ProtocolError::NbtDepthExceeded { max: m, .. }) if m == max
    ));

    // Nested `extra` children use the text-collecting path.
    let mut extras = vec![0x0A];
    for _ in 0..100_000 {
        extras.extend([0x09, 0x00, 0x05, b'e', b'x', b't', b'r', b'a', 0x0A]);
        extras.extend([0x00, 0x00, 0x00, 0x01]);
    }
    assert!(matches!(
        PlayerInfoUpdatePacket::decode_payload(&display_name_payload(&extras)),
        Err(ProtocolError::NbtDepthExceeded { max: m, .. }) if m == max
    ));
}

#[test]
fn test_full_action_mask_wire_fixture() {
    // Every action in table order, written by hand.
    #[rustfmt::skip]
    let mut payload = vec![0xFF, 0x01];
    payload.extend([0x0B; 16]);
    payload.extend([0x03, b'A', b'b', b'c']); // Name.
    payload.push(0x01); // One property.
    payload.extend([0x01, b'k', 0x01, b'v', 0x01, 0x01, b's']);
    payload.push(0x01); // Chat session present.
    payload.extend([0x0C; 16]);
    payload.extend([0, 0, 0, 0, 0, 0, 0x30, 0x39]); // Expiry 12345.
    payload.extend([0x01, 0xAA, 0x02, 0xBB, 0xCC]); // Key and key signature.
    payload.push(0x03); // Spectator.
    payload.push(0x01); // Listed.
    payload.extend([0xFF, 0xFF, 0xFF, 0xFF, 0x0F]); // Ping -1.
    payload.push(0x00); // No display name.
    payload.push(0x2A); // Priority 42.
    payload.push(0x00); // Hat hidden.

    let decoded = PlayerInfoUpdatePacket::decode_payload(&payload).unwrap();
    let entry = &decoded.entries[0];
    assert_eq!(entry.name, "Abc");
    assert_eq!(entry.properties[0].signature.as_deref(), Some("s"));
    assert_eq!(entry.chat_session.as_ref().unwrap().expires_at, 12345);
    assert_eq!(entry.game_mode, GameMode::Spectator);
    assert_eq!(entry.latency, -1);
    assert_eq!(entry.list_order, 42);
    assert!(!entry.show_hat);
    assert_eq!(decoded.encode_payload().unwrap(), payload);
}

#[test]
fn test_tracker_list_order_and_hat() {
    let mut tracker = TabListTracker::new();
    let uuid = sample_uuid(4);
    let entry = sample_entry(4, "OrderHatTester");
    tracker.add_player(entry);

    // Update list order
    let order_packet = tracker.update_list_order(&uuid, 77).unwrap();
    assert_eq!(
        order_packet.actions,
        PlayerInfoAction::UpdateListOrder as u8
    );
    assert_eq!(order_packet.entries[0].list_order, 77);
    assert_eq!(tracker.get_player(&uuid).unwrap().list_order, 77);

    // Update hat
    let hat_packet = tracker.update_hat(&uuid, false).unwrap();
    assert_eq!(hat_packet.actions, PlayerInfoAction::UpdateHat as u8);
    assert!(!hat_packet.entries[0].show_hat);
    assert!(!tracker.get_player(&uuid).unwrap().show_hat);
}
