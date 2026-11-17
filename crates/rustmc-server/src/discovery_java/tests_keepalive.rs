use super::keepalive::*;

#[test]
fn test_keepalive_challenge_response_success() {
    let mut tracker = KeepAliveTracker::new(300);
    assert!(!tracker.has_pending_challenge());

    // Issue challenge at tick 100
    let challenge_id = 9876543210_i64;
    assert_eq!(
        tracker.issue_challenge(100, challenge_id).unwrap(),
        challenge_id
    );
    assert!(tracker.has_pending_challenge());
    assert_eq!(tracker.pending_challenge_id(), Some(challenge_id));

    // Client responds at tick 104 (4 ticks = 200 ms latency)
    let ping = tracker.handle_response(104, challenge_id).unwrap();
    assert_eq!(ping, 200);
    assert_eq!(tracker.last_ping_ms(), Some(200));
    assert!(!tracker.has_pending_challenge());
}

#[test]
fn test_duplicate_in_flight_challenge_rejected() {
    let mut tracker = KeepAliveTracker::new(300);
    tracker.issue_challenge(100, 1111).unwrap();

    // Issuing a second challenge while the first is in-flight must fail
    let err = tracker.issue_challenge(105, 2222).unwrap_err();
    assert_eq!(err, KeepAliveError::ChallengeAlreadyPending);
}

#[test]
fn test_mismatched_challenge_rejected() {
    let mut tracker = KeepAliveTracker::new(300);
    tracker.issue_challenge(100, 1111).unwrap();

    // Client responds with wrong challenge ID
    let err = tracker.handle_response(102, 9999).unwrap_err();
    assert_eq!(
        err,
        KeepAliveError::MismatchedChallenge {
            expected: 1111,
            received: 9999
        }
    );
}

#[test]
fn test_keepalive_timeout_detection() {
    let mut tracker = KeepAliveTracker::new(300); // 300 ticks timeout
    tracker.issue_challenge(100, 12345).unwrap();

    // At tick 399: not timed out yet (299 ticks elapsed)
    assert!(!tracker.is_timed_out(399));

    // At tick 401: timed out (301 ticks elapsed > 300)
    assert!(tracker.is_timed_out(401));

    // Responding after timeout threshold fails with TimedOut
    let err = tracker.handle_response(401, 12345).unwrap_err();
    assert_eq!(err, KeepAliveError::TimedOut);
}

#[test]
fn login_disconnect_carries_a_json_text_component() {
    let packet = DisconnectPayload::encode(DisconnectPhase::Login, "Bye").unwrap();
    let json = br#"{"text":"Bye"}"#;
    // Frame length, login_disconnect 0, string length, JSON.
    let mut expected = vec![(2 + json.len()) as u8, 0, json.len() as u8];
    expected.extend_from_slice(json);
    assert_eq!(packet, expected);
}

#[test]
fn configuration_and_play_disconnects_carry_an_nbt_string() {
    for (phase, id) in [
        (DisconnectPhase::Configuration, 2),
        (DisconnectPhase::Play, 32),
    ] {
        let packet = DisconnectPayload::encode(phase, "Bye").unwrap();
        // Frame length, packet ID, TAG_String, u16 length 3, "Bye".
        assert_eq!(packet, [7, id, 8, 0, 3, b'B', b'y', b'e']);
    }
}

#[test]
fn nbt_reasons_use_modified_utf8() {
    let packet = DisconnectPayload::encode(DisconnectPhase::Play, "a\0\u{1F600}").unwrap();
    // 'a', NUL as C0 80, and U+1F600 as surrogates D83D DE00 in three bytes each.
    assert_eq!(
        &packet[3..],
        &[0, 9, b'a', 0xc0, 0x80, 0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80]
    );
    let too_long = "x".repeat(65_536);
    assert_eq!(
        DisconnectPayload::encode(DisconnectPhase::Play, &too_long),
        Err(DisconnectError::ReasonTooLong)
    );
    assert!(DisconnectPayload::encode(DisconnectPhase::Login, &too_long).is_ok());
}

#[test]
fn json_reasons_escape_every_control_character() {
    assert_eq!(
        DisconnectPayload::format_reason_json("Timed out: connection dropped"),
        "{\"text\":\"Timed out: connection dropped\"}"
    );
    assert_eq!(
        DisconnectPayload::format_reason_json("Hello \"World\"\nNext\\line"),
        "{\"text\":\"Hello \\\"World\\\"\\nNext\\\\line\"}"
    );
    assert_eq!(
        DisconnectPayload::format_reason_json("\0\u{8}\u{c}\u{1f}"),
        "{\"text\":\"\\u0000\\u0008\\u000c\\u001f\"}"
    );
}
