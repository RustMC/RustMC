//! Unit tests for `net::framing`.

use super::*;

const SECRET: [u8; 16] = [3; 16];

fn round_trip(encoder: &mut FrameEncoder, decoder: &mut FrameDecoder, id: u32, body: &[u8]) {
    let mut wire = Vec::new();
    encoder.encode(id, body, &mut wire).expect("encode");
    // Feed one byte at a time to exercise partial reads.
    let mut decoded = None;
    for byte in &wire {
        decoder.push(&[*byte]).expect("push");
        if let Some(packet) = decoder.next_packet().expect("decode") {
            decoded = Some(packet);
        }
    }
    assert_eq!(
        decoded,
        Some(Packet {
            id,
            body: body.to_vec()
        })
    );
}

#[test]
fn plain_compressed_and_encrypted_frames_round_trip() {
    let mut encoder = FrameEncoder::new();
    let mut decoder = FrameDecoder::new(1 << 20);
    round_trip(&mut encoder, &mut decoder, 7, b"plain");
    encoder.enable_compression(64);
    decoder.enable_compression(64);
    round_trip(&mut encoder, &mut decoder, 8, b"short");
    round_trip(&mut encoder, &mut decoder, 9, &[42; 4096]);
    encoder.enable_encryption(&SECRET);
    decoder.enable_encryption(&SECRET);
    round_trip(&mut encoder, &mut decoder, 10, &[1; 300]);
    round_trip(&mut encoder, &mut decoder, 11, b"again");
}

#[test]
fn plain_frame_bytes_are_length_id_body() {
    let mut wire = Vec::new();
    FrameEncoder::new()
        .encode(2, &[0xaa, 0xbb], &mut wire)
        .expect("encode");
    assert_eq!(wire, [3, 2, 0xaa, 0xbb]);
}

#[test]
fn bytes_received_with_the_enabling_frame_are_decrypted_retroactively() {
    let mut first = Vec::new();
    FrameEncoder::new()
        .encode(1, b"key", &mut first)
        .expect("encode");
    let mut client = FrameEncoder::new();
    client.enable_encryption(&SECRET);
    let mut second = Vec::new();
    client.encode(3, b"after", &mut second).expect("encode");
    let mut decoder = FrameDecoder::new(1024);
    let mut both = first.clone();
    both.extend_from_slice(&second);
    decoder.push(&both).expect("push");
    assert_eq!(decoder.next_packet().expect("first").map(|p| p.id), Some(1));
    decoder.enable_encryption(&SECRET);
    assert_eq!(
        decoder.next_packet().expect("second"),
        Some(Packet {
            id: 3,
            body: b"after".to_vec()
        })
    );
}

#[test]
fn oversized_and_malformed_frames_are_rejected() {
    let mut decoder = FrameDecoder::new(16);
    decoder.push(&[100, 0]).expect("push");
    assert!(matches!(
        decoder.next_packet(),
        Err(FrameError::TooLarge { .. })
    ));
    let mut decoder = FrameDecoder::new(16);
    decoder.push(&[0xff, 0xff, 0xff, 0xff, 0x0f]).expect("push");
    assert!(decoder.next_packet().is_err());
    let mut decoder = FrameDecoder::new(16);
    assert!(decoder.push(&[0; 100]).is_err(), "buffer is bounded");
}

#[test]
fn compressed_frames_below_the_threshold_with_data_are_rejected() {
    // Data Length 3 (below threshold 64) claims compression: invalid.
    let mut decoder = FrameDecoder::new(1024);
    decoder.enable_compression(64);
    decoder.push(&[3, 3, 0x78, 0x9c]).expect("push");
    assert!(decoder.next_packet().is_err());
}
