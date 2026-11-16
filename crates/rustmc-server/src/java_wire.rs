//! Java Edition wire encoders and decoders shared by RustMC packet code.
//!
//! Cached preview chunk packets are built with these functions, so this file is
//! part of the packet-cache fingerprint in `build.rs`.

/// Maximum number of bytes in a 32-bit protocol VarInt.
const MAX_VARINT_BYTES: usize = 5;
/// Top four bits of the fifth VarInt byte that overflow `u32` or continue past five bytes.
const FIFTH_BYTE_DISALLOWED_BITS: u8 = 0xf0;

/// Bytes that can never become a valid value, however much more input arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Malformed;

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
pub(crate) fn read_varint(input: &[u8]) -> Result<Option<(u32, usize)>, Malformed> {
    let mut value = 0u32;
    for (i, byte) in input.iter().take(MAX_VARINT_BYTES).enumerate() {
        if i == MAX_VARINT_BYTES - 1 && byte & FIFTH_BYTE_DISALLOWED_BITS != 0 {
            return Err(Malformed);
        }
        value |= u32::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            if i > 0 && *byte == 0 {
                return Err(Malformed);
            }
            return Ok(Some((value, i + 1)));
        }
    }
    if input.len() >= MAX_VARINT_BYTES {
        Err(Malformed)
    } else {
        Ok(None)
    }
}

/// Decode a protocol string (VarInt byte length, then UTF-8) bounded by `max_bytes`.
pub(crate) fn read_string(input: &[u8], max_bytes: usize) -> Result<(&str, &[u8]), Malformed> {
    let Some((length, prefix)) = read_varint(input)? else {
        return Err(Malformed);
    };
    let length = length as usize;
    let rest = &input[prefix..];
    if length > max_bytes || rest.len() < length {
        return Err(Malformed);
    }
    let (bytes, remaining) = rest.split_at(length);
    let value = std::str::from_utf8(bytes).map_err(|_| Malformed)?;
    Ok((value, remaining))
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

        assert_eq!(read_varint(&[0xff, 0xff, 0xff, 0xff, 0x1f]), Err(Malformed));
        assert_eq!(read_varint(&[0x80, 0x80, 0x80, 0x80, 0x80]), Err(Malformed));
        assert_eq!(
            read_varint(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
            Err(Malformed)
        );
        assert_eq!(read_varint(&[0x80, 0x00]), Err(Malformed));
        assert_eq!(read_varint(&[0xff, 0x80, 0x00]), Err(Malformed));

        assert_eq!(read_varint(&[]), Ok(None));
        assert_eq!(read_varint(&[0x80]), Ok(None));
        assert_eq!(read_varint(&[0xff, 0xff, 0xff, 0xff]), Ok(None));
    }

    #[test]
    fn read_string_enforces_byte_limit_and_utf8() {
        let sample = [3, b'h', 0xc3, 0xa9, 9];
        assert_eq!(read_string(&sample, 255), Ok(("hé", &[9][..])));
        assert_eq!(read_string(&sample, 3), Ok(("hé", &[9][..])));
        assert_eq!(read_string(&sample, 2), Err(Malformed));

        assert_eq!(read_string(&[0], 255), Ok(("", &[][..])));

        assert_eq!(read_string(&[], 255), Err(Malformed));
        assert_eq!(read_string(&[0x80], 255), Err(Malformed));
        assert_eq!(read_string(&[3, b'h'], 255), Err(Malformed));
        assert_eq!(read_string(&[1, 0xff], 255), Err(Malformed));
    }
}
