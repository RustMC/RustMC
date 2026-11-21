//! Unit tests for `vanilla::biome`.

use super::*;

/// The reference rule `find` must reproduce: the first row with the
/// smallest fitness.
fn brute_force_row(table: &BiomePlacement, target: &[i64; 6]) -> Option<usize> {
    let mut best: Option<(i64, usize)> = None;
    for (row, (_, point)) in table.entries.iter().enumerate() {
        let fitness = point.fitness(target);
        if best.is_none_or(|(previous, _)| fitness < previous) {
            best = Some((fitness, row));
        }
    }
    best.map(|(_, row)| row)
}

fn brute_force<'a>(table: &'a BiomePlacement, target: &[i64; 6]) -> Option<&'a str> {
    let row = brute_force_row(table, target)?;
    table.entry_id(row)
}

fn lcg_targets(count: usize, spread: i64) -> Vec<[i64; 6]> {
    let mut state = 0x2026_u64;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 33) as i64 % (2 * spread + 1)) - spread
    };
    (0..count)
        .map(|_| [next(), next(), next(), next(), next(), next()])
        .collect()
}

#[test]
fn pruned_search_matches_brute_force_including_ties() {
    // Identical rows and rows that tie at many targets: the earlier row
    // must keep winning however the previous lookup primed the search.
    let row = |name: &str, min: i64, max: i64, offset: i64| {
        let param = Param { min, max };
        (
            name.to_owned(),
            ParameterPoint {
                temperature: param,
                humidity: param,
                continentalness: param,
                erosion: param,
                depth: param,
                weirdness: param,
                offset,
            },
        )
    };
    let table = BiomePlacement::from_entries(vec![
        row("a", -100, 100, 0),
        row("b", -100, 100, 0),
        row("c", 0, 500, 0),
        row("d", -500, 0, 0),
        row("e", 0, 500, 0),
        row("f", -50, 50, 10),
    ]);
    for target in lcg_targets(5_000, 800) {
        assert_eq!(
            table.find(&target),
            brute_force(&table, &target),
            "{target:?}"
        );
    }
    // Prime with a later tying row, then ask for a point where an earlier
    // row ties with it.
    table.last_winner.set(Some(4));
    assert_eq!(table.find(&[300; 6]), Some("c"));
    table.last_winner.set(Some(1));
    assert_eq!(table.find(&[0; 6]), Some("a"));
}

/// Run with:
/// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pruned_search_matches_brute_force_on_the_vanilla_table`
#[test]
#[ignore = "requires operator-provisioned local data"]
fn pruned_search_matches_brute_force_on_the_vanilla_table() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
    let table = BiomePlacement::load(&root, "overworld").expect("overworld placement table");
    // Quantized climate values span roughly +/-2 * 10_000.
    for target in lcg_targets(50_000, 20_000) {
        assert_eq!(
            table.find(&target),
            brute_force(&table, &target),
            "{target:?}"
        );
    }
}

#[test]
fn quantization_truncates_toward_zero() {
    assert_eq!(quantize_coord(0.7), 7000);
    assert_eq!(quantize_coord(-0.375), -3750);
    // 1.1F * 10000 rounds in f32 just under 11000 and truncates.
    assert_eq!(quantize_coord(1.1), 11000);
}

#[test]
fn interval_distance_is_zero_inside() {
    let param = Param { min: -4, max: 7 };
    assert_eq!(param.distance(-10), 6);
    assert_eq!(param.distance(0), 0);
    assert_eq!(param.distance(10), 3);
}

/// The dimension's possible-biome set is the table read in order with
/// repeats dropped: a biome with two parameter ranges occupies two rows
/// but one slot, and the slot is its first row's.
#[test]
fn possible_biomes_follow_declaration_order_and_drop_repeats() {
    let text = concat!(
        "0|minecraft:plains|t=[-2000-2000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
        "1|minecraft:forest|t=[2000-10000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
        "2|minecraft:plains|t=[-10000--2000]|h=[-10000-10000]|c=0|e=[-100-100]|d=0|w=[-10000-10000]|off=0\n",
    );
    let table = BiomePlacement::parse(text).expect("test table parses");
    assert_eq!(table.len(), 3);
    assert_eq!(
        table.possible_biomes(),
        vec!["minecraft:plains", "minecraft:forest"],
    );
}

