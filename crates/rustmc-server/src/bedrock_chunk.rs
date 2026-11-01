//! Minecraft Bedrock Edition 1.26.51 LevelChunkPacket framing and SubChunk palette serialization.
//!
//! Under Milestone M3, Bedrock clients require chunk columns to be transmitted as
//! [`LevelChunkPacket`]s (ID `0x3A`). Each chunk column contains a stack of vertical
//! 16x16x16 subchunks (standard 24 subchunks covering Overworld Y = -64 to 319) followed
//! by 3D biomes, border blocks, and optional block entities.
//!
//! Subchunks support versions 8 and 9 with paletted block storages packing 4,096 block
//! indices per storage layer into 32-bit little-endian words.

use std::fmt;

/// Packet identifier for Bedrock `LevelChunkPacket` (58 / 0x3A).
pub const LEVEL_CHUNK_PACKET_ID: u32 = 0x3a;

/// Bedrock SubChunk format version 8.
pub const SUBCHUNK_VERSION_8: u8 = 8;

/// Bedrock SubChunk format version 9 (includes vertical subchunk Y index).
pub const SUBCHUNK_VERSION_9: u8 = 9;

/// Number of blocks in a 16x16x16 subchunk (4,096).
pub const BLOCKS_PER_SUBCHUNK: usize = 16 * 16 * 16;

/// Standard number of subchunks in an Overworld chunk column (Y = -64 to 319).
pub const OVERWORLD_SUBCHUNK_COUNT: u32 = 24;

/// Maximum allowable subchunk count for an Overworld chunk column (bounds guard).
pub const MAX_SUBCHUNKS: u32 = 32;

/// Maximum allowable uncompressed serialized chunk payload size (1 MiB).
pub const MAX_SERIALIZED_CHUNK_SIZE: usize = 1024 * 1024;

/// Errors arising during Bedrock chunk serialization, framing, or decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BedrockChunkError {
    /// Packet identifier did not match [`LEVEL_CHUNK_PACKET_ID`].
    InvalidPacketId(u32),
    /// VarInt or VarUint byte sequence is malformed, truncated, or exceeds 32 bits.
    MalformedVarInt,
    /// Subchunk count was zero or exceeded [`MAX_SUBCHUNKS`].
    InvalidSubChunkCount(u32),
    /// Serialized chunk payload exceeded [`MAX_SERIALIZED_CHUNK_SIZE`].
    PayloadTooLarge { size: usize, limit: usize },
    /// Input buffer was shorter than expected.
    TruncatedPacket { expected: usize, available: usize },
    /// Words buffer was shorter than expected for the declared bits per block.
    TruncatedWords { expected: usize, available: usize },
    /// SubChunk format version is not supported (only 8 and 9 are supported).
    UnsupportedSubChunkVersion(u8),
    /// Bits-per-block value is not supported by Bedrock palette format.
    UnsupportedBitsPerBlock(u8),
    /// Palette cannot be empty.
    EmptyPalette,
    /// Palette entries count exceeds subchunk block capacity (4,096).
    PaletteTooLarge(usize),
    /// A block index exceeded the capacity of the current bits-per-block representation.
    IndexExceedsBitsPerBlock { index: u32, bits_per_block: u8 },
    /// Subchunk block index was out of bounds (must be < 4,096).
    IndexOutOfBounds(usize),
    /// Local coordinates within subchunk were out of bounds (0..16).
    CoordinatesOutOfBounds { x: usize, y: usize, z: usize },
    /// Biome cell count was invalid (expected 64 coarse cells or 4,096 subchunk blocks).
    InvalidBiomeCellCount(usize),
    /// NBT palette data was malformed or corrupted.
    MalformedNbtPalette,
}

impl fmt::Display for BedrockChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPacketId(id) => {
                write!(f, "invalid Bedrock LevelChunk packet ID: {id:#04x}")
            }
            Self::MalformedVarInt => write!(f, "malformed or overflowing VarInt in Bedrock chunk"),
            Self::InvalidSubChunkCount(count) => write!(
                f,
                "subchunk count {count} is invalid (max {MAX_SUBCHUNKS} for Overworld)"
            ),
            Self::PayloadTooLarge { size, limit } => write!(
                f,
                "serialized chunk payload size {size} exceeds limit {limit}"
            ),
            Self::TruncatedPacket {
                expected,
                available,
            } => write!(
                f,
                "truncated packet: expected {expected} bytes, but only {available} available"
            ),
            Self::TruncatedWords {
                expected,
                available,
            } => write!(
                f,
                "truncated words array: expected {expected} words, but only {available} available"
            ),
            Self::UnsupportedSubChunkVersion(version) => {
                write!(
                    f,
                    "unsupported SubChunk version: {version} (expected 8 or 9)"
                )
            }
            Self::UnsupportedBitsPerBlock(bits) => {
                write!(f, "unsupported bits per block: {bits}")
            }
            Self::EmptyPalette => write!(f, "subchunk palette cannot be empty"),
            Self::PaletteTooLarge(count) => {
                write!(f, "subchunk palette size {count} exceeds maximum allowable")
            }
            Self::IndexExceedsBitsPerBlock {
                index,
                bits_per_block,
            } => write!(
                f,
                "block index {index} exceeds bits-per-block capacity ({bits_per_block} bits)"
            ),
            Self::IndexOutOfBounds(index) => write!(
                f,
                "subchunk block index {index} out of bounds (must be < {BLOCKS_PER_SUBCHUNK})"
            ),
            Self::CoordinatesOutOfBounds { x, y, z } => write!(
                f,
                "local block coordinates ({x}, {y}, {z}) out of bounds (0..16)"
            ),
            Self::InvalidBiomeCellCount(count) => write!(
                f,
                "invalid biome cell count {count}: expected 64 or {BLOCKS_PER_SUBCHUNK}"
            ),
            Self::MalformedNbtPalette => write!(f, "malformed NBT palette in subchunk storage"),
        }
    }
}

impl std::error::Error for BedrockChunkError {}

// --- VarInt / VarUint LEB128 and ZigZag wire helpers ---

/// Encodes an `i32` using 32-bit ZigZag mapping into a `u32`.
#[inline]
pub fn zigzag_encode32(val: i32) -> u32 {
    ((val << 1) ^ (val >> 31)) as u32
}

/// Decodes a 32-bit ZigZag-mapped `u32` into a signed `i32`.
#[inline]
pub fn zigzag_decode32(val: u32) -> i32 {
    ((val >> 1) as i32) ^ (-((val & 1) as i32))
}

