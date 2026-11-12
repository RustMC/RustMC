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

/// NBT tag ID for TAG_String, the form of a literal text component.
const TAG_STRING: u8 = 8;

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
    (((x as i64 & 0x3FFFFFF) << 38) | ((z as i64 & 0x3FFFFFF) << 12) | (y as i64 & 0xFFF)) as u64
}

/// Unpacks a 64-bit integer into 3D block coordinates (X, Y, Z).
#[inline]
pub fn unpack_position(val: u64) -> [i32; 3] {
    let val = val as i64;
    let x = (val >> 38) as i32;
    let y = ((val << 52) >> 52) as i32;
    let z = ((val << 26) >> 38) as i32;
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
    let mut value = 0u32;
    for (i, &byte) in input.iter().take(5).enumerate() {
        if i == 4 && (byte & 0xf0) != 0 {
            return Err(EntityProtocolError::MalformedVarInt);
        }
        value |= u32::from(byte & 0x7f) << (i * 7);
        if (byte & 0x80) == 0 {
            if i > 0 && byte == 0 && value == 0 {
                return Err(EntityProtocolError::MalformedVarInt);
            }
            return Ok((value as i32, i + 1));
        }
    }
    if input.len() >= 5 {
        Err(EntityProtocolError::MalformedVarInt)
    } else {
        Err(EntityProtocolError::UnexpectedEof)
    }
}

/// Writes a VarInt into a byte vector.
pub fn write_varint(value: i32, out: &mut Vec<u8>) {
    let mut v = value as u32;
    loop {
        let mut byte = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if v == 0 {
            break;
        }
    }
}

/// Reads a VarInt-prefixed UTF-8 string from a byte slice.
pub fn read_string(input: &[u8]) -> Result<(&str, usize), EntityProtocolError> {
    let (length, prefix_len) = read_varint(input)?;
    if length < 0 {
        return Err(EntityProtocolError::MalformedVarInt);
    }
    let length = length as usize;
    let total_len = prefix_len + length;
    if input.len() < total_len {
        return Err(EntityProtocolError::UnexpectedEof);
    }
    let s = std::str::from_utf8(&input[prefix_len..total_len])
        .map_err(|_| EntityProtocolError::InvalidUtf8)?;
    Ok((s, total_len))
}

/// Writes a VarInt-prefixed UTF-8 string into a byte vector.
pub fn write_string(s: &str, out: &mut Vec<u8>) {
    write_varint(s.len() as i32, out);
    out.extend_from_slice(s.as_bytes());
}

/// Frames a packet payload with VarInt packet ID and prepends the VarInt packet length.
pub fn frame_packet(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::new();
    write_varint(id as i32, &mut packet);
    packet.extend_from_slice(payload);

    let mut out = Vec::new();
    write_varint(packet.len() as i32, &mut out);
    out.extend(packet);
    out
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

/// Clientbound packet for spawning entities in Java Edition 26.3 (protocol 777).
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnEntityPacket {
    pub entity_id: i32,
    pub uuid: [u8; 16],
    pub entity_type: i32,
    pub position: [f64; 3],
    pub pitch: u8,
    pub yaw: u8,
    pub head_yaw: u8,
    pub data: i32,
}

impl SpawnEntityPacket {
    pub const PACKET_ID: u32 = PACKET_ID_SPAWN_ENTITY;

    /// Creates a new `SpawnEntityPacket` at rest, with raw angle bytes.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        entity_id: i32,
        uuid: [u8; 16],
        entity_type: i32,
        position: [f64; 3],
        pitch: u8,
        yaw: u8,
        head_yaw: u8,
        data: i32,
    ) -> Result<Self, EntityProtocolError> {
        validate_coordinates(position)?;
        Ok(Self {
            entity_id,
            uuid,
            entity_type,
            position,
            pitch,
            yaw,
            head_yaw,
            data,
        })
    }

    /// Creates a new `SpawnEntityPacket` at rest, converting angles in degrees.
    #[allow(clippy::too_many_arguments)]
    pub fn with_angles(
        entity_id: i32,
        uuid: [u8; 16],
        entity_type: i32,
        position: [f64; 3],
        pitch_deg: f64,
        yaw_deg: f64,
        head_yaw_deg: f64,
        data: i32,
    ) -> Result<Self, EntityProtocolError> {
        Self::new(
            entity_id,
            uuid,
            entity_type,
            position,
            angle_to_byte(pitch_deg),
            angle_to_byte(yaw_deg),
            angle_to_byte(head_yaw_deg),
            data,
        )
    }

    /// Encodes the packet payload (without outer packet length or packet ID frame).
    pub fn encode_payload(&self, out: &mut Vec<u8>) {
        write_varint(self.entity_id, out);
        out.extend_from_slice(&self.uuid);
        write_varint(self.entity_type, out);
        out.extend_from_slice(&self.position[0].to_be_bytes());
        out.extend_from_slice(&self.position[1].to_be_bytes());
        out.extend_from_slice(&self.position[2].to_be_bytes());
        out.push(ZERO_LP_VEC3);
        out.push(self.pitch);
        out.push(self.yaw);
        out.push(self.head_yaw);
        write_varint(self.data, out);
    }

    /// Encodes the framed packet ready for network transmission.
    pub fn encode(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        self.encode_payload(&mut payload);
        frame_packet(Self::PACKET_ID, &payload)
    }

    /// Decodes the payload bytes for `SpawnEntityPacket`.
    pub fn decode_payload(data: &[u8]) -> Result<Self, EntityProtocolError> {
        let mut cursor = 0;
        let (entity_id, n) = read_varint(&data[cursor..])?;
        cursor += n;

        if data.len() < cursor + 16 {
            return Err(EntityProtocolError::UnexpectedEof);
        }
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&data[cursor..cursor + 16]);
        cursor += 16;

        let (entity_type, n) = read_varint(&data[cursor..])?;
        cursor += n;

        if data.len() < cursor + 24 {
            return Err(EntityProtocolError::UnexpectedEof);
        }
        let x = f64::from_be_bytes(
            data[cursor..cursor + 8]
                .try_into()
                .map_err(|_| EntityProtocolError::UnexpectedEof)?,
        );
        cursor += 8;
        let y = f64::from_be_bytes(
            data[cursor..cursor + 8]
                .try_into()
                .map_err(|_| EntityProtocolError::UnexpectedEof)?,
        );
        cursor += 8;
        let z = f64::from_be_bytes(
            data[cursor..cursor + 8]
                .try_into()
                .map_err(|_| EntityProtocolError::UnexpectedEof)?,
        );
        cursor += 8;

        if data.len() < cursor + 4 {
            return Err(EntityProtocolError::UnexpectedEof);
        }
        if data[cursor] != ZERO_LP_VEC3 {
            return Err(EntityProtocolError::UnsupportedVelocity);
        }
        cursor += 1;
        let pitch = data[cursor];
        cursor += 1;
        let yaw = data[cursor];
        cursor += 1;
        let head_yaw = data[cursor];
        cursor += 1;

        let (data_val, n) = read_varint(&data[cursor..])?;
        cursor += n;

        if cursor != data.len() {
            return Err(EntityProtocolError::TrailingData);
        }

        Self::new(
            entity_id,
            uuid,
            entity_type,
            [x, y, z],
            pitch,
            yaw,
            head_yaw,
            data_val,
        )
    }

    /// Decodes a framed `SpawnEntityPacket` from network wire bytes.
    pub fn decode(packet: &[u8]) -> Result<Self, EntityProtocolError> {
        let (length, n1) = read_varint(packet)?;
        if length < 0 {
            return Err(EntityProtocolError::MalformedVarInt);
        }
        let remainder = &packet[n1..];
        if remainder.len() != length as usize {
            return Err(EntityProtocolError::InvalidPacketLength {
                expected: length as usize,
                received: remainder.len(),
            });
        }
        let (packet_id, n2) = read_varint(remainder)?;
        if packet_id as u32 != Self::PACKET_ID {
            return Err(EntityProtocolError::UnexpectedPacketId {
                expected: Self::PACKET_ID,
                received: packet_id as u32,
            });
        }
        Self::decode_payload(&remainder[n2..])
    }
}

