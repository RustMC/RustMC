//! The PlayerInfoUpdate packet codec and the tab-list tracker.

use super::*;

/// Clientbound `PlayerInfoUpdate` packet (packet ID 0x47 in Java 26.3 protocol 777).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoUpdatePacket {
    pub actions: u8,
    pub entries: Vec<PlayerInfoEntry>,
}

impl PlayerInfoUpdatePacket {
    pub const PACKET_ID: u32 = PACKET_ID_PLAYER_INFO_UPDATE;

    /// Creates a new `PlayerInfoUpdatePacket` with given action bitmask and entries.
    pub fn new(actions: u8, entries: Vec<PlayerInfoEntry>) -> Self {
        Self { actions, entries }
    }

    /// Creates a packet containing a single entry.
    pub fn single(actions: u8, entry: PlayerInfoEntry) -> Self {
        Self {
            actions,
            entries: vec![entry],
        }
    }

    /// Encodes the packet into framed byte format (VarInt length + VarInt packet ID 0x47 + body).
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let payload = self.encode_payload()?;
        let mut header = Vec::with_capacity(5);
        write_varint(Self::PACKET_ID as i32, &mut header);
        let total_len = header.len() + payload.len();

        let mut framed = Vec::with_capacity(varint_len(total_len as u32) + total_len);
        write_varint(total_len as i32, &mut framed);
        framed.extend_from_slice(&header);
        framed.extend_from_slice(&payload);
        Ok(framed)
    }

    /// Encodes only the unframed packet payload (actions bitmask + entry count + entries).
    pub fn encode_payload(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut out = Vec::new();
        out.push(self.actions);
        write_varint(self.entries.len() as i32, &mut out);

        for entry in &self.entries {
            write_uuid(&entry.uuid, &mut out);

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::AddPlayer) {
                check_chars(&entry.name, MAX_NAME_CHARS)?;
                write_string(&entry.name, &mut out);
                check_length(entry.properties.len(), MAX_PROPERTIES)?;
                write_varint(entry.properties.len() as i32, &mut out);
                for prop in &entry.properties {
                    check_chars(&prop.name, MAX_PROPERTY_NAME_CHARS)?;
                    check_chars(&prop.value, MAX_STRING_CHARS)?;
                    write_string(&prop.name, &mut out);
                    write_string(&prop.value, &mut out);
                    match &prop.signature {
                        Some(sig) => {
                            check_chars(sig, MAX_PROPERTY_SIGNATURE_CHARS)?;
                            write_bool(true, &mut out);
                            write_string(sig, &mut out);
                        }
                        None => {
                            write_bool(false, &mut out);
                        }
                    }
                }
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::InitializeChat) {
                match &entry.chat_session {
                    Some(session) => {
                        check_length(session.public_key.len(), MAX_PUBLIC_KEY_BYTES)?;
                        check_length(session.key_signature.len(), MAX_KEY_SIGNATURE_BYTES)?;
                        write_bool(true, &mut out);
                        write_uuid(&session.session_id, &mut out);
                        out.extend_from_slice(&session.expires_at.to_be_bytes());
                        write_byte_array(&session.public_key, &mut out);
                        write_byte_array(&session.key_signature, &mut out);
                    }
                    None => {
                        write_bool(false, &mut out);
                    }
                }
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::UpdateGameMode) {
                write_varint(entry.game_mode.id(), &mut out);
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::UpdateListed) {
                write_bool(entry.listed, &mut out);
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::UpdateLatency) {
                write_varint(entry.latency, &mut out);
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::UpdateDisplayName) {
                match &entry.display_name {
                    Some(name) => {
                        write_bool(true, &mut out);
                        write_display_name_nbt(name, &mut out)?;
                    }
                    None => {
                        write_bool(false, &mut out);
                    }
                }
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::UpdateListOrder) {
                write_varint(entry.list_order, &mut out);
            }

            if PlayerInfoAction::is_set(self.actions, PlayerInfoAction::UpdateHat) {
                write_bool(entry.show_hat, &mut out);
            }
        }

        Ok(out)
    }

    /// Decodes a framed packet with VarInt length prefix and packet ID 0x47.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (frame_len, offset) = read_varint(bytes)?;
        if frame_len < 0 {
            return Err(ProtocolError::MalformedVarInt);
        }
        let frame_len = frame_len as usize;
        let expected_total = offset + frame_len;
        if bytes.len() < expected_total {
            return Err(ProtocolError::UnexpectedEof);
        }
        if bytes.len() > expected_total {
            return Err(ProtocolError::TrailingData {
                remaining: bytes.len() - expected_total,
            });
        }

        let frame_slice = &bytes[offset..expected_total];
        let (packet_id, id_offset) = read_varint(frame_slice)?;
        if packet_id as u32 != Self::PACKET_ID {
            return Err(ProtocolError::UnexpectedPacketId {
                expected: Self::PACKET_ID,
                received: packet_id as u32,
            });
        }

        let payload = &frame_slice[id_offset..];
        Self::decode_payload(payload)
    }

    /// Decodes an unframed packet payload (actions bitmask + entry count + entries).
    pub fn decode_payload(payload: &[u8]) -> Result<Self, ProtocolError> {
        let mut cursor = 0;
        if payload.is_empty() {
            return Err(ProtocolError::UnexpectedEof);
        }
        let actions = payload[cursor];
        cursor += 1;

        let (entry_count, n) = read_varint(&payload[cursor..])?;
        cursor += n;
        if entry_count < 0 {
            return Err(ProtocolError::MalformedVarInt);
        }
        let entry_count = entry_count as usize;

        let mut entries = Vec::with_capacity(entry_count.min(1024));

        for _ in 0..entry_count {
            let (uuid, n) = read_uuid(&payload[cursor..])?;
            cursor += n;

            let mut entry = PlayerInfoEntry {
                uuid,
                ..Default::default()
            };

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::AddPlayer) {
                let (name, n) = read_string(&payload[cursor..], MAX_NAME_CHARS)?;
                cursor += n;
                entry.name = name;

                let (prop_count, n) = read_varint(&payload[cursor..])?;
                cursor += n;
                if prop_count < 0 {
                    return Err(ProtocolError::MalformedVarInt);
                }
                let prop_count = prop_count as usize;
                check_length(prop_count, MAX_PROPERTIES)?;
                let mut props = Vec::with_capacity(prop_count);
                for _ in 0..prop_count {
                    let (prop_name, n) = read_string(&payload[cursor..], MAX_PROPERTY_NAME_CHARS)?;
                    cursor += n;
                    let (prop_val, n) = read_string(&payload[cursor..], MAX_STRING_CHARS)?;
                    cursor += n;
                    let (has_sig, n) = read_bool(&payload[cursor..])?;
                    cursor += n;
                    let sig = if has_sig {
                        let (sig_str, n) =
                            read_string(&payload[cursor..], MAX_PROPERTY_SIGNATURE_CHARS)?;
                        cursor += n;
                        Some(sig_str)
                    } else {
                        None
                    };
                    props.push(ProfileProperty {
                        name: prop_name,
                        value: prop_val,
                        signature: sig,
                    });
                }
                entry.properties = props;
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::InitializeChat) {
                let (has_chat, n) = read_bool(&payload[cursor..])?;
                cursor += n;
                if has_chat {
                    let (session_id, n) = read_uuid(&payload[cursor..])?;
                    cursor += n;
                    let (expires_at, _) = java_wire::read_i64_be(&payload[cursor..])
                        .map_err(|_| ProtocolError::UnexpectedEof)?;
                    cursor += 8;

                    let (public_key, n) =
                        read_byte_array(&payload[cursor..], MAX_PUBLIC_KEY_BYTES)?;
                    cursor += n;

                    let (key_signature, n) =
                        read_byte_array(&payload[cursor..], MAX_KEY_SIGNATURE_BYTES)?;
                    cursor += n;

                    entry.chat_session = Some(ChatSessionData {
                        session_id,
                        expires_at,
                        public_key,
                        key_signature,
                    });
                } else {
                    entry.chat_session = None;
                }
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::UpdateGameMode) {
                let (mode_id, n) = read_varint(&payload[cursor..])?;
                cursor += n;
                entry.game_mode = GameMode::from_id(mode_id)?;
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::UpdateListed) {
                let (listed, n) = read_bool(&payload[cursor..])?;
                cursor += n;
                entry.listed = listed;
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::UpdateLatency) {
                let (latency, n) = read_varint(&payload[cursor..])?;
                cursor += n;
                entry.latency = latency;
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::UpdateDisplayName) {
                let (has_display, n) = read_bool(&payload[cursor..])?;
                cursor += n;
                if has_display {
                    let (display_name, n) =
                        read_display_name_nbt(&payload[cursor..], MAX_STRING_CHARS)?;
                    cursor += n;
                    entry.display_name = Some(display_name);
                } else {
                    entry.display_name = None;
                }
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::UpdateListOrder) {
                let (list_order, n) = read_varint(&payload[cursor..])?;
                cursor += n;
                entry.list_order = list_order;
            }

            if PlayerInfoAction::is_set(actions, PlayerInfoAction::UpdateHat) {
                let (show_hat, n) = read_bool(&payload[cursor..])?;
                cursor += n;
                entry.show_hat = show_hat;
            }

            entries.push(entry);
        }

        if cursor < payload.len() {
            return Err(ProtocolError::TrailingData {
                remaining: payload.len() - cursor,
            });
        }

        Ok(Self { actions, entries })
    }
}

