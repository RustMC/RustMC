//! Unit tests for `java_preview`.

use super::*;
use crate::world::Terrain;

fn read_varint(input: &[u8]) -> (u32, usize) {
    crate::java_wire::read_varint(input)
        .expect("valid test packet VarInt")
        .expect("complete test packet VarInt")
}

fn packet_ids(mut stream: &[u8]) -> Vec<u32> {
    let mut ids = Vec::new();
    while !stream.is_empty() {
        let (length, prefix) = read_varint(stream);
        let packet = &stream[prefix..prefix + length as usize];
        ids.push(read_varint(packet).0);
        stream = &stream[prefix + length as usize..];
    }
    ids
}

fn manifest() -> RegistryManifest {
    RegistryManifest {
        tags: Vec::new(),
        registries: vec![
            (
                "minecraft:dimension_type".into(),
                vec!["minecraft:overworld".into()],
            ),
            (
                "minecraft:world_clock".into(),
                vec!["minecraft:overworld".into()],
            ),
            (
                "minecraft:worldgen/biome".into(),
                Biome::ALL.iter().map(|b| b.identifier().into()).collect(),
            ),
        ],
    }
}

#[test]
fn vanilla_ready_chunks_share_a_bounded_acknowledged_batch() {
    let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
    preview.teleport_acknowledged = true;
    let (tx, rx) = mpsc::sync_channel(8);
    let positions = [(0, 0), (1, 0), (0, 1)];
    for position in positions {
        tx.send(BuiltChunk {
            position,
            packet: Ok(Some(frame(CHUNK, b"test"))),
            generation_us: 4,
            encoding_us: 2,
        })
        .unwrap();
    }
    preview.vanilla = Some(VanillaWorker {
        requests: None,
        results: rx,
        handles: Vec::new(),
        in_flight: positions.into_iter().collect(),
        pending: None,
        capacity: 8,
        current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
        cache: None,
    });
    let (stream, generation_us, encoding_us) = preview.next_chunk().unwrap();
    let ids = packet_ids(&stream);
    assert_eq!(ids.iter().filter(|id| **id == CHUNK).count(), 3);
    assert_eq!(ids.iter().filter(|id| **id == BATCH_START).count(), 1);
    assert_eq!(ids.iter().filter(|id| **id == BATCH_END).count(), 1);
    assert_eq!((generation_us, encoding_us), (12, 6));
    assert!(preview.awaiting_batch);
    assert_eq!(preview.sent.len(), 3);
}

#[test]
fn vanilla_batch_defers_packet_when_byte_budget_is_full() {
    let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
    preview.teleport_acknowledged = true;
    let (tx, rx) = mpsc::sync_channel(8);
    for position in [(0, 0), (1, 0)] {
        tx.send(BuiltChunk {
            position,
            packet: Ok(Some(frame(CHUNK, &vec![0; 400_000]))),
            generation_us: 0,
            encoding_us: 0,
        })
        .unwrap();
    }
    preview.vanilla = Some(VanillaWorker {
        requests: None,
        results: rx,
        handles: Vec::new(),
        in_flight: [(0, 0), (1, 0)].into_iter().collect(),
        pending: None,
        capacity: 8,
        current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
        cache: None,
    });
    let first = preview.next_chunk().unwrap().0;
    assert_eq!(
        packet_ids(&first).iter().filter(|id| **id == CHUNK).count(),
        1
    );
    assert!(first.len() < MAX_BATCH_BYTES + 32);
    assert!(preview.vanilla.as_ref().unwrap().pending.is_some());
    preview.awaiting_batch = false;
    let second = preview.next_chunk().unwrap().0;
    assert_eq!(
        packet_ids(&second)
            .iter()
            .filter(|id| **id == CHUNK)
            .count(),
        1
    );
    assert!(preview.vanilla.as_ref().unwrap().pending.is_none());
    assert_eq!(preview.sent.len(), 2);
}

