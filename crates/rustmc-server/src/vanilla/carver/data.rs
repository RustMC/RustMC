//! The carver registry and its JSON parsers.

use super::*;

/// Carver registry documents plus the per-biome carver lists, resolved
/// once from the operator-provisioned data root.
#[derive(Debug, Default)]
pub struct CarverData {
    pub(super) carvers: HashMap<String, Rc<Carver>>,
    pub(super) biomes: HashMap<String, Rc<Vec<Option<Rc<Carver>>>>>,
}

pub(super) fn resolve_biome_carvers(
    biome_id: &str,
    document: &Value,
    known_ids: &HashSet<String>,
    supported: &HashMap<String, Rc<Carver>>,
) -> Result<Vec<Option<Rc<Carver>>>, WorldgenError> {
    let Some(entries) = document.get("carvers") else {
        return Ok(Vec::new());
    };
    // The configured-carver holder accepts a single identifier as well as
    // an array. Preserve source positions in either representation.
    let one = std::slice::from_ref(entries);
    let entries = entries.as_array().map(Vec::as_slice).unwrap_or(one);
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let name = entry.as_str().ok_or_else(|| {
                WorldgenError::Invalid(format!(
                    "{biome_id}: carvers[{index}] must be an identifier"
                ))
            })?;
            if !known_ids.contains(name) {
                return Err(WorldgenError::Invalid(format!(
                    "{biome_id}: carvers[{index}] references unknown {name}"
                )));
            }
            // Unsupported dimension-specific carvers keep their original
            // index, so later supported carvers retain their random seed.
            Ok(supported.get(name).map(Rc::clone))
        })
        .collect()
}

impl CarverData {
    /// Loads `data/<ns>/worldgen/carver/*.json` and
    /// `data/<ns>/worldgen/biome/*.json` under the data root. Carver
    /// types without a runtime implementation here (the nether family)
    /// retain empty positions in biome lists to preserve subsequent indices.
    pub fn load(root: &Path) -> Result<Self, WorldgenError> {
        let mut data = Self::default();
        let data_dir = root.join("data");
        let Some(namespaces) = read_dir_optional(&data_dir)? else {
            return Ok(data);
        };
        let mut carver_docs: Vec<(String, Value)> = Vec::new();
        let mut biome_docs: Vec<(String, Value)> = Vec::new();
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
            for entry in walk_json(&worldgen.join("carver"))? {
                carver_docs.push((format!("{ns}:{}", stem(&entry)), read_json(&entry)?));
            }
            for entry in walk_json(&worldgen.join("biome"))? {
                biome_docs.push((format!("{ns}:{}", stem(&entry)), read_json(&entry)?));
            }
        }
        let known_ids: HashSet<String> = carver_docs.iter().map(|(id, _)| id.clone()).collect();
        for (id, document) in carver_docs {
            let Some(carver) = parse_carver(&id, &document)? else {
                continue; // unsupported carver type
            };
            data.carvers.insert(id, Rc::new(carver));
        }
        for (id, document) in biome_docs {
            let list = resolve_biome_carvers(&id, &document, &known_ids, &data.carvers)?;
            data.biomes.insert(id, Rc::new(list));
        }
        Ok(data)
    }

    pub fn is_empty(&self) -> bool {
        self.biomes.is_empty() && self.carvers.is_empty()
    }

    pub fn carvers_for_biome(&self, biome_id: &str) -> Option<Rc<Vec<Option<Rc<Carver>>>>> {
        self.biomes.get(biome_id).map(Rc::clone)
    }
}

/// Parses one carver registry document; `None` for types this module
/// does not implement.
pub(super) fn parse_carver(id: &str, document: &Value) -> Result<Option<Carver>, WorldgenError> {
    let kind = document
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match kind {
        "minecraft:cave" => parse_cave_carver(document)
            .map(|carver| Some(Carver::Cave(carver)))
            .map_err(|message| WorldgenError::Invalid(format!("{id}: {message}"))),
        "minecraft:canyon" => parse_canyon_carver(document)
            .map(|carver| Some(Carver::Canyon(carver)))
            .map_err(|message| WorldgenError::Invalid(format!("{id}: {message}"))),
        _ => Ok(None),
    }
}

pub(super) fn parse_cave_carver(document: &Value) -> Result<CaveCarver, String> {
    Ok(CaveCarver {
        probability: probability(document),
        y: parse_height(document, "y")?,
        count: parse_int(document.get("count").ok_or("missing count")?)?,
        thickness: parse_float(document.get("thickness").ok_or("missing thickness")?)?,
        weird_thickness_bias: document
            .get("weird_thickness_bias")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        room_vertical: parse_float(
            document
                .get("room_vertical_radius_multiplier")
                .ok_or("missing room_vertical_radius_multiplier")?,
        )?,
        horizontal_multiplier: parse_float(
            document
                .get("horizontal_radius_multiplier")
                .ok_or("missing horizontal_radius_multiplier")?,
        )?,
        vertical_multiplier: parse_float(
            document
                .get("vertical_radius_multiplier")
                .ok_or("missing vertical_radius_multiplier")?,
        )?,
        start_vertical: document
            .get("start_vertical_radius_multiplier")
            .map(parse_float)
            .transpose()?
            .unwrap_or(FloatSpec::Constant(1.0)),
        floor: parse_float(document.get("floor_level").ok_or("missing floor_level")?)?,
    })
}

