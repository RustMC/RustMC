//! Minecraft Java Edition 26.3 entity spawning protocol packets and visual tracker.
//!
//! Provides encoders, decoders, and deterministic distance-based visual tracking for:
//! - `SpawnEntityPacket` (ID 0x01 in Java 26.3 protocol 777)
//! - `SetEntityDataPacket` (ID 0x58 in Java 26.3 protocol 777)
//! - `RemoveEntitiesPacket` (ID 0x42 in Java 26.3 protocol 777)
//!
//! # Protocol Specification
//!
//! - **SpawnEntity** (0x01):
//!   - `entity_id`: VarInt
//!   - `uuid`: 16 bytes (128-bit)
//!   - `entity_type`: VarInt
//!   - `position`: 3x f64 (X, Y, Z)
//!   - `pitch`, `yaw`, `head_yaw`: angle bytes, quantized as `(angle * 256.0 / 360.0).floor() as u8`
//!   - `data`: VarInt (velocity direction or item metadata)
//!   - `velocity`: 3x i16, scaled in units of 1/8000 block/tick: `(v * 8000.0).clamp(-32768.0, 32767.0) as i16`
//!
//! - **SetEntityData** (0x58):
//!   - `entity_id`: VarInt
//!   - `metadata`: array of entries terminated by byte `0xFF`
//!     - Index: `u8`
//!     - Type: VarInt
//!     - Value: Type-dependent payload (Byte, VarInt, Float, String, Chat, Slot, Bool, Rotations,
//!       OptPosition, Direction, OptUUID, BlockState, NBT, Particle, VillagerData, OptVarInt, Pose)
//!
//! - **RemoveEntities** (0x42):
//!   - `count`: VarInt
//!   - `entity_ids`: array of VarInt entity identifiers
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

/// Clientbound packet ID for Java 26.3 SetEntityData (Play state 0x58).
pub const PACKET_ID_SET_ENTITY_DATA: u32 = 0x58;

/// Clientbound packet ID for Java 26.3 RemoveEntities (Play state 0x42).
pub const PACKET_ID_REMOVE_ENTITIES: u32 = 0x42;

/// Sentinel byte terminating entity metadata collections in Java Edition protocol.
pub const METADATA_TERMINATOR: u8 = 0xFF;

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
/// Follows the official quantization formula: `(angle * 256.0 / 360.0).floor() as u8`.
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

/// Scales velocity in blocks/tick to protocol units of 1/8000 block/tick.
///
/// Follows the official scaling formula: `(v * 8000.0).clamp(-32768.0, 32767.0) as i16`.
#[inline]
pub fn velocity_to_fixed(v: f64) -> i16 {
    (v * 8000.0).clamp(-32768.0, 32767.0) as i16
}

/// Converts protocol velocity units of 1/8000 block/tick back to blocks/tick.
#[inline]
pub fn velocity_from_fixed(fixed: i16) -> f64 {
    f64::from(fixed) / 8000.0
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
    pub velocity: [i16; 3],
}

impl SpawnEntityPacket {
    pub const PACKET_ID: u32 = PACKET_ID_SPAWN_ENTITY;

