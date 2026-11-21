//! Block, item and enchantment tag registries loaded from
//! `data/<namespace>/tags/**`, with nested `#tag` references resolved.

use super::*;

pub(super) fn load_registry(
    root: &Path,
    registry: &str,
) -> Result<HashMap<String, Vec<String>>, WorldgenError> {
    load_registry_from_roots(&[root], registry)
}

/// Loads `data/<namespace>/tags/<registry>/**/*.json` across ordered pack
/// roots, applying `"replace": true` before resolving nested `#tag` references.
pub(super) fn load_registry_from_roots(
    roots: &[&Path],
    registry: &str,
) -> Result<HashMap<String, Vec<String>>, WorldgenError> {
    let mut raw: HashMap<String, Vec<Value>> = HashMap::new();
    for root in roots {
        let data_dir = root.join("data");
        let Some(namespaces) = read_dir_optional(&data_dir)? else {
            continue;
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
        for ns_path in ns_dirs {
            let ns = ns_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let category = ns_path.join("tags").join(registry);
            for entry in walk_json(&category)? {
                let document = read_json(&entry)?;
                let replace = match document.get("replace") {
                    None => false,
                    Some(value) => value.as_bool().ok_or_else(|| {
                        WorldgenError::Invalid(format!(
                            "{}: tag field 'replace' must be a boolean",
                            entry.display()
                        ))
                    })?,
                };
                let values = match document.get("values") {
                    None => Vec::new(),
                    Some(value) => value
                        .as_array()
                        .ok_or_else(|| {
                            WorldgenError::Invalid(format!(
                                "{}: tag field 'values' must be an array",
                                entry.display()
                            ))
                        })?
                        .clone(),
                };
                // A tag's id is its path under the category directory, so a
                // pack that files one away (`mineable/pickaxe.json`) keeps
                // the id its references use, and two same-named files in
                // different directories stay separate entries.
                let relative = entry
                    .strip_prefix(&category)
                    .unwrap_or(entry.as_path())
                    .with_extension("");
                let mut id = ns.clone();
                id.push(':');
                id.push_str(&relative.to_string_lossy().replace('\\', "/"));
                let slot = raw.entry(id).or_default();
                if replace {
                    slot.clear();
                }
                slot.extend(values);
            }
        }
    }
    // Entries may name another tag; resolve those references into a
    // flat member list. A reference cycle stops at the first repeat
    // instead of recursing forever.
    let mut entries: HashMap<String, Vec<String>> = HashMap::new();
    let ids: Vec<String> = raw.keys().cloned().collect();
    for id in ids {
        let mut members: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        seen.insert(id.clone());
        let mut queue: VecDeque<Value> = raw.get(&id).cloned().unwrap_or_default().into();
        while let Some(value) = queue.pop_front() {
            let (name, reference, required) = match &value {
                Value::String(value) => match value.strip_prefix('#') {
                    Some(tag) => (None, Some(tag), true),
                    None => (Some(value.as_str()), None, true),
                },
                Value::Object(object) => {
                    let required = object
                        .get("required")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    if let Some(tag) = object.get("tag").and_then(Value::as_str) {
                        (None, Some(tag.trim_start_matches('#')), required)
                    } else if let Some(value) = object.get("id").and_then(Value::as_str) {
                        match value.strip_prefix('#') {
                            Some(tag) => (None, Some(tag), required),
                            None => (Some(value), None, required),
                        }
                    } else {
                        continue;
                    }
                }
                _ => continue,
            };
            if let Some(name) = name.filter(|name| !name.is_empty())
                && !members.iter().any(|member| member == name)
            {
                members.push(name.to_string());
            }
            if let Some(tag) = reference {
                match raw.get(tag) {
                    Some(values) if seen.insert(tag.to_string()) => {
                        queue.extend(values.iter().cloned());
                    }
                    Some(_) => {}
                    None if required => {
                        return Err(WorldgenError::Invalid(format!(
                            "{registry} tag {id} references missing required tag {tag}"
                        )));
                    }
                    None => {}
                }
            }
        }
        entries.insert(id, members);
    }
    Ok(entries)
}

pub(super) fn tag_contains(entries: &HashMap<String, Vec<String>>, tag: &str, id: &str) -> bool {
    entries
        .get(tag)
        .is_some_and(|members| members.iter().any(|member| member == id))
}

pub(super) fn tag_members<'a>(
    entries: &'a HashMap<String, Vec<String>>,
    tag: &str,
) -> Option<&'a [String]> {
    entries.get(tag).map(Vec::as_slice)
}

