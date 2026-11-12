//! World time progression and weather cycle state machine.
//!
//! Tracks the diurnal cycle (`day_time`, `game_time`) and a weather state machine
//! (`Clear`, `Rain`, `Thunder`).
//!
//! # Diurnal Specification
//!
//! - **Total Day Length**: 24,000 ticks (20 minutes of real time at 20 TPS).
//! - **Day Start (06:00)**: Tick 0.
//! - **Noon (12:00)**: Tick 6,000.
//! - **Sunset (18:00)**: Tick 12,000.
//! - **Sleep Allowed**: Tick 12,542 to 23,458 (or whenever thundering).
//! - **Midnight (00:00)**: Tick 18,000.
//! - **Sunrise (05:00)**: Tick 23,000.
//! - **Game Time**: Monotonic tick count representing world age. Only simulated
//!   ticks count, so a sleep skip leaves it unchanged.
//! - **Day Time**: If negative in Java protocol packets, daylight cycle is paused (`doDaylightCycle = false`).
//!
//! # Weather Cycle
//!
//! When a weather interval expires the state moves `Clear` -> `Rain` -> `Thunder`
//! -> `Clear` and takes its next duration from a [`WeatherSchedule`]. This is a
//! deterministic RustMC schedule, not vanilla weather: vanilla's randomized
//! intervals and its independent rain and thunder timers are not modelled.

/// Total ticks in a single Minecraft day.
pub const TICKS_PER_DAY: i64 = 24_000;

/// Tick at which night begins and players may sleep in beds.
pub const SLEEP_START_TICK: i64 = 12_542;

/// Tick at which night ends and players wake from beds.
pub const SLEEP_END_TICK: i64 = 23_458;

/// World diurnal and total time tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldTime {
    game_time: i64,
    day_time: i64,
    cycle_enabled: bool,
}

impl Default for WorldTime {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldTime {
    pub const fn new() -> Self {
        Self {
            game_time: 0,
            day_time: 0,
            cycle_enabled: true,
        }
    }

    pub const fn from_times(game_time: i64, day_time: i64, cycle_enabled: bool) -> Self {
        Self {
            game_time: if game_time < 0 { 0 } else { game_time },
            day_time: day_time.rem_euclid(TICKS_PER_DAY),
            cycle_enabled,
        }
    }

    #[inline]
    pub const fn game_time(&self) -> i64 {
        self.game_time
    }

    #[inline]
    pub const fn day_time(&self) -> i64 {
        self.day_time
    }

    #[inline]
    pub const fn cycle_enabled(&self) -> bool {
        self.cycle_enabled
    }

    pub fn set_cycle_enabled(&mut self, enabled: bool) {
        self.cycle_enabled = enabled;
    }

    pub fn set_day_time(&mut self, time: i64) {
        self.day_time = time.rem_euclid(TICKS_PER_DAY);
    }

    /// Advances world time by `ticks`.
    pub fn advance(&mut self, ticks: u32) {
        let delta = ticks as i64;
        self.game_time = self.game_time.saturating_add(delta);
        if self.cycle_enabled {
            self.day_time = (self.day_time + delta).rem_euclid(TICKS_PER_DAY);
        }
    }

    /// Checks if it is currently daytime (0..12,000 ticks).
    pub const fn is_day(&self) -> bool {
        self.day_time < 12_000
    }

    /// Checks if it is currently nighttime (12,000..24,000 ticks).
    pub const fn is_night(&self) -> bool {
        self.day_time >= 12_000
    }

    /// Checks if sleeping in a bed is currently permitted based solely on time of day.
    pub const fn can_sleep_time(&self) -> bool {
        self.day_time >= SLEEP_START_TICK && self.day_time <= SLEEP_END_TICK
    }

    /// Skips forward to the next morning (06:00 / tick 0 of the next diurnal cycle).
    /// World age is unchanged because the skipped ticks were never simulated.
    pub fn skip_to_morning(&mut self) {
        self.day_time = 0;
    }

    /// Computes the network packet day_time value according to Java Edition rules
    /// (negative day_time indicates daylight cycle is paused).
    pub const fn packet_day_time(&self) -> i64 {
        if self.cycle_enabled {
            self.day_time
        } else if self.day_time == 0 {
            -TICKS_PER_DAY
        } else {
            -self.day_time
        }
    }
}

/// The three weather conditions in Minecraft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherKind {
    Clear,
    Rain,
    Thunder,
}

impl WeatherKind {
    #[inline]
    pub const fn is_raining(self) -> bool {
        matches!(self, Self::Rain | Self::Thunder)
    }

