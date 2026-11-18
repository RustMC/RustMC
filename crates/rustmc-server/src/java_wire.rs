//! Java Edition wire encoders and decoders shared by RustMC packet code.
//!
//! Cached preview chunk packets are built with these functions, so this file is
//! part of the packet-cache fingerprint in `build.rs`.

/// Maximum number of bytes in a 32-bit protocol VarInt.
const MAX_VARINT_BYTES: usize = 5;
/// Top four bits of the fifth VarInt byte that overflow `u32` or continue past five bytes.
const FIFTH_BYTE_DISALLOWED_BITS: u8 = 0xf0;

/// Why bytes cannot be decoded, however much more input arrives (except `Truncated`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WireError {
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

/// Append a plain network NBT text component (`TAG_String`, `u16` length, modified UTF-8).
pub(crate) fn put_nbt_text(text: &str, out: &mut Vec<u8>) -> Result<(), WireError> {
    let mut encoded = Vec::with_capacity(text.len());
    encode_modified_utf8(text, &mut encoded);
    let Ok(length) = u16::try_from(encoded.len()) else {
        return Err(WireError::TooLong {
            length: encoded.len(),
            max: MAX_NBT_STRING_BYTES,
        });
    };
    out.push(NBT_TAG_STRING);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&encoded);
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
mod tests {
    use super::*;

    #[test]
    fn varints_frames_and_strings_match_the_protocol_encoding() {
        let mut out = Vec::new();
        for value in [0, 1, 127, 128, 300, u32::MAX] {
            put_varint(value, &mut out);
        }
        assert_eq!(
            out,
            [
                0, 1, 0x7f, 0x80, 0x01, 0xac, 0x02, 0xff, 0xff, 0xff, 0xff, 0x0f
            ]
        );
        assert_eq!(frame(0x2a, &[9, 9]), [3, 0x2a, 9, 9]);
        let mut text = Vec::new();
        put_string("hé", &mut text);
        assert_eq!(text, [3, b'h', 0xc3, 0xa9]);
    }

