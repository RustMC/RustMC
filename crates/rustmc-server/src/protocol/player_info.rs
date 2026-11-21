//! Java Edition 26.3 PlayerInfoUpdate (packet 0x47, protocol 777) packet codec
//! and player tab-list tracker.
//!
//! # Protocol Specification
//!
//! Under Minecraft Java Edition 26.3 (protocol 777), tab-list state and player presence
//! updates are communicated using clientbound `PlayerInfoUpdate` (ID 0x47 in Play state).
//!
//! The packet consists of:
//! - An `actions` bitmask (`u8`) controlling which fields are populated for each entry.
//! - A VarInt count of player entries.
//! - An array of player entries, each containing a 16-byte UUID followed by the fields
//!   specified by the active action flags in deterministic protocol order.
//!
//! ## Actions Bitflags
//!
//! - `AddPlayer` (0x01): Username (String, max 16 chars) and property list (e.g. skin textures).
//! - `InitializeChat` (0x02): Chat session presence flag and cryptographic public key data.
//! - `UpdateGameMode` (0x04): GameMode VarInt (0: Survival, 1: Creative, 2: Adventure, 3: Spectator).
//! - `UpdateListed` (0x08): Boolean flag controlling tab-list visibility.
//! - `UpdateLatency` (0x10): Latency in milliseconds (VarInt).
//! - `UpdateDisplayName` (0x20): Optional Network NBT text component display name.
//! - `UpdateListOrder` (0x40): Tab-list display ordering index (VarInt).
//! - `UpdateHat` (0x80): Boolean flag controlling whether the player skin hat layer is shown.
//!
//! The packet ID comes from the 26.3 server's generated packet report. Layouts and
//! field limits follow the public protocol reference for 26.3 / protocol 777:
//! <https://minecraft.wiki/w/Java_Edition_protocol/Packets> (revision 3868170) and
//! <https://minecraft.wiki/w/Java_Edition_protocol/Data_types> (revision 3855787).

use std::collections::BTreeMap;
use std::fmt;

use crate::java_wire::{self, WireError, put_nbt_text, put_string, put_varint};

/// Packet ID for clientbound PlayerInfoUpdate in Java Edition 26.3 (Play state 0x47 / 71).
pub const PACKET_ID_PLAYER_INFO_UPDATE: u32 = 0x47;

/// Maximum character length for player usernames in Java Edition.
pub const MAX_NAME_CHARS: usize = 16;

/// Maximum UTF-16 length for general protocol strings (e.g. property values).
pub const MAX_STRING_CHARS: usize = 32767;

/// Maximum number of profile properties for one player.
pub const MAX_PROPERTIES: usize = 16;

/// Maximum UTF-16 length of a profile property name.
pub const MAX_PROPERTY_NAME_CHARS: usize = 64;

/// Maximum UTF-16 length of a profile property signature.
pub const MAX_PROPERTY_SIGNATURE_CHARS: usize = 1024;

/// Maximum byte length of a chat session public key.
pub const MAX_PUBLIC_KEY_BYTES: usize = 512;

/// Maximum byte length of a chat session public key signature.
pub const MAX_KEY_SIGNATURE_BYTES: usize = 4096;

/// RustMC policy bound on NBT compound and list nesting. It limits recursion on
/// untrusted input to prevent stack exhaustion; it is not a protocol value.
pub const MAX_NBT_DEPTH: usize = 512;

/// Action bitflags defining which fields are present in a `PlayerInfoUpdate` packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum PlayerInfoAction {
    AddPlayer = 0x01,
    InitializeChat = 0x02,
    UpdateGameMode = 0x04,
    UpdateListed = 0x08,
    UpdateLatency = 0x10,
    UpdateDisplayName = 0x20,
    UpdateListOrder = 0x40,
    UpdateHat = 0x80,
}

impl PlayerInfoAction {
    pub const ADD_PLAYER: u8 = 0x01;
    pub const INITIALIZE_CHAT: u8 = 0x02;
    pub const UPDATE_GAME_MODE: u8 = 0x04;
    pub const UPDATE_LISTED: u8 = 0x08;
    pub const UPDATE_LATENCY: u8 = 0x10;
    pub const UPDATE_DISPLAY_NAME: u8 = 0x20;
    pub const UPDATE_LIST_ORDER: u8 = 0x40;
    pub const UPDATE_HAT: u8 = 0x80;

    /// Bitmask with all 8 valid player info actions enabled (0xFF).
    pub const ALL: u8 = Self::ADD_PLAYER
        | Self::INITIALIZE_CHAT
        | Self::UPDATE_GAME_MODE
        | Self::UPDATE_LISTED
        | Self::UPDATE_LATENCY
        | Self::UPDATE_DISPLAY_NAME
        | Self::UPDATE_LIST_ORDER
        | Self::UPDATE_HAT;

    /// Bitmask of all recognized action bits (0xFF).
    pub const MASK_VALID: u8 = Self::ALL;

    /// Returns the raw bitmask flag for this action.
    #[inline]
    pub const fn flag(self) -> u8 {
        self as u8
    }

