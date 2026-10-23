//! Authoritative block interaction engine and validation types.
//!
//! Handles server-side validation of block digging progression and placement intents
//! according to Minecraft protocol rules and spatial constraints (reach, world bounds,
//! tick-based digging progression, and block-type aware player hitbox collision).
//!
//! # Scope & Progression
//!
//! This module provides authoritative validation for digging and placement actions.
//! Survival digging progression enforces start-to-finish tick duration checks and
//! rejects unbreakable blocks (e.g. bedrock). Placement checks account for non-collidable
//! blocks (e.g. torches, flowers) to avoid invalid collision rejections.

/// 3D cardinal and vertical directions for block faces and neighbor offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Direction {
    pub const ALL: [Self; 6] = [
        Self::Down,
        Self::Up,
        Self::North,
        Self::South,
        Self::West,
        Self::East,
    ];

    /// Returns the coordinate offset along this direction: (x, y, z).
    #[inline]
    pub fn offset(self, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
        match self {
            Self::Down => (x, y.saturating_sub(1), z),
            Self::Up => (x, y.saturating_add(1), z),
            Self::North => (x, y, z.saturating_sub(1)),
            Self::South => (x, y, z.saturating_add(1)),
            Self::West => (x.saturating_sub(1), y, z),
            Self::East => (x.saturating_add(1), y, z),
        }
    }

    /// Returns the unit vector (dx, dy, dz) for this direction.
    #[inline]
    pub const fn unit_vector(self) -> (i32, i32, i32) {
        match self {
            Self::Down => (0, -1, 0),
            Self::Up => (0, 1, 0),
            Self::North => (0, 0, -1),
            Self::South => (0, 0, 1),
            Self::West => (-1, 0, 0),
            Self::East => (1, 0, 0),
        }
    }

    /// Returns the opposite direction.
    #[inline]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Down => Self::Up,
            Self::Up => Self::Down,
            Self::North => Self::South,
            Self::South => Self::North,
            Self::West => Self::East,
            Self::East => Self::West,
        }
    }
}

/// The status phase of a digging action from the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiggingStatus {
    Started,
    Cancelled,
    Finished,
}

/// Digging action intent sent by a player session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiggingAction {
    pub status: DiggingStatus,
    pub pos: (i32, i32, i32),
    pub face: Direction,
    pub sequence: i32,
}

/// Use-item-on (block placement) action intent sent by a player session.
#[derive(Debug, Clone, PartialEq)]
pub struct UseItemOnAction {
    pub pos: (i32, i32, i32),
    pub face: Direction,
    pub cursor: (f32, f32, f32),
    pub inside_block: bool,
    pub sequence: i32,
}

/// Tracks an ongoing digging action for a player session in survival mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiggingSession {
    pub pos: (i32, i32, i32),
    pub start_tick: u64,
    pub required_ticks: u32,
}

impl DiggingSession {
    pub const fn new(pos: (i32, i32, i32), start_tick: u64, required_ticks: u32) -> Self {
        Self {
            pos,
            start_tick,
            required_ticks,
        }
    }
}

/// Rejection reason when an authoritative block interaction cannot be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InteractionRejectReason {
    TooFar,
    OutOfBounds,
    InsidePlayer,
    TargetNotReplaceable,
    Malformed,
    PrematureFinish,
}

/// Result of authoritative interaction validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionResult {
    Accepted {
        placed: Option<(i32, i32, i32)>,
        broken: Option<(i32, i32, i32)>,
        sequence: i32,
    },
    Rejected {
        reason: InteractionRejectReason,
        sequence: i32,
    },
}

impl InteractionResult {
    #[inline]
    pub const fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted { .. })
    }

    #[inline]
    pub const fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }

    #[inline]
    pub const fn sequence(&self) -> i32 {
        match *self {
            Self::Accepted { sequence, .. } | Self::Rejected { sequence, .. } => sequence,
        }
    }
}

/// Authoritative validator for player block interactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockInteractionEngine;

impl BlockInteractionEngine {
    /// Maximum reach distance from player eye to target block center (4.5 blocks).
    pub const MAX_REACH_DISTANCE: f64 = 4.5;
    /// Squared reach distance (20.25) used for fast distance comparisons without sqrt.
    pub const MAX_REACH_DISTANCE_SQUARED: f64 = 20.25;

