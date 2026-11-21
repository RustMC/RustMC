//! Block interaction packets, rollbacks and the apply step that commits
//! a validated break or place to a chunk.

use super::*;

/// Outgoing `BlockUpdatePacket` emitted to restore or update an authoritative block cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockUpdatePacket {
    pub pos: BlockPos,
    pub block_state_id: u32,
}

impl BlockUpdatePacket {
    /// Creates a new `BlockUpdatePacket`.
    pub const fn new(pos: BlockPos, block_state_id: u32) -> Self {
        Self {
            pos,
            block_state_id,
        }
    }

    /// Encodes into standard Java 26.3 framed packet bytes:
    /// `VarInt(len) + VarInt(packet_id=8) + 8-byte packed Position + VarInt(block_state_id)`.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(
            &pack_block_position(self.pos.x, self.pos.y, self.pos.z).to_be_bytes(),
        );
        put_varint(self.block_state_id, &mut body);
        frame(BLOCK_UPDATE_PACKET_ID, &body)
    }
}

/// Outgoing `AcknowledgeBlockChangePacket` acknowledging client interaction sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcknowledgeBlockChangePacket {
    pub sequence: i32,
}

impl AcknowledgeBlockChangePacket {
    /// Creates a new `AcknowledgeBlockChangePacket`.
    pub const fn new(sequence: i32) -> Self {
        Self { sequence }
    }

    /// Encodes into standard Java 26.3 framed packet bytes:
    /// `VarInt(len) + VarInt(packet_id=4) + VarInt(sequence)`.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        put_varint(self.sequence as u32, &mut body);
        frame(ACKNOWLEDGE_BLOCK_CHANGE_PACKET_ID, &body)
    }
}

/// A non-empty stack cannot be written to the wire without its numeric
/// `minecraft:item` registry ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedItemId {
    pub item: String,
}

impl fmt::Display for UnresolvedItemId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "no protocol item ID for {}", self.item)
    }
}

impl std::error::Error for UnresolvedItemId {}

/// Outgoing `ContainerSetSlotPacket` syncing an item slot into the client inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSetSlotPacket {
    pub window_id: i8,
    pub state_id: i32,
    pub slot: i16,
    pub item: Option<ItemStack>,
    pub item_id: Option<u32>,
}

impl ContainerSetSlotPacket {
    /// Creates a new `ContainerSetSlotPacket`.
    pub const fn new(
        window_id: i8,
        state_id: i32,
        slot: i16,
        item: Option<ItemStack>,
        item_id: Option<u32>,
    ) -> Self {
        Self {
            window_id,
            state_id,
            slot,
            item,
            item_id,
        }
    }

    /// Player inventory slot update (`window_id = 0`).
    pub fn player_inventory(slot: i16, item: Option<ItemStack>, item_id: Option<u32>) -> Self {
        Self::new(0, 0, slot, item, item_id)
    }

    /// Encodes into standard Java 26.3 framed packet bytes:
    /// `VarInt(len) + VarInt(packet_id=20) + VarInt(window_id) + VarInt(state_id)
    /// + Short(slot) + Slot`.
    pub fn encode(&self) -> Result<Vec<u8>, UnresolvedItemId> {
        let mut body = Vec::new();
        put_varint(i32::from(self.window_id) as u32, &mut body);
        put_varint(self.state_id as u32, &mut body);
        body.extend_from_slice(&self.slot.to_be_bytes());
        match &self.item {
            None => put_varint(0, &mut body),
            Some(stack) => {
                put_varint(u32::from(stack.count()), &mut body);
                let item_id = self.item_id.ok_or_else(|| UnresolvedItemId {
                    item: stack.item().to_owned(),
                })?;
                put_varint(item_id, &mut body);
                put_varint(0, &mut body);
                put_varint(0, &mut body);
            }
        }
        Ok(frame(CONTAINER_SET_SLOT_PACKET_ID, &body))
    }
}

/// Rollback packet set emitted when a `BlockBreakEvent` is cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakRollback {
    /// `None` when this chunk does not own the position, so the server holds no
    /// authoritative state to restore.
    pub block_update: Option<BlockUpdatePacket>,
    pub ack: Option<AcknowledgeBlockChangePacket>,
}

