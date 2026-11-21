//! Reading ground truth from save chunks: heightmaps, column profiles,
//! biome cells, palettes and the sampling grid.

use super::*;

/// Absolute Y of the top terrain block from the stored heightmaps, or
/// `Ok(None)` when the column is not filled yet.
///
/// 26.3 saves (DataVersion 5023, observed 30 September 2026 in the owner's
/// world) store heightmaps relative to the world minimum Y, so the absolute
/// top block is `value - 1 + yPos * 16`. `MOTION_BLOCKING_NO_LEAVES` is the
/// closest stored analogue of a hand-cleared F3 ground reading.
///
/// A stored chunk with no heightmaps at all is a chunk the client wrote
/// before the height pass, not a decode failure: it answers `Ok(None)` so
/// samplers can report it as ungenerated instead of losing the run.
pub(super) fn surface_top(root: &Tag, lx: u8, lz: u8) -> Result<Option<i32>, String> {
    let heightmaps = root.get("Heightmaps").or_else(|| root.get("heightmaps"));
    let Some(packed) = heightmaps
        .and_then(|h| {
            h.get("MOTION_BLOCKING_NO_LEAVES")
                .or_else(|| h.get("WORLD_SURFACE"))
                .or_else(|| h.get("MOTION_BLOCKING"))
        })
        .and_then(Tag::as_long_array)
    else {
        return Ok(None);
    };
    let min_y = root.get("yPos").and_then(Tag::as_i32).unwrap_or(-4) * 16;
    let heights = heightmap_heights(packed, 256);
    let index = usize::from(lx) + usize::from(lz) * 16;
    Ok(match heights.get(index).copied() {
        Some(h) if h > 0 => Some(h as i32 - 1 + min_y),
        _ => None,
    })
}

/// Heightmap bit width: enough for `max_y - min_y + 1` up to 512 levels.
pub(super) const HEIGHTMAP_BITS: usize = 9;

/// Decode a stored heightmap's 16x16 column values.
///
/// Heightmaps are 9-bit values, and 9 does not divide 64, so the two
/// documented bit-storage layouts need different long counts: spanning
/// (`ceil(256*9/64) = 36` longs) and non-spanning, where each long holds 7
/// values and wastes the remaining 6 bits (`ceil(256/7) = 37` longs).
/// Observed in the owner's 26.3 world (30 September 2026): every stored
/// heightmap is the 37-long non-spanning form, and decoding it as spanning
/// reads correct values only for the first seven columns. The long count
/// selects the layout because the two counts never coincide.
pub(super) fn heightmap_heights(data: &[i64], count: usize) -> Vec<u32> {
    let non_spanning_len = count.div_ceil(64 / HEIGHTMAP_BITS);
    if data.len() == non_spanning_len {
        unpack_non_spanning(data, HEIGHTMAP_BITS, count)
    } else {
        unpack_spanning(data, HEIGHTMAP_BITS, count)
    }
}

/// Extract ground truth for one column of one stored chunk.
pub fn column_truth(
    root: &Tag,
    chunk_x: i32,
    chunk_z: i32,
    lx: u8,
    lz: u8,
) -> Result<Option<VanillaColumn>, String> {
    let surface_top = match surface_top(root, lx, lz)? {
        Some(y) => y,
        None => return Ok(None),
    };
    let sections = root
        .get("sections")
        .or_else(|| root.get("Sections"))
        .and_then(Tag::as_list)
        .ok_or("chunk has no sections list")?;
    let mut top_block = None;
    let mut biome = None;
    for section in sections {
        let Some(y_min) = section.get("Y").and_then(Tag::as_i32).map(|y| y * 16) else {
            continue;
        };
        if surface_top < y_min || surface_top >= y_min + 16 {
            continue;
        }
        top_block = block_at(section, lx, lz, surface_top - y_min)?;
        biome = biome_at(section, lx, surface_top, lz)?;
        break;
    }
    Ok(Some(VanillaColumn {
        x: i64::from(chunk_x) * 16 + i64::from(lx),
        z: i64::from(chunk_z) * 16 + i64::from(lz),
        surface_y: surface_top,
        top_block,
        biome,
    }))
}

