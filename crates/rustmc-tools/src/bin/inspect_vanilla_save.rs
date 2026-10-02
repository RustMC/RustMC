//! Read only comparison-critical metadata from a local Java 26.3 save.
//! Usage: inspect_vanilla_save WORLD_DIRECTORY

use std::{fs::File, io::Read, path::Path};

use flate2::read::GzDecoder;
use rustmc_tools::nbt::{self, Tag};

const MAX_LEVEL_BYTES: u64 = 16 * 1024 * 1024;

fn field<'a>(tag: &'a Tag, name: &str) -> Result<&'a Tag, String> {
    tag.get(name).ok_or_else(|| format!("missing {name}"))
}

fn describe(level_bytes: &[u8], settings_bytes: &[u8]) -> Result<String, String> {
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
    if version != "26.3" {
        return Err(format!("expected Java 26.3 save, found {version}"));
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
    Ok(format!(
        "version={version}\nseed={seed}\noverworld_generator={generator_type}\noverworld_settings={noise_settings}\nwas_modded={modded}\nenabled_packs={packs}\nspawn_xyz={spawn}"
    ))
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

fn inspect(world: &Path) -> Result<String, String> {
    let level = read_gzip(&world.join("level.dat"))?;
    let settings = read_gzip(&world.join("data/minecraft/world_gen_settings.dat"))?;
    describe(&level, &settings)
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 1 {
        eprintln!("usage: inspect_vanilla_save WORLD_DIRECTORY");
        std::process::exit(2);
    }
    match inspect(Path::new(&args[0])) {
        Ok(description) => println!("{description}"),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn compound(entries: impl IntoIterator<Item = (&'static str, Tag)>) -> Tag {
        Tag::Compound(
            entries
                .into_iter()
                .map(|(name, tag)| (name.to_owned(), tag))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    fn write_compound(entries: &BTreeMap<String, Tag>, out: &mut Vec<u8>) {
        for (name, tag) in entries {
            out.push(match tag {
                Tag::Compound(_) => 10,
                Tag::Long(_) => 4,
                Tag::String(_) => 8,
                _ => panic!("test fixture uses only compound, long, and string"),
            });
            out.extend((name.len() as u16).to_be_bytes());
            out.extend(name.as_bytes());
            match tag {
                Tag::Compound(inner) => write_compound(inner, out),
                Tag::Long(value) => out.extend(value.to_be_bytes()),
                Tag::String(value) => {
                    out.extend((value.len() as u16).to_be_bytes());
                    out.extend(value.as_bytes());
                }
                _ => unreachable!(),
            }
        }
        out.push(0);
    }

    #[test]
    fn reads_exact_seed_and_generator_without_personal_fields() {
        let level = compound([(
            "Data",
            compound([
                ("Version", compound([("Name", Tag::String("26.3".into()))])),
                ("Player", Tag::String("private".into())),
            ]),
        )]);
        let settings = compound([(
            "data",
            compound([
                ("seed", Tag::Long(-2026)),
                (
                    "dimensions",
                    compound([(
                        "minecraft:overworld",
                        compound([(
                            "generator",
                            compound([
                                ("type", Tag::String("minecraft:noise".into())),
                                ("settings", Tag::String("minecraft:overworld".into())),
                            ]),
                        )]),
                    )]),
                ),
            ]),
        )]);
        let encode = |tag| {
            let Tag::Compound(root) = tag else {
                unreachable!()
            };
            let mut bytes = vec![10, 0, 0];
            write_compound(&root, &mut bytes);
            bytes
        };
        let output = describe(&encode(level), &encode(settings)).unwrap();
        assert!(output.contains("seed=-2026"));
        assert!(output.contains("overworld_settings=minecraft:overworld"));
        assert!(!output.contains("private"));
    }
}
