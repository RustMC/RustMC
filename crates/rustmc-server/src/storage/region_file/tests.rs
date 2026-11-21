//! Unit tests for `storage::region_file`.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rustmc-region-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn chunk(text: &str) -> StoredChunk {
    compress(text.as_bytes()).expect("compress")
}

#[test]
fn saved_chunks_read_back_and_untouched_chunks_survive_a_second_save() {
    let dir = TempDir::new();
    let path = dir.0.join("region").join("r.0.0.mca");
    let first = BTreeMap::from([((0, 0), chunk("origin")), ((31, 31), chunk("corner"))]);
    save_chunks(&path, first, 100).expect("save");
    assert_eq!(
        read_chunk(&path, 0, 0).expect("read"),
        Some(b"origin".to_vec())
    );
    assert_eq!(
        read_chunk(&path, 31, 31).expect("read"),
        Some(b"corner".to_vec())
    );
    assert_eq!(read_chunk(&path, 5, 5).expect("read"), None);

    let big = "x".repeat(3 * SECTOR);
    save_chunks(&path, BTreeMap::from([((0, 0), chunk(&big))]), 200).expect("resave");
    assert_eq!(
        read_chunk(&path, 0, 0).expect("read"),
        Some(big.into_bytes())
    );
    assert_eq!(
        read_chunk(&path, 31, 31).expect("read"),
        Some(b"corner".to_vec())
    );
    let bytes = fs::read(&path).expect("file");
    assert_eq!(bytes.len() % SECTOR, 0, "sectors are padded");
    assert_eq!(
        bytes[SECTOR..SECTOR + 4],
        200u32.to_be_bytes(),
        "new timestamp"
    );
    let corner = SECTOR + (31 + 31 * REGION_SIDE) * 4;
    assert_eq!(
        bytes[corner..corner + 4],
        100u32.to_be_bytes(),
        "kept timestamp"
    );
    assert!(
        !path.with_extension("tmp").exists(),
        "no temporary file is left"
    );
}

#[test]
fn missing_files_read_as_empty_and_damaged_ones_are_errors() {
    let dir = TempDir::new();
    let path = dir.0.join("r.1.1.mca");
    assert_eq!(read_chunk(&path, 0, 0).expect("missing"), None);
    let mut header = vec![0u8; HEADER];
    header[0..4].copy_from_slice(&((1u32 << 8) | 1).to_be_bytes());
    fs::write(&path, &header).expect("write");
    assert!(
        read_chunk(&path, 0, 0).is_err(),
        "location points into the header"
    );
    assert!(save_chunks(&path, BTreeMap::new(), 0).is_err());
}

#[test]
fn chunks_beyond_255_sectors_are_refused() {
    let dir = TempDir::new();
    let path = dir.0.join("r.0.0.mca");
    let huge = StoredChunk {
        compression: ZLIB,
        payload: vec![0; MAX_CHUNK_SECTORS * SECTOR],
    };
    assert!(save_chunks(&path, BTreeMap::from([((0, 0), huge)]), 0).is_err());
    assert!(!path.exists(), "nothing is written when a save is refused");
}

#[test]
#[ignore = "requires operator-provisioned local data"]
fn reads_the_same_chunks_as_the_save_reader() {
    let save = std::env::var("RUSTMC_VANILLA_SAVE").expect("RUSTMC_VANILLA_SAVE");
    let save = std::path::Path::new(&save);
    let path = save.join("dimensions/minecraft/overworld/region/r.0.0.mca");
    let mut store = crate::storage::region::RegionStore::new(save);
    for (x, z) in [(0, 0), (3, 7), (31, 31)] {
        assert_eq!(
            read_chunk(&path, x, z).expect("read"),
            store.chunk_raw(x as i32, z as i32).expect("store"),
            "chunk {x} {z}"
        );
    }
}

#[test]
fn external_gzip_and_uncompressed_chunks_are_read() {
    let dir = TempDir::new();
    let region = dir.0.join("r.-1.2.mca");
    let external = chunk("stored outside");
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(b"gzip chunk").expect("gzip");
    let changed = BTreeMap::from([
        // Local (3, 4) of region (-1, 2) is chunk (-29, 68).
        (
            (3, 4),
            StoredChunk {
                compression: ZLIB + EXTERNAL,
                payload: Vec::new(),
            },
        ),
        (
            (0, 0),
            StoredChunk {
                compression: GZIP,
                payload: gzip.finish().expect("gzip"),
            },
        ),
        (
            (1, 0),
            StoredChunk {
                compression: UNCOMPRESSED,
                payload: b"plain chunk".to_vec(),
            },
        ),
    ]);
    save_chunks(&region, changed, 0).expect("save");
    fs::write(dir.0.join("c.-29.68.mcc"), &external.payload).expect("mcc");
    assert_eq!(
        read_chunk(&region, 3, 4).expect("read"),
        Some(b"stored outside".to_vec())
    );
    assert_eq!(
        read_chunk(&region, 0, 0).expect("read"),
        Some(b"gzip chunk".to_vec())
    );
    assert_eq!(
        read_chunk(&region, 1, 0).expect("read"),
        Some(b"plain chunk".to_vec())
    );
    fs::remove_file(dir.0.join("c.-29.68.mcc")).expect("remove");
    assert!(
        read_chunk(&region, 3, 4).is_err(),
        "a missing .mcc is an error"
    );
}
