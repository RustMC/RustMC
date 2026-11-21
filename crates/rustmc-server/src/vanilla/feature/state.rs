//! Block state providers, block predicates, fluid and solidity facts,
//! and heightmap kinds used by feature placement.

use super::*;

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
    /// `minecraft:rule_based` — the first rule whose predicate holds at the
    /// position supplies the state, and the fallback answers when none does.
    /// The pack's sand and grass disks pick their state this way.
    RuleBased {
        fallback: Box<StateProvider>,
        rules: Vec<RuleBasedEntry>,
    },
    /// `minecraft:randomized_int` — samples `source` and overrides `property`
    /// with an integer drawn from `values`.
    RandomizedInt {
        source: Box<StateProvider>,
        property: String,
        values: RangeProvider,
    },
    /// `minecraft:rotated` — sets or randomly rotates `axis` (`x`, `y`, `z`) or
    /// `facing` (`north`, `east`, `south`, `west`) on a block state.
    ///
    /// Source: minecraft.wiki `Block state provider` (revision 3828513):
    /// "Randomly rotates any block with `axis` or `facing` properties."
    Rotated {
        state: Box<StateProvider>,
        direction: Option<Direction>,
    },
    /// Named reference into `worldgen/block_state_provider/*.json`. Resolved
    /// during [`FeatureData::load`] when the document exists in the pack;
    /// unresolved references remain named gaps.
    Reference { id: String },
    /// A provider shape without a runtime here. Named so the gap report can
    /// tell an unmodeled state apart from a feature that placed nothing.
    Unsupported { kind: String },
}

/// One rule of a `minecraft:rule_based` block-state provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleBasedEntry {
    pub if_true: BlockPredicate,
    pub then: StateProvider,
}

impl StateProvider {
    /// Whether a provider shape, and every provider and predicate nested in
    /// it, has a runtime here.
    pub fn is_modelable(&self) -> bool {
        match self {
            Self::Constant { .. } | Self::Weighted { .. } => true,
            Self::RuleBased { fallback, rules } => {
                fallback.is_modelable()
                    && rules
                        .iter()
                        .all(|rule| rule.if_true.is_modelable() && rule.then.is_modelable())
            }
            Self::RandomizedInt {
                source,
                property,
                values,
            } => !property.is_empty() && source.is_modelable() && values.is_modelable(),
            Self::Rotated { state, .. } => state.is_modelable(),
            Self::Reference { .. } | Self::Unsupported { .. } => false,
        }
    }

    /// Whether sampling this provider consumes no decoration RNG draws.
    pub(super) fn costs_no_draws(&self) -> bool {
        match self {
            Self::Constant { .. } => true,
            Self::RuleBased { fallback, rules } => {
                fallback.costs_no_draws() && rules.iter().all(|rule| rule.then.costs_no_draws())
            }
            _ => false,
        }
    }

    /// Every state this provider can hand out, whichever rule or entry
    /// picks it.
    pub(super) fn states(&self) -> Vec<&str> {
        match self {
            Self::Constant { state } => vec![state.as_str()],
            Self::Weighted { entries } => {
                entries.iter().map(|entry| entry.state.as_str()).collect()
            }
            Self::RuleBased { fallback, rules } => rules
                .iter()
                .flat_map(|rule| rule.then.states())
                .chain(fallback.states())
                .collect(),
            Self::RandomizedInt { source, .. } | Self::Rotated { state: source, .. } => {
                source.states()
            }
            Self::Reference { .. } | Self::Unsupported { .. } => Vec::new(),
        }
    }

