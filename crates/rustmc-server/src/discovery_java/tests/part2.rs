//! Unit tests, part 2 of 3.

use super::*;

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

pub(super) fn test_protocol_ids() -> Arc<ProtocolIds> {
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
        },
        "minecraft:entity_type": { "entries": { "minecraft:player": { "protocol_id": 159 } } }
    }"#;
    Arc::new(
        ProtocolIds::from_json_str(
            std::path::Path::new("reports/registries.json"),
            SYNTHETIC_REGISTRIES,
        )
        .expect("synthetic protocol ids"),
    )
}

pub(super) fn test_manifest_with_enchantments() -> Arc<RegistryManifest> {
    let mut manifest = (*test_manifest()).clone();
    manifest.registries.push((
        "minecraft:enchantment".into(),
        vec!["minecraft:sharpness".into(), "minecraft:unbreaking".into()],
    ));
    Arc::new(manifest)
}

pub(super) fn enter_play_session_with_protocol_ids()
-> (Session, Arc<ProtocolIds>, DynamicRegistryIds) {
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

pub(super) fn creative_slot_frame(
    window_slot: i16,
    stack: &Option<crate::item::ItemStack>,
    ids: &ProtocolIds,
    dynamic_ids: &DynamicRegistryIds,
) -> Vec<u8> {
    let mut body = window_slot.to_be_bytes().to_vec();
    crate::protocol::slot::encode_slot(stack, ids, dynamic_ids, &mut body)
        .expect("encode creative slot");
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
