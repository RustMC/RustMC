//! Vanilla enchanting table level-cost and enchantment selection.
//!
//! Semantics follow minecraft.wiki `Enchanting table mechanics` (revision 3881519),
//! `Enchanting Table` (revision 3892267), `Bookshelf` (revision 3775299), and
//! `Enchantment definition` (revision 3843772).
//!
//! Note on RNG parity: minecraft.wiki specifies the probability distributions rather than
//! Java Edition's exact per-call RNG consumption sequence. RustMC matches those documented
//! distributions rather than per-seed outputs.

use crate::item::{ItemDefaults, MIN_ENCHANTMENT_LEVEL, items_equal};
use crate::vanilla::feature::EnchantmentTags;
use crate::vanilla::random::RandomSource;
use crate::world::enchantment::EnchantmentRegistry;

/// Maximum number of active bookshelves counted by an enchanting table.
///
/// Sources:
/// - minecraft.wiki `Enchanting table mechanics` (revision 3881519):
///   "The maximum number of active bookshelves is 15, and further bookshelves are ignored."
/// - minecraft.wiki `Enchanting Table` (revision 3892267):
///   "Surrounding it with up to fifteen bookshelves increases both its enchanting power and level requirement."
/// - minecraft.wiki `Bookshelf` (revision 3775299):
///   "up to a maximum of fifteen bookshelves"
pub const MAX_BOOKSHELVES: u8 = 15;

/// Minimum enchanting slot level cost.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// "Top slot: `xpCost <- floor(max(1, xpBase / 3))`".
pub const MIN_SLOT_COST: u32 = 1;

/// Constant offset added to `xpBase`.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// `xpBase <- 1 + randInt(7) + floor(min(15, bookshelves) / 2) + randInt(min(15, bookshelves))`.
const BASE_COST_OFFSET: u32 = 1;

/// Inclusive upper bound of the fixed `randInt(7)` term in `xpBase`.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// "`xpBase <- 1 + randInt(7) + floor(min(15, bookshelves) / 2) + randInt(min(15, bookshelves))`
/// where `randInt` generates a random integer in range `0` to `n` (both included)."
const BASE_RANDOM_MAX_INCLUSIVE: u32 = 7;

/// Divisor applied to the capped bookshelf count in `xpBase`.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// `floor(min(15, bookshelves) / 2)`.
const BOOKSHELF_HALF_DIVISOR: u32 = 2;

/// Divisor applied to `xpBase` for the top enchanting slot.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// `Top slot: xpCost <- floor(max(1, xpBase / 3))`.
const TOP_SLOT_DIVISOR: u32 = 3;

/// Numerator multiplier applied to `xpBase` for the middle enchanting slot.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// `Middle slot: xpCost <- floor(2 * xpBase / 3) + 1`.
const MIDDLE_SLOT_NUMERATOR: u32 = 2;

/// Divisor applied to `2 * xpBase` for the middle enchanting slot.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// `Middle slot: xpCost <- floor(2 * xpBase / 3) + 1`.
const MIDDLE_SLOT_DIVISOR: u32 = 3;

/// Constant offset added to the middle enchanting slot cost.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// `Middle slot: xpCost <- floor(2 * xpBase / 3) + 1`.
const MIDDLE_SLOT_OFFSET: u32 = 1;

/// Multiplier on the capped bookshelf count giving the bottom slot's minimum level.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// `Bottom slot: xpCost <- floor(max(xpBase, 2 * bookshelves))` with bookshelf count
/// "capped at 15" (reaching the table maximum of 30 at 15 bookshelves; also
/// minecraft.wiki `Enchanting Table` revision 3892267: "a level 30 enchantment (the maximum)").
const BOTTOM_SLOT_BOOKSHELF_MULTIPLIER: u32 = 2;

/// Enchantment tag listing all enchantments available in an enchanting table.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// "the enchanting table's list of enchantments are stored in the tag
/// `enchantments:in_enchanting_table`."
const IN_ENCHANTING_TABLE_TAG: &str = "minecraft:in_enchanting_table";

/// Canonical item identifier for plain books.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// "If the item to enchant is not a book, all enchantments to which the item is not
/// a primary item are removed." / "Books being primary to all enchantments is hardcoded."
const BOOK_ITEM_ID: &str = "minecraft:book";

/// Canonical item identifier for enchanted books (used when `enchant_with_levels`
/// runs on an already-converted book in loot evaluation).
const ENCHANTED_BOOK_ITEM_ID: &str = "minecraft:enchanted_book";

