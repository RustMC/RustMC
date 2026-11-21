//! Registry carver runtime: the caves and canyons carved through the
//! filled terrain by a per-chunk mask replay.
//!
//! The semantics implemented here were established by knowledge-only
//! consultation under ADR-0014 (as amended) and are recorded as
//! numeric/structural facts in `docs/PROVENANCE.md` (session 9). Nothing
//! from that consultation is present in this repository as code; the
//! module is written independently from the recorded facts. Full geometry
//! parity vectors remain to be captured; current tests cover components and
//! aggregate save comparisons cover the assembled output.
//!
//! Shape of the runtime: one legacy-LCG stream is reseeded per source
//! chunk and carver index over the 17×17 chunk window around the target
//! (`set_large_feature_seed(world_seed + index, src_x, src_z)`), a
//! probability gate decides starts, and each start walks caves/tunnels or
//! a canyon that stamp ellipsoids into a [`CarveMask`]. Applying the mask
//! replaces every masked position with the aquifer's answer at density
//! `0.0`, so carved terrain becomes cave air, water, or lava exactly as
//! the fluid picker dictates. Like the rest of this pipeline the replay
//! is structurally free of block states: uncarvable-tagged blocks and the
//! post-carve grass-top recolor do not occur in density-only terrain
//! replay, so the tag check and recolor are out of scope (the recolor is
//! category-neutral anyway).
//!
//! The per-tunnel stream is the legacy 48-bit LCG, not xoroshiro: Java's
//! `RandomSource.createThreadLocalInstance` returns a
//! `SingleThreadedRandomSource` built on the same bit generator
//! (PROVENANCE session 9).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use serde_json::Value;

use crate::vanilla::random::LegacyRandom;
use crate::vanilla::worldgen::{WorldgenError, read_dir_optional, read_json, stem, walk_json};

/// Java `Mth.sin(float)`/`cos(float)` are the double trig widened back
/// down; keeping the cast explicit preserves that rounding step.
fn sin_f32(value: f32) -> f32 {
    (value as f64).sin() as f32
}

fn cos_f32(value: f32) -> f32 {
    (value as f64).cos() as f32
}

/// Java `Mth.floor(double)`.
fn floor_f64(value: f64) -> i32 {
    value.floor() as i32
}

/// The float constants the walkers multiply headings by, each exactly
/// the `(float)` cast of its double counterpart.
const PI_F: f32 = std::f64::consts::PI as f32;
const TWO_PI_F: f32 = (2.0 * std::f64::consts::PI) as f32;
const HALF_PI_F: f32 = (std::f64::consts::FRAC_PI_2) as f32;

/// Carver geometry needs the world's vertical extent and sea level to
/// resolve height anchors.
#[derive(Debug, Clone, Copy)]
pub struct CarverContext {
    pub min_y: i32,
    pub gen_depth: i32,
    pub sea_level: i32,
}

/// The per-target-chunk carving mask: a bitset over the 16×16 columns
/// and the inclusive Y window the orchestration allocates it with.
#[derive(Debug, Clone)]
pub struct CarveMask {
    min_y: i32,
    height: i32,
    bits: Vec<u64>,
}

impl CarveMask {
    pub fn new(min_y: i32, max_y: i32) -> Self {
        let height = max_y - min_y + 1;
        let words = (256usize * height as usize).div_ceil(64);
        Self {
            min_y,
            height,
            bits: vec![0; words],
        }
    }

    /// Heap bytes of the bitset for one chunk's Y window: the price of
    /// caching a target chunk's mask, used to size the mask cache.
    pub const fn heap_bytes(min_y: i32, max_y: i32) -> usize {
        let height = (max_y - min_y + 1) as usize;
        (256 * height).div_ceil(64) * size_of::<u64>()
    }

    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    pub fn max_y(&self) -> i32 {
        self.min_y + self.height - 1
    }

