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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A unique scratch directory; all data written into it is
    /// self-authored synthetic numbers, never vendor data.
    fn scratch_root(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustmc-worldgen-{label}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create scratch dir");
        path
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, text).expect("write");
    }

    /// A minimal fabricated datapack mirroring the 26.3 document shapes:
    /// resolved noise definitions, registry-id string references (with a
    /// nested path), a bare-number density document, inline router
    /// objects, and the slim `noise` bounds section.
    fn synthetic_root(label: &str) -> PathBuf {
        let root = scratch_root(label);
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("noise/fabricated.json"),
            r#"{"base_amplitude": 2.0, "base_octave": 3, "octave_count": 2}"#,
        );
        write(
            &worldgen.join("noise/solo.json"),
            r#"{"base_amplitude": 1.5, "base_octave": -2}"#,
        );
        write(&worldgen.join("density_function/zero.json"), "0.0");
        write(
            &worldgen.join("density_function/group/erosion.json"),
            r#"{"type": "noise", "noise": "testns:fabricated", "xz_scale": 0.9, "y_scale": 0.9}"#,
        );
        write(
            &worldgen.join("density_function/depth.json"),
            r#"{"type": "sub", "left": -0.3, "right": "testns:group/erosion"}"#,
        );
        write(
            &worldgen.join("density_function/final.json"),
            r#"{"type": "max", "left": "testns:zero", "right": "testns:depth"}"#,
        );
        write(
            &worldgen.join("noise_settings/fabricated_dimension.json"),
            r#"{
                "noise": {"min_y": -16, "height": 64},
                "sea_level": 20,
                "noise_router": {
                    "final_density": "testns:final",
                    "continents": "testns:zero",
                    "erosion": "testns:group/erosion",
                    "depth": {"type": "mul", "left": 2, "right": "testns:zero"},
                    "ridges": "testns:zero",
                    "temperature": "testns:zero",
                    "vegetation": "testns:zero",
                    "chunk_surface_level": "testns:zero"
                },
                "aquifers": {
                    "barrier": "testns:zero",
                    "fluid_level_floodedness": "testns:zero",
                    "fluid_level_spread": {
                        "type": "noise", "noise": "testns:solo",
                        "xz_scale": 1.0, "y_scale": 1.0
                    },
                    "lava": "testns:zero",
                    "exclusion": "testns:zero",
                    "surface_level": "testns:group/erosion"
                }
            }"#,
        );
        write(
            &worldgen.join("noise_settings/broken_dimension.json"),
            r#"{
                "noise": {"min_y": 0, "height": 32},
                "sea_level": 0,
                "noise_router": {
                    "final_density": "testns:nonexistent",
                    "continents": "minecraft:y",
                    "erosion": "testns:zero",
                    "depth": "testns:zero",
                    "ridges": "testns:zero",
                    "temperature": "testns:zero",
                    "vegetation": "testns:zero"
                }
            }"#,
        );
        root
    }

    #[test]
    fn loads_namespaced_ids_including_nested_paths() {
        let root = synthetic_root("load");
        let data = WorldgenData::load(&root).expect("load");
        let parameters = data
            .noise_definitions()
            .get("testns:fabricated")
            .expect("noise id resolves");
        assert_eq!(parameters.base_amplitude, 2.0);
        assert_eq!(parameters.base_octave, 3);
        assert_eq!(parameters.octave_count, 2);
        assert!(parameters.normalize);
        // Resolved form pads missing modifiers with identity amplitudes.
        assert_eq!(parameters.amplitude_modifiers, vec![1.0, 1.0]);
        // Single-octave definitions omit `octave_count` and modifiers.
        let solo = data
            .noise_definitions()
            .get("testns:solo")
            .expect("solo noise id resolves");
        assert_eq!(solo.base_amplitude, 1.5);
        assert_eq!(solo.base_octave, -2);
        assert_eq!(solo.octave_count, 1);
        assert_eq!(solo.amplitude_modifiers, vec![1.0]);
        assert!(
            data.density_documents()
                .contains_key("testns:group/erosion")
        );
        assert!(data.density_documents().contains_key("testns:zero"));
        assert!(data.noise_settings("testns:fabricated_dimension").is_some());
        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn missing_data_directory_loads_empty() {
        let root = scratch_root("empty");
        let data = WorldgenData::load(&root).expect("load");
        assert!(data.noise_definitions().is_empty());
        assert!(data.density_documents().is_empty());
        assert!(data.noise_settings("anything").is_none());
        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn router_compiles_reference_and_inline_slots() {
        let root = synthetic_root("router");
        let data = WorldgenData::load(&root).expect("load");
        let engine = data.engine(2026);
        let registry = data.registry(&engine);
        let router = data
            .router(&registry, "testns:fabricated_dimension")
            .expect("router");
        assert_eq!(router.min_y, -16);
        assert_eq!(router.height, 64);
        assert_eq!(router.sea_level, 20);
        // An inline object slot compiles independently of the documents.
        let inline = router.depth.sample(4, 8, -2);
        assert_eq!(inline, 0.0);
        let point = (7, 11, -3);
        let erosion = registry
            .compile(&"testns:group/erosion".to_owned())
            .expect("erosion");
        let e = erosion.sample(point.0, point.1, point.2);
        let depth = registry
            .compile(&"testns:depth".to_owned())
            .expect("depth document");
        let d = depth.sample(point.0, point.1, point.2);
        // `sub` with a constant left input is exact f32 subtraction.
        assert_eq!(d, -0.3f32 - e);
        let final_ = router.final_density.sample(point.0, point.1, point.2);
        assert!(final_ == 0.0 || (d > 0.0 && final_ == d));
        // The aquifers section compiles its six density fields; an inline
        // noise node resolves through the noise registry, not density ids.
        assert_eq!(router.default_fluid, Fluid::Water);
        let aquifers = router.aquifers.as_ref().expect("aquifers section");
        assert_eq!(aquifers.barrier.sample(point.0, point.1, point.2), 0.0);
        let spread = aquifers
            .fluid_level_spread
            .sample(point.0, point.1, point.2);
        assert!(spread.is_finite() && spread.abs() <= 2.0);
        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn unresolved_references_reports_missing_ids_only() {
        let root = synthetic_root("refs");
        let data = WorldgenData::load(&root).expect("load");
        let clean = unresolved_references(&data, "testns:fabricated_dimension");
        assert!(clean.is_empty(), "unexpected missing: {clean:?}");
        let broken = unresolved_references(&data, "testns:broken_dimension");
        assert_eq!(broken, vec!["testns:nonexistent".to_owned()]);
        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn router_reports_uncompilable_reference() {
        let root = synthetic_root("badrouter");
        let data = WorldgenData::load(&root).expect("load");
        let engine = data.engine(2026);
        let registry = data.registry(&engine);
        let error = match data.router(&registry, "testns:broken_dimension") {
            Ok(_) => panic!("missing id must not compile"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("testns:nonexistent"),
            "error should name the unresolved id: {error}"
        );
        fs::remove_dir_all(&root).expect("cleanup");
    }

    /// Manual verification against an operator-provisioned datapack root
    /// (never committed; see `docs/PROVENANCE.md`). Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored`
    /// or rely on the default project-local path.
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn smoke_loads_operator_worldgen_data() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
        let data = WorldgenData::load(&root).expect("operator data loads");
        let engine = data.engine(2026);
        let registry = data.registry(&engine);
        let missing = unresolved_references(&data, "minecraft:overworld");
        assert!(missing.is_empty(), "unresolved: {missing:?}");
        let router = data
            .router(&registry, "minecraft:overworld")
            .expect("overworld router");
        assert_eq!(router.min_y, -64);
        assert_eq!(router.height, 384);
        assert_eq!(router.sea_level, 63);
        // The overworld wires its runtime aquifer: six density fields and
        // the water flooding fluid (publicly documented dimension facts).
        assert_eq!(router.default_fluid, Fluid::Water);
        let aquifers = router.aquifers.as_ref().expect("overworld aquifers");
        for sample in [&aquifers.barrier, &aquifers.surface_level] {
            assert!(sample.sample(0, 0, 0).is_finite());
        }
        // Sampling the real final-density graph must produce finite values
        // across the documented vertical range.
        for y in [-64, 0, 64, 200, 319] {
            let v = router.final_density.sample(0, y, 0);
            assert!(v.is_finite(), "final density at y={y}: {v}");
        }
        println!("smoke OK: router compiled from {}", root.display());
    }
}
