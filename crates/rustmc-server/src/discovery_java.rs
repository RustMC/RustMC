//! Bounded Java Edition 26.3 status and optional local login experiment.

pub mod keepalive;

#[cfg(test)]
mod tests_keepalive;

use crate::java_packets::{configuration, handshake, intent, login, play, status};
use crate::java_preview::{MAX_HORIZONTAL_POSITION, MAX_VERTICAL_POSITION};
use crate::java_wire::{
    WireError, frame, pack_block_position, put_string, put_varint, read_f32_be, read_f64_be,
    read_i16_be, read_i64_be, read_string, read_u64_be, read_varint, unpack_block_position,
};
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
    let mut body = Vec::new();
    put_varint(1, &mut body);
    for text in ["minecraft", "core", VERSION] {
        put_string(text, &mut body);
    }
    frame(configuration::clientbound::SELECT_KNOWN_PACKS, &body)
}

fn registry_data(registry: &str, entries: &[String]) -> Vec<u8> {
    let mut body = Vec::new();
    put_string(registry, &mut body);
    put_varint(entries.len() as u32, &mut body);
    for entry in entries {
        put_string(entry, &mut body);
        body.push(0); // Entry definition comes from the negotiated core pack.
    }
    frame(configuration::clientbound::REGISTRY_DATA, &body)
}

fn initial_configuration(manifest: Option<&RegistryManifest>) -> Vec<Vec<u8>> {
    let mut features = Vec::new();
    put_varint(1, &mut features);
    put_string("minecraft:vanilla", &mut features);
    let mut packets = vec![frame(
        configuration::clientbound::UPDATE_ENABLED_FEATURES,
        &features,
    )];
    if let Some(manifest) = manifest {
        for (registry, entries) in &manifest.registries {
            packets.push(registry_data(registry, entries));
        }
    }
    let mut tags = Vec::new();
    put_varint(manifest.map_or(0, |m| m.tags.len()) as u32, &mut tags);
    if let Some(manifest) = manifest {
        for registry in &manifest.tags {
            put_string(&registry.registry, &mut tags);
            put_varint(registry.tags.len() as u32, &mut tags);
            for (name, ids) in &registry.tags {
                put_string(name, &mut tags);
                put_varint(ids.len() as u32, &mut tags);
                for id in ids {
                    put_varint(*id, &mut tags);
                }
            }
        }
    }
    packets.push(frame(configuration::clientbound::UPDATE_TAGS, &tags));
    packets.push(frame(configuration::clientbound::FINISH_CONFIGURATION, &[]));
    packets
}

fn recognizes_core_pack(body: &[u8]) -> Result<bool, Error> {
    let Some((count, prefix)) = read_varint(body)? else {
        return Err(Error::Malformed);
    };
    if count > 16 {
        return Err(Error::Oversized);
    }
    let mut remaining = &body[prefix..];
    let mut recognized = false;
    for _ in 0..count {
        let (namespace, rest) = read_string(remaining, MAX_SHORT_STRING)?;
        let (id, rest) = read_string(rest, MAX_SHORT_STRING)?;
        let (version, rest) = read_string(rest, MAX_SHORT_STRING)?;
        recognized |= namespace == "minecraft" && id == "core" && version == VERSION;
        remaining = rest;
    }
    if !remaining.is_empty() {
        return Err(Error::Malformed);
    }
    Ok(recognized)
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

/// Authoritative Overworld vertical bounds (-64..=319).
pub const MIN_BLOCK_Y: i32 = -64;
pub const MAX_BLOCK_Y: i32 = 319;

/// World border horizontal limits (+-30,000,000 blocks).
pub const MIN_BLOCK_XZ: i32 = -30_000_000;
pub const MAX_BLOCK_XZ: i32 = 30_000_000;

/// Unpacks a 64-bit packed Minecraft Java block position into (x, y, z).
///
/// Java block position encoding:
/// - x: 26-bit signed integer (bits 38..=63)
/// - z: 26-bit signed integer (bits 12..=37)
/// - y: 12-bit signed integer (bits 0..=11)
#[inline]
pub fn unpack_block_pos(packed: i64) -> (i32, i32, i32) {
    unpack_block_position(packed)
}

/// Packs (x, y, z) coordinates into a 64-bit Minecraft Java block position.
#[inline]
pub fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    pack_block_position(x, y, z)
}

/// Checks if block coordinates fall within the authoritative world bounds.
#[inline]
pub fn is_valid_block_pos(x: i32, y: i32, z: i32) -> bool {
    (MIN_BLOCK_XZ..=MAX_BLOCK_XZ).contains(&x)
        && (MIN_BLOCK_Y..=MAX_BLOCK_Y).contains(&y)
        && (MIN_BLOCK_XZ..=MAX_BLOCK_XZ).contains(&z)
}

/// The status phase or action of a PlayerAction packet from the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerActionStatus {
    StartedDigging = 0,
    CancelledDigging = 1,
    FinishedDigging = 2,
    DropItemStack = 3,
    DropItem = 4,
    ShootArrowOrFinishEating = 5,
    SwapItemInHand = 6,
}

impl TryFrom<u32> for PlayerActionStatus {
    type Error = Error;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::StartedDigging),
            1 => Ok(Self::CancelledDigging),
            2 => Ok(Self::FinishedDigging),
            3 => Ok(Self::DropItemStack),
            4 => Ok(Self::DropItem),
            5 => Ok(Self::ShootArrowOrFinishEating),
            6 => Ok(Self::SwapItemInHand),
            _ => Err(Error::Malformed),
        }
    }
}

/// Play serverbound `minecraft:player_action` in the official 26.3 packet report.
pub const PLAYER_ACTION_ID: u32 = play::serverbound::PLAYER_ACTION;
/// Play serverbound `minecraft:use_item_on` in the official 26.3 packet report.
pub const USE_ITEM_ON_ID: u32 = play::serverbound::USE_ITEM_ON;
/// Play clientbound `minecraft:block_changed_ack` in the official 26.3 packet report.
pub const BLOCK_CHANGED_ACK_ID: u32 = play::clientbound::BLOCK_CHANGED_ACK;

/// Serverbound PlayerAction (ID `0x29` / 41) packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerAction {
    pub status: PlayerActionStatus,
    pub pos: (i32, i32, i32),
    pub face: u8,
    pub sequence: u32,
}

/// Decodes a serverbound `PlayerAction` packet (ID `0x29` / 41).
pub fn decode_player_action(body: &[u8]) -> Result<PlayerAction, Error> {
    let Some((status_val, prefix)) = read_varint(body)? else {
        return Err(Error::Malformed);
    };
    let status = PlayerActionStatus::try_from(status_val)?;

    let (packed_pos, mut rest) = read_i64_be(&body[prefix..])?;
    let (x, y, z) = unpack_block_pos(packed_pos);
    if !is_valid_block_pos(x, y, z) {
        return Err(Error::Malformed);
    }

    if rest.is_empty() {
        return Err(Error::Malformed);
    }
    let face = rest[0];
    if face > 5 {
        return Err(Error::Malformed);
    }
    rest = &rest[1..];

    let Some((sequence, seq_len)) = read_varint(rest)? else {
        return Err(Error::Malformed);
    };
    if seq_len != rest.len() {
        return Err(Error::Malformed);
    }

    Ok(PlayerAction {
        status,
        pos: (x, y, z),
        face,
        sequence,
    })
}

/// Encodes a `PlayerAction` packet body.
pub fn encode_player_action(action: &PlayerAction) -> Vec<u8> {
    let mut body = Vec::new();
    put_varint(action.status as u32, &mut body);
    let packed = pack_block_pos(action.pos.0, action.pos.1, action.pos.2);
    body.extend_from_slice(&packed.to_be_bytes());
    body.push(action.face);
    put_varint(action.sequence, &mut body);
    body
}

/// Serverbound UseItemOn (ID `0x42` / 66) packet.
#[derive(Debug, Clone, PartialEq)]
pub struct UseItemOn {
    pub hand: u32,
    pub pos: (i32, i32, i32),
    pub face: u32,
    pub cursor_x: f32,
    pub cursor_y: f32,
    pub cursor_z: f32,
    pub inside_block: bool,
    pub world_border_hit: bool,
    pub sequence: u32,
}

impl UseItemOn {
    pub fn cursor(&self) -> (f32, f32, f32) {
        (self.cursor_x, self.cursor_y, self.cursor_z)
    }
}

