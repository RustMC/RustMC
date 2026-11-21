//! Unit tests for `protocol::ids`.

use super::*;

fn scratch_root(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rustmc-protocol-ids-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

const SYNTHETIC_REGISTRIES: &str = r#"{
    "minecraft:block": {
        "entries": {
            "minecraft:air": { "protocol_id": 0 },
            "minecraft:stone": { "protocol_id": 1 }
        }
    },
    "minecraft:item": {
        "default": "minecraft:air",
        "entries": {
            "minecraft:air": { "protocol_id": 0 },
            "minecraft:stone": { "protocol_id": 1 },
            "minecraft:diamond_sword": { "protocol_id": 851 }
        }
    },
    "minecraft:data_component_type": {
        "entries": {
            "minecraft:damage": { "protocol_id": 3 },
            "minecraft:custom_name": { "protocol_id": 6 },
            "minecraft:enchantments": { "protocol_id": 13 }
        }
    },
    "minecraft:potion": {
        "entries": {
            "minecraft:water": { "protocol_id": 0 },
            "minecraft:awkward": { "protocol_id": 3 }
        }
    },
    "minecraft:mob_effect": {
        "entries": {
            "minecraft:speed": { "protocol_id": 0 },
            "minecraft:night_vision": { "protocol_id": 15 },
            "minecraft:saturation": { "protocol_id": 22 }
        }
    }
}"#;

#[test]
fn loads_only_needed_static_registries_from_disk_and_resolves_both_ways() {
    let root = scratch_root("valid");
    let reports_dir = root.join("reports");
    std::fs::create_dir_all(&reports_dir).expect("create reports dir");
    let file = reports_dir.join("registries.json");
    std::fs::write(&file, SYNTHETIC_REGISTRIES).expect("write registries.json");

    let ids = ProtocolIds::load(&root).expect("load synthetic registries.json");
    assert_eq!(ids.item_count(), 3);
    assert_eq!(ids.component_type_count(), 3);
    assert_eq!(ids.potion_count(), 2);
    assert_eq!(ids.mob_effect_count(), 3);

    assert_eq!(ids.item_id("minecraft:stone"), Some(1));
    assert_eq!(ids.item_id("minecraft:diamond_sword"), Some(851));
    assert_eq!(ids.item_name(851), Some("minecraft:diamond_sword"));
    assert_eq!(ids.item_id("minecraft:missing"), None);
    assert_eq!(ids.item_name(999), None);

    assert_eq!(ids.component_type_id("minecraft:damage"), Some(3));
    assert_eq!(ids.component_type_id("minecraft:custom_name"), Some(6));
    assert_eq!(ids.component_type_id("minecraft:enchantments"), Some(13));
    assert_eq!(ids.component_type_name(6), Some("minecraft:custom_name"));
    assert_eq!(ids.component_type_id("minecraft:unknown"), None);

    assert_eq!(ids.potion_id("minecraft:water"), Some(0));
    assert_eq!(ids.potion_id("minecraft:awkward"), Some(3));
    assert_eq!(ids.potion_name(3), Some("minecraft:awkward"));
    assert_eq!(ids.potion_name(99), None);

    assert_eq!(ids.mob_effect_id("minecraft:speed"), Some(0));
    assert_eq!(ids.mob_effect_id("minecraft:night_vision"), Some(15));
    assert_eq!(ids.mob_effect_id("minecraft:saturation"), Some(22));
    assert_eq!(ids.mob_effect_name(15), Some("minecraft:night_vision"));
    assert_eq!(ids.mob_effect_name(99), None);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn errors_name_the_file_for_missing_malformed_or_duplicate_registries() {
    let root = scratch_root("missing");
    let missing_file = root.join("reports").join("registries.json");
    let err = ProtocolIds::load(&root).expect_err("missing file must error");
    assert!(matches!(err, ProtocolIdsError::Io { .. }));
    assert!(
        err.to_string()
            .contains(&missing_file.display().to_string()),
        "error must name file: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);

    let path = Path::new("reports/registries.json");

    let json_err = ProtocolIds::from_json_str(path, "{bad json").expect_err("bad json");
    assert!(matches!(json_err, ProtocolIdsError::Json { .. }));
    assert!(json_err.to_string().contains("reports/registries.json"));

    let top_err = ProtocolIds::from_json_str(path, "[]").expect_err("non-object root");
    assert!(matches!(top_err, ProtocolIdsError::InvalidFormat { .. }));
    assert!(top_err.to_string().contains("reports/registries.json"));

    let missing_reg = ProtocolIds::from_json_str(
        path,
        r#"{"minecraft:item":{"entries":{}},"minecraft:data_component_type":{"entries":{}}}"#,
    )
    .expect_err("missing potion registry");
    assert!(matches!(
        missing_reg,
        ProtocolIdsError::MissingRegistry {
            registry: "minecraft:potion",
            ..
        }
    ));
    assert!(missing_reg.to_string().contains("reports/registries.json"));

    let missing_mob_effect = ProtocolIds::from_json_str(
        path,
        r#"{"minecraft:item":{"entries":{}},"minecraft:data_component_type":{"entries":{}},"minecraft:potion":{"entries":{}}}"#,
    )
    .expect_err("missing mob_effect registry");
    assert!(matches!(
        missing_mob_effect,
        ProtocolIdsError::MissingRegistry {
            registry: "minecraft:mob_effect",
            ..
        }
    ));

    let dup_err = ProtocolIds::from_json_str(
        path,
        r#"{
            "minecraft:item": { "entries": {
                "minecraft:a": { "protocol_id": 1 },
                "minecraft:b": { "protocol_id": 1 }
            }},
            "minecraft:data_component_type": { "entries": {} },
            "minecraft:potion": { "entries": {} },
            "minecraft:mob_effect": { "entries": {} }
        }"#,
    )
    .expect_err("duplicate protocol_id must error");
    assert!(matches!(
        dup_err,
        ProtocolIdsError::InvalidRegistry {
            registry: "minecraft:item",
            ..
        }
    ));
    assert!(dup_err.to_string().contains("reports/registries.json"));

    let neg_err = ProtocolIds::from_json_str(
        path,
        r#"{
            "minecraft:item": { "entries": {} },
            "minecraft:data_component_type": { "entries": { "minecraft:x": { "protocol_id": -1 } } },
            "minecraft:potion": { "entries": {} },
            "minecraft:mob_effect": { "entries": {} }
        }"#,
    )
    .expect_err("negative protocol_id must error");
    assert!(matches!(
        neg_err,
        ProtocolIdsError::InvalidRegistry {
            registry: "minecraft:data_component_type",
            ..
        }
    ));
    assert!(neg_err.to_string().contains("reports/registries.json"));
}

