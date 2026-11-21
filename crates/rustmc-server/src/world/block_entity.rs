//! Block entity storage, sign text state, and Java 26.3 network synchronization.
//!
//! Provides authoritative block entity state attachments indexed by [`BlockPos`],
//! full mutable state for signs (front and back [`SignSide`] with lines, color,
//! and glowing flags), and wire codecs for:
//! - Java 26.3 `ClientboundBlockEntityDataPacket` (`0x06`): position, block entity
//!   type ID, and network NBT payload.
//! - Java 26.3 `ServerboundUpdateSignPacket` (`0x3E`): position, front/back selector,
//!   and 4 validated UTF-8 lines (capped at 384 bytes each).
//!
//! Packet IDs come from the official 26.3 server's packet report and block entity
//! type IDs from its registry report (`minecraft:block_entity_type`), both produced
//! with `java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports`.

use rustc_hash::FxHashMap;
use std::fmt;

use crate::java_wire::{self, WireError, frame, put_string, put_varint};

/// Maximum byte length for a single sign text line (384 bytes UTF-8).
pub const MAX_LINE_BYTES: usize = 384;

/// Number of lines on one side of a sign.
pub const LINES_PER_SIDE: usize = 4;

/// Play clientbound `minecraft:block_entity_data` in the 26.3 packet report.
pub const PACKET_ID_BLOCK_ENTITY_DATA: u32 = 0x06;

/// Play serverbound `minecraft:sign_update` in the 26.3 packet report.
pub const PACKET_ID_UPDATE_SIGN: u32 = 0x3E;

/// `minecraft:block_entity_type` protocol IDs in the 26.3 registry report.
pub const TYPE_ID_FURNACE: u32 = 0;
pub const TYPE_ID_CHEST: u32 = 1;
pub const TYPE_ID_SIGN: u32 = 7;
pub const TYPE_ID_HANGING_SIGN: u32 = 8;

/// Block entity type identifier strings.
pub const TYPE_NAME_CHEST: &str = "minecraft:chest";
pub const TYPE_NAME_FURNACE: &str = "minecraft:furnace";
pub const TYPE_NAME_HANGING_SIGN: &str = "minecraft:hanging_sign";
pub const TYPE_NAME_SIGN: &str = "minecraft:sign";

/// 3D integer block coordinate in world space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    /// Authoritative Overworld vertical bounds (-64..=319).
    pub const MIN_Y: i32 = -64;
    pub const MAX_Y: i32 = 319;

    /// World border horizontal limits (+-30,000,000 blocks).
    pub const MIN_XZ: i32 = -30_000_000;
    pub const MAX_XZ: i32 = 30_000_000;

    #[inline]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Packs (x, y, z) into standard Java 64-bit packed block position format:
    /// - `x`: 26-bit signed integer (bits 38..=63)
    /// - `z`: 26-bit signed integer (bits 12..=37)
    /// - `y`: 12-bit signed integer (bits 0..=11)
    #[inline]
    pub const fn pack(&self) -> i64 {
        ((self.x as i64 & 0x3FF_FFFF) << 38)
            | ((self.z as i64 & 0x3FF_FFFF) << 12)
            | (self.y as i64 & 0xFFF)
    }

    /// Unpacks a 64-bit Java packed block position into (x, y, z).
    #[inline]
    pub const fn unpack(packed: i64) -> Self {
        let x = (packed >> 38) as i32;
        let y = (packed << 52 >> 52) as i32;
        let z = (packed << 26 >> 38) as i32;
        Self { x, y, z }
    }

    /// Whether coordinates fall within the authoritative world boundary.
    #[inline]
    pub const fn is_valid(&self) -> bool {
        self.x >= Self::MIN_XZ
            && self.x <= Self::MAX_XZ
            && self.y >= Self::MIN_Y
            && self.y <= Self::MAX_Y
            && self.z >= Self::MIN_XZ
            && self.z <= Self::MAX_XZ
    }
}

impl From<(i32, i32, i32)> for BlockPos {
    #[inline]
    fn from((x, y, z): (i32, i32, i32)) -> Self {
        Self::new(x, y, z)
    }
}

