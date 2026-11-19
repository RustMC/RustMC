//! Combined loader and summary for all operator-provisioned vanilla 26.3 rule data.
//!
//! Loads item, block, and enchantment tags, default item component reports,
//! recipes (resolved against [`ItemTags`] and [`ItemDefaults`]), loot tables across
//! all supported categories, enchantment definitions, and static protocol IDs in
//! dependency order from one data root.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::path::Path;

use crate::item::{ItemDefaults, ItemDefaultsError};
use crate::protocol::ids::{ProtocolIds, ProtocolIdsError};
use crate::vanilla::feature::{BlockTags, EnchantmentTags, ItemTags};
use crate::vanilla::worldgen::WorldgenError;
use crate::world::enchantment::{EnchantmentError, EnchantmentRegistry};
use crate::world::loot::{LootError, LootTables};
use crate::world::recipe_data::{CookingKind, RecipeData, RecipeDataError};

const LOOT_CATEGORIES: [&str; 5] = ["blocks", "entities", "chests", "gameplay", "archaeology"];

/// Error returned when loading any part of the vanilla rule bundle fails.
#[derive(Debug)]
pub enum VanillaRulesError {
    /// Loading item tags from `data/<ns>/tags/item` failed.
    ItemTags(WorldgenError),
    /// Loading block tags from `data/<ns>/tags/block` failed.
    BlockTags(WorldgenError),
    /// Loading enchantment tags from `data/<ns>/tags/enchantment` failed.
    EnchantmentTags(WorldgenError),
    /// Loading default item components from `reports/minecraft/components/item` failed.
    ItemDefaults(ItemDefaultsError),
    /// Loading recipe definitions from `data/<ns>/recipe` failed.
    RecipeData(RecipeDataError),
    /// Loading loot tables from `data/<ns>/loot_table/<category>` failed.
    LootTables(LootError),
    /// Loading enchantment definitions from `data/<ns>/enchantment` failed.
    EnchantmentRegistry(EnchantmentError),
    /// Loading static protocol IDs from `reports/registries.json` failed.
    ProtocolIds(ProtocolIdsError),
}

impl fmt::Display for VanillaRulesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ItemTags(source) => write!(f, "failed to load ItemTags: {source}"),
            Self::BlockTags(source) => write!(f, "failed to load BlockTags: {source}"),
            Self::EnchantmentTags(source) => {
                write!(f, "failed to load EnchantmentTags: {source}")
            }
            Self::ItemDefaults(source) => write!(f, "failed to load ItemDefaults: {source}"),
            Self::RecipeData(source) => write!(f, "failed to load RecipeData: {source}"),
            Self::LootTables(source) => write!(f, "failed to load LootTables: {source}"),
            Self::EnchantmentRegistry(source) => {
                write!(f, "failed to load EnchantmentRegistry: {source}")
            }
            Self::ProtocolIds(source) => write!(f, "failed to load ProtocolIds: {source}"),
        }
    }
}

impl Error for VanillaRulesError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ItemTags(source) | Self::BlockTags(source) | Self::EnchantmentTags(source) => {
                Some(source)
            }
            Self::ItemDefaults(source) => Some(source),
            Self::RecipeData(source) => Some(source),
            Self::LootTables(source) => Some(source),
            Self::EnchantmentRegistry(source) => Some(source),
            Self::ProtocolIds(source) => Some(source),
        }
    }
}

/// Counts of loaded tags per tag registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TagSummary {
    /// Number of item tags (`data/<ns>/tags/item`).
    pub item: usize,
    /// Number of block tags (`data/<ns>/tags/block`).
    pub block: usize,
    /// Number of enchantment tags (`data/<ns>/tags/enchantment`).
    pub enchantment: usize,
}

/// Counts of items with default components exposed by [`ItemDefaults`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ItemComponentSummary {
    /// Items carrying `minecraft:food`.
    pub foods: usize,
    /// Items carrying `minecraft:tool`.
    pub tools: usize,
    /// Items carrying `minecraft:max_damage`.
    pub damageable_items: usize,
}

/// Counts of loaded cooking recipes per furnace-family kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CookingSummary {
    /// Loaded `minecraft:smelting` recipes.
    pub smelting: usize,
    /// Loaded `minecraft:blasting` recipes.
    pub blasting: usize,
    /// Loaded `minecraft:smoking` recipes.
    pub smoking: usize,
    /// Loaded `minecraft:campfire_cooking` recipes.
    pub campfire_cooking: usize,
}

