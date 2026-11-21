//! The authoritative world state: committed and pending block mutations
//! over generated chunks, and its errors.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockMutation {
    pub x: i64,
    pub y: i64,
    pub z: i64,
    pub block: Block,
}

impl BlockMutation {
    pub const fn new(x: i64, y: i64, z: i64, block: Block) -> Self {
        Self { x, y, z, block }
    }
}

#[derive(Debug)]
pub enum WorldError {
    Io(std::io::Error),
    Corrupted(String),
    SeedMismatch {
        expected: u64,
        found: u64,
    },
    TerrainMismatch {
        expected: Terrain,
        found: Terrain,
    },
    InvalidMagic,
    OutOfBounds {
        x: i64,
        y: i64,
        z: i64,
    },
    InvalidBlockId(u8),
    InvalidTerrainId(u8),
    /// A commit or sync failed earlier; the world must be reopened.
    JournalUnusable,
    TooManyMutations {
        count: usize,
        limit: usize,
    },
    TickOutOfSequence {
        expected: u64,
        found: u64,
    },
}

impl std::fmt::Display for WorldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Corrupted(msg) => write!(f, "corrupted world data: {msg}"),
            Self::SeedMismatch { expected, found } => {
                write!(f, "seed mismatch: expected {expected}, found {found}")
            }
            Self::TerrainMismatch { expected, found } => {
                write!(
                    f,
                    "terrain mismatch: expected {expected:?}, found {found:?}"
                )
            }
            Self::InvalidMagic => write!(f, "invalid file magic header"),
            Self::OutOfBounds { x, y, z } => {
                write!(f, "coordinates out of bounds: ({x}, {y}, {z})")
            }
            Self::InvalidBlockId(id) => write!(f, "invalid block id: {id}"),
            Self::InvalidTerrainId(id) => write!(f, "invalid terrain id: {id}"),
            Self::JournalUnusable => {
                write!(f, "journal failed earlier and must be reopened")
            }
            Self::TooManyMutations { count, limit } => {
                write!(f, "{count} mutations exceed the per-tick limit of {limit}")
            }
            Self::TickOutOfSequence { expected, found } => {
                write!(
                    f,
                    "tick out of sequence: expected {expected}, found {found}"
                )
            }
        }
    }
}

impl std::error::Error for WorldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WorldError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

#[derive(Debug)]
pub struct World {
    pub(super) generator: Generator,
    pub(super) chunk_cache: std::collections::BTreeMap<(i32, i32), Chunk>,
    pub(super) committed_mutations: std::collections::BTreeMap<(i64, i64, i64), Block>,
    pub(super) journal: Option<WorldJournal>,
    pub(super) current_tick: u64,
    pub(super) pending_tick_mutations: Vec<BlockMutation>,
}

impl World {
    pub fn new(generator: Generator) -> Self {
        Self {
            generator,
            chunk_cache: std::collections::BTreeMap::new(),
            committed_mutations: std::collections::BTreeMap::new(),
            journal: None,
            current_tick: 0,
            pending_tick_mutations: Vec::new(),
        }
    }

    pub fn open(
        path: impl AsRef<std::path::Path>,
        generator: Generator,
    ) -> Result<Self, WorldError> {
        let (journal, committed_mutations, current_tick) =
            WorldJournal::open(path.as_ref(), generator.seed(), generator.terrain())?;
        Ok(Self {
            generator,
            chunk_cache: std::collections::BTreeMap::new(),
            committed_mutations,
            journal: Some(journal),
            current_tick,
            pending_tick_mutations: Vec::new(),
        })
    }

    pub fn block(&mut self, x: i64, y: i64, z: i64) -> Option<Block> {
        if y < 0 || y >= WORLD_HEIGHT as i64 {
            return None;
        }
        if let Some(m) = self
            .pending_tick_mutations
            .iter()
            .rev()
            .find(|m| m.x == x && m.y == y && m.z == z)
        {
            return Some(m.block);
        }
        if let Some(&block) = self.committed_mutations.get(&(x, y, z)) {
            return Some(block);
        }
        let side = CHUNK_SIDE as i64;
        let chunk_x = i32::try_from(x.div_euclid(side)).ok()?;
        let chunk_z = i32::try_from(z.div_euclid(side)).ok()?;
        let chunk = self.chunk(chunk_x, chunk_z);
        chunk.block(
            x.rem_euclid(side) as usize,
            y as usize,
            z.rem_euclid(side) as usize,
        )
    }

