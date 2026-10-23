//! Bounded Java Edition 26.3 status and optional local login experiment.

use crate::preview_data::RegistryManifest;
use std::sync::Arc;

pub const PROTOCOL: i32 = 777;
pub const VERSION: &str = "26.3";
pub const MAX_FRAME: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Handshake,
    Status,
    Ping,
    Login,
    LoginAcknowledgement,
    Configuration,
    ConfigurationData,
    Play,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Malformed,
    Oversized,
    WrongState,
}

fn varint(input: &[u8]) -> Result<Option<(u32, usize)>, Error> {
    let mut value = 0u32;
    for (i, byte) in input.iter().take(5).enumerate() {
        if i == 4 && byte & 0xf0 != 0 {
            return Err(Error::Malformed);
        }
        value |= u32::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            if i > 0 && *byte == 0 {
                return Err(Error::Malformed);
            }
            return Ok(Some((value, i + 1)));
        }
    }
    if input.len() >= 5 {
        Err(Error::Malformed)
    } else {
        Ok(None)
    }
}

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

pub(crate) fn frame(id: u32, body: &[u8]) -> Vec<u8> {
    let mut packet = Vec::new();
    put_varint(id, &mut packet);
    packet.extend_from_slice(body);
    let mut out = Vec::new();
    put_varint(packet.len() as u32, &mut out);
    out.extend(packet);
    out
}

pub(crate) fn put_string(value: &str, out: &mut Vec<u8>) {
    put_varint(value.len() as u32, out);
    out.extend_from_slice(value.as_bytes());
}

fn read_string(input: &[u8]) -> Result<(&str, &[u8]), Error> {
    let Some((length, prefix)) = varint(input)? else {
        return Err(Error::Malformed);
    };
    let length = length as usize;
    if length > 255 || input.len() < prefix + length {
        return Err(Error::Malformed);
    }
    let value =
        std::str::from_utf8(&input[prefix..prefix + length]).map_err(|_| Error::Malformed)?;
    Ok((value, &input[prefix + length..]))
}

fn known_packs() -> Vec<u8> {
    let mut body = Vec::new();
    put_varint(1, &mut body);
    for text in ["minecraft", "core", VERSION] {
        put_string(text, &mut body);
    }
    frame(15, &body)
}

fn registry_data(registry: &str, entries: &[String]) -> Vec<u8> {
    let mut body = Vec::new();
    put_string(registry, &mut body);
    put_varint(entries.len() as u32, &mut body);
    for entry in entries {
        put_string(entry, &mut body);
        body.push(0); // Entry definition comes from the negotiated core pack.
    }
    frame(7, &body)
}

fn initial_configuration(manifest: Option<&RegistryManifest>) -> Vec<Vec<u8>> {
    let mut features = Vec::new();
    put_varint(1, &mut features);
    put_string("minecraft:vanilla", &mut features);
    let mut packets = vec![frame(13, &features)];
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
    packets.push(frame(14, &tags));
    packets.push(frame(3, &[]));
    packets
}

fn recognizes_core_pack(body: &[u8]) -> Result<bool, Error> {
    let Some((count, prefix)) = varint(body)? else {
        return Err(Error::Malformed);
    };
    if count > 16 {
        return Err(Error::Oversized);
    }
    let mut remaining = &body[prefix..];
    let mut recognized = false;
    for _ in 0..count {
        let (namespace, rest) = read_string(remaining)?;
        let (id, rest) = read_string(rest)?;
        let (version, rest) = read_string(rest)?;
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
    frame(0, &body)
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
    let x = (packed >> 38) as i32;
    let y = (packed << 52 >> 52) as i32;
    let z = (packed << 26 >> 38) as i32;
    (x, y, z)
}

/// Packs (x, y, z) coordinates into a 64-bit Minecraft Java block position.
#[inline]
pub fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    ((x as i64 & 0x3FF_FFFF) << 38) | ((z as i64 & 0x3FF_FFFF) << 12) | (y as i64 & 0xFFF)
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

/// Serverbound PlayerAction (ID `0x24` / 36) packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerAction {
    pub status: PlayerActionStatus,
    pub pos: (i32, i32, i32),
    pub face: u8,
    pub sequence: u32,
}

