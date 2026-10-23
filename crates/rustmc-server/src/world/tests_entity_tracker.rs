use super::entity_tracker::{
    EntityType, SpatialEntityTracker, TrackingError, TrackingEvent, chunk_coords, try_chunk_coords,
};

#[test]
fn test_entity_spawn_visibility_in_range() {
    let mut tracker = SpatialEntityTracker::new();

    // 1. Spawn observer Player 1 at (0.0, 64.0, 0.0)
    let (p1, p1_events) = tracker
        .add_entity(EntityType::Player, (0.0, 64.0, 0.0))
        .expect("valid coords");
    assert_eq!(p1, 1);
    assert!(
        p1_events.is_empty(),
        "First player sees no existing entities"
    );
    assert!(tracker.observed_by(p1).unwrap().is_empty());

    // 2. Spawn Monster at (20.0, 64.0, 0.0) within Monster tracking range (48.0)
    let (m1, m1_events) = tracker
        .add_entity(EntityType::Monster, (20.0, 64.0, 0.0))
        .expect("valid coords");
    assert_eq!(m1, 2);
    assert_eq!(
        m1_events,
        vec![TrackingEvent::Spawn {
            observer: p1,
            entity: m1,
            pos: (20.0, 64.0, 0.0),
            entity_type: EntityType::Monster,
        }]
    );
    assert!(tracker.is_observing(p1, m1));

    // 3. Spawn Monster 2 at (100.0, 64.0, 0.0) outside Monster tracking range (48.0)
    let (m2, m2_events) = tracker
        .add_entity(EntityType::Monster, (100.0, 64.0, 0.0))
        .expect("valid coords");
    assert_eq!(m2, 3);
    assert!(
        m2_events.is_empty(),
        "Out-of-range monster emits no spawn events"
    );
    assert!(!tracker.is_observing(p1, m2));

    // 4. Spawn Player 2 at (30.0, 64.0, 0.0)
    // - P1 <-> P2 distance is 30.0 <= 64.0 (both players observe each other)
    // - P2 <-> M1 distance is 10.0 <= 48.0 (P2 observes M1)
    // - P2 <-> M2 distance is 70.0 > 48.0 (P2 does not observe M2)
    let (p2, p2_events) = tracker
        .add_entity(EntityType::Player, (30.0, 64.0, 0.0))
        .expect("valid coords");
    assert_eq!(p2, 4);

    assert_eq!(
        p2_events,
        vec![
            // P1 observes new entity P2
            TrackingEvent::Spawn {
                observer: p1,
                entity: p2,
                pos: (30.0, 64.0, 0.0),
                entity_type: EntityType::Player,
            },
            // P2 as observer sees existing entities within range: P1 and M1
            TrackingEvent::Spawn {
                observer: p2,
                entity: p1,
                pos: (0.0, 64.0, 0.0),
                entity_type: EntityType::Player,
            },
            TrackingEvent::Spawn {
                observer: p2,
                entity: m1,
                pos: (20.0, 64.0, 0.0),
                entity_type: EntityType::Monster,
            },
        ]
    );

    // Verify view sets
    assert_eq!(
        tracker.observed_by(p1).unwrap(),
        &[m1, p2].into_iter().collect()
    );
    assert_eq!(
        tracker.observed_by(p2).unwrap(),
        &[p1, m1].into_iter().collect()
    );
    // Neither player observes themselves
    assert!(!tracker.is_observing(p1, p1));
    assert!(!tracker.is_observing(p2, p2));
}

