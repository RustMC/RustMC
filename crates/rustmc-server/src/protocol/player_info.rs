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

/// Clientbound `PlayerInfoUpdate` packet (packet ID 0x47 in Java 26.3 protocol 777).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoUpdatePacket {
    pub actions: u8,
    pub entries: Vec<PlayerInfoEntry>,
}

impl PlayerInfoUpdatePacket {
    pub const PACKET_ID: u32 = PACKET_ID_PLAYER_INFO_UPDATE;

    /// Creates a new `PlayerInfoUpdatePacket` with given action bitmask and entries.
    pub fn new(actions: u8, entries: Vec<PlayerInfoEntry>) -> Self {
        Self { actions, entries }
    }

    /// Creates a packet containing a single entry.
    pub fn single(actions: u8, entry: PlayerInfoEntry) -> Self {
        Self {
            actions,
            entries: vec![entry],
        }
    }

    /// Encodes the packet into framed byte format (VarInt length + VarInt packet ID 0x47 + body).
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let payload = self.encode_payload()?;
        let mut header = Vec::with_capacity(5);
        write_varint(Self::PACKET_ID as i32, &mut header);
        let total_len = header.len() + payload.len();

        let mut framed = Vec::with_capacity(varint_len(total_len as u32) + total_len);
        write_varint(total_len as i32, &mut framed);
        framed.extend_from_slice(&header);
        framed.extend_from_slice(&payload);
        Ok(framed)
    }

    /// Encodes only the unframed packet payload (actions bitmask + entry count + entries).
    pub fn encode_payload(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut out = Vec::new();
        out.push(self.actions);
        write_varint(self.entries.len() as i32, &mut out);

        for entry in &self.entries {
            write_uuid(&entry.uuid, &mut out);

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::AddPlayer) {
                check_chars(&entry.name, MAX_NAME_CHARS)?;
                write_string(&entry.name, &mut out);
                check_length(entry.properties.len(), MAX_PROPERTIES)?;
                write_varint(entry.properties.len() as i32, &mut out);
                for prop in &entry.properties {
                    check_chars(&prop.name, MAX_PROPERTY_NAME_CHARS)?;
                    check_chars(&prop.value, MAX_STRING_CHARS)?;
                    write_string(&prop.name, &mut out);
                    write_string(&prop.value, &mut out);
                    match &prop.signature {
                        Some(sig) => {
                            check_chars(sig, MAX_PROPERTY_SIGNATURE_CHARS)?;
                            write_bool(true, &mut out);
                            write_string(sig, &mut out);
                        }
                        None => {
                            write_bool(false, &mut out);
                        }
                    }
                }
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::InitializeChat) {
                match &entry.chat_session {
                    Some(session) => {
                        check_length(session.public_key.len(), MAX_PUBLIC_KEY_BYTES)?;
                        check_length(session.key_signature.len(), MAX_KEY_SIGNATURE_BYTES)?;
                        write_bool(true, &mut out);
                        write_uuid(&session.session_id, &mut out);
                        out.extend_from_slice(&session.expires_at.to_be_bytes());
                        write_byte_array(&session.public_key, &mut out);
                        write_byte_array(&session.key_signature, &mut out);
                    }
                    None => {
                        write_bool(false, &mut out);
                    }
                }
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::UpdateGameMode) {
                write_varint(entry.game_mode.id(), &mut out);
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::UpdateListed) {
                write_bool(entry.listed, &mut out);
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::UpdateLatency) {
                write_varint(entry.latency, &mut out);
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::UpdateDisplayName) {
                match &entry.display_name {
                    Some(name) => {
                        write_bool(true, &mut out);
                        write_display_name_nbt(name, &mut out)?;
                    }
                    None => {
                        write_bool(false, &mut out);
                    }
                }
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::UpdateListOrder) {
                write_varint(entry.list_order, &mut out);
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::UpdateHat) {
                write_bool(entry.show_hat, &mut out);
            }
        }

        Ok(out)
    }

    /// Decodes a framed packet with VarInt length prefix and packet ID 0x47.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (frame_len, offset) = read_varint(bytes)?;
        if frame_len < 0 {
            return Err(ProtocolError::MalformedVarInt);
        }
        let frame_len = frame_len as usize;
        let expected_total = offset + frame_len;
        if bytes.len() < expected_total {
            return Err(ProtocolError::UnexpectedEof);
        }
        if bytes.len() > expected_total {
            return Err(ProtocolError::TrailingData {
                remaining: bytes.len() - expected_total,
            });
        }

        let frame_slice = &bytes[offset..expected_total];
        let (packet_id, id_offset) = read_varint(frame_slice)?;
        if packet_id as u32 != Self::PACKET_ID {
            return Err(ProtocolError::UnexpectedPacketId {
                expected: Self::PACKET_ID,
                received: packet_id as u32,
            });
        }

        let payload = &frame_slice[id_offset..];
        Self::decode_payload(payload)
    }

    /// Decodes an unframed packet payload (actions bitmask + entry count + entries).
    pub fn decode_payload(payload: &[u8]) -> Result<Self, ProtocolError> {
        let mut cursor = 0;
        if payload.is_empty() {
            return Err(ProtocolError::UnexpectedEof);
        }
        let actions = payload[cursor];
        cursor += 1;

        let (entry_count, n) = read_varint(&payload[cursor..])?;
        cursor += n;
        if entry_count < 0 {
            return Err(ProtocolError::MalformedVarInt);
        }
        let entry_count = entry_count as usize;

        let mut entries = Vec::with_capacity(entry_count.min(1024));

        for _ in 0..entry_count {
            let (uuid, n) = read_uuid(&payload[cursor..])?;
            cursor += n;

            let mut entry = PlayerInfoEntry {
                uuid,
                ..Default::default()
            };

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::AddPlayer) {
                let (name, n) = read_string(&payload[cursor..], MAX_NAME_CHARS)?;
                cursor += n;
                entry.name = name;

                let (prop_count, n) = read_varint(&payload[cursor..])?;
                cursor += n;
                if prop_count < 0 {
                    return Err(ProtocolError::MalformedVarInt);
                }
                let prop_count = prop_count as usize;
                check_length(prop_count, MAX_PROPERTIES)?;
                let mut props = Vec::with_capacity(prop_count);
                for _ in 0..prop_count {
                    let (prop_name, n) = read_string(&payload[cursor..], MAX_PROPERTY_NAME_CHARS)?;
                    cursor += n;
                    let (prop_val, n) = read_string(&payload[cursor..], MAX_STRING_CHARS)?;
                    cursor += n;
                    let (has_sig, n) = read_bool(&payload[cursor..])?;
                    cursor += n;
                    let sig = if has_sig {
                        let (sig_str, n) =
                            read_string(&payload[cursor..], MAX_PROPERTY_SIGNATURE_CHARS)?;
                        cursor += n;
                        Some(sig_str)
                    } else {
                        None
                    };
                    props.push(ProfileProperty {
                        name: prop_name,
                        value: prop_val,
                        signature: sig,
                    });
                }
                entry.properties = props;
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::InitializeChat) {
                let (has_chat, n) = read_bool(&payload[cursor..])?;
                cursor += n;
                if has_chat {
                    let (session_id, n) = read_uuid(&payload[cursor..])?;
                    cursor += n;
                    let (expires_at, _) = java_wire::read_i64_be(&payload[cursor..])
                        .map_err(|_| ProtocolError::UnexpectedEof)?;
                    cursor += 8;

                    let (public_key, n) =
                        read_byte_array(&payload[cursor..], MAX_PUBLIC_KEY_BYTES)?;
                    cursor += n;

                    let (key_signature, n) =
                        read_byte_array(&payload[cursor..], MAX_KEY_SIGNATURE_BYTES)?;
                    cursor += n;

                    entry.chat_session = Some(ChatSessionData {
                        session_id,
                        expires_at,
                        public_key,
                        key_signature,
                    });
                } else {
                    entry.chat_session = None;
                }
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::UpdateGameMode) {
                let (mode_id, n) = read_varint(&payload[cursor..])?;
                cursor += n;
                entry.game_mode = GameMode::from_id(mode_id)?;
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::UpdateListed) {
                let (listed, n) = read_bool(&payload[cursor..])?;
                cursor += n;
                entry.listed = listed;
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::UpdateLatency) {
                let (latency, n) = read_varint(&payload[cursor..])?;
                cursor += n;
                entry.latency = latency;
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::UpdateDisplayName) {
                let (has_display, n) = read_bool(&payload[cursor..])?;
                cursor += n;
                if has_display {
                    let (display_name, n) =
                        read_display_name_nbt(&payload[cursor..], MAX_STRING_CHARS)?;
                    cursor += n;
                    entry.display_name = Some(display_name);
                } else {
                    entry.display_name = None;
                }
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::UpdateListOrder) {
                let (list_order, n) = read_varint(&payload[cursor..])?;
                cursor += n;
                entry.list_order = list_order;
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::UpdateHat) {
                let (show_hat, n) = read_bool(&payload[cursor..])?;
                cursor += n;
                entry.show_hat = show_hat;
            }

            entries.push(entry);
        }

        if cursor < payload.len() {
            return Err(ProtocolError::TrailingData {
                remaining: payload.len() - cursor,
            });
        }

        Ok(Self { actions, entries })
    }
}

