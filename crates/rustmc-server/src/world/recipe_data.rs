//! Runtime loader for operator-provisioned vanilla recipe JSON files.
//!
//! Reads `data/<namespace>/recipe/**/*.json` at runtime, resolves item tag
//! ingredients via [`ItemTags`], converts crafting recipes into
//! [`crate::world::crafting`] types, indexes cooking and stonecutting recipes,
//! and records unsupported recipe types or component-bearing results in
//! [`RecipeData::skipped`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::item::ItemStack;
use crate::vanilla::feature::ItemTags;
use crate::vanilla::worldgen::{WorldgenError, read_dir_optional, read_json, walk_json};
use crate::world::crafting::{Ingredient, Recipe, ShapedRecipe, ShapelessRecipe};
use crate::world::furnace::default_max_stack;

/// Cooking recipe category matching the four vanilla furnace-family block types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CookingKind {
    Smelting,
    Blasting,
    Smoking,
    CampfireCooking,
}

impl CookingKind {
    /// All supported cooking kinds in canonical order.
    pub const ALL: [Self; 4] = [
        Self::Smelting,
        Self::Blasting,
        Self::Smoking,
        Self::CampfireCooking,
    ];

    /// The vanilla recipe `type` identifier for this cooking kind.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Smelting => "minecraft:smelting",
            Self::Blasting => "minecraft:blasting",
            Self::Smoking => "minecraft:smoking",
            Self::CampfireCooking => "minecraft:campfire_cooking",
        }
    }

    fn from_recipe_type(recipe_type: &str) -> Option<Self> {
        match recipe_type {
            "minecraft:smelting" => Some(Self::Smelting),
            "minecraft:blasting" => Some(Self::Blasting),
            "minecraft:smoking" => Some(Self::Smoking),
            "minecraft:campfire_cooking" => Some(Self::CampfireCooking),
            _ => None,
        }
    }
}

impl fmt::Display for CookingKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A parsed cooking recipe (`smelting`, `blasting`, `smoking`, or `campfire_cooking`).
#[derive(Debug, Clone, PartialEq)]
pub struct CookingRecipe {
    id: String,
    kind: CookingKind,
    ingredient: Ingredient,
    result: ItemStack,
    experience: f32,
    cooking_time: u16,
    group: Option<String>,
    category: Option<String>,
}

impl CookingRecipe {
    /// Namespaced recipe identifier (`<namespace>:<path without .json>`).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Cooking block category (`smelting`, `blasting`, `smoking`, or `campfire_cooking`).
    #[must_use]
    pub fn kind(&self) -> CookingKind {
        self.kind
    }

    /// Resolved input ingredient items.
    #[must_use]
    pub fn ingredient(&self) -> &Ingredient {
        &self.ingredient
    }

    /// Output item stack produced per cook.
    #[must_use]
    pub fn result(&self) -> &ItemStack {
        &self.result
    }

    /// Experience awarded per cooked output.
    #[must_use]
    pub fn experience(&self) -> f32 {
        self.experience
    }

    /// Cooking duration in game ticks.
    #[must_use]
    pub fn cooking_time(&self) -> u16 {
        self.cooking_time
    }

    /// Optional recipe-book group name.
    #[must_use]
    pub fn group(&self) -> Option<&str> {
        self.group.as_deref()
    }

    /// Optional recipe-book category name.
    #[must_use]
    pub fn category(&self) -> Option<&str> {
        self.category.as_deref()
    }
}

/// A parsed `minecraft:stonecutting` recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StonecuttingRecipe {
    id: String,
    ingredient: Ingredient,
    result: ItemStack,
}

impl StonecuttingRecipe {
    /// Namespaced recipe identifier (`<namespace>:<path without .json>`).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Resolved input ingredient items.
    #[must_use]
    pub fn ingredient(&self) -> &Ingredient {
        &self.ingredient
    }

    /// Output item stack produced by cutting the ingredient.
    #[must_use]
    pub fn result(&self) -> &ItemStack {
        &self.result
    }
}

/// Reason a recipe file was skipped instead of loaded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SkipReason {
    /// Recipe `type` is outside the supported crafting/cooking/stonecutting kinds.
    UnsupportedType(String),
    /// Recipe `result` carries item `components`, which are not modeled yet.
    ResultHasComponents,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedType(kind) => write!(f, "unsupported recipe type: {kind}"),
            Self::ResultHasComponents => f.write_str("result has components"),
        }
    }
}

