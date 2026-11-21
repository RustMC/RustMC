//! A world directory on disk: chunk regions, player files, level data and
//! the edit journal, in the layout a 26.3 save uses.
//!
//! Why: one place decides where every file lives and how it is written, so
//! the game loop only says "load", "save" and "remember".
//! Done: column load and save grouped by region; per-player position,
//! rotation and game mode by UUID; the seed and clocks in `level.dat`.
//! Not done: inventories (added with inventory play), other dimensions.
//!
//! Layout (observed in a 26.3 save, see docs/PROVENANCE.md):
//! `dimensions/minecraft/overworld/region/r.X.Z.mca` and
//! `players/data/<uuid>.dat` (gzip-compressed NBT). RustMC keeps its own
//! level data and journal under `rustmc/`, so it never overwrites a vanilla
//! save's `level.dat`. Opening a vanilla save reads its seed from
//! `data/minecraft/world_gen_settings.dat` (`data.seed`) and its age from
//! `level.dat` (`Data.Time`); its day clock is not read yet, so day time
//! starts at 0 there.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

use super::chunk::{ChunkError, DATA_VERSION, decode, encode};
use super::nbt::{Tag, compound, parse_root, write_root};
use super::region_file::{REGION_SIDE, compress, read_chunk, replace_atomically, save_chunks};
use crate::chunk_adapter::RegistryTables;
use crate::net::identity::uuid_text;
use crate::server::join::GameMode;
use crate::server::world::column::Column;

/// Largest player or level file accepted, so a damaged file cannot exhaust
/// memory; real ones are a few kilobytes.
const MAX_SMALL_FILE: u64 = 1024 * 1024;
/// The level name written to `level.dat`.
pub const LEVEL_NAME: &str = "RustMC world";

/// Why world storage failed.
#[derive(Debug)]
pub enum StorageError {
    Io(PathBuf, io::Error),
    Chunk(PathBuf, ChunkError),
    /// A player or level file is not the expected NBT.
    Malformed(PathBuf, &'static str),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
            Self::Chunk(path, error) => write!(f, "{}: {error}", path.display()),
            Self::Malformed(path, what) => write!(f, "{}: invalid {what}", path.display()),
        }
    }
}

impl std::error::Error for StorageError {}

/// What is kept for a player between sessions.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerData {
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub game_mode: GameMode,
}

/// World-wide state kept in `level.dat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelData {
    pub seed: i64,
    pub world_age: u64,
    pub day_time: u64,
}

/// The files of one world directory.
#[derive(Debug, Clone)]
pub struct WorldFiles {
    root: PathBuf,
}

fn read_gzip_nbt(path: &Path) -> Result<Option<Tag>, StorageError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(StorageError::Io(path.to_path_buf(), error)),
    };
    let mut bytes = Vec::new();
    GzDecoder::new(file)
        .take(MAX_SMALL_FILE)
        .read_to_end(&mut bytes)
        .map_err(|error| StorageError::Io(path.to_path_buf(), error))?;
    parse_root(&bytes)
        .map(Some)
        .map_err(|_| StorageError::Malformed(path.to_path_buf(), "NBT"))
}

fn write_gzip_nbt(path: &Path, tag: &Tag) -> Result<(), StorageError> {
    let mut nbt = Vec::new();
    write_root(tag, &mut nbt);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    let bytes = encoder
        .write_all(&nbt)
        .and_then(|()| encoder.finish())
        .map_err(|error| StorageError::Io(path.to_path_buf(), error))?;
    replace_atomically(path, &[&bytes]).map_err(|error| StorageError::Io(path.to_path_buf(), error))
}

fn mode_from(id: i32) -> Option<GameMode> {
    [
        GameMode::Survival,
        GameMode::Creative,
        GameMode::Adventure,
        GameMode::Spectator,
    ]
    .into_iter()
    .find(|mode| *mode as i32 == id)
}

impl WorldFiles {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    fn region_path(&self, chunk_x: i32, chunk_z: i32) -> PathBuf {
        let side = REGION_SIDE as i32;
        self.root
            .join("dimensions/minecraft/overworld/region")
            .join(format!(
                "r.{}.{}.mca",
                chunk_x.div_euclid(side),
                chunk_z.div_euclid(side)
            ))
    }

