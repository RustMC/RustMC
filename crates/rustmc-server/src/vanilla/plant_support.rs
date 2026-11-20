//! What ground or water each simple vegetation state requires before
//! `minecraft:simple_block`, `minecraft:block_column`, or
//! `minecraft:would_survive` accepts a position.
//!
//! Sources (all public):
//! - Data-root 26.3 block tags under `data/minecraft/tags/block/`
//!   (`supports_vegetation`, `supports_dry_vegetation`, `supports_sugar_cane`,
//!   `supports_sugar_cane_adjacently`, `supports_cactus`,
//!   `cannot_support_seagrass`, `cannot_support_kelp`, `supports_lily_pad`,
//!   `replaceable`, `blocks_motion`)
//! - `minecraft.wiki`: `Flower` (rev 3887970), `Bush` (rev 3899792),
//!   `Firefly Bush` (rev 3871336), `Sugar Cane` (rev 3899892),
//!   `Cactus` (rev 3902732), `Cactus Flower` (rev 3868794),
//!   `Dead Bush` (rev 3870644), `Red Shrub` (rev 3903902),
//!   `Seagrass` (rev 3901734), `Kelp` (rev 3894359), `Lily Pad`
//! - Mushrooms and `leaf_litter` keep no ground rule here (`Brown Mushroom`
//!   rev 3887431 notes light `< 13` / `#overrides_mushroom_light_requirement`;
//!   sky/block light is not modeled during worldgen decoration).

use super::feature::{
    BlockTags, DecorationPos, DecorationTarget, fluid_from_block_state, is_legacy_solid,
};

const SUPPORTS_VEGETATION: &str = "minecraft:supports_vegetation";
const SUPPORTS_DRY_VEGETATION: &str = "minecraft:supports_dry_vegetation";
const SUPPORTS_SUGAR_CANE: &str = "minecraft:supports_sugar_cane";
const SUPPORTS_SUGAR_CANE_ADJACENTLY: &str = "minecraft:supports_sugar_cane_adjacently";
const SUPPORTS_CACTUS: &str = "minecraft:supports_cactus";
const CANNOT_SUPPORT_SEAGRASS: &str = "minecraft:cannot_support_seagrass";
const CANNOT_SUPPORT_KELP: &str = "minecraft:cannot_support_kelp";
const SUPPORTS_LILY_PAD: &str = "minecraft:supports_lily_pad";
const REPLACEABLE: &str = "minecraft:replaceable";

/// Fallback membership for `#minecraft:supports_vegetation` when a synthetic
/// test pack does not supply block tags.
const FALLBACK_SUPPORTS_VEGETATION: [&str; 11] = [
    "minecraft:grass_block",
    "minecraft:dirt",
    "minecraft:coarse_dirt",
    "minecraft:rooted_dirt",
    "minecraft:podzol",
    "minecraft:mycelium",
    "minecraft:mud",
    "minecraft:muddy_mangrove_roots",
    "minecraft:moss_block",
    "minecraft:pale_moss_block",
    "minecraft:farmland",
];

/// Single-row plants that grow on `#minecraft:supports_vegetation` in air.
const VEGETATION_PLANTS: [&str; 21] = [
    "minecraft:allium",
    "minecraft:azure_bluet",
    "minecraft:blue_orchid",
    "minecraft:bush",
    "minecraft:closed_eyeblossom",
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
    "minecraft:red_shrub",
    "minecraft:red_tulip",
    "minecraft:short_grass",
    "minecraft:sweet_berry_bush",
    "minecraft:white_tulip",
    "minecraft:wildflowers",
];

/// Two-row land plants (`Flower` rev 3887970, `Tall Grass` rev 3899862):
/// the lower half requires `#minecraft:supports_vegetation` and open air above.
const DOUBLE_PLANTS: [&str; 6] = [
    "minecraft:large_fern",
    "minecraft:lilac",
    "minecraft:peony",
    "minecraft:rose_bush",
    "minecraft:sunflower",
    "minecraft:tall_grass",
];

/// Single-row plants that grow on `#minecraft:supports_dry_vegetation` (`Dead Bush`
/// rev 3870644, `Dry Grass`, and pack tag `supports_dry_vegetation`).
const DRY_VEGETATION_PLANTS: [&str; 3] = [
    "minecraft:dead_bush",
    "minecraft:short_dry_grass",
    "minecraft:tall_dry_grass",
];

