//! Comparison-critical metadata of a local Java 26.3 world save: the single
//! reader of `level.dat` plus `data/minecraft/world_gen_settings.dat`, shared
//! by the `inspect_vanilla_save` and `vanilla_oracle` tools.
//!
//! Only aggregate public facts (version name, world seed, overworld generator
//! ids, enabled data packs, spawn position) are decoded and printed; no save
//! content is copied into the repository. Format facts are documented at
//! <https://minecraft.wiki/w/Level.dat> and
//! <https://minecraft.wiki/w/Java_Edition_level_format>.

use std::{fs::File, io::Read, path::Path};

use flate2::read::GzDecoder;

use crate::nbt::{self, Tag};

/// Both files are small; a larger decompressed stream is treated as a corrupt
/// or hostile input rather than parsed.
const MAX_LEVEL_BYTES: u64 = 16 * 1024 * 1024;
/// Single-player save root holding the version and game settings.
const LEVEL_NAME: &str = "level.dat";
/// Stored world generation settings, including the seed actually used.
const SETTINGS_NAME: &str = "data/minecraft/world_gen_settings.dat";
/// The save version the oracle slice is documented against.
const EXPECTED_VERSION: &str = "26.3";

/// Everything the oracle tools need to attribute a measurement to one save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveMetadata {
    pub version: String,
    /// The seed as the game stores it: a signed 64-bit value printed as-is, so
    /// a report line `seed=-2026` names the same number the oracle's `<seed>`
    /// argument has to be.
    pub seed: i64,
    pub generator_type: String,
    pub noise_settings: String,
    pub was_modded: String,
    pub enabled_packs: String,
    pub spawn_xyz: String,
}

impl SaveMetadata {
    /// The one-fact-per-line description `inspect_vanilla_save` prints.
    pub fn describe(&self) -> String {
        format!(
            "version={}\nseed={}\noverworld_generator={}\noverworld_settings={}\nwas_modded={}\nenabled_packs={}\nspawn_xyz={}",
            self.version,
            self.seed,
            self.generator_type,
            self.noise_settings,
            self.was_modded,
            self.enabled_packs,
            self.spawn_xyz
        )
    }
}

fn field<'a>(tag: &'a Tag, name: &str) -> Result<&'a Tag, String> {
    tag.get(name).ok_or_else(|| format!("missing {name}"))
}

