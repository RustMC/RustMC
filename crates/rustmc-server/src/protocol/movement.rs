//! Java Edition 26.3 player movement packet decoders, teleport confirmation tracker,
//! and movement speed/displacement validation guards.
//!
//! # Protocol Specification
//!
//! Under Minecraft Java Edition 26.3 (protocol 777), player movement updates are sent by
//! the client in Play state across four movement packet variants and one teleport confirmation packet:
//!
//! - `MovePlayerPos` (0x1A): `x: f64, y: f64, z: f64, on_ground: bool`
//! - `MovePlayerPosRot` (0x1B): `x: f64, y: f64, z: f64, yaw: f32, pitch: f32, on_ground: bool`
//! - `MovePlayerRot` (0x1C): `yaw: f32, pitch: f32, on_ground: bool`
//! - `MovePlayerStatus` (0x1D): `on_ground: bool`
//! - `AcceptTeleportation` (0x00): `teleport_id: i32` (VarInt)
//!
//! The server coordinates player teleportation with clientbound `PlayerPositionSync` (0x40):
//! - `teleport_id: VarInt`
//! - `x: f64, y: f64, z: f64`
//! - `dx: f64, dy: f64, dz: f64` (velocity)
//! - `yaw: f32, pitch: f32`
//! - `flags: i32` (bitfield for relative coordinates)
//!
//! # Validation Guards
//!
//! - Reject non-finite floating-point coordinates (NaN and Infinity).
//! - Normalize yaw into `[-180.0, 180.0)` degrees and clamp pitch to `[-90.0, 90.0]` degrees.
//! - Enforce teleport challenge-response tracking: reject client movements when a teleport
//!   confirmation is pending.
//! - Guard against illegal speed and teleportation exploits by enforcing single-tick maximum
//!   displacement ($\Delta d \le 100.0$ blocks) unless authorized by a confirmed teleport.

use std::collections::BTreeMap;
use std::fmt;

/// Serverbound packet ID for Java 26.3 AcceptTeleportation (Play state).
pub const PACKET_ID_ACCEPT_TELEPORTATION: u32 = 0x00;

/// Serverbound packet ID for Java 26.3 MovePlayerPos (Play state).
pub const PACKET_ID_MOVE_PLAYER_POS: u32 = 0x1A;

/// Serverbound packet ID for Java 26.3 MovePlayerPosRot (Play state).
pub const PACKET_ID_MOVE_PLAYER_POS_ROT: u32 = 0x1B;

/// Serverbound packet ID for Java 26.3 MovePlayerRot (Play state).
pub const PACKET_ID_MOVE_PLAYER_ROT: u32 = 0x1C;

/// Serverbound packet ID for Java 26.3 MovePlayerStatus (Play state).
pub const PACKET_ID_MOVE_PLAYER_STATUS: u32 = 0x1D;

/// Clientbound packet ID for Java 26.3 PlayerPositionSync (Play state).
pub const PACKET_ID_PLAYER_POSITION_SYNC: u32 = 0x40;

/// Maximum single-tick Euclidean displacement threshold (in blocks).
pub const MAX_DISPLACEMENT_PER_TICK: f64 = 100.0;

/// Maximum bounded pending teleports tracked per session.
pub const MAX_PENDING_TELEPORTS: usize = 32;

/// Bitfield flags for clientbound `PlayerPositionSync` packet.
pub mod position_sync_flags {
    pub const RELATIVE_X: i32 = 0x01;
    pub const RELATIVE_Y: i32 = 0x02;
    pub const RELATIVE_Z: i32 = 0x04;
    pub const RELATIVE_Y_ROT: i32 = 0x08;
    pub const RELATIVE_X_ROT: i32 = 0x10;
    pub const RELATIVE_DELTA_X: i32 = 0x20;
    pub const RELATIVE_DELTA_Y: i32 = 0x40;
    pub const RELATIVE_DELTA_Z: i32 = 0x80;
    pub const ROTATE_DELTA: i32 = 0x100;
}

/// Errors occurring during movement packet decoding, framing, or parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovementError {
    UnexpectedEof,
    MalformedVarInt,
    TrailingData,
    InvalidPacketLength { expected: usize, received: usize },
    InvalidFlags(u8),
    NonFiniteCoordinate,
    NonFiniteRotation,
    NonFiniteFloat,
    UnknownPacketId(u32),
    UnexpectedPacketId { expected: u32, received: u32 },
}

impl fmt::Display for MovementError {
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
            Self::InvalidFlags(flags) => {
                write!(
                    f,
                    "invalid movement flags byte with unknown bits: 0x{flags:02X}"
                )
            }
            Self::NonFiniteCoordinate => {
                write!(f, "movement coordinate is non-finite (NaN or Inf)")
            }
            Self::NonFiniteRotation => write!(f, "rotation angle is non-finite (NaN or Inf)"),
            Self::NonFiniteFloat => write!(f, "packet contains non-finite floating-point value"),
            Self::UnknownPacketId(id) => write!(f, "unknown movement packet ID: 0x{id:02X}"),
            Self::UnexpectedPacketId { expected, received } => {
                write!(
                    f,
                    "unexpected packet ID: expected 0x{expected:02X}, received 0x{received:02X}"
                )
            }
        }
    }
}

impl std::error::Error for MovementError {}

/// Errors occurring during teleportation tracking and confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeleportError {
    NoPendingTeleport,
    UnknownTeleportId { id: i32 },
    ObsoleteTeleportId { id: i32, latest: i32 },
    MovementWhilePending { pending_id: i32 },
}

impl fmt::Display for TeleportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPendingTeleport => {
                write!(f, "received teleport confirmation with no teleport pending")
            }
            Self::UnknownTeleportId { id } => write!(f, "unknown teleport ID confirmed: {id}"),
            Self::ObsoleteTeleportId { id, latest } => {
                write!(
                    f,
                    "obsolete teleport ID confirmed: {id} (latest active teleport is {latest})"
                )
            }
            Self::MovementWhilePending { pending_id } => {
                write!(
                    f,
                    "client movement update rejected while teleport {pending_id} is awaiting confirmation"
                )
            }
        }
    }
}

impl std::error::Error for TeleportError {}

/// Errors occurring during movement speed and authoritative position validation.
#[derive(Debug, Clone, PartialEq)]
pub enum MovementValidationError {
    TeleportPending {
        pending_id: i32,
        expected: (f64, f64, f64),
    },
    SpeedExceeded {
        displacement: f64,
        max_allowed: f64,
        reset_to: (f64, f64, f64),
    },
    TeleportError(TeleportError),
    MalformedPacket(MovementError),
}

impl fmt::Display for MovementValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TeleportPending {
                pending_id,
                expected,
            } => {
                write!(
                    f,
                    "movement rejected: pending teleport {pending_id} to ({:.2}, {:.2}, {:.2}) awaiting confirmation",
                    expected.0, expected.1, expected.2
                )
            }
            Self::SpeedExceeded {
                displacement,
                max_allowed,
                reset_to,
            } => {
                write!(
                    f,
                    "illegal movement displacement: {:.2} blocks exceeded threshold {:.2}; reset to authoritative ({:.2}, {:.2}, {:.2})",
                    displacement, max_allowed, reset_to.0, reset_to.1, reset_to.2
                )
            }
            Self::TeleportError(err) => write!(f, "teleport error: {err}"),
            Self::MalformedPacket(err) => write!(f, "malformed packet: {err}"),
        }
    }
}

impl std::error::Error for MovementValidationError {}

impl From<TeleportError> for MovementValidationError {
    fn from(err: TeleportError) -> Self {
        Self::TeleportError(err)
    }
}

impl From<MovementError> for MovementValidationError {
    fn from(err: MovementError) -> Self {
        Self::MalformedPacket(err)
    }
}