/// Counts of loaded and skipped recipes per kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecipeSummary {
    /// Cooking recipe counts per kind.
    pub cooking: CookingSummary,
    /// Loaded `minecraft:crafting_shaped` recipes.
    pub crafting_shaped: usize,
    /// Loaded `minecraft:crafting_shapeless` recipes.
    pub crafting_shapeless: usize,
    /// Loaded `minecraft:crafting_transmute` recipes.
    pub crafting_transmute: usize,
    /// Loaded `minecraft:crafting_special_repairitem` recipes.
    pub crafting_special_repairitem: usize,
    /// Loaded `minecraft:stonecutting` recipes.
    pub stonecutting: usize,
    /// Loaded `minecraft:smithing_transform` recipes.
    pub smithing_transform: usize,
    /// Loaded `minecraft:smithing_trim` recipes.
    pub smithing_trim: usize,
    /// Loaded `minecraft:brewing` recipes.
    pub brewing: usize,
    /// Recipe files skipped due to unsupported type or component-bearing results.
    pub skipped: usize,
}

/// Counts of loaded loot tables per category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LootCategorySummary {
    /// Loaded block loot tables (`data/<ns>/loot_table/blocks`).
    pub blocks: usize,
    /// Loaded entity loot tables (`data/<ns>/loot_table/entities`).
    pub entities: usize,
    /// Loaded chest loot tables (`data/<ns>/loot_table/chests`).
    pub chests: usize,
    /// Loaded gameplay loot tables (`data/<ns>/loot_table/gameplay`).
    pub gameplay: usize,
    /// Loaded archaeology loot tables (`data/<ns>/loot_table/archaeology`).
    pub archaeology: usize,
}

/// Summary of unsupported loot condition and modifier nodes across loaded loot tables.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnsupportedLootSummary {
    /// Total number of unsupported condition and modifier nodes.
    pub count: usize,
    /// Sorted distinct unsupported node identifiers (`what` labels).
    pub kinds: Vec<String>,
}

/// Counts of static Java Edition protocol IDs loaded from `reports/registries.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProtocolIdSummary {
    /// Loaded `minecraft:item` entries.
    pub items: usize,
    /// Loaded `minecraft:data_component_type` entries.
    pub component_types: usize,
    /// Loaded `minecraft:potion` entries.
    pub potions: usize,
    /// Loaded `minecraft:mob_effect` entries.
    pub mob_effects: usize,
}

/// Summary counts across every dataset in [`VanillaRules`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VanillaRulesSummary {
    /// Tag counts per registry.
    pub tags: TagSummary,
    /// Number of default item component entries (`reports/minecraft/components/item`).
    pub items: usize,
    /// Counts of exposed default item components across loaded items.
    pub item_components: ItemComponentSummary,
    /// Recipe counts per kind and skipped count.
    pub recipes: RecipeSummary,
    /// Total number of loaded loot tables across all supported categories.
    pub loot_tables: usize,
    /// Loaded loot tables per category.
    pub loot_categories: LootCategorySummary,
    /// Unsupported loot condition and modifier nodes across loaded tables.
    pub unsupported_loot_nodes: UnsupportedLootSummary,
    /// Number of loaded enchantment definitions.
    pub enchantments: usize,
    /// Static Java Edition protocol ID counts per registry.
    pub protocol_ids: ProtocolIdSummary,
}

/// Bundle of all vanilla 26.3 rule registries loaded from one operator data root.
#[derive(Debug, Clone, Default)]
pub struct VanillaRules {
    /// Item tag definitions.
    pub item_tags: ItemTags,
    /// Block tag definitions.
    pub block_tags: BlockTags,
    /// Enchantment tag definitions.
    pub enchantment_tags: EnchantmentTags,
    /// Default per-item components from the item report.
    pub item_defaults: ItemDefaults,
    /// Crafting, cooking, stonecutting, smithing, and brewing recipes.
    pub recipes: RecipeData,
    /// Loot tables across all supported categories and resolved predicates.
    pub loot_tables: LootTables,
    /// Enchantment definitions.
    pub enchantments: EnchantmentRegistry,
    /// Static Java Edition protocol IDs from `reports/registries.json`.
    pub protocol_ids: ProtocolIds,
}