#[test]
fn vanilla_move_sends_unload_without_waiting_for_generated_chunk() {
    let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
    preview.teleport_acknowledged = true;
    preview.sent.insert((0, 0));
    let (_tx, results) = mpsc::sync_channel(1);
    preview.vanilla = Some(VanillaWorker {
        requests: None,
        results,
        handles: Vec::new(),
        in_flight: BTreeSet::new(),
        pending: None,
        capacity: 1,
        current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
        cache: None,
    });
    preview.move_to(48.5, 80.0, 0.5).unwrap();
    let (stream, _, _) = preview.next_chunk().expect("unload-only stream");
    assert_eq!(packet_ids(&stream), vec![CACHE_CENTER, FORGET_CHUNK]);
    assert!(!preview.sent.contains(&(0, 0)));
    assert!(!preview.awaiting_batch);
}

#[test]
fn cancelled_far_chunk_releases_slot_without_failing_preview() {
    let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
    preview.teleport_acknowledged = true;
    let (tx, results) = mpsc::sync_channel(1);
    tx.send(BuiltChunk {
        position: (2, 2),
        packet: Ok(None),
        generation_us: 1,
        encoding_us: 0,
    })
    .unwrap();
    preview.vanilla = Some(VanillaWorker {
        requests: None,
        results,
        handles: Vec::new(),
        in_flight: [(2, 2)].into_iter().collect(),
        pending: None,
        capacity: 1,
        current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
        cache: None,
    });
    assert!(preview.next_chunk().is_none());
    assert!(!preview.failed);
    assert!(preview.vanilla.as_ref().unwrap().in_flight.is_empty());
    assert_eq!(preview.next_view_position, 0);
}

#[test]
fn completed_view_is_not_rescanned_until_center_changes() {
    let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
    preview.teleport_acknowledged = true;
    preview.sent.extend(preview.view_order.iter().copied());
    let (_tx, results) = mpsc::sync_channel(1);
    preview.vanilla = Some(VanillaWorker {
        requests: None,
        results,
        handles: Vec::new(),
        in_flight: BTreeSet::new(),
        pending: None,
        capacity: 1,
        current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
        cache: None,
    });
    assert!(preview.next_chunk().is_none());
    assert_eq!(preview.next_view_position, preview.view_order.len());
    assert!(preview.next_chunk().is_none());
    assert_eq!(preview.next_view_position, preview.view_order.len());
    preview.move_to(16.5, 80.0, 0.5).unwrap();
    assert_eq!(preview.next_view_position, 0);
    let stream = preview.next_chunk().expect("old edge unload").0;
    assert!(packet_ids(&stream).contains(&FORGET_CHUNK));
    assert!(preview.next_view_position < preview.view_order.len());
}

#[test]
fn stationary_view_uses_full_bounded_pool_for_outer_chunks() {
    let mut preview = Preview::new(2027, 5, Terrain::Preview, &manifest()).unwrap();
    preview.teleport_acknowledged = true;
    preview.last_center_change = Instant::now() - MOVING_PRIORITY_WINDOW;
    preview.sent.extend(
        preview
            .view_order
            .iter()
            .copied()
            .filter(|(x, z)| x.abs().max(z.abs()) <= MOVING_NEAR_RADIUS),
    );
    let (requests, queued) = mpsc::sync_channel(8);
    let (_results_sender, results) = mpsc::sync_channel(8);
    preview.vanilla = Some(VanillaWorker {
        requests: Some(requests),
        results,
        handles: Vec::new(),
        in_flight: BTreeSet::new(),
        pending: None,
        capacity: 8,
        current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
        cache: None,
    });
    assert!(preview.next_chunk().is_none());
    assert_eq!(preview.vanilla.as_ref().unwrap().in_flight.len(), 8);
    let queued: Vec<_> = queued.try_iter().map(|request| request.position).collect();
    assert_eq!(queued.len(), 8);
    assert!(queued.iter().all(|(x, z)| x.abs().max(z.abs()) == 4));
}

