//! Bounded Java Edition 26.3 status and optional local login experiment.

pub mod keepalive;

#[cfg(test)]
mod tests_keepalive;

use crate::java_packets::{configuration, handshake, intent, login, play, status};
use crate::java_preview::{MAX_HORIZONTAL_POSITION, MAX_VERTICAL_POSITION};
use crate::java_wire::{
    WireError, encode_set_compression, frame, frame_compressed, pack_block_position, put_string,
    put_varint, read_compressed_frame, read_f32_be, read_f64_be, read_i16_be, read_i64_be,
    read_u64_be, read_varint, reframe_compressed, unpack_block_position,
};
use crate::net::configuration as configuration_packets;
use crate::preview_data::RegistryManifest;
use crate::protocol::{
    chat,
    ids::{DynamicRegistryIds, ProtocolIds},
    slot,
};
use crate::world::inventory::{PlayerInventory, WINDOW_CREATIVE_DROP_SLOT, WINDOW_PLAYER_SLOTS};
use std::sync::Arc;

pub const PROTOCOL: i32 = 777;
pub const VERSION: &str = "26.3";
pub const MAX_FRAME: usize = 4096;
/// Chat frames held for the supervisor between two `receive` calls.
const MAX_QUEUED_BROADCASTS: usize = 64;
/// Vanilla offline-mode player names are 3 to 16 ASCII word characters.
const PLAYER_NAME_LENGTH: std::ops::RangeInclusive<usize> = 3..=16;
const UUID_BYTES: usize = 16;
/// Longest handshake host or known-pack string accepted, in bytes.
const MAX_SHORT_STRING: usize = 255;
/// Login refusals sent as a plain JSON text component; static ASCII only.
const DISCOVERY_ONLY_REASON: &str = "RustMC is discovery-only; login is unavailable.";
const VERSION_REASON: &str = "RustMC local preview requires Java 26.3 (protocol 777).";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Handshake,
    Status,
    Ping,
    Login,
    /// Login was requested but is unavailable; the hello receives a reason.
    LoginRefused(&'static str),
    LoginAcknowledgement,
    Configuration,
    ConfigurationData,
    Play,
    /// Status exchange finished.
    Done,
    /// A refusal reason was queued; the connection closes after it flushes.
    Refused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Malformed,
    Oversized,
    WrongState,
}

/// Maps wire-level decoding failures into [`Error::Malformed`].
impl From<WireError> for Error {
    fn from(_: WireError) -> Self {
        Self::Malformed
    }
}

fn known_packs() -> Vec<u8> {
    framed(configuration_packets::known_packs())
}

fn initial_configuration(manifest: Option<&RegistryManifest>) -> Vec<Vec<u8>> {
    configuration_packets::initial_configuration(manifest)
        .into_iter()
        .map(framed)
        .collect()
}

fn framed(packet: crate::net::framing::Packet) -> Vec<u8> {
    frame(packet.id, &packet.body)
}

fn recognizes_core_pack(body: &[u8]) -> Result<bool, Error> {
    configuration_packets::recognizes_core_pack(body).map_err(|error| match error {
        configuration_packets::PackListError::TooMany => Error::Oversized,
        configuration_packets::PackListError::Malformed => Error::Malformed,
    })
}

fn status(local_preview: bool) -> Vec<u8> {
    // Static ASCII is intentionally used; no untrusted content is interpolated.
    let description = if local_preview {
        "RustMC local terrain preview; read-only"
    } else {
        "RustMC discovery only; login unavailable"
    };
    let json = format!(
        "{{\"version\":{{\"name\":\"{VERSION}\",\"protocol\":{PROTOCOL}}},\"players\":{{\"max\":0,\"online\":0}},\"description\":{{\"text\":\"{description}\"}}}}"
    );
    let mut body = Vec::new();
    put_varint(json.len() as u32, &mut body);
    body.extend_from_slice(json.as_bytes());
    frame(status::clientbound::STATUS_RESPONSE, &body)
}

