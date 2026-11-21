//! Minecraft Java Edition 26.3 entity spawning protocol packets and visual tracker.
//!
//! Provides encoders, decoders, and deterministic distance-based visual tracking for:
//! - `SpawnEntityPacket` (ID 0x01 in Java 26.3 protocol 777)
//! - `SetEntityDataPacket` (ID 0x65 in Java 26.3 protocol 777)
//! - `RemoveEntitiesPacket` (ID 0x4E in Java 26.3 protocol 777)
//!
//! Packet IDs come from the 26.3 server's generated packet report. Layouts follow the
//! public protocol reference for 26.3 / protocol 777:
//! <https://minecraft.wiki/w/Java_Edition_protocol/Packets> (revision 3868170),
//! <https://minecraft.wiki/w/Java_Edition_protocol/Data_types> (revision 3855787) and
//! <https://minecraft.wiki/w/Java_Edition_protocol/Entity_metadata> (revision 3869248).
//!
//! # Protocol Specification
//!
//! - **SpawnEntity** (0x01):
//!   - `entity_id`: VarInt
//!   - `uuid`: 16 bytes (128-bit)
//!   - `entity_type`: VarInt
//!   - `position`: 3x f64 (X, Y, Z)
//!   - `velocity`: LpVec3, written only as its one-byte zero form; see below
//!   - `pitch`, `yaw`, `head_yaw`: angle bytes in steps of 1/256 of a turn
//!   - `data`: VarInt, meaning dependent on the entity type
//!
//! - **SetEntityData** (0x65):
//!   - `entity_id`: VarInt
//!   - `metadata`: array of entries terminated by byte `0xFF`
//!     - Index: `u8`
//!     - Type: VarInt, see [`MetadataValue::type_id`]
//!     - Value: type-dependent payload
//!
//! - **RemoveEntities** (0x4E):
//!   - `count`: VarInt
//!   - `entity_ids`: array of VarInt entity identifiers
//!
//! # Scope
//!
//! Nonzero LpVec3 velocity is not implemented. Its quantization parameters have no
//! primary source yet, so spawned entities start at rest and a nonzero value is
//! rejected on decode.
//!
//! # Visual Tracking
//!
//! `EntityVisualTracker` maintains the visible entity set for every observer player based on
//! Euclidean view radius. When an entity enters an observer's radius, `SpawnEntityPacket` and
//! `SetEntityDataPacket` are emitted. When an entity leaves an observer's radius,
//! `RemoveEntitiesPacket` is emitted.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::java_wire::{
    self, NBT_TAG_STRING, WireError, frame, pack_block_position, put_nbt_text, put_string,
    put_varint, unpack_block_position,
};

/// Clientbound packet ID for Java 26.3 SpawnEntity (Play state 0x01).
pub const PACKET_ID_SPAWN_ENTITY: u32 = 0x01;

/// Clientbound packet ID for Java 26.3 SetEntityData (Play state 0x65).
pub const PACKET_ID_SET_ENTITY_DATA: u32 = 0x65;

/// Clientbound packet ID for Java 26.3 RemoveEntities (Play state 0x4E).
pub const PACKET_ID_REMOVE_ENTITIES: u32 = 0x4E;

/// Sentinel byte terminating entity metadata collections in Java Edition protocol.
pub const METADATA_TERMINATOR: u8 = 0xFF;

/// Maximum UTF-16 length of a metadata String value.
pub const MAX_METADATA_STRING_CHARS: usize = 32767;

/// The one-byte LpVec3 form of a zero vector.
const ZERO_LP_VEC3: u8 = 0x00;

/// Protocol direction constants.
pub mod direction {
    pub const DOWN: i32 = 0;
    pub const UP: i32 = 1;
    pub const NORTH: i32 = 2;
    pub const SOUTH: i32 = 3;
    pub const WEST: i32 = 4;
    pub const EAST: i32 = 5;
}

/// Protocol pose constants.
pub mod pose {
    pub const STANDING: i32 = 0;
    pub const FALL_FLYING: i32 = 1;
    pub const SLEEPING: i32 = 2;
    pub const SWIMMING: i32 = 3;
    pub const SPIN_ATTACK: i32 = 4;
    pub const SNEAKING: i32 = 5;
    pub const LONG_JUMPING: i32 = 6;
    pub const DYING: i32 = 7;
    pub const CROAKING: i32 = 8;
    pub const USING_TONGUE: i32 = 9;
    pub const SITTING: i32 = 10;
    pub const ROARING: i32 = 11;
    pub const SNIFFING: i32 = 12;
    pub const EMERGING: i32 = 13;
    pub const DIGGING: i32 = 14;
    pub const SLIDING: i32 = 15;
    pub const SHOOTING: i32 = 16;
    pub const INHALING: i32 = 17;
}

/// Quantizes an angle in degrees into Minecraft's protocol angle byte (0..=255).
///
/// Quantizes with `(angle * 256.0 / 360.0).floor() as u8`, a RustMC rounding choice;
/// the protocol defines only the 1/256-turn step.
#[inline]
pub fn angle_to_byte(angle: f64) -> u8 {
    let steps = (angle * 256.0 / 360.0).floor() as i64;
    steps.rem_euclid(256) as u8
}

/// Converts a protocol angle byte (0..=255) back into degrees in `[0.0, 360.0)`.
#[inline]
pub fn byte_to_angle(byte: u8) -> f64 {
    f64::from(byte) * 360.0 / 256.0
}

/// Packs 3D block coordinates into a 64-bit integer following modern Java protocol.
///
/// Layout:
/// - X: 26 bits (bits 38..=63)
/// - Z: 26 bits (bits 12..=37)
/// - Y: 12 bits (bits 0..=11)
#[inline]
pub fn pack_position(x: i32, y: i32, z: i32) -> u64 {
    pack_block_position(x, y, z) as u64
}

