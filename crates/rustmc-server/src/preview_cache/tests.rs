//! Unit tests for `preview_cache`.

use super::*;

/// Input lengths and the SHA-256 hex `sha2 0.10.9` produced for them: a
/// repeating `a..z` filler of each length, and the packet body the
/// round-trip test caches.
const SHA256_KAT: &[(usize, &str)] = &[
    (
        0,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ),
    (
        1,
        "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb",
    ),
    (
        64,
        "2fcd5a0d60e4c941381fcc4e00a4bf8be422c3ddfafb93c809e8d1e2bfffae8e",
    ),
    (
        200,
        "8013a82140d916576e2cf550b27449a368abec66cc154a7d9f599019d33aa3d2",
    ),
    (
        4_096,
        "bc45051ac426475f459ec0b0c88a6646d037b8dfb1b9fa3ca3ef9203ce33e283",
    ),
];

fn filler(len: usize) -> Vec<u8> {
    (0..len).map(|i| b'a' + (i % 26) as u8).collect()
}

/// Why: an exact-pin refresh of `sha2` must not change cached-preview
/// bytes, so the digests below were captured from the 0.10.9 stack before
/// the bump and are asserted unchanged; the inputs cover the empty value,
/// a partial block, one whole SHA-256 block, a multi-block packet, a
/// 4 KiB payload, and the packet body the round-trip test caches.
#[test]
fn digests_match_the_0_10_stack() {
    for (len, expected) in SHA256_KAT {
        assert_eq!(hex(&Sha256::digest(filler(*len))), *expected, "{len} bytes");
    }
    assert_eq!(
        hex(&Sha256::digest(b"framed-packet")),
        "53d64589780947efe64b83ee9e653fb5ec34fd46f4f12a196d8a8fb2a1faac19"
    );
}

/// The cache hashes its inputs through several `update` calls per record,
/// so a single-shot digest per input would leave the incremental path
/// unpinned: these are the same 0.10.9 digests fed across block boundaries
/// at every chunk size that could split a SHA-256 block differently.
#[test]
fn streamed_updates_match_the_0_10_stack() {
    for (len, expected) in SHA256_KAT {
        let len = *len;
        let input = filler(len);
        for chunk in [1usize, 7, 63, 64, 65] {
            let mut hasher = Sha256::new();
            for part in input.chunks(chunk) {
                hasher.update(part);
            }
            assert_eq!(
                hex(&hasher.finalize()),
                *expected,
                "{len} bytes in {chunk}-byte updates"
            );
        }
        let mut split = Sha256::new();
        split.update(&input[..len.min(1)]);
        split.update(&input[len.min(1)..]);
        assert_eq!(hex(&split.finalize()), *expected, "{len} bytes split");
    }
}