    #[inline]
    pub const fn is_thundering(self) -> bool {
        matches!(self, Self::Thunder)
    }

    /// The weather that follows this one when its interval expires.
    #[inline]
    pub const fn next(self) -> Self {
        match self {
            Self::Clear => Self::Rain,
            Self::Rain => Self::Thunder,
            Self::Thunder => Self::Clear,
        }
    }
}

/// How long each weather kind lasts once the cycle enters it, in ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeatherSchedule {
    pub clear_ticks: u32,
    pub rain_ticks: u32,
    pub thunder_ticks: u32,
}

impl WeatherSchedule {
    /// Half a day per weather kind. A project default, not a vanilla interval.
    pub const DEFAULT_TICKS: u32 = 12_000;

    pub const fn new() -> Self {
        Self {
            clear_ticks: Self::DEFAULT_TICKS,
            rain_ticks: Self::DEFAULT_TICKS,
            thunder_ticks: Self::DEFAULT_TICKS,
        }
    }

    pub const fn duration(&self, kind: WeatherKind) -> u32 {
        match kind {
            WeatherKind::Clear => self.clear_ticks,
            WeatherKind::Rain => self.rain_ticks,
            WeatherKind::Thunder => self.thunder_ticks,
        }
    }
}

impl Default for WeatherSchedule {
    fn default() -> Self {
        Self::new()
    }
}

/// Authoritative weather simulation state machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherState {
    kind: WeatherKind,
    remaining_duration: u32,
    rain_level: f32,
    thunder_level: f32,
    schedule: WeatherSchedule,
}

impl Default for WeatherState {
    fn default() -> Self {
        Self::new()
    }
}

impl WeatherState {
    pub const fn new() -> Self {
        Self {
            kind: WeatherKind::Clear,
            remaining_duration: WeatherSchedule::DEFAULT_TICKS,
            rain_level: 0.0,
            thunder_level: 0.0,
            schedule: WeatherSchedule::new(),
        }
    }

    pub const fn from_kind(kind: WeatherKind, duration: u32) -> Self {
        let (rain, thunder) = match kind {
            WeatherKind::Clear => (0.0, 0.0),
            WeatherKind::Rain => (1.0, 0.0),
            WeatherKind::Thunder => (1.0, 1.0),
        };
        Self {
            kind,
            remaining_duration: duration,
            rain_level: rain,
            thunder_level: thunder,
            schedule: WeatherSchedule::new(),
        }
    }

    /// Uses `schedule` for every interval after the current one.
    pub const fn with_schedule(mut self, schedule: WeatherSchedule) -> Self {
        self.schedule = schedule;
        self
    }

    #[inline]
    pub const fn kind(&self) -> WeatherKind {
        self.kind
    }

    #[inline]
    pub const fn remaining_duration(&self) -> u32 {
        self.remaining_duration
    }

    #[inline]
    pub const fn rain_level(&self) -> f32 {
        self.rain_level
    }

    #[inline]
    pub const fn thunder_level(&self) -> f32 {
        self.thunder_level
    }

    /// Sets weather kind and duration explicitly (e.g. from `/weather` command).
    pub fn set_weather(&mut self, kind: WeatherKind, duration: u32) {
        self.kind = kind;
        self.remaining_duration = duration;
    }

    /// Steps the weather simulation by one tick: counts down, enters the next
    /// weather when the interval expires, and interpolates gradient levels.
    pub fn tick(&mut self) {
        self.remaining_duration = self.remaining_duration.saturating_sub(1);
        if self.remaining_duration == 0 {
            self.kind = self.kind.next();
            self.remaining_duration = self.schedule.duration(self.kind);
        }

        // Smooth transition for rain level (rate: 0.01 per tick)
        let target_rain = if self.kind.is_raining() { 1.0 } else { 0.0 };
        if self.rain_level < target_rain {
            self.rain_level = (self.rain_level + 0.01).min(target_rain);
        } else if self.rain_level > target_rain {
            self.rain_level = (self.rain_level - 0.01).max(target_rain);
        }

        // Smooth transition for thunder level (rate: 0.01 per tick)
        let target_thunder = if self.kind.is_thundering() { 1.0 } else { 0.0 };
        if self.thunder_level < target_thunder {
            self.thunder_level = (self.thunder_level + 0.01).min(target_thunder);
        } else if self.thunder_level > target_thunder {
            self.thunder_level = (self.thunder_level - 0.01).max(target_thunder);
        }
    }
}
