//! Which entities each player sees: the visual tracker that turns
//! entity movement into spawn, update and remove packets.

use super::*;

/// Complete tracked entity state registered in the visual tracker.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackedEntityState {
    pub entity_id: i32,
    pub uuid: [u8; 16],
    pub entity_type: i32,
    pub position: [f64; 3],
    pub pitch: u8,
    pub yaw: u8,
    pub head_yaw: u8,
    pub data: i32,
    pub metadata: Vec<EntityMetadataEntry>,
}

impl TrackedEntityState {
    /// Creates a new tracked entity state.
    pub fn new(fields: EntitySpawnFields) -> Result<Self, EntityProtocolError> {
        Self::with_metadata(fields, Vec::new())
    }

    /// Creates a tracked entity state with initial metadata entries.
    pub fn with_metadata(
        fields: EntitySpawnFields,
        metadata: Vec<EntityMetadataEntry>,
    ) -> Result<Self, EntityProtocolError> {
        validate_coordinates(fields.position)?;
        Ok(Self {
            entity_id: fields.entity_id,
            uuid: fields.uuid,
            entity_type: fields.entity_type,
            position: fields.position,
            pitch: fields.pitch,
            yaw: fields.yaw,
            head_yaw: fields.head_yaw,
            data: fields.data,
            metadata,
        })
    }

    /// Constructs the corresponding `SpawnEntityPacket`.
    pub fn spawn_packet(&self) -> SpawnEntityPacket {
        SpawnEntityPacket {
            entity_id: self.entity_id,
            uuid: self.uuid,
            entity_type: self.entity_type,
            position: self.position,
            pitch: self.pitch,
            yaw: self.yaw,
            head_yaw: self.head_yaw,
            data: self.data,
        }
    }

    /// Constructs the corresponding `SetEntityDataPacket`.
    pub fn metadata_packet(&self) -> SetEntityDataPacket {
        SetEntityDataPacket {
            entity_id: self.entity_id,
            metadata: self.metadata.clone(),
        }
    }
}

/// Visual tracking events emitted to observers when visibility changes.
#[derive(Debug, Clone, PartialEq)]
pub enum EntityTrackerEvent {
    /// Entity entered view radius: emit spawn packet followed by entity data packet.
    Spawn {
        spawn_packet: SpawnEntityPacket,
        data_packet: SetEntityDataPacket,
    },
    /// Entity left view radius: emit despawn packet.
    Despawn(RemoveEntitiesPacket),
}

impl EntityTrackerEvent {
    pub fn is_spawn(&self) -> bool {
        matches!(self, Self::Spawn { .. })
    }

    pub fn is_despawn(&self) -> bool {
        matches!(self, Self::Despawn(_))
    }
}

/// State of an observer player tracked in the visual tracker.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackedPlayer {
    pub player_id: i32,
    pub position: [f64; 3],
    pub view_radius: f64,
    pub visible_entities: BTreeSet<i32>,
}

/// Distance-based entity visual tracker for Minecraft Java Edition.
///
/// Maintains visible entity sets per player based on view radius.
/// Emits `SpawnEntityPacket` and `SetEntityDataPacket` on entry into view radius,
/// and `RemoveEntitiesPacket` on exit.
#[derive(Debug, Clone, Default)]
pub struct EntityVisualTracker {
    pub(super) players: BTreeMap<i32, TrackedPlayer>,
    pub(super) entities: BTreeMap<i32, TrackedEntityState>,
}

impl EntityVisualTracker {
    /// Creates a new empty `EntityVisualTracker`.
    pub fn new() -> Self {
        Self {
            players: BTreeMap::new(),
            entities: BTreeMap::new(),
        }
    }

    /// Registers a new player with an initial position and view radius.
    ///
    /// Evaluates existing entities and emits `SpawnEntityPacket` and `SetEntityDataPacket`
    /// for all entities within the player's view radius.
    pub fn add_player(
        &mut self,
        player_id: i32,
        position: [f64; 3],
        view_radius: f64,
    ) -> Result<Vec<EntityTrackerEvent>, EntityProtocolError> {
        validate_coordinates(position)?;
        if !view_radius.is_finite() || view_radius < 0.0 {
            return Err(EntityProtocolError::InvalidRadius);
        }

        let mut visible = BTreeSet::new();
        let mut events = Vec::new();

        for entity in self.entities.values() {
            if is_in_radius(position, entity.position, view_radius) {
                visible.insert(entity.entity_id);
                events.push(EntityTrackerEvent::Spawn {
                    spawn_packet: entity.spawn_packet(),
                    data_packet: entity.metadata_packet(),
                });
            }
        }

        self.players.insert(
            player_id,
            TrackedPlayer {
                player_id,
                position,
                view_radius,
                visible_entities: visible,
            },
        );

        Ok(events)
    }

