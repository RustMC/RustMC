//! Unit tests for `item`.

use super::*;

#[test]
fn try_new_accepts_valid_stacks_and_default_max_stack() {
    let one = ItemStack::try_new("minecraft:stone", 1, 64).unwrap();
    assert_eq!(one.item(), "minecraft:stone");
    assert_eq!(one.count(), 1);
    assert_eq!(one.max_stack(), 64);

    let full = ItemStack::try_new("minecraft:stone", 64, DEFAULT_MAX_STACK).unwrap();
    assert_eq!(full.count(), 64);
    assert_eq!(full.max_stack(), DEFAULT_MAX_STACK);

    let potion = ItemStack::try_new("minecraft:potion", 1, 1).unwrap();
    assert_eq!(potion.item(), "minecraft:potion");
    assert_eq!(potion.count(), 1);
    assert_eq!(potion.max_stack(), 1);

    let max = ItemStack::try_new("a:b", 99, MAX_STACK_LIMIT).unwrap();
    assert_eq!(max.item(), "a:b");
    assert_eq!(max.count(), 99);
    assert_eq!(max.max_stack(), MAX_STACK_LIMIT);
}

#[test]
fn validates_item_id_character_and_separator_rules() {
    for invalid in [
        "",
        "stone",
        ":stone",
        "minecraft:",
        "minecraft:stone:extra",
        "Minecraft:stone",
        "minecraft:Stone",
        "minecraft:sto ne",
        "my/mod:stone",
    ] {
        assert_eq!(
            ItemStack::try_new(invalid, 1, DEFAULT_MAX_STACK),
            Err(ItemStackError::InvalidItem),
            "expected InvalidItem for `{invalid}`"
        );
    }

    let custom = ItemStack::try_new("my_mod.x-1:block/a_b.c-2", 1, DEFAULT_MAX_STACK).unwrap();
    assert_eq!(custom.item(), "my_mod.x-1:block/a_b.c-2");
}

#[test]
fn rejects_invalid_max_stack_and_prioritizes_earlier_checks() {
    assert_eq!(
        ItemStack::try_new("minecraft:stone", 1, 0),
        Err(ItemStackError::InvalidMaxStack(0))
    );
    assert_eq!(
        ItemStack::try_new("minecraft:stone", 1, 100),
        Err(ItemStackError::InvalidMaxStack(100))
    );
    assert_eq!(
        ItemStack::try_new("NotValid", 1, 0),
        Err(ItemStackError::InvalidItem)
    );
    assert_eq!(
        ItemStack::try_new("NotValid", 0, 100),
        Err(ItemStackError::InvalidItem)
    );
    assert_eq!(
        ItemStack::try_new("minecraft:stone", 0, 0),
        Err(ItemStackError::InvalidMaxStack(0))
    );
}

#[test]
fn rejects_zero_or_over_limit_count() {
    assert_eq!(
        ItemStack::try_new("minecraft:stone", 0, 64),
        Err(ItemStackError::InvalidCount(0))
    );
    assert_eq!(
        ItemStack::try_new("minecraft:stone", 65, 64),
        Err(ItemStackError::InvalidCount(65))
    );
    assert_eq!(
        ItemStack::try_new("minecraft:potion", 2, 1),
        Err(ItemStackError::InvalidCount(2))
    );
}

#[test]
fn error_display_is_lowercase_and_never_echoes_item_id() {
    let item_err = ItemStack::try_new("secret_ns:Bad_Item", 1, 64).unwrap_err();
    let msg = item_err.to_string();
    assert!(!msg.is_empty());
    assert_eq!(msg, msg.to_lowercase());
    assert!(!msg.contains("secret_ns"));
    assert!(!msg.contains("bad_item"));

    for err in [
        ItemStackError::InvalidMaxStack(0),
        ItemStackError::InvalidCount(65),
    ] {
        let text = err.to_string();
        assert!(!text.is_empty());
        assert_eq!(text, text.to_lowercase());
        let _: &dyn std::error::Error = &err;
    }
}