#[test]
fn test_entity_move_emits_destroy_when_exiting_radius() {
    let mut tracker = SpatialEntityTracker::new();

    let (player, _) = tracker
        .add_entity(EntityType::Player, (0.0, 64.0, 0.0))
        .unwrap();
    let (monster, _) = tracker
        .add_entity(EntityType::Monster, (10.0, 64.0, 0.0))
        .unwrap();
    assert!(tracker.is_observing(player, monster));

    // 1. Move monster within range (distance 25.0 <= 48.0)
    let move_events = tracker
        .move_entity(monster, (25.0, 64.0, 0.0))
        .expect("valid move");
    assert_eq!(
        move_events,
        vec![TrackingEvent::Move {
            observer: player,
            entity: monster,
            new_pos: (25.0, 64.0, 0.0),
        }]
    );
    assert!(tracker.is_observing(player, monster));

    // 2. Move monster out of tracking range (distance 50.0 > 48.0)
    let destroy_events = tracker
        .move_entity(monster, (50.0, 64.0, 0.0))
        .expect("valid move");
    assert_eq!(
        destroy_events,
        vec![TrackingEvent::Destroy {
            observer: player,
            entity: monster,
        }]
    );
    assert!(!tracker.is_observing(player, monster));

    // 3. Move monster while staying out of range (distance 60.0 > 48.0)
    let out_events = tracker
        .move_entity(monster, (60.0, 64.0, 0.0))
        .expect("valid move");
    assert!(
        out_events.is_empty(),
        "No events when moving outside radius"
    );
    assert!(!tracker.is_observing(player, monster));

    // 4. Move monster back into range (distance 30.0 <= 48.0)
    let spawn_events = tracker
        .move_entity(monster, (30.0, 64.0, 0.0))
        .expect("valid move");
    assert_eq!(
        spawn_events,
        vec![TrackingEvent::Spawn {
            observer: player,
            entity: monster,
            pos: (30.0, 64.0, 0.0),
            entity_type: EntityType::Monster,
        }]
    );
    assert!(tracker.is_observing(player, monster));

    // 5. Move observer away so that entity exits observer's radius
    let player_move_events = tracker
        .move_entity(player, (-50.0, 64.0, 0.0))
        .expect("valid move");
    assert_eq!(
        player_move_events,
        vec![TrackingEvent::Destroy {
            observer: player,
            entity: monster,
        }]
    );
    assert!(!tracker.is_observing(player, monster));
}

#[test]
fn test_entity_chunk_boundary_migration() {
    let mut tracker = SpatialEntityTracker::new();

    // Add entity in chunk (0, 0)
    assert_eq!(chunk_coords((5.0, 64.0, 5.0)), (0, 0));
    let (entity_id, _) = tracker
        .add_entity(EntityType::Animal, (5.0, 64.0, 5.0))
        .unwrap();
    let entity = tracker.get_entity(entity_id).unwrap();
    assert_eq!(entity.chunk, (0, 0));
    assert!(tracker.chunk_index()[&(0, 0)].contains(&entity_id));
    assert!(!tracker.chunk_index().contains_key(&(1, 0)));

    // Cross X chunk boundary into chunk (1, 0) [x >= 16.0]
    tracker.move_entity(entity_id, (16.5, 64.0, 5.0)).unwrap();
    let entity = tracker.get_entity(entity_id).unwrap();
    assert_eq!(entity.chunk, (1, 0));
    assert!(tracker.chunk_index()[&(1, 0)].contains(&entity_id));
    assert!(
        tracker
            .chunk_index()
            .get(&(0, 0))
            .is_none_or(|s| !s.contains(&entity_id)),
        "Old chunk bucket must no longer contain the entity"
    );

    // Cross into negative chunk coordinates (-1, -1) [x < 0.0, z < 0.0]
    tracker.move_entity(entity_id, (-5.0, 64.0, -10.0)).unwrap();
    let entity = tracker.get_entity(entity_id).unwrap();
    assert_eq!(entity.chunk, (-1, -1));
    assert!(tracker.chunk_index()[&(-1, -1)].contains(&entity_id));
    assert!(
        tracker
            .chunk_index()
            .get(&(1, 0))
            .is_none_or(|s| !s.contains(&entity_id))
    );

    // Add a second entity to the same chunk (-1, -1)
    let (entity2_id, _) = tracker
        .add_entity(EntityType::Animal, (-12.0, 64.0, -4.0))
        .unwrap();
    assert_eq!(tracker.get_entity(entity2_id).unwrap().chunk, (-1, -1));
    let chunk_set = &tracker.chunk_index()[&(-1, -1)];
    assert!(chunk_set.contains(&entity_id));
    assert!(chunk_set.contains(&entity2_id));
    assert_eq!(chunk_set.len(), 2);

    // Move first entity away to (0.0, 64.0, 0.0) in chunk (0, 0)
    tracker.move_entity(entity_id, (0.0, 64.0, 0.0)).unwrap();
    assert_eq!(tracker.get_entity(entity_id).unwrap().chunk, (0, 0));
    assert!(tracker.chunk_index()[&(0, 0)].contains(&entity_id));
    // Chunk (-1, -1) should still retain entity2
    assert_eq!(tracker.chunk_index()[&(-1, -1)].len(), 1);
    assert!(tracker.chunk_index()[&(-1, -1)].contains(&entity2_id));
    assert!(!tracker.chunk_index()[&(-1, -1)].contains(&entity_id));
}