// ---------------------------------------------------------------------------
// RemoveEntitiesPacket (0x4E)
// ---------------------------------------------------------------------------

/// Clientbound packet for despawning entities in Java Edition 26.3 (protocol 777).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveEntitiesPacket {
    pub entity_ids: Vec<i32>,
}

impl RemoveEntitiesPacket {
    pub const PACKET_ID: u32 = PACKET_ID_REMOVE_ENTITIES;

    /// Creates a packet removing multiple entities by ID.
    pub fn new(entity_ids: Vec<i32>) -> Self {
        Self { entity_ids }
    }

    /// Creates a packet removing a single entity by ID.
    pub fn single(entity_id: i32) -> Self {
        Self {
            entity_ids: vec![entity_id],
        }
    }

    /// Encodes the packet payload (VarInt count followed by each VarInt entity ID).
    pub fn encode_payload(&self, out: &mut Vec<u8>) {
        write_varint(self.entity_ids.len() as i32, out);
        for &id in &self.entity_ids {
            write_varint(id, out);
        }
    }

    /// Encodes the framed packet.
    pub fn encode(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        self.encode_payload(&mut payload);
        frame_packet(Self::PACKET_ID, &payload)
    }

    /// Decodes the payload bytes for `RemoveEntitiesPacket`.
    pub fn decode_payload(data: &[u8]) -> Result<Self, EntityProtocolError> {
        let mut cursor = 0;
        let (count, n) = read_varint(&data[cursor..])?;
        cursor += n;
        if count < 0 {
            return Err(EntityProtocolError::MalformedVarInt);
        }
        let count = count as usize;
        // Each ID takes at least one byte, so this never trusts the claimed count.
        let mut entity_ids = Vec::with_capacity(count.min(data.len() - cursor));
        for _ in 0..count {
            if cursor >= data.len() {
                return Err(EntityProtocolError::UnexpectedEof);
            }
            let (id, n) = read_varint(&data[cursor..])?;
            cursor += n;
            entity_ids.push(id);
        }
        if cursor != data.len() {
            return Err(EntityProtocolError::TrailingData);
        }
        Ok(Self { entity_ids })
    }

    /// Decodes a framed `RemoveEntitiesPacket` from network wire bytes.
    pub fn decode(packet: &[u8]) -> Result<Self, EntityProtocolError> {
        let (length, n1) = read_varint(packet)?;
        if length < 0 {
            return Err(EntityProtocolError::MalformedVarInt);
        }
        let remainder = &packet[n1..];
        if remainder.len() != length as usize {
            return Err(EntityProtocolError::InvalidPacketLength {
                expected: length as usize,
                received: remainder.len(),
            });
        }
        let (packet_id, n2) = read_varint(remainder)?;
        if packet_id as u32 != Self::PACKET_ID {
            return Err(EntityProtocolError::UnexpectedPacketId {
                expected: Self::PACKET_ID,
                received: packet_id as u32,
            });
        }
        Self::decode_payload(&remainder[n2..])
    }
}

// ---------------------------------------------------------------------------
// Entity Metadata & SetEntityDataPacket (0x65)
// ---------------------------------------------------------------------------

/// An item stack in entity metadata, limited to stacks with no data components.
///
/// The wire format is a VarInt count and, when positive, a VarInt item ID followed
/// by the added and removed component counts. RustMC does not model components, so
/// both counts are written as zero and a nonzero count is rejected on decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SlotData {
    pub item_id: i32,
    pub count: i32,
}

impl SlotData {
    /// Creates an empty slot.
    pub const fn empty() -> Self {
        Self {
            item_id: 0,
            count: 0,
        }
    }

    /// Creates a slot containing an item with no data components.
    pub const fn item(item_id: i32, count: i32) -> Self {
        Self { item_id, count }
    }
}

/// Entity metadata values whose 26.3 layout RustMC can encode and decode exactly.
///
/// Other types, including particles whose payload depends on the particle type,
/// are rejected by [`MetadataValue::decode`] rather than misread. Text components
/// are limited to literal text, which the protocol sends as a TAG_String.
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataValue {
    Byte(i8),
    VarInt(i32),
    Float(f32),
    String(String),
    Chat(String),
    OptChat(Option<String>),
    Slot(SlotData),
    Bool(bool),
    Rotations([f32; 3]),
    OptPosition(Option<[i32; 3]>),
    Direction(i32),
    OptUUID(Option<[u8; 16]>),
    BlockState(i32),
    VillagerData {
        villager_type: i32,
        profession: i32,
        level: i32,
    },
    OptVarInt(Option<i32>),
    Pose(i32),
}

/// Appends `value` as an anonymous TAG_String with modified UTF-8 content: UTF-16
/// code units, U+0000 as `C0 80`, and each surrogate as three bytes.
fn write_text_component(value: &str, out: &mut Vec<u8>) -> Result<(), EntityProtocolError> {
    let mut encoded = Vec::with_capacity(value.len());
    for unit in value.encode_utf16() {
        match unit {
            0x0001..=0x007F => encoded.push(unit as u8),
            0x0000 | 0x0080..=0x07FF => {
                encoded.push(0xC0 | (unit >> 6) as u8);
                encoded.push(0x80 | (unit & 0x3F) as u8);
            }
            _ => {
                encoded.push(0xE0 | (unit >> 12) as u8);
                encoded.push(0x80 | ((unit >> 6) & 0x3F) as u8);
                encoded.push(0x80 | (unit & 0x3F) as u8);
            }
        }
    }
    let length = u16::try_from(encoded.len()).map_err(|_| EntityProtocolError::StringTooLong {
        length: encoded.len(),
        max: usize::from(u16::MAX),
    })?;
    out.push(TAG_STRING);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend(encoded);
    Ok(())
}

