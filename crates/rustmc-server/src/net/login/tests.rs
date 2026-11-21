//! Unit tests for `net::login`: complete status, offline and online flows.

use std::sync::Mutex;

use super::*;
use crate::net::identity::offline_uuid;

/// Session server stand-in that records the hash it was asked about.
struct FakeVerifier {
    answer: Option<GameProfile>,
    asked: Mutex<Vec<(String, String)>>,
}

impl SessionVerifier for FakeVerifier {
    fn has_joined(&self, name: &str, hash: &str) -> Result<Option<GameProfile>, VerifyError> {
        self.asked
            .lock()
            .expect("lock")
            .push((name.to_owned(), hash.to_owned()));
        Ok(self.answer.clone())
    }
}

fn context(online_mode: bool, verifier: Arc<FakeVerifier>, online: usize) -> Arc<LoginContext> {
    Arc::new(LoginContext {
        online_mode,
        compression_threshold: Some(256),
        motd: "A \"quoted\" server".to_owned(),
        max_players: 2,
        online_players: Arc::new(AtomicUsize::new(online)),
        manifest: Arc::new(RegistryManifest {
            tags: Vec::new(),
            registries: Vec::new(),
        }),
        key: Arc::new(ServerKey::generate().expect("key")),
        verifier,
    })
}

fn verifier(answer: Option<GameProfile>) -> Arc<FakeVerifier> {
    Arc::new(FakeVerifier {
        answer,
        asked: Mutex::new(Vec::new()),
    })
}

fn packet(id: u32, body: Vec<u8>) -> Packet {
    Packet { id, body }
}

fn handshake_packet(version: u32, next: u32) -> Packet {
    let mut body = Vec::new();
    put_varint(version, &mut body);
    put_string("localhost", &mut body);
    body.extend_from_slice(&25565u16.to_be_bytes());
    put_varint(next, &mut body);
    packet(handshake::serverbound::INTENTION, body)
}

fn hello_packet(name: &str) -> Packet {
    let mut body = Vec::new();
    put_string(name, &mut body);
    body.extend_from_slice(&[0; 16]);
    packet(login::serverbound::HELLO, body)
}

fn sent_ids(actions: &[Action]) -> Vec<u32> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Send(p) => Some(p.id),
            _ => None,
        })
        .collect()
}

fn login_to_start(login: &mut Login) {
    let actions = login
        .handle(handshake_packet(PROTOCOL as u32, intent::LOGIN))
        .expect("handshake");
    assert!(actions.is_empty());
}

#[test]
fn status_reports_motd_players_and_version_then_pongs() {
    let mut login = Login::new(context(false, verifier(None), 1));
    login
        .handle(handshake_packet(PROTOCOL as u32, intent::STATUS))
        .expect("handshake");
    assert!(login.is_status());
    let actions = login
        .handle(packet(status::serverbound::STATUS_REQUEST, Vec::new()))
        .expect("status");
    let Action::Send(response) = &actions[0] else {
        panic!("status response expected");
    };
    let (text, _) = read_string(&response.body, 1 << 15).expect("json");
    let document: serde_json::Value = serde_json::from_str(text).expect("valid json");
    assert_eq!(document["players"]["max"], 2);
    assert_eq!(document["players"]["online"], 1);
    assert_eq!(document["version"]["protocol"], PROTOCOL);
    assert_eq!(document["description"]["text"], "A \"quoted\" server");
    let actions = login
        .handle(packet(
            status::serverbound::PING_REQUEST,
            vec![1, 2, 3, 4, 5, 6, 7, 8],
        ))
        .expect("ping");
    assert_eq!(
        actions,
        vec![
            Action::Send(packet(
                status::clientbound::PONG_RESPONSE,
                vec![1, 2, 3, 4, 5, 6, 7, 8]
            )),
            Action::Close
        ]
    );
}

#[test]
fn offline_login_compresses_then_configures_and_joins() {
    let mut login = Login::new(context(false, verifier(None), 0));
    login_to_start(&mut login);
    let actions = login.handle(hello_packet("Steve")).expect("hello");
    assert_eq!(actions[1], Action::EnableCompression(256));
    assert_eq!(
        sent_ids(&actions),
        [
            login::clientbound::SET_COMPRESSION,
            login::clientbound::LOGIN_FINISHED
        ]
    );
    let Action::Send(success) = &actions[2] else {
        panic!("login success expected");
    };
    assert_eq!(success.body[..16], offline_uuid("Steve"));
    assert_eq!(
        success.body.len(),
        16 + 6 + 1 + 16,
        "profile then session id"
    );

    let actions = login
        .handle(packet(login::serverbound::LOGIN_ACKNOWLEDGED, Vec::new()))
        .expect("ack");
    assert_eq!(
        sent_ids(&actions),
        [configuration::clientbound::SELECT_KNOWN_PACKS]
    );
    let mut info = Vec::new();
    put_string("en_us", &mut info);
    info.extend_from_slice(&[12, 0, 1]);
    login
        .handle(packet(configuration::serverbound::CLIENT_INFORMATION, info))
        .expect("client information");
    let actions = login
        .handle(packet(
            configuration::serverbound::SELECT_KNOWN_PACKS,
            known_packs().body,
        ))
        .expect("known packs");
    assert_eq!(
        sent_ids(&actions).last(),
        Some(&configuration::clientbound::FINISH_CONFIGURATION)
    );
    let actions = login
        .handle(packet(
            configuration::serverbound::FINISH_CONFIGURATION,
            Vec::new(),
        ))
        .expect("finish");
    let [Action::Join(join)] = actions.as_slice() else {
        panic!("join expected, got {actions:?}");
    };
    assert_eq!(join.profile.name, "Steve");
    assert_eq!(join.settings.view_distance, 12);
    assert_eq!(join.settings.locale, "en_us");
}

