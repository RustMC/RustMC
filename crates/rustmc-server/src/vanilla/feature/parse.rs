//! JSON parsers for block states, providers, predicates and configured
//! features.

use super::*;

/// One block state the pack names directly: a bare id, or the
/// `{"id": ..., "properties": {...}}` object the 26.3 documents write. The
/// properties are rendered in the `id[k=v,...]` form the save palettes use,
/// with the keys in sorted order so the name does not depend on the
/// document's own ordering.
pub(super) fn parse_direct_state(value: &Value) -> Option<String> {
    if let Some(name) = value.as_str() {
        return Some(name.to_string());
    }
    let object = value.as_object()?;
    state_from_object(object)
}

/// The `{"id": ..., "properties": {...}}` form of a directly named state.
pub(super) fn state_from_object(object: &serde_json::Map<String, Value>) -> Option<String> {
    let name = object.get("id").and_then(Value::as_str)?;
    let properties = object.get("properties").and_then(Value::as_object);
    let Some(properties) = properties.filter(|map| !map.is_empty()) else {
        return Some(name.to_string());
    };
    let mut keys: Vec<&str> = properties.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let inner: Vec<String> = keys
        .iter()
        .filter_map(|key| Some(format!("{key}={}", properties[*key].as_str()?)))
        .collect();
    if inner.len() != keys.len() {
        // A non-string property value has no agreed rendering here.
        return None;
    }
    Some(format!("{name}[{}]", inner.join(",")))
}

