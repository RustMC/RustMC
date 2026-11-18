//! Runtime loader for operator-provisioned vanilla recipe JSON files.
//!
//! Reads `data/<namespace>/recipe/**/*.json` at runtime, resolves item tag
//! ingredients via [`ItemTags`], converts crafting recipes into
//! [`crate::world::crafting`] types, indexes cooking, stonecutting, brewing, and
//! smithing recipes, and records unsupported recipe types or component-bearing
//! results in [`RecipeData::skipped`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::item::{ArmorTrim, ItemDefaults, ItemStack, items_equal, normalize_item_id};
use crate::vanilla::feature::ItemTags;
use crate::vanilla::worldgen::{WorldgenError, read_dir_optional, read_json, walk_json};
use crate::world::crafting::{CraftingGrid, Ingredient, Recipe, ShapedRecipe, ShapelessRecipe};
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

/// A parsed `minecraft:brewing` recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrewingRecipe {
    id: String,
    input_item: String,
    input_potions: Vec<String>,
    reagent: Ingredient,
    output: ItemStack,
}

impl BrewingRecipe {
    /// Namespaced recipe identifier (`<namespace>:<path without .json>`).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Input potion container item identifier (for example `minecraft:potion`).
    #[must_use]
    pub fn input_item(&self) -> &str {
        &self.input_item
    }

    /// Accepted input `minecraft:potion` registry identifier(s).
    #[must_use]
    pub fn input_potions(&self) -> &[String] {
        &self.input_potions
    }

    /// Resolved reagent item(s).
    #[must_use]
    pub fn reagent(&self) -> &Ingredient {
        &self.reagent
    }

    /// Output container item stack produced per brew (with its `potion` component set).
    #[must_use]
    pub fn output(&self) -> &ItemStack {
        &self.output
    }

    /// Output container item identifier (for example `minecraft:splash_potion`).
    #[must_use]
    pub fn output_item(&self) -> &str {
        self.output.item()
    }

    /// Output `minecraft:potion` registry identifier.
    #[must_use]
    pub fn output_potion(&self) -> &str {
        self.output.components().potion().unwrap_or("")
    }
}

/// A parsed `minecraft:smithing_transform` recipe.
///
/// Source: minecraft.wiki `Recipe (Java Edition)` (revision 3873625).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmithingTransformRecipe {
    id: String,
    template: Option<Ingredient>,
    base: Ingredient,
    addition: Option<Ingredient>,
    result: ItemStack,
}

impl SmithingTransformRecipe {
    /// Namespaced recipe identifier (`<namespace>:<path without .json>`).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Optional template slot ingredient.
    #[must_use]
    pub fn template(&self) -> Option<&Ingredient> {
        self.template.as_ref()
    }

    /// Base item ingredient to upgrade.
    #[must_use]
    pub fn base(&self) -> &Ingredient {
        &self.base
    }

    /// Optional addition slot ingredient.
    #[must_use]
    pub fn addition(&self) -> Option<&Ingredient> {
        self.addition.as_ref()
    }

    /// Output item stack produced by the smithing transform.
    #[must_use]
    pub fn result(&self) -> &ItemStack {
        &self.result
    }
}

/// A parsed `minecraft:smithing_trim` recipe.
///
/// Source: minecraft.wiki `Recipe (Java Edition)` (revision 3873625).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmithingTrimRecipe {
    id: String,
    template: Option<Ingredient>,
    base: Ingredient,
    addition: Ingredient,
    pattern: String,
    base_stacks: BTreeMap<String, ItemStack>,
    /// Addition item (without `minecraft:`) to the trim material id it provides.
    materials: BTreeMap<String, String>,
}

impl SmithingTrimRecipe {
    /// Namespaced recipe identifier (`<namespace>:<path without .json>`).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Optional template slot ingredient.
    #[must_use]
    pub fn template(&self) -> Option<&Ingredient> {
        self.template.as_ref()
    }

    /// Base armor ingredient to trim.
    #[must_use]
    pub fn base(&self) -> &Ingredient {
        &self.base
    }

    /// Trim material ingredient (`addition`).
    #[must_use]
    pub fn addition(&self) -> &Ingredient {
        &self.addition
    }

    /// Armor trim pattern identifier (for example `"minecraft:coast"`).
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }
}

/// Result of looking up inputs in [`RecipeData::find_smithing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmithingResult {
    /// Result stack of a `minecraft:smithing_transform` recipe.
    Transform {
        /// Matched recipe identifier.
        recipe_id: String,
        /// Transformed output stack.
        result: ItemStack,
    },
    /// Result stack of a `minecraft:smithing_trim` recipe with its `trim`
    /// data component (`pattern` + `material`) applied to the base item stack.
    ///
    /// Source: minecraft.wiki `Data component format` (revision 3903955,
    /// `Data component format/trim` revision 3685746):
    /// "`minecraft:trim`: Contains the trim applied to this armor piece."
    Trim {
        /// Matched recipe identifier.
        recipe_id: String,
        /// Base item stack with its `minecraft:trim` component set.
        result: ItemStack,
    },
}