impl VanillaRules {
    /// Loads every vanilla rule dataset under `root` in dependency order.
    pub fn load(root: &Path) -> Result<Self, VanillaRulesError> {
        let item_tags = ItemTags::load(root).map_err(VanillaRulesError::ItemTags)?;
        let block_tags = BlockTags::load(root).map_err(VanillaRulesError::BlockTags)?;
        let enchantment_tags =
            EnchantmentTags::load(root).map_err(VanillaRulesError::EnchantmentTags)?;
        let item_defaults = ItemDefaults::load(root).map_err(VanillaRulesError::ItemDefaults)?;
        let recipes = RecipeData::load_with_items(root, &item_tags, &item_defaults)
            .map_err(VanillaRulesError::RecipeData)?;
        let enchantments = EnchantmentRegistry::load(root, &item_tags, &enchantment_tags)
            .map_err(VanillaRulesError::EnchantmentRegistry)?;
        let loot_tables = LootTables::load_categories(root, &LOOT_CATEGORIES)
            .map_err(VanillaRulesError::LootTables)?;
        let protocol_ids = ProtocolIds::load(root).map_err(VanillaRulesError::ProtocolIds)?;

        Ok(Self {
            item_tags,
            block_tags,
            enchantment_tags,
            item_defaults,
            recipes,
            loot_tables,
            enchantments,
            protocol_ids,
        })
    }

    /// Returns a summary of counts across all loaded rule registries.
    #[must_use]
    pub fn summary(&self) -> VanillaRulesSummary {
        let mut loot_categories = LootCategorySummary::default();
        for (id, _) in self.loot_tables.iter() {
            if let Some(category) = id
                .split_once(':')
                .and_then(|(_, rest)| rest.split_once('/'))
                .map(|(cat, _)| cat)
            {
                match category {
                    "blocks" => loot_categories.blocks += 1,
                    "entities" => loot_categories.entities += 1,
                    "chests" => loot_categories.chests += 1,
                    "gameplay" => loot_categories.gameplay += 1,
                    "archaeology" => loot_categories.archaeology += 1,
                    _ => {}
                }
            }
        }

        let unsupported = self.loot_tables.unsupported();
        let kinds: BTreeSet<String> = unsupported
            .iter()
            .map(|(_, what)| (*what).to_owned())
            .collect();

        VanillaRulesSummary {
            tags: TagSummary {
                item: self.item_tags.len(),
                block: self.block_tags.len(),
                enchantment: self.enchantment_tags.len(),
            },
            items: self.item_defaults.len(),
            item_components: ItemComponentSummary {
                foods: self.item_defaults.food_count(),
                tools: self.item_defaults.tool_count(),
                damageable_items: self.item_defaults.damageable_items().count(),
            },
            recipes: RecipeSummary {
                cooking: CookingSummary {
                    smelting: self.recipes.cooking(CookingKind::Smelting).len(),
                    blasting: self.recipes.cooking(CookingKind::Blasting).len(),
                    smoking: self.recipes.cooking(CookingKind::Smoking).len(),
                    campfire_cooking: self.recipes.cooking(CookingKind::CampfireCooking).len(),
                },
                crafting_shaped: self.recipes.shaped_recipes().count(),
                crafting_shapeless: self.recipes.shapeless_recipes().count(),
                crafting_transmute: self.recipes.transmute().len(),
                crafting_special_repairitem: self.recipes.repair_item().len(),
                stonecutting: self.recipes.stonecutting().len(),
                smithing_transform: self.recipes.smithing_transform().len(),
                smithing_trim: self.recipes.smithing_trim().len(),
                brewing: self.recipes.brewing().len(),
                skipped: self.recipes.skipped().len(),
            },
            loot_tables: self.loot_tables.len(),
            loot_categories,
            unsupported_loot_nodes: UnsupportedLootSummary {
                count: unsupported.len(),
                kinds: kinds.into_iter().collect(),
            },
            enchantments: self.enchantments.len(),
            protocol_ids: ProtocolIdSummary {
                items: self.protocol_ids.item_count(),
                component_types: self.protocol_ids.component_type_count(),
                potions: self.protocol_ids.potion_count(),
                mob_effects: self.protocol_ids.mob_effect_count(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(label: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rustmc-vanilla-rules-{label}-{}-{n}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create scratch dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().expect("parent dir")).expect("create parent dir");
            fs::write(path, contents).expect("write test fixture");
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_minimal_valid_root(root: &ScratchDir) {
        root.write(
            "data/testns/tags/item/widgets.json",
            r#"{"values": ["minecraft:widget"]}"#,
        );
        root.write(
            "data/testns/tags/block/stones.json",
            r#"{"values": ["testns:stone"]}"#,
        );
        root.write(
            "data/testns/tags/enchantment/combat.json",
            r#"{"values": ["testns:keen"]}"#,
        );
        root.write(
            "reports/minecraft/components/item/widget.json",
            r#"{"components": {"minecraft:max_stack_size": 16, "minecraft:provides_trim_material": "testns:widget_material"}}"#,
        );
        root.write(
            "data/testns/recipe/widget_craft.json",
            r##"{
                "type": "minecraft:crafting_shaped",
                "pattern": ["#"],
                "key": {"#": "#testns:widgets"},
                "result": {"id": "minecraft:widget", "count": 2}
            }"##,
        );
        root.write(
            "data/testns/loot_table/blocks/stone.json",
            r#"{
                "type": "minecraft:block",
                "pools": [{
                    "rolls": 1,
                    "entries": [{"type": "minecraft:item", "name": "minecraft:widget"}]
                }]
            }"#,
        );
        root.write(
            "data/testns/enchantment/keen.json",
            r##"{
                "max_level": 3,
                "anvil_cost": 2,
                "weight": 5,
                "min_cost": {"base": 1, "per_level_above_first": 5},
                "max_cost": {"base": 10, "per_level_above_first": 5},
                "slots": ["mainhand"],
                "supported_items": "#testns:widgets",
                "exclusive_set": "#testns:combat"
            }"##,
        );
        root.write(
            "reports/registries.json",
            r#"{
                "minecraft:item": {
                    "entries": {
                        "minecraft:widget": {"protocol_id": 0}
                    }
                },
                "minecraft:data_component_type": {
                    "entries": {
                        "minecraft:max_stack_size": {"protocol_id": 0}
                    }
                },
                "minecraft:potion": {
                    "entries": {
                        "minecraft:water": {"protocol_id": 0}
                    }
                },
                "minecraft:mob_effect": {
                    "entries": {
                        "minecraft:speed": {"protocol_id": 0}
                    }
                }
            }"#,
        );
    }