/// Errors encountered while loading vanilla recipe files from disk.
#[derive(Debug)]
pub enum RecipeDataError {
    Worldgen(WorldgenError),
    Invalid {
        file: PathBuf,
        message: String,
    },
    DuplicateCookingInput {
        kind: CookingKind,
        item: String,
        first_recipe: String,
        second_recipe: String,
    },
}

impl fmt::Display for RecipeDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Worldgen(error) => write!(f, "{error}"),
            Self::Invalid { file, message } => {
                write!(f, "invalid recipe file {}: {message}", file.display())
            }
            Self::DuplicateCookingInput {
                kind,
                item,
                first_recipe,
                second_recipe,
            } => write!(
                f,
                "duplicate {kind} input `{item}` in recipes `{first_recipe}` and `{second_recipe}`"
            ),
        }
    }
}

impl std::error::Error for RecipeDataError {}

impl From<WorldgenError> for RecipeDataError {
    fn from(error: WorldgenError) -> Self {
        Self::Worldgen(error)
    }
}

/// Loaded vanilla recipe dataset from an operator-provisioned data root.
#[derive(Debug, Clone, Default)]
pub struct RecipeData {
    cooking: BTreeMap<CookingKind, Vec<CookingRecipe>>,
    crafting: Vec<Recipe>,
    stonecutting: Vec<StonecuttingRecipe>,
    skipped: Vec<(String, SkipReason)>,
    total_files: usize,
}

impl RecipeData {
    /// Loads every `data/<namespace>/recipe/**/*.json` file under `root`,
    /// resolving `#tag` ingredients against `tags`.
    pub fn load(root: &Path, tags: &ItemTags) -> Result<Self, RecipeDataError> {
        let mut data = Self::default();
        let data_dir = root.join("data");
        let Some(namespaces) = read_dir_optional(&data_dir)? else {
            return Ok(data);
        };

        let mut ns_dirs = Vec::new();
        for namespace in namespaces {
            let namespace =
                namespace.map_err(|error| WorldgenError::Io(data_dir.clone(), error))?;
            if namespace
                .file_type()
                .map_err(|error| WorldgenError::Io(namespace.path(), error))?
                .is_dir()
            {
                ns_dirs.push(namespace.path());
            }
        }
        ns_dirs.sort();

        let mut seen_cooking: BTreeMap<CookingKind, HashMap<String, String>> = BTreeMap::new();

        for ns_path in ns_dirs {
            let ns = ns_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let recipe_dir = ns_path.join("recipe");
            for entry in walk_json(&recipe_dir)? {
                data.total_files += 1;
                let id = recipe_id(&ns, &recipe_dir, &entry);
                let document = read_json(&entry)?;
                let obj = document
                    .as_object()
                    .ok_or_else(|| RecipeDataError::Invalid {
                        file: entry.clone(),
                        message: "recipe root must be a JSON object".to_owned(),
                    })?;
                let recipe_type = obj.get("type").and_then(Value::as_str).ok_or_else(|| {
                    RecipeDataError::Invalid {
                        file: entry.clone(),
                        message: "missing or non-string `type` field".to_owned(),
                    }
                })?;

                if let Some(kind) = CookingKind::from_recipe_type(recipe_type) {
                    match parse_cooking_recipe(&id, kind, obj, tags, &entry)? {
                        ParsedOrSkipped::Skipped(reason) => data.skipped.push((id, reason)),
                        ParsedOrSkipped::Loaded(recipe) => {
                            let by_item = seen_cooking.entry(kind).or_default();
                            for item in recipe.ingredient().allowed_items() {
                                if let Some(first_recipe) = by_item.get(item) {
                                    return Err(RecipeDataError::DuplicateCookingInput {
                                        kind,
                                        item: item.clone(),
                                        first_recipe: first_recipe.clone(),
                                        second_recipe: id,
                                    });
                                }
                                by_item.insert(item.clone(), id.clone());
                            }
                            data.cooking.entry(kind).or_default().push(recipe);
                        }
                    }
                } else if recipe_type == "minecraft:crafting_shaped" {
                    match parse_shaped_recipe(&id, obj, tags, &entry)? {
                        ParsedOrSkipped::Skipped(reason) => data.skipped.push((id, reason)),
                        ParsedOrSkipped::Loaded(recipe) => {
                            data.crafting.push(Recipe::Shaped(recipe))
                        }
                    }
                } else if recipe_type == "minecraft:crafting_shapeless" {
                    match parse_shapeless_recipe(&id, obj, tags, &entry)? {
                        ParsedOrSkipped::Skipped(reason) => data.skipped.push((id, reason)),
                        ParsedOrSkipped::Loaded(recipe) => {
                            data.crafting.push(Recipe::Shapeless(recipe))
                        }
                    }
                } else if recipe_type == "minecraft:stonecutting" {
                    match parse_stonecutting_recipe(&id, obj, tags, &entry)? {
                        ParsedOrSkipped::Skipped(reason) => data.skipped.push((id, reason)),
                        ParsedOrSkipped::Loaded(recipe) => data.stonecutting.push(recipe),
                    }
                } else {
                    data.skipped
                        .push((id, SkipReason::UnsupportedType(recipe_type.to_owned())));
                }
            }
        }

        data.crafting.sort_by(|a, b| a.id().cmp(&b.id()));
        Ok(data)
    }

