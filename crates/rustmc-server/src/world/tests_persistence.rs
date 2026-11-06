//! Persistence and crash-recovery unit tests for authoritative world journal and checkpoints.

use super::*;
use std::fs::OpenOptions;
use std::io::Write;

struct TestDir(std::path::PathBuf);

impl TestDir {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rustmc-{prefix}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn test_in_memory_mutation_across_chunk_borders() {
    let mut world = World::new(Generator::new(2026));
    assert!(world.journal().is_none());

    // Chunk (0, 0) and Chunk (1, 0)
    assert!(world.set_block(15, 64, 0, Block::Sand));
    assert!(world.set_block(16, 64, 0, Block::Sandstone));

    // Chunk (-1, 0) and Chunk (0, 0)
    assert!(world.set_block(-1, 64, 0, Block::Terracotta));
    assert!(world.set_block(0, 64, 0, Block::RedSand));

    // Chunk (0, -1) and Chunk (-1, -1)
    assert!(world.set_block(0, 64, -1, Block::Snow));
    assert!(world.set_block(-1, 64, -1, Block::SpruceLog));

    // Out of bounds mutations must be rejected
    assert!(!world.set_block(0, -1, 0, Block::Stone));
    assert!(!world.set_block(0, WORLD_HEIGHT as i64, 0, Block::Stone));

    // Staged pending mutations are immediately readable
    assert_eq!(world.block(15, 64, 0), Some(Block::Sand));
    assert_eq!(world.block(16, 64, 0), Some(Block::Sandstone));

    // Commit tick
    world.commit_tick().expect("commit tick");

    // After commit, mutations are permanent in committed state and cached chunks
    assert_eq!(world.block(15, 64, 0), Some(Block::Sand));
    assert_eq!(world.block(16, 64, 0), Some(Block::Sandstone));
    assert_eq!(world.block(-1, 64, 0), Some(Block::Terracotta));
    assert_eq!(world.block(0, 64, 0), Some(Block::RedSand));
    assert_eq!(world.block(0, 64, -1), Some(Block::Snow));
    assert_eq!(world.block(-1, 64, -1), Some(Block::SpruceLog));

