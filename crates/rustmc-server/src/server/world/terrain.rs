//! Default terrain for the playable server: the fast, deterministic
//! coordinate-derived generator (`crate::world::Generator`), resolved to
//! 26.3 block states and laid over a solid base down to the world floor.
//!
//! Why: players need ground immediately; vanilla-accurate terrain is a later
//! phase and stays in the opt-in `vanilla` module.
//! Done: hills, biomes, surface layers and trees from the generator; stone
//! below y 0 with a bedrock floor. Not done: caves, water, ores.

use super::column::Column;
use crate::chunk_adapter::{
    CHUNK_SIDE, OVERWORLD_MIN_Y, RegistryError, RegistryTables, SECTION_BIOME_SIDE, SECTION_HEIGHT,
};
use crate::world::{Biome, Block, Generator};

/// Every block and biome of the default terrain as protocol ids.
#[derive(Debug, Clone)]
pub struct TerrainPalette {
    blocks: Vec<u32>,
    biomes: Vec<(Biome, u32)>,
    air: u32,
}

impl TerrainPalette {
    /// Resolves every default-terrain block and biome in the provisioned
    /// registry table; a missing name is a provisioning error.
    pub fn resolve(tables: &RegistryTables) -> Result<Self, RegistryError> {
        let mut blocks = vec![tables.air_state(); Block::ALL.len()];
        for block in Block::ALL {
            blocks[block as usize] = tables.state(block.state_name())?.id;
        }
        let biomes = Biome::ALL
            .iter()
            .map(|biome| tables.biome(biome.identifier()).map(|id| (*biome, id)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            blocks,
            biomes,
            air: tables.air_state(),
        })
    }

    fn block(&self, block: Block) -> u32 {
        self.blocks.get(block as usize).copied().unwrap_or(self.air)
    }

    fn biome(&self, biome: Biome) -> u32 {
        self.biomes
            .iter()
            .find(|(b, _)| *b == biome)
            .map_or(0, |(_, id)| *id)
    }
}

/// Generates the default-terrain column at `(chunk_x, chunk_z)`.
pub fn generate(
    generator: Generator,
    palette: &TerrainPalette,
    chunk_x: i32,
    chunk_z: i32,
) -> Column {
    let base = generator.generate(chunk_x, chunk_z);
    let stone = palette.block(Block::Stone);
    let mut column = Column::filled(chunk_x, chunk_z, palette.air, palette.biome(Biome::Plains));
    // Everything below the generator's range is solid stone.
    let solid_sections = (0 - OVERWORLD_MIN_Y) as usize / SECTION_HEIGHT;
    for section in 0..solid_sections {
        column.fill_section(section, stone);
    }
    let bedrock = palette.block(Block::Bedrock);
    for z in 0..CHUNK_SIDE {
        for x in 0..CHUNK_SIDE {
            column.set(x, OVERWORLD_MIN_Y, z, bedrock);
            for y in 0..crate::world::WORLD_HEIGHT {
                let block = base.block(x, y, z).unwrap_or(Block::Air);
                let block = if block == Block::Bedrock {
                    Block::Stone
                } else {
                    block
                };
                if block != Block::Air {
                    column.set(x, y as i32, z, palette.block(block));
                }
            }
        }
    }
    let min_x = i64::from(chunk_x) * CHUNK_SIDE as i64;
    let min_z = i64::from(chunk_z) * CHUNK_SIDE as i64;
    let cell = (CHUNK_SIDE / SECTION_BIOME_SIDE) as i64;
    for bz in 0..SECTION_BIOME_SIDE {
        for bx in 0..SECTION_BIOME_SIDE {
            let biome = generator.biome(min_x + bx as i64 * cell, min_z + bz as i64 * cell);
            column.set_biome_column(bx, bz, palette.biome(biome));
        }
    }
    column
}

/// The Y a player can stand at above the surface at block `(x, z)`.
pub fn surface_y(generator: Generator, x: i64, z: i64) -> i32 {
    generator.height(x, z) as i32 + 1
}

#[cfg(test)]
mod tests;
