//! Data-driven biome placement (T2 slice F).
//!
//! Vanilla 26.3 places overworld biomes by sampling six `noise_router`
//! densities on a 4-block grid, quantizing each `f32` coordinate to a
//! fixed-point integer (`value × 10 000` truncated toward zero), and
//! picking the parameter entry with the smallest squared-distance
//! "fitness" across the six inclusive ranges plus a constant offset term.
//! The parameter table itself lives in the game's code, not its datapack
//! JSON; under ADR-0014 the owner provisions it as a numeric capture
//! (`rustmc/biome_placement/<preset>.psv` beside the worldgen data root,
//! never committed) produced by the disposable extraction harness
//! documented in `docs/PROVENANCE.md`.
//!
//! The pipeline is structurally free of structure blending and
//! post-processing (same stance as the density and aquifer modules), so
//! the placement answer is the raw resolver value; blender-driven biome
//! borders near structures remain a known residual.

use std::fs;
use std::path::{Path, PathBuf};

use crate::vanilla::density::Density;

/// Fixed-point scale of a quantized climate coordinate.
const QUANTIZATION_FACTOR: f32 = 10000.0;

/// One dimension's inclusive quantized interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Param {
    pub min: i64,
    pub max: i64,
}

impl Param {
    /// Distance from a target coordinate to the interval: zero inside,
    /// otherwise the gap to the nearer end.
    pub fn distance(&self, target: i64) -> i64 {
        let above = target - self.max;
        let below = self.min - target;
        if above > 0 { above } else { below.max(0) }
    }
}

/// One placement entry: six interval dimensions plus the constant offset
/// penalty (quantized like the coordinates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterPoint {
    pub temperature: Param,
    pub humidity: Param,
    pub continentalness: Param,
    pub erosion: Param,
    pub depth: Param,
    pub weirdness: Param,
    pub offset: i64,
}

impl ParameterPoint {
    /// Squared interval distances summed over all seven dimensions.
    pub fn fitness(&self, t: &[i64; 6]) -> i64 {
        let dims = [
            &self.temperature,
            &self.humidity,
            &self.continentalness,
            &self.erosion,
            &self.depth,
            &self.weirdness,
        ];
        dims.iter()
            .zip(t)
            .map(|(param, target)| square(param.distance(*target)))
            .sum::<i64>()
            + square(self.offset)
    }
}

fn square(value: i64) -> i64 {
    value * value
}

/// Java `(long)(float)` truncation toward zero after the f32 multiply.
pub fn quantize_coord(value: f32) -> i64 {
    (value * QUANTIZATION_FACTOR) as i64
}

/// A preset's placement table, in the exact order the game builds it:
/// fitness ties keep the earlier entry.
#[derive(Debug, Clone)]
pub struct BiomePlacement {
    entries: Vec<(String, ParameterPoint)>,
}

