use super::interaction::{
    BlockInteractionEngine, DiggingAction, DiggingStatus, Direction, InteractionRejectReason,
    InteractionResult, UseItemOnAction,
};

#[test]
fn test_reach_distance_guard() {
    let target = (0, 60, 0);

    // Target center is (0.5, 60.5, 0.5).
    // Eye at (0.5, 64.5, 0.5) gives distance 4.0 <= 4.5.
    let close_eye = (0.5, 64.5, 0.5);

    let dig_close = DiggingAction {
        status: DiggingStatus::Started,
        pos: target,
        face: Direction::Up,
        sequence: 1,
    };
    let res_dig_close = BlockInteractionEngine::validate_digging(
        close_eye,
        true,
        &dig_close,
        Some("minecraft:stone"),
    );
    assert_eq!(
        res_dig_close,
        InteractionResult::Accepted {
            placed: None,
            broken: Some(target),
            sequence: 1,
        }
    );

    let place_close = UseItemOnAction {
        pos: target,
        face: Direction::Up,
        cursor: (0.5, 0.5, 0.5),
        inside_block: false,
        sequence: 2,
    };
    let player_box = ((10.0, 60.0, 10.0), (10.6, 61.8, 10.6));
    let res_place_close = BlockInteractionEngine::validate_block_placement(
        (10.3, 60.0, 10.3),
        close_eye,
        player_box,
        &place_close,
        false,
        true,
    );
    assert_eq!(
        res_place_close,
        InteractionResult::Accepted {
            placed: Some((0, 61, 0)),
            broken: None,
            sequence: 2,
        }
    );

    // Eye at (0.5, 65.0, 0.5) gives distance exactly 4.5 <= 4.5.
    let exact_eye = (0.5, 65.0, 0.5);
    let dig_exact = DiggingAction {
        status: DiggingStatus::Started,
        pos: target,
        face: Direction::Up,
        sequence: 3,
    };
    let res_dig_exact = BlockInteractionEngine::validate_digging(
        exact_eye,
        true,
        &dig_exact,
        Some("minecraft:stone"),
    );
    assert_eq!(
        res_dig_exact,
        InteractionResult::Accepted {
            placed: None,
            broken: Some(target),
            sequence: 3,
        }
    );

    // Eye at (0.5, 65.1, 0.5) gives distance 4.6 > 4.5.
    let far_eye = (0.5, 65.1, 0.5);
    let dig_far = DiggingAction {
        status: DiggingStatus::Started,
        pos: target,
        face: Direction::Up,
        sequence: 4,
    };
    let res_dig_far =
        BlockInteractionEngine::validate_digging(far_eye, true, &dig_far, Some("minecraft:stone"));
    assert_eq!(
        res_dig_far,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::TooFar,
            sequence: 4,
        }
    );

    let place_far = UseItemOnAction {
        pos: target,
        face: Direction::Up,
        cursor: (0.5, 0.5, 0.5),
        inside_block: false,
        sequence: 5,
    };
    let res_place_far = BlockInteractionEngine::validate_block_placement(
        (10.3, 60.0, 10.3),
        far_eye,
        player_box,
        &place_far,
        false,
        true,
    );
    assert_eq!(
        res_place_far,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::TooFar,
            sequence: 5,
        }
    );

    // 3D diagonal distance checks.
    // Offset (2, 2, 2) from center gives distance squared = 12.0 <= 20.25 (dist ~ 3.46 <= 4.5).
    let diagonal_valid_eye = (2.5, 62.5, 2.5);
    let res_diag_valid = BlockInteractionEngine::validate_digging(
        diagonal_valid_eye,
        true,
        &dig_close,
        Some("minecraft:stone"),
    );
    assert!(res_diag_valid.is_accepted());

    // Offset (3, 3, 3) from center gives distance squared = 27.0 > 20.25 (dist ~ 5.20 > 4.5).
    let diagonal_far_eye = (3.5, 63.5, 3.5);
    let res_diag_far = BlockInteractionEngine::validate_digging(
        diagonal_far_eye,
        true,
        &dig_close,
        Some("minecraft:stone"),
    );
    assert_eq!(
        res_diag_far,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::TooFar,
            sequence: 1,
        }
    );
}

