//! Unit tests for `vanilla::worldgen`.

use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A unique scratch directory; all data written into it is
/// self-authored synthetic numbers, never vendor data.
fn scratch_root(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "rustmc-worldgen-{label}-{}-{n}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create scratch dir");
    path
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, text).expect("write");
}

/// A minimal fabricated datapack mirroring the 26.3 document shapes:
/// resolved noise definitions, registry-id string references (with a
/// nested path), a bare-number density document, inline router
/// objects, and the slim `noise` bounds section.
fn synthetic_root(label: &str) -> PathBuf {
    let root = scratch_root(label);
    let worldgen = root.join("data/testns/worldgen");
    write(
        &worldgen.join("noise/fabricated.json"),
        r#"{"base_amplitude": 2.0, "base_octave": 3, "octave_count": 2}"#,
    );
    write(
        &worldgen.join("noise/solo.json"),
        r#"{"base_amplitude": 1.5, "base_octave": -2}"#,
    );
    write(&worldgen.join("density_function/zero.json"), "0.0");
    write(
        &worldgen.join("density_function/group/erosion.json"),
        r#"{"type": "noise", "noise": "testns:fabricated", "xz_scale": 0.9, "y_scale": 0.9}"#,
    );
    write(
        &worldgen.join("density_function/depth.json"),
        r#"{"type": "sub", "left": -0.3, "right": "testns:group/erosion"}"#,
    );
    write(
        &worldgen.join("density_function/final.json"),
        r#"{"type": "max", "left": "testns:zero", "right": "testns:depth"}"#,
    );
    write(
        &worldgen.join("noise_settings/fabricated_dimension.json"),
        r#"{
            "noise": {"min_y": -16, "height": 64},
            "sea_level": 20,
            "noise_router": {
                "final_density": "testns:final",
                "continents": "testns:zero",
                "erosion": "testns:group/erosion",
                "depth": {"type": "mul", "left": 2, "right": "testns:zero"},
                "ridges": "testns:zero",
                "temperature": "testns:zero",
                "vegetation": "testns:zero",
                "chunk_surface_level": "testns:zero"
            },
            "aquifers": {
                "barrier": "testns:zero",
                "fluid_level_floodedness": "testns:zero",
                "fluid_level_spread": {
                    "type": "noise", "noise": "testns:solo",
                    "xz_scale": 1.0, "y_scale": 1.0
                },
                "lava": "testns:zero",
                "exclusion": "testns:zero",
                "surface_level": "testns:group/erosion"
            }
        }"#,
    );
    write(
        &worldgen.join("noise_settings/broken_dimension.json"),
        r#"{
            "noise": {"min_y": 0, "height": 32},
            "sea_level": 0,
            "noise_router": {
                "final_density": "testns:nonexistent",
                "continents": "minecraft:y",
                "erosion": "testns:zero",
                "depth": "testns:zero",
                "ridges": "testns:zero",
                "temperature": "testns:zero",
                "vegetation": "testns:zero"
            }
        }"#,
    );
    root
}

#[test]
fn loads_namespaced_ids_including_nested_paths() {
    let root = synthetic_root("load");
    let data = WorldgenData::load(&root).expect("load");
    let parameters = data
        .noise_definitions()
        .get("testns:fabricated")
        .expect("noise id resolves");
    assert_eq!(parameters.base_amplitude, 2.0);
    assert_eq!(parameters.base_octave, 3);
    assert_eq!(parameters.octave_count, 2);
    assert!(parameters.normalize);
    // Resolved form pads missing modifiers with identity amplitudes.
    assert_eq!(parameters.amplitude_modifiers, vec![1.0, 1.0]);
    // Single-octave definitions omit `octave_count` and modifiers.
    let solo = data
        .noise_definitions()
        .get("testns:solo")
        .expect("solo noise id resolves");
    assert_eq!(solo.base_amplitude, 1.5);
    assert_eq!(solo.base_octave, -2);
    assert_eq!(solo.octave_count, 1);
    assert_eq!(solo.amplitude_modifiers, vec![1.0]);
    assert!(
        data.density_documents()
            .contains_key("testns:group/erosion")
    );
    assert!(data.density_documents().contains_key("testns:zero"));
    assert!(data.noise_settings("testns:fabricated_dimension").is_some());
    fs::remove_dir_all(&root).expect("cleanup");
}