    // Verify chunk objects directly
    assert_eq!(world.chunk(0, 0).block(15, 64, 0), Some(Block::Sand));
    assert_eq!(world.chunk(1, 0).block(0, 64, 0), Some(Block::Sandstone));
    assert_eq!(world.chunk(-1, 0).block(15, 64, 0), Some(Block::Terracotta));
    assert_eq!(world.chunk(0, 0).block(0, 64, 0), Some(Block::RedSand));
    assert_eq!(world.chunk(0, -1).block(0, 64, 15), Some(Block::Snow));
    assert_eq!(
        world.chunk(-1, -1).block(15, 64, 15),
        Some(Block::SpruceLog)
    );
}

#[test]
fn test_journal_clean_restart_recovery() {
    let test_dir = TestDir::new("clean-restart");
    let generator = Generator::new(42);

    {
        let mut world = World::open(test_dir.path(), generator).expect("open world");

        // Tick 0
        assert!(world.set_block(10, 64, 10, Block::Stone));
        assert!(world.set_block(-5, 60, -5, Block::Dirt));
        world.commit_tick().expect("commit tick 0");

        // Tick 1
        assert!(world.set_block(10, 65, 10, Block::Grass));
        assert!(world.set_block(20, 70, 20, Block::Log));
        world.commit_tick().expect("commit tick 1");

        // Tick 2: overwrite previous block
        assert!(world.set_block(10, 64, 10, Block::Leaves));
        world.commit_tick().expect("commit tick 2");

        world.flush().expect("flush journal");
        assert_eq!(world.current_tick(), 3);
    }

    // Reopen world after clean shutdown
    {
        let mut world = World::open(test_dir.path(), generator).expect("reopen world");
        assert_eq!(world.current_tick(), 3);

        assert_eq!(world.block(10, 64, 10), Some(Block::Leaves));
        assert_eq!(world.block(-5, 60, -5), Some(Block::Dirt));
        assert_eq!(world.block(10, 65, 10), Some(Block::Grass));
        assert_eq!(world.block(20, 70, 20), Some(Block::Log));

        // Further tick writes resume cleanly
        assert!(world.set_block(30, 80, 30, Block::Snow));
        world.commit_tick().expect("commit tick 3");
        assert_eq!(world.current_tick(), 4);
        world.flush().expect("flush journal");
    }

    // Reopen again and check all changes exist
    {
        let mut world = World::open(test_dir.path(), generator).expect("reopen world 2");
        assert_eq!(world.current_tick(), 4);
        assert_eq!(world.block(30, 80, 30), Some(Block::Snow));
        assert_eq!(world.block(10, 64, 10), Some(Block::Leaves));
    }
}

#[test]
fn test_journal_crash_recovery_truncated_trailing_record() {
    let test_dir = TestDir::new("crash-truncated");
    let generator = Generator::new(100);

    let valid_len;
    {
        let mut world = World::open(test_dir.path(), generator).expect("open world");
        assert!(world.set_block(1, 60, 1, Block::Stone));
        world.commit_tick().expect("commit tick 0");
        world.flush().expect("flush journal");
        drop(world);

        valid_len = std::fs::metadata(test_dir.path().join("journal.log"))
            .expect("metadata")
            .len();
    }

    // Append partial, corrupted record bytes simulating a crash / power outage
    {
        let mut f = OpenOptions::new()
            .append(true)
            .open(test_dir.path().join("journal.log"))
            .expect("open journal for append");
        f.write_all(b"partial-corrupted-bytes-from-a-crash-torn-write")
            .expect("write partial bytes");
        f.flush().expect("flush corrupted bytes");
    }

    // Open world again: recovery must detect truncated trailing bytes,
    // truncate back to valid_len, and resume cleanly.
    {
        let mut world = World::open(test_dir.path(), generator).expect("recover world");
        assert_eq!(world.current_tick(), 1);
        assert_eq!(world.block(1, 60, 1), Some(Block::Stone));

        let restored_len = std::fs::metadata(test_dir.path().join("journal.log"))
            .expect("metadata")
            .len();
        assert_eq!(restored_len, valid_len);

        // Resume writing new records
        assert!(world.set_block(2, 60, 2, Block::Dirt));
        world.commit_tick().expect("commit tick 1");
        world.flush().expect("flush journal");
    }

    // Verify both original and new records survived
    {
        let mut world = World::open(test_dir.path(), generator).expect("reopen world");
        assert_eq!(world.current_tick(), 2);
        assert_eq!(world.block(1, 60, 1), Some(Block::Stone));
        assert_eq!(world.block(2, 60, 2), Some(Block::Dirt));
    }
}

#[test]
fn test_atomic_compaction_and_deduplication() {
    let test_dir = TestDir::new("compaction-dedup");
    let generator = Generator::new(2026);

    {
        let mut world = World::open(test_dir.path(), generator).expect("open world");

        // Mutate same cell across multiple ticks
        assert!(world.set_block(5, 60, 5, Block::Dirt));
        world.commit_tick().expect("commit tick 0");

        assert!(world.set_block(5, 60, 5, Block::Stone));
        world.commit_tick().expect("commit tick 1");

        assert!(world.set_block(5, 60, 5, Block::Sand));
        world.commit_tick().expect("commit tick 2");

        assert!(world.set_block(6, 60, 6, Block::Log));
        world.commit_tick().expect("commit tick 3");

        assert_eq!(world.block(5, 60, 5), Some(Block::Sand));

        // Atomic compaction
        world.compact().expect("compact world");

        let cp_path = test_dir.path().join("checkpoint.bin");
        assert!(cp_path.is_file(), "checkpoint.bin must exist");

        // Journal log must be reset to header length
        let log_len = std::fs::metadata(test_dir.path().join("journal.log"))
            .expect("log metadata")
            .len();
        assert_eq!(log_len, 32);
    }

    // Reopen from compacted state
    {
        let mut world = World::open(test_dir.path(), generator).expect("reopen world");
        assert_eq!(world.current_tick(), 4);

        // Deduplication: only 2 unique cells mutated
        assert_eq!(world.mutations().len(), 2);
        assert_eq!(world.block(5, 60, 5), Some(Block::Sand));
        assert_eq!(world.block(6, 60, 6), Some(Block::Log));
    }
}

#[test]
fn test_crash_during_compaction_orphaned_temp_file_ignored() {
    let test_dir = TestDir::new("orphaned-temp");
    let generator = Generator::new(2026);

    {
        let mut world = World::open(test_dir.path(), generator).expect("open world");
        assert!(world.set_block(3, 60, 3, Block::Stone));
        world.commit_tick().expect("commit tick 0");
        world.compact().expect("compact world");
    }

    // Simulate orphaned temp file from a previous interrupted compaction
    let orphaned_path = test_dir.path().join(".checkpoint.tmp-crash-leftover");
    std::fs::write(&orphaned_path, b"garbage data from interrupted compaction")
        .expect("write temp");

    // Reopening the world ignores the orphaned temp file and reads checkpoint.bin cleanly
    {
        let mut world = World::open(test_dir.path(), generator).expect("open world");
        assert_eq!(world.block(3, 60, 3), Some(Block::Stone));

        assert!(!orphaned_path.exists(), "orphans are removed at open");

        // Compaction succeeds without interference
        assert!(world.set_block(4, 60, 4, Block::Dirt));
        world.commit_tick().expect("commit tick 1");
        world.compact().expect("compact world");
    }
}

#[test]
fn test_corrupted_checkpoint_crc_failure() {
    let test_dir = TestDir::new("corrupted-checkpoint");
    let generator = Generator::new(2026);

    {
        let mut world = World::open(test_dir.path(), generator).expect("open world");
        assert!(world.set_block(0, 60, 0, Block::Sand));
        world.commit_tick().expect("commit tick");
        world.compact().expect("compact world");
    }

    // Corrupt one byte of checkpoint.bin
    let cp_path = test_dir.path().join("checkpoint.bin");
    let mut bytes = std::fs::read(&cp_path).expect("read checkpoint");
    let last_idx = bytes.len() - 1;
    bytes[last_idx] ^= 0xff;
    std::fs::write(&cp_path, &bytes).expect("write corrupted checkpoint");

    // Attempting to open the world must fail with a CRC error
    let result = World::open(test_dir.path(), generator);
    match result {
        Err(WorldError::Corrupted(msg)) => {
            assert!(
                msg.contains("CRC32 mismatch"),
                "expected CRC32 mismatch message, got {msg}"
            );
        }
        other => panic!("expected WorldError::Corrupted, got {other:?}"),
    }
}

#[test]
fn test_seed_mismatch_rejection() {
    let test_dir = TestDir::new("seed-mismatch");

    {
        let mut world = World::open(test_dir.path(), Generator::new(55555)).expect("open world");
        assert!(world.set_block(0, 60, 0, Block::Dirt));
        world.commit_tick().expect("commit tick");
        world.compact().expect("compact world");
    }

    // Attempting to open the world with a different seed must be rejected
    let result = World::open(test_dir.path(), Generator::new(99999));
    match result {
        Err(WorldError::SeedMismatch { expected, found }) => {
            assert_eq!(expected, 99999);
            assert_eq!(found, 55555);
        }
        other => panic!("expected WorldError::SeedMismatch, got {other:?}"),
    }
}

#[test]
fn test_mid_journal_corruption_rejected() {
    let test_dir = TestDir::new("mid-journal-corruption");
    let generator = Generator::new(2026);

    {
        let mut world = World::open(test_dir.path(), generator).expect("open world");
        assert!(world.set_block(0, 60, 0, Block::Dirt));
        world.commit_tick().expect("commit tick 0");
        assert!(world.set_block(1, 60, 1, Block::Stone));
        world.commit_tick().expect("commit tick 1");
    }

    // Corrupt bytes in the 1st record payload, while 2nd record remains in file
    let log_path = test_dir.path().join("journal.log");
    let mut bytes = std::fs::read(&log_path).expect("read log");
    // Offset 32 is start of first record; offset 50 is inside first record's coordinate payload,
    // which starts after the 16-byte record header
    bytes[50] ^= 0xff;
    std::fs::write(&log_path, &bytes).expect("write corrupted log");

    // Opening world must fail loudly on mid-journal corruption, rather than truncating
    let result = World::open(test_dir.path(), generator);
    match result {
        Err(WorldError::Corrupted(msg)) => {
            assert!(
                msg.contains("checksum mismatch"),
                "expected checksum mismatch message, got {msg}"
            );
        }
        other => panic!("expected WorldError::Corrupted, got {other:?}"),
    }
}

#[test]
fn test_non_monotonic_sequence_rejected() {
    let test_dir = TestDir::new("non-monotonic-sequence");
    let generator = Generator::new(2026);

    {
        let mut world = World::open(test_dir.path(), generator).expect("open world");
        assert!(world.set_block(0, 60, 0, Block::Dirt));
        world.commit_tick().expect("commit tick 0");
    }

    // Manually forge a record with tick sequence ID regressing backwards
    let log_path = test_dir.path().join("journal.log");
    let mut bytes = std::fs::read(&log_path).expect("read log");

    // Create a forged record with tick = 0 (when recovered_tick is 1)
    let mut record = Vec::new();
    record.extend_from_slice(&0u64.to_be_bytes()); // tick 0 < recovered_tick 1
    record.extend_from_slice(&1u32.to_be_bytes()); // count 1
    let header_crc = crc32fast::hash(&record);
    record.extend_from_slice(&header_crc.to_be_bytes());
    record.extend_from_slice(&10i64.to_be_bytes()); // x
    record.extend_from_slice(&60i64.to_be_bytes()); // y
    record.extend_from_slice(&10i64.to_be_bytes()); // z
    record.push(Block::Sand as u8); // block
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&record);
    let crc = hasher.finalize();
    record.extend_from_slice(&crc.to_be_bytes());