    /// Official dimension bounds for Overworld (-64..=319).
    pub const MIN_Y: i32 = -64;
    pub const MAX_Y: i32 = 319;

    /// World border horizontal limits (+-30,000,000 blocks).
    pub const MIN_XZ: i32 = -30_000_000;
    pub const MAX_XZ: i32 = 30_000_000;

    /// Checks if a block position is within the authoritative world bounds.
    #[inline]
    pub const fn is_within_bounds(pos: (i32, i32, i32)) -> bool {
        pos.1 >= Self::MIN_Y
            && pos.1 <= Self::MAX_Y
            && pos.0 >= Self::MIN_XZ
            && pos.0 <= Self::MAX_XZ
            && pos.2 >= Self::MIN_XZ
            && pos.2 <= Self::MAX_XZ
    }

    /// Computes squared distance from player eye to the center of a block position.
    #[inline]
    pub fn reach_distance_squared(player_eye: (f64, f64, f64), block_pos: (i32, i32, i32)) -> f64 {
        let center_x = block_pos.0 as f64 + 0.5;
        let center_y = block_pos.1 as f64 + 0.5;
        let center_z = block_pos.2 as f64 + 0.5;
        let dx = center_x - player_eye.0;
        let dy = center_y - player_eye.1;
        let dz = center_z - player_eye.2;
        dx * dx + dy * dy + dz * dz
    }

    /// Returns the hardness value for known standard blocks.
    /// Unbreakable blocks like bedrock return a negative value (< 0.0).
    pub fn get_block_hardness(block_id: Option<&str>) -> f32 {
        let Some(id) = block_id else {
            return 1.0;
        };
        let stripped = id.strip_prefix("minecraft:").unwrap_or(id);
        match stripped {
            "bedrock" | "barrier" | "end_portal_frame" => -1.0,
            "air" | "cave_air" | "void_air" => 0.0,
            "short_grass" | "grass" | "fern" | "dead_bush" | "dandelion" | "poppy" => 0.0,
            "dirt" | "coarse_dirt" | "grass_block" => 0.5,
            "stone" | "cobblestone" => 1.5,
            "iron_ore" | "coal_ore" | "copper_ore" => 3.0,
            "obsidian" => 50.0,
            _ => 1.0,
        }
    }

    /// Computes minimum required ticks to break a block given block hardness.
    /// In vanilla, break time in ticks without tool multiplier is `(hardness * 30.0)` for blocks requiring tools,
    /// or `(hardness * 15.0)` when harvestable. Instant breaks (hardness == 0.0) take 0 ticks.
    pub fn required_break_ticks(hardness: f32) -> u32 {
        if hardness <= 0.0 {
            0
        } else {
            (hardness * 30.0).ceil() as u32
        }
    }

    /// Checks whether a block type is collidable (has a physical collision box that blocks entities).
    /// Non-collidable blocks include air, fluids, plants, flowers, torches, redstone, etc.
    pub fn is_block_collidable(block_id: Option<&str>) -> bool {
        let Some(id) = block_id else {
            return true;
        };
        let stripped = id.strip_prefix("minecraft:").unwrap_or(id);
        !matches!(
            stripped,
            "air"
                | "cave_air"
                | "void_air"
                | "water"
                | "lava"
                | "short_grass"
                | "grass"
                | "tall_grass"
                | "fern"
                | "large_fern"
                | "dandelion"
                | "poppy"
                | "blue_orchid"
                | "allium"
                | "azure_bluet"
                | "red_tulip"
                | "orange_tulip"
                | "white_tulip"
                | "pink_tulip"
                | "oxeye_daisy"
                | "cornflower"
                | "lily_of_the_valley"
                | "wither_rose"
                | "sunflower"
                | "torch"
                | "wall_torch"
                | "redstone_torch"
                | "redstone_wall_torch"
                | "redstone_wire"
                | "lever"
                | "stone_button"
                | "oak_button"
                | "tripwire"
                | "tripwire_hook"
                | "rail"
                | "powered_rail"
                | "detector_rail"
                | "activator_rail"
        )
    }