    /// Java `CarvingMask.getIndex`: `y - minY + (z + (x << 4)) * height`.
    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        (y - self.min_y + (z + (x << 4)) * self.height) as usize
    }

    /// Stamp one relative-block position; Y outside the window is
    /// dropped (the ellipsoid clip already prevents this in practice).
    pub fn carve(&mut self, x: i32, y: i32, z: i32) {
        if y < self.min_y || y > self.max_y() || !self.in_columns(x, z) {
            debug_assert!(false, "carve outside mask window: {x} {y} {z}");
            return;
        }
        let index = self.index(x, y, z);
        self.bits[index / 64] |= 1u64 << (index % 64);
    }

    pub fn contains(&self, x: i32, y: i32, z: i32) -> bool {
        if y < self.min_y || y > self.max_y() || !self.in_columns(x, z) {
            return false;
        }
        let index = self.index(x, y, z);
        self.bits[index / 64] >> (index % 64) & 1 != 0
    }

    /// Relative column coordinates stay inside the chunk's 16×16.
    fn in_columns(&self, x: i32, z: i32) -> bool {
        (0..16).contains(&x) && (0..16).contains(&z)
    }

    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|word| *word == 0)
    }

    pub fn carved_count(&self) -> usize {
        self.bits
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }
}

/// One `VerticalAnchor`: `absolute`, `above_bottom`, `below_top`, or
/// `relative_to_sea_level` with an integer offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
    RelativeToSeaLevel(i32),
}

impl Anchor {
    fn resolve(&self, ctx: &CarverContext) -> i32 {
        match self {
            Self::Absolute(value) => *value,
            Self::AboveBottom(offset) => ctx.min_y + offset,
            Self::BelowTop(offset) => ctx.min_y + ctx.gen_depth - 1 - offset,
            Self::RelativeToSeaLevel(offset) => ctx.sea_level + offset,
        }
    }
}

/// The `minecraft:uniform` height provider over two anchors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeightSpec {
    pub min: Anchor,
    pub max: Anchor,
}

impl HeightSpec {
    /// Uniform height: an empty range resolves to its (clamped) minimum
    /// without a draw; else the inclusive integer uniform draw.
    pub fn sample(&self, ctx: &CarverContext, random: &mut LegacyRandom) -> i32 {
        let min = self.min.resolve(ctx);
        let max = self.max.resolve(ctx);
        if min > max {
            return min;
        }
        random.next_int_bounded(max - min + 1) + min
    }
}

/// Integer value providers the carvers use: `constant`, `uniform`, and
/// `very_biased_to_bottom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntSpec {
    Constant(i32),
    Uniform { min: i32, max: i32 },
    VeryBiasedToBottom { min: i32, max: i32 },
}

impl IntSpec {
    pub fn sample(&self, random: &mut LegacyRandom) -> i32 {
        match self {
            Self::Constant(value) => *value,
            Self::Uniform { min, max } => random.next_int_bounded(max - min + 1) + min,
            Self::VeryBiasedToBottom { min, max } => {
                let span = random.next_int_bounded(max - min + 1);
                let once = random.next_int_bounded(span + 1);
                let twice = random.next_int_bounded(once + 1);
                min + twice
            }
        }
    }
}

/// Float value providers the carvers use: `constant`, `uniform`, and
/// `trapezoid`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FloatSpec {
    Constant(f32),
    Uniform { min: f32, max: f32 },
    Trapezoid { min: f32, max: f32, plateau: f32 },
}

impl FloatSpec {
    pub fn sample(&self, random: &mut LegacyRandom) -> f32 {
        match self {
            Self::Constant(value) => *value,
            Self::Uniform { min, max } => random.next_float() * (max - min) + min,
            Self::Trapezoid { min, max, plateau } => {
                let range = max - min;
                let plateau_start = (range - plateau) / 2.0;
                let plateau_end = range - plateau_start;
                *min + random.next_float() * plateau_end + random.next_float() * plateau_start
            }
        }
    }
}

/// The skip predicate applied to each candidate block of an ellipsoid,
/// by carver family.
enum Skip<'a> {
    /// Cave: never carve below the floor fraction, and only within the
    /// unit sphere.
    Cave { floor: f64 },
    /// Canyon: a per-height width factor squashes the horizontal radius
    /// and the vertical radius is divided by six.
    Canyon { width: &'a [f32], min_y: i32 },
}

impl Skip<'_> {
    fn test(&self, xd: f64, yd: f64, zd: f64, world_y: i32) -> bool {
        match self {
            Self::Cave { floor } => *floor >= yd || xd * xd + yd * yd + zd * zd >= 1.0,
            Self::Canyon { width, min_y } => {
                let factor = f64::from(width[(world_y - min_y - 1) as usize]);
                (xd * xd + zd * zd) * factor + yd * yd / 6.0 >= 1.0
            }
        }
    }
}

