//! Placement modifiers and placed features: where a configured feature
//! is attempted inside a chunk.

use super::*;

/// One decoration step of a placed feature.
#[derive(Debug, Clone, PartialEq)]
pub enum PlacementModifier {
    /// `minecraft:count` — how many attempts this feature makes.
    Count { count: RangeProvider },
    /// `minecraft:rarity_filter` — one attempt per `chance` on average.
    RarityFilter { chance: i64 },
    /// `minecraft:random_chance` — keeps the candidate with probability `chance`.
    ///
    /// Source: minecraft.wiki `Placed feature` (revision 3816343):
    /// "Gates the feature being placed behind a random chance."
    RandomChance { chance: f32 },
    /// `minecraft:in_square` — a horizontal offset inside the chunk.
    InSquare,
    /// `minecraft:heightmap` — snaps Y to one block above the named heightmap.
    ///
    /// Source: minecraft.wiki `Placed feature` (revision 3816343):
    /// "Sets the Y coordinate to one block above the heightmap."
    Heightmap { heightmap: HeightmapKind },
    /// `minecraft:offset` — offsets `(x, y, z)` by per-axis int providers in `-16..=16`.
    ///
    /// Source: minecraft.wiki `Placed feature` (revision 3816343):
    /// "Applies an offset to the current position."
    Offset {
        x: RangeProvider,
        y: RangeProvider,
        z: RangeProvider,
    },
    /// `minecraft:height_range` — the vertical band of the attempt.
    HeightRange { height: RangeProvider },
    /// `minecraft:biome` — keep the position only if the biome at it lists
    /// this feature.
    Biome,
    /// `minecraft:block_predicate_filter` — keep the candidate only if the
    /// predicate holds around it.
    BlockPredicateFilter { predicate: BlockPredicate },
    /// `minecraft:surface_relative_threshold_filter` — keeps the candidate when
    /// `pos.y - ocean_floor_height` lies inside `[min_inclusive, max_inclusive]`.
    SurfaceRelativeThresholdFilter {
        heightmap: String,
        min_inclusive: i32,
        max_inclusive: i32,
    },
    /// `minecraft:environment_scan` — walks along `direction_of_search` up to
    /// `max_steps` while `allowed_search_condition` holds, advancing on the
    /// first position where `target_condition` holds.
    EnvironmentScan {
        direction_of_search: Direction,
        max_steps: i32,
        target_condition: BlockPredicate,
        allowed_search_condition: Option<BlockPredicate>,
    },
    /// A decoration without a runtime here. Named, never skipped silently.
    Unsupported { kind: String },
}

impl PlacementModifier {
    pub(super) fn costs_no_draws(&self) -> bool {
        matches!(
            self,
            Self::Heightmap { .. }
                | Self::Biome
                | Self::BlockPredicateFilter { .. }
                | Self::SurfaceRelativeThresholdFilter { .. }
                | Self::EnvironmentScan { .. }
                | Self::Unsupported { .. }
        )
    }

    pub(super) fn horizontal_step(&self) -> Option<(i32, i32, i32, i32)> {
        match self {
            Self::InSquare => Some((0, 15, 0, 15)),
            Self::Offset { x, z, .. } => {
                let (xmin, xmax) = x.range(0, 0)?;
                let (zmin, zmax) = z.range(0, 0)?;
                Some((
                    xmin.clamp(-16, 16),
                    xmax.clamp(-16, 16),
                    zmin.clamp(-16, 16),
                    zmax.clamp(-16, 16),
                ))
            }
            Self::EnvironmentScan {
                direction_of_search,
                max_steps,
                ..
            } => {
                let (dx, _, dz) = direction_of_search.unit_vector();
                let sx = dx.saturating_mul(*max_steps);
                let sz = dz.saturating_mul(*max_steps);
                Some((sx.min(0), sx.max(0), sz.min(0), sz.max(0)))
            }
            Self::Count { .. }
            | Self::RarityFilter { .. }
            | Self::RandomChance { .. }
            | Self::Heightmap { .. }
            | Self::HeightRange { .. }
            | Self::Biome
            | Self::BlockPredicateFilter { .. }
            | Self::SurfaceRelativeThresholdFilter { .. }
            | Self::Unsupported { .. } => Some((0, 0, 0, 0)),
        }
    }
}

