//! The `[server]` configuration table of the playable server.
//!
//! Why: running a shared world needs its own settings (address, online mode,
//! player limit, MOTD, compression, view distance) and connection limits.
//! Done: parsing with defaults and named bounds; key names follow vanilla's
//! `server.properties` (`online-mode` becomes `online_mode`). Not done: a
//! literal `server.properties` file (planned with the server features track).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::path::PathBuf;

use super::join::GameMode;
use crate::java_wire::MAX_PACKET_WIRE_BYTES;
use crate::net::session_server::MOJANG_SESSION_SERVER;

/// Vanilla's default `server-port`.
pub const DEFAULT_PORT: u16 = 25565;
/// Vanilla's default `max-players`.
pub const DEFAULT_MAX_PLAYERS: usize = 20;
/// Vanilla's default `network-compression-threshold`.
pub const DEFAULT_COMPRESSION_THRESHOLD: usize = 256;
/// Vanilla's default `view-distance`.
pub const DEFAULT_VIEW_DISTANCE: u8 = 10;
/// Vanilla's default `motd`.
pub const DEFAULT_MOTD: &str = "A Minecraft Server";
/// Default `read_bytes_per_second`: far above what a playing client sends,
/// low enough that one connection cannot monopolise a reader thread.
pub const DEFAULT_READ_BYTES_PER_SECOND: usize = 1_048_576;
/// Default `login_timeout_ms`, generous for a session-server round trip.
pub const DEFAULT_LOGIN_TIMEOUT_MS: u64 = 30_000;
/// Default `outbound_queue_packets`: room for a full view-distance chunk
/// burst plus entity updates.
pub const DEFAULT_OUTBOUND_QUEUE_PACKETS: usize = 4_096;

/// Allowed `max_players`: each player owns bounded queues and a reader and
/// writer thread, so this caps memory and threads.
pub const MAX_PLAYERS: RangeInclusive<usize> = 1..=1_000;
/// Allowed `view_distance` in chunks, the client's own range.
pub const VIEW_DISTANCE: RangeInclusive<u8> = 2..=32;
/// Allowed `max_packet_bytes` (one serverbound packet after decompression).
pub const MAX_PACKET_BYTES: RangeInclusive<usize> = 1_024..=MAX_PACKET_WIRE_BYTES;
/// Allowed `read_bytes_per_second` per connection.
pub const READ_BYTES_PER_SECOND: RangeInclusive<usize> = 4_096..=16_777_216;
/// Allowed `login_timeout_ms`: how long a connection may take from first
/// byte to joining the world.
pub const LOGIN_TIMEOUT_MS: RangeInclusive<u64> = 1_000..=120_000;
/// Allowed `outbound_queue_packets`: packets waiting for one slow client
/// before it is disconnected.
pub const OUTBOUND_QUEUE_PACKETS: RangeInclusive<usize> = 64..=65_536;
/// Allowed `autosave_interval_seconds`. Vanilla saves every 6,000 ticks
/// (five minutes); edits in between are kept safe by the journal.
pub const AUTOSAVE_INTERVAL_SECONDS: RangeInclusive<u64> = 10..=3_600;
/// Default `autosave_interval_seconds`, vanilla's five minutes.
pub const DEFAULT_AUTOSAVE_INTERVAL_SECONDS: u64 = 300;
/// Default `world_dir`, relative to the working directory.
pub const DEFAULT_WORLD_DIR: &str = "world";

/// Allowed `generation_workers`: chunk generation threads.
pub const GENERATION_WORKERS: RangeInclusive<usize> = 1..=64;

/// Longest MOTD accepted, in bytes.
pub const MAX_MOTD_BYTES: usize = 256;

/// Settings of the playable server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub address: SocketAddr,
    pub online_mode: bool,
    pub max_players: usize,
    pub motd: String,
    /// `None` disables compression (vanilla's `-1`).
    pub compression_threshold: Option<usize>,
    pub view_distance: u8,
    pub game_mode: GameMode,
    /// Operator-provisioned identifier manifest of the 26.3 registries.
    pub registry_manifest: PathBuf,
    /// Operator-provisioned 26.3 block-state and biome id table.
    pub registry_table: PathBuf,
    /// The official 26.3 `reports/registries.json` (item and entity ids).
    pub registries_report: PathBuf,
    /// World seed for the default terrain.
    pub seed: i64,
    /// Chunk generation threads.
    pub generation_workers: usize,
    /// Directory holding the saved world.
    pub world_dir: PathBuf,
    pub autosave_interval_seconds: u64,
    pub session_server: String,
    pub max_packet_bytes: usize,
    pub read_bytes_per_second: usize,
    pub login_timeout_ms: u64,
    pub outbound_queue_packets: usize,
}

impl ServerConfig {
    /// Defaults for everything but the provisioned registry files.
    pub fn with_registries(
        registry_manifest: PathBuf,
        registry_table: PathBuf,
        registries_report: PathBuf,
    ) -> Self {
        Self {
            address: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_PORT),
            online_mode: true,
            max_players: DEFAULT_MAX_PLAYERS,
            motd: DEFAULT_MOTD.to_owned(),
            compression_threshold: Some(DEFAULT_COMPRESSION_THRESHOLD),
            view_distance: DEFAULT_VIEW_DISTANCE,
            game_mode: GameMode::Survival,
            registry_manifest,
            registry_table,
            registries_report,
            seed: 0,
            generation_workers: default_workers(),
            world_dir: PathBuf::from(DEFAULT_WORLD_DIR),
            autosave_interval_seconds: DEFAULT_AUTOSAVE_INTERVAL_SECONDS,
            session_server: MOJANG_SESSION_SERVER.to_owned(),
            max_packet_bytes: MAX_PACKET_WIRE_BYTES,
            read_bytes_per_second: DEFAULT_READ_BYTES_PER_SECOND,
            login_timeout_ms: DEFAULT_LOGIN_TIMEOUT_MS,
            outbound_queue_packets: DEFAULT_OUTBOUND_QUEUE_PACKETS,
        }
    }
}