impl BreakRollback {
    /// Encodes all rollback packets to wire frames.
    pub fn to_packets(&self) -> Vec<Vec<u8>> {
        let mut packets: Vec<_> = self.block_update.iter().map(|u| u.encode()).collect();
        if let Some(ack) = &self.ack {
            packets.push(ack.encode());
        }
        packets
    }
}

/// Rollback packet set emitted when a `BlockPlaceEvent` is cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceRollback {
    /// `None` when this chunk does not own the position, so the server holds no
    /// authoritative state to restore.
    pub block_update: Option<BlockUpdatePacket>,
    pub inventory_sync: ContainerSetSlotPacket,
    pub ack: Option<AcknowledgeBlockChangePacket>,
}

impl PlaceRollback {
    /// Encodes all rollback packets to wire frames.
    pub fn to_packets(&self) -> Result<Vec<Vec<u8>>, UnresolvedItemId> {
        let mut packets: Vec<_> = self.block_update.iter().map(|u| u.encode()).collect();
        packets.push(self.inventory_sync.encode()?);
        if let Some(ack) = &self.ack {
            packets.push(ack.encode());
        }
        Ok(packets)
    }
}

/// Produces authoritative break rollback packets restoring the original block state.
pub fn rollback_break(event: &BlockBreakEvent) -> Option<BreakRollback> {
    rollback_break_explicit(event, block_default_state_id(event.block), None)
}

/// Produces authoritative break rollback packets with sequence acknowledgement.
pub fn rollback_break_with_ack(event: &BlockBreakEvent, sequence: i32) -> Option<BreakRollback> {
    rollback_break_explicit(event, block_default_state_id(event.block), Some(sequence))
}

pub(super) fn build_break_rollback(
    event: &BlockBreakEvent,
    state_id: u32,
    sequence: Option<i32>,
) -> BreakRollback {
    BreakRollback {
        block_update: Some(BlockUpdatePacket::new(event.pos, state_id)),
        ack: sequence.map(AcknowledgeBlockChangePacket::new),
    }
}

/// Produces authoritative break rollback with explicit block state ID and sequence.
pub fn rollback_break_explicit(
    event: &BlockBreakEvent,
    state_id: u32,
    sequence: Option<i32>,
) -> Option<BreakRollback> {
    event
        .is_cancelled()
        .then(|| build_break_rollback(event, state_id, sequence))
}

/// Produces authoritative place rollback packets restoring air and syncing client inventory.
pub fn rollback_place(event: &BlockPlaceEvent, inventory_slot: i16) -> Option<PlaceRollback> {
    rollback_place_explicit(
        event,
        block_default_state_id(Block::Air),
        inventory_slot,
        None,
    )
}

/// Produces authoritative place rollback packets with sequence acknowledgement.
pub fn rollback_place_with_ack(
    event: &BlockPlaceEvent,
    inventory_slot: i16,
    sequence: i32,
) -> Option<PlaceRollback> {
    rollback_place_explicit(
        event,
        block_default_state_id(Block::Air),
        inventory_slot,
        Some(sequence),
    )
}

pub(super) fn build_place_rollback(
    event: &BlockPlaceEvent,
    air_state_id: u32,
    inventory_slot: i16,
    sequence: Option<i32>,
) -> PlaceRollback {
    PlaceRollback {
        block_update: Some(BlockUpdatePacket::new(event.placed_pos, air_state_id)),
        inventory_sync: ContainerSetSlotPacket::player_inventory(
            inventory_slot,
            event.item_in_hand.clone(),
            None,
        ),
        ack: sequence.map(AcknowledgeBlockChangePacket::new),
    }
}

/// Produces authoritative place rollback with explicit air state ID, inventory slot, and sequence.
pub fn rollback_place_explicit(
    event: &BlockPlaceEvent,
    air_state_id: u32,
    inventory_slot: i16,
    sequence: Option<i32>,
) -> Option<PlaceRollback> {
    event
        .is_cancelled()
        .then(|| build_place_rollback(event, air_state_id, inventory_slot, sequence))
}

/// Result of an accepted world block break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockBreakOutcome {
    pub player_id: u32,
    pub pos: BlockPos,
    pub original_block: Block,
    pub drops: Vec<ItemStack>,
}

/// Result of an accepted world block placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPlaceOutcome {
    pub player_id: u32,
    pub placed_pos: BlockPos,
    pub placed_against: BlockPos,
    pub placed_block: Block,
    pub remaining_item: Option<ItemStack>,
}

