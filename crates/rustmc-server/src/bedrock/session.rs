//! Bedrock session state machine and handshake lifecycle for protocol version 766 (v1.21.50).

use crate::bedrock::packet::{
    BEDROCK_PROTOCOL_VERSION, BedrockIdentity, BedrockPacket, Login, NetworkSettings, PlayStatus,
    RequestNetworkSettings, ResourcePackClientResponse, ResourcePackStack, ResourcePacksInfo,
    ResponseStatus, StartGame,
};
use std::fmt;

/// Bedrock connection session state lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BedrockSessionState {
    Offline,
    Handshake,
    Connected,
    ResourcePacks,
    InGame,
    Disconnected,
}

impl fmt::Display for BedrockSessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Errors occurring during Bedrock session packet handling or state transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BedrockSessionError {
    OutOfOrder {
        expected: BedrockSessionState,
        actual: BedrockSessionState,
    },
    ProtocolMismatch {
        expected: i32,
        actual: i32,
    },
    PackRefused,
    MalformedPacket(String),
    Disconnected,
}

impl BedrockSessionError {
    /// Return the corresponding PlayStatus disconnect code if applicable.
    pub fn play_status(&self) -> Option<PlayStatus> {
        match self {
            Self::ProtocolMismatch { .. } => Some(PlayStatus::FailedClient),
            Self::OutOfOrder { .. } => Some(PlayStatus::FailedClient),
            Self::MalformedPacket(_) => Some(PlayStatus::FailedClient),
            Self::PackRefused => Some(PlayStatus::FailedClient),
            Self::Disconnected => Some(PlayStatus::FailedClient),
        }
    }
}

impl fmt::Display for BedrockSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfOrder { expected, actual } => {
                write!(f, "out of order packet: expected {expected}, was {actual}")
            }
            Self::ProtocolMismatch { expected, actual } => {
                write!(
                    f,
                    "protocol version mismatch: server requires {expected}, client provided {actual}"
                )
            }
            Self::PackRefused => write!(f, "client refused required resource packs"),
            Self::MalformedPacket(reason) => write!(f, "malformed packet: {reason}"),
            Self::Disconnected => write!(f, "session is disconnected"),
        }
    }
}

impl std::error::Error for BedrockSessionError {}

/// Bedrock client connection session managing state transitions, packet negotiation,
/// identity extraction, and world/player initialization.
#[derive(Debug, Clone)]
pub struct BedrockSession {
    session_id: u64,
    seed: i64,
    state: BedrockSessionState,
    identity: Option<BedrockIdentity>,
    disconnect_status: Option<PlayStatus>,

    pub entity_id: i64,
    pub runtime_id: u64,
    pub player_gamemode: i32,
    pub player_pos: (f32, f32, f32),
    pub pitch_yaw: (f32, f32),
    pub dimension: i32,
    pub generator: i32,
    pub world_gamemode: i32,
    pub difficulty: i32,
    pub spawn_pos: (i32, i32, i32),
    pub level_id: String,
    pub world_name: String,

    pub compression_threshold: u16,
    pub compression_algorithm: u16,
}

impl BedrockSession {
    /// Create a new Bedrock session with a session identifier and world seed.
    pub fn new(session_id: u64, seed: i64) -> Self {
        Self {
            session_id,
            seed,
            state: BedrockSessionState::Offline,
            identity: None,
            disconnect_status: None,

            entity_id: session_id as i64,
            runtime_id: session_id,
            player_gamemode: 0, // Survival
            player_pos: (0.0, 64.0, 0.0),
            pitch_yaw: (0.0, 0.0),
            dimension: 0, // Overworld
            generator: 1, // Overworld / infinite
            world_gamemode: 0,
            difficulty: 1, // Normal
            spawn_pos: (0, 64, 0),
            level_id: "world".to_string(),
            world_name: "RustMC Bedrock World".to_string(),

            compression_threshold: 0, // No compression threshold for local preview
            compression_algorithm: 0, // Deflate / Zlib
        }
    }