    /// Loaded cooking recipes for `kind`.
    #[must_use]
    pub fn cooking(&self, kind: CookingKind) -> &[CookingRecipe] {
        self.cooking.get(&kind).map_or(&[], Vec::as_slice)
    }

    /// Number of distinct input items covered by cooking recipes of `kind`.
    #[must_use]
    pub fn cooking_input_count(&self, kind: CookingKind) -> usize {
        self.cooking(kind)
            .iter()
            .map(|recipe| recipe.ingredient().allowed_items().len())
            .sum()
    }

    /// Loaded crafting recipes (`crafting_shaped` and `crafting_shapeless`),
    /// sorted by recipe identifier.
    #[must_use]
    pub fn crafting(&self) -> &[Recipe] {
        &self.crafting
    }

    /// Iterator over loaded `minecraft:crafting_shaped` recipes.
    pub fn shaped_recipes(&self) -> impl Iterator<Item = &ShapedRecipe> {
        self.crafting.iter().filter_map(|recipe| match recipe {
            Recipe::Shaped(shaped) => Some(shaped),
            Recipe::Shapeless(_) => None,
        })
    }

    /// Iterator over loaded `minecraft:crafting_shapeless` recipes.
    pub fn shapeless_recipes(&self) -> impl Iterator<Item = &ShapelessRecipe> {
        self.crafting.iter().filter_map(|recipe| match recipe {
            Recipe::Shapeless(shapeless) => Some(shapeless),
            Recipe::Shaped(_) => None,
        })
    }

    /// Loaded `minecraft:stonecutting` recipes.
    #[must_use]
    pub fn stonecutting(&self) -> &[StonecuttingRecipe] {
        &self.stonecutting
    }

    /// Recipes skipped due to unsupported `type` or `result.components`.
    #[must_use]
    pub fn skipped(&self) -> &[(String, SkipReason)] {
        &self.skipped
    }

    /// Total number of recipe JSON files read from the data root.
    #[must_use]
    pub fn total_files(&self) -> usize {
        self.total_files
    }

    /// Finds a loaded cooking recipe by its namespaced id.
    #[must_use]
    pub fn get_cooking_by_id(&self, id: &str) -> Option<&CookingRecipe> {
        self.cooking
            .values()
            .flatten()
            .find(|recipe| recipe.id() == id)
    }

    /// Finds a loaded crafting recipe by its namespaced id.
    #[must_use]
    pub fn get_crafting_by_id(&self, id: &str) -> Option<&Recipe> {
        self.crafting.iter().find(|recipe| recipe.id() == Some(id))
    }

    /// Finds a loaded stonecutting recipe by its namespaced id.
    #[must_use]
    pub fn get_stonecutting_by_id(&self, id: &str) -> Option<&StonecuttingRecipe> {
        self.stonecutting.iter().find(|recipe| recipe.id() == id)
    }
}

enum ParsedOrSkipped<T> {
    Loaded(T),
    Skipped(SkipReason),
}

