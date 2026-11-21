//! Unit tests for `net::configuration`.

use super::*;

#[test]
fn known_pack_offer_round_trips_through_the_recognizer() {
    let offer = known_packs();
    assert_eq!(offer.id, clientbound::SELECT_KNOWN_PACKS);
    assert_eq!(recognizes_core_pack(&offer.body), Ok(true));
    let mut other = Vec::new();
    put_varint(1, &mut other);
    for text in ["minecraft", "core", "0.0"] {
        put_string(text, &mut other);
    }
    assert_eq!(recognizes_core_pack(&other), Ok(false));
}

#[test]
fn known_pack_replies_are_bounded() {
    let mut many = Vec::new();
    put_varint(MAX_KNOWN_PACKS + 1, &mut many);
    assert_eq!(recognizes_core_pack(&many), Err(PackListError::TooMany));
    assert_eq!(recognizes_core_pack(&[1]), Err(PackListError::Malformed));
    let mut trailing = known_packs().body;
    trailing.push(0);
    assert_eq!(
        recognizes_core_pack(&trailing),
        Err(PackListError::Malformed)
    );
}

#[test]
fn configuration_without_a_manifest_still_finishes() {
    let packets = initial_configuration(None);
    let ids: Vec<u32> = packets.iter().map(|p| p.id).collect();
    assert_eq!(
        ids,
        [
            clientbound::UPDATE_ENABLED_FEATURES,
            clientbound::UPDATE_TAGS,
            clientbound::FINISH_CONFIGURATION
        ]
    );
    assert_eq!(packets[1].body, [0]);
}