/// Reads a VarInt from a byte buffer. Returns `(value, bytes_read)` on success.
pub fn read_varint(input: &[u8]) -> Result<(i32, usize), MovementError> {
    let mut value = 0u32;
    for (i, &byte) in input.iter().take(5).enumerate() {
        if i == 4 && (byte & 0xf0) != 0 {
            return Err(MovementError::MalformedVarInt);
        }
        value |= u32::from(byte & 0x7f) << (i * 7);
        if (byte & 0x80) == 0 {
            if i > 0 && byte == 0 && value == 0 {
                return Err(MovementError::MalformedVarInt);
            }
            return Ok((value as i32, i + 1));
        }
    }
    if input.len() >= 5 {
        Err(MovementError::MalformedVarInt)
    } else {
        Err(MovementError::UnexpectedEof)
    }
}

/// Writes a VarInt to a byte buffer.
pub fn write_varint(mut value: u32, out: &mut Vec<u8>) {
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

/// Frames a packet with its packet ID and outer length VarInt.
pub fn frame_packet(packet_id: u32, body: &[u8]) -> Vec<u8> {
    let mut packet = Vec::new();
    write_varint(packet_id, &mut packet);
    packet.extend_from_slice(body);

    let mut framed = Vec::new();
    write_varint(packet.len() as u32, &mut framed);
    framed.extend(packet);
    framed
}

/// Unframes an outer-framed packet, returning `(packet_id, body_slice)`.
pub fn unframe_packet(framed: &[u8]) -> Result<(u32, &[u8]), MovementError> {
    let (length, prefix_len) = read_varint(framed)?;
    if length < 0 {
        return Err(MovementError::MalformedVarInt);
    }
    let length = length as usize;
    if framed.len() < prefix_len + length {
        return Err(MovementError::UnexpectedEof);
    }
    if framed.len() > prefix_len + length {
        return Err(MovementError::TrailingData);
    }
    let packet_data = &framed[prefix_len..prefix_len + length];
    let (packet_id, id_len) = read_varint(packet_data)?;
    if packet_id < 0 {
        return Err(MovementError::MalformedVarInt);
    }
    Ok((packet_id as u32, &packet_data[id_len..]))
}

/// Validates whether float coordinates (x, y, z) are finite (not NaN or Infinity).
#[inline]
pub fn validate_coords(x: f64, y: f64, z: f64) -> Result<(), MovementError> {
    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
        return Err(MovementError::NonFiniteCoordinate);
    }
    Ok(())
}

/// Validates whether rotation angles (yaw, pitch) are finite.
#[inline]
pub fn validate_rotation(yaw: f32, pitch: f32) -> Result<(), MovementError> {
    if !yaw.is_finite() || !pitch.is_finite() {
        return Err(MovementError::NonFiniteRotation);
    }
    Ok(())
}

/// Normalizes yaw angle to the standard range `[-180.0, 180.0)` degrees.
#[inline]
pub fn normalize_yaw(yaw: f32) -> f32 {
    let mut wrapped = yaw % 360.0;
    if wrapped >= 180.0 {
        wrapped -= 360.0;
    } else if wrapped < -180.0 {
        wrapped += 360.0;
    }
    if wrapped == -0.0 { 0.0 } else { wrapped }
}

/// Clamps pitch angle to `[-90.0, 90.0]` degrees.
#[inline]
pub fn normalize_pitch(pitch: f32) -> f32 {
    pitch.clamp(-90.0, 90.0)
}

/// Parses the movement flags byte (bit 0 = on_ground, bit 1 = horizontal_collision).
///
/// Rejects flags with unknown upper bits set (`byte & !0x03 != 0`).
#[inline]
pub fn parse_flags_byte(byte: u8) -> Result<(bool, bool), MovementError> {
    if (byte & !0x03) != 0 {
        return Err(MovementError::InvalidFlags(byte));
    }
    let on_ground = (byte & 0x01) != 0;
    let horizontal_collision = (byte & 0x02) != 0;
    Ok((on_ground, horizontal_collision))
}

/// Encodes the movement flags byte from `on_ground` and `horizontal_collision`.
#[inline]
pub fn encode_flags_byte(on_ground: bool, horizontal_collision: bool) -> u8 {
    let mut flags = 0u8;
    if on_ground {
        flags |= 0x01;
    }
    if horizontal_collision {
        flags |= 0x02;
    }
    flags
}

/// Computes the 3D Euclidean displacement between two coordinates:
/// $\Delta d = \sqrt{\Delta x^2 + \Delta y^2 + \Delta z^2}$.
#[inline]
pub fn calculate_displacement(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> f64 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let dz = z2 - z1;
    dx.mul_add(dx, dy.mul_add(dy, dz * dz)).sqrt()
}

/// Serverbound `MovePlayerPos` packet (0x1A): `x: f64, y: f64, z: f64, on_ground: bool`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovePlayerPos {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub on_ground: bool,
    pub horizontal_collision: bool,
}

impl MovePlayerPos {
    pub const PACKET_ID: u32 = PACKET_ID_MOVE_PLAYER_POS;

    pub const fn new(x: f64, y: f64, z: f64, on_ground: bool) -> Self {
        Self {
            x,
            y,
            z,
            on_ground,
            horizontal_collision: false,
        }
    }

    pub const fn with_collision(
        x: f64,
        y: f64,
        z: f64,
        on_ground: bool,
        horizontal_collision: bool,
    ) -> Self {
        Self {
            x,
            y,
            z,
            on_ground,
            horizontal_collision,
        }
    }

    /// Decodes the payload bytes for `MovePlayerPos` (25 bytes).
    pub fn decode(data: &[u8]) -> Result<Self, MovementError> {
        if data.len() != 25 {
            return Err(MovementError::InvalidPacketLength {
                expected: 25,
                received: data.len(),
            });
        }
        let x = f64::from_be_bytes(data[0..8].try_into().unwrap());
        let y = f64::from_be_bytes(data[8..16].try_into().unwrap());
        let z = f64::from_be_bytes(data[16..24].try_into().unwrap());
        validate_coords(x, y, z)?;
        let (on_ground, horizontal_collision) = parse_flags_byte(data[24])?;

        Ok(Self {
            x,
            y,
            z,
            on_ground,
            horizontal_collision,
        })
    }

    /// Encodes the packet payload to bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(25);
        out.extend_from_slice(&self.x.to_be_bytes());
        out.extend_from_slice(&self.y.to_be_bytes());
        out.extend_from_slice(&self.z.to_be_bytes());
        out.push(encode_flags_byte(self.on_ground, self.horizontal_collision));
        out
    }

    /// Encodes a framed packet (length VarInt + packet ID 0x1A + payload).
    pub fn encode_framed(&self) -> Vec<u8> {
        frame_packet(Self::PACKET_ID, &self.encode())
    }
}

/// Serverbound `MovePlayerPosRot` packet (0x1B):
/// `x: f64, y: f64, z: f64, yaw: f32, pitch: f32, on_ground: bool`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovePlayerPosRot {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub horizontal_collision: bool,
}

impl MovePlayerPosRot {
    pub const PACKET_ID: u32 = PACKET_ID_MOVE_PLAYER_POS_ROT;

    pub const fn new(x: f64, y: f64, z: f64, yaw: f32, pitch: f32, on_ground: bool) -> Self {
        Self {
            x,
            y,
            z,
            yaw,
            pitch,
            on_ground,
            horizontal_collision: false,
        }
    }

    pub const fn with_collision(
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
        horizontal_collision: bool,
    ) -> Self {
        Self {
            x,
            y,
            z,
            yaw,
            pitch,
            on_ground,
            horizontal_collision,
        }
    }

