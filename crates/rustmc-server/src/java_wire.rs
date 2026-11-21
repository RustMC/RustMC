//! Java Edition wire encoders and decoders shared by RustMC packet code.
//!
//! Cached preview chunk packets are built with these functions, so this file is
//! part of the packet-cache fingerprint in `build.rs`.

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::{Compression, Decompress, FlushDecompress, Status};

/// Maximum number of bytes in a 32-bit protocol VarInt.
const MAX_VARINT_BYTES: usize = 5;
/// Top four bits of the fifth VarInt byte that overflow `u32` or continue past five bytes.
const FIFTH_BYTE_DISALLOWED_BITS: u8 = 0xf0;
/// Maximum on-wire `Packet Length` value (`2^21 - 1` = `2_097_151` bytes, the
/// maximum representable in a 3-byte `VarInt`).
///
/// Quote from `https://minecraft.wiki/w/Java_Edition_protocol/Packets`
/// (revision 3868170, section "Packet format"):
/// "Packets cannot be larger than 2^21 − 1 or 2097151 bytes (the maximum that
/// can be sent in a 3-byte VarInt). Moreover, the length field must not be
/// longer than 3 bytes, even if the encoded value is within the limit."
pub const MAX_PACKET_WIRE_BYTES: usize = (1 << 21) - 1;
/// Maximum length of the outer `Packet Length` VarInt in bytes.
const MAX_PACKET_LENGTH_VARINT_BYTES: usize = 3;
/// Maximum uncompressed `(Packet ID + Data)` length permitted for serverbound
/// compressed-format packets (`2^23` = `8_388_608` bytes, inclusive).
///
/// Quote from `https://minecraft.wiki/w/Java_Edition_protocol/Packets`
/// (revision 3868170, section "With compression"):
/// "For serverbound packets, the uncompressed length of (Packet ID + Data) must
/// not be greater than 2^23 or 8388608 bytes. Note that a length equal to 2^23
/// is permitted, which differs from the compressed length limit."
pub const MAX_UNCOMPRESSED_PACKET_BYTES: usize = 1 << 23;
/// Incremental buffer growth cap when inflating a large declared payload.
const INFLATE_GROWTH_CHUNK: usize = 64 * 1024;

/// Why bytes cannot be decoded, however much more input arrives (except `Truncated`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    /// A length-prefixed value ends past the end of the input.
    Truncated,
    /// A VarInt runs past five bytes or sets bits above 32.
    VarIntTooLong,
    /// A VarInt ends with a redundant zero group, such as `[0x80, 0x00]`.
    NonCanonicalVarInt,
    /// A length exceeds the caller's bound of `max` bytes.
    TooLong { length: usize, max: usize },
    /// Bytes are not valid UTF-8 or modified UTF-8.
    InvalidText,
    /// A compressed packet declared a non-zero uncompressed length below `threshold`.
    BelowCompressionThreshold { length: usize, threshold: usize },
    /// A zlib stream is truncated, corrupt, or has trailing bytes, or a frame is malformed.
    InvalidCompression,
    /// A zlib payload inflated to fewer or more bytes than `declared`.
    DecompressedSizeMismatch { declared: usize },
}

/// Most modified UTF-8 bytes an NBT string's u16 length prefix can describe.
pub(crate) const MAX_NBT_STRING_BYTES: usize = u16::MAX as usize;
/// NBT tag type of a string; a plain network text component is one bare string tag.
pub(crate) const NBT_TAG_STRING: u8 = 8;

/// Bit width of packed horizontal (X and Z) block coordinates.
const BLOCK_XZ_BITS: u32 = 26;
/// Bit width of packed vertical (Y) block coordinates.
const BLOCK_Y_BITS: u32 = 12;
const BLOCK_Z_SHIFT: u32 = BLOCK_Y_BITS;
const BLOCK_X_SHIFT: u32 = BLOCK_Y_BITS + BLOCK_XZ_BITS;
const BLOCK_XZ_MASK: i64 = (1_i64 << BLOCK_XZ_BITS) - 1;
const BLOCK_Y_MASK: i64 = (1_i64 << BLOCK_Y_BITS) - 1;
const BLOCK_Y_SIGN_EXTEND_SHIFT: u32 = i64::BITS - BLOCK_Y_BITS;
const BLOCK_Z_SIGN_EXTEND_SHIFT: u32 = i64::BITS - BLOCK_XZ_BITS;
/// Bits above the Z field, holding X; shifted out before sign-extending Z.
const BLOCK_Z_HIGH_BITS: u32 = i64::BITS - BLOCK_X_SHIFT;

