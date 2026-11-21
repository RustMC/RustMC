//! Unit tests for `server::world::view`.

use std::sync::mpsc::sync_channel;
use std::time::Duration;

use super::*;
use crate::server::connection::Event;
use crate::server::world::terrain::TerrainPalette;
use crate::server::world::workers::{ChunkWorkers, ColumnSource};
use crate::storage::world_files::WorldFiles;
use crate::world::Generator;

fn ids(out: &[Outgoing]) -> Vec<u32> {
    out.iter()
        .map(|o| match o {
            Outgoing::Packet(p) => p.id,
            Outgoing::Shared(p) => p.id,
            other => panic!("unexpected {other:?}"),
        })
        .collect()
}

#[test]
fn recentering_moves_the_cache_centre_and_forgets_far_chunks() {
    let mut view = View::new((0, 0), 2);
    assert!(view.contains((2, -2)));
    assert!(!view.contains((3, 0)));
    view.sent.insert((-2, 0));
    view.sent.insert((2, 0));
    assert!(view.recenter((0, 0)).is_empty());
    let out = view.recenter((1, 0));
    assert_eq!(
        ids(&out),
        [
            clientbound::SET_CHUNK_CACHE_CENTER,
            clientbound::FORGET_LEVEL_CHUNK
        ]
    );
    let Outgoing::Packet(forget) = &out[1] else {
        panic!("forget expected");
    };
    let mut expected = 0i32.to_be_bytes().to_vec();
    expected.extend((-2i32).to_be_bytes());
    assert_eq!(forget.body, expected, "chunk Z then chunk X");
    assert!(view.has_sent((2, 0)));
}

#[test]
fn the_client_rate_sizes_later_batches_within_bounds() {
    let mut view = View::new((0, 0), 2);
    view.batch_received(4.2);
    assert_eq!(view.batch_size, 5);
    view.batch_received(1e9);
    assert_eq!(view.batch_size, MAX_BATCH);
    view.batch_received(f32::NAN);
    assert_eq!(view.batch_size, MAX_BATCH, "invalid rates are ignored");
}

#[test]
fn batches_send_generated_chunks_nearest_first_one_batch_at_a_time() {
    let tables = Arc::new(crate::server::world::terrain::tests::tables());
    let palette = TerrainPalette::resolve(&tables).expect("palette");
    let world = std::env::temp_dir().join(format!("rustmc-view-test-{}", std::process::id()));
    let source = Arc::new(ColumnSource {
        files: WorldFiles::new(&world),
        generator: Generator::new(1),
        palette,
        tables: Arc::clone(&tables),
    });
    let (results, inbox) = sync_channel(64);
    let workers = ChunkWorkers::spawn(2, source, results).expect("workers");
    let mut store = ChunkStore::new(workers, tables);
    let mut view = View::new((0, 0), 1);
    assert!(
        view.next_batch(&mut store).is_none(),
        "nothing generated yet"
    );
    for _ in 0..9 {
        match inbox.recv_timeout(Duration::from_secs(30)).expect("chunk") {
            Event::ChunkReady(ready) => store.insert(*ready),
            other => panic!("unexpected {other:?}"),
        }
    }
    let batch = view.next_batch(&mut store).expect("batch");
    let ids = ids(&batch);
    assert_eq!(ids.first(), Some(&clientbound::CHUNK_BATCH_START));
    assert_eq!(ids.last(), Some(&clientbound::CHUNK_BATCH_FINISHED));
    assert_eq!(ids.len(), 2 + INITIAL_BATCH);
    assert!(view.has_sent((0, 0)), "the centre goes first");
    assert!(view.next_batch(&mut store).is_none(), "one batch in flight");
    view.batch_received(1.0);
    assert!(
        view.next_batch(&mut store).is_none(),
        "every chunk already sent"
    );
}
