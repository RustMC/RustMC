//! Unit tests for `storage::world_files`.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::server::world::terrain::{TerrainPalette, generate, tests::tables};
use crate::world::{Block, Generator};

struct TempWorld(PathBuf);

impl TempWorld {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(std::env::temp_dir().join(format!(
            "rustmc-world-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        )))
    }
}

impl Drop for TempWorld {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn columns_save_by_region_and_load_back() {
    let world = TempWorld::new();
    let files = WorldFiles::new(&world.0);
    let tables = tables();
    let palette = TerrainPalette::resolve(&tables).expect("palette");
    let mut a = generate(Generator::new(3), &palette, 0, 0);
    a.set(1, 80, 1, Block::Obsidian as u32);
    let b = generate(Generator::new(3), &palette, 31, -1);
    let c = generate(Generator::new(3), &palette, -33, 40);
    let saved = files
        .save_columns([&a, &b, &c], &tables, 50, 7)
        .expect("save");
    assert_eq!(saved, 3);
    for column in [&a, &b, &c] {
        let loaded = files
            .load_column(column.x, column.z, &tables)
            .expect("load")
            .expect("saved");
        assert_eq!(loaded.to_adapter(), column.to_adapter());
    }
    assert!(files.load_column(5, 5, &tables).expect("load").is_none());
    assert!(
        world
            .0
            .join("dimensions/minecraft/overworld/region/r.-2.1.mca")
            .exists()
    );
}

#[test]
fn a_chunk_filed_under_another_slot_is_an_error() {
    let world = TempWorld::new();
    let files = WorldFiles::new(&world.0);
    let tables = tables();
    let palette = TerrainPalette::resolve(&tables).expect("palette");
    // Chunk (32, 0) sits in slot (0, 0) of r.1.0; renamed to r.0.0 it claims
    // to be chunk (0, 0).
    let column = generate(Generator::new(3), &palette, 32, 0);
    files.save_columns([&column], &tables, 0, 0).expect("save");
    let regions = world.0.join("dimensions/minecraft/overworld/region");
    std::fs::rename(regions.join("r.1.0.mca"), regions.join("r.0.0.mca")).expect("rename");
    assert!(matches!(
        files.load_column(0, 0, &tables),
        Err(StorageError::Malformed(_, "chunk coordinates"))
    ));
}

#[test]
fn players_and_level_data_round_trip_and_missing_files_are_none() {
    let world = TempWorld::new();
    let files = WorldFiles::new(&world.0);
    let uuid = [0xab; 16];
    assert_eq!(files.load_player(&uuid).expect("load"), None);
    assert_eq!(files.load_level().expect("load"), None);
    let player = PlayerData {
        position: [10.5, 70.0, -3.25],
        yaw: 90.0,
        pitch: -15.0,
        game_mode: GameMode::Creative,
    };
    files.save_player(&uuid, &player).expect("save");
    assert_eq!(files.load_player(&uuid).expect("load"), Some(player));
    let level = LevelData {
        seed: -42,
        world_age: 12_345,
        day_time: 6_000,
    };
    files.save_level(&level).expect("save");
    assert_eq!(files.load_level().expect("load"), Some(level));
    assert!(
        world
            .0
            .join("players/data/abababab-abab-abab-abab-abababababab.dat")
            .exists()
    );
}

#[test]
fn a_vanilla_save_supplies_its_seed_and_age_and_its_level_dat_is_kept() {
    let world = TempWorld::new();
    let files = WorldFiles::new(&world.0);
    let settings = compound(&[("data", compound(&[("seed", Tag::Long(2026))]))]);
    write_gzip_nbt(
        &world.0.join("data/minecraft/world_gen_settings.dat"),
        &settings,
    )
    .expect("settings");
    let vanilla = compound(&[("Data", compound(&[("Time", Tag::Long(777))]))]);
    let level_dat = world.0.join("level.dat");
    write_gzip_nbt(&level_dat, &vanilla).expect("level");
    let before = std::fs::read(&level_dat).expect("read");
    assert_eq!(
        files.load_level().expect("load"),
        Some(LevelData {
            seed: 2026,
            world_age: 777,
            day_time: 0
        })
    );
    files
        .save_level(&LevelData {
            seed: 2026,
            world_age: 800,
            day_time: 10,
        })
        .expect("save");
    assert_eq!(
        std::fs::read(&level_dat).expect("read"),
        before,
        "vanilla level.dat untouched"
    );
    assert_eq!(
        files.load_level().expect("load").map(|l| l.world_age),
        Some(800)
    );
}

#[test]
fn damaged_player_files_are_errors_not_defaults() {
    let world = TempWorld::new();
    let files = WorldFiles::new(&world.0);
    let uuid = [1; 16];
    let path = files.player_path(&uuid);
    std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
    std::fs::write(&path, b"not gzip").expect("write");
    assert!(files.load_player(&uuid).is_err());
}
