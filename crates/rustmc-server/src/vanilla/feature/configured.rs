//! Configured features: the feature families this runtime models and
//! their parameters, plus the horizontal write bounds each one can reach.

use super::*;

/// One ore target: what to test and what to write when it matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OreTarget {
    pub predicate: RuleTest,
    pub state: String,
}

/// How many blocks a patch may scatter from its origin. An origin at local 15
/// reaches local 31 of the next chunk at this distance, the last column the
/// decoration pass still covers by replaying the eight neighbouring anchors. A
/// wider patch would lose placements on grid-aligned chunks, so it stays
/// unmodeled. This is a limit of the replay window, not a game rule.
pub(super) const MAX_PATCH_REACH: i32 = 16;

/// The widest disk this runtime runs. A disk is bounded by the same replay
/// window as a patch: centred on local 15 it reaches local 31 of the next
/// chunk at this radius, and the pack's widest disk is 6.
pub(super) const MAX_DISK_RADIUS: i32 = MAX_PATCH_REACH;

/// The most child attempts one patch may ask for: RustMC's work budget.
pub(super) const MAX_PATCH_TRIES: i64 = 256;

/// The widest vertical patch scatter accepted: RustMC's work budget.
pub(super) const MAX_PATCH_Y_SPREAD: i64 = 32;

/// A configured feature. Only the families with a runtime here carry their
/// parameters; everything else records its type name.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfiguredFeature {
    /// `minecraft:ore` — the vein-shaped replacement used by the stone
    /// families, coal, iron, copper, gold, redstone, lapis and diamond.
    Ore {
        size: i32,
        targets: Vec<OreTarget>,
        discard_chance_on_air_exposure: f32,
    },
    /// `minecraft:random_patch`, and `minecraft:flower`, which shares its
    /// configuration: repeat one child feature around the origin. The 26.3
    /// pack writes no document of either type, so this serves operator packs;
    /// its own vegetation spells the same scatter as `count` plus `offset`.
    RandomPatch {
        tries: i32,
        xz_spread: i32,
        y_spread: i32,
        feature: Box<PlacedFeature>,
    },
    /// `minecraft:simple_block` — one state at one position.
    SimpleBlock {
        to_place: StateProvider,
    },
    /// `minecraft:disk` — a flat circular disk of blocks replacing target blocks.
    Disk {
        state_provider: StateProvider,
        target_predicate: BlockPredicate,
        radius: RangeProvider,
        half_height: i32,
    },
    /// `minecraft:spring_feature` — a single fluid source block placed in an enclosed pocket.
    Spring {
        state: String,
        rock_count: i32,
        hole_count: i32,
        requires_block_below: bool,
        valid_blocks: Vec<String>,
    },
    /// `minecraft:simple_random_selector` — picks one placed feature from
    /// `features` uniformly at random and runs it at the candidate position.
    SimpleRandomSelector {
        features: Vec<Rc<PlacedFeature>>,
    },
    /// `minecraft:random_selector` — checks each `(feature, chance)` entry in
    /// order and runs `default` if none succeed.
    ///
    /// Source: minecraft.wiki `Random Selector` (revision 3818741).
    RandomSelector {
        features: Vec<(Rc<PlacedFeature>, f32)>,
        default: Rc<PlacedFeature>,
    },
    /// `minecraft:weighted_random_selector` — selects one placed feature by weight.
    ///
    /// Source: minecraft.wiki `Java Edition 26.2` (revision 3880953):
    /// "Randomly selects one of the given features to generate based on their weights."
    WeightedRandomSelector {
        features: Vec<(Rc<PlacedFeature>, i32)>,
    },
    /// `minecraft:random_boolean_selector` — chooses `feature_true` or
    /// `feature_false` with equal probability.
    ///
    /// Source: minecraft.wiki `Random Boolean Selector` (revision 3818739).
    RandomBooleanSelector {
        feature_true: Rc<PlacedFeature>,
        feature_false: Rc<PlacedFeature>,
    },
    /// `minecraft:block_column` — builds a contiguous column of layer blocks
    /// along `direction` while `allowed_placement` holds.
    ///
    /// Source: minecraft.wiki `Cave Vines (feature)` (revision 3818484):
    /// "`prioritize_tip`: Determines where to cut off blocks when space is restricted.
    /// If `true`, starts removing layers from the start of the column."
    BlockColumn {
        direction: Direction,
        allowed_placement: BlockPredicate,
        prioritize_tip: bool,
        layers: Vec<BlockColumnLayer>,
    },
    Unsupported {
        kind: String,
    },
}

