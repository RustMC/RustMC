//! Compilation of density-function JSON into evaluation trees, with the
//! shared noise engine and reference resolution.

use super::*;

/// World-seeded noise provider: the positional factory derived from the
/// world seed, plus the noise registry. Noise instances are created once
/// per identifier and shared, matching the documented world-level caching.
pub struct NoiseEngine {
    pub(super) positional: PositionalRandomFactory,
    pub(super) definitions: HashMap<Id, NoiseParameters>,
    pub(super) stacks: RefCell<HashMap<Id, Rc<NoiseStack>>>,
}

impl NoiseEngine {
    pub fn new(
        positional: PositionalRandomFactory,
        definitions: HashMap<Id, NoiseParameters>,
    ) -> Self {
        Self {
            positional,
            definitions,
            stacks: RefCell::new(HashMap::new()),
        }
    }

    /// The root positional random factory derived from the world seed.
    /// Later phases derive named factories from it exactly as the
    /// documented world-seed chain does (for example the aquifer grid).
    pub fn positional(&self) -> PositionalRandomFactory {
        self.positional
    }

    /// Raw noise for a datapack `worldgen/noise` identifier, seeded from
    /// the world factory exactly as the documented `getOrCreateNoise`
    /// caching does. Surface rules sample these directly.
    pub fn noise_stack(&self, id: &str) -> Result<Rc<NoiseStack>, DensityError> {
        self.stack(&id.to_owned())
    }

    pub(super) fn stack(&self, id: &Id) -> Result<Rc<NoiseStack>, DensityError> {
        if let Some(stack) = self.stacks.borrow().get(id) {
            return Ok(Rc::clone(stack));
        }
        let parameters = self
            .definitions
            .get(id)
            .ok_or_else(|| DensityError::Unresolved(id.clone()))?
            .clone();
        let mut random = self.positional.from_hash_of(id);
        let stack = Rc::new(NormalNoise::new(&parameters).create(&mut random));
        self.stacks
            .borrow_mut()
            .insert(id.clone(), Rc::clone(&stack));
        Ok(stack)
    }

    pub(super) fn random(&self, id: &str) -> RandomSource {
        self.positional.from_hash_of(id)
    }
}

/// Density JSON documents keyed by identifier.
pub struct DensityRegistry<'e> {
    pub(super) engine: &'e NoiseEngine,
    pub(super) documents: HashMap<Id, Value>,
    pub(super) compiled: RefCell<HashMap<Id, Density>>,
    pub(super) compiling: RefCell<HashSet<Id>>,
}

// A reference chain also nests Rust parser frames. Keep the limit below the
// default test-thread stack while accepting the provisioned 26.3 graph.
pub(super) const MAX_REFERENCE_DEPTH: usize = 32;

impl<'e> DensityRegistry<'e> {
    pub fn new(engine: &'e NoiseEngine, documents: HashMap<Id, Value>) -> Self {
        Self {
            engine,
            documents,
            compiled: RefCell::new(HashMap::new()),
            compiling: RefCell::new(HashSet::new()),
        }
    }

    /// Resolves a documented density-function identifier (built-in or
    /// datapack-provided) into a compiled graph.
    pub fn compile(&self, id: &Id) -> Result<Density, DensityError> {
        if let Some(existing) = self.compiled.borrow().get(id) {
            return Ok(existing.clone());
        }
        if self.compiling.borrow().len() >= MAX_REFERENCE_DEPTH {
            return Err(DensityError::Parse(format!(
                "density reference depth exceeds {MAX_REFERENCE_DEPTH} at {id}"
            )));
        }
        if !self.compiling.borrow_mut().insert(id.clone()) {
            return Err(DensityError::Parse(format!(
                "cyclic density reference: {id}"
            )));
        }
        let result = if let Some(document) = self.documents.get(id) {
            self.compile_value(document)
        } else {
            self.compile_builtin(id)
        };
        self.compiling.borrow_mut().remove(id);
        let density = result?;
        self.compiled
            .borrow_mut()
            .insert(id.clone(), density.clone());
        Ok(density)
    }

