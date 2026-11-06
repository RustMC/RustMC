//! Serverbound chat packets, clientbound system chat messages, JSON chat components,
//! and command dispatcher for Minecraft Java Edition.

use std::{collections::HashMap, error::Error, fmt, sync::Arc};

// Packet IDs come from the 26.3 server's generated packet report. Layouts follow
// the public protocol reference for 26.3 / protocol 777:
// https://minecraft.wiki/w/Java_Edition_protocol/Packets (revision 3868170) and
// https://minecraft.wiki/w/Java_Edition_protocol/Data_types (revision 3855787).

/// Serverbound `chat` packet ID.
pub const CHAT_PACKET_ID: u32 = 0x09;

/// Serverbound `chat_ack` packet ID.
pub const CHAT_ACK_PACKET_ID: u32 = 0x06;

/// Serverbound `chat_command` packet ID (unsigned layout).
pub const CHAT_COMMAND_PACKET_ID: u32 = 0x07;

/// Serverbound `chat_command_signed` packet ID.
pub const CHAT_COMMAND_SIGNED_PACKET_ID: u32 = 0x08;

/// Serverbound `chat_session_update` packet ID.
pub const CHAT_SESSION_UPDATE_PACKET_ID: u32 = 0x0A;

/// Clientbound `system_chat` packet ID.
pub const SYSTEM_CHAT_MESSAGE_PACKET_ID: u32 = 0x7C;

/// Maximum chat message length in UTF-16 code units.
pub const MAX_CHAT_LENGTH: usize = 256;

/// Maximum command length in UTF-16 code units.
pub const MAX_COMMAND_LENGTH: usize = 32767;

/// Maximum signed arguments in `chat_command_signed`.
pub const MAX_ARGUMENT_SIGNATURES: usize = 8;

/// Maximum signed argument name length in UTF-16 code units.
pub const MAX_ARGUMENT_NAME_LENGTH: usize = 16;

/// Length of a chat or argument signature, which has no length prefix.
pub const SIGNATURE_LENGTH: usize = 256;

/// Bytes in the last-seen Fixed BitSet (20).
pub const LAST_SEEN_BITSET_BYTES: usize = 3;

/// Maximum encoded `system_chat` content in bytes.
pub const MAX_TEXT_COMPONENT_BYTES: usize = 262_144;

/// RustMC policy bound on text component nesting. It limits decoder and encoder
/// recursion; it is not a protocol value.
pub const MAX_TEXT_COMPONENT_DEPTH: usize = 32;

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

/// Strip a `[VarInt(length), VarInt(id), ...]` frame and return its payload.
fn expect_frame(bytes: &[u8], expected: u32) -> Result<&[u8], ProtocolError> {
    let (id, body) = unframe(bytes)?;
    if id != expected {
        return Err(ProtocolError::WrongPacketId {
            expected,
            actual: id,
        });
    }
    Ok(body)
}

/// Strip the `VarInt(id)` of an unframed packet and return its payload.
fn expect_packet(bytes: &[u8], expected: u32) -> Result<&[u8], ProtocolError> {
    let (id, prefix) = read_varint(bytes)?;
    if id != expected {
        return Err(ProtocolError::WrongPacketId {
            expected,
            actual: id,
        });
    }
    Ok(&bytes[prefix..])
}

/// Prefix an unframed packet with its VarInt length.
fn with_length(packet: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(packet.len() + 5);
    put_varint(packet.len() as u32, &mut out);
    out.extend(packet);
    out
}

fn check_utf16_len(value: &str, max: usize) -> Result<(), ProtocolError> {
    let actual = value.encode_utf16().count();
    if actual > max {
        return Err(ProtocolError::StringTooLong { max, actual });
    }
    Ok(())
}

/// Read a protocol string whose limit counts UTF-16 code units, rejecting a
/// hostile length prefix before any UTF-8 validation.
fn read_bounded_string(input: &[u8], max: usize) -> Result<(&str, usize), ProtocolError> {
    let (length, _) = read_varint(input)?;
    if length as usize > max.saturating_mul(3) {
        return Err(ProtocolError::StringTooLong {
            max,
            actual: length as usize,
        });
    }
    let (value, consumed) = read_string(input)?;
    check_utf16_len(value, max)?;
    Ok((value, consumed))
}

fn read_i64(input: &[u8]) -> Result<(i64, &[u8]), ProtocolError> {
    let (bytes, rest) = input
        .split_first_chunk::<8>()
        .ok_or(ProtocolError::UnexpectedEof)?;
    Ok((i64::from_be_bytes(*bytes), rest))
}

fn read_signature(input: &[u8]) -> Result<([u8; SIGNATURE_LENGTH], &[u8]), ProtocolError> {
    let (bytes, rest) = input
        .split_first_chunk::<SIGNATURE_LENGTH>()
        .ok_or(ProtocolError::UnexpectedEof)?;
    Ok((*bytes, rest))
}

/// The last-seen acknowledgment tail of `chat` and `chat_command_signed`.
///
/// These fields are mandatory, including when a message carries no signature.
/// RustMC never sends signed `player_chat` messages, so they are parsed for
/// framing and retained but not validated against a last-seen list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LastSeenUpdate {
    /// Message Count: messages seen since the client's last chat packet.
    pub message_count: u32,
    /// Acknowledged: Fixed BitSet (20), encoded as `ceil(20 / 8)` bytes.
    pub acknowledged: [u8; LAST_SEEN_BITSET_BYTES],
    /// Checksum over the last-seen signatures.
    pub checksum: u8,
}

impl LastSeenUpdate {
    fn write(&self, out: &mut Vec<u8>) {
        put_varint(self.message_count, out);
        out.extend_from_slice(&self.acknowledged);
        out.push(self.checksum);
    }

