//! Milestone M4: 2x2 and 3x3 Crafting Recipe Grid Evaluator.
//!
//! Provides authoritative crafting grid representation, bounding box normalization,
//! shaped and shapeless recipe matching (with horizontal symmetry mirroring), recipe
//! registry query, and ingredient consumption with crafting residual preservation.

use std::fmt;

/// Maximum allowable grid dimension (width or height) to prevent unbounded memory allocation.
pub const MAX_GRID_DIMENSION: usize = 16;

/// Standard vanilla items and their known crafting remainders.
///
/// Under documented Minecraft crafting rules, consuming these items leaves behind
/// their container/residual item (e.g. buckets or bottles).
#[must_use]
pub fn default_remainder(item_id: &str) -> Option<&'static str> {
    match item_id {
        "minecraft:milk_bucket"
        | "minecraft:water_bucket"
        | "minecraft:lava_bucket"
        | "minecraft:powder_snow_bucket" => Some("minecraft:bucket"),
        "minecraft:honey_bottle" | "minecraft:dragon_breath" | "minecraft:potion" => {
            Some("minecraft:glass_bottle")
        }
        "minecraft:mushroom_stew"
        | "minecraft:rabbit_stew"
        | "minecraft:beetroot_soup"
        | "minecraft:suspicious_stew" => Some("minecraft:bowl"),
        _ => None,
    }
}

/// Errors that can occur during crafting grid manipulation or recipe definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CraftingError {
    /// Grid dimensions are invalid (0 or exceeding maximum limit).
    InvalidDimensions { width: usize, height: usize },
    /// Slot coordinate (col, row) is out of bounds for the grid.
    OutOfBounds { col: usize, row: usize },
    /// Pattern rows must have identical non-zero character lengths.
    InvalidPatternRowLength { expected: usize, found: usize },
    /// Pattern cannot be empty.
    EmptyRecipePattern,
    /// Recipe must contain at least one ingredient.
    EmptyRecipeIngredients,
    /// Ingredient list length does not match width * height.
    RecipeDimensionMismatch { expected: usize, found: usize },
    /// A pattern character was not found in the key mapping.
    UnknownKeyInPattern(char),
    /// Exceeded maximum supported shapeless ingredients.
    TooManyIngredients(usize),
}

impl fmt::Display for CraftingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimensions { width, height } => {
                write!(f, "invalid grid dimensions {width}x{height}")
            }
            Self::OutOfBounds { col, row } => {
                write!(f, "slot coordinate ({col}, {row}) out of bounds")
            }
            Self::InvalidPatternRowLength { expected, found } => {
                write!(
                    f,
                    "inconsistent pattern row length: expected {expected}, found {found}"
                )
            }
            Self::EmptyRecipePattern => write!(f, "recipe pattern cannot be empty"),
            Self::EmptyRecipeIngredients => {
                write!(f, "recipe must contain at least one ingredient")
            }
            Self::RecipeDimensionMismatch { expected, found } => {
                write!(
                    f,
                    "ingredient count mismatch: expected {expected}, found {found}"
                )
            }
            Self::UnknownKeyInPattern(ch) => {
                write!(f, "unknown key '{ch}' in recipe pattern")
            }
            Self::TooManyIngredients(count) => {
                write!(f, "too many ingredients for shapeless recipe: {count}")
            }
        }
    }
}

impl std::error::Error for CraftingError {}

/// An item stack representing an item identifier, a count, and an optional crafting remainder.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemStack {
    pub item: String,
    pub count: u32,
    pub remainder: Option<String>,
}

impl ItemStack {
    /// Constructs a new `ItemStack`. If `item` has a known standard Minecraft crafting remainder
    /// (e.g. milk bucket -> empty bucket), the remainder is automatically populated.
    pub fn new(item: impl Into<String>, count: u32) -> Self {
        let item = item.into();
        let remainder = default_remainder(&item).map(str::to_owned);
        Self {
            item,
            count,
            remainder,
        }
    }

    /// Sets or overrides the residual item returned when this stack is consumed in crafting.
    #[must_use]
    pub fn with_remainder(mut self, remainder: impl Into<String>) -> Self {
        self.remainder = Some(remainder.into());
        self
    }

    /// Clears any residual item for this stack.
    #[must_use]
    pub fn without_remainder(mut self) -> Self {
        self.remainder = None;
        self
    }

    /// Item identifier string (e.g. `"minecraft:iron_ingot"`).
    #[must_use]
    pub fn item(&self) -> &str {
        &self.item
    }

    /// Item count in this stack.
    #[must_use]
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Residual item identifier returned when consumed, if any.
    #[must_use]
    pub fn remainder(&self) -> Option<&str> {
        self.remainder.as_deref()
    }

    /// Returns `true` if count is zero.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// An ingredient specification matching accepted item types in a crafting slot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ingredient {
    allowed: Vec<String>,
}

impl Ingredient {
    /// An ingredient accepting a single exact item identifier.
    pub fn item(item: impl Into<String>) -> Self {
        Self {
            allowed: vec![item.into()],
        }
    }

    /// An ingredient accepting any item from the provided list (e.g. tags such as `#minecraft:planks`).
    pub fn any<I, S>(items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            allowed: items.into_iter().map(Into::into).collect(),
        }
    }

    /// Returns `true` if the item stack satisfies this ingredient (non-empty and item id matches).
    #[must_use]
    pub fn matches(&self, stack: &ItemStack) -> bool {
        stack.count >= 1 && self.matches_item(&stack.item)
    }

    /// Returns `true` if the item identifier is in the allowed set.
    #[must_use]
    pub fn matches_item(&self, item_id: &str) -> bool {
        self.allowed.iter().any(|allowed| allowed == item_id)
    }

    /// The list of accepted item identifiers for this ingredient.
    #[must_use]
    pub fn allowed_items(&self) -> &[String] {
        &self.allowed
    }
}

impl From<&str> for Ingredient {
    fn from(s: &str) -> Self {
        Self::item(s)
    }
}

impl From<String> for Ingredient {
    fn from(s: String) -> Self {
        Self::item(s)
    }
}

impl From<ItemStack> for Ingredient {
    fn from(stack: ItemStack) -> Self {
        Self::item(stack.item)
    }
}

impl From<&ItemStack> for Ingredient {
    fn from(stack: &ItemStack) -> Self {
        Self::item(stack.item.clone())
    }
}

