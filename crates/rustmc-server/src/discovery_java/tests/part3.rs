//! Unit tests, part 3 of 3: play-state sessions.

use super::*;

pub(super) fn test_manifest() -> Arc<RegistryManifest> {
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

pub(super) fn play_session() -> Session {
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

pub(super) fn frame_bytes(packet: &[u8]) -> Vec<u8> {
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

pub(super) fn enter_play_session() -> Session {
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

pub(super) fn teleport_confirm(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
    let mut body = vec![1u8];
    body.extend(x.to_be_bytes());
    body.extend(y.to_be_bytes());
    body.extend(z.to_be_bytes());
    body.extend(yaw.to_be_bytes());
    body.extend(pitch.to_be_bytes());
    body
}

pub(super) fn player_position(x: f64, y: f64, z: f64, on_ground: u8) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(x.to_be_bytes());
    body.extend(y.to_be_bytes());
    body.extend(z.to_be_bytes());
    body.push(on_ground);
    body
}

pub(super) fn player_pos_rot(
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    on_ground: u8,
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(x.to_be_bytes());
    body.extend(y.to_be_bytes());
    body.extend(z.to_be_bytes());
    body.extend(yaw.to_be_bytes());
    body.extend(pitch.to_be_bytes());
    body.push(on_ground);
    body
}

pub(super) fn player_rotation(yaw: f32, pitch: f32, on_ground: u8) -> Vec<u8> {
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
