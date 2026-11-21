//! The game loop: the one thread that owns the shared world state and every
//! joined player, ticking at 20 TPS.
//!
//! Why: a single owner of world state needs no locks and makes every change
//! happen in a defined order; connections only exchange messages with it.
//! Done: joining at the terrain surface, duplicate-login replacement,
//! keep-alive with timeout kicks, teleport confirmation, chunk streaming
//! around each player, clean disconnect on shutdown.
//! Not done: persistence (track C), movement validation and block play (D),
//! chat and commands (E).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use super::config::ServerConfig;
use super::connection::{ConnectionHandle, ConnectionId, Event, Outgoing};
use super::join::{Spawn, join_packets};
use super::log;
use super::world::store::ChunkStore;
use super::world::terrain::surface_y;
use super::world::view::{View, chunk_of};
use crate::discovery_java::keepalive::KeepAliveTracker;
use crate::java_packets::play::{clientbound, serverbound};
use crate::java_wire::{put_nbt_text, read_f32_be, read_varint};
use crate::net::crypto::random_bytes;
use crate::net::framing::Packet;
use crate::net::identity::{GameProfile, uuid_text};
use crate::net::login::JoinRequest;
use crate::preview_data::RegistryManifest;
use crate::protocol::movement::{ServerboundMovementPacket, decode_serverbound_movement_packet};
use crate::world::Generator;

/// Length of one game tick at 20 ticks per second.
pub const TICK: Duration = Duration::from_millis(50);
/// Ticks between keep-alive challenges. The client gives up after 20 s
/// without one and the server kicks after 15 s without an answer (wiki
/// `Keep Alive`), so a challenge every 5 s leaves room for both.
pub const KEEPALIVE_INTERVAL_TICKS: u64 = 100;
/// Ticks in a Minecraft day.
pub const TICKS_PER_DAY: u64 = 24_000;
/// Ticks between unloads of unedited columns no player views.
pub const UNLOAD_INTERVAL_TICKS: u64 = 100;
/// World spawn column; players appear on the terrain surface here.
pub const SPAWN_XZ: [f64; 2] = [0.5, 0.5];

/// Disconnect messages.
pub mod reason {
    pub const TIMED_OUT: &str = "Timed out";
    pub const DUPLICATE: &str = "You logged in from another location";
    pub const SHUTDOWN: &str = "Server closed";
    pub const SLOW: &str = "Connection too slow";
    pub const BAD_PACKET: &str = "Invalid packet";
}

struct Player {
    profile: GameProfile,
    handle: ConnectionHandle,
    keepalive: KeepAliveTracker,
    last_keepalive_tick: u64,
    awaiting_teleport: Option<u32>,
    position: [f64; 3],
    view: View,
}

/// The world-owning game loop.
pub struct Game {
    config: Arc<ServerConfig>,
    manifest: Arc<RegistryManifest>,
    online: Arc<AtomicUsize>,
    generator: Generator,
    store: ChunkStore,
    players: HashMap<ConnectionId, Player>,
    next_entity_id: i32,
    tick: u64,
    day_time: u64,
}

impl Game {
    pub fn new(
        config: Arc<ServerConfig>,
        manifest: Arc<RegistryManifest>,
        online: Arc<AtomicUsize>,
        generator: Generator,
        store: ChunkStore,
    ) -> Self {
        Self {
            config,
            manifest,
            online,
            generator,
            store,
            players: HashMap::new(),
            next_entity_id: 1,
            tick: 0,
            day_time: 0,
        }
    }