/// Extracted bounding box of non-empty slots in a `CraftingGrid`.
///
/// Empty leading and trailing rows and columns have been trimmed, leaving
/// normalized dimensions and a tightly packed item slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridBoundingBox<'a> {
    pub min_col: usize,
    pub min_row: usize,
    pub width: usize,
    pub height: usize,
    pub items: Vec<Option<&'a ItemStack>>,
}

impl<'a> GridBoundingBox<'a> {
    /// Returns `true` if the bounding box has no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0 || self.items.is_empty()
    }

    /// Returns the item at relative bounding box coordinates `(col, row)`.
    #[must_use]
    pub fn get(&self, col: usize, row: usize) -> Option<&'a ItemStack> {
        if col >= self.width || row >= self.height {
            return None;
        }
        self.items[row * self.width + col]
    }
}

/// A 2D crafting grid with configurable dimensions (such as 2x2 player inventory or 3x3 workbench).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CraftingGrid {
    width: usize,
    height: usize,
    slots: Vec<Option<ItemStack>>,
}

impl CraftingGrid {
    /// Creates a new crafting grid with specified dimensions, initialized to empty slots.
    pub fn new(width: usize, height: usize) -> Result<Self, CraftingError> {
        if width == 0 || height == 0 || width > MAX_GRID_DIMENSION || height > MAX_GRID_DIMENSION {
            return Err(CraftingError::InvalidDimensions { width, height });
        }
        let total = width
            .checked_mul(height)
            .ok_or(CraftingError::InvalidDimensions { width, height })?;
        Ok(Self {
            width,
            height,
            slots: vec![None; total],
        })
    }

    /// Creates a 2x2 crafting grid for player inventory crafting.
    #[must_use]
    pub fn inventory_2x2() -> Self {
        Self::new(2, 2).expect("2x2 dimensions are valid")
    }

    /// Creates a 3x3 crafting grid for crafting tables / workbenches.
    #[must_use]
    pub fn table_3x3() -> Self {
        Self::new(3, 3).expect("3x3 dimensions are valid")
    }

    /// Grid width in columns.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Grid height in rows.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Total slot count (`width * height`).
    #[must_use]
    pub fn size(&self) -> usize {
        self.slots.len()
    }

    /// Converts `(col, row)` to a linear slot index if in bounds.
    #[must_use]
    pub fn index(&self, col: usize, row: usize) -> Option<usize> {
        if col < self.width && row < self.height {
            Some(row * self.width + col)
        } else {
            None
        }
    }

    /// Gets a reference to the item stack at `(col, row)`.
    #[must_use]
    pub fn get(&self, col: usize, row: usize) -> Option<&ItemStack> {
        let idx = self.index(col, row)?;
        self.slots[idx].as_ref().filter(|s| !s.is_empty())
    }

    /// Gets a mutable reference to the item stack at `(col, row)`.
    pub fn get_mut(&mut self, col: usize, row: usize) -> Option<&mut ItemStack> {
        let idx = self.index(col, row)?;
        self.slots[idx].as_mut().filter(|s| !s.is_empty())
    }

    /// Sets or clears the slot at `(col, row)`.
    ///
    /// Stacks with count 0 are normalized to empty (`None`).
    pub fn set(
        &mut self,
        col: usize,
        row: usize,
        item: Option<ItemStack>,
    ) -> Result<(), CraftingError> {
        let idx = self
            .index(col, row)
            .ok_or(CraftingError::OutOfBounds { col, row })?;
        self.slots[idx] = item.filter(|s| !s.is_empty());
        Ok(())
    }

    /// Sets a non-empty item stack at `(col, row)`.
    pub fn set_slot(
        &mut self,
        col: usize,
        row: usize,
        item: ItemStack,
    ) -> Result<(), CraftingError> {
        self.set(col, row, Some(item))
    }

    /// Clears the slot at `(col, row)`, returning any previous item stack.
    pub fn clear_slot(
        &mut self,
        col: usize,
        row: usize,
    ) -> Result<Option<ItemStack>, CraftingError> {
        let idx = self
            .index(col, row)
            .ok_or(CraftingError::OutOfBounds { col, row })?;
        let prev = self.slots[idx].take().filter(|s| !s.is_empty());
        Ok(prev)
    }

    /// Clears all slots in the grid.
    pub fn clear(&mut self) {
        self.slots.fill(None);
    }

    /// Returns `true` if all slots in the grid are empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots
            .iter()
            .all(|s| s.as_ref().is_none_or(ItemStack::is_empty))
    }

    /// Returns the number of non-empty slots in the grid.
    #[must_use]
    pub fn non_empty_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.as_ref().is_some_and(|stack| !stack.is_empty()))
            .count()
    }

    /// Collects references to all non-empty item stacks in the grid.
    #[must_use]
    pub fn non_empty_items(&self) -> Vec<&ItemStack> {
        self.slots
            .iter()
            .filter_map(|s| s.as_ref().filter(|stack| !stack.is_empty()))
            .collect()
    }

    /// Extracts the bounding box by trimming empty leading/trailing rows and columns.
    ///
    /// If the grid contains no items, an empty bounding box with width 0 and height 0 is returned.
    #[must_use]
    pub fn bounding_box(&self) -> GridBoundingBox<'_> {
        let mut min_col = usize::MAX;
        let mut max_col = 0;
        let mut min_row = usize::MAX;
        let mut max_row = 0;
        let mut has_items = false;

        for r in 0..self.height {
            for c in 0..self.width {
                if self.get(c, r).is_some() {
                    has_items = true;
                    min_col = min_col.min(c);
                    max_col = max_col.max(c);
                    min_row = min_row.min(r);
                    max_row = max_row.max(r);
                }
            }
        }

        if !has_items {
            return GridBoundingBox {
                min_col: 0,
                min_row: 0,
                width: 0,
                height: 0,
                items: Vec::new(),
            };
        }

        let width = max_col - min_col + 1;
        let height = max_row - min_row + 1;
        let mut items = Vec::with_capacity(width * height);

        for r in min_row..=max_row {
            for c in min_col..=max_col {
                items.push(self.get(c, r));
            }
        }

        GridBoundingBox {
            min_col,
            min_row,
            width,
            height,
            items,
        }
    }

    /// Consumes 1 item from each non-empty slot in the grid.
    ///
    /// Preserves residual items (such as milk buckets returning empty buckets):
    /// - If the consumed slot had count == 1, the slot in the grid is replaced with 1 remainder item (if any).
    /// - If the consumed slot had count > 1, the slot count is decremented by 1, and any remainder item
    ///   is collected and returned as an overflow residual item.
    ///
    /// Returns any overflow residual items that could not remain in the grid slots.
    pub fn consume_ingredients(&mut self) -> Vec<ItemStack> {
        let mut overflow_residuals = Vec::new();

        for slot in &mut self.slots {
            if let Some(stack) = slot {
                if stack.is_empty() {
                    *slot = None;
                    continue;
                }
                let remainder_item = stack.remainder.clone();
                if stack.count <= 1 {
                    if let Some(remainder) = remainder_item {
                        *slot = Some(ItemStack::new(remainder, 1));
                    } else {
                        *slot = None;
                    }
                } else {
                    stack.count -= 1;
                    if let Some(remainder) = remainder_item {
                        overflow_residuals.push(ItemStack::new(remainder, 1));
                    }
                }
            }
        }

        overflow_residuals
    }
}