fn recipe_id(ns: &str, recipe_dir: &Path, entry: &Path) -> String {
    let relative = entry
        .strip_prefix(recipe_dir)
        .unwrap_or(entry)
        .with_extension("");
    let mut id = String::from(ns);
    id.push(':');
    id.push_str(&relative.to_string_lossy().replace('\\', "/"));
    id
}

fn invalid(file: &Path, message: impl Into<String>) -> RecipeDataError {
    RecipeDataError::Invalid {
        file: file.to_path_buf(),
        message: message.into(),
    }
}

fn parse_optional_string(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    file: &Path,
) -> Result<Option<String>, RecipeDataError> {
    match obj.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(|s| Some(s.to_owned()))
            .ok_or_else(|| invalid(file, format!("field `{key}` must be a string"))),
    }
}

fn parse_result(
    obj: &serde_json::Map<String, Value>,
    file: &Path,
) -> Result<ParsedOrSkipped<ItemStack>, RecipeDataError> {
    let result_val = obj
        .get("result")
        .ok_or_else(|| invalid(file, "missing `result` field"))?;
    let result_obj = result_val
        .as_object()
        .ok_or_else(|| invalid(file, "`result` must be a JSON object"))?;
    if result_obj.contains_key("components") {
        return Ok(ParsedOrSkipped::Skipped(SkipReason::ResultHasComponents));
    }
    let id = result_obj
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(file, "missing or non-string `result.id`"))?;
    let count = match result_obj.get("count") {
        None => 1,
        Some(value) => {
            let raw = value
                .as_u64()
                .ok_or_else(|| invalid(file, "`result.count` must be a positive integer"))?;
            u16::try_from(raw)
                .map_err(|_| invalid(file, format!("`result.count` {raw} out of range")))?
        }
    };
    let stack = ItemStack::try_new(id, count, default_max_stack(id))
        .map_err(|error| invalid(file, format!("invalid `result` stack: {error}")))?;
    Ok(ParsedOrSkipped::Loaded(stack))
}

fn resolve_ingredient_token(
    token: &str,
    tags: &ItemTags,
    file: &Path,
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
) -> Result<(), RecipeDataError> {
    if let Some(tag) = token.strip_prefix('#') {
        let members = tags
            .members(tag)
            .ok_or_else(|| invalid(file, format!("unknown item tag `#{tag}`")))?;
        for member in members {
            if seen.insert(member.clone()) {
                out.push(member.clone());
            }
        }
    } else {
        if token.is_empty() {
            return Err(invalid(file, "empty item id in ingredient"));
        }
        if seen.insert(token.to_owned()) {
            out.push(token.to_owned());
        }
    }
    Ok(())
}

fn parse_ingredient(
    value: &Value,
    tags: &ItemTags,
    file: &Path,
) -> Result<Ingredient, RecipeDataError> {
    let mut resolved = Vec::new();
    let mut seen = HashSet::new();
    match value {
        Value::String(token) => {
            resolve_ingredient_token(token, tags, file, &mut resolved, &mut seen)?;
        }
        Value::Array(items) => {
            if items.is_empty() {
                return Err(invalid(file, "ingredient list must not be empty"));
            }
            for item in items {
                let token = item.as_str().ok_or_else(|| {
                    invalid(file, "ingredient list entries must be item or #tag strings")
                })?;
                resolve_ingredient_token(token, tags, file, &mut resolved, &mut seen)?;
            }
        }
        _ => {
            return Err(invalid(
                file,
                "ingredient must be an item string, #tag string, or list of strings",
            ));
        }
    }
    Ok(Ingredient::any(resolved))
}