    fn read(input: &[u8]) -> Result<(Self, &[u8]), ProtocolError> {
        let (message_count, prefix) = read_varint(input)?;
        let (acknowledged, rest) = input[prefix..]
            .split_first_chunk::<LAST_SEEN_BITSET_BYTES>()
            .ok_or(ProtocolError::UnexpectedEof)?;
        let (checksum, rest) = rest.split_first().ok_or(ProtocolError::UnexpectedEof)?;
        Ok((
            Self {
                message_count,
                acknowledged: *acknowledged,
                checksum: *checksum,
            },
            rest,
        ))
    }
}

/// Serverbound `chat` packet (`0x09`).
///
/// Layout: String (256) message, Long timestamp, Long salt, Prefixed Optional
/// 256-byte signature, then the [`LastSeenUpdate`] tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatPacket {
    /// Message string (at most 256 UTF-16 code units).
    pub message: String,
    /// Timestamp (epoch milliseconds).
    pub timestamp: i64,
    /// Signature salt.
    pub salt: i64,
    /// Fixed-size signature, absent for unsigned messages.
    pub signature: Option<[u8; SIGNATURE_LENGTH]>,
    /// Mandatory acknowledgment tail.
    pub last_seen: LastSeenUpdate,
}

impl ChatPacket {
    /// Create a new ChatPacket with validation of the message length.
    pub fn new(
        message: impl Into<String>,
        timestamp: i64,
        salt: i64,
        signature: Option<[u8; SIGNATURE_LENGTH]>,
        last_seen: LastSeenUpdate,
    ) -> Result<Self, ProtocolError> {
        let message = message.into();
        check_utf16_len(&message, MAX_CHAT_LENGTH)?;
        Ok(Self {
            message,
            timestamp,
            salt,
            signature,
            last_seen,
        })
    }

    /// Create an unsigned ChatPacket with an empty acknowledgment tail.
    pub fn unsigned(
        message: impl Into<String>,
        timestamp: i64,
        salt: i64,
    ) -> Result<Self, ProtocolError> {
        Self::new(message, timestamp, salt, None, LastSeenUpdate::default())
    }

    /// Encode as an unframed packet: `[VarInt(0x09), payload]`.
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(CHAT_PACKET_ID, &mut out);
        put_string(&self.message, &mut out);
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.salt.to_be_bytes());
        match &self.signature {
            Some(signature) => {
                out.push(1);
                out.extend_from_slice(signature);
            }
            None => out.push(0),
        }
        self.last_seen.write(&mut out);
        out
    }

    /// Encode as a framed packet: `[VarInt(length), VarInt(0x09), payload]`.
    pub fn to_frame(&self) -> Vec<u8> {
        with_length(self.encode_packet())
    }

    /// Decode a framed packet: `[VarInt(length), VarInt(0x09), payload]`.
    pub fn decode_frame(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_frame(bytes, CHAT_PACKET_ID)?)
    }

    /// Decode an unframed packet: `[VarInt(0x09), payload]`.
    pub fn decode_packet(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_packet(bytes, CHAT_PACKET_ID)?)
    }

    /// Decode a payload without length or packet ID.
    pub fn decode_payload(body: &[u8]) -> Result<Self, ProtocolError> {
        let (message, consumed) = read_bounded_string(body, MAX_CHAT_LENGTH)?;
        let (timestamp, rest) = read_i64(&body[consumed..])?;
        let (salt, rest) = read_i64(rest)?;
        let (present, rest) = rest.split_first().ok_or(ProtocolError::UnexpectedEof)?;
        let (signature, rest) = match present {
            0 => (None, rest),
            1 => {
                let (signature, rest) = read_signature(rest)?;
                (Some(signature), rest)
            }
            other => {
                return Err(ProtocolError::Malformed(format!(
                    "Invalid boolean byte for signature presence: 0x{other:02X}"
                )));
            }
        };
        let (last_seen, rest) = LastSeenUpdate::read(rest)?;
        if !rest.is_empty() {
            return Err(ProtocolError::Malformed(
                "Trailing bytes in chat packet".into(),
            ));
        }
        Ok(Self {
            message: message.to_string(),
            timestamp,
            salt,
            signature,
            last_seen,
        })
    }
}

/// Serverbound `chat_command` packet (`0x07`): only the unsigned command text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatCommandPacket {
    /// Command text without its leading `/`.
    pub command: String,
}

impl ChatCommandPacket {
    /// Create a command packet, stripping any leading `/` and checking length.
    pub fn new(command: impl Into<String>) -> Result<Self, ProtocolError> {
        let raw = command.into();
        let command = raw.strip_prefix('/').unwrap_or(&raw).to_string();
        check_utf16_len(&command, MAX_COMMAND_LENGTH)?;
        Ok(Self { command })
    }

    /// Encode as an unframed packet: `[VarInt(0x07), command]`.
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(CHAT_COMMAND_PACKET_ID, &mut out);
        put_string(&self.command, &mut out);
        out
    }

    /// Encode as a framed packet: `[VarInt(length), VarInt(0x07), command]`.
    pub fn to_frame(&self) -> Vec<u8> {
        with_length(self.encode_packet())
    }

    /// Decode a framed packet: `[VarInt(length), VarInt(0x07), command]`.
    pub fn decode_frame(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_frame(bytes, CHAT_COMMAND_PACKET_ID)?)
    }

    /// Decode an unframed packet: `[VarInt(0x07), command]`.
    pub fn decode_packet(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_packet(bytes, CHAT_COMMAND_PACKET_ID)?)
    }

    /// Decode a payload without length or packet ID.
    pub fn decode_payload(body: &[u8]) -> Result<Self, ProtocolError> {
        let (command, consumed) = read_bounded_string(body, MAX_COMMAND_LENGTH)?;
        if consumed != body.len() {
            return Err(ProtocolError::Malformed(
                "Trailing bytes in chat_command packet".into(),
            ));
        }
        Ok(Self {
            command: command.to_string(),
        })
    }
}

