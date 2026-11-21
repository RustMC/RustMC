//! Network NBT: the tag model, modified UTF-8 and the bounded reader
//! and writer used by block entity packets.

use super::*;

/// Lightweight, memory-safe NBT Tag representation for block entity payloads.
#[derive(Debug, Clone, PartialEq)]
pub enum NbtTag {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<NbtTag>),
    Compound(Vec<(String, NbtTag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl NbtTag {
    pub fn tag_type_id(&self) -> u8 {
        match self {
            Self::End => 0,
            Self::Byte(_) => 1,
            Self::Short(_) => 2,
            Self::Int(_) => 3,
            Self::Long(_) => 4,
            Self::Float(_) => 5,
            Self::Double(_) => 6,
            Self::ByteArray(_) => 7,
            Self::String(_) => 8,
            Self::List(_) => 9,
            Self::Compound(_) => 10,
            Self::IntArray(_) => 11,
            Self::LongArray(_) => 12,
        }
    }

    pub fn get<'a>(&'a self, key: &str) -> Option<&'a NbtTag> {
        match self {
            Self::Compound(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Byte(b) => Some(*b != 0),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[NbtTag]> {
        match self {
            Self::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_compound(&self) -> Option<&[(String, NbtTag)]> {
        match self {
            Self::Compound(c) => Some(c),
            _ => None,
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(NbtTag::as_str)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(NbtTag::as_bool)
    }

    pub fn get_compound(&self, key: &str) -> Option<&[(String, NbtTag)]> {
        self.get(key).and_then(NbtTag::as_compound)
    }
}

/// Encodes a UTF-8 string into Java Modified UTF-8 bytes for NBT storage.
///
/// Under Java Modified UTF-8:
/// - Code points 1..=127 are 1 byte.
/// - Code point 0 is encoded as `[0xC0, 0x80]`.
/// - Code points 128..=2047 are encoded in 2 bytes.
/// - Code points 2048..=65535 are encoded in 3 bytes.
/// - Supplementary characters (> 65535, e.g. emoji like 😀) are encoded as surrogate pairs,
///   each surrogate occupying 3 bytes, producing a total of 6 bytes.
pub fn encode_modified_utf8(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    java_wire::encode_modified_utf8(s, &mut out);
    out
}

/// Decodes Java Modified UTF-8 bytes into a standard Rust UTF-8 String.
pub fn decode_modified_utf8(bytes: &[u8]) -> Result<String, BlockEntityError> {
    java_wire::decode_modified_utf8(bytes).map_err(map_wire_error)
}

pub(super) fn write_nbt_payload(tag: &NbtTag, out: &mut Vec<u8>) {
    match tag {
        NbtTag::End => {}
        NbtTag::Byte(v) => out.push(*v as u8),
        NbtTag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        NbtTag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        NbtTag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        NbtTag::Float(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        NbtTag::Double(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        NbtTag::ByteArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            out.extend(v.iter().map(|b| *b as u8));
        }
        NbtTag::String(v) => {
            let encoded = encode_modified_utf8(v);
            out.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
            out.extend_from_slice(&encoded);
        }
        NbtTag::List(items) => {
            if items.is_empty() {
                out.push(0);
                out.extend_from_slice(&0i32.to_be_bytes());
            } else {
                let kind = items[0].tag_type_id();
                out.push(kind);
                out.extend_from_slice(&(items.len() as i32).to_be_bytes());
                for item in items {
                    write_nbt_payload(item, out);
                }
            }
        }
        NbtTag::Compound(entries) => {
            for (name, val) in entries {
                out.push(val.tag_type_id());
                let enc_name = encode_modified_utf8(name);
                out.extend_from_slice(&(enc_name.len() as u16).to_be_bytes());
                out.extend_from_slice(&enc_name);
                write_nbt_payload(val, out);
            }
            out.push(0); // TAG_End
        }
        NbtTag::IntArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
        NbtTag::LongArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
    }
}

/// Encodes an anonymous root compound tag into network NBT bytes.
pub fn write_network_nbt(tag: &NbtTag, out: &mut Vec<u8>) {
    out.push(10); // TAG_Compound
    write_nbt_payload(tag, out);
}

/// Encodes a named root compound tag into standard NBT bytes.
pub fn write_named_network_nbt(name: &str, tag: &NbtTag, out: &mut Vec<u8>) {
    out.push(10); // TAG_Compound
    let enc_name = encode_modified_utf8(name);
    out.extend_from_slice(&(enc_name.len() as u16).to_be_bytes());
    out.extend_from_slice(&enc_name);
    write_nbt_payload(tag, out);
}

pub(super) struct NbtReader<'a> {
    pub(super) data: &'a [u8],
    pub(super) pos: usize,
}

impl<'a> NbtReader<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub(super) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub(super) fn read_u8(&mut self) -> Result<u8, BlockEntityError> {
        if self.pos >= self.data.len() {
            return Err(BlockEntityError::UnexpectedEof);
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    pub(super) fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], BlockEntityError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(BlockEntityError::UnexpectedEof)?;
        if end > self.data.len() {
            return Err(BlockEntityError::UnexpectedEof);
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub(super) fn read_u16(&mut self) -> Result<u16, BlockEntityError> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub(super) fn read_i32(&mut self) -> Result<i32, BlockEntityError> {
        let b = self.read_bytes(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(super) fn read_i64(&mut self) -> Result<i64, BlockEntityError> {
        let b = self.read_bytes(8)?;
        Ok(i64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub(super) fn read_string(&mut self) -> Result<String, BlockEntityError> {
        let rest = self
            .data
            .get(self.pos..)
            .ok_or(BlockEntityError::UnexpectedEof)?;
        let (text, consumed) = java_wire::read_nbt_string(rest).map_err(map_wire_error)?;
        self.pos += consumed;
        Ok(text)
    }

    pub(super) fn read_payload(
        &mut self,
        tag_type: u8,
        depth: usize,
    ) -> Result<NbtTag, BlockEntityError> {
        if depth > 64 {
            return Err(BlockEntityError::Malformed("NBT depth exceeds maximum 64"));
        }
        match tag_type {
            0 => Ok(NbtTag::End),
            1 => Ok(NbtTag::Byte(self.read_u8()? as i8)),
            2 => Ok(NbtTag::Short(self.read_u16()? as i16)),
            3 => Ok(NbtTag::Int(self.read_i32()?)),
            4 => Ok(NbtTag::Long(self.read_i64()?)),
            5 => {
                let bits = self.read_i32()? as u32;
                Ok(NbtTag::Float(f32::from_bits(bits)))
            }
            6 => {
                let bits = self.read_i64()? as u64;
                Ok(NbtTag::Double(f64::from_bits(bits)))
            }
            7 => {
                let len = self.read_i32()?;
                if len < 0 {
                    return Err(BlockEntityError::Malformed("negative ByteArray length"));
                }
                let bytes = self.read_bytes(len as usize)?;
                Ok(NbtTag::ByteArray(bytes.iter().map(|&b| b as i8).collect()))
            }
            8 => Ok(NbtTag::String(self.read_string()?)),
            9 => {
                let elem_type = self.read_u8()?;
                let len = self.read_i32()?;
                if len < 0 {
                    return Err(BlockEntityError::Malformed("negative List length"));
                }
                if elem_type == 0 && len > 0 {
                    return Err(BlockEntityError::Malformed("nonempty TAG_End list"));
                }
                let count = len as usize;
                let mut items = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    items.push(self.read_payload(elem_type, depth + 1)?);
                }
                Ok(NbtTag::List(items))
            }
            10 => {
                let mut entries = Vec::new();
                loop {
                    let child_type = self.read_u8()?;
                    if child_type == 0 {
                        break; // TAG_End
                    }
                    let name = self.read_string()?;
                    let val = self.read_payload(child_type, depth + 1)?;
                    entries.push((name, val));
                }
                Ok(NbtTag::Compound(entries))
            }
            11 => {
                let len = self.read_i32()?;
                if len < 0 {
                    return Err(BlockEntityError::Malformed("negative IntArray length"));
                }
                let count = len as usize;
                let mut ints = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    ints.push(self.read_i32()?);
                }
                Ok(NbtTag::IntArray(ints))
            }
            12 => {
                let len = self.read_i32()?;
                if len < 0 {
                    return Err(BlockEntityError::Malformed("negative LongArray length"));
                }
                let count = len as usize;
                let mut longs = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    longs.push(self.read_i64()?);
                }
                Ok(NbtTag::LongArray(longs))
            }
            _ => Err(BlockEntityError::Malformed("unknown NBT tag type")),
        }
    }
}

/// Decodes one anonymous root network NBT tag (without a root name), returning the tag
/// and the number of bytes consumed from `data`.
pub fn read_anonymous_network_nbt(data: &[u8]) -> Result<(NbtTag, usize), BlockEntityError> {
    let mut reader = NbtReader::new(data);
    let root_type = reader.read_u8()?;
    if root_type == 0 {
        return Err(BlockEntityError::Malformed(
            "unexpected TAG_End as root network NBT tag",
        ));
    }
    let tag = reader.read_payload(root_type, 0)?;
    Ok((tag, reader.pos))
}

/// Decodes network NBT from bytes, accepting both anonymous and named root compounds.
pub fn read_network_nbt(data: &[u8]) -> Result<NbtTag, BlockEntityError> {
    let mut reader = NbtReader::new(data);
    let root_type = reader.read_u8()?;
    if root_type == 0 {
        return Ok(NbtTag::Compound(Vec::new()));
    }
    if root_type != 10 {
        return Err(BlockEntityError::Malformed(
            "root NBT tag must be Compound (10)",
        ));
    }

    // Check if this is a named root compound (has u16 name_len + name bytes).
    // In anonymous network NBT, the next byte is either TAG_End (0) or a child tag type (1..=12).
    // If the next byte is 0 and there are 2+ bytes available with data following,
    // or if the first 2 bytes declare a name length, consume the root name.
    if reader.remaining() >= 2 && reader.data[reader.pos] == 0 {
        let name_len = reader.read_u16()? as usize;
        let _root_name = reader.read_bytes(name_len)?;
    }

    let mut entries = Vec::new();
    loop {
        let child_type = reader.read_u8()?;
        if child_type == 0 {
            break;
        }
        let name = reader.read_string()?;
        let val = reader.read_payload(child_type, 1)?;
        entries.push((name, val));
    }
    Ok(NbtTag::Compound(entries))
}

/// Decodes a named root compound tag from standard NBT bytes, returning the root name and tag.
pub fn read_named_network_nbt(data: &[u8]) -> Result<(String, NbtTag), BlockEntityError> {
    let mut reader = NbtReader::new(data);
    let root_type = reader.read_u8()?;
    if root_type == 0 {
        return Ok((String::new(), NbtTag::Compound(Vec::new())));
    }
    if root_type != 10 {
        return Err(BlockEntityError::Malformed(
            "root NBT tag must be Compound (10)",
        ));
    }
    let root_name = reader.read_string()?;
    let mut entries = Vec::new();
    loop {
        let child_type = reader.read_u8()?;
        if child_type == 0 {
            break;
        }
        let name = reader.read_string()?;
        let val = reader.read_payload(child_type, 1)?;
        entries.push((name, val));
    }
    Ok((root_name, NbtTag::Compound(entries)))
}

pub(super) fn map_wire_error(err: WireError) -> BlockEntityError {
    match err {
        WireError::Truncated => BlockEntityError::UnexpectedEof,
        WireError::VarIntTooLong => BlockEntityError::Malformed("VarInt overflow"),
        WireError::NonCanonicalVarInt => BlockEntityError::Malformed("non-canonical VarInt"),
        WireError::TooLong { length, .. } => BlockEntityError::LineTooLong(length),
        WireError::InvalidText => BlockEntityError::Malformed("invalid text encoding"),
        WireError::BelowCompressionThreshold { .. }
        | WireError::InvalidCompression
        | WireError::DecompressedSizeMismatch { .. } => {
            BlockEntityError::Malformed("invalid compressed framing")
        }
    }
}

pub(super) fn read_varint(input: &[u8]) -> Result<(u32, usize), BlockEntityError> {
    java_wire::read_varint(input)
        .map_err(map_wire_error)?
        .ok_or(BlockEntityError::UnexpectedEof)
}
