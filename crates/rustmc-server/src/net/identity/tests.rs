//! Unit tests for `net::identity`.

use super::*;

#[test]
fn offline_uuid_is_a_version_3_ietf_uuid_of_the_prefixed_name() {
    let uuid = offline_uuid("Notch");
    assert_eq!(uuid[6] >> 4, 3, "version nibble");
    assert_eq!(uuid[8] >> 6, 0b10, "IETF variant");
    let mut expected: [u8; 16] = Md5::digest(b"OfflinePlayer:Notch").into();
    expected[6] = (expected[6] & 0x0f) | 0x30;
    expected[8] = (expected[8] & 0x3f) | 0x80;
    assert_eq!(uuid, expected);
    assert_eq!(uuid_text(&uuid), "b50ad385-829d-3141-a216-7e7d7539ba7f");
    assert_ne!(offline_uuid("notch"), uuid, "names are case-sensitive");
}

#[test]
fn names_are_ascii_word_characters_up_to_sixteen() {
    assert!(is_valid_name("Steve_01"));
    assert!(is_valid_name("abcdefghijklmnop"));
    assert!(!is_valid_name(""));
    assert!(!is_valid_name("abcdefghijklmnopq"));
    assert!(!is_valid_name("has space"));
    assert!(!is_valid_name("é"));
}

#[test]
fn uuid_text_round_trips_with_and_without_dashes() {
    let uuid = offline_uuid("Alex");
    assert_eq!(parse_uuid(&uuid_text(&uuid)), Some(uuid));
    assert_eq!(parse_uuid(&uuid_text(&uuid).replace('-', "")), Some(uuid));
    assert_eq!(parse_uuid("not-a-uuid"), None);
}

#[test]
fn profile_encoding_matches_the_game_profile_layout() {
    let profile = GameProfile {
        uuid: [1; 16],
        name: "Ab".to_owned(),
        properties: vec![
            ProfileProperty {
                name: "textures".to_owned(),
                value: "v".to_owned(),
                signature: Some("s".to_owned()),
            },
            ProfileProperty {
                name: "x".to_owned(),
                value: "y".to_owned(),
                signature: None,
            },
        ],
    };
    let mut out = Vec::new();
    profile.encode(&mut out);
    let mut expected = vec![1; 16];
    expected.extend([2, b'A', b'b', 2]);
    expected.extend([8]);
    expected.extend(b"textures");
    expected.extend([1, b'v', 1, 1, b's']);
    expected.extend([1, b'x', 1, b'y', 0]);
    assert_eq!(out, expected);
}