/// Decodes a serverbound `PlayerAction` packet (ID `0x24` / 36).
pub fn decode_player_action(body: &[u8]) -> Result<PlayerAction, Error> {
    let Some((status_val, prefix)) = varint(body)? else {
        return Err(Error::Malformed);
    };
    let status = PlayerActionStatus::try_from(status_val)?;
    let mut rest = &body[prefix..];

    if rest.len() < 8 {
        return Err(Error::Malformed);
    }
    let packed_pos = i64::from_be_bytes(rest[..8].try_into().unwrap());
    let (x, y, z) = unpack_block_pos(packed_pos);
    if !is_valid_block_pos(x, y, z) {
        return Err(Error::Malformed);
    }
    rest = &rest[8..];

    if rest.is_empty() {
        return Err(Error::Malformed);
    }
    let face = rest[0];
    if face > 5 {
        return Err(Error::Malformed);
    }
    rest = &rest[1..];

    let Some((sequence, seq_len)) = varint(rest)? else {
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

/// Serverbound UseItemOn (ID `0x38` / 56) packet.
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

/// Decodes a serverbound `UseItemOn` packet (ID `0x38` / 56).
pub fn decode_use_item_on(body: &[u8]) -> Result<UseItemOn, Error> {
    let Some((hand, prefix)) = varint(body)? else {
        return Err(Error::Malformed);
    };
    if hand > 1 {
        return Err(Error::Malformed);
    }
    let mut rest = &body[prefix..];

    if rest.len() < 8 {
        return Err(Error::Malformed);
    }
    let packed_pos = i64::from_be_bytes(rest[..8].try_into().unwrap());
    let (x, y, z) = unpack_block_pos(packed_pos);
    if !is_valid_block_pos(x, y, z) {
        return Err(Error::Malformed);
    }
    rest = &rest[8..];

    let Some((face, face_len)) = varint(rest)? else {
        return Err(Error::Malformed);
    };
    if face > 5 {
        return Err(Error::Malformed);
    }
    rest = &rest[face_len..];

    if rest.len() < 12 {
        return Err(Error::Malformed);
    }
    let cursor_x = f32::from_be_bytes(rest[0..4].try_into().unwrap());
    let cursor_y = f32::from_be_bytes(rest[4..8].try_into().unwrap());
    let cursor_z = f32::from_be_bytes(rest[8..12].try_into().unwrap());
    if !cursor_x.is_finite()
        || !cursor_y.is_finite()
        || !cursor_z.is_finite()
        || !(0.0..=1.0).contains(&cursor_x)
        || !(0.0..=1.0).contains(&cursor_y)
        || !(0.0..=1.0).contains(&cursor_z)
    {
        return Err(Error::Malformed);
    }
    rest = &rest[12..];

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
    let Some((sequence, seq_len)) = varint(rest)? else {
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

/// Emits a framed `AcknowledgeBlockChange` packet (ID `0x05`).
pub fn acknowledge_block_change(sequence: u32) -> Vec<u8> {
    let mut body = Vec::new();
    put_varint(sequence, &mut body);
    frame(0x05, &body)
}

/// Emits an unframed `AcknowledgeBlockChange` packet payload: `[0x05, varint(sequence)]`.
pub fn acknowledge_block_change_packet(sequence: u32) -> Vec<u8> {
    let mut packet = Vec::new();
    put_varint(0x05, &mut packet);
    put_varint(sequence, &mut packet);
    packet
}

/// Decodes an unframed `AcknowledgeBlockChange` packet payload `[0x05, varint(sequence)]`.
pub fn decode_acknowledge_block_change_packet(bytes: &[u8]) -> Result<u32, Error> {
    let Some((id, prefix)) = varint(bytes)? else {
        return Err(Error::Malformed);
    };
    if id != 0x05 {
        return Err(Error::Malformed);
    }
    let rest = &bytes[prefix..];
    let Some((sequence, seq_len)) = varint(rest)? else {
        return Err(Error::Malformed);
    };
    if seq_len != rest.len() {
        return Err(Error::Malformed);
    }
    Ok(sequence)
}

/// Decodes a framed `AcknowledgeBlockChange` packet: `[varint(length), 0x05, varint(sequence)]`.
pub fn decode_acknowledge_block_change(bytes: &[u8]) -> Result<u32, Error> {
    let Some((len, prefix)) = varint(bytes)? else {
        return Err(Error::Malformed);
    };
    if prefix + len as usize != bytes.len() {
        return Err(Error::Malformed);
    }
    decode_acknowledge_block_change_packet(&bytes[prefix..])
}

pub struct Session {
    state: State,
    input: Vec<u8>,
    bytes: usize,
    local_preview: bool,
    session_id: [u8; 16],
    registry_manifest: Option<Arc<RegistryManifest>>,
    pub preview: Option<crate::java_preview::Preview>,
    preview_seed: u64,
    preview_radius: u8,
    preview_terrain: crate::world::Terrain,
    vanilla_source: Option<crate::java_preview::VanillaSource>,
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
            preview: None,
            preview_seed: 0,
            preview_radius: 2,
            preview_terrain: crate::world::Terrain::Preview,
            vanilla_source: None,
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
    pub fn state(&self) -> State {
        self.state
    }
    pub fn with_registry_manifest(mut self, manifest: Arc<RegistryManifest>) -> Self {
        self.registry_manifest = Some(manifest);
        self
    }
    pub fn receive(&mut self, bytes: &[u8], limit: usize) -> Result<Vec<Vec<u8>>, Error> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or(Error::Oversized)?;
        if self.bytes > limit || self.input.len().saturating_add(bytes.len()) > MAX_FRAME + 5 {
            return Err(Error::Oversized);
        }
        self.input.extend_from_slice(bytes);
        let mut responses = Vec::new();
        while let Some((size, prefix)) = varint(&self.input)? {
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
            let Some((id, id_len)) = varint(&packet)? else {
                return Err(Error::Malformed);
            };
            let body = &packet[id_len..];
            match (self.state, id) {
                (State::Handshake, 0) => {
                    let Some((version, n)) = varint(body)? else {
                        return Err(Error::Malformed);
                    };
                    let body = &body[n..];
                    let Some((host_len, n)) = varint(body)? else {
                        return Err(Error::Malformed);
                    };
                    let host_len = host_len as usize;
                    if host_len > 255 || body.len() < n + host_len + 2 {
                        return Err(Error::Malformed);
                    }
                    if std::str::from_utf8(&body[n..n + host_len]).is_err() {
                        return Err(Error::Malformed);
                    }
                    let rest = &body[n + host_len + 2..];
                    let Some((next, n)) = varint(rest)? else {
                        return Err(Error::Malformed);
                    };
                    if n != rest.len() {
                        return Err(Error::WrongState);
                    }
                    self.state = match next {
                        1 => State::Status,
                        2 if self.local_preview && version == PROTOCOL as u32 => State::Login,
                        _ => return Err(Error::WrongState),
                    };
                }
                (State::Status, 0) if body.is_empty() => {
                    responses.push(status(self.local_preview));
                    self.state = State::Ping;
                }
                (State::Ping, 1) if body.len() == 8 => {
                    responses.push(frame(1, body));
                    self.state = State::Done;
                }
                (State::Login, 0) => {
                    let Some((name_len, prefix)) = varint(body)? else {
                        return Err(Error::Malformed);
                    };
                    let name_len = name_len as usize;
                    if !(3..=16).contains(&name_len) || body.len() != prefix + name_len + 16 {
                        return Err(Error::Malformed);
                    }
                    let name = &body[prefix..prefix + name_len];
                    if !name
                        .iter()
                        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                    {
                        return Err(Error::Malformed);
                    }
                    // The client UUID is an opaque, unverified local-session value.
                    let mut success = body[prefix + name_len..].to_vec();
                    put_varint(name_len as u32, &mut success);
                    success.extend_from_slice(name);
                    put_varint(0, &mut success); // No profile properties or signed textures.
                    // 26.3 appends a separate connection-scoped session UUID. This is
                    // only a local identifier; it grants no authenticated identity.
                    success.extend_from_slice(&self.session_id);
                    responses.push(frame(2, &success));
                    self.state = State::LoginAcknowledgement;
                }
                (State::LoginAcknowledgement, 3) if body.is_empty() => {
                    self.state = State::Configuration;
                    responses.push(known_packs());
                }
                (State::Configuration, 0 | 2) => {} // No effect; bounded by the frame cap.
                (State::Configuration, 7) => {
                    if !recognizes_core_pack(body)? {
                        return Err(Error::WrongState);
                    }
                    self.state = State::ConfigurationData;
                    if self.registry_manifest.is_some() {
                        responses.extend(initial_configuration(self.registry_manifest.as_deref()));
                    }
                }
                (State::ConfigurationData, 3) if body.is_empty() => {
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
                    self.preview = Some(preview);
                    self.state = State::Play;
                }
                (State::Play, id) => {
                    let preview = self.preview.as_mut().ok_or(Error::WrongState)?;
                    match id {
                        0 if body.len() == 33 && body[0] == 1 => {
                            if !body[1..25]
                                .as_chunks::<8>()
                                .0
                                .iter()
                                .all(|v| f64::from_be_bytes(*v).is_finite())
                                || !body[25..]
                                    .as_chunks::<4>()
                                    .0
                                    .iter()
                                    .all(|v| f32::from_be_bytes(*v).is_finite())
                            {
                                return Err(Error::Malformed);
                            }
                            preview.teleport_acknowledged = true;
                        }
                        44 if body.is_empty() => preview.client_loaded = true,
                        13 if body.is_empty() => {}
                        30 | 31 if body.len() == (if id == 30 { 25 } else { 33 }) => {
                            let xyz: Vec<f64> = body[..24]
                                .as_chunks::<8>()
                                .0
                                .iter()
                                .map(|v| f64::from_be_bytes(*v))
                                .collect();
                            preview
                                .move_to(xyz[0], xyz[1], xyz[2])
                                .map_err(|_| Error::Malformed)?;
                        }
                        32 if body.len() == 9 => {}
                        33 | 40 | 43 if body.len() == 1 => {}
                        11 if body.len() == 4 => {
                            let rate = f32::from_be_bytes(body.try_into().unwrap());
                            if !preview.awaiting_batch || !rate.is_finite() || rate <= 0.0 {
                                return Err(Error::Malformed);
                            }
                            preview.awaiting_batch = false;
                        }
                        28 if body.len() == 8 => {
                            let id = u64::from_be_bytes(body.try_into().unwrap());
                            if preview.pending_keepalive != Some(id) {
                                return Err(Error::Malformed);
                            }
                            preview.pending_keepalive = None;
                        }
                        36 => {
                            let action = decode_player_action(body)?;
                            responses.push(acknowledge_block_change(action.sequence));
                        }
                        42 => {
                            let mut rest = body;
                            for _ in 0..3 {
                                let (_, n) = varint(rest)?.ok_or(Error::Malformed)?;
                                rest = &rest[n..];
                            }
                            if !rest.is_empty() {
                                return Err(Error::Malformed);
                            }
                            // Observer posture commands have no world effect.
                        }
                        46 if body.is_empty() => {} // Punch has no world effect in the preview.
                        56 => {
                            let action = decode_use_item_on(body)?;
                            responses.push(acknowledge_block_change(action.sequence));
                        }
                        63 if body == [0] => {} // Spectator action with no target.
                        41 | 57 | 66 | 67 if !body.is_empty() => {
                            // Creative inputs are bounded by the frame/session limits.
                            // The preview is immutable and does not apply them.
                        }
                        54 if body.len() == 2 => {} // Selected hotbar slot is client-local here.
                        14 | 22 => {} // Bounded information/custom payloads have no effect.
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
    #[test]
    fn login_is_rejected() {
        let mut h = handshake(777);
        *h.last_mut().unwrap() = 2;
        assert_eq!(Session::default().receive(&h, 4096), Err(Error::WrongState));
    }

    #[test]
    fn local_preview_login_is_version_gated_and_unverified() {
        let mut old = handshake(766);
        *old.last_mut().unwrap() = 2;
        assert_eq!(
            Session::new(true).receive(&old, 4096),
            Err(Error::WrongState)
        );
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
            assert_eq!(unframed[0], 0x05, "packet ID must be 0x05");
            let decoded_unframed =
                decode_acknowledge_block_change_packet(&unframed).expect("decode unframed ack");
            assert_eq!(decoded_unframed, seq);
        }

        // Exact byte boundary verification for sequence = 1 << 28:
        // unframed is [0x05, 0x80, 0x80, 0x80, 0x80, 0x01] (6 bytes total).
        // decode_acknowledge_block_change must reject this unframed payload rather than
        // misinterpreting 0x05 as frame length 5 and failing on packet ID.
        let large_seq_unframed = acknowledge_block_change_packet(1 << 28);
        assert_eq!(large_seq_unframed.len(), 6);
        assert_eq!(large_seq_unframed[0], 0x05);
        assert_eq!(
            decode_acknowledge_block_change_packet(&large_seq_unframed).unwrap(),
            1 << 28
        );
        assert_eq!(
            decode_acknowledge_block_change(&large_seq_unframed),
            Err(Error::Malformed)
        );

        // Test exact byte encoding for known sequences
        assert_eq!(acknowledge_block_change_packet(0), vec![0x05, 0x00]);
        assert_eq!(acknowledge_block_change_packet(1), vec![0x05, 0x01]);
        assert_eq!(acknowledge_block_change_packet(128), vec![0x05, 0x80, 0x01]);

        // Malformed AcknowledgeBlockChange
        assert_eq!(decode_acknowledge_block_change(&[]), Err(Error::Malformed));
        assert_eq!(
            decode_acknowledge_block_change_packet(&[]),
            Err(Error::Malformed)
        );
        assert_eq!(
            decode_acknowledge_block_change_packet(&[0x04, 0x01]),
            Err(Error::Malformed)
        ); // Wrong ID
        assert_eq!(
            decode_acknowledge_block_change_packet(&[0x05, 0x80]),
            Err(Error::Malformed)
        ); // Truncated varint
        assert_eq!(
            decode_acknowledge_block_change_packet(&[0x05, 0x01, 0x99]),
            Err(Error::Malformed)
        ); // Trailing byte
    }

    #[test]
    fn test_session_play_state_action_packets() {
        let manifest = Arc::new(RegistryManifest {
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
        });

        let mut s = Session::new(true).with_registry_manifest(manifest);
        let mut h = handshake(PROTOCOL as u32);
        *h.last_mut().unwrap() = 2; // Login state
        s.receive(&h, 4096).unwrap();

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

        // 1. Send valid PlayerAction (ID 0x24 / 36) -> should emit AcknowledgeBlockChange
        let action = PlayerAction {
            status: PlayerActionStatus::StartedDigging,
            pos: (10, 64, -20),
            face: 1,
            sequence: 12,
        };
        let responses = s
            .receive(&frame(0x24, &encode_player_action(&action)), 4096)
            .unwrap();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], acknowledge_block_change(12));

        // 2. Send valid UseItemOn (ID 0x38 / 56) -> should emit AcknowledgeBlockChange
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
            .receive(&frame(0x38, &encode_use_item_on(&use_item)), 4096)
            .unwrap();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], acknowledge_block_change(15));

        // 3. Send malformed PlayerAction in Play state -> Error::Malformed
        let mut malformed_action = encode_player_action(&action);
        malformed_action.push(0xff); // Trailing byte
        assert_eq!(
            s.receive(&frame(0x24, &malformed_action), 4096),
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
            s.receive(&frame(0x38, &bad_cursor_use), 4096),
            Err(Error::Malformed)
        );
    }
}
