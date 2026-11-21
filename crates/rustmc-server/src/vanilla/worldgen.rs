//! Loader for operator-provisioned vanilla world-generation data packs.
//!
//! Vanilla terrain is data-driven: noise definitions, density-function
//! graphs, and per-dimension `noise_settings` documents live in the
//! game's datapack layout. Mojang's files are never committed to this
//! repository (see `docs/PROVENANCE.md` and ADR-0014); an operator
//! provisions a local directory and this module reads it at runtime.
//!
//! The layout recognized here is the documented datapack structure:
//! `data/<namespace>/worldgen/noise/<name>.json`,
//! `data/<namespace>/worldgen/density_function/<path>.json` (nested
//! subdirectories are part of the identifier),
//! `data/<namespace>/worldgen/material_rule/<path>.json`,
//! `data/<namespace>/worldgen/material_condition/<path>.json`, and
//! `data/<namespace>/worldgen/noise_settings/<name>.json`. Identifiers
//! are `<namespace>:<path>`, matching the string references used inside
//! density JSON and the `noise_router` section.

use std::collections::HashMap;
use std::fs::{self, DirEntry};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::vanilla::aquifer::{AquiferConfig, Fluid};
use crate::vanilla::density::{Density, DensityRegistry, NoiseEngine, builtin_density_ids};
use crate::vanilla::noise::NoiseParameters;
use crate::vanilla::random::RandomSource;

#[derive(Debug)]
pub enum WorldgenError {
    Io(PathBuf, std::io::Error),
    Json(PathBuf, serde_json::Error),
    Invalid(String),
}

impl std::fmt::Display for WorldgenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(path, error) => write!(f, "cannot read {}: {error}", path.display()),
            Self::Json(path, error) => write!(f, "cannot parse {}: {error}", path.display()),
            Self::Invalid(message) => write!(f, "invalid worldgen data: {message}"),
        }
    }
}

impl std::error::Error for WorldgenError {}

/// A parsed world-generation data root: noise definitions, density
/// function documents, and `noise_settings` documents, all keyed by
/// their datapack identifier.
#[derive(Debug)]
pub struct WorldgenData {
    noise: HashMap<String, NoiseParameters>,
    density_documents: HashMap<String, Value>,
    material_rules: HashMap<String, Value>,
    material_conditions: HashMap<String, Value>,
    noise_settings: HashMap<String, Value>,
}

impl WorldgenData {
    /// Loads every `worldgen/noise`, `worldgen/density_function`,
    /// `worldgen/material_rule`, `worldgen/material_condition`, and
    /// `worldgen/noise_settings` JSON file under `root/data`.
    pub fn load(root: &Path) -> Result<Self, WorldgenError> {
        let mut data = Self {
            noise: HashMap::new(),
            density_documents: HashMap::new(),
            material_rules: HashMap::new(),
            material_conditions: HashMap::new(),
            noise_settings: HashMap::new(),
        };
        let data_dir = root.join("data");
        let Some(namespaces) = read_dir_optional(&data_dir)? else {
            // A pack without a data directory simply contributes nothing.
            return Ok(data);
        };
        for namespace in namespaces {
            let namespace =
                namespace.map_err(|error| WorldgenError::Io(data_dir.clone(), error))?;
            if !namespace
                .file_type()
                .map_err(|error| WorldgenError::Io(namespace.path(), error))?
                .is_dir()
            {
                continue;
            }
            let worldgen = namespace.path().join("worldgen");
            if !worldgen.is_dir() {
                continue;
            }
            let ns = namespace.file_name().to_string_lossy().into_owned();
            for entry in walk_json(&worldgen.join("noise"))? {
                let id = format!("{ns}:{}", stem(&entry));
                data.noise.insert(id, parse_noise_definition(&entry)?);
            }
            for entry in walk_json(&worldgen.join("density_function"))? {
                let id = format!(
                    "{ns}:{}",
                    relative_stem(&entry, &worldgen.join("density_function"))
                );
                data.density_documents.insert(id, read_json(&entry)?);
            }
            for registry in ["material_rule", "material_condition"] {
                let documents = if registry == "material_rule" {
                    &mut data.material_rules
                } else {
                    &mut data.material_conditions
                };
                let base = worldgen.join(registry);
                for entry in walk_json(&base)? {
                    let id = format!("{ns}:{}", relative_stem(&entry, &base));
                    documents.insert(id, read_json(&entry)?);
                }
            }
            for entry in walk_json(&worldgen.join("noise_settings"))? {
                let id = format!("{ns}:{}", stem(&entry));
                data.noise_settings.insert(id, read_json(&entry)?);
            }
        }
        Ok(data)
    }