/// Tab-list tracker maintaining online player state and generating differential
/// update packets for broadcast to connected players.
#[derive(Debug, Clone, Default)]
pub struct TabListTracker {
    players: BTreeMap<[u8; 16], PlayerInfoEntry>,
}

impl TabListTracker {
    /// Creates an empty `TabListTracker`.
    pub fn new() -> Self {
        Self {
            players: BTreeMap::new(),
        }
    }

    /// Returns the number of players currently tracked in the tab list.
    #[inline]
    pub fn len(&self) -> usize {
        self.players.len()
    }

    /// Returns true if no players are currently tracked.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.players.is_empty()
    }

    /// Checks if a player with the given UUID is currently online.
    #[inline]
    pub fn contains_player(&self, uuid: &[u8; 16]) -> bool {
        self.players.contains_key(uuid)
    }

    /// Gets a reference to the tracked player entry with the given UUID.
    #[inline]
    pub fn get_player(&self, uuid: &[u8; 16]) -> Option<&PlayerInfoEntry> {
        self.players.get(uuid)
    }

    /// Gets a mutable reference to the tracked player entry with the given UUID.
    #[inline]
    pub fn get_player_mut(&mut self, uuid: &[u8; 16]) -> Option<&mut PlayerInfoEntry> {
        self.players.get_mut(uuid)
    }

    /// Returns an iterator over all tracked online player entries in deterministic UUID order.
    pub fn players(&self) -> impl Iterator<Item = &PlayerInfoEntry> {
        self.players.values()
    }

    /// Adds a player to the tab list and returns a differential packet
    /// suitable for broadcasting the new player to all connected clients.
    pub fn add_player(&mut self, entry: PlayerInfoEntry) -> PlayerInfoUpdatePacket {
        let mut actions = PlayerInfoAction::AddPlayer
            | PlayerInfoAction::UpdateGameMode
            | PlayerInfoAction::UpdateListed
            | PlayerInfoAction::UpdateLatency
            | PlayerInfoAction::UpdateDisplayName
            | PlayerInfoAction::UpdateListOrder
            | PlayerInfoAction::UpdateHat;
        if entry.chat_session.is_some() {
            actions |= PlayerInfoAction::InitializeChat;
        }

        let packet = PlayerInfoUpdatePacket::single(actions, entry.clone());
        self.players.insert(entry.uuid, entry);
        packet
    }

    /// Removes a player from the tab list tracker by UUID.
    /// Returns the removed `PlayerInfoEntry` if the player was present.
    pub fn remove_player(&mut self, uuid: &[u8; 16]) -> Option<PlayerInfoEntry> {
        self.players.remove(uuid)
    }

    /// Updates the latency (ping in ms) for an online player and generates
    /// an `UpdateLatency` diff packet for broadcast. Returns `None` if the player is not online.
    pub fn update_latency(
        &mut self,
        uuid: &[u8; 16],
        latency: i32,
    ) -> Option<PlayerInfoUpdatePacket> {
        let entry = self.players.get_mut(uuid)?;
        entry.latency = latency;
        Some(PlayerInfoUpdatePacket::single(
            PlayerInfoAction::UpdateLatency as u8,
            PlayerInfoEntry {
                uuid: *uuid,
                latency,
                ..Default::default()
            },
        ))
    }

    /// Updates the game mode for an online player and generates
    /// an `UpdateGameMode` diff packet for broadcast. Returns `None` if the player is not online.
    pub fn update_game_mode(
        &mut self,
        uuid: &[u8; 16],
        game_mode: GameMode,
    ) -> Option<PlayerInfoUpdatePacket> {
        let entry = self.players.get_mut(uuid)?;
        entry.game_mode = game_mode;
        Some(PlayerInfoUpdatePacket::single(
            PlayerInfoAction::UpdateGameMode as u8,
            PlayerInfoEntry {
                uuid: *uuid,
                game_mode,
                ..Default::default()
            },
        ))
    }

    /// Updates the display name for an online player and generates
    /// an `UpdateDisplayName` diff packet for broadcast. Returns `None` if the player is not online.
    pub fn update_display_name(
        &mut self,
        uuid: &[u8; 16],
        display_name: Option<PlayerDisplayName>,
    ) -> Option<PlayerInfoUpdatePacket> {
        let entry = self.players.get_mut(uuid)?;
        entry.display_name = display_name.clone();
        Some(PlayerInfoUpdatePacket::single(
            PlayerInfoAction::UpdateDisplayName as u8,
            PlayerInfoEntry {
                uuid: *uuid,
                display_name,
                ..Default::default()
            },
        ))
    }

    /// Updates the list order for an online player and generates
    /// an `UpdateListOrder` diff packet for broadcast. Returns `None` if the player is not online.
    pub fn update_list_order(
        &mut self,
        uuid: &[u8; 16],
        list_order: i32,
    ) -> Option<PlayerInfoUpdatePacket> {
        let entry = self.players.get_mut(uuid)?;
        entry.list_order = list_order;
        Some(PlayerInfoUpdatePacket::single(
            PlayerInfoAction::UpdateListOrder as u8,
            PlayerInfoEntry {
                uuid: *uuid,
                list_order,
                ..Default::default()
            },
        ))
    }

    /// Updates the hat visibility for an online player and generates
    /// an `UpdateHat` diff packet for broadcast. Returns `None` if the player is not online.
    pub fn update_hat(
        &mut self,
        uuid: &[u8; 16],
        show_hat: bool,
    ) -> Option<PlayerInfoUpdatePacket> {
        let entry = self.players.get_mut(uuid)?;
        entry.show_hat = show_hat;
        Some(PlayerInfoUpdatePacket::single(
            PlayerInfoAction::UpdateHat as u8,
            PlayerInfoEntry {
                uuid: *uuid,
                show_hat,
                ..Default::default()
            },
        ))
    }

    /// Updates the listed status for an online player and generates
    /// an `UpdateListed` diff packet for broadcast. Returns `None` if the player is not online.
    pub fn update_listed(
        &mut self,
        uuid: &[u8; 16],
        listed: bool,
    ) -> Option<PlayerInfoUpdatePacket> {
        let entry = self.players.get_mut(uuid)?;
        entry.listed = listed;
        Some(PlayerInfoUpdatePacket::single(
            PlayerInfoAction::UpdateListed as u8,
            PlayerInfoEntry {
                uuid: *uuid,
                listed,
                ..Default::default()
            },
        ))
    }

    /// Generates a full synchronization packet containing all currently online players.
    /// This is sent to newly joined players to populate their entire tab list.
    pub fn full_tab_list_packet(&self) -> PlayerInfoUpdatePacket {
        let actions = PlayerInfoAction::ALL;
        let entries = self.players.values().cloned().collect();
        PlayerInfoUpdatePacket::new(actions, entries)
    }

    /// Clears all tracked players.
    pub fn clear(&mut self) {
        self.players.clear();
    }
}

