//! Diagnosis of feature pieces this runtime does not model yet, so
//! unsupported features are skipped visibly instead of approximated.

use super::*;

pub(super) fn record_unmodeled_range(counts: &mut HashMap<String, usize>, range: &RangeProvider) {
    match range {
        RangeProvider::Unsupported { kind } => {
            *counts.entry(format!("range:{kind}")).or_insert(0) += 1;
        }
        RangeProvider::Clamped { source, .. } => record_unmodeled_range(counts, source),
        RangeProvider::WeightedList { distribution } => {
            for (entry, _) in distribution {
                record_unmodeled_range(counts, entry);
            }
        }
        _ => {}
    }
}

/// Counts unmodeled configured features and their inner components for the startup gap report.
pub(super) fn record_unmodeled_configured(
    counts: &mut HashMap<String, usize>,
    feature: &ConfiguredFeature,
) {
    match feature {
        ConfiguredFeature::Unsupported { kind } => {
            *counts.entry(kind.clone()).or_insert(0) += 1;
        }
        ConfiguredFeature::Ore { targets, .. } => {
            for target in targets {
                record_unmodeled_rule(counts, &target.predicate);
            }
        }
        ConfiguredFeature::Disk {
            state_provider,
            target_predicate,
            radius,
            ..
        } => {
            record_unmodeled_provider(counts, state_provider);
            record_unmodeled_predicates(counts, target_predicate);
            record_unmodeled_range(counts, radius);
        }
        ConfiguredFeature::Spring { .. } => {}
        ConfiguredFeature::RandomPatch { feature: child, .. } => {
            if feature.patch_reach() > MAX_PATCH_REACH {
                *counts
                    .entry("minecraft:random_patch:reach".to_string())
                    .or_insert(0) += 1;
            }
            record_unmodeled_modifiers(counts, child);
            record_unmodeled_configured(counts, &child.feature);
        }
        ConfiguredFeature::SimpleBlock { to_place } => {
            record_unmodeled_provider(counts, to_place);
            record_unplaceable_states(counts, to_place);
        }
        ConfiguredFeature::SimpleRandomSelector { features } => {
            if features.is_empty() {
                *counts
                    .entry("minecraft:simple_random_selector:empty".to_string())
                    .or_insert(0) += 1;
            }
            for child in features {
                record_unmodeled_modifiers(counts, child);
                record_unmodeled_configured(counts, &child.feature);
            }
        }
        ConfiguredFeature::RandomSelector { features, default } => {
            for (child, _) in features {
                record_unmodeled_modifiers(counts, child);
                record_unmodeled_configured(counts, &child.feature);
            }
            record_unmodeled_modifiers(counts, default);
            record_unmodeled_configured(counts, &default.feature);
        }
        ConfiguredFeature::WeightedRandomSelector { features } => {
            if features.is_empty() {
                *counts
                    .entry("minecraft:weighted_random_selector:empty".to_string())
                    .or_insert(0) += 1;
            }
            for (child, _) in features {
                record_unmodeled_modifiers(counts, child);
                record_unmodeled_configured(counts, &child.feature);
            }
        }
        ConfiguredFeature::RandomBooleanSelector {
            feature_true,
            feature_false,
        } => {
            record_unmodeled_modifiers(counts, feature_true);
            record_unmodeled_configured(counts, &feature_true.feature);
            record_unmodeled_modifiers(counts, feature_false);
            record_unmodeled_configured(counts, &feature_false.feature);
        }
        ConfiguredFeature::BlockColumn {
            allowed_placement,
            layers,
            ..
        } => {
            record_unmodeled_predicates(counts, allowed_placement);
            for layer in layers {
                record_unmodeled_range(counts, &layer.height);
                record_unmodeled_provider(counts, &layer.provider);
                record_unplaceable_states(counts, &layer.provider);
            }
        }
    }
}

/// Counts the rule-test kinds a target uses without a runtime here, so the
/// startup report names them the same way it names unmodeled feature types.
pub(super) fn record_unmodeled_rule(counts: &mut HashMap<String, usize>, rule: &RuleTest) {
    match rule {
        RuleTest::Unsupported { kind } => {
            *counts.entry(format!("target:{kind}")).or_insert(0) += 1;
        }
        RuleTest::AnyOf(rules) | RuleTest::AllOf(rules) => {
            for nested in rules {
                record_unmodeled_rule(counts, nested);
            }
        }
        RuleTest::Not(nested) => record_unmodeled_rule(counts, nested),
        _ => {}
    }
}

