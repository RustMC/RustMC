//! Minecraft Bedrock game packet batch framing over RakNet (marker 0xFE).
//!
//! A batch is [`BATCH_PACKET_ID`] followed by one or more game packets, each
//! prefixed by its byte length as a VarUint32. Which bytes sit between the
//! marker and the packets depends on the session phase, modelled by
//! [`BatchFraming`]:
//!
//! - Before `NetworkSettings` takes effect the packets follow the marker
//!   directly, uncompressed.
//! - Afterwards a one-byte algorithm selector precedes the data: `0x00` raw
//!   DEFLATE (RFC 1951, with no RFC 1950 header or Adler-32 trailer), `0x01`
//!   Snappy, `0xFF` uncompressed.
//!
//! This module is a DEFLATE-only utility. It emits and accepts selectors `0x00`
//! and `0xFF` and rejects Snappy with
//! [`BedrockFramingError::UnsupportedCompression`], so a session must negotiate
//! `ZLib` in `NetworkSettings`. Application-layer encryption is out of scope. No
//! Bedrock client has exercised this code yet.
//!
//! Sources. Mojang's 1.26.51 schema (protocol 2193, release `v1.26.51` of
//! <https://github.com/Mojang/bedrock-protocol-docs>) defines the negotiated
//! algorithms and the threshold rule in `NetworkSettingsPacketPayload.json` and
//! `PacketCompressionAlgorithm.json`. That schema does not describe the outer
//! batch, so the marker, selector values, and raw DEFLATE come from community
//! documentation: <https://minecraft.wiki/w/Bedrock_Edition_protocol>, revision
//! 3866014.

use std::io::Write;

use flate2::write::DeflateEncoder;
use flate2::{Compression, Decompress, FlushDecompress, Status};

/// Batch packet identifier prefix for Bedrock game packets over RakNet.
pub const BATCH_PACKET_ID: u8 = 0xfe;

/// Selector for a raw DEFLATE payload, negotiated as `ZLib` in `NetworkSettings`.
pub const COMPRESSION_DEFLATE: u8 = 0x00;

/// Selector for a Snappy payload, which this module does not implement.
pub const COMPRESSION_SNAPPY: u8 = 0x01;

/// Selector for an uncompressed payload.
pub const COMPRESSION_NONE: u8 = 0xff;

/// Maximum allowable decompressed payload size for a single batch packet (4 MiB).
///
/// This is a RustMC memory budget, not a protocol limit.
pub const MAX_DECOMPRESSED_BATCH_SIZE: usize = 4 * 1024 * 1024;

/// Maximum allowable number of game packets in a single batch.
///
/// This is a RustMC work budget, not a protocol limit.
pub const MAX_PACKETS_PER_BATCH: usize = 256;

/// Initial inflate buffer size; the buffer doubles up to the decompression bound.
const INFLATE_INITIAL_CAPACITY: usize = 64 * 1024;

/// Session phase that decides the bytes between the marker and the packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchFraming {
    /// `NetworkSettings` has not taken effect: no selector, no compression.
    PreNegotiation,
    /// `NetworkSettings` has taken effect: a selector precedes the data.
    Negotiated {
        /// The negotiated `Compression Threshold`. Mojang's schema defines `0` as
        /// compression disabled and `1` as compressing everything of one byte or
        /// more. Only the encoder consults it, because the selector tells the
        /// decoder what was sent.
        threshold: u16,
    },
}