// ---------------------------------------------------------------------------
// Protocol wire codec helpers
// ---------------------------------------------------------------------------

/// Computes the number of bytes required to serialize `value` as a VarInt.
#[inline]
pub fn varint_len(value: u32) -> usize {
    let mut len = 1;
    let mut v = value >> 7;
    while v != 0 {
        len += 1;
        v >>= 7;
    }
    len
}

/// Encodes a 32-bit signed integer as a canonical VarInt into `out`.
pub fn write_varint(value: i32, out: &mut Vec<u8>) {
    put_varint(value as u32, out);
}

/// Decodes a 32-bit signed integer from a canonical VarInt stream.
///
/// Rejects non-canonical encodings with superfluous trailing zero bytes (e.g. `[0x80, 0x00]`)
/// and malformed sequences exceeding 5 bytes or containing illegal upper bits.
pub fn read_varint(input: &[u8]) -> Result<(i32, usize), ProtocolError> {
    match java_wire::read_varint(input) {
        Ok(Some((value, bytes_read))) => Ok((value as i32, bytes_read)),
        Ok(None) => Err(ProtocolError::UnexpectedEof),
        Err(WireError::NonCanonicalVarInt) => Err(ProtocolError::NonCanonicalVarInt),
        Err(_) => Err(ProtocolError::MalformedVarInt),
    }
}