#[test]
fn test_creative_instant_break() {
    let eye = (5.5, 66.0, 5.5);
    let target = (5, 64, 5);

    // Creative mode: DiggingStatus::Started immediately breaks the block.
    let creative_start = DiggingAction {
        status: DiggingStatus::Started,
        pos: target,
        face: Direction::Up,
        sequence: 10,
    };
    let res_creative = BlockInteractionEngine::validate_digging(
        eye,
        true,
        &creative_start,
        Some("minecraft:stone"),
    );
    assert_eq!(
        res_creative,
        InteractionResult::Accepted {
            placed: None,
            broken: Some(target),
            sequence: 10,
        }
    );

    // Creative mode: can break unbreakable bedrock instantly as well.
    let res_bedrock_creative = BlockInteractionEngine::validate_digging(
        eye,
        true,
        &creative_start,
        Some("minecraft:bedrock"),
    );
    assert_eq!(
        res_bedrock_creative,
        InteractionResult::Accepted {
            placed: None,
            broken: Some(target),
            sequence: 10,
        }
    );

    // Creative mode: Cancelled does not break.
    let creative_cancel = DiggingAction {
        status: DiggingStatus::Cancelled,
        pos: target,
        face: Direction::Up,
        sequence: 11,
    };
    let res_cancel = BlockInteractionEngine::validate_digging(
        eye,
        true,
        &creative_cancel,
        Some("minecraft:stone"),
    );
    assert_eq!(
        res_cancel,
        InteractionResult::Accepted {
            placed: None,
            broken: None,
            sequence: 11,
        }
    );

    // Survival mode: DiggingStatus::Started does not instantly break.
    let survival_start = DiggingAction {
        status: DiggingStatus::Started,
        pos: target,
        face: Direction::Up,
        sequence: 20,
    };
    let res_survival_start = BlockInteractionEngine::validate_digging(
        eye,
        false,
        &survival_start,
        Some("minecraft:stone"),
    );
    assert_eq!(
        res_survival_start,
        InteractionResult::Accepted {
            placed: None,
            broken: None,
            sequence: 20,
        }
    );

    // Survival mode: DiggingStatus::Finished breaks the block.
    let survival_finish = DiggingAction {
        status: DiggingStatus::Finished,
        pos: target,
        face: Direction::Up,
        sequence: 21,
    };
    let res_survival_finish = BlockInteractionEngine::validate_digging(
        eye,
        false,
        &survival_finish,
        Some("minecraft:stone"),
    );
    assert_eq!(
        res_survival_finish,
        InteractionResult::Accepted {
            placed: None,
            broken: Some(target),
            sequence: 21,
        }
    );

    // Survival mode: Finished on bedrock is rejected (unbreakable).
    let res_bedrock_survival = BlockInteractionEngine::validate_digging(
        eye,
        false,
        &survival_finish,
        Some("minecraft:bedrock"),
    );
    assert_eq!(
        res_bedrock_survival,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::TargetNotReplaceable,
            sequence: 21,
        }
    );
}

#[test]
fn test_placement_face_offset_calculation() {
    let eye = (10.5, 66.0, 10.5);
    let target = (10, 64, 10);
    let player_box = ((20.0, 64.0, 20.0), (20.6, 65.8, 20.6));

    // Direction offset tests when target is not replaceable and candidate is empty.
    let test_cases = [
        (Direction::Down, (10, 63, 10)),
        (Direction::Up, (10, 65, 10)),
        (Direction::North, (10, 64, 9)),
        (Direction::South, (10, 64, 11)),
        (Direction::West, (9, 64, 10)),
        (Direction::East, (11, 64, 10)),
    ];

    for (face, expected_offset) in test_cases {
        assert_eq!(face.offset(target.0, target.1, target.2), expected_offset);

        let action = UseItemOnAction {
            pos: target,
            face,
            cursor: (0.5, 0.5, 0.5),
            inside_block: false,
            sequence: 30,
        };

        let result = BlockInteractionEngine::validate_block_placement(
            (20.3, 64.0, 20.3),
            eye,
            player_box,
            &action,
            false,
            true,
        );

        assert_eq!(
            result,
            InteractionResult::Accepted {
                placed: Some(expected_offset),
                broken: None,
                sequence: 30,
            }
        );
    }

    // When target_is_replaceable is true, placement occupies the target position directly.
    let action_replace = UseItemOnAction {
        pos: target,
        face: Direction::Up,
        cursor: (0.5, 0.5, 0.5),
        inside_block: false,
        sequence: 31,
    };
    let result_replace = BlockInteractionEngine::validate_block_placement(
        (20.3, 64.0, 20.3),
        eye,
        player_box,
        &action_replace,
        true,
        false,
    );
    assert_eq!(
        result_replace,
        InteractionResult::Accepted {
            placed: Some(target),
            broken: None,
            sequence: 31,
        }
    );

    // When target_is_replaceable is false and candidate_is_empty is false, placement is rejected.
    let result_occupied = BlockInteractionEngine::validate_block_placement(
        (20.3, 64.0, 20.3),
        eye,
        player_box,
        &action_replace,
        false,
        false,
    );
    assert_eq!(
        result_occupied,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::TargetNotReplaceable,
            sequence: 31,
        }
    );
}

