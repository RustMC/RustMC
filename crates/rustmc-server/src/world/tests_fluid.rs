//! Comprehensive tests for fluid simulation mechanics.

use super::fluid::*;

#[test]
fn test_water_horizontal_spread_and_decay() {
    let mut world = InMemoryFluidWorld::new();
    // Solid floor below at y = -1
    for x in -10..=10 {
        for z in -10..=10 {
            world.set_solid(BlockPos::new(x, -1, z), true);
        }
    }

    // Place water source at (0, 0, 0)
    world.set_fluid(BlockPos::new(0, 0, 0), FluidState::source(FluidType::Water));

    // Initial spread to (1, 0, 0)
    let spread_1 = compute_fluid_spread(&world, BlockPos::new(1, 0, 0));
    assert_eq!(spread_1, Some(FluidState::new(FluidType::Water, 1, false)));
    assert_eq!(
        spread_1.unwrap().to_block_state(),
        "minecraft:water[level=1]"
    );

    // Verify decay by +1 level per block up to max level 7 (max reach 7 blocks)
    for x in 1..=7 {
        world.set_fluid(
            BlockPos::new(x, 0, 0),
            FluidState::new(FluidType::Water, x as u8, false),
        );
        let next_pos = BlockPos::new(x + 1, 0, 0);
        let spread = compute_fluid_spread(&world, next_pos);
        if x < 7 {
            assert_eq!(
                spread,
                Some(FluidState::new(FluidType::Water, (x + 1) as u8, false)),
                "decay at distance {}",
                x + 1
            );
        } else {
            // At distance 8 (x = 8), water cannot spread past max reach 7
            assert_eq!(spread, None, "water should not spread beyond max level 7");
        }
    }
}

#[test]
fn test_infinite_water_source_creation() {
    let mut world = InMemoryFluidWorld::new();
    // Solid floor at y = -1
    for x in 0..=2 {
        for z in 0..=2 {
            world.set_solid(BlockPos::new(x, -1, z), true);
        }
    }

    // Two water sources at (0, 0, 0) and (2, 0, 0)
    world.set_fluid(BlockPos::new(0, 0, 0), FluidState::source(FluidType::Water));
    world.set_fluid(BlockPos::new(2, 0, 0), FluidState::source(FluidType::Water));

    // The cell between them at (1, 0, 0) has solid floor below at (1, -1, 0)
    let middle = BlockPos::new(1, 0, 0);
    let outcome = compute_fluid_spread(&world, middle);
    assert_eq!(
        outcome,
        Some(FluidState::source(FluidType::Water)),
        "cell with >= 2 water source neighbors and solid block below becomes a source"
    );
    assert!(outcome.unwrap().is_source());

    // Negative case 1: Without a solid block below, infinite source must NOT form
    world.set_solid(BlockPos::new(1, -1, 0), false);
    let outcome_no_floor = compute_fluid_spread(&world, middle);
    assert_ne!(
        outcome_no_floor,
        Some(FluidState::source(FluidType::Water)),
        "without solid floor below, infinite water source must not form"
    );

    // Negative case 2: Lava does not generate infinite sources
    let mut lava_world = InMemoryFluidWorld::new();
    for x in 0..=2 {
        lava_world.set_solid(BlockPos::new(x, -1, 0), true);
    }
    lava_world.set_fluid(BlockPos::new(0, 0, 0), FluidState::source(FluidType::Lava));
    lava_world.set_fluid(BlockPos::new(2, 0, 0), FluidState::source(FluidType::Lava));
    let lava_middle = compute_fluid_spread(&lava_world, BlockPos::new(1, 0, 0));
    assert_ne!(
        lava_middle,
        Some(FluidState::source(FluidType::Lava)),
        "lava must not form infinite sources"
    );
}

#[test]
fn test_lava_horizontal_reach_limit() {
    let mut world = InMemoryFluidWorld::new();
    for x in -5..=5 {
        for z in -5..=5 {
            world.set_solid(BlockPos::new(x, -1, z), true);
        }
    }

    // Lava source at (0, 0, 0)
    world.set_fluid(BlockPos::new(0, 0, 0), FluidState::source(FluidType::Lava));

    // Distance 1: (1, 0, 0) gets level 1
    let spread_1 = compute_fluid_spread(&world, BlockPos::new(1, 0, 0));
    assert_eq!(spread_1, Some(FluidState::new(FluidType::Lava, 1, false)));
    assert_eq!(
        spread_1.unwrap().to_block_state(),
        "minecraft:lava[level=1]"
    );

    // Verify decay up to max reach 3 blocks (level 3) in overworld
    for x in 1..=3 {
        world.set_fluid(
            BlockPos::new(x, 0, 0),
            FluidState::new(FluidType::Lava, x as u8, false),
        );
        let next_pos = BlockPos::new(x + 1, 0, 0);
        let spread = compute_fluid_spread(&world, next_pos);
        if x < 3 {
            assert_eq!(
                spread,
                Some(FluidState::new(FluidType::Lava, (x + 1) as u8, false)),
                "lava decay at distance {}",
                x + 1
            );
        } else {
            // Distance 4: exceeds max level 3, does not spread
            assert_eq!(
                spread, None,
                "lava in overworld should not spread beyond max reach 3 blocks (level 3)"
            );
        }
    }
}

