//! Java 26.3 chunk adapter for complete vanilla Overworld columns.
//!
//! This is the wire half of vanilla terrain: it turns the generator's column
//! data (block-state names from the material-rule descent plus biome
//! identifiers) into the packet body a 26.3 client accepts, using the same
//! field order, bit widths, and palette rules as the synthetic preview path
//! (`crate::java_preview`), whose packet-ID and field-order provenance is
//! logged in `docs/PROVENANCE.md`. The container, heightmap, and light facts
//! this module adds on top are stated at each write site and are logged as
//! `docs/PROVENANCE.md` Session 11, which also records the registry-count fact
//! that was removed from a tracked comment rather than left in the code. Nothing
//! was copied: only shapes were consulted, and every consulted number is
//! re-derived here from the operator-provisioned tables.
//!
//! Pipeline:
//!
//! ```text
//! VanillaGenerator --chunk_from_generator--> VanillaChunk --encode_chunk--> Vec<u8> frame
//!                         (registry IDs)                    (packet 46 body)
//! ```
//!
//! Wire facts this module depends on (`protocol_id` of heightmap types,
//! paletted-container byte layout, section field order, light masks, fixed-size
//! long arrays) are documented at each write site.
//!
//! # Deferred
//!
//! * Client observation: the tests below decode the encoded bytes back off the
//!   wire, so they pin self-consistency and the preview's accepted shape. A
//!   26.3 client has not yet been shown an adapter-built real column.
//! * Block entities: the list is always empty (VarInt 0). The vanilla
//!   generator's decorations are not yet entities.
//! * Border blocks: section containers hold only this chunk's 16x16 columns;
//!   no neighbour-chunk block or biome data is transferred here.
//! * Block light: no emitting states are modelled, so the block-light update
//!   list is empty and every block section is declared empty.

pub mod registry;

use std::collections::BTreeSet;

use crate::discovery_java::{frame, put_varint};
use crate::vanilla::generator::VanillaGenerator;

pub use registry::{
    HeightmapKind, RegistryError, RegistryTables, SUPPORTED_PROTOCOL, SUPPORTED_VERSION,
    StateEntry, StateKind, canonical_state_key, ceillog2,
};

/// Protocol-777 id of `ClientboundLevelChunkWithLightPacket`.
pub const CHUNK_PACKET: u32 = 46;
/// Blocks per horizontal chunk side.
pub const CHUNK_SIDE: usize = 16;
/// Vertical size of one chunk section in blocks.
pub const SECTION_HEIGHT: usize = 16;
/// Vertical block span of the Overworld.
pub const OVERWORLD_HEIGHT: usize = 384;
/// Absolute Y of the lowest Overworld buildable row.
pub const OVERWORLD_MIN_Y: i32 = -64;
/// Sections in one complete Overworld column (`384 / 16`).
pub const SECTIONS: usize = OVERWORLD_HEIGHT / SECTION_HEIGHT;
/// Block cells in one section.
pub const SECTION_CELLS: usize = CHUNK_SIDE * CHUNK_SIDE * SECTION_HEIGHT;
/// Block cells in one whole-column chunk.
pub const CHUNK_CELLS: usize = CHUNK_SIDE * CHUNK_SIDE * OVERWORLD_HEIGHT;
/// Biome cells per horizontal section slice.
pub const SECTION_BIOME_SIDE: usize = 4;
/// Biome cells in one layer of the 4x4x4 grid (`4 * 4`).
pub const BIOME_LAYER_CELLS: usize = SECTION_BIOME_SIDE * SECTION_BIOME_SIDE;
/// Biome cells in one section (`4 * 4 * 4`).
pub const SECTION_BIOME_CELLS: usize = BIOME_LAYER_CELLS * SECTION_BIOME_SIDE;
/// 4-block biome layers in one whole-column chunk (`384 / 4`).
pub const BIOME_LAYERS: usize = OVERWORLD_HEIGHT / SECTION_BIOME_SIDE;
/// Biome cells in one whole-column chunk.
pub const CHUNK_BIOMES: usize = SECTION_BIOME_SIDE * SECTION_BIOME_SIDE * BIOME_LAYERS;
/// Light layers: every section plus the one above and the one below.
pub const LIGHT_SECTIONS: usize = SECTIONS + 2;
/// Bytes in one light layer (`4096` nibbles).
pub const LIGHT_BYTES: usize = SECTION_CELLS / 2;
/// Heightmap entries in a chunk (one per 16x16 column).
pub const HEIGHTMAP_COLUMNS: usize = CHUNK_SIDE * CHUNK_SIDE;
/// Bits per heightmap entry: `Mth.ceillog2(height)` for a 384-tall dimension.
pub const HEIGHTMAP_BITS: usize = 9;
/// Heightmap types a 26.3 client expects: world surface and both motion types.
pub const HEIGHTMAP_TYPES: usize = 3;
/// Indirect block palettes start at 4 bits and stop at 8 (`Strategy$1`).
pub const BLOCK_PALETTE_MIN_BITS: usize = 4;
/// Beyond this the block container switches to the global (direct) palette.
pub const BLOCK_PALETTE_MAX_BITS: usize = 8;
/// Indirect biome palettes start at 1 bit and stop at 3 (`Strategy$2`).
pub const BIOME_PALETTE_MIN_BITS: usize = 1;
/// Beyond this the biome container switches to the global (direct) palette.
pub const BIOME_PALETTE_MAX_BITS: usize = 3;
/// Per-batch chunk budget inherited from the accepted preview path; one chunk
/// packet must fit inside it on its own.
pub const MAX_CHUNK_PACKET_BYTES: usize = 768 * 1024;

