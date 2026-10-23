//! Spatial entity indexing and distance-based view tracking.
//!
//! Maintains spatial indices (by chunk) and observer view sets so that
//! visibility events (`Spawn`, `Destroy`, `Move`) are deterministically emitted
//! when entities enter, leave, or move within an observer's tracking range.

use std::collections::{BTreeMap, BTreeSet};

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

/// Computes the chunk coordinates `(chunk_x, chunk_z)` for a 3D world position.
pub fn chunk_coords(pos: (f64, f64, f64)) -> (i32, i32) {
    (
        (pos.0.floor() as i64).div_euclid(16) as i32,
        (pos.2.floor() as i64).div_euclid(16) as i32,
    )
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

/// Spatial entity tracker indexing entities by chunk and maintaining observer view sets.
#[derive(Debug, Clone)]
pub struct SpatialEntityTracker {
    /// Chunk index mapping `(chunk_x, chunk_z)` to the entities in that chunk.
    pub chunk_index: BTreeMap<(i32, i32), BTreeSet<EntityId>>,
    /// Entity registry mapping `EntityId` to entity metadata and position.
    pub entity_registry: BTreeMap<EntityId, TrackedEntity>,
    /// Observer view sets mapping observer `EntityId` to the set of entities it observes.
    pub observer_view_sets: BTreeMap<EntityId, BTreeSet<EntityId>>,
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

    /// Accessor for chunk index.
    pub fn chunk_index(&self) -> &BTreeMap<(i32, i32), BTreeSet<EntityId>> {
        &self.chunk_index
    }

    /// Accessor for entity registry.
    pub fn entity_registry(&self) -> &BTreeMap<EntityId, TrackedEntity> {
        &self.entity_registry
    }

    /// Accessor for observer view sets.
    pub fn observer_view_sets(&self) -> &BTreeMap<EntityId, BTreeSet<EntityId>> {
        &self.observer_view_sets
    }

    /// Retrieves an entity by its ID if registered.
    pub fn get_entity(&self, id: EntityId) -> Option<&TrackedEntity> {
        self.entity_registry.get(&id)
    }

    /// Adds a new entity with the given type and initial position.
    ///
    /// Allocates a deterministic monotonic positive integer ID, registers the entity
    /// in the chunk index and registry, updates observer view sets, and emits appropriate
    /// `Spawn` events.
    pub fn add_entity(
        &mut self,
        entity_type: EntityType,
        pos: (f64, f64, f64),
    ) -> (EntityId, Vec<TrackingEvent>) {
        let id = self.next_entity_id;
        self.next_entity_id = self
            .next_entity_id
            .checked_add(1)
            .expect("entity id overflow");

        let chunk = chunk_coords(pos);
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

        // Notify existing observers if this new entity is within their tracking range.
        for (&obs_id, obs_view_set) in self.observer_view_sets.iter_mut() {
            if obs_id == id {
                continue;
            }
            if let Some(obs) = self.entity_registry.get(&obs_id)
                && is_in_range(obs.pos, pos, entity_type.tracking_range())
            {
                obs_view_set.insert(id);
                events.push(TrackingEvent::Spawn {
                    observer: obs_id,
                    entity: id,
                    pos,
                    entity_type,
                });
            }
        }

        // If the new entity is an observer, check all existing entities in its view range.
        if entity_type.is_observer() {
            let mut newly_observed = BTreeSet::new();
            for (&other_id, other) in self.entity_registry.iter() {
                if other_id == id {
                    continue;
                }
                if is_in_range(pos, other.pos, other.entity_type.tracking_range()) {
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

        (id, events)
    }

    /// Moves an entity to a new position.
    ///
    /// Updates the chunk index if crossing chunk boundaries, recalculates distance against
    /// observers, and emits `Spawn` if newly entered radius, `Destroy` if left radius,
    /// or `Move` if staying within radius.
    pub fn move_entity(&mut self, id: EntityId, new_pos: (f64, f64, f64)) -> Vec<TrackingEvent> {
        let Some(entity) = self.entity_registry.get(&id) else {
            return Vec::new();
        };
        let old_chunk = entity.chunk;
        let entity_type = entity.entity_type;
        let new_chunk = chunk_coords(new_pos);

        // Update chunk bucket if crossed border
        if old_chunk != new_chunk {
            if let Some(set) = self.chunk_index.get_mut(&old_chunk) {
                set.remove(&id);
                if set.is_empty() {
                    self.chunk_index.remove(&old_chunk);
                }
            }
            self.chunk_index.entry(new_chunk).or_default().insert(id);
        }

        // Update entity position and chunk
        if let Some(entity) = self.entity_registry.get_mut(&id) {
            entity.pos = new_pos;
            entity.chunk = new_chunk;
        }

        let mut events = Vec::new();

        // 1. Recalculate distance against observers observing this entity.
        for (&obs_id, obs_view_set) in self.observer_view_sets.iter_mut() {
            if obs_id == id {
                continue;
            }
            if let Some(obs) = self.entity_registry.get(&obs_id) {
                let in_range = is_in_range(obs.pos, new_pos, entity_type.tracking_range());
                let was_observing = obs_view_set.contains(&id);

                if !was_observing && in_range {
                    obs_view_set.insert(id);
                    events.push(TrackingEvent::Spawn {
                        observer: obs_id,
                        entity: id,
                        pos: new_pos,
                        entity_type,
                    });
                } else if was_observing && !in_range {
                    obs_view_set.remove(&id);
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

        // 2. If the moving entity is an observer, update its own view of other entities.
        if entity_type.is_observer() {
            let other_entities: Vec<(EntityId, (f64, f64, f64), EntityType)> = self
                .entity_registry
                .iter()
                .filter(|(e_id, _)| **e_id != id)
                .map(|(&e_id, e)| (e_id, e.pos, e.entity_type))
                .collect();

            if let Some(this_view_set) = self.observer_view_sets.get_mut(&id) {
                for (other_id, other_pos, other_type) in other_entities {
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

        events
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