    pub fn get_block(&self, x: i64, y: i64, z: i64) -> Option<Block> {
        if y < 0 || y >= WORLD_HEIGHT as i64 {
            return None;
        }
        if let Some(m) = self
            .pending_tick_mutations
            .iter()
            .rev()
            .find(|m| m.x == x && m.y == y && m.z == z)
        {
            return Some(m.block);
        }
        if let Some(&block) = self.committed_mutations.get(&(x, y, z)) {
            return Some(block);
        }
        let side = CHUNK_SIDE as i64;
        let chunk_x = i32::try_from(x.div_euclid(side)).ok()?;
        let chunk_z = i32::try_from(z.div_euclid(side)).ok()?;
        if let Some(chunk) = self.chunk_cache.get(&(chunk_x, chunk_z)) {
            chunk.block(
                x.rem_euclid(side) as usize,
                y as usize,
                z.rem_euclid(side) as usize,
            )
        } else {
            let chunk = self.generator.generate(chunk_x, chunk_z);
            chunk.block(
                x.rem_euclid(side) as usize,
                y as usize,
                z.rem_euclid(side) as usize,
            )
        }
    }

    pub fn chunk(&mut self, chunk_x: i32, chunk_z: i32) -> &Chunk {
        let generator = &self.generator;
        let committed = &self.committed_mutations;
        let pending = &self.pending_tick_mutations;
        self.chunk_cache
            .entry((chunk_x, chunk_z))
            .or_insert_with(|| {
                let mut chunk = generator.generate(chunk_x, chunk_z);
                let min_x = i64::from(chunk_x) * CHUNK_SIDE as i64;
                let max_x = min_x + CHUNK_SIDE as i64;
                let min_z = i64::from(chunk_z) * CHUNK_SIDE as i64;
                let max_z = min_z + CHUNK_SIDE as i64;
                for (&(x, y, z), &block) in committed {
                    if x >= min_x && x < max_x && z >= min_z && z < max_z {
                        chunk.set_world_block(x, y, z, block);
                    }
                }
                for m in pending {
                    if m.x >= min_x && m.x < max_x && m.z >= min_z && m.z < max_z {
                        chunk.set_world_block(m.x, m.y, m.z, m.block);
                    }
                }
                chunk
            })
    }

    /// Stages a mutation for the current tick. Returns `false` when the
    /// coordinates are out of range or the tick already holds
    /// [`MAX_MUTATIONS_PER_TICK`] mutations.
    pub fn set_block(&mut self, x: i64, y: i64, z: i64, block: Block) -> bool {
        if y < 0 || y >= WORLD_HEIGHT as i64 {
            return false;
        }
        if self.pending_tick_mutations.len() >= MAX_MUTATIONS_PER_TICK {
            return false;
        }
        let side = CHUNK_SIDE as i64;
        let Ok(chunk_x) = i32::try_from(x.div_euclid(side)) else {
            return false;
        };
        let Ok(chunk_z) = i32::try_from(z.div_euclid(side)) else {
            return false;
        };
        self.pending_tick_mutations
            .push(BlockMutation { x, y, z, block });
        if let Some(chunk) = self.chunk_cache.get_mut(&(chunk_x, chunk_z)) {
            chunk.set_world_block(x, y, z, block);
        }
        true
    }

    /// Commits the staged mutations. `Ok` means they are durable. After `Err`
    /// the staged mutations remain in memory but their durability is unknown,
    /// and a journaled world must be reopened to continue.
    pub fn commit_tick(&mut self) -> Result<(), WorldError> {
        if let Some(journal) = &mut self.journal {
            journal.append_tick(self.current_tick, &self.pending_tick_mutations)?;
        }
        for m in self.pending_tick_mutations.drain(..) {
            self.committed_mutations.insert((m.x, m.y, m.z), m.block);
            let side = CHUNK_SIDE as i64;
            let Ok(chunk_x) = i32::try_from(m.x.div_euclid(side)) else {
                continue;
            };
            let Ok(chunk_z) = i32::try_from(m.z.div_euclid(side)) else {
                continue;
            };
            if let Some(chunk) = self.chunk_cache.get_mut(&(chunk_x, chunk_z)) {
                chunk.set_world_block(m.x, m.y, m.z, m.block);
            }
        }
        self.current_tick += 1;
        Ok(())
    }

    pub fn compact(&mut self) -> Result<(), WorldError> {
        if let Some(journal) = &mut self.journal {
            journal.compact(self.current_tick, &self.committed_mutations)?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), WorldError> {
        if let Some(journal) = &mut self.journal {
            journal.flush()?;
        }
        Ok(())
    }

    pub fn current_tick(&self) -> u64 {
        self.current_tick
    }

    pub fn generator(&self) -> Generator {
        self.generator
    }

    pub fn mutations(&self) -> &std::collections::BTreeMap<(i64, i64, i64), Block> {
        &self.committed_mutations
    }

    pub fn pending_mutations(&self) -> &[BlockMutation] {
        &self.pending_tick_mutations
    }

    pub fn journal(&self) -> Option<&WorldJournal> {
        self.journal.as_ref()
    }

    pub fn journal_mut(&mut self) -> Option<&mut WorldJournal> {
        self.journal.as_mut()
    }
}
