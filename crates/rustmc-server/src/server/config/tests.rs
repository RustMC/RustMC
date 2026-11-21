//! Unit tests for `server::config`.

use super::*;

fn parse(text: &str) -> Result<ServerConfig, String> {
    parse_server(&toml::from_str::<toml::Value>(text).expect("toml"))
}

#[test]
fn defaults_follow_vanilla_server_properties() {
    let config = parse("registry_manifest = \"registries.toml\"\nregistry_table = \"table.json\"\nregistries_report = \"registries.json\"")
        .expect("config");
    assert_eq!(config.address.port(), 25565);
    assert!(config.online_mode);
    assert_eq!(config.max_players, 20);
    assert_eq!(config.compression_threshold, Some(256));
    assert_eq!(config.view_distance, 10);
    assert_eq!(config.game_mode, GameMode::Survival);
    assert_eq!(config.motd, "A Minecraft Server");
    assert_eq!(config.world_dir, PathBuf::from("world"));
    assert_eq!(config.autosave_interval_seconds, 300);
}

#[test]
fn every_key_parses() {
    let config = parse(
        r#"
        registry_manifest = "r.toml"
        registry_table = "t.json"
        registries_report = "r.json"
        seed = -5
        generation_workers = 3
        world_dir = "saves/main"
        autosave_interval_seconds = 60
        bind_address = "127.0.0.1"
        port = 25570
        online_mode = false
        max_players = 4
        motd = "Hello"
        network_compression_threshold = -1
        view_distance = 6
        game_mode = "creative"
        session_server = "http://127.0.0.1:9"
        max_packet_bytes = 65536
        read_bytes_per_second = 65536
        login_timeout_ms = 5000
        outbound_queue_packets = 128
        "#,
    )
    .expect("config");
    assert_eq!(config.address.to_string(), "127.0.0.1:25570");
    assert!(!config.online_mode);
    assert_eq!(config.compression_threshold, None);
    assert_eq!(config.game_mode, GameMode::Creative);
    assert_eq!(config.outbound_queue_packets, 128);
    assert_eq!(config.seed, -5);
    assert_eq!(config.generation_workers, 3);
    assert_eq!(config.world_dir, PathBuf::from("saves/main"));
    assert_eq!(config.autosave_interval_seconds, 60);
}

#[test]
fn invalid_values_are_rejected_with_the_key_name() {
    for (text, key) in [
        ("", "registry_manifest"),
        ("registry_manifest = \"r\"", "registry_table"),
        (
            "registry_manifest = \"r\"\nregistry_table = \"t\"",
            "registries_report",
        ),
        (
            "registry_manifest = \"r\"\nregistry_table = \"t\"\nregistries_report = \"j\"\ngeneration_workers = 0",
            "generation_workers",
        ),
        (
            "registry_manifest = \"r\"\nregistry_table = \"t\"\nregistries_report = \"j\"\nmax_players = 0",
            "max_players",
        ),
        (
            "registry_manifest = \"r\"\nregistry_table = \"t\"\nregistries_report = \"j\"\nview_distance = 40",
            "view_distance",
        ),
        (
            "registry_manifest = \"r\"\nregistry_table = \"t\"\nregistries_report = \"j\"\ngame_mode = \"god\"",
            "game_mode",
        ),
        (
            "registry_manifest = \"r\"\nregistry_table = \"t\"\nregistries_report = \"j\"\nbind_address = \"nowhere\"",
            "bind_address",
        ),
        (
            "registry_manifest = \"r\"\nregistry_table = \"t\"\nregistries_report = \"j\"\nsession_server = \"ftp://x\"",
            "session_server",
        ),
        (
            "registry_manifest = \"r\"\nregistry_table = \"t\"\nregistries_report = \"j\"\nunknown = 1",
            "unknown",
        ),
    ] {
        let error = parse(text).expect_err(text);
        assert!(error.contains(key), "{error}");
    }
}