    bytes.extend_from_slice(&record);
    std::fs::write(&log_path, &bytes).expect("write non-monotonic log");

    let result = World::open(test_dir.path(), generator);
    match result {
        Err(WorldError::Corrupted(msg)) => {
            assert!(
                msg.contains("non-monotonic sequence ID"),
                "expected non-monotonic sequence ID message, got {msg}"
            );
        }
        other => panic!("expected WorldError::Corrupted, got {other:?}"),
    }
}

fn journal_path(dir: &TestDir) -> std::path::PathBuf {
    dir.path().join("journal.log")
}

fn checkpoint_path(dir: &TestDir) -> std::path::PathBuf {
    dir.path().join("checkpoint.bin")
}

/// Commits `count` ticks that each write a distinct cell.
fn commit_ticks(world: &mut World, count: u64) {
    for _ in 0..count {
        let x = world.current_tick() as i64;
        assert!(world.set_block(x, 60, 0, Block::Stone));
        world.commit_tick().expect("commit tick");
    }
}

/// Asserts the reopened world holds exactly ticks `0..expected_tick`.
fn assert_recovered(dir: &TestDir, generator: Generator, expected_tick: u64) -> World {
    let world = World::open(dir.path(), generator).expect("recover world");
    assert_eq!(world.current_tick(), expected_tick);
    assert_eq!(world.mutations().len(), expected_tick as usize);
    for x in 0..expected_tick as i64 {
        assert_eq!(world.mutations().get(&(x, 60, 0)), Some(&Block::Stone));
    }
    world
}