#[test]
fn test_placement_inside_player_hitbox_rejected() {
    let player_pos = (0.5, 65.0, 0.5);
    let player_eye = (0.5, 66.62, 0.5);
    // Standard player hitbox: 0.6 wide, 1.8 tall, centered at (0.5, 65.0, 0.5).
    let player_box = ((0.2, 65.0, 0.2), (0.8, 66.8, 0.8));

    // Placing a block at (0, 65, 0) occupies [0.0, 1.0] x [65.0, 66.0] x [0.0, 1.0],
    // which intersects the player hitbox.
    let place_inside = UseItemOnAction {
        pos: (0, 64, 0),
        face: Direction::Up,
        cursor: (0.5, 0.5, 0.5),
        inside_block: false,
        sequence: 40,
    };
    let res_inside = BlockInteractionEngine::validate_block_placement(
        player_pos,
        player_eye,
        player_box,
        &place_inside,
        false,
        true,
    );
    assert_eq!(
        res_inside,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::InsidePlayer,
            sequence: 40,
        }
    );

    // Placing a block at (0, 64, 0) directly (replaceable target below feet).
    // Occupies [0.0, 1.0] x [64.0, 65.0] x [0.0, 1.0].
    // Touches feet at Y = 65.0, does not intersect player interior.
    let place_below = UseItemOnAction {
        pos: (0, 64, 0),
        face: Direction::Up,
        cursor: (0.5, 0.5, 0.5),
        inside_block: false,
        sequence: 41,
    };
    let res_below = BlockInteractionEngine::validate_block_placement(
        player_pos,
        player_eye,
        player_box,
        &place_below,
        true,
        false,
    );
    assert_eq!(
        res_below,
        InteractionResult::Accepted {
            placed: Some((0, 64, 0)),
            broken: None,
            sequence: 41,
        }
    );

    // Placing adjacent to player at (1, 65, 0): [1.0, 2.0] x [65.0, 66.0] x [0.0, 1.0].
    // Does not intersect player hitbox [0.2, 0.8] in X.
    let place_adjacent = UseItemOnAction {
        pos: (0, 65, 0),
        face: Direction::East,
        cursor: (0.5, 0.5, 0.5),
        inside_block: false,
        sequence: 42,
    };
    let res_adjacent = BlockInteractionEngine::validate_block_placement(
        player_pos,
        player_eye,
        player_box,
        &place_adjacent,
        false,
        true,
    );
    assert_eq!(
        res_adjacent,
        InteractionResult::Accepted {
            placed: Some((1, 65, 0)),
            broken: None,
            sequence: 42,
        }
    );
}