    pub fn noise_definitions(&self) -> &HashMap<String, NoiseParameters> {
        &self.noise
    }

    pub fn density_documents(&self) -> &HashMap<String, Value> {
        &self.density_documents
    }

    pub fn material_rule_documents(&self) -> &HashMap<String, Value> {
        &self.material_rules
    }

    pub fn material_condition_documents(&self) -> &HashMap<String, Value> {
        &self.material_conditions
    }

    pub fn noise_settings(&self, id: &str) -> Option<&Value> {
        self.noise_settings.get(id)
    }

    /// Builds the world-seeded noise engine from these definitions. The
    /// seed is the raw signed world seed; the bit pattern is what the
    /// vanilla seeding chain consumes.
    pub fn engine(&self, world_seed: i64) -> NoiseEngine {
        let mut root = RandomSource::from_world_seed(world_seed as u64);
        let positional = root.fork_positional();
        NoiseEngine::new(positional, self.noise.clone())
    }

    /// A density registry over these documents for the given engine.
    pub fn registry<'a>(&'a self, engine: &'a NoiseEngine) -> DensityRegistry<'a> {
        DensityRegistry::new(engine, self.density_documents.clone())
    }

    /// The evaluated fields of a `noise_settings` document that the
    /// chunk generator needs: dimension bounds, sea level, and the
    /// compiled `noise_router` entries.
    pub fn router<'a>(
        &'a self,
        registry: &'a DensityRegistry<'a>,
        settings_id: &str,
    ) -> Result<NoiseRouter, WorldgenError> {
        let settings = self.noise_settings(settings_id).ok_or_else(|| {
            WorldgenError::Invalid(format!("missing noise settings {settings_id}"))
        })?;
        NoiseRouter::from_settings(settings, settings_id, registry)
    }
}

/// The `noise_router` wiring of a dimension plus the numeric bounds the
/// generator needs. Field semantics follow the public datapack format.
pub struct NoiseRouter {
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
    /// The settings `default_block` id: what the chunk filler writes at
    /// solid positions before material rules run (the initial-fill block
    /// the surface rules replace).
    pub default_block: String,
    pub default_fluid: Fluid,
    pub final_density: Density,
    pub continents: Density,
    pub erosion: Density,
    pub depth: Density,
    pub ridges: Density,
    pub temperature: Density,
    pub vegetation: Density,
    pub chunk_surface_level: Option<Density>,
    /// The `material_rule` registry reference of the settings document,
    /// when present: the root of the surface/underground rule tree.
    pub material_rule: Option<String>,
    /// The compiled `aquifers` section when the settings provide one.
    pub aquifers: Option<AquiferConfig>,
}

// The compiled densities are opaque graphs; the debug view shows the
// numeric bounds and which slots are wired.
impl std::fmt::Debug for NoiseRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoiseRouter")
            .field("min_y", &self.min_y)
            .field("height", &self.height)
            .field("sea_level", &self.sea_level)
            .field(
                "chunk_surface_level",
                &self
                    .chunk_surface_level
                    .as_ref()
                    .map_or("absent", |_| "present"),
            )
            .finish_non_exhaustive()
    }
}