#[test]
fn dynamic_registry_ids_preserve_manifest_order_and_handle_absent_registries() {
    let manifest = RegistryManifest {
        tags: Vec::new(),
        registries: vec![
            (
                "minecraft:worldgen/biome".to_owned(),
                vec!["minecraft:plains".to_owned()],
            ),
            (
                "minecraft:enchantment".to_owned(),
                vec![
                    "minecraft:sharpness".to_owned(),
                    "minecraft:unbreaking".to_owned(),
                    "minecraft:mending".to_owned(),
                ],
            ),
            (
                "minecraft:trim_pattern".to_owned(),
                vec!["minecraft:coast".to_owned(), "minecraft:ward".to_owned()],
            ),
            (
                "minecraft:trim_material".to_owned(),
                vec![
                    "minecraft:amethyst".to_owned(),
                    "minecraft:diamond".to_owned(),
                ],
            ),
            (
                "minecraft:instrument".to_owned(),
                vec![
                    "minecraft:ponder_goat_horn".to_owned(),
                    "minecraft:sing_goat_horn".to_owned(),
                ],
            ),
        ],
    };

    let dyn_ids = DynamicRegistryIds::from_manifest(&manifest);
    assert_eq!(dyn_ids.enchantment_count(), 3);
    assert_eq!(dyn_ids.trim_pattern_count(), 2);
    assert_eq!(dyn_ids.trim_material_count(), 2);
    assert_eq!(dyn_ids.instrument_count(), 2);

    assert_eq!(dyn_ids.enchantment_id("minecraft:sharpness"), Some(0));
    assert_eq!(dyn_ids.enchantment_id("minecraft:unbreaking"), Some(1));
    assert_eq!(dyn_ids.enchantment_id("minecraft:mending"), Some(2));
    assert_eq!(dyn_ids.enchantment_name(2), Some("minecraft:mending"));
    assert_eq!(dyn_ids.enchantment_name(3), None);
    assert_eq!(dyn_ids.enchantment_id("minecraft:looting"), None);

    assert_eq!(dyn_ids.trim_pattern_id("minecraft:coast"), Some(0));
    assert_eq!(dyn_ids.trim_pattern_id("minecraft:ward"), Some(1));
    assert_eq!(dyn_ids.trim_pattern_name(1), Some("minecraft:ward"));
    assert_eq!(dyn_ids.trim_pattern_name(2), None);

    assert_eq!(dyn_ids.trim_material_id("minecraft:amethyst"), Some(0));
    assert_eq!(dyn_ids.trim_material_id("minecraft:diamond"), Some(1));
    assert_eq!(dyn_ids.trim_material_name(0), Some("minecraft:amethyst"));
    assert_eq!(dyn_ids.trim_material_name(2), None);

    assert_eq!(dyn_ids.instrument_id("minecraft:ponder_goat_horn"), Some(0));
    assert_eq!(dyn_ids.instrument_id("minecraft:sing_goat_horn"), Some(1));
    assert_eq!(dyn_ids.instrument_name(1), Some("minecraft:sing_goat_horn"));
    assert_eq!(dyn_ids.instrument_name(2), None);
    assert_eq!(dyn_ids.instrument_id("minecraft:admire_goat_horn"), None);

    let empty = DynamicRegistryIds::from_manifest(&RegistryManifest {
        tags: Vec::new(),
        registries: vec![(
            "minecraft:dimension_type".to_owned(),
            vec!["minecraft:overworld".to_owned()],
        )],
    });
    assert_eq!(empty.enchantment_count(), 0);
    assert_eq!(empty.trim_pattern_count(), 0);
    assert_eq!(empty.trim_material_count(), 0);
    assert_eq!(empty.instrument_count(), 0);
    assert_eq!(empty.enchantment_id("minecraft:sharpness"), None);
    assert_eq!(empty.instrument_id("minecraft:ponder_goat_horn"), None);
}

/// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored vanilla_registries_pin_26_3_protocol_ids`
#[test]
#[ignore]
fn vanilla_registries_pin_26_3_protocol_ids() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
    let root_path = Path::new(&root);
    let ids = ProtocolIds::load(root_path).expect("load reports/registries.json");

    let raw = std::fs::read_to_string(root_path.join("reports").join("registries.json"))
        .expect("read reports/registries.json");
    let parsed: Value = serde_json::from_str(&raw).expect("parse registries.json");

    let read_expected = |registry: &str, entry: &str| -> u32 {
        u32::try_from(
            parsed[registry]["entries"][entry]["protocol_id"]
                .as_u64()
                .expect("protocol_id in registries.json"),
        )
        .expect("protocol_id fits u32")
    };

    assert_eq!(
        parsed["minecraft:item"]["entries"]
            .as_object()
            .expect("item entries")
            .len(),
        1658
    );
    assert_eq!(
        parsed["minecraft:data_component_type"]["entries"]
            .as_object()
            .expect("component entries")
            .len(),
        122
    );
    assert_eq!(
        parsed["minecraft:potion"]["entries"]
            .as_object()
            .expect("potion entries")
            .len(),
        46
    );
    assert_eq!(
        parsed["minecraft:mob_effect"]["entries"]
            .as_object()
            .expect("mob_effect entries")
            .len(),
        40
    );

    assert_eq!(ids.item_count(), 1658);
    assert_eq!(ids.component_type_count(), 122);
    assert_eq!(ids.potion_count(), 46);
    assert_eq!(ids.mob_effect_count(), 40);

    let expected_components = [
        ("minecraft:damage", 3),
        ("minecraft:custom_name", 6),
        ("minecraft:item_name", 9),
        ("minecraft:enchantments", 13),
        ("minecraft:repair_cost", 19),
        ("minecraft:stored_enchantments", 45),
        ("minecraft:potion_contents", 53),
        ("minecraft:suspicious_stew_effects", 55),
        ("minecraft:trim", 58),
        ("minecraft:instrument", 63),
        ("minecraft:ominous_bottle_amplifier", 65),
    ];
    for (name, pinned) in expected_components {
        let from_file = read_expected("minecraft:data_component_type", name);
        assert_eq!(from_file, pinned, "file mismatch for {name}");
        assert_eq!(ids.component_type_id(name), Some(from_file));
        assert_eq!(ids.component_type_name(from_file), Some(name));
    }

    let expected_mob_effects = [
        ("minecraft:speed", 0),
        ("minecraft:night_vision", 15),
        ("minecraft:poison", 18),
        ("minecraft:saturation", 22),
    ];
    for (name, pinned) in expected_mob_effects {
        let from_file = read_expected("minecraft:mob_effect", name);
        assert_eq!(from_file, pinned, "file mismatch for {name}");
        assert_eq!(ids.mob_effect_id(name), Some(from_file));
        assert_eq!(ids.mob_effect_name(from_file), Some(name));
    }

    let awkward_from_file = read_expected("minecraft:potion", "minecraft:awkward");
    assert_eq!(awkward_from_file, 3);
    assert_eq!(ids.potion_id("minecraft:awkward"), Some(awkward_from_file));
    assert_eq!(
        ids.potion_name(awkward_from_file),
        Some("minecraft:awkward")
    );

    let stone_from_file = read_expected("minecraft:item", "minecraft:stone");
    assert_eq!(ids.item_id("minecraft:stone"), Some(stone_from_file));
    assert_eq!(ids.item_name(stone_from_file), Some("minecraft:stone"));
}
