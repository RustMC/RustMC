//! Chat, chat command, signed chat command and system chat packet codecs.

use super::*;

/// The last-seen acknowledgment tail of `chat` and `chat_command_signed`.
///
/// These fields are mandatory, including when a message carries no signature.
/// RustMC never sends signed `player_chat` messages, so they are parsed for
/// framing and retained but not validated against a last-seen list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LastSeenUpdate {
    /// Message Count: messages seen since the client's last chat packet.
    pub message_count: u32,
    /// Acknowledged: Fixed BitSet (20), encoded as `ceil(20 / 8)` bytes.
    pub acknowledged: [u8; LAST_SEEN_BITSET_BYTES],
    /// Checksum over the last-seen signatures.
    pub checksum: u8,
}

impl LastSeenUpdate {
    pub(super) fn write(&self, out: &mut Vec<u8>) {
        put_varint(self.message_count, out);
        out.extend_from_slice(&self.acknowledged);
        out.push(self.checksum);
    }

    pub(super) fn read(input: &[u8]) -> Result<(Self, &[u8]), ProtocolError> {
        let (message_count, prefix) = read_varint(input)?;
        let (acknowledged, rest) = input[prefix..]
            .split_first_chunk::<LAST_SEEN_BITSET_BYTES>()
            .ok_or(ProtocolError::UnexpectedEof)?;
        let (checksum, rest) = rest.split_first().ok_or(ProtocolError::UnexpectedEof)?;
        Ok((
            Self {
                message_count,
                acknowledged: *acknowledged,
                checksum: *checksum,
            },
            rest,
        ))
    }
}

/// Serverbound `chat` packet (`0x09`).
///
/// Layout: String (256) message, Long timestamp, Long salt, Prefixed Optional
/// 256-byte signature, then the [`LastSeenUpdate`] tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatPacket {
    /// Message string (at most 256 UTF-16 code units).
    pub message: String,
    /// Timestamp (epoch milliseconds).
    pub timestamp: i64,
    /// Signature salt.
    pub salt: i64,
    /// Fixed-size signature, absent for unsigned messages.
    pub signature: Option<[u8; SIGNATURE_LENGTH]>,
    /// Mandatory acknowledgment tail.
    pub last_seen: LastSeenUpdate,
}

impl ChatPacket {
    /// Create a new ChatPacket with validation of the message length.
    pub fn new(
        message: impl Into<String>,
        timestamp: i64,
        salt: i64,
        signature: Option<[u8; SIGNATURE_LENGTH]>,
        last_seen: LastSeenUpdate,
    ) -> Result<Self, ProtocolError> {
        let message = message.into();
        check_utf16_len(&message, MAX_CHAT_LENGTH)?;
        Ok(Self {
            message,
            timestamp,
            salt,
            signature,
            last_seen,
        })
    }

    /// Create an unsigned ChatPacket with an empty acknowledgment tail.
    pub fn unsigned(
        message: impl Into<String>,
        timestamp: i64,
        salt: i64,
    ) -> Result<Self, ProtocolError> {
        Self::new(message, timestamp, salt, None, LastSeenUpdate::default())
    }

    /// Encode as an unframed packet: `[VarInt(0x09), payload]`.
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(CHAT_PACKET_ID, &mut out);
        put_string(&self.message, &mut out);
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.salt.to_be_bytes());
        match &self.signature {
            Some(signature) => {
                out.push(1);
                out.extend_from_slice(signature);
            }
            None => out.push(0),
        }
        self.last_seen.write(&mut out);
        out
    }

    /// Encode as a framed packet: `[VarInt(length), VarInt(0x09), payload]`.
    pub fn to_frame(&self) -> Vec<u8> {
        with_length(self.encode_packet())
    }

    /// Decode a framed packet: `[VarInt(length), VarInt(0x09), payload]`.
    pub fn decode_frame(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_frame(bytes, CHAT_PACKET_ID)?)
    }

    /// Decode an unframed packet: `[VarInt(0x09), payload]`.
    pub fn decode_packet(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_packet(bytes, CHAT_PACKET_ID)?)
    }

    /// Decode a payload without length or packet ID.
    pub fn decode_payload(body: &[u8]) -> Result<Self, ProtocolError> {
        let (message, consumed) = read_bounded_string(body, MAX_CHAT_LENGTH)?;
        let (timestamp, rest) = read_i64(&body[consumed..])?;
        let (salt, rest) = read_i64(rest)?;
        let (present, rest) = rest.split_first().ok_or(ProtocolError::UnexpectedEof)?;
        let (signature, rest) = match present {
            0 => (None, rest),
            1 => {
                let (signature, rest) = read_signature(rest)?;
                (Some(signature), rest)
            }
            other => {
                return Err(ProtocolError::Malformed(format!(
                    "Invalid boolean byte for signature presence: 0x{other:02X}"
                )));
            }
        };
        let (last_seen, rest) = LastSeenUpdate::read(rest)?;
        if !rest.is_empty() {
            return Err(ProtocolError::Malformed(
                "Trailing bytes in chat packet".into(),
            ));
        }
        Ok(Self {
            message: message.to_string(),
            timestamp,
            salt,
            signature,
            last_seen,
        })
    }
}