impl NoiseRouter {
    fn from_settings(
        settings: &Value,
        settings_id: &str,
        registry: &DensityRegistry<'_>,
    ) -> Result<Self, WorldgenError> {
        let top = settings
            .as_object()
            .ok_or_else(|| WorldgenError::Invalid(format!("{settings_id}: not an object")))?;
        let noise = top
            .get("noise")
            .and_then(Value::as_object)
            .ok_or_else(|| WorldgenError::Invalid(format!("{settings_id}: missing noise")))?;
        let min_y = required_int(noise, "min_y", settings_id)?;
        let height = required_int(noise, "height", settings_id)?;
        if height <= 0 || height > i32::from(u16::MAX) {
            return Err(WorldgenError::Invalid(format!(
                "{settings_id}: noise height out of range"
            )));
        }
        let sea_level = required_int(top, "sea_level", settings_id)?;
        let default_block = top
            .get("default_block")
            .and_then(Value::as_str)
            .unwrap_or("minecraft:stone")
            .to_owned();
        let default_fluid = top
            .get("default_fluid")
            .map(|value| {
                value.as_str().map(parse_fluid_name).ok_or_else(|| {
                    WorldgenError::Invalid(format!("{settings_id}: default_fluid not a string"))
                })
            })
            .transpose()?
            .unwrap_or(Fluid::Water);
        let router = top
            .get("noise_router")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                WorldgenError::Invalid(format!("{settings_id}: missing noise_router"))
            })?;
        let compile = |field: &str| -> Result<Density, WorldgenError> {
            let value = router
                .get(field)
                .ok_or_else(|| WorldgenError::Invalid(format!("{settings_id}: missing {field}")))?;
            registry
                .compile_slot(value)
                .map_err(|error| WorldgenError::Invalid(format!("{settings_id}: {error}")))
        };
        let compile_optional = |field: &str| -> Result<Option<Density>, WorldgenError> {
            match router.get(field) {
                Some(value) => registry
                    .compile_slot(value)
                    .map(Some)
                    .map_err(|error| WorldgenError::Invalid(format!("{settings_id}: {error}"))),
                None => Ok(None),
            }
        };
        let aquifers = match top.get("aquifers") {
            None => None,
            Some(value) => {
                let section = value.as_object().ok_or_else(|| {
                    WorldgenError::Invalid(format!("{settings_id}: aquifers not an object"))
                })?;
                let field = |name: &str| -> Result<Density, WorldgenError> {
                    let entry = section.get(name).ok_or_else(|| {
                        WorldgenError::Invalid(format!("{settings_id}: aquifers missing {name}"))
                    })?;
                    registry
                        .compile_slot(entry)
                        .map_err(|error| WorldgenError::Invalid(format!("{settings_id}: {error}")))
                };
                Some(AquiferConfig {
                    barrier: field("barrier")?,
                    fluid_level_floodedness: field("fluid_level_floodedness")?,
                    fluid_level_spread: field("fluid_level_spread")?,
                    lava: field("lava")?,
                    exclusion: field("exclusion")?,
                    surface_level: field("surface_level")?,
                })
            }
        };
        let material_rule = top
            .get("material_rule")
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    WorldgenError::Invalid(format!("{settings_id}: material_rule not a string"))
                })
            })
            .transpose()?;
        Ok(Self {
            min_y,
            height,
            sea_level,
            default_block,
            default_fluid,
            final_density: compile("final_density")?,
            continents: compile("continents")?,
            erosion: compile("erosion")?,
            depth: compile("depth")?,
            ridges: compile("ridges")?,
            temperature: compile("temperature")?,
            vegetation: compile("vegetation")?,
            chunk_surface_level: compile_optional("chunk_surface_level")?,
            material_rule,
            aquifers,
        })
    }
}

/// Fluid kind from a namespaced block name; anything not named lava is
/// treated as the dimension's ordinary flooding fluid.
fn parse_fluid_name(name: &str) -> Fluid {
    if name.contains("lava") {
        Fluid::Lava
    } else {
        Fluid::Water
    }
}

/// Cross-checks that every `noise_router`-adjacent id referenced by the
/// settings documents exists, returning the offending references. Used
/// by tests and diagnostics; an empty vector means the wiring resolves.
pub fn unresolved_references(data: &WorldgenData, settings_id: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if let Some(settings) = data.noise_settings(settings_id) {
        for section in ["noise_router", "aquifers"] {
            if let Some(router) = settings.get(section).and_then(Value::as_object) {
                for (_, value) in router {
                    collect_missing(data, value, &mut missing);
                }
            }
        }
    }
    missing
}

fn collect_missing(data: &WorldgenData, value: &Value, missing: &mut Vec<String>) {
    match value {
        Value::String(id) => {
            // Only namespaced strings are registry references; other
            // string fields (`type`, `axis`, spline knobs) are literals.
            if id.contains(':')
                && !data.density_documents.contains_key(id)
                && !builtin_density_ids().iter().any(|builtin| *builtin == id)
            {
                missing.push(id.clone());
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                if key == "type" || key == "noise" {
                    // `noise` names a noise-registry definition, not a
                    // density function.
                    continue;
                }
                collect_missing(data, child, missing);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_missing(data, child, missing);
            }
        }
        _ => {}
    }
}

