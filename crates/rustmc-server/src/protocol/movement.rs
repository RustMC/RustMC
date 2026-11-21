//! Java Edition 26.3 player movement packet decoders, teleport confirmation tracker,
//! and movement speed/displacement validation guards.
//!
//! # Protocol Specification
//!
//! Under Minecraft Java Edition 26.3 (protocol 777), player movement updates are sent by
//! the client in Play state across four movement packet variants and one teleport confirmation packet:
//!
//! - `MovePlayerPos` (0x1E): `x: f64, y: f64, z: f64, on_ground: bool`
//! - `MovePlayerPosRot` (0x1F): `x: f64, y: f64, z: f64, yaw: f32, pitch: f32, on_ground: bool`
//! - `MovePlayerRot` (0x20): `yaw: f32, pitch: f32, on_ground: bool`
//! - `MovePlayerStatus` (0x21): `on_ground: bool`
//! - `AcceptTeleportation` (0x00): `teleport_id: VarInt`, then the confirmed `x, y, z: f64` and `yaw, pitch: f32`
//!
//! The server coordinates player teleportation with clientbound `PlayerPositionSync` (0x49):
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

use std::collections::{BTreeMap, btree_map::Entry};
use std::fmt;

use crate::java_wire::{self, frame, put_varint};

/// Serverbound packet ID for Java 26.3 AcceptTeleportation (Play state).
pub const PACKET_ID_ACCEPT_TELEPORTATION: u32 = 0x00;

/// Serverbound packet ID for Java 26.3 MovePlayerPos (Play state 30 / 0x1E).
pub const PACKET_ID_MOVE_PLAYER_POS: u32 = 0x1E;

/// Serverbound packet ID for Java 26.3 MovePlayerPosRot (Play state 31 / 0x1F).
pub const PACKET_ID_MOVE_PLAYER_POS_ROT: u32 = 0x1F;

/// Serverbound packet ID for Java 26.3 MovePlayerRot (Play state 32 / 0x20).
pub const PACKET_ID_MOVE_PLAYER_ROT: u32 = 0x20;

/// Serverbound packet ID for Java 26.3 MovePlayerStatus (Play state 33 / 0x21).
pub const PACKET_ID_MOVE_PLAYER_STATUS: u32 = 0x21;

/// Clientbound packet ID for Java 26.3 PlayerPositionSync (Play state 73 / 0x49).
pub const PACKET_ID_PLAYER_POSITION_SYNC: u32 = 0x49;

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
    NonFiniteCoordinate,
    NonFiniteRotation,
}

impl fmt::Display for MovementValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteCoordinate => write!(f, "coordinate component is not finite (NaN/Inf)"),
            Self::NonFiniteRotation => write!(f, "rotation component is not finite (NaN/Inf)"),
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
    match java_wire::read_varint(input) {
        Ok(Some((value, bytes_read))) => Ok((value as i32, bytes_read)),
        Ok(None) => Err(MovementError::UnexpectedEof),
        Err(_) => Err(MovementError::MalformedVarInt),
    }
}

#[inline]
fn read_f64(input: &[u8]) -> Result<(f64, &[u8]), MovementError> {
    java_wire::read_f64_be(input).map_err(|_| MovementError::UnexpectedEof)
}

#[inline]
fn read_f32(input: &[u8]) -> Result<(f32, &[u8]), MovementError> {
    java_wire::read_f32_be(input).map_err(|_| MovementError::UnexpectedEof)
}

#[inline]
fn read_i32(input: &[u8]) -> Result<(i32, &[u8]), MovementError> {
    java_wire::read_i32_be(input).map_err(|_| MovementError::UnexpectedEof)
}

/// Writes a VarInt to a byte buffer.
pub fn write_varint(value: u32, out: &mut Vec<u8>) {
    put_varint(value, out);
}

