use super::time_weather::*;

#[test]
fn test_world_time_progression_and_day_night() {
    let mut time = WorldTime::new();
    assert_eq!(time.game_time(), 0);
    assert_eq!(time.day_time(), 0);
    assert!(time.is_day());
    assert!(!time.is_night());
    assert!(!time.can_sleep_time());

    // Advance to noon (tick 6,000)
    time.advance(6_000);
    assert_eq!(time.game_time(), 6_000);
    assert_eq!(time.day_time(), 6_000);
    assert!(time.is_day());

    // Advance to night (tick 13,000) -> night and players can sleep
    time.advance(7_000);
    assert_eq!(time.day_time(), 13_000);
    assert!(time.is_night());
    assert!(time.can_sleep_time());

    // Advance past diurnal cycle (tick 25,000) -> day_time wraps to 1,000
    time.advance(12_000);
    assert_eq!(time.game_time(), 25_000);
    assert_eq!(time.day_time(), 1_000);
    assert!(time.is_day());
}

#[test]
fn test_skip_to_morning_jump() {
    let mut time = WorldTime::from_times(13_000, 13_000, true);
    assert!(time.can_sleep_time());

    // Sleep in bed skips to next morning (tick 0)
    time.skip_to_morning();
    assert_eq!(time.day_time(), 0);
    assert_eq!(time.game_time(), 13_000, "skipped ticks never happened");
    assert!(time.is_day());

    // The next simulated tick continues from the unchanged world age.
    time.advance(1);
    assert_eq!(time.game_time(), 13_001);
    assert_eq!(time.day_time(), 1);
}

#[test]
fn test_paused_daylight_cycle_negative_packet_encoding() {
    let mut time = WorldTime::from_times(100, 6_000, true);
    assert_eq!(time.packet_day_time(), 6_000);

    // Disable daylight cycle
    time.set_cycle_enabled(false);
    assert_eq!(time.packet_day_time(), -6_000);

    // Advancing time increases game_time but freezes day_time
    time.advance(500);
    assert_eq!(time.game_time(), 600);
    assert_eq!(time.day_time(), 6_000);
    assert_eq!(time.packet_day_time(), -6_000);
}

#[test]
fn test_weather_state_transitions_and_gradient() {
    let mut weather = WeatherState::from_kind(WeatherKind::Clear, 100);
    assert_eq!(weather.kind(), WeatherKind::Clear);
    assert_eq!(weather.rain_level(), 0.0);
    assert_eq!(weather.thunder_level(), 0.0);

    // Set weather to rain
    weather.set_weather(WeatherKind::Rain, 500);
    assert_eq!(weather.kind(), WeatherKind::Rain);

    // After 10 ticks, rain level ramps up smoothly
    for _ in 0..10 {
        weather.tick();
    }
    assert_eq!(weather.remaining_duration(), 490);
    assert!((weather.rain_level() - 0.1).abs() < 0.001);
    assert_eq!(weather.thunder_level(), 0.0);

    // After 100 more ticks, rain level saturates at 1.0
    for _ in 0..100 {
        weather.tick();
    }
    assert_eq!(weather.rain_level(), 1.0);
}

#[test]
fn test_weather_cycle_advances_past_each_expiry() {
    let schedule = WeatherSchedule {
        clear_ticks: 3,
        rain_ticks: 2,
        thunder_ticks: 4,
    };
    let mut weather = WeatherState::from_kind(WeatherKind::Clear, 1).with_schedule(schedule);

    let mut kinds = Vec::new();
    for _ in 0..10 {
        weather.tick();
        kinds.push(weather.kind());
    }
    use WeatherKind::{Clear, Rain, Thunder};
    assert_eq!(
        kinds,
        [
            Rain, Rain, Thunder, Thunder, Thunder, Thunder, Clear, Clear, Clear, Rain
        ]
    );
}

#[test]
fn test_finite_weather_expires_and_starts_the_next_interval() {
    let mut rain = WeatherState::from_kind(WeatherKind::Rain, 1);
    rain.tick();
    assert_eq!(rain.kind(), WeatherKind::Thunder);
    assert_eq!(rain.remaining_duration(), WeatherSchedule::DEFAULT_TICKS);

    let mut clear = WeatherState::new();
    for _ in 0..WeatherSchedule::DEFAULT_TICKS {
        clear.tick();
    }
    assert_eq!(clear.kind(), WeatherKind::Rain);
    assert_eq!(clear.remaining_duration(), WeatherSchedule::DEFAULT_TICKS);
}

#[test]
fn test_zero_duration_expires_on_the_next_tick() {
    let mut weather = WeatherState::from_kind(WeatherKind::Clear, 0);
    weather.tick();
    assert_eq!(weather.kind(), WeatherKind::Rain);
}
