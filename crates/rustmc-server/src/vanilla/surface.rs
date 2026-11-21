//! Surface material rules: the data-driven block replacement vanilla
//! applies to the noise-filled column before carvers run (T2 slice G).
//!
//! The rule/condition document shapes and the evaluation semantics were
//! established by knowledge-only consultation of deobfuscated vanilla
//! 26.3 classes (see `docs/PROVENANCE.md`, session 7); this file is an
//! independent Rust implementation. Documents live in the operator-
//! provisioned data pack at `data/<namespace>/worldgen/material_rule/`
//! and `data/<namespace>/worldgen/material_condition/` and are referenced
//! from the dimension settings' `material_rule` field.
//!
//! Scope: the plain-rule language (sequence, condition, block, bandlands,
//! ore_vein) plus the ten overworld condition types. The hardcoded
//! eroded-badlands and frozen-ocean column extensions of the material
//! system are not implemented here (documented residuals).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use serde_json::Value;

use crate::vanilla::density::{Density, DensityRegistry, NoiseEngine};
use crate::vanilla::noise::NoiseStack;
use crate::vanilla::random::{PositionalRandomFactory, RandomSource};
use crate::vanilla::worldgen::{NoiseRouter, WorldgenData, WorldgenError};

/// `MaterialRuleContext.HOW_FAR_BELOW_PRELIMINARY_SURFACE_LEVEL_TO_BUILD_SURFACE`.
const MIN_SURFACE_OFFSET: i32 = 8;
/// The clay band table length (the badlands `generateBands` program).
const BAND_COUNT: usize = 192;

/// A material condition predicate. Vertical anchors are resolved to
/// absolute block Y at load time using the router's dimension bounds
/// (`absolute: n` -> n, `above_bottom: n` -> min_y + n, `below_top: n`
/// -> min_y + height - 1 - n, per the 26.3 anchor records).
#[derive(Debug, Clone)]
pub enum Condition {
    Biome(Vec<String>),
    NoiseThreshold {
        noise: String,
        min: f64,
        max: f64,
        is_3d: bool,
    },
    VerticalGradient {
        random_name: String,
        true_at_and_below: i32,
        false_at_and_above: i32,
    },
    YAbove {
        anchor_y: i32,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    },
    Water {
        offset: i32,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    },
    StoneDepth {
        offset: i32,
        add_surface_depth: bool,
        secondary_depth_range: i32,
        ceiling: bool,
    },
    Steep,
    Hole,
    Not(Box<Condition>),
    AbovePreliminarySurface,
}

/// A compiled material rule node. `Block` and the vein blocks carry the
/// plain block-state ids used by the overworld data (property-bearing
/// object states keep only the `Name`).
#[derive(Clone)]
pub enum Rule {
    Sequence(Vec<Rule>),
    Condition {
        condition: Condition,
        then_run: Box<Rule>,
    },
    Block(String),
    Bandlands,
    OreVein {
        ore_block: String,
        raw_ore_block: String,
        filler_block: String,
        raw_ore_chance: f32,
        density: Density,
        richness: Density,
        filler_gap: Density,
    },
}

/// A dimension's compiled surface-rule program plus every raw noise the
/// evaluation needs, instantiated once from the world seed.
pub struct SurfaceRules {
    root: Rule,
    noise: HashMap<String, Rc<NoiseStack>>,
    surface_noise: Rc<NoiseStack>,
    surface_secondary_noise: Rc<NoiseStack>,
    clay_bands_offset_noise: Rc<NoiseStack>,
    positional: PositionalRandomFactory,
    clay_bands: Vec<&'static str>,
}

// The rule tree holds opaque compiled densities; the debug view shows the
// wiring around them.
impl std::fmt::Debug for SurfaceRules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfaceRules")
            .field("noise", &self.noise.keys().collect::<Vec<_>>())
            .field("clay_bands", &self.clay_bands.len())
            .finish_non_exhaustive()
    }
}

