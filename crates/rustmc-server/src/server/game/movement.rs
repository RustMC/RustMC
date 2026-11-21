//! Player movement: accepting the client's reported position and rotation,
//! within bounds the server enforces.
//!
//! Why: the client simulates its own movement, so the server must refuse
//! positions that are impossible or that would skip across the world.
//! Done: kicks for non-finite or out-of-world coordinates; moves of more
//! than [`MAX_MOVE`] blocks in one packet are undone with a teleport back;
//! moves sent before the client confirmed a teleport are ignored; the view
//! follows the player.
//! Not done: collision with blocks. It needs each block state's collision
//! shape (grass and flowers have none, slabs are half height), which the
//! official data reports do not include; until RustMC has that data a
//! check would refuse ordinary walking. Speed checks over time, fall damage
//! and flight checks belong to the survival phase.
//!
//! Sources: minecraft.wiki `Java Edition protocol/Packets` (Set Player
//! Position, Set Player Position and Rotation, Set Player Rotation,
//! Synchronize Player Position, Confirm Teleportation); `World border`
//! (the world ends 30 million blocks from the origin).

use super::{Game, reason};
use crate::protocol::movement::{ServerboundMovementPacket, decode_serverbound_movement_packet};
use crate::server::connection::{ConnectionId, Outgoing};
use crate::server::join::teleport;
use crate::server::world::view::chunk_of;

/// Largest horizontal coordinate a player may report: the world border's
/// maximum distance from the origin.
pub const MAX_HORIZONTAL: f64 = 30_000_000.0;
/// Largest vertical distance from y = 0 a player may report; RustMC policy,
/// far outside any reachable height.
pub const MAX_VERTICAL: f64 = 20_000_000.0;
/// Longest move accepted in one packet, in blocks; RustMC policy. Longer
/// moves are undone with a teleport back to the last accepted position.
pub const MAX_MOVE: f64 = 10.0;

/// Why a move ends the connection.
pub const INVALID_MOVE: &str = "Invalid move player packet received";

fn in_world([x, y, z]: [f64; 3]) -> bool {
    [x, y, z].iter().all(|v| v.is_finite())
        && x.abs() <= MAX_HORIZONTAL
        && z.abs() <= MAX_HORIZONTAL
        && y.abs() <= MAX_VERTICAL
}

impl Game {
    /// Handles the three serverbound movement packets.
    pub(super) fn move_player(
        &mut self,
        id: ConnectionId,
        packet_id: u32,
        body: &[u8],
    ) -> Result<(), &'static str> {
        let Some(player) = self.players.get_mut(&id) else {
            return Ok(());
        };
        let (position, rotation, on_ground) =
            match decode_serverbound_movement_packet(packet_id, body) {
                Ok(ServerboundMovementPacket::Pos(p)) => ([p.x, p.y, p.z], None, p.on_ground),
                Ok(ServerboundMovementPacket::PosRot(p)) => {
                    ([p.x, p.y, p.z], Some((p.yaw, p.pitch)), p.on_ground)
                }
                Ok(ServerboundMovementPacket::Rot(p)) => {
                    (player.position, Some((p.yaw, p.pitch)), p.on_ground)
                }
                _ => return Err(reason::BAD_PACKET),
            };
        let rotation_ok = rotation.is_none_or(|(yaw, pitch)| yaw.is_finite() && pitch.is_finite());
        if !in_world(position) || !rotation_ok {
            return Err(INVALID_MOVE);
        }
        if player.awaiting_teleport.is_some() {
            // Moves sent before the client confirmed the last teleport are
            // stale; vanilla ignores them the same way.
            return Ok(());
        }
        let distance_sq: f64 = position
            .iter()
            .zip(player.position)
            .map(|(new, old)| (new - old) * (new - old))
            .sum();
        if distance_sq > MAX_MOVE * MAX_MOVE {
            player.next_teleport_id = player.next_teleport_id.wrapping_add(1);
            let teleport_id = player.next_teleport_id;
            player.awaiting_teleport = Some(teleport_id);
            let back = teleport(teleport_id, player.position, player.yaw, player.pitch);
            return if player.handle.send(Outgoing::Packet(back)) {
                Ok(())
            } else {
                Err(reason::SLOW)
            };
        }
        player.position = position;
        player.on_ground = on_ground;
        player.moved = true;
        if let Some((yaw, pitch)) = rotation {
            player.yaw = yaw;
            player.pitch = pitch;
        }
        for out in player.view.recenter(chunk_of(position[0], position[2])) {
            if !player.handle.send(out) {
                return Err(reason::SLOW);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_world_ends_at_the_border_and_at_finite_numbers() {
        assert!(in_world([0.5, 64.0, 0.5]));
        assert!(in_world([MAX_HORIZONTAL, -64.0, -MAX_HORIZONTAL]));
        assert!(!in_world([MAX_HORIZONTAL + 1.0, 64.0, 0.0]));
        assert!(!in_world([0.0, MAX_VERTICAL * 2.0, 0.0]));
        assert!(!in_world([f64::NAN, 64.0, 0.0]));
        assert!(!in_world([0.0, f64::INFINITY, 0.0]));
    }
}