#[test]
fn test_out_of_bounds_interaction_rejected() {
    let player_box = ((10.0, 64.0, 10.0), (10.6, 65.8, 10.6));

    // Digging below min Y (-64)
    let dig_below_min_y = DiggingAction {
        status: DiggingStatus::Started,
        pos: (0, -65, 0),
        face: Direction::Up,
        sequence: 50,
    };
    let res_dig_below = BlockInteractionEngine::validate_digging(
        (0.5, -64.0, 0.5),
        true,
        &dig_below_min_y,
        Some("minecraft:stone"),
    );
    assert_eq!(
        res_dig_below,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::OutOfBounds,
            sequence: 50,
        }
    );

    // Digging above max Y (319)
    let dig_above_max_y = DiggingAction {
        status: DiggingStatus::Started,
        pos: (0, 320, 0),
        face: Direction::Down,
        sequence: 51,
    };
    let res_dig_above = BlockInteractionEngine::validate_digging(
        (0.5, 319.0, 0.5),
        true,
        &dig_above_max_y,
        Some("minecraft:stone"),
    );
    assert_eq!(
        res_dig_above,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::OutOfBounds,
            sequence: 51,
        }
    );

    // Digging beyond world border
    let dig_border = DiggingAction {
        status: DiggingStatus::Started,
        pos: (30_000_001, 64, 0),
        face: Direction::North,
        sequence: 52,
    };
    let res_border = BlockInteractionEngine::validate_digging(
        (30_000_001.0, 64.5, 0.5),
        true,
        &dig_border,
        Some("minecraft:stone"),
    );
    assert_eq!(
        res_border,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::OutOfBounds,
            sequence: 52,
        }
    );

    // Placing against top face at max Y (319) -> candidate is at 320 (out of bounds)
    let place_at_top = UseItemOnAction {
        pos: (0, 319, 0),
        face: Direction::Up,
        cursor: (0.5, 0.5, 0.5),
        inside_block: false,
        sequence: 53,
    };
    let res_place_top = BlockInteractionEngine::validate_block_placement(
        (10.0, 64.0, 10.0),
        (0.5, 318.5, 0.5),
        player_box,
        &place_at_top,
        false,
        true,
    );
    assert_eq!(
        res_place_top,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::OutOfBounds,
            sequence: 53,
        }
    );

    // Placing against bottom face at min Y (-64) -> candidate is at -65 (out of bounds)
    let place_at_bottom = UseItemOnAction {
        pos: (0, -64, 0),
        face: Direction::Down,
        cursor: (0.5, 0.5, 0.5),
        inside_block: false,
        sequence: 54,
    };
    let res_place_bottom = BlockInteractionEngine::validate_block_placement(
        (10.0, 64.0, 10.0),
        (0.5, -63.5, 0.5),
        player_box,
        &place_at_bottom,
        false,
        true,
    );
    assert_eq!(
        res_place_bottom,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::OutOfBounds,
            sequence: 54,
        }
    );

    // Boundary edge cases that are VALID:
    // Digging at min Y (-64)
    let dig_at_min_y = DiggingAction {
        status: DiggingStatus::Started,
        pos: (0, -64, 0),
        face: Direction::Up,
        sequence: 55,
    };
    let res_at_min = BlockInteractionEngine::validate_digging(
        (0.5, -63.5, 0.5),
        true,
        &dig_at_min_y,
        Some("minecraft:bedrock"),
    );
    assert_eq!(
        res_at_min,
        InteractionResult::Accepted {
            placed: None,
            broken: Some((0, -64, 0)),
            sequence: 55,
        }
    );

    // Placing against top face at min Y (-64) -> placed at -63 (valid)
    let place_on_min_y = UseItemOnAction {
        pos: (0, -64, 0),
        face: Direction::Up,
        cursor: (0.5, 0.5, 0.5),
        inside_block: false,
        sequence: 56,
    };
    let res_on_min = BlockInteractionEngine::validate_block_placement(
        (10.0, 64.0, 10.0),
        (0.5, -63.5, 0.5),
        player_box,
        &place_on_min_y,
        false,
        true,
    );
    assert_eq!(
        res_on_min,
        InteractionResult::Accepted {
            placed: Some((0, -63, 0)),
            broken: None,
            sequence: 56,
        }
    );
}