impl SurfaceRules {
    /// Compiles the rule tree reachable from `root_id` plus the three
    /// system noises the material system always instantiates.
    pub fn compile(
        data: &WorldgenData,
        engine: &NoiseEngine,
        registry: &DensityRegistry<'_>,
        router: &NoiseRouter,
        root_id: &str,
    ) -> Result<Self, WorldgenError> {
        let mut referenced_noise = Vec::new();
        let mut visiting = HashSet::new();
        let root = load_rule(
            &Value::String(root_id.to_owned()),
            data,
            registry,
            router,
            &mut visiting,
            &mut referenced_noise,
        )?;
        let resolve = |id: &str| {
            engine
                .noise_stack(id)
                .map_err(|error| WorldgenError::Invalid(format!("material rules: {error}")))
        };
        let mut noise = HashMap::new();
        for id in referenced_noise {
            if !noise.contains_key(&id) {
                noise.insert(id.clone(), resolve(&id)?);
            }
        }
        Ok(Self {
            root,
            noise,
            surface_noise: resolve("minecraft:surface")?,
            surface_secondary_noise: resolve("minecraft:surface_secondary")?,
            clay_bands_offset_noise: resolve("minecraft:clay_bands_offset")?,
            positional: engine.positional(),
            clay_bands: generate_clay_bands(
                engine.positional().from_hash_of("minecraft:clay_bands"),
            ),
        })
    }

    /// Applies the rule program at the context's current block position.
    pub fn apply(&self, ctx: &mut SurfaceContext<'_>) -> Option<String> {
        eval_rule(&self.root, self, ctx)
    }

    fn noise_value(&self, id: &str) -> Option<&Rc<NoiseStack>> {
        self.noise.get(id)
    }
}

/// Per-column evaluation state: XZ-lazy surface values, the current-Y
/// descent inputs, and the per-sampler caches, mirroring the documented
/// context's update-XZ / update-Y invalidation scheme.
pub struct SurfaceContext<'a> {
    x: i32,
    z: i32,
    surface_depth: i32,
    surface_secondary: Option<f64>,
    min_surface_level: Option<i32>,
    gradients: Option<(i32, i32)>,
    preliminary_surface: Option<&'a Density>,
    height_at: Box<dyn 'a + FnMut(i32, i32) -> i32>,
    biome_at: Box<dyn 'a + FnMut(i32, i32, i32) -> Option<String>>,
    block_y: i32,
    stone_above: i32,
    stone_below: i32,
    water_height: Option<i32>,
    noise_2d: HashMap<String, f64>,
    noise_3d: HashMap<String, f64>,
    factories: HashMap<String, PositionalRandomFactory>,
    biome_cache: Option<(i32, Option<String>)>,
}

impl<'a> SurfaceContext<'a> {
    /// Binds one column. The surface depth is the documented eager
    /// per-XZ value: `(int)(surfaceNoise(x, 0, z) * 2.75 + 3.0 +
    /// positional.at(x, 0, z).nextDouble() * 0.25)`.
    pub fn new(
        rules: &SurfaceRules,
        preliminary_surface: Option<&'a Density>,
        x: i32,
        z: i32,
        height_at: impl FnMut(i32, i32) -> i32 + 'a,
        biome_at: impl FnMut(i32, i32, i32) -> Option<String> + 'a,
    ) -> Self {
        let noise = f64::from(rules.surface_noise.get(f64::from(x), 0.0, f64::from(z)));
        let jitter = rules.positional.at(x, 0, z).next_double();
        let surface_depth = (noise * 2.75 + 3.0 + jitter * 0.25) as i32;
        Self {
            x,
            z,
            surface_depth,
            surface_secondary: None,
            min_surface_level: None,
            gradients: None,
            preliminary_surface,
            height_at: Box::new(height_at),
            biome_at: Box::new(biome_at),
            block_y: i32::MIN,
            stone_above: 0,
            stone_below: 0,
            water_height: None,
            noise_2d: HashMap::new(),
            noise_3d: HashMap::new(),
            factories: HashMap::new(),
            biome_cache: None,
        }
    }