/// Parses one `to_place` block-state provider. A state the pack names
/// directly needs no runtime; a reference into the pack's
/// `block_state_provider` registry, or a provider shape with no sampling code
/// here, is named as a gap rather than guessed at.
pub(super) fn parse_state_provider(value: Option<&Value>) -> StateProvider {
    let unsupported = |kind: &str| StateProvider::Unsupported {
        kind: kind.to_string(),
    };
    match value {
        None => unsupported("minecraft:missing_to_place"),
        Some(Value::String(name)) if name.starts_with('#') => {
            // A block-tag reference picks its state from the tag's members at
            // placement; no runtime here reads that back.
            unsupported("minecraft:block_tag")
        }
        Some(Value::String(id)) => StateProvider::Reference { id: id.clone() },
        Some(Value::Object(object)) => {
            match object.get("type").and_then(Value::as_str) {
                Some("minecraft:weighted") => {
                    let entries: Vec<WeightedState> = object
                        .get("entries")
                        .and_then(Value::as_array)
                        .map(Vec::as_slice)
                        .unwrap_or(&[])
                        .iter()
                        .filter_map(|entry| {
                            let weight = entry.get("weight").and_then(Value::as_i64)?;
                            Some(WeightedState {
                                state: parse_direct_state(entry.get("data")?)?,
                                weight: i32::try_from(weight).ok()?,
                            })
                        })
                        .collect();
                    let count = object
                        .get("entries")
                        .and_then(Value::as_array)
                        .map(Vec::len)
                        .unwrap_or_default();
                    // A partially parsed list would change which entry the
                    // draw picks, so an entry this runtime cannot name gates
                    // the whole provider instead.
                    if entries.len() != count
                        || entries.is_empty()
                        || entries.len() > 64
                        || !entries
                            .iter()
                            .all(|entry| (1..=1_000_000).contains(&entry.weight))
                    {
                        unsupported("minecraft:weighted")
                    } else {
                        StateProvider::Weighted { entries }
                    }
                }
                Some("minecraft:rule_based") => {
                    // A rule that does not parse would let a later rule or the
                    // fallback answer in its place, so it gates the provider.
                    let rules: Option<Vec<RuleBasedEntry>> = object
                        .get("rules")
                        .and_then(Value::as_array)
                        .map(|rules| {
                            rules
                                .iter()
                                .map(|rule| {
                                    let rule = rule.as_object()?;
                                    Some(RuleBasedEntry {
                                        if_true: parse_block_predicate(rule.get("if_true")),
                                        then: parse_state_provider(Some(rule.get("then")?)),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    match (rules, object.get("fallback")) {
                        (Some(rules), Some(fallback)) => StateProvider::RuleBased {
                            fallback: Box::new(parse_state_provider(Some(fallback))),
                            rules,
                        },
                        _ => unsupported("minecraft:rule_based"),
                    }
                }
                Some("minecraft:randomized_int") => {
                    let Some(property) = object
                        .get("property")
                        .and_then(Value::as_str)
                        .filter(|p| !p.is_empty())
                    else {
                        return unsupported("minecraft:randomized_int");
                    };
                    let source = match object.get("source") {
                        Some(src) => Box::new(parse_state_provider(Some(src))),
                        None => Box::new(unsupported("minecraft:missing_state")),
                    };
                    let values = object
                        .get("values")
                        .and_then(|v| RangeProvider::from_value(v, false))
                        .unwrap_or_else(|| RangeProvider::Unsupported {
                            kind: "minecraft:missing_values".to_string(),
                        });
                    StateProvider::RandomizedInt {
                        source,
                        property: property.to_string(),
                        values,
                    }
                }
                Some("minecraft:rotated" | "minecraft:rotated_block_provider") => {
                    let Some(state_val) = object.get("state") else {
                        return unsupported("minecraft:rotated");
                    };
                    let state = if state_val.is_string() {
                        parse_direct_state(state_val).map_or_else(
                            || parse_state_provider(Some(state_val)),
                            |s| StateProvider::Constant { state: s },
                        )
                    } else {
                        parse_state_provider(Some(state_val))
                    };
                    let direction = match object.get("direction").and_then(Value::as_str) {
                        Some(name) => match parse_direction_name(name) {
                            Some(dir) => Some(dir),
                            None => return unsupported("minecraft:rotated"),
                        },
                        None => None,
                    };
                    StateProvider::Rotated {
                        state: Box::new(state),
                        direction,
                    }
                }
                Some(other) => unsupported(other),
                // No `type`: the direct block-state form.
                None => state_from_object(object).map_or_else(
                    || unsupported("minecraft:missing_state"),
                    |state| StateProvider::Constant { state },
                ),
            }
        }
        Some(_) => unsupported("minecraft:missing_to_place"),
    }
}

/// A `minecraft:block_predicate_filter`'s position offset: three small
/// integers, `[0, 0, 0]` when the document writes none. A wider shift reads
/// beyond the region the decoration pass resolves, so it is refused.
pub(super) fn parse_block_offset(
    object: &serde_json::Map<String, Value>,
) -> Option<(i32, i32, i32)> {
    let Some(value) = object.get("offset") else {
        return Some((0, 0, 0));
    };
    let parts = value.as_array().filter(|parts| parts.len() == 3)?;
    let mut axes = [0i32; 3];
    for (index, part) in parts.iter().enumerate() {
        let number = i32::try_from(part.as_i64()?).ok()?;
        axes[index] = number;
    }
    axes.iter()
        .all(|axis| (-16..=16).contains(axis))
        .then_some((axes[0], axes[1], axes[2]))
}

/// Parses one placement block predicate. An unknown predicate type, a missing
/// one, or an offset this runtime will not read becomes `Unsupported`, which
/// gates the chain out of the run instead of letting an untested position
/// through.
pub(super) fn parse_block_predicate(value: Option<&Value>) -> BlockPredicate {
    let unsupported = |kind: &str| BlockPredicate::Unsupported {
        kind: kind.to_string(),
    };
    let Some(Value::Object(object)) = value else {
        return unsupported("minecraft:missing_predicate");
    };
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("minecraft:missing_predicate");
    let nested = |key: &str| -> Vec<BlockPredicate> {
        object
            .get(key)
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|entry| parse_block_predicate(Some(entry)))
                    .collect()
            })
            .unwrap_or_default()
    };
    match kind {
        "minecraft:matching_block_tag" => {
            let Some(tag) = object.get("tag").and_then(Value::as_str) else {
                return unsupported(kind);
            };
            match parse_block_offset(object) {
                Some(offset) => BlockPredicate::BlockTag {
                    tag: tag.to_string(),
                    offset,
                },
                None => unsupported(kind),
            }
        }
        "minecraft:matching_blocks" => {
            let blocks: Vec<String> = match object.get("blocks") {
                Some(Value::Array(list)) => list
                    .iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect(),
                Some(Value::String(single)) => vec![single.to_string()],
                _ => Vec::new(),
            };
            let count = object
                .get("blocks")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(1);
            if blocks.is_empty()
                || blocks.len() != count
                || blocks.iter().any(|block| block.starts_with('#'))
            {
                return unsupported(kind);
            }
            match parse_block_offset(object) {
                Some(offset) => BlockPredicate::Blocks { blocks, offset },
                None => unsupported(kind),
            }
        }
        "minecraft:matching_fluids" => {
            let fluids: Vec<String> = match object.get("fluids") {
                Some(Value::Array(list)) => list
                    .iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect(),
                Some(Value::String(single)) => vec![single.to_string()],
                _ => Vec::new(),
            };
            let count = object
                .get("fluids")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(1);
            if fluids.is_empty() || fluids.len() != count {
                return unsupported(kind);
            }
            match parse_block_offset(object) {
                Some(offset) => BlockPredicate::MatchingFluids { fluids, offset },
                None => unsupported(kind),
            }
        }
        "minecraft:inside_world_bounds" => match parse_block_offset(object) {
            Some(offset) => BlockPredicate::InsideWorldBounds { offset },
            None => unsupported(kind),
        },
        "minecraft:replaceable" => match parse_block_offset(object) {
            Some(offset) => BlockPredicate::Replaceable { offset },
            None => unsupported(kind),
        },
        "minecraft:would_survive" => {
            let Some(offset) = parse_block_offset(object) else {
                return unsupported(kind);
            };
            let Some(state) = object.get("state").and_then(parse_direct_state) else {
                return unsupported(kind);
            };
            BlockPredicate::WouldSurvive { offset, state }
        }
        "minecraft:true" => BlockPredicate::True,
        // An empty conjunction would accept every position and an empty
        // disjunction is a no-op, so neither is read as a filter at all.
        "minecraft:any_of" | "minecraft:all_of" => {
            let rules = nested("predicates");
            if rules.is_empty() {
                return unsupported(kind);
            }
            if kind == "minecraft:any_of" {
                BlockPredicate::AnyOf(rules)
            } else {
                BlockPredicate::AllOf(rules)
            }
        }
        "minecraft:not" => match object.get("predicate") {
            Some(inner) => BlockPredicate::Not(Box::new(parse_block_predicate(Some(inner)))),
            None => unsupported(kind),
        },
        other => unsupported(other),
    }
}

/// Parses a list of placement modifiers with attempt budget validation.
pub(super) fn parse_modifiers_list(
    value: Option<&Value>,
    id: &str,
) -> Result<Vec<PlacementModifier>, WorldgenError> {
    let list = value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let modifiers = list
        .iter()
        .enumerate()
        .map(|(index, entry)| parse_placement_modifier(id, index, entry))
        .collect::<Result<Vec<_>, _>>()?;
    let attempt_budget = modifiers.iter().try_fold(1u32, |product, modifier| {
        if let PlacementModifier::Count { count } = modifier {
            match count.range(0, 0) {
                Some((_, max)) => product.checked_mul(max.max(0) as u32),
                None => Some(product),
            }
        } else {
            Some(product)
        }
    });
    if !matches!(attempt_budget, Some(0..=4096)) {
        return Err(WorldgenError::Invalid(format!(
            "{id}: count modifiers exceed the 4096-attempt placement budget"
        )));
    }
    Ok(modifiers)
}

/// Parses an inline placed feature inside a random patch.
pub(super) fn parse_inline_placed_feature(value: &Value) -> Result<PlacedFeature, WorldgenError> {
    if let Some(name) = value.as_str() {
        return Ok(PlacedFeature {
            id: name.to_string(),
            feature: Rc::new(ConfiguredFeature::Unsupported {
                kind: name.to_string(),
            }),
            modifiers: Vec::new(),
        });
    }
    let Some(object) = value.as_object() else {
        return Err(WorldgenError::Invalid(
            "random_patch feature must be an object or string".to_string(),
        ));
    };
    if let Some(inner) = object.get("feature") {
        let feature = if let Some(name) = inner.as_str() {
            Rc::new(ConfiguredFeature::Unsupported {
                kind: name.to_string(),
            })
        } else {
            Rc::new(parse_configured_feature(inner)?)
        };
        let modifiers = parse_modifiers_list(object.get("placement"), "inline_patch")?;
        let placed = PlacedFeature {
            id: "inline_patch".to_string(),
            feature,
            modifiers,
        };
        placed.validate_attempt_budget(0)?;
        Ok(placed)
    } else {
        let feature = Rc::new(parse_configured_feature(value)?);
        let modifiers = parse_modifiers_list(object.get("placement"), "inline_patch")?;
        let placed = PlacedFeature {
            id: "inline_patch".to_string(),
            feature,
            modifiers,
        };
        placed.validate_attempt_budget(0)?;
        Ok(placed)
    }
}

/// Parses one configured feature document. Types without a runtime here
/// keep only their name.
pub(crate) fn parse_configured_feature(
    document: &Value,
) -> Result<ConfiguredFeature, WorldgenError> {
    let kind = document
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("minecraft:missing_type")
        .to_string();

    let body = match document.get("config") {
        Some(config) if config.is_object() => config,
        _ => document,
    };

    match kind.as_str() {
        "minecraft:ore" => {
            let size = body
                .get("size")
                .and_then(Value::as_i64)
                .ok_or_else(|| WorldgenError::Invalid("ore feature without a size".to_string()))?;
            let Ok(size) = i32::try_from(size) else {
                return Err(WorldgenError::Invalid(format!(
                    "ore feature size must be in 1..=128, got {size}"
                )));
            };
            if !(1..=128).contains(&size) {
                return Err(WorldgenError::Invalid(format!(
                    "ore feature size must be in 1..=128, got {size}"
                )));
            }
            let discard_chance_on_air_exposure = body
                .get("discard_chance_on_air_exposure")
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as f32;
            let mut targets = Vec::new();
            for entry in body
                .get("targets")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[])
            {
                let state = entry.get("state").and_then(Value::as_str).ok_or_else(|| {
                    WorldgenError::Invalid("ore target without a state".to_string())
                })?;
                targets.push(OreTarget {
                    predicate: parse_rule_test(entry.get("target"))?,
                    state: state.to_string(),
                });
            }
            if targets.is_empty() {
                return Err(WorldgenError::Invalid(
                    "ore feature without targets".to_string(),
                ));
            }
            Ok(ConfiguredFeature::Ore {
                size,
                targets,
                discard_chance_on_air_exposure,
            })
        }
        "minecraft:random_patch" | "minecraft:flower" => parse_random_patch(&kind, body),
        "minecraft:simple_block" => Ok(ConfiguredFeature::SimpleBlock {
            to_place: parse_state_provider(body.get("to_place")),
        }),
        "minecraft:disk" => parse_disk(body),
        "minecraft:spring_feature" => parse_spring(body),
        "minecraft:simple_random_selector" => parse_simple_random_selector(body),
        "minecraft:random_selector" => parse_random_selector(body),
        "minecraft:weighted_random_selector" => parse_weighted_random_selector(body),
        "minecraft:random_boolean_selector" => parse_random_boolean_selector(body),
        "minecraft:block_column" => parse_block_column(body),
        _ => Ok(ConfiguredFeature::Unsupported { kind }),
    }
}

/// Parses a `minecraft:disk` configuration.
pub(super) fn parse_disk(body: &Value) -> Result<ConfiguredFeature, WorldgenError> {
    let half_height = body
        .get("half_height")
        .and_then(Value::as_i64)
        .ok_or_else(|| WorldgenError::Invalid("disk feature without half_height".to_string()))?;
    let half_height = i32::try_from(half_height).map_err(|_| {
        WorldgenError::Invalid(format!("disk half_height out of bounds: {half_height}"))
    })?;
    if !(0..=16).contains(&half_height) {
        return Err(WorldgenError::Invalid(format!(
            "disk half_height must be in 0..=16, got {half_height}"
        )));
    }
    let radius_val = body
        .get("radius")
        .ok_or_else(|| WorldgenError::Invalid("disk feature without radius".to_string()))?;
    let radius = RangeProvider::from_value(radius_val, false)
        .ok_or_else(|| WorldgenError::Invalid("invalid disk radius provider".to_string()))?;
    // An unsupported provider has no extrema and reports its own gap. A
    // radius this runtime will not run keeps the pack loading as a gap,
    // because a valid disk wider than the replay window is not an error.
    if radius
        .range(0, 0)
        .is_some_and(|(min, max)| min < 0 || max > MAX_DISK_RADIUS)
    {
        return Ok(ConfiguredFeature::Unsupported {
            kind: "minecraft:disk:radius".to_string(),
        });
    }
    Ok(ConfiguredFeature::Disk {
        state_provider: parse_state_provider(body.get("state_provider")),
        target_predicate: parse_block_predicate(body.get("target")),
        radius,
        half_height,
    })
}

/// Parses a `minecraft:spring_feature` configuration.
pub(super) fn parse_spring(body: &Value) -> Result<ConfiguredFeature, WorldgenError> {
    let state_val = body
        .get("state")
        .ok_or_else(|| WorldgenError::Invalid("spring feature without state".to_string()))?;
    let Some(state) = spring_block_state(state_val) else {
        return Ok(ConfiguredFeature::Unsupported {
            kind: "minecraft:spring_feature:state".to_string(),
        });
    };
    // The pack omits these on the features that keep the codec's default
    // and writes `rock_count: 5, hole_count: 0` for the closed nether
    // spring, so the two always sum to the five cells counted. The default
    // itself is observed: in a stored seed-2026 save, among still water
    // under a valid ceiling whose other sides are valid or flowing water,
    // 716 sources have 4 valid sides and 1 opening and none have 3 and 2.
    let count = |name: &str, default: i64| -> Option<i32> {
        let value = match body.get(name) {
            Some(value) => value.as_i64()?,
            None => default,
        };
        (0..=5).contains(&value).then_some(value as i32)
    };
    let Some(rock_count) = count("rock_count", 4) else {
        return Ok(ConfiguredFeature::Unsupported {
            kind: "minecraft:spring_feature:rock_count".to_string(),
        });
    };
    let Some(hole_count) = count("hole_count", 1) else {
        return Ok(ConfiguredFeature::Unsupported {
            kind: "minecraft:spring_feature:hole_count".to_string(),
        });
    };
    let requires_block_below = body
        .get("requires_block_below")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let valid_blocks = match body.get("valid_blocks") {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => {
            return Err(WorldgenError::Invalid(
                "spring feature without valid_blocks".to_string(),
            ));
        }
    };
    Ok(ConfiguredFeature::Spring {
        state,
        rock_count,
        hole_count,
        requires_block_below,
        valid_blocks,
    })
}

/// Parses the configuration `minecraft:random_patch` and `minecraft:flower`
/// share. No document in the 26.3 pack states a default for these fields, so a
/// field left out is a gap rather than a number chosen here, and so is a value
/// outside this runtime's work budget: such a document keeps its ordinal and
/// its place in the gap report, and the rest of the pack still loads.
pub(super) fn parse_random_patch(
    kind: &str,
    body: &Value,
) -> Result<ConfiguredFeature, WorldgenError> {
    let unmodeled = |field: &str| ConfiguredFeature::Unsupported {
        kind: format!("{kind}:{field}"),
    };
    let field = |name: &str, range: std::ops::RangeInclusive<i64>| -> Option<i32> {
        let value = body.get(name)?.as_i64()?;
        if !range.contains(&value) {
            return None;
        }
        i32::try_from(value).ok()
    };
    let Some(tries) = field("tries", 1..=MAX_PATCH_TRIES) else {
        return Ok(unmodeled("tries"));
    };
    let Some(xz_spread) = field("xz_spread", 0..=i64::from(MAX_PATCH_REACH)) else {
        return Ok(unmodeled("xz_spread"));
    };
    let Some(y_spread) = field("y_spread", 0..=MAX_PATCH_Y_SPREAD) else {
        return Ok(unmodeled("y_spread"));
    };
    let Some(child) = body.get("feature") else {
        return Ok(unmodeled("feature"));
    };
    Ok(ConfiguredFeature::RandomPatch {
        tries,
        xz_spread,
        y_spread,
        feature: Box::new(parse_inline_placed_feature(child)?),
    })
}

/// Parses a `minecraft:simple_random_selector` configuration.
pub(super) fn parse_simple_random_selector(
    body: &Value,
) -> Result<ConfiguredFeature, WorldgenError> {
    let entries = body
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            WorldgenError::Invalid("simple_random_selector feature without features".to_string())
        })?;
    let mut features = Vec::with_capacity(entries.len());
    for entry in entries {
        features.push(Rc::new(parse_inline_placed_feature(entry)?));
    }
    Ok(ConfiguredFeature::SimpleRandomSelector { features })
}