/// Decodes a serverbound `UseItemOn` packet (ID `0x42` / 66).
pub fn decode_use_item_on(body: &[u8]) -> Result<UseItemOn, Error> {
    let Some((hand, prefix)) = read_varint(body)? else {
        return Err(Error::Malformed);
    };
    if hand > 1 {
        return Err(Error::Malformed);
    }

    let (packed_pos, rest) = read_i64_be(&body[prefix..])?;
    let (x, y, z) = unpack_block_pos(packed_pos);
    if !is_valid_block_pos(x, y, z) {
        return Err(Error::Malformed);
    }

    let Some((face, face_len)) = read_varint(rest)? else {
        return Err(Error::Malformed);
    };
    if face > 5 {
        return Err(Error::Malformed);
    }
    let rest = &rest[face_len..];

    let (cursor_x, rest) = read_f32_be(rest)?;
    let (cursor_y, rest) = read_f32_be(rest)?;
    let (cursor_z, mut rest) = read_f32_be(rest)?;
    if !cursor_x.is_finite()
        || !cursor_y.is_finite()
        || !cursor_z.is_finite()
        || !(0.0..=1.0).contains(&cursor_x)
        || !(0.0..=1.0).contains(&cursor_y)
        || !(0.0..=1.0).contains(&cursor_z)
    {
        return Err(Error::Malformed);
    }

    if rest.is_empty() {
        return Err(Error::Malformed);
    }
    let inside_byte = rest[0];
    if inside_byte > 1 {
        return Err(Error::Malformed);
    }
    let inside_block = inside_byte == 1;
    rest = &rest[1..];

    if rest.is_empty() {
        return Err(Error::Malformed);
    }
    let world_border_byte = rest[0];
    if world_border_byte > 1 {
        return Err(Error::Malformed);
    }
    let world_border_hit = world_border_byte == 1;
    rest = &rest[1..];

    if rest.is_empty() {
        return Err(Error::Malformed);
    }

    // Pinned Java 26.3 (1.21.2+) ServerboundUseItemOnPacket schema:
    // hand: VarInt, pos: Position (i64), face: VarInt,
    // cursor: 3x f32, inside_block: Boolean (u8), world_border_hit: Boolean (u8), sequence: VarInt
    let Some((sequence, seq_len)) = read_varint(rest)? else {
        return Err(Error::Malformed);
    };
    if seq_len != rest.len() {
        return Err(Error::Malformed);
    }

    Ok(UseItemOn {
        hand,
        pos: (x, y, z),
        face,
        cursor_x,
        cursor_y,
        cursor_z,
        inside_block,
        world_border_hit,
        sequence,
    })
}

/// Encodes a `UseItemOn` packet body according to the pinned Java 26.3 specification.
pub fn encode_use_item_on(action: &UseItemOn) -> Vec<u8> {
    let mut body = Vec::new();
    put_varint(action.hand, &mut body);
    let packed = pack_block_pos(action.pos.0, action.pos.1, action.pos.2);
    body.extend_from_slice(&packed.to_be_bytes());
    put_varint(action.face, &mut body);
    body.extend_from_slice(&action.cursor_x.to_be_bytes());
    body.extend_from_slice(&action.cursor_y.to_be_bytes());
    body.extend_from_slice(&action.cursor_z.to_be_bytes());
    body.push(if action.inside_block { 1 } else { 0 });
    body.push(if action.world_border_hit { 1 } else { 0 });
    put_varint(action.sequence, &mut body);
    body
}

/// Emits a framed `AcknowledgeBlockChange` packet (ID `0x04`).
pub fn acknowledge_block_change(sequence: u32) -> Vec<u8> {
    let mut body = Vec::new();
    put_varint(sequence, &mut body);
    frame(BLOCK_CHANGED_ACK_ID, &body)
}

/// Emits an unframed `AcknowledgeBlockChange` packet payload: `[0x04, varint(sequence)]`.
pub fn acknowledge_block_change_packet(sequence: u32) -> Vec<u8> {
    let mut packet = Vec::new();
    put_varint(BLOCK_CHANGED_ACK_ID, &mut packet);
    put_varint(sequence, &mut packet);
    packet
}

/// Decodes an unframed `AcknowledgeBlockChange` packet payload `[0x04, varint(sequence)]`.
pub fn decode_acknowledge_block_change_packet(bytes: &[u8]) -> Result<u32, Error> {
    let Some((id, prefix)) = read_varint(bytes)? else {
        return Err(Error::Malformed);
    };
    if id != BLOCK_CHANGED_ACK_ID {
        return Err(Error::Malformed);
    }
    let rest = &bytes[prefix..];
    let Some((sequence, seq_len)) = read_varint(rest)? else {
        return Err(Error::Malformed);
    };
    if seq_len != rest.len() {
        return Err(Error::Malformed);
    }
    Ok(sequence)
}

/// Decodes a framed `AcknowledgeBlockChange` packet: `[varint(length), 0x04, varint(sequence)]`.
pub fn decode_acknowledge_block_change(bytes: &[u8]) -> Result<u32, Error> {
    let Some((len, prefix)) = read_varint(bytes)? else {
        return Err(Error::Malformed);
    };
    if prefix + len as usize != bytes.len() {
        return Err(Error::Malformed);
    }
    decode_acknowledge_block_change_packet(&bytes[prefix..])
}

