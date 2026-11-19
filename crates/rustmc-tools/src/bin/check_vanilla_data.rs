//! Check an operator-provisioned Java 26.3 vanilla rule data root.
//!
//! Usage: `check_vanilla_data [DATA_ROOT]` (or set `RUSTMC_VANILLA_DATA`).
//! Loads the combined rule bundle and prints its summary as one JSON object.

use std::error::Error;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use rustmc_server::world::vanilla_rules::{VanillaRules, VanillaRulesSummary};
use serde_json::{Value, json};

const USAGE: &str = "usage: check_vanilla_data [DATA_ROOT] (or set RUSTMC_VANILLA_DATA)";

fn resolve_data_root(
    args: &[OsString],
    env_root: Option<OsString>,
) -> Result<PathBuf, &'static str> {
    match args {
        [arg] => Ok(PathBuf::from(arg)),
        [] => env_root
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or(USAGE),
        _ => Err(USAGE),
    }
}

fn summary_json(summary: &VanillaRulesSummary) -> Value {
    json!({
        "tags": {
            "item": summary.tags.item,
            "block": summary.tags.block,
            "enchantment": summary.tags.enchantment,
        },
        "items": summary.items,
        "item_components": {
            "foods": summary.item_components.foods,
            "tools": summary.item_components.tools,
            "damageable_items": summary.item_components.damageable_items,
        },
        "recipes": {
            "cooking": {
                "smelting": summary.recipes.cooking.smelting,
                "blasting": summary.recipes.cooking.blasting,
                "smoking": summary.recipes.cooking.smoking,
                "campfire_cooking": summary.recipes.cooking.campfire_cooking,
            },
            "crafting_shaped": summary.recipes.crafting_shaped,
            "crafting_shapeless": summary.recipes.crafting_shapeless,
            "crafting_transmute": summary.recipes.crafting_transmute,
            "crafting_special_repairitem": summary.recipes.crafting_special_repairitem,
            "stonecutting": summary.recipes.stonecutting,
            "smithing_transform": summary.recipes.smithing_transform,
            "smithing_trim": summary.recipes.smithing_trim,
            "brewing": summary.recipes.brewing,
            "skipped": summary.recipes.skipped,
        },
        "loot_tables": summary.loot_tables,
        "loot_categories": {
            "blocks": summary.loot_categories.blocks,
            "entities": summary.loot_categories.entities,
            "chests": summary.loot_categories.chests,
            "gameplay": summary.loot_categories.gameplay,
            "archaeology": summary.loot_categories.archaeology,
        },
        "unsupported_loot_nodes": {
            "count": summary.unsupported_loot_nodes.count,
            "kinds": summary.unsupported_loot_nodes.kinds,
        },
        "enchantments": summary.enchantments,
        "protocol_ids": {
            "items": summary.protocol_ids.items,
            "component_types": summary.protocol_ids.component_types,
            "potions": summary.protocol_ids.potions,
            "mob_effects": summary.protocol_ids.mob_effects,
        },
    })
}

fn print_error_chain(error: &dyn Error) {
    eprintln!("error: {error}");
    let mut source = error.source();
    while let Some(cause) = source {
        eprintln!("caused by: {cause}");
        source = cause.source();
    }
}