/// Encodes a 32-bit unsigned integer into variable-length LEB128 bytes (VarUint32).
pub fn write_varuint32(mut value: u32, out: &mut Vec<u8>) {
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

/// Decodes a 32-bit unsigned integer from variable-length LEB128 bytes (VarUint32).
///
/// Returns the decoded integer and the number of bytes consumed.
pub fn read_varuint32(slice: &[u8]) -> Result<(u32, usize), BedrockChunkError> {
    let mut value = 0u32;
    for (i, &byte) in slice.iter().take(5).enumerate() {
        if i == 4 {
            if byte & 0xf0 != 0 {
                return Err(BedrockChunkError::MalformedVarInt);
            }
            value |= u32::from(byte & 0x0f) << 28;
            return Ok((value, 5));
        }
        value |= u32::from(byte & 0x7f) << (i * 7);
        if (byte & 0x80) == 0 {
            return Ok((value, i + 1));
        }
    }
    Err(BedrockChunkError::MalformedVarInt)
}

/// Encodes a signed 32-bit integer into ZigZag-encoded VarInt32 bytes.
pub fn write_varint32(value: i32, out: &mut Vec<u8>) {
    write_varuint32(zigzag_encode32(value), out);
}

/// Decodes a signed 32-bit integer from ZigZag-encoded VarInt32 bytes.
pub fn read_varint32(slice: &[u8]) -> Result<(i32, usize), BedrockChunkError> {
    let (u, len) = read_varuint32(slice)?;
    Ok((zigzag_decode32(u), len))
}

// --- Coordinate and bit packing helpers ---

/// Converts local subchunk coordinates `(x, y, z)` into a 1D block index in Bedrock XZY order.
///
/// `index = (x << 8) | (z << 4) | y`.
#[inline]
pub fn block_index(x: usize, y: usize, z: usize) -> Result<usize, BedrockChunkError> {
    if x >= 16 || y >= 16 || z >= 16 {
        return Err(BedrockChunkError::CoordinatesOutOfBounds { x, y, z });
    }
    Ok((x << 8) | (z << 4) | y)
}

/// Converts a 1D block index back into local `(x, y, z)` coordinates within a subchunk.
#[inline]
pub fn block_coords(index: usize) -> Result<(usize, usize, usize), BedrockChunkError> {
    if index >= BLOCKS_PER_SUBCHUNK {
        return Err(BedrockChunkError::IndexOutOfBounds(index));
    }
    let x = (index >> 8) & 0x0f;
    let z = (index >> 4) & 0x0f;
    let y = index & 0x0f;
    Ok((x, y, z))
}

/// Returns the number of blocks packed into a single 32-bit word for a given `bits_per_block`.
///
/// In Bedrock subchunk storage, block indices never span across 32-bit word boundaries.
pub fn blocks_per_word(bits_per_block: u8) -> Result<usize, BedrockChunkError> {
    match bits_per_block {
        1 => Ok(32),
        2 => Ok(16),
        3 => Ok(10),
        4 => Ok(8),
        5 => Ok(6),
        6 => Ok(5),
        8 => Ok(4),
        16 => Ok(2),
        other => Err(BedrockChunkError::UnsupportedBitsPerBlock(other)),
    }
}

/// Returns the number of 32-bit words required to store all 4,096 blocks at `bits_per_block`.
pub fn words_count(bits_per_block: u8) -> Result<usize, BedrockChunkError> {
    let bpw = blocks_per_word(bits_per_block)?;
    Ok(BLOCKS_PER_SUBCHUNK.div_ceil(bpw))
}

/// Selects the minimum standard Bedrock `bits_per_block` for a given palette size.
///
/// Standard Bedrock subchunk palette widths are 1, 2, 3, 4, 5, 6, 8, or 16 bits.
pub fn select_bits_per_block(palette_size: usize) -> Result<u8, BedrockChunkError> {
    if palette_size == 0 {
        return Err(BedrockChunkError::EmptyPalette);
    }
    if palette_size <= 2 {
        Ok(1)
    } else if palette_size <= 4 {
        Ok(2)
    } else if palette_size <= 8 {
        Ok(3)
    } else if palette_size <= 16 {
        Ok(4)
    } else if palette_size <= 32 {
        Ok(5)
    } else if palette_size <= 64 {
        Ok(6)
    } else if palette_size <= 256 {
        Ok(8)
    } else if palette_size <= 65536 {
        Ok(16)
    } else {
        Err(BedrockChunkError::PaletteTooLarge(palette_size))
    }
}

/// Packs 4,096 subchunk block indices into an array of 32-bit little-endian words.
///
/// Indices are packed sequentially into each word starting from LSB. If a word cannot fit
/// another complete index, the remaining high bits are left as zero padding.
pub fn pack_indices(
    indices: &[u16; BLOCKS_PER_SUBCHUNK],
    bits_per_block: u8,
) -> Result<Vec<u32>, BedrockChunkError> {
    let bpw = blocks_per_word(bits_per_block)?;
    let total_words = words_count(bits_per_block)?;
    let mut words = vec![0u32; total_words];
    let mask = (1u32 << bits_per_block) - 1;
    let bpb = usize::from(bits_per_block);

    for (w_idx, word) in words.iter_mut().enumerate() {
        let mut current_word = 0u32;
        for slot in 0..bpw {
            let block_idx = w_idx * bpw + slot;
            if block_idx >= BLOCKS_PER_SUBCHUNK {
                break;
            }
            let val = u32::from(indices[block_idx]);
            if val > mask {
                return Err(BedrockChunkError::IndexExceedsBitsPerBlock {
                    index: val,
                    bits_per_block,
                });
            }
            current_word |= (val & mask) << (slot * bpb);
        }
        *word = current_word;
    }
    Ok(words)
}

/// Unpacks 4,096 subchunk block indices from an array of 32-bit words.
pub fn unpack_indices(
    words: &[u32],
    bits_per_block: u8,
) -> Result<[u16; BLOCKS_PER_SUBCHUNK], BedrockChunkError> {
    let bpw = blocks_per_word(bits_per_block)?;
    let total_words = words_count(bits_per_block)?;
    if words.len() < total_words {
        return Err(BedrockChunkError::TruncatedWords {
            expected: total_words,
            available: words.len(),
        });
    }
    let mut indices = [0u16; BLOCKS_PER_SUBCHUNK];
    let mask = (1u32 << bits_per_block) - 1;
    let bpb = usize::from(bits_per_block);

    for (block_idx, index_out) in indices.iter_mut().enumerate() {
        let w_idx = block_idx / bpw;
        let slot = block_idx % bpw;
        let val = (words[w_idx] >> (slot * bpb)) & mask;
        *index_out = val as u16;
    }
    Ok(indices)
}

// --- SubChunk block storage and palette structures ---

/// Palette representation for a Bedrock block storage layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockPalette {
    /// Network runtime IDs encoded as ZigZag VarInt32.
    Runtime(Vec<i32>),
    /// Persistent storage NBT block state compounds.
    Nbt(Vec<Vec<u8>>),
}

impl BlockPalette {
    /// Whether this palette represents network runtime block IDs.
    pub fn is_network_runtime(&self) -> bool {
        matches!(self, Self::Runtime(_))
    }

    /// Number of palette entries.
    pub fn len(&self) -> usize {
        match self {
            Self::Runtime(v) => v.len(),
            Self::Nbt(v) => v.len(),
        }
    }

    /// Whether the palette is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A single block storage layer within a SubChunk (layer 0 for solid blocks, layer 1 for liquids).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockStorage {
    /// Bits per block index in words array (1, 2, 3, 4, 5, 6, 8, or 16).
    pub bits_per_block: u8,
    /// Packed 32-bit little-endian words containing block indices.
    pub words: Vec<u32>,
    /// Palette mapping indices to block runtime IDs or NBT compounds.
    pub palette: BlockPalette,
}

impl BlockStorage {
    /// Creates a new `BlockStorage` layer with explicit words and palette, verifying consistency.
    pub fn new(
        bits_per_block: u8,
        words: Vec<u32>,
        palette: BlockPalette,
    ) -> Result<Self, BedrockChunkError> {
        let expected_words = words_count(bits_per_block)?;
        if words.len() != expected_words {
            return Err(BedrockChunkError::TruncatedWords {
                expected: expected_words,
                available: words.len(),
            });
        }
        if palette.is_empty() {
            return Err(BedrockChunkError::EmptyPalette);
        }
        if palette.len() > BLOCKS_PER_SUBCHUNK {
            return Err(BedrockChunkError::PaletteTooLarge(palette.len()));
        }
        Ok(Self {
            bits_per_block,
            words,
            palette,
        })
    }

    /// Constructs a network runtime `BlockStorage` layer from 4,096 indices and a runtime ID palette.
    pub fn from_indices(
        indices: &[u16; BLOCKS_PER_SUBCHUNK],
        palette: Vec<i32>,
    ) -> Result<Self, BedrockChunkError> {
        let bpb = select_bits_per_block(palette.len())?;
        let words = pack_indices(indices, bpb)?;
        Ok(Self {
            bits_per_block: bpb,
            words,
            palette: BlockPalette::Runtime(palette),
        })
    }