/// Constant offset added before applying the two enchantability modifiers.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// `eCost <- round((xpCost + 1 + randInt(floor(enchantability / 4)) + randInt(floor(enchantability / 4))) * (1 + randTri(0.15)))`.
const MODIFIED_LEVEL_OFFSET: u32 = 1;

/// Divisor applied to item enchantability in the two discrete modifiers.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// `floor(enchantability / 4)`.
const ENCHANTABILITY_DIVISOR: u32 = 4;

/// Half-width `n` of the triangular modifier `randTri(0.15)` in `[-0.15, 0.15)`.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// "`randTri(n)` generates a random number in range `-n` (included) to `n` (excluded)
/// with triangular distribution: `randTri(n) |-> (randFloat() + randFloat() - 1) * n`
/// where `randFloat()` generates a random number in range `0` (included) to `1` (excluded)."
const TRIANGULAR_MODIFIER_BOUND: f64 = 0.15;

/// Nearest-integer rounding offset (ties towards positive infinity, matching
/// `world::loot::round_f64_to_i32` / minecraft.wiki `Number provider` revision 3869456).
const ROUND_HALF_OFFSET: f64 = 0.5;

/// Denominator of the continuation probability `(eCost + 1) / 50`.
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// "If a chance of `(eCost + 1) / 50` is fails, the enchantment selection ends."
const REPEAT_CHANCE_DENOMINATOR: u32 = 50;

/// Divisor applied to `eCost` after each selection round (`floor(eCost / 2)`).
///
/// Source: minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// "The enchantment cost (`eCost`) is divided by two and rounded down, and the process
/// returns to step 1."
const COST_HALVING_DIVISOR: i32 = 2;

fn draw_base_level(capped_bookshelves: u32, random: &mut RandomSource) -> u32 {
    let fixed_draw = random.next_int_bounded(BASE_RANDOM_MAX_INCLUSIVE + 1);
    let shelf_draw = random.next_int_bounded(capped_bookshelves + 1);
    BASE_COST_OFFSET + fixed_draw + (capped_bookshelves / BOOKSHELF_HALF_DIVISOR) + shelf_draw
}

/// Computes the three enchanting table slot experience-level costs `[top, middle, bottom]`.
///
/// Follows minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// - Bookshelf count is capped at [`MAX_BOOKSHELVES`] (`15`):
///   "The maximum number of active bookshelves is 15, and further bookshelves are ignored."
/// - "For each slot, a base experience cost is calculated as:
///   `xpBase <- 1 + randInt(7) + floor(min(15, bookshelves) / 2) + randInt(min(15, bookshelves))`
///   where `randInt` generates a random integer in range `0` to `n` (both included)."
/// - "Top slot: `xpCost <- floor(max(1, xpBase / 3))`"
/// - "Middle slot: `xpCost <- floor(2 * xpBase / 3) + 1`"
/// - "Bottom slot: `xpCost <- floor(max(xpBase, 2 * bookshelves))`" (using the capped bookshelf count)
///
/// Matches the distributions documented on minecraft.wiki rather than vanilla's per-seed
/// RNG call sequence.
#[must_use]
pub fn slot_costs(bookshelves: u8, random: &mut RandomSource) -> [u32; 3] {
    let capped = u32::from(bookshelves.min(MAX_BOOKSHELVES));

    let top_base = draw_base_level(capped, random);
    let top = (top_base / TOP_SLOT_DIVISOR).max(MIN_SLOT_COST);

    let middle_base = draw_base_level(capped, random);
    let middle = (MIDDLE_SLOT_NUMERATOR * middle_base) / MIDDLE_SLOT_DIVISOR + MIDDLE_SLOT_OFFSET;

    let bottom_base = draw_base_level(capped, random);
    let bottom = bottom_base
        .max(BOTTOM_SLOT_BOOKSHELF_MULTIPLIER * capped)
        .max(MIN_SLOT_COST);

    [top, middle, bottom]
}

struct Candidate<'a> {
    id: &'a str,
    level: u8,
    weight: u32,
}

fn draw_modified_level(level: u32, enchantability: u32, random: &mut RandomSource) -> i32 {
    let half_quarter = enchantability / ENCHANTABILITY_DIVISOR;
    let bonus_a = random.next_int_bounded(half_quarter.saturating_add(1));
    let bonus_b = random.next_int_bounded(half_quarter.saturating_add(1));
    let base_modified = level
        .saturating_add(MODIFIED_LEVEL_OFFSET)
        .saturating_add(bonus_a)
        .saturating_add(bonus_b);
    let tri_modifier = (f64::from(random.next_float()) + f64::from(random.next_float()) - 1.0)
        * TRIANGULAR_MODIFIER_BOUND;
    ((f64::from(base_modified) * (1.0 + tri_modifier)) + ROUND_HALF_OFFSET)
        .floor()
        .clamp(1.0, f64::from(i32::MAX)) as i32
}