#[test]
fn test_fluid_collision_obsidian_and_cobblestone() {
    // Case 1: Water touches lava source (level == 0) -> converts to minecraft:obsidian.
    {
        let mut world = InMemoryFluidWorld::new();
        let lava_pos = BlockPos::new(0, 0, 0);
        let water_pos = BlockPos::new(1, 0, 0);
        world.set_fluid(lava_pos, FluidState::source(FluidType::Lava));
        world.set_fluid(water_pos, FluidState::source(FluidType::Water));

        assert_eq!(
            FluidSimulator::check_collision(&world, lava_pos),
            Some("minecraft:obsidian")
        );
        assert_eq!(
            FluidSimulator::step_cell(&world, lava_pos),
            FlowOutcome::Solidify("minecraft:obsidian")
        );
    }

    // Case 2: Water touches flowing lava (level > 0) -> converts to minecraft:cobblestone.
    {
        let mut world = InMemoryFluidWorld::new();
        let lava_pos = BlockPos::new(0, 0, 0);
        let water_pos = BlockPos::new(1, 0, 0);
        world.set_fluid(lava_pos, FluidState::new(FluidType::Lava, 1, false));
        world.set_fluid(water_pos, FluidState::source(FluidType::Water));

        assert_eq!(
            FluidSimulator::check_collision(&world, lava_pos),
            Some("minecraft:cobblestone")
        );
        assert_eq!(
            FluidSimulator::step_cell(&world, lava_pos),
            FlowOutcome::Solidify("minecraft:cobblestone")
        );
    }

    // Case 3: Lava flows vertically on top of water -> converts to minecraft:stone.
    {
        let mut world = InMemoryFluidWorld::new();
        let water_pos = BlockPos::new(0, 0, 0);
        let lava_above = BlockPos::new(0, 1, 0);
        world.set_fluid(water_pos, FluidState::source(FluidType::Water));
        world.set_fluid(lava_above, FluidState::source(FluidType::Lava));

        assert_eq!(
            FluidSimulator::check_collision(&world, water_pos),
            Some("minecraft:stone")
        );
        assert_eq!(
            FluidSimulator::step_cell(&world, water_pos),
            FlowOutcome::Solidify("minecraft:stone")
        );
    }
}

#[test]
fn test_downward_flow_falling_flag() {
    let mut world = InMemoryFluidWorld::new();
    // Water source at (0, 2, 0)
    let source_pos = BlockPos::new(0, 2, 0);
    world.set_fluid(source_pos, FluidState::source(FluidType::Water));

    // Open air at (0, 1, 0) below source
    let below_pos = BlockPos::new(0, 1, 0);
    let outcome = compute_fluid_spread(&world, below_pos);
    assert_eq!(
        outcome,
        Some(FluidState::new(FluidType::Water, 1, true)),
        "downward flow should set falling: true"
    );
    assert!(outcome.unwrap().falling);

    // Downward flow takes precedence over horizontal flow:
    // (0, 2, 0) has open space below it, so it does not spread horizontally to (1, 2, 0)
    let horizontal_neighbor = BlockPos::new(1, 2, 0);
    let horizontal_spread = compute_fluid_spread(&world, horizontal_neighbor);
    assert_eq!(
        horizontal_spread, None,
        "fluid should not spread horizontally when downward path is open"
    );
}

#[test]
fn test_inward_slope_search_prefers_drop() {
    let mut world = InMemoryFluidWorld::new();
    // Solid floor at y = -1 everywhere except at (3, -1, 0) which is a cliff/drop
    for x in -6..=6 {
        for z in -6..=6 {
            world.set_solid(BlockPos::new(x, -1, z), true);
        }
    }
    // Drop at (3, -1, 0)
    world.set_solid(BlockPos::new(3, -1, 0), false);

    // Water source at (0, 0, 0)
    let source = BlockPos::new(0, 0, 0);
    world.set_fluid(source, FluidState::source(FluidType::Water));

    let preferred = preferred_flow_directions(&world, source, FluidType::Water);
    // Should prefer East (+X) because of the cliff at (3, -1, 0) within search distance 4
    assert_eq!(preferred, vec![BlockPos::new(1, 0, 0)]);

    // Check spread: East receives flow, other horizontal neighbors do not
    assert!(compute_fluid_spread(&world, BlockPos::new(1, 0, 0)).is_some());
    assert_eq!(compute_fluid_spread(&world, BlockPos::new(-1, 0, 0)), None);
    assert_eq!(compute_fluid_spread(&world, BlockPos::new(0, 0, 1)), None);
    assert_eq!(compute_fluid_spread(&world, BlockPos::new(0, 0, -1)), None);
}

