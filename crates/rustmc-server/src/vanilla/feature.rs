//! Data-driven feature registry: the placement stage that runs after the
//! surface and carving passes.
//!
//! The registry is read from the operator-provisioned data pack exactly as
//! the carver registry is — one entry per pack element, so its size is
//! bounded by the pack rather than by the world. Identifiers the runtime
//! cannot model are kept as explicit `Unsupported` positions instead of
//! being dropped: the decoration loop numbers each placed feature within
//! its generation step, and the number is part of that feature's random
//! identity, so the numbering must stay stable whether or not the runtime
//! models the feature itself.
//!
//! That number — the ordinal — is not the position in the biome's own list.
//! The reference runtime derives one ordering per generation step from the
//! whole biome source before it decorates anything, and a chunk runs the
//! ordinals its 3x3 biome union contributes to a step, ascending. Both the
//! order and the union are reproduced here from the pack's own data; see
//! `docs/PROVENANCE.md` session 13 for the rules and their derivation.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::OnceLock;

use serde_json::Value;

use crate::vanilla::random::DecorationRandom;
use crate::vanilla::worldgen::{WorldgenError, read_dir_optional, read_json, stem, walk_json};

/// One block-replacement rule test, as the data pack writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleTest {
    /// `minecraft:tag_match` — the block belongs to the named tag.
    Tag { tag: String },
    /// `minecraft:block_match` — exactly one block.
    Block { block: String },
    /// `minecraft:matching_blocks` — one of the listed blocks.
    Blocks { blocks: Vec<String> },
    /// `minecraft:height_match` — the absolute row lies in the range.
    Height {
        min_inclusive: i32,
        max_inclusive: i32,
    },
    /// `minecraft:any_of` — at least one nested test passes.
    AnyOf(Vec<RuleTest>),
    /// `minecraft:all_of` — every nested test passes.
    AllOf(Vec<RuleTest>),
    /// `minecraft:not` — the nested test fails.
    Not(Box<RuleTest>),
    /// A test this runtime does not model. The name is kept so the caller
    /// can refuse the feature instead of guessing at its meaning.
    Unsupported { kind: String },
}

impl RuleTest {
    /// Whether this test and every nested test can be evaluated here.
    pub fn is_modelable(&self) -> bool {
        match self {
            Self::Unsupported { .. } => false,
            Self::AnyOf(rules) | Self::AllOf(rules) => rules.iter().all(Self::is_modelable),
            Self::Not(rule) => rule.is_modelable(),
            _ => true,
        }
    }

    /// Evaluates the test against the block at a candidate position.
    /// `block` is `None` for a position that holds no block at all.
    pub fn test(&self, block: Option<&str>, y: i32, tags: &BlockTags) -> bool {
        match self {
            Self::Tag { tag } => block.is_some_and(|block| tags.contains(tag, block)),
            Self::Block { block: want } => block == Some(want.as_str()),
            Self::Blocks { blocks } => {
                block.is_some_and(|block| blocks.iter().any(|candidate| candidate == block))
            }
            Self::Height {
                min_inclusive,
                max_inclusive,
            } => (*min_inclusive..=*max_inclusive).contains(&y),
            Self::AnyOf(rules) => rules.iter().any(|rule| rule.test(block, y, tags)),
            Self::AllOf(rules) => rules.iter().all(|rule| rule.test(block, y, tags)),
            Self::Not(rule) => !rule.test(block, y, tags),
            // Never reached for a modelable feature; refusing to place is
            // the conservative reading, and placement checks
            // `is_modelable` before it gets here.
            Self::Unsupported { .. } => false,
        }
    }
}

/// The block tags of the provisioned pack, keyed by namespaced id.
#[derive(Debug, Default, Clone)]
pub struct BlockTags {
    entries: HashMap<String, Vec<String>>,
}

impl BlockTags {
    /// Loads `data/<namespace>/tags/block/**/*.json` under the data root.
    /// A missing directory yields an empty registry, which makes every tag
    /// test fail rather than assuming membership.
    fn load(root: &Path) -> Result<Self, WorldgenError> {
        let data_dir = root.join("data");
        let mut raw: HashMap<String, Vec<Value>> = HashMap::new();
        let Some(namespaces) = read_dir_optional(&data_dir)? else {
            return Ok(Self::default());
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
            let ns = namespace.file_name().to_string_lossy().into_owned();
            let category = namespace.path().join("tags").join("block");
            for entry in walk_json(&category)? {
                let document = read_json(&entry)?;
                let values = document
                    .get("values")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                // A tag's id is its path under the category directory, so a
                // pack that files one away (`mineable/pickaxe.json`) keeps
                // the id its references use, and two same-named files in
                // different directories stay separate entries.
                let relative = entry
                    .strip_prefix(&category)
                    .unwrap_or(entry.as_path())
                    .with_extension("");
                let mut id = ns.clone();
                id.push(':');
                id.push_str(&relative.to_string_lossy().replace('\\', "/"));
                raw.entry(id).or_default().extend(values);
            }
        }
        // Entries may name another tag; resolve those references into a
        // flat member list. A reference cycle stops at the first repeat
        // instead of recursing forever.
        let mut tags = Self::default();
        let ids: Vec<String> = raw.keys().cloned().collect();
        for id in ids {
            let mut members: Vec<String> = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            let mut queue: Vec<Value> = raw.get(&id).cloned().unwrap_or_default();
            while let Some(value) = queue.pop() {
                let (name, reference) = match &value {
                    Value::String(name) => (name.as_str(), None),
                    Value::Object(object) => (
                        object.get("id").and_then(Value::as_str).unwrap_or(""),
                        object.get("tag").and_then(Value::as_str),
                    ),
                    _ => continue,
                };
                if !name.is_empty() && !members.iter().any(|member| member == name) {
                    members.push(name.to_string());
                }
                if let Some(tag) = reference.filter(|tag| seen.insert(tag.to_string()))
                    && let Some(values) = raw.get(tag)
                {
                    queue.extend(values.iter().cloned());
                }
            }
            tags.entries.insert(id, members);
        }
        Ok(tags)
    }

    /// Whether `block` is a member of `tag`.
    pub fn contains(&self, tag: &str, block: &str) -> bool {
        self.entries
            .get(tag)
            .is_some_and(|members| members.iter().any(|member| member == block))
    }

    /// Number of tags read from the pack.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pack declared no block tags at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One entry of a `minecraft:weighted` block-state provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightedState {
    pub state: String,
    pub weight: i32,
}

/// The state a configured feature asks to write, as a `to_place` field
/// writes it. The pack's ground-cover features name one block state; the
/// flower and grass variants pick between several by weight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateProvider {
    /// One fixed state: the pack's `{"id": ..., "properties": ...}` object,
    /// or a bare state string.
    Constant { state: String },
    /// `minecraft:weighted` — one uniform draw over the summed weights picks
    /// the entry, so the pick costs exactly one bounded draw.
    Weighted { entries: Vec<WeightedState> },
    /// A provider shape without a runtime here. Named so the gap report can
    /// tell an unmodeled state apart from a feature that placed nothing.
    Unsupported { kind: String },
}

impl StateProvider {
    /// Whether a provider shape has a runtime here.
    pub fn is_modelable(&self) -> bool {
        !matches!(self, Self::Unsupported { .. })
    }

    /// The state to place, drawn from the decoration stream. A constant
    /// costs nothing and a weighted pick costs one bounded draw, matching
    /// the draw-count discipline the count and height providers keep.
    pub fn sample(&self, random: &mut DecorationRandom) -> Option<&str> {
        match self {
            Self::Constant { state } => Some(state.as_str()),
            Self::Weighted { entries } => {
                let total: i32 = entries.iter().map(|entry| entry.weight).sum();
                if total <= 0 {
                    return None;
                }
                let mut pick = random.next_int_bounded(total);
                for entry in entries {
                    if pick < entry.weight {
                        return Some(entry.state.as_str());
                    }
                    pick -= entry.weight;
                }
                None
            }
            // `is_modelable` gates this out before the feature runs.
            Self::Unsupported { .. } => None,
        }
    }
}

/// One block test the `minecraft:block_predicate_filter` decoration asks
/// about. Its own codec names predicates differently from the ore target
/// rules (`minecraft:matching_block_tag` for a tag rather than
/// `minecraft:tag_match`, and a `predicates` list for a conjunction), so it
/// is parsed and evaluated separately instead of sharing `RuleTest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockPredicate {
    /// `minecraft:matching_block_tag` — the block at the position plus
    /// `offset` belongs to the named tag.
    BlockTag {
        tag: String,
        offset: (i32, i32, i32),
    },
    /// `minecraft:matching_blocks` — one of the listed blocks sits there.
    Blocks {
        blocks: Vec<String>,
        offset: (i32, i32, i32),
    },
    /// `minecraft:any_of` — at least one nested test passes.
    AnyOf(Vec<BlockPredicate>),
    /// `minecraft:all_of` — every nested test passes.
    AllOf(Vec<BlockPredicate>),
    /// `minecraft:not` — the nested test fails.
    Not(Box<BlockPredicate>),
    /// A predicate this runtime does not model. The name is kept so the gap
    /// report can say which one gated the chain, and the test never accepts
    /// a position on its behalf.
    Unsupported { kind: String },
}

impl BlockPredicate {
    /// Whether this predicate and every nested one can be evaluated here.
    pub fn is_modelable(&self) -> bool {
        match self {
            Self::Unsupported { .. } => false,
            Self::AnyOf(rules) | Self::AllOf(rules) => rules.iter().all(Self::is_modelable),
            Self::Not(rule) => rule.is_modelable(),
            _ => true,
        }
    }

    /// Evaluates the predicate against the terrain around one candidate
    /// position. A shifted read that leaves the decorated chunk answers
    /// "no block", which is the conservative reading: the neighbour's own
    /// pass paints its half of the patch, exactly as for the ore path.
    pub fn test(
        &self,
        target: &dyn DecorationTarget,
        pos: DecorationPos,
        tags: &BlockTags,
    ) -> bool {
        match self {
            Self::BlockTag { tag, offset } => target
                .block_at(pos.x + offset.0, pos.y + offset.1, pos.z + offset.2)
                .is_some_and(|block| tags.contains(tag, block)),
            Self::Blocks { blocks, offset } => target
                .block_at(pos.x + offset.0, pos.y + offset.1, pos.z + offset.2)
                .is_some_and(|block| blocks.iter().any(|candidate| candidate == block)),
            Self::AnyOf(rules) => rules.iter().any(|rule| rule.test(target, pos, tags)),
            Self::AllOf(rules) => rules.iter().all(|rule| rule.test(target, pos, tags)),
            Self::Not(rule) => !rule.test(target, pos, tags),
            Self::Unsupported { .. } => false,
        }
    }
}

/// A `Heightmap.Types` name the `minecraft:heightmap` decoration asks a
/// column's surface row from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeightmapType {
    /// `MOTION_BLOCKING` — the highest row that stops entity motion.
    MotionBlocking,
    /// `MOTION_BLOCKING_NO_LEAVES` — the same walk with leaves ignored.
    MotionBlockingNoLeaves,
    /// `WORLD_SURFACE_WG` — the highest non-air row of the decoration-time map.
    WorldSurfaceWg,
    /// `OCEAN_FLOOR` — the highest terrain row, the fluid above a sea floor
    /// not counted.
    OceanFloor,
    /// `OCEAN_FLOOR_WG` — the same terrain row from the decoration-time map.
    OceanFloorWg,
    /// A map name without a runtime here, kept for the gap report.
    Unsupported { kind: String },
}

impl HeightmapType {
    /// Names the pack writes. An unrecognised name stays `Unsupported`
    /// rather than being read as one of the five.
    pub fn from_name(name: &str) -> Self {
        match name {
            "MOTION_BLOCKING" => Self::MotionBlocking,
            "MOTION_BLOCKING_NO_LEAVES" => Self::MotionBlockingNoLeaves,
            "WORLD_SURFACE_WG" => Self::WorldSurfaceWg,
            "OCEAN_FLOOR" => Self::OceanFloor,
            "OCEAN_FLOOR_WG" => Self::OceanFloorWg,
            other => Self::Unsupported {
                kind: other.to_string(),
            },
        }
    }

    /// Whether a map name has a runtime here.
    pub fn is_modelable(&self) -> bool {
        !matches!(self, Self::Unsupported { .. })
    }

    /// The column row this map answers with.
    ///
    /// A pre-decoration column holds filler solids, sea fluid and air only:
    /// nothing that stops motion is not solid, and no leaf block exists yet
    /// because no tree feature is modeled here. The three surface-ish maps
    /// therefore answer the same highest-non-air row, and the two ocean-floor
    /// maps answer the terrain row the generator already stores.
    fn row(&self, target: &dyn DecorationTarget, x: i32, z: i32) -> Option<i32> {
        match self {
            Self::MotionBlocking | Self::MotionBlockingNoLeaves | Self::WorldSurfaceWg => {
                target.surface_height(x, z)
            }
            // The stored terrain row of a column that has no surface at all
            // is the dimension floor, which is not a surface: the candidate
            // drops for the same reason the world-surface maps drop it, so no
            // modeled map type can decorate the void.
            Self::OceanFloor | Self::OceanFloorWg => target
                .surface_height(x, z)
                .map(|_| target.ocean_floor_height(x, z)),
            // Never reached: `is_modelable` gates the chain out first.
            Self::Unsupported { .. } => None,
        }
    }
}

/// One ore target: what to test and what to write when it matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OreTarget {
    pub predicate: RuleTest,
    pub state: String,
}

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
    /// `minecraft:simple_block` — one state at one position, the pack's
    /// ground-cover family: grass, flowers, mushrooms, bushes and fire.
    SimpleBlock {
        to_place: StateProvider,
    },
    Unsupported {
        kind: String,
    },
}

impl ConfiguredFeature {
    /// Whether placement of this feature is modeled here. An ore whose
    /// targets contain a test this runtime cannot evaluate is not modeled,
    /// even though the ore shape itself is; the same goes for a simple block
    /// whose state provider has no runtime.
    pub fn is_supported(&self) -> bool {
        match self {
            Self::Ore { targets, .. } => {
                targets.iter().all(|target| target.predicate.is_modelable())
            }
            Self::SimpleBlock { to_place } => to_place.is_modelable(),
            Self::Unsupported { .. } => false,
        }
    }
}

/// A `VerticalAnchor`: a row expressed against one of the world bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAnchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}

impl VerticalAnchor {
    /// Resolves the anchor to an absolute row for a dimension spanning
    /// `min_y` with `height` rows.
    pub fn resolve(&self, min_y: i32, height: i32) -> i32 {
        match self {
            Self::Absolute(y) => *y,
            Self::AboveBottom(offset) => min_y + offset,
            Self::BelowTop(offset) => min_y + height - 1 - offset,
        }
    }

    /// Parses the datapack representation: either a bare number (absolute)
    /// or an object with exactly one of the three keys.
    fn from_value(value: &Value) -> Option<Self> {
        let bounded = |number: i64| {
            i32::try_from(number)
                .ok()
                .filter(|n| (-32_768..=32_768).contains(n))
        };
        if let Some(y) = value.as_i64() {
            return Some(Self::Absolute(bounded(y)?));
        }
        let object = value.as_object()?;
        for key in ["absolute", "above_bottom", "below_top"] {
            if let Some(offset) = object.get(key).and_then(Value::as_i64)
                && object.len() == 1
            {
                return Some(match key {
                    "absolute" => Self::Absolute(bounded(offset)?),
                    "above_bottom" => Self::AboveBottom(bounded(offset)?),
                    _ => Self::BelowTop(bounded(offset)?),
                });
            }
        }
        None
    }
}

/// An integer provider over an anchor range, as used by the count and
/// height decorations. The pack expresses an ore band either as a uniform
/// range or as the symmetric triangle of a `minecraft:trapezoid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeProvider {
    /// `minecraft:constant`, or a bare number.
    Constant(i32),
    /// `minecraft:uniform` over an inclusive anchor range.
    Uniform {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
    },
    /// `minecraft:trapezoid` over an inclusive anchor range. The pack never
    /// writes the plateau or fold parameters for an ore height, so the
    /// distribution is the symmetric triangle those defaults describe.
    Trapezoid {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
    },
    /// A provider shape without a runtime here.
    Unsupported { kind: String },
}

impl RangeProvider {
    /// Parses a provider given either a constant number, a constant object,
    /// a uniform or trapezoid object, or a bare anchor object (height
    /// ranges only).
    fn from_value(value: &Value, anchors_allowed: bool) -> Option<Self> {
        if let Some(n) = value.as_i64() {
            let n = i32::try_from(n).ok()?;
            if anchors_allowed && !(-32_768..=32_768).contains(&n) {
                return None;
            }
            return Some(Self::Constant(n));
        }
        let object = value.as_object()?;
        let anchored = |kind: &str| -> Option<Self> {
            let min_inclusive = VerticalAnchor::from_value(object.get("min_inclusive")?)?;
            let max_inclusive = VerticalAnchor::from_value(object.get("max_inclusive")?)?;
            // A count provider is a plain numeric range; only the height
            // decoration accepts anchors that resolve against the dimension.
            if !anchors_allowed
                && !(matches!(min_inclusive, VerticalAnchor::Absolute(_))
                    && matches!(max_inclusive, VerticalAnchor::Absolute(_)))
            {
                return None;
            }
            Some(if kind == "minecraft:uniform" {
                Self::Uniform {
                    min_inclusive,
                    max_inclusive,
                }
            } else {
                Self::Trapezoid {
                    min_inclusive,
                    max_inclusive,
                }
            })
        };
        match object.get("type").and_then(Value::as_str) {
            Some("minecraft:constant") => {
                let value = object.get("value")?;
                if anchors_allowed && let Some(anchor) = VerticalAnchor::from_value(value) {
                    // A constant anchor is a degenerate range; the height
                    // decoration still resolves it through the provider.
                    return Some(Self::Uniform {
                        min_inclusive: anchor,
                        max_inclusive: anchor,
                    });
                }
                let n = i32::try_from(value.as_i64()?).ok()?;
                if anchors_allowed && !(-32_768..=32_768).contains(&n) {
                    return None;
                }
                Some(Self::Constant(n))
            }
            Some(kind @ ("minecraft:uniform" | "minecraft:trapezoid")) => anchored(kind),
            Some(other) => Some(Self::Unsupported {
                kind: other.to_string(),
            }),
            // No `type` key: an inline anchor pair, which only the height
            // decoration accepts.
            None if anchors_allowed => anchored("minecraft:uniform"),
            None => None,
        }
    }

    /// Whether a provider shape has a runtime here.
    pub fn is_modelable(&self) -> bool {
        !matches!(self, Self::Unsupported { .. })
    }