/// One height + block-state-provider layer of a `minecraft:block_column`.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockColumnLayer {
    pub height: RangeProvider,
    pub provider: StateProvider,
}

impl ConfiguredFeature {
    /// Whether placement of this feature is modeled here. An ore whose
    /// targets contain a test this runtime cannot evaluate is not modeled,
    /// even though the ore shape itself is.
    pub fn is_supported(&self) -> bool {
        match self {
            Self::Ore { targets, .. } => {
                targets.iter().all(|target| target.predicate.is_modelable())
            }
            Self::RandomPatch { feature, .. } => {
                feature.is_modelable() && self.patch_reach() <= MAX_PATCH_REACH
            }
            Self::SimpleBlock { to_place } => to_place.is_modelable() && to_place.is_placeable(),
            Self::Disk {
                state_provider,
                target_predicate,
                radius,
                ..
            } => {
                state_provider.is_modelable()
                    && target_predicate.is_modelable()
                    && radius.is_modelable()
            }
            Self::Spring { .. } => true,
            Self::SimpleRandomSelector { features } => {
                !features.is_empty() && features.iter().all(|feature| feature.is_modelable())
            }
            Self::RandomSelector { features, default } => {
                default.is_modelable()
                    && features.iter().all(|(feature, chance)| {
                        (0.0..=1.0).contains(chance) && feature.is_modelable()
                    })
            }
            Self::WeightedRandomSelector { features } => {
                !features.is_empty()
                    && features
                        .iter()
                        .all(|(feature, weight)| *weight > 0 && feature.is_modelable())
            }
            Self::RandomBooleanSelector {
                feature_true,
                feature_false,
            } => feature_true.is_modelable() && feature_false.is_modelable(),
            Self::BlockColumn {
                allowed_placement,
                layers,
                ..
            } => {
                allowed_placement.is_modelable()
                    && layers.iter().all(|layer| {
                        layer.height.is_modelable()
                            && layer.provider.is_modelable()
                            && layer.provider.is_placeable()
                    })
            }
            Self::Unsupported { .. } => false,
        }
    }

    /// The farthest column a patch and any patch nested in it can write from
    /// the patch's origin.
    pub(super) fn patch_reach(&self) -> i32 {
        match self {
            Self::Disk { radius, .. } => radius.range(0, 0).map_or(0, |(_, max)| max.max(0)),
            Self::RandomPatch {
                xz_spread, feature, ..
            } => xz_spread.saturating_add(feature.patch_reach()),
            Self::SimpleRandomSelector { features } => features
                .iter()
                .map(|feature| feature.patch_reach())
                .max()
                .unwrap_or(0),
            Self::RandomSelector { features, default } => features
                .iter()
                .map(|(feature, _)| feature.patch_reach())
                .max()
                .unwrap_or(0)
                .max(default.patch_reach()),
            Self::WeightedRandomSelector { features } => features
                .iter()
                .map(|(feature, _)| feature.patch_reach())
                .max()
                .unwrap_or(0),
            Self::RandomBooleanSelector {
                feature_true,
                feature_false,
            } => feature_true.patch_reach().max(feature_false.patch_reach()),
            _ => 0,
        }
    }

