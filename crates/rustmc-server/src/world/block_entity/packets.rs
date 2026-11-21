//! Block Entity Data and Update Sign packet codecs.

use super::*;

/// Java 26.3 Clientbound `BlockEntityDataPacket` (`0x06`).
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntityDataPacket {
    pub pos: BlockPos,
    pub block_entity_type: u32,
    pub nbt: NbtTag,
}

impl BlockEntityDataPacket {
    pub fn new(pos: BlockPos, block_entity_type: u32, nbt: NbtTag) -> Self {
        Self {
            pos,
            block_entity_type,
            nbt,
        }
    }

    pub fn from_sign(pos: BlockPos, sign: &SignBlockEntity) -> Self {
        Self {
            pos,
            block_entity_type: TYPE_ID_SIGN,
            nbt: sign.to_nbt(),
        }
    }

    /// Encodes the packet payload (without frame length/id prefix).
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&self.pos.pack().to_be_bytes());
        put_varint(self.block_entity_type, &mut body);
        write_network_nbt(&self.nbt, &mut body);
        body
    }

    /// Encodes a framed packet ready for wire transmission.
    pub fn frame(&self) -> Vec<u8> {
        frame(PACKET_ID_BLOCK_ENTITY_DATA, &self.encode())
    }

    /// Decodes from the packet payload (unframed).
    pub fn decode(input: &[u8]) -> Result<Self, BlockEntityError> {
        let (pos_raw, mut input) =
            java_wire::read_i64_be(input).map_err(|_| BlockEntityError::UnexpectedEof)?;
        let pos = BlockPos::unpack(pos_raw);

        let (type_id, varint_len) = read_varint(input)?;
        input = &input[varint_len..];

        let nbt = read_network_nbt(input)?;

        Ok(Self {
            pos,
            block_entity_type: type_id,
            nbt,
        })
    }

    /// Decodes from a framed packet (skipping frame length and packet ID `0x06`).
    pub fn decode_framed(input: &[u8]) -> Result<Self, BlockEntityError> {
        let (frame_len, len_bytes) = read_varint(input)?;
        let frame_payload = &input[len_bytes..];
        if frame_payload.len() != frame_len as usize {
            return Err(BlockEntityError::Malformed("frame length mismatch"));
        }
        let (packet_id, id_bytes) = read_varint(frame_payload)?;
        if packet_id != PACKET_ID_BLOCK_ENTITY_DATA {
            return Err(BlockEntityError::MismatchedType {
                expected: PACKET_ID_BLOCK_ENTITY_DATA,
                got: packet_id,
            });
        }
        Self::decode(&frame_payload[id_bytes..])
    }

    /// Converts the NBT payload into a `SignBlockEntity` if this is a sign packet.
    pub fn to_sign(&self) -> Result<SignBlockEntity, BlockEntityError> {
        if self.block_entity_type != TYPE_ID_SIGN {
            return Err(BlockEntityError::MismatchedType {
                expected: TYPE_ID_SIGN,
                got: self.block_entity_type,
            });
        }
        SignBlockEntity::from_nbt(&self.nbt)
    }
}

/// Java 26.3 Serverbound `UpdateSignPacket` (`0x3E`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateSignPacket {
    pub pos: BlockPos,
    pub is_front_text: bool,
    pub lines: [String; LINES_PER_SIDE],
}

impl UpdateSignPacket {
    pub fn new(
        pos: BlockPos,
        is_front_text: bool,
        lines: [String; LINES_PER_SIDE],
    ) -> Result<Self, BlockEntityError> {
        if !pos.is_valid() {
            return Err(BlockEntityError::InvalidPosition(pos));
        }
        for line in &lines {
            if line.len() > MAX_LINE_BYTES {
                return Err(BlockEntityError::LineTooLong(line.len()));
            }
        }
        Ok(Self {
            pos,
            is_front_text,
            lines,
        })
    }