/// One ellipsoid stamp: walk center and the two radii.
struct Ellipsoid {
    x: f64,
    y: f64,
    z: f64,
    horizontal_radius: f64,
    vertical_radius: f64,
}

/// Stamp one ellipsoid of the walk into the target chunk's mask.
fn carve_ellipsoid(mask: &mut CarveMask, target: (i32, i32), shape: &Ellipsoid, skip: &Skip<'_>) {
    let x = shape.x;
    let y = shape.y;
    let z = shape.z;
    let horizontal_radius = shape.horizontal_radius;
    let vertical_radius = shape.vertical_radius;
    let middle_x = f64::from((target.0 << 4) + 8);
    let middle_z = f64::from((target.1 << 4) + 8);
    let max_delta = 16.0 + horizontal_radius * 2.0;
    if (x - middle_x).abs() > max_delta || (z - middle_z).abs() > max_delta {
        return;
    }
    let min_x = target.0 << 4;
    let min_z = target.1 << 4;
    let x_lo = (floor_f64(x - horizontal_radius) - min_x - 1).max(0);
    let x_hi = (floor_f64(x + horizontal_radius) - min_x).min(15);
    let z_lo = (floor_f64(z - horizontal_radius) - min_z - 1).max(0);
    let z_hi = (floor_f64(z + horizontal_radius) - min_z).min(15);
    let y_lo = (floor_f64(y - vertical_radius) - 1).max(mask.min_y());
    let y_hi = (floor_f64(y + vertical_radius) + 1).min(mask.max_y());
    if x_lo > x_hi || z_lo > z_hi {
        return;
    }
    for xi in x_lo..=x_hi {
        let xd = (f64::from(min_x + xi) + 0.5 - x) / horizontal_radius;
        for zi in z_lo..=z_hi {
            let zd = (f64::from(min_z + zi) + 0.5 - z) / horizontal_radius;
            if xd * xd + zd * zd >= 1.0 {
                continue;
            }
            let mut wy = y_hi;
            while wy > y_lo {
                let yd = (f64::from(wy) - 0.5 - y) / vertical_radius;
                if !skip.test(xd, yd, zd, wy) {
                    mask.carve(xi, wy, zi);
                }
                wy -= 1;
            }
        }
    }
}

/// How far a walker step may still reach the target chunk.
fn can_reach(
    target: (i32, i32),
    x: f64,
    z: f64,
    current_step: i32,
    total_steps: i32,
    thickness: f32,
) -> bool {
    let xd = x - f64::from((target.0 << 4) + 8);
    let zd = z - f64::from((target.1 << 4) + 8);
    let remaining = f64::from(total_steps - current_step);
    let reach = f64::from(thickness + 2.0 + 16.0);
    xd * xd + zd * zd - remaining * remaining <= reach * reach
}

/// The shared reach distance in blocks for both walker families:
/// `(range * 2 - 1) * 16` with the default carver range of 4 sections.
const MAX_DISTANCE: i32 = 112;

/// Cave-family parameters, from a `minecraft:cave` carver document.
#[derive(Debug, Clone)]
pub struct CaveCarver {
    pub probability: f32,
    pub y: HeightSpec,
    pub count: IntSpec,
    pub thickness: FloatSpec,
    pub weird_thickness_bias: bool,
    pub room_vertical: FloatSpec,
    pub horizontal_multiplier: FloatSpec,
    pub vertical_multiplier: FloatSpec,
    pub start_vertical: FloatSpec,
    pub floor: FloatSpec,
}

/// Canyon-family shape parameters, from a `minecraft:canyon` document.
#[derive(Debug, Clone)]
pub struct CanyonShape {
    pub distance_factor: FloatSpec,
    pub thickness: FloatSpec,
    pub width_smoothness: i32,
    pub horizontal_factor: FloatSpec,
    pub vertical_default: f32,
    pub vertical_center: f32,
    pub y_scale: FloatSpec,
}

#[derive(Debug, Clone)]
pub struct CanyonCarver {
    pub probability: f32,
    pub y: HeightSpec,
    pub vertical_rotation: FloatSpec,
    pub shape: CanyonShape,
}

#[derive(Debug, Clone)]
pub enum Carver {
    Cave(CaveCarver),
    Canyon(CanyonCarver),
}

/// Mutable per-tunnel walk state for the cave family.
struct TunnelState {
    x: f64,
    y: f64,
    z: f64,
    horizontal_rotation: f32,
    vertical_rotation: f32,
    thickness: f32,
    step: i32,
    distance: i32,
    vertical_scale: f64,
}