fn parse_cooking_recipe(
    id: &str,
    kind: CookingKind,
    obj: &serde_json::Map<String, Value>,
    tags: &ItemTags,
    file: &Path,
) -> Result<ParsedOrSkipped<CookingRecipe>, RecipeDataError> {
    let result = match parse_result(obj, file)? {
        ParsedOrSkipped::Skipped(reason) => return Ok(ParsedOrSkipped::Skipped(reason)),
        ParsedOrSkipped::Loaded(stack) => stack,
    };
    let ingredient_val = obj
        .get("ingredient")
        .ok_or_else(|| invalid(file, "missing `ingredient` field"))?;
    let ingredient = parse_ingredient(ingredient_val, tags, file)?;
    let experience = match obj.get("experience") {
        None => 0.0_f32,
        Some(value) => {
            let raw = value
                .as_f64()
                .ok_or_else(|| invalid(file, "`experience` must be a number"))?;
            if !raw.is_finite() || raw < 0.0 {
                return Err(invalid(
                    file,
                    "`experience` must be non-negative and finite",
                ));
            }
            raw as f32
        }
    };
    let cooking_time_u64 = obj
        .get("cookingtime")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid(file, "missing or invalid `cookingtime` integer"))?;
    let cooking_time = u16::try_from(cooking_time_u64).map_err(|_| {
        invalid(
            file,
            format!("`cookingtime` {cooking_time_u64} exceeds u16::MAX"),
        )
    })?;
    let group = parse_optional_string(obj, "group", file)?;
    let category = parse_optional_string(obj, "category", file)?;

    Ok(ParsedOrSkipped::Loaded(CookingRecipe {
        id: id.to_owned(),
        kind,
        ingredient,
        result,
        experience,
        cooking_time,
        group,
        category,
    }))
}

fn parse_shaped_recipe(
    id: &str,
    obj: &serde_json::Map<String, Value>,
    tags: &ItemTags,
    file: &Path,
) -> Result<ParsedOrSkipped<ShapedRecipe>, RecipeDataError> {
    let result = match parse_result(obj, file)? {
        ParsedOrSkipped::Skipped(reason) => return Ok(ParsedOrSkipped::Skipped(reason)),
        ParsedOrSkipped::Loaded(stack) => stack,
    };
    let pattern_arr = obj
        .get("pattern")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(file, "missing or non-array `pattern`"))?;
    let mut pattern = Vec::with_capacity(pattern_arr.len());
    for row in pattern_arr {
        let row_str = row
            .as_str()
            .ok_or_else(|| invalid(file, "`pattern` rows must be strings"))?;
        pattern.push(row_str);
    }

    let key_obj = obj
        .get("key")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(file, "missing or non-object `key`"))?;
    let mut keys = Vec::with_capacity(key_obj.len());
    for (k, val) in key_obj {
        let mut chars = k.chars();
        let ch = chars
            .next()
            .filter(|_| chars.next().is_none())
            .ok_or_else(|| invalid(file, format!("shaped recipe key `{k}` must be one char")))?;
        let ingredient = parse_ingredient(val, tags, file)?;
        keys.push((ch, ingredient));
    }

    let group = parse_optional_string(obj, "group", file)?;
    let category = parse_optional_string(obj, "category", file)?;
    let show_notification = match obj.get("show_notification") {
        None => None,
        Some(value) => Some(
            value
                .as_bool()
                .ok_or_else(|| invalid(file, "`show_notification` must be a boolean"))?,
        ),
    };

    let mut recipe = ShapedRecipe::from_pattern(&pattern, &keys, result)
        .map_err(|error| invalid(file, format!("invalid shaped pattern: {error}")))?
        .with_id(id);
    if let Some(group) = group {
        recipe = recipe.with_group(group);
    }
    if let Some(category) = category {
        recipe = recipe.with_category(category);
    }
    if let Some(show) = show_notification {
        recipe = recipe.with_show_notification(show);
    }
    Ok(ParsedOrSkipped::Loaded(recipe))
}

fn parse_shapeless_recipe(
    id: &str,
    obj: &serde_json::Map<String, Value>,
    tags: &ItemTags,
    file: &Path,
) -> Result<ParsedOrSkipped<ShapelessRecipe>, RecipeDataError> {
    let result = match parse_result(obj, file)? {
        ParsedOrSkipped::Skipped(reason) => return Ok(ParsedOrSkipped::Skipped(reason)),
        ParsedOrSkipped::Loaded(stack) => stack,
    };
    let ing_arr = obj
        .get("ingredients")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(file, "missing or non-array `ingredients`"))?;
    let mut ingredients = Vec::with_capacity(ing_arr.len());
    for val in ing_arr {
        ingredients.push(parse_ingredient(val, tags, file)?);
    }

    let group = parse_optional_string(obj, "group", file)?;
    let category = parse_optional_string(obj, "category", file)?;

    let mut recipe = ShapelessRecipe::new(ingredients, result)
        .map_err(|error| invalid(file, format!("invalid shapeless recipe: {error}")))?
        .with_id(id);
    if let Some(group) = group {
        recipe = recipe.with_group(group);
    }
    if let Some(category) = category {
        recipe = recipe.with_category(category);
    }
    Ok(ParsedOrSkipped::Loaded(recipe))
}