    /// Creates a new `SpawnEntityPacket` with raw angle bytes and velocity units.
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
        velocity: [i16; 3],
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
            velocity,
        })
    }

    /// Creates a new `SpawnEntityPacket` converting angles (in degrees) and velocities (in blocks/tick).
    #[allow(clippy::too_many_arguments)]
    pub fn with_angles_and_velocity(
        entity_id: i32,
        uuid: [u8; 16],
        entity_type: i32,
        position: [f64; 3],
        pitch_deg: f64,
        yaw_deg: f64,
        head_yaw_deg: f64,
        data: i32,
        velocity_blocks_per_tick: [f64; 3],
    ) -> Result<Self, EntityProtocolError> {
        validate_coordinates(position)?;
        Ok(Self {
            entity_id,
            uuid,
            entity_type,
            position,
            pitch: angle_to_byte(pitch_deg),
            yaw: angle_to_byte(yaw_deg),
            head_yaw: angle_to_byte(head_yaw_deg),
            data,
            velocity: [
                velocity_to_fixed(velocity_blocks_per_tick[0]),
                velocity_to_fixed(velocity_blocks_per_tick[1]),
                velocity_to_fixed(velocity_blocks_per_tick[2]),
            ],
        })
    }

    /// Encodes the packet payload (without outer packet length or packet ID frame).
    pub fn encode_payload(&self, out: &mut Vec<u8>) {
        write_varint(self.entity_id, out);
        out.extend_from_slice(&self.uuid);
        write_varint(self.entity_type, out);
        out.extend_from_slice(&self.position[0].to_be_bytes());
        out.extend_from_slice(&self.position[1].to_be_bytes());
        out.extend_from_slice(&self.position[2].to_be_bytes());
        out.push(self.pitch);
        out.push(self.yaw);
        out.push(self.head_yaw);
        write_varint(self.data, out);
        out.extend_from_slice(&self.velocity[0].to_be_bytes());
        out.extend_from_slice(&self.velocity[1].to_be_bytes());
        out.extend_from_slice(&self.velocity[2].to_be_bytes());
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

        if data.len() < cursor + 3 {
            return Err(EntityProtocolError::UnexpectedEof);
        }
        let pitch = data[cursor];
        cursor += 1;
        let yaw = data[cursor];
        cursor += 1;
        let head_yaw = data[cursor];
        cursor += 1;

        let (data_val, n) = read_varint(&data[cursor..])?;
        cursor += n;

        if data.len() < cursor + 6 {
            return Err(EntityProtocolError::UnexpectedEof);
        }
        let vx = i16::from_be_bytes(
            data[cursor..cursor + 2]
                .try_into()
                .map_err(|_| EntityProtocolError::UnexpectedEof)?,
        );
        cursor += 2;
        let vy = i16::from_be_bytes(
            data[cursor..cursor + 2]
                .try_into()
                .map_err(|_| EntityProtocolError::UnexpectedEof)?,
        );
        cursor += 2;
        let vz = i16::from_be_bytes(
            data[cursor..cursor + 2]
                .try_into()
                .map_err(|_| EntityProtocolError::UnexpectedEof)?,
        );
        cursor += 2;

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
            [vx, vy, vz],
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
// RemoveEntitiesPacket (0x42)
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
        let mut entity_ids = Vec::with_capacity(count);
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
// Entity Metadata & SetEntityDataPacket (0x58)
// ---------------------------------------------------------------------------

/// Structured representation of item stack data within entity metadata slots.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SlotData {
    pub present: bool,
    pub item_id: i32,
    pub item_count: i8,
    pub nbt: Vec<u8>,
}

impl SlotData {
    /// Creates an empty slot.
    pub const fn empty() -> Self {
        Self {
            present: false,
            item_id: 0,
            item_count: 0,
            nbt: Vec::new(),
        }
    }

    /// Creates a slot containing an item with count and no NBT.
    pub fn item(item_id: i32, item_count: i8) -> Self {
        Self {
            present: true,
            item_id,
            item_count,
            nbt: Vec::new(),
        }
    }

    /// Creates a slot containing an item with count and custom NBT byte payload.
    pub fn with_nbt(item_id: i32, item_count: i8, nbt: Vec<u8>) -> Self {
        Self {
            present: true,
            item_id,
            item_count,
            nbt,
        }
    }
}

/// Particle configuration data within entity metadata.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParticleData {
    pub particle_id: i32,
    pub data: Vec<u8>,
}

impl ParticleData {
    pub fn new(particle_id: i32) -> Self {
        Self {
            particle_id,
            data: Vec::new(),
        }
    }

    pub fn with_data(particle_id: i32, data: Vec<u8>) -> Self {
        Self { particle_id, data }
    }
}

/// Dynamic value types supported in Minecraft Java Edition entity metadata.
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataValue {
    Byte(i8),
    VarInt(i32),
    Float(f32),
    String(String),
    Chat(String),
    Slot(SlotData),
    Bool(bool),
    Rotations([f32; 3]),
    OptPosition(Option<[i32; 3]>),
    Direction(i32),
    OptUUID(Option<[u8; 16]>),
    BlockState(i32),
    NBT(Vec<u8>),
    Particle(ParticleData),
    VillagerData {
        villager_type: i32,
        profession: i32,
        level: i32,
    },
    OptVarInt(Option<i32>),
    Pose(i32),
}

