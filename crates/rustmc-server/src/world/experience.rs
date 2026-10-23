//! Player experience points, leveling, and progress mechanics.
//!
//! Implements canonical Minecraft Java 26.3 experience mechanics and leveling curves
//! as documented by the official game specifications.
//!
//! # Canonical Formulas (Java Edition)
//!
//! The experience required to advance from `level` to `level + 1`:
//! - Level 0–15: `2 * level + 7`
//! - Level 16–30: `5 * level - 38`
//! - Level 31+: `9 * level - 158`
//!
//! Total experience from level 0 up to level `L`:
//! - `0 <= L <= 16`: `L^2 + 6 * L`
//! - `17 <= L <= 31`: `(5 * L^2 - 81 * L + 720) / 2`
//! - `L >= 32`: `(9 * L^2 - 325 * L + 4440) / 2`
//!
//! # Player Experience State
//!
//! Tracks `level`, `progress` (in `[0.0, 1.0)`), and `total_xp`.
//! In vanilla Java, dying players drop `min(100, 7 * level)` XP points.

use std::fmt;

/// Maximum allowable player level (capped to prevent 32-bit arithmetic overflow).
pub const MAX_LEVEL: u32 = 21_863;

/// Returns the experience points required to advance from `level` to `level + 1`.
pub fn xp_for_next_level(level: u32) -> u32 {
    if level <= 15 {
        2 * level + 7
    } else if level <= 30 {
        5 * level - 38
    } else {
        9 * level - 158
    }
}

/// Returns the exact cumulative experience points required to reach level `level` from 0.
pub fn total_xp_for_level(level: u32) -> u32 {
    let l = level as u64;
    let total = if l <= 16 {
        l * l + 6 * l
    } else if l <= 31 {
        (5 * l * l - 81 * l + 720) / 2
    } else {
        (9 * l * l - 325 * l + 4440) / 2
    };
    total.min(u32::MAX as u64) as u32
}

/// Converts cumulative experience points into `(level, remaining_xp_in_level)`.
pub fn level_from_total_xp(mut total_xp: u32) -> (u32, u32) {
    let mut level = 0;
    loop {
        let req = xp_for_next_level(level);
        if total_xp < req {
            break;
        }
        total_xp -= req;
        level += 1;
        if level >= MAX_LEVEL {
            break;
        }
    }
    (level, total_xp)
}

/// Errors occurring during experience transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperienceError {
    InsufficientLevels { required: u32, available: u32 },
    InsufficientXp { required: u32, available: u32 },
}

impl fmt::Display for ExperienceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsufficientLevels {
                required,
                available,
            } => {
                write!(
                    f,
                    "insufficient levels: required {required}, available {available}"
                )
            }
            Self::InsufficientXp {
                required,
                available,
            } => {
                write!(
                    f,
                    "insufficient experience: required {required}, available {available}"
                )
            }
        }
    }
}

impl std::error::Error for ExperienceError {}

/// Tracks a player's experience state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerExperience {
    level: u32,
    progress: f32,
    total_xp: u32,
}

impl Default for PlayerExperience {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerExperience {
    pub const fn new() -> Self {
        Self {
            level: 0,
            progress: 0.0,
            total_xp: 0,
        }
    }

    pub fn from_total_xp(total_xp: u32) -> Self {
        let mut exp = Self::new();
        exp.add_experience(total_xp);
        exp
    }

    #[inline]
    pub const fn level(&self) -> u32 {
        self.level
    }

    #[inline]
    pub const fn progress(&self) -> f32 {
        self.progress
    }

    #[inline]
    pub const fn total_xp(&self) -> u32 {
        self.total_xp
    }

    /// Adds experience points, updating level and fractional progress.
    pub fn add_experience(&mut self, amount: u32) {
        self.total_xp = self.total_xp.saturating_add(amount);
        let (new_level, in_level_xp) = level_from_total_xp(self.total_xp);
        self.level = new_level;
        let req = xp_for_next_level(self.level);
        self.progress = if req > 0 {
            (in_level_xp as f32 / req as f32).clamp(0.0, 0.999_999)
        } else {
            0.0
        };
    }

    /// Consumes experience points directly.
    pub fn consume_experience(&mut self, amount: u32) -> Result<(), ExperienceError> {
        if self.total_xp < amount {
            return Err(ExperienceError::InsufficientXp {
                required: amount,
                available: self.total_xp,
            });
        }
        self.total_xp -= amount;
        let (new_level, in_level_xp) = level_from_total_xp(self.total_xp);
        self.level = new_level;
        let req = xp_for_next_level(self.level);
        self.progress = if req > 0 {
            (in_level_xp as f32 / req as f32).clamp(0.0, 0.999_999)
        } else {
            0.0
        };
        Ok(())
    }

    /// Consumes full levels (e.g. for enchanting tables or anvil repairs).
    ///
    /// Preserves the fractional progress bar within the new lower level.
    pub fn consume_levels(&mut self, levels: u32) -> Result<(), ExperienceError> {
        if self.level < levels {
            return Err(ExperienceError::InsufficientLevels {
                required: levels,
                available: self.level,
            });
        }
        self.level -= levels;
        let req = xp_for_next_level(self.level);
        let in_level = (self.progress * req as f32).round() as u32;
        let base_xp = total_xp_for_level(self.level);
        self.total_xp = base_xp.saturating_add(in_level);
        Ok(())
    }

    /// Computes experience dropped by the player upon death.
    ///
    /// Capped in vanilla Java Edition at 100 XP (or `7 * level` if lower).
    pub fn calculate_death_dropped_xp(&self) -> u32 {
        (self.level * 7).min(100)
    }

    /// Resets experience state upon death/respawn.
    pub fn reset_on_death(&mut self) {
        self.level = 0;
        self.progress = 0.0;
        self.total_xp = 0;
    }
}
