//! Unit tests for `protocol::movement`.

use super::*;

#[test]
fn test_varint_roundtrip() {
    for val in [
        0,
        1,
        127,
        128,
        255,
        256,
        16383,
        16384,
        2097151,
        -1,
        -2147483648,
        2147483647,
    ] {
        let mut buf = Vec::new();
        write_varint(val as u32, &mut buf);
        let (decoded, bytes) = read_varint(&buf).unwrap();
        assert_eq!(decoded, val);
        assert_eq!(bytes, buf.len());
    }

    // Non-canonical VarInt rejection (e.g. 0 encoded with continuation bit: [0x80, 0x00])
    assert_eq!(
        read_varint(&[0x80, 0x00]),
        Err(MovementError::MalformedVarInt)
    );

    // VarInt overflow (> 5 bytes)
    assert_eq!(
        read_varint(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
        Err(MovementError::MalformedVarInt)
    );
}

#[test]
fn test_varint_rejects_non_canonical_nonzero() {
    assert_eq!(
        read_varint(&[0x81, 0x00]),
        Err(MovementError::MalformedVarInt)
    );
}

#[test]
fn test_decode_move_player_pos_valid_and_malformed() {
    // Valid packet
    let pkt = MovePlayerPos::with_collision(12.5, 64.0, -100.25, true, true);
    let encoded = pkt.encode();
    assert_eq!(encoded.len(), 25);
    let decoded = MovePlayerPos::decode(&encoded).unwrap();
    assert_eq!(decoded, pkt);

    // Valid with horizontal collision flag = 3
    let mut data = encoded.clone();
    data[24] = 3;
    let decoded = MovePlayerPos::decode(&data).unwrap();
    assert!(decoded.on_ground);
    assert!(decoded.horizontal_collision);

    // Reject NaN coordinates
    let mut nan_x = encoded.clone();
    nan_x[0..8].copy_from_slice(&f64::NAN.to_be_bytes());
    assert_eq!(
        MovePlayerPos::decode(&nan_x),
        Err(MovementError::NonFiniteCoordinate)
    );

    let mut nan_y = encoded.clone();
    nan_y[8..16].copy_from_slice(&f64::NAN.to_be_bytes());
    assert_eq!(
        MovePlayerPos::decode(&nan_y),
        Err(MovementError::NonFiniteCoordinate)
    );

    let mut nan_z = encoded.clone();
    nan_z[16..24].copy_from_slice(&f64::NAN.to_be_bytes());
    assert_eq!(
        MovePlayerPos::decode(&nan_z),
        Err(MovementError::NonFiniteCoordinate)
    );

    // Reject Infinity coordinates
    let mut inf_x = encoded.clone();
    inf_x[0..8].copy_from_slice(&f64::INFINITY.to_be_bytes());
    assert_eq!(
        MovePlayerPos::decode(&inf_x),
        Err(MovementError::NonFiniteCoordinate)
    );

    let mut neg_inf_y = encoded.clone();
    neg_inf_y[8..16].copy_from_slice(&f64::NEG_INFINITY.to_be_bytes());
    assert_eq!(
        MovePlayerPos::decode(&neg_inf_y),
        Err(MovementError::NonFiniteCoordinate)
    );

    // Reject invalid length
    assert!(matches!(
        MovePlayerPos::decode(&encoded[0..24]),
        Err(MovementError::InvalidPacketLength {
            expected: 25,
            received: 24
        })
    ));

    // Reject invalid flag bits (> 3)
    let mut invalid_flag = encoded.clone();
    invalid_flag[24] = 4;
    assert_eq!(
        MovePlayerPos::decode(&invalid_flag),
        Err(MovementError::InvalidFlags(4))
    );
    invalid_flag[24] = 255;
    assert_eq!(
        MovePlayerPos::decode(&invalid_flag),
        Err(MovementError::InvalidFlags(255))
    );
}

#[test]
fn test_decode_move_player_pos_rot_valid_normalization_and_malformed() {
    // Valid packet with yaw=450.0 (normalizes to 90.0) and pitch=120.0 (clamps to 90.0)
    let pkt = MovePlayerPosRot::new(0.0, 64.0, 0.0, 450.0, 120.0, false);
    let encoded = pkt.encode();
    assert_eq!(encoded.len(), 33);
    let decoded = MovePlayerPosRot::decode(&encoded).unwrap();
    assert_eq!(decoded.x, 0.0);
    assert_eq!(decoded.y, 64.0);
    assert_eq!(decoded.z, 0.0);
    assert!((decoded.yaw - 90.0).abs() < 1e-4);
    assert_eq!(decoded.pitch, 90.0);
    assert!(!decoded.on_ground);

    // Negative pitch clamped to -90.0
    let pkt_neg = MovePlayerPosRot::new(0.0, 64.0, 0.0, -190.0, -120.0, true);
    let decoded_neg = MovePlayerPosRot::decode(&pkt_neg.encode()).unwrap();
    assert!((decoded_neg.yaw - 170.0).abs() < 1e-4);
    assert_eq!(decoded_neg.pitch, -90.0);
    assert!(decoded_neg.on_ground);

    // Reject NaN coordinates and rotation
    let mut nan_buf = pkt.encode();
    nan_buf[0..8].copy_from_slice(&f64::NAN.to_be_bytes());
    assert_eq!(
        MovePlayerPosRot::decode(&nan_buf),
        Err(MovementError::NonFiniteCoordinate)
    );

    let mut nan_rot = pkt.encode();
    nan_rot[24..28].copy_from_slice(&f32::NAN.to_be_bytes());
    assert_eq!(
        MovePlayerPosRot::decode(&nan_rot),
        Err(MovementError::NonFiniteRotation)
    );

    let mut inf_rot = pkt.encode();
    inf_rot[28..32].copy_from_slice(&f32::INFINITY.to_be_bytes());
    assert_eq!(
        MovePlayerPosRot::decode(&inf_rot),
        Err(MovementError::NonFiniteRotation)
    );

    // Reject invalid packet length
    assert!(matches!(
        MovePlayerPosRot::decode(&pkt.encode()[0..32]),
        Err(MovementError::InvalidPacketLength {
            expected: 33,
            received: 32
        })
    ));

    // Reject invalid flag byte
    let mut invalid_flag = pkt.encode();
    invalid_flag[32] = 5;
    assert_eq!(
        MovePlayerPosRot::decode(&invalid_flag),
        Err(MovementError::InvalidFlags(5))
    );
}

#[test]
fn test_decode_move_player_rot_valid_and_malformed() {
    let pkt = MovePlayerRot::new(180.0, -45.0, true);
    let encoded = pkt.encode();
    assert_eq!(encoded.len(), 9);
    let decoded = MovePlayerRot::decode(&encoded).unwrap();
    assert_eq!(decoded.yaw, -180.0);
    assert_eq!(decoded.pitch, -45.0);
    assert!(decoded.on_ground);

    // Reject NaN / Inf
    let mut nan_yaw = encoded.clone();
    nan_yaw[0..4].copy_from_slice(&f32::NAN.to_be_bytes());
    assert_eq!(
        MovePlayerRot::decode(&nan_yaw),
        Err(MovementError::NonFiniteRotation)
    );

    let mut inf_pitch = encoded.clone();
    inf_pitch[4..8].copy_from_slice(&f32::NEG_INFINITY.to_be_bytes());
    assert_eq!(
        MovePlayerRot::decode(&inf_pitch),
        Err(MovementError::NonFiniteRotation)
    );

    // Reject invalid length
    assert!(matches!(
        MovePlayerRot::decode(&encoded[0..8]),
        Err(MovementError::InvalidPacketLength {
            expected: 9,
            received: 8
        })
    ));

    // Reject invalid flags
    let mut inv_flag = encoded.clone();
    inv_flag[8] = 8;
    assert_eq!(
        MovePlayerRot::decode(&inv_flag),
        Err(MovementError::InvalidFlags(8))
    );
}

#[test]
fn test_decode_move_player_status_valid_and_malformed() {
    for flag in [0, 1, 2, 3] {
        let decoded = MovePlayerStatus::decode(&[flag]).unwrap();
        assert_eq!(decoded.on_ground, (flag & 0x01) != 0);
        assert_eq!(decoded.horizontal_collision, (flag & 0x02) != 0);
    }

    // Reject length != 1
    assert!(matches!(
        MovePlayerStatus::decode(&[]),
        Err(MovementError::InvalidPacketLength {
            expected: 1,
            received: 0
        })
    ));
    assert!(matches!(
        MovePlayerStatus::decode(&[0, 1]),
        Err(MovementError::InvalidPacketLength {
            expected: 1,
            received: 2
        })
    ));

    // Reject invalid flags
    assert_eq!(
        MovePlayerStatus::decode(&[4]),
        Err(MovementError::InvalidFlags(4))
    );
    assert_eq!(
        MovePlayerStatus::decode(&[255]),
        Err(MovementError::InvalidFlags(255))
    );
}

#[test]
fn test_decode_accept_teleportation_valid_and_malformed() {
    let pkt = AcceptTeleportation::new(42);
    let encoded = pkt.encode();
    let decoded = AcceptTeleportation::decode(&encoded).unwrap();
    assert_eq!(decoded.teleport_id, 42);

    // Large VarInt ID
    let pkt_large = AcceptTeleportation::new(1048576);
    let decoded_large = AcceptTeleportation::decode(&pkt_large.encode()).unwrap();
    assert_eq!(decoded_large.teleport_id, 1048576);

    // Reject trailing unparsed bytes
    let mut trailing = encoded.clone();
    trailing.push(0x00);
    assert_eq!(
        AcceptTeleportation::decode(&trailing),
        Err(MovementError::TrailingData)
    );
}

#[test]
fn test_decode_serverbound_movement_packet_routing() {
    let pos = MovePlayerPos::new(10.0, 64.0, 20.0, true);
    let decoded =
        decode_serverbound_movement_packet(PACKET_ID_MOVE_PLAYER_POS, &pos.encode()).unwrap();
    assert_eq!(decoded, ServerboundMovementPacket::Pos(pos));

    let pos_rot = MovePlayerPosRot::new(1.0, 2.0, 3.0, 0.0, 0.0, false);
    let decoded =
        decode_serverbound_movement_packet(PACKET_ID_MOVE_PLAYER_POS_ROT, &pos_rot.encode())
            .unwrap();
    assert_eq!(decoded, ServerboundMovementPacket::PosRot(pos_rot));

    let rot = MovePlayerRot::new(45.0, 30.0, true);
    let decoded =
        decode_serverbound_movement_packet(PACKET_ID_MOVE_PLAYER_ROT, &rot.encode()).unwrap();
    assert_eq!(decoded, ServerboundMovementPacket::Rot(rot));

    let status = MovePlayerStatus::new(true);
    let decoded =
        decode_serverbound_movement_packet(PACKET_ID_MOVE_PLAYER_STATUS, &status.encode()).unwrap();
    assert_eq!(decoded, ServerboundMovementPacket::Status(status));

    let tp = AcceptTeleportation::new(5);
    let decoded =
        decode_serverbound_movement_packet(PACKET_ID_ACCEPT_TELEPORTATION, &tp.encode()).unwrap();
    assert_eq!(decoded, ServerboundMovementPacket::AcceptTeleport(tp));

    // Unknown packet ID
    assert_eq!(
        decode_serverbound_movement_packet(0xFF, &[]),
        Err(MovementError::UnknownPacketId(0xFF))
    );
}

#[test]
fn test_player_position_sync_serialization_and_framing() {
    let sync = PlayerPositionSync::with_velocity(PositionSyncParams {
        teleport_id: 7,
        x: 100.5,
        y: 65.0,
        z: -200.75,
        dx: 0.1,
        dy: -0.5,
        dz: 0.0,
        yaw: 90.0,
        pitch: -15.0,
        flags: position_sync_flags::RELATIVE_X | position_sync_flags::RELATIVE_Y_ROT,
    });

    // Verify framed round-trip
    let framed = sync.encode_framed();
    assert!(!framed.is_empty());
    let decoded = PlayerPositionSync::decode_framed(&framed).unwrap();
    assert_eq!(sync, decoded);

    // Verify body round-trip
    let mut body = Vec::new();
    sync.encode_body(&mut body);
    let decoded_body = PlayerPositionSync::decode_body(&body).unwrap();
    assert_eq!(sync, decoded_body);

    // Rejection of non-finite float in body
    let mut nan_body = body.clone();
    // offset to x is after VarInt (1 byte for 7)
    nan_body[1..9].copy_from_slice(&f64::NAN.to_be_bytes());
    assert_eq!(
        PlayerPositionSync::decode_body(&nan_body),
        Err(MovementError::NonFiniteFloat)
    );

    // Rejection of unexpected packet ID in frame
    let wrong_framed = frame_packet(0x3F, &body);
    assert_eq!(
        PlayerPositionSync::decode_framed(&wrong_framed),
        Err(MovementError::UnexpectedPacketId {
            expected: PACKET_ID_PLAYER_POSITION_SYNC,
            received: 0x3F,
        })
    );
}

#[test]
fn test_teleport_tracker_handshake_and_challenge_sequence() {
    let mut tracker = TeleportTracker::new();
    assert!(!tracker.has_pending_teleport());
    assert_eq!(tracker.last_confirmed_id(), None);

    // Sequential IDs
    let id1 = tracker.next_teleport_id();
    let id2 = tracker.next_teleport_id();
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);

    // Register teleport 3
    tracker.register_teleport(3, (50.0, 70.0, 50.0), 100);
    assert!(tracker.has_pending_teleport());
    assert_eq!(
        tracker.validate_movement_allowed(),
        Err(TeleportError::MovementWhilePending { pending_id: 3 })
    );

    // Rejection of unknown teleport ID (e.g. 99)
    assert_eq!(
        tracker.accept_teleport(99),
        Err(TeleportError::UnknownTeleportId { id: 99 })
    );
    assert!(tracker.has_pending_teleport());

    // Successful handshake confirmation
    let confirmed = tracker.accept_teleport(3).unwrap();
    assert_eq!(confirmed.teleport_id, 3);
    assert_eq!(confirmed.x, 50.0);
    assert_eq!(confirmed.y, 70.0);
    assert_eq!(confirmed.z, 50.0);
    assert_eq!(confirmed.tick, 100);

    assert!(!tracker.has_pending_teleport());
    assert_eq!(tracker.last_confirmed_id(), Some(3));
    assert!(tracker.validate_movement_allowed().is_ok());

    // Rejection of obsolete teleport ID (already confirmed ID 3)
    assert_eq!(
        tracker.accept_teleport(3),
        Err(TeleportError::ObsoleteTeleportId { id: 3, latest: 3 })
    );

    // Issue teleport 4 then teleport 5
    let id4 = tracker.issue_teleport((10.0, 64.0, 10.0), 105);
    let id5 = tracker.issue_teleport((20.0, 64.0, 20.0), 110);
    assert_eq!(id4, 3); // next_id continued from 3
    assert_eq!(id5, 4);

    // Attempting to confirm superseded id 3 when 4 is active is obsolete
    assert_eq!(
        tracker.accept_teleport(id4),
        Err(TeleportError::ObsoleteTeleportId {
            id: id4,
            latest: id5
        })
    );

    // Confirming latest id 4 clears all pending
    assert!(tracker.accept_teleport(id5).is_ok());
    assert!(!tracker.has_pending_teleport());
}

