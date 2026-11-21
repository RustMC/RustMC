//! Unit tests for the block-play geometry rules.

use super::*;

#[test]
fn neighbours_follow_the_protocol_face_order() {
    let pos = (10, 64, -3);
    assert_eq!(neighbour(pos, 0), Some((10, 63, -3)));
    assert_eq!(neighbour(pos, 1), Some((10, 65, -3)));
    assert_eq!(neighbour(pos, 2), Some((10, 64, -4)));
    assert_eq!(neighbour(pos, 3), Some((10, 64, -2)));
    assert_eq!(neighbour(pos, 4), Some((9, 64, -3)));
    assert_eq!(neighbour(pos, 5), Some((11, 64, -3)));
    assert_eq!(neighbour(pos, 6), None);
}

#[test]
fn reach_is_longer_in_creative_and_measured_from_the_eyes() {
    let feet = [0.5, 64.0, 0.5];
    // Block centre about 5.7 blocks from the eyes: beyond survival reach
    // (4.5 + 1.0 allowance), within creative reach (5.0 + 1.0).
    let target = (5, 67, 2);
    assert!(!within_reach(feet, GameMode::Survival, target));
    assert!(within_reach(feet, GameMode::Creative, target));
    assert!(within_reach(feet, GameMode::Survival, (0, 63, 0)));
    assert!(!within_reach(feet, GameMode::Creative, (0, 50, 0)));
}

#[test]
fn a_player_occupies_the_two_cells_of_their_hitbox_only() {
    let feet = [0.5, 64.0, 0.5];
    assert!(overlaps(feet, (0, 64, 0)));
    assert!(overlaps(feet, (0, 65, 0)));
    assert!(!overlaps(feet, (0, 66, 0)));
    assert!(!overlaps(feet, (0, 63, 0)));
    assert!(!overlaps(feet, (1, 64, 0)));
    // Standing on a cell edge touches both neighbours.
    assert!(overlaps([1.0, 64.0, 0.5], (1, 64, 0)));
}