    pub fn state(&self) -> BedrockSessionState {
        self.state
    }

    pub fn set_state(&mut self, state: BedrockSessionState) {
        self.state = state;
    }

    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    pub fn seed(&self) -> i64 {
        self.seed
    }

    pub fn identity(&self) -> Option<&BedrockIdentity> {
        self.identity.as_ref()
    }

    pub fn disconnect_status(&self) -> Option<PlayStatus> {
        self.disconnect_status
    }

    pub fn connect(&mut self) {
        if self.state == BedrockSessionState::Offline {
            self.state = BedrockSessionState::Connected;
        }
    }

    pub fn disconnect(&mut self, status: Option<PlayStatus>) {
        self.state = BedrockSessionState::Disconnected;
        self.disconnect_status = status;
    }

    pub fn with_world(
        mut self,
        spawn_pos: (i32, i32, i32),
        level_id: impl Into<String>,
        world_name: impl Into<String>,
    ) -> Self {
        self.spawn_pos = spawn_pos;
        self.player_pos = (spawn_pos.0 as f32, spawn_pos.1 as f32, spawn_pos.2 as f32);
        self.level_id = level_id.into();
        self.world_name = world_name.into();
        self
    }

    pub fn with_player_pos(mut self, pos: (f32, f32, f32), pitch_yaw: (f32, f32)) -> Self {
        self.player_pos = pos;
        self.pitch_yaw = pitch_yaw;
        self
    }

    pub fn with_network_settings(mut self, threshold: u16, algorithm: u16) -> Self {
        self.compression_threshold = threshold;
        self.compression_algorithm = algorithm;
        self
    }

    /// Handle client RequestNetworkSettings packet (0xC1 / 193).
    ///
    /// Validates protocol version 766, transitions state to `Handshake`,
    /// and emits `NetworkSettings` (0x8F / 143).
    /// Rejects or transitions to `Disconnected` on version mismatch or out-of-order state.
    pub fn handle_network_settings_request(
        &mut self,
        request: impl Into<RequestNetworkSettings>,
    ) -> Result<Vec<BedrockPacket>, BedrockSessionError> {
        let req = request.into();
        if !matches!(
            self.state,
            BedrockSessionState::Offline | BedrockSessionState::Connected
        ) {
            self.state = BedrockSessionState::Disconnected;
            self.disconnect_status = Some(PlayStatus::FailedClient);
            return Err(BedrockSessionError::OutOfOrder {
                expected: BedrockSessionState::Offline,
                actual: self.state,
            });
        }

        if req.protocol_version != BEDROCK_PROTOCOL_VERSION {
            self.state = BedrockSessionState::Disconnected;
            self.disconnect_status = Some(PlayStatus::FailedClient);
            return Err(BedrockSessionError::ProtocolMismatch {
                expected: BEDROCK_PROTOCOL_VERSION,
                actual: req.protocol_version,
            });
        }

        self.state = BedrockSessionState::Handshake;
        let settings = NetworkSettings::new(self.compression_threshold, self.compression_algorithm);
        Ok(vec![BedrockPacket::NetworkSettings(settings)])
    }

    /// Strongly-typed variant of `handle_network_settings_request`.
    pub fn handle_network_settings_request_typed(
        &mut self,
        request: &RequestNetworkSettings,
    ) -> Result<NetworkSettings, BedrockSessionError> {
        let packets = self.handle_network_settings_request(request.clone())?;
        match packets.into_iter().next() {
            Some(BedrockPacket::NetworkSettings(s)) => Ok(s),
            _ => Err(BedrockSessionError::MalformedPacket(
                "expected NetworkSettings response".to_string(),
            )),
        }
    }

