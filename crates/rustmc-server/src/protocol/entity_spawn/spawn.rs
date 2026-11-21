//! Spawn Entity and Remove Entities packet codecs.

use super::*;

/// Core placement and identity fields shared by [`SpawnEntityPacket`] and [`TrackedEntityState`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntitySpawnFields {
    pub entity_id: i32,
    pub uuid: [u8; 16],
    pub entity_type: i32,
    pub position: [f64; 3],
    pub pitch: u8,
    pub yaw: u8,
    pub head_yaw: u8,
    pub data: i32,
}

/// Entity spawn fields taking angles in degrees for [`SpawnEntityPacket::with_angles`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntitySpawnAngles {
    pub entity_id: i32,
    pub uuid: [u8; 16],
    pub entity_type: i32,
    pub position: [f64; 3],
    pub pitch_deg: f64,
    pub yaw_deg: f64,
    pub head_yaw_deg: f64,
    pub data: i32,
}

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
    pub fn new(fields: EntitySpawnFields) -> Result<Self, EntityProtocolError> {
        validate_coordinates(fields.position)?;
        Ok(Self {
            entity_id: fields.entity_id,
            uuid: fields.uuid,
            entity_type: fields.entity_type,
            position: fields.position,
            pitch: fields.pitch,
            yaw: fields.yaw,
            head_yaw: fields.head_yaw,
            data: fields.data,
        })
    }

    /// Creates a new `SpawnEntityPacket` at rest, converting angles in degrees.
    pub fn with_angles(fields: EntitySpawnAngles) -> Result<Self, EntityProtocolError> {
        Self::new(EntitySpawnFields {
            entity_id: fields.entity_id,
            uuid: fields.uuid,
            entity_type: fields.entity_type,
            position: fields.position,
            pitch: angle_to_byte(fields.pitch_deg),
            yaw: angle_to_byte(fields.yaw_deg),
            head_yaw: angle_to_byte(fields.head_yaw_deg),
            data: fields.data,
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

        Self::new(EntitySpawnFields {
            entity_id,
            uuid,
            entity_type,
            position: [x, y, z],
            pitch,
            yaw,
            head_yaw,
            data: data_val,
        })
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