#[test]
fn test_item_entity_shorter_tracking_range() {
    let mut tracker = SpatialEntityTracker::new();

    assert_eq!(EntityType::Item.tracking_range(), 16.0);
    assert_eq!(EntityType::Animal.tracking_range(), 32.0);
    assert_eq!(EntityType::Projectile.tracking_range(), 32.0);
    assert_eq!(EntityType::Monster.tracking_range(), 48.0);
    assert_eq!(EntityType::Player.tracking_range(), 64.0);

    let (player, _) = tracker
        .add_entity(EntityType::Player, (0.0, 64.0, 0.0))
        .unwrap();

    // Spawn an Item at distance 20.0 (20.0 > 16.0 => outside range)
    let (item_far, item_far_events) = tracker
        .add_entity(EntityType::Item, (20.0, 64.0, 0.0))
        .unwrap();
    assert!(
        item_far_events.is_empty(),
        "Item at distance 20.0 must be outside Item tracking range (16.0)"
    );
    assert!(!tracker.is_observing(player, item_far));

    // Spawn a Monster at distance 20.0 (20.0 <= 48.0 => inside range)
    let (monster, monster_events) = tracker
        .add_entity(EntityType::Monster, (20.0, 64.0, 0.0))
        .unwrap();
    assert_eq!(
        monster_events,
        vec![TrackingEvent::Spawn {
            observer: player,
            entity: monster,
            pos: (20.0, 64.0, 0.0),
            entity_type: EntityType::Monster,
        }],
        "Monster at distance 20.0 must be visible because Monster range is 48.0"
    );
    assert!(tracker.is_observing(player, monster));

    // Spawn an Item at distance 15.0 (15.0 <= 16.0 => inside range)
    let (item_near, item_near_events) = tracker
        .add_entity(EntityType::Item, (15.0, 64.0, 0.0))
        .unwrap();
    assert_eq!(
        item_near_events,
        vec![TrackingEvent::Spawn {
            observer: player,
            entity: item_near,
            pos: (15.0, 64.0, 0.0),
            entity_type: EntityType::Item,
        }],
        "Item at distance 15.0 must be visible within 16.0 range"
    );
    assert!(tracker.is_observing(player, item_near));

    // Move the far item closer into range (distance 12.0 <= 16.0)
    let move_in_events = tracker.move_entity(item_far, (12.0, 64.0, 0.0)).unwrap();
    assert_eq!(
        move_in_events,
        vec![TrackingEvent::Spawn {
            observer: player,
            entity: item_far,
            pos: (12.0, 64.0, 0.0),
            entity_type: EntityType::Item,
        }]
    );
    assert!(tracker.is_observing(player, item_far));

    // Move the near item out of range (distance 16.5 > 16.0)
    let move_out_events = tracker.move_entity(item_near, (16.5, 64.0, 0.0)).unwrap();
    assert_eq!(
        move_out_events,
        vec![TrackingEvent::Destroy {
            observer: player,
            entity: item_near,
        }]
    );
    assert!(!tracker.is_observing(player, item_near));
}

