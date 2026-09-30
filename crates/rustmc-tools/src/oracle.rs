//! Vanilla ground truth extraction and match-rate reporting for the T0
//! oracle slice (docs/research/vanilla-worldgen-feasibility.md). Reads only
//! chunk data the owner's own licensed client generated; compares against
//! RustMC's current generator and reports exact-match percentages.

use rustmc_server::vanilla::generator::VanillaGenerator;
use rustmc_server::world::Generator;

use crate::nbt::Tag;
use crate::region::RegionStore;

#[derive(Debug, Clone, PartialEq)]
pub struct VanillaColumn {
    pub x: i64,
    pub z: i64,
    /// Absolute Y of the top terrain block (heightmap value − 1 + world min Y).
    pub surface_y: i32,
    pub top_block: Option<String>,
    pub biome: Option<String>,
}

/// One sampled comparison: vanilla truth vs the RustMC generator.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnVerdict {
    pub x: i64,
    pub z: i64,
    pub vanilla_surface_y: i32,
    pub vanilla_biome: Option<String>,
    pub rustmc_height: i64,
    pub rustmc_biome: String,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct MatchReport {
    pub columns: usize,
    pub height_matches: usize,
    pub biome_matches: usize,
    pub mismatches: Vec<ColumnVerdict>,
}

/// Any source of per-column RustMC answers: the current generator or the
/// data-driven vanilla density pipeline.
pub trait ColumnSource {
    fn column_height(&self, x: i64, z: i64) -> i64;
    fn column_biome(&self, x: i64, z: i64) -> String;
}

impl ColumnSource for Generator {
    fn column_height(&self, x: i64, z: i64) -> i64 {
        self.height(x, z)
    }
    fn column_biome(&self, x: i64, z: i64) -> String {
        self.biome(x, z).identifier().to_string()
    }
}

impl ColumnSource for VanillaGenerator {
    fn column_height(&self, x: i64, z: i64) -> i64 {
        let (Ok(x), Ok(z)) = (i32::try_from(x), i32::try_from(z)) else {
            return i64::MIN;
        };
        i64::from(self.surface_height(x, z))
    }
    fn column_biome(&self, _x: i64, _z: i64) -> String {
        // Biome placement is tier T2; report a never-matching placeholder.
        "<pending-T2>".to_owned()
    }
}

pub fn compare_columns(
    columns: impl IntoIterator<Item = VanillaColumn>,
    source: &dyn ColumnSource,
    mismatch_cap: usize,
) -> MatchReport {
    let mut report = MatchReport::default();
    for column in columns {
        let rustmc_height = source.column_height(column.x, column.z);
        let rustmc_biome = source.column_biome(column.x, column.z);
        report.columns += 1;
        let height_ok = i64::from(column.surface_y) == rustmc_height;
        let biome_ok = column.biome.as_deref() == Some(rustmc_biome.as_str());
        report.height_matches += usize::from(height_ok);
        report.biome_matches += usize::from(biome_ok);
        // The capped detail list tracks height gaps only; biome misses are
        // tier T2 work and would otherwise drown the diagnostic (the
        // aggregate biome count above still reports them).
        if !height_ok && report.mismatches.len() < mismatch_cap {
            report.mismatches.push(ColumnVerdict {
                x: column.x,
                z: column.z,
                vanilla_surface_y: column.surface_y,
                vanilla_biome: column.biome.clone(),
                rustmc_height,
                rustmc_biome,
            });
        }
    }
    report
}

pub fn percent(matches: usize, columns: usize) -> f64 {
    if columns == 0 {
        0.0
    } else {
        100.0 * matches as f64 / columns as f64
    }
}