/// Login-phase disconnect carrying a JSON text component.
fn login_disconnect(reason: &'static str) -> Vec<u8> {
    // Reasons are fixed ASCII constants without quotes or backslashes.
    debug_assert!(reason.is_ascii() && !reason.contains(['"', '\\']));
    let mut body = Vec::new();
    put_string(&format!("{{\"text\":\"{reason}\"}}"), &mut body);
    frame(login::clientbound::LOGIN_DISCONNECT, &body)
}

/// Validate a login `hello` body: player name followed by the client UUID.
fn read_hello(body: &[u8]) -> Result<(&[u8], &[u8]), Error> {
    let Some((name_len, prefix)) = read_varint(body)? else {
        return Err(Error::Malformed);
    };
    let name_len = name_len as usize;
    if !PLAYER_NAME_LENGTH.contains(&name_len) || body.len() != prefix + name_len + UUID_BYTES {
        return Err(Error::Malformed);
    }
    let name = &body[prefix..prefix + name_len];
    if !name
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return Err(Error::Malformed);
    }
    Ok((name, &body[prefix + name_len..]))
}

/// The trailing byte of every serverbound `move_player_*` packet packs
/// on-ground (`0x01`) and horizontal-collision (`0x02`); other bits are unused.
const MOVEMENT_FLAGS: u8 = 0x03;
/// Hotbar slots selectable through serverbound `set_carried_item` (0 to 8).
const HOTBAR_SLOTS: i16 = 9;
/// Serverbound `player_abilities` carries only the flying flag (`0x02`).
const ABILITIES_FLYING: u8 = 0x02;
/// Serverbound `player_input` packs seven movement flags (forward, backward,
/// left, right, jump, shift, sprint) into the low bits; the top bit is unused.
const PLAYER_INPUT_FLAGS: u8 = 0x7f;

fn is_valid_rotation(yaw: f32, pitch: f32) -> bool {
    yaw.is_finite() && pitch.is_finite() && (-90.0..=90.0).contains(&pitch)
}