#[test]
fn nearby_hole_blocks_outer_requests_and_discards_early_far_result() {
    let mut preview = Preview::new(2027, 5, Terrain::Preview, &manifest()).unwrap();
    preview.teleport_acknowledged = true;
    preview.last_center_change = Instant::now() - MOVING_PRIORITY_WINDOW;
    preview.sent.extend(
        preview
            .view_order
            .iter()
            .copied()
            .filter(|(x, z)| x.abs().max(z.abs()) <= MOVING_NEAR_RADIUS && (*x, *z) != (0, 0)),
    );
    let (requests, queued) = mpsc::sync_channel(8);
    let (results_sender, results) = mpsc::sync_channel(8);
    results_sender
        .send(BuiltChunk {
            position: (4, 0),
            packet: Ok(Some(frame(CHUNK, &[4]))),
            generation_us: 0,
            encoding_us: 0,
        })
        .unwrap();
    results_sender
        .send(BuiltChunk {
            position: (0, 0),
            packet: Ok(Some(frame(CHUNK, &[0]))),
            generation_us: 0,
            encoding_us: 0,
        })
        .unwrap();
    preview.vanilla = Some(VanillaWorker {
        requests: Some(requests),
        results,
        handles: Vec::new(),
        in_flight: [(4, 0), (0, 0)].into_iter().collect(),
        pending: None,
        capacity: 8,
        current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
        cache: None,
    });
    let (stream, _, _) = preview.next_chunk().expect("nearby chunk is ready");
    assert_eq!(
        packet_ids(&stream),
        vec![CACHE_CENTER, BATCH_START, CHUNK, BATCH_END]
    );
    assert!(preview.sent.contains(&(0, 0)));
    assert!(!preview.sent.contains(&(4, 0)));
    assert!(queued.try_iter().next().is_none());
    preview.awaiting_batch = false;
    preview.next_chunk();
    assert!(
        preview
            .vanilla
            .as_ref()
            .unwrap()
            .in_flight
            .iter()
            .any(|(x, z)| x.abs().max(z.abs()) == 4)
    );
}

#[test]
fn moving_view_keeps_square_shells_and_prefers_travel_direction() {
    let east = ordered_view((0, 0), 3, (1, 0));
    let west = ordered_view((0, 0), 3, (-1, 0));
    assert_eq!(east.len(), 49);
    assert_eq!(east[0], (0, 0));
    assert_eq!(east[1], (1, -1));
    assert_eq!(west[1], (-1, -1));
    assert!(
        east.windows(2).all(
            |pair| pair[0].0.abs().max(pair[0].1.abs()) <= pair[1].0.abs().max(pair[1].1.abs())
        )
    );
    assert_eq!(east.iter().copied().collect::<BTreeSet<_>>().len(), 49);
    assert_eq!(west.iter().copied().collect::<BTreeSet<_>>().len(), 49);
}

#[test]
fn maximum_view_uses_bounded_acknowledged_batches_and_unloads_old_view() {
    assert!(Preview::new(2026, 33, Terrain::Preview, &manifest()).is_none());
    let mut preview = Preview::new(2026, 32, Terrain::Preview, &manifest()).unwrap();
    assert!(preview.next_chunk().is_none());
    preview.teleport_acknowledged = true;
    let (first, _, _) = preview.next_chunk().unwrap();
    assert!(first.len() <= MAX_BATCH_BYTES + 16);
    assert!(preview.sent.len() > 1);
    assert!(preview.sent.len() <= MAX_CHUNKS_PER_BATCH);
    assert!(preview.next_chunk().is_none());
    preview.awaiting_batch = false;
    preview.move_to(1024.5, 80.0, -1024.5).unwrap();
    assert_eq!(preview.center, (64, -65));
    let (moved, _, _) = preview.next_chunk().unwrap();
    assert_ne!(first, moved);
    assert!(!preview.sent.contains(&(0, 0))); // Old view was unloaded.
    assert!(preview.sent.len() <= MAX_CHUNKS_PER_BATCH);
    assert!(preview.move_to(f64::NAN, 80.0, 0.0).is_err());
}

#[test]
fn move_to_accepts_vanilla_position_range() {
    let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
    preview.teleport_acknowledged = true;

    preview.move_to(5_000_000.5, 1000.0, 0.5).unwrap();
    assert_eq!(preview.center, (312_500, 0));

    preview
        .move_to(-30_000_000.0, -20_000_000.0, 30_000_000.0)
        .unwrap();
    assert_eq!(preview.center, (-1_875_000, 1_875_000));

    for (x, y, z) in [
        (30_000_000.5, 0.0, 0.0),
        (0.0, 0.0, -30_000_000.5),
        (0.0, 20_000_000.5, 0.0),
        (0.0, -20_000_000.5, 0.0),
    ] {
        assert!(preview.move_to(x, y, z).is_err());
    }
}

