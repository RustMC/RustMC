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

/// A minimal `registries.json` report: the registries the server loads, with
/// a few items whose names match default-terrain blocks.
const REGISTRIES_REPORT: &str = r#"{
    "minecraft:item": { "entries": {
        "minecraft:air": { "protocol_id": 0 },
        "minecraft:stone": { "protocol_id": 1 },
        "minecraft:dirt": { "protocol_id": 28 }
    } },
    "minecraft:data_component_type": { "entries": {
        "minecraft:max_stack_size": { "protocol_id": 1 }
    } },
    "minecraft:potion": { "entries": { "minecraft:water": { "protocol_id": 0 } } },
    "minecraft:mob_effect": { "entries": { "minecraft:speed": { "protocol_id": 0 } } },
    "minecraft:entity_type": { "entries": { "minecraft:player": { "protocol_id": 159 } } }
}"#;

/// Players join test servers in Creative so they can place and break blocks
/// without survival rules.
fn game_mode_for_tests() -> crate::server::join::GameMode {
    crate::server::join::GameMode::Creative
}

/// A started server and the means to stop it.
struct TestServer {
    address: SocketAddr,
    shutdown: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<Result<(), ServerError>>>,
    /// Removed when the server is dropped, after it stopped.
    dir: Option<TempDir>,
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

fn temp_dir() -> std::path::PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "rustmc-server-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn start(online_mode: bool, verifier: Option<Arc<dyn SessionVerifier>>) -> TestServer {
    let dir = temp_dir();
    let mut server = start_in(&dir, online_mode, verifier);
    server.dir = Some(TempDir(dir));
    server
}

/// Starts a server whose world lives in `dir`; the caller owns `dir`.
fn start_in(
    dir: &std::path::Path,
    online_mode: bool,
    verifier: Option<Arc<dyn SessionVerifier>>,
) -> TestServer {
    let manifest = dir.join("registries.toml");
    std::fs::write(&manifest, MANIFEST).expect("manifest");
    let table = dir.join("registry-table.json");
    std::fs::write(&table, registry_table()).expect("table");
    let report = dir.join("registries.json");
    std::fs::write(&report, REGISTRIES_REPORT).expect("report");
    let mut config = ServerConfig::with_registries(manifest, table, report);
    config.game_mode = game_mode_for_tests();
    config.generation_workers = 2;
    config.world_dir = dir.join("world");
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
        dir: None,
    }
}