#[test]
fn parsing_handles_ranges_points_and_negative_bounds() {
    let table = test_table();
    assert_eq!(table.len(), 2);
    assert_eq!(table.entries[0].1.humidity, Param::default_full());
    assert_eq!(table.entries[1].1.temperature.min, 2000);
}

#[test]
fn nearest_entry_wins_and_ties_keep_the_earlier() {
    let table = test_table();
    assert_eq!(table.find(&[5000, 0, 0, 0, 0, 0]), Some("minecraft:forest"));
    assert_eq!(
        table.find(&[-1000, 0, 0, 0, 0, 0]),
        Some("minecraft:plains")
    );
    // Exactly tied fitness between the two entries keeps entry 0.
    assert_eq!(table.find(&[0, 0, 0, 0, 0, 0]), Some("minecraft:plains"));
}

fn resolve_column_by_runs(
    table: &BiomePlacement,
    column: &[[i64; 6]],
    scratch: &mut ColumnSearchScratch,
    depths: &mut Vec<i64>,
    rows: &mut Vec<usize>,
    out: &mut Vec<usize>,
) {
    out.clear();
    let mut start = 0;
    while start < column.len() {
        let len = fixed_run_len(&column[start..]);
        let fixed = fixed_climate_coords(&column[start]);
        depths.clear();
        depths.extend(column[start..start + len].iter().map(|target| target[4]));
        table.find_column(fixed, depths, scratch, rows);
        assert_eq!(rows.len(), len);
        out.extend_from_slice(rows);
        start += len;
    }
}

#[test]
fn column_search_matches_find_and_brute_force_including_ties() {
    let entry = |name: &str, t: Param, h: Param, c: Param, e: Param, d: Param, w: Param, off| {
        (
            name.to_owned(),
            ParameterPoint {
                temperature: t,
                humidity: h,
                continentalness: c,
                erosion: e,
                depth: d,
                weirdness: w,
                offset: off,
            },
        )
    };
    let p = |min, max| Param { min, max };
    let zero = p(0, 0);
    // Deliberately craft equal rows, equal fixed parts A_i, non-zero
    // offsets, and late-sorted earlier-index ties (row 0 has A=10000, d=0
    // whereas row 1 has A=0, d=100: at target [0;6], row 1 sets best=(10000,1)
    // first, then row 0 at A==best_fitness must still beat it by index).
    let table = BiomePlacement::from_entries(vec![
        entry("early_high_a", p(100, 100), zero, zero, zero, zero, zero, 0),
        entry("late_low_a", zero, zero, zero, zero, p(100, 100), zero, 0),
        entry("dup_low_a", zero, zero, zero, zero, p(100, 100), zero, 0),
        entry("dup_high_a", p(100, 100), zero, zero, zero, zero, zero, 0),
        entry(
            "equal_a_mid",
            p(-50, 50),
            p(-50, 50),
            zero,
            zero,
            p(-40, 40),
            zero,
            120,
        ),
        entry(
            "equal_a_alt",
            p(-50, 50),
            p(-50, 50),
            zero,
            zero,
            p(20, 80),
            zero,
            120,
        ),
        entry(
            "offset_tie_1",
            p(-100, 100),
            p(-100, 100),
            p(-100, 100),
            zero,
            p(-100, 100),
            zero,
            100,
        ),
        entry(
            "offset_tie_2",
            p(-100, 100),
            p(-100, 100),
            p(-100, 100),
            zero,
            p(-100, 100),
            zero,
            -100,
        ),
        entry(
            "wide_a",
            p(200, 400),
            p(-400, 400),
            p(-400, 400),
            p(-400, 400),
            p(-200, 200),
            p(-400, 400),
            25,
        ),
        entry(
            "wide_b",
            p(200, 400),
            p(-400, 400),
            p(-400, 400),
            p(-400, 400),
            p(100, 300),
            p(-400, 400),
            25,
        ),
        entry(
            "identical_1",
            p(200, 500),
            p(-200, 200),
            p(-100, 100),
            p(0, 300),
            p(-300, 300),
            p(-500, 500),
            0,
        ),
        entry(
            "identical_2",
            p(200, 500),
            p(-200, 200),
            p(-100, 100),
            p(0, 300),
            p(-300, 300),
            p(-500, 500),
            0,
        ),
    ]);

    let mut scratch = ColumnSearchScratch::default();
    let mut depths = Vec::new();
    let mut rows = Vec::new();
    let mut resolved = Vec::new();

    // Empty depths leaves `out` empty.
    rows.push(999);
    table.find_column([0; 5], &[], &mut scratch, &mut rows);
    assert!(rows.is_empty());

    // Direct tie cases including A == best_fitness and later/earlier indices.
    let edge_targets = [
        [0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 50, 0],
        [0, 0, 0, 0, 100, 0],
        [100, 0, 0, 0, 0, 0],
        [300, 0, 0, 150, -100, 0],
    ];
    resolve_column_by_runs(
        &table,
        &edge_targets,
        &mut scratch,
        &mut depths,
        &mut rows,
        &mut resolved,
    );
    for (target, &row) in edge_targets.iter().zip(&resolved) {
        assert_eq!(Some(row), table.find_row_for_test(target), "{target:?}");
        assert_eq!(Some(row), brute_force_row(&table, target), "{target:?}");
        assert_eq!(
            table.entry_id(row),
            brute_force(&table, target),
            "{target:?}"
        );
    }
    assert_eq!(resolved[0], 0);

    // Reuse the same scratch across a different-sized table and back.
    let small = test_table();
    small.find_column([0; 5], &[0, 1000], &mut scratch, &mut rows);
    assert_eq!(rows, vec![0, 0]);

    // 3,000 random column runs: half uniform 5D fixed coords, half with
    // mid-column fixed-coord splits (including single-cell runs).
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next_i64 = |spread: i64| -> i64 {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 33) as i64 % (2 * spread + 1)) - spread
    };
    let mut column = Vec::with_capacity(24);
    for run in 0..3_000 {
        column.clear();
        let mut fixed = [
            next_i64(600),
            next_i64(600),
            next_i64(600),
            next_i64(600),
            next_i64(600),
        ];
        let len = 16 + (run % 17);
        for cell in 0..len {
            if run % 2 == 1 && (cell == 3 || cell == 4 || cell == 9 || cell == 13) {
                fixed = [
                    next_i64(600),
                    next_i64(600),
                    next_i64(600),
                    next_i64(600),
                    next_i64(600),
                ];
            }
            let depth = next_i64(600);
            column.push([fixed[0], fixed[1], fixed[2], fixed[3], depth, fixed[4]]);
        }
        resolve_column_by_runs(
            &table,
            &column,
            &mut scratch,
            &mut depths,
            &mut rows,
            &mut resolved,
        );
        for (target, &row) in column.iter().zip(&resolved) {
            assert_eq!(
                Some(row),
                table.find_row_for_test(target),
                "run {run} {target:?}"
            );
            assert_eq!(
                Some(row),
                brute_force_row(&table, target),
                "run {run} {target:?}"
            );
            assert_eq!(
                table.entry_id(row),
                table.find(target),
                "run {run} {target:?}"
            );
        }
    }
}