enum EnchantmentPool<'a> {
    Table(&'a EnchantmentTags),
    Loot(Option<&'a [String]>),
}

impl EnchantmentPool<'_> {
    fn contains(&self, id: &str) -> bool {
        match self {
            Self::Table(tags) => tags.contains(IN_ENCHANTING_TABLE_TAG, id),
            Self::Loot(None) => true,
            Self::Loot(Some(allowed)) => allowed.iter().any(|cand| items_equal(cand, id)),
        }
    }
}

fn collect_candidates<'a>(
    item: &str,
    is_book: bool,
    modified_level: i32,
    registry: &'a EnchantmentRegistry,
    pool: &EnchantmentPool<'_>,
) -> Vec<Candidate<'a>> {
    let mut candidates = Vec::new();
    for (id, def) in registry.iter() {
        if !pool.contains(id) {
            continue;
        }
        if !is_book
            && !def
                .primary_items()
                .iter()
                .any(|primary| items_equal(primary, item))
        {
            continue;
        }
        let matching_level = (MIN_ENCHANTMENT_LEVEL..=def.max_level())
            .rev()
            .find(|&lvl| {
                def.min_cost().at_level(lvl) <= modified_level
                    && modified_level <= def.max_cost().at_level(lvl)
            });
        if let Some(ench_level) = matching_level {
            candidates.push(Candidate {
                id: def.id(),
                level: ench_level,
                weight: def.weight(),
            });
        }
    }
    candidates
}

fn pick_weighted<'a>(
    candidates: &[Candidate<'a>],
    random: &mut RandomSource,
) -> Option<(&'a str, u8)> {
    let total_weight = candidates
        .iter()
        .fold(0_u32, |acc, candidate| acc.saturating_add(candidate.weight));
    if total_weight == 0 {
        return None;
    }
    let mut roll = random.next_int_bounded(total_weight);
    for candidate in candidates {
        if roll < candidate.weight {
            return Some((candidate.id, candidate.level));
        }
        roll -= candidate.weight;
    }
    None
}

fn select_enchantments_inner(
    item: &str,
    level: u32,
    registry: &EnchantmentRegistry,
    pool: EnchantmentPool<'_>,
    defaults: &ItemDefaults,
    is_enchanting_table: bool,
    random: &mut RandomSource,
) -> Vec<(String, u8)> {
    let is_book = items_equal(item, BOOK_ITEM_ID)
        || (!is_enchanting_table && items_equal(item, ENCHANTED_BOOK_ITEM_ID));
    let enchantability = defaults
        .enchantability(item)
        .or_else(|| {
            if !is_enchanting_table && items_equal(item, ENCHANTED_BOOK_ITEM_ID) {
                defaults.enchantability(BOOK_ITEM_ID)
            } else {
                None
            }
        })
        .filter(|&value| value > 0);
    let Some(enchantability) = enchantability else {
        return Vec::new();
    };

    let modified_level = draw_modified_level(level, enchantability, random);
    let mut candidates = collect_candidates(item, is_book, modified_level, registry, &pool);
    if candidates.is_empty() {
        return Vec::new();
    }

    let mut remaining_cost = modified_level;
    let mut selected = Vec::new();

    while let Some((chosen_id, chosen_level)) = pick_weighted(&candidates, random) {
        selected.push((chosen_id.to_owned(), chosen_level));

        candidates.retain(|candidate| registry.are_compatible(chosen_id, candidate.id));
        if candidates.is_empty() {
            break;
        }

        if remaining_cost < 0
            || (random.next_int_bounded(REPEAT_CHANCE_DENOMINATOR) as i32) > remaining_cost
        {
            break;
        }
        remaining_cost /= COST_HALVING_DIVISOR;
    }

    if is_enchanting_table && is_book && selected.len() > 1 {
        let len = u32::try_from(selected.len()).unwrap_or(u32::MAX);
        let remove_idx = random.next_int_bounded(len) as usize;
        selected.remove(remove_idx);
    }

    selected
}