    /// The state to place at `pos`, drawn from the decoration stream. A
    /// constant costs nothing and a weighted pick costs one bounded draw,
    /// matching the draw-count discipline the count and height providers
    /// keep. A rule-based pick reads the terrain around `pos`, which costs no
    /// draw, and then samples the provider the first passing rule names.
    pub fn sample(
        &self,
        target: &dyn DecorationTarget,
        pos: DecorationPos,
        tags: &BlockTags,
        random: &mut DecorationRandom,
    ) -> Option<Cow<'_, str>> {
        match self {
            Self::Constant { state } => Some(Cow::Borrowed(state.as_str())),
            Self::Weighted { entries } => {
                let total: i32 = entries.iter().map(|entry| entry.weight).sum();
                if total <= 0 {
                    return None;
                }
                let mut pick = random.next_int_bounded(total);
                for entry in entries {
                    if pick < entry.weight {
                        return Some(Cow::Borrowed(entry.state.as_str()));
                    }
                    pick -= entry.weight;
                }
                None
            }
            Self::RuleBased { fallback, rules } => rules
                .iter()
                .find(|rule| rule.if_true.test(target, pos, tags))
                .map_or(fallback.as_ref(), |rule| &rule.then)
                .sample(target, pos, tags, random),
            Self::RandomizedInt {
                source,
                property,
                values,
            } => {
                let base = source.sample(target, pos, tags, random)?;
                let value = values.sample(target.min_y(), target.height(), random);
                Some(Cow::Owned(set_state_property(&base, property, value)))
            }
            Self::Rotated { state, direction } => {
                let base = state.sample(target, pos, tags, random)?;
                if block_state_property(&base, "axis").is_some() {
                    let axis = match direction {
                        Some(Direction::East | Direction::West) => "x",
                        Some(Direction::Up | Direction::Down) => "y",
                        Some(Direction::North | Direction::South) => "z",
                        None => match random.next_int_bounded(3) {
                            0 => "x",
                            1 => "y",
                            _ => "z",
                        },
                    };
                    Some(Cow::Owned(set_state_property_str(&base, "axis", axis)))
                } else if block_state_property(&base, "facing").is_some() {
                    let facing = match direction {
                        Some(Direction::North) => "north",
                        Some(Direction::East) => "east",
                        Some(Direction::South) => "south",
                        Some(Direction::West) => "west",
                        Some(_) | None => match random.next_int_bounded(4) {
                            0 => "north",
                            1 => "east",
                            2 => "south",
                            _ => "west",
                        },
                    };
                    Some(Cow::Owned(set_state_property_str(&base, "facing", facing)))
                } else {
                    Some(base)
                }
            }
            // `is_modelable` gates this out before the feature runs.
            Self::Reference { .. } | Self::Unsupported { .. } => None,
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
    /// `minecraft:matching_fluids` — the fluid carried by the block state at
    /// `pos + offset` matches one of `fluids` (or a `#` fluid tag).
    MatchingFluids {
        fluids: Vec<String>,
        offset: (i32, i32, i32),
    },
    /// `minecraft:inside_world_bounds` — tests whether `pos + offset` lies within
    /// the buildable vertical range `[min_y, min_y + height)`.
    InsideWorldBounds { offset: (i32, i32, i32) },
    /// `minecraft:replaceable` — checks whether the block at `pos + offset`
    /// belongs to `#minecraft:replaceable`.
    ///
    /// Source: minecraft.wiki `Block predicate` (revision 3840614):
    /// "Checks if the block at the specified offset can be replaced by placing blocks."
    Replaceable { offset: (i32, i32, i32) },
    /// `minecraft:would_survive` — checks whether `state` is a valid placement
    /// at `pos + offset`.
    ///
    /// Source: minecraft.wiki `Block predicate` (revision 3840614):
    /// "Checks if a specified block state is a valid placement at the given offset."
    WouldSurvive {
        offset: (i32, i32, i32),
        state: String,
    },
    /// `minecraft:true` — always succeeds.
    True,
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
            Self::WouldSurvive { state, .. } => plant_support::supports_survival_state(state),
            Self::AnyOf(rules) | Self::AllOf(rules) => rules.iter().all(Self::is_modelable),
            Self::Not(rule) => rule.is_modelable(),
            _ => true,
        }
    }

    /// Evaluates the predicate against the terrain around one candidate
    /// position. A read that leaves the decorated chunk goes through
    /// [`DecorationTarget::read_block`], so it sees the neighbouring terrain
    /// as the fill and carver passes left it. Neighbour decorations are not
    /// visible there: this pass has not replayed them.
    pub fn test(
        &self,
        target: &dyn DecorationTarget,
        pos: DecorationPos,
        tags: &BlockTags,
    ) -> bool {
        match self {
            Self::BlockTag { tag, offset } => {
                let px = pos.x + offset.0;
                let py = pos.y + offset.1;
                let pz = pos.z + offset.2;
                if tag == "minecraft:air"
                    && tags.contains("minecraft:air", "minecraft:air")
                    && tags.contains("minecraft:air", "minecraft:cave_air")
                    && tags.contains("minecraft:air", "minecraft:void_air")
                {
                    py >= target.min_y()
                        && py < target.min_y() + target.height()
                        && target.is_air(px, py, pz)
                } else {
                    target
                        .read_block(px, py, pz)
                        .is_some_and(|block| tags.contains(tag, base_block_id(&block)))
                }
            }
            Self::Blocks { blocks, offset } => target
                .read_block(pos.x + offset.0, pos.y + offset.1, pos.z + offset.2)
                .is_some_and(|block| {
                    let block = base_block_id(&block);
                    blocks
                        .iter()
                        .any(|candidate| base_block_id(candidate) == block)
                }),
            Self::MatchingFluids { fluids, offset } => target
                .read_block(pos.x + offset.0, pos.y + offset.1, pos.z + offset.2)
                .is_some_and(|block| {
                    let fluid = fluid_from_block_state(&block);
                    let base = base_block_id(&block);
                    fluids.iter().any(|candidate| {
                        if let Some(tag) = candidate.strip_prefix('#') {
                            tags.contains(tag, fluid) || tags.contains(tag, base)
                        } else {
                            candidate == fluid
                        }
                    })
                }),
            Self::InsideWorldBounds { offset } => {
                let ty = pos.y + offset.1;
                ty >= target.min_y() && ty < target.min_y() + target.height()
            }
            Self::Replaceable { offset } => target
                .read_block(pos.x + offset.0, pos.y + offset.1, pos.z + offset.2)
                .is_some_and(|block| is_replaceable_block(base_block_id(&block), tags)),
            Self::WouldSurvive { offset, state } => {
                let checked = DecorationPos {
                    x: pos.x + offset.0,
                    y: pos.y + offset.1,
                    z: pos.z + offset.2,
                };
                plant_support::would_survive(target, tags, state, checked)
            }
            Self::True => true,
            Self::AnyOf(rules) => rules.iter().any(|rule| rule.test(target, pos, tags)),
            Self::AllOf(rules) => rules.iter().all(|rule| rule.test(target, pos, tags)),
            Self::Not(rule) => !rule.test(target, pos, tags),
            Self::Unsupported { .. } => false,
        }
    }
}