    /// Removes a player from the tracker, returning despawn events for any tracked entities.
    pub fn remove_player(&mut self, player_id: i32) -> Option<RemoveEntitiesPacket> {
        let player = self.players.remove(&player_id)?;
        if player.visible_entities.is_empty() {
            None
        } else {
            Some(RemoveEntitiesPacket::new(
                player.visible_entities.into_iter().collect(),
            ))
        }
    }

    /// Updates an observer player's position and recalculates visibility.
    ///
    /// Emits `Spawn` events for newly visible entities and `Despawn` events for
    /// entities that left the player's radius.
    pub fn update_player_position(
        &mut self,
        player_id: i32,
        new_position: [f64; 3],
    ) -> Result<Vec<EntityTrackerEvent>, EntityProtocolError> {
        validate_coordinates(new_position)?;

        let player = self
            .players
            .get_mut(&player_id)
            .ok_or(EntityProtocolError::PlayerNotFound(player_id))?;
        player.position = new_position;
        let radius = player.view_radius;

        let mut events = Vec::new();
        let mut newly_visible = Vec::new();
        let mut newly_hidden = Vec::new();

        for (&entity_id, entity) in &self.entities {
            let was_visible = player.visible_entities.contains(&entity_id);
            let in_range = is_in_radius(new_position, entity.position, radius);

            if !was_visible && in_range {
                newly_visible.push(entity.clone());
            } else if was_visible && !in_range {
                newly_hidden.push(entity_id);
            }
        }

        for entity in newly_visible {
            player.visible_entities.insert(entity.entity_id);
            events.push(EntityTrackerEvent::Spawn {
                spawn_packet: entity.spawn_packet(),
                data_packet: entity.metadata_packet(),
            });
        }

        if !newly_hidden.is_empty() {
            for &id in &newly_hidden {
                player.visible_entities.remove(&id);
            }
            events.push(EntityTrackerEvent::Despawn(RemoveEntitiesPacket::new(
                newly_hidden,
            )));
        }

        Ok(events)
    }

    /// Updates an observer player's view radius and recalculates visibility.
    pub fn update_player_radius(
        &mut self,
        player_id: i32,
        new_radius: f64,
    ) -> Result<Vec<EntityTrackerEvent>, EntityProtocolError> {
        if !new_radius.is_finite() || new_radius < 0.0 {
            return Err(EntityProtocolError::InvalidRadius);
        }

        let player = self
            .players
            .get_mut(&player_id)
            .ok_or(EntityProtocolError::PlayerNotFound(player_id))?;
        player.view_radius = new_radius;
        let pos = player.position;

        let mut events = Vec::new();
        let mut newly_visible = Vec::new();
        let mut newly_hidden = Vec::new();

        for (&entity_id, entity) in &self.entities {
            let was_visible = player.visible_entities.contains(&entity_id);
            let in_range = is_in_radius(pos, entity.position, new_radius);

            if !was_visible && in_range {
                newly_visible.push(entity.clone());
            } else if was_visible && !in_range {
                newly_hidden.push(entity_id);
            }
        }

        for entity in newly_visible {
            player.visible_entities.insert(entity.entity_id);
            events.push(EntityTrackerEvent::Spawn {
                spawn_packet: entity.spawn_packet(),
                data_packet: entity.metadata_packet(),
            });
        }

        if !newly_hidden.is_empty() {
            for &id in &newly_hidden {
                player.visible_entities.remove(&id);
            }
            events.push(EntityTrackerEvent::Despawn(RemoveEntitiesPacket::new(
                newly_hidden,
            )));
        }

        Ok(events)
    }