/// Selects enchantments for `item` at slot `level` using enchanting table rules.
///
/// Follows minecraft.wiki `Enchanting table mechanics` (revision 3881519):
/// - Item eligibility: items without a positive `minecraft:enchantable` value in
///   `defaults` return an empty list ("It is enchantable, meaning in Java Edition
///   it has the components `enchantable` and `enchantments`" / "In Java Edition, an
///   item's enchantability is stored in the `minecraft:enchantable` item component").
/// - Enchantment cost (`eCost`):
///   `eCost <- round((xpCost + 1 + randInt(floor(enchantability / 4)) + randInt(floor(enchantability / 4))) * (1 + randTri(0.15)))`
///   where `randInt(n)` draws uniformly in `0..=n`, `randTri(0.15)` draws
///   `(randFloat() + randFloat() - 1) * 0.15` with `randFloat()` in `[0, 1)`, and
///   `round` rounds to nearest integer with `.5` ties rounded towards positive infinity
///   (exact half-integer ties occur with negligible probability under continuous floats).
/// - Candidate filtering:
///   - Starts from enchantments in `#minecraft:in_enchanting_table` ("the enchanting
///     table's list of enchantments are stored in the tag
///     `enchantments:in_enchanting_table`").
///   - If `item` is not `minecraft:book`, removes enchantments whose `primary_items`
///     do not contain `item` ("If the item to enchant is not a book, all enchantments
///     to which the item is not a primary item are removed" / "Books being primary to
///     all enchantments is hardcoded").
///   - For each candidate, picks "the enchantment's highest level for which the
///     enchantment cost falls within its cost range", or removes the enchantment if no
///     level's `[min_cost, max_cost]` range contains `eCost`.
/// - Repetition loop ("The change of the enchantment cost in this final stage does not
///   retroactively change the set of candidate"):
///   1. "A random candidate is selected based on weights, meaning the chance of a
///      candidate being selected is its weight divided by the total weight of the
///      current candidates."
///   2. "The selected candidate and incompatible ones are removed from the set of
///      candidates, and if no candidates remain, the selection ends."
///   3. "If a chance of `(eCost + 1) / 50` is fails, the enchantment selection ends."
///   4. "The enchantment cost (`eCost`) is divided by two and rounded down, and the
///      process returns to step 1."
/// - Enchanted book post-step: "Finally after the loop, if the item is a book, an
///   enchanting table is used, and multiple enchantments were selected, a random one
///   is removed."
///
/// Matches the distributions documented on minecraft.wiki rather than vanilla's per-seed
/// RNG call sequence.
#[must_use]
pub fn select_enchantments(
    item: &str,
    level: u32,
    registry: &EnchantmentRegistry,
    tags: &EnchantmentTags,
    defaults: &ItemDefaults,
    random: &mut RandomSource,
) -> Vec<(String, u8)> {
    select_enchantments_inner(
        item,
        level,
        registry,
        EnchantmentPool::Table(tags),
        defaults,
        true,
        random,
    )
}

