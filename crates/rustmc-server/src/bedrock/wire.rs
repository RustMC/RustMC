//! Bedrock Edition LEB128 VarInt and ZigZag wire codecs shared by framing,
//! session packets, and chunk serialization.

use std::fmt;

/// Maximum byte length of an unsigned or ZigZag-encoded 32-bit VarInt (5 * 7 = 35 bits).
const MAX_VARINT32_BYTES: usize = 5;

/// Bits in the fifth VarInt32 byte that exceed 32 bits or continue past five bytes.
const VARINT32_LAST_BYTE_DISALLOWED_BITS: u8 = 0xf0;

/// Maximum byte length of an unsigned or ZigZag-encoded 64-bit VarInt (10 * 7 = 70 bits).
const MAX_VARINT64_BYTES: usize = 10;

/// Bits in the tenth VarInt64 byte that exceed 64 bits or continue past ten bytes.
const VARINT64_LAST_BYTE_DISALLOWED_BITS: u8 = 0xfe;

/// Errors returned when decoding a Bedrock LEB128 VarInt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BedrockWireError {
    /// Input ended before a terminating byte without continuation bit `0x80`.
    Truncated,
    /// VarInt ran past its maximum byte count or set bits past 32/64 bits.
    Overflow,
    /// VarInt ended with a redundant zero group, such as `[0x80, 0x00]`.
    NonCanonical,
}

impl fmt::Display for BedrockWireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated Bedrock VarInt"),
            Self::Overflow => f.write_str("Bedrock VarInt overflow"),
            Self::NonCanonical => f.write_str("non-canonical Bedrock VarInt"),
        }
    }
}

impl std::error::Error for BedrockWireError {}

/// Encodes an `i32` using 32-bit ZigZag mapping into a `u32`.
#[inline]
pub const fn zigzag_encode32(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}

/// Decodes a 32-bit ZigZag-mapped `u32` into a signed `i32`.
#[inline]
pub const fn zigzag_decode32(value: u32) -> i32 {
    ((value >> 1) as i32) ^ (-((value & 1) as i32))
}