pub struct Session {
    state: State,
    input: Vec<u8>,
    bytes: usize,
    local_preview: bool,
    session_id: [u8; 16],
    registry_manifest: Option<Arc<RegistryManifest>>,
    protocol_ids: Option<Arc<ProtocolIds>>,
    dynamic_registry_ids: DynamicRegistryIds,
    inventory: PlayerInventory,
    pub preview: Option<crate::java_preview::Preview>,
    preview_seed: u64,
    preview_radius: u8,
    preview_terrain: crate::world::Terrain,
    vanilla_source: Option<crate::java_preview::VanillaSource>,
    player_name: String,
    dispatcher: crate::commands::CommandDispatcher,
    broadcasts: Vec<Vec<u8>>,
    compression_threshold: Option<usize>,
    compression_active: bool,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            state: State::Handshake,
            input: Vec::new(),
            bytes: 0,
            local_preview: false,
            session_id: [0; 16],
            registry_manifest: None,
            protocol_ids: None,
            dynamic_registry_ids: DynamicRegistryIds::default(),
            inventory: PlayerInventory::new(),
            preview: None,
            preview_seed: 0,
            preview_radius: 2,
            preview_terrain: crate::world::Terrain::Preview,
            vanilla_source: None,
            player_name: "Player".to_string(),
            dispatcher: crate::commands::CommandDispatcher::read_only_preview(0),
            broadcasts: Vec::new(),
            compression_threshold: None,
            compression_active: false,
        }
    }
}
impl Session {
    pub fn new(local_preview: bool) -> Self {
        Self {
            local_preview,
            session_id: if local_preview {
                uuid::Uuid::new_v4().into_bytes()
            } else {
                [0; 16]
            },
            ..Self::default()
        }
    }
    pub fn with_world(mut self, seed: u64, radius: u8, terrain: crate::world::Terrain) -> Self {
        self.preview_seed = seed;
        self.preview_radius = radius;
        self.preview_terrain = terrain;
        self
    }
    pub fn with_vanilla_source(mut self, source: crate::java_preview::VanillaSource) -> Self {
        self.vanilla_source = Some(source);
        self
    }
    pub fn with_protocol_ids(mut self, protocol_ids: Arc<ProtocolIds>) -> Self {
        self.protocol_ids = Some(protocol_ids);
        self
    }
    pub fn with_compression_threshold(mut self, threshold: Option<usize>) -> Self {
        self.compression_threshold = threshold;
        self
    }
    fn active_compression_threshold(&self) -> Option<usize> {
        self.compression_active
            .then_some(self.compression_threshold)
            .flatten()
    }
    fn encode_frame(&self, id: u32, body: &[u8]) -> Result<Vec<u8>, Error> {
        let Some(threshold) = self.active_compression_threshold() else {
            return Ok(frame(id, body));
        };
        let mut out = Vec::new();
        frame_compressed(id, body, threshold, &mut out).map_err(|err| match err {
            WireError::TooLong { .. } => Error::Oversized,
            _ => Error::Malformed,
        })?;
        Ok(out)
    }
    fn reframe_packet(&self, uncompressed: Vec<u8>) -> Result<Vec<u8>, Error> {
        let Some(threshold) = self.active_compression_threshold() else {
            return Ok(uncompressed);
        };
        let mut out = Vec::new();
        reframe_compressed(&uncompressed, threshold, &mut out).map_err(|err| match err {
            WireError::TooLong { .. } => Error::Oversized,
            _ => Error::Malformed,
        })?;
        Ok(out)
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn inventory(&self) -> &PlayerInventory {
        &self.inventory
    }
    pub fn with_registry_manifest(mut self, manifest: Arc<RegistryManifest>) -> Self {
        self.dynamic_registry_ids = DynamicRegistryIds::from_manifest(&manifest);
        self.registry_manifest = Some(manifest);
        self
    }
    /// Take the frames this session produced for every other Play connection.
    pub fn take_broadcasts(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.broadcasts)
    }
    fn run_command(&mut self, command: &str) -> Result<Vec<u8>, Error> {
        let sender = crate::commands::CommandSender::player(&self.player_name, (0.0, 64.0, 0.0), 2);
        let reply = match self.dispatcher.execute(&sender, command) {
            crate::commands::CommandResult::Success(msg) => {
                chat::SystemChatMessagePacket::chat_text(msg)
            }
            crate::commands::CommandResult::Error(err) => chat::SystemChatMessagePacket::chat(
                chat::ChatComponent::colored(err, chat::ChatColor::Red),
            ),
        }
        .map_err(|_| Error::Malformed)?;
        Ok(reply.to_frame())
    }
    pub fn receive(&mut self, bytes: &[u8], limit: usize) -> Result<Vec<Vec<u8>>, Error> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or(Error::Oversized)?;
        // Before play `limit` caps the whole session; during play the caller
        // enforces a rate budget, so it caps only this read.
        let counted = if self.state == State::Play {
            bytes.len()
        } else {
            self.bytes
        };
        if counted > limit || self.input.len().saturating_add(bytes.len()) > MAX_FRAME + 5 {
            return Err(Error::Oversized);
        }
        self.input.extend_from_slice(bytes);
        let mut responses = Vec::new();
        while let Some((size, prefix)) = read_varint(&self.input)? {
            let size = size as usize;
            if size == 0 {
                return Err(Error::Malformed);
            }
            if size > MAX_FRAME || size > limit {
                return Err(Error::Oversized);
            }
            if self.input.len() < prefix + size {
                break;
            }
            let (id, body_owned);
            let body: &[u8] = if let Some(threshold) = self.active_compression_threshold() {
                let max_uncompressed = MAX_FRAME.min(limit);
                let (decoded_id, decoded_body, rest) = read_compressed_frame(
                    &self.input[..prefix + size],
                    threshold,
                    max_uncompressed,
                )
                .map_err(|err| match err {
                    WireError::TooLong { .. } => Error::Oversized,
                    _ => Error::Malformed,
                })?;
                if !rest.is_empty() {
                    return Err(Error::Malformed);
                }
                self.input.drain(..prefix + size);
                id = decoded_id;
                body_owned = decoded_body;
                &body_owned
            } else {
                let packet = self.input[prefix..prefix + size].to_vec();
                self.input.drain(..prefix + size);
                let Some((decoded_id, id_len)) = read_varint(&packet)? else {
                    return Err(Error::Malformed);
                };
                id = decoded_id;
                body_owned = packet;
                &body_owned[id_len..]
            };
            match (self.state, id) {
                (State::Handshake, handshake::serverbound::INTENTION) => {
                    let Some((version, n)) = read_varint(body)? else {
                        return Err(Error::Malformed);
                    };
                    let body = &body[n..];
                    let Some((host_len, n)) = read_varint(body)? else {
                        return Err(Error::Malformed);
                    };
                    let host_len = host_len as usize;
                    if host_len > MAX_SHORT_STRING || body.len() < n + host_len + 2 {
                        return Err(Error::Malformed);
                    }
                    if std::str::from_utf8(&body[n..n + host_len]).is_err() {
                        return Err(Error::Malformed);
                    }
                    let rest = &body[n + host_len + 2..];
                    let Some((next, n)) = read_varint(rest)? else {
                        return Err(Error::Malformed);
                    };
                    if n != rest.len() {
                        return Err(Error::WrongState);
                    }
                    self.state = match next {
                        intent::STATUS => State::Status,
                        intent::LOGIN if !self.local_preview => {
                            State::LoginRefused(DISCOVERY_ONLY_REASON)
                        }
                        intent::LOGIN if version != PROTOCOL as u32 => {
                            State::LoginRefused(VERSION_REASON)
                        }
                        intent::LOGIN => State::Login,
                        _ => return Err(Error::WrongState),
                    };
                }
                (State::Status, status::serverbound::STATUS_REQUEST) if body.is_empty() => {
                    responses.push(status(self.local_preview));
                    self.state = State::Ping;
                }
                (State::Ping, status::serverbound::PING_REQUEST) if body.len() == 8 => {
                    responses.push(frame(status::clientbound::PONG_RESPONSE, body));
                    self.state = State::Done;
                }
                (State::LoginRefused(reason), login::serverbound::HELLO) => {
                    read_hello(body)?;
                    responses.push(login_disconnect(reason));
                    self.state = State::Refused;
                }
                (State::Login, login::serverbound::HELLO) => {
                    let (name, client_uuid) = read_hello(body)?;
                    self.player_name = String::from_utf8_lossy(name).to_string();
                    if let Some(threshold) = self.compression_threshold {
                        let threshold_i32 =
                            i32::try_from(threshold).map_err(|_| Error::Oversized)?;
                        // Quote from `https://minecraft.wiki/w/Java_Edition_protocol/Packets`
                        // (revision 3868170, Login flow):
                        // "Set Compression, if present, must be sent before Login Success.
                        // Note that anything sent after Set Compression must use the
                        // Post Compression packet format."
                        responses.push(encode_set_compression(threshold_i32));
                        self.compression_active = true;
                    }
                    // The client UUID is an opaque, unverified local-session value.
                    let mut success = client_uuid.to_vec();
                    put_varint(name.len() as u32, &mut success);
                    success.extend_from_slice(name);
                    put_varint(0, &mut success); // No profile properties or signed textures.
                    // 26.3 appends a separate connection-scoped session UUID. This is
                    // only a local identifier; it grants no authenticated identity.
                    success.extend_from_slice(&self.session_id);
                    responses
                        .push(self.encode_frame(login::clientbound::LOGIN_FINISHED, &success)?);
                    self.state = State::LoginAcknowledgement;
                }
                (State::LoginAcknowledgement, login::serverbound::LOGIN_ACKNOWLEDGED)
                    if body.is_empty() =>
                {
                    self.state = State::Configuration;
                    responses.push(self.reframe_packet(known_packs())?);
                }
                (
                    State::Configuration,
                    configuration::serverbound::CLIENT_INFORMATION
                    | configuration::serverbound::CUSTOM_PAYLOAD,
                ) => {} // No effect; bounded by the frame cap.
                (State::Configuration, configuration::serverbound::SELECT_KNOWN_PACKS) => {
                    if !recognizes_core_pack(body)? {
                        return Err(Error::WrongState);
                    }
                    self.state = State::ConfigurationData;
                    if self.registry_manifest.is_some() {
                        for packet in initial_configuration(self.registry_manifest.as_deref()) {
                            responses.push(self.reframe_packet(packet)?);
                        }
                    }
                }
                (State::ConfigurationData, configuration::serverbound::FINISH_CONFIGURATION)
                    if body.is_empty() =>
                {
                    let manifest = self.registry_manifest.as_ref().ok_or(Error::WrongState)?;
                    let preview = if let Some(source) = &self.vanilla_source {
                        crate::java_preview::Preview::new_vanilla(
                            self.preview_radius,
                            manifest,
                            source.clone(),
                        )
                    } else {
                        crate::java_preview::Preview::new(
                            self.preview_seed,
                            self.preview_radius,
                            self.preview_terrain,
                            manifest,
                        )
                    }
                    .map(|p| p.with_compression_threshold(self.compression_threshold))
                    .ok_or(Error::Malformed)?;
                    responses.extend(preview.initial(manifest).ok_or(Error::Malformed)?);
                    self.dispatcher.set_seed(self.preview_seed as i64);
                    self.preview = Some(preview);
                    self.state = State::Play;
                }
                (State::Play, id) => {
                    use play::serverbound as sb;
                    let preview = self.preview.as_mut().ok_or(Error::WrongState)?;
                    match id {
                        sb::ACCEPT_TELEPORTATION => {
                            let Some((&1, rest)) = body.split_first() else {
                                return Err(Error::Malformed);
                            };
                            let (x, rest) = read_f64_be(rest)?;
                            let (y, rest) = read_f64_be(rest)?;
                            let (z, rest) = read_f64_be(rest)?;
                            let (yaw, rest) = read_f32_be(rest)?;
                            let (pitch, rest) = read_f32_be(rest)?;
                            if !rest.is_empty()
                                || !is_valid_coord(x, y, z)
                                || !is_valid_rotation(yaw, pitch)
                            {
                                return Err(Error::Malformed);
                            }
                            preview.teleport_acknowledged = true;
                        }
                        sb::CHUNK_BATCH_RECEIVED => {
                            let (rate, rest) = read_f32_be(body)?;
                            // Clients report any positive desired rate; large values
                            // are valid on fast machines and only cap the batch size.
                            if !rest.is_empty()
                                || !preview.awaiting_batch
                                || !rate.is_finite()
                                || rate <= 0.0
                            {
                                return Err(Error::Malformed);
                            }
                            preview.awaiting_batch = false;
                        }
                        sb::CLIENT_TICK_END => {
                            if !body.is_empty() {
                                return Err(Error::Malformed);
                            }
                        }
                        sb::CLIENT_INFORMATION | sb::CUSTOM_PAYLOAD => {} // Bounded information/custom payloads have no effect.
                        sb::KEEP_ALIVE => {
                            let (id, rest) = read_u64_be(body)?;
                            if !rest.is_empty() || preview.pending_keepalive != Some(id) {
                                return Err(Error::Malformed);
                            }
                            preview.pending_keepalive = None;
                        }
                        sb::MOVE_PLAYER_POS => {
                            let (x, rest) = read_f64_be(body)?;
                            let (y, rest) = read_f64_be(rest)?;
                            let (z, rest) = read_f64_be(rest)?;
                            let &[flags] = rest else {
                                return Err(Error::Malformed);
                            };
                            if (flags & !MOVEMENT_FLAGS) != 0 || !is_valid_coord(x, y, z) {
                                return Err(Error::Malformed);
                            }
                            preview.move_to(x, y, z).map_err(|_| Error::Malformed)?;
                        }
                        sb::MOVE_PLAYER_POS_ROT => {
                            let (x, rest) = read_f64_be(body)?;
                            let (y, rest) = read_f64_be(rest)?;
                            let (z, rest) = read_f64_be(rest)?;
                            let (yaw, rest) = read_f32_be(rest)?;
                            let (pitch, rest) = read_f32_be(rest)?;
                            let &[flags] = rest else {
                                return Err(Error::Malformed);
                            };
                            if (flags & !MOVEMENT_FLAGS) != 0
                                || !is_valid_coord(x, y, z)
                                || !is_valid_rotation(yaw, pitch)
                            {
                                return Err(Error::Malformed);
                            }
                            preview.move_to(x, y, z).map_err(|_| Error::Malformed)?;
                        }
                        sb::MOVE_PLAYER_ROT => {
                            let (yaw, rest) = read_f32_be(body)?;
                            let (pitch, rest) = read_f32_be(rest)?;
                            let &[flags] = rest else {
                                return Err(Error::Malformed);
                            };
                            if (flags & !MOVEMENT_FLAGS) != 0 || !is_valid_rotation(yaw, pitch) {
                                return Err(Error::Malformed);
                            }
                        }
                        sb::MOVE_PLAYER_STATUS_ONLY => {
                            if body.len() != 1 || (body[0] & !MOVEMENT_FLAGS) != 0 {
                                return Err(Error::Malformed);
                            }
                        }
                        sb::PLAYER_ABILITIES => {
                            if body.len() != 1 || body[0] & !ABILITIES_FLYING != 0 {
                                return Err(Error::Malformed);
                            }
                        }
                        sb::PLAYER_INPUT => {
                            if body.len() != 1 || body[0] & !PLAYER_INPUT_FLAGS != 0 {
                                return Err(Error::Malformed);
                            }
                        }
                        PLAYER_ACTION_ID => {
                            let action = decode_player_action(body)?;
                            responses.push(
                                self.reframe_packet(acknowledge_block_change(action.sequence))?,
                            );
                        }
                        USE_ITEM_ON_ID => {
                            let action = decode_use_item_on(body)?;
                            responses.push(
                                self.reframe_packet(acknowledge_block_change(action.sequence))?,
                            );
                        }
                        sb::SET_CREATIVE_MODE_SLOT => {
                            // `Set Creative Mode Slot` (0x39 / 57, `Java Edition protocol/Packets`
                            // revision 3868170): `Slot` (Short) followed by `Clicked Item` (Slot,
                            // `Java Edition protocol/Slot data` revision 3763756).
                            //
                            // Player inventory window slots (`Java Edition protocol/Inventory`
                            // revision 3201379, `== Player Inventory ==`): main inventory `9..=35`
                            // maps to `PlayerInventory` `9..=35` and hotbar `36..=44` maps to
                            // `PlayerInventory` `0..=8`. Without `ProtocolIds` (when started
                            // without a vanilla data root containing `reports/registries.json`),
                            // creative slot packets remain bounded by frame/session limits and
                            // ignored.
                            if let Some(protocol_ids) = &self.protocol_ids {
                                let (window_slot, rest) = read_i16_be(body)?;
                                if window_slot != WINDOW_CREATIVE_DROP_SLOT
                                    && !WINDOW_PLAYER_SLOTS.contains(&window_slot)
                                {
                                    return Err(Error::Malformed);
                                }
                                let decoded = match slot::decode_slot(
                                    rest,
                                    protocol_ids,
                                    &self.dynamic_registry_ids,
                                ) {
                                    Ok((stack, consumed)) if consumed == rest.len() => Some(stack),
                                    // Creative tabs offer items with components RustMC does
                                    // not model yet; that is valid client input, so the slot
                                    // is no longer tracked rather than ending the session.
                                    Err(slot::SlotError::UnsupportedComponent(_)) => None,
                                    _ => return Err(Error::Malformed),
                                };
                                // Only main inventory and hotbar slots are modelled; drops
                                // (-1), crafting, armor and offhand slots are accepted and
                                // not stored.
                                if let Some(slot_index) =
                                    PlayerInventory::window_slot_to_index(window_slot)
                                {
                                    self.inventory
                                        .set_slot(slot_index, decoded.flatten())
                                        .map_err(|_| Error::Malformed)?;
                                }
                            } else if body.is_empty() {
                                return Err(Error::Malformed);
                            }
                        }
                        sb::USE_ITEM
                        | sb::PICK_ITEM_FROM_BLOCK
                        | sb::PICK_ITEM_FROM_ENTITY
                        | sb::CONTAINER_CLOSE => {
                            if body.is_empty() {
                                return Err(Error::Malformed);
                            }
                            // Creative inputs are bounded by the frame/session limits.
                            // The preview is immutable and does not apply them.
                        }
                        sb::PLAYER_COMMAND => {
                            let mut rest = body;
                            for _ in 0..3 {
                                let (_, n) = read_varint(rest)?.ok_or(Error::Malformed)?;
                                rest = &rest[n..];
                            }
                            if !rest.is_empty() {
                                return Err(Error::Malformed);
                            }
                            // Observer posture commands have no world effect.
                        }
                        sb::PLAYER_LOADED => {
                            if !body.is_empty() {
                                return Err(Error::Malformed);
                            }
                            preview.client_loaded = true;
                        }
                        sb::PUNCH => {
                            if !body.is_empty() {
                                return Err(Error::Malformed);
                            }
                            // Punch has no world effect in the preview.
                        }
                        sb::SET_CARRIED_ITEM => {
                            let (slot, rest) = read_i16_be(body)?;
                            if !rest.is_empty() || !(0..HOTBAR_SLOTS).contains(&slot) {
                                return Err(Error::Malformed);
                            }
                            self.inventory
                                .set_held_slot(slot as u8)
                                .map_err(|_| Error::Malformed)?;
                        }
                        sb::SPECTATOR_ACTION => {
                            if body != [0] {
                                return Err(Error::Malformed);
                            }
                            // Spectator action with no target.
                        }
                        sb::CHAT => {
                            // No secure chat is negotiated, so a signature is
                            // not verified and the text is relayed as system chat.
                            let packet = chat::ChatPacket::decode_payload(body)
                                .map_err(|_| Error::Malformed)?;
                            let raw_frame =
                                chat::broadcast_chat(&self.player_name, &packet.message)
                                    .map_err(|_| Error::Malformed)?
                                    .to_frame();
                            let frame = self.reframe_packet(raw_frame)?;
                            if self.broadcasts.len() >= MAX_QUEUED_BROADCASTS {
                                return Err(Error::Oversized);
                            }
                            self.broadcasts.push(frame.clone());
                            responses.push(frame);
                        }
                        sb::CHAT_COMMAND => {
                            let packet = chat::ChatCommandPacket::decode_payload(body)
                                .map_err(|_| Error::Malformed)?;
                            let reply = self.run_command(&packet.command)?;
                            responses.push(self.reframe_packet(reply)?);
                        }
                        sb::CHAT_COMMAND_SIGNED => {
                            let packet = chat::SignedChatCommandPacket::decode_payload(body)
                                .map_err(|_| Error::Malformed)?;
                            let reply = self.run_command(&packet.command)?;
                            responses.push(self.reframe_packet(reply)?);
                        }
                        // Only system chat is sent, so there is nothing to acknowledge.
                        sb::CHAT_ACK => {
                            let (ack_count, n) = read_varint(body)?.ok_or(Error::Malformed)?;
                            let _ = ack_count;
                            if n != body.len() {
                                return Err(Error::Malformed);
                            }
                        }
                        // A keyed client may announce a session; it is unused
                        // because the login packet negotiates no secure chat.
                        sb::CHAT_SESSION_UPDATE => {}
                        _ => {
                            eprintln!(
                                "event=preview_unsupported_packet packet_id={id} bytes={}",
                                body.len()
                            );
                            return Err(Error::WrongState);
                        }
                    }
                }
                (State::Configuration | State::ConfigurationData, _) => {
                    return Err(Error::WrongState);
                }
                _ => return Err(Error::WrongState),
            }
        }
        Ok(responses)
    }
}

mod actions;
pub use actions::*;

#[cfg(test)]
mod tests;