#[test]
fn test_deterministic_entity_id_allocation() {
    let mut tracker1 = SpatialEntityTracker::new();

    let (id1, _) = tracker1
        .add_entity(EntityType::Player, (0.0, 64.0, 0.0))
        .unwrap();
    let (id2, _) = tracker1
        .add_entity(EntityType::Monster, (1.0, 64.0, 0.0))
        .unwrap();
    let (id3, _) = tracker1
        .add_entity(EntityType::Animal, (2.0, 64.0, 0.0))
        .unwrap();
    let (id4, _) = tracker1
        .add_entity(EntityType::Item, (3.0, 64.0, 0.0))
        .unwrap();
    let (id5, _) = tracker1
        .add_entity(EntityType::Projectile, (4.0, 64.0, 0.0))
        .unwrap();

    // Must be strictly monotonic positive integers starting at 1
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    assert_eq!(id3, 3);
    assert_eq!(id4, 4);
    assert_eq!(id5, 5);

    // Removing entities does not recycle or regress the sequence
    tracker1.remove_entity(id2);
    tracker1.remove_entity(id4);

    let (id6, _) = tracker1
        .add_entity(EntityType::Player, (5.0, 64.0, 0.0))
        .unwrap();
    assert_eq!(
        id6, 6,
        "ID sequence must advance monotonically after deletions"
    );

    // Replay on fresh tracker produces identical deterministic sequence
    let mut tracker2 = SpatialEntityTracker::new();
    let (t2_id1, _) = tracker2
        .add_entity(EntityType::Player, (0.0, 64.0, 0.0))
        .unwrap();
    let (t2_id2, _) = tracker2
        .add_entity(EntityType::Monster, (1.0, 64.0, 0.0))
        .unwrap();
    let (t2_id3, _) = tracker2
        .add_entity(EntityType::Animal, (2.0, 64.0, 0.0))
        .unwrap();
    let (t2_id4, _) = tracker2
        .add_entity(EntityType::Item, (3.0, 64.0, 0.0))
        .unwrap();
    let (t2_id5, _) = tracker2
        .add_entity(EntityType::Projectile, (4.0, 64.0, 0.0))
        .unwrap();
    tracker2.remove_entity(t2_id2);
    tracker2.remove_entity(t2_id4);
    let (t2_id6, _) = tracker2
        .add_entity(EntityType::Player, (5.0, 64.0, 0.0))
        .unwrap();

    assert_eq!(
        [id1, id2, id3, id4, id5, id6],
        [t2_id1, t2_id2, t2_id3, t2_id4, t2_id5, t2_id6]
    );
}

#[test]
fn test_remove_entity_emits_destroy_to_observers() {
    let mut tracker = SpatialEntityTracker::new();

    let (p1, _) = tracker
        .add_entity(EntityType::Player, (0.0, 64.0, 0.0))
        .unwrap();
    let (p2, _) = tracker
        .add_entity(EntityType::Player, (10.0, 64.0, 0.0))
        .unwrap();
    let (monster, _) = tracker
        .add_entity(EntityType::Monster, (5.0, 64.0, 0.0))
        .unwrap();

    assert!(tracker.is_observing(p1, monster));
    assert!(tracker.is_observing(p2, monster));

    // Removing monster emits Destroy to both P1 and P2
    let destroy_events = tracker.remove_entity(monster);
    assert_eq!(
        destroy_events,
        vec![
            TrackingEvent::Destroy {
                observer: p1,
                entity: monster,
            },
            TrackingEvent::Destroy {
                observer: p2,
                entity: monster,
            },
        ]
    );

    assert!(!tracker.is_observing(p1, monster));
    assert!(!tracker.is_observing(p2, monster));
    assert!(tracker.get_entity(monster).is_none());

    // Removing an already removed or nonexistent entity returns empty events safely
    let empty_events = tracker.remove_entity(monster);
    assert!(empty_events.is_empty());
}

#[test]
fn test_3d_diagonal_distance_calculation() {
    let mut tracker = SpatialEntityTracker::new();

    let (player, _) = tracker
        .add_entity(EntityType::Player, (0.0, 0.0, 0.0))
        .unwrap();

    // Item range is 16.0.
    // 3D vector (10.0, 10.0, 10.0) has length sqrt(300) ~= 17.32 > 16.0.
    let (item_diag_far, events_far) = tracker
        .add_entity(EntityType::Item, (10.0, 10.0, 10.0))
        .unwrap();
    assert!(
        events_far.is_empty(),
        "Diagonal distance 17.32 must exceed 16.0 range"
    );
    assert!(!tracker.is_observing(player, item_diag_far));

    // 3D vector (9.0, 9.0, 9.0) has length sqrt(243) ~= 15.588 <= 16.0.
    let (item_diag_near, events_near) = tracker
        .add_entity(EntityType::Item, (9.0, 9.0, 9.0))
        .unwrap();
    assert_eq!(events_near.len(), 1);
    assert!(tracker.is_observing(player, item_diag_near));
}

