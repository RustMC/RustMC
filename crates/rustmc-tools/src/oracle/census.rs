//! Block-family census of save and generated chunks.

use super::*;

/// Block families the T4 decoration baseline counts: ore feature
/// outputs (stone and deepslate variants kept apart because they are
/// placed by different distributions), the raw-metal vein outputs,
/// the vein/blob filler stones, the base stones that ores replace,
/// and the vegetation families trees and surface patches produce.
/// Everything outside this list is ignored by the census.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CensusFamily {
    Stone,
    Deepslate,
    Coal,
    DeepslateCoal,
    Iron,
    DeepslateIron,
    Copper,
    DeepslateCopper,
    Gold,
    DeepslateGold,
    Redstone,
    DeepslateRedstone,
    Lapis,
    DeepslateLapis,
    Diamond,
    DeepslateDiamond,
    Emerald,
    DeepslateEmerald,
    RawCopper,
    RawIron,
    RawGold,
    AncientDebris,
    Granite,
    Diorite,
    Andesite,
    Tuff,
    Logs,
    Leaves,
    Saplings,
    Grasses,
    /// Selected small-flower names; the legacy report also included pitcher plants.
    Flowers,
    Wildflowers,
    Bushes,
    TallFlowers,
    Cactus,
    SugarCane,
}

impl CensusFamily {
    pub fn name(self) -> &'static str {
        match self {
            Self::Stone => "stone",
            Self::Deepslate => "deepslate",
            Self::Coal => "coal_ore",
            Self::DeepslateCoal => "deepslate_coal_ore",
            Self::Iron => "iron_ore",
            Self::DeepslateIron => "deepslate_iron_ore",
            Self::Copper => "copper_ore",
            Self::DeepslateCopper => "deepslate_copper_ore",
            Self::Gold => "gold_ore",
            Self::DeepslateGold => "deepslate_gold_ore",
            Self::Redstone => "redstone_ore",
            Self::DeepslateRedstone => "deepslate_redstone_ore",
            Self::Lapis => "lapis_ore",
            Self::DeepslateLapis => "deepslate_lapis_ore",
            Self::Diamond => "diamond_ore",
            Self::DeepslateDiamond => "deepslate_diamond_ore",
            Self::Emerald => "emerald_ore",
            Self::DeepslateEmerald => "deepslate_emerald_ore",
            Self::RawCopper => "raw_copper_block",
            Self::RawIron => "raw_iron_block",
            Self::RawGold => "raw_gold_block",
            Self::AncientDebris => "ancient_debris",
            Self::Granite => "granite",
            Self::Diorite => "diorite",
            Self::Andesite => "andesite",
            Self::Tuff => "tuff",
            Self::Logs => "logs",
            Self::Leaves => "leaves",
            Self::Saplings => "saplings",
            Self::Grasses => "grasses",
            Self::Flowers => "flowers",
            Self::Wildflowers => "wildflowers",
            Self::Bushes => "bushes",
            Self::TallFlowers => "tall_flowers",
            Self::Cactus => "cactus",
            Self::SugarCane => "sugar_cane",
        }
    }

    pub fn all() -> [Self; 36] {
        [
            Self::Stone,
            Self::Deepslate,
            Self::Coal,
            Self::DeepslateCoal,
            Self::Iron,
            Self::DeepslateIron,
            Self::Copper,
            Self::DeepslateCopper,
            Self::Gold,
            Self::DeepslateGold,
            Self::Redstone,
            Self::DeepslateRedstone,
            Self::Lapis,
            Self::DeepslateLapis,
            Self::Diamond,
            Self::DeepslateDiamond,
            Self::Emerald,
            Self::DeepslateEmerald,
            Self::RawCopper,
            Self::RawIron,
            Self::RawGold,
            Self::AncientDebris,
            Self::Granite,
            Self::Diorite,
            Self::Andesite,
            Self::Tuff,
            Self::Logs,
            Self::Leaves,
            Self::Saplings,
            Self::Grasses,
            Self::Flowers,
            Self::Wildflowers,
            Self::Bushes,
            Self::TallFlowers,
            Self::Cactus,
            Self::SugarCane,
        ]
    }
}