/// One column's stored substance profile: the air/fluid/solid category
/// of every absolute Y from the lowest stored section up to the heightmap
/// surface. Chunks store sections only where content exists, so a Y not
/// covered by any section is empty: it is recorded as air, matching how
/// the filler would have written it.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnProfile {
    pub x: i64,
    pub z: i64,
    pub surface_y: i32,
    pub min_y: i32,
    pub categories: Vec<Category>,
    /// Saved block-state names at each absolute Y. Missing sections are air.
    pub block_ids: Vec<String>,
}

pub fn column_profile(
    root: &Tag,
    chunk_x: i32,
    chunk_z: i32,
    lx: u8,
    lz: u8,
) -> Result<Option<ColumnProfile>, String> {
    let surface_y = match surface_top(root, lx, lz)? {
        Some(y) => y,
        None => return Ok(None),
    };
    let sections = root
        .get("sections")
        .or_else(|| root.get("Sections"))
        .and_then(Tag::as_list)
        .ok_or("chunk has no sections list")?;
    let stored: Vec<(i32, &Tag)> = sections
        .iter()
        .filter_map(|s| s.get("Y").and_then(Tag::as_i32).map(|y| (y * 16, s)))
        .filter(|(y_min, _)| *y_min <= surface_y)
        .collect();
    let min_y = stored
        .iter()
        .map(|(y_min, _)| *y_min)
        .min()
        .unwrap_or(surface_y);
    let mut categories = vec![Category::Air; (surface_y - min_y + 1) as usize];
    let mut block_ids = vec!["minecraft:air".to_owned(); categories.len()];
    for (y_min, section) in &stored {
        for local_y in 0..16 {
            let y = y_min + local_y;
            if y > surface_y {
                break;
            }
            if let Some(name) = block_at(section, lx, lz, local_y)? {
                categories[(y - min_y) as usize] = block_category(&name);
                block_ids[(y - min_y) as usize] = name;
            }
        }
    }
    Ok(Some(ColumnProfile {
        x: i64::from(chunk_x) * 16 + i64::from(lx),
        z: i64::from(chunk_z) * 16 + i64::from(lz),
        surface_y,
        min_y,
        categories,
        block_ids,
    }))
}

pub(super) fn block_at(
    section: &Tag,
    lx: u8,
    lz: u8,
    local_y: i32,
) -> Result<Option<String>, String> {
    let states = section
        .get("block_states")
        .or_else(|| section.get("BlockStates"))
        .ok_or("section without block_states")?;
    let palette = states
        .get("palette")
        .and_then(Tag::as_list)
        .ok_or("block_states without palette")?;
    let index = usize::from(lx) + usize::from(lz) * 16 + (local_y as usize) * 256;
    let value = palette_value(palette, states.get("data"), index, 16 * 16 * 16)?;
    Ok(value.and_then(state_name))
}

/// Blockstate palette entry names: 26.3 saves use `{"": "name"}` for
/// property-less states and `{"id": "name", "properties": {…}}` otherwise;
/// older/legacy entries use `{"Name": …, "Properties": …}`.
pub(super) fn state_name(entry: &Tag) -> Option<String> {
    match entry {
        Tag::String(s) => Some(s.clone()),
        Tag::Compound(_) => {
            let name = entry
                .get("Name")
                .or_else(|| entry.get(""))
                .or_else(|| entry.get("id"))
                .and_then(Tag::as_str)?;
            let props = entry
                .get("Properties")
                .or_else(|| entry.get("properties"))
                .and_then(|p| match p {
                    Tag::Compound(map) => Some(map),
                    _ => None,
                });
            let Some(props) = props.filter(|p| !p.is_empty()) else {
                return Some(name.to_string());
            };
            let inner: Vec<String> = props
                .iter()
                .map(|(k, v)| {
                    let value = match v {
                        Tag::String(s) => s.clone(),
                        other => format!("{other:?}"),
                    };
                    format!("{k}={value}")
                })
                .collect();
            Some(format!("{name}[{}]", inner.join(",")))
        }
        _ => None,
    }
}