    /// Decodes the payload bytes for `MovePlayerPosRot` (33 bytes).
    pub fn decode(data: &[u8]) -> Result<Self, MovementError> {
        if data.len() != 33 {
            return Err(MovementError::InvalidPacketLength {
                expected: 33,
                received: data.len(),
            });
        }
        let x = f64::from_be_bytes(data[0..8].try_into().unwrap());
        let y = f64::from_be_bytes(data[8..16].try_into().unwrap());
        let z = f64::from_be_bytes(data[16..24].try_into().unwrap());
        validate_coords(x, y, z)?;

        let raw_yaw = f32::from_be_bytes(data[24..28].try_into().unwrap());
        let raw_pitch = f32::from_be_bytes(data[28..32].try_into().unwrap());
        validate_rotation(raw_yaw, raw_pitch)?;

        let yaw = normalize_yaw(raw_yaw);
        let pitch = normalize_pitch(raw_pitch);
        let (on_ground, horizontal_collision) = parse_flags_byte(data[32])?;

        Ok(Self {
            x,
            y,
            z,
            yaw,
            pitch,
            on_ground,
            horizontal_collision,
        })
    }

    /// Encodes the packet payload to bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(33);
        out.extend_from_slice(&self.x.to_be_bytes());
        out.extend_from_slice(&self.y.to_be_bytes());
        out.extend_from_slice(&self.z.to_be_bytes());
        out.extend_from_slice(&self.yaw.to_be_bytes());
        out.extend_from_slice(&self.pitch.to_be_bytes());
        out.push(encode_flags_byte(self.on_ground, self.horizontal_collision));
        out
    }

    /// Encodes a framed packet (length VarInt + packet ID 0x1B + payload).
    pub fn encode_framed(&self) -> Vec<u8> {
        frame_packet(Self::PACKET_ID, &self.encode())
    }
}

/// Serverbound `MovePlayerRot` packet (0x1C): `yaw: f32, pitch: f32, on_ground: bool`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovePlayerRot {
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub horizontal_collision: bool,
}

impl MovePlayerRot {
    pub const PACKET_ID: u32 = PACKET_ID_MOVE_PLAYER_ROT;

    pub const fn new(yaw: f32, pitch: f32, on_ground: bool) -> Self {
        Self {
            yaw,
            pitch,
            on_ground,
            horizontal_collision: false,
        }
    }

    pub const fn with_collision(
        yaw: f32,
        pitch: f32,
        on_ground: bool,
        horizontal_collision: bool,
    ) -> Self {
        Self {
            yaw,
            pitch,
            on_ground,
            horizontal_collision,
        }
    }

    /// Decodes the payload bytes for `MovePlayerRot` (9 bytes).
    pub fn decode(data: &[u8]) -> Result<Self, MovementError> {
        if data.len() != 9 {
            return Err(MovementError::InvalidPacketLength {
                expected: 9,
                received: data.len(),
            });
        }
        let raw_yaw = f32::from_be_bytes(data[0..4].try_into().unwrap());
        let raw_pitch = f32::from_be_bytes(data[4..8].try_into().unwrap());
        validate_rotation(raw_yaw, raw_pitch)?;

        let yaw = normalize_yaw(raw_yaw);
        let pitch = normalize_pitch(raw_pitch);
        let (on_ground, horizontal_collision) = parse_flags_byte(data[8])?;

        Ok(Self {
            yaw,
            pitch,
            on_ground,
            horizontal_collision,
        })
    }

    /// Encodes the packet payload to bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(9);
        out.extend_from_slice(&self.yaw.to_be_bytes());
        out.extend_from_slice(&self.pitch.to_be_bytes());
        out.push(encode_flags_byte(self.on_ground, self.horizontal_collision));
        out
    }

    /// Encodes a framed packet (length VarInt + packet ID 0x1C + payload).
    pub fn encode_framed(&self) -> Vec<u8> {
        frame_packet(Self::PACKET_ID, &self.encode())
    }
}

/// Serverbound `MovePlayerStatus` packet (0x1D): `on_ground: bool`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovePlayerStatus {
    pub on_ground: bool,
    pub horizontal_collision: bool,
}

impl MovePlayerStatus {
    pub const PACKET_ID: u32 = PACKET_ID_MOVE_PLAYER_STATUS;

    pub const fn new(on_ground: bool) -> Self {
        Self {
            on_ground,
            horizontal_collision: false,
        }
    }

    pub const fn with_collision(on_ground: bool, horizontal_collision: bool) -> Self {
        Self {
            on_ground,
            horizontal_collision,
        }
    }

    /// Decodes the payload bytes for `MovePlayerStatus` (1 byte).
    pub fn decode(data: &[u8]) -> Result<Self, MovementError> {
        if data.len() != 1 {
            return Err(MovementError::InvalidPacketLength {
                expected: 1,
                received: data.len(),
            });
        }
        let (on_ground, horizontal_collision) = parse_flags_byte(data[0])?;
        Ok(Self {
            on_ground,
            horizontal_collision,
        })
    }

    /// Encodes the packet payload to bytes.
    pub fn encode(&self) -> Vec<u8> {
        vec![encode_flags_byte(self.on_ground, self.horizontal_collision)]
    }

    /// Encodes a framed packet (length VarInt + packet ID 0x1D + payload).
    pub fn encode_framed(&self) -> Vec<u8> {
        frame_packet(Self::PACKET_ID, &self.encode())
    }
}

/// Serverbound `AcceptTeleportation` packet (0x00): `teleport_id: i32` (VarInt).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptTeleportation {
    pub teleport_id: i32,
}

impl AcceptTeleportation {
    pub const PACKET_ID: u32 = PACKET_ID_ACCEPT_TELEPORTATION;

    pub const fn new(teleport_id: i32) -> Self {
        Self { teleport_id }
    }

    /// Decodes the payload bytes for `AcceptTeleportation` (VarInt).
    pub fn decode(data: &[u8]) -> Result<Self, MovementError> {
        let (teleport_id, bytes_read) = read_varint(data)?;
        if bytes_read != data.len() {
            return Err(MovementError::TrailingData);
        }
        Ok(Self { teleport_id })
    }

    /// Encodes the packet payload to bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(5);
        write_varint(self.teleport_id as u32, &mut out);
        out
    }

    /// Encodes a framed packet (length VarInt + packet ID 0x00 + payload).
    pub fn encode_framed(&self) -> Vec<u8> {
        frame_packet(Self::PACKET_ID, &self.encode())
    }
}

/// Enumeration of all serverbound Java 26.3 movement packet variants.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerboundMovementPacket {
    Pos(MovePlayerPos),
    PosRot(MovePlayerPosRot),
    Rot(MovePlayerRot),
    Status(MovePlayerStatus),
    AcceptTeleport(AcceptTeleportation),
}

/// Decodes any serverbound movement packet given its packet ID and payload.
pub fn decode_serverbound_movement_packet(
    packet_id: u32,
    body: &[u8],
) -> Result<ServerboundMovementPacket, MovementError> {
    match packet_id {
        PACKET_ID_MOVE_PLAYER_POS => {
            MovePlayerPos::decode(body).map(ServerboundMovementPacket::Pos)
        }
        PACKET_ID_MOVE_PLAYER_POS_ROT => {
            MovePlayerPosRot::decode(body).map(ServerboundMovementPacket::PosRot)
        }
        PACKET_ID_MOVE_PLAYER_ROT => {
            MovePlayerRot::decode(body).map(ServerboundMovementPacket::Rot)
        }
        PACKET_ID_MOVE_PLAYER_STATUS => {
            MovePlayerStatus::decode(body).map(ServerboundMovementPacket::Status)
        }
        PACKET_ID_ACCEPT_TELEPORTATION => {
            AcceptTeleportation::decode(body).map(ServerboundMovementPacket::AcceptTeleport)
        }
        _ => Err(MovementError::UnknownPacketId(packet_id)),
    }
}

