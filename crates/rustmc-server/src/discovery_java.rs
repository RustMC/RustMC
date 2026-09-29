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

fn put_varint(mut value: u32, out: &mut Vec<u8>) {
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

fn frame(id: u32, body: &[u8]) -> Vec<u8> {
    let mut packet = Vec::new();
    put_varint(id, &mut packet);
    packet.extend_from_slice(body);
    let mut out = Vec::new();
    put_varint(packet.len() as u32, &mut out);
    out.extend(packet);
    out
}

fn put_string(value: &str, out: &mut Vec<u8>) {
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
        "RustMC local Java preview; world unavailable"
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

pub struct Session {
    state: State,
    input: Vec<u8>,
    bytes: usize,
    local_preview: bool,
    session_id: [u8; 16],
    registry_manifest: Option<Arc<RegistryManifest>>,
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
                    self.state = State::Play;
                }
                (State::Configuration | State::ConfigurationData | State::Play, _) => {
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
}
