//! Decoration targets and the placement routines of the modeled feature
//! families (disk, spring, ore, simple block, block column).

use super::*;

/// The block state a spring's fluid state places. The pack writes the fluid
/// state, `{"id": "minecraft:water", "properties": {"falling": "true"}}`, but
/// `falling` belongs to the fluid: the 26.3 block-state report gives
/// `minecraft:water` and `minecraft:lava` only a `level` property, so the
/// document's name with its fluid properties would be an unknown block state.
/// A still fluid is a source, whichever way `falling` reads, and a source is
/// the block's default state. Any other fluid has no mapping here.
pub(super) fn spring_block_state(value: &Value) -> Option<String> {
    let id = value.as_str().or_else(|| value.get("id")?.as_str())?;
    matches!(id, "minecraft:water" | "minecraft:lava").then(|| id.to_string())
}

/// Whether a block read for a spring's neighbour check is air.
pub(super) fn is_air_block(block: Option<&str>) -> bool {
    block.is_some_and(|block| {
        matches!(
            base_block_id(block),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        )
    })
}

/// Whether the given block matches any block in `valid_blocks` (or tag if prefixed by `#`).
pub(super) fn is_valid_block(
    block: Option<&str>,
    valid_blocks: &[String],
    tags: &BlockTags,
) -> bool {
    let Some(block) = block else {
        return false;
    };
    let base = base_block_id(block);
    valid_blocks.iter().any(|valid| {
        if let Some(tag) = valid.strip_prefix('#') {
            tags.contains(tag, base) || tags.contains(tag, block)
        } else {
            let valid_base = base_block_id(valid);
            valid_base == base || valid == block
        }
    })
}

pub(super) struct DiskFeature<'a> {
    pub(super) state_provider: &'a StateProvider,
    pub(super) target_predicate: &'a BlockPredicate,
    pub(super) radius: &'a RangeProvider,
    pub(super) half_height: i32,
}

/// Places a circular disk feature around `pos`.
pub(super) fn place_disk(
    target: &mut dyn DecorationTarget,
    random: &mut DecorationRandom,
    tags: &BlockTags,
    disk: DiskFeature<'_>,
    pos: DecorationPos,
) -> usize {
    let DiskFeature {
        state_provider,
        target_predicate,
        radius,
        half_height,
    } = disk;
    let r = radius.sample(target.min_y(), target.height(), random);
    if r < 0 || half_height < 0 {
        return 0;
    }
    let mut placed = 0;
    let r_sq = r * r;
    // Each column finishes top to bottom before the next starts, so a provider
    // that reads the block below sees it as the terrain left it, not as this
    // disk rewrote it. A candidate outside the chunk is clipped before its
    // provider runs; no pack disk has a provider that draws.
    for dx in -r..=r {
        let px = pos.x + dx;
        for dz in -r..=r {
            if dx * dx + dz * dz > r_sq {
                continue;
            }
            let pz = pos.z + dz;
            for dy in (-half_height..=half_height).rev() {
                let py = pos.y + dy;
                if !target.writable(px, py, pz) {
                    continue;
                }
                let candidate_pos = DecorationPos {
                    x: px,
                    y: py,
                    z: pz,
                };
                if !target_predicate.test(target, candidate_pos, tags) {
                    continue;
                }
                let Some(state) = state_provider.sample(target, candidate_pos, tags, random) else {
                    continue;
                };
                target.set_block(px, py, pz, state.as_ref());
                placed += 1;
            }
        }
    }
    placed
}

pub(super) struct SpringFeature<'a> {
    pub(super) state: &'a str,
    pub(super) rock_count: i32,
    pub(super) hole_count: i32,
    pub(super) requires_block_below: bool,
    pub(super) valid_blocks: &'a [String],
}