/// A placed feature: one configured feature plus its ordered decorations.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedFeature {
    pub id: String,
    pub feature: Rc<ConfiguredFeature>,
    pub modifiers: Vec<PlacementModifier>,
}

impl PlacedFeature {
    /// Validates that total feature attempts and nesting depth stay bounded.
    ///
    /// Multiplies placement count modifiers by child configured-feature tries and
    /// any nested placed feature attempts, ensuring total work per feature does not
    /// exceed 4,096 and nesting depth does not exceed 2.
    pub fn validate_attempt_budget(&self, depth: usize) -> Result<u32, WorldgenError> {
        self.validate_budget(depth, 0)
    }

    pub(super) fn validate_budget(
        &self,
        patch_depth: usize,
        selector_depth: usize,
    ) -> Result<u32, WorldgenError> {
        if patch_depth > 2 {
            return Err(WorldgenError::Invalid(format!(
                "{}: random_patch nesting depth exceeds limit (maximum allowed depth is 2)",
                self.id
            )));
        }
        if selector_depth > 16 {
            return Err(WorldgenError::Invalid(format!(
                "{}: selector nesting depth exceeds limit",
                self.id
            )));
        }

        let modifier_budget = self
            .modifiers
            .iter()
            .try_fold(1u32, |product, modifier| {
                if let PlacementModifier::Count { count } = modifier {
                    match count.range(0, 0) {
                        Some((_, max)) => product.checked_mul(max.max(0) as u32),
                        None => Some(product),
                    }
                } else {
                    Some(product)
                }
            })
            .ok_or_else(|| {
                WorldgenError::Invalid(format!(
                    "{}: count modifiers overflow attempt budget",
                    self.id
                ))
            })?;

        let child_budget = match self.feature.as_ref() {
            ConfiguredFeature::RandomPatch {
                tries,
                feature: child,
                ..
            } => {
                let child_attempts = child.validate_budget(patch_depth + 1, selector_depth)?;
                (*tries as u32).checked_mul(child_attempts).ok_or_else(|| {
                    WorldgenError::Invalid(format!(
                        "{}: random_patch tries overflow attempt budget",
                        self.id
                    ))
                })?
            }
            ConfiguredFeature::SimpleRandomSelector { features } => {
                let mut max_child = 1u32;
                for child in features {
                    max_child =
                        max_child.max(child.validate_budget(patch_depth, selector_depth + 1)?);
                }
                max_child
            }
            ConfiguredFeature::RandomSelector { features, default } => {
                let mut max_child = default.validate_budget(patch_depth, selector_depth + 1)?;
                for (child, _) in features {
                    max_child =
                        max_child.max(child.validate_budget(patch_depth, selector_depth + 1)?);
                }
                max_child
            }
            ConfiguredFeature::WeightedRandomSelector { features } => {
                let mut max_child = 1u32;
                for (child, _) in features {
                    max_child =
                        max_child.max(child.validate_budget(patch_depth, selector_depth + 1)?);
                }
                max_child
            }
            ConfiguredFeature::RandomBooleanSelector {
                feature_true,
                feature_false,
            } => feature_true
                .validate_budget(patch_depth, selector_depth + 1)?
                .max(feature_false.validate_budget(patch_depth, selector_depth + 1)?),
            _ => 1,
        };

        let total = modifier_budget.checked_mul(child_budget).ok_or_else(|| {
            WorldgenError::Invalid(format!(
                "{}: total feature attempts overflow budget",
                self.id
            ))
        })?;

        if total > 4096 {
            return Err(WorldgenError::Invalid(format!(
                "{}: total attempts ({total}) exceed the 4096-attempt placement budget",
                self.id
            )));
        }

        Ok(total)
    }

