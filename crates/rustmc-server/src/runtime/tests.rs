//! Unit tests for `runtime`.

use super::*;

#[test]
fn broadcast_queue_marks_a_recipient_that_would_pass_the_write_bound() {
    let mut pending = vec![0; MAX_PENDING_BYTES - 2];
    let mut overflow = false;
    queue_broadcast(&mut pending, &mut overflow, &[1, 2]);
    assert_eq!(pending.len(), MAX_PENDING_BYTES);
    assert!(!overflow);
    queue_broadcast(&mut pending, &mut overflow, &[3]);
    assert_eq!(pending.len(), MAX_PENDING_BYTES);
    assert!(overflow);
}

#[test]
fn prebind_shutdown_never_opens_socket() {
    let flag = AtomicBool::new(true);
    let mut events = Vec::new();
    run_listener(&ListenerConfig::default(), &flag, Instant::now(), |event| {
        events.push(event)
    })
    .unwrap();
    assert_eq!(
        events.iter().map(|e| e.kind).collect::<Vec<_>>(),
        ["starting", "stopping", "stopped"]
    );
}

#[test]
fn direct_invalid_limits_fail_before_binding() {
    let config = ListenerConfig {
        max_connections: usize::MAX,
        ..ListenerConfig::default()
    };
    let flag = AtomicBool::new(false);
    let mut events = Vec::new();
    let error =
        run_listener(&config, &flag, Instant::now(), |event| events.push(event)).unwrap_err();
    assert_eq!(error.exit_code(), 4);
    assert!(
        events
            .iter()
            .any(|event| event.reason == Some("invalid_limits"))
    );
}

fn loopback_connection() -> (Connection, TcpStream) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (stream, _) = listener.accept().unwrap();
    let connection = Connection {
        stream,
        discovery: discovery_java::Session::default(),
        pending: Vec::new(),
        bytes_read: 0,
        last_activity: Instant::now(),
        created: Instant::now(),
        pending_chunk_since: None,
        write_overflow: false,
        rate_window: Instant::now(),
        rate_window_bytes: 0,
        play_started: None,
    };
    (connection, client)
}

#[test]
fn pre_play_reads_are_capped_for_the_whole_session() {
    let config = ListenerConfig::default();
    let (mut connection, _client) = loopback_connection();
    assert_eq!(connection.read_allowance(&config, false), (4096, 4096));
    connection.bytes_read = 4000;
    assert_eq!(connection.read_allowance(&config, false), (96, 4096));
    connection.bytes_read = 4096;
    assert_eq!(connection.read_allowance(&config, false).0, 0);
}

#[test]
fn play_reads_pause_when_the_rate_window_is_spent_and_resume_after_it() {
    let config = ListenerConfig {
        preview_read_bytes_per_second: 5000,
        ..ListenerConfig::default()
    };
    let (mut connection, _client) = loopback_connection();
    // Total history does not matter during play; only the current window.
    connection.bytes_read = 10 * 1024 * 1024;
    assert_eq!(
        connection.read_allowance(&config, true),
        (READ_CHUNK, READ_CHUNK)
    );
    connection.rate_window_bytes = 4500;
    assert_eq!(connection.read_allowance(&config, true).0, 500);
    connection.rate_window_bytes = 5000;
    assert_eq!(connection.read_allowance(&config, true).0, 0);
    connection.rate_window = Instant::now() - READ_RATE_WINDOW;
    assert_eq!(connection.read_allowance(&config, true).0, READ_CHUNK);
    assert_eq!(connection.rate_window_bytes, 0);
}

#[test]
fn play_lifetime_counts_from_entering_play_not_from_accept() {
    let config = ListenerConfig {
        max_connection_lifetime_ms: 60_000,
        preview_session_lifetime_ms: 10_000,
        ..ListenerConfig::default()
    };
    let (mut connection, _client) = loopback_connection();
    // Login and configuration took 30 s: past the play limit, within the
    // pre-play limit.
    connection.created = Instant::now() - Duration::from_secs(30);
    assert!(!connection.lifetime_expired(&config, false));
    connection.play_started = Some(Instant::now());
    assert!(!connection.lifetime_expired(&config, true));
    connection.play_started = Some(Instant::now() - Duration::from_secs(10));
    assert!(connection.lifetime_expired(&config, true));
    // Before play, the accept-time limit still applies.
    connection.created = Instant::now() - Duration::from_secs(60);
    assert!(connection.lifetime_expired(&config, false));
}

#[test]
fn direct_invalid_play_limits_fail_before_binding() {
    for config in [
        ListenerConfig {
            preview_session_lifetime_ms: 0,
            ..ListenerConfig::default()
        },
        ListenerConfig {
            preview_read_bytes_per_second: usize::MAX,
            ..ListenerConfig::default()
        },
        ListenerConfig {
            preview_compression_threshold: Some(256),
            ..ListenerConfig::default()
        },
        ListenerConfig {
            local_java_preview: true,
            preview_compression_threshold: Some(usize::MAX),
            ..ListenerConfig::default()
        },
    ] {
        let flag = AtomicBool::new(false);
        let error = run_listener(&config, &flag, Instant::now(), |_| {}).unwrap_err();
        assert_eq!(error.exit_code(), 4);
    }
}

#[test]
fn runtime_failure_has_distinct_exit_code() {
    let error = RuntimeError::Io(io::Error::other("test failure"));
    assert_eq!(error.exit_code(), 5);
}

#[test]
fn occupied_port_fails_with_startup_code() {
    let occupied = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let config = ListenerConfig {
        port: occupied.local_addr().unwrap().port(),
        ..ListenerConfig::default()
    };
    let flag = AtomicBool::new(false);
    let mut events = Vec::new();
    let error =
        run_listener(&config, &flag, Instant::now(), |event| events.push(event)).unwrap_err();
    assert_eq!(error.exit_code(), 4);
    assert!(
        events
            .iter()
            .any(|event| event.state == LifecycleState::Failed)
    );
}
