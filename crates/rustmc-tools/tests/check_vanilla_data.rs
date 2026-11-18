use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(label: &str) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustmc-check-vanilla-data-{label}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_synthetic_root(root: &ScratchDir) {
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
        r#"{"components": {"minecraft:max_stack_size": 16}}"#,
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
}

fn assert_expected_json(stdout: &[u8]) {
    let parsed: Value = serde_json::from_slice(stdout).expect("stdout must be valid JSON");
    let obj = parsed.as_object().expect("JSON root must be an object");
    for key in ["tags", "items", "recipes", "loot_tables", "enchantments"] {
        assert!(obj.contains_key(key), "missing top-level key `{key}`");
    }
    assert_eq!(parsed["tags"]["item"], 1);
    assert_eq!(parsed["tags"]["block"], 1);
    assert_eq!(parsed["tags"]["enchantment"], 1);
    assert_eq!(parsed["items"], 1);
    assert_eq!(parsed["recipes"]["cooking"]["smelting"], 0);
    assert_eq!(parsed["recipes"]["cooking"]["blasting"], 0);
    assert_eq!(parsed["recipes"]["cooking"]["smoking"], 0);
    assert_eq!(parsed["recipes"]["cooking"]["campfire_cooking"], 0);
    assert_eq!(parsed["recipes"]["crafting_shaped"], 1);
    assert_eq!(parsed["recipes"]["crafting_shapeless"], 0);
    assert_eq!(parsed["recipes"]["stonecutting"], 0);
    assert_eq!(parsed["recipes"]["smithing_transform"], 0);
    assert_eq!(parsed["recipes"]["smithing_trim"], 0);
    assert_eq!(parsed["recipes"]["brewing"], 0);
    assert_eq!(parsed["recipes"]["skipped"], 0);
    assert_eq!(parsed["loot_tables"], 1);
    assert_eq!(parsed["enchantments"], 1);
}

#[test]
fn synthetic_root_via_arg_and_env_exits_0_and_prints_json_summary() {
    let root = ScratchDir::new("ok");
    write_synthetic_root(&root);

    let output = Command::new(env!("CARGO_BIN_EXE_check_vanilla_data"))
        .arg(root.path())
        .env_remove("RUSTMC_VANILLA_DATA")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert_expected_json(&output.stdout);

    let env_output = Command::new(env!("CARGO_BIN_EXE_check_vanilla_data"))
        .env("RUSTMC_VANILLA_DATA", root.path())
        .output()
        .unwrap();
    assert_eq!(env_output.status.code(), Some(0));
    assert!(env_output.stderr.is_empty());
    assert_expected_json(&env_output.stdout);
}

#[test]
fn missing_root_and_broken_data_exit_1_with_error_chain() {
    let missing = ScratchDir::new("missing").path().join("absent");
    let output = Command::new(env!("CARGO_BIN_EXE_check_vanilla_data"))
        .arg(&missing)
        .env_remove("RUSTMC_VANILLA_DATA")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error:"), "{stderr}");
    assert!(stderr.contains("caused by:"), "{stderr}");

    let broken = ScratchDir::new("broken-recipe");
    write_synthetic_root(&broken);
    broken.write("data/testns/recipe/bad.json", r#"{"type": 42}"#);
    let broken_output = Command::new(env!("CARGO_BIN_EXE_check_vanilla_data"))
        .arg(broken.path())
        .env_remove("RUSTMC_VANILLA_DATA")
        .output()
        .unwrap();
    assert_eq!(broken_output.status.code(), Some(1));
    assert!(broken_output.stdout.is_empty());
    let broken_stderr = String::from_utf8_lossy(&broken_output.stderr);
    assert!(broken_stderr.contains("RecipeData"), "{broken_stderr}");
    assert!(broken_stderr.contains("caused by:"), "{broken_stderr}");
    assert!(broken_stderr.contains("bad.json"), "{broken_stderr}");
}

#[test]
fn missing_arg_and_env_or_extra_args_exit_2_with_usage() {
    let no_input = Command::new(env!("CARGO_BIN_EXE_check_vanilla_data"))
        .env_remove("RUSTMC_VANILLA_DATA")
        .output()
        .unwrap();
    assert_eq!(no_input.status.code(), Some(2));
    assert!(no_input.stdout.is_empty());
    assert!(String::from_utf8_lossy(&no_input.stderr).contains("usage:"));

    let extra_args = Command::new(env!("CARGO_BIN_EXE_check_vanilla_data"))
        .args(["one", "two"])
        .env_remove("RUSTMC_VANILLA_DATA")
        .output()
        .unwrap();
    assert_eq!(extra_args.status.code(), Some(2));
    assert!(extra_args.stdout.is_empty());
    assert!(String::from_utf8_lossy(&extra_args.stderr).contains("usage:"));
}