    /// Records the descent inputs for one block row; resets the caches
    /// the documented context treats as Y-lazy.
    pub fn set_y(&mut self, stone_above: i32, stone_below: i32, water_height: Option<i32>, y: i32) {
        self.block_y = y;
        self.stone_above = stone_above;
        self.stone_below = stone_below;
        self.water_height = water_height;
        self.noise_3d.clear();
        self.biome_cache = None;
    }

    fn noise_at(&mut self, rules: &SurfaceRules, id: &str, three_d: bool) -> f64 {
        let cached = if three_d {
            self.noise_3d.get(id)
        } else {
            self.noise_2d.get(id)
        };
        if let Some(&value) = cached {
            return value;
        }
        let y = if three_d { self.block_y } else { 0 };
        // Unknown ids were rejected at compile time; a stack that is
        // absent here can only be an internal wiring bug.
        let value = rules
            .noise_value(id)
            .map(|stack| f64::from(stack.get(f64::from(self.x), f64::from(y), f64::from(self.z))))
            .unwrap_or(f64::NAN);
        if three_d {
            self.noise_3d.insert(id.to_owned(), value);
        } else {
            self.noise_2d.insert(id.to_owned(), value);
        }
        value
    }

    fn surface_secondary(&mut self, rules: &SurfaceRules) -> f64 {
        match self.surface_secondary {
            Some(value) => value,
            None => {
                let value = f64::from(rules.surface_secondary_noise.get(
                    f64::from(self.x),
                    0.0,
                    f64::from(self.z),
                ));
                self.surface_secondary = Some(value);
                value
            }
        }
    }

    /// `Mth.floor(preliminarySurface(x, 0, z)) + surfaceDepth - 8` from
    /// the documented one-row surface volume; dimensions without a
    /// `chunk_surface_level` router entry fall back to a zero constant.
    fn min_surface_level(&mut self) -> i32 {
        if let Some(level) = self.min_surface_level {
            return level;
        }
        let preliminary = self
            .preliminary_surface
            .map(|density| density.sample(self.x, 0, self.z))
            .unwrap_or(0.0);
        let level = preliminary.floor() as i32 + self.surface_depth - MIN_SURFACE_OFFSET;
        self.min_surface_level = Some(level);
        level
    }

    /// WORLD_SURFACE_WG differences with the documented chunk-local
    /// clamp (`min(lx + 1, 15)`, `max(lx - 1, 0)`); at chunk edges the
    /// clamped neighbours coincide and the gradient is zero.
    fn gradients(&mut self) -> (i32, i32) {
        if let Some(gradients) = self.gradients {
            return gradients;
        }
        let base_x = self.x.div_euclid(16) * 16;
        let lx = self.x.rem_euclid(16);
        let base_z = self.z.div_euclid(16) * 16;
        let lz = self.z.rem_euclid(16);
        let high_x = base_x + (lx + 1).min(15);
        let low_x = base_x + (lx - 1).max(0);
        let high_z = base_z + (lz + 1).min(15);
        let low_z = base_z + (lz - 1).max(0);
        let gradient_x = (self.height_at)(high_x, self.z) - (self.height_at)(low_x, self.z);
        let gradient_z = (self.height_at)(self.x, high_z) - (self.height_at)(self.x, low_z);
        self.gradients = Some((gradient_x, gradient_z));
        (gradient_x, gradient_z)
    }

    fn factory(&mut self, rules: &SurfaceRules, name: &str) -> PositionalRandomFactory {
        if let Some(factory) = self.factories.get(name) {
            return *factory;
        }
        let factory = rules.positional.from_hash_of(name).fork_positional();
        self.factories.insert(name.to_owned(), factory);
        factory
    }

