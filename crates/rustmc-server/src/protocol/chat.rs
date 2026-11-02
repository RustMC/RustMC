//! Serverbound chat packets, clientbound system chat messages, JSON chat components,
//! and command dispatcher for Minecraft Java Edition.

use std::{collections::HashMap, error::Error, fmt, sync::Arc};

/// Serverbound Chat Message packet ID (0x06 / decimal 6).
pub const CHAT_PACKET_ID: u32 = 0x06;

/// Serverbound Chat Command packet ID (0x04 / decimal 4).
pub const CHAT_COMMAND_PACKET_ID: u32 = 0x04;

/// Clientbound System Chat Message packet ID (0x6C / decimal 108).
pub const SYSTEM_CHAT_MESSAGE_PACKET_ID: u32 = 0x6C;

/// Maximum allowed length for chat messages in characters (256).
pub const MAX_CHAT_LENGTH: usize = 256;

/// Maximum allowed length for command strings in characters (256).
pub const MAX_COMMAND_LENGTH: usize = 256;

/// World coordinate boundary for teleportation.
pub const MAX_COORDINATE: f64 = 30_000_000.0;

// ============================================================================
// Errors
// ============================================================================

/// Protocol codec and validation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    Malformed(String),
    Oversized(String),
    WrongPacketId { expected: u32, actual: u32 },
    StringTooLong { max: usize, actual: usize },
    InvalidUtf8,
    UnexpectedEof,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(msg) => write!(f, "Malformed packet: {msg}"),
            Self::Oversized(msg) => write!(f, "Oversized packet: {msg}"),
            Self::WrongPacketId { expected, actual } => {
                write!(
                    f,
                    "Wrong packet ID: expected 0x{expected:02X}, got 0x{actual:02X}"
                )
            }
            Self::StringTooLong { max, actual } => {
                write!(
                    f,
                    "String exceeds maximum length of {max} characters (was {actual})"
                )
            }
            Self::InvalidUtf8 => write!(f, "Invalid UTF-8 sequence in packet string"),
            Self::UnexpectedEof => write!(f, "Unexpected end of packet buffer"),
        }
    }
}

impl Error for ProtocolError {}

/// Command parsing and execution errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    UnknownCommand(String),
    MissingArgument(String),
    InvalidArgument(String),
    InvalidCoordinate(String),
    PlayerNotFound(String),
    ConsoleNotAllowed(String),
    ExecutionError(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(cmd) => {
                write!(
                    f,
                    "Unknown command '{cmd}'. Use /help for a list of commands."
                )
            }
            Self::MissingArgument(msg) => write!(f, "Missing argument: {msg}"),
            Self::InvalidArgument(msg) => write!(f, "Invalid argument: {msg}"),
            Self::InvalidCoordinate(msg) => write!(f, "Invalid coordinate: {msg}"),
            Self::PlayerNotFound(name) => write!(f, "Player not found: '{name}'"),
            Self::ConsoleNotAllowed(msg) => write!(f, "Console not allowed: {msg}"),
            Self::ExecutionError(msg) => write!(f, "Command execution error: {msg}"),
        }
    }
}

impl Error for CommandError {}

// ============================================================================
// Wire Codec Primitives
// ============================================================================

/// Encode a 32-bit unsigned integer as a VarInt into the output buffer.
pub fn put_varint(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Read a VarInt from input bytes, returning `(value, bytes_read)`.
pub fn read_varint(input: &[u8]) -> Result<(u32, usize), ProtocolError> {
    let mut value = 0u32;
    for (i, &byte) in input.iter().take(5).enumerate() {
        if i == 4 && (byte & 0xf0) != 0 {
            return Err(ProtocolError::Malformed("VarInt exceeds 5 bytes".into()));
        }
        value |= u32::from(byte & 0x7f) << (i * 7);
        if (byte & 0x80) == 0 {
            if i > 0 && byte == 0 {
                return Err(ProtocolError::Malformed("Non-canonical VarInt zero".into()));
            }
            return Ok((value, i + 1));
        }
    }
    if input.len() >= 5 {
        Err(ProtocolError::Malformed("VarInt too large".into()))
    } else {
        Err(ProtocolError::UnexpectedEof)
    }
}

/// Encode a string as a VarInt length prefix followed by UTF-8 bytes.
pub fn put_string(value: &str, out: &mut Vec<u8>) {
    put_varint(value.len() as u32, out);
    out.extend_from_slice(value.as_bytes());
}

/// Read a string from input bytes, returning `(&str, bytes_read)`.
pub fn read_string(input: &[u8]) -> Result<(&str, usize), ProtocolError> {
    let (length, prefix) = read_varint(input)?;
    let length = length as usize;
    if input.len() < prefix + length {
        return Err(ProtocolError::UnexpectedEof);
    }
    let s = std::str::from_utf8(&input[prefix..prefix + length])
        .map_err(|_| ProtocolError::InvalidUtf8)?;
    Ok((s, prefix + length))
}

/// Encapsulate a packet ID and payload into a standard Minecraft packet frame:
/// `[VarInt(length), VarInt(packet_id), payload]`.
pub fn frame(packet_id: u32, body: &[u8]) -> Vec<u8> {
    let mut packet = Vec::new();
    put_varint(packet_id, &mut packet);
    packet.extend_from_slice(body);
    let mut out = Vec::new();
    put_varint(packet.len() as u32, &mut out);
    out.extend(packet);
    out
}

/// Unframe a standard Minecraft packet, returning `(packet_id, body_slice)`.
pub fn unframe(bytes: &[u8]) -> Result<(u32, &[u8]), ProtocolError> {
    let (length, prefix) = read_varint(bytes)?;
    let length = length as usize;
    if bytes.len() != prefix + length {
        return Err(ProtocolError::Malformed(format!(
            "Frame length mismatch: header declares {length} bytes, total buffer is {}",
            bytes.len() - prefix
        )));
    }
    let body_bytes = &bytes[prefix..prefix + length];
    let (packet_id, id_len) = read_varint(body_bytes)?;
    Ok((packet_id, &body_bytes[id_len..]))
}

// ============================================================================
// Packets
// ============================================================================

/// Serverbound Chat Message packet (ID 0x06 / decimal 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatPacket {
    /// Message string (max 256 characters).
    pub message: String,
    /// Timestamp (epoch milliseconds).
    pub timestamp: i64,
    /// Cryptographic salt value.
    pub salt: i64,
    /// Cryptographic signature bytes (if present).
    pub signature: Option<Vec<u8>>,
}

impl ChatPacket {
    /// Create a new ChatPacket with validation of message character length.
    pub fn new(
        message: impl Into<String>,
        timestamp: i64,
        salt: i64,
        signature: Option<Vec<u8>>,
    ) -> Result<Self, ProtocolError> {
        let message = message.into();
        let char_count = message.chars().count();
        if char_count > MAX_CHAT_LENGTH {
            return Err(ProtocolError::StringTooLong {
                max: MAX_CHAT_LENGTH,
                actual: char_count,
            });
        }
        Ok(Self {
            message,
            timestamp,
            salt,
            signature,
        })
    }

    /// Create an unsigned ChatPacket.
    pub fn unsigned(
        message: impl Into<String>,
        timestamp: i64,
        salt: i64,
    ) -> Result<Self, ProtocolError> {
        Self::new(message, timestamp, salt, None)
    }

    /// Create a signed ChatPacket.
    pub fn with_signature(
        message: impl Into<String>,
        timestamp: i64,
        salt: i64,
        signature: Vec<u8>,
    ) -> Result<Self, ProtocolError> {
        Self::new(message, timestamp, salt, Some(signature))
    }

    /// Signature bytes slice if present.
    pub fn signature_bytes(&self) -> Option<&[u8]> {
        self.signature.as_deref()
    }