/// Writes a boolean value as a single byte (0x01 for true, 0x00 for false).
#[inline]
pub fn write_bool(val: bool, out: &mut Vec<u8>) {
    out.push(if val { 1 } else { 0 });
}

/// Reads a boolean value from the stream, strictly rejecting any byte other than 0 or 1.
pub fn read_bool(input: &[u8]) -> Result<(bool, usize), ProtocolError> {
    if input.is_empty() {
        return Err(ProtocolError::UnexpectedEof);
    }
    match input[0] {
        0 => Ok((false, 1)),
        1 => Ok((true, 1)),
        other => Err(ProtocolError::InvalidBoolean(other)),
    }
}

/// Writes a 16-byte UUID into `out`.
#[inline]
pub fn write_uuid(uuid: &[u8; 16], out: &mut Vec<u8>) {
    out.extend_from_slice(uuid);
}

/// Reads a 16-byte UUID from `input`.
pub fn read_uuid(input: &[u8]) -> Result<([u8; 16], usize), ProtocolError> {
    if input.len() < 16 {
        return Err(ProtocolError::UnexpectedEof);
    }
    let mut uuid = [0u8; 16];
    uuid.copy_from_slice(&input[..16]);
    Ok((uuid, 16))
}

/// Writes a VarInt-prefixed UTF-8 string into `out`.
pub fn write_string(s: &str, out: &mut Vec<u8>) {
    put_string(s, out);
}

