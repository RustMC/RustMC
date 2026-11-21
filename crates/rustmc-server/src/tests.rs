//! Unit tests for `lib`.

use super::*;

#[test]
fn accepts_supported_values_and_loopback_defaults() {
    let parsed = parse_config("schema_version = 1\nlog_level = 'trace'\n").unwrap();
    assert_eq!(parsed.log_level, LogLevel::Trace);
    assert!(parsed.listener.bind_address.is_loopback());
    assert_eq!(parsed.listener.port, 0);
}

#[test]
fn rejects_schema_and_fields() {
    assert!(
        parse_config("schema_version = 2\nlog_level = 'info'")
            .unwrap_err()
            .contains("unsupported")
    );
    assert!(
        parse_config("schema_version = 1\nlog_level = 'info'\nsecret = 'x'")
            .unwrap_err()
            .contains("unknown")
    );
    assert!(
        parse_config("schema_version = 1\nlog_level = 'info'\n[listener]\nextra = 1")
            .unwrap_err()
            .contains("unknown")
    );
}

#[test]
fn rejects_nonlocal_and_conflicting_address() {
    for address in ["0.0.0.0", "192.0.2.1", "127.0.0.1:25565"] {
        let input = format!(
            "schema_version = 1\nlog_level = 'info'\n[listener]\nbind_address = '{address}'\n"
        );
        assert!(parse_config(&input).unwrap_err().contains("bind_address"));
    }
}

#[test]
fn rejects_invalid_limits_and_timeout() {
    for (field, value) in [
        ("port", "65536"),
        ("port", "-1"),
        ("max_connections", "0"),
        ("max_connections", "65"),
        ("max_bytes_per_connection", "0"),
        ("max_bytes_per_connection", "65537"),
        ("idle_timeout_ms", "9"),
        ("idle_timeout_ms", "60001"),
        ("max_connection_lifetime_ms", "9"),
        ("max_connection_lifetime_ms", "60001"),
    ] {
        let input =
            format!("schema_version = 1\nlog_level = 'info'\n[listener]\n{field} = {value}\n");
        assert!(parse_config(&input).unwrap_err().contains(field));
    }
}

#[test]
fn local_preview_allows_a_bounded_longer_session_for_large_views() {
    let config = parse_config("schema_version = 1\nlog_level = 'info'\n[listener]\nlocal_java_preview = true\nmax_connection_lifetime_ms = 3600000\npreview_view_distance = 32\n").unwrap();
    assert_eq!(config.listener.max_connection_lifetime_ms, 3_600_000);
    assert_eq!(config.listener.preview_view_distance, 32);
    assert!(parse_config("schema_version = 1\nlog_level = 'info'\n[listener]\nmax_connection_lifetime_ms = 3600000\n").is_err());
}

#[test]
fn rejects_conflicting_timeouts() {
    let input = "schema_version = 1\nlog_level = 'info'\n[listener]\nidle_timeout_ms = 200\nmax_connection_lifetime_ms = 100\n";
    assert!(parse_config(input).unwrap_err().contains("cannot exceed"));
}

#[test]
fn does_not_echo_invalid_value() {
    let error = parse_config("schema_version = 1\nlog_level = 'private-token'").unwrap_err();
    assert!(!error.contains("private-token"));
}

#[test]
fn preview_requires_explicit_boolean_and_stays_on_loopback() {
    let config = parse_config("schema_version = 1\nlog_level = 'info'\n").unwrap();
    assert!(!config.listener.local_java_preview);
    let config = parse_config(
        "schema_version = 1\nlog_level = 'info'\n[listener]\nlocal_java_preview = true\n",
    )
    .unwrap();
    assert!(config.listener.local_java_preview);
    assert!(config.listener.bind_address.is_loopback());
    assert!(
        parse_config(
            "schema_version = 1\nlog_level = 'info'\n[listener]\nlocal_java_preview = 'yes'\n"
        )
        .unwrap_err()
        .contains("local_java_preview")
    );
    assert!(parse_config("schema_version = 1\nlog_level = 'info'\n[listener]\nbind_address = '0.0.0.0'\nlocal_java_preview = true\n").is_err());
}

