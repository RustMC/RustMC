use super::{DamageEvent, DamageSource, PlayerSurvival};

#[test]
fn test_fall_damage_threshold_and_scaling() {
    let mut player = PlayerSurvival::default();
    assert_eq!(player.health, 20.0);
    assert_eq!(player.fall_distance, 0.0);

    // Falling <= 3.0 blocks produces no damage
    let event = player.handle_fall(true, 0.0);
    assert_eq!(event, None);
    assert_eq!(player.health, 20.0);
    assert_eq!(player.fall_distance, 0.0);

    let event = player.handle_fall(true, 2.5);
    assert_eq!(event, None);
    assert_eq!(player.health, 20.0);
    assert_eq!(player.fall_distance, 0.0);

    let event = player.handle_fall(true, 3.0);
    assert_eq!(event, None);
    assert_eq!(player.health, 20.0);
    assert_eq!(player.fall_distance, 0.0);

    // Falling > 3.0 blocks scales as (fall_distance - 3.0)
    // 4.0 blocks -> 1.0 damage
    let event = player.handle_fall(true, 4.0);
    assert_eq!(
        event,
        Some(DamageEvent {
            source: DamageSource::Fall,
            amount: 1.0,
        })
    );
    assert_eq!(player.health, 19.0);
    assert_eq!(player.fall_distance, 0.0);

    // 5.5 blocks -> 2.5 damage
    let event = player.handle_fall(true, 5.5);
    assert_eq!(
        event,
        Some(DamageEvent {
            source: DamageSource::Fall,
            amount: 2.5,
        })
    );
    assert_eq!(player.health, 16.5);
    assert_eq!(player.fall_distance, 0.0);

    // Mid-air tracking: on_ground = false does not inflict damage and tracks fall_distance
    let event = player.handle_fall(false, 6.0);
    assert_eq!(event, None);
    assert_eq!(player.fall_distance, 6.0);
    assert_eq!(player.health, 16.5);

    // Landing on ground without specifying fall_distance uses accumulated fall_distance
    let event = player.handle_fall(true, 0.0);
    assert_eq!(
        event,
        Some(DamageEvent {
            source: DamageSource::Fall,
            amount: 3.0,
        })
    );
    assert_eq!(player.health, 13.5);
    assert_eq!(player.fall_distance, 0.0);
}

#[test]
fn test_air_depletion_and_drowning_damage() {
    let mut player = PlayerSurvival::default();
    assert_eq!(player.air_supply, 300);

    // In water, air depletes by 1 per tick
    let events = player.tick(true, false, 0.0);
    assert!(events.is_empty());
    assert_eq!(player.air_supply, 299);

    player.tick(true, false, 0.0);
    assert_eq!(player.air_supply, 298);

    // Out of water, air supply refills to 300
    let events = player.tick(false, false, 0.0);
    assert!(events.is_empty());
    assert_eq!(player.air_supply, 300);

    // When air reaches <= 0, drowning damage occurs every 20 ticks.
    // Set food_level below 18 so natural regeneration does not counteract drowning.
    player.air_supply = 0;
    player.food_level = 10;
    player.saturation = 0.0;
    for tick in 1..=19 {
        let events = player.tick(true, false, 0.0);
        assert!(
            events.is_empty(),
            "tick {tick} should not deal drowning damage yet"
        );
        assert_eq!(player.air_supply, -tick);
    }

    // Tick 20: drowning damage applied
    let events = player.tick(true, false, 0.0);
    assert_eq!(
        events,
        vec![DamageEvent {
            source: DamageSource::Drowning,
            amount: 2.0,
        }]
    );
    assert_eq!(player.health, 18.0);
    assert_eq!(player.air_supply, 0);

    // Next 19 ticks no damage
    for _ in 1..=19 {
        let events = player.tick(true, false, 0.0);
        assert!(events.is_empty());
    }

    // Tick 40: another 2.0 drowning damage applied
    let events = player.tick(true, false, 0.0);
    assert_eq!(
        events,
        vec![DamageEvent {
            source: DamageSource::Drowning,
            amount: 2.0,
        }]
    );
    assert_eq!(player.health, 16.0);
    assert_eq!(player.air_supply, 0);

    // Leaving water refills air immediately
    player.tick(false, false, 0.0);
    assert_eq!(player.air_supply, 300);
}