/// Reads the block at `pos`, or `None` when this chunk does not own that cell.
pub(super) fn owned_cell(chunk: &Chunk, pos: BlockPos) -> Option<Block> {
    if chunk_coords_of(pos) != (chunk.x, chunk.z) || !(0..WORLD_HEIGHT as i32).contains(&pos.y) {
        return None;
    }
    let (lx, y, lz) = local_coords_of(pos);
    debug_assert!(lx < CHUNK_SIDE && lz < CHUNK_SIDE);
    chunk.block(lx, y, lz)
}

/// Authoritative block break execution pipeline.
pub fn apply_block_break(
    bus: &EventBus,
    chunk: &mut Chunk,
    player_id: u32,
    pos: BlockPos,
    sequence: Option<i32>,
    tool: Option<ItemStack>,
    default_drops: Vec<ItemStack>,
) -> Result<BlockBreakOutcome, BreakRollback> {
    let ack = sequence.map(AcknowledgeBlockChangePacket::new);
    let Some(current_block) = owned_cell(chunk, pos) else {
        return Err(BreakRollback {
            block_update: None,
            ack,
        });
    };
    if current_block == Block::Air {
        return Err(BreakRollback {
            block_update: Some(BlockUpdatePacket::new(
                pos,
                block_default_state_id(Block::Air),
            )),
            ack,
        });
    }

    let mut event = BlockBreakEvent::new(player_id, pos, current_block, tool, default_drops);
    if bus.dispatch(&mut event).is_err() {
        event.cancel();
    }

    if event.is_cancelled() {
        Err(build_break_rollback(
            &event,
            block_default_state_id(current_block),
            sequence,
        ))
    } else {
        chunk.set_world_block(
            i64::from(pos.x),
            i64::from(pos.y),
            i64::from(pos.z),
            Block::Air,
        );
        Ok(BlockBreakOutcome {
            player_id,
            pos,
            original_block: current_block,
            drops: event.drops,
        })
    }
}

/// Inputs for [`apply_block_place`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPlaceRequest {
    pub player_id: u32,
    pub placed_pos: BlockPos,
    pub placed_against: BlockPos,
    pub placed_block: Block,
    pub item_in_hand: Option<ItemStack>,
    pub inventory_slot: i16,
    pub sequence: Option<i32>,
}

/// Authoritative block place execution pipeline.
pub fn apply_block_place(
    bus: &EventBus,
    chunk: &mut Chunk,
    request: BlockPlaceRequest,
) -> Result<BlockPlaceOutcome, PlaceRollback> {
    let BlockPlaceRequest {
        player_id,
        placed_pos,
        placed_against,
        placed_block,
        item_in_hand,
        inventory_slot,
        sequence,
    } = request;
    let Some(existing) = owned_cell(chunk, placed_pos) else {
        return Err(PlaceRollback {
            block_update: None,
            inventory_sync: ContainerSetSlotPacket::player_inventory(
                inventory_slot,
                item_in_hand,
                None,
            ),
            ack: sequence.map(AcknowledgeBlockChangePacket::new),
        });
    };

    let mut event = BlockPlaceEvent::new(
        player_id,
        placed_pos,
        placed_against,
        placed_block,
        item_in_hand,
    );
    if existing != Block::Air || event.item_in_hand.is_none() || bus.dispatch(&mut event).is_err() {
        event.cancel();
    }

    if event.is_cancelled() {
        return Err(build_place_rollback(
            &event,
            block_default_state_id(existing),
            inventory_slot,
            sequence,
        ));
    }
    let Some(hand_item) = event.item_in_hand.take() else {
        event.cancel();
        return Err(build_place_rollback(
            &event,
            block_default_state_id(existing),
            inventory_slot,
            sequence,
        ));
    };
    chunk.set_world_block(
        i64::from(placed_pos.x),
        i64::from(placed_pos.y),
        i64::from(placed_pos.z),
        placed_block,
    );
    let remaining_item = match hand_item.split(1) {
        Ok((_, rest)) => rest,
        Err(unchanged) => Some(unchanged),
    };
    Ok(BlockPlaceOutcome {
        player_id,
        placed_pos,
        placed_against,
        placed_block,
        remaining_item,
    })
}