    #[test]
    fn loads_synthetic_root_with_one_file_per_kind_and_summarizes_counts() {
        let root = ScratchDir::new("synthetic-one-each");
        write_minimal_valid_root(&root);

        let rules = VanillaRules::load(root.path()).expect("synthetic vanilla rules load");
        assert_eq!(
            rules.summary(),
            VanillaRulesSummary {
                tags: TagSummary {
                    item: 1,
                    block: 1,
                    enchantment: 1,
                },
                items: 1,
                item_components: ItemComponentSummary {
                    foods: 0,
                    tools: 0,
                    damageable_items: 0,
                },
                recipes: RecipeSummary {
                    cooking: CookingSummary {
                        smelting: 0,
                        blasting: 0,
                        smoking: 0,
                        campfire_cooking: 0,
                    },
                    crafting_shaped: 1,
                    crafting_shapeless: 0,
                    crafting_transmute: 0,
                    crafting_special_repairitem: 0,
                    stonecutting: 0,
                    smithing_transform: 0,
                    smithing_trim: 0,
                    brewing: 0,
                    skipped: 0,
                },
                loot_tables: 1,
                loot_categories: LootCategorySummary {
                    blocks: 1,
                    entities: 0,
                    chests: 0,
                    gameplay: 0,
                    archaeology: 0,
                },
                unsupported_loot_nodes: UnsupportedLootSummary {
                    count: 0,
                    kinds: Vec::new(),
                },
                enchantments: 1,
                protocol_ids: ProtocolIdSummary {
                    items: 1,
                    component_types: 1,
                    potions: 1,
                    mob_effects: 1,
                },
            }
        );

        let crafted = rules
            .recipes
            .get_crafting_by_id("testns:widget_craft")
            .expect("crafted recipe loaded");
        assert_eq!(crafted.output().max_stack(), 16);
        assert!(rules.loot_tables.get("testns:stone").is_some());
        assert!(
            rules
                .enchantments
                .can_apply("testns:keen", "minecraft:widget")
        );
        assert_eq!(rules.protocol_ids.item_id("minecraft:widget"), Some(0));
    }

