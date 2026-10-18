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
