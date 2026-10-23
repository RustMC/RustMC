//! Player survival state, health, hunger, exhaustion, and environmental damage.

/// Causes of player damage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DamageSource {
    Fall,
    Drowning,
    Fire,
    Lava,
    Suffocation,
    Starvation,
    Void,
}

impl std::fmt::Display for DamageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fall => write!(f, "fall"),
            Self::Drowning => write!(f, "drowning"),
            Self::Fire => write!(f, "fire"),
            Self::Lava => write!(f, "lava"),
            Self::Suffocation => write!(f, "suffocation"),
            Self::Starvation => write!(f, "starvation"),
            Self::Void => write!(f, "void"),
        }
    }
}

/// A damage event recording damage applied to a player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageEvent {
    pub source: DamageSource,
    pub amount: f32,
}

impl DamageEvent {
    /// Constructs a new damage event.
    pub const fn new(source: DamageSource, amount: f32) -> Self {
        Self { source, amount }
    }
}

pub const MAX_HEALTH: f32 = 20.0;
pub const MAX_FOOD_LEVEL: u32 = 20;
pub const MAX_SATURATION: f32 = 20.0;
pub const DEFAULT_SATURATION: f32 = 5.0;
pub const MAX_EXHAUSTION: f32 = 4.0;
pub const MAX_AIR_SUPPLY: i32 = 300;
pub const MIN_AIR_SUPPLY: i32 = -20;

pub const DROWNING_DAMAGE: f32 = 2.0;
pub const VOID_DAMAGE: f32 = 4.0;
pub const SUFFOCATION_DAMAGE: f32 = 1.0;
pub const STARVATION_DAMAGE: f32 = 1.0;

pub const REGEN_HEAL_AMOUNT: f32 = 1.0;
pub const REGEN_EXHAUSTION: f32 = 6.0;

pub const FAST_HEAL_INTERVAL_TICKS: u32 = 10;
pub const STANDARD_HEAL_INTERVAL_TICKS: u32 = 80;
pub const STARVATION_INTERVAL_TICKS: u32 = 80;
pub const SUFFOCATION_INTERVAL_TICKS: u32 = 20;

pub const VOID_THRESHOLD_Y: f64 = -64.0;
pub const FALL_DAMAGE_DISTANCE_THRESHOLD: f32 = 3.0;

/// Authoritative player survival state including health, hunger, and environmental hazard tracking.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerSurvival {
    /// Current health, clamped to `0.0..=20.0`.
    pub health: f32,
    /// Current food level, clamped to `0..=20`.
    pub food_level: u32,
    /// Food saturation reserve, clamped to `0.0..=20.0`.
    pub saturation: f32,
    /// Exhaustion level towards the next food/saturation depletion, clamped to `0.0..=4.0`.
    pub exhaustion: f32,
    /// Air supply in ticks, clamped to `-20..=300`.
    pub air_supply: i32,
    /// Distance accumulated during fall.
    pub fall_distance: f32,
    /// Elapsed ticks suffocating inside a block.
    pub suffocation_timer: u32,
    /// Elapsed ticks for food healing or starvation.
    pub food_timer: u32,
}

impl Default for PlayerSurvival {
    fn default() -> Self {
        Self {
            health: MAX_HEALTH,
            food_level: MAX_FOOD_LEVEL,
            saturation: DEFAULT_SATURATION,
            exhaustion: 0.0,
            air_supply: MAX_AIR_SUPPLY,
            fall_distance: 0.0,
            suffocation_timer: 0,
            food_timer: 0,
        }
    }
}

impl PlayerSurvival {
    /// Creates a new player survival state with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enforces boundary clamping across all survival fields.
    pub fn clamp(&mut self) {
        self.health = self.health.clamp(0.0, MAX_HEALTH);
        self.food_level = self.food_level.min(MAX_FOOD_LEVEL);
        self.saturation = self.saturation.clamp(0.0, MAX_SATURATION);
        self.exhaustion = self.exhaustion.clamp(0.0, MAX_EXHAUSTION);
        self.air_supply = self.air_supply.clamp(MIN_AIR_SUPPLY, MAX_AIR_SUPPLY);
        if self.fall_distance < 0.0 {
            self.fall_distance = 0.0;
        }
    }

    /// Whether the player is currently alive (`health > 0.0`).
    pub fn is_alive(&self) -> bool {
        self.health > 0.0
    }

    /// Applies damage to the player, deducting health and returning the actual damage dealt.
    pub fn apply_damage(&mut self, _source: DamageSource, amount: f32) -> f32 {
        if !self.is_alive() || amount <= 0.0 || amount.is_nan() {
            return 0.0;
        }
        let actual = amount.min(self.health);
        self.health = (self.health - actual).clamp(0.0, MAX_HEALTH);
        actual
    }

    /// Restores health up to the maximum of 20.0.
    pub fn heal(&mut self, amount: f32) {
        if !self.is_alive() || amount <= 0.0 || amount.is_nan() {
            return;
        }
        self.health = (self.health + amount).min(MAX_HEALTH);
    }