    /// Encode as unframed packet payload: `[0x06, message, timestamp, salt, signature_data]`.
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(CHAT_PACKET_ID, &mut out);
        put_string(&self.message, &mut out);
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.salt.to_be_bytes());
        match &self.signature {
            Some(sig) => {
                out.push(1);
                out.extend_from_slice(sig);
            }
            None => {
                out.push(0);
            }
        }
        out
    }

    /// Encode as a framed packet: `[VarInt(length), 0x06, ...]`.
    pub fn to_frame(&self) -> Vec<u8> {
        let payload = self.encode_packet();
        let mut out = Vec::new();
        put_varint(payload.len() as u32, &mut out);
        out.extend(payload);
        out
    }

    /// Alias for `to_frame`.
    pub fn encode(&self) -> Vec<u8> {
        self.to_frame()
    }

    /// Decode from either framed bytes `[VarInt(len), 0x06, ...]` or unframed `[0x06, ...]`.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.is_empty() {
            return Err(ProtocolError::UnexpectedEof);
        }
        if let Ok((id, body)) = unframe(bytes) {
            if id != CHAT_PACKET_ID {
                return Err(ProtocolError::WrongPacketId {
                    expected: CHAT_PACKET_ID,
                    actual: id,
                });
            }
            return Self::decode_payload(body);
        }
        Self::decode_packet(bytes)
    }

    /// Decode from unframed packet bytes `[0x06, ...]`.
    pub fn decode_packet(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (id, prefix) = read_varint(bytes)?;
        if id != CHAT_PACKET_ID {
            return Err(ProtocolError::WrongPacketId {
                expected: CHAT_PACKET_ID,
                actual: id,
            });
        }
        Self::decode_payload(&bytes[prefix..])
    }

    /// Decode payload without packet ID.
    pub fn decode_payload(body: &[u8]) -> Result<Self, ProtocolError> {
        let (message_str, msg_len) = read_string(body)?;
        let char_count = message_str.chars().count();
        if char_count > MAX_CHAT_LENGTH {
            return Err(ProtocolError::StringTooLong {
                max: MAX_CHAT_LENGTH,
                actual: char_count,
            });
        }
        let message = message_str.to_string();
        let mut rest = &body[msg_len..];

        if rest.len() < 16 {
            return Err(ProtocolError::UnexpectedEof);
        }
        let timestamp = i64::from_be_bytes(
            rest[..8]
                .try_into()
                .map_err(|_| ProtocolError::Malformed("Invalid timestamp".into()))?,
        );
        let salt = i64::from_be_bytes(
            rest[8..16]
                .try_into()
                .map_err(|_| ProtocolError::Malformed("Invalid salt".into()))?,
        );
        rest = &rest[16..];

        let signature = if rest.is_empty() {
            None
        } else if rest[0] == 0 {
            if rest.len() > 1 {
                return Err(ProtocolError::Malformed(
                    "Trailing bytes after unsigned flag".into(),
                ));
            }
            None
        } else if rest[0] == 1 {
            let sig_bytes = rest[1..].to_vec();
            if sig_bytes.is_empty() {
                return Err(ProtocolError::UnexpectedEof);
            }
            Some(sig_bytes)
        } else {
            // Raw signature bytes without boolean prefix
            Some(rest.to_vec())
        };

        Ok(Self {
            message,
            timestamp,
            salt,
            signature,
        })
    }
}

/// Cryptographically signed argument in a ChatCommandPacket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentSignature {
    pub name: String,
    pub signature: Vec<u8>,
}

/// Serverbound Chat Command packet (ID 0x04 / decimal 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatCommandPacket {
    /// Command string without leading forward slash `/` (max 256 characters).
    pub command: String,
    /// Timestamp (epoch milliseconds).
    pub timestamp: i64,
    /// Cryptographic salt value.
    pub salt: i64,
    /// Signatures for signable command arguments.
    pub argument_signatures: Vec<ArgumentSignature>,
}

impl ChatCommandPacket {
    /// Create a new ChatCommandPacket, stripping any leading `/` and checking character count.
    pub fn new(
        command: impl Into<String>,
        timestamp: i64,
        salt: i64,
        argument_signatures: Vec<ArgumentSignature>,
    ) -> Result<Self, ProtocolError> {
        let raw = command.into();
        let stripped = raw.strip_prefix('/').unwrap_or(&raw).to_string();
        let char_count = stripped.chars().count();
        if char_count > MAX_COMMAND_LENGTH {
            return Err(ProtocolError::StringTooLong {
                max: MAX_COMMAND_LENGTH,
                actual: char_count,
            });
        }
        Ok(Self {
            command: stripped,
            timestamp,
            salt,
            argument_signatures,
        })
    }

    /// Create an unsigned ChatCommandPacket.
    pub fn unsigned(
        command: impl Into<String>,
        timestamp: i64,
        salt: i64,
    ) -> Result<Self, ProtocolError> {
        Self::new(command, timestamp, salt, Vec::new())
    }

    /// Encode as unframed packet payload: `[0x04, command, timestamp, salt, argument_signatures]`.
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(CHAT_COMMAND_PACKET_ID, &mut out);
        put_string(&self.command, &mut out);
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.salt.to_be_bytes());
        put_varint(self.argument_signatures.len() as u32, &mut out);
        for sig in &self.argument_signatures {
            put_string(&sig.name, &mut out);
            put_varint(sig.signature.len() as u32, &mut out);
            out.extend_from_slice(&sig.signature);
        }
        out
    }

    /// Encode as a framed packet: `[VarInt(length), 0x04, ...]`.
    pub fn to_frame(&self) -> Vec<u8> {
        let payload = self.encode_packet();
        let mut out = Vec::new();
        put_varint(payload.len() as u32, &mut out);
        out.extend(payload);
        out
    }

    /// Alias for `to_frame`.
    pub fn encode(&self) -> Vec<u8> {
        self.to_frame()
    }

    /// Decode from either framed bytes `[VarInt(len), 0x04, ...]` or unframed `[0x04, ...]`.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.is_empty() {
            return Err(ProtocolError::UnexpectedEof);
        }
        if let Ok((id, body)) = unframe(bytes) {
            if id != CHAT_COMMAND_PACKET_ID {
                return Err(ProtocolError::WrongPacketId {
                    expected: CHAT_COMMAND_PACKET_ID,
                    actual: id,
                });
            }
            return Self::decode_payload(body);
        }
        Self::decode_packet(bytes)
    }

    /// Decode from unframed packet bytes `[0x04, ...]`.
    pub fn decode_packet(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (id, prefix) = read_varint(bytes)?;
        if id != CHAT_COMMAND_PACKET_ID {
            return Err(ProtocolError::WrongPacketId {
                expected: CHAT_COMMAND_PACKET_ID,
                actual: id,
            });
        }
        Self::decode_payload(&bytes[prefix..])
    }

    /// Decode payload without packet ID.
    pub fn decode_payload(body: &[u8]) -> Result<Self, ProtocolError> {
        let (command_str, cmd_len) = read_string(body)?;
        let stripped = command_str.strip_prefix('/').unwrap_or(command_str);
        let char_count = stripped.chars().count();
        if char_count > MAX_COMMAND_LENGTH {
            return Err(ProtocolError::StringTooLong {
                max: MAX_COMMAND_LENGTH,
                actual: char_count,
            });
        }
        let command = stripped.to_string();
        let mut rest = &body[cmd_len..];

        if rest.len() < 16 {
            return Err(ProtocolError::UnexpectedEof);
        }
        let timestamp = i64::from_be_bytes(
            rest[..8]
                .try_into()
                .map_err(|_| ProtocolError::Malformed("Invalid timestamp".into()))?,
        );
        let salt = i64::from_be_bytes(
            rest[8..16]
                .try_into()
                .map_err(|_| ProtocolError::Malformed("Invalid salt".into()))?,
        );
        rest = &rest[16..];

        let mut argument_signatures = Vec::new();
        if !rest.is_empty() {
            let (count, count_len) = read_varint(rest)?;
            rest = &rest[count_len..];
            for _ in 0..count {
                let (name, name_len) = read_string(rest)?;
                rest = &rest[name_len..];
                let (sig_len, sig_len_bytes) = read_varint(rest)?;
                let sig_len = sig_len as usize;
                rest = &rest[sig_len_bytes..];
                if rest.len() < sig_len {
                    return Err(ProtocolError::UnexpectedEof);
                }
                let signature = rest[..sig_len].to_vec();
                rest = &rest[sig_len..];
                argument_signatures.push(ArgumentSignature {
                    name: name.to_string(),
                    signature,
                });
            }
        }

        if !rest.is_empty() {
            return Err(ProtocolError::Malformed(
                "Trailing bytes in ChatCommand packet".into(),
            ));
        }

        Ok(Self {
            command,
            timestamp,
            salt,
            argument_signatures,
        })
    }
}