    /// Encodes the packet payload (unframed).
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&self.pos.pack().to_be_bytes());
        body.push(if self.is_front_text { 1 } else { 0 });
        for line in &self.lines {
            put_string(line, &mut body);
        }
        body
    }

    /// Encodes a framed packet ready for wire transmission.
    pub fn frame(&self) -> Vec<u8> {
        frame(PACKET_ID_UPDATE_SIGN, &self.encode())
    }

    /// Decodes and validates a serverbound `UpdateSignPacket` payload.
    pub fn decode(input: &[u8]) -> Result<Self, BlockEntityError> {
        let (pos_raw, rest) =
            java_wire::read_i64_be(input).map_err(|_| BlockEntityError::UnexpectedEof)?;
        let (&front_byte, mut input) = rest.split_first().ok_or(BlockEntityError::UnexpectedEof)?;
        let pos = BlockPos::unpack(pos_raw);
        if !pos.is_valid() {
            return Err(BlockEntityError::InvalidPosition(pos));
        }

        if front_byte > 1 {
            return Err(BlockEntityError::Malformed("is_front_text must be 0 or 1"));
        }
        let is_front_text = front_byte == 1;

        let mut lines = [String::new(), String::new(), String::new(), String::new()];
        for line in &mut lines {
            let (text, rest) =
                java_wire::read_string(input, MAX_LINE_BYTES).map_err(map_wire_error)?;
            *line = text.to_string();
            input = rest;
        }

        if !input.is_empty() {
            return Err(BlockEntityError::Malformed(
                "trailing bytes in UpdateSignPacket",
            ));
        }

        Ok(Self {
            pos,
            is_front_text,
            lines,
        })
    }

    /// Decodes from a framed packet (skipping frame length and packet ID `0x3E`).
    pub fn decode_framed(input: &[u8]) -> Result<Self, BlockEntityError> {
        let (frame_len, len_bytes) = read_varint(input)?;
        let frame_payload = &input[len_bytes..];
        if frame_payload.len() != frame_len as usize {
            return Err(BlockEntityError::Malformed("frame length mismatch"));
        }
        let (packet_id, id_bytes) = read_varint(frame_payload)?;
        if packet_id != PACKET_ID_UPDATE_SIGN {
            return Err(BlockEntityError::MismatchedType {
                expected: PACKET_ID_UPDATE_SIGN,
                got: packet_id,
            });
        }
        Self::decode(&frame_payload[id_bytes..])
    }

    /// Applies this update packet to a target `SignBlockEntity`.
    /// Rejects if the sign is waxed.
    pub fn apply_to(&self, sign: &mut SignBlockEntity) -> Result<(), SignError> {
        sign.set_lines(self.is_front_text, self.lines.clone())
    }
}

/// Block entity and packet serialization errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockEntityError {
    UnexpectedEof,
    Malformed(&'static str),
    LineTooLong(usize),
    InvalidPosition(BlockPos),
    MissingField(&'static str),
    MismatchedType { expected: u32, got: u32 },
    Sign(SignError),
}

impl fmt::Display for BlockEntityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected end of packet data"),
            Self::Malformed(msg) => write!(f, "malformed packet data: {msg}"),
            Self::LineTooLong(len) => {
                write!(
                    f,
                    "sign line length {len} exceeds maximum {MAX_LINE_BYTES} bytes"
                )
            }
            Self::InvalidPosition(pos) => {
                write!(
                    f,
                    "invalid block position ({}, {}, {}) out of world bounds",
                    pos.x, pos.y, pos.z
                )
            }
            Self::MissingField(field) => write!(f, "missing required field: {field}"),
            Self::MismatchedType { expected, got } => {
                write!(f, "mismatched type ID: expected {expected}, got {got}")
            }
            Self::Sign(err) => write!(f, "sign error: {err}"),
        }
    }
}

impl std::error::Error for BlockEntityError {}

impl From<SignError> for BlockEntityError {
    fn from(err: SignError) -> Self {
        Self::Sign(err)
    }
}
