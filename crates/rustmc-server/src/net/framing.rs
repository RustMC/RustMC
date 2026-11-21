//! Stateful packet framing for one connection: length prefixes, optional
//! zlib compression and optional AES/CFB8 encryption.
//!
//! Why: login switches a connection to compression and then encryption at
//! fixed points, and every later byte must pass through both layers in order.
//! Done: incremental decoding of partial reads, retroactive decryption of
//! bytes that arrived with the frame that enabled encryption, bounded buffers.
//! Not done: nothing known.
//!
//! Source: minecraft.wiki `Java Edition protocol/Packets` ("Packet format",
//! "With compression") and `Java Edition protocol/Encryption` ("everything is
//! encrypted, including the length field, packet ID, and data length").

use std::fmt;

use super::crypto::{SHARED_SECRET_BYTES, StreamDecryptor, StreamEncryptor};
use crate::java_wire::{
    MAX_PACKET_WIRE_BYTES, WireError, frame, frame_compressed, read_compressed_frame, read_varint,
};

/// One decoded packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub id: u32,
    pub body: Vec<u8>,
}

/// Why a connection's byte stream cannot be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// A frame or its contents violate the protocol.
    Malformed(WireError),
    /// A packet is larger than this connection allows.
    TooLarge { length: usize, max: usize },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(error) => write!(f, "malformed packet: {error:?}"),
            Self::TooLarge { length, max } => write!(f, "packet of {length} bytes exceeds {max}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<WireError> for FrameError {
    fn from(error: WireError) -> Self {
        match error {
            WireError::TooLong { length, max } => Self::TooLarge { length, max },
            other => Self::Malformed(other),
        }
    }
}

/// Bytes a length prefix can add in front of the largest frame.
const LENGTH_PREFIX_BYTES: usize = 3;

/// Decodes the bytes a client sends.
pub struct FrameDecoder {
    buffer: Vec<u8>,
    cipher: Option<StreamDecryptor>,
    compression: Option<usize>,
    max_packet: usize,
}

impl FrameDecoder {
    /// A decoder that accepts packets of up to `max_packet` bytes (id and
    /// body, after decompression).
    pub fn new(max_packet: usize) -> Self {
        Self {
            buffer: Vec::new(),
            cipher: None,
            compression: None,
            max_packet: max_packet.min(MAX_PACKET_WIRE_BYTES),
        }
    }

    /// Decrypts everything not yet decoded and every later byte.
    pub fn enable_encryption(&mut self, secret: &[u8; SHARED_SECRET_BYTES]) {
        let mut cipher = StreamDecryptor::new(secret);
        cipher.apply(&mut self.buffer);
        self.cipher = Some(cipher);
    }

    /// Reads every later frame in the compressed format.
    pub fn enable_compression(&mut self, threshold: usize) {
        self.compression = Some(threshold);
    }

    /// Adds bytes read from the socket. Fails once more than one maximal
    /// frame is waiting, so a client cannot make the buffer grow unbounded.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), FrameError> {
        let start = self.buffer.len();
        self.buffer.extend_from_slice(bytes);
        if let Some(cipher) = &mut self.cipher {
            cipher.apply(&mut self.buffer[start..]);
        }
        let limit = self.max_packet + LENGTH_PREFIX_BYTES + DATA_LENGTH_BYTES;
        if self.buffer.len() > limit.saturating_mul(2) {
            return Err(FrameError::TooLarge {
                length: self.buffer.len(),
                max: limit,
            });
        }
        Ok(())
    }

    /// The next complete packet, if one is buffered.
    pub fn next_packet(&mut self) -> Result<Option<Packet>, FrameError> {
        let Some((length, prefix)) = read_varint(&self.buffer)? else {
            return Ok(None);
        };
        if prefix > LENGTH_PREFIX_BYTES {
            return Err(FrameError::Malformed(WireError::VarIntTooLong));
        }
        let length = length as usize;
        if length > self.max_packet + DATA_LENGTH_BYTES {
            return Err(FrameError::TooLarge {
                length,
                max: self.max_packet,
            });
        }
        let end = prefix + length;
        if self.buffer.len() < end {
            return Ok(None);
        }
        let packet = match self.compression {
            Some(threshold) => {
                let (id, body, _) =
                    read_compressed_frame(&self.buffer[..end], threshold, self.max_packet)?;
                Packet { id, body }
            }
            None => {
                let payload = &self.buffer[prefix..end];
                let Some((id, id_len)) = read_varint(payload)? else {
                    return Err(FrameError::Malformed(WireError::Truncated));
                };
                Packet {
                    id,
                    body: payload[id_len..].to_vec(),
                }
            }
        };
        self.buffer.drain(..end);
        Ok(Some(packet))
    }
}

/// Most bytes the compressed format's Data Length field adds.
const DATA_LENGTH_BYTES: usize = 5;

/// Encodes the bytes the server sends.
#[derive(Default)]
pub struct FrameEncoder {
    cipher: Option<StreamEncryptor>,
    compression: Option<usize>,
}

impl FrameEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Encrypts every later byte.
    pub fn enable_encryption(&mut self, secret: &[u8; SHARED_SECRET_BYTES]) {
        self.cipher = Some(StreamEncryptor::new(secret));
    }

    /// Frames every later packet in the compressed format.
    pub fn enable_compression(&mut self, threshold: usize) {
        self.compression = Some(threshold);
    }

    /// Appends one framed (and, when enabled, compressed and encrypted)
    /// packet to `out`.
    pub fn encode(&mut self, id: u32, body: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
        let start = out.len();
        match self.compression {
            Some(threshold) => frame_compressed(id, body, threshold, out)?,
            None => {
                let framed = frame(id, body);
                if framed.len() > MAX_PACKET_WIRE_BYTES + LENGTH_PREFIX_BYTES {
                    return Err(FrameError::TooLarge {
                        length: framed.len(),
                        max: MAX_PACKET_WIRE_BYTES,
                    });
                }
                out.extend_from_slice(&framed);
            }
        }
        if let Some(cipher) = &mut self.cipher {
            cipher.apply(&mut out[start..]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
