//! Protocol numeric ID lookups for Java Edition 26.3 (protocol 777).
//!
//! Static built-in registries (`minecraft:item`, `minecraft:data_component_type`,
//! `minecraft:potion`, `minecraft:mob_effect`) are loaded from the official
//! server's `reports/registries.json`.
//!
//! Data-driven synchronized registries (`minecraft:enchantment`,
//! `minecraft:trim_pattern`, `minecraft:trim_material`, `minecraft:instrument`)
//! derive their 0-based network IDs from the entry order in [`RegistryManifest`],
//! matching the configuration phase `Registry Data` packets sent by
//! `crate::discovery_java::registry_data` (see `Java Edition protocol/Registries`
//! revision 3886333: "The ordering of the entries in the Entries array defines
//! the numeric IDs that they will be assigned to, starting from 0 and counting
//! upwards.").

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::preview_data::RegistryManifest;

/// Maximum `reports/registries.json` payload size accepted when loading static IDs.
pub const MAX_REGISTRIES_JSON_BYTES: usize = 16 * 1024 * 1024;

const REGISTRY_ITEM: &str = "minecraft:item";
const REGISTRY_DATA_COMPONENT_TYPE: &str = "minecraft:data_component_type";
const REGISTRY_POTION: &str = "minecraft:potion";
const REGISTRY_MOB_EFFECT: &str = "minecraft:mob_effect";

const REGISTRY_ENCHANTMENT: &str = "minecraft:enchantment";
const REGISTRY_TRIM_PATTERN: &str = "minecraft:trim_pattern";
const REGISTRY_TRIM_MATERIAL: &str = "minecraft:trim_material";
const REGISTRY_INSTRUMENT: &str = "minecraft:instrument";

/// Errors encountered while loading or validating `reports/registries.json`.
#[derive(Debug)]
pub enum ProtocolIdsError {
    /// Reading `reports/registries.json` failed.
    Io {
        file: PathBuf,
        source: std::io::Error,
    },
    /// Parsing `reports/registries.json` as JSON failed.
    Json {
        file: PathBuf,
        source: serde_json::Error,
    },
    /// Top-level structure or size of `reports/registries.json` is invalid.
    InvalidFormat { file: PathBuf, message: String },
    /// A required static registry is absent from `reports/registries.json`.
    MissingRegistry {
        file: PathBuf,
        registry: &'static str,
    },
    /// A required static registry has malformed or duplicate entries.
    InvalidRegistry {
        file: PathBuf,
        registry: &'static str,
        message: String,
    },
}

impl fmt::Display for ProtocolIdsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { file, source } => {
                write!(
                    f,
                    "cannot read registries report {}: {source}",
                    file.display()
                )
            }
            Self::Json { file, source } => {
                write!(
                    f,
                    "cannot parse registries report {}: {source}",
                    file.display()
                )
            }
            Self::InvalidFormat { file, message } => {
                write!(f, "invalid registries report {}: {message}", file.display())
            }
            Self::MissingRegistry { file, registry } => {
                write!(
                    f,
                    "registries report {} is missing required registry `{registry}`",
                    file.display()
                )
            }
            Self::InvalidRegistry {
                file,
                registry,
                message,
            } => {
                write!(
                    f,
                    "invalid registry `{registry}` in {}: {message}",
                    file.display()
                )
            }
        }
    }
}

