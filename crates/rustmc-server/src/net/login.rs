//! The pre-play conversation of one connection: handshake, status, login
//! (offline or online), compression and configuration.
//!
//! Why: everything a client does before it joins the world is per-connection
//! and needs no shared world state, so it runs on the connection's own thread
//! as a sans-IO state machine: packets in, ordered actions out. The caller
//! applies the actions to its framing and socket in order.
//! Done: status with MOTD and player counts, offline UUIDs, online-mode
//! encryption and session-server verification, the compression switch, Login
//! Success with the 26.3 session id, configuration, refusal of outdated
//! clients and full servers. Not done: login plugin messages, cookies,
//! transfers (refused as unknown packets).
//!
//! Sources: minecraft.wiki `Java Edition protocol/Packets` (Handshaking,
//! Status, Login, Configuration) and `Java Edition protocol/Encryption`; ids
//! from the official 26.3 packet report via `java_packets`.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use super::configuration::{
    PackListError, initial_configuration, known_packs, recognizes_core_pack,
};
use super::crypto::{
    CryptoError, SHARED_SECRET_BYTES, ServerKey, VERIFY_TOKEN_BYTES, random_bytes, server_hash,
};
use super::framing::Packet;
use super::identity::{GameProfile, is_valid_name};
use super::session_server::{SessionVerifier, VerifyError};
use crate::discovery_java::{PROTOCOL, VERSION};
use crate::java_packets::{configuration, handshake, intent, login, status};
use crate::java_wire::{WireError, put_string, put_varint, read_string, read_varint};
use crate::preview_data::RegistryManifest;

/// Longest server address a handshake may carry (`String(255)`).
pub const MAX_HOST_BYTES: usize = 255;
/// Largest RSA-encrypted field accepted in Encryption Response. A 1024-bit
/// key yields 128 bytes; four times that leaves room for longer keys without
/// letting a client make the server decrypt large inputs.
pub const MAX_ENCRYPTED_FIELD: usize = 512;
/// Bytes of the status ping payload (`Long`).
const PING_PAYLOAD_BYTES: usize = 8;
/// Bytes of the UUID that follows the name in Login Start.
const UUID_BYTES: usize = 16;

/// Settings and shared services every login on this server uses.
pub struct LoginContext {
    pub online_mode: bool,
    pub compression_threshold: Option<usize>,
    pub motd: String,
    pub max_players: usize,
    /// Players currently in the world, maintained by the game loop.
    pub online_players: Arc<AtomicUsize>,
    pub manifest: Arc<RegistryManifest>,
    pub key: Arc<ServerKey>,
    pub verifier: Arc<dyn SessionVerifier>,
}

/// What the client told the server about itself during configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientSettings {
    pub locale: String,
    /// The client's render distance in chunks.
    pub view_distance: u8,
}

/// A player who finished configuration and may join the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinRequest {
    pub profile: GameProfile,
    pub settings: ClientSettings,
}

/// One step the connection must take, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send a packet with the framing currently in effect.
    Send(Packet),
    /// Encrypt and decrypt every later byte with this secret.
    EnableEncryption([u8; SHARED_SECRET_BYTES]),
    /// Use the compressed format for every later frame, both ways.
    EnableCompression(usize),
    /// Close the connection after sending what was queued.
    Close,
    /// Hand the connection to the game loop.
    Join(Box<JoinRequest>),
}

/// Why a connection is dropped without a polite disconnect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    /// A packet that is invalid in the current state.
    Unexpected {
        state: &'static str,
        id: u32,
    },
    /// A packet whose fields do not parse.
    Malformed,
    Crypto(CryptoError),
    /// The verify token did not match.
    TokenMismatch,
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unexpected { state, id } => write!(f, "packet {id} not allowed in {state}"),
            Self::Malformed => f.write_str("malformed login packet"),
            Self::Crypto(error) => write!(f, "encryption failed: {error}"),
            Self::TokenMismatch => f.write_str("verify token mismatch"),
        }
    }
}

