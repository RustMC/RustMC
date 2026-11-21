//! Block play: breaking and placing, validated by the server and sent to
//! everyone who can see the change, plus the inventory state they depend on.
//!
//! Why: the server is the authority on every block; a client's action is a
//! request that is checked, applied, journaled and broadcast, or refused and
//! corrected on that client.
//! Done: creative inventory slots and the held hotbar slot; breaking
//! (instantly in creative, on finished digging in survival); placing the held
//! block item against the clicked face of an existing block into air, within
//! reach and not inside a player; survival placement uses up the item; Block Update to every
//! player who has the chunk and Block Changed Ack to the actor.
//! Not done: dig times and tool rules (survival basics phase), placement
//! states such as stair facing (the block's default state is placed),
//! replacing grass and other replaceable blocks, items that place a block of
//! another name.
//!
//! Sources: minecraft.wiki `Java Edition protocol/Packets` (Player Action,
//! Use Item On, Set Creative Mode Slot, Set Held Item, Block Update,
//! Acknowledge Block Change); `Attribute` (`block_interaction_range` 4.5,
//! plus 0.5 in Creative); `Player` (eye height 1.62, hitbox 0.6 × 1.8).

use super::Game;
use crate::chunk_adapter::StateKind;
use crate::discovery_java::actions::{
    PlayerActionStatus, decode_player_action, decode_use_item_on,
};
use crate::java_packets::play::clientbound;
use crate::java_wire::{pack_block_position, put_varint, read_i16_be};
use crate::net::framing::Packet;
use crate::server::connection::{ConnectionId, Outgoing};
use crate::server::join::GameMode;
use crate::server::world::view::chunk_of;
use crate::storage::journal::BlockEdit;
use crate::world::inventory::HOTBAR_SLOTS;

/// Eye height of a standing player.
pub const EYE_HEIGHT: f64 = 1.62;
/// Default `block_interaction_range` attribute.
pub const SURVIVAL_REACH: f64 = 4.5;
/// Creative mode adds 0.5 to the interaction range.
pub const CREATIVE_REACH: f64 = 5.0;
/// Extra distance allowed for the player moving between the click and the
/// server applying it; a RustMC choice, not a vanilla value.
pub const REACH_ALLOWANCE: f64 = 1.0;
/// Player hitbox width and height.
pub const PLAYER_WIDTH: f64 = 0.6;
pub const PLAYER_HEIGHT: f64 = 1.8;

/// Why a block action ends the connection (malformed input only; invalid
/// but well-formed actions are refused and corrected instead).
pub const MALFORMED: &str = "Invalid packet";

/// The block next to `pos` across `face` (0 down, 1 up, 2 north, 3 south,
/// 4 west, 5 east).
fn neighbour((x, y, z): (i32, i32, i32), face: u32) -> Option<(i32, i32, i32)> {
    Some(match face {
        0 => (x, y - 1, z),
        1 => (x, y + 1, z),
        2 => (x, y, z - 1),
        3 => (x, y, z + 1),
        4 => (x - 1, y, z),
        5 => (x + 1, y, z),
        _ => return None,
    })
}

fn within_reach(position: [f64; 3], game_mode: GameMode, (x, y, z): (i32, i32, i32)) -> bool {
    let reach = match game_mode {
        GameMode::Creative => CREATIVE_REACH,
        _ => SURVIVAL_REACH,
    } + REACH_ALLOWANCE;
    let eye = [position[0], position[1] + EYE_HEIGHT, position[2]];
    let centre = [f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5];
    let distance_sq: f64 = eye.iter().zip(centre).map(|(a, b)| (a - b) * (a - b)).sum();
    distance_sq <= reach * reach
}

/// Whether a block cell overlaps a player's hitbox.
fn overlaps([px, py, pz]: [f64; 3], (x, y, z): (i32, i32, i32)) -> bool {
    let half = PLAYER_WIDTH / 2.0;
    let (bx, by, bz) = (f64::from(x), f64::from(y), f64::from(z));
    px + half > bx
        && px - half < bx + 1.0
        && py + PLAYER_HEIGHT > by
        && py < by + 1.0
        && pz + half > bz
        && pz - half < bz + 1.0
}

fn block_update((x, y, z): (i32, i32, i32), state: u32) -> Packet {
    let mut body = pack_block_position(x, y, z).to_be_bytes().to_vec();
    put_varint(state, &mut body);
    Packet {
        id: clientbound::BLOCK_UPDATE,
        body,
    }
}

fn acknowledge(sequence: u32) -> Packet {
    let mut body = Vec::new();
    put_varint(sequence, &mut body);
    Packet {
        id: clientbound::BLOCK_CHANGED_ACK,
        body,
    }
}