/// Everything that can stop a column from becoming a packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// A block-state or biome name could not be resolved, or the provisioned
    /// table is not valid for 26.3.
    Registry(RegistryError),
    /// The adapter speaks the Overworld profile only: `-64` plus `384` rows.
    UnsupportedMinY {
        /// Minimum build Y reported by the generator's dimension.
        min_y: i32,
    },
    /// The generator handed back a column of an unexpected height.
    WrongColumnLength {
        /// Rows expected for one 16x16 column.
        expected: usize,
        /// Rows actually present.
        got: usize,
    },
    /// The state array is not a complete 16x16x384 column.
    WrongStateCount {
        /// Cells expected for one chunk.
        expected: usize,
        /// Cells actually present.
        got: usize,
    },
    /// The biome array is not a complete 4x4x96 grid.
    WrongBiomeCount {
        /// Biome cells expected for one chunk.
        expected: usize,
        /// Biome cells actually present.
        got: usize,
    },
    /// A queried position is outside this chunk's column.
    CellOutOfRange {
        /// Chunk-relative X.
        x: usize,
        /// Absolute Y.
        y: i32,
        /// Chunk-relative Z.
        z: usize,
    },
    /// A chunk coordinate cannot be represented as an absolute block origin.
    ChunkCoordinateOverflow {
        /// Chunk X from the requested column.
        chunk_x: i32,
        /// Chunk Z from the requested column.
        chunk_z: i32,
    },
    /// A state id is not inside the versioned block-state registry.
    StateOutOfRange {
        /// Offending id.
        id: u32,
        /// Declared registry size.
        count: u32,
    },
    /// A state id is not one the provisioned table describes, so its
    /// heightmap and light behaviour cannot be known.
    UnregisteredState {
        /// Offending id.
        id: u32,
    },
    /// A biome id is not inside the versioned biome registry.
    BiomeOutOfRange {
        /// Offending id.
        id: u32,
        /// Declared registry size.
        count: u32,
    },
    /// The generator has no biome placement table for its preset, so no biome
    /// can be named; nothing is invented to fill the column.
    MissingBiomePlacement,
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Registry(error) => write!(f, "registry: {error}"),
            Self::UnsupportedMinY { min_y } => write!(
                f,
                "only the Overworld profile is supported: expected min_y {OVERWORLD_MIN_Y}, got {min_y}"
            ),
            Self::WrongColumnLength { expected, got } => {
                write!(f, "generator column has {got} rows, expected {expected}")
            }
            Self::WrongStateCount { expected, got } => {
                write!(f, "chunk has {got} block cells, expected {expected}")
            }
            Self::WrongBiomeCount { expected, got } => {
                write!(f, "chunk has {got} biome cells, expected {expected}")
            }
            Self::CellOutOfRange { x, y, z } => {
                write!(f, "position {x},{y},{z} is outside the chunk column")
            }
            Self::ChunkCoordinateOverflow { chunk_x, chunk_z } => write!(
                f,
                "chunk {chunk_x},{chunk_z} cannot be addressed with i32 block coordinates"
            ),
            Self::StateOutOfRange { id, count } => {
                write!(f, "block state id {id} is outside 0..{count}")
            }
            Self::UnregisteredState { id } => write!(
                f,
                "block state id {id} is not described by the provisioned table (no heightmap or light class)"
            ),
            Self::BiomeOutOfRange { id, count } => {
                write!(f, "biome id {id} is outside 0..{count}")
            }
            Self::MissingBiomePlacement => write!(
                f,
                "the generator has no biome placement table, so no biome id can be resolved"
            ),
        }
    }
}