/// Shared carve resources threaded through one cave walk.
struct CaveRun<'a> {
    target: (i32, i32),
    mask: &'a mut CarveMask,
    skip: &'a Skip<'a>,
    horizontal_multiplier: f64,
    vertical_multiplier: f64,
}

impl Carver {
    pub fn is_start_chunk(&self, random: &mut LegacyRandom) -> bool {
        random.next_float() <= self.probability()
    }

    fn probability(&self) -> f32 {
        match self {
            Self::Cave(carver) => carver.probability,
            Self::Canyon(carver) => carver.probability,
        }
    }

    /// Walk one start; stamps into the target chunk's mask.
    pub fn carve(
        &self,
        ctx: &CarverContext,
        random: &mut LegacyRandom,
        target: (i32, i32),
        source: (i32, i32),
        mask: &mut CarveMask,
    ) {
        match self {
            Self::Cave(carver) => carver.carve(ctx, random, target, source, mask),
            Self::Canyon(carver) => carver.carve(ctx, random, target, source, mask),
        }
    }
}

impl CaveCarver {
    fn carve(
        &self,
        ctx: &CarverContext,
        random: &mut LegacyRandom,
        target: (i32, i32),
        source: (i32, i32),
        mask: &mut CarveMask,
    ) {
        let count = self.count.sample(random);
        for _ in 0..count {
            // Per-cave origin and constant multipliers, in draw order.
            let x = f64::from((source.0 << 4) + random.next_int_bounded(16));
            let y = f64::from(self.y.sample(ctx, random));
            let z = f64::from((source.1 << 4) + random.next_int_bounded(16));
            let horizontal_multiplier = f64::from(self.horizontal_multiplier.sample(random));
            let vertical_multiplier = f64::from(self.vertical_multiplier.sample(random));
            let start_vertical = f64::from(self.start_vertical.sample(random));
            let floor = f64::from(self.floor.sample(random));
            let skip = Skip::Cave { floor };
            let mut tunnels = 1;
            if random.next_int_bounded(4) == 0 {
                let room_scale = f64::from(self.room_vertical.sample(random));
                let room_thickness = 1.0 + random.next_float() * 6.0;
                let radius = 1.5 + f64::from(sin_f32(HALF_PI_F) * room_thickness);
                carve_ellipsoid(
                    mask,
                    target,
                    &Ellipsoid {
                        x: x + 1.0,
                        y,
                        z,
                        horizontal_radius: radius,
                        vertical_radius: radius * room_scale,
                    },
                    &skip,
                );
                tunnels += random.next_int_bounded(4);
            }
            let mut run = CaveRun {
                target,
                mask,
                skip: &skip,
                horizontal_multiplier,
                vertical_multiplier,
            };
            for _ in 0..tunnels {
                let horizontal_rotation = random.next_float() * TWO_PI_F;
                let vertical_rotation = (random.next_float() - 0.5) / 4.0;
                let mut thickness = self.thickness.sample(random);
                if self.weird_thickness_bias && random.next_int_bounded(10) == 0 {
                    thickness *= random.next_float() * random.next_float() * 3.0 + 1.0;
                }
                let distance = MAX_DISTANCE - random.next_int_bounded(MAX_DISTANCE / 4);
                let tunnel_seed = random.next_long();
                let mut walker = LegacyRandom::new(tunnel_seed);
                self.tunnel(
                    &mut walker,
                    &mut run,
                    TunnelState {
                        x,
                        y,
                        z,
                        horizontal_rotation,
                        vertical_rotation,
                        thickness,
                        step: 0,
                        distance,
                        vertical_scale: start_vertical,
                    },
                );
            }
        }
    }