#[test]
fn preview_terrain_defaults_to_preview_and_opts_in_experimental() {
    let base = "schema_version = 1\nlog_level = 'info'\n[listener]\nlocal_java_preview = true\n";
    assert_eq!(
        parse_config(base).unwrap().listener.preview_terrain,
        world::Terrain::Preview
    );
    assert_eq!(
        parse_config(&format!("{base}preview_terrain = 'experimental'\n"))
            .unwrap()
            .listener
            .preview_terrain,
        world::Terrain::Experimental
    );
    // Unknown names are rejected without echoing the value.
    let error = parse_config(&format!("{base}preview_terrain = 'vanilla'\n")).unwrap_err();
    assert!(error.contains("preview_terrain"));
    assert!(!error.contains("vanilla"));
    // Setting terrain without the preview opt-in stays rejected.
    assert!(
        parse_config(
            "schema_version = 1\nlog_level = 'info'\n[listener]\npreview_terrain = 'experimental'\n"
        )
        .unwrap_err()
        .contains("local_java_preview")
    );
}

#[test]
fn preview_play_limits_default_validate_and_require_the_preview() {
    let base = "schema_version = 1\nlog_level = 'info'\n[listener]\nlocal_java_preview = true\n";
    let defaults = parse_config(base).unwrap().listener;
    assert_eq!(defaults.preview_session_lifetime_ms, 600_000);
    assert_eq!(defaults.preview_read_bytes_per_second, 65_536);
    let custom = parse_config(&format!(
        "{base}preview_session_lifetime_ms = 3600000\npreview_read_bytes_per_second = 1024\n"
    ))
    .unwrap()
    .listener;
    assert_eq!(custom.preview_session_lifetime_ms, 3_600_000);
    assert_eq!(custom.preview_read_bytes_per_second, 1024);
    for (field, value) in [
        ("preview_session_lifetime_ms", "9999"),
        ("preview_session_lifetime_ms", "3600001"),
        ("preview_read_bytes_per_second", "1023"),
        ("preview_read_bytes_per_second", "1048577"),
    ] {
        let error = parse_config(&format!("{base}{field} = {value}\n")).unwrap_err();
        assert!(error.contains(field), "{field}={value}: {error}");
    }
    for field in [
        "preview_session_lifetime_ms",
        "preview_read_bytes_per_second",
    ] {
        let error = parse_config(&format!(
            "schema_version = 1\nlog_level = 'info'\n[listener]\n{field} = 60000\n"
        ))
        .unwrap_err();
        assert!(error.contains(field) && error.contains("local_java_preview"));
    }
}

#[test]
fn preview_seed_accepts_the_signed_vanilla_range() {
    let base = "schema_version = 1\nlog_level = 'info'\n[listener]\nlocal_java_preview = true\n";
    for seed in [i64::MIN, -1, 0, 2026, i64::MAX] {
        let config = parse_config(&format!("{base}preview_seed = {seed}\n")).unwrap();
        assert_eq!(config.listener.preview_seed as i64, seed);
    }
    assert!(
        parse_config(&format!("{base}preview_seed = 'x'\n"))
            .unwrap_err()
            .contains("preview_seed")
    );
}