/// Clientbound `PlayerPositionSync` packet (0x40 in Java 26.3):
/// Encodes `teleport_id: VarInt`, `x: f64, y: f64, z: f64`,
/// `dx: f64, dy: f64, dz: f64` (velocity), `yaw: f32, pitch: f32`, `flags: i32`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerPositionSync {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub flags: i32,
}

impl PlayerPositionSync {
    pub const PACKET_ID: u32 = PACKET_ID_PLAYER_POSITION_SYNC;

    /// Creates an absolute position synchronization packet with zero initial velocity.
    pub const fn new_absolute(
        teleport_id: i32,
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
    ) -> Self {
        Self {
            teleport_id,
            x,
            y,
            z,
            dx: 0.0,
            dy: 0.0,
            dz: 0.0,
            yaw,
            pitch,
            flags: 0,
        }
    }

    /// Creates a position synchronization packet specifying velocities and relative flags.
    #[allow(clippy::too_many_arguments)]
    pub const fn with_velocity(
        teleport_id: i32,
        x: f64,
        y: f64,
        z: f64,
        dx: f64,
        dy: f64,
        dz: f64,
        yaw: f32,
        pitch: f32,
        flags: i32,
    ) -> Self {
        Self {
            teleport_id,
            x,
            y,
            z,
            dx,
            dy,
            dz,
            yaw,
            pitch,
            flags,
        }
    }

    /// Encodes the packet payload bytes.
    pub fn encode_body(&self, out: &mut Vec<u8>) {
        write_varint(self.teleport_id as u32, out);
        out.extend_from_slice(&self.x.to_be_bytes());
        out.extend_from_slice(&self.y.to_be_bytes());
        out.extend_from_slice(&self.z.to_be_bytes());
        out.extend_from_slice(&self.dx.to_be_bytes());
        out.extend_from_slice(&self.dy.to_be_bytes());
        out.extend_from_slice(&self.dz.to_be_bytes());
        out.extend_from_slice(&self.yaw.to_be_bytes());
        out.extend_from_slice(&self.pitch.to_be_bytes());
        out.extend_from_slice(&self.flags.to_be_bytes());
    }

    /// Encodes packet ID 0x40 plus the body bytes (unframed).
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(70);
        write_varint(Self::PACKET_ID, &mut packet);
        self.encode_body(&mut packet);
        packet
    }

    /// Encodes an outer-framed packet (length VarInt + packet ID 0x40 + payload).
    pub fn encode_framed(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(65);
        self.encode_body(&mut body);
        frame_packet(Self::PACKET_ID, &body)
    }

    /// Decodes the payload bytes into `PlayerPositionSync`.
    pub fn decode_body(input: &[u8]) -> Result<Self, MovementError> {
        let (teleport_id, offset) = read_varint(input)?;
        let remaining = &input[offset..];
        if remaining.len() != 60 {
            return Err(MovementError::InvalidPacketLength {
                expected: offset + 60,
                received: input.len(),
            });
        }

        let x = f64::from_be_bytes(remaining[0..8].try_into().unwrap());
        let y = f64::from_be_bytes(remaining[8..16].try_into().unwrap());
        let z = f64::from_be_bytes(remaining[16..24].try_into().unwrap());
        let dx = f64::from_be_bytes(remaining[24..32].try_into().unwrap());
        let dy = f64::from_be_bytes(remaining[32..40].try_into().unwrap());
        let dz = f64::from_be_bytes(remaining[40..48].try_into().unwrap());
        let yaw = f32::from_be_bytes(remaining[48..52].try_into().unwrap());
        let pitch = f32::from_be_bytes(remaining[52..56].try_into().unwrap());
        let flags = i32::from_be_bytes(remaining[56..60].try_into().unwrap());

        if !x.is_finite()
            || !y.is_finite()
            || !z.is_finite()
            || !dx.is_finite()
            || !dy.is_finite()
            || !dz.is_finite()
            || !yaw.is_finite()
            || !pitch.is_finite()
        {
            return Err(MovementError::NonFiniteFloat);
        }

        Ok(Self {
            teleport_id,
            x,
            y,
            z,
            dx,
            dy,
            dz,
            yaw,
            pitch,
            flags,
        })
    }

    /// Decodes an outer-framed packet into `PlayerPositionSync`.
    pub fn decode_framed(input: &[u8]) -> Result<Self, MovementError> {
        let (packet_id, body) = unframe_packet(input)?;
        if packet_id != Self::PACKET_ID {
            return Err(MovementError::UnexpectedPacketId {
                expected: Self::PACKET_ID,
                received: packet_id,
            });
        }
        Self::decode_body(body)
    }
}

/// Information recorded for a pending teleport awaiting client confirmation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingTeleport {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub tick: u64,
}

/// Tracks sequential teleport confirmations and guards movement states.
#[derive(Debug, Clone, PartialEq)]
pub struct TeleportTracker {
    next_id: i32,
    pending: BTreeMap<i32, PendingTeleport>,
    latest_issued_id: Option<i32>,
    last_confirmed_id: Option<i32>,
}

impl Default for TeleportTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl TeleportTracker {
    /// Creates a new `TeleportTracker` with sequential IDs starting at 1.
    pub const fn new() -> Self {
        Self {
            next_id: 1,
            pending: BTreeMap::new(),
            latest_issued_id: None,
            last_confirmed_id: None,
        }
    }

    /// Issues the next sequential teleport ID.
    pub fn next_teleport_id(&mut self) -> i32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id <= 0 {
            self.next_id = 1;
        }
        id
    }

    /// Records a pending teleport with expected target coordinates and tick.
    pub fn register_teleport(&mut self, teleport_id: i32, target: (f64, f64, f64), tick: u64) {
        // Enforce bounded memory retention
        if self.pending.len() >= MAX_PENDING_TELEPORTS
            && let Some(&oldest_id) = self.pending.keys().next()
        {
            self.pending.remove(&oldest_id);
        }

        self.latest_issued_id = Some(teleport_id);
        self.pending.insert(
            teleport_id,
            PendingTeleport {
                teleport_id,
                x: target.0,
                y: target.1,
                z: target.2,
                tick,
            },
        );
    }

    /// Issues a new teleport ID and registers it in a single operation.
    pub fn issue_teleport(&mut self, target: (f64, f64, f64), tick: u64) -> i32 {
        let id = self.next_teleport_id();
        self.register_teleport(id, target, tick);
        id
    }

    /// Returns `true` if any teleport is currently pending confirmation.
    #[inline]
    pub fn has_pending_teleport(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Alias for `has_pending_teleport`.
    #[inline]
    pub fn is_teleport_pending(&self) -> bool {
        self.has_pending_teleport()
    }

    /// Returns a reference to the latest active pending teleport, if any.
    pub fn pending_teleport(&self) -> Option<&PendingTeleport> {
        self.latest_issued_id
            .and_then(|id| self.pending.get(&id))
            .or_else(|| self.pending.values().next_back())
    }

    /// Returns the number of currently pending teleports.
    #[inline]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Returns the last confirmed teleport ID, if any.
    #[inline]
    pub const fn last_confirmed_id(&self) -> Option<i32> {
        self.last_confirmed_id
    }

    /// Asserts that normal movement updates are permitted (no teleport pending).
    pub fn validate_movement_allowed(&self) -> Result<(), TeleportError> {
        if let Some(pending) = self.pending_teleport() {
            return Err(TeleportError::MovementWhilePending {
                pending_id: pending.teleport_id,
            });
        }
        Ok(())
    }

    /// Accepts a client teleport confirmation.
    ///
    /// Clears confirmed and superseded pending teleports, updating `last_confirmed_id`.
    /// Rejects obsolete or unknown teleport IDs.
    pub fn accept_teleport(&mut self, teleport_id: i32) -> Result<PendingTeleport, TeleportError> {
        if let Some(last) = self.last_confirmed_id
            && teleport_id <= last
        {
            return Err(TeleportError::ObsoleteTeleportId {
                id: teleport_id,
                latest: self.latest_issued_id.unwrap_or(last),
            });
        }

        if !self.pending.contains_key(&teleport_id) {
            if self.pending.is_empty() {
                return Err(TeleportError::NoPendingTeleport);
            }
            return Err(TeleportError::UnknownTeleportId { id: teleport_id });
        }

        let latest_id = self.latest_issued_id.unwrap_or(teleport_id);
        if teleport_id < latest_id {
            return Err(TeleportError::ObsoleteTeleportId {
                id: teleport_id,
                latest: latest_id,
            });
        }

        let confirmed = self.pending.remove(&teleport_id).unwrap();
        // Clear any prior pending teleports up to this confirmed ID
        self.pending.retain(|&id, _| id > teleport_id);
        self.last_confirmed_id = Some(teleport_id);
        Ok(confirmed)
    }

    /// Resets all teleport tracker state.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.latest_issued_id = None;
        self.last_confirmed_id = None;
    }
}