/// Classify one base block id (no property suffix) into a census family.
pub fn census_family(base_name: &str) -> Option<CensusFamily> {
    let name = base_name.strip_prefix("minecraft:")?;
    let family = match name {
        "stone" => CensusFamily::Stone,
        "deepslate" => CensusFamily::Deepslate,
        "coal_ore" => CensusFamily::Coal,
        "deepslate_coal_ore" => CensusFamily::DeepslateCoal,
        "iron_ore" => CensusFamily::Iron,
        "deepslate_iron_ore" => CensusFamily::DeepslateIron,
        "copper_ore" => CensusFamily::Copper,
        "deepslate_copper_ore" => CensusFamily::DeepslateCopper,
        "gold_ore" => CensusFamily::Gold,
        "deepslate_gold_ore" => CensusFamily::DeepslateGold,
        "redstone_ore" => CensusFamily::Redstone,
        "deepslate_redstone_ore" => CensusFamily::DeepslateRedstone,
        "lapis_ore" => CensusFamily::Lapis,
        "deepslate_lapis_ore" => CensusFamily::DeepslateLapis,
        "diamond_ore" => CensusFamily::Diamond,
        "deepslate_diamond_ore" => CensusFamily::DeepslateDiamond,
        "emerald_ore" => CensusFamily::Emerald,
        "deepslate_emerald_ore" => CensusFamily::DeepslateEmerald,
        "raw_copper_block" => CensusFamily::RawCopper,
        "raw_iron_block" => CensusFamily::RawIron,
        "raw_gold_block" => CensusFamily::RawGold,
        "ancient_debris" => CensusFamily::AncientDebris,
        "granite" => CensusFamily::Granite,
        "diorite" => CensusFamily::Diorite,
        "andesite" => CensusFamily::Andesite,
        "tuff" => CensusFamily::Tuff,
        "short_grass" | "tall_grass" | "grass" | "fern" | "large_fern" => CensusFamily::Grasses,
        "wildflowers" => CensusFamily::Wildflowers,
        "bush" | "firefly_bush" => CensusFamily::Bushes,
        "rose_bush" | "lilac" | "peony" | "sunflower" | "pitcher_plant" => {
            CensusFamily::TallFlowers
        }
        "cactus" => CensusFamily::Cactus,
        "sugar_cane" => CensusFamily::SugarCane,
        "dandelion" | "poppy" | "blue_orchid" | "allium" | "azure_bluet" | "red_tulip"
        | "orange_tulip" | "white_tulip" | "pink_tulip" | "oxeye_daisy" | "cornflower"
        | "lily_of_the_valley" | "wither_rose" | "torchflower" => CensusFamily::Flowers,
        other => {
            if other.ends_with("_log")
                || matches!(
                    other,
                    "crimson_stem"
                        | "warped_stem"
                        | "stripped_crimson_stem"
                        | "stripped_warped_stem"
                )
                || other.ends_with("hyphae")
            {
                CensusFamily::Logs
            } else if other.ends_with("_leaves") || other.ends_with("foliage") {
                CensusFamily::Leaves
            } else if other.ends_with("_sapling") {
                CensusFamily::Saplings
            } else {
                return None;
            }
        }
    };
    Some(family)
}