    /// Compiles one child-slot value: an object node, a reference
    /// string resolved through the registry, or a bare number constant.
    pub fn compile_slot(&self, value: &Value) -> Result<Density, DensityError> {
        self.child(value)
    }

    /// Compiles a detached JSON document (not stored in the registry).
    pub fn compile_value(&self, value: &Value) -> Result<Density, DensityError> {
        // Registry document roots may be bare numbers: the datapack codec
        // reads them as constant functions (26.3 `zero.json` is `0.0`).
        if let Value::Number(_) = value {
            return Ok(Density::constant(noise_value(value)?));
        }
        let node = self.parse(value)?;
        Ok(Density(Rc::new(node)))
    }

    pub(super) fn compile_builtin(&self, id: &Id) -> Result<Density, DensityError> {
        let node = match id.as_str() {
            "minecraft:zero" => Node::Constant(0.0),
            // Registered by the vanilla bootstrap as a clamped identity
            // gradient over the doubled block-position range; independent
            // of any dimension (`PROVENANCE.md`: 26.3 packs Y in 12 bits,
            // `Y_SIZE = (1 << 12) - 32 = 4064`, `MIN_Y = -2032`,
            // `MAX_Y = 2031`).
            "minecraft:y" => Node::Axial {
                axis: Axis::Y,
                tiling: Tiling::ClampToEdge,
                from_coordinate: BUILTIN_Y_FROM,
                coordinate_range: BUILTIN_Y_TO - BUILTIN_Y_FROM,
                from_value: BUILTIN_Y_FROM as f32,
                coordinate_factor: 1.0,
            },
            "minecraft:shift_x" => cache_node(Density(Rc::new(Node::ShiftNoise {
                kind: ShiftKind::Xy,
                stack: self.engine.stack(&"minecraft:shift".to_owned())?,
                horizontal: RefCell::new(BoundedCache::new(512)),
            }))),
            "minecraft:shift_z" => cache_node(Density(Rc::new(Node::ShiftNoise {
                kind: ShiftKind::Z,
                stack: self.engine.stack(&"minecraft:shift".to_owned())?,
                horizontal: RefCell::new(BoundedCache::new(512)),
            }))),
            _ => return Err(DensityError::Unresolved(id.clone())),
        };
        Ok(Density(Rc::new(node)))
    }

    /// Parses one child slot: an object node, a `"namespace:path"`
    /// reference, or a bare number constant.
    pub(super) fn child(&self, value: &Value) -> Result<Density, DensityError> {
        match value {
            Value::Object(_) => self.compile_value(value),
            Value::String(id) => self.compile(id),
            Value::Number(_) => Ok(Density::constant(noise_value(value)?)),
            other => Err(DensityError::Parse(format!(
                "expected density node, got {other}"
            ))),
        }
    }

