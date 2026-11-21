//! Resolution of block state provider, placed feature and selector
//! references after the registry is loaded.

use super::*;

/// Inserts or overrides one integer property on a rendered block state string,
/// keeping property keys in sorted order to match [`state_from_object`].
pub(super) fn set_state_property(state: &str, property: &str, value: i32) -> String {
    set_state_property_str(state, property, &value.to_string())
}

/// Inserts or overrides one property on a rendered block state string, keeping
/// property keys in sorted order to match [`state_from_object`].
pub(super) fn set_state_property_str(state: &str, property: &str, value_str: &str) -> String {
    let (id, rest) = state
        .split_once('[')
        .map_or((state, None), |(base, tail)| (base, Some(tail)));
    let mut properties: BTreeMap<&str, String> = BTreeMap::new();
    if let Some(tail) = rest {
        let props_str = tail.strip_suffix(']').unwrap_or(tail);
        for pair in props_str.split(',') {
            let pair = pair.trim();
            if let Some((k, v)) = pair.split_once('=') {
                properties.insert(k.trim(), v.trim().to_string());
            }
        }
    }
    properties.insert(property, value_str.to_string());
    let rendered: Vec<String> = properties
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    format!("{id}[{}]", rendered.join(","))
}

pub(super) fn resolve_state_provider_references(
    provider: &mut StateProvider,
    registry: &HashMap<String, StateProvider>,
    depth: usize,
) {
    if depth >= 16 {
        return;
    }
    match provider {
        StateProvider::Reference { id } => {
            if let Some(resolved) = registry.get(id.as_str()) {
                *provider = resolved.clone();
                resolve_state_provider_references(provider, registry, depth + 1);
            }
        }
        StateProvider::RuleBased { fallback, rules } => {
            resolve_state_provider_references(fallback, registry, depth + 1);
            for rule in rules {
                resolve_state_provider_references(&mut rule.then, registry, depth + 1);
            }
        }
        StateProvider::RandomizedInt { source, .. }
        | StateProvider::Rotated { state: source, .. } => {
            resolve_state_provider_references(source, registry, depth + 1);
        }
        StateProvider::Constant { .. }
        | StateProvider::Weighted { .. }
        | StateProvider::Unsupported { .. } => {}
    }
}

pub(super) fn resolve_configured_providers(
    feature: &mut ConfiguredFeature,
    registry: &HashMap<String, StateProvider>,
) {
    match feature {
        ConfiguredFeature::SimpleBlock { to_place } => {
            resolve_state_provider_references(to_place, registry, 0);
        }
        ConfiguredFeature::Disk { state_provider, .. } => {
            resolve_state_provider_references(state_provider, registry, 0);
        }
        ConfiguredFeature::BlockColumn { layers, .. } => {
            for layer in layers {
                resolve_state_provider_references(&mut layer.provider, registry, 0);
            }
        }
        ConfiguredFeature::RandomPatch { feature: child, .. } => {
            resolve_placed_providers(child, registry);
        }
        ConfiguredFeature::SimpleRandomSelector { features } => {
            for child in features {
                resolve_placed_providers(Rc::make_mut(child), registry);
            }
        }
        ConfiguredFeature::RandomSelector { features, default } => {
            for (child, _) in features {
                resolve_placed_providers(Rc::make_mut(child), registry);
            }
            resolve_placed_providers(Rc::make_mut(default), registry);
        }
        ConfiguredFeature::WeightedRandomSelector { features } => {
            for (child, _) in features {
                resolve_placed_providers(Rc::make_mut(child), registry);
            }
        }
        ConfiguredFeature::RandomBooleanSelector {
            feature_true,
            feature_false,
        } => {
            resolve_placed_providers(Rc::make_mut(feature_true), registry);
            resolve_placed_providers(Rc::make_mut(feature_false), registry);
        }
        ConfiguredFeature::Ore { .. }
        | ConfiguredFeature::Spring { .. }
        | ConfiguredFeature::Unsupported { .. } => {}
    }
}

pub(super) fn resolve_placed_providers(
    placed: &mut PlacedFeature,
    registry: &HashMap<String, StateProvider>,
) {
    resolve_configured_providers(Rc::make_mut(&mut placed.feature), registry);
}