/// Decode every slot of one section's block palette into name counts.
/// The width selection mirrors `palette_value`'s candidate ordering but
/// validates all 4096 slots, which a whole-section decode can afford.
pub fn section_name_counts(section: &Tag) -> Result<Vec<(String, usize)>, String> {
    const SLOTS: usize = 16 * 16 * 16;
    let states = section
        .get("block_states")
        .or_else(|| section.get("BlockStates"))
        .ok_or("section without block_states")?;
    let palette = states
        .get("palette")
        .and_then(Tag::as_list)
        .ok_or("block_states without palette")?;
    if palette.len() == 1 {
        let name = state_name(&palette[0]).ok_or("unusable palette entry")?;
        return Ok(vec![(name, SLOTS)]);
    }
    let packed = states
        .get("data")
        .and_then(Tag::as_long_array)
        .ok_or("multi-value palette without data array")?;
    let computed = (palette.len() - 1).ilog2() as usize + 1;
    let mut widths: Vec<usize> = (1..=16)
        .filter(|&bits| {
            let per_long = 64 / bits;
            per_long > 0 && packed.len() == SLOTS.div_ceil(per_long)
        })
        .collect();
    widths.sort_by_key(|bits| (bits != &computed, *bits));
    if widths.is_empty() {
        widths.push(computed);
    }
    for bits in widths {
        let slots = unpack_non_spanning(packed, bits, SLOTS);
        let mut counts = vec![0usize; palette.len()];
        let mut valid = true;
        for slot in slots {
            let Some(entry) = usize::try_from(slot)
                .ok()
                .and_then(|idx| counts.get_mut(idx))
            else {
                valid = false;
                break;
            };
            *entry += 1;
        }
        if valid {
            let mut out = Vec::new();
            for (entry, count) in palette.iter().zip(counts) {
                if count == 0 {
                    continue;
                }
                out.push((state_name(entry).ok_or("unusable palette entry")?, count));
            }
            return Ok(out);
        }
    }
    Err(format!(
        "no consistent palette width for {} entries",
        palette.len()
    ))
}

/// Aggregate census over a chunk rectangle: family totals and per
/// 32-block Y-band counts. A sections Y window always sits inside one
/// band (sections are 16-aligned, bands 32-wide), so band attribution
/// is exact.
#[derive(Debug, Default)]
pub struct Census {
    pub chunks: usize,
    pub missing_chunks: usize,
    pub totals: std::collections::BTreeMap<CensusFamily, usize>,
    pub bands: std::collections::BTreeMap<(CensusFamily, i32), usize>,
}

impl Census {
    pub(super) fn record_section(&mut self, section: &Tag) -> Result<(), String> {
        let y_min = section
            .get("Y")
            .and_then(Tag::as_i32)
            .ok_or("section without Y")?
            .checked_mul(16)
            .ok_or("section Y overflows block coordinates")?;
        let band = y_min.div_euclid(32) * 32;
        for (name, count) in section_name_counts(section)? {
            if let Some(family) = census_family(base_block_name(&name)) {
                *self.totals.entry(family).or_default() += count;
                *self.bands.entry((family, band)).or_default() += count;
            }
        }
        Ok(())
    }

    pub(super) fn record_chunk(&mut self, root: &Tag) -> Result<bool, String> {
        let sections = chunk_sections(root)?;
        if sections.is_empty() {
            return Ok(false);
        }
        for section in sections {
            self.record_section(section)?;
        }
        self.chunks += 1;
        Ok(true)
    }

    /// Fold one generated column (as produced by
    /// `VanillaGenerator::column_ids`, indexed from the dimension's
    /// minimum build Y) into the census with the same band windows as
    /// the stored decode.
    pub fn record_column_ids(&mut self, min_y: i32, ids: &[Option<String>]) {
        for (offset, id) in ids.iter().enumerate() {
            let Some(name) = id else { continue };
            let band = (min_y + offset as i32).div_euclid(32) * 32;
            if let Some(family) = census_family(base_block_name(name)) {
                *self.totals.entry(family).or_default() += 1;
                *self.bands.entry((family, band)).or_default() += 1;
            }
        }
    }
}

/// The stored section list of a chunk root, accepting both NBT spelling
/// variants used across versions.
pub(super) fn chunk_sections(root: &Tag) -> Result<&[Tag], String> {
    root.get("sections")
        .or_else(|| root.get("Sections"))
        .and_then(Tag::as_list)
        .ok_or_else(|| "chunk has no sections list".to_owned())
}

