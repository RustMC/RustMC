//! Unit tests for `vanilla::aquifer`.

use super::*;

#[test]
fn global_fluid_bounds_sea_and_lava() {
    let fluids = GlobalFluid {
        sea_level: 63,
        sea_fluid: Fluid::Water,
    };
    assert_eq!(fluids.pick(100).at(100), None);
    assert_eq!(fluids.pick(63).at(63), None);
    assert_eq!(fluids.pick(62).at(62), Some(Fluid::Water));
    assert_eq!(fluids.pick(-54).at(-54), Some(Fluid::Water));
    assert_eq!(fluids.pick(-55).at(-55), Some(Fluid::Lava));
}

#[test]
fn similarity_fades_over_25_squared_blocks() {
    assert_eq!(similarity(10, 10), 1.0);
    assert_eq!(similarity(10, 35), 0.0);
    assert!(similarity(10, 40) < 0.0);
}

#[test]
fn nearest_four_keep_ties_by_displacement() {
    let mut nearest = [([0, 0, 0], i32::MAX); 4];
    for (cell, distance) in [
        ([1, 1, 1], 30),
        ([2, 2, 2], 10),
        ([3, 3, 3], 10),
        ([4, 4, 4], 20),
        ([5, 5, 5], 40),
    ] {
        insert_nearest(&mut nearest, cell, distance);
    }
    // Equal distances displace earlier entries into the next slot.
    assert_eq!(nearest[0].0, [3, 3, 3]);
    assert_eq!(nearest[1].0, [2, 2, 2]);
    assert_eq!(nearest[2].0, [4, 4, 4]);
    assert_eq!(nearest[3].0, [1, 1, 1]);
}

#[test]
fn helper_maps_match_the_documented_shapes() {
    assert_eq!(clamped_map(0.0, 0.0, 64.0, 1.0, 0.0), 1.0);
    assert_eq!(clamped_map(32.0, 0.0, 64.0, 1.0, 0.0), 0.5);
    assert_eq!(clamped_map(64.0, 0.0, 64.0, 1.0, 0.0), 0.0);
    assert_eq!(clamped_map(-10.0, 0.0, 64.0, 1.0, 0.0), 1.0);
    assert_eq!(clamped_map(100.0, 0.0, 64.0, 1.0, 0.0), 0.0);
    assert_eq!(map(1.0, 1.0, 0.0, -0.3, 0.8), -0.3);
    assert_eq!(map(0.0, 1.0, 0.0, -0.3, 0.8), 0.8);
    assert_eq!(clamp(-2.0, -1.0, 1.0), -1.0);
    assert_eq!(clamp(2.0, -1.0, 1.0), 1.0);
}

#[test]
fn quantization_snaps_to_the_quart_grid() {
    assert_eq!(quantize_quart(7), 4);
    assert_eq!(quantize_quart(-1), -4);
    assert_eq!(quantize_quart(-4), -4);
    assert_eq!(quantize_quart(4), 4);
}