/// Serverbound `chat_command` packet (`0x07`): only the unsigned command text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatCommandPacket {
    /// Command text without its leading `/`.
    pub command: String,
}

impl ChatCommandPacket {
    /// Create a command packet, stripping any leading `/` and checking length.
    pub fn new(command: impl Into<String>) -> Result<Self, ProtocolError> {
        let raw = command.into();
        let command = raw.strip_prefix('/').unwrap_or(&raw).to_string();
        check_utf16_len(&command, MAX_COMMAND_LENGTH)?;
        Ok(Self { command })
    }

    /// Encode as an unframed packet: `[VarInt(0x07), command]`.
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(CHAT_COMMAND_PACKET_ID, &mut out);
        put_string(&self.command, &mut out);
        out
    }

    /// Encode as a framed packet: `[VarInt(length), VarInt(0x07), command]`.
    pub fn to_frame(&self) -> Vec<u8> {
        with_length(self.encode_packet())
    }

    /// Decode a framed packet: `[VarInt(length), VarInt(0x07), command]`.
    pub fn decode_frame(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_frame(bytes, CHAT_COMMAND_PACKET_ID)?)
    }

    /// Decode an unframed packet: `[VarInt(0x07), command]`.
    pub fn decode_packet(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_packet(bytes, CHAT_COMMAND_PACKET_ID)?)
    }

    /// Decode a payload without length or packet ID.
    pub fn decode_payload(body: &[u8]) -> Result<Self, ProtocolError> {
        let (command, consumed) = read_bounded_string(body, MAX_COMMAND_LENGTH)?;
        if consumed != body.len() {
            return Err(ProtocolError::Malformed(
                "Trailing bytes in chat_command packet".into(),
            ));
        }
        Ok(Self {
            command: command.to_string(),
        })
    }
}

/// One signed command argument: String (16) name and a fixed 256-byte signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentSignature {
    pub name: String,
    pub signature: [u8; SIGNATURE_LENGTH],
}

/// Serverbound `chat_command_signed` packet (`0x08`).
///
/// Layout: String (32767) command, Long timestamp, Long salt, Prefixed Array
/// (8) of [`ArgumentSignature`], then the [`LastSeenUpdate`] tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedChatCommandPacket {
    /// Command text without its leading `/`.
    pub command: String,
    /// Timestamp (epoch milliseconds).
    pub timestamp: i64,
    /// Signature salt.
    pub salt: i64,
    /// Signatures for signable arguments.
    pub argument_signatures: Vec<ArgumentSignature>,
    /// Mandatory acknowledgment tail.
    pub last_seen: LastSeenUpdate,
}

impl SignedChatCommandPacket {
    /// Create a signed command packet, validating every bounded field.
    pub fn new(
        command: impl Into<String>,
        timestamp: i64,
        salt: i64,
        argument_signatures: Vec<ArgumentSignature>,
        last_seen: LastSeenUpdate,
    ) -> Result<Self, ProtocolError> {
        let raw = command.into();
        let command = raw.strip_prefix('/').unwrap_or(&raw).to_string();
        check_utf16_len(&command, MAX_COMMAND_LENGTH)?;
        if argument_signatures.len() > MAX_ARGUMENT_SIGNATURES {
            return Err(ProtocolError::Oversized(format!(
                "{} argument signatures exceed the limit of {MAX_ARGUMENT_SIGNATURES}",
                argument_signatures.len()
            )));
        }
        for argument in &argument_signatures {
            check_utf16_len(&argument.name, MAX_ARGUMENT_NAME_LENGTH)?;
        }
        Ok(Self {
            command,
            timestamp,
            salt,
            argument_signatures,
            last_seen,
        })
    }

