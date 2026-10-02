//! Versioned protocol-777 registry-ID tables for the vanilla chunk adapter.
//!
//! Java chunk sections never carry block or biome *names*: every section slot
//! holds a numeric index into two versioned registries (the static block-state
//! registry and the `minecraft:worldgen/biome` dynamic registry). Those numbers
//! are Mojang-derived, so RustMC never commits them: an operator provisions a
//! versioned table at runtime (the same policy as the local preview manifest,
//! see `docs/PROVENANCE.md` and [ADR-0014](docs/decisions/ADR-0014.md)) and this
//! module turns it into the lookup the adapter needs.
//!
//! Two guarantees this module owns:
//!
//! * **No silent substitution.** Every name that the generator can emit must be
//!   present in the table. A miss is a typed
//!   [`RegistryError::UnknownState`] / [`RegistryError::UnknownBiome`], never a
//!   fallback ID. `minecraft:air` is additionally required up front, because the
//!   adapter needs it for rows the generator leaves unwritten.
//! * **Direct-mode widths come from the registry size, not from a constant.**
//!   A section whose palette exceeds the indirect limit switches to the global
//!   (direct) palette, whose bits-per-entry is `ceil(log2(registry_size))` —
//!   exactly the vanilla rule (`Mth.ceillog2` of the id-map size). For 26.3 the
//!   provisioned block-state registry needs 16 bits and the biome registry 7,
//!   but both numbers are read from the table so a future version cannot reuse
//!   a stale width.
//!
//! ## Provisioned table shape
//!
//! ```json
//! {
//!   "version": "26.3",
//!   "protocol": 777,
//!   "block_state_count": 65536,
//!   "biome_count": 128,
//!   "block_states": {"minecraft:air": 0, "minecraft:stone[snowy=false]": 9},
//!   "biomes": {"minecraft:plains": 0},
//!   "state_kinds": {"minecraft:water": "fluid"}
//! }
//! ```
//!
//! `block_states` maps a block-state key to its protocol ID. Keys may be bare
//! block names (`minecraft:stone`, for the state the generator's material rules
//! report) or fully qualified states with properties
//! (`minecraft:grass_block[snowy=false]`); property order is normalized so both
//! spellings of the same state collapse to one entry. `state_kinds` is optional
//! and overrides the name-derived classification below.
//!
//! ## Block-state classification
//!
//! Heightmaps and sky light need more than an ID: they need to know whether a
//! state is air, fluid, leaves, or plain solid. RustMC classifies by the
//! documented behavior of the block families the terrain generator can emit
//! (world-gen air, the dimension's `default_fluid` family, leaf blocks, and
//! everything else), and the table may override any single state. Facts this
//! classification encodes: `Heightmap.Types.WORLD_SURFACE` captures every
//! non-air state, `MOTION_BLOCKING` only states that block motion (fluids do
//! not), `MOTION_BLOCKING_NO_LEAVES` drops leaves as well, and the section
//! `fluidCount` field counts states with a non-empty fluid
//! (`LevelChunkSection$1BlockCounter#accept`, consulted 2 October 2026 under
//! the ADR-0014 knowledge-consultation policy; nothing was copied).

use std::collections::BTreeMap;

use serde_json::Value;

/// Game version the adapter speaks. A table for any other version is rejected.
pub const SUPPORTED_VERSION: &str = "26.3";
/// Network protocol the adapter encodes for (Java Edition 26.3).
pub const SUPPORTED_PROTOCOL: u32 = 777;

/// How one block state participates in heightmaps, fluid counts, and skylight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StateKind {
    /// `air`, `cave_air`, `void_air`: invisible to every heightmap, no
    /// attenuation of skylight.
    Air,
    /// The dimension fluid (`water`, `lava`): counted in `WORLD_SURFACE` and in
    /// the section fluid count, but does not block motion.
    Fluid,
    /// Leaves: block motion, excluded from `MOTION_BLOCKING_NO_LEAVES`,
    /// attenuate skylight by one level.
    Leaves,
    /// Any other state: blocks motion and stops skylight.
    Solid,
}