impl std::error::Error for LoginError {}

impl From<WireError> for LoginError {
    fn from(_: WireError) -> Self {
        Self::Malformed
    }
}

impl From<CryptoError> for LoginError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<PackListError> for LoginError {
    fn from(_: PackListError) -> Self {
        Self::Malformed
    }
}

/// Disconnect reasons shown to the player.
pub mod reason {
    pub const OUTDATED: &str = "This server runs Minecraft 26.3";
    pub const FULL: &str = "The server is full";
    pub const NOT_VERIFIED: &str = "Failed to verify username";
    pub const SESSION_SERVER_DOWN: &str = "Authentication servers are unavailable";
    pub const BAD_NAME: &str = "Invalid player name";
    pub const UNKNOWN_PACK: &str = "This server needs the vanilla 26.3 core pack";
}

enum State {
    Handshake,
    Status,
    Ping,
    Start {
        outdated: bool,
    },
    Key {
        name: String,
        token: [u8; VERIFY_TOKEN_BYTES],
    },
    Acknowledge(GameProfile),
    Configuration(GameProfile, ClientSettings),
    ConfigurationData(GameProfile, ClientSettings),
    Finished,
}

impl State {
    fn name(&self) -> &'static str {
        match self {
            Self::Handshake => "handshake",
            Self::Status => "status",
            Self::Ping => "ping",
            Self::Start { .. } => "login start",
            Self::Key { .. } => "encryption",
            Self::Acknowledge(_) => "login acknowledgement",
            Self::Configuration(..) => "configuration",
            Self::ConfigurationData(..) => "configuration data",
            Self::Finished => "finished",
        }
    }
}

/// The login state machine of one connection.
pub struct Login {
    context: Arc<LoginContext>,
    state: State,
}

impl Login {
    pub fn new(context: Arc<LoginContext>) -> Self {
        Self {
            context,
            state: State::Handshake,
        }
    }

    /// Whether the connection is still in the status exchange, where the
    /// caller may apply a shorter idle timeout.
    pub fn is_status(&self) -> bool {
        matches!(self.state, State::Status | State::Ping)
    }

    /// Advances on one client packet.
    pub fn handle(&mut self, packet: Packet) -> Result<Vec<Action>, LoginError> {
        let state = std::mem::replace(&mut self.state, State::Finished);
        let unexpected = LoginError::Unexpected {
            state: state.name(),
            id: packet.id,
        };
        let body = packet.body.as_slice();
        let (next, actions) = match (state, packet.id) {
            (State::Handshake, handshake::serverbound::INTENTION) => self.handshake(body)?,
            (State::Status, status::serverbound::STATUS_REQUEST) if body.is_empty() => {
                (State::Ping, vec![Action::Send(self.status())])
            }
            (State::Ping, status::serverbound::PING_REQUEST)
                if body.len() == PING_PAYLOAD_BYTES =>
            {
                let pong = Packet {
                    id: status::clientbound::PONG_RESPONSE,
                    body: body.to_vec(),
                };
                (State::Finished, vec![Action::Send(pong), Action::Close])
            }
            (State::Start { outdated }, login::serverbound::HELLO) => self.hello(body, outdated)?,
            (State::Key { name, token }, login::serverbound::KEY) => {
                self.encryption_response(body, &name, &token)?
            }
            (State::Acknowledge(profile), login::serverbound::LOGIN_ACKNOWLEDGED)
                if body.is_empty() =>
            {
                (
                    State::Configuration(profile, ClientSettings::default()),
                    vec![Action::Send(known_packs())],
                )
            }
            (
                State::Configuration(profile, settings),
                configuration::serverbound::SELECT_KNOWN_PACKS,
            ) => {
                if !recognizes_core_pack(body)? {
                    (State::Finished, disconnect(reason::UNKNOWN_PACK, false)?)
                } else {
                    let packets = initial_configuration(Some(&self.context.manifest));
                    (
                        State::ConfigurationData(profile, settings),
                        packets.into_iter().map(Action::Send).collect(),
                    )
                }
            }
            (State::Configuration(profile, _), configuration::serverbound::CLIENT_INFORMATION) => (
                State::Configuration(profile, client_settings(body)?),
                Vec::new(),
            ),
            (
                State::ConfigurationData(profile, _),
                configuration::serverbound::CLIENT_INFORMATION,
            ) => (
                State::ConfigurationData(profile, client_settings(body)?),
                Vec::new(),
            ),
            (
                state @ (State::Configuration(..) | State::ConfigurationData(..)),
                configuration::serverbound::CUSTOM_PAYLOAD,
            ) => (state, Vec::new()),
            (
                State::ConfigurationData(profile, settings),
                configuration::serverbound::FINISH_CONFIGURATION,
            ) if body.is_empty() => (
                State::Finished,
                vec![Action::Join(Box::new(JoinRequest { profile, settings }))],
            ),
            _ => return Err(unexpected),
        };
        self.state = next;
        Ok(actions)
    }