/// Parses a `minecraft:random_selector` configuration (`Random Selector` rev 3818741).
pub(super) fn parse_random_selector(body: &Value) -> Result<ConfiguredFeature, WorldgenError> {
    let default_val = body.get("default").ok_or_else(|| {
        WorldgenError::Invalid("random_selector feature without default".to_string())
    })?;
    let default = Rc::new(parse_inline_placed_feature(default_val)?);
    let entries = body
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            WorldgenError::Invalid("random_selector feature without features".to_string())
        })?;
    let mut features = Vec::with_capacity(entries.len());
    for entry in entries {
        let chance = entry.get("chance").and_then(Value::as_f64).ok_or_else(|| {
            WorldgenError::Invalid("random_selector entry without chance".to_string())
        })? as f32;
        if !(0.0..=1.0).contains(&chance) {
            return Err(WorldgenError::Invalid(format!(
                "random_selector chance must be in 0.0..=1.0, got {chance}"
            )));
        }
        let child_val = entry.get("feature").ok_or_else(|| {
            WorldgenError::Invalid("random_selector entry without feature".to_string())
        })?;
        features.push((Rc::new(parse_inline_placed_feature(child_val)?), chance));
    }
    Ok(ConfiguredFeature::RandomSelector { features, default })
}

/// Parses a `minecraft:weighted_random_selector` configuration (`Java Edition 26.2` rev 3880953).
pub(super) fn parse_weighted_random_selector(
    body: &Value,
) -> Result<ConfiguredFeature, WorldgenError> {
    let entries = body
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            WorldgenError::Invalid("weighted_random_selector feature without features".to_string())
        })?;
    let mut features = Vec::with_capacity(entries.len());
    for entry in entries {
        let weight = entry
            .get("weight")
            .and_then(Value::as_i64)
            .and_then(|w| i32::try_from(w).ok())
            .ok_or_else(|| {
                WorldgenError::Invalid("weighted_random_selector entry without weight".to_string())
            })?;
        if weight <= 0 {
            return Err(WorldgenError::Invalid(format!(
                "weighted_random_selector weight must be positive, got {weight}"
            )));
        }
        let data = entry.get("data").ok_or_else(|| {
            WorldgenError::Invalid("weighted_random_selector entry without data".to_string())
        })?;
        features.push((Rc::new(parse_inline_placed_feature(data)?), weight));
    }
    Ok(ConfiguredFeature::WeightedRandomSelector { features })
}