/// One generation thread per core, leaving one core for the game loop and
/// connections.
pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |cores| cores.get().saturating_sub(1))
        .clamp(*GENERATION_WORKERS.start(), *GENERATION_WORKERS.end())
}

const KEYS: [&str; 20] = [
    "bind_address",
    "port",
    "online_mode",
    "max_players",
    "motd",
    "network_compression_threshold",
    "view_distance",
    "game_mode",
    "registry_manifest",
    "registry_table",
    "registries_report",
    "seed",
    "generation_workers",
    "world_dir",
    "autosave_interval_seconds",
    "session_server",
    "max_packet_bytes",
    "read_bytes_per_second",
    "login_timeout_ms",
    "outbound_queue_packets",
];

fn integer<T: TryFrom<i64> + Copy + PartialOrd + std::fmt::Display>(
    table: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: T,
    range: &RangeInclusive<T>,
) -> Result<T, String> {
    let Some(value) = table.get(key) else {
        return Ok(default);
    };
    value
        .as_integer()
        .and_then(|n| T::try_from(n).ok())
        .filter(|n| range.contains(n))
        .ok_or_else(|| {
            format!(
                "`server.{key}` must be an integer from {} to {}",
                range.start(),
                range.end()
            )
        })
}

fn text<'a>(
    table: &'a toml::map::Map<String, toml::Value>,
    key: &str,
) -> Result<Option<&'a str>, String> {
    table
        .get(key)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| format!("`server.{key}` must be a string"))
        })
        .transpose()
}

/// Parses the `[server]` table.
pub fn parse_server(value: &toml::Value) -> Result<ServerConfig, String> {
    let table = value
        .as_table()
        .ok_or_else(|| "`server` must be a TOML table".to_owned())?;
    if let Some(key) = table.keys().find(|key| !KEYS.contains(&key.as_str())) {
        return Err(format!("unknown field `server.{key}`"));
    }
    let required = |key: &str| -> Result<PathBuf, String> {
        text(table, key)?
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| format!("`server.{key}` is required"))
    };
    let mut config = ServerConfig::with_registries(
        required("registry_manifest")?,
        required("registry_table")?,
        required("registries_report")?,
    );
    if let Some(value) = table.get("seed") {
        config.seed = value
            .as_integer()
            .ok_or_else(|| "`server.seed` must be a 64-bit integer".to_owned())?;
    }
    config.generation_workers = integer(
        table,
        "generation_workers",
        config.generation_workers,
        &GENERATION_WORKERS,
    )?;
    if let Some(dir) = text(table, "world_dir")? {
        if dir.is_empty() {
            return Err("`server.world_dir` must not be empty".to_owned());
        }
        config.world_dir = PathBuf::from(dir);
    }
    config.autosave_interval_seconds = integer(
        table,
        "autosave_interval_seconds",
        config.autosave_interval_seconds,
        &AUTOSAVE_INTERVAL_SECONDS,
    )?;
    if let Some(address) = text(table, "bind_address")? {
        let ip: IpAddr = address
            .parse()
            .map_err(|_| "`server.bind_address` must be an IP address".to_owned())?;
        config.address.set_ip(ip);
    }
    config
        .address
        .set_port(integer(table, "port", DEFAULT_PORT, &(0..=u16::MAX))?);
    if let Some(value) = table.get("online_mode") {
        config.online_mode = value
            .as_bool()
            .ok_or_else(|| "`server.online_mode` must be true or false".to_owned())?;
    }
    config.max_players = integer(table, "max_players", DEFAULT_MAX_PLAYERS, &MAX_PLAYERS)?;
    if let Some(motd) = text(table, "motd")? {
        if motd.len() > MAX_MOTD_BYTES {
            return Err(format!(
                "`server.motd` must be at most {MAX_MOTD_BYTES} bytes"
            ));
        }
        config.motd = motd.to_owned();
    }
    if let Some(value) = table.get("network_compression_threshold") {
        let threshold = value
            .as_integer()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| {
                "`server.network_compression_threshold` must be a 32-bit integer".to_owned()
            })?;
        config.compression_threshold = usize::try_from(threshold).ok();
    }
    config.view_distance = integer(
        table,
        "view_distance",
        DEFAULT_VIEW_DISTANCE,
        &VIEW_DISTANCE,
    )?;
    if let Some(mode) = text(table, "game_mode")? {
        config.game_mode = GameMode::from_name(mode).ok_or_else(|| {
            "`server.game_mode` must be survival, creative, adventure or spectator".to_owned()
        })?;
    }
    if let Some(url) = text(table, "session_server")? {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err("`server.session_server` must be an http(s) URL".to_owned());
        }
        config.session_server = url.to_owned();
    }
    config.max_packet_bytes = integer(
        table,
        "max_packet_bytes",
        config.max_packet_bytes,
        &MAX_PACKET_BYTES,
    )?;
    config.read_bytes_per_second = integer(
        table,
        "read_bytes_per_second",
        config.read_bytes_per_second,
        &READ_BYTES_PER_SECOND,
    )?;
    config.login_timeout_ms = integer(
        table,
        "login_timeout_ms",
        config.login_timeout_ms,
        &LOGIN_TIMEOUT_MS,
    )?;
    config.outbound_queue_packets = integer(
        table,
        "outbound_queue_packets",
        config.outbound_queue_packets,
        &OUTBOUND_QUEUE_PACKETS,
    )?;
    Ok(config)
}

#[cfg(test)]
mod tests;