    /// Adds exhaustion points. When exhaustion reaches or exceeds 4.0, reduces saturation
    /// (or food level if saturation is 0.0) and wraps the remaining exhaustion.
    pub fn add_exhaustion(&mut self, amount: f32) {
        if amount <= 0.0 || !amount.is_finite() {
            return;
        }
        self.exhaustion += amount;
        let points = (self.exhaustion / MAX_EXHAUSTION).floor() as u32;
        if points > 0 {
            self.exhaustion -= points as f32 * MAX_EXHAUSTION;
            let mut remaining_points = points;
            if self.saturation > 0.0 {
                let sat_deduct = (self.saturation.ceil() as u32).min(remaining_points);
                self.saturation = (self.saturation - sat_deduct as f32).max(0.0);
                remaining_points -= sat_deduct;
            }
            if remaining_points > 0 {
                self.food_level = self.food_level.saturating_sub(remaining_points);
            }
        }
        self.exhaustion = self.exhaustion.clamp(0.0, MAX_EXHAUSTION);
    }

    /// Evaluates fall distance and ground state. When landing on ground, resets `fall_distance`
    /// and calculates damage as `(fall_distance - 3.0).max(0.0)`.
    pub fn handle_fall(&mut self, on_ground: bool, fall_distance: f32) -> Option<DamageEvent> {
        if !self.is_alive() {
            return None;
        }
        if on_ground {
            let dist = if fall_distance > 0.0 {
                fall_distance
            } else {
                self.fall_distance
            };
            self.fall_distance = 0.0;
            let damage = (dist - FALL_DAMAGE_DISTANCE_THRESHOLD).max(0.0);
            if damage > 0.0 {
                self.apply_damage(DamageSource::Fall, damage);
                Some(DamageEvent {
                    source: DamageSource::Fall,
                    amount: damage,
                })
            } else {
                None
            }
        } else {
            self.fall_distance = fall_distance.max(self.fall_distance);
            None
        }
    }

    /// Runs a deterministic survival simulation tick.
    ///
    /// - Air supply: depletes in water (-1 per tick), refills instantly out of water.
    ///   If <= 0, applies 2.0 drowning damage every 20 ticks.
    /// - Void: if `y_pos < -64.0`, applies 4.0 void damage per tick.
    /// - Suffocation: if `is_suffocating`, applies 1.0 damage every 20 ticks.
    /// - Food & regeneration: fast heal at food 20 & saturation > 0, standard heal at food >= 18,
    ///   starvation damage at food == 0 every 80 ticks.
    pub fn tick(
        &mut self,
        is_in_water: bool,
        is_suffocating: bool,
        y_pos: f64,
    ) -> Vec<DamageEvent> {
        if !self.is_alive() {
            return Vec::new();
        }

        self.clamp();
        let mut events = Vec::new();

        // 1. Air supply / Drowning
        if is_in_water {
            self.air_supply = (self.air_supply - 1).clamp(MIN_AIR_SUPPLY, MAX_AIR_SUPPLY);
            if self.air_supply <= MIN_AIR_SUPPLY {
                self.air_supply = 0;
                self.apply_damage(DamageSource::Drowning, DROWNING_DAMAGE);
                events.push(DamageEvent {
                    source: DamageSource::Drowning,
                    amount: DROWNING_DAMAGE,
                });
            }
        } else {
            self.air_supply = MAX_AIR_SUPPLY;
        }

        if !self.is_alive() {
            return events;
        }

        // 2. Void damage
        if y_pos < VOID_THRESHOLD_Y {
            self.apply_damage(DamageSource::Void, VOID_DAMAGE);
            events.push(DamageEvent {
                source: DamageSource::Void,
                amount: VOID_DAMAGE,
            });
        }

        if !self.is_alive() {
            return events;
        }

        // 3. Suffocation damage
        if is_suffocating {
            self.suffocation_timer += 1;
            if self.suffocation_timer >= SUFFOCATION_INTERVAL_TICKS {
                self.suffocation_timer = 0;
                self.apply_damage(DamageSource::Suffocation, SUFFOCATION_DAMAGE);
                events.push(DamageEvent {
                    source: DamageSource::Suffocation,
                    amount: SUFFOCATION_DAMAGE,
                });
            }
        } else {
            self.suffocation_timer = 0;
        }

        if !self.is_alive() {
            return events;
        }

        // 4. Food & natural regeneration / Starvation
        if self.health < MAX_HEALTH && self.food_level >= 20 && self.saturation > 0.0 {
            self.food_timer += 1;
            if self.food_timer >= FAST_HEAL_INTERVAL_TICKS {
                self.food_timer = 0;
                self.heal(REGEN_HEAL_AMOUNT);
                self.add_exhaustion(REGEN_EXHAUSTION);
            }
        } else if self.health < MAX_HEALTH && self.food_level >= 18 {
            self.food_timer += 1;
            if self.food_timer >= STANDARD_HEAL_INTERVAL_TICKS {
                self.food_timer = 0;
                self.heal(REGEN_HEAL_AMOUNT);
                self.add_exhaustion(REGEN_EXHAUSTION);
            }
        } else if self.food_level == 0 {
            self.food_timer += 1;
            if self.food_timer >= STARVATION_INTERVAL_TICKS {
                self.food_timer = 0;
                self.apply_damage(DamageSource::Starvation, STARVATION_DAMAGE);
                events.push(DamageEvent {
                    source: DamageSource::Starvation,
                    amount: STARVATION_DAMAGE,
                });
            }
        } else {
            self.food_timer = 0;
        }

        events
    }
}