    /// Creates a solid subchunk storage layer filled entirely with a single block runtime ID.
    pub fn solid(runtime_id: i32) -> Self {
        let words = vec![0u32; 128]; // 4096 / 32 = 128 words at bits_per_block = 1
        Self {
            bits_per_block: 1,
            words,
            palette: BlockPalette::Runtime(vec![runtime_id]),
        }
    }

    /// Creates an empty air subchunk storage layer (runtime ID 0).
    pub fn air() -> Self {
        Self::solid(0)
    }

    /// Serializes this storage layer into the Bedrock wire format.
    pub fn serialize(&self, out: &mut Vec<u8>) -> Result<(), BedrockChunkError> {
        let is_runtime: u8 = if self.palette.is_network_runtime() {
            1
        } else {
            0
        };
        // Header byte: (bits_per_block << 1) | is_network_runtime
        out.push((self.bits_per_block << 1) | is_runtime);

        // Word array (little-endian 32-bit unsigned integers)
        for &word in &self.words {
            out.extend_from_slice(&word.to_le_bytes());
        }

        // Palette entries
        match &self.palette {
            BlockPalette::Runtime(entries) => {
                // Palette size is encoded as ZigZag VarInt32
                write_varint32(entries.len() as i32, out);
                for &entry in entries {
                    write_varint32(entry, out);
                }
            }
            BlockPalette::Nbt(entries) => {
                // NBT palette count is stored as 32-bit little-endian integer in storage format
                out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
                for entry in entries {
                    out.extend_from_slice(entry);
                }
            }
        }
        Ok(())
    }
}

fn scan_nbt_compound_le(bytes: &[u8], offset: &mut usize) -> Result<(), BedrockChunkError> {
    if *offset >= bytes.len() {
        return Err(BedrockChunkError::TruncatedPacket {
            expected: *offset + 1,
            available: bytes.len(),
        });
    }
    let tag_type = bytes[*offset];
    *offset += 1;
    if tag_type != 10 {
        return Err(BedrockChunkError::MalformedNbtPalette);
    }
    if *offset + 2 > bytes.len() {
        return Err(BedrockChunkError::TruncatedPacket {
            expected: *offset + 2,
            available: bytes.len(),
        });
    }
    let name_len = u16::from_le_bytes([bytes[*offset], bytes[*offset + 1]]) as usize;
    *offset += 2;
    if *offset + name_len > bytes.len() {
        return Err(BedrockChunkError::TruncatedPacket {
            expected: *offset + name_len,
            available: bytes.len(),
        });
    }
    *offset += name_len;
    scan_compound_payload_le(bytes, offset, 0)
}

fn scan_compound_payload_le(
    bytes: &[u8],
    offset: &mut usize,
    depth: usize,
) -> Result<(), BedrockChunkError> {
    if depth > 128 {
        return Err(BedrockChunkError::MalformedNbtPalette);
    }
    loop {
        if *offset >= bytes.len() {
            return Err(BedrockChunkError::TruncatedPacket {
                expected: *offset + 1,
                available: bytes.len(),
            });
        }
        let tag_id = bytes[*offset];
        *offset += 1;
        if tag_id == 0 {
            break;
        }
        if *offset + 2 > bytes.len() {
            return Err(BedrockChunkError::TruncatedPacket {
                expected: *offset + 2,
                available: bytes.len(),
            });
        }
        let name_len = u16::from_le_bytes([bytes[*offset], bytes[*offset + 1]]) as usize;
        *offset += 2;
        if *offset + name_len > bytes.len() {
            return Err(BedrockChunkError::TruncatedPacket {
                expected: *offset + name_len,
                available: bytes.len(),
            });
        }
        *offset += name_len;
        skip_nbt_tag_payload_le(tag_id, bytes, offset, depth + 1)?;
    }
    Ok(())
}

fn skip_nbt_tag_payload_le(
    tag_id: u8,
    bytes: &[u8],
    offset: &mut usize,
    depth: usize,
) -> Result<(), BedrockChunkError> {
    match tag_id {
        1 => {
            if *offset >= bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + 1,
                    available: bytes.len(),
                });
            }
            *offset += 1;
        }
        2 => {
            if *offset + 2 > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + 2,
                    available: bytes.len(),
                });
            }
            *offset += 2;
        }
        3 => {
            if *offset + 4 > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + 4,
                    available: bytes.len(),
                });
            }
            *offset += 4;
        }
        4 => {
            if *offset + 8 > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + 8,
                    available: bytes.len(),
                });
            }
            *offset += 8;
        }
        5 => {
            if *offset + 4 > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + 4,
                    available: bytes.len(),
                });
            }
            *offset += 4;
        }
        6 => {
            if *offset + 8 > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + 8,
                    available: bytes.len(),
                });
            }
            *offset += 8;
        }
        7 => {
            if *offset + 4 > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + 4,
                    available: bytes.len(),
                });
            }
            let len = i32::from_le_bytes([
                bytes[*offset],
                bytes[*offset + 1],
                bytes[*offset + 2],
                bytes[*offset + 3],
            ]);
            *offset += 4;
            if len < 0 {
                return Err(BedrockChunkError::MalformedNbtPalette);
            }
            let len = len as usize;
            if *offset + len > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + len,
                    available: bytes.len(),
                });
            }
            *offset += len;
        }
        8 => {
            if *offset + 2 > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + 2,
                    available: bytes.len(),
                });
            }
            let len = u16::from_le_bytes([bytes[*offset], bytes[*offset + 1]]) as usize;
            *offset += 2;
            if *offset + len > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + len,
                    available: bytes.len(),
                });
            }
            *offset += len;
        }
        9 => {
            if *offset + 5 > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + 5,
                    available: bytes.len(),
                });
            }
            let elem_type = bytes[*offset];
            *offset += 1;
            let count = i32::from_le_bytes([
                bytes[*offset],
                bytes[*offset + 1],
                bytes[*offset + 2],
                bytes[*offset + 3],
            ]);
            *offset += 4;
            if count <= 0 {
                return Ok(());
            }
            if elem_type == 0 {
                return Err(BedrockChunkError::MalformedNbtPalette);
            }
            for _ in 0..count {
                skip_nbt_tag_payload_le(elem_type, bytes, offset, depth + 1)?;
            }
        }
        10 => {
            scan_compound_payload_le(bytes, offset, depth + 1)?;
        }
        11 => {
            if *offset + 4 > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + 4,
                    available: bytes.len(),
                });
            }
            let count = i32::from_le_bytes([
                bytes[*offset],
                bytes[*offset + 1],
                bytes[*offset + 2],
                bytes[*offset + 3],
            ]);
            *offset += 4;
            if count < 0 {
                return Err(BedrockChunkError::MalformedNbtPalette);
            }
            let bytes_len = (count as usize)
                .checked_mul(4)
                .ok_or(BedrockChunkError::MalformedNbtPalette)?;
            if *offset + bytes_len > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + bytes_len,
                    available: bytes.len(),
                });
            }
            *offset += bytes_len;
        }
        12 => {
            if *offset + 4 > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + 4,
                    available: bytes.len(),
                });
            }
            let count = i32::from_le_bytes([
                bytes[*offset],
                bytes[*offset + 1],
                bytes[*offset + 2],
                bytes[*offset + 3],
            ]);
            *offset += 4;
            if count < 0 {
                return Err(BedrockChunkError::MalformedNbtPalette);
            }
            let bytes_len = (count as usize)
                .checked_mul(8)
                .ok_or(BedrockChunkError::MalformedNbtPalette)?;
            if *offset + bytes_len > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: *offset + bytes_len,
                    available: bytes.len(),
                });
            }
            *offset += bytes_len;
        }
        _ => return Err(BedrockChunkError::MalformedNbtPalette),
    }
    Ok(())
}