/// Clientbound System Chat Message packet (ID 0x6C / decimal 108).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemChatMessagePacket {
    /// Serialized JSON text component string.
    pub content: String,
    /// Overlay boolean: `true` for Action Bar, `false` for standard Chat Box.
    pub overlay: bool,
}

impl SystemChatMessagePacket {
    /// Create a new SystemChatMessagePacket.
    pub fn new(content: impl Into<String>, overlay: bool) -> Self {
        Self {
            content: content.into(),
            overlay,
        }
    }

    /// Create from a ChatComponent for chat box display.
    pub fn chat(component: &ChatComponent) -> Self {
        Self::new(component.to_json(), false)
    }

    /// Create from a ChatComponent for action bar overlay display.
    pub fn overlay(component: &ChatComponent) -> Self {
        Self::new(component.to_json(), true)
    }

    /// Create a plain chat box message.
    pub fn chat_text(text: impl Into<String>) -> Self {
        Self::chat(&ChatComponent::text(text))
    }

    /// Create a plain overlay action bar message.
    pub fn overlay_text(text: impl Into<String>) -> Self {
        Self::overlay(&ChatComponent::text(text))
    }

    /// Encode as unframed packet payload: `[0x6C, content_string, overlay_boolean]`.
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(SYSTEM_CHAT_MESSAGE_PACKET_ID, &mut out);
        put_string(&self.content, &mut out);
        out.push(if self.overlay { 1 } else { 0 });
        out
    }

    /// Encode as a framed packet: `[VarInt(length), 0x6C, ...]`.
    pub fn to_frame(&self) -> Vec<u8> {
        let payload = self.encode_packet();
        let mut out = Vec::new();
        put_varint(payload.len() as u32, &mut out);
        out.extend(payload);
        out
    }

    /// Alias for `to_frame`.
    pub fn encode(&self) -> Vec<u8> {
        self.to_frame()
    }

    /// Decode from either framed bytes `[VarInt(len), 0x6C, ...]` or unframed `[0x6C, ...]`.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.is_empty() {
            return Err(ProtocolError::UnexpectedEof);
        }
        if let Ok((id, body)) = unframe(bytes) {
            if id != SYSTEM_CHAT_MESSAGE_PACKET_ID {
                return Err(ProtocolError::WrongPacketId {
                    expected: SYSTEM_CHAT_MESSAGE_PACKET_ID,
                    actual: id,
                });
            }
            return Self::decode_payload(body);
        }
        Self::decode_packet(bytes)
    }

    /// Decode from unframed packet bytes `[0x6C, ...]`.
    pub fn decode_packet(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (id, prefix) = read_varint(bytes)?;
        if id != SYSTEM_CHAT_MESSAGE_PACKET_ID {
            return Err(ProtocolError::WrongPacketId {
                expected: SYSTEM_CHAT_MESSAGE_PACKET_ID,
                actual: id,
            });
        }
        Self::decode_payload(&bytes[prefix..])
    }

    /// Decode payload without packet ID.
    pub fn decode_payload(body: &[u8]) -> Result<Self, ProtocolError> {
        let (content_str, str_len) = read_string(body)?;
        let content = content_str.to_string();
        let rest = &body[str_len..];
        if rest.len() != 1 {
            return Err(ProtocolError::Malformed(
                "Expected exactly 1 byte for overlay flag".into(),
            ));
        }
        let overlay = match rest[0] {
            0 => false,
            1 => true,
            other => {
                return Err(ProtocolError::Malformed(format!(
                    "Invalid boolean byte for overlay flag: 0x{other:02X}"
                )));
            }
        };
        Ok(Self { content, overlay })
    }
}

// Function-level decoders and encoders for convenience
pub fn decode_chat_packet(bytes: &[u8]) -> Result<ChatPacket, ProtocolError> {
    ChatPacket::decode(bytes)
}

pub fn encode_chat_packet(packet: &ChatPacket) -> Vec<u8> {
    packet.encode_packet()
}

pub fn decode_chat_command_packet(bytes: &[u8]) -> Result<ChatCommandPacket, ProtocolError> {
    ChatCommandPacket::decode(bytes)
}

pub fn encode_chat_command_packet(packet: &ChatCommandPacket) -> Vec<u8> {
    packet.encode_packet()
}

pub fn decode_system_chat_message_packet(
    bytes: &[u8],
) -> Result<SystemChatMessagePacket, ProtocolError> {
    SystemChatMessagePacket::decode(bytes)
}

pub fn encode_system_chat_message_packet(packet: &SystemChatMessagePacket) -> Vec<u8> {
    packet.encode_packet()
}

// ============================================================================
// Chat Colors & Chat Component Builder
// ============================================================================

/// Standard Minecraft text colors.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChatColor {
    Black,
    DarkBlue,
    DarkGreen,
    DarkAqua,
    DarkRed,
    DarkPurple,
    Gold,
    Gray,
    DarkGray,
    Blue,
    Green,
    Aqua,
    Red,
    LightPurple,
    Yellow,
    White,
    Custom(String),
}

impl ChatColor {
    /// Return the canonical Minecraft identifier for this color.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Black => "black",
            Self::DarkBlue => "dark_blue",
            Self::DarkGreen => "dark_green",
            Self::DarkAqua => "dark_aqua",
            Self::DarkRed => "dark_red",
            Self::DarkPurple => "dark_purple",
            Self::Gold => "gold",
            Self::Gray => "gray",
            Self::DarkGray => "dark_gray",
            Self::Blue => "blue",
            Self::Green => "green",
            Self::Aqua => "aqua",
            Self::Red => "red",
            Self::LightPurple => "light_purple",
            Self::Yellow => "yellow",
            Self::White => "white",
            Self::Custom(hex) => hex.as_str(),
        }
    }

    /// Parse a color name or `#RRGGBB` hex string into a ChatColor.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "black" => Some(Self::Black),
            "dark_blue" => Some(Self::DarkBlue),
            "dark_green" => Some(Self::DarkGreen),
            "dark_aqua" => Some(Self::DarkAqua),
            "dark_red" => Some(Self::DarkRed),
            "dark_purple" => Some(Self::DarkPurple),
            "gold" => Some(Self::Gold),
            "gray" => Some(Self::Gray),
            "dark_gray" => Some(Self::DarkGray),
            "blue" => Some(Self::Blue),
            "green" => Some(Self::Green),
            "aqua" => Some(Self::Aqua),
            "red" => Some(Self::Red),
            "light_purple" => Some(Self::LightPurple),
            "yellow" => Some(Self::Yellow),
            "white" => Some(Self::White),
            hex if hex.starts_with('#') && hex.len() == 7 => Some(Self::Custom(hex.to_string())),
            _ => None,
        }
    }
}

/// JSON Chat Component adhering to Minecraft Java Edition text component specification.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatComponent {
    /// The base text content.
    pub text: String,
    /// Color of this component.
    pub color: Option<ChatColor>,
    /// Bold styling flag.
    pub bold: Option<bool>,
    /// Italic styling flag.
    pub italic: Option<bool>,
    /// Underlined styling flag.
    pub underlined: Option<bool>,
    /// Strikethrough styling flag.
    pub strikethrough: Option<bool>,
    /// Obfuscated styling flag.
    pub obfuscated: Option<bool>,
    /// Extra sibling components.
    pub extra: Vec<ChatComponent>,
}

impl ChatComponent {
    /// Create a plain text chat component.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    /// Create a plain text chat component (alias).
    pub fn plain(text: impl Into<String>) -> Self {
        Self::text(text)
    }

    /// Create a colored text chat component.
    pub fn colored(text: impl Into<String>, color: ChatColor) -> Self {
        Self::text(text).with_color(color)
    }

    /// Format a sender prefix and message, e.g. `<Player> hello`.
    pub fn player_message(sender: &str, message: &str) -> Self {
        Self::text(format!("<{sender}> {message}"))
    }