/// Parses a `worldgen/noise` definition file into the resolved octave
/// parameters consumed by `NormalNoise`.
pub fn parse_noise_definition(path: &Path) -> Result<NoiseParameters, WorldgenError> {
    let document = read_json(path)?;
    let object = document
        .as_object()
        .ok_or_else(|| WorldgenError::Invalid(format!("{}: not an object", path.display())))?;
    let number = |key: &str, default: f64| -> Result<f64, WorldgenError> {
        match object.get(key) {
            Some(value) => value.as_f64().ok_or_else(|| {
                WorldgenError::Invalid(format!("{}: {key} not a number", path.display()))
            }),
            None => Ok(default),
        }
    };
    let integer = |key: &str| -> Result<i32, WorldgenError> {
        object
            .get(key)
            .and_then(Value::as_i64)
            .map(|v| v as i32)
            .ok_or_else(|| WorldgenError::Invalid(format!("{}: {key} missing", path.display())))
    };
    let base_octave = integer("base_octave")?;
    let mut amplitude_modifiers = match object.get("amplitude_modifiers") {
        Some(value) => value
            .as_array()
            .ok_or_else(|| {
                WorldgenError::Invalid(format!(
                    "{}: amplitude_modifiers not an array",
                    path.display()
                ))
            })?
            .iter()
            .map(|item| {
                item.as_f64().ok_or_else(|| {
                    WorldgenError::Invalid(format!(
                        "{}: amplitude modifier not a number",
                        path.display()
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };
    // The resolved codec omits defaults: `octave_count` 1 and identity
    // modifiers are absent from single-octave definitions.
    let octave_count = match object.get("octave_count") {
        Some(value) => value.as_i64().map(|v| v as i32).ok_or_else(|| {
            WorldgenError::Invalid(format!("{}: octave_count not an int", path.display()))
        })?,
        None => 1.max(amplitude_modifiers.len() as i32),
    };
    if octave_count <= 0 || octave_count > 32 {
        return Err(WorldgenError::Invalid(format!(
            "{}: octave_count out of range",
            path.display()
        )));
    }
    let normalize = match object.get("normalize") {
        Some(value) => value.as_bool().ok_or_else(|| {
            WorldgenError::Invalid(format!("{}: normalize not a bool", path.display()))
        })?,
        None => true,
    };
    amplitude_modifiers.resize(octave_count as usize, 1.0);
    Ok(NoiseParameters {
        base_amplitude: number("base_amplitude", 1.0)?,
        base_octave,
        octave_count: octave_count as usize,
        normalize,
        amplitude_modifiers,
    })
}

fn required_int(
    object: &serde_json::Map<String, Value>,
    key: &str,
    location: &str,
) -> Result<i32, WorldgenError> {
    object
        .get(key)
        .and_then(Value::as_i64)
        .map(|value| value as i32)
        .ok_or_else(|| WorldgenError::Invalid(format!("{location}: {key} missing or not an int")))
}

/// Recursively lists `*.json` files under `dir`; a missing directory
/// simply yields nothing.
pub(crate) fn walk_json(dir: &Path) -> Result<Vec<PathBuf>, WorldgenError> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if let Some(entries) = read_dir_optional(&current)? {
            for entry in entries {
                let entry: DirEntry =
                    entry.map_err(|error| WorldgenError::Io(current.clone(), error))?;
                let path = entry.path();
                if entry
                    .file_type()
                    .map_err(|e| WorldgenError::Io(path.clone(), e))?
                    .is_dir()
                {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "json") {
                    found.push(path);
                }
            }
        }
    }
    found.sort();
    Ok(found)
}

pub(crate) fn read_dir_optional(path: &Path) -> Result<Option<std::fs::ReadDir>, WorldgenError> {
    match fs::read_dir(path) {
        Ok(entries) => Ok(Some(entries)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(WorldgenError::Io(path.to_path_buf(), error)),
    }
}

pub(crate) fn read_json(path: &Path) -> Result<Value, WorldgenError> {
    let text =
        fs::read_to_string(path).map_err(|error| WorldgenError::Io(path.to_path_buf(), error))?;
    serde_json::from_str(&text).map_err(|error| WorldgenError::Json(path.to_path_buf(), error))
}

pub(crate) fn stem(path: &Path) -> String {
    path.file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
}

fn relative_stem(path: &Path, base: &Path) -> String {
    let relative = path.strip_prefix(base).unwrap_or(path);
    let without_extension = relative.with_extension("");
    without_extension
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests;