/// Offset of record `index` in a journal of single-mutation records:
/// 32-byte file header, then 16 + 25 + 4 bytes per record.
fn record_offset(index: usize) -> usize {
    32 + index * 45
}

#[test]
fn record_layout_matches_independently_computed_bytes() {
    let dir = TestDir::new("record-layout");
    let generator = Generator::new(7);
    let mut world = World::open(dir.path(), generator).expect("open world");
    assert!(world.set_block(1, 2, -1, Block::Dirt));
    world.commit_tick().expect("commit tick");
    drop(world);

    let bytes = std::fs::read(journal_path(&dir)).expect("read log");
    // CRC-32/ISO-HDLC values below were computed with Python's zlib, so a
    // layout change cannot pass by matching its own encoder.
    let tick_and_count = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
    assert_eq!(bytes[32..44], tick_and_count);
    assert_eq!(bytes[44..48], [0x0C, 0xD2, 0xF6, 0xF9]);
    assert_eq!(bytes[48..56], [0, 0, 0, 0, 0, 0, 0, 1]);
    assert_eq!(bytes[56..64], [0, 0, 0, 0, 0, 0, 0, 2]);
    assert_eq!(bytes[64..72], [0xFF; 8]);
    assert_eq!(bytes[72], 3);
    assert_eq!(bytes[73..77], [0x35, 0x92, 0x66, 0xAE]);
    assert_eq!(bytes.len(), record_offset(1));
}