/// Append `value` as a protocol VarInt: seven bits per byte, low bits first.
pub(crate) fn put_varint(mut value: u32, out: &mut Vec<u8>) {
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

/// Append a VarLong: seven bits per byte, low groups first.
pub(crate) fn put_varlong(mut value: u64, out: &mut Vec<u8>) {
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

/// Frame one uncompressed packet: VarInt length, then VarInt ID and body.
pub(crate) fn frame(id: u32, body: &[u8]) -> Vec<u8> {
    let mut packet = Vec::new();
    put_varint(id, &mut packet);
    packet.extend_from_slice(body);
    let mut out = Vec::new();
    put_varint(packet.len() as u32, &mut out);
    out.extend(packet);
    out
}

/// Encode a `u32` as a VarInt on the stack, returning `(bytes, byte_len)`.
const fn encode_varint_bytes(mut value: u32) -> ([u8; MAX_VARINT_BYTES], usize) {
    let mut buf = [0u8; MAX_VARINT_BYTES];
    let mut len = 0;
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf[len] = byte;
        len += 1;
        if value == 0 {
            break;
        }
    }
    (buf, len)
}

/// Frame an already-serialized `[VarInt(id), body...]` packet slice (split across
/// `parts` so callers can avoid copying) using the post-`Set Compression` format.
fn frame_compressed_parts(
    parts: [&[u8]; 2],
    threshold: usize,
    out: &mut Vec<u8>,
) -> Result<(), WireError> {
    let Some(uncompressed_len) = parts[0].len().checked_add(parts[1].len()) else {
        return Err(WireError::TooLong {
            length: usize::MAX,
            max: MAX_UNCOMPRESSED_PACKET_BYTES,
        });
    };
    let start = out.len();
    let result = (|| {
        if uncompressed_len < threshold {
            let packet_len = 1usize
                .checked_add(uncompressed_len)
                .filter(|&len| len <= MAX_PACKET_WIRE_BYTES)
                .ok_or(WireError::TooLong {
                    length: uncompressed_len.saturating_add(1),
                    max: MAX_PACKET_WIRE_BYTES,
                })?;
            let (_, prefix_len) = encode_varint_bytes(packet_len as u32);
            out.reserve(prefix_len + packet_len);
            put_varint(packet_len as u32, out);
            put_varint(0, out);
            out.extend_from_slice(parts[0]);
            out.extend_from_slice(parts[1]);
            return Ok(());
        }

        let Ok(data_len_i32) = i32::try_from(uncompressed_len) else {
            return Err(WireError::TooLong {
                length: uncompressed_len,
                max: i32::MAX as usize,
            });
        };
        let data_len_u32 = data_len_i32 as u32;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(parts[0])
            .map_err(|_| WireError::InvalidCompression)?;
        encoder
            .write_all(parts[1])
            .map_err(|_| WireError::InvalidCompression)?;
        let compressed = encoder
            .finish()
            .map_err(|_| WireError::InvalidCompression)?;

        let (_, data_len_prefix) = encode_varint_bytes(data_len_u32);
        let packet_len = data_len_prefix
            .checked_add(compressed.len())
            .filter(|&len| len <= MAX_PACKET_WIRE_BYTES)
            .ok_or(WireError::TooLong {
                length: data_len_prefix.saturating_add(compressed.len()),
                max: MAX_PACKET_WIRE_BYTES,
            })?;
        let (_, prefix_len) = encode_varint_bytes(packet_len as u32);
        out.reserve(prefix_len + packet_len);
        put_varint(packet_len as u32, out);
        put_varint(data_len_u32, out);
        out.extend_from_slice(&compressed);
        Ok(())
    })();
    if result.is_err() {
        out.truncate(start);
    }
    result
}

/// Frame one packet using the Java 26.3 (protocol 777) post-`Set Compression` format
/// and append it to `out`.
///
/// Follows `https://minecraft.wiki/w/Java_Edition_protocol/Packets`
/// (revision 3868170, section "With compression") and
/// `https://minecraft.wiki/w/Java_Edition_protocol/Data_types` (revision 3855787,
/// `VarInt` signed 32-bit two's complement, 1–5 bytes):
///
/// | Present? | Compressed? | Field Name | Field Type | Notes |
/// | --- | --- | --- | --- | --- |
/// | always | No | Packet Length | VarInt | Length of (Data Length) + length of compressed (Packet ID + Data) |
/// | if size >= threshold | No | Data Length | VarInt | Length of uncompressed (Packet ID + Data) |
/// | if size >= threshold | Yes | Packet ID | VarInt | zlib compressed packet ID |
/// | if size >= threshold | Yes | Data | Byte Array | zlib compressed packet data |
/// | if size < threshold | No | Data Length | VarInt | 0 to indicate uncompressed |
/// | if size < threshold | No | Packet ID | VarInt | packet ID |
/// | if size < threshold | No | Data | Byte Array | packet data |
///
/// Packets whose uncompressed `Packet ID + Data` length is strictly below
/// `threshold` are emitted uncompressed with `Data Length = 0`. Leaves `out`
/// unmodified on error.
pub fn frame_compressed(
    id: u32,
    body: &[u8],
    threshold: usize,
    out: &mut Vec<u8>,
) -> Result<(), WireError> {
    let (id_bytes, id_len) = encode_varint_bytes(id);
    frame_compressed_parts([&id_bytes[..id_len], body], threshold, out)
}

/// Re-frame an uncompressed-framed packet (`[VarInt(len), VarInt(id), body...]`)
/// into `out` using the post-`Set Compression` format (`threshold`).
///
/// Leaves `out` unmodified on error.
pub fn reframe_compressed(
    uncompressed_frame: &[u8],
    threshold: usize,
    out: &mut Vec<u8>,
) -> Result<(), WireError> {
    let Some((len, prefix)) = read_varint(uncompressed_frame)? else {
        return Err(WireError::Truncated);
    };
    let len = len as usize;
    if len == 0 || prefix + len != uncompressed_frame.len() {
        return Err(WireError::InvalidCompression);
    }
    frame_compressed_parts([&uncompressed_frame[prefix..], &[]], threshold, out)
}

/// Encode the login-phase `Set Compression` clientbound packet (`0x03`).
///
/// Sent in the uncompressed packet format before `Login Success`; compression
/// applies to all packets following `Set Compression` when `threshold >= 0`.
///
/// Follows `https://minecraft.wiki/w/Java_Edition_protocol/Packets`
/// (revision 3868170, section "Set Compression") and
/// `https://minecraft.wiki/w/Java_Edition_protocol/Data_types` (revision 3855787):
///
/// | Field Name | Field Type | Notes |
/// | --- | --- | --- |
/// | Threshold | VarInt | Maximum size of a packet before it is compressed. |
///
/// Negative values disable compression. Packet ID comes from
/// [`crate::java_packets::login::clientbound::SET_COMPRESSION`].
pub fn encode_set_compression(threshold: i32) -> Vec<u8> {
    let mut body = Vec::with_capacity(MAX_VARINT_BYTES);
    put_varint(threshold as u32, &mut body);
    frame(
        crate::java_packets::login::clientbound::SET_COMPRESSION,
        &body,
    )
}

/// Inflate a complete zlib stream (`compressed`) into `out`, enforcing exact
/// size `declared_len` without ever growing `out.capacity()` beyond `declared_len`.
fn inflate_exact_into(
    compressed: &[u8],
    declared_len: usize,
    out: &mut Vec<u8>,
) -> Result<(), WireError> {
    out.clear();
    out.reserve_exact(declared_len.min(INFLATE_GROWTH_CHUNK));
    let mut inflater = Decompress::new(true);
    loop {
        let consumed = usize::try_from(inflater.total_in()).unwrap_or(usize::MAX);
        let remaining = compressed.get(consumed..).unwrap_or_default();
        if out.len() < declared_len {
            if out.len() == out.capacity() {
                let add = out
                    .capacity()
                    .max(1024)
                    .min(declared_len.saturating_sub(out.len()));
                out.reserve_exact(add);
            }
            let produced_before = out.len();
            let status = inflater
                .decompress_vec(remaining, out, FlushDecompress::None)
                .map_err(|_| WireError::InvalidCompression)?;
            if out.len() > declared_len {
                return Err(WireError::DecompressedSizeMismatch {
                    declared: declared_len,
                });
            }
            if status == Status::StreamEnd {
                if out.len() != declared_len {
                    return Err(WireError::DecompressedSizeMismatch {
                        declared: declared_len,
                    });
                }
                if usize::try_from(inflater.total_in()).ok() != Some(compressed.len()) {
                    return Err(WireError::InvalidCompression);
                }
                return Ok(());
            }
            let consumed_now = usize::try_from(inflater.total_in()).unwrap_or(usize::MAX);
            if consumed_now == consumed
                && out.len() == produced_before
                && out.len() < out.capacity()
            {
                return Err(WireError::InvalidCompression);
            }
        } else {
            // Output reached `declared_len`, so probe only into a 1-byte stack
            // buffer to finish reading the zlib Adler-32 trailer or reject
            // streams that attempt to inflate past `declared_len`.
            let mut extra = [0u8; 1];
            let total_out_before = inflater.total_out();
            let status = inflater
                .decompress(remaining, &mut extra, FlushDecompress::None)
                .map_err(|_| WireError::InvalidCompression)?;
            if inflater.total_out() > total_out_before {
                return Err(WireError::DecompressedSizeMismatch {
                    declared: declared_len,
                });
            }
            if status == Status::StreamEnd {
                if usize::try_from(inflater.total_in()).ok() != Some(compressed.len()) {
                    return Err(WireError::InvalidCompression);
                }
                return Ok(());
            }
            let consumed_now = usize::try_from(inflater.total_in()).unwrap_or(usize::MAX);
            if consumed_now == consumed {
                return Err(WireError::InvalidCompression);
            }
        }
    }
}

/// Decode one compressed-format Java 26.3 (protocol 777) packet frame from
/// `input`, returning `(packet_id, body, remaining_input)`.
///
/// Follows `https://minecraft.wiki/w/Java_Edition_protocol/Packets`
/// (revision 3868170, sections "Packet format" and "With compression"):
///
/// - Rejects `Packet Length` above `2_097_151` (`2^21 - 1`) or encoded in more
///   than 3 VarInt bytes.
/// - Rejects declared `Data Length` above `max_len.min(8_388_608)` (`2^23` bytes,
///   the wiki's serverbound uncompressed maximum).
/// - Rejects compressed packets (`Data Length > 0`) whose declared uncompressed
///   length is strictly below `threshold` ("The vanilla server (but not client)
///   rejects compressed packets smaller than the threshold. Uncompressed packets
///   exceeding the threshold, however, are accepted.").
/// - Bounded decompression: never allocates more than the validated declared
///   uncompressed size and rejects truncated, corrupt, trailing-byte, or
///   over-inflating zlib streams.
pub fn read_compressed_frame(
    input: &[u8],
    threshold: usize,
    max_len: usize,
) -> Result<(u32, Vec<u8>, &[u8]), WireError> {
    let Some((packet_len_u32, prefix)) = read_varint(input)? else {
        return Err(WireError::Truncated);
    };
    let packet_len = packet_len_u32 as usize;
    if packet_len > MAX_PACKET_WIRE_BYTES {
        return Err(WireError::TooLong {
            length: packet_len,
            max: MAX_PACKET_WIRE_BYTES,
        });
    }
    if prefix > MAX_PACKET_LENGTH_VARINT_BYTES {
        return Err(WireError::VarIntTooLong);
    }
    if packet_len == 0 {
        return Err(WireError::InvalidCompression);
    }
    let after_prefix = &input[prefix..];
    if after_prefix.len() < packet_len {
        return Err(WireError::Truncated);
    }
    let (frame_bytes, remaining) = after_prefix.split_at(packet_len);

    let Some((data_len_u32, dl_len)) = read_varint(frame_bytes)? else {
        return Err(WireError::InvalidCompression);
    };
    if i32::try_from(data_len_u32).is_err() {
        return Err(WireError::InvalidCompression);
    }
    let payload = &frame_bytes[dl_len..];
    let effective_max = max_len.min(MAX_UNCOMPRESSED_PACKET_BYTES);
    let uncompressed = if data_len_u32 == 0 {
        if payload.len() > effective_max {
            return Err(WireError::TooLong {
                length: payload.len(),
                max: effective_max,
            });
        }
        payload.to_vec()
    } else {
        let declared = data_len_u32 as usize;
        if declared > effective_max {
            return Err(WireError::TooLong {
                length: declared,
                max: effective_max,
            });
        }
        if declared < threshold {
            return Err(WireError::BelowCompressionThreshold {
                length: declared,
                threshold,
            });
        }
        if payload.is_empty() {
            return Err(WireError::InvalidCompression);
        }
        let mut decompressed = Vec::new();
        inflate_exact_into(payload, declared, &mut decompressed)?;
        decompressed
    };

    let Some((id, id_len)) = read_varint(&uncompressed)? else {
        return Err(WireError::InvalidCompression);
    };
    let body = uncompressed[id_len..].to_vec();
    Ok((id, body, remaining))
}

/// Append a protocol string: VarInt byte length, then UTF-8 bytes.
pub(crate) fn put_string(value: &str, out: &mut Vec<u8>) {
    put_varint(value.len() as u32, out);
    out.extend_from_slice(value.as_bytes());
}

/// Decode a protocol VarInt; returns `Ok(None)` when more input bytes are needed.
pub(crate) fn read_varint(input: &[u8]) -> Result<Option<(u32, usize)>, WireError> {
    let mut value = 0u32;
    for (i, byte) in input.iter().take(MAX_VARINT_BYTES).enumerate() {
        if i == MAX_VARINT_BYTES - 1 && byte & FIFTH_BYTE_DISALLOWED_BITS != 0 {
            return Err(WireError::VarIntTooLong);
        }
        value |= u32::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            if i > 0 && *byte == 0 {
                return Err(WireError::NonCanonicalVarInt);
            }
            return Ok(Some((value, i + 1)));
        }
    }
    if input.len() >= MAX_VARINT_BYTES {
        Err(WireError::VarIntTooLong)
    } else {
        Ok(None)
    }
}