/// Reads an anonymous TAG_String text component, returning the bytes consumed.
fn read_text_component(data: &[u8]) -> Result<(String, usize), EntityProtocolError> {
    let (tag, rest) = data
        .split_first()
        .ok_or(EntityProtocolError::UnexpectedEof)?;
    if *tag != TAG_STRING {
        return Err(EntityProtocolError::UnsupportedTextComponent(*tag));
    }
    let (length, rest) = rest
        .split_first_chunk::<2>()
        .ok_or(EntityProtocolError::UnexpectedEof)?;
    let length = usize::from(u16::from_be_bytes(*length));
    let bytes = rest
        .get(..length)
        .ok_or(EntityProtocolError::UnexpectedEof)?;

    let mut units = Vec::with_capacity(length);
    let mut index = 0;
    while index < length {
        let first = bytes[index];
        let next = |offset: usize| -> Result<u16, EntityProtocolError> {
            match bytes.get(index + offset) {
                Some(byte) if byte & 0xC0 == 0x80 => Ok(u16::from(byte & 0x3F)),
                _ => Err(EntityProtocolError::InvalidUtf8),
            }
        };
        let (unit, width) = match first {
            0x01..=0x7F => (u16::from(first), 1),
            0xC0..=0xDF => (u16::from(first & 0x1F) << 6 | next(1)?, 2),
            0xE0..=0xEF => (u16::from(first & 0x0F) << 12 | next(1)? << 6 | next(2)?, 3),
            _ => return Err(EntityProtocolError::InvalidUtf8),
        };
        units.push(unit);
        index += width;
    }
    let text = String::from_utf16(&units).map_err(|_| EntityProtocolError::InvalidUtf8)?;
    Ok((text, 3 + length))
}

fn read_bool(data: &[u8]) -> Result<bool, EntityProtocolError> {
    match data.first() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        Some(other) => Err(EntityProtocolError::InvalidBoolean(*other)),
        None => Err(EntityProtocolError::UnexpectedEof),
    }
}

fn read_f32(data: &[u8]) -> Result<f32, EntityProtocolError> {
    let (bytes, _) = data
        .split_first_chunk::<4>()
        .ok_or(EntityProtocolError::UnexpectedEof)?;
    Ok(f32::from_be_bytes(*bytes))
}

impl MetadataValue {
    /// Returns the 26.3 metadata type ID for this value.
    pub const fn type_id(&self) -> i32 {
        match self {
            Self::Byte(_) => 0,
            Self::VarInt(_) => 1,
            Self::Float(_) => 3,
            Self::String(_) => 4,
            Self::Chat(_) => 5,
            Self::OptChat(_) => 6,
            Self::Slot(_) => 7,
            Self::Bool(_) => 8,
            Self::Rotations(_) => 9,
            Self::OptPosition(_) => 11,
            Self::Direction(_) => 12,
            Self::OptUUID(_) => 13,
            Self::BlockState(_) => 14,
            Self::VillagerData { .. } => 18,
            Self::OptVarInt(_) => 19,
            Self::Pose(_) => 20,
        }
    }

    /// Encodes the value bytes into the destination buffer.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), EntityProtocolError> {
        match self {
            Self::Byte(v) => out.push(*v as u8),
            Self::VarInt(v) | Self::Direction(v) | Self::BlockState(v) | Self::Pose(v) => {
                write_varint(*v, out);
            }
            Self::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
            Self::String(s) => {
                let length = s.encode_utf16().count();
                if length > MAX_METADATA_STRING_CHARS {
                    return Err(EntityProtocolError::StringTooLong {
                        length,
                        max: MAX_METADATA_STRING_CHARS,
                    });
                }
                write_string(s, out);
            }
            Self::Chat(s) => write_text_component(s, out)?,
            Self::OptChat(None) => out.push(0x00),
            Self::OptChat(Some(s)) => {
                out.push(0x01);
                write_text_component(s, out)?;
            }
            Self::Slot(slot) => {
                if slot.count < 0 {
                    return Err(EntityProtocolError::InvalidItemCount(slot.count));
                }
                write_varint(slot.count, out);
                if slot.count > 0 {
                    write_varint(slot.item_id, out);
                    out.extend([0x00, 0x00]); // No components added or removed.
                }
            }
            Self::Bool(b) => out.push(u8::from(*b)),
            Self::Rotations(r) => {
                for value in r {
                    out.extend_from_slice(&value.to_be_bytes());
                }
            }
            Self::OptPosition(None) | Self::OptUUID(None) => out.push(0x00),
            Self::OptPosition(Some([x, y, z])) => {
                out.push(0x01);
                out.extend_from_slice(&pack_position(*x, *y, *z).to_be_bytes());
            }
            Self::OptUUID(Some(uuid)) => {
                out.push(0x01);
                out.extend_from_slice(uuid);
            }
            Self::VillagerData {
                villager_type,
                profession,
                level,
            } => {
                write_varint(*villager_type, out);
                write_varint(*profession, out);
                write_varint(*level, out);
            }
            Self::OptVarInt(None) => write_varint(0, out),
            Self::OptVarInt(Some(v)) => {
                let raw = v
                    .checked_add(1)
                    .ok_or(EntityProtocolError::MalformedVarInt)?;
                write_varint(raw, out);
            }
        }
        Ok(())
    }

    /// Decodes a metadata value given its type ID, returning the bytes consumed.
    pub fn decode(type_id: i32, data: &[u8]) -> Result<(Self, usize), EntityProtocolError> {
        Ok(match type_id {
            0 => {
                let byte = *data.first().ok_or(EntityProtocolError::UnexpectedEof)?;
                (Self::Byte(byte as i8), 1)
            }
            1 => {
                let (v, n) = read_varint(data)?;
                (Self::VarInt(v), n)
            }
            3 => (Self::Float(read_f32(data)?), 4),
            4 => {
                let (s, n) = read_string(data)?;
                let length = s.encode_utf16().count();
                if length > MAX_METADATA_STRING_CHARS {
                    return Err(EntityProtocolError::StringTooLong {
                        length,
                        max: MAX_METADATA_STRING_CHARS,
                    });
                }
                (Self::String(s.to_string()), n)
            }
            5 => {
                let (s, n) = read_text_component(data)?;
                (Self::Chat(s), n)
            }
            6 => {
                if read_bool(data)? {
                    let (s, n) = read_text_component(&data[1..])?;
                    (Self::OptChat(Some(s)), 1 + n)
                } else {
                    (Self::OptChat(None), 1)
                }
            }
            7 => {
                let (count, mut cursor) = read_varint(data)?;
                if count < 0 {
                    return Err(EntityProtocolError::InvalidItemCount(count));
                }
                if count == 0 {
                    return Ok((Self::Slot(SlotData::empty()), cursor));
                }
                let (item_id, n) = read_varint(&data[cursor..])?;
                cursor += n;
                for _ in 0..2 {
                    let (components, n) = read_varint(&data[cursor..])?;
                    cursor += n;
                    if components != 0 {
                        return Err(EntityProtocolError::UnknownTypeId(type_id));
                    }
                }
                (Self::Slot(SlotData::item(item_id, count)), cursor)
            }
            8 => (Self::Bool(read_bool(data)?), 1),
            9 => {
                let mut rotation = [0.0; 3];
                for (index, value) in rotation.iter_mut().enumerate() {
                    *value = read_f32(data.get(index * 4..).unwrap_or_default())?;
                }
                (Self::Rotations(rotation), 12)
            }
            11 => {
                if read_bool(data)? {
                    let (bytes, _) = data[1..]
                        .split_first_chunk::<8>()
                        .ok_or(EntityProtocolError::UnexpectedEof)?;
                    let position = unpack_position(u64::from_be_bytes(*bytes));
                    (Self::OptPosition(Some(position)), 9)
                } else {
                    (Self::OptPosition(None), 1)
                }
            }
            12 => {
                let (v, n) = read_varint(data)?;
                (Self::Direction(v), n)
            }
            13 => {
                if read_bool(data)? {
                    let (bytes, _) = data[1..]
                        .split_first_chunk::<16>()
                        .ok_or(EntityProtocolError::UnexpectedEof)?;
                    (Self::OptUUID(Some(*bytes)), 17)
                } else {
                    (Self::OptUUID(None), 1)
                }
            }
            14 => {
                let (v, n) = read_varint(data)?;
                (Self::BlockState(v), n)
            }
            18 => {
                let (villager_type, n1) = read_varint(data)?;
                let (profession, n2) = read_varint(&data[n1..])?;
                let (level, n3) = read_varint(&data[n1 + n2..])?;
                (
                    Self::VillagerData {
                        villager_type,
                        profession,
                        level,
                    },
                    n1 + n2 + n3,
                )
            }
            19 => {
                let (raw, n) = read_varint(data)?;
                match raw {
                    0 => (Self::OptVarInt(None), n),
                    raw => {
                        let value = raw
                            .checked_sub(1)
                            .ok_or(EntityProtocolError::MalformedVarInt)?;
                        (Self::OptVarInt(Some(value)), n)
                    }
                }
            }
            20 => {
                let (v, n) = read_varint(data)?;
                (Self::Pose(v), n)
            }
            other => return Err(EntityProtocolError::UnknownTypeId(other)),
        })
    }
}