    fn handshake(&self, body: &[u8]) -> Result<(State, Vec<Action>), LoginError> {
        let Some((version, n)) = read_varint(body)? else {
            return Err(LoginError::Malformed);
        };
        let (_host, rest) = read_string(&body[n..], MAX_HOST_BYTES)?;
        let Some(rest) = rest.get(2..) else {
            return Err(LoginError::Malformed);
        };
        let Some((next, n)) = read_varint(rest)? else {
            return Err(LoginError::Malformed);
        };
        if n != rest.len() {
            return Err(LoginError::Malformed);
        }
        match next {
            intent::STATUS => Ok((State::Status, Vec::new())),
            intent::LOGIN => Ok((
                State::Start {
                    outdated: version != PROTOCOL as u32,
                },
                Vec::new(),
            )),
            _ => Err(LoginError::Malformed),
        }
    }

    fn status(&self) -> Packet {
        let document = json!({
            "version": {"name": VERSION, "protocol": PROTOCOL},
            "players": {
                "max": self.context.max_players,
                "online": self.context.online_players.load(Ordering::Relaxed),
            },
            "description": {"text": self.context.motd},
        });
        let mut body = Vec::new();
        put_string(&document.to_string(), &mut body);
        Packet {
            id: status::clientbound::STATUS_RESPONSE,
            body,
        }
    }

    fn hello(&self, body: &[u8], outdated: bool) -> Result<(State, Vec<Action>), LoginError> {
        let (name, rest) = read_string(body, super::identity::MAX_NAME_CHARS)?;
        if rest.len() != UUID_BYTES {
            return Err(LoginError::Malformed);
        }
        if outdated {
            return Ok((State::Finished, disconnect(reason::OUTDATED, true)?));
        }
        if !is_valid_name(name) {
            return Ok((State::Finished, disconnect(reason::BAD_NAME, true)?));
        }
        if self.context.online_players.load(Ordering::Relaxed) >= self.context.max_players {
            return Ok((State::Finished, disconnect(reason::FULL, true)?));
        }
        if !self.context.online_mode {
            let profile = GameProfile::offline(name);
            return self.finish_login(profile, Vec::new());
        }
        let mut token = [0u8; VERIFY_TOKEN_BYTES];
        random_bytes(&mut token)?;
        let mut request = Vec::new();
        put_string("", &mut request);
        put_prefixed(self.context.key.public_der(), &mut request);
        put_prefixed(&token, &mut request);
        request.push(1); // should authenticate
        Ok((
            State::Key {
                name: name.to_owned(),
                token,
            },
            vec![Action::Send(Packet {
                id: login::clientbound::HELLO,
                body: request,
            })],
        ))
    }