    /// Whether executing this feature without placement modifiers draws zero RNG values.
    pub(super) fn costs_no_draws(&self) -> bool {
        match self {
            Self::SimpleBlock { to_place } => to_place.costs_no_draws(),
            Self::Spring { .. } | Self::Unsupported { .. } => true,
            Self::BlockColumn { layers, .. } => layers
                .iter()
                .all(|layer| layer.height.costs_no_draws() && layer.provider.costs_no_draws()),
            Self::Disk {
                state_provider,
                radius,
                ..
            } => radius.costs_no_draws() && state_provider.costs_no_draws(),
            Self::Ore { .. }
            | Self::RandomPatch { .. }
            | Self::SimpleRandomSelector { .. }
            | Self::RandomSelector { .. }
            | Self::WeightedRandomSelector { .. }
            | Self::RandomBooleanSelector { .. } => false,
        }
    }

    /// Conservative inclusive horizontal write offset `(min_dx, max_dx, min_dz, max_dz)`
    /// relative to the feature origin, or `None` when unbounded (ores).
    pub(super) fn write_bounds(&self) -> Option<(i32, i32, i32, i32)> {
        match self {
            Self::SimpleBlock { .. } | Self::Spring { .. } | Self::Unsupported { .. } => {
                Some((0, 0, 0, 0))
            }
            Self::Disk { radius, .. } => {
                let (_, max_r) = radius.range(0, 0)?;
                let r = max_r.max(0);
                Some((-r, r, -r, r))
            }
            Self::BlockColumn {
                direction, layers, ..
            } => {
                let (dx, _, dz) = direction.unit_vector();
                if dx == 0 && dz == 0 {
                    Some((0, 0, 0, 0))
                } else {
                    let mut total = 0i32;
                    for layer in layers {
                        let (_, max_h) = layer.height.range(0, 0)?;
                        total = total.saturating_add(max_h.max(0));
                    }
                    let rx = dx.saturating_mul(total);
                    let rz = dz.saturating_mul(total);
                    Some((rx.min(0), rx.max(0), rz.min(0), rz.max(0)))
                }
            }
            Self::RandomPatch {
                xz_spread, feature, ..
            } => {
                let s = (*xz_spread).max(0);
                let child = feature.remaining_write_bounds(0)?;
                Some(add_horizontal_bounds((-s, s, -s, s), child))
            }
            Self::SimpleRandomSelector { features } => {
                union_placed_bounds(features.iter().map(|f| f.as_ref()))
            }
            Self::RandomSelector { features, default } => union_placed_bounds(
                features
                    .iter()
                    .map(|(f, _)| f.as_ref())
                    .chain(std::iter::once(default.as_ref())),
            ),
            Self::WeightedRandomSelector { features } => {
                union_placed_bounds(features.iter().map(|(f, _)| f.as_ref()))
            }
            Self::RandomBooleanSelector {
                feature_true,
                feature_false,
            } => {
                let a = feature_true.remaining_write_bounds(0)?;
                let b = feature_false.remaining_write_bounds(0)?;
                Some(union_horizontal_bounds(a, b))
            }
            Self::Ore { .. } => None,
        }
    }
}

pub(super) fn add_horizontal_bounds(
    a: (i32, i32, i32, i32),
    b: (i32, i32, i32, i32),
) -> (i32, i32, i32, i32) {
    (
        a.0.saturating_add(b.0),
        a.1.saturating_add(b.1),
        a.2.saturating_add(b.2),
        a.3.saturating_add(b.3),
    )
}

pub(super) fn union_horizontal_bounds(
    a: (i32, i32, i32, i32),
    b: (i32, i32, i32, i32),
) -> (i32, i32, i32, i32) {
    (a.0.min(b.0), a.1.max(b.1), a.2.min(b.2), a.3.max(b.3))
}

pub(super) fn union_placed_bounds<'a>(
    features: impl IntoIterator<Item = &'a PlacedFeature>,
) -> Option<(i32, i32, i32, i32)> {
    let mut combined: Option<(i32, i32, i32, i32)> = None;
    for feature in features {
        let bounds = feature.remaining_write_bounds(0)?;
        combined = Some(match combined {
            Some(acc) => union_horizontal_bounds(acc, bounds),
            None => bounds,
        });
    }
    Some(combined.unwrap_or((0, 0, 0, 0)))
}