    /// Format a sender prefix and message with custom sender and message colors.
    pub fn player_message_styled(
        sender: &str,
        sender_color: ChatColor,
        message: &str,
        message_color: Option<ChatColor>,
    ) -> Self {
        let mut msg_comp = Self::text(message);
        if let Some(col) = message_color {
            msg_comp = msg_comp.with_color(col);
        }
        Self::text(format!("<{sender}> "))
            .with_color(sender_color)
            .add_extra(msg_comp)
    }

    /// Set the color for this component.
    pub fn with_color(mut self, color: ChatColor) -> Self {
        self.color = Some(color);
        self
    }

    /// Set bold styling.
    pub fn with_bold(mut self, bold: bool) -> Self {
        self.bold = Some(bold);
        self
    }

    /// Set italic styling.
    pub fn with_italic(mut self, italic: bool) -> Self {
        self.italic = Some(italic);
        self
    }

    /// Set underlined styling.
    pub fn with_underlined(mut self, underlined: bool) -> Self {
        self.underlined = Some(underlined);
        self
    }

    /// Set strikethrough styling.
    pub fn with_strikethrough(mut self, strikethrough: bool) -> Self {
        self.strikethrough = Some(strikethrough);
        self
    }

    /// Set obfuscated styling.
    pub fn with_obfuscated(mut self, obfuscated: bool) -> Self {
        self.obfuscated = Some(obfuscated);
        self
    }

    /// Add an extra sibling component.
    pub fn add_extra(mut self, extra: ChatComponent) -> Self {
        self.extra.push(extra);
        self
    }

    /// Convert this component into a `serde_json::Value` adhering strictly to Minecraft text JSON schema.
    pub fn to_value(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert(
            "text".to_string(),
            serde_json::Value::String(self.text.clone()),
        );
        if let Some(ref color) = self.color {
            map.insert(
                "color".to_string(),
                serde_json::Value::String(color.as_str().to_string()),
            );
        }
        if let Some(bold) = self.bold {
            map.insert("bold".to_string(), serde_json::Value::Bool(bold));
        }
        if let Some(italic) = self.italic {
            map.insert("italic".to_string(), serde_json::Value::Bool(italic));
        }
        if let Some(underlined) = self.underlined {
            map.insert(
                "underlined".to_string(),
                serde_json::Value::Bool(underlined),
            );
        }
        if let Some(strikethrough) = self.strikethrough {
            map.insert(
                "strikethrough".to_string(),
                serde_json::Value::Bool(strikethrough),
            );
        }
        if let Some(obfuscated) = self.obfuscated {
            map.insert(
                "obfuscated".to_string(),
                serde_json::Value::Bool(obfuscated),
            );
        }
        if !self.extra.is_empty() {
            let extra_vals: Vec<serde_json::Value> =
                self.extra.iter().map(|e| e.to_value()).collect();
            map.insert("extra".to_string(), serde_json::Value::Array(extra_vals));
        }
        serde_json::Value::Object(map)
    }

    /// Serialize this component to a standard Minecraft JSON text string (`{"text":"..."}`).
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.to_value())
            .unwrap_or_else(|_| format!("{{\"text\":\"{}\"}}", self.text))
    }

    /// Parse a JSON string conforming to Minecraft text component specification into a `ChatComponent`.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| format!("Failed to parse chat component JSON: {e}"))?;
        Self::from_value(&value)
    }

    /// Parse a `serde_json::Value` into a `ChatComponent`.
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        match value {
            serde_json::Value::String(s) => Ok(Self::text(s.clone())),
            serde_json::Value::Object(map) => {
                let text = map
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let color = map
                    .get("color")
                    .and_then(|v| v.as_str())
                    .and_then(ChatColor::parse);
                let bold = map.get("bold").and_then(|v| v.as_bool());
                let italic = map.get("italic").and_then(|v| v.as_bool());
                let underlined = map.get("underlined").and_then(|v| v.as_bool());
                let strikethrough = map.get("strikethrough").and_then(|v| v.as_bool());
                let obfuscated = map.get("obfuscated").and_then(|v| v.as_bool());
                let mut extra = Vec::new();
                if let Some(serde_json::Value::Array(arr)) = map.get("extra") {
                    for item in arr {
                        extra.push(Self::from_value(item)?);
                    }
                }
                Ok(Self {
                    text,
                    color,
                    bold,
                    italic,
                    underlined,
                    strikethrough,
                    obfuscated,
                    extra,
                })
            }
            _ => Err("Invalid chat component JSON type, expected object or string".into()),
        }
    }

    /// Extract concatenated plain text stripped of JSON formatting.
    pub fn to_plain_text(&self) -> String {
        let mut out = self.text.clone();
        for child in &self.extra {
            out.push_str(&child.to_plain_text());
        }
        out
    }
}

impl fmt::Display for ChatComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_json())
    }
}

/// Create a system chat broadcast packet for a player message `<sender> message`.
pub fn broadcast_chat(sender: &str, message: &str) -> SystemChatMessagePacket {
    let component = ChatComponent::player_message(sender, message);
    SystemChatMessagePacket::chat(&component)
}

// ============================================================================
// Command Dispatcher & Built-in Handlers
// ============================================================================

/// Minecraft game modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    /// Canonical lowercase name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Survival => "survival",
            Self::Creative => "creative",
            Self::Adventure => "adventure",
            Self::Spectator => "spectator",
        }
    }

    /// Parse from mode name, short alias, or numeric ID.
    pub fn parse(s: &str) -> Result<Self, CommandError> {
        match s.to_ascii_lowercase().as_str() {
            "survival" | "s" | "0" => Ok(Self::Survival),
            "creative" | "c" | "1" => Ok(Self::Creative),
            "adventure" | "a" | "2" => Ok(Self::Adventure),
            "spectator" | "sp" | "3" => Ok(Self::Spectator),
            other => Err(CommandError::InvalidArgument(format!(
                "Unknown game mode '{other}', expected 'survival', 'creative', 'adventure', or 'spectator'"
            ))),
        }
    }
}

/// Command execution sender context.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandSender {
    Player(String),
    Console,
}

impl CommandSender {
    /// Return the sender identifier or player name.
    pub fn name(&self) -> &str {
        match self {
            Self::Player(name) => name.as_str(),
            Self::Console => "Server",
        }
    }
}

/// State of a player tracked by the command dispatcher.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub gamemode: GameMode,
    pub inventory: HashMap<String, u32>,
}

impl PlayerState {
    /// Create a new PlayerState with standard defaults.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            x: 0.0,
            y: 64.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            gamemode: GameMode::Survival,
            inventory: HashMap::new(),
        }
    }

    /// Set player position.
    pub fn with_position(mut self, x: f64, y: f64, z: f64) -> Self {
        self.x = x;
        self.y = y;
        self.z = z;
        self
    }

    /// Set player game mode.
    pub fn with_gamemode(mut self, gamemode: GameMode) -> Self {
        self.gamemode = gamemode;
        self
    }

    /// Give items to player inventory.
    pub fn give_item(&mut self, item: &str, count: u32) {
        let key = item.strip_prefix("minecraft:").unwrap_or(item).to_string();
        *self.inventory.entry(key).or_insert(0) += count;
    }
}

/// Registered command handler signature.
pub type CommandHandler = Arc<
    dyn Fn(&mut CommandDispatcher, &CommandSender, &[&str]) -> Result<String, CommandError>
        + Send
        + Sync,
>;

/// Metadata and callback for a registered command.
#[derive(Clone)]
pub struct CommandRegistration {
    pub name: String,
    pub description: String,
    pub usage: String,
    pub handler: CommandHandler,
}

/// Command registry, argument parser, and execution dispatcher.
pub struct CommandDispatcher {
    pub world_time: u64,
    pub players: HashMap<String, PlayerState>,
    commands: HashMap<String, CommandRegistration>,
}

impl Default for CommandDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandDispatcher {
    /// Create a new dispatcher pre-populated with built-in commands:
    /// `/help`, `/tp`, `/gamemode`, `/time`, `/give`.
    pub fn new() -> Self {
        let mut dispatcher = Self {
            world_time: 1000,
            players: HashMap::new(),
            commands: HashMap::new(),
        };
        dispatcher.register_builtins();
        dispatcher
    }

