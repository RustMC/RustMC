//! Unit tests for `chunk_adapter::registry`.

use super::*;

fn tables() -> RegistryTables {
    RegistryTables::new(
        SUPPORTED_VERSION,
        SUPPORTED_PROTOCOL,
        65_536,
        128,
        [
            ("minecraft:air".to_string(), 0u32),
            ("minecraft:stone".to_string(), 1),
            ("minecraft:grass_block[snowy=false]".to_string(), 9),
            ("minecraft:oak_leaves[persistent=false]".to_string(), 300),
            ("minecraft:water".to_string(), 700),
            ("minecraft:cave_air".to_string(), 1_024),
        ],
        [
            ("minecraft:plains".to_string(), 0u32),
            ("minecraft:desert".to_string(), 12),
        ],
    )
    .expect("fixture tables")
}

#[test]
fn keys_are_normalized_and_property_order_irrelevant() {
    let tables = tables();
    let first = tables
        .state("minecraft:oak_leaves[persistent=false]")
        .expect("exact state");
    let spaced = tables
        .state("minecraft:oak_leaves[ persistent = false ]")
        .expect("spaced properties normalize to the same key");
    assert_eq!(first, spaced);
    assert_eq!(first.kind, StateKind::Leaves);
    assert_eq!(
        tables.state("minecraft:water").unwrap().kind,
        StateKind::Fluid
    );
    assert_eq!(
        tables.state("minecraft:cave_air").unwrap().kind,
        StateKind::Air,
        "carved rows must not read as solid"
    );
}

#[test]
fn unmappable_names_are_hard_errors() {
    let tables = tables();
    assert_eq!(
        tables.state("minecraft:glow_lichen"),
        Err(RegistryError::UnknownState {
            name: "minecraft:glow_lichen".to_string()
        })
    );
    assert_eq!(
        tables.biome("minecraft:jungle"),
        Err(RegistryError::UnknownBiome {
            name: "minecraft:jungle".to_string()
        })
    );
    for bad in [
        "",
        "minecraft:",
        ":stone",
        "Stone",
        "minecraft:stone[",
        "minecraft:stone[]",
        "minecraft:stone[snowy]",
        "minecraft:stone[snowy=]",
    ] {
        assert!(
            canonical_state_key(bad).is_err(),
            "`{bad}` must not parse as a state key"
        );
    }
}

#[test]
fn versions_and_id_ranges_are_enforced() {
    let error = RegistryTables::new(
        "26.2",
        SUPPORTED_PROTOCOL,
        65_536,
        128,
        [("minecraft:air".to_string(), 0u32)],
        [],
    )
    .unwrap_err();
    assert!(matches!(error, RegistryError::UnsupportedVersion { .. }));
    let error = RegistryTables::new(
        SUPPORTED_VERSION,
        766,
        65_536,
        128,
        [("minecraft:air".to_string(), 0u32)],
        [],
    )
    .unwrap_err();
    assert!(matches!(error, RegistryError::UnsupportedVersion { .. }));
    assert_eq!(
        RegistryTables::new(
            SUPPORTED_VERSION,
            SUPPORTED_PROTOCOL,
            16,
            128,
            [
                ("minecraft:air".to_string(), 0u32),
                ("minecraft:stone".to_string(), 16)
            ],
            [],
        )
        .unwrap_err(),
        RegistryError::IdOutOfRange {
            name: "minecraft:stone".to_string(),
            id: 16,
            count: 16
        }
    );
    assert!(
        RegistryTables::new(
            SUPPORTED_VERSION,
            SUPPORTED_PROTOCOL,
            65_536,
            128,
            [("minecraft:stone".to_string(), 1u32)],
            [],
        )
        .is_err(),
        "a table without minecraft:air cannot fill the column"
    );
    assert!(
        RegistryTables::new(
            SUPPORTED_VERSION,
            SUPPORTED_PROTOCOL,
            0,
            128,
            [("minecraft:air".to_string(), 0u32)],
            [],
        )
        .is_err()
    );
}

#[test]
fn duplicates_are_rejected_with_the_canonical_name() {
    let error = RegistryTables::new(
        SUPPORTED_VERSION,
        SUPPORTED_PROTOCOL,
        65_536,
        128,
        [
            ("minecraft:air".to_string(), 0u32),
            ("minecraft:air".to_string(), 0),
        ],
        [],
    )
    .unwrap_err();
    assert_eq!(
        error,
        RegistryError::DuplicateKey("minecraft:air".to_string())
    );
}

