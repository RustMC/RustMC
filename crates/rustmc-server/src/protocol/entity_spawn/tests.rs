//! Unit tests for `protocol::entity_spawn`.

use super::*;

#[test]
fn test_angle_quantization_and_wrapping() {
    assert_eq!(angle_to_byte(0.0), 0);
    assert_eq!(angle_to_byte(90.0), 64);
    assert_eq!(angle_to_byte(180.0), 128);
    assert_eq!(angle_to_byte(270.0), 192);
    assert_eq!(angle_to_byte(360.0), 0);

    // Negative angles
    assert_eq!(angle_to_byte(-90.0), 192);
    assert_eq!(angle_to_byte(-180.0), 128);
    assert_eq!(angle_to_byte(-270.0), 64);
    assert_eq!(angle_to_byte(-360.0), 0);

    // Small negative fraction
    assert_eq!(angle_to_byte(-0.5), 255);

    // Round-trip test within 1 byte precision (360.0 / 256.0 = 1.40625 deg)
    for byte in 0..=255u8 {
        let deg = byte_to_angle(byte);
        assert_eq!(angle_to_byte(deg), byte);
    }
}

#[test]
fn test_position_pack_unpack_roundtrip() {
    let test_cases = [
        [0, 0, 0],
        [-5, 64, -100],
        [12345, -64, 67890],
        [-30_000_000, 319, 30_000_000],
        [30_000_000, -2048, -30_000_000],
    ];

    for pos in test_cases {
        let packed = pack_position(pos[0], pos[1], pos[2]);
        let unpacked = unpack_position(packed);
        assert_eq!(pos, unpacked, "Failed round-trip for pos: {:?}", pos);
    }
}

#[test]
fn packet_ids_match_the_26_3_report() {
    assert_eq!(PACKET_ID_SPAWN_ENTITY, 1);
    assert_eq!(PACKET_ID_SET_ENTITY_DATA, 101);
    assert_eq!(PACKET_ID_REMOVE_ENTITIES, 78);
}

// Wire fixtures below are written by hand from the protocol tables, not
// produced by this module's encoders.

fn spawn_fixture() -> Vec<u8> {
    let mut bytes = vec![0x2A]; // Entity ID 42.
    bytes.extend([0x12; 16]); // UUID.
    bytes.push(0x64); // Type 100.
    bytes.extend([0x40, 0x24, 0, 0, 0, 0, 0, 0]); // X 10.0.
    bytes.extend([0x40, 0x50, 0, 0, 0, 0, 0, 0]); // Y 64.0.
    bytes.extend([0xC0, 0x24, 0, 0, 0, 0, 0, 0]); // Z -10.0.
    bytes.push(0x00); // Zero LpVec3 velocity comes before the angles.
    bytes.extend([0x40, 0x80, 0xC0]); // Pitch, yaw, head yaw.
    bytes.push(0x07); // Data comes last.
    bytes
}

#[test]
fn test_spawn_entity_packet_payload_and_framing() {
    let packet = SpawnEntityPacket::new(EntitySpawnFields {
        entity_id: 42,
        uuid: [0x12; 16],
        entity_type: 100,
        position: [10.0, 64.0, -10.0],
        pitch: 0x40,
        yaw: 0x80,
        head_yaw: 0xC0,
        data: 7,
    })
    .expect("valid coords");

    let fixture = spawn_fixture();
    let mut payload = Vec::new();
    packet.encode_payload(&mut payload);
    assert_eq!(payload, fixture);
    assert_eq!(SpawnEntityPacket::decode_payload(&fixture).unwrap(), packet);

    let mut framed = vec![fixture.len() as u8 + 1, 0x01];
    framed.extend(&fixture);
    assert_eq!(packet.encode(), framed);
    assert_eq!(SpawnEntityPacket::decode(&framed).unwrap(), packet);

    assert!(
        SpawnEntityPacket::new(EntitySpawnFields {
            entity_id: 1,
            uuid: [0; 16],
            entity_type: 1,
            position: [f64::NAN, 0.0, 0.0],
            pitch: 0,
            yaw: 0,
            head_yaw: 0,
            data: 0,
        })
        .is_err()
    );
    assert!(SpawnEntityPacket::decode_payload(&fixture[..fixture.len() - 1]).is_err());

    let mut trailing = fixture.clone();
    trailing.push(0x99);
    assert_eq!(
        SpawnEntityPacket::decode_payload(&trailing),
        Err(EntityProtocolError::TrailingData)
    );

    // A nonzero LpVec3 has no model here and must not be misread as angles.
    let mut moving = fixture;
    moving[42] = 0x01;
    assert_eq!(
        SpawnEntityPacket::decode_payload(&moving),
        Err(EntityProtocolError::UnsupportedVelocity)
    );

    assert_eq!(
        SpawnEntityPacket::decode(&[0x02, 0x7F, 0x00]),
        Err(EntityProtocolError::UnexpectedPacketId {
            expected: 0x01,
            received: 0x7F
        })
    );
}

