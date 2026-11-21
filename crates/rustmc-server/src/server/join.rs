//! The packets that put a configured player into the world: Login (play),
//! abilities, time, chunk-cache centre, the chunk-wait game event and the
//! first teleport.
//!
//! Why: the client shows the world only after this exact sequence; one typed
//! builder keeps every field named instead of scattered byte literals.
//! Done: survival and creative joins in the overworld. Not done: other
//! dimensions, death locations, hardcore.
//!
//! Field order follows the Login (play) layout the preview session has sent
//! to real 26.3 clients (`docs/PROVENANCE.md`), with names from minecraft.wiki
//! `Java Edition protocol/Packets` (Login (play), Player Abilities, Set Center
//! Chunk, Game Event, Synchronize Player Position).

use super::super::net::framing::Packet;
use crate::java_packets::play::clientbound;
use crate::java_wire::put_varint;
use crate::preview_data::RegistryManifest;

/// The only dimension the playable server runs.
pub const OVERWORLD: &str = "minecraft:overworld";
/// Overworld sea level (`sea_level` of `minecraft:overworld` noise settings).
pub const SEA_LEVEL: u32 = 63;
/// Game Event id "Start waiting for chunks", required after every Login (play).
pub const GAME_EVENT_START_WAITING_FOR_CHUNKS: u8 = 13;
/// Default flying speed in Player Abilities ("0.05 by default").
pub const DEFAULT_FLYING_SPEED: f32 = 0.05;
/// Field-of-view modifier in Player Abilities: vanilla sends the player's
/// movement speed attribute, whose default is 0.1.
pub const DEFAULT_FOV_MODIFIER: f32 = 0.1;

/// Player Abilities flag bits.
pub mod ability {
    pub const INVULNERABLE: u8 = 0x01;
    pub const FLYING: u8 = 0x02;
    pub const ALLOW_FLYING: u8 = 0x04;
    pub const INSTANT_BREAK: u8 = 0x08;
}

/// Game modes in their protocol order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Survival = 0,
    Creative = 1,
    Adventure = 2,
    Spectator = 3,
}

impl GameMode {
    /// The Player Abilities flags this mode grants.
    pub fn ability_flags(self) -> u8 {
        match self {
            Self::Survival | Self::Adventure => 0,
            Self::Creative => {
                ability::INVULNERABLE | ability::ALLOW_FLYING | ability::INSTANT_BREAK
            }
            Self::Spectator => ability::INVULNERABLE | ability::FLYING | ability::ALLOW_FLYING,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "survival" => Some(Self::Survival),
            "creative" => Some(Self::Creative),
            "adventure" => Some(Self::Adventure),
            "spectator" => Some(Self::Spectator),
            _ => None,
        }
    }
}

/// Where and how a player enters the world.
#[derive(Debug, Clone, PartialEq)]
pub struct Spawn {
    pub entity_id: i32,
    pub game_mode: GameMode,
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub teleport_id: u32,
    pub view_distance: u8,
    pub max_players: u32,
    /// Whether the server verified this player with the session server.
    pub online_mode: bool,
    /// Ticks into the day for Set Time.
    pub day_time: u64,
    pub world_age: u64,
}

/// Why the join packets could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    /// The registry manifest lacks an entry the client needs by id.
    MissingRegistryEntry(&'static str, &'static str),
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingRegistryEntry(registry, entry) => {
                write!(f, "registry manifest has no {entry} in {registry}")
            }
        }
    }
}

impl std::error::Error for JoinError {}

/// The network id of `entry` in `registry`: its position in the manifest,
/// which is the order the configuration phase sent.
pub fn registry_id(
    manifest: &RegistryManifest,
    registry: &'static str,
    entry: &'static str,
) -> Result<u32, JoinError> {
    manifest
        .registries
        .iter()
        .find(|(key, _)| key == registry)
        .and_then(|(_, entries)| entries.iter().position(|key| key == entry))
        .map(|id| id as u32)
        .ok_or(JoinError::MissingRegistryEntry(registry, entry))
}

fn put_identifier(text: &str, out: &mut Vec<u8>) {
    crate::java_wire::put_string(text, out);
}