/// Frames a packet with its packet ID and outer length VarInt.
pub fn frame_packet(packet_id: u32, body: &[u8]) -> Vec<u8> {
    frame(packet_id, body)
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

/// Serverbound `MovePlayerPos` packet (0x1E): `x: f64, y: f64, z: f64, on_ground: bool`.
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
        let (x, rest) = read_f64(data)?;
        let (y, rest) = read_f64(rest)?;
        let (z, rest) = read_f64(rest)?;
        validate_coords(x, y, z)?;
        let &[flags] = rest else {
            return Err(MovementError::UnexpectedEof);
        };
        let (on_ground, horizontal_collision) = parse_flags_byte(flags)?;

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

    /// Encodes a framed packet (length VarInt + packet ID 0x1E + payload).
    pub fn encode_framed(&self) -> Vec<u8> {
        frame_packet(Self::PACKET_ID, &self.encode())
    }
}

/// Serverbound `MovePlayerPosRot` packet (0x1F):
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
        let (x, rest) = read_f64(data)?;
        let (y, rest) = read_f64(rest)?;
        let (z, rest) = read_f64(rest)?;
        validate_coords(x, y, z)?;

        let (raw_yaw, rest) = read_f32(rest)?;
        let (raw_pitch, rest) = read_f32(rest)?;
        validate_rotation(raw_yaw, raw_pitch)?;

        let yaw = normalize_yaw(raw_yaw);
        let pitch = normalize_pitch(raw_pitch);
        let &[flags] = rest else {
            return Err(MovementError::UnexpectedEof);
        };
        let (on_ground, horizontal_collision) = parse_flags_byte(flags)?;

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

    /// Encodes a framed packet (length VarInt + packet ID 0x1F + payload).
    pub fn encode_framed(&self) -> Vec<u8> {
        frame_packet(Self::PACKET_ID, &self.encode())
    }
}

/// Serverbound `MovePlayerRot` packet (0x20): `yaw: f32, pitch: f32, on_ground: bool`.
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
        let (raw_yaw, rest) = read_f32(data)?;
        let (raw_pitch, rest) = read_f32(rest)?;
        validate_rotation(raw_yaw, raw_pitch)?;

        let yaw = normalize_yaw(raw_yaw);
        let pitch = normalize_pitch(raw_pitch);
        let &[flags] = rest else {
            return Err(MovementError::UnexpectedEof);
        };
        let (on_ground, horizontal_collision) = parse_flags_byte(flags)?;

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

    /// Encodes a framed packet (length VarInt + packet ID 0x20 + payload).
    pub fn encode_framed(&self) -> Vec<u8> {
        frame_packet(Self::PACKET_ID, &self.encode())
    }
}

/// Serverbound `MovePlayerStatus` packet (0x21): `on_ground: bool`.
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

    /// Encodes a framed packet (length VarInt + packet ID 0x21 + payload).
    pub fn encode_framed(&self) -> Vec<u8> {
        frame_packet(Self::PACKET_ID, &self.encode())
    }
}

/// Serverbound `AcceptTeleportation` packet (0x00): `teleport_id: i32` (VarInt).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AcceptTeleportation {
    pub teleport_id: i32,
    pub target_pos: Option<[f64; 3]>,
    pub target_rot: Option<[f32; 2]>,
}

impl AcceptTeleportation {
    pub const PACKET_ID: u32 = PACKET_ID_ACCEPT_TELEPORTATION;

    #[inline]
    pub const fn new(teleport_id: i32) -> Self {
        Self {
            teleport_id,
            target_pos: None,
            target_rot: None,
        }
    }

    #[inline]
    pub const fn with_confirmation(
        teleport_id: i32,
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
    ) -> Self {
        Self {
            teleport_id,
            target_pos: Some([x, y, z]),
            target_rot: Some([yaw, pitch]),
        }
    }