impl StateKind {
    /// Classification from the state name alone, used unless the provisioned
    /// table overrides it.
    pub fn from_state_name(name: &str) -> Self {
        let base = name.split_once('[').map_or(name, |(base, _)| base);
        match base {
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => Self::Air,
            "minecraft:water" | "minecraft:lava" => Self::Fluid,
            _ if base.ends_with("_leaves") || base.ends_with("_leaf") => Self::Leaves,
            _ => Self::Solid,
        }
    }

    /// Whether a state in this slot is captured by a heightmap type.
    pub fn captured_by(self, kind: HeightmapKind) -> bool {
        match kind {
            HeightmapKind::WorldSurface => self != Self::Air,
            HeightmapKind::MotionBlocking => matches!(self, Self::Leaves | Self::Solid),
            HeightmapKind::MotionBlockingNoLeaves => self == Self::Solid,
        }
    }

    /// Skylight levels removed when light passes straight down through this
    /// state. Air passes, leaves and fluid cost one level, solids close the
    /// column; this is the same vertical model the accepted preview uses.
    pub fn sky_attenuation(self) -> u8 {
        match self {
            Self::Air => 0,
            Self::Fluid | Self::Leaves => 1,
            Self::Solid => 15,
        }
    }

    /// Name form used in `state_kinds` overrides.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Air => "air",
            Self::Fluid => "fluid",
            Self::Leaves => "leaves",
            Self::Solid => "solid",
        }
    }

    /// Inverse of [`StateKind::as_str`], for parsing an override table.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "air" => Self::Air,
            "fluid" => Self::Fluid,
            "leaves" => Self::Leaves,
            "solid" => Self::Solid,
            _ => return None,
        })
    }
}

/// The three heightmap types a 26.3 client expects in a chunk packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HeightmapKind {
    /// Protocol id 1: highest non-air state.
    WorldSurface,
    /// Protocol id 4: highest motion-blocking state.
    MotionBlocking,
    /// Protocol id 5: highest motion-blocking state that is not leaves.
    MotionBlockingNoLeaves,
}

impl HeightmapKind {
    /// The on-wire `Heightmap.Types` ordinal for this heightmap.
    pub fn protocol_id(self) -> u32 {
        match self {
            Self::WorldSurface => 1,
            Self::MotionBlocking => 4,
            Self::MotionBlockingNoLeaves => 5,
        }
    }
}

/// Every way a provisioned table can fail to describe the target version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// The table targets another game version or protocol.
    UnsupportedVersion { version: String, protocol: u32 },
    /// A required top-level field is absent or not of the expected type.
    MissingField(String),
    /// The table JSON itself could not be read.
    MalformedTable(String),
    /// A state or biome key is not a `namespace:path` identifier, optionally
    /// followed by a `property=value` list.
    MalformedKey(String),
    /// Two table entries normalize to the same state or biome.
    DuplicateKey(String),
    /// An ID is not inside the declared registry, so it cannot be a real
    /// protocol ID for this version.
    IdOutOfRange { name: String, id: u32, count: u32 },
    /// A registry size is zero, so no direct-mode width can be derived.
    EmptyRegistry(String),
    /// The generator emitted a name the table does not carry.
    UnknownState { name: String },
    /// The generator emitted a biome the table does not carry.
    UnknownBiome { name: String },
    /// The table has no entry for `minecraft:air`, which the adapter needs for
    /// rows above the generated column.
    MissingAirState,
    /// A `state_kinds` override named a state that is not in the table.
    UnknownOverride { name: String },
    /// A `state_kinds` override value is not one of the four kinds.
    InvalidKind { value: String },
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion { version, protocol } => write!(
                f,
                "registry table targets {version}/{protocol}, expected {SUPPORTED_VERSION}/{SUPPORTED_PROTOCOL}"
            ),
            Self::MissingField(name) => write!(f, "registry table lacks field `{name}`"),
            Self::MalformedTable(message) => write!(f, "registry table is not readable: {message}"),
            Self::MalformedKey(key) => {
                write!(f, "registry key `{key}` is not a valid state or biome id")
            }
            Self::DuplicateKey(key) => write!(f, "registry key `{key}` is listed twice"),
            Self::IdOutOfRange { name, id, count } => {
                write!(f, "registry id {id} for `{name}` is outside 0..{count}")
            }
            Self::EmptyRegistry(name) => write!(f, "registry `{name}` has no entries"),
            Self::UnknownState { name } => write!(f, "no protocol id for block state `{name}`"),
            Self::UnknownBiome { name } => write!(f, "no protocol id for biome `{name}`"),
            Self::MissingAirState => write!(f, "registry table has no `minecraft:air` state"),
            Self::UnknownOverride { name } => {
                write!(f, "state_kinds override names unknown state `{name}`")
            }
            Self::InvalidKind { value } => {
                write!(
                    f,
                    "state kind `{value}` is not air, fluid, leaves, or solid"
                )
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// One resolvable block state: its protocol id and how it behaves in a section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateEntry {
    /// Protocol-777 index into the versioned block-state registry.
    pub id: u32,
    /// Classification used for heightmaps, fluid counts, and skylight.
    pub kind: StateKind,
}