/// A single indexed metadata entry for an entity.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityMetadataEntry {
    pub index: u8,
    pub value: MetadataValue,
}

impl EntityMetadataEntry {
    pub fn new(index: u8, value: MetadataValue) -> Self {
        Self { index, value }
    }

    /// Encodes this entry into a byte vector: index (u8) + type (VarInt) + value.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), EntityProtocolError> {
        out.push(self.index);
        write_varint(self.value.type_id(), out);
        self.value.encode(out)
    }

    /// Decodes an entry given that the index `u8` has already been read.
    pub fn decode_entry(index: u8, data: &[u8]) -> Result<(Self, usize), EntityProtocolError> {
        let (type_id, n1) = read_varint(data)?;
        let (value, n2) = MetadataValue::decode(type_id, &data[n1..])?;
        Ok((Self { index, value }, n1 + n2))
    }
}

/// Clientbound packet synchronizing entity metadata entries in Java Edition 26.3 (protocol 777).
#[derive(Debug, Clone, PartialEq)]
pub struct SetEntityDataPacket {
    pub entity_id: i32,
    pub metadata: Vec<EntityMetadataEntry>,
}

impl SetEntityDataPacket {
    pub const PACKET_ID: u32 = PACKET_ID_SET_ENTITY_DATA;

    pub fn new(entity_id: i32, metadata: Vec<EntityMetadataEntry>) -> Self {
        Self {
            entity_id,
            metadata,
        }
    }

    /// Encodes the packet payload (VarInt entity ID + metadata entries + 0xFF terminator).
    pub fn encode_payload(&self, out: &mut Vec<u8>) -> Result<(), EntityProtocolError> {
        write_varint(self.entity_id, out);
        for entry in &self.metadata {
            entry.encode(out)?;
        }
        out.push(METADATA_TERMINATOR);
        Ok(())
    }

    /// Encodes the framed packet.
    pub fn encode(&self) -> Result<Vec<u8>, EntityProtocolError> {
        let mut payload = Vec::new();
        self.encode_payload(&mut payload)?;
        Ok(frame_packet(Self::PACKET_ID, &payload))
    }

    /// Decodes the payload bytes for `SetEntityDataPacket`.
    pub fn decode_payload(data: &[u8]) -> Result<Self, EntityProtocolError> {
        let mut cursor = 0;
        let (entity_id, n) = read_varint(&data[cursor..])?;
        cursor += n;

        let mut metadata = Vec::new();
        loop {
            if cursor >= data.len() {
                return Err(EntityProtocolError::UnexpectedEof);
            }
            let index = data[cursor];
            cursor += 1;
            if index == METADATA_TERMINATOR {
                break;
            }
            let (entry, read_bytes) = EntityMetadataEntry::decode_entry(index, &data[cursor..])?;
            cursor += read_bytes;
            metadata.push(entry);
        }

        if cursor != data.len() {
            return Err(EntityProtocolError::TrailingData);
        }

        Ok(Self {
            entity_id,
            metadata,
        })
    }

    /// Decodes a framed `SetEntityDataPacket` from network wire bytes.
    pub fn decode(packet: &[u8]) -> Result<Self, EntityProtocolError> {
        let (length, n1) = read_varint(packet)?;
        if length < 0 {
            return Err(EntityProtocolError::MalformedVarInt);
        }
        let remainder = &packet[n1..];
        if remainder.len() != length as usize {
            return Err(EntityProtocolError::InvalidPacketLength {
                expected: length as usize,
                received: remainder.len(),
            });
        }
        let (packet_id, n2) = read_varint(remainder)?;
        if packet_id as u32 != Self::PACKET_ID {
            return Err(EntityProtocolError::UnexpectedPacketId {
                expected: Self::PACKET_ID,
                received: packet_id as u32,
            });
        }
        Self::decode_payload(&remainder[n2..])
    }
}

// ---------------------------------------------------------------------------
// EntityVisualTracker
// ---------------------------------------------------------------------------

/// Complete tracked entity state registered in the visual tracker.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackedEntityState {
    pub entity_id: i32,
    pub uuid: [u8; 16],
    pub entity_type: i32,
    pub position: [f64; 3],
    pub pitch: u8,
    pub yaw: u8,
    pub head_yaw: u8,
    pub data: i32,
    pub metadata: Vec<EntityMetadataEntry>,
}