    /// Decodes the payload bytes for `AcceptTeleportation`.
    ///
    /// Accepts either VarInt alone (teleport ID) or the complete 26.3 client confirmation
    /// containing VarInt teleport ID followed by 24 bytes (3x f64 coords) and 8 bytes (2x f32 rotation).
    pub fn decode(data: &[u8]) -> Result<Self, MovementError> {
        let (teleport_id, bytes_read) = read_varint(data)?;
        let remainder = &data[bytes_read..];
        if remainder.is_empty() {
            return Ok(Self {
                teleport_id,
                target_pos: None,
                target_rot: None,
            });
        }
        if remainder.len() == 32 {
            let (x, rest) = read_f64(remainder)?;
            let (y, rest) = read_f64(rest)?;
            let (z, rest) = read_f64(rest)?;
            let (yaw, rest) = read_f32(rest)?;
            let (pitch, _) = read_f32(rest)?;
            if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                return Err(MovementError::NonFiniteCoordinate);
            }
            if !yaw.is_finite() || !pitch.is_finite() {
                return Err(MovementError::NonFiniteRotation);
            }
            return Ok(Self {
                teleport_id,
                target_pos: Some([x, y, z]),
                target_rot: Some([yaw, pitch]),
            });
        }
        Err(MovementError::TrailingData)
    }

    /// Encodes the packet payload to bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(37);
        write_varint(self.teleport_id as u32, &mut out);
        if let Some([x, y, z]) = self.target_pos {
            out.extend_from_slice(&x.to_be_bytes());
            out.extend_from_slice(&y.to_be_bytes());
            out.extend_from_slice(&z.to_be_bytes());
            if let Some([yaw, pitch]) = self.target_rot {
                out.extend_from_slice(&yaw.to_be_bytes());
                out.extend_from_slice(&pitch.to_be_bytes());
            }
        }
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

/// Clientbound `PlayerPositionSync` packet (0x49 in Java 26.3):
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