    /// Handle client Login packet (0x01).
    ///
    /// Validates protocol version 766, extracts player identity tokens,
    /// emits `PlayStatus(LoginSuccess)` and `ResourcePacksInfo`, and transitions to `ResourcePacks`.
    /// Rejects or transitions to `Disconnected` on version mismatch or out-of-order state.
    pub fn handle_login(
        &mut self,
        login: impl Into<Login>,
    ) -> Result<Vec<BedrockPacket>, BedrockSessionError> {
        let login_pkt = login.into();
        if !matches!(
            self.state,
            BedrockSessionState::Handshake | BedrockSessionState::Connected
        ) {
            self.state = BedrockSessionState::Disconnected;
            self.disconnect_status = Some(PlayStatus::FailedClient);
            return Err(BedrockSessionError::OutOfOrder {
                expected: BedrockSessionState::Handshake,
                actual: self.state,
            });
        }

        if login_pkt.protocol_version != BEDROCK_PROTOCOL_VERSION {
            self.state = BedrockSessionState::Disconnected;
            self.disconnect_status = Some(PlayStatus::FailedClient);
            return Err(BedrockSessionError::ProtocolMismatch {
                expected: BEDROCK_PROTOCOL_VERSION,
                actual: login_pkt.protocol_version,
            });
        }

        if let Ok(identity) = login_pkt.extract_identity() {
            self.identity = Some(identity);
        }

        self.state = BedrockSessionState::ResourcePacks;
        let packs_info = ResourcePacksInfo::new(false, false, false);
        Ok(vec![
            BedrockPacket::PlayStatus(PlayStatus::LoginSuccess),
            BedrockPacket::ResourcePacksInfo(packs_info),
        ])
    }

    /// Strongly-typed variant of `handle_login`.
    pub fn handle_login_typed(
        &mut self,
        login: &Login,
    ) -> Result<(PlayStatus, ResourcePacksInfo), BedrockSessionError> {
        let mut packets = self.handle_login(login.clone())?;
        if packets.len() == 2 {
            let info = match packets.pop().unwrap() {
                BedrockPacket::ResourcePacksInfo(i) => i,
                _ => {
                    return Err(BedrockSessionError::MalformedPacket(
                        "expected ResourcePacksInfo".to_string(),
                    ));
                }
            };
            let status = match packets.pop().unwrap() {
                BedrockPacket::PlayStatus(s) => s,
                _ => {
                    return Err(BedrockSessionError::MalformedPacket(
                        "expected PlayStatus".to_string(),
                    ));
                }
            };
            Ok((status, info))
        } else {
            Err(BedrockSessionError::MalformedPacket(
                "unexpected login response count".to_string(),
            ))
        }
    }

    /// Handle client ResourcePackClientResponse (0x08).
    ///
    /// - `HaveAllPacks`: emits `ResourcePackStack`
    /// - `Completed`: emits `StartGame` and `PlayStatus(PlayerSpawn)`, transitions to `InGame`
    /// - `Refused`: transitions to `Disconnected` and returns `Err(PackRefused)`
    pub fn handle_pack_client_response(
        &mut self,
        response: impl Into<ResourcePackClientResponse>,
    ) -> Result<Vec<BedrockPacket>, BedrockSessionError> {
        let resp = response.into();
        if self.state != BedrockSessionState::ResourcePacks {
            self.state = BedrockSessionState::Disconnected;
            self.disconnect_status = Some(PlayStatus::FailedClient);
            return Err(BedrockSessionError::OutOfOrder {
                expected: BedrockSessionState::ResourcePacks,
                actual: self.state,
            });
        }

        match resp.status {
            ResponseStatus::HaveAllPacks => {
                let stack = ResourcePackStack::new(false);
                Ok(vec![BedrockPacket::ResourcePackStack(stack)])
            }
            ResponseStatus::Completed => {
                self.state = BedrockSessionState::InGame;
                let start_game = StartGame {
                    entity_id: self.entity_id,
                    runtime_id: self.runtime_id,
                    player_gamemode: self.player_gamemode,
                    player_pos: self.player_pos,
                    pitch_yaw: self.pitch_yaw,
                    seed: self.seed,
                    dimension: self.dimension,
                    generator: self.generator,
                    world_gamemode: self.world_gamemode,
                    difficulty: self.difficulty,
                    spawn_pos: self.spawn_pos,
                    level_id: self.level_id.clone(),
                    world_name: self.world_name.clone(),
                };
                Ok(vec![
                    BedrockPacket::StartGame(start_game),
                    BedrockPacket::PlayStatus(PlayStatus::PlayerSpawn),
                ])
            }
            ResponseStatus::Refused => {
                self.state = BedrockSessionState::Disconnected;
                self.disconnect_status = Some(PlayStatus::FailedClient);
                Err(BedrockSessionError::PackRefused)
            }
            ResponseStatus::SendPacks => {
                // Server has no optional packs to download; advance with empty stack
                let stack = ResourcePackStack::new(false);
                Ok(vec![BedrockPacket::ResourcePackStack(stack)])
            }
            ResponseStatus::None => Ok(Vec::new()),
        }
    }