    /// Add a player to the dispatcher.
    pub fn add_player(&mut self, player: PlayerState) {
        self.players.insert(player.name.clone(), player);
    }

    /// Get a reference to a player state.
    pub fn get_player(&self, name: &str) -> Option<&PlayerState> {
        self.players.get(name)
    }

    /// Get a mutable reference to a player state.
    pub fn get_player_mut(&mut self, name: &str) -> Option<&mut PlayerState> {
        self.players.get_mut(name)
    }

    /// Register a new command handler.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        usage: impl Into<String>,
        handler: impl Fn(
            &mut CommandDispatcher,
            &CommandSender,
            &[&str],
        ) -> Result<String, CommandError>
        + Send
        + Sync
        + 'static,
    ) {
        let name = name.into().to_ascii_lowercase();
        let stripped = name.strip_prefix('/').unwrap_or(&name).to_string();
        let registration = CommandRegistration {
            name: stripped.clone(),
            description: description.into(),
            usage: usage.into(),
            handler: Arc::new(handler),
        };
        self.commands.insert(stripped, registration);
    }

    /// Parse and execute a command string, returning a clean success message or CommandError.
    pub fn dispatch(
        &mut self,
        sender: &CommandSender,
        command_line: &str,
    ) -> Result<String, CommandError> {
        let trimmed = command_line.trim();
        let without_slash = trimmed.strip_prefix('/').unwrap_or(trimmed);
        if without_slash.is_empty() {
            return Err(CommandError::MissingArgument(
                "Command string cannot be empty".into(),
            ));
        }

        let tokens = tokenize(without_slash)?;
        if tokens.is_empty() {
            return Err(CommandError::MissingArgument(
                "Command string cannot be empty".into(),
            ));
        }

        let cmd_name = tokens[0].to_ascii_lowercase();
        let args: Vec<&str> = tokens[1..].iter().map(|s| s.as_str()).collect();

        let handler = match self.commands.get(&cmd_name) {
            Some(reg) => Arc::clone(&reg.handler),
            None => return Err(CommandError::UnknownCommand(cmd_name)),
        };

        handler(self, sender, &args)
    }

    /// Execute a command (alias for `dispatch`).
    pub fn execute(
        &mut self,
        sender: &CommandSender,
        command_line: &str,
    ) -> Result<String, CommandError> {
        self.dispatch(sender, command_line)
    }

    /// Dispatch a command and format the response as a `SystemChatMessagePacket`.
    pub fn dispatch_to_chat(
        &mut self,
        sender: &CommandSender,
        command_line: &str,
    ) -> SystemChatMessagePacket {
        match self.dispatch(sender, command_line) {
            Ok(msg) => SystemChatMessagePacket::chat_text(msg),
            Err(err) => {
                let comp = ChatComponent::colored(err.to_string(), ChatColor::Red);
                SystemChatMessagePacket::chat(&comp)
            }
        }
    }

    fn register_builtins(&mut self) {
        // /help
        self.register(
            "help",
            "Lists available commands or shows help for a specific command",
            "/help [command]",
            handle_help,
        );

        // /tp and /teleport
        self.register(
            "tp",
            "Teleports the player to coordinates or target",
            "/tp <x> <y> <z>",
            handle_tp,
        );
        self.register(
            "teleport",
            "Teleports the player to coordinates or target",
            "/teleport <x> <y> <z>",
            handle_tp,
        );

        // /gamemode
        self.register(
            "gamemode",
            "Sets the player's game mode",
            "/gamemode <survival|creative|adventure|spectator>",
            handle_gamemode,
        );

        // /time
        self.register(
            "time",
            "Changes or queries the world time",
            "/time set <day|night|ticks>",
            handle_time,
        );

        // /give
        self.register(
            "give",
            "Gives an item to a player",
            "/give <item> [count]",
            handle_give,
        );
    }
}

// ============================================================================
// Built-in Command Implementations
// ============================================================================

fn handle_help(
    dispatcher: &mut CommandDispatcher,
    _sender: &CommandSender,
    args: &[&str],
) -> Result<String, CommandError> {
    if args.is_empty() {
        let mut names: Vec<String> = dispatcher.commands.keys().cloned().collect();
        names.sort();
        let mut lines = Vec::new();
        lines.push("--- Available Commands ---".to_string());
        for name in names {
            if let Some(reg) = dispatcher.commands.get(&name) {
                lines.push(format!("{} - {}", reg.usage, reg.description));
            }
        }
        Ok(lines.join("\n"))
    } else {
        let target = args[0]
            .strip_prefix('/')
            .unwrap_or(args[0])
            .to_ascii_lowercase();
        match dispatcher.commands.get(&target) {
            Some(reg) => Ok(format!("{}: {} - {}", reg.name, reg.usage, reg.description)),
            None => Err(CommandError::UnknownCommand(format!(
                "Help topic '{target}' not found"
            ))),
        }
    }
}

fn parse_coordinate(arg: &str, current: f64) -> Result<f64, CommandError> {
    let arg = arg.trim();
    if arg.is_empty() {
        return Err(CommandError::InvalidCoordinate(
            "Coordinate cannot be empty".into(),
        ));
    }
    let val = if let Some(offset_str) = arg.strip_prefix('~') {
        if offset_str.is_empty() {
            current
        } else {
            let offset: f64 = offset_str.parse().map_err(|_| {
                CommandError::InvalidCoordinate(format!("Invalid relative coordinate '{arg}'"))
            })?;
            current + offset
        }
    } else {
        arg.parse::<f64>()
            .map_err(|_| CommandError::InvalidCoordinate(format!("Invalid coordinate '{arg}'")))?
    };

    if !val.is_finite() {
        return Err(CommandError::InvalidCoordinate(format!(
            "Coordinate '{arg}' is not finite"
        )));
    }
    if val.abs() > MAX_COORDINATE {
        return Err(CommandError::InvalidCoordinate(format!(
            "Coordinate '{arg}' exceeds maximum boundary of {MAX_COORDINATE}"
        )));
    }

    Ok(val)
}

fn handle_tp(
    dispatcher: &mut CommandDispatcher,
    sender: &CommandSender,
    args: &[&str],
) -> Result<String, CommandError> {
    match args.len() {
        3 => {
            // /tp <x> <y> <z>
            let player_name = match sender {
                CommandSender::Player(name) => name.clone(),
                CommandSender::Console => {
                    return Err(CommandError::ConsoleNotAllowed(
                        "Console must specify target player. Usage: /tp <player> <x> <y> <z>"
                            .into(),
                    ));
                }
            };
            let (cur_x, cur_y, cur_z) = if let Some(p) = dispatcher.players.get(&player_name) {
                (p.x, p.y, p.z)
            } else {
                (0.0, 64.0, 0.0)
            };

            let x = parse_coordinate(args[0], cur_x)?;
            let y = parse_coordinate(args[1], cur_y)?;
            let z = parse_coordinate(args[2], cur_z)?;

            let player = dispatcher
                .players
                .entry(player_name.clone())
                .or_insert_with(|| PlayerState::new(&player_name));
            player.x = x;
            player.y = y;
            player.z = z;

            Ok(format!(
                "Teleported {player_name} to {x:.2}, {y:.2}, {z:.2}"
            ))
        }
        4 => {
            // /tp <player> <x> <y> <z>
            let target_name = args[0].to_string();
            let (cur_x, cur_y, cur_z) = if let Some(p) = dispatcher.players.get(&target_name) {
                (p.x, p.y, p.z)
            } else {
                return Err(CommandError::PlayerNotFound(target_name));
            };

            let x = parse_coordinate(args[1], cur_x)?;
            let y = parse_coordinate(args[2], cur_y)?;
            let z = parse_coordinate(args[3], cur_z)?;

            let player = dispatcher.players.get_mut(&target_name).unwrap();
            player.x = x;
            player.y = y;
            player.z = z;

            Ok(format!(
                "Teleported {target_name} to {x:.2}, {y:.2}, {z:.2}"
            ))
        }
        _ => Err(CommandError::MissingArgument(
            "Usage: /tp <x> <y> <z> or /tp <player> <x> <y> <z>".into(),
        )),
    }
}