/// A shaped recipe with specific pattern dimensions, slot ingredients, and an output result.
///
/// Supports horizontal symmetrical mirroring by default so that bidirectional recipes (such
/// as tools, axes, and stairs) can be placed in either orientation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapedRecipe {
    id: Option<String>,
    width: usize,
    height: usize,
    ingredients: Vec<Option<Ingredient>>,
    result: ItemStack,
    allow_mirroring: bool,
}

impl ShapedRecipe {
    /// Constructs a new shaped recipe with explicit dimensions and ingredients.
    pub fn new(
        width: usize,
        height: usize,
        ingredients: Vec<Option<Ingredient>>,
        result: ItemStack,
    ) -> Result<Self, CraftingError> {
        if width == 0 || height == 0 || width > MAX_GRID_DIMENSION || height > MAX_GRID_DIMENSION {
            return Err(CraftingError::InvalidDimensions { width, height });
        }
        let expected = width
            .checked_mul(height)
            .ok_or(CraftingError::InvalidDimensions { width, height })?;
        if ingredients.len() != expected {
            return Err(CraftingError::RecipeDimensionMismatch {
                expected,
                found: ingredients.len(),
            });
        }
        if !ingredients.iter().any(Option::is_some) {
            return Err(CraftingError::EmptyRecipeIngredients);
        }

        Ok(Self {
            id: None,
            width,
            height,
            ingredients,
            result,
            allow_mirroring: true,
        })
    }

    /// Constructs a shaped recipe from a 2D pattern slice and a character key map.
    ///
    /// The `' '` (space) character is always treated as an empty slot (`None`).
    pub fn from_pattern(
        pattern: &[&str],
        keys: &[(char, Ingredient)],
        result: ItemStack,
    ) -> Result<Self, CraftingError> {
        if pattern.is_empty() {
            return Err(CraftingError::EmptyRecipePattern);
        }
        let height = pattern.len();
        let width = pattern[0].chars().count();
        if width == 0 {
            return Err(CraftingError::EmptyRecipePattern);
        }

        let mut ingredients = Vec::with_capacity(width * height);
        for row in pattern {
            let row_len = row.chars().count();
            if row_len != width {
                return Err(CraftingError::InvalidPatternRowLength {
                    expected: width,
                    found: row_len,
                });
            }
            for ch in row.chars() {
                if ch == ' ' {
                    ingredients.push(None);
                } else if let Some((_, ingredient)) = keys.iter().find(|(k, _)| *k == ch) {
                    ingredients.push(Some(ingredient.clone()));
                } else {
                    return Err(CraftingError::UnknownKeyInPattern(ch));
                }
            }
        }

        Self::new(width, height, ingredients, result)
    }

    /// Assigns a unique identifier to this recipe (e.g. `"minecraft:stick"`).
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Configures whether horizontal symmetrical mirroring is allowed during matching.
    #[must_use]
    pub fn with_mirroring(mut self, allow: bool) -> Self {
        self.allow_mirroring = allow;
        self
    }

    /// Optional recipe identifier.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Pattern width in columns.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Pattern height in rows.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Pattern ingredients slice (length is `width * height`).
    #[must_use]
    pub fn ingredients(&self) -> &[Option<Ingredient>] {
        &self.ingredients
    }

    /// Output item stack produced by this recipe.
    #[must_use]
    pub fn result(&self) -> &ItemStack {
        &self.result
    }

    /// Whether horizontal mirroring is permitted.
    #[must_use]
    pub fn allow_mirroring(&self) -> bool {
        self.allow_mirroring
    }

    /// Gets the expected ingredient at pattern position `(col, row)`.
    #[must_use]
    pub fn get_ingredient(&self, col: usize, row: usize) -> Option<&Ingredient> {
        if col < self.width && row < self.height {
            self.ingredients[row * self.width + col].as_ref()
        } else {
            None
        }
    }

    /// Matches this shaped recipe against a crafting grid.
    ///
    /// Evaluates:
    /// 1. Extract bounding box from the input grid (ignoring surrounding empty rows/columns).
    /// 2. Compares dimensions against recipe `(width, height)`.
    /// 3. Checks direct slot match.
    /// 4. If direct match fails and `allow_mirroring` is true, checks horizontal mirrored match.
    #[must_use]
    pub fn matches(&self, grid: &CraftingGrid) -> bool {
        let bbox = grid.bounding_box();
        if bbox.is_empty() || bbox.width != self.width || bbox.height != self.height {
            return false;
        }

        // 1. Direct match
        let direct = (0..self.height).all(|r| {
            (0..self.width).all(|c| {
                let expected = self.get_ingredient(c, r);
                let actual = bbox.get(c, r);
                match (expected, actual) {
                    (None, None) => true,
                    (Some(exp), Some(act)) => exp.matches(act),
                    _ => false,
                }
            })
        });

        if direct {
            return true;
        }

        if !self.allow_mirroring {
            return false;
        }

        // 2. Horizontal mirrored match
        (0..self.height).all(|r| {
            (0..self.width).all(|c| {
                let mirrored_c = self.width - 1 - c;
                let expected = self.get_ingredient(mirrored_c, r);
                let actual = bbox.get(c, r);
                match (expected, actual) {
                    (None, None) => true,
                    (Some(exp), Some(act)) => exp.matches(act),
                    _ => false,
                }
            })
        })
    }
}

/// A shapeless recipe matching an exact multiset of ingredients regardless of position in the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapelessRecipe {
    id: Option<String>,
    ingredients: Vec<Ingredient>,
    result: ItemStack,
}

