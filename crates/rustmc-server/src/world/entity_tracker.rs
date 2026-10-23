//! Spatial entity indexing and distance-based view tracking.
//!
//! Maintains spatial indices (by chunk) and observer view sets so that
//! visibility events (`Spawn`, `Destroy`, `Move`) are deterministically emitted
//! when entities enter, leave, or move within an observer's tracking range.
//! Chunk spatial queries bound candidate evaluation to neighboring chunks.

use std::collections::{BTreeMap, BTreeSet};

/// Maximum tracking range among all entity types (in blocks).
pub const MAX_TRACKING_RANGE: f64 = 64.0;

/// Entity categories and their canonical tracking ranges in blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntityType {
    Player,
    Monster,
    Animal,
    Item,
    Projectile,
}

impl EntityType {
    /// Tracking radius in blocks for this entity type.
    pub const fn tracking_range(self) -> f64 {
        match self {
            Self::Player => 64.0,
            Self::Monster => 48.0,
            Self::Animal => 32.0,
            Self::Item => 16.0,
            Self::Projectile => 32.0,
        }
    }

    /// Whether entities of this type act as observers of other entities.
    pub const fn is_observer(self) -> bool {
        matches!(self, Self::Player)
    }

    /// Maximum tracking range across all entity types.
    pub const fn max_tracking_range() -> f64 {
        MAX_TRACKING_RANGE
    }
}

/// Monotonic positive entity identifier.
pub type EntityId = u32;

/// Representation of an entity registered in the spatial tracker.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackedEntity {
    pub id: EntityId,
    pub entity_type: EntityType,
    pub pos: (f64, f64, f64),
    pub chunk: (i32, i32),
}

/// Tracking visibility events dispatched to observers.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackingEvent {
    Spawn {
        observer: EntityId,
        entity: EntityId,
        pos: (f64, f64, f64),
        entity_type: EntityType,
    },
    Destroy {
        observer: EntityId,
        entity: EntityId,
    },
    Move {
        observer: EntityId,
        entity: EntityId,
        new_pos: (f64, f64, f64),
    },
}

/// Errors returned by spatial tracking operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackingError {
    NonFinitePosition,
    OutOfBounds,
    EntityNotFound(EntityId),
    IdOverflow,
}

impl std::fmt::Display for TrackingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFinitePosition => {
                write!(f, "coordinates contain non-finite float (NaN or Infinity)")
            }
            Self::OutOfBounds => write!(f, "coordinates exceed authoritative world boundaries"),
            Self::EntityNotFound(id) => write!(f, "entity {id} not found in registry"),
            Self::IdOverflow => write!(f, "entity id space exhausted (u32 overflow)"),
        }
    }
}

impl std::error::Error for TrackingError {}

/// Computes the chunk coordinates `(chunk_x, chunk_z)` for a 3D world position,
/// or returns `None` if the coordinates are non-finite or out of world boundaries.
pub fn try_chunk_coords(pos: (f64, f64, f64)) -> Option<(i32, i32)> {
    if !pos.0.is_finite() || !pos.1.is_finite() || !pos.2.is_finite() {
        return None;
    }
    if pos.0.abs() > 30_000_000.0 || pos.2.abs() > 30_000_000.0 || pos.1 < -2048.0 || pos.1 > 2048.0
    {
        return None;
    }
    Some((
        (pos.0.floor() as i64).div_euclid(16) as i32,
        (pos.2.floor() as i64).div_euclid(16) as i32,
    ))
}

/// Computes the chunk coordinates `(chunk_x, chunk_z)` for a 3D world position.
///
/// Panics if coordinates are non-finite or out of bounds.
pub fn chunk_coords(pos: (f64, f64, f64)) -> (i32, i32) {
    try_chunk_coords(pos).expect("valid world position required for chunk_coords")
}

/// Returns the squared Euclidean distance between two 3D positions.
fn distance_sq(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    let dz = a.2 - b.2;
    dx * dx + dy * dy + dz * dz
}

/// Checks whether an entity at `entity_pos` is within `range` of `observer_pos`.
fn is_in_range(observer_pos: (f64, f64, f64), entity_pos: (f64, f64, f64), range: f64) -> bool {
    distance_sq(observer_pos, entity_pos) <= range * range
}

