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

fn is_valid_coord(x: f64, y: f64, z: f64) -> bool {
    x.is_finite()
        && y.is_finite()
        && z.is_finite()
        && (-1_000_000.0..=1_000_000.0).contains(&x)
        && (-64.0..=512.0).contains(&y)
        && (-1_000_000.0..=1_000_000.0).contains(&z)
}

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
                        0 => {
                            if body.len() != 33 || body[0] != 1 {
                                return Err(Error::Malformed);
                            }
                            let x = f64::from_be_bytes(body[1..9].try_into().unwrap());
                            let y = f64::from_be_bytes(body[9..17].try_into().unwrap());
                            let z = f64::from_be_bytes(body[17..25].try_into().unwrap());
                            let yaw = f32::from_be_bytes(body[25..29].try_into().unwrap());
                            let pitch = f32::from_be_bytes(body[29..33].try_into().unwrap());
                            if !is_valid_coord(x, y, z) || !is_valid_rotation(yaw, pitch) {
                                return Err(Error::Malformed);
                            }
                            preview.teleport_acknowledged = true;
                        }
                        11 => {
                            if body.len() != 4 {
                                return Err(Error::Malformed);
                            }
                            let rate = f32::from_be_bytes(body.try_into().unwrap());
                            if !preview.awaiting_batch
                                || !rate.is_finite()
                                || rate <= 0.0
                                || rate > 100.0
                            {
                                return Err(Error::Malformed);
                            }
                            preview.awaiting_batch = false;
                        }
                        13 => {
                            if !body.is_empty() {
                                return Err(Error::Malformed);
                            }
                        }
                        14 | 22 => {} // Bounded information/custom payloads have no effect.
                        28 => {
                            if body.len() != 8 {
                                return Err(Error::Malformed);
                            }
                            let id = u64::from_be_bytes(body.try_into().unwrap());
                            if preview.pending_keepalive != Some(id) {
                                return Err(Error::Malformed);
                            }
                            preview.pending_keepalive = None;
                        }
                        30 => {
                            if body.len() != 25 || (body[24] & !0x03) != 0 {
                                return Err(Error::Malformed);
                            }
                            let x = f64::from_be_bytes(body[0..8].try_into().unwrap());
                            let y = f64::from_be_bytes(body[8..16].try_into().unwrap());
                            let z = f64::from_be_bytes(body[16..24].try_into().unwrap());
                            if !is_valid_coord(x, y, z) {
                                return Err(Error::Malformed);
                            }
                            preview.move_to(x, y, z).map_err(|_| Error::Malformed)?;
                        }
                        31 => {
                            if body.len() != 33 || (body[32] & !0x03) != 0 {
                                return Err(Error::Malformed);
                            }
                            let x = f64::from_be_bytes(body[0..8].try_into().unwrap());
                            let y = f64::from_be_bytes(body[8..16].try_into().unwrap());
                            let z = f64::from_be_bytes(body[16..24].try_into().unwrap());
                            let yaw = f32::from_be_bytes(body[24..28].try_into().unwrap());
                            let pitch = f32::from_be_bytes(body[28..32].try_into().unwrap());
                            if !is_valid_coord(x, y, z) || !is_valid_rotation(yaw, pitch) {
                                return Err(Error::Malformed);
                            }
                            preview.move_to(x, y, z).map_err(|_| Error::Malformed)?;
                        }
                        32 => {
                            if body.len() != 9 || (body[8] & !0x03) != 0 {
                                return Err(Error::Malformed);
                            }
                            let yaw = f32::from_be_bytes(body[0..4].try_into().unwrap());
                            let pitch = f32::from_be_bytes(body[4..8].try_into().unwrap());
                            if !is_valid_rotation(yaw, pitch) {
                                return Err(Error::Malformed);
                            }
                        }
                        33 => {
                            if body.len() != 1 || (body[0] & !0x03) != 0 {
                                return Err(Error::Malformed);
                            }
                        }
                        40 | 43 => {
                            if body.len() != 1 || !matches!(body[0], 0 | 1) {
                                return Err(Error::Malformed);
                            }
                        }
                        41 | 57 | 66 | 67 => {
                            if body.is_empty() {
                                return Err(Error::Malformed);
                            }
                            // Creative inputs are bounded by the frame/session limits.
                            // The preview is immutable and does not apply them.
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
                        44 => {
                            if !body.is_empty() {
                                return Err(Error::Malformed);
                            }
                            preview.client_loaded = true;
                        }
                        46 => {
                            if !body.is_empty() {
                                return Err(Error::Malformed);
                            }
                            // Punch has no world effect in the preview.
                        }
                        54 => {
                            if body.len() != 2 {
                                return Err(Error::Malformed);
                            }
                            let slot = i16::from_be_bytes(body.try_into().unwrap());
                            if !(0..=8).contains(&slot) {
                                return Err(Error::Malformed);
                            }
                            // Selected hotbar slot is client-local here.
                        }
                        63 => {
                            if body != [0] {
                                return Err(Error::Malformed);
                            }
                            // Spectator action with no target.
                        }
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

        // Boolean-only packets (40, 43) reject any non-boolean byte (> 1)
        assert_eq!(
            enter_play_session().receive(&frame(40, &[2]), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(43, &[2]), 4096),
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

        // 6. Extreme coordinate guards
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(1_000_000.1, 64.0, 0.0, 0.0, 0.0)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(-1_000_000.1, 64.0, 0.0, 0.0, 0.0)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(0.0, -64.1, 0.0, 0.0, 0.0)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(0.0, 512.1, 0.0, 0.0, 0.0)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(0, &teleport_confirm(0.0, 64.0, 1_000_000.1, 0.0, 0.0)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(30, &player_position(1_000_001.0, 64.0, 0.0, 1)),
                4096
            ),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(30, &player_position(0.0, -65.0, 0.0, 1)), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(&frame(30, &player_position(0.0, 513.0, 0.0, 1)), 4096),
            Err(Error::Malformed)
        );
        assert_eq!(
            enter_play_session().receive(
                &frame(31, &player_pos_rot(0.0, 64.0, -1_000_001.0, 0.0, 0.0, 1)),
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
        let mut s_batch = enter_play_session();
        s_batch.preview.as_mut().unwrap().awaiting_batch = true;
        assert_eq!(
            s_batch.receive(&frame(11, &100.5f32.to_be_bytes()), 4096),
            Err(Error::Malformed)
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
        assert!(s.receive(&frame(40, &[0]), 4096).is_ok());
        assert!(s.receive(&frame(43, &[1]), 4096).is_ok());
        assert!(s.receive(&frame(44, &[]), 4096).is_ok());
        assert!(s.preview.as_ref().unwrap().client_loaded);
        assert!(s.receive(&frame(46, &[]), 4096).is_ok());
        assert!(s.receive(&frame(54, &4i16.to_be_bytes()), 4096).is_ok());
        assert!(s.receive(&frame(63, &[0]), 4096).is_ok());
    }
}