#[test]
fn column_search_handles_negative_depths_above_non_negative_intervals() {
    let entry = |name: &str, t: Param, d: Param, off: i64| {
        let zero = Param { min: 0, max: 0 };
        (
            name.to_owned(),
            ParameterPoint {
                temperature: t,
                humidity: zero,
                continentalness: zero,
                erosion: zero,
                depth: d,
                weirdness: zero,
                offset: off,
            },
        )
    };
    let p = |min, max| Param { min, max };
    // Every row has depth.min >= 0, with overlapping, merged, and disjoint
    // intervals plus deliberate A_i + D(t4) == best_fitness ties.
    let table = BiomePlacement::from_entries(vec![
        entry("deep_low_a", p(0, 0), p(2000, 9000), 0),
        entry("surface_tie_early", p(100, 100), p(0, 0), 0),
        entry("mid_a", p(10, 10), p(500, 1200), 0),
        entry("mid_b_overlap", p(10, 10), p(100, 500), 0),
        entry("surface_tie_late", p(100, 100), p(0, 0), 0),
        entry("surface_offset", p(0, 0), p(0, 0), 100),
        entry("deepest_point", p(20, 20), p(11000, 11000), 0),
        entry("ceiling_point", p(5, 5), p(10000, 10000), 0),
    ]);

    // Merged depth union must be sorted and strictly disjoint.
    assert_eq!(
        table.depth_ranges,
        vec![
            p(0, 0),
            p(100, 1200),
            p(2000, 9000),
            p(10000, 10000),
            p(11000, 11000),
        ]
    );
    for d in [
        -20_000, -5_000, -1, 0, 50, 100, 1500, 5000, 9500, 10500, 15000,
    ] {
        let brute_d = table
            .entries
            .iter()
            .map(|(_, pt)| square(pt.depth.distance(d)))
            .min()
            .expect("non-empty");
        assert_eq!(table.min_depth_fitness(d), brute_d, "depth {d}");
    }

    let mut scratch = ColumnSearchScratch::default();
    let mut depths = Vec::new();
    let mut rows = Vec::new();
    let mut resolved = Vec::new();

    let edge_column = [
        [0, 0, 0, 0, -20_000, 0],
        [0, 0, 0, 0, -5_000, 0],
        [0, 0, 0, 0, -100, 0],
        [0, 0, 0, 0, -1, 0],
        [0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 50, 0],
        [0, 0, 0, 0, 1_500, 0],
        [0, 0, 0, 0, 5_000, 0],
        [0, 0, 0, 0, 12_000, 0],
    ];
    resolve_column_by_runs(
        &table,
        &edge_column,
        &mut scratch,
        &mut depths,
        &mut rows,
        &mut resolved,
    );
    for (target, &row) in edge_column.iter().zip(&resolved) {
        assert_eq!(Some(row), brute_force_row(&table, target), "{target:?}");
        assert_eq!(Some(row), table.find_row_for_test(target), "{target:?}");
    }

    let mut state = 0x517C_C1B7_2722_0A95_u64;
    let mut next_i64 = |low: i64, high: i64| -> i64 {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let span = (high - low + 1) as u64;
        low + ((state >> 32) % span) as i64
    };
    let mut column = Vec::with_capacity(24);
    for run in 0..2_000 {
        column.clear();
        let t = next_i64(-300, 300);
        for _ in 0..16 {
            let d = next_i64(-20_000, -1);
            column.push([t, 0, 0, 0, d, 0]);
        }
        resolve_column_by_runs(
            &table,
            &column,
            &mut scratch,
            &mut depths,
            &mut rows,
            &mut resolved,
        );
        for (target, &row) in column.iter().zip(&resolved) {
            assert_eq!(
                Some(row),
                brute_force_row(&table, target),
                "run {run} {target:?}"
            );
            assert_eq!(
                Some(row),
                table.find_row_for_test(target),
                "run {run} {target:?}"
            );
        }
    }
}