/// Decode a protocol string (VarInt byte length, then UTF-8) bounded by `max_bytes`.
pub(crate) fn read_string(input: &[u8], max_bytes: usize) -> Result<(&str, &[u8]), WireError> {
    let Some((length, prefix)) = read_varint(input)? else {
        return Err(WireError::Truncated);
    };
    let length = length as usize;
    if length > max_bytes {
        return Err(WireError::TooLong {
            length,
            max: max_bytes,
        });
    }
    let rest = &input[prefix..];
    if rest.len() < length {
        return Err(WireError::Truncated);
    }
    let (bytes, remaining) = rest.split_at(length);
    let value = std::str::from_utf8(bytes).map_err(|_| WireError::InvalidText)?;
    Ok((value, remaining))
}

/// Append `text` in Java modified UTF-8 (U+0000 as `C0 80`, surrogates as three bytes each).
pub(crate) fn encode_modified_utf8(text: &str, out: &mut Vec<u8>) {
    out.reserve(text.len());
    for unit in text.encode_utf16() {
        match unit {
            0x0001..=0x007f => out.push(unit as u8),
            0x0000 | 0x0080..=0x07ff => {
                out.push(0xc0 | (unit >> 6) as u8);
                out.push(0x80 | (unit & 0x3f) as u8);
            }
            _ => {
                out.push(0xe0 | (unit >> 12) as u8);
                out.push(0x80 | ((unit >> 6) & 0x3f) as u8);
                out.push(0x80 | (unit & 0x3f) as u8);
            }
        }
    }
}