    fn biome(&mut self) -> Option<String> {
        if let Some((y, cached)) = &self.biome_cache
            && *y == self.block_y
        {
            return cached.clone();
        }
        let value = (self.biome_at)(self.x, self.block_y, self.z);
        self.biome_cache = Some((self.block_y, value.clone()));
        value
    }
    /// The badlands band at the current row: clay band index
    /// `(y + round(clayBandsOffset(x, 0, z) * 4.0F) + 192) % 192`.
    fn band(&mut self, rules: &SurfaceRules) -> String {
        let offset_noise = f64::from(rules.clay_bands_offset_noise.get(
            f64::from(self.x),
            0.0,
            f64::from(self.z),
        ));
        // Java Math.round(float) is floor of the value plus one half.
        let scaled = (offset_noise as f32) * 4.0f32;
        let rounded = (f64::from(scaled) + 0.5).floor() as i32;
        let index = (self.block_y + rounded).rem_euclid(rules.clay_bands.len() as i32) as usize;
        rules.clay_bands[index].to_owned()
    }
}

/// Java `Mth.map(double, ...)`: the operand order is preserved so the
/// f64 rounding matches bit-for-bit.
fn mth_map(value: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    (value - in_min) * (out_max - out_min) / (in_max - in_min) + out_min
}

fn eval_rule(rule: &Rule, rules: &SurfaceRules, ctx: &mut SurfaceContext<'_>) -> Option<String> {
    match rule {
        Rule::Sequence(children) => children
            .iter()
            .find_map(|child| eval_rule(child, rules, ctx)),
        Rule::Condition {
            condition,
            then_run,
        } => {
            if eval_condition(condition, rules, ctx) {
                eval_rule(then_run, rules, ctx)
            } else {
                None
            }
        }
        Rule::Block(state) => Some(state.clone()),
        Rule::Bandlands => Some(ctx.band(rules)),
        Rule::OreVein {
            ore_block,
            raw_ore_block,
            filler_block,
            raw_ore_chance,
            density,
            richness,
            filler_gap,
        } => {
            let (x, y, z) = (ctx.x, ctx.block_y, ctx.z);
            let density_value = density.sample(x, y, z);
            if density_value <= 0.0 {
                return None;
            }
            let factory = ctx.factory(rules, "minecraft:ore");
            let mut random = factory.at(x, y, z);
            if random.next_float() > density_value {
                return None;
            }
            let richness_value = richness.sample(x, y, z);
            if random.next_float() < richness_value && filler_gap.sample(x, y, z) < 0.0 {
                Some(if random.next_float() < *raw_ore_chance {
                    raw_ore_block.clone()
                } else {
                    ore_block.clone()
                })
            } else {
                Some(filler_block.clone())
            }
        }
    }
}