/// Unpacks a 64-bit integer into 3D block coordinates (X, Y, Z).
#[inline]
pub fn unpack_position(val: u64) -> [i32; 3] {
    let (x, y, z) = unpack_block_position(val as i64);
    [x, y, z]
}

/// Errors occurring during entity protocol encoding, decoding, or tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityProtocolError {
    UnexpectedEof,
    MalformedVarInt,
    TrailingData,
    InvalidPacketLength { expected: usize, received: usize },
    UnexpectedPacketId { expected: u32, received: u32 },
    InvalidUtf8,
    UnknownTypeId(i32),
    NonFiniteCoordinate,
    InvalidRadius,
    InvalidBoolean(u8),
    InvalidItemCount(i32),
    StringTooLong { length: usize, max: usize },
    UnsupportedTextComponent(u8),
    UnsupportedVelocity,
    PlayerNotFound(i32),
    EntityNotFound(i32),
}

impl fmt::Display for EntityProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected end of packet buffer"),
            Self::MalformedVarInt => write!(f, "malformed VarInt in packet stream"),
            Self::TrailingData => write!(f, "unparsed trailing data remaining in packet"),
            Self::InvalidPacketLength { expected, received } => {
                write!(
                    f,
                    "invalid packet length: expected {expected} bytes, received {received}"
                )
            }
            Self::UnexpectedPacketId { expected, received } => {
                write!(
                    f,
                    "unexpected packet id: expected 0x{expected:02X}, received 0x{received:02X}"
                )
            }
            Self::InvalidUtf8 => write!(f, "invalid utf-8 string in packet payload"),
            Self::UnknownTypeId(type_id) => {
                write!(f, "unknown entity metadata type id: {type_id}")
            }
            Self::NonFiniteCoordinate => write!(f, "coordinates contain non-finite float"),
            Self::InvalidRadius => write!(f, "view radius must be non-negative and finite"),
            Self::InvalidBoolean(byte) => write!(f, "invalid boolean byte: 0x{byte:02X}"),
            Self::InvalidItemCount(count) => write!(f, "invalid item count: {count}"),
            Self::StringTooLong { length, max } => {
                write!(f, "string length {length} exceeds maximum {max}")
            }
            Self::UnsupportedTextComponent(tag) => {
                write!(f, "unsupported text component NBT tag: {tag}")
            }
            Self::UnsupportedVelocity => write!(f, "nonzero spawn velocity is not supported"),
            Self::PlayerNotFound(id) => write!(f, "player {id} not found in visual tracker"),
            Self::EntityNotFound(id) => write!(f, "entity {id} not found in visual tracker"),
        }
    }
}

impl std::error::Error for EntityProtocolError {}

/// Reads a VarInt from a byte slice.
pub fn read_varint(input: &[u8]) -> Result<(i32, usize), EntityProtocolError> {
    match java_wire::read_varint(input) {
        Ok(Some((value, bytes_read))) => Ok((value as i32, bytes_read)),
        Ok(None) => Err(EntityProtocolError::UnexpectedEof),
        Err(_) => Err(EntityProtocolError::MalformedVarInt),
    }
}

/// Writes a VarInt into a byte vector.
pub fn write_varint(value: i32, out: &mut Vec<u8>) {
    put_varint(value as u32, out);
}

/// Reads a VarInt-prefixed UTF-8 string from a byte slice.
pub fn read_string(input: &[u8]) -> Result<(&str, usize), EntityProtocolError> {
    match java_wire::read_string(input, i32::MAX as usize) {
        Ok((s, rest)) => Ok((s, input.len() - rest.len())),
        Err(WireError::Truncated) => Err(EntityProtocolError::UnexpectedEof),
        Err(
            WireError::VarIntTooLong
            | WireError::NonCanonicalVarInt
            | WireError::TooLong { .. }
            | WireError::BelowCompressionThreshold { .. }
            | WireError::InvalidCompression
            | WireError::DecompressedSizeMismatch { .. },
        ) => Err(EntityProtocolError::MalformedVarInt),
        Err(WireError::InvalidText) => Err(EntityProtocolError::InvalidUtf8),
    }
}

/// Writes a VarInt-prefixed UTF-8 string into a byte vector.
pub fn write_string(s: &str, out: &mut Vec<u8>) {
    put_string(s, out);
}

/// Frames a packet payload with VarInt packet ID and prepends the VarInt packet length.
pub fn frame_packet(id: u32, payload: &[u8]) -> Vec<u8> {
    frame(id, payload)
}

/// Validates that 3D coordinates are finite (neither NaN nor Infinity).
#[inline]
pub fn validate_coordinates(pos: [f64; 3]) -> Result<(), EntityProtocolError> {
    if pos[0].is_finite() && pos[1].is_finite() && pos[2].is_finite() {
        Ok(())
    } else {
        Err(EntityProtocolError::NonFiniteCoordinate)
    }
}

/// Computes squared 3D Euclidean distance between two positions.
#[inline]
fn distance_sq(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

/// Checks whether two points are within the given radius of each other.
#[inline]
fn is_in_radius(a: [f64; 3], b: [f64; 3], radius: f64) -> bool {
    distance_sq(a, b) <= radius * radius
}

// ---------------------------------------------------------------------------
// SpawnEntityPacket (0x01)
// ---------------------------------------------------------------------------

mod spawn;
pub use spawn::*;
mod metadata;
pub use metadata::*;
mod tracker;
pub use tracker::*;

#[cfg(test)]
mod tests;
