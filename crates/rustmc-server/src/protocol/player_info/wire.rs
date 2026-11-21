//! Wire helpers for player info: VarInts, strings, UUIDs, byte arrays
//! and the bounded NBT reader for display names.

use super::*;

/// Computes the number of bytes required to serialize `value` as a VarInt.
#[inline]
pub fn varint_len(value: u32) -> usize {
    let mut len = 1;
    let mut v = value >> 7;
    while v != 0 {
        len += 1;
        v >>= 7;
    }
    len
}

/// Encodes a 32-bit signed integer as a canonical VarInt into `out`.
pub fn write_varint(value: i32, out: &mut Vec<u8>) {
    put_varint(value as u32, out);
}

/// Decodes a 32-bit signed integer from a canonical VarInt stream.
///
/// Rejects non-canonical encodings with superfluous trailing zero bytes (e.g. `[0x80, 0x00]`)
/// and malformed sequences exceeding 5 bytes or containing illegal upper bits.
pub fn read_varint(input: &[u8]) -> Result<(i32, usize), ProtocolError> {
    match java_wire::read_varint(input) {
        Ok(Some((value, bytes_read))) => Ok((value as i32, bytes_read)),
        Ok(None) => Err(ProtocolError::UnexpectedEof),
        Err(WireError::NonCanonicalVarInt) => Err(ProtocolError::NonCanonicalVarInt),
        Err(_) => Err(ProtocolError::MalformedVarInt),
    }
}

/// Writes a boolean value as a single byte (0x01 for true, 0x00 for false).
#[inline]
pub fn write_bool(val: bool, out: &mut Vec<u8>) {
    out.push(if val { 1 } else { 0 });
}

/// Reads a boolean value from the stream, strictly rejecting any byte other than 0 or 1.
pub fn read_bool(input: &[u8]) -> Result<(bool, usize), ProtocolError> {
    if input.is_empty() {
        return Err(ProtocolError::UnexpectedEof);
    }
    match input[0] {
        0 => Ok((false, 1)),
        1 => Ok((true, 1)),
        other => Err(ProtocolError::InvalidBoolean(other)),
    }
}

/// Writes a 16-byte UUID into `out`.
#[inline]
pub fn write_uuid(uuid: &[u8; 16], out: &mut Vec<u8>) {
    out.extend_from_slice(uuid);
}

/// Reads a 16-byte UUID from `input`.
pub fn read_uuid(input: &[u8]) -> Result<([u8; 16], usize), ProtocolError> {
    if input.len() < 16 {
        return Err(ProtocolError::UnexpectedEof);
    }
    let mut uuid = [0u8; 16];
    uuid.copy_from_slice(&input[..16]);
    Ok((uuid, 16))
}

/// Writes a VarInt-prefixed UTF-8 string into `out`.
pub fn write_string(s: &str, out: &mut Vec<u8>) {
    put_string(s, out);
}

/// Rejects a collection or byte buffer longer than `max`.
pub(super) fn check_length(length: usize, max: usize) -> Result<(), ProtocolError> {
    if length > max {
        return Err(ProtocolError::StringTooLong { length, max });
    }
    Ok(())
}

/// Rejects a string over `max_chars`, a limit the protocol counts in UTF-16 code units.
pub(super) fn check_chars(value: &str, max_chars: usize) -> Result<(), ProtocolError> {
    check_length(value.encode_utf16().count(), max_chars)
}

/// Reads a VarInt-prefixed UTF-8 string bounded by `max_chars`.
pub fn read_string(input: &[u8], max_chars: usize) -> Result<(String, usize), ProtocolError> {
    let max_bytes = max_chars.saturating_mul(3).min(i32::MAX as usize);
    let (s, rest) = java_wire::read_string(input, max_bytes).map_err(|err| match err {
        WireError::Truncated => ProtocolError::UnexpectedEof,
        WireError::VarIntTooLong => ProtocolError::MalformedVarInt,
        WireError::NonCanonicalVarInt => ProtocolError::NonCanonicalVarInt,
        WireError::TooLong { length, .. } if length > i32::MAX as usize => {
            ProtocolError::MalformedVarInt
        }
        WireError::TooLong { length, .. } => ProtocolError::StringTooLong {
            length,
            max: max_chars,
        },
        WireError::InvalidText => ProtocolError::InvalidUtf8,
        WireError::BelowCompressionThreshold { length, threshold } => {
            ProtocolError::InvalidPacketLength {
                expected: threshold,
                received: length,
            }
        }
        WireError::InvalidCompression | WireError::DecompressedSizeMismatch { .. } => {
            ProtocolError::MalformedVarInt
        }
    })?;
    check_chars(s, max_chars)?;
    Ok((s.to_owned(), input.len() - rest.len()))
}

