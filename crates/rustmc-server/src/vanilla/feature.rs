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

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::sync::OnceLock;

use serde_json::Value;

use super::plant_support;

use crate::vanilla::random::DecorationRandom;
use crate::vanilla::worldgen::{WorldgenError, read_dir_optional, read_json, stem, walk_json};
use crate::world::Direction;

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

fn load_registry(
    root: &Path,
    registry: &str,
) -> Result<HashMap<String, Vec<String>>, WorldgenError> {
    load_registry_from_roots(&[root], registry)
}

/// Loads `data/<namespace>/tags/<registry>/**/*.json` across ordered pack
/// roots, applying `"replace": true` before resolving nested `#tag` references.
fn load_registry_from_roots(
    roots: &[&Path],
    registry: &str,
) -> Result<HashMap<String, Vec<String>>, WorldgenError> {
    let mut raw: HashMap<String, Vec<Value>> = HashMap::new();
    for root in roots {
        let data_dir = root.join("data");
        let Some(namespaces) = read_dir_optional(&data_dir)? else {
            continue;
        };
        let mut ns_dirs = Vec::new();
        for namespace in namespaces {
            let namespace =
                namespace.map_err(|error| WorldgenError::Io(data_dir.clone(), error))?;
            if namespace
                .file_type()
                .map_err(|error| WorldgenError::Io(namespace.path(), error))?
                .is_dir()
            {
                ns_dirs.push(namespace.path());
            }
        }
        ns_dirs.sort();
        for ns_path in ns_dirs {
            let ns = ns_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let category = ns_path.join("tags").join(registry);
            for entry in walk_json(&category)? {
                let document = read_json(&entry)?;
                let replace = match document.get("replace") {
                    None => false,
                    Some(value) => value.as_bool().ok_or_else(|| {
                        WorldgenError::Invalid(format!(
                            "{}: tag field 'replace' must be a boolean",
                            entry.display()
                        ))
                    })?,
                };
                let values = match document.get("values") {
                    None => Vec::new(),
                    Some(value) => value
                        .as_array()
                        .ok_or_else(|| {
                            WorldgenError::Invalid(format!(
                                "{}: tag field 'values' must be an array",
                                entry.display()
                            ))
                        })?
                        .clone(),
                };
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
                let slot = raw.entry(id).or_default();
                if replace {
                    slot.clear();
                }
                slot.extend(values);
            }
        }
    }
    // Entries may name another tag; resolve those references into a
    // flat member list. A reference cycle stops at the first repeat
    // instead of recursing forever.
    let mut entries: HashMap<String, Vec<String>> = HashMap::new();
    let ids: Vec<String> = raw.keys().cloned().collect();
    for id in ids {
        let mut members: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        seen.insert(id.clone());
        let mut queue: VecDeque<Value> = raw.get(&id).cloned().unwrap_or_default().into();
        while let Some(value) = queue.pop_front() {
            let (name, reference, required) = match &value {
                Value::String(value) => match value.strip_prefix('#') {
                    Some(tag) => (None, Some(tag), true),
                    None => (Some(value.as_str()), None, true),
                },
                Value::Object(object) => {
                    let required = object
                        .get("required")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    if let Some(tag) = object.get("tag").and_then(Value::as_str) {
                        (None, Some(tag.trim_start_matches('#')), required)
                    } else if let Some(value) = object.get("id").and_then(Value::as_str) {
                        match value.strip_prefix('#') {
                            Some(tag) => (None, Some(tag), required),
                            None => (Some(value), None, required),
                        }
                    } else {
                        continue;
                    }
                }
                _ => continue,
            };
            if let Some(name) = name.filter(|name| !name.is_empty())
                && !members.iter().any(|member| member == name)
            {
                members.push(name.to_string());
            }
            if let Some(tag) = reference {
                match raw.get(tag) {
                    Some(values) if seen.insert(tag.to_string()) => {
                        queue.extend(values.iter().cloned());
                    }
                    Some(_) => {}
                    None if required => {
                        return Err(WorldgenError::Invalid(format!(
                            "{registry} tag {id} references missing required tag {tag}"
                        )));
                    }
                    None => {}
                }
            }
        }
        entries.insert(id, members);
    }
    Ok(entries)
}

fn tag_contains(entries: &HashMap<String, Vec<String>>, tag: &str, id: &str) -> bool {
    entries
        .get(tag)
        .is_some_and(|members| members.iter().any(|member| member == id))
}

fn tag_members<'a>(entries: &'a HashMap<String, Vec<String>>, tag: &str) -> Option<&'a [String]> {
    entries.get(tag).map(Vec::as_slice)
}

/// The block tags of the provisioned pack, keyed by namespaced id.
#[derive(Debug, Default, Clone)]
pub struct BlockTags {
    entries: HashMap<String, Vec<String>>,
    /// Precomputed set membership for O(1) lookup when loaded from pack data.
    membership: HashMap<String, HashSet<String>>,
}

fn build_tag_membership(
    entries: &HashMap<String, Vec<String>>,
) -> HashMap<String, HashSet<String>> {
    entries
        .iter()
        .map(|(tag, members)| (tag.clone(), members.iter().cloned().collect()))
        .collect()
}

impl BlockTags {
    /// Loads `data/<namespace>/tags/block/**/*.json` under the data root.
    /// A missing directory yields an empty registry, which makes every tag
    /// test fail rather than assuming membership.
    pub fn load(root: &Path) -> Result<Self, WorldgenError> {
        let entries = load_registry(root, "block")?;
        let membership = build_tag_membership(&entries);
        Ok(Self {
            entries,
            membership,
        })
    }

    /// Loads `data/<namespace>/tags/block/**/*.json` across ordered pack roots.
    pub fn load_from_roots(roots: &[&Path]) -> Result<Self, WorldgenError> {
        let entries = load_registry_from_roots(roots, "block")?;
        let membership = build_tag_membership(&entries);
        Ok(Self {
            entries,
            membership,
        })
    }

    /// Members of `tag`, or `None` if `tag` is absent from the pack.
    pub fn members(&self, tag: &str) -> Option<&[String]> {
        tag_members(&self.entries, tag)
    }

