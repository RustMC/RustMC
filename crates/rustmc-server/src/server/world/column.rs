//! One chunk column of the shared world, stored as 26.3 block-state ids.
//!
//! Why: players build with any block, so the world stores protocol state ids
//! rather than a fixed block enum. Sections that hold a single state (open
//! air, solid rock) keep one value instead of 4,096.
//! Done: reads, writes with section promotion, conversion to the chunk
//! adapter's column for encoding. Not done: palette compression of mixed
//! sections in memory (a full section costs 16 KiB).

use crate::chunk_adapter::{
    CHUNK_BIOMES, CHUNK_SIDE, EncodeError, OVERWORLD_HEIGHT, OVERWORLD_MIN_Y, SECTION_CELLS,
    SECTION_HEIGHT, SECTIONS, VanillaChunk,
};

/// Highest buildable Y (exclusive) in the overworld.
pub const MAX_Y: i32 = OVERWORLD_MIN_Y + OVERWORLD_HEIGHT as i32;

/// A read view of one section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionCells<'a> {
    Uniform(u32),
    Mixed(&'a [u32]),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Section {
    Uniform(u32),
    Mixed(Box<[u32]>),
}

/// One 16x384x16 column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub x: i32,
    pub z: i32,
    sections: Vec<Section>,
    biomes: Vec<u32>,
}

fn cell(x: usize, local_y: usize, z: usize) -> usize {
    x + z * CHUNK_SIDE + local_y * CHUNK_SIDE * CHUNK_SIDE
}

impl Column {
    /// A column of one state and one biome.
    pub fn filled(x: i32, z: i32, state: u32, biome: u32) -> Self {
        Self {
            x,
            z,
            sections: vec![Section::Uniform(state); SECTIONS],
            biomes: vec![biome; CHUNK_BIOMES],
        }
    }

    /// Splits an absolute Y into section and local row, if in range.
    fn locate(y: i32) -> Option<(usize, usize)> {
        if !(OVERWORLD_MIN_Y..MAX_Y).contains(&y) {
            return None;
        }
        let row = (y - OVERWORLD_MIN_Y) as usize;
        Some((row / SECTION_HEIGHT, row % SECTION_HEIGHT))
    }

    /// The state at local `x`, `z` (0..16) and absolute `y`.
    pub fn get(&self, x: usize, y: i32, z: usize) -> Option<u32> {
        let (section, row) = Self::locate(y)?;
        if x >= CHUNK_SIDE || z >= CHUNK_SIDE {
            return None;
        }
        Some(match &self.sections[section] {
            Section::Uniform(state) => *state,
            Section::Mixed(cells) => cells[cell(x, row, z)],
        })
    }

    /// Sets one block and returns the previous state, or `None` when the
    /// position is outside the column.
    pub fn set(&mut self, x: usize, y: i32, z: usize, state: u32) -> Option<u32> {
        let (section, row) = Self::locate(y)?;
        if x >= CHUNK_SIDE || z >= CHUNK_SIDE {
            return None;
        }
        let slot = &mut self.sections[section];
        if let Section::Uniform(current) = *slot {
            if current == state {
                return Some(current);
            }
            *slot = Section::Mixed(vec![current; SECTION_CELLS].into_boxed_slice());
        }
        match slot {
            Section::Mixed(cells) => Some(std::mem::replace(&mut cells[cell(x, row, z)], state)),
            Section::Uniform(_) => None,
        }
    }

    /// One section's cells, if `section` is in range.
    pub fn section(&self, section: usize) -> Option<SectionCells<'_>> {
        Some(match self.sections.get(section)? {
            Section::Uniform(state) => SectionCells::Uniform(*state),
            Section::Mixed(cells) => SectionCells::Mixed(cells),
        })
    }

    /// Replaces one section; `cells` must hold 4,096 states in section order.
    /// A section of one repeated state is stored as a single value.
    pub fn set_section(&mut self, section: usize, cells: Vec<u32>) -> bool {
        if cells.len() != SECTION_CELLS || section >= self.sections.len() {
            return false;
        }
        self.sections[section] = match cells.first() {
            Some(first) if cells.iter().all(|state| state == first) => Section::Uniform(*first),
            _ => Section::Mixed(cells.into_boxed_slice()),
        };
        true
    }

    /// Every biome cell, in the chunk adapter's order.
    pub fn biomes(&self) -> &[u32] {
        &self.biomes
    }

    /// Replaces the biome cells; `biomes` must cover the whole column.
    pub fn set_biomes(&mut self, biomes: Vec<u32>) -> bool {
        if biomes.len() != CHUNK_BIOMES {
            return false;
        }
        self.biomes = biomes;
        true
    }

    /// Fills a whole section with one state.
    pub fn fill_section(&mut self, section: usize, state: u32) {
        if let Some(slot) = self.sections.get_mut(section) {
            *slot = Section::Uniform(state);
        }
    }

    /// Sets every biome cell of the 4x4 column at biome cell `bx`, `bz`.
    pub fn set_biome_column(&mut self, bx: usize, bz: usize, biome: u32) {
        let side = crate::chunk_adapter::SECTION_BIOME_SIDE;
        for layer in 0..CHUNK_BIOMES / (side * side) {
            self.biomes[bx + bz * side + layer * side * side] = biome;
        }
    }

    /// Bytes this column holds in memory, for budgeting.
    pub fn memory_bytes(&self) -> usize {
        let mixed = self
            .sections
            .iter()
            .filter(|s| matches!(s, Section::Mixed(_)))
            .count();
        mixed * SECTION_CELLS * 4 + self.biomes.len() * 4
    }

    /// The full column in the chunk adapter's layout, for encoding.
    pub fn to_adapter(&self) -> Result<VanillaChunk, EncodeError> {
        let mut states = Vec::with_capacity(SECTIONS * SECTION_CELLS);
        for section in &self.sections {
            match section {
                Section::Uniform(state) => {
                    states.extend(std::iter::repeat_n(*state, SECTION_CELLS))
                }
                Section::Mixed(cells) => states.extend_from_slice(cells),
            }
        }
        VanillaChunk::new(self.x, self.z, states, self.biomes.clone())
    }
}

#[cfg(test)]
mod tests;
