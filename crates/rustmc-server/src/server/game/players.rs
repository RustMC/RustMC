//! Players seeing each other: the tab list, player entities that appear and
//! disappear with each viewer's loaded chunks, and their movement.
//!
//! Why: multiplayer needs every client to know who is online and where the
//! players it can see are; the game loop is the only place that knows both.
//! Done: Player Info Update for every player (the joining one included) and
//! Player Info Remove on leaving; a player's entity is spawned for a viewer
//! while the viewer has the chunk it stands in and removed otherwise;
//! position, rotation and head rotation sent once per tick for players that
//! moved.
//! Not done: skins' second layer and other entity metadata, sneaking and
//! other poses, swing animations, latency updates.
//!
//! Sources: minecraft.wiki `Java Edition protocol/Packets` for 26.3, protocol
//! 777 (Player Info Update, Player Info Remove, Spawn Entity, Remove Entities,
//! Teleport Entity `entity_position_sync`, Set Head Rotation); packet ids are
//! checked against the official packet report in `java_packets` tests.

use std::sync::Arc;

use super::Game;
use crate::java_packets::play::clientbound;
use crate::java_wire::put_varint;
use crate::net::framing::Packet;
use crate::protocol::entity_spawn::{
    EntitySpawnAngles, RemoveEntitiesPacket, SpawnEntityPacket, angle_to_byte,
};
use crate::protocol::player_info::{
    GameMode as InfoGameMode, PlayerInfoAction, PlayerInfoEntry, PlayerInfoUpdatePacket,
    ProfileProperty,
};
use crate::server::connection::{ConnectionId, Outgoing};
use crate::server::world::view::chunk_of;

/// The registry name of the player entity type.
pub const PLAYER_ENTITY_TYPE: &str = "minecraft:player";

/// Player Info Update actions sent for a new player: added, game mode,
/// listed in the tab list and latency.
const JOIN_ACTIONS: u8 = PlayerInfoAction::ADD_PLAYER
    | PlayerInfoAction::UPDATE_GAME_MODE
    | PlayerInfoAction::UPDATE_LISTED
    | PlayerInfoAction::UPDATE_LATENCY;

/// Entity Position Sync path type for a plain position (wiki: 0 = Linear).
const PATH_LINEAR: u32 = 0;

impl Game {
    /// The tab-list entry for one player.
    fn info_entry(&self, id: ConnectionId) -> Option<PlayerInfoEntry> {
        let player = self.players.get(&id)?;
        let properties = player
            .profile
            .properties
            .iter()
            .map(|p| match &p.signature {
                Some(signature) => ProfileProperty::signed(&p.name, &p.value, signature),
                None => ProfileProperty::unsigned(&p.name, &p.value),
            })
            .collect();
        let game_mode = InfoGameMode::from_id(player.game_mode as i32).ok()?;
        Some(
            PlayerInfoEntry::new(player.profile.uuid, &player.profile.name)
                .with_properties(properties)
                .with_game_mode(game_mode)
                .with_listed(true)
                .with_latency(0),
        )
    }

    /// Announces a player who just joined: everyone learns about them, and
    /// they learn about everyone (themselves included).
    pub(super) fn announce(&mut self, id: ConnectionId) {
        let Some(entry) = self.info_entry(id) else {
            return;
        };
        let everyone: Vec<PlayerInfoEntry> = self
            .players
            .keys()
            .filter_map(|other| self.info_entry(*other))
            .collect();
        if let Some(packet) = info_packet(PlayerInfoUpdatePacket::new(JOIN_ACTIONS, everyone))
            && let Some(player) = self.players.get(&id)
        {
            player.handle.send(Outgoing::Packet(packet));
        }
        let Some(packet) = info_packet(PlayerInfoUpdatePacket::single(JOIN_ACTIONS, entry)) else {
            return;
        };
        let shared = Arc::new(packet);
        for (other, player) in &self.players {
            if *other != id {
                player.handle.send(Outgoing::Shared(Arc::clone(&shared)));
            }
        }
    }

