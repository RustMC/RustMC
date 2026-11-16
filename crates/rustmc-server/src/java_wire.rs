//! Java Edition wire encoders shared by every packet RustMC writes.
//!
//! Cached preview chunk packets are built with these functions, so this file is
//! part of the packet-cache fingerprint in `build.rs`.

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
}