    #[test]
    fn summary_counts_all_recipe_kinds_and_skipped() {
        let root = ScratchDir::new("all-recipe-kinds");
        write_minimal_valid_root(&root);
        root.write(
            "data/testns/recipe/smelt.json",
            r#"{"type": "minecraft:smelting", "ingredient": "minecraft:widget", "result": {"id": "minecraft:widget"}, "cookingtime": 200}"#,
        );
        root.write(
            "data/testns/recipe/blast.json",
            r#"{"type": "minecraft:blasting", "ingredient": "minecraft:widget", "result": {"id": "minecraft:widget"}, "cookingtime": 100}"#,
        );
        root.write(
            "data/testns/recipe/smoke.json",
            r#"{"type": "minecraft:smoking", "ingredient": "minecraft:widget", "result": {"id": "minecraft:widget"}, "cookingtime": 100}"#,
        );
        root.write(
            "data/testns/recipe/camp.json",
            r#"{"type": "minecraft:campfire_cooking", "ingredient": "minecraft:widget", "result": {"id": "minecraft:widget"}, "cookingtime": 600}"#,
        );
        root.write(
            "data/testns/recipe/shapeless.json",
            r#"{"type": "minecraft:crafting_shapeless", "ingredients": ["minecraft:widget"], "result": {"id": "minecraft:widget"}}"#,
        );
        root.write(
            "data/testns/recipe/transmute.json",
            r#"{"type": "minecraft:crafting_transmute", "input": "minecraft:widget", "material": "minecraft:widget", "result": {"id": "minecraft:widget"}}"#,
        );
        root.write(
            "data/testns/recipe/repair.json",
            r#"{"type": "minecraft:crafting_special_repairitem"}"#,
        );
        root.write(
            "data/testns/recipe/cut.json",
            r#"{"type": "minecraft:stonecutting", "ingredient": "minecraft:widget", "result": {"id": "minecraft:widget"}}"#,
        );
        root.write(
            "data/testns/recipe/transform.json",
            r#"{"type": "minecraft:smithing_transform", "base": "minecraft:widget", "result": {"id": "minecraft:widget"}}"#,
        );
        root.write(
            "data/testns/recipe/trim.json",
            r#"{"type": "minecraft:smithing_trim", "base": "minecraft:widget", "addition": "minecraft:widget", "pattern": "testns:p"}"#,
        );
        root.write(
            "data/testns/recipe/brew.json",
            r#"{
                "type": "minecraft:brewing",
                "input": {"item": "minecraft:widget", "potion_contents": {"potions": "testns:base"}},
                "reagent": {"item": "minecraft:widget"},
                "output": {"id": "minecraft:widget", "components": {"minecraft:potion_contents": {"potion": "testns:out"}}}
            }"#,
        );
        root.write(
            "data/testns/recipe/skipped_special.json",
            r#"{"type": "minecraft:crafting_decorated_pot"}"#,
        );