/// What one state needs at and beneath its placement position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ground {
    /// Any full/existing ground with no special soil tag (`pumpkin`, `melon`).
    Any,
    /// Block below must satisfy `#minecraft:supports_vegetation`.
    Vegetation,
    /// Block below must satisfy `#minecraft:supports_dry_vegetation`.
    DryVegetation,
    /// Sugar cane: `#minecraft:supports_sugar_cane` with adjacent water or
    /// `#minecraft:supports_sugar_cane_adjacently`, or stacking on `sugar_cane`.
    SugarCane,
    /// Cactus: `#minecraft:supports_cactus` or `cactus`, four horizontal neighbors
    /// free of legacy solid blocks or lava, and block above free of water/lava.
    Cactus,
    /// Cactus flower (`Cactus Flower` rev 3868794): `cactus`, `farmland`, or full solid top.
    CactusFlower,
    /// Submerged aquatic floor plant (`Seagrass` rev 3901734, `Sea Pickle`, `Kelp` rev 3894359).
    AquaticFloor {
        cannot_tag: Option<&'static str>,
        requires_water: bool,
    },
    /// Floating surface plant (`Lily Pad`): water or `#minecraft:supports_lily_pad` below.
    LilyPad,
}

fn base_id(state: &str) -> &str {
    state.split_once('[').map_or(state, |(base, _)| base)
}

fn has_tag_or_fallback(tags: &BlockTags, tag: &str, block_id: &str, fallback: &[&str]) -> bool {
    if tags.members(tag).is_some() {
        tags.contains(tag, block_id)
    } else {
        fallback.contains(&block_id)
    }
}

fn supports_vegetation_block(tags: &BlockTags, block_id: &str) -> bool {
    has_tag_or_fallback(
        tags,
        SUPPORTS_VEGETATION,
        block_id,
        &FALLBACK_SUPPORTS_VEGETATION,
    )
}

fn supports_dry_vegetation_block(tags: &BlockTags, block_id: &str) -> bool {
    if tags.members(SUPPORTS_DRY_VEGETATION).is_some() {
        return tags.contains(SUPPORTS_DRY_VEGETATION, block_id);
    }
    supports_vegetation_block(tags, block_id)
        || matches!(
            block_id,
            "minecraft:sand"
                | "minecraft:red_sand"
                | "minecraft:suspicious_sand"
                | "minecraft:terracotta"
                | "minecraft:white_terracotta"
                | "minecraft:orange_terracotta"
                | "minecraft:magenta_terracotta"
                | "minecraft:light_blue_terracotta"
                | "minecraft:yellow_terracotta"
                | "minecraft:lime_terracotta"
                | "minecraft:pink_terracotta"
                | "minecraft:gray_terracotta"
                | "minecraft:light_gray_terracotta"
                | "minecraft:cyan_terracotta"
                | "minecraft:purple_terracotta"
                | "minecraft:blue_terracotta"
                | "minecraft:brown_terracotta"
                | "minecraft:green_terracotta"
                | "minecraft:red_terracotta"
                | "minecraft:black_terracotta"
        )
}

fn is_double_land_plant(base: &str) -> bool {
    DOUBLE_PLANTS.contains(&base)
}

fn is_aquatic_state(state: &str) -> bool {
    match base_id(state) {
        "minecraft:seagrass"
        | "minecraft:tall_seagrass"
        | "minecraft:kelp"
        | "minecraft:kelp_plant" => true,
        "minecraft:sea_pickle" => state.contains("waterlogged=true"),
        _ => false,
    }
}

fn ground_for(base: &str, state: &str) -> Option<Ground> {
    if VEGETATION_PLANTS.contains(&base) || DOUBLE_PLANTS.contains(&base) {
        return Some(Ground::Vegetation);
    }
    if DRY_VEGETATION_PLANTS.contains(&base) {
        return Some(Ground::DryVegetation);
    }
    match base {
        "minecraft:pumpkin" | "minecraft:melon" => Some(Ground::Any),
        "minecraft:sugar_cane" => Some(Ground::SugarCane),
        "minecraft:cactus" => Some(Ground::Cactus),
        "minecraft:cactus_flower" => Some(Ground::CactusFlower),
        "minecraft:seagrass" | "minecraft:tall_seagrass" => Some(Ground::AquaticFloor {
            cannot_tag: Some(CANNOT_SUPPORT_SEAGRASS),
            requires_water: true,
        }),
        "minecraft:kelp" | "minecraft:kelp_plant" => Some(Ground::AquaticFloor {
            cannot_tag: Some(CANNOT_SUPPORT_KELP),
            requires_water: true,
        }),
        "minecraft:sea_pickle" => Some(Ground::AquaticFloor {
            cannot_tag: None,
            requires_water: state.contains("waterlogged=true"),
        }),
        "minecraft:lily_pad" => Some(Ground::LilyPad),
        _ => None,
    }
}

