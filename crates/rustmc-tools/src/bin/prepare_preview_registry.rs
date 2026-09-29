//! Prepare local-only identifier metadata from the licensed official 26.3 archive.
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const SHA1: &str = "33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c";
const INNER: &str = "META-INF/versions/26.3/server-26.3.jar";
const REGISTRIES: &[&str] = &[
    "worldgen/biome",
    "chat_type",
    "trim_pattern",
    "trim_material",
    "wolf_variant",
    "wolf_sound_variant",
    "pig_variant",
    "pig_sound_variant",
    "frog_variant",
    "cat_variant",
    "cat_sound_variant",
    "cow_sound_variant",
    "cow_variant",
    "chicken_sound_variant",
    "chicken_variant",
    "zombie_nautilus_variant",
    "painting_variant",
    "sulfur_cube_archetype",
    "dimension_type",
    "damage_type",
    "banner_pattern",
    "enchantment",
    "jukebox_song",
    "instrument",
    "test_environment",
    "test_instance",
    "dialog",
    "world_clock",
    "timeline",
    "decorated_pot_pattern",
    "block_transformer",
    "worldgen/block_state_provider",
];

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn command(program: &str, args: &[&str]) -> Result<Vec<u8>, Box<dyn Error>> {
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output.stdout)
}

fn files_under(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            result.extend(files_under(&path)?);
        } else if path.extension().is_some_and(|ext| ext == "json") {
            result.push(path);
        }
    }
    result.sort();
    Ok(result)
}

fn names(root: &Path, registry: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let prefix = root.join("data/minecraft").join(registry);
    Ok(files_under(&prefix)?
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(&prefix).expect("path from prefix");
            format!(
                "minecraft:{}",
                relative.with_extension("").to_string_lossy()
            )
        })
        .collect())
}

fn resolve_tag(
    tag: &str,
    definitions: &BTreeMap<String, Value>,
    ids: &BTreeMap<String, usize>,
    trail: &mut BTreeSet<String>,
) -> Result<BTreeSet<usize>, Box<dyn Error>> {
    if !trail.insert(tag.to_string()) {
        return Err(format!("cyclic registry tag {tag}").into());
    }
    let values = definitions[tag]["values"]
        .as_array()
        .ok_or_else(|| format!("tag {tag} has no values array"))?;
    let mut result = BTreeSet::new();
    for raw in values {
        let (value, required) = if let Some(text) = raw.as_str() {
            (text, true)
        } else {
            (
                raw["id"].as_str().ok_or("tag value without id")?,
                raw["required"].as_bool().unwrap_or(true),
            )
        };
        if let Some(nested) = value.strip_prefix('#') {
            if definitions.contains_key(nested) {
                result.extend(resolve_tag(nested, definitions, ids, trail)?);
            } else if required {
                return Err(format!("missing tag {nested}").into());
            }
        } else if let Some(id) = ids.get(value) {
            result.insert(*id);
        } else if required {
            return Err(format!("missing entry {value}").into());
        }
    }
    trail.remove(tag);
    Ok(result)
}

