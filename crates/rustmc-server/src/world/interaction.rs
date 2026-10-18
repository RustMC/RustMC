//! Authoritative block interaction engine and validation types.
//!
//! Handles server-side validation of block digging and placement intents
//! according to Minecraft protocol rules and spatial constraints (reach,
//! world bounds, player hitbox collision).

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

/// Rejection reason when an authoritative block interaction cannot be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InteractionRejectReason {
    TooFar,
    OutOfBounds,
    InsidePlayer,
    TargetNotReplaceable,
    Malformed,
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

    /// Validates a digging action authoritative intent.
    pub fn validate_digging(
        player_eye: (f64, f64, f64),
        is_creative: bool,
        action: &DiggingAction,
        current_block: Option<&str>,
    ) -> InteractionResult {
        if player_eye.0.is_nan()
            || player_eye.1.is_nan()
            || player_eye.2.is_nan()
            || player_eye.0.is_infinite()
            || player_eye.1.is_infinite()
            || player_eye.2.is_infinite()
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

        match action.status {
            DiggingStatus::Cancelled => InteractionResult::Accepted {
                placed: None,
                broken: None,
                sequence: action.sequence,
            },
            DiggingStatus::Started => {
                if is_creative {
                    InteractionResult::Accepted {
                        placed: None,
                        broken: Some(action.pos),
                        sequence: action.sequence,
                    }
                } else {
                    InteractionResult::Accepted {
                        placed: None,
                        broken: None,
                        sequence: action.sequence,
                    }
                }
            }
            DiggingStatus::Finished => {
                if !is_creative && matches!(current_block, Some("minecraft:bedrock" | "bedrock")) {
                    InteractionResult::Rejected {
                        reason: InteractionRejectReason::TargetNotReplaceable,
                        sequence: action.sequence,
                    }
                } else {
                    InteractionResult::Accepted {
                        placed: None,
                        broken: Some(action.pos),
                        sequence: action.sequence,
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

        // AABB collision test between placed block [min, max] and player hitbox
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

        InteractionResult::Accepted {
            placed: Some(placed_pos),
            broken: None,
            sequence: action.sequence,
        }
    }
}