impl ShapelessRecipe {
    /// Constructs a shapeless recipe with the given ingredient list and result stack.
    pub fn new(ingredients: Vec<Ingredient>, result: ItemStack) -> Result<Self, CraftingError> {
        if ingredients.is_empty() {
            return Err(CraftingError::EmptyRecipeIngredients);
        }
        if ingredients.len() > MAX_GRID_DIMENSION * MAX_GRID_DIMENSION {
            return Err(CraftingError::TooManyIngredients(ingredients.len()));
        }

        Ok(Self {
            id: None,
            ingredients,
            result,
        })
    }

    /// Constructs a shapeless recipe from an iterator of ingredient convertible items.
    pub fn from_items<I, S>(items: I, result: ItemStack) -> Result<Self, CraftingError>
    where
        I: IntoIterator<Item = S>,
        S: Into<Ingredient>,
    {
        let ingredients = items.into_iter().map(Into::into).collect();
        Self::new(ingredients, result)
    }

    /// Assigns a unique identifier to this recipe.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Optional recipe identifier.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Required multiset of ingredients.
    #[must_use]
    pub fn ingredients(&self) -> &[Ingredient] {
        &self.ingredients
    }

    /// Output item stack produced by this recipe.
    #[must_use]
    pub fn result(&self) -> &ItemStack {
        &self.result
    }

    /// Matches this shapeless recipe against a crafting grid.
    ///
    /// Requires that non-empty slots in the grid match the exact multiset of ingredients.
    #[must_use]
    pub fn matches(&self, grid: &CraftingGrid) -> bool {
        let non_empty = grid.non_empty_items();
        if non_empty.len() != self.ingredients.len() {
            return false;
        }

        let mut used = vec![false; non_empty.len()];
        can_match_shapeless(0, &self.ingredients, &non_empty, &mut used)
    }
}

fn can_match_shapeless(
    idx: usize,
    ingredients: &[Ingredient],
    items: &[&ItemStack],
    used: &mut [bool],
) -> bool {
    if idx == ingredients.len() {
        return true;
    }
    let ingredient = &ingredients[idx];
    for (i, &item) in items.iter().enumerate() {
        if !used[i] && ingredient.matches(item) {
            used[i] = true;
            if can_match_shapeless(idx + 1, ingredients, items, used) {
                return true;
            }
            used[i] = false;
        }
    }
    false
}

/// A recipe in the crafting system, either shaped or shapeless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recipe {
    Shaped(ShapedRecipe),
    Shapeless(ShapelessRecipe),
}

impl Recipe {
    /// Optional recipe identifier.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Shaped(r) => r.id(),
            Self::Shapeless(r) => r.id(),
        }
    }

    /// Returns `true` if this recipe matches the contents of the crafting grid.
    #[must_use]
    pub fn matches(&self, grid: &CraftingGrid) -> bool {
        match self {
            Self::Shaped(r) => r.matches(grid),
            Self::Shapeless(r) => r.matches(grid),
        }
    }

    /// Output item stack produced by this recipe.
    #[must_use]
    pub fn output(&self) -> &ItemStack {
        match self {
            Self::Shaped(r) => r.result(),
            Self::Shapeless(r) => r.result(),
        }
    }
}

impl From<ShapedRecipe> for Recipe {
    fn from(r: ShapedRecipe) -> Self {
        Self::Shaped(r)
    }
}

impl From<ShapelessRecipe> for Recipe {
    fn from(r: ShapelessRecipe) -> Self {
        Self::Shapeless(r)
    }
}

/// Result of a crafting operation, including the output stack and any residual items
/// that could not fit back into the grid slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CraftResult {
    pub output: ItemStack,
    pub residuals: Vec<ItemStack>,
}

/// Registry to register and query crafting recipes.
#[derive(Debug, Clone, Default)]
pub struct RecipeRegistry {
    recipes: Vec<Recipe>,
}

impl RecipeRegistry {
    /// Constructs a new empty recipe registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a recipe in the registry.
    pub fn register(&mut self, recipe: impl Into<Recipe>) {
        self.recipes.push(recipe.into());
    }

    /// Finds a registered recipe by its optional identifier.
    #[must_use]
    pub fn get_by_id(&self, id: &str) -> Option<&Recipe> {
        self.recipes.iter().find(|r| r.id() == Some(id))
    }

    /// Returns the first recipe in registration order that matches the grid.
    #[must_use]
    pub fn find_recipe(&self, grid: &CraftingGrid) -> Option<&Recipe> {
        self.recipes.iter().find(|recipe| recipe.matches(grid))
    }

    /// Evaluates the grid and returns the output `ItemStack` of the first matching recipe, if any.
    #[must_use]
    pub fn evaluate(&self, grid: &CraftingGrid) -> Option<ItemStack> {
        self.find_recipe(grid).map(|r| r.output().clone())
    }

    /// Evaluates the grid, consumes 1 item from each ingredient slot (preserving residuals),
    /// and returns the crafted result.
    pub fn craft(&self, grid: &mut CraftingGrid) -> Option<CraftResult> {
        let recipe = self.find_recipe(grid)?;
        let output = recipe.output().clone();
        let residuals = grid.consume_ingredients();
        Some(CraftResult { output, residuals })
    }