#[test]
fn every_truncation_point_recovers_the_complete_records() {
    let dir = TestDir::new("every-truncation");
    let generator = Generator::new(11);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 3);
    drop(world);
    let full = std::fs::read(journal_path(&dir)).expect("read log");

    for len in journal::HEADER_SIZE..=full.len() {
        std::fs::write(journal_path(&dir), &full[..len]).expect("truncate log");
        let complete = (len - 32) / 45;
        let mut world = assert_recovered(&dir, generator, complete as u64);
        assert_eq!(
            std::fs::metadata(journal_path(&dir))
                .expect("metadata")
                .len(),
            record_offset(complete) as u64,
            "torn bytes must be removed at length {len}"
        );

        // The next tick lands on the record boundary and replays afterwards.
        commit_ticks(&mut world, 1);
        drop(world);
        drop(assert_recovered(&dir, generator, complete as u64 + 1));
    }
}

#[test]
fn a_journal_shorter_than_its_header_is_rejected() {
    let dir = TestDir::new("short-header");
    let generator = Generator::new(11);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 1);
    drop(world);
    let full = std::fs::read(journal_path(&dir)).expect("read log");

    for len in 0..journal::HEADER_SIZE {
        std::fs::write(journal_path(&dir), &full[..len]).expect("truncate log");
        assert!(matches!(
            World::open(dir.path(), generator),
            Err(WorldError::Corrupted(_))
        ));
    }
}

#[test]
fn every_bit_flip_before_the_final_record_is_reported() {
    let dir = TestDir::new("bit-flips");
    let generator = Generator::new(13);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 3);
    drop(world);
    let full = std::fs::read(journal_path(&dir)).expect("read log");

    for offset in 0..record_offset(2) {
        for bit in 0..8 {
            let mut damaged = full.clone();
            damaged[offset] ^= 1 << bit;
            std::fs::write(journal_path(&dir), &damaged).expect("write damaged log");
            let result = World::open(dir.path(), generator);
            assert!(
                result.is_err(),
                "flip at byte {offset} bit {bit} must not be accepted"
            );
            assert_eq!(
                std::fs::read(journal_path(&dir)).expect("read log"),
                damaged,
                "a rejected journal must be left untouched"
            );
        }
    }
}

#[test]
fn a_bit_flip_in_the_final_record_costs_at_most_that_tick() {
    let dir = TestDir::new("final-record-flips");
    let generator = Generator::new(13);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 3);
    drop(world);
    let full = std::fs::read(journal_path(&dir)).expect("read log");

    for offset in record_offset(2)..full.len() {
        for bit in 0..8 {
            let mut damaged = full.clone();
            damaged[offset] ^= 1 << bit;
            std::fs::write(journal_path(&dir), &damaged).expect("write damaged log");
            drop(assert_recovered(&dir, generator, 2));
        }
    }
}

