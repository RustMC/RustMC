//! JSON parsers for rule tests, placed features, placement modifiers and
//! biome feature steps.

use super::*;

pub(super) fn parse_rule_test(value: Option<&Value>) -> Result<RuleTest, WorldgenError> {
    let Some(value) = value else {
        return Err(WorldgenError::Invalid(
            "ore target without a predicate".to_string(),
        ));
    };
    let kind = value
        .get("predicate_type")
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("minecraft:missing_predicate");
    let nested = |key: &str| -> Result<Vec<RuleTest>, WorldgenError> {
        value
            .get(key)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .map(|entry| parse_rule_test(Some(entry)))
            .collect()
    };
    match kind {
        "minecraft:tag_match" => value
            .get("tag")
            .and_then(Value::as_str)
            .map(|tag| RuleTest::Tag {
                tag: tag.to_string(),
            })
            .ok_or_else(|| WorldgenError::Invalid("tag_match target without a tag".to_string())),
        "minecraft:block_match" => value
            .get("block")
            .and_then(Value::as_str)
            .map(|block| RuleTest::Block {
                block: block.to_string(),
            })
            .ok_or_else(|| {
                WorldgenError::Invalid("block_match target without a block".to_string())
            }),
        "minecraft:matching_blocks" => {
            let blocks = value.get("blocks");
            let list: Vec<String> = match blocks.and_then(Value::as_array) {
                Some(array) => array
                    .iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect(),
                None => blocks
                    .and_then(Value::as_str)
                    .map(|single| vec![single.to_string()])
                    .unwrap_or_default(),
            };
            if list.is_empty() {
                return Err(WorldgenError::Invalid(
                    "matching_blocks target without blocks".to_string(),
                ));
            }
            Ok(RuleTest::Blocks { blocks: list })
        }
        "minecraft:height_match" => Ok(RuleTest::Height {
            min_inclusive: value
                .get("min_inclusive")
                .and_then(Value::as_i64)
                .ok_or_else(|| {
                    WorldgenError::Invalid("height_match target without a minimum".to_string())
                })?
                .try_into()
                .map_err(|_| {
                    WorldgenError::Invalid("height_match minimum is out of range".to_string())
                })?,
            max_inclusive: value
                .get("max_inclusive")
                .and_then(Value::as_i64)
                .ok_or_else(|| {
                    WorldgenError::Invalid("height_match target without a maximum".to_string())
                })?
                .try_into()
                .map_err(|_| {
                    WorldgenError::Invalid("height_match maximum is out of range".to_string())
                })?,
        }),
        "minecraft:any_of" => Ok(RuleTest::AnyOf(nested("rules")?)),
        "minecraft:all_of" => Ok(RuleTest::AllOf(nested("rules")?)),
        "minecraft:not" => Ok(RuleTest::Not(Box::new(parse_rule_test(value.get("rule"))?))),
        other => Ok(RuleTest::Unsupported {
            kind: other.to_string(),
        }),
    }
}

/// Parses one placed feature document, resolving its configured feature by
/// identifier. An unresolvable reference is an error.
pub(crate) fn parse_placed_feature(
    id: &str,
    document: &Value,
    configured: &HashMap<String, Rc<ConfiguredFeature>>,
) -> Result<PlacedFeature, WorldgenError> {
    let feature_id = document
        .get("feature")
        .and_then(Value::as_str)
        .ok_or_else(|| WorldgenError::Invalid(format!("{id}: placed feature without a feature")))?;
    let feature = configured.get(feature_id).cloned().ok_or_else(|| {
        WorldgenError::Invalid(format!(
            "{id}: references unknown configured feature {feature_id}"
        ))
    })?;
    let modifiers = parse_modifiers_list(document.get("placement"), id)?;
    let placed = PlacedFeature {
        id: id.to_string(),
        feature,
        modifiers,
    };
    placed.validate_attempt_budget(0)?;
    Ok(placed)
}

