#![forbid(unsafe_code)]

//! RustMC configuration, discovery, and opt-in local Java terrain preview.
//! The preview is unauthenticated and has no authoritative gameplay world.

pub mod chunk_adapter;
pub mod discovery_bedrock;
pub mod discovery_java;
pub mod java_preview;
pub mod preview_data;
pub mod runtime;
pub mod vanilla;
pub mod world;

use std::{net::IpAddr, path::Path};

/// Configuration schema supported by the bootstrap and development CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub schema_version: u32,
    pub log_level: LogLevel,
    pub listener: ListenerConfig,
}

/// Diagnostic verbosity. Lifecycle control events are always emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    /// Whether connection-level diagnostics should be emitted.
    pub fn allows_connection_events(self) -> bool {
        matches!(self, Self::Debug | Self::Trace)
    }
}

/// Bounded, loopback-only development listener settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerConfig {
    pub bind_address: IpAddr,
    pub port: u16,
    pub max_connections: usize,
    pub max_bytes_per_connection: usize,
    pub idle_timeout_ms: u64,
    pub max_connection_lifetime_ms: u64,
    /// Explicitly allow an unauthenticated Java login experiment on loopback.
    pub local_java_preview: bool,
    /// Local identifier-only registry manifest used only by the preview path.
    pub preview_seed: u64,
    pub preview_view_distance: u8,
    /// Terrain field for the local preview; defaults to the accepted preview.
    pub preview_terrain: world::Terrain,
    pub preview_registry_manifest: Option<std::path::PathBuf>,
    /// Operator-local 26.3 worldgen data and protocol IDs; both required together.
    pub vanilla_data_root: Option<std::path::PathBuf>,
    pub vanilla_registry_table: Option<std::path::PathBuf>,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            bind_address: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: 0,
            max_connections: 8,
            max_bytes_per_connection: 4096,
            idle_timeout_ms: 1000,
            max_connection_lifetime_ms: 10000,
            local_java_preview: false,
            preview_registry_manifest: None,
            vanilla_data_root: None,
            vanilla_registry_table: None,
            preview_seed: 0,
            preview_view_distance: 4,
            preview_terrain: world::Terrain::Preview,
        }
    }
}

fn integer_field(
    table: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, String> {
    let Some(value) = table.get(key) else {
        return Ok(default);
    };
    let Some(number) = value.as_integer().and_then(|n| u64::try_from(n).ok()) else {
        return Err(format!(
            "`listener.{key}` must be an integer from {min} to {max}"
        ));
    };
    if !(min..=max).contains(&number) {
        return Err(format!("`listener.{key}` must be from {min} to {max}"));
    }
    Ok(number)
}

fn parse_listener(value: Option<&toml::Value>) -> Result<ListenerConfig, String> {
    let defaults = ListenerConfig::default();
    let Some(value) = value else {
        return Ok(defaults);
    };
    let table = value
        .as_table()
        .ok_or_else(|| "`listener` must be a TOML table".to_owned())?;
    for key in table.keys() {
        if !matches!(
            key.as_str(),
            "bind_address"
                | "port"
                | "max_connections"
                | "max_bytes_per_connection"
                | "idle_timeout_ms"
                | "max_connection_lifetime_ms"
                | "local_java_preview"
                | "preview_registry_manifest"
                | "preview_seed"
                | "preview_view_distance"
                | "preview_terrain"
                | "vanilla_data_root"
                | "vanilla_registry_table"
        ) {
            return Err(format!("unknown `listener` field `{key}`"));
        }
    }
    let bind_address = match table.get("bind_address") {
        None => defaults.bind_address,
        Some(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| "`listener.bind_address` must be a loopback IP string".to_owned())?;
            let address: IpAddr = text.parse().map_err(|_| {
                "`listener.bind_address` must be a loopback IP without a port; set `listener.port` separately".to_owned()
            })?;
            if !address.is_loopback() {
                return Err("`listener.bind_address` must be a loopback IP in M1".to_owned());
            }
            address
        }
    };
    let idle_timeout_ms = integer_field(
        table,
        "idle_timeout_ms",
        defaults.idle_timeout_ms,
        10,
        60000,
    )?;
    let max_connection_lifetime_ms = integer_field(
        table,
        "max_connection_lifetime_ms",
        defaults.max_connection_lifetime_ms,
        10,
        60000,
    )?;
    if idle_timeout_ms > max_connection_lifetime_ms {
        return Err(
            "`listener.idle_timeout_ms` cannot exceed `listener.max_connection_lifetime_ms`"
                .to_owned(),
        );
    }
    let local_java_preview = match table.get("local_java_preview") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "`listener.local_java_preview` must be true or false".to_owned())?,
    };
    let preview_registry_manifest = match table.get("preview_registry_manifest") {
        None => None,
        Some(value) => Some(std::path::PathBuf::from(value.as_str().ok_or_else(
            || "`listener.preview_registry_manifest` must be a file path string".to_owned(),
        )?)),
    };
    let local_path = |key: &str| -> Result<Option<std::path::PathBuf>, String> {
        table
            .get(key)
            .map(|value| {
                value
                    .as_str()
                    .filter(|path| !path.is_empty())
                    .map(std::path::PathBuf::from)
                    .ok_or_else(|| format!("`listener.{key}` must be a nonempty path string"))
            })
            .transpose()
    };
    let vanilla_data_root = local_path("vanilla_data_root")?;
    let vanilla_registry_table = local_path("vanilla_registry_table")?;
    if vanilla_data_root.is_some() != vanilla_registry_table.is_some() {
        return Err("`listener.vanilla_data_root` and `listener.vanilla_registry_table` must be set together".to_owned());
    }
    if (preview_registry_manifest.is_some()
        || table.contains_key("preview_seed")
        || table.contains_key("preview_view_distance")
        || table.contains_key("preview_terrain")
        || vanilla_data_root.is_some()
        || vanilla_registry_table.is_some())
        && !local_java_preview
    {
        return Err(
            "`listener.preview_registry_manifest` requires `listener.local_java_preview = true`"
                .to_owned(),
        );
    }
    if vanilla_data_root.is_some() && preview_registry_manifest.is_none() {
        return Err("vanilla preview requires `listener.preview_registry_manifest`".to_owned());
    }
    if vanilla_data_root.is_some()
        && table
            .get("max_connections")
            .and_then(toml::Value::as_integer)
            .unwrap_or(8)
            != 1
    {
        return Err("vanilla preview requires `listener.max_connections = 1`".to_owned());
    }
    let preview_terrain = match table.get("preview_terrain") {
        None => defaults.preview_terrain,
        Some(value) => match value.as_str() {
            Some("preview") => world::Terrain::Preview,
            Some("experimental") => world::Terrain::Experimental,
            _ => {
                return Err(
                    "`listener.preview_terrain` must be \"preview\" or \"experimental\"".to_owned(),
                );
            }
        },
    };
    if vanilla_data_root.is_some() && preview_terrain != world::Terrain::Preview {
        return Err("vanilla preview cannot be combined with experimental terrain".to_owned());
    }
    Ok(ListenerConfig {
        bind_address,
        port: integer_field(table, "port", u64::from(defaults.port), 0, 65535)? as u16,
        max_connections: integer_field(
            table,
            "max_connections",
            defaults.max_connections as u64,
            1,
            64,
        )? as usize,
        max_bytes_per_connection: integer_field(
            table,
            "max_bytes_per_connection",
            defaults.max_bytes_per_connection as u64,
            1,
            65536,
        )? as usize,
        idle_timeout_ms,
        max_connection_lifetime_ms,
        local_java_preview,
        preview_registry_manifest,
        vanilla_data_root,
        vanilla_registry_table,
        preview_seed: integer_field(
            table,
            "preview_seed",
            defaults.preview_seed,
            0,
            i64::MAX as u64,
        )?,
        preview_view_distance: integer_field(
            table,
            "preview_view_distance",
            defaults.preview_view_distance.into(),
            2,
            32,
        )? as u8,
        preview_terrain,
    })
}