#[test]
fn can_stack_with_requires_same_id_and_max_stack() {
    let stone_one = ItemStack::try_new("minecraft:stone", 1, 64).unwrap();
    let stone_ten = ItemStack::try_new("minecraft:stone", 10, 64).unwrap();
    let dirt = ItemStack::try_new("minecraft:dirt", 1, 64).unwrap();
    let stone_small_max = ItemStack::try_new("minecraft:stone", 1, 16).unwrap();

    assert!(stone_one.can_stack_with(&stone_ten));
    assert!(!stone_one.can_stack_with(&dirt));
    assert!(!stone_one.can_stack_with(&stone_small_max));
}

#[test]
fn add_caps_at_max_stack_and_returns_overflow() {
    let mut stack = ItemStack::try_new("minecraft:stone", 60, 64).unwrap();
    assert_eq!(stack.add(0), 0);
    assert_eq!(stack.count(), 60);

    assert_eq!(stack.add(4), 0);
    assert_eq!(stack.count(), 64);

    let mut near_full = ItemStack::try_new("minecraft:stone", 60, 64).unwrap();
    assert_eq!(near_full.add(10), 6);
    assert_eq!(near_full.count(), 64);

    assert_eq!(near_full.add(25), 25);
    assert_eq!(near_full.count(), 64);

    let mut single = ItemStack::try_new("minecraft:stone", 1, 64).unwrap();
    assert_eq!(single.add(u16::MAX), u16::MAX - 63);
    assert_eq!(single.count(), 64);

    let mut pearl = ItemStack::try_new("minecraft:ender_pearl", 1, 16).unwrap();
    assert_eq!(pearl.add(20), 5);
    assert_eq!(pearl.count(), 16);
}

#[test]
fn split_returns_err_on_zero_and_keeps_non_empty_stacks() {
    let original = ItemStack::try_new("minecraft:ender_pearl", 10, 16).unwrap();
    let returned = original.clone().split(0).unwrap_err();
    assert_eq!(returned, original);

    let (taken, rest) = original.clone().split(4).unwrap();
    assert_eq!(taken.item(), "minecraft:ender_pearl");
    assert_eq!(taken.count(), 4);
    assert_eq!(taken.max_stack(), 16);
    let rest = rest.expect("remainder should exist");
    assert_eq!(rest.item(), "minecraft:ender_pearl");
    assert_eq!(rest.count(), 6);
    assert_eq!(rest.max_stack(), 16);

    let (taken_exact, rest_exact) = original.clone().split(10).unwrap();
    assert_eq!(taken_exact.item(), "minecraft:ender_pearl");
    assert_eq!(taken_exact.count(), 10);
    assert_eq!(taken_exact.max_stack(), 16);
    assert_eq!(rest_exact, None);

    let (taken_more, rest_more) = original.split(50).unwrap();
    assert_eq!(taken_more.item(), "minecraft:ender_pearl");
    assert_eq!(taken_more.count(), 10);
    assert_eq!(taken_more.max_stack(), 16);
    assert_eq!(rest_more, None);
}

#[test]
fn empty_components_equal_and_default_on_new_stacks() {
    let a = ItemComponents::new();
    let b = ItemComponents::default();
    assert_eq!(a, b);
    assert!(a.is_empty());
    assert_eq!(a.damage(), None);
    assert_eq!(a.custom_name(), None);
    assert_eq!(a.custom_name_component(), None);
    assert_eq!(a.item_name(), None);
    assert_eq!(a.item_name_component(), None);
    assert!(a.enchantments().is_empty());
    assert!(a.stored_enchantments().is_empty());
    assert_eq!(a.repair_cost(), None);
    assert_eq!(a.potion(), None);
    assert_eq!(a.trim(), None);
    assert!(a.suspicious_stew_effects().is_empty());
    assert_eq!(a.instrument(), None);
    assert_eq!(a.ominous_bottle_amplifier(), None);

    let stack = ItemStack::try_new("minecraft:stone", 1, 64).unwrap();
    assert_eq!(stack.components(), &a);
    assert!(stack.components().is_empty());
}