    pub(super) fn parse(&self, value: &Value) -> Result<Node, DensityError> {
        let object = value
            .as_object()
            .ok_or_else(|| DensityError::Parse("density node must be an object".to_owned()))?;
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| DensityError::Parse("missing \"type\"".to_owned()))?;
        let kind = kind.strip_prefix("minecraft:").unwrap_or(kind);
        let get = |key: &str| {
            object
                .get(key)
                .ok_or_else(|| DensityError::Parse(format!("{kind} missing {key:?}")))
        };
        let node = match kind {
            "constant" => Node::Constant(noise_value(get("value")?)?),
            "noise" => Node::Noise {
                stack: self.engine.stack(&referenced_id(get("noise")?)?)?,
                xz_scale: get("xz_scale")?
                    .as_f64()
                    .ok_or_else(|| DensityError::Parse("xz_scale must be a number".to_owned()))?,
                y_scale: get("y_scale")?
                    .as_f64()
                    .ok_or_else(|| DensityError::Parse("y_scale must be a number".to_owned()))?,
                shift_x: self
                    .child(object.get("shift_x").unwrap_or(&Value::Null).default_zero())?,
                shift_y: self
                    .child(object.get("shift_y").unwrap_or(&Value::Null).default_zero())?,
                shift_z: self
                    .child(object.get("shift_z").unwrap_or(&Value::Null).default_zero())?,
            },
            name @ ("shift" | "shift_a" | "shift_b") => Node::ShiftNoise {
                kind: match name {
                    "shift" => ShiftKind::X,
                    "shift_a" => ShiftKind::Xy,
                    _ => ShiftKind::Z,
                },
                stack: self.engine.stack(&referenced_id(get("noise")?)?)?,
                horizontal: RefCell::new(BoundedCache::new(512)),
            },
            "gradient" => {
                let axis = Axis::parse(
                    get("axis")?
                        .as_str()
                        .ok_or_else(|| DensityError::Parse("axis must be a string".to_owned()))?,
                )?;
                let tiling = match get("tiling") {
                    Ok(Value::String(name)) => Tiling::parse(name)?,
                    Ok(Value::Null) | Err(_) => Tiling::ClampToEdge,
                    Ok(other) => return Err(DensityError::Parse(format!("bad tiling {other}"))),
                };
                let from_coordinate = get("from_coordinate")?
                    .as_i64()
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| {
                        DensityError::Parse("from_coordinate must be a 32-bit int".to_owned())
                    })?;
                let to_coordinate = get("to_coordinate")?
                    .as_i64()
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| {
                        DensityError::Parse("to_coordinate must be a 32-bit int".to_owned())
                    })?;
                let from_value = noise_value(get("from_value")?)?;
                let to_value = noise_value(get("to_value")?)?;
                let coordinate_range = to_coordinate
                    .checked_sub(from_coordinate)
                    .filter(|value| *value != 0)
                    .ok_or_else(|| {
                        DensityError::Parse(
                            "gradient coordinate range must be nonzero and fit a 32-bit int"
                                .to_owned(),
                        )
                    })?;
                Node::Axial {
                    axis,
                    tiling,
                    from_coordinate,
                    coordinate_range,
                    from_value,
                    coordinate_factor: (to_value - from_value) / coordinate_range as f32,
                }
            }
            "blend_alpha" => Node::BlendAlpha,
            "blend_offset" => Node::BlendOffset,
            "beardifier" => Node::Beardifier,
            _ => self.parse_with_input(kind, get, object)?,
        };
        Ok(node)
    }

    pub(super) fn parse_with_input<'v>(
        &self,
        kind: &str,
        get: impl Fn(&str) -> Result<&'v Value, DensityError>,
        object: &'v serde_json::Map<String, Value>,
    ) -> Result<Node, DensityError> {
        let node = match kind {
            "cache" => cache_node(self.child(get("input")?)?),
            "interpolated" => {
                let cell_size_xz = positive_int(get("cell_size_xz")?)?;
                let cell_size_y = positive_int(get("cell_size_y")?)?;
                Node::Interpolated {
                    input: self.child(get("input")?)?,
                    cell_size_xz,
                    cell_size_y,
                    corners: RefCell::new(BoundedCache::new(64)),
                }
            }
            "slice" => Node::Slice {
                axis: Axis::parse(
                    get("axis")?
                        .as_str()
                        .ok_or_else(|| DensityError::Parse("axis must be a string".to_owned()))?,
                )?,
                coordinate: get("coordinate")?
                    .as_i64()
                    .ok_or_else(|| DensityError::Parse("coordinate must be an int".to_owned()))?
                    as i32,
                input: self.child(get("input")?)?,
            },
            "clamp" => Node::Clamp {
                input: self.child(get("input")?)?,
                min: noise_value(get("min")?)?,
                max: noise_value(get("max")?)?,
            },
            "lerp" => Node::Lerp {
                alpha: self.child(get("alpha")?)?,
                first: self.child(get("first")?)?,
                second: self.child(get("second")?)?,
            },
            "range_choice" => Node::RangeChoice {
                input: self.child(get("input")?)?,
                min_inclusive: noise_value(get("min_inclusive")?)?,
                max_exclusive: noise_value(get("max_exclusive")?)?,
                when_in_range: self.child(get("when_in_range")?)?,
                when_out_of_range: self.child(get("when_out_of_range")?)?,
            },
            "interval_select" => {
                let thresholds_value = get("thresholds")?;
                let thresholds: Vec<f32> = thresholds_value
                    .as_array()
                    .ok_or_else(|| DensityError::Parse("thresholds must be an array".to_owned()))?
                    .iter()
                    .map(noise_value)
                    .collect::<Result<_, _>>()?;
                let functions: Vec<Density> = get("functions")?
                    .as_array()
                    .ok_or_else(|| DensityError::Parse("functions must be an array".to_owned()))?
                    .iter()
                    .map(|item| self.child(item))
                    .collect::<Result<_, _>>()?;
                if functions.len() < 2 || thresholds.len() + 1 != functions.len() {
                    return Err(DensityError::Parse(format!(
                        "interval_select wants {} thresholds for {} functions, got {}",
                        functions.len() - 1,
                        functions.len(),
                        thresholds.len()
                    )));
                }
                if thresholds.windows(2).any(|pair| pair[0] > pair[1]) {
                    return Err(DensityError::Parse(
                        "interval_select thresholds must be ordered".to_owned(),
                    ));
                }
                Node::IntervalSelect {
                    input: self.child(get("input")?)?,
                    thresholds: thresholds.into(),
                    functions: functions.into(),
                }
            }
            "spline" => Node::Spline(self.parse_spline(get("spline")?)?),
            "find_top_surface" => Node::FindTopSurface {
                density: self.child(get("density")?)?,
                upper_bound: self.child(get("upper_bound")?)?,
                lower_bound: get("lower_bound")?
                    .as_i64()
                    .ok_or_else(|| DensityError::Parse("lower_bound must be an int".to_owned()))?
                    as i32,
                cell_height: positive_int(get("cell_height")?)?,
            },
            "distance_to_point" => {
                // Vanilla decodes `point` with the vec3 codec, which only
                // accepts the three-element array form.
                let coords = get("point")?
                    .as_array()
                    .ok_or_else(|| DensityError::Parse("point must be a json array".to_owned()))?;
                if coords.len() != 3 {
                    return Err(DensityError::Parse(
                        "point must have exactly three coordinates".to_owned(),
                    ));
                }
                let mut point = [0i32; 3];
                for (slot, value) in point.iter_mut().zip(coords) {
                    *slot = value.as_i64().ok_or_else(|| {
                        DensityError::Parse("point coordinates must be ints".to_owned())
                    })? as i32;
                }
                let metric =
                    DistanceMetric::parse(get("metric")?.as_str().ok_or_else(|| {
                        DensityError::Parse("metric must be a string".to_owned())
                    })?)?;
                Node::DistanceToPoint { point, metric }
            }
            "blend_density" => Node::BlendDensity(self.child(get("input")?)?),
            "old_blended_noise" => Node::Blended(self.blended_set(get)?),
            "end_outer_islands" => {
                return Err(DensityError::Unsupported(
                    "end_outer_islands (needs the simplex provider; slice C)".to_owned(),
                ));
            }
            other => {
                if let Some(unary) = UnaryKind::parse(other) {
                    Node::Unary(unary, self.child(get("input")?)?)
                } else if let Some(binary) = BinaryKind::parse(other) {
                    Node::Binary(
                        binary,
                        self.child(get("left")?)?,
                        self.child(get("right")?)?,
                    )
                } else if let Some(round) = RoundKind::parse(other) {
                    let multiple = match object.get("multiple") {
                        None | Some(Value::Null) => Density::constant(1.0),
                        Some(value) => self.child(value)?,
                    };
                    Node::Round {
                        kind: round,
                        input: self.child(get("input")?)?,
                        multiple,
                    }
                } else if other == "pow" {
                    let base = self.child(get("base")?)?;
                    let exponent = self.child(get("exponent")?)?;
                    fold_pow(base, exponent)
                } else {
                    return Err(DensityError::Unsupported(other.to_owned()));
                }
            }
        };
        Ok(node)
    }

    pub(super) fn parse_spline(&self, value: &Value) -> Result<Spline, DensityError> {
        match value {
            Value::Number(_) => Ok(Spline::Constant(noise_value(value)?)),
            Value::Object(object) => {
                let coordinate = object.get("coordinate").ok_or_else(|| {
                    DensityError::Parse("spline missing \"coordinate\"".to_owned())
                })?;
                let points = object
                    .get("points")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        DensityError::Parse("spline missing non-empty \"points\"".to_owned())
                    })?;
                if points.is_empty() {
                    return Err(DensityError::Parse(
                        "spline points must be non-empty".to_owned(),
                    ));
                }
                let mut locations = Vec::with_capacity(points.len());
                let mut values = Vec::with_capacity(points.len());
                let mut derivatives = Vec::with_capacity(points.len());
                for point in points {
                    let point = point.as_object().ok_or_else(|| {
                        DensityError::Parse("spline point must be an object".to_owned())
                    })?;
                    let read = |key: &str| {
                        point.get(key).ok_or_else(|| {
                            DensityError::Parse(format!("spline point missing {key:?}"))
                        })
                    };
                    locations.push(float_field(read("location")?)?);
                    values.push(self.parse_spline(read("value")?)?);
                    derivatives.push(float_field(read("derivative")?)?);
                }
                Ok(Spline::Multipoint {
                    coordinate: self.child(coordinate)?,
                    locations: locations.into(),
                    derivatives: derivatives.into(),
                    values: values.into(),
                })
            }
            other => Err(DensityError::Parse(format!(
                "spline value must be a number or object, got {other}"
            ))),
        }
    }

    pub(super) fn blended_set<'v>(
        &self,
        get: impl Fn(&str) -> Result<&'v Value, DensityError>,
    ) -> Result<BlendedSet, DensityError> {
        let xz_scale = get("xz_scale")?
            .as_f64()
            .ok_or_else(|| DensityError::Parse("xz_scale must be a number".to_owned()))?;
        let y_scale = get("y_scale")?
            .as_f64()
            .ok_or_else(|| DensityError::Parse("y_scale must be a number".to_owned()))?;
        let xz_factor = get("xz_factor")?
            .as_f64()
            .ok_or_else(|| DensityError::Parse("xz_factor must be a number".to_owned()))?;
        let y_factor = get("y_factor")?
            .as_f64()
            .ok_or_else(|| DensityError::Parse("y_factor must be a number".to_owned()))?;
        let smear_scale_multiplier = get("smear_scale_multiplier")?.as_f64().ok_or_else(|| {
            DensityError::Parse("smear_scale_multiplier must be a number".to_owned())
        })?;
        for (name, scale) in [
            ("xz_scale", xz_scale),
            ("y_scale", y_scale),
            ("xz_factor", xz_factor),
            ("y_factor", y_factor),
        ] {
            if !(0.001..=1000.0).contains(&scale) {
                return Err(DensityError::Parse(format!("{name} {scale} out of range")));
            }
        }
        if !(1.0..=8.0).contains(&smear_scale_multiplier) {
            return Err(DensityError::Parse(
                "smear_scale_multiplier out of range".to_owned(),
            ));
        }
        // The three FBMs consume one sequential stream, min limit first.
        let mut random = self.engine.random("minecraft:terrain");
        let xz_multiplier = BASE_SCALE * xz_scale;
        let y_multiplier = BASE_SCALE * y_scale;
        let limit_smear = y_multiplier * smear_scale_multiplier;
        let min = create_blended_fbm(&mut random, -15, limit_smear, f64::from(LIMIT_FACTOR));
        let max = create_blended_fbm(&mut random, -15, limit_smear, f64::from(LIMIT_FACTOR));
        let main = create_blended_fbm(&mut random, -7, limit_smear / y_factor, MAIN_FACTOR);
        Ok(BlendedSet {
            min,
            max,
            main,
            xz_multiplier,
            y_multiplier,
            main_xz_multiplier: xz_multiplier / xz_factor,
            main_y_multiplier: y_multiplier / y_factor,
        })
    }
}