fn eval_condition(
    condition: &Condition,
    rules: &SurfaceRules,
    ctx: &mut SurfaceContext<'_>,
) -> bool {
    match condition {
        Condition::Biome(ids) => match ctx.biome() {
            Some(biome) => ids.contains(&biome),
            None => false,
        },
        Condition::NoiseThreshold {
            noise,
            min,
            max,
            is_3d,
        } => {
            let value = ctx.noise_at(rules, noise, *is_3d);
            value >= *min && value <= *max
        }
        Condition::VerticalGradient {
            random_name,
            true_at_and_below,
            false_at_and_above,
        } => {
            let y = ctx.block_y;
            if y <= *true_at_and_below {
                return true;
            }
            if y >= *false_at_and_above {
                return false;
            }
            let probability = mth_map(
                f64::from(y),
                f64::from(*true_at_and_below),
                f64::from(*false_at_and_above),
                1.0,
                0.0,
            );
            let factory = ctx.factory(rules, random_name);
            f64::from(factory.at(ctx.x, y, ctx.z).next_float()) < probability
        }
        Condition::YAbove {
            anchor_y,
            surface_depth_multiplier,
            add_stone_depth,
        } => {
            let stone = if *add_stone_depth { ctx.stone_above } else { 0 };
            ctx.block_y + stone >= anchor_y + ctx.surface_depth * surface_depth_multiplier
        }
        Condition::Water {
            offset,
            surface_depth_multiplier,
            add_stone_depth,
        } => match ctx.water_height {
            None => true,
            Some(water_height) => {
                let stone = if *add_stone_depth { ctx.stone_above } else { 0 };
                ctx.block_y + stone
                    >= water_height + offset + ctx.surface_depth * surface_depth_multiplier
            }
        },
        Condition::StoneDepth {
            offset,
            add_surface_depth,
            secondary_depth_range,
            ceiling,
        } => {
            let depth = if *ceiling {
                ctx.stone_below
            } else {
                ctx.stone_above
            };
            let surface = if *add_surface_depth {
                ctx.surface_depth
            } else {
                0
            };
            let secondary = if *secondary_depth_range == 0 {
                0
            } else {
                mth_map(
                    ctx.surface_secondary(rules),
                    -1.0,
                    1.0,
                    0.0,
                    f64::from(*secondary_depth_range),
                ) as i32
            };
            depth <= 1 + offset + surface + secondary
        }
        Condition::Steep => {
            let (gradient_x, gradient_z) = ctx.gradients();
            gradient_x <= -4 || gradient_z >= 4
        }
        Condition::Hole => ctx.surface_depth <= 0,
        Condition::Not(inner) => !eval_condition(inner, rules, ctx),
        Condition::AbovePreliminarySurface => ctx.block_y >= ctx.min_surface_level(),
    }
}

// ---------------------------------------------------------------- loading

fn type_kind(
    object: &serde_json::Map<String, Value>,
    context: &str,
) -> Result<String, WorldgenError> {
    let raw = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| WorldgenError::Invalid(format!("{context}: missing type")))?;
    Ok(raw.strip_prefix("minecraft:").unwrap_or(raw).to_owned())
}

fn required_int(
    object: &serde_json::Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<i32, WorldgenError> {
    object
        .get(key)
        .and_then(Value::as_i64)
        .map(|value| value as i32)
        .ok_or_else(|| WorldgenError::Invalid(format!("{context}: {key} missing or not an int")))
}

fn required_str<'v>(
    object: &'v serde_json::Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'v str, WorldgenError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| WorldgenError::Invalid(format!("{context}: {key} missing or not a string")))
}

fn resolve_anchor(
    value: &Value,
    router: &NoiseRouter,
    context: &str,
) -> Result<i32, WorldgenError> {
    if let Some(y) = value.as_i64() {
        return Ok(y as i32);
    }
    let object = value
        .as_object()
        .ok_or_else(|| WorldgenError::Invalid(format!("{context}: anchor is not an object")))?;
    if let Some(y) = object.get("absolute").and_then(Value::as_i64) {
        Ok(y as i32)
    } else if let Some(offset) = object.get("above_bottom").and_then(Value::as_i64) {
        Ok(router.min_y + offset as i32)
    } else if let Some(offset) = object.get("below_top").and_then(Value::as_i64) {
        Ok(router.min_y + router.height - 1 - offset as i32)
    } else {
        Err(WorldgenError::Invalid(format!(
            "{context}: unsupported vertical anchor {value}"
        )))
    }
}

fn parse_block_state_id(value: &Value, context: &str) -> Result<String, WorldgenError> {
    match value {
        Value::String(name) => Ok(name.clone()),
        Value::Object(object) => {
            let name = object
                .get("Name")
                .or_else(|| object.get("name"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    WorldgenError::Invalid(format!("{context}: block state without Name"))
                })?;
            Ok(name.to_owned())
        }
        _ => Err(WorldgenError::Invalid(format!(
            "{context}: block state is neither a string nor an object"
        ))),
    }
}

