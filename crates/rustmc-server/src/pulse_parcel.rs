//! M6 Pulse–Parcel Deterministic Replay & Partition Equivalence Evaluation.
//!
//! Provides the reference oracle (`ReferenceExecutor`) and parallel partitioned
//! execution model (`ParcelExecutor`) evaluated under spatial partitioning,
//! dynamic repartitioning, cross-parcel boundary coordination, and player
//! migration transfer points.

use crate::world::{Block, CHUNK_SIDE, Chunk, WORLD_HEIGHT};
use std::collections::{BTreeMap, BTreeSet};

/// Player envelope width in blocks (documented whole-cube approximation).
pub const PLAYER_WIDTH: f64 = 0.6;
/// Player standing envelope height in blocks.
pub const PLAYER_HEIGHT: f64 = 1.8;
/// Player eye level offset in blocks.
pub const PLAYER_EYE_HEIGHT: f64 = 1.62;
/// Interaction reach distance limit in blocks.
pub const INTERACTION_REACH: f64 = 6.0;

/// Numbered logical tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PulseId(pub u64);

impl PulseId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Addressable world block position `(x, y, z)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellCoord {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

impl CellCoord {
    pub const fn new(x: i64, y: i64, z: i64) -> Self {
        Self { x, y, z }
    }

    /// Owning chunk coordinate `(chunk_x, chunk_z)`.
    pub fn chunk_coord(&self) -> (i32, i32) {
        let side = CHUNK_SIDE as i64;
        let cx = self.x.div_euclid(side);
        let cz = self.z.div_euclid(side);
        (
            cx.try_into()
                .unwrap_or(if cx < 0 { i32::MIN } else { i32::MAX }),
            cz.try_into()
                .unwrap_or(if cz < 0 { i32::MIN } else { i32::MAX }),
        )
    }

    /// In-chunk local cell indices `(local_x, y, local_z)`.
    pub fn chunk_local(&self) -> (usize, usize, usize) {
        let side = CHUNK_SIDE as i64;
        (
            self.x.rem_euclid(side) as usize,
            self.y as usize,
            self.z.rem_euclid(side) as usize,
        )
    }
}

/// Identifier for a spatial parcel partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParcelId(pub u32);

impl ParcelId {
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

/// Spatial mapping assigning chunks to parcels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionLayout {
    chunk_to_parcel: BTreeMap<(i32, i32), ParcelId>,
    default_parcel: Option<ParcelId>,
}

impl PartitionLayout {
    pub fn new() -> Self {
        Self {
            chunk_to_parcel: BTreeMap::new(),
            default_parcel: None,
        }
    }

    pub fn with_default(default_parcel: ParcelId) -> Self {
        Self {
            chunk_to_parcel: BTreeMap::new(),
            default_parcel: Some(default_parcel),
        }
    }

    pub fn assign_chunk(&mut self, chunk: (i32, i32), parcel: ParcelId) {
        self.chunk_to_parcel.insert(chunk, parcel);
    }

    pub fn parcel_for_chunk(&self, chunk: (i32, i32)) -> Option<ParcelId> {
        self.chunk_to_parcel
            .get(&chunk)
            .copied()
            .or(self.default_parcel)
    }

    pub fn parcel_for_cell(&self, cell: CellCoord) -> Option<ParcelId> {
        self.parcel_for_chunk(cell.chunk_coord())
    }

    pub fn chunk_assignments(&self) -> &BTreeMap<(i32, i32), ParcelId> {
        &self.chunk_to_parcel
    }

    pub fn parcels(&self) -> BTreeSet<ParcelId> {
        let mut set = BTreeSet::new();
        if let Some(def) = self.default_parcel {
            set.insert(def);
        }
        for &p in self.chunk_to_parcel.values() {
            set.insert(p);
        }
        set
    }
}

impl Default for PartitionLayout {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifier for an admitted player session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlayerId(pub u64);

impl PlayerId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Authoritative state of a player. Bit-exact equality via float bits.
#[derive(Debug, Clone)]
pub struct PlayerState {
    pub player_id: PlayerId,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub epoch: u64,
    pub last_sequence: u64,
    pub parcel_id: ParcelId,
}

impl PlayerState {
    pub fn new(player_id: PlayerId, x: f64, y: f64, z: f64, parcel_id: ParcelId) -> Self {
        Self {
            player_id,
            x,
            y,
            z,
            yaw: 0.0,
            pitch: 0.0,
            epoch: 0,
            last_sequence: 0,
            parcel_id,
        }
    }
}

impl PartialEq for PlayerState {
    fn eq(&self, other: &Self) -> bool {
        self.player_id == other.player_id
            && self.x.to_bits() == other.x.to_bits()
            && self.y.to_bits() == other.y.to_bits()
            && self.z.to_bits() == other.z.to_bits()
            && self.yaw.to_bits() == other.yaw.to_bits()
            && self.pitch.to_bits() == other.pitch.to_bits()
            && self.epoch == other.epoch
            && self.last_sequence == other.last_sequence
            && self.parcel_id == other.parcel_id
    }
}

impl Eq for PlayerState {}

impl Ord for PlayerState {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.player_id
            .cmp(&other.player_id)
            .then_with(|| self.epoch.cmp(&other.epoch))
            .then_with(|| self.last_sequence.cmp(&other.last_sequence))
            .then_with(|| self.parcel_id.cmp(&other.parcel_id))
            .then_with(|| self.x.to_bits().cmp(&other.x.to_bits()))
            .then_with(|| self.y.to_bits().cmp(&other.y.to_bits()))
            .then_with(|| self.z.to_bits().cmp(&other.z.to_bits()))
            .then_with(|| self.yaw.to_bits().cmp(&other.yaw.to_bits()))
            .then_with(|| self.pitch.to_bits().cmp(&other.pitch.to_bits()))
    }
}

impl PartialOrd for PlayerState {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Decoded gameplay action intent kind.
#[derive(Debug, Clone, PartialEq)]
pub enum IntentKind {
    Move {
        to_x: f64,
        to_y: f64,
        to_z: f64,
    },
    BreakBlock {
        target: CellCoord,
        claimed_block: Block,
    },
    PlaceBlock {
        target: CellCoord,
        block: Block,
        claimed_block: Block,
    },
}

impl Eq for IntentKind {}

impl Ord for IntentKind {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (
                IntentKind::Move {
                    to_x: x1,
                    to_y: y1,
                    to_z: z1,
                },
                IntentKind::Move {
                    to_x: x2,
                    to_y: y2,
                    to_z: z2,
                },
            ) => x1
                .to_bits()
                .cmp(&x2.to_bits())
                .then_with(|| y1.to_bits().cmp(&y2.to_bits()))
                .then_with(|| z1.to_bits().cmp(&z2.to_bits())),
            (IntentKind::Move { .. }, _) => std::cmp::Ordering::Less,
            (_, IntentKind::Move { .. }) => std::cmp::Ordering::Greater,
            (
                IntentKind::BreakBlock {
                    target: t1,
                    claimed_block: b1,
                },
                IntentKind::BreakBlock {
                    target: t2,
                    claimed_block: b2,
                },
            ) => t1.cmp(t2).then_with(|| b1.cmp(b2)),
            (IntentKind::BreakBlock { .. }, _) => std::cmp::Ordering::Less,
            (_, IntentKind::BreakBlock { .. }) => std::cmp::Ordering::Greater,
            (
                IntentKind::PlaceBlock {
                    target: t1,
                    block: bl1,
                    claimed_block: cl1,
                },
                IntentKind::PlaceBlock {
                    target: t2,
                    block: bl2,
                    claimed_block: cl2,
                },
            ) => t1
                .cmp(t2)
                .then_with(|| bl1.cmp(bl2))
                .then_with(|| cl1.cmp(cl2)),
        }
    }
}

impl PartialOrd for IntentKind {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Admitted player intent with canonical global admission order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AdmittedIntent {
    pub admission_order: u64,
    pub player_id: PlayerId,
    pub sequence: u64,
    pub intent: IntentKind,
}

impl AdmittedIntent {
    pub fn new(
        admission_order: u64,
        player_id: PlayerId,
        sequence: u64,
        intent: IntentKind,
    ) -> Self {
        Self {
            admission_order,
            player_id,
            sequence,
            intent,
        }
    }
}

/// Input batch admitted for one pulse tick.
#[derive(Debug, Clone, PartialEq)]
pub struct PulseInput {
    pub pulse_id: PulseId,
    pub intents: Vec<AdmittedIntent>,
    pub layout: Option<PartitionLayout>,
}

impl PulseInput {
    pub fn new(pulse_id: PulseId) -> Self {
        Self {
            pulse_id,
            intents: Vec::new(),
            layout: None,
        }
    }

    pub fn with_intents(mut self, intents: Vec<AdmittedIntent>) -> Self {
        self.intents = intents;
        self
    }

    pub fn with_layout(mut self, layout: PartitionLayout) -> Self {
        self.layout = Some(layout);
        self
    }

    pub fn push_intent(&mut self, intent: AdmittedIntent) {
        self.intents.push(intent);
    }
}

/// Explicit reason for rejecting an intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RejectionReason {
    DuplicateSequence,
    PlayerNotFound,
    OutOfBounds,
    UnloadedChunk,
    OutOfReach,
    BodyBlocked,
    UnsupportedFoot,
    StaleClaim,
    WouldEmbed,
}

/// Committed immutable effect resulting from an admitted intent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommittedEffect {
    PlayerMoved {
        player_id: PlayerId,
        from_x_bits: u64,
        from_y_bits: u64,
        from_z_bits: u64,
        to_x_bits: u64,
        to_y_bits: u64,
        to_z_bits: u64,
    },
    BlockChange {
        cell: CellCoord,
        previous: Block,
        current: Block,
    },
}

impl CommittedEffect {
    pub fn player_moved(player_id: PlayerId, from: (f64, f64, f64), to: (f64, f64, f64)) -> Self {
        Self::PlayerMoved {
            player_id,
            from_x_bits: from.0.to_bits(),
            from_y_bits: from.1.to_bits(),
            from_z_bits: from.2.to_bits(),
            to_x_bits: to.0.to_bits(),
            to_y_bits: to.1.to_bits(),
            to_z_bits: to.2.to_bits(),
        }
    }

    pub fn block_change(cell: CellCoord, previous: Block, current: Block) -> Self {
        Self::BlockChange {
            cell,
            previous,
            current,
        }
    }
}

/// Authoritative event emitted into the pulse event stream.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PulseEvent {
    PartitionApplied {
        pulse_id: PulseId,
    },
    IntentCommitted {
        pulse_id: PulseId,
        player_id: PlayerId,
        sequence: u64,
        admission_order: u64,
        effect: CommittedEffect,
    },
    IntentRejected {
        pulse_id: PulseId,
        player_id: PlayerId,
        sequence: u64,
        admission_order: u64,
        reason: RejectionReason,
    },
    PlayerMigrated {
        pulse_id: PulseId,
        player_id: PlayerId,
        from_parcel: ParcelId,
        to_parcel: ParcelId,
        epoch: u64,
    },
    PulseCompleted {
        pulse_id: PulseId,
    },
}

// ---------------------------------------------------------------------------
// Spatial & Geometric Helpers
// ---------------------------------------------------------------------------

/// Integer block cells whose interior the half-open interval `[lo, hi)` crosses.
pub fn crossed_cells(lo: f64, hi: f64) -> Vec<i64> {
    if !lo.is_finite() || !hi.is_finite() {
        return Vec::new();
    }
    let first = lo.floor();
    let last = if (hi - hi.floor()).abs() < 1e-9 {
        hi.floor() - 1.0
    } else {
        hi.floor()
    };
    if first > last {
        return Vec::new();
    }
    (first as i64..=last as i64).collect()
}

/// Set of all world block cells intersecting a player envelope at `(x, y, z)`.
pub fn player_envelope_cells(x: f64, y: f64, z: f64) -> BTreeSet<CellCoord> {
    let half_w = PLAYER_WIDTH / 2.0;
    let xs = crossed_cells(x - half_w, x + half_w);
    let zs = crossed_cells(z - half_w, z + half_w);
    let ys = crossed_cells(y, y + PLAYER_HEIGHT);
    let mut cells = BTreeSet::new();
    for cy in ys {
        for cz in &zs {
            for cx in &xs {
                cells.insert(CellCoord::new(*cx, cy, *cz));
            }
        }
    }
    cells
}

/// Check if player envelope at destination `(to_x, to_y, to_z)` is free of solid blocks.
fn validate_body_clear<F>(
    get_block: &F,
    to_x: f64,
    to_y: f64,
    to_z: f64,
) -> Result<(), RejectionReason>
where
    F: Fn(CellCoord) -> Option<Block>,
{
    let half_w = PLAYER_WIDTH / 2.0;
    let xs = crossed_cells(to_x - half_w, to_x + half_w);
    let zs = crossed_cells(to_z - half_w, to_z + half_w);
    let ys = crossed_cells(to_y, to_y + PLAYER_HEIGHT);

    for cy in ys {
        if cy < 0 || cy >= WORLD_HEIGHT as i64 {
            return Err(RejectionReason::OutOfBounds);
        }
        for cz in &zs {
            for cx in &xs {
                let cell = CellCoord::new(*cx, cy, *cz);
                match get_block(cell) {
                    None => return Err(RejectionReason::UnloadedChunk),
                    Some(Block::Air) => {}
                    Some(_) => return Err(RejectionReason::BodyBlocked),
                }
            }
        }
    }
    Ok(())
}

/// Check if at least one cell under the footprint rests on a solid block.
fn validate_has_support<F>(
    get_block: &F,
    to_x: f64,
    to_y: f64,
    to_z: f64,
) -> Result<(), RejectionReason>
where
    F: Fn(CellCoord) -> Option<Block>,
{
    let below_y = to_y.floor() as i64 - 1;
    if below_y < 0 {
        return Err(RejectionReason::UnsupportedFoot);
    }
    if below_y >= WORLD_HEIGHT as i64 {
        return Err(RejectionReason::OutOfBounds);
    }
    let half_w = PLAYER_WIDTH / 2.0;
    let xs = crossed_cells(to_x - half_w, to_x + half_w);
    let zs = crossed_cells(to_z - half_w, to_z + half_w);

    let mut found_support = false;
    for cz in &zs {
        for cx in &xs {
            let cell = CellCoord::new(*cx, below_y, *cz);
            match get_block(cell) {
                None => return Err(RejectionReason::UnloadedChunk),
                Some(Block::Air) => {}
                Some(_) => found_support = true,
            }
        }
    }

    if found_support {
        Ok(())
    } else {
        Err(RejectionReason::UnsupportedFoot)
    }
}

/// Check interaction reach distance from player eye to target block center.
fn validate_reach(player: &PlayerState, target: CellCoord) -> Result<(), RejectionReason> {
    let eye_x = player.x;
    let eye_y = player.y + PLAYER_EYE_HEIGHT;
    let eye_z = player.z;

    let target_center_x = target.x as f64 + 0.5;
    let target_center_y = target.y as f64 + 0.5;
    let target_center_z = target.z as f64 + 0.5;

    let dx = eye_x - target_center_x;
    let dy = eye_y - target_center_y;
    let dz = eye_z - target_center_z;
    let dist_sq = dx * dx + dy * dy + dz * dz;

    if dist_sq <= INTERACTION_REACH * INTERACTION_REACH {
        Ok(())
    } else {
        Err(RejectionReason::OutOfReach)
    }
}

// ---------------------------------------------------------------------------
// Reference Executor (Single authoritative flat state oracle)
// ---------------------------------------------------------------------------

/// Single authoritative flat state executor operating in canonical admission order.
#[derive(Debug, Clone)]
pub struct ReferenceExecutor {
    chunks: BTreeMap<(i32, i32), Chunk>,
    players: BTreeMap<PlayerId, PlayerState>,
    layout: PartitionLayout,
    event_log: Vec<PulseEvent>,
    current_pulse: PulseId,
}

impl ReferenceExecutor {
    pub fn new(layout: PartitionLayout) -> Self {
        Self {
            chunks: BTreeMap::new(),
            players: BTreeMap::new(),
            layout,
            event_log: Vec::new(),
            current_pulse: PulseId::new(0),
        }
    }

    pub fn insert_chunk(&mut self, chunk: Chunk) {
        self.chunks.insert((chunk.x, chunk.z), chunk);
    }

    pub fn insert_player(&mut self, player: PlayerState) {
        self.players.insert(player.player_id, player);
    }

    pub fn chunks(&self) -> &BTreeMap<(i32, i32), Chunk> {
        &self.chunks
    }

    pub fn players(&self) -> &BTreeMap<PlayerId, PlayerState> {
        &self.players
    }

    pub fn event_log(&self) -> &[PulseEvent] {
        &self.event_log
    }

    pub fn layout(&self) -> &PartitionLayout {
        &self.layout
    }

    pub fn current_pulse(&self) -> PulseId {
        self.current_pulse
    }

    pub fn get_block(&self, cell: CellCoord) -> Option<Block> {
        let chunk_coord = cell.chunk_coord();
        let chunk = self.chunks.get(&chunk_coord)?;
        let (lx, y, lz) = cell.chunk_local();
        chunk.block(lx, y, lz)
    }

    fn set_block(&mut self, cell: CellCoord, block: Block) -> Option<Block> {
        let chunk_coord = cell.chunk_coord();
        let chunk = self.chunks.get_mut(&chunk_coord)?;
        let (lx, y, lz) = cell.chunk_local();
        chunk.set_block_raw(lx, y, lz, block)
    }

    /// Advance one pulse tick sequentially in canonical admission order.
    pub fn step(&mut self, input: PulseInput) {
        self.current_pulse = input.pulse_id;

        // Dynamic repartition if supplied
        if let Some(new_layout) = input.layout {
            self.layout = new_layout;
            // Update player parcel assignments based on current position
            for player in self.players.values_mut() {
                let cell = CellCoord::new(
                    player.x.floor() as i64,
                    player.y.floor() as i64,
                    player.z.floor() as i64,
                );
                if let Some(new_parcel) = self.layout.parcel_for_cell(cell) {
                    player.parcel_id = new_parcel;
                }
            }
            self.event_log.push(PulseEvent::PartitionApplied {
                pulse_id: input.pulse_id,
            });
        }

        // Sort admitted intents canonically by admission_order
        let mut sorted_intents = input.intents;
        sorted_intents.sort();

        for admitted in sorted_intents {
            let player_id = admitted.player_id;
            let sequence = admitted.sequence;
            let admission_order = admitted.admission_order;

            let player = match self.players.get_mut(&player_id) {
                Some(p) => p,
                None => {
                    self.event_log.push(PulseEvent::IntentRejected {
                        pulse_id: input.pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::PlayerNotFound,
                    });
                    continue;
                }
            };

            // Idempotence guard: reject duplicate sequence
            if sequence <= player.last_sequence {
                self.event_log.push(PulseEvent::IntentRejected {
                    pulse_id: input.pulse_id,
                    player_id,
                    sequence,
                    admission_order,
                    reason: RejectionReason::DuplicateSequence,
                });
                continue;
            }
            player.last_sequence = sequence;

            match admitted.intent {
                IntentKind::Move { to_x, to_y, to_z } => {
                    let get_block = |cell: CellCoord| {
                        let chunk = self.chunks.get(&cell.chunk_coord())?;
                        let (lx, y, lz) = cell.chunk_local();
                        chunk.block(lx, y, lz)
                    };

                    if !to_x.is_finite()
                        || !to_y.is_finite()
                        || !to_z.is_finite()
                        || to_x.abs() > 30_000_000.0
                        || to_z.abs() > 30_000_000.0
                        || to_y < 0.0
                        || to_y + PLAYER_HEIGHT >= WORLD_HEIGHT as f64
                    {
                        self.event_log.push(PulseEvent::IntentRejected {
                            pulse_id: input.pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason: RejectionReason::OutOfBounds,
                        });
                        continue;
                    }

                    if let Err(reason) = validate_body_clear(&get_block, to_x, to_y, to_z) {
                        self.event_log.push(PulseEvent::IntentRejected {
                            pulse_id: input.pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason,
                        });
                        continue;
                    }

                    if let Err(reason) = validate_has_support(&get_block, to_x, to_y, to_z) {
                        self.event_log.push(PulseEvent::IntentRejected {
                            pulse_id: input.pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason,
                        });
                        continue;
                    }

                    let player = self.players.get_mut(&player_id).unwrap();
                    let from = (player.x, player.y, player.z);
                    player.x = to_x;
                    player.y = to_y;
                    player.z = to_z;

                    // Check parcel migration
                    let target_cell = CellCoord::new(
                        to_x.floor() as i64,
                        to_y.floor() as i64,
                        to_z.floor() as i64,
                    );
                    if let Some(target_parcel) = self.layout.parcel_for_cell(target_cell)
                        && target_parcel != player.parcel_id
                    {
                        let old_parcel = player.parcel_id;
                        player.parcel_id = target_parcel;
                        player.epoch += 1;
                        self.event_log.push(PulseEvent::PlayerMigrated {
                            pulse_id: input.pulse_id,
                            player_id,
                            from_parcel: old_parcel,
                            to_parcel: target_parcel,
                            epoch: player.epoch,
                        });
                    }

                    self.event_log.push(PulseEvent::IntentCommitted {
                        pulse_id: input.pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        effect: CommittedEffect::player_moved(player_id, from, (to_x, to_y, to_z)),
                    });
                }

                IntentKind::BreakBlock {
                    target,
                    claimed_block,
                } => {
                    if target.y < 0 || target.y >= WORLD_HEIGHT as i64 {
                        self.event_log.push(PulseEvent::IntentRejected {
                            pulse_id: input.pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason: RejectionReason::OutOfBounds,
                        });
                        continue;
                    }

                    let player = self.players.get(&player_id).unwrap();
                    if let Err(reason) = validate_reach(player, target) {
                        self.event_log.push(PulseEvent::IntentRejected {
                            pulse_id: input.pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason,
                        });
                        continue;
                    }

                    let actual_block = match self.get_block(target) {
                        Some(b) => b,
                        None => {
                            self.event_log.push(PulseEvent::IntentRejected {
                                pulse_id: input.pulse_id,
                                player_id,
                                sequence,
                                admission_order,
                                reason: RejectionReason::UnloadedChunk,
                            });
                            continue;
                        }
                    };

                    if actual_block != claimed_block {
                        self.event_log.push(PulseEvent::IntentRejected {
                            pulse_id: input.pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason: RejectionReason::StaleClaim,
                        });
                        continue;
                    }

                    self.set_block(target, Block::Air);
                    self.event_log.push(PulseEvent::IntentCommitted {
                        pulse_id: input.pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        effect: CommittedEffect::block_change(target, claimed_block, Block::Air),
                    });
                }

                IntentKind::PlaceBlock {
                    target,
                    block,
                    claimed_block,
                } => {
                    if target.y < 0 || target.y >= WORLD_HEIGHT as i64 {
                        self.event_log.push(PulseEvent::IntentRejected {
                            pulse_id: input.pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason: RejectionReason::OutOfBounds,
                        });
                        continue;
                    }

                    let player = self.players.get(&player_id).unwrap();
                    if let Err(reason) = validate_reach(player, target) {
                        self.event_log.push(PulseEvent::IntentRejected {
                            pulse_id: input.pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason,
                        });
                        continue;
                    }

                    let actual_block = match self.get_block(target) {
                        Some(b) => b,
                        None => {
                            self.event_log.push(PulseEvent::IntentRejected {
                                pulse_id: input.pulse_id,
                                player_id,
                                sequence,
                                admission_order,
                                reason: RejectionReason::UnloadedChunk,
                            });
                            continue;
                        }
                    };

                    if actual_block != claimed_block {
                        self.event_log.push(PulseEvent::IntentRejected {
                            pulse_id: input.pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason: RejectionReason::StaleClaim,
                        });
                        continue;
                    }

                    // Check if placement intersects any player envelope
                    let mut would_embed = false;
                    for p in self.players.values() {
                        if player_envelope_cells(p.x, p.y, p.z).contains(&target) {
                            would_embed = true;
                            break;
                        }
                    }

                    if would_embed {
                        self.event_log.push(PulseEvent::IntentRejected {
                            pulse_id: input.pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason: RejectionReason::WouldEmbed,
                        });
                        continue;
                    }

                    self.set_block(target, block);
                    self.event_log.push(PulseEvent::IntentCommitted {
                        pulse_id: input.pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        effect: CommittedEffect::block_change(target, claimed_block, block),
                    });
                }
            }
        }

        self.event_log.push(PulseEvent::PulseCompleted {
            pulse_id: input.pulse_id,
        });
    }
}

// ---------------------------------------------------------------------------
// Parcel & Boundary Coordinator Types
// ---------------------------------------------------------------------------

/// One spatial partition parcel owning a subset of chunks and players.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parcel {
    pub id: ParcelId,
    pub chunks: BTreeMap<(i32, i32), Chunk>,
    pub players: BTreeMap<PlayerId, PlayerState>,
}

impl Parcel {
    pub fn new(id: ParcelId) -> Self {
        Self {
            id,
            chunks: BTreeMap::new(),
            players: BTreeMap::new(),
        }
    }

    pub fn insert_chunk(&mut self, chunk: Chunk) {
        self.chunks.insert((chunk.x, chunk.z), chunk);
    }

    pub fn insert_player(&mut self, player: PlayerState) {
        self.players.insert(player.player_id, player);
    }

    pub fn remove_player(&mut self, player_id: PlayerId) -> Option<PlayerState> {
        self.players.remove(&player_id)
    }

    pub fn get_block(&self, cell: CellCoord) -> Option<Block> {
        let chunk_coord = cell.chunk_coord();
        let chunk = self.chunks.get(&chunk_coord)?;
        let (lx, y, lz) = cell.chunk_local();
        chunk.block(lx, y, lz)
    }

    pub fn set_block(&mut self, cell: CellCoord, block: Block) -> Option<Block> {
        let chunk_coord = cell.chunk_coord();
        let chunk = self.chunks.get_mut(&chunk_coord)?;
        let (lx, y, lz) = cell.chunk_local();
        chunk.set_block_raw(lx, y, lz, block)
    }
}

/// Deterministic coordinator for cross-parcel boundary intents,
/// border envelopes, competing edits, and migration transfers.
#[derive(Debug, Clone, Copy, Default)]
pub struct BoundaryCoordinator;

impl BoundaryCoordinator {
    pub fn new() -> Self {
        Self
    }

    /// Check if an admitted intent involves cross-parcel boundaries or multi-parcel envelopes.
    pub fn is_boundary_intent(
        &self,
        parcels: &BTreeMap<ParcelId, Parcel>,
        layout: &PartitionLayout,
        admitted: &AdmittedIntent,
    ) -> bool {
        // Find player's owning parcel
        let player_parcel_id = match Self::find_player_parcel(parcels, admitted.player_id) {
            Some(pid) => pid,
            None => return true, // Player not found routes to coordinator to reject consistently
        };

        match &admitted.intent {
            IntentKind::Move { to_x, to_y, to_z } => {
                if !to_x.is_finite()
                    || !to_y.is_finite()
                    || !to_z.is_finite()
                    || to_x.abs() > 30_000_000.0
                    || to_z.abs() > 30_000_000.0
                    || *to_y < 0.0
                    || *to_y + PLAYER_HEIGHT >= WORLD_HEIGHT as f64
                {
                    return false;
                }
                let target_cell = CellCoord::new(
                    to_x.floor() as i64,
                    to_y.floor() as i64,
                    to_z.floor() as i64,
                );
                let target_parcel = layout.parcel_for_cell(target_cell);
                if target_parcel != Some(player_parcel_id) {
                    return true; // Migration across parcel boundary
                }

                // Check if destination envelope crosses any chunk assigned to another parcel
                for cell in player_envelope_cells(*to_x, *to_y, *to_z) {
                    if layout.parcel_for_cell(cell) != Some(player_parcel_id) {
                        return true;
                    }
                }
                false
            }
            IntentKind::BreakBlock { target, .. } => {
                layout.parcel_for_cell(*target) != Some(player_parcel_id)
            }
            IntentKind::PlaceBlock { target, .. } => {
                if layout.parcel_for_cell(*target) != Some(player_parcel_id) {
                    return true;
                }
                // Check if target block intersects any player's envelope from another parcel
                for parcel in parcels.values() {
                    if parcel.id != player_parcel_id {
                        for p in parcel.players.values() {
                            if player_envelope_cells(p.x, p.y, p.z).contains(target) {
                                return true;
                            }
                        }
                    }
                }
                false
            }
        }
    }

    fn find_player_parcel(
        parcels: &BTreeMap<ParcelId, Parcel>,
        player_id: PlayerId,
    ) -> Option<ParcelId> {
        for parcel in parcels.values() {
            if parcel.players.contains_key(&player_id) {
                return Some(parcel.id);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Parcel Executor (Spatially partitioned parallel execution model)
// ---------------------------------------------------------------------------

/// Parallel partitioned executor operating across independent parcels
/// with a deterministic cross-parcel boundary coordinator.
#[derive(Debug, Clone)]
pub struct ParcelExecutor {
    parcels: BTreeMap<ParcelId, Parcel>,
    layout: PartitionLayout,
    boundary_coordinator: BoundaryCoordinator,
    event_log: Vec<PulseEvent>,
    current_pulse: PulseId,
}

impl ParcelExecutor {
    pub fn new(layout: PartitionLayout) -> Self {
        let mut parcels = BTreeMap::new();
        for pid in layout.parcels() {
            parcels.insert(pid, Parcel::new(pid));
        }

        Self {
            parcels,
            layout,
            boundary_coordinator: BoundaryCoordinator::new(),
            event_log: Vec::new(),
            current_pulse: PulseId::new(0),
        }
    }

    pub fn insert_chunk(&mut self, chunk: Chunk) {
        let chunk_coord = (chunk.x, chunk.z);
        let pid = self
            .layout
            .parcel_for_chunk(chunk_coord)
            .unwrap_or(ParcelId(0));
        self.parcels
            .entry(pid)
            .or_insert_with(|| Parcel::new(pid))
            .insert_chunk(chunk);
    }

    pub fn insert_player(&mut self, player: PlayerState) {
        let pid = player.parcel_id;
        self.parcels
            .entry(pid)
            .or_insert_with(|| Parcel::new(pid))
            .insert_player(player);
    }

    pub fn parcels(&self) -> &BTreeMap<ParcelId, Parcel> {
        &self.parcels
    }

    pub fn layout(&self) -> &PartitionLayout {
        &self.layout
    }

    pub fn event_log(&self) -> &[PulseEvent] {
        &self.event_log
    }

    pub fn current_pulse(&self) -> PulseId {
        self.current_pulse
    }

    pub fn all_chunks(&self) -> BTreeMap<(i32, i32), Chunk> {
        let mut chunks = BTreeMap::new();
        for parcel in self.parcels.values() {
            for (&coord, chunk) in &parcel.chunks {
                chunks.insert(coord, chunk.clone());
            }
        }
        chunks
    }

    pub fn all_players(&self) -> BTreeMap<PlayerId, PlayerState> {
        let mut players = BTreeMap::new();
        for parcel in self.parcels.values() {
            for (&pid, player) in &parcel.players {
                players.insert(pid, player.clone());
            }
        }
        players
    }

    pub fn get_block(&self, cell: CellCoord) -> Option<Block> {
        let chunk_coord = cell.chunk_coord();
        let pid = self.layout.parcel_for_chunk(chunk_coord)?;
        let parcel = self.parcels.get(&pid)?;
        parcel.get_block(cell)
    }

    pub fn set_block(&mut self, cell: CellCoord, block: Block) -> Option<Block> {
        let chunk_coord = cell.chunk_coord();
        let pid = self.layout.parcel_for_chunk(chunk_coord)?;
        let parcel = self.parcels.get_mut(&pid)?;
        parcel.set_block(cell, block)
    }

    /// Perform dynamic repartitioning according to the new layout.
    fn apply_repartition(&mut self, new_layout: PartitionLayout, pulse_id: PulseId) {
        // Ensure all target parcels exist
        for pid in new_layout.parcels() {
            self.parcels.entry(pid).or_insert_with(|| Parcel::new(pid));
        }

        // 1. Move reassigned chunks
        let mut chunks_to_move = Vec::new();
        for (pid, parcel) in &mut self.parcels {
            let mut remove_coords = Vec::new();
            for &coord in parcel.chunks.keys() {
                if let Some(target_pid) = new_layout.parcel_for_chunk(coord)
                    && target_pid != *pid
                {
                    remove_coords.push(coord);
                }
            }
            for coord in remove_coords {
                let chunk = parcel.chunks.remove(&coord).unwrap();
                let target_pid = new_layout.parcel_for_chunk(coord).unwrap();
                chunks_to_move.push((target_pid, chunk));
            }
        }
        for (target_pid, chunk) in chunks_to_move {
            self.parcels
                .entry(target_pid)
                .or_insert_with(|| Parcel::new(target_pid))
                .insert_chunk(chunk);
        }

        // 2. Move players according to new layout without changing state or lost actions
        let mut players_to_move = Vec::new();
        for (pid, parcel) in &mut self.parcels {
            let mut remove_players = Vec::new();
            for (&pl_id, player) in &parcel.players {
                let cell = CellCoord::new(
                    player.x.floor() as i64,
                    player.y.floor() as i64,
                    player.z.floor() as i64,
                );
                if let Some(target_pid) = new_layout.parcel_for_cell(cell)
                    && target_pid != *pid
                {
                    remove_players.push((pl_id, target_pid));
                }
            }
            for (pl_id, target_pid) in remove_players {
                let mut player = parcel.players.remove(&pl_id).unwrap();
                player.parcel_id = target_pid;
                players_to_move.push((target_pid, player));
            }
        }
        for (target_pid, player) in players_to_move {
            self.parcels
                .entry(target_pid)
                .or_insert_with(|| Parcel::new(target_pid))
                .insert_player(player);
        }

        self.layout = new_layout;
        self.event_log
            .push(PulseEvent::PartitionApplied { pulse_id });
    }

    /// Step the partitioned execution model through one pulse tick.
    pub fn step(&mut self, input: PulseInput) {
        self.current_pulse = input.pulse_id;

        // Repartitioning phase if layout update supplied
        if let Some(new_layout) = input.layout {
            self.apply_repartition(new_layout, input.pulse_id);
        }

        // Sort admitted intents canonically
        let mut sorted_intents = input.intents;
        sorted_intents.sort();

        for admitted in sorted_intents {
            let is_boundary = self.boundary_coordinator.is_boundary_intent(
                &self.parcels,
                &self.layout,
                &admitted,
            );

            if !is_boundary {
                // Independent parcel stepping
                let player_pid =
                    BoundaryCoordinator::find_player_parcel(&self.parcels, admitted.player_id)
                        .unwrap();
                let parcel = self.parcels.get_mut(&player_pid).unwrap();
                Self::step_local_parcel_intent(
                    parcel,
                    &admitted,
                    input.pulse_id,
                    &mut self.event_log,
                );
            } else {
                // Cross-parcel boundary coordination & migration transfer point
                self.coordinate_boundary_intent(&admitted, input.pulse_id);
            }
        }

        self.event_log.push(PulseEvent::PulseCompleted {
            pulse_id: input.pulse_id,
        });
    }

    /// Intra-parcel local execution step.
    fn step_local_parcel_intent(
        parcel: &mut Parcel,
        admitted: &AdmittedIntent,
        pulse_id: PulseId,
        event_log: &mut Vec<PulseEvent>,
    ) {
        let player_id = admitted.player_id;
        let sequence = admitted.sequence;
        let admission_order = admitted.admission_order;

        let player = match parcel.players.get_mut(&player_id) {
            Some(p) => p,
            None => {
                event_log.push(PulseEvent::IntentRejected {
                    pulse_id,
                    player_id,
                    sequence,
                    admission_order,
                    reason: RejectionReason::PlayerNotFound,
                });
                return;
            }
        };

        if sequence <= player.last_sequence {
            event_log.push(PulseEvent::IntentRejected {
                pulse_id,
                player_id,
                sequence,
                admission_order,
                reason: RejectionReason::DuplicateSequence,
            });
            return;
        }
        player.last_sequence = sequence;

        match admitted.intent {
            IntentKind::Move { to_x, to_y, to_z } => {
                let get_block = |cell: CellCoord| parcel.get_block(cell);

                if !to_x.is_finite()
                    || !to_y.is_finite()
                    || !to_z.is_finite()
                    || to_x.abs() > 30_000_000.0
                    || to_z.abs() > 30_000_000.0
                    || to_y < 0.0
                    || to_y + PLAYER_HEIGHT >= WORLD_HEIGHT as f64
                {
                    event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::OutOfBounds,
                    });
                    return;
                }

                if let Err(reason) = validate_body_clear(&get_block, to_x, to_y, to_z) {
                    event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason,
                    });
                    return;
                }

                if let Err(reason) = validate_has_support(&get_block, to_x, to_y, to_z) {
                    event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason,
                    });
                    return;
                }

                let p = parcel.players.get_mut(&player_id).unwrap();
                let from = (p.x, p.y, p.z);
                p.x = to_x;
                p.y = to_y;
                p.z = to_z;

                event_log.push(PulseEvent::IntentCommitted {
                    pulse_id,
                    player_id,
                    sequence,
                    admission_order,
                    effect: CommittedEffect::player_moved(player_id, from, (to_x, to_y, to_z)),
                });
            }

            IntentKind::BreakBlock {
                target,
                claimed_block,
            } => {
                if target.y < 0 || target.y >= WORLD_HEIGHT as i64 {
                    event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::OutOfBounds,
                    });
                    return;
                }

                let p = parcel.players.get(&player_id).unwrap();
                if let Err(reason) = validate_reach(p, target) {
                    event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason,
                    });
                    return;
                }

                let actual_block = match parcel.get_block(target) {
                    Some(b) => b,
                    None => {
                        event_log.push(PulseEvent::IntentRejected {
                            pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason: RejectionReason::UnloadedChunk,
                        });
                        return;
                    }
                };

                if actual_block != claimed_block {
                    event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::StaleClaim,
                    });
                    return;
                }

                parcel.set_block(target, Block::Air);
                event_log.push(PulseEvent::IntentCommitted {
                    pulse_id,
                    player_id,
                    sequence,
                    admission_order,
                    effect: CommittedEffect::block_change(target, claimed_block, Block::Air),
                });
            }

            IntentKind::PlaceBlock {
                target,
                block,
                claimed_block,
            } => {
                if target.y < 0 || target.y >= WORLD_HEIGHT as i64 {
                    event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::OutOfBounds,
                    });
                    return;
                }

                let p = parcel.players.get(&player_id).unwrap();
                if let Err(reason) = validate_reach(p, target) {
                    event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason,
                    });
                    return;
                }

                let actual_block = match parcel.get_block(target) {
                    Some(b) => b,
                    None => {
                        event_log.push(PulseEvent::IntentRejected {
                            pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason: RejectionReason::UnloadedChunk,
                        });
                        return;
                    }
                };

                if actual_block != claimed_block {
                    event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::StaleClaim,
                    });
                    return;
                }

                let mut would_embed = false;
                for p in parcel.players.values() {
                    if player_envelope_cells(p.x, p.y, p.z).contains(&target) {
                        would_embed = true;
                        break;
                    }
                }

                if would_embed {
                    event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::WouldEmbed,
                    });
                    return;
                }

                parcel.set_block(target, block);
                event_log.push(PulseEvent::IntentCommitted {
                    pulse_id,
                    player_id,
                    sequence,
                    admission_order,
                    effect: CommittedEffect::block_change(target, claimed_block, block),
                });
            }
        }
    }

    /// Boundary-coordinated execution step across parcels.
    fn coordinate_boundary_intent(&mut self, admitted: &AdmittedIntent, pulse_id: PulseId) {
        let player_id = admitted.player_id;
        let sequence = admitted.sequence;
        let admission_order = admitted.admission_order;

        let player_pid = match BoundaryCoordinator::find_player_parcel(&self.parcels, player_id) {
            Some(pid) => pid,
            None => {
                self.event_log.push(PulseEvent::IntentRejected {
                    pulse_id,
                    player_id,
                    sequence,
                    admission_order,
                    reason: RejectionReason::PlayerNotFound,
                });
                return;
            }
        };

        // Idempotence guard: reject duplicate sequence
        let player = self
            .parcels
            .get_mut(&player_pid)
            .unwrap()
            .players
            .get_mut(&player_id)
            .unwrap();
        if sequence <= player.last_sequence {
            self.event_log.push(PulseEvent::IntentRejected {
                pulse_id,
                player_id,
                sequence,
                admission_order,
                reason: RejectionReason::DuplicateSequence,
            });
            return;
        }
        player.last_sequence = sequence;

        match admitted.intent {
            IntentKind::Move { to_x, to_y, to_z } => {
                if !to_x.is_finite()
                    || !to_y.is_finite()
                    || !to_z.is_finite()
                    || to_x.abs() > 30_000_000.0
                    || to_z.abs() > 30_000_000.0
                    || to_y < 0.0
                    || to_y + PLAYER_HEIGHT >= WORLD_HEIGHT as f64
                {
                    self.event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::OutOfBounds,
                    });
                    return;
                }

                let get_block = |cell: CellCoord| self.get_block(cell);

                if let Err(reason) = validate_body_clear(&get_block, to_x, to_y, to_z) {
                    self.event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason,
                    });
                    return;
                }

                if let Err(reason) = validate_has_support(&get_block, to_x, to_y, to_z) {
                    self.event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason,
                    });
                    return;
                }

                let player = self
                    .parcels
                    .get_mut(&player_pid)
                    .unwrap()
                    .players
                    .get_mut(&player_id)
                    .unwrap();
                let from = (player.x, player.y, player.z);
                player.x = to_x;
                player.y = to_y;
                player.z = to_z;

                let target_cell = CellCoord::new(
                    to_x.floor() as i64,
                    to_y.floor() as i64,
                    to_z.floor() as i64,
                );
                let target_parcel = self
                    .layout
                    .parcel_for_cell(target_cell)
                    .unwrap_or(player_pid);

                // Committed player migration transfer point
                if target_parcel != player_pid {
                    let mut transferred_player = self
                        .parcels
                        .get_mut(&player_pid)
                        .unwrap()
                        .players
                        .remove(&player_id)
                        .unwrap();
                    transferred_player.parcel_id = target_parcel;
                    transferred_player.epoch += 1;
                    let epoch = transferred_player.epoch;

                    self.parcels
                        .entry(target_parcel)
                        .or_insert_with(|| Parcel::new(target_parcel))
                        .insert_player(transferred_player);

                    self.event_log.push(PulseEvent::PlayerMigrated {
                        pulse_id,
                        player_id,
                        from_parcel: player_pid,
                        to_parcel: target_parcel,
                        epoch,
                    });
                }

                self.event_log.push(PulseEvent::IntentCommitted {
                    pulse_id,
                    player_id,
                    sequence,
                    admission_order,
                    effect: CommittedEffect::player_moved(player_id, from, (to_x, to_y, to_z)),
                });
            }

            IntentKind::BreakBlock {
                target,
                claimed_block,
            } => {
                if target.y < 0 || target.y >= WORLD_HEIGHT as i64 {
                    self.event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::OutOfBounds,
                    });
                    return;
                }

                let current_pid =
                    BoundaryCoordinator::find_player_parcel(&self.parcels, player_id).unwrap();
                let player = self
                    .parcels
                    .get(&current_pid)
                    .unwrap()
                    .players
                    .get(&player_id)
                    .unwrap();
                if let Err(reason) = validate_reach(player, target) {
                    self.event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason,
                    });
                    return;
                }

                let actual_block = match self.get_block(target) {
                    Some(b) => b,
                    None => {
                        self.event_log.push(PulseEvent::IntentRejected {
                            pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason: RejectionReason::UnloadedChunk,
                        });
                        return;
                    }
                };

                if actual_block != claimed_block {
                    self.event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::StaleClaim,
                    });
                    return;
                }

                self.set_block(target, Block::Air);
                self.event_log.push(PulseEvent::IntentCommitted {
                    pulse_id,
                    player_id,
                    sequence,
                    admission_order,
                    effect: CommittedEffect::block_change(target, claimed_block, Block::Air),
                });
            }

            IntentKind::PlaceBlock {
                target,
                block,
                claimed_block,
            } => {
                if target.y < 0 || target.y >= WORLD_HEIGHT as i64 {
                    self.event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::OutOfBounds,
                    });
                    return;
                }

                let current_pid =
                    BoundaryCoordinator::find_player_parcel(&self.parcels, player_id).unwrap();
                let player = self
                    .parcels
                    .get(&current_pid)
                    .unwrap()
                    .players
                    .get(&player_id)
                    .unwrap();
                if let Err(reason) = validate_reach(player, target) {
                    self.event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason,
                    });
                    return;
                }

                let actual_block = match self.get_block(target) {
                    Some(b) => b,
                    None => {
                        self.event_log.push(PulseEvent::IntentRejected {
                            pulse_id,
                            player_id,
                            sequence,
                            admission_order,
                            reason: RejectionReason::UnloadedChunk,
                        });
                        return;
                    }
                };

                if actual_block != claimed_block {
                    self.event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::StaleClaim,
                    });
                    return;
                }

                // Check cross-parcel border envelopes across all parcels
                let mut would_embed = false;
                for parcel in self.parcels.values() {
                    for p in parcel.players.values() {
                        if player_envelope_cells(p.x, p.y, p.z).contains(&target) {
                            would_embed = true;
                            break;
                        }
                    }
                    if would_embed {
                        break;
                    }
                }

                if would_embed {
                    self.event_log.push(PulseEvent::IntentRejected {
                        pulse_id,
                        player_id,
                        sequence,
                        admission_order,
                        reason: RejectionReason::WouldEmbed,
                    });
                    return;
                }

                self.set_block(target, block);
                self.event_log.push(PulseEvent::IntentCommitted {
                    pulse_id,
                    player_id,
                    sequence,
                    admission_order,
                    effect: CommittedEffect::block_change(target, claimed_block, block),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Event Log & Execution Equivalence Comparator
// ---------------------------------------------------------------------------

/// Detailed comparison report verifying bit-for-bit equivalence between executors.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EquivalenceReport {
    pub chunk_arrays_match: bool,
    pub player_states_match: bool,
    pub event_streams_match: bool,
    pub differences: Vec<String>,
}

impl EquivalenceReport {
    pub fn is_equivalent(&self) -> bool {
        self.chunk_arrays_match && self.player_states_match && self.event_streams_match
    }
}

/// Compare a ReferenceExecutor against a ParcelExecutor for bit-for-bit equivalence.
pub fn compare_executors(
    reference: &ReferenceExecutor,
    parcel: &ParcelExecutor,
) -> EquivalenceReport {
    let mut report = EquivalenceReport::default();

    // 1. Event streams comparison
    let ref_events = reference.event_log();
    let parcel_events = parcel.event_log();

    if ref_events.len() != parcel_events.len() {
        report.differences.push(format!(
            "Event log length mismatch: ref has {} events, parcel has {}",
            ref_events.len(),
            parcel_events.len()
        ));
    } else {
        let mut events_match = true;
        for (i, (re, pe)) in ref_events.iter().zip(parcel_events.iter()).enumerate() {
            if re != pe {
                events_match = false;
                report.differences.push(format!(
                    "Event divergence at index {}: ref={:?}, parcel={:?}",
                    i, re, pe
                ));
            }
        }
        report.event_streams_match = events_match;
    }

    // 2. Player states comparison
    let ref_players = reference.players();
    let parcel_players = parcel.all_players();

    if ref_players.len() != parcel_players.len() {
        report.differences.push(format!(
            "Player count mismatch: ref has {}, parcel has {}",
            ref_players.len(),
            parcel_players.len()
        ));
    } else {
        let mut players_match = true;
        for (pid, rp) in ref_players {
            match parcel_players.get(pid) {
                None => {
                    players_match = false;
                    report
                        .differences
                        .push(format!("Player {:?} missing from parcel executor", pid));
                }
                Some(pp) => {
                    if rp != pp {
                        players_match = false;
                        report.differences.push(format!(
                            "Player {:?} state mismatch: ref={:?}, parcel={:?}",
                            pid, rp, pp
                        ));
                    }
                }
            }
        }
        report.player_states_match = players_match;
    }

    // 3. Chunk block arrays bit-for-bit comparison
    let ref_chunks = reference.chunks();
    let parcel_chunks = parcel.all_chunks();

    if ref_chunks.len() != parcel_chunks.len() {
        report.differences.push(format!(
            "Chunk count mismatch: ref has {}, parcel has {}",
            ref_chunks.len(),
            parcel_chunks.len()
        ));
    } else {
        let mut chunks_match = true;
        for (coord, rc) in ref_chunks {
            match parcel_chunks.get(coord) {
                None => {
                    chunks_match = false;
                    report
                        .differences
                        .push(format!("Chunk {:?} missing from parcel executor", coord));
                }
                Some(pc) => {
                    if rc.blocks_raw() != pc.blocks_raw() {
                        chunks_match = false;
                        report
                            .differences
                            .push(format!("Chunk {:?} block array bit divergence", coord));
                    }
                }
            }
        }
        report.chunk_arrays_match = chunks_match;
    }

    report
}

/// Assert that the reference and parcel executors produced bit-for-bit identical outcomes.
pub fn assert_execution_equivalent(reference: &ReferenceExecutor, parcel: &ParcelExecutor) {
    let report = compare_executors(reference, parcel);
    assert!(
        report.is_equivalent(),
        "Execution equivalence failed:\n{}",
        report.differences.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a chunk with solid ground up to y=63 and air above.
    fn create_test_chunk(chunk_x: i32, chunk_z: i32) -> Chunk {
        let mut chunk = Chunk::new_empty(chunk_x, chunk_z);
        for z in 0..CHUNK_SIDE {
            for x in 0..CHUNK_SIDE {
                chunk.set_block_raw(x, 0, z, Block::Bedrock);
                for y in 1..63 {
                    chunk.set_block_raw(x, y, z, Block::Dirt);
                }
                chunk.set_block_raw(x, 63, z, Block::Grass);
            }
        }
        chunk
    }

    #[test]
    fn single_parcel_and_multi_parcel_layouts_match_reference_bit_for_bit() {
        // 4 chunks in a 2x2 grid
        let coords = [(0, 0), (1, 0), (0, 1), (1, 1)];

        // Single parcel layout (all chunks in Parcel 0)
        let mut single_layout = PartitionLayout::with_default(ParcelId(0));
        for c in coords {
            single_layout.assign_chunk(c, ParcelId(0));
        }

        // Multi-parcel layout (each chunk in its own parcel)
        let mut multi_layout = PartitionLayout::new();
        multi_layout.assign_chunk((0, 0), ParcelId(0));
        multi_layout.assign_chunk((1, 0), ParcelId(1));
        multi_layout.assign_chunk((0, 1), ParcelId(2));
        multi_layout.assign_chunk((1, 1), ParcelId(3));

        let mut ref_single = ReferenceExecutor::new(single_layout.clone());
        let mut single_exec = ParcelExecutor::new(single_layout);

        let mut ref_multi = ReferenceExecutor::new(multi_layout.clone());
        let mut multi_exec = ParcelExecutor::new(multi_layout);

        for c in coords {
            ref_single.insert_chunk(create_test_chunk(c.0, c.1));
            single_exec.insert_chunk(create_test_chunk(c.0, c.1));
            ref_multi.insert_chunk(create_test_chunk(c.0, c.1));
            multi_exec.insert_chunk(create_test_chunk(c.0, c.1));
        }

        let p1_single = PlayerState::new(PlayerId(1), 4.0, 64.0, 4.0, ParcelId(0));
        let p2_single = PlayerState::new(PlayerId(2), 20.0, 64.0, 20.0, ParcelId(0));
        ref_single.insert_player(p1_single.clone());
        ref_single.insert_player(p2_single.clone());
        single_exec.insert_player(p1_single);
        single_exec.insert_player(p2_single);

        let p1_multi = PlayerState::new(PlayerId(1), 4.0, 64.0, 4.0, ParcelId(0));
        let p2_multi = PlayerState::new(PlayerId(2), 20.0, 64.0, 20.0, ParcelId(3));
        ref_multi.insert_player(p1_multi.clone());
        ref_multi.insert_player(p2_multi.clone());
        multi_exec.insert_player(p1_multi);
        multi_exec.insert_player(p2_multi);

        // Pulse 1: Player 1 moves, Player 2 breaks grass
        let pulse1 = PulseInput::new(PulseId(1)).with_intents(vec![
            AdmittedIntent::new(
                1,
                PlayerId(1),
                1,
                IntentKind::Move {
                    to_x: 5.0,
                    to_y: 64.0,
                    to_z: 5.0,
                },
            ),
            AdmittedIntent::new(
                2,
                PlayerId(2),
                1,
                IntentKind::BreakBlock {
                    target: CellCoord::new(20, 63, 21),
                    claimed_block: Block::Grass,
                },
            ),
        ]);

        ref_single.step(pulse1.clone());
        single_exec.step(pulse1.clone());
        ref_multi.step(pulse1.clone());
        multi_exec.step(pulse1);

        // Pulse 2: Player 1 places stone, Player 2 moves
        let pulse2 = PulseInput::new(PulseId(2)).with_intents(vec![
            AdmittedIntent::new(
                10,
                PlayerId(1),
                2,
                IntentKind::PlaceBlock {
                    target: CellCoord::new(5, 64, 6),
                    block: Block::Stone,
                    claimed_block: Block::Air,
                },
            ),
            AdmittedIntent::new(
                20,
                PlayerId(2),
                2,
                IntentKind::Move {
                    to_x: 21.0,
                    to_y: 64.0,
                    to_z: 20.0,
                },
            ),
        ]);

        ref_single.step(pulse2.clone());
        single_exec.step(pulse2.clone());
        ref_multi.step(pulse2.clone());
        multi_exec.step(pulse2);

        // Pulse 3: Player 1 breaks stone, Player 2 places dirt back
        let pulse3 = PulseInput::new(PulseId(3)).with_intents(vec![
            AdmittedIntent::new(
                30,
                PlayerId(1),
                3,
                IntentKind::BreakBlock {
                    target: CellCoord::new(5, 64, 6),
                    claimed_block: Block::Stone,
                },
            ),
            AdmittedIntent::new(
                40,
                PlayerId(2),
                3,
                IntentKind::PlaceBlock {
                    target: CellCoord::new(20, 63, 21),
                    block: Block::Dirt,
                    claimed_block: Block::Air,
                },
            ),
        ]);

        ref_single.step(pulse3.clone());
        single_exec.step(pulse3.clone());
        ref_multi.step(pulse3.clone());
        multi_exec.step(pulse3);

        // Both layouts match their reference executor bit-for-bit
        assert_execution_equivalent(&ref_single, &single_exec);
        assert_execution_equivalent(&ref_multi, &multi_exec);

        // Chunk block arrays across single-parcel and multi-parcel match bit-for-bit
        for c in coords {
            let mut chunks_s = single_exec.all_chunks();
            let mut chunks_m = multi_exec.all_chunks();
            let c_s = chunks_s.remove(&c).unwrap();
            let c_m = chunks_m.remove(&c).unwrap();
            assert_eq!(c_s.blocks_raw(), c_m.blocks_raw());
        }
    }

    #[test]
    fn player_migration_across_chunk_and_parcel_boundary_preserves_id_epoch_and_same_pulse_actions()
    {
        let mut layout = PartitionLayout::new();
        layout.assign_chunk((0, 0), ParcelId(0));
        layout.assign_chunk((1, 0), ParcelId(1));

        let mut ref_exec = ReferenceExecutor::new(layout.clone());
        let mut parcel_exec = ParcelExecutor::new(layout);

        ref_exec.insert_chunk(create_test_chunk(0, 0));
        ref_exec.insert_chunk(create_test_chunk(1, 0));
        parcel_exec.insert_chunk(create_test_chunk(0, 0));
        parcel_exec.insert_chunk(create_test_chunk(1, 0));

        // Player starts near chunk border in Chunk (0, 0), Parcel 0
        let player = PlayerState::new(PlayerId(42), 15.0, 64.0, 8.0, ParcelId(0));
        ref_exec.insert_player(player.clone());
        parcel_exec.insert_player(player);

        // In same pulse: Move into Chunk (1, 0) Parcel 1, then place a block in Chunk (1, 0)
        let pulse = PulseInput::new(PulseId(1)).with_intents(vec![
            AdmittedIntent::new(
                1,
                PlayerId(42),
                1,
                IntentKind::Move {
                    to_x: 17.0,
                    to_y: 64.0,
                    to_z: 8.0,
                },
            ),
            AdmittedIntent::new(
                2,
                PlayerId(42),
                2,
                IntentKind::PlaceBlock {
                    target: CellCoord::new(18, 64, 8),
                    block: Block::Stone,
                    claimed_block: Block::Air,
                },
            ),
        ]);

        ref_exec.step(pulse.clone());
        parcel_exec.step(pulse);

        // Bit-for-bit equivalence
        assert_execution_equivalent(&ref_exec, &parcel_exec);

        // Verify player migrated with preserved ID and incremented epoch
        let p_ref = ref_exec.players().get(&PlayerId(42)).unwrap();
        let all_players = parcel_exec.all_players();
        let p_par = all_players.get(&PlayerId(42)).unwrap();
        assert_eq!(p_ref.player_id, PlayerId(42));
        assert_eq!(p_ref.epoch, 1);
        assert_eq!(p_ref.parcel_id, ParcelId(1));
        assert_eq!(p_ref, p_par);

        // Verify placed block committed
        assert_eq!(
            ref_exec.get_block(CellCoord::new(18, 64, 8)),
            Some(Block::Stone)
        );
        assert_eq!(
            parcel_exec.get_block(CellCoord::new(18, 64, 8)),
            Some(Block::Stone)
        );
    }

    #[test]
    fn cross_parcel_border_envelope_and_would_embed_conflict_resolve_in_canonical_admission_order()
    {
        let mut layout = PartitionLayout::new();
        layout.assign_chunk((0, 0), ParcelId(0));
        layout.assign_chunk((1, 0), ParcelId(1));

        let mut ref_exec = ReferenceExecutor::new(layout.clone());
        let mut parcel_exec = ParcelExecutor::new(layout);

        ref_exec.insert_chunk(create_test_chunk(0, 0));
        ref_exec.insert_chunk(create_test_chunk(1, 0));
        parcel_exec.insert_chunk(create_test_chunk(0, 0));
        parcel_exec.insert_chunk(create_test_chunk(1, 0));

        // Player 1 in Parcel 0 at x=15.8. Envelope [15.5, 16.1] crosses into Chunk (1, 0) Parcel 1 at x=16!
        let p1 = PlayerState::new(PlayerId(1), 15.8, 64.0, 8.0, ParcelId(0));
        // Player 2 in Parcel 1 at x=18.0
        let p2 = PlayerState::new(PlayerId(2), 18.0, 64.0, 8.0, ParcelId(1));

        ref_exec.insert_player(p1.clone());
        ref_exec.insert_player(p2.clone());
        parcel_exec.insert_player(p1);
        parcel_exec.insert_player(p2);

        // Pulse 1: Player 2 attempts to place block at (16, 64, 8) which intersects Player 1 envelope
        let pulse1 = PulseInput::new(PulseId(1)).with_intents(vec![AdmittedIntent::new(
            1,
            PlayerId(2),
            1,
            IntentKind::PlaceBlock {
                target: CellCoord::new(16, 64, 8),
                block: Block::Stone,
                claimed_block: Block::Air,
            },
        )]);

        ref_exec.step(pulse1.clone());
        parcel_exec.step(pulse1);

        assert_execution_equivalent(&ref_exec, &parcel_exec);
        // Player 2's placement must be rejected with WouldEmbed
        assert_eq!(
            ref_exec.event_log()[0],
            PulseEvent::IntentRejected {
                pulse_id: PulseId(1),
                player_id: PlayerId(2),
                sequence: 1,
                admission_order: 1,
                reason: RejectionReason::WouldEmbed,
            }
        );
        assert_eq!(
            ref_exec.get_block(CellCoord::new(16, 64, 8)),
            Some(Block::Air)
        );

        // Pulse 2: Conflict resolution in canonical admission order
        // Move P1 away first (admission order 10), then P2 places block (admission order 20) -> should succeed
        let pulse2 = PulseInput::new(PulseId(2)).with_intents(vec![
            AdmittedIntent::new(
                10,
                PlayerId(1),
                1,
                IntentKind::Move {
                    to_x: 14.0,
                    to_y: 64.0,
                    to_z: 8.0,
                },
            ),
            AdmittedIntent::new(
                20,
                PlayerId(2),
                2,
                IntentKind::PlaceBlock {
                    target: CellCoord::new(16, 64, 8),
                    block: Block::Stone,
                    claimed_block: Block::Air,
                },
            ),
        ]);

        ref_exec.step(pulse2.clone());
        parcel_exec.step(pulse2);

        assert_execution_equivalent(&ref_exec, &parcel_exec);
        assert_eq!(
            ref_exec.get_block(CellCoord::new(16, 64, 8)),
            Some(Block::Stone)
        );
    }

    #[test]
    fn same_pulse_competing_cross_parcel_block_edits_preserve_stale_claim_rejection() {
        let mut layout = PartitionLayout::new();
        layout.assign_chunk((0, 0), ParcelId(0));
        layout.assign_chunk((1, 0), ParcelId(1));

        let mut ref_exec = ReferenceExecutor::new(layout.clone());
        let mut parcel_exec = ParcelExecutor::new(layout);

        let c0 = create_test_chunk(0, 0);
        let mut c1 = create_test_chunk(1, 0);
        // Border block at (16, 64, 8) in Chunk (1, 0)
        c1.set_block_raw(0, 64, 8, Block::Stone);

        ref_exec.insert_chunk(c0.clone());
        ref_exec.insert_chunk(c1.clone());
        parcel_exec.insert_chunk(c0);
        parcel_exec.insert_chunk(c1);

        // Player 1 in Parcel 0 at x=14.0 within reach of x=16
        let p1 = PlayerState::new(PlayerId(1), 14.0, 64.0, 8.0, ParcelId(0));
        // Player 2 in Parcel 1 at x=18.0 within reach of x=16
        let p2 = PlayerState::new(PlayerId(2), 18.0, 64.0, 8.0, ParcelId(1));

        ref_exec.insert_player(p1.clone());
        ref_exec.insert_player(p2.clone());
        parcel_exec.insert_player(p1);
        parcel_exec.insert_player(p2);

        // Both players submit BreakBlock claiming Stone on (16, 64, 8)
        let pulse = PulseInput::new(PulseId(1)).with_intents(vec![
            AdmittedIntent::new(
                10,
                PlayerId(1),
                1,
                IntentKind::BreakBlock {
                    target: CellCoord::new(16, 64, 8),
                    claimed_block: Block::Stone,
                },
            ),
            AdmittedIntent::new(
                20,
                PlayerId(2),
                1,
                IntentKind::BreakBlock {
                    target: CellCoord::new(16, 64, 8),
                    claimed_block: Block::Stone,
                },
            ),
        ]);

        ref_exec.step(pulse.clone());
        parcel_exec.step(pulse);

        assert_execution_equivalent(&ref_exec, &parcel_exec);

        // Player 1 committed, Player 2 rejected with StaleClaim
        let events = ref_exec.event_log();
        assert_eq!(
            events[0],
            PulseEvent::IntentCommitted {
                pulse_id: PulseId(1),
                player_id: PlayerId(1),
                sequence: 1,
                admission_order: 10,
                effect: CommittedEffect::block_change(
                    CellCoord::new(16, 64, 8),
                    Block::Stone,
                    Block::Air
                ),
            }
        );
        assert_eq!(
            events[1],
            PulseEvent::IntentRejected {
                pulse_id: PulseId(1),
                player_id: PlayerId(2),
                sequence: 1,
                admission_order: 20,
                reason: RejectionReason::StaleClaim,
            }
        );
    }

    #[test]
    fn dynamic_repartition_across_pulses_and_duplicate_sequence_idempotence() {
        // Initial layout: Chunks (0, 0) and (1, 0) both in Parcel 0
        let mut layout1 = PartitionLayout::new();
        layout1.assign_chunk((0, 0), ParcelId(0));
        layout1.assign_chunk((1, 0), ParcelId(0));

        let mut ref_exec = ReferenceExecutor::new(layout1.clone());
        let mut parcel_exec = ParcelExecutor::new(layout1);

        ref_exec.insert_chunk(create_test_chunk(0, 0));
        ref_exec.insert_chunk(create_test_chunk(1, 0));
        parcel_exec.insert_chunk(create_test_chunk(0, 0));
        parcel_exec.insert_chunk(create_test_chunk(1, 0));

        let player = PlayerState::new(PlayerId(99), 17.0, 64.0, 8.0, ParcelId(0));
        ref_exec.insert_player(player.clone());
        parcel_exec.insert_player(player);

        // Pulse 1: Place stone at (18, 64, 8) with sequence 1
        let pulse1 = PulseInput::new(PulseId(1)).with_intents(vec![AdmittedIntent::new(
            1,
            PlayerId(99),
            1,
            IntentKind::PlaceBlock {
                target: CellCoord::new(18, 64, 8),
                block: Block::Stone,
                claimed_block: Block::Air,
            },
        )]);

        ref_exec.step(pulse1.clone());
        parcel_exec.step(pulse1);

        assert_execution_equivalent(&ref_exec, &parcel_exec);

        // Pulse 2: Dynamic repartition moves Chunk (1, 0) to Parcel 1
        // Also: Player retransmits duplicate sequence 1, and submits new sequence 2
        let mut layout2 = PartitionLayout::new();
        layout2.assign_chunk((0, 0), ParcelId(0));
        layout2.assign_chunk((1, 0), ParcelId(1));

        let pulse2 = PulseInput::new(PulseId(2))
            .with_layout(layout2)
            .with_intents(vec![
                AdmittedIntent::new(
                    10,
                    PlayerId(99),
                    1, // Duplicate sequence
                    IntentKind::PlaceBlock {
                        target: CellCoord::new(18, 64, 8),
                        block: Block::Stone,
                        claimed_block: Block::Air,
                    },
                ),
                AdmittedIntent::new(
                    20,
                    PlayerId(99),
                    2, // Valid new sequence
                    IntentKind::BreakBlock {
                        target: CellCoord::new(18, 64, 8),
                        claimed_block: Block::Stone,
                    },
                ),
            ]);

        ref_exec.step(pulse2.clone());
        parcel_exec.step(pulse2);

        assert_execution_equivalent(&ref_exec, &parcel_exec);

        // Player now in Parcel 1 with last_sequence 2
        let all_players = parcel_exec.all_players();
        let p_par = all_players.get(&PlayerId(99)).unwrap();
        assert_eq!(p_par.parcel_id, ParcelId(1));
        assert_eq!(p_par.last_sequence, 2);

        // Block broken to Air
        assert_eq!(
            parcel_exec.get_block(CellCoord::new(18, 64, 8)),
            Some(Block::Air)
        );
    }

    #[test]
    fn test_nonfinite_and_overflow_move_intents_rejected_identically() {
        let chunk0 = create_test_chunk(0, 0);
        let mut layout = PartitionLayout::new();
        layout.assign_chunk((0, 0), ParcelId(0));

        let mut ref_exec = ReferenceExecutor::new(layout.clone());
        ref_exec.insert_chunk(chunk0.clone());
        ref_exec.insert_player(PlayerState::new(PlayerId(1), 8.0, 64.0, 8.0, ParcelId(0)));

        let mut parcel_exec = ParcelExecutor::new(layout.clone());
        parcel_exec.insert_chunk(chunk0);
        parcel_exec.insert_player(PlayerState::new(PlayerId(1), 8.0, 64.0, 8.0, ParcelId(0)));

        let nonfinite_cases = [
            (f64::NAN, 64.0, 8.0),
            (8.0, f64::NAN, 8.0),
            (8.0, 64.0, f64::NAN),
            (f64::INFINITY, 64.0, 8.0),
            (-f64::INFINITY, 64.0, 8.0),
            (8.0, f64::INFINITY, 8.0),
            (8.0, -10.0, 8.0),
            (8.0, 300.0, 8.0),
            (35_000_000.0, 64.0, 8.0),
            (-35_000_000.0, 64.0, 8.0),
            (8.0, 64.0, 35_000_000.0),
        ];

        for (seq, (x, y, z)) in (1u64..).zip(nonfinite_cases) {
            let pulse = PulseInput::new(PulseId(seq)).with_intents(vec![AdmittedIntent::new(
                1,
                PlayerId(1),
                seq,
                IntentKind::Move {
                    to_x: x,
                    to_y: y,
                    to_z: z,
                },
            )]);

            ref_exec.step(pulse.clone());
            parcel_exec.step(pulse);

            assert_execution_equivalent(&ref_exec, &parcel_exec);

            let rejected = ref_exec.event_log.iter().any(|e| {
                matches!(
                    e,
                    PulseEvent::IntentRejected {
                        pulse_id: PulseId(p),
                        sequence: s,
                        reason: RejectionReason::OutOfBounds,
                        ..
                    } if *p == seq && *s == seq
                )
            });
            assert!(
                rejected,
                "case ({x}, {y}, {z}) should reject with OutOfBounds"
            );
        }
    }
}
