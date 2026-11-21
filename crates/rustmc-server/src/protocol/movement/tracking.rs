//! Clientbound position sync, the pending-teleport tracker and the
//! per-player movement guard.

use super::*;

/// Clientbound `PlayerPositionSync` packet (0x49 in Java 26.3):
/// Encodes `teleport_id: VarInt`, `x: f64, y: f64, z: f64`,
/// `dx: f64, dy: f64, dz: f64` (velocity), `yaw: f32, pitch: f32`, `flags: i32`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerPositionSync {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub flags: i32,
}

/// Full position, velocity, orientation, and flag fields for [`PlayerPositionSync::with_velocity`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionSyncParams {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub flags: i32,
}

impl PlayerPositionSync {
    pub const PACKET_ID: u32 = PACKET_ID_PLAYER_POSITION_SYNC;

    /// Creates an absolute position synchronization packet with zero initial velocity.
    pub const fn new_absolute(
        teleport_id: i32,
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
    ) -> Self {
        Self {
            teleport_id,
            x,
            y,
            z,
            dx: 0.0,
            dy: 0.0,
            dz: 0.0,
            yaw,
            pitch,
            flags: 0,
        }
    }

    /// Creates a position synchronization packet specifying velocities and relative flags.
    pub const fn with_velocity(params: PositionSyncParams) -> Self {
        Self {
            teleport_id: params.teleport_id,
            x: params.x,
            y: params.y,
            z: params.z,
            dx: params.dx,
            dy: params.dy,
            dz: params.dz,
            yaw: params.yaw,
            pitch: params.pitch,
            flags: params.flags,
        }
    }

    /// Encodes the packet payload bytes.
    pub fn encode_body(&self, out: &mut Vec<u8>) {
        write_varint(self.teleport_id as u32, out);
        out.extend_from_slice(&self.x.to_be_bytes());
        out.extend_from_slice(&self.y.to_be_bytes());
        out.extend_from_slice(&self.z.to_be_bytes());
        out.extend_from_slice(&self.dx.to_be_bytes());
        out.extend_from_slice(&self.dy.to_be_bytes());
        out.extend_from_slice(&self.dz.to_be_bytes());
        out.extend_from_slice(&self.yaw.to_be_bytes());
        out.extend_from_slice(&self.pitch.to_be_bytes());
        out.extend_from_slice(&self.flags.to_be_bytes());
    }

    /// Encodes packet ID 0x49 plus the body bytes (unframed).
    pub fn encode_packet(&self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(70);
        write_varint(Self::PACKET_ID, &mut packet);
        self.encode_body(&mut packet);
        packet
    }

    /// Encodes an outer-framed packet (length VarInt + packet ID 0x49 + payload).
    pub fn encode_framed(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(65);
        self.encode_body(&mut body);
        frame_packet(Self::PACKET_ID, &body)
    }

    /// Decodes the payload bytes into `PlayerPositionSync`.
    pub fn decode_body(input: &[u8]) -> Result<Self, MovementError> {
        let (teleport_id, offset) = read_varint(input)?;
        let remaining = &input[offset..];
        if remaining.len() != 60 {
            return Err(MovementError::InvalidPacketLength {
                expected: offset + 60,
                received: input.len(),
            });
        }

        let (x, rest) = read_f64(remaining)?;
        let (y, rest) = read_f64(rest)?;
        let (z, rest) = read_f64(rest)?;
        let (dx, rest) = read_f64(rest)?;
        let (dy, rest) = read_f64(rest)?;
        let (dz, rest) = read_f64(rest)?;
        let (yaw, rest) = read_f32(rest)?;
        let (pitch, rest) = read_f32(rest)?;
        let (flags, _) = read_i32(rest)?;

        if !x.is_finite()
            || !y.is_finite()
            || !z.is_finite()
            || !dx.is_finite()
            || !dy.is_finite()
            || !dz.is_finite()
            || !yaw.is_finite()
            || !pitch.is_finite()
        {
            return Err(MovementError::NonFiniteFloat);
        }

        Ok(Self {
            teleport_id,
            x,
            y,
            z,
            dx,
            dy,
            dz,
            yaw,
            pitch,
            flags,
        })
    }

