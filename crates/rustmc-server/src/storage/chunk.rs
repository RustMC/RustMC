//! A world column as an Anvil chunk: the NBT layout 26.3 saves use.
//!
//! Why: chunks must survive restarts, in a format vanilla tools can read.
//! Done: block-state and biome palettes per section with packed long arrays,
//! the column position, data version and status. Not done: block entities,
//! scheduled ticks, light and heightmaps (left for the game to recompute, as
//! `isLightOn = 0` tells vanilla); loading by vanilla itself is not verified.
//!
//! Facts observed in a 26.3 vanilla save (recorded in docs/PROVENANCE.md):
//! chunks carry `DataVersion` 5023, `xPos`, `zPos`, `yPos`, `Status` and a
//! `sections` list whose entries hold `Y`, `block_states` (a `palette` of
//! bare names, or of `{id, properties}` compounds with property-less states
//! wrapped as `{"": name}`, and `data`) and `biomes` (`palette` of names and `data`). Packed values never
//! span two longs.

use std::collections::BTreeMap;
use std::fmt;

use super::nbt::{Tag, compound};
use crate::chunk_adapter::{
    OVERWORLD_MIN_Y, RegistryTables, SECTION_BIOME_CELLS, SECTION_CELLS, SECTION_HEIGHT, SECTIONS,
    ceillog2,
};
use crate::server::world::column::{Column, SectionCells};

/// `DataVersion` of Java Edition 26.3 chunks.
pub const DATA_VERSION: i32 = 5023;
/// The generation status of a finished chunk.
pub const STATUS_FULL: &str = "minecraft:full";
/// Fewest bits a block palette index uses (vanilla's minimum for sections).
pub const MIN_BLOCK_BITS: usize = 4;
/// Section Y of the lowest section (`-64 / 16`).
const MIN_SECTION_Y: i32 = OVERWORLD_MIN_Y / SECTION_HEIGHT as i32;

/// Why a stored chunk cannot be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkError {
    /// A state or biome id is not in the registry table.
    UnknownId(u32),
    /// A saved block state or biome is not in the registry table.
    UnknownName(String),
    /// A required field is missing or has the wrong type.
    Malformed(&'static str),
}

impl fmt::Display for ChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownId(id) => write!(f, "id {id} is not in the registry table"),
            Self::UnknownName(name) => write!(f, "{name} is not in the registry table"),
            Self::Malformed(field) => write!(f, "chunk field {field} is missing or invalid"),
        }
    }
}

impl std::error::Error for ChunkError {}

/// Packs `indices` at `bits` each, low bits first, without spanning longs.
fn pack(indices: &[usize], bits: usize) -> Vec<i64> {
    let per_long = 64 / bits;
    indices
        .chunks(per_long)
        .map(|chunk| {
            chunk.iter().enumerate().fold(0u64, |word, (i, index)| {
                word | ((*index as u64) << (i * bits))
            }) as i64
        })
        .collect()
}

/// Unpacks `count` indices of `bits` each.
fn unpack(data: &[i64], bits: usize, count: usize) -> Option<Vec<usize>> {
    let per_long = 64 / bits;
    if data.len() != count.div_ceil(per_long) {
        return None;
    }
    let mask = (1u64 << bits) - 1;
    Some(
        (0..count)
            .map(|i| ((data[i / per_long] as u64 >> ((i % per_long) * bits)) & mask) as usize)
            .collect(),
    )
}

/// The palette and index of every cell, in first-seen order.
fn palette_of(values: &[u32]) -> (Vec<u32>, Vec<usize>) {
    let mut palette = Vec::new();
    let mut positions = BTreeMap::new();
    let indices = values
        .iter()
        .map(|value| {
            *positions.entry(*value).or_insert_with(|| {
                palette.push(*value);
                palette.len() - 1
            })
        })
        .collect();
    (palette, indices)
}