    fn tunnel(&self, walker: &mut LegacyRandom, run: &mut CaveRun<'_>, mut state: TunnelState) {
        let split_point = walker.next_int_bounded(state.distance / 2) + state.distance / 4;
        let steep = walker.next_int_bounded(6) == 0;
        let mut x_delta_rotation = 0.0f32;
        let mut y_delta_rotation = 0.0f32;
        while state.step < state.distance {
            let step = state.step;
            let horizontal_radius = 1.5
                + f64::from(
                    sin_f32(PI_F * (step as f32) / (state.distance as f32)) * state.thickness,
                );
            let vertical_radius = horizontal_radius * state.vertical_scale;
            let rotation_cos = cos_f32(state.vertical_rotation);
            state.x += f64::from(cos_f32(state.horizontal_rotation) * rotation_cos);
            state.y += f64::from(sin_f32(state.vertical_rotation));
            state.z += f64::from(sin_f32(state.horizontal_rotation) * rotation_cos);
            state.vertical_rotation *= if steep { 0.92 } else { 0.7 };
            state.vertical_rotation += x_delta_rotation * 0.1;
            state.horizontal_rotation += y_delta_rotation * 0.1;
            x_delta_rotation *= 0.9;
            y_delta_rotation *= 0.75;
            x_delta_rotation +=
                (walker.next_float() - walker.next_float()) * walker.next_float() * 2.0;
            y_delta_rotation +=
                (walker.next_float() - walker.next_float()) * walker.next_float() * 4.0;
            if step == split_point && state.thickness > 1.0 {
                // Two recursive forks; the parent walk ends here. Each
                // child gets its own stream, in seed-then-thickness draw
                // order, at a right angle to the split heading.
                for fork in [-1.0, 1.0] {
                    let child_seed = walker.next_long();
                    let child_thickness = walker.next_float() * 0.5 + 0.5;
                    let mut child = LegacyRandom::new(child_seed);
                    self.tunnel(
                        &mut child,
                        run,
                        TunnelState {
                            x: state.x,
                            y: state.y,
                            z: state.z,
                            horizontal_rotation: state.horizontal_rotation + fork * HALF_PI_F,
                            vertical_rotation: state.vertical_rotation / 3.0,
                            thickness: child_thickness,
                            step,
                            distance: state.distance,
                            vertical_scale: 1.0,
                        },
                    );
                }
                return;
            }
            if walker.next_int_bounded(4) != 0 {
                if !can_reach(
                    run.target,
                    state.x,
                    state.z,
                    step,
                    state.distance,
                    state.thickness,
                ) {
                    return;
                }
                carve_ellipsoid(
                    run.mask,
                    run.target,
                    &Ellipsoid {
                        x: state.x,
                        y: state.y,
                        z: state.z,
                        horizontal_radius: horizontal_radius * run.horizontal_multiplier,
                        vertical_radius: vertical_radius * run.vertical_multiplier,
                    },
                    run.skip,
                );
            }
            state.step += 1;
        }
    }
}

impl CanyonCarver {
    fn carve(
        &self,
        ctx: &CarverContext,
        random: &mut LegacyRandom,
        target: (i32, i32),
        source: (i32, i32),
        mask: &mut CarveMask,
    ) {
        let mut x = f64::from((source.0 << 4) + random.next_int_bounded(16));
        let mut y = f64::from(self.y.sample(ctx, random));
        let mut z = f64::from((source.1 << 4) + random.next_int_bounded(16));
        let mut horizontal_rotation = random.next_float() * TWO_PI_F;
        let mut vertical_rotation = self.vertical_rotation.sample(random);
        let y_scale = f64::from(self.shape.y_scale.sample(random));
        let thickness = self.shape.thickness.sample(random);
        let distance = (MAX_DISTANCE as f32 * self.shape.distance_factor.sample(random)) as i32;
        let tunnel_seed = random.next_long();
        let mut walker = LegacyRandom::new(tunnel_seed);
        let width_factors = self.init_width_factors(ctx, &mut walker);
        let skip = Skip::Canyon {
            width: &width_factors,
            min_y: ctx.min_y,
        };
        let mut x_delta_rotation = 0.0f32;
        let mut y_delta_rotation = 0.0f32;
        for step in 0..distance {
            let mut horizontal_radius =
                1.5 + f64::from(sin_f32((step as f32) * PI_F / (distance as f32)) * thickness);
            let mut vertical_radius = horizontal_radius * y_scale;
            horizontal_radius *= f64::from(self.shape.horizontal_factor.sample(&mut walker));
            vertical_radius = self.update_vertical_radius(
                &mut walker,
                vertical_radius,
                distance as f32,
                step as f32,
            );
            let rotation_cos = cos_f32(vertical_rotation);
            x += f64::from(cos_f32(horizontal_rotation) * rotation_cos);
            y += f64::from(sin_f32(vertical_rotation));
            z += f64::from(sin_f32(horizontal_rotation) * rotation_cos);
            vertical_rotation *= 0.7;
            vertical_rotation += x_delta_rotation * 0.05;
            horizontal_rotation += y_delta_rotation * 0.05;
            x_delta_rotation *= 0.8;
            y_delta_rotation *= 0.5;
            x_delta_rotation +=
                (walker.next_float() - walker.next_float()) * walker.next_float() * 2.0;
            y_delta_rotation +=
                (walker.next_float() - walker.next_float()) * walker.next_float() * 4.0;
            if walker.next_int_bounded(4) != 0 {
                if !can_reach(target, x, z, step, distance, thickness) {
                    return;
                }
                carve_ellipsoid(
                    mask,
                    target,
                    &Ellipsoid {
                        x,
                        y,
                        z,
                        horizontal_radius,
                        vertical_radius,
                    },
                    &skip,
                );
            }
        }
    }