impl BlockStorage {
    /// Deserializes a storage layer from bytes, returning the layer and bytes consumed.
    pub fn deserialize(bytes: &[u8]) -> Result<(Self, usize), BedrockChunkError> {
        if bytes.is_empty() {
            return Err(BedrockChunkError::TruncatedPacket {
                expected: 1,
                available: 0,
            });
        }
        let header = bytes[0];
        let bits_per_block = header >> 1;
        let is_runtime = (header & 1) != 0;

        let total_words = words_count(bits_per_block)?;
        let words_bytes = total_words
            .checked_mul(4)
            .ok_or(BedrockChunkError::TruncatedWords {
                expected: total_words,
                available: 0,
            })?;

        let mut offset = 1;
        if bytes.len() < offset + words_bytes {
            return Err(BedrockChunkError::TruncatedPacket {
                expected: offset + words_bytes,
                available: bytes.len(),
            });
        }

        let mut words = Vec::with_capacity(total_words);
        for chunk in bytes[offset..offset + words_bytes].as_chunks::<4>().0 {
            let word = u32::from_le_bytes(*chunk);
            words.push(word);
        }
        offset += words_bytes;

        let palette = if is_runtime {
            let (palette_len_i32, len) = read_varint32(&bytes[offset..])?;
            offset += len;
            if palette_len_i32 < 0 || (palette_len_i32 as usize) > BLOCKS_PER_SUBCHUNK {
                return Err(BedrockChunkError::PaletteTooLarge(
                    palette_len_i32.max(0) as usize
                ));
            }
            let palette_count = palette_len_i32 as usize;
            let mut entries = Vec::with_capacity(palette_count);
            for _ in 0..palette_count {
                let (entry, len) = read_varint32(&bytes[offset..])?;
                offset += len;
                entries.push(entry);
            }
            BlockPalette::Runtime(entries)
        } else {
            if bytes.len() < offset + 4 {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: offset + 4,
                    available: bytes.len(),
                });
            }
            let palette_count = u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]) as usize;
            offset += 4;
            if palette_count > BLOCKS_PER_SUBCHUNK {
                return Err(BedrockChunkError::PaletteTooLarge(palette_count));
            }
            let mut entries = Vec::with_capacity(palette_count);
            for _ in 0..palette_count {
                let start = offset;
                scan_nbt_compound_le(bytes, &mut offset)?;
                entries.push(bytes[start..offset].to_vec());
            }
            BlockPalette::Nbt(entries)
        };

        let storage = Self {
            bits_per_block,
            words,
            palette,
        };
        Ok((storage, offset))
    }
}

// --- SubChunk container ---

/// SubChunk format version (version 8 without Y-index, or version 9 with Y-index).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubChunkVersion {
    /// Format version 8: version byte (8) followed by storage layers count.
    V8 = 8,
    /// Format version 9: version byte (9), storage layers count, and signed Y index byte.
    V9 = 9,
}

impl SubChunkVersion {
    /// Byte representation of this format version.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a SubChunk format version byte.
    pub fn from_u8(val: u8) -> Result<Self, BedrockChunkError> {
        match val {
            SUBCHUNK_VERSION_8 => Ok(Self::V8),
            SUBCHUNK_VERSION_9 => Ok(Self::V9),
            other => Err(BedrockChunkError::UnsupportedSubChunkVersion(other)),
        }
    }
}

/// A single vertical 16x16x16 subchunk column section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubChunk {
    /// Format version (8 or 9).
    pub version: SubChunkVersion,
    /// Vertical subchunk index (used in version 9, e.g. -4 to 19 for Overworld).
    pub sub_chunk_y: Option<i8>,
    /// Storage layers (layer 0 for blocks, layer 1 for optional waterlogged blocks).
    pub layers: Vec<BlockStorage>,
}

impl SubChunk {
    /// Creates a new `SubChunk` with the specified version, optional Y index, and layers.
    pub fn new(
        version: SubChunkVersion,
        sub_chunk_y: Option<i8>,
        layers: Vec<BlockStorage>,
    ) -> Self {
        Self {
            version,
            sub_chunk_y,
            layers,
        }
    }

    /// Creates a `SubChunk` with a single block storage layer.
    pub fn single_layer(
        version: SubChunkVersion,
        sub_chunk_y: Option<i8>,
        layer: BlockStorage,
    ) -> Self {
        Self {
            version,
            sub_chunk_y,
            layers: vec![layer],
        }
    }

    /// Creates an empty air `SubChunk` (single layer of air, runtime ID 0).
    pub fn empty(version: SubChunkVersion, sub_chunk_y: Option<i8>) -> Self {
        Self::single_layer(version, sub_chunk_y, BlockStorage::air())
    }

    /// Creates a solid `SubChunk` filled entirely with a single block runtime ID.
    pub fn solid(version: SubChunkVersion, sub_chunk_y: Option<i8>, runtime_id: i32) -> Self {
        Self::single_layer(version, sub_chunk_y, BlockStorage::solid(runtime_id))
    }

    /// Serializes this `SubChunk` into the Bedrock wire byte format.
    pub fn serialize(&self, out: &mut Vec<u8>) -> Result<(), BedrockChunkError> {
        out.push(self.version.as_u8());
        out.push(self.layers.len() as u8);
        if self.version == SubChunkVersion::V9 {
            out.push(self.sub_chunk_y.unwrap_or(0) as u8);
        }
        for layer in &self.layers {
            layer.serialize(out)?;
        }
        Ok(())
    }

    /// Deserializes a `SubChunk` from wire bytes, returning the subchunk and bytes consumed.
    pub fn deserialize(bytes: &[u8]) -> Result<(Self, usize), BedrockChunkError> {
        if bytes.len() < 2 {
            return Err(BedrockChunkError::TruncatedPacket {
                expected: 2,
                available: bytes.len(),
            });
        }
        let version = SubChunkVersion::from_u8(bytes[0])?;
        let layer_count = bytes[1] as usize;

        let (sub_chunk_y, mut offset) = match version {
            SubChunkVersion::V8 => (None, 2),
            SubChunkVersion::V9 => {
                if bytes.len() < 3 {
                    return Err(BedrockChunkError::TruncatedPacket {
                        expected: 3,
                        available: bytes.len(),
                    });
                }
                (Some(bytes[2] as i8), 3)
            }
        };

        let mut layers = Vec::with_capacity(layer_count);
        for _ in 0..layer_count {
            let (layer, consumed) = BlockStorage::deserialize(&bytes[offset..])?;
            offset += consumed;
            layers.push(layer);
        }

        Ok((
            Self {
                version,
                sub_chunk_y,
                layers,
            },
            offset,
        ))
    }
}

// --- 3D Biomes storage ---

/// 3D Biome container for a SubChunk column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BiomeStorage {
    /// Single uniform biome ID for the entire subchunk (compact bits_per_biome = 0).
    Single(i32),
    /// Paletted 3D biomes with bit-packed words and palette entries.
    Paletted {
        bits_per_biome: u8,
        words: Vec<u32>,
        palette: Vec<i32>,
    },
}

impl BiomeStorage {
    /// Creates a single uniform biome storage.
    pub fn single(biome_id: i32) -> Self {
        Self::Single(biome_id)
    }