/// Rejects a collection or byte buffer longer than `max`.
fn check_length(length: usize, max: usize) -> Result<(), ProtocolError> {
    if length > max {
        return Err(ProtocolError::StringTooLong { length, max });
    }
    Ok(())
}

/// Rejects a string over `max_chars`, a limit the protocol counts in UTF-16 code units.
fn check_chars(value: &str, max_chars: usize) -> Result<(), ProtocolError> {
    check_length(value.encode_utf16().count(), max_chars)
}

/// Reads a VarInt-prefixed UTF-8 string bounded by `max_chars`.
pub fn read_string(input: &[u8], max_chars: usize) -> Result<(String, usize), ProtocolError> {
    let max_bytes = max_chars.saturating_mul(3).min(i32::MAX as usize);
    let (s, rest) = java_wire::read_string(input, max_bytes).map_err(|err| match err {
        WireError::Truncated => ProtocolError::UnexpectedEof,
        WireError::VarIntTooLong => ProtocolError::MalformedVarInt,
        WireError::NonCanonicalVarInt => ProtocolError::NonCanonicalVarInt,
        WireError::TooLong { length, .. } if length > i32::MAX as usize => {
            ProtocolError::MalformedVarInt
        }
        WireError::TooLong { length, .. } => ProtocolError::StringTooLong {
            length,
            max: max_chars,
        },
        WireError::InvalidText => ProtocolError::InvalidUtf8,
        WireError::BelowCompressionThreshold { length, threshold } => {
            ProtocolError::InvalidPacketLength {
                expected: threshold,
                received: length,
            }
        }
        WireError::InvalidCompression | WireError::DecompressedSizeMismatch { .. } => {
            ProtocolError::MalformedVarInt
        }
    })?;
    check_chars(s, max_chars)?;
    Ok((s.to_owned(), input.len() - rest.len()))
}

/// Writes a VarInt-prefixed byte array into `out`.
pub fn write_byte_array(bytes: &[u8], out: &mut Vec<u8>) {
    write_varint(bytes.len() as i32, out);
    out.extend_from_slice(bytes);
}