/// Decodes the two NBT streams. Fails loudly when either lacks a field the
/// oracle needs or is not the expected save version: an unverifiable save must
/// never be measured against a published baseline.
pub fn metadata(level_bytes: &[u8], settings_bytes: &[u8]) -> Result<SaveMetadata, String> {
    let level = nbt::parse_root(level_bytes)?;
    let data = field(&level, "Data")?;
    let settings = nbt::parse_root(settings_bytes)?;
    let settings = field(&settings, "data")?;
    let seed = match field(settings, "seed")? {
        Tag::Long(value) => *value,
        _ => return Err("world_gen_settings.dat seed is not a long".to_owned()),
    };
    let version = field(field(data, "Version")?, "Name")?
        .as_str()
        .ok_or("Version.Name is not a string")?;
    if version != EXPECTED_VERSION {
        return Err(format!(
            "expected Java {EXPECTED_VERSION} save, found {version}"
        ));
    }
    let generator = field(
        field(field(settings, "dimensions")?, "minecraft:overworld")?,
        "generator",
    )?;
    let generator_type = field(generator, "type")?
        .as_str()
        .ok_or("overworld generator type is not a string")?;
    let noise_settings = field(generator, "settings")?
        .as_str()
        .ok_or("overworld noise settings is not a string")?;
    let packs = data
        .get("DataPacks")
        .and_then(|packs| packs.get("Enabled"))
        .and_then(Tag::as_list)
        .map(|packs| {
            packs
                .iter()
                .filter_map(Tag::as_str)
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_else(|| "<unknown>".to_owned());
    let modded = match data.get("WasModded") {
        Some(Tag::Byte(value)) => (*value != 0).to_string(),
        _ => "<unknown>".to_owned(),
    };
    let spawn = match data.get("spawn").and_then(|tag| tag.get("pos")) {
        Some(Tag::IntArray(pos)) if pos.len() == 3 => {
            format!("{},{},{}", pos[0], pos[1], pos[2])
        }
        _ => "<unknown>".to_owned(),
    };
    Ok(SaveMetadata {
        version: version.to_owned(),
        seed,
        generator_type: generator_type.to_owned(),
        noise_settings: noise_settings.to_owned(),
        was_modded: modded,
        enabled_packs: packs,
        spawn_xyz: spawn,
    })
}

/// Describes already-read metadata bytes.
pub fn describe(level_bytes: &[u8], settings_bytes: &[u8]) -> Result<String, String> {
    Ok(metadata(level_bytes, settings_bytes)?.describe())
}

fn read_gzip(path: &Path) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut input = GzDecoder::new(file).take(MAX_LEVEL_BYTES + 1);
    let mut bytes = Vec::new();
    input
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_LEVEL_BYTES {
        return Err(format!("{} exceeds the 16 MiB read limit", path.display()));
    }
    Ok(bytes)
}

/// Reads and decodes both metadata files of a world directory. An unreadable
/// or unparsable file is an error, never a skipped check.
pub fn metadata_at(world: &Path) -> Result<SaveMetadata, String> {
    let level = read_gzip(&world.join(LEVEL_NAME))?;
    let settings = read_gzip(&world.join(SETTINGS_NAME))?;
    metadata(&level, &settings)
}

/// Describes a world directory by path.
pub fn inspect(world: &Path) -> Result<String, String> {
    Ok(metadata_at(world)?.describe())
}

/// The seed the save itself stores.
pub fn seed(world: &Path) -> Result<i64, String> {
    Ok(metadata_at(world)?.seed)
}

/// Rejects a `<seed>` argument that does not name the save being measured,
/// quoting both values and where each one came from.
pub fn attest_seed(stored: i64, argument: i64) -> Result<(), String> {
    if stored == argument {
        return Ok(());
    }
    Err(format!(
        "seed mismatch: the save's {SETTINGS_NAME} stores seed={stored} while the command-line <seed> argument is seed={argument}; the oracle only measures the save it is pointed at, so pass the stored seed (inspect_vanilla_save prints it)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::{compound, write_root};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    fn encode(tag: &Tag) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_root(tag, &mut bytes);
        bytes
    }

    /// A minimal save metadata pair carrying `seed`, plus a private field in
    /// `level.dat` that must never reach a report.
    fn fixture(seed: i64) -> (Vec<u8>, Vec<u8>) {
        let level = compound(&[(
            "Data",
            compound(&[
                ("Version", compound(&[("Name", Tag::String("26.3".into()))])),
                ("Player", Tag::String("private".into())),
            ]),
        )]);
        let settings = compound(&[(
            "data",
            compound(&[
                ("seed", Tag::Long(seed)),
                (
                    "dimensions",
                    compound(&[(
                        "minecraft:overworld",
                        compound(&[(
                            "generator",
                            compound(&[
                                ("type", Tag::String("minecraft:noise".into())),
                                ("settings", Tag::String("minecraft:overworld".into())),
                            ]),
                        )]),
                    )]),
                ),
            ]),
        )]);
        (encode(&level), encode(&settings))
    }

    fn scratch_dir(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rustmc-save-{}-{}", label, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn attestation_accepts_the_stored_signed_seed() {
        let (level, settings) = fixture(-2026);
        assert_eq!(metadata(&level, &settings).unwrap().seed, -2026);
        assert!(attest_seed(-2026, -2026).is_ok());
        assert!(attest_seed(2027, 2027).is_ok());
    }

    #[test]
    fn attestation_names_both_seeds_on_a_mismatch() {
        let error = attest_seed(-2026, 2026).expect_err("a sign flip is a different seed");
        assert!(error.contains("seed=-2026"), "{error}");
        assert!(error.contains("seed=2026"), "{error}");
        assert!(error.contains("world_gen_settings.dat"), "{error}");
        assert!(error.contains("<seed> argument"), "{error}");
    }

    #[test]
    fn seed_and_description_come_from_the_saves_own_metadata() {
        let dir = scratch_dir("metadata");
        let (level, settings) = fixture(-2026);
        std::fs::create_dir_all(dir.join("data/minecraft")).unwrap();
        std::fs::write(dir.join("level.dat"), gzip(&level)).unwrap();
        std::fs::write(
            dir.join("data/minecraft/world_gen_settings.dat"),
            gzip(&settings),
        )
        .unwrap();
        assert_eq!(seed(&dir).unwrap(), -2026);
        let description = inspect(&dir).unwrap();
        assert!(description.contains("seed=-2026"), "{description}");
        assert!(
            description.contains("overworld_settings=minecraft:overworld"),
            "{description}"
        );
        assert!(!description.contains("private"), "{description}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_metadata_is_a_loud_error_not_a_skipped_check() {
        let dir = scratch_dir("unreadable");
        std::fs::create_dir_all(&dir).unwrap();
        let error = seed(&dir).expect_err("a save without level.dat is unverifiable");
        assert!(error.contains("level.dat"), "{error}");
        std::fs::write(dir.join("level.dat"), gzip(&fixture(-2026).0)).unwrap();
        let error = seed(&dir).expect_err("a save without world settings is unverifiable");
        assert!(error.contains("world_gen_settings.dat"), "{error}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_a_save_of_another_version() {
        let level = compound(&[(
            "Data",
            compound(&[("Version", compound(&[("Name", Tag::String("1.21".into()))]))]),
        )]);
        let settings = compound(&[("data", compound(&[("seed", Tag::Long(1))]))]);
        let error = metadata(&encode(&level), &encode(&settings))
            .expect_err("the oracle measures 26.3 saves only");
        assert!(
            error.contains("expected Java 26.3 save, found 1.21"),
            "{error}"
        );
    }
}