#[test]
fn missing_data_directory_loads_empty() {
    let root = scratch_root("empty");
    let data = WorldgenData::load(&root).expect("load");
    assert!(data.noise_definitions().is_empty());
    assert!(data.density_documents().is_empty());
    assert!(data.noise_settings("anything").is_none());
    fs::remove_dir_all(&root).expect("cleanup");
}

#[test]
fn router_compiles_reference_and_inline_slots() {
    let root = synthetic_root("router");
    let data = WorldgenData::load(&root).expect("load");
    let engine = data.engine(2026);
    let registry = data.registry(&engine);
    let router = data
        .router(&registry, "testns:fabricated_dimension")
        .expect("router");
    assert_eq!(router.min_y, -16);
    assert_eq!(router.height, 64);
    assert_eq!(router.sea_level, 20);
    // An inline object slot compiles independently of the documents.
    let inline = router.depth.sample(4, 8, -2);
    assert_eq!(inline, 0.0);
    let point = (7, 11, -3);
    let erosion = registry
        .compile(&"testns:group/erosion".to_owned())
        .expect("erosion");
    let e = erosion.sample(point.0, point.1, point.2);
    let depth = registry
        .compile(&"testns:depth".to_owned())
        .expect("depth document");
    let d = depth.sample(point.0, point.1, point.2);
    // `sub` with a constant left input is exact f32 subtraction.
    assert_eq!(d, -0.3f32 - e);
    let final_ = router.final_density.sample(point.0, point.1, point.2);
    assert!(final_ == 0.0 || (d > 0.0 && final_ == d));
    // The aquifers section compiles its six density fields; an inline
    // noise node resolves through the noise registry, not density ids.
    assert_eq!(router.default_fluid, Fluid::Water);
    let aquifers = router.aquifers.as_ref().expect("aquifers section");
    assert_eq!(aquifers.barrier.sample(point.0, point.1, point.2), 0.0);
    let spread = aquifers
        .fluid_level_spread
        .sample(point.0, point.1, point.2);
    assert!(spread.is_finite() && spread.abs() <= 2.0);
    fs::remove_dir_all(&root).expect("cleanup");
}

#[test]
fn unresolved_references_reports_missing_ids_only() {
    let root = synthetic_root("refs");
    let data = WorldgenData::load(&root).expect("load");
    let clean = unresolved_references(&data, "testns:fabricated_dimension");
    assert!(clean.is_empty(), "unexpected missing: {clean:?}");
    let broken = unresolved_references(&data, "testns:broken_dimension");
    assert_eq!(broken, vec!["testns:nonexistent".to_owned()]);
    fs::remove_dir_all(&root).expect("cleanup");
}

#[test]
fn router_reports_uncompilable_reference() {
    let root = synthetic_root("badrouter");
    let data = WorldgenData::load(&root).expect("load");
    let engine = data.engine(2026);
    let registry = data.registry(&engine);
    let error = match data.router(&registry, "testns:broken_dimension") {
        Ok(_) => panic!("missing id must not compile"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("testns:nonexistent"),
        "error should name the unresolved id: {error}"
    );
    fs::remove_dir_all(&root).expect("cleanup");
}

/// Manual verification against an operator-provisioned datapack root
/// (never committed; see `docs/PROVENANCE.md`). Run with:
/// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored`
/// or rely on the default project-local path.
#[test]
#[ignore = "requires operator-provisioned local data"]
fn smoke_loads_operator_worldgen_data() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let data = WorldgenData::load(&root).expect("operator data loads");
    let engine = data.engine(2026);
    let registry = data.registry(&engine);
    let missing = unresolved_references(&data, "minecraft:overworld");
    assert!(missing.is_empty(), "unresolved: {missing:?}");
    let router = data
        .router(&registry, "minecraft:overworld")
        .expect("overworld router");
    assert_eq!(router.min_y, -64);
    assert_eq!(router.height, 384);
    assert_eq!(router.sea_level, 63);
    // The overworld wires its runtime aquifer: six density fields and
    // the water flooding fluid (publicly documented dimension facts).
    assert_eq!(router.default_fluid, Fluid::Water);
    let aquifers = router.aquifers.as_ref().expect("overworld aquifers");
    for sample in [&aquifers.barrier, &aquifers.surface_level] {
        assert!(sample.sample(0, 0, 0).is_finite());
    }
    // Sampling the real final-density graph must produce finite values
    // across the documented vertical range.
    for y in [-64, 0, 64, 200, 319] {
        let v = router.final_density.sample(0, y, 0);
        assert!(v.is_finite(), "final density at y={y}: {v}");
    }
    println!("smoke OK: router compiled from {}", root.display());
}