pub(super) fn parse_placement_modifier(
    id: &str,
    index: usize,
    entry: &Value,
) -> Result<PlacementModifier, WorldgenError> {
    let kind = entry
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| WorldgenError::Invalid(format!("{id}: placement[{index}] has no type")))?;
    match kind {
        "minecraft:count" => {
            let count = entry.get("count").ok_or_else(|| {
                WorldgenError::Invalid(format!("{id}: count placement without a count"))
            })?;
            let count = RangeProvider::from_value(count, false).ok_or_else(|| {
                WorldgenError::Invalid(format!("{id}: count placement is not an int provider"))
            })?;
            if matches!(count.range(0, 0), Some((min, max)) if !(0..=256).contains(&min) || !(0..=256).contains(&max))
            {
                return Err(WorldgenError::Invalid(format!(
                    "{id}: count placement must be in 0..=256"
                )));
            }
            Ok(PlacementModifier::Count { count })
        }
        "minecraft:rarity_filter" => {
            let chance = entry.get("chance").and_then(Value::as_i64).ok_or_else(|| {
                WorldgenError::Invalid(format!("{id}: rarity_filter without a chance"))
            })?;
            if chance <= 0 {
                return Err(WorldgenError::Invalid(format!(
                    "{id}: rarity_filter chance must be positive, got {chance}"
                )));
            }
            Ok(PlacementModifier::RarityFilter { chance })
        }
        "minecraft:random_chance" => {
            let chance = entry.get("chance").and_then(Value::as_f64).ok_or_else(|| {
                WorldgenError::Invalid(format!("{id}: random_chance without a chance"))
            })? as f32;
            if !(0.0..=1.0).contains(&chance) {
                return Err(WorldgenError::Invalid(format!(
                    "{id}: random_chance must be in 0.0..=1.0, got {chance}"
                )));
            }
            Ok(PlacementModifier::RandomChance { chance })
        }
        "minecraft:in_square" => Ok(PlacementModifier::InSquare),
        "minecraft:heightmap" => {
            let name = entry
                .get("heightmap")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    WorldgenError::Invalid(format!("{id}: heightmap modifier without heightmap"))
                })?;
            let heightmap = HeightmapKind::parse(name)
                .ok_or_else(|| WorldgenError::Invalid(format!("{id}: unknown heightmap {name}")))?;
            Ok(PlacementModifier::Heightmap { heightmap })
        }
        "minecraft:offset" => {
            let axis = |key: &str| -> Result<RangeProvider, WorldgenError> {
                let raw = entry.get(key).ok_or_else(|| {
                    WorldgenError::Invalid(format!("{id}: offset modifier without {key}"))
                })?;
                let rp = RangeProvider::from_value(raw, false).ok_or_else(|| {
                    WorldgenError::Invalid(format!("{id}: offset {key} is not an int provider"))
                })?;
                if matches!(rp.range(0, 0), Some((min, max)) if !(-16..=16).contains(&min) || !(-16..=16).contains(&max))
                {
                    return Err(WorldgenError::Invalid(format!(
                        "{id}: offset {key} must be in -16..=16"
                    )));
                }
                Ok(rp)
            };
            Ok(PlacementModifier::Offset {
                x: axis("x")?,
                y: axis("y")?,
                z: axis("z")?,
            })
        }
        "minecraft:height_range" => {
            let height = entry.get("height").ok_or_else(|| {
                WorldgenError::Invalid(format!("{id}: height_range without a height"))
            })?;
            Ok(PlacementModifier::HeightRange {
                height: RangeProvider::from_value(height, true).ok_or_else(|| {
                    WorldgenError::Invalid(format!("{id}: height_range is not an int provider"))
                })?,
            })
        }
        "minecraft:biome" => Ok(PlacementModifier::Biome),
        "minecraft:block_predicate_filter" => Ok(PlacementModifier::BlockPredicateFilter {
            predicate: parse_block_predicate(entry.get("predicate")),
        }),
        "minecraft:surface_relative_threshold_filter" => {
            let heightmap = entry
                .get("heightmap")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    WorldgenError::Invalid(format!(
                        "{id}: surface_relative_threshold_filter missing heightmap"
                    ))
                })?
                .to_string();
            let min_inclusive = match entry.get("min_inclusive") {
                Some(v) => v
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| {
                        WorldgenError::Invalid(format!(
                            "{id}: surface_relative_threshold_filter invalid min_inclusive"
                        ))
                    })?,
                None => i32::MIN,
            };
            let max_inclusive = match entry.get("max_inclusive") {
                Some(v) => v
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| {
                        WorldgenError::Invalid(format!(
                            "{id}: surface_relative_threshold_filter invalid max_inclusive"
                        ))
                    })?,
                None => i32::MAX,
            };
            Ok(PlacementModifier::SurfaceRelativeThresholdFilter {
                heightmap,
                min_inclusive,
                max_inclusive,
            })
        }
        "minecraft:environment_scan" => {
            let dir_str = entry
                .get("direction_of_search")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    WorldgenError::Invalid(format!(
                        "{id}: environment_scan missing direction_of_search"
                    ))
                })?;
            let direction_of_search = parse_direction_name(dir_str).ok_or_else(|| {
                WorldgenError::Invalid(format!(
                    "{id}: environment_scan invalid direction {dir_str}"
                ))
            })?;
            let max_steps = entry
                .get("max_steps")
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .ok_or_else(|| {
                    WorldgenError::Invalid(format!("{id}: environment_scan missing max_steps"))
                })?;
            // Minecraft's EnvironmentScanFilter codec accepts `max_steps` in `1..=32`.
            if !(1..=32).contains(&max_steps) {
                return Err(WorldgenError::Invalid(format!(
                    "{id}: environment_scan max_steps must be in 1..=32, got {max_steps}"
                )));
            }
            let target_condition = parse_block_predicate(entry.get("target_condition"));
            let allowed_search_condition = entry
                .get("allowed_search_condition")
                .map(|cond| parse_block_predicate(Some(cond)));
            Ok(PlacementModifier::EnvironmentScan {
                direction_of_search,
                max_steps,
                target_condition,
                allowed_search_condition,
            })
        }
        other => Ok(PlacementModifier::Unsupported {
            kind: other.to_string(),
        }),
    }
}