/// Decode Java modified UTF-8, rejecting raw NULs, overlong encodings, 4-byte leads,
/// and lone surrogates.
pub(crate) fn decode_modified_utf8(bytes: &[u8]) -> Result<String, WireError> {
    let mut units = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let first = bytes[index];
        let continuation = |offset: usize| -> Result<u16, WireError> {
            match bytes.get(index + offset) {
                Some(&byte) if byte & 0xC0 == 0x80 => Ok(u16::from(byte & 0x3F)),
                _ => Err(WireError::InvalidText),
            }
        };
        let (unit, width) = match first {
            0x01..=0x7F => (u16::from(first), 1),
            0xC0..=0xDF => {
                let unit = (u16::from(first & 0x1F) << 6) | continuation(1)?;
                // Only U+0000 (`C0 80`) may use a two-byte form below U+0080.
                if unit != 0 && unit < 0x80 {
                    return Err(WireError::InvalidText);
                }
                (unit, 2)
            }
            0xE0..=0xEF => {
                let unit =
                    (u16::from(first & 0x0F) << 12) | (continuation(1)? << 6) | continuation(2)?;
                if unit < 0x0800 {
                    return Err(WireError::InvalidText);
                }
                (unit, 3)
            }
            _ => return Err(WireError::InvalidText),
        };
        units.push(unit);
        index += width;
    }
    String::from_utf16(&units).map_err(|_| WireError::InvalidText)
}