impl From<BlockPos> for (i32, i32, i32) {
    #[inline]
    fn from(pos: BlockPos) -> Self {
        (pos.x, pos.y, pos.z)
    }
}

/// The 16 vanilla Minecraft dye colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DyeColor {
    White,
    Orange,
    Magenta,
    LightBlue,
    Yellow,
    Lime,
    Pink,
    Gray,
    LightGray,
    Cyan,
    Purple,
    Blue,
    Brown,
    Green,
    Red,
    #[default]
    Black,
}

impl DyeColor {
    pub const ALL: [Self; 16] = [
        Self::White,
        Self::Orange,
        Self::Magenta,
        Self::LightBlue,
        Self::Yellow,
        Self::Lime,
        Self::Pink,
        Self::Gray,
        Self::LightGray,
        Self::Cyan,
        Self::Purple,
        Self::Blue,
        Self::Brown,
        Self::Green,
        Self::Red,
        Self::Black,
    ];

    /// Lowercase namespaced string identifier matching vanilla NBT.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::White => "white",
            Self::Orange => "orange",
            Self::Magenta => "magenta",
            Self::LightBlue => "light_blue",
            Self::Yellow => "yellow",
            Self::Lime => "lime",
            Self::Pink => "pink",
            Self::Gray => "gray",
            Self::LightGray => "light_gray",
            Self::Cyan => "cyan",
            Self::Purple => "purple",
            Self::Blue => "blue",
            Self::Brown => "brown",
            Self::Green => "green",
            Self::Red => "red",
            Self::Black => "black",
        }
    }

    /// Parses from a vanilla dye color string identifier.
    pub fn from_str_name(name: &str) -> Option<Self> {
        match name {
            "white" => Some(Self::White),
            "orange" => Some(Self::Orange),
            "magenta" => Some(Self::Magenta),
            "light_blue" => Some(Self::LightBlue),
            "yellow" => Some(Self::Yellow),
            "lime" => Some(Self::Lime),
            "pink" => Some(Self::Pink),
            "gray" => Some(Self::Gray),
            "light_gray" => Some(Self::LightGray),
            "cyan" => Some(Self::Cyan),
            "purple" => Some(Self::Purple),
            "blue" => Some(Self::Blue),
            "brown" => Some(Self::Brown),
            "green" => Some(Self::Green),
            "red" => Some(Self::Red),
            "black" => Some(Self::Black),
            _ => None,
        }
    }
}

/// Sign text and styling state for one side (front or back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignSide {
    pub messages: [String; LINES_PER_SIDE],
    pub color: DyeColor,
    pub has_glowing_text: bool,
}

impl Default for SignSide {
    fn default() -> Self {
        Self {
            messages: [String::new(), String::new(), String::new(), String::new()],
            color: DyeColor::Black,
            has_glowing_text: false,
        }
    }
}

impl SignSide {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads line at 0-based index `0..4`.
    pub fn line(&self, index: usize) -> Option<&str> {
        self.messages.get(index).map(String::as_str)
    }

    /// Updates line text at index `0..4`.
    /// Rejects lines exceeding 384 bytes UTF-8.
    pub fn set_line(&mut self, index: usize, text: impl Into<String>) -> Result<(), SignError> {
        if index >= LINES_PER_SIDE {
            return Err(SignError::InvalidLineIndex(index));
        }
        let s = text.into();
        if s.len() > MAX_LINE_BYTES {
            return Err(SignError::LineTooLong {
                line: index,
                bytes: s.len(),
            });
        }
        self.messages[index] = s;
        Ok(())
    }

    pub fn set_color(&mut self, color: DyeColor) {
        self.color = color;
    }

    pub fn set_glowing(&mut self, glowing: bool) {
        self.has_glowing_text = glowing;
    }

    pub fn clear(&mut self) {
        self.messages = [String::new(), String::new(), String::new(), String::new()];
    }

    pub fn to_nbt(&self) -> NbtTag {
        let messages = self
            .messages
            .iter()
            .map(|m| NbtTag::String(m.clone()))
            .collect();
        NbtTag::Compound(vec![
            (
                "color".to_string(),
                NbtTag::String(self.color.as_str().to_string()),
            ),
            (
                "has_glowing_text".to_string(),
                NbtTag::Byte(if self.has_glowing_text { 1 } else { 0 }),
            ),
            ("messages".to_string(), NbtTag::List(messages)),
        ])
    }