/// Reads a VarInt-prefixed byte array bounded by `max_len`.
pub fn read_byte_array(input: &[u8], max_len: usize) -> Result<(Vec<u8>, usize), ProtocolError> {
    let (byte_len, offset) = read_varint(input)?;
    if byte_len < 0 {
        return Err(ProtocolError::MalformedVarInt);
    }
    let byte_len = byte_len as usize;
    if byte_len > max_len {
        return Err(ProtocolError::StringTooLong {
            length: byte_len,
            max: max_len,
        });
    }
    let total = offset + byte_len;
    if input.len() < total {
        return Err(ProtocolError::UnexpectedEof);
    }
    Ok((input[offset..total].to_vec(), total))
}

/// Helper to skip the payload of an NBT tag by type ID, validating its structure.
pub fn skip_nbt_tag_payload(tag_type: u8, input: &[u8]) -> Result<usize, ProtocolError> {
    skip_nbt_payload(tag_type, input, 0)
}

/// Rejects a nested tag past [`MAX_NBT_DEPTH`] before it can recurse further.
fn check_nbt_depth(depth: usize) -> Result<(), ProtocolError> {
    if depth > MAX_NBT_DEPTH {
        return Err(ProtocolError::NbtDepthExceeded {
            depth,
            max: MAX_NBT_DEPTH,
        });
    }
    Ok(())
}

/// Reads a TAG_String payload: an unsigned 16-bit byte length and modified UTF-8.
fn read_nbt_string(input: &[u8]) -> Result<(String, usize), ProtocolError> {
    java_wire::read_nbt_string(input).map_err(|err| match err {
        WireError::Truncated => ProtocolError::UnexpectedEof,
        _ => ProtocolError::InvalidUtf8,
    })
}

