//! End-to-end tests of the playable server over real loopback sockets, with
//! a minimal client written from the same protocol facts.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use rsa::pkcs8::DecodePublicKey;

use super::*;
use crate::discovery_java::PROTOCOL;
use crate::java_packets::{configuration, handshake, intent, login, play, status};
use crate::java_wire::{put_string, put_varint, read_string, read_varint};
use crate::net::configuration::known_packs;
use crate::net::framing::{FrameDecoder, FrameEncoder, Packet};
use crate::net::identity::{GameProfile, offline_uuid};
use crate::net::session_server::VerifyError;

const MANIFEST: &str = r#"version = "26.3"
[registries]
"minecraft:dimension_type" = ["minecraft:overworld"]
"minecraft:worldgen/biome" = ["minecraft:plains"]
"minecraft:damage_type" = ["minecraft:generic"]
"minecraft:world_clock" = ["minecraft:overworld"]
"#;

/// A provisioned-format registry table with exactly the default-terrain
/// block states and biomes.
fn registry_table() -> String {
    use crate::world::{Biome, Block};
    let states: serde_json::Map<String, serde_json::Value> = Block::ALL
        .iter()
        .map(|b| (b.state_name().to_owned(), serde_json::json!(*b as u32)))
        .collect();
    let biomes: serde_json::Map<String, serde_json::Value> = Biome::ALL
        .iter()
        .enumerate()
        .map(|(id, b)| (b.identifier().to_owned(), serde_json::json!(id)))
        .collect();
    serde_json::json!({
        "version": crate::chunk_adapter::SUPPORTED_VERSION,
        "protocol": crate::chunk_adapter::SUPPORTED_PROTOCOL,
        "block_state_count": states.len(),
        "block_states": states,
        "biome_count": biomes.len(),
        "biomes": biomes,
    })
    .to_string()
}

/// A started server and the means to stop it.
struct TestServer {
    address: SocketAddr,
    shutdown: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<Result<(), ServerError>>>,
    _dir: TempDir,
}