/// Places a fluid spring feature at `pos`: the ceiling must be valid, the block
/// below too when the feature requires it, and the four sides and the block
/// below must then hold exactly `rock_count` valid blocks and `hole_count` air
/// cells. Neighbours past the chunk edge are read from the terrain, so a pocket
/// on a border is judged as it would be anywhere else.
pub(super) fn place_spring(
    target: &mut dyn DecorationTarget,
    tags: &BlockTags,
    spring: SpringFeature<'_>,
    pos: DecorationPos,
) -> usize {
    let SpringFeature {
        state,
        rock_count,
        hole_count,
        requires_block_below,
        valid_blocks,
    } = spring;
    if !target.writable(pos.x, pos.y, pos.z) {
        return 0;
    }
    let valid = |target: &dyn DecorationTarget, x: i32, y: i32, z: i32| {
        is_valid_block(target.read_block(x, y, z).as_deref(), valid_blocks, tags)
    };
    if !valid(target, pos.x, pos.y + 1, pos.z) {
        return 0;
    }
    if requires_block_below && !valid(target, pos.x, pos.y - 1, pos.z) {
        return 0;
    }
    if !target.is_air(pos.x, pos.y, pos.z) && !valid(target, pos.x, pos.y, pos.z) {
        return 0;
    }
    let neighbors = [
        (pos.x - 1, pos.y, pos.z),
        (pos.x + 1, pos.y, pos.z),
        (pos.x, pos.y, pos.z - 1),
        (pos.x, pos.y, pos.z + 1),
        (pos.x, pos.y - 1, pos.z),
    ];
    let mut rocks = 0i32;
    let mut holes = 0i32;
    for (nx, ny, nz) in neighbors {
        let block = target.read_block(nx, ny, nz);
        if is_valid_block(block.as_deref(), valid_blocks, tags) {
            rocks += 1;
        } else if is_air_block(block.as_deref()) {
            holes += 1;
        }
    }
    if rocks == rock_count && holes == hole_count {
        target.set_block(pos.x, pos.y, pos.z, state);
        1
    } else {
        0
    }
}

/// A position the decoration loop is considering, in absolute coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecorationPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// The terrain a decoration pass reads and writes. The generator implements
/// this over one chunk's resolved columns; writing it here keeps the vein
/// geometry independent of how the terrain was produced.
///
/// Every position the pass writes is inside the implementation's own chunk.
/// Reads reach one column beyond it: the six-neighbour air test, and the
/// heightmap the ore anchor gate walks over its box's footprint, whose halo
/// columns stay within the 3x3 chunk region the pass already resolves biomes
/// over.
pub trait DecorationTarget {
    /// The dimension's lowest buildable row.
    fn min_y(&self) -> i32;
    /// The number of buildable rows.
    fn height(&self) -> i32;
    /// Whether a position may be written at all. Positions outside the
    /// decorated chunk are refused *before* the replacement rules are
    /// evaluated, so they cost no draws either; the neighbour chunk's own
    /// pass paints its half of a vein that straddles the border.
    fn writable(&self, x: i32, y: i32, z: i32) -> bool;
    /// Optional inclusive horizontal chunk bounds `(min_x, max_x, min_z, max_z)`
    /// of writable positions, used to skip halo anchors that cannot reach.
    fn writable_bounds(&self) -> Option<(i32, i32, i32, i32)> {
        None
    }
    /// The block name currently at the position, `None` where the world
    /// holds air. Earlier decorations in this pass have already painted
    /// here: a later vein may replace stone a blob turned to granite.
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<&str>;
    /// The block a predicate sees at any position in the build volume. Inside
    /// the decorated chunk this is [`Self::block_at`] with an empty row read
    /// as air; beyond it the generator answers with the terrain as the fill
    /// and carver passes left it, since the neighbour's decorations have not
    /// been replayed. `None` means the row is outside the build volume.
    fn read_block(&self, x: i32, y: i32, z: i32) -> Option<Cow<'_, str>> {
        self.block_at(x, y, z).map(Cow::Borrowed)
    }
    /// Whether the position holds air, for the air-exposure test. Reads
    /// beyond the writable zone still have an answer, because the six
    /// neighbours of a border block are queried regardless.
    fn is_air(&self, x: i32, y: i32, z: i32) -> bool;
    /// Writes the block, replacing whatever was there.
    fn set_block(&mut self, x: i32, y: i32, z: i32, name: &str);
    /// The biome at a position, which the `minecraft:biome` filter asks for.
    fn biome_at(&self, x: i32, y: i32, z: i32) -> Option<String>;
    /// Allocation-free biome predicate check for the `minecraft:biome` filter.
    fn has_biome(&self, x: i32, y: i32, z: i32, allowed: &dyn Fn(&str) -> bool) -> bool {
        self.biome_at(x, y, z).is_some_and(|biome| allowed(&biome))
    }
    /// The absolute Y of the highest terrain row of the column, as the
    /// world-gen ocean-floor heightmap reports it: fluid and air above a sea
    /// floor do not count. The ore anchor gate reads it for every column of
    /// its box's footprint, including the halo over the neighbour chunks.
    fn ocean_floor_height(&self, x: i32, z: i32) -> i32;
    /// One block above the highest matching block at `(x, z)` for `kind`, or
    /// `min_y()` when the column contains no matching block.
    fn heightmap_height(&self, kind: HeightmapKind, x: i32, z: i32, tags: &BlockTags) -> i32 {
        default_heightmap_height(self, kind, x, z, tags)
    }
}