#[test]
fn test_stateful_survival_digging_progression() {
    let eye = (10.5, 66.0, 10.5);
    let target = (10, 64, 10);

    let start_action = DiggingAction {
        status: DiggingStatus::Started,
        pos: target,
        face: Direction::Up,
        sequence: 100,
    };

    let finish_action = DiggingAction {
        status: DiggingStatus::Finished,
        pos: target,
        face: Direction::Up,
        sequence: 101,
    };

    // 1. Survival player starts digging stone (hardness 1.5 -> 45 ticks)
    let (res_start, session) = BlockInteractionEngine::validate_digging_stateful(
        eye,
        false, // survival
        &start_action,
        Some("minecraft:stone"),
        1000, // current tick
        None,
        false, // do not allow untracked
    );
    assert_eq!(
        res_start,
        InteractionResult::Accepted {
            placed: None,
            broken: None,
            sequence: 100,
        }
    );
    assert!(session.is_some());
    let active_session = session.unwrap();
    assert_eq!(active_session.pos, target);
    assert_eq!(active_session.start_tick, 1000);
    assert_eq!(active_session.required_ticks, 45);

    // 2. Client finishes prematurely at tick 1020 (only 20 ticks elapsed < 45) -> Rejected
    let (res_early, retained_session) = BlockInteractionEngine::validate_digging_stateful(
        eye,
        false,
        &finish_action,
        Some("minecraft:stone"),
        1020,
        Some(&active_session),
        false,
    );
    assert_eq!(
        res_early,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::PrematureFinish,
            sequence: 101,
        }
    );
    assert_eq!(retained_session, Some(active_session));

    // 3. Client finishes after sufficient ticks at tick 1045 (45 ticks elapsed) -> Accepted break
    let (res_valid, completed_session) = BlockInteractionEngine::validate_digging_stateful(
        eye,
        false,
        &finish_action,
        Some("minecraft:stone"),
        1045,
        Some(&active_session),
        false,
    );
    assert_eq!(
        res_valid,
        InteractionResult::Accepted {
            placed: None,
            broken: Some(target),
            sequence: 101,
        }
    );
    assert!(completed_session.is_none());

    // 4. Client attempts Finished without an active session -> Rejected
    let (res_no_session, _) = BlockInteractionEngine::validate_digging_stateful(
        eye,
        false,
        &finish_action,
        Some("minecraft:stone"),
        1045,
        None,
        false,
    );
    assert_eq!(
        res_no_session,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::PrematureFinish,
            sequence: 101,
        }
    );
}

#[test]
fn test_non_collidable_block_placement_inside_player() {
    let player_pos = (10.0, 64.0, 10.0);
    let eye = (10.0, 65.62, 10.0);
    // Player hitbox spanning [9.7..10.3, 64.0..65.8, 9.7..10.3]
    let player_box = ((9.7, 64.0, 9.7), (10.3, 65.8, 10.3));

    // Placing against floor at (10, 63, 10) face UP -> candidate placed block is at (10, 64, 10)
    // which intersects the player hitbox!
    let place_action = UseItemOnAction {
        pos: (10, 63, 10),
        face: Direction::Up,
        cursor: (0.5, 1.0, 0.5),
        inside_block: false,
        sequence: 200,
    };

    // Case 1: Solid block (e.g. stone) placed inside player hitbox must be rejected
    let res_stone = BlockInteractionEngine::validate_block_placement_with_block(
        player_pos,
        eye,
        player_box,
        &place_action,
        false,
        true,
        Some("minecraft:stone"),
    );
    assert_eq!(
        res_stone,
        InteractionResult::Rejected {
            reason: InteractionRejectReason::InsidePlayer,
            sequence: 200,
        }
    );

    // Case 2: Non-collidable block (e.g. torch, short_grass) placed inside player hitbox must be ACCEPTED
    let res_torch = BlockInteractionEngine::validate_block_placement_with_block(
        player_pos,
        eye,
        player_box,
        &place_action,
        false,
        true,
        Some("minecraft:torch"),
    );
    assert_eq!(
        res_torch,
        InteractionResult::Accepted {
            placed: Some((10, 64, 10)),
            broken: None,
            sequence: 200,
        }
    );

    let res_grass = BlockInteractionEngine::validate_block_placement_with_block(
        player_pos,
        eye,
        player_box,
        &place_action,
        false,
        true,
        Some("minecraft:short_grass"),
    );
    assert_eq!(
        res_grass,
        InteractionResult::Accepted {
            placed: Some((10, 64, 10)),
            broken: None,
            sequence: 200,
        }
    );
}