impl StateProvider {
    /// Whether every state this provider can hand out has a ground rule.
    pub(super) fn is_placeable(&self) -> bool {
        self.is_modelable() && self.states().into_iter().all(plant_support::is_placeable)
    }
}

/// Extracts one property value from a rendered block state string `id[k1=v1,k2=v2]`.
pub(super) fn block_state_property<'a>(state: &'a str, key: &str) -> Option<&'a str> {
    let (_, rest) = state.split_once('[')?;
    let props = rest.strip_suffix(']').unwrap_or(rest);
    for pair in props.split(',') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        if k.trim() == key {
            return Some(v.trim());
        }
    }
    None
}

/// Blocks that always contain a water source block without a `waterlogged` property (minecraft.wiki `Waterlogging`).
pub(super) const ALWAYS_WATER_SOURCE_BLOCKS: &[&str] = &[
    "minecraft:seagrass",
    "minecraft:tall_seagrass",
    "minecraft:kelp",
    "minecraft:kelp_plant",
    "minecraft:bubble_column",
];

/// Derives the fluid identifier (`minecraft:fluid` registry) carried by a block
/// state from its `waterlogged` / `level` state properties (from 26.3 `blocks.json`),
/// always-waterlogged vanilla blocks, and fluid block identity (`minecraft:water` / `minecraft:lava`).
pub(crate) fn fluid_from_block_state(state: &str) -> &'static str {
    if block_state_property(state, "waterlogged") == Some("true") {
        return "minecraft:water";
    }
    let base = base_block_id(state);
    if ALWAYS_WATER_SOURCE_BLOCKS.contains(&base) {
        return "minecraft:water";
    }
    match base {
        "minecraft:water" => {
            if block_state_property(state, "level").is_some_and(|lvl| lvl != "0") {
                "minecraft:flowing_water"
            } else {
                "minecraft:water"
            }
        }
        "minecraft:lava" => {
            if block_state_property(state, "level").is_some_and(|lvl| lvl != "0") {
                "minecraft:flowing_lava"
            } else {
                "minecraft:lava"
            }
        }
        _ => "minecraft:empty",
    }
}

