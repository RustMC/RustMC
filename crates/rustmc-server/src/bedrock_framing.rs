//! Minecraft Bedrock game packet batch framing (packet ID 0xFE).
//!
//! All Bedrock game packets are wrapped into batches. A batch packet begins with
//! [`BATCH_PACKET_ID`] (0xFE), followed by a zlib-compressed stream containing
//! one or more length-prefixed game packets. Packet lengths are encoded as
//! unsigned 32-bit variable-length integers (VarUint32 / VarInt).

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

/// Batch packet identifier prefix for Bedrock game packets over RakNet.
pub const BATCH_PACKET_ID: u8 = 0xfe;

/// Maximum allowable decompressed payload size for a single batch packet (4 MiB).
pub const MAX_DECOMPRESSED_BATCH_SIZE: usize = 4 * 1024 * 1024;

/// Maximum allowable number of game packets in a single batch.
pub const MAX_PACKETS_PER_BATCH: usize = 256;

/// Errors arising during Bedrock batch packet encoding or decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BedrockFramingError {
    /// Batch packet header does not match [`BATCH_PACKET_ID`].
    InvalidHeader(u8),
    /// Zlib decompression failed or input was malformed compressed data.
    DecompressFailed(String),
    /// Decompressed batch payload exceeded [`MAX_DECOMPRESSED_BATCH_SIZE`].
    DecompressionBomb { size: usize, limit: usize },
    /// VarInt / VarUint byte sequence is malformed, truncated, or exceeds 32 bits.
    MalformedVarInt,
    /// Packet payload was shorter than the declared length prefix.
    TruncatedPacket { expected: usize, available: usize },
    /// Number of packets in batch exceeds [`MAX_PACKETS_PER_BATCH`].
    TooManyPackets(usize),
}

impl std::fmt::Display for BedrockFramingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHeader(header) => {
                write!(f, "invalid bedrock batch packet header: {header:#04x}")
            }
            Self::DecompressFailed(reason) => {
                write!(f, "bedrock batch decompression failed: {reason}")
            }
            Self::DecompressionBomb { size, limit } => write!(
                f,
                "bedrock batch decompressed size {size} exceeds limit {limit}"
            ),
            Self::MalformedVarInt => write!(f, "malformed VarInt in bedrock batch"),
            Self::TruncatedPacket {
                expected,
                available,
            } => write!(
                f,
                "truncated packet: expected {expected} bytes, but only {available} available"
            ),
            Self::TooManyPackets(count) => {
                write!(f, "too many packets in bedrock batch: {count}")
            }
        }
    }
}

impl std::error::Error for BedrockFramingError {}

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
/// Returns the decoded integer and the number of bytes consumed from `slice`.
pub fn read_varuint32(slice: &[u8]) -> Result<(u32, usize), BedrockFramingError> {
    let mut value = 0u32;
    let mut shift = 0;
    for (i, &byte) in slice.iter().take(5).enumerate() {
        if i == 4 {
            if byte & 0xf0 != 0 {
                return Err(BedrockFramingError::MalformedVarInt);
            }
            value |= u32::from(byte & 0x0f) << shift;
            return Ok((value, 5));
        }
        value |= u32::from(byte & 0x7f) << shift;
        if (byte & 0x80) == 0 {
            return Ok((value, i + 1));
        }
        shift += 7;
    }
    Err(BedrockFramingError::MalformedVarInt)
}

/// Alias for [`write_varuint32`].
pub fn write_varint(value: u32, out: &mut Vec<u8>) {
    write_varuint32(value, out);
}

/// Alias for [`read_varuint32`].
pub fn read_varint(slice: &[u8]) -> Result<(u32, usize), BedrockFramingError> {
    read_varuint32(slice)
}