impl TrackedEntityState {
    /// Creates a new tracked entity state.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        entity_id: i32,
        uuid: [u8; 16],
        entity_type: i32,
        position: [f64; 3],
        pitch: u8,
        yaw: u8,
        head_yaw: u8,
        data: i32,
    ) -> Result<Self, EntityProtocolError> {
        validate_coordinates(position)?;
        Ok(Self {
            entity_id,
            uuid,
            entity_type,
            position,
            pitch,
            yaw,
            head_yaw,
            data,
            metadata: Vec::new(),
        })
    }

    /// Creates a tracked entity state with initial metadata entries.
    #[allow(clippy::too_many_arguments)]
    pub fn with_metadata(
        entity_id: i32,
        uuid: [u8; 16],
        entity_type: i32,
        position: [f64; 3],
        pitch: u8,
        yaw: u8,
        head_yaw: u8,
        data: i32,
        metadata: Vec<EntityMetadataEntry>,
    ) -> Result<Self, EntityProtocolError> {
        validate_coordinates(position)?;
        Ok(Self {
            entity_id,
            uuid,
            entity_type,
            position,
            pitch,
            yaw,
            head_yaw,
            data,
            metadata,
        })
    }

    /// Constructs the corresponding `SpawnEntityPacket`.
    pub fn spawn_packet(&self) -> SpawnEntityPacket {
        SpawnEntityPacket {
            entity_id: self.entity_id,
            uuid: self.uuid,
            entity_type: self.entity_type,
            position: self.position,
            pitch: self.pitch,
            yaw: self.yaw,
            head_yaw: self.head_yaw,
            data: self.data,
        }
    }

    /// Constructs the corresponding `SetEntityDataPacket`.
    pub fn metadata_packet(&self) -> SetEntityDataPacket {
        SetEntityDataPacket {
            entity_id: self.entity_id,
            metadata: self.metadata.clone(),
        }
    }
}

/// Visual tracking events emitted to observers when visibility changes.
#[derive(Debug, Clone, PartialEq)]
pub enum EntityTrackerEvent {
    /// Entity entered view radius: emit spawn packet followed by entity data packet.
    Spawn {
        spawn_packet: SpawnEntityPacket,
        data_packet: SetEntityDataPacket,
    },
    /// Entity left view radius: emit despawn packet.
    Despawn(RemoveEntitiesPacket),
}

impl EntityTrackerEvent {
    pub fn is_spawn(&self) -> bool {
        matches!(self, Self::Spawn { .. })
    }

    pub fn is_despawn(&self) -> bool {
        matches!(self, Self::Despawn(_))
    }
}

/// State of an observer player tracked in the visual tracker.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackedPlayer {
    pub player_id: i32,
    pub position: [f64; 3],
    pub view_radius: f64,
    pub visible_entities: BTreeSet<i32>,
}

/// Distance-based entity visual tracker for Minecraft Java Edition.
///
/// Maintains visible entity sets per player based on view radius.
/// Emits `SpawnEntityPacket` and `SetEntityDataPacket` on entry into view radius,
/// and `RemoveEntitiesPacket` on exit.
#[derive(Debug, Clone, Default)]
pub struct EntityVisualTracker {
    players: BTreeMap<i32, TrackedPlayer>,
    entities: BTreeMap<i32, TrackedEntityState>,
}

impl EntityVisualTracker {
    /// Creates a new empty `EntityVisualTracker`.
    pub fn new() -> Self {
        Self {
            players: BTreeMap::new(),
            entities: BTreeMap::new(),
        }
    }

    /// Registers a new player with an initial position and view radius.
    ///
    /// Evaluates existing entities and emits `SpawnEntityPacket` and `SetEntityDataPacket`
    /// for all entities within the player's view radius.
    pub fn add_player(
        &mut self,
        player_id: i32,
        position: [f64; 3],
        view_radius: f64,
    ) -> Result<Vec<EntityTrackerEvent>, EntityProtocolError> {
        validate_coordinates(position)?;
        if !view_radius.is_finite() || view_radius < 0.0 {
            return Err(EntityProtocolError::InvalidRadius);
        }

        let mut visible = BTreeSet::new();
        let mut events = Vec::new();

        for entity in self.entities.values() {
            if is_in_radius(position, entity.position, view_radius) {
                visible.insert(entity.entity_id);
                events.push(EntityTrackerEvent::Spawn {
                    spawn_packet: entity.spawn_packet(),
                    data_packet: entity.metadata_packet(),
                });
            }
        }

        self.players.insert(
            player_id,
            TrackedPlayer {
                player_id,
                position,
                view_radius,
                visible_entities: visible,
            },
        );

        Ok(events)
    }

    /// Removes a player from the tracker, returning despawn events for any tracked entities.
    pub fn remove_player(&mut self, player_id: i32) -> Option<RemoveEntitiesPacket> {
        let player = self.players.remove(&player_id)?;
        if player.visible_entities.is_empty() {
            None
        } else {
            Some(RemoveEntitiesPacket::new(
                player.visible_entities.into_iter().collect(),
            ))
        }
    }

    /// Updates an observer player's position and recalculates visibility.
    ///
    /// Emits `Spawn` events for newly visible entities and `Despawn` events for
    /// entities that left the player's radius.
    pub fn update_player_position(
        &mut self,
        player_id: i32,
        new_position: [f64; 3],
    ) -> Result<Vec<EntityTrackerEvent>, EntityProtocolError> {
        validate_coordinates(new_position)?;

        let player = self
            .players
            .get_mut(&player_id)
            .ok_or(EntityProtocolError::PlayerNotFound(player_id))?;
        player.position = new_position;
        let radius = player.view_radius;

        let mut events = Vec::new();
        let mut newly_visible = Vec::new();
        let mut newly_hidden = Vec::new();

        for (&entity_id, entity) in &self.entities {
            let was_visible = player.visible_entities.contains(&entity_id);
            let in_range = is_in_radius(new_position, entity.position, radius);

            if !was_visible && in_range {
                newly_visible.push(entity.clone());
            } else if was_visible && !in_range {
                newly_hidden.push(entity_id);
            }
        }

        for entity in newly_visible {
            player.visible_entities.insert(entity.entity_id);
            events.push(EntityTrackerEvent::Spawn {
                spawn_packet: entity.spawn_packet(),
                data_packet: entity.metadata_packet(),
            });
        }

        if !newly_hidden.is_empty() {
            for &id in &newly_hidden {
                player.visible_entities.remove(&id);
            }
            events.push(EntityTrackerEvent::Despawn(RemoveEntitiesPacket::new(
                newly_hidden,
            )));
        }

        Ok(events)
    }

    /// Updates an observer player's view radius and recalculates visibility.
    pub fn update_player_radius(
        &mut self,
        player_id: i32,
        new_radius: f64,
    ) -> Result<Vec<EntityTrackerEvent>, EntityProtocolError> {
        if !new_radius.is_finite() || new_radius < 0.0 {
            return Err(EntityProtocolError::InvalidRadius);
        }

        let player = self
            .players
            .get_mut(&player_id)
            .ok_or(EntityProtocolError::PlayerNotFound(player_id))?;
        player.view_radius = new_radius;
        let pos = player.position;

        let mut events = Vec::new();
        let mut newly_visible = Vec::new();
        let mut newly_hidden = Vec::new();

        for (&entity_id, entity) in &self.entities {
            let was_visible = player.visible_entities.contains(&entity_id);
            let in_range = is_in_radius(pos, entity.position, new_radius);

            if !was_visible && in_range {
                newly_visible.push(entity.clone());
            } else if was_visible && !in_range {
                newly_hidden.push(entity_id);
            }
        }

        for entity in newly_visible {
            player.visible_entities.insert(entity.entity_id);
            events.push(EntityTrackerEvent::Spawn {
                spawn_packet: entity.spawn_packet(),
                data_packet: entity.metadata_packet(),
            });
        }

        if !newly_hidden.is_empty() {
            for &id in &newly_hidden {
                player.visible_entities.remove(&id);
            }
            events.push(EntityTrackerEvent::Despawn(RemoveEntitiesPacket::new(
                newly_hidden,
            )));
        }

        Ok(events)
    }