#[test]
fn test_rejects_non_finite_and_out_of_bounds_positions() {
    let mut tracker = SpatialEntityTracker::new();

    // NaN coordinates
    assert_eq!(
        tracker.add_entity(EntityType::Player, (f64::NAN, 64.0, 0.0)),
        Err(TrackingError::NonFinitePosition)
    );
    assert_eq!(
        tracker.add_entity(EntityType::Monster, (0.0, f64::NAN, 0.0)),
        Err(TrackingError::NonFinitePosition)
    );
    assert_eq!(
        tracker.add_entity(EntityType::Item, (0.0, 64.0, f64::NAN)),
        Err(TrackingError::NonFinitePosition)
    );

    // Infinite coordinates
    assert_eq!(
        tracker.add_entity(EntityType::Player, (f64::INFINITY, 64.0, 0.0)),
        Err(TrackingError::NonFinitePosition)
    );
    assert_eq!(
        tracker.add_entity(EntityType::Player, (0.0, f64::NEG_INFINITY, 0.0)),
        Err(TrackingError::NonFinitePosition)
    );

    // Out-of-bounds coordinates
    assert_eq!(
        tracker.add_entity(EntityType::Player, (35_000_000.0, 64.0, 0.0)),
        Err(TrackingError::OutOfBounds)
    );
    assert_eq!(
        tracker.add_entity(EntityType::Player, (0.0, 5000.0, 0.0)),
        Err(TrackingError::OutOfBounds)
    );

    // Valid add followed by invalid move attempts
    let (p1, _) = tracker
        .add_entity(EntityType::Player, (0.0, 64.0, 0.0))
        .unwrap();
    assert_eq!(
        tracker.move_entity(p1, (f64::NAN, 64.0, 0.0)),
        Err(TrackingError::NonFinitePosition)
    );
    assert_eq!(
        tracker.move_entity(p1, (50_000_000.0, 64.0, 0.0)),
        Err(TrackingError::OutOfBounds)
    );

    // Position of p1 must remain unchanged at original valid location
    let entity = tracker.get_entity(p1).unwrap();
    assert_eq!(entity.pos, (0.0, 64.0, 0.0));

    // Move nonexistent entity
    assert_eq!(
        tracker.move_entity(999, (10.0, 64.0, 10.0)),
        Err(TrackingError::EntityNotFound(999))
    );

    // try_chunk_coords tests
    assert_eq!(try_chunk_coords((f64::NAN, 64.0, 0.0)), None);
    assert_eq!(try_chunk_coords((40_000_000.0, 64.0, 0.0)), None);
    assert_eq!(try_chunk_coords((16.0, 64.0, -1.0)), Some((1, -1)));
}

#[test]
fn test_spatial_chunk_indexing_candidate_bounding() {
    let mut tracker = SpatialEntityTracker::new();

    // Spawn observer player at (0.0, 64.0, 0.0) [chunk (0, 0)]
    let (p1, _) = tracker
        .add_entity(EntityType::Player, (0.0, 64.0, 0.0))
        .unwrap();

    // Spawn 100 entities far away at chunk (100, 100) ~ block (1600.0, 64.0, 1600.0)
    for i in 0..100 {
        let (id, events) = tracker
            .add_entity(
                EntityType::Animal,
                (1600.0 + (i as f64) * 0.1, 64.0, 1600.0),
            )
            .unwrap();
        assert!(events.is_empty(), "Distant entities emit no events to p1");
        assert!(!tracker.is_observing(p1, id));
    }

    // p1 observer view set must be completely empty
    assert_eq!(tracker.observed_by(p1).unwrap().len(), 0);

    // Spawn 1 monster in adjacent chunk (1, 0) at (20.0, 64.0, 0.0) within 48 blocks
    let (m1, m_events) = tracker
        .add_entity(EntityType::Monster, (20.0, 64.0, 0.0))
        .unwrap();
    assert_eq!(m_events.len(), 1);
    assert!(tracker.is_observing(p1, m1));
    assert_eq!(tracker.observed_by(p1).unwrap().len(), 1);

    // Moving m1 within range emits Move to observer p1
    let move_events = tracker.move_entity(m1, (21.0, 64.0, 0.0)).unwrap();
    assert_eq!(
        move_events,
        vec![TrackingEvent::Move {
            observer: p1,
            entity: m1,
            new_pos: (21.0, 64.0, 0.0),
        }]
    );
    assert_eq!(tracker.observed_by(p1).unwrap().len(), 1);

    // Moving p1 does not emit events to itself, and does not observe distant entities
    let p1_move_events = tracker.move_entity(p1, (2.0, 64.0, 0.0)).unwrap();
    assert!(p1_move_events.is_empty());
    assert_eq!(tracker.observed_by(p1).unwrap().len(), 1);
}