fn load_rule(
    value: &Value,
    data: &WorldgenData,
    registry: &DensityRegistry<'_>,
    router: &NoiseRouter,
    visiting: &mut HashSet<String>,
    noise_ids: &mut Vec<String>,
) -> Result<Rule, WorldgenError> {
    match value {
        Value::String(id) => {
            if !visiting.insert(id.clone()) {
                return Err(WorldgenError::Invalid(format!(
                    "material rule reference cycle at {id}"
                )));
            }
            let document = data
                .material_rule_documents()
                .get(id)
                .ok_or_else(|| WorldgenError::Invalid(format!("unknown material rule {id}")))?;
            let rule = load_rule(document, data, registry, router, visiting, noise_ids);
            visiting.remove(id);
            rule
        }
        Value::Object(object) => {
            let context = format!("material rule {:?}", object.get("type"));
            match type_kind(object, &context)?.as_str() {
                "sequence" => {
                    let items = object
                        .get("sequence")
                        .and_then(Value::as_array)
                        .ok_or_else(|| {
                            WorldgenError::Invalid(format!("{context}: sequence missing"))
                        })?;
                    if items.is_empty() {
                        return Err(WorldgenError::Invalid(format!("{context}: empty sequence")));
                    }
                    let mut children = Vec::with_capacity(items.len());
                    for item in items {
                        children.push(load_rule(
                            item, data, registry, router, visiting, noise_ids,
                        )?);
                    }
                    Ok(Rule::Sequence(children))
                }
                "condition" => {
                    let condition = load_condition(
                        object.get("if_true").ok_or_else(|| {
                            WorldgenError::Invalid(format!("{context}: if_true missing"))
                        })?,
                        data,
                        router,
                        visiting,
                        noise_ids,
                    )?;
                    let then_run = load_rule(
                        object.get("then_run").ok_or_else(|| {
                            WorldgenError::Invalid(format!("{context}: then_run missing"))
                        })?,
                        data,
                        registry,
                        router,
                        visiting,
                        noise_ids,
                    )?;
                    Ok(Rule::Condition {
                        condition,
                        then_run: Box::new(then_run),
                    })
                }
                "block" => {
                    let state = object.get("result_state").ok_or_else(|| {
                        WorldgenError::Invalid(format!("{context}: result_state missing"))
                    })?;
                    Ok(Rule::Block(parse_block_state_id(state, &context)?))
                }
                "bandlands" => Ok(Rule::Bandlands),
                "ore_vein" => {
                    let density = |key: &str| -> Result<Density, WorldgenError> {
                        let value = object.get(key).ok_or_else(|| {
                            WorldgenError::Invalid(format!("{context}: {key} missing"))
                        })?;
                        registry
                            .compile_slot(value)
                            .map_err(|error| WorldgenError::Invalid(format!("{context}: {error}")))
                    };
                    Ok(Rule::OreVein {
                        ore_block: parse_block_state_id(
                            object.get("ore_block").ok_or_else(|| {
                                WorldgenError::Invalid(format!("{context}: ore_block missing"))
                            })?,
                            &context,
                        )?,
                        raw_ore_block: parse_block_state_id(
                            object.get("raw_ore_block").ok_or_else(|| {
                                WorldgenError::Invalid(format!("{context}: raw_ore_block missing"))
                            })?,
                            &context,
                        )?,
                        filler_block: parse_block_state_id(
                            object.get("filler_block").ok_or_else(|| {
                                WorldgenError::Invalid(format!("{context}: filler_block missing"))
                            })?,
                            &context,
                        )?,
                        raw_ore_chance: required_f64(object, "raw_ore_chance", &context)? as f32,
                        density: density("density")?,
                        richness: density("richness")?,
                        filler_gap: density("filler_gap")?,
                    })
                }
                other => Err(WorldgenError::Invalid(format!(
                    "{context}: unsupported material rule type {other:?}"
                ))),
            }
        }
        _ => Err(WorldgenError::Invalid(
            "material rule is neither a reference nor an object".to_owned(),
        )),
    }
}

