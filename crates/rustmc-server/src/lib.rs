#![forbid(unsafe_code)]

//! RustMC configuration and local discovery runtime. No login or world exists yet.

pub mod discovery_bedrock;
pub mod discovery_java;
pub mod runtime;

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
}