#[test]
fn column_search_fallback_matches_brute_force_when_limit_exceeded() {
    let zero = Param { min: 0, max: 0 };
    let far_depth = Param {
        min: 5_000,
        max: 5_000,
    };
    let surface_depth = Param { min: 0, max: 0 };
    let total = COLUMN_SCAN_FALLBACK_ROWS + 64;
    let mut entries = Vec::with_capacity(total);
    // First COLUMN_SCAN_FALLBACK_ROWS + 20 rows have smaller fixed cost A_i
    // at fixed = [0; 5], but a large depth gap at t4 <= 0 so the scan
    // cannot terminate within COLUMN_SCAN_FALLBACK_ROWS rows.
    for i in 0..COLUMN_SCAN_FALLBACK_ROWS + 20 {
        let coord = i as i64;
        entries.push((
            format!("far_{i}"),
            ParameterPoint {
                temperature: Param {
                    min: coord,
                    max: coord,
                },
                humidity: zero,
                continentalness: zero,
                erosion: zero,
                depth: far_depth,
                weirdness: zero,
                offset: 0,
            },
        ));
    }
    // Remaining rows sit beyond COLUMN_SCAN_FALLBACK_ROWS in sorted A_i
    // order, including deliberate ties at surface_depth = 0.
    let winner_coord = (COLUMN_SCAN_FALLBACK_ROWS + 25) as i64;
    let tie_coord = (COLUMN_SCAN_FALLBACK_ROWS + 30) as i64;
    let early_beyond_row = entries.len();
    entries.push((
        "surface_winner".to_owned(),
        ParameterPoint {
            temperature: Param {
                min: winner_coord,
                max: winner_coord,
            },
            humidity: zero,
            continentalness: zero,
            erosion: zero,
            depth: surface_depth,
            weirdness: zero,
            offset: 0,
        },
    ));
    entries.push((
        "surface_tie_earlier".to_owned(),
        ParameterPoint {
            temperature: Param {
                min: tie_coord,
                max: tie_coord,
            },
            humidity: zero,
            continentalness: zero,
            erosion: zero,
            depth: surface_depth,
            weirdness: zero,
            offset: 0,
        },
    ));
    entries.push((
        "surface_tie_later".to_owned(),
        ParameterPoint {
            temperature: Param {
                min: tie_coord,
                max: tie_coord,
            },
            humidity: zero,
            continentalness: zero,
            erosion: zero,
            depth: surface_depth,
            weirdness: zero,
            offset: 0,
        },
    ));
    while entries.len() < total {
        let coord = (entries.len() + 40) as i64;
        entries.push((
            format!("tail_{coord}"),
            ParameterPoint {
                temperature: Param {
                    min: coord,
                    max: coord,
                },
                humidity: zero,
                continentalness: zero,
                erosion: zero,
                depth: surface_depth,
                weirdness: zero,
                offset: 0,
            },
        ));
    }
    let table = BiomePlacement::from_entries(entries);
    // Confirm the winning row at t4 = 0 sits strictly beyond the scan limit.
    let fixed = [0; 5];
    let winner_a = table.entries[early_beyond_row].1.fixed_fitness(&fixed);
    let smaller_a_count = table
        .entries
        .iter()
        .filter(|(_, pt)| pt.fixed_fitness(&fixed) < winner_a)
        .count();
    assert!(smaller_a_count >= COLUMN_SCAN_FALLBACK_ROWS);

    let depths = [5_000, 2_500, 1_000, 0, -500, -5_000];
    let mut scratch = ColumnSearchScratch::default();
    let mut out = Vec::new();
    // Prime last_winner to a late tying row before the column search.
    table.last_winner.set(Some(early_beyond_row + 2));
    table.find_column(fixed, &depths, &mut scratch, &mut out);
    assert_eq!(out.len(), depths.len());
    assert_eq!(out[0], 0);
    assert_eq!(out[3], early_beyond_row);
    assert_eq!(out[4], early_beyond_row);
    assert_eq!(out[5], early_beyond_row);

    for (&depth, &row) in depths.iter().zip(&out) {
        let target = [fixed[0], fixed[1], fixed[2], fixed[3], depth, fixed[4]];
        assert_eq!(Some(row), brute_force_row(&table, &target), "depth {depth}");
        assert_eq!(Some(row), table.find_row_for_test(&target), "depth {depth}");
    }

    // Verify ties among rows beyond COLUMN_SCAN_FALLBACK_ROWS keep the earlier row.
    let tie_fixed = [tie_coord, 0, 0, 0, 0];
    table.last_winner.set(Some(early_beyond_row + 2));
    table.find_column(tie_fixed, &[0, -200], &mut scratch, &mut out);
    assert_eq!(out, vec![early_beyond_row + 1, early_beyond_row + 1]);
    for &depth in &[0, -200] {
        let target = [tie_fixed[0], 0, 0, 0, depth, 0];
        assert_eq!(brute_force_row(&table, &target), Some(early_beyond_row + 1));
    }
}