pub(super) fn parse_canyon_carver(document: &Value) -> Result<CanyonCarver, String> {
    let shape = document.get("shape").ok_or("missing shape")?;
    let number = |object: &Value, field: &str| -> Result<f32, String> {
        object
            .get(field)
            .and_then(Value::as_f64)
            .map(|value| value as f32)
            .ok_or_else(|| format!("missing {field}"))
    };
    Ok(CanyonCarver {
        probability: probability(document),
        y: parse_height(document, "y")?,
        vertical_rotation: parse_float(
            document
                .get("vertical_rotation")
                .ok_or("missing vertical_rotation")?,
        )?,
        shape: CanyonShape {
            distance_factor: parse_float(
                shape
                    .get("distance_factor")
                    .ok_or("missing distance_factor")?,
            )?,
            thickness: parse_float(shape.get("thickness").ok_or("missing thickness")?)?,
            width_smoothness: shape
                .get("width_smoothness")
                .and_then(Value::as_i64)
                .ok_or("missing width_smoothness")? as i32,
            horizontal_factor: parse_float(
                shape
                    .get("horizontal_radius_factor")
                    .ok_or("missing horizontal_radius_factor")?,
            )?,
            vertical_default: number(shape, "vertical_radius_default_factor")?,
            vertical_center: number(shape, "vertical_radius_center_factor")?,
            y_scale: parse_float(shape.get("y_scale").ok_or("missing y_scale")?)?,
        },
    })
}

pub(super) fn probability(document: &Value) -> f32 {
    document
        .get("probability")
        .and_then(Value::as_f64)
        .map_or(0.0, |value| value as f32)
}

pub(super) fn parse_anchor(value: &Value) -> Result<Anchor, String> {
    let object = value.as_object().ok_or("anchor must be an object")?;
    let (key, amount) = object
        .iter()
        .next()
        .and_then(|(key, value)| value.as_i64().map(|value| (key.as_str(), value)))
        .ok_or("anchor needs one integer field")?;
    match key {
        "absolute" => Ok(Anchor::Absolute(amount as i32)),
        "above_bottom" => Ok(Anchor::AboveBottom(amount as i32)),
        "below_top" => Ok(Anchor::BelowTop(amount as i32)),
        "relative_to_sea_level" => Ok(Anchor::RelativeToSeaLevel(amount as i32)),
        other => Err(format!("unknown anchor {other}")),
    }
}

pub(super) fn parse_height(document: &Value, field: &str) -> Result<HeightSpec, String> {
    let height = document
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("missing {field}"))?;
    let kind = height
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{field} needs a type"))?;
    if kind != "minecraft:uniform" {
        return Err(format!("unsupported height provider {kind}"));
    }
    Ok(HeightSpec {
        min: parse_anchor(
            height
                .get("min_inclusive")
                .ok_or_else(|| format!("{field} missing min_inclusive"))?,
        )?,
        max: parse_anchor(
            height
                .get("max_inclusive")
                .ok_or_else(|| format!("{field} missing max_inclusive"))?,
        )?,
    })
}

pub(super) fn parse_int(value: &Value) -> Result<IntSpec, String> {
    if let Some(number) = value.as_i64() {
        return Ok(IntSpec::Constant(number as i32));
    }
    dispatch(value, |kind, object| match kind {
        "minecraft:constant" => object
            .get("value")
            .and_then(Value::as_i64)
            .map(|value| IntSpec::Constant(value as i32))
            .ok_or_else(|| "constant needs a value".to_owned()),
        "minecraft:uniform" | "minecraft:very_biased_to_bottom" => {
            let min = object
                .get("min_inclusive")
                .and_then(Value::as_i64)
                .ok_or("uniform needs min_inclusive")? as i32;
            let max = object
                .get("max_inclusive")
                .and_then(Value::as_i64)
                .ok_or("uniform needs max_inclusive")? as i32;
            if kind == "minecraft:uniform" {
                Ok(IntSpec::Uniform { min, max })
            } else {
                Ok(IntSpec::VeryBiasedToBottom { min, max })
            }
        }
        other => Err(format!("unsupported int provider {other}")),
    })
}

pub(super) fn parse_float(value: &Value) -> Result<FloatSpec, String> {
    if let Some(number) = value.as_f64() {
        return Ok(FloatSpec::Constant(number as f32));
    }
    dispatch(value, |kind, object| match kind {
        "minecraft:constant" => object
            .get("value")
            .and_then(Value::as_f64)
            .map(|value| FloatSpec::Constant(value as f32))
            .ok_or_else(|| "constant needs a value".to_owned()),
        "minecraft:uniform" => {
            let min = object
                .get("min_inclusive")
                .and_then(Value::as_f64)
                .ok_or("uniform needs min_inclusive")? as f32;
            let max = object
                .get("max_exclusive")
                .and_then(Value::as_f64)
                .ok_or("uniform needs max_exclusive")? as f32;
            Ok(FloatSpec::Uniform { min, max })
        }
        "minecraft:trapezoid" => {
            let min = object
                .get("min")
                .and_then(Value::as_f64)
                .ok_or("trapezoid needs min")? as f32;
            let max = object
                .get("max")
                .and_then(Value::as_f64)
                .ok_or("trapezoid needs max")? as f32;
            let plateau = object
                .get("plateau")
                .and_then(Value::as_f64)
                .ok_or("trapezoid needs plateau")? as f32;
            Ok(FloatSpec::Trapezoid { min, max, plateau })
        }
        other => Err(format!("unsupported float provider {other}")),
    })
}

/// Reads a provider object's `type` and hands the body to the matcher.
pub(super) fn dispatch<T>(
    value: &Value,
    build: impl Fn(&str, &serde_json::Map<String, Value>) -> Result<T, String>,
) -> Result<T, String> {
    let object = value
        .as_object()
        .ok_or("provider must be a value or object")?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or("provider needs a type")?;
    build(kind, object)
}