impl BiomePlacement {
    /// Looks for `rustmc/biome_placement/<preset>.psv` under the worldgen
    /// data root; `None` when the operator has not provisioned it.
    pub fn load(data_root: &Path, preset: &str) -> Option<Self> {
        let path: PathBuf = data_root
            .join("rustmc")
            .join("biome_placement")
            .join(format!("{preset}.psv"));
        let text = fs::read_to_string(path).ok()?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut entries = Vec::new();
        for line in text.lines() {
            let mut fields = line.split('|');
            let (
                Some(_index),
                Some(id),
                Some(t),
                Some(h),
                Some(c),
                Some(e),
                Some(d),
                Some(w),
                Some(off),
            ) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            )
            else {
                return None;
            };
            let point = ParameterPoint {
                temperature: parse_param(t, "t")?,
                humidity: parse_param(h, "h")?,
                continentalness: parse_param(c, "c")?,
                erosion: parse_param(e, "e")?,
                depth: parse_param(d, "d")?,
                weirdness: parse_param(w, "w")?,
                offset: off
                    .strip_prefix("off=")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default(),
            };
            entries.push((id.to_owned(), point));
        }
        (!entries.is_empty()).then_some(Self { entries })
    }

    /// Nearest entry for a quantized target; ties resolve to the earlier
    /// table entry, matching the reference (brute-force) implementation
    /// the game itself uses for testing.
    pub fn find(&self, target: &[i64; 6]) -> Option<&str> {
        let mut best: Option<(i64, &str)> = None;
        for (id, point) in &self.entries {
            let fitness = point.fitness(target);
            if best.is_none_or(|(previous, _)| fitness < previous) {
                best = Some((fitness, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// The biomes this table can select, in the order the table declares
    /// them and with repeats dropped. A biome with several parameter ranges
    /// occupies several rows, so the reference runtime's set of possible
    /// biomes is this same first-appearance dedupe of the table, and the
    /// placement stage numbers its features over exactly that list.
    pub fn possible_biomes(&self) -> Vec<&str> {
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        for (id, _) in &self.entries {
            if seen.insert(id.as_str()) {
                names.push(id.as_str());
            }
        }
        names
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn parse_param(field: &str, name: &str) -> Option<Param> {
    let value = field.strip_prefix(&format!("{name}="))?;
    if let Some(range) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        // Both bounds are integers and either may be negative, so the
        // separator is the first dash whose two sides both parse.
        for split in 1..range.len() {
            if range.as_bytes()[split] != b'-' {
                continue;
            }
            if let (Ok(min), Ok(max)) = (
                range[..split].parse::<i64>(),
                range[split + 1..].parse::<i64>(),
            ) {
                return Some(Param { min, max });
            }
        }
        None
    } else {
        let point = value.parse().ok()?;
        Some(Param {
            min: point,
            max: point,
        })
    }
}

/// The six climate coordinate samplers of a dimension, wired from the
/// compiled `noise_router` exactly as the chunk generator wires them:
/// temperature and vegetation feed the humidity axis, ridges feed the
/// weirdness axis.
pub struct ClimateSampler {
    pub temperature: Density,
    pub vegetation: Density,
    pub continents: Density,
    pub erosion: Density,
    pub depth: Density,
    pub ridges: Density,
}

impl ClimateSampler {
    /// Samples the target at block coordinates (the chunk resolver calls
    /// this at quart-cell bottoms, `quart * 4`).
    pub fn target_at_block(&self, x: i32, y: i32, z: i32) -> [i64; 6] {
        [
            quantize_coord(self.temperature.sample(x, y, z)),
            quantize_coord(self.vegetation.sample(x, y, z)),
            quantize_coord(self.continents.sample(x, y, z)),
            quantize_coord(self.erosion.sample(x, y, z)),
            quantize_coord(self.depth.sample(x, y, z)),
            quantize_coord(self.ridges.sample(x, y, z)),
        ]
    }

    /// Biome identifier for a block position given its placement table.
    pub fn biome(&self, placement: &BiomePlacement, x: i32, y: i32, z: i32) -> Option<String> {
        let quart = (x >> 2, y >> 2, z >> 2);
        let target = self.target_at_block(quart.0 << 2, quart.1 << 2, quart.2 << 2);
        Some(placement.find(&target)?.to_owned())
    }
}

/// Small table used by unit tests; shapes mirror the captured file but
/// the numbers are self-authored.
#[cfg(test)]
fn test_table() -> BiomePlacement {
    let text = concat!(
        "0|minecraft:plains|t=[-2000-2000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
        "1|minecraft:forest|t=[2000-10000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
    );
    BiomePlacement::parse(text).expect("test table parses")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantization_truncates_toward_zero() {
        assert_eq!(quantize_coord(0.7), 7000);
        assert_eq!(quantize_coord(-0.375), -3750);
        // 1.1F * 10000 rounds in f32 just under 11000 and truncates.
        assert_eq!(quantize_coord(1.1), 11000);
    }

    #[test]
    fn interval_distance_is_zero_inside() {
        let param = Param { min: -4, max: 7 };
        assert_eq!(param.distance(-10), 6);
        assert_eq!(param.distance(0), 0);
        assert_eq!(param.distance(10), 3);
    }

    /// The dimension's possible-biome set is the table read in order with
    /// repeats dropped: a biome with two parameter ranges occupies two rows
    /// but one slot, and the slot is its first row's.
    #[test]
    fn possible_biomes_follow_declaration_order_and_drop_repeats() {
        let text = concat!(
            "0|minecraft:plains|t=[-2000-2000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
            "1|minecraft:forest|t=[2000-10000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
            "2|minecraft:plains|t=[-10000--2000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
        );
        let table = BiomePlacement::parse(text).expect("test table parses");
        assert_eq!(table.len(), 3);
        assert_eq!(
            table.possible_biomes(),
            vec!["minecraft:plains", "minecraft:forest"],
        );
    }

    #[test]
    fn parsing_handles_ranges_points_and_negative_bounds() {
        let table = test_table();
        assert_eq!(table.len(), 2);
        assert_eq!(table.entries[0].1.humidity, Param::default_full());
        assert_eq!(table.entries[1].1.temperature.min, 2000);
    }

    #[test]
    fn nearest_entry_wins_and_ties_keep_the_earlier() {
        let table = test_table();
        assert_eq!(table.find(&[5000, 0, 0, 0, 0, 0]), Some("minecraft:forest"));
        assert_eq!(
            table.find(&[-1000, 0, 0, 0, 0, 0]),
            Some("minecraft:plains")
        );
        // Exactly tied fitness between the two entries keeps entry 0.
        assert_eq!(table.find(&[0, 0, 0, 0, 0, 0]), Some("minecraft:plains"));
    }

    impl Param {
        fn default_full() -> Self {
            Self {
                min: -10000,
                max: 10000,
            }
        }
    }
}