    /// Registers a new entity in the world.
    ///
    /// Evaluates all players and emits `Spawn` events for observers within view radius.
    pub fn add_entity(
        &mut self,
        entity: TrackedEntityState,
    ) -> Result<Vec<(i32, EntityTrackerEvent)>, EntityProtocolError> {
        validate_coordinates(entity.position)?;
        let mut events = Vec::new();

        for player in self.players.values_mut() {
            if is_in_radius(player.position, entity.position, player.view_radius) {
                player.visible_entities.insert(entity.entity_id);
                events.push((
                    player.player_id,
                    EntityTrackerEvent::Spawn {
                        spawn_packet: entity.spawn_packet(),
                        data_packet: entity.metadata_packet(),
                    },
                ));
            }
        }

        self.entities.insert(entity.entity_id, entity);
        Ok(events)
    }

    /// Updates an entity's 3D position and updates observer visibility sets.
    pub fn update_entity_position(
        &mut self,
        entity_id: i32,
        new_position: [f64; 3],
    ) -> Result<Vec<(i32, EntityTrackerEvent)>, EntityProtocolError> {
        validate_coordinates(new_position)?;

        let entity = self
            .entities
            .get_mut(&entity_id)
            .ok_or(EntityProtocolError::EntityNotFound(entity_id))?;
        entity.position = new_position;
        let entity_clone = entity.clone();

        let mut events = Vec::new();

        for player in self.players.values_mut() {
            let was_visible = player.visible_entities.contains(&entity_id);
            let in_range = is_in_radius(player.position, new_position, player.view_radius);

            if !was_visible && in_range {
                player.visible_entities.insert(entity_id);
                events.push((
                    player.player_id,
                    EntityTrackerEvent::Spawn {
                        spawn_packet: entity_clone.spawn_packet(),
                        data_packet: entity_clone.metadata_packet(),
                    },
                ));
            } else if was_visible && !in_range {
                player.visible_entities.remove(&entity_id);
                events.push((
                    player.player_id,
                    EntityTrackerEvent::Despawn(RemoveEntitiesPacket::single(entity_id)),
                ));
            }
        }

        Ok(events)
    }

    /// Updates entity metadata and notifies all observers currently tracking this entity.
    pub fn update_entity_metadata(
        &mut self,
        entity_id: i32,
        metadata: Vec<EntityMetadataEntry>,
    ) -> Result<Vec<(i32, SetEntityDataPacket)>, EntityProtocolError> {
        let entity = self
            .entities
            .get_mut(&entity_id)
            .ok_or(EntityProtocolError::EntityNotFound(entity_id))?;
        entity.metadata = metadata.clone();

        let mut packets = Vec::new();
        for player in self.players.values() {
            if player.visible_entities.contains(&entity_id) {
                packets.push((
                    player.player_id,
                    SetEntityDataPacket::new(entity_id, metadata.clone()),
                ));
            }
        }

        Ok(packets)
    }

    /// Removes an entity from the tracker and despawns it for all observers currently viewing it.
    pub fn remove_entity(
        &mut self,
        entity_id: i32,
    ) -> Result<Vec<(i32, RemoveEntitiesPacket)>, EntityProtocolError> {
        self.entities
            .remove(&entity_id)
            .ok_or(EntityProtocolError::EntityNotFound(entity_id))?;

        let mut despawns = Vec::new();
        for player in self.players.values_mut() {
            if player.visible_entities.remove(&entity_id) {
                despawns.push((player.player_id, RemoveEntitiesPacket::single(entity_id)));
            }
        }

        Ok(despawns)
    }

    /// Returns the set of entity IDs visible to a specific player.
    pub fn visible_entities(&self, player_id: i32) -> Option<&BTreeSet<i32>> {
        self.players.get(&player_id).map(|p| &p.visible_entities)
    }

    /// Checks whether an entity is currently visible to a specific player.
    pub fn is_entity_visible_to_player(&self, player_id: i32, entity_id: i32) -> bool {
        self.players
            .get(&player_id)
            .map(|p| p.visible_entities.contains(&entity_id))
            .unwrap_or(false)
    }

    /// Returns a reference to a tracked entity state.
    pub fn get_entity(&self, entity_id: i32) -> Option<&TrackedEntityState> {
        self.entities.get(&entity_id)
    }

    /// Returns a reference to a tracked player state.
    pub fn get_player(&self, player_id: i32) -> Option<&TrackedPlayer> {
        self.players.get(&player_id)
    }

    /// Total number of tracked entities in the world.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Total number of registered observer players.
    pub fn player_count(&self) -> usize {
        self.players.len()
    }
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_angle_quantization_and_wrapping() {
        assert_eq!(angle_to_byte(0.0), 0);
        assert_eq!(angle_to_byte(90.0), 64);
        assert_eq!(angle_to_byte(180.0), 128);
        assert_eq!(angle_to_byte(270.0), 192);
        assert_eq!(angle_to_byte(360.0), 0);

        // Negative angles
        assert_eq!(angle_to_byte(-90.0), 192);
        assert_eq!(angle_to_byte(-180.0), 128);
        assert_eq!(angle_to_byte(-270.0), 64);
        assert_eq!(angle_to_byte(-360.0), 0);

        // Small negative fraction
        assert_eq!(angle_to_byte(-0.5), 255);

        // Round-trip test within 1 byte precision (360.0 / 256.0 = 1.40625 deg)
        for byte in 0..=255u8 {
            let deg = byte_to_angle(byte);
            assert_eq!(angle_to_byte(deg), byte);
        }
    }

    #[test]
    fn test_position_pack_unpack_roundtrip() {
        let test_cases = [
            [0, 0, 0],
            [-5, 64, -100],
            [12345, -64, 67890],
            [-30_000_000, 319, 30_000_000],
            [30_000_000, -2048, -30_000_000],
        ];

        for pos in test_cases {
            let packed = pack_position(pos[0], pos[1], pos[2]);
            let unpacked = unpack_position(packed);
            assert_eq!(pos, unpacked, "Failed round-trip for pos: {:?}", pos);
        }
    }

    #[test]
    fn packet_ids_match_the_26_3_report() {
        assert_eq!(PACKET_ID_SPAWN_ENTITY, 1);
        assert_eq!(PACKET_ID_SET_ENTITY_DATA, 101);
        assert_eq!(PACKET_ID_REMOVE_ENTITIES, 78);
    }

    // Wire fixtures below are written by hand from the protocol tables, not
    // produced by this module's encoders.

    fn spawn_fixture() -> Vec<u8> {
        let mut bytes = vec![0x2A]; // Entity ID 42.
        bytes.extend([0x12; 16]); // UUID.
        bytes.push(0x64); // Type 100.
        bytes.extend([0x40, 0x24, 0, 0, 0, 0, 0, 0]); // X 10.0.
        bytes.extend([0x40, 0x50, 0, 0, 0, 0, 0, 0]); // Y 64.0.
        bytes.extend([0xC0, 0x24, 0, 0, 0, 0, 0, 0]); // Z -10.0.
        bytes.push(0x00); // Zero LpVec3 velocity comes before the angles.
        bytes.extend([0x40, 0x80, 0xC0]); // Pitch, yaw, head yaw.
        bytes.push(0x07); // Data comes last.
        bytes
    }