fn prepare(archive: &Path, output: &Path, report: &Path) -> Result<(), Box<dyn Error>> {
    let archive_arg = archive.to_str().ok_or("non-UTF8 archive path")?;
    let actual = String::from_utf8(command("sha1sum", &[archive_arg])?)?;
    if actual.split_whitespace().next() != Some(SHA1) {
        return Err("official Java 26.3 server archive SHA-1 mismatch".into());
    }
    let version: Value =
        serde_json::from_slice(&command("unzip", &["-p", archive_arg, "version.json"])?)?;
    if version["id"] != "26.3" || version["protocol_version"] != 777 {
        return Err("archive version metadata is not Java 26.3 / 777".into());
    }
    let temp_path = std::env::temp_dir().join(format!("rustmc-registry-{}", std::process::id()));
    fs::create_dir(&temp_path)?;
    let temp = TempDir(temp_path);
    let inner = temp.0.join("server-26.3.jar");
    fs::write(&inner, command("unzip", &["-p", archive_arg, INNER])?)?;
    let extract = temp.0.join("extracted");
    let inner_arg = inner.to_str().ok_or("non-UTF8 temporary path")?;
    let extract_arg = extract.to_str().ok_or("non-UTF8 temporary path")?;
    command("unzip", &["-q", "-o", inner_arg, "-d", extract_arg])?;
    let static_report: Value = serde_json::from_slice(&fs::read(report)?)?;
    let mut registry_keys: BTreeSet<String> = REGISTRIES.iter().map(|s| (*s).to_string()).collect();
    let static_object = static_report.as_object().ok_or("invalid registry report")?;
    registry_keys.extend(
        static_object
            .keys()
            .filter_map(|k| k.strip_prefix("minecraft:").map(str::to_string)),
    );
    let mut tags: BTreeMap<String, BTreeMap<String, Vec<usize>>> = BTreeMap::new();
    for registry in registry_keys {
        let prefix = extract.join("data/minecraft/tags").join(&registry);
        let definitions: BTreeMap<String, Value> = files_under(&prefix)?
            .into_iter()
            .map(|path| -> Result<_, Box<dyn Error>> {
                let relative = path.strip_prefix(&prefix)?;
                Ok((
                    format!(
                        "minecraft:{}",
                        relative.with_extension("").to_string_lossy()
                    ),
                    serde_json::from_slice(&fs::read(&path)?)?,
                ))
            })
            .collect::<Result<_, _>>()?;
        if definitions.is_empty() {
            continue;
        }
        let entries = if REGISTRIES.contains(&registry.as_str()) {
            names(&extract, &registry)?
        } else {
            let values = static_report[format!("minecraft:{registry}")]["entries"]
                .as_object()
                .ok_or("missing static registry entries")?;
            let mut entries: Vec<_> = values
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        value["protocol_id"].as_u64().unwrap_or(u64::MAX),
                    )
                })
                .collect();
            entries.sort_by_key(|(_, id)| *id);
            if entries
                .iter()
                .enumerate()
                .any(|(i, (_, id))| *id != i as u64)
            {
                return Err(format!("noncontiguous static registry IDs in {registry}").into());
            }
            entries.into_iter().map(|(name, _)| name).collect()
        };
        let ids: BTreeMap<_, _> = entries
            .into_iter()
            .enumerate()
            .map(|(i, name)| (name, i))
            .collect();
        let mut resolved = BTreeMap::new();
        for tag in definitions.keys() {
            resolved.insert(
                tag.clone(),
                resolve_tag(tag, &definitions, &ids, &mut BTreeSet::new())?
                    .into_iter()
                    .collect(),
            );
        }
        tags.insert(registry, resolved);
    }
    let mut lines = vec!["version = \"26.3\"".to_string(), "[registries]".to_string()];
    let mut total = 0;
    for registry in REGISTRIES {
        let entries = names(&extract, registry)?;
        if entries.is_empty()
            && ![
                "dialog",
                "test_environment",
                "test_instance",
                "worldgen/block_state_provider",
            ]
            .contains(registry)
        {
            return Err(format!("expected entries for {registry}").into());
        }
        total += entries.len();
        lines.push(format!(
            "{} = [",
            serde_json::to_string(&format!("minecraft:{registry}"))?
        ));
        lines.extend(
            entries
                .iter()
                .map(|name| format!("  {},", serde_json::to_string(name).unwrap())),
        );
        lines.push("]".to_string());
    }
    lines.push("[static_registry_sizes]".to_string());
    for registry in tags
        .keys()
        .filter(|name| !REGISTRIES.contains(&name.as_str()))
    {
        let size = static_report[format!("minecraft:{registry}")]["entries"]
            .as_object()
            .ok_or("missing static registry")?
            .len();
        lines.push(format!(
            "{} = {size}",
            serde_json::to_string(&format!("minecraft:{registry}"))?
        ));
    }
    for (registry, values) in tags {
        lines.push(format!(
            "[tags.{}]",
            serde_json::to_string(&format!("minecraft:{registry}"))?
        ));
        for (tag, ids) in values {
            lines.push(format!(
                "{} = {}",
                serde_json::to_string(&tag)?,
                serde_json::to_string(&ids)?
            ));
        }
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, lines.join("\n") + "\n")?;
    println!(
        "Wrote {} registry lists, {total} identifiers to {}",
        REGISTRIES.len(),
        output.display()
    );
    println!("Keep this generated manifest local; do not commit or redistribute it.");
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 4 {
        return Err("usage: prepare_preview_registry OFFICIAL_SERVER_JAR LOCAL_OUTPUT_TOML OFFICIAL_REGISTRIES_REPORT".into());
    }
    prepare(
        Path::new(&args[1]),
        Path::new(&args[2]),
        Path::new(&args[3]),
    )
}