pub(super) fn default_heightmap_height<T: DecorationTarget + ?Sized>(
    target: &T,
    kind: HeightmapKind,
    x: i32,
    z: i32,
    tags: &BlockTags,
) -> i32 {
    let min_y = target.min_y();
    let max_y = min_y + target.height() - 1;
    for y in (min_y..=max_y).rev() {
        if let Some(block) = target.read_block(x, y, z)
            && kind.matches_block(block.as_ref(), tags)
        {
            return y + 1;
        }
    }
    if matches!(
        kind,
        HeightmapKind::OceanFloor | HeightmapKind::OceanFloorWg
    ) {
        let fallback = target.ocean_floor_height(x, z);
        if fallback > min_y {
            return fallback;
        }
    }
    min_y
}

/// Java `Mth.SIN`: a 65536-entry table indexed by a fixed-point multiple of
/// the argument, so the runtime's sine is a table lookup rather than the
/// library function. Built once per process; 256 KiB, independent of the
/// world.
pub(super) const SIN_STEPS: usize = 65_536;
pub(super) const SIN_SCALE: f64 = 10_430.378_350_470_453;

pub(super) fn mth_sin(value: f64) -> f32 {
    static TABLE: OnceLock<[f32; SIN_STEPS]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0.0f32; SIN_STEPS];
        for (index, value) in table.iter_mut().enumerate() {
            *value = f64::sin(index as f64 / SIN_SCALE) as f32;
        }
        table
    });
    let index = (value * SIN_SCALE) as i64 & (SIN_STEPS as i64 - 1);
    table[index as usize]
}

/// Java `Mth.ceil(float)`: widen, round up, truncate toward zero.
pub(super) fn ceil_float(value: f32) -> i32 {
    f64::ceil(f64::from(value)) as i32
}

/// Java `Mth.floor(double)`.
pub(super) fn floor_double(value: f64) -> i32 {
    value.floor() as i32
}

/// Java `Mth.lerp(double, double, double)`.
pub(super) fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

/// The float pi literal the ore geometry multiplies its angle by; Java's
/// `(float)Math.PI` is not the exact half-turn.
pub(super) const PI_F32: f32 = core::f32::consts::PI;

/// Whether the discard roll lets a block through without the air test.
/// A zero chance skips the test entirely and a certain one rejects it, both
/// without drawing; only a strict fraction costs one `nextFloat`.
pub(super) fn should_skip_air_check(random: &mut DecorationRandom, discard: f32) -> bool {
    if discard <= 0.0 {
        return true;
    }
    if discard >= 1.0 {
        return false;
    }
    random.next_float() >= discard
}

/// Whether any of the six cardinal neighbours of a position is air.
pub(super) fn adjacent_to_air(target: &dyn DecorationTarget, x: i32, y: i32, z: i32) -> bool {
    [
        (x, y - 1, z),
        (x, y + 1, z),
        (x, y, z - 1),
        (x, y, z + 1),
        (x - 1, y, z),
        (x + 1, y, z),
    ]
    .iter()
    .any(|(nx, ny, nz)| target.is_air(*nx, *ny, *nz))
}

/// Whether any column of a box's footprint reaches down to the box's base
/// row. The reference runtime's ore placement asks its world-gen ocean-floor
/// heightmap this question over the `width + 1` by `width + 1` grid of columns
/// the box covers, and gives up on the attempt when none of them answers: a
/// box that floats entirely above the terrain can hold nothing a stone-family
/// replacement test accepts, so the attempt is over before it draws a radius.
/// What it *does* decide is where the attempt leaves the stream: the rejected
/// one costs only the three draws that fixed its axis, and every later attempt
/// of the same feature resumes from there.
pub(super) fn anchored(
    target: &dyn DecorationTarget,
    base_x: i32,
    base_y: i32,
    base_z: i32,
    width: i32,
) -> bool {
    (base_x..=base_x + width)
        .flat_map(|x| (base_z..=base_z + width).map(move |z| (x, z)))
        .any(|(x, z)| base_y <= target.ocean_floor_height(x, z))
}