    /// Tells the remaining players that `uuid` (entity `entity_id`) left.
    pub(super) fn depart(&mut self, uuid: [u8; 16], entity_id: i32) {
        let mut body = Vec::new();
        put_varint(1, &mut body);
        body.extend(uuid);
        let removed = Arc::new(Packet {
            id: clientbound::PLAYER_INFO_REMOVE,
            body,
        });
        for player in self.players.values_mut() {
            player.handle.send(Outgoing::Shared(Arc::clone(&removed)));
            if player.seen.remove(&entity_id) {
                player
                    .handle
                    .send(Outgoing::Packet(remove_entity(entity_id)));
            }
        }
    }

    /// Spawns and removes player entities so each viewer sees exactly the
    /// players standing in chunks it has, then sends this tick's movement.
    pub(super) fn track_players(&mut self) {
        let Some(entity_type) = self.player_entity_type else {
            return;
        };
        let snapshot: Vec<(ConnectionId, Snapshot)> = self
            .players
            .iter()
            .map(|(id, p)| {
                (
                    *id,
                    Snapshot {
                        entity_id: p.entity_id,
                        uuid: p.profile.uuid,
                        position: p.position,
                        yaw: p.yaw,
                        pitch: p.pitch,
                        on_ground: p.on_ground,
                        moved: p.moved,
                    },
                )
            })
            .collect();
        for (viewer_id, viewer) in &mut self.players {
            for (other_id, other) in &snapshot {
                if other_id == viewer_id {
                    continue;
                }
                let visible = viewer
                    .view
                    .has_sent(chunk_of(other.position[0], other.position[2]));
                let known = viewer.seen.contains(&other.entity_id);
                if visible && !known {
                    if let Some(packet) = spawn_packet(other, entity_type) {
                        viewer.handle.send(Outgoing::Packet(packet));
                        viewer.seen.insert(other.entity_id);
                    }
                } else if !visible && known {
                    viewer.seen.remove(&other.entity_id);
                    viewer
                        .handle
                        .send(Outgoing::Packet(remove_entity(other.entity_id)));
                } else if visible && other.moved {
                    for packet in movement_packets(other) {
                        viewer.handle.send(Outgoing::Packet(packet));
                    }
                }
            }
        }
        for player in self.players.values_mut() {
            player.moved = false;
        }
    }
}

/// What other players need to know about one player this tick.
struct Snapshot {
    entity_id: i32,
    uuid: [u8; 16],
    position: [f64; 3],
    yaw: f32,
    pitch: f32,
    on_ground: bool,
    moved: bool,
}

fn info_packet(packet: PlayerInfoUpdatePacket) -> Option<Packet> {
    Some(Packet {
        id: clientbound::PLAYER_INFO_UPDATE,
        body: packet.encode_payload().ok()?,
    })
}

fn remove_entity(entity_id: i32) -> Packet {
    let mut body = Vec::new();
    RemoveEntitiesPacket::single(entity_id).encode_payload(&mut body);
    Packet {
        id: clientbound::REMOVE_ENTITIES,
        body,
    }
}

fn spawn_packet(player: &Snapshot, entity_type: u32) -> Option<Packet> {
    let spawn = SpawnEntityPacket::with_angles(EntitySpawnAngles {
        entity_id: player.entity_id,
        uuid: player.uuid,
        entity_type: i32::try_from(entity_type).ok()?,
        position: player.position,
        pitch_deg: f64::from(player.pitch),
        yaw_deg: f64::from(player.yaw),
        head_yaw_deg: f64::from(player.yaw),
        data: 0,
    })
    .ok()?;
    let mut body = Vec::new();
    spawn.encode_payload(&mut body);
    Some(Packet {
        id: clientbound::ADD_ENTITY,
        body,
    })
}

fn movement_packets(player: &Snapshot) -> [Packet; 2] {
    let entity_id = player.entity_id as u32;
    let mut sync = Vec::new();
    put_varint(entity_id, &mut sync);
    put_varint(PATH_LINEAR, &mut sync);
    for axis in player.position {
        sync.extend(axis.to_be_bytes());
    }
    sync.extend(player.yaw.to_be_bytes());
    sync.extend(player.pitch.to_be_bytes());
    sync.push(u8::from(player.on_ground));
    let mut head = Vec::new();
    put_varint(entity_id, &mut head);
    head.push(angle_to_byte(f64::from(player.yaw)));
    [
        Packet {
            id: clientbound::ENTITY_POSITION_SYNC,
            body: sync,
        },
        Packet {
            id: clientbound::ROTATE_HEAD,
            body: head,
        },
    ]
}