/// Login (play).
pub fn login(spawn: &Spawn, manifest: &RegistryManifest) -> Result<Packet, JoinError> {
    let mut body = spawn.entity_id.to_be_bytes().to_vec();
    body.push(0); // is hardcore
    put_varint(1, &mut body); // dimension names
    put_identifier(OVERWORLD, &mut body);
    put_varint(spawn.max_players, &mut body);
    put_varint(u32::from(spawn.view_distance), &mut body);
    put_varint(u32::from(spawn.view_distance), &mut body); // simulation distance
    body.push(0); // reduced debug info
    body.push(1); // enable respawn screen
    body.push(0); // do limited crafting
    put_varint(
        registry_id(manifest, "minecraft:dimension_type", OVERWORLD)?,
        &mut body,
    );
    put_identifier(OVERWORLD, &mut body);
    body.extend(0i64.to_be_bytes()); // hashed seed (not shared with clients)
    put_varint(spawn.game_mode as u32, &mut body);
    put_varint(0, &mut body); // previous game mode: undefined
    body.push(0); // is debug
    body.push(0); // is flat
    body.push(0); // has death location
    put_varint(0, &mut body); // portal cooldown
    put_varint(SEA_LEVEL, &mut body);
    body.push(u8::from(spawn.online_mode));
    body.push(0); // enforces secure chat: chat is not signed yet
    Ok(Packet {
        id: clientbound::LOGIN,
        body,
    })
}

/// Player Abilities for the player's game mode.
pub fn abilities(game_mode: GameMode) -> Packet {
    let mut body = vec![game_mode.ability_flags()];
    body.extend(DEFAULT_FLYING_SPEED.to_be_bytes());
    body.extend(DEFAULT_FOV_MODIFIER.to_be_bytes());
    Packet {
        id: clientbound::PLAYER_ABILITIES,
        body,
    }
}

/// Clock ticks per client tick for a clock that runs at normal speed.
pub const CLOCK_RATE_NORMAL: f32 = 1.0;

/// Set Time with the overworld clock running at normal speed.
pub fn time(spawn: &Spawn, manifest: &RegistryManifest) -> Result<Packet, JoinError> {
    let mut body = (spawn.world_age as i64).to_be_bytes().to_vec();
    put_varint(1, &mut body); // one clock
    put_varint(
        registry_id(manifest, "minecraft:world_clock", OVERWORLD)?,
        &mut body,
    );
    crate::java_wire::put_varlong(spawn.day_time, &mut body);
    body.extend(0f32.to_be_bytes()); // fractional time
    body.extend(CLOCK_RATE_NORMAL.to_be_bytes());
    Ok(Packet {
        id: clientbound::SET_TIME,
        body,
    })
}

/// Set Center Chunk.
pub fn chunk_cache_center(chunk_x: i32, chunk_z: i32) -> Packet {
    let mut body = Vec::new();
    put_varint(chunk_x as u32, &mut body);
    put_varint(chunk_z as u32, &mut body);
    Packet {
        id: clientbound::SET_CHUNK_CACHE_CENTER,
        body,
    }
}

/// Synchronize Player Position with absolute coordinates.
pub fn teleport(teleport_id: u32, position: [f64; 3], yaw: f32, pitch: f32) -> Packet {
    let mut body = Vec::new();
    put_varint(teleport_id, &mut body);
    for value in position {
        body.extend(value.to_be_bytes());
    }
    for velocity in [0f64; 3] {
        body.extend(velocity.to_be_bytes());
    }
    body.extend(yaw.to_be_bytes());
    body.extend(pitch.to_be_bytes());
    body.extend(0u32.to_be_bytes()); // every field absolute
    Packet {
        id: clientbound::PLAYER_POSITION,
        body,
    }
}

/// The whole join sequence, in the order the client expects it.
pub fn join_packets(spawn: &Spawn, manifest: &RegistryManifest) -> Result<Vec<Packet>, JoinError> {
    let chunk_x = (spawn.position[0].floor() as i32).div_euclid(16);
    let chunk_z = (spawn.position[2].floor() as i32).div_euclid(16);
    let mut wait = vec![GAME_EVENT_START_WAITING_FOR_CHUNKS];
    wait.extend(0f32.to_be_bytes());
    Ok(vec![
        login(spawn, manifest)?,
        abilities(spawn.game_mode),
        time(spawn, manifest)?,
        chunk_cache_center(chunk_x, chunk_z),
        Packet {
            id: clientbound::GAME_EVENT,
            body: wait,
        },
        teleport(spawn.teleport_id, spawn.position, spawn.yaw, spawn.pitch),
    ])
}

#[cfg(test)]
mod tests;