    /// Decodes an outer-framed packet into `PlayerPositionSync`.
    pub fn decode_framed(input: &[u8]) -> Result<Self, MovementError> {
        let (packet_id, body) = unframe_packet(input)?;
        if packet_id != Self::PACKET_ID {
            return Err(MovementError::UnexpectedPacketId {
                expected: Self::PACKET_ID,
                received: packet_id,
            });
        }
        Self::decode_body(body)
    }
}

/// Information recorded for a pending teleport awaiting client confirmation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingTeleport {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub tick: u64,
}

/// Tracks sequential teleport confirmations and guards movement states.
#[derive(Debug, Clone, PartialEq)]
pub struct TeleportTracker {
    pub(super) next_id: i32,
    pub(super) pending: BTreeMap<i32, PendingTeleport>,
    pub(super) latest_issued_id: Option<i32>,
    pub(super) last_confirmed_id: Option<i32>,
}

impl Default for TeleportTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl TeleportTracker {
    /// Creates a new `TeleportTracker` with sequential IDs starting at 1.
    pub const fn new() -> Self {
        Self {
            next_id: 1,
            pending: BTreeMap::new(),
            latest_issued_id: None,
            last_confirmed_id: None,
        }
    }

    /// Issues the next sequential teleport ID.
    pub fn next_teleport_id(&mut self) -> i32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id <= 0 {
            self.next_id = 1;
        }
        id
    }

    /// Records a pending teleport with expected target coordinates and tick.
    pub fn register_teleport(&mut self, teleport_id: i32, target: (f64, f64, f64), tick: u64) {
        // Enforce bounded memory retention
        if self.pending.len() >= MAX_PENDING_TELEPORTS
            && let Some(&oldest_id) = self.pending.keys().next()
        {
            self.pending.remove(&oldest_id);
        }

        self.latest_issued_id = Some(teleport_id);
        self.pending.insert(
            teleport_id,
            PendingTeleport {
                teleport_id,
                x: target.0,
                y: target.1,
                z: target.2,
                tick,
            },
        );
    }

    /// Issues a new teleport ID and registers it in a single operation.
    pub fn issue_teleport(&mut self, target: (f64, f64, f64), tick: u64) -> i32 {
        let id = self.next_teleport_id();
        self.register_teleport(id, target, tick);
        id
    }

    /// Returns `true` if any teleport is currently pending confirmation.
    #[inline]
    pub fn has_pending_teleport(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Alias for `has_pending_teleport`.
    #[inline]
    pub fn is_teleport_pending(&self) -> bool {
        self.has_pending_teleport()
    }

    /// Returns a reference to the latest active pending teleport, if any.
    pub fn pending_teleport(&self) -> Option<&PendingTeleport> {
        self.latest_issued_id
            .and_then(|id| self.pending.get(&id))
            .or_else(|| self.pending.values().next_back())
    }

    /// Returns the number of currently pending teleports.
    #[inline]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Returns the last confirmed teleport ID, if any.
    #[inline]
    pub const fn last_confirmed_id(&self) -> Option<i32> {
        self.last_confirmed_id
    }

    /// Asserts that normal movement updates are permitted (no teleport pending).
    pub fn validate_movement_allowed(&self) -> Result<(), TeleportError> {
        if let Some(pending) = self.pending_teleport() {
            return Err(TeleportError::MovementWhilePending {
                pending_id: pending.teleport_id,
            });
        }
        Ok(())
    }

    /// Accepts a client teleport confirmation.
    ///
    /// Clears confirmed and superseded pending teleports, updating `last_confirmed_id`.
    /// Rejects obsolete or unknown teleport IDs.
    pub fn accept_teleport(&mut self, teleport_id: i32) -> Result<PendingTeleport, TeleportError> {
        if let Some(last) = self.last_confirmed_id
            && teleport_id <= last
        {
            return Err(TeleportError::ObsoleteTeleportId {
                id: teleport_id,
                latest: self.latest_issued_id.unwrap_or(last),
            });
        }

        if self.pending.is_empty() {
            return Err(TeleportError::NoPendingTeleport);
        }

        let latest_id = self.latest_issued_id.unwrap_or(teleport_id);
        let Entry::Occupied(entry) = self.pending.entry(teleport_id) else {
            return Err(TeleportError::UnknownTeleportId { id: teleport_id });
        };

        if teleport_id < latest_id {
            return Err(TeleportError::ObsoleteTeleportId {
                id: teleport_id,
                latest: latest_id,
            });
        }

        let confirmed = entry.remove();
        // Clear any prior pending teleports up to this confirmed ID
        self.pending.retain(|&id, _| id > teleport_id);
        self.last_confirmed_id = Some(teleport_id);
        Ok(confirmed)
    }

    /// Resets all teleport tracker state.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.latest_issued_id = None;
        self.last_confirmed_id = None;
    }
}

