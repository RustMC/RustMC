use super::experience::*;

#[test]
fn test_xp_per_level_thresholds() {
    // Level 0: 2 * 0 + 7 = 7 XP
    assert_eq!(xp_for_next_level(0), 7);
    // Level 1: 2 * 1 + 7 = 9 XP
    assert_eq!(xp_for_next_level(1), 9);
    // Level 15 (boundary): 2 * 15 + 7 = 37 XP
    assert_eq!(xp_for_next_level(15), 37);
    // Level 16: 5 * 16 - 38 = 42 XP
    assert_eq!(xp_for_next_level(16), 42);
    // Level 30 (boundary): 5 * 30 - 38 = 112 XP
    assert_eq!(xp_for_next_level(30), 112);
    // Level 31: 9 * 31 - 158 = 121 XP
    assert_eq!(xp_for_next_level(31), 121);
}

#[test]
fn test_total_xp_for_level_exact_curve() {
    // Level 0: 0
    assert_eq!(total_xp_for_level(0), 0);
    // Level 1: 1^2 + 6*1 = 7
    assert_eq!(total_xp_for_level(1), 7);
    // Level 7: 7^2 + 6*7 = 91
    assert_eq!(total_xp_for_level(7), 91);
    // Level 16: 16^2 + 6*16 = 352
    assert_eq!(total_xp_for_level(16), 352);
    // Level 30: (5*900 - 81*30 + 720) / 2 = (4500 - 2430 + 720) / 2 = 2790 / 2 = 1395
    assert_eq!(total_xp_for_level(30), 1395);
}

#[test]
fn test_player_experience_progression() {
    let mut exp = PlayerExperience::new();
    assert_eq!(exp.level(), 0);
    assert_eq!(exp.progress(), 0.0);
    assert_eq!(exp.total_xp(), 0);

    // Add 7 XP -> reaches Level 1, progress 0.0
    exp.add_experience(7);
    assert_eq!(exp.level(), 1);
    assert_eq!(exp.total_xp(), 7);
    assert!(exp.progress() < 0.001);

    // Add 4.5 XP into level 1 (needs 9 XP) -> progress ~ 0.5
    exp.add_experience(4);
    assert_eq!(exp.level(), 1);
    assert_eq!(exp.total_xp(), 11);
    assert!((exp.progress() - 4.0 / 9.0).abs() < 0.01);
}

#[test]
fn test_consume_levels_and_xp() {
    let mut exp = PlayerExperience::from_total_xp(1395); // Exactly level 30
    assert_eq!(exp.level(), 30);

    // Consume 3 levels (e.g. tier 3 enchantment) -> level 27
    exp.consume_levels(3).unwrap();
    assert_eq!(exp.level(), 27);

    // Cannot consume more levels than available
    let err = exp.consume_levels(50).unwrap_err();
    assert_eq!(
        err,
        ExperienceError::InsufficientLevels {
            required: 50,
            available: 27
        }
    );
}

#[test]
fn test_death_dropped_xp_cap() {
    // Level 5: 5 * 7 = 35 XP
    let exp5 = PlayerExperience::from_total_xp(total_xp_for_level(5));
    assert_eq!(exp5.calculate_death_dropped_xp(), 35);

    // Level 30: 30 * 7 = 210, but capped at 100 in vanilla Java Edition!
    let exp30 = PlayerExperience::from_total_xp(total_xp_for_level(30));
    assert_eq!(exp30.calculate_death_dropped_xp(), 100);

    // Level 100: also capped at 100
    let exp100 = PlayerExperience::from_total_xp(total_xp_for_level(100));
    assert_eq!(exp100.calculate_death_dropped_xp(), 100);
}