impl std::error::Error for EncodeError {}

impl From<RegistryError> for EncodeError {
    fn from(error: RegistryError) -> Self {
        Self::Registry(error)
    }
}

/// Flat index of one block cell inside a whole-column state array: the section
/// layout a 26.3 client reads (`x + z * 16 + localY * 256`), stacked over the
/// dimension's sections.
pub const fn state_index(x: usize, y: i32, z: usize) -> usize {
    x + z * CHUNK_SIDE + (y - OVERWORLD_MIN_Y) as usize * CHUNK_SIDE * CHUNK_SIDE
}

/// Flat index of one biome cell inside a whole-column biome array: the 4x4x4
/// cell layout a 26.3 client reads (`x + z * 4 + localY * 16`), stacked over
/// the dimension's biome layers. `by` is an absolute Y; the layer is the
/// 4-block cell containing it.
pub const fn biome_index(bx: usize, by: i32, bz: usize) -> usize {
    bx + bz * SECTION_BIOME_SIDE
        + (by - OVERWORLD_MIN_Y) as usize / SECTION_BIOME_SIDE * BIOME_LAYER_CELLS
}

/// One complete `-64..319` Overworld column pair, already resolved to
/// protocol-777 registry ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VanillaChunk {
    chunk_x: i32,
    chunk_z: i32,
    states: Vec<u32>,
    biomes: Vec<u32>,
}

impl VanillaChunk {
    /// Wraps a complete column. Partial columns are rejected: the adapter only
    /// speaks full `-64..319` Overworld chunks, so a truncated array is a bug
    /// rather than something to paper over.
    pub fn new(
        chunk_x: i32,
        chunk_z: i32,
        states: Vec<u32>,
        biomes: Vec<u32>,
    ) -> Result<Self, EncodeError> {
        if states.len() != CHUNK_CELLS {
            return Err(EncodeError::WrongStateCount {
                expected: CHUNK_CELLS,
                got: states.len(),
            });
        }
        if biomes.len() != CHUNK_BIOMES {
            return Err(EncodeError::WrongBiomeCount {
                expected: CHUNK_BIOMES,
                got: biomes.len(),
            });
        }
        Ok(Self {
            chunk_x,
            chunk_z,
            states,
            biomes,
        })
    }

    /// Chunk coordinates as the client sees them.
    pub const fn coordinates(&self) -> (i32, i32) {
        (self.chunk_x, self.chunk_z)
    }

    /// Every block cell, in [`state_index`] order.
    pub fn states(&self) -> &[u32] {
        &self.states
    }

    /// Every biome cell, in [`biome_index`] order.
    pub fn biomes(&self) -> &[u32] {
        &self.biomes
    }

    /// Protocol id at one absolute position.
    pub fn state_at(&self, x: usize, y: i32, z: usize) -> Option<u32> {
        if x >= CHUNK_SIDE || z >= CHUNK_SIDE || y < OVERWORLD_MIN_Y {
            return None;
        }
        self.states.get(state_index(x, y, z)).copied()
    }

    /// Protocol id at an absolute world block position. The caller cannot
    /// accidentally read this chunk using another chunk's local coordinates.
    pub fn state_at_world(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if x.div_euclid(CHUNK_SIDE as i32) != self.chunk_x
            || z.div_euclid(CHUNK_SIDE as i32) != self.chunk_z
        {
            return None;
        }
        self.state_at(
            x.rem_euclid(CHUNK_SIDE as i32) as usize,
            y,
            z.rem_euclid(CHUNK_SIDE as i32) as usize,
        )
    }