    /// Encode as an unframed packet: `[VarInt(0x08), payload]`.
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(CHAT_COMMAND_SIGNED_PACKET_ID, &mut out);
        put_string(&self.command, &mut out);
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.salt.to_be_bytes());
        put_varint(self.argument_signatures.len() as u32, &mut out);
        for argument in &self.argument_signatures {
            put_string(&argument.name, &mut out);
            out.extend_from_slice(&argument.signature);
        }
        self.last_seen.write(&mut out);
        out
    }

    /// Encode as a framed packet: `[VarInt(length), VarInt(0x08), payload]`.
    pub fn to_frame(&self) -> Vec<u8> {
        with_length(self.encode_packet())
    }

    /// Decode a framed packet: `[VarInt(length), VarInt(0x08), payload]`.
    pub fn decode_frame(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_frame(bytes, CHAT_COMMAND_SIGNED_PACKET_ID)?)
    }

    /// Decode an unframed packet: `[VarInt(0x08), payload]`.
    pub fn decode_packet(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_packet(bytes, CHAT_COMMAND_SIGNED_PACKET_ID)?)
    }

    /// Decode a payload without length or packet ID.
    pub fn decode_payload(body: &[u8]) -> Result<Self, ProtocolError> {
        let (command, consumed) = read_bounded_string(body, MAX_COMMAND_LENGTH)?;
        let (timestamp, rest) = read_i64(&body[consumed..])?;
        let (salt, rest) = read_i64(rest)?;
        let (count, prefix) = read_varint(rest)?;
        let count = count as usize;
        if count > MAX_ARGUMENT_SIGNATURES {
            return Err(ProtocolError::Oversized(format!(
                "{count} argument signatures exceed the limit of {MAX_ARGUMENT_SIGNATURES}"
            )));
        }
        let mut rest = &rest[prefix..];
        let mut argument_signatures = Vec::with_capacity(count);
        for _ in 0..count {
            let (name, consumed) = read_bounded_string(rest, MAX_ARGUMENT_NAME_LENGTH)?;
            let (signature, after) = read_signature(&rest[consumed..])?;
            argument_signatures.push(ArgumentSignature {
                name: name.to_string(),
                signature,
            });
            rest = after;
        }
        let (last_seen, rest) = LastSeenUpdate::read(rest)?;
        if !rest.is_empty() {
            return Err(ProtocolError::Malformed(
                "Trailing bytes in chat_command_signed packet".into(),
            ));
        }
        Ok(Self {
            command: command.to_string(),
            timestamp,
            salt,
            argument_signatures,
            last_seen,
        })
    }
}

/// Clientbound `system_chat` packet (`0x7C`): Text Component, Boolean overlay.
///
/// The component is validated and encoded on construction, so a value of this
/// type can always be written to the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemChatMessagePacket {
    pub(super) content: ChatComponent,
    pub(super) overlay: bool,
    pub(super) content_nbt: Vec<u8>,
}

impl SystemChatMessagePacket {
    /// Create a packet, rejecting a component that exceeds the wire bounds.
    pub fn new(content: ChatComponent, overlay: bool) -> Result<Self, ProtocolError> {
        let content_nbt = content.to_network_nbt()?;
        Ok(Self {
            content,
            overlay,
            content_nbt,
        })
    }

    /// Create a message for the chat box.
    pub fn chat(component: ChatComponent) -> Result<Self, ProtocolError> {
        Self::new(component, false)
    }

    /// Create a message for the action bar.
    pub fn overlay(component: ChatComponent) -> Result<Self, ProtocolError> {
        Self::new(component, true)
    }

    /// Create a plain message for the chat box.
    pub fn chat_text(text: impl Into<String>) -> Result<Self, ProtocolError> {
        Self::chat(ChatComponent::text(text))
    }

    /// Create a plain message for the action bar.
    pub fn overlay_text(text: impl Into<String>) -> Result<Self, ProtocolError> {
        Self::overlay(ChatComponent::text(text))
    }

    /// The displayed component.
    pub fn content(&self) -> &ChatComponent {
        &self.content
    }

    /// `true` for the action bar, `false` for the chat box.
    pub fn is_overlay(&self) -> bool {
        self.overlay
    }

    /// Encode as an unframed packet: `[VarInt(0x7C), network NBT, overlay]`.
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.content_nbt.len() + 3);
        put_varint(SYSTEM_CHAT_MESSAGE_PACKET_ID, &mut out);
        out.extend_from_slice(&self.content_nbt);
        out.push(u8::from(self.overlay));
        out
    }

    /// Encode as a framed packet: `[VarInt(length), VarInt(0x7C), payload]`.
    pub fn to_frame(&self) -> Vec<u8> {
        with_length(self.encode_packet())
    }

    /// Decode a framed packet: `[VarInt(length), VarInt(0x7C), payload]`.
    pub fn decode_frame(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_frame(bytes, SYSTEM_CHAT_MESSAGE_PACKET_ID)?)
    }

    /// Decode an unframed packet: `[VarInt(0x7C), payload]`.
    pub fn decode_packet(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::decode_payload(expect_packet(bytes, SYSTEM_CHAT_MESSAGE_PACKET_ID)?)
    }

    /// Decode a payload without length or packet ID.
    pub fn decode_payload(body: &[u8]) -> Result<Self, ProtocolError> {
        let (content, rest) = ChatComponent::read_network_nbt(body)?;
        let overlay = match rest {
            [0] => false,
            [1] => true,
            [other] => {
                return Err(ProtocolError::Malformed(format!(
                    "Invalid boolean byte for overlay flag: 0x{other:02X}"
                )));
            }
            _ => {
                return Err(ProtocolError::Malformed(
                    "Expected exactly 1 byte for overlay flag".into(),
                ));
            }
        };
        Self::new(content, overlay)
    }
}

// ============================================================================
// Network NBT text components
// ============================================================================
