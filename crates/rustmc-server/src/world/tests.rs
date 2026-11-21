//! Unit tests for `world`.

use super::*;
#[test]
fn same_seed_and_coordinates_are_stable() {
    let g = Generator::new(42);
    assert_eq!(g.generate(-1, 2), g.generate(-1, 2));
    assert_ne!(g.generate(-1, 2), Generator::new(43).generate(-1, 2));
}
#[test]
fn request_order_does_not_change_chunks() {
    let g = Generator::new(1234);
    let first = g.generate(0, 0);
    let neighbor = g.generate(1, 0);
    assert_eq!(neighbor, g.generate(1, 0));
    assert_eq!(first, g.generate(0, 0));
}
#[test]
fn terrain_layers_and_bounded_height() {
    let g = Generator::new(9);
    let c = g.generate(0, 0);
    for z in 0..16 {
        for x in 0..16 {
            let h = g.height(x as i64, z as i64) as usize;
            assert_eq!(c.block(x, 0, z), Some(Block::Bedrock));
            let biome = g.biome(x as i64, z as i64);
            let top = match biome {
                Biome::Desert => Block::Sand,
                Biome::Badlands => Block::RedSand,
                Biome::SnowyPlains => Block::Snow,
                _ => Block::Grass,
            };
            let below = match biome {
                Biome::Desert => Block::Sand,
                Biome::Badlands => Block::RedSand,
                _ => Block::Dirt,
            };
            assert_eq!(c.block(x, h, z), Some(top));
            assert_eq!(c.block(x, h - 1, z), Some(below));
            assert_eq!(c.block(x, WORLD_HEIGHT - 1, z), Some(Block::Air));
        }
    }
}
#[test]
fn trees_can_cross_chunk_edges_without_order_dependence() {
    let mut found = false;
    for seed in 0..500 {
        let g = Generator::new(seed);
        let a = g.generate(0, 0);
        let b = g.generate(1, 0);
        for z in 0..16 {
            for y in 55..100 {
                if matches!(
                    a.block(15, y, z),
                    Some(
                        Block::Leaves
                            | Block::SpruceLeaves
                            | Block::AcaciaLeaves
                            | Block::JungleLeaves
                    )
                ) && matches!(
                    b.block(0, y, z),
                    Some(
                        Block::Leaves
                            | Block::SpruceLeaves
                            | Block::AcaciaLeaves
                            | Block::JungleLeaves
                    )
                ) {
                    found = true;
                    assert_eq!(a, g.generate(0, 0));
                    assert_eq!(b, g.generate(1, 0));
                    break;
                }
            }
        }
        if found {
            break;
        }
    }
    assert!(found);
}
#[test]
fn heights_vary_and_clearings_exclude_tree_roots() {
    let g = Generator::new(2026);
    let heights: std::collections::BTreeSet<_> = (-64..64)
        .step_by(8)
        .flat_map(|z| (-64..64).step_by(8).map(move |x| g.height(x, z)))
        .collect();
    assert!(heights.len() > 5);
    let mut clearings = 0;
    for z in -10..10 {
        for x in -10..10 {
            if let Some((root_x, root_z)) = g.tree_root(x, z) {
                assert!(!g.clearing(root_x, root_z));
            }
            let candidate_x = x * TREE_CELL + 2 + (g.hash(x, z, 1) % 4) as i64;
            let candidate_z = z * TREE_CELL + 2 + (g.hash(x, z, 2) % 4) as i64;
            if g.clearing(candidate_x, candidate_z) {
                clearings += 1;
                assert!(g.tree_root(x, z).is_none());
            }
        }
    }
    assert!(clearings > 0);
}

#[test]
fn experimental_terrain_is_stable_varied_and_continuous() {
    let g = Generator::with_terrain(2026, Terrain::Experimental);
    assert_eq!(g.height(1234, -5678), g.height(1234, -5678));
    assert_ne!(
        g.height(0, 0),
        Generator::with_terrain(2027, Terrain::Experimental).height(0, 0)
    );
    let mut min = i64::MAX;
    let mut max = i64::MIN;
    let mut prev = g.height(0, 37);
    for x in 1..4096 {
        let h = g.height(x, 37);
        min = min.min(h);
        max = max.max(h);
        assert!((40..=108).contains(&h), "height {h} at x={x}");
        assert!((h - prev).abs() <= 3, "cliff at x={x}: {prev} -> {h}");
        prev = h;
    }
    assert!(max - min > 25, "relief too flat: {min}..{max}");
    let negative: std::collections::BTreeSet<_> =
        (-512..512).step_by(64).map(|x| g.height(x, -64)).collect();
    assert!(negative.len() > 4);
}

#[test]
fn preview_remains_the_default_terrain_field() {
    let a = Generator::new(2026);
    let b = Generator::with_terrain(2026, Terrain::Preview);
    assert_eq!(a.terrain(), Terrain::Preview);
    assert_eq!(a.height(999, -1001), b.height(999, -1001));
    assert_eq!(a.biome(999, -1001), b.biome(999, -1001));
    assert_eq!(a.generate(3, -4), b.generate(3, -4));
}

#[test]
fn seed_2026_comparison_points_match_the_published_worksheet() {
    // Pins the RustMC column of docs/research/java-26.3-seed-comparison.md.
    // Changing the generator must update that worksheet (and its recorded
    // vanilla 26.3 comparison) in the same change.
    let generator = Generator::new(2026);
    const POINTS: [(i64, i64, i64, &str); 6] = [
        (0, 0, 71, "minecraft:badlands"),
        (256, 0, 61, "minecraft:plains"),
        (0, 256, 68, "minecraft:forest"),
        (-256, 0, 73, "minecraft:plains"),
        (0, -256, 65, "minecraft:badlands"),
        (512, 512, 65, "minecraft:taiga"),
    ];
    for (x, z, height, biome) in POINTS {
        assert_eq!(generator.height(x, z), height, "height at ({x}, {z})");
        assert_eq!(
            generator.biome(x, z).identifier(),
            biome,
            "biome at ({x}, {z})"
        );
    }
}

#[test]
fn biomes_cover_all_preview_regions_and_are_coordinate_stable() {
    let generator = Generator::new(2026);
    let mut seen = std::collections::BTreeSet::new();
    for z in (-2048..2048).step_by(96) {
        for x in (-2048..2048).step_by(96) {
            let biome = generator.biome(x, z);
            assert_eq!(biome, generator.biome(x, z));
            seen.insert(biome.identifier());
        }
    }
    assert_eq!(seen.len(), Biome::ALL.len());
    assert_ne!(
        generator.biome(-192, -192),
        Generator::new(2027).biome(-192, -192)
    );
}

#[test]
fn obsidian_round_trips_and_has_expected_blast_resistance() {
    assert_eq!(Block::from_u8(Block::Obsidian as u8), Some(Block::Obsidian));
    assert_eq!(Block::Obsidian.blast_resistance(), 1200.0);
    assert_eq!(Block::Bedrock.blast_resistance(), 3_600_000.0);
}
