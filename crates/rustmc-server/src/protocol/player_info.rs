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

/// Encodes a UTF-8 string slice into Java Modified UTF-8 (MUTF-8) bytes.
///
/// In Java MUTF-8:
/// - U+0000 is encoded as 2 bytes: `0xC0 0x80`.
/// - 1-byte ASCII (U+0001..=U+007F), 2-byte (U+0080..=U+07FF), and 3-byte (U+0800..=U+FFFF)
///   use standard UTF-8 encodings.
/// - Supplementary code points (U+010000..=U+10FFFF) are encoded as a UTF-16 surrogate pair,
///   each encoded as a 3-byte sequence (total 6 bytes).
pub fn encode_mutf8(s: &str, out: &mut Vec<u8>) {
    for c in s.chars() {
        let cp = c as u32;
        if cp == 0 {
            out.push(0xC0);
            out.push(0x80);
        } else if cp <= 0x7F {
            out.push(cp as u8);
        } else if cp <= 0x7FF {
            out.push((0xC0 | ((cp >> 6) & 0x1F)) as u8);
            out.push((0x80 | (cp & 0x3F)) as u8);
        } else if cp <= 0xFFFF {
            out.push((0xE0 | ((cp >> 12) & 0x0F)) as u8);
            out.push((0x80 | ((cp >> 6) & 0x3F)) as u8);
            out.push((0x80 | (cp & 0x3F)) as u8);
        } else {
            let temp = cp - 0x10000;
            let high = 0xD800 + (temp >> 10);
            let low = 0xDC00 + (temp & 0x3FF);
            // High surrogate (0xD800..=0xDBFF)
            out.push((0xE0 | ((high >> 12) & 0x0F)) as u8);
            out.push((0x80 | ((high >> 6) & 0x3F)) as u8);
            out.push((0x80 | (high & 0x3F)) as u8);
            // Low surrogate (0xDC00..=0xDFFF)
            out.push((0xE0 | ((low >> 12) & 0x0F)) as u8);
            out.push((0x80 | ((low >> 6) & 0x3F)) as u8);
            out.push((0x80 | (low & 0x3F)) as u8);
        }
    }
}

/// Decodes Java Modified UTF-8 (MUTF-8) bytes into a Rust String.
pub fn decode_mutf8(bytes: &[u8]) -> Result<String, ProtocolError> {
    let mut s = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b1 = bytes[i];
        if b1 == 0 {
            return Err(ProtocolError::InvalidUtf8);
        }
        if b1 <= 0x7F {
            s.push(b1 as char);
            i += 1;
        } else if b1 & 0xE0 == 0xC0 {
            if i + 1 >= bytes.len() {
                return Err(ProtocolError::InvalidUtf8);
            }
            let b2 = bytes[i + 1];
            if b2 & 0xC0 != 0x80 {
                return Err(ProtocolError::InvalidUtf8);
            }
            let cp = (((b1 & 0x1F) as u32) << 6) | ((b2 & 0x3F) as u32);
            if b1 == 0xC0 && b2 == 0x80 {
                s.push('\0');
            } else if cp < 0x80 {
                return Err(ProtocolError::InvalidUtf8);
            } else {
                let ch = char::from_u32(cp).ok_or(ProtocolError::InvalidUtf8)?;
                s.push(ch);
            }
            i += 2;
        } else if b1 & 0xF0 == 0xE0 {
            if i + 2 >= bytes.len() {
                return Err(ProtocolError::InvalidUtf8);
            }
            let b2 = bytes[i + 1];
            let b3 = bytes[i + 2];
            if (b2 & 0xC0 != 0x80) || (b3 & 0xC0 != 0x80) {
                return Err(ProtocolError::InvalidUtf8);
            }
            let code =
                (((b1 & 0x0F) as u32) << 12) | (((b2 & 0x3F) as u32) << 6) | ((b3 & 0x3F) as u32);

            if (0xD800..=0xDBFF).contains(&code) {
                if i + 5 >= bytes.len() {
                    return Err(ProtocolError::InvalidUtf8);
                }
                let b4 = bytes[i + 3];
                let b5 = bytes[i + 4];
                let b6 = bytes[i + 5];
                if (b4 & 0xF0 != 0xE0) || (b5 & 0xC0 != 0x80) || (b6 & 0xC0 != 0x80) {
                    return Err(ProtocolError::InvalidUtf8);
                }
                let low = (((b4 & 0x0F) as u32) << 12)
                    | (((b5 & 0x3F) as u32) << 6)
                    | ((b6 & 0x3F) as u32);
                if !(0xDC00..=0xDFFF).contains(&low) {
                    return Err(ProtocolError::InvalidUtf8);
                }
                let cp = 0x10000 + (((code - 0xD800) << 10) | (low - 0xDC00));
                let ch = char::from_u32(cp).ok_or(ProtocolError::InvalidUtf8)?;
                s.push(ch);
                i += 6;
            } else if (0xDC00..=0xDFFF).contains(&code) || code < 0x800 {
                return Err(ProtocolError::InvalidUtf8);
            } else {
                let ch = char::from_u32(code).ok_or(ProtocolError::InvalidUtf8)?;
                s.push(ch);
                i += 3;
            }
        } else {
            return Err(ProtocolError::InvalidUtf8);
        }
    }
    Ok(s)
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
                    if payload[cursor..].len() < 8 {
                        return Err(ProtocolError::UnexpectedEof);
                    }
                    let expires_at =
                        i64::from_be_bytes(payload[cursor..cursor + 8].try_into().unwrap());
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
    let mut u = value as u32;
    loop {
        let mut byte = (u & 0x7F) as u8;
        u >>= 7;
        if u != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if u == 0 {
            break;
        }
    }
}