fn handle_gamemode(
    dispatcher: &mut CommandDispatcher,
    sender: &CommandSender,
    args: &[&str],
) -> Result<String, CommandError> {
    if args.is_empty() {
        return Err(CommandError::MissingArgument(
            "Usage: /gamemode <survival|creative|adventure|spectator> [player]".into(),
        ));
    }
    if args.len() > 2 {
        return Err(CommandError::InvalidArgument(
            "Too many arguments for /gamemode".into(),
        ));
    }

    let mode = GameMode::parse(args[0])?;
    let target_name = if args.len() == 2 {
        args[1].to_string()
    } else {
        match sender {
            CommandSender::Player(name) => name.clone(),
            CommandSender::Console => {
                return Err(CommandError::ConsoleNotAllowed(
                    "Console must specify target player. Usage: /gamemode <mode> <player>".into(),
                ));
            }
        }
    };

    let player = dispatcher
        .players
        .entry(target_name.clone())
        .or_insert_with(|| PlayerState::new(&target_name));
    player.gamemode = mode;

    Ok(format!(
        "Set {}'s game mode to {}",
        target_name,
        mode.name()
    ))
}

fn handle_time(
    dispatcher: &mut CommandDispatcher,
    _sender: &CommandSender,
    args: &[&str],
) -> Result<String, CommandError> {
    if args.is_empty() {
        return Err(CommandError::MissingArgument(
            "Usage: /time set <day|night|ticks>".into(),
        ));
    }

    match args[0].to_ascii_lowercase().as_str() {
        "set" => {
            if args.len() < 2 {
                return Err(CommandError::MissingArgument(
                    "Usage: /time set <day|night|ticks>".into(),
                ));
            }
            if args.len() > 2 {
                return Err(CommandError::InvalidArgument(
                    "Too many arguments for /time set".into(),
                ));
            }
            let ticks: u64 = match args[1].to_ascii_lowercase().as_str() {
                "day" => 1000,
                "night" => 13000,
                "noon" => 6000,
                "midnight" => 18000,
                ticks_str => {
                    let val: i64 = ticks_str.parse().map_err(|_| {
                        CommandError::InvalidArgument(format!("Invalid time value '{ticks_str}'"))
                    })?;
                    if val < 0 {
                        return Err(CommandError::InvalidArgument(
                            "Time ticks cannot be negative".into(),
                        ));
                    }
                    val as u64
                }
            };
            dispatcher.world_time = ticks;
            Ok(format!("Set the time to {ticks}"))
        }
        "add" => {
            if args.len() < 2 {
                return Err(CommandError::MissingArgument(
                    "Usage: /time add <ticks>".into(),
                ));
            }
            let val: i64 = args[1].parse().map_err(|_| {
                CommandError::InvalidArgument(format!("Invalid time value '{}'", args[1]))
            })?;
            if val < 0 {
                return Err(CommandError::InvalidArgument(
                    "Added time ticks cannot be negative".into(),
                ));
            }
            dispatcher.world_time = dispatcher.world_time.wrapping_add(val as u64);
            Ok(format!("Added {val} to the time"))
        }
        "query" => {
            if args.len() < 2 {
                return Err(CommandError::MissingArgument(
                    "Expected 'daytime', 'gametime', or 'day'".into(),
                ));
            }
            match args[1].to_ascii_lowercase().as_str() {
                "daytime" => {
                    let val = dispatcher.world_time % 24000;
                    Ok(format!("The time is {val}"))
                }
                "gametime" => {
                    let val = dispatcher.world_time;
                    Ok(format!("The time is {val}"))
                }
                "day" => {
                    let val = dispatcher.world_time / 24000;
                    Ok(format!("The time is {val}"))
                }
                other => Err(CommandError::InvalidArgument(format!(
                    "Unknown time query target '{other}', expected 'daytime', 'gametime', or 'day'"
                ))),
            }
        }
        other => Err(CommandError::InvalidArgument(format!(
            "Unknown subcommand '{other}', expected 'set', 'add', or 'query'"
        ))),
    }
}

fn handle_give(
    dispatcher: &mut CommandDispatcher,
    sender: &CommandSender,
    args: &[&str],
) -> Result<String, CommandError> {
    if args.is_empty() {
        return Err(CommandError::MissingArgument(
            "Usage: /give <item> [count] or /give <player> <item> [count]".into(),
        ));
    }

    // Disambiguate arguments:
    // If sender is a player and args.len() is 1: /give <item>
    // If sender is a player and args.len() is 2 with second arg numeric: /give <item> [count]
    // Otherwise: /give <player> <item> [count]
    let (target_name, item_str, count) = if args.len() == 1 {
        let player_name = match sender {
            CommandSender::Player(name) => name.clone(),
            CommandSender::Console => {
                return Err(CommandError::ConsoleNotAllowed(
                    "Console must specify target player. Usage: /give <player> <item> [count]"
                        .into(),
                ));
            }
        };
        (player_name, args[0], 1u32)
    } else if args.len() == 2 {
        if let Ok(c) = args[1].parse::<i64>() {
            let player_name = match sender {
                CommandSender::Player(name) => name.clone(),
                CommandSender::Console => {
                    return Err(CommandError::ConsoleNotAllowed(
                        "Console must specify target player. Usage: /give <player> <item> [count]"
                            .into(),
                    ));
                }
            };
            if c < 1 {
                return Err(CommandError::InvalidArgument(
                    "Count must be at least 1".into(),
                ));
            }
            if c > 6400 {
                return Err(CommandError::InvalidArgument(
                    "Count must not be larger than 6400".into(),
                ));
            }
            (player_name, args[0], c as u32)
        } else {
            (args[0].to_string(), args[1], 1u32)
        }
    } else if args.len() == 3 {
        let c: i64 = args[2]
            .parse()
            .map_err(|_| CommandError::InvalidArgument(format!("Invalid count '{}'", args[2])))?;
        if c < 1 {
            return Err(CommandError::InvalidArgument(
                "Count must be at least 1".into(),
            ));
        }
        if c > 6400 {
            return Err(CommandError::InvalidArgument(
                "Count must not be larger than 6400".into(),
            ));
        }
        (args[0].to_string(), args[1], c as u32)
    } else {
        return Err(CommandError::InvalidArgument(
            "Too many arguments for /give".into(),
        ));
    };

    let item_clean = item_str.trim();
    if item_clean.is_empty() {
        return Err(CommandError::InvalidArgument(
            "Item ID cannot be empty".into(),
        ));
    }
    let canonical_item = item_clean.strip_prefix("minecraft:").unwrap_or(item_clean);

    let player = dispatcher
        .players
        .entry(target_name.clone())
        .or_insert_with(|| PlayerState::new(&target_name));
    player.give_item(canonical_item, count);

    Ok(format!("Gave {count} [{canonical_item}] to {target_name}"))
}

