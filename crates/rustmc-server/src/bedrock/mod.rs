//! Bedrock protocol version 766 (v1.21.50) packet models and session state machine.

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
        let info = ResourcePacksInfo::new(true, false, true);
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
            ResponseStatus::None,
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
        let mut stack = ResourcePackStack::new(false);
        stack.behavior_packs.push(PackInstance::new(
            "behavior-uuid-1",
            "1.0.0",
            "subpack-behav",
        ));
        stack
            .resource_packs
            .push(PackInstance::new("resource-uuid-1", "1.2.0", "subpack-res"));

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
            &ResourcePacksInfo::new(false, false, false)
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
        let outdated_req = RequestNetworkSettings::new(765);
        let err = session
            .handle_network_settings_request(&outdated_req)
            .unwrap_err();
        assert_eq!(
            err,
            BedrockSessionError::ProtocolMismatch {
                expected: BEDROCK_PROTOCOL_VERSION,
                actual: 765
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
}