/// Errors arising during Bedrock batch packet encoding or decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BedrockFramingError {
    /// Batch packet header does not match [`BATCH_PACKET_ID`].
    InvalidHeader(u8),
    /// The selector names an algorithm this module does not implement.
    UnsupportedCompression(u8),
    /// DEFLATE compression of an outgoing batch failed.
    CompressFailed(String),
    /// DEFLATE data was malformed, truncated, or followed by trailing bytes.
    DecompressFailed(String),
    /// Decompressed batch payload exceeded [`MAX_DECOMPRESSED_BATCH_SIZE`].
    DecompressionBomb { size: usize, limit: usize },
    /// Total uncompressed batch size to encode exceeded [`MAX_DECOMPRESSED_BATCH_SIZE`].
    BatchTooLarge { size: usize, limit: usize },
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
            Self::UnsupportedCompression(selector) => {
                write!(
                    f,
                    "unsupported bedrock compression selector: {selector:#04x}"
                )
            }
            Self::CompressFailed(reason) => {
                write!(f, "bedrock batch compression failed: {reason}")
            }
            Self::DecompressFailed(reason) => {
                write!(f, "bedrock batch decompression failed: {reason}")
            }
            Self::DecompressionBomb { size, limit } => write!(
                f,
                "bedrock batch decompressed size {size} exceeds limit {limit}"
            ),
            Self::BatchTooLarge { size, limit } => write!(
                f,
                "bedrock batch uncompressed size {size} exceeds limit {limit}"
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
/// Prepends [`BATCH_PACKET_ID`] (0xFE). Under [`BatchFraming::Negotiated`], a
/// payload at or above a nonzero threshold is raw DEFLATE behind
/// [`COMPRESSION_DEFLATE`]; anything else is stored behind [`COMPRESSION_NONE`].
pub fn encode_batch(
    packets: &[&[u8]],
    framing: BatchFraming,
) -> Result<Vec<u8>, BedrockFramingError> {
    if packets.len() > MAX_PACKETS_PER_BATCH {
        return Err(BedrockFramingError::TooManyPackets(packets.len()));
    }

    let mut uncompressed = Vec::new();
    for packet in packets {
        let len = u32::try_from(packet.len()).map_err(|_| BedrockFramingError::MalformedVarInt)?;
        let mut varint_bytes = Vec::with_capacity(5);
        write_varuint32(len, &mut varint_bytes);
        let needed = uncompressed
            .len()
            .checked_add(varint_bytes.len())
            .and_then(|l| l.checked_add(packet.len()))
            .ok_or(BedrockFramingError::BatchTooLarge {
                size: usize::MAX,
                limit: MAX_DECOMPRESSED_BATCH_SIZE,
            })?;
        if needed > MAX_DECOMPRESSED_BATCH_SIZE {
            return Err(BedrockFramingError::BatchTooLarge {
                size: needed,
                limit: MAX_DECOMPRESSED_BATCH_SIZE,
            });
        }
        uncompressed.extend_from_slice(&varint_bytes);
        uncompressed.extend_from_slice(packet);
    }

    let mut out = Vec::with_capacity(uncompressed.len() + 2);
    out.push(BATCH_PACKET_ID);
    let BatchFraming::Negotiated { threshold } = framing else {
        out.extend_from_slice(&uncompressed);
        return Ok(out);
    };
    if threshold == 0 || uncompressed.len() < usize::from(threshold) {
        out.push(COMPRESSION_NONE);
        out.extend_from_slice(&uncompressed);
        return Ok(out);
    }

    out.push(COMPRESSION_DEFLATE);
    let mut encoder = DeflateEncoder::new(out, Compression::default());
    encoder
        .write_all(&uncompressed)
        .map_err(|e| BedrockFramingError::CompressFailed(e.to_string()))?;
    encoder
        .finish()
        .map_err(|e| BedrockFramingError::CompressFailed(e.to_string()))
}

/// Inflates a complete raw DEFLATE stream of at most
/// [`MAX_DECOMPRESSED_BATCH_SIZE`] bytes.
///
/// Output never grows past the bound plus one byte. Success requires the
/// inflater to report the stream end at exactly the end of `input`, so truncated
/// streams and trailing bytes are both rejected.
fn inflate_bounded(input: &[u8]) -> Result<Vec<u8>, BedrockFramingError> {
    const BOUND: usize = MAX_DECOMPRESSED_BATCH_SIZE + 1;

    let mut inflater = Decompress::new(false);
    let mut out = Vec::with_capacity(INFLATE_INITIAL_CAPACITY);
    loop {
        let consumed = usize::try_from(inflater.total_in()).unwrap_or(usize::MAX);
        let remaining = input.get(consumed..).unwrap_or_default();
        let produced_before = out.len();
        let status = inflater
            .decompress_vec(remaining, &mut out, FlushDecompress::None)
            .map_err(|e| BedrockFramingError::DecompressFailed(e.to_string()))?;

        if out.len() > MAX_DECOMPRESSED_BATCH_SIZE {
            return Err(BedrockFramingError::DecompressionBomb {
                size: out.len(),
                limit: MAX_DECOMPRESSED_BATCH_SIZE,
            });
        }
        if status == Status::StreamEnd {
            if usize::try_from(inflater.total_in()).ok() != Some(input.len()) {
                return Err(BedrockFramingError::DecompressFailed(
                    "trailing bytes after deflate stream".to_string(),
                ));
            }
            return Ok(out);
        }
        if out.len() == out.capacity() {
            let target = out.capacity().saturating_mul(2).min(BOUND);
            out.reserve_exact(target - out.len());
            continue;
        }

        let consumed_now = usize::try_from(inflater.total_in()).unwrap_or(usize::MAX);
        if consumed_now == consumed && out.len() == produced_before {
            return Err(BedrockFramingError::DecompressFailed(
                "deflate stream ended before its final block".to_string(),
            ));
        }
    }
}

/// Decodes a Bedrock batch packet into individual raw game packet buffers.
///
/// Verifies the [`BATCH_PACKET_ID`] (0xFE) prefix, then under
/// [`BatchFraming::Negotiated`] reads the selector and inflates at most
/// [`MAX_DECOMPRESSED_BATCH_SIZE`] + 1 bytes (rejecting decompression bombs). An
/// uncompressed payload obeys the same size limit. Each length-prefixed packet
/// is then parsed.
pub fn decode_batch(
    batch: &[u8],
    framing: BatchFraming,
) -> Result<Vec<Vec<u8>>, BedrockFramingError> {
    let Some((&header, body)) = batch.split_first() else {
        return Err(BedrockFramingError::TruncatedPacket {
            expected: 1,
            available: 0,
        });
    };
    if header != BATCH_PACKET_ID {
        return Err(BedrockFramingError::InvalidHeader(header));
    }

    let inflated;
    let payload = match framing {
        BatchFraming::PreNegotiation => body,
        BatchFraming::Negotiated { .. } => {
            let Some((&selector, data)) = body.split_first() else {
                return Err(BedrockFramingError::TruncatedPacket {
                    expected: 1,
                    available: 0,
                });
            };
            match selector {
                COMPRESSION_DEFLATE => {
                    inflated = inflate_bounded(data)?;
                    inflated.as_slice()
                }
                COMPRESSION_NONE => data,
                other => return Err(BedrockFramingError::UnsupportedCompression(other)),
            }
        }
    };
    if payload.len() > MAX_DECOMPRESSED_BATCH_SIZE {
        return Err(BedrockFramingError::BatchTooLarge {
            size: payload.len(),
            limit: MAX_DECOMPRESSED_BATCH_SIZE,
        });
    }

    let mut packets = Vec::new();
    let mut offset = 0;
    while offset < payload.len() {
        let (packet_len, varint_len) = read_varuint32(&payload[offset..])?;
        offset += varint_len;
        let packet_len = packet_len as usize;
        let available = payload.len() - offset;
        if available < packet_len {
            return Err(BedrockFramingError::TruncatedPacket {
                expected: packet_len,
                available,
            });
        }
        packets.push(payload[offset..offset + packet_len].to_vec());
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

    const NEGOTIATED: BatchFraming = BatchFraming::Negotiated { threshold: 1 };

    /// A negotiated batch whose payload is `raw`, compressed independently of
    /// `encode_batch`.
    fn deflate_batch(raw: &[u8]) -> Vec<u8> {
        let mut encoder = DeflateEncoder::new(
            vec![BATCH_PACKET_ID, COMPRESSION_DEFLATE],
            Compression::fast(),
        );
        encoder.write_all(raw).unwrap();
        encoder.finish().unwrap()
    }

    /// RequestNetworkSettings for protocol 2193, as written out by hand: length
    /// prefix 6, header varuint 193, then the big-endian network version.
    const REQUEST_NETWORK_SETTINGS: [u8; 7] = [0x06, 0xc1, 0x01, 0x00, 0x00, 0x08, 0x91];

    #[test]
    fn test_decodes_handwritten_stored_deflate_block() {
        // RFC 1951 stored block: BFINAL=1 BTYPE=00, LEN=7, NLEN=!7.
        let mut wire = vec![0xfe, 0x00, 0x01, 0x07, 0x00, 0xf8, 0xff];
        wire.extend_from_slice(&REQUEST_NETWORK_SETTINGS);
        let decoded = decode_batch(&wire, NEGOTIATED).expect("stored block should decode");
        assert_eq!(decoded, vec![REQUEST_NETWORK_SETTINGS[1..].to_vec()]);
    }

    #[test]
    fn test_decodes_handwritten_fixed_huffman_empty_stream() {
        // RFC 1951 final fixed-Huffman block holding only end-of-block.
        let wire = [0xfe, 0x00, 0x03, 0x00];
        assert!(decode_batch(&wire, NEGOTIATED).unwrap().is_empty());
    }

    #[test]
    fn test_rejects_rfc1950_wrapped_stream() {
        // A zlib-wrapped empty stream: header 78 9c, empty block, Adler-32 of "".
        let wire = [0xfe, 0x00, 0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
        assert!(matches!(
            decode_batch(&wire, NEGOTIATED).unwrap_err(),
            BedrockFramingError::DecompressFailed(_)
        ));
    }

    #[test]
    fn test_pre_negotiation_wire_has_no_selector() {
        let packet = &REQUEST_NETWORK_SETTINGS[1..];
        let expected = [0xfe, 0x06, 0xc1, 0x01, 0x00, 0x00, 0x08, 0x91];
        let encoded = encode_batch(&[packet], BatchFraming::PreNegotiation).unwrap();
        assert_eq!(encoded, expected);
        let decoded = decode_batch(&expected, BatchFraming::PreNegotiation).unwrap();
        assert_eq!(decoded, vec![packet.to_vec()]);
    }

    #[test]
    fn test_below_threshold_is_stored_behind_none_selector() {
        let packet = &REQUEST_NETWORK_SETTINGS[1..];
        let expected = [0xfe, 0xff, 0x06, 0xc1, 0x01, 0x00, 0x00, 0x08, 0x91];
        // The payload is 7 bytes, so 8 is the smallest threshold it falls under.
        let framing = BatchFraming::Negotiated { threshold: 8 };
        assert_eq!(encode_batch(&[packet], framing).unwrap(), expected);
        assert_eq!(
            decode_batch(&expected, framing).unwrap(),
            vec![packet.to_vec()]
        );
    }

    #[test]
    fn test_threshold_zero_disables_compression() {
        let packet = vec![0x11u8; 4096];
        let framing = BatchFraming::Negotiated { threshold: 0 };
        let encoded = encode_batch(&[&packet], framing).unwrap();
        assert_eq!(encoded[..2], [BATCH_PACKET_ID, COMPRESSION_NONE]);
    }

    #[test]
    fn test_at_threshold_is_raw_deflate() {
        use std::io::Read;

        let packet = &REQUEST_NETWORK_SETTINGS[1..];
        let framing = BatchFraming::Negotiated { threshold: 7 };
        let encoded = encode_batch(&[packet], framing).unwrap();
        assert_eq!(encoded[..2], [BATCH_PACKET_ID, COMPRESSION_DEFLATE]);

        // Inflate through a separate reader rather than this module's decoder.
        let mut inflated = Vec::new();
        flate2::read::DeflateDecoder::new(&encoded[2..])
            .read_to_end(&mut inflated)
            .unwrap();
        assert_eq!(inflated, REQUEST_NETWORK_SETTINGS);
    }

    #[test]
    fn test_unknown_and_snappy_selectors_are_rejected() {
        for selector in [COMPRESSION_SNAPPY, 0x02, 0x78, 0xfe] {
            assert_eq!(
                decode_batch(&[0xfe, selector, 0x00], NEGOTIATED).unwrap_err(),
                BedrockFramingError::UnsupportedCompression(selector)
            );
        }
    }

    #[test]
    fn test_missing_selector_is_truncated() {
        assert_eq!(
            decode_batch(&[0xfe], NEGOTIATED).unwrap_err(),
            BedrockFramingError::TruncatedPacket {
                expected: 1,
                available: 0,
            }
        );
    }

    #[test]
    fn test_every_truncation_of_a_deflate_stream_is_rejected() {
        let wire = deflate_batch(&REQUEST_NETWORK_SETTINGS);
        assert!(decode_batch(&wire, NEGOTIATED).is_ok());
        for end in 2..wire.len() {
            assert!(
                matches!(
                    decode_batch(&wire[..end], NEGOTIATED),
                    Err(BedrockFramingError::DecompressFailed(_))
                ),
                "prefix of {end} bytes should fail"
            );
        }
    }

    #[test]
    fn test_truncated_multi_buffer_stream_is_rejected() {
        // Incompressible enough to span several inflate buffer growths.
        let mut state = 0x2545_f491u32;
        let payload: Vec<u8> = (0..300_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let encoded = encode_batch(&[&payload], NEGOTIATED).unwrap();
        assert_eq!(decode_batch(&encoded, NEGOTIATED).unwrap(), vec![payload]);
        assert!(matches!(
            decode_batch(&encoded[..encoded.len() - 1], NEGOTIATED),
            Err(BedrockFramingError::DecompressFailed(_))
        ));
    }

    #[test]
    fn test_trailing_bytes_after_stream_are_rejected() {
        let mut wire = deflate_batch(&REQUEST_NETWORK_SETTINGS);
        wire.push(0x00);
        assert!(matches!(
            decode_batch(&wire, NEGOTIATED).unwrap_err(),
            BedrockFramingError::DecompressFailed(_)
        ));
    }

    #[test]
    fn test_maximum_size_payload_round_trips() {
        // One packet whose prefix and body exactly fill the limit.
        let body = MAX_DECOMPRESSED_BATCH_SIZE - 4;
        let packet = vec![0x5au8; body];
        let encoded = encode_batch(&[&packet], NEGOTIATED).expect("limit should encode");
        let decoded = decode_batch(&encoded, NEGOTIATED).expect("limit should decode");
        assert_eq!(decoded, vec![packet]);
    }

    #[test]
    fn test_oversized_uncompressed_payload_is_rejected() {
        for (framing, mut wire) in [
            (BatchFraming::PreNegotiation, vec![0xfe]),
            (NEGOTIATED, vec![0xfe, 0xff]),
        ] {
            wire.resize(wire.len() + MAX_DECOMPRESSED_BATCH_SIZE + 1, 0);
            assert_eq!(
                decode_batch(&wire, framing).unwrap_err(),
                BedrockFramingError::BatchTooLarge {
                    size: MAX_DECOMPRESSED_BATCH_SIZE + 1,
                    limit: MAX_DECOMPRESSED_BATCH_SIZE,
                }
            );
        }
    }

    #[test]
    fn test_round_trip_multiple_packets() {
        let p1 = b"client_to_server_handshake";
        let p2 = b"";
        let p3 = b"login";
        let p4 = vec![0xa5u8; 4096];
        let p5 = vec![0x3cu8; 65536];

        let packets: Vec<&[u8]> = vec![p1, p2, p3, &p4, &p5];
        let encoded = encode_batch(&packets, NEGOTIATED).expect("encode should succeed");
        assert_eq!(encoded[..2], [BATCH_PACKET_ID, COMPRESSION_DEFLATE]);

        let decoded = decode_batch(&encoded, NEGOTIATED).expect("decode should succeed");
        assert_eq!(decoded.len(), packets.len());
        for (i, expected) in packets.iter().enumerate() {
            assert_eq!(&decoded[i], expected);
        }
    }

    #[test]
    fn test_empty_batch() {
        // An empty payload is below any nonzero threshold.
        let encoded = encode_batch(&[], NEGOTIATED).expect("empty batch encoding should succeed");
        assert_eq!(encoded, [BATCH_PACKET_ID, COMPRESSION_NONE]);

        let decoded = decode_batch(&encoded, NEGOTIATED).expect("empty batch should decode");
        assert!(decoded.is_empty());

        // Also test decoding a batch that decompresses to 0 bytes directly.
        let decoded2 = decode_batch(&deflate_batch(&[]), NEGOTIATED)
            .expect("empty stream decoding should succeed");
        assert!(decoded2.is_empty());
    }

    #[test]
    fn test_decompression_bomb_rejection() {
        // One byte over the limit, then a significantly oversized stream. Both must
        // stop at the bound without inflating the rest.
        for size in [
            MAX_DECOMPRESSED_BATCH_SIZE + 1,
            MAX_DECOMPRESSED_BATCH_SIZE * 4,
        ] {
            let wire = deflate_batch(&vec![0u8; size]);
            assert_eq!(
                decode_batch(&wire, NEGOTIATED).unwrap_err(),
                BedrockFramingError::DecompressionBomb {
                    size: MAX_DECOMPRESSED_BATCH_SIZE + 1,
                    limit: MAX_DECOMPRESSED_BATCH_SIZE,
                }
            );
        }
    }

    #[test]
    fn test_malformed_header_rejection() {
        // Non-0xfe headers
        for invalid_header in [0x00, 0x01, 0x1c, 0xfd, 0xff] {
            let batch = [invalid_header, 0x00, 0x03, 0x00];
            assert_eq!(
                decode_batch(&batch, NEGOTIATED).unwrap_err(),
                BedrockFramingError::InvalidHeader(invalid_header)
            );
        }

        // Empty batch slice
        assert_eq!(
            decode_batch(&[], NEGOTIATED).unwrap_err(),
            BedrockFramingError::TruncatedPacket {
                expected: 1,
                available: 0,
            }
        );
    }

    #[test]
    fn test_truncated_length_or_payload() {
        // 1. Truncated VarInt length prefix: 0x80 has continuation bit set but stream ends.
        assert_eq!(
            decode_batch(&deflate_batch(&[0x80]), NEGOTIATED).unwrap_err(),
            BedrockFramingError::MalformedVarInt
        );

        // 2. Truncated payload: packet declares length 20, but only 5 bytes are in stream.
        let mut raw = Vec::new();
        write_varuint32(20, &mut raw);
        raw.extend_from_slice(&[1, 2, 3, 4, 5]);

        assert_eq!(
            decode_batch(&deflate_batch(&raw), NEGOTIATED).unwrap_err(),
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
            encode_batch(&packets, NEGOTIATED).unwrap_err(),
            BedrockFramingError::TooManyPackets(MAX_PACKETS_PER_BATCH + 1)
        );

        // Exactly MAX_PACKETS_PER_BATCH should succeed
        let max_packets = vec![&b"x"[..]; MAX_PACKETS_PER_BATCH];
        let encoded = encode_batch(&max_packets, NEGOTIATED).expect("256 packets should succeed");
        let decoded = decode_batch(&encoded, NEGOTIATED).expect("256 packets should decode");
        assert_eq!(decoded.len(), MAX_PACKETS_PER_BATCH);

        // Exceeding on decode (crafted raw batch with 257 packets)
        let mut raw = Vec::new();
        for _ in 0..=MAX_PACKETS_PER_BATCH {
            write_varuint32(0, &mut raw);
        }
        assert_eq!(
            decode_batch(&deflate_batch(&raw), NEGOTIATED).unwrap_err(),
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
    fn test_corrupt_deflate_decompression_failure() {
        // BTYPE=11 is reserved in RFC 1951.
        let corrupt = [BATCH_PACKET_ID, COMPRESSION_DEFLATE, 0x07, 0x34, 0x56];
        assert!(matches!(
            decode_batch(&corrupt, NEGOTIATED).unwrap_err(),
            BedrockFramingError::DecompressFailed(_)
        ));
    }

    #[test]
    fn test_encode_batch_too_large_rejection() {
        // Single packet exceeding MAX_DECOMPRESSED_BATCH_SIZE
        let huge_packet = vec![0xaa; MAX_DECOMPRESSED_BATCH_SIZE + 1];
        let err = encode_batch(&[&huge_packet], NEGOTIATED).unwrap_err();
        assert!(matches!(err, BedrockFramingError::BatchTooLarge { .. }));

        // Multiple packets whose aggregate uncompressed size exceeds limit
        let chunk1 = vec![0xbb; MAX_DECOMPRESSED_BATCH_SIZE / 2 + 10];
        let chunk2 = vec![0xcc; MAX_DECOMPRESSED_BATCH_SIZE / 2 + 10];
        let err = encode_batch(&[&chunk1, &chunk2], NEGOTIATED).unwrap_err();
        assert!(matches!(err, BedrockFramingError::BatchTooLarge { .. }));
    }
}
