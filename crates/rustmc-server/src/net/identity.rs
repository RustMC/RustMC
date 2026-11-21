//! Player identity: names, UUIDs and the game profile sent at login.
//!
//! Why: every player is keyed by UUID for persistence and visibility, and the
//! client expects its profile in Login Success.
//! Done: name validation, offline-mode UUIDs, profile encoding, dashed/plain
//! UUID text. Not done: profile caching between sessions (online mode asks
//! the session server on every login).
//!
//! Source: minecraft.wiki `Java Edition protocol/Packets`, Login — "The
//! vanilla server only uses UUID v3 for offline player UUIDs, deriving it from
//! the string `OfflinePlayer:<player's name>`"; Game Profile is UUID, Username
//! (String(16)) and a Prefixed Array(16) of properties (Name String(64),
//! Value String(32767), Signature Prefixed Optional String(1024)).

use md5::{Digest, Md5};

use crate::java_wire::{put_string, put_varint};

/// Longest player name the protocol allows (`String(16)`).
pub const MAX_NAME_CHARS: usize = 16;
/// Most profile properties the protocol allows (`Prefixed Array(16)`).
pub const MAX_PROPERTIES: usize = 16;
/// Longest property name (`String(64)`).
pub const MAX_PROPERTY_NAME: usize = 64;
/// Longest property value (`String(32767)`).
pub const MAX_PROPERTY_VALUE: usize = 32_767;
/// Longest property signature (`String(1024)`).
pub const MAX_PROPERTY_SIGNATURE: usize = 1_024;

/// One signed profile property, such as `textures`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileProperty {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

/// A player's identity as the server and client agree on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameProfile {
    pub uuid: [u8; 16],
    pub name: String,
    pub properties: Vec<ProfileProperty>,
}

/// Whether `name` is a name the server accepts: 1 to 16 ASCII letters,
/// digits or underscores (the characters vanilla player names use).
pub fn is_valid_name(name: &str) -> bool {
    (1..=MAX_NAME_CHARS).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The offline-mode UUID: an MD5 name-based (version 3) UUID of
/// `OfflinePlayer:<name>`.
pub fn offline_uuid(name: &str) -> [u8; 16] {
    let mut bytes: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}

/// Parses 32 hex digits, with or without the four dashes.
pub fn parse_uuid(text: &str) -> Option<[u8; 16]> {
    let digits: String = text.chars().filter(|c| *c != '-').collect();
    if digits.len() != 32 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 16];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Formats a UUID in the dashed 8-4-4-4-12 form.
pub fn uuid_text(uuid: &[u8; 16]) -> String {
    let hex: String = uuid.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

impl GameProfile {
    /// The profile of an offline-mode player.
    pub fn offline(name: &str) -> Self {
        Self {
            uuid: offline_uuid(name),
            name: name.to_owned(),
            properties: Vec::new(),
        }
    }

    /// Appends the Game Profile wire form.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.uuid);
        put_string(&self.name, out);
        let properties = &self.properties[..self.properties.len().min(MAX_PROPERTIES)];
        put_varint(properties.len() as u32, out);
        for property in properties {
            put_string(&property.name, out);
            put_string(&property.value, out);
            match &property.signature {
                Some(signature) => {
                    out.push(1);
                    put_string(signature, out);
                }
                None => out.push(0),
            }
        }
    }
}

#[cfg(test)]
mod tests;