#[test]
fn walking_one_chunk_forgets_old_edge_and_rejoin_starts_fresh() {
    let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
    preview.teleport_acknowledged = true;
    let first = preview.next_chunk().unwrap().0;
    preview.awaiting_batch = false;
    while preview.sent.len() < 25 {
        preview.next_chunk().unwrap();
        preview.awaiting_batch = false;
    }
    assert!(preview.next_chunk().is_none());
    preview.move_to(16.5, 80.0, 0.5).unwrap();
    let moved = preview.next_chunk().unwrap().0;
    let ids = packet_ids(&moved);
    assert_eq!(ids.iter().filter(|id| **id == FORGET_CHUNK).count(), 5);
    assert_eq!(ids.iter().filter(|id| **id == CHUNK).count(), 5);
    assert_eq!(preview.sent.len(), 25);
    assert!(!preview.sent.contains(&(-2, 0)));
    assert!(preview.sent.contains(&(3, 0)));

    let mut rejoined = Preview::new(2026, 2, Terrain::Preview, &manifest()).unwrap();
    rejoined.teleport_acknowledged = true;
    assert_eq!(rejoined.next_chunk().unwrap().0, first);
}

#[test]
fn block_state_table_follows_block_discriminants() {
    for (index, (block, _)) in BLOCK_STATES.iter().enumerate() {
        assert_eq!(*block as usize, index, "{block:?} is out of order");
    }
}

#[test]
fn chunk_wire_output_is_repeatable_across_request_order() {
    let generator = Generator::new(42);
    let biomes = [0, 1, 2, 3, 4, 5, 6, 7];
    let first = encode_chunk(&generator.generate(-1, 2), generator, &biomes);
    let _unrelated = generator.generate(17, -3);
    assert_eq!(
        first,
        encode_chunk(&generator.generate(-1, 2), generator, &biomes)
    );
    assert_ne!(
        first,
        encode_chunk(&generator.generate(0, 2), generator, &biomes)
    );
}

