//! Basic Minecraft fluid flow mechanics and simulation.
//!
//! Provides fluid types (`Water`, `Lava`), block-state parsing and formatting,
//! inward slope drop search, infinite water source generation, fluid-fluid
//! collision resolution, and spread computation over a [`FluidWorldView`].
//!
//! # Scope & In-Memory Simulation
//!
//! This module provides an in-memory simulation model (`FluidSimulator`, `InMemoryFluidWorld`)
//! for testing and verifying fluid dynamics algorithms. It is currently scoped as a standalone
//! simulation component until wired to authoritative multi-chunk world state in future integration milestones.
//!
//! # Vanilla Specification & Source Citations
//!
//! - **Level Property**: `generated/reports/blocks.json` from the official Java 26.3 server data
//!   generator lists a single property for `minecraft:water` and `minecraft:lava`: `level`,
//!   `0..=15`. There is no `falling` block-state property.
//! - **Falling Encoding**: [Minecraft Wiki: Water](https://minecraft.wiki/w/Water) and
//!   [Lava](https://minecraft.wiki/w/Lava) document block-state level 8 as falling fluid, and
//!   `falling` as a fluid-state property only. [`FluidState`] keeps `falling` as a separate field
//!   and translates it to and from `level` at the block-state boundary: `8 + level` when falling.
//! - **Fluid Collisions**: Water touching lava source blocks creates `minecraft:obsidian`;
//!   water touching flowing lava creates `minecraft:cobblestone`; lava flowing on top of water creates `minecraft:stone`.
//!   Simultaneous horizontal arrival of water and lava at an empty cell creates `minecraft:cobblestone`.

use std::fmt;
use std::str::FromStr;

/// Standard Minecraft block identifiers for fluid mechanics.
pub const BLOCK_WATER: &str = "minecraft:water";
pub const BLOCK_LAVA: &str = "minecraft:lava";
pub const BLOCK_OBSIDIAN: &str = "minecraft:obsidian";
pub const BLOCK_COBBLESTONE: &str = "minecraft:cobblestone";
pub const BLOCK_STONE: &str = "minecraft:stone";
pub const BLOCK_AIR: &str = "minecraft:air";

/// Block-state `level` values from here up mean falling fluid.
const FALLING_LEVEL: u8 = 8;
const MAX_BLOCK_STATE_LEVEL: u8 = 15;

/// A 3D discrete block position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            z: self.z + dz,
        }
    }

    pub fn up(self) -> Self {
        self.offset(0, 1, 0)
    }

    pub fn down(self) -> Self {
        self.offset(0, -1, 0)
    }

    pub fn north(self) -> Self {
        self.offset(0, 0, -1)
    }

    pub fn south(self) -> Self {
        self.offset(0, 0, 1)
    }

    pub fn west(self) -> Self {
        self.offset(-1, 0, 0)
    }

    pub fn east(self) -> Self {
        self.offset(1, 0, 0)
    }

    /// Four horizontal cardinal neighbors: North, South, West, East.
    pub fn horizontal_neighbors(self) -> [BlockPos; 4] {
        [self.north(), self.south(), self.west(), self.east()]
    }
}

/// The kind of fluid: Water or Lava.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FluidType {
    Water,
    Lava,
}

impl FluidType {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Water => "water",
            Self::Lava => "lava",
        }
    }

    pub const fn block_name(self) -> &'static str {
        match self {
            Self::Water => BLOCK_WATER,
            Self::Lava => BLOCK_LAVA,
        }
    }

    /// Maximum flowing level in the overworld.
    /// Water can reach up to level 7 (7 blocks from source).
    /// Lava in the overworld can reach up to level 3 (3 blocks from source).
    pub const fn max_level(self) -> u8 {
        match self {
            Self::Water => 7,
            Self::Lava => 3,
        }
    }

    /// Level increase per horizontal block spread.
    pub const fn flow_decay(self) -> u8 {
        1
    }

    /// Maximum horizontal search distance to locate a drop / cliff.
    /// 4 for Water, 2 for Lava.
    pub const fn slope_search_distance(self) -> u8 {
        match self {
            Self::Water => 4,
            Self::Lava => 2,
        }
    }

    /// Whether this fluid can form infinite sources when adjacent to >= 2 source blocks.
    pub const fn can_create_infinite_source(self) -> bool {
        match self {
            Self::Water => true,
            Self::Lava => false,
        }
    }
}

