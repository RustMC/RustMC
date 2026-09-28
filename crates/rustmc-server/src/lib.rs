#![forbid(unsafe_code)]

//! Bootstrap configuration validation for RustMC. No game server exists yet.

use std::path::Path;

/// Configuration schema supported by the bootstrap CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub schema_version: u32,
    pub log_level: LogLevel,
}

/// Valid log levels for the future runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
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
        if key != "schema_version" && key != "log_level" {
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
    Ok(Config {
        schema_version: 1,
        log_level,
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
    fn accepts_supported_values() {
        let parsed = parse_config("schema_version = 1\nlog_level = 'trace'\n").unwrap();
        assert_eq!(parsed.log_level, LogLevel::Trace);
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
    }

    #[test]
    fn does_not_echo_invalid_value() {
        let error = parse_config("schema_version = 1\nlog_level = 'private-token'").unwrap_err();
        assert!(!error.contains("private-token"));
    }
}