    /// Creates a paletted 3D biome storage from cell indices and a palette of biome IDs.
    ///
    /// Accepts either a full 4,096-block subchunk grid or a coarse 4x4x4 (64-cell) grid,
    /// which will be expanded to the full 4,096 blocks (covering 4x4x4 blocks per biome cell).
    pub fn from_indices(indices: &[u16], palette: Vec<i32>) -> Result<Self, BedrockChunkError> {
        if palette.is_empty() {
            return Err(BedrockChunkError::EmptyPalette);
        }
        if palette.len() == 1 {
            return Ok(Self::Single(palette[0]));
        }
        let full_indices: Vec<u16> = match indices.len() {
            BLOCKS_PER_SUBCHUNK => indices.to_vec(),
            64 => {
                let mut expanded = vec![0u16; BLOCKS_PER_SUBCHUNK];
                for x in 0..16 {
                    let qx = x / 4;
                    for z in 0..16 {
                        let qz = z / 4;
                        for y in 0..16 {
                            let qy = y / 4;
                            let quart_idx = (qx << 4) | (qz << 2) | qy;
                            let block_idx = (x << 8) | (z << 4) | y;
                            expanded[block_idx] = indices[quart_idx];
                        }
                    }
                }
                expanded
            }
            other => return Err(BedrockChunkError::InvalidBiomeCellCount(other)),
        };

        let bpb = select_bits_per_block(palette.len())?;
        let bpw = blocks_per_word(bpb)?;
        let total_words = BLOCKS_PER_SUBCHUNK.div_ceil(bpw);
        let mut words = vec![0u32; total_words];
        let mask = (1u32 << bpb) - 1;
        let bpb_usize = usize::from(bpb);

        for (w_idx, word) in words.iter_mut().enumerate() {
            let mut current_word = 0u32;
            for slot in 0..bpw {
                let cell_idx = w_idx * bpw + slot;
                if cell_idx >= full_indices.len() {
                    break;
                }
                let val = u32::from(full_indices[cell_idx]);
                if val > mask {
                    return Err(BedrockChunkError::IndexExceedsBitsPerBlock {
                        index: val,
                        bits_per_block: bpb,
                    });
                }
                current_word |= (val & mask) << (slot * bpb_usize);
            }
            *word = current_word;
        }

        Ok(Self::Paletted {
            bits_per_biome: bpb,
            words,
            palette,
        })
    }

    /// Serializes the biome storage into the Bedrock wire format.
    pub fn serialize(&self, out: &mut Vec<u8>) -> Result<(), BedrockChunkError> {
        match self {
            Self::Single(biome_id) => {
                // bits_per_biome = 0, non-persistence flag = 1 => header byte is (0 << 1) | 1 = 1
                out.push(1);
                write_varint32(*biome_id, out);
            }
            Self::Paletted {
                bits_per_biome,
                words,
                palette,
            } => {
                out.push((bits_per_biome << 1) | 1);
                for &word in words {
                    out.extend_from_slice(&word.to_le_bytes());
                }
                write_varint32(palette.len() as i32, out);
                for &entry in palette {
                    write_varint32(entry, out);
                }
            }
        }
        Ok(())
    }

    /// Deserializes a biome storage entry from wire bytes covering all 4,096 blocks of a subchunk.
    pub fn deserialize(bytes: &[u8]) -> Result<(Self, usize), BedrockChunkError> {
        if bytes.is_empty() {
            return Err(BedrockChunkError::TruncatedPacket {
                expected: 1,
                available: 0,
            });
        }
        let header = bytes[0];
        let bits_per_biome = header >> 1;
        if bits_per_biome == 0 {
            let (biome_id, len) = read_varint32(&bytes[1..])?;
            Ok((Self::Single(biome_id), 1 + len))
        } else {
            let cell_count = BLOCKS_PER_SUBCHUNK;
            let bpw = blocks_per_word(bits_per_biome)?;
            let total_words = cell_count.div_ceil(bpw);
            let words_bytes = total_words * 4;
            let mut offset = 1;
            if bytes.len() < offset + words_bytes {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: offset + words_bytes,
                    available: bytes.len(),
                });
            }
            let mut words = Vec::with_capacity(total_words);
            for chunk in bytes[offset..offset + words_bytes].as_chunks::<4>().0 {
                let word = u32::from_le_bytes(*chunk);
                words.push(word);
            }
            offset += words_bytes;

            let (palette_count_i32, len) = read_varint32(&bytes[offset..])?;
            offset += len;
            if palette_count_i32 < 0 || (palette_count_i32 as usize) > cell_count {
                return Err(BedrockChunkError::PaletteTooLarge(
                    palette_count_i32.max(0) as usize
                ));
            }
            let palette_count = palette_count_i32 as usize;
            let mut palette = Vec::with_capacity(palette_count);
            for _ in 0..palette_count {
                let (entry, len) = read_varint32(&bytes[offset..])?;
                offset += len;
                palette.push(entry);
            }

            Ok((
                Self::Paletted {
                    bits_per_biome,
                    words,
                    palette,
                },
                offset,
            ))
        }
    }
}

// --- Payload encoder helper ---

/// Serializes a complete Bedrock chunk payload (all subchunks followed by 3D biomes and border count).
///
/// Bounded by [`MAX_SERIALIZED_CHUNK_SIZE`].
pub fn encode_chunk_payload(
    subchunks: &[SubChunk],
    biomes: &[BiomeStorage],
) -> Result<Vec<u8>, BedrockChunkError> {
    if subchunks.len() > MAX_SUBCHUNKS as usize {
        return Err(BedrockChunkError::InvalidSubChunkCount(
            subchunks.len() as u32
        ));
    }

    let mut payload = Vec::new();

    // 1. Subchunks
    for subchunk in subchunks {
        subchunk.serialize(&mut payload)?;
        if payload.len() > MAX_SERIALIZED_CHUNK_SIZE {
            return Err(BedrockChunkError::PayloadTooLarge {
                size: payload.len(),
                limit: MAX_SERIALIZED_CHUNK_SIZE,
            });
        }
    }

    // 2. 3D Biomes
    for biome in biomes {
        biome.serialize(&mut payload)?;
        if payload.len() > MAX_SERIALIZED_CHUNK_SIZE {
            return Err(BedrockChunkError::PayloadTooLarge {
                size: payload.len(),
                limit: MAX_SERIALIZED_CHUNK_SIZE,
            });
        }
    }

    // 3. Border blocks count (0 byte)
    payload.push(0);

    if payload.len() > MAX_SERIALIZED_CHUNK_SIZE {
        return Err(BedrockChunkError::PayloadTooLarge {
            size: payload.len(),
            limit: MAX_SERIALIZED_CHUNK_SIZE,
        });
    }

    Ok(payload)
}

// --- LevelChunkPacket ---

/// Bedrock 1.26.51 `LevelChunkPacket` (ID `0x3A`).
///
/// Transmits chunk terrain and biome data for a single column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelChunkPacket {
    /// Chunk X grid coordinate (zigzag signed VarInt32 on wire).
    pub chunk_x: i32,
    /// Chunk Z grid coordinate (zigzag signed VarInt32 on wire).
    pub chunk_z: i32,
    /// Dimension ID (0 = Overworld, 1 = Nether, 2 = The End; zigzag signed VarInt32 on wire).
    pub dimension_id: i32,
    /// Number of subchunks enclosed (standard 24 for Overworld).
    pub subchunk_count: u32,
    /// Whether client blob caching is enabled.
    pub cache_enabled: bool,
    /// Blob hashes when caching is enabled (each 64-bit LE hash).
    pub blob_hashes: Vec<u64>,
    /// Serialized chunk subchunks, biomes, and tile entities payload.
    pub data: Vec<u8>,
}

impl LevelChunkPacket {
    /// Packet ID constant (0x3A).
    pub const PACKET_ID: u32 = LEVEL_CHUNK_PACKET_ID;