pub(super) fn biome_at(section: &Tag, lx: u8, y: i32, lz: u8) -> Result<Option<String>, String> {
    let biomes = section
        .get("biomes")
        .or_else(|| section.get("Biomes"))
        .ok_or("section without biomes")?;
    let palette = biomes
        .get("palette")
        .and_then(Tag::as_list)
        .ok_or("biomes without palette")?;
    let local_y = y - section
        .get("Y")
        .and_then(Tag::as_i32)
        .map(|v| v * 16)
        .unwrap_or(i32::MIN);
    let index = usize::from(lx) / 4 + (usize::from(lz) / 4) * 4 + (local_y as usize / 4) * 16;
    let value = palette_value(palette, biomes.get("data"), index, 4 * 4 * 4)?;
    Ok(value.and_then(|tag| tag.as_str().map(str::to_string)))
}

/// One stored quart-grid biome cell at its bottom block coordinate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiomeCell {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub name: String,
}

/// Decode the saved 4×4×4 biome cells of a chunk, preserving their Y levels.
/// Missing sections are not scored: the save provides no biome answer there.
pub fn biome_cells(root: &Tag, chunk_x: i32, chunk_z: i32) -> Result<Vec<BiomeCell>, String> {
    let origin_x = chunk_x
        .checked_mul(16)
        .ok_or("chunk X overflows block coordinates")?;
    let origin_z = chunk_z
        .checked_mul(16)
        .ok_or("chunk Z overflows block coordinates")?;
    let mut cells = Vec::new();
    for section in chunk_sections(root)? {
        let section_y = section
            .get("Y")
            .and_then(Tag::as_i32)
            .ok_or("section without Y")?;
        let base_y = section_y
            .checked_mul(16)
            .ok_or("section Y overflows block coordinates")?;
        for by in 0..4 {
            for bz in 0..4 {
                for bx in 0..4 {
                    let y = base_y
                        .checked_add(by * 4)
                        .ok_or("section Y overflows block coordinates")?;
                    let Some(name) = biome_at(section, (bx * 4) as u8, y, (bz * 4) as u8)? else {
                        return Err("saved biome cell has no palette entry".to_owned());
                    };
                    cells.push(BiomeCell {
                        x: origin_x + bx * 4,
                        y,
                        z: origin_z + bz * 4,
                        name,
                    });
                }
            }
        }
    }
    Ok(cells)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BiomeVolumeReport {
    pub chunks: usize,
    pub missing_chunks: usize,
    pub cells: usize,
    pub matches: usize,
    pub unresolved: usize,
    /// 32-block Y band -> (scored cells, exact matches).
    pub bands: std::collections::BTreeMap<i32, (usize, usize)>,
    pub residuals: std::collections::BTreeMap<(String, String), usize>,
}

/// Compare every stored quart cell in an inclusive square of saved chunks
/// against the generated biome at the same bottom block coordinate.
pub fn compare_biome_volume(
    store: &mut RegionStore,
    generator: &VanillaGenerator,
    min_chunk: i32,
    max_chunk: i32,
) -> Result<BiomeVolumeReport, String> {
    if min_chunk > max_chunk || i64::from(max_chunk) - i64::from(min_chunk) > 31 {
        return Err("chunk range must be ordered and at most 32 wide".to_owned());
    }
    let mut report = BiomeVolumeReport::default();
    for chunk_z in min_chunk..=max_chunk {
        for chunk_x in min_chunk..=max_chunk {
            let Some(root) = store.chunk_root(chunk_x, chunk_z)? else {
                report.missing_chunks += 1;
                continue;
            };
            let cells = biome_cells(&root, chunk_x, chunk_z)?;
            if cells.is_empty() {
                report.missing_chunks += 1;
                continue;
            }
            report.chunks += 1;
            for cell in cells {
                report.cells += 1;
                let band = cell.y.div_euclid(32) * 32;
                let entry = report.bands.entry(band).or_default();
                entry.0 += 1;
                let Some(ours) = generator.biome(cell.x, cell.z, cell.y) else {
                    report.unresolved += 1;
                    continue;
                };
                let matched = ours == cell.name;
                report.matches += usize::from(matched);
                entry.1 += usize::from(matched);
                if !matched {
                    *report.residuals.entry((cell.name, ours)).or_default() += 1;
                }
            }
        }
    }
    Ok(report)
}

pub(super) fn palette_value<'a>(
    palette: &'a [Tag],
    data: Option<&'a Tag>,
    index: usize,
    count: usize,
) -> Result<Option<&'a Tag>, String> {
    if palette.len() == 1 {
        return Ok(palette.first());
    }
    let computed = (palette.len() - 1).ilog2() as usize + 1;
    let packed = data
        .and_then(Tag::as_long_array)
        .ok_or("multi-value palette without data array")?;
    // 26.3 disk sections use the exact bit width the palette needs (biome
    // palettes with two entries are stored at one bit, observed in the
    // owner's world), and stored data can outlive palette changes. Derive
    // candidate widths from the data length; where several widths share a
    // long count (64 entries: 3 or 4 bits), accept the first whose decoded
    // slots all fit the palette, preferring the palette-derived width.
    let mut widths: Vec<usize> = (1..=16)
        .filter(|&bits| {
            let per_long = 64 / bits;
            per_long > 0 && packed.len() == count.div_ceil(per_long)
        })
        .collect();
    widths.sort_by_key(|bits| (bits != &computed, *bits));
    if widths.is_empty() {
        widths.push(computed);
    }
    for bits in widths {
        let slots = unpack_non_spanning(packed, bits, index + 1);
        if slots.iter().all(|slot| (*slot as usize) < palette.len()) {
            return Ok(palette.get(slots[index] as usize));
        }
    }
    Err(format!(
        "no consistent palette width for {} entries",
        palette.len()
    ))
}