impl Error for ProtocolIdsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::InvalidFormat { .. }
            | Self::MissingRegistry { .. }
            | Self::InvalidRegistry { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegistryLookup {
    name_to_id: BTreeMap<String, u32>,
    id_to_name: BTreeMap<u32, String>,
}

impl RegistryLookup {
    fn parse(
        file: &Path,
        registry: &'static str,
        value: Option<&Value>,
    ) -> Result<Self, ProtocolIdsError> {
        let obj = value.ok_or_else(|| ProtocolIdsError::MissingRegistry {
            file: file.to_path_buf(),
            registry,
        })?;
        let entries = obj
            .get("entries")
            .and_then(Value::as_object)
            .ok_or_else(|| ProtocolIdsError::InvalidRegistry {
                file: file.to_path_buf(),
                registry,
                message: "missing `entries` JSON object".to_owned(),
            })?;

        let mut name_to_id = BTreeMap::new();
        let mut id_to_name = BTreeMap::new();
        for (name, entry) in entries {
            if name.is_empty() {
                return Err(ProtocolIdsError::InvalidRegistry {
                    file: file.to_path_buf(),
                    registry,
                    message: "empty entry name".to_owned(),
                });
            }
            let raw_id = entry
                .get("protocol_id")
                .and_then(Value::as_u64)
                .ok_or_else(|| ProtocolIdsError::InvalidRegistry {
                    file: file.to_path_buf(),
                    registry,
                    message: format!(
                        "entry `{name}` is missing a non-negative integer `protocol_id`"
                    ),
                })?;
            if raw_id > i32::MAX as u64 {
                return Err(ProtocolIdsError::InvalidRegistry {
                    file: file.to_path_buf(),
                    registry,
                    message: format!("entry `{name}` protocol_id {raw_id} exceeds i32::MAX"),
                });
            }
            let id = raw_id as u32;
            if let Some(previous) = id_to_name.insert(id, name.clone()) {
                return Err(ProtocolIdsError::InvalidRegistry {
                    file: file.to_path_buf(),
                    registry,
                    message: format!(
                        "duplicate protocol_id {id} shared by `{previous}` and `{name}`"
                    ),
                });
            }
            name_to_id.insert(name.clone(), id);
        }

        Ok(Self {
            name_to_id,
            id_to_name,
        })
    }

    fn id(&self, name: &str) -> Option<u32> {
        self.name_to_id.get(name).copied()
    }

    fn name(&self, id: u32) -> Option<&str> {
        self.id_to_name.get(&id).map(String::as_str)
    }

    fn len(&self) -> usize {
        self.name_to_id.len()
    }
}

/// Static Java Edition protocol IDs loaded from `reports/registries.json` for
/// items, data component types, potions, and mob effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolIds {
    items: RegistryLookup,
    component_types: RegistryLookup,
    potions: RegistryLookup,
    mob_effects: RegistryLookup,
}

impl ProtocolIds {
    /// Loads `reports/registries.json` under `root`, retaining
    /// `minecraft:item`, `minecraft:data_component_type`, `minecraft:potion`,
    /// and `minecraft:mob_effect`.
    pub fn load(root: &Path) -> Result<Self, ProtocolIdsError> {
        let file = root.join("reports").join("registries.json");
        let handle = File::open(&file).map_err(|source| ProtocolIdsError::Io {
            file: file.clone(),
            source,
        })?;
        let mut raw = String::new();
        handle
            .take(MAX_REGISTRIES_JSON_BYTES as u64 + 1)
            .read_to_string(&mut raw)
            .map_err(|source| ProtocolIdsError::Io {
                file: file.clone(),
                source,
            })?;
        if raw.len() > MAX_REGISTRIES_JSON_BYTES {
            return Err(ProtocolIdsError::InvalidFormat {
                file,
                message: format!(
                    "file exceeds maximum supported size of {MAX_REGISTRIES_JSON_BYTES} bytes"
                ),
            });
        }
        Self::from_json_str(&file, &raw)
    }

    /// Parses a `registries.json` document from `input`, attributing any errors to `file`.
    pub fn from_json_str(file: &Path, input: &str) -> Result<Self, ProtocolIdsError> {
        let parsed: Value =
            serde_json::from_str(input).map_err(|source| ProtocolIdsError::Json {
                file: file.to_path_buf(),
                source,
            })?;
        let root_obj = parsed
            .as_object()
            .ok_or_else(|| ProtocolIdsError::InvalidFormat {
                file: file.to_path_buf(),
                message: "top-level value must be a JSON object".to_owned(),
            })?;

        let items = RegistryLookup::parse(file, REGISTRY_ITEM, root_obj.get(REGISTRY_ITEM))?;
        let component_types = RegistryLookup::parse(
            file,
            REGISTRY_DATA_COMPONENT_TYPE,
            root_obj.get(REGISTRY_DATA_COMPONENT_TYPE),
        )?;
        let potions = RegistryLookup::parse(file, REGISTRY_POTION, root_obj.get(REGISTRY_POTION))?;
        let mob_effects =
            RegistryLookup::parse(file, REGISTRY_MOB_EFFECT, root_obj.get(REGISTRY_MOB_EFFECT))?;

        Ok(Self {
            items,
            component_types,
            potions,
            mob_effects,
        })
    }