pub(super) fn resolve_child_placed_feature(
    child: &mut Rc<PlacedFeature>,
    configured: &HashMap<String, Rc<ConfiguredFeature>>,
    placed: &HashMap<String, Rc<PlacedFeature>>,
    providers: &HashMap<String, StateProvider>,
    depth: usize,
) {
    if depth >= 8 {
        return;
    }
    if let ConfiguredFeature::Unsupported { kind } = child.feature.as_ref() {
        if child.id == *kind
            && let Some(resolved_placed) = placed.get(&child.id)
        {
            *child = Rc::clone(resolved_placed);
            return;
        }
        if let Some(resolved_configured) = configured.get(kind) {
            Rc::make_mut(child).feature = Rc::clone(resolved_configured);
        }
    }
    resolve_placed_providers(Rc::make_mut(child), providers);
    resolve_nested_placed(
        Rc::make_mut(child),
        configured,
        placed,
        providers,
        depth + 1,
    );
}

pub(super) fn resolve_nested_placed(
    pf: &mut PlacedFeature,
    configured: &HashMap<String, Rc<ConfiguredFeature>>,
    placed: &HashMap<String, Rc<PlacedFeature>>,
    providers: &HashMap<String, StateProvider>,
    depth: usize,
) {
    match Rc::make_mut(&mut pf.feature) {
        ConfiguredFeature::RandomPatch { feature: child, .. } => {
            if let ConfiguredFeature::Unsupported { kind } = child.feature.as_ref()
                && child.id == *kind
                && let Some(resolved) = placed.get(&child.id)
            {
                **child = resolved.as_ref().clone();
            }
            resolve_placed_providers(child, providers);
        }
        ConfiguredFeature::SimpleRandomSelector { features } => {
            for child in features {
                resolve_child_placed_feature(child, configured, placed, providers, depth);
            }
        }
        ConfiguredFeature::RandomSelector { features, default } => {
            for (child, _) in features {
                resolve_child_placed_feature(child, configured, placed, providers, depth);
            }
            resolve_child_placed_feature(default, configured, placed, providers, depth);
        }
        ConfiguredFeature::WeightedRandomSelector { features } => {
            for (child, _) in features {
                resolve_child_placed_feature(child, configured, placed, providers, depth);
            }
        }
        ConfiguredFeature::RandomBooleanSelector {
            feature_true,
            feature_false,
        } => {
            resolve_child_placed_feature(feature_true, configured, placed, providers, depth);
            resolve_child_placed_feature(feature_false, configured, placed, providers, depth);
        }
        _ => {}
    }
}

/// Resolves named child feature references inside selector configured features
/// against loaded placed, configured, and state-provider registries.
pub(super) fn resolve_selector_references(
    configured: &mut HashMap<String, Rc<ConfiguredFeature>>,
    placed: &HashMap<String, Rc<PlacedFeature>>,
    providers: &HashMap<String, StateProvider>,
) {
    let configured_snapshot = configured.clone();
    for feature in configured.values_mut() {
        match Rc::make_mut(feature) {
            ConfiguredFeature::SimpleRandomSelector { features } => {
                for child in features.iter_mut() {
                    resolve_child_placed_feature(child, &configured_snapshot, placed, providers, 0);
                }
            }
            ConfiguredFeature::RandomSelector { features, default } => {
                for (child, _) in features.iter_mut() {
                    resolve_child_placed_feature(child, &configured_snapshot, placed, providers, 0);
                }
                resolve_child_placed_feature(default, &configured_snapshot, placed, providers, 0);
            }
            ConfiguredFeature::WeightedRandomSelector { features } => {
                for (child, _) in features.iter_mut() {
                    resolve_child_placed_feature(child, &configured_snapshot, placed, providers, 0);
                }
            }
            ConfiguredFeature::RandomBooleanSelector {
                feature_true,
                feature_false,
            } => {
                resolve_child_placed_feature(
                    feature_true,
                    &configured_snapshot,
                    placed,
                    providers,
                    0,
                );
                resolve_child_placed_feature(
                    feature_false,
                    &configured_snapshot,
                    placed,
                    providers,
                    0,
                );
            }
            _ => {}
        }
    }
}