/// The state of a fluid block: type, flow level, and whether it is falling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FluidState {
    pub type_: FluidType,
    pub level: u8,
    pub falling: bool,
}

impl FluidState {
    pub const fn new(type_: FluidType, level: u8, falling: bool) -> Self {
        Self {
            type_,
            level,
            falling,
        }
    }

    pub const fn source(type_: FluidType) -> Self {
        Self {
            type_,
            level: 0,
            falling: false,
        }
    }

    pub const fn is_source(&self) -> bool {
        self.level == 0 && !self.falling
    }

    /// The level this fluid gives a horizontal neighbor, or `None` past its
    /// reach. Falling fluid counts as full strength, so spread restarts at the
    /// foot of a fall.
    fn spread_level(&self) -> Option<u8> {
        let strength = if self.falling { 0 } else { self.level };
        let level = strength + self.type_.flow_decay();
        (level <= self.type_.max_level()).then_some(level)
    }

    /// Format to standard Minecraft block state representation.
    pub fn to_block_state(&self) -> String {
        let name = self.type_.block_name();
        let level = if self.falling {
            self.level + FALLING_LEVEL
        } else {
            self.level
        };
        format!("{name}[level={level}]")
    }

    /// Format to compact block state where non-falling source blocks omit properties.
    pub fn to_canonical_block_state(&self) -> String {
        if self.is_source() {
            self.type_.block_name().to_string()
        } else {
            self.to_block_state()
        }
    }

    /// Parse from a Minecraft block state string.
    ///
    /// Supports:
    /// - `minecraft:water` or `water`
    /// - `minecraft:water[level=0]`
    /// - `minecraft:water[level=N]`
    /// - `minecraft:lava` or `lava`
    /// - `minecraft:lava[level=0]`
    /// - `minecraft:lava[level=N]`
    ///
    /// `level` is `0..=15`; `8..=15` decode as falling. Any other property is
    /// rejected because these blocks have none.
    pub fn from_block_state(s: &str) -> Option<Self> {
        let s = s.trim();
        let (raw_name, props_str) = match s.find('[') {
            Some(idx) => {
                let name = &s[..idx];
                let rest = &s[idx + 1..];
                let props = rest.strip_suffix(']')?;
                (name, Some(props))
            }
            None => (s, None),
        };

        let type_ = if raw_name == BLOCK_WATER || raw_name == "water" {
            FluidType::Water
        } else if raw_name == BLOCK_LAVA || raw_name == "lava" {
            FluidType::Lava
        } else {
            return None;
        };

        let mut level: u8 = 0;

        if let Some(props) = props_str {
            for part in props.split(',') {
                let (key, val) = part.split_once('=')?;
                if key.trim() != "level" {
                    return None;
                }
                level = val.trim().parse().ok()?;
            }
        }

        if level > MAX_BLOCK_STATE_LEVEL {
            return None;
        }

        let falling = level >= FALLING_LEVEL;
        Some(Self {
            type_,
            level: if falling {
                level - FALLING_LEVEL
            } else {
                level
            },
            falling,
        })
    }
}

impl fmt::Display for FluidState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_block_state())
    }
}

impl FromStr for FluidState {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_block_state(s).ok_or("invalid fluid block state")
    }
}

/// Read-only view into the world for fluid simulation queries.
pub trait FluidWorldView {
    /// Returns the fluid state at `pos`, if any fluid is present.
    fn get_fluid(&self, pos: BlockPos) -> Option<FluidState>;

    /// Returns whether the block at `pos` is solid (blocks fluid flow).
    fn is_solid(&self, pos: BlockPos) -> bool;

    /// Returns whether fluid can enter or pass through `pos`.
    fn can_flow_into(&self, pos: BlockPos) -> bool {
        !self.is_solid(pos)
    }

    /// Returns the block identifier at `pos` (e.g. "minecraft:air", "minecraft:stone", etc.).
    fn get_block_id(&self, pos: BlockPos) -> String {
        if let Some(fluid) = self.get_fluid(pos) {
            fluid.to_block_state()
        } else if self.is_solid(pos) {
            BLOCK_STONE.to_string()
        } else {
            BLOCK_AIR.to_string()
        }
    }
}

/// The outcome of evaluating a cell during fluid simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowOutcome {
    /// The cell contains fluid.
    Fluid(FluidState),
    /// The fluid collided and solidified into a solid block (e.g. obsidian, cobblestone, stone).
    Solidify(&'static str),
    /// The cell is empty (air).
    Empty,
}