    pub fn from_compound(entries: &[(String, NbtTag)]) -> Result<Self, BlockEntityError> {
        let mut messages = [String::new(), String::new(), String::new(), String::new()];
        let mut color = DyeColor::Black;
        let mut has_glowing_text = false;

        for (k, v) in entries {
            match k.as_str() {
                "color" => {
                    if let Some(s) = v.as_str() {
                        color = DyeColor::from_str_name(s).unwrap_or(DyeColor::Black);
                    }
                }
                "has_glowing_text" => {
                    if let Some(b) = v.as_bool() {
                        has_glowing_text = b;
                    }
                }
                "messages" => {
                    if let Some(list) = v.as_list() {
                        for (i, item) in list.iter().take(LINES_PER_SIDE).enumerate() {
                            if let Some(text) = item.as_str() {
                                if text.len() > MAX_LINE_BYTES {
                                    return Err(BlockEntityError::LineTooLong(text.len()));
                                }
                                messages[i] = text.to_string();
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(Self {
            messages,
            color,
            has_glowing_text,
        })
    }
}

/// Sign block entity state storing front/back sides and waxed status.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignBlockEntity {
    pub front_text: SignSide,
    pub back_text: SignSide,
    pub is_waxed: bool,
}

impl SignBlockEntity {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn side(&self, is_front: bool) -> &SignSide {
        if is_front {
            &self.front_text
        } else {
            &self.back_text
        }
    }

    #[inline]
    pub fn side_mut(&mut self, is_front: bool) -> &mut SignSide {
        if is_front {
            &mut self.front_text
        } else {
            &mut self.back_text
        }
    }

    /// Updates one line of text on either front or back side.
    /// Rejects mutations when the sign is waxed.
    pub fn set_line(
        &mut self,
        is_front: bool,
        line: usize,
        text: impl Into<String>,
    ) -> Result<(), SignError> {
        if self.is_waxed {
            return Err(SignError::Waxed);
        }
        self.side_mut(is_front).set_line(line, text)
    }

    /// Sets all 4 lines on either front or back side.
    /// Rejects mutations if the sign is waxed or any line exceeds 384 bytes.
    pub fn set_lines(&mut self, is_front: bool, lines: [String; 4]) -> Result<(), SignError> {
        if self.is_waxed {
            return Err(SignError::Waxed);
        }
        for (i, line) in lines.iter().enumerate() {
            if line.len() > MAX_LINE_BYTES {
                return Err(SignError::LineTooLong {
                    line: i,
                    bytes: line.len(),
                });
            }
        }
        self.side_mut(is_front).messages = lines;
        Ok(())
    }

    /// Sets text dye color on specified side.
    pub fn set_color(&mut self, is_front: bool, color: DyeColor) -> Result<(), SignError> {
        if self.is_waxed {
            return Err(SignError::Waxed);
        }
        self.side_mut(is_front).set_color(color);
        Ok(())
    }

    /// Toggles glowing text on specified side.
    pub fn set_glowing(&mut self, is_front: bool, glowing: bool) -> Result<(), SignError> {
        if self.is_waxed {
            return Err(SignError::Waxed);
        }
        self.side_mut(is_front).set_glowing(glowing);
        Ok(())
    }

    pub fn set_waxed(&mut self, waxed: bool) {
        self.is_waxed = waxed;
    }

    pub fn is_waxed(&self) -> bool {
        self.is_waxed
    }

    pub fn to_nbt(&self) -> NbtTag {
        NbtTag::Compound(vec![
            ("back_text".to_string(), self.back_text.to_nbt()),
            ("front_text".to_string(), self.front_text.to_nbt()),
            (
                "is_waxed".to_string(),
                NbtTag::Byte(if self.is_waxed { 1 } else { 0 }),
            ),
        ])
    }

    pub fn from_nbt(nbt: &NbtTag) -> Result<Self, BlockEntityError> {
        let front_tag = nbt
            .get_compound("front_text")
            .ok_or(BlockEntityError::MissingField("front_text"))?;
        let back_tag = nbt
            .get_compound("back_text")
            .ok_or(BlockEntityError::MissingField("back_text"))?;
        let is_waxed = nbt.get_bool("is_waxed").unwrap_or(false);

        let front_text = SignSide::from_compound(front_tag)?;
        let back_text = SignSide::from_compound(back_tag)?;

        Ok(Self {
            front_text,
            back_text,
            is_waxed,
        })
    }
}

/// Sign modification errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    Waxed,
    LineTooLong { line: usize, bytes: usize },
    InvalidLineIndex(usize),
}

impl fmt::Display for SignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Waxed => write!(f, "sign is waxed and cannot be modified"),
            Self::LineTooLong { line, bytes } => {
                write!(
                    f,
                    "line {line} exceeds maximum length {MAX_LINE_BYTES} bytes ({bytes} bytes)"
                )
            }
            Self::InvalidLineIndex(idx) => {
                write!(f, "invalid line index {idx}, expected 0..4")
            }
        }
    }
}

impl std::error::Error for SignError {}

/// Authoritative block entities attached to block positions.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockEntity {
    Sign(Box<SignBlockEntity>),
    Chest,
    Furnace,
}