pub fn unpack_spanning(data: &[i64], bits: usize, count: usize) -> Vec<u32> {
    let mask = (1u64 << bits) - 1;
    (0..count)
        .map(|i| {
            let bit = i * bits;
            let long = bit / 64;
            let off = bit % 64;
            let cur = data.get(long).copied().unwrap_or(0) as u64;
            let low = cur >> off;
            let value = if off + bits <= 64 {
                low
            } else {
                let next = data.get(long + 1).copied().unwrap_or(0) as u64;
                low | (next << (64 - off))
            };
            (value & mask) as u32
        })
        .collect()
}

pub(super) fn unpack_non_spanning(data: &[i64], bits: usize, count: usize) -> Vec<u32> {
    let mask = (1u64 << bits) - 1;
    let per_long = 64 / bits;
    (0..count)
        .map(|i| {
            let long = i / per_long;
            let off = (i % per_long) * bits;
            let value = data.get(long).copied().unwrap_or(0) as u64;
            ((value >> off) & mask) as u32
        })
        .collect()
}

/// Grid points a sampler could not score, split by cause so a report can
/// state its denominator completely.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SampleGaps {
    /// No chunk is stored in the region file at the grid point.
    pub missing_chunks: usize,
    /// A chunk is stored but holds no filled surface column yet: the
    /// client wrote it before the height pass, or it is a not-generated
    /// placeholder.
    pub ungenerated_columns: usize,
}

