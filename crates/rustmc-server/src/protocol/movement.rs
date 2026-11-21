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

mod packets;
pub use packets::*;
mod tracking;
pub use tracking::*;

#[cfg(test)]
mod tests;