/// The block tags of the provisioned pack, keyed by namespaced id.
#[derive(Debug, Default, Clone)]
pub struct BlockTags {
    pub(super) entries: HashMap<String, Vec<String>>,
    /// Precomputed set membership for O(1) lookup when loaded from pack data.
    pub(super) membership: HashMap<String, HashSet<String>>,
}

pub(super) fn build_tag_membership(
    entries: &HashMap<String, Vec<String>>,
) -> HashMap<String, HashSet<String>> {
    entries
        .iter()
        .map(|(tag, members)| (tag.clone(), members.iter().cloned().collect()))
        .collect()
}

impl BlockTags {
    /// Loads `data/<namespace>/tags/block/**/*.json` under the data root.
    /// A missing directory yields an empty registry, which makes every tag
    /// test fail rather than assuming membership.
    pub fn load(root: &Path) -> Result<Self, WorldgenError> {
        let entries = load_registry(root, "block")?;
        let membership = build_tag_membership(&entries);
        Ok(Self {
            entries,
            membership,
        })
    }

    /// Loads `data/<namespace>/tags/block/**/*.json` across ordered pack roots.
    pub fn load_from_roots(roots: &[&Path]) -> Result<Self, WorldgenError> {
        let entries = load_registry_from_roots(roots, "block")?;
        let membership = build_tag_membership(&entries);
        Ok(Self {
            entries,
            membership,
        })
    }

    /// Members of `tag`, or `None` if `tag` is absent from the pack.
    pub fn members(&self, tag: &str) -> Option<&[String]> {
        tag_members(&self.entries, tag)
    }

    /// Whether `block` is a member of `tag`.
    pub fn contains(&self, tag: &str, block: &str) -> bool {
        if let Some(set) = self.membership.get(tag) {
            set.contains(block)
        } else {
            tag_contains(&self.entries, tag, block)
        }
    }

    /// Number of tags read from the pack.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pack declared no block tags at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The item tags of the provisioned pack, keyed by namespaced id.
#[derive(Debug, Default, Clone)]
pub struct ItemTags {
    pub(super) entries: HashMap<String, Vec<String>>,
}

impl ItemTags {
    /// Loads `data/<namespace>/tags/item/**/*.json` under the data root.
    /// A missing directory yields an empty registry.
    pub fn load(root: &Path) -> Result<Self, WorldgenError> {
        Ok(Self {
            entries: load_registry(root, "item")?,
        })
    }

    /// Loads `data/<namespace>/tags/item/**/*.json` across ordered pack roots.
    pub fn load_from_roots(roots: &[&Path]) -> Result<Self, WorldgenError> {
        Ok(Self {
            entries: load_registry_from_roots(roots, "item")?,
        })
    }

    /// Members of `tag`, or `None` if `tag` is absent from the pack.
    pub fn members(&self, tag: &str) -> Option<&[String]> {
        tag_members(&self.entries, tag)
    }

    /// Whether `item` is a member of `tag`.
    pub fn contains(&self, tag: &str, item: &str) -> bool {
        tag_contains(&self.entries, tag, item)
    }

    /// Number of tags read from the pack.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pack declared no item tags at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The enchantment tags of the provisioned pack, keyed by namespaced id.
#[derive(Debug, Default, Clone)]
pub struct EnchantmentTags {
    pub(super) entries: HashMap<String, Vec<String>>,
}

impl EnchantmentTags {
    /// Loads `data/<namespace>/tags/enchantment/**/*.json` under the data root.
    /// A missing directory yields an empty registry.
    pub fn load(root: &Path) -> Result<Self, WorldgenError> {
        Ok(Self {
            entries: load_registry(root, "enchantment")?,
        })
    }

    /// Loads `data/<namespace>/tags/enchantment/**/*.json` across ordered pack roots.
    pub fn load_from_roots(roots: &[&Path]) -> Result<Self, WorldgenError> {
        Ok(Self {
            entries: load_registry_from_roots(roots, "enchantment")?,
        })
    }

    /// Members of `tag`, or `None` if `tag` is absent from the pack.
    pub fn members(&self, tag: &str) -> Option<&[String]> {
        tag_members(&self.entries, tag)
    }

    /// Whether `enchantment` is a member of `tag`.
    pub fn contains(&self, tag: &str, enchantment: &str) -> bool {
        tag_contains(&self.entries, tag, enchantment)
    }

    /// Number of tags read from the pack.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pack declared no enchantment tags at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