/// Returns all chunk coordinates within a given bounding box around `pos` with radius `range`.
fn chunks_in_radius(pos: (f64, f64, f64), radius: f64) -> impl Iterator<Item = (i32, i32)> {
    let min_cx = ((pos.0 - radius).floor() as i64).div_euclid(16) as i32;
    let max_cx = ((pos.0 + radius).floor() as i64).div_euclid(16) as i32;
    let min_cz = ((pos.2 - radius).floor() as i64).div_euclid(16) as i32;
    let max_cz = ((pos.2 + radius).floor() as i64).div_euclid(16) as i32;
    (min_cx..=max_cx).flat_map(move |cx| (min_cz..=max_cz).map(move |cz| (cx, cz)))
}

/// Spatial entity tracker indexing entities by chunk and maintaining observer view sets.
///
/// Indexes and registries are encapsulated as private fields to prevent desynchronization.
/// Spatial candidate bounding uses chunk indices to prevent $O(N_{\text{all}})$ full table scans.
#[derive(Debug, Clone)]
pub struct SpatialEntityTracker {
    /// Chunk index mapping `(chunk_x, chunk_z)` to the entities in that chunk.
    chunk_index: BTreeMap<(i32, i32), BTreeSet<EntityId>>,
    /// Entity registry mapping `EntityId` to entity metadata and position.
    entity_registry: BTreeMap<EntityId, TrackedEntity>,
    /// Observer view sets mapping observer `EntityId` to the set of entities it observes.
    observer_view_sets: BTreeMap<EntityId, BTreeSet<EntityId>>,
    /// Monotonically increasing next entity ID (starts at 1).
    next_entity_id: EntityId,
}

impl Default for SpatialEntityTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SpatialEntityTracker {
    /// Creates a new, empty spatial entity tracker.
    pub fn new() -> Self {
        Self {
            chunk_index: BTreeMap::new(),
            entity_registry: BTreeMap::new(),
            observer_view_sets: BTreeMap::new(),
            next_entity_id: 1,
        }
    }

    /// Read-only accessor for chunk index.
    pub fn chunk_index(&self) -> &BTreeMap<(i32, i32), BTreeSet<EntityId>> {
        &self.chunk_index
    }

    /// Read-only accessor for entity registry.
    pub fn entity_registry(&self) -> &BTreeMap<EntityId, TrackedEntity> {
        &self.entity_registry
    }

    /// Read-only accessor for observer view sets.
    pub fn observer_view_sets(&self) -> &BTreeMap<EntityId, BTreeSet<EntityId>> {
        &self.observer_view_sets
    }

    /// Retrieves an entity by its ID if registered.
    pub fn get_entity(&self, id: EntityId) -> Option<&TrackedEntity> {
        self.entity_registry.get(&id)
    }

    /// Returns the set of entity IDs currently observed by the given observer.
    pub fn observed_by(&self, observer: EntityId) -> Option<&BTreeSet<EntityId>> {
        self.observer_view_sets.get(&observer)
    }

    /// Checks whether an observer is currently tracking a target entity.
    pub fn is_observing(&self, observer: EntityId, target: EntityId) -> bool {
        self.observer_view_sets
            .get(&observer)
            .is_some_and(|set| set.contains(&target))
    }

