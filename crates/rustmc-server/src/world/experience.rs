//! Player experience points, leveling, and progress mechanics.
//!
//! The curves below match the community [Minecraft Wiki Experience page], retrieved
//! 7 November 2026. That is a secondary source not pinned to Java 26.3, and no primary
//! source or client observation has been recorded for them.
//!
//! [Minecraft Wiki Experience page]: https://minecraft.wiki/w/Experience
//!
//! # Formulas (Java Edition)
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
//! Tracks `level`, `progress` (in `[0.0, 1.0)`), and `total_xp`. The three always describe
//! one position on the curve: `total_xp` determines `level` and `progress`.
//! The same page gives the death drop as `min(100, 7 * level)` XP points.
//!
//! # Limits
//!
//! [`MAX_LEVEL`] and [`MAX_TOTAL_XP`] are RustMC policy, not vanilla values: the wiki
//! describes far higher levels. They keep every total inside `i32`, and a total is never
//! retained above the cap, so no experience hides behind a clamped progress bar.

use std::fmt;

/// Maximum player level, a RustMC policy limit that keeps cumulative totals inside `i32`.
pub const MAX_LEVEL: u32 = 21_863;

/// Maximum retained experience: exactly the total at [`MAX_LEVEL`] with an empty bar.
pub const MAX_TOTAL_XP: u32 = 2_147_407_943;

/// Returns the experience points required to advance from `level` to `level + 1`.
///
/// Levels above [`MAX_LEVEL`] are treated as [`MAX_LEVEL`].
pub fn xp_for_next_level(level: u32) -> u32 {
    let level = level.min(MAX_LEVEL);
    if level <= 15 {
        2 * level + 7
    } else if level <= 30 {
        5 * level - 38
    } else {
        9 * level - 158
    }
}

/// Returns the exact cumulative experience points required to reach level `level` from 0.
///
/// Levels above [`MAX_LEVEL`] are treated as [`MAX_LEVEL`].
pub fn total_xp_for_level(level: u32) -> u32 {
    let l = u64::from(level.min(MAX_LEVEL));
    let total = if l <= 16 {
        l * l + 6 * l
    } else if l <= 31 {
        (5 * l * l + 720 - 81 * l) / 2
    } else {
        // The positive constant comes first: 9 * 32^2 is smaller than 325 * 32.
        (9 * l * l + 4440 - 325 * l) / 2
    };
    total as u32
}

/// Converts cumulative experience points into `(level, remaining_xp_in_level)`.
///
/// Totals above [`MAX_TOTAL_XP`] are treated as [`MAX_TOTAL_XP`].
pub fn level_from_total_xp(total_xp: u32) -> (u32, u32) {
    let mut total_xp = total_xp.min(MAX_TOTAL_XP);
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
    ///
    /// Experience beyond [`MAX_TOTAL_XP`] is discarded.
    pub fn add_experience(&mut self, amount: u32) {
        self.set_total_xp(self.total_xp.saturating_add(amount));
    }

    /// Stores `total_xp` and derives the level and progress bar from it.
    fn set_total_xp(&mut self, total_xp: u32) {
        self.total_xp = total_xp.min(MAX_TOTAL_XP);
        let (new_level, in_level_xp) = level_from_total_xp(self.total_xp);
        self.level = new_level;
        let req = xp_for_next_level(self.level);
        self.progress = (in_level_xp as f32 / req as f32).clamp(0.0, 0.999_999);
    }

    /// Consumes experience points directly.
    pub fn consume_experience(&mut self, amount: u32) -> Result<(), ExperienceError> {
        if self.total_xp < amount {
            return Err(ExperienceError::InsufficientXp {
                required: amount,
                available: self.total_xp,
            });
        }
        self.set_total_xp(self.total_xp - amount);
        Ok(())
    }

    /// Consumes full levels (e.g. for enchanting tables or anvil repairs).
    ///
    /// Carries the fractional progress bar into the new lower level, rounded to a whole
    /// point and kept below that level's requirement so no spent level is restored.
    pub fn consume_levels(&mut self, levels: u32) -> Result<(), ExperienceError> {
        if self.level < levels {
            return Err(ExperienceError::InsufficientLevels {
                required: levels,
                available: self.level,
            });
        }
        let new_level = self.level - levels;
        let req = xp_for_next_level(new_level);
        let in_level = ((self.progress * req as f32).round() as u32).min(req - 1);
        self.set_total_xp(total_xp_for_level(new_level) + in_level);
        Ok(())
    }

    /// Computes experience dropped by the player upon death.
    ///
    /// Capped at 100 XP (or `7 * level` if lower).
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