#[test]
fn test_remove_entities_packet_payload_and_framing() {
    // Three IDs: 1, 300 and -1.
    let payload = [0x03, 0x01, 0xAC, 0x02, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
    let packet = RemoveEntitiesPacket::new(vec![1, 300, -1]);
    let mut encoded = Vec::new();
    packet.encode_payload(&mut encoded);
    assert_eq!(encoded, payload);
    assert_eq!(
        RemoveEntitiesPacket::decode_payload(&payload).unwrap(),
        packet
    );

    let mut framed = vec![0x0A, 0x4E];
    framed.extend(payload);
    assert_eq!(packet.encode(), framed);
    assert_eq!(RemoveEntitiesPacket::decode(&framed).unwrap(), packet);

    assert_eq!(
        RemoveEntitiesPacket::decode_payload(&[0x02, 0x01]),
        Err(EntityProtocolError::UnexpectedEof)
    );
    // A hostile count fails on the missing data without a huge reservation.
    assert_eq!(
        RemoveEntitiesPacket::decode_payload(&[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
        Err(EntityProtocolError::UnexpectedEof)
    );
}

#[test]
fn test_set_entity_data_uses_26_3_metadata_type_ids() {
    let cases: Vec<(MetadataValue, Vec<u8>)> = vec![
        (MetadataValue::Byte(-12), vec![0x00, 0xF4]),
        (MetadataValue::VarInt(300), vec![0x01, 0xAC, 0x02]),
        (
            MetadataValue::Float(1.0),
            vec![0x03, 0x3F, 0x80, 0x00, 0x00],
        ),
        (
            MetadataValue::String("ab".into()),
            vec![0x04, 0x02, b'a', b'b'],
        ),
        (
            MetadataValue::Chat("ab".into()),
            vec![0x05, 0x08, 0x00, 0x02, b'a', b'b'],
        ),
        (MetadataValue::OptChat(None), vec![0x06, 0x00]),
        (
            MetadataValue::OptChat(Some("\u{1F600}".into())),
            vec![
                0x06, 0x01, 0x08, 0x00, 0x06, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80,
            ],
        ),
        (MetadataValue::Slot(SlotData::empty()), vec![0x07, 0x00]),
        (
            MetadataValue::Slot(SlotData::item(276, 3)),
            vec![0x07, 0x03, 0x94, 0x02, 0x00, 0x00],
        ),
        (MetadataValue::Bool(true), vec![0x08, 0x01]),
        (
            MetadataValue::Rotations([1.0, 0.0, -2.0]),
            vec![
                0x09, 0x3F, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00,
            ],
        ),
        (MetadataValue::OptPosition(None), vec![0x0B, 0x00]),
        (
            // x=1 at bit 38, z=2 at bit 12, y=3 at bit 0.
            MetadataValue::OptPosition(Some([1, 3, 2])),
            vec![0x0B, 0x01, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x20, 0x03],
        ),
        (MetadataValue::Direction(direction::NORTH), vec![0x0C, 0x02]),
        (MetadataValue::OptUUID(None), vec![0x0D, 0x00]),
        (MetadataValue::BlockState(88), vec![0x0E, 0x58]),
        (
            MetadataValue::VillagerData {
                villager_type: 2,
                profession: 9,
                level: 1,
            },
            vec![0x12, 0x02, 0x09, 0x01],
        ),
        (MetadataValue::OptVarInt(None), vec![0x13, 0x00]),
        (MetadataValue::OptVarInt(Some(41)), vec![0x13, 0x2A]),
        (MetadataValue::Pose(pose::SNEAKING), vec![0x14, 0x05]),
    ];

    for (value, wire) in cases {
        let mut payload = vec![0x05, 0x02]; // Entity 5, index 2.
        payload.extend(&wire);
        payload.push(0xFF);

        let packet = SetEntityDataPacket::new(5, vec![EntityMetadataEntry::new(2, value)]);
        let mut encoded = Vec::new();
        packet.encode_payload(&mut encoded).unwrap();
        assert_eq!(encoded, payload);
        assert_eq!(
            SetEntityDataPacket::decode_payload(&payload).unwrap(),
            packet
        );

        let mut framed = vec![payload.len() as u8 + 1, 0x65];
        framed.extend(&payload);
        assert_eq!(packet.encode().unwrap(), framed);
        assert_eq!(SetEntityDataPacket::decode(&framed).unwrap(), packet);
    }
}

#[test]
fn test_metadata_rejects_what_it_cannot_represent() {
    assert!(SetEntityDataPacket::decode_payload(&[0x05, 0x02, 0x08, 0x01]).is_err());

    // VarLong, Particle and later types are not misread as neighbouring ones.
    for type_id in [2, 10, 15, 16, 17, 21, 43] {
        assert_eq!(
            MetadataValue::decode(type_id, &[0; 16]),
            Err(EntityProtocolError::UnknownTypeId(type_id))
        );
    }

    // A compound text component is unsupported, never truncated.
    assert_eq!(
        MetadataValue::decode(5, &[0x0A, 0x00]),
        Err(EntityProtocolError::UnsupportedTextComponent(0x0A))
    );
    // A slot with data components cannot be represented.
    assert!(MetadataValue::decode(7, &[0x01, 0x05, 0x01, 0x00]).is_err());
    assert_eq!(
        MetadataValue::decode(8, &[0x02]),
        Err(EntityProtocolError::InvalidBoolean(0x02))
    );
    assert_eq!(
        MetadataValue::decode(5, &[0x08, 0x00, 0x04, 0xF0, 0x9F, 0x98, 0x80]),
        Err(EntityProtocolError::InvalidUtf8)
    );

    let mut out = Vec::new();
    assert!(
        MetadataValue::OptVarInt(Some(i32::MAX))
            .encode(&mut out)
            .is_err()
    );
    assert!(
        MetadataValue::Slot(SlotData::item(1, -1))
            .encode(&mut out)
            .is_err()
    );
    assert!(
        MetadataValue::Chat("a".repeat(65536))
            .encode(&mut out)
            .is_err()
    );
    assert!(
        MetadataValue::String("a".repeat(32768))
            .encode(&mut out)
            .is_err()
    );
}

#[test]
fn test_entity_visual_tracker_enter_and_leave_radius() {
    let mut tracker = EntityVisualTracker::new();

    let uuid1 = [1u8; 16];
    let uuid2 = [2u8; 16];

    // Register entity 1 at (10.0, 64.0, 10.0)
    let entity1 = TrackedEntityState::new(EntitySpawnFields {
        entity_id: 1,
        uuid: uuid1,
        entity_type: 20,
        position: [10.0, 64.0, 10.0],
        pitch: 0,
        yaw: 0,
        head_yaw: 0,
        data: 0,
    })
    .unwrap();

    // Register entity 2 at (100.0, 64.0, 100.0)
    let entity2 = TrackedEntityState::new(EntitySpawnFields {
        entity_id: 2,
        uuid: uuid2,
        entity_type: 21,
        position: [100.0, 64.0, 100.0],
        pitch: 0,
        yaw: 0,
        head_yaw: 0,
        data: 0,
    })
    .unwrap();

    tracker.add_entity(entity1).unwrap();
    tracker.add_entity(entity2).unwrap();

    // Player joins at (0.0, 64.0, 0.0) with view radius 32.0 blocks
    // Entity 1 is at dist ~14.14 <= 32 -> should spawn
    // Entity 2 is at dist ~141.42 > 32 -> should not spawn
    let events = tracker.add_player(100, [0.0, 64.0, 0.0], 32.0).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        EntityTrackerEvent::Spawn {
            spawn_packet,
            data_packet,
        } => {
            assert_eq!(spawn_packet.entity_id, 1);
            assert_eq!(data_packet.entity_id, 1);
        }
        _ => panic!("Expected Spawn event"),
    }
    assert!(tracker.is_entity_visible_to_player(100, 1));
    assert!(!tracker.is_entity_visible_to_player(100, 2));

    // Player moves closer to entity 2: to (90.0, 64.0, 90.0)
    // Entity 1 is now at dist ~113.1 > 32 -> should despawn
    // Entity 2 is now at dist ~14.14 <= 32 -> should spawn
    let move_events = tracker
        .update_player_position(100, [90.0, 64.0, 90.0])
        .unwrap();
    assert_eq!(move_events.len(), 2);

    let mut spawned = None;
    let mut despawned = None;
    for ev in move_events {
        match ev {
            EntityTrackerEvent::Spawn { spawn_packet, .. } => {
                spawned = Some(spawn_packet.entity_id);
            }
            EntityTrackerEvent::Despawn(packet) => {
                despawned = Some(packet.entity_ids);
            }
        }
    }
    assert_eq!(spawned, Some(2));
    assert_eq!(despawned, Some(vec![1]));

    assert!(!tracker.is_entity_visible_to_player(100, 1));
    assert!(tracker.is_entity_visible_to_player(100, 2));

    // Entity 2 moves away to (200.0, 64.0, 200.0) -> should despawn for player 100
    let entity_move_events = tracker
        .update_entity_position(2, [200.0, 64.0, 200.0])
        .unwrap();
    assert_eq!(entity_move_events.len(), 1);
    assert_eq!(entity_move_events[0].0, 100);
    assert!(entity_move_events[0].1.is_despawn());
    assert!(!tracker.is_entity_visible_to_player(100, 2));

    // Entity 2 moves back into range -> should spawn for player 100
    let entity_move_back = tracker
        .update_entity_position(2, [95.0, 64.0, 95.0])
        .unwrap();
    assert_eq!(entity_move_back.len(), 1);
    assert_eq!(entity_move_back[0].0, 100);
    assert!(entity_move_back[0].1.is_spawn());
    assert!(tracker.is_entity_visible_to_player(100, 2));

    // Metadata update
    let new_meta = vec![EntityMetadataEntry::new(
        0,
        MetadataValue::Pose(pose::SLEEPING),
    )];
    let meta_events = tracker.update_entity_metadata(2, new_meta).unwrap();
    assert_eq!(meta_events.len(), 1);
    assert_eq!(meta_events[0].0, 100);
    assert_eq!(meta_events[0].1.entity_id, 2);

    // Remove entity from world -> despawn packet
    let despawns = tracker.remove_entity(2).unwrap();
    assert_eq!(despawns.len(), 1);
    assert_eq!(despawns[0].0, 100);
    assert_eq!(despawns[0].1.entity_ids, vec![2]);
    assert_eq!(tracker.entity_count(), 1);

    // Remove player -> returns None since visible set is now empty
    assert!(tracker.remove_player(100).is_none());
    assert_eq!(tracker.player_count(), 0);
}

#[test]
fn test_tracker_multiple_players_and_radius_expansion() {
    let mut tracker = EntityVisualTracker::new();

    let uuid = [0x55; 16];
    let entity = TrackedEntityState::new(EntitySpawnFields {
        entity_id: 10,
        uuid,
        entity_type: 1,
        position: [0.0, 0.0, 0.0],
        pitch: 0,
        yaw: 0,
        head_yaw: 0,
        data: 0,
    })
    .unwrap();
    tracker.add_entity(entity).unwrap();

    // Player A at (20.0, 0.0, 0.0) with radius 15.0 -> out of range
    let events_a = tracker.add_player(1, [20.0, 0.0, 0.0], 15.0).unwrap();
    assert!(events_a.is_empty());
    assert!(!tracker.is_entity_visible_to_player(1, 10));

    // Player B at (20.0, 0.0, 0.0) with radius 25.0 -> in range
    let events_b = tracker.add_player(2, [20.0, 0.0, 0.0], 25.0).unwrap();
    assert_eq!(events_b.len(), 1);
    assert!(tracker.is_entity_visible_to_player(2, 10));

    // Increase Player A view radius to 25.0 -> entity 10 enters range!
    let expand_events = tracker.update_player_radius(1, 25.0).unwrap();
    assert_eq!(expand_events.len(), 1);
    assert!(expand_events[0].is_spawn());
    assert!(tracker.is_entity_visible_to_player(1, 10));

    // Decrease Player B view radius to 10.0 -> entity 10 leaves range!
    let shrink_events = tracker.update_player_radius(2, 10.0).unwrap();
    assert_eq!(shrink_events.len(), 1);
    assert!(shrink_events[0].is_despawn());
    assert!(!tracker.is_entity_visible_to_player(2, 10));
}

#[test]
fn test_varint_rejects_non_canonical_nonzero() {
    assert_eq!(
        read_varint(&[0x81, 0x00]),
        Err(EntityProtocolError::MalformedVarInt)
    );
}

#[test]
fn test_read_text_component_rejects_overlong_modified_utf8() {
    assert_eq!(
        read_text_component(&[8, 0, 2, 0xC1, 0x81]),
        Err(EntityProtocolError::InvalidUtf8)
    );
}