/// `Mth.ceillog2`: bits needed to index `count` distinct values, the vanilla
/// rule for choosing a direct (global) palette width.
pub const fn ceillog2(count: u32) -> usize {
    if count <= 1 {
        return 0;
    }
    (u32::BITS - (count - 1).leading_zeros()) as usize
}

/// Normalized `namespace:path[k=v,...]` key: properties sorted by name, so the
/// generator's spelling and the report's spelling compare equal.
pub fn canonical_state_key(name: &str) -> Result<String, RegistryError> {
    let trimmed = name.trim();
    let (base, properties) = match trimmed.split_once('[') {
        Some((base, rest)) => {
            let properties = rest
                .strip_suffix(']')
                .ok_or_else(|| RegistryError::MalformedKey(trimmed.to_string()))?;
            (base, properties)
        }
        None => (trimmed, ""),
    };
    if !is_identifier(base) || trimmed.len() == base.len() && !properties.is_empty() {
        return Err(RegistryError::MalformedKey(trimmed.to_string()));
    }
    if properties.is_empty() {
        if trimmed.len() != base.len() {
            // `namespace:path[]`: brackets with nothing inside are malformed,
            // not an empty property set.
            return Err(RegistryError::MalformedKey(trimmed.to_string()));
        }
        return Ok(base.to_string());
    }
    let mut pairs: Vec<(String, String)> = Vec::new();
    for part in properties.split(',') {
        let (key, value) = part
            .split_once('=')
            .ok_or_else(|| RegistryError::MalformedKey(trimmed.to_string()))?;
        if key.trim().is_empty() || value.trim().is_empty() || pairs.iter().any(|(k, _)| k == key) {
            return Err(RegistryError::MalformedKey(trimmed.to_string()));
        }
        pairs.push((key.trim().to_string(), value.trim().to_string()));
    }
    pairs.sort();
    let inner = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!("{base}[{inner}]"))
}

fn is_identifier(value: &str) -> bool {
    let Some((namespace, path)) = value.split_once(':') else {
        return false;
    };
    let check = |part: &str, allow_dot: bool| {
        !part.is_empty()
            && part.chars().all(|c| {
                c.is_ascii_lowercase()
                    || c.is_ascii_digit()
                    || c == '_'
                    || c == '-'
                    || (allow_dot && c == '.')
            })
    };
    check(namespace, false) && check(path, true)
}

/// Validated protocol-777 ID tables for one game version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryTables {
    version: String,
    protocol: u32,
    state_count: u32,
    biome_count: u32,
    states: BTreeMap<String, StateEntry>,
    kinds_by_id: BTreeMap<u32, StateKind>,
    biomes: BTreeMap<String, u32>,
    air: u32,
}

