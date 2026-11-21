//! Online-mode verification against Mojang's session server.
//!
//! Why: in online mode the server must confirm that the connecting client
//! owns the account it names before it trusts the name and UUID.
//! Done: the `hasJoined` request with a bounded timeout and response size,
//! profile parsing, a trait so tests and offline runs need no network.
//! Not done: the optional `ip` parameter (vanilla sends it only with
//! `prevent-proxy-connections`), response caching.
//!
//! Source: minecraft.wiki `Java Edition protocol/Encryption`, Authentication >
//! Server — the server "sends a HTTP GET to
//! `https://sessionserver.mojang.com/session/minecraft/hasJoined?username=_username_&serverId=_hash_`";
//! success is JSON `{"id", "name", "properties": [{"name", "value",
//! "signature"}]}` with an undashed id; otherwise "204 No Content".

use std::fmt;
use std::time::Duration;

use serde_json::Value;

use super::identity::{
    GameProfile, MAX_PROPERTIES, MAX_PROPERTY_NAME, MAX_PROPERTY_SIGNATURE, MAX_PROPERTY_VALUE,
    ProfileProperty, is_valid_name, parse_uuid,
};

/// Mojang's session server, the default `server.session_server`.
pub const MOJANG_SESSION_SERVER: &str = "https://sessionserver.mojang.com";
/// Longest wait for the session server; a login that cannot be verified in
/// this time is refused rather than holding its connection open.
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest response accepted: a profile with sixteen maximal properties.
pub const MAX_RESPONSE_BYTES: u64 =
    (MAX_PROPERTIES * (MAX_PROPERTY_NAME + MAX_PROPERTY_VALUE + MAX_PROPERTY_SIGNATURE) * 2) as u64;

/// Why a login could not be verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The session server could not be reached or answered with an error.
    Unavailable(String),
    /// The session server answered with something that is not a profile.
    BadResponse,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "session server unavailable: {reason}"),
            Self::BadResponse => f.write_str("session server sent an invalid profile"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Answers "did this player join with this server hash?".
pub trait SessionVerifier: Send + Sync {
    /// The verified profile, or `None` when the session server does not know
    /// the join (wrong account, stale hash, or a cracked client).
    fn has_joined(&self, name: &str, server_hash: &str)
    -> Result<Option<GameProfile>, VerifyError>;
}

/// The HTTPS session server client.
pub struct HttpSessionServer {
    base_url: String,
    agent: ureq::Agent,
}

impl HttpSessionServer {
    /// A client for the session server at `base_url` (no trailing slash).
    pub fn new(base_url: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(SESSION_TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            agent,
        }
    }
}

impl SessionVerifier for HttpSessionServer {
    fn has_joined(
        &self,
        name: &str,
        server_hash: &str,
    ) -> Result<Option<GameProfile>, VerifyError> {
        let url = format!("{}/session/minecraft/hasJoined", self.base_url);
        let mut response = self
            .agent
            .get(&url)
            .query("username", name)
            .query("serverId", server_hash)
            .call()
            .map_err(|error| VerifyError::Unavailable(error.to_string()))?;
        match response.status().as_u16() {
            200 => {
                let body = response
                    .body_mut()
                    .with_config()
                    .limit(MAX_RESPONSE_BYTES)
                    .read_to_string()
                    .map_err(|_| VerifyError::BadResponse)?;
                parse_profile(&body, name).map(Some)
            }
            204 => Ok(None),
            status => Err(VerifyError::Unavailable(format!("HTTP status {status}"))),
        }
    }
}

/// Parses a `hasJoined` profile and checks it names the expected player
/// (the wiki: the username "is case insensitive").
pub fn parse_profile(body: &str, expected_name: &str) -> Result<GameProfile, VerifyError> {
    let value: Value = serde_json::from_str(body).map_err(|_| VerifyError::BadResponse)?;
    let text = |v: &Value, max: usize| -> Result<String, VerifyError> {
        v.as_str()
            .filter(|s| s.len() <= max)
            .map(str::to_owned)
            .ok_or(VerifyError::BadResponse)
    };
    let uuid = value["id"]
        .as_str()
        .and_then(parse_uuid)
        .ok_or(VerifyError::BadResponse)?;
    let name = text(&value["name"], super::identity::MAX_NAME_CHARS)?;
    if !is_valid_name(&name) || !name.eq_ignore_ascii_case(expected_name) {
        return Err(VerifyError::BadResponse);
    }
    let mut properties = Vec::new();
    if let Some(list) = value.get("properties") {
        let list = list.as_array().ok_or(VerifyError::BadResponse)?;
        if list.len() > MAX_PROPERTIES {
            return Err(VerifyError::BadResponse);
        }
        for entry in list {
            properties.push(ProfileProperty {
                name: text(&entry["name"], MAX_PROPERTY_NAME)?,
                value: text(&entry["value"], MAX_PROPERTY_VALUE)?,
                signature: match entry.get("signature") {
                    None | Some(Value::Null) => None,
                    Some(signature) => Some(text(signature, MAX_PROPERTY_SIGNATURE)?),
                },
            });
        }
    }
    Ok(GameProfile {
        uuid,
        name,
        properties,
    })
}

#[cfg(test)]
mod tests;