/// Histogram of every block-state name stored in the inclusive chunk
/// rectangle, with the number of census chunks read. This is the
/// classification audit tool: it lists what the save actually contains
/// so `census_family` can be checked against real ids rather than
/// assumed names.
pub fn census_name_histogram(
    store: &mut RegionStore,
    min_cx: i32,
    max_cx: i32,
    min_cz: i32,
    max_cz: i32,
) -> Result<(usize, std::collections::BTreeMap<String, usize>), String> {
    let mut chunks = 0usize;
    let mut names = std::collections::BTreeMap::new();
    for chunk_z in min_cz..=max_cz {
        for chunk_x in min_cx..=max_cx {
            let Some(root) = store.chunk_root(chunk_x, chunk_z)? else {
                continue;
            };
            let sections =
                chunk_sections(&root).map_err(|e| format!("chunk ({chunk_x}, {chunk_z}): {e}"))?;
            if sections.is_empty() {
                continue;
            }
            for section in sections {
                for (name, count) in section_name_counts(section)? {
                    *names.entry(name).or_default() += count;
                }
            }
            chunks += 1;
        }
    }
    Ok((chunks, names))
}

/// Census every stored chunk with coordinates in the inclusive chunk
/// rectangle; ungenerated chunks are counted as missing.
pub fn census_chunks(
    store: &mut RegionStore,
    min_cx: i32,
    max_cx: i32,
    min_cz: i32,
    max_cz: i32,
) -> Result<Census, String> {
    let mut census = Census::default();
    for chunk_z in min_cz..=max_cz {
        for chunk_x in min_cx..=max_cx {
            match store.chunk_root(chunk_x, chunk_z)? {
                None => census.missing_chunks += 1,
                Some(root) => {
                    let scored = census
                        .record_chunk(&root)
                        .map_err(|e| format!("chunk ({chunk_x}, {chunk_z}): {e}"))?;
                    if !scored {
                        census.missing_chunks += 1;
                    }
                }
            }
        }
    }
    Ok(census)
}

/// Convenience: chunk rectangle covering the inclusive block range.
pub fn census_blocks(store: &mut RegionStore, min: i64, max: i64) -> Result<Census, String> {
    let to_chunk = |v: i64| -> Result<i32, String> {
        i32::try_from(v.div_euclid(16)).map_err(|_| "coordinate too large".to_string())
    };
    census_chunks(
        store,
        to_chunk(min)?,
        to_chunk(max)?,
        to_chunk(min)?,
        to_chunk(max)?,
    )
}

/// Census the generator's full material-rule descent over the inclusive
/// block square: every column from `min` to `max` in both axes, counted
/// in the same family/band shape as the save census so the two line up
/// row by row. `chunks` reports the 16-aligned chunk square covered.
pub fn census_generated(
    generator: &VanillaGenerator,
    min: i64,
    max: i64,
) -> Result<Census, String> {
    let (Ok(min), Ok(max)) = (i32::try_from(min), i32::try_from(max)) else {
        return Err("coordinate too large".to_string());
    };
    let mut census = Census::default();
    for x in min..=max {
        for z in min..=max {
            let ids = generator.column_ids(x, z);
            census.record_column_ids(generator.min_y(), &ids);
        }
    }
    let side = max - min + 1;
    census.chunks = (side.div_euclid(16) * side.div_euclid(16)) as usize;
    Ok(census)
}

/// Convenience for `inspect`: read specific absolute columns.
pub fn read_columns(
    store: &mut RegionStore,
    points: &[(i64, i64)],
) -> Result<Vec<ColumnRead>, String> {
    let mut out = Vec::new();
    for &(x, z) in points {
        let result = (|| {
            let (chunk_x, chunk_z, lx, lz) = chunk_cell_coords(x, z)?;
            match store.chunk_root(chunk_x, chunk_z)? {
                None => Ok(None),
                Some(root) => column_truth(&root, chunk_x, chunk_z, lx, lz),
            }
        })();
        out.push((x, z, result));
    }
    Ok(out)
}