    /// Validates a digging action authoritative intent without tracking session state.
    pub fn validate_digging(
        player_eye: (f64, f64, f64),
        is_creative: bool,
        action: &DiggingAction,
        current_block: Option<&str>,
    ) -> InteractionResult {
        let (result, _) = Self::validate_digging_stateful(
            player_eye,
            is_creative,
            action,
            current_block,
            0,
            None,
            true,
        );
        result
    }

    /// Validates a digging action with stateful session and tick-duration tracking.
    pub fn validate_digging_stateful(
        player_eye: (f64, f64, f64),
        is_creative: bool,
        action: &DiggingAction,
        current_block: Option<&str>,
        current_tick: u64,
        session: Option<&DiggingSession>,
        allow_untracked: bool,
    ) -> (InteractionResult, Option<DiggingSession>) {
        if player_eye.0.is_nan()
            || player_eye.1.is_nan()
            || player_eye.2.is_nan()
            || player_eye.0.is_infinite()
            || player_eye.1.is_infinite()
            || player_eye.2.is_infinite()
        {
            return (
                InteractionResult::Rejected {
                    reason: InteractionRejectReason::Malformed,
                    sequence: action.sequence,
                },
                None,
            );
        }

        if !Self::is_within_bounds(action.pos) {
            return (
                InteractionResult::Rejected {
                    reason: InteractionRejectReason::OutOfBounds,
                    sequence: action.sequence,
                },
                None,
            );
        }

        let dist_sq = Self::reach_distance_squared(player_eye, action.pos);
        if dist_sq > Self::MAX_REACH_DISTANCE_SQUARED {
            return (
                InteractionResult::Rejected {
                    reason: InteractionRejectReason::TooFar,
                    sequence: action.sequence,
                },
                None,
            );
        }

        let hardness = Self::get_block_hardness(current_block);

        match action.status {
            DiggingStatus::Cancelled => (
                InteractionResult::Accepted {
                    placed: None,
                    broken: None,
                    sequence: action.sequence,
                },
                None,
            ),
            DiggingStatus::Started => {
                if !is_creative && hardness < 0.0 {
                    (
                        InteractionResult::Rejected {
                            reason: InteractionRejectReason::TargetNotReplaceable,
                            sequence: action.sequence,
                        },
                        None,
                    )
                } else if is_creative {
                    (
                        InteractionResult::Accepted {
                            placed: None,
                            broken: Some(action.pos),
                            sequence: action.sequence,
                        },
                        None,
                    )
                } else {
                    let req_ticks = Self::required_break_ticks(hardness);
                    (
                        InteractionResult::Accepted {
                            placed: None,
                            broken: None,
                            sequence: action.sequence,
                        },
                        Some(DiggingSession::new(action.pos, current_tick, req_ticks)),
                    )
                }
            }
            DiggingStatus::Finished => {
                if !is_creative && hardness < 0.0 {
                    (
                        InteractionResult::Rejected {
                            reason: InteractionRejectReason::TargetNotReplaceable,
                            sequence: action.sequence,
                        },
                        None,
                    )
                } else if is_creative || allow_untracked {
                    (
                        InteractionResult::Accepted {
                            placed: None,
                            broken: Some(action.pos),
                            sequence: action.sequence,
                        },
                        None,
                    )
                } else {
                    match session {
                        Some(s) if s.pos == action.pos => {
                            let elapsed = current_tick.saturating_sub(s.start_tick);
                            if elapsed >= s.required_ticks as u64 {
                                (
                                    InteractionResult::Accepted {
                                        placed: None,
                                        broken: Some(action.pos),
                                        sequence: action.sequence,
                                    },
                                    None,
                                )
                            } else {
                                (
                                    InteractionResult::Rejected {
                                        reason: InteractionRejectReason::PrematureFinish,
                                        sequence: action.sequence,
                                    },
                                    Some(*s),
                                )
                            }
                        }
                        _ => (
                            InteractionResult::Rejected {
                                reason: InteractionRejectReason::PrematureFinish,
                                sequence: action.sequence,
                            },
                            None,
                        ),
                    }
                }
            }
        }
    }

    /// Validates a block placement authoritative intent.
    pub fn validate_block_placement(
        player_pos: (f64, f64, f64),
        player_eye: (f64, f64, f64),
        player_box: ((f64, f64, f64), (f64, f64, f64)),
        action: &UseItemOnAction,
        target_is_replaceable: bool,
        candidate_is_empty: bool,
    ) -> InteractionResult {
        Self::validate_block_placement_with_block(
            player_pos,
            player_eye,
            player_box,
            action,
            target_is_replaceable,
            candidate_is_empty,
            None,
        )
    }