    /// Strongly-typed variant for `ResponseStatus::HaveAllPacks`.
    pub fn handle_pack_client_response_have_all_packs(
        &mut self,
    ) -> Result<ResourcePackStack, BedrockSessionError> {
        let packets = self.handle_pack_client_response(ResponseStatus::HaveAllPacks)?;
        match packets.into_iter().next() {
            Some(BedrockPacket::ResourcePackStack(stack)) => Ok(stack),
            _ => Err(BedrockSessionError::MalformedPacket(
                "expected ResourcePackStack".to_string(),
            )),
        }
    }

    /// Strongly-typed variant for `ResponseStatus::Completed`.
    pub fn handle_pack_client_response_completed(
        &mut self,
    ) -> Result<(StartGame, PlayStatus), BedrockSessionError> {
        let mut packets = self.handle_pack_client_response(ResponseStatus::Completed)?;
        if packets.len() == 2 {
            let status = match packets.pop().unwrap() {
                BedrockPacket::PlayStatus(s) => s,
                _ => {
                    return Err(BedrockSessionError::MalformedPacket(
                        "expected PlayStatus".to_string(),
                    ));
                }
            };
            let start = match packets.pop().unwrap() {
                BedrockPacket::StartGame(g) => g,
                _ => {
                    return Err(BedrockSessionError::MalformedPacket(
                        "expected StartGame".to_string(),
                    ));
                }
            };
            Ok((start, status))
        } else {
            Err(BedrockSessionError::MalformedPacket(
                "unexpected completed response count".to_string(),
            ))
        }
    }

    /// General packet dispatcher routing incoming Bedrock packets to appropriate handler.
    pub fn handle_packet(
        &mut self,
        packet: &BedrockPacket,
    ) -> Result<Vec<BedrockPacket>, BedrockSessionError> {
        match packet {
            BedrockPacket::RequestNetworkSettings(req) => {
                self.handle_network_settings_request(req.clone())
            }
            BedrockPacket::Login(login) => self.handle_login(login.clone()),
            BedrockPacket::ResourcePackClientResponse(resp) => {
                self.handle_pack_client_response(resp.clone())
            }
            _ => {
                self.state = BedrockSessionState::Disconnected;
                self.disconnect_status = Some(PlayStatus::FailedClient);
                Err(BedrockSessionError::OutOfOrder {
                    expected: match self.state {
                        BedrockSessionState::Offline | BedrockSessionState::Connected => {
                            BedrockSessionState::Offline
                        }
                        BedrockSessionState::Handshake => BedrockSessionState::Handshake,
                        BedrockSessionState::ResourcePacks => BedrockSessionState::ResourcePacks,
                        _ => self.state,
                    },
                    actual: self.state,
                })
            }
        }
    }
}