/// Tokenize a command line string into whitespace-separated arguments,
/// supporting single-quoted and double-quoted strings.
pub fn tokenize(command_line: &str) -> Result<Vec<String>, CommandError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = command_line.chars().peekable();
    let mut in_quote = None;

    while let Some(ch) = chars.next() {
        match ch {
            '"' | '\'' => {
                if let Some(quote_char) = in_quote {
                    if quote_char == ch {
                        in_quote = None;
                    } else {
                        current.push(ch);
                    }
                } else {
                    in_quote = Some(ch);
                }
            }
            ' ' | '\t' | '\r' | '\n' => {
                if in_quote.is_some() {
                    current.push(ch);
                } else if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            '\\' => {
                if let Some(next) = chars.peek().copied()
                    && (next == '"' || next == '\'' || next == '\\')
                {
                    chars.next();
                    current.push(next);
                    continue;
                }
                current.push(ch);
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if in_quote.is_some() {
        return Err(CommandError::InvalidArgument(
            "Unclosed quoted string in command".into(),
        ));
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_packet_codec_round_trip() {
        let packet = ChatPacket::new("Hello world!", 1_700_000_000, 123456789, None).unwrap();
        let encoded = packet.encode_packet();
        assert_eq!(encoded[0], CHAT_PACKET_ID as u8);

        let decoded = ChatPacket::decode_packet(&encoded).unwrap();
        assert_eq!(packet, decoded);

        // Framed round trip
        let framed = packet.to_frame();
        let decoded_framed = ChatPacket::decode(&framed).unwrap();
        assert_eq!(packet, decoded_framed);

        // Signed round trip
        let signed = ChatPacket::with_signature(
            "Signed message",
            1_700_000_123,
            987654321,
            vec![0xAA, 0xBB, 0xCC, 0xDD],
        )
        .unwrap();
        let encoded_signed = signed.to_frame();
        let decoded_signed = ChatPacket::decode(&encoded_signed).unwrap();
        assert_eq!(signed, decoded_signed);
        assert_eq!(
            decoded_signed.signature_bytes(),
            Some(&[0xAA, 0xBB, 0xCC, 0xDD][..])
        );
    }

    #[test]
    fn test_chat_packet_length_bounds() {
        let max_str = "A".repeat(256);
        assert!(ChatPacket::unsigned(max_str, 0, 0).is_ok());

        let oversized = "A".repeat(257);
        let err = ChatPacket::unsigned(oversized, 0, 0).unwrap_err();
        assert!(matches!(
            err,
            ProtocolError::StringTooLong {
                max: 256,
                actual: 257
            }
        ));
    }

    #[test]
    fn test_chat_command_packet_codec_round_trip() {
        let packet = ChatCommandPacket::new(
            "tp 100 64 200",
            1_700_000_000,
            42,
            vec![ArgumentSignature {
                name: "target".into(),
                signature: vec![1, 2, 3, 4],
            }],
        )
        .unwrap();

        // Stored without leading slash
        assert_eq!(packet.command, "tp 100 64 200");

        let encoded = packet.to_frame();
        let decoded = ChatCommandPacket::decode(&encoded).unwrap();
        assert_eq!(packet, decoded);

        // Test stripping leading slash on construction
        let with_slash = ChatCommandPacket::unsigned("/gamemode creative", 0, 0).unwrap();
        assert_eq!(with_slash.command, "gamemode creative");

        // Test max length
        let max_cmd = "a".repeat(256);
        assert!(ChatCommandPacket::unsigned(max_cmd, 0, 0).is_ok());
        let oversized = "a".repeat(257);
        assert!(ChatCommandPacket::unsigned(oversized, 0, 0).is_err());
    }

    #[test]
    fn test_system_chat_message_packet_codec_round_trip() {
        let packet = SystemChatMessagePacket::new("{\"text\":\"Server rebooting\"}", false);
        let encoded = packet.to_frame();
        let decoded = SystemChatMessagePacket::decode(&encoded).unwrap();
        assert_eq!(packet, decoded);
        assert!(!decoded.overlay);

        let overlay_packet = SystemChatMessagePacket::overlay_text("Action bar message");
        let decoded_overlay = SystemChatMessagePacket::decode(&overlay_packet.to_frame()).unwrap();
        assert!(decoded_overlay.overlay);
        assert!(decoded_overlay.content.contains("Action bar message"));
    }

    #[test]
    fn test_chat_component_json_serialization() {
        // Plain text
        let plain = ChatComponent::text("Hello, world!");
        assert_eq!(plain.to_json(), "{\"text\":\"Hello, world!\"}");

        // Colored text
        let colored = ChatComponent::colored("Red text", ChatColor::Red);
        assert_eq!(
            colored.to_json(),
            "{\"color\":\"red\",\"text\":\"Red text\"}"
        );

        // Formatted sender message
        let player_msg = ChatComponent::player_message("Alice", "Hello there");
        assert_eq!(player_msg.to_json(), "{\"text\":\"<Alice> Hello there\"}");

        // Complex styled component with extra children
        let complex = ChatComponent::text("Prefix: ")
            .with_color(ChatColor::Gold)
            .with_bold(true)
            .add_extra(ChatComponent::colored("Suffix", ChatColor::Green).with_italic(true));
        let json = complex.to_json();
        assert!(json.contains("\"color\":\"gold\""));
        assert!(json.contains("\"bold\":true"));
        assert!(json.contains("\"extra\":["));
        assert!(json.contains("\"color\":\"green\""));
        assert!(json.contains("\"italic\":true"));

        // Round trip from_json
        let parsed = ChatComponent::from_json(&json).unwrap();
        assert_eq!(complex.text, parsed.text);
        assert_eq!(complex.color, parsed.color);
        assert_eq!(complex.bold, parsed.bold);
        assert_eq!(complex.extra.len(), 1);
        assert_eq!(complex.extra[0].text, "Suffix");
    }

    #[test]
    fn test_broadcast_chat_helper() {
        let packet = broadcast_chat("Bob", "testing 123");
        assert!(!packet.overlay);
        assert_eq!(packet.content, "{\"text\":\"<Bob> testing 123\"}");
    }

    #[test]
    fn test_command_dispatcher_help() {
        let mut dispatcher = CommandDispatcher::new();
        let sender = CommandSender::Console;

        let help = dispatcher.dispatch(&sender, "/help").unwrap();
        assert!(help.contains("/tp"));
        assert!(help.contains("/gamemode"));
        assert!(help.contains("/time"));
        assert!(help.contains("/give"));

        let help_tp = dispatcher.dispatch(&sender, "help tp").unwrap();
        assert!(help_tp.contains("/tp <x> <y> <z>"));

        let help_unknown = dispatcher.dispatch(&sender, "/help foobar");
        assert!(help_unknown.is_err());
    }

    #[test]
    fn test_command_dispatcher_tp() {
        let mut dispatcher = CommandDispatcher::new();
        let player = PlayerState::new("Player1").with_position(10.0, 64.0, 20.0);
        dispatcher.add_player(player);

        let sender = CommandSender::Player("Player1".into());

        // Absolute tp
        let res = dispatcher.dispatch(&sender, "/tp 100 70 -50").unwrap();
        assert!(res.contains("Teleported Player1 to 100.00, 70.00, -50.00"));
        let p = dispatcher.get_player("Player1").unwrap();
        assert_eq!((p.x, p.y, p.z), (100.0, 70.0, -50.0));

        // Relative tp with tildes
        let res2 = dispatcher.dispatch(&sender, "/tp ~5 ~-10 ~").unwrap();
        assert!(res2.contains("Teleported Player1 to 105.00, 60.00, -50.00"));
        let p = dispatcher.get_player("Player1").unwrap();
        assert_eq!((p.x, p.y, p.z), (105.0, 60.0, -50.0));

        // Invalid tp coordinates
        let err = dispatcher.dispatch(&sender, "/tp abc 70 80").unwrap_err();
        assert!(matches!(err, CommandError::InvalidCoordinate(_)));

        // Missing coordinates
        let err_missing = dispatcher.dispatch(&sender, "/tp 100").unwrap_err();
        assert!(matches!(err_missing, CommandError::MissingArgument(_)));
    }

    #[test]
    fn test_command_dispatcher_gamemode() {
        let mut dispatcher = CommandDispatcher::new();
        dispatcher.add_player(PlayerState::new("Player1"));

        let sender = CommandSender::Player("Player1".into());

        let res = dispatcher.dispatch(&sender, "/gamemode creative").unwrap();
        assert!(res.contains("Set Player1's game mode to creative"));
        assert_eq!(
            dispatcher.get_player("Player1").unwrap().gamemode,
            GameMode::Creative
        );

        let res_alias = dispatcher.dispatch(&sender, "/gamemode s").unwrap();
        assert!(res_alias.contains("Set Player1's game mode to survival"));
        assert_eq!(
            dispatcher.get_player("Player1").unwrap().gamemode,
            GameMode::Survival
        );

        let err = dispatcher
            .dispatch(&sender, "/gamemode godmode")
            .unwrap_err();
        assert!(matches!(err, CommandError::InvalidArgument(_)));
    }

    #[test]
    fn test_command_dispatcher_time() {
        let mut dispatcher = CommandDispatcher::new();
        let sender = CommandSender::Console;

        let res = dispatcher.dispatch(&sender, "/time set day").unwrap();
        assert_eq!(dispatcher.world_time, 1000);
        assert!(res.contains("Set the time to 1000"));

        let res_night = dispatcher.dispatch(&sender, "/time set night").unwrap();
        assert_eq!(dispatcher.world_time, 13000);
        assert!(res_night.contains("Set the time to 13000"));

        let res_ticks = dispatcher.dispatch(&sender, "/time set 18000").unwrap();
        assert_eq!(dispatcher.world_time, 18000);
        assert!(res_ticks.contains("Set the time to 18000"));

        let err = dispatcher
            .dispatch(&sender, "/time set invalid")
            .unwrap_err();
        assert!(matches!(err, CommandError::InvalidArgument(_)));
    }

    #[test]
    fn test_command_dispatcher_give() {
        let mut dispatcher = CommandDispatcher::new();
        dispatcher.add_player(PlayerState::new("Steve"));

        let sender = CommandSender::Player("Steve".into());

        // Default count = 1
        let res = dispatcher.dispatch(&sender, "/give diamond").unwrap();
        assert!(res.contains("Gave 1 [diamond] to Steve"));
        let p = dispatcher.get_player("Steve").unwrap();
        assert_eq!(*p.inventory.get("diamond").unwrap(), 1);

        // Explicit count
        let res64 = dispatcher
            .dispatch(&sender, "/give minecraft:apple 64")
            .unwrap();
        assert!(res64.contains("Gave 64 [apple] to Steve"));
        let p = dispatcher.get_player("Steve").unwrap();
        assert_eq!(*p.inventory.get("apple").unwrap(), 64);

        // Invalid count
        let err = dispatcher
            .dispatch(&sender, "/give iron_ingot 0")
            .unwrap_err();
        assert!(matches!(err, CommandError::InvalidArgument(_)));
    }

    #[test]
    fn test_command_dispatcher_unknown_command() {
        let mut dispatcher = CommandDispatcher::new();
        let sender = CommandSender::Console;

        let err = dispatcher
            .dispatch(&sender, "/nonexistent arg1 arg2")
            .unwrap_err();
        assert_eq!(err, CommandError::UnknownCommand("nonexistent".into()));
    }

    #[test]
    fn test_command_dispatcher_dispatch_to_chat() {
        let mut dispatcher = CommandDispatcher::new();
        let sender = CommandSender::Console;

        let success_packet = dispatcher.dispatch_to_chat(&sender, "/time set 5000");
        assert!(!success_packet.overlay);
        assert!(success_packet.content.contains("5000"));

        let error_packet = dispatcher.dispatch_to_chat(&sender, "/unknown_cmd");
        assert!(!error_packet.overlay);
        assert!(error_packet.content.contains("red"));
        assert!(error_packet.content.contains("Unknown command"));
    }

    #[test]
    fn test_packet_codec_malformed_and_errors() {
        // Empty bytes
        assert_eq!(ChatPacket::decode(&[]), Err(ProtocolError::UnexpectedEof));
        assert_eq!(
            ChatCommandPacket::decode(&[]),
            Err(ProtocolError::UnexpectedEof)
        );
        assert_eq!(
            SystemChatMessagePacket::decode(&[]),
            Err(ProtocolError::UnexpectedEof)
        );

        // Wrong packet ID
        let wrong_id_packet = frame(0x99, &[0; 10]);
        assert!(matches!(
            ChatPacket::decode(&wrong_id_packet),
            Err(ProtocolError::WrongPacketId {
                expected: CHAT_PACKET_ID,
                actual: 0x99
            })
        ));
        assert!(matches!(
            ChatCommandPacket::decode(&wrong_id_packet),
            Err(ProtocolError::WrongPacketId {
                expected: CHAT_COMMAND_PACKET_ID,
                actual: 0x99
            })
        ));
        assert!(matches!(
            SystemChatMessagePacket::decode(&wrong_id_packet),
            Err(ProtocolError::WrongPacketId {
                expected: SYSTEM_CHAT_MESSAGE_PACKET_ID,
                actual: 0x99
            })
        ));

        // Truncated ChatPacket payload
        let mut truncated = vec![CHAT_PACKET_ID as u8];
        put_string("Hi", &mut truncated);
        // Only 8 bytes of timestamp, missing salt
        truncated.extend_from_slice(&[0; 8]);
        assert_eq!(
            ChatPacket::decode_packet(&truncated),
            Err(ProtocolError::UnexpectedEof)
        );

        // SystemChatMessagePacket with invalid overlay byte (e.g. 0x02)
        let mut bad_overlay = vec![SYSTEM_CHAT_MESSAGE_PACKET_ID as u8];
        put_string("{}", &mut bad_overlay);
        bad_overlay.push(0x02);
        assert!(matches!(
            SystemChatMessagePacket::decode_packet(&bad_overlay),
            Err(ProtocolError::Malformed(_))
        ));
    }

    #[test]
    fn test_chat_component_styling_and_hex_colors() {
        // Hex color
        let custom_color = ChatColor::parse("#1234AB").unwrap();
        assert_eq!(custom_color.as_str(), "#1234ab");
        let comp = ChatComponent::colored("Custom", custom_color.clone())
            .with_underlined(true)
            .with_strikethrough(true)
            .with_obfuscated(true);
        let json = comp.to_json();
        assert!(json.contains("\"color\":\"#1234ab\""));
        assert!(json.contains("\"underlined\":true"));
        assert!(json.contains("\"strikethrough\":true"));
        assert!(json.contains("\"obfuscated\":true"));

        // Plain text extraction
        let parent = ChatComponent::text("First ")
            .add_extra(ChatComponent::text("Second "))
            .add_extra(ChatComponent::text("Third"));
        assert_eq!(parent.to_plain_text(), "First Second Third");

        // Display trait output matches to_json
        assert_eq!(format!("{comp}"), comp.to_json());
    }

    #[test]
    fn test_tokenizer_quoted_strings_and_escapes() {
        let tokens = tokenize("say \"hello world\" 'single quoted' standard").unwrap();
        assert_eq!(
            tokens,
            vec!["say", "hello world", "single quoted", "standard"]
        );

        let escaped = tokenize("give steve \"diamond\\\"sword\"").unwrap();
        assert_eq!(escaped, vec!["give", "steve", "diamond\"sword"]);

        let unclosed = tokenize("say \"unclosed");
        assert!(matches!(unclosed, Err(CommandError::InvalidArgument(_))));
    }

    #[test]
    fn test_command_dispatcher_extended() {
        let mut dispatcher = CommandDispatcher::new();
        dispatcher.add_player(PlayerState::new("Alice").with_position(0.0, 64.0, 0.0));
        let console = CommandSender::Console;

        // /tp <target> <x> <y> <z> from Console
        let res = dispatcher
            .dispatch(&console, "/tp Alice 50 100 150")
            .unwrap();
        assert!(res.contains("Teleported Alice to 50.00, 100.00, 150.00"));
        assert_eq!(
            (
                dispatcher.get_player("Alice").unwrap().x,
                dispatcher.get_player("Alice").unwrap().y,
                dispatcher.get_player("Alice").unwrap().z
            ),
            (50.0, 100.0, 150.0)
        );

        // /tp out of bounds
        let player_sender = CommandSender::Player("Alice".into());
        let err_oob = dispatcher
            .dispatch(&player_sender, "/tp 40000000 64 0")
            .unwrap_err();
        assert!(matches!(err_oob, CommandError::InvalidCoordinate(_)));

        // /time add
        dispatcher.dispatch(&console, "/time set 1000").unwrap();
        dispatcher.dispatch(&console, "/time add 500").unwrap();
        assert_eq!(dispatcher.world_time, 1500);

        // /time query
        let q_daytime = dispatcher
            .dispatch(&console, "/time query daytime")
            .unwrap();
        assert!(q_daytime.contains("1500"));

        // Custom command registration
        dispatcher.register("ping", "Replies with pong", "/ping", |_d, _s, _args| {
            Ok("pong!".to_string())
        });
        let pong = dispatcher.dispatch(&console, "/ping").unwrap();
        assert_eq!(pong, "pong!");
    }
}