#[test]
fn flipped_record_count_does_not_truncate_committed_ticks() {
    let dir = TestDir::new("flipped-count");
    let generator = Generator::new(17);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 2);
    drop(world);

    let mut bytes = std::fs::read(journal_path(&dir)).expect("read log");
    // Count is at record offset 8..12: 1 becomes 3, longer than the file.
    bytes[record_offset(0) + 11] = 3;
    std::fs::write(journal_path(&dir), &bytes).expect("write damaged log");

    assert!(matches!(
        World::open(dir.path(), generator),
        Err(WorldError::Corrupted(_))
    ));
    assert_eq!(
        std::fs::read(journal_path(&dir)).expect("read log"),
        bytes,
        "committed records must survive a failed recovery"
    );
}

#[test]
fn damage_longer_than_one_record_is_corruption() {
    let dir = TestDir::new("long-damage");
    let generator = Generator::new(19);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 1);
    drop(world);

    let mut file = OpenOptions::new()
        .append(true)
        .open(journal_path(&dir))
        .expect("open log");
    file.write_all(&vec![0xA5; 16 + 100_000 * 25 + 4 + 1])
        .expect("append damage");
    drop(file);

    assert!(matches!(
        World::open(dir.path(), generator),
        Err(WorldError::Corrupted(_))
    ));
}

#[test]
fn zero_filled_tail_is_treated_as_a_torn_write() {
    let dir = TestDir::new("zero-tail");
    let generator = Generator::new(19);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 2);
    drop(world);

    // Some filesystems extend the file before its data blocks reach disk.
    let mut file = OpenOptions::new()
        .append(true)
        .open(journal_path(&dir))
        .expect("open log");
    file.write_all(&[0u8; 45]).expect("append zeros");
    drop(file);

    drop(assert_recovered(&dir, generator, 2));
}

#[test]
fn replayed_duplicate_tick_is_corruption_not_a_torn_tail() {
    let dir = TestDir::new("duplicate-tick");
    let generator = Generator::new(23);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 1);
    drop(world);

    let mut bytes = std::fs::read(journal_path(&dir)).expect("read log");
    let record = bytes[record_offset(0)..].to_vec();
    bytes.extend_from_slice(&record);
    std::fs::write(journal_path(&dir), &bytes).expect("write log");

    assert!(matches!(
        World::open(dir.path(), generator),
        Err(WorldError::Corrupted(message)) if message.contains("non-monotonic")
    ));
}

#[test]
fn append_sync_failure_never_leaves_an_unrecoverable_log() {
    for failing in [
        vec!["append.sync"],
        vec!["append.write"],
        vec!["append.partial_write"],
        vec!["append.sync", "append.rollback"],
        vec!["append.partial_write", "append.rollback"],
    ] {
        let dir = TestDir::new("append-failure");
        let generator = Generator::new(29);
        let mut world = World::open(dir.path(), generator).expect("open world");
        commit_ticks(&mut world, 2);

        assert!(world.set_block(2, 60, 0, Block::Stone));
        journal::faults::fail_steps(&failing);
        let error = world
            .commit_tick()
            .expect_err("commit must report the fault");
        assert!(matches!(error, WorldError::Io(_)));
        assert_eq!(journal::faults::clear().len(), failing.len());

        // The tick must not be acknowledged by a retry on the same handle.
        assert_eq!(world.current_tick(), 2);
        assert!(matches!(
            world.commit_tick(),
            Err(WorldError::JournalUnusable)
        ));
        assert!(matches!(world.flush(), Err(WorldError::JournalUnusable)));
        assert!(matches!(world.compact(), Err(WorldError::JournalUnusable)));
        drop(world);

        // A successful rollback drops the tick. Without one, a complete record
        // recovers as that tick; either way the log stays replayable.
        let survived = failing == ["append.sync", "append.rollback"];
        let expected = if survived { 3 } else { 2 };
        let mut world = assert_recovered(&dir, generator, expected);
        commit_ticks(&mut world, 1);
        drop(world);
        drop(assert_recovered(&dir, generator, expected + 1));
    }
}

#[test]
fn failed_flush_makes_the_journal_unusable() {
    let dir = TestDir::new("flush-failure");
    let generator = Generator::new(29);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 1);

    journal::faults::fail_steps(&["flush.sync"]);
    assert!(matches!(world.flush(), Err(WorldError::Io(_))));
    journal::faults::clear();
    assert!(matches!(
        world.commit_tick(),
        Err(WorldError::JournalUnusable)
    ));
    drop(world);
    drop(assert_recovered(&dir, generator, 1));
}

