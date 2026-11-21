#![forbid(unsafe_code)]

//! RustMC configuration, discovery, and opt-in local Java terrain preview.
//! The preview is unauthenticated and has no authoritative gameplay world.

pub mod chunk_adapter;
pub mod commands;
pub mod discovery_bedrock;
pub mod discovery_java;
pub mod entity;
pub mod item;
pub mod java_packets;
pub mod java_preview;
pub mod java_wire;
pub mod net;
pub mod preview_cache;
pub mod preview_data;
pub mod protocol;
pub mod runtime;
pub mod server;
pub mod vanilla;
pub mod world;

use std::{net::IpAddr, ops::RangeInclusive, path::Path};

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
    /// Optional operator-local immutable packet cache; not a saved world.
    pub vanilla_cache_root: Option<std::path::PathBuf>,
    /// Parallel chunk builders for the opt-in vanilla preview.
    pub vanilla_generation_workers: usize,
    /// Lifetime of a preview connection once it reaches play.
    pub preview_session_lifetime_ms: u64,
    /// Client bytes read per second during play; excess input waits in the
    /// socket (TCP backpressure) instead of closing the session.
    pub preview_read_bytes_per_second: usize,
    /// Optional zlib compression threshold in bytes for the local Java preview.
    ///
    /// Absent or negative values (`-1`) disable compression per
    /// `https://minecraft.wiki/w/Java_Edition_protocol/Packets` (revision
    /// 3868170: "Negative values will disable compression, meaning the packet
    /// format should remain in the uncompressed packet format.") and
    /// `https://minecraft.wiki/w/Server.properties` (revision 3904780,
    /// `network-compression-threshold`: default `256`, `-1` disables
    /// compression, `0` compresses every packet).
    pub preview_compression_threshold: Option<usize>,
}

/// Allowed `listener.preview_session_lifetime_ms` values.
pub const PREVIEW_SESSION_LIFETIME_MS: RangeInclusive<u64> = 10_000..=3_600_000;
/// Allowed `listener.preview_read_bytes_per_second` values.
pub const PREVIEW_READ_BYTES_PER_SECOND: RangeInclusive<usize> = 1_024..=1_048_576;
/// Allowed `listener.max_connections`: each connection owns bounded buffers,
/// so this caps the listener's memory.
pub const MAX_CONNECTIONS: RangeInclusive<usize> = 1..=64;
/// Allowed `listener.max_bytes_per_connection` (the read budget of one
/// connection before it is closed).
pub const MAX_BYTES_PER_CONNECTION: RangeInclusive<usize> = 1..=65_536;
/// Allowed `listener.idle_timeout_ms`.
pub const IDLE_TIMEOUT_MS: RangeInclusive<u64> = 10..=60_000;
/// Allowed `listener.max_connection_lifetime_ms` when the preview is enabled.
pub const CONNECTION_LIFETIME_MS: RangeInclusive<u64> = 10..=3_600_000;
/// Longest connection a discovery-only listener keeps: status and ping
/// exchanges finish in well under a minute.
pub const DISCOVERY_CONNECTION_LIFETIME_MAX_MS: u64 = 60_000;
/// Allowed `listener.vanilla_generation_workers` (generator threads).
pub const VANILLA_GENERATION_WORKERS: RangeInclusive<usize> = 1..=20;
/// Allowed `listener.preview_view_distance` in chunks, the client's own
/// render-distance range.
pub const PREVIEW_VIEW_DISTANCE: RangeInclusive<u8> = 2..=32;

impl ListenerConfig {
    /// Longest connection lifetime this listener accepts.
    fn max_lifetime_ms(&self) -> u64 {
        if self.local_java_preview {
            *CONNECTION_LIFETIME_MS.end()
        } else {
            DISCOVERY_CONNECTION_LIFETIME_MAX_MS
        }
    }