/// One signed command argument: String (16) name and a fixed 256-byte signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentSignature {
    pub name: String,
    pub signature: [u8; SIGNATURE_LENGTH],
}

/// Serverbound `chat_command_signed` packet (`0x08`).
///
/// Layout: String (32767) command, Long timestamp, Long salt, Prefixed Array
/// (8) of [`ArgumentSignature`], then the [`LastSeenUpdate`] tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedChatCommandPacket {
    /// Command text without its leading `/`.
    pub command: String,
    /// Timestamp (epoch milliseconds).
    pub timestamp: i64,
    /// Signature salt.
    pub salt: i64,
    /// Signatures for signable arguments.
    pub argument_signatures: Vec<ArgumentSignature>,
    /// Mandatory acknowledgment tail.
    pub last_seen: LastSeenUpdate,
}

impl SignedChatCommandPacket {
    /// Create a signed command packet, validating every bounded field.
    pub fn new(
        command: impl Into<String>,
        timestamp: i64,
        salt: i64,
        argument_signatures: Vec<ArgumentSignature>,
        last_seen: LastSeenUpdate,
    ) -> Result<Self, ProtocolError> {
        let raw = command.into();
        let command = raw.strip_prefix('/').unwrap_or(&raw).to_string();
        check_utf16_len(&command, MAX_COMMAND_LENGTH)?;
        if argument_signatures.len() > MAX_ARGUMENT_SIGNATURES {
            return Err(ProtocolError::Oversized(format!(
                "{} argument signatures exceed the limit of {MAX_ARGUMENT_SIGNATURES}",
                argument_signatures.len()
            )));
        }
        for argument in &argument_signatures {
            check_utf16_len(&argument.name, MAX_ARGUMENT_NAME_LENGTH)?;
        }
        Ok(Self {
            command,
            timestamp,
            salt,
            argument_signatures,
            last_seen,
        })
    }

    /// Encode as an unframed packet: `[VarInt(0x08), payload]`.
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(CHAT_COMMAND_SIGNED_PACKET_ID, &mut out);
        put_string(&self.command, &mut out);
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.salt.to_be_bytes());
        put_varint(self.argument_signatures.len() as u32, &mut out);
        for argument in &self.argument_signatures {
            put_string(&argument.name, &mut out);
            out.extend_from_slice(&argument.signature);
        }
        self.last_seen.write(&mut out);
        out
    }

    /// Encode as a framed packet: `[VarInt(length), VarInt(0x08), payload]`.
    pub fn to_frame(&self) -> Vec<u8> {
        with_length(self.encode_packet())
    }

    /// Decode a framed packet: `[VarInt(length), VarInt(0x08), payload]`.
    pub fn decode_frame(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_frame(bytes, CHAT_COMMAND_SIGNED_PACKET_ID)?)
    }

    /// Decode an unframed packet: `[VarInt(0x08), payload]`.
    pub fn decode_packet(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_packet(bytes, CHAT_COMMAND_SIGNED_PACKET_ID)?)
    }

    /// Decode a payload without length or packet ID.
    pub fn decode_payload(body: &[u8]) -> Result<Self, ProtocolError> {
        let (command, consumed) = read_bounded_string(body, MAX_COMMAND_LENGTH)?;
        let (timestamp, rest) = read_i64(&body[consumed..])?;
        let (salt, rest) = read_i64(rest)?;
        let (count, prefix) = read_varint(rest)?;
        let count = count as usize;
        if count > MAX_ARGUMENT_SIGNATURES {
            return Err(ProtocolError::Oversized(format!(
                "{count} argument signatures exceed the limit of {MAX_ARGUMENT_SIGNATURES}"
            )));
        }
        let mut rest = &rest[prefix..];
        let mut argument_signatures = Vec::with_capacity(count);
        for _ in 0..count {
            let (name, consumed) = read_bounded_string(rest, MAX_ARGUMENT_NAME_LENGTH)?;
            let (signature, after) = read_signature(&rest[consumed..])?;
            argument_signatures.push(ArgumentSignature {
                name: name.to_string(),
                signature,
            });
            rest = after;
        }
        let (last_seen, rest) = LastSeenUpdate::read(rest)?;
        if !rest.is_empty() {
            return Err(ProtocolError::Malformed(
                "Trailing bytes in chat_command_signed packet".into(),
            ));
        }
        Ok(Self {
            command: command.to_string(),
            timestamp,
            salt,
            argument_signatures,
            last_seen,
        })
    }
}

/// Clientbound `system_chat` packet (`0x7C`): Text Component, Boolean overlay.
///
/// The component is validated and encoded on construction, so a value of this
/// type can always be written to the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemChatMessagePacket {
    content: ChatComponent,
    overlay: bool,
    content_nbt: Vec<u8>,
}

impl SystemChatMessagePacket {
    /// Create a packet, rejecting a component that exceeds the wire bounds.
    pub fn new(content: ChatComponent, overlay: bool) -> Result<Self, ProtocolError> {
        let content_nbt = content.to_network_nbt()?;
        Ok(Self {
            content,
            overlay,
            content_nbt,
        })
    }

    /// Create a message for the chat box.
    pub fn chat(component: ChatComponent) -> Result<Self, ProtocolError> {
        Self::new(component, false)
    }

    /// Create a message for the action bar.
    pub fn overlay(component: ChatComponent) -> Result<Self, ProtocolError> {
        Self::new(component, true)
    }