    /// Biome id of the 4x4x4 cell containing one absolute position.
    pub fn biome_at(&self, bx: usize, by: i32, bz: usize) -> Option<u32> {
        if bx >= SECTION_BIOME_SIDE
            || bz >= SECTION_BIOME_SIDE
            || by < OVERWORLD_MIN_Y
            || by >= OVERWORLD_MIN_Y + OVERWORLD_HEIGHT as i32
        {
            return None;
        }
        self.biomes.get(biome_index(bx, by, bz)).copied()
    }

    /// Biome id at an absolute world block position, including negative X/Z.
    pub fn biome_at_world(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if x.div_euclid(CHUNK_SIDE as i32) != self.chunk_x
            || z.div_euclid(CHUNK_SIDE as i32) != self.chunk_z
        {
            return None;
        }
        self.biome_at(
            x.rem_euclid(CHUNK_SIDE as i32) as usize / SECTION_BIOME_SIDE,
            y,
            z.rem_euclid(CHUNK_SIDE as i32) as usize / SECTION_BIOME_SIDE,
        )
    }

    /// The `256` state ids of one section, in the client's cell order.
    pub fn section_states(&self, section: usize) -> Vec<u32> {
        let base = OVERWORLD_MIN_Y + section as i32 * SECTION_HEIGHT as i32;
        (0..SECTION_HEIGHT)
            .flat_map(|local| {
                let y = base + local as i32;
                (0..CHUNK_SIDE).flat_map(move |z| {
                    (0..CHUNK_SIDE).map(move |x| self.states[state_index(x, y, z)])
                })
            })
            .collect()
    }

    /// The `64` biome ids of one section, in the client's cell order.
    pub fn section_biomes(&self, section: usize) -> Vec<u32> {
        let base = section * SECTION_BIOME_SIDE;
        (0..SECTION_BIOME_SIDE)
            .flat_map(|local| {
                let by = OVERWORLD_MIN_Y + (base + local) as i32 * SECTION_BIOME_SIDE as i32;
                (0..SECTION_BIOME_SIDE).flat_map(move |bz| {
                    (0..SECTION_BIOME_SIDE).map(move |bx| self.biomes[biome_index(bx, by, bz)])
                })
            })
            .collect()
    }
}

/// Resolves one generated column pair into protocol ids.
///
/// Every name the generator can emit must exist in `tables`: a miss is a typed
/// [`EncodeError::Registry`] error, never a fallback id. Rows the generator
/// left unwritten become `minecraft:air`.
///
/// Biomes are resolved at the bottom block of each 4x4x4 cell. The climate
/// sampler quantizes block positions to this grid, including Y.
pub fn chunk_from_generator(
    generator: &VanillaGenerator,
    chunk_x: i32,
    chunk_z: i32,
    tables: &RegistryTables,
) -> Result<VanillaChunk, EncodeError> {
    chunk_from_generator_cancellable(generator, chunk_x, chunk_z, tables, || false)
        .map(|chunk| chunk.expect("unconditional generation cannot cancel"))
}

fn chunk_origin(chunk_x: i32, chunk_z: i32) -> Result<(i32, i32), EncodeError> {
    let overflow = || EncodeError::ChunkCoordinateOverflow { chunk_x, chunk_z };
    let x = chunk_x
        .checked_mul(CHUNK_SIDE as i32)
        .ok_or_else(overflow)?;
    let z = chunk_z
        .checked_mul(CHUNK_SIDE as i32)
        .ok_or_else(overflow)?;
    if x.checked_add(CHUNK_SIDE as i32 - 1).is_none()
        || z.checked_add(CHUNK_SIDE as i32 - 1).is_none()
    {
        return Err(overflow());
    }
    Ok((x, z))
}