#[test]
#[ignore = "requires operator-provisioned local data"]
fn final_density_volume_matches_point_samples_bit_for_bit() {
    use crate::vanilla::density::DensityVolume;

    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let data = WorldgenData::load(&root).expect("operator data loads");
    let engine = data.engine(2026);
    let registry = data.registry(&engine);
    let router = data
        .router(&registry, "minecraft:overworld")
        .expect("overworld router");

    let mut mismatches = 0usize;
    let mut first_mismatch = None;
    for (cx, cz) in [(0, 0), (-3, 2), (17, -9), (40, 40)] {
        let min_x = cx * 16;
        let min_z = cz * 16;
        let volume = DensityVolume::new(
            [16, router.height, 16],
            [min_x, router.min_y, min_z],
            [1, 1, 1],
        )
        .expect("chunk volume");
        let mut buffer = vec![0.0f32; volume.slot_count()];
        router.final_density.sample_volume(&volume, &mut buffer);
        for z in 0..16 {
            for x in 0..16 {
                for y in 0..router.height {
                    let wx = min_x + x;
                    let wy = router.min_y + y;
                    let wz = min_z + z;
                    let got = buffer[volume.index(x as usize, y as usize, z as usize)];
                    let want = router.final_density.sample(wx, wy, wz);
                    if got.to_bits() != want.to_bits() {
                        mismatches += 1;
                        if first_mismatch.is_none() {
                            first_mismatch = Some((wx, wy, wz, got, want));
                        }
                    }
                }
            }
        }
    }
    assert_eq!(
        mismatches, 0,
        "volume != point ({mismatches} mismatches), first: {first_mismatch:?}"
    );
}

#[test]
#[ignore = "requires operator-provisioned local data"]
fn climate_volume_targets_match_point_targets_bit_for_bit() {
    use crate::vanilla::biome::ClimateSampler;
    use crate::vanilla::density::DensityVolume;

    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rustmc-local/vanilla-data"));
    let data = WorldgenData::load(&root).expect("operator data loads");
    let engine = data.engine(2026);
    let registry = data.registry(&engine);
    let router = data
        .router(&registry, "minecraft:overworld")
        .expect("overworld router");
    let climate = ClimateSampler {
        temperature: router.temperature.clone(),
        vegetation: router.vegetation.clone(),
        continents: router.continents.clone(),
        erosion: router.erosion.clone(),
        depth: router.depth.clone(),
        ridges: router.ridges.clone(),
    };

    let mut scratch = Vec::new();
    let mut targets = Vec::new();
    let quart_height = router.height / 4;
    for (cx, cz) in [(0, 0), (-3, 2), (17, -9), (40, 40)] {
        let min_x = cx * 16;
        let min_z = cz * 16;
        let volume = DensityVolume::new(
            [4, quart_height, 4],
            [min_x, router.min_y, min_z],
            [4, 4, 4],
        )
        .expect("quart volume");
        climate.targets_for_volume(&volume, &mut scratch, &mut targets);
        for lz in 0..4 {
            for lx in 0..4 {
                for ly in 0..quart_height {
                    let wx = min_x + lx * 4;
                    let wy = router.min_y + ly * 4;
                    let wz = min_z + lz * 4;
                    let got = targets[volume.index(lx as usize, ly as usize, lz as usize)];
                    let want = climate.target_at_block(wx, wy, wz);
                    assert_eq!(got, want, "target mismatch at ({wx},{wy},{wz})");
                }
            }
        }
    }
}