/// `BlendedNoise` construction constants captured in `docs/PROVENANCE.md`.
pub(super) const BASE_SCALE: f64 = 684.412;
pub(super) const LIMIT_FACTOR: f32 = 0.99998474;
pub(super) const MAIN_FACTOR: f64 = 12.75;

/// Built-in `minecraft:y` gradient bounds, `DimensionType.MIN_Y * 2` and
/// `MAX_Y * 2` as established in `docs/PROVENANCE.md`.
pub(super) const BUILTIN_Y_FROM: i32 = -4064;
pub(super) const BUILTIN_Y_TO: i32 = 4062;

pub(super) trait DefaultZero {
    fn default_zero(&self) -> &Value;
}

impl DefaultZero for Value {
    fn default_zero(&self) -> &Value {
        if matches!(self, Value::Null) {
            // The absent shift is a constant zero node; keep one shared
            // instance through the caller's parse path by returning a
            // numeric literal JSON value.
            ZERO_LITERAL
                .get_or_init(|| serde_json::json!({ "type": "minecraft:constant", "value": 0.0 }))
        } else {
            self
        }
    }
}

pub(super) static ZERO_LITERAL: OnceLock<Value> = OnceLock::new();

pub(super) fn referenced_id(value: &Value) -> Result<Id, DensityError> {
    value
        .as_str()
        .map(|id| {
            if id.contains(':') {
                id.to_owned()
            } else {
                format!("minecraft:{id}")
            }
        })
        .ok_or_else(|| DensityError::Parse("expected a resource id string".to_owned()))
}