/// The client side of one connection.
struct Client {
    stream: TcpStream,
    encoder: FrameEncoder,
    decoder: FrameDecoder,
    /// Body of the last Synchronize Player Position received.
    last_teleport: Option<Vec<u8>>,
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
            last_teleport: None,
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
                self.last_teleport = Some(packet.body);
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
    let secret = crate::net::crypto::test_secret();
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
    let spawn = spawn_position(&mut client);
    walk_to(&mut client, spawn, [400.5, 120.0, 0.5]);
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

/// Reads the first Synchronize Player Position after joining.
/// Moves the player from `from` to `to` in steps short enough to be
/// accepted as walking.
fn walk_to(client: &mut Client, from: [f64; 3], to: [f64; 3]) {
    const STEP: f64 = 9.0;
    let distance = from
        .iter()
        .zip(to)
        .map(|(a, b)| (b - a) * (b - a))
        .sum::<f64>()
        .sqrt();
    let steps = (distance / STEP).ceil().max(1.0) as u32;
    for step in 1..=steps {
        let t = f64::from(step) / f64::from(steps);
        let mut body = Vec::new();
        for (a, b) in from.iter().zip(to) {
            body.extend((a + (b - a) * t).to_be_bytes());
        }
        body.push(1);
        client.send(play::serverbound::MOVE_PLAYER_POS, &body);
    }
}

fn spawn_position(client: &mut Client) -> [f64; 3] {
    // finish_login stops at the teleport; its body starts with the teleport
    // id VarInt (1) and then x, y, z as doubles.
    let teleport = client.last_teleport.clone().expect("teleport");
    let mut position = [0.0; 3];
    for (axis, value) in position.iter_mut().enumerate() {
        let start = 1 + axis * 8;
        *value = f64::from_be_bytes(teleport[start..start + 8].try_into().expect("double"));
    }
    position
}

#[test]
fn players_and_journaled_edits_survive_a_restart() {
    use crate::storage::journal::{BlockEdit, Journal};
    use crate::storage::world_files::WorldFiles;
    let dir = temp_dir();
    let world = dir.join("world");
    {
        let server = start_in(&dir, false, None);
        let mut client = Client::connect(server.address);
        client.handshake(intent::LOGIN);
        client.hello("Saver");
        client.finish_login();
        client.send(play::serverbound::ACCEPT_TELEPORTATION, &[1]);
        let spawn = spawn_position(&mut client);
        walk_to(&mut client, spawn, [10.5, 70.0, -8.5]);
        let mut moved = Vec::new();
        for value in [10.5f64, 70.0, -8.5] {
            moved.extend(value.to_be_bytes());
        }
        moved.extend(45.0f32.to_be_bytes());
        moved.extend(10.0f32.to_be_bytes());
        moved.push(1);
        client.send(play::serverbound::MOVE_PLAYER_POS_ROT, &moved);
        // Turning in place sends rotation only; it must still be saved.
        let mut turned = 135.0f32.to_be_bytes().to_vec();
        turned.extend(5.0f32.to_be_bytes());
        turned.push(1);
        client.send(play::serverbound::MOVE_PLAYER_ROT, &turned);
        // A status ping completes only after the game loop handled the move.
        wait_for_online(server.address, 1);
        drop(client);
        wait_for_online(server.address, 0);
    }
    // Leave an edit in the journal, as a crash after a tick would.
    let edit = BlockEdit {
        x: 5,
        y: 100,
        z: 5,
        state: crate::world::Block::Obsidian as u32,
    };
    {
        let (mut journal, _) =
            Journal::open(&WorldFiles::new(&world).journal_path()).expect("journal");
        journal.append(edit).expect("append");
        journal.sync().expect("sync");
    }
    {
        let server = start_in(&dir, false, None);
        let mut client = Client::connect(server.address);
        client.handshake(intent::LOGIN);
        client.hello("Saver");
        client.finish_login();
        assert_eq!(spawn_position(&mut client), [10.5, 70.0, -8.5]);
    }
    let files = WorldFiles::new(&world);
    let saved = files
        .load_player(&offline_uuid("Saver"))
        .expect("load")
        .expect("saved");
    assert_eq!(
        (saved.yaw, saved.pitch),
        (135.0, 5.0),
        "rotation-only turn was saved"
    );
    let tables =
        crate::chunk_adapter::RegistryTables::from_provisioned(&registry_table()).expect("tables");
    let column = files
        .load_column(0, 0, &tables)
        .expect("load")
        .expect("saved");
    assert_eq!(
        column.get(5, 100, 5),
        Some(edit.state),
        "the journaled edit was saved"
    );
    let (_, replay) = Journal::open(&files.journal_path()).expect("journal");
    assert!(replay.is_empty(), "the journal was emptied after the save");
    drop(TempDir(dir));
}

/// Joins `name` and confirms the first teleport.
fn join_playing(address: SocketAddr, name: &str) -> Client {
    let mut client = Client::connect(address);
    client.handshake(intent::LOGIN);
    client.hello(name);
    client.finish_login();
    client.send(play::serverbound::ACCEPT_TELEPORTATION, &[1]);
    client
}

/// Reads packets until one with `id` arrives, answering keep-alives and
/// chunk batches on the way. Bounded by time, not by count: keep-alives keep
/// the socket busy, so a missing packet would otherwise never time out.
fn receive_until(client: &mut Client, id: u32) -> Packet {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let packet = client.receive();
        match packet.id {
            play::clientbound::KEEP_ALIVE => {
                client.send(play::serverbound::KEEP_ALIVE, &packet.body);
            }
            play::clientbound::CHUNK_BATCH_FINISHED => {
                client.send(
                    play::serverbound::CHUNK_BATCH_RECEIVED,
                    &64.0f32.to_be_bytes(),
                );
            }
            _ => {}
        }
        if packet.id == id {
            return packet;
        }
    }
    panic!("packet {id} never arrived");
}

