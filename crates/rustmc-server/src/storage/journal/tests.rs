//! Unit tests for `storage::journal`.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

fn temp_path() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir()
        .join(format!(
            "rustmc-journal-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ))
        .join("journal.bin")
}

fn edit(x: i32, state: u32) -> BlockEdit {
    BlockEdit {
        x,
        y: -64,
        z: -x,
        state,
    }
}

#[test]
fn synced_edits_replay_in_order_and_reset_empties_the_journal() {
    let path = temp_path();
    let (mut journal, replay) = Journal::open(&path).expect("open");
    assert!(replay.is_empty());
    assert!(path.exists(), "a new journal exists before its first sync");
    journal.append(edit(1, 10)).expect("append");
    journal.append(edit(-2, 20)).expect("append");
    journal.sync().expect("sync");
    drop(journal);
    let (mut journal, replay) = Journal::open(&path).expect("reopen");
    assert_eq!(replay, vec![edit(1, 10), edit(-2, 20)]);
    journal.reset().expect("reset");
    journal.append(edit(3, 30)).expect("append");
    journal.sync().expect("sync");
    drop(journal);
    assert_eq!(Journal::open(&path).expect("reopen").1, vec![edit(3, 30)]);
    let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
}

#[test]
fn a_torn_or_damaged_tail_is_dropped_and_later_edits_still_replay() {
    let path = temp_path();
    let (mut journal, _) = Journal::open(&path).expect("open");
    journal.append(edit(1, 10)).expect("append");
    journal.sync().expect("sync");
    drop(journal);
    let mut bytes = std::fs::read(&path).expect("read");
    let mut damaged = edit(2, 20).encode();
    damaged[3] ^= 0xff;
    bytes.extend_from_slice(&damaged);
    bytes.extend_from_slice(&edit(3, 30).encode()[..7]);
    std::fs::write(&path, &bytes).expect("damage");
    let (mut journal, replay) = Journal::open(&path).expect("reopen");
    assert_eq!(replay, vec![edit(1, 10)], "stops at the damaged record");
    journal.append(edit(4, 40)).expect("append");
    journal.sync().expect("sync");
    drop(journal);
    assert_eq!(
        Journal::open(&path).expect("reopen").1,
        vec![edit(1, 10), edit(4, 40)]
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
}