/// Blocks marked legacy solid despite thin/empty collision bounds (`Block properties/Solid (legacy)`,
/// rev 3717400).
pub(super) const LEGACY_SOLID_OVERRIDES: &[&str] = &[
    "minecraft:cobweb",
    "minecraft:moving_piston",
    "minecraft:cake",
    "minecraft:bamboo_sapling",
    "minecraft:bamboo",
    "minecraft:amethyst_cluster",
    "minecraft:sculk_vein",
    "minecraft:pointed_dripstone",
    "minecraft:firefly_bush",
    "minecraft:conduit",
    "minecraft:turtle_egg",
    "minecraft:dried_ghast",
    "minecraft:bell",
    "minecraft:lantern",
    "minecraft:soul_lantern",
    "minecraft:lightning_rod",
];

/// Blocks excluded from legacy solid (`Block properties/Solid (legacy)`, rev 3717400).
pub(super) const LEGACY_NON_SOLID_OVERRIDES: &[&str] = &[
    "minecraft:ladder",
    "minecraft:snow",
    "minecraft:end_rod",
    "minecraft:chorus_plant",
    "minecraft:chorus_flower",
    "minecraft:azalea",
    "minecraft:flowering_azalea",
    "minecraft:big_dripleaf",
];

/// Whether `base_id` has the legacy solid property (`Block properties/Solid (legacy)` rev 3717400),
/// using `#minecraft:blocks_motion` when loaded or a non-fluid/non-plant fallback in synthetic tests.
pub(crate) fn is_legacy_solid(base_id: &str, tags: &BlockTags) -> bool {
    if is_air_block(Some(base_id)) || LEGACY_NON_SOLID_OVERRIDES.contains(&base_id) {
        return false;
    }
    if LEGACY_SOLID_OVERRIDES.contains(&base_id) {
        return true;
    }
    if tags.members("minecraft:blocks_motion").is_some() {
        return tags.contains("minecraft:blocks_motion", base_id);
    }
    !matches!(
        base_id,
        "minecraft:water"
            | "minecraft:flowing_water"
            | "minecraft:lava"
            | "minecraft:flowing_lava"
            | "minecraft:short_grass"
            | "minecraft:tall_grass"
            | "minecraft:fern"
            | "minecraft:large_fern"
            | "minecraft:dead_bush"
            | "minecraft:bush"
            | "minecraft:short_dry_grass"
            | "minecraft:tall_dry_grass"
            | "minecraft:sugar_cane"
            | "minecraft:dandelion"
            | "minecraft:poppy"
            | "minecraft:blue_orchid"
            | "minecraft:allium"
            | "minecraft:azure_bluet"
            | "minecraft:red_tulip"
            | "minecraft:orange_tulip"
            | "minecraft:white_tulip"
            | "minecraft:pink_tulip"
            | "minecraft:oxeye_daisy"
            | "minecraft:cornflower"
            | "minecraft:lily_of_the_valley"
            | "minecraft:sunflower"
            | "minecraft:lilac"
            | "minecraft:rose_bush"
            | "minecraft:peony"
            | "minecraft:wildflowers"
            | "minecraft:pink_petals"
            | "minecraft:leaf_litter"
            | "minecraft:small_dripleaf"
            | "minecraft:moss_carpet"
            | "minecraft:pale_moss_carpet"
            | "minecraft:lily_pad"
            | "minecraft:vine"
            | "minecraft:glow_lichen"
            | "minecraft:seagrass"
            | "minecraft:tall_seagrass"
            | "minecraft:kelp"
            | "minecraft:kelp_plant"
    )
}

