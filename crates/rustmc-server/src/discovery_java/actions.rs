//! Block positions and the block action packets of the preview session:
//! Player Action, Use Item On and Block Changed Ack.

use super::*;

/// Authoritative Overworld vertical bounds (-64..=319).
pub const MIN_BLOCK_Y: i32 = -64;
pub const MAX_BLOCK_Y: i32 = 319;

/// World border horizontal limits (+-30,000,000 blocks).
pub const MIN_BLOCK_XZ: i32 = -30_000_000;
pub const MAX_BLOCK_XZ: i32 = 30_000_000;

/// Unpacks a 64-bit packed Minecraft Java block position into (x, y, z).
///
/// Java block position encoding:
/// - x: 26-bit signed integer (bits 38..=63)
/// - z: 26-bit signed integer (bits 12..=37)
/// - y: 12-bit signed integer (bits 0..=11)
#[inline]
pub fn unpack_block_pos(packed: i64) -> (i32, i32, i32) {
    unpack_block_position(packed)
}

/// Packs (x, y, z) coordinates into a 64-bit Minecraft Java block position.
#[inline]
pub fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    pack_block_position(x, y, z)
}

/// Checks if block coordinates fall within the authoritative world bounds.
#[inline]
pub fn is_valid_block_pos(x: i32, y: i32, z: i32) -> bool {
    (MIN_BLOCK_XZ..=MAX_BLOCK_XZ).contains(&x)
        && (MIN_BLOCK_Y..=MAX_BLOCK_Y).contains(&y)
        && (MIN_BLOCK_XZ..=MAX_BLOCK_XZ).contains(&z)
}

/// The status phase or action of a PlayerAction packet from the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerActionStatus {
    StartedDigging = 0,
    CancelledDigging = 1,
    FinishedDigging = 2,
    DropItemStack = 3,
    DropItem = 4,
    ShootArrowOrFinishEating = 5,
    SwapItemInHand = 6,
}

impl TryFrom<u32> for PlayerActionStatus {
    type Error = Error;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::StartedDigging),
            1 => Ok(Self::CancelledDigging),
            2 => Ok(Self::FinishedDigging),
            3 => Ok(Self::DropItemStack),
            4 => Ok(Self::DropItem),
            5 => Ok(Self::ShootArrowOrFinishEating),
            6 => Ok(Self::SwapItemInHand),
            _ => Err(Error::Malformed),
        }
    }
}

/// Play serverbound `minecraft:player_action` in the official 26.3 packet report.
pub const PLAYER_ACTION_ID: u32 = play::serverbound::PLAYER_ACTION;
/// Play serverbound `minecraft:use_item_on` in the official 26.3 packet report.
pub const USE_ITEM_ON_ID: u32 = play::serverbound::USE_ITEM_ON;
/// Play clientbound `minecraft:block_changed_ack` in the official 26.3 packet report.
pub const BLOCK_CHANGED_ACK_ID: u32 = play::clientbound::BLOCK_CHANGED_ACK;

/// Serverbound PlayerAction (ID `0x29` / 41) packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerAction {
    pub status: PlayerActionStatus,
    pub pos: (i32, i32, i32),
    pub face: u8,
    pub sequence: u32,
}

/// Decodes a serverbound `PlayerAction` packet (ID `0x29` / 41).
pub fn decode_player_action(body: &[u8]) -> Result<PlayerAction, Error> {
    let Some((status_val, prefix)) = read_varint(body)? else {
        return Err(Error::Malformed);
    };
    let status = PlayerActionStatus::try_from(status_val)?;

    let (packed_pos, mut rest) = read_i64_be(&body[prefix..])?;
    let (x, y, z) = unpack_block_pos(packed_pos);
    if !is_valid_block_pos(x, y, z) {
        return Err(Error::Malformed);
    }

    if rest.is_empty() {
        return Err(Error::Malformed);
    }
    let face = rest[0];
    if face > 5 {
        return Err(Error::Malformed);
    }
    rest = &rest[1..];

    let Some((sequence, seq_len)) = read_varint(rest)? else {
        return Err(Error::Malformed);
    };
    if seq_len != rest.len() {
        return Err(Error::Malformed);
    }

    Ok(PlayerAction {
        status,
        pos: (x, y, z),
        face,
        sequence,
    })
}

/// Encodes a `PlayerAction` packet body.
pub fn encode_player_action(action: &PlayerAction) -> Vec<u8> {
    let mut body = Vec::new();
    put_varint(action.status as u32, &mut body);
    let packed = pack_block_pos(action.pos.0, action.pos.1, action.pos.2);
    body.extend_from_slice(&packed.to_be_bytes());
    body.push(action.face);
    put_varint(action.sequence, &mut body);
    body
}

/// Serverbound UseItemOn (ID `0x42` / 66) packet.
#[derive(Debug, Clone, PartialEq)]
pub struct UseItemOn {
    pub hand: u32,
    pub pos: (i32, i32, i32),
    pub face: u32,
    pub cursor_x: f32,
    pub cursor_y: f32,
    pub cursor_z: f32,
    pub inside_block: bool,
    pub world_border_hit: bool,
    pub sequence: u32,
}

impl UseItemOn {
    pub fn cursor(&self) -> (f32, f32, f32) {
        (self.cursor_x, self.cursor_y, self.cursor_z)
    }
}