#[test]
#[ignore = "requires operator-provisioned 26.3 worldgen data and registry IDs"]
fn local_vanilla_worker_delivers_a_framed_chunk_without_blocking_network_poll() {
    let source = VanillaSource {
        data_root: std::env::var_os("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA"),
        registry_table: std::env::var_os("RUSTMC_CHUNK_REGISTRY")
            .map(PathBuf::from)
            .expect("set RUSTMC_CHUNK_REGISTRY"),
        seed: 2026,
        spawn_y: 117,
        cache: None,
        workers: 8,
    };
    let mut preview = Preview::new_vanilla(2, &manifest(), source).expect("preview");
    preview.teleport_acknowledged = true;
    let started = Instant::now();
    assert!(preview.next_chunk().is_none());
    assert!(started.elapsed() < std::time::Duration::from_millis(100));
    let delivered = loop {
        if let Some((stream, _, _)) = preview.next_chunk() {
            break stream;
        }
        assert!(!preview.failed, "worker failed before returning a chunk");
        // A bound on delivery, not on speed. This smoke runs the decorated
        // generator from a debug build, where one cold column descent costs
        // tens of times the 1,750 ms per chunk the release bench measures
        // (91 s to first delivery observed here, on a machine also running
        // three oracle grids), and each worker compiles the provisioned pack
        // before it draws anything. So this is a hang detector; the measured
        // cost of this path is `bench_vanilla_chunks` and
        // `docs/PROVENANCE.md`.
        assert!(started.elapsed() < std::time::Duration::from_secs(600));
        thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(packet_ids(&delivered).contains(&CHUNK));
    assert_eq!(preview.sent.len(), 1);
    preview.move_to(16.5, 80.0, 0.5).unwrap();
    assert!(
        preview.next_chunk().is_none(),
        "the first batch still awaits acknowledgement"
    );
    assert!(
        !preview.vanilla.as_ref().unwrap().in_flight.is_empty(),
        "generation should continue while awaiting the client"
    );
}

/// Reads skylight from the end of a framed chunk packet: 26 layers of a
/// two-byte `2048` length and 2048 bytes, then one byte for the empty block
/// light list. Layer 0 starts at y = -80; nibbles are low-first.
fn sky_at(packet: &[u8], x: usize, y: i32, z: usize) -> u8 {
    let first_layer = packet.len() - 1 - 26 * 2050;
    let layer = ((y + 80) / 16) as usize;
    let data = first_layer + layer * 2050 + 2;
    let index = (y.rem_euclid(16) as usize * 16 + z) * 16 + x;
    (packet[data + index / 2] >> ((index % 2) * 4)) & 0x0f
}

fn encoded(chunk: &Chunk) -> Vec<u8> {
    encode_chunk(chunk, Generator::new(1), &[0; 8])
}

#[test]
fn plants_and_thin_covers_pass_skylight() {
    for block in [
        Block::ShortGrass,
        Block::TallGrassLower,
        Block::TallGrassUpper,
        Block::SnowLayer,
        Block::Carpet,
    ] {
        let mut chunk = Chunk::empty(0, 0);
        chunk.set_block(2, 10, 3, Block::Grass);
        chunk.set_block(2, 11, 3, block);
        let packet = encoded(&chunk);
        assert_eq!(sky_at(&packet, 2, 12, 3), 15, "{block:?}");
        assert_eq!(sky_at(&packet, 2, 11, 3), 15, "{block:?}");
        assert_eq!(sky_at(&packet, 2, 10, 3), 0, "{block:?}");
        assert_eq!(sky_at(&packet, 2, 9, 3), 0, "{block:?}");
    }
}

#[test]
fn grass_is_only_in_the_world_surface_heightmap() {
    let mut chunk = Chunk::empty(0, 0);
    chunk.set_block(0, 10, 0, Block::Grass);
    let without_plant = encoded(&chunk);
    chunk.set_block(0, 11, 0, Block::ShortGrass);
    let with_plant = encoded(&chunk);

    // 8 coordinate bytes, map size, then per type: id, length 37, longs.
    // Column (0, 0) is the low 9 bits of each array's first big-endian long.
    let first_height = |packet: &[u8], map_index: usize| {
        let (_, frame_prefix) = read_varint(packet);
        let body = frame_prefix + 1 + 8 + 1;
        let entry = body + map_index * (2 + 37 * 8);
        let word: [u8; 8] = packet[entry + 2..entry + 10].try_into().unwrap();
        u64::from_be_bytes(word) & 0x1ff
    };
    // Floor top at y = 10 is stored as 10 + 1 + 64.
    assert_eq!(first_height(&without_plant, 0), 75);
    assert_eq!(first_height(&with_plant, 0), 76);
    assert_eq!(first_height(&with_plant, 1), 75);
    assert_eq!(first_height(&with_plant, 2), 75);
}

#[test]
fn new_block_variants_reach_the_chunk_palette() {
    // Protocol state IDs from the official 26.3 blocks report, as VarInts.
    for (block, id_bytes) in [
        (Block::ShortGrass, [0xBB, 0x12]),
        (Block::SnowLayer, [0x8D, 0x43]),
        (Block::Carpet, [0x97, 0x73]),
        (Block::TallGrassLower, [0xB3, 0x73]),
        (Block::TallGrassUpper, [0xB2, 0x73]),
    ] {
        let mut chunk = Chunk::empty(0, 0);
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set_block(x, y, z, block);
                }
            }
        }
        // 4096 non-air blocks, no fluid, single-value palette, state ID.
        let section = [0x10, 0x00, 0x00, 0x00, 0x00, id_bytes[0], id_bytes[1]];
        let packet = encoded(&chunk);
        assert!(
            packet.windows(section.len()).any(|bytes| bytes == section),
            "{block:?}"
        );
    }
}

#[test]
fn preview_block_state_ids_match_chunk_registry() {
    // Official 26.3 block report values, written out independently of the table.
    for (block, state) in [
        (Block::ShortGrass, 2363),
        (Block::SnowLayer, 8589),
        (Block::Carpet, 14743),
        (Block::TallGrassLower, 14771),
        (Block::TallGrassUpper, 14770),
    ] {
        assert_eq!(block_state(block as u16), state, "{block:?}");
    }
}

#[test]
fn obsidian_block_state_id_matches_26_3_default() {
    assert_eq!(block_state(Block::Obsidian as u16), 4925);
}