    fn encryption_response(
        &self,
        body: &[u8],
        name: &str,
        token: &[u8; VERIFY_TOKEN_BYTES],
    ) -> Result<(State, Vec<Action>), LoginError> {
        let (encrypted_secret, rest) = read_prefixed(body)?;
        let (encrypted_token, rest) = read_prefixed(rest)?;
        if !rest.is_empty() {
            return Err(LoginError::Malformed);
        }
        let key = &self.context.key;
        if key.decrypt(encrypted_token)? != token {
            return Err(LoginError::TokenMismatch);
        }
        let secret = key.decrypt_secret(encrypted_secret)?;
        let mut actions = vec![Action::EnableEncryption(secret)];
        let hash = server_hash(&[b"", &secret, key.public_der()]);
        match self.context.verifier.has_joined(name, &hash) {
            Ok(Some(profile)) => {
                let (state, more) = self.finish_login(profile, Vec::new())?;
                actions.extend(more);
                Ok((state, actions))
            }
            Ok(None) => {
                actions.extend(disconnect(reason::NOT_VERIFIED, true)?);
                Ok((State::Finished, actions))
            }
            Err(VerifyError::Unavailable(_) | VerifyError::BadResponse) => {
                actions.extend(disconnect(reason::SESSION_SERVER_DOWN, true)?);
                Ok((State::Finished, actions))
            }
        }
    }

    /// Set Compression (when configured) and Login Success.
    fn finish_login(
        &self,
        profile: GameProfile,
        mut actions: Vec<Action>,
    ) -> Result<(State, Vec<Action>), LoginError> {
        if let Some(threshold) = self.context.compression_threshold {
            let mut body = Vec::new();
            put_varint(threshold as u32, &mut body);
            actions.push(Action::Send(Packet {
                id: login::clientbound::SET_COMPRESSION,
                body,
            }));
            actions.push(Action::EnableCompression(threshold));
        }
        let mut body = Vec::new();
        profile.encode(&mut body);
        let mut session = [0u8; UUID_BYTES];
        random_bytes(&mut session)?;
        body.extend_from_slice(&session);
        actions.push(Action::Send(Packet {
            id: login::clientbound::LOGIN_FINISHED,
            body,
        }));
        Ok((State::Acknowledge(profile), actions))
    }
}

/// Reads Client Information's leading locale and view distance; the other
/// fields (chat mode, skin parts, hand, filtering, listing, particles) are
/// not used yet and are left unparsed.
fn client_settings(body: &[u8]) -> Result<ClientSettings, LoginError> {
    let (locale, rest) = read_string(body, super::identity::MAX_NAME_CHARS)?;
    let &view_distance = rest.first().ok_or(LoginError::Malformed)?;
    Ok(ClientSettings {
        locale: locale.to_owned(),
        view_distance,
    })
}

/// A login-phase disconnect (`login_state`) or configuration-phase one.
fn disconnect(text: &str, login_state: bool) -> Result<Vec<Action>, LoginError> {
    let mut body = Vec::new();
    let component = json!({ "text": text }).to_string();
    if login_state {
        put_string(&component, &mut body);
        Ok(vec![
            Action::Send(Packet {
                id: login::clientbound::LOGIN_DISCONNECT,
                body,
            }),
            Action::Close,
        ])
    } else {
        crate::java_wire::put_nbt_text(text, &mut body)?;
        Ok(vec![
            Action::Send(Packet {
                id: configuration::clientbound::DISCONNECT,
                body,
            }),
            Action::Close,
        ])
    }
}

fn put_prefixed(bytes: &[u8], out: &mut Vec<u8>) {
    put_varint(bytes.len() as u32, out);
    out.extend_from_slice(bytes);
}

fn read_prefixed(input: &[u8]) -> Result<(&[u8], &[u8]), LoginError> {
    let Some((length, n)) = read_varint(input)? else {
        return Err(LoginError::Malformed);
    };
    let length = length as usize;
    if length > MAX_ENCRYPTED_FIELD {
        return Err(LoginError::Malformed);
    }
    let rest = &input[n..];
    if rest.len() < length {
        return Err(LoginError::Malformed);
    }
    Ok(rest.split_at(length))
}

#[cfg(test)]
mod tests;