/// Counts the placement-modifier kinds a placed-feature document asks for
/// without a runtime here, keyed `placement:<kind>` to keep those namespaced
/// kinds from reading as configured-feature types or as `target:` rules. The
/// prefixes are a naming convention, not a namespace check: a configured type
/// whose name itself began `placement:` would land in the same key, which no
/// provisioned pack does today. One increment per document carrying the kind,
/// matching the configured-kind counters: the report answers "which placed
/// features does this pack ask for that this generator cannot run", not how
/// many times a chain repeats a decoration, and it counts documents rather
/// than registry entries, so two files with the same identifier stem would
/// each contribute. An unmodeled modifier keeps its place in the chain and the
/// chain then yields nothing, so without this key a feature whose configured
/// type *is* modeled — `minecraft:ore` — would be
/// missing from the gap report entirely.
pub(super) fn record_unmodeled_modifiers(
    counts: &mut HashMap<String, usize>,
    placed: &PlacedFeature,
) {
    let mut seen: HashSet<String> = HashSet::new();
    for modifier in &placed.modifiers {
        match modifier {
            PlacementModifier::Unsupported { kind } => {
                let key = format!("placement:{kind}");
                if seen.insert(key.clone()) {
                    *counts.entry(key).or_insert(0) += 1;
                }
            }
            PlacementModifier::BlockPredicateFilter { predicate } => {
                for kind in unmodeled_predicates(predicate) {
                    for key in [
                        "placement:minecraft:block_predicate_filter".to_string(),
                        format!("predicate:{kind}"),
                    ] {
                        if seen.insert(key.clone()) {
                            *counts.entry(key).or_insert(0) += 1;
                        }
                    }
                }
            }
            PlacementModifier::SurfaceRelativeThresholdFilter { heightmap, .. }
                if heightmap != "OCEAN_FLOOR_WG" =>
            {
                let key =
                    format!("placement:minecraft:surface_relative_threshold_filter:{heightmap}");
                if seen.insert(key.clone()) {
                    *counts.entry(key).or_insert(0) += 1;
                }
            }
            PlacementModifier::EnvironmentScan {
                target_condition,
                allowed_search_condition,
                ..
            } => {
                let mut kinds = unmodeled_predicates(target_condition);
                if let Some(allowed) = allowed_search_condition {
                    kinds.extend(unmodeled_predicates(allowed));
                }
                for kind in kinds {
                    for key in [
                        "placement:minecraft:environment_scan".to_string(),
                        format!("predicate:{kind}"),
                    ] {
                        if seen.insert(key.clone()) {
                            *counts.entry(key).or_insert(0) += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// The unmodeled predicate types one `minecraft:block_predicate_filter` nests,
/// in the order the document writes them.
pub(super) fn unmodeled_predicates(predicate: &BlockPredicate) -> Vec<&str> {
    let mut kinds = Vec::new();
    match predicate {
        BlockPredicate::Unsupported { kind } => kinds.push(kind.as_str()),
        BlockPredicate::WouldSurvive { state, .. }
            if !plant_support::supports_survival_state(state) =>
        {
            kinds.push("minecraft:would_survive");
        }
        BlockPredicate::AnyOf(rules) | BlockPredicate::AllOf(rules) => {
            for nested in rules {
                kinds.extend(unmodeled_predicates(nested));
            }
        }
        BlockPredicate::Not(nested) => kinds.extend(unmodeled_predicates(nested)),
        _ => {}
    }
    kinds
}

/// Counts the block-state provider shapes a modeled configured feature asks
/// for without a runtime here, keyed `provider:<kind>`. Without it the
/// configured type's own key would disappear from the report as soon as the
/// type was modeled, and a `minecraft:simple_block` whose state comes from a
/// provider this runtime does not resolve would look like no gap at all.
/// A rule-based provider reports the predicates its rules cannot evaluate,
/// keyed `predicate:<kind>`, alongside its nested providers.
pub(super) fn record_unmodeled_provider(
    counts: &mut HashMap<String, usize>,
    provider: &StateProvider,
) {
    match provider {
        StateProvider::Unsupported { kind } => {
            *counts.entry(format!("provider:{kind}")).or_insert(0) += 1;
        }
        StateProvider::Reference { .. } => {
            *counts
                .entry("provider:minecraft:block_state_provider_reference".to_string())
                .or_insert(0) += 1;
        }
        StateProvider::RuleBased { fallback, rules } => {
            for rule in rules {
                record_unmodeled_predicates(counts, &rule.if_true);
                record_unmodeled_provider(counts, &rule.then);
            }
            record_unmodeled_provider(counts, fallback);
        }
        StateProvider::RandomizedInt { source, values, .. } => {
            record_unmodeled_provider(counts, source);
            record_unmodeled_range(counts, values);
        }
        StateProvider::Rotated { state, .. } => record_unmodeled_provider(counts, state),
        StateProvider::Constant { .. } | StateProvider::Weighted { .. } => {}
    }
}

/// Counts each predicate type a modeled configured feature nests without a
/// runtime here, keyed `predicate:<kind>`.
pub(super) fn record_unmodeled_predicates(
    counts: &mut HashMap<String, usize>,
    predicate: &BlockPredicate,
) {
    for kind in unmodeled_predicates(predicate) {
        *counts.entry(format!("predicate:{kind}")).or_insert(0) += 1;
    }
}

/// Counts the states a `minecraft:simple_block` could write but has no ground
/// rule for, keyed `state:<id>`, so the feature they gate is not a silent gap.
pub(super) fn record_unplaceable_states(
    counts: &mut HashMap<String, usize>,
    provider: &StateProvider,
) {
    for state in provider.states() {
        if !plant_support::is_placeable(state) {
            *counts
                .entry(format!("state:{}", base_block_id(state)))
                .or_insert(0) += 1;
        }
    }
}

pub(super) fn push_unique_piece(pieces: &mut Vec<String>, seen: &mut HashSet<String>, key: String) {
    if seen.insert(key.clone()) {
        pieces.push(key);
    }
}

pub(super) fn collect_unmodeled_range_pieces(
    range: &RangeProvider,
    pieces: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match range {
        RangeProvider::Unsupported { kind } => {
            push_unique_piece(pieces, seen, format!("range:{kind}"));
        }
        RangeProvider::Clamped { source, .. } => {
            collect_unmodeled_range_pieces(source, pieces, seen);
        }
        RangeProvider::WeightedList { distribution } => {
            for (entry, _) in distribution {
                collect_unmodeled_range_pieces(entry, pieces, seen);
            }
        }
        _ => {}
    }
}

/// Collects every unsupported piece of one placed feature in evaluation order:
/// placement modifiers in list order, then the configured feature and its
/// nested components.
pub(super) fn collect_unmodeled_placed_pieces(
    placed: &PlacedFeature,
    pieces: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    for modifier in &placed.modifiers {
        match modifier {
            PlacementModifier::Unsupported { kind } => {
                push_unique_piece(pieces, seen, format!("placement:{kind}"));
            }
            PlacementModifier::Count { count } => {
                collect_unmodeled_range_pieces(count, pieces, seen);
            }
            PlacementModifier::HeightRange { height } => {
                collect_unmodeled_range_pieces(height, pieces, seen);
            }
            PlacementModifier::Offset { x, y, z } => {
                collect_unmodeled_range_pieces(x, pieces, seen);
                collect_unmodeled_range_pieces(y, pieces, seen);
                collect_unmodeled_range_pieces(z, pieces, seen);
            }
            PlacementModifier::BlockPredicateFilter { predicate } => {
                for kind in unmodeled_predicates(predicate) {
                    push_unique_piece(pieces, seen, format!("predicate:{kind}"));
                }
            }
            PlacementModifier::SurfaceRelativeThresholdFilter { heightmap, .. }
                if heightmap != "OCEAN_FLOOR_WG" =>
            {
                push_unique_piece(
                    pieces,
                    seen,
                    format!("placement:minecraft:surface_relative_threshold_filter:{heightmap}"),
                );
            }
            PlacementModifier::EnvironmentScan {
                target_condition,
                allowed_search_condition,
                ..
            } => {
                for kind in unmodeled_predicates(target_condition) {
                    push_unique_piece(pieces, seen, format!("predicate:{kind}"));
                }
                if let Some(allowed) = allowed_search_condition {
                    for kind in unmodeled_predicates(allowed) {
                        push_unique_piece(pieces, seen, format!("predicate:{kind}"));
                    }
                }
            }
            _ => {}
        }
    }
    if placed.patch_reach() > MAX_PATCH_REACH
        && !matches!(*placed.feature, ConfiguredFeature::RandomPatch { .. })
    {
        push_unique_piece(pieces, seen, "placement:minecraft:offset:reach".to_string());
    }
    collect_unmodeled_configured_pieces(&placed.feature, pieces, seen);
    if placed.validate_attempt_budget(0).is_err() {
        push_unique_piece(pieces, seen, "placement:attempt_budget".to_string());
    }
}

pub(super) fn collect_unmodeled_configured_pieces(
    feature: &ConfiguredFeature,
    pieces: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match feature {
        ConfiguredFeature::Unsupported { kind } => {
            push_unique_piece(pieces, seen, kind.clone());
        }
        ConfiguredFeature::Ore { targets, .. } => {
            for target in targets {
                collect_unmodeled_rule_pieces(&target.predicate, pieces, seen);
            }
        }
        ConfiguredFeature::Disk {
            state_provider,
            target_predicate,
            radius,
            ..
        } => {
            collect_unmodeled_provider_pieces(state_provider, pieces, seen);
            for kind in unmodeled_predicates(target_predicate) {
                push_unique_piece(pieces, seen, format!("predicate:{kind}"));
            }
            collect_unmodeled_range_pieces(radius, pieces, seen);
        }
        ConfiguredFeature::Spring { .. } => {}
        ConfiguredFeature::RandomPatch { feature: child, .. } => {
            if feature.patch_reach() > MAX_PATCH_REACH {
                push_unique_piece(pieces, seen, "minecraft:random_patch:reach".to_string());
            }
            collect_unmodeled_placed_pieces(child, pieces, seen);
        }
        ConfiguredFeature::SimpleBlock { to_place } => {
            collect_unmodeled_provider_pieces(to_place, pieces, seen);
            for state in to_place.states() {
                if !plant_support::is_placeable(state) {
                    push_unique_piece(pieces, seen, format!("state:{}", base_block_id(state)));
                }
            }
        }
        ConfiguredFeature::SimpleRandomSelector { features } => {
            if features.is_empty() {
                push_unique_piece(
                    pieces,
                    seen,
                    "minecraft:simple_random_selector:empty".to_string(),
                );
            }
            for child in features {
                collect_unmodeled_placed_pieces(child, pieces, seen);
            }
        }
        ConfiguredFeature::RandomSelector { features, default } => {
            for (child, _) in features {
                collect_unmodeled_placed_pieces(child, pieces, seen);
            }
            collect_unmodeled_placed_pieces(default, pieces, seen);
        }
        ConfiguredFeature::WeightedRandomSelector { features } => {
            if features.is_empty() {
                push_unique_piece(
                    pieces,
                    seen,
                    "minecraft:weighted_random_selector:empty".to_string(),
                );
            }
            for (child, _) in features {
                collect_unmodeled_placed_pieces(child, pieces, seen);
            }
        }
        ConfiguredFeature::RandomBooleanSelector {
            feature_true,
            feature_false,
        } => {
            collect_unmodeled_placed_pieces(feature_true, pieces, seen);
            collect_unmodeled_placed_pieces(feature_false, pieces, seen);
        }
        ConfiguredFeature::BlockColumn {
            allowed_placement,
            layers,
            ..
        } => {
            for kind in unmodeled_predicates(allowed_placement) {
                push_unique_piece(pieces, seen, format!("predicate:{kind}"));
            }
            for layer in layers {
                collect_unmodeled_range_pieces(&layer.height, pieces, seen);
                collect_unmodeled_provider_pieces(&layer.provider, pieces, seen);
                for state in layer.provider.states() {
                    if !plant_support::is_placeable(state) {
                        push_unique_piece(pieces, seen, format!("state:{}", base_block_id(state)));
                    }
                }
            }
        }
    }
}

pub(super) fn collect_unmodeled_rule_pieces(
    rule: &RuleTest,
    pieces: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match rule {
        RuleTest::Unsupported { kind } => {
            push_unique_piece(pieces, seen, format!("target:{kind}"));
        }
        RuleTest::AnyOf(rules) | RuleTest::AllOf(rules) => {
            for nested in rules {
                collect_unmodeled_rule_pieces(nested, pieces, seen);
            }
        }
        RuleTest::Not(nested) => collect_unmodeled_rule_pieces(nested, pieces, seen),
        _ => {}
    }
}

pub(super) fn collect_unmodeled_provider_pieces(
    provider: &StateProvider,
    pieces: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match provider {
        StateProvider::Unsupported { kind } => {
            push_unique_piece(pieces, seen, format!("provider:{kind}"));
        }
        StateProvider::Reference { .. } => {
            push_unique_piece(
                pieces,
                seen,
                "provider:minecraft:block_state_provider_reference".to_string(),
            );
        }
        StateProvider::RuleBased { fallback, rules } => {
            for rule in rules {
                for kind in unmodeled_predicates(&rule.if_true) {
                    push_unique_piece(pieces, seen, format!("predicate:{kind}"));
                }
                collect_unmodeled_provider_pieces(&rule.then, pieces, seen);
            }
            collect_unmodeled_provider_pieces(fallback, pieces, seen);
        }
        StateProvider::RandomizedInt {
            source,
            property,
            values,
        } => {
            if property.is_empty() {
                push_unique_piece(
                    pieces,
                    seen,
                    "provider:minecraft:randomized_int".to_string(),
                );
            }
            collect_unmodeled_provider_pieces(source, pieces, seen);
            collect_unmodeled_range_pieces(values, pieces, seen);
        }
        StateProvider::Rotated { state, .. } => {
            collect_unmodeled_provider_pieces(state, pieces, seen);
        }
        StateProvider::Constant { .. } | StateProvider::Weighted { .. } => {}
    }
}
