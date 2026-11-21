//! The loaded feature registry and the per-step feature order derived
//! from biome feature lists.

use super::*;

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
    pub(super) configured: HashMap<String, Rc<ConfiguredFeature>>,
    pub(super) placed: HashMap<String, Rc<PlacedFeature>>,
    /// Biome id → generation step index → placed features, in pack order.
    pub(super) biomes: HashMap<String, Vec<Vec<Rc<PlacedFeature>>>>,
    /// Biome id → the placed features it lists in any step. The
    /// `minecraft:biome` filter asks exactly this question at a candidate
    /// origin, and the pack's own answer is a flat set over the whole
    /// positional array, not per step.
    pub(super) membership: HashMap<String, HashSet<String>>,
    /// Generation step index → the placed features that step carries
    /// somewhere in the pack, in the order the decoration loop runs them.
    /// A feature's index in this list is its ordinal, and the ordinal is
    /// part of its random identity, so unmodeled features keep a slot.
    pub(super) steps: Vec<StepSchedule>,
    /// Biome id → generation step index → the ordinals that biome lists at
    /// that step, deduplicated and ascending. A chunk decorates the union
    /// of these lists over the biomes in its 3x3 neighbourhood.
    pub(super) biome_ordinals: HashMap<String, Vec<Vec<usize>>>,
    /// The biomes the schedule is built over, in the order the dimension's
    /// biome source declares them. Empty until the generator supplies the
    /// source's list, which is when the schedule falls back to the pack's
    /// own identifier order.
    pub(super) biome_order: Vec<String>,
    /// The pack's block tags, used by `minecraft:tag_match` targets.
    pub(super) tags: BlockTags,
    /// What the pack asks for that this generator does not do: the gap
    /// report. Keyed by a configured feature's `minecraft:` type name, by
    /// `target:<rule>` for an unmodeled ore-target predicate, and by
    /// `placement:<modifier>` for an unmodeled decoration; both prefixes
    /// keep their namespaced kind names from reading as a feature type.
    /// Nothing prints it from the server yet — the operator-data smoke is
    /// its only reader until a startup surface exists.
    pub(crate) unimplemented_features: HashMap<String, usize>,
    pub(super) state_providers: HashMap<String, StateProvider>,
}

/// One generation step's ordered features, plus the lookup that turns a
/// placed feature into its ordinal within the step.
#[derive(Debug, Default)]
pub struct StepSchedule {
    pub(super) features: Vec<Rc<PlacedFeature>>,
    /// Feature id → its position in `features`. The graph the schedule is
    /// derived from is keyed by (feature, step), so a step can list a given
    /// placed feature at most once and this is a bijection.
    pub(super) ordinals: HashMap<String, usize>,
}

impl StepSchedule {
    /// The step's features in the order the decoration loop runs them.
    pub fn features(&self) -> &[Rc<PlacedFeature>] {
        &self.features
    }

    /// The ordinal this step gives one placed feature, or `None` when the
    /// step does not carry it.
    pub(super) fn ordinal_of(&self, id: &str) -> Option<usize> {
        self.ordinals.get(id).copied()
    }
}

/// One node of the ordering graph: a placed feature's global number paired
/// with the generation step it appears in. The reference runtime's node
/// record carries the feature too, but its number identifies the instance,
/// so the pair is the node's identity.
pub(super) type Node = (usize, usize);

/// The ordered steps a candidate biome list produces.
#[derive(Debug, Default)]
pub(super) struct BuiltSchedule {
    pub(super) steps: Vec<StepSchedule>,
    pub(super) biome_ordinals: HashMap<String, Vec<Vec<usize>>>,
}

/// How many rebuilds the cycle recovery may try before RustMC gives up on
/// the ordering. The reference runtime recurses without a bound, and its
/// search is exponential in the number of biomes; see
/// `docs/PROVENANCE.md` session 13 for the deviation and why it is safe to
/// take. The provisioned pack is shown not to reach the bound by
/// `generator::tests::smoke_operator_feature_schedule_needs_no_cycle_recovery`,
/// which is an operator-data smoke because the bound only matters for a pack
/// whose biome chains contradict each other.
pub(super) const MAX_CYCLE_ATTEMPTS: usize = 8;

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

    pub(super) fn build_step_schedule(&mut self) {
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
    pub(super) fn schedule_with_retry(
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
    pub(super) fn attempt_schedule(&self, order: &[&str]) -> Option<BuiltSchedule> {
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
pub(super) fn depth_first_visit(
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
