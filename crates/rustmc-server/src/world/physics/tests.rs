//! Unit tests for `world::physics`.

use super::*;

#[test]
fn freefall_in_empty_space_accelerates_and_clamps_at_terminal_velocity() {
    let mut body = PhysicsBody::player([0.0, 1000.0, 0.0]);
    let empty_world = Vec::<Aabb>::new();

    // Tick 1 moves by the initial zero velocity, then v_y = (0 - 0.08) * 0.98.
    let res = body.step(&empty_world);
    assert!((body.velocity[1] - -0.0784).abs() < 1e-12);
    assert_eq!(body.position[1], 1000.0);
    assert!(!res.on_ground);

    // Tick 2 moves by -0.0784, then v_y = (-0.0784 - 0.08) * 0.98.
    body.step(&empty_world);
    assert!((body.position[1] - 999.9216).abs() < 1e-9);
    assert!((body.velocity[1] - -0.155232).abs() < 1e-12);

    // Velocity approaches the 3.92 blocks/tick fixed point without overshooting it.
    for _ in 0..1000 {
        body.step(&empty_world);
        assert!(body.velocity[1] >= -3.92);
    }
    assert!((body.velocity[1] - -3.92).abs() < 1e-6);
    assert!(!body.on_ground);
    assert!(body.fall_distance > 500.0);
}

#[test]
fn landing_on_solid_floor_stops_vertical_motion_and_sets_on_ground_and_fall_damage() {
    let mut body = PhysicsBody::player([0.5, 5.0, 0.5]);
    // Solid block floor at y = 0 covering [0, 0, 0] to [1, 1, 1], so floor top is y = 1.0.
    let floor = Aabb::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let world = vec![floor];

    let mut landed = false;
    let mut recorded_damage = 0;
    for _ in 0..100 {
        let res = body.step(&world);
        if res.on_ground {
            landed = true;
            recorded_damage = res.fall_damage;
            break;
        }
    }

    assert!(landed);
    assert!((body.position[1] - 1.0).abs() < 1e-9);
    // The landing zeroes vertical velocity; gravity and drag then queue the next tick's.
    assert!((body.velocity[1] - -0.0784).abs() < 1e-12);
    assert!(body.on_ground);
    // Fall from 5.0 to 1.0 is exactly 4.0 blocks: 4.0 - 3.0 = 1.0 damage
    assert_eq!(recorded_damage, 1);
    assert_eq!(body.fall_distance, 0.0);

    // Subsequent tick on ground stays resting at 1.0 with 0 damage
    let next_res = body.step(&world);
    assert!((body.position[1] - 1.0).abs() < 1e-9);
    assert!((body.velocity[1] - -0.0784).abs() < 1e-12);
    assert!(next_res.on_ground);
    assert_eq!(next_res.fall_damage, 0);
}

#[test]
fn fall_damage_values_3_blocks_4_blocks_and_24_blocks() {
    assert_eq!(compute_fall_damage(0.0), 0);
    assert_eq!(compute_fall_damage(2.9), 0);
    assert_eq!(compute_fall_damage(3.0), 0);
    assert_eq!(compute_fall_damage(3.5), 0);
    assert_eq!(compute_fall_damage(4.0), 1);
    assert_eq!(compute_fall_damage(4.9), 1);
    assert_eq!(compute_fall_damage(24.0), 21);
    assert_eq!(compute_fall_damage(24.9), 21);

    // Physical drop test: exactly 3 blocks (4.0 to 1.0)
    let mut body3 = PhysicsBody::player([0.5, 4.0, 0.5]);
    let world = vec![Aabb::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])];
    let mut dmg3 = 0;
    while !body3.on_ground {
        let r = body3.step(&world);
        if r.on_ground {
            dmg3 = r.fall_damage;
        }
    }
    assert_eq!(dmg3, 0);

    // Physical drop test: exactly 4 blocks (5.0 to 1.0)
    let mut body4 = PhysicsBody::player([0.5, 5.0, 0.5]);
    let mut dmg4 = 0;
    while !body4.on_ground {
        let r = body4.step(&world);
        if r.on_ground {
            dmg4 = r.fall_damage;
        }
    }
    assert_eq!(dmg4, 1);

    // Physical drop test: exactly 24 blocks (25.0 to 1.0)
    let mut body24 = PhysicsBody::player([0.5, 25.0, 0.5]);
    let mut dmg24 = 0;
    while !body24.on_ground {
        let r = body24.step(&world);
        if r.on_ground {
            dmg24 = r.fall_damage;
        }
    }
    assert_eq!(dmg24, 21);
}

#[test]
fn wall_collision_slides_along_z_while_stopping_along_x() {
    let mut body = PhysicsBody::player([0.5, 0.0, 0.0]);
    body.on_ground = true;

    // Ground floor at y = [-1, 0]
    let floor = Aabb::new([-10.0, -1.0, -10.0], [10.0, 0.0, 10.0]);
    // Wall along X at x in [1.0, 2.0]
    let wall = Aabb::new([1.0, 0.0, -10.0], [2.0, 3.0, 10.0]);
    let world = vec![floor, wall];

    // Diagonal velocity into the wall along X and along Z
    body.velocity = [0.5, 0.0, 0.5];

    let res = body.step(&world);
    // Player width is 0.6, so max_x reaches wall at 1.0, meaning pos[0] stops at 0.7
    assert!((body.position[0] - 0.7).abs() < 1e-6);
    assert_eq!(body.velocity[0], 0.0);
    // Z movement continues without obstruction
    assert!(body.position[2] > 0.4);
    assert!(body.velocity[2] > 0.0);
    assert!(res.collided_horizontal);
    assert!(res.on_ground);

    // Continuing another tick: still blocked along X, still sliding along Z
    body.velocity[0] = 0.5;
    let res2 = body.step(&world);
    assert!((body.position[0] - 0.7).abs() < 1e-6);
    assert_eq!(body.velocity[0], 0.0);
    assert!(body.position[2] > 0.8);
    assert!(res2.collided_horizontal);
}