    /// Returns the `minecraft:item` protocol ID for `name`.
    #[must_use]
    pub fn item_id(&self, name: &str) -> Option<u32> {
        self.items.id(name)
    }

    /// Returns the namespaced item identifier for `id`.
    #[must_use]
    pub fn item_name(&self, id: u32) -> Option<&str> {
        self.items.name(id)
    }

    /// Returns the `minecraft:data_component_type` protocol ID for `name`.
    #[must_use]
    pub fn component_type_id(&self, name: &str) -> Option<u32> {
        self.component_types.id(name)
    }

    /// Returns the namespaced data component type identifier for `id`.
    #[must_use]
    pub fn component_type_name(&self, id: u32) -> Option<&str> {
        self.component_types.name(id)
    }

    /// Returns the `minecraft:potion` protocol ID for `name`.
    #[must_use]
    pub fn potion_id(&self, name: &str) -> Option<u32> {
        self.potions.id(name)
    }

    /// Returns the namespaced potion identifier for `id`.
    #[must_use]
    pub fn potion_name(&self, id: u32) -> Option<&str> {
        self.potions.name(id)
    }

    /// Number of loaded `minecraft:item` entries.
    #[must_use]
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Number of loaded `minecraft:data_component_type` entries.
    #[must_use]
    pub fn component_type_count(&self) -> usize {
        self.component_types.len()
    }

    /// Number of loaded `minecraft:potion` entries.
    #[must_use]
    pub fn potion_count(&self) -> usize {
        self.potions.len()
    }

    /// Returns the `minecraft:mob_effect` protocol ID for `name`.
    #[must_use]
    pub fn mob_effect_id(&self, name: &str) -> Option<u32> {
        self.mob_effects.id(name)
    }

    /// Returns the namespaced mob effect identifier for `id`.
    #[must_use]
    pub fn mob_effect_name(&self, id: u32) -> Option<&str> {
        self.mob_effects.name(id)
    }

