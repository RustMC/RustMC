//! Vanilla grindstone disenchanting, repairing, and experience drops.
//!
//! Semantics follow minecraft.wiki `Grindstone` (revision 3892272) and
//! `Item repair` (revision 3791643), subject to the binding reviewer
//! decisions documented below where `Grindstone` revision 3892272 contradicts
//! its own XP Drops table or leaves edge cases open:
//!
//! - **Prior work penalty reset** (`Grindstone` rev 3892272):
//!   "It resets the prior work penalty (i.e. the repair cost) for items."
//! - **Single enchanted item / enchanted book** (`Grindstone` rev 3892272):
//!   "When one enchanted item is placed in either input slot, a disenchanted
//!   copy of the item appears in the output slot. The output item has the same
//!   durability for a piece of armor or a tool. If an enchanted book is placed
//!   in the input, a normal book appears in the output. Removing the item from
//!   the output slot deletes the input item and causes the grindstone to drop
//!   some experience."
//!   - **Reviewer decision (enchanted book with remaining curses):** an
//!     `enchanted_book` whose remaining enchantments are only curses stays a
//!     `minecraft:enchanted_book` with only the curse `stored_enchantments`; it
//!     becomes `minecraft:book` only when no stored enchantments remain.
//!   - **Reviewer decision (single item with only curses):** a single item with
//!     only curses (no non-curse enchantments) produces a copy with the curses
//!     kept, `repair_cost` reset, and `0` experience.
//! - **Two items of the same type** (`Grindstone` rev 3892272):
//!   "Placing two tools or pieces of armor (enchanted or not) of the same type
//!   in the input slots causes a non-enchanted item of the same type to appear
//!   in the output slot. The output item has the same durability as what the
//!   player would get from using the item repair crafting recipe if the 2 input
//!   items were unenchanted. This means the output durability is the sum of the
//!   durabilities of the two input items, plus 5% of the maximum durability of
//!   the output item (rounded down), capped so it does not exceed the maximum
//!   durability of the output item. Both input items are deleted in this process
//!   of disenchanting them and repairing them. If either input item was
//!   enchanted, then removing the output item drops some experience. The output
//!   item has the same armor trim (if any) and name (if any) as the input item
//!   in the top slot."
//!   Also History `1.20.5` (`24w10a`): "No longer disenchants two enchanted books
//!   at the same time."
//! - **Curses and custom name** (`Grindstone` rev 3892272):
//!   "Disenchanting an item in the grindstone does not remove curse
//!   enchantments or an item's custom name."
//! - **Unenchanted single items** (`Grindstone` rev 3892272, Java Edition):
//!   "Unenchantable items can not be placed in a grindstone. Unenchanted
//!   items{{only|je}} or items with no prior work penalty{{only|be}} placed
//!   inside will have a red cross over the arrow and no item in the output
//!   slot." (Thus unenchanted single items give no output in Java Edition.)
//! - **Experience formula & wiki contradiction** (`Grindstone` rev 3892272):
//!   - Conflicting prose in `Grindstone` rev 3892272:
//!     "The amount of experience dropped by the grindstone is a uniformly
//!     distributed pseudorandom number between 50% and 100% (rounded up) of all
//!     the non-curse enchantments' minimum modified enchantment levels
//!     combined. For example, Sharpness V on average gives 35 XP, because that
//!     is 75% of 45."
//!   - Populated cells in the `XP Drops` table in `Grindstone` rev 3892272:
//!     - `Sharpness`: `I = 1–1`, `II = 6–11`, `III = 12–23`, `IV = 17–33`, `V = 23–45`
//!     - `Protection`: `I = 1–1`, `II = 6–11`, `III = 12–23`, `IV = 17–33`
//!     - `Quick Charge`: `I = 6–11`, `II = 16–31`, `III = 26–51`
//!     - For even totals `S` (such as Sharpness II `min_cost = 12` -> `6–11`
//!       and Quick Charge III `min_cost = 52` -> `26–51`), the table upper
//!       bound is `S - 1`, disagreeing with the prose's "100%"; for Sharpness V
//!       (`min_cost = 45`), the table range `23–45` has uniform mean `34`,
//!       disagreeing with the prose example's "35 XP".
//!   - **Reviewer decision (experience):** let `S` be the sum of `min_cost` at
//!     their levels of all removed non-curse enchantments (across both inputs).
//!     When `S = 0`, experience is `0`; when `S > 0`, let `k = ceil(S / 2)`,
//!     and experience is `k + randInt(k - 1)` (uniform integer in `k..=2k - 1`).
//!
//! Note on RNG parity: minecraft.wiki specifies the probability distribution
//! rather than Java Edition's exact per-call RNG consumption sequence. RustMC
//! matches the documented uniform integer distribution rather than per-seed
//! outputs.

use std::collections::BTreeMap;

use crate::item::{DEFAULT_MAX_STACK, ItemComponents, ItemDefaults, ItemStack, items_equal};
use crate::vanilla::feature::EnchantmentTags;
use crate::vanilla::random::RandomSource;
use crate::world::anvil::ENCHANTED_BOOK_ID;
use crate::world::enchantment::EnchantmentRegistry;
use crate::world::recipe_data::repair_combined_damage;