/// Full position, velocity, orientation, and flag fields for [`PlayerPositionSync::with_velocity`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionSyncParams {
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
    pub const fn with_velocity(params: PositionSyncParams) -> Self {
        Self {
            teleport_id: params.teleport_id,
            x: params.x,
            y: params.y,
            z: params.z,
            dx: params.dx,
            dy: params.dy,
            dz: params.dz,
            yaw: params.yaw,
            pitch: params.pitch,
            flags: params.flags,
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

    /// Encodes packet ID 0x49 plus the body bytes (unframed).
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(70);
        write_varint(Self::PACKET_ID, &mut packet);
        self.encode_body(&mut packet);
        packet
    }

    /// Encodes an outer-framed packet (length VarInt + packet ID 0x49 + payload).
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

        let (x, rest) = read_f64(remaining)?;
        let (y, rest) = read_f64(rest)?;
        let (z, rest) = read_f64(rest)?;
        let (dx, rest) = read_f64(rest)?;
        let (dy, rest) = read_f64(rest)?;
        let (dz, rest) = read_f64(rest)?;
        let (yaw, rest) = read_f32(rest)?;
        let (pitch, rest) = read_f32(rest)?;
        let (flags, _) = read_i32(rest)?;

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

        if self.pending.is_empty() {
            return Err(TeleportError::NoPendingTeleport);
        }

        let latest_id = self.latest_issued_id.unwrap_or(teleport_id);
        let Entry::Occupied(entry) = self.pending.entry(teleport_id) else {
            return Err(TeleportError::UnknownTeleportId { id: teleport_id });
        };

        if teleport_id < latest_id {
            return Err(TeleportError::ObsoleteTeleportId {
                id: teleport_id,
                latest: latest_id,
            });
        }

        let confirmed = entry.remove();
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
    tick_baseline_pos: (f64, f64, f64),
    accumulated_tick_displacement: f64,
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
            tick_baseline_pos: (x, y, z),
            accumulated_tick_displacement: 0.0,
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
            tick_baseline_pos: (x, y, z),
            accumulated_tick_displacement: 0.0,
            authoritative_rot: (normalize_yaw(yaw), normalize_pitch(pitch)),
            authoritative_on_ground: true,
            teleport_tracker: TeleportTracker::new(),
            max_displacement: MAX_DISPLACEMENT_PER_TICK,
        }
    }

    /// Advances to the next server tick, resetting accumulated per-tick displacement.
    pub fn advance_tick(&mut self) {
        self.tick_baseline_pos = self.authoritative_pos;
        self.accumulated_tick_displacement = 0.0;
    }

    /// Returns accumulated displacement in the current server tick.
    #[inline]
    pub const fn accumulated_tick_displacement(&self) -> f64 {
        self.accumulated_tick_displacement
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
        self.tick_baseline_pos = self.authoritative_pos;
        self.accumulated_tick_displacement = 0.0;
        Ok(confirmed)
    }

    /// Creates an authoritative reset `PlayerPositionSync` packet to restore client coordinate.
    pub fn create_position_sync_reset(&mut self, tick: u64) -> PlayerPositionSync {
        self.trigger_teleport(self.authoritative_pos, tick)
    }

    fn reject_if_teleport_pending(&self) -> Result<(), MovementValidationError> {
        if let Some(pending) = self.teleport_tracker.pending_teleport() {
            return Err(MovementValidationError::TeleportPending {
                pending_id: pending.teleport_id,
                expected: (pending.x, pending.y, pending.z),
            });
        }
        Ok(())
    }

    /// Validates and applies a position-only update (`MovePlayerPos`).
    pub fn validate_and_apply_pos(
        &mut self,
        new_x: f64,
        new_y: f64,
        new_z: f64,
        on_ground: bool,
    ) -> Result<f64, MovementValidationError> {
        if !new_x.is_finite() || !new_y.is_finite() || !new_z.is_finite() {
            return Err(MovementValidationError::NonFiniteCoordinate);
        }

        self.reject_if_teleport_pending()?;

        let step_displacement = calculate_displacement(
            self.authoritative_pos.0,
            self.authoritative_pos.1,
            self.authoritative_pos.2,
            new_x,
            new_y,
            new_z,
        );

        let new_accumulated = self.accumulated_tick_displacement + step_displacement;
        if new_accumulated > self.max_displacement {
            return Err(MovementValidationError::SpeedExceeded {
                displacement: new_accumulated,
                max_allowed: self.max_displacement,
                reset_to: self.authoritative_pos,
            });
        }

        self.accumulated_tick_displacement = new_accumulated;
        self.authoritative_pos = (new_x, new_y, new_z);
        self.authoritative_on_ground = on_ground;
        Ok(step_displacement)
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
        if !new_x.is_finite() || !new_y.is_finite() || !new_z.is_finite() {
            return Err(MovementValidationError::NonFiniteCoordinate);
        }
        if !yaw.is_finite() || !pitch.is_finite() {
            return Err(MovementValidationError::NonFiniteRotation);
        }

        self.reject_if_teleport_pending()?;

        let step_displacement = calculate_displacement(
            self.authoritative_pos.0,
            self.authoritative_pos.1,
            self.authoritative_pos.2,
            new_x,
            new_y,
            new_z,
        );

        let new_accumulated = self.accumulated_tick_displacement + step_displacement;
        if new_accumulated > self.max_displacement {
            return Err(MovementValidationError::SpeedExceeded {
                displacement: new_accumulated,
                max_allowed: self.max_displacement,
                reset_to: self.authoritative_pos,
            });
        }

        self.accumulated_tick_displacement = new_accumulated;
        self.authoritative_pos = (new_x, new_y, new_z);
        self.authoritative_rot = (yaw, pitch);
        self.authoritative_on_ground = on_ground;
        Ok(step_displacement)
    }

    /// Validates and applies a rotation-only update (`MovePlayerRot`).
    pub fn validate_and_apply_rot(
        &mut self,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    ) -> Result<(), MovementValidationError> {
        if !yaw.is_finite() || !pitch.is_finite() {
            return Err(MovementValidationError::NonFiniteRotation);
        }

        self.reject_if_teleport_pending()?;

        self.authoritative_rot = (yaw, pitch);
        self.authoritative_on_ground = on_ground;
        Ok(())
    }

    /// Validates and applies a status-only update (`MovePlayerStatus`).
    pub fn validate_and_apply_status(
        &mut self,
        on_ground: bool,
    ) -> Result<(), MovementValidationError> {
        self.reject_if_teleport_pending()?;

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
mod tests;