/// Selects enchantments for `item` at experience cost `level` for the
/// `minecraft:enchant_with_levels` item modifier.
///
/// Reuses the exact modified-level, primary-item, cost-range, weight, and
/// repetition selection rules of [`select_enchantments`], differing only per
/// `minecraft.wiki` `Enchanting table mechanics` (revision `3881519`) and
/// `Item modifier` (revision `3896146`):
/// - Uses `allowed_enchantments` (from `options`) when `Some`, or all registered
///   enchantments when `None` (`"If omitted, all enchantments are possible."`).
/// - Does not drop an enchantment when multiple are selected on a book
///   (`"if the item is a book, an enchanting table is used, and multiple
///   enchantments were selected, a random one is removed."`).
#[must_use]
pub fn select_enchantments_for_loot(
    item: &str,
    level: u32,
    allowed_enchantments: Option<&[String]>,
    registry: &EnchantmentRegistry,
    defaults: &ItemDefaults,
    random: &mut RandomSource,
) -> Vec<(String, u8)> {
    select_enchantments_inner(
        item,
        level,
        registry,
        EnchantmentPool::Loot(allowed_enchantments),
        defaults,
        false,
        random,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vanilla::feature::ItemTags;
    use std::collections::{BTreeMap, HashSet};
    use std::ops::RangeInclusive;
    use std::path::{Path, PathBuf};

    const SAMPLE_SEEDS: u64 = 20_000;

    fn scratch_root(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustmc-enchanting-{label}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch dir");
        path
    }

    fn write_file(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        std::fs::write(path, text).expect("write");
    }

    struct SyntheticFixtures {
        root: PathBuf,
        defaults: ItemDefaults,
        tags: EnchantmentTags,
        registry: EnchantmentRegistry,
    }

    impl Drop for SyntheticFixtures {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn build_synthetic_fixtures(
        label: &str,
        items: &[(&str, &str)],
        in_table_values: &[&str],
        enchantments: &[(&str, &str)],
        extra_tags: &[(&str, &str)],
    ) -> SyntheticFixtures {
        let root = scratch_root(label);
        for (name, body) in items {
            write_file(
                &root
                    .join("reports/minecraft/components/item")
                    .join(format!("{name}.json")),
                body,
            );
        }
        let tag_json = serde_json::json!({ "values": in_table_values }).to_string();
        write_file(
            &root.join("data/minecraft/tags/enchantment/in_enchanting_table.json"),
            &tag_json,
        );
        for (rel_path, body) in extra_tags {
            write_file(&root.join(rel_path), body);
        }
        for (name, body) in enchantments {
            write_file(
                &root
                    .join("data/testns/enchantment")
                    .join(format!("{name}.json")),
                body,
            );
        }

        let item_tags = ItemTags::load(&root).expect("load synthetic item tags");
        let tags = EnchantmentTags::load(&root).expect("load synthetic enchantment tags");
        let defaults = ItemDefaults::load(&root).expect("load synthetic item defaults");
        let registry = EnchantmentRegistry::load(&root, &item_tags, &tags)
            .expect("load synthetic enchantment registry");

        SyntheticFixtures {
            root,
            defaults,
            tags,
            registry,
        }
    }

    fn formula_bounds(bookshelves: u8) -> [RangeInclusive<u32>; 3] {
        let b = u32::from(bookshelves.min(MAX_BOOKSHELVES));
        let base_min = 1 + (b / 2);
        let base_max = 1 + 7 + (b / 2) + b;

        let top = (base_min / 3).max(1)..=(base_max / 3).max(1);
        let middle = ((2 * base_min) / 3 + 1)..=((2 * base_max) / 3 + 1);
        let bottom = base_min.max(2 * b)..=base_max.max(2 * b);
        [top, middle, bottom]
    }

    #[test]
    fn bookshelves_are_capped_at_fifteen() {
        for seed in 0..500 {
            let expected = slot_costs(MAX_BOOKSHELVES, &mut RandomSource::from_world_seed(seed));
            for extra in [16_u8, 20, 64, u8::MAX] {
                let actual = slot_costs(extra, &mut RandomSource::from_world_seed(seed));
                assert_eq!(
                    actual, expected,
                    "bookshelves={extra} must match capped bookshelves={MAX_BOOKSHELVES} at seed {seed}"
                );
            }
        }

        let bounds = formula_bounds(MAX_BOOKSHELVES);
        assert_eq!(bounds, [2..=10, 6..=21, 30..=30]);
        for seed in 0..SAMPLE_SEEDS {
            let costs = slot_costs(u8::MAX, &mut RandomSource::from_world_seed(seed));
            for (slot, (&cost, range)) in costs.iter().zip(bounds.iter()).enumerate() {
                assert!(
                    range.contains(&cost),
                    "capped slot {slot} cost {cost} outside {range:?} at seed {seed}"
                );
            }
        }
    }

    #[test]
    fn zero_bookshelves_stay_within_formula_bounds_over_20k_seeds() {
        let bounds = formula_bounds(0);
        // Pin minecraft.wiki `Enchanting table mechanics` (rev 3881519) table column for 0 bookshelves.
        assert_eq!(bounds, [1..=2, 1..=6, 1..=8]);

        let mut observed_min = [u32::MAX; 3];
        let mut observed_max = [0_u32; 3];

        for seed in 0..SAMPLE_SEEDS {
            let costs = slot_costs(0, &mut RandomSource::from_world_seed(seed));
            for slot in 0..3 {
                assert!(
                    bounds[slot].contains(&costs[slot]),
                    "0 bookshelves slot {slot} cost {} outside {:?} at seed {seed}",
                    costs[slot],
                    bounds[slot]
                );
                observed_min[slot] = observed_min[slot].min(costs[slot]);
                observed_max[slot] = observed_max[slot].max(costs[slot]);
            }
        }

        for slot in 0..3 {
            assert_eq!(observed_min[slot], *bounds[slot].start());
            assert_eq!(observed_max[slot], *bounds[slot].end());
        }
    }

    #[test]
    fn max_bookshelves_stay_within_bounds_and_reach_level_30_over_20k_seeds() {
        let bounds = formula_bounds(MAX_BOOKSHELVES);
        // Pin minecraft.wiki `Enchanting table mechanics` (rev 3881519) table column for 15 bookshelves.
        assert_eq!(bounds, [2..=10, 6..=21, 30..=30]);

        let mut observed_min = [u32::MAX; 3];
        let mut observed_max = [0_u32; 3];
        let mut reached_max = false;

        for seed in 0..SAMPLE_SEEDS {
            let costs = slot_costs(MAX_BOOKSHELVES, &mut RandomSource::from_world_seed(seed));
            for slot in 0..3 {
                assert!(
                    bounds[slot].contains(&costs[slot]),
                    "15 bookshelves slot {slot} cost {} outside {:?} at seed {seed}",
                    costs[slot],
                    bounds[slot]
                );
                observed_min[slot] = observed_min[slot].min(costs[slot]);
                observed_max[slot] = observed_max[slot].max(costs[slot]);
            }
            if costs[2] == 30 {
                reached_max = true;
            }
        }

        assert!(
            reached_max,
            "bottom slot must reach level 30 with 15 bookshelves"
        );
        for slot in 0..3 {
            assert_eq!(observed_min[slot], *bounds[slot].start());
            assert_eq!(observed_max[slot], *bounds[slot].end());
        }
    }

    #[test]
    fn intermediate_bookshelves_match_wiki_table_bounds() {
        // Pin selected columns from minecraft.wiki `Enchanting table mechanics` (rev 3881519).
        assert_eq!(formula_bounds(1), [1..=3, 1..=7, 2..=9]);
        assert_eq!(formula_bounds(5), [1..=5, 3..=11, 10..=15]);
        assert_eq!(formula_bounds(10), [2..=7, 5..=16, 20..=23]);
        assert_eq!(formula_bounds(14), [2..=9, 6..=20, 28..=29]);

        for shelves in [1_u8, 5, 10, 14] {
            let bounds = formula_bounds(shelves);
            for seed in 0..2_000 {
                let costs = slot_costs(shelves, &mut RandomSource::from_world_seed(seed));
                for slot in 0..3 {
                    assert!(
                        bounds[slot].contains(&costs[slot]),
                        "bookshelves={shelves} slot {slot} cost {} outside {:?} at seed {seed}",
                        costs[slot],
                        bounds[slot]
                    );
                }
            }
        }
    }

    #[test]
    fn select_enchantments_returns_empty_without_enchantability() {
        let fx = build_synthetic_fixtures(
            "no-enchantability",
            &[
                (
                    "stick",
                    r#"{"components": {"minecraft:max_stack_size": 64}}"#,
                ),
                (
                    "blade",
                    r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:enchantable": {"value": 10}}}"#,
                ),
            ],
            &["testns:power"],
            &[(
                "power",
                r#"{
                    "supported_items": ["minecraft:stick", "minecraft:blade"],
                    "weight": 10,
                    "max_level": 1,
                    "min_cost": {"base": 1, "per_level_above_first": 0},
                    "max_cost": {"base": 100, "per_level_above_first": 0},
                    "anvil_cost": 1,
                    "slots": ["mainhand"]
                }"#,
            )],
            &[],
        );

        for seed in 0..50 {
            let mut rng = RandomSource::from_world_seed(seed);
            assert!(
                select_enchantments(
                    "minecraft:stick",
                    30,
                    &fx.registry,
                    &fx.tags,
                    &fx.defaults,
                    &mut rng
                )
                .is_empty()
            );
            assert!(
                select_enchantments(
                    "minecraft:unknown_item",
                    30,
                    &fx.registry,
                    &fx.tags,
                    &fx.defaults,
                    &mut rng
                )
                .is_empty()
            );
        }
    }

    #[test]
    fn synthetic_candidate_filtering_highest_level_overlap_and_negative_range() {
        let fx = build_synthetic_fixtures(
            "candidate-filter",
            &[
                (
                    "tool",
                    r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:enchantable": {"value": 1}}}"#,
                ),
                (
                    "book",
                    r#"{"components": {"minecraft:max_stack_size": 64, "minecraft:enchantable": {"value": 1}}}"#,
                ),
            ],
            &[
                "testns:tiered",
                "testns:secondary_only",
                "testns:negative_lvl",
            ],
            &[
                // Not in `in_enchanting_table`: must never appear on tool or book.
                (
                    "treasure",
                    r#"{
                        "supported_items": "minecraft:tool",
                        "weight": 10,
                        "max_level": 1,
                        "min_cost": {"base": 1, "per_level_above_first": 0},
                        "max_cost": {"base": 100, "per_level_above_first": 0},
                        "anvil_cost": 1,
                        "slots": ["mainhand"]
                    }"#,
                ),
                // Secondary only for `minecraft:tool` (primary_items omits it): excluded on tool, allowed on book.
                (
                    "secondary_only",
                    r#"{
                        "supported_items": ["minecraft:tool", "minecraft:other"],
                        "primary_items": "minecraft:other",
                        "weight": 10,
                        "max_level": 1,
                        "min_cost": {"base": 1, "per_level_above_first": 0},
                        "max_cost": {"base": 100, "per_level_above_first": 0},
                        "anvil_cost": 1,
                        "slots": ["mainhand"]
                    }"#,
                ),
                // Overlapping cost ranges: L1=[1, 60], L2=[15, 70], L3=[25, 80].
                // At level=30 and enchantability=1, eCost in 26..=36 -> always selects L3.
                (
                    "tiered",
                    r#"{
                        "supported_items": "minecraft:tool",
                        "weight": 5,
                        "max_level": 3,
                        "min_cost": {"base": 1, "per_level_above_first": 12},
                        "max_cost": {"base": 60, "per_level_above_first": 10},
                        "anvil_cost": 1,
                        "slots": ["mainhand"]
                    }"#,
                ),
                // Negative range at level 1 (min=50, max=40): impossible to select.
                (
                    "negative_lvl",
                    r#"{
                        "supported_items": "minecraft:tool",
                        "weight": 10,
                        "max_level": 1,
                        "min_cost": {"base": 50, "per_level_above_first": 0},
                        "max_cost": {"base": 40, "per_level_above_first": 0},
                        "anvil_cost": 1,
                        "slots": ["mainhand"]
                    }"#,
                ),
            ],
            &[],
        );

        // On `minecraft:tool` at level 30, only `testns:tiered` is eligible, and overlapping
        // ranges pick its highest matching level (3).
        for seed in 0..200 {
            let mut rng = RandomSource::from_world_seed(seed);
            let picked = select_enchantments(
                "minecraft:tool",
                30,
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng,
            );
            assert_eq!(picked, vec![("testns:tiered".to_owned(), 3)]);
        }

        // On `minecraft:book` at level 30, `testns:secondary_only` IS eligible too,
        // and because books are primary to all `#minecraft:in_enchanting_table` entries,
        // `secondary_only` appears across seeds while `treasure` and `negative_lvl` never do.
        let mut saw_secondary = false;
        for seed in 0..500 {
            let mut rng = RandomSource::from_world_seed(seed);
            let picked = select_enchantments(
                "minecraft:book",
                30,
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng,
            );
            assert!(!picked.is_empty());
            for (id, _) in &picked {
                assert_ne!(id, "testns:treasure");
                assert_ne!(id, "testns:negative_lvl");
                if id == "testns:secondary_only" {
                    saw_secondary = true;
                }
            }
        }
        assert!(
            saw_secondary,
            "books treat all table enchantments as primary"
        );
    }

    #[test]
    fn synthetic_exclusive_set_pruning_weights_and_book_removal() {
        let fx = build_synthetic_fixtures(
            "exclusive-and-book",
            &[
                (
                    "sword",
                    r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:enchantable": {"value": 1}}}"#,
                ),
                (
                    "book",
                    r#"{"components": {"minecraft:max_stack_size": 64, "minecraft:enchantable": {"value": 1}}}"#,
                ),
            ],
            &["testns:heavy", "testns:light", "testns:compatible"],
            &[
                (
                    "heavy",
                    r##"{
                        "exclusive_set": ["testns:light"],
                        "supported_items": "minecraft:sword",
                        "weight": 10,
                        "max_level": 1,
                        "min_cost": {"base": 1, "per_level_above_first": 0},
                        "max_cost": {"base": 200, "per_level_above_first": 0},
                        "anvil_cost": 1,
                        "slots": ["mainhand"]
                    }"##,
                ),
                (
                    "light",
                    r##"{
                        "exclusive_set": ["testns:heavy"],
                        "supported_items": "minecraft:sword",
                        "weight": 2,
                        "max_level": 1,
                        "min_cost": {"base": 1, "per_level_above_first": 0},
                        "max_cost": {"base": 200, "per_level_above_first": 0},
                        "anvil_cost": 1,
                        "slots": ["mainhand"]
                    }"##,
                ),
                (
                    "compatible",
                    r##"{
                        "supported_items": "minecraft:sword",
                        "weight": 5,
                        "max_level": 1,
                        "min_cost": {"base": 1, "per_level_above_first": 0},
                        "max_cost": {"base": 200, "per_level_above_first": 0},
                        "anvil_cost": 1,
                        "slots": ["mainhand"]
                    }"##,
                ),
            ],
            &[],
        );

        let mut heavy_first_picks = 0_u32;
        let mut light_first_picks = 0_u32;
        // At level=80, eCost >= round(81 * 0.85) = 69 >= 49 so continuation after pick 1 is
        // guaranteed (probability (69 + 1) / 50 >= 1). On `sword`, 2 compatible enchantments
        // are always selected and `heavy` + `light` must never co-occur.
        for seed in 0..2_000 {
            let mut rng = RandomSource::from_world_seed(seed);
            let picked = select_enchantments(
                "minecraft:sword",
                80,
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng,
            );
            assert_eq!(picked.len(), 2);
            let ids: HashSet<&str> = picked.iter().map(|(id, _)| id.as_str()).collect();
            assert!(
                !(ids.contains("testns:heavy") && ids.contains("testns:light")),
                "exclusive enchantments must never co-occur"
            );
            if picked[0].0 == "testns:heavy" {
                heavy_first_picks += 1;
            } else if picked[0].0 == "testns:light" {
                light_first_picks += 1;
            }
        }
        assert!(
            heavy_first_picks > light_first_picks * 3,
            "weight 10 should outnumber weight 2: heavy={heavy_first_picks}, light={light_first_picks}"
        );

        // On `book` at level=80, the loop picks 2 enchantments every time, and the book
        // post-step removes one at random so the final result always has length 1.
        let mut surviving_ids = HashSet::new();
        for seed in 0..500 {
            let mut rng = RandomSource::from_world_seed(seed);
            let picked = select_enchantments(
                "minecraft:book",
                80,
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng,
            );
            assert_eq!(picked.len(), 1);
            surviving_ids.insert(picked[0].0.clone());
        }
        assert!(surviving_ids.contains("testns:compatible"));
        assert!(surviving_ids.contains("testns:heavy"));
    }

    /// Verifies enchanting-table selection on vanilla 26.3 data:
    /// diamond sword at level 30 over 20,000 seeds and unenchantable stick.
    ///
    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_select_enchantments_diamond_sword_and_stick`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_select_enchantments_diamond_sword_and_stick() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let item_tags = ItemTags::load(&root).expect("load vanilla item tags");
        let ench_tags = EnchantmentTags::load(&root).expect("load vanilla enchantment tags");
        let defaults = ItemDefaults::load(&root).expect("load vanilla item defaults");
        let registry = EnchantmentRegistry::load(&root, &item_tags, &ench_tags)
            .expect("load vanilla enchantments");

        let sharpness_weight = registry
            .get("minecraft:sharpness")
            .expect("sharpness")
            .weight();
        let fire_aspect_weight = registry
            .get("minecraft:fire_aspect")
            .expect("fire_aspect")
            .weight();
        assert_eq!(sharpness_weight, 10);
        assert_eq!(fire_aspect_weight, 2);
        assert!(sharpness_weight > fire_aspect_weight);

        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        for seed in 0..SAMPLE_SEEDS {
            let mut rng = RandomSource::from_world_seed(seed);
            let picked = select_enchantments(
                "minecraft:diamond_sword",
                30,
                &registry,
                &ench_tags,
                &defaults,
                &mut rng,
            );
            assert!(
                !picked.is_empty(),
                "level 30 diamond sword must receive at least one enchantment at seed {seed}"
            );

            for (idx, (id, level)) in picked.iter().enumerate() {
                assert!(
                    ench_tags.contains(IN_ENCHANTING_TABLE_TAG, id),
                    "enchantment {id} must be in {IN_ENCHANTING_TABLE_TAG}"
                );
                assert!(
                    registry.can_apply(id, "minecraft:diamond_sword"),
                    "enchantment {id} must apply to diamond_sword"
                );
                let def = registry.get(id).expect("registered enchantment");
                assert!(
                    def.primary_items()
                        .iter()
                        .any(|primary| items_equal(primary, "minecraft:diamond_sword")),
                    "enchantment {id} must have diamond_sword as a primary item"
                );
                assert!(
                    *level >= MIN_ENCHANTMENT_LEVEL && *level <= def.max_level(),
                    "level {level} out of bounds 1..={} for {id}",
                    def.max_level()
                );
                for (other_id, _) in &picked[(idx + 1)..] {
                    assert!(
                        registry.are_compatible(id, other_id),
                        "incompatible enchantments {id} and {other_id} co-occurred at seed {seed}"
                    );
                }
                *counts.entry(id.clone()).or_insert(0) += 1;
            }
        }

        let sharpness_count = counts.get("minecraft:sharpness").copied().unwrap_or(0);
        let fire_aspect_count = counts.get("minecraft:fire_aspect").copied().unwrap_or(0);
        assert!(fire_aspect_count > 0, "fire_aspect must appear");
        assert!(
            sharpness_count > fire_aspect_count,
            "sharpness (weight {sharpness_weight}) must appear more often than fire_aspect (weight {fire_aspect_weight}): {sharpness_count} vs {fire_aspect_count}"
        );

        for seed in 0..100 {
            let mut rng = RandomSource::from_world_seed(seed);
            assert!(
                select_enchantments(
                    "minecraft:stick",
                    30,
                    &registry,
                    &ench_tags,
                    &defaults,
                    &mut rng
                )
                .is_empty(),
                "stick has no enchantability and must receive no enchantments"
            );
        }
    }
}