/// Reason a recipe file was skipped instead of loaded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SkipReason {
    /// Recipe `type` is outside the supported crafting/cooking/stonecutting/brewing/smithing kinds.
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
    DuplicateBrewingInput {
        input_item: String,
        input_potion: String,
        reagent: String,
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
            Self::DuplicateBrewingInput {
                input_item,
                input_potion,
                reagent,
                first_recipe,
                second_recipe,
            } => write!(
                f,
                "duplicate brewing input ({input_item}, {input_potion}, {reagent}) in recipes `{first_recipe}` and `{second_recipe}`"
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
    brewing: Vec<BrewingRecipe>,
    brewing_by_reagent: HashMap<String, Vec<usize>>,
    smithing_transform: Vec<SmithingTransformRecipe>,
    smithing_trim: Vec<SmithingTrimRecipe>,
    skipped: Vec<(String, SkipReason)>,
    total_files: usize,
}

impl RecipeData {
    /// Loads every `data/<namespace>/recipe/**/*.json` file under `root`,
    /// resolving `#tag` ingredients against `tags` and result stack limits
    /// against the offline fallback table.
    pub fn load(root: &Path, tags: &ItemTags) -> Result<Self, RecipeDataError> {
        Self::load_inner(root, tags, None)
    }

    /// Loads every `data/<namespace>/recipe/**/*.json` file under `root`,
    /// resolving `#tag` ingredients against `tags` and result stack limits
    /// against `items`. Unknown result items fail with an error naming the
    /// recipe file.
    pub fn load_with_items(
        root: &Path,
        tags: &ItemTags,
        items: &ItemDefaults,
    ) -> Result<Self, RecipeDataError> {
        Self::load_inner(root, tags, Some(items))
    }

    fn load_inner(
        root: &Path,
        tags: &ItemTags,
        items: Option<&ItemDefaults>,
    ) -> Result<Self, RecipeDataError> {
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
        let mut seen_brewing: HashMap<(String, String, String), String> = HashMap::new();

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
                    match parse_cooking_recipe(&id, kind, obj, tags, items, &entry)? {
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
                    match parse_shaped_recipe(&id, obj, tags, items, &entry)? {
                        ParsedOrSkipped::Skipped(reason) => data.skipped.push((id, reason)),
                        ParsedOrSkipped::Loaded(recipe) => {
                            data.crafting.push(Recipe::Shaped(recipe))
                        }
                    }
                } else if recipe_type == "minecraft:crafting_shapeless" {
                    match parse_shapeless_recipe(&id, obj, tags, items, &entry)? {
                        ParsedOrSkipped::Skipped(reason) => data.skipped.push((id, reason)),
                        ParsedOrSkipped::Loaded(recipe) => {
                            data.crafting.push(Recipe::Shapeless(recipe))
                        }
                    }
                } else if recipe_type == "minecraft:stonecutting" {
                    match parse_stonecutting_recipe(&id, obj, tags, items, &entry)? {
                        ParsedOrSkipped::Skipped(reason) => data.skipped.push((id, reason)),
                        ParsedOrSkipped::Loaded(recipe) => data.stonecutting.push(recipe),
                    }
                } else if recipe_type == "minecraft:brewing" {
                    let recipe = parse_brewing_recipe(&id, obj, tags, items, &entry)?;
                    let norm_input_item = normalize_item_id(recipe.input_item()).to_owned();
                    for input_potion in recipe.input_potions() {
                        let norm_potion = normalize_item_id(input_potion).to_owned();
                        for reagent in recipe.reagent().allowed_items() {
                            let key = (
                                norm_input_item.clone(),
                                norm_potion.clone(),
                                normalize_item_id(reagent).to_owned(),
                            );
                            if let Some(first_recipe) = seen_brewing.get(&key) {
                                if first_recipe != &id {
                                    return Err(RecipeDataError::DuplicateBrewingInput {
                                        input_item: recipe.input_item().to_owned(),
                                        input_potion: input_potion.clone(),
                                        reagent: reagent.clone(),
                                        first_recipe: first_recipe.clone(),
                                        second_recipe: id,
                                    });
                                }
                            } else {
                                seen_brewing.insert(key, id.clone());
                            }
                        }
                    }
                    let idx = data.brewing.len();
                    let mut seen_reagent_keys = HashSet::new();
                    for reagent in recipe.reagent().allowed_items() {
                        let norm_reagent = normalize_item_id(reagent).to_owned();
                        if seen_reagent_keys.insert(norm_reagent.clone()) {
                            data.brewing_by_reagent
                                .entry(norm_reagent)
                                .or_default()
                                .push(idx);
                        }
                    }
                    data.brewing.push(recipe);
                } else if recipe_type == "minecraft:smithing_transform" {
                    match parse_smithing_transform_recipe(&id, obj, tags, items, &entry)? {
                        ParsedOrSkipped::Skipped(reason) => data.skipped.push((id, reason)),
                        ParsedOrSkipped::Loaded(recipe) => data.smithing_transform.push(recipe),
                    }
                } else if recipe_type == "minecraft:smithing_trim" {
                    let recipe = parse_smithing_trim_recipe(&id, obj, tags, items, &entry)?;
                    data.smithing_trim.push(recipe);
                } else {
                    data.skipped
                        .push((id, SkipReason::UnsupportedType(recipe_type.to_owned())));
                }
            }
        }

        data.crafting.sort_by(|a, b| a.id().cmp(&b.id()));
        data.stonecutting.sort_by(|a, b| a.id().cmp(b.id()));
        data.smithing_transform.sort_by(|a, b| a.id().cmp(b.id()));
        data.smithing_trim.sort_by(|a, b| a.id().cmp(b.id()));
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

    /// Loaded `minecraft:brewing` recipes.
    #[must_use]
    pub fn brewing(&self) -> &[BrewingRecipe] {
        &self.brewing
    }

    /// Finds a loaded `minecraft:brewing` recipe by its namespaced id.
    #[must_use]
    pub fn get_brewing_by_id(&self, id: &str) -> Option<&BrewingRecipe> {
        self.brewing.iter().find(|recipe| recipe.id() == id)
    }

    /// Finds a loaded `minecraft:brewing` recipe matching `input_item`,
    /// `input_potion`, and `reagent`, ignoring optional `minecraft:` prefixes.
    #[must_use]
    pub fn find_brewing(
        &self,
        input_item: &str,
        input_potion: &str,
        reagent: &str,
    ) -> Option<&BrewingRecipe> {
        let indices = self.brewing_by_reagent.get(normalize_item_id(reagent))?;
        indices.iter().find_map(|&idx| {
            let recipe = &self.brewing[idx];
            (items_equal(recipe.input_item(), input_item)
                && recipe
                    .input_potions()
                    .iter()
                    .any(|p| items_equal(p, input_potion))
                && recipe
                    .reagent()
                    .allowed_items()
                    .iter()
                    .any(|r| items_equal(r, reagent)))
            .then_some(recipe)
        })
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

    /// Lists every loaded `minecraft:stonecutting` recipe whose ingredient
    /// accepts `input_item` (ignoring an optional `minecraft:` prefix),
    /// returning `(recipe_id, result)` pairs sorted by `recipe_id`.
    ///
    /// Sources:
    /// - minecraft.wiki `Stonecutter` (revision 3892466):
    ///   - "All of its recipes require just one ingredient and produce one product (in varying quantities)."
    ///   - "The GUI contains a block selection menu that is empty. The menu is populated with blocks that can be crafted from that particular material, only if a block is inserted into its input."
    /// - minecraft.wiki `Recipe (Java Edition)` (revision 3873625, `stonecutting`):
    ///   - "`ingredient`: The ingredient for the recipe. Can be one of an string ID, a string tag with `#`, or an list array containing IDs."
    ///   - "`result`: The output item of the recipe."
    ///
    /// Neither page states a vanilla ordering for the stonecutter output list;
    /// sorting by `recipe_id` is RustMC's deterministic order.
    #[must_use]
    pub fn stonecutting_results(&self, input_item: &str) -> Vec<(&str, ItemStack)> {
        let mut results: Vec<(&str, ItemStack)> = self
            .stonecutting
            .iter()
            .filter(|recipe| ingredient_matches_item(&recipe.ingredient, input_item))
            .map(|recipe| (recipe.id(), recipe.result().clone()))
            .collect();
        results.sort_by_key(|(id, _)| *id);
        results
    }

    /// Finds a loaded crafting recipe matching `grid`, returning `(recipe_id, result)`.
    ///
    /// Uses `world::crafting` matching (shaped recipes match at any offset and
    /// horizontally mirrored; shapeless recipes match in any order). If more
    /// than one recipe matches, returns the one with the lexicographically
    /// smallest recipe id. This is RustMC's deterministic tie-break, not a
    /// claim about vanilla Java Edition.
    #[must_use]
    pub fn find_crafting(&self, grid: &CraftingGrid) -> Option<(&str, ItemStack)> {
        self.crafting
            .iter()
            .filter(|recipe| recipe.matches(grid))
            .filter_map(|recipe| recipe.id().map(|id| (id, recipe)))
            .min_by(|(id_a, _), (id_b, _)| id_a.cmp(id_b))
            .map(|(id, recipe)| (id, recipe.output().clone()))
    }

    /// Loaded `minecraft:smithing_transform` recipes, sorted by recipe id.
    #[must_use]
    pub fn smithing_transform(&self) -> &[SmithingTransformRecipe] {
        &self.smithing_transform
    }

    /// Loaded `minecraft:smithing_trim` recipes, sorted by recipe id.
    #[must_use]
    pub fn smithing_trim(&self) -> &[SmithingTrimRecipe] {
        &self.smithing_trim
    }

    /// Finds a loaded `minecraft:smithing_transform` recipe by its namespaced id.
    #[must_use]
    pub fn get_smithing_transform_by_id(&self, id: &str) -> Option<&SmithingTransformRecipe> {
        self.smithing_transform
            .iter()
            .find(|recipe| recipe.id() == id)
    }

    /// Finds a loaded `minecraft:smithing_trim` recipe by its namespaced id.
    #[must_use]
    pub fn get_smithing_trim_by_id(&self, id: &str) -> Option<&SmithingTrimRecipe> {
        self.smithing_trim.iter().find(|recipe| recipe.id() == id)
    }

    /// Finds a loaded smithing transform or trim recipe matching `(template, base, addition)`,
    /// ignoring optional `minecraft:` prefixes.
    ///
    /// A trim result's `trim` component holds the trim material the addition item
    /// provides (minecraft.wiki `Data component format/trim` revision 3685746:
    /// "`material`: The ID of the trim material to apply"), read from the item report.
    /// Trim recipes loaded without item defaults ([`RecipeData::load`]) therefore never
    /// match; load them with [`RecipeData::load_with_items`].
    #[must_use]
    pub fn find_smithing(
        &self,
        template: &str,
        base: &str,
        addition: &str,
    ) -> Option<SmithingResult> {
        for recipe in &self.smithing_transform {
            if optional_ingredient_matches(recipe.template.as_ref(), template)
                && ingredient_matches_item(&recipe.base, base)
                && optional_ingredient_matches(recipe.addition.as_ref(), addition)
            {
                return Some(SmithingResult::Transform {
                    recipe_id: recipe.id.clone(),
                    result: recipe.result.clone(),
                });
            }
        }
        for recipe in &self.smithing_trim {
            if !optional_ingredient_matches(recipe.template.as_ref(), template) {
                continue;
            }
            let Some(base_stack) = recipe.base_stacks.get(normalize_item_id(base)) else {
                continue;
            };
            let Some(material) = recipe.materials.get(normalize_item_id(addition)) else {
                continue;
            };
            let Ok(trim) = ArmorTrim::try_new(recipe.pattern.as_str(), material.as_str()) else {
                continue;
            };
            let mut result = base_stack.clone();
            result.components_mut().set_trim(Some(trim));
            return Some(SmithingResult::Trim {
                recipe_id: recipe.id.clone(),
                result,
            });
        }
        None
    }
}

fn find_matching_ingredient_item<'a>(ingredient: &'a Ingredient, input: &str) -> Option<&'a str> {
    if input.is_empty() {
        return None;
    }
    ingredient
        .allowed_items()
        .iter()
        .find(|allowed| items_equal(allowed, input))
        .map(String::as_str)
}