impl RegistryTables {
    /// Builds and validates a table pair.
    ///
    /// `state_count` / `biome_count` are the sizes of the versioned registries,
    /// which fix the direct-mode bit widths; every ID must be inside them.
    pub fn new(
        version: &str,
        protocol: u32,
        state_count: u32,
        biome_count: u32,
        states: impl IntoIterator<Item = (String, u32)>,
        biomes: impl IntoIterator<Item = (String, u32)>,
    ) -> Result<Self, RegistryError> {
        if version != SUPPORTED_VERSION || protocol != SUPPORTED_PROTOCOL {
            return Err(RegistryError::UnsupportedVersion {
                version: version.to_string(),
                protocol,
            });
        }
        if state_count == 0 {
            return Err(RegistryError::EmptyRegistry("block_state".to_string()));
        }
        if biome_count == 0 {
            return Err(RegistryError::EmptyRegistry("biome".to_string()));
        }
        let mut table: BTreeMap<String, StateEntry> = BTreeMap::new();
        for (name, id) in states {
            let key = canonical_state_key(&name)?;
            if id >= state_count {
                return Err(RegistryError::IdOutOfRange {
                    name: key,
                    id,
                    count: state_count,
                });
            }
            let entry = StateEntry {
                id,
                kind: StateKind::from_state_name(&key),
            };
            if table.insert(key.clone(), entry).is_some() {
                return Err(RegistryError::DuplicateKey(key));
            }
        }
        let air = table
            .get("minecraft:air")
            .ok_or(RegistryError::MissingAirState)?
            .id;
        let mut kinds_by_id = BTreeMap::new();
        for entry in table.values() {
            kinds_by_id.insert(entry.id, entry.kind);
        }
        let mut biome_table: BTreeMap<String, u32> = BTreeMap::new();
        for (name, id) in biomes {
            if !is_identifier(&name) {
                return Err(RegistryError::MalformedKey(name));
            }
            if id >= biome_count {
                return Err(RegistryError::IdOutOfRange {
                    name: name.clone(),
                    id,
                    count: biome_count,
                });
            }
            if biome_table.insert(name.clone(), id).is_some() {
                return Err(RegistryError::DuplicateKey(name));
            }
        }
        Ok(Self {
            version: version.to_string(),
            protocol,
            state_count,
            biome_count,
            states: table,
            kinds_by_id,
            biomes: biome_table,
            air,
        })
    }