#[test]
fn checkpoint_directory_sync_failure_preserves_the_journal() {
    let dir = TestDir::new("dir-sync-failure");
    let generator = Generator::new(31);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 3);
    let journal_before = std::fs::read(journal_path(&dir)).expect("read log");

    journal::faults::fail_steps(&["checkpoint.dir_sync"]);
    assert!(matches!(world.compact(), Err(WorldError::Io(_))));
    assert_eq!(journal::faults::clear(), ["checkpoint.dir_sync"]);
    assert_eq!(
        std::fs::read(journal_path(&dir)).expect("read log"),
        journal_before,
        "the journal must outlive an unconfirmed checkpoint"
    );

    // Power loss can roll the rename back; the journal alone still recovers.
    std::fs::remove_file(checkpoint_path(&dir)).expect("roll back rename");
    commit_ticks(&mut world, 1);
    drop(world);
    drop(assert_recovered(&dir, generator, 4));
}

#[test]
fn appends_after_a_failed_compaction_replay_past_the_checkpoint() {
    let dir = TestDir::new("append-after-failed-compaction");
    let generator = Generator::new(31);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 3);

    journal::faults::fail_steps(&["log.write"]);
    assert!(matches!(world.compact(), Err(WorldError::Io(_))));
    journal::faults::clear();
    drop(world);

    // The checkpoint is installed and the old journal stays in place.
    let world = assert_recovered(&dir, generator, 3);
    drop(world);

    let dir = TestDir::new("append-after-failed-checkpoint-sync");
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 3);
    journal::faults::fail_steps(&["checkpoint.dir_sync"]);
    assert!(matches!(world.compact(), Err(WorldError::Io(_))));
    journal::faults::clear();
    commit_ticks(&mut world, 2);
    drop(world);

    // Ticks 3 and 4 exist only in the journal and must not be dropped.
    let world = assert_recovered(&dir, generator, 5);
    assert!(checkpoint_path(&dir).is_file());
    drop(world);
    drop(assert_recovered(&dir, generator, 5));
}

#[test]
fn journal_replacement_failure_after_rename_refuses_further_appends() {
    let dir = TestDir::new("log-dir-sync-failure");
    let generator = Generator::new(31);
    let mut world = World::open(dir.path(), generator).expect("open world");
    commit_ticks(&mut world, 2);

    journal::faults::fail_steps(&["log.dir_sync"]);
    assert!(matches!(world.compact(), Err(WorldError::Io(_))));
    journal::faults::clear();
    assert!(matches!(
        world.commit_tick(),
        Err(WorldError::JournalUnusable)
    ));
    drop(world);
    drop(assert_recovered(&dir, generator, 2));
}

#[test]
fn crash_at_every_compaction_step_recovers_all_ticks() {
    let generator = Generator::new(37);
    let mut steps_seen = Vec::new();
    for skipped in 0.. {
        let dir = TestDir::new("compaction-crash");
        let mut world = World::open(dir.path(), generator).expect("open world");
        commit_ticks(&mut world, 3);
        world.compact().expect("first compaction");
        commit_ticks(&mut world, 2);

        journal::faults::fail_after(skipped);
        let result = world.compact();
        let fired = journal::faults::clear();
        drop(world);
        if fired.is_empty() {
            result.expect("compaction with no fault");
            drop(assert_recovered(&dir, generator, 5));
            break;
        }
        assert!(result.is_err());
        steps_seen.extend(fired);

        let mut world = assert_recovered(&dir, generator, 5);
        commit_ticks(&mut world, 1);
        drop(world);
        drop(assert_recovered(&dir, generator, 6));
    }
    assert_eq!(
        steps_seen,
        [
            "checkpoint.write",
            "checkpoint.sync",
            "checkpoint.rename",
            "checkpoint.dir_sync",
            "log.write",
            "log.sync",
            "log.rename",
            "log.dir_sync",
        ]
    );
}

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