/// Parses vanilla's `Direction` codec name (`down`/`up`/`north`/`south`/`west`/`east`).
pub(super) fn parse_direction_name(name: &str) -> Option<Direction> {
    match name {
        "down" | "DOWN" => Some(Direction::Down),
        "up" | "UP" => Some(Direction::Up),
        "north" | "NORTH" => Some(Direction::North),
        "south" | "SOUTH" => Some(Direction::South),
        "west" | "WEST" => Some(Direction::West),
        "east" | "EAST" => Some(Direction::East),
        _ => None,
    }
}

/// Parses a biome's positional `features` array into one list per step.
pub(super) fn parse_biome_steps(
    id: &str,
    document: &Value,
    placed: &HashMap<String, Rc<PlacedFeature>>,
    configured: &HashMap<String, Rc<ConfiguredFeature>>,
    placed_ids: &[String],
    configured_ids: &[String],
    inline_sites: &mut usize,
) -> Result<Vec<Vec<Rc<PlacedFeature>>>, WorldgenError> {
    let Some(steps) = document.get("features").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    steps
        .iter()
        .enumerate()
        .map(|(step, entries)| {
            let list = entries.as_array().cloned().unwrap_or_default();
            list.iter()
                .map(|entry| {
                    let name = entry.as_str().ok_or_else(|| {
                        WorldgenError::Invalid(format!(
                            "{id}: features[{step}] entries must be identifiers"
                        ))
                    })?;
                    if let Some(feature) = placed.get(name) {
                        return Ok(Rc::clone(feature));
                    }
                    // An inline configured feature (no placed wrapper) has
                    // no decorations; it is still a stream position, so it
                    // is kept as a placed feature with an empty modifier
                    // list when the kind is known, and rejected otherwise.
                    if configured.contains_key(name) {
                        *inline_sites += 1;
                        return Ok(Rc::new(PlacedFeature {
                            id: format!("{name}#{}", *inline_sites - 1),
                            feature: Rc::clone(&configured[name]),
                            modifiers: Vec::new(),
                        }));
                    }
                    Err(WorldgenError::Invalid(format!(
                        "{id}: features[{step}] references unknown {name} (known placed: {}; known configured: {})",
                        placed_ids.len(),
                        configured_ids.len()
                    )))
                })
                .collect()
        })
        .collect()
}
