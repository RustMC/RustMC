//! Validation for a local identifier-only manifest from the official 26.3 archive.

use std::{fs::File, io::Read, path::Path};

pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RegistryManifest {
    pub tags: Vec<RegistryTags>,
    pub registries: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone)]
pub struct RegistryTags {
    pub registry: String,
    pub tags: Vec<(String, Vec<u32>)>,
}

fn valid_identifier(value: &str) -> bool {
    value.len() > 10
        && value.len() <= 255
        && value.starts_with("minecraft:")
        && value[10..].bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_./-".contains(&byte)
        })
}

pub fn parse(input: &str) -> Result<RegistryManifest, String> {
    if input.len() > MAX_MANIFEST_BYTES {
        return Err("preview registry manifest exceeds 1 MiB".to_owned());
    }
    let value: toml::Value = toml::from_str(input)
        .map_err(|_| "preview registry manifest is not valid TOML".to_owned())?;
    let table = value
        .as_table()
        .ok_or_else(|| "preview registry manifest must be a TOML table".to_owned())?;
    if table.get("version").and_then(toml::Value::as_str) != Some("26.3") {
        return Err("preview registry manifest must target Java 26.3".to_owned());
    }
    let entries = table
        .get("registries")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "preview registry manifest needs a [registries] table".to_owned())?;
    if entries.is_empty() || entries.len() > 64 {
        return Err("preview registry count must be from 1 to 64".to_owned());
    }
    let mut registries = Vec::with_capacity(entries.len());
    let mut total = 0usize;
    for (key, value) in entries {
        if !valid_identifier(key) {
            return Err("preview registry key is invalid".to_owned());
        }
        let names = value
            .as_array()
            .ok_or_else(|| "preview registry entries must be arrays".to_owned())?;
        if names.len() > 512 {
            return Err("preview registry has more than 512 entries".to_owned());
        }
        total += names.len();
        if total > 4096 {
            return Err("preview registry manifest has too many entries".to_owned());
        }
        let mut parsed = Vec::with_capacity(names.len());
        for name in names {
            let name = name
                .as_str()
                .ok_or_else(|| "preview registry entry is not a string".to_owned())?;
            if !valid_identifier(name) || parsed.iter().any(|prior| prior == name) {
                return Err("preview registry entry is invalid or duplicated".to_owned());
            }
            parsed.push(name.to_owned());
        }
        registries.push((key.to_owned(), parsed));
    }
    for (key, required) in [
        ("minecraft:dimension_type", "minecraft:overworld"),
        ("minecraft:worldgen/biome", "minecraft:plains"),
        ("minecraft:damage_type", "minecraft:generic"),
    ] {
        if !registries
            .iter()
            .any(|(name, values)| name == key && values.iter().any(|value| value == required))
        {
            return Err(format!(
                "preview registry manifest lacks required {key} entry"
            ));
        }
    }
    let mut tags = Vec::new();
    if let Some(value) = table.get("tags") {
        let tables = value.as_table().ok_or("tags must be tables")?;
        let mut tag_count = 0;
        let mut id_count = 0;
        for (registry, value) in tables {
            let dynamic = registries
                .iter()
                .find(|(key, _)| key == registry)
                .map(|(_, entries)| entries.len());
            let static_size = table
                .get("static_registry_sizes")
                .and_then(toml::Value::as_table)
                .and_then(|sizes| sizes.get(registry))
                .and_then(toml::Value::as_integer)
                .filter(|size| (1..=32768).contains(size))
                .map(|size| size as usize);
            let entry_count = dynamic
                .or(static_size)
                .ok_or("tag registry is not present")?;
            if !valid_identifier(registry) {
                return Err("invalid tag registry".into());
            }
            let values = value.as_table().ok_or("registry tags must be a table")?;
            let mut parsed = Vec::new();
            for (name, value) in values {
                tag_count += 1;
                if tag_count > 4096 || !valid_identifier(name) {
                    return Err("invalid or excessive tags".into());
                }
                let ids = value.as_array().ok_or("tag IDs must be arrays")?;
                id_count += ids.len();
                if id_count > 131072 {
                    return Err("too many tag IDs".into());
                }
                let mut checked = Vec::new();
                for id in ids {
                    let id = id.as_integer().ok_or("tag ID must be an integer")?;
                    if id < 0 || id as usize >= entry_count || checked.contains(&(id as u32)) {
                        return Err("tag ID is invalid or duplicated".into());
                    }
                    checked.push(id as u32);
                }
                parsed.push((name.clone(), checked));
            }
            tags.push(RegistryTags {
                registry: registry.clone(),
                tags: parsed,
            });
        }
    }
    Ok(RegistryManifest { registries, tags })
}

pub fn load(path: &Path) -> Result<RegistryManifest, String> {
    let mut input = String::new();
    File::open(path)
        .and_then(|file| {
            file.take(MAX_MANIFEST_BYTES as u64 + 1)
                .read_to_string(&mut input)
        })
        .map_err(|error| format!("could not read preview registry manifest: {error}"))?;
    parse(&input)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "version = '26.3'\n[registries]\n'minecraft:dimension_type' = ['minecraft:overworld']\n'minecraft:worldgen/biome' = ['minecraft:plains']\n'minecraft:damage_type' = ['minecraft:generic']\n";

    #[test]
    fn accepts_required_local_identifiers() {
        let parsed = parse(VALID).unwrap();
        assert_eq!(parsed.registries.len(), 3);
    }

    #[test]
    fn tag_ids_are_bounded_by_their_registry() {
        let input = format!("{VALID}\n[tags.'minecraft:dimension_type']\n'minecraft:test' = [0]\n");
        assert_eq!(parse(&input).unwrap().tags[0].tags[0].1, [0]);
        for ids in ["[-1]", "[1]", "[0, 0]", "['zero']"] {
            assert!(parse(&input.replace("[0]", ids)).is_err());
        }
        assert!(parse(&input.replace("minecraft:test", "minecraft:")).is_err());
        assert!(parse(&"x".repeat(MAX_MANIFEST_BYTES + 1)).is_err());
    }

    #[test]
    fn rejects_missing_required_or_invalid_entries() {
        assert!(parse(&VALID.replace("26.3", "26.2")).is_err());
        assert!(parse(&VALID.replace("minecraft:plains", "minecraft:forest")).is_err());
        assert!(parse(&VALID.replace("minecraft:generic", "BAD NAME")).is_err());
        assert!(
            parse(&VALID.replace(
                "['minecraft:generic']",
                "['minecraft:generic', 'minecraft:generic']"
            ))
            .is_err()
        );
    }
}
