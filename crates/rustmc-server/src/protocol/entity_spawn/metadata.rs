//! Entity metadata values and the Set Entity Data packet codec.

use super::*;

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
pub(super) fn write_text_component(
    value: &str,
    out: &mut Vec<u8>,
) -> Result<(), EntityProtocolError> {
    put_nbt_text(value, out).map_err(|err| match err {
        WireError::TooLong { length, max } => EntityProtocolError::StringTooLong { length, max },
        _ => EntityProtocolError::InvalidUtf8,
    })
}

/// Reads an anonymous TAG_String text component, returning the bytes consumed.
pub(super) fn read_text_component(data: &[u8]) -> Result<(String, usize), EntityProtocolError> {
    let (tag, rest) = data
        .split_first()
        .ok_or(EntityProtocolError::UnexpectedEof)?;
    if *tag != NBT_TAG_STRING {
        return Err(EntityProtocolError::UnsupportedTextComponent(*tag));
    }
    let (text, consumed) = java_wire::read_nbt_string(rest).map_err(|err| match err {
        WireError::Truncated => EntityProtocolError::UnexpectedEof,
        _ => EntityProtocolError::InvalidUtf8,
    })?;
    Ok((text, 1 + consumed))
}

pub(super) fn read_bool(data: &[u8]) -> Result<bool, EntityProtocolError> {
    match data.first() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        Some(other) => Err(EntityProtocolError::InvalidBoolean(*other)),
        None => Err(EntityProtocolError::UnexpectedEof),
    }
}

pub(super) fn read_f32(data: &[u8]) -> Result<f32, EntityProtocolError> {
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