/// Encodes an `i64` using 64-bit ZigZag mapping into a `u64`.
#[inline]
pub const fn zigzag_encode64(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

/// Decodes a 64-bit ZigZag-mapped `u64` into a signed `i64`.
#[inline]
pub const fn zigzag_decode64(value: u64) -> i64 {
    ((value >> 1) as i64) ^ (-((value & 1) as i64))
}

/// Encodes a 64-bit unsigned integer into canonical LEB128 bytes (VarUint64).
pub fn put_varuint64(mut value: u64, out: &mut Vec<u8>) {
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

/// Encodes a 32-bit unsigned integer into canonical LEB128 bytes (VarUint32).
#[inline]
pub fn put_varuint32(value: u32, out: &mut Vec<u8>) {
    put_varuint64(u64::from(value), out);
}

/// Encodes a signed 32-bit integer into ZigZag LEB128 bytes (VarInt32).
#[inline]
pub fn put_varint32(value: i32, out: &mut Vec<u8>) {
    put_varuint32(zigzag_encode32(value), out);
}

/// Encodes a signed 64-bit integer into ZigZag LEB128 bytes (VarInt64).
#[inline]
pub fn put_varint64(value: i64, out: &mut Vec<u8>) {
    put_varuint64(zigzag_encode64(value), out);
}

fn read_varuint(
    input: &[u8],
    max_bytes: usize,
    last_byte_disallowed_bits: u8,
) -> Result<(u64, usize), BedrockWireError> {
    let mut value = 0u64;
    for (i, &byte) in input.iter().take(max_bytes).enumerate() {
        if i == max_bytes - 1 && byte & last_byte_disallowed_bits != 0 {
            return Err(BedrockWireError::Overflow);
        }
        value |= u64::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            if i > 0 && byte == 0 {
                return Err(BedrockWireError::NonCanonical);
            }
            return Ok((value, i + 1));
        }
    }
    Err(BedrockWireError::Truncated)
}

/// Decodes a canonical 32-bit unsigned LEB128 VarInt and returns `(value, bytes_consumed)`.
pub fn read_varuint32(input: &[u8]) -> Result<(u32, usize), BedrockWireError> {
    let (value, len) = read_varuint(
        input,
        MAX_VARINT32_BYTES,
        VARINT32_LAST_BYTE_DISALLOWED_BITS,
    )?;
    Ok((value as u32, len))
}

/// Decodes a canonical 64-bit unsigned LEB128 VarInt and returns `(value, bytes_consumed)`.
pub fn read_varuint64(input: &[u8]) -> Result<(u64, usize), BedrockWireError> {
    read_varuint(
        input,
        MAX_VARINT64_BYTES,
        VARINT64_LAST_BYTE_DISALLOWED_BITS,
    )
}

/// Decodes a canonical ZigZag-encoded 32-bit signed VarInt and returns `(value, bytes_consumed)`.
pub fn read_varint32(input: &[u8]) -> Result<(i32, usize), BedrockWireError> {
    let (zigzag, len) = read_varuint32(input)?;
    Ok((zigzag_decode32(zigzag), len))
}

/// Decodes a canonical ZigZag-encoded 64-bit signed VarInt and returns `(value, bytes_consumed)`.
pub fn read_varint64(input: &[u8]) -> Result<(i64, usize), BedrockWireError> {
    let (zigzag, len) = read_varuint64(input)?;
    Ok((zigzag_decode64(zigzag), len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_varints_round_trip_and_match_boundaries() {
        for value in [
            0u32,
            1,
            127,
            128,
            16_383,
            16_384,
            2_097_151,
            268_435_455,
            u32::MAX,
        ] {
            let mut buf = Vec::new();
            put_varuint32(value, &mut buf);
            assert_eq!(read_varuint32(&buf), Ok((value, buf.len())));
        }

        for value in [0u64, 1, 127, 128, u64::from(u32::MAX), u64::MAX] {
            let mut buf = Vec::new();
            put_varuint64(value, &mut buf);
            assert_eq!(read_varuint64(&buf), Ok((value, buf.len())));
        }

        assert_eq!(
            read_varuint32(&[0xff, 0xff, 0xff, 0xff, 0x0f]),
            Ok((u32::MAX, 5))
        );
        assert_eq!(
            read_varuint64(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]),
            Ok((u64::MAX, 10))
        );
    }

    #[test]
    fn signed_zigzag_varints_round_trip_and_match_wire_values() {
        assert_eq!(zigzag_encode32(0), 0);
        assert_eq!(zigzag_encode32(-1), 1);
        assert_eq!(zigzag_encode32(1), 2);
        assert_eq!(zigzag_encode32(-2), 3);
        assert_eq!(zigzag_encode64(0), 0);
        assert_eq!(zigzag_encode64(-1), 1);
        assert_eq!(zigzag_encode64(1), 2);
        assert_eq!(zigzag_encode64(-2), 3);

        for value in [0i32, 1, -1, 127, -128, 16_384, -16_384, i32::MAX, i32::MIN] {
            let mut buf = Vec::new();
            put_varint32(value, &mut buf);
            assert_eq!(read_varint32(&buf), Ok((value, buf.len())));
        }

        for value in [
            0i64,
            1,
            -1,
            i64::from(i32::MAX),
            i64::from(i32::MIN),
            i64::MAX,
            i64::MIN,
        ] {
            let mut buf = Vec::new();
            put_varint64(value, &mut buf);
            assert_eq!(read_varint64(&buf), Ok((value, buf.len())));
        }
    }

    #[test]
    fn readers_reject_truncated_overflowing_and_non_canonical_varints() {
        assert_eq!(read_varuint32(&[]), Err(BedrockWireError::Truncated));
        assert_eq!(read_varuint32(&[0x80]), Err(BedrockWireError::Truncated));
        assert_eq!(
            read_varuint32(&[0xff, 0xff, 0xff, 0xff]),
            Err(BedrockWireError::Truncated)
        );
        assert_eq!(read_varuint64(&[0x80; 9]), Err(BedrockWireError::Truncated));

        assert_eq!(
            read_varuint32(&[0x80, 0x80, 0x80, 0x80, 0x10]),
            Err(BedrockWireError::Overflow)
        );
        assert_eq!(
            read_varuint32(&[0xff, 0xff, 0xff, 0xff, 0x1f]),
            Err(BedrockWireError::Overflow)
        );
        assert_eq!(
            read_varuint32(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
            Err(BedrockWireError::Overflow)
        );
        assert_eq!(
            read_varuint64(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02]),
            Err(BedrockWireError::Overflow)
        );
        assert_eq!(
            read_varuint64(&[
                0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01
            ]),
            Err(BedrockWireError::Overflow)
        );

        // Non-canonical trailing zero bytes must be rejected across all widths and callers.
        for non_canonical in [
            &[0x80, 0x00][..],
            &[0xff, 0x80, 0x00][..],
            &[0x80, 0x80, 0x80, 0x80, 0x00][..],
        ] {
            assert_eq!(
                read_varuint32(non_canonical),
                Err(BedrockWireError::NonCanonical)
            );
            assert_eq!(
                read_varint32(non_canonical),
                Err(BedrockWireError::NonCanonical)
            );
            assert_eq!(
                read_varuint64(non_canonical),
                Err(BedrockWireError::NonCanonical)
            );
            assert_eq!(
                read_varint64(non_canonical),
                Err(BedrockWireError::NonCanonical)
            );
            assert!(crate::bedrock_framing::read_varuint32(non_canonical).is_err());
            assert!(crate::bedrock::packet::read_varuint32(non_canonical).is_err());
            assert!(crate::bedrock_chunk::read_varuint32(non_canonical).is_err());
        }
        assert_eq!(
            read_varuint64(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00]),
            Err(BedrockWireError::NonCanonical)
        );
    }
}
