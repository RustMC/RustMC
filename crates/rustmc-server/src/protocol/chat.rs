//! Serverbound chat packets, clientbound system chat messages, and JSON/NBT text components
//! for Minecraft Java Edition.

use std::{error::Error, fmt};

use crate::java_packets::play::{clientbound, serverbound};
use crate::java_wire::{
    self, MAX_NBT_STRING_BYTES, NBT_TAG_STRING, WireError, encode_modified_utf8, put_string,
    put_varint,
};

// Packet IDs come from `crate::java_packets`. Wire layouts follow the public
// protocol reference for 26.3 / protocol 777:
// https://minecraft.wiki/w/Java_Edition_protocol/Packets (revision 3868170) and
// https://minecraft.wiki/w/Java_Edition_protocol/Data_types (revision 3855787).

/// Serverbound `chat` packet ID.
pub const CHAT_PACKET_ID: u32 = serverbound::CHAT;

/// Serverbound `chat_ack` packet ID.
pub const CHAT_ACK_PACKET_ID: u32 = serverbound::CHAT_ACK;

/// Serverbound `chat_command` packet ID (unsigned layout).
pub const CHAT_COMMAND_PACKET_ID: u32 = serverbound::CHAT_COMMAND;

/// Serverbound `chat_command_signed` packet ID.
pub const CHAT_COMMAND_SIGNED_PACKET_ID: u32 = serverbound::CHAT_COMMAND_SIGNED;

/// Serverbound `chat_session_update` packet ID.
pub const CHAT_SESSION_UPDATE_PACKET_ID: u32 = serverbound::CHAT_SESSION_UPDATE;

/// Clientbound `system_chat` packet ID.
pub const SYSTEM_CHAT_MESSAGE_PACKET_ID: u32 = clientbound::SYSTEM_CHAT;

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

// ============================================================================
// Wire Codec Adapters over `crate::java_wire`
// ============================================================================

fn map_wire_error(err: WireError) -> ProtocolError {
    match err {
        WireError::Truncated => ProtocolError::UnexpectedEof,
        WireError::VarIntTooLong => ProtocolError::Malformed("VarInt exceeds 5 bytes".into()),
        WireError::NonCanonicalVarInt => {
            ProtocolError::Malformed("Non-canonical VarInt zero".into())
        }
        WireError::TooLong { length, max } => ProtocolError::StringTooLong {
            max,
            actual: length,
        },
        WireError::InvalidText => ProtocolError::InvalidUtf8,
    }
}

fn read_varint(input: &[u8]) -> Result<(u32, usize), ProtocolError> {
    java_wire::read_varint(input)
        .map_err(map_wire_error)?
        .ok_or(ProtocolError::UnexpectedEof)
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
/// hostile byte length prefix before UTF-8 decoding.
fn read_bounded_string(input: &[u8], max: usize) -> Result<(&str, usize), ProtocolError> {
    let byte_limit = max.saturating_mul(3);
    let (value, rest) = java_wire::read_string(input, byte_limit).map_err(|err| match err {
        WireError::TooLong { length, .. } => ProtocolError::StringTooLong {
            max,
            actual: length,
        },
        other => map_wire_error(other),
    })?;
    check_utf16_len(value, max)?;
    Ok((value, input.len() - rest.len()))
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
const TAG_STRING: u8 = NBT_TAG_STRING;
const TAG_LIST: u8 = 9;
const TAG_COMPOUND: u8 = 10;

/// Write a TAG_String payload: unsigned 16-bit byte length and modified UTF-8 via `java_wire`.
fn put_nbt_string(value: &str, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
    let mut encoded = Vec::with_capacity(value.len());
    encode_modified_utf8(value, &mut encoded);
    let length = u16::try_from(encoded.len()).map_err(|_| {
        ProtocolError::Oversized(format!(
            "NBT string of {} bytes exceeds {MAX_NBT_STRING_BYTES}",
            encoded.len()
        ))
    })?;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend(encoded);
    Ok(())
}

fn read_nbt_string(input: &[u8]) -> Result<(String, &[u8]), ProtocolError> {
    let (text, consumed) = java_wire::read_nbt_string(input).map_err(map_wire_error)?;
    Ok((text, &input[consumed..]))
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
}