/// Encode `text` as a bare NBT string payload (`u16` length + modified UTF-8, without tag byte).
pub(crate) fn put_nbt_string_payload(text: &str, out: &mut Vec<u8>) -> Result<(), WireError> {
    let mut encoded = Vec::with_capacity(text.len());
    encode_modified_utf8(text, &mut encoded);
    let Ok(length) = u16::try_from(encoded.len()) else {
        return Err(WireError::TooLong {
            length: encoded.len(),
            max: MAX_NBT_STRING_BYTES,
        });
    };
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&encoded);
    Ok(())
}

/// Append a plain network NBT text component (`TAG_String`, `u16` length, modified UTF-8).
pub(crate) fn put_nbt_text(text: &str, out: &mut Vec<u8>) -> Result<(), WireError> {
    let start = out.len();
    out.push(NBT_TAG_STRING);
    if let Err(err) = put_nbt_string_payload(text, out) {
        out.truncate(start);
        return Err(err);
    }
    Ok(())
}

/// Decode an NBT string payload (`u16` length plus modified UTF-8, without a tag byte).
pub(crate) fn read_nbt_string(input: &[u8]) -> Result<(String, usize), WireError> {
    let (length_bytes, rest) = input.split_first_chunk::<2>().ok_or(WireError::Truncated)?;
    let length = usize::from(u16::from_be_bytes(*length_bytes));
    let bytes = rest.get(..length).ok_or(WireError::Truncated)?;
    let text = decode_modified_utf8(bytes)?;
    Ok((text, length_bytes.len() + length))
}