    /// Checks the numeric limits and preview-only options, whether the value
    /// came from a file or was built in code.
    pub fn validate(&self) -> Result<(), String> {
        let lifetime = *CONNECTION_LIFETIME_MS.start()..=self.max_lifetime_ms();
        let checks = [
            (
                MAX_CONNECTIONS.contains(&self.max_connections),
                "`listener.max_connections` is out of range",
            ),
            (
                MAX_BYTES_PER_CONNECTION.contains(&self.max_bytes_per_connection),
                "`listener.max_bytes_per_connection` is out of range",
            ),
            (
                IDLE_TIMEOUT_MS.contains(&self.idle_timeout_ms),
                "`listener.idle_timeout_ms` is out of range",
            ),
            (
                lifetime.contains(&self.max_connection_lifetime_ms),
                "`listener.max_connection_lifetime_ms` is out of range",
            ),
            (
                self.idle_timeout_ms <= self.max_connection_lifetime_ms,
                "`listener.idle_timeout_ms` cannot exceed `listener.max_connection_lifetime_ms`",
            ),
            (
                PREVIEW_VIEW_DISTANCE.contains(&self.preview_view_distance),
                "`listener.preview_view_distance` is out of range",
            ),
            (
                VANILLA_GENERATION_WORKERS.contains(&self.vanilla_generation_workers),
                "`listener.vanilla_generation_workers` is out of range",
            ),
            (
                PREVIEW_SESSION_LIFETIME_MS.contains(&self.preview_session_lifetime_ms),
                "`listener.preview_session_lifetime_ms` is out of range",
            ),
            (
                PREVIEW_READ_BYTES_PER_SECOND.contains(&self.preview_read_bytes_per_second),
                "`listener.preview_read_bytes_per_second` is out of range",
            ),
            (
                self.local_java_preview
                    || (self.preview_registry_manifest.is_none()
                        && self.preview_compression_threshold.is_none()),
                "preview options require `listener.local_java_preview = true`",
            ),
            (
                self.preview_compression_threshold
                    .is_none_or(|threshold| i32::try_from(threshold).is_ok()),
                "`listener.preview_compression_threshold` is out of range",
            ),
        ];
        match checks.into_iter().find(|(ok, _)| !ok) {
            Some((_, message)) => Err(message.to_owned()),
            None => Ok(()),
        }
    }
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
            vanilla_cache_root: None,
            vanilla_generation_workers: 8,
            preview_seed: 0,
            preview_view_distance: 4,
            preview_terrain: world::Terrain::Preview,
            preview_session_lifetime_ms: 600_000,
            preview_read_bytes_per_second: 65_536,
            preview_compression_threshold: None,
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
                | "vanilla_cache_root"
                | "vanilla_generation_workers"
                | "preview_session_lifetime_ms"
                | "preview_read_bytes_per_second"
                | "preview_compression_threshold"
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
        *IDLE_TIMEOUT_MS.start(),
        *IDLE_TIMEOUT_MS.end(),
    )?;
    let max_connection_lifetime_ms = integer_field(
        table,
        "max_connection_lifetime_ms",
        defaults.max_connection_lifetime_ms,
        *CONNECTION_LIFETIME_MS.start(),
        *CONNECTION_LIFETIME_MS.end(),
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
    if !local_java_preview && max_connection_lifetime_ms > DISCOVERY_CONNECTION_LIFETIME_MAX_MS {
        return Err(
            "`listener.max_connection_lifetime_ms` exceeds the discovery-only limit".to_owned(),
        );
    }
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
    let vanilla_cache_root = local_path("vanilla_cache_root")?;
    if vanilla_data_root.is_some() != vanilla_registry_table.is_some() {
        return Err("`listener.vanilla_data_root` and `listener.vanilla_registry_table` must be set together".to_owned());
    }
    if vanilla_cache_root.is_some() && vanilla_data_root.is_none() {
        return Err("`listener.vanilla_cache_root` requires vanilla data paths".to_owned());
    }
    if table.contains_key("vanilla_generation_workers") && vanilla_data_root.is_none() {
        return Err("`listener.vanilla_generation_workers` requires vanilla data paths".to_owned());
    }
    if !local_java_preview
        && let Some(key) = [
            "preview_registry_manifest",
            "preview_seed",
            "preview_view_distance",
            "preview_terrain",
            "preview_session_lifetime_ms",
            "preview_read_bytes_per_second",
            "preview_compression_threshold",
            "vanilla_data_root",
            "vanilla_registry_table",
        ]
        .into_iter()
        .find(|key| table.contains_key(*key))
    {
        return Err(format!(
            "`listener.{key}` requires `listener.local_java_preview = true`"
        ));
    }
    if vanilla_data_root.is_some() && preview_registry_manifest.is_none() {
        return Err("vanilla preview requires `listener.preview_registry_manifest`".to_owned());
    }
    if vanilla_data_root.is_some()
        && table
            .get("max_connections")
            .and_then(toml::Value::as_integer)
            .unwrap_or(defaults.max_connections as i64)
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
        port: integer_field(
            table,
            "port",
            u64::from(defaults.port),
            0,
            u64::from(u16::MAX),
        )? as u16,
        max_connections: integer_field(
            table,
            "max_connections",
            defaults.max_connections as u64,
            *MAX_CONNECTIONS.start() as u64,
            *MAX_CONNECTIONS.end() as u64,
        )? as usize,
        max_bytes_per_connection: integer_field(
            table,
            "max_bytes_per_connection",
            defaults.max_bytes_per_connection as u64,
            *MAX_BYTES_PER_CONNECTION.start() as u64,
            *MAX_BYTES_PER_CONNECTION.end() as u64,
        )? as usize,
        idle_timeout_ms,
        max_connection_lifetime_ms,
        local_java_preview,
        preview_registry_manifest,
        vanilla_data_root,
        vanilla_registry_table,
        vanilla_cache_root,
        vanilla_generation_workers: integer_field(
            table,
            "vanilla_generation_workers",
            defaults.vanilla_generation_workers as u64,
            *VANILLA_GENERATION_WORKERS.start() as u64,
            *VANILLA_GENERATION_WORKERS.end() as u64,
        )? as usize,
        preview_seed: preview_seed(table)?,
        preview_view_distance: integer_field(
            table,
            "preview_view_distance",
            defaults.preview_view_distance.into(),
            (*PREVIEW_VIEW_DISTANCE.start()).into(),
            (*PREVIEW_VIEW_DISTANCE.end()).into(),
        )? as u8,
        preview_terrain,
        preview_session_lifetime_ms: integer_field(
            table,
            "preview_session_lifetime_ms",
            defaults.preview_session_lifetime_ms,
            *PREVIEW_SESSION_LIFETIME_MS.start(),
            *PREVIEW_SESSION_LIFETIME_MS.end(),
        )?,
        preview_read_bytes_per_second: integer_field(
            table,
            "preview_read_bytes_per_second",
            defaults.preview_read_bytes_per_second as u64,
            *PREVIEW_READ_BYTES_PER_SECOND.start() as u64,
            *PREVIEW_READ_BYTES_PER_SECOND.end() as u64,
        )? as usize,
        preview_compression_threshold: compression_threshold(table)?,
    })
}