    /// Parses one axis of a `minecraft:offset` decoration. The pack writes an
    /// offset's range as a plain numeric `min`/`max` (trapezoid) or
    /// `min_inclusive`/`max_inclusive` (uniform) pair rather than the anchors
    /// a height band resolves against the dimension, so both bounds become
    /// absolute and the existing sample paths give the shift the same draw
    /// count it has for a height: none for a constant, one for a uniform, two
    /// for a triangle. A shape with no runtime here — a `plateau` the sampling
    /// does not model, a `minecraft:clamped_normal`, a missing axis — stays
    /// `Unsupported`, which gates the whole chain out and names it.
    fn from_offset_value(value: &Value) -> Self {
        let absolute = |number: Option<i64>| -> Option<VerticalAnchor> {
            let number = number?;
            let number = i32::try_from(number).ok()?;
            (-32_768..=32_768)
                .contains(&number)
                .then_some(VerticalAnchor::Absolute(number))
        };
        let constant = |number: Option<i64>| -> Option<Self> {
            let number = i32::try_from(number?).ok()?;
            (-32_768..=32_768)
                .contains(&number)
                .then_some(Self::Constant(number))
        };
        let unsupported = |kind: &str| Self::Unsupported {
            kind: kind.to_string(),
        };
        if value.as_i64().is_some() {
            return constant(value.as_i64())
                .unwrap_or_else(|| unsupported("minecraft:offset_too_wide"));
        }
        let Some(object) = value.as_object() else {
            return unsupported("minecraft:missing_provider");
        };
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("minecraft:missing_provider");
        if kind == "minecraft:constant" {
            return constant(object.get("value").and_then(Value::as_i64))
                .unwrap_or_else(|| unsupported(kind));
        }
        let (min_key, max_key) = if kind == "minecraft:trapezoid" {
            ("min", "max")
        } else {
            ("min_inclusive", "max_inclusive")
        };
        let (Some(min_inclusive), Some(max_inclusive)) = (
            absolute(object.get(min_key).and_then(Value::as_i64)),
            absolute(object.get(max_key).and_then(Value::as_i64)),
        ) else {
            return unsupported(kind);
        };
        if kind == "minecraft:trapezoid"
            && object.get("plateau").and_then(Value::as_i64).unwrap_or(0) != 0
        {
            // A plateau flattens the triangle; the shared sampler models the
            // symmetric one only, which is what the pack's offsets use.
            return unsupported(kind);
        }
        match kind {
            "minecraft:uniform" => Self::Uniform {
                min_inclusive,
                max_inclusive,
            },
            "minecraft:trapezoid" => Self::Trapezoid {
                min_inclusive,
                max_inclusive,
            },
            _ => unsupported(kind),
        }
    }
}

/// One decoration step of a placed feature.
#[derive(Debug, Clone, PartialEq)]
pub enum PlacementModifier {
    /// `minecraft:count` — how many attempts this feature makes.
    Count { count: RangeProvider },
    /// `minecraft:rarity_filter` — one attempt per `chance` on average.
    RarityFilter { chance: i64 },
    /// `minecraft:in_square` — a horizontal offset inside the chunk.
    InSquare,
    /// `minecraft:height_range` — the vertical band of the attempt.
    HeightRange { height: RangeProvider },
    /// `minecraft:heightmap` — move the candidate to the surface row of the
    /// named map for its column. Costs no draws.
    Heightmap { heightmap: HeightmapType },
    /// `minecraft:offset` — shift the candidate by three int providers, x, y
    /// and z in that order.
    Offset {
        x: RangeProvider,
        y: RangeProvider,
        z: RangeProvider,
    },
    /// `minecraft:block_predicate_filter` — keep the candidate only where the
    /// block around it answers the predicate. Costs no draws.
    BlockPredicateFilter { predicate: BlockPredicate },
    /// `minecraft:biome` — keep the position only if the biome at it lists
    /// this feature.
    Biome,
    /// A decoration without a runtime here. Named, never skipped silently.
    Unsupported { kind: String },
}

/// A placed feature: one configured feature plus its ordered decorations.
#[derive(Debug, Clone)]
pub struct PlacedFeature {
    pub id: String,
    pub feature: Rc<ConfiguredFeature>,
    pub modifiers: Vec<PlacementModifier>,
}

impl PlacedFeature {
    /// Whether every decoration and the configured feature itself can be
    /// modeled here. A false answer means this runtime places nothing for
    /// this feature; the feature still holds its ordinal in its step, so
    /// the numbers the other features are seeded with do not move.
    pub fn is_modelable(&self) -> bool {
        self.feature.is_supported() && self.modifiers.iter().all(modifier_is_modelable)
    }
}

/// Whether one decoration step has a runtime here. Split out of
/// [`PlacedFeature::is_modelable`] so a measurement can ask *which* decoration
/// of a gated chain gated it, and stay unable to disagree with the gate itself.
fn modifier_is_modelable(modifier: &PlacementModifier) -> bool {
    match modifier {
        PlacementModifier::Unsupported { .. } => false,
        PlacementModifier::Count { count } => count.is_modelable(),
        PlacementModifier::HeightRange { height } => height.is_modelable(),
        PlacementModifier::Heightmap { heightmap } => heightmap.is_modelable(),
        PlacementModifier::Offset { x, y, z } => {
            x.is_modelable() && y.is_modelable() && z.is_modelable()
        }
        PlacementModifier::BlockPredicateFilter { predicate } => predicate.is_modelable(),
        _ => true,
    }
}

/// The configured/placed feature registries plus the per-biome ordered
/// feature steps, resolved once from the operator-provisioned data root.
#[derive(Debug, Default)]
pub struct FeatureData {
    configured: HashMap<String, Rc<ConfiguredFeature>>,
    placed: HashMap<String, Rc<PlacedFeature>>,
    /// Biome id → generation step index → placed features, in pack order.
    biomes: HashMap<String, Vec<Vec<Rc<PlacedFeature>>>>,
    /// Biome id → the placed features it lists in any step. The
    /// `minecraft:biome` filter asks exactly this question at a candidate
    /// origin, and the pack's own answer is a flat set over the whole
    /// positional array, not per step.
    membership: HashMap<String, HashSet<String>>,
    /// Generation step index → the placed features that step carries
    /// somewhere in the pack, in the order the decoration loop runs them.
    /// A feature's index in this list is its ordinal, and the ordinal is
    /// part of its random identity, so unmodeled features keep a slot.
    steps: Vec<StepSchedule>,
    /// Biome id → generation step index → the ordinals that biome lists at
    /// that step, deduplicated and ascending. A chunk decorates the union
    /// of these lists over the biomes in its 3x3 neighbourhood.
    biome_ordinals: HashMap<String, Vec<Vec<usize>>>,
    /// The biomes the schedule is built over, in the order the dimension's
    /// biome source declares them. Empty until the generator supplies the
    /// source's list, which is when the schedule falls back to the pack's
    /// own identifier order.
    biome_order: Vec<String>,
    /// The pack's block tags, used by `minecraft:tag_match` targets.
    tags: BlockTags,
    /// What the pack asks for that this generator does not do: the gap
    /// report. Keyed by a configured feature's `minecraft:` type name, by
    /// `target:<rule>` for an unmodeled ore-target predicate, and by
    /// `placement:<modifier>` for an unmodeled decoration; both prefixes
    /// keep their namespaced kind names from reading as a feature type.
    /// Nothing prints it from the server yet — the operator-data smoke is
    /// its only reader until a startup surface exists.
    pub(crate) unimplemented_features: HashMap<String, usize>,
}

/// One generation step's ordered features, plus the lookup that turns a
/// placed feature into its ordinal within the step.
#[derive(Debug, Default)]
pub struct StepSchedule {
    features: Vec<Rc<PlacedFeature>>,
    /// Feature id → its position in `features`. The graph the schedule is
    /// derived from is keyed by (feature, step), so a step can list a given
    /// placed feature at most once and this is a bijection.
    ordinals: HashMap<String, usize>,
}

impl StepSchedule {
    /// The step's features in the order the decoration loop runs them.
    pub fn features(&self) -> &[Rc<PlacedFeature>] {
        &self.features
    }

    /// The ordinal this step gives one placed feature, or `None` when the
    /// step does not carry it.
    fn ordinal_of(&self, id: &str) -> Option<usize> {
        self.ordinals.get(id).copied()
    }
}

/// One node of the ordering graph: a placed feature's global number paired
/// with the generation step it appears in. The reference runtime's node
/// record carries the feature too, but its number identifies the instance,
/// so the pair is the node's identity.
type Node = (usize, usize);

/// The ordered steps a candidate biome list produces.
#[derive(Debug, Default)]
struct BuiltSchedule {
    steps: Vec<StepSchedule>,
    biome_ordinals: HashMap<String, Vec<Vec<usize>>>,
}

/// How many rebuilds the cycle recovery may try before RustMC gives up on
/// the ordering. The reference runtime recurses without a bound, and its
/// search is exponential in the number of biomes; see
/// `docs/PROVENANCE.md` session 13 for the deviation and why it is safe to
/// take. The provisioned pack is shown not to reach the bound by
/// `generator::tests::smoke_operator_feature_schedule_needs_no_cycle_recovery`,
/// which is an operator-data smoke because the bound only matters for a pack
/// whose biome chains contradict each other.
const MAX_CYCLE_ATTEMPTS: usize = 8;

/// The number of generation steps a biome's `features` array must have for
/// the overworld pack shape; the array is positional, so the length is part
/// of the data contract.
pub const GENERATION_STEPS: usize = 11;

impl FeatureData {
    /// Loads `worldgen/feature`, `worldgen/placed_feature`, `worldgen/biome`
    /// and `tags/block` documents under the data root and resolves every
    /// biome reference. An unknown reference is an error, never a skip.
    pub fn load(root: &Path) -> Result<Self, WorldgenError> {
        let tags = BlockTags::load(root)?;
        let mut data = Self {
            tags,
            ..Default::default()
        };
        let data_dir = root.join("data");
        let Some(namespaces) = read_dir_optional(&data_dir)? else {
            return Ok(data);
        };
        let mut configured_docs: Vec<(String, Value)> = Vec::new();
        let mut placed_docs: Vec<(String, Value)> = Vec::new();
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
            for entry in walk_json(&worldgen.join("feature"))? {
                configured_docs.push((format!("{ns}:{}", stem(&entry)), read_json(&entry)?));
            }
            for entry in walk_json(&worldgen.join("placed_feature"))? {
                placed_docs.push((format!("{ns}:{}", stem(&entry)), read_json(&entry)?));
            }
            for entry in walk_json(&worldgen.join("biome"))? {
                biome_docs.push((format!("{ns}:{}", stem(&entry)), read_json(&entry)?));
            }
        }
        // Directory order is the filesystem's, so the documents are sorted
        // by identifier before anything is parsed: the schedule a biome's
        // inline feature reference gets must not depend on that order.
        configured_docs.sort_by(|left, right| left.0.cmp(&right.0));
        placed_docs.sort_by(|left, right| left.0.cmp(&right.0));
        biome_docs.sort_by(|left, right| left.0.cmp(&right.0));
        for (id, document) in configured_docs {
            let feature = parse_configured_feature(&document)?;
            match &feature {
                ConfiguredFeature::Unsupported { kind } => {
                    *data.unimplemented_features.entry(kind.clone()).or_insert(0) += 1;
                }
                ConfiguredFeature::Ore { targets, .. } => {
                    for target in targets {
                        record_unmodeled_rule(&mut data.unimplemented_features, &target.predicate);
                    }
                }
                ConfiguredFeature::SimpleBlock { to_place } => {
                    record_unmodeled_provider(&mut data.unimplemented_features, to_place);
                }
            }
            data.configured.insert(id, Rc::new(feature));
        }
        for (id, document) in placed_docs {
            let feature = parse_placed_feature(&id, &document, &data.configured)?;
            record_unmodeled_modifiers(&mut data.unimplemented_features, &feature);
            data.placed.insert(id, Rc::new(feature));
        }
        let placed_ids: Vec<String> = data.placed.keys().cloned().collect();
        let configured_ids: Vec<String> = data.configured.keys().cloned().collect();
        let mut inline_sites = 0usize;
        for (id, document) in biome_docs {
            let steps = parse_biome_steps(
                &id,
                &document,
                &data.placed,
                &data.configured,
                &placed_ids,
                &configured_ids,
                &mut inline_sites,
            )?;
            let membership: HashSet<String> = steps
                .iter()
                .flat_map(|list| list.iter().map(|feature| feature.id.clone()))
                .collect();
            data.membership.insert(id.clone(), membership);
            data.biomes.insert(id, steps);
        }
        data.build_step_schedule();
        Ok(data)
    }

    /// Orders the generation steps over the biomes the dimension's biome
    /// source can produce, in its declaration order. The reference runtime
    /// builds its feature order once from exactly that list, so both which
    /// biomes contribute and the ordinal each feature gets depend on it.
    /// An empty list leaves the pack's own identifier order in place.
    pub fn set_biome_order(&mut self, order: &[&str]) {
        self.biome_order = order.iter().map(|id| (*id).to_owned()).collect();
        self.build_step_schedule();
    }

    /// The biomes the current schedule was built over, empty when it fell
    /// back to the pack's identifier order.
    pub fn biome_order(&self) -> &[String] {
        &self.biome_order
    }

    fn build_step_schedule(&mut self) {
        let order: Vec<&str> = if self.biome_order.is_empty() {
            // A hash map's iteration order would give the ordinals a
            // different answer on every run, so the fallback is the pack's
            // identifier order.
            let mut names: Vec<&str> = self.biomes.keys().map(String::as_str).collect();
            names.sort_unstable();
            names
        } else {
            self.biome_order.iter().map(String::as_str).collect()
        };
        let mut attempts = MAX_CYCLE_ATTEMPTS;
        let built = match self.schedule_with_retry(&order, &mut attempts) {
            Some((built, dropped)) => {
                if dropped > 0 {
                    *self
                        .unimplemented_features
                        .entry("feature_order_cycle".to_string())
                        .or_insert(0) += dropped;
                }
                built
            }
            None => {
                // A pack whose biome feature chains contradict each other has
                // no order to run, and a cycle the bounded recovery cannot
                // break is reported instead of guessed at.
                *self
                    .unimplemented_features
                    .entry("feature_order_cycle_unresolved".to_string())
                    .or_insert(0) += 1;
                BuiltSchedule::default()
            }
        };
        self.steps = built.steps;
        self.biome_ordinals = built.biome_ordinals;
    }

    /// The reference runtime's cycle recovery: when a biome list cannot be
    /// ordered, take one biome out at a time, in list order, and rebuild
    /// from scratch — the first list that orders wins. The reference has no
    /// budget for those rebuilds; `MAX_CYCLE_ATTEMPTS` bounds this one, and
    /// the count returned is how many biomes it had to drop.
    fn schedule_with_retry(
        &self,
        order: &[&str],
        attempts: &mut usize,
    ) -> Option<(BuiltSchedule, usize)> {
        if let Some(built) = self.attempt_schedule(order) {
            return Some((built, 0));
        }
        for index in 0..order.len() {
            if *attempts == 0 {
                return None;
            }
            *attempts -= 1;
            let mut pruned = order.to_vec();
            pruned.remove(index);
            if let Some((built, dropped)) = self.schedule_with_retry(&pruned, attempts) {
                return Some((built, dropped + 1));
            }
        }
        None
    }

    /// Orders one candidate biome list: number every placed feature by its
    /// first appearance, link each feature to the next one in the same
    /// biome's flattened list, topologically sort those links, and split the
    /// result per generation step. `None` when the links contradict.
    fn attempt_schedule(&self, order: &[&str]) -> Option<BuiltSchedule> {
        let mut numbers: HashMap<&str, usize> = HashMap::new();
        let mut numbered: Vec<Rc<PlacedFeature>> = Vec::new();
        let mut width = 0usize;
        let mut chains: Vec<Vec<Node>> = Vec::with_capacity(order.len());
        for biome in order {
            let Some(biome_steps) = self.biomes.get(*biome) else {
                chains.push(Vec::new());
                continue;
            };
            width = width.max(biome_steps.len());
            let mut chain = Vec::with_capacity(biome_steps.iter().map(Vec::len).sum());
            for (step, list) in biome_steps.iter().enumerate() {
                for feature in list {
                    let number = match numbers.get(feature.id.as_str()) {
                        Some(number) => *number,
                        None => {
                            let number = numbered.len();
                            numbers.insert(feature.id.as_str(), number);
                            numbered.push(Rc::clone(feature));
                            number
                        }
                    };
                    chain.push((number, step));
                }
            }
            chains.push(chain);
        }

        // Each biome's flattened list is a chain: an edge from every element
        // to the one after it, so the topological order keeps each biome's
        // own relative order. Both the node list and each node's successors
        // are walked in (step, number) order, which is what decides the
        // interleaving of two biomes that share no features.
        let mut successors: HashMap<Node, Vec<Node>> = HashMap::new();
        for chain in &chains {
            for (index, node) in chain.iter().enumerate() {
                successors.entry(*node).or_default();
                if let Some(next) = chain.get(index + 1) {
                    let edges = successors.get_mut(node).expect("node just entered");
                    if !edges.contains(next) {
                        edges.push(*next);
                    }
                }
            }
        }
        let mut nodes: Vec<Node> = successors.keys().copied().collect();
        nodes.sort_by_key(|&(number, step)| (step, number));
        for edges in successors.values_mut() {
            edges.sort_by_key(|&(number, step)| (step, number));
            edges.dedup();
        }

        let mut visited: HashSet<Node> = HashSet::new();
        let mut in_progress: HashSet<Node> = HashSet::new();
        let mut emitted: Vec<Node> = Vec::with_capacity(nodes.len());
        for root in &nodes {
            if visited.contains(root) {
                continue;
            }
            if depth_first_visit(
                &successors,
                *root,
                &mut visited,
                &mut in_progress,
                &mut emitted,
            ) {
                return None;
            }
        }
        // A node is emitted only after all of its successors, so reversing
        // the emission turns each edge into a forward one.
        emitted.reverse();

        let mut steps: Vec<StepSchedule> = (0..width).map(|_| StepSchedule::default()).collect();
        for &(number, step) in &emitted {
            if let Some(schedule) = steps.get_mut(step) {
                schedule.features.push(Rc::clone(&numbered[number]));
            }
        }
        for schedule in &mut steps {
            for (index, feature) in schedule.features.iter().enumerate() {
                schedule.ordinals.insert(feature.id.clone(), index);
            }
        }

        let mut biome_ordinals: HashMap<String, Vec<Vec<usize>>> = HashMap::new();
        for biome in order {
            let Some(biome_steps) = self.biomes.get(*biome) else {
                continue;
            };
            let mut per_step = vec![Vec::new(); width];
            for (step, list) in biome_steps.iter().enumerate() {
                let Some(schedule) = steps.get(step) else {
                    continue;
                };
                let mut indices = Vec::with_capacity(list.len());
                for feature in list {
                    if let Some(ordinal) = schedule.ordinal_of(&feature.id) {
                        indices.push(ordinal);
                    }
                }
                indices.sort_unstable();
                indices.dedup();
                per_step[step] = indices;
            }
            biome_ordinals.insert((*biome).to_owned(), per_step);
        }
        Some(BuiltSchedule {
            steps,
            biome_ordinals,
        })
    }

    /// The pack's block tags, for `minecraft:tag_match` target tests.
    pub fn tags(&self) -> &BlockTags {
        &self.tags
    }

    /// The placed features a generation step runs, in ordinal order.
    pub fn step(&self, index: usize) -> Option<&[Rc<PlacedFeature>]> {
        self.steps.get(index).map(StepSchedule::features)
    }

    /// The ordinals one chunk's decoration pass runs for one generation
    /// step, given the biomes its 3x3 neighbourhood contains. The reference
    /// runtime collects the step index of every feature any of those biomes
    /// lists at that step, then sorts them ascending; a biome the dimension
    /// cannot produce contributes nothing, which is its `retainAll` over the
    /// source's possible biomes.
    pub fn region_ordinals(&self, step: usize, biomes: &[String]) -> Vec<usize> {
        let mut ordinals = Vec::new();
        for biome in biomes {
            if let Some(indices) = self
                .biome_ordinals
                .get(biome)
                .and_then(|per_step| per_step.get(step))
            {
                ordinals.extend_from_slice(indices);
            }
        }
        ordinals.sort_unstable();
        ordinals.dedup();
        ordinals
    }

    /// Whether one biome lists this placed feature at all, which is what the
    /// `minecraft:biome` decoration tests at a candidate origin.
    pub fn biome_lists(&self, biome: &str, feature: &str) -> bool {
        self.membership
            .get(biome)
            .is_some_and(|ids| ids.contains(feature))
    }

    /// Ordered placed features for one biome, by generation step index.
    pub fn features_for_biome(&self, biome_id: &str) -> Option<&Vec<Vec<Rc<PlacedFeature>>>> {
        self.biomes.get(biome_id)
    }

    /// The step indices that carry at least one feature somewhere in the
    /// pack, ascending. Vanilla iterates its own fixed step list; here the
    /// list is derived from the data so an empty pack decorates nothing.
    pub fn decorated_steps(&self) -> Vec<usize> {
        (0..self.steps.len())
            .filter(|step| !self.steps[*step].features.is_empty())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.biomes.is_empty() && self.placed.is_empty() && self.configured.is_empty()
    }
}