/// Extract ground truth for one column of one stored chunk.
///
/// 26.3 saves (DataVersion 5023, observed 30 September 2026 in the owner's
/// world) store heightmaps relative to the world minimum Y, so the absolute
/// top block is `value - 1 + yPos * 16`. `MOTION_BLOCKING_NO_LEAVES` is the
/// closest stored analogue of a hand-cleared F3 ground reading.
pub fn column_truth(
    root: &Tag,
    chunk_x: i32,
    chunk_z: i32,
    lx: u8,
    lz: u8,
) -> Result<Option<VanillaColumn>, String> {
    let heightmaps = root.get("Heightmaps").or_else(|| root.get("heightmaps"));
    let packed = heightmaps
        .and_then(|h| {
            h.get("MOTION_BLOCKING_NO_LEAVES")
                .or_else(|| h.get("WORLD_SURFACE"))
                .or_else(|| h.get("MOTION_BLOCKING"))
        })
        .and_then(Tag::as_long_array)
        .ok_or("chunk has no usable heightmap")?;
    let min_y = root.get("yPos").and_then(Tag::as_i32).unwrap_or(-4) * 16;
    let heights = unpack_spanning(packed, 9, 256);
    let index = usize::from(lx) + usize::from(lz) * 16;
    let surface_top = match heights.get(index).copied() {
        Some(h) if h > 0 => h as i32 - 1 + min_y,
        _ => return Ok(None),
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

fn block_at(section: &Tag, lx: u8, lz: u8, local_y: i32) -> Result<Option<String>, String> {
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
fn state_name(entry: &Tag) -> Option<String> {
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

fn biome_at(section: &Tag, lx: u8, y: i32, lz: u8) -> Result<Option<String>, String> {
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

fn palette_value<'a>(
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

fn unpack_non_spanning(data: &[i64], bits: usize, count: usize) -> Vec<u32> {
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

/// Sample every `stride`-th column in the inclusive block range, skipping
/// chunks the world has not generated. Deterministic order (z then x).
pub fn sample_columns(
    store: &mut RegionStore,
    min_x: i64,
    max_x: i64,
    min_z: i64,
    max_z: i64,
    stride: i64,
) -> Result<(Vec<VanillaColumn>, usize), String> {
    if stride < 1 {
        return Err("stride must be at least 1".to_string());
    }
    let mut columns = Vec::new();
    let mut missing_chunks = 0usize;
    let stride = u8::try_from(stride).map_err(|_| "stride above 255 unsupported".to_string())?;
    let mut z = min_z;
    while z <= max_z {
        let mut x = min_x;
        while x <= max_x {
            let chunk_x =
                i32::try_from(x.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
            let chunk_z =
                i32::try_from(z.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
            let lx = u8::try_from(x.rem_euclid(16)).expect("rem_euclid(16) fits u8");
            let lz = u8::try_from(z.rem_euclid(16)).expect("rem_euclid(16) fits u8");
            match store.chunk_root(chunk_x, chunk_z) {
                Ok(None) => missing_chunks += 1,
                Ok(Some(root)) => {
                    let column = column_truth(&root, chunk_x, chunk_z, lx, lz)
                        .map_err(|e| format!("chunk ({chunk_x}, {chunk_z}) at ({x}, {z}): {e}"))?;
                    if let Some(column) = column {
                        columns.push(column);
                    }
                }
                Err(e) => return Err(format!("chunk ({chunk_x}, {chunk_z}): {e}")),
            }
            x += i64::from(stride);
        }
        z += i64::from(stride);
    }
    Ok((columns, missing_chunks))
}

pub fn worksheet_columns() -> [(i64, i64); 6] {
    [(0, 0), (256, 0), (0, 256), (-256, 0), (0, -256), (512, 512)]
}

/// One requested column read: absolute coordinates plus the extracted
/// column, or a per-column error.
pub type ColumnRead = (i64, i64, Result<Option<VanillaColumn>, String>);

/// Convenience for `inspect`: read specific absolute columns.
pub fn read_columns(
    store: &mut RegionStore,
    points: &[(i64, i64)],
) -> Result<Vec<ColumnRead>, String> {
    let mut out = Vec::new();
    for &(x, z) in points {
        let result = (|| {
            let chunk_x =
                i32::try_from(x.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
            let chunk_z =
                i32::try_from(z.div_euclid(16)).map_err(|_| "coordinate too large".to_string())?;
            let lx = u8::try_from(x.rem_euclid(16)).expect("rem_euclid(16) fits u8");
            let lz = u8::try_from(z.rem_euclid(16)).expect("rem_euclid(16) fits u8");
            match store.chunk_root(chunk_x, chunk_z)? {
                None => Ok(None),
                Some(root) => column_truth(&root, chunk_x, chunk_z, lx, lz),
            }
        })();
        out.push((x, z, result));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::compound;

    fn palette_entry(name: &str) -> Tag {
        compound(&[
            ("Name", Tag::String(name.into())),
            ("Properties", compound(&[])),
        ])
    }

    /// 16x16x16 section at Y=4 (blocks 64..=79): everything grass_block,
    /// single biome, heightmap says surface top at 75 for all columns.
    /// The chunk has no yPos, so the world minimum is -64 and heightmaps are
    /// packed min_y-relative (140 = 75 - (-64) + 1).
    fn synthetic_chunk(biome_single: bool) -> Tag {
        let mut height_data = vec![0i64; 36]; // ceil(256*9/64) = 36
        let value = 140u64;
        for i in 0..256 {
            let bit = i * 9;
            let long = bit / 64;
            let off = bit % 64;
            height_data[long] |= (value << off) as i64;
            if off + 9 > 64 {
                height_data[long + 1] |= (value >> (64 - off)) as i64;
            }
        }
        let block_states = compound(&[(
            "palette",
            Tag::List(vec![palette_entry("minecraft:grass_block")]),
        )]);
        let biomes = if biome_single {
            compound(&[(
                "palette",
                Tag::List(vec![Tag::String("minecraft:plains".into())]),
            )])
        } else {
            // Two-value palette stored at 3 bits: 4 longs is also
            // consistent with 4 bits, so the validator must pick 3.
            let mut data = vec![0i64; 4]; // 64 entries, 21 per long
            for i in 0..64 {
                let long = i / 21;
                let off = (i % 21) * 3;
                let slot = if i < 40 { 0 } else { 1 };
                data[long] |= (slot as i64) << off;
            }
            compound(&[
                (
                    "palette",
                    Tag::List(vec![
                        Tag::String("minecraft:plains".into()),
                        Tag::String("minecraft:forest".into()),
                    ]),
                ),
                ("data", Tag::LongArray(data)),
            ])
        };
        let section = compound(&[
            ("Y", Tag::Byte(4)),
            ("block_states", block_states),
            ("biomes", biomes),
        ]);
        compound(&[
            (
                "Heightmaps",
                compound(&[("WORLD_SURFACE", Tag::LongArray(height_data))]),
            ),
            ("sections", Tag::List(vec![section])),
        ])
    }

    #[test]
    fn extracts_surface_block_height_and_single_biome() {
        let root = synthetic_chunk(true);
        let column = column_truth(&root, 1, -2, 3, 9).unwrap().expect("column");
        assert_eq!(column.x, 19);
        assert_eq!(column.z, -23);
        assert_eq!(column.surface_y, 75);
        assert_eq!(column.top_block.as_deref(), Some("minecraft:grass_block"));
        assert_eq!(column.biome.as_deref(), Some("minecraft:plains"));
    }

    #[test]
    fn decodes_multi_value_biome_palette() {
        let root = synthetic_chunk(false);
        // local_y 11 within section => y index 11/4=2; lz/4 chooses halves.
        let first = column_truth(&root, 0, 0, 0, 0).unwrap().unwrap();
        assert_eq!(first.biome.as_deref(), Some("minecraft:plains"));
        let second = column_truth(&root, 0, 0, 0, 15).unwrap().unwrap();
        assert_eq!(second.biome.as_deref(), Some("minecraft:forest"));
    }

    #[test]
    fn spanning_and_non_spanning_unpackers_agree_with_manual_packing() {
        // 9-bit spanning: values 500.. pattern crossing long boundaries.
        let mut longs = vec![0i64; 36];
        let values: Vec<u32> = (0..256).map(|i| (i % 512) as u32).collect();
        for (i, v) in values.iter().enumerate() {
            let bit = i * 9;
            let long = bit / 64;
            let off = bit % 64;
            longs[long] |= (*v as i64) << off;
            if off + 9 > 64 {
                longs[long + 1] |= (*v as i64) >> (64 - off);
            }
        }
        assert_eq!(unpack_spanning(&longs, 9, 256), values);
        assert_eq!(unpack_non_spanning(&[0b1010_0101], 4, 4), vec![5, 10, 0, 0]);
    }

    #[test]
    fn state_name_handles_26_3_palette_forms() {
        let bare = compound(&[("", Tag::String("minecraft:stone".into()))]);
        assert_eq!(state_name(&bare).as_deref(), Some("minecraft:stone"));
        let with_props = compound(&[
            ("id", Tag::String("minecraft:water".into())),
            (
                "properties",
                compound(&[("level", Tag::String("3".into()))]),
            ),
        ]);
        assert_eq!(
            state_name(&with_props).as_deref(),
            Some("minecraft:water[level=3]")
        );
        assert_eq!(
            state_name(&Tag::String("minecraft:dirt".into())).as_deref(),
            Some("minecraft:dirt")
        );
    }

    #[test]
    fn ambiguous_palette_width_prefers_slot_consistency() {
        // Two-entry biome palette stored at 4 bits: at 3 bits the second
        // slot decodes to 2 (out of range), at 4 bits both slots are valid.
        let palette = vec![
            Tag::String("minecraft:plains".into()),
            Tag::String("minecraft:forest".into()),
        ];
        let data = Tag::LongArray(vec![0x11, 0, 0, 0]);
        let value = palette_value(&palette, Some(&data), 1, 64).unwrap();
        assert_eq!(value.unwrap().as_str(), Some("minecraft:forest"));
    }

    #[test]
    fn heightmaps_are_relative_to_stored_y_pos() {
        // yPos 0 with value 1 => absolute top block y = 0.
        let mut root = synthetic_chunk(true);
        let Tag::Compound(map) = &mut root else {
            unreachable!()
        };
        map.insert("yPos".to_string(), Tag::Int(0));
        let column = column_truth(&root, 0, 0, 0, 0).unwrap().unwrap();
        assert_eq!(column.surface_y, 140 - 1);
    }

    #[test]
    fn compare_report_counts_matches_and_caps_mismatches() {
        let generator = Generator::new(2026);
        let columns: Vec<VanillaColumn> = [(0i64, 0i64), (256, 0)]
            .into_iter()
            .map(|(x, z)| VanillaColumn {
                x,
                z,
                surface_y: generator.height(x, z) as i32,
                top_block: None,
                biome: Some(generator.biome(x, z).identifier().to_string()),
            })
            .collect();
        let report = compare_columns(columns.iter().cloned(), &generator, 5);
        assert_eq!(report.columns, 2);
        assert_eq!(report.height_matches, 2);
        assert_eq!(report.biome_matches, 2);
        assert!(report.mismatches.is_empty());
        assert_eq!(percent(2, 2), 100.0);
        assert_eq!(percent(0, 0), 0.0);
    }
}