    /// Checks whether the given bitmask contains this action.
    #[inline]
    pub const fn is_set(bitmask: u8, action: Self) -> bool {
        (bitmask & (action as u8)) != 0
    }
}

impl std::ops::BitOr for PlayerInfoAction {
    type Output = u8;

    #[inline]
    fn bitor(self, rhs: Self) -> Self::Output {
        (self as u8) | (rhs as u8)
    }
}

impl std::ops::BitOr<u8> for PlayerInfoAction {
    type Output = u8;

    #[inline]
    fn bitor(self, rhs: u8) -> Self::Output {
        (self as u8) | rhs
    }
}

impl std::ops::BitOr<PlayerInfoAction> for u8 {
    type Output = u8;

    #[inline]
    fn bitor(self, rhs: PlayerInfoAction) -> Self::Output {
        self | (rhs as u8)
    }
}

impl std::ops::BitOrAssign<PlayerInfoAction> for u8 {
    #[inline]
    fn bitor_assign(&mut self, rhs: PlayerInfoAction) {
        *self |= rhs as u8;
    }
}

impl TryFrom<u8> for PlayerInfoAction {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(Self::AddPlayer),
            0x02 => Ok(Self::InitializeChat),
            0x04 => Ok(Self::UpdateGameMode),
            0x08 => Ok(Self::UpdateListed),
            0x10 => Ok(Self::UpdateLatency),
            0x20 => Ok(Self::UpdateDisplayName),
            0x40 => Ok(Self::UpdateListOrder),
            0x80 => Ok(Self::UpdateHat),
            other => Err(ProtocolError::InvalidActionBitmask(other)),
        }
    }
}

/// Java Edition gameplay modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(i32)]
pub enum GameMode {
    #[default]
    Survival = 0,
    Creative = 1,
    Adventure = 2,
    Spectator = 3,
}

impl GameMode {
    /// Parses a GameMode from its VarInt protocol identifier.
    pub fn from_id(id: i32) -> Result<Self, ProtocolError> {
        match id {
            0 => Ok(Self::Survival),
            1 => Ok(Self::Creative),
            2 => Ok(Self::Adventure),
            3 => Ok(Self::Spectator),
            other => Err(ProtocolError::InvalidGameMode(other)),
        }
    }

    /// Returns the protocol integer ID for this game mode.
    #[inline]
    pub const fn id(self) -> i32 {
        self as i32
    }
}

/// Profile property attached to a player (e.g. skin/cape textures and signatures).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ProfileProperty {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

impl ProfileProperty {
    pub fn new(
        name: impl Into<String>,
        value: impl Into<String>,
        signature: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            signature,
        }
    }

    pub fn unsigned(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(name, value, None)
    }

    pub fn signed(
        name: impl Into<String>,
        value: impl Into<String>,
        signature: impl Into<String>,
    ) -> Self {
        Self::new(name, value, Some(signature.into()))
    }
}

/// Cryptographic chat session data associated with signed player chat.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChatSessionData {
    pub session_id: [u8; 16],
    pub expires_at: i64,
    pub public_key: Vec<u8>,
    pub key_signature: Vec<u8>,
}

impl ChatSessionData {
    pub fn new(
        session_id: [u8; 16],
        expires_at: i64,
        public_key: Vec<u8>,
        key_signature: Vec<u8>,
    ) -> Self {
        Self {
            session_id,
            expires_at,
            public_key,
            key_signature,
        }
    }
}

/// Entry representing a single player's metadata in the tab-list update packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoEntry {
    pub uuid: [u8; 16],
    pub name: String,
    pub properties: Vec<ProfileProperty>,
    pub chat_session: Option<ChatSessionData>,
    pub game_mode: GameMode,
    pub listed: bool,
    pub latency: i32,
    pub display_name: Option<PlayerDisplayName>,
    pub list_order: i32,
    pub show_hat: bool,
}

impl Default for PlayerInfoEntry {
    fn default() -> Self {
        Self {
            uuid: [0u8; 16],
            name: String::new(),
            properties: Vec::new(),
            chat_session: None,
            game_mode: GameMode::Survival,
            listed: true,
            latency: 0,
            display_name: None,
            list_order: 0,
            show_hat: true,
        }
    }
}