/// Places one `minecraft:ore` vein, returning the number of blocks written.
///
/// The shape is the documented two-endpoint ellipsoid sweep: three draws fix
/// the vein's axis and height jitter, the box those endpoints define is
/// anchored [`anchored`] against the heightmap, `size` points along the axis
/// each get a radius from a single `nextDouble`, points whose radius difference
/// exceeds their distance are culled, and the surviving spheres are written
/// where the column's replacement rules accept them.
pub(super) fn place_ore(
    target: &mut dyn DecorationTarget,
    random: &mut DecorationRandom,
    tags: &BlockTags,
    size: i32,
    targets: &[OreTarget],
    discard: f32,
    origin: DecorationPos,
) -> usize {
    let angle = random.next_float() * PI_F32;
    let scale = size as f32 / 8.0;
    let span = ceil_float((size as f32 / 16.0 * 2.0 + 1.0) / 2.0);
    let extent = f64::from(scale);
    let (sin, cos) = (f64::sin(f64::from(angle)), f64::cos(f64::from(angle)));
    let start_x = origin.x as f64 + sin * extent;
    let end_x = origin.x as f64 - sin * extent;
    let start_z = origin.z as f64 + cos * extent;
    let end_z = origin.z as f64 - cos * extent;
    let start_y = f64::from(origin.y + random.next_int_bounded(3) - 2);
    let end_y = f64::from(origin.y + random.next_int_bounded(3) - 2);
    let reach = ceil_float(scale);
    let base_x = origin.x - reach - span;
    let base_y = origin.y - 2 - span;
    let base_z = origin.z - reach - span;
    let width = 2 * (reach + span);
    let box_height = 2 * (span + 2);
    if !anchored(target, base_x, base_y, base_z, width) {
        return 0;
    }
    let points = size.max(0) as usize;
    let mut vein = vec![0.0f64; points * 4];
    for index in 0..points {
        let step = index as f32 / size as f32;
        let jitter = random.next_double() * size as f64 / 16.0;
        let radius = (f64::from(mth_sin(f64::from(PI_F32 * step)) + 1.0f32) * jitter + 1.0) / 2.0;
        let offset = index * 4;
        vein[offset] = lerp(f64::from(step), start_x, end_x);
        vein[offset + 1] = lerp(f64::from(step), start_y, end_y);
        vein[offset + 2] = lerp(f64::from(step), start_z, end_z);
        vein[offset + 3] = radius;
    }
    // A point the length of whose radius difference exceeds its distance to
    // another point is inside it, and drops out of the sweep.
    for left in 0..points.saturating_sub(1) {
        if vein[left * 4 + 3] <= 0.0 {
            continue;
        }
        for right in left + 1..points {
            if vein[right * 4 + 3] <= 0.0 {
                continue;
            }
            let radius_delta = vein[left * 4 + 3] - vein[right * 4 + 3];
            let distance = (vein[left * 4] - vein[right * 4]).powi(2)
                + (vein[left * 4 + 1] - vein[right * 4 + 1]).powi(2)
                + (vein[left * 4 + 2] - vein[right * 4 + 2]).powi(2);
            if radius_delta * radius_delta > distance {
                if radius_delta > 0.0 {
                    vein[right * 4 + 3] = -1.0;
                } else {
                    vein[left * 4 + 3] = -1.0;
                }
            }
        }
    }
    let mut written = 0usize;
    let cells = width.max(0) as usize * box_height.max(0) as usize * width.max(0) as usize;
    let mut visited = vec![false; cells];
    for point in 0..points {
        let radius = vein[point * 4 + 3];
        if radius < 0.0 {
            continue;
        }
        let center_x = vein[point * 4];
        let center_y = vein[point * 4 + 1];
        let center_z = vein[point * 4 + 2];
        let min_x = floor_double(center_x - radius).max(base_x);
        let min_y = floor_double(center_y - radius).max(base_y);
        let min_z = floor_double(center_z - radius).max(base_z);
        let max_x = floor_double(center_x + radius).max(min_x);
        let max_y = floor_double(center_y + radius).max(min_y);
        let max_z = floor_double(center_z + radius).max(min_z);
        for x in min_x..=max_x {
            let dx = (f64::from(x) + 0.5 - center_x) / radius;
            if dx * dx >= 1.0 {
                continue;
            }
            for y in min_y..=max_y {
                let dy = (f64::from(y) + 0.5 - center_y) / radius;
                if dx * dx + dy * dy >= 1.0 {
                    continue;
                }
                for z in min_z..=max_z {
                    let dz = (f64::from(z) + 0.5 - center_z) / radius;
                    if dx * dx + dy * dy + dz * dz >= 1.0 {
                        continue;
                    }
                    if y < target.min_y() || y > target.min_y() + target.height() - 1 {
                        continue;
                    }
                    let slot = (x - base_x) as usize
                        + (y - base_y) as usize * width as usize
                        + (z - base_z) as usize * width as usize * box_height as usize;
                    if visited.get(slot).copied().unwrap_or(true) {
                        continue;
                    }
                    visited[slot] = true;
                    if !target.writable(x, y, z) {
                        continue;
                    }
                    let current = target.block_at(x, y, z);
                    for replacement in targets {
                        if !replacement.predicate.test(current, y, tags) {
                            continue;
                        }
                        if !should_skip_air_check(random, discard)
                            && adjacent_to_air(target, x, y, z)
                        {
                            continue;
                        }
                        target.set_block(x, y, z, &replacement.state);
                        written += 1;
                        break;
                    }
                }
            }
        }
    }
    written
}