/// Splits `minecraft:name[a=b,c=d]` into the name and its properties.
fn split_key(key: &str) -> (&str, BTreeMap<String, Tag>) {
    let (name, properties) = match key.split_once('[') {
        Some((name, rest)) => (name, rest.trim_end_matches(']')),
        None => (key, ""),
    };
    let map = properties
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_owned(), Tag::String(v.to_owned())))
        .collect();
    (name, map)
}

/// A block palette in the 26.3 form: bare names when no state has
/// properties; otherwise compounds, `{id, properties}` for states with
/// properties and `{"": name}` for the others (an NBT list holds one type).
fn block_palette(keys: &[&str]) -> Tag {
    let split: Vec<_> = keys.iter().map(|key| split_key(key)).collect();
    if split.iter().all(|(_, properties)| properties.is_empty()) {
        return Tag::List(
            split
                .into_iter()
                .map(|(name, _)| Tag::String(name.to_owned()))
                .collect(),
        );
    }
    Tag::List(
        split
            .into_iter()
            .map(|(name, properties)| {
                if properties.is_empty() {
                    compound(&[("", Tag::String(name.to_owned()))])
                } else {
                    compound(&[
                        ("id", Tag::String(name.to_owned())),
                        ("properties", Tag::Compound(properties)),
                    ])
                }
            })
            .collect(),
    )
}

/// A palette entry back to its full state key: a bare name, `{"": name}`,
/// `{id, properties}`, or the older `{Name, Properties}`.
fn entry_key(entry: &Tag) -> Option<String> {
    if let Some(name) = entry
        .as_str()
        .or_else(|| entry.get("").and_then(Tag::as_str))
    {
        return Some(name.to_owned());
    }
    let (name, properties) = match (entry.get("id"), entry.get("Name")) {
        (Some(id), _) => (id.as_str()?, entry.get("properties")),
        (None, Some(name)) => (name.as_str()?, entry.get("Properties")),
        (None, None) => return None,
    };
    let Some(Tag::Compound(properties)) = properties else {
        return Some(name.to_owned());
    };
    let pairs: Vec<String> = properties
        .iter()
        .map(|(k, v)| v.as_str().map(|v| format!("{k}={v}")))
        .collect::<Option<_>>()?;
    Some(format!("{name}[{}]", pairs.join(",")))
}