    /// Create a plain message for the chat box.
    pub fn chat_text(text: impl Into<String>) -> Result<Self, ProtocolError> {
        Self::chat(ChatComponent::text(text))
    }

    /// Create a plain message for the action bar.
    pub fn overlay_text(text: impl Into<String>) -> Result<Self, ProtocolError> {
        Self::overlay(ChatComponent::text(text))
    }

    /// The displayed component.
    pub fn content(&self) -> &ChatComponent {
        &self.content
    }

    /// `true` for the action bar, `false` for the chat box.
    pub fn is_overlay(&self) -> bool {
        self.overlay
    }

    /// Encode as an unframed packet: `[VarInt(0x7C), network NBT, overlay]`.
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.content_nbt.len() + 3);
        put_varint(SYSTEM_CHAT_MESSAGE_PACKET_ID, &mut out);
        out.extend_from_slice(&self.content_nbt);
        out.push(u8::from(self.overlay));
        out
    }

    /// Encode as a framed packet: `[VarInt(length), VarInt(0x7C), payload]`.
    pub fn to_frame(&self) -> Vec<u8> {
        with_length(self.encode_packet())
    }

    /// Decode a framed packet: `[VarInt(length), VarInt(0x7C), payload]`.
    pub fn decode_frame(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_frame(bytes, SYSTEM_CHAT_MESSAGE_PACKET_ID)?)
    }

    /// Decode an unframed packet: `[VarInt(0x7C), payload]`.
    pub fn decode_packet(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_packet(bytes, SYSTEM_CHAT_MESSAGE_PACKET_ID)?)
    }

    /// Decode a payload without length or packet ID.
    pub fn decode_payload(body: &[u8]) -> Result<Self, ProtocolError> {
        let (content, rest) = ChatComponent::read_network_nbt(body)?;
        let overlay = match rest {
            [0] => false,
            [1] => true,
            [other] => {
                return Err(ProtocolError::Malformed(format!(
                    "Invalid boolean byte for overlay flag: 0x{other:02X}"
                )));
            }
            _ => {
                return Err(ProtocolError::Malformed(
                    "Expected exactly 1 byte for overlay flag".into(),
                ));
            }
        };
        Self::new(content, overlay)
    }
}

// ============================================================================
// Network NBT text components
// ============================================================================

const TAG_END: u8 = 0;
const TAG_BYTE: u8 = 1;
const TAG_STRING: u8 = 8;
const TAG_LIST: u8 = 9;
const TAG_COMPOUND: u8 = 10;

/// Append `value` in Java's modified UTF-8: UTF-16 code units, with U+0000 as
/// `C0 80` and each surrogate encoded separately as three bytes.
fn put_modified_utf8(value: &str, out: &mut Vec<u8>) {
    for unit in value.encode_utf16() {
        match unit {
            0x0001..=0x007F => out.push(unit as u8),
            0x0000 | 0x0080..=0x07FF => {
                out.push(0xC0 | (unit >> 6) as u8);
                out.push(0x80 | (unit & 0x3F) as u8);
            }
            _ => {
                out.push(0xE0 | (unit >> 12) as u8);
                out.push(0x80 | ((unit >> 6) & 0x3F) as u8);
                out.push(0x80 | (unit & 0x3F) as u8);
            }
        }
    }
}

/// Decode Java's modified UTF-8, rejecting forms a Java reader cannot produce
/// and unpaired surrogates, which a Rust string cannot represent.
fn parse_modified_utf8(bytes: &[u8]) -> Result<String, ProtocolError> {
    let mut units = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let first = bytes[index];
        let continuation = |offset: usize| -> Result<u16, ProtocolError> {
            match bytes.get(index + offset) {
                Some(byte) if byte & 0xC0 == 0x80 => Ok(u16::from(byte & 0x3F)),
                _ => Err(ProtocolError::InvalidUtf8),
            }
        };
        let (unit, width) = match first {
            0x01..=0x7F => (u16::from(first), 1),
            0xC0..=0xDF => (u16::from(first & 0x1F) << 6 | continuation(1)?, 2),
            0xE0..=0xEF => (
                u16::from(first & 0x0F) << 12 | continuation(1)? << 6 | continuation(2)?,
                3,
            ),
            _ => return Err(ProtocolError::InvalidUtf8),
        };
        units.push(unit);
        index += width;
    }
    String::from_utf16(&units).map_err(|_| ProtocolError::InvalidUtf8)
}

/// Write a TAG_String payload: unsigned 16-bit byte length and modified UTF-8.
fn put_nbt_string(value: &str, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
    let mut encoded = Vec::with_capacity(value.len());
    put_modified_utf8(value, &mut encoded);
    let length = u16::try_from(encoded.len()).map_err(|_| {
        ProtocolError::Oversized(format!(
            "NBT string of {} bytes exceeds 65535",
            encoded.len()
        ))
    })?;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend(encoded);
    Ok(())
}

fn read_nbt_string(input: &[u8]) -> Result<(String, &[u8]), ProtocolError> {
    let (length, rest) = input
        .split_first_chunk::<2>()
        .ok_or(ProtocolError::UnexpectedEof)?;
    let length = usize::from(u16::from_be_bytes(*length));
    if rest.len() < length {
        return Err(ProtocolError::UnexpectedEof);
    }
    let (bytes, rest) = rest.split_at(length);
    Ok((parse_modified_utf8(bytes)?, rest))
}

fn put_nbt_named(tag: u8, name: &str, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
    out.push(tag);
    put_nbt_string(name, out)
}