/// Enchantment tag identifying curses retained by a grindstone.
///
/// Source: minecraft.wiki `Grindstone` (revision 3892272):
/// "Disenchanting an item in the grindstone does not remove curse enchantments
/// or an item's custom name."
const CURSE_ENCHANTMENT_TAG: &str = "minecraft:curse";

/// Canonical item identifier for plain books produced when all stored
/// enchantments are stripped from an enchanted book.
///
/// Source: minecraft.wiki `Grindstone` (revision 3892272):
/// "If an enchanted book is placed in the input, a normal book appears in the
/// output."
const BOOK_ITEM_ID: &str = "minecraft:book";

/// Stack count of a grindstone output item.
///
/// Source: minecraft.wiki `Grindstone` (revision 3892272):
/// "a disenchanted copy of the item appears in the output slot" /
/// "causes a non-enchanted item of the same type to appear in the output slot."
const GRINDSTONE_RESULT_COUNT: u16 = 1;

/// Divisor in `k = ceil(S / 2)` for the grindstone experience lower bound.
///
/// Source: reviewer decision matching every populated cell of the
/// minecraft.wiki `Grindstone` (revision 3892272) `XP Drops` table
/// (`k = ceil(S / 2)`, experience uniformly distributed in `k..=2k - 1`).
const EXPERIENCE_HALF_DIVISOR: u64 = 2;

/// Outcome of evaluating a grindstone with one or two input slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrindstoneResult {
    /// Resulting disenchanted and/or repaired output stack.
    pub output: ItemStack,
    /// Experience points dropped when removing the output item.
    pub experience: u32,
}

fn is_curse(tags: &EnchantmentTags, enchantment_id: &str) -> bool {
    tags.contains(CURSE_ENCHANTMENT_TAG, enchantment_id)
}

fn partition_enchantments(
    enchantments: &BTreeMap<String, u8>,
    registry: &EnchantmentRegistry,
    tags: &EnchantmentTags,
) -> (BTreeMap<String, u8>, u64) {
    let mut kept_curses = BTreeMap::new();
    let mut removed_min_cost_sum = 0_u64;
    for (id, &level) in enchantments {
        if is_curse(tags, id) {
            kept_curses.insert(id.clone(), level);
        } else if let Some(def) = registry.get(id) {
            let cost = def.min_cost().at_level(level);
            if cost > 0 {
                removed_min_cost_sum =
                    removed_min_cost_sum.saturating_add(u64::from(cost.cast_unsigned()));
            }
        }
    }
    (kept_curses, removed_min_cost_sum)
}

fn roll_experience(total_min_cost: u64, random: &mut RandomSource) -> u32 {
    if total_min_cost == 0 {
        return 0;
    }
    let min_xp = total_min_cost.div_ceil(EXPERIENCE_HALF_DIVISOR);
    let bound = u32::try_from(min_xp).unwrap_or(u32::MAX);
    bound.saturating_add(random.next_int_bounded(bound))
}

fn single_item_result(
    input: &ItemStack,
    registry: &EnchantmentRegistry,
    tags: &EnchantmentTags,
    defaults: &ItemDefaults,
    random: &mut RandomSource,
) -> Option<GrindstoneResult> {
    let is_enchanted_book = items_equal(input.item(), ENCHANTED_BOOK_ID);
    if is_enchanted_book {
        if input.components().stored_enchantments().is_empty()
            && input.components().enchantments().is_empty()
        {
            return None;
        }
        let (kept_stored, stored_cost) =
            partition_enchantments(input.components().stored_enchantments(), registry, tags);
        let (kept_active, active_cost) =
            partition_enchantments(input.components().enchantments(), registry, tags);
        let total_min_cost = stored_cost.saturating_add(active_cost);

        let output = if kept_stored.is_empty() && kept_active.is_empty() {
            let book_max_stack = defaults
                .max_stack_size(BOOK_ITEM_ID)
                .map_or(DEFAULT_MAX_STACK, u16::from);
            let mut components = ItemComponents::new();
            if let Some(name) = input.components().custom_name() {
                components.set_custom_name(Some(name.to_owned()));
            }
            ItemStack::try_new(BOOK_ITEM_ID, input.count(), book_max_stack)
                .ok()?
                .with_components(components)
                .ok()?
        } else {
            let mut components = input.components().clone();
            components.set_stored_enchantments(kept_stored).ok()?;
            components.set_enchantments(kept_active).ok()?;
            components.set_repair_cost(None);
            input.clone().with_components(components).ok()?
        };

        return Some(GrindstoneResult {
            output,
            experience: roll_experience(total_min_cost, random),
        });
    }

    if input.components().enchantments().is_empty() {
        return None;
    }

    let (kept_curses, total_min_cost) =
        partition_enchantments(input.components().enchantments(), registry, tags);
    let mut components = input.components().clone();
    components.set_enchantments(kept_curses).ok()?;
    components.set_repair_cost(None);
    let output = input.clone().with_components(components).ok()?;

    Some(GrindstoneResult {
        output,
        experience: roll_experience(total_min_cost, random),
    })
}