    /// The edit journal's path.
    pub fn journal_path(&self) -> PathBuf {
        self.root.join("rustmc").join("journal.bin")
    }

    fn player_path(&self, uuid: &[u8; 16]) -> PathBuf {
        self.root
            .join("players/data")
            .join(format!("{}.dat", uuid_text(uuid)))
    }

    fn level_path(&self) -> PathBuf {
        self.root.join("rustmc").join("level.dat")
    }

    /// Seed and age of a vanilla save in this directory, if it is one.
    fn vanilla_level(&self) -> Result<Option<LevelData>, StorageError> {
        let settings_path = self.root.join("data/minecraft/world_gen_settings.dat");
        let Some(settings) = read_gzip_nbt(&settings_path)? else {
            return Ok(None);
        };
        let seed = match settings.get("data").and_then(|data| data.get("seed")) {
            Some(Tag::Long(seed)) => *seed,
            _ => return Err(StorageError::Malformed(settings_path, "data.seed")),
        };
        let level_path = self.root.join("level.dat");
        let world_age = match read_gzip_nbt(&level_path)?
            .as_ref()
            .and_then(|level| level.get("Data"))
            .and_then(|data| data.get("Time"))
        {
            Some(Tag::Long(time)) => (*time).max(0) as u64,
            _ => 0,
        };
        Ok(Some(LevelData {
            seed,
            world_age,
            day_time: 0,
        }))
    }

    /// The saved column at `(chunk_x, chunk_z)`, if one was saved.
    pub fn load_column(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        tables: &RegistryTables,
    ) -> Result<Option<Column>, StorageError> {
        let path = self.region_path(chunk_x, chunk_z);
        let side = REGION_SIDE as i32;
        let bytes = read_chunk(
            &path,
            chunk_x.rem_euclid(side) as usize,
            chunk_z.rem_euclid(side) as usize,
        )
        .map_err(|error| StorageError::Io(path.clone(), error))?;
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let tag = parse_root(&bytes).map_err(|_| StorageError::Malformed(path.clone(), "chunk"))?;
        // A chunk filed under another slot is damaged: serving it would key
        // it by its own coordinates and leave the requested one missing.
        let stored = (
            tag.get("xPos").and_then(Tag::as_i32),
            tag.get("zPos").and_then(Tag::as_i32),
        );
        if stored != (Some(chunk_x), Some(chunk_z)) {
            return Err(StorageError::Malformed(path, "chunk coordinates"));
        }
        decode(&tag, tables)
            .map(Some)
            .map_err(|error| StorageError::Chunk(path, error))
    }