impl ChatComponent {
    /// Encode as a network Text Component: an anonymous NBT tag that is a
    /// TAG_String for plain text and a TAG_Compound for anything else.
    pub fn to_network_nbt(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut out = Vec::new();
        if self.is_plain() {
            out.push(TAG_STRING);
            put_nbt_string(&self.text, &mut out)?;
        } else {
            out.push(TAG_COMPOUND);
            self.write_compound(&mut out, 1)?;
        }
        if out.len() > MAX_TEXT_COMPONENT_BYTES {
            return Err(ProtocolError::Oversized(format!(
                "Text component of {} bytes exceeds {MAX_TEXT_COMPONENT_BYTES}",
                out.len()
            )));
        }
        Ok(out)
    }

    fn is_plain(&self) -> bool {
        self.color.is_none()
            && self.bold.is_none()
            && self.italic.is_none()
            && self.underlined.is_none()
            && self.strikethrough.is_none()
            && self.obfuscated.is_none()
            && self.extra.is_empty()
    }

    fn write_compound(&self, out: &mut Vec<u8>, depth: usize) -> Result<(), ProtocolError> {
        if depth > MAX_TEXT_COMPONENT_DEPTH {
            return Err(ProtocolError::Oversized(format!(
                "Text component nesting exceeds {MAX_TEXT_COMPONENT_DEPTH}"
            )));
        }
        put_nbt_named(TAG_STRING, "text", out)?;
        put_nbt_string(&self.text, out)?;
        if let Some(color) = &self.color {
            put_nbt_named(TAG_STRING, "color", out)?;
            put_nbt_string(color.as_str(), out)?;
        }
        for (name, value) in self.style_flags() {
            if let Some(value) = value {
                put_nbt_named(TAG_BYTE, name, out)?;
                out.push(u8::from(value));
            }
        }
        if !self.extra.is_empty() {
            put_nbt_named(TAG_LIST, "extra", out)?;
            out.push(TAG_COMPOUND);
            out.extend_from_slice(&(self.extra.len() as i32).to_be_bytes());
            for child in &self.extra {
                child.write_compound(out, depth + 1)?;
            }
        }
        out.push(TAG_END);
        Ok(())
    }