/// Tab-list tracker maintaining online player state and generating differential
/// update packets for broadcast to connected players.
#[derive(Debug, Clone, Default)]
pub struct TabListTracker {
    pub(super) players: BTreeMap<[u8; 16], PlayerInfoEntry>,
}

impl TabListTracker {
    /// Creates an empty `TabListTracker`.
    pub fn new() -> Self {
        Self {
            players: BTreeMap::new(),
        }
    }

    /// Returns the number of players currently tracked in the tab list.
    #[inline]
    pub fn len(&self) -> usize {
        self.players.len()
    }

    /// Returns true if no players are currently tracked.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.players.is_empty()
    }

    /// Checks if a player with the given UUID is currently online.
    #[inline]
    pub fn contains_player(&self, uuid: &[u8; 16]) -> bool {
        self.players.contains_key(uuid)
    }

    /// Gets a reference to the tracked player entry with the given UUID.
    #[inline]
    pub fn get_player(&self, uuid: &[u8; 16]) -> Option<&PlayerInfoEntry> {
        self.players.get(uuid)
    }

    /// Gets a mutable reference to the tracked player entry with the given UUID.
    #[inline]
    pub fn get_player_mut(&mut self, uuid: &[u8; 16]) -> Option<&mut PlayerInfoEntry> {
        self.players.get_mut(uuid)
    }

    /// Returns an iterator over all tracked online player entries in deterministic UUID order.
    pub fn players(&self) -> impl Iterator<Item = &PlayerInfoEntry> {
        self.players.values()
    }

    /// Adds a player to the tab list and returns a differential packet
    /// suitable for broadcasting the new player to all connected clients.
    pub fn add_player(&mut self, entry: PlayerInfoEntry) -> PlayerInfoUpdatePacket {
        let mut actions = PlayerInfoAction::AddPlayer
            | PlayerInfoAction::UpdateGameMode
            | PlayerInfoAction::UpdateListed
            | PlayerInfoAction::UpdateLatency
            | PlayerInfoAction::UpdateDisplayName
            | PlayerInfoAction::UpdateListOrder
            | PlayerInfoAction::UpdateHat;
        if entry.chat_session.is_some() {
            actions |= PlayerInfoAction::InitializeChat;
        }

        let packet = PlayerInfoUpdatePacket::single(actions, entry.clone());
        self.players.insert(entry.uuid, entry);
        packet
    }

    /// Removes a player from the tab list tracker by UUID.
    /// Returns the removed `PlayerInfoEntry` if the player was present.
    pub fn remove_player(&mut self, uuid: &[u8; 16]) -> Option<PlayerInfoEntry> {
        self.players.remove(uuid)
    }

    /// Updates the latency (ping in ms) for an online player and generates
    /// an `UpdateLatency` diff packet for broadcast. Returns `None` if the player is not online.
    pub fn update_latency(
        &mut self,
        uuid: &[u8; 16],
        latency: i32,
    ) -> Option<PlayerInfoUpdatePacket> {
        let entry = self.players.get_mut(uuid)?;
        entry.latency = latency;
        Some(PlayerInfoUpdatePacket::single(
            PlayerInfoAction::UpdateLatency as u8,
            PlayerInfoEntry {
                uuid: *uuid,
                latency,
                ..Default::default()
            },
        ))
    }

    /// Updates the game mode for an online player and generates
    /// an `UpdateGameMode` diff packet for broadcast. Returns `None` if the player is not online.
    pub fn update_game_mode(
        &mut self,
        uuid: &[u8; 16],
        game_mode: GameMode,
    ) -> Option<PlayerInfoUpdatePacket> {
        let entry = self.players.get_mut(uuid)?;
        entry.game_mode = game_mode;
        Some(PlayerInfoUpdatePacket::single(
            PlayerInfoAction::UpdateGameMode as u8,
            PlayerInfoEntry {
                uuid: *uuid,
                game_mode,
                ..Default::default()
            },
        ))
    }

    /// Updates the display name for an online player and generates
    /// an `UpdateDisplayName` diff packet for broadcast. Returns `None` if the player is not online.
    pub fn update_display_name(
        &mut self,
        uuid: &[u8; 16],
        display_name: Option<PlayerDisplayName>,
    ) -> Option<PlayerInfoUpdatePacket> {
        let entry = self.players.get_mut(uuid)?;
        entry.display_name = display_name.clone();
        Some(PlayerInfoUpdatePacket::single(
            PlayerInfoAction::UpdateDisplayName as u8,
            PlayerInfoEntry {
                uuid: *uuid,
                display_name,
                ..Default::default()
            },
        ))
    }

    /// Updates the list order for an online player and generates
    /// an `UpdateListOrder` diff packet for broadcast. Returns `None` if the player is not online.
    pub fn update_list_order(
        &mut self,
        uuid: &[u8; 16],
        list_order: i32,
    ) -> Option<PlayerInfoUpdatePacket> {
        let entry = self.players.get_mut(uuid)?;
        entry.list_order = list_order;
        Some(PlayerInfoUpdatePacket::single(
            PlayerInfoAction::UpdateListOrder as u8,
            PlayerInfoEntry {
                uuid: *uuid,
                list_order,
                ..Default::default()
            },
        ))
    }

    /// Updates the hat visibility for an online player and generates
    /// an `UpdateHat` diff packet for broadcast. Returns `None` if the player is not online.
    pub fn update_hat(
        &mut self,
        uuid: &[u8; 16],
        show_hat: bool,
    ) -> Option<PlayerInfoUpdatePacket> {
        let entry = self.players.get_mut(uuid)?;
        entry.show_hat = show_hat;
        Some(PlayerInfoUpdatePacket::single(
            PlayerInfoAction::UpdateHat as u8,
            PlayerInfoEntry {
                uuid: *uuid,
                show_hat,
                ..Default::default()
            },
        ))
    }

    /// Updates the listed status for an online player and generates
    /// an `UpdateListed` diff packet for broadcast. Returns `None` if the player is not online.
    pub fn update_listed(
        &mut self,
        uuid: &[u8; 16],
        listed: bool,
    ) -> Option<PlayerInfoUpdatePacket> {
        let entry = self.players.get_mut(uuid)?;
        entry.listed = listed;
        Some(PlayerInfoUpdatePacket::single(
            PlayerInfoAction::UpdateListed as u8,
            PlayerInfoEntry {
                uuid: *uuid,
                listed,
                ..Default::default()
            },
        ))
    }

    /// Generates a full synchronization packet containing all currently online players.
    /// This is sent to newly joined players to populate their entire tab list.
    pub fn full_tab_list_packet(&self) -> PlayerInfoUpdatePacket {
        let actions = PlayerInfoAction::ALL;
        let entries = self.players.values().cloned().collect();
        PlayerInfoUpdatePacket::new(actions, entries)
    }

    /// Clears all tracked players.
    pub fn clear(&mut self) {
        self.players.clear();
    }
}

// ---------------------------------------------------------------------------
// Protocol wire codec helpers
// ---------------------------------------------------------------------------