/// Parse and validate a configuration document without exposing raw values in errors.
pub fn parse_config(input: &str) -> Result<Config, String> {
    let value: toml::Value = input
        .parse()
        .map_err(|_| "malformed TOML; check syntax and quoting".to_owned())?;
    let table = value
        .as_table()
        .ok_or_else(|| "configuration must be a TOML table".to_owned())?;
    for key in table.keys() {
        if key != "schema_version" && key != "log_level" && key != "listener" {
            return Err(format!("unknown configuration field `{key}`"));
        }
    }
    let version = table
        .get("schema_version")
        .ok_or_else(|| "missing required field `schema_version`".to_owned())?
        .as_integer()
        .ok_or_else(|| "`schema_version` must be the integer 1".to_owned())?;
    if version != 1 {
        return Err("unsupported `schema_version`; supported version is 1".to_owned());
    }
    let level = table
        .get("log_level")
        .ok_or_else(|| "missing required field `log_level`".to_owned())?
        .as_str()
        .ok_or_else(|| "`log_level` must be a string".to_owned())?;
    let log_level = match level {
        "error" => LogLevel::Error,
        "warn" => LogLevel::Warn,
        "info" => LogLevel::Info,
        "debug" => LogLevel::Debug,
        "trace" => LogLevel::Trace,
        _ => {
            return Err(
                "invalid `log_level`; choose error, warn, info, debug, or trace".to_owned(),
            );
        }
    };
    let listener = parse_listener(table.get("listener"))?;
    Ok(Config {
        schema_version: 1,
        log_level,
        listener,
    })
}

/// Read and validate a configuration file without writing to disk.
pub fn check_config(path: &Path) -> Result<Config, String> {
    let input = std::fs::read_to_string(path)
        .map_err(|error| format!("could not read configuration file: {error}"))?;
    parse_config(&input)
}

#[cfg(test)]
mod tests {
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
        let base =
            "schema_version = 1\nlog_level = 'info'\n[listener]\nlocal_java_preview = true\n";
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
        assert!(parse_config(
            "schema_version = 1\nlog_level = 'info'\n[listener]\npreview_terrain = 'experimental'\n"
        )
        .unwrap_err()
        .contains("local_java_preview"));
    }

    #[test]
    fn vanilla_preview_requires_complete_local_input_and_one_connection() {
        let base = "schema_version = 1\nlog_level = 'info'\n[listener]\nlocal_java_preview = true\nmax_connections = 1\npreview_registry_manifest = 'manifest.toml'\n";
        assert!(parse_config(&format!("{base}vanilla_data_root = 'data'\n")).is_err());
        let complete =
            format!("{base}vanilla_data_root = 'data'\nvanilla_registry_table = 'ids.json'\n");
        let config = parse_config(&complete).unwrap();
        assert_eq!(
            config.listener.vanilla_data_root.as_deref(),
            Some(Path::new("data"))
        );
        assert!(
            parse_config(&complete.replace("max_connections = 1", "max_connections = 2")).is_err()
        );
        assert!(parse_config(&format!("{complete}preview_terrain = 'experimental'\n")).is_err());
    }
}