        let rules = VanillaRules::load(root.path()).expect("all recipe kinds load");
        assert_eq!(
            rules.summary().recipes,
            RecipeSummary {
                cooking: CookingSummary {
                    smelting: 1,
                    blasting: 1,
                    smoking: 1,
                    campfire_cooking: 1,
                },
                crafting_shaped: 1,
                crafting_shapeless: 1,
                crafting_transmute: 1,
                crafting_special_repairitem: 1,
                stonecutting: 1,
                smithing_transform: 1,
                smithing_trim: 1,
                brewing: 1,
                skipped: 1,
            }
        );
    }

    #[test]
    fn summary_counts_loot_categories_unsupported_nodes_item_components_and_protocol_ids() {
        let root = ScratchDir::new("extended-summary");
        write_minimal_valid_root(&root);
        root.write(
            "reports/minecraft/components/item/apple_snack.json",
            r#"{"components": {"minecraft:max_stack_size": 64, "minecraft:food": {"nutrition": 4, "saturation": 2.4}}}"#,
        );
        root.write(
            "reports/minecraft/components/item/pick.json",
            r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:max_damage": 250, "minecraft:tool": {"rules": []}}}"#,
        );
        root.write(
            "reports/minecraft/components/item/shears_tool.json",
            r#"{"components": {"minecraft:max_stack_size": 1, "minecraft:max_damage": 120}}"#,
        );
        root.write(
            "data/testns/loot_table/entities/slime.json",
            r#"{"type": "minecraft:entity", "pools": [{"rolls": 1, "entries": [{"type": "minecraft:item", "name": "minecraft:widget"}]}]}"#,
        );
        root.write(
            "data/testns/loot_table/chests/vault.json",
            r#"{
                "type": "minecraft:chest",
                "pools": [{
                    "rolls": 1,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "minecraft:widget",
                        "modifier": [
                            {"type": "minecraft:exploration_map"},
                            {"type": "testns:custom_fn"},
                            {"type": "minecraft:exploration_map"}
                        ]
                    }]
                }]
            }"#,
        );
        root.write(
            "data/testns/loot_table/gameplay/fishing.json",
            r#"{"type": "minecraft:fishing", "pools": [{"rolls": 1, "condition": [{"type": "testns:custom_cond"}], "entries": [{"type": "minecraft:empty"}]}]}"#,
        );
        root.write(
            "data/testns/loot_table/archaeology/ruins.json",
            r#"{"type": "minecraft:archaeology", "pools": [{"rolls": 1, "entries": [{"type": "minecraft:item", "name": "minecraft:widget"}]}]}"#,
        );
        root.write(
            "reports/registries.json",
            r#"{
                "minecraft:item": {
                    "entries": {
                        "minecraft:widget": {"protocol_id": 0},
                        "minecraft:apple_snack": {"protocol_id": 1},
                        "minecraft:pick": {"protocol_id": 2}
                    }
                },
                "minecraft:data_component_type": {
                    "entries": {
                        "minecraft:max_stack_size": {"protocol_id": 0},
                        "minecraft:food": {"protocol_id": 1}
                    }
                },
                "minecraft:potion": {
                    "entries": {
                        "minecraft:water": {"protocol_id": 0},
                        "minecraft:awkward": {"protocol_id": 1},
                        "minecraft:swiftness": {"protocol_id": 2},
                        "minecraft:healing": {"protocol_id": 3}
                    }
                },
                "minecraft:mob_effect": {
                    "entries": {
                        "minecraft:speed": {"protocol_id": 0},
                        "minecraft:haste": {"protocol_id": 1}
                    }
                }
            }"#,
        );

        let rules = VanillaRules::load(root.path()).expect("extended summary loads");
        let summary = rules.summary();
        assert_eq!(summary.items, 4);
        assert_eq!(
            summary.item_components,
            ItemComponentSummary {
                foods: 1,
                tools: 1,
                damageable_items: 2,
            }
        );
        assert_eq!(summary.loot_tables, 5);
        assert_eq!(
            summary.loot_categories,
            LootCategorySummary {
                blocks: 1,
                entities: 1,
                chests: 1,
                gameplay: 1,
                archaeology: 1,
            }
        );
        assert_eq!(
            summary.unsupported_loot_nodes,
            UnsupportedLootSummary {
                count: 4,
                kinds: vec![
                    "minecraft:exploration_map".to_owned(),
                    "testns:custom_cond".to_owned(),
                    "testns:custom_fn".to_owned(),
                ],
            }
        );
        assert_eq!(
            summary.protocol_ids,
            ProtocolIdSummary {
                items: 3,
                component_types: 2,
                potions: 4,
                mob_effects: 2,
            }
        );
    }

    #[test]
    fn broken_recipe_reports_recipe_data_and_source() {
        let root = ScratchDir::new("broken-recipe");
        write_minimal_valid_root(&root);
        root.write("data/testns/recipe/broken.json", r#"{"type": 42}"#);

        let err = VanillaRules::load(root.path()).expect_err("broken recipe must fail");
        assert!(
            matches!(err, VanillaRulesError::RecipeData(_)),
            "expected RecipeData error variant, got {err:?}"
        );
        assert!(
            err.to_string().contains("RecipeData"),
            "error message must name RecipeData: {err}"
        );
        let source = err.source().expect("error must expose source()");
        assert!(
            source.to_string().contains("broken.json"),
            "source error must name broken file: {source}"
        );
    }

    #[test]
    fn each_failing_part_identifies_its_variant_and_source() {
        let missing = ScratchDir::new("missing-reports");
        let err = VanillaRules::load(missing.path()).expect_err("missing reports must fail");
        assert!(matches!(err, VanillaRulesError::ItemDefaults(_)));
        assert!(err.to_string().contains("ItemDefaults"));
        assert!(err.source().is_some());

        let bad_item_tag = ScratchDir::new("bad-item-tag");
        write_minimal_valid_root(&bad_item_tag);
        bad_item_tag.write("data/testns/tags/item/bad.json", "{not json");
        let err = VanillaRules::load(bad_item_tag.path()).expect_err("bad item tag must fail");
        assert!(matches!(err, VanillaRulesError::ItemTags(_)));
        assert!(err.to_string().contains("ItemTags"));
        assert!(err.source().is_some());

        let bad_block_tag = ScratchDir::new("bad-block-tag");
        write_minimal_valid_root(&bad_block_tag);
        bad_block_tag.write("data/testns/tags/block/bad.json", "{not json");
        let err = VanillaRules::load(bad_block_tag.path()).expect_err("bad block tag must fail");
        assert!(matches!(err, VanillaRulesError::BlockTags(_)));
        assert!(err.to_string().contains("BlockTags"));
        assert!(err.source().is_some());

        let bad_ench_tag = ScratchDir::new("bad-ench-tag");
        write_minimal_valid_root(&bad_ench_tag);
        bad_ench_tag.write("data/testns/tags/enchantment/bad.json", "{not json");
        let err =
            VanillaRules::load(bad_ench_tag.path()).expect_err("bad enchantment tag must fail");
        assert!(matches!(err, VanillaRulesError::EnchantmentTags(_)));
        assert!(err.to_string().contains("EnchantmentTags"));
        assert!(err.source().is_some());

        let bad_loot = ScratchDir::new("bad-loot");
        write_minimal_valid_root(&bad_loot);
        bad_loot.write("data/testns/loot_table/blocks/bad.json", "{not json");
        let err = VanillaRules::load(bad_loot.path()).expect_err("bad loot table must fail");
        assert!(matches!(err, VanillaRulesError::LootTables(_)));
        assert!(err.to_string().contains("LootTables"));
        assert!(err.source().is_some());

        let bad_ench = ScratchDir::new("bad-ench");
        write_minimal_valid_root(&bad_ench);
        bad_ench.write("data/testns/enchantment/bad.json", "{not json");
        let err = VanillaRules::load(bad_ench.path()).expect_err("bad enchantment must fail");
        assert!(matches!(err, VanillaRulesError::EnchantmentRegistry(_)));
        assert!(err.to_string().contains("EnchantmentRegistry"));
        assert!(err.source().is_some());

        let bad_ids = ScratchDir::new("bad-registries");
        write_minimal_valid_root(&bad_ids);
        bad_ids.write("reports/registries.json", "{not json");
        let err = VanillaRules::load(bad_ids.path()).expect_err("bad registries must fail");
        assert!(matches!(err, VanillaRulesError::ProtocolIds(_)));
        assert!(err.to_string().contains("ProtocolIds"));
        assert!(err.source().is_some());
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_vanilla_rules_pin_26_3_summary`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_vanilla_rules_pin_26_3_summary() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let rules = VanillaRules::load(&root).expect("load all 26.3 vanilla rules");
        assert_eq!(
            rules.summary(),
            VanillaRulesSummary {
                tags: TagSummary {
                    item: 236,
                    block: 300,
                    enchantment: 22,
                },
                items: 1658,
                item_components: ItemComponentSummary {
                    foods: 44,
                    tools: 38,
                    damageable_items: 84,
                },
                recipes: RecipeSummary {
                    cooking: CookingSummary {
                        smelting: 73,
                        blasting: 25,
                        smoking: 9,
                        campfire_cooking: 9,
                    },
                    crafting_shaped: 827,
                    crafting_shapeless: 358,
                    crafting_transmute: 33,
                    crafting_special_repairitem: 1,
                    stonecutting: 351,
                    smithing_transform: 12,
                    smithing_trim: 18,
                    brewing: 279,
                    // 48 previously skipped minus 1 loaded `minecraft:crafting_special_repairitem` file = 47.
                    skipped: 47,
                },
                loot_tables: 1201 + 109 + 58 + 26 + 6,
                loot_categories: LootCategorySummary {
                    blocks: 1201,
                    entities: 109,
                    chests: 58,
                    gameplay: 26,
                    archaeology: 6,
                },
                unsupported_loot_nodes: UnsupportedLootSummary {
                    count: 36,
                    kinds: vec![
                        "minecraft:exploration_map".to_owned(),
                        "minecraft:filtered".to_owned(),
                    ],
                },
                enchantments: 43,
                protocol_ids: ProtocolIdSummary {
                    items: 1658,
                    component_types: 122,
                    potions: 46,
                    mob_effects: 40,
                },
            }
        );
    }
}