/// Encodes a sequence of raw game packets into a Bedrock batch packet.
///
/// Prepends [`BATCH_PACKET_ID`] (0xFE) and compresses the length-prefixed packet
/// sequence using zlib deflate.
pub fn encode_batch(packets: &[&[u8]]) -> Result<Vec<u8>, BedrockFramingError> {
    if packets.len() > MAX_PACKETS_PER_BATCH {
        return Err(BedrockFramingError::TooManyPackets(packets.len()));
    }

    let mut uncompressed = Vec::new();
    for packet in packets {
        let len = u32::try_from(packet.len()).map_err(|_| BedrockFramingError::MalformedVarInt)?;
        write_varuint32(len, &mut uncompressed);
        uncompressed.extend_from_slice(packet);
    }

    let mut out = vec![BATCH_PACKET_ID];
    let mut encoder = ZlibEncoder::new(&mut out, Compression::default());
    encoder
        .write_all(&uncompressed)
        .map_err(|e| BedrockFramingError::DecompressFailed(e.to_string()))?;
    encoder
        .finish()
        .map_err(|e| BedrockFramingError::DecompressFailed(e.to_string()))?;

    Ok(out)
}

/// Decodes a Bedrock batch packet into individual raw game packet buffers.
///
/// Verifies the [`BATCH_PACKET_ID`] (0xFE) prefix, decompresses at most
/// [`MAX_DECOMPRESSED_BATCH_SIZE`] + 1 bytes (rejecting decompression bombs),
/// and parses each length-prefixed packet.
pub fn decode_batch(batch: &[u8]) -> Result<Vec<Vec<u8>>, BedrockFramingError> {
    let Some(&header) = batch.first() else {
        return Err(BedrockFramingError::TruncatedPacket {
            expected: 1,
            available: 0,
        });
    };
    if header != BATCH_PACKET_ID {
        return Err(BedrockFramingError::InvalidHeader(header));
    }

    let mut decoder = ZlibDecoder::new(&batch[1..]).take((MAX_DECOMPRESSED_BATCH_SIZE + 1) as u64);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| BedrockFramingError::DecompressFailed(e.to_string()))?;

    if decompressed.len() > MAX_DECOMPRESSED_BATCH_SIZE {
        return Err(BedrockFramingError::DecompressionBomb {
            size: decompressed.len(),
            limit: MAX_DECOMPRESSED_BATCH_SIZE,
        });
    }

    let mut packets = Vec::new();
    let mut offset = 0;
    while offset < decompressed.len() {
        let (packet_len, varint_len) = read_varuint32(&decompressed[offset..])?;
        offset += varint_len;
        let packet_len = packet_len as usize;
        let available = decompressed.len() - offset;
        if available < packet_len {
            return Err(BedrockFramingError::TruncatedPacket {
                expected: packet_len,
                available,
            });
        }
        packets.push(decompressed[offset..offset + packet_len].to_vec());
        offset += packet_len;
        if packets.len() > MAX_PACKETS_PER_BATCH {
            return Err(BedrockFramingError::TooManyPackets(packets.len()));
        }
    }

    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip_multiple_packets() {
        let p1 = b"client_to_server_handshake";
        let p2 = b"";
        let p3 = b"login";
        let p4 = vec![0xa5u8; 4096];
        let p5 = vec![0x3cu8; 65536];

        let packets: Vec<&[u8]> = vec![p1, p2, p3, &p4, &p5];
        let encoded = encode_batch(&packets).expect("encode should succeed");
        assert_eq!(encoded[0], BATCH_PACKET_ID);

        let decoded = decode_batch(&encoded).expect("decode should succeed");
        assert_eq!(decoded.len(), packets.len());
        for (i, expected) in packets.iter().enumerate() {
            assert_eq!(&decoded[i], expected);
        }
    }

    #[test]
    fn test_empty_batch() {
        let encoded = encode_batch(&[]).expect("empty batch encoding should succeed");
        assert_eq!(encoded[0], BATCH_PACKET_ID);

        let decoded = decode_batch(&encoded).expect("empty batch decoding should succeed");
        assert!(decoded.is_empty());

        // Also test decoding a batch that decompresses to 0 bytes directly.
        let mut raw = vec![BATCH_PACKET_ID];
        let encoder = ZlibEncoder::new(&mut raw, Compression::default());
        encoder.finish().unwrap();
        let decoded2 = decode_batch(&raw).expect("empty stream decoding should succeed");
        assert!(decoded2.is_empty());
    }

    #[test]
    fn test_decompression_bomb_rejection() {
        // Stream that decompresses to MAX_DECOMPRESSED_BATCH_SIZE + 1 bytes.
        let uncompressed = vec![0u8; MAX_DECOMPRESSED_BATCH_SIZE + 1];
        let mut compressed = vec![BATCH_PACKET_ID];
        let mut encoder = ZlibEncoder::new(&mut compressed, Compression::fast());
        encoder.write_all(&uncompressed).unwrap();
        encoder.finish().unwrap();

        let result = decode_batch(&compressed);
        match result {
            Err(BedrockFramingError::DecompressionBomb { size, limit }) => {
                assert_eq!(limit, MAX_DECOMPRESSED_BATCH_SIZE);
                assert_eq!(size, MAX_DECOMPRESSED_BATCH_SIZE + 1);
            }
            other => panic!("expected DecompressionBomb, got {other:?}"),
        }

        // Significantly oversized bomb should also be caught without reading > MAX_DECOMPRESSED_BATCH_SIZE + 1.
        let big_bomb = vec![0u8; MAX_DECOMPRESSED_BATCH_SIZE + 1024];
        let mut compressed_big = vec![BATCH_PACKET_ID];
        let mut encoder = ZlibEncoder::new(&mut compressed_big, Compression::fast());
        encoder.write_all(&big_bomb).unwrap();
        encoder.finish().unwrap();

        let result_big = decode_batch(&compressed_big);
        match result_big {
            Err(BedrockFramingError::DecompressionBomb { size, limit }) => {
                assert_eq!(limit, MAX_DECOMPRESSED_BATCH_SIZE);
                assert_eq!(size, MAX_DECOMPRESSED_BATCH_SIZE + 1);
            }
            other => panic!("expected DecompressionBomb, got {other:?}"),
        }
    }

    #[test]
    fn test_malformed_header_rejection() {
        // Non-0xfe headers
        for invalid_header in [0x00, 0x01, 0x1c, 0xfd, 0xff] {
            let batch = vec![
                invalid_header,
                0x78,
                0x9c,
                0x03,
                0x00,
                0x00,
                0x00,
                0x00,
                0x01,
            ];
            assert_eq!(
                decode_batch(&batch).unwrap_err(),
                BedrockFramingError::InvalidHeader(invalid_header)
            );
        }

        // Empty batch slice
        assert_eq!(
            decode_batch(&[]).unwrap_err(),
            BedrockFramingError::TruncatedPacket {
                expected: 1,
                available: 0,
            }
        );
    }

    #[test]
    fn test_truncated_length_or_payload() {
        // 1. Truncated VarInt length prefix: 0x80 has continuation bit set but stream ends.
        let mut truncated_varint = vec![BATCH_PACKET_ID];
        let mut encoder = ZlibEncoder::new(&mut truncated_varint, Compression::default());
        encoder.write_all(&[0x80]).unwrap();
        encoder.finish().unwrap();
        assert_eq!(
            decode_batch(&truncated_varint).unwrap_err(),
            BedrockFramingError::MalformedVarInt
        );

        // 2. Truncated payload: packet declares length 20, but only 5 bytes are in stream.
        let mut raw = Vec::new();
        write_varuint32(20, &mut raw);
        raw.extend_from_slice(&[1, 2, 3, 4, 5]);

        let mut truncated_payload = vec![BATCH_PACKET_ID];
        let mut encoder = ZlibEncoder::new(&mut truncated_payload, Compression::default());
        encoder.write_all(&raw).unwrap();
        encoder.finish().unwrap();
        assert_eq!(
            decode_batch(&truncated_payload).unwrap_err(),
            BedrockFramingError::TruncatedPacket {
                expected: 20,
                available: 5,
            }
        );
    }

    #[test]
    fn test_too_many_packets_rejection() {
        // Exceeding on encode
        let packets = vec![&b""[..]; MAX_PACKETS_PER_BATCH + 1];
        assert_eq!(
            encode_batch(&packets).unwrap_err(),
            BedrockFramingError::TooManyPackets(MAX_PACKETS_PER_BATCH + 1)
        );

        // Exactly MAX_PACKETS_PER_BATCH should succeed
        let max_packets = vec![&b"x"[..]; MAX_PACKETS_PER_BATCH];
        let encoded = encode_batch(&max_packets).expect("256 packets should succeed");
        let decoded = decode_batch(&encoded).expect("256 packets should decode");
        assert_eq!(decoded.len(), MAX_PACKETS_PER_BATCH);

        // Exceeding on decode (crafted raw batch with 257 packets)
        let mut raw = Vec::new();
        for _ in 0..=MAX_PACKETS_PER_BATCH {
            write_varuint32(0, &mut raw);
        }
        let mut batch = vec![BATCH_PACKET_ID];
        let mut encoder = ZlibEncoder::new(&mut batch, Compression::default());
        encoder.write_all(&raw).unwrap();
        encoder.finish().unwrap();

        assert_eq!(
            decode_batch(&batch).unwrap_err(),
            BedrockFramingError::TooManyPackets(MAX_PACKETS_PER_BATCH + 1)
        );
    }

    #[test]
    fn test_varuint32_roundtrip_and_edge_cases() {
        let test_values = [
            0u32,
            1,
            127,
            128,
            255,
            256,
            16383,
            16384,
            2097151,
            2097152,
            268435455,
            268435456,
            u32::MAX - 1,
            u32::MAX,
        ];

        for &val in &test_values {
            let mut buf = Vec::new();
            write_varuint32(val, &mut buf);
            let (decoded, consumed) = read_varuint32(&buf).expect("should decode valid varuint");
            assert_eq!(decoded, val);
            assert_eq!(consumed, buf.len());

            // Test aliases
            let mut buf_alias = Vec::new();
            write_varint(val, &mut buf_alias);
            assert_eq!(buf, buf_alias);
            let (decoded_alias, consumed_alias) =
                read_varint(&buf_alias).expect("should decode valid varint");
            assert_eq!(decoded_alias, val);
            assert_eq!(consumed_alias, consumed);
        }
    }

    #[test]
    fn test_varuint32_malformed() {
        // Empty slice
        assert_eq!(
            read_varuint32(&[]).unwrap_err(),
            BedrockFramingError::MalformedVarInt
        );

        // Truncated multi-byte sequence
        assert_eq!(
            read_varuint32(&[0x80]).unwrap_err(),
            BedrockFramingError::MalformedVarInt
        );
        assert_eq!(
            read_varuint32(&[0x80, 0x80]).unwrap_err(),
            BedrockFramingError::MalformedVarInt
        );

        // 5th byte with upper bits set (overflow)
        assert_eq!(
            read_varuint32(&[0x80, 0x80, 0x80, 0x80, 0xf0]).unwrap_err(),
            BedrockFramingError::MalformedVarInt
        );

        // 6-byte varint sequence
        assert_eq!(
            read_varuint32(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]).unwrap_err(),
            BedrockFramingError::MalformedVarInt
        );
    }

    #[test]
    fn test_corrupt_zlib_decompression_failure() {
        let corrupt = vec![BATCH_PACKET_ID, 0x12, 0x34, 0x56, 0x78];
        assert!(matches!(
            decode_batch(&corrupt).unwrap_err(),
            BedrockFramingError::DecompressFailed(_)
        ));
    }
}