fn required_f64(
    object: &serde_json::Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<f64, WorldgenError> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| WorldgenError::Invalid(format!("{context}: {key} missing or not a number")))
}

fn load_condition(
    value: &Value,
    data: &WorldgenData,
    router: &NoiseRouter,
    visiting: &mut HashSet<String>,
    noise_ids: &mut Vec<String>,
) -> Result<Condition, WorldgenError> {
    match value {
        Value::String(id) => {
            if !visiting.insert(id.clone()) {
                return Err(WorldgenError::Invalid(format!(
                    "material condition reference cycle at {id}"
                )));
            }
            let document = data.material_condition_documents().get(id).ok_or_else(|| {
                WorldgenError::Invalid(format!("unknown material condition {id}"))
            })?;
            let condition = load_condition(document, data, router, visiting, noise_ids);
            visiting.remove(id);
            condition
        }
        Value::Object(object) => {
            let context = format!("material condition {:?}", object.get("type"));
            match type_kind(object, &context)?.as_str() {
                "biome" => {
                    let biome_is = object.get("biome_is").ok_or_else(|| {
                        WorldgenError::Invalid(format!("{context}: biome_is missing"))
                    })?;
                    let ids = match biome_is {
                        Value::String(single) => vec![single.clone()],
                        Value::Array(items) => items
                            .iter()
                            .map(|item| {
                                item.as_str().map(str::to_owned).ok_or_else(|| {
                                    WorldgenError::Invalid(format!(
                                        "{context}: biome list entries must be ids"
                                    ))
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                        _ => {
                            return Err(WorldgenError::Invalid(format!(
                                "{context}: unsupported biome holder set {biome_is}"
                            )));
                        }
                    };
                    Ok(Condition::Biome(ids))
                }
                "noise_threshold" => {
                    let noise = required_str(object, "noise", &context)?.to_owned();
                    if !noise_ids.contains(&noise) {
                        noise_ids.push(noise.clone());
                    }
                    Ok(Condition::NoiseThreshold {
                        noise,
                        min: required_f64(object, "min_threshold", &context)?,
                        max: required_f64(object, "max_threshold", &context)?,
                        is_3d: object
                            .get("is_3d")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                }
                "vertical_gradient" => Ok(Condition::VerticalGradient {
                    random_name: required_str(object, "random_name", &context)?.to_owned(),
                    true_at_and_below: resolve_anchor(
                        object.get("true_at_and_below").ok_or_else(|| {
                            WorldgenError::Invalid(format!("{context}: true_at_and_below missing"))
                        })?,
                        router,
                        &context,
                    )?,
                    false_at_and_above: resolve_anchor(
                        object.get("false_at_and_above").ok_or_else(|| {
                            WorldgenError::Invalid(format!("{context}: false_at_and_above missing"))
                        })?,
                        router,
                        &context,
                    )?,
                }),
                "y_above" => Ok(Condition::YAbove {
                    anchor_y: resolve_anchor(
                        object.get("anchor").ok_or_else(|| {
                            WorldgenError::Invalid(format!("{context}: anchor missing"))
                        })?,
                        router,
                        &context,
                    )?,
                    surface_depth_multiplier: required_int(
                        object,
                        "surface_depth_multiplier",
                        &context,
                    )?,
                    add_stone_depth: object
                        .get("add_stone_depth")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| {
                            WorldgenError::Invalid(format!("{context}: add_stone_depth missing"))
                        })?,
                }),
                "water" => Ok(Condition::Water {
                    offset: required_int(object, "offset", &context)?,
                    surface_depth_multiplier: required_int(
                        object,
                        "surface_depth_multiplier",
                        &context,
                    )?,
                    add_stone_depth: object
                        .get("add_stone_depth")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                }),
                "stone_depth" => {
                    let surface_type = required_str(object, "surface_type", &context)?;
                    Ok(Condition::StoneDepth {
                        offset: required_int(object, "offset", &context)?,
                        add_surface_depth: object
                            .get("add_surface_depth")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        secondary_depth_range: required_int(
                            object,
                            "secondary_depth_range",
                            &context,
                        )?,
                        ceiling: match surface_type {
                            "floor" => false,
                            "ceiling" => true,
                            _ => {
                                return Err(WorldgenError::Invalid(format!(
                                    "{context}: unsupported surface_type {surface_type:?}"
                                )));
                            }
                        },
                    })
                }
                "steep" => Ok(Condition::Steep),
                "hole" => Ok(Condition::Hole),
                "above_preliminary_surface" => Ok(Condition::AbovePreliminarySurface),
                "not" => {
                    let inner = load_condition(
                        object.get("invert").ok_or_else(|| {
                            WorldgenError::Invalid(format!("{context}: invert missing"))
                        })?,
                        data,
                        router,
                        visiting,
                        noise_ids,
                    )?;
                    Ok(Condition::Not(Box::new(inner)))
                }
                other => Err(WorldgenError::Invalid(format!(
                    "{context}: unsupported material condition type {other:?}"
                ))),
            }
        }
        _ => Err(WorldgenError::Invalid(
            "material condition is neither a reference nor an object".to_owned(),
        )),
    }
}

// ------------------------------------------------------------ clay bands

/// The documented badlands band program: a 192-entry terracotta table
/// painted by one fixed draw order on the `minecraft:clay_bands` stream.
fn generate_clay_bands(mut random: RandomSource) -> Vec<&'static str> {
    const TERRACOTTA: &str = "minecraft:terracotta";
    const ORANGE: &str = "minecraft:orange_terracotta";
    const YELLOW: &str = "minecraft:yellow_terracotta";
    const BROWN: &str = "minecraft:brown_terracotta";
    const RED: &str = "minecraft:red_terracotta";
    const WHITE: &str = "minecraft:white_terracotta";
    const LIGHT_GRAY: &str = "minecraft:light_gray_terracotta";

    let mut bands: Vec<&'static str> = vec![TERRACOTTA; BAND_COUNT];
    let mut i: i32 = 0;
    while i < bands.len() as i32 {
        i += random.next_int_bounded(5) as i32 + 1;
        if (i as usize) < bands.len() {
            bands[i as usize] = ORANGE;
        }
        i += 1;
    }
    make_bands(&mut random, &mut bands, 1, YELLOW);
    make_bands(&mut random, &mut bands, 2, BROWN);
    make_bands(&mut random, &mut bands, 1, RED);
    let white_band_count = random.next_int_bounded(15 - 9 + 1) + 9;
    let mut placed = 0u32;
    let mut start: i32 = 0;
    while placed < white_band_count && start < bands.len() as i32 {
        bands[start as usize] = WHITE;
        if start - 1 > 0 && random.next_bool() {
            bands[(start - 1) as usize] = LIGHT_GRAY;
        }
        if start + 1 < bands.len() as i32 && random.next_bool() {
            bands[(start + 1) as usize] = LIGHT_GRAY;
        }
        placed += 1;
        start += random.next_int_bounded(16) as i32 + 4;
    }
    bands
}

fn make_bands(
    random: &mut RandomSource,
    bands: &mut [&'static str],
    base_width: u32,
    state: &'static str,
) {
    // The documented helper draws: band count 6..=15 inclusive, then per
    // band a width (base + nextInt(3)) and a start position.
    let band_count = random.next_int_bounded(10) + 6;
    for _ in 0..band_count {
        let width = base_width + random.next_int_bounded(3);
        let start = random.next_int_bounded(bands.len() as u32) as usize;
        let mut painted = 0u32;
        while start + (painted as usize) < bands.len() && painted < width {
            bands[start + painted as usize] = state;
            painted += 1;
        }
    }
}

#[cfg(test)]
mod tests;