/// The position and state of a Block Update body.
fn block_update_of(packet: &Packet) -> ((i32, i32, i32), u32) {
    let packed = i64::from_be_bytes(packet.body[..8].try_into().expect("position"));
    let (state, _) = read_varint(&packet.body[8..]).unwrap().unwrap();
    (
        crate::discovery_java::actions::unpack_block_pos(packed),
        state,
    )
}

fn use_item_on(pos: (i32, i32, i32), face: u32, sequence: u32) -> Vec<u8> {
    let mut body = Vec::new();
    put_varint(0, &mut body); // main hand
    body.extend(crate::java_wire::pack_block_position(pos.0, pos.1, pos.2).to_be_bytes());
    put_varint(face, &mut body);
    for cursor in [0.5f32, 1.0, 0.5] {
        body.extend(cursor.to_be_bytes());
    }
    body.extend([0, 0]); // not inside a block, no world border hit
    put_varint(sequence, &mut body);
    body
}

#[test]
fn placed_and_broken_blocks_reach_other_players_and_survive_a_restart() {
    use crate::item::ItemStack;
    use crate::protocol::ids::{DynamicRegistryIds, ProtocolIds};
    use crate::world::Block;
    let dir = temp_dir();
    let top;
    {
        let server = start_in(&dir, false, None);
        let mut builder = join_playing(server.address, "Builder");
        let mut watcher = join_playing(server.address, "Watcher");
        receive_until(&mut builder, play::clientbound::CHUNK_BATCH_FINISHED);
        receive_until(&mut watcher, play::clientbound::CHUNK_BATCH_FINISHED);
        let surface = spawn_position(&mut builder)[1] as i32;
        let ids = ProtocolIds::from_json_str(std::path::Path::new("r.json"), REGISTRIES_REPORT)
            .expect("ids");
        let dynamic = DynamicRegistryIds::default();
        let mut slot = 36i16.to_be_bytes().to_vec(); // hotbar slot 0
        let stone = ItemStack::try_new("minecraft:stone", 64, 64).expect("stack");
        crate::protocol::slot::encode_slot(&Some(stone), &ids, &dynamic, &mut slot).expect("slot");
        builder.send(play::serverbound::SET_CREATIVE_MODE_SLOT, &slot);
        // Place on top of the ground two columns away from spawn; the test
        // world uses the default seed, 0.
        let generator = crate::world::Generator::new(0);
        top = crate::server::world::terrain::surface_y(generator, 2, 0);
        let ground = (2, top - 1, 0);
        builder.send(play::serverbound::USE_ITEM_ON, &use_item_on(ground, 1, 7));
        let ack = receive_until(&mut builder, play::clientbound::BLOCK_CHANGED_ACK);
        assert_eq!(read_varint(&ack.body).unwrap().unwrap().0, 7);
        let seen = receive_until(&mut watcher, play::clientbound::BLOCK_UPDATE);
        assert_eq!(block_update_of(&seen), ((2, top, 0), Block::Stone as u32));
        // Placing into a player's body is refused and corrected.
        builder.send(
            play::serverbound::USE_ITEM_ON,
            &use_item_on((0, surface - 1, 0), 1, 8),
        );
        let corrected = receive_until(&mut builder, play::clientbound::BLOCK_UPDATE);
        assert_eq!(
            block_update_of(&corrected),
            ((0, surface, 0), Block::Air as u32)
        );
        // Breaking in Creative is instant and reaches the watcher.
        let mut dig = Vec::new();
        put_varint(0, &mut dig); // started digging
        dig.extend(crate::java_wire::pack_block_position(2, top, 0).to_be_bytes());
        dig.push(1);
        put_varint(9, &mut dig);
        builder.send(play::serverbound::PLAYER_ACTION, &dig);
        let broken = receive_until(&mut watcher, play::clientbound::BLOCK_UPDATE);
        assert_eq!(block_update_of(&broken), ((2, top, 0), Block::Air as u32));
        // Place again so a block is left for the restart.
        builder.send(play::serverbound::USE_ITEM_ON, &use_item_on(ground, 1, 10));
        let placed = receive_until(&mut watcher, play::clientbound::BLOCK_UPDATE);
        assert_eq!(block_update_of(&placed).1, Block::Stone as u32);
    }
    let tables =
        crate::chunk_adapter::RegistryTables::from_provisioned(&registry_table()).expect("tables");
    let column = crate::storage::world_files::WorldFiles::new(&dir.join("world"))
        .load_column(0, 0, &tables)
        .expect("load")
        .expect("saved");
    assert_eq!(column.get(2, top, 0), Some(Block::Stone as u32));
    drop(TempDir(dir));
}