impl PlayerInfoEntry {
    /// Creates a new player info entry with default presence settings.
    pub fn new(uuid: [u8; 16], name: impl Into<String>) -> Self {
        Self {
            uuid,
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn with_game_mode(mut self, game_mode: GameMode) -> Self {
        self.game_mode = game_mode;
        self
    }

    pub fn with_listed(mut self, listed: bool) -> Self {
        self.listed = listed;
        self
    }

    pub fn with_latency(mut self, latency: i32) -> Self {
        self.latency = latency;
        self
    }

    pub fn with_display_name(mut self, display_name: Option<PlayerDisplayName>) -> Self {
        self.display_name = display_name;
        self
    }

    pub fn with_list_order(mut self, list_order: i32) -> Self {
        self.list_order = list_order;
        self
    }

    pub fn with_show_hat(mut self, show_hat: bool) -> Self {
        self.show_hat = show_hat;
        self
    }

    pub fn with_properties(mut self, properties: Vec<ProfileProperty>) -> Self {
        self.properties = properties;
        self
    }

    pub fn with_chat_session(mut self, chat_session: Option<ChatSessionData>) -> Self {
        self.chat_session = chat_session;
        self
    }
}

/// Errors arising during protocol encoding, decoding, or framing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    UnexpectedEof,
    MalformedVarInt,
    NonCanonicalVarInt,
    StringTooLong { length: usize, max: usize },
    InvalidUtf8,
    InvalidGameMode(i32),
    InvalidActionBitmask(u8),
    InvalidBoolean(u8),
    InvalidNbtTag(u8),
    NbtDepthExceeded { depth: usize, max: usize },
    UnsupportedComponent,
    UnexpectedPacketId { expected: u32, received: u32 },
    InvalidPacketLength { expected: usize, received: usize },
    TrailingData { remaining: usize },
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected end of packet stream"),
            Self::MalformedVarInt => write!(f, "malformed VarInt in packet stream"),
            Self::NonCanonicalVarInt => {
                write!(
                    f,
                    "non-canonical VarInt encoding with redundant zero padding"
                )
            }
            Self::StringTooLong { length, max } => {
                write!(
                    f,
                    "string exceeds maximum permitted characters ({length} > {max})"
                )
            }
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 sequence in string"),
            Self::InvalidGameMode(mode) => write!(f, "invalid game mode ID: {mode}"),
            Self::InvalidActionBitmask(mask) => {
                write!(f, "invalid action bitmask with unknown bits: 0x{mask:02X}")
            }
            Self::InvalidBoolean(val) => {
                write!(f, "invalid boolean value: {val} (expected 0 or 1)")
            }
            Self::InvalidNbtTag(tag) => {
                write!(
                    f,
                    "invalid NBT tag type for network text component: 0x{tag:02X}"
                )
            }
            Self::NbtDepthExceeded { depth, max } => {
                write!(
                    f,
                    "NBT nesting depth exceeded maximum permitted limit ({depth} > {max})"
                )
            }
            Self::UnsupportedComponent => {
                write!(f, "unsupported or empty text component structure in NBT")
            }
            Self::UnexpectedPacketId { expected, received } => {
                write!(
                    f,
                    "unexpected packet ID: expected 0x{expected:02X}, received 0x{received:02X}"
                )
            }
            Self::InvalidPacketLength { expected, received } => {
                write!(
                    f,
                    "invalid packet length: expected {expected} bytes, received {received}"
                )
            }
            Self::TrailingData { remaining } => {
                write!(
                    f,
                    "unparsed trailing data remaining in packet ({remaining} bytes)"
                )
            }
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Represents a player's tab-list display name.
///
/// In Java Edition 26.3 / protocol 777, display names on the wire are serialized
/// as anonymous Network NBT (primitive `TAG_String` 0x08 or `TAG_Compound` 0x0A).
///
/// This type preserves complete component formatting, nested `extra` arrays, and
/// translations on the wire while exposing the component's literal text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerDisplayName {
    /// Plain text display name, serialized as an anonymous Network NBT TAG_String (0x08).
    Plain(String),
    /// Complete Network NBT compound, preserving all nested tags, styling, colors,
    /// and extra components without loss. `text` joins its literal `text` and `extra`
    /// content; other content, such as a translation, is only in `raw`. The encoder
    /// validates `raw` again, so a hand-built value cannot emit invalid NBT.
    RawNbt { raw: Vec<u8>, text: String },
}

impl PlayerDisplayName {
    /// Creates a new plain text display name.
    pub fn plain(text: impl Into<String>) -> Self {
        Self::Plain(text.into())
    }

    /// Creates a display name from raw Network NBT bytes, parsing and validating
    /// its structure and extracting visible text.
    pub fn from_raw_nbt(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (dn, consumed) = read_display_name_nbt(bytes, MAX_STRING_CHARS)?;
        if consumed != bytes.len() {
            return Err(ProtocolError::TrailingData {
                remaining: bytes.len() - consumed,
            });
        }
        Ok(dn)
    }

    /// Returns the literal text of the display name.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Plain(s) => s.as_str(),
            Self::RawNbt { text, .. } => text.as_str(),
        }
    }

    /// Returns the raw Network NBT bytes if this was parsed from a raw component.
    pub fn raw_nbt(&self) -> Option<&[u8]> {
        match self {
            Self::Plain(_) => None,
            Self::RawNbt { raw, .. } => Some(raw.as_slice()),
        }
    }
}

impl std::ops::Deref for PlayerDisplayName {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for PlayerDisplayName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for PlayerDisplayName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<&str> for PlayerDisplayName {
    fn from(s: &str) -> Self {
        Self::Plain(s.to_string())
    }
}

impl From<String> for PlayerDisplayName {
    fn from(s: String) -> Self {
        Self::Plain(s)
    }
}

impl PartialEq<str> for PlayerDisplayName {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for PlayerDisplayName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

mod packet;
pub use packet::*;
mod wire;
pub use wire::*;

#[cfg(test)]
mod tests;
