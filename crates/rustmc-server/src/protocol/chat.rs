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
        WireError::BelowCompressionThreshold { length, threshold } => ProtocolError::Malformed(
            format!("Compressed length {length} below threshold {threshold}"),
        ),
        WireError::InvalidCompression | WireError::DecompressedSizeMismatch { .. } => {
            ProtocolError::Malformed("Invalid compressed frame".into())
        }
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

mod packets;
pub use packets::*;
mod component;
pub use component::*;

#[cfg(test)]
mod tests;