fn skip_nbt_payload(tag_type: u8, input: &[u8], depth: usize) -> Result<usize, ProtocolError> {
    match tag_type {
        1 => {
            // TAG_Byte
            if input.is_empty() {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(1)
        }
        2 => {
            // TAG_Short
            if input.len() < 2 {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(2)
        }
        3 => {
            // TAG_Int
            if input.len() < 4 {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(4)
        }
        4 => {
            // TAG_Long
            if input.len() < 8 {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(8)
        }
        5 => {
            // TAG_Float
            if input.len() < 4 {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(4)
        }
        6 => {
            // TAG_Double
            if input.len() < 8 {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(8)
        }
        7 => {
            // TAG_Byte_Array
            skip_nbt_array(input, 1)
        }
        8 => {
            // TAG_String
            Ok(read_nbt_string(input)?.1)
        }
        9 => {
            // TAG_List
            check_nbt_depth(depth + 1)?;
            let (&elem_type, rest) = input.split_first().ok_or(ProtocolError::UnexpectedEof)?;
            let (count, _) =
                java_wire::read_i32_be(rest).map_err(|_| ProtocolError::UnexpectedEof)?;
            if count < 0 {
                return Err(ProtocolError::InvalidPacketLength {
                    expected: 0,
                    received: count as usize,
                });
            }
            let mut cursor = 5;
            for _ in 0..count {
                let n = skip_nbt_payload(elem_type, &input[cursor..], depth + 1)?;
                cursor += n;
            }
            Ok(cursor)
        }
        10 => {
            // TAG_Compound
            check_nbt_depth(depth + 1)?;
            let mut cursor = 0;
            loop {
                let item_tag = *input.get(cursor).ok_or(ProtocolError::UnexpectedEof)?;
                cursor += 1;
                if item_tag == 0 {
                    break;
                }
                cursor += read_nbt_string(&input[cursor..])?.1;
                cursor += skip_nbt_payload(item_tag, &input[cursor..], depth + 1)?;
            }
            Ok(cursor)
        }
        11 => {
            // TAG_Int_Array
            skip_nbt_array(input, 4)
        }
        12 => {
            // TAG_Long_Array
            skip_nbt_array(input, 8)
        }
        other => Err(ProtocolError::InvalidNbtTag(other)),
    }
}

fn skip_nbt_array(input: &[u8], elem_bytes: usize) -> Result<usize, ProtocolError> {
    let (len, _) = java_wire::read_i32_be(input).map_err(|_| ProtocolError::UnexpectedEof)?;
    if len < 0 {
        return Err(ProtocolError::InvalidPacketLength {
            expected: 0,
            received: len as usize,
        });
    }
    let total = 4 + (len as usize * elem_bytes);
    if input.len() < total {
        return Err(ProtocolError::UnexpectedEof);
    }
    Ok(total)
}

/// Serializes a display name as an anonymous Network NBT text component.
pub fn write_display_name_nbt(
    name: &PlayerDisplayName,
    out: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
    match name {
        PlayerDisplayName::Plain(text) => {
            check_chars(text, MAX_STRING_CHARS)?;
            put_nbt_text(text, out).map_err(|err| match err {
                WireError::TooLong { length, max } => ProtocolError::StringTooLong { length, max },
                _ => ProtocolError::InvalidUtf8,
            })?;
        }
        PlayerDisplayName::RawNbt { raw, .. } => {
            let (_, consumed) = read_display_name_nbt(raw, MAX_STRING_CHARS)?;
            if consumed != raw.len() {
                return Err(ProtocolError::TrailingData {
                    remaining: raw.len() - consumed,
                });
            }
            out.extend_from_slice(raw);
        }
    }
    Ok(())
}

/// Reads an anonymous Network NBT text component display name.
///
/// A TAG_String (0x08) becomes [`PlayerDisplayName::Plain`]. A TAG_Compound (0x0A)
/// is validated and kept byte for byte as [`PlayerDisplayName::RawNbt`], so a later
/// encode cannot lose styling, `extra` children or translations.
pub fn read_display_name_nbt(
    input: &[u8],
    max_chars: usize,
) -> Result<(PlayerDisplayName, usize), ProtocolError> {
    let (tag, body) = input.split_first().ok_or(ProtocolError::UnexpectedEof)?;
    match *tag {
        0x08 => {
            let (text, consumed) = read_nbt_string(body)?;
            check_chars(&text, max_chars)?;
            Ok((PlayerDisplayName::Plain(text), 1 + consumed))
        }
        0x0A => {
            if body.first() == Some(&0) {
                return Err(ProtocolError::UnsupportedComponent);
            }
            let mut text = String::new();
            let consumed = read_component_compound(body, 1, &mut text)?;
            check_chars(&text, max_chars)?;
            let total = 1 + consumed;
            Ok((
                PlayerDisplayName::RawNbt {
                    raw: input[..total].to_vec(),
                    text,
                },
                total,
            ))
        }
        other => Err(ProtocolError::InvalidNbtTag(other)),
    }
}

/// Validates one component compound and appends its literal text to `text`, its
/// own `text` field first and then its `extra` children, whatever the tag order.
fn read_component_compound(
    input: &[u8],
    depth: usize,
    text: &mut String,
) -> Result<usize, ProtocolError> {
    check_nbt_depth(depth)?;
    let mut own = String::new();
    let mut children = String::new();
    let mut cursor = 0;
    loop {
        let tag = *input.get(cursor).ok_or(ProtocolError::UnexpectedEof)?;
        cursor += 1;
        if tag == 0 {
            break;
        }
        let (name, consumed) = read_nbt_string(&input[cursor..])?;
        cursor += consumed;
        cursor += match (tag, name.as_str()) {
            (0x08, "text") => {
                let (value, consumed) = read_nbt_string(&input[cursor..])?;
                own = value;
                consumed
            }
            (0x09, "extra") => read_component_list(&input[cursor..], depth, &mut children)?,
            _ => skip_nbt_payload(tag, &input[cursor..], depth)?,
        };
    }
    text.push_str(&own);
    text.push_str(&children);
    Ok(cursor)
}

/// Validates an `extra` list and appends the literal text of its children.
fn read_component_list(
    input: &[u8],
    depth: usize,
    text: &mut String,
) -> Result<usize, ProtocolError> {
    check_nbt_depth(depth + 1)?;
    let (element, rest) = input.split_first().ok_or(ProtocolError::UnexpectedEof)?;
    let (count, _) = rest
        .split_first_chunk::<4>()
        .ok_or(ProtocolError::UnexpectedEof)?;
    let count = i32::from_be_bytes(*count);
    if count < 0 {
        return Err(ProtocolError::MalformedVarInt);
    }
    let mut cursor = 5;
    for _ in 0..count {
        cursor += match *element {
            0x08 => {
                let (value, consumed) = read_nbt_string(&input[cursor..])?;
                text.push_str(&value);
                consumed
            }
            0x0A => read_component_compound(&input[cursor..], depth + 1, text)?,
            other => skip_nbt_payload(other, &input[cursor..], depth + 1)?,
        };
    }
    Ok(cursor)
}

#[cfg(test)]
mod tests;