#[test]
fn components_builders_validate_wiki_rules_and_reject_invalid_values() {
    assert_eq!(
        ItemComponents::new().with_enchantment("minecraft:sharpness", 0),
        Err(ItemComponentsError::InvalidEnchantmentLevel(0))
    );
    assert_eq!(
        ItemComponents::new().with_stored_enchantment("minecraft:mending", 0),
        Err(ItemComponentsError::InvalidEnchantmentLevel(0))
    );
    let bad_map = BTreeMap::from([("minecraft:sharpness".to_owned(), 0u8)]);
    assert_eq!(
        ItemComponents::new().with_enchantments(bad_map.clone()),
        Err(ItemComponentsError::InvalidEnchantmentLevel(0))
    );
    assert_eq!(
        ItemComponents::new().with_stored_enchantments(bad_map),
        Err(ItemComponentsError::InvalidEnchantmentLevel(0))
    );

    for invalid_id in ["", "sharpness", ":sharpness", "minecraft:", "Bad:id"] {
        assert_eq!(
            ItemComponents::new().with_enchantment(invalid_id, 1),
            Err(ItemComponentsError::InvalidEnchantmentId)
        );
        assert_eq!(
            ItemComponents::new().with_stored_enchantment(invalid_id, 1),
            Err(ItemComponentsError::InvalidEnchantmentId)
        );
        assert_eq!(
            ItemComponents::new().with_potion(invalid_id),
            Err(ItemComponentsError::InvalidPotionId)
        );
        assert_eq!(
            ArmorTrim::try_new(invalid_id, "minecraft:gold_ingot"),
            Err(ItemComponentsError::InvalidTrimPattern)
        );
        assert_eq!(
            ArmorTrim::try_new("minecraft:coast", invalid_id),
            Err(ItemComponentsError::InvalidTrimMaterial)
        );
        assert_eq!(
            SuspiciousStewEffect::try_new(invalid_id, 100),
            Err(ItemComponentsError::InvalidEffectId)
        );
        assert_eq!(
            ItemComponents::new().with_suspicious_stew_effect(invalid_id, 100),
            Err(ItemComponentsError::InvalidEffectId)
        );
        assert_eq!(
            ItemComponents::new().with_instrument(invalid_id),
            Err(ItemComponentsError::InvalidInstrumentId)
        );
    }

    assert_eq!(
        ItemComponents::new().with_ominous_bottle_amplifier(5),
        Err(ItemComponentsError::InvalidOminousBottleAmplifier(5))
    );
    assert_eq!(
        ItemComponents::new().set_ominous_bottle_amplifier(Some(255)),
        Err(ItemComponentsError::InvalidOminousBottleAmplifier(255))
    );

    let stew_default = SuspiciousStewEffect::with_default_duration("minecraft:poison").unwrap();
    assert_eq!(stew_default.id(), "minecraft:poison");
    assert_eq!(
        stew_default.duration_ticks(),
        DEFAULT_STEW_EFFECT_DURATION_TICKS
    );
    assert_eq!(stew_default.duration_ticks(), 160);

    let trim = ArmorTrim::try_new("minecraft:coast", "minecraft:gold_ingot").unwrap();
    assert_eq!(trim.pattern(), "minecraft:coast");
    assert_eq!(trim.material(), "minecraft:gold_ingot");

    let built = ItemComponents::new()
        .with_damage(12)
        .with_custom_name("Blade")
        .with_item_name(TextComponent::translatable("filled_map.buried_treasure"))
        .with_enchantment("minecraft:sharpness", 255)
        .unwrap()
        .with_stored_enchantment("minecraft:unbreaking", 1)
        .unwrap()
        .with_repair_cost(3)
        .with_potion("minecraft:healing")
        .unwrap()
        .with_trim(trim.clone())
        .with_suspicious_stew_effect("minecraft:night_vision", 140)
        .unwrap()
        .with_instrument("minecraft:ponder_goat_horn")
        .unwrap()
        .with_ominous_bottle_amplifier(4)
        .unwrap();

    assert!(!built.is_empty());
    assert_eq!(built.damage(), Some(12));
    assert_eq!(built.custom_name(), Some("Blade"));
    assert_eq!(
        built.custom_name_component(),
        Some(&TextComponent::Literal("Blade".to_owned()))
    );
    assert_eq!(built.item_name(), None);
    assert_eq!(
        built.item_name_component(),
        Some(&TextComponent::Translatable {
            key: "filled_map.buried_treasure".to_owned()
        })
    );
    assert_eq!(built.enchantments().get("minecraft:sharpness"), Some(&255));
    assert_eq!(
        built.stored_enchantments().get("minecraft:unbreaking"),
        Some(&1)
    );
    assert_eq!(built.repair_cost(), Some(3));
    assert_eq!(built.potion(), Some("minecraft:healing"));
    assert_eq!(built.trim(), Some(&trim));
    assert_eq!(
        built.suspicious_stew_effects(),
        &[SuspiciousStewEffect::try_new("minecraft:night_vision", 140).unwrap()]
    );
    assert_eq!(built.instrument(), Some("minecraft:ponder_goat_horn"));
    assert_eq!(built.ominous_bottle_amplifier(), Some(4));

    let literal_item_name = ItemComponents::new().with_item_name("Wayfinder");
    assert_eq!(literal_item_name.item_name(), Some("Wayfinder"));
    assert_eq!(
        literal_item_name.item_name_component(),
        Some(&TextComponent::Literal("Wayfinder".to_owned()))
    );

    let translatable_custom =
        ItemComponents::new().with_custom_name(TextComponent::translatable("item.example.custom"));
    assert_eq!(translatable_custom.custom_name(), None);
    assert_eq!(
        translatable_custom
            .custom_name_component()
            .and_then(TextComponent::translation_key),
        Some("item.example.custom")
    );

    for amp in MIN_OMINOUS_BOTTLE_AMPLIFIER..=MAX_OMINOUS_BOTTLE_AMPLIFIER {
        let ok = ItemComponents::new()
            .with_ominous_bottle_amplifier(amp)
            .expect("0..=4 is valid");
        assert_eq!(ok.ominous_bottle_amplifier(), Some(amp));
    }

    for err in [
        ItemComponentsError::InvalidEnchantmentId,
        ItemComponentsError::InvalidEnchantmentLevel(0),
        ItemComponentsError::InvalidPotionId,
        ItemComponentsError::InvalidTrimPattern,
        ItemComponentsError::InvalidTrimMaterial,
        ItemComponentsError::InvalidEffectId,
        ItemComponentsError::InvalidInstrumentId,
        ItemComponentsError::InvalidOminousBottleAmplifier(5),
    ] {
        let text = err.to_string();
        assert!(!text.is_empty());
        assert_eq!(text, text.to_lowercase());
        let wrapped = ItemStackError::from(err);
        assert_eq!(wrapped, ItemStackError::InvalidComponents(err));
        assert_eq!(wrapped.to_string(), text);
    }
}