/// Places one `minecraft:simple_block`. The state is drawn first: two chunks
/// replaying one anchor must consume the same stream whether or not this
/// candidate lands in the chunk being decorated. The block then only replaces
/// air/water/replaceable, and only where the plant's ground holds.
pub(super) fn place_simple_block(
    target: &mut dyn DecorationTarget,
    random: &mut DecorationRandom,
    tags: &BlockTags,
    to_place: &StateProvider,
    pos: DecorationPos,
) -> usize {
    let Some(state) = to_place.sample(target, pos, tags, random) else {
        return 0;
    };
    plant_support::place_plant(target, tags, state.as_ref(), pos)
}

pub(super) struct BlockColumnFeature<'a> {
    pub(super) direction: Direction,
    pub(super) allowed_placement: &'a BlockPredicate,
    pub(super) prioritize_tip: bool,
    pub(super) layers: &'a [BlockColumnLayer],
}

/// Places a `minecraft:block_column` along `direction`, trimming from the start
/// when `prioritize_tip` is `true` and from the tip otherwise (`Cave Vines (feature)`
/// rev 3818484).
pub(super) fn place_block_column(
    target: &mut dyn DecorationTarget,
    random: &mut DecorationRandom,
    tags: &BlockTags,
    column: BlockColumnFeature<'_>,
    pos: DecorationPos,
) -> usize {
    let BlockColumnFeature {
        direction,
        allowed_placement,
        prioritize_tip,
        layers,
    } = column;
    let min_y = target.min_y();
    let max_y = min_y + target.height();
    let heights: Vec<usize> = layers
        .iter()
        .map(|layer| layer.height.sample(min_y, target.height(), random).max(0) as usize)
        .collect();
    let total: usize = heights.iter().sum();
    if total == 0 {
        return 0;
    }
    let (dx, dy, dz) = direction.unit_vector();
    let mut avail = 0usize;
    let mut scan = pos;
    while avail < total {
        if scan.y < min_y
            || scan.y >= max_y
            || !target.writable(scan.x, scan.y, scan.z)
            || !allowed_placement.test(target, scan, tags)
        {
            break;
        }
        avail += 1;
        scan.x += dx;
        scan.y += dy;
        scan.z += dz;
    }
    if avail == 0 {
        return 0;
    }
    let mut trim_from_start = if prioritize_tip { total - avail } else { 0 };
    let mut remaining = avail;
    let mut cur = pos;
    let mut written = 0usize;
    for (layer, height) in layers.iter().zip(heights) {
        let skip = trim_from_start.min(height);
        trim_from_start -= skip;
        let take = (height - skip).min(remaining);
        remaining -= take;
        for _ in 0..take {
            if let Some(state) = layer.provider.sample(target, cur, tags, random) {
                target.set_block(cur.x, cur.y, cur.z, state.as_ref());
                written += 1;
            }
            cur.x += dx;
            cur.y += dy;
            cur.z += dz;
        }
    }
    written
}