#[test]
fn vanilla_preview_requires_complete_local_input_and_one_connection() {
    let base = "schema_version = 1\nlog_level = 'info'\n[listener]\nlocal_java_preview = true\nmax_connections = 1\npreview_registry_manifest = 'manifest.toml'\n";
    assert!(parse_config(&format!("{base}vanilla_data_root = 'data'\n")).is_err());
    let complete =
        format!("{base}vanilla_data_root = 'data'\nvanilla_registry_table = 'ids.json'\n");
    let config = parse_config(&complete).unwrap();
    assert!(config.listener.vanilla_cache_root.is_none());
    assert_eq!(config.listener.vanilla_generation_workers, 8);
    assert_eq!(
        parse_config(&format!("{complete}vanilla_generation_workers = 16\n"))
            .unwrap()
            .listener
            .vanilla_generation_workers,
        16
    );
    for workers in [0, 21] {
        assert!(
            parse_config(&format!(
                "{complete}vanilla_generation_workers = {workers}\n"
            ))
            .is_err()
        );
    }
    assert!(parse_config(&format!("{base}vanilla_generation_workers = 16\n")).is_err());
    let cached = parse_config(&format!("{complete}vanilla_cache_root = 'cache'\n")).unwrap();
    assert_eq!(
        cached.listener.vanilla_cache_root.as_deref(),
        Some(Path::new("cache"))
    );
    assert!(parse_config(&format!("{base}vanilla_cache_root = 'cache'\n")).is_err());
    assert_eq!(
        config.listener.vanilla_data_root.as_deref(),
        Some(Path::new("data"))
    );
    assert!(parse_config(&complete.replace("max_connections = 1", "max_connections = 2")).is_err());
    assert!(parse_config(&format!("{complete}preview_terrain = 'experimental'\n")).is_err());
}

#[test]
fn preview_compression_threshold_validates_signed_varint_and_requires_preview() {
    let base = "schema_version = 1\nlog_level = 'info'\n[listener]\nlocal_java_preview = true\n";
    assert_eq!(
        parse_config(base)
            .unwrap()
            .listener
            .preview_compression_threshold,
        None
    );
    for off in [-1, -2, i32::MIN] {
        let config =
            parse_config(&format!("{base}preview_compression_threshold = {off}\n")).unwrap();
        assert_eq!(config.listener.preview_compression_threshold, None);
    }
    for on in [0, 256, i32::MAX] {
        let config =
            parse_config(&format!("{base}preview_compression_threshold = {on}\n")).unwrap();
        assert_eq!(
            config.listener.preview_compression_threshold,
            Some(on as usize)
        );
    }
    for bad in ["'256'", "true", "2147483648", "-2147483649"] {
        let err =
            parse_config(&format!("{base}preview_compression_threshold = {bad}\n")).unwrap_err();
        assert!(
            err.contains("preview_compression_threshold"),
            "{bad}: {err}"
        );
    }
    let err = parse_config(
        "schema_version = 1\nlog_level = 'info'\n[listener]\npreview_compression_threshold = 256\n",
    )
    .unwrap_err();
    assert!(err.contains("preview_compression_threshold") && err.contains("local_java_preview"));
}

#[test]
fn listener_validation_rejects_out_of_range_limits_built_in_code() {
    assert_eq!(ListenerConfig::default().validate(), Ok(()));
    let too_many = ListenerConfig {
        max_connections: *MAX_CONNECTIONS.end() + 1,
        ..ListenerConfig::default()
    };
    assert_eq!(
        too_many.validate(),
        Err("`listener.max_connections` is out of range".to_owned())
    );
    let long_discovery = ListenerConfig {
        max_connection_lifetime_ms: DISCOVERY_CONNECTION_LIFETIME_MAX_MS + 1,
        ..ListenerConfig::default()
    };
    assert!(long_discovery.validate().is_err());
    let long_preview = ListenerConfig {
        local_java_preview: true,
        ..long_discovery
    };
    assert_eq!(long_preview.validate(), Ok(()));
    let compression_without_preview = ListenerConfig {
        preview_compression_threshold: Some(256),
        ..ListenerConfig::default()
    };
    assert_eq!(
        compression_without_preview.validate(),
        Err("preview options require `listener.local_java_preview = true`".to_owned())
    );
}
