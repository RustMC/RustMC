//! Bedrock protocol 2193 (1.26.51) packet models and session state machine.

pub mod packet;
pub mod session;

pub use packet::{
    BEDROCK_GAME_VERSION, BEDROCK_PROTOCOL_VERSION, BedrockIdentity, BedrockPacket, ID_LOGIN,
    ID_NETWORK_SETTINGS, ID_PLAY_STATUS, ID_REQUEST_NETWORK_SETTINGS,
    ID_RESOURCE_PACK_CLIENT_RESPONSE, ID_RESOURCE_PACK_STACK, ID_RESOURCE_PACKS_INFO,
    ID_START_GAME, Login, NetworkSettings, PackInstance, PlayStatus, RequestNetworkSettings,
    ResourcePackClientResponse, ResourcePackStack, ResourcePacksInfo, ResponseStatus, StartGame,
    base64_url_decode, base64_url_encode, put_bedrock_string, put_varint32, put_varint64,
    put_varuint32, put_varuint64, read_bedrock_string, read_varint32, read_varint64,
    read_varuint32, read_varuint64,
};

pub use session::{BedrockSession, BedrockSessionError, BedrockSessionState};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_network_settings_codec() {
        let req = RequestNetworkSettings::new(BEDROCK_PROTOCOL_VERSION);
        let bytes = req.encode();
        let decoded = RequestNetworkSettings::decode(&bytes).unwrap();
        assert_eq!(decoded.protocol_version, BEDROCK_PROTOCOL_VERSION);

        let packet = BedrockPacket::RequestNetworkSettings(req.clone());
        let packet_bytes = packet.encode();
        let decoded_packet = BedrockPacket::decode(&packet_bytes).unwrap();
        assert_eq!(decoded_packet, packet);
    }

    #[test]
    fn test_network_settings_codec() {
        let settings = NetworkSettings::new(512, 0);
        let bytes = settings.encode();
        let decoded = NetworkSettings::decode(&bytes).unwrap();
        assert_eq!(decoded.compression_threshold, 512);
        assert_eq!(decoded.compression_algorithm, 0);

        let packet = BedrockPacket::NetworkSettings(settings.clone());
        let packet_bytes = packet.encode();
        let decoded_packet = BedrockPacket::decode(&packet_bytes).unwrap();
        assert_eq!(decoded_packet, packet);
    }

    #[test]
    fn test_login_codec_and_identity_extraction() {
        let login = Login::create(
            BEDROCK_PROTOCOL_VERSION,
            "AlexGamer",
            "2535412345678901",
            "MHYwEAYHKoZIzj0CAQYFK4EEACIDYgAEMockKeyDataForTesting123456789",
        );
        let bytes = login.encode();
        let decoded = Login::decode(&bytes).unwrap();
        assert_eq!(decoded.protocol_version, BEDROCK_PROTOCOL_VERSION);

        let identity = decoded.extract_identity().unwrap();
        assert_eq!(identity.display_name, "AlexGamer");
        assert_eq!(identity.xuid, "2535412345678901");
        assert_eq!(
            identity.identity_public_key,
            "MHYwEAYHKoZIzj0CAQYFK4EEACIDYgAEMockKeyDataForTesting123456789"
        );

        let packet = BedrockPacket::Login(login.clone());
        let packet_bytes = packet.encode();
        let decoded_packet = BedrockPacket::decode(&packet_bytes).unwrap();
        assert_eq!(decoded_packet, packet);
    }

    #[test]
    fn test_play_status_codec() {
        for status in [
            PlayStatus::LoginSuccess,
            PlayStatus::FailedClient,
            PlayStatus::FailedServer,
            PlayStatus::PlayerSpawn,
        ] {
            let bytes = status.encode();
            let decoded = PlayStatus::decode(&bytes).unwrap();
            assert_eq!(decoded, status);

            let packet = BedrockPacket::PlayStatus(status);
            let packet_bytes = packet.encode();
            let decoded_packet = BedrockPacket::decode(&packet_bytes).unwrap();
            assert_eq!(decoded_packet, packet);
        }
    }

    #[test]
    fn test_resource_packs_info_codec() {
        let info = ResourcePacksInfo::new(true, false, false, true);
        let bytes = info.encode();
        let decoded = ResourcePacksInfo::decode(&bytes).unwrap();
        assert_eq!(decoded, info);

        let packet = BedrockPacket::ResourcePacksInfo(info.clone());
        let packet_bytes = packet.encode();
        let decoded_packet = BedrockPacket::decode(&packet_bytes).unwrap();
        assert_eq!(decoded_packet, packet);
    }

    #[test]
    fn test_resource_pack_client_response_codec() {
        for status in [
            ResponseStatus::Refused,
            ResponseStatus::SendPacks,
            ResponseStatus::HaveAllPacks,
            ResponseStatus::Completed,
        ] {
            let resp = ResourcePackClientResponse::new(status);
            let bytes = resp.encode();
            let decoded = ResourcePackClientResponse::decode(&bytes).unwrap();
            assert_eq!(decoded.status, status);

            let packet = BedrockPacket::ResourcePackClientResponse(resp);
            let packet_bytes = packet.encode();
            let decoded_packet = BedrockPacket::decode(&packet_bytes).unwrap();
            assert_eq!(decoded_packet, packet);
        }
    }

    #[test]
    fn test_resource_pack_stack_codec() {
        let stack = ResourcePackStack::with_packs(
            false,
            vec![
                PackInstance::new("behavior-uuid-1", "1.0.0", "subpack-behav"),
                PackInstance::new("resource-uuid-1", "1.2.0", "subpack-res"),
            ],
        );

        let bytes = stack.encode();
        let decoded = ResourcePackStack::decode(&bytes).unwrap();
        assert_eq!(decoded, stack);

        let packet = BedrockPacket::ResourcePackStack(stack.clone());
        let packet_bytes = packet.encode();
        let decoded_packet = BedrockPacket::decode(&packet_bytes).unwrap();
        assert_eq!(decoded_packet, packet);
    }

    #[test]
    fn test_start_game_codec() {
        let start = StartGame {
            entity_id: 1001,
            runtime_id: 1,
            player_gamemode: 0,
            player_pos: (12.5, 64.0, -45.75),
            pitch_yaw: (15.0, 90.0),
            seed: 987654321012345,
            dimension: 0,
            generator: 1,
            world_gamemode: 0,
            difficulty: 2,
            spawn_pos: (12, 64, -45),
            level_id: "preview_level".to_string(),
            world_name: "RustMC Preview World".to_string(),
            level_current_time: 6000,
            enchantment_seed: -7,
            multiplayer_correlation_id: "correlation".to_string(),
            server_version: "RustMC test".to_string(),
        };

        let bytes = start.encode();
        let decoded = StartGame::decode(&bytes).unwrap();
        assert_eq!(decoded, start);
        assert_eq!(decoded.pitch(), 15.0);
        assert_eq!(decoded.yaw(), 90.0);

        let packet = BedrockPacket::StartGame(start.clone());
        let packet_bytes = packet.encode();
        let decoded_packet = BedrockPacket::decode(&packet_bytes).unwrap();
        assert_eq!(decoded_packet, packet);
    }

    #[test]
    fn test_framed_packets_roundtrip() {
        let start = StartGame {
            entity_id: 42,
            runtime_id: 42,
            player_gamemode: 1,
            player_pos: (0.0, 70.0, 0.0),
            pitch_yaw: (0.0, 0.0),
            seed: 12345,
            dimension: 0,
            generator: 1,
            world_gamemode: 1,
            difficulty: 1,
            spawn_pos: (0, 70, 0),
            level_id: "world".to_string(),
            world_name: "World".to_string(),
            level_current_time: 0,
            enchantment_seed: 0,
            multiplayer_correlation_id: String::new(),
            server_version: String::new(),
        };
        let packet = BedrockPacket::StartGame(start);
        let framed = packet.encode_framed();
        let (decoded, len) = BedrockPacket::decode_framed(&framed).unwrap();
        assert_eq!(decoded, packet);
        assert_eq!(len, framed.len());
    }

    #[test]
    fn test_full_handshake_transition_to_ingame() {
        let mut session = BedrockSession::new(100, 1234567890).with_world(
            (10, 65, 20),
            "test_level",
            "Test World",
        );
        assert_eq!(session.state(), BedrockSessionState::Offline);

        // 1. handle_network_settings_request
        let req = RequestNetworkSettings::new(BEDROCK_PROTOCOL_VERSION);
        let packets1 = session.handle_network_settings_request(&req).unwrap();
        assert_eq!(session.state(), BedrockSessionState::Handshake);
        assert_eq!(packets1.len(), 1);
        let settings = packets1[0].as_network_settings().unwrap();
        assert_eq!(settings.compression_threshold, 0);
        assert_eq!(settings.compression_algorithm, 0);

        // 2. handle_login
        let login = Login::create(
            BEDROCK_PROTOCOL_VERSION,
            "Steve",
            "999888777",
            "pubkey_for_steve",
        );
        let packets2 = session.handle_login(&login).unwrap();
        assert_eq!(session.state(), BedrockSessionState::ResourcePacks);
        assert_eq!(packets2.len(), 2);
        assert_eq!(
            packets2[0].as_play_status().unwrap(),
            &PlayStatus::LoginSuccess
        );
        assert_eq!(
            packets2[1].as_resource_packs_info().unwrap(),
            &ResourcePacksInfo::default()
        );
        let ident = session.identity().unwrap();
        assert_eq!(ident.display_name, "Steve");
        assert_eq!(ident.xuid, "999888777");

        // 3. handle_pack_client_response(HaveAllPacks)
        let packets3 = session
            .handle_pack_client_response(ResponseStatus::HaveAllPacks)
            .unwrap();
        assert_eq!(session.state(), BedrockSessionState::ResourcePacks);
        assert_eq!(packets3.len(), 1);
        assert_eq!(
            packets3[0].as_resource_pack_stack().unwrap(),
            &ResourcePackStack::new(false)
        );

        // 4. handle_pack_client_response(Completed)
        let packets4 = session
            .handle_pack_client_response(ResponseStatus::Completed)
            .unwrap();
        assert_eq!(session.state(), BedrockSessionState::InGame);
        assert_eq!(packets4.len(), 2);
        let start_game = packets4[0].as_start_game().unwrap();
        assert_eq!(start_game.seed, 1234567890);
        assert_eq!(start_game.spawn_pos, (10, 65, 20));
        assert_eq!(start_game.level_id, "test_level");
        assert_eq!(start_game.world_name, "Test World");
        assert_eq!(
            packets4[1].as_play_status().unwrap(),
            &PlayStatus::PlayerSpawn
        );
    }

    #[test]
    fn test_typed_pipeline_helpers() {
        let mut session = BedrockSession::new(42, 987);
        let req = RequestNetworkSettings::new(BEDROCK_PROTOCOL_VERSION);
        let settings = session.handle_network_settings_request_typed(&req).unwrap();
        assert_eq!(settings.compression_threshold, 0);

        let login = Login::create(BEDROCK_PROTOCOL_VERSION, "Player", "123", "key");
        let (status, info) = session.handle_login_typed(&login).unwrap();
        assert_eq!(status, PlayStatus::LoginSuccess);
        assert!(!info.must_accept);

        let stack = session
            .handle_pack_client_response_have_all_packs()
            .unwrap();
        assert!(!stack.must_accept);

        let (start, status) = session.handle_pack_client_response_completed().unwrap();
        assert_eq!(status, PlayStatus::PlayerSpawn);
        assert_eq!(start.seed, 987);
        assert_eq!(session.state(), BedrockSessionState::InGame);
    }

    #[test]
    fn test_protocol_mismatch_in_network_settings() {
        let mut session = BedrockSession::new(1, 100);
        let outdated_req = RequestNetworkSettings::new(2192);
        let err = session
            .handle_network_settings_request(&outdated_req)
            .unwrap_err();
        assert_eq!(
            err,
            BedrockSessionError::ProtocolMismatch {
                expected: BEDROCK_PROTOCOL_VERSION,
                actual: 2192
            }
        );
        assert_eq!(err.play_status(), Some(PlayStatus::FailedClient));
        assert_eq!(session.state(), BedrockSessionState::Disconnected);
        assert_eq!(session.disconnect_status(), Some(PlayStatus::FailedClient));
    }

    #[test]
    fn test_protocol_mismatch_in_login() {
        let mut session = BedrockSession::new(1, 100);
        session
            .handle_network_settings_request(RequestNetworkSettings::new(BEDROCK_PROTOCOL_VERSION))
            .unwrap();
        assert_eq!(session.state(), BedrockSessionState::Handshake);

        let outdated_login = Login::new(700, vec![]);
        let err = session.handle_login(&outdated_login).unwrap_err();
        assert_eq!(
            err,
            BedrockSessionError::ProtocolMismatch {
                expected: BEDROCK_PROTOCOL_VERSION,
                actual: 700
            }
        );
        assert_eq!(session.state(), BedrockSessionState::Disconnected);
        assert_eq!(session.disconnect_status(), Some(PlayStatus::FailedClient));
    }

    #[test]
    fn test_out_of_order_packets() {
        // 1. Sending Login when session is Offline
        let mut session1 = BedrockSession::new(1, 100);
        let login = Login::new(BEDROCK_PROTOCOL_VERSION, vec![]);
        let err1 = session1.handle_login(&login).unwrap_err();
        assert!(matches!(err1, BedrockSessionError::OutOfOrder { .. }));
        assert_eq!(session1.state(), BedrockSessionState::Disconnected);
        assert_eq!(session1.disconnect_status(), Some(PlayStatus::FailedClient));

        // 2. Sending ResourcePackClientResponse before Login
        let mut session2 = BedrockSession::new(2, 200);
        let err2 = session2
            .handle_pack_client_response(ResponseStatus::HaveAllPacks)
            .unwrap_err();
        assert!(matches!(err2, BedrockSessionError::OutOfOrder { .. }));
        assert_eq!(session2.state(), BedrockSessionState::Disconnected);

        // 3. Sending RequestNetworkSettings after in Handshake
        let mut session3 = BedrockSession::new(3, 300);
        session3
            .handle_network_settings_request(RequestNetworkSettings::new(BEDROCK_PROTOCOL_VERSION))
            .unwrap();
        let err3 = session3
            .handle_network_settings_request(RequestNetworkSettings::new(BEDROCK_PROTOCOL_VERSION))
            .unwrap_err();
        assert!(matches!(err3, BedrockSessionError::OutOfOrder { .. }));
        assert_eq!(session3.state(), BedrockSessionState::Disconnected);
    }

    #[test]
    fn test_resource_pack_refused_disconnects() {
        let mut session = BedrockSession::new(1, 100);
        session
            .handle_network_settings_request(RequestNetworkSettings::new(BEDROCK_PROTOCOL_VERSION))
            .unwrap();
        session
            .handle_login(Login::create(BEDROCK_PROTOCOL_VERSION, "User", "1", "key"))
            .unwrap();
        assert_eq!(session.state(), BedrockSessionState::ResourcePacks);

        let err = session
            .handle_pack_client_response(ResponseStatus::Refused)
            .unwrap_err();
        assert_eq!(err, BedrockSessionError::PackRefused);
        assert_eq!(session.state(), BedrockSessionState::Disconnected);
        assert_eq!(session.disconnect_status(), Some(PlayStatus::FailedClient));
    }

    #[test]
    fn test_unified_handle_packet_routing() {
        let mut session = BedrockSession::new(99, 555);

        let p1 = BedrockPacket::RequestNetworkSettings(RequestNetworkSettings::new(
            BEDROCK_PROTOCOL_VERSION,
        ));
        let r1 = session.handle_packet(&p1).unwrap();
        assert_eq!(session.state(), BedrockSessionState::Handshake);
        assert_eq!(r1.len(), 1);

        let p2 = BedrockPacket::Login(Login::create(BEDROCK_PROTOCOL_VERSION, "Alex", "123", "k"));
        let r2 = session.handle_packet(&p2).unwrap();
        assert_eq!(session.state(), BedrockSessionState::ResourcePacks);
        assert_eq!(r2.len(), 2);

        let p3 = BedrockPacket::ResourcePackClientResponse(ResourcePackClientResponse::new(
            ResponseStatus::HaveAllPacks,
        ));
        let r3 = session.handle_packet(&p3).unwrap();
        assert_eq!(r3.len(), 1);

        let p4 = BedrockPacket::ResourcePackClientResponse(ResourcePackClientResponse::new(
            ResponseStatus::Completed,
        ));
        let r4 = session.handle_packet(&p4).unwrap();
        assert_eq!(session.state(), BedrockSessionState::InGame);
        assert_eq!(r4.len(), 2);
    }
    /// Builds an unsigned JWT with the given claims.
    fn jwt(claims: serde_json::Value) -> String {
        format!(
            "{}.{}.{}",
            base64_url_encode(b"{}"),
            base64_url_encode(claims.to_string().as_bytes()),
            base64_url_encode(b"sig")
        )
    }

    fn login_with_chain(chain: serde_json::Value) -> Login {
        let document = serde_json::json!({ "chain": chain }).to_string();
        let mut payload = (document.len() as u32).to_le_bytes().to_vec();
        payload.extend_from_slice(document.as_bytes());
        Login::new(BEDROCK_PROTOCOL_VERSION, payload)
    }

    fn at_login_stage() -> BedrockSession {
        let mut session = BedrockSession::new(1, 100);
        session
            .handle_network_settings_request(RequestNetworkSettings::new(BEDROCK_PROTOCOL_VERSION))
            .unwrap();
        session
    }

    #[test]
    fn test_target_matches_discovery_and_mojang_schema() {
        assert_eq!(BEDROCK_PROTOCOL_VERSION, 2193);
        assert_eq!(BEDROCK_GAME_VERSION, "1.26.51");
    }

    #[test]
    fn test_packet_ids_match_mojang_schema() {
        assert_eq!(ID_LOGIN, 1);
        assert_eq!(ID_PLAY_STATUS, 2);
        assert_eq!(ID_RESOURCE_PACKS_INFO, 6);
        assert_eq!(ID_RESOURCE_PACK_STACK, 7);
        assert_eq!(ID_RESOURCE_PACK_CLIENT_RESPONSE, 8);
        assert_eq!(ID_START_GAME, 11);
        assert_eq!(ID_NETWORK_SETTINGS, 143);
        assert_eq!(ID_REQUEST_NETWORK_SETTINGS, 193);
    }

    #[test]
    fn test_request_network_settings_wire() {
        let wire = [0xc1, 0x01, 0x00, 0x00, 0x08, 0x91];
        assert_eq!(
            RequestNetworkSettings::new(2193).encode(),
            wire,
            "header varuint 193, then a big-endian network version"
        );
        assert_eq!(
            RequestNetworkSettings::decode(&wire)
                .unwrap()
                .protocol_version,
            2193
        );
    }

    #[test]
    fn test_network_settings_wire() {
        // Header, threshold 512, ZLib, throttle off, threshold 0, scalar 0.0.
        let wire = [
            0x8f, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let settings = NetworkSettings::new(512, 0);
        assert_eq!(settings.encode(), wire);
        assert_eq!(NetworkSettings::decode(&wire).unwrap(), settings);

        // Throttle enabled, threshold 3, scalar 3.5.
        let throttled = [
            0x8f, 0x01, 0x01, 0x00, 0x00, 0x00, 0x01, 0x03, 0x00, 0x00, 0x60, 0x40,
        ];
        let decoded = NetworkSettings::decode(&throttled).unwrap();
        assert!(decoded.client_throttle_enabled);
        assert_eq!(decoded.client_throttle_threshold, 3);
        assert_eq!(decoded.client_throttle_scalar, 3.5);
        assert_eq!(decoded.encode(), throttled);

        assert!(NetworkSettings::decode(&wire[..wire.len() - 1]).is_err());
    }

    #[test]
    fn test_play_status_wire() {
        assert_eq!(
            PlayStatus::PlayerSpawn.encode(),
            [0x02, 0x00, 0x00, 0x00, 0x03]
        );
    }

    #[test]
    fn test_resource_packs_info_wire() {
        let mut wire = vec![0x06, 0x01, 0x00, 0x00, 0x01];
        wire.extend_from_slice(&[0; 16]); // World template UUID
        wire.push(0x00); // World template version
        wire.push(0x00); // Resource packs
        let info = ResourcePacksInfo::new(true, false, false, true);
        assert_eq!(info.encode(), wire);
        assert_eq!(ResourcePacksInfo::decode(&wire).unwrap(), info);
    }

    #[test]
    fn test_resource_pack_stack_wire() {
        let wire = [
            0x07, 0x01, // Header, required
            0x00, // Pack list
            0x07, b'1', b'.', b'2', b'6', b'.', b'5', b'1', // Base game version
            0x00, 0x00, 0x00, 0x00, 0x00, // Experiments
            0x00, // Include editor packs
        ];
        let stack = ResourcePackStack::new(true);
        assert_eq!(stack.encode(), wire);
        assert_eq!(ResourcePackStack::decode(&wire).unwrap(), stack);
    }

    #[test]
    fn test_resource_pack_client_response_wire() {
        let finished = [0x08, 0x04];
        assert_eq!(
            ResourcePackClientResponse::new(ResponseStatus::Completed).encode(),
            finished
        );
        assert_eq!(
            ResourcePackClientResponse::decode(&finished)
                .unwrap()
                .status,
            ResponseStatus::Completed
        );
        assert_eq!(
            ResourcePackClientResponse::new(ResponseStatus::SendPacks).encode(),
            [0x08, 0x02, 0x00]
        );
        assert!(ResourcePackClientResponse::decode(&[0x08, 0x00]).is_err());
    }

    /// The `StartGame` body written field by field from Mojang's schema.
    fn start_game_wire() -> Vec<u8> {
        let mut wire: Vec<u8> = vec![
            0x0b, // Header
            0xd2, 0x0f, // Entity ID 1001
            0x01, // Runtime ID 1
            0x00, // Game type: Survival
            0x00, 0x00, 0x48, 0x41, // X 12.5
            0x00, 0x00, 0x80, 0x42, // Y 64.0
            0x00, 0x00, 0x37, 0xc2, // Z -45.75
            0x00, 0x00, 0x70, 0x41, // Pitch 15.0
            0x00, 0x00, 0xb4, 0x42, // Yaw 90.0
            // LevelSettings.
            0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, // Seed
            0x00, 0x00, // Spawn biome type
            0x00, // User defined biome name
            0x00, // Dimension
            0x02, // Generator: Overworld
            0x00, // Game type: Survival
            0x00, // Is hardcore
            0x04, // Difficulty: Normal
            0x18, 0x80, 0x01, 0x59, // Spawn block 12, 64, -45
            0x01, // Achievements disabled
            0x00, // Editor world type
            0x00, 0x00, // Created in, exported from editor
            0x00, // Day cycle stop time
            0x00, // Education edition offer
            0x00, // Education features enabled
            0x00, // Education product ID
            0x00, 0x00, 0x00, 0x00, // Rain level
            0x00, 0x00, 0x00, 0x00, // Lightning level
            0x00, // Confirmed platform locked content
            0x01, // Multiplayer game intent
            0x00, // LAN broadcast intent
            0x00, 0x00, // Xbox Live, platform broadcast settings
            0x00, // Commands enabled
            0x00, // Texture packs required
            0x00, // Game rules
            0x00, 0x00, 0x00, 0x00, // Experiment toggles
            0x00, // Experiments ever toggled
            0x00, 0x00, // Bonus chest, start with map
            0x00, // Player permissions: Visitor
            0x04, 0x00, 0x00, 0x00, // Server chunk tick range
            0x00, 0x00, 0x00, 0x00, 0x00, // Locked packs through MSA gamertags
            0x00, 0x00, 0x00, 0x00, 0x00, // World template through emote chat
            0x07, b'1', b'.', b'2', b'6', b'.', b'5', b'1', // Base game version
            0x00, 0x00, 0x00, 0x00, // Limited world width
            0x00, 0x00, 0x00, 0x00, // Limited world depth
            0x00, // Nether type
            0x00, 0x00, // Edu shared resource
            0x00, // Override force experimental gameplay: absent
            0x00, // Chat restriction level
            0x00, // Disable player interactions
            0x00, // Server editor connection policy
            0x00, // Anonymous block drops in editor worlds
        ];
        wire.extend_from_slice(b"\x02id"); // Level ID
        wire.extend_from_slice(b"\x04name"); // Level name
        wire.extend_from_slice(&[
            0x00, // Template content identity
            0x00, // Is trial
            0x00, 0x00, // Movement settings
            0x70, 0x17, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Level time 6000
            0x0d, // Enchantment seed -7
            0x00, // Block properties
        ]);
        wire.extend_from_slice(b"\x03abc"); // Multiplayer correlation ID
        wire.push(0x00); // Enable item stack net manager
        wire.extend_from_slice(b"\x06RustMC"); // Server version
        wire.extend_from_slice(&[0x0a, 0x00, 0x00]); // Player property data
        wire.extend_from_slice(&[0; 8]); // Block type registry checksum
        wire.extend_from_slice(&[0; 16]); // World template ID
        wire.extend_from_slice(&[
            0x00, 0x00, // Client-side generation, hashed block IDs
            0x00, // Server auth sound enabled
            0x00, // Server configuration join info: absent
            0x00, 0x00, 0x00, 0x00, // Server telemetry data
        ]);
        wire
    }

    fn start_game_value() -> StartGame {
        StartGame {
            entity_id: 1001,
            runtime_id: 1,
            player_gamemode: 0,
            player_pos: (12.5, 64.0, -45.75),
            pitch_yaw: (15.0, 90.0),
            seed: 0x0102_0304_0506_0708,
            dimension: 0,
            generator: 1,
            world_gamemode: 0,
            difficulty: 2,
            spawn_pos: (12, 64, -45),
            level_id: "id".to_string(),
            world_name: "name".to_string(),
            level_current_time: 6000,
            enchantment_seed: -7,
            multiplayer_correlation_id: "abc".to_string(),
            server_version: "RustMC".to_string(),
        }
    }

    #[test]
    fn test_start_game_wire() {
        assert_eq!(start_game_value().encode(), start_game_wire());
        assert_eq!(
            StartGame::decode(&start_game_wire()).unwrap(),
            start_game_value()
        );
    }

    #[test]
    fn test_start_game_rejects_every_truncation_and_trailing_bytes() {
        let wire = start_game_wire();
        for end in 0..wire.len() {
            assert!(
                StartGame::decode(&wire[..end]).is_err(),
                "prefix of {end} bytes should fail"
            );
        }
        let mut trailing = wire;
        trailing.push(0);
        assert!(StartGame::decode(&trailing).is_err());
    }

    #[test]
    fn test_decoders_reject_a_different_packet() {
        assert!(NetworkSettings::decode(&PlayStatus::LoginSuccess.encode()).is_err());
        assert!(StartGame::decode(&PlayStatus::LoginSuccess.encode()).is_err());
    }

    #[test]
    fn test_login_wire_and_length_checks() {
        let wire = [0x01, 0x00, 0x00, 0x08, 0x91, 0x02, 0xaa, 0xbb];
        let login = Login::decode(&wire).unwrap();
        assert_eq!(login.protocol_version, 2193);
        assert_eq!(login.payload, [0xaa, 0xbb]);
        assert_eq!(login.encode(), wire);

        assert!(Login::decode(&wire[..wire.len() - 1]).is_err());
        let mut trailing = wire.to_vec();
        trailing.push(0);
        assert!(Login::decode(&trailing).is_err());
    }

    #[test]
    fn test_identity_comes_from_the_identity_bearing_token() {
        let login = login_with_chain(serde_json::json!([
            jwt(serde_json::json!({ "identityPublicKey": "intermediate-key" })),
            jwt(
                serde_json::json!({ "identityPublicKey": "second-key", "certificateAuthority": true })
            ),
            jwt(serde_json::json!({
                "identityPublicKey": "player-key",
                "extraData": { "displayName": "Alex", "XUID": "42" },
            })),
        ]));
        let identity = login.extract_identity().unwrap();
        assert_eq!(identity.display_name, "Alex");
        assert_eq!(identity.xuid, "42");
        assert_eq!(identity.identity_public_key, "player-key");
    }

    #[test]
    fn test_malformed_identity_payloads_are_rejected() {
        let no_identity = serde_json::json!([jwt(serde_json::json!({
            "identityPublicKey": "intermediate-key"
        }))]);
        let empty_name = serde_json::json!([jwt(serde_json::json!({
            "identityPublicKey": "key",
            "extraData": { "displayName": "", "XUID": "42" },
        }))]);
        let no_key = serde_json::json!([jwt(serde_json::json!({
            "extraData": { "displayName": "Alex", "XUID": "42" },
        }))]);
        for chain in [
            no_identity,
            empty_name,
            no_key,
            serde_json::json!([]),
            serde_json::json!(["not a jwt"]),
            serde_json::json!([17]),
            serde_json::json!("chain"),
        ] {
            assert!(login_with_chain(chain).extract_identity().is_err());
        }

        // Empty, short, overlong length, and non-JSON payloads.
        for payload in [
            vec![],
            vec![0x01],
            vec![0xff, 0xff, 0xff, 0xff, b'{'],
            vec![0x01, 0x00, 0x00, 0x00, b'{'],
        ] {
            let login = Login::new(BEDROCK_PROTOCOL_VERSION, payload);
            assert!(login.extract_identity().is_err());
        }
    }

    #[test]
    fn test_login_without_identity_disconnects() {
        let mut session = at_login_stage();
        let login = Login::new(BEDROCK_PROTOCOL_VERSION, vec![]);
        let err = session.handle_login(&login).unwrap_err();
        assert!(matches!(err, BedrockSessionError::MalformedPacket(_)));
        assert_eq!(session.state(), BedrockSessionState::Disconnected);
        assert_eq!(session.disconnect_status(), Some(PlayStatus::FailedClient));
        assert!(session.identity().is_none());
    }

    #[test]
    fn test_out_of_order_error_names_the_arrival_state() {
        let mut session = BedrockSession::new(1, 100);
        let err = session
            .handle_pack_client_response(ResponseStatus::HaveAllPacks)
            .unwrap_err();
        assert_eq!(
            err,
            BedrockSessionError::OutOfOrder {
                expected: BedrockSessionState::ResourcePacks,
                actual: BedrockSessionState::Offline,
            }
        );
    }

    #[test]
    fn test_session_start_game_encodes_normal_difficulty() {
        let mut session = at_login_stage();
        session
            .handle_login(Login::create(BEDROCK_PROTOCOL_VERSION, "User", "1", "key"))
            .unwrap();
        let (start, _) = session.handle_pack_client_response_completed().unwrap();
        assert_eq!(start.difficulty, 2);
        assert_eq!(StartGame::decode(&start.encode()).unwrap(), start);
    }
}