/// Parses a `minecraft:random_boolean_selector` configuration (`Random Boolean Selector` rev 3818739).
pub(super) fn parse_random_boolean_selector(
    body: &Value,
) -> Result<ConfiguredFeature, WorldgenError> {
    let feature_true = Rc::new(parse_inline_placed_feature(
        body.get("feature_true").ok_or_else(|| {
            WorldgenError::Invalid("random_boolean_selector without feature_true".to_string())
        })?,
    )?);
    let feature_false = Rc::new(parse_inline_placed_feature(
        body.get("feature_false").ok_or_else(|| {
            WorldgenError::Invalid("random_boolean_selector without feature_false".to_string())
        })?,
    )?);
    Ok(ConfiguredFeature::RandomBooleanSelector {
        feature_true,
        feature_false,
    })
}

/// Parses a `minecraft:block_column` configuration (`Cave Vines (feature)` rev 3818484).
pub(super) fn parse_block_column(body: &Value) -> Result<ConfiguredFeature, WorldgenError> {
    let dir_str = body
        .get("direction")
        .and_then(Value::as_str)
        .ok_or_else(|| WorldgenError::Invalid("block_column without direction".to_string()))?;
    let direction = parse_direction_name(dir_str).ok_or_else(|| {
        WorldgenError::Invalid(format!("block_column invalid direction {dir_str}"))
    })?;
    let allowed_placement = parse_block_predicate(body.get("allowed_placement"));
    let prioritize_tip = body
        .get("prioritize_tip")
        .and_then(Value::as_bool)
        .ok_or_else(|| WorldgenError::Invalid("block_column without prioritize_tip".to_string()))?;
    let layer_entries = body
        .get("layers")
        .and_then(Value::as_array)
        .ok_or_else(|| WorldgenError::Invalid("block_column without layers".to_string()))?;
    let mut layers = Vec::with_capacity(layer_entries.len());
    for entry in layer_entries {
        let height_val = entry.get("height").ok_or_else(|| {
            WorldgenError::Invalid("block_column layer without height".to_string())
        })?;
        let height = RangeProvider::from_value(height_val, false).ok_or_else(|| {
            WorldgenError::Invalid("block_column layer height is not an int provider".to_string())
        })?;
        let provider = parse_state_provider(entry.get("provider"));
        layers.push(BlockColumnLayer { height, provider });
    }
    Ok(ConfiguredFeature::BlockColumn {
        direction,
        allowed_placement,
        prioritize_tip,
        layers,
    })
}
