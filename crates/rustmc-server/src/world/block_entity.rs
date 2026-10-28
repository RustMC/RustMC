//! Block entity storage, sign text state, and Java 26.3 network synchronization.
//!
//! Provides authoritative block entity state attachments indexed by [`BlockPos`],
//! full mutable state for signs (front and back [`SignSide`] with lines, color,
//! and glowing flags), and wire codecs for:
//! - Java 26.3 `ClientboundBlockEntityDataPacket` (`0x07`): position, registry type ID 40,
//!   and network NBT payload.
//! - Java 26.3 `ServerboundUpdateSignPacket` (`0x33`): position, front/back selector,
//!   and 4 validated UTF-8 lines (capped at 384 bytes each).

use rustc_hash::FxHashMap;
use std::fmt;

use crate::discovery_java::{frame, put_string, put_varint};

/// Maximum byte length for a single sign text line (384 bytes UTF-8).
pub const MAX_LINE_BYTES: usize = 384;

/// Number of lines on one side of a sign.
pub const LINES_PER_SIDE: usize = 4;

/// Packet ID for clientbound Block Entity Data packet in Java 26.3.
pub const PACKET_ID_BLOCK_ENTITY_DATA: u32 = 0x07;

/// Packet ID for serverbound Update Sign packet in Java 26.3.
pub const PACKET_ID_UPDATE_SIGN: u32 = 0x33;