/// Walks the ordering graph from one root, emitting a node only once every
/// node it points to has been emitted, and reporting whether it met a node
/// that is still on the path. The walk keeps its own stack rather than
/// recursing: a pack's chain can be longer than a call stack wants to hold.
fn depth_first_visit(
    successors: &HashMap<Node, Vec<Node>>,
    start: Node,
    visited: &mut HashSet<Node>,
    in_progress: &mut HashSet<Node>,
    emitted: &mut Vec<Node>,
) -> bool {
    let mut path: Vec<(Node, usize)> = Vec::new();
    let mut pending = Some(start);
    loop {
        if let Some(node) = pending.take() {
            if visited.contains(&node) {
                if path.is_empty() {
                    return false;
                }
                continue;
            }
            if in_progress.contains(&node) {
                return true;
            }
            in_progress.insert(node);
            path.push((node, 0));
        }
        let Some((node, cursor)) = path.last_mut() else {
            return false;
        };
        let edges = successors.get(node).map(Vec::as_slice).unwrap_or(&[]);
        if *cursor < edges.len() {
            let next = edges[*cursor];
            *cursor += 1;
            pending = Some(next);
            continue;
        }
        let (done, _) = path.pop().expect("a frame to finish");
        in_progress.remove(&done);
        visited.insert(done);
        emitted.push(done);
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
    if kind == "minecraft:simple_block" {
        // The pack writes the state provider at the top level, and the older
        // `config` wrapper is accepted here too, as for an ore.
        let body = match document.get("config") {
            Some(config) if config.is_object() => config,
            _ => document,
        };
        return Ok(ConfiguredFeature::SimpleBlock {
            to_place: parse_state_provider(body.get("to_place")),
        });
    }
    if kind != "minecraft:ore" {
        return Ok(ConfiguredFeature::Unsupported { kind });
    }
    // The 26.3 pack writes ore parameters at the top level; the older
    // `config` wrapper is accepted too so a hand-built fixture reads the
    // same as the pack.
    let body = match document.get("config") {
        Some(config) if config.is_object() => config,
        _ => document,
    };
    let size = body
        .get("size")
        .and_then(Value::as_i64)
        .ok_or_else(|| WorldgenError::Invalid("ore feature without a size".to_string()))?;
    // The supported ore placer builds a cubic scratch box and compares all
    // vein points pairwise. Reject operator-pack values that exceed the
    // bounded runtime this implementation can provide.
    if !(1..=128).contains(&size) {
        return Err(WorldgenError::Invalid(format!(
            "ore feature size must be in 1..=128, got {size}"
        )));
    }
    let size = i32::try_from(size).expect("checked ore size");
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
        let state = entry
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(|| WorldgenError::Invalid("ore target without a state".to_string()))?;
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

/// Counts the rule-test kinds a target uses without a runtime here, so the
/// startup report names them the same way it names unmodeled feature types.
fn record_unmodeled_rule(counts: &mut HashMap<String, usize>, rule: &RuleTest) {
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
///
/// The decorations this runtime models carry their own parameter, and a
/// parameter without a runtime gates the chain out just as an unknown modifier
/// type does. Those get the modifier-level key above *and* a key naming the
/// parameter (`heightmap:<name>`, `offset:<name>`, `predicate:<name>`, and
/// `range:<name>` for the two int-provider parameters `minecraft:count` and
/// `minecraft:height_range` take), so the report says which inner shape is
/// missing rather than only which modifier was carrying it.
fn record_unmodeled_modifiers(counts: &mut HashMap<String, usize>, placed: &PlacedFeature) {
    let mut seen: HashSet<String> = HashSet::new();
    let mut record = |key: String| {
        if seen.insert(key.clone()) {
            *counts.entry(key).or_insert(0) += 1;
        }
    };
    for modifier in &placed.modifiers {
        match modifier {
            PlacementModifier::Unsupported { kind } => record(format!("placement:{kind}")),
            PlacementModifier::Count {
                count: RangeProvider::Unsupported { kind },
            } => {
                record("placement:minecraft:count".to_string());
                record(format!("range:{kind}"));
            }
            PlacementModifier::HeightRange {
                height: RangeProvider::Unsupported { kind },
            } => {
                record("placement:minecraft:height_range".to_string());
                record(format!("range:{kind}"));
            }
            PlacementModifier::Heightmap {
                heightmap: HeightmapType::Unsupported { kind },
            } => {
                record("placement:minecraft:heightmap".to_string());
                record(format!("heightmap:{kind}"));
            }
            PlacementModifier::Offset { x, y, z } => {
                for provider in [x, y, z] {
                    if let RangeProvider::Unsupported { kind } = provider {
                        record("placement:minecraft:offset".to_string());
                        record(format!("offset:{kind}"));
                    }
                }
            }
            PlacementModifier::BlockPredicateFilter { predicate } => {
                for kind in unmodeled_predicates(predicate) {
                    record("placement:minecraft:block_predicate_filter".to_string());
                    record(format!("predicate:{kind}"));
                }
            }
            _ => {}
        }
    }
}

/// The unmodeled predicate types one `minecraft:block_predicate_filter` nests,
/// in the order the document writes them.
fn unmodeled_predicates(predicate: &BlockPredicate) -> Vec<&str> {
    let mut kinds = Vec::new();
    match predicate {
        BlockPredicate::Unsupported { kind } => kinds.push(kind.as_str()),
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
fn record_unmodeled_provider(counts: &mut HashMap<String, usize>, provider: &StateProvider) {
    if let StateProvider::Unsupported { kind } = provider {
        *counts.entry(format!("provider:{kind}")).or_insert(0) += 1;
    }
}

fn parse_rule_test(value: Option<&Value>) -> Result<RuleTest, WorldgenError> {
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

/// One block state the pack names directly: a bare id, or the
/// `{"id": ..., "properties": {...}}` object the 26.3 documents write. The
/// properties are rendered in the `id[k=v,...]` form the save palettes use,
/// with the keys in sorted order so the name does not depend on the
/// document's own ordering.
fn parse_direct_state(value: &Value) -> Option<String> {
    if let Some(name) = value.as_str() {
        return Some(name.to_string());
    }
    let object = value.as_object()?;
    state_from_object(object)
}

/// The `{"id": ..., "properties": {...}}` form of a directly named state.
fn state_from_object(object: &serde_json::Map<String, Value>) -> Option<String> {
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
fn parse_state_provider(value: Option<&Value>) -> StateProvider {
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
        // The 26.3 pack puts a *reference into its own
        // `block_state_provider` registry* at this position, not a block
        // name: every bare string it writes here
        // (`minecraft:flower_plain`, `minecraft:flower_meadow`,
        // `minecraft:flower_flower_forest`) is the id of a document under
        // `worldgen/block_state_provider`, and none of them is a block.
        // Reading one as a state would write a block that does not exist, so
        // it stays a named gap. A string inside a `weighted` entry is the
        // other codec field — a block state — and is a constant there.
        Some(Value::String(_)) => unsupported("minecraft:block_state_provider_reference"),
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
fn parse_block_offset(object: &serde_json::Map<String, Value>) -> Option<(i32, i32, i32)> {
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
fn parse_block_predicate(value: Option<&Value>) -> BlockPredicate {
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
    let modifiers = document
        .get("placement")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
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
    Ok(PlacedFeature {
        id: id.to_string(),
        feature,
        modifiers,
    })
}

fn parse_placement_modifier(
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
        "minecraft:in_square" => Ok(PlacementModifier::InSquare),
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
        "minecraft:heightmap" => {
            // An unmapped map name is kept as `HeightmapType::Unsupported`
            // rather than failed here: the pack is read whole, and one
            // dimension's exotic map must not stop the others loading.
            let name = entry
                .get("heightmap")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    WorldgenError::Invalid(format!("{id}: heightmap placement without a map name"))
                })?;
            Ok(PlacementModifier::Heightmap {
                heightmap: HeightmapType::from_name(name),
            })
        }
        "minecraft:offset" => {
            let axis = |key: &str| {
                entry.get(key).map_or_else(
                    || RangeProvider::Unsupported {
                        kind: "minecraft:missing_provider".to_string(),
                    },
                    RangeProvider::from_offset_value,
                )
            };
            Ok(PlacementModifier::Offset {
                x: axis("x"),
                y: axis("y"),
                z: axis("z"),
            })
        }
        "minecraft:block_predicate_filter" => Ok(PlacementModifier::BlockPredicateFilter {
            predicate: parse_block_predicate(entry.get("predicate")),
        }),
        other => Ok(PlacementModifier::Unsupported {
            kind: other.to_string(),
        }),
    }
}

/// Parses a biome's positional `features` array into one list per step.
fn parse_biome_steps(
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

/// The vertical band of one decoration attempt, resolved to absolute rows.
impl RangeProvider {
    /// The provider's inclusive numeric range for a dimension spanning
    /// `min_y` with `height` rows, or `None` for a shape with no runtime
    /// here.
    pub fn range(&self, min_y: i32, height: i32) -> Option<(i32, i32)> {
        match self {
            Self::Constant(value) => Some((*value, *value)),
            Self::Uniform {
                min_inclusive,
                max_inclusive,
            }
            | Self::Trapezoid {
                min_inclusive,
                max_inclusive,
            } => Some((
                min_inclusive.resolve(min_y, height),
                max_inclusive.resolve(min_y, height),
            )),
            Self::Unsupported { .. } => None,
        }
    }

    /// Draws the provider's value from the decoration stream.
    ///
    /// The draw count is part of the contract: a constant costs nothing, a
    /// uniform costs exactly one bounded draw even when its bounds are equal
    /// (`randomBetweenInclusive` always calls `nextInt`), and a trapezoid
    /// costs two unless its bounds are inverted. A provider whose minimum
    /// resolves above its maximum returns the minimum without drawing, which
    /// is what the reference runtime logs and does.
    pub fn sample(&self, min_y: i32, height: i32, random: &mut DecorationRandom) -> i32 {
        let Some((min, max)) = self.range(min_y, height) else {
            return 0;
        };
        if max < min {
            return min;
        }
        match self {
            Self::Constant(value) => *value,
            Self::Trapezoid { .. } => {
                let span = max - min;
                if span <= 0 {
                    return min + random.next_int_bounded(span + 1);
                }
                // `plateau` defaults to zero, so the triangle is the sum of
                // two uniforms over the halves of the span.
                let low = span / 2;
                let high = span - low;
                min + random.next_int_bounded(high + 1) + random.next_int_bounded(low + 1)
            }
            _ => min + random.next_int_bounded(max - min + 1),
        }
    }
}

/// A position the decoration loop is considering, in absolute coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecorationPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// The terrain a decoration pass reads and writes. The generator implements
/// this over one chunk's resolved columns; writing it here keeps the vein
/// geometry independent of how the terrain was produced.
///
/// Every position the pass writes is inside the implementation's own chunk.
/// Reads reach one column beyond it: the six-neighbour air test, the
/// heightmap the ore anchor gate walks over its box's footprint, and the
/// single column a `minecraft:heightmap` decoration resolves — halo columns
/// stay within the 3x3 chunk region the pass already resolves biomes over.
pub trait DecorationTarget {
    /// The dimension's lowest buildable row.
    fn min_y(&self) -> i32;
    /// The number of buildable rows.
    fn height(&self) -> i32;
    /// Whether a position may be written at all. Positions outside the
    /// decorated chunk are refused *before* the replacement rules are
    /// evaluated, so they cost no draws either. A vein that straddles the
    /// border is covered because the pass replays the eight neighbouring
    /// anchors and clips to itself — but only as far as that nine-chunk
    /// window reaches: a predicate offset of up to ±16 or an offset provider
    /// of up to ±32768 can ask for a column outside it, and the write there
    /// is refused and lost. `docs/PROVENANCE.md` records that as a
    /// grid-aligned under-count, not as parity.
    fn writable(&self, x: i32, y: i32, z: i32) -> bool;
    /// The block name at the position. Inside the decorated chunk this is
    /// always a name: a row the fill pass left empty holds air and answers
    /// `minecraft:air`, which is the same answer [`Self::is_air`] gives, while
    /// a carved row answers the carver's own `minecraft:cave_air`. `None`
    /// means the position is outside the chunk the view writes. Earlier
    /// decorations in this pass have already painted here: a later vein may
    /// replace stone a blob turned to granite.
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<&str>;
    /// Whether the position holds air, for the air-exposure test. Reads
    /// beyond the writable zone still have an answer, because the six
    /// neighbours of a border block are queried regardless.
    fn is_air(&self, x: i32, y: i32, z: i32) -> bool;
    /// Writes the block, replacing whatever was there.
    fn set_block(&mut self, x: i32, y: i32, z: i32, name: &str);
    /// The biome at a position, which the `minecraft:biome` filter asks for.
    fn biome_at(&self, x: i32, y: i32, z: i32) -> Option<String>;
    /// The absolute Y of the highest terrain row of the column, as the
    /// world-gen ocean-floor heightmap reports it: fluid and air above a sea
    /// floor do not count. The ore anchor gate reads it for every column of
    /// its box's footprint, including the halo over the neighbour chunks.
    fn ocean_floor_height(&self, x: i32, z: i32) -> i32;
    /// The absolute Y of the highest non-air row of the column, as the
    /// decoration-time world-surface map reports it, or `None` when the
    /// column holds no block at all. The `minecraft:heightmap` decoration
    /// reads it for the one column of each candidate, so a column with no
    /// surface drops the candidate rather than decorating the void.
    fn surface_height(&self, x: i32, z: i32) -> Option<i32>;
}

/// Java `Mth.SIN`: a 65536-entry table indexed by a fixed-point multiple of
/// the argument, so the runtime's sine is a table lookup rather than the
/// library function. Built once per process; 256 KiB, independent of the
/// world.
const SIN_STEPS: usize = 65_536;
const SIN_SCALE: f64 = 10_430.378_350_470_453;

fn mth_sin(value: f64) -> f32 {
    static TABLE: OnceLock<[f32; SIN_STEPS]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0.0f32; SIN_STEPS];
        for (index, value) in table.iter_mut().enumerate() {
            *value = f64::sin(index as f64 / SIN_SCALE) as f32;
        }
        table
    });
    let index = (value * SIN_SCALE) as i64 & (SIN_STEPS as i64 - 1);
    table[index as usize]
}

/// Java `Mth.ceil(float)`: widen, round up, truncate toward zero.
fn ceil_float(value: f32) -> i32 {
    f64::ceil(f64::from(value)) as i32
}

/// Java `Mth.floor(double)`.
fn floor_double(value: f64) -> i32 {
    value.floor() as i32
}

/// Java `Mth.lerp(double, double, double)`.
fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

/// The float pi literal the ore geometry multiplies its angle by; Java's
/// `(float)Math.PI` is not the exact half-turn.
const PI_F32: f32 = core::f32::consts::PI;

/// Whether the discard roll lets a block through without the air test.
/// A zero chance skips the test entirely and a certain one rejects it, both
/// without drawing; only a strict fraction costs one `nextFloat`.
fn should_skip_air_check(random: &mut DecorationRandom, discard: f32) -> bool {
    if discard <= 0.0 {
        return true;
    }
    if discard >= 1.0 {
        return false;
    }
    random.next_float() >= discard
}

/// Whether any of the six cardinal neighbours of a position is air.
fn adjacent_to_air(target: &dyn DecorationTarget, x: i32, y: i32, z: i32) -> bool {
    [
        (x, y - 1, z),
        (x, y + 1, z),
        (x, y, z - 1),
        (x, y, z + 1),
        (x - 1, y, z),
        (x + 1, y, z),
    ]
    .iter()
    .any(|(nx, ny, nz)| target.is_air(*nx, *ny, *nz))
}

/// Whether any column of a box's footprint reaches down to the box's base
/// row. The reference runtime's ore placement asks its world-gen ocean-floor
/// heightmap this question over the `width + 1` by `width + 1` grid of columns
/// the box covers, and gives up on the attempt when none of them answers: a
/// box that floats entirely above the terrain can hold nothing a stone-family
/// replacement test accepts, so the attempt is over before it draws a radius.
/// What it *does* decide is where the attempt leaves the stream: the rejected
/// one costs only the three draws that fixed its axis, and every later attempt
/// of the same feature resumes from there.
fn anchored(
    target: &dyn DecorationTarget,
    base_x: i32,
    base_y: i32,
    base_z: i32,
    width: i32,
) -> bool {
    (base_x..=base_x + width)
        .flat_map(|x| (base_z..=base_z + width).map(move |z| (x, z)))
        .any(|(x, z)| base_y <= target.ocean_floor_height(x, z))
}

/// Places one `minecraft:ore` vein, returning the number of blocks written.
///
/// The shape is the documented two-endpoint ellipsoid sweep: three draws fix
/// the vein's axis and height jitter, the box those endpoints define is
/// anchored [`anchored`] against the heightmap, `size` points along the axis
/// each get a radius from a single `nextDouble`, points whose radius difference
/// exceeds their distance are culled, and the surviving spheres are written
/// where the column's replacement rules accept them.
fn place_ore(
    target: &mut dyn DecorationTarget,
    random: &mut DecorationRandom,
    tags: &BlockTags,
    size: i32,
    targets: &[OreTarget],
    discard: f32,
    origin: DecorationPos,
) -> usize {
    let angle = random.next_float() * PI_F32;
    let scale = size as f32 / 8.0;
    let span = ceil_float((size as f32 / 16.0 * 2.0 + 1.0) / 2.0);
    let extent = f64::from(scale);
    let (sin, cos) = (f64::sin(f64::from(angle)), f64::cos(f64::from(angle)));
    let start_x = origin.x as f64 + sin * extent;
    let end_x = origin.x as f64 - sin * extent;
    let start_z = origin.z as f64 + cos * extent;
    let end_z = origin.z as f64 - cos * extent;
    let start_y = f64::from(origin.y + random.next_int_bounded(3) - 2);
    let end_y = f64::from(origin.y + random.next_int_bounded(3) - 2);
    let reach = ceil_float(scale);
    let base_x = origin.x - reach - span;
    let base_y = origin.y - 2 - span;
    let base_z = origin.z - reach - span;
    let width = 2 * (reach + span);
    let box_height = 2 * (span + 2);
    if !anchored(target, base_x, base_y, base_z, width) {
        return 0;
    }
    let points = size.max(0) as usize;
    let mut vein = vec![0.0f64; points * 4];
    for index in 0..points {
        let step = index as f32 / size as f32;
        let jitter = random.next_double() * size as f64 / 16.0;
        let radius = (f64::from(mth_sin(f64::from(PI_F32 * step)) + 1.0f32) * jitter + 1.0) / 2.0;
        let offset = index * 4;
        vein[offset] = lerp(f64::from(step), start_x, end_x);
        vein[offset + 1] = lerp(f64::from(step), start_y, end_y);
        vein[offset + 2] = lerp(f64::from(step), start_z, end_z);
        vein[offset + 3] = radius;
    }
    // A point the length of whose radius difference exceeds its distance to
    // another point is inside it, and drops out of the sweep.
    for left in 0..points.saturating_sub(1) {
        if vein[left * 4 + 3] <= 0.0 {
            continue;
        }
        for right in left + 1..points {
            if vein[right * 4 + 3] <= 0.0 {
                continue;
            }
            let radius_delta = vein[left * 4 + 3] - vein[right * 4 + 3];
            let distance = (vein[left * 4] - vein[right * 4]).powi(2)
                + (vein[left * 4 + 1] - vein[right * 4 + 1]).powi(2)
                + (vein[left * 4 + 2] - vein[right * 4 + 2]).powi(2);
            if radius_delta * radius_delta > distance {
                if radius_delta > 0.0 {
                    vein[right * 4 + 3] = -1.0;
                } else {
                    vein[left * 4 + 3] = -1.0;
                }
            }
        }
    }
    let mut written = 0usize;
    let cells = width.max(0) as usize * box_height.max(0) as usize * width.max(0) as usize;
    let mut visited = vec![false; cells];
    for point in 0..points {
        let radius = vein[point * 4 + 3];
        if radius < 0.0 {
            continue;
        }
        let center_x = vein[point * 4];
        let center_y = vein[point * 4 + 1];
        let center_z = vein[point * 4 + 2];
        let min_x = floor_double(center_x - radius).max(base_x);
        let min_y = floor_double(center_y - radius).max(base_y);
        let min_z = floor_double(center_z - radius).max(base_z);
        let max_x = floor_double(center_x + radius).max(min_x);
        let max_y = floor_double(center_y + radius).max(min_y);
        let max_z = floor_double(center_z + radius).max(min_z);
        for x in min_x..=max_x {
            let dx = (f64::from(x) + 0.5 - center_x) / radius;
            if dx * dx >= 1.0 {
                continue;
            }
            for y in min_y..=max_y {
                let dy = (f64::from(y) + 0.5 - center_y) / radius;
                if dx * dx + dy * dy >= 1.0 {
                    continue;
                }
                for z in min_z..=max_z {
                    let dz = (f64::from(z) + 0.5 - center_z) / radius;
                    if dx * dx + dy * dy + dz * dz >= 1.0 {
                        continue;
                    }
                    if y < target.min_y() || y > target.min_y() + target.height() - 1 {
                        continue;
                    }
                    let slot = (x - base_x) as usize
                        + (y - base_y) as usize * width as usize
                        + (z - base_z) as usize * width as usize * box_height as usize;
                    if visited.get(slot).copied().unwrap_or(true) {
                        continue;
                    }
                    visited[slot] = true;
                    if !target.writable(x, y, z) {
                        continue;
                    }
                    let current = target.block_at(x, y, z);
                    for replacement in targets {
                        if !replacement.predicate.test(current, y, tags) {
                            continue;
                        }
                        if !should_skip_air_check(random, discard)
                            && adjacent_to_air(target, x, y, z)
                        {
                            continue;
                        }
                        target.set_block(x, y, z, &replacement.state);
                        written += 1;
                        break;
                    }
                }
            }
        }
    }
    written
}

/// Places one `minecraft:simple_block` state, returning the number of blocks
/// written.
///
/// The position has to be inside the decorated chunk before anything is drawn,
/// the same confinement and the same stream discipline the vein path keeps:
/// the neighbour's own pass paints its half of a patch that straddles the
/// border. The state then comes from the provider — a weighted pick costs one
/// bounded draw — and the block written only replaces air, which is what the
/// pack's own `minecraft:air` tag says the chains gated on it expect.
fn place_simple_block(
    target: &mut dyn DecorationTarget,
    random: &mut DecorationRandom,
    to_place: &StateProvider,
    pos: DecorationPos,
) -> usize {
    if !target.writable(pos.x, pos.y, pos.z) {
        return 0;
    }
    let Some(state) = to_place.sample(random) else {
        return 0;
    };
    if !target.is_air(pos.x, pos.y, pos.z) {
        return 0;
    }
    target.set_block(pos.x, pos.y, pos.z, state);
    1
}

impl FeatureData {
    /// Runs one generation step's modeled features for one chunk, drawing
    /// from the stream the step's ordinal gives that chunk, and returns the
    /// number of blocks written.
    ///
    /// Only the ordinals the chunk's 3x3 biome union lists at this step run:
    /// the reference runtime collects exactly that set before it places
    /// anything, so a feature whose only biomes lie outside the neighbourhood
    /// is never attempted here, whatever its own biome filter would say.
    ///
    /// Each feature restarts its stream from the chunk's decoration seed plus
    /// its ordinal and step, so a feature with no runtime here contributes
    /// nothing without moving the seeds of the features around it — which is
    /// also why the ordinals are kept for unmodeled features.
    pub fn decorate_step(
        &self,
        target: &mut dyn DecorationTarget,
        step: usize,
        chunk_x: i32,
        chunk_z: i32,
        world_seed: i64,
        region_biomes: &[String],
    ) -> usize {
        let mut written = 0usize;
        for ordinal in self.region_ordinals(step, region_biomes) {
            written += self.decorate_ordinal(target, step, ordinal, chunk_x, chunk_z, world_seed);
        }
        written
    }

    /// Runs the one feature a step's schedule holds at `ordinal` for one
    /// chunk, exactly as [`Self::decorate_step`] would run it, and returns the
    /// number of blocks written.
    ///
    /// Splitting this out is only sound because a feature's stream restarts
    /// from the chunk's decoration seed with its own ordinal and step: no
    /// ordinal can borrow state from the one before it, so running one alone
    /// draws the same sequence as running it inside the step's loop. That is
    /// what lets a measurement attribute a written block to the feature that
    /// placed it.
    pub fn decorate_ordinal(
        &self,
        target: &mut dyn DecorationTarget,
        step: usize,
        ordinal: usize,
        chunk_x: i32,
        chunk_z: i32,
        world_seed: i64,
    ) -> usize {
        let Some(placed) = self.step(step).and_then(|s| s.get(ordinal)) else {
            return 0;
        };
        if !placed.is_modelable() {
            return 0;
        }
        let origin = DecorationPos {
            x: chunk_x << 4,
            y: target.min_y(),
            z: chunk_z << 4,
        };
        let mut random = DecorationRandom::new(world_seed);
        let decoration_seed = random.set_decoration_seed(world_seed, origin.x, origin.z);
        random.set_feature_seed(decoration_seed, ordinal as i32, step as i32);
        self.run_chain(target, placed, 0, origin, &mut random)
    }

    /// Walks one placed feature's decoration list. Each modifier either
    /// filters the candidate out, rewrites it, or multiplies it; the walk is
    /// the documented order, and the draws happen in that order too.
    fn run_chain(
        &self,
        target: &mut dyn DecorationTarget,
        placed: &PlacedFeature,
        index: usize,
        pos: DecorationPos,
        random: &mut DecorationRandom,
    ) -> usize {
        let Some(modifier) = placed.modifiers.get(index) else {
            let feature = &*placed.feature;
            return match feature {
                ConfiguredFeature::Ore {
                    size,
                    targets,
                    discard_chance_on_air_exposure,
                } => place_ore(
                    target,
                    random,
                    &self.tags,
                    *size,
                    targets,
                    *discard_chance_on_air_exposure,
                    pos,
                ),
                ConfiguredFeature::SimpleBlock { to_place } => {
                    place_simple_block(target, random, to_place, pos)
                }
                ConfiguredFeature::Unsupported { .. } => 0,
            };
        };
        match modifier {
            PlacementModifier::Count { count } => {
                let attempts = count.sample(target.min_y(), target.height(), random);
                let mut written = 0;
                for _ in 0..attempts.max(0) {
                    written += self.run_chain(target, placed, index + 1, pos, random);
                }
                written
            }
            PlacementModifier::RarityFilter { chance } => {
                if random.next_float() < 1.0f32 / (*chance as f32) {
                    self.run_chain(target, placed, index + 1, pos, random)
                } else {
                    0
                }
            }
            PlacementModifier::InSquare => {
                let mut next = pos;
                next.x += random.next_int_bounded(16);
                next.z += random.next_int_bounded(16);
                self.run_chain(target, placed, index + 1, next, random)
            }
            PlacementModifier::HeightRange { height: provider } => {
                let mut next = pos;
                next.y = provider.sample(target.min_y(), target.height(), random);
                self.run_chain(target, placed, index + 1, next, random)
            }
            PlacementModifier::Heightmap { heightmap } => {
                // Resolving the map costs no draws. A column whose surface
                // does not exist answers nothing, and the candidate is dropped
                // rather than decorated into the void.
                let Some(y) = heightmap.row(target, pos.x, pos.z) else {
                    return 0;
                };
                let mut next = pos;
                next.y = y;
                self.run_chain(target, placed, index + 1, next, random)
            }
            PlacementModifier::Offset { x, y, z } => {
                // The three axes are sampled in the order the document names
                // them, and all three before the result is tested, so a shift
                // that leaves the build volume costs its draws and drops the
                // candidate instead of wrapping inside it.
                let (min_y, height) = (target.min_y(), target.height());
                let mut next = pos;
                next.x += x.sample(min_y, height, random);
                next.y += y.sample(min_y, height, random);
                next.z += z.sample(min_y, height, random);
                if next.y < min_y || next.y > min_y + height - 1 {
                    return 0;
                }
                self.run_chain(target, placed, index + 1, next, random)
            }
            PlacementModifier::BlockPredicateFilter { predicate } => {
                // The predicate reads the terrain around the candidate and
                // costs no draws; an unmodeled shape inside it is gated out by
                // `is_modelable` long before this point.
                if predicate.test(target, pos, &self.tags) {
                    self.run_chain(target, placed, index + 1, pos, random)
                } else {
                    0
                }
            }
            PlacementModifier::Biome => {
                // The filter asks the biome at the candidate position, not at
                // the chunk origin, and it costs no draws.
                let allowed = target
                    .biome_at(pos.x, pos.y, pos.z)
                    .is_some_and(|biome| self.biome_lists(&biome, &placed.id));
                if allowed {
                    self.run_chain(target, placed, index + 1, pos, random)
                } else {
                    0
                }
            }
            // `is_modelable` gates this out before the step runs.
            PlacementModifier::Unsupported { .. } => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn scratch_root(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rustmc-feature-{label}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create scratch dir");
        path
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, text).expect("write");
    }

    #[test]
    fn parses_the_ore_shape_and_its_targets() {
        let document = json!({
            "type": "minecraft:ore",
            "size": 64,
            "discard_chance_on_air_exposure": 0.0,
            "targets": [{
                "state": "minecraft:andesite",
                "target": {"predicate_type": "minecraft:tag_match", "tag": "minecraft:base_stone_overworld"}
            }]
        });
        let feature = parse_configured_feature(&document).expect("parses");
        let ConfiguredFeature::Ore {
            size,
            targets,
            discard_chance_on_air_exposure,
        } = feature
        else {
            panic!("expected an ore feature: {feature:?}");
        };
        assert_eq!(size, 64);
        assert_eq!(discard_chance_on_air_exposure, 0.0);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].state, "minecraft:andesite");
        assert_eq!(
            targets[0].predicate,
            RuleTest::Tag {
                tag: "minecraft:base_stone_overworld".to_string()
            }
        );
    }

    #[test]
    fn unknown_feature_types_are_recorded_not_dropped() {
        let feature = parse_configured_feature(&json!({"type": "minecraft:tree"})).expect("parses");
        assert_eq!(
            feature,
            ConfiguredFeature::Unsupported {
                kind: "minecraft:tree".to_string()
            }
        );
        assert!(!feature.is_supported());
    }

    #[test]
    fn an_out_of_budget_ore_size_is_rejected() {
        for size in [0, -8, 129, i64::MAX] {
            let error = parse_configured_feature(&json!({
                "type": "minecraft:ore",
                "size": size,
                "targets": [{
                    "state": "minecraft:tuff",
                    "target": {"predicate_type": "minecraft:matching_blocks", "blocks": ["minecraft:stone"]}
                }]
            }))
            .expect_err("rejected");
            assert!(
                error.to_string().contains("size must be in 1..=128"),
                "{error}"
            );
        }
    }

    #[test]
    fn placement_rejects_overflowing_and_unbounded_inputs() {
        let configured = HashMap::from([(
            "minecraft:test".to_string(),
            Rc::new(ConfiguredFeature::Unsupported {
                kind: "minecraft:tree".to_string(),
            }),
        )]);
        for placement in [
            json!([{"type": "minecraft:count", "count": i64::MAX}]),
            json!([{"type": "minecraft:count", "count": 257}]),
            json!([{"type": "minecraft:count", "count": {
                "type": "minecraft:uniform", "min_inclusive": {"absolute": 0},
                "max_inclusive": {"absolute": 257}
            }}]),
            json!([{"type": "minecraft:height_range", "height": {
                "type": "minecraft:uniform", "min_inclusive": {"absolute": i64::MAX},
                "max_inclusive": {"absolute": 0}
            }}]),
            json!([{"type": "minecraft:count", "count": 256},
                   {"type": "minecraft:count", "count": 256}]),
        ] {
            let document = json!({"feature": "minecraft:test", "placement": placement});
            assert!(
                parse_placed_feature("minecraft:test_placed", &document, &configured).is_err(),
                "oversized input must fail at load time: {document}"
            );
        }
    }

    #[test]
    fn reads_the_count_rarity_square_height_and_biome_steps() {
        let configured = HashMap::from([(
            "minecraft:ore_granite".to_string(),
            Rc::new(ConfiguredFeature::Ore {
                size: 64,
                targets: vec![OreTarget {
                    predicate: RuleTest::Tag {
                        tag: "minecraft:base_stone_overworld".to_string(),
                    },
                    state: "minecraft:granite".to_string(),
                }],
                discard_chance_on_air_exposure: 0.0,
            }),
        )]);
        let document = json!({
            "feature": "minecraft:ore_granite",
            "placement": [
                {"type": "minecraft:rarity_filter", "chance": 6},
                {"type": "minecraft:in_square"},
                {"type": "minecraft:height_range", "height": {
                    "type": "minecraft:uniform",
                    "max_inclusive": {"absolute": 128},
                    "min_inclusive": {"absolute": 64}
                }},
                {"type": "minecraft:biome"}
            ]
        });
        let placed = parse_placed_feature("minecraft:ore_granite_upper", &document, &configured)
            .expect("parses");
        assert!(placed.is_modelable());
        assert_eq!(
            placed.modifiers,
            vec![
                PlacementModifier::RarityFilter { chance: 6 },
                PlacementModifier::InSquare,
                PlacementModifier::HeightRange {
                    height: RangeProvider::Uniform {
                        min_inclusive: VerticalAnchor::Absolute(64),
                        max_inclusive: VerticalAnchor::Absolute(128),
                    }
                },
                PlacementModifier::Biome,
            ]
        );
    }

    #[test]
    fn an_above_bottom_anchor_resolves_against_the_dimension_bounds() {
        let document = json!({
            "feature": "minecraft:ore_tuff",
            "placement": [
                {"type": "minecraft:count", "count": 2},
                {"type": "minecraft:in_square"},
                {"type": "minecraft:height_range", "height": {
                    "type": "minecraft:uniform",
                    "max_inclusive": {"absolute": 0},
                    "min_inclusive": {"above_bottom": 0}
                }},
                {"type": "minecraft:biome"}
            ]
        });
        let tuff = ConfiguredFeature::Ore {
            size: 64,
            targets: vec![OreTarget {
                predicate: RuleTest::Blocks {
                    blocks: vec!["minecraft:deepslate".to_string()],
                },
                state: "minecraft:tuff".to_string(),
            }],
            discard_chance_on_air_exposure: 0.0,
        };
        let configured = HashMap::from([("minecraft:ore_tuff".to_string(), Rc::new(tuff.clone()))]);
        let placed =
            parse_placed_feature("minecraft:ore_tuff", &document, &configured).expect("parses");
        let PlacementModifier::HeightRange {
            height:
                RangeProvider::Uniform {
                    min_inclusive,
                    max_inclusive,
                },
        } = &placed.modifiers[2]
        else {
            panic!("expected a uniform height range: {:?}", placed.modifiers[2]);
        };
        // The overworld spans -64..=319.
        assert_eq!(min_inclusive.resolve(-64, 384), -64);
        assert_eq!(max_inclusive.resolve(-64, 384), 0);
        assert_eq!(VerticalAnchor::BelowTop(0).resolve(-64, 384), 319);
    }

    #[test]
    fn an_unmodeled_decoration_makes_the_feature_unmodelable() {
        let ore = ConfiguredFeature::Ore {
            size: 10,
            targets: vec![OreTarget {
                predicate: RuleTest::Tag {
                    tag: "minecraft:base_stone_overworld".to_string(),
                },
                state: "minecraft:coal_ore".to_string(),
            }],
            discard_chance_on_air_exposure: 0.0,
        };
        let configured =
            HashMap::from([("minecraft:ore_coal_upper".to_string(), Rc::new(ore.clone()))]);
        let document = json!({
            "feature": "minecraft:ore_coal_upper",
            "placement": [
                {"type": "minecraft:count", "count": {"type": "minecraft:clumped", "inner": 5}},
                {"type": "minecraft:random_offset", "xz_spread": 7, "y_spread": 3},
                {"type": "minecraft:biome"}
            ]
        });
        let placed = parse_placed_feature("test", &document, &configured).expect("parses");
        assert!(
            !placed.is_modelable(),
            "an unmodeled provider or decoration must not be silently run"
        );
    }

    /// A pack of one modeled ore feature, one placed feature whose
    /// `placement` array the caller supplies verbatim, and one biome listing
    /// it — so the only thing the report can be talking about is the
    /// decorations.
    fn report_pack(label: &str, placement: &str) -> PathBuf {
        let root = scratch_root(label);
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("feature/andesite.json"),
            r#"{"type": "minecraft:ore", "size": 8, "discard_chance_on_air_exposure": 0.0,
                "targets": [{"state": "minecraft:andesite",
                    "target": {"predicate_type": "minecraft:matching_blocks",
                        "blocks": ["minecraft:stone"]}}]}"#,
        );
        write(
            &worldgen.join("placed_feature/one.json"),
            &format!(r#"{{"feature": "testns:andesite", "placement": {placement}}}"#),
        );
        write(
            &worldgen.join("biome/plains.json"),
            r#"{"features": [["testns:one"]]}"#,
        );
        root
    }

    #[test]
    fn an_unmodeled_decoration_is_named_in_the_report() {
        // The configured type here is `minecraft:ore`, which this runtime
        // models, so before this counter the feature places nothing and the
        // report says nothing about why.
        let root = report_pack(
            "report-modifier",
            r#"[{"type": "minecraft:count", "count": 2},
                {"type": "minecraft:random_offset", "xz_spread": 7, "y_spread": 3},
                {"type": "minecraft:biome"}]"#,
        );
        let data = FeatureData::load(&root).expect("loads");
        assert_eq!(
            data.unimplemented_features
                .get("placement:minecraft:random_offset"),
            Some(&1),
            "the decoration that gates the chain is named"
        );
        assert_eq!(
            data.unimplemented_features.len(),
            1,
            "every other decoration in the chain is modeled: {:?}",
            data.unimplemented_features
        );
        assert!(
            !data.step(0).expect("step 0")[0].is_modelable(),
            "and the feature is still gated out of the run"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// A modeled decoration whose *parameter* has no runtime gates the chain
    /// out exactly as an unknown decoration type does, and both halves have to
    /// be named: the decoration that carried it and the provider shape that
    /// broke it. `minecraft:count` and `minecraft:height_range` take an int
    /// provider, and were the two the report stayed silent about — the pack
    /// writes `minecraft:weighted_list` for the count of its bamboo and birch
    /// ordinals, and those chains vanished from the gap report entirely.
    #[test]
    fn an_unmodeled_modifier_parameter_names_the_modifier_and_its_provider() {
        let root = report_pack(
            "report-range-params",
            r#"[{"type": "minecraft:count", "count": {"type": "minecraft:weighted_list",
                    "distribution": [{"data": 3, "weight": 1}, {"data": 5, "weight": 2}]}},
                {"type": "minecraft:height_range", "height": {"type": "minecraft:clamped_normal",
                    "mean": 30, "deviation": 4,
                    "min_inclusive": {"absolute": 5}, "max_inclusive": {"absolute": 60}}},
                {"type": "minecraft:biome"}]"#,
        );
        let data = FeatureData::load(&root).expect("loads");
        for key in [
            "placement:minecraft:count",
            "range:minecraft:weighted_list",
            "placement:minecraft:height_range",
            "range:minecraft:clamped_normal",
        ] {
            assert_eq!(
                data.unimplemented_features.get(key),
                Some(&1),
                "{key} is a named gap: {:?}",
                data.unimplemented_features
            );
        }
        assert!(
            !data.step(0).expect("step 0")[0].is_modelable(),
            "and both chains stay gated out of the run"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// The offset a position predicate may carry: three integers inside ±16,
    /// which is as far as the decoration pass resolves a neighbouring column.
    /// Anything wider or malformed is refused rather than read, because a
    /// predicate reaching past the nine-chunk window would answer from terrain
    /// the pass never resolved.
    #[test]
    fn a_predicate_offset_outside_the_resolved_window_is_refused() {
        let offset = |text: &str| {
            let value: Value = serde_json::from_str(text).expect("predicate object");
            parse_block_offset(value.as_object().expect("object"))
        };
        assert_eq!(
            offset("{}"),
            Some((0, 0, 0)),
            "a document writing no offset means the position itself"
        );
        assert_eq!(
            offset(r#"{"offset": [-16, 0, 16]}"#),
            Some((-16, 0, 16)),
            "the bound is inclusive"
        );
        for text in [
            r#"{"offset": [17, 0, 0]}"#,
            r#"{"offset": [0, -17, 0]}"#,
            r#"{"offset": [0, 0, 32768]}"#,
            r#"{"offset": [0, 0]}"#,
            r#"{"offset": [0, 0, 0, 0]}"#,
            r#"{"offset": "up"}"#,
            r#"{"offset": [0, 1.5, 0]}"#,
            r#"{"offset": [[0], 0, 0]}"#,
        ] {
            assert_eq!(
                offset(text),
                None,
                "{text} is not an offset this runtime reads"
            );
        }
    }

    #[test]
    fn a_fully_modeled_chain_adds_nothing_to_the_report() {
        let root = report_pack(
            "report-clean",
            r#"[{"type": "minecraft:count", "count": 2},
                {"type": "minecraft:rarity_filter", "chance": 4},
                {"type": "minecraft:in_square"},
                {"type": "minecraft:height_range", "height": {"type": "minecraft:uniform",
                    "min_inclusive": {"absolute": 10}, "max_inclusive": {"absolute": 60}}},
                {"type": "minecraft:biome"}]"#,
        );
        let data = FeatureData::load(&root).expect("loads");
        assert!(
            data.unimplemented_features.is_empty(),
            "a chain this runtime runs is not a gap: {:?}",
            data.unimplemented_features
        );
        assert!(data.step(0).expect("step 0")[0].is_modelable());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_repeated_decoration_counts_once_per_document() {
        let root = report_pack(
            "report-dedup",
            r#"[{"type": "minecraft:random_offset", "xz_spread": 7, "y_spread": 3},
                {"type": "minecraft:random_offset", "xz_spread": 2, "y_spread": 1},
                {"type": "minecraft:noise_threshold_count", "noise_level": -0.8,
                    "below_noise": 1, "above_noise": 4},
                {"type": "minecraft:biome"}]"#,
        );
        let data = FeatureData::load(&root).expect("loads");
        // One document, one kind, two occurrences: the counter counts the
        // feature the report is about, not the modifier's repetition.
        assert_eq!(
            data.unimplemented_features
                .get("placement:minecraft:random_offset"),
            Some(&1)
        );
        assert_eq!(
            data.unimplemented_features
                .get("placement:minecraft:noise_threshold_count"),
            Some(&1),
            "a second unmodeled kind in the same document gets its own key"
        );
        assert_eq!(data.unimplemented_features.len(), 2);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn the_configured_and_target_keys_keep_their_meaning() {
        let root = scratch_root("report-kinds");
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("feature/swamp.json"),
            r#"{"type": "minecraft:tree"}"#,
        );
        write(
            &worldgen.join("feature/diorite.json"),
            r#"{"type": "minecraft:ore", "size": 8, "discard_chance_on_air_exposure": 0.0,
                "targets": [
                    {"state": "minecraft:diorite",
                        "target": {"predicate_type": "minecraft:matching_blocks",
                            "blocks": ["minecraft:stone"]}},
                    {"state": "minecraft:granite",
                        "target": {"predicate_type": "minecraft:random_block_match",
                            "block": "minecraft:stone", "probability": 0.5}}
                ]}"#,
        );
        write(
            &worldgen.join("placed_feature/one.json"),
            r#"{"feature": "testns:diorite", "placement": [
                {"type": "minecraft:offset", "location": {"positive": true}}
            ]}"#,
        );
        write(
            &worldgen.join("biome/plains.json"),
            r#"{"features": [["testns:one"]]}"#,
        );
        let data = FeatureData::load(&root).expect("loads");
        assert_eq!(
            data.unimplemented_features.get("minecraft:tree"),
            Some(&1),
            "one increment per configured document of an unmodeled type"
        );
        assert_eq!(
            data.unimplemented_features
                .get("target:minecraft:random_block_match"),
            Some(&1),
            "one increment per ore target using an unmodeled predicate"
        );
        assert_eq!(
            data.unimplemented_features
                .get("placement:minecraft:offset"),
            Some(&1)
        );
        assert_eq!(
            data.unimplemented_features
                .get("offset:minecraft:missing_provider"),
            Some(&1),
            "the offset is modeled, but the parameter inside it that is not gets \
             its own key"
        );
        assert_eq!(
            data.unimplemented_features.len(),
            4,
            "the four namespaces stay separate: {:?}",
            data.unimplemented_features
        );
        assert!(
            !data
                .unimplemented_features
                .contains_key("minecraft:random_block_match"),
            "a rule or decoration kind never reads as a feature type"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_biome_reference_to_an_unknown_feature_is_an_error() {
        let data = FeatureData::default();
        let error = parse_biome_steps(
            "minecraft:plains",
            &json!({"features": [[], [], [], [], [], [], ["minecraft:ore_typo"]]}),
            &data.placed,
            &data.configured,
            &[],
            &[],
            &mut 0,
        )
        .expect_err("rejected");
        assert!(error.to_string().contains("references unknown"), "{error}");
    }

    #[test]
    fn an_empty_biome_section_yields_no_steps() {
        let steps = parse_biome_steps(
            "minecraft:plains",
            &json!({}),
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &[],
            &mut 0,
        )
        .expect("a biome without features contributes nothing");
        assert!(steps.is_empty());
    }

    // --- The decoration runtime -------------------------------------------

    /// A terrain the runtime can paint on without a generator: an unlimited
    /// volume of one base block, the writable zone pinned to one 16-wide
    /// chunk, and one biome for the `minecraft:biome` filter. Reads answer
    /// outside the writable chunk too, because a border block's six
    /// neighbours are queried regardless; above the build volume the terrain
    /// answer is air, which is what makes a vein's top rows exposed.
    #[derive(Debug)]
    struct TestTarget {
        min_y: i32,
        height: i32,
        chunk_x: i32,
        chunk_z: i32,
        base: String,
        write_anywhere: bool,
        ground: i32,
        surface: Option<i32>,
        air_above: Option<i32>,
        written: HashMap<(i32, i32, i32), String>,
        biome: Option<String>,
    }

    impl TestTarget {
        fn new() -> Self {
            Self {
                min_y: 0,
                height: 128,
                chunk_x: 0,
                chunk_z: 0,
                base: "minecraft:stone".to_string(),
                write_anywhere: false,
                ground: 127,
                surface: Some(127),
                air_above: None,
                written: HashMap::new(),
                biome: Some("minecraft:plains".to_string()),
            }
        }

        /// The chunk the pass may write into.
        fn anchored_at(mut self, chunk_x: i32, chunk_z: i32) -> Self {
            self.chunk_x = chunk_x;
            self.chunk_z = chunk_z;
            self
        }

        /// Let the pass write anywhere in the build volume, so a test can
        /// measure a vein's true extent instead of its clipped one.
        fn write_anywhere(mut self) -> Self {
            self.write_anywhere = true;
            self
        }

        /// The row every column's ocean-floor heightmap reports. The fixture
        /// terrain fills the build volume, so the default is its top row and no
        /// attempt is ever unanchored; a lower row leaves the sky above it empty
        /// for the anchor gate to read.
        fn with_ground(mut self, y: i32) -> Self {
            self.ground = y;
            self
        }

        /// The block every filled row of the fixture reads as, for a host test
        /// that has to name the terrain the cover sits on.
        fn with_base(mut self, name: &str) -> Self {
            self.base = name.to_string();
            self
        }

        /// The row every column's decoration-time world-surface map reports,
        /// for the `minecraft:heightmap` decoration to resolve. The fixture
        /// terrain fills the build volume, so the default is its top row; a
        /// target with no surface at all answers `None`.
        fn with_surface(mut self, y: i32) -> Self {
            self.surface = Some(y);
            self
        }

        fn without_surface(mut self) -> Self {
            self.surface = None;
            self
        }

        /// Leave every row above `y` empty, so the fixture reads as terrain up
        /// to `y` and air over it. Ground cover resolves its candidate onto the
        /// surface row and then asks whether that row is free, which the
        /// all-solid fixture could never answer.
        fn with_air_above(mut self, y: i32) -> Self {
            self.air_above = Some(y);
            self
        }

        fn in_biome(mut self, biome: Option<&str>) -> Self {
            self.biome = biome.map(str::to_owned);
            self
        }

        fn max_y(&self) -> i32 {
            self.min_y + self.height - 1
        }

        /// How many written positions hold `name`.
        fn blocks(&self, name: &str) -> usize {
            self.written
                .values()
                .filter(|state| state.as_str() == name)
                .count()
        }

        fn total(&self) -> usize {
            self.written.len()
        }
    }

    impl DecorationTarget for TestTarget {
        fn min_y(&self) -> i32 {
            self.min_y
        }

        fn height(&self) -> i32 {
            self.height
        }

        fn writable(&self, x: i32, y: i32, z: i32) -> bool {
            (self.write_anywhere || (x >> 4, z >> 4) == (self.chunk_x, self.chunk_z))
                && y >= self.min_y
                && y <= self.max_y()
        }

        fn block_at(&self, x: i32, y: i32, z: i32) -> Option<&str> {
            if let Some(name) = self.written.get(&(x, y, z)) {
                return Some(name);
            }
            if self.air_above.is_some_and(|top| y > top) {
                return Some("minecraft:air");
            }
            (y >= self.min_y && y <= self.max_y()).then_some(self.base.as_str())
        }

        fn is_air(&self, x: i32, y: i32, z: i32) -> bool {
            match self.block_at(x, y, z) {
                None => true,
                Some(name) => name == "minecraft:air",
            }
        }

        fn set_block(&mut self, x: i32, y: i32, z: i32, name: &str) {
            self.written.insert((x, y, z), name.to_string());
        }

        fn biome_at(&self, _x: i32, _y: i32, _z: i32) -> Option<String> {
            self.biome.clone()
        }

        fn ocean_floor_height(&self, _x: i32, _z: i32) -> i32 {
            self.ground
        }

        fn surface_height(&self, _x: i32, _z: i32) -> Option<i32> {
            self.surface
        }
    }

    /// An ore that replaces the fixture terrain's base block.
    fn stone_replacement(state: &str, size: i32, discard: f32) -> Rc<ConfiguredFeature> {
        Rc::new(ConfiguredFeature::Ore {
            size,
            targets: vec![OreTarget {
                predicate: RuleTest::Blocks {
                    blocks: vec!["minecraft:stone".to_string()],
                },
                state: state.to_string(),
            }],
            discard_chance_on_air_exposure: discard,
        })
    }

    fn wrapper(
        id: &str,
        feature: &Rc<ConfiguredFeature>,
        modifiers: Vec<PlacementModifier>,
    ) -> Rc<PlacedFeature> {
        Rc::new(PlacedFeature {
            id: id.to_string(),
            feature: Rc::clone(feature),
            modifiers,
        })
    }

    /// A registry running one step's schedule, listing everything it carries
    /// in the fixture biome. A single biome whose only chain is that step's
    /// list is numbered by position, which is what the derived ordering gives
    /// when one biome is the whole source.
    fn registry(step: Vec<Rc<PlacedFeature>>) -> FeatureData {
        let membership: HashSet<String> = step.iter().map(|feature| feature.id.clone()).collect();
        let mut ordinals = HashMap::new();
        let mut positions = Vec::new();
        for (index, feature) in step.iter().enumerate() {
            ordinals.insert(feature.id.clone(), index);
            positions.push(index);
        }
        FeatureData {
            steps: vec![StepSchedule {
                features: step,
                ordinals,
            }],
            biome_ordinals: HashMap::from([("minecraft:plains".to_string(), vec![positions])]),
            membership: HashMap::from([("minecraft:plains".to_string(), membership)]),
            ..Default::default()
        }
    }

    /// The fixture decoration region: one anchor chunk whose biome volume is
    /// the fixture biome every `TestTarget` reports.
    fn region() -> Vec<String> {
        vec!["minecraft:plains".to_string()]
    }

    /// The identifiers one generation step's schedule runs, in ordinal order.
    fn step_ids(data: &FeatureData, step: usize) -> Vec<String> {
        data.step(step)
            .unwrap_or_else(|| panic!("the pack has no step {step}"))
            .iter()
            .map(|feature| feature.id.clone())
            .collect()
    }

    /// Runs one vein on `target` from the stream `seed`/`ordinal` gives it.
    fn paint(
        mut target: TestTarget,
        seed: i64,
        ordinal: i32,
        origin: DecorationPos,
        feature: &ConfiguredFeature,
    ) -> TestTarget {
        let ConfiguredFeature::Ore {
            size,
            targets,
            discard_chance_on_air_exposure,
        } = feature
        else {
            panic!("the fixture ore is an ore: {feature:?}");
        };
        let mut random = DecorationRandom::new(seed);
        random.set_feature_seed(seed, ordinal, 0);
        let written = place_ore(
            &mut target,
            &mut random,
            &BlockTags::default(),
            *size,
            targets,
            *discard_chance_on_air_exposure,
            origin,
        );
        // The visited set makes each position of a vein a write, never a
        // repaint: the two counts have to agree.
        assert_eq!(
            written,
            target.total(),
            "one vein writes each position once"
        );
        target
    }

    /// Runs one ore attempt against a target that keeps its own stream, so a
    /// test can watch what the attempts of one chain spend in order.
    fn attempt(
        target: &mut TestTarget,
        random: &mut DecorationRandom,
        origin: DecorationPos,
        feature: &ConfiguredFeature,
    ) -> usize {
        let ConfiguredFeature::Ore {
            size,
            targets,
            discard_chance_on_air_exposure,
        } = feature
        else {
            panic!("the fixture ore is an ore: {feature:?}");
        };
        place_ore(
            target,
            random,
            &BlockTags::default(),
            *size,
            targets,
            *discard_chance_on_air_exposure,
            origin,
        )
    }

    /// The next three delegate draws of a stream, so it can be compared
    /// against a reference stream that consumed a known number of longs.
    fn signature(random: &DecorationRandom) -> [i32; 3] {
        let mut probe = random.clone();
        [probe.next_int(), probe.next_int(), probe.next_int()]
    }

    fn reference(baseline: &DecorationRandom, longs: u32) -> [i32; 3] {
        let mut probe = baseline.clone();
        for _ in 0..longs {
            probe.next_bits(32);
        }
        signature(&probe)
    }

    #[test]
    fn a_vein_is_reproducible_and_seed_dependent() {
        let ore = stone_replacement("minecraft:granite", 64, 0.0);
        let origin = DecorationPos {
            x: 40,
            y: 64,
            z: -12,
        };
        let first = paint(TestTarget::new().write_anywhere(), 2026, 3, origin, &ore);
        let again = paint(TestTarget::new().write_anywhere(), 2026, 3, origin, &ore);
        assert!(
            first.total() > 8,
            "the fixture vein must actually place blocks, got {}",
            first.total()
        );
        assert_eq!(
            first.written, again.written,
            "the same seed, ordinal and origin must replay identically"
        );
        let other = paint(TestTarget::new().write_anywhere(), 2027, 3, origin, &ore);
        assert_ne!(
            first.written, other.written,
            "a different world seed must move the stream"
        );
    }

    /// The anchor gate: a vein's box is measured against the heightmap row of
    /// the columns under its footprint, and an attempt whose base row clears
    /// every one of them is abandoned before it draws a radius. The row that
    /// decides is the box's own base — seven under the origin for a size-64
    /// vein — so the boundary sits a whole box-height above where the shape
    /// would first miss the ground, and what a dropped attempt costs the chain
    /// is the three draws that fixed its axis.
    #[test]
    fn a_box_that_clears_the_terrain_costs_only_its_axis_draws() {
        let ore = stone_replacement("minecraft:granite", 64, 0.0);
        let ground = 40;
        let origin = DecorationPos {
            x: 4,
            y: ground + 7,
            z: 4,
        };

        let mut boundary = TestTarget::new().with_ground(ground);
        let mut random = DecorationRandom::new(2026);
        random.set_feature_seed(2026, 0, 0);
        let placed = attempt(&mut boundary, &mut random, origin, &ore);
        assert!(
            placed > 0,
            "a box whose base row reaches the terrain places, got {placed}"
        );

        let mut floating = TestTarget::new().with_ground(ground);
        let mut random = DecorationRandom::new(2026);
        random.set_feature_seed(2026, 0, 0);
        let dropped = attempt(
            &mut floating,
            &mut random,
            DecorationPos { y: 48, ..origin },
            &ore,
        );
        assert_eq!(dropped, 0, "the same box one row higher is dropped");
        assert_eq!(
            floating.total(),
            0,
            "and the abandoned box leaves nothing behind"
        );

        // The fixture's terrain fills the build volume unless a test lowers it,
        // so the row above is the gate's doing, not a vein that lands nowhere:
        // the identical attempt places into a full column.
        let mut full_column = TestTarget::new();
        let mut random = DecorationRandom::new(2026);
        random.set_feature_seed(2026, 0, 0);
        let unbounded = attempt(
            &mut full_column,
            &mut random,
            DecorationPos { y: 48, ..origin },
            &ore,
        );
        assert!(
            unbounded > 0,
            "without the heightmap floor the same attempt places, got {unbounded}"
        );

        // A dropped attempt is over the moment the gate answers, so the stream
        // it leaves behind is the stream three draws — the angle and the two
        // height jitters — would have left, with none of the vein's radii spent.
        let mut baseline = DecorationRandom::new(2026);
        baseline.set_feature_seed(2026, 0, 0);

        let mut dropped = TestTarget::new().with_ground(ground);
        let mut spent = baseline.clone();
        let skipped = attempt(
            &mut dropped,
            &mut spent,
            DecorationPos { y: 100, ..origin },
            &ore,
        );
        assert_eq!(skipped, 0, "the sky-high box is dropped");
        assert_eq!(
            signature(&spent),
            reference(&baseline, 3),
            "a dropped attempt costs its three axis draws, not its radii"
        );

        // Which is what the next attempt of the same chain is written against:
        // it lands where three draws leave the stream, not where a full vein
        // would have left it.
        let mut chained = TestTarget::new().with_ground(ground);
        let mut random = baseline.clone();
        attempt(
            &mut chained,
            &mut random,
            DecorationPos { y: 100, ..origin },
            &ore,
        );
        let after = attempt(&mut chained, &mut random, origin, &ore);

        let mut resumed = TestTarget::new().with_ground(ground);
        let mut random = baseline.clone();
        let _ = random.next_float();
        let _ = random.next_int_bounded(3);
        let _ = random.next_int_bounded(3);
        let direct = attempt(&mut resumed, &mut random, origin, &ore);

        assert!(direct > 0, "the anchored attempt places, got {direct}");
        assert_eq!(
            after, direct,
            "the attempt after a dropped one draws the numbers a three-draw gap leaves"
        );
        assert_eq!(
            chained.written, resumed.written,
            "and lands the same blocks at the same positions"
        );
    }

    /// The premise of the ring-one replay, measured from the geometry itself
    /// rather than from the box arithmetic: a vein's blocks never lie more
    /// than one chunk away from the chunk its attempt was anchored in, so
    /// replaying the eight neighbours captures every write a wider radius
    /// could add.
    #[test]
    fn a_vein_never_reaches_past_the_neighbour_chunk() {
        for size in [4, 10, 20, 64] {
            let ore = stone_replacement("minecraft:granite", size, 0.0);
            let mut total = 0usize;
            for ordinal in 0..6i32 {
                for seed in [2026i64, 7777] {
                    // Origins spread across a chunk the way `in_square` does.
                    let origin = DecorationPos {
                        x: (ordinal * 3 + seed as i32 % 16).rem_euclid(16),
                        y: 64,
                        z: ordinal * 5,
                    };
                    let target = paint(
                        TestTarget::new().write_anywhere(),
                        seed,
                        ordinal,
                        origin,
                        &ore,
                    );
                    let (chunk_x, chunk_z) = (origin.x >> 4, origin.z >> 4);
                    for &(x, _y, z) in target.written.keys() {
                        assert!(
                            ((x >> 4) - chunk_x).abs() <= 1 && ((z >> 4) - chunk_z).abs() <= 1,
                            "a size-{size} vein at ({x}, {z}) reached chunk {} {}, outside the \
                             one-chunk reach of its anchor chunk {chunk_x}, {chunk_z}",
                            x >> 4,
                            z >> 4,
                        );
                    }
                    total += target.total();
                }
            }
            assert!(total > 0, "a size-{size} vein must place something");
        }
    }

    #[test]
    fn writes_outside_the_decorated_chunk_are_refused() {
        let ore = stone_replacement("minecraft:granite", 64, 0.0);
        let target = paint(
            TestTarget::new().anchored_at(3, -2),
            2026,
            0,
            DecorationPos {
                x: 48,
                y: 64,
                z: -32,
            },
            &ore,
        );
        assert!(
            target.total() > 0,
            "the fixture chunk must be reached at all"
        );
        for &(x, _y, z) in target.written.keys() {
            assert_eq!(
                (x >> 4, z >> 4),
                (3, -2),
                "the pass clips to the chunk it decorates; the neighbour paints its half of a straddling vein"
            );
        }
    }

    /// A chunk-1 anchor reaches chunk 0 and a chunk-5 anchor cannot, which is
    /// what makes the three-by-three replay both sufficient and the smallest
    /// honest window.
    #[test]
    fn a_neighbour_anchor_reaches_the_target_chunk_and_a_far_one_does_not() {
        let ore = stone_replacement("minecraft:granite", 64, 0.0);
        let mut from_neighbour = 0usize;
        let mut from_far_chunk = 0usize;
        for seed in 2026..2034 {
            for ordinal in 0..4i32 {
                for offset in [0i32, 4, 8, 12] {
                    for (anchor, reached) in [(1, &mut from_neighbour), (5, &mut from_far_chunk)] {
                        let target = paint(
                            TestTarget::new().anchored_at(0, 0),
                            seed,
                            ordinal,
                            DecorationPos {
                                x: (anchor << 4) + offset,
                                y: 64,
                                z: 8,
                            },
                            &ore,
                        );
                        *reached += target.total();
                    }
                }
            }
        }
        assert!(
            from_neighbour > 0,
            "a vein seeded in the neighbour chunk must bleed into the target chunk"
        );
        assert_eq!(
            from_far_chunk, 0,
            "an anchor four chunks away is past any vein's reach"
        );
    }

    /// Both a zero and a certain discard cost no random draws, so the same
    /// vein geometry is evaluated twice and only the exposure test differs:
    /// the certain run is a strict subset of the free one, thinned exactly
    /// where the terrain runs out.
    #[test]
    fn air_exposure_only_removes_blocks_that_touch_air() {
        let loose = paint(
            TestTarget::new().write_anywhere(),
            2026,
            1,
            DecorationPos {
                x: 20,
                y: 126,
                z: 20,
            },
            &stone_replacement("minecraft:granite", 64, 0.0),
        );
        let strict = paint(
            TestTarget::new().write_anywhere(),
            2026,
            1,
            DecorationPos {
                x: 20,
                y: 126,
                z: 20,
            },
            &stone_replacement("minecraft:granite", 64, 1.0),
        );
        assert!(
            loose.written.keys().any(|&(_x, y, _z)| y == loose.max_y()),
            "the fixture vein must reach the top of the volume to test exposure"
        );
        assert!(
            strict.total() > 0 && strict.total() < loose.total(),
            "the exposed rows must drop out and the buried core must stay: {} vs {}",
            strict.total(),
            loose.total()
        );
        for (position, name) in &strict.written {
            assert_eq!(
                loose.written.get(position),
                Some(name),
                "a discard roll must thin the vein, not move it, at {position:?}"
            );
            assert!(
                position.1 < strict.max_y(),
                "{position:?} is exposed above the volume and must have been rejected"
            );
        }
    }

    #[test]
    fn each_range_provider_draws_its_documented_number_of_randoms() {
        let cases: [(RangeProvider, i32, u32); 5] = [
            (RangeProvider::Constant(64), 64, 0),
            (
                RangeProvider::Uniform {
                    min_inclusive: VerticalAnchor::Absolute(0),
                    max_inclusive: VerticalAnchor::Absolute(0),
                },
                0,
                1,
            ),
            (
                RangeProvider::Uniform {
                    min_inclusive: VerticalAnchor::Absolute(0),
                    max_inclusive: VerticalAnchor::Absolute(15),
                },
                15,
                1,
            ),
            (
                RangeProvider::Trapezoid {
                    min_inclusive: VerticalAnchor::Absolute(3),
                    max_inclusive: VerticalAnchor::Absolute(3),
                },
                3,
                1,
            ),
            (
                RangeProvider::Trapezoid {
                    min_inclusive: VerticalAnchor::Absolute(0),
                    max_inclusive: VerticalAnchor::Absolute(1),
                },
                1,
                2,
            ),
        ];
        let baseline = DecorationRandom::new(2026);
        for (provider, upper_bound, longs) in cases {
            let mut sampled = baseline.clone();
            let value = provider.sample(0, 128, &mut sampled);
            assert!(
                (0..=upper_bound).contains(&value),
                "{provider:?} sampled {value} outside its band"
            );
            if let RangeProvider::Constant(_) = provider {
                assert_eq!(value, 64);
            }
            assert_eq!(
                signature(&sampled),
                reference(&baseline, longs),
                "{provider:?} must cost exactly {longs} delegate longs"
            );
        }
        // Bounds the pack wrote the wrong way round: the reference runtime
        // reports the minimum and draws nothing at all.
        let inverted = RangeProvider::Uniform {
            min_inclusive: VerticalAnchor::Absolute(70),
            max_inclusive: VerticalAnchor::Absolute(20),
        };
        let mut sampled = baseline.clone();
        assert_eq!(inverted.sample(0, 128, &mut sampled), 70);
        assert_eq!(
            signature(&sampled),
            reference(&baseline, 0),
            "an inverted band must not draw"
        );
    }

    #[test]
    fn a_rarity_filter_gates_the_whole_attempt_and_a_count_repeats_it() {
        let ore = stone_replacement("minecraft:granite", 20, 0.0);
        let chain = |chance: i64| {
            registry(vec![wrapper(
                "testns:ore",
                &ore,
                vec![
                    PlacementModifier::Count {
                        count: RangeProvider::Constant(3),
                    },
                    PlacementModifier::HeightRange {
                        height: RangeProvider::Constant(64),
                    },
                    PlacementModifier::RarityFilter { chance },
                ],
            )])
        };
        let mut always = TestTarget::new().write_anywhere();
        let written = chain(1).decorate_step(&mut always, 0, 0, 0, 2026, &region());
        assert!(
            written > 0 && always.blocks("minecraft:granite") > 0,
            "a chance of one fires every time and the count multiplies the attempt"
        );
        let mut rare = TestTarget::new().write_anywhere();
        assert_eq!(
            chain(1_000_000).decorate_step(&mut rare, 0, 0, 0, 2026, &region()),
            0,
            "one attempt in a million does not fire on this stream"
        );
    }

    #[test]
    fn the_biome_filter_keeps_only_a_biome_that_lists_the_feature() {
        let ore = stone_replacement("minecraft:granite", 20, 0.0);
        let data = registry(vec![wrapper(
            "testns:ore",
            &ore,
            vec![
                PlacementModifier::Count {
                    count: RangeProvider::Constant(2),
                },
                PlacementModifier::HeightRange {
                    height: RangeProvider::Constant(64),
                },
                PlacementModifier::Biome,
            ],
        )]);
        let mut listed = TestTarget::new().write_anywhere();
        assert!(
            data.decorate_step(&mut listed, 0, 0, 0, 2026, &region()) > 0,
            "the fixture biome lists the feature"
        );
        let mut elsewhere = TestTarget::new()
            .write_anywhere()
            .in_biome(Some("minecraft:desert"));
        assert_eq!(
            data.decorate_step(&mut elsewhere, 0, 0, 0, 2026, &region()),
            0,
            "a biome that does not list it places nothing"
        );
        let mut unknown = TestTarget::new().write_anywhere().in_biome(None);
        assert_eq!(
            data.decorate_step(&mut unknown, 0, 0, 0, 2026, &region()),
            0
        );
    }

    /// A feature with no runtime here places nothing and still costs no
    /// draws, but it holds its ordinal, so the streams of the features after
    /// it are the ones the pack seeded them with.
    #[test]
    fn an_unmodeled_feature_still_shifts_its_neighbours() {
        let ore = stone_replacement("minecraft:granite", 20, 0.0);
        let vein = wrapper(
            "testns:ore",
            &ore,
            vec![
                PlacementModifier::Count {
                    count: RangeProvider::Constant(2),
                },
                PlacementModifier::HeightRange {
                    height: RangeProvider::Constant(64),
                },
            ],
        );
        let forest = wrapper(
            "testns:forest",
            &Rc::new(ConfiguredFeature::Unsupported {
                kind: "minecraft:tree".to_string(),
            }),
            Vec::new(),
        );
        let mut alone = TestTarget::new().write_anywhere();
        registry(vec![Rc::clone(&vein)]).decorate_step(&mut alone, 0, 0, 0, 2026, &region());
        let mut shifted = TestTarget::new().write_anywhere();
        registry(vec![Rc::clone(&forest), Rc::clone(&vein)]).decorate_step(
            &mut shifted,
            0,
            0,
            0,
            2026,
            &region(),
        );
        assert!(!alone.written.is_empty() && !shifted.written.is_empty());
        assert!(shifted.blocks("minecraft:granite") > 0 && alone.blocks("minecraft:granite") > 0);
        assert_ne!(
            alone.written, shifted.written,
            "the unmodeled feature holds a slot, so the vein after it is seeded differently"
        );
    }

    #[test]
    fn rule_trees_nest_any_all_and_not_tests() {
        let tags = BlockTags::default();
        let tree = RuleTest::AllOf(vec![
            RuleTest::Not(Box::new(RuleTest::Block {
                block: "minecraft:bedrock".to_string(),
            })),
            RuleTest::AnyOf(vec![
                RuleTest::Blocks {
                    blocks: vec![
                        "minecraft:stone".to_string(),
                        "minecraft:deepslate".to_string(),
                    ],
                },
                RuleTest::Tag {
                    tag: "minecraft:base_stone_overworld".to_string(),
                },
            ]),
            RuleTest::Height {
                min_inclusive: -64,
                max_inclusive: 0,
            },
        ]);
        assert!(tree.is_modelable());
        assert!(tree.test(Some("minecraft:deepslate"), -40, &tags));
        assert!(
            tree.test(Some("minecraft:stone"), 0, &tags),
            "both bounds are inclusive"
        );
        assert!(
            !tree.test(Some("minecraft:stone"), 1, &tags),
            "the height test bounds the band"
        );
        assert!(
            !tree.test(Some("minecraft:bedrock"), -40, &tags),
            "the not-test holds"
        );
        assert!(
            !tree.test(Some("minecraft:granite"), -40, &tags),
            "an unknown tag matches nothing rather than assuming membership"
        );
        assert!(
            !tree.test(None, -40, &tags),
            "a position holding no block matches no block test"
        );
        let unsupported = RuleTest::AnyOf(vec![
            RuleTest::Block {
                block: "minecraft:stone".to_string(),
            },
            RuleTest::Unsupported {
                kind: "minecraft:random_block_match".to_string(),
            },
        ]);
        assert!(!unsupported.is_modelable());
        assert!(
            !RuleTest::Unsupported {
                kind: "minecraft:random_block_match".to_string(),
            }
            .test(Some("minecraft:stone"), 0, &tags),
            "an unmodeled test matches nothing when it is asked"
        );
    }

    #[test]
    fn tag_membership_resolves_nested_references_and_cycles() {
        let root = scratch_root("tags");
        write(
            &root.join("data/minecraft/tags/block/base_stone_overworld.json"),
            r#"{"values": ["minecraft:stone", "minecraft:deepslate"]}"#,
        );
        // Filed away, so its id keeps the directory.
        write(
            &root.join("data/testns/tags/block/deep/deep_stone.json"),
            r#"{"values": ["minecraft:cobblestone", {"tag": "testns:base_stone"}]}"#,
        );
        write(
            &root.join("data/testns/tags/block/base_stone.json"),
            r#"{"values": [{"id": "minecraft:stone", "required": true}, {"tag": "testns:deep/deep_stone"}]}"#,
        );
        let tags = BlockTags::load(&root).expect("loads");
        assert!(tags.contains("minecraft:base_stone_overworld", "minecraft:stone"));
        assert!(
            !tags.contains("minecraft:base_stone_overworld", "minecraft:granite"),
            "a member list is exactly what the pack declared"
        );
        assert!(tags.contains("testns:base_stone", "minecraft:stone"));
        assert!(
            tags.contains("testns:base_stone", "minecraft:cobblestone"),
            "a reference to another tag resolves into the member list"
        );
        assert!(
            tags.contains("testns:deep/deep_stone", "minecraft:stone"),
            "and so does the reference that reaches back through the cycle"
        );
        assert_eq!(tags.len(), 3, "one entry per path under the category");

        let empty = scratch_root("no-tags");
        let tags = BlockTags::load(&empty).expect("a pack without tags loads");
        assert!(
            tags.is_empty() && !tags.contains("minecraft:base_stone_overworld", "minecraft:stone")
        );
        fs::remove_dir_all(root).expect("cleanup");
        fs::remove_dir_all(empty).expect("cleanup");
    }

    /// A hand-built decorating pack: two ores and an unmodeled tree, placed
    /// features that reference them, nested block tags, and two biomes whose
    /// positional `features` arrays the caller supplies.
    fn schedule_pack(label: &str, biome_one: &str, biome_two: &str) -> PathBuf {
        let root = scratch_root(label);
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("feature/granite.json"),
            r#"{"type": "minecraft:ore", "size": 64, "discard_chance_on_air_exposure": 0.0,
                "targets": [{"state": "minecraft:granite",
                    "target": {"predicate_type": "minecraft:tag_match", "tag": "testns:base_stone"}}]}"#,
        );
        write(
            &worldgen.join("feature/tuff.json"),
            r#"{"type": "minecraft:ore", "size": 64, "discard_chance_on_air_exposure": 0.0,
                "targets": [{"state": "minecraft:tuff",
                    "target": {"predicate_type": "minecraft:block_match", "block": "minecraft:stone"}}]}"#,
        );
        write(
            &worldgen.join("feature/swamp.json"),
            r#"{"type": "minecraft:tree"}"#,
        );
        write(
            &root.join("data/testns/tags/block/base_stone.json"),
            r#"{"values": ["minecraft:stone", {"tag": "testns:deep_stone"}]}"#,
        );
        write(
            &root.join("data/testns/tags/block/deep_stone.json"),
            r#"{"values": ["minecraft:dirt"]}"#,
        );
        write(
            &worldgen.join("placed_feature/ore_a.json"),
            r#"{"feature": "testns:granite", "placement": [
                {"type": "minecraft:count", "count": 2},
                {"type": "minecraft:height_range", "height": {"type": "minecraft:constant",
                    "value": {"absolute": 60}}},
                {"type": "minecraft:biome"}
            ]}"#,
        );
        write(
            &worldgen.join("placed_feature/ore_b.json"),
            r#"{"feature": "testns:tuff", "placement": [
                {"type": "minecraft:count", "count": 2},
                {"type": "minecraft:height_range", "height": {"type": "minecraft:constant",
                    "value": {"absolute": 60}}}
            ]}"#,
        );
        write(
            &worldgen.join("placed_feature/aa_forest.json"),
            r#"{"feature": "testns:swamp", "placement": [{"type": "minecraft:biome"}]}"#,
        );
        write(
            &worldgen.join("biome/one.json"),
            &format!(r#"{{"features": {biome_one}}}"#),
        );
        write(
            &worldgen.join("biome/two.json"),
            &format!(r#"{{"features": {biome_two}}}"#),
        );
        root
    }

    #[test]
    fn the_step_schedule_orders_the_biome_chains_into_step_schedules() {
        // Biome `one` lists ore_b and the tree at step 0 and ore_a at step 2;
        // biome `two` lists ore_a at step 0 and ore_b at step 3. Neither
        // contradicts the other, so the walk has one answer: every biome's
        // own relative order holds, across the step boundaries too.
        let root = schedule_pack(
            "schedule",
            r#"[["testns:ore_b", "testns:aa_forest"], [], ["testns:ore_a"]]"#,
            r#"[["testns:ore_a"], [], [], ["testns:ore_b"]]"#,
        );
        let data = FeatureData::load(&root).expect("loads");
        let ids = |step: usize| -> Vec<&str> {
            data.step(step)
                .unwrap_or_else(|| panic!("step {step}"))
                .iter()
                .map(|feature| feature.id.as_str())
                .collect()
        };
        assert_eq!(
            ids(0),
            ["testns:ore_a", "testns:ore_b", "testns:aa_forest"],
            "the step order comes from the two chains, not from the identifiers"
        );
        assert!(
            !data.step(0).expect("step 0")[2].is_modelable(),
            "the tree holds its slot without being modeled"
        );
        assert_eq!(
            data.decorated_steps(),
            [0, 2, 3],
            "an empty step carries nothing and the schedule is as wide as the widest biome"
        );
        assert_eq!(ids(2), ["testns:ore_a"]);
        assert_eq!(ids(3), ["testns:ore_b"]);
        // A feature listed at two steps is numbered separately in each, and
        // the pair of step and ordinal is its random identity.
        assert_eq!(
            data.steps[0].ordinal_of("testns:ore_b"),
            Some(1),
            "ore_b is second in step 0's schedule"
        );
        assert_eq!(
            data.steps[3].ordinal_of("testns:ore_b"),
            Some(0),
            "and first in step 3's"
        );

        // The ordinals a chunk runs come from the biomes in its neighbourhood,
        // not from the biome a candidate position lands in.
        let one = "testns:one".to_string();
        let two = "testns:two".to_string();
        let only_one = vec![one.clone()];
        let only_two = vec![two.clone()];
        let both = vec![two.clone(), one.clone()];
        assert_eq!(data.region_ordinals(0, &only_one), vec![1, 2]);
        assert_eq!(data.region_ordinals(0, &only_two), vec![0]);
        assert_eq!(data.region_ordinals(0, &both), vec![0, 1, 2]);
        assert_eq!(data.region_ordinals(2, &only_one), vec![0]);
        assert_eq!(data.region_ordinals(3, &only_two), vec![0]);
        assert!(data.region_ordinals(3, &only_one).is_empty());
        assert!(data.region_ordinals(1, &only_two).is_empty());

        assert!(data.biome_lists("testns:one", "testns:ore_a"));
        assert!(
            data.biome_lists("testns:two", "testns:ore_b"),
            "membership is flat over the biome's whole positional array"
        );
        assert!(
            !data.biome_lists("testns:two", "testns:aa_forest"),
            "the second biome never lists the tree"
        );
        assert_eq!(
            data.unimplemented_features.get("minecraft:tree"),
            Some(&1),
            "the startup report names what it skipped"
        );
        assert!(data.tags().contains("testns:base_stone", "minecraft:dirt"));

        // And the runtime decorates from it: the granite ore needs the tag,
        // the biome filter needs the membership, and a region that leaves an
        // ordinal out runs nothing at all.
        let both = vec![one, two];
        let mut plains = TestTarget::new()
            .anchored_at(0, 0)
            .in_biome(Some("testns:one"));
        data.decorate_step(&mut plains, 0, 0, 0, 2026, &both);
        assert!(plains.blocks("minecraft:granite") > 0, "tag_match fired");
        assert!(plains.blocks("minecraft:tuff") > 0, "block_match fired");
        let mut desert = TestTarget::new()
            .anchored_at(0, 0)
            .in_biome(Some("minecraft:desert"));
        data.decorate_step(&mut desert, 0, 0, 0, 2026, &both);
        assert_eq!(
            desert.blocks("minecraft:granite"),
            0,
            "the biome filter gated it"
        );
        assert!(
            desert.blocks("minecraft:tuff") > 0,
            "ore_b has no biome filter"
        );
        let mut single = TestTarget::new()
            .anchored_at(0, 0)
            .in_biome(Some("testns:one"));
        data.decorate_step(&mut single, 0, 0, 0, 2026, &only_two);
        assert!(
            single.blocks("minecraft:granite") > 0,
            "the one ordinal that region contributes is ore_a"
        );
        assert_eq!(
            single.blocks("minecraft:tuff"),
            0,
            "ore_b is not listed at step 0 by that biome"
        );

        let empty = scratch_root("no-features");
        assert!(FeatureData::load(&empty).expect("loads").is_empty());
        fs::remove_dir_all(root).expect("cleanup");
        fs::remove_dir_all(empty).expect("cleanup");
    }

    /// Running one ordinal alone is the same as running it inside its step,
    /// because each feature reseeds from the chunk's decoration seed with its
    /// own ordinal and step and never borrows a neighbour's stream. This is
    /// what lets a measurement attribute a written block to one feature.
    #[test]
    fn one_ordinal_alone_reproduces_its_share_of_the_step() {
        let root = schedule_pack(
            "attribution",
            r#"[["testns:ore_b", "testns:aa_forest"], [], ["testns:ore_a"]]"#,
            r#"[["testns:ore_a"], [], [], ["testns:ore_b"]]"#,
        );
        let data = FeatureData::load(&root).expect("loads");
        let both = vec!["testns:one".to_string(), "testns:two".to_string()];

        let mut step = TestTarget::new()
            .anchored_at(0, 0)
            .in_biome(Some("testns:one"));
        data.decorate_step(&mut step, 0, 0, 0, 2026, &both);
        let mut granite = TestTarget::new()
            .anchored_at(0, 0)
            .in_biome(Some("testns:one"));
        let granite_draws = data.decorate_ordinal(&mut granite, 0, 0, 0, 0, 2026);
        let mut tuff = TestTarget::new()
            .anchored_at(0, 0)
            .in_biome(Some("testns:one"));
        let tuff_draws = data.decorate_ordinal(&mut tuff, 0, 1, 0, 0, 2026);
        assert!(
            granite_draws > 0 && tuff_draws > 0,
            "both veins place when run alone"
        );

        // The step runs its ordinals ascending, so a position the granite
        // ordinal reached keeps granite: the later tuff ordinal reads stone
        // there, fails its own target test and does not overwrite it.
        let mut expected = granite.written.clone();
        for (position, name) in &tuff.written {
            expected.entry(*position).or_insert_with(|| name.clone());
        }
        assert_eq!(
            step.written, expected,
            "each ordinal draws alone exactly as it draws inside its step"
        );

        let mut unmodeled = TestTarget::new()
            .anchored_at(0, 0)
            .in_biome(Some("testns:one"));
        assert_eq!(
            data.decorate_ordinal(&mut unmodeled, 0, 2, 0, 0, 2026),
            0,
            "the tree keeps its slot in the schedule but places nothing here"
        );

        let mut past_end = TestTarget::new().anchored_at(0, 0);
        assert_eq!(
            data.decorate_ordinal(&mut past_end, 0, 9, 0, 0, 2026),
            0,
            "an ordinal past the step's schedule holds nothing"
        );
        let mut no_step = TestTarget::new().anchored_at(0, 0);
        assert_eq!(
            data.decorate_ordinal(&mut no_step, 9, 0, 0, 0, 2026),
            0,
            "a step the schedule never built contributes nothing"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// Two biomes that list the same pair of features in opposite orders give
    /// the ordering nothing consistent to build: the reference runtime drops
    /// one biome at a time, in source order, and rebuilds from scratch.
    #[test]
    fn the_step_schedule_recovers_from_a_biome_order_cycle() {
        let root = schedule_pack(
            "cycle",
            r#"[["testns:ore_b", "testns:aa_forest", "testns:ore_a"], [], ["testns:ore_a"]]"#,
            r#"[["testns:ore_a", "testns:ore_b"]]"#,
        );
        let data = FeatureData::load(&root).expect("loads");
        let ids: Vec<&str> = data
            .step(0)
            .expect("step 0")
            .iter()
            .map(|feature| feature.id.as_str())
            .collect();
        assert_eq!(
            ids,
            ["testns:ore_a", "testns:ore_b"],
            "the surviving biome's own order is all that is left"
        );
        assert_eq!(
            data.decorated_steps(),
            [0],
            "the dropped biome was the only one listing steps 1 and 2"
        );
        assert_eq!(
            data.unimplemented_features.get("feature_order_cycle"),
            Some(&1),
            "the recovery is reported, not silent"
        );
        let one = "testns:one".to_string();
        let two = "testns:two".to_string();
        assert!(
            data.region_ordinals(0, &[one]).is_empty(),
            "a biome the schedule dropped contributes nothing"
        );
        assert_eq!(data.region_ordinals(0, &[two]), vec![0, 1]);
        assert!(
            data.biome_lists("testns:one", "testns:aa_forest"),
            "membership is read from the pack, not from the surviving order"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// Two biomes that share no features give the ordering nothing to
    /// interleave by, so which step-0 list comes first is decided by the
    /// source's own declaration order.
    #[test]
    fn incomparable_biomes_are_ordered_by_the_biome_source() {
        let ore = stone_replacement("minecraft:granite", 8, 0.0);
        let a = wrapper("testns:ore_a", &ore, Vec::new());
        let b = wrapper("testns:ore_b", &ore, Vec::new());
        let mut data = FeatureData {
            biomes: HashMap::from([
                ("testns:one".to_string(), vec![vec![Rc::clone(&a)]]),
                ("testns:two".to_string(), vec![vec![Rc::clone(&b)]]),
            ]),
            ..Default::default()
        };
        data.set_biome_order(&["testns:one", "testns:two"]);
        // The walk emits a node only after everything it points to, and the
        // schedule is the reversal of that emission, so the biome listed
        // later by the source ends up with the lower ordinal.
        assert_eq!(step_ids(&data, 0), ["testns:ore_b", "testns:ore_a"]);
        let one = "testns:one".to_string();
        let only_one = vec![one];
        assert_eq!(data.region_ordinals(0, &only_one), vec![1]);
        data.set_biome_order(&["testns:two", "testns:one"]);
        assert_eq!(step_ids(&data, 0), ["testns:ore_a", "testns:ore_b"]);
        assert_eq!(data.region_ordinals(0, &only_one), vec![0]);
    }

    /// The ground-cover feature: one state written at the surviving position.
    fn ground_cover(state: &str) -> Rc<ConfiguredFeature> {
        Rc::new(ConfiguredFeature::SimpleBlock {
            to_place: StateProvider::Constant {
                state: state.to_string(),
            },
        })
    }

    /// The tag fixture a `minecraft:matching_block_tag` predicate reads: air is
    /// a member, the fixture's base stone is not.
    fn air_tags() -> BlockTags {
        BlockTags {
            entries: HashMap::from([(
                "minecraft:air".to_string(),
                vec!["minecraft:air".to_string()],
            )]),
        }
    }

    /// Runs one ground-cover chain over the plains biome of the fixture region.
    fn cover_run(
        feature: &Rc<ConfiguredFeature>,
        modifiers: Vec<PlacementModifier>,
        target: &mut TestTarget,
    ) -> usize {
        let data = registry(vec![wrapper("testns:cover", feature, modifiers)]);
        data.decorate_step(target, 0, 0, 0, 2026, &region())
    }

    #[test]
    fn the_heightmap_decoration_resolves_the_surface_row_of_its_map() {
        // Terrain up to row 70, air over it, and an ocean-floor row the world
        // surface does not report: the two map families have to answer their
        // own row, not each other's.
        let lift = || PlacementModifier::Offset {
            x: RangeProvider::Constant(0),
            y: RangeProvider::Constant(1),
            z: RangeProvider::Constant(0),
        };
        for (name, expected) in [
            (HeightmapType::MotionBlocking, 70),
            (HeightmapType::MotionBlockingNoLeaves, 70),
            (HeightmapType::WorldSurfaceWg, 70),
            (HeightmapType::OceanFloor, 60),
            (HeightmapType::OceanFloorWg, 60),
        ] {
            let surface = if expected == 70 { 70 } else { 80 };
            let target = TestTarget::new()
                .with_surface(surface)
                .with_ground(60)
                .with_air_above(59);
            assert_eq!(
                name.row(&target, 0, 0),
                Some(expected),
                "{name:?} resolves its own row"
            );
            let mut target = TestTarget::new()
                .with_surface(surface)
                .with_ground(60)
                .with_air_above(59);
            let written = cover_run(
                &ground_cover("minecraft:short_grass"),
                vec![
                    PlacementModifier::Heightmap {
                        heightmap: name.clone(),
                    },
                    lift(),
                ],
                &mut target,
            );
            assert_eq!(
                target.written.get(&(0, expected + 1, 0)),
                Some(&"minecraft:short_grass".to_string()),
                "{name:?} lands its candidate one row above its own surface"
            );
            assert_eq!(written, 1, "{name:?} places exactly one block");
        }
    }

    #[test]
    fn a_heightmap_over_a_column_with_no_surface_drops_the_candidate() {
        // The void must not be decorated: every modeled map type, the terrain
        // rows included, drops the candidate when the column answers nothing.
        for name in [
            HeightmapType::MotionBlocking,
            HeightmapType::MotionBlockingNoLeaves,
            HeightmapType::WorldSurfaceWg,
            HeightmapType::OceanFloor,
            HeightmapType::OceanFloorWg,
        ] {
            let target = TestTarget::new().without_surface();
            assert_eq!(name.row(&target, 0, 0), None, "{name:?} reads the void");
            let mut target = TestTarget::new().without_surface().write_anywhere();
            let written = cover_run(
                &ground_cover("minecraft:short_grass"),
                vec![PlacementModifier::Heightmap {
                    heightmap: name.clone(),
                }],
                &mut target,
            );
            assert_eq!(written, 0, "{name:?} must drop, not decorate the void");
            assert!(target.written.is_empty(), "{name:?} wrote into the void");
        }
    }

    #[test]
    fn an_unknown_heightmap_name_stays_a_named_gap() {
        // A map name the pack invents is not silently read as one of the five.
        assert!(matches!(
            HeightmapType::from_name("MOTION_BLOCKING"),
            HeightmapType::MotionBlocking
        ));
        let typo = HeightmapType::from_name("WORLD_SURFACE");
        assert!(
            !typo.is_modelable()
                && matches!(typo, HeightmapType::Unsupported { kind } if kind == "WORLD_SURFACE"),
            "an unmapped name keeps its own name"
        );
        let root = report_pack(
            "report-heightmap-gap",
            r#"[{"type": "minecraft:count", "count": 1},
                {"type": "minecraft:heightmap", "heightmap": "WORLD_SURFACE"},
                {"type": "minecraft:biome"}]"#,
        );
        let data = FeatureData::load(&root).expect("loads");
        assert_eq!(
            data.unimplemented_features.get("heightmap:WORLD_SURFACE"),
            Some(&1),
            "the parameter is named, not only its decoration"
        );
        assert_eq!(
            data.unimplemented_features
                .get("placement:minecraft:heightmap"),
            Some(&1),
            "and the decoration that carries it stays named too"
        );
        assert!(
            !data.step(0).expect("step 0")[0].is_modelable(),
            "so the chain is gated out of the run"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn the_offset_decoration_shifts_the_candidate_on_all_three_axes() {
        let mut target = TestTarget::new()
            .with_surface(70)
            .with_air_above(70)
            .write_anywhere();
        let written = cover_run(
            &ground_cover("minecraft:poppy"),
            vec![
                PlacementModifier::Heightmap {
                    heightmap: HeightmapType::MotionBlocking,
                },
                PlacementModifier::Offset {
                    x: RangeProvider::Constant(-3),
                    y: RangeProvider::Constant(1),
                    z: RangeProvider::Constant(5),
                },
            ],
            &mut target,
        );
        assert_eq!(written, 1);
        assert_eq!(
            target.written.keys().collect::<Vec<_>>(),
            vec![&(-3, 71, 5)],
            "the shift is added to the resolved position, negatives included"
        );
    }

    #[test]
    fn an_offset_outside_the_build_volume_drops_the_candidate() {
        // One chain, three candidate rows. The shift is applied first and the
        // build volume tested after, so a candidate that leaves the volume
        // drops instead of wrapping inside it. Every row of this fixture reads
        // as air, so only the bound can be what rejects.
        fn chain(dy: i32) -> PlacedFeature {
            PlacedFeature {
                id: "testns:cover".to_string(),
                feature: ground_cover("minecraft:poppy"),
                modifiers: vec![PlacementModifier::Offset {
                    x: RangeProvider::Constant(0),
                    y: RangeProvider::Constant(dy),
                    z: RangeProvider::Constant(0),
                }],
            }
        }
        let run = |placed: &PlacedFeature, y: i32| {
            let mut target = TestTarget::new().with_air_above(-1);
            let mut random = DecorationRandom::new(2026);
            let written = FeatureData::default().run_chain(
                &mut target,
                placed,
                0,
                DecorationPos { x: 0, y, z: 0 },
                &mut random,
            );
            (written, target.blocks("minecraft:poppy"))
        };
        let downward = chain(-1);
        let upward = chain(1);
        assert_eq!(
            run(&downward, 0),
            (0, 0),
            "a shift below the dimension floor drops the candidate"
        );
        assert_eq!(
            run(&upward, 127),
            (0, 0),
            "and so does one past the ceiling"
        );
        assert_eq!(
            run(&downward, 1),
            (1, 1),
            "the same negative shift inside the volume places"
        );
        // The drop happens after the shift drew: the bound is tested only once
        // all three axes have been sampled, so the stream a chain spends does
        // not depend on where its candidate landed.
        let dropped = PlacedFeature {
            id: "testns:cover".to_string(),
            feature: ground_cover("minecraft:poppy"),
            modifiers: vec![PlacementModifier::Offset {
                x: RangeProvider::Uniform {
                    min_inclusive: VerticalAnchor::Absolute(0),
                    max_inclusive: VerticalAnchor::Absolute(1),
                },
                y: RangeProvider::Constant(1),
                z: RangeProvider::Constant(0),
            }],
        };
        let mut probe = DecorationRandom::new(2026);
        let mut reference = DecorationRandom::new(2026);
        reference.next_int_bounded(2);
        let mut ceiling = TestTarget::new().with_air_above(-1);
        assert_eq!(
            FeatureData::default().run_chain(
                &mut ceiling,
                &dropped,
                0,
                DecorationPos { x: 0, y: 127, z: 0 },
                &mut probe,
            ),
            0
        );
        assert_eq!(
            signature(&probe),
            signature(&reference),
            "the dropped candidate still cost its one bounded draw"
        );
    }

    #[test]
    fn the_offset_provider_reads_the_shapes_the_pack_writes() {
        // 26.3 writes an offset's trapezoid as `min`/`max`/`plateau`, unlike the
        // `min_inclusive`/`max_inclusive` pair a height band uses; both a bare
        // number and a constant object are constants.
        assert_eq!(
            RangeProvider::from_offset_value(&json!(7)),
            RangeProvider::Constant(7)
        );
        assert_eq!(
            RangeProvider::from_offset_value(&json!({"type": "minecraft:constant", "value": -2})),
            RangeProvider::Constant(-2)
        );
        assert_eq!(
            RangeProvider::from_offset_value(
                &json!({"type": "minecraft:trapezoid", "min": -7, "max": 7, "plateau": 0})
            ),
            RangeProvider::Trapezoid {
                min_inclusive: VerticalAnchor::Absolute(-7),
                max_inclusive: VerticalAnchor::Absolute(7),
            }
        );
        assert_eq!(
            RangeProvider::from_offset_value(
                &json!({"type": "minecraft:uniform", "min_inclusive": -3, "max_inclusive": 3})
            ),
            RangeProvider::Uniform {
                min_inclusive: VerticalAnchor::Absolute(-3),
                max_inclusive: VerticalAnchor::Absolute(3),
            }
        );
        // Shapes with no runtime here, and an anchor that resolves against the
        // dimension rather than shifting the candidate, are refused.
        for value in [
            json!({"type": "minecraft:trapezoid", "min": -7, "max": 7, "plateau": 2}),
            json!({"type": "minecraft:clamped_normal", "deviation": 1.0,
                "min_inclusive": -3, "max_inclusive": 3, "mean": 0}),
            json!({"type": "minecraft:uniform", "min_inclusive": {"absolute": 10}}),
            json!(true),
        ] {
            let provider = RangeProvider::from_offset_value(&value);
            assert!(
                matches!(&provider, RangeProvider::Unsupported { kind } if !kind.is_empty()),
                "{value} has no offset runtime, got {provider:?}"
            );
        }
        // A shift is a shift: it never borrows the dimension bounds.
        let shift = RangeProvider::from_offset_value(
            &json!({"type": "minecraft:uniform", "min_inclusive": -3, "max_inclusive": 3}),
        );
        let mut random = DecorationRandom::new(2026);
        for _ in 0..64 {
            let drawn = shift.sample(-64, 384, &mut random);
            assert!((-3..=3).contains(&drawn), "offset drew {drawn}");
        }
    }

    #[test]
    fn the_block_predicate_filter_tests_the_block_under_its_offset() {
        let tags = air_tags();
        // One host column of grass block at row 70 in terrain that is
        // otherwise stone, so a shifted read can land on the host or miss it.
        let mut target = TestTarget::new()
            .with_base("minecraft:stone")
            .with_surface(70)
            .with_air_above(70);
        target.set_block(0, 70, 0, "minecraft:grass_block");
        let blocks = |name: &str, offset: (i32, i32, i32)| BlockPredicate::Blocks {
            blocks: vec![name.to_string()],
            offset,
        };
        let pos = DecorationPos { x: 0, y: 71, z: 0 };
        assert!(
            blocks("minecraft:grass_block", (0, -1, 0)).test(&target, pos, &tags),
            "the block one row down is the host the predicate names"
        );
        assert!(
            !blocks("minecraft:grass_block", (0, 0, 0)).test(&target, pos, &tags),
            "the candidate's own row is air, so it is not the host"
        );
        assert!(
            !blocks("minecraft:grass_block", (0, -1, 1)).test(&target, pos, &tags),
            "a neighbour column of the host is not the host"
        );
        assert!(
            !blocks("minecraft:dirt", (0, -1, 0)).test(&target, pos, &tags),
            "the same read asks a block the host is not"
        );
        // The same test through the runtime: a chain whose host is wrong places
        // nothing at all.
        let mut planted = TestTarget::new()
            .with_base("minecraft:dirt")
            .with_surface(70)
            .with_air_above(70);
        planted.set_block(0, 70, 0, "minecraft:dirt");
        let mut blocked = TestTarget::new()
            .with_base("minecraft:stone")
            .with_surface(70)
            .with_air_above(70);
        blocked.set_block(0, 70, 0, "minecraft:stone");
        let chain = |target: &mut TestTarget| {
            cover_run(
                &ground_cover("minecraft:poppy"),
                vec![
                    PlacementModifier::Heightmap {
                        heightmap: HeightmapType::MotionBlocking,
                    },
                    PlacementModifier::Offset {
                        x: RangeProvider::Constant(0),
                        y: RangeProvider::Constant(1),
                        z: RangeProvider::Constant(0),
                    },
                    PlacementModifier::BlockPredicateFilter {
                        predicate: blocks("minecraft:dirt", (0, -1, 0)),
                    },
                ],
                target,
            )
        };
        assert_eq!(chain(&mut planted), 1, "a dirt host carries the cover");
        assert_eq!(chain(&mut blocked), 0, "a stone host does not");
        assert!(
            blocked
                .written
                .values()
                .all(|state| state != "minecraft:poppy"),
            "and the rejected candidate wrote nothing anywhere"
        );
    }

    #[test]
    fn tag_and_composition_predicates_accept_and_reject_as_documented() {
        let tags = air_tags();
        let target = TestTarget::new().with_air_above(70);
        let air = DecorationPos { x: 0, y: 71, z: 0 };
        let solid = DecorationPos { x: 0, y: 70, z: 0 };
        let tag = BlockPredicate::BlockTag {
            tag: "minecraft:air".to_string(),
            offset: (0, 0, 0),
        };
        assert!(tag.test(&target, air, &tags), "air is in the air tag");
        assert!(!tag.test(&target, solid, &tags), "terrain is not");
        let unknown_tag = BlockPredicate::BlockTag {
            tag: "minecraft:not_declared".to_string(),
            offset: (0, 0, 0),
        };
        assert!(
            !unknown_tag.test(&target, air, &tags),
            "an undeclared tag has no members, so nothing matches it"
        );
        let any = BlockPredicate::AnyOf(vec![tag.clone(), unknown_tag]);
        let all = BlockPredicate::AllOf(vec![tag.clone(), tag.clone()]);
        let not = BlockPredicate::Not(Box::new(BlockPredicate::Blocks {
            blocks: vec!["minecraft:air".to_string()],
            offset: (0, 0, 0),
        }));
        assert!(any.test(&target, air, &tags));
        assert!(!any.test(&target, solid, &tags));
        assert!(all.test(&target, air, &tags));
        assert!(!all.test(&target, solid, &tags));
        assert!(!not.test(&target, air, &tags), "air is not not-air");
        assert!(not.test(&target, solid, &tags));
        // A predicate with no runtime never lets a position through, whatever
        // the terrain says, and its composition keeps the whole chain out.
        let gap = BlockPredicate::Unsupported {
            kind: "minecraft:would_survive".to_string(),
        };
        assert!(!gap.test(&target, air, &tags));
        assert!(!gap.is_modelable());
        assert!(
            !BlockPredicate::AllOf(vec![tag.clone(), gap.clone()]).is_modelable(),
            "an unsupported leaf inside a conjunction gates the conjunction"
        );
        assert!(
            !BlockPredicate::Not(Box::new(gap.clone())).is_modelable(),
            "and inside a negation"
        );
        assert!(
            !BlockPredicate::AnyOf(vec![gap]).is_modelable(),
            "and inside a disjunction"
        );
        assert!(
            tag.is_modelable() && all.is_modelable() && not.is_modelable() && any.is_modelable()
        );
    }

    #[test]
    fn simple_block_places_only_where_air_is_and_only_in_its_chunk() {
        let provider = StateProvider::Constant {
            state: "minecraft:tall_grass[half=lower]".to_string(),
        };
        let mut random = DecorationRandom::new(2026);
        // An empty row takes the state.
        let mut host = TestTarget::new().with_air_above(70);
        assert_eq!(
            place_simple_block(
                &mut host,
                &mut random,
                &provider,
                DecorationPos { x: 4, y: 71, z: 4 },
            ),
            1
        );
        assert_eq!(host.blocks("minecraft:tall_grass[half=lower]"), 1);
        // A row the terrain already fills refuses, and costs nothing.
        let mut solid = TestTarget::new().with_air_above(70);
        solid.set_block(4, 70, 4, "minecraft:stone");
        assert_eq!(
            place_simple_block(
                &mut solid,
                &mut random,
                &provider,
                DecorationPos { x: 4, y: 70, z: 4 },
            ),
            0
        );
        assert_eq!(
            solid.blocks("minecraft:stone"),
            1,
            "the host block is not repainted"
        );
        // A position outside the decorated chunk is refused before anything is
        // drawn, the same confinement the vein path keeps.
        let mut neighbour = TestTarget::new().with_air_above(70);
        let mut before = random.clone();
        assert_eq!(
            place_simple_block(
                &mut neighbour,
                &mut before,
                &provider,
                DecorationPos { x: 16, y: 71, z: 4 },
            ),
            0,
            "row 16 of chunk 0 is the neighbour's own column to paint"
        );
        assert!(neighbour.written.is_empty());
        assert_eq!(
            signature(&before),
            signature(&random),
            "a refused position spends no draw"
        );
    }

    #[test]
    fn a_weighted_state_provider_costs_one_draw_and_names_its_entries() {
        let provider = parse_state_provider(Some(&json!({
            "type": "minecraft:weighted",
            "entries": [
                {"data": "minecraft:short_grass", "weight": 3},
                {"data": {"id": "minecraft:fern", "properties": {}}, "weight": 1}
            ]
        })));
        let StateProvider::Weighted { entries } = &provider else {
            panic!("the pack shape is a weighted provider: {provider:?}");
        };
        assert_eq!(
            entries
                .iter()
                .map(|e| (e.state.as_str(), e.weight))
                .collect::<Vec<_>>(),
            vec![("minecraft:short_grass", 3), ("minecraft:fern", 1)],
            "an empty property map leaves the bare id"
        );
        // Exactly one bounded draw over the summed weights, and that draw is
        // what decides the entry.
        let mut sampled = DecorationRandom::new(2026);
        let mut reference = DecorationRandom::new(2026);
        let picked = provider.sample(&mut sampled).expect("picks");
        let control = reference.next_int_bounded(4);
        assert_eq!(
            picked,
            if control < 3 {
                "minecraft:short_grass"
            } else {
                "minecraft:fern"
            },
            "the weights decide which entry the one draw picks"
        );
        assert_eq!(
            signature(&sampled),
            signature(&reference),
            "the weighted pick costs one bounded draw and no more"
        );
        // Both entries are reachable across seeds, and nothing else is ever.
        let mut grass = 0;
        let mut fern = 0;
        for seed in 0..256i64 {
            let mut random = DecorationRandom::new(seed);
            match provider.sample(&mut random) {
                Some("minecraft:short_grass") => grass += 1,
                Some("minecraft:fern") => fern += 1,
                other => panic!("unexpected pick {other:?}"),
            }
        }
        assert_eq!(grass + fern, 256, "only the two entries are ever picked");
        assert!(
            fern > 0 && grass > 0,
            "both entries must be reachable: {grass} grass / {fern} fern"
        );
        // A provider whose entries cannot all be named is refused rather than
        // partially read, because a dropped entry would move the pick.
        for value in [
            json!({"type": "minecraft:weighted", "entries": []}),
            json!({"type": "minecraft:weighted", "entries": [
                {"data": "minecraft:air", "weight": 0}]}),
            json!({"type": "minecraft:weighted", "entries": [
                {"data": {"id": "minecraft:x", "properties": {"n": 1}}, "weight": 1}]}),
            json!({"type": "minecraft:weighted"}),
        ] {
            assert!(
                !parse_state_provider(Some(&value)).is_modelable(),
                "{value} must not read as a placeable state"
            );
        }
    }

    #[test]
    fn the_state_provider_names_the_pack_shapes() {
        // The three shapes the 26.3 features write directly.
        assert_eq!(
            parse_state_provider(Some(&json!("minecraft:poppy"))),
            StateProvider::Unsupported {
                kind: "minecraft:block_state_provider_reference".to_string()
            },
            "a bare string at this position is the pack's registry reference: \
             `minecraft:flower_plain` and friends are documents under \
             `worldgen/block_state_provider`, not blocks"
        );
        assert_eq!(
            parse_state_provider(Some(&json!({"id": "minecraft:tall_grass",
                "properties": {"half": "lower", "waterlogged": "false"}}))),
            StateProvider::Constant {
                state: "minecraft:tall_grass[half=lower,waterlogged=false]".to_string()
            },
            "properties render sorted, so the name does not track the document order"
        );
        assert_eq!(
            parse_state_provider(Some(&json!({"id": "minecraft:brown_mushroom"}))),
            StateProvider::Constant {
                state: "minecraft:brown_mushroom".to_string()
            }
        );
        // Everything else is a gap with a name, never a silent state.
        assert_eq!(
            parse_state_provider(Some(&json!({
                "type": "minecraft:randomized_int", "property": "pickles",
                "source": {"id": "minecraft:sea_pickle"},
                "values": {"type": "minecraft:uniform", "min_inclusive": 1, "max_inclusive": 4}}))),
            StateProvider::Unsupported {
                kind: "minecraft:randomized_int".to_string()
            }
        );
        assert_eq!(
            parse_state_provider(Some(&json!("#minecraft:small_flowers"))),
            StateProvider::Unsupported {
                kind: "minecraft:block_tag".to_string()
            }
        );
        assert!(!parse_state_provider(None).is_modelable());
        // And an unmodelable provider is not a supported configured feature.
        assert!(
            !ConfiguredFeature::SimpleBlock {
                to_place: parse_state_provider(Some(&json!("minecraft:flower_plain")))
            }
            .is_supported()
        );
        assert!(
            ConfiguredFeature::SimpleBlock {
                to_place: parse_state_provider(Some(&json!({"id": "minecraft:poppy"})))
            }
            .is_supported()
        );
    }

    #[test]
    fn a_modeled_ground_cover_chain_leaves_the_gap_report_alone() {
        // The chain `minecraft:patch_tall_grass` writes, reduced to one of each
        // shape. Before this slice every one of these three decorations was a
        // gap and the configured type was one; now none of them is.
        let root = report_pack(
            "report-cover-clean",
            r#"[{"type": "minecraft:count", "count": 4},
                {"type": "minecraft:rarity_filter", "chance": 5},
                {"type": "minecraft:in_square"},
                {"type": "minecraft:heightmap", "heightmap": "MOTION_BLOCKING"},
                {"type": "minecraft:biome"},
                {"type": "minecraft:offset",
                    "x": {"type": "minecraft:trapezoid", "min": -7, "max": 7, "plateau": 0},
                    "y": {"type": "minecraft:trapezoid", "min": -3, "max": 3, "plateau": 0},
                    "z": {"type": "minecraft:trapezoid", "min": -7, "max": 7, "plateau": 0}},
                {"type": "minecraft:block_predicate_filter",
                    "predicate": {"type": "minecraft:matching_block_tag", "tag": "minecraft:air"}}]"#,
        );
        let data = FeatureData::load(&root).expect("loads");
        assert!(
            data.unimplemented_features.is_empty(),
            "a ground-cover chain this runtime runs is not a gap: {:?}",
            data.unimplemented_features
        );
        assert!(data.step(0).expect("step 0")[0].is_modelable());
        fs::remove_dir_all(root).expect("cleanup");
        // The same shapes written for an ore keep the report empty too: the
        // configured type is what changed, not the decorations' own keys.
        let cover = report_pack(
            "report-cover-clean2",
            r#"[{"type": "minecraft:heightmap", "heightmap": "WORLD_SURFACE_WG"}]"#,
        );
        let data = FeatureData::load(&cover).expect("loads");
        assert!(
            data.unimplemented_features.is_empty(),
            "{:?}",
            data.unimplemented_features
        );
        fs::remove_dir_all(cover).expect("cleanup");
    }

    #[test]
    fn unmodeled_ground_cover_parameters_are_named_individually() {
        // A predicate type the pack invented, an offset axis with no runtime,
        // and a state provider this generator does not resolve: each keeps its
        // own key so the report says what is missing, and none of them can
        // place a block.
        let root = scratch_root("report-cover-gaps");
        let worldgen = root.join("data/testns/worldgen");
        write(
            &worldgen.join("feature/cover.json"),
            r#"{"type": "minecraft:simple_block", "to_place": {"type": "minecraft:randomized_int",
                "property": "pickles", "source": {"id": "minecraft:sea_pickle"},
                "values": {"type": "minecraft:uniform", "min_inclusive": 1, "max_inclusive": 4}}}"#,
        );
        write(
            &worldgen.join("placed_feature/one.json"),
            r#"{"feature": "testns:cover", "placement": [
                {"type": "minecraft:heightmap", "heightmap": "OCEAN_FLOOR_WG"},
                {"type": "minecraft:offset", "x": {"type": "minecraft:clamped_normal",
                    "deviation": 1.0, "min_inclusive": -3, "max_inclusive": 3, "mean": 0},
                    "y": 1, "z": 0},
                {"type": "minecraft:block_predicate_filter", "predicate": {"type": "minecraft:all_of",
                    "predicates": [
                        {"type": "minecraft:matching_block_tag", "tag": "minecraft:air"},
                        {"type": "minecraft:would_survive", "state": {"id": "minecraft:grass_block"}}]}}
            ]}"#,
        );
        write(
            &worldgen.join("biome/plains.json"),
            r#"{"features": [["testns:one"]]}"#,
        );
        let data = FeatureData::load(&root).expect("loads");
        for key in [
            "provider:minecraft:randomized_int",
            "placement:minecraft:offset",
            "offset:minecraft:clamped_normal",
            "placement:minecraft:block_predicate_filter",
            "predicate:minecraft:would_survive",
        ] {
            assert_eq!(
                data.unimplemented_features.get(key),
                Some(&1),
                "{key} stays a named gap: {:?}",
                data.unimplemented_features
            );
        }
        assert!(
            !data
                .unimplemented_features
                .contains_key("minecraft:simple_block"),
            "the configured type is modeled now: {:?}",
            data.unimplemented_features
        );
        assert!(
            !data
                .unimplemented_features
                .contains_key("heightmap:OCEAN_FLOOR_WG"),
            "a map this runtime resolves adds nothing"
        );
        assert!(
            !data
                .unimplemented_features
                .contains_key("placement:minecraft:heightmap"),
            "and neither does its decoration"
        );
        assert!(
            !data.step(0).expect("step 0")[0].is_modelable(),
            "one unmodeled parameter gates the whole chain"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// Requires the operator-provisioned data pack. Measures the ground-cover
    /// unlock this slice was for, over the pack's own documents rather than any
    /// fixture: how many placed features target a `minecraft:simple_block`
    /// configured feature, how many of those now answer `is_modelable`, and
    /// which gap keys the run lost or kept. Also prints the provider and
    /// predicate keys, which are the residual gaps. Run with
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib --
    /// --ignored --nocapture`.
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn smoke_measures_the_ground_cover_unlock() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
        let data = FeatureData::load(&root).expect("the provisioned pack loads");
        let mut targeting = 0usize;
        let mut modelable = 0usize;
        let mut supported_configured = 0usize;
        let mut simple_block_docs = 0usize;
        for feature in data.configured.values() {
            if matches!(&**feature, ConfiguredFeature::SimpleBlock { .. }) {
                simple_block_docs += 1;
                if feature.is_supported() {
                    supported_configured += 1;
                }
            }
        }
        let mut gated: Vec<(String, Vec<String>)> = Vec::new();
        let mut decorations_modeled = 0usize;
        for placed in data.placed.values() {
            if matches!(&*placed.feature, ConfiguredFeature::SimpleBlock { .. }) {
                targeting += 1;
                if placed.modifiers.iter().all(modifier_is_modelable) {
                    decorations_modeled += 1;
                }
                if placed.is_modelable() {
                    modelable += 1;
                    continue;
                }
                let names = placed
                    .modifiers
                    .iter()
                    .filter(|modifier| !modifier_is_modelable(modifier))
                    .map(|modifier| match modifier {
                        PlacementModifier::Unsupported { kind } => kind.clone(),
                        PlacementModifier::Heightmap { heightmap } => {
                            format!("heightmap({heightmap:?})")
                        }
                        PlacementModifier::Offset { x, y, z } => {
                            format!("offset({x:?},{y:?},{z:?})")
                        }
                        PlacementModifier::BlockPredicateFilter { predicate } => {
                            format!("predicate({predicate:?})")
                        }
                        other => format!("{other:?}"),
                    })
                    .collect::<Vec<_>>();
                // A chain whose decorations are all modeled but which is still
                // gated is gated by its own state provider.
                let names = if names.is_empty() {
                    vec!["state provider".to_string()]
                } else {
                    names
                };
                gated.push((placed.id.clone(), names));
            }
        }
        let key = |name: &str| data.unimplemented_features.get(name).copied().unwrap_or(0);
        println!(
            "ground cover: {} simple_block configured documents, {} with a modeled state provider; \
             {} placed features target them, {} with every decoration modeled, {} modelable overall",
            simple_block_docs, supported_configured, targeting, decorations_modeled, modelable
        );
        println!(
            "gap keys (placement:heightmap, placement:offset, placement:block_predicate_filter, \
             minecraft:simple_block) = {}, {}, {}, {}",
            key("placement:minecraft:heightmap"),
            key("placement:minecraft:offset"),
            key("placement:minecraft:block_predicate_filter"),
            key("minecraft:simple_block"),
        );
        if !gated.is_empty() {
            println!("  -- chains still gated --");
            let mut gated = gated;
            gated.sort();
            for (id, names) in &gated {
                println!("  {id} gated by {}", names.join(", "));
            }
        }
        let mut entries: Vec<(&String, &usize)> = data.unimplemented_features.iter().collect();
        entries.sort_unstable_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));
        println!("  -- ground-cover keys still open --");
        for (key, count) in &entries {
            let inner = key.split_once(':').unwrap_or_default().1;
            if key.starts_with("placement:")
                || key.starts_with("provider:")
                || key.starts_with("predicate:")
                || key.starts_with("offset:")
                || key.starts_with("range:")
                || key.starts_with("heightmap:")
                || inner == "minecraft:simple_block"
            {
                println!("  {key} {count}");
            }
        }
        assert_eq!(
            targeting, 64,
            "the pack's measured simple_block chain census"
        );
        assert_eq!(
            supported_configured, 33,
            "37 configured documents less the three registry references and \
             the one randomized-int provider"
        );
        assert_eq!(
            decorations_modeled, 50,
            "the three decorations plus the configured type were to unlock most of \
             the {targeting} chains, got {decorations_modeled} with every decoration modeled"
        );
        assert_eq!(
            modelable, 46,
            "of those, {modelable} are modelable overall: the difference is the chains whose \
             state provider this runtime still cannot resolve"
        );
        assert_eq!(
            key("placement:minecraft:heightmap"),
            0,
            "every heightmap name the pack writes now has a runtime"
        );
        assert_eq!(
            key("minecraft:simple_block"),
            0,
            "and the configured type is modeled"
        );
    }

    /// Requires the operator-provisioned data pack. Prints the gap ledger the
    /// loader builds from it — the honest gap list, per key and per count;
    /// this smoke, not the server, is what surfaces it today.
    /// `FeatureData::load` has no dimension filter (the overworld
    /// biome order only shapes the schedule), so the report covers every
    /// placed and configured document in the pack. Run with
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib --
    /// --ignored --nocapture`.
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn smoke_prints_the_pack_wide_gap_report() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
        let data = FeatureData::load(&root).expect("the provisioned pack loads");
        assert!(
            !data.unimplemented_features.is_empty(),
            "the provisioned pack asks for nothing this generator lacks"
        );
        let mut entries: Vec<(&String, &usize)> = data.unimplemented_features.iter().collect();
        entries.sort_unstable_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));
        let placements = entries
            .iter()
            .filter(|(key, _)| key.starts_with("placement:"))
            .count();
        println!(
            "gap report: {} keys ({} of them placement modifiers), root {}",
            entries.len(),
            placements,
            root.display()
        );
        for (key, count) in entries {
            println!("  {key} {count}");
        }
    }
}
