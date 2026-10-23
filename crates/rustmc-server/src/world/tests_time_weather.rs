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
    assert_eq!(time.game_time(), 24_000);
    assert!(time.is_day());
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
    weather.set_weather(WeatherKind::Rain, 50);
    assert_eq!(weather.kind(), WeatherKind::Rain);

    // After 10 ticks, rain level ramps up smoothly
    for _ in 0..10 {
        weather.tick();
    }
    assert_eq!(weather.remaining_duration(), 40);
    assert!((weather.rain_level() - 0.1).abs() < 0.001);
    assert_eq!(weather.thunder_level(), 0.0);

    // After 100 more ticks, rain level saturates at 1.0
    for _ in 0..100 {
        weather.tick();
    }
    assert_eq!(weather.rain_level(), 1.0);
}