    /// Creates a new `LevelChunkPacket` validating bounds.
    pub fn new(
        chunk_x: i32,
        chunk_z: i32,
        dimension_id: i32,
        subchunk_count: u32,
        cache_enabled: bool,
        blob_hashes: Vec<u64>,
        data: Vec<u8>,
    ) -> Result<Self, BedrockChunkError> {
        if subchunk_count > MAX_SUBCHUNKS {
            return Err(BedrockChunkError::InvalidSubChunkCount(subchunk_count));
        }
        if data.len() > MAX_SERIALIZED_CHUNK_SIZE {
            return Err(BedrockChunkError::PayloadTooLarge {
                size: data.len(),
                limit: MAX_SERIALIZED_CHUNK_SIZE,
            });
        }
        Ok(Self {
            chunk_x,
            chunk_z,
            dimension_id,
            subchunk_count,
            cache_enabled,
            blob_hashes,
            data,
        })
    }

    /// Convenience constructor for Overworld non-cached chunks.
    pub fn overworld(
        chunk_x: i32,
        chunk_z: i32,
        subchunk_count: u32,
        data: Vec<u8>,
    ) -> Result<Self, BedrockChunkError> {
        Self::new(chunk_x, chunk_z, 0, subchunk_count, false, Vec::new(), data)
    }

    /// Convenience constructor creating a direct `LevelChunkPacket` from subchunks and biomes.
    pub fn from_subchunks(
        chunk_x: i32,
        chunk_z: i32,
        dimension_id: i32,
        subchunks: &[SubChunk],
        biomes: &[BiomeStorage],
    ) -> Result<Self, BedrockChunkError> {
        let payload = encode_chunk_payload(subchunks, biomes)?;
        Self::new(
            chunk_x,
            chunk_z,
            dimension_id,
            subchunks.len() as u32,
            false,
            Vec::new(),
            payload,
        )
    }

    /// Encodes this packet into Bedrock wire bytes prefixed with packet ID `0x3A`.
    pub fn encode(&self) -> Result<Vec<u8>, BedrockChunkError> {
        if self.subchunk_count > MAX_SUBCHUNKS {
            return Err(BedrockChunkError::InvalidSubChunkCount(self.subchunk_count));
        }
        if self.data.len() > MAX_SERIALIZED_CHUNK_SIZE {
            return Err(BedrockChunkError::PayloadTooLarge {
                size: self.data.len(),
                limit: MAX_SERIALIZED_CHUNK_SIZE,
            });
        }

        let mut out = Vec::with_capacity(16 + self.data.len());

        // 1. Packet ID (0x3A / 58)
        write_varuint32(Self::PACKET_ID, &mut out);

        // 2. Chunk X (zigzag VarInt32)
        write_varint32(self.chunk_x, &mut out);

        // 3. Chunk Z (zigzag VarInt32)
        write_varint32(self.chunk_z, &mut out);

        // 4. Dimension ID (zigzag VarInt32)
        write_varint32(self.dimension_id, &mut out);

        // 5. SubChunk count (Unsigned VarInt)
        write_varuint32(self.subchunk_count, &mut out);

        // 6. Cache enabled (bool)
        out.push(if self.cache_enabled { 1 } else { 0 });

        // If caching is enabled, write blob hashes
        if self.cache_enabled {
            write_varuint32(self.blob_hashes.len() as u32, &mut out);
            for &hash in &self.blob_hashes {
                out.extend_from_slice(&hash.to_le_bytes());
            }
        }

        // 7. Data payload (prefixed with unsigned VarInt32 byte length)
        write_varuint32(self.data.len() as u32, &mut out);
        out.extend_from_slice(&self.data);

        Ok(out)
    }

    /// Decodes a `LevelChunkPacket` from Bedrock wire bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, BedrockChunkError> {
        let mut offset = 0;

        // 1. Packet ID
        let (packet_id, len) = read_varuint32(&bytes[offset..])?;
        offset += len;
        if packet_id != Self::PACKET_ID {
            return Err(BedrockChunkError::InvalidPacketId(packet_id));
        }

        // 2. Chunk X
        let (chunk_x, len) = read_varint32(&bytes[offset..])?;
        offset += len;

        // 3. Chunk Z
        let (chunk_z, len) = read_varint32(&bytes[offset..])?;
        offset += len;

        // 4. Dimension ID
        let (dimension_id, len) = read_varint32(&bytes[offset..])?;
        offset += len;

        // 5. SubChunk count
        let (subchunk_count, len) = read_varuint32(&bytes[offset..])?;
        offset += len;
        if subchunk_count > MAX_SUBCHUNKS {
            return Err(BedrockChunkError::InvalidSubChunkCount(subchunk_count));
        }

        // 6. Cache enabled
        if offset >= bytes.len() {
            return Err(BedrockChunkError::TruncatedPacket {
                expected: offset + 1,
                available: bytes.len(),
            });
        }
        let cache_enabled = bytes[offset] != 0;
        offset += 1;

        let mut blob_hashes = Vec::new();
        if cache_enabled {
            let (blob_count, len) = read_varuint32(&bytes[offset..])?;
            offset += len;
            let blob_bytes =
                (blob_count as usize)
                    .checked_mul(8)
                    .ok_or(BedrockChunkError::PayloadTooLarge {
                        size: usize::MAX,
                        limit: MAX_SERIALIZED_CHUNK_SIZE,
                    })?;
            if offset + blob_bytes > bytes.len() {
                return Err(BedrockChunkError::TruncatedPacket {
                    expected: offset + blob_bytes,
                    available: bytes.len(),
                });
            }
            blob_hashes.reserve(blob_count as usize);
            for chunk in bytes[offset..offset + blob_bytes].as_chunks::<8>().0 {
                blob_hashes.push(u64::from_le_bytes(*chunk));
            }
            offset += blob_bytes;
        }

        // 7. Data payload
        let (data_len, len) = read_varuint32(&bytes[offset..])?;
        offset += len;
        let data_len = data_len as usize;
        if data_len > MAX_SERIALIZED_CHUNK_SIZE {
            return Err(BedrockChunkError::PayloadTooLarge {
                size: data_len,
                limit: MAX_SERIALIZED_CHUNK_SIZE,
            });
        }
        if offset + data_len > bytes.len() {
            return Err(BedrockChunkError::TruncatedPacket {
                expected: offset + data_len,
                available: bytes.len(),
            });
        }
        let data = bytes[offset..offset + data_len].to_vec();