    #[test]
    fn read_varint_decodes_valid_and_rejects_malformed_or_truncated() {
        assert_eq!(read_varint(&[0x00]), Ok(Some((0, 1))));
        assert_eq!(read_varint(&[0x7f]), Ok(Some((127, 1))));
        assert_eq!(read_varint(&[0x80, 0x01]), Ok(Some((128, 2))));
        assert_eq!(read_varint(&[0xac, 0x02]), Ok(Some((300, 2))));
        assert_eq!(
            read_varint(&[0xff, 0xff, 0xff, 0xff, 0x0f]),
            Ok(Some((u32::MAX, 5)))
        );
        assert_eq!(read_varint(&[0x01, 0xaa]), Ok(Some((1, 1))));

        for value in [0, 1, 127, 128, 300, u32::MAX] {
            let mut out = Vec::new();
            put_varint(value, &mut out);
            assert_eq!(read_varint(&out), Ok(Some((value, out.len()))));
        }

        assert_eq!(
            read_varint(&[0xff, 0xff, 0xff, 0xff, 0x1f]),
            Err(WireError::VarIntTooLong)
        );
        assert_eq!(
            read_varint(&[0x80, 0x80, 0x80, 0x80, 0x80]),
            Err(WireError::VarIntTooLong)
        );
        assert_eq!(
            read_varint(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
            Err(WireError::VarIntTooLong)
        );
        assert_eq!(
            read_varint(&[0x80, 0x00]),
            Err(WireError::NonCanonicalVarInt)
        );
        assert_eq!(
            read_varint(&[0xff, 0x80, 0x00]),
            Err(WireError::NonCanonicalVarInt)
        );

        assert_eq!(read_varint(&[]), Ok(None));
        assert_eq!(read_varint(&[0x80]), Ok(None));
        assert_eq!(read_varint(&[0xff, 0xff, 0xff, 0xff]), Ok(None));
    }

    #[test]
    fn read_string_enforces_byte_limit_and_utf8() {
        let sample = [3, b'h', 0xc3, 0xa9, 9];
        assert_eq!(read_string(&sample, 255), Ok(("hé", &[9][..])));
        assert_eq!(read_string(&sample, 3), Ok(("hé", &[9][..])));
        assert_eq!(
            read_string(&sample, 2),
            Err(WireError::TooLong { length: 3, max: 2 })
        );

        assert_eq!(read_string(&[0], 255), Ok(("", &[][..])));

        assert_eq!(read_string(&[], 255), Err(WireError::Truncated));
        assert_eq!(read_string(&[0x80], 255), Err(WireError::Truncated));
        assert_eq!(read_string(&[3, b'h'], 255), Err(WireError::Truncated));
        assert_eq!(read_string(&[1, 0xff], 255), Err(WireError::InvalidText));
    }

    #[test]
    fn encode_and_decode_modified_utf8() {
        let input = "a\0é\u{800}\u{1F600}";
        let expected = [
            0x61, 0xC0, 0x80, 0xC3, 0xA9, 0xE0, 0xA0, 0x80, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80,
        ];
        let mut encoded = Vec::new();
        encode_modified_utf8(input, &mut encoded);
        assert_eq!(encoded, expected);
        assert_eq!(decode_modified_utf8(&encoded), Ok(input.to_owned()));

        for invalid in [
            &[0x00][..],
            &[0xC1, 0x81][..],
            &[0xE0, 0x81, 0x81][..],
            &[0xED, 0xA0, 0xBD][..],
            &[0xED, 0xB8, 0x80][..],
            &[0xED, 0xA0, 0xBD, 0x61][..],
            &[0xF0, 0x9F, 0x98, 0x80][..],
            &[0x80][..],
            &[0xC3][..],
        ] {
            assert_eq!(decode_modified_utf8(invalid), Err(WireError::InvalidText));
        }
    }

    #[test]
    fn nbt_text_helpers_encode_decode_and_bound_length() {
        let mut out = Vec::new();
        put_nbt_text("a\0\u{1F600}", &mut out).unwrap();
        assert_eq!(
            out,
            [
                8, 0, 9, b'a', 0xC0, 0x80, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80
            ]
        );

        let mut max_out = Vec::new();
        put_nbt_text(&"x".repeat(65_535), &mut max_out).unwrap();
        assert_eq!(&max_out[..3], &[8, 0xFF, 0xFF]);
        assert_eq!(max_out.len(), 3 + 65_535);

        let mut unchanged = vec![7];
        assert_eq!(
            put_nbt_text(&"x".repeat(65_536), &mut unchanged),
            Err(WireError::TooLong {
                length: 65_536,
                max: 65_535
            })
        );
        assert_eq!(unchanged, [7]);

        assert_eq!(
            read_nbt_string(&[0, 2, b'h', b'i', 9]),
            Ok(("hi".into(), 4))
        );
        assert_eq!(read_nbt_string(&[0]), Err(WireError::Truncated));
        assert_eq!(read_nbt_string(&[0, 2, b'h']), Err(WireError::Truncated));
    }

    #[test]
    fn block_position_packing_and_unpacking_match_bit_layout() {
        let packed = pack_block_position(18_357_644, 831, -20_882_616);
        assert_eq!(packed, 0x4607_632C_15B4_833F_u64 as i64);
        assert_eq!(
            unpack_block_position(packed),
            (18_357_644, 831, -20_882_616)
        );

        for coords in [
            (-33_554_432, -2_048, 33_554_431),
            (33_554_431, 2_047, -33_554_432),
        ] {
            let packed = pack_block_position(coords.0, coords.1, coords.2);
            assert_eq!(unpack_block_position(packed), coords);
        }
    }

    #[test]
    fn fixed_width_big_endian_readers_decode_values_and_reject_truncation() {
        for value in [i16::MIN, -1, 0, 8, i16::MAX] {
            let mut buf = value.to_be_bytes().to_vec();
            buf.push(0xaa);
            assert_eq!(read_i16_be(&buf), Ok((value, &[0xaa][..])));
        }
        for len in 0..2 {
            assert_eq!(read_i16_be(&[0; 2][..len]), Err(WireError::Truncated));
        }

        for value in [i64::MIN, -1, 0, 0x0102_0304_0506_0708, i64::MAX] {
            let mut buf = value.to_be_bytes().to_vec();
            buf.extend_from_slice(&[0xbb, 0xcc]);
            assert_eq!(read_i64_be(&buf), Ok((value, &[0xbb, 0xcc][..])));
        }
        for len in 0..8 {
            assert_eq!(read_i64_be(&[0; 8][..len]), Err(WireError::Truncated));
        }

        for value in [0u64, 1, 0x0102_0304_0506_0708, u64::MAX] {
            let mut buf = value.to_be_bytes().to_vec();
            buf.push(0xdd);
            assert_eq!(read_u64_be(&buf), Ok((value, &[0xdd][..])));
        }
        for len in 0..8 {
            assert_eq!(read_u64_be(&[0; 8][..len]), Err(WireError::Truncated));
        }

        for value in [-90.0f32, -0.5, 0.0, 0.25, 180.0, f32::INFINITY] {
            let mut buf = value.to_be_bytes().to_vec();
            buf.push(0xee);
            let (decoded, rest) = read_f32_be(&buf).unwrap();
            assert_eq!(decoded.to_bits(), value.to_bits());
            assert_eq!(rest, &[0xee][..]);
        }
        for len in 0..4 {
            assert_eq!(read_f32_be(&[0; 4][..len]), Err(WireError::Truncated));
        }

        for value in [
            -30_000_000.0f64,
            -1.5,
            0.0,
            64.0,
            30_000_000.0,
            f64::NEG_INFINITY,
        ] {
            let mut buf = value.to_be_bytes().to_vec();
            buf.push(0xff);
            let (decoded, rest) = read_f64_be(&buf).unwrap();
            assert_eq!(decoded.to_bits(), value.to_bits());
            assert_eq!(rest, &[0xff][..]);
        }
        for len in 0..8 {
            assert_eq!(read_f64_be(&[0; 8][..len]), Err(WireError::Truncated));
        }
    }
}