/// Discards a partly built chunk when it is no longer useful to the caller.
pub fn chunk_from_generator_cancellable(
    generator: &VanillaGenerator,
    chunk_x: i32,
    chunk_z: i32,
    tables: &RegistryTables,
    mut should_cancel: impl FnMut() -> bool,
) -> Result<Option<VanillaChunk>, EncodeError> {
    if generator.min_y() != OVERWORLD_MIN_Y {
        return Err(EncodeError::UnsupportedMinY {
            min_y: generator.min_y(),
        });
    }
    let (origin_x, origin_z) = chunk_origin(chunk_x, chunk_z)?;
    let air = tables.air_state();
    let mut states = vec![air; CHUNK_CELLS];
    for z in 0..CHUNK_SIDE {
        for x in 0..CHUNK_SIDE {
            if should_cancel() {
                return Ok(None);
            }
            let wx = origin_x + x as i32;
            let wz = origin_z + z as i32;
            let column = generator.column_ids(wx, wz);
            if column.len() != OVERWORLD_HEIGHT {
                return Err(EncodeError::WrongColumnLength {
                    expected: OVERWORLD_HEIGHT,
                    got: column.len(),
                });
            }
            for (row, name) in column.iter().enumerate() {
                let id = match name {
                    Some(name) => tables.state(name)?.id,
                    None => air,
                };
                let y = OVERWORLD_MIN_Y + row as i32;
                states[state_index(x, y, z)] = id;
            }
        }
    }
    let mut biomes = vec![0u32; CHUNK_BIOMES];
    for bz in 0..SECTION_BIOME_SIDE {
        for bx in 0..SECTION_BIOME_SIDE {
            let wx = origin_x + bx as i32 * SECTION_BIOME_SIDE as i32;
            let wz = origin_z + bz as i32 * SECTION_BIOME_SIDE as i32;
            for layer in 0..BIOME_LAYERS {
                if should_cancel() {
                    return Ok(None);
                }
                let by = OVERWORLD_MIN_Y + layer as i32 * SECTION_BIOME_SIDE as i32;
                let name = generator
                    .biome(wx, wz, by)
                    .ok_or(EncodeError::MissingBiomePlacement)?;
                let id = tables.biome(&name)?;
                biomes[biome_index(bx, by, bz)] = id;
            }
        }
    }
    Ok(Some(VanillaChunk {
        chunk_x,
        chunk_z,
        states,
        biomes,
    }))
}

/// Encodes one complete column as a framed 26.3 chunk packet.
pub fn encode_chunk(chunk: &VanillaChunk, tables: &RegistryTables) -> Result<Vec<u8>, EncodeError> {
    // `ClientboundLevelChunkWithLightPacket.STREAM_CODEC` = INT x, INT z,
    // chunk data, light data; the chunk coordinates are fixed-width
    // big-endian ints, not VarInts.
    let mut body = Vec::with_capacity(64 * 1024);
    body.extend_from_slice(&chunk.chunk_x.to_be_bytes());
    body.extend_from_slice(&chunk.chunk_z.to_be_bytes());
    write_heightmaps(chunk, tables, &mut body)?;
    let sections = write_sections(chunk, tables)?;
    // The section buffer is one opaque `byteArray(2097152)` field.
    put_varint(sections.len() as u32, &mut body);
    body.extend_from_slice(&sections);
    // Block entities: `list(BlockEntityInfo.STREAM_CODEC)`, deferred.
    put_varint(0, &mut body);
    write_light(chunk, tables, &mut body)?;
    Ok(frame(CHUNK_PACKET, &body))
}

/// Highest captured row per column for one heightmap type, as the stored
/// raw values (`absoluteY + 1 - min_y`, zero when nothing is captured).
pub fn heightmap_values(
    chunk: &VanillaChunk,
    tables: &RegistryTables,
    kind: HeightmapKind,
) -> Result<Vec<u32>, EncodeError> {
    let mut tops = Vec::with_capacity(HEIGHTMAP_COLUMNS);
    for z in 0..CHUNK_SIDE {
        for x in 0..CHUNK_SIDE {
            let mut captured = 0u32;
            for row in (0..OVERWORLD_HEIGHT).rev() {
                let y = OVERWORLD_MIN_Y + row as i32;
                let id = chunk
                    .state_at(x, y, z)
                    .ok_or(EncodeError::CellOutOfRange { x, y, z })?;
                if state_kind(tables, id)?.captured_by(kind) {
                    captured = row as u32 + 1;
                    break;
                }
            }
            tops.push(captured);
        }
    }
    Ok(tops)
}