#[test]
fn test_rejection_of_movement_packets_while_teleport_is_pending() {
    let mut guard = MovementGuard::new(0.0, 64.0, 0.0);

    // Normal movement allowed before teleport
    let pos_pkt = MovePlayerPos::new(1.0, 64.0, 0.0, true);
    assert!(
        guard
            .handle_movement_packet(&ServerboundMovementPacket::Pos(pos_pkt))
            .is_ok()
    );
    assert_eq!(guard.position(), (1.0, 64.0, 0.0));

    // Trigger server teleport to (500.0, 70.0, 500.0)
    let sync_pkt = guard.trigger_teleport((500.0, 70.0, 500.0), 200);
    assert_eq!(sync_pkt.x, 500.0);
    assert!(guard.teleport_tracker().has_pending_teleport());

    // Client movement updates must be rejected while teleport is awaiting confirmation
    let move_pos = MovePlayerPos::new(2.0, 64.0, 0.0, true);
    assert_eq!(
        guard.handle_movement_packet(&ServerboundMovementPacket::Pos(move_pos)),
        Err(MovementValidationError::TeleportPending {
            pending_id: sync_pkt.teleport_id,
            expected: (500.0, 70.0, 500.0),
        })
    );

    let move_pos_rot = MovePlayerPosRot::new(2.0, 64.0, 0.0, 0.0, 0.0, true);
    assert_eq!(
        guard.handle_movement_packet(&ServerboundMovementPacket::PosRot(move_pos_rot)),
        Err(MovementValidationError::TeleportPending {
            pending_id: sync_pkt.teleport_id,
            expected: (500.0, 70.0, 500.0),
        })
    );

    let move_rot = MovePlayerRot::new(90.0, 0.0, true);
    assert_eq!(
        guard.handle_movement_packet(&ServerboundMovementPacket::Rot(move_rot)),
        Err(MovementValidationError::TeleportPending {
            pending_id: sync_pkt.teleport_id,
            expected: (500.0, 70.0, 500.0),
        })
    );

    let move_status = MovePlayerStatus::new(true);
    assert_eq!(
        guard.handle_movement_packet(&ServerboundMovementPacket::Status(move_status)),
        Err(MovementValidationError::TeleportPending {
            pending_id: sync_pkt.teleport_id,
            expected: (500.0, 70.0, 500.0),
        })
    );

    // Confirm teleport
    let accept = AcceptTeleportation::new(sync_pkt.teleport_id);
    let outcome = guard
        .handle_movement_packet(&ServerboundMovementPacket::AcceptTeleport(accept))
        .unwrap();
    assert!(matches!(outcome, MovementOutcome::TeleportConfirmed(_)));
    assert_eq!(guard.position(), (500.0, 70.0, 500.0));

    // Movement updates allowed again after confirmation
    let move_after = MovePlayerPos::new(500.5, 70.0, 500.0, true);
    assert!(
        guard
            .handle_movement_packet(&ServerboundMovementPacket::Pos(move_after))
            .is_ok()
    );
    assert_eq!(guard.position(), (500.5, 70.0, 500.0));
}