pub(super) fn is_replaceable_block(base_id: &str, tags: &BlockTags) -> bool {
    if tags.members("minecraft:replaceable").is_some() {
        tags.contains("minecraft:replaceable", base_id)
    } else {
        matches!(
            base_id,
            "minecraft:air"
                | "minecraft:cave_air"
                | "minecraft:void_air"
                | "minecraft:water"
                | "minecraft:lava"
                | "minecraft:short_grass"
                | "minecraft:fern"
                | "minecraft:dead_bush"
                | "minecraft:bush"
                | "minecraft:short_dry_grass"
                | "minecraft:tall_dry_grass"
                | "minecraft:seagrass"
                | "minecraft:tall_seagrass"
                | "minecraft:tall_grass"
                | "minecraft:large_fern"
                | "minecraft:snow"
                | "minecraft:vine"
                | "minecraft:glow_lichen"
                | "minecraft:leaf_litter"
        )
    }
}

/// Heightmap variants defined by `Placed feature` (rev 3816343) and
/// `Heightmap` (rev 3806795).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeightmapKind {
    WorldSurfaceWg,
    WorldSurface,
    OceanFloorWg,
    OceanFloor,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

impl HeightmapKind {
    /// Parses a vanilla heightmap identifier.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "WORLD_SURFACE_WG" => Some(Self::WorldSurfaceWg),
            "WORLD_SURFACE" => Some(Self::WorldSurface),
            "OCEAN_FLOOR_WG" => Some(Self::OceanFloorWg),
            "OCEAN_FLOOR" => Some(Self::OceanFloor),
            "MOTION_BLOCKING" => Some(Self::MotionBlocking),
            "MOTION_BLOCKING_NO_LEAVES" => Some(Self::MotionBlockingNoLeaves),
            _ => None,
        }
    }

    /// Returns whether `state` counts as a blocking/surface block under this heightmap
    /// per `Heightmap` (rev 3806795) and 26.3 heightmap tags.
    pub fn matches_block(self, state: &str, tags: &BlockTags) -> bool {
        if is_air_block(Some(state)) {
            return false;
        }
        match self {
            Self::WorldSurfaceWg | Self::WorldSurface => true,
            Self::OceanFloorWg | Self::OceanFloor => heightmap_blocks_motion(state, tags, false),
            Self::MotionBlocking => {
                heightmap_blocks_motion(state, tags, false)
                    || fluid_from_block_state(state) != "minecraft:empty"
            }
            Self::MotionBlockingNoLeaves => {
                heightmap_blocks_motion(state, tags, true)
                    || fluid_from_block_state(state) != "minecraft:empty"
            }
        }
    }
}

pub(super) fn heightmap_blocks_motion(state: &str, tags: &BlockTags, exclude_leaves: bool) -> bool {
    let base = base_block_id(state);
    let tag = if exclude_leaves {
        "minecraft:blocks_motion_in_heightmap_no_leaves"
    } else {
        "minecraft:blocks_motion_in_heightmap"
    };
    if tags.members(tag).is_some() {
        return tags.contains(tag, base);
    }
    if exclude_leaves && base.ends_with("_leaves") {
        return false;
    }
    is_legacy_solid(base, tags)
}

/// Strips a state's properties, leaving its block id.
pub(crate) fn base_block_id(state: &str) -> &str {
    state.split_once('[').map_or(state, |(base, _)| base)
}