fn two_item_result(
    top: &ItemStack,
    bottom: &ItemStack,
    registry: &EnchantmentRegistry,
    tags: &EnchantmentTags,
    defaults: &ItemDefaults,
    random: &mut RandomSource,
) -> Option<GrindstoneResult> {
    if !items_equal(top.item(), bottom.item()) {
        return None;
    }
    if top.count() != GRINDSTONE_RESULT_COUNT || bottom.count() != GRINDSTONE_RESULT_COUNT {
        return None;
    }
    let max_damage = defaults.max_damage(top.item()).filter(|&max| max > 0)?;
    let damage_top = top.components().damage().unwrap_or(0);
    let damage_bottom = bottom.components().damage().unwrap_or(0);
    let result_damage = repair_combined_damage(max_damage, damage_top, damage_bottom)?;

    let (mut merged_curses, cost_top) =
        partition_enchantments(top.components().enchantments(), registry, tags);
    let (curses_bottom, cost_bottom) =
        partition_enchantments(bottom.components().enchantments(), registry, tags);
    for (id, level) in curses_bottom {
        merged_curses
            .entry(id)
            .and_modify(|existing| *existing = (*existing).max(level))
            .or_insert(level);
    }

    let mut components = ItemComponents::new();
    if result_damage > 0 {
        components.set_damage(Some(result_damage));
    }
    if let Some(name) = top.components().custom_name() {
        components.set_custom_name(Some(name.to_owned()));
    }
    if let Some(trim) = top.components().trim() {
        components.set_trim(Some(trim.clone()));
    }
    components.set_enchantments(merged_curses).ok()?;

    let max_stack = defaults
        .max_stack_size(top.item())
        .map_or_else(|| top.max_stack(), u16::from);
    let output = ItemStack::try_new(top.item(), GRINDSTONE_RESULT_COUNT, max_stack)
        .ok()?
        .with_components(components)
        .ok()?;

    let total_min_cost = cost_top.saturating_add(cost_bottom);
    Some(GrindstoneResult {
        output,
        experience: roll_experience(total_min_cost, random),
    })
}