/// Writes a VarInt-prefixed byte array into `out`.
pub fn write_byte_array(bytes: &[u8], out: &mut Vec<u8>) {
    write_varint(bytes.len() as i32, out);
    out.extend_from_slice(bytes);
}

/// Reads a VarInt-prefixed byte array bounded by `max_len`.
pub fn read_byte_array(input: &[u8], max_len: usize) -> Result<(Vec<u8>, usize), ProtocolError> {
    let (byte_len, offset) = read_varint(input)?;
    if byte_len < 0 {
        return Err(ProtocolError::MalformedVarInt);
    }
    let byte_len = byte_len as usize;
    if byte_len > max_len {
        return Err(ProtocolError::StringTooLong {
            length: byte_len,
            max: max_len,
        });
    }
    let total = offset + byte_len;
    if input.len() < total {
        return Err(ProtocolError::UnexpectedEof);
    }
    Ok((input[offset..total].to_vec(), total))
}

/// Helper to skip the payload of an NBT tag by type ID, validating its structure.
pub fn skip_nbt_tag_payload(tag_type: u8, input: &[u8]) -> Result<usize, ProtocolError> {
    skip_nbt_payload(tag_type, input, 0)
}

/// Rejects a nested tag past [`MAX_NBT_DEPTH`] before it can recurse further.
pub(super) fn check_nbt_depth(depth: usize) -> Result<(), ProtocolError> {
    if depth > MAX_NBT_DEPTH {
        return Err(ProtocolError::NbtDepthExceeded {
            depth,
            max: MAX_NBT_DEPTH,
        });
    }
    Ok(())
}

/// Reads a TAG_String payload: an unsigned 16-bit byte length and modified UTF-8.
pub(super) fn read_nbt_string(input: &[u8]) -> Result<(String, usize), ProtocolError> {
    java_wire::read_nbt_string(input).map_err(|err| match err {
        WireError::Truncated => ProtocolError::UnexpectedEof,
        _ => ProtocolError::InvalidUtf8,
    })
}