/// Decodes a serverbound `UseItemOn` packet (ID `0x42` / 66).
pub fn decode_use_item_on(body: &[u8]) -> Result<UseItemOn, Error> {
    let Some((hand, prefix)) = read_varint(body)? else {
        return Err(Error::Malformed);
    };
    if hand > 1 {
        return Err(Error::Malformed);
    }

    let (packed_pos, rest) = read_i64_be(&body[prefix..])?;
    let (x, y, z) = unpack_block_pos(packed_pos);
    if !is_valid_block_pos(x, y, z) {
        return Err(Error::Malformed);
    }

    let Some((face, face_len)) = read_varint(rest)? else {
        return Err(Error::Malformed);
    };
    if face > 5 {
        return Err(Error::Malformed);
    }
    let rest = &rest[face_len..];

    let (cursor_x, rest) = read_f32_be(rest)?;
    let (cursor_y, rest) = read_f32_be(rest)?;
    let (cursor_z, mut rest) = read_f32_be(rest)?;
    if !cursor_x.is_finite()
        || !cursor_y.is_finite()
        || !cursor_z.is_finite()
        || !(0.0..=1.0).contains(&cursor_x)
        || !(0.0..=1.0).contains(&cursor_y)
        || !(0.0..=1.0).contains(&cursor_z)
    {
        return Err(Error::Malformed);
    }

    if rest.is_empty() {
        return Err(Error::Malformed);
    }
    let inside_byte = rest[0];
    if inside_byte > 1 {
        return Err(Error::Malformed);
    }
    let inside_block = inside_byte == 1;
    rest = &rest[1..];

    if rest.is_empty() {
        return Err(Error::Malformed);
    }
    let world_border_byte = rest[0];
    if world_border_byte > 1 {
        return Err(Error::Malformed);
    }
    let world_border_hit = world_border_byte == 1;
    rest = &rest[1..];

    if rest.is_empty() {
        return Err(Error::Malformed);
    }

    // Pinned Java 26.3 (1.21.2+) ServerboundUseItemOnPacket schema:
    // hand: VarInt, pos: Position (i64), face: VarInt,
    // cursor: 3x f32, inside_block: Boolean (u8), world_border_hit: Boolean (u8), sequence: VarInt
    let Some((sequence, seq_len)) = read_varint(rest)? else {
        return Err(Error::Malformed);
    };
    if seq_len != rest.len() {
        return Err(Error::Malformed);
    }

    Ok(UseItemOn {
        hand,
        pos: (x, y, z),
        face,
        cursor_x,
        cursor_y,
        cursor_z,
        inside_block,
        world_border_hit,
        sequence,
    })
}

/// Encodes a `UseItemOn` packet body according to the pinned Java 26.3 specification.
pub fn encode_use_item_on(action: &UseItemOn) -> Vec<u8> {
    let mut body = Vec::new();
    put_varint(action.hand, &mut body);
    let packed = pack_block_pos(action.pos.0, action.pos.1, action.pos.2);
    body.extend_from_slice(&packed.to_be_bytes());
    put_varint(action.face, &mut body);
    body.extend_from_slice(&action.cursor_x.to_be_bytes());
    body.extend_from_slice(&action.cursor_y.to_be_bytes());
    body.extend_from_slice(&action.cursor_z.to_be_bytes());
    body.push(if action.inside_block { 1 } else { 0 });
    body.push(if action.world_border_hit { 1 } else { 0 });
    put_varint(action.sequence, &mut body);
    body
}

/// Emits a framed `AcknowledgeBlockChange` packet (ID `0x04`).
pub fn acknowledge_block_change(sequence: u32) -> Vec<u8> {
    let mut body = Vec::new();
    put_varint(sequence, &mut body);
    frame(BLOCK_CHANGED_ACK_ID, &body)
}

/// Emits an unframed `AcknowledgeBlockChange` packet payload: `[0x04, varint(sequence)]`.
pub fn acknowledge_block_change_packet(sequence: u32) -> Vec<u8> {
    let mut packet = Vec::new();
    put_varint(BLOCK_CHANGED_ACK_ID, &mut packet);
    put_varint(sequence, &mut packet);
    packet
}

/// Decodes an unframed `AcknowledgeBlockChange` packet payload `[0x04, varint(sequence)]`.
pub fn decode_acknowledge_block_change_packet(bytes: &[u8]) -> Result<u32, Error> {
    let Some((id, prefix)) = read_varint(bytes)? else {
        return Err(Error::Malformed);
    };
    if id != BLOCK_CHANGED_ACK_ID {
        return Err(Error::Malformed);
    }
    let rest = &bytes[prefix..];
    let Some((sequence, seq_len)) = read_varint(rest)? else {
        return Err(Error::Malformed);
    };
    if seq_len != rest.len() {
        return Err(Error::Malformed);
    }
    Ok(sequence)
}

/// Decodes a framed `AcknowledgeBlockChange` packet: `[varint(length), 0x04, varint(sequence)]`.
pub fn decode_acknowledge_block_change(bytes: &[u8]) -> Result<u32, Error> {
    let Some((len, prefix)) = read_varint(bytes)? else {
        return Err(Error::Malformed);
    };
    if prefix + len as usize != bytes.len() {
        return Err(Error::Malformed);
    }
    decode_acknowledge_block_change_packet(&bytes[prefix..])
}

pub(super) fn is_valid_coord(x: f64, y: f64, z: f64) -> bool {
    x.is_finite()
        && y.is_finite()
        && z.is_finite()
        && x.abs() <= MAX_HORIZONTAL_POSITION
        && y.abs() <= MAX_VERTICAL_POSITION
        && z.abs() <= MAX_HORIZONTAL_POSITION
}