    pub(super) fn patch_reach(&self) -> i32 {
        let modifier_reach: i32 = self.modifiers.iter().fold(0i32, |acc, modifier| {
            if let PlacementModifier::Offset { x, z, .. } = modifier {
                let max_abs = |r: &RangeProvider| {
                    r.range(0, 0)
                        .map_or(0, |(min, max)| min.abs().max(max.abs()))
                };
                acc.saturating_add(max_abs(x).max(max_abs(z)))
            } else {
                acc
            }
        });
        modifier_reach.saturating_add(self.feature.patch_reach())
    }

    pub(super) fn remaining_costs_no_draws(&self, index: usize) -> bool {
        self.modifiers
            .get(index..)
            .is_some_and(|rest| rest.iter().all(PlacementModifier::costs_no_draws))
            && self.feature.costs_no_draws()
    }

    pub(super) fn remaining_write_bounds(&self, index: usize) -> Option<(i32, i32, i32, i32)> {
        let mut bounds = self.feature.write_bounds()?;
        if let Some(rest) = self.modifiers.get(index..) {
            for modifier in rest {
                bounds = add_horizontal_bounds(bounds, modifier.horizontal_step()?);
            }
        }
        Some(bounds)
    }

    /// Whether every decoration and the configured feature itself can be
    /// modeled here. A false answer means this runtime places nothing for
    /// this feature; the feature still holds its ordinal in its step, so
    /// the numbers the other features are seeded with do not move.
    pub fn is_modelable(&self) -> bool {
        self.feature.is_supported()
            && self.validate_attempt_budget(0).is_ok()
            && self.patch_reach() <= MAX_PATCH_REACH
            && self.modifiers.iter().all(|modifier| match modifier {
                PlacementModifier::Unsupported { .. } => false,
                PlacementModifier::Count { count } => count.is_modelable(),
                PlacementModifier::Offset { x, y, z } => {
                    x.is_modelable() && y.is_modelable() && z.is_modelable()
                }
                PlacementModifier::RandomChance { chance } => (0.0..=1.0).contains(chance),
                PlacementModifier::HeightRange { height } => height.is_modelable(),
                PlacementModifier::BlockPredicateFilter { predicate } => predicate.is_modelable(),
                PlacementModifier::SurfaceRelativeThresholdFilter { heightmap, .. } => {
                    heightmap == "OCEAN_FLOOR_WG"
                }
                PlacementModifier::EnvironmentScan {
                    target_condition,
                    allowed_search_condition,
                    ..
                } => {
                    target_condition.is_modelable()
                        && allowed_search_condition
                            .as_ref()
                            .is_none_or(BlockPredicate::is_modelable)
                }
                _ => true,
            })
    }

    /// Every unsupported piece of this placed feature in evaluation order:
    /// placement modifiers first (in list order), then the configured feature,
    /// its state providers, predicates, or child features, using the gap
    /// report's prefixed naming (`placement:`, `range:`, `predicate:`,
    /// `provider:`, `state:`, `target:`, or the configured feature type).
    ///
    /// Source: minecraft.wiki `Placed feature` (revision 3816343):
    /// "Placed features are applied in order to determine where feature
    /// placement attempt(s) should occur."
    pub fn unmodeled_pieces(&self) -> Vec<String> {
        let mut pieces = Vec::new();
        let mut seen = HashSet::new();
        collect_unmodeled_placed_pieces(self, &mut pieces, &mut seen);
        pieces
    }

    /// The first unsupported piece blocking placement of this feature in
    /// evaluation order, or `None` when [`Self::is_modelable`] holds.
    pub fn first_unmodeled_piece(&self) -> Option<String> {
        self.unmodeled_pieces().into_iter().next()
    }

    /// Diagnostic summary for this placed feature.
    pub fn diagnose(&self) -> PlacedFeatureDiagnosis {
        let unsupported_pieces = self.unmodeled_pieces();
        let first_unsupported = unsupported_pieces.first().cloned();
        PlacedFeatureDiagnosis {
            id: self.id.clone(),
            modelable: self.is_modelable(),
            first_unsupported,
            unsupported_pieces,
        }
    }
}
