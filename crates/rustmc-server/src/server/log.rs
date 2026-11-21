//! Structured log lines: `event=<kind> key=value ...` on standard output.
//!
//! Why: operators and tests grep one machine-readable line per event.
//! Done: quoting of values with spaces or quotes. Not done: log levels and
//! files (server features track).

use std::fmt::Display;
use std::io::Write;

/// Formats one event line.
pub fn line(kind: &str, fields: &[(&str, &dyn Display)]) -> String {
    let mut line = format!("event={kind}");
    for (key, value) in fields {
        let value = value.to_string();
        if value.is_empty() || value.contains([' ', '"', '=', '\n']) {
            line.push_str(&format!(" {key}={value:?}"));
        } else {
            line.push_str(&format!(" {key}={value}"));
        }
    }
    line
}

/// Writes one event line; a closed stdout is ignored.
pub fn event(kind: &str, fields: &[(&str, &dyn Display)]) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", line(kind, fields));
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_with_spaces_are_quoted() {
        assert_eq!(
            line(
                "player_left",
                &[("name", &"Steve"), ("reason", &"timed out")]
            ),
            "event=player_left name=Steve reason=\"timed out\""
        );
        assert_eq!(line("x", &[("empty", &"")]), "event=x empty=\"\"");
    }
}