    /// Registers a new entity in the world.
    ///
    /// Evaluates all players and emits `Spawn` events for observers within view radius.
    pub fn add_entity(
        &mut self,
        entity: TrackedEntityState,
    ) -> Result<Vec<(i32, EntityTrackerEvent)>, EntityProtocolError> {
        validate_coordinates(entity.position)?;
        let mut events = Vec::new();

        for player in self.players.values_mut() {
            if is_in_radius(player.position, entity.position, player.view_radius) {
                player.visible_entities.insert(entity.entity_id);
                events.push((
                    player.player_id,
                    EntityTrackerEvent::Spawn {
                        spawn_packet: entity.spawn_packet(),
                        data_packet: entity.metadata_packet(),
                    },
                ));
            }
        }

        self.entities.insert(entity.entity_id, entity);
        Ok(events)
    }

    /// Updates an entity's 3D position and updates observer visibility sets.
    pub fn update_entity_position(
        &mut self,
        entity_id: i32,
        new_position: [f64; 3],
    ) -> Result<Vec<(i32, EntityTrackerEvent)>, EntityProtocolError> {
        validate_coordinates(new_position)?;

        let entity = self
            .entities
            .get_mut(&entity_id)
            .ok_or(EntityProtocolError::EntityNotFound(entity_id))?;
        entity.position = new_position;
        let entity_clone = entity.clone();

        let mut events = Vec::new();

        for player in self.players.values_mut() {
            let was_visible = player.visible_entities.contains(&entity_id);
            let in_range = is_in_radius(player.position, new_position, player.view_radius);

            if !was_visible && in_range {
                player.visible_entities.insert(entity_id);
                events.push((
                    player.player_id,
                    EntityTrackerEvent::Spawn {
                        spawn_packet: entity_clone.spawn_packet(),
                        data_packet: entity_clone.metadata_packet(),
                    },
                ));
            } else if was_visible && !in_range {
                player.visible_entities.remove(&entity_id);
                events.push((
                    player.player_id,
                    EntityTrackerEvent::Despawn(RemoveEntitiesPacket::single(entity_id)),
                ));
            }
        }

        Ok(events)
    }

    /// Updates entity metadata and notifies all observers currently tracking this entity.
    pub fn update_entity_metadata(
        &mut self,
        entity_id: i32,
        metadata: Vec<EntityMetadataEntry>,
    ) -> Result<Vec<(i32, SetEntityDataPacket)>, EntityProtocolError> {
        let entity = self
            .entities
            .get_mut(&entity_id)
            .ok_or(EntityProtocolError::EntityNotFound(entity_id))?;
        entity.metadata = metadata.clone();

        let mut packets = Vec::new();
        for player in self.players.values() {
            if player.visible_entities.contains(&entity_id) {
                packets.push((
                    player.player_id,
                    SetEntityDataPacket::new(entity_id, metadata.clone()),
                ));
            }
        }

        Ok(packets)
    }

    /// Removes an entity from the tracker and despawns it for all observers currently viewing it.
    pub fn remove_entity(
        &mut self,
        entity_id: i32,
    ) -> Result<Vec<(i32, RemoveEntitiesPacket)>, EntityProtocolError> {
        self.entities
            .remove(&entity_id)
            .ok_or(EntityProtocolError::EntityNotFound(entity_id))?;

        let mut despawns = Vec::new();
        for player in self.players.values_mut() {
            if player.visible_entities.remove(&entity_id) {
                despawns.push((player.player_id, RemoveEntitiesPacket::single(entity_id)));
            }
        }

        Ok(despawns)
    }

    /// Returns the set of entity IDs visible to a specific player.
    pub fn visible_entities(&self, player_id: i32) -> Option<&BTreeSet<i32>> {
        self.players.get(&player_id).map(|p| &p.visible_entities)
    }

    /// Checks whether an entity is currently visible to a specific player.
    pub fn is_entity_visible_to_player(&self, player_id: i32, entity_id: i32) -> bool {
        self.players
            .get(&player_id)
            .map(|p| p.visible_entities.contains(&entity_id))
            .unwrap_or(false)
    }

    /// Returns a reference to a tracked entity state.
    pub fn get_entity(&self, entity_id: i32) -> Option<&TrackedEntityState> {
        self.entities.get(&entity_id)
    }

    /// Returns a reference to a tracked player state.
    pub fn get_player(&self, player_id: i32) -> Option<&TrackedPlayer> {
        self.players.get(&player_id)
    }

    /// Total number of tracked entities in the world.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Total number of registered observer players.
    pub fn player_count(&self) -> usize {
        self.players.len()
    }
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------
