//! Vanilla enchanting table level-cost selection.
//!
//! Semantics follow minecraft.wiki `Enchanting table mechanics` (revision 3881519),
//! `Enchanting Table` (revision 3892267), and `Bookshelf` (revision 3775299).
//!
//! Note on RNG parity: minecraft.wiki specifies the probability distributions rather than
//! Java Edition's exact per-call RNG consumption sequence. RustMC matches those documented
//! distributions rather than per-seed outputs.

use crate::vanilla::random::RandomSource;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::RangeInclusive;

    const SAMPLE_SEEDS: u64 = 20_000;

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
}