#[test]
fn two_players_see_each_other_join_move_and_leave() {
    let server = start(false, None);
    let mut builder = join_playing(server.address, "Builder");
    receive_until(&mut builder, play::clientbound::CHUNK_BATCH_FINISHED);
    let mut watcher = join_playing(server.address, "Watcher");
    // The joining player's tab list lists both players.
    let info = receive_until(&mut watcher, play::clientbound::PLAYER_INFO_UPDATE);
    let listed = crate::protocol::player_info::PlayerInfoUpdatePacket::decode_payload(&info.body)
        .expect("player info");
    let mut names: Vec<_> = listed.entries.iter().map(|e| e.name.clone()).collect();
    names.sort();
    assert_eq!(names, ["Builder", "Watcher"]);
    // The earlier player learns about the new one.
    let joined = receive_until(&mut builder, play::clientbound::PLAYER_INFO_UPDATE);
    let joined = crate::protocol::player_info::PlayerInfoUpdatePacket::decode_payload(&joined.body)
        .expect("player info");
    assert_eq!(joined.entries.len(), 1);
    assert_eq!(joined.entries[0].name, "Watcher");
    // Each sees the other's entity once it has the chunk they stand in.
    let spawned = receive_until(&mut watcher, play::clientbound::ADD_ENTITY);
    let (builder_entity, n) = read_varint(&spawned.body).unwrap().unwrap();
    assert_eq!(spawned.body[n..n + 16], offline_uuid("Builder"));
    receive_until(&mut builder, play::clientbound::ADD_ENTITY);
    // Movement reaches the watcher.
    let builder_spawn = spawn_position(&mut builder);
    let mut moved = Vec::new();
    for value in [3.5f64, builder_spawn[1], 4.5] {
        moved.extend(value.to_be_bytes());
    }
    moved.push(1);
    builder.send(play::serverbound::MOVE_PLAYER_POS, &moved);
    let sync = receive_until(&mut watcher, play::clientbound::ENTITY_POSITION_SYNC);
    let (entity, n) = read_varint(&sync.body).unwrap().unwrap();
    assert_eq!(entity, builder_entity);
    let (path, m) = read_varint(&sync.body[n..]).unwrap().unwrap();
    assert_eq!(path, 0, "linear path");
    let at = n + m;
    let x = f64::from_be_bytes(sync.body[at..at + 8].try_into().expect("x"));
    assert_eq!(x, 3.5);
    // Leaving removes the entity and the tab-list entry.
    drop(builder);
    let gone = receive_until(&mut watcher, play::clientbound::PLAYER_INFO_REMOVE);
    assert_eq!(gone.body[1..17], offline_uuid("Builder"));
    let removed = receive_until(&mut watcher, play::clientbound::REMOVE_ENTITIES);
    assert_eq!(read_varint(&removed.body).unwrap().unwrap().0, 1);
    assert_eq!(
        read_varint(&removed.body[1..]).unwrap().unwrap().0,
        builder_entity
    );
}

#[test]
fn a_move_of_more_than_ten_blocks_is_undone() {
    let server = start(false, None);
    let mut client = join_playing(server.address, "Jumper");
    let spawn = spawn_position(&mut client);
    let mut far = Vec::new();
    for value in [spawn[0] + 50.0, spawn[1], spawn[2]] {
        far.extend(value.to_be_bytes());
    }
    far.push(1);
    client.send(play::serverbound::MOVE_PLAYER_POS, &far);
    let back = receive_until(&mut client, play::clientbound::PLAYER_POSITION);
    let (teleport, n) = read_varint(&back.body).unwrap().unwrap();
    assert_eq!(teleport, 2, "a new teleport id");
    let x = f64::from_be_bytes(back.body[n..n + 8].try_into().expect("x"));
    assert_eq!(x, spawn[0], "back to the last accepted position");
}