    /// Reads an operator-provisioned JSON table. Nothing from the table is
    /// stored in the repository; this is the runtime seam.
    pub fn from_provisioned(text: &str) -> Result<Self, RegistryError> {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| RegistryError::MalformedTable(error.to_string()))?;
        let object = value
            .as_object()
            .ok_or_else(|| RegistryError::MalformedTable("root is not an object".into()))?;
        let field = |name: &str| {
            object
                .get(name)
                .ok_or_else(|| RegistryError::MissingField(name.to_string()))
        };
        let version = field("version")?
            .as_str()
            .ok_or_else(|| RegistryError::MissingField("version".to_string()))?
            .to_string();
        let protocol = u32::try_from(
            field("protocol")?
                .as_u64()
                .ok_or_else(|| RegistryError::MissingField("protocol".to_string()))?,
        )
        .map_err(|_| RegistryError::MissingField("protocol".to_string()))?;
        let number = |name: &str| -> Result<u32, RegistryError> {
            u32::try_from(
                field(name)?
                    .as_u64()
                    .ok_or_else(|| RegistryError::MissingField(name.to_string()))?,
            )
            .map_err(|_| RegistryError::MissingField(name.to_string()))
        };
        let state_count = number("block_state_count")?;
        let biome_count = number("biome_count")?;
        let mapping = |name: &str| -> Result<Vec<(String, u32)>, RegistryError> {
            let entries = field(name)?
                .as_object()
                .ok_or_else(|| RegistryError::MissingField(name.to_string()))?;
            entries
                .iter()
                .map(|(key, value)| {
                    let id = u32::try_from(
                        value
                            .as_u64()
                            .ok_or_else(|| RegistryError::MissingField(format!("{name}.{key}")))?,
                    )
                    .map_err(|_| RegistryError::MissingField(format!("{name}.{key}")))?;
                    Ok((key.clone(), id))
                })
                .collect()
        };
        let states = mapping("block_states")?;
        let biomes = mapping("biomes")?;
        let mut tables = Self::new(&version, protocol, state_count, biome_count, states, biomes)?;
        if let Some(Value::Object(overrides)) = object.get("state_kinds") {
            for (name, value) in overrides {
                let text = value
                    .as_str()
                    .ok_or_else(|| RegistryError::MissingField(format!("state_kinds.{name}")))?;
                let kind = StateKind::parse(text).ok_or_else(|| RegistryError::InvalidKind {
                    value: text.to_string(),
                })?;
                tables = tables.with_kind_override(name, kind)?;
            }
        }
        Ok(tables)
    }

    /// Overrides the classification of one state, e.g. a mod datapack block
    /// whose name does not follow the leaf/fluid naming rule.
    pub fn with_kind_override(
        mut self,
        name: &str,
        kind: StateKind,
    ) -> Result<Self, RegistryError> {
        let key = canonical_state_key(name)?;
        let entry = self
            .states
            .get_mut(&key)
            .ok_or(RegistryError::UnknownOverride { name: key })?;
        if entry.kind != kind {
            let stale = self
                .kinds_by_id
                .get(&entry.id)
                .is_some_and(|previous| *previous == entry.kind);
            if stale {
                // Last state carrying the old class: drop the stale id.
                self.kinds_by_id.remove(&entry.id);
            }
            entry.kind = kind;
            self.kinds_by_id.insert(entry.id, kind);
        }
        Ok(self)
    }

    /// Resolves one block-state name to its protocol id and class. Unknown or
    /// unmappable names are an error, never a substitution.
    pub fn state(&self, name: &str) -> Result<StateEntry, RegistryError> {
        let key = canonical_state_key(name)?;
        self.states
            .get(&key)
            .copied()
            .ok_or(RegistryError::UnknownState { name: key })
    }

    /// Resolves one biome identifier to its dynamic-registry id.
    pub fn biome(&self, name: &str) -> Result<u32, RegistryError> {
        self.biomes
            .get(name)
            .copied()
            .ok_or_else(|| RegistryError::UnknownBiome {
                name: name.to_string(),
            })
    }

    /// The class of an already-resolved state id, `None` when the id did not
    /// come from this table.
    pub fn kind(&self, id: u32) -> Option<StateKind> {
        self.kinds_by_id.get(&id).copied()
    }

    /// Protocol id of `minecraft:air`, used for rows the generator left unset.
    pub fn air_state(&self) -> u32 {
        self.air
    }

    /// Bits per entry for a direct block-state palette.
    pub fn state_bits(&self) -> usize {
        ceillog2(self.state_count)
    }

    /// Bits per entry for a direct biome palette.
    pub fn biome_bits(&self) -> usize {
        ceillog2(self.biome_count)
    }

    /// Size of the versioned block-state registry.
    pub fn state_count(&self) -> u32 {
        self.state_count
    }

    /// Size of the versioned biome registry.
    pub fn biome_count(&self) -> u32 {
        self.biome_count
    }

    /// Game version these IDs were provisioned for.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Network protocol these IDs were provisioned for.
    pub fn protocol(&self) -> u32 {
        self.protocol
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> RegistryTables {
        RegistryTables::new(
            SUPPORTED_VERSION,
            SUPPORTED_PROTOCOL,
            65_536,
            128,
            [
                ("minecraft:air".to_string(), 0u32),
                ("minecraft:stone".to_string(), 1),
                ("minecraft:grass_block[snowy=false]".to_string(), 9),
                ("minecraft:oak_leaves[persistent=false]".to_string(), 300),
                ("minecraft:water".to_string(), 700),
                ("minecraft:cave_air".to_string(), 1_024),
            ],
            [
                ("minecraft:plains".to_string(), 0u32),
                ("minecraft:desert".to_string(), 12),
            ],
        )
        .expect("fixture tables")
    }

    #[test]
    fn keys_are_normalized_and_property_order_irrelevant() {
        let tables = tables();
        let first = tables
            .state("minecraft:oak_leaves[persistent=false]")
            .expect("exact state");
        let spaced = tables
            .state("minecraft:oak_leaves[ persistent = false ]")
            .expect("spaced properties normalize to the same key");
        assert_eq!(first, spaced);
        assert_eq!(first.kind, StateKind::Leaves);
        assert_eq!(
            tables.state("minecraft:water").unwrap().kind,
            StateKind::Fluid
        );
        assert_eq!(
            tables.state("minecraft:cave_air").unwrap().kind,
            StateKind::Air,
            "carved rows must not read as solid"
        );
    }

    #[test]
    fn unmappable_names_are_hard_errors() {
        let tables = tables();
        assert_eq!(
            tables.state("minecraft:glow_lichen"),
            Err(RegistryError::UnknownState {
                name: "minecraft:glow_lichen".to_string()
            })
        );
        assert_eq!(
            tables.biome("minecraft:jungle"),
            Err(RegistryError::UnknownBiome {
                name: "minecraft:jungle".to_string()
            })
        );
        for bad in [
            "",
            "minecraft:",
            ":stone",
            "Stone",
            "minecraft:stone[",
            "minecraft:stone[]",
            "minecraft:stone[snowy]",
            "minecraft:stone[snowy=]",
        ] {
            assert!(
                canonical_state_key(bad).is_err(),
                "`{bad}` must not parse as a state key"
            );
        }
    }

    #[test]
    fn versions_and_id_ranges_are_enforced() {
        let error = RegistryTables::new(
            "26.2",
            SUPPORTED_PROTOCOL,
            65_536,
            128,
            [("minecraft:air".to_string(), 0u32)],
            [],
        )
        .unwrap_err();
        assert!(matches!(error, RegistryError::UnsupportedVersion { .. }));
        let error = RegistryTables::new(
            SUPPORTED_VERSION,
            766,
            65_536,
            128,
            [("minecraft:air".to_string(), 0u32)],
            [],
        )
        .unwrap_err();
        assert!(matches!(error, RegistryError::UnsupportedVersion { .. }));
        assert_eq!(
            RegistryTables::new(
                SUPPORTED_VERSION,
                SUPPORTED_PROTOCOL,
                16,
                128,
                [
                    ("minecraft:air".to_string(), 0u32),
                    ("minecraft:stone".to_string(), 16)
                ],
                [],
            )
            .unwrap_err(),
            RegistryError::IdOutOfRange {
                name: "minecraft:stone".to_string(),
                id: 16,
                count: 16
            }
        );
        assert!(
            RegistryTables::new(
                SUPPORTED_VERSION,
                SUPPORTED_PROTOCOL,
                65_536,
                128,
                [("minecraft:stone".to_string(), 1u32)],
                [],
            )
            .is_err(),
            "a table without minecraft:air cannot fill the column"
        );
        assert!(
            RegistryTables::new(
                SUPPORTED_VERSION,
                SUPPORTED_PROTOCOL,
                0,
                128,
                [("minecraft:air".to_string(), 0u32)],
                [],
            )
            .is_err()
        );
    }

    #[test]
    fn duplicates_are_rejected_with_the_canonical_name() {
        let error = RegistryTables::new(
            SUPPORTED_VERSION,
            SUPPORTED_PROTOCOL,
            65_536,
            128,
            [
                ("minecraft:air".to_string(), 0u32),
                ("minecraft:air".to_string(), 0),
            ],
            [],
        )
        .unwrap_err();
        assert_eq!(
            error,
            RegistryError::DuplicateKey("minecraft:air".to_string())
        );
    }

    #[test]
    fn direct_widths_follow_the_declared_registry_sizes() {
        let tables = tables();
        // 65_536 states need 16 bits and 128 biomes need 7. The fixture sizes
        // are invented, but chosen so both derived widths match what the real
        // registries of a provisioned 26.3 table produce, which keeps the
        // assertions meaningful without carrying any Mojang registry count
        // into the repository.
        assert_eq!(tables.state_bits(), 16);
        assert_eq!(tables.biome_bits(), 7);
        assert_eq!(ceillog2(1), 0);
        assert_eq!(ceillog2(2), 1);
        assert_eq!(ceillog2(3), 2);
        assert_eq!(ceillog2(256), 8);
        assert_eq!(ceillog2(257), 9);
        let smaller = RegistryTables::new(
            SUPPORTED_VERSION,
            SUPPORTED_PROTOCOL,
            16,
            8,
            [("minecraft:air".to_string(), 0u32)],
            [("minecraft:plains".to_string(), 0u32)],
        )
        .expect("small registries");
        assert_eq!(smaller.state_bits(), 4);
        assert_eq!(smaller.biome_bits(), 3);
    }

    #[test]
    fn classification_and_heightmap_capture_follow_the_documented_rules() {
        assert_eq!(
            StateKind::from_state_name("minecraft:water"),
            StateKind::Fluid
        );
        assert_eq!(
            StateKind::from_state_name("minecraft:mangrove_leaves[waterlogged=true]"),
            StateKind::Leaves
        );
        assert_eq!(
            StateKind::from_state_name("minecraft:deepslate"),
            StateKind::Solid
        );
        for kind in [
            StateKind::Air,
            StateKind::Fluid,
            StateKind::Leaves,
            StateKind::Solid,
        ] {
            assert_eq!(
                (
                    kind.captured_by(HeightmapKind::WorldSurface),
                    kind.captured_by(HeightmapKind::MotionBlocking),
                    kind.captured_by(HeightmapKind::MotionBlockingNoLeaves),
                ),
                match kind {
                    StateKind::Air => (false, false, false),
                    StateKind::Fluid => (true, false, false),
                    StateKind::Leaves => (true, true, false),
                    StateKind::Solid => (true, true, true),
                }
            );
        }
        assert_eq!(StateKind::Air.sky_attenuation(), 0);
        assert_eq!(StateKind::Leaves.sky_attenuation(), 1);
        assert_eq!(StateKind::Fluid.sky_attenuation(), 1);
        assert_eq!(StateKind::Solid.sky_attenuation(), 15);
    }

    #[test]
    fn kind_overrides_retarget_both_lookups() {
        let tables = tables()
            .with_kind_override("minecraft:stone", StateKind::Leaves)
            .expect("override");
        assert_eq!(
            tables.state("minecraft:stone").unwrap().kind,
            StateKind::Leaves
        );
        assert_eq!(tables.kind(1), Some(StateKind::Leaves));
        assert!(
            tables
                .clone()
                .with_kind_override("minecraft:glow_lichen", StateKind::Air)
                .is_err()
        );
        // An override cannot make the id lookup ambiguous: each id keeps the
        // class of the entry that carries it.
        assert_eq!(tables.state("minecraft:air").unwrap().kind, StateKind::Air);
    }

    #[test]
    fn provisioned_json_round_trips_the_same_validation() {
        let text = r#"{
            "version": "26.3",
            "protocol": 777,
            "block_state_count": 65536,
            "biome_count": 128,
            "block_states": {"minecraft:air": 0, "minecraft:stone": 1, "minecraft:kelp_plant": 40},
            "biomes": {"minecraft:plains": 0},
            "state_kinds": {"minecraft:kelp_plant": "leaves"}
        }"#;
        let tables = RegistryTables::from_provisioned(text).expect("provisioned table");
        assert_eq!(tables.version(), SUPPORTED_VERSION);
        assert_eq!(tables.protocol(), SUPPORTED_PROTOCOL);
        assert_eq!(
            tables.state("minecraft:kelp_plant").unwrap().kind,
            StateKind::Leaves
        );
        assert_eq!(tables.biome("minecraft:plains"), Ok(0));
        for broken in [
            r#"{"version": "26.3"}"#,
            r#"{"version": "26.2", "protocol": 777, "block_state_count": 1, "biome_count": 1, "block_states": {}, "biomes": {}}"#,
            r#"not json"#,
            r#"{"version": "26.3", "protocol": 777, "block_state_count": 1, "biome_count": 1, "block_states": [], "biomes": {}}"#,
            r#"{"version": "26.3", "protocol": 777, "block_state_count": 2, "biome_count": 1, "block_states": {"minecraft:air": 5}, "biomes": {}}"#,
            r#"{"version": "26.3", "protocol": 777, "block_state_count": 2, "biome_count": 1, "block_states": {"minecraft:air": 0}, "biomes": {}, "state_kinds": {"minecraft:air": "plasma"}}"#,
        ] {
            assert!(
                RegistryTables::from_provisioned(broken).is_err(),
                "table `{broken}` must be rejected"
            );
        }
    }
}