#[test]
fn stacks_with_different_components_do_not_stack_and_split_and_add_keep_components() {
    let unnamed = ItemStack::try_new("minecraft:diamond", 4, 64).unwrap();
    let named_a1 = ItemStack::try_new("minecraft:diamond", 4, 64)
        .unwrap()
        .with_components(ItemComponents::new().with_custom_name("Alpha"))
        .unwrap();
    let mut named_a2 = ItemStack::try_new("minecraft:diamond", 10, 64)
        .unwrap()
        .with_components(ItemComponents::new().with_custom_name("Alpha"))
        .unwrap();
    let named_b = ItemStack::try_new("minecraft:diamond", 4, 64)
        .unwrap()
        .with_components(ItemComponents::new().with_custom_name("Beta"))
        .unwrap();

    assert!(!unnamed.can_stack_with(&named_a1));
    assert!(!named_a1.can_stack_with(&named_b));
    assert!(named_a1.can_stack_with(&named_a2));

    let other_enchant = ItemStack::try_new("minecraft:diamond", 4, 64)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_custom_name("Alpha")
                .with_enchantment("minecraft:unbreaking", 1)
                .unwrap(),
        )
        .unwrap();
    assert!(!named_a1.can_stack_with(&other_enchant));

    assert_eq!(named_a2.add(5), 0);
    assert_eq!(named_a2.count(), 15);
    assert_eq!(named_a2.components().custom_name(), Some("Alpha"));

    let rich = ItemStack::try_new("minecraft:stick", 8, 64)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_damage(3)
                .with_custom_name("Wand")
                .with_enchantment("minecraft:knockback", 2)
                .unwrap()
                .with_repair_cost(1)
                .with_trim(ArmorTrim::try_new("minecraft:coast", "minecraft:diamond").unwrap()),
        )
        .unwrap();
    let expected_components = rich.components().clone();
    let (taken, rest) = rich.split(3).unwrap();
    assert_eq!(taken.count(), 3);
    assert_eq!(taken.components(), &expected_components);
    let rest = rest.expect("remainder should exist");
    assert_eq!(rest.count(), 5);
    assert_eq!(rest.components(), &expected_components);
    assert!(taken.can_stack_with(&rest));

    // Different instruments, stew effects, item_names, or ominous bottle amplifiers do not stack.
    let horn_ponder = ItemStack::try_new("minecraft:goat_horn", 1, 1)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_instrument("minecraft:ponder_goat_horn")
                .unwrap(),
        )
        .unwrap();
    let horn_sing = ItemStack::try_new("minecraft:goat_horn", 1, 1)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_instrument("minecraft:sing_goat_horn")
                .unwrap(),
        )
        .unwrap();
    let horn_ponder_2 = ItemStack::try_new("minecraft:goat_horn", 1, 1)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_instrument("minecraft:ponder_goat_horn")
                .unwrap(),
        )
        .unwrap();
    assert!(!horn_ponder.can_stack_with(&horn_sing));
    assert!(horn_ponder.can_stack_with(&horn_ponder_2));

    let stew_nv = ItemStack::try_new("minecraft:suspicious_stew", 1, 1)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_suspicious_stew_effect("minecraft:night_vision", 160)
                .unwrap(),
        )
        .unwrap();
    let stew_nv_longer = ItemStack::try_new("minecraft:suspicious_stew", 1, 1)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_suspicious_stew_effect("minecraft:night_vision", 200)
                .unwrap(),
        )
        .unwrap();
    let stew_blind = ItemStack::try_new("minecraft:suspicious_stew", 1, 1)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_suspicious_stew_effect("minecraft:blindness", 160)
                .unwrap(),
        )
        .unwrap();
    assert!(!stew_nv.can_stack_with(&stew_nv_longer));
    assert!(!stew_nv.can_stack_with(&stew_blind));

    let bottle_0 = ItemStack::try_new("minecraft:ominous_bottle", 1, 64)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_ominous_bottle_amplifier(0)
                .unwrap(),
        )
        .unwrap();
    let bottle_1 = ItemStack::try_new("minecraft:ominous_bottle", 1, 64)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_ominous_bottle_amplifier(1)
                .unwrap(),
        )
        .unwrap();
    let bottle_1b = ItemStack::try_new("minecraft:ominous_bottle", 2, 64)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_ominous_bottle_amplifier(1)
                .unwrap(),
        )
        .unwrap();
    assert!(!bottle_0.can_stack_with(&bottle_1));
    assert!(bottle_1.can_stack_with(&bottle_1b));

    let item_named_lit = ItemStack::try_new("minecraft:map", 1, 64)
        .unwrap()
        .with_components(ItemComponents::new().with_item_name("Treasure Map"))
        .unwrap();
    let item_named_tr = ItemStack::try_new("minecraft:map", 1, 64)
        .unwrap()
        .with_components(
            ItemComponents::new()
                .with_item_name(TextComponent::translatable("filled_map.buried_treasure")),
        )
        .unwrap();
    assert!(!item_named_lit.can_stack_with(&item_named_tr));
}

#[test]
fn item_id_helpers_normalize_compare_and_default_max_stack() {
    assert_eq!(normalize_item_id("minecraft:stone"), "stone");
    assert_eq!(normalize_item_id("stone"), "stone");
    assert_eq!(normalize_item_id("custom:widget"), "custom:widget");

    assert!(items_equal("minecraft:oak_planks", "oak_planks"));
    assert!(items_equal("oak_planks", "oak_planks"));
    assert!(!items_equal("custom:oak_planks", "oak_planks"));

    for single in [
        "minecraft:lava_bucket",
        "water_bucket",
        "minecraft:milk_bucket",
        "minecraft:potion",
        "splash_potion",
        "minecraft:lingering_potion",
        "minecraft:enchanted_book",
    ] {
        assert_eq!(default_max_stack(single), 1, "{single}");
    }
    for sixteen in ["bucket", "minecraft:snowball", "ender_pearl", "egg"] {
        assert_eq!(default_max_stack(sixteen), 16, "{sixteen}");
    }
    assert_eq!(default_max_stack("minecraft:stone"), DEFAULT_MAX_STACK);
}