#[test]
fn test_void_damage_below_bottom_y() {
    let mut player = PlayerSurvival::default();
    assert_eq!(player.health, 20.0);

    // Above or at y = -64.0, no void damage
    let events = player.tick(false, false, 0.0);
    assert!(events.is_empty());
    assert_eq!(player.health, 20.0);

    let events = player.tick(false, false, -64.0);
    assert!(events.is_empty());
    assert_eq!(player.health, 20.0);

    // Below -64.0, takes 4.0 void damage per tick
    let events = player.tick(false, false, -64.1);
    assert_eq!(
        events,
        vec![DamageEvent {
            source: DamageSource::Void,
            amount: 4.0,
        }]
    );
    assert_eq!(player.health, 16.0);

    let events = player.tick(false, false, -70.0);
    assert_eq!(
        events,
        vec![DamageEvent {
            source: DamageSource::Void,
            amount: 4.0,
        }]
    );
    assert_eq!(player.health, 12.0);

    // Continue until death
    player.tick(false, false, -70.0); // 8.0
    player.tick(false, false, -70.0); // 4.0
    let events = player.tick(false, false, -70.0); // 0.0 (death)
    assert_eq!(
        events,
        vec![DamageEvent {
            source: DamageSource::Void,
            amount: 4.0,
        }]
    );
    assert_eq!(player.health, 0.0);
    assert!(!player.is_alive());

    // Once dead, no further damage events
    let events = player.tick(false, false, -70.0);
    assert!(events.is_empty());
}

#[test]
fn test_exhaustion_depletes_saturation_then_food() {
    let mut player = PlayerSurvival::default();
    assert_eq!(player.food_level, 20);
    assert_eq!(player.saturation, 5.0);
    assert_eq!(player.exhaustion, 0.0);

    // Adding less than 4.0 exhaustion accumulates without depleting saturation
    player.add_exhaustion(2.5);
    assert_eq!(player.exhaustion, 2.5);
    assert_eq!(player.saturation, 5.0);
    assert_eq!(player.food_level, 20);

    // Crossing 4.0 boundary wraps exhaustion and depletes 1.0 saturation
    player.add_exhaustion(1.5);
    assert_eq!(player.exhaustion, 0.0);
    assert_eq!(player.saturation, 4.0);
    assert_eq!(player.food_level, 20);

    // Adding 6.0 exhaustion wraps by 4.0 (exhaustion = 2.0) and depletes 1.0 saturation
    player.add_exhaustion(6.0);
    assert_eq!(player.exhaustion, 2.0);
    assert_eq!(player.saturation, 3.0);
    assert_eq!(player.food_level, 20);

    // Deplete remaining saturation (3.0 saturation left, need 3 wraps = 10.0 more exhaustion)
    player.add_exhaustion(10.0); // 2.0 + 10.0 = 12.0 -> 3 wraps -> exhaustion = 0.0, saturation = 0.0
    assert_eq!(player.exhaustion, 0.0);
    assert_eq!(player.saturation, 0.0);
    assert_eq!(player.food_level, 20);

    // Once saturation is 0.0, exhaustion reduces food_level
    player.add_exhaustion(4.0);
    assert_eq!(player.exhaustion, 0.0);
    assert_eq!(player.saturation, 0.0);
    assert_eq!(player.food_level, 19);

    player.add_exhaustion(8.0); // 2 wraps
    assert_eq!(player.exhaustion, 0.0);
    assert_eq!(player.food_level, 17);

    // Exhaustion cannot reduce food_level below 0
    player.add_exhaustion(100.0);
    assert_eq!(player.food_level, 0);
    assert_eq!(player.saturation, 0.0);
    assert!(player.exhaustion >= 0.0 && player.exhaustion <= 4.0);
}

