//! Unit tests, part 2 of 2.

use super::*;

#[test]
fn crash_between_checkpoint_and_journal_reset_recovers_and_finishes_compaction() {
    let dir = TestDir::new("checkpoint-with-old-journal");
    let generator = Generator::new(41);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 3);
    let old_journal = std::fs::read(journal_path(&dir)).expect("read log");
    world.compact().expect("compact world");
    drop(world);

    // A valid new checkpoint beside the complete old journal.
    std::fs::write(journal_path(&dir), &old_journal).expect("restore old journal");

    let mut world = assert_recovered(&dir, generator, 3);
    assert_eq!(
        std::fs::metadata(journal_path(&dir))
            .expect("metadata")
            .len(),
        32,
        "recovery finishes the interrupted compaction"
    );
    commit_ticks(&mut world, 1);
    drop(world);
    drop(assert_recovered(&dir, generator, 4));
}

#[test]
fn checkpoint_beside_a_torn_old_journal_keeps_tick_continuity() {
    let dir = TestDir::new("checkpoint-torn-old-journal");
    let generator = Generator::new(41);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 3);
    let old_journal = std::fs::read(journal_path(&dir)).expect("read log");
    world.compact().expect("compact world");
    drop(world);

    // Unsynced tail of the old journal lost: it now ends before the checkpoint.
    std::fs::write(journal_path(&dir), &old_journal[..record_offset(1) + 7])
        .expect("write torn journal");

    let mut world = assert_recovered(&dir, generator, 3);
    commit_ticks(&mut world, 2);
    drop(world);
    drop(assert_recovered(&dir, generator, 5));
}

#[test]
fn crash_while_finishing_a_compaction_at_open_stays_recoverable() {
    let generator = Generator::new(43);
    let mut fired_steps = 0;
    for skipped in 0.. {
        let dir = TestDir::new("open-crash");
        let mut world = World::open(dir.path(), generator).expect("open world");
        commit_ticks(&mut world, 3);
        let old_journal = std::fs::read(journal_path(&dir)).expect("read log");
        world.compact().expect("compact world");
        drop(world);
        std::fs::write(journal_path(&dir), &old_journal).expect("restore old journal");

        journal::faults::fail_after(skipped);
        let result = World::open(dir.path(), generator);
        let fired = journal::faults::clear();
        if fired.is_empty() {
            assert_eq!(result.expect("open with no fault").current_tick(), 3);
            break;
        }
        assert!(result.is_err());
        fired_steps += 1;
        drop(assert_recovered(&dir, generator, 3));
    }
    assert_eq!(fired_steps, 5);
}

#[test]
fn crash_while_truncating_a_torn_tail_stays_recoverable() {
    let dir = TestDir::new("truncate-crash");
    let generator = Generator::new(43);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 2);
    drop(world);

    let mut file = OpenOptions::new()
        .append(true)
        .open(journal_path(&dir))
        .expect("open log");
    file.write_all(&[0x55; 20]).expect("append torn bytes");
    drop(file);

    for step in ["open.truncate", "open.sync"] {
        journal::faults::fail_steps(&[step]);
        assert!(World::open(dir.path(), generator).is_err());
        assert_eq!(journal::faults::clear(), [step]);
    }
    drop(assert_recovered(&dir, generator, 2));
}

#[test]
fn a_journal_starting_after_the_checkpoint_is_rejected() {
    let dir = TestDir::new("checkpoint-rolled-back");
    let generator = Generator::new(47);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 2);
    world.compact().expect("compact world");
    commit_ticks(&mut world, 1);
    drop(world);

    // The new journal without the checkpoint it depends on: ticks 0 and 1 gone.
    std::fs::remove_file(checkpoint_path(&dir)).expect("remove checkpoint");
    assert!(matches!(
        World::open(dir.path(), generator),
        Err(WorldError::Corrupted(message)) if message.contains("acknowledged ticks are missing")
    ));
}

#[test]
fn a_checkpoint_without_its_journal_is_rejected() {
    let dir = TestDir::new("missing-journal");
    let generator = Generator::new(47);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 1);
    world.compact().expect("compact world");
    drop(world);

    std::fs::remove_file(journal_path(&dir)).expect("remove journal");
    assert!(matches!(
        World::open(dir.path(), generator),
        Err(WorldError::Corrupted(_))
    ));
}

#[test]
fn mutation_limit_is_enforced_before_commit_and_replays() {
    let dir = TestDir::new("mutation-limit");
    let generator = Generator::new(53);
    let mut world = World::open(dir.path(), generator).expect("open world");

    for _ in 0..MAX_MUTATIONS_PER_TICK {
        assert!(world.set_block(0, 60, 0, Block::Stone));
    }
    assert!(
        !world.set_block(0, 60, 0, Block::Dirt),
        "the writer must refuse what the reader would reject"
    );
    world.commit_tick().expect("commit a full tick");
    assert!(world.set_block(1, 60, 0, Block::Dirt));
    world.commit_tick().expect("commit the next tick");
    drop(world);

    let world = World::open(dir.path(), generator).expect("replay a full tick");
    assert_eq!(world.current_tick(), 2);
    assert_eq!(world.get_block(0, 60, 0), Some(Block::Stone));
    assert_eq!(world.get_block(1, 60, 0), Some(Block::Dirt));
}

#[test]
fn direct_appends_are_bounded_and_sequenced_without_touching_the_log() {
    let dir = TestDir::new("direct-append");
    let generator = Generator::new(53);
    let mut world = World::open(dir.path(), generator).expect("open world");
    let journal = world.journal_mut().expect("journal");

    let oversized = vec![BlockMutation::new(0, 60, 0, Block::Dirt); MAX_MUTATIONS_PER_TICK + 1];
    assert!(matches!(
        journal.append_tick(0, &oversized),
        Err(WorldError::TooManyMutations { .. })
    ));
    assert!(matches!(
        journal.append_tick(5, &[]),
        Err(WorldError::TickOutOfSequence {
            expected: 0,
            found: 5
        })
    ));
    assert!(matches!(
        journal.compact(5, &std::collections::BTreeMap::new()),
        Err(WorldError::TickOutOfSequence { .. })
    ));
    journal.append_tick(0, &[]).expect("journal stays usable");
    drop(world);

    assert_eq!(
        std::fs::metadata(journal_path(&dir))
            .expect("metadata")
            .len(),
        32 + 16 + 4
    );
}

#[test]
fn every_block_round_trips_through_its_journal_discriminant() {
    for (index, block) in Block::ALL.iter().enumerate() {
        assert_eq!(
            *block as usize, index,
            "{block:?} is out of order in Block::ALL"
        );
        assert_eq!(Block::from_u8(*block as u8), Some(*block));
    }
    // One past the last block is corrupt input, not a new block.
    assert_eq!(Block::from_u8(Block::ALL.len() as u8), None);
    assert_eq!(Block::from_u8(u8::MAX), None);
}