    /// Runs until `shutdown` is set, then disconnects every player.
    pub fn run(mut self, events: &Receiver<Event>, shutdown: &AtomicBool) {
        let mut next_tick = Instant::now() + TICK;
        while !shutdown.load(Ordering::SeqCst) {
            let now = Instant::now();
            if now >= next_tick {
                self.tick();
                next_tick += TICK;
                if next_tick < now {
                    // Fell behind by more than a tick: skip, do not burst.
                    next_tick = now + TICK;
                }
                continue;
            }
            match events.recv_timeout(next_tick - now) {
                Ok(event) => self.handle(event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        for player in self.players.values() {
            disconnect(&player.handle, reason::SHUTDOWN);
        }
        self.players.clear();
        self.online.store(0, Ordering::Relaxed);
    }

    /// Handles one inbox event.
    pub fn handle(&mut self, event: Event) {
        match event {
            Event::ChunkReady(ready) => self.store.insert(*ready),
            Event::Joined { handle, join } => self.join(handle, *join),
            Event::Packet { id, packet } => self.packet(id, packet),
            Event::Left { id, reason } => {
                if let Some(player) = self.players.remove(&id) {
                    self.online.store(self.players.len(), Ordering::Relaxed);
                    log::event(
                        "player_left",
                        &[("name", &player.profile.name), ("reason", &reason)],
                    );
                }
            }
        }
    }

    fn join(&mut self, handle: ConnectionHandle, join: JoinRequest) {
        let profile = join.profile;
        let duplicates: Vec<ConnectionId> = self
            .players
            .iter()
            .filter(|(_, p)| p.profile.uuid == profile.uuid)
            .map(|(id, _)| *id)
            .collect();
        for id in duplicates {
            if let Some(old) = self.players.remove(&id) {
                disconnect(&old.handle, reason::DUPLICATE);
            }
        }
        let entity_id = self.next_entity_id;
        self.next_entity_id = self.next_entity_id.wrapping_add(1).max(1);
        let teleport_id = 1;
        let position = [
            SPAWN_XZ[0],
            f64::from(surface_y(
                self.generator,
                SPAWN_XZ[0].floor() as i64,
                SPAWN_XZ[1].floor() as i64,
            )),
            SPAWN_XZ[1],
        ];
        // Vanilla uses the smaller of the server's and the client's distance.
        let view_distance = match join.settings.view_distance {
            0 => self.config.view_distance,
            client => client.min(self.config.view_distance),
        };
        let spawn = Spawn {
            entity_id,
            game_mode: self.config.game_mode,
            position,
            yaw: 0.0,
            pitch: 0.0,
            teleport_id,
            view_distance,
            max_players: self.config.max_players as u32,
            online_mode: self.config.online_mode,
            day_time: self.day_time,
            world_age: self.tick,
        };
        let packets = match join_packets(&spawn, &self.manifest) {
            Ok(packets) => packets,
            Err(error) => {
                log::event("join_failed", &[("name", &profile.name), ("error", &error)]);
                handle.close();
                return;
            }
        };
        if !packets
            .into_iter()
            .all(|p| handle.send(Outgoing::Packet(p)))
        {
            handle.close();
            return;
        }
        log::event(
            "player_joined",
            &[
                ("name", &profile.name),
                ("uuid", &uuid_text(&profile.uuid)),
                ("peer", &handle.peer),
                ("view_distance", &view_distance),
            ],
        );
        self.players.insert(
            handle.id,
            Player {
                profile,
                handle,
                keepalive: KeepAliveTracker::default(),
                last_keepalive_tick: self.tick,
                awaiting_teleport: Some(teleport_id),
                position,
                view: View::new(chunk_of(position[0], position[2]), view_distance),
            },
        );
        self.online.store(self.players.len(), Ordering::Relaxed);
    }

    fn packet(&mut self, id: ConnectionId, packet: Packet) {
        let tick = self.tick;
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let body = packet.body.as_slice();
        let kick = match packet.id {
            serverbound::KEEP_ALIVE => match <[u8; 8]>::try_from(body) {
                Ok(bytes) => player
                    .keepalive
                    .handle_response(tick, i64::from_be_bytes(bytes))
                    .map(|_| ())
                    .map_err(|_| reason::TIMED_OUT),
                Err(_) => Err(reason::BAD_PACKET),
            },
            serverbound::ACCEPT_TELEPORTATION => match read_varint(body) {
                Ok(Some((teleport, n))) if n == body.len() => {
                    if player.awaiting_teleport == Some(teleport) {
                        player.awaiting_teleport = None;
                    }
                    Ok(())
                }
                _ => Err(reason::BAD_PACKET),
            },
            serverbound::CHUNK_BATCH_RECEIVED => match read_f32_be(body) {
                Ok((rate, rest)) if rest.is_empty() => {
                    player.view.batch_received(rate);
                    Ok(())
                }
                _ => Err(reason::BAD_PACKET),
            },
            serverbound::MOVE_PLAYER_POS | serverbound::MOVE_PLAYER_POS_ROT => {
                move_player(player, packet.id, body)
            }
            _ => Ok(()),
        };
        if let Err(text) = kick
            && let Some(player) = self.players.remove(&id)
        {
            disconnect(&player.handle, text);
            self.online.store(self.players.len(), Ordering::Relaxed);
        }
    }

    fn tick(&mut self) {
        self.tick += 1;
        self.day_time = (self.day_time + 1) % TICKS_PER_DAY;
        let tick = self.tick;
        let mut dropped = Vec::new();
        for (id, player) in &mut self.players {
            if player.keepalive.is_timed_out(tick) {
                disconnect(&player.handle, reason::TIMED_OUT);
                dropped.push(*id);
                continue;
            }
            let mut healthy = true;
            if !player.keepalive.has_pending_challenge()
                && tick - player.last_keepalive_tick >= KEEPALIVE_INTERVAL_TICKS
            {
                let mut challenge = [0u8; 8];
                healthy = random_bytes(&mut challenge).is_ok()
                    && player
                        .keepalive
                        .issue_challenge(tick, i64::from_be_bytes(challenge))
                        .is_ok()
                    && player.handle.send(Outgoing::Packet(Packet {
                        id: clientbound::KEEP_ALIVE,
                        body: challenge.to_vec(),
                    }));
                player.last_keepalive_tick = tick;
            }
            if healthy && let Some(batch) = player.view.next_batch(&mut self.store) {
                healthy = batch.into_iter().all(|out| player.handle.send(out));
            }
            if !healthy {
                disconnect(&player.handle, reason::SLOW);
                dropped.push(*id);
            }
        }
        for id in dropped {
            self.players.remove(&id);
        }
        self.online.store(self.players.len(), Ordering::Relaxed);
        if tick % UNLOAD_INTERVAL_TICKS == 0 {
            let players = &self.players;
            self.store
                .unload_unviewed(|position| players.values().any(|p| p.view.contains(position)));
        }
    }

    /// Players currently joined.
    pub fn player_count(&self) -> usize {
        self.players.len()
    }
}

/// Applies a position update and moves the player's view with it.
fn move_player(player: &mut Player, id: u32, body: &[u8]) -> Result<(), &'static str> {
    let position = match decode_serverbound_movement_packet(id, body) {
        Ok(ServerboundMovementPacket::Pos(p)) => [p.x, p.y, p.z],
        Ok(ServerboundMovementPacket::PosRot(p)) => [p.x, p.y, p.z],
        _ => return Err(reason::BAD_PACKET),
    };
    if player.awaiting_teleport.is_some() {
        // Moves sent before the client confirmed the last teleport are
        // stale; vanilla ignores them the same way.
        return Ok(());
    }
    player.position = position;
    for out in player.view.recenter(chunk_of(position[0], position[2])) {
        if !player.handle.send(out) {
            return Err(reason::SLOW);
        }
    }
    Ok(())
}

/// Sends a play-phase Disconnect and closes the connection.
pub fn disconnect(handle: &ConnectionHandle, text: &str) {
    let mut body = Vec::new();
    if put_nbt_text(text, &mut body).is_ok() {
        let _ = handle.send(Outgoing::Packet(Packet {
            id: clientbound::DISCONNECT,
            body,
        }));
    }
    handle.close();
}