#[test]
fn column_search_fixed_and_depth_terms_stay_far_below_i64_max() {
    // Documented bound for quantized coordinates (`value * 10_000`):
    // Overworld climate noises stay roughly within [-2.0, 2.0] (+/-20_000)
    // and depth within [-3.0, 3.0] (+/-30_000), while the 26.3 overworld
    // placement table bounds lie within [-12_000, 11_000] with offset = 0.
    // Even with a conservative 50x margin on targets (|t_d| <= 1_000_000,
    // i.e., |density| <= 100.0) and parameter/offset bounds within
    // [-100_000, 100_000]:
    //   max_distance = 1_100_000
    //   max_term     = 1_100_000^2 = 1_210_000_000_000 (1.21e12)
    //   |A_i|       <= 5 * 1.21e12 + 100_000^2 = 6_060_000_000_000
    //   |A_i| + max_depth_term <= 7_270_000_000_000 (7.27e12),
    // which is over 1,260,000x smaller than i64::MAX (~9.22e18).
    const MAX_QUANTIZED_COORD: i64 = 1_000_000;
    const MAX_PARAM_BOUND: i64 = 100_000;
    const DOCUMENTED_TOTAL_BOUND: i64 = 7_270_000_000_000;

    let corner = Param {
        min: -MAX_PARAM_BOUND,
        max: -MAX_PARAM_BOUND,
    };
    let table = BiomePlacement::from_entries(vec![(
        "extreme".to_owned(),
        ParameterPoint {
            temperature: corner,
            humidity: corner,
            continentalness: corner,
            erosion: corner,
            depth: corner,
            weirdness: corner,
            offset: MAX_PARAM_BOUND,
        },
    )]);
    let target = [MAX_QUANTIZED_COORD; 6];
    let fixed = fixed_climate_coords(&target);
    let point = &table.entries[0].1;

    let a_i = point.fixed_fitness(&fixed);
    let dist4 = point.depth.distance(target[4]);
    let depth_term = dist4.checked_mul(dist4).expect("depth term fits i64");
    let total = a_i
        .checked_abs()
        .and_then(|abs_a| abs_a.checked_add(depth_term))
        .expect("|A_i| + depth_term fits i64");

    let exact_i128 = (5_i128 * 1_100_000_i128 * 1_100_000_i128)
        + (MAX_PARAM_BOUND as i128).pow(2)
        + (1_100_000_i128 * 1_100_000_i128);
    assert_eq!(total as i128, exact_i128);
    assert!(total <= DOCUMENTED_TOTAL_BOUND);
    assert!(total < i64::MAX / 1_000_000);

    let mut scratch = ColumnSearchScratch::default();
    let mut rows = Vec::new();
    table.find_column(fixed, &[target[4]], &mut scratch, &mut rows);
    assert_eq!(rows, vec![0]);
    assert_eq!(brute_force_row(&table, &target), Some(0));
}