/// Whether fluid at `pos` can actually descend. A source of the same fluid
/// below is already full, so it does not take precedence over lateral flow.
fn flows_down_from(world: &dyn FluidWorldView, pos: BlockPos, fluid_type: FluidType) -> bool {
    let below = pos.down();
    !world.is_solid(below)
        && !world
            .get_fluid(below)
            .is_some_and(|fluid| fluid.type_ == fluid_type && fluid.is_source())
}

/// The state `fluid` at `neighbor` would give `pos` by horizontal flow, if any.
fn horizontal_arrival(
    world: &dyn FluidWorldView,
    neighbor: BlockPos,
    fluid: FluidState,
    pos: BlockPos,
) -> Option<FluidState> {
    if !preferred_flow_directions(world, neighbor, fluid.type_).contains(&pos) {
        return None;
    }
    let level = fluid.spread_level()?;
    Some(FluidState::new(fluid.type_, level, false))
}

/// Calculates the horizontal neighbor positions that fluid at `origin` will spread towards,
/// according to inward slope drop search and downward flow precedence.
pub fn preferred_flow_directions(
    world: &dyn FluidWorldView,
    origin: BlockPos,
    fluid_type: FluidType,
) -> Vec<BlockPos> {
    // Downward flow takes precedence: if fluid can descend from origin,
    // it falls down and does NOT spread horizontally.
    if flows_down_from(world, origin, fluid_type) {
        return Vec::new();
    }

    let neighbors = origin.horizontal_neighbors();
    let search_dist = fluid_type.slope_search_distance() as usize;

    let mut drop_distances: [Option<usize>; 4] = [None; 4];

    for (i, &neighbor) in neighbors.iter().enumerate() {
        if world.is_solid(neighbor) {
            continue;
        }
        if flows_down_from(world, neighbor, fluid_type) {
            // Drop right at the neighbor! Distance is 1.
            drop_distances[i] = Some(1);
            continue;
        }

        // BFS outward from neighbor to locate drops within search_dist
        if search_dist > 1 {
            let mut visited = Vec::with_capacity(32);
            visited.push(origin);
            visited.push(neighbor);

            let mut queue: Vec<(BlockPos, usize)> = Vec::with_capacity(32);
            queue.push((neighbor, 1));
            let mut head = 0;

            while head < queue.len() {
                let (curr, dist) = queue[head];
                head += 1;

                if dist >= search_dist {
                    continue;
                }

                for next in curr.horizontal_neighbors() {
                    if visited.contains(&next) || world.is_solid(next) {
                        continue;
                    }
                    visited.push(next);

                    if flows_down_from(world, next, fluid_type) {
                        let drop_dist = dist + 1;
                        if drop_distances[i].is_none_or(|d| drop_dist < d) {
                            drop_distances[i] = Some(drop_dist);
                        }
                    } else if dist + 1 < search_dist {
                        queue.push((next, dist + 1));
                    }
                }
            }
        }
    }

    let min_dist = drop_distances.iter().filter_map(|&d| d).min();

    let mut result = Vec::with_capacity(4);
    for (i, &neighbor) in neighbors.iter().enumerate() {
        if world.is_solid(neighbor) {
            continue;
        }
        match min_dist {
            Some(min) => {
                if drop_distances[i] == Some(min) {
                    result.push(neighbor);
                }
            }
            None => {
                result.push(neighbor);
            }
        }
    }

    result
}