/// Parses `listener.preview_compression_threshold`: absent or negative disables
/// compression (`None`); non-negative values enable zlib compression at that
/// byte threshold (up to `i32::MAX`, the protocol `VarInt` range).
fn compression_threshold(
    table: &toml::map::Map<String, toml::Value>,
) -> Result<Option<usize>, String> {
    let Some(value) = table.get("preview_compression_threshold") else {
        return Ok(None);
    };
    let Some(number) = value.as_integer().and_then(|n| i32::try_from(n).ok()) else {
        return Err(
            "`listener.preview_compression_threshold` must be a signed 32-bit integer".to_owned(),
        );
    };
    Ok((number >= 0).then_some(number as usize))
}

/// World seeds use the full signed 64-bit range, like vanilla seeds. The value
/// is stored as its two's-complement bit pattern; non-negative seeds keep
/// their previous preview terrain.
fn preview_seed(table: &toml::map::Map<String, toml::Value>) -> Result<u64, String> {
    match table.get("preview_seed") {
        None => Ok(ListenerConfig::default().preview_seed),
        Some(value) => value
            .as_integer()
            .map(|seed| seed as u64)
            .ok_or_else(|| "`listener.preview_seed` must be a 64-bit integer".to_owned()),
    }
}

/// Parse and validate a configuration document without exposing raw values in errors.
pub fn parse_config(input: &str) -> Result<Config, String> {
    let value: toml::Value =
        toml::from_str(input).map_err(|_| "malformed TOML; check syntax and quoting".to_owned())?;
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
mod tests;