/// `NOISE_VALUE_CODEC`: float values limited to |v| <= 1e6.
pub(super) fn noise_value(value: &Value) -> Result<f32, DensityError> {
    let raw = value
        .as_f64()
        .ok_or_else(|| DensityError::Parse("expected a number".to_owned()))?;
    let as_float = raw as f32;
    if as_float.abs() > 1.0e6 {
        return Err(DensityError::Parse(format!(
            "noise value {raw} exceeds the codec limit"
        )));
    }
    Ok(as_float)
}

pub(super) fn float_field(value: &Value) -> Result<f32, DensityError> {
    value
        .as_f64()
        .map(|raw| raw as f32)
        .ok_or_else(|| DensityError::Parse("expected a float".to_owned()))
}

pub(super) fn positive_int(value: &Value) -> Result<i32, DensityError> {
    let raw = value
        .as_i64()
        .ok_or_else(|| DensityError::Parse("expected an int".to_owned()))?;
    if raw <= 0 || raw > i64::from(i32::MAX) {
        return Err(DensityError::Parse(format!("{raw} is not positive")));
    }
    Ok(raw as i32)
}

/// Java folds constant-exponent `pow` into dedicated operations at compile
/// time: sqrt/passthrough/square/cube, wrapped in a reciprocal for negative
/// exponents. Anything else keeps the general two-operand `Math.pow`.
pub(super) fn fold_pow(base: Density, exponent: Density) -> Node {
    if let Node::Constant(value) = *exponent.0 {
        let absolute = value.abs();
        let folded = if absolute == 0.5 {
            UnaryKind::Sqrt
        } else if absolute == 1.0 {
            if value > 0.0 {
                return cache_node(base);
            }
            UnaryKind::Reciprocal
        } else if absolute == 2.0 {
            UnaryKind::Square
        } else if absolute == 3.0 {
            UnaryKind::Cube
        } else {
            return Node::PowConstExponent {
                input: base,
                exponent: f64::from(value),
            };
        };
        return if value < 0.0 && folded != UnaryKind::Reciprocal {
            Node::Unary(
                UnaryKind::Reciprocal,
                Density(Rc::new(Node::Unary(folded, base))),
            )
        } else {
            Node::Unary(folded, base)
        };
    }
    if let Node::Constant(value) = *base.0 {
        return Node::PowConstBase {
            base: f64::from(value),
            exponent,
        };
    }
    Node::Pow(base, exponent)
}