impl BlockEntity {
    /// 26.3 registry protocol ID.
    pub fn type_id(&self) -> u32 {
        match self {
            Self::Sign(_) => TYPE_ID_SIGN,
            Self::Chest => TYPE_ID_CHEST,
            Self::Furnace => TYPE_ID_FURNACE,
        }
    }

    /// Namespaced identifier string.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Sign(_) => TYPE_NAME_SIGN,
            Self::Chest => TYPE_NAME_CHEST,
            Self::Furnace => TYPE_NAME_FURNACE,
        }
    }

    pub fn as_sign(&self) -> Option<&SignBlockEntity> {
        match self {
            Self::Sign(sign) => Some(sign),
            _ => None,
        }
    }

    pub fn as_sign_mut(&mut self) -> Option<&mut SignBlockEntity> {
        match self {
            Self::Sign(sign) => Some(sign),
            _ => None,
        }
    }
}

/// In-memory storage and fast lookup of block entities indexed by [`BlockPos`].
#[derive(Debug, Clone, Default)]
pub struct BlockEntityRegistry {
    entities: FxHashMap<BlockPos, BlockEntity>,
}

impl BlockEntityRegistry {
    pub fn new() -> Self {
        Self {
            entities: FxHashMap::default(),
        }
    }

    pub fn get(&self, pos: &BlockPos) -> Option<&BlockEntity> {
        self.entities.get(pos)
    }

    pub fn get_mut(&mut self, pos: &BlockPos) -> Option<&mut BlockEntity> {
        self.entities.get_mut(pos)
    }

    pub fn get_sign(&self, pos: &BlockPos) -> Option<&SignBlockEntity> {
        self.get(pos).and_then(BlockEntity::as_sign)
    }

    pub fn get_sign_mut(&mut self, pos: &BlockPos) -> Option<&mut SignBlockEntity> {
        self.get_mut(pos).and_then(BlockEntity::as_sign_mut)
    }

    pub fn insert(&mut self, pos: BlockPos, entity: BlockEntity) -> Option<BlockEntity> {
        self.entities.insert(pos, entity)
    }

    pub fn insert_sign(&mut self, pos: BlockPos, sign: SignBlockEntity) -> Option<BlockEntity> {
        self.insert(pos, BlockEntity::Sign(Box::new(sign)))
    }

    pub fn remove(&mut self, pos: &BlockPos) -> Option<BlockEntity> {
        self.entities.remove(pos)
    }

    /// Removes and returns the block entity attached to `pos` upon block destruction.
    pub fn on_block_destruction(&mut self, pos: &BlockPos) -> Option<BlockEntity> {
        self.remove(pos)
    }

    pub fn contains(&self, pos: &BlockPos) -> bool {
        self.entities.contains_key(pos)
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn clear(&mut self) {
        self.entities.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&BlockPos, &BlockEntity)> {
        self.entities.iter()
    }
}

mod nbt;
pub use nbt::*;
mod packets;
pub use packets::*;

#[cfg(test)]
mod tests;