/// Decodes a 32-bit signed integer from a canonical VarInt stream.
///
/// Rejects non-canonical encodings with superfluous trailing zero bytes (e.g. `[0x80, 0x00]`)
/// and malformed sequences exceeding 5 bytes or containing illegal upper bits.
pub fn read_varint(input: &[u8]) -> Result<(i32, usize), ProtocolError> {
    let mut value = 0u32;
    for (i, &byte) in input.iter().take(5).enumerate() {
        if i == 4 && (byte & 0xF0) != 0 {
            return Err(ProtocolError::MalformedVarInt);
        }
        value |= u32::from(byte & 0x7F) << (i * 7);
        if (byte & 0x80) == 0 {
            if i > 0 && byte == 0 {
                return Err(ProtocolError::NonCanonicalVarInt);
            }
            return Ok((value as i32, i + 1));
        }
    }
    if input.len() >= 5 {
        Err(ProtocolError::MalformedVarInt)
    } else {
        Err(ProtocolError::UnexpectedEof)
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
    write_varint(s.len() as i32, out);
    out.extend_from_slice(s.as_bytes());
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
    let (byte_len, offset) = read_varint(input)?;
    if byte_len < 0 {
        return Err(ProtocolError::MalformedVarInt);
    }
    let byte_len = byte_len as usize;
    if byte_len > max_chars * 3 {
        return Err(ProtocolError::StringTooLong {
            length: byte_len,
            max: max_chars,
        });
    }
    let total = offset + byte_len;
    if input.len() < total {
        return Err(ProtocolError::UnexpectedEof);
    }
    let slice = &input[offset..total];
    let s = std::str::from_utf8(slice).map_err(|_| ProtocolError::InvalidUtf8)?;
    check_chars(s, max_chars)?;
    Ok((s.to_owned(), total))
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
    let (length, rest) = input
        .split_first_chunk::<2>()
        .ok_or(ProtocolError::UnexpectedEof)?;
    let length = usize::from(u16::from_be_bytes(*length));
    let bytes = rest.get(..length).ok_or(ProtocolError::UnexpectedEof)?;
    Ok((decode_mutf8(bytes)?, 2 + length))
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
            if input.len() < 4 {
                return Err(ProtocolError::UnexpectedEof);
            }
            let len = i32::from_be_bytes(input[0..4].try_into().unwrap());
            if len < 0 {
                return Err(ProtocolError::InvalidPacketLength {
                    expected: 0,
                    received: len as usize,
                });
            }
            let total = 4 + len as usize;
            if input.len() < total {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(total)
        }
        8 => {
            // TAG_String
            Ok(read_nbt_string(input)?.1)
        }
        9 => {
            // TAG_List
            check_nbt_depth(depth + 1)?;
            if input.len() < 5 {
                return Err(ProtocolError::UnexpectedEof);
            }
            let elem_type = input[0];
            let count = i32::from_be_bytes(input[1..5].try_into().unwrap());
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
            if input.len() < 4 {
                return Err(ProtocolError::UnexpectedEof);
            }
            let len = i32::from_be_bytes(input[0..4].try_into().unwrap());
            if len < 0 {
                return Err(ProtocolError::InvalidPacketLength {
                    expected: 0,
                    received: len as usize,
                });
            }
            let total = 4 + (len as usize * 4);
            if input.len() < total {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(total)
        }
        12 => {
            // TAG_Long_Array
            if input.len() < 4 {
                return Err(ProtocolError::UnexpectedEof);
            }
            let len = i32::from_be_bytes(input[0..4].try_into().unwrap());
            if len < 0 {
                return Err(ProtocolError::InvalidPacketLength {
                    expected: 0,
                    received: len as usize,
                });
            }
            let total = 4 + (len as usize * 8);
            if input.len() < total {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(total)
        }
        other => Err(ProtocolError::InvalidNbtTag(other)),
    }
}

/// Serializes a display name as an anonymous Network NBT text component.
pub fn write_display_name_nbt(
    name: &PlayerDisplayName,
    out: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
    match name {
        PlayerDisplayName::Plain(text) => {
            check_chars(text, MAX_STRING_CHARS)?;
            let mut encoded = Vec::with_capacity(text.len());
            encode_mutf8(text, &mut encoded);
            // The NBT limit applies to encoded bytes, not to the source string.
            let byte_len =
                u16::try_from(encoded.len()).map_err(|_| ProtocolError::StringTooLong {
                    length: encoded.len(),
                    max: u16::MAX as usize,
                })?;
            out.push(0x08); // TAG_String
            out.extend_from_slice(&byte_len.to_be_bytes());
            out.extend_from_slice(&encoded);
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
mod tests {
    use super::*;

    fn sample_uuid(val: u8) -> [u8; 16] {
        [val; 16]
    }

    fn sample_entry(id: u8, name: &str) -> PlayerInfoEntry {
        PlayerInfoEntry {
            uuid: sample_uuid(id),
            name: name.to_owned(),
            properties: vec![
                ProfileProperty::signed("textures", "sample_texture_payload", "sample_sig"),
                ProfileProperty::unsigned("custom_badge", "developer"),
            ],
            chat_session: Some(ChatSessionData::new(
                sample_uuid(0xAA),
                1_700_000_000_000,
                vec![0x01, 0x02, 0x03, 0x04],
                vec![0x0A, 0x0B, 0x0C, 0x0D],
            )),
            game_mode: GameMode::Creative,
            listed: true,
            latency: 42,
            display_name: Some(format!("§6{name}§r").into()),
            list_order: 10,
            show_hat: true,
        }
    }

    #[test]
    fn test_action_bitflags_constants_and_operations() {
        assert_eq!(PlayerInfoAction::AddPlayer.flag(), 0x01);
        assert_eq!(PlayerInfoAction::InitializeChat.flag(), 0x02);
        assert_eq!(PlayerInfoAction::UpdateGameMode.flag(), 0x04);
        assert_eq!(PlayerInfoAction::UpdateListed.flag(), 0x08);
        assert_eq!(PlayerInfoAction::UpdateLatency.flag(), 0x10);
        assert_eq!(PlayerInfoAction::UpdateDisplayName.flag(), 0x20);
        assert_eq!(PlayerInfoAction::UpdateListOrder.flag(), 0x40);
        assert_eq!(PlayerInfoAction::UpdateHat.flag(), 0x80);

        let combined = PlayerInfoAction::AddPlayer | PlayerInfoAction::UpdateLatency;
        assert_eq!(combined, 0x11);
        assert!(PlayerInfoAction::is_set(
            combined,
            PlayerInfoAction::AddPlayer
        ));
        assert!(PlayerInfoAction::is_set(
            combined,
            PlayerInfoAction::UpdateLatency
        ));
        assert!(!PlayerInfoAction::is_set(
            combined,
            PlayerInfoAction::UpdateGameMode
        ));

        let all = PlayerInfoAction::ALL;
        assert_eq!(all, 0xFF);
        for action in [
            PlayerInfoAction::AddPlayer,
            PlayerInfoAction::InitializeChat,
            PlayerInfoAction::UpdateGameMode,
            PlayerInfoAction::UpdateListed,
            PlayerInfoAction::UpdateLatency,
            PlayerInfoAction::UpdateDisplayName,
            PlayerInfoAction::UpdateListOrder,
            PlayerInfoAction::UpdateHat,
        ] {
            assert!(PlayerInfoAction::is_set(all, action));
            assert_eq!(PlayerInfoAction::try_from(action.flag()).unwrap(), action);
        }

        assert_eq!(
            PlayerInfoAction::try_from(0x00).unwrap_err(),
            ProtocolError::InvalidActionBitmask(0x00)
        );
    }

    #[test]
    fn test_game_mode_mapping() {
        assert_eq!(GameMode::from_id(0).unwrap(), GameMode::Survival);
        assert_eq!(GameMode::from_id(1).unwrap(), GameMode::Creative);
        assert_eq!(GameMode::from_id(2).unwrap(), GameMode::Adventure);
        assert_eq!(GameMode::from_id(3).unwrap(), GameMode::Spectator);
        assert_eq!(
            GameMode::from_id(4).unwrap_err(),
            ProtocolError::InvalidGameMode(4)
        );
        assert_eq!(
            GameMode::from_id(-1).unwrap_err(),
            ProtocolError::InvalidGameMode(-1)
        );
    }

    #[test]
    fn test_roundtrip_all_actions_full_entry() {
        let entry1 = sample_entry(1, "Alice");
        let mut entry2 = sample_entry(2, "Bob");
        entry2.chat_session = None;
        entry2.display_name = None;
        entry2.properties.clear();
        entry2.game_mode = GameMode::Spectator;
        entry2.listed = false;
        entry2.latency = 120;

        let packet = PlayerInfoUpdatePacket::new(PlayerInfoAction::ALL, vec![entry1, entry2]);
        let encoded = packet.encode().expect("encode should succeed");
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).expect("decode should succeed");
        assert_eq!(packet, decoded);
    }

    #[test]
    fn test_roundtrip_individual_actions() {
        let uuid = sample_uuid(7);

        // 1. AddPlayer only
        let entry = PlayerInfoEntry {
            uuid,
            name: "Steve".to_owned(),
            properties: vec![ProfileProperty::unsigned("tex", "val")],
            ..Default::default()
        };
        let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::AddPlayer as u8, entry);
        let encoded = packet.encode().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
        assert_eq!(decoded.actions, PlayerInfoAction::AddPlayer as u8);
        assert_eq!(decoded.entries.len(), 1);
        assert_eq!(decoded.entries[0].name, "Steve");
        assert_eq!(decoded.entries[0].properties.len(), 1);

        // 2. InitializeChat only (with chat session)
        let entry = PlayerInfoEntry {
            uuid,
            chat_session: Some(ChatSessionData::new(
                sample_uuid(99),
                987654321,
                vec![1, 2, 3],
                vec![4, 5, 6],
            )),
            ..Default::default()
        };
        let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::InitializeChat as u8, entry);
        let encoded = packet.encode().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
        assert_eq!(decoded.actions, PlayerInfoAction::InitializeChat as u8);
        assert!(decoded.entries[0].chat_session.is_some());

        // 3. InitializeChat only (with None)
        let entry = PlayerInfoEntry {
            uuid,
            chat_session: None,
            ..Default::default()
        };
        let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::InitializeChat as u8, entry);
        let encoded = packet.encode().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
        assert_eq!(decoded.actions, PlayerInfoAction::InitializeChat as u8);
        assert!(decoded.entries[0].chat_session.is_none());

        // 4. UpdateGameMode only
        for mode in [
            GameMode::Survival,
            GameMode::Creative,
            GameMode::Adventure,
            GameMode::Spectator,
        ] {
            let entry = PlayerInfoEntry {
                uuid,
                game_mode: mode,
                ..Default::default()
            };
            let packet =
                PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateGameMode as u8, entry);
            let encoded = packet.encode().unwrap();
            let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
            assert_eq!(decoded.entries[0].game_mode, mode);
        }

        // 5. UpdateListed only
        for listed in [true, false] {
            let entry = PlayerInfoEntry {
                uuid,
                listed,
                ..Default::default()
            };
            let packet =
                PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateListed as u8, entry);
            let encoded = packet.encode().unwrap();
            let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
            assert_eq!(decoded.entries[0].listed, listed);
        }

        // 6. UpdateLatency only
        for lat in [0, 50, 999, -1, 3500] {
            let entry = PlayerInfoEntry {
                uuid,
                latency: lat,
                ..Default::default()
            };
            let packet =
                PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateLatency as u8, entry);
            let encoded = packet.encode().unwrap();
            let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
            assert_eq!(decoded.entries[0].latency, lat);
        }

        // 7. UpdateDisplayName only (Some and None)
        let entry = PlayerInfoEntry {
            uuid,
            display_name: Some("Custom Nick".into()),
            ..Default::default()
        };
        let packet =
            PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateDisplayName as u8, entry);
        let encoded = packet.encode().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
        assert_eq!(
            decoded.entries[0].display_name.as_deref(),
            Some("Custom Nick")
        );

        let entry = PlayerInfoEntry {
            uuid,
            display_name: None,
            ..Default::default()
        };
        let packet =
            PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateDisplayName as u8, entry);
        let encoded = packet.encode().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
        assert_eq!(decoded.entries[0].display_name, None);

        // 8. UpdateListOrder only
        for order in [0, 1, 999, -50] {
            let entry = PlayerInfoEntry {
                uuid,
                list_order: order,
                ..Default::default()
            };
            let packet =
                PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateListOrder as u8, entry);
            let encoded = packet.encode().unwrap();
            let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
            assert_eq!(decoded.entries[0].list_order, order);
        }

        // 9. UpdateHat only
        for hat in [true, false] {
            let entry = PlayerInfoEntry {
                uuid,
                show_hat: hat,
                ..Default::default()
            };
            let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateHat as u8, entry);
            let encoded = packet.encode().unwrap();
            let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
            assert_eq!(decoded.entries[0].show_hat, hat);
        }
    }

    #[test]
    fn test_all_256_bitmask_combinations_roundtrip() {
        for mask in 0u8..=0xFF {
            let entry = sample_entry(mask, "Tester");
            let packet = PlayerInfoUpdatePacket::new(mask, vec![entry]);
            let encoded = packet.encode().expect("encoding must succeed");
            let decoded = PlayerInfoUpdatePacket::decode(&encoded).expect("decoding must succeed");
            assert_eq!(decoded.actions, mask);
            assert_eq!(decoded.entries.len(), 1);
            assert_eq!(decoded.entries[0].uuid, sample_uuid(mask));

            if PlayerInfoAction::is_set(mask, PlayerInfoAction::AddPlayer) {
                assert_eq!(decoded.entries[0].name, "Tester");
                assert_eq!(decoded.entries[0].properties.len(), 2);
            }
            if PlayerInfoAction::is_set(mask, PlayerInfoAction::InitializeChat) {
                assert!(decoded.entries[0].chat_session.is_some());
            }
            if PlayerInfoAction::is_set(mask, PlayerInfoAction::UpdateGameMode) {
                assert_eq!(decoded.entries[0].game_mode, GameMode::Creative);
            }
            if PlayerInfoAction::is_set(mask, PlayerInfoAction::UpdateListed) {
                assert!(decoded.entries[0].listed);
            }
            if PlayerInfoAction::is_set(mask, PlayerInfoAction::UpdateLatency) {
                assert_eq!(decoded.entries[0].latency, 42);
            }
            if PlayerInfoAction::is_set(mask, PlayerInfoAction::UpdateDisplayName) {
                assert_eq!(
                    decoded.entries[0].display_name.as_deref(),
                    Some("§6Tester§r")
                );
            }
            if PlayerInfoAction::is_set(mask, PlayerInfoAction::UpdateListOrder) {
                assert_eq!(decoded.entries[0].list_order, 10);
            }
            if PlayerInfoAction::is_set(mask, PlayerInfoAction::UpdateHat) {
                assert!(decoded.entries[0].show_hat);
            }
        }
    }

    #[test]
    fn test_non_canonical_and_malformed_inputs() {
        // 1. Non-canonical VarInt with trailing zero [0x80, 0x00]
        let non_canonical = [0x80, 0x00];
        assert_eq!(
            read_varint(&non_canonical).unwrap_err(),
            ProtocolError::NonCanonicalVarInt
        );

        // 2. Malformed 6-byte VarInt
        let malformed = [0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        assert_eq!(
            read_varint(&malformed).unwrap_err(),
            ProtocolError::MalformedVarInt
        );

        // 3. VarInt 5th byte with overflow bits
        let overflow = [0x80, 0x80, 0x80, 0x80, 0xF0];
        assert_eq!(
            read_varint(&overflow).unwrap_err(),
            ProtocolError::MalformedVarInt
        );

        // 4. Unexpected EOF on empty input
        assert_eq!(
            PlayerInfoUpdatePacket::decode(&[]).unwrap_err(),
            ProtocolError::UnexpectedEof
        );

        // 5. Unexpected packet ID (0x3F instead of 0x47)
        let mut wrong_packet = Vec::new();
        write_varint(2, &mut wrong_packet); // Frame len = 2
        write_varint(0x3F, &mut wrong_packet); // Wrong packet ID 0x3F
        wrong_packet.push(0x00); // Empty actions
        assert_eq!(
            PlayerInfoUpdatePacket::decode(&wrong_packet).unwrap_err(),
            ProtocolError::UnexpectedPacketId {
                expected: 0x47,
                received: 0x3F,
            }
        );

        // 6. Action bitmask TryFrom rejects invalid single flags (0x00, 0x43)
        assert_eq!(
            PlayerInfoAction::try_from(0x00).unwrap_err(),
            ProtocolError::InvalidActionBitmask(0x00)
        );
        assert_eq!(
            PlayerInfoAction::try_from(0x43).unwrap_err(),
            ProtocolError::InvalidActionBitmask(0x43)
        );

        // 7. Invalid boolean (byte 0x02)
        assert_eq!(
            read_bool(&[0x02]).unwrap_err(),
            ProtocolError::InvalidBoolean(0x02)
        );

        // 8. Trailing data after framed packet
        let valid_packet = PlayerInfoUpdatePacket::new(0, vec![]);
        let mut framed_with_trailing = valid_packet.encode().unwrap();
        framed_with_trailing.push(0xFF);
        assert_eq!(
            PlayerInfoUpdatePacket::decode(&framed_with_trailing).unwrap_err(),
            ProtocolError::TrailingData { remaining: 1 }
        );

        // 9. Name string exceeding 16 chars
        let long_name_entry = PlayerInfoEntry {
            uuid: sample_uuid(1),
            name: "SuperLongPlayerNameExceeding16".to_owned(),
            ..Default::default()
        };
        let packet =
            PlayerInfoUpdatePacket::single(PlayerInfoAction::AddPlayer as u8, long_name_entry);
        assert!(matches!(
            packet.encode().unwrap_err(),
            ProtocolError::StringTooLong { max: 16, .. }
        ));
    }

    #[test]
    fn test_tab_list_tracker_state_transitions() {
        let mut tracker = TabListTracker::new();
        assert_eq!(tracker.len(), 0);
        assert!(tracker.is_empty());

        let uuid1 = sample_uuid(1);
        let uuid2 = sample_uuid(2);

        // 1. Add player 1
        let entry1 = sample_entry(1, "PlayerOne");
        let add_packet1 = tracker.add_player(entry1);
        assert_eq!(tracker.len(), 1);
        assert!(!tracker.is_empty());
        assert!(tracker.contains_player(&uuid1));
        assert_eq!(add_packet1.entries.len(), 1);
        assert_eq!(add_packet1.entries[0].name, "PlayerOne");
        assert!(PlayerInfoAction::is_set(
            add_packet1.actions,
            PlayerInfoAction::AddPlayer
        ));

        // 2. Add player 2
        let mut entry2 = sample_entry(2, "PlayerTwo");
        entry2.chat_session = None;
        let add_packet2 = tracker.add_player(entry2);
        assert_eq!(tracker.len(), 2);
        assert!(tracker.contains_player(&uuid2));
        assert_eq!(add_packet2.entries[0].name, "PlayerTwo");
        assert!(!PlayerInfoAction::is_set(
            add_packet2.actions,
            PlayerInfoAction::InitializeChat
        ));

        // 3. Update latency
        let lat_packet = tracker
            .update_latency(&uuid1, 95)
            .expect("update latency should succeed");
        assert_eq!(lat_packet.actions, PlayerInfoAction::UpdateLatency as u8);
        assert_eq!(lat_packet.entries[0].uuid, uuid1);
        assert_eq!(lat_packet.entries[0].latency, 95);
        assert_eq!(tracker.get_player(&uuid1).unwrap().latency, 95);

        // Latency update for unknown player fails gracefully
        assert!(tracker.update_latency(&sample_uuid(99), 10).is_none());

        // 4. Update game mode
        let gm_packet = tracker
            .update_game_mode(&uuid1, GameMode::Adventure)
            .expect("update game mode should succeed");
        assert_eq!(gm_packet.actions, PlayerInfoAction::UpdateGameMode as u8);
        assert_eq!(gm_packet.entries[0].game_mode, GameMode::Adventure);
        assert_eq!(
            tracker.get_player(&uuid1).unwrap().game_mode,
            GameMode::Adventure
        );

        // Game mode update for unknown player fails
        assert!(
            tracker
                .update_game_mode(&sample_uuid(99), GameMode::Survival)
                .is_none()
        );

        // 5. Update display name
        let dn_packet = tracker
            .update_display_name(&uuid1, Some("MasterOne".into()))
            .expect("update display name should succeed");
        assert_eq!(dn_packet.actions, PlayerInfoAction::UpdateDisplayName as u8);
        assert_eq!(
            dn_packet.entries[0].display_name.as_deref(),
            Some("MasterOne")
        );
        assert_eq!(
            tracker.get_player(&uuid1).unwrap().display_name.as_deref(),
            Some("MasterOne")
        );

        // Clear display name to None
        let dn_clear_packet = tracker
            .update_display_name(&uuid1, None)
            .expect("clearing display name should succeed");
        assert_eq!(dn_clear_packet.entries[0].display_name, None);
        assert_eq!(tracker.get_player(&uuid1).unwrap().display_name, None);

        // 6. Update listed status
        let listed_packet = tracker
            .update_listed(&uuid2, false)
            .expect("update listed should succeed");
        assert_eq!(listed_packet.actions, PlayerInfoAction::UpdateListed as u8);
        assert!(!listed_packet.entries[0].listed);
        assert!(!tracker.get_player(&uuid2).unwrap().listed);

        // 7. Full tab list packet
        let full_sync = tracker.full_tab_list_packet();
        assert_eq!(full_sync.actions, PlayerInfoAction::ALL);
        assert_eq!(full_sync.entries.len(), 2);
        // Deterministic order by UUID (uuid1 < uuid2)
        assert_eq!(full_sync.entries[0].uuid, uuid1);
        assert_eq!(full_sync.entries[1].uuid, uuid2);

        // Full sync can be encoded and decoded cleanly
        let encoded_sync = full_sync.encode().unwrap();
        let decoded_sync = PlayerInfoUpdatePacket::decode(&encoded_sync).unwrap();
        assert_eq!(full_sync, decoded_sync);

        // 8. Remove player
        let removed1 = tracker
            .remove_player(&uuid1)
            .expect("player 1 should be removed");
        assert_eq!(removed1.uuid, uuid1);
        assert_eq!(tracker.len(), 1);
        assert!(!tracker.contains_player(&uuid1));

        // Removing already-removed player returns None
        assert!(tracker.remove_player(&uuid1).is_none());

        // Remove player 2
        let removed2 = tracker
            .remove_player(&uuid2)
            .expect("player 2 should be removed");
        assert_eq!(removed2.uuid, uuid2);
        assert_eq!(tracker.len(), 0);
        assert!(tracker.is_empty());
    }

    #[test]
    fn test_unframed_payload_codec_direct() {
        let entry = sample_entry(42, "DirectCodec");
        let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::ALL, entry);
        let payload = packet.encode_payload().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode_payload(&payload).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn test_empty_entries_packet_roundtrip() {
        let packet = PlayerInfoUpdatePacket::new(PlayerInfoAction::ALL, vec![]);
        let encoded = packet.encode().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();
        assert_eq!(packet, decoded);
        assert_eq!(decoded.entries.len(), 0);
    }

    #[test]
    fn test_string_boundary_length_16_vs_17() {
        let name_16 = "1234567890123456";
        assert_eq!(name_16.len(), 16);
        let entry_ok = PlayerInfoEntry::new(sample_uuid(1), name_16);
        let packet_ok = PlayerInfoUpdatePacket::single(PlayerInfoAction::AddPlayer as u8, entry_ok);
        assert!(packet_ok.encode().is_ok());

        let name_17 = "12345678901234567";
        assert_eq!(name_17.len(), 17);
        let entry_err = PlayerInfoEntry::new(sample_uuid(2), name_17);
        let packet_err =
            PlayerInfoUpdatePacket::single(PlayerInfoAction::AddPlayer as u8, entry_err);
        assert!(matches!(
            packet_err.encode().unwrap_err(),
            ProtocolError::StringTooLong {
                length: 17,
                max: 16
            }
        ));
    }

    #[test]
    fn test_invalid_utf8_in_string_rejected() {
        // Construct payload with an invalid UTF-8 byte sequence (0xFF, 0xFE)
        let invalid_utf8_bytes = [0xFF, 0xFE];
        let mut raw_str = Vec::new();
        write_varint(invalid_utf8_bytes.len() as i32, &mut raw_str);
        raw_str.extend_from_slice(&invalid_utf8_bytes);

        let res = read_string(&raw_str, 16);
        assert_eq!(res.unwrap_err(), ProtocolError::InvalidUtf8);
    }

    #[test]
    fn test_tracker_getters_and_clear() {
        let mut tracker = TabListTracker::new();
        let e1 = sample_entry(10, "User10");
        let e2 = sample_entry(20, "User20");
        tracker.add_player(e1);
        tracker.add_player(e2);

        assert_eq!(tracker.len(), 2);
        assert_eq!(tracker.get_player(&sample_uuid(10)).unwrap().name, "User10");
        assert_eq!(tracker.get_player(&sample_uuid(20)).unwrap().name, "User20");

        // Mutate player in-place
        if let Some(p) = tracker.get_player_mut(&sample_uuid(10)) {
            p.latency = 999;
        }
        assert_eq!(tracker.get_player(&sample_uuid(10)).unwrap().latency, 999);

        // Iterate over players
        let names: Vec<&str> = tracker.players().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["User10", "User20"]);

        // Clear
        tracker.clear();
        assert_eq!(tracker.len(), 0);
        assert!(tracker.is_empty());
    }

    #[test]
    fn test_chat_session_key_and_signature_exact_preservation() {
        let pub_key = vec![0x30, 0x82, 0x01, 0x22, 0x30, 0x0D, 0x06, 0x09];
        let key_sig = vec![0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00, 0x11];
        let chat = ChatSessionData::new(
            sample_uuid(0x55),
            1_234_567_890_123,
            pub_key.clone(),
            key_sig.clone(),
        );

        let mut entry = PlayerInfoEntry::new(sample_uuid(1), "CryptoUser");
        entry.chat_session = Some(chat);

        let packet = PlayerInfoUpdatePacket::single(PlayerInfoAction::InitializeChat as u8, entry);
        let encoded = packet.encode().unwrap();
        let decoded = PlayerInfoUpdatePacket::decode(&encoded).unwrap();

        let decoded_chat = decoded.entries[0].chat_session.as_ref().unwrap();
        assert_eq!(decoded_chat.session_id, sample_uuid(0x55));
        assert_eq!(decoded_chat.expires_at, 1_234_567_890_123);
        assert_eq!(decoded_chat.public_key, pub_key);
        assert_eq!(decoded_chat.key_signature, key_sig);
    }

    #[test]
    fn test_independent_protocol_777_player_info_update_fixture() {
        // Independently constructed framed fixture for Java 26.3 protocol 777 PlayerInfoUpdate:
        // Frame length: 27 bytes (0x1B)
        // Packet ID: VarInt 0x47 (clientbound PlayerInfoUpdate)
        // Actions bitmask: 0x20 (UpdateDisplayName)
        // Number of entries: VarInt 1 (0x01)
        // UUID: 16 bytes [0x07; 16]
        // has_display_name: 0x01 (true)
        // Network NBT TAG_String (0x08):
        //   tag type: 0x08
        //   length (u16 BE): 0x00, 0x04
        //   string: b"Nick"
        let mut fixture = Vec::new();
        // Frame length = 1 (packet ID 0x47) + 1 (action 0x20) + 1 (count 1) + 16 (UUID) + 1 (has_dn) + 1 (tag 0x08) + 2 (len 4) + 4 (Nick) = 27 bytes
        write_varint(27, &mut fixture);
        write_varint(0x47, &mut fixture); // Packet ID 0x47
        fixture.push(0x20); // Action: UpdateDisplayName
        write_varint(1, &mut fixture); // 1 entry
        fixture.extend_from_slice(&[0x07; 16]); // UUID
        fixture.push(0x01); // has_display_name = true
        fixture.push(0x08); // TAG_String
        fixture.extend_from_slice(&[0x00, 0x04]); // length 4
        fixture.extend_from_slice(b"Nick"); // string "Nick"

        let packet = PlayerInfoUpdatePacket::decode(&fixture)
            .expect("fixture decoding must succeed with packet ID 0x47");
        assert_eq!(packet.actions, 0x20);
        assert_eq!(packet.entries.len(), 1);
        assert_eq!(packet.entries[0].uuid, [0x07; 16]);
        assert_eq!(packet.entries[0].display_name.as_deref(), Some("Nick"));
    }

    #[test]
    fn test_network_nbt_compound_display_name_decoding() {
        // Independently constructed framed fixture with anonymous TAG_Compound display name:
        // Actions: 0x20
        // has_display_name: true
        // Anonymous TAG_Compound (0x0A):
        //   TAG_String (0x08)
        //   name length (u16 BE): 0x00, 0x04
        //   name: "text"
        //   string length (u16 BE): 0x00, 0x05
        //   value: "Hello"
        //   TAG_End (0x00)
        let mut fixture = Vec::new();
        let mut compound = Vec::new();
        compound.push(0x0A); // TAG_Compound
        compound.push(0x08); // TAG_String
        compound.extend_from_slice(&4u16.to_be_bytes()); // "text" length
        compound.extend_from_slice(b"text");
        compound.extend_from_slice(&5u16.to_be_bytes()); // "Hello" length
        compound.extend_from_slice(b"Hello");
        compound.push(0x00); // TAG_End

        let payload_len = 1 + 1 + 16 + 1 + compound.len();
        let total_frame_len = 1 + payload_len; // 1 byte for VarInt 0x47
        write_varint(total_frame_len as i32, &mut fixture);
        write_varint(0x47, &mut fixture);
        fixture.push(0x20);
        write_varint(1, &mut fixture);
        fixture.extend_from_slice(&[0x08; 16]);
        fixture.push(0x01);
        fixture.extend_from_slice(&compound);

        let packet = PlayerInfoUpdatePacket::decode(&fixture)
            .expect("compound display name fixture must decode successfully");
        assert_eq!(packet.entries[0].display_name.as_deref(), Some("Hello"));
    }

    #[test]
    fn test_actions_0x40_and_0x80_wire_fixture() {
        // Test independently constructed wire bytes for UpdateListOrder (0x40) and UpdateHat (0x80)
        let mut fixture = Vec::new();
        let mut payload = Vec::new();
        payload.push(0xC0); // 0x40 | 0x80
        write_varint(1, &mut payload); // 1 entry
        payload.extend_from_slice(&[0x09; 16]); // UUID
        write_varint(42, &mut payload); // list_order = 42
        payload.push(0x01); // show_hat = true

        let total_frame_len = 1 + payload.len();
        write_varint(total_frame_len as i32, &mut fixture);
        write_varint(0x47, &mut fixture);
        fixture.extend_from_slice(&payload);

        let packet = PlayerInfoUpdatePacket::decode(&fixture)
            .expect("0xC0 actions fixture must decode successfully");
        assert_eq!(packet.actions, 0xC0);
        assert_eq!(packet.entries[0].list_order, 42);
        assert!(packet.entries[0].show_hat);
    }

    fn too_long(packet: PlayerInfoUpdatePacket, length: usize, max: usize) {
        assert_eq!(
            packet.encode_payload().unwrap_err(),
            ProtocolError::StringTooLong { length, max }
        );
    }

    #[test]
    fn test_encode_field_size_limit_validations() {
        let uuid = sample_uuid(1);
        let add = PlayerInfoAction::AddPlayer as u8;
        let chat = PlayerInfoAction::InitializeChat as u8;

        let mut entry = PlayerInfoEntry::new(uuid, "Steve");
        entry.properties = vec![ProfileProperty::unsigned("custom", "x".repeat(32768))];
        too_long(PlayerInfoUpdatePacket::single(add, entry), 32768, 32767);

        let mut entry = PlayerInfoEntry::new(uuid, "Steve");
        entry.properties = vec![ProfileProperty::unsigned("n".repeat(65), "val")];
        too_long(PlayerInfoUpdatePacket::single(add, entry), 65, 64);

        let mut entry = PlayerInfoEntry::new(uuid, "Steve");
        entry.properties = vec![ProfileProperty::signed("textures", "val", "s".repeat(1025))];
        too_long(PlayerInfoUpdatePacket::single(add, entry), 1025, 1024);

        let mut entry = PlayerInfoEntry::new(uuid, "Steve");
        entry.properties = vec![ProfileProperty::unsigned("a", "b"); 17];
        too_long(PlayerInfoUpdatePacket::single(add, entry), 17, 16);

        let mut entry = PlayerInfoEntry::new(uuid, "Steve");
        entry.chat_session = Some(ChatSessionData::new(
            sample_uuid(2),
            12345,
            vec![0x42; 513],
            vec![1, 2, 3],
        ));
        too_long(PlayerInfoUpdatePacket::single(chat, entry), 513, 512);

        let mut entry = PlayerInfoEntry::new(uuid, "Steve");
        entry.chat_session = Some(ChatSessionData::new(
            sample_uuid(2),
            12345,
            vec![1, 2, 3],
            vec![0x42; 4097],
        ));
        too_long(PlayerInfoUpdatePacket::single(chat, entry), 4097, 4096);

        let mut entry = PlayerInfoEntry::new(uuid, "Steve");
        entry.display_name = Some("d".repeat(32768).into());
        too_long(
            PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateDisplayName as u8, entry),
            32768,
            32767,
        );

        // 32767 code units fit the string limit, but not 65535 encoded NBT bytes.
        let mut entry = PlayerInfoEntry::new(uuid, "Steve");
        entry.display_name = Some("\u{20AC}".repeat(32767).into());
        too_long(
            PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateDisplayName as u8, entry),
            98301,
            65535,
        );
    }

    #[test]
    fn test_decode_enforces_the_same_field_limits() {
        // Actions 0x01, one entry, name "ab", 17 properties.
        let mut payload = vec![0x01, 0x01];
        payload.extend([0x01; 16]);
        payload.extend([0x02, b'a', b'b', 0x11]);
        assert_eq!(
            PlayerInfoUpdatePacket::decode_payload(&payload).unwrap_err(),
            ProtocolError::StringTooLong {
                length: 17,
                max: 16
            }
        );
    }

    /// Wraps an anonymous NBT component in a display-name-only packet payload.
    fn display_name_payload(component: &[u8]) -> Vec<u8> {
        let mut payload = vec![0x20, 0x01];
        payload.extend([0x07; 16]);
        payload.push(0x01);
        payload.extend_from_slice(component);
        payload
    }

    #[test]
    fn test_display_name_uses_modified_utf8() {
        // U+1F600 is the surrogate pair D83D DE00, three bytes each.
        let component = [0x08, 0x00, 0x06, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80];
        let payload = display_name_payload(&component);

        let mut entry = PlayerInfoEntry::new([0x07; 16], "");
        entry.display_name = Some("\u{1F600}".into());
        let packet =
            PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateDisplayName as u8, entry);
        assert_eq!(packet.encode_payload().unwrap(), payload);

        let decoded = PlayerInfoUpdatePacket::decode_payload(&payload).unwrap();
        assert_eq!(
            decoded.entries[0].display_name.as_deref(),
            Some("\u{1F600}")
        );

        // U+0000 is C0 80 and never a zero byte.
        let nul = [0x08, 0x00, 0x02, 0xC0, 0x80];
        assert_eq!(
            PlayerDisplayName::from_raw_nbt(&nul).unwrap(),
            PlayerDisplayName::plain("\0")
        );
        for invalid in [
            &[0x08, 0x00, 0x01, 0x00][..],                   // Raw NUL.
            &[0x08, 0x00, 0x04, 0xF0, 0x9F, 0x98, 0x80][..], // Standard 4-byte UTF-8.
            &[0x08, 0x00, 0x03, 0xED, 0xA0, 0xBD][..],       // Lone high surrogate.
            &[0x08, 0x00, 0x03, 0xED, 0xB8, 0x80][..],       // Lone low surrogate.
        ] {
            assert_eq!(
                PlayerDisplayName::from_raw_nbt(invalid),
                Err(ProtocolError::InvalidUtf8)
            );
        }
    }

    #[test]
    fn test_nested_component_is_preserved_byte_for_byte() {
        // {"text":"","extra":[{"text":"Nick"}],"color":"gold"}
        #[rustfmt::skip]
        let component = [
            0x0A,
            0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x00,
            0x09, 0x00, 0x05, b'e', b'x', b't', b'r', b'a', 0x0A, 0x00, 0x00, 0x00, 0x01,
            0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x04, b'N', b'i', b'c', b'k',
            0x00,
            0x08, 0x00, 0x05, b'c', b'o', b'l', b'o', b'r', 0x00, 0x04, b'g', b'o', b'l', b'd',
            0x00,
        ];
        let payload = display_name_payload(&component);
        let decoded = PlayerInfoUpdatePacket::decode_payload(&payload).unwrap();
        let name = decoded.entries[0].display_name.as_ref().unwrap();
        assert_eq!(name.as_str(), "Nick");
        assert_eq!(name.raw_nbt(), Some(&component[..]));
        assert_eq!(decoded.encode_payload().unwrap(), payload);

        // {"translate":"k"} has no literal text and still survives a round trip.
        #[rustfmt::skip]
        let translated = [
            0x0A,
            0x08, 0x00, 0x09, b't', b'r', b'a', b'n', b's', b'l', b'a', b't', b'e',
            0x00, 0x01, b'k',
            0x00,
        ];
        let payload = display_name_payload(&translated);
        let decoded = PlayerInfoUpdatePacket::decode_payload(&payload).unwrap();
        assert_eq!(decoded.entries[0].display_name.as_deref(), Some(""));
        assert_eq!(decoded.encode_payload().unwrap(), payload);

        // Literal text keeps its order when `extra` precedes `text`, and an
        // `extra` list may hold plain strings.
        #[rustfmt::skip]
        let reversed = [
            0x0A,
            0x09, 0x00, 0x05, b'e', b'x', b't', b'r', b'a', 0x08, 0x00, 0x00, 0x00, 0x01,
            0x00, 0x01, b'b',
            0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x01, b'a',
            0x00,
        ];
        let name = PlayerDisplayName::from_raw_nbt(&reversed).unwrap();
        assert_eq!(name.as_str(), "ab");

        assert_eq!(
            PlayerDisplayName::from_raw_nbt(&[0x0A, 0x00]),
            Err(ProtocolError::UnsupportedComponent)
        );
    }

    #[test]
    fn test_hand_built_raw_component_is_validated_on_encode() {
        let mut entry = PlayerInfoEntry::new([0x07; 16], "");
        entry.display_name = Some(PlayerDisplayName::RawNbt {
            raw: vec![0x0A, 0x63],
            text: "x".into(),
        });
        let packet =
            PlayerInfoUpdatePacket::single(PlayerInfoAction::UpdateDisplayName as u8, entry);
        assert!(packet.encode_payload().is_err());
    }

    #[test]
    fn test_nbt_nesting_is_bounded() {
        let max = MAX_NBT_DEPTH;

        // Nested compounds under a non-component field reach the skipper.
        let mut compounds = vec![0x0A];
        for _ in 0..100_000 {
            compounds.extend([0x0A, 0x00, 0x01, b'a']);
        }
        assert!(matches!(
            PlayerInfoUpdatePacket::decode_payload(&display_name_payload(&compounds)),
            Err(ProtocolError::NbtDepthExceeded { max: m, .. }) if m == max
        ));

        // Nested lists of lists.
        let mut lists = vec![0x0A, 0x09, 0x00, 0x01, b'a'];
        for _ in 0..100_000 {
            lists.extend([0x09, 0x00, 0x00, 0x00, 0x01]);
        }
        assert!(matches!(
            PlayerInfoUpdatePacket::decode_payload(&display_name_payload(&lists)),
            Err(ProtocolError::NbtDepthExceeded { max: m, .. }) if m == max
        ));

        // Nested `extra` children use the text-collecting path.
        let mut extras = vec![0x0A];
        for _ in 0..100_000 {
            extras.extend([0x09, 0x00, 0x05, b'e', b'x', b't', b'r', b'a', 0x0A]);
            extras.extend([0x00, 0x00, 0x00, 0x01]);
        }
        assert!(matches!(
            PlayerInfoUpdatePacket::decode_payload(&display_name_payload(&extras)),
            Err(ProtocolError::NbtDepthExceeded { max: m, .. }) if m == max
        ));
    }

    #[test]
    fn test_full_action_mask_wire_fixture() {
        // Every action in table order, written by hand.
        #[rustfmt::skip]
        let mut payload = vec![0xFF, 0x01];
        payload.extend([0x0B; 16]);
        payload.extend([0x03, b'A', b'b', b'c']); // Name.
        payload.push(0x01); // One property.
        payload.extend([0x01, b'k', 0x01, b'v', 0x01, 0x01, b's']);
        payload.push(0x01); // Chat session present.
        payload.extend([0x0C; 16]);
        payload.extend([0, 0, 0, 0, 0, 0, 0x30, 0x39]); // Expiry 12345.
        payload.extend([0x01, 0xAA, 0x02, 0xBB, 0xCC]); // Key and key signature.
        payload.push(0x03); // Spectator.
        payload.push(0x01); // Listed.
        payload.extend([0xFF, 0xFF, 0xFF, 0xFF, 0x0F]); // Ping -1.
        payload.push(0x00); // No display name.
        payload.push(0x2A); // Priority 42.
        payload.push(0x00); // Hat hidden.

        let decoded = PlayerInfoUpdatePacket::decode_payload(&payload).unwrap();
        let entry = &decoded.entries[0];
        assert_eq!(entry.name, "Abc");
        assert_eq!(entry.properties[0].signature.as_deref(), Some("s"));
        assert_eq!(entry.chat_session.as_ref().unwrap().expires_at, 12345);
        assert_eq!(entry.game_mode, GameMode::Spectator);
        assert_eq!(entry.latency, -1);
        assert_eq!(entry.list_order, 42);
        assert!(!entry.show_hat);
        assert_eq!(decoded.encode_payload().unwrap(), payload);
    }

    #[test]
    fn test_tracker_list_order_and_hat() {
        let mut tracker = TabListTracker::new();
        let uuid = sample_uuid(4);
        let entry = sample_entry(4, "OrderHatTester");
        tracker.add_player(entry);

        // Update list order
        let order_packet = tracker.update_list_order(&uuid, 77).unwrap();
        assert_eq!(
            order_packet.actions,
            PlayerInfoAction::UpdateListOrder as u8
        );
        assert_eq!(order_packet.entries[0].list_order, 77);
        assert_eq!(tracker.get_player(&uuid).unwrap().list_order, 77);

        // Update hat
        let hat_packet = tracker.update_hat(&uuid, false).unwrap();
        assert_eq!(hat_packet.actions, PlayerInfoAction::UpdateHat as u8);
        assert!(!hat_packet.entries[0].show_hat);
        assert!(!tracker.get_player(&uuid).unwrap().show_hat);
    }
}