    /// Saves columns, one atomic rewrite per region file touched.
    pub fn save_columns<'a>(
        &self,
        columns: impl IntoIterator<Item = &'a Column>,
        tables: &RegistryTables,
        game_time: u64,
        timestamp: u32,
    ) -> Result<usize, StorageError> {
        let mut regions: BTreeMap<PathBuf, BTreeMap<(usize, usize), _>> = BTreeMap::new();
        let side = REGION_SIDE as i32;
        let mut count = 0;
        for column in columns {
            let path = self.region_path(column.x, column.z);
            let tag = encode(column, tables, game_time as i64)
                .map_err(|error| StorageError::Chunk(path.clone(), error))?;
            let mut nbt = Vec::new();
            write_root(&tag, &mut nbt);
            let stored = compress(&nbt).map_err(|error| StorageError::Io(path.clone(), error))?;
            regions.entry(path).or_default().insert(
                (
                    column.x.rem_euclid(side) as usize,
                    column.z.rem_euclid(side) as usize,
                ),
                stored,
            );
            count += 1;
        }
        for (path, chunks) in regions {
            save_chunks(&path, chunks, timestamp).map_err(|error| StorageError::Io(path, error))?;
        }
        Ok(count)
    }

    /// A player's saved state, if they played here before.
    pub fn load_player(&self, uuid: &[u8; 16]) -> Result<Option<PlayerData>, StorageError> {
        let path = self.player_path(uuid);
        let Some(tag) = read_gzip_nbt(&path)? else {
            return Ok(None);
        };
        let malformed = |what| StorageError::Malformed(path.clone(), what);
        let doubles: Vec<f64> = match tag.get("Pos").and_then(Tag::as_list) {
            Some(list) => list
                .iter()
                .map(|v| match v {
                    Tag::Double(d) if d.is_finite() => Ok(*d),
                    _ => Err(malformed("Pos")),
                })
                .collect::<Result<_, _>>()?,
            None => return Err(malformed("Pos")),
        };
        let floats: Vec<f32> = match tag.get("Rotation").and_then(Tag::as_list) {
            Some(list) => list
                .iter()
                .map(|v| match v {
                    Tag::Float(f) if f.is_finite() => Ok(*f),
                    _ => Err(malformed("Rotation")),
                })
                .collect::<Result<_, _>>()?,
            None => return Err(malformed("Rotation")),
        };
        let ([x, y, z], [yaw, pitch]) = (
            <[f64; 3]>::try_from(doubles).map_err(|_| malformed("Pos"))?,
            <[f32; 2]>::try_from(floats).map_err(|_| malformed("Rotation"))?,
        );
        let game_mode = tag
            .get("playerGameType")
            .and_then(Tag::as_i32)
            .and_then(mode_from)
            .ok_or_else(|| malformed("playerGameType"))?;
        Ok(Some(PlayerData {
            position: [x, y, z],
            yaw,
            pitch,
            game_mode,
        }))
    }

    /// Saves a player's state into their file. Tags RustMC does not model
    /// (a vanilla save's `Inventory`, health, experience and so on) are kept;
    /// a damaged existing file is an error rather than being overwritten.
    pub fn save_player(&self, uuid: &[u8; 16], player: &PlayerData) -> Result<(), StorageError> {
        let path = self.player_path(uuid);
        let mut tags = match read_gzip_nbt(&path)? {
            Some(Tag::Compound(tags)) => tags,
            Some(_) => return Err(StorageError::Malformed(path, "player")),
            None => BTreeMap::new(),
        };
        let Tag::Compound(modelled) = compound(&[
            ("DataVersion", Tag::Int(DATA_VERSION)),
            (
                "Pos",
                Tag::List(player.position.iter().map(|v| Tag::Double(*v)).collect()),
            ),
            (
                "Rotation",
                Tag::List(vec![Tag::Float(player.yaw), Tag::Float(player.pitch)]),
            ),
            ("playerGameType", Tag::Int(player.game_mode as i32)),
            ("Dimension", Tag::String("minecraft:overworld".to_owned())),
        ]) else {
            return Err(StorageError::Malformed(path, "player"));
        };
        tags.extend(modelled);
        write_gzip_nbt(&path, &Tag::Compound(tags))
    }

    /// The world's saved seed and clocks: RustMC's own level data, else a
    /// vanilla save's, else `None` for a new world.
    pub fn load_level(&self) -> Result<Option<LevelData>, StorageError> {
        let path = self.level_path();
        let Some(tag) = read_gzip_nbt(&path)? else {
            return self.vanilla_level();
        };
        let data = tag
            .get("Data")
            .ok_or_else(|| StorageError::Malformed(path.clone(), "Data"))?;
        let long = |key: &'static str| match data.get(key) {
            Some(Tag::Long(value)) => Ok(*value),
            _ => Err(StorageError::Malformed(path.clone(), key)),
        };
        Ok(Some(LevelData {
            seed: long("RustMCSeed")?,
            world_age: long("Time")?.max(0) as u64,
            day_time: long("DayTime")?.max(0) as u64,
        }))
    }

    /// Saves the seed and clocks.
    pub fn save_level(&self, level: &LevelData) -> Result<(), StorageError> {
        let data = compound(&[
            ("DataVersion", Tag::Int(DATA_VERSION)),
            ("LevelName", Tag::String(LEVEL_NAME.to_owned())),
            ("Time", Tag::Long(level.world_age as i64)),
            ("DayTime", Tag::Long(level.day_time as i64)),
            ("RustMCSeed", Tag::Long(level.seed)),
        ]);
        write_gzip_nbt(&self.level_path(), &compound(&[("Data", data)]))
    }
}

#[cfg(test)]
mod tests;