/// Check if a fluid collision occurs at `pos`.
///
/// Collision rules:
/// 1. Water touches lava source (`level == 0`) -> converts to `minecraft:obsidian`.
/// 2. Water touches flowing lava (`level > 0`) -> converts to `minecraft:cobblestone`.
/// 3. Lava flows vertically on top of water -> converts to `minecraft:stone`.
/// 4. Water and lava simultaneously arrive horizontally at an empty cell -> converts to `minecraft:cobblestone`.
pub fn check_fluid_collision(world: &dyn FluidWorldView, pos: BlockPos) -> Option<&'static str> {
    let current = world.get_fluid(pos);

    // Rule 3: Lava flows vertically on top of water -> converts to minecraft:stone.
    if let Some(fluid) = current
        && fluid.type_ == FluidType::Water
        && let Some(above) = world.get_fluid(pos.up())
        && above.type_ == FluidType::Lava
    {
        return Some(BLOCK_STONE);
    }

    // Rules 1 & 2: Water touches lava:
    if let Some(fluid) = current
        && fluid.type_ == FluidType::Lava
    {
        let has_water_touch = pos.horizontal_neighbors().iter().any(|&n| {
            world
                .get_fluid(n)
                .is_some_and(|f| f.type_ == FluidType::Water)
        }) || world
            .get_fluid(pos.up())
            .is_some_and(|f| f.type_ == FluidType::Water);

        if has_water_touch {
            if fluid.is_source() {
                // Water touches lava source -> obsidian
                return Some(BLOCK_OBSIDIAN);
            } else {
                // Water touches flowing lava -> cobblestone
                return Some(BLOCK_COBBLESTONE);
            }
        }
    }

    // Rule 4: Horizontal collision on an empty cell where both water and lava arrive simultaneously:
    if current.is_none() && !world.is_solid(pos) {
        let mut has_water_arrival = false;
        let mut has_lava_arrival = false;

        for &neighbor in &pos.horizontal_neighbors() {
            if let Some(fluid) = world.get_fluid(neighbor)
                && horizontal_arrival(world, neighbor, fluid, pos).is_some()
            {
                match fluid.type_ {
                    FluidType::Water => has_water_arrival = true,
                    FluidType::Lava => has_lava_arrival = true,
                }
            }
        }

        if has_water_arrival && has_lava_arrival {
            return Some(BLOCK_COBBLESTONE);
        }
    }

    None
}

/// Computes the resulting fluid state at `pos` based on surrounding world state.
///
/// Follows standard Minecraft fluid dynamics:
/// 1. Returns `None` if the cell is solid or converted by collision.
/// 2. If cell has $\ge 2$ horizontally adjacent water sources AND a solid block or water
///    source below, creates an infinite water source (`level = 0`).
/// 3. Downward flow from `pos.up()` takes precedence over horizontal flow (`falling = true`).
/// 4. Horizontal flow from neighbors:
///    - Neighbors that can descend fall down and do not spread horizontally.
///    - Neighbors perform inward slope search to prefer cliff/drop directions.
///    - Fluid decays by +1 level per block, up to max reach (7 for water, 3 for lava).
///    - Falling neighbors spread at full strength, restarting reach at the foot of a fall.
///    - Lowest level (strongest fluid) among arriving neighbors wins.
///    - A cell already holding fluid is fed only by that fluid; solidifying the other
///      fluid is that cell's own collision.
pub fn compute_fluid_spread(world: &dyn FluidWorldView, pos: BlockPos) -> Option<FluidState> {
    if world.is_solid(pos) {
        return None;
    }

    // Check collision: if collision solidifies this block, no fluid remains.
    if check_fluid_collision(world, pos).is_some() {
        return None;
    }

    // Infinite water source creation:
    // If a cell has `level > 0` (or air) and has >= 2 horizontally adjacent water source blocks (`level == 0`)
    // AND has a solid block or water source below, it converts into a water source (`level = 0`).
    let supported = world.is_solid(pos.down())
        || world
            .get_fluid(pos.down())
            .is_some_and(|f| f.type_ == FluidType::Water && f.is_source());
    if supported {
        let adjacent_water_sources = pos
            .horizontal_neighbors()
            .iter()
            .filter(|&&n| {
                world
                    .get_fluid(n)
                    .is_some_and(|f| f.type_ == FluidType::Water && f.is_source())
            })
            .count();

        if adjacent_water_sources >= 2 {
            return Some(FluidState::source(FluidType::Water));
        }
    }

    // Existing source block remains source if not consumed by collision.
    if let Some(current) = world.get_fluid(pos)
        && current.is_source()
    {
        return Some(current);
    }

    // Downward flow from above takes precedence over horizontal flow.
    if let Some(above) = world.get_fluid(pos.up()) {
        return Some(FluidState::new(above.type_, 0, true));
    }

    // Horizontal flow from neighbors. Two different fluids reaching an empty
    // cell were already resolved above as a collision.
    let current_type = world.get_fluid(pos).map(|fluid| fluid.type_);
    let mut best_state: Option<FluidState> = None;

    for &neighbor in &pos.horizontal_neighbors() {
        let Some(neighbor_fluid) = world.get_fluid(neighbor) else {
            continue;
        };
        if current_type.is_some_and(|fluid_type| fluid_type != neighbor_fluid.type_) {
            continue;
        }
        let Some(candidate) = horizontal_arrival(world, neighbor, neighbor_fluid, pos) else {
            continue;
        };
        if best_state.is_none_or(|best| candidate.level < best.level) {
            best_state = Some(candidate);
        }
    }

    best_state
}