#[test]
fn direct_widths_follow_the_declared_registry_sizes() {
    let tables = tables();
    // 65_536 states need 16 bits and 128 biomes need 7. The fixture sizes
    // are invented, but chosen so both derived widths match what the real
    // registries of a provisioned 26.3 table produce, which keeps the
    // assertions meaningful without carrying any Mojang registry count
    // into the repository.
    assert_eq!(tables.state_bits(), 16);
    assert_eq!(tables.biome_bits(), 7);
    assert_eq!(ceillog2(1), 0);
    assert_eq!(ceillog2(2), 1);
    assert_eq!(ceillog2(3), 2);
    assert_eq!(ceillog2(256), 8);
    assert_eq!(ceillog2(257), 9);
    let smaller = RegistryTables::new(
        SUPPORTED_VERSION,
        SUPPORTED_PROTOCOL,
        16,
        8,
        [("minecraft:air".to_string(), 0u32)],
        [("minecraft:plains".to_string(), 0u32)],
    )
    .expect("small registries");
    assert_eq!(smaller.state_bits(), 4);
    assert_eq!(smaller.biome_bits(), 3);
}

#[test]
fn classification_and_heightmap_capture_follow_the_documented_rules() {
    assert_eq!(
        StateKind::from_state_name("minecraft:water"),
        StateKind::Fluid
    );
    assert_eq!(
        StateKind::from_state_name("minecraft:mangrove_leaves[waterlogged=true]"),
        StateKind::Leaves
    );
    assert_eq!(
        StateKind::from_state_name("minecraft:deepslate"),
        StateKind::Solid
    );
    for kind in [
        StateKind::Air,
        StateKind::Fluid,
        StateKind::Leaves,
        StateKind::Solid,
    ] {
        assert_eq!(
            (
                kind.captured_by(HeightmapKind::WorldSurface),
                kind.captured_by(HeightmapKind::MotionBlocking),
                kind.captured_by(HeightmapKind::MotionBlockingNoLeaves),
            ),
            match kind {
                StateKind::Air => (false, false, false),
                StateKind::Fluid => (true, true, true),
                StateKind::Leaves => (true, true, false),
                StateKind::Solid => (true, true, true),
            }
        );
    }
    assert_eq!(StateKind::Air.sky_attenuation(), 0);
    assert_eq!(StateKind::Leaves.sky_attenuation(), 1);
    assert_eq!(StateKind::Fluid.sky_attenuation(), 1);
    assert_eq!(StateKind::Solid.sky_attenuation(), 15);
}

#[test]
fn kind_overrides_retarget_both_lookups() {
    let tables = tables()
        .with_kind_override("minecraft:stone", StateKind::Leaves)
        .expect("override");
    assert_eq!(
        tables.state("minecraft:stone").unwrap().kind,
        StateKind::Leaves
    );
    assert_eq!(tables.kind(1), Some(StateKind::Leaves));
    assert!(
        tables
            .clone()
            .with_kind_override("minecraft:glow_lichen", StateKind::Air)
            .is_err()
    );
    // An override cannot make the id lookup ambiguous: each id keeps the
    // class of the entry that carries it.
    assert_eq!(tables.state("minecraft:air").unwrap().kind, StateKind::Air);
}

#[test]
fn provisioned_json_round_trips_the_same_validation() {
    let text = r#"{
        "version": "26.3",
        "protocol": 777,
        "block_state_count": 65536,
        "biome_count": 128,
        "block_states": {"minecraft:air": 0, "minecraft:stone": 1, "minecraft:kelp_plant": 40},
        "biomes": {"minecraft:plains": 0},
        "state_kinds": {"minecraft:kelp_plant": "leaves"}
    }"#;
    let tables = RegistryTables::from_provisioned(text).expect("provisioned table");
    assert_eq!(tables.version(), SUPPORTED_VERSION);
    assert_eq!(tables.protocol(), SUPPORTED_PROTOCOL);
    assert_eq!(
        tables.state("minecraft:kelp_plant").unwrap().kind,
        StateKind::Leaves
    );
    assert_eq!(tables.biome("minecraft:plains"), Ok(0));
    for broken in [
        r#"{"version": "26.3"}"#,
        r#"{"version": "26.2", "protocol": 777, "block_state_count": 1, "biome_count": 1, "block_states": {}, "biomes": {}}"#,
        r#"not json"#,
        r#"{"version": "26.3", "protocol": 777, "block_state_count": 1, "biome_count": 1, "block_states": [], "biomes": {}}"#,
        r#"{"version": "26.3", "protocol": 777, "block_state_count": 2, "biome_count": 1, "block_states": {"minecraft:air": 5}, "biomes": {}}"#,
        r#"{"version": "26.3", "protocol": 777, "block_state_count": 2, "biome_count": 1, "block_states": {"minecraft:air": 0}, "biomes": {}, "state_kinds": {"minecraft:air": "plasma"}}"#,
    ] {
        assert!(
            RegistryTables::from_provisioned(broken).is_err(),
            "table `{broken}` must be rejected"
        );
    }
}
