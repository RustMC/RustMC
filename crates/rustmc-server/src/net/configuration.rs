//! The configuration phase: known packs, registry data, enabled features and
//! tags that every client receives between login and play.
//!
//! Why: the preview session and the playable server send the same
//! configuration; one builder keeps them identical.
//! Done: known-pack negotiation against the `minecraft:core` pack, registry
//! entries by identifier (definitions come from the negotiated core pack),
//! tags, finish. Not done: custom data packs (only the core pack is offered).
//!
//! Source: the official 26.3 packet report for ids; field layouts as recorded
//! in `docs/PROVENANCE.md` (known-pack and registry-data rows).

use super::framing::Packet;
use crate::discovery_java::VERSION;
use crate::java_packets::configuration::clientbound;
use crate::java_wire::{WireError, put_string, put_varint, read_string, read_varint};
use crate::preview_data::RegistryManifest;

/// Most known packs a client may list; the vanilla client lists one.
pub const MAX_KNOWN_PACKS: u32 = 16;
/// Longest namespace, id or version string in a known-pack entry.
pub const MAX_PACK_STRING: usize = 255;

/// Why a client's known-pack list was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackListError {
    TooMany,
    Malformed,
}

impl From<WireError> for PackListError {
    fn from(_: WireError) -> Self {
        Self::Malformed
    }
}

/// Select Known Packs offering only the vanilla core pack of this version.
pub fn known_packs() -> Packet {
    let mut body = Vec::new();
    put_varint(1, &mut body);
    for text in ["minecraft", "core", VERSION] {
        put_string(text, &mut body);
    }
    Packet {
        id: clientbound::SELECT_KNOWN_PACKS,
        body,
    }
}

/// Whether the client's known-pack reply includes this version's core pack.
pub fn recognizes_core_pack(body: &[u8]) -> Result<bool, PackListError> {
    let Some((count, prefix)) = read_varint(body)? else {
        return Err(PackListError::Malformed);
    };
    if count > MAX_KNOWN_PACKS {
        return Err(PackListError::TooMany);
    }
    let mut remaining = &body[prefix..];
    let mut recognized = false;
    for _ in 0..count {
        let (namespace, rest) = read_string(remaining, MAX_PACK_STRING)?;
        let (id, rest) = read_string(rest, MAX_PACK_STRING)?;
        let (version, rest) = read_string(rest, MAX_PACK_STRING)?;
        recognized |= namespace == "minecraft" && id == "core" && version == VERSION;
        remaining = rest;
    }
    if !remaining.is_empty() {
        return Err(PackListError::Malformed);
    }
    Ok(recognized)
}

fn registry_data(registry: &str, entries: &[String]) -> Packet {
    let mut body = Vec::new();
    put_string(registry, &mut body);
    put_varint(entries.len() as u32, &mut body);
    for entry in entries {
        put_string(entry, &mut body);
        // No inline definition: the entry comes from the negotiated core pack.
        body.push(0);
    }
    Packet {
        id: clientbound::REGISTRY_DATA,
        body,
    }
}

/// Everything sent after the known-pack reply, ending with Finish
/// Configuration.
pub fn initial_configuration(manifest: Option<&RegistryManifest>) -> Vec<Packet> {
    let mut features = Vec::new();
    put_varint(1, &mut features);
    put_string("minecraft:vanilla", &mut features);
    let mut packets = vec![Packet {
        id: clientbound::UPDATE_ENABLED_FEATURES,
        body: features,
    }];
    if let Some(manifest) = manifest {
        for (registry, entries) in &manifest.registries {
            packets.push(registry_data(registry, entries));
        }
    }
    let mut tags = Vec::new();
    put_varint(manifest.map_or(0, |m| m.tags.len()) as u32, &mut tags);
    if let Some(manifest) = manifest {
        for registry in &manifest.tags {
            put_string(&registry.registry, &mut tags);
            put_varint(registry.tags.len() as u32, &mut tags);
            for (name, ids) in &registry.tags {
                put_string(name, &mut tags);
                put_varint(ids.len() as u32, &mut tags);
                for id in ids {
                    put_varint(*id, &mut tags);
                }
            }
        }
    }
    packets.push(Packet {
        id: clientbound::UPDATE_TAGS,
        body: tags,
    });
    packets.push(Packet {
        id: clientbound::FINISH_CONFIGURATION,
        body: Vec::new(),
    });
    packets
}

#[cfg(test)]
mod tests;