    /// Adds a new entity with the given type and initial position.
    ///
    /// Validates position finiteness and world bounds, allocates a monotonic positive integer ID,
    /// indexes the entity in the chunk index, and bounds observer discovery to neighboring chunks.
    pub fn add_entity(
        &mut self,
        entity_type: EntityType,
        pos: (f64, f64, f64),
    ) -> Result<(EntityId, Vec<TrackingEvent>), TrackingError> {
        let chunk = try_chunk_coords(pos).ok_or_else(|| {
            if !pos.0.is_finite() || !pos.1.is_finite() || !pos.2.is_finite() {
                TrackingError::NonFinitePosition
            } else {
                TrackingError::OutOfBounds
            }
        })?;

        let id = self.next_entity_id;
        self.next_entity_id = self
            .next_entity_id
            .checked_add(1)
            .ok_or(TrackingError::IdOverflow)?;

        let entity = TrackedEntity {
            id,
            entity_type,
            pos,
            chunk,
        };

        self.entity_registry.insert(id, entity);
        self.chunk_index.entry(chunk).or_default().insert(id);

        let mut events = Vec::new();

        if entity_type.is_observer() {
            self.observer_view_sets.insert(id, BTreeSet::new());
        }

        // 1. Notify nearby observers in chunks within tracking range of the new entity.
        let range = entity_type.tracking_range();
        let mut candidate_observers = BTreeSet::new();
        for ch in chunks_in_radius(pos, range) {
            if let Some(chunk_entities) = self.chunk_index.get(&ch) {
                for &e_id in chunk_entities {
                    if e_id != id && self.observer_view_sets.contains_key(&e_id) {
                        candidate_observers.insert(e_id);
                    }
                }
            }
        }

        for obs_id in candidate_observers {
            if let Some(obs) = self.entity_registry.get(&obs_id)
                && is_in_range(obs.pos, pos, range)
            {
                if let Some(view_set) = self.observer_view_sets.get_mut(&obs_id) {
                    view_set.insert(id);
                }
                events.push(TrackingEvent::Spawn {
                    observer: obs_id,
                    entity: id,
                    pos,
                    entity_type,
                });
            }
        }

        // 2. If the new entity is an observer, discover entities in chunks within max tracking range.
        if entity_type.is_observer() {
            let mut newly_observed = BTreeSet::new();
            let mut candidate_entities = BTreeSet::new();
            for ch in chunks_in_radius(pos, EntityType::max_tracking_range()) {
                if let Some(chunk_entities) = self.chunk_index.get(&ch) {
                    for &e_id in chunk_entities {
                        if e_id != id {
                            candidate_entities.insert(e_id);
                        }
                    }
                }
            }

            for other_id in candidate_entities {
                if let Some(other) = self.entity_registry.get(&other_id)
                    && is_in_range(pos, other.pos, other.entity_type.tracking_range())
                {
                    newly_observed.insert(other_id);
                    events.push(TrackingEvent::Spawn {
                        observer: id,
                        entity: other_id,
                        pos: other.pos,
                        entity_type: other.entity_type,
                    });
                }
            }
            if let Some(view_set) = self.observer_view_sets.get_mut(&id) {
                *view_set = newly_observed;
            }
        }

        Ok((id, events))
    }

