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
        "recipes": {
            "cooking": {
                "smelting": summary.recipes.cooking.smelting,
                "blasting": summary.recipes.cooking.blasting,
                "smoking": summary.recipes.cooking.smoking,
                "campfire_cooking": summary.recipes.cooking.campfire_cooking,
            },
            "crafting_shaped": summary.recipes.crafting_shaped,
            "crafting_shapeless": summary.recipes.crafting_shapeless,
            "stonecutting": summary.recipes.stonecutting,
            "smithing_transform": summary.recipes.smithing_transform,
            "smithing_trim": summary.recipes.smithing_trim,
            "brewing": summary.recipes.brewing,
            "skipped": summary.recipes.skipped,
        },
        "loot_tables": summary.loot_tables,
        "enchantments": summary.enchantments,
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
    use rustmc_server::world::vanilla_rules::{CookingSummary, RecipeSummary, TagSummary};

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
            recipes: RecipeSummary {
                cooking: CookingSummary {
                    smelting: 5,
                    blasting: 6,
                    smoking: 7,
                    campfire_cooking: 8,
                },
                crafting_shaped: 9,
                crafting_shapeless: 10,
                stonecutting: 11,
                smithing_transform: 12,
                smithing_trim: 13,
                brewing: 14,
                skipped: 15,
            },
            loot_tables: 16,
            enchantments: 17,
        };
        let value = summary_json(&summary);
        assert_eq!(value["tags"]["item"], 1);
        assert_eq!(value["tags"]["block"], 2);
        assert_eq!(value["tags"]["enchantment"], 3);
        assert_eq!(value["items"], 4);
        assert_eq!(value["recipes"]["cooking"]["smelting"], 5);
        assert_eq!(value["recipes"]["cooking"]["blasting"], 6);
        assert_eq!(value["recipes"]["cooking"]["smoking"], 7);
        assert_eq!(value["recipes"]["cooking"]["campfire_cooking"], 8);
        assert_eq!(value["recipes"]["crafting_shaped"], 9);
        assert_eq!(value["recipes"]["crafting_shapeless"], 10);
        assert_eq!(value["recipes"]["stonecutting"], 11);
        assert_eq!(value["recipes"]["smithing_transform"], 12);
        assert_eq!(value["recipes"]["smithing_trim"], 13);
        assert_eq!(value["recipes"]["brewing"], 14);
        assert_eq!(value["recipes"]["skipped"], 15);
        assert_eq!(value["loot_tables"], 16);
        assert_eq!(value["enchantments"], 17);
    }
}