/// Result of processing a validated movement or teleport packet.
#[derive(Debug, Clone, PartialEq)]
pub enum MovementOutcome {
    TeleportConfirmed(PendingTeleport),
    PositionUpdated {
        x: f64,
        y: f64,
        z: f64,
        displacement: f64,
    },
    PositionAndRotationUpdated {
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
        displacement: f64,
    },
    RotationUpdated {
        yaw: f32,
        pitch: f32,
    },
    StatusUpdated {
        on_ground: bool,
    },
}

/// Movement speed, distance, and authoritative position guard.
#[derive(Debug, Clone, PartialEq)]
pub struct MovementGuard {
    authoritative_pos: (f64, f64, f64),
    authoritative_rot: (f32, f32),
    authoritative_on_ground: bool,
    teleport_tracker: TeleportTracker,
    max_displacement: f64,
}

impl MovementGuard {
    /// Creates a new `MovementGuard` at the given initial spawn coordinate.
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self {
            authoritative_pos: (x, y, z),
            authoritative_rot: (0.0, 0.0),
            authoritative_on_ground: true,
            teleport_tracker: TeleportTracker::new(),
            max_displacement: MAX_DISPLACEMENT_PER_TICK,
        }
    }

    /// Creates a new `MovementGuard` with initial position and orientation.
    pub fn with_rotation(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Self {
        Self {
            authoritative_pos: (x, y, z),
            authoritative_rot: (normalize_yaw(yaw), normalize_pitch(pitch)),
            authoritative_on_ground: true,
            teleport_tracker: TeleportTracker::new(),
            max_displacement: MAX_DISPLACEMENT_PER_TICK,
        }
    }

    #[inline]
    pub const fn position(&self) -> (f64, f64, f64) {
        self.authoritative_pos
    }

    #[inline]
    pub const fn rotation(&self) -> (f32, f32) {
        self.authoritative_rot
    }

    #[inline]
    pub const fn on_ground(&self) -> bool {
        self.authoritative_on_ground
    }

    #[inline]
    pub const fn teleport_tracker(&self) -> &TeleportTracker {
        &self.teleport_tracker
    }

    #[inline]
    pub fn teleport_tracker_mut(&mut self) -> &mut TeleportTracker {
        &mut self.teleport_tracker
    }

    #[inline]
    pub const fn max_displacement(&self) -> f64 {
        self.max_displacement
    }

    #[inline]
    pub fn set_max_displacement(&mut self, threshold: f64) {
        self.max_displacement = threshold;
    }

    /// Initiates a server-authoritative teleport, recording it in the tracker
    /// and returning an encoded `PlayerPositionSync` ready to frame and send.
    pub fn trigger_teleport(&mut self, target: (f64, f64, f64), tick: u64) -> PlayerPositionSync {
        let teleport_id = self.teleport_tracker.issue_teleport(target, tick);
        PlayerPositionSync::new_absolute(
            teleport_id,
            target.0,
            target.1,
            target.2,
            self.authoritative_rot.0,
            self.authoritative_rot.1,
        )
    }

    /// Handles client acceptance of a pending teleport (`AcceptTeleportation`).
    ///
    /// Confirms the teleport in the tracker and updates authoritative coordinates to target.
    pub fn handle_accept_teleport(
        &mut self,
        teleport_id: i32,
    ) -> Result<PendingTeleport, TeleportError> {
        let confirmed = self.teleport_tracker.accept_teleport(teleport_id)?;
        self.authoritative_pos = (confirmed.x, confirmed.y, confirmed.z);
        Ok(confirmed)
    }

    /// Creates an authoritative reset `PlayerPositionSync` packet to restore client coordinate.
    pub fn create_position_sync_reset(&mut self, tick: u64) -> PlayerPositionSync {
        self.trigger_teleport(self.authoritative_pos, tick)
    }

    /// Validates and applies a position-only update (`MovePlayerPos`).
    pub fn validate_and_apply_pos(
        &mut self,
        new_x: f64,
        new_y: f64,
        new_z: f64,
        on_ground: bool,
    ) -> Result<f64, MovementValidationError> {
        if self.teleport_tracker.has_pending_teleport() {
            let pending = self.teleport_tracker.pending_teleport().unwrap();
            return Err(MovementValidationError::TeleportPending {
                pending_id: pending.teleport_id,
                expected: (pending.x, pending.y, pending.z),
            });
        }

        let displacement = calculate_displacement(
            self.authoritative_pos.0,
            self.authoritative_pos.1,
            self.authoritative_pos.2,
            new_x,
            new_y,
            new_z,
        );

        if displacement > self.max_displacement {
            return Err(MovementValidationError::SpeedExceeded {
                displacement,
                max_allowed: self.max_displacement,
                reset_to: self.authoritative_pos,
            });
        }

        self.authoritative_pos = (new_x, new_y, new_z);
        self.authoritative_on_ground = on_ground;
        Ok(displacement)
    }

    /// Validates and applies a position and rotation update (`MovePlayerPosRot`).
    pub fn validate_and_apply_pos_rot(
        &mut self,
        new_x: f64,
        new_y: f64,
        new_z: f64,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    ) -> Result<f64, MovementValidationError> {
        if self.teleport_tracker.has_pending_teleport() {
            let pending = self.teleport_tracker.pending_teleport().unwrap();
            return Err(MovementValidationError::TeleportPending {
                pending_id: pending.teleport_id,
                expected: (pending.x, pending.y, pending.z),
            });
        }

        let displacement = calculate_displacement(
            self.authoritative_pos.0,
            self.authoritative_pos.1,
            self.authoritative_pos.2,
            new_x,
            new_y,
            new_z,
        );

        if displacement > self.max_displacement {
            return Err(MovementValidationError::SpeedExceeded {
                displacement,
                max_allowed: self.max_displacement,
                reset_to: self.authoritative_pos,
            });
        }

        self.authoritative_pos = (new_x, new_y, new_z);
        self.authoritative_rot = (yaw, pitch);
        self.authoritative_on_ground = on_ground;
        Ok(displacement)
    }

    /// Validates and applies a rotation-only update (`MovePlayerRot`).
    pub fn validate_and_apply_rot(
        &mut self,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    ) -> Result<(), MovementValidationError> {
        if self.teleport_tracker.has_pending_teleport() {
            let pending = self.teleport_tracker.pending_teleport().unwrap();
            return Err(MovementValidationError::TeleportPending {
                pending_id: pending.teleport_id,
                expected: (pending.x, pending.y, pending.z),
            });
        }

        self.authoritative_rot = (yaw, pitch);
        self.authoritative_on_ground = on_ground;
        Ok(())
    }

    /// Validates and applies a status-only update (`MovePlayerStatus`).
    pub fn validate_and_apply_status(
        &mut self,
        on_ground: bool,
    ) -> Result<(), MovementValidationError> {
        if self.teleport_tracker.has_pending_teleport() {
            let pending = self.teleport_tracker.pending_teleport().unwrap();
            return Err(MovementValidationError::TeleportPending {
                pending_id: pending.teleport_id,
                expected: (pending.x, pending.y, pending.z),
            });
        }

        self.authoritative_on_ground = on_ground;
        Ok(())
    }

    /// Dispatches a decoded serverbound movement packet through the validation guard.
    pub fn handle_movement_packet(
        &mut self,
        packet: &ServerboundMovementPacket,
    ) -> Result<MovementOutcome, MovementValidationError> {
        match packet {
            ServerboundMovementPacket::Pos(pos) => {
                let displacement =
                    self.validate_and_apply_pos(pos.x, pos.y, pos.z, pos.on_ground)?;
                Ok(MovementOutcome::PositionUpdated {
                    x: pos.x,
                    y: pos.y,
                    z: pos.z,
                    displacement,
                })
            }
            ServerboundMovementPacket::PosRot(pos_rot) => {
                let displacement = self.validate_and_apply_pos_rot(
                    pos_rot.x,
                    pos_rot.y,
                    pos_rot.z,
                    pos_rot.yaw,
                    pos_rot.pitch,
                    pos_rot.on_ground,
                )?;
                Ok(MovementOutcome::PositionAndRotationUpdated {
                    x: pos_rot.x,
                    y: pos_rot.y,
                    z: pos_rot.z,
                    yaw: pos_rot.yaw,
                    pitch: pos_rot.pitch,
                    displacement,
                })
            }
            ServerboundMovementPacket::Rot(rot) => {
                self.validate_and_apply_rot(rot.yaw, rot.pitch, rot.on_ground)?;
                Ok(MovementOutcome::RotationUpdated {
                    yaw: rot.yaw,
                    pitch: rot.pitch,
                })
            }
            ServerboundMovementPacket::Status(status) => {
                self.validate_and_apply_status(status.on_ground)?;
                Ok(MovementOutcome::StatusUpdated {
                    on_ground: status.on_ground,
                })
            }
            ServerboundMovementPacket::AcceptTeleport(accept) => {
                let confirmed = self.handle_accept_teleport(accept.teleport_id)?;
                Ok(MovementOutcome::TeleportConfirmed(confirmed))
            }
        }
    }

    /// Handles an incoming raw packet by ID and body payload.
    pub fn handle_incoming_frame(
        &mut self,
        packet_id: u32,
        body: &[u8],
    ) -> Result<MovementOutcome, MovementValidationError> {
        let packet = decode_serverbound_movement_packet(packet_id, body)?;
        self.handle_movement_packet(&packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_roundtrip() {
        for val in [
            0,
            1,
            127,
            128,
            255,
            256,
            16383,
            16384,
            2097151,
            -1,
            -2147483648,
            2147483647,
        ] {
            let mut buf = Vec::new();
            write_varint(val as u32, &mut buf);
            let (decoded, bytes) = read_varint(&buf).unwrap();
            assert_eq!(decoded, val);
            assert_eq!(bytes, buf.len());
        }

        // Non-canonical VarInt rejection (e.g. 0 encoded with continuation bit: [0x80, 0x00])
        assert_eq!(
            read_varint(&[0x80, 0x00]),
            Err(MovementError::MalformedVarInt)
        );

        // VarInt overflow (> 5 bytes)
        assert_eq!(
            read_varint(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
            Err(MovementError::MalformedVarInt)
        );
    }

    #[test]
    fn test_decode_move_player_pos_valid_and_malformed() {
        // Valid packet
        let pkt = MovePlayerPos::with_collision(12.5, 64.0, -100.25, true, true);
        let encoded = pkt.encode();
        assert_eq!(encoded.len(), 25);
        let decoded = MovePlayerPos::decode(&encoded).unwrap();
        assert_eq!(decoded, pkt);

        // Valid with horizontal collision flag = 3
        let mut data = encoded.clone();
        data[24] = 3;
        let decoded = MovePlayerPos::decode(&data).unwrap();
        assert!(decoded.on_ground);
        assert!(decoded.horizontal_collision);

        // Reject NaN coordinates
        let mut nan_x = encoded.clone();
        nan_x[0..8].copy_from_slice(&f64::NAN.to_be_bytes());
        assert_eq!(
            MovePlayerPos::decode(&nan_x),
            Err(MovementError::NonFiniteCoordinate)
        );

        let mut nan_y = encoded.clone();
        nan_y[8..16].copy_from_slice(&f64::NAN.to_be_bytes());
        assert_eq!(
            MovePlayerPos::decode(&nan_y),
            Err(MovementError::NonFiniteCoordinate)
        );

        let mut nan_z = encoded.clone();
        nan_z[16..24].copy_from_slice(&f64::NAN.to_be_bytes());
        assert_eq!(
            MovePlayerPos::decode(&nan_z),
            Err(MovementError::NonFiniteCoordinate)
        );

        // Reject Infinity coordinates
        let mut inf_x = encoded.clone();
        inf_x[0..8].copy_from_slice(&f64::INFINITY.to_be_bytes());
        assert_eq!(
            MovePlayerPos::decode(&inf_x),
            Err(MovementError::NonFiniteCoordinate)
        );

        let mut neg_inf_y = encoded.clone();
        neg_inf_y[8..16].copy_from_slice(&f64::NEG_INFINITY.to_be_bytes());
        assert_eq!(
            MovePlayerPos::decode(&neg_inf_y),
            Err(MovementError::NonFiniteCoordinate)
        );

        // Reject invalid length
        assert!(matches!(
            MovePlayerPos::decode(&encoded[0..24]),
            Err(MovementError::InvalidPacketLength {
                expected: 25,
                received: 24
            })
        ));

        // Reject invalid flag bits (> 3)
        let mut invalid_flag = encoded.clone();
        invalid_flag[24] = 4;
        assert_eq!(
            MovePlayerPos::decode(&invalid_flag),
            Err(MovementError::InvalidFlags(4))
        );
        invalid_flag[24] = 255;
        assert_eq!(
            MovePlayerPos::decode(&invalid_flag),
            Err(MovementError::InvalidFlags(255))
        );
    }

    #[test]
    fn test_decode_move_player_pos_rot_valid_normalization_and_malformed() {
        // Valid packet with yaw=450.0 (normalizes to 90.0) and pitch=120.0 (clamps to 90.0)
        let pkt = MovePlayerPosRot::new(0.0, 64.0, 0.0, 450.0, 120.0, false);
        let encoded = pkt.encode();
        assert_eq!(encoded.len(), 33);
        let decoded = MovePlayerPosRot::decode(&encoded).unwrap();
        assert_eq!(decoded.x, 0.0);
        assert_eq!(decoded.y, 64.0);
        assert_eq!(decoded.z, 0.0);
        assert!((decoded.yaw - 90.0).abs() < 1e-4);
        assert_eq!(decoded.pitch, 90.0);
        assert!(!decoded.on_ground);

        // Negative pitch clamped to -90.0
        let pkt_neg = MovePlayerPosRot::new(0.0, 64.0, 0.0, -190.0, -120.0, true);
        let decoded_neg = MovePlayerPosRot::decode(&pkt_neg.encode()).unwrap();
        assert!((decoded_neg.yaw - 170.0).abs() < 1e-4);
        assert_eq!(decoded_neg.pitch, -90.0);
        assert!(decoded_neg.on_ground);

        // Reject NaN coordinates and rotation
        let mut nan_buf = pkt.encode();
        nan_buf[0..8].copy_from_slice(&f64::NAN.to_be_bytes());
        assert_eq!(
            MovePlayerPosRot::decode(&nan_buf),
            Err(MovementError::NonFiniteCoordinate)
        );

        let mut nan_rot = pkt.encode();
        nan_rot[24..28].copy_from_slice(&f32::NAN.to_be_bytes());
        assert_eq!(
            MovePlayerPosRot::decode(&nan_rot),
            Err(MovementError::NonFiniteRotation)
        );

        let mut inf_rot = pkt.encode();
        inf_rot[28..32].copy_from_slice(&f32::INFINITY.to_be_bytes());
        assert_eq!(
            MovePlayerPosRot::decode(&inf_rot),
            Err(MovementError::NonFiniteRotation)
        );

        // Reject invalid packet length
        assert!(matches!(
            MovePlayerPosRot::decode(&pkt.encode()[0..32]),
            Err(MovementError::InvalidPacketLength {
                expected: 33,
                received: 32
            })
        ));

        // Reject invalid flag byte
        let mut invalid_flag = pkt.encode();
        invalid_flag[32] = 5;
        assert_eq!(
            MovePlayerPosRot::decode(&invalid_flag),
            Err(MovementError::InvalidFlags(5))
        );
    }

    #[test]
    fn test_decode_move_player_rot_valid_and_malformed() {
        let pkt = MovePlayerRot::new(180.0, -45.0, true);
        let encoded = pkt.encode();
        assert_eq!(encoded.len(), 9);
        let decoded = MovePlayerRot::decode(&encoded).unwrap();
        assert_eq!(decoded.yaw, -180.0);
        assert_eq!(decoded.pitch, -45.0);
        assert!(decoded.on_ground);

        // Reject NaN / Inf
        let mut nan_yaw = encoded.clone();
        nan_yaw[0..4].copy_from_slice(&f32::NAN.to_be_bytes());
        assert_eq!(
            MovePlayerRot::decode(&nan_yaw),
            Err(MovementError::NonFiniteRotation)
        );

        let mut inf_pitch = encoded.clone();
        inf_pitch[4..8].copy_from_slice(&f32::NEG_INFINITY.to_be_bytes());
        assert_eq!(
            MovePlayerRot::decode(&inf_pitch),
            Err(MovementError::NonFiniteRotation)
        );

        // Reject invalid length
        assert!(matches!(
            MovePlayerRot::decode(&encoded[0..8]),
            Err(MovementError::InvalidPacketLength {
                expected: 9,
                received: 8
            })
        ));

        // Reject invalid flags
        let mut inv_flag = encoded.clone();
        inv_flag[8] = 8;
        assert_eq!(
            MovePlayerRot::decode(&inv_flag),
            Err(MovementError::InvalidFlags(8))
        );
    }

    #[test]
    fn test_decode_move_player_status_valid_and_malformed() {
        for flag in [0, 1, 2, 3] {
            let decoded = MovePlayerStatus::decode(&[flag]).unwrap();
            assert_eq!(decoded.on_ground, (flag & 0x01) != 0);
            assert_eq!(decoded.horizontal_collision, (flag & 0x02) != 0);
        }

        // Reject length != 1
        assert!(matches!(
            MovePlayerStatus::decode(&[]),
            Err(MovementError::InvalidPacketLength {
                expected: 1,
                received: 0
            })
        ));
        assert!(matches!(
            MovePlayerStatus::decode(&[0, 1]),
            Err(MovementError::InvalidPacketLength {
                expected: 1,
                received: 2
            })
        ));

        // Reject invalid flags
        assert_eq!(
            MovePlayerStatus::decode(&[4]),
            Err(MovementError::InvalidFlags(4))
        );
        assert_eq!(
            MovePlayerStatus::decode(&[255]),
            Err(MovementError::InvalidFlags(255))
        );
    }

    #[test]
    fn test_decode_accept_teleportation_valid_and_malformed() {
        let pkt = AcceptTeleportation::new(42);
        let encoded = pkt.encode();
        let decoded = AcceptTeleportation::decode(&encoded).unwrap();
        assert_eq!(decoded.teleport_id, 42);

        // Large VarInt ID
        let pkt_large = AcceptTeleportation::new(1048576);
        let decoded_large = AcceptTeleportation::decode(&pkt_large.encode()).unwrap();
        assert_eq!(decoded_large.teleport_id, 1048576);

        // Reject trailing unparsed bytes
        let mut trailing = encoded.clone();
        trailing.push(0x00);
        assert_eq!(
            AcceptTeleportation::decode(&trailing),
            Err(MovementError::TrailingData)
        );
    }

    #[test]
    fn test_decode_serverbound_movement_packet_routing() {
        let pos = MovePlayerPos::new(10.0, 64.0, 20.0, true);
        let decoded =
            decode_serverbound_movement_packet(PACKET_ID_MOVE_PLAYER_POS, &pos.encode()).unwrap();
        assert_eq!(decoded, ServerboundMovementPacket::Pos(pos));

        let pos_rot = MovePlayerPosRot::new(1.0, 2.0, 3.0, 0.0, 0.0, false);
        let decoded =
            decode_serverbound_movement_packet(PACKET_ID_MOVE_PLAYER_POS_ROT, &pos_rot.encode())
                .unwrap();
        assert_eq!(decoded, ServerboundMovementPacket::PosRot(pos_rot));

        let rot = MovePlayerRot::new(45.0, 30.0, true);
        let decoded =
            decode_serverbound_movement_packet(PACKET_ID_MOVE_PLAYER_ROT, &rot.encode()).unwrap();
        assert_eq!(decoded, ServerboundMovementPacket::Rot(rot));

        let status = MovePlayerStatus::new(true);
        let decoded =
            decode_serverbound_movement_packet(PACKET_ID_MOVE_PLAYER_STATUS, &status.encode())
                .unwrap();
        assert_eq!(decoded, ServerboundMovementPacket::Status(status));

        let tp = AcceptTeleportation::new(5);
        let decoded =
            decode_serverbound_movement_packet(PACKET_ID_ACCEPT_TELEPORTATION, &tp.encode())
                .unwrap();
        assert_eq!(decoded, ServerboundMovementPacket::AcceptTeleport(tp));

        // Unknown packet ID
        assert_eq!(
            decode_serverbound_movement_packet(0xFF, &[]),
            Err(MovementError::UnknownPacketId(0xFF))
        );
    }

    #[test]
    fn test_player_position_sync_serialization_and_framing() {
        let sync = PlayerPositionSync::with_velocity(
            7,
            100.5,
            65.0,
            -200.75,
            0.1,
            -0.5,
            0.0,
            90.0,
            -15.0,
            position_sync_flags::RELATIVE_X | position_sync_flags::RELATIVE_Y_ROT,
        );

        // Verify framed round-trip
        let framed = sync.encode_framed();
        assert!(!framed.is_empty());
        let decoded = PlayerPositionSync::decode_framed(&framed).unwrap();
        assert_eq!(sync, decoded);

        // Verify body round-trip
        let mut body = Vec::new();
        sync.encode_body(&mut body);
        let decoded_body = PlayerPositionSync::decode_body(&body).unwrap();
        assert_eq!(sync, decoded_body);

        // Rejection of non-finite float in body
        let mut nan_body = body.clone();
        // offset to x is after VarInt (1 byte for 7)
        nan_body[1..9].copy_from_slice(&f64::NAN.to_be_bytes());
        assert_eq!(
            PlayerPositionSync::decode_body(&nan_body),
            Err(MovementError::NonFiniteFloat)
        );

        // Rejection of unexpected packet ID in frame
        let wrong_framed = frame_packet(0x3F, &body);
        assert_eq!(
            PlayerPositionSync::decode_framed(&wrong_framed),
            Err(MovementError::UnexpectedPacketId {
                expected: PACKET_ID_PLAYER_POSITION_SYNC,
                received: 0x3F,
            })
        );
    }

    #[test]
    fn test_teleport_tracker_handshake_and_challenge_sequence() {
        let mut tracker = TeleportTracker::new();
        assert!(!tracker.has_pending_teleport());
        assert_eq!(tracker.last_confirmed_id(), None);

        // Sequential IDs
        let id1 = tracker.next_teleport_id();
        let id2 = tracker.next_teleport_id();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);

        // Register teleport 3
        tracker.register_teleport(3, (50.0, 70.0, 50.0), 100);
        assert!(tracker.has_pending_teleport());
        assert_eq!(
            tracker.validate_movement_allowed(),
            Err(TeleportError::MovementWhilePending { pending_id: 3 })
        );

        // Rejection of unknown teleport ID (e.g. 99)
        assert_eq!(
            tracker.accept_teleport(99),
            Err(TeleportError::UnknownTeleportId { id: 99 })
        );
        assert!(tracker.has_pending_teleport());

        // Successful handshake confirmation
        let confirmed = tracker.accept_teleport(3).unwrap();
        assert_eq!(confirmed.teleport_id, 3);
        assert_eq!(confirmed.x, 50.0);
        assert_eq!(confirmed.y, 70.0);
        assert_eq!(confirmed.z, 50.0);
        assert_eq!(confirmed.tick, 100);

        assert!(!tracker.has_pending_teleport());
        assert_eq!(tracker.last_confirmed_id(), Some(3));
        assert!(tracker.validate_movement_allowed().is_ok());

        // Rejection of obsolete teleport ID (already confirmed ID 3)
        assert_eq!(
            tracker.accept_teleport(3),
            Err(TeleportError::ObsoleteTeleportId { id: 3, latest: 3 })
        );

        // Issue teleport 4 then teleport 5
        let id4 = tracker.issue_teleport((10.0, 64.0, 10.0), 105);
        let id5 = tracker.issue_teleport((20.0, 64.0, 20.0), 110);
        assert_eq!(id4, 3); // next_id continued from 3
        assert_eq!(id5, 4);

        // Attempting to confirm superseded id 3 when 4 is active is obsolete
        assert_eq!(
            tracker.accept_teleport(id4),
            Err(TeleportError::ObsoleteTeleportId {
                id: id4,
                latest: id5
            })
        );

        // Confirming latest id 4 clears all pending
        assert!(tracker.accept_teleport(id5).is_ok());
        assert!(!tracker.has_pending_teleport());
    }

    #[test]
    fn test_rejection_of_movement_packets_while_teleport_is_pending() {
        let mut guard = MovementGuard::new(0.0, 64.0, 0.0);

        // Normal movement allowed before teleport
        let pos_pkt = MovePlayerPos::new(1.0, 64.0, 0.0, true);
        assert!(
            guard
                .handle_movement_packet(&ServerboundMovementPacket::Pos(pos_pkt))
                .is_ok()
        );
        assert_eq!(guard.position(), (1.0, 64.0, 0.0));

        // Trigger server teleport to (500.0, 70.0, 500.0)
        let sync_pkt = guard.trigger_teleport((500.0, 70.0, 500.0), 200);
        assert_eq!(sync_pkt.x, 500.0);
        assert!(guard.teleport_tracker().has_pending_teleport());

        // Client movement updates must be rejected while teleport is awaiting confirmation
        let move_pos = MovePlayerPos::new(2.0, 64.0, 0.0, true);
        assert_eq!(
            guard.handle_movement_packet(&ServerboundMovementPacket::Pos(move_pos)),
            Err(MovementValidationError::TeleportPending {
                pending_id: sync_pkt.teleport_id,
                expected: (500.0, 70.0, 500.0),
            })
        );

        let move_pos_rot = MovePlayerPosRot::new(2.0, 64.0, 0.0, 0.0, 0.0, true);
        assert_eq!(
            guard.handle_movement_packet(&ServerboundMovementPacket::PosRot(move_pos_rot)),
            Err(MovementValidationError::TeleportPending {
                pending_id: sync_pkt.teleport_id,
                expected: (500.0, 70.0, 500.0),
            })
        );

        let move_rot = MovePlayerRot::new(90.0, 0.0, true);
        assert_eq!(
            guard.handle_movement_packet(&ServerboundMovementPacket::Rot(move_rot)),
            Err(MovementValidationError::TeleportPending {
                pending_id: sync_pkt.teleport_id,
                expected: (500.0, 70.0, 500.0),
            })
        );

        let move_status = MovePlayerStatus::new(true);
        assert_eq!(
            guard.handle_movement_packet(&ServerboundMovementPacket::Status(move_status)),
            Err(MovementValidationError::TeleportPending {
                pending_id: sync_pkt.teleport_id,
                expected: (500.0, 70.0, 500.0),
            })
        );

        // Confirm teleport
        let accept = AcceptTeleportation::new(sync_pkt.teleport_id);
        let outcome = guard
            .handle_movement_packet(&ServerboundMovementPacket::AcceptTeleport(accept))
            .unwrap();
        assert!(matches!(outcome, MovementOutcome::TeleportConfirmed(_)));
        assert_eq!(guard.position(), (500.0, 70.0, 500.0));

        // Movement updates allowed again after confirmation
        let move_after = MovePlayerPos::new(500.5, 70.0, 500.0, true);
        assert!(
            guard
                .handle_movement_packet(&ServerboundMovementPacket::Pos(move_after))
                .is_ok()
        );
        assert_eq!(guard.position(), (500.5, 70.0, 500.0));
    }

    #[test]
    fn test_enforcement_of_maximum_displacement_threshold() {
        let mut guard = MovementGuard::new(0.0, 64.0, 0.0);

        // Movement within single-tick threshold (e.g. 10.0 blocks <= 100.0)
        let valid_move = MovePlayerPos::new(10.0, 64.0, 0.0, true);
        let outcome = guard
            .handle_movement_packet(&ServerboundMovementPacket::Pos(valid_move))
            .unwrap();
        assert_eq!(
            outcome,
            MovementOutcome::PositionUpdated {
                x: 10.0,
                y: 64.0,
                z: 0.0,
                displacement: 10.0,
            }
        );
        assert_eq!(guard.position(), (10.0, 64.0, 0.0));

        // Movement exceeding single-tick threshold (> 100.0 blocks, e.g. 10.0 -> 120.0 = 110 blocks)
        let illegal_move = MovePlayerPos::new(120.0, 64.0, 0.0, true);
        let err = guard
            .handle_movement_packet(&ServerboundMovementPacket::Pos(illegal_move))
            .unwrap_err();

        match err {
            MovementValidationError::SpeedExceeded {
                displacement,
                max_allowed,
                reset_to,
            } => {
                assert!((displacement - 110.0).abs() < 1e-4);
                assert_eq!(max_allowed, 100.0);
                assert_eq!(reset_to, (10.0, 64.0, 0.0));
            }
            other => panic!("expected SpeedExceeded, got {other:?}"),
        }

        // Authoritative position must remain at the pre-violation coordinate
        assert_eq!(guard.position(), (10.0, 64.0, 0.0));

        // Server can generate an authoritative reset packet
        let reset_sync = guard.create_position_sync_reset(300);
        assert_eq!(reset_sync.x, 10.0);
        assert_eq!(reset_sync.y, 64.0);
        assert_eq!(reset_sync.z, 0.0);

        // Client acknowledges the reset teleport
        guard
            .handle_movement_packet(&ServerboundMovementPacket::AcceptTeleport(
                AcceptTeleportation::new(reset_sync.teleport_id),
            ))
            .unwrap();
        assert_eq!(guard.position(), (10.0, 64.0, 0.0));
    }
}