#[test]
fn roundtrip_corruption_and_input_invalidation() {
    let root = std::env::temp_dir().join(format!(
        "rustmc-preview-cache-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let data = root.join("data");
    let worldgen = data.join("data/minecraft/worldgen");
    fs::create_dir_all(&worldgen).unwrap();
    let registry = root.join("registry.json");
    fs::write(worldgen.join("world.json"), b"alpha").unwrap();
    fs::write(&registry, b"registry-a").unwrap();
    let cache = PreviewCache::open(&root.join("cache"), &data, &registry, 2026).unwrap();
    assert_eq!(cache.read(-2, 3).unwrap(), None);
    assert!(!cache.contains(-2, 3));
    cache.write(-2, 3, b"framed-packet").unwrap();
    assert!(cache.contains(-2, 3));
    assert_eq!(cache.read(-2, 3).unwrap(), Some(b"framed-packet".to_vec()));
    let reopened = PreviewCache::open(&root.join("cache"), &data, &registry, 2026).unwrap();
    assert_eq!(reopened.cached_count(), 1);
    assert!(reopened.contains(-2, 3));
    assert!(!reopened.contains(3, -2));
    assert_eq!(
        reopened.read(-2, 3).unwrap(),
        Some(b"framed-packet".to_vec())
    );
    let mut corrupt = fs::read(cache.path(-2, 3)).unwrap();
    *corrupt.last_mut().unwrap() ^= 1;
    fs::write(cache.path(-2, 3), corrupt).unwrap();
    assert_eq!(cache.read(-2, 3).unwrap(), None);
    assert_eq!(
        PreviewCache::open(&root.join("cache"), &data, &registry, 2027)
            .unwrap()
            .cached_count(),
        0
    );
    fs::write(worldgen.join("world.json"), b"beta").unwrap();
    assert_eq!(
        PreviewCache::open(&root.join("cache"), &data, &registry, 2026)
            .unwrap()
            .cached_count(),
        0
    );
    fs::write(&registry, b"registry-b").unwrap();
    assert_eq!(
        PreviewCache::open(&root.join("cache"), &data, &registry, 2026)
            .unwrap()
            .cached_count(),
        0
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn only_generator_inputs_shape_the_cache_identity() {
    let root = std::env::temp_dir().join(format!(
        "rustmc-preview-cache-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let data = root.join("data");
    let worldgen = data.join("data/minecraft/worldgen");
    let block_tags = data.join("data/minecraft/tags/block");
    let recipes = data.join("data/minecraft/recipe");
    let reports = data.join("reports/minecraft/components/item");
    for directory in [&worldgen, &block_tags, &recipes, &reports] {
        fs::create_dir_all(directory).unwrap();
    }
    let registry = root.join("registry.json");
    fs::write(worldgen.join("world.json"), b"alpha").unwrap();
    fs::write(block_tags.join("stones.json"), b"stones").unwrap();
    fs::write(&registry, b"registry-a").unwrap();
    let identity = input_identity(&data, &registry, 2026).unwrap();

    // Rule data beside the worldgen files does not change the identity, even
    // when there are more such files than the generator input limit.
    for index in 0..=MAX_INPUT_FILES {
        fs::write(recipes.join(format!("r{index}.json")), b"{}").unwrap();
    }
    fs::write(reports.join("stone.json"), b"{}").unwrap();
    assert_eq!(input_identity(&data, &registry, 2026).unwrap(), identity);

    // Generator inputs still do.
    fs::write(block_tags.join("stones.json"), b"more stones").unwrap();
    assert_ne!(input_identity(&data, &registry, 2026).unwrap(), identity);
    fs::create_dir_all(data.join("rustmc/biome_placement")).unwrap();
    let tagged = input_identity(&data, &registry, 2026).unwrap();
    fs::write(data.join("rustmc/biome_placement/overworld.psv"), b"p").unwrap();
    assert_ne!(input_identity(&data, &registry, 2026).unwrap(), tagged);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prune_returns_evicted_paths_without_unlinking() {
    let root = std::env::temp_dir().join(format!(
        "rustmc-preview-cache-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let data = root.join("data");
    let worldgen = data.join("data/minecraft/worldgen");
    fs::create_dir_all(&worldgen).unwrap();
    let registry = root.join("registry.json");
    fs::write(worldgen.join("world.json"), b"alpha").unwrap();
    fs::write(&registry, b"registry-a").unwrap();
    let cache = PreviewCache::open(&root.join("cache"), &data, &registry, 2026).unwrap();

    let oldest = cache.path(0, 0);
    let newest = cache.path(1, 0);
    fs::write(&oldest, b"oldest-packet").unwrap();
    fs::write(&newest, b"newest-packet").unwrap();

    let mut budget = Budget {
        files: VecDeque::from([(oldest.clone(), MAX_BYTES), (newest.clone(), 1)]),
        positions: BTreeSet::from([(0, 0), (1, 0)]),
        bytes: MAX_BYTES + 1,
    };

    let evicted = prune(&mut budget);
    assert_eq!(evicted, vec![oldest.clone()]);
    assert!(!budget.positions.contains(&(0, 0)));
    assert!(budget.positions.contains(&(1, 0)));
    assert_eq!(budget.bytes, 1);
    assert!(oldest.exists());

    remove_evicted(evicted).unwrap();
    assert!(!oldest.exists());
    assert!(newest.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn read_forgets_corrupt_entry() {
    let root = std::env::temp_dir().join(format!(
        "rustmc-preview-cache-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let data = root.join("data");
    let worldgen = data.join("data/minecraft/worldgen");
    fs::create_dir_all(&worldgen).unwrap();
    let registry = root.join("registry.json");
    fs::write(worldgen.join("world.json"), b"alpha").unwrap();
    fs::write(&registry, b"registry-a").unwrap();
    let cache = PreviewCache::open(&root.join("cache"), &data, &registry, 2026).unwrap();

    cache.write(3, 4, b"framed-packet").unwrap();
    assert!(cache.contains(3, 4));
    let count_before = cache.cached_count();
    assert_eq!(count_before, 1);

    let path = cache.path(3, 4);
    fs::write(&path, b"garbage").unwrap();
    assert_eq!(cache.read(3, 4).unwrap(), None);
    assert!(!cache.contains(3, 4));
    assert_eq!(cache.cached_count(), count_before - 1);
    assert_eq!(
        cache
            .budget
            .lock()
            .expect("preview cache lock poisoned")
            .bytes,
        0
    );
    assert!(!path.exists());

    fs::remove_dir_all(root).unwrap();
}
