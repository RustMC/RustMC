use std::io::Write;
use std::path::Path;
use std::process::Command;

use flate2::{
    Compression,
    write::{GzEncoder, ZlibEncoder},
};
fn name(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

fn compound(bytes: &mut Vec<u8>, field: &str) {
    bytes.push(10);
    name(bytes, field);
}

fn string(bytes: &mut Vec<u8>, field: &str, value: &str) {
    bytes.push(8);
    name(bytes, field);
    name(bytes, value);
}

fn list(bytes: &mut Vec<u8>, field: &str, element_tag: u8, count: i32) {
    bytes.push(9);
    name(bytes, field);
    bytes.push(element_tag);
    bytes.extend_from_slice(&count.to_be_bytes());
}

fn write_single_stored_chunk(world: &Path) {
    // Independent synthetic Anvil data: one generated column at (0, 0),
    // with the other five worksheet points absent from the save.
    let mut chunk = Vec::new();
    compound(&mut chunk, "");
    compound(&mut chunk, "Heightmaps");
    chunk.push(12);
    name(&mut chunk, "WORLD_SURFACE");
    chunk.extend_from_slice(&36i32.to_be_bytes());
    chunk.extend_from_slice(&140i64.to_be_bytes());
    chunk.extend(std::iter::repeat_n(0u8, 35 * 8));
    chunk.push(0);
    list(&mut chunk, "sections", 10, 1);
    chunk.push(1);
    name(&mut chunk, "Y");
    chunk.push(4);
    compound(&mut chunk, "block_states");
    list(&mut chunk, "palette", 10, 1);
    string(&mut chunk, "Name", "minecraft:stone");
    chunk.push(0);
    chunk.push(0);
    compound(&mut chunk, "biomes");
    list(&mut chunk, "palette", 8, 1);
    name(&mut chunk, "minecraft:plains");
    chunk.push(0);
    chunk.extend_from_slice(&[0, 0]);

    let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
    zlib.write_all(&chunk).unwrap();
    let compressed = zlib.finish().unwrap();
    let mut region = vec![0u8; 8192 + 4096];
    region[..4].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
    region[8192..8196].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
    region[8196] = 2;
    region[8197..8197 + compressed.len()].copy_from_slice(&compressed);
    std::fs::create_dir_all(world.join("region")).unwrap();
    std::fs::write(world.join("region/r.0.0.mca"), region).unwrap();
}

fn metadata_nbt() -> (Vec<u8>, Vec<u8>) {
    let mut level = Vec::new();
    compound(&mut level, "");
    compound(&mut level, "Data");
    compound(&mut level, "Version");
    string(&mut level, "Name", "26.3");
    level.extend_from_slice(&[0, 0, 0]);

    let mut settings = Vec::new();
    compound(&mut settings, "");
    compound(&mut settings, "data");
    settings.push(4);
    name(&mut settings, "seed");
    settings.extend_from_slice(&2026i64.to_be_bytes());
    compound(&mut settings, "dimensions");
    compound(&mut settings, "minecraft:overworld");
    compound(&mut settings, "generator");
    string(&mut settings, "type", "minecraft:noise");
    string(&mut settings, "settings", "minecraft:overworld");
    settings.extend_from_slice(&[0, 0, 0, 0, 0]);
    (level, settings)
}

fn write_gzip_nbt(path: &Path, bytes: &[u8]) {
    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(bytes).unwrap();
    std::fs::write(path, gzip.finish().unwrap()).unwrap();
}

#[test]
fn absent_worksheet_and_column_report_their_denominators_even_with_partial_flag() {
    let dir = std::env::temp_dir().join(format!(
        "rustmc-oracle-coverage-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("data/minecraft")).unwrap();
    let (level, settings) = metadata_nbt();
    write_gzip_nbt(&dir.join("level.dat"), &level);
    write_gzip_nbt(
        &dir.join("data/minecraft/world_gen_settings.dat"),
        &settings,
    );

    for (mode, args, coverage) in [
        (
            "worksheet",
            vec!["2026", "preview"],
            "coverage scored=0 expected=6 missing=6 unaccounted=0 source=preview-not-vanilla-generator",
        ),
        (
            "column",
            vec!["2026", "0", "0"],
            "coverage scored=0 expected=1 missing=1 unaccounted=0 source=vanilla",
        ),
    ] {
        for allow_partial in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_vanilla_oracle"));
            command.arg(mode).arg(&dir).args(&args);
            if allow_partial {
                command.arg("--allow-partial");
            }
            let output = command.output().unwrap();
            assert!(!output.status.success(), "{mode} accepted an empty sample");
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert!(stderr.contains(coverage), "{mode}: {stderr}");
            assert!(stderr.contains("nothing was scored"), "{mode}: {stderr}");
        }
    }

    write_single_stored_chunk(&dir);
    for allow_partial in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vanilla_oracle"));
        command.arg("worksheet").arg(&dir).args(["2026", "preview"]);
        if allow_partial {
            command.arg("--allow-partial");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.success(), allow_partial);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("coverage scored=1 expected=6 missing=5 unaccounted=0 source=preview-not-vanilla-generator"),
            "{stderr}"
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(stdout.lines().count(), 2, "{stdout}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}