/// Result of processing a validated movement or teleport packet.
#[derive(Debug, Clone, PartialEq)]
pub enum MovementOutcome {
    TeleportConfirmed(PendingTeleport),
    PositionUpdated {
        x: f64,
        y: f64,
        z: f64,
        displacement: f64,
    },
    PositionAndRotationUpdated {
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
        displacement: f64,
    },
    RotationUpdated {
        yaw: f32,
        pitch: f32,
    },
    StatusUpdated {
        on_ground: bool,
    },
}

/// Movement speed, distance, and authoritative position guard.
#[derive(Debug, Clone, PartialEq)]
pub struct MovementGuard {
    pub(super) authoritative_pos: (f64, f64, f64),
    pub(super) tick_baseline_pos: (f64, f64, f64),
    pub(super) accumulated_tick_displacement: f64,
    pub(super) authoritative_rot: (f32, f32),
    pub(super) authoritative_on_ground: bool,
    pub(super) teleport_tracker: TeleportTracker,
    pub(super) max_displacement: f64,
}

impl MovementGuard {
    /// Creates a new `MovementGuard` at the given initial spawn coordinate.
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self {
            authoritative_pos: (x, y, z),
            tick_baseline_pos: (x, y, z),
            accumulated_tick_displacement: 0.0,
            authoritative_rot: (0.0, 0.0),
            authoritative_on_ground: true,
            teleport_tracker: TeleportTracker::new(),
            max_displacement: MAX_DISPLACEMENT_PER_TICK,
        }
    }

    /// Creates a new `MovementGuard` with initial position and orientation.
    pub fn with_rotation(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Self {
        Self {
            authoritative_pos: (x, y, z),
            tick_baseline_pos: (x, y, z),
            accumulated_tick_displacement: 0.0,
            authoritative_rot: (normalize_yaw(yaw), normalize_pitch(pitch)),
            authoritative_on_ground: true,
            teleport_tracker: TeleportTracker::new(),
            max_displacement: MAX_DISPLACEMENT_PER_TICK,
        }
    }

    /// Advances to the next server tick, resetting accumulated per-tick displacement.
    pub fn advance_tick(&mut self) {
        self.tick_baseline_pos = self.authoritative_pos;
        self.accumulated_tick_displacement = 0.0;
    }

    /// Returns accumulated displacement in the current server tick.
    #[inline]
    pub const fn accumulated_tick_displacement(&self) -> f64 {
        self.accumulated_tick_displacement
    }

    #[inline]
    pub const fn position(&self) -> (f64, f64, f64) {
        self.authoritative_pos
    }

    #[inline]
    pub const fn rotation(&self) -> (f32, f32) {
        self.authoritative_rot
    }

    #[inline]
    pub const fn on_ground(&self) -> bool {
        self.authoritative_on_ground
    }

    #[inline]
    pub const fn teleport_tracker(&self) -> &TeleportTracker {
        &self.teleport_tracker
    }

    #[inline]
    pub fn teleport_tracker_mut(&mut self) -> &mut TeleportTracker {
        &mut self.teleport_tracker
    }

    #[inline]
    pub const fn max_displacement(&self) -> f64 {
        self.max_displacement
    }

    #[inline]
    pub fn set_max_displacement(&mut self, threshold: f64) {
        self.max_displacement = threshold;
    }

    /// Initiates a server-authoritative teleport, recording it in the tracker
    /// and returning an encoded `PlayerPositionSync` ready to frame and send.
    pub fn trigger_teleport(&mut self, target: (f64, f64, f64), tick: u64) -> PlayerPositionSync {
        let teleport_id = self.teleport_tracker.issue_teleport(target, tick);
        PlayerPositionSync::new_absolute(
            teleport_id,
            target.0,
            target.1,
            target.2,
            self.authoritative_rot.0,
            self.authoritative_rot.1,
        )
    }

    /// Handles client acceptance of a pending teleport (`AcceptTeleportation`).
    ///
    /// Confirms the teleport in the tracker and updates authoritative coordinates to target.
    pub fn handle_accept_teleport(
        &mut self,
        teleport_id: i32,
    ) -> Result<PendingTeleport, TeleportError> {
        let confirmed = self.teleport_tracker.accept_teleport(teleport_id)?;
        self.authoritative_pos = (confirmed.x, confirmed.y, confirmed.z);
        self.tick_baseline_pos = self.authoritative_pos;
        self.accumulated_tick_displacement = 0.0;
        Ok(confirmed)
    }

    /// Creates an authoritative reset `PlayerPositionSync` packet to restore client coordinate.
    pub fn create_position_sync_reset(&mut self, tick: u64) -> PlayerPositionSync {
        self.trigger_teleport(self.authoritative_pos, tick)
    }

    pub(super) fn reject_if_teleport_pending(&self) -> Result<(), MovementValidationError> {
        if let Some(pending) = self.teleport_tracker.pending_teleport() {
            return Err(MovementValidationError::TeleportPending {
                pending_id: pending.teleport_id,
                expected: (pending.x, pending.y, pending.z),
            });
        }
        Ok(())
    }

    /// Validates and applies a position-only update (`MovePlayerPos`).
    pub fn validate_and_apply_pos(
        &mut self,
        new_x: f64,
        new_y: f64,
        new_z: f64,
        on_ground: bool,
    ) -> Result<f64, MovementValidationError> {
        if !new_x.is_finite() || !new_y.is_finite() || !new_z.is_finite() {
            return Err(MovementValidationError::NonFiniteCoordinate);
        }

        self.reject_if_teleport_pending()?;

        let step_displacement = calculate_displacement(
            self.authoritative_pos.0,
            self.authoritative_pos.1,
            self.authoritative_pos.2,
            new_x,
            new_y,
            new_z,
        );

        let new_accumulated = self.accumulated_tick_displacement + step_displacement;
        if new_accumulated > self.max_displacement {
            return Err(MovementValidationError::SpeedExceeded {
                displacement: new_accumulated,
                max_allowed: self.max_displacement,
                reset_to: self.authoritative_pos,
            });
        }

        self.accumulated_tick_displacement = new_accumulated;
        self.authoritative_pos = (new_x, new_y, new_z);
        self.authoritative_on_ground = on_ground;
        Ok(step_displacement)
    }

    /// Validates and applies a position and rotation update (`MovePlayerPosRot`).
    pub fn validate_and_apply_pos_rot(
        &mut self,
        new_x: f64,
        new_y: f64,
        new_z: f64,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    ) -> Result<f64, MovementValidationError> {
        if !new_x.is_finite() || !new_y.is_finite() || !new_z.is_finite() {
            return Err(MovementValidationError::NonFiniteCoordinate);
        }
        if !yaw.is_finite() || !pitch.is_finite() {
            return Err(MovementValidationError::NonFiniteRotation);
        }

        self.reject_if_teleport_pending()?;

        let step_displacement = calculate_displacement(
            self.authoritative_pos.0,
            self.authoritative_pos.1,
            self.authoritative_pos.2,
            new_x,
            new_y,
            new_z,
        );

        let new_accumulated = self.accumulated_tick_displacement + step_displacement;
        if new_accumulated > self.max_displacement {
            return Err(MovementValidationError::SpeedExceeded {
                displacement: new_accumulated,
                max_allowed: self.max_displacement,
                reset_to: self.authoritative_pos,
            });
        }

        self.accumulated_tick_displacement = new_accumulated;
        self.authoritative_pos = (new_x, new_y, new_z);
        self.authoritative_rot = (yaw, pitch);
        self.authoritative_on_ground = on_ground;
        Ok(step_displacement)
    }

    /// Validates and applies a rotation-only update (`MovePlayerRot`).
    pub fn validate_and_apply_rot(
        &mut self,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    ) -> Result<(), MovementValidationError> {
        if !yaw.is_finite() || !pitch.is_finite() {
            return Err(MovementValidationError::NonFiniteRotation);
        }

        self.reject_if_teleport_pending()?;

        self.authoritative_rot = (yaw, pitch);
        self.authoritative_on_ground = on_ground;
        Ok(())
    }

    /// Validates and applies a status-only update (`MovePlayerStatus`).
    pub fn validate_and_apply_status(
        &mut self,
        on_ground: bool,
    ) -> Result<(), MovementValidationError> {
        self.reject_if_teleport_pending()?;

        self.authoritative_on_ground = on_ground;
        Ok(())
    }

    /// Dispatches a decoded serverbound movement packet through the validation guard.
    pub fn handle_movement_packet(
        &mut self,
        packet: &ServerboundMovementPacket,
    ) -> Result<MovementOutcome, MovementValidationError> {
        match packet {
            ServerboundMovementPacket::Pos(pos) => {
                let displacement =
                    self.validate_and_apply_pos(pos.x, pos.y, pos.z, pos.on_ground)?;
                Ok(MovementOutcome::PositionUpdated {
                    x: pos.x,
                    y: pos.y,
                    z: pos.z,
                    displacement,
                })
            }
            ServerboundMovementPacket::PosRot(pos_rot) => {
                let displacement = self.validate_and_apply_pos_rot(
                    pos_rot.x,
                    pos_rot.y,
                    pos_rot.z,
                    pos_rot.yaw,
                    pos_rot.pitch,
                    pos_rot.on_ground,
                )?;
                Ok(MovementOutcome::PositionAndRotationUpdated {
                    x: pos_rot.x,
                    y: pos_rot.y,
                    z: pos_rot.z,
                    yaw: pos_rot.yaw,
                    pitch: pos_rot.pitch,
                    displacement,
                })
            }
            ServerboundMovementPacket::Rot(rot) => {
                self.validate_and_apply_rot(rot.yaw, rot.pitch, rot.on_ground)?;
                Ok(MovementOutcome::RotationUpdated {
                    yaw: rot.yaw,
                    pitch: rot.pitch,
                })
            }
            ServerboundMovementPacket::Status(status) => {
                self.validate_and_apply_status(status.on_ground)?;
                Ok(MovementOutcome::StatusUpdated {
                    on_ground: status.on_ground,
                })
            }
            ServerboundMovementPacket::AcceptTeleport(accept) => {
                let confirmed = self.handle_accept_teleport(accept.teleport_id)?;
                Ok(MovementOutcome::TeleportConfirmed(confirmed))
            }
        }
    }

    /// Handles an incoming raw packet by ID and body payload.
    pub fn handle_incoming_frame(
        &mut self,
        packet_id: u32,
        body: &[u8],
    ) -> Result<MovementOutcome, MovementValidationError> {
        let packet = decode_serverbound_movement_packet(packet_id, body)?;
        self.handle_movement_packet(&packet)
    }
}