#[test]
fn vanilla_worker_chunk_over_threshold_is_compressed_at_send_time() {
    let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest())
        .unwrap()
        .with_compression_threshold(Some(256));
    preview.teleport_acknowledged = true;
    let (tx, rx) = mpsc::sync_channel(4);
    let raw_chunk_body = vec![0x5a; 4096];
    let cached_uncompressed = frame(CHUNK, &raw_chunk_body);
    tx.send(BuiltChunk {
        position: (0, 0),
        packet: Ok(Some(cached_uncompressed)),
        generation_us: 3,
        encoding_us: 1,
    })
    .unwrap();
    preview.vanilla = Some(VanillaWorker {
        requests: None,
        results: rx,
        handles: Vec::new(),
        in_flight: [(0, 0)].into_iter().collect(),
        pending: None,
        capacity: 4,
        current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
        cache: None,
    });

    let (stream, generation_us, encoding_us) = preview.next_chunk().unwrap();
    assert_eq!((generation_us, encoding_us), (3, 1));
    let mut cursor = stream.as_slice();
    let mut ids = Vec::new();
    let mut saw_compressed_chunk = false;
    while !cursor.is_empty() {
        let (pkt_len, pkt_prefix) = read_varint(cursor);
        let (data_len, _) = read_varint(&cursor[pkt_prefix..]);
        let (id, body, rest) =
            crate::java_wire::read_compressed_frame(cursor, 256, 1_048_576).unwrap();
        if id == CHUNK {
            assert!(data_len > 0);
            assert!((pkt_len as usize) < data_len as usize);
            assert_eq!(body, raw_chunk_body);
            saw_compressed_chunk = true;
        } else {
            assert_eq!(data_len, 0);
        }
        ids.push(id);
        cursor = rest;
    }
    assert!(saw_compressed_chunk);
    assert_eq!(ids, vec![CACHE_CENTER, BATCH_START, CHUNK, BATCH_END]);
}

#[test]
fn cached_uncompressed_chunk_packet_compresses_at_send_time_without_changing_cache() {
    let root = std::env::temp_dir().join(format!(
        "rustmc-java-preview-cache-comp-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    std::fs::create_dir_all(data.join("data/minecraft/worldgen")).unwrap();
    let registry = root.join("registry.json");
    std::fs::write(&registry, b"{}").unwrap();
    let cache =
        crate::preview_cache::PreviewCache::open(&root.join("cache"), &data, &registry, 2026)
            .unwrap();

    let raw_chunk_body = vec![0x3c; 2048];
    let cached_frame = frame(CHUNK, &raw_chunk_body);
    cache.write(0, 0, &cached_frame).unwrap();
    let read_back = cache.read(0, 0).unwrap().unwrap();
    assert_eq!(read_back, cached_frame);

    let mut preview = Preview::new(2026, 2, Terrain::Preview, &manifest())
        .unwrap()
        .with_compression_threshold(Some(256));
    preview.teleport_acknowledged = true;
    let (tx, rx) = mpsc::sync_channel(2);
    tx.send(BuiltChunk {
        position: (0, 0),
        packet: Ok(Some(read_back)),
        generation_us: 0,
        encoding_us: 0,
    })
    .unwrap();
    preview.vanilla = Some(VanillaWorker {
        requests: None,
        results: rx,
        handles: Vec::new(),
        in_flight: [(0, 0)].into_iter().collect(),
        pending: None,
        capacity: 2,
        current_center: Arc::new(AtomicU64::new(packed_center((0, 0)))),
        cache: Some(cache.clone()),
    });

    let (stream, _, _) = preview.next_chunk().unwrap();
    let mut cursor = stream.as_slice();
    let mut decoded_chunk = None;
    while !cursor.is_empty() {
        let (_, prefix) = read_varint(cursor);
        let (data_len, _) = read_varint(&cursor[prefix..]);
        let (id, body, rest) =
            crate::java_wire::read_compressed_frame(cursor, 256, 1_048_576).unwrap();
        if id == CHUNK {
            assert!(data_len > 0);
            decoded_chunk = Some(body);
        }
        cursor = rest;
    }
    assert_eq!(decoded_chunk.as_deref(), Some(raw_chunk_body.as_slice()));
    assert_eq!(cache.read(0, 0).unwrap(), Some(cached_frame));
    let _ = std::fs::remove_dir_all(&root);
}