/// Plays the client side of Encryption Request: returns the key packet and
/// the secret it chose.
fn answer_encryption(
    context: &LoginContext,
    request: &Packet,
    token_override: Option<&[u8]>,
) -> (Packet, [u8; 16]) {
    let (server_id, rest) = read_string(&request.body, 20).expect("server id");
    assert_eq!(server_id, "");
    let (public_key, rest) = read_prefixed(rest).expect("key");
    assert_eq!(public_key, context.key.public_der());
    let (token, rest) = read_prefixed(rest).expect("token");
    assert_eq!(token.len(), VERIFY_TOKEN_BYTES);
    assert_eq!(rest, [1], "should authenticate");
    let secret = [5u8; 16];
    let mut body = Vec::new();
    put_prefixed(
        &context.key.encrypt_for_test(&secret).expect("secret"),
        &mut body,
    );
    let token = token_override.unwrap_or(token);
    put_prefixed(
        &context.key.encrypt_for_test(token).expect("token"),
        &mut body,
    );
    (packet(login::serverbound::KEY, body), secret)
}

#[test]
fn online_login_encrypts_and_verifies_with_the_session_server() {
    let account = GameProfile {
        uuid: [9; 16],
        name: "Notch".to_owned(),
        properties: Vec::new(),
    };
    let fake = verifier(Some(account.clone()));
    let context = context(true, Arc::clone(&fake), 0);
    let mut login = Login::new(Arc::clone(&context));
    login_to_start(&mut login);
    let actions = login.handle(hello_packet("Notch")).expect("hello");
    let [Action::Send(request)] = actions.as_slice() else {
        panic!("encryption request expected");
    };
    assert_eq!(request.id, login::clientbound::HELLO);
    let (key_packet, secret) = answer_encryption(&context, request, None);
    let actions = login.handle(key_packet).expect("key");
    assert_eq!(actions[0], Action::EnableEncryption(secret));
    assert_eq!(
        sent_ids(&actions),
        [
            login::clientbound::SET_COMPRESSION,
            login::clientbound::LOGIN_FINISHED
        ]
    );
    let Action::Send(success) = actions.last().expect("success") else {
        panic!("login success expected");
    };
    assert_eq!(
        success.body[..16],
        account.uuid,
        "verified UUID, not offline"
    );
    let asked = fake.asked.lock().expect("lock").clone();
    assert_eq!(
        asked,
        vec![(
            "Notch".to_owned(),
            server_hash(&[&secret, context.key.public_der()])
        )]
    );
}

#[test]
fn online_login_refuses_unverified_players_and_wrong_tokens() {
    let context = context(true, verifier(None), 0);
    let mut login = Login::new(Arc::clone(&context));
    login_to_start(&mut login);
    let actions = login.handle(hello_packet("Cracked")).expect("hello");
    let Action::Send(request) = &actions[0] else {
        panic!("request expected");
    };
    let (key_packet, _) = answer_encryption(&context, request, None);
    let actions = login.handle(key_packet).expect("key");
    assert_eq!(sent_ids(&actions), [login::clientbound::LOGIN_DISCONNECT]);
    assert_eq!(actions.last(), Some(&Action::Close));

    let mut login = Login::new(Arc::clone(&context));
    login_to_start(&mut login);
    let actions = login.handle(hello_packet("Forger")).expect("hello");
    let Action::Send(request) = &actions[0] else {
        panic!("request expected");
    };
    let (key_packet, _) = answer_encryption(&context, request, Some(&[0, 0, 0, 0]));
    assert_eq!(login.handle(key_packet), Err(LoginError::TokenMismatch));
}

#[test]
fn outdated_clients_full_servers_and_bad_names_are_disconnected() {
    let mut login = Login::new(context(false, verifier(None), 0));
    login
        .handle(handshake_packet(PROTOCOL as u32 - 1, intent::LOGIN))
        .expect("handshake");
    let actions = login.handle(hello_packet("Steve")).expect("hello");
    assert_eq!(sent_ids(&actions), [login::clientbound::LOGIN_DISCONNECT]);

    let mut login = Login::new(context(false, verifier(None), 2));
    login_to_start(&mut login);
    let actions = login.handle(hello_packet("Steve")).expect("hello");
    assert_eq!(sent_ids(&actions), [login::clientbound::LOGIN_DISCONNECT]);

    let mut login = Login::new(context(false, verifier(None), 0));
    login_to_start(&mut login);
    let actions = login.handle(hello_packet("bad name")).expect("hello");
    assert_eq!(actions.last(), Some(&Action::Close));
}

#[test]
fn packets_out_of_order_or_malformed_are_errors() {
    // Login Acknowledged before any handshake (Hello shares id 0 with the
    // handshake, so it would parse as a malformed handshake instead).
    let mut login = Login::new(context(false, verifier(None), 0));
    assert!(matches!(
        login.handle(packet(login::serverbound::LOGIN_ACKNOWLEDGED, Vec::new())),
        Err(LoginError::Unexpected { .. })
    ));
    let mut login = Login::new(context(false, verifier(None), 0));
    assert_eq!(
        login.handle(packet(handshake::serverbound::INTENTION, vec![0x80])),
        Err(LoginError::Malformed)
    );
    let mut login = Login::new(context(false, verifier(None), 0));
    login_to_start(&mut login);
    let mut short = Vec::new();
    put_string("Steve", &mut short);
    assert_eq!(
        login.handle(packet(login::serverbound::HELLO, short)),
        Err(LoginError::Malformed)
    );
}