#[test]
fn test_natural_healing_from_food() {
    let mut player = PlayerSurvival {
        health: 10.0,
        ..Default::default()
    };
    player.food_level = 20;
    player.saturation = 5.0;

    // Fast heal: at food 20 & saturation > 0, heals 1.0 every 10 ticks
    for _ in 1..=9 {
        let events = player.tick(false, false, 0.0);
        assert!(events.is_empty());
        assert_eq!(player.health, 10.0);
    }

    // Tick 10: heals 1.0
    player.tick(false, false, 0.0);
    assert_eq!(player.health, 11.0);
    // Regeneration added 6.0 exhaustion, which depleted saturation from 5.0 to 4.0 (and 2.0 wrapped exhaustion)
    assert_eq!(player.saturation, 4.0);
    assert_eq!(player.exhaustion, 2.0);

    // Standard heal: food >= 18 but saturation == 0.0
    player.health = 15.0;
    player.food_level = 18;
    player.saturation = 0.0;
    player.exhaustion = 0.0;
    player.food_timer = 0;

    for _ in 1..=79 {
        player.tick(false, false, 0.0);
        assert_eq!(player.health, 15.0);
    }

    // Tick 80: heals 1.0
    player.tick(false, false, 0.0);
    assert_eq!(player.health, 16.0);
    // Exhaustion added reduced food_level from 18 to 17
    assert_eq!(player.food_level, 17);

    // When food < 18, no healing occurs
    for _ in 0..100 {
        player.tick(false, false, 0.0);
    }
    assert_eq!(player.health, 16.0);

    // Full health does not deplete food or saturation
    player.health = 20.0;
    player.food_level = 20;
    player.saturation = 5.0;
    player.exhaustion = 0.0;
    for _ in 0..100 {
        player.tick(false, false, 0.0);
    }
    assert_eq!(player.health, 20.0);
    assert_eq!(player.food_level, 20);
    assert_eq!(player.saturation, 5.0);
}

#[test]
fn test_starvation_damage() {
    let mut player = PlayerSurvival {
        health: 20.0,
        food_level: 0,
        saturation: 0.0,
        ..Default::default()
    };

    // Ticks 1..=79: no starvation damage
    for tick in 1..=79 {
        let events = player.tick(false, false, 0.0);
        assert!(
            events.is_empty(),
            "tick {tick} should not deal starvation damage yet"
        );
        assert_eq!(player.health, 20.0);
    }

    // Tick 80: takes 1.0 starvation damage
    let events = player.tick(false, false, 0.0);
    assert_eq!(
        events,
        vec![DamageEvent {
            source: DamageSource::Starvation,
            amount: 1.0,
        }]
    );
    assert_eq!(player.health, 19.0);

    // Next 79 ticks: no damage
    for _ in 1..=79 {
        let events = player.tick(false, false, 0.0);
        assert!(events.is_empty());
    }

    // Tick 160: another 1.0 starvation damage
    let events = player.tick(false, false, 0.0);
    assert_eq!(
        events,
        vec![DamageEvent {
            source: DamageSource::Starvation,
            amount: 1.0,
        }]
    );
    assert_eq!(player.health, 18.0);

    // If food is restored, starvation stops
    player.food_level = 5;
    for _ in 0..100 {
        let events = player.tick(false, false, 0.0);
        assert!(events.is_empty());
    }
    assert_eq!(player.health, 18.0);
}

#[test]
fn test_suffocation_damage() {
    let mut player = PlayerSurvival::default();
    assert_eq!(player.health, 20.0);

    for _ in 1..=19 {
        let events = player.tick(false, true, 0.0);
        assert!(events.is_empty());
        assert_eq!(player.health, 20.0);
    }

    let events = player.tick(false, true, 0.0);
    assert_eq!(
        events,
        vec![DamageEvent {
            source: DamageSource::Suffocation,
            amount: 1.0,
        }]
    );
    assert_eq!(player.health, 19.0);

    // If no longer suffocating, timer resets
    player.tick(false, false, 0.0);
    assert_eq!(player.suffocation_timer, 0);
}

#[test]
fn test_apply_damage_and_heal_clamping() {
    let mut player = PlayerSurvival::default();
    assert_eq!(player.health, 20.0);

    // Damage returns actual dealt
    let dealt = player.apply_damage(DamageSource::Fire, 5.0);
    assert_eq!(dealt, 5.0);
    assert_eq!(player.health, 15.0);

    // Heal clamps to 20.0
    player.heal(10.0);
    assert_eq!(player.health, 20.0);

    // Fatal damage clamps to 0.0
    let dealt = player.apply_damage(DamageSource::Lava, 30.0);
    assert_eq!(dealt, 20.0);
    assert_eq!(player.health, 0.0);
    assert!(!player.is_alive());

    // Dead player cannot heal or take damage
    player.heal(10.0);
    assert_eq!(player.health, 0.0);
    let dealt = player.apply_damage(DamageSource::Fire, 5.0);
    assert_eq!(dealt, 0.0);
}
