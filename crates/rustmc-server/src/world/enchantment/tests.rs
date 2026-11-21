//! Unit tests for `world::enchantment`.

use super::*;

fn scratch_root(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "rustmc-enchantment-{label}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create scratch dir");
    path
}

fn write_file(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    std::fs::write(path, text).expect("write");
}

#[test]
fn loads_synthetic_enchantments_resolves_tags_and_checks_compatibility() {
    let root = scratch_root("load-ok");
    write_file(
        &root.join("data/testns/tags/item/blades.json"),
        r#"{"values": ["testns:copper_blade", "testns:iron_blade"]}"#,
    );
    write_file(
        &root.join("data/testns/tags/item/heavy_blades.json"),
        r##"{"values": ["#testns:blades", "testns:battle_axe"]}"##,
    );
    write_file(
        &root.join("data/testns/tags/enchantment/exclusive_set/blade_damage.json"),
        r#"{"values": ["testns:keen_edge", "testns:blunt_force"]}"#,
    );

    write_file(
        &root.join("data/testns/enchantment/keen_edge.json"),
        r##"{
            "description": {"translate": "enchantment.testns.keen_edge"},
            "effects": {"minecraft:damage": []},
            "exclusive_set": "#testns:exclusive_set/blade_damage",
            "supported_items": "#testns:heavy_blades",
            "primary_items": "#testns:blades",
            "weight": 10,
            "max_level": 5,
            "min_cost": {"base": 1, "per_level_above_first": 11},
            "max_cost": {"base": 21, "per_level_above_first": 11},
            "anvil_cost": 1,
            "slots": ["mainhand"]
        }"##,
    );
    write_file(
        &root.join("data/testns/enchantment/blunt_force.json"),
        r#"{"supported_items": "testns:battle_axe",
            "weight": 5,
            "max_level": 3,
            "min_cost": {"base": 5, "per_level_above_first": 8},
            "max_cost": {"base": 25, "per_level_above_first": 8},
            "anvil_cost": 2,
            "slots": ["mainhand", "offhand"]}"#,
    );
    write_file(
        &root.join("data/testns/enchantment/sturdy.json"),
        r##"{"supported_items": ["testns:copper_blade", "testns:shield"],
            "weight": 5,
            "max_level": 3,
            "min_cost": {"base": 5, "per_level_above_first": 8},
            "max_cost": {"base": 55, "per_level_above_first": 8},
            "anvil_cost": 2,
            "slots": ["any"]}"##,
    );

    let item_tags = ItemTags::load(&root).expect("load item tags");
    let ench_tags = EnchantmentTags::load(&root).expect("load enchantment tags");
    let registry =
        EnchantmentRegistry::load(&root, &item_tags, &ench_tags).expect("load enchantments");

    assert_eq!(registry.len(), 3);
    assert!(!registry.is_empty());

    let keen = registry.get("testns:keen_edge").expect("keen_edge");
    assert_eq!(keen.max_level(), 5);
    assert_eq!(keen.anvil_cost(), 1);
    assert_eq!(keen.weight(), 10);
    assert_eq!(keen.min_cost(), EnchantmentCost::new(1, 11));
    assert_eq!(keen.max_cost(), EnchantmentCost::new(21, 11));
    assert_eq!(keen.min_cost().at_level(3), 23);
    assert_eq!(keen.slots(), &["mainhand".to_owned()]);
    assert_eq!(
        keen.supported_items(),
        &[
            "testns:battle_axe".to_owned(),
            "testns:copper_blade".to_owned(),
            "testns:iron_blade".to_owned(),
        ]
    );
    assert_eq!(
        keen.primary_items(),
        &[
            "testns:copper_blade".to_owned(),
            "testns:iron_blade".to_owned(),
        ]
    );
    assert_eq!(
        keen.exclusive_set(),
        &[
            "testns:blunt_force".to_owned(),
            "testns:keen_edge".to_owned(),
        ]
    );

    // Omitted primary_items defaults to supported_items; omitted exclusive_set is empty.
    let blunt = registry.get("testns:blunt_force").expect("blunt_force");
    assert_eq!(blunt.explicit_primary_items(), None);
    assert_eq!(blunt.primary_items(), &["testns:battle_axe".to_owned()]);
    assert!(blunt.exclusive_set().is_empty());

    // Compatibility checks symmetry even when exclusive_set is one-sided.
    assert!(!registry.are_compatible("testns:keen_edge", "testns:keen_edge"));
    assert!(!registry.are_compatible("testns:keen_edge", "testns:blunt_force"));
    assert!(!registry.are_compatible("testns:blunt_force", "testns:keen_edge"));
    assert!(registry.are_compatible("testns:keen_edge", "testns:sturdy"));
    assert!(!registry.are_compatible("testns:keen_edge", "testns:absent"));

    // can_apply checks supported_items.
    assert!(registry.can_apply("testns:keen_edge", "testns:battle_axe"));
    assert!(!registry.can_apply("testns:keen_edge", "testns:shield"));
    assert!(registry.can_apply("testns:sturdy", "testns:shield"));

    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn rejects_unknown_fields_and_invalid_values_naming_file() {
    let base_valid = r#"{
        "supported_items": "testns:widget",
        "weight": 5,
        "max_level": 3,
        "min_cost": {"base": 1, "per_level_above_first": 5},
        "max_cost": {"base": 10, "per_level_above_first": 5},
        "anvil_cost": 2,
        "slots": ["mainhand"]
    }"#;

    let bad_cases = [
        (
            "unknown_top.json",
            r#"{
                "supported_items": "testns:widget",
                "weight": 5,
                "max_level": 3,
                "min_cost": {"base": 1, "per_level_above_first": 5},
                "max_cost": {"base": 10, "per_level_above_first": 5},
                "anvil_cost": 2,
                "slots": ["mainhand"],
                "treasure": true
            }"#,
        ),
        (
            "unknown_cost_field.json",
            r#"{
                "supported_items": "testns:widget",
                "weight": 5,
                "max_level": 3,
                "min_cost": {"base": 1, "per_level_above_first": 5, "extra": 9},
                "max_cost": {"base": 10, "per_level_above_first": 5},
                "anvil_cost": 2,
                "slots": ["mainhand"]
            }"#,
        ),
        (
            "zero_max_level.json",
            r#"{
                "supported_items": "testns:widget",
                "weight": 5,
                "max_level": 0,
                "min_cost": {"base": 1, "per_level_above_first": 5},
                "max_cost": {"base": 10, "per_level_above_first": 5},
                "anvil_cost": 2,
                "slots": ["mainhand"]
            }"#,
        ),
        (
            "bad_weight.json",
            r#"{
                "supported_items": "testns:widget",
                "weight": 1025,
                "max_level": 1,
                "min_cost": {"base": 1, "per_level_above_first": 5},
                "max_cost": {"base": 10, "per_level_above_first": 5},
                "anvil_cost": 2,
                "slots": ["mainhand"]
            }"#,
        ),
        (
            "bad_slot.json",
            r#"{
                "supported_items": "testns:widget",
                "weight": 5,
                "max_level": 1,
                "min_cost": {"base": 1, "per_level_above_first": 5},
                "max_cost": {"base": 10, "per_level_above_first": 5},
                "anvil_cost": 2,
                "slots": ["pocket"]
            }"#,
        ),
        (
            "missing_item_tag.json",
            r##"{
                "supported_items": "#testns:absent",
                "weight": 5,
                "max_level": 1,
                "min_cost": {"base": 1, "per_level_above_first": 5},
                "max_cost": {"base": 10, "per_level_above_first": 5},
                "anvil_cost": 2,
                "slots": ["mainhand"]
            }"##,
        ),
    ];

    for (name, body) in bad_cases {
        let root = scratch_root(name);
        write_file(&root.join("data/testns/enchantment").join(name), body);
        let err =
            EnchantmentRegistry::load(&root, &ItemTags::default(), &EnchantmentTags::default())
                .expect_err("invalid enchantment must fail")
                .to_string();
        assert!(err.contains(name), "error must name {name}: {err}");
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    // Sanity check that the baseline JSON itself loads cleanly.
    let ok_root = scratch_root("baseline");
    write_file(&ok_root.join("data/testns/enchantment/ok.json"), base_valid);
    let ok = EnchantmentRegistry::load(&ok_root, &ItemTags::default(), &EnchantmentTags::default())
        .expect("baseline loads");
    assert_eq!(ok.len(), 1);
    std::fs::remove_dir_all(ok_root).expect("cleanup");
}