fn run(root: &Path) -> Result<String, Box<dyn Error>> {
    let rules = VanillaRules::load(root)?;
    let encoded = serde_json::to_string_pretty(&summary_json(&rules.summary()))?;
    Ok(encoded)
}

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let root = match resolve_data_root(&args, std::env::var_os("RUSTMC_VANILLA_DATA")) {
        Ok(root) => root,
        Err(usage) => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    };
    match run(&root) {
        Ok(output) => println!("{output}"),
        Err(error) => {
            print_error_chain(error.as_ref());
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustmc_server::world::vanilla_rules::{
        CookingSummary, ItemComponentSummary, LootCategorySummary, ProtocolIdSummary,
        RecipeSummary, TagSummary, UnsupportedLootSummary,
    };

    #[test]
    fn resolve_data_root_prefers_arg_then_env_and_rejects_extra_args() {
        let arg = [OsString::from("cli-root")];
        let env = Some(OsString::from("env-root"));
        assert_eq!(
            resolve_data_root(&arg, env.clone()).unwrap(),
            PathBuf::from("cli-root")
        );
        assert_eq!(
            resolve_data_root(&[], env).unwrap(),
            PathBuf::from("env-root")
        );
        assert!(resolve_data_root(&[], None).is_err());
        assert!(resolve_data_root(&[], Some(OsString::from(""))).is_err());
        assert!(resolve_data_root(&[OsString::from("a"), OsString::from("b")], None).is_err());
    }

    #[test]
    fn summary_json_serializes_every_count_field() {
        let summary = VanillaRulesSummary {
            tags: TagSummary {
                item: 1,
                block: 2,
                enchantment: 3,
            },
            items: 4,
            item_components: ItemComponentSummary {
                foods: 18,
                tools: 19,
                damageable_items: 20,
            },
            recipes: RecipeSummary {
                cooking: CookingSummary {
                    smelting: 5,
                    blasting: 6,
                    smoking: 7,
                    campfire_cooking: 8,
                },
                crafting_shaped: 9,
                crafting_shapeless: 10,
                crafting_transmute: 21,
                crafting_special_repairitem: 22,
                stonecutting: 11,
                smithing_transform: 12,
                smithing_trim: 13,
                brewing: 14,
                skipped: 15,
            },
            loot_tables: 16,
            loot_categories: LootCategorySummary {
                blocks: 23,
                entities: 24,
                chests: 25,
                gameplay: 26,
                archaeology: 27,
            },
            unsupported_loot_nodes: UnsupportedLootSummary {
                count: 28,
                kinds: vec![
                    "minecraft:exploration_map".to_owned(),
                    "minecraft:filtered".to_owned(),
                ],
            },
            enchantments: 17,
            protocol_ids: ProtocolIdSummary {
                items: 29,
                component_types: 30,
                potions: 31,
                mob_effects: 32,
            },
        };
        let value = summary_json(&summary);
        assert_eq!(value["tags"]["item"], 1);
        assert_eq!(value["tags"]["block"], 2);
        assert_eq!(value["tags"]["enchantment"], 3);
        assert_eq!(value["items"], 4);
        assert_eq!(value["item_components"]["foods"], 18);
        assert_eq!(value["item_components"]["tools"], 19);
        assert_eq!(value["item_components"]["damageable_items"], 20);
        assert_eq!(value["recipes"]["cooking"]["smelting"], 5);
        assert_eq!(value["recipes"]["cooking"]["blasting"], 6);
        assert_eq!(value["recipes"]["cooking"]["smoking"], 7);
        assert_eq!(value["recipes"]["cooking"]["campfire_cooking"], 8);
        assert_eq!(value["recipes"]["crafting_shaped"], 9);
        assert_eq!(value["recipes"]["crafting_shapeless"], 10);
        assert_eq!(value["recipes"]["crafting_transmute"], 21);
        assert_eq!(value["recipes"]["crafting_special_repairitem"], 22);
        assert_eq!(value["recipes"]["stonecutting"], 11);
        assert_eq!(value["recipes"]["smithing_transform"], 12);
        assert_eq!(value["recipes"]["smithing_trim"], 13);
        assert_eq!(value["recipes"]["brewing"], 14);
        assert_eq!(value["recipes"]["skipped"], 15);
        assert_eq!(value["loot_tables"], 16);
        assert_eq!(value["loot_categories"]["blocks"], 23);
        assert_eq!(value["loot_categories"]["entities"], 24);
        assert_eq!(value["loot_categories"]["chests"], 25);
        assert_eq!(value["loot_categories"]["gameplay"], 26);
        assert_eq!(value["loot_categories"]["archaeology"], 27);
        assert_eq!(value["unsupported_loot_nodes"]["count"], 28);
        assert_eq!(
            value["unsupported_loot_nodes"]["kinds"],
            json!(["minecraft:exploration_map", "minecraft:filtered"])
        );
        assert_eq!(value["enchantments"], 17);
        assert_eq!(value["protocol_ids"]["items"], 29);
        assert_eq!(value["protocol_ids"]["component_types"], 30);
        assert_eq!(value["protocol_ids"]["potions"], 31);
        assert_eq!(value["protocol_ids"]["mob_effects"], 32);
    }
}