pub(super) fn skip_nbt_payload(
    tag_type: u8,
    input: &[u8],
    depth: usize,
) -> Result<usize, ProtocolError> {
    match tag_type {
        1 => {
            // TAG_Byte
            if input.is_empty() {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(1)
        }
        2 => {
            // TAG_Short
            if input.len() < 2 {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(2)
        }
        3 => {
            // TAG_Int
            if input.len() < 4 {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(4)
        }
        4 => {
            // TAG_Long
            if input.len() < 8 {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(8)
        }
        5 => {
            // TAG_Float
            if input.len() < 4 {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(4)
        }
        6 => {
            // TAG_Double
            if input.len() < 8 {
                return Err(ProtocolError::UnexpectedEof);
            }
            Ok(8)
        }
        7 => {
            // TAG_Byte_Array
            skip_nbt_array(input, 1)
        }
        8 => {
            // TAG_String
            Ok(read_nbt_string(input)?.1)
        }
        9 => {
            // TAG_List
            check_nbt_depth(depth + 1)?;
            let (&elem_type, rest) = input.split_first().ok_or(ProtocolError::UnexpectedEof)?;
            let (count, _) =
                java_wire::read_i32_be(rest).map_err(|_| ProtocolError::UnexpectedEof)?;
            if count < 0 {
                return Err(ProtocolError::InvalidPacketLength {
                    expected: 0,
                    received: count as usize,
                });
            }
            let mut cursor = 5;
            for _ in 0..count {
                let n = skip_nbt_payload(elem_type, &input[cursor..], depth + 1)?;
                cursor += n;
            }
            Ok(cursor)
        }
        10 => {
            // TAG_Compound
            check_nbt_depth(depth + 1)?;
            let mut cursor = 0;
            loop {
                let item_tag = *input.get(cursor).ok_or(ProtocolError::UnexpectedEof)?;
                cursor += 1;
                if item_tag == 0 {
                    break;
                }
                cursor += read_nbt_string(&input[cursor..])?.1;
                cursor += skip_nbt_payload(item_tag, &input[cursor..], depth + 1)?;
            }
            Ok(cursor)
        }
        11 => {
            // TAG_Int_Array
            skip_nbt_array(input, 4)
        }
        12 => {
            // TAG_Long_Array
            skip_nbt_array(input, 8)
        }
        other => Err(ProtocolError::InvalidNbtTag(other)),
    }
}

pub(super) fn skip_nbt_array(input: &[u8], elem_bytes: usize) -> Result<usize, ProtocolError> {
    let (len, _) = java_wire::read_i32_be(input).map_err(|_| ProtocolError::UnexpectedEof)?;
    if len < 0 {
        return Err(ProtocolError::InvalidPacketLength {
            expected: 0,
            received: len as usize,
        });
    }
    let total = 4 + (len as usize * elem_bytes);
    if input.len() < total {
        return Err(ProtocolError::UnexpectedEof);
    }
    Ok(total)
}

/// Serializes a display name as an anonymous Network NBT text component.
pub fn write_display_name_nbt(
    name: &PlayerDisplayName,
    out: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
    match name {
        PlayerDisplayName::Plain(text) => {
            check_chars(text, MAX_STRING_CHARS)?;
            put_nbt_text(text, out).map_err(|err| match err {
                WireError::TooLong { length, max } => ProtocolError::StringTooLong { length, max },
                _ => ProtocolError::InvalidUtf8,
            })?;
        }
        PlayerDisplayName::RawNbt { raw, .. } => {
            let (_, consumed) = read_display_name_nbt(raw, MAX_STRING_CHARS)?;
            if consumed != raw.len() {
                return Err(ProtocolError::TrailingData {
                    remaining: raw.len() - consumed,
                });
            }
            out.extend_from_slice(raw);
        }
    }
    Ok(())
}

/// Reads an anonymous Network NBT text component display name.
///
/// A TAG_String (0x08) becomes [`PlayerDisplayName::Plain`]. A TAG_Compound (0x0A)
/// is validated and kept byte for byte as [`PlayerDisplayName::RawNbt`], so a later
/// encode cannot lose styling, `extra` children or translations.
pub fn read_display_name_nbt(
    input: &[u8],
    max_chars: usize,
) -> Result<(PlayerDisplayName, usize), ProtocolError> {
    let (tag, body) = input.split_first().ok_or(ProtocolError::UnexpectedEof)?;
    match *tag {
        0x08 => {
            let (text, consumed) = read_nbt_string(body)?;
            check_chars(&text, max_chars)?;
            Ok((PlayerDisplayName::Plain(text), 1 + consumed))
        }
        0x0A => {
            if body.first() == Some(&0) {
                return Err(ProtocolError::UnsupportedComponent);
            }
            let mut text = String::new();
            let consumed = read_component_compound(body, 1, &mut text)?;
            check_chars(&text, max_chars)?;
            let total = 1 + consumed;
            Ok((
                PlayerDisplayName::RawNbt {
                    raw: input[..total].to_vec(),
                    text,
                },
                total,
            ))
        }
        other => Err(ProtocolError::InvalidNbtTag(other)),
    }
}

/// Validates one component compound and appends its literal text to `text`, its
/// own `text` field first and then its `extra` children, whatever the tag order.
pub(super) fn read_component_compound(
    input: &[u8],
    depth: usize,
    text: &mut String,
) -> Result<usize, ProtocolError> {
    check_nbt_depth(depth)?;
    let mut own = String::new();
    let mut children = String::new();
    let mut cursor = 0;
    loop {
        let tag = *input.get(cursor).ok_or(ProtocolError::UnexpectedEof)?;
        cursor += 1;
        if tag == 0 {
            break;
        }
        let (name, consumed) = read_nbt_string(&input[cursor..])?;
        cursor += consumed;
        cursor += match (tag, name.as_str()) {
            (0x08, "text") => {
                let (value, consumed) = read_nbt_string(&input[cursor..])?;
                own = value;
                consumed
            }
            (0x09, "extra") => read_component_list(&input[cursor..], depth, &mut children)?,
            _ => skip_nbt_payload(tag, &input[cursor..], depth)?,
        };
    }
    text.push_str(&own);
    text.push_str(&children);
    Ok(cursor)
}

/// Validates an `extra` list and appends the literal text of its children.
pub(super) fn read_component_list(
    input: &[u8],
    depth: usize,
    text: &mut String,
) -> Result<usize, ProtocolError> {
    check_nbt_depth(depth + 1)?;
    let (element, rest) = input.split_first().ok_or(ProtocolError::UnexpectedEof)?;
    let (count, _) = rest
        .split_first_chunk::<4>()
        .ok_or(ProtocolError::UnexpectedEof)?;
    let count = i32::from_be_bytes(*count);
    if count < 0 {
        return Err(ProtocolError::MalformedVarInt);
    }
    let mut cursor = 5;
    for _ in 0..count {
        cursor += match *element {
            0x08 => {
                let (value, consumed) = read_nbt_string(&input[cursor..])?;
                text.push_str(&value);
                consumed
            }
            0x0A => read_component_compound(&input[cursor..], depth + 1, text)?,
            other => skip_nbt_payload(other, &input[cursor..], depth + 1)?,
        };
    }
    Ok(cursor)
}
