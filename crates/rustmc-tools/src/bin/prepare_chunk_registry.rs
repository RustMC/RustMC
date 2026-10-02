//! Build a local-only Java 26.3 chunk ID table from official data reports.
//!
//! Usage: prepare_chunk_registry BLOCKS_REPORT PREVIEW_MANIFEST OUTPUT_JSON
//! The reports and output are operator-provisioned game data; keep them out of Git.

use std::{collections::BTreeMap, error::Error, fs, path::Path};

use rustmc_server::{
    chunk_adapter::{RegistryTables, SUPPORTED_PROTOCOL, SUPPORTED_VERSION},
    preview_data,
};
use serde_json::{Value, json};

fn state_table(report: &Value) -> Result<(BTreeMap<String, u32>, u32), String> {
    let blocks = report
        .as_object()
        .ok_or("blocks report must be a JSON object")?;
    let mut states = BTreeMap::new();
    let mut largest = None::<u32>;
    for (block, data) in blocks {
        let entries = data
            .get("states")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{block} has no states array"))?;
        if entries.is_empty() {
            return Err(format!("{block} has no states"));
        }
        let mut default_count = 0;
        for entry in entries {
            let id = u32::try_from(
                entry
                    .get("id")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("{block} has a state without an id"))?,
            )
            .map_err(|_| format!("{block} has an out-of-range state id"))?;
            largest = Some(largest.map_or(id, |value| value.max(id)));
            let properties = entry.get("properties").and_then(Value::as_object);
            let key = match properties {
                Some(properties) if !properties.is_empty() => {
                    let parts = properties
                        .iter()
                        .map(|(name, value)| {
                            value
                                .as_str()
                                .map(|value| format!("{name}={value}"))
                                .ok_or_else(|| format!("{block} has a non-string property"))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    format!("{block}[{}]", parts.join(","))
                }
                _ => block.clone(),
            };
            if states.insert(key.clone(), id).is_some() {
                return Err(format!("duplicate state {key}"));
            }
            if entry.get("default").and_then(Value::as_bool) == Some(true) {
                default_count += 1;
                // The material rules emit bare block names. This alias
                // resolves each one to the report's explicit default state.
                states.insert(block.clone(), id);
            }
        }
        if default_count != 1 {
            return Err(format!("{block} must have exactly one default state"));
        }
    }
    let count = largest
        .and_then(|id| id.checked_add(1))
        .ok_or("blocks report has no valid state count")?;
    Ok((states, count))
}

fn prepare(blocks: &Path, manifest: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let report: Value = serde_json::from_slice(&fs::read(blocks)?)?;
    let (states, state_count) = state_table(&report)?;
    let manifest = preview_data::load(manifest)?;
    let biomes = manifest
        .registries
        .iter()
        .find(|(name, _)| name == "minecraft:worldgen/biome")
        .ok_or("preview manifest has no biome registry")?
        .1
        .iter()
        .enumerate()
        .map(|(id, name)| (name.clone(), id as u32))
        .collect::<BTreeMap<_, _>>();
    let document = json!({
        "version": SUPPORTED_VERSION,
        "protocol": SUPPORTED_PROTOCOL,
        "block_state_count": state_count,
        "biome_count": biomes.len(),
        "block_states": states,
        "biomes": biomes,
    });
    let encoded = serde_json::to_vec(&document)?;
    RegistryTables::from_provisioned(std::str::from_utf8(&encoded)?)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, encoded)?;
    println!(
        "Prepared local Java 26.3 chunk ID table: {}",
        output.display()
    );
    Ok(())
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        eprintln!("usage: prepare_chunk_registry BLOCKS_REPORT PREVIEW_MANIFEST OUTPUT_JSON");
        std::process::exit(2);
    }
    if let Err(error) = prepare(
        Path::new(&args[0]),
        Path::new(&args[1]),
        Path::new(&args[2]),
    ) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_alias_and_sorted_properties_preserve_report_ids() {
        let report = json!({
            "minecraft:air": {"states": [{"id": 0, "default": true}]},
            "minecraft:oak_log": {"states": [
                {"id": 4, "properties": {"axis": "x"}},
                {"id": 5, "properties": {"axis": "y"}, "default": true}
            ]}
        });
        let (states, count) = state_table(&report).unwrap();
        assert_eq!(count, 6);
        assert_eq!(states["minecraft:oak_log"], 5);
        assert_eq!(states["minecraft:oak_log[axis=x]"], 4);
        assert_eq!(states["minecraft:oak_log[axis=y]"], 5);
    }

    #[test]
    fn missing_or_ambiguous_default_is_rejected() {
        for report in [
            json!({"minecraft:air": {"states": [{"id": 0}]}}),
            json!({"minecraft:air": {"states": [
                {"id": 0, "default": true}, {"id": 1, "default": true}
            ]}}),
        ] {
            assert!(state_table(&report).is_err());
        }
    }
}