/// Evaluates the grindstone output item and experience drop for `top` and `bottom` inputs.
///
/// Follows minecraft.wiki `Grindstone` (revision 3892272), `Item repair`
/// (revision 3791643), and the module-level reviewer decisions:
/// - **No inputs** or **different item kinds**: returns `None`.
/// - **Single item** (in either `top` or `bottom`):
///   - Unenchanted items give no output (`None`), even if `repair_cost` is non-zero.
///   - Non-curse enchantments are removed and contribute their `min_cost` at level
///     to the experience sum `S`; curses (`#minecraft:curse`), `custom_name`, `damage`,
///     and `trim` are preserved, and `repair_cost` is reset.
///   - Single items with only curses yield a copy with curses kept, `repair_cost`
///     reset, and `0` experience.
///   - `minecraft:enchanted_book` with remaining curse `stored_enchantments` stays
///     a `minecraft:enchanted_book` with only those curses; it becomes
///     `minecraft:book` only when no stored enchantments remain.
/// - **Two items of the same damageable kind** (`minecraft:max_damage > 0` in
///   `defaults`):
///   - Combines durability via [`repair_combined_damage`] (the crafting item-repair
///     rule: `min(uses_top + uses_bottom + floor(max_damage / 20), max_damage)`).
///   - Removes non-curse enchantments from both inputs (summing their `min_cost`
///     at level into `S`), keeps curses from both, resets `repair_cost`, and keeps
///     the `top` input's `custom_name` and `trim`.
///   - Rejects non-damageable pairs (such as two `minecraft:enchanted_book`s).
/// - **Experience**: `0` when `S == 0`, otherwise `k + randInt(k - 1)` uniformly in
///   `k..=2k - 1` where `k = ceil(S / 2)`.
///
/// Matches the distributions documented on minecraft.wiki rather than vanilla's
/// per-seed RNG call sequence.
#[must_use]
pub fn grindstone_result(
    top: Option<&ItemStack>,
    bottom: Option<&ItemStack>,
    registry: &EnchantmentRegistry,
    tags: &EnchantmentTags,
    defaults: &ItemDefaults,
    random: &mut RandomSource,
) -> Option<GrindstoneResult> {
    match (top, bottom) {
        (None, None) => None,
        (Some(single), None) | (None, Some(single)) => {
            single_item_result(single, registry, tags, defaults, random)
        }
        (Some(top_item), Some(bottom_item)) => {
            two_item_result(top_item, bottom_item, registry, tags, defaults, random)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ArmorTrim;
    use crate::vanilla::feature::ItemTags;
    use crate::world::crafting::CraftingGrid;
    use crate::world::recipe_data::{CRAFTING_REPAIR_BONUS_DIVISOR, RecipeData};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    const SAMPLE_SEEDS: u64 = 20_000;

    fn scratch_root(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustmc-grindstone-{label}-{}-{n}",
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

    fn build_synthetic_fixtures(label: &str) -> SyntheticFixtures {
        let root = scratch_root(label);
        let items = [
            (
                "iron_sword",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:max_damage": 250, "minecraft:enchantable": {"value": 14}}}"#,
            ),
            (
                "iron_pickaxe",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:max_damage": 250, "minecraft:enchantable": {"value": 14}}}"#,
            ),
            (
                "diamond_sword",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:max_damage": 1561, "minecraft:enchantable": {"value": 10}}}"#,
            ),
            (
                "iron_chestplate",
                r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:max_damage": 240, "minecraft:enchantable": {"value": 9}}}"#,
            ),
            (
                "book",
                r#"{"components": {"minecraft:max_stack_size": 64, "minecraft:enchantable": {"value": 1}}}"#,
            ),
            (
                "enchanted_book",
                r#"{"components": {"minecraft:max_stack_size": 1}}"#,
            ),
            (
                "stick",
                r#"{"components": {"minecraft:max_stack_size": 64}}"#,
            ),
        ];
        for (name, body) in items {
            write_file(
                &root
                    .join("reports/minecraft/components/item")
                    .join(format!("{name}.json")),
                body,
            );
        }
        write_file(
            &root.join("data/minecraft/tags/enchantment/curse.json"),
            r#"{"values": ["minecraft:binding_curse", "minecraft:vanishing_curse"]}"#,
        );
        let enchantments = [
            (
                "sharpness",
                r#"{
                    "supported_items": ["minecraft:iron_sword", "minecraft:diamond_sword"],
                    "weight": 10,
                    "max_level": 5,
                    "min_cost": {"base": 1, "per_level_above_first": 11},
                    "max_cost": {"base": 21, "per_level_above_first": 11},
                    "anvil_cost": 1,
                    "slots": ["mainhand"]
                }"#,
            ),
            (
                "smite",
                r#"{
                    "supported_items": ["minecraft:iron_sword", "minecraft:diamond_sword"],
                    "weight": 5,
                    "max_level": 5,
                    "min_cost": {"base": 5, "per_level_above_first": 8},
                    "max_cost": {"base": 25, "per_level_above_first": 8},
                    "anvil_cost": 2,
                    "slots": ["mainhand"]
                }"#,
            ),
            (
                "efficiency",
                r#"{
                    "supported_items": "minecraft:iron_pickaxe",
                    "weight": 10,
                    "max_level": 5,
                    "min_cost": {"base": 1, "per_level_above_first": 10},
                    "max_cost": {"base": 51, "per_level_above_first": 10},
                    "anvil_cost": 1,
                    "slots": ["mainhand"]
                }"#,
            ),
            (
                "protection",
                r#"{
                    "supported_items": "minecraft:iron_chestplate",
                    "weight": 10,
                    "max_level": 4,
                    "min_cost": {"base": 1, "per_level_above_first": 11},
                    "max_cost": {"base": 12, "per_level_above_first": 11},
                    "anvil_cost": 1,
                    "slots": ["armor"]
                }"#,
            ),
            (
                "quick_charge",
                r#"{
                    "supported_items": "minecraft:iron_sword",
                    "weight": 5,
                    "max_level": 3,
                    "min_cost": {"base": 12, "per_level_above_first": 20},
                    "max_cost": {"base": 50, "per_level_above_first": 0},
                    "anvil_cost": 2,
                    "slots": ["mainhand"]
                }"#,
            ),
            (
                "vanishing_curse",
                r#"{
                    "supported_items": ["minecraft:iron_sword", "minecraft:iron_pickaxe", "minecraft:iron_chestplate"],
                    "weight": 1,
                    "max_level": 1,
                    "min_cost": {"base": 25, "per_level_above_first": 0},
                    "max_cost": {"base": 50, "per_level_above_first": 0},
                    "anvil_cost": 8,
                    "slots": ["any"]
                }"#,
            ),
            (
                "binding_curse",
                r#"{
                    "supported_items": "minecraft:iron_chestplate",
                    "weight": 1,
                    "max_level": 1,
                    "min_cost": {"base": 25, "per_level_above_first": 0},
                    "max_cost": {"base": 50, "per_level_above_first": 0},
                    "anvil_cost": 8,
                    "slots": ["armor"]
                }"#,
            ),
        ];
        for (name, body) in enchantments {
            write_file(
                &root
                    .join("data/minecraft/enchantment")
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

    fn expected_xp_bounds(sum_min_cost: u32) -> (u32, u32) {
        let k = sum_min_cost.div_ceil(2);
        (k, 2 * k - 1)
    }

    #[test]
    fn single_sword_with_sharpness_iii_and_vanishing_curse_keeps_curse_and_drops_formula_xp() {
        let fx = build_synthetic_fixtures("sword-sharp-curse");
        let sword = ItemStack::try_new("minecraft:iron_sword", 1, 1)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_damage(42)
                    .with_custom_name("Hexblade")
                    .with_repair_cost(3)
                    .with_enchantment("minecraft:sharpness", 3)
                    .unwrap()
                    .with_enchantment("minecraft:vanishing_curse", 1)
                    .unwrap(),
            )
            .unwrap();

        // Sharpness III min_cost = 1 + 11 * 2 = 23 -> k = ceil(23 / 2) = 12, range 12..=23.
        let (min_xp, max_xp) = expected_xp_bounds(23);
        assert_eq!((min_xp, max_xp), (12, 23));

        let mut seen = HashSet::new();
        let mut total_xp = 0_u64;
        for seed in 0..SAMPLE_SEEDS {
            let mut rng_top = RandomSource::from_world_seed(seed);
            let res_top = grindstone_result(
                Some(&sword),
                None,
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng_top,
            )
            .expect("top-slot disenchant");

            let mut rng_bottom = RandomSource::from_world_seed(seed);
            let res_bottom = grindstone_result(
                None,
                Some(&sword),
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng_bottom,
            )
            .expect("bottom-slot disenchant");

            assert_eq!(res_top, res_bottom);
            assert_eq!(res_top.output.item(), "minecraft:iron_sword");
            assert_eq!(res_top.output.components().damage(), Some(42));
            assert_eq!(res_top.output.components().custom_name(), Some("Hexblade"));
            assert_eq!(res_top.output.components().repair_cost(), None);
            assert_eq!(
                res_top
                    .output
                    .components()
                    .enchantments()
                    .get("minecraft:sharpness"),
                None
            );
            assert_eq!(
                res_top
                    .output
                    .components()
                    .enchantments()
                    .get("minecraft:vanishing_curse"),
                Some(&1)
            );
            assert!(
                (min_xp..=max_xp).contains(&res_top.experience),
                "xp {} outside {min_xp}..={max_xp} at seed {seed}",
                res_top.experience
            );
            seen.insert(res_top.experience);
            total_xp += u64::from(res_top.experience);
        }

        for xp in min_xp..=max_xp {
            assert!(seen.contains(&xp), "uniform distribution missed xp={xp}");
        }
        let mean = total_xp as f64 / SAMPLE_SEEDS as f64;
        let expected_mean = f64::from(min_xp + max_xp) / 2.0;
        assert!(
            (mean - expected_mean).abs() < 0.2,
            "mean {mean} deviated from expected {expected_mean}"
        );
    }

    #[test]
    fn single_item_with_only_curses_and_unenchanted_rejections() {
        let fx = build_synthetic_fixtures("only-curses-and-empty");
        let cursed_only = ItemStack::try_new("minecraft:iron_sword", 1, 1)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_damage(15)
                    .with_custom_name("Doomed")
                    .with_repair_cost(7)
                    .with_enchantment("minecraft:vanishing_curse", 1)
                    .unwrap(),
            )
            .unwrap();

        let mut rng = RandomSource::from_world_seed(42);
        let res = grindstone_result(
            Some(&cursed_only),
            None,
            &fx.registry,
            &fx.tags,
            &fx.defaults,
            &mut rng,
        )
        .expect("curse-only item yields copy with reset repair_cost");
        assert_eq!(res.experience, 0);
        assert_eq!(res.output.components().repair_cost(), None);
        assert_eq!(res.output.components().damage(), Some(15));
        assert_eq!(res.output.components().custom_name(), Some("Doomed"));
        assert_eq!(
            res.output
                .components()
                .enchantments()
                .get("minecraft:vanishing_curse"),
            Some(&1)
        );

        let plain_sword = ItemStack::try_new("minecraft:iron_sword", 1, 1)
            .unwrap()
            .with_components(ItemComponents::new().with_damage(50).with_repair_cost(5))
            .unwrap();
        let plain_book = ItemStack::try_new("minecraft:book", 1, 64).unwrap();
        let empty_enchanted_book = ItemStack::try_new("minecraft:enchanted_book", 1, 1).unwrap();
        let stick = ItemStack::try_new("minecraft:stick", 1, 64).unwrap();

        assert_eq!(
            grindstone_result(None, None, &fx.registry, &fx.tags, &fx.defaults, &mut rng),
            None
        );
        for unenchanted in [&plain_sword, &plain_book, &empty_enchanted_book, &stick] {
            assert_eq!(
                grindstone_result(
                    Some(unenchanted),
                    None,
                    &fx.registry,
                    &fx.tags,
                    &fx.defaults,
                    &mut rng
                ),
                None
            );
            assert_eq!(
                grindstone_result(
                    None,
                    Some(unenchanted),
                    &fx.registry,
                    &fx.tags,
                    &fx.defaults,
                    &mut rng
                ),
                None
            );
        }

        // Different item kinds and non-damageable pairs cannot be combined.
        let diamond_sword = ItemStack::try_new("minecraft:diamond_sword", 1, 1).unwrap();
        assert_eq!(
            grindstone_result(
                Some(&plain_sword),
                Some(&diamond_sword),
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng
            ),
            None
        );
        assert_eq!(
            grindstone_result(
                Some(&stick),
                Some(&stick),
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng
            ),
            None
        );
    }

    #[test]
    fn enchanted_book_stored_enchantments_conversion_and_curse_retention() {
        let fx = build_synthetic_fixtures("books");

        // 1. Non-curse only -> becomes `minecraft:book`, keeps custom_name, resets repair_cost.
        let sharp_book = ItemStack::try_new("minecraft:enchanted_book", 1, 1)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_custom_name("Manual of Edge")
                    .with_repair_cost(3)
                    .with_stored_enchantment("minecraft:sharpness", 5)
                    .unwrap(),
            )
            .unwrap();
        // Sharpness V min_cost = 45 -> k = ceil(45 / 2) = 23, range 23..=45, mean 34.0.
        let (min_xp, max_xp) = expected_xp_bounds(45);
        assert_eq!((min_xp, max_xp), (23, 45));

        let mut xp_sum = 0_u64;
        for seed in 0..SAMPLE_SEEDS {
            let mut rng = RandomSource::from_world_seed(seed);
            let res = grindstone_result(
                Some(&sharp_book),
                None,
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng,
            )
            .expect("disenchant sharpness V book");
            assert_eq!(res.output.item(), "minecraft:book");
            assert_eq!(res.output.max_stack(), 64);
            assert_eq!(res.output.count(), 1);
            assert_eq!(
                res.output.components().custom_name(),
                Some("Manual of Edge")
            );
            assert_eq!(res.output.components().repair_cost(), None);
            assert!(res.output.components().stored_enchantments().is_empty());
            assert!(res.output.components().enchantments().is_empty());
            assert!((min_xp..=max_xp).contains(&res.experience));
            xp_sum += u64::from(res.experience);
        }
        let mean = xp_sum as f64 / SAMPLE_SEEDS as f64;
        assert!(
            (mean - 34.0).abs() < 0.25,
            "Sharpness V uniform 23..=45 mean should be ~34.0, got {mean}"
        );

        // 2. Non-curse + curse -> stays `minecraft:enchanted_book` with only curse stored.
        let mixed_book = ItemStack::try_new("minecraft:enchanted_book", 1, 1)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_custom_name("Mixed Tome")
                    .with_repair_cost(1)
                    .with_stored_enchantment("minecraft:sharpness", 2)
                    .unwrap()
                    .with_stored_enchantment("minecraft:binding_curse", 1)
                    .unwrap(),
            )
            .unwrap();
        // Sharpness II min_cost = 12 (even): k = 6 -> 6..=11 (NOT 6..=12).
        let mut seen = HashSet::new();
        for seed in 0..2_000 {
            let mut rng = RandomSource::from_world_seed(seed);
            let res = grindstone_result(
                Some(&mixed_book),
                None,
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng,
            )
            .expect("mixed book");
            assert_eq!(res.output.item(), "minecraft:enchanted_book");
            assert_eq!(res.output.components().custom_name(), Some("Mixed Tome"));
            assert_eq!(res.output.components().repair_cost(), None);
            assert_eq!(res.output.components().stored_enchantments().len(), 1);
            assert_eq!(
                res.output
                    .components()
                    .stored_enchantments()
                    .get("minecraft:binding_curse"),
                Some(&1)
            );
            assert!((6..=11).contains(&res.experience));
            seen.insert(res.experience);
        }
        assert_eq!(seen, (6..=11).collect::<HashSet<u32>>());

        // 3. Curse only -> stays `minecraft:enchanted_book` with 0 XP.
        let curse_book = ItemStack::try_new("minecraft:enchanted_book", 1, 1)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_repair_cost(7)
                    .with_stored_enchantment("minecraft:vanishing_curse", 1)
                    .unwrap(),
            )
            .unwrap();
        let mut rng = RandomSource::from_world_seed(1);
        let curse_res = grindstone_result(
            Some(&curse_book),
            None,
            &fx.registry,
            &fx.tags,
            &fx.defaults,
            &mut rng,
        )
        .expect("curse-only enchanted book");
        assert_eq!(curse_res.output.item(), "minecraft:enchanted_book");
        assert_eq!(curse_res.experience, 0);
        assert_eq!(curse_res.output.components().repair_cost(), None);
        assert_eq!(
            curse_res
                .output
                .components()
                .stored_enchantments()
                .get("minecraft:vanishing_curse"),
            Some(&1)
        );

        // 4. Two enchanted books cannot be combined/disenchanted together.
        assert_eq!(
            grindstone_result(
                Some(&sharp_book),
                Some(&mixed_book),
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng
            ),
            None
        );
    }

    #[test]
    fn two_damaged_items_reuse_crafting_repair_rule_keep_top_name_and_trim_and_sum_xp() {
        let fx = build_synthetic_fixtures("two-items");
        let top_trim = ArmorTrim::try_new("minecraft:coast", "minecraft:gold").unwrap();
        let bottom_trim = ArmorTrim::try_new("minecraft:sentry", "minecraft:emerald").unwrap();

        // Iron chestplate max_damage = 240 -> 5% bonus = 240 / 20 = 12.
        // Top damage 190 (50 uses), bottom damage 140 (100 uses) -> 50 + 100 + 12 = 162 uses -> damage 78.
        let top_plate = ItemStack::try_new("minecraft:iron_chestplate", 1, 1)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_damage(190)
                    .with_custom_name("Top Plate")
                    .with_trim(top_trim.clone())
                    .with_repair_cost(7)
                    .with_enchantment("minecraft:protection", 2)
                    .unwrap()
                    .with_enchantment("minecraft:binding_curse", 1)
                    .unwrap(),
            )
            .unwrap();
        let bottom_plate = ItemStack::try_new("minecraft:iron_chestplate", 1, 1)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_damage(140)
                    .with_custom_name("Bottom Plate")
                    .with_trim(bottom_trim)
                    .with_repair_cost(3)
                    .with_enchantment("minecraft:protection", 1)
                    .unwrap()
                    .with_enchantment("minecraft:vanishing_curse", 1)
                    .unwrap(),
            )
            .unwrap();

        // Removed non-curses: Protection II (min=12) + Protection I (min=1) -> S = 13 -> k = 7 -> 7..=13.
        let mut seen = HashSet::new();
        for seed in 0..2_000 {
            let mut rng = RandomSource::from_world_seed(seed);
            let res = grindstone_result(
                Some(&top_plate),
                Some(&bottom_plate),
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng,
            )
            .expect("combine two chestplates");
            assert_eq!(res.output.item(), "minecraft:iron_chestplate");
            assert_eq!(res.output.components().damage(), Some(78));
            assert_eq!(res.output.components().custom_name(), Some("Top Plate"));
            assert_eq!(res.output.components().trim(), Some(&top_trim));
            assert_eq!(res.output.components().repair_cost(), None);
            assert_eq!(res.output.components().enchantments().len(), 2);
            assert_eq!(
                res.output
                    .components()
                    .enchantments()
                    .get("minecraft:binding_curse"),
                Some(&1)
            );
            assert_eq!(
                res.output
                    .components()
                    .enchantments()
                    .get("minecraft:vanishing_curse"),
                Some(&1)
            );
            assert!((7..=13).contains(&res.experience));
            seen.insert(res.experience);
        }
        assert_eq!(seen, (7..=13).collect::<HashSet<u32>>());

        // When top has no name/trim and bottom does, output keeps top's (None).
        let bare_top = ItemStack::try_new("minecraft:iron_chestplate", 1, 1)
            .unwrap()
            .with_components(ItemComponents::new().with_damage(10))
            .unwrap();
        let mut rng = RandomSource::from_world_seed(0);
        let capped = grindstone_result(
            Some(&bare_top),
            Some(&bottom_plate),
            &fx.registry,
            &fx.tags,
            &fx.defaults,
            &mut rng,
        )
        .expect("bare top + enchanted bottom");
        // Uses 230 + 100 + 12 >= 240 -> capped to full durability (`damage = None`).
        assert_eq!(capped.output.components().damage(), None);
        assert_eq!(capped.output.components().custom_name(), None);
        assert_eq!(capped.output.components().trim(), None);
        // Protection I min_cost = 1 -> k = 1 -> 1..=1.
        assert_eq!(capped.experience, 1);
    }

    #[test]
    fn quick_charge_iii_matches_even_sum_xp_table_bounds() {
        let fx = build_synthetic_fixtures("quick-charge-even");
        let sword = ItemStack::try_new("minecraft:iron_sword", 1, 1)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_enchantment("minecraft:quick_charge", 3)
                    .unwrap(),
            )
            .unwrap();
        // Quick Charge III min_cost = 12 + 20 * 2 = 52 -> k = 26 -> 26..=51 (wiki XP Drops table).
        let mut min_seen = u32::MAX;
        let mut max_seen = 0_u32;
        for seed in 0..SAMPLE_SEEDS {
            let mut rng = RandomSource::from_world_seed(seed);
            let res = grindstone_result(
                Some(&sword),
                None,
                &fx.registry,
                &fx.tags,
                &fx.defaults,
                &mut rng,
            )
            .expect("quick charge III");
            assert!((26..=51).contains(&res.experience));
            min_seen = min_seen.min(res.experience);
            max_seen = max_seen.max(res.experience);
        }
        assert_eq!((min_seen, max_seen), (26, 51));
    }

    /// Verifies grindstone disenchanting, crafting-repair durability parity,
    /// enchanted book handling, and XP drop ranges on vanilla 26.3 data.
    ///
    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_grindstone_disenchants_repairs_and_matches_vanilla_xp`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_grindstone_disenchants_repairs_and_matches_vanilla_xp() {
        const IRON_PICKAXE_MAX_DAMAGE: u32 = 250;

        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let item_tags = ItemTags::load(&root).expect("load vanilla item tags");
        let ench_tags = EnchantmentTags::load(&root).expect("load vanilla enchantment tags");
        let defaults = ItemDefaults::load(&root).expect("load vanilla item defaults");
        let registry = EnchantmentRegistry::load(&root, &item_tags, &ench_tags)
            .expect("load vanilla enchantments");
        let recipes = RecipeData::load_with_items(&root, &item_tags, &defaults)
            .expect("load vanilla recipes");

        // 1. Iron sword with Sharpness III + Curse of Vanishing -> no Sharpness, curse kept,
        //    repair_cost reset, XP in 12..=23 (wiki XP Drops table).
        let sword = ItemStack::try_new("minecraft:iron_sword", 1, 1)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_damage(50)
                    .with_custom_name("Blade")
                    .with_repair_cost(3)
                    .with_enchantment("minecraft:sharpness", 3)
                    .unwrap()
                    .with_enchantment("minecraft:vanishing_curse", 1)
                    .unwrap(),
            )
            .unwrap();
        let mut seen = HashSet::new();
        for seed in 0..SAMPLE_SEEDS {
            let mut rng = RandomSource::from_world_seed(seed);
            let res = grindstone_result(
                Some(&sword),
                None,
                &registry,
                &ench_tags,
                &defaults,
                &mut rng,
            )
            .expect("disenchant iron sword");
            assert_eq!(res.output.item(), "minecraft:iron_sword");
            assert_eq!(res.output.components().damage(), Some(50));
            assert_eq!(res.output.components().custom_name(), Some("Blade"));
            assert_eq!(res.output.components().repair_cost(), None);
            assert_eq!(
                res.output
                    .components()
                    .enchantments()
                    .get("minecraft:sharpness"),
                None
            );
            assert_eq!(
                res.output
                    .components()
                    .enchantments()
                    .get("minecraft:vanishing_curse"),
                Some(&1)
            );
            assert!((12..=23).contains(&res.experience));
            seen.insert(res.experience);
        }
        assert_eq!(seen, (12..=23).collect::<HashSet<u32>>());

        // 2. Two damaged pickaxes combine with the exact same durability and curse rules
        //    as `minecraft:repair_item`, while keeping top input's custom name and trim.
        assert_eq!(
            defaults.max_damage("minecraft:iron_pickaxe"),
            Some(IRON_PICKAXE_MAX_DAMAGE)
        );
        let pick_top = ItemStack::try_new("minecraft:iron_pickaxe", 1, 1)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_damage(180)
                    .with_custom_name("Miner Top")
                    .with_repair_cost(3)
                    .with_enchantment("minecraft:efficiency", 3)
                    .unwrap(),
            )
            .unwrap();
        let pick_bottom = ItemStack::try_new("minecraft:iron_pickaxe", 1, 1)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_damage(110)
                    .with_custom_name("Miner Bottom")
                    .with_enchantment("minecraft:unbreaking", 2)
                    .unwrap()
                    .with_enchantment("minecraft:vanishing_curse", 1)
                    .unwrap(),
            )
            .unwrap();

        let mut grid = CraftingGrid::table_3x3();
        grid.set_slot(0, 0, pick_top.clone()).unwrap();
        grid.set_slot(1, 1, pick_bottom.clone()).unwrap();
        let (recipe_id, crafted) = recipes
            .find_crafting(&grid)
            .expect("crafting repair iron pickaxes");
        assert_eq!(recipe_id, "minecraft:repair_item");
        let expected_uses = (IRON_PICKAXE_MAX_DAMAGE - 180)
            + (IRON_PICKAXE_MAX_DAMAGE - 110)
            + (IRON_PICKAXE_MAX_DAMAGE / CRAFTING_REPAIR_BONUS_DIVISOR);
        assert_eq!(
            crafted.components().damage(),
            Some(IRON_PICKAXE_MAX_DAMAGE - expected_uses)
        );

        // Removed non-curses: Efficiency III (min=21) + Unbreaking II (min=13) -> S = 34 -> k = 17 -> 17..=33.
        let mut pick_seen = HashSet::new();
        for seed in 0..SAMPLE_SEEDS {
            let mut rng = RandomSource::from_world_seed(seed);
            let ground = grindstone_result(
                Some(&pick_top),
                Some(&pick_bottom),
                &registry,
                &ench_tags,
                &defaults,
                &mut rng,
            )
            .expect("grindstone repair iron pickaxes");
            assert_eq!(ground.output.item(), crafted.item());
            assert_eq!(
                ground.output.components().damage(),
                crafted.components().damage()
            );
            assert_eq!(
                ground.output.components().enchantments(),
                crafted.components().enchantments()
            );
            assert_eq!(ground.output.components().repair_cost(), None);
            assert_eq!(ground.output.components().custom_name(), Some("Miner Top"));
            assert!((17..=33).contains(&ground.experience));
            pick_seen.insert(ground.experience);
        }
        assert_eq!(pick_seen, (17..=33).collect::<HashSet<u32>>());

        // 3. Pin vanilla 26.3 registry XP Drop ranges against wiki table entries via enchanted books.
        let wiki_cells = [
            ("minecraft:sharpness", 1_u8, 1_u32..=1),
            ("minecraft:sharpness", 2, 6..=11),
            ("minecraft:sharpness", 5, 23..=45),
            ("minecraft:protection", 4, 17..=33),
            ("minecraft:quick_charge", 3, 26..=51),
            ("minecraft:swift_sneak", 3, 38..=75),
        ];
        for (ench_id, level, range) in wiki_cells {
            let book = ItemStack::try_new("minecraft:enchanted_book", 1, 1)
                .unwrap()
                .with_components(
                    ItemComponents::new()
                        .with_stored_enchantment(ench_id, level)
                        .unwrap(),
                )
                .unwrap();
            let mut rng = RandomSource::from_world_seed(7);
            let res = grindstone_result(
                Some(&book),
                None,
                &registry,
                &ench_tags,
                &defaults,
                &mut rng,
            )
            .expect("disenchant book");
            assert_eq!(res.output.item(), "minecraft:book");
            assert!(
                range.contains(&res.experience),
                "{ench_id} {level} xp {} outside {range:?}",
                res.experience
            );
        }

        // Enchanted book with curse stays `minecraft:enchanted_book`.
        let cursed_book = ItemStack::try_new("minecraft:enchanted_book", 1, 1)
            .unwrap()
            .with_components(
                ItemComponents::new()
                    .with_repair_cost(1)
                    .with_stored_enchantment("minecraft:sharpness", 1)
                    .unwrap()
                    .with_stored_enchantment("minecraft:binding_curse", 1)
                    .unwrap(),
            )
            .unwrap();
        let mut rng = RandomSource::from_world_seed(0);
        let cursed_book_res = grindstone_result(
            Some(&cursed_book),
            None,
            &registry,
            &ench_tags,
            &defaults,
            &mut rng,
        )
        .expect("book with curse stays enchanted_book");
        assert_eq!(cursed_book_res.output.item(), "minecraft:enchanted_book");
        assert_eq!(cursed_book_res.experience, 1);
        assert_eq!(cursed_book_res.output.components().repair_cost(), None);
        assert_eq!(
            cursed_book_res
                .output
                .components()
                .stored_enchantments()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["minecraft:binding_curse"]
        );
    }
}
