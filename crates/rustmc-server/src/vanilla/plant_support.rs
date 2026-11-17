//! What a plant needs from the ground before a `minecraft:simple_block`
//! feature may write it.
//!
//! The pack's ground-cover chains filter a candidate on open air and nothing
//! else, so the ground check lives in the placement itself. The pack names the
//! ground as block tags (`minecraft:supports_vegetation`,
//! `minecraft:supports_dry_vegetation`) but not which plant reads which tag.
//!
//! That pairing is observed, not derived: the rows below come from the block
//! under every stored plant in 14,422 full overworld chunks of a seed-2026
//! Java 26.3 save, read with an independent Anvil decoder. For example,
//! `short_grass` stood on the vegetation tag for 190,673 of 190,766 rows and
//! `dead_bush` on the dry tag for 342 of 343; what remains is player paths and
//! later features overlaying the ground. `leaf_litter` (on stone and sand),
//! the mushrooms and the cave plants do not fit either tag, and a plant the
//! save did not hold has no evidence at all, so neither has a row here. A
//! state with no row is not guessed at: the feature asking for it stays
//! unmodeled and the gap report names the state.

use super::feature::{BlockTags, DecorationPos, DecorationTarget};

const SUPPORTS_VEGETATION: &str = "minecraft:supports_vegetation";
const SUPPORTS_DRY_VEGETATION: &str = "minecraft:supports_dry_vegetation";

/// Plants the census found standing on `minecraft:supports_vegetation`.
const VEGETATION_PLANTS: [&str; 17] = [
    "minecraft:azure_bluet",
    "minecraft:bush",
    "minecraft:cornflower",
    "minecraft:dandelion",
    "minecraft:fern",
    "minecraft:firefly_bush",
    "minecraft:lily_of_the_valley",
    "minecraft:orange_tulip",
    "minecraft:oxeye_daisy",
    "minecraft:pink_petals",
    "minecraft:pink_tulip",
    "minecraft:poppy",
    "minecraft:red_tulip",
    "minecraft:short_grass",
    "minecraft:sweet_berry_bush",
    "minecraft:white_tulip",
    "minecraft:wildflowers",
];

/// Two-row plants: the state a feature names is the lower half, and the
/// census found the same ground under each of them.
const DOUBLE_PLANTS: [&str; 6] = [
    "minecraft:large_fern",
    "minecraft:lilac",
    "minecraft:peony",
    "minecraft:rose_bush",
    "minecraft:sunflower",
    "minecraft:tall_grass",
];

/// What one state needs beneath it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ground {
    /// A full block with no ground rule.
    Any,
    /// The block below belongs to this tag.
    Tag(&'static str),
}

fn base_id(state: &str) -> &str {
    state.split_once('[').map_or(state, |(base, _)| base)
}

fn ground_for(base: &str) -> Option<Ground> {
    if VEGETATION_PLANTS.contains(&base) || DOUBLE_PLANTS.contains(&base) {
        return Some(Ground::Tag(SUPPORTS_VEGETATION));
    }
    match base {
        "minecraft:dead_bush" => Some(Ground::Tag(SUPPORTS_DRY_VEGETATION)),
        "minecraft:pumpkin" => Some(Ground::Any),
        _ => None,
    }
}

/// Whether placement of `state` is modeled at all. A state outside the census
/// keeps the feature naming it out of the run.
pub(crate) fn is_placeable(state: &str) -> bool {
    ground_for(base_id(state)).is_some()
}

/// The upper half of a two-row plant's lower-half state.
fn upper_half(state: &str) -> String {
    if state.contains("half=lower") {
        state.replace("half=lower", "half=upper")
    } else {
        format!("{}[half=upper]", base_id(state))
    }
}

/// Writes `state` at `pos` if its ground holds, returning the rows written.
/// A two-row plant also needs open air directly above and writes both halves
/// or neither, so neither the build ceiling nor a block overhead can leave a
/// lower half standing alone. Costs no draws.
pub(crate) fn place_plant(
    target: &mut dyn DecorationTarget,
    tags: &BlockTags,
    state: &str,
    pos: DecorationPos,
) -> usize {
    let base = base_id(state);
    let Some(ground) = ground_for(base) else {
        return 0;
    };
    if let Ground::Tag(tag) = ground {
        let holds = target
            .block_at(pos.x, pos.y - 1, pos.z)
            .is_some_and(|below| tags.contains(tag, base_id(below)));
        if !holds {
            return 0;
        }
    }
    if !DOUBLE_PLANTS.contains(&base) {
        target.set_block(pos.x, pos.y, pos.z, state);
        return 1;
    }
    if !target.writable(pos.x, pos.y + 1, pos.z) || !target.is_air(pos.x, pos.y + 1, pos.z) {
        return 0;
    }
    target.set_block(pos.x, pos.y, pos.z, state);
    target.set_block(pos.x, pos.y + 1, pos.z, &upper_half(state));
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_outside_the_census_are_not_placeable() {
        assert!(is_placeable("minecraft:short_grass"));
        assert!(is_placeable("minecraft:tall_grass[half=lower]"));
        assert!(is_placeable("minecraft:pumpkin"));
        assert!(!is_placeable("minecraft:brown_mushroom"));
        assert!(!is_placeable("minecraft:leaf_litter[facing=north]"));
    }

    #[test]
    fn the_upper_half_keeps_every_other_property() {
        assert_eq!(
            upper_half("minecraft:tall_grass[half=lower]"),
            "minecraft:tall_grass[half=upper]"
        );
        assert_eq!(
            upper_half("minecraft:sunflower"),
            "minecraft:sunflower[half=upper]"
        );
    }
}