/// Whether placement of `state` is modeled by `minecraft:simple_block` and
/// `minecraft:block_column`. Saplings (tree-only), mushrooms (lighting-dependent),
/// and `leaf_litter` stay unmodeled here.
pub(crate) fn is_placeable(state: &str) -> bool {
    ground_for(base_id(state), state).is_some()
}

/// Whether `minecraft:would_survive` knows the survival rule for `state`.
/// Intentionally excludes saplings (tree-only) and lighting-dependent plants.
pub(crate) fn supports_survival_state(state: &str) -> bool {
    is_placeable(state)
}

fn is_air_state(block: &str) -> bool {
    matches!(
        base_id(block),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

fn is_still_water(block: &str) -> bool {
    block == "minecraft:water" || block == "minecraft:water[level=0]"
}

fn is_fluid_liquid(block: &str) -> bool {
    matches!(
        fluid_from_block_state(block),
        "minecraft:water" | "minecraft:flowing_water" | "minecraft:lava" | "minecraft:flowing_lava"
    )
}

/// Evaluates `minecraft:would_survive` for `state` at `pos` against `target`
/// and `tags`.
pub(crate) fn would_survive(
    target: &dyn DecorationTarget,
    tags: &BlockTags,
    state: &str,
    pos: DecorationPos,
) -> bool {
    let base = base_id(state);
    let Some(ground) = ground_for(base, state) else {
        return false;
    };
    let below = target.read_block(pos.x, pos.y - 1, pos.z);
    let ground_ok = match ground {
        Ground::Any => true,
        Ground::Vegetation => below
            .as_deref()
            .is_some_and(|b| supports_vegetation_block(tags, base_id(b))),
        Ground::DryVegetation => below
            .as_deref()
            .is_some_and(|b| supports_dry_vegetation_block(tags, base_id(b))),
        Ground::SugarCane => {
            let Some(below_str) = below.as_deref() else {
                return false;
            };
            let below_id = base_id(below_str);
            if below_id == "minecraft:sugar_cane" {
                true
            } else {
                let soil_ok = has_tag_or_fallback(
                    tags,
                    SUPPORTS_SUGAR_CANE,
                    below_id,
                    &[
                        "minecraft:grass_block",
                        "minecraft:dirt",
                        "minecraft:coarse_dirt",
                        "minecraft:rooted_dirt",
                        "minecraft:podzol",
                        "minecraft:mycelium",
                        "minecraft:mud",
                        "minecraft:muddy_mangrove_roots",
                        "minecraft:moss_block",
                        "minecraft:pale_moss_block",
                        "minecraft:sand",
                        "minecraft:red_sand",
                        "minecraft:suspicious_sand",
                    ],
                );
                if !soil_ok {
                    false
                } else {
                    [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .into_iter()
                        .any(|(dx, dz)| {
                            target
                                .read_block(pos.x + dx, pos.y - 1, pos.z + dz)
                                .is_some_and(|nbr| {
                                    let fluid = fluid_from_block_state(&nbr);
                                    fluid == "minecraft:water"
                                        || fluid == "minecraft:flowing_water"
                                        || has_tag_or_fallback(
                                            tags,
                                            SUPPORTS_SUGAR_CANE_ADJACENTLY,
                                            base_id(&nbr),
                                            &["minecraft:frosted_ice"],
                                        )
                                })
                        })
                }
            }
        }
        Ground::Cactus => {
            let Some(below_str) = below.as_deref() else {
                return false;
            };
            let below_id = base_id(below_str);
            let soil_ok = below_id == "minecraft:cactus"
                || has_tag_or_fallback(
                    tags,
                    SUPPORTS_CACTUS,
                    below_id,
                    &[
                        "minecraft:sand",
                        "minecraft:red_sand",
                        "minecraft:suspicious_sand",
                    ],
                );
            if !soil_ok {
                return false;
            }
            let above_liquid = target
                .read_block(pos.x, pos.y + 1, pos.z)
                .is_some_and(|b| is_fluid_liquid(&b));
            if above_liquid {
                return false;
            }
            let side_blocks = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .into_iter()
                .any(|(dx, dz)| {
                    target
                        .read_block(pos.x + dx, pos.y, pos.z + dz)
                        .is_some_and(|nbr| {
                            let fluid = fluid_from_block_state(&nbr);
                            fluid == "minecraft:lava"
                                || fluid == "minecraft:flowing_lava"
                                || is_legacy_solid(base_id(&nbr), tags)
                        })
                });
            !side_blocks
        }
        Ground::CactusFlower => below.as_deref().is_some_and(|b| {
            let id = base_id(b);
            id == "minecraft:cactus"
                || id == "minecraft:farmland"
                || (!is_air_state(b) && !is_fluid_liquid(b) && is_legacy_solid(id, tags))
        }),
        Ground::AquaticFloor {
            cannot_tag,
            requires_water: _,
        } => below.as_deref().is_some_and(|b| {
            let id = base_id(b);
            if id == "minecraft:kelp" || id == "minecraft:kelp_plant" {
                return matches!(base, "minecraft:kelp" | "minecraft:kelp_plant");
            }
            if let Some(tag) = cannot_tag {
                let forbidden = if tags.members(tag).is_some() {
                    tags.contains(tag, id)
                } else {
                    id == "minecraft:magma_block"
                };
                if forbidden {
                    return false;
                }
            }
            !is_air_state(b) && !is_fluid_liquid(b)
        }),
        Ground::LilyPad => below.as_deref().is_some_and(|b| {
            let fluid = fluid_from_block_state(b);
            fluid == "minecraft:water"
                || has_tag_or_fallback(
                    tags,
                    SUPPORTS_LILY_PAD,
                    base_id(b),
                    &["minecraft:ice", "minecraft:frosted_ice"],
                )
        }),
    };
    if !ground_ok {
        return false;
    }
    if is_double_land_plant(base) {
        return target.is_air(pos.x, pos.y + 1, pos.z);
    }
    if base == "minecraft:tall_seagrass" {
        return target
            .read_block(pos.x, pos.y + 1, pos.z)
            .is_some_and(|b| is_still_water(&b));
    }
    true
}

/// The upper half of a two-row plant's lower-half state.
fn upper_half(state: &str) -> String {
    if state.contains("half=lower") {
        state.replace("half=lower", "half=upper")
    } else {
        format!("{}[half=upper]", base_id(state))
    }
}

/// Writes `state` at `pos` if its destination block and ground rule hold,
/// returning the rows written. Costs no draws.
pub(crate) fn place_plant(
    target: &mut dyn DecorationTarget,
    tags: &BlockTags,
    state: &str,
    pos: DecorationPos,
) -> usize {
    if !target.writable(pos.x, pos.y, pos.z) {
        return 0;
    }
    let base = base_id(state);
    if is_aquatic_state(state) {
        let in_water = target
            .block_at(pos.x, pos.y, pos.z)
            .is_some_and(is_still_water);
        if !in_water {
            return 0;
        }
    } else if matches!(base, "minecraft:melon" | "minecraft:pumpkin") {
        let can_replace = target.is_air(pos.x, pos.y, pos.z)
            || target.block_at(pos.x, pos.y, pos.z).is_some_and(|cur| {
                !is_fluid_liquid(cur)
                    && has_tag_or_fallback(
                        tags,
                        REPLACEABLE,
                        base_id(cur),
                        &["minecraft:short_grass", "minecraft:fern"],
                    )
            });
        if !can_replace {
            return 0;
        }
    } else if !target.is_air(pos.x, pos.y, pos.z) {
        return 0;
    }

    if !would_survive(target, tags, state, pos) {
        return 0;
    }

    if is_double_land_plant(base) {
        if !target.writable(pos.x, pos.y + 1, pos.z) || !target.is_air(pos.x, pos.y + 1, pos.z) {
            return 0;
        }
        target.set_block(pos.x, pos.y, pos.z, state);
        target.set_block(pos.x, pos.y + 1, pos.z, &upper_half(state));
        return 2;
    }

    if base == "minecraft:tall_seagrass" {
        if !target.writable(pos.x, pos.y + 1, pos.z)
            || !target
                .block_at(pos.x, pos.y + 1, pos.z)
                .is_some_and(is_still_water)
        {
            return 0;
        }
        target.set_block(pos.x, pos.y, pos.z, state);
        target.set_block(pos.x, pos.y + 1, pos.z, &upper_half(state));
        return 2;
    }

    target.set_block(pos.x, pos.y, pos.z, state);
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_outside_the_census_are_not_placeable() {
        assert!(is_placeable("minecraft:short_grass"));
        assert!(is_placeable("minecraft:tall_grass[half=lower]"));
        assert!(is_placeable("minecraft:pumpkin"));
        assert!(is_placeable("minecraft:sugar_cane"));
        assert!(is_placeable("minecraft:seagrass"));
        assert!(!is_placeable("minecraft:brown_mushroom"));
        assert!(!is_placeable("minecraft:leaf_litter[facing=north]"));
        assert!(!is_placeable("minecraft:oak_sapling"));
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