        Ok(Self {
            chunk_x,
            chunk_z,
            dimension_id,
            subchunk_count,
            cache_enabled,
            blob_hashes,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zigzag32_round_trips_boundaries_and_zero() {
        for val in [
            0,
            1,
            -1,
            2,
            -2,
            42,
            -42,
            i32::MAX,
            i32::MIN,
            i32::MAX - 1,
            i32::MIN + 1,
        ] {
            let encoded = zigzag_encode32(val);
            let decoded = zigzag_decode32(encoded);
            assert_eq!(decoded, val, "failed zigzag round-trip for {val}");
        }
        assert_eq!(zigzag_encode32(0), 0);
        assert_eq!(zigzag_encode32(-1), 1);
        assert_eq!(zigzag_encode32(1), 2);
        assert_eq!(zigzag_encode32(-2), 3);
    }

    #[test]
    fn varuint32_and_varint32_round_trip() {
        for val in [0u32, 1, 127, 128, 255, 16383, 16384, u32::MAX] {
            let mut buf = Vec::new();
            write_varuint32(val, &mut buf);
            let (decoded, len) = read_varuint32(&buf).expect("decode varuint32");
            assert_eq!(decoded, val);
            assert_eq!(len, buf.len());
        }

        for val in [0i32, 1, -1, 127, -128, 16384, -16384, i32::MAX, i32::MIN] {
            let mut buf = Vec::new();
            write_varint32(val, &mut buf);
            let (decoded, len) = read_varint32(&buf).expect("decode varint32");
            assert_eq!(decoded, val);
            assert_eq!(len, buf.len());
        }
    }

    #[test]
    fn varuint32_rejects_overflow_and_truncation() {
        // Truncated: byte with continuation bit but stream ends
        assert_eq!(
            read_varuint32(&[0x80]),
            Err(BedrockChunkError::MalformedVarInt)
        );
        // 5 bytes with overflow in upper nibble
        assert_eq!(
            read_varuint32(&[0x80, 0x80, 0x80, 0x80, 0x10]),
            Err(BedrockChunkError::MalformedVarInt)
        );
        // 6th byte continuation
        assert_eq!(
            read_varuint32(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
            Err(BedrockChunkError::MalformedVarInt)
        );
    }

    #[test]
    fn block_index_and_coords_round_trip() {
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..16 {
                    let idx = block_index(x, y, z).unwrap();
                    let (rx, ry, rz) = block_coords(idx).unwrap();
                    assert_eq!((rx, ry, rz), (x, y, z));
                }
            }
        }
        assert!(block_index(16, 0, 0).is_err());
        assert!(block_index(0, 16, 0).is_err());
        assert!(block_index(0, 0, 16).is_err());
        assert!(block_coords(BLOCKS_PER_SUBCHUNK).is_err());
    }

    #[test]
    fn serialize_empty_air_subchunk() {
        // Single-entry palette: [0] (air runtime ID)
        // bits_per_block = 1, 128 words of zeroes
        let subchunk_v8 = SubChunk::empty(SubChunkVersion::V8, None);
        let mut buf_v8 = Vec::new();
        subchunk_v8.serialize(&mut buf_v8).unwrap();

        // Check header: version 8, 1 storage layer
        assert_eq!(buf_v8[0], SUBCHUNK_VERSION_8);
        assert_eq!(buf_v8[1], 1); // 1 storage layer

        // Storage layer header: (bits_per_block << 1) | 1 = (1 << 1) | 1 = 3
        assert_eq!(buf_v8[2], 0x03);

        // 128 words of 0 (512 bytes)
        let words_bytes = &buf_v8[3..3 + 512];
        assert_eq!(words_bytes, &[0u8; 512]);

        // Palette length: ZigZag VarInt32 of 1 => 2 (0x02)
        assert_eq!(buf_v8[3 + 512], 0x02);

        // Palette entry 0: ZigZag VarInt32 of 0 => 0 (0x00)
        assert_eq!(buf_v8[3 + 512 + 1], 0x00);
        assert_eq!(buf_v8.len(), 3 + 512 + 2);

        // Test round-trip deserialization
        let (deserialized_v8, consumed_v8) = SubChunk::deserialize(&buf_v8).unwrap();
        assert_eq!(consumed_v8, buf_v8.len());
        assert_eq!(deserialized_v8.version, SubChunkVersion::V8);
        assert_eq!(deserialized_v8.layers.len(), 1);
        assert_eq!(deserialized_v8.layers[0].bits_per_block, 1);
        assert_eq!(
            deserialized_v8.layers[0].palette,
            BlockPalette::Runtime(vec![0])
        );

        // Test Version 9 empty air subchunk
        let subchunk_v9 = SubChunk::empty(SubChunkVersion::V9, Some(-4));
        let mut buf_v9 = Vec::new();
        subchunk_v9.serialize(&mut buf_v9).unwrap();

        assert_eq!(buf_v9[0], SUBCHUNK_VERSION_9);
        assert_eq!(buf_v9[1], 1); // 1 layer
        assert_eq!(buf_v9[2] as i8, -4); // sub_chunk_y = -4

        let (deserialized_v9, consumed_v9) = SubChunk::deserialize(&buf_v9).unwrap();
        assert_eq!(consumed_v9, buf_v9.len());
        assert_eq!(deserialized_v9.version, SubChunkVersion::V9);
        assert_eq!(deserialized_v9.sub_chunk_y, Some(-4));
    }

    #[test]
    fn serialize_solid_stone_subchunk() {
        const STONE_ID: i32 = 1;
        let subchunk = SubChunk::solid(SubChunkVersion::V8, None, STONE_ID);
        let mut buf = Vec::new();
        subchunk.serialize(&mut buf).unwrap();

        let (deserialized, consumed) = SubChunk::deserialize(&buf).unwrap();
        assert_eq!(consumed, buf.len());
        assert_eq!(deserialized.layers.len(), 1);
        assert_eq!(deserialized.layers[0].bits_per_block, 1);
        assert_eq!(
            deserialized.layers[0].palette,
            BlockPalette::Runtime(vec![STONE_ID])
        );

        // Unpack indices and verify every single block maps to index 0 (which is STONE_ID)
        let indices = unpack_indices(&deserialized.layers[0].words, 1).unwrap();
        for &idx in &indices {
            assert_eq!(idx, 0);
        }
    }

    #[test]
    fn serialize_mixed_subchunk_bit_packing_round_trip() {
        // Test multiple bits_per_block: 1, 2, 3, 4, 5, 6, 8, 16
        for &palette_size in &[2, 4, 7, 16, 25, 50, 128, 300] {
            let palette: Vec<i32> = (0..palette_size as i32).map(|i| i * 10 + 1).collect();
            let mut indices = [0u16; BLOCKS_PER_SUBCHUNK];
            for (i, slot) in indices.iter_mut().enumerate() {
                *slot = (i % palette_size) as u16;
            }

            let storage = BlockStorage::from_indices(&indices, palette.clone()).unwrap();
            let bpb = storage.bits_per_block;
            assert_eq!(bpb, select_bits_per_block(palette_size).unwrap());

            let mut out = Vec::new();
            storage.serialize(&mut out).unwrap();

            let (deserialized, consumed) = BlockStorage::deserialize(&out).unwrap();
            assert_eq!(consumed, out.len());
            assert_eq!(deserialized.bits_per_block, bpb);
            assert_eq!(deserialized.palette, BlockPalette::Runtime(palette));

            // Unpack indices and verify exact equality
            let unpacked = unpack_indices(&deserialized.words, bpb).unwrap();
            assert_eq!(unpacked, indices);
        }
    }

    #[test]
    fn biome_storage_single_and_paletted_round_trip() {
        // Single biome
        let single = BiomeStorage::single(42);
        let mut buf = Vec::new();
        single.serialize(&mut buf).unwrap();
        assert_eq!(buf[0], 0x01); // (0 << 1) | 1 = 1
        let (deserialized, len) = BiomeStorage::deserialize(&buf).unwrap();
        assert_eq!(len, buf.len());
        assert_eq!(deserialized, BiomeStorage::Single(42));

        // Paletted biomes: 64 coarse cells expanded to 4,096 blocks
        let palette = vec![1, 2, 3];
        let indices: Vec<u16> = (0..64).map(|i| (i % 3) as u16).collect();
        let paletted = BiomeStorage::from_indices(&indices, palette.clone()).unwrap();
        let mut p_buf = Vec::new();
        paletted.serialize(&mut p_buf).unwrap();

        let (deserialized_p, p_len) = BiomeStorage::deserialize(&p_buf).unwrap();
        assert_eq!(p_len, p_buf.len());
        assert_eq!(
            deserialized_p,
            BiomeStorage::Paletted {
                bits_per_biome: select_bits_per_block(3).unwrap(),
                words: match &paletted {
                    BiomeStorage::Paletted { words, .. } => words.clone(),
                    _ => unreachable!(),
                },
                palette,
            }
        );
    }

    #[test]
    fn encode_complete_level_chunk_packet() {
        // Build 24 subchunks (Overworld standard)
        let subchunks: Vec<SubChunk> = (-4..20)
            .map(|y| {
                if y < 0 {
                    SubChunk::solid(SubChunkVersion::V9, Some(y), 1) // Deepslate/stone
                } else if y == 0 {
                    SubChunk::solid(SubChunkVersion::V9, Some(y), 2) // Dirt/grass
                } else {
                    SubChunk::empty(SubChunkVersion::V9, Some(y)) // Air
                }
            })
            .collect();
        assert_eq!(subchunks.len(), OVERWORLD_SUBCHUNK_COUNT as usize);

        let biomes: Vec<BiomeStorage> = (0..24).map(|_| BiomeStorage::single(1)).collect();

        let packet = LevelChunkPacket::from_subchunks(10, -25, 0, &subchunks, &biomes).unwrap();
        assert_eq!(packet.chunk_x, 10);
        assert_eq!(packet.chunk_z, -25);
        assert_eq!(packet.dimension_id, 0);
        assert_eq!(packet.subchunk_count, 24);
        assert!(!packet.cache_enabled);
        assert!(!packet.data.is_empty());
        assert!(packet.data.len() < MAX_SERIALIZED_CHUNK_SIZE);

        let wire = packet.encode().unwrap();

        // 1. Packet ID must be 0x3A (58)
        assert_eq!(wire[0], LEVEL_CHUNK_PACKET_ID as u8);

        // 2. Decode packet and verify round-trip
        let decoded = LevelChunkPacket::decode(&wire).unwrap();
        assert_eq!(decoded.chunk_x, 10);
        assert_eq!(decoded.chunk_z, -25);
        assert_eq!(decoded.dimension_id, 0);
        assert_eq!(decoded.subchunk_count, 24);
        assert!(!decoded.cache_enabled);
        assert_eq!(decoded.data, packet.data);
    }

    #[test]
    fn bounds_check_rejects_excessive_or_invalid_subchunk_counts() {
        // > 32 subchunks rejected
        assert_eq!(
            LevelChunkPacket::overworld(0, 0, 33, vec![0]),
            Err(BedrockChunkError::InvalidSubChunkCount(33))
        );
        assert_eq!(
            LevelChunkPacket::overworld(0, 0, 100, vec![0]),
            Err(BedrockChunkError::InvalidSubChunkCount(100))
        );

        let invalid_packet = LevelChunkPacket {
            chunk_x: 0,
            chunk_z: 0,
            dimension_id: 0,
            subchunk_count: 35,
            cache_enabled: false,
            blob_hashes: Vec::new(),
            data: vec![0],
        };
        assert_eq!(
            invalid_packet.encode(),
            Err(BedrockChunkError::InvalidSubChunkCount(35))
        );

        // Construct wire bytes with subchunk_count = 33
        let mut wire = Vec::new();
        write_varuint32(LEVEL_CHUNK_PACKET_ID, &mut wire);
        write_varint32(0, &mut wire);
        write_varint32(0, &mut wire);
        write_varint32(0, &mut wire); // dimension_id
        write_varuint32(33, &mut wire); // invalid count
        wire.push(0); // cache_enabled
        write_varuint32(1, &mut wire); // data len
        wire.push(0); // data
        assert_eq!(
            LevelChunkPacket::decode(&wire),
            Err(BedrockChunkError::InvalidSubChunkCount(33))
        );
    }

    #[test]
    fn bounds_check_rejects_payload_exceeding_limit() {
        let oversized = vec![0u8; MAX_SERIALIZED_CHUNK_SIZE + 1];
        assert_eq!(
            LevelChunkPacket::overworld(0, 0, 24, oversized.clone()),
            Err(BedrockChunkError::PayloadTooLarge {
                size: MAX_SERIALIZED_CHUNK_SIZE + 1,
                limit: MAX_SERIALIZED_CHUNK_SIZE,
            })
        );

        let packet = LevelChunkPacket {
            chunk_x: 0,
            chunk_z: 0,
            dimension_id: 0,
            subchunk_count: 24,
            cache_enabled: false,
            blob_hashes: Vec::new(),
            data: oversized,
        };
        assert_eq!(
            packet.encode(),
            Err(BedrockChunkError::PayloadTooLarge {
                size: MAX_SERIALIZED_CHUNK_SIZE + 1,
                limit: MAX_SERIALIZED_CHUNK_SIZE,
            })
        );
    }

    #[test]
    fn decode_rejects_wrong_packet_id_and_truncated_packet() {
        // Wrong packet ID (0x39 instead of 0x3A)
        let mut wire = Vec::new();
        write_varuint32(0x39, &mut wire);
        write_varint32(0, &mut wire);
        write_varint32(0, &mut wire);
        write_varint32(0, &mut wire);
        write_varuint32(24, &mut wire);
        wire.push(0);
        write_varuint32(0, &mut wire);

        assert_eq!(
            LevelChunkPacket::decode(&wire),
            Err(BedrockChunkError::InvalidPacketId(0x39))
        );

        // Truncated packet
        assert!(LevelChunkPacket::decode(&[0x3a]).is_err());
        assert!(LevelChunkPacket::decode(&[]).is_err());
    }

    #[test]
    fn independently_constructed_wire_header_decodes_expected_fields() {
        // Wire bytes: Packet ID (0x3a), chunk_x: -1 (zigzag 1), chunk_z: 2 (zigzag 4),
        // dimension_id: 0 (zigzag 0), subchunks: 24, cache_enabled: false (0), data len: 4
        let mut wire = vec![0x3a, 0x01, 0x04, 0x00, 24, 0, 4];
        wire.extend_from_slice(&[0xca, 0xfe, 0xba, 0xbe]);

        let decoded = LevelChunkPacket::decode(&wire).unwrap();
        assert_eq!(decoded.chunk_x, -1);
        assert_eq!(decoded.chunk_z, 2);
        assert_eq!(decoded.dimension_id, 0);
        assert_eq!(decoded.subchunk_count, 24);
        assert!(!decoded.cache_enabled);
        assert!(decoded.blob_hashes.is_empty());
        assert_eq!(decoded.data, vec![0xca, 0xfe, 0xba, 0xbe]);
    }

    #[test]
    fn cache_enabled_round_trip_with_blob_hashes() {
        let hashes = vec![0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210];
        let packet =
            LevelChunkPacket::new(5, -3, 0, 16, true, hashes.clone(), vec![1, 2, 3]).unwrap();
        let encoded = packet.encode().unwrap();
        let decoded = LevelChunkPacket::decode(&encoded).unwrap();
        assert_eq!(decoded.chunk_x, 5);
        assert_eq!(decoded.chunk_z, -3);
        assert_eq!(decoded.dimension_id, 0);
        assert_eq!(decoded.subchunk_count, 16);
        assert!(decoded.cache_enabled);
        assert_eq!(decoded.blob_hashes, hashes);
        assert_eq!(decoded.data, vec![1, 2, 3]);
    }

    #[test]
    fn nbt_palette_compounds_round_trip_and_preserve_bytes() {
        // Construct two minimal LE NBT compounds:
        // Compound 1: TAG_Compound (10), name_len 0 (00 00), TAG_End (0) -> [10, 0, 0, 0]
        let comp1 = vec![10, 0, 0, 0];
        // Compound 2: TAG_Compound (10), name_len 0, TAG_Byte (1), name_len 1 ('a'), val 42, TAG_End (0)
        let comp2 = vec![10, 0, 0, 1, 1, 0, b'a', 42, 0];
        let palette = BlockPalette::Nbt(vec![comp1.clone(), comp2.clone()]);
        let storage = BlockStorage::new(1, vec![0u32; 128], palette).unwrap();
        let mut bytes = Vec::new();
        storage.serialize(&mut bytes).unwrap();

        let (decoded, consumed) = BlockStorage::deserialize(&bytes).unwrap();
        assert_eq!(consumed, bytes.len());
        match decoded.palette {
            BlockPalette::Nbt(entries) => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0], comp1);
                assert_eq!(entries[1], comp2);
            }
            _ => panic!("expected Nbt palette"),
        }
    }
}
