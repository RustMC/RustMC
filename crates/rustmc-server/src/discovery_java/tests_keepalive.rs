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
fn test_disconnect_packet_encoding() {
    let packet = DisconnectPayload::encode_disconnect_packet(0x00, "Server closed");
    assert!(!packet.is_empty());

    let json_text = DisconnectPayload::format_reason_json("Timed out: connection dropped");
    assert_eq!(json_text, "{\"text\":\"Timed out: connection dropped\"}");

    // Verify escaping
    let json_escaped = DisconnectPayload::format_reason_json("Hello \"World\"\nNext line");
    assert_eq!(
        json_escaped,
        "{\"text\":\"Hello \\\"World\\\"\\nNext line\"}"
    );
}