    #[test]
    fn test_spawn_entity_packet_payload_and_framing() {
        let packet = SpawnEntityPacket::new(
            42,
            [0x12; 16],
            100,
            [10.0, 64.0, -10.0],
            0x40,
            0x80,
            0xC0,
            7,
        )
        .expect("valid coords");

        let fixture = spawn_fixture();
        let mut payload = Vec::new();
        packet.encode_payload(&mut payload);
        assert_eq!(payload, fixture);
        assert_eq!(SpawnEntityPacket::decode_payload(&fixture).unwrap(), packet);

        let mut framed = vec![fixture.len() as u8 + 1, 0x01];
        framed.extend(&fixture);
        assert_eq!(packet.encode(), framed);
        assert_eq!(SpawnEntityPacket::decode(&framed).unwrap(), packet);

        assert!(SpawnEntityPacket::new(1, [0; 16], 1, [f64::NAN, 0.0, 0.0], 0, 0, 0, 0).is_err());
        assert!(SpawnEntityPacket::decode_payload(&fixture[..fixture.len() - 1]).is_err());

        let mut trailing = fixture.clone();
        trailing.push(0x99);
        assert_eq!(
            SpawnEntityPacket::decode_payload(&trailing),
            Err(EntityProtocolError::TrailingData)
        );

        // A nonzero LpVec3 has no model here and must not be misread as angles.
        let mut moving = fixture;
        moving[42] = 0x01;
        assert_eq!(
            SpawnEntityPacket::decode_payload(&moving),
            Err(EntityProtocolError::UnsupportedVelocity)
        );

        assert_eq!(
            SpawnEntityPacket::decode(&[0x02, 0x7F, 0x00]),
            Err(EntityProtocolError::UnexpectedPacketId {
                expected: 0x01,
                received: 0x7F
            })
        );
    }