    /// Number of loaded `minecraft:mob_effect` entries.
    #[must_use]
    pub fn mob_effect_count(&self) -> usize {
        self.mob_effects.len()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DynamicLookup {
    entries: Vec<String>,
    name_to_id: BTreeMap<String, u32>,
}

impl DynamicLookup {
    fn from_manifest(manifest: &RegistryManifest, registry: &str) -> Self {
        let Some((_, entries)) = manifest.registries.iter().find(|(key, _)| key == registry) else {
            return Self::default();
        };
        let mut stored = Vec::with_capacity(entries.len());
        let mut name_to_id = BTreeMap::new();
        for (idx, name) in entries.iter().enumerate() {
            let Ok(id) = u32::try_from(idx) else {
                break;
            };
            stored.push(name.clone());
            name_to_id.entry(name.clone()).or_insert(id);
        }
        Self {
            entries: stored,
            name_to_id,
        }
    }

    fn id(&self, name: &str) -> Option<u32> {
        self.name_to_id.get(name).copied()
    }

    fn name(&self, id: u32) -> Option<&str> {
        self.entries.get(id as usize).map(String::as_str)
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Network IDs for data-driven registries synchronized during configuration.
///
/// IDs equal the 0-based entry index in [`RegistryManifest`], matching the
/// order `crate::discovery_java::registry_data` sends in `Registry Data`:
///
/// ```text
/// // crates/rustmc-server/src/discovery_java.rs:77-80
/// for entry in entries {
///     put_string(entry, &mut body);
///     body.push(0); // Entry definition comes from the negotiated core pack.
/// }
/// ```
///
/// and `crate::discovery_java::initial_configuration` (lines 93-95):
///
/// ```text
/// for (registry, entries) in &manifest.registries {
///     packets.push(registry_data(registry, entries));
/// }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DynamicRegistryIds {
    enchantments: DynamicLookup,
    trim_patterns: DynamicLookup,
    trim_materials: DynamicLookup,
    instruments: DynamicLookup,
}

impl DynamicRegistryIds {
    /// Extracts synced network IDs for `minecraft:enchantment`,
    /// `minecraft:trim_pattern`, `minecraft:trim_material`, and
    /// `minecraft:instrument` from `manifest`.
    #[must_use]
    pub fn from_manifest(manifest: &RegistryManifest) -> Self {
        Self {
            enchantments: DynamicLookup::from_manifest(manifest, REGISTRY_ENCHANTMENT),
            trim_patterns: DynamicLookup::from_manifest(manifest, REGISTRY_TRIM_PATTERN),
            trim_materials: DynamicLookup::from_manifest(manifest, REGISTRY_TRIM_MATERIAL),
            instruments: DynamicLookup::from_manifest(manifest, REGISTRY_INSTRUMENT),
        }
    }

    /// Returns the synced `minecraft:enchantment` network ID for `name`.
    #[must_use]
    pub fn enchantment_id(&self, name: &str) -> Option<u32> {
        self.enchantments.id(name)
    }

    /// Returns the namespaced `minecraft:enchantment` identifier for `id`.
    #[must_use]
    pub fn enchantment_name(&self, id: u32) -> Option<&str> {
        self.enchantments.name(id)
    }

    /// Returns the synced `minecraft:trim_pattern` network ID for `name`.
    #[must_use]
    pub fn trim_pattern_id(&self, name: &str) -> Option<u32> {
        self.trim_patterns.id(name)
    }

    /// Returns the namespaced `minecraft:trim_pattern` identifier for `id`.
    #[must_use]
    pub fn trim_pattern_name(&self, id: u32) -> Option<&str> {
        self.trim_patterns.name(id)
    }

    /// Returns the synced `minecraft:trim_material` network ID for `name`.
    #[must_use]
    pub fn trim_material_id(&self, name: &str) -> Option<u32> {
        self.trim_materials.id(name)
    }

    /// Returns the namespaced `minecraft:trim_material` identifier for `id`.
    #[must_use]
    pub fn trim_material_name(&self, id: u32) -> Option<&str> {
        self.trim_materials.name(id)
    }

    /// Number of synced `minecraft:enchantment` entries.
    #[must_use]
    pub fn enchantment_count(&self) -> usize {
        self.enchantments.len()
    }

    /// Number of synced `minecraft:trim_pattern` entries.
    #[must_use]
    pub fn trim_pattern_count(&self) -> usize {
        self.trim_patterns.len()
    }

    /// Number of synced `minecraft:trim_material` entries.
    #[must_use]
    pub fn trim_material_count(&self) -> usize {
        self.trim_materials.len()
    }

    /// Returns the synced `minecraft:instrument` network ID for `name`.
    #[must_use]
    pub fn instrument_id(&self, name: &str) -> Option<u32> {
        self.instruments.id(name)
    }

    /// Returns the namespaced `minecraft:instrument` identifier for `id`.
    #[must_use]
    pub fn instrument_name(&self, id: u32) -> Option<&str> {
        self.instruments.name(id)
    }

    /// Number of synced `minecraft:instrument` entries.
    #[must_use]
    pub fn instrument_count(&self) -> usize {
        self.instruments.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rustmc-protocol-ids-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    const SYNTHETIC_REGISTRIES: &str = r#"{
        "minecraft:block": {
            "entries": {
                "minecraft:air": { "protocol_id": 0 },
                "minecraft:stone": { "protocol_id": 1 }
            }
        },
        "minecraft:item": {
            "default": "minecraft:air",
            "entries": {
                "minecraft:air": { "protocol_id": 0 },
                "minecraft:stone": { "protocol_id": 1 },
                "minecraft:diamond_sword": { "protocol_id": 851 }
            }
        },
        "minecraft:data_component_type": {
            "entries": {
                "minecraft:damage": { "protocol_id": 3 },
                "minecraft:custom_name": { "protocol_id": 6 },
                "minecraft:enchantments": { "protocol_id": 13 }
            }
        },
        "minecraft:potion": {
            "entries": {
                "minecraft:water": { "protocol_id": 0 },
                "minecraft:awkward": { "protocol_id": 3 }
            }
        },
        "minecraft:mob_effect": {
            "entries": {
                "minecraft:speed": { "protocol_id": 0 },
                "minecraft:night_vision": { "protocol_id": 15 },
                "minecraft:saturation": { "protocol_id": 22 }
            }
        }
    }"#;

    #[test]
    fn loads_only_needed_static_registries_from_disk_and_resolves_both_ways() {
        let root = scratch_root("valid");
        let reports_dir = root.join("reports");
        std::fs::create_dir_all(&reports_dir).expect("create reports dir");
        let file = reports_dir.join("registries.json");
        std::fs::write(&file, SYNTHETIC_REGISTRIES).expect("write registries.json");

        let ids = ProtocolIds::load(&root).expect("load synthetic registries.json");
        assert_eq!(ids.item_count(), 3);
        assert_eq!(ids.component_type_count(), 3);
        assert_eq!(ids.potion_count(), 2);
        assert_eq!(ids.mob_effect_count(), 3);

        assert_eq!(ids.item_id("minecraft:stone"), Some(1));
        assert_eq!(ids.item_id("minecraft:diamond_sword"), Some(851));
        assert_eq!(ids.item_name(851), Some("minecraft:diamond_sword"));
        assert_eq!(ids.item_id("minecraft:missing"), None);
        assert_eq!(ids.item_name(999), None);

        assert_eq!(ids.component_type_id("minecraft:damage"), Some(3));
        assert_eq!(ids.component_type_id("minecraft:custom_name"), Some(6));
        assert_eq!(ids.component_type_id("minecraft:enchantments"), Some(13));
        assert_eq!(ids.component_type_name(6), Some("minecraft:custom_name"));
        assert_eq!(ids.component_type_id("minecraft:unknown"), None);

        assert_eq!(ids.potion_id("minecraft:water"), Some(0));
        assert_eq!(ids.potion_id("minecraft:awkward"), Some(3));
        assert_eq!(ids.potion_name(3), Some("minecraft:awkward"));
        assert_eq!(ids.potion_name(99), None);

        assert_eq!(ids.mob_effect_id("minecraft:speed"), Some(0));
        assert_eq!(ids.mob_effect_id("minecraft:night_vision"), Some(15));
        assert_eq!(ids.mob_effect_id("minecraft:saturation"), Some(22));
        assert_eq!(ids.mob_effect_name(15), Some("minecraft:night_vision"));
        assert_eq!(ids.mob_effect_name(99), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn errors_name_the_file_for_missing_malformed_or_duplicate_registries() {
        let root = scratch_root("missing");
        let missing_file = root.join("reports").join("registries.json");
        let err = ProtocolIds::load(&root).expect_err("missing file must error");
        assert!(matches!(err, ProtocolIdsError::Io { .. }));
        assert!(
            err.to_string()
                .contains(&missing_file.display().to_string()),
            "error must name file: {err}"
        );
        let _ = std::fs::remove_dir_all(&root);

        let path = Path::new("reports/registries.json");

        let json_err = ProtocolIds::from_json_str(path, "{bad json").expect_err("bad json");
        assert!(matches!(json_err, ProtocolIdsError::Json { .. }));
        assert!(json_err.to_string().contains("reports/registries.json"));

        let top_err = ProtocolIds::from_json_str(path, "[]").expect_err("non-object root");
        assert!(matches!(top_err, ProtocolIdsError::InvalidFormat { .. }));
        assert!(top_err.to_string().contains("reports/registries.json"));

        let missing_reg = ProtocolIds::from_json_str(
            path,
            r#"{"minecraft:item":{"entries":{}},"minecraft:data_component_type":{"entries":{}}}"#,
        )
        .expect_err("missing potion registry");
        assert!(matches!(
            missing_reg,
            ProtocolIdsError::MissingRegistry {
                registry: "minecraft:potion",
                ..
            }
        ));
        assert!(missing_reg.to_string().contains("reports/registries.json"));

        let missing_mob_effect = ProtocolIds::from_json_str(
            path,
            r#"{"minecraft:item":{"entries":{}},"minecraft:data_component_type":{"entries":{}},"minecraft:potion":{"entries":{}}}"#,
        )
        .expect_err("missing mob_effect registry");
        assert!(matches!(
            missing_mob_effect,
            ProtocolIdsError::MissingRegistry {
                registry: "minecraft:mob_effect",
                ..
            }
        ));

        let dup_err = ProtocolIds::from_json_str(
            path,
            r#"{
                "minecraft:item": { "entries": {
                    "minecraft:a": { "protocol_id": 1 },
                    "minecraft:b": { "protocol_id": 1 }
                }},
                "minecraft:data_component_type": { "entries": {} },
                "minecraft:potion": { "entries": {} },
                "minecraft:mob_effect": { "entries": {} }
            }"#,
        )
        .expect_err("duplicate protocol_id must error");
        assert!(matches!(
            dup_err,
            ProtocolIdsError::InvalidRegistry {
                registry: "minecraft:item",
                ..
            }
        ));
        assert!(dup_err.to_string().contains("reports/registries.json"));

        let neg_err = ProtocolIds::from_json_str(
            path,
            r#"{
                "minecraft:item": { "entries": {} },
                "minecraft:data_component_type": { "entries": { "minecraft:x": { "protocol_id": -1 } } },
                "minecraft:potion": { "entries": {} },
                "minecraft:mob_effect": { "entries": {} }
            }"#,
        )
        .expect_err("negative protocol_id must error");
        assert!(matches!(
            neg_err,
            ProtocolIdsError::InvalidRegistry {
                registry: "minecraft:data_component_type",
                ..
            }
        ));
        assert!(neg_err.to_string().contains("reports/registries.json"));
    }

    #[test]
    fn dynamic_registry_ids_preserve_manifest_order_and_handle_absent_registries() {
        let manifest = RegistryManifest {
            tags: Vec::new(),
            registries: vec![
                (
                    "minecraft:worldgen/biome".to_owned(),
                    vec!["minecraft:plains".to_owned()],
                ),
                (
                    "minecraft:enchantment".to_owned(),
                    vec![
                        "minecraft:sharpness".to_owned(),
                        "minecraft:unbreaking".to_owned(),
                        "minecraft:mending".to_owned(),
                    ],
                ),
                (
                    "minecraft:trim_pattern".to_owned(),
                    vec!["minecraft:coast".to_owned(), "minecraft:ward".to_owned()],
                ),
                (
                    "minecraft:trim_material".to_owned(),
                    vec![
                        "minecraft:amethyst".to_owned(),
                        "minecraft:diamond".to_owned(),
                    ],
                ),
                (
                    "minecraft:instrument".to_owned(),
                    vec![
                        "minecraft:ponder_goat_horn".to_owned(),
                        "minecraft:sing_goat_horn".to_owned(),
                    ],
                ),
            ],
        };

        let dyn_ids = DynamicRegistryIds::from_manifest(&manifest);
        assert_eq!(dyn_ids.enchantment_count(), 3);
        assert_eq!(dyn_ids.trim_pattern_count(), 2);
        assert_eq!(dyn_ids.trim_material_count(), 2);
        assert_eq!(dyn_ids.instrument_count(), 2);

        assert_eq!(dyn_ids.enchantment_id("minecraft:sharpness"), Some(0));
        assert_eq!(dyn_ids.enchantment_id("minecraft:unbreaking"), Some(1));
        assert_eq!(dyn_ids.enchantment_id("minecraft:mending"), Some(2));
        assert_eq!(dyn_ids.enchantment_name(2), Some("minecraft:mending"));
        assert_eq!(dyn_ids.enchantment_name(3), None);
        assert_eq!(dyn_ids.enchantment_id("minecraft:looting"), None);

        assert_eq!(dyn_ids.trim_pattern_id("minecraft:coast"), Some(0));
        assert_eq!(dyn_ids.trim_pattern_id("minecraft:ward"), Some(1));
        assert_eq!(dyn_ids.trim_pattern_name(1), Some("minecraft:ward"));
        assert_eq!(dyn_ids.trim_pattern_name(2), None);

        assert_eq!(dyn_ids.trim_material_id("minecraft:amethyst"), Some(0));
        assert_eq!(dyn_ids.trim_material_id("minecraft:diamond"), Some(1));
        assert_eq!(dyn_ids.trim_material_name(0), Some("minecraft:amethyst"));
        assert_eq!(dyn_ids.trim_material_name(2), None);

        assert_eq!(dyn_ids.instrument_id("minecraft:ponder_goat_horn"), Some(0));
        assert_eq!(dyn_ids.instrument_id("minecraft:sing_goat_horn"), Some(1));
        assert_eq!(dyn_ids.instrument_name(1), Some("minecraft:sing_goat_horn"));
        assert_eq!(dyn_ids.instrument_name(2), None);
        assert_eq!(dyn_ids.instrument_id("minecraft:admire_goat_horn"), None);

        let empty = DynamicRegistryIds::from_manifest(&RegistryManifest {
            tags: Vec::new(),
            registries: vec![(
                "minecraft:dimension_type".to_owned(),
                vec!["minecraft:overworld".to_owned()],
            )],
        });
        assert_eq!(empty.enchantment_count(), 0);
        assert_eq!(empty.trim_pattern_count(), 0);
        assert_eq!(empty.trim_material_count(), 0);
        assert_eq!(empty.instrument_count(), 0);
        assert_eq!(empty.enchantment_id("minecraft:sharpness"), None);
        assert_eq!(empty.instrument_id("minecraft:ponder_goat_horn"), None);
    }

    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_registries_pin_26_3_protocol_ids`
    #[test]
    #[ignore]
    fn vanilla_registries_pin_26_3_protocol_ids() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let root_path = Path::new(&root);
        let ids = ProtocolIds::load(root_path).expect("load reports/registries.json");

        let raw = std::fs::read_to_string(root_path.join("reports").join("registries.json"))
            .expect("read reports/registries.json");
        let parsed: Value = serde_json::from_str(&raw).expect("parse registries.json");

        let read_expected = |registry: &str, entry: &str| -> u32 {
            u32::try_from(
                parsed[registry]["entries"][entry]["protocol_id"]
                    .as_u64()
                    .expect("protocol_id in registries.json"),
            )
            .expect("protocol_id fits u32")
        };

        assert_eq!(
            parsed["minecraft:item"]["entries"]
                .as_object()
                .expect("item entries")
                .len(),
            1658
        );
        assert_eq!(
            parsed["minecraft:data_component_type"]["entries"]
                .as_object()
                .expect("component entries")
                .len(),
            122
        );
        assert_eq!(
            parsed["minecraft:potion"]["entries"]
                .as_object()
                .expect("potion entries")
                .len(),
            46
        );
        assert_eq!(
            parsed["minecraft:mob_effect"]["entries"]
                .as_object()
                .expect("mob_effect entries")
                .len(),
            40
        );

        assert_eq!(ids.item_count(), 1658);
        assert_eq!(ids.component_type_count(), 122);
        assert_eq!(ids.potion_count(), 46);
        assert_eq!(ids.mob_effect_count(), 40);

        let expected_components = [
            ("minecraft:damage", 3),
            ("minecraft:custom_name", 6),
            ("minecraft:item_name", 9),
            ("minecraft:enchantments", 13),
            ("minecraft:repair_cost", 19),
            ("minecraft:stored_enchantments", 45),
            ("minecraft:potion_contents", 53),
            ("minecraft:suspicious_stew_effects", 55),
            ("minecraft:trim", 58),
            ("minecraft:instrument", 63),
            ("minecraft:ominous_bottle_amplifier", 65),
        ];
        for (name, pinned) in expected_components {
            let from_file = read_expected("minecraft:data_component_type", name);
            assert_eq!(from_file, pinned, "file mismatch for {name}");
            assert_eq!(ids.component_type_id(name), Some(from_file));
            assert_eq!(ids.component_type_name(from_file), Some(name));
        }

        let expected_mob_effects = [
            ("minecraft:speed", 0),
            ("minecraft:night_vision", 15),
            ("minecraft:poison", 18),
            ("minecraft:saturation", 22),
        ];
        for (name, pinned) in expected_mob_effects {
            let from_file = read_expected("minecraft:mob_effect", name);
            assert_eq!(from_file, pinned, "file mismatch for {name}");
            assert_eq!(ids.mob_effect_id(name), Some(from_file));
            assert_eq!(ids.mob_effect_name(from_file), Some(name));
        }

        let awkward_from_file = read_expected("minecraft:potion", "minecraft:awkward");
        assert_eq!(awkward_from_file, 3);
        assert_eq!(ids.potion_id("minecraft:awkward"), Some(awkward_from_file));
        assert_eq!(
            ids.potion_name(awkward_from_file),
            Some("minecraft:awkward")
        );

        let stone_from_file = read_expected("minecraft:item", "minecraft:stone");
        assert_eq!(ids.item_id("minecraft:stone"), Some(stone_from_file));
        assert_eq!(ids.item_name(stone_from_file), Some("minecraft:stone"));
    }
}