/// Run with:
/// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_enchantment_registry_pins_vanilla_definitions`
#[test]
#[ignore = "requires operator-provisioned local data"]
fn pack_enchantment_registry_pins_vanilla_definitions() {
    let root = std::env::var("RUSTMC_VANILLA_DATA")
        .map(PathBuf::from)
        .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
    let item_tags = ItemTags::load(&root).expect("load vanilla item tags");
    let ench_tags = EnchantmentTags::load(&root).expect("load vanilla enchantment tags");
    let registry = EnchantmentRegistry::load(&root, &item_tags, &ench_tags)
        .expect("load vanilla enchantments");

    assert_eq!(registry.len(), 43);

    let sharpness = registry.get("minecraft:sharpness").expect("sharpness");
    assert_eq!(sharpness.max_level(), 5);
    assert_eq!(sharpness.anvil_cost(), 1);
    assert_eq!(sharpness.weight(), 10);
    assert_eq!(registry.get("sharpness"), Some(sharpness));

    assert!(!registry.are_compatible("minecraft:sharpness", "minecraft:smite"));
    assert!(registry.are_compatible("minecraft:sharpness", "minecraft:unbreaking"));

    // Verify exclusive_set contents for mending & infinity directly from data.
    let mending = registry.get("minecraft:mending").expect("mending");
    let infinity = registry.get("minecraft:infinity").expect("infinity");
    assert!(mending.exclusive_set().is_empty());
    assert!(
        infinity
            .exclusive_set()
            .iter()
            .any(|id| id == "minecraft:mending")
    );
    assert!(!registry.are_compatible("minecraft:mending", "minecraft:infinity"));

    // Loyalty + Channeling are compatible; Riptide excludes both.
    assert!(registry.are_compatible("minecraft:loyalty", "minecraft:channeling"));
    assert!(!registry.are_compatible("minecraft:riptide", "minecraft:loyalty"));
    assert!(!registry.are_compatible("minecraft:riptide", "minecraft:channeling"));

    // Sharpness applies to diamond_sword (and axes) via supported_items, not to bow.
    assert!(registry.can_apply("minecraft:sharpness", "minecraft:diamond_sword"));
    assert!(registry.can_apply("minecraft:sharpness", "minecraft:diamond_axe"));
    assert!(!registry.can_apply("minecraft:sharpness", "minecraft:bow"));
}