fn write_heightmaps(
    chunk: &VanillaChunk,
    tables: &RegistryTables,
    out: &mut Vec<u8>,
) -> Result<(), EncodeError> {
    // 26.3 heightmaps are `map(Heightmap.Types.STREAM_CODEC, LONG_ARRAY)`:
    // an entry count, then per entry the type id and a VarInt-prefixed,
    // big-endian long array. No NBT names.
    put_varint(HEIGHTMAP_TYPES as u32, out);
    for kind in [
        HeightmapKind::WorldSurface,
        HeightmapKind::MotionBlocking,
        HeightmapKind::MotionBlockingNoLeaves,
    ] {
        put_varint(kind.protocol_id(), out);
        let tops = heightmap_values(chunk, tables, kind)?;
        let words = packed(&tops, HEIGHTMAP_BITS);
        put_varint(words.len() as u32, out);
        for word in words {
            out.extend_from_slice(&word.to_be_bytes());
        }
    }
    Ok(())
}

fn write_sections(chunk: &VanillaChunk, tables: &RegistryTables) -> Result<Vec<u8>, EncodeError> {
    let mut out = Vec::with_capacity(256 * 1024);
    for section in 0..SECTIONS {
        let cells = chunk.section_states(section);
        let mut non_air = 0u16;
        let mut fluids = 0u16;
        for id in &cells {
            let kind = state_kind(tables, *id)?;
            // `LevelChunkSection$1BlockCounter`: the counts are keyed on
            // `isAir()` and on a non-empty fluid state, so carved rows
            // (`cave_air`) count as air and water only feeds the fluid count.
            if kind != StateKind::Air {
                non_air += 1;
            }
            if kind == StateKind::Fluid {
                fluids += 1;
            }
        }
        out.extend_from_slice(&non_air.to_be_bytes());
        out.extend_from_slice(&fluids.to_be_bytes());
        write_block_container(&cells, tables, &mut out)?;
        let biomes = chunk.section_biomes(section);
        write_biome_container(&biomes, tables, &mut out)?;
    }
    Ok(out)
}

fn write_block_container(
    cells: &[u32],
    tables: &RegistryTables,
    out: &mut Vec<u8>,
) -> Result<(), EncodeError> {
    let palette = distinct(cells);
    if palette.len() == 1 {
        // Single-value palette: byte 0 and one bare id, and no long array at
        // all (`SingleValuePalette#write`).
        out.push(0);
        put_varint(palette[0], out);
        return Ok(());
    }
    let bits = ceillog2(palette.len() as u32).max(BLOCK_PALETTE_MIN_BITS);
    if bits > BLOCK_PALETTE_MAX_BITS {
        // Direct mode: the byte is the global palette width and no palette
        // entries follow it (`Strategy$1` falls through to `Configuration.Global`).
        let width = tables.state_bits();
        out.push(width as u8);
        write_words(cells, width, out);
        return Ok(());
    }
    out.push(bits as u8);
    put_varint(palette.len() as u32, out);
    for id in &palette {
        put_varint(*id, out);
    }
    let indices = palette_indices(cells, &palette);
    write_words(&indices, bits, out);
    Ok(())
}

fn write_biome_container(
    cells: &[u32],
    tables: &RegistryTables,
    out: &mut Vec<u8>,
) -> Result<(), EncodeError> {
    for id in cells {
        if *id >= tables.biome_count() {
            return Err(EncodeError::BiomeOutOfRange {
                id: *id,
                count: tables.biome_count(),
            });
        }
    }
    let palette = distinct(cells);
    if palette.len() == 1 {
        out.push(0);
        put_varint(palette[0], out);
        return Ok(());
    }
    let bits = ceillog2(palette.len() as u32).max(BIOME_PALETTE_MIN_BITS);
    if bits > BIOME_PALETTE_MAX_BITS {
        let width = tables.biome_bits();
        out.push(width as u8);
        write_words(cells, width, out);
        return Ok(());
    }
    out.push(bits as u8);
    put_varint(palette.len() as u32, out);
    for id in &palette {
        put_varint(*id, out);
    }
    let indices = palette_indices(cells, &palette);
    write_words(&indices, bits, out);
    Ok(())
}