#[test]
fn test_enforcement_of_maximum_displacement_threshold() {
    let mut guard = MovementGuard::new(0.0, 64.0, 0.0);

    // Movement within single-tick threshold (e.g. 10.0 blocks <= 100.0)
    let valid_move = MovePlayerPos::new(10.0, 64.0, 0.0, true);
    let outcome = guard
        .handle_movement_packet(&ServerboundMovementPacket::Pos(valid_move))
        .unwrap();
    assert_eq!(
        outcome,
        MovementOutcome::PositionUpdated {
            x: 10.0,
            y: 64.0,
            z: 0.0,
            displacement: 10.0,
        }
    );
    assert_eq!(guard.position(), (10.0, 64.0, 0.0));
    assert_eq!(guard.accumulated_tick_displacement(), 10.0);

    // Advance tick resets accumulated displacement
    guard.advance_tick();
    assert_eq!(guard.accumulated_tick_displacement(), 0.0);

    // Movement exceeding single-tick threshold (> 100.0 blocks, e.g. 10.0 -> 120.0 = 110 blocks)
    let illegal_move = MovePlayerPos::new(120.0, 64.0, 0.0, true);
    let err = guard
        .handle_movement_packet(&ServerboundMovementPacket::Pos(illegal_move))
        .unwrap_err();

    match err {
        MovementValidationError::SpeedExceeded {
            displacement,
            max_allowed,
            reset_to,
        } => {
            assert!((displacement - 110.0).abs() < 1e-4);
            assert_eq!(max_allowed, 100.0);
            assert_eq!(reset_to, (10.0, 64.0, 0.0));
        }
        other => panic!("expected SpeedExceeded, got {other:?}"),
    }

    // Multi-packet fragmentation within a single tick is bounded:
    guard.advance_tick();
    // First packet in tick: +60 blocks (allowed)
    let step1 = MovePlayerPos::new(70.0, 64.0, 0.0, true);
    guard
        .handle_movement_packet(&ServerboundMovementPacket::Pos(step1))
        .unwrap();
    assert_eq!(guard.accumulated_tick_displacement(), 60.0);

    // Second packet in same tick: +50 blocks (60 + 50 = 110 > 100 -> rejected)
    let step2 = MovePlayerPos::new(120.0, 64.0, 0.0, true);
    let err2 = guard
        .handle_movement_packet(&ServerboundMovementPacket::Pos(step2))
        .unwrap_err();
    match err2 {
        MovementValidationError::SpeedExceeded { displacement, .. } => {
            assert!((displacement - 110.0).abs() < 1e-4);
        }
        other => panic!("expected SpeedExceeded, got {other:?}"),
    }

    // Non-finite coordinates and rotations are rejected at public entry points
    assert_eq!(
        guard.validate_and_apply_pos(f64::NAN, 64.0, 0.0, true),
        Err(MovementValidationError::NonFiniteCoordinate)
    );
    assert_eq!(
        guard.validate_and_apply_pos_rot(10.0, 64.0, 0.0, f32::INFINITY, 0.0, true),
        Err(MovementValidationError::NonFiniteRotation)
    );
    assert_eq!(
        guard.validate_and_apply_rot(0.0, f32::NAN, true),
        Err(MovementValidationError::NonFiniteRotation)
    );

    // Authoritative position must remain at the pre-violation coordinate
    assert_eq!(guard.position(), (70.0, 64.0, 0.0));

    // Server can generate an authoritative reset packet
    let reset_sync = guard.create_position_sync_reset(300);
    assert_eq!(reset_sync.x, 70.0);
    assert_eq!(reset_sync.y, 64.0);
    assert_eq!(reset_sync.z, 0.0);

    // Client acknowledges the reset teleport with full 33-byte 26.3 confirmation
    let full_confirm =
        AcceptTeleportation::with_confirmation(reset_sync.teleport_id, 70.0, 64.0, 0.0, 0.0, 0.0);
    guard
        .handle_movement_packet(&ServerboundMovementPacket::AcceptTeleport(full_confirm))
        .unwrap();
    assert_eq!(guard.position(), (70.0, 64.0, 0.0));
}