#[test]
fn test_block_state_parsing_and_formatting() {
    // Parsing water states
    let water_default = FluidState::from_block_state("minecraft:water").unwrap();
    assert_eq!(water_default, FluidState::source(FluidType::Water));
    assert!(water_default.is_source());

    let water_l0 = FluidState::from_block_state("minecraft:water[level=0]").unwrap();
    assert_eq!(water_l0, FluidState::source(FluidType::Water));

    let water_l3 = FluidState::from_block_state("minecraft:water[level=3]").unwrap();
    assert_eq!(water_l3, FluidState::new(FluidType::Water, 3, false));

    let water_falling =
        FluidState::from_block_state("minecraft:water[falling=true,level=2]").unwrap();
    assert_eq!(water_falling, FluidState::new(FluidType::Water, 2, true));

    let water_falling_rev =
        FluidState::from_block_state("minecraft:water[level=2,falling=true]").unwrap();
    assert_eq!(water_falling_rev, water_falling);

    // Parsing lava states
    let lava_default = FluidState::from_block_state("minecraft:lava").unwrap();
    assert_eq!(lava_default, FluidState::source(FluidType::Lava));

    let lava_l2 = FluidState::from_block_state("minecraft:lava[level=2]").unwrap();
    assert_eq!(lava_l2, FluidState::new(FluidType::Lava, 2, false));

    // Formatting
    assert_eq!(water_l0.to_block_state(), "minecraft:water[level=0]");
    assert_eq!(water_l0.to_canonical_block_state(), "minecraft:water");
    assert_eq!(water_l3.to_block_state(), "minecraft:water[level=3]");
    assert_eq!(
        water_falling.to_block_state(),
        "minecraft:water[falling=true,level=2]"
    );
    assert_eq!(lava_l2.to_block_state(), "minecraft:lava[level=2]");

    // Invalid states
    assert!(FluidState::from_block_state("minecraft:stone").is_none());
    assert!(FluidState::from_block_state("minecraft:water[level=9]").is_none());
}

#[test]
fn test_in_memory_fluid_world_state_transitions() {
    let mut world = InMemoryFluidWorld::new();
    let pos = BlockPos::new(1, 0, 0);

    // Set solid stone at pos
    world.set_block(pos, "minecraft:stone");
    assert!(world.is_solid(pos));

    // Transition block from solid to air -> solid flag must become false
    world.set_block(pos, "minecraft:air");
    assert!(
        !world.is_solid(pos),
        "setting block to air must clear solid flag"
    );

    // Floor below pos and source neighbor
    world.set_solid(BlockPos::new(0, -1, 0), true);
    world.set_solid(BlockPos::new(1, -1, 0), true);

    // Water source at (0, 0, 0) should now spread into pos
    world.set_fluid(BlockPos::new(0, 0, 0), FluidState::source(FluidType::Water));
    let spread = compute_fluid_spread(&world, pos);
    assert_eq!(
        spread,
        Some(FluidState::new(FluidType::Water, 1, false)),
        "water should spread into cell after solid block was cleared to air"
    );
}

#[test]
fn test_horizontal_fluid_collision_empty_cell_cobblestone() {
    let mut world = InMemoryFluidWorld::new();
    let empty_pos = BlockPos::new(0, 0, 0);
    let water_source_pos = BlockPos::new(-1, 0, 0);
    let lava_source_pos = BlockPos::new(1, 0, 0);

    // Floor underneath to allow horizontal spread
    world.set_solid(BlockPos::new(-1, -1, 0), true);
    world.set_solid(BlockPos::new(0, -1, 0), true);
    world.set_solid(BlockPos::new(1, -1, 0), true);

    world.set_fluid(water_source_pos, FluidState::source(FluidType::Water));
    world.set_fluid(lava_source_pos, FluidState::source(FluidType::Lava));

    // Empty cell at (0, 0, 0) receives simultaneous water and lava horizontal arrival
    assert_eq!(
        FluidSimulator::check_collision(&world, empty_pos),
        Some("minecraft:cobblestone"),
        "simultaneous water and lava arrival must solidify into cobblestone"
    );

    assert_eq!(
        FluidSimulator::step_cell(&world, empty_pos),
        FlowOutcome::Solidify("minecraft:cobblestone")
    );

    // Step world simulation at empty cell
    world.step(&[empty_pos]);
    assert_eq!(
        world.get_block_id(empty_pos),
        "minecraft:cobblestone",
        "stepping empty cell with colliding water and lava must set block to cobblestone"
    );
    assert!(
        world.is_solid(empty_pos),
        "solidified cobblestone must be marked solid"
    );
}