fn ingredient_matches_item(ingredient: &Ingredient, input: &str) -> bool {
    find_matching_ingredient_item(ingredient, input).is_some()
}

fn optional_ingredient_matches(ingredient: Option<&Ingredient>, input: &str) -> bool {
    match ingredient {
        Some(ing) => ingredient_matches_item(ing, input),
        None => input.is_empty(),
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

fn build_result_stack(
    id: &str,
    count: u16,
    items: Option<&ItemDefaults>,
    file: &Path,
) -> Result<ItemStack, RecipeDataError> {
    let max_stack = match items {
        Some(defaults) => {
            let max = defaults
                .max_stack_size(id)
                .ok_or_else(|| invalid(file, format!("unknown result item `{id}`")))?;
            u16::from(max)
        }
        None => default_max_stack(id),
    };
    ItemStack::try_new(id, count, max_stack)
        .map_err(|error| invalid(file, format!("invalid `result` stack: {error}")))
}

fn parse_result(
    obj: &serde_json::Map<String, Value>,
    items: Option<&ItemDefaults>,
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
    let stack = build_result_stack(id, count, items, file)?;
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
    items: Option<&ItemDefaults>,
    file: &Path,
) -> Result<ParsedOrSkipped<CookingRecipe>, RecipeDataError> {
    let result = match parse_result(obj, items, file)? {
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
    items: Option<&ItemDefaults>,
    file: &Path,
) -> Result<ParsedOrSkipped<ShapedRecipe>, RecipeDataError> {
    let result = match parse_result(obj, items, file)? {
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
    let show_notification = parse_optional_bool(obj, "show_notification", file)?;

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
    items: Option<&ItemDefaults>,
    file: &Path,
) -> Result<ParsedOrSkipped<ShapelessRecipe>, RecipeDataError> {
    let result = match parse_result(obj, items, file)? {
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
    items: Option<&ItemDefaults>,
    file: &Path,
) -> Result<ParsedOrSkipped<StonecuttingRecipe>, RecipeDataError> {
    let result = match parse_result(obj, items, file)? {
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

fn reject_unknown_keys(
    obj: &serde_json::Map<String, Value>,
    allowed: &[&str],
    context: &str,
    file: &Path,
) -> Result<(), RecipeDataError> {
    if let Some(unknown) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(invalid(
            file,
            format!("unknown key `{unknown}` in {context}"),
        ));
    }
    Ok(())
}

fn parse_non_tag_id<'a>(
    obj: &'a serde_json::Map<String, Value>,
    key: &str,
    context: &str,
    file: &Path,
) -> Result<&'a str, RecipeDataError> {
    let val = obj
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .ok_or_else(|| invalid(file, format!("missing or invalid id `{context}`")))?;
    ItemStack::try_new(val, 1, 1)
        .map_err(|error| invalid(file, format!("invalid id in `{context}`: {error}")))?;
    Ok(val)
}

fn parse_potion_list(value: &Value, file: &Path) -> Result<Vec<String>, RecipeDataError> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut push_id = |raw: &str| -> Result<(), RecipeDataError> {
        if raw.is_empty() || raw.starts_with('#') {
            return Err(invalid(
                file,
                "brewing `input.potion_contents.potions` entries must be potion id strings",
            ));
        }
        ItemStack::try_new(raw, 1, 1).map_err(|error| {
            invalid(
                file,
                format!("invalid potion id `{raw}` in `input.potion_contents.potions`: {error}"),
            )
        })?;
        if seen.insert(raw.to_owned()) {
            out.push(raw.to_owned());
        }
        Ok(())
    };
    match value {
        Value::String(s) => push_id(s)?,
        Value::Array(items) => {
            if items.is_empty() {
                return Err(invalid(
                    file,
                    "`input.potion_contents.potions` list must not be empty",
                ));
            }
            for entry in items {
                let s = entry.as_str().ok_or_else(|| {
                    invalid(
                        file,
                        "`input.potion_contents.potions` list entries must be strings",
                    )
                })?;
                push_id(s)?;
            }
        }
        _ => {
            return Err(invalid(
                file,
                "`input.potion_contents.potions` must be a potion id string or list of strings",
            ));
        }
    }
    Ok(out)
}

fn parse_brewing_recipe(
    id: &str,
    obj: &serde_json::Map<String, Value>,
    tags: &ItemTags,
    items: Option<&ItemDefaults>,
    file: &Path,
) -> Result<BrewingRecipe, RecipeDataError> {
    reject_unknown_keys(
        obj,
        &["type", "input", "reagent", "output"],
        "brewing recipe",
        file,
    )?;

    let input_obj = obj
        .get("input")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(file, "missing or non-object `input` field"))?;
    reject_unknown_keys(
        input_obj,
        &["item", "potion_contents"],
        "brewing `input`",
        file,
    )?;
    let input_item = parse_non_tag_id(input_obj, "item", "input.item", file)?.to_owned();
    let input_pc_obj = input_obj
        .get("potion_contents")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(file, "missing or non-object `input.potion_contents`"))?;
    reject_unknown_keys(
        input_pc_obj,
        &["potions"],
        "brewing `input.potion_contents`",
        file,
    )?;
    let potions_val = input_pc_obj
        .get("potions")
        .ok_or_else(|| invalid(file, "missing `input.potion_contents.potions`"))?;
    let input_potions = parse_potion_list(potions_val, file)?;

    let reagent_obj = obj
        .get("reagent")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(file, "missing or non-object `reagent` field"))?;
    reject_unknown_keys(reagent_obj, &["item"], "brewing `reagent`", file)?;
    let reagent_val = reagent_obj
        .get("item")
        .ok_or_else(|| invalid(file, "missing `reagent.item` field"))?;
    let reagent = parse_ingredient(reagent_val, tags, file)?;

    let output_obj = obj
        .get("output")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(file, "missing or non-object `output` field"))?;
    reject_unknown_keys(output_obj, &["id", "components"], "brewing `output`", file)?;
    let output_id = parse_non_tag_id(output_obj, "id", "output.id", file)?;
    let output_comp_obj = output_obj
        .get("components")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(file, "missing or non-object `output.components`"))?;
    reject_unknown_keys(
        output_comp_obj,
        &["minecraft:potion_contents"],
        "brewing `output.components`",
        file,
    )?;
    let output_pc_obj = output_comp_obj
        .get("minecraft:potion_contents")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            invalid(
                file,
                "missing or non-object `output.components[\"minecraft:potion_contents\"]`",
            )
        })?;
    reject_unknown_keys(
        output_pc_obj,
        &["potion"],
        "brewing `output.components[\"minecraft:potion_contents\"]`",
        file,
    )?;
    let output_potion = parse_non_tag_id(
        output_pc_obj,
        "potion",
        "output.components[\"minecraft:potion_contents\"].potion",
        file,
    )?
    .to_owned();

    let mut output = build_result_stack(output_id, 1, items, file)?;
    output
        .components_mut()
        .set_potion(Some(output_potion))
        .map_err(|error| invalid(file, format!("invalid output potion component: {error}")))?;
    Ok(BrewingRecipe {
        id: id.to_owned(),
        input_item,
        input_potions,
        reagent,
        output,
    })
}

fn parse_optional_bool(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    file: &Path,
) -> Result<Option<bool>, RecipeDataError> {
    match obj.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| invalid(file, format!("`{key}` must be a boolean"))),
    }
}

fn parse_optional_ingredient(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    tags: &ItemTags,
    file: &Path,
) -> Result<Option<Ingredient>, RecipeDataError> {
    match obj.get(key) {
        None => Ok(None),
        Some(val) => parse_ingredient(val, tags, file).map(Some),
    }
}

fn parse_required_ingredient(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    tags: &ItemTags,
    file: &Path,
) -> Result<Ingredient, RecipeDataError> {
    let val = obj
        .get(key)
        .ok_or_else(|| invalid(file, format!("missing `{key}` field")))?;
    parse_ingredient(val, tags, file)
}