    fn style_flags(&self) -> [(&'static str, Option<bool>); 5] {
        [
            ("bold", self.bold),
            ("italic", self.italic),
            ("underlined", self.underlined),
            ("strikethrough", self.strikethrough),
            ("obfuscated", self.obfuscated),
        ]
    }

    /// Read one anonymous network Text Component, returning the remaining
    /// bytes. A field this model cannot represent is rejected, never dropped.
    pub fn read_network_nbt(input: &[u8]) -> Result<(Self, &[u8]), ProtocolError> {
        let (tag, rest) = input.split_first().ok_or(ProtocolError::UnexpectedEof)?;
        let (component, rest) = match *tag {
            TAG_STRING => {
                let (text, rest) = read_nbt_string(rest)?;
                (Self::text(text), rest)
            }
            TAG_COMPOUND => Self::read_compound(rest, 1)?,
            other => {
                return Err(ProtocolError::Malformed(format!(
                    "Unsupported text component root tag {other}"
                )));
            }
        };
        if input.len() - rest.len() > MAX_TEXT_COMPONENT_BYTES {
            return Err(ProtocolError::Oversized(format!(
                "Text component exceeds {MAX_TEXT_COMPONENT_BYTES} bytes"
            )));
        }
        Ok((component, rest))
    }

    fn read_compound(mut input: &[u8], depth: usize) -> Result<(Self, &[u8]), ProtocolError> {
        if depth > MAX_TEXT_COMPONENT_DEPTH {
            return Err(ProtocolError::Oversized(format!(
                "Text component nesting exceeds {MAX_TEXT_COMPONENT_DEPTH}"
            )));
        }
        let mut component = Self::default();
        loop {
            let (tag, rest) = input.split_first().ok_or(ProtocolError::UnexpectedEof)?;
            if *tag == TAG_END {
                return Ok((component, rest));
            }
            let (name, rest) = read_nbt_string(rest)?;
            input = match (*tag, name.as_str()) {
                (TAG_STRING, "text") => {
                    let (text, rest) = read_nbt_string(rest)?;
                    component.text = text;
                    rest
                }
                (TAG_STRING, "color") => {
                    let (color, rest) = read_nbt_string(rest)?;
                    component.color = Some(ChatColor::parse(&color).ok_or_else(|| {
                        ProtocolError::Malformed(format!("Unknown text color '{color}'"))
                    })?);
                    rest
                }
                (TAG_BYTE, "bold" | "italic" | "underlined" | "strikethrough" | "obfuscated") => {
                    let (value, rest) = rest.split_first().ok_or(ProtocolError::UnexpectedEof)?;
                    let value = Some(*value != 0);
                    match name.as_str() {
                        "bold" => component.bold = value,
                        "italic" => component.italic = value,
                        "underlined" => component.underlined = value,
                        "strikethrough" => component.strikethrough = value,
                        _ => component.obfuscated = value,
                    }
                    rest
                }
                (TAG_LIST, "extra") => {
                    let (children, rest) = Self::read_extra(rest, depth)?;
                    component.extra = children;
                    rest
                }
                _ => {
                    return Err(ProtocolError::Malformed(format!(
                        "Unsupported text component field '{name}' with tag {tag}"
                    )));
                }
            };
        }
    }

    fn read_extra(input: &[u8], depth: usize) -> Result<(Vec<Self>, &[u8]), ProtocolError> {
        let (element, rest) = input.split_first().ok_or(ProtocolError::UnexpectedEof)?;
        let (count, mut rest) = rest
            .split_first_chunk::<4>()
            .ok_or(ProtocolError::UnexpectedEof)?;
        let count = i32::from_be_bytes(*count);
        if count < 0 {
            return Err(ProtocolError::Malformed("Negative NBT list length".into()));
        }
        // Every element takes at least one byte, so a false count fails early
        // without reserving memory for it.
        let mut children = Vec::new();
        for _ in 0..count {
            let (child, after) = match *element {
                TAG_STRING => {
                    let (text, after) = read_nbt_string(rest)?;
                    (Self::text(text), after)
                }
                TAG_COMPOUND => Self::read_compound(rest, depth + 1)?,
                other => {
                    return Err(ProtocolError::Malformed(format!(
                        "Unsupported extra element tag {other}"
                    )));
                }
            };
            children.push(child);
            rest = after;
        }
        Ok((children, rest))
    }
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
pub fn broadcast_chat(
    sender: &str,
    message: &str,
) -> Result<SystemChatMessagePacket, ProtocolError> {
    SystemChatMessagePacket::chat(ChatComponent::player_message(sender, message))
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

    /// Create the dispatcher for a live preview session.
    ///
    /// The preview world, clock, game mode and inventory are read-only, and this
    /// dispatcher's state is not connected to them. Mutating commands are
    /// therefore rejected instead of reporting an effect the client never sees.
    pub fn read_only_preview() -> Self {
        let mut dispatcher = Self {
            world_time: 0,
            players: HashMap::new(),
            commands: HashMap::new(),
        };
        dispatcher.register(
            "help",
            "Lists available commands or shows help for a specific command",
            "/help [command]",
            handle_help,
        );
        for name in ["tp", "teleport", "gamemode", "time", "give"] {
            dispatcher.register(
                name,
                "Unavailable in the read-only preview",
                format!("/{name}"),
                handle_read_only,
            );
        }
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
    ) -> Result<SystemChatMessagePacket, ProtocolError> {
        match self.dispatch(sender, command_line) {
            Ok(msg) => SystemChatMessagePacket::chat_text(msg),
            Err(err) => SystemChatMessagePacket::chat(ChatComponent::colored(
                err.to_string(),
                ChatColor::Red,
            )),
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

fn handle_read_only(
    _dispatcher: &mut CommandDispatcher,
    _sender: &CommandSender,
    _args: &[&str],
) -> Result<String, CommandError> {
    Err(CommandError::ExecutionError(
        "This preview session is read-only, so the command was not applied".into(),
    ))
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

    // Wire fixtures below are written by hand from the protocol tables, not
    // produced by this module's encoders.

    const TIMESTAMP: [u8; 8] = [0x00, 0x00, 0x01, 0x8B, 0xCF, 0xE5, 0x68, 0x00];
    const SALT: [u8; 8] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];

    fn unsigned_chat_fixture() -> Vec<u8> {
        let mut bytes = vec![0x09, 0x02, b'h', b'i'];
        bytes.extend(TIMESTAMP);
        bytes.extend(SALT);
        bytes.push(0x00); // No signature.
        bytes.extend([0x00, 0x00, 0x00, 0x00, 0x00]); // Count, 3-byte bitset, checksum.
        bytes
    }

    #[test]
    fn packet_ids_match_the_26_3_report() {
        assert_eq!(CHAT_ACK_PACKET_ID, 6);
        assert_eq!(CHAT_COMMAND_PACKET_ID, 7);
        assert_eq!(CHAT_COMMAND_SIGNED_PACKET_ID, 8);
        assert_eq!(CHAT_PACKET_ID, 9);
        assert_eq!(CHAT_SESSION_UPDATE_PACKET_ID, 10);
        assert_eq!(SYSTEM_CHAT_MESSAGE_PACKET_ID, 124);
    }

    #[test]
    fn unsigned_chat_parses_the_mandatory_acknowledgment_tail() {
        let packet = ChatPacket::decode_packet(&unsigned_chat_fixture()).unwrap();
        assert_eq!(packet.message, "hi");
        assert_eq!(packet.timestamp, 0x0000_018B_CFE5_6800);
        assert_eq!(packet.salt, 0x1122_3344_5566_7788);
        assert_eq!(packet.signature, None);
        assert_eq!(packet.last_seen, LastSeenUpdate::default());
        assert_eq!(packet.encode_packet(), unsigned_chat_fixture());
    }

    #[test]
    fn signed_chat_keeps_signature_and_tail_separate() {
        let mut bytes = vec![0x09, 0x01, b'x'];
        bytes.extend(TIMESTAMP);
        bytes.extend(SALT);
        bytes.push(0x01);
        bytes.extend([0xAB; 256]);
        bytes.extend([0x03, 0x05, 0x00, 0x80, 0x7F]);

        let packet = ChatPacket::decode_packet(&bytes).unwrap();
        assert_eq!(packet.signature, Some([0xAB; 256]));
        assert_eq!(
            packet.last_seen,
            LastSeenUpdate {
                message_count: 3,
                acknowledged: [0x05, 0x00, 0x80],
                checksum: 0x7F,
            }
        );
        assert_eq!(packet.encode_packet(), bytes);
    }

    #[test]
    fn chat_rejects_missing_tail_trailing_bytes_and_invalid_flag() {
        let fixture = unsigned_chat_fixture();
        assert_eq!(
            ChatPacket::decode_packet(&fixture[..fixture.len() - 1]),
            Err(ProtocolError::UnexpectedEof)
        );

        let mut trailing = fixture.clone();
        trailing.push(0);
        assert!(matches!(
            ChatPacket::decode_packet(&trailing),
            Err(ProtocolError::Malformed(_))
        ));

        let mut bad_flag = fixture;
        bad_flag[20] = 0x02;
        assert!(matches!(
            ChatPacket::decode_packet(&bad_flag),
            Err(ProtocolError::Malformed(_))
        ));

        // A signature shorter than the fixed 256 bytes cannot be absorbed.
        let mut short = vec![0x09, 0x01, b'x'];
        short.extend(TIMESTAMP);
        short.extend(SALT);
        short.push(0x01);
        short.extend([0xAB; 200]);
        assert_eq!(
            ChatPacket::decode_packet(&short),
            Err(ProtocolError::UnexpectedEof)
        );
    }

    #[test]
    fn chat_length_counts_utf16_code_units() {
        assert!(ChatPacket::unsigned("A".repeat(256), 0, 0).is_ok());
        assert_eq!(
            ChatPacket::unsigned("A".repeat(257), 0, 0),
            Err(ProtocolError::StringTooLong {
                max: 256,
                actual: 257
            })
        );
        // 129 supplementary characters are 258 code units.
        assert_eq!(
            ChatPacket::unsigned("\u{1F600}".repeat(129), 0, 0),
            Err(ProtocolError::StringTooLong {
                max: 256,
                actual: 258
            })
        );
    }

    #[test]
    fn unsigned_command_is_only_a_string() {
        let bytes = [
            0x07, 0x0C, b't', b'i', b'm', b'e', b' ', b's', b'e', b't', b' ', b'd', b'a', b'y',
        ];
        let packet = ChatCommandPacket::decode_packet(&bytes).unwrap();
        assert_eq!(packet.command, "time set day");
        assert_eq!(packet.encode_packet(), bytes);
        assert_eq!(
            ChatCommandPacket::new("/time set day").unwrap().command,
            "time set day"
        );

        let mut trailing = bytes.to_vec();
        trailing.push(0);
        assert!(matches!(
            ChatCommandPacket::decode_packet(&trailing),
            Err(ProtocolError::Malformed(_))
        ));
        assert!(ChatCommandPacket::new("a".repeat(32767)).is_ok());
        assert!(ChatCommandPacket::new("a".repeat(32768)).is_err());
    }

    #[test]
    fn signed_command_uses_fixed_signatures_and_acknowledgment_tail() {
        let mut bytes = vec![0x08, 0x03, b's', b'a', b'y'];
        bytes.extend(TIMESTAMP);
        bytes.extend(SALT);
        bytes.push(0x01); // One argument.
        bytes.extend([0x07, b'm', b'e', b's', b's', b'a', b'g', b'e']);
        bytes.extend([0xCD; 256]);
        bytes.extend([0x00, 0x00, 0x00, 0x00, 0x00]);

        let packet = SignedChatCommandPacket::decode_packet(&bytes).unwrap();
        assert_eq!(packet.command, "say");
        assert_eq!(
            packet.argument_signatures,
            vec![ArgumentSignature {
                name: "message".into(),
                signature: [0xCD; 256],
            }]
        );
        assert_eq!(packet.encode_packet(), bytes);

        let mut too_many = vec![0x08, 0x00];
        too_many.extend(TIMESTAMP);
        too_many.extend(SALT);
        too_many.push(0x09);
        assert!(matches!(
            SignedChatCommandPacket::decode_packet(&too_many),
            Err(ProtocolError::Oversized(_))
        ));
    }

    #[test]
    fn system_chat_plain_text_is_a_string_tag() {
        let bytes = [0x7C, 0x08, 0x00, 0x02, b'h', b'i', 0x00];
        let packet = SystemChatMessagePacket::chat_text("hi").unwrap();
        assert_eq!(packet.encode_packet(), bytes);
        assert_eq!(
            SystemChatMessagePacket::decode_packet(&bytes).unwrap(),
            packet
        );
        assert_eq!(
            packet.to_frame(),
            [0x07, 0x7C, 0x08, 0x00, 0x02, b'h', b'i', 0x00]
        );
        assert!(
            SystemChatMessagePacket::overlay_text("hi")
                .unwrap()
                .is_overlay()
        );
    }

    #[test]
    fn system_chat_styled_text_is_an_anonymous_compound() {
        #[rustfmt::skip]
        let bytes = [
            0x7C,
            0x0A, // Root compound, no name.
            0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x01, b'a',
            0x08, 0x00, 0x05, b'c', b'o', b'l', b'o', b'r', 0x00, 0x03, b'r', b'e', b'd',
            0x01, 0x00, 0x04, b'b', b'o', b'l', b'd', 0x01,
            0x09, 0x00, 0x05, b'e', b'x', b't', b'r', b'a', 0x0A, 0x00, 0x00, 0x00, 0x01,
            0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x01, b'b',
            0x00, // End of the child compound.
            0x00, // End of the root compound.
            0x01, // Overlay.
        ];
        let component = ChatComponent::colored("a", ChatColor::Red)
            .with_bold(true)
            .add_extra(ChatComponent::text("b"));
        let packet = SystemChatMessagePacket::overlay(component).unwrap();
        assert_eq!(packet.encode_packet(), bytes);
        assert_eq!(
            SystemChatMessagePacket::decode_packet(&bytes).unwrap(),
            packet
        );
    }

    #[test]
    fn nbt_strings_use_modified_utf8() {
        // U+1F600 is the surrogate pair D83D DE00, three bytes each.
        let emoji = [
            0x7C, 0x08, 0x00, 0x06, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80, 0x00,
        ];
        let packet = SystemChatMessagePacket::chat_text("\u{1F600}").unwrap();
        assert_eq!(packet.encode_packet(), emoji);
        assert_eq!(
            SystemChatMessagePacket::decode_packet(&emoji).unwrap(),
            packet
        );

        let nul = [0x7C, 0x08, 0x00, 0x02, 0xC0, 0x80, 0x00];
        let packet = SystemChatMessagePacket::chat_text("\0").unwrap();
        assert_eq!(packet.encode_packet(), nul);
        assert_eq!(
            SystemChatMessagePacket::decode_packet(&nul).unwrap(),
            packet
        );

        // Four-byte UTF-8 is not modified UTF-8.
        let standard = [0x7C, 0x08, 0x00, 0x04, 0xF0, 0x9F, 0x98, 0x80, 0x00];
        assert_eq!(
            SystemChatMessagePacket::decode_packet(&standard),
            Err(ProtocolError::InvalidUtf8)
        );
    }

    #[test]
    fn text_component_bounds_are_enforced_both_ways() {
        let mut nested = ChatComponent::text("leaf");
        for _ in 0..MAX_TEXT_COMPONENT_DEPTH {
            nested = ChatComponent::text("").add_extra(nested);
        }
        assert!(matches!(
            SystemChatMessagePacket::chat(nested),
            Err(ProtocolError::Oversized(_))
        ));
        assert!(matches!(
            SystemChatMessagePacket::chat_text("a".repeat(65536)),
            Err(ProtocolError::Oversized(_))
        ));

        // A deeply nested hostile payload fails without exhausting the stack.
        let mut hostile = vec![0x0A];
        for _ in 0..100_000 {
            hostile.extend([0x09, 0x00, 0x05, b'e', b'x', b't', b'r', b'a', 0x0A]);
            hostile.extend([0x00, 0x00, 0x00, 0x01]);
        }
        assert!(matches!(
            SystemChatMessagePacket::decode_payload(&hostile),
            Err(ProtocolError::Oversized(_))
        ));

        // Content this model cannot represent is rejected, never dropped.
        #[rustfmt::skip]
        let translated = [
            0x0A,
            0x08, 0x00, 0x09, b't', b'r', b'a', b'n', b's', b'l', b'a', b't', b'e',
            0x00, 0x01, b'k',
            0x00, 0x00,
        ];
        assert!(matches!(
            SystemChatMessagePacket::decode_payload(&translated),
            Err(ProtocolError::Malformed(_))
        ));
    }

    #[test]
    fn framed_and_unframed_entry_points_are_explicit() {
        // 120 characters make an unframed packet whose leading ID 0x7C equals
        // its remaining length, so guessing the format would misread it.
        let packet = SystemChatMessagePacket::chat_text("a".repeat(120)).unwrap();
        let unframed = packet.encode_packet();
        assert_eq!(unframed[0] as usize, unframed.len() - 1);
        assert_eq!(
            SystemChatMessagePacket::decode_packet(&unframed).unwrap(),
            packet
        );
        assert!(SystemChatMessagePacket::decode_frame(&unframed).is_err());
        assert_eq!(
            SystemChatMessagePacket::decode_frame(&packet.to_frame()).unwrap(),
            packet
        );
    }

    #[test]
    fn read_only_preview_rejects_mutating_commands() {
        let mut dispatcher = CommandDispatcher::read_only_preview();
        let sender = CommandSender::Player("Alice".into());
        for command in [
            "tp 1 2 3",
            "teleport 1 2 3",
            "gamemode survival",
            "time set day",
            "give diamond",
        ] {
            assert!(
                matches!(
                    dispatcher.dispatch(&sender, command),
                    Err(CommandError::ExecutionError(_))
                ),
                "{command}"
            );
        }
        assert!(dispatcher.players.is_empty());
        assert_eq!(dispatcher.world_time, 0);
        assert!(dispatcher.dispatch(&sender, "help").is_ok());
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
        let packet = broadcast_chat("Bob", "testing 123").unwrap();
        assert!(!packet.is_overlay());
        assert_eq!(packet.content().text, "<Bob> testing 123");
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

        let success_packet = dispatcher
            .dispatch_to_chat(&sender, "/time set 5000")
            .unwrap();
        assert!(!success_packet.is_overlay());
        assert!(success_packet.content().text.contains("5000"));

        let error_packet = dispatcher
            .dispatch_to_chat(&sender, "/unknown_cmd")
            .unwrap();
        assert!(!error_packet.is_overlay());
        assert_eq!(error_packet.content().color, Some(ChatColor::Red));
        assert!(error_packet.content().text.contains("Unknown command"));
    }

    #[test]
    fn test_packet_codec_malformed_and_errors() {
        assert_eq!(
            ChatPacket::decode_frame(&[]),
            Err(ProtocolError::UnexpectedEof)
        );
        assert_eq!(
            ChatCommandPacket::decode_frame(&[]),
            Err(ProtocolError::UnexpectedEof)
        );
        assert_eq!(
            SystemChatMessagePacket::decode_frame(&[]),
            Err(ProtocolError::UnexpectedEof)
        );

        // Length 3, packet ID 0x7F, two payload bytes.
        let wrong_id_packet = [0x03, 0x7F, 0x00, 0x00];
        assert_eq!(
            ChatPacket::decode_frame(&wrong_id_packet),
            Err(ProtocolError::WrongPacketId {
                expected: CHAT_PACKET_ID,
                actual: 0x7F
            })
        );
        assert_eq!(
            ChatCommandPacket::decode_frame(&wrong_id_packet),
            Err(ProtocolError::WrongPacketId {
                expected: CHAT_COMMAND_PACKET_ID,
                actual: 0x7F
            })
        );
        assert_eq!(
            SystemChatMessagePacket::decode_frame(&wrong_id_packet),
            Err(ProtocolError::WrongPacketId {
                expected: SYSTEM_CHAT_MESSAGE_PACKET_ID,
                actual: 0x7F
            })
        );

        // Timestamp present, salt missing.
        let mut truncated = vec![0x09, 0x02, b'H', b'i'];
        truncated.extend_from_slice(&[0; 8]);
        assert_eq!(
            ChatPacket::decode_packet(&truncated),
            Err(ProtocolError::UnexpectedEof)
        );

        let bad_overlay = [0x7C, 0x08, 0x00, 0x00, 0x02];
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