impl MetadataValue {
    /// Returns the canonical VarInt type identifier for this metadata value.
    pub const fn type_id(&self) -> i32 {
        match self {
            Self::Byte(_) => 0,
            Self::VarInt(_) => 1,
            Self::Float(_) => 2,
            Self::String(_) => 3,
            Self::Chat(_) => 4,
            Self::Slot(_) => 5,
            Self::Bool(_) => 6,
            Self::Rotations(_) => 7,
            Self::OptPosition(_) => 8,
            Self::Direction(_) => 9,
            Self::OptUUID(_) => 10,
            Self::BlockState(_) => 11,
            Self::NBT(_) => 12,
            Self::Particle(_) => 13,
            Self::VillagerData { .. } => 14,
            Self::OptVarInt(_) => 15,
            Self::Pose(_) => 16,
        }
    }

    /// Encodes the value bytes into the destination buffer.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Byte(v) => out.push(*v as u8),
            Self::VarInt(v) => write_varint(*v, out),
            Self::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
            Self::String(s) => write_string(s, out),
            Self::Chat(s) => write_string(s, out),
            Self::Slot(slot) => {
                if !slot.present {
                    out.push(0x00);
                } else {
                    out.push(0x01);
                    write_varint(slot.item_id, out);
                    out.push(slot.item_count as u8);
                    write_varint(slot.nbt.len() as i32, out);
                    out.extend_from_slice(&slot.nbt);
                }
            }
            Self::Bool(b) => out.push(if *b { 1 } else { 0 }),
            Self::Rotations(r) => {
                out.extend_from_slice(&r[0].to_be_bytes());
                out.extend_from_slice(&r[1].to_be_bytes());
                out.extend_from_slice(&r[2].to_be_bytes());
            }
            Self::OptPosition(opt) => match opt {
                None => out.push(0x00),
                Some([x, y, z]) => {
                    out.push(0x01);
                    let packed = pack_position(*x, *y, *z);
                    out.extend_from_slice(&packed.to_be_bytes());
                }
            },
            Self::Direction(dir) => write_varint(*dir, out),
            Self::OptUUID(opt) => match opt {
                None => out.push(0x00),
                Some(uuid) => {
                    out.push(0x01);
                    out.extend_from_slice(uuid);
                }
            },
            Self::BlockState(bs) => write_varint(*bs, out),
            Self::NBT(nbt) => {
                write_varint(nbt.len() as i32, out);
                out.extend_from_slice(nbt);
            }
            Self::Particle(p) => {
                write_varint(p.particle_id, out);
                write_varint(p.data.len() as i32, out);
                out.extend_from_slice(&p.data);
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
            Self::OptVarInt(opt) => match opt {
                None => write_varint(0, out),
                Some(v) => write_varint(v + 1, out),
            },
            Self::Pose(p) => write_varint(*p, out),
        }
    }

    /// Decodes a metadata value given its type ID.
    pub fn decode(type_id: i32, data: &[u8]) -> Result<(Self, usize), EntityProtocolError> {
        let mut cursor = 0;
        let val = match type_id {
            0 => {
                if data.is_empty() {
                    return Err(EntityProtocolError::UnexpectedEof);
                }
                let byte = data[0] as i8;
                cursor += 1;
                Self::Byte(byte)
            }
            1 => {
                let (v, n) = read_varint(data)?;
                cursor += n;
                Self::VarInt(v)
            }
            2 => {
                if data.len() < 4 {
                    return Err(EntityProtocolError::UnexpectedEof);
                }
                let f = f32::from_be_bytes(
                    data[0..4]
                        .try_into()
                        .map_err(|_| EntityProtocolError::UnexpectedEof)?,
                );
                cursor += 4;
                Self::Float(f)
            }
            3 => {
                let (s, n) = read_string(data)?;
                cursor += n;
                Self::String(s.to_string())
            }
            4 => {
                let (s, n) = read_string(data)?;
                cursor += n;
                Self::Chat(s.to_string())
            }
            5 => {
                if data.is_empty() {
                    return Err(EntityProtocolError::UnexpectedEof);
                }
                let present = data[0] != 0;
                cursor += 1;
                if !present {
                    Self::Slot(SlotData::empty())
                } else {
                    let (item_id, n) = read_varint(&data[cursor..])?;
                    cursor += n;
                    if cursor >= data.len() {
                        return Err(EntityProtocolError::UnexpectedEof);
                    }
                    let item_count = data[cursor] as i8;
                    cursor += 1;
                    let (nbt_len, n) = read_varint(&data[cursor..])?;
                    cursor += n;
                    if nbt_len < 0 {
                        return Err(EntityProtocolError::MalformedVarInt);
                    }
                    let nbt_len = nbt_len as usize;
                    if data.len() < cursor + nbt_len {
                        return Err(EntityProtocolError::UnexpectedEof);
                    }
                    let nbt = data[cursor..cursor + nbt_len].to_vec();
                    cursor += nbt_len;
                    Self::Slot(SlotData {
                        present: true,
                        item_id,
                        item_count,
                        nbt,
                    })
                }
            }
            6 => {
                if data.is_empty() {
                    return Err(EntityProtocolError::UnexpectedEof);
                }
                let b = data[0] != 0;
                cursor += 1;
                Self::Bool(b)
            }
            7 => {
                if data.len() < 12 {
                    return Err(EntityProtocolError::UnexpectedEof);
                }
                let rx = f32::from_be_bytes(
                    data[0..4]
                        .try_into()
                        .map_err(|_| EntityProtocolError::UnexpectedEof)?,
                );
                let ry = f32::from_be_bytes(
                    data[4..8]
                        .try_into()
                        .map_err(|_| EntityProtocolError::UnexpectedEof)?,
                );
                let rz = f32::from_be_bytes(
                    data[8..12]
                        .try_into()
                        .map_err(|_| EntityProtocolError::UnexpectedEof)?,
                );
                cursor += 12;
                Self::Rotations([rx, ry, rz])
            }
            8 => {
                if data.is_empty() {
                    return Err(EntityProtocolError::UnexpectedEof);
                }
                let present = data[0] != 0;
                cursor += 1;
                if !present {
                    Self::OptPosition(None)
                } else {
                    if data.len() < cursor + 8 {
                        return Err(EntityProtocolError::UnexpectedEof);
                    }
                    let packed = u64::from_be_bytes(
                        data[cursor..cursor + 8]
                            .try_into()
                            .map_err(|_| EntityProtocolError::UnexpectedEof)?,
                    );
                    cursor += 8;
                    Self::OptPosition(Some(unpack_position(packed)))
                }
            }
            9 => {
                let (dir, n) = read_varint(data)?;
                cursor += n;
                Self::Direction(dir)
            }
            10 => {
                if data.is_empty() {
                    return Err(EntityProtocolError::UnexpectedEof);
                }
                let present = data[0] != 0;
                cursor += 1;
                if !present {
                    Self::OptUUID(None)
                } else {
                    if data.len() < cursor + 16 {
                        return Err(EntityProtocolError::UnexpectedEof);
                    }
                    let mut uuid = [0u8; 16];
                    uuid.copy_from_slice(&data[cursor..cursor + 16]);
                    cursor += 16;
                    Self::OptUUID(Some(uuid))
                }
            }
            11 => {
                let (bs, n) = read_varint(data)?;
                cursor += n;
                Self::BlockState(bs)
            }
            12 => {
                let (len, n) = read_varint(data)?;
                cursor += n;
                if len < 0 {
                    return Err(EntityProtocolError::MalformedVarInt);
                }
                let len = len as usize;
                if data.len() < cursor + len {
                    return Err(EntityProtocolError::UnexpectedEof);
                }
                let nbt = data[cursor..cursor + len].to_vec();
                cursor += len;
                Self::NBT(nbt)
            }
            13 => {
                let (particle_id, n1) = read_varint(data)?;
                cursor += n1;
                let (len, n2) = read_varint(&data[cursor..])?;
                cursor += n2;
                if len < 0 {
                    return Err(EntityProtocolError::MalformedVarInt);
                }
                let len = len as usize;
                if data.len() < cursor + len {
                    return Err(EntityProtocolError::UnexpectedEof);
                }
                let pdata = data[cursor..cursor + len].to_vec();
                cursor += len;
                Self::Particle(ParticleData {
                    particle_id,
                    data: pdata,
                })
            }
            14 => {
                let (villager_type, n1) = read_varint(data)?;
                cursor += n1;
                let (profession, n2) = read_varint(&data[cursor..])?;
                cursor += n2;
                let (level, n3) = read_varint(&data[cursor..])?;
                cursor += n3;
                Self::VillagerData {
                    villager_type,
                    profession,
                    level,
                }
            }
            15 => {
                let (raw, n) = read_varint(data)?;
                cursor += n;
                if raw == 0 {
                    Self::OptVarInt(None)
                } else {
                    Self::OptVarInt(Some(raw - 1))
                }
            }
            16 => {
                let (pose, n) = read_varint(data)?;
                cursor += n;
                Self::Pose(pose)
            }
            other => return Err(EntityProtocolError::UnknownTypeId(other)),
        };
        Ok((val, cursor))
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
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.index);
        write_varint(self.value.type_id(), out);
        self.value.encode(out);
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
    pub fn encode_payload(&self, out: &mut Vec<u8>) {
        write_varint(self.entity_id, out);
        for entry in &self.metadata {
            entry.encode(out);
        }
        out.push(METADATA_TERMINATOR);
    }

    /// Encodes the framed packet.
    pub fn encode(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        self.encode_payload(&mut payload);
        frame_packet(Self::PACKET_ID, &payload)
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
    pub velocity: [i16; 3],
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
        velocity: [i16; 3],
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
            velocity,
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
        velocity: [i16; 3],
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
            velocity,
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
            velocity: self.velocity,
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
    fn test_velocity_scaling_units_and_clamping() {
        assert_eq!(velocity_to_fixed(0.0), 0);
        assert_eq!(velocity_to_fixed(1.0), 8000);
        assert_eq!(velocity_to_fixed(-1.0), -8000);
        assert_eq!(velocity_to_fixed(0.5), 4000);
        assert_eq!(velocity_to_fixed(-0.5), -4000);

        // Precision round trip
        assert_eq!(velocity_from_fixed(8000), 1.0);
        assert_eq!(velocity_from_fixed(-8000), -1.0);
        assert_eq!(velocity_from_fixed(4000), 0.5);

        // Boundary clamp
        assert_eq!(velocity_to_fixed(100.0), 32767);
        assert_eq!(velocity_to_fixed(-100.0), -32768);
        assert_eq!(velocity_to_fixed(4.095875), 32767);
        assert_eq!(velocity_to_fixed(-4.096), -32768);
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
    fn test_spawn_entity_packet_payload_and_framing() {
        let uuid = [
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            0x77, 0x88,
        ];
        let packet = SpawnEntityPacket::new(
            42,
            uuid,
            100, // Zombie entity type
            [123.456, 64.0, -789.012],
            angle_to_byte(-45.0),
            angle_to_byte(120.0),
            angle_to_byte(115.0),
            7,
            [8000, -4000, 16000],
        )
        .expect("valid coords");

        // Payload round trip
        let mut payload = Vec::new();
        packet.encode_payload(&mut payload);
        let decoded = SpawnEntityPacket::decode_payload(&payload).expect("decode payload");
        assert_eq!(packet, decoded);

        // Framed round trip
        let framed = packet.encode();
        let decoded_framed = SpawnEntityPacket::decode(&framed).expect("decode framed");
        assert_eq!(packet, decoded_framed);

        // Reject non-finite coordinates
        assert!(
            SpawnEntityPacket::new(1, uuid, 1, [f64::NAN, 0.0, 0.0], 0, 0, 0, 0, [0, 0, 0],)
                .is_err()
        );

        // Reject truncated packet
        assert!(SpawnEntityPacket::decode_payload(&payload[..payload.len() - 5]).is_err());

        // Reject trailing data
        let mut bad_payload = payload.clone();
        bad_payload.push(0x99);
        assert!(matches!(
            SpawnEntityPacket::decode_payload(&bad_payload),
            Err(EntityProtocolError::TrailingData)
        ));

        // Reject wrong packet ID
        let wrong_frame = frame_packet(0x99, &payload);
        assert!(matches!(
            SpawnEntityPacket::decode(&wrong_frame),
            Err(EntityProtocolError::UnexpectedPacketId {
                expected: 0x01,
                received: 0x99
            })
        ));
    }

    #[test]
    fn test_remove_entities_packet_payload_and_framing() {
        let ids = vec![1, 2, 3, 1000, 20000];
        let packet = RemoveEntitiesPacket::new(ids);

        let mut payload = Vec::new();
        packet.encode_payload(&mut payload);
        let decoded = RemoveEntitiesPacket::decode_payload(&payload).expect("decode payload");
        assert_eq!(packet, decoded);

        let framed = packet.encode();
        let decoded_framed = RemoveEntitiesPacket::decode(&framed).expect("decode framed");
        assert_eq!(packet, decoded_framed);

        // Single entity helper
        let single = RemoveEntitiesPacket::single(99);
        assert_eq!(single.entity_ids, vec![99]);

        // Empty entity removal
        let empty = RemoveEntitiesPacket::new(vec![]);
        let decoded_empty = RemoveEntitiesPacket::decode_payload(&{
            let mut b = Vec::new();
            empty.encode_payload(&mut b);
            b
        })
        .expect("decode empty");
        assert_eq!(empty, decoded_empty);
    }

    #[test]
    fn test_set_entity_data_packet_all_17_metadata_types() {
        let uuid = [0xAA; 16];
        let entries = vec![
            EntityMetadataEntry::new(0, MetadataValue::Byte(-12)),
            EntityMetadataEntry::new(1, MetadataValue::VarInt(1337)),
            EntityMetadataEntry::new(2, MetadataValue::Float(std::f32::consts::PI)),
            EntityMetadataEntry::new(3, MetadataValue::String("RustMC-Zombie".to_string())),
            EntityMetadataEntry::new(4, MetadataValue::Chat("{\"text\":\"Hello\"}".to_string())),
            EntityMetadataEntry::new(5, MetadataValue::Slot(SlotData::empty())),
            EntityMetadataEntry::new(
                6,
                MetadataValue::Slot(SlotData::with_nbt(276, 1, vec![0x0A, 0x00, 0x00])),
            ),
            EntityMetadataEntry::new(7, MetadataValue::Bool(true)),
            EntityMetadataEntry::new(8, MetadataValue::Rotations([15.0, 30.0, 45.0])),
            EntityMetadataEntry::new(9, MetadataValue::OptPosition(None)),
            EntityMetadataEntry::new(10, MetadataValue::OptPosition(Some([-10, 64, 25]))),
            EntityMetadataEntry::new(11, MetadataValue::Direction(direction::NORTH)),
            EntityMetadataEntry::new(12, MetadataValue::OptUUID(None)),
            EntityMetadataEntry::new(13, MetadataValue::OptUUID(Some(uuid))),
            EntityMetadataEntry::new(14, MetadataValue::BlockState(88)),
            EntityMetadataEntry::new(15, MetadataValue::NBT(vec![0x00])),
            EntityMetadataEntry::new(16, MetadataValue::Particle(ParticleData::new(12))),
            EntityMetadataEntry::new(
                17,
                MetadataValue::Particle(ParticleData::with_data(15, vec![1, 2, 3])),
            ),
            EntityMetadataEntry::new(
                18,
                MetadataValue::VillagerData {
                    villager_type: 1,
                    profession: 2,
                    level: 3,
                },
            ),
            EntityMetadataEntry::new(19, MetadataValue::OptVarInt(None)),
            EntityMetadataEntry::new(20, MetadataValue::OptVarInt(Some(42))),
            EntityMetadataEntry::new(21, MetadataValue::Pose(pose::SNEAKING)),
        ];

        let packet = SetEntityDataPacket::new(1001, entries);

        // Payload round trip
        let mut payload = Vec::new();
        packet.encode_payload(&mut payload);
        let decoded = SetEntityDataPacket::decode_payload(&payload).expect("decode payload");
        assert_eq!(packet, decoded);

        // Framed round trip
        let framed = packet.encode();
        let decoded_framed = SetEntityDataPacket::decode(&framed).expect("decode framed");
        assert_eq!(packet, decoded_framed);

        // Reject missing terminator (0xFF)
        assert!(SetEntityDataPacket::decode_payload(&payload[..payload.len() - 1]).is_err());
    }

    #[test]
    fn test_entity_visual_tracker_enter_and_leave_radius() {
        let mut tracker = EntityVisualTracker::new();

        let uuid1 = [1u8; 16];
        let uuid2 = [2u8; 16];

        // Register entity 1 at (10.0, 64.0, 10.0)
        let entity1 =
            TrackedEntityState::new(1, uuid1, 20, [10.0, 64.0, 10.0], 0, 0, 0, 0, [0, 0, 0])
                .unwrap();

        // Register entity 2 at (100.0, 64.0, 100.0)
        let entity2 =
            TrackedEntityState::new(2, uuid2, 21, [100.0, 64.0, 100.0], 0, 0, 0, 0, [0, 0, 0])
                .unwrap();

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
        let entity =
            TrackedEntityState::new(10, uuid, 1, [0.0, 0.0, 0.0], 0, 0, 0, 0, [0, 0, 0]).unwrap();
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