fn parse_smithing_transform_recipe(
    id: &str,
    obj: &serde_json::Map<String, Value>,
    tags: &ItemTags,
    items: Option<&ItemDefaults>,
    file: &Path,
) -> Result<ParsedOrSkipped<SmithingTransformRecipe>, RecipeDataError> {
    reject_unknown_keys(
        obj,
        &[
            "type",
            "template",
            "base",
            "addition",
            "result",
            "show_notification",
        ],
        "smithing_transform recipe",
        file,
    )?;
    parse_optional_bool(obj, "show_notification", file)?;
    let result = match parse_result(obj, items, file)? {
        ParsedOrSkipped::Skipped(reason) => return Ok(ParsedOrSkipped::Skipped(reason)),
        ParsedOrSkipped::Loaded(stack) => stack,
    };
    let template = parse_optional_ingredient(obj, "template", tags, file)?;
    let base = parse_required_ingredient(obj, "base", tags, file)?;
    let addition = parse_optional_ingredient(obj, "addition", tags, file)?;

    Ok(ParsedOrSkipped::Loaded(SmithingTransformRecipe {
        id: id.to_owned(),
        template,
        base,
        addition,
        result,
    }))
}

fn parse_smithing_trim_recipe(
    id: &str,
    obj: &serde_json::Map<String, Value>,
    tags: &ItemTags,
    items: Option<&ItemDefaults>,
    file: &Path,
) -> Result<SmithingTrimRecipe, RecipeDataError> {
    reject_unknown_keys(
        obj,
        &[
            "type",
            "template",
            "base",
            "addition",
            "pattern",
            "show_notification",
        ],
        "smithing_trim recipe",
        file,
    )?;
    parse_optional_bool(obj, "show_notification", file)?;
    let template = parse_optional_ingredient(obj, "template", tags, file)?;
    let base = parse_required_ingredient(obj, "base", tags, file)?;
    let addition = parse_required_ingredient(obj, "addition", tags, file)?;
    let pattern = parse_non_tag_id(obj, "pattern", "pattern", file)?.to_owned();

    let mut base_stacks = BTreeMap::new();
    for allowed in base.allowed_items() {
        let stack = build_result_stack(allowed, 1, items, file)?;
        base_stacks.insert(normalize_item_id(allowed).to_owned(), stack);
    }

    // The trim component stores the trim material id, which each addition item
    // declares in its `minecraft:provides_trim_material` component (item report),
    // not the item id. Without item defaults the materials are unknown, so the
    // recipe loads but cannot produce a result.
    let mut materials = BTreeMap::new();
    if let Some(items) = items {
        for allowed in addition.allowed_items() {
            let material = items.trim_material(allowed).ok_or_else(|| {
                invalid(
                    file,
                    format!(
                        "trim addition `{allowed}` provides no trim material in the item report"
                    ),
                )
            })?;
            materials.insert(normalize_item_id(allowed).to_owned(), material.to_owned());
        }
    }

    Ok(SmithingTrimRecipe {
        id: id.to_owned(),
        template,
        base,
        addition,
        pattern,
        base_stacks,
        materials,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::DEFAULT_MAX_STACK;
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
            &root.join("data/testns/recipe/transmute.json"),
            r#"{"type": "minecraft:crafting_transmute"}"#,
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
                ("testns:stew".to_owned(), SkipReason::ResultHasComponents),
                (
                    "testns:transmute".to_owned(),
                    SkipReason::UnsupportedType("minecraft:crafting_transmute".to_owned())
                ),
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

    #[test]
    fn find_crafting_matches_shaped_mirrored_shapeless_and_smallest_id_tie_break() {
        let root = scratch_root("find-crafting-tiebreak");
        write(
            &root.join("data/testns/recipe/b_shaped.json"),
            r###"{
                "type": "minecraft:crafting_shaped",
                "pattern": ["# ", "##"],
                "key": {"#": "testns:stone"},
                "result": {"id": "testns:second_result", "count": 2}
            }"###,
        );
        write(
            &root.join("data/testns/recipe/a_shapeless.json"),
            r#"{
                "type": "minecraft:crafting_shapeless",
                "ingredients": ["testns:stone", "testns:stone", "testns:stone"],
                "result": {"id": "testns:first_result", "count": 1}
            }"#,
        );
        write(
            &root.join("data/testns/recipe/c_only_shaped.json"),
            r#"{
                "type": "minecraft:crafting_shaped",
                "pattern": ["A", "B"],
                "key": {"A": "testns:top", "B": "testns:bottom"},
                "result": {"id": "testns:pair_out", "count": 4}
            }"#,
        );

        let data = RecipeData::load(&root, &ItemTags::default()).expect("load recipes");
        let stack = |id: &str| ItemStack::try_new(id, 1, 64).expect("stack");

        // Both `testns:a_shapeless` and `testns:b_shaped` match 3 `testns:stone` in an L-shape.
        let mut tie_grid = CraftingGrid::table_3x3();
        tie_grid.set_slot(2, 1, stack("testns:stone")).unwrap();
        tie_grid.set_slot(1, 2, stack("testns:stone")).unwrap();
        tie_grid.set_slot(2, 2, stack("testns:stone")).unwrap();
        let (matched_id, matched_out) = data.find_crafting(&tie_grid).expect("tie break match");
        assert_eq!(matched_id, "testns:a_shapeless");
        assert_eq!(matched_out.item(), "testns:first_result");
        assert_eq!(matched_out.count(), 1);

        // Offset placement in bottom-right of 3x3 grid matches shaped recipe.
        let mut pair_grid = CraftingGrid::table_3x3();
        pair_grid.set_slot(2, 1, stack("testns:top")).unwrap();
        pair_grid.set_slot(2, 2, stack("testns:bottom")).unwrap();
        let (pair_id, pair_out) = data.find_crafting(&pair_grid).expect("offset shaped match");
        assert_eq!(pair_id, "testns:c_only_shaped");
        assert_eq!(pair_out.item(), "testns:pair_out");
        assert_eq!(pair_out.count(), 4);

        // Unmatched grid returns None.
        let mut none_grid = CraftingGrid::table_3x3();
        none_grid.set_slot(0, 0, stack("testns:top")).unwrap();
        assert_eq!(data.find_crafting(&none_grid), None);

        fs::remove_dir_all(root).expect("cleanup");
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_crafting_lookup_matches_vanilla_recipes`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_crafting_lookup_matches_vanilla_recipes() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let tags = ItemTags::load(&root).expect("load vanilla item tags");
        let data = RecipeData::load(&root, &tags).expect("load vanilla recipes");

        let read_expected = |rel: &str| -> (String, u16) {
            let doc = read_json(&root.join(rel)).expect("read recipe json");
            let id = doc["result"]["id"].as_str().expect("result id").to_owned();
            let count = doc["result"]
                .get("count")
                .and_then(Value::as_u64)
                .unwrap_or(1) as u16;
            (id, count)
        };

        let (torch_item, torch_count) = read_expected("data/minecraft/recipe/torch.json");
        let (planks_item, planks_count) = read_expected("data/minecraft/recipe/oak_planks.json");
        let (stairs_item, stairs_count) = read_expected("data/minecraft/recipe/stone_stairs.json");
        let stack = |id: &str| ItemStack::try_new(id, 1, 64).expect("stack");

        // 1. Coal above stick in the bottom-right corner of 3x3 table grid.
        let mut coal_torch = CraftingGrid::table_3x3();
        coal_torch.set_slot(2, 1, stack("minecraft:coal")).unwrap();
        coal_torch.set_slot(2, 2, stack("minecraft:stick")).unwrap();
        let (id, out) = data.find_crafting(&coal_torch).expect("coal torch");
        assert_eq!(id, "minecraft:torch");
        assert_eq!(out.item(), torch_item);
        assert_eq!(out.count(), torch_count);
        assert_eq!(out.count(), 4);

        // 2. Charcoal above stick in 3x3 table grid.
        let mut charcoal_torch = CraftingGrid::table_3x3();
        charcoal_torch
            .set_slot(0, 0, stack("minecraft:charcoal"))
            .unwrap();
        charcoal_torch
            .set_slot(0, 1, stack("minecraft:stick"))
            .unwrap();
        let (id, out) = data.find_crafting(&charcoal_torch).expect("charcoal torch");
        assert_eq!(id, "minecraft:torch");
        assert_eq!(out.item(), torch_item);
        assert_eq!(out.count(), torch_count);

        // 3. One stripped_oak_wood in 2x2 inventory grid -> oak_planks.
        let mut inv_planks = CraftingGrid::inventory_2x2();
        inv_planks
            .set_slot(1, 0, stack("minecraft:stripped_oak_wood"))
            .unwrap();
        let (id, out) = data.find_crafting(&inv_planks).expect("oak planks");
        assert_eq!(id, "minecraft:oak_planks");
        assert_eq!(out.item(), planks_item);
        assert_eq!(out.count(), planks_count);
        assert_eq!(out.count(), 4);

        // 4. Stone in mirrored stair shape ("  #", " ##", "###") -> stone_stairs.
        let mut stairs = CraftingGrid::table_3x3();
        stairs.set_slot(2, 0, stack("minecraft:stone")).unwrap();
        stairs.set_slot(1, 1, stack("minecraft:stone")).unwrap();
        stairs.set_slot(2, 1, stack("minecraft:stone")).unwrap();
        stairs.set_slot(0, 2, stack("minecraft:stone")).unwrap();
        stairs.set_slot(1, 2, stack("minecraft:stone")).unwrap();
        stairs.set_slot(2, 2, stack("minecraft:stone")).unwrap();
        let (id, out) = data.find_crafting(&stairs).expect("mirrored stone stairs");
        assert_eq!(id, "minecraft:stone_stairs");
        assert_eq!(out.item(), stairs_item);
        assert_eq!(out.count(), stairs_count);
        assert_eq!(out.count(), 4);

        // 5. A single stick -> no result.
        let mut single_stick = CraftingGrid::table_3x3();
        single_stick
            .set_slot(1, 1, stack("minecraft:stick"))
            .unwrap();
        assert_eq!(data.find_crafting(&single_stick), None);
    }

    #[test]
    fn load_with_items_uses_item_defaults_and_rejects_unknown_result() {
        let root = scratch_root("with-items");
        write(
            &root.join("reports/minecraft/components/item/widget_sign.json"),
            r#"{"components": {"minecraft:max_stack_size": 16}}"#,
        );
        write(
            &root.join("data/testns/recipe/make_sign.json"),
            r#"{
                "type": "minecraft:crafting_shapeless",
                "ingredients": ["testns:plank"],
                "result": {"id": "minecraft:widget_sign", "count": 3}
            }"#,
        );

        let items = ItemDefaults::load(&root).expect("load item defaults");
        let tags = ItemTags::default();
        let data = RecipeData::load_with_items(&root, &tags, &items).expect("load with items");
        let recipe = data
            .get_crafting_by_id("testns:make_sign")
            .expect("make_sign recipe");
        assert_eq!(recipe.output().count(), 3);
        assert_eq!(recipe.output().max_stack(), 16);

        // Fallback load still uses default_max_stack (64 for unknown custom items).
        let fallback = RecipeData::load(&root, &tags).expect("fallback load");
        assert_eq!(
            fallback
                .get_crafting_by_id("testns:make_sign")
                .expect("fallback recipe")
                .output()
                .max_stack(),
            64
        );

        // Unknown result item fails with an error naming the recipe file.
        write(
            &root.join("data/testns/recipe/bad_result.json"),
            r#"{
                "type": "minecraft:stonecutting",
                "ingredient": "testns:plank",
                "result": {"id": "testns:unknown_widget"}
            }"#,
        );
        let err = RecipeData::load_with_items(&root, &tags, &items)
            .expect_err("unknown result item must fail")
            .to_string();
        assert!(
            err.contains("bad_result.json"),
            "error must name recipe file: {err}"
        );
        assert!(
            err.contains("testns:unknown_widget"),
            "error must name unknown item: {err}"
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_recipes_load_with_items_pin_stack_sizes`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_recipes_load_with_items_pin_stack_sizes() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let tags = ItemTags::load(&root).expect("load vanilla item tags");
        let items = ItemDefaults::load(&root).expect("load vanilla item defaults");
        let data = RecipeData::load_with_items(&root, &tags, &items).expect("load recipes");

        let raw_iron = data
            .get_cooking_by_id("minecraft:iron_ingot_from_smelting_raw_iron")
            .expect("raw_iron smelting recipe");
        assert_eq!(raw_iron.result().max_stack(), 64);

        for kind in CookingKind::ALL {
            for recipe in data.cooking(kind) {
                let expected = items
                    .max_stack_size(recipe.result().item())
                    .expect("cooking result item in ItemDefaults");
                assert_eq!(
                    recipe.result().max_stack(),
                    u16::from(expected),
                    "cooking recipe {}",
                    recipe.id()
                );
            }
        }
        for recipe in data.crafting() {
            let expected = items
                .max_stack_size(recipe.output().item())
                .expect("crafting result item in ItemDefaults");
            assert_eq!(
                recipe.output().max_stack(),
                u16::from(expected),
                "crafting recipe {:?}",
                recipe.id()
            );
        }
        for recipe in data.stonecutting() {
            let expected = items
                .max_stack_size(recipe.result().item())
                .expect("stonecutting result item in ItemDefaults");
            assert_eq!(
                recipe.result().max_stack(),
                u16::from(expected),
                "stonecutting recipe {}",
                recipe.id()
            );
        }
        for recipe in data.brewing() {
            let expected = items
                .max_stack_size(recipe.output().item())
                .expect("brewing result item in ItemDefaults");
            assert_eq!(
                recipe.output().max_stack(),
                u16::from(expected),
                "brewing recipe {}",
                recipe.id()
            );
        }
    }

    #[test]
    fn loads_brewing_recipes_supports_lists_and_tags_and_finds_by_triple() {
        let root = scratch_root("brewing-parse-lookup");
        write(
            &root.join("data/testns/tags/item/reagents.json"),
            r#"{"values": ["testns:herb_a", "testns:herb_b"]}"#,
        );
        write(
            &root.join("data/testns/recipe/brewing/single.json"),
            r#"{
                "type": "minecraft:brewing",
                "input": {
                    "item": "testns:bottle",
                    "potion_contents": {"potions": "testns:base"}
                },
                "reagent": {"item": "testns:catalyst"},
                "output": {
                    "id": "testns:splash_bottle",
                    "components": {
                        "minecraft:potion_contents": {"potion": "testns:charged"}
                    }
                }
            }"#,
        );
        write(
            &root.join("data/testns/recipe/brewing/multi.json"),
            r##"{
                "type": "minecraft:brewing",
                "input": {
                    "item": "minecraft:potion",
                    "potion_contents": {"potions": ["testns:tonic", "testns:elixir"]}
                },
                "reagent": {"item": "#testns:reagents"},
                "output": {
                    "id": "minecraft:splash_potion",
                    "components": {
                        "minecraft:potion_contents": {"potion": "testns:upgraded"}
                    }
                }
            }"##,
        );

        let tags = ItemTags::load(&root).expect("load tags");
        let data = RecipeData::load(&root, &tags).expect("load brewing recipes");
        assert_eq!(data.brewing().len(), 2);
        assert!(data.skipped().is_empty());

        let single = data
            .find_brewing("testns:bottle", "testns:base", "testns:catalyst")
            .expect("single recipe match");
        assert_eq!(single.id(), "testns:brewing/single");
        assert_eq!(single.output_item(), "testns:splash_bottle");
        assert_eq!(single.output_potion(), "testns:charged");

        // `minecraft:` prefixes are optional for lookup.
        let multi = data
            .find_brewing("potion", "testns:elixir", "testns:herb_b")
            .expect("multi recipe match");
        assert_eq!(multi.id(), "testns:brewing/multi");
        assert_eq!(multi.output_item(), "minecraft:splash_potion");
        assert_eq!(multi.output_potion(), "testns:upgraded");

        assert!(
            data.find_brewing("minecraft:splash_potion", "testns:elixir", "testns:herb_b")
                .is_none()
        );
        assert!(
            data.find_brewing("minecraft:potion", "testns:other", "testns:herb_b")
                .is_none()
        );
        assert!(
            data.find_brewing("minecraft:potion", "testns:elixir", "testns:other")
                .is_none()
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn duplicate_brewing_triple_errors_naming_both_recipe_ids() {
        let root = scratch_root("brewing-dup");
        write(
            &root.join("data/testns/recipe/brewing/first.json"),
            r#"{
                "type": "minecraft:brewing",
                "input": {
                    "item": "testns:bottle",
                    "potion_contents": {"potions": "testns:base"}
                },
                "reagent": {"item": "testns:catalyst"},
                "output": {
                    "id": "testns:bottle",
                    "components": {"minecraft:potion_contents": {"potion": "testns:out_a"}}
                }
            }"#,
        );
        write(
            &root.join("data/testns/recipe/brewing/second.json"),
            r#"{
                "type": "minecraft:brewing",
                "input": {
                    "item": "testns:bottle",
                    "potion_contents": {"potions": ["testns:other", "testns:base"]}
                },
                "reagent": {"item": "testns:catalyst"},
                "output": {
                    "id": "testns:bottle",
                    "components": {"minecraft:potion_contents": {"potion": "testns:out_b"}}
                }
            }"#,
        );

        let err = RecipeData::load(&root, &ItemTags::default())
            .expect_err("duplicate brewing triple must fail")
            .to_string();
        assert!(err.contains("testns:brewing/first"), "{err}");
        assert!(err.contains("testns:brewing/second"), "{err}");

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn unknown_brewing_shape_errors_naming_file() {
        let cases = [
            (
                "extra_top_key.json",
                r#"{
                    "type": "minecraft:brewing",
                    "extra": 1,
                    "input": {"item": "testns:b", "potion_contents": {"potions": "testns:p"}},
                    "reagent": {"item": "testns:r"},
                    "output": {"id": "testns:b", "components": {"minecraft:potion_contents": {"potion": "testns:o"}}}
                }"#,
            ),
            (
                "bad_reagent_shape.json",
                r#"{
                    "type": "minecraft:brewing",
                    "input": {"item": "testns:b", "potion_contents": {"potions": "testns:p"}},
                    "reagent": {"tag": "testns:r"},
                    "output": {"id": "testns:b", "components": {"minecraft:potion_contents": {"potion": "testns:o"}}}
                }"#,
            ),
            (
                "missing_output_potion.json",
                r#"{
                    "type": "minecraft:brewing",
                    "input": {"item": "testns:b", "potion_contents": {"potions": "testns:p"}},
                    "reagent": {"item": "testns:r"},
                    "output": {"id": "testns:b", "components": {"minecraft:potion_contents": {}}}
                }"#,
            ),
        ];

        for (name, body) in cases {
            let root = scratch_root(name);
            write(&root.join("data/testns/recipe/brewing").join(name), body);
            let err = RecipeData::load(&root, &ItemTags::default())
                .expect_err("unknown brewing shape must fail")
                .to_string();
            assert!(err.contains(name), "error must name {name}: {err}");
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_brewing_recipes_load_and_lookup`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_brewing_recipes_load_and_lookup() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let tags = ItemTags::load(&root).expect("load vanilla item tags");
        let items = ItemDefaults::load(&root).expect("load vanilla item defaults");
        let data = RecipeData::load_with_items(&root, &tags, &items).expect("load recipes");

        assert_eq!(data.brewing().len(), 279);
        assert!(
            !data
                .skipped()
                .iter()
                .any(|(id, reason)| id.starts_with("minecraft:brewing/")
                    || *reason == SkipReason::UnsupportedType("minecraft:brewing".to_owned())),
            "no brewing recipes may be skipped"
        );

        let strength = data
            .find_brewing(
                "minecraft:potion",
                "minecraft:awkward",
                "minecraft:blaze_powder",
            )
            .expect("potion + awkward + blaze_powder recipe");
        assert_eq!(strength.output_item(), "minecraft:potion");
        assert_eq!(strength.output_potion(), "minecraft:strength");

        let gunpowder_path =
            root.join("data/minecraft/recipe/brewing/potion_awkward_gunpowder.json");
        if gunpowder_path.exists() {
            let doc = read_json(&gunpowder_path).expect("read potion_awkward_gunpowder.json");
            let expected_item = doc["output"]["id"].as_str().expect("output id");
            let expected_potion =
                doc["output"]["components"]["minecraft:potion_contents"]["potion"]
                    .as_str()
                    .expect("output potion");
            let splash = data
                .find_brewing(
                    "minecraft:potion",
                    "minecraft:awkward",
                    "minecraft:gunpowder",
                )
                .expect("potion + awkward + gunpowder recipe");
            assert_eq!(splash.output_item(), expected_item);
            assert_eq!(splash.output_potion(), expected_potion);
            assert_eq!(splash.output_item(), "minecraft:splash_potion");
            assert_eq!(splash.output_potion(), "minecraft:awkward");
        }
    }

    #[test]
    fn loads_smithing_transform_and_trim_resolves_tags_and_matches() {
        let root = scratch_root("smithing");
        write(
            &root.join("data/testns/tags/item/upgrade_mats.json"),
            r#"{"values": ["testns:alloy"]}"#,
        );
        write(
            &root.join("data/testns/tags/item/armors.json"),
            r#"{"values": ["minecraft:bronze_plate", "minecraft:iron_chestplate"]}"#,
        );
        write(
            &root.join("data/testns/tags/item/trim_mats.json"),
            r#"{"values": ["minecraft:ruby_gem", "minecraft:sapphire_gem"]}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/ruby_gem.json"),
            r#"{"components": {"minecraft:max_stack_size": 64, "minecraft:provides_trim_material": "testns:ruby"}}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/sapphire_gem.json"),
            r#"{"components": {"minecraft:max_stack_size": 64, "minecraft:provides_trim_material": "testns:sapphire"}}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/bronze_plate.json"),
            r#"{"components": {"minecraft:max_stack_size": 1}}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/iron_chestplate.json"),
            r#"{"components": {"minecraft:max_stack_size": 1}}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/mythril_blade.json"),
            r#"{"components": {"minecraft:max_stack_size": 1}}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/free_result.json"),
            r#"{"components": {"minecraft:max_stack_size": 16}}"#,
        );
        write(
            &root.join("data/testns/recipe/upgrade_blade.json"),
            r##"{
                "type": "minecraft:smithing_transform",
                "template": "testns:forge_template",
                "base": "testns:steel_blade",
                "addition": "#testns:upgrade_mats",
                "result": {"id": "minecraft:mythril_blade"}
            }"##,
        );
        write(
            &root.join("data/testns/recipe/templateless_upgrade.json"),
            r#"{
                "type": "minecraft:smithing_transform",
                "base": "testns:raw_core",
                "result": {"id": "minecraft:free_result", "count": 2},
                "show_notification": false
            }"#,
        );
        write(
            &root.join("data/testns/recipe/wave_trim.json"),
            r##"{
                "type": "minecraft:smithing_trim",
                "template": "testns:wave_template",
                "base": "#testns:armors",
                "addition": "#testns:trim_mats",
                "pattern": "testns:wave"
            }"##,
        );
        write(
            &root.join("data/testns/recipe/component_transform.json"),
            r#"{
                "type": "minecraft:smithing_transform",
                "template": "testns:t",
                "base": "testns:b",
                "addition": "testns:a",
                "result": {"id": "minecraft:mythril_blade", "components": {"minecraft:enchantments": {}}}
            }"#,
        );

        let tags = ItemTags::load(&root).expect("load synthetic item tags");
        let items = ItemDefaults::load(&root).expect("load synthetic item defaults");
        let data = RecipeData::load(&root, &tags).expect("load synthetic recipes");

        assert_eq!(data.smithing_transform().len(), 2);
        assert_eq!(data.smithing_trim().len(), 1);
        assert_eq!(
            data.skipped(),
            &[(
                "testns:component_transform".to_owned(),
                SkipReason::ResultHasComponents
            )]
        );

        // Transform match.
        let upgrade = data
            .find_smithing(
                "testns:forge_template",
                "testns:steel_blade",
                "testns:alloy",
            )
            .expect("upgrade match");
        assert_eq!(
            upgrade,
            SmithingResult::Transform {
                recipe_id: "testns:upgrade_blade".to_owned(),
                result: ItemStack::try_new("minecraft:mythril_blade", 1, 64).unwrap(),
            }
        );

        // Optional template/addition omitted only match empty inputs.
        assert_eq!(
            data.find_smithing("", "testns:raw_core", ""),
            Some(SmithingResult::Transform {
                recipe_id: "testns:templateless_upgrade".to_owned(),
                result: ItemStack::try_new("minecraft:free_result", 2, 64).unwrap(),
            })
        );
        assert_eq!(
            data.find_smithing("testns:forge_template", "testns:raw_core", ""),
            None
        );

        // Without item defaults the trim materials are unknown, so trims never match.
        assert_eq!(
            data.find_smithing("testns:wave_template", "iron_chestplate", "ruby_gem"),
            None
        );

        // Also check ItemDefaults max_stack_size propagation via load_with_items.
        let data_with_items = RecipeData::load_with_items(&root, &tags, &items)
            .expect("load synthetic recipes with items");
        let with_items_trim = data_with_items
            .find_smithing(
                "testns:wave_template",
                "minecraft:iron_chestplate",
                "sapphire_gem",
            )
            .expect("trim with items");
        let SmithingResult::Trim { recipe_id, result } = with_items_trim else {
            panic!("expected Trim result");
        };
        assert_eq!(recipe_id, "testns:wave_trim");
        assert_eq!(result.item(), "minecraft:iron_chestplate");
        assert_eq!(result.count(), 1);
        assert_eq!(result.max_stack(), 1);
        assert_eq!(
            result.components().trim(),
            Some(&ArmorTrim::try_new("testns:wave", "testns:sapphire").unwrap())
        );
        assert_eq!(
            data_with_items
                .find_smithing("testns:wave_template", "iron_chestplate", "ruby_gem")
                .and_then(|found| match found {
                    SmithingResult::Trim { result, .. } => result.components().trim().cloned(),
                    SmithingResult::Transform { .. } => None,
                }),
            Some(ArmorTrim::try_new("testns:wave", "testns:ruby").unwrap())
        );

        // Wrong template/base/addition -> None.
        assert_eq!(
            data_with_items.find_smithing("testns:wrong", "iron_chestplate", "ruby_gem"),
            None
        );
        assert_eq!(
            data_with_items.find_smithing("testns:wave_template", "testns:not_armor", "ruby_gem"),
            None
        );
        assert_eq!(
            data_with_items.find_smithing(
                "testns:wave_template",
                "iron_chestplate",
                "testns:stone"
            ),
            None
        );
        let with_items_upgrade = data_with_items
            .find_smithing(
                "testns:forge_template",
                "testns:steel_blade",
                "testns:alloy",
            )
            .expect("upgrade with items");
        assert_eq!(
            with_items_upgrade,
            SmithingResult::Transform {
                recipe_id: "testns:upgrade_blade".to_owned(),
                result: ItemStack::try_new("minecraft:mythril_blade", 1, 1).unwrap(),
            }
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn smithing_rejects_malformed_shapes_naming_file() {
        let cases = [
            (
                "missing_base.json",
                r#"{"type": "minecraft:smithing_transform", "template": "testns:t", "addition": "testns:a", "result": {"id": "testns:out"}}"#,
            ),
            (
                "bad_template.json",
                r#"{"type": "minecraft:smithing_transform", "template": 42, "base": "testns:b", "result": {"id": "testns:out"}}"#,
            ),
            (
                "unknown_transform_key.json",
                r#"{"type": "minecraft:smithing_transform", "base": "testns:b", "result": {"id": "testns:out"}, "extra": 1}"#,
            ),
            (
                "missing_pattern.json",
                r#"{"type": "minecraft:smithing_trim", "template": "testns:t", "base": "testns:b", "addition": "testns:a"}"#,
            ),
            (
                "missing_trim_addition.json",
                r#"{"type": "minecraft:smithing_trim", "template": "testns:t", "base": "testns:b", "pattern": "testns:p"}"#,
            ),
            (
                "bad_pattern_tag.json",
                r##"{"type": "minecraft:smithing_trim", "template": "testns:t", "base": "testns:b", "addition": "testns:a", "pattern": "#testns:p"}"##,
            ),
            (
                "unknown_trim_key.json",
                r#"{"type": "minecraft:smithing_trim", "template": "testns:t", "base": "testns:b", "addition": "testns:a", "pattern": "testns:p", "extra": true}"#,
            ),
        ];

        for (name, body) in cases {
            let root = scratch_root(name);
            write(&root.join("data/testns/recipe").join(name), body);
            let err = RecipeData::load(&root, &ItemTags::default())
                .expect_err("malformed smithing recipe must fail")
                .to_string();
            assert!(err.contains(name), "error must name {name}: {err}");
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_smithing_recipes_load_and_lookup`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_smithing_recipes_load_and_lookup() {
        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let tags = ItemTags::load(&root).expect("load vanilla item tags");
        let items = ItemDefaults::load(&root).expect("load vanilla item defaults");
        let data = RecipeData::load_with_items(&root, &tags, &items).expect("load recipes");

        assert_eq!(data.smithing_transform().len(), 12);
        assert_eq!(data.smithing_trim().len(), 18);
        assert!(
            !data.skipped().iter().any(|(_, reason)| {
                *reason == SkipReason::UnsupportedType("minecraft:smithing_transform".to_owned())
                    || *reason == SkipReason::UnsupportedType("minecraft:smithing_trim".to_owned())
            }),
            "no smithing recipes may be skipped"
        );

        let netherite_sword = data
            .find_smithing(
                "minecraft:netherite_upgrade_smithing_template",
                "minecraft:diamond_sword",
                "minecraft:netherite_ingot",
            )
            .expect("netherite sword smithing transform");
        assert_eq!(
            netherite_sword,
            SmithingResult::Transform {
                recipe_id: "minecraft:netherite_sword_smithing".to_owned(),
                result: ItemStack::try_new("minecraft:netherite_sword", 1, 1).unwrap(),
            }
        );

        assert!(
            tags.contains("minecraft:trim_materials", "minecraft:gold_ingot"),
            "gold_ingot must be in #minecraft:trim_materials"
        );
        let coast_trim = data
            .find_smithing(
                "minecraft:coast_armor_trim_smithing_template",
                "minecraft:iron_chestplate",
                "minecraft:gold_ingot",
            )
            .expect("coast armor trim with iron chestplate and gold ingot");
        let SmithingResult::Trim { recipe_id, result } = coast_trim else {
            panic!("expected Trim result");
        };
        assert_eq!(
            recipe_id,
            "minecraft:coast_armor_trim_smithing_template_smithing_trim"
        );
        assert_eq!(result.item(), "minecraft:iron_chestplate");
        assert_eq!(result.count(), 1);
        assert_eq!(result.max_stack(), 1);
        assert_eq!(
            result.components().trim(),
            Some(&ArmorTrim::try_new("minecraft:coast", "minecraft:gold").unwrap())
        );

        // Wrong template returns None.
        assert_eq!(
            data.find_smithing(
                "minecraft:coast_armor_trim_smithing_template",
                "minecraft:diamond_sword",
                "minecraft:netherite_ingot",
            ),
            None
        );
        assert_eq!(
            data.find_smithing(
                "minecraft:netherite_upgrade_smithing_template",
                "minecraft:iron_chestplate",
                "minecraft:gold_ingot",
            ),
            None
        );
        assert_eq!(
            data.find_smithing(
                "minecraft:stick",
                "minecraft:diamond_sword",
                "minecraft:netherite_ingot",
            ),
            None
        );
    }

    #[test]
    fn smithing_trim_result_carries_trim_component_and_prevents_mismatched_stacking() {
        let root = scratch_root("smithing-trim-comp");
        write(
            &root.join("data/testns/tags/item/armors.json"),
            r#"{"values": ["minecraft:iron_chestplate"]}"#,
        );
        write(
            &root.join("data/testns/tags/item/trim_mats.json"),
            r#"{"values": ["minecraft:gold_ingot", "minecraft:diamond"]}"#,
        );
        write(
            &root.join("data/testns/recipe/coast_trim.json"),
            r##"{
                "type": "minecraft:smithing_trim",
                "template": "minecraft:coast_armor_trim_smithing_template",
                "base": "#testns:armors",
                "addition": "#testns:trim_mats",
                "pattern": "minecraft:coast"
            }"##,
        );
        write(
            &root.join("reports/minecraft/components/item/iron_chestplate.json"),
            r#"{"components": {"minecraft:max_stack_size": 1}}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/gold_ingot.json"),
            r#"{"components": {"minecraft:max_stack_size": 64, "minecraft:provides_trim_material": "minecraft:gold"}}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/diamond.json"),
            r#"{"components": {"minecraft:max_stack_size": 64, "minecraft:provides_trim_material": "minecraft:diamond"}}"#,
        );

        let tags = ItemTags::load(&root).expect("load tags");
        let items = ItemDefaults::load(&root).expect("load item defaults");
        let data = RecipeData::load_with_items(&root, &tags, &items).expect("load recipes");

        let SmithingResult::Trim {
            result: gold_trimmed,
            ..
        } = data
            .find_smithing(
                "minecraft:coast_armor_trim_smithing_template",
                "minecraft:iron_chestplate",
                "minecraft:gold_ingot",
            )
            .expect("gold trim")
        else {
            panic!("expected Trim result");
        };
        let SmithingResult::Trim {
            result: diamond_trimmed,
            ..
        } = data
            .find_smithing(
                "minecraft:coast_armor_trim_smithing_template",
                "minecraft:iron_chestplate",
                "minecraft:diamond",
            )
            .expect("diamond trim")
        else {
            panic!("expected Trim result");
        };
        let untrimmed = ItemStack::try_new("minecraft:iron_chestplate", 1, 1).unwrap();

        assert_eq!(
            gold_trimmed.components().trim(),
            Some(&ArmorTrim::try_new("minecraft:coast", "minecraft:gold").unwrap())
        );
        assert_eq!(
            diamond_trimmed.components().trim(),
            Some(&ArmorTrim::try_new("minecraft:coast", "minecraft:diamond").unwrap())
        );
        assert!(!gold_trimmed.can_stack_with(&untrimmed));
        assert!(!gold_trimmed.can_stack_with(&diamond_trimmed));
        assert!(gold_trimmed.can_stack_with(&gold_trimmed.clone()));

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn smithing_trim_addition_without_trim_material_is_a_load_error() {
        let root = scratch_root("smithing-trim-no-material");
        write(
            &root.join("data/testns/recipe/coast_trim.json"),
            r#"{
                "type": "minecraft:smithing_trim",
                "template": "minecraft:coast_armor_trim_smithing_template",
                "base": "minecraft:iron_chestplate",
                "addition": "minecraft:stick",
                "pattern": "minecraft:coast"
            }"#,
        );
        write(
            &root.join("reports/minecraft/components/item/iron_chestplate.json"),
            r#"{"components": {"minecraft:max_stack_size": 1}}"#,
        );
        write(
            &root.join("reports/minecraft/components/item/stick.json"),
            r#"{"components": {"minecraft:max_stack_size": 64}}"#,
        );
        let tags = ItemTags::load(&root).expect("load tags");
        let items = ItemDefaults::load(&root).expect("load item defaults");
        let error = RecipeData::load_with_items(&root, &tags, &items)
            .expect_err("addition without a trim material must fail")
            .to_string();
        assert!(error.contains("coast_trim.json"), "{error}");
        assert!(error.contains("minecraft:stick"), "{error}");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn stonecutting_results_filters_and_sorts_by_recipe_id() {
        let root = scratch_root("stonecutting-results");
        write(
            &root.join("data/testns/tags/item/cuttable_stones.json"),
            r#"{"values": ["minecraft:stone", "testns:granite"]}"#,
        );
        write(
            &root.join("data/testns/recipe/z_slab.json"),
            r##"{
                "type": "minecraft:stonecutting",
                "ingredient": "#testns:cuttable_stones",
                "result": {"id": "testns:cut_slab", "count": 2}
            }"##,
        );
        write(
            &root.join("data/testns/recipe/a_stairs.json"),
            r#"{
                "type": "minecraft:stonecutting",
                "ingredient": "minecraft:stone",
                "result": {"id": "testns:cut_stairs"}
            }"#,
        );
        write(
            &root.join("data/testns/recipe/m_wall.json"),
            r#"{
                "type": "minecraft:stonecutting",
                "ingredient": ["testns:other", "minecraft:stone"],
                "result": {"id": "testns:cut_wall"}
            }"#,
        );
        write(
            &root.join("data/testns/recipe/other_only.json"),
            r#"{
                "type": "minecraft:stonecutting",
                "ingredient": "testns:other",
                "result": {"id": "testns:other_slab", "count": 2}
            }"#,
        );

        let tags = ItemTags::load(&root).expect("load tags");
        let data = RecipeData::load(&root, &tags).expect("load recipes");

        let expected = vec![
            (
                "testns:a_stairs",
                ItemStack::try_new("testns:cut_stairs", 1, DEFAULT_MAX_STACK).unwrap(),
            ),
            (
                "testns:m_wall",
                ItemStack::try_new("testns:cut_wall", 1, DEFAULT_MAX_STACK).unwrap(),
            ),
            (
                "testns:z_slab",
                ItemStack::try_new("testns:cut_slab", 2, DEFAULT_MAX_STACK).unwrap(),
            ),
        ];
        assert_eq!(data.stonecutting_results("minecraft:stone"), expected);
        assert_eq!(data.stonecutting_results("stone"), expected);
        assert!(data.stonecutting_results("minecraft:dirt").is_empty());
        assert!(data.stonecutting_results("").is_empty());

        fs::remove_dir_all(root).expect("cleanup");
    }

    /// Run with:
    /// `RUSTMC_VANILLA_DATA=<root> cargo test -p rustmc-server --lib -- --ignored pack_stonecutting_results_match_vanilla_recipes`
    #[test]
    #[ignore = "requires operator-provisioned local data"]
    fn pack_stonecutting_results_match_vanilla_recipes() {
        // Number of vanilla Java 26.3 `minecraft:stonecutting` recipe files
        // under `data/minecraft/recipe` whose ingredient accepts `minecraft:stone`.
        const VANILLA_STONE_STONECUTTING_COUNT: usize = 11;
        // Slab output count per block cut in a stonecutter
        // (minecraft.wiki `Stonecutter` revision 3892466: "Andesite -> Andesite Slab, 2").
        const STONECUTTER_SLAB_COUNT: u16 = 2;

        let root = std::env::var("RUSTMC_VANILLA_DATA")
            .map(PathBuf::from)
            .expect("set RUSTMC_VANILLA_DATA to the provisioned 26.3 data root");
        let tags = ItemTags::load(&root).expect("load vanilla item tags");
        let items = ItemDefaults::load(&root).expect("load vanilla item defaults");
        let data = RecipeData::load_with_items(&root, &tags, &items).expect("load recipes");

        let recipe_dir = root.join("data/minecraft/recipe");
        let mut expected_stone: Vec<(String, ItemStack)> = Vec::new();
        for entry in walk_json(&recipe_dir).expect("walk vanilla recipes") {
            let doc = read_json(&entry).expect("read recipe json");
            if doc.get("type").and_then(Value::as_str) != Some("minecraft:stonecutting") {
                continue;
            }
            let ing = parse_ingredient(&doc["ingredient"], &tags, &entry).expect("ingredient");
            if !ingredient_matches_item(&ing, "minecraft:stone") {
                continue;
            }
            let id = recipe_id("minecraft", &recipe_dir, &entry);
            let ParsedOrSkipped::Loaded(stack) =
                parse_result(doc.as_object().unwrap(), Some(&items), &entry).expect("result")
            else {
                continue;
            };
            expected_stone.push((id, stack));
        }
        expected_stone.sort_by(|(id_a, _), (id_b, _)| id_a.cmp(id_b));

        assert_eq!(expected_stone.len(), VANILLA_STONE_STONECUTTING_COUNT);

        let actual_stone = data.stonecutting_results("minecraft:stone");
        assert_eq!(actual_stone.len(), VANILLA_STONE_STONECUTTING_COUNT);
        let expected_refs: Vec<(&str, ItemStack)> = expected_stone
            .iter()
            .map(|(id, stack)| (id.as_str(), stack.clone()))
            .collect();
        assert_eq!(actual_stone, expected_refs);
        assert_eq!(data.stonecutting_results("stone"), expected_refs);

        let andesite_results = data.stonecutting_results("minecraft:andesite");
        let expected_andesite_slab = ItemStack::try_new(
            "minecraft:andesite_slab",
            STONECUTTER_SLAB_COUNT,
            u16::from(
                items
                    .max_stack_size("minecraft:andesite_slab")
                    .expect("andesite_slab max stack"),
            ),
        )
        .expect("andesite_slab stack");
        assert!(
            andesite_results.contains(&(
                "minecraft:andesite_slab_from_andesite_stonecutting",
                expected_andesite_slab
            )),
            "andesite stonecutting results must include andesite_slab x2: {andesite_results:?}"
        );

        assert!(data.stonecutting_results("minecraft:dirt").is_empty());
    }
}