impl Game {
    /// Set Creative Mode Slot; ignored outside Creative, as the client should
    /// not send it there.
    pub(super) fn creative_slot(
        &mut self,
        id: ConnectionId,
        body: &[u8],
    ) -> Result<(), &'static str> {
        let Some(player) = self.players.get_mut(&id) else {
            return Ok(());
        };
        if player.game_mode != GameMode::Creative {
            return Ok(());
        }
        player
            .inventory
            .apply_creative_slot(body, &self.ids, &self.dynamic_ids)
            .map_err(|_| MALFORMED)
    }

    /// Set Held Item: the selected hotbar slot.
    pub(super) fn carried_item(
        &mut self,
        id: ConnectionId,
        body: &[u8],
    ) -> Result<(), &'static str> {
        let Some(player) = self.players.get_mut(&id) else {
            return Ok(());
        };
        let (slot, rest) = read_i16_be(body).map_err(|_| MALFORMED)?;
        let slot = u8::try_from(slot).map_err(|_| MALFORMED)?;
        if !rest.is_empty() || !HOTBAR_SLOTS.contains(&slot) {
            return Err(MALFORMED);
        }
        player.inventory.set_held_slot(slot).map_err(|_| MALFORMED)
    }

    /// Player Action: breaking.
    pub(super) fn player_action(
        &mut self,
        id: ConnectionId,
        body: &[u8],
    ) -> Result<(), &'static str> {
        let action = decode_player_action(body).map_err(|_| MALFORMED)?;
        let Some(player) = self.players.get(&id) else {
            return Ok(());
        };
        let breaks = matches!(
            (player.game_mode, action.status),
            (GameMode::Creative, PlayerActionStatus::StartedDigging)
                | (GameMode::Survival, PlayerActionStatus::FinishedDigging)
        );
        if !breaks {
            return Ok(());
        }
        let air = self.source.tables.air_state();
        let current = self.store.block(action.pos.0, action.pos.1, action.pos.2);
        let allowed = within_reach(player.position, player.game_mode, action.pos)
            && current.is_some_and(|state| self.source.tables.kind(state) != Some(StateKind::Air));
        self.resolve(id, action.pos, allowed.then_some(air), action.sequence);
        Ok(())
    }

    /// Use Item On: placing the held block item.
    pub(super) fn use_item_on(
        &mut self,
        id: ConnectionId,
        body: &[u8],
    ) -> Result<(), &'static str> {
        let action = decode_use_item_on(body).map_err(|_| MALFORMED)?;
        let Some(target) = neighbour(action.pos, action.face) else {
            return Err(MALFORMED);
        };
        let Some(player) = self.players.get(&id) else {
            return Ok(());
        };
        let held = player
            .inventory
            .held_item()
            .map(|stack| stack.item().to_owned());
        let state = held
            .as_deref()
            .and_then(|item| self.source.tables.state(item).ok())
            .map(|entry| entry.id);
        // A real client only clicks an existing block.
        let clicked = self
            .store
            .block(action.pos.0, action.pos.1, action.pos.2)
            .is_some_and(|current| self.source.tables.kind(current) != Some(StateKind::Air));
        let free = self
            .store
            .block(target.0, target.1, target.2)
            .is_some_and(|current| self.source.tables.kind(current) == Some(StateKind::Air));
        let allowed = state.is_some()
            && clicked
            && free
            && within_reach(player.position, player.game_mode, action.pos)
            && !self
                .players
                .values()
                .any(|other| overlaps(other.position, target));
        let placed = state.filter(|_| allowed);
        if placed.is_some()
            && let Some(player) = self.players.get_mut(&id)
            && player.game_mode == GameMode::Survival
            && player.inventory.consume_held(1).is_err()
        {
            self.resolve(id, target, None, action.sequence);
            return Ok(());
        }
        self.resolve(id, target, placed, action.sequence);
        Ok(())
    }

    /// Applies `change` at `pos` (or refuses it when `None`), acknowledges
    /// `sequence` to the actor and sends the resulting state to everyone who
    /// has the chunk, the actor included, so a refused prediction is undone.
    fn resolve(
        &mut self,
        actor: ConnectionId,
        pos: (i32, i32, i32),
        change: Option<u32>,
        sequence: u32,
    ) {
        if let Some(state) = change {
            self.set_block(pos, state);
        }
        if let Some(player) = self.players.get(&actor) {
            player.handle.send(Outgoing::Packet(acknowledge(sequence)));
            if change.is_none()
                && let Some(current) = self.store.block(pos.0, pos.1, pos.2)
            {
                player
                    .handle
                    .send(Outgoing::Packet(block_update(pos, current)));
            }
        }
    }

    /// Changes one block: the store, the journal and every viewer.
    pub(super) fn set_block(&mut self, (x, y, z): (i32, i32, i32), state: u32) {
        if self.store.set_block(x, y, z, state).is_none() {
            return;
        }
        if let Err(error) = self.journal.append(BlockEdit { x, y, z, state }) {
            crate::server::log::event("journal_append_failed", &[("error", &error)]);
        }
        let chunk = chunk_of(f64::from(x), f64::from(z));
        let update = std::sync::Arc::new(block_update((x, y, z), state));
        for player in self.players.values() {
            if player.view.has_sent(chunk) {
                player
                    .handle
                    .send(Outgoing::Shared(std::sync::Arc::clone(&update)));
            }
        }
    }
}

#[cfg(test)]
mod tests;