    /// Validates a block placement authoritative intent with block-type collision awareness.
    pub fn validate_block_placement_with_block(
        player_pos: (f64, f64, f64),
        player_eye: (f64, f64, f64),
        player_box: ((f64, f64, f64), (f64, f64, f64)),
        action: &UseItemOnAction,
        target_is_replaceable: bool,
        candidate_is_empty: bool,
        placed_block_id: Option<&str>,
    ) -> InteractionResult {
        if player_pos.0.is_nan()
            || player_pos.1.is_nan()
            || player_pos.2.is_nan()
            || player_pos.0.is_infinite()
            || player_pos.1.is_infinite()
            || player_pos.2.is_infinite()
            || player_eye.0.is_nan()
            || player_eye.1.is_nan()
            || player_eye.2.is_nan()
            || player_eye.0.is_infinite()
            || player_eye.1.is_infinite()
            || player_eye.2.is_infinite()
            || action.cursor.0.is_nan()
            || action.cursor.1.is_nan()
            || action.cursor.2.is_nan()
            || action.cursor.0.is_infinite()
            || action.cursor.1.is_infinite()
            || action.cursor.2.is_infinite()
            || action.cursor.0 < 0.0
            || action.cursor.0 > 1.0
            || action.cursor.1 < 0.0
            || action.cursor.1 > 1.0
            || action.cursor.2 < 0.0
            || action.cursor.2 > 1.0
            || player_box.0.0 > player_box.1.0
            || player_box.0.1 > player_box.1.1
            || player_box.0.2 > player_box.1.2
            || player_box.0.0.is_nan()
            || player_box.1.0.is_nan()
            || player_box.0.1.is_nan()
            || player_box.1.1.is_nan()
            || player_box.0.2.is_nan()
            || player_box.1.2.is_nan()
        {
            return InteractionResult::Rejected {
                reason: InteractionRejectReason::Malformed,
                sequence: action.sequence,
            };
        }

        if !Self::is_within_bounds(action.pos) {
            return InteractionResult::Rejected {
                reason: InteractionRejectReason::OutOfBounds,
                sequence: action.sequence,
            };
        }

        let dist_sq = Self::reach_distance_squared(player_eye, action.pos);
        if dist_sq > Self::MAX_REACH_DISTANCE_SQUARED {
            return InteractionResult::Rejected {
                reason: InteractionRejectReason::TooFar,
                sequence: action.sequence,
            };
        }

        let placed_pos = if target_is_replaceable {
            action.pos
        } else {
            if !candidate_is_empty {
                return InteractionResult::Rejected {
                    reason: InteractionRejectReason::TargetNotReplaceable,
                    sequence: action.sequence,
                };
            }
            action.face.offset(action.pos.0, action.pos.1, action.pos.2)
        };

        if !Self::is_within_bounds(placed_pos) {
            return InteractionResult::Rejected {
                reason: InteractionRejectReason::OutOfBounds,
                sequence: action.sequence,
            };
        }

        // AABB collision test between placed block [min, max] and player hitbox,
        // applicable only if the block has physical collision geometry.
        if Self::is_block_collidable(placed_block_id) {
            let block_min = (
                placed_pos.0 as f64,
                placed_pos.1 as f64,
                placed_pos.2 as f64,
            );
            let block_max = (
                placed_pos.0 as f64 + 1.0,
                placed_pos.1 as f64 + 1.0,
                placed_pos.2 as f64 + 1.0,
            );
            let (p_min, p_max) = player_box;

            let collides = block_min.0 < p_max.0
                && block_max.0 > p_min.0
                && block_min.1 < p_max.1
                && block_max.1 > p_min.1
                && block_min.2 < p_max.2
                && block_max.2 > p_min.2;

            if collides {
                return InteractionResult::Rejected {
                    reason: InteractionRejectReason::InsidePlayer,
                    sequence: action.sequence,
                };
            }
        }

        InteractionResult::Accepted {
            placed: Some(placed_pos),
            broken: None,
            sequence: action.sequence,
        }
    }
}