fn container(palette: Tag, indices: &[usize], bits: usize) -> Tag {
    let mut pairs = vec![("palette", palette)];
    if bits > 0 {
        pairs.push(("data", Tag::LongArray(pack(indices, bits))));
    }
    compound(&pairs)
}
/// Encodes a column as a chunk compound.
pub fn encode(column: &Column, tables: &RegistryTables, game_time: i64) -> Result<Tag, ChunkError> {
    let biomes = column.biomes();
    let mut sections = Vec::with_capacity(SECTIONS);
    for index in 0..SECTIONS {
        let cells: Vec<u32> = match column.section(index) {
            Some(SectionCells::Uniform(state)) => vec![state],
            Some(SectionCells::Mixed(cells)) => cells.to_vec(),
            None => return Err(ChunkError::Malformed("sections")),
        };
        let (palette, indices) = palette_of(&cells);
        let keys = palette
            .iter()
            .map(|id| tables.state_name(*id).ok_or(ChunkError::UnknownId(*id)))
            .collect::<Result<Vec<_>, _>>()?;
        let bits = if palette.len() == 1 {
            0
        } else {
            ceillog2(palette.len() as u32).max(MIN_BLOCK_BITS)
        };
        let blocks = container(block_palette(&keys), &indices, bits);
        let layer = &biomes[index * SECTION_BIOME_CELLS..(index + 1) * SECTION_BIOME_CELLS];
        let (biome_palette, biome_indices) = palette_of(layer);
        let biome_entries = Tag::List(
            biome_palette
                .iter()
                .map(|id| {
                    tables
                        .biome_name(*id)
                        .map(|name| Tag::String(name.to_owned()))
                        .ok_or(ChunkError::UnknownId(*id))
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        let biome_bits = ceillog2(biome_palette.len() as u32);
        sections.push(compound(&[
            ("Y", Tag::Byte((MIN_SECTION_Y + index as i32) as i8)),
            ("block_states", blocks),
            (
                "biomes",
                container(biome_entries, &biome_indices, biome_bits),
            ),
        ]));
    }
    Ok(compound(&[
        ("DataVersion", Tag::Int(DATA_VERSION)),
        ("xPos", Tag::Int(column.x)),
        ("zPos", Tag::Int(column.z)),
        ("yPos", Tag::Int(MIN_SECTION_Y)),
        ("Status", Tag::String(STATUS_FULL.to_owned())),
        ("LastUpdate", Tag::Long(game_time)),
        ("InhabitedTime", Tag::Long(0)),
        ("isLightOn", Tag::Byte(0)),
        ("sections", Tag::List(sections)),
    ]))
}

fn read_container<T>(
    tag: &Tag,
    count: usize,
    min_bits: usize,
    resolve: impl Fn(&Tag) -> Result<T, ChunkError>,
) -> Result<Vec<T>, ChunkError>
where
    T: Copy,
{
    let palette = tag
        .get("palette")
        .and_then(Tag::as_list)
        .ok_or(ChunkError::Malformed("palette"))?
        .iter()
        .map(&resolve)
        .collect::<Result<Vec<T>, _>>()?;
    let first = *palette.first().ok_or(ChunkError::Malformed("palette"))?;
    if palette.len() == 1 {
        return Ok(vec![first; count]);
    }
    let bits = ceillog2(palette.len() as u32).max(min_bits);
    let data = tag
        .get("data")
        .and_then(Tag::as_long_array)
        .ok_or(ChunkError::Malformed("data"))?;
    unpack(data, bits, count)
        .ok_or(ChunkError::Malformed("data"))?
        .into_iter()
        .map(|index| {
            palette
                .get(index)
                .copied()
                .ok_or(ChunkError::Malformed("data"))
        })
        .collect()
}

/// Decodes a chunk compound into a column.
pub fn decode(tag: &Tag, tables: &RegistryTables) -> Result<Column, ChunkError> {
    let x = tag
        .get("xPos")
        .and_then(Tag::as_i32)
        .ok_or(ChunkError::Malformed("xPos"))?;
    let z = tag
        .get("zPos")
        .and_then(Tag::as_i32)
        .ok_or(ChunkError::Malformed("zPos"))?;
    let mut column = Column::filled(x, z, tables.air_state(), 0);
    let mut biomes = column.biomes().to_vec();
    let sections = tag
        .get("sections")
        .and_then(Tag::as_list)
        .ok_or(ChunkError::Malformed("sections"))?;
    for section in sections {
        let y = match section.get("Y") {
            Some(Tag::Byte(y)) => i32::from(*y),
            _ => return Err(ChunkError::Malformed("Y")),
        };
        let Ok(index) = usize::try_from(y - MIN_SECTION_Y) else {
            continue;
        };
        if index >= SECTIONS {
            continue;
        }
        if let Some(states) = section.get("block_states") {
            let cells = read_container(states, SECTION_CELLS, MIN_BLOCK_BITS, |entry| {
                let key = entry_key(entry).ok_or(ChunkError::Malformed("block_states"))?;
                tables
                    .state(&key)
                    .map(|state| state.id)
                    .map_err(|_| ChunkError::UnknownName(key))
            })?;
            column.set_section(index, cells);
        }
        if let Some(layer) = section.get("biomes") {
            let cells = read_container(layer, SECTION_BIOME_CELLS, 0, |entry| {
                let name = entry.as_str().ok_or(ChunkError::Malformed("biomes"))?;
                tables
                    .biome(name)
                    .map_err(|_| ChunkError::UnknownName(name.to_owned()))
            })?;
            let start = index * SECTION_BIOME_CELLS;
            biomes[start..start + SECTION_BIOME_CELLS].copy_from_slice(&cells);
        }
    }
    column.set_biomes(biomes);
    Ok(column)
}

#[cfg(test)]
mod tests;