/// Simulation facade for fluid mechanics.
pub struct FluidSimulator;

impl FluidSimulator {
    /// Computes the fluid spread at `pos`.
    pub fn compute_spread(world: &dyn FluidWorldView, pos: BlockPos) -> Option<FluidState> {
        compute_fluid_spread(world, pos)
    }

    /// Checks if a fluid collision occurs at `pos`.
    pub fn check_collision(world: &dyn FluidWorldView, pos: BlockPos) -> Option<&'static str> {
        check_fluid_collision(world, pos)
    }

    /// Evaluates a cell's transition during a simulation step.
    pub fn step_cell(world: &dyn FluidWorldView, pos: BlockPos) -> FlowOutcome {
        if world.is_solid(pos) {
            return FlowOutcome::Empty;
        }

        if let Some(solid_block) = Self::check_collision(world, pos) {
            return FlowOutcome::Solidify(solid_block);
        }

        match compute_fluid_spread(world, pos) {
            Some(fluid) => FlowOutcome::Fluid(fluid),
            None => FlowOutcome::Empty,
        }
    }
}

use std::collections::HashMap;

/// An in-memory, grid-based world view for fluid simulation and testing.
#[derive(Debug, Default, Clone)]
pub struct InMemoryFluidWorld {
    fluids: HashMap<BlockPos, FluidState>,
    solids: HashMap<BlockPos, bool>,
    blocks: HashMap<BlockPos, String>,
}

impl InMemoryFluidWorld {
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs `fluid`, replacing whatever block occupied `pos`.
    pub fn set_fluid(&mut self, pos: BlockPos, fluid: FluidState) {
        self.blocks.remove(&pos);
        self.solids.insert(pos, false);
        self.fluids.insert(pos, fluid);
    }

    pub fn remove_fluid(&mut self, pos: BlockPos) {
        self.fluids.remove(&pos);
    }

    /// Marks `pos` solid or open, clearing any stored block identifier. A
    /// solid cell cannot also hold fluid.
    pub fn set_solid(&mut self, pos: BlockPos, solid: bool) {
        self.blocks.remove(&pos);
        if solid {
            self.fluids.remove(&pos);
        }
        self.solids.insert(pos, solid);
    }

    /// Replaces the block at `pos`. Fluid block states install that fluid, so
    /// the cell's block, solidity, and fluid always describe the same state.
    pub fn set_block(&mut self, pos: BlockPos, block_id: impl Into<String>) {
        let id = block_id.into();
        if let Some(fluid) = FluidState::from_block_state(&id) {
            self.set_fluid(pos, fluid);
            return;
        }
        let solid = id != BLOCK_AIR;
        self.set_solid(pos, solid);
        self.fluids.remove(&pos);
        self.blocks.insert(pos, id);
    }

    /// Applies a simulation step to the given positions.
    pub fn step(&mut self, positions: &[BlockPos]) {
        let mut updates = Vec::new();
        for &pos in positions {
            let outcome = FluidSimulator::step_cell(self, pos);
            updates.push((pos, outcome));
        }
        for (pos, outcome) in updates {
            match outcome {
                FlowOutcome::Fluid(fluid) => {
                    self.set_fluid(pos, fluid);
                }
                FlowOutcome::Solidify(block_id) => {
                    self.set_block(pos, block_id);
                }
                FlowOutcome::Empty => {
                    self.remove_fluid(pos);
                }
            }
        }
    }
}

impl FluidWorldView for InMemoryFluidWorld {
    fn get_fluid(&self, pos: BlockPos) -> Option<FluidState> {
        self.fluids.get(&pos).copied()
    }

    fn is_solid(&self, pos: BlockPos) -> bool {
        self.solids.get(&pos).copied().unwrap_or(false)
    }

    fn get_block_id(&self, pos: BlockPos) -> String {
        if let Some(block) = self.blocks.get(&pos) {
            block.clone()
        } else if let Some(fluid) = self.get_fluid(pos) {
            fluid.to_block_state()
        } else if self.is_solid(pos) {
            BLOCK_STONE.to_string()
        } else {
            BLOCK_AIR.to_string()
        }
    }
}