impl SampleGaps {
    pub fn total(self) -> usize {
        self.missing_chunks + self.ungenerated_columns
    }
}

/// Sample every `stride`-th column in the inclusive block range, skipping
/// chunks the world has not generated. Deterministic order (z then x).
pub fn sample_columns(
    store: &mut RegionStore,
    min_x: i64,
    max_x: i64,
    min_z: i64,
    max_z: i64,
    stride: i64,
) -> Result<(Vec<VanillaColumn>, SampleGaps), String> {
    sample_grid(store, min_x, max_x, min_z, max_z, stride, column_truth)
}

/// Like [`sample_columns`], but reading full substance profiles.
pub fn sample_profiles(
    store: &mut RegionStore,
    min_x: i64,
    max_x: i64,
    min_z: i64,
    max_z: i64,
    stride: i64,
) -> Result<(Vec<ColumnProfile>, SampleGaps), String> {
    sample_grid(store, min_x, max_x, min_z, max_z, stride, column_profile)
}

/// One column read from a stored chunk root, shared by the samplers.
pub(super) type ColumnReader<T> = fn(&Tag, i32, i32, u8, u8) -> Result<Option<T>, String>;

pub(super) fn chunk_cell_coords(x: i64, z: i64) -> Result<(i32, i32, u8, u8), String> {
    let chunk_x =
        i32::try_from(x.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
    let chunk_z =
        i32::try_from(z.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
    let lx = (x & 0x0f) as u8;
    let lz = (z & 0x0f) as u8;
    Ok((chunk_x, chunk_z, lx, lz))
}

pub(super) fn sample_grid<T>(
    store: &mut RegionStore,
    min_x: i64,
    max_x: i64,
    min_z: i64,
    max_z: i64,
    stride: i64,
    read: ColumnReader<T>,
) -> Result<(Vec<T>, SampleGaps), String> {
    if stride < 1 {
        return Err("stride must be at least 1".to_string());
    }
    let mut columns = Vec::new();
    let mut gaps = SampleGaps::default();
    let stride = u8::try_from(stride).map_err(|_| "stride above 255 unsupported".to_string())?;
    let mut z = min_z;
    while z <= max_z {
        let mut x = min_x;
        while x <= max_x {
            let (chunk_x, chunk_z, lx, lz) = chunk_cell_coords(x, z)?;
            match store.chunk_root(chunk_x, chunk_z) {
                Ok(None) => gaps.missing_chunks += 1,
                Ok(Some(root)) => {
                    let column = (read)(&root, chunk_x, chunk_z, lx, lz)
                        .map_err(|e| format!("chunk ({chunk_x}, {chunk_z}) at ({x}, {z}): {e}"))?;
                    match column {
                        Some(column) => columns.push(column),
                        None => gaps.ungenerated_columns += 1,
                    }
                }
                Err(e) => return Err(format!("chunk ({chunk_x}, {chunk_z}): {e}")),
            }
            x += i64::from(stride);
        }
        z += i64::from(stride);
    }
    Ok((columns, gaps))
}

pub fn worksheet_columns() -> [(i64, i64); 6] {
    [(0, 0), (256, 0), (0, 256), (-256, 0), (0, -256), (512, 512)]
}

/// Read one absolute column's stored substance profile, or `None` when
/// the chunk is not generated.
pub fn read_profile(
    store: &mut RegionStore,
    x: i64,
    z: i64,
) -> Result<Option<ColumnProfile>, String> {
    let (chunk_x, chunk_z, lx, lz) = chunk_cell_coords(x, z)?;
    match store.chunk_root(chunk_x, chunk_z)? {
        None => Ok(None),
        Some(root) => column_profile(&root, chunk_x, chunk_z, lx, lz),
    }
}

/// One requested column read: absolute coordinates plus the extracted
/// column, or a per-column error.
pub type ColumnRead = (i64, i64, Result<Option<VanillaColumn>, String>);