    /// The per-height width factors: mostly sticky, re-rolled every
    /// `width_smoothness` on average, squared.
    fn init_width_factors(&self, ctx: &CarverContext, walker: &mut LegacyRandom) -> Vec<f32> {
        let mut factors = vec![1.0f32; ctx.gen_depth as usize];
        let mut width_factor = 1.0f32;
        for (index, slot) in factors.iter_mut().enumerate() {
            // The Java loop re-rolls unconditionally at index 0.
            if index == 0 || walker.next_int_bounded(self.shape.width_smoothness) == 0 {
                width_factor = 1.0 + walker.next_float() * walker.next_float();
            }
            *slot = width_factor * width_factor;
        }
        factors
    }

    /// Vertical radius squeeze: a center-biased factor times a random
    /// 0.75..1.0 draw.
    fn update_vertical_radius(
        &self,
        walker: &mut LegacyRandom,
        vertical_radius: f64,
        distance: f32,
        step: f32,
    ) -> f64 {
        let center_bias = 1.0 - (0.5 - step / distance).abs() * 2.0;
        let factor = self.shape.vertical_default + self.shape.vertical_center * center_bias;
        let random_scale = walker.next_float() * (1.0 - 0.75) + 0.75;
        f64::from(factor) * vertical_radius * f64::from(random_scale)
    }
}

/// Carver registry documents plus the per-biome carver lists, resolved
/// once from the operator-provisioned data root.
#[derive(Debug, Default)]
pub struct CarverData {
    carvers: HashMap<String, Rc<Carver>>,
    biomes: HashMap<String, Rc<Vec<Option<Rc<Carver>>>>>,
}

fn resolve_biome_carvers(
    biome_id: &str,
    document: &Value,
    known_ids: &HashSet<String>,
    supported: &HashMap<String, Rc<Carver>>,
) -> Result<Vec<Option<Rc<Carver>>>, WorldgenError> {
    let Some(entries) = document.get("carvers") else {
        return Ok(Vec::new());
    };
    // The configured-carver holder accepts a single identifier as well as
    // an array. Preserve source positions in either representation.
    let one = std::slice::from_ref(entries);
    let entries = entries.as_array().map(Vec::as_slice).unwrap_or(one);
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let name = entry.as_str().ok_or_else(|| {
                WorldgenError::Invalid(format!(
                    "{biome_id}: carvers[{index}] must be an identifier"
                ))
            })?;
            if !known_ids.contains(name) {
                return Err(WorldgenError::Invalid(format!(
                    "{biome_id}: carvers[{index}] references unknown {name}"
                )));
            }
            // Unsupported dimension-specific carvers keep their original
            // index, so later supported carvers retain their random seed.
            Ok(supported.get(name).map(Rc::clone))
        })
        .collect()
}