struct TempDir(std::path::PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn start(online_mode: bool, verifier: Option<Arc<dyn SessionVerifier>>) -> TestServer {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "rustmc-server-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let manifest = dir.join("registries.toml");
    std::fs::write(&manifest, MANIFEST).expect("manifest");
    let table = dir.join("registry-table.json");
    std::fs::write(&table, registry_table()).expect("table");
    let mut config = ServerConfig::with_registries(manifest, table);
    config.generation_workers = 2;
    config.address = "127.0.0.1:0".parse().expect("address");
    config.online_mode = online_mode;
    config.max_players = 2;
    config.motd = "test".to_owned();
    let shutdown = Arc::new(AtomicBool::new(false));
    let (bound_tx, bound_rx) = mpsc::channel();
    let thread = {
        let shutdown = Arc::clone(&shutdown);
        thread::spawn(move || {
            run(config, shutdown, verifier, move |address| {
                let _ = bound_tx.send(address);
            })
        })
    };
    let address = bound_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("server bound");
    TestServer {
        address,
        shutdown,
        thread: Some(thread),
        _dir: TempDir(dir),
    }
}

/// The client side of one connection.
struct Client {
    stream: TcpStream,
    encoder: FrameEncoder,
    decoder: FrameDecoder,
}

impl Client {
    fn connect(address: SocketAddr) -> Self {
        let stream = TcpStream::connect(address).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        Self {
            stream,
            encoder: FrameEncoder::new(),
            decoder: FrameDecoder::new(1 << 21),
        }
    }

    fn send(&mut self, id: u32, body: &[u8]) {
        let mut wire = Vec::new();
        self.encoder.encode(id, body, &mut wire).expect("encode");
        self.stream.write_all(&wire).expect("write");
    }

    fn receive(&mut self) -> Packet {
        loop {
            if let Some(packet) = self.decoder.next_packet().expect("decode") {
                return packet;
            }
            let mut buffer = [0u8; 4096];
            let read = self.stream.read(&mut buffer).expect("read");
            assert!(read > 0, "server closed the connection");
            self.decoder.push(&buffer[..read]).expect("push");
        }
    }

    fn handshake(&mut self, next: u32) {
        let mut body = Vec::new();
        put_varint(PROTOCOL as u32, &mut body);
        put_string("localhost", &mut body);
        body.extend_from_slice(&25565u16.to_be_bytes());
        put_varint(next, &mut body);
        self.send(handshake::serverbound::INTENTION, &body);
    }

    fn hello(&mut self, name: &str) {
        let mut body = Vec::new();
        put_string(name, &mut body);
        body.extend_from_slice(&[0; 16]);
        self.send(login::serverbound::HELLO, &body);
    }

    /// From Set Compression to the first play teleport; returns Login Success.
    fn finish_login(&mut self) -> Packet {
        let compression = self.receive();
        assert_eq!(compression.id, login::clientbound::SET_COMPRESSION);
        let (threshold, _) = read_varint(&compression.body).unwrap().unwrap();
        self.encoder.enable_compression(threshold as usize);
        self.decoder.enable_compression(threshold as usize);
        let success = self.receive();
        assert_eq!(success.id, login::clientbound::LOGIN_FINISHED);
        self.send(login::serverbound::LOGIN_ACKNOWLEDGED, &[]);
        assert_eq!(
            self.receive().id,
            configuration::clientbound::SELECT_KNOWN_PACKS
        );
        self.send(
            configuration::serverbound::SELECT_KNOWN_PACKS,
            &known_packs().body,
        );
        while self.receive().id != configuration::clientbound::FINISH_CONFIGURATION {}
        self.send(configuration::serverbound::FINISH_CONFIGURATION, &[]);
        let mut ids = Vec::new();
        loop {
            let packet = self.receive();
            ids.push(packet.id);
            if packet.id == play::clientbound::PLAYER_POSITION {
                break;
            }
        }
        assert_eq!(ids[0], play::clientbound::LOGIN);
        success
    }

    fn status_online(address: SocketAddr) -> u64 {
        let mut client = Self::connect(address);
        client.handshake(intent::STATUS);
        client.send(status::serverbound::STATUS_REQUEST, &[]);
        let response = client.receive();
        let (text, _) = read_string(&response.body, 1 << 15).expect("json");
        let json: serde_json::Value = serde_json::from_str(text).expect("json");
        json["players"]["online"].as_u64().expect("online")
    }
}

fn wait_for_online(address: SocketAddr, expected: u64) {
    for _ in 0..100 {
        if Client::status_online(address) == expected {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("online count never reached {expected}");
}

#[test]
fn offline_player_joins_and_leaving_frees_the_slot() {
    let server = start(false, None);
    let mut client = Client::connect(server.address);
    client.handshake(intent::LOGIN);
    client.hello("Steve");
    let success = client.finish_login();
    assert_eq!(success.body[..16], offline_uuid("Steve"));
    wait_for_online(server.address, 1);
    drop(client);
    wait_for_online(server.address, 0);
}

#[test]
fn second_login_with_the_same_name_replaces_the_first() {
    let server = start(false, None);
    let mut first = Client::connect(server.address);
    first.handshake(intent::LOGIN);
    first.hello("Alex");
    first.finish_login();
    let mut second = Client::connect(server.address);
    second.handshake(intent::LOGIN);
    second.hello("Alex");
    second.finish_login();
    let mut kicked = false;
    for _ in 0..20 {
        if first.receive().id == play::clientbound::DISCONNECT {
            kicked = true;
            break;
        }
    }
    assert!(kicked, "the first session receives Disconnect");
    wait_for_online(server.address, 1);
}

struct AcceptingVerifier {
    asked: Mutex<Option<String>>,
}

impl SessionVerifier for AcceptingVerifier {
    fn has_joined(&self, name: &str, hash: &str) -> Result<Option<GameProfile>, VerifyError> {
        *self.asked.lock().expect("lock") = Some(hash.to_owned());
        Ok(Some(GameProfile {
            uuid: [7; 16],
            name: name.to_owned(),
            properties: Vec::new(),
        }))
    }
}

#[test]
fn online_player_joins_over_an_encrypted_connection() {
    let verifier = Arc::new(AcceptingVerifier {
        asked: Mutex::new(None),
    });
    let server = start(true, Some(verifier.clone()));
    let mut client = Client::connect(server.address);
    client.handshake(intent::LOGIN);
    client.hello("Notch");
    let request = client.receive();
    assert_eq!(request.id, login::clientbound::HELLO);
    let (_, rest) = read_string(&request.body, 20).expect("server id");
    let (key_len, n) = read_varint(rest).unwrap().unwrap();
    let key_der = &rest[n..n + key_len as usize];
    let rest = &rest[n + key_len as usize..];
    let (token_len, n) = read_varint(rest).unwrap().unwrap();
    let token = rest[n..n + token_len as usize].to_vec();
    let public = rsa::PublicKey::from_public_key_der(key_der).expect("server key");
    let secret = [0x11u8; 16];
    let mut body = Vec::new();
    for plain in [&secret[..], &token[..]] {
        let encrypted = public.pkcs1v15_encrypt(plain).expect("encrypt");
        put_varint(encrypted.len() as u32, &mut body);
        body.extend_from_slice(&encrypted);
    }
    client.send(login::serverbound::KEY, &body);
    client.encoder.enable_encryption(&secret);
    client.decoder.enable_encryption(&secret);
    let success = client.finish_login();
    assert_eq!(success.body[..16], [7; 16], "the verified profile");
    let hash = verifier.asked.lock().expect("lock").clone();
    assert_eq!(
        hash,
        Some(crate::net::crypto::server_hash(&[&secret, key_der]))
    );
}

#[test]
fn a_full_server_refuses_further_logins() {
    let server = start(false, None);
    let mut players = Vec::new();
    for name in ["One", "Two"] {
        let mut client = Client::connect(server.address);
        client.handshake(intent::LOGIN);
        client.hello(name);
        client.finish_login();
        players.push(client);
    }
    wait_for_online(server.address, 2);
    let mut third = Client::connect(server.address);
    third.handshake(intent::LOGIN);
    third.hello("Three");
    assert_eq!(third.receive().id, login::clientbound::LOGIN_DISCONNECT);
}

#[test]
fn joined_players_receive_terrain_and_walking_moves_the_view() {
    let server = start(false, None);
    let mut client = Client::connect(server.address);
    client.handshake(intent::LOGIN);
    client.hello("Walker");
    client.finish_login();
    let mut chunks = Vec::new();
    loop {
        let packet = client.receive();
        if packet.id == play::clientbound::LEVEL_CHUNK_WITH_LIGHT {
            chunks.push((
                i32::from_be_bytes(packet.body[0..4].try_into().expect("x")),
                i32::from_be_bytes(packet.body[4..8].try_into().expect("z")),
            ));
        }
        if packet.id == play::clientbound::CHUNK_BATCH_FINISHED {
            break;
        }
    }
    assert!(
        chunks.contains(&(0, 0)),
        "the spawn chunk comes first: {chunks:?}"
    );
    client.send(
        play::serverbound::CHUNK_BATCH_RECEIVED,
        &25.0f32.to_be_bytes(),
    );
    client.send(play::serverbound::ACCEPT_TELEPORTATION, &[1]);
    let mut moved = Vec::new();
    for value in [400.5f64, 120.0, 0.5] {
        moved.extend(value.to_be_bytes());
    }
    moved.push(1);
    client.send(play::serverbound::MOVE_PLAYER_POS, &moved);
    let mut saw_center = false;
    let mut saw_forget = false;
    for _ in 0..2_000 {
        let packet = client.receive();
        match packet.id {
            play::clientbound::SET_CHUNK_CACHE_CENTER => {
                saw_center = true;
            }
            play::clientbound::FORGET_LEVEL_CHUNK => saw_forget = true,
            play::clientbound::CHUNK_BATCH_FINISHED => client.send(
                play::serverbound::CHUNK_BATCH_RECEIVED,
                &25.0f32.to_be_bytes(),
            ),
            _ => {}
        }
        if saw_center && saw_forget {
            break;
        }
    }
    assert!(saw_center && saw_forget, "moving recentres and forgets");
}