    #[test]
    fn test_remove_entities_packet_payload_and_framing() {
        // Three IDs: 1, 300 and -1.
        let payload = [0x03, 0x01, 0xAC, 0x02, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
        let packet = RemoveEntitiesPacket::new(vec![1, 300, -1]);
        let mut encoded = Vec::new();
        packet.encode_payload(&mut encoded);
        assert_eq!(encoded, payload);
        assert_eq!(
            RemoveEntitiesPacket::decode_payload(&payload).unwrap(),
            packet
        );

        let mut framed = vec![0x0A, 0x4E];
        framed.extend(payload);
        assert_eq!(packet.encode(), framed);
        assert_eq!(RemoveEntitiesPacket::decode(&framed).unwrap(), packet);

        assert_eq!(
            RemoveEntitiesPacket::decode_payload(&[0x02, 0x01]),
            Err(EntityProtocolError::UnexpectedEof)
        );
        // A hostile count fails on the missing data without a huge reservation.
        assert_eq!(
            RemoveEntitiesPacket::decode_payload(&[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
            Err(EntityProtocolError::UnexpectedEof)
        );
    }

    #[test]
    fn test_set_entity_data_uses_26_3_metadata_type_ids() {
        let cases: Vec<(MetadataValue, Vec<u8>)> = vec![
            (MetadataValue::Byte(-12), vec![0x00, 0xF4]),
            (MetadataValue::VarInt(300), vec![0x01, 0xAC, 0x02]),
            (
                MetadataValue::Float(1.0),
                vec![0x03, 0x3F, 0x80, 0x00, 0x00],
            ),
            (
                MetadataValue::String("ab".into()),
                vec![0x04, 0x02, b'a', b'b'],
            ),
            (
                MetadataValue::Chat("ab".into()),
                vec![0x05, 0x08, 0x00, 0x02, b'a', b'b'],
            ),
            (MetadataValue::OptChat(None), vec![0x06, 0x00]),
            (
                MetadataValue::OptChat(Some("\u{1F600}".into())),
                vec![
                    0x06, 0x01, 0x08, 0x00, 0x06, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80,
                ],
            ),
            (MetadataValue::Slot(SlotData::empty()), vec![0x07, 0x00]),
            (
                MetadataValue::Slot(SlotData::item(276, 3)),
                vec![0x07, 0x03, 0x94, 0x02, 0x00, 0x00],
            ),
            (MetadataValue::Bool(true), vec![0x08, 0x01]),
            (
                MetadataValue::Rotations([1.0, 0.0, -2.0]),
                vec![
                    0x09, 0x3F, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00,
                ],
            ),
            (MetadataValue::OptPosition(None), vec![0x0B, 0x00]),
            (
                // x=1 at bit 38, z=2 at bit 12, y=3 at bit 0.
                MetadataValue::OptPosition(Some([1, 3, 2])),
                vec![0x0B, 0x01, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x20, 0x03],
            ),
            (MetadataValue::Direction(direction::NORTH), vec![0x0C, 0x02]),
            (MetadataValue::OptUUID(None), vec![0x0D, 0x00]),
            (MetadataValue::BlockState(88), vec![0x0E, 0x58]),
            (
                MetadataValue::VillagerData {
                    villager_type: 2,
                    profession: 9,
                    level: 1,
                },
                vec![0x12, 0x02, 0x09, 0x01],
            ),
            (MetadataValue::OptVarInt(None), vec![0x13, 0x00]),
            (MetadataValue::OptVarInt(Some(41)), vec![0x13, 0x2A]),
            (MetadataValue::Pose(pose::SNEAKING), vec![0x14, 0x05]),
        ];

        for (value, wire) in cases {
            let mut payload = vec![0x05, 0x02]; // Entity 5, index 2.
            payload.extend(&wire);
            payload.push(0xFF);

            let packet = SetEntityDataPacket::new(5, vec![EntityMetadataEntry::new(2, value)]);
            let mut encoded = Vec::new();
            packet.encode_payload(&mut encoded).unwrap();
            assert_eq!(encoded, payload);
            assert_eq!(
                SetEntityDataPacket::decode_payload(&payload).unwrap(),
                packet
            );

            let mut framed = vec![payload.len() as u8 + 1, 0x65];
            framed.extend(&payload);
            assert_eq!(packet.encode().unwrap(), framed);
            assert_eq!(SetEntityDataPacket::decode(&framed).unwrap(), packet);
        }
    }

    #[test]
    fn test_metadata_rejects_what_it_cannot_represent() {
        assert!(SetEntityDataPacket::decode_payload(&[0x05, 0x02, 0x08, 0x01]).is_err());

        // VarLong, Particle and later types are not misread as neighbouring ones.
        for type_id in [2, 10, 15, 16, 17, 21, 43] {
            assert_eq!(
                MetadataValue::decode(type_id, &[0; 16]),
                Err(EntityProtocolError::UnknownTypeId(type_id))
            );
        }

        // A compound text component is unsupported, never truncated.
        assert_eq!(
            MetadataValue::decode(5, &[0x0A, 0x00]),
            Err(EntityProtocolError::UnsupportedTextComponent(0x0A))
        );
        // A slot with data components cannot be represented.
        assert!(MetadataValue::decode(7, &[0x01, 0x05, 0x01, 0x00]).is_err());
        assert_eq!(
            MetadataValue::decode(8, &[0x02]),
            Err(EntityProtocolError::InvalidBoolean(0x02))
        );
        assert_eq!(
            MetadataValue::decode(5, &[0x08, 0x00, 0x04, 0xF0, 0x9F, 0x98, 0x80]),
            Err(EntityProtocolError::InvalidUtf8)
        );

        let mut out = Vec::new();
        assert!(
            MetadataValue::OptVarInt(Some(i32::MAX))
                .encode(&mut out)
                .is_err()
        );
        assert!(
            MetadataValue::Slot(SlotData::item(1, -1))
                .encode(&mut out)
                .is_err()
        );
        assert!(
            MetadataValue::Chat("a".repeat(65536))
                .encode(&mut out)
                .is_err()
        );
        assert!(
            MetadataValue::String("a".repeat(32768))
                .encode(&mut out)
                .is_err()
        );
    }

    #[test]
    fn test_entity_visual_tracker_enter_and_leave_radius() {
        let mut tracker = EntityVisualTracker::new();

        let uuid1 = [1u8; 16];
        let uuid2 = [2u8; 16];

        // Register entity 1 at (10.0, 64.0, 10.0)
        let entity1 =
            TrackedEntityState::new(1, uuid1, 20, [10.0, 64.0, 10.0], 0, 0, 0, 0).unwrap();

        // Register entity 2 at (100.0, 64.0, 100.0)
        let entity2 =
            TrackedEntityState::new(2, uuid2, 21, [100.0, 64.0, 100.0], 0, 0, 0, 0).unwrap();

        tracker.add_entity(entity1).unwrap();
        tracker.add_entity(entity2).unwrap();

        // Player joins at (0.0, 64.0, 0.0) with view radius 32.0 blocks
        // Entity 1 is at dist ~14.14 <= 32 -> should spawn
        // Entity 2 is at dist ~141.42 > 32 -> should not spawn
        let events = tracker.add_player(100, [0.0, 64.0, 0.0], 32.0).unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            EntityTrackerEvent::Spawn {
                spawn_packet,
                data_packet,
            } => {
                assert_eq!(spawn_packet.entity_id, 1);
                assert_eq!(data_packet.entity_id, 1);
            }
            _ => panic!("Expected Spawn event"),
        }
        assert!(tracker.is_entity_visible_to_player(100, 1));
        assert!(!tracker.is_entity_visible_to_player(100, 2));

        // Player moves closer to entity 2: to (90.0, 64.0, 90.0)
        // Entity 1 is now at dist ~113.1 > 32 -> should despawn
        // Entity 2 is now at dist ~14.14 <= 32 -> should spawn
        let move_events = tracker
            .update_player_position(100, [90.0, 64.0, 90.0])
            .unwrap();
        assert_eq!(move_events.len(), 2);

        let mut spawned = None;
        let mut despawned = None;
        for ev in move_events {
            match ev {
                EntityTrackerEvent::Spawn { spawn_packet, .. } => {
                    spawned = Some(spawn_packet.entity_id);
                }
                EntityTrackerEvent::Despawn(packet) => {
                    despawned = Some(packet.entity_ids);
                }
            }
        }
        assert_eq!(spawned, Some(2));
        assert_eq!(despawned, Some(vec![1]));

        assert!(!tracker.is_entity_visible_to_player(100, 1));
        assert!(tracker.is_entity_visible_to_player(100, 2));

        // Entity 2 moves away to (200.0, 64.0, 200.0) -> should despawn for player 100
        let entity_move_events = tracker
            .update_entity_position(2, [200.0, 64.0, 200.0])
            .unwrap();
        assert_eq!(entity_move_events.len(), 1);
        assert_eq!(entity_move_events[0].0, 100);
        assert!(entity_move_events[0].1.is_despawn());
        assert!(!tracker.is_entity_visible_to_player(100, 2));

        // Entity 2 moves back into range -> should spawn for player 100
        let entity_move_back = tracker
            .update_entity_position(2, [95.0, 64.0, 95.0])
            .unwrap();
        assert_eq!(entity_move_back.len(), 1);
        assert_eq!(entity_move_back[0].0, 100);
        assert!(entity_move_back[0].1.is_spawn());
        assert!(tracker.is_entity_visible_to_player(100, 2));

        // Metadata update
        let new_meta = vec![EntityMetadataEntry::new(
            0,
            MetadataValue::Pose(pose::SLEEPING),
        )];
        let meta_events = tracker.update_entity_metadata(2, new_meta).unwrap();
        assert_eq!(meta_events.len(), 1);
        assert_eq!(meta_events[0].0, 100);
        assert_eq!(meta_events[0].1.entity_id, 2);

        // Remove entity from world -> despawn packet
        let despawns = tracker.remove_entity(2).unwrap();
        assert_eq!(despawns.len(), 1);
        assert_eq!(despawns[0].0, 100);
        assert_eq!(despawns[0].1.entity_ids, vec![2]);
        assert_eq!(tracker.entity_count(), 1);

        // Remove player -> returns None since visible set is now empty
        assert!(tracker.remove_player(100).is_none());
        assert_eq!(tracker.player_count(), 0);
    }

    #[test]
    fn test_tracker_multiple_players_and_radius_expansion() {
        let mut tracker = EntityVisualTracker::new();

        let uuid = [0x55; 16];
        let entity = TrackedEntityState::new(10, uuid, 1, [0.0, 0.0, 0.0], 0, 0, 0, 0).unwrap();
        tracker.add_entity(entity).unwrap();

        // Player A at (20.0, 0.0, 0.0) with radius 15.0 -> out of range
        let events_a = tracker.add_player(1, [20.0, 0.0, 0.0], 15.0).unwrap();
        assert!(events_a.is_empty());
        assert!(!tracker.is_entity_visible_to_player(1, 10));

        // Player B at (20.0, 0.0, 0.0) with radius 25.0 -> in range
        let events_b = tracker.add_player(2, [20.0, 0.0, 0.0], 25.0).unwrap();
        assert_eq!(events_b.len(), 1);
        assert!(tracker.is_entity_visible_to_player(2, 10));

        // Increase Player A view radius to 25.0 -> entity 10 enters range!
        let expand_events = tracker.update_player_radius(1, 25.0).unwrap();
        assert_eq!(expand_events.len(), 1);
        assert!(expand_events[0].is_spawn());
        assert!(tracker.is_entity_visible_to_player(1, 10));

        // Decrease Player B view radius to 10.0 -> entity 10 leaves range!
        let shrink_events = tracker.update_player_radius(2, 10.0).unwrap();
        assert_eq!(shrink_events.len(), 1);
        assert!(shrink_events[0].is_despawn());
        assert!(!tracker.is_entity_visible_to_player(2, 10));
    }
}
