//! Unit tests for `server::join`.

use super::*;
use crate::java_wire::{read_string, read_varint};

fn manifest() -> RegistryManifest {
    RegistryManifest {
        tags: Vec::new(),
        registries: vec![
            (
                "minecraft:dimension_type".to_owned(),
                vec!["minecraft:the_end".to_owned(), OVERWORLD.to_owned()],
            ),
            (
                "minecraft:world_clock".to_owned(),
                vec![OVERWORLD.to_owned()],
            ),
        ],
    }
}

fn spawn() -> Spawn {
    Spawn {
        entity_id: 7,
        game_mode: GameMode::Creative,
        position: [0.5, 70.0, -15.5],
        yaw: 90.0,
        pitch: 0.0,
        teleport_id: 1,
        view_distance: 10,
        max_players: 20,
        online_mode: true,
        day_time: 1000,
        world_age: 5,
    }
}

#[test]
fn login_lays_out_every_field_in_order() {
    let packet = login(&spawn(), &manifest()).expect("login");
    let body = packet.body.as_slice();
    assert_eq!(body[..4], 7i32.to_be_bytes());
    assert_eq!(body[4], 0, "not hardcore");
    let (count, n) = read_varint(&body[5..]).unwrap().unwrap();
    assert_eq!(count, 1);
    let (name, rest) = read_string(&body[5 + n..], 64).unwrap();
    assert_eq!(name, OVERWORLD);
    assert_eq!(rest[..6], [20, 10, 10, 0, 1, 0]);
    assert_eq!(rest[6], 1, "dimension type id of the overworld");
    let (dimension, rest) = read_string(&rest[7..], 64).unwrap();
    assert_eq!(dimension, OVERWORLD);
    assert_eq!(rest[..8], [0; 8], "hashed seed");
    assert_eq!(
        rest[8..],
        [1, 0, 0, 0, 0, 0, SEA_LEVEL as u8, 1, 0],
        "mode, previous, debug, flat, death, portal, sea level, online, secure chat"
    );
}

#[test]
fn missing_registry_entries_are_reported() {
    let empty = RegistryManifest {
        tags: Vec::new(),
        registries: Vec::new(),
    };
    assert_eq!(
        login(&spawn(), &empty),
        Err(JoinError::MissingRegistryEntry(
            "minecraft:dimension_type",
            OVERWORLD
        ))
    );
}

#[test]
fn abilities_follow_the_game_mode() {
    assert_eq!(abilities(GameMode::Survival).body[0], 0);
    assert_eq!(
        abilities(GameMode::Creative).body[0],
        ability::INVULNERABLE | ability::ALLOW_FLYING | ability::INSTANT_BREAK
    );
    assert_eq!(GameMode::from_name("spectator"), Some(GameMode::Spectator));
    assert_eq!(GameMode::from_name("hardcore"), None);
}

#[test]
fn time_runs_the_overworld_clock_at_normal_speed() {
    let packet = time(&spawn(), &manifest()).expect("time");
    let mut expected = 5i64.to_be_bytes().to_vec();
    expected.extend([1, 0, 0xe8, 0x07]);
    expected.extend(0f32.to_be_bytes());
    expected.extend(1f32.to_be_bytes());
    assert_eq!(packet.body, expected);
}

#[test]
fn join_sequence_ends_with_the_first_teleport() {
    let packets = join_packets(&spawn(), &manifest()).expect("join");
    let ids: Vec<u32> = packets.iter().map(|p| p.id).collect();
    assert_eq!(
        ids,
        [
            clientbound::LOGIN,
            clientbound::PLAYER_ABILITIES,
            clientbound::SET_TIME,
            clientbound::SET_CHUNK_CACHE_CENTER,
            clientbound::GAME_EVENT,
            clientbound::PLAYER_POSITION
        ]
    );
    assert_eq!(
        packets[3].body,
        [0, 0xff, 0xff, 0xff, 0xff, 0x0f],
        "chunk (0, -1)"
    );
    assert_eq!(packets[5].body.len(), 1 + 6 * 8 + 2 * 4 + 4);
}