/// Decode a big-endian `i16`, returning the value and the remaining slice.
#[inline]
pub(crate) fn read_i16_be(input: &[u8]) -> Result<(i16, &[u8]), WireError> {
    let (bytes, rest) = input.split_first_chunk::<2>().ok_or(WireError::Truncated)?;
    Ok((i16::from_be_bytes(*bytes), rest))
}

/// Decode a big-endian `i32`, returning the value and the remaining slice.
#[inline]
pub(crate) fn read_i32_be(input: &[u8]) -> Result<(i32, &[u8]), WireError> {
    let (bytes, rest) = input.split_first_chunk::<4>().ok_or(WireError::Truncated)?;
    Ok((i32::from_be_bytes(*bytes), rest))
}

/// Decode a big-endian `i64`, returning the value and the remaining slice.
#[inline]
pub(crate) fn read_i64_be(input: &[u8]) -> Result<(i64, &[u8]), WireError> {
    let (bytes, rest) = input.split_first_chunk::<8>().ok_or(WireError::Truncated)?;
    Ok((i64::from_be_bytes(*bytes), rest))
}

/// Decode a big-endian `u64`, returning the value and the remaining slice.
#[inline]
pub(crate) fn read_u64_be(input: &[u8]) -> Result<(u64, &[u8]), WireError> {
    let (bytes, rest) = input.split_first_chunk::<8>().ok_or(WireError::Truncated)?;
    Ok((u64::from_be_bytes(*bytes), rest))
}

/// Decode a big-endian `f32`, returning the value and the remaining slice.
#[inline]
pub(crate) fn read_f32_be(input: &[u8]) -> Result<(f32, &[u8]), WireError> {
    let (bytes, rest) = input.split_first_chunk::<4>().ok_or(WireError::Truncated)?;
    Ok((f32::from_be_bytes(*bytes), rest))
}

/// Decode a big-endian `f64`, returning the value and the remaining slice.
#[inline]
pub(crate) fn read_f64_be(input: &[u8]) -> Result<(f64, &[u8]), WireError> {
    let (bytes, rest) = input.split_first_chunk::<8>().ok_or(WireError::Truncated)?;
    Ok((f64::from_be_bytes(*bytes), rest))
}

/// Pack signed `(x, y, z)` block coordinates into Minecraft's 26/26/12-bit `i64` layout.
#[inline]
pub(crate) fn pack_block_position(x: i32, y: i32, z: i32) -> i64 {
    ((i64::from(x) & BLOCK_XZ_MASK) << BLOCK_X_SHIFT)
        | ((i64::from(z) & BLOCK_XZ_MASK) << BLOCK_Z_SHIFT)
        | (i64::from(y) & BLOCK_Y_MASK)
}

/// Unpack Minecraft's 26/26/12-bit `i64` block position into sign-extended `(x, y, z)`.
#[inline]
pub(crate) fn unpack_block_position(packed: i64) -> (i32, i32, i32) {
    let x = (packed >> BLOCK_X_SHIFT) as i32;
    let y = ((packed << BLOCK_Y_SIGN_EXTEND_SHIFT) >> BLOCK_Y_SIGN_EXTEND_SHIFT) as i32;
    let z = ((packed << BLOCK_Z_HIGH_BITS) >> BLOCK_Z_SIGN_EXTEND_SHIFT) as i32;
    (x, y, z)
}

#[cfg(test)]
mod tests;