    /// Returns the number of registered recipes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.recipes.len()
    }

    /// Returns `true` if no recipes are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }

    /// Clears all recipes from the registry.
    pub fn clear(&mut self) {
        self.recipes.clear();
    }

    /// Slice of all registered recipes in order.
    #[must_use]
    pub fn recipes(&self) -> &[Recipe] {
        &self.recipes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_2x2_wood_planks_to_sticks() {
        let recipe = ShapedRecipe::new(
            1,
            2,
            vec![
                Some(Ingredient::item("minecraft:oak_planks")),
                Some(Ingredient::item("minecraft:oak_planks")),
            ],
            ItemStack::new("minecraft:stick", 4),
        )
        .expect("valid stick recipe");

        let mut registry = RecipeRegistry::new();
        registry.register(recipe);

        // Test in column 0 of 2x2 grid
        let mut grid_col0 = CraftingGrid::inventory_2x2();
        grid_col0
            .set_slot(0, 0, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        grid_col0
            .set_slot(0, 1, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();

        assert_eq!(
            registry.evaluate(&grid_col0),
            Some(ItemStack::new("minecraft:stick", 4))
        );
        let craft_col0 = registry.craft(&mut grid_col0).expect("craft sticks");
        assert_eq!(craft_col0.output, ItemStack::new("minecraft:stick", 4));
        assert!(craft_col0.residuals.is_empty());
        assert!(grid_col0.is_empty());

        // Test in column 1 of 2x2 grid
        let mut grid_col1 = CraftingGrid::inventory_2x2();
        grid_col1
            .set_slot(1, 0, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        grid_col1
            .set_slot(1, 1, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();

        assert_eq!(
            registry.evaluate(&grid_col1),
            Some(ItemStack::new("minecraft:stick", 4))
        );
        let craft_col1 = registry.craft(&mut grid_col1).expect("craft sticks");
        assert_eq!(craft_col1.output, ItemStack::new("minecraft:stick", 4));
        assert!(craft_col1.residuals.is_empty());
        assert!(grid_col1.is_empty());
    }

    #[test]
    fn test_2x2_iron_ingots_to_iron_block() {
        let recipe = ShapedRecipe::new(
            2,
            2,
            vec![
                Some(Ingredient::item("minecraft:iron_ingot")),
                Some(Ingredient::item("minecraft:iron_ingot")),
                Some(Ingredient::item("minecraft:iron_ingot")),
                Some(Ingredient::item("minecraft:iron_ingot")),
            ],
            ItemStack::new("minecraft:iron_block", 1),
        )
        .expect("valid iron block recipe");

        let mut registry = RecipeRegistry::new();
        registry.register(recipe);

        let mut grid = CraftingGrid::inventory_2x2();
        for r in 0..2 {
            for c in 0..2 {
                grid.set_slot(c, r, ItemStack::new("minecraft:iron_ingot", 1))
                    .unwrap();
            }
        }

        assert_eq!(
            registry.evaluate(&grid),
            Some(ItemStack::new("minecraft:iron_block", 1))
        );
        let result = registry.craft(&mut grid).expect("craft iron block");
        assert_eq!(result.output, ItemStack::new("minecraft:iron_block", 1));
        assert!(grid.is_empty());
    }

    #[test]
    fn test_3x3_shaped_recipes_pickaxe_chest_furnace() {
        let pickaxe_recipe = ShapedRecipe::from_pattern(
            &["###", " | ", " | "],
            &[
                ('#', Ingredient::item("minecraft:iron_ingot")),
                ('|', Ingredient::item("minecraft:stick")),
            ],
            ItemStack::new("minecraft:iron_pickaxe", 1),
        )
        .expect("valid pickaxe recipe")
        .with_id("minecraft:iron_pickaxe");

        let chest_recipe = ShapedRecipe::from_pattern(
            &["###", "# #", "###"],
            &[('#', Ingredient::item("minecraft:oak_planks"))],
            ItemStack::new("minecraft:chest", 1),
        )
        .expect("valid chest recipe")
        .with_id("minecraft:chest");

        let furnace_recipe = ShapedRecipe::from_pattern(
            &["###", "# #", "###"],
            &[('#', Ingredient::item("minecraft:cobblestone"))],
            ItemStack::new("minecraft:furnace", 1),
        )
        .expect("valid furnace recipe")
        .with_id("minecraft:furnace");

        let mut registry = RecipeRegistry::new();
        registry.register(pickaxe_recipe);
        registry.register(chest_recipe);
        registry.register(furnace_recipe);

        // 1. Pickaxe
        let mut pickaxe_grid = CraftingGrid::table_3x3();
        pickaxe_grid
            .set_slot(0, 0, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        pickaxe_grid
            .set_slot(1, 0, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        pickaxe_grid
            .set_slot(2, 0, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        pickaxe_grid
            .set_slot(1, 1, ItemStack::new("minecraft:stick", 1))
            .unwrap();
        pickaxe_grid
            .set_slot(1, 2, ItemStack::new("minecraft:stick", 1))
            .unwrap();

        assert_eq!(
            registry.evaluate(&pickaxe_grid),
            Some(ItemStack::new("minecraft:iron_pickaxe", 1))
        );
        let pickaxe_res = registry.craft(&mut pickaxe_grid).unwrap();
        assert_eq!(
            pickaxe_res.output,
            ItemStack::new("minecraft:iron_pickaxe", 1)
        );
        assert!(pickaxe_grid.is_empty());

        // 2. Chest
        let mut chest_grid = CraftingGrid::table_3x3();
        for r in 0..3 {
            for c in 0..3 {
                if !(r == 1 && c == 1) {
                    chest_grid
                        .set_slot(c, r, ItemStack::new("minecraft:oak_planks", 1))
                        .unwrap();
                }
            }
        }
        assert_eq!(
            registry.evaluate(&chest_grid),
            Some(ItemStack::new("minecraft:chest", 1))
        );
        let chest_res = registry.craft(&mut chest_grid).unwrap();
        assert_eq!(chest_res.output, ItemStack::new("minecraft:chest", 1));
        assert!(chest_grid.is_empty());

        // 3. Furnace
        let mut furnace_grid = CraftingGrid::table_3x3();
        for r in 0..3 {
            for c in 0..3 {
                if !(r == 1 && c == 1) {
                    furnace_grid
                        .set_slot(c, r, ItemStack::new("minecraft:cobblestone", 1))
                        .unwrap();
                }
            }
        }
        assert_eq!(
            registry.evaluate(&furnace_grid),
            Some(ItemStack::new("minecraft:furnace", 1))
        );
        let furnace_res = registry.craft(&mut furnace_grid).unwrap();
        assert_eq!(furnace_res.output, ItemStack::new("minecraft:furnace", 1));
        assert!(furnace_grid.is_empty());
    }

    #[test]
    fn test_offset_placements_1x1_and_1x2() {
        let planks_recipe = ShapedRecipe::new(
            1,
            1,
            vec![Some(Ingredient::item("minecraft:oak_log"))],
            ItemStack::new("minecraft:oak_planks", 4),
        )
        .expect("valid planks recipe");

        let mut registry = RecipeRegistry::new();
        registry.register(planks_recipe);

        // 1x1 placed in all 4 positions of a 2x2 grid
        for r in 0..2 {
            for c in 0..2 {
                let mut grid = CraftingGrid::inventory_2x2();
                grid.set_slot(c, r, ItemStack::new("minecraft:oak_log", 1))
                    .unwrap();

                assert_eq!(
                    registry.evaluate(&grid),
                    Some(ItemStack::new("minecraft:oak_planks", 4)),
                    "offset 1x1 at ({c}, {r}) in 2x2 grid"
                );
            }
        }

        // 1x1 placed in all 9 positions of a 3x3 grid
        for r in 0..3 {
            for c in 0..3 {
                let mut grid = CraftingGrid::table_3x3();
                grid.set_slot(c, r, ItemStack::new("minecraft:oak_log", 1))
                    .unwrap();

                assert_eq!(
                    registry.evaluate(&grid),
                    Some(ItemStack::new("minecraft:oak_planks", 4)),
                    "offset 1x1 at ({c}, {r}) in 3x3 grid"
                );
            }
        }

        // 1x2 stick recipe placed in all 6 positions of a 3x3 grid
        let stick_recipe = ShapedRecipe::new(
            1,
            2,
            vec![
                Some(Ingredient::item("minecraft:oak_planks")),
                Some(Ingredient::item("minecraft:oak_planks")),
            ],
            ItemStack::new("minecraft:stick", 4),
        )
        .expect("valid stick recipe");

        for c in 0..3 {
            for r in 0..2 {
                let mut grid = CraftingGrid::table_3x3();
                grid.set_slot(c, r, ItemStack::new("minecraft:oak_planks", 1))
                    .unwrap();
                grid.set_slot(c, r + 1, ItemStack::new("minecraft:oak_planks", 1))
                    .unwrap();

                assert!(
                    stick_recipe.matches(&grid),
                    "1x2 sticks at col {c}, row {r} in 3x3 grid"
                );
            }
        }
    }

    #[test]
    fn test_mirrored_shaped_recipes_stairs_and_axe() {
        // Stairs: 3x3
        let stairs_recipe = ShapedRecipe::from_pattern(
            &["#  ", "## ", "###"],
            &[('#', Ingredient::item("minecraft:oak_planks"))],
            ItemStack::new("minecraft:oak_stairs", 4),
        )
        .expect("valid stairs recipe");

        // Normal left-facing placement
        let mut left_stairs = CraftingGrid::table_3x3();
        left_stairs
            .set_slot(0, 0, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        left_stairs
            .set_slot(0, 1, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        left_stairs
            .set_slot(1, 1, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        left_stairs
            .set_slot(0, 2, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        left_stairs
            .set_slot(1, 2, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        left_stairs
            .set_slot(2, 2, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();

        assert!(stairs_recipe.matches(&left_stairs));

        // Mirrored right-facing placement
        let mut right_stairs = CraftingGrid::table_3x3();
        right_stairs
            .set_slot(2, 0, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        right_stairs
            .set_slot(1, 1, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        right_stairs
            .set_slot(2, 1, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        right_stairs
            .set_slot(0, 2, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        right_stairs
            .set_slot(1, 2, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        right_stairs
            .set_slot(2, 2, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();

        assert!(stairs_recipe.matches(&right_stairs));

        // Axe: 2x3 pattern
        let axe_recipe = ShapedRecipe::from_pattern(
            &["##", "#|", " |"],
            &[
                ('#', Ingredient::item("minecraft:iron_ingot")),
                ('|', Ingredient::item("minecraft:stick")),
            ],
            ItemStack::new("minecraft:iron_axe", 1),
        )
        .expect("valid axe recipe");

        // Left-facing axe in 3x3 grid at col 0
        let mut left_axe = CraftingGrid::table_3x3();
        left_axe
            .set_slot(0, 0, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        left_axe
            .set_slot(1, 0, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        left_axe
            .set_slot(0, 1, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        left_axe
            .set_slot(1, 1, ItemStack::new("minecraft:stick", 1))
            .unwrap();
        left_axe
            .set_slot(1, 2, ItemStack::new("minecraft:stick", 1))
            .unwrap();

        assert!(axe_recipe.matches(&left_axe));

        // Right-facing axe in 3x3 grid at col 1
        let mut right_axe = CraftingGrid::table_3x3();
        right_axe
            .set_slot(1, 0, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        right_axe
            .set_slot(2, 0, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        right_axe
            .set_slot(2, 1, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        right_axe
            .set_slot(1, 1, ItemStack::new("minecraft:stick", 1))
            .unwrap();
        right_axe
            .set_slot(1, 2, ItemStack::new("minecraft:stick", 1))
            .unwrap();

        assert!(axe_recipe.matches(&right_axe));

        // Disabling mirroring causes mirrored pattern to fail
        let non_mirroring_axe = axe_recipe.with_mirroring(false);
        assert!(non_mirroring_axe.matches(&left_axe));
        assert!(!non_mirroring_axe.matches(&right_axe));
    }

    #[test]
    fn test_shapeless_recipes_dye_and_mushroom_stew() {
        let dye_recipe = ShapelessRecipe::from_items(
            ["minecraft:dandelion"],
            ItemStack::new("minecraft:yellow_dye", 1),
        )
        .expect("valid dye recipe");

        let stew_recipe = ShapelessRecipe::from_items(
            [
                "minecraft:red_mushroom",
                "minecraft:brown_mushroom",
                "minecraft:bowl",
            ],
            ItemStack::new("minecraft:mushroom_stew", 1),
        )
        .expect("valid stew recipe");

        let mut registry = RecipeRegistry::new();
        registry.register(dye_recipe);
        registry.register(stew_recipe);

        // Dye in slot (1, 1) of 2x2 grid
        let mut dye_grid = CraftingGrid::inventory_2x2();
        dye_grid
            .set_slot(1, 1, ItemStack::new("minecraft:dandelion", 1))
            .unwrap();
        assert_eq!(
            registry.evaluate(&dye_grid),
            Some(ItemStack::new("minecraft:yellow_dye", 1))
        );

        // Stew in arbitrary non-adjacent positions of 3x3 grid
        let mut stew_grid = CraftingGrid::table_3x3();
        stew_grid
            .set_slot(0, 0, ItemStack::new("minecraft:bowl", 1))
            .unwrap();
        stew_grid
            .set_slot(2, 1, ItemStack::new("minecraft:red_mushroom", 1))
            .unwrap();
        stew_grid
            .set_slot(1, 2, ItemStack::new("minecraft:brown_mushroom", 1))
            .unwrap();

        assert_eq!(
            registry.evaluate(&stew_grid),
            Some(ItemStack::new("minecraft:mushroom_stew", 1))
        );
        let stew_res = registry.craft(&mut stew_grid).unwrap();
        assert_eq!(
            stew_res.output,
            ItemStack::new("minecraft:mushroom_stew", 1)
        );
        assert!(stew_grid.is_empty());
    }

    #[test]
    fn test_rejection_of_invalid_layouts_and_surplus_items() {
        let planks_recipe = ShapedRecipe::new(
            1,
            1,
            vec![Some(Ingredient::item("minecraft:oak_log"))],
            ItemStack::new("minecraft:oak_planks", 4),
        )
        .unwrap();

        let stick_recipe = ShapedRecipe::new(
            1,
            2,
            vec![
                Some(Ingredient::item("minecraft:oak_planks")),
                Some(Ingredient::item("minecraft:oak_planks")),
            ],
            ItemStack::new("minecraft:stick", 4),
        )
        .unwrap();

        let pickaxe_recipe = ShapedRecipe::from_pattern(
            &["###", " | ", " | "],
            &[
                ('#', Ingredient::item("minecraft:iron_ingot")),
                ('|', Ingredient::item("minecraft:stick")),
            ],
            ItemStack::new("minecraft:iron_pickaxe", 1),
        )
        .unwrap();

        let stew_recipe = ShapelessRecipe::from_items(
            [
                "minecraft:red_mushroom",
                "minecraft:brown_mushroom",
                "minecraft:bowl",
            ],
            ItemStack::new("minecraft:mushroom_stew", 1),
        )
        .unwrap();

        // 1. Surplus items in 1x1 recipe
        let mut grid_surplus_1x1 = CraftingGrid::inventory_2x2();
        grid_surplus_1x1
            .set_slot(0, 0, ItemStack::new("minecraft:oak_log", 1))
            .unwrap();
        grid_surplus_1x1
            .set_slot(1, 1, ItemStack::new("minecraft:dirt", 1))
            .unwrap();
        assert!(!planks_recipe.matches(&grid_surplus_1x1));

        // 2. Extra log in 1x1 recipe
        let mut grid_double_log = CraftingGrid::inventory_2x2();
        grid_double_log
            .set_slot(0, 0, ItemStack::new("minecraft:oak_log", 1))
            .unwrap();
        grid_double_log
            .set_slot(1, 0, ItemStack::new("minecraft:oak_log", 1))
            .unwrap();
        assert!(!planks_recipe.matches(&grid_double_log));

        // 3. Surplus item in 2x2 sticks recipe
        let mut grid_surplus_sticks = CraftingGrid::inventory_2x2();
        grid_surplus_sticks
            .set_slot(0, 0, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        grid_surplus_sticks
            .set_slot(0, 1, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        grid_surplus_sticks
            .set_slot(1, 0, ItemStack::new("minecraft:dirt", 1))
            .unwrap();
        assert!(!stick_recipe.matches(&grid_surplus_sticks));

        // 4. Horizontal planks (wrong orientation for 1x2 stick recipe)
        let mut grid_horizontal_planks = CraftingGrid::inventory_2x2();
        grid_horizontal_planks
            .set_slot(0, 0, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        grid_horizontal_planks
            .set_slot(1, 0, ItemStack::new("minecraft:oak_planks", 1))
            .unwrap();
        assert!(!stick_recipe.matches(&grid_horizontal_planks));

        // 5. Incomplete pickaxe
        let mut grid_broken_pickaxe = CraftingGrid::table_3x3();
        grid_broken_pickaxe
            .set_slot(0, 0, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        grid_broken_pickaxe
            .set_slot(1, 0, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        // missing (2, 0) ingot
        grid_broken_pickaxe
            .set_slot(1, 1, ItemStack::new("minecraft:stick", 1))
            .unwrap();
        grid_broken_pickaxe
            .set_slot(1, 2, ItemStack::new("minecraft:stick", 1))
            .unwrap();
        assert!(!pickaxe_recipe.matches(&grid_broken_pickaxe));

        // 6. Wrong item in pickaxe (stone instead of iron ingot)
        let mut grid_wrong_item_pickaxe = CraftingGrid::table_3x3();
        grid_wrong_item_pickaxe
            .set_slot(0, 0, ItemStack::new("minecraft:cobblestone", 1))
            .unwrap();
        grid_wrong_item_pickaxe
            .set_slot(1, 0, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        grid_wrong_item_pickaxe
            .set_slot(2, 0, ItemStack::new("minecraft:iron_ingot", 1))
            .unwrap();
        grid_wrong_item_pickaxe
            .set_slot(1, 1, ItemStack::new("minecraft:stick", 1))
            .unwrap();
        grid_wrong_item_pickaxe
            .set_slot(1, 2, ItemStack::new("minecraft:stick", 1))
            .unwrap();
        assert!(!pickaxe_recipe.matches(&grid_wrong_item_pickaxe));

        // 7. Surplus item in shapeless stew
        let mut grid_surplus_stew = CraftingGrid::table_3x3();
        grid_surplus_stew
            .set_slot(0, 0, ItemStack::new("minecraft:bowl", 1))
            .unwrap();
        grid_surplus_stew
            .set_slot(1, 0, ItemStack::new("minecraft:red_mushroom", 1))
            .unwrap();
        grid_surplus_stew
            .set_slot(2, 0, ItemStack::new("minecraft:brown_mushroom", 1))
            .unwrap();
        grid_surplus_stew
            .set_slot(0, 1, ItemStack::new("minecraft:dirt", 1))
            .unwrap();
        assert!(!stew_recipe.matches(&grid_surplus_stew));

        // 8. Missing item in shapeless stew
        let mut grid_missing_stew = CraftingGrid::table_3x3();
        grid_missing_stew
            .set_slot(0, 0, ItemStack::new("minecraft:bowl", 1))
            .unwrap();
        grid_missing_stew
            .set_slot(1, 0, ItemStack::new("minecraft:red_mushroom", 1))
            .unwrap();
        assert!(!stew_recipe.matches(&grid_missing_stew));

        // 9. Completely empty grid
        let empty_grid = CraftingGrid::table_3x3();
        assert!(!planks_recipe.matches(&empty_grid));
        assert!(!stick_recipe.matches(&empty_grid));
        assert!(!pickaxe_recipe.matches(&empty_grid));
        assert!(!stew_recipe.matches(&empty_grid));
    }

    #[test]
    fn test_ingredient_consumption_and_residual_preservation() {
        // Cake recipe:
        // Row 0: Milk Bucket, Milk Bucket, Milk Bucket
        // Row 1: Sugar, Egg, Sugar
        // Row 2: Wheat, Wheat, Wheat
        let cake_recipe = ShapedRecipe::from_pattern(
            &["MMM", "SES", "WWW"],
            &[
                ('M', Ingredient::item("minecraft:milk_bucket")),
                ('S', Ingredient::item("minecraft:sugar")),
                ('E', Ingredient::item("minecraft:egg")),
                ('W', Ingredient::item("minecraft:wheat")),
            ],
            ItemStack::new("minecraft:cake", 1),
        )
        .expect("valid cake recipe");

        let mut registry = RecipeRegistry::new();
        registry.register(cake_recipe);

        let mut grid = CraftingGrid::table_3x3();
        // Row 0: milk buckets (with known default remainder "minecraft:bucket")
        grid.set_slot(0, 0, ItemStack::new("minecraft:milk_bucket", 1))
            .unwrap();
        grid.set_slot(1, 0, ItemStack::new("minecraft:milk_bucket", 1))
            .unwrap();
        grid.set_slot(2, 0, ItemStack::new("minecraft:milk_bucket", 1))
            .unwrap();

        // Row 1: sugar, egg, sugar (with higher stack counts)
        grid.set_slot(0, 1, ItemStack::new("minecraft:sugar", 10))
            .unwrap();
        grid.set_slot(1, 1, ItemStack::new("minecraft:egg", 16))
            .unwrap();
        grid.set_slot(2, 1, ItemStack::new("minecraft:sugar", 5))
            .unwrap();

        // Row 2: wheat
        grid.set_slot(0, 2, ItemStack::new("minecraft:wheat", 1))
            .unwrap();
        grid.set_slot(1, 2, ItemStack::new("minecraft:wheat", 1))
            .unwrap();
        grid.set_slot(2, 2, ItemStack::new("minecraft:wheat", 1))
            .unwrap();

        let craft_res = registry.craft(&mut grid).expect("craft cake");
        assert_eq!(craft_res.output, ItemStack::new("minecraft:cake", 1));
        assert!(craft_res.residuals.is_empty());

        // Check top row milk buckets turned into empty buckets in place!
        assert_eq!(grid.get(0, 0), Some(&ItemStack::new("minecraft:bucket", 1)));
        assert_eq!(grid.get(1, 0), Some(&ItemStack::new("minecraft:bucket", 1)));
        assert_eq!(grid.get(2, 0), Some(&ItemStack::new("minecraft:bucket", 1)));

        // Check middle row counts decremented
        assert_eq!(grid.get(0, 1), Some(&ItemStack::new("minecraft:sugar", 9)));
        assert_eq!(grid.get(1, 1), Some(&ItemStack::new("minecraft:egg", 15)));
        assert_eq!(grid.get(2, 1), Some(&ItemStack::new("minecraft:sugar", 4)));

        // Check bottom row consumed to empty
        assert_eq!(grid.get(0, 2), None);
        assert_eq!(grid.get(1, 2), None);
        assert_eq!(grid.get(2, 2), None);
    }

    #[test]
    fn test_stacked_residual_item_overflow() {
        // Test when a slot has count > 1 with a remainder
        let mut grid = CraftingGrid::inventory_2x2();
        grid.set_slot(0, 0, ItemStack::new("minecraft:milk_bucket", 3))
            .unwrap();

        let residuals = grid.consume_ingredients();
        // Since count was 3 > 1, slot count becomes 2, and 1 bucket is returned as overflow residual
        assert_eq!(
            grid.get(0, 0),
            Some(&ItemStack::new("minecraft:milk_bucket", 2))
        );
        assert_eq!(residuals, vec![ItemStack::new("minecraft:bucket", 1)]);
    }

    #[test]
    fn test_recipe_registry_operations() {
        let mut registry = RecipeRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        let r1 = ShapedRecipe::new(
            1,
            1,
            vec![Some(Ingredient::item("minecraft:oak_log"))],
            ItemStack::new("minecraft:oak_planks", 4),
        )
        .unwrap()
        .with_id("recipe:planks");

        let r2 = ShapelessRecipe::from_items(
            ["minecraft:dandelion"],
            ItemStack::new("minecraft:yellow_dye", 1),
        )
        .unwrap()
        .with_id("recipe:dye");

        registry.register(r1);
        registry.register(r2);

        assert_eq!(registry.len(), 2);
        assert!(!registry.is_empty());
        assert!(registry.get_by_id("recipe:planks").is_some());
        assert!(registry.get_by_id("recipe:dye").is_some());
        assert!(registry.get_by_id("recipe:unknown").is_none());

        registry.clear();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_bounding_box_and_error_handling() {
        let empty_grid = CraftingGrid::table_3x3();
        let bbox = empty_grid.bounding_box();
        assert!(bbox.is_empty());
        assert_eq!(bbox.width, 0);
        assert_eq!(bbox.height, 0);

        let mut grid = CraftingGrid::table_3x3();
        grid.set_slot(1, 2, ItemStack::new("minecraft:dirt", 1))
            .unwrap();
        let bbox = grid.bounding_box();
        assert!(!bbox.is_empty());
        assert_eq!(bbox.min_col, 1);
        assert_eq!(bbox.min_row, 2);
        assert_eq!(bbox.width, 1);
        assert_eq!(bbox.height, 1);
        assert_eq!(bbox.get(0, 0), Some(&ItemStack::new("minecraft:dirt", 1)));

        // Out of bounds get / set
        assert_eq!(grid.get(3, 0), None);
        assert!(matches!(
            grid.set_slot(3, 0, ItemStack::new("minecraft:dirt", 1)),
            Err(CraftingError::OutOfBounds { col: 3, row: 0 })
        ));

        // Invalid grid dimensions
        assert!(matches!(
            CraftingGrid::new(0, 2),
            Err(CraftingError::InvalidDimensions {
                width: 0,
                height: 2
            })
        ));
        assert!(matches!(
            CraftingGrid::new(20, 2),
            Err(CraftingError::InvalidDimensions {
                width: 20,
                height: 2
            })
        ));

        // Pattern validation errors
        assert!(matches!(
            ShapedRecipe::from_pattern(&[], &[], ItemStack::new("minecraft:dirt", 1)),
            Err(CraftingError::EmptyRecipePattern)
        ));
        assert!(matches!(
            ShapedRecipe::from_pattern(
                &["#", "##"],
                &[('#', Ingredient::item("minecraft:dirt"))],
                ItemStack::new("minecraft:dirt", 1)
            ),
            Err(CraftingError::InvalidPatternRowLength {
                expected: 1,
                found: 2
            })
        ));
        assert!(matches!(
            ShapedRecipe::from_pattern(&["#"], &[], ItemStack::new("minecraft:dirt", 1)),
            Err(CraftingError::UnknownKeyInPattern('#'))
        ));

        // Zero count item stack is treated as empty
        let zero_stack = ItemStack::new("minecraft:dirt", 0);
        assert!(zero_stack.is_empty());
        grid.set(1, 2, Some(zero_stack)).unwrap();
        assert!(grid.get(1, 2).is_none());
    }
}