    /// Moves an entity to a new position.
    ///
    /// Validates position finiteness and world bounds, updates the chunk index across boundaries,
    /// recalculates visibility using bounded chunk queries, and emits `Spawn`, `Destroy`, or `Move` events.
    pub fn move_entity(
        &mut self,
        id: EntityId,
        new_pos: (f64, f64, f64),
    ) -> Result<Vec<TrackingEvent>, TrackingError> {
        let new_chunk = try_chunk_coords(new_pos).ok_or_else(|| {
            if !new_pos.0.is_finite() || !new_pos.1.is_finite() || !new_pos.2.is_finite() {
                TrackingError::NonFinitePosition
            } else {
                TrackingError::OutOfBounds
            }
        })?;

        let (old_chunk, entity_type) = {
            let entity = self
                .entity_registry
                .get(&id)
                .ok_or(TrackingError::EntityNotFound(id))?;
            (entity.chunk, entity.entity_type)
        };

        // Update chunk index if boundary was crossed
        if old_chunk != new_chunk {
            if let Some(set) = self.chunk_index.get_mut(&old_chunk) {
                set.remove(&id);
                if set.is_empty() {
                    self.chunk_index.remove(&old_chunk);
                }
            }
            self.chunk_index.entry(new_chunk).or_default().insert(id);
        }

        // Update entity state
        if let Some(entity) = self.entity_registry.get_mut(&id) {
            entity.pos = new_pos;
            entity.chunk = new_chunk;
        }

        let mut events = Vec::new();
        let range = entity_type.tracking_range();

        // 1. Gather candidate observers from both old and new positions using chunk index bounding
        let mut candidate_observers = BTreeSet::new();
        // Any observer currently viewing this entity must be checked (in case it left range)
        for (&obs_id, view_set) in self.observer_view_sets.iter() {
            if view_set.contains(&id) {
                candidate_observers.insert(obs_id);
            }
        }
        // Observers in chunks near the new position may newly see this entity
        for ch in chunks_in_radius(new_pos, range) {
            if let Some(chunk_entities) = self.chunk_index.get(&ch) {
                for &e_id in chunk_entities {
                    if e_id != id && self.observer_view_sets.contains_key(&e_id) {
                        candidate_observers.insert(e_id);
                    }
                }
            }
        }

        for obs_id in candidate_observers {
            if obs_id == id {
                continue;
            }
            if let Some(obs) = self.entity_registry.get(&obs_id) {
                let in_range = is_in_range(obs.pos, new_pos, range);
                let was_observing = self
                    .observer_view_sets
                    .get(&obs_id)
                    .is_some_and(|s| s.contains(&id));

                if !was_observing && in_range {
                    if let Some(s) = self.observer_view_sets.get_mut(&obs_id) {
                        s.insert(id);
                    }
                    events.push(TrackingEvent::Spawn {
                        observer: obs_id,
                        entity: id,
                        pos: new_pos,
                        entity_type,
                    });
                } else if was_observing && !in_range {
                    if let Some(s) = self.observer_view_sets.get_mut(&obs_id) {
                        s.remove(&id);
                    }
                    events.push(TrackingEvent::Destroy {
                        observer: obs_id,
                        entity: id,
                    });
                } else if was_observing && in_range {
                    events.push(TrackingEvent::Move {
                        observer: obs_id,
                        entity: id,
                        new_pos,
                    });
                }
            }
        }

        // 2. If the moving entity is an observer, update its own view of other entities bounded by chunks
        if entity_type.is_observer() {
            let mut candidate_entities = BTreeSet::new();
            if let Some(view_set) = self.observer_view_sets.get(&id) {
                for &e_id in view_set {
                    candidate_entities.insert(e_id);
                }
            }
            for ch in chunks_in_radius(new_pos, EntityType::max_tracking_range()) {
                if let Some(chunk_entities) = self.chunk_index.get(&ch) {
                    for &e_id in chunk_entities {
                        if e_id != id {
                            candidate_entities.insert(e_id);
                        }
                    }
                }
            }

            let candidate_list: Vec<(EntityId, (f64, f64, f64), EntityType)> = candidate_entities
                .into_iter()
                .filter_map(|e_id| {
                    self.entity_registry
                        .get(&e_id)
                        .map(|e| (e_id, e.pos, e.entity_type))
                })
                .collect();

            if let Some(this_view_set) = self.observer_view_sets.get_mut(&id) {
                for (other_id, other_pos, other_type) in candidate_list {
                    let in_range = is_in_range(new_pos, other_pos, other_type.tracking_range());
                    let was_observing = this_view_set.contains(&other_id);

                    if !was_observing && in_range {
                        this_view_set.insert(other_id);
                        events.push(TrackingEvent::Spawn {
                            observer: id,
                            entity: other_id,
                            pos: other_pos,
                            entity_type: other_type,
                        });
                    } else if was_observing && !in_range {
                        this_view_set.remove(&other_id);
                        events.push(TrackingEvent::Destroy {
                            observer: id,
                            entity: other_id,
                        });
                    }
                }
            }
        }

        Ok(events)
    }

    /// Removes an entity by ID from the spatial tracker.
    ///
    /// Emits `Destroy` events to all observers that were currently tracking it,
    /// and unregisters the entity from the registry, chunk index, and observer view sets.
    pub fn remove_entity(&mut self, id: EntityId) -> Vec<TrackingEvent> {
        let Some(entity) = self.entity_registry.remove(&id) else {
            return Vec::new();
        };

        if let Some(set) = self.chunk_index.get_mut(&entity.chunk) {
            set.remove(&id);
            if set.is_empty() {
                self.chunk_index.remove(&entity.chunk);
            }
        }

        let mut events = Vec::new();

        // Notify observers that were viewing this entity
        for (&obs_id, view_set) in self.observer_view_sets.iter_mut() {
            if obs_id == id {
                continue;
            }
            if view_set.remove(&id) {
                events.push(TrackingEvent::Destroy {
                    observer: obs_id,
                    entity: id,
                });
            }
        }

        // If the removed entity was an observer, remove its view set
        self.observer_view_sets.remove(&id);

        events
    }
}