#[test]
fn step_height_entity_walking_forward_into_slab_steps_up_smoothly() {
    let mut body = PhysicsBody::player([0.5, 0.0, 0.5]);
    body.on_ground = true;

    // Ground floor at y = [-1, 0]
    let floor = Aabb::new([-5.0, -1.0, -5.0], [5.0, 0.0, 5.0]);
    // 0.5-block slab at x in [1.0, 2.0], y in [0.0, 0.5], z in [0.0, 1.0]
    let slab = Aabb::new([1.0, 0.0, 0.0], [2.0, 0.5, 1.0]);
    let world = vec![floor, slab];

    // Walk forward in +X towards the slab
    body.velocity = [0.4, 0.0, 0.0];

    let res = body.step(&world);
    assert!(res.stepped_up);
    // Stepped onto the top of the slab at y = 0.5
    assert!((body.position[1] - 0.5).abs() < 1e-6);
    assert!(body.on_ground);
    // Advanced forward horizontally onto the slab
    assert!(body.position[0] > 0.8);
    // Velocity along X was not stopped because step-up cleared the barrier
    assert!(body.velocity[0] > 0.0);
}

#[test]
fn full_wall_exceeding_step_height_blocks_movement() {
    let mut body = PhysicsBody::player([0.5, 0.0, 0.5]);
    body.on_ground = true;

    let floor = Aabb::new([-5.0, -1.0, -5.0], [5.0, 0.0, 5.0]);
    // 1.0-block tall wall (step height is 0.6, so cannot step up)
    let wall = Aabb::new([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]);
    let world = vec![floor, wall];

    body.velocity = [0.4, 0.0, 0.0];
    let res = body.step(&world);

    assert!(!res.stepped_up);
    assert_eq!(body.position[1], 0.0);
    assert!((body.position[0] - 0.7).abs() < 1e-6);
    assert_eq!(body.velocity[0], 0.0);
    assert!(res.collided_horizontal);
}

#[test]
fn jumping_into_ceiling_stops_rising_motion() {
    let mut body = PhysicsBody::player([0.0, 1.0, 0.0]);
    // Ceiling at y = 3.0. Player height = 1.8, so head is at 2.8.
    let ceiling = Aabb::new([-5.0, 3.0, -5.0], [5.0, 4.0, 5.0]);
    let world = vec![ceiling];

    // Jump upward with high velocity
    body.velocity = [0.0, 1.0, 0.0];

    let res = body.step(&world);
    assert!(res.ceiling_collision);
    // Player top stopped at ceiling y = 3.0, so base pos[1] stopped at 3.0 - 1.8 = 1.2
    assert!((body.position[1] - 1.2).abs() < 1e-6);
    // The ceiling zeroes the rise; gravity and drag then queue the next tick's fall.
    assert!((body.velocity[1] - -0.0784).abs() < 1e-12);
    assert!(!body.on_ground);
}

#[test]
fn aabb_intersects_expand_offset_contracts() {
    let a = Aabb::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let b = Aabb::new([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]);
    let c = Aabb::new([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]); // touching face
    let d = Aabb::new([2.0, 2.0, 2.0], [3.0, 3.0, 3.0]); // disjoint

    assert!(a.intersects(&b));
    assert!(b.intersects(&a));
    assert!(!a.intersects(&c)); // face touching is not intersecting
    assert!(!a.intersects(&d));

    let expanded = a.expand(1.0, -2.0, 0.5);
    assert_eq!(expanded.min, [0.0, -2.0, 0.0]);
    assert_eq!(expanded.max, [2.0, 1.0, 1.5]);

    let offset = a.offset(2.0, 3.0, 4.0);
    assert_eq!(offset.min, [2.0, 3.0, 4.0]);
    assert_eq!(offset.max, [3.0, 4.0, 5.0]);

    assert!(a.contains_point([0.5, 0.5, 0.5]));
    assert!(!a.contains_point([1.5, 0.5, 0.5]));
}

#[test]
fn simple_voxel_world_and_chunk_collision_view() {
    let mut voxel_world = SimpleVoxelWorld::new();
    voxel_world.add_solid_block(0, 0, 0);
    voxel_world.add_solid_block(0, 1, 0);

    let query = Aabb::new([-0.5, -0.5, -0.5], [1.5, 2.5, 1.5]);
    let boxes = voxel_world.get_colliding_boxes(&query);
    assert_eq!(boxes.len(), 2);

    // Chunk query test
    let mut chunk = Chunk {
        x: 0,
        z: 0,
        blocks: vec![Block::Air; CHUNK_SIDE * CHUNK_SIDE * WORLD_HEIGHT],
    };
    // Place stone at (2, 5, 2)
    chunk.set(2, 5, 2, Block::Stone);

    let chunk_query = Aabb::new([1.5, 4.5, 1.5], [2.5, 5.5, 2.5]);
    let chunk_boxes = chunk.get_colliding_boxes(&chunk_query);
    assert_eq!(chunk_boxes.len(), 1);
    assert_eq!(chunk_boxes[0], Aabb::from_block(2, 5, 2));

    // Air only query returns nothing
    let air_query = Aabb::new([10.0, 10.0, 10.0], [11.0, 11.0, 11.0]);
    assert!(chunk.get_colliding_boxes(&air_query).is_empty());
}