fn is_valid_coord(x: f64, y: f64, z: f64) -> bool {
    x.is_finite()
        && y.is_finite()
        && z.is_finite()
        && x.abs() <= MAX_HORIZONTAL_POSITION
        && y.abs() <= MAX_VERTICAL_POSITION
        && z.abs() <= MAX_HORIZONTAL_POSITION
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
            let packet = self.input[prefix..prefix + size].to_vec();
            self.input.drain(..prefix + size);
            let Some((id, id_len)) = read_varint(&packet)? else {
                return Err(Error::Malformed);
            };
            let body = &packet[id_len..];
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
                    // The client UUID is an opaque, unverified local-session value.
                    let mut success = client_uuid.to_vec();
                    put_varint(name.len() as u32, &mut success);
                    success.extend_from_slice(name);
                    put_varint(0, &mut success); // No profile properties or signed textures.
                    // 26.3 appends a separate connection-scoped session UUID. This is
                    // only a local identifier; it grants no authenticated identity.
                    success.extend_from_slice(&self.session_id);
                    responses.push(frame(login::clientbound::LOGIN_FINISHED, &success));
                    self.state = State::LoginAcknowledgement;
                }
                (State::LoginAcknowledgement, login::serverbound::LOGIN_ACKNOWLEDGED)
                    if body.is_empty() =>
                {
                    self.state = State::Configuration;
                    responses.push(known_packs());
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
                        responses.extend(initial_configuration(self.registry_manifest.as_deref()));
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
                            responses.push(acknowledge_block_change(action.sequence));
                        }
                        USE_ITEM_ON_ID => {
                            let action = decode_use_item_on(body)?;
                            responses.push(acknowledge_block_change(action.sequence));
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
                            let frame = chat::broadcast_chat(&self.player_name, &packet.message)
                                .map_err(|_| Error::Malformed)?
                                .to_frame();
                            if self.broadcasts.len() >= MAX_QUEUED_BROADCASTS {
                                return Err(Error::Oversized);
                            }
                            self.broadcasts.push(frame.clone());
                            responses.push(frame);
                        }
                        sb::CHAT_COMMAND => {
                            let packet = chat::ChatCommandPacket::decode_payload(body)
                                .map_err(|_| Error::Malformed)?;
                            responses.push(self.run_command(&packet.command)?);
                        }
                        sb::CHAT_COMMAND_SIGNED => {
                            let packet = chat::SignedChatCommandPacket::decode_payload(body)
                                .map_err(|_| Error::Malformed)?;
                            responses.push(self.run_command(&packet.command)?);
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

#[cfg(test)]
mod tests {
    use super::*;
    fn handshake(version: u32) -> Vec<u8> {
        let mut body = Vec::new();
        put_varint(version, &mut body);
        body.push(1);
        body.push(b'x');
        body.extend(25565u16.to_be_bytes());
        body.push(1);
        frame(0, &body)
    }
    #[test]
    fn split_status_and_ping() {
        let mut s = Session::default();
        let h = handshake(PROTOCOL as u32);
        for b in h {
            assert!(s.receive(&[b], 4096).unwrap().is_empty());
        }
        assert_eq!(s.state(), State::Status);
        let answer = s.receive(&frame(0, &[]), 4096).unwrap();
        assert!(String::from_utf8_lossy(&answer[0]).contains("777"));
        let nonce = [9u8; 8];
        assert_eq!(
            s.receive(&frame(1, &nonce), 4096).unwrap()[0],
            frame(1, &nonce)
        );
        assert_eq!(s.state(), State::Done);
    }
    #[test]
    fn older_version_gets_current_status() {
        let mut s = Session::default();
        s.receive(&handshake(766), 4096).unwrap();
        assert!(
            String::from_utf8_lossy(&s.receive(&frame(0, &[]), 4096).unwrap()[0]).contains("777")
        );
    }
    #[test]
    fn malformed_and_oversized() {
        assert_eq!(
            Session::default().receive(&[0x80, 0x80, 0x80, 0x80, 0x80], 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            Session::default().receive(&[0x81, 0x20], 4096),
            Err(Error::Oversized)
        );
        assert_eq!(
            Session::default().receive(&frame(0, &[]), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            Session::default().receive(&handshake(777), 3),
            Err(Error::Oversized)
        );
    }
    fn hello(name: &[u8]) -> Vec<u8> {
        let mut body = vec![name.len() as u8];
        body.extend_from_slice(name);
        body.extend_from_slice(&[7; 16]);
        frame(login::serverbound::HELLO, &body)
    }

    fn assert_refused(mut session: Session, version: u32, reason: &str) {
        let mut h = handshake(version);
        *h.last_mut().unwrap() = intent::LOGIN as u8;
        assert!(session.receive(&h, 4096).unwrap().is_empty());
        let reply = session.receive(&hello(b"Test"), 4096).unwrap();
        assert_eq!(reply.len(), 1);
        assert_eq!(reply[0][1], login::clientbound::LOGIN_DISCONNECT as u8);
        let text = String::from_utf8_lossy(&reply[0]);
        assert!(text.contains(&format!("{{\"text\":\"{reason}\"}}")));
        assert_eq!(session.state(), State::Refused);
        // Nothing further is accepted after the refusal.
        assert_eq!(
            session.receive(&hello(b"Test"), 4096),
            Err(Error::WrongState)
        );
    }

    #[test]
    fn discovery_only_login_is_refused_with_a_reason() {
        assert_refused(Session::default(), 777, DISCOVERY_ONLY_REASON);
    }

    #[test]
    fn refused_login_still_validates_the_hello() {
        let mut h = handshake(777);
        *h.last_mut().unwrap() = intent::LOGIN as u8;
        let mut session = Session::default();
        session.receive(&h, 4096).unwrap();
        assert_eq!(session.receive(&hello(b"x!"), 4096), Err(Error::Malformed));
    }

    #[test]
    fn local_preview_login_is_version_gated_and_unverified() {
        assert_refused(Session::new(true), 766, VERSION_REASON);
        let mut current = handshake(777);
        *current.last_mut().unwrap() = 2;
        let mut s = Session::new(true);
        s.receive(&current, 4096).unwrap();
        assert_eq!(s.state(), State::Login);
        let mut login = vec![4];
        login.extend_from_slice(b"Test");
        login.extend_from_slice(&[7; 16]);
        let reply = s.receive(&frame(0, &login), 4096).unwrap();
        assert_eq!(reply.len(), 1);
        assert!(reply[0].windows(16).any(|w| w == [7; 16]));
        assert_eq!(s.state(), State::LoginAcknowledgement);
        s.receive(&frame(3, &[]), 4096).unwrap();
        assert_eq!(s.state(), State::Configuration);
        let mut known = vec![1];
        for text in ["minecraft", "core", VERSION] {
            put_string(text, &mut known);
        }
        s.receive(&frame(7, &known), 4096).unwrap();
        assert_eq!(s.state(), State::ConfigurationData);
    }

    #[test]
    fn known_pack_reply_rejects_oversized_and_malformed_lists() {
        assert_eq!(recognizes_core_pack(&[17]), Err(Error::Oversized));
        assert_eq!(recognizes_core_pack(&[1, 255, 255]), Err(Error::Malformed));
        assert_eq!(recognizes_core_pack(&[0, 1]), Err(Error::Malformed));
        assert_eq!(recognizes_core_pack(&[0]), Ok(false));
    }

    #[test]
    fn invalid_host_text_and_out_of_order_ping_are_rejected() {
        let mut h = handshake(777);
        h[5] = 0xff;
        assert_eq!(Session::default().receive(&h, 4096), Err(Error::Malformed));
        let mut s = Session::default();
        s.receive(&handshake(777), 4096).unwrap();
        assert_eq!(s.receive(&frame(1, &[0; 8]), 4096), Err(Error::WrongState));
    }

    #[test]
    fn test_unpack_and_pack_block_pos() {
        let test_coords = [
            (0, 0, 0),
            (1, 2, 3),
            (-1, -2, -3),
            (10, 20, 30),
            (-10, -20, -30),
            (12345, -64, -98765),
            (-30_000_000, -64, -30_000_000),
            (30_000_000, 319, 30_000_000),
            (33_554_431, 2047, 33_554_431),
            (-33_554_432, -2048, -33_554_432),
            (1, -1, 1),
            (-1, 1, -1),
        ];
        for (x, y, z) in test_coords {
            let packed = pack_block_pos(x, y, z);
            let unpacked = unpack_block_pos(packed);
            assert_eq!((x, y, z), unpacked, "round trip failed for ({x}, {y}, {z})");
        }
    }

    #[test]
    fn test_player_action_decoding_positive_and_negative_coords() {
        // Positive coordinates
        let action_pos = PlayerAction {
            status: PlayerActionStatus::StartedDigging,
            pos: (1234, 100, 5678),
            face: 1, // Up
            sequence: 42,
        };
        let encoded = encode_player_action(&action_pos);
        let decoded = decode_player_action(&encoded).expect("valid positive coords player action");
        assert_eq!(decoded, action_pos);

        // Negative coordinates
        let action_neg = PlayerAction {
            status: PlayerActionStatus::FinishedDigging,
            pos: (-500, -64, -1200),
            face: 2, // North
            sequence: 99,
        };
        let encoded_neg = encode_player_action(&action_neg);
        let decoded_neg =
            decode_player_action(&encoded_neg).expect("valid negative coords player action");
        assert_eq!(decoded_neg, action_neg);

        // Test all status variants (0..=6)
        let statuses = [
            PlayerActionStatus::StartedDigging,
            PlayerActionStatus::CancelledDigging,
            PlayerActionStatus::FinishedDigging,
            PlayerActionStatus::DropItemStack,
            PlayerActionStatus::DropItem,
            PlayerActionStatus::ShootArrowOrFinishEating,
            PlayerActionStatus::SwapItemInHand,
        ];
        for (i, status) in statuses.into_iter().enumerate() {
            let action = PlayerAction {
                status,
                pos: (i as i32 * 10, (i as i32 * 10) - 20, -(i as i32 * 10)),
                face: (i % 6) as u8,
                sequence: i as u32 + 1,
            };
            let enc = encode_player_action(&action);
            let dec = decode_player_action(&enc).expect("status variant decode");
            assert_eq!(dec, action);
        }

        // Test all faces 0..=5
        for face in 0..=5 {
            let action = PlayerAction {
                status: PlayerActionStatus::StartedDigging,
                pos: (0, 0, 0),
                face,
                sequence: 1,
            };
            let enc = encode_player_action(&action);
            let dec = decode_player_action(&enc).expect("face variant decode");
            assert_eq!(dec.face, face);
        }
    }

    #[test]
    fn test_use_item_on_decoding_cursor_and_faces() {
        // Positive & negative coords with main hand
        let use_item = UseItemOn {
            hand: 0,
            pos: (42, 64, -84),
            face: 1, // Up
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.25,
            inside_block: false,
            world_border_hit: false,
            sequence: 10,
        };
        let encoded = encode_use_item_on(&use_item);
        let decoded = decode_use_item_on(&encoded).expect("valid use_item_on");
        assert_eq!(decoded, use_item);
        assert_eq!(decoded.cursor(), (0.5, 1.0, 0.25));

        // Off-hand with negative coords and inside_block = true
        let use_item_off = UseItemOn {
            hand: 1,
            pos: (-100, -50, 200),
            face: 4, // West
            cursor_x: 0.0,
            cursor_y: 0.0,
            cursor_z: 0.0,
            inside_block: true,
            world_border_hit: false,
            sequence: 100,
        };
        let encoded_off = encode_use_item_on(&use_item_off);
        let decoded_off = decode_use_item_on(&encoded_off).expect("valid off-hand use_item_on");
        assert_eq!(decoded_off, use_item_off);

        // Edge cursor coordinates (1.0, 1.0, 1.0)
        let use_edge = UseItemOn {
            hand: 0,
            pos: (0, 0, 0),
            face: 5,
            cursor_x: 1.0,
            cursor_y: 1.0,
            cursor_z: 1.0,
            inside_block: false,
            world_border_hit: false,
            sequence: 55,
        };
        let enc_edge = encode_use_item_on(&use_edge);
        let dec_edge = decode_use_item_on(&enc_edge).expect("edge cursor use_item_on");
        assert_eq!(dec_edge, use_edge);

        // All faces 0..=5
        for face in 0..=5 {
            let action = UseItemOn {
                hand: 0,
                pos: (0, 0, 0),
                face,
                cursor_x: 0.5,
                cursor_y: 0.5,
                cursor_z: 0.5,
                inside_block: false,
                world_border_hit: false,
                sequence: 1,
            };
            let enc = encode_use_item_on(&action);
            let dec = decode_use_item_on(&enc).expect("face decode");
            assert_eq!(dec.face, face);
        }

        // Test with world_border_hit: u8 representation
        let mut with_border = Vec::new();
        put_varint(0, &mut with_border); // hand
        let packed = pack_block_pos(10, 20, 30);
        with_border.extend_from_slice(&packed.to_be_bytes());
        put_varint(3, &mut with_border); // face: South
        with_border.extend_from_slice(&0.25f32.to_be_bytes());
        with_border.extend_from_slice(&0.5f32.to_be_bytes());
        with_border.extend_from_slice(&0.75f32.to_be_bytes());
        with_border.push(0); // inside_block
        with_border.push(1); // world_border_hit
        put_varint(77, &mut with_border); // sequence
        let dec_border =
            decode_use_item_on(&with_border).expect("use_item_on with world_border_hit");
        assert_eq!(dec_border.hand, 0);
        assert_eq!(dec_border.pos, (10, 20, 30));
        assert_eq!(dec_border.face, 3);
        assert_eq!(dec_border.cursor_x, 0.25);
        assert_eq!(dec_border.cursor_y, 0.5);
        assert_eq!(dec_border.cursor_z, 0.75);
        assert!(!dec_border.inside_block);
        assert!(dec_border.world_border_hit);
        assert_eq!(dec_border.sequence, 77);
    }

    #[test]
    fn test_malformed_packet_rejection() {
        // --- PlayerAction malformed tests ---
        // 1. Truncated bytes
        assert_eq!(decode_player_action(&[]), Err(Error::Malformed));
        assert_eq!(decode_player_action(&[0]), Err(Error::Malformed)); // Status only, missing pos
        let mut partial = vec![0];
        partial.extend_from_slice(&[0; 4]); // Only 4 bytes of pos
        assert_eq!(decode_player_action(&partial), Err(Error::Malformed));
        let mut missing_face = vec![0];
        missing_face.extend_from_slice(&0i64.to_be_bytes());
        assert_eq!(decode_player_action(&missing_face), Err(Error::Malformed));
        let mut missing_seq = missing_face.clone();
        missing_seq.push(1); // Face present, missing sequence
        assert_eq!(decode_player_action(&missing_seq), Err(Error::Malformed));
        let mut truncated_varint = missing_seq.clone();
        truncated_varint.push(0x80); // Incomplete varint
        assert_eq!(
            decode_player_action(&truncated_varint),
            Err(Error::Malformed)
        );

        // 2. Invalid face > 5
        let mut bad_face = missing_face.clone();
        bad_face.push(6); // Invalid face
        put_varint(1, &mut bad_face);
        assert_eq!(decode_player_action(&bad_face), Err(Error::Malformed));
        bad_face[9] = 255;
        assert_eq!(decode_player_action(&bad_face), Err(Error::Malformed));

        // 3. Invalid status
        let mut bad_status = Vec::new();
        put_varint(7, &mut bad_status); // Status 7 is invalid (only 0..=6)
        bad_status.extend_from_slice(&0i64.to_be_bytes());
        bad_status.push(0);
        put_varint(1, &mut bad_status);
        assert_eq!(decode_player_action(&bad_status), Err(Error::Malformed));
        let mut bad_status2 = Vec::new();
        put_varint(100, &mut bad_status2);
        bad_status2.extend_from_slice(&0i64.to_be_bytes());
        bad_status2.push(0);
        put_varint(1, &mut bad_status2);
        assert_eq!(decode_player_action(&bad_status2), Err(Error::Malformed));

        // 4. Invalid coordinate range
        for (bx, by, bz) in [
            (0, -65, 0),
            (0, 320, 0),
            (30_000_001, 0, 0),
            (-30_000_001, 0, 0),
            (0, 0, 30_000_001),
            (0, 0, -30_000_001),
        ] {
            let mut bad_coord = vec![0];
            let packed = pack_block_pos(bx, by, bz);
            bad_coord.extend_from_slice(&packed.to_be_bytes());
            bad_coord.push(0);
            put_varint(1, &mut bad_coord);
            assert_eq!(
                decode_player_action(&bad_coord),
                Err(Error::Malformed),
                "coord ({bx}, {by}, {bz}) should be rejected"
            );
        }

        // 5. Surplus / trailing bytes
        let mut surplus = encode_player_action(&PlayerAction {
            status: PlayerActionStatus::StartedDigging,
            pos: (0, 0, 0),
            face: 0,
            sequence: 1,
        });
        surplus.push(0xff);
        assert_eq!(decode_player_action(&surplus), Err(Error::Malformed));

        // --- UseItemOn malformed tests ---
        // 1. Truncated bytes
        assert_eq!(decode_use_item_on(&[]), Err(Error::Malformed));
        assert_eq!(decode_use_item_on(&[0]), Err(Error::Malformed)); // Hand only
        let mut u_partial = vec![0];
        u_partial.extend_from_slice(&[0; 4]); // Partial pos
        assert_eq!(decode_use_item_on(&u_partial), Err(Error::Malformed));
        let mut u_missing_face = vec![0];
        u_missing_face.extend_from_slice(&0i64.to_be_bytes());
        assert_eq!(decode_use_item_on(&u_missing_face), Err(Error::Malformed));
        let mut u_missing_cursor = u_missing_face.clone();
        put_varint(1, &mut u_missing_cursor); // Face present
        assert_eq!(decode_use_item_on(&u_missing_cursor), Err(Error::Malformed));
        u_missing_cursor.extend_from_slice(&0.5f32.to_be_bytes()); // Only 4 bytes cursor
        assert_eq!(decode_use_item_on(&u_missing_cursor), Err(Error::Malformed));
        let mut u_missing_inside = u_missing_face.clone();
        put_varint(1, &mut u_missing_inside);
        u_missing_inside.extend_from_slice(&0.5f32.to_be_bytes());
        u_missing_inside.extend_from_slice(&0.5f32.to_be_bytes());
        u_missing_inside.extend_from_slice(&0.5f32.to_be_bytes());
        assert_eq!(decode_use_item_on(&u_missing_inside), Err(Error::Malformed));
        let mut u_missing_seq = u_missing_inside.clone();
        u_missing_seq.push(0); // inside_block present, missing sequence
        assert_eq!(decode_use_item_on(&u_missing_seq), Err(Error::Malformed));

        // 2. Invalid hand > 1
        let mut bad_hand = Vec::new();
        put_varint(2, &mut bad_hand);
        bad_hand.extend_from_slice(&0i64.to_be_bytes());
        put_varint(1, &mut bad_hand);
        bad_hand.extend_from_slice(&0.5f32.to_be_bytes());
        bad_hand.extend_from_slice(&0.5f32.to_be_bytes());
        bad_hand.extend_from_slice(&0.5f32.to_be_bytes());
        bad_hand.push(0);
        put_varint(1, &mut bad_hand);
        assert_eq!(decode_use_item_on(&bad_hand), Err(Error::Malformed));

        // 3. Invalid face > 5
        let mut u_bad_face = Vec::new();
        put_varint(0, &mut u_bad_face);
        u_bad_face.extend_from_slice(&0i64.to_be_bytes());
        put_varint(6, &mut u_bad_face); // Face 6 is invalid
        u_bad_face.extend_from_slice(&0.5f32.to_be_bytes());
        u_bad_face.extend_from_slice(&0.5f32.to_be_bytes());
        u_bad_face.extend_from_slice(&0.5f32.to_be_bytes());
        u_bad_face.push(0);
        put_varint(1, &mut u_bad_face);
        assert_eq!(decode_use_item_on(&u_bad_face), Err(Error::Malformed));

        // 4. NaN, infinity, and out-of-range cursor floats
        let test_bad_floats = [
            (f32::NAN, 0.5f32, 0.5f32),
            (0.5f32, f32::NAN, 0.5f32),
            (0.5f32, 0.5f32, f32::NAN),
            (f32::INFINITY, 0.5f32, 0.5f32),
            (0.5f32, f32::NEG_INFINITY, 0.5f32),
            (-0.01f32, 0.5f32, 0.5f32),
            (0.5f32, 1.01f32, 0.5f32),
            (0.5f32, 0.5f32, -1.0f32),
            (0.5f32, 0.5f32, 2.0f32),
        ];
        for (cx, cy, cz) in test_bad_floats {
            let mut bad_cursor = Vec::new();
            put_varint(0, &mut bad_cursor);
            bad_cursor.extend_from_slice(&0i64.to_be_bytes());
            put_varint(0, &mut bad_cursor);
            bad_cursor.extend_from_slice(&cx.to_be_bytes());
            bad_cursor.extend_from_slice(&cy.to_be_bytes());
            bad_cursor.extend_from_slice(&cz.to_be_bytes());
            bad_cursor.push(0);
            put_varint(1, &mut bad_cursor);
            assert_eq!(
                decode_use_item_on(&bad_cursor),
                Err(Error::Malformed),
                "cursor ({cx}, {cy}, {cz}) should be rejected"
            );
        }

        // 5. Invalid inside_block > 1
        let mut bad_inside = Vec::new();
        put_varint(0, &mut bad_inside);
        bad_inside.extend_from_slice(&0i64.to_be_bytes());
        put_varint(0, &mut bad_inside);
        bad_inside.extend_from_slice(&0.5f32.to_be_bytes());
        bad_inside.extend_from_slice(&0.5f32.to_be_bytes());
        bad_inside.extend_from_slice(&0.5f32.to_be_bytes());
        bad_inside.push(2); // Invalid inside_block
        put_varint(1, &mut bad_inside);
        assert_eq!(decode_use_item_on(&bad_inside), Err(Error::Malformed));

        // 6. Invalid coordinate range for UseItemOn
        for (bx, by, bz) in [(0, -65, 0), (0, 320, 0), (30_000_001, 0, 0)] {
            let mut bad_coord = Vec::new();
            put_varint(0, &mut bad_coord);
            let packed = pack_block_pos(bx, by, bz);
            bad_coord.extend_from_slice(&packed.to_be_bytes());
            put_varint(0, &mut bad_coord);
            bad_coord.extend_from_slice(&0.5f32.to_be_bytes());
            bad_coord.extend_from_slice(&0.5f32.to_be_bytes());
            bad_coord.extend_from_slice(&0.5f32.to_be_bytes());
            bad_coord.push(0);
            put_varint(1, &mut bad_coord);
            assert_eq!(decode_use_item_on(&bad_coord), Err(Error::Malformed));
        }

        // 7. Surplus bytes
        let mut u_surplus = encode_use_item_on(&UseItemOn {
            hand: 0,
            pos: (0, 0, 0),
            face: 0,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside_block: false,
            world_border_hit: false,
            sequence: 1,
        });
        u_surplus.push(0xaa);
        assert_eq!(decode_use_item_on(&u_surplus), Err(Error::Malformed));
    }

    #[test]
    fn test_acknowledge_block_change_round_trip() {
        let test_sequences = [0, 1, 42, 127, 128, 255, 1000, 65536, 1_000_000, 1 << 28];
        for seq in test_sequences {
            // Test framed packet round-trip
            let framed = acknowledge_block_change(seq);
            let decoded = decode_acknowledge_block_change(&framed).expect("decode framed ack");
            assert_eq!(decoded, seq);

            // Test unframed packet payload round-trip
            let unframed = acknowledge_block_change_packet(seq);
            assert_eq!(unframed[0], 0x04, "packet ID must be 0x04");
            let decoded_unframed =
                decode_acknowledge_block_change_packet(&unframed).expect("decode unframed ack");
            assert_eq!(decoded_unframed, seq);
        }

        // Exact byte boundary verification for sequence = 1 << 28:
        // unframed is [0x04, 0x80, 0x80, 0x80, 0x80, 0x01] (6 bytes total).
        // decode_acknowledge_block_change must reject this unframed payload rather than
        // misinterpreting 0x04 as frame length 5 and failing on packet ID.
        let large_seq_unframed = acknowledge_block_change_packet(1 << 28);
        assert_eq!(large_seq_unframed.len(), 6);
        assert_eq!(large_seq_unframed[0], 0x04);
        assert_eq!(
            decode_acknowledge_block_change_packet(&large_seq_unframed).unwrap(),
            1 << 28
        );
        assert_eq!(
            decode_acknowledge_block_change(&large_seq_unframed),
            Err(Error::Malformed)
        );

        // Test exact byte encoding for known sequences
        assert_eq!(acknowledge_block_change_packet(0), vec![0x04, 0x00]);
        assert_eq!(acknowledge_block_change_packet(1), vec![0x04, 0x01]);
        assert_eq!(acknowledge_block_change_packet(128), vec![0x04, 0x80, 0x01]);

        // Malformed AcknowledgeBlockChange
        assert_eq!(decode_acknowledge_block_change(&[]), Err(Error::Malformed));
        assert_eq!(
            decode_acknowledge_block_change_packet(&[]),
            Err(Error::Malformed)
        );
        assert_eq!(
            decode_acknowledge_block_change_packet(&[0x05, 0x01]),
            Err(Error::Malformed)
        ); // Wrong ID (block_destruction)
        assert_eq!(
            decode_acknowledge_block_change_packet(&[0x04, 0x80]),
            Err(Error::Malformed)
        ); // Truncated varint
        assert_eq!(
            decode_acknowledge_block_change_packet(&[0x04, 0x01, 0x99]),
            Err(Error::Malformed)
        ); // Trailing byte
    }

    fn test_manifest() -> Arc<RegistryManifest> {
        Arc::new(RegistryManifest {
            tags: Vec::new(),
            registries: vec![
                (
                    "minecraft:dimension_type".into(),
                    vec!["minecraft:overworld".into()],
                ),
                (
                    "minecraft:world_clock".into(),
                    vec!["minecraft:overworld".into()],
                ),
                (
                    "minecraft:worldgen/biome".into(),
                    crate::world::Biome::ALL
                        .iter()
                        .map(|b| b.identifier().into())
                        .collect(),
                ),
            ],
        })
    }

    fn play_session() -> Session {
        let mut s = enter_play_session();
        s.player_name = "Alice".to_string();
        s
    }

    #[test]
    fn chat_and_command_packets_in_play_state() {
        let mut s = play_session();

        // Unsigned chat with the mandatory acknowledgment tail, written by hand.
        let mut chat = vec![0x09, 0x05, b'H', b'e', b'l', b'l', b'o'];
        chat.extend([0; 16]); // Timestamp and salt.
        chat.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // No signature, empty tail.
        let replies = s.receive(&frame_bytes(&chat), 4096).unwrap();
        let expected: &[u8] = b"<Alice> Hello";
        let mut system_chat = vec![0x7C, 0x08, 0x00, expected.len() as u8];
        system_chat.extend(expected);
        system_chat.push(0x00);
        assert_eq!(replies, vec![frame_bytes(&system_chat)]);
        assert_eq!(s.take_broadcasts(), vec![frame_bytes(&system_chat)]);
        assert!(s.take_broadcasts().is_empty());

        // An offline client sends a string-only mutating command, and all
        // world-mutating commands (tp, gamemode, time set, give) are refused
        // with a red system chat message while the preview is read-only.
        for cmd in [
            "time set day",
            "tp 1 2 3",
            "gamemode creative",
            "give Alice diamond 1",
        ] {
            let mut body = vec![0x07];
            put_string(cmd, &mut body);
            let replies = s.receive(&frame_bytes(&body), 4096).unwrap();
            assert_eq!(replies.len(), 1);
            let reply = chat::SystemChatMessagePacket::decode_frame(&replies[0]).unwrap();
            assert_eq!(reply.content().color, Some(chat::ChatColor::Red), "{cmd}");
            assert!(reply.content().text.contains("read-only"), "{cmd}");
            assert!(s.take_broadcasts().is_empty());

            // The signed form reaches the same `crate::commands` dispatcher.
            let mut signed_mut = vec![0x08];
            put_string(cmd, &mut signed_mut);
            signed_mut.extend([0; 16]); // Timestamp and salt.
            signed_mut.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // No arguments, empty tail.
            let replies = s.receive(&frame_bytes(&signed_mut), 4096).unwrap();
            assert_eq!(replies.len(), 1);
            let reply = chat::SystemChatMessagePacket::decode_frame(&replies[0]).unwrap();
            assert_eq!(reply.content().color, Some(chat::ChatColor::Red), "{cmd}");
            assert!(reply.content().text.contains("read-only"), "{cmd}");
        }

        // Read-only commands (/help and /seed) succeed through `crate::commands`.
        let mut signed = vec![0x08, 0x04, b'h', b'e', b'l', b'p'];
        signed.extend([0; 16]);
        signed.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // No arguments, empty tail.
        let replies = s.receive(&frame_bytes(&signed), 4096).unwrap();
        let reply = chat::SystemChatMessagePacket::decode_frame(&replies[0]).unwrap();
        assert!(reply.content().text.contains("/help"));

        let seed_cmd = [0x07, 0x04, b's', b'e', b'e', b'd'];
        let replies = s.receive(&frame_bytes(&seed_cmd), 4096).unwrap();
        let reply = chat::SystemChatMessagePacket::decode_frame(&replies[0]).unwrap();
        assert_eq!(reply.content().text, "Seed: [0]");

        // Acknowledgments and session announcements have no reply.
        assert!(
            s.receive(&frame_bytes(&[0x06, 0x00]), 4096)
                .unwrap()
                .is_empty()
        );
        assert!(
            s.receive(&frame_bytes(&[0x0A, 0x00]), 4096)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            s.receive(&frame_bytes(&[0x06, 0x00, 0x00]), 4096),
            Err(Error::Malformed)
        );
    }

    fn frame_bytes(packet: &[u8]) -> Vec<u8> {
        let mut out = vec![packet.len() as u8];
        assert!(packet.len() < 0x80);
        out.extend(packet);
        out
    }

    #[test]
    fn queued_broadcasts_are_bounded() {
        let mut s = play_session();
        let mut chat = vec![0x09, 0x01, b'x'];
        chat.extend([0; 22]);
        let chat = frame_bytes(&chat);
        for _ in 0..MAX_QUEUED_BROADCASTS {
            s.receive(&chat, 1 << 20).unwrap();
        }
        assert_eq!(s.receive(&chat, 1 << 20), Err(Error::Oversized));
        assert_eq!(s.take_broadcasts().len(), MAX_QUEUED_BROADCASTS);
    }

    fn enter_play_session() -> Session {
        let mut current = handshake(777);
        *current.last_mut().unwrap() = 2;
        let mut s = Session::new(true).with_registry_manifest(test_manifest());
        s.receive(&current, 4096).unwrap();
        assert_eq!(s.state(), State::Login);

        let mut login = vec![4];
        login.extend_from_slice(b"Test");
        login.extend_from_slice(&[7; 16]);
        s.receive(&frame(0, &login), 4096).unwrap();
        assert_eq!(s.state(), State::LoginAcknowledgement);

        s.receive(&frame(3, &[]), 4096).unwrap();
        assert_eq!(s.state(), State::Configuration);

        let mut known = vec![1];
        for text in ["minecraft", "core", VERSION] {
            put_string(text, &mut known);
        }
        s.receive(&frame(7, &known), 4096).unwrap();
        assert_eq!(s.state(), State::ConfigurationData);

        s.receive(&frame(3, &[]), 4096).unwrap();
        assert_eq!(s.state(), State::Play);

        s
    }

    fn teleport_confirm(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
        let mut body = vec![1u8];
        body.extend(x.to_be_bytes());
        body.extend(y.to_be_bytes());
        body.extend(z.to_be_bytes());
        body.extend(yaw.to_be_bytes());
        body.extend(pitch.to_be_bytes());
        body
    }

    fn player_position(x: f64, y: f64, z: f64, on_ground: u8) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend(x.to_be_bytes());
        body.extend(y.to_be_bytes());
        body.extend(z.to_be_bytes());
        body.push(on_ground);
        body
    }

    fn player_pos_rot(x: f64, y: f64, z: f64, yaw: f32, pitch: f32, on_ground: u8) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend(x.to_be_bytes());
        body.extend(y.to_be_bytes());
        body.extend(z.to_be_bytes());
        body.extend(yaw.to_be_bytes());
        body.extend(pitch.to_be_bytes());
        body.push(on_ground);
        body
    }

    fn player_rotation(yaw: f32, pitch: f32, on_ground: u8) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend(yaw.to_be_bytes());
        body.extend(pitch.to_be_bytes());
        body.push(on_ground);
        body
    }

    #[test]
    fn play_state_packet_validation_guards() {
        // 1. NaN coordinate guards
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(f64::NAN, 64.0, 0.0, 0.0, 0.0)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(0.0, f64::NAN, 0.0, 0.0, 0.0)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(0.0, 64.0, f64::NAN, 0.0, 0.0)),
                4096
            ),
            Err(Error::Malformed)
        );

        assert_eq!(
            enter_play_session()
                .receive(&frame(30, &player_position(f64::NAN, 64.0, 0.0, 1)), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(30, &player_position(0.0, f64::NAN, 0.0, 1)), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session()
                .receive(&frame(30, &player_position(0.0, 64.0, f64::NAN, 1)), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(31, &player_pos_rot(f64::NAN, 64.0, 0.0, 0.0, 0.0, 1)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(31, &player_pos_rot(0.0, f64::NAN, 0.0, 0.0, 0.0, 1)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(31, &player_pos_rot(0.0, 64.0, f64::NAN, 0.0, 0.0, 1)),
                4096
            ),
            Err(Error::Malformed)
        );

        // 2. Non-finite pitch guards
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, f32::NAN)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, f32::INFINITY)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, f32::NEG_INFINITY)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(31, &player_pos_rot(0.0, 64.0, 0.0, 0.0, f32::NAN, 1)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(31, &player_pos_rot(0.0, 64.0, 0.0, 0.0, f32::INFINITY, 1)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(32, &player_rotation(0.0, f32::NAN, 1)), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(32, &player_rotation(0.0, f32::INFINITY, 1)), 4096),
            Err(Error::Malformed)
        );

        // 3. Pitch > 90.0 (and < -90.0) guards
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, 90.001)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, -90.001)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(31, &player_pos_rot(0.0, 64.0, 0.0, 0.0, 91.0, 1)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(31, &player_pos_rot(0.0, 64.0, 0.0, 0.0, -91.0, 1)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(32, &player_rotation(0.0, 90.5, 1)), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(32, &player_rotation(0.0, -90.5, 1)), 4096),
            Err(Error::Malformed)
        );

        // 4. On-ground and horizontal-collision flag bit validation (0..=3 valid)
        for flag in [0, 1, 2, 3] {
            let mut s = enter_play_session();
            s.receive(&frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, 0.0)), 4096)
                .unwrap();
            assert!(
                s.receive(&frame(30, &player_position(0.0, 64.0, 0.0, flag)), 4096)
                    .is_ok()
            );
            assert!(
                s.receive(
                    &frame(31, &player_pos_rot(0.0, 64.0, 0.0, 0.0, 0.0, flag)),
                    4096
                )
                .is_ok()
            );
            assert!(
                s.receive(&frame(32, &player_rotation(0.0, 0.0, flag)), 4096)
                    .is_ok()
            );
            assert!(s.receive(&frame(33, &[flag]), 4096).is_ok());
        }

        // Invalid flags (> 3, unknown bits set) are rejected as malformed
        for invalid_flag in [4, 5, 8, 255] {
            assert_eq!(
                enter_play_session().receive(
                    &frame(30, &player_position(0.0, 64.0, 0.0, invalid_flag)),
                    4096
                ),
                Err(Error::Malformed)
            );
            assert_eq!(
                enter_play_session().receive(
                    &frame(31, &player_pos_rot(0.0, 64.0, 0.0, 0.0, 0.0, invalid_flag)),
                    4096
                ),
                Err(Error::Malformed)
            );
            assert_eq!(
                enter_play_session()
                    .receive(&frame(32, &player_rotation(0.0, 0.0, invalid_flag)), 4096),
                Err(Error::Malformed)
            );
            assert_eq!(
                enter_play_session().receive(&frame(33, &[invalid_flag]), 4096),
                Err(Error::Malformed)
            );
        }

        // player_abilities carries only the flying flag; player_input uses seven flags.
        for flags in [0x01, 0x04, 0x80, 0xff] {
            assert_eq!(
                enter_play_session().receive(&frame(40, &[flags]), 4096),
                Err(Error::Malformed)
            );
        }
        assert_eq!(
            enter_play_session().receive(&frame(43, &[0x80]), 4096),
            Err(Error::Malformed)
        );

        // 5. Trailing payload byte guards
        let mut p0 = teleport_confirm(0.0, 64.0, 0.0, 0.0, 0.0);
        p0.push(0);
        assert_eq!(
            enter_play_session().receive(&frame(0, &p0), 4096),
            Err(Error::Malformed)
        );

        let mut p11 = 1.0f32.to_be_bytes().to_vec();
        p11.push(0);
        assert_eq!(
            enter_play_session().receive(&frame(11, &p11), 4096),
            Err(Error::Malformed)
        );

        assert_eq!(
            enter_play_session().receive(&frame(13, &[0]), 4096),
            Err(Error::Malformed)
        );

        let mut p28 = 1u64.to_be_bytes().to_vec();
        p28.push(0);
        assert_eq!(
            enter_play_session().receive(&frame(28, &p28), 4096),
            Err(Error::Malformed)
        );

        let mut p30 = player_position(0.0, 64.0, 0.0, 1);
        p30.push(0);
        assert_eq!(
            enter_play_session().receive(&frame(30, &p30), 4096),
            Err(Error::Malformed)
        );

        let mut p31 = player_pos_rot(0.0, 64.0, 0.0, 0.0, 0.0, 1);
        p31.push(0);
        assert_eq!(
            enter_play_session().receive(&frame(31, &p31), 4096),
            Err(Error::Malformed)
        );

        let mut p32 = player_rotation(0.0, 0.0, 1);
        p32.push(0);
        assert_eq!(
            enter_play_session().receive(&frame(32, &p32), 4096),
            Err(Error::Malformed)
        );

        assert_eq!(
            enter_play_session().receive(&frame(33, &[1, 0]), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(40, &[0, 0]), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(43, &[1, 0]), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(44, &[0]), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(46, &[0]), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(54, &[0, 0, 0]), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(63, &[0, 0]), 4096),
            Err(Error::Malformed)
        );

        // 6. Extreme coordinate guards: vanilla only rejects positions beyond
        // +-30,000,000 horizontally and +-20,000,000 vertically.
        let teleport = |x, y, z| {
            enter_play_session().receive(&frame(0, &teleport_confirm(x, y, z, 0.0, 0.0)), 4096)
        };
        for (x, y, z) in [
            (30_000_000.5, 64.0, 0.0),
            (-30_000_000.5, 64.0, 0.0),
            (0.0, 20_000_000.5, 0.0),
            (0.0, -20_000_000.5, 0.0),
            (0.0, 64.0, 30_000_000.5),
        ] {
            assert_eq!(teleport(x, y, z), Err(Error::Malformed), "{x} {y} {z}");
        }
        assert!(teleport(5_000_000.0, 1000.0, -5_000_000.0).is_ok());
        assert_eq!(
            enter_play_session().receive(
                &frame(30, &player_position(30_000_001.0, 64.0, 0.0, 1)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(31, &player_pos_rot(0.0, 64.0, -30_000_001.0, 0.0, 0.0, 1)),
                4096
            ),
            Err(Error::Malformed)
        );

        // 7. Invalid hotbar slot guards
        assert_eq!(
            enter_play_session().receive(&frame(54, &9i16.to_be_bytes()), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(54, &(-1i16).to_be_bytes()), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(54, &100i16.to_be_bytes()), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(54, &(-10i16).to_be_bytes()), 4096),
            Err(Error::Malformed)
        );

        // Valid hotbar slots 0 and 8 succeed
        assert!(
            enter_play_session()
                .receive(&frame(54, &0i16.to_be_bytes()), 4096)
                .is_ok()
        );
        assert!(
            enter_play_session()
                .receive(&frame(54, &8i16.to_be_bytes()), 4096)
                .is_ok()
        );

        // 8. Batch rate guards
        let mut s_batch = enter_play_session();
        s_batch.preview.as_mut().unwrap().awaiting_batch = true;
        assert_eq!(
            s_batch.receive(&frame(11, &0.0f32.to_be_bytes()), 4096),
            Err(Error::Malformed)
        );
        let mut s_batch = enter_play_session();
        s_batch.preview.as_mut().unwrap().awaiting_batch = true;
        assert_eq!(
            s_batch.receive(&frame(11, &(-1.0f32).to_be_bytes()), 4096),
            Err(Error::Malformed)
        );
        // A fast client may report a high desired rate; it is not malformed.
        let mut s_batch = enter_play_session();
        s_batch.preview.as_mut().unwrap().awaiting_batch = true;
        assert!(
            s_batch
                .receive(&frame(11, &1000.0f32.to_be_bytes()), 4096)
                .is_ok()
        );
        let mut s_batch = enter_play_session();
        s_batch.preview.as_mut().unwrap().awaiting_batch = true;
        assert_eq!(
            s_batch.receive(&frame(11, &f32::NAN.to_be_bytes()), 4096),
            Err(Error::Malformed)
        );
        let mut s_batch = enter_play_session();
        s_batch.preview.as_mut().unwrap().awaiting_batch = true;
        assert!(
            s_batch
                .receive(&frame(11, &50.0f32.to_be_bytes()), 4096)
                .is_ok()
        );

        // 9. Keep alive guards
        let mut s_keepalive = enter_play_session();
        s_keepalive.preview.as_mut().unwrap().pending_keepalive = Some(12345);
        assert_eq!(
            s_keepalive.receive(&frame(28, &99999u64.to_be_bytes()), 4096),
            Err(Error::Malformed)
        );
        assert!(
            s_keepalive
                .receive(&frame(28, &12345u64.to_be_bytes()), 4096)
                .is_ok()
        );

        // 10. Unsupported unknown packet in Play state returns WrongState
        assert_eq!(
            enter_play_session().receive(&frame(99, &[]), 4096),
            Err(Error::WrongState)
        );

        // 11. Valid play packets flow smoothly
        let mut s = enter_play_session();
        s.receive(&frame(0, &teleport_confirm(0.0, 64.0, 0.0, 0.0, 0.0)), 4096)
            .unwrap();
        assert!(s.preview.as_ref().unwrap().teleport_acknowledged);

        assert!(
            s.receive(&frame(30, &player_position(10.0, 64.0, 10.0, 1)), 4096)
                .is_ok()
        );
        assert!(
            s.receive(
                &frame(31, &player_pos_rot(15.0, 64.0, 15.0, 90.0, 45.0, 0)),
                4096
            )
            .is_ok()
        );
        assert!(
            s.receive(&frame(32, &player_rotation(180.0, -45.0, 1)), 4096)
                .is_ok()
        );
        assert!(s.receive(&frame(33, &[1]), 4096).is_ok());
        // Start and stop flying (abilities flag 0x02).
        assert!(s.receive(&frame(40, &[0x02]), 4096).is_ok());
        assert!(s.receive(&frame(40, &[0]), 4096).is_ok());
        // Forward, backward, left, right, jump, shift, and sprint together.
        assert!(s.receive(&frame(43, &[0x7f]), 4096).is_ok());
        assert!(s.receive(&frame(43, &[0x02]), 4096).is_ok());
        assert!(s.receive(&frame(44, &[]), 4096).is_ok());
        assert!(s.preview.as_ref().unwrap().client_loaded);
        assert!(s.receive(&frame(46, &[]), 4096).is_ok());
        assert!(s.receive(&frame(54, &4i16.to_be_bytes()), 4096).is_ok());
        assert!(s.receive(&frame(63, &[0]), 4096).is_ok());
    }

    #[test]
    fn test_session_play_state_action_packets() {
        let mut s = enter_play_session();
        // 1. Send valid PlayerAction (ID 0x29 / 41) -> should emit AcknowledgeBlockChange
        let action = PlayerAction {
            status: PlayerActionStatus::StartedDigging,
            pos: (10, 64, -20),
            face: 1,
            sequence: 12,
        };
        let responses = s
            .receive(
                &frame(PLAYER_ACTION_ID, &encode_player_action(&action)),
                4096,
            )
            .unwrap();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], acknowledge_block_change(12));

        // 2. Send valid UseItemOn (ID 0x42 / 66) -> should emit AcknowledgeBlockChange
        let use_item = UseItemOn {
            hand: 0,
            pos: (10, 64, -20),
            face: 1,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside_block: false,
            world_border_hit: false,
            sequence: 15,
        };
        let responses = s
            .receive(&frame(USE_ITEM_ON_ID, &encode_use_item_on(&use_item)), 4096)
            .unwrap();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], acknowledge_block_change(15));

        // 3. Send malformed PlayerAction in Play state -> Error::Malformed
        let mut malformed_action = encode_player_action(&action);
        malformed_action.push(0xff); // Trailing byte
        assert_eq!(
            s.receive(&frame(PLAYER_ACTION_ID, &malformed_action), 4096),
            Err(Error::Malformed)
        );

        // 4. Send malformed UseItemOn in Play state -> Error::Malformed
        let mut bad_cursor_use = Vec::new();
        put_varint(0, &mut bad_cursor_use);
        bad_cursor_use.extend_from_slice(&pack_block_pos(0, 0, 0).to_be_bytes());
        put_varint(0, &mut bad_cursor_use);
        bad_cursor_use.extend_from_slice(&f32::NAN.to_be_bytes()); // NaN cursor float
        bad_cursor_use.extend_from_slice(&0.5f32.to_be_bytes());
        bad_cursor_use.extend_from_slice(&0.5f32.to_be_bytes());
        bad_cursor_use.push(0);
        put_varint(1, &mut bad_cursor_use);
        assert_eq!(
            s.receive(&frame(USE_ITEM_ON_ID, &bad_cursor_use), 4096),
            Err(Error::Malformed)
        );
    }

    #[test]
    fn truncated_player_action_is_malformed() {
        let valid = encode_player_action(&PlayerAction {
            status: PlayerActionStatus::StartedDigging,
            pos: (10, 64, -20),
            face: 1,
            sequence: 7,
        });
        for len in 0..valid.len() {
            assert_eq!(decode_player_action(&valid[..len]), Err(Error::Malformed));
            assert_eq!(
                enter_play_session().receive(
                    &frame(play::serverbound::PLAYER_ACTION, &valid[..len]),
                    4096
                ),
                Err(Error::Malformed)
            );
        }
    }

    #[test]
    fn truncated_use_item_on_is_malformed() {
        let valid = encode_use_item_on(&UseItemOn {
            hand: 0,
            pos: (10, 64, -20),
            face: 1,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside_block: false,
            world_border_hit: false,
            sequence: 7,
        });
        for len in 0..valid.len() {
            assert_eq!(decode_use_item_on(&valid[..len]), Err(Error::Malformed));
            assert_eq!(
                enter_play_session()
                    .receive(&frame(play::serverbound::USE_ITEM_ON, &valid[..len]), 4096),
                Err(Error::Malformed)
            );
        }
    }

    #[test]
    fn truncated_accept_teleportation_is_malformed() {
        let valid = teleport_confirm(0.0, 64.0, 0.0, 0.0, 0.0);
        for len in 0..valid.len() {
            assert_eq!(
                enter_play_session().receive(
                    &frame(play::serverbound::ACCEPT_TELEPORTATION, &valid[..len]),
                    4096
                ),
                Err(Error::Malformed)
            );
        }
    }

    #[test]
    fn truncated_chunk_batch_received_is_malformed() {
        let valid = 20.0f32.to_be_bytes();
        for len in 0..valid.len() {
            let mut s = enter_play_session();
            s.preview.as_mut().unwrap().awaiting_batch = true;
            assert_eq!(
                s.receive(
                    &frame(play::serverbound::CHUNK_BATCH_RECEIVED, &valid[..len]),
                    4096
                ),
                Err(Error::Malformed)
            );
        }
    }

    #[test]
    fn truncated_keep_alive_is_malformed() {
        let valid = 12345u64.to_be_bytes();
        for len in 0..valid.len() {
            let mut s = enter_play_session();
            s.preview.as_mut().unwrap().pending_keepalive = Some(12345);
            assert_eq!(
                s.receive(&frame(play::serverbound::KEEP_ALIVE, &valid[..len]), 4096),
                Err(Error::Malformed)
            );
        }
    }

    #[test]
    fn truncated_move_player_pos_is_malformed() {
        let valid = player_position(0.0, 64.0, 0.0, 1);
        for len in 0..valid.len() {
            let mut s = enter_play_session();
            s.preview.as_mut().unwrap().teleport_acknowledged = true;
            assert_eq!(
                s.receive(
                    &frame(play::serverbound::MOVE_PLAYER_POS, &valid[..len]),
                    4096
                ),
                Err(Error::Malformed)
            );
        }
    }

    #[test]
    fn truncated_move_player_pos_rot_is_malformed() {
        let valid = player_pos_rot(0.0, 64.0, 0.0, 0.0, 0.0, 1);
        for len in 0..valid.len() {
            let mut s = enter_play_session();
            s.preview.as_mut().unwrap().teleport_acknowledged = true;
            assert_eq!(
                s.receive(
                    &frame(play::serverbound::MOVE_PLAYER_POS_ROT, &valid[..len]),
                    4096
                ),
                Err(Error::Malformed)
            );
        }
    }

    #[test]
    fn truncated_move_player_rot_is_malformed() {
        let valid = player_rotation(0.0, 0.0, 1);
        for len in 0..valid.len() {
            assert_eq!(
                enter_play_session().receive(
                    &frame(play::serverbound::MOVE_PLAYER_ROT, &valid[..len]),
                    4096
                ),
                Err(Error::Malformed)
            );
        }
    }

    #[test]
    fn truncated_set_carried_item_is_malformed() {
        let valid = 4i16.to_be_bytes();
        for len in 0..valid.len() {
            assert_eq!(
                enter_play_session().receive(
                    &frame(play::serverbound::SET_CARRIED_ITEM, &valid[..len]),
                    4096
                ),
                Err(Error::Malformed)
            );
        }
    }

    fn test_protocol_ids() -> Arc<ProtocolIds> {
        const SYNTHETIC_REGISTRIES: &str = r#"{
            "minecraft:item": {
                "entries": {
                    "minecraft:air": { "protocol_id": 0 },
                    "minecraft:stone": { "protocol_id": 1 },
                    "minecraft:cobblestone": { "protocol_id": 14 },
                    "minecraft:diamond_sword": { "protocol_id": 851 }
                }
            },
            "minecraft:data_component_type": {
                "entries": {
                    "minecraft:damage": { "protocol_id": 3 },
                    "minecraft:custom_name": { "protocol_id": 6 },
                    "minecraft:enchantments": { "protocol_id": 13 }
                }
            },
            "minecraft:potion": {
                "entries": {
                    "minecraft:water": { "protocol_id": 0 }
                }
            }
        }"#;
        Arc::new(
            ProtocolIds::from_json_str(
                std::path::Path::new("reports/registries.json"),
                SYNTHETIC_REGISTRIES,
            )
            .expect("synthetic protocol ids"),
        )
    }

    fn test_manifest_with_enchantments() -> Arc<RegistryManifest> {
        let mut manifest = (*test_manifest()).clone();
        manifest.registries.push((
            "minecraft:enchantment".into(),
            vec!["minecraft:sharpness".into(), "minecraft:unbreaking".into()],
        ));
        Arc::new(manifest)
    }

    fn enter_play_session_with_protocol_ids() -> (Session, Arc<ProtocolIds>, DynamicRegistryIds) {
        let ids = test_protocol_ids();
        let manifest = test_manifest_with_enchantments();
        let dynamic_ids = DynamicRegistryIds::from_manifest(&manifest);

        let mut current = handshake(777);
        *current.last_mut().unwrap() = 2;
        let mut s = Session::new(true)
            .with_registry_manifest(manifest)
            .with_protocol_ids(Arc::clone(&ids));
        s.receive(&current, 4096).unwrap();
        let mut login = vec![4];
        login.extend_from_slice(b"Test");
        login.extend_from_slice(&[7; 16]);
        s.receive(&frame(0, &login), 4096).unwrap();
        s.receive(&frame(3, &[]), 4096).unwrap();
        let mut known = vec![1];
        for text in ["minecraft", "core", VERSION] {
            put_string(text, &mut known);
        }
        s.receive(&frame(7, &known), 4096).unwrap();
        s.receive(&frame(3, &[]), 4096).unwrap();
        assert_eq!(s.state(), State::Play);
        (s, ids, dynamic_ids)
    }

    fn creative_slot_frame(
        window_slot: i16,
        stack: &Option<crate::item::ItemStack>,
        ids: &ProtocolIds,
        dynamic_ids: &DynamicRegistryIds,
    ) -> Vec<u8> {
        let mut body = window_slot.to_be_bytes().to_vec();
        slot::encode_slot(stack, ids, dynamic_ids, &mut body).expect("encode creative slot");
        frame(play::serverbound::SET_CREATIVE_MODE_SLOT, &body)
    }

    #[test]
    fn creative_slot_stores_and_clears_stacks_and_tracks_carried_item() {
        use crate::item::{DEFAULT_MAX_STACK, ItemComponents, ItemStack};

        let (mut s, ids, dynamic_ids) = enter_play_session_with_protocol_ids();
        assert_eq!(s.inventory().held_slot(), 0);
        assert!(s.inventory().held_item().is_none());

        // Window slot 36 is hotbar slot 0 (`Java Edition protocol/Inventory` rev 3201379).
        let stone = ItemStack::try_new("minecraft:stone", 64, DEFAULT_MAX_STACK).unwrap();
        let replies = s
            .receive(
                &creative_slot_frame(36, &Some(stone.clone()), &ids, &dynamic_ids),
                4096,
            )
            .unwrap();
        assert!(replies.is_empty());
        assert_eq!(s.inventory().get_slot(0), Some(&stone));
        assert_eq!(s.inventory().get_window_slot(36), Some(&stone));
        assert_eq!(s.inventory().held_item(), Some(&stone));

        // Window slot 44 is hotbar slot 8; store an enchanted named sword with damage.
        let sword = ItemStack::try_new("minecraft:diamond_sword", 1, DEFAULT_MAX_STACK)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_damage(12)
                    .with_custom_name("Excalibur")
                    .with_enchantment("minecraft:sharpness", 5)
                    .unwrap(),
            )
            .unwrap();
        let replies = s
            .receive(
                &creative_slot_frame(44, &Some(sword.clone()), &ids, &dynamic_ids),
                4096,
            )
            .unwrap();
        assert!(replies.is_empty());
        assert_eq!(s.inventory().get_slot(8), Some(&sword));
        assert_eq!(s.inventory().get_window_slot(44), Some(&sword));

        // Window slot 9 is the first main inventory slot (`9..=35`).
        let cobble = ItemStack::try_new("minecraft:cobblestone", 32, DEFAULT_MAX_STACK).unwrap();
        let replies = s
            .receive(
                &creative_slot_frame(9, &Some(cobble.clone()), &ids, &dynamic_ids),
                4096,
            )
            .unwrap();
        assert!(replies.is_empty());
        assert_eq!(s.inventory().get_slot(9), Some(&cobble));
        assert_eq!(s.inventory().get_window_slot(9), Some(&cobble));

        // `SET_CARRIED_ITEM` selects hotbar slot 8 (window slot 44).
        let replies = s
            .receive(
                &frame(play::serverbound::SET_CARRIED_ITEM, &8i16.to_be_bytes()),
                4096,
            )
            .unwrap();
        assert!(replies.is_empty());
        assert_eq!(s.inventory().held_slot(), 8);
        assert_eq!(s.inventory().held_item(), Some(&sword));

        // Clearing hotbar slot 0 (window slot 36) and hotbar slot 8 (window slot 44) removes them.
        assert!(
            s.receive(&creative_slot_frame(36, &None, &ids, &dynamic_ids), 4096)
                .unwrap()
                .is_empty()
        );
        assert!(s.inventory().get_slot(0).is_none());
        assert!(
            s.receive(&creative_slot_frame(44, &None, &ids, &dynamic_ids), 4096)
                .unwrap()
                .is_empty()
        );
        assert!(s.inventory().get_slot(8).is_none());
        assert!(s.inventory().held_item().is_none());
        assert_eq!(s.inventory().get_slot(9), Some(&cobble));
    }

    #[test]
    fn creative_slot_accepts_drop_crafting_armor_and_offhand_slots_without_storing() {
        use crate::item::{DEFAULT_MAX_STACK, ItemStack};

        let (mut s, ids, dynamic_ids) = enter_play_session_with_protocol_ids();
        let stone = ItemStack::try_new("minecraft:stone", 1, DEFAULT_MAX_STACK).unwrap();
        // -1 drops the item; 0..=8 are crafting output/input and armor; 45 is the offhand.
        for slot in [-1, 0, 1, 4, 5, 8, 45] {
            let packet = creative_slot_frame(slot, &Some(stone.clone()), &ids, &dynamic_ids);
            assert!(
                s.receive(&packet, 4096).is_ok(),
                "window slot {slot} is valid creative input"
            );
        }
        assert_eq!(s.state(), State::Play);
        assert_eq!(s.inventory().total_item_count(), 0);
    }

    #[test]
    fn creative_slot_with_unmodelled_component_keeps_the_session_and_untracks_the_slot() {
        use crate::item::{DEFAULT_MAX_STACK, ItemStack};

        let (mut s, ids, dynamic_ids) = enter_play_session_with_protocol_ids();
        let stone = ItemStack::try_new("minecraft:stone", 1, DEFAULT_MAX_STACK).unwrap();
        s.receive(
            &creative_slot_frame(36, &Some(stone), &ids, &dynamic_ids),
            4096,
        )
        .unwrap();
        assert_eq!(s.inventory().total_item_count(), 1);

        // Component type 99 is not modelled (for example fireworks or banner patterns).
        let mut body = 36i16.to_be_bytes().to_vec();
        put_varint(1, &mut body); // count
        put_varint(1, &mut body); // minecraft:stone
        put_varint(1, &mut body); // one component to add
        put_varint(0, &mut body); // none to remove
        put_varint(99, &mut body); // unmodelled component type
        body.push(0);
        assert!(
            s.receive(
                &frame(play::serverbound::SET_CREATIVE_MODE_SLOT, &body),
                4096
            )
            .is_ok()
        );
        assert_eq!(s.state(), State::Play);
        assert!(s.inventory().get_window_slot(36).is_none());
    }

    #[test]
    fn creative_slot_rejects_out_of_range_slot_and_unknown_item_id() {
        use crate::item::{DEFAULT_MAX_STACK, ItemStack};

        let (mut s, ids, dynamic_ids) = enter_play_session_with_protocol_ids();
        let stone = ItemStack::try_new("minecraft:stone", 1, DEFAULT_MAX_STACK).unwrap();

        // Only numbers outside the player inventory window (`0..=45`) and the creative
        // drop slot (`-1`) are rejected.
        for bad_slot in [i16::MIN, -999, -2, 46, 100, i16::MAX] {
            let packet = creative_slot_frame(bad_slot, &Some(stone.clone()), &ids, &dynamic_ids);
            assert_eq!(
                s.receive(&packet, 4096),
                Err(Error::Malformed),
                "window slot {bad_slot} should be rejected"
            );
        }
        assert_eq!(s.inventory().total_item_count(), 0);

        // Unknown item protocol ID (999) in a valid window slot (36) is rejected as Malformed.
        let mut unknown_item = 36i16.to_be_bytes().to_vec();
        put_varint(1, &mut unknown_item); // count = 1
        put_varint(999, &mut unknown_item); // unknown item id
        put_varint(0, &mut unknown_item); // 0 components to add
        put_varint(0, &mut unknown_item); // 0 components to remove
        assert_eq!(
            s.receive(
                &frame(play::serverbound::SET_CREATIVE_MODE_SLOT, &unknown_item),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(s.inventory().total_item_count(), 0);

        // Truncated creative slot body and trailing bytes after the slot are rejected.
        assert_eq!(
            s.receive(
                &frame(play::serverbound::SET_CREATIVE_MODE_SLOT, &[0x00]),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            s.receive(
                &frame(
                    play::serverbound::SET_CREATIVE_MODE_SLOT,
                    &36i16.to_be_bytes()
                ),
                4096
            ),
            Err(Error::Malformed)
        );
        let mut trailing = 36i16.to_be_bytes().to_vec();
        trailing.push(0); // Empty slot
        trailing.push(0); // Trailing byte
        assert_eq!(
            s.receive(
                &frame(play::serverbound::SET_CREATIVE_MODE_SLOT, &trailing),
                4096
            ),
            Err(Error::Malformed)
        );
    }

    #[test]
    fn creative_slot_without_protocol_ids_stays_bounded_and_ignored() {
        let mut s = enter_play_session();
        assert_eq!(
            s.receive(&frame(play::serverbound::SET_CREATIVE_MODE_SLOT, &[]), 4096),
            Err(Error::Malformed)
        );
        for payload in [&[0u8][..], &[0, 36, 1, 99, 0, 0][..], &[0xff; 32][..]] {
            assert!(
                s.receive(
                    &frame(play::serverbound::SET_CREATIVE_MODE_SLOT, payload),
                    4096
                )
                .unwrap()
                .is_empty()
            );
        }
        assert_eq!(s.inventory().total_item_count(), 0);
    }
}