/// Run with:
/// `RUSTMC_VANILLA_DATA=<root> cargo test --release -p rustmc-server --lib -- --ignored column_search_matches_find_and_brute_force_on_eight_chunks`
#[test]
#[ignore = "requires operator-provisioned local data"]
fn column_search_matches_find_and_brute_force_on_eight_chunks() {
    use crate::vanilla::worldgen::WorldgenData;

    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
    let table = BiomePlacement::load(&root, "overworld").expect("overworld placement table");
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

    let quart_height = router.height / 4;
    let mut density_scratch = Vec::new();
    let mut targets = Vec::new();
    let mut col_scratch = ColumnSearchScratch::default();
    let mut depths = Vec::new();
    let mut rows = Vec::new();
    let mut resolved = Vec::new();
    let mut checked_cells = 0usize;
    let mut max_total_fitness = 0i64;

    for (cx, cz) in [
        (0, 0),
        (-3, 2),
        (17, -9),
        (40, 40),
        (-120, 7),
        (300, -300),
        (1, 1),
        (2, 2),
    ] {
        let volume = DensityVolume::new(
            [4, quart_height, 4],
            [cx * 16, router.min_y, cz * 16],
            [4, 4, 4],
        )
        .expect("quart volume");
        climate.targets_for_volume(&volume, &mut density_scratch, &mut targets);
        for lz in 0..4usize {
            for lx in 0..4usize {
                let base = volume.index(lx, 0, lz);
                let col = &targets[base..base + quart_height as usize];
                resolve_column_by_runs(
                    &table,
                    col,
                    &mut col_scratch,
                    &mut depths,
                    &mut rows,
                    &mut resolved,
                );
                for (target, &row) in col.iter().zip(&resolved) {
                    assert_eq!(
                        Some(row),
                        table.find_row_for_test(target),
                        "chunk ({cx}, {cz}) cell ({lx}, {lz}) {target:?}"
                    );
                    assert_eq!(
                        Some(row),
                        brute_force_row(&table, target),
                        "chunk ({cx}, {cz}) cell ({lx}, {lz}) {target:?}"
                    );
                    assert_eq!(
                        table.entry_id(row),
                        table.find(target),
                        "chunk ({cx}, {cz}) cell ({lx}, {lz}) {target:?}"
                    );
                    checked_cells += 1;
                }
                let max_a = col_scratch
                    .order
                    .iter()
                    .map(|item| item.fixed_cost.abs())
                    .max()
                    .unwrap_or(0);
                let max_d = col
                    .iter()
                    .flat_map(|t| {
                        table
                            .entries
                            .iter()
                            .map(move |(_, p)| square(p.depth.distance(t[4])))
                    })
                    .max()
                    .unwrap_or(0);
                max_total_fitness = max_total_fitness.max(max_a + max_d);
            }
        }
    }
    assert_eq!(checked_cells, 8 * 4 * 4 * quart_height as usize);
    assert!(
        max_total_fitness < i64::MAX / 1_000_000,
        "max |A_i| + depth term {max_total_fitness} too close to i64::MAX"
    );
    println!("verified {checked_cells} quart cells; max |A_i|+depth_term={max_total_fitness}");
}

impl Param {
    fn default_full() -> Self {
        Self {
            min: -10000,
            max: 10000,
        }
    }
}