impl CarverData {
    /// Loads `data/<ns>/worldgen/carver/*.json` and
    /// `data/<ns>/worldgen/biome/*.json` under the data root. Carver
    /// types without a runtime implementation here (the nether family)
    /// retain empty positions in biome lists to preserve subsequent indices.
    pub fn load(root: &Path) -> Result<Self, WorldgenError> {
        let mut data = Self::default();
        let data_dir = root.join("data");
        let Some(namespaces) = read_dir_optional(&data_dir)? else {
            return Ok(data);
        };
        let mut carver_docs: Vec<(String, Value)> = Vec::new();
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
            for entry in walk_json(&worldgen.join("carver"))? {
                carver_docs.push((format!("{ns}:{}", stem(&entry)), read_json(&entry)?));
            }
            for entry in walk_json(&worldgen.join("biome"))? {
                biome_docs.push((format!("{ns}:{}", stem(&entry)), read_json(&entry)?));
            }
        }
        let known_ids: HashSet<String> = carver_docs.iter().map(|(id, _)| id.clone()).collect();
        for (id, document) in carver_docs {
            let Some(carver) = parse_carver(&id, &document)? else {
                continue; // unsupported carver type
            };
            data.carvers.insert(id, Rc::new(carver));
        }
        for (id, document) in biome_docs {
            let list = resolve_biome_carvers(&id, &document, &known_ids, &data.carvers)?;
            data.biomes.insert(id, Rc::new(list));
        }
        Ok(data)
    }

    pub fn is_empty(&self) -> bool {
        self.biomes.is_empty() && self.carvers.is_empty()
    }

    pub fn carvers_for_biome(&self, biome_id: &str) -> Option<Rc<Vec<Option<Rc<Carver>>>>> {
        self.biomes.get(biome_id).map(Rc::clone)
    }
}

/// Parses one carver registry document; `None` for types this module
/// does not implement.
fn parse_carver(id: &str, document: &Value) -> Result<Option<Carver>, WorldgenError> {
    let kind = document
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match kind {
        "minecraft:cave" => parse_cave_carver(document)
            .map(|carver| Some(Carver::Cave(carver)))
            .map_err(|message| WorldgenError::Invalid(format!("{id}: {message}"))),
        "minecraft:canyon" => parse_canyon_carver(document)
            .map(|carver| Some(Carver::Canyon(carver)))
            .map_err(|message| WorldgenError::Invalid(format!("{id}: {message}"))),
        _ => Ok(None),
    }
}

fn parse_cave_carver(document: &Value) -> Result<CaveCarver, String> {
    Ok(CaveCarver {
        probability: probability(document),
        y: parse_height(document, "y")?,
        count: parse_int(document.get("count").ok_or("missing count")?)?,
        thickness: parse_float(document.get("thickness").ok_or("missing thickness")?)?,
        weird_thickness_bias: document
            .get("weird_thickness_bias")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        room_vertical: parse_float(
            document
                .get("room_vertical_radius_multiplier")
                .ok_or("missing room_vertical_radius_multiplier")?,
        )?,
        horizontal_multiplier: parse_float(
            document
                .get("horizontal_radius_multiplier")
                .ok_or("missing horizontal_radius_multiplier")?,
        )?,
        vertical_multiplier: parse_float(
            document
                .get("vertical_radius_multiplier")
                .ok_or("missing vertical_radius_multiplier")?,
        )?,
        start_vertical: document
            .get("start_vertical_radius_multiplier")
            .map(parse_float)
            .transpose()?
            .unwrap_or(FloatSpec::Constant(1.0)),
        floor: parse_float(document.get("floor_level").ok_or("missing floor_level")?)?,
    })
}

fn parse_canyon_carver(document: &Value) -> Result<CanyonCarver, String> {
    let shape = document.get("shape").ok_or("missing shape")?;
    let number = |object: &Value, field: &str| -> Result<f32, String> {
        object
            .get(field)
            .and_then(Value::as_f64)
            .map(|value| value as f32)
            .ok_or_else(|| format!("missing {field}"))
    };
    Ok(CanyonCarver {
        probability: probability(document),
        y: parse_height(document, "y")?,
        vertical_rotation: parse_float(
            document
                .get("vertical_rotation")
                .ok_or("missing vertical_rotation")?,
        )?,
        shape: CanyonShape {
            distance_factor: parse_float(
                shape
                    .get("distance_factor")
                    .ok_or("missing distance_factor")?,
            )?,
            thickness: parse_float(shape.get("thickness").ok_or("missing thickness")?)?,
            width_smoothness: shape
                .get("width_smoothness")
                .and_then(Value::as_i64)
                .ok_or("missing width_smoothness")? as i32,
            horizontal_factor: parse_float(
                shape
                    .get("horizontal_radius_factor")
                    .ok_or("missing horizontal_radius_factor")?,
            )?,
            vertical_default: number(shape, "vertical_radius_default_factor")?,
            vertical_center: number(shape, "vertical_radius_center_factor")?,
            y_scale: parse_float(shape.get("y_scale").ok_or("missing y_scale")?)?,
        },
    })
}