    /// Whether `block` is a member of `tag`.
    pub fn contains(&self, tag: &str, block: &str) -> bool {
        if let Some(set) = self.membership.get(tag) {
            set.contains(block)
        } else {
            tag_contains(&self.entries, tag, block)
        }
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

/// The item tags of the provisioned pack, keyed by namespaced id.
#[derive(Debug, Default, Clone)]
pub struct ItemTags {
    entries: HashMap<String, Vec<String>>,
}

impl ItemTags {
    /// Loads `data/<namespace>/tags/item/**/*.json` under the data root.
    /// A missing directory yields an empty registry.
    pub fn load(root: &Path) -> Result<Self, WorldgenError> {
        Ok(Self {
            entries: load_registry(root, "item")?,
        })
    }

    /// Loads `data/<namespace>/tags/item/**/*.json` across ordered pack roots.
    pub fn load_from_roots(roots: &[&Path]) -> Result<Self, WorldgenError> {
        Ok(Self {
            entries: load_registry_from_roots(roots, "item")?,
        })
    }

    /// Members of `tag`, or `None` if `tag` is absent from the pack.
    pub fn members(&self, tag: &str) -> Option<&[String]> {
        tag_members(&self.entries, tag)
    }

    /// Whether `item` is a member of `tag`.
    pub fn contains(&self, tag: &str, item: &str) -> bool {
        tag_contains(&self.entries, tag, item)
    }

    /// Number of tags read from the pack.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pack declared no item tags at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The enchantment tags of the provisioned pack, keyed by namespaced id.
#[derive(Debug, Default, Clone)]
pub struct EnchantmentTags {
    entries: HashMap<String, Vec<String>>,
}

impl EnchantmentTags {
    /// Loads `data/<namespace>/tags/enchantment/**/*.json` under the data root.
    /// A missing directory yields an empty registry.
    pub fn load(root: &Path) -> Result<Self, WorldgenError> {
        Ok(Self {
            entries: load_registry(root, "enchantment")?,
        })
    }

    /// Loads `data/<namespace>/tags/enchantment/**/*.json` across ordered pack roots.
    pub fn load_from_roots(roots: &[&Path]) -> Result<Self, WorldgenError> {
        Ok(Self {
            entries: load_registry_from_roots(roots, "enchantment")?,
        })
    }

    /// Members of `tag`, or `None` if `tag` is absent from the pack.
    pub fn members(&self, tag: &str) -> Option<&[String]> {
        tag_members(&self.entries, tag)
    }

    /// Whether `enchantment` is a member of `tag`.
    pub fn contains(&self, tag: &str, enchantment: &str) -> bool {
        tag_contains(&self.entries, tag, enchantment)
    }

    /// Number of tags read from the pack.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pack declared no enchantment tags at all.
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
    fn costs_no_draws(&self) -> bool {
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
    fn states(&self) -> Vec<&str> {
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
    fn is_placeable(&self) -> bool {
        self.is_modelable() && self.states().into_iter().all(plant_support::is_placeable)
    }
}

/// Extracts one property value from a rendered block state string `id[k1=v1,k2=v2]`.
fn block_state_property<'a>(state: &'a str, key: &str) -> Option<&'a str> {
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
const ALWAYS_WATER_SOURCE_BLOCKS: &[&str] = &[
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
const LEGACY_SOLID_OVERRIDES: &[&str] = &[
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
const LEGACY_NON_SOLID_OVERRIDES: &[&str] = &[
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

fn is_replaceable_block(base_id: &str, tags: &BlockTags) -> bool {
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

fn heightmap_blocks_motion(state: &str, tags: &BlockTags, exclude_leaves: bool) -> bool {
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
const MAX_PATCH_REACH: i32 = 16;

/// The widest disk this runtime runs. A disk is bounded by the same replay
/// window as a patch: centred on local 15 it reaches local 31 of the next
/// chunk at this radius, and the pack's widest disk is 6.
const MAX_DISK_RADIUS: i32 = MAX_PATCH_REACH;

/// The most child attempts one patch may ask for: RustMC's work budget.
const MAX_PATCH_TRIES: i64 = 256;

/// The widest vertical patch scatter accepted: RustMC's work budget.
const MAX_PATCH_Y_SPREAD: i64 = 32;

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
    fn patch_reach(&self) -> i32 {
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
    fn costs_no_draws(&self) -> bool {
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
    fn write_bounds(&self) -> Option<(i32, i32, i32, i32)> {
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

fn add_horizontal_bounds(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
    (
        a.0.saturating_add(b.0),
        a.1.saturating_add(b.1),
        a.2.saturating_add(b.2),
        a.3.saturating_add(b.3),
    )
}

fn union_horizontal_bounds(
    a: (i32, i32, i32, i32),
    b: (i32, i32, i32, i32),
) -> (i32, i32, i32, i32) {
    (a.0.min(b.0), a.1.max(b.1), a.2.min(b.2), a.3.max(b.3))
}

fn union_placed_bounds<'a>(
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
    /// `minecraft:trapezoid` with explicit non-zero plateau width (`Java Edition 26.1`
    /// rev 3880954).
    TrapezoidPlateau {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
        plateau: i32,
    },
    /// `minecraft:biased_to_bottom` (`Configured feature/int provider` rev 3267224;
    /// `Sugar Cane` rev 3899892: `11/18`, `5/18`, `2/18` over `[2..4]`).
    BiasedToBottom {
        min_inclusive: i32,
        max_inclusive: i32,
    },
    /// `minecraft:clamped` (`Configured feature/int provider` rev 3267224).
    Clamped {
        min_inclusive: i32,
        max_inclusive: i32,
        source: Box<RangeProvider>,
    },
    /// `minecraft:weighted_list` (`Configured feature/int provider` rev 3267224).
    WeightedList {
        distribution: Vec<(RangeProvider, i32)>,
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
            let min_val = object
                .get("min_inclusive")
                .or_else(|| object.get("min"))
                .or_else(|| {
                    object
                        .get("value")
                        .and_then(|v| v.get("min_inclusive").or_else(|| v.get("min")))
                })?;
            let max_val = object
                .get("max_inclusive")
                .or_else(|| object.get("max"))
                .or_else(|| {
                    object
                        .get("value")
                        .and_then(|v| v.get("max_inclusive").or_else(|| v.get("max")))
                })?;
            let min_inclusive = VerticalAnchor::from_value(min_val)?;
            let max_inclusive = VerticalAnchor::from_value(max_val)?;
            // A count provider is a plain numeric range; only the height
            // decoration accepts anchors that resolve against the dimension.
            if !anchors_allowed
                && !(matches!(min_inclusive, VerticalAnchor::Absolute(_))
                    && matches!(max_inclusive, VerticalAnchor::Absolute(_)))
            {
                return None;
            }
            if kind == "minecraft:uniform" {
                Some(Self::Uniform {
                    min_inclusive,
                    max_inclusive,
                })
            } else {
                let plateau = object
                    .get("plateau")
                    .or_else(|| object.get("value").and_then(|v| v.get("plateau")))
                    .and_then(Value::as_i64)
                    .and_then(|v| i32::try_from(v).ok())
                    .unwrap_or(0);
                if plateau > 0 {
                    Some(Self::TrapezoidPlateau {
                        min_inclusive,
                        max_inclusive,
                        plateau,
                    })
                } else {
                    Some(Self::Trapezoid {
                        min_inclusive,
                        max_inclusive,
                    })
                }
            }
        };
        let plain_bounds = || -> Option<(i32, i32)> {
            let min_i = object
                .get("min_inclusive")
                .or_else(|| object.get("value").and_then(|v| v.get("min_inclusive")))
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())?;
            let max_i = object
                .get("max_inclusive")
                .or_else(|| object.get("value").and_then(|v| v.get("max_inclusive")))
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())?;
            if max_i < min_i {
                return None;
            }
            Some((min_i, max_i))
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
            Some("minecraft:biased_to_bottom") if !anchors_allowed => {
                let (min_inclusive, max_inclusive) = plain_bounds()?;
                Some(Self::BiasedToBottom {
                    min_inclusive,
                    max_inclusive,
                })
            }
            Some("minecraft:clamped") if !anchors_allowed => {
                let (min_inclusive, max_inclusive) = plain_bounds()?;
                let src_val = object
                    .get("source")
                    .or_else(|| object.get("value").and_then(|v| v.get("source")))?;
                let source = Self::from_value(src_val, false)?;
                Some(Self::Clamped {
                    min_inclusive,
                    max_inclusive,
                    source: Box::new(source),
                })
            }
            Some("minecraft:weighted_list") if !anchors_allowed => {
                let dist_val = object
                    .get("distribution")
                    .or_else(|| object.get("value").and_then(|v| v.get("distribution")))?
                    .as_array()?;
                let mut distribution = Vec::with_capacity(dist_val.len());
                for entry in dist_val {
                    let weight = entry
                        .get("weight")
                        .and_then(Value::as_i64)
                        .and_then(|w| i32::try_from(w).ok())?;
                    if weight <= 0 {
                        return None;
                    }
                    let data = Self::from_value(entry.get("data")?, false)?;
                    distribution.push((data, weight));
                }
                if distribution.is_empty() {
                    return None;
                }
                Some(Self::WeightedList { distribution })
            }
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
        match self {
            Self::Clamped { source, .. } => source.is_modelable(),
            Self::WeightedList { distribution } => {
                !distribution.is_empty()
                    && distribution
                        .iter()
                        .all(|(provider, weight)| *weight > 0 && provider.is_modelable())
            }
            Self::Unsupported { .. } => false,
            _ => true,
        }
    }

    /// Whether sampling this range provider consumes zero RNG draws.
    fn costs_no_draws(&self) -> bool {
        matches!(self, Self::Constant(_))
    }
}

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
    fn costs_no_draws(&self) -> bool {
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

    fn horizontal_step(&self) -> Option<(i32, i32, i32, i32)> {
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

    fn validate_budget(
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

    fn patch_reach(&self) -> i32 {
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

    fn remaining_costs_no_draws(&self, index: usize) -> bool {
        self.modifiers
            .get(index..)
            .is_some_and(|rest| rest.iter().all(PlacementModifier::costs_no_draws))
            && self.feature.costs_no_draws()
    }

    fn remaining_write_bounds(&self, index: usize) -> Option<(i32, i32, i32, i32)> {
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

/// Diagnostic summary for one placed feature in a generation step.
///
/// Source: minecraft.wiki `Placed feature` (revision 3816343):
/// "Placed features are applied in order to determine where feature
/// placement attempt(s) should occur."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedFeatureDiagnosis {
    /// Placed feature identifier.
    pub id: String,
    /// Whether RustMC currently models and runs this placed feature.
    pub modelable: bool,
    /// The first unsupported piece blocking placement in evaluation order,
    /// or `None` when `modelable` is `true`.
    pub first_unsupported: Option<String>,
    /// Every unsupported piece encountered in evaluation order.
    pub unsupported_pieces: Vec<String>,
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
    state_providers: HashMap<String, StateProvider>,
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

/// Index of the `VEGETAL_DECORATION` step in a biome's 11-entry `features`
/// array.
///
/// Source: minecraft.wiki `Biome definition (Java Edition)` (revision 3862553):
/// "VEGETAL_DECORATION: Used for trees, bamboo, cacti, kelp, and other ground
/// and ocean vegetation in vanilla."
pub const VEGETAL_DECORATION_STEP: usize = 9;

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
        let mut provider_docs: Vec<(String, Value)> = Vec::new();
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
            for entry in walk_json(&worldgen.join("block_state_provider"))? {
                provider_docs.push((format!("{ns}:{}", stem(&entry)), read_json(&entry)?));
            }
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
        provider_docs.sort_by(|left, right| left.0.cmp(&right.0));
        configured_docs.sort_by(|left, right| left.0.cmp(&right.0));
        placed_docs.sort_by(|left, right| left.0.cmp(&right.0));
        biome_docs.sort_by(|left, right| left.0.cmp(&right.0));
        for (id, document) in provider_docs {
            data.state_providers
                .insert(id, parse_state_provider(Some(&document)));
        }
        let provider_snapshot = data.state_providers.clone();
        for provider in data.state_providers.values_mut() {
            resolve_state_provider_references(provider, &provider_snapshot, 0);
        }
        for (id, document) in configured_docs {
            let mut feature = parse_configured_feature(&document)?;
            resolve_configured_providers(&mut feature, &data.state_providers);
            data.configured.insert(id, Rc::new(feature));
        }
        let mut placed_feature_targets: Vec<(String, String)> = Vec::new();
        for (id, document) in placed_docs {
            if let Some(target_cf) = document.get("feature").and_then(Value::as_str) {
                placed_feature_targets.push((id.clone(), target_cf.to_string()));
            }
            let feature = parse_placed_feature(&id, &document, &data.configured)?;
            data.placed.insert(id, Rc::new(feature));
        }
        for _ in 0..4 {
            resolve_selector_references(&mut data.configured, &data.placed, &data.state_providers);
            for (placed_id, cf_id) in &placed_feature_targets {
                if let (Some(cf), Some(placed_rc)) =
                    (data.configured.get(cf_id), data.placed.get_mut(placed_id))
                {
                    Rc::make_mut(placed_rc).feature = Rc::clone(cf);
                }
            }
        }
        for feature in data.configured.values() {
            record_unmodeled_configured(&mut data.unimplemented_features, feature);
        }
        for feature in data.placed.values() {
            record_unmodeled_modifiers(&mut data.unimplemented_features, feature);
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
                let edges = successors.entry(*node).or_default();
                if let Some(next) = chain.get(index + 1)
                    && !edges.contains(next)
                {
                    edges.push(*next);
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
    pub fn region_ordinals<S: AsRef<str>>(&self, step: usize, biomes: &[S]) -> Vec<usize> {
        let mut ordinals = Vec::new();
        for biome in biomes {
            if let Some(indices) = self
                .biome_ordinals
                .get(biome.as_ref())
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

    /// Diagnoses the placed features that `biomes` contribute to `step`, in the
    /// step schedule's ordinal order (`self.region_ordinals(step, biomes)`).
    pub fn diagnose_step_for_biomes(
        &self,
        step: usize,
        biomes: &[String],
    ) -> Vec<PlacedFeatureDiagnosis> {
        let Some(schedule) = self.step(step) else {
            return Vec::new();
        };
        self.region_ordinals(step, biomes)
            .into_iter()
            .filter_map(|ordinal| schedule.get(ordinal))
            .map(|placed| placed.diagnose())
            .collect()
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
        let Some((done, _)) = path.pop() else {
            return false;
        };
        in_progress.remove(&done);
        visited.insert(done);
        emitted.push(done);
    }
}

/// Inserts or overrides one integer property on a rendered block state string,
/// keeping property keys in sorted order to match [`state_from_object`].
fn set_state_property(state: &str, property: &str, value: i32) -> String {
    set_state_property_str(state, property, &value.to_string())
}

/// Inserts or overrides one property on a rendered block state string, keeping
/// property keys in sorted order to match [`state_from_object`].
fn set_state_property_str(state: &str, property: &str, value_str: &str) -> String {
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

fn resolve_state_provider_references(
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

fn resolve_configured_providers(
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

fn resolve_placed_providers(placed: &mut PlacedFeature, registry: &HashMap<String, StateProvider>) {
    resolve_configured_providers(Rc::make_mut(&mut placed.feature), registry);
}

fn resolve_child_placed_feature(
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

fn resolve_nested_placed(
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
fn resolve_selector_references(
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
fn parse_modifiers_list(
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
fn parse_inline_placed_feature(value: &Value) -> Result<PlacedFeature, WorldgenError> {
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
fn parse_disk(body: &Value) -> Result<ConfiguredFeature, WorldgenError> {
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
fn parse_spring(body: &Value) -> Result<ConfiguredFeature, WorldgenError> {
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
fn parse_random_patch(kind: &str, body: &Value) -> Result<ConfiguredFeature, WorldgenError> {
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
fn parse_simple_random_selector(body: &Value) -> Result<ConfiguredFeature, WorldgenError> {
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
fn parse_random_selector(body: &Value) -> Result<ConfiguredFeature, WorldgenError> {
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
fn parse_weighted_random_selector(body: &Value) -> Result<ConfiguredFeature, WorldgenError> {
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
fn parse_random_boolean_selector(body: &Value) -> Result<ConfiguredFeature, WorldgenError> {
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
fn parse_block_column(body: &Value) -> Result<ConfiguredFeature, WorldgenError> {
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

fn record_unmodeled_range(counts: &mut HashMap<String, usize>, range: &RangeProvider) {
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
fn record_unmodeled_configured(counts: &mut HashMap<String, usize>, feature: &ConfiguredFeature) {
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
fn record_unmodeled_modifiers(counts: &mut HashMap<String, usize>, placed: &PlacedFeature) {
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
fn unmodeled_predicates(predicate: &BlockPredicate) -> Vec<&str> {
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
fn record_unmodeled_provider(counts: &mut HashMap<String, usize>, provider: &StateProvider) {
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
fn record_unmodeled_predicates(counts: &mut HashMap<String, usize>, predicate: &BlockPredicate) {
    for kind in unmodeled_predicates(predicate) {
        *counts.entry(format!("predicate:{kind}")).or_insert(0) += 1;
    }
}

/// Counts the states a `minecraft:simple_block` could write but has no ground
/// rule for, keyed `state:<id>`, so the feature they gate is not a silent gap.
fn record_unplaceable_states(counts: &mut HashMap<String, usize>, provider: &StateProvider) {
    for state in provider.states() {
        if !plant_support::is_placeable(state) {
            *counts
                .entry(format!("state:{}", base_block_id(state)))
                .or_insert(0) += 1;
        }
    }
}

fn push_unique_piece(pieces: &mut Vec<String>, seen: &mut HashSet<String>, key: String) {
    if seen.insert(key.clone()) {
        pieces.push(key);
    }
}

fn collect_unmodeled_range_pieces(
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
fn collect_unmodeled_placed_pieces(
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

fn collect_unmodeled_configured_pieces(
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

fn collect_unmodeled_rule_pieces(
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

fn collect_unmodeled_provider_pieces(
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

/// The block state a spring's fluid state places. The pack writes the fluid
/// state, `{"id": "minecraft:water", "properties": {"falling": "true"}}`, but
/// `falling` belongs to the fluid: the 26.3 block-state report gives
/// `minecraft:water` and `minecraft:lava` only a `level` property, so the
/// document's name with its fluid properties would be an unknown block state.
/// A still fluid is a source, whichever way `falling` reads, and a source is
/// the block's default state. Any other fluid has no mapping here.
fn spring_block_state(value: &Value) -> Option<String> {
    let id = value.as_str().or_else(|| value.get("id")?.as_str())?;
    matches!(id, "minecraft:water" | "minecraft:lava").then(|| id.to_string())
}

/// Whether a block read for a spring's neighbour check is air.
fn is_air_block(block: Option<&str>) -> bool {
    block.is_some_and(|block| {
        matches!(
            base_block_id(block),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        )
    })
}

/// Whether the given block matches any block in `valid_blocks` (or tag if prefixed by `#`).
fn is_valid_block(block: Option<&str>, valid_blocks: &[String], tags: &BlockTags) -> bool {
    let Some(block) = block else {
        return false;
    };
    let base = base_block_id(block);
    valid_blocks.iter().any(|valid| {
        if let Some(tag) = valid.strip_prefix('#') {
            tags.contains(tag, base) || tags.contains(tag, block)
        } else {
            let valid_base = base_block_id(valid);
            valid_base == base || valid == block
        }
    })
}

struct DiskFeature<'a> {
    state_provider: &'a StateProvider,
    target_predicate: &'a BlockPredicate,
    radius: &'a RangeProvider,
    half_height: i32,
}

/// Places a circular disk feature around `pos`.
fn place_disk(
    target: &mut dyn DecorationTarget,
    random: &mut DecorationRandom,
    tags: &BlockTags,
    disk: DiskFeature<'_>,
    pos: DecorationPos,
) -> usize {
    let DiskFeature {
        state_provider,
        target_predicate,
        radius,
        half_height,
    } = disk;
    let r = radius.sample(target.min_y(), target.height(), random);
    if r < 0 || half_height < 0 {
        return 0;
    }
    let mut placed = 0;
    let r_sq = r * r;
    // Each column finishes top to bottom before the next starts, so a provider
    // that reads the block below sees it as the terrain left it, not as this
    // disk rewrote it. A candidate outside the chunk is clipped before its
    // provider runs; no pack disk has a provider that draws.
    for dx in -r..=r {
        let px = pos.x + dx;
        for dz in -r..=r {
            if dx * dx + dz * dz > r_sq {
                continue;
            }
            let pz = pos.z + dz;
            for dy in (-half_height..=half_height).rev() {
                let py = pos.y + dy;
                if !target.writable(px, py, pz) {
                    continue;
                }
                let candidate_pos = DecorationPos {
                    x: px,
                    y: py,
                    z: pz,
                };
                if !target_predicate.test(target, candidate_pos, tags) {
                    continue;
                }
                let Some(state) = state_provider.sample(target, candidate_pos, tags, random) else {
                    continue;
                };
                target.set_block(px, py, pz, state.as_ref());
                placed += 1;
            }
        }
    }
    placed
}

struct SpringFeature<'a> {
    state: &'a str,
    rock_count: i32,
    hole_count: i32,
    requires_block_below: bool,
    valid_blocks: &'a [String],
}

/// Places a fluid spring feature at `pos`: the ceiling must be valid, the block
/// below too when the feature requires it, and the four sides and the block
/// below must then hold exactly `rock_count` valid blocks and `hole_count` air
/// cells. Neighbours past the chunk edge are read from the terrain, so a pocket
/// on a border is judged as it would be anywhere else.
fn place_spring(
    target: &mut dyn DecorationTarget,
    tags: &BlockTags,
    spring: SpringFeature<'_>,
    pos: DecorationPos,
) -> usize {
    let SpringFeature {
        state,
        rock_count,
        hole_count,
        requires_block_below,
        valid_blocks,
    } = spring;
    if !target.writable(pos.x, pos.y, pos.z) {
        return 0;
    }
    let valid = |target: &dyn DecorationTarget, x: i32, y: i32, z: i32| {
        is_valid_block(target.read_block(x, y, z).as_deref(), valid_blocks, tags)
    };
    if !valid(target, pos.x, pos.y + 1, pos.z) {
        return 0;
    }
    if requires_block_below && !valid(target, pos.x, pos.y - 1, pos.z) {
        return 0;
    }
    if !target.is_air(pos.x, pos.y, pos.z) && !valid(target, pos.x, pos.y, pos.z) {
        return 0;
    }
    let neighbors = [
        (pos.x - 1, pos.y, pos.z),
        (pos.x + 1, pos.y, pos.z),
        (pos.x, pos.y, pos.z - 1),
        (pos.x, pos.y, pos.z + 1),
        (pos.x, pos.y - 1, pos.z),
    ];
    let mut rocks = 0i32;
    let mut holes = 0i32;
    for (nx, ny, nz) in neighbors {
        let block = target.read_block(nx, ny, nz);
        if is_valid_block(block.as_deref(), valid_blocks, tags) {
            rocks += 1;
        } else if is_air_block(block.as_deref()) {
            holes += 1;
        }
    }
    if rocks == rock_count && holes == hole_count {
        target.set_block(pos.x, pos.y, pos.z, state);
        1
    } else {
        0
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
fn parse_direction_name(name: &str) -> Option<Direction> {
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
            }
            | Self::TrapezoidPlateau {
                min_inclusive,
                max_inclusive,
                ..
            } => Some((
                min_inclusive.resolve(min_y, height),
                max_inclusive.resolve(min_y, height),
            )),
            Self::BiasedToBottom {
                min_inclusive,
                max_inclusive,
            } => Some((*min_inclusive, *max_inclusive)),
            Self::Clamped {
                min_inclusive,
                max_inclusive,
                source,
            } => {
                let (s_min, s_max) = source.range(min_y, height)?;
                Some((
                    s_min.clamp(*min_inclusive, *max_inclusive),
                    s_max.clamp(*min_inclusive, *max_inclusive),
                ))
            }
            Self::WeightedList { distribution } => {
                let mut low: Option<i32> = None;
                let mut high: Option<i32> = None;
                for (entry, _) in distribution {
                    let (a, b) = entry.range(min_y, height)?;
                    low = Some(low.map_or(a, |prev| prev.min(a)));
                    high = Some(high.map_or(b, |prev| prev.max(b)));
                }
                low.zip(high)
            }
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
            Self::TrapezoidPlateau { plateau, .. } => {
                let span = max - min;
                if span <= 0 || *plateau >= span {
                    return min + random.next_int_bounded(span.max(0) + 1);
                }
                let low = (span - *plateau) / 2;
                let high = span - low;
                min + random.next_int_bounded(high + 1) + random.next_int_bounded(low + 1)
            }
            Self::BiasedToBottom { .. } => {
                let span = max - min;
                let bound = random.next_int_bounded(span + 1) + 1;
                min + random.next_int_bounded(bound)
            }
            Self::Clamped {
                min_inclusive,
                max_inclusive,
                source,
            } => source
                .sample(min_y, height, random)
                .clamp(*min_inclusive, *max_inclusive),
            Self::WeightedList { distribution } => {
                let total: i32 = distribution.iter().map(|(_, weight)| *weight).sum();
                if total <= 0 {
                    return 0;
                }
                let mut pick = random.next_int_bounded(total);
                for (entry, weight) in distribution {
                    if pick < *weight {
                        return entry.sample(min_y, height, random);
                    }
                    pick -= *weight;
                }
                0
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
/// Reads reach one column beyond it: the six-neighbour air test, and the
/// heightmap the ore anchor gate walks over its box's footprint, whose halo
/// columns stay within the 3x3 chunk region the pass already resolves biomes
/// over.
pub trait DecorationTarget {
    /// The dimension's lowest buildable row.
    fn min_y(&self) -> i32;
    /// The number of buildable rows.
    fn height(&self) -> i32;
    /// Whether a position may be written at all. Positions outside the
    /// decorated chunk are refused *before* the replacement rules are
    /// evaluated, so they cost no draws either; the neighbour chunk's own
    /// pass paints its half of a vein that straddles the border.
    fn writable(&self, x: i32, y: i32, z: i32) -> bool;
    /// Optional inclusive horizontal chunk bounds `(min_x, max_x, min_z, max_z)`
    /// of writable positions, used to skip halo anchors that cannot reach.
    fn writable_bounds(&self) -> Option<(i32, i32, i32, i32)> {
        None
    }
    /// The block name currently at the position, `None` where the world
    /// holds air. Earlier decorations in this pass have already painted
    /// here: a later vein may replace stone a blob turned to granite.
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<&str>;
    /// The block a predicate sees at any position in the build volume. Inside
    /// the decorated chunk this is [`Self::block_at`] with an empty row read
    /// as air; beyond it the generator answers with the terrain as the fill
    /// and carver passes left it, since the neighbour's decorations have not
    /// been replayed. `None` means the row is outside the build volume.
    fn read_block(&self, x: i32, y: i32, z: i32) -> Option<Cow<'_, str>> {
        self.block_at(x, y, z).map(Cow::Borrowed)
    }
    /// Whether the position holds air, for the air-exposure test. Reads
    /// beyond the writable zone still have an answer, because the six
    /// neighbours of a border block are queried regardless.
    fn is_air(&self, x: i32, y: i32, z: i32) -> bool;
    /// Writes the block, replacing whatever was there.
    fn set_block(&mut self, x: i32, y: i32, z: i32, name: &str);
    /// The biome at a position, which the `minecraft:biome` filter asks for.
    fn biome_at(&self, x: i32, y: i32, z: i32) -> Option<String>;
    /// Allocation-free biome predicate check for the `minecraft:biome` filter.
    fn has_biome(&self, x: i32, y: i32, z: i32, allowed: &dyn Fn(&str) -> bool) -> bool {
        self.biome_at(x, y, z).is_some_and(|biome| allowed(&biome))
    }
    /// The absolute Y of the highest terrain row of the column, as the
    /// world-gen ocean-floor heightmap reports it: fluid and air above a sea
    /// floor do not count. The ore anchor gate reads it for every column of
    /// its box's footprint, including the halo over the neighbour chunks.
    fn ocean_floor_height(&self, x: i32, z: i32) -> i32;
    /// One block above the highest matching block at `(x, z)` for `kind`, or
    /// `min_y()` when the column contains no matching block.
    fn heightmap_height(&self, kind: HeightmapKind, x: i32, z: i32, tags: &BlockTags) -> i32 {
        default_heightmap_height(self, kind, x, z, tags)
    }
}

fn default_heightmap_height<T: DecorationTarget + ?Sized>(
    target: &T,
    kind: HeightmapKind,
    x: i32,
    z: i32,
    tags: &BlockTags,
) -> i32 {
    let min_y = target.min_y();
    let max_y = min_y + target.height() - 1;
    for y in (min_y..=max_y).rev() {
        if let Some(block) = target.read_block(x, y, z)
            && kind.matches_block(block.as_ref(), tags)
        {
            return y + 1;
        }
    }
    if matches!(
        kind,
        HeightmapKind::OceanFloor | HeightmapKind::OceanFloorWg
    ) {
        let fallback = target.ocean_floor_height(x, z);
        if fallback > min_y {
            return fallback;
        }
    }
    min_y
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

/// Places one `minecraft:simple_block`. The state is drawn first: two chunks
/// replaying one anchor must consume the same stream whether or not this
/// candidate lands in the chunk being decorated. The block then only replaces
/// air/water/replaceable, and only where the plant's ground holds.
fn place_simple_block(
    target: &mut dyn DecorationTarget,
    random: &mut DecorationRandom,
    tags: &BlockTags,
    to_place: &StateProvider,
    pos: DecorationPos,
) -> usize {
    let Some(state) = to_place.sample(target, pos, tags, random) else {
        return 0;
    };
    plant_support::place_plant(target, tags, state.as_ref(), pos)
}

struct BlockColumnFeature<'a> {
    direction: Direction,
    allowed_placement: &'a BlockPredicate,
    prioritize_tip: bool,
    layers: &'a [BlockColumnLayer],
}

/// Places a `minecraft:block_column` along `direction`, trimming from the start
/// when `prioritize_tip` is `true` and from the tip otherwise (`Cave Vines (feature)`
/// rev 3818484).
fn place_block_column(
    target: &mut dyn DecorationTarget,
    random: &mut DecorationRandom,
    tags: &BlockTags,
    column: BlockColumnFeature<'_>,
    pos: DecorationPos,
) -> usize {
    let BlockColumnFeature {
        direction,
        allowed_placement,
        prioritize_tip,
        layers,
    } = column;
    let min_y = target.min_y();
    let max_y = min_y + target.height();
    let heights: Vec<usize> = layers
        .iter()
        .map(|layer| layer.height.sample(min_y, target.height(), random).max(0) as usize)
        .collect();
    let total: usize = heights.iter().sum();
    if total == 0 {
        return 0;
    }
    let (dx, dy, dz) = direction.unit_vector();
    let mut avail = 0usize;
    let mut scan = pos;
    while avail < total {
        if scan.y < min_y
            || scan.y >= max_y
            || !target.writable(scan.x, scan.y, scan.z)
            || !allowed_placement.test(target, scan, tags)
        {
            break;
        }
        avail += 1;
        scan.x += dx;
        scan.y += dy;
        scan.z += dz;
    }
    if avail == 0 {
        return 0;
    }
    let mut trim_from_start = if prioritize_tip { total - avail } else { 0 };
    let mut remaining = avail;
    let mut cur = pos;
    let mut written = 0usize;
    for (layer, height) in layers.iter().zip(heights) {
        let skip = trim_from_start.min(height);
        trim_from_start -= skip;
        let take = (height - skip).min(remaining);
        remaining -= take;
        for _ in 0..take {
            if let Some(state) = layer.provider.sample(target, cur, tags, random) {
                target.set_block(cur.x, cur.y, cur.z, state.as_ref());
                written += 1;
            }
            cur.x += dx;
            cur.y += dy;
            cur.z += dz;
        }
    }
    written
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
    pub fn decorate_step<S: AsRef<str>>(
        &self,
        target: &mut dyn DecorationTarget,
        step: usize,
        chunk_x: i32,
        chunk_z: i32,
        world_seed: i64,
        region_biomes: &[S],
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
    pub(crate) fn run_chain(
        &self,
        target: &mut dyn DecorationTarget,
        placed: &PlacedFeature,
        index: usize,
        pos: DecorationPos,
        random: &mut DecorationRandom,
    ) -> usize {
        let mut ctx = PlacementContext {
            writable_bounds: target.writable_bounds(),
            target,
            random,
        };
        self.run_chain_step(&mut ctx, placed, index, pos, false)
    }

    fn run_chain_step(
        &self,
        ctx: &mut PlacementContext<'_>,
        placed: &PlacedFeature,
        index: usize,
        pos: DecorationPos,
        in_loop: bool,
    ) -> usize {
        if !in_loop
            && !matches!(placed.feature.as_ref(), ConfiguredFeature::Ore { .. })
            && !bounds_overlap(
                pos,
                placed.remaining_write_bounds(index),
                ctx.writable_bounds,
            )
        {
            return 0;
        }
        let Some(modifier) = placed.modifiers.get(index) else {
            let feature = &*placed.feature;
            return match feature {
                ConfiguredFeature::Ore {
                    size,
                    targets,
                    discard_chance_on_air_exposure,
                } => place_ore(
                    ctx.target,
                    ctx.random,
                    &self.tags,
                    *size,
                    targets,
                    *discard_chance_on_air_exposure,
                    pos,
                ),
                ConfiguredFeature::RandomPatch {
                    tries,
                    xz_spread,
                    y_spread,
                    feature: child,
                } => {
                    let child_no_draws = child.remaining_costs_no_draws(0);
                    let child_bounds = child.remaining_write_bounds(0);
                    let mut written = 0;
                    for try_idx in 0..*tries {
                        // The pack's own vegetation states this scatter as an
                        // `offset` of three plateau-free trapezoids, x then y
                        // then z. Two draws per axis, a spread of zero included.
                        let mut axis = |spread: i32| {
                            ctx.random.next_int_bounded(spread + 1)
                                + ctx.random.next_int_bounded(spread + 1)
                                - spread
                        };
                        let dx = axis(*xz_spread);
                        let dy = axis(*y_spread);
                        let dz = axis(*xz_spread);
                        let next = DecorationPos {
                            x: pos.x + dx,
                            y: pos.y + dy,
                            z: pos.z + dz,
                        };
                        let try_in_loop = in_loop || try_idx + 1 < *tries;
                        if (!try_in_loop || child_no_draws)
                            && !bounds_overlap(next, child_bounds, ctx.writable_bounds)
                        {
                            continue;
                        }
                        written += self.run_chain_step(ctx, child, 0, next, try_in_loop);
                    }
                    written
                }
                ConfiguredFeature::SimpleBlock { to_place } => {
                    place_simple_block(ctx.target, ctx.random, &self.tags, to_place, pos)
                }
                ConfiguredFeature::Disk {
                    state_provider,
                    target_predicate,
                    radius,
                    half_height,
                } => place_disk(
                    ctx.target,
                    ctx.random,
                    &self.tags,
                    DiskFeature {
                        state_provider,
                        target_predicate,
                        radius,
                        half_height: *half_height,
                    },
                    pos,
                ),
                ConfiguredFeature::Spring {
                    state,
                    rock_count,
                    hole_count,
                    requires_block_below,
                    valid_blocks,
                } => place_spring(
                    ctx.target,
                    &self.tags,
                    SpringFeature {
                        state,
                        rock_count: *rock_count,
                        hole_count: *hole_count,
                        requires_block_below: *requires_block_below,
                        valid_blocks,
                    },
                    pos,
                ),
                ConfiguredFeature::SimpleRandomSelector { features } => {
                    if features.is_empty() {
                        0
                    } else {
                        let chosen = ctx.random.next_int_bounded(features.len() as i32) as usize;
                        self.run_chain_step(ctx, &features[chosen], 0, pos, in_loop)
                    }
                }
                ConfiguredFeature::RandomSelector { features, default } => {
                    for (child, chance) in features {
                        if ctx.random.next_float() < *chance {
                            return self.run_chain_step(ctx, child, 0, pos, in_loop);
                        }
                    }
                    self.run_chain_step(ctx, default, 0, pos, in_loop)
                }
                ConfiguredFeature::WeightedRandomSelector { features } => {
                    let total: i32 = features.iter().map(|(_, weight)| *weight).sum();
                    if total <= 0 {
                        0
                    } else {
                        let mut pick = ctx.random.next_int_bounded(total);
                        let mut chosen = None;
                        for (child, weight) in features {
                            if pick < *weight {
                                chosen = Some(child);
                                break;
                            }
                            pick -= *weight;
                        }
                        chosen.map_or(0, |child| self.run_chain_step(ctx, child, 0, pos, in_loop))
                    }
                }
                ConfiguredFeature::RandomBooleanSelector {
                    feature_true,
                    feature_false,
                } => {
                    let chosen = if ctx.random.next_bool() {
                        feature_true
                    } else {
                        feature_false
                    };
                    self.run_chain_step(ctx, chosen, 0, pos, in_loop)
                }
                ConfiguredFeature::BlockColumn {
                    direction,
                    allowed_placement,
                    prioritize_tip,
                    layers,
                } => place_block_column(
                    ctx.target,
                    ctx.random,
                    &self.tags,
                    BlockColumnFeature {
                        direction: *direction,
                        allowed_placement,
                        prioritize_tip: *prioritize_tip,
                        layers,
                    },
                    pos,
                ),
                ConfiguredFeature::Unsupported { .. } => 0,
            };
        };
        match modifier {
            PlacementModifier::Count { count } => {
                let attempts = count.sample(ctx.target.min_y(), ctx.target.height(), ctx.random);
                let total = attempts.max(0);
                let mut written = 0;
                for attempt in 0..total {
                    written += self.run_chain_step(
                        ctx,
                        placed,
                        index + 1,
                        pos,
                        in_loop || attempt + 1 < total,
                    );
                }
                written
            }
            PlacementModifier::RarityFilter { chance } => {
                if ctx.random.next_float() < 1.0f32 / (*chance as f32) {
                    self.run_chain_step(ctx, placed, index + 1, pos, in_loop)
                } else {
                    0
                }
            }
            PlacementModifier::RandomChance { chance } => {
                if ctx.random.next_float() < *chance {
                    self.run_chain_step(ctx, placed, index + 1, pos, in_loop)
                } else {
                    0
                }
            }
            PlacementModifier::InSquare => {
                let mut next = pos;
                next.x += ctx.random.next_int_bounded(16);
                next.z += ctx.random.next_int_bounded(16);
                self.run_chain_step(ctx, placed, index + 1, next, in_loop)
            }
            PlacementModifier::Heightmap { heightmap } => {
                let mut next = pos;
                next.y = ctx
                    .target
                    .heightmap_height(*heightmap, pos.x, pos.z, &self.tags);
                self.run_chain_step(ctx, placed, index + 1, next, in_loop)
            }
            PlacementModifier::Offset { x, y, z } => {
                let dx = x
                    .sample(ctx.target.min_y(), ctx.target.height(), ctx.random)
                    .clamp(-16, 16);
                let dy = y
                    .sample(ctx.target.min_y(), ctx.target.height(), ctx.random)
                    .clamp(-16, 16);
                let dz = z
                    .sample(ctx.target.min_y(), ctx.target.height(), ctx.random)
                    .clamp(-16, 16);
                let next = DecorationPos {
                    x: pos.x + dx,
                    y: pos.y + dy,
                    z: pos.z + dz,
                };
                self.run_chain_step(ctx, placed, index + 1, next, in_loop)
            }
            PlacementModifier::HeightRange { height: provider } => {
                let mut next = pos;
                next.y = provider.sample(ctx.target.min_y(), ctx.target.height(), ctx.random);
                self.run_chain_step(ctx, placed, index + 1, next, in_loop)
            }
            PlacementModifier::Biome => {
                // The filter asks the biome at the candidate position, not at
                // the chunk origin, and it costs no draws.
                let allowed = ctx.target.has_biome(pos.x, pos.y, pos.z, &|biome| {
                    self.biome_lists(biome, &placed.id)
                });
                if allowed {
                    self.run_chain_step(ctx, placed, index + 1, pos, in_loop)
                } else {
                    0
                }
            }
            PlacementModifier::BlockPredicateFilter { predicate } => {
                // The predicate reads the terrain around the candidate and
                // costs no draws; an unmodeled shape inside it is gated out by
                // `is_modelable` long before this point.
                if predicate.test(ctx.target, pos, &self.tags) {
                    self.run_chain_step(ctx, placed, index + 1, pos, in_loop)
                } else {
                    0
                }
            }
            PlacementModifier::SurfaceRelativeThresholdFilter {
                heightmap,
                min_inclusive,
                max_inclusive,
            } => {
                if heightmap != "OCEAN_FLOOR_WG" {
                    return 0;
                }
                let surface_y = ctx.target.ocean_floor_height(pos.x, pos.z);
                let delta = i64::from(pos.y) - i64::from(surface_y);
                if delta >= i64::from(*min_inclusive) && delta <= i64::from(*max_inclusive) {
                    self.run_chain_step(ctx, placed, index + 1, pos, in_loop)
                } else {
                    0
                }
            }
            PlacementModifier::EnvironmentScan {
                direction_of_search,
                max_steps,
                target_condition,
                allowed_search_condition,
            } => {
                if let Some(allowed) = allowed_search_condition
                    && !allowed.test(ctx.target, pos, &self.tags)
                {
                    return 0;
                }
                let (dx, dy, dz) = direction_of_search.unit_vector();
                let mut cur = pos;
                let mut step = 0;
                while step < *max_steps {
                    if target_condition.test(ctx.target, cur, &self.tags) {
                        return self.run_chain_step(ctx, placed, index + 1, cur, in_loop);
                    }
                    cur.x += dx;
                    cur.y += dy;
                    cur.z += dz;
                    if cur.y < ctx.target.min_y()
                        || cur.y >= ctx.target.min_y() + ctx.target.height()
                    {
                        return 0;
                    }
                    if let Some(allowed) = allowed_search_condition
                        && !allowed.test(ctx.target, cur, &self.tags)
                    {
                        break;
                    }
                    step += 1;
                }
                if target_condition.test(ctx.target, cur, &self.tags) {
                    self.run_chain_step(ctx, placed, index + 1, cur, in_loop)
                } else {
                    0
                }
            }
            // `is_modelable` gates this out before the step runs.
            PlacementModifier::Unsupported { .. } => 0,
        }
    }
}

struct PlacementContext<'a> {
    target: &'a mut dyn DecorationTarget,
    random: &'a mut DecorationRandom,
    writable_bounds: Option<(i32, i32, i32, i32)>,
}

fn bounds_overlap(
    pos: DecorationPos,
    bounds: Option<(i32, i32, i32, i32)>,
    writable: Option<(i32, i32, i32, i32)>,
) -> bool {
    let (Some((min_dx, max_dx, min_dz, max_dz)), Some((min_wx, max_wx, min_wz, max_wz))) =
        (bounds, writable)
    else {
        return true;
    };
    pos.x + max_dx >= min_wx
        && pos.x + min_dx <= max_wx
        && pos.z + max_dz >= min_wz
        && pos.z + min_dz <= max_wz
}

#[cfg(test)]
mod tests;
