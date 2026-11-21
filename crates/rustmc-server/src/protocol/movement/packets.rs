//! Serverbound movement packet codecs (position, rotation, status,
//! teleport confirmation).

use super::*;

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