/// Java 26.3 block entity registry type IDs.
pub const TYPE_ID_CHEST: u32 = 10;
pub const TYPE_ID_FURNACE: u32 = 26;
pub const TYPE_ID_HANGING_SIGN: u32 = 27;
pub const TYPE_ID_SIGN: u32 = 40;

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
#[allow(clippy::large_enum_variant)]
pub enum BlockEntity {
    Sign(SignBlockEntity),
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
        self.insert(pos, BlockEntity::Sign(sign))
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

/// Lightweight, memory-safe NBT Tag representation for block entity payloads.
#[derive(Debug, Clone, PartialEq)]
pub enum NbtTag {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<NbtTag>),
    Compound(Vec<(String, NbtTag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl NbtTag {
    pub fn tag_type_id(&self) -> u8 {
        match self {
            Self::End => 0,
            Self::Byte(_) => 1,
            Self::Short(_) => 2,
            Self::Int(_) => 3,
            Self::Long(_) => 4,
            Self::Float(_) => 5,
            Self::Double(_) => 6,
            Self::ByteArray(_) => 7,
            Self::String(_) => 8,
            Self::List(_) => 9,
            Self::Compound(_) => 10,
            Self::IntArray(_) => 11,
            Self::LongArray(_) => 12,
        }
    }

    pub fn get<'a>(&'a self, key: &str) -> Option<&'a NbtTag> {
        match self {
            Self::Compound(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Byte(b) => Some(*b != 0),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[NbtTag]> {
        match self {
            Self::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_compound(&self) -> Option<&[(String, NbtTag)]> {
        match self {
            Self::Compound(c) => Some(c),
            _ => None,
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(NbtTag::as_str)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(NbtTag::as_bool)
    }

    pub fn get_compound(&self, key: &str) -> Option<&[(String, NbtTag)]> {
        self.get(key).and_then(NbtTag::as_compound)
    }
}

fn write_nbt_payload(tag: &NbtTag, out: &mut Vec<u8>) {
    match tag {
        NbtTag::End => {}
        NbtTag::Byte(v) => out.push(*v as u8),
        NbtTag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        NbtTag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        NbtTag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        NbtTag::Float(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        NbtTag::Double(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        NbtTag::ByteArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            out.extend(v.iter().map(|b| *b as u8));
        }
        NbtTag::String(v) => {
            out.extend_from_slice(&(v.len() as u16).to_be_bytes());
            out.extend_from_slice(v.as_bytes());
        }
        NbtTag::List(items) => {
            if items.is_empty() {
                out.push(0);
                out.extend_from_slice(&0i32.to_be_bytes());
            } else {
                let kind = items[0].tag_type_id();
                out.push(kind);
                out.extend_from_slice(&(items.len() as i32).to_be_bytes());
                for item in items {
                    write_nbt_payload(item, out);
                }
            }
        }
        NbtTag::Compound(entries) => {
            for (name, val) in entries {
                out.push(val.tag_type_id());
                out.extend_from_slice(&(name.len() as u16).to_be_bytes());
                out.extend_from_slice(name.as_bytes());
                write_nbt_payload(val, out);
            }
            out.push(0); // TAG_End
        }
        NbtTag::IntArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
        NbtTag::LongArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
    }
}

/// Encodes an anonymous root compound tag into network NBT bytes.
pub fn write_network_nbt(tag: &NbtTag, out: &mut Vec<u8>) {
    out.push(10); // TAG_Compound
    write_nbt_payload(tag, out);
}

struct NbtReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> NbtReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn read_u8(&mut self) -> Result<u8, BlockEntityError> {
        if self.pos >= self.data.len() {
            return Err(BlockEntityError::UnexpectedEof);
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], BlockEntityError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(BlockEntityError::UnexpectedEof)?;
        if end > self.data.len() {
            return Err(BlockEntityError::UnexpectedEof);
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_u16(&mut self) -> Result<u16, BlockEntityError> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn read_i32(&mut self) -> Result<i32, BlockEntityError> {
        let b = self.read_bytes(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i64(&mut self) -> Result<i64, BlockEntityError> {
        let b = self.read_bytes(8)?;
        Ok(i64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_string(&mut self) -> Result<String, BlockEntityError> {
        let len = self.read_u16()? as usize;
        let bytes = self.read_bytes(len)?;
        std::str::from_utf8(bytes)
            .map(|s| s.to_string())
            .map_err(|_| BlockEntityError::Malformed("invalid UTF-8 in NBT string"))
    }

    fn read_payload(&mut self, tag_type: u8, depth: usize) -> Result<NbtTag, BlockEntityError> {
        if depth > 64 {
            return Err(BlockEntityError::Malformed("NBT depth exceeds maximum 64"));
        }
        match tag_type {
            0 => Ok(NbtTag::End),
            1 => Ok(NbtTag::Byte(self.read_u8()? as i8)),
            2 => Ok(NbtTag::Short(self.read_u16()? as i16)),
            3 => Ok(NbtTag::Int(self.read_i32()?)),
            4 => Ok(NbtTag::Long(self.read_i64()?)),
            5 => {
                let bits = self.read_i32()? as u32;
                Ok(NbtTag::Float(f32::from_bits(bits)))
            }
            6 => {
                let bits = self.read_i64()? as u64;
                Ok(NbtTag::Double(f64::from_bits(bits)))
            }
            7 => {
                let len = self.read_i32()?;
                if len < 0 {
                    return Err(BlockEntityError::Malformed("negative ByteArray length"));
                }
                let bytes = self.read_bytes(len as usize)?;
                Ok(NbtTag::ByteArray(bytes.iter().map(|&b| b as i8).collect()))
            }
            8 => Ok(NbtTag::String(self.read_string()?)),
            9 => {
                let elem_type = self.read_u8()?;
                let len = self.read_i32()?;
                if len < 0 {
                    return Err(BlockEntityError::Malformed("negative List length"));
                }
                let count = len as usize;
                let mut items = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    items.push(self.read_payload(elem_type, depth + 1)?);
                }
                Ok(NbtTag::List(items))
            }
            10 => {
                let mut entries = Vec::new();
                loop {
                    let child_type = self.read_u8()?;
                    if child_type == 0 {
                        break; // TAG_End
                    }
                    let name = self.read_string()?;
                    let val = self.read_payload(child_type, depth + 1)?;
                    entries.push((name, val));
                }
                Ok(NbtTag::Compound(entries))
            }
            11 => {
                let len = self.read_i32()?;
                if len < 0 {
                    return Err(BlockEntityError::Malformed("negative IntArray length"));
                }
                let count = len as usize;
                let mut ints = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    ints.push(self.read_i32()?);
                }
                Ok(NbtTag::IntArray(ints))
            }
            12 => {
                let len = self.read_i32()?;
                if len < 0 {
                    return Err(BlockEntityError::Malformed("negative LongArray length"));
                }
                let count = len as usize;
                let mut longs = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    longs.push(self.read_i64()?);
                }
                Ok(NbtTag::LongArray(longs))
            }
            _ => Err(BlockEntityError::Malformed("unknown NBT tag type")),
        }
    }
}

/// Decodes network NBT from bytes, accepting both anonymous and named root compounds.
pub fn read_network_nbt(data: &[u8]) -> Result<NbtTag, BlockEntityError> {
    let mut reader = NbtReader::new(data);
    let root_type = reader.read_u8()?;
    if root_type == 0 {
        return Ok(NbtTag::Compound(Vec::new()));
    }
    if root_type != 10 {
        return Err(BlockEntityError::Malformed(
            "root NBT tag must be Compound (10)",
        ));
    }

    // Check if this is a named root compound (has u16 name_len + name bytes)
    // In anonymous network NBT, the next byte is either TAG_End (0) or a child tag type (1..=12).
    if reader.remaining() >= 2 {
        let b0 = reader.data[reader.pos];
        let b1 = reader.data[reader.pos + 1];
        if b0 == 0 && b1 == 0 && reader.remaining() > 2 {
            // Named compound with empty name
            reader.pos += 2;
        }
    }

    let mut entries = Vec::new();
    loop {
        let child_type = reader.read_u8()?;
        if child_type == 0 {
            break;
        }
        let name = reader.read_string()?;
        let val = reader.read_payload(child_type, 1)?;
        entries.push((name, val));
    }
    Ok(NbtTag::Compound(entries))
}

fn read_varint_slice(input: &[u8]) -> Result<(u32, usize), BlockEntityError> {
    let mut value = 0u32;
    for (i, &byte) in input.iter().take(5).enumerate() {
        if i == 4 && byte & 0xf0 != 0 {
            return Err(BlockEntityError::Malformed("VarInt overflow"));
        }
        value |= u32::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            if i > 0 && byte == 0 {
                return Err(BlockEntityError::Malformed("non-canonical VarInt"));
            }
            return Ok((value, i + 1));
        }
    }
    Err(BlockEntityError::UnexpectedEof)
}

/// Java 26.3 Clientbound `BlockEntityDataPacket` (`0x07`).
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntityDataPacket {
    pub pos: BlockPos,
    pub block_entity_type: u32,
    pub nbt: NbtTag,
}

impl BlockEntityDataPacket {
    pub fn new(pos: BlockPos, block_entity_type: u32, nbt: NbtTag) -> Self {
        Self {
            pos,
            block_entity_type,
            nbt,
        }
    }

    pub fn from_sign(pos: BlockPos, sign: &SignBlockEntity) -> Self {
        Self {
            pos,
            block_entity_type: TYPE_ID_SIGN,
            nbt: sign.to_nbt(),
        }
    }

    /// Encodes the packet payload (without frame length/id prefix).
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&self.pos.pack().to_be_bytes());
        put_varint(self.block_entity_type, &mut body);
        write_network_nbt(&self.nbt, &mut body);
        body
    }

    /// Encodes a framed packet ready for wire transmission.
    pub fn frame(&self) -> Vec<u8> {
        frame(PACKET_ID_BLOCK_ENTITY_DATA, &self.encode())
    }

    /// Decodes from the packet payload (unframed).
    pub fn decode(mut input: &[u8]) -> Result<Self, BlockEntityError> {
        if input.len() < 8 {
            return Err(BlockEntityError::UnexpectedEof);
        }
        let pos_raw = i64::from_be_bytes(input[..8].try_into().unwrap());
        input = &input[8..];
        let pos = BlockPos::unpack(pos_raw);

        let (type_id, varint_len) = read_varint_slice(input)?;
        input = &input[varint_len..];

        let nbt = read_network_nbt(input)?;

        Ok(Self {
            pos,
            block_entity_type: type_id,
            nbt,
        })
    }

    /// Decodes from a framed packet (skipping frame length and packet ID `0x07`).
    pub fn decode_framed(input: &[u8]) -> Result<Self, BlockEntityError> {
        let (frame_len, len_bytes) = read_varint_slice(input)?;
        let frame_payload = &input[len_bytes..];
        if frame_payload.len() != frame_len as usize {
            return Err(BlockEntityError::Malformed("frame length mismatch"));
        }
        let (packet_id, id_bytes) = read_varint_slice(frame_payload)?;
        if packet_id != PACKET_ID_BLOCK_ENTITY_DATA {
            return Err(BlockEntityError::MismatchedType {
                expected: PACKET_ID_BLOCK_ENTITY_DATA,
                got: packet_id,
            });
        }
        Self::decode(&frame_payload[id_bytes..])
    }

    /// Converts the NBT payload into a `SignBlockEntity` if this is a sign packet.
    pub fn to_sign(&self) -> Result<SignBlockEntity, BlockEntityError> {
        if self.block_entity_type != TYPE_ID_SIGN {
            return Err(BlockEntityError::MismatchedType {
                expected: TYPE_ID_SIGN,
                got: self.block_entity_type,
            });
        }
        SignBlockEntity::from_nbt(&self.nbt)
    }
}

/// Java 26.3 Serverbound `UpdateSignPacket` (`0x33`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateSignPacket {
    pub pos: BlockPos,
    pub is_front_text: bool,
    pub lines: [String; LINES_PER_SIDE],
}

impl UpdateSignPacket {
    pub fn new(
        pos: BlockPos,
        is_front_text: bool,
        lines: [String; LINES_PER_SIDE],
    ) -> Result<Self, BlockEntityError> {
        if !pos.is_valid() {
            return Err(BlockEntityError::InvalidPosition(pos));
        }
        for line in &lines {
            if line.len() > MAX_LINE_BYTES {
                return Err(BlockEntityError::LineTooLong(line.len()));
            }
        }
        Ok(Self {
            pos,
            is_front_text,
            lines,
        })
    }

    /// Encodes the packet payload (unframed).
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&self.pos.pack().to_be_bytes());
        body.push(if self.is_front_text { 1 } else { 0 });
        for line in &self.lines {
            put_string(line, &mut body);
        }
        body
    }

    /// Encodes a framed packet ready for wire transmission.
    pub fn frame(&self) -> Vec<u8> {
        frame(PACKET_ID_UPDATE_SIGN, &self.encode())
    }

    /// Decodes and validates a serverbound `UpdateSignPacket` payload.
    pub fn decode(mut input: &[u8]) -> Result<Self, BlockEntityError> {
        if input.len() < 9 {
            return Err(BlockEntityError::UnexpectedEof);
        }
        let pos_raw = i64::from_be_bytes(input[..8].try_into().unwrap());
        let pos = BlockPos::unpack(pos_raw);
        if !pos.is_valid() {
            return Err(BlockEntityError::InvalidPosition(pos));
        }
        input = &input[8..];

        let front_byte = input[0];
        if front_byte > 1 {
            return Err(BlockEntityError::Malformed("is_front_text must be 0 or 1"));
        }
        let is_front_text = front_byte == 1;
        input = &input[1..];

        let mut lines = [String::new(), String::new(), String::new(), String::new()];
        for line in &mut lines {
            let (str_len, prefix) = read_varint_slice(input)?;
            input = &input[prefix..];
            let len = str_len as usize;
            if len > MAX_LINE_BYTES {
                return Err(BlockEntityError::LineTooLong(len));
            }
            if input.len() < len {
                return Err(BlockEntityError::UnexpectedEof);
            }
            let text = std::str::from_utf8(&input[..len])
                .map_err(|_| BlockEntityError::Malformed("invalid UTF-8 in sign line"))?;
            *line = text.to_string();
            input = &input[len..];
        }

        if !input.is_empty() {
            return Err(BlockEntityError::Malformed(
                "trailing bytes in UpdateSignPacket",
            ));
        }

        Ok(Self {
            pos,
            is_front_text,
            lines,
        })
    }

    /// Decodes from a framed packet (skipping frame length and packet ID `0x33`).
    pub fn decode_framed(input: &[u8]) -> Result<Self, BlockEntityError> {
        let (frame_len, len_bytes) = read_varint_slice(input)?;
        let frame_payload = &input[len_bytes..];
        if frame_payload.len() != frame_len as usize {
            return Err(BlockEntityError::Malformed("frame length mismatch"));
        }
        let (packet_id, id_bytes) = read_varint_slice(frame_payload)?;
        if packet_id != PACKET_ID_UPDATE_SIGN {
            return Err(BlockEntityError::MismatchedType {
                expected: PACKET_ID_UPDATE_SIGN,
                got: packet_id,
            });
        }
        Self::decode(&frame_payload[id_bytes..])
    }

    /// Applies this update packet to a target `SignBlockEntity`.
    /// Rejects if the sign is waxed.
    pub fn apply_to(&self, sign: &mut SignBlockEntity) -> Result<(), SignError> {
        sign.set_lines(self.is_front_text, self.lines.clone())
    }
}

/// Block entity and packet serialization errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockEntityError {
    UnexpectedEof,
    Malformed(&'static str),
    LineTooLong(usize),
    InvalidPosition(BlockPos),
    MissingField(&'static str),
    MismatchedType { expected: u32, got: u32 },
    Sign(SignError),
}

impl fmt::Display for BlockEntityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected end of packet data"),
            Self::Malformed(msg) => write!(f, "malformed packet data: {msg}"),
            Self::LineTooLong(len) => {
                write!(
                    f,
                    "sign line length {len} exceeds maximum {MAX_LINE_BYTES} bytes"
                )
            }
            Self::InvalidPosition(pos) => {
                write!(
                    f,
                    "invalid block position ({}, {}, {}) out of world bounds",
                    pos.x, pos.y, pos.z
                )
            }
            Self::MissingField(field) => write!(f, "missing required field: {field}"),
            Self::MismatchedType { expected, got } => {
                write!(f, "mismatched type ID: expected {expected}, got {got}")
            }
            Self::Sign(err) => write!(f, "sign error: {err}"),
        }
    }
}

impl std::error::Error for BlockEntityError {}

impl From<SignError> for BlockEntityError {
    fn from(err: SignError) -> Self {
        Self::Sign(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_pos_packing_roundtrip() {
        let test_cases = [
            BlockPos::new(0, 0, 0),
            BlockPos::new(100, 64, -200),
            BlockPos::new(-1, -64, -1),
            BlockPos::new(BlockPos::MIN_XZ, BlockPos::MIN_Y, BlockPos::MIN_XZ),
            BlockPos::new(BlockPos::MAX_XZ, BlockPos::MAX_Y, BlockPos::MAX_XZ),
            BlockPos::new(-30_000_000, 319, 30_000_000),
            BlockPos::new(1234567, -15, -7654321),
        ];

        for pos in test_cases {
            assert!(pos.is_valid(), "coordinate {pos:?} should be valid");
            let packed = pos.pack();
            let unpacked = BlockPos::unpack(packed);
            assert_eq!(pos, unpacked, "failed roundtrip for {pos:?}");
        }

        let invalid = BlockPos::new(0, 320, 0);
        assert!(!invalid.is_valid());
        let invalid_xz = BlockPos::new(30_000_001, 0, 0);
        assert!(!invalid_xz.is_valid());
    }

    #[test]
    fn test_sign_initialization_defaults() {
        let sign = SignBlockEntity::new();
        assert!(!sign.is_waxed());

        // Front text defaults
        assert_eq!(
            sign.front_text.messages,
            [String::new(), String::new(), String::new(), String::new()]
        );
        assert_eq!(sign.front_text.color, DyeColor::Black);
        assert!(!sign.front_text.has_glowing_text);

        // Back text defaults
        assert_eq!(
            sign.back_text.messages,
            [String::new(), String::new(), String::new(), String::new()]
        );
        assert_eq!(sign.back_text.color, DyeColor::Black);
        assert!(!sign.back_text.has_glowing_text);
    }

    #[test]
    fn test_sign_text_modification_front_and_back() {
        let mut sign = SignBlockEntity::new();

        // Modify front lines
        sign.set_line(true, 0, "Welcome").unwrap();
        sign.set_line(true, 1, "to").unwrap();
        sign.set_line(true, 2, "RustMC").unwrap();
        sign.set_line(true, 3, "!").unwrap();

        assert_eq!(sign.front_text.line(0), Some("Welcome"));
        assert_eq!(sign.front_text.line(1), Some("to"));
        assert_eq!(sign.front_text.line(2), Some("RustMC"));
        assert_eq!(sign.front_text.line(3), Some("!"));

        // Back lines remain unaffected
        assert_eq!(sign.back_text.line(0), Some(""));

        // Modify back lines
        sign.set_line(false, 0, "Rear Line 0").unwrap();
        sign.set_line(false, 3, "Rear Line 3").unwrap();

        assert_eq!(sign.back_text.line(0), Some("Rear Line 0"));
        assert_eq!(sign.back_text.line(1), Some(""));
        assert_eq!(sign.back_text.line(3), Some("Rear Line 3"));

        // Index out of bounds
        assert_eq!(
            sign.set_line(true, 4, "Invalid"),
            Err(SignError::InvalidLineIndex(4))
        );
    }

    #[test]
    fn test_sign_color_and_glowing_updates() {
        let mut sign = SignBlockEntity::new();

        // Front: red dye, glowing
        sign.set_color(true, DyeColor::Red).unwrap();
        sign.set_glowing(true, true).unwrap();

        // Back: blue dye, not glowing
        sign.set_color(false, DyeColor::Blue).unwrap();
        sign.set_glowing(false, false).unwrap();

        assert_eq!(sign.front_text.color, DyeColor::Red);
        assert!(sign.front_text.has_glowing_text);

        assert_eq!(sign.back_text.color, DyeColor::Blue);
        assert!(!sign.back_text.has_glowing_text);

        // Verify all 16 dye colors
        for color in DyeColor::ALL {
            assert_eq!(DyeColor::from_str_name(color.as_str()), Some(color));
        }
    }

    #[test]
    fn test_line_length_validation_rejects_oversized_lines() {
        let mut sign = SignBlockEntity::new();

        // Exact bound 384 bytes should succeed
        let max_text = "a".repeat(MAX_LINE_BYTES);
        assert_eq!(max_text.len(), 384);
        assert!(sign.set_line(true, 0, max_text).is_ok());

        // 385 bytes should be rejected
        let too_long = "a".repeat(MAX_LINE_BYTES + 1);
        assert_eq!(too_long.len(), 385);
        assert_eq!(
            sign.set_line(true, 1, too_long.clone()),
            Err(SignError::LineTooLong {
                line: 1,
                bytes: 385
            })
        );

        // Batch lines rejection
        let batch = [
            "line 0".to_string(),
            "line 1".to_string(),
            too_long,
            "line 3".to_string(),
        ];
        assert_eq!(
            sign.set_lines(true, batch),
            Err(SignError::LineTooLong {
                line: 2,
                bytes: 385
            })
        );
    }

    #[test]
    fn test_waxed_sign_rejects_further_edits() {
        let mut sign = SignBlockEntity::new();
        sign.set_line(true, 0, "Initial Text").unwrap();
        sign.set_waxed(true);
        assert!(sign.is_waxed());

        // Text mutation rejected
        assert_eq!(sign.set_line(true, 0, "Modified"), Err(SignError::Waxed));
        assert_eq!(
            sign.set_lines(false, ["a".into(), "b".into(), "c".into(), "d".into()]),
            Err(SignError::Waxed)
        );

        // Color & glowing updates rejected
        assert_eq!(sign.set_color(true, DyeColor::Green), Err(SignError::Waxed));
        assert_eq!(sign.set_glowing(true, true), Err(SignError::Waxed));

        // Original line unchanged
        assert_eq!(sign.front_text.line(0), Some("Initial Text"));
    }

    #[test]
    fn test_block_entity_registry_operations() {
        let mut registry = BlockEntityRegistry::new();
        let pos1 = BlockPos::new(10, 64, 20);
        let pos2 = BlockPos::new(30, 70, 40);

        let mut sign1 = SignBlockEntity::new();
        sign1.set_line(true, 0, "Sign 1").unwrap();

        let mut sign2 = SignBlockEntity::new();
        sign2.set_line(true, 0, "Sign 2").unwrap();

        assert!(registry.insert_sign(pos1, sign1).is_none());
        assert!(registry.insert_sign(pos2, sign2).is_none());
        assert_eq!(registry.len(), 2);

        // Lookup
        assert_eq!(
            registry.get_sign(&pos1).unwrap().front_text.line(0),
            Some("Sign 1")
        );
        assert_eq!(
            registry.get_sign(&pos2).unwrap().front_text.line(0),
            Some("Sign 2")
        );

        // Mutable lookup
        let sign1_mut = registry.get_sign_mut(&pos1).unwrap();
        sign1_mut.set_line(true, 1, "Updated Line").unwrap();
        assert_eq!(
            registry.get_sign(&pos1).unwrap().front_text.line(1),
            Some("Updated Line")
        );

        // Removal on destruction
        let removed = registry.on_block_destruction(&pos1).unwrap();
        assert_eq!(removed.type_id(), TYPE_ID_SIGN);
        assert!(!registry.contains(&pos1));
        assert_eq!(registry.len(), 1);
        assert!(registry.contains(&pos2));
    }

    #[test]
    fn test_block_entity_data_packet_encoding_and_decoding() {
        let pos = BlockPos::new(-5, 65, 42);
        let mut sign = SignBlockEntity::new();
        sign.set_line(true, 0, "RustMC Front").unwrap();
        sign.set_line(true, 3, "Front Line 3").unwrap();
        sign.set_color(true, DyeColor::Cyan).unwrap();
        sign.set_glowing(true, true).unwrap();

        sign.set_line(false, 1, "RustMC Back").unwrap();
        sign.set_color(false, DyeColor::Yellow).unwrap();
        sign.set_waxed(true);

        // Create packet from sign
        let packet = BlockEntityDataPacket::from_sign(pos, &sign);
        assert_eq!(packet.block_entity_type, TYPE_ID_SIGN);

        // Unframed payload round-trip
        let encoded = packet.encode();
        let decoded = BlockEntityDataPacket::decode(&encoded).expect("decode packet payload");
        assert_eq!(decoded.pos, pos);
        assert_eq!(decoded.block_entity_type, TYPE_ID_SIGN);

        let decoded_sign = decoded.to_sign().expect("convert decoded to sign");
        assert_eq!(decoded_sign, sign);

        // Framed round-trip
        let framed = packet.frame();
        let decoded_framed =
            BlockEntityDataPacket::decode_framed(&framed).expect("decode framed packet");
        assert_eq!(decoded_framed.pos, pos);
        assert_eq!(decoded_framed.to_sign().unwrap(), sign);
    }

    #[test]
    fn test_update_sign_packet_roundtrip_and_validation() {
        let pos = BlockPos::new(100, 64, -200);
        let lines = [
            "First Line".to_string(),
            "Second Line".to_string(),
            "Third Line".to_string(),
            "Fourth Line".to_string(),
        ];

        let packet = UpdateSignPacket::new(pos, true, lines.clone()).unwrap();

        // Unframed round-trip
        let encoded = packet.encode();
        let decoded = UpdateSignPacket::decode(&encoded).expect("decode UpdateSignPacket");
        assert_eq!(decoded, packet);

        // Framed round-trip
        let framed = packet.frame();
        let decoded_framed =
            UpdateSignPacket::decode_framed(&framed).expect("decode framed UpdateSignPacket");
        assert_eq!(decoded_framed, packet);

        // Apply packet to sign
        let mut sign = SignBlockEntity::new();
        decoded.apply_to(&mut sign).expect("apply packet to sign");
        assert_eq!(sign.front_text.messages, lines);

        // Apply to waxed sign fails
        sign.set_waxed(true);
        assert_eq!(decoded.apply_to(&mut sign), Err(SignError::Waxed));

        // Malformed: invalid position
        let bad_pos = BlockPos::new(0, 400, 0); // Out of world bounds
        assert!(UpdateSignPacket::new(bad_pos, true, lines.clone()).is_err());

        // Malformed: oversized line
        let bad_lines = ["a".repeat(385), String::new(), String::new(), String::new()];
        assert!(UpdateSignPacket::new(pos, true, bad_lines).is_err());

        // Decode rejects trailing garbage
        let mut bad_encoded = encoded.clone();
        bad_encoded.push(0xff);
        assert!(matches!(
            UpdateSignPacket::decode(&bad_encoded),
            Err(BlockEntityError::Malformed(_))
        ));

        // Decode rejects invalid is_front_text byte
        let mut bad_bool = encoded.clone();
        bad_bool[8] = 2; // Not 0 or 1
        assert!(matches!(
            UpdateSignPacket::decode(&bad_bool),
            Err(BlockEntityError::Malformed(_))
        ));
    }

    #[test]
    fn test_sign_synchronization_roundtrip_flow() {
        let mut registry = BlockEntityRegistry::new();
        let pos = BlockPos::new(12, 65, -88);

        // 1. Initial sign placement in registry
        let sign = SignBlockEntity::new();
        registry.insert_sign(pos, sign);

        // 2. Client submits UpdateSignPacket for front text
        let front_lines = [
            "Town Square".to_string(),
            "Market Ahead".to_string(),
            "Beware of Phantoms".to_string(),
            "[Shop Open]".to_string(),
        ];
        let client_packet = UpdateSignPacket::new(pos, true, front_lines.clone()).unwrap();
        let wire_update_bytes = client_packet.frame();

        // 3. Server decodes client packet
        let server_received = UpdateSignPacket::decode_framed(&wire_update_bytes).unwrap();
        assert_eq!(server_received.pos, pos);
        assert!(server_received.is_front_text);

        // 4. Server applies update to registered sign entity
        let sign_entry = registry.get_sign_mut(&pos).expect("sign in registry");
        server_received
            .apply_to(sign_entry)
            .expect("apply text update");
        sign_entry.set_color(true, DyeColor::Yellow).unwrap();
        sign_entry.set_glowing(true, true).unwrap();

        // 5. Server encodes BlockEntityDataPacket (0x07) for observers
        let sync_packet = BlockEntityDataPacket::from_sign(pos, sign_entry);
        let wire_sync_bytes = sync_packet.frame();

        // 6. Observing client decodes BlockEntityDataPacket
        let client_received = BlockEntityDataPacket::decode_framed(&wire_sync_bytes).unwrap();
        assert_eq!(client_received.pos, pos);
        assert_eq!(client_received.block_entity_type, TYPE_ID_SIGN);

        let synchronized_sign = client_received.to_sign().unwrap();
        assert_eq!(synchronized_sign.front_text.messages, front_lines);
        assert_eq!(synchronized_sign.front_text.color, DyeColor::Yellow);
        assert!(synchronized_sign.front_text.has_glowing_text);
        assert!(!synchronized_sign.is_waxed());

        // 7. Waxing the sign prevents further text edits
        let sign_entry = registry.get_sign_mut(&pos).unwrap();
        sign_entry.set_waxed(true);

        let malicious_edit = UpdateSignPacket::new(
            pos,
            true,
            ["Hacked".into(), "Text".into(), String::new(), String::new()],
        )
        .unwrap();
        assert_eq!(malicious_edit.apply_to(sign_entry), Err(SignError::Waxed));
        assert_eq!(sign_entry.front_text.messages, front_lines);
    }
}