fn parse_stonecutting_recipe(
    id: &str,
    obj: &serde_json::Map<String, Value>,
    tags: &ItemTags,
    file: &Path,
) -> Result<ParsedOrSkipped<StonecuttingRecipe>, RecipeDataError> {
    let result = match parse_result(obj, file)? {
        ParsedOrSkipped::Skipped(reason) => return Ok(ParsedOrSkipped::Skipped(reason)),
        ParsedOrSkipped::Loaded(stack) => stack,
    };
    let ingredient_val = obj
        .get("ingredient")
        .ok_or_else(|| invalid(file, "missing `ingredient` field"))?;
    let ingredient = parse_ingredient(ingredient_val, tags, file)?;

    Ok(ParsedOrSkipped::Loaded(StonecuttingRecipe {
        id: id.to_owned(),
        ingredient,
        result,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn scratch_root(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustmc-recipe-data-{label}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create scratch dir");
        path
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, text).expect("write");
    }

    #[test]
    fn loads_each_supported_type_and_resolves_tag_and_list_ingredients() {
        let root = scratch_root("all-types");
        write(
            &root.join("data/testns/tags/item/widgets.json"),
            r#"{"values": ["testns:widget_a", "testns:widget_b"]}"#,
        );
        write(
            &root.join("data/testns/recipe/smelt_widget.json"),
            r#"{
                "type": "minecraft:smelting",
                "ingredient": "testns:raw_widget",
                "result": {"id": "testns:widget_ingot"},
                "experience": 0.5,
                "cookingtime": 200,
                "group": "ingots",
                "category": "misc"
            }"#,
        );
        write(
            &root.join("data/testns/recipe/blast_widgets.json"),
            r##"{
                "type": "minecraft:blasting",
                "ingredient": "#testns:widgets",
                "result": {"id": "testns:dense_widget", "count": 2},
                "cookingtime": 100
            }"##,
        );
        write(
            &root.join("data/testns/recipe/smoke_herb.json"),
            r#"{
                "type": "minecraft:smoking",
                "ingredient": ["testns:herb_a", "testns:herb_b"],
                "result": {"id": "testns:dried_herb"},
                "experience": 0.25,
                "cookingtime": 100
            }"#,
        );
        write(
            &root.join("data/testns/recipe/campfire_herb.json"),
            r#"{
                "type": "minecraft:campfire_cooking",
                "ingredient": "testns:herb_a",
                "result": {"id": "testns:charred_herb"},
                "cookingtime": 600
            }"#,
        );
        write(
            &root.join("data/testns/recipe/shaped_gear.json"),
            r##"{
                "type": "minecraft:crafting_shaped",
                "pattern": [" # ", "# #", " # "],
                "key": {"#": "#testns:widgets"},
                "result": {"id": "testns:gear", "count": 4},
                "group": "gears",
                "category": "misc",
                "show_notification": false
            }"##,
        );
        write(
            &root.join("data/testns/recipe/sub/shapeless_mix.json"),
            r##"{
                "type": "minecraft:crafting_shapeless",
                "ingredients": ["#testns:widgets", ["testns:herb_a", "testns:herb_b"]],
                "result": {"id": "testns:mix"}
            }"##,
        );
        write(
            &root.join("data/testns/recipe/cut_stone.json"),
            r#"{
                "type": "minecraft:stonecutting",
                "ingredient": "testns:raw_stone",
                "result": {"id": "testns:cut_slab", "count": 2}
            }"#,
        );

        let tags = ItemTags::load(&root).expect("load item tags");
        let data = RecipeData::load(&root, &tags).expect("load recipes");

        assert_eq!(data.total_files(), 7);
        assert!(data.skipped().is_empty());

        let smelt = data
            .get_cooking_by_id("testns:smelt_widget")
            .expect("smelt recipe");
        assert_eq!(smelt.kind(), CookingKind::Smelting);
        assert_eq!(smelt.result().item(), "testns:widget_ingot");
        assert_eq!(smelt.result().count(), 1, "count defaults to 1");
        assert!((smelt.experience() - 0.5).abs() < 1e-6);
        assert_eq!(smelt.cooking_time(), 200);
        assert_eq!(smelt.group(), Some("ingots"));
        assert_eq!(smelt.category(), Some("misc"));

        let blast = data
            .get_cooking_by_id("testns:blast_widgets")
            .expect("blast recipe");
        assert_eq!(blast.kind(), CookingKind::Blasting);
        assert_eq!(
            blast.ingredient().allowed_items(),
            &["testns:widget_a", "testns:widget_b"]
        );
        assert_eq!(blast.result().count(), 2);
        assert_eq!(blast.experience(), 0.0, "experience defaults to 0");
        assert_eq!(blast.cooking_time(), 100);

        let smoke = data
            .get_cooking_by_id("testns:smoke_herb")
            .expect("smoke recipe");
        assert_eq!(smoke.kind(), CookingKind::Smoking);
        assert_eq!(
            smoke.ingredient().allowed_items(),
            &["testns:herb_a", "testns:herb_b"]
        );

        let campfire = data
            .get_cooking_by_id("testns:campfire_herb")
            .expect("campfire recipe");
        assert_eq!(campfire.kind(), CookingKind::CampfireCooking);
        assert_eq!(campfire.cooking_time(), 600);

        assert_eq!(data.shaped_recipes().count(), 1);
        let shaped = data.shaped_recipes().next().expect("shaped recipe present");
        assert_eq!(shaped.id(), Some("testns:shaped_gear"));
        assert_eq!(shaped.result().count(), 4);
        assert_eq!(shaped.group(), Some("gears"));
        assert_eq!(shaped.category(), Some("misc"));
        assert_eq!(shaped.show_notification(), Some(false));

        assert_eq!(data.shapeless_recipes().count(), 1);
        let shapeless = data
            .get_crafting_by_id("testns:sub/shapeless_mix")
            .expect("nested path becomes id");
        assert_eq!(shapeless.output().item(), "testns:mix");
        assert_eq!(shapeless.output().count(), 1);

        assert_eq!(data.stonecutting().len(), 1);
        let cut = data
            .get_stonecutting_by_id("testns:cut_stone")
            .expect("stonecutting recipe");
        assert_eq!(cut.result().item(), "testns:cut_slab");
        assert_eq!(cut.result().count(), 2);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn skips_unsupported_types_and_component_results() {
        let root = scratch_root("skipped");
        write(
            &root.join("data/testns/recipe/smithing.json"),
            r#"{"type": "minecraft:smithing_trim"}"#,
        );
        write(
            &root.join("data/testns/recipe/stew.json"),
            r#"{
                "type": "minecraft:crafting_shapeless",
                "ingredients": ["testns:bowl", "testns:flower"],
                "result": {
                    "id": "testns:stew",
                    "components": {"minecraft:suspicious_stew_effects": []}
                }
            }"#,
        );

        let tags = ItemTags::default();
        let data = RecipeData::load(&root, &tags).expect("unsupported and components skip");
        assert_eq!(data.total_files(), 2);
        assert!(data.crafting().is_empty());
        assert_eq!(
            data.skipped(),
            &[
                (
                    "testns:smithing".to_owned(),
                    SkipReason::UnsupportedType("minecraft:smithing_trim".to_owned())
                ),
                ("testns:stew".to_owned(), SkipReason::ResultHasComponents),
            ]
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn bad_json_and_unknown_tag_errors_name_the_file() {
        let root_json = scratch_root("bad-json");
        let bad_file = root_json.join("data/testns/recipe/broken.json");
        write(&bad_file, "{ not valid json");
        let err = RecipeData::load(&root_json, &ItemTags::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("broken.json"), "error names file: {err}");
        fs::remove_dir_all(root_json).expect("cleanup");

        let root_tag = scratch_root("unknown-tag");
        let tag_file = root_tag.join("data/testns/recipe/missing_tag.json");
        write(
            &tag_file,
            r##"{
                "type": "minecraft:crafting_shapeless",
                "ingredients": ["#testns:no_such_tag"],
                "result": {"id": "testns:out"}
            }"##,
        );
        let err = RecipeData::load(&root_tag, &ItemTags::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing_tag.json"), "error names file: {err}");
        assert!(err.contains("testns:no_such_tag"), "error names tag: {err}");
        fs::remove_dir_all(root_tag).expect("cleanup");
    }

    #[test]
    fn duplicate_cooking_input_in_same_kind_errors_naming_both_recipes() {
        let root = scratch_root("dup-cooking");
        write(
            &root.join("data/testns/recipe/a_smelt.json"),
            r#"{
                "type": "minecraft:smelting",
                "ingredient": "testns:ore",
                "result": {"id": "testns:ingot_a"},
                "cookingtime": 200
            }"#,
        );
        write(
            &root.join("data/testns/recipe/b_smelt.json"),
            r#"{
                "type": "minecraft:smelting",
                "ingredient": ["testns:other", "testns:ore"],
                "result": {"id": "testns:ingot_b"},
                "cookingtime": 200
            }"#,
        );
        let err = RecipeData::load(&root, &ItemTags::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("testns:a_smelt"), "{err}");
        assert!(err.contains("testns:b_smelt"), "{err}");
        assert!(err.contains("testns:ore"), "{err}");

        // Sharing an input across DIFFERENT cooking kinds is allowed (e.g. smelting vs blasting).
        let ok_root = scratch_root("diff-cooking-kind");
        write(
            &ok_root.join("data/testns/recipe/smelt.json"),
            r#"{
                "type": "minecraft:smelting",
                "ingredient": "testns:ore",
                "result": {"id": "testns:ingot"},
                "cookingtime": 200
            }"#,
        );
        write(
            &ok_root.join("data/testns/recipe/blast.json"),
            r#"{
                "type": "minecraft:blasting",
                "ingredient": "testns:ore",
                "result": {"id": "testns:ingot"},
                "cookingtime": 100
            }"#,
        );
        assert!(RecipeData::load(&ok_root, &ItemTags::default()).is_ok());

        fs::remove_dir_all(root).expect("cleanup");
        fs::remove_dir_all(ok_root).expect("cleanup");
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_recipes_pin_vanilla_counts`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_recipes_pin_vanilla_counts() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let tags = ItemTags::load(&root).expect("load vanilla item tags");
        let data = RecipeData::load(&root, &tags).expect("load vanilla recipes");

        assert_eq!(data.cooking(CookingKind::Smelting).len(), 73);
        assert_eq!(data.cooking_input_count(CookingKind::Smelting), 163);

        assert_eq!(data.cooking(CookingKind::Blasting).len(), 25);
        assert_eq!(data.cooking_input_count(CookingKind::Blasting), 62);

        assert_eq!(data.cooking(CookingKind::Smoking).len(), 9);
        assert_eq!(data.cooking_input_count(CookingKind::Smoking), 9);

        assert_eq!(data.cooking(CookingKind::CampfireCooking).len(), 9);
        assert_eq!(data.cooking_input_count(CookingKind::CampfireCooking), 9);

        assert_eq!(data.shaped_recipes().count(), 827);
        assert_eq!(data.shapeless_recipes().count(), 358);

        let components_skipped = data
            .skipped()
            .iter()
            .filter(|(_, reason)| *reason == SkipReason::ResultHasComponents)
            .count();
        assert_eq!(components_skipped, 17);

        assert_eq!(data.stonecutting().len(), 351);

        // 1763 top-level *.json + 279 recipe/brewing/*.json = 2042 recursive *.json files.
        assert_eq!(data.total_files(), 2042);

        let mut skipped_by_reason: BTreeMap<String, usize> = BTreeMap::new();
        for (_, reason) in data.skipped() {
            *skipped_by_reason.entry(reason.to_string()).or_default() += 1;
        }
        for (reason, count) in &skipped_by_reason {
            eprintln!("skipped: {count:4}  {reason}");
        }

        let raw_iron = data
            .get_cooking_by_id("minecraft:iron_ingot_from_smelting_raw_iron")
            .expect("raw_iron smelting recipe");
        assert_eq!(
            raw_iron.ingredient().allowed_items(),
            &["minecraft:raw_iron"]
        );
        assert_eq!(raw_iron.result().item(), "minecraft:iron_ingot");
        assert_eq!(raw_iron.result().count(), 1);
        assert!((raw_iron.experience() - 0.7).abs() < 1e-6);
        assert_eq!(raw_iron.cooking_time(), 200);
    }
}