fn probability(document: &Value) -> f32 {
    document
        .get("probability")
        .and_then(Value::as_f64)
        .map_or(0.0, |value| value as f32)
}

fn parse_anchor(value: &Value) -> Result<Anchor, String> {
    let object = value.as_object().ok_or("anchor must be an object")?;
    let (key, amount) = object
        .iter()
        .next()
        .and_then(|(key, value)| value.as_i64().map(|value| (key.as_str(), value)))
        .ok_or("anchor needs one integer field")?;
    match key {
        "absolute" => Ok(Anchor::Absolute(amount as i32)),
        "above_bottom" => Ok(Anchor::AboveBottom(amount as i32)),
        "below_top" => Ok(Anchor::BelowTop(amount as i32)),
        "relative_to_sea_level" => Ok(Anchor::RelativeToSeaLevel(amount as i32)),
        other => Err(format!("unknown anchor {other}")),
    }
}

fn parse_height(document: &Value, field: &str) -> Result<HeightSpec, String> {
    let height = document
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("missing {field}"))?;
    let kind = height
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{field} needs a type"))?;
    if kind != "minecraft:uniform" {
        return Err(format!("unsupported height provider {kind}"));
    }
    Ok(HeightSpec {
        min: parse_anchor(
            height
                .get("min_inclusive")
                .ok_or_else(|| format!("{field} missing min_inclusive"))?,
        )?,
        max: parse_anchor(
            height
                .get("max_inclusive")
                .ok_or_else(|| format!("{field} missing max_inclusive"))?,
        )?,
    })
}

fn parse_int(value: &Value) -> Result<IntSpec, String> {
    if let Some(number) = value.as_i64() {
        return Ok(IntSpec::Constant(number as i32));
    }
    dispatch(value, |kind, object| match kind {
        "minecraft:constant" => object
            .get("value")
            .and_then(Value::as_i64)
            .map(|value| IntSpec::Constant(value as i32))
            .ok_or_else(|| "constant needs a value".to_owned()),
        "minecraft:uniform" | "minecraft:very_biased_to_bottom" => {
            let min = object
                .get("min_inclusive")
                .and_then(Value::as_i64)
                .ok_or("uniform needs min_inclusive")? as i32;
            let max = object
                .get("max_inclusive")
                .and_then(Value::as_i64)
                .ok_or("uniform needs max_inclusive")? as i32;
            if kind == "minecraft:uniform" {
                Ok(IntSpec::Uniform { min, max })
            } else {
                Ok(IntSpec::VeryBiasedToBottom { min, max })
            }
        }
        other => Err(format!("unsupported int provider {other}")),
    })
}

fn parse_float(value: &Value) -> Result<FloatSpec, String> {
    if let Some(number) = value.as_f64() {
        return Ok(FloatSpec::Constant(number as f32));
    }
    dispatch(value, |kind, object| match kind {
        "minecraft:constant" => object
            .get("value")
            .and_then(Value::as_f64)
            .map(|value| FloatSpec::Constant(value as f32))
            .ok_or_else(|| "constant needs a value".to_owned()),
        "minecraft:uniform" => {
            let min = object
                .get("min_inclusive")
                .and_then(Value::as_f64)
                .ok_or("uniform needs min_inclusive")? as f32;
            let max = object
                .get("max_exclusive")
                .and_then(Value::as_f64)
                .ok_or("uniform needs max_exclusive")? as f32;
            Ok(FloatSpec::Uniform { min, max })
        }
        "minecraft:trapezoid" => {
            let min = object
                .get("min")
                .and_then(Value::as_f64)
                .ok_or("trapezoid needs min")? as f32;
            let max = object
                .get("max")
                .and_then(Value::as_f64)
                .ok_or("trapezoid needs max")? as f32;
            let plateau = object
                .get("plateau")
                .and_then(Value::as_f64)
                .ok_or("trapezoid needs plateau")? as f32;
            Ok(FloatSpec::Trapezoid { min, max, plateau })
        }
        other => Err(format!("unsupported float provider {other}")),
    })
}

/// Reads a provider object's `type` and hands the body to the matcher.
fn dispatch<T>(
    value: &Value,
    build: impl Fn(&str, &serde_json::Map<String, Value>) -> Result<T, String>,
) -> Result<T, String> {
    let object = value
        .as_object()
        .ok_or("provider must be a value or object")?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or("provider needs a type")?;
    build(kind, object)
}

#[cfg(test)]
mod tests;