fn write_light(
    chunk: &VanillaChunk,
    tables: &RegistryTables,
    out: &mut Vec<u8>,
) -> Result<(), EncodeError> {
    // `ClientboundLightUpdatePacketData.STREAM_CODEC`: four BIT_SET masks,
    // then the sky list, then the block list. `BIT_SET` here is
    // `writeByteArray(bitSet.toByteArray())`, i.e. a VarInt byte count over
    // little-endian bytes truncated at the highest set bit — not a long array.
    let all = (1u64 << LIGHT_SECTIONS) - 1;
    put_bit_set(all, out); // skyYMask: every layer is sent.
    put_bit_set(0, out); // blockYMask
    put_bit_set(0, out); // emptySkyYMask
    put_bit_set(all, out); // emptyBlockYMask: no block light computed yet.
    let layers = sky_layers(chunk, tables)?;
    put_varint(LIGHT_SECTIONS as u32, out);
    for layer in &layers {
        put_varint(LIGHT_BYTES as u32, out);
        out.extend_from_slice(layer);
    }
    put_varint(0, out); // Empty block-light update list.
    Ok(())
}

/// Vertical skylight only: the column above a cell decides its level, leaves
/// and fluid cost one level, anything else closes the column. Boundary layers
/// above and below the buildable range are treated as air, matching the
/// preview's accepted output.
fn sky_layers(
    chunk: &VanillaChunk,
    tables: &RegistryTables,
) -> Result<Vec<[u8; LIGHT_BYTES]>, EncodeError> {
    let bottom = OVERWORLD_MIN_Y - SECTION_HEIGHT as i32;
    let top = OVERWORLD_MIN_Y + OVERWORLD_HEIGHT as i32 + SECTION_HEIGHT as i32;
    let mut layers = vec![[0u8; LIGHT_BYTES]; LIGHT_SECTIONS];
    for z in 0..CHUNK_SIDE {
        for x in 0..CHUNK_SIDE {
            let mut sky = 15u8;
            for y in (bottom..top).rev() {
                let kind = match chunk.state_at(x, y, z) {
                    Some(id) => state_kind(tables, id)?,
                    None => StateKind::Air,
                };
                sky = sky.saturating_sub(kind.sky_attenuation());
                let offset = (y - bottom) as usize;
                let cell = x + z * CHUNK_SIDE + offset % SECTION_HEIGHT * CHUNK_SIDE * CHUNK_SIDE;
                layers[offset / SECTION_HEIGHT][cell / 2] |= sky << ((cell % 2) * 4);
            }
        }
    }
    Ok(layers)
}

fn put_bit_set(mut mask: u64, out: &mut Vec<u8>) {
    if mask == 0 {
        put_varint(0, out);
        return;
    }
    let length = (64 - mask.leading_zeros() as usize).div_ceil(8);
    put_varint(length as u32, out);
    for _ in 0..length {
        out.push(mask as u8);
        mask >>= 8;
    }
}

/// The heightmap/light class of one id. Ids the provisioned table does not
/// describe are a hard error: substituting a class would silently change
/// surface heights.
fn state_kind(tables: &RegistryTables, id: u32) -> Result<StateKind, EncodeError> {
    if id >= tables.state_count() {
        return Err(EncodeError::StateOutOfRange {
            id,
            count: tables.state_count(),
        });
    }
    tables.kind(id).ok_or(EncodeError::UnregisteredState { id })
}

fn distinct(cells: &[u32]) -> Vec<u32> {
    cells
        .iter()
        .copied()
        .collect::<BTreeSet<u32>>()
        .into_iter()
        .collect()
}

fn palette_indices(cells: &[u32], palette: &[u32]) -> Vec<u32> {
    cells
        .iter()
        .map(|id| {
            palette
                .binary_search(id)
                .expect("palette holds every cell value") as u32
        })
        .collect()
}

/// Packs fixed-width values without ever crossing a 64-bit word, the
/// `SimpleBitStorage` rule (`valuesPerLong = 64 / bits`), and writes the
/// resulting array big-endian with no length prefix: a container's long array
/// is exactly `ceil(count / valuesPerLong)` words.
fn packed(values: &[u32], bits: usize) -> Vec<u64> {
    let per_word = 64 / bits;
    values
        .chunks(per_word)
        .map(|part| {
            part.iter().enumerate().fold(0u64, |word, (index, value)| {
                word | (u64::from(*value) << (index * bits))
            })
        })
        .collect()
}

fn write_words(values: &[u32], bits: usize, out: &mut Vec<u8>) {
    for word in packed(values, bits) {
        out.extend_from_slice(&word.to_be_bytes());
    }
}

#[cfg(test)]
mod tests;
