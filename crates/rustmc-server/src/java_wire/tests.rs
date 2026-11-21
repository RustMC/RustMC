//! Unit tests for `java_wire`.

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

    for value in [i32::MIN, -1, 0, 0x0102_0304, i32::MAX] {
        let mut buf = value.to_be_bytes().to_vec();
        buf.push(0x99);
        assert_eq!(read_i32_be(&buf), Ok((value, &[0x99][..])));
    }
    for len in 0..4 {
        assert_eq!(read_i32_be(&[0; 4][..len]), Err(WireError::Truncated));
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

fn zlib_compress(raw: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(raw).unwrap();
    encoder.finish().unwrap()
}

fn manual_compressed_frame(data_len: u32, payload: &[u8]) -> Vec<u8> {
    let mut inner = Vec::new();
    put_varint(data_len, &mut inner);
    inner.extend_from_slice(payload);
    let mut out = Vec::new();
    put_varint(inner.len() as u32, &mut out);
    out.extend_from_slice(&inner);
    out
}

#[test]
fn set_compression_and_small_uncompressed_frame_match_pinned_bytes() {
    // Login Set Compression (0x03) carries a signed VarInt threshold.
    assert_eq!(encode_set_compression(256), [0x03, 0x03, 0x80, 0x02]);
    assert_eq!(encode_set_compression(0), [0x02, 0x03, 0x00]);
    assert_eq!(
        encode_set_compression(-1),
        [0x06, 0x03, 0xff, 0xff, 0xff, 0xff, 0x0f]
    );

    // Uncompressed packet under threshold=256: Packet Length=4, Data Length=0,
    // Packet ID=0x2a, Body=[9, 9].
    let mut out = Vec::new();
    frame_compressed(0x2a, &[9, 9], 256, &mut out).unwrap();
    assert_eq!(out, [0x04, 0x00, 0x2a, 0x09, 0x09]);
}

#[test]
fn compressed_frames_round_trip_below_at_and_above_threshold() {
    let threshold = 64;

    // Below threshold (1 byte ID + 62 bytes body = 63 < 64): Data Length = 0.
    let below_body = vec![0xab; 62];
    let mut below_wire = Vec::new();
    frame_compressed(0x15, &below_body, threshold, &mut below_wire).unwrap();
    let (pkt_len, prefix) = read_varint(&below_wire).unwrap().unwrap();
    assert_eq!(pkt_len as usize, 1 + 63);
    assert_eq!(below_wire[prefix], 0x00);
    let (id, decoded, rest) = read_compressed_frame(&below_wire, threshold, 4096).unwrap();
    assert!(rest.is_empty());
    assert_eq!(id, 0x15);
    assert_eq!(decoded, below_body);

    // Exactly at threshold (1 byte ID + 63 bytes body = 64 == 64): compressed.
    let at_body = vec![0xcd; 63];
    let mut at_wire = Vec::new();
    frame_compressed(0x15, &at_body, threshold, &mut at_wire).unwrap();
    let (_, prefix) = read_varint(&at_wire).unwrap().unwrap();
    let (data_len, _) = read_varint(&at_wire[prefix..]).unwrap().unwrap();
    assert_eq!(data_len, 64);
    let (id, decoded, rest) = read_compressed_frame(&at_wire, threshold, 4096).unwrap();
    assert!(rest.is_empty());
    assert_eq!(id, 0x15);
    assert_eq!(decoded, at_body);

    // Above threshold with multi-byte packet ID (ID=300 takes 2 VarInt bytes).
    let above_body: Vec<u8> = (0..1024).map(|i| (i % 251) as u8).collect();
    let mut above_wire = Vec::new();
    frame_compressed(300, &above_body, threshold, &mut above_wire).unwrap();
    let (_, prefix) = read_varint(&above_wire).unwrap().unwrap();
    let (data_len, _) = read_varint(&above_wire[prefix..]).unwrap().unwrap();
    assert_eq!(data_len as usize, 2 + above_body.len());

    // Also test concatenating two frames and decoding sequentially.
    let mut stream = above_wire.clone();
    frame_compressed(0x01, b"ok", threshold, &mut stream).unwrap();
    let (id1, dec1, rest) = read_compressed_frame(&stream, threshold, 4096).unwrap();
    assert_eq!(id1, 300);
    assert_eq!(dec1, above_body);
    let (id2, dec2, rest2) = read_compressed_frame(rest, threshold, 4096).unwrap();
    assert_eq!(id2, 0x01);
    assert_eq!(dec2, b"ok");
    assert!(rest2.is_empty());

    // Threshold 0 compresses even a 1-byte packet (ID only, empty body).
    let mut zero_threshold = Vec::new();
    frame_compressed(0x03, &[], 0, &mut zero_threshold).unwrap();
    let (_, prefix) = read_varint(&zero_threshold).unwrap().unwrap();
    assert_eq!(zero_threshold[prefix], 1);
    let (id, dec, rest) = read_compressed_frame(&zero_threshold, 0, 4096).unwrap();
    assert_eq!((id, dec.as_slice(), rest), (0x03, &[][..], &[][..]));
}

#[test]
fn compressed_packet_below_threshold_is_rejected_while_uncompressed_above_is_accepted() {
    let mut raw = Vec::new();
    put_varint(0x01, &mut raw);
    raw.extend_from_slice(b"tiny"); // 5 bytes < threshold 256
    let compressed = zlib_compress(&raw);
    let below_wire = manual_compressed_frame(raw.len() as u32, &compressed);
    assert_eq!(
        read_compressed_frame(&below_wire, 256, 4096),
        Err(WireError::BelowCompressionThreshold {
            length: 5,
            threshold: 256
        })
    );

    // Exact boundary: 255 < 256 is rejected, 256 == 256 is accepted.
    let mut raw_255 = vec![0x01];
    raw_255.resize(255, 0x42);
    let wire_255 = manual_compressed_frame(255, &zlib_compress(&raw_255));
    assert_eq!(
        read_compressed_frame(&wire_255, 256, 4096),
        Err(WireError::BelowCompressionThreshold {
            length: 255,
            threshold: 256
        })
    );

    // Uncompressed packets exceeding the threshold (Data Length = 0) are accepted per the wiki.
    let large_uncompressed = vec![0x55; 300];
    let mut raw_large = vec![0x02];
    raw_large.extend_from_slice(&large_uncompressed);
    let wire_uncomp = manual_compressed_frame(0, &raw_large);
    let (id, decoded, rest) = read_compressed_frame(&wire_uncomp, 256, 4096).unwrap();
    assert_eq!(id, 0x02);
    assert_eq!(decoded, large_uncompressed);
    assert!(rest.is_empty());
}

#[test]
fn declared_length_above_maximum_and_protocol_limits_are_rejected() {
    // Declared length exceeds caller max_len.
    let wire = manual_compressed_frame(513, &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
    assert_eq!(
        read_compressed_frame(&wire, 256, 512),
        Err(WireError::TooLong {
            length: 513,
            max: 512
        })
    );

    // Declared length exceeds protocol serverbound limit (2^23 = 8_388_608),
    // even when caller passes a larger max_len.
    let over_protocol = manual_compressed_frame(
        (MAX_UNCOMPRESSED_PACKET_BYTES + 1) as u32,
        &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
    );
    assert_eq!(
        read_compressed_frame(&over_protocol, 256, 16_000_000),
        Err(WireError::TooLong {
            length: MAX_UNCOMPRESSED_PACKET_BYTES + 1,
            max: MAX_UNCOMPRESSED_PACKET_BYTES
        })
    );

    // Exact 2^23 = 8_388_608 uncompressed length IS permitted when max_len allows it.
    let max_allowed = vec![0u8; MAX_UNCOMPRESSED_PACKET_BYTES];
    let mut max_wire = Vec::new();
    frame_compressed(0x00, &max_allowed[1..], 256, &mut max_wire).unwrap();
    let (id, decoded, rest) =
        read_compressed_frame(&max_wire, 256, MAX_UNCOMPRESSED_PACKET_BYTES).unwrap();
    assert_eq!(id, 0x00);
    assert_eq!(decoded.len(), MAX_UNCOMPRESSED_PACKET_BYTES - 1);
    assert!(rest.is_empty());

    // Packet Length > 2^21 - 1 (2_097_151) is rejected before reading frame body.
    let mut too_big_wire = Vec::new();
    put_varint((MAX_PACKET_WIRE_BYTES + 1) as u32, &mut too_big_wire);
    assert_eq!(
        read_compressed_frame(&too_big_wire, 256, MAX_UNCOMPRESSED_PACKET_BYTES),
        Err(WireError::TooLong {
            length: MAX_PACKET_WIRE_BYTES + 1,
            max: MAX_PACKET_WIRE_BYTES
        })
    );

    // Negative signed Data Length is rejected.
    let neg_dl = manual_compressed_frame((-1i32) as u32, &[0x78, 0x9c]);
    assert_eq!(
        read_compressed_frame(&neg_dl, 0, 4096),
        Err(WireError::InvalidCompression)
    );
}

#[test]
fn truncated_and_corrupt_zlib_streams_are_rejected() {
    let mut valid_wire = Vec::new();
    let body = vec![0x37; 300];
    frame_compressed(0x09, &body, 256, &mut valid_wire).unwrap();
    let (_, prefix) = read_varint(&valid_wire).unwrap().unwrap();
    let (data_len, dl_len) = read_varint(&valid_wire[prefix..]).unwrap().unwrap();
    let zlib_bytes = &valid_wire[prefix + dl_len..];

    // Outer frame truncated at every byte boundary.
    for cut in 0..valid_wire.len() {
        assert!(read_compressed_frame(&valid_wire[..cut], 256, 4096).is_err());
    }

    // Truncated zlib payload inside a complete frame.
    for cut in 0..zlib_bytes.len() {
        let wire = manual_compressed_frame(data_len, &zlib_bytes[..cut]);
        assert_eq!(
            read_compressed_frame(&wire, 256, 4096),
            Err(WireError::InvalidCompression)
        );
    }

    // Corrupt zlib header, deflate body, and Adler-32 trailer.
    for idx in [0, zlib_bytes.len() / 2, zlib_bytes.len() - 1] {
        let mut bad = zlib_bytes.to_vec();
        bad[idx] ^= 0xff;
        let wire = manual_compressed_frame(data_len, &bad);
        assert_eq!(
            read_compressed_frame(&wire, 256, 4096),
            Err(WireError::InvalidCompression)
        );
    }

    // Trailing byte after valid zlib stream inside the same frame.
    let mut trailing = zlib_bytes.to_vec();
    trailing.push(0x00);
    let trailing_wire = manual_compressed_frame(data_len, &trailing);
    assert_eq!(
        read_compressed_frame(&trailing_wire, 256, 4096),
        Err(WireError::InvalidCompression)
    );

    // Stream that finishes early with fewer bytes than declared.
    let under_wire = manual_compressed_frame(data_len + 10, zlib_bytes);
    assert_eq!(
        read_compressed_frame(&under_wire, 256, 4096),
        Err(WireError::DecompressedSizeMismatch {
            declared: data_len as usize + 10
        })
    );
}

#[test]
fn zip_bomb_inflating_past_declared_length_is_rejected_without_overallocation() {
    // 256 KiB of zeros compresses cleanly to a tiny frame, claiming only 300 bytes.
    let mut raw_bomb = vec![0x01];
    raw_bomb.resize(256 * 1024, 0);
    let bomb_zlib = zlib_compress(&raw_bomb);
    let declared = 300usize;
    assert!(bomb_zlib.len() < 4096);

    let mut out = Vec::new();
    assert_eq!(
        inflate_exact_into(&bomb_zlib, declared, &mut out),
        Err(WireError::DecompressedSizeMismatch { declared })
    );
    assert!(
        out.capacity() <= declared,
        "inflater allocated capacity {} > declared {}",
        out.capacity(),
        declared
    );

    let wire = manual_compressed_frame(declared as u32, &bomb_zlib);
    assert_eq!(
        read_compressed_frame(&wire, 256, 1_048_576),
        Err(WireError::DecompressedSizeMismatch { declared })
    );
}
