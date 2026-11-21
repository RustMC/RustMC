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

/// Encodes a UTF-8 string into Java Modified UTF-8 bytes for NBT storage.
///
/// Under Java Modified UTF-8:
/// - Code points 1..=127 are 1 byte.
/// - Code point 0 is encoded as `[0xC0, 0x80]`.
/// - Code points 128..=2047 are encoded in 2 bytes.
/// - Code points 2048..=65535 are encoded in 3 bytes.
/// - Supplementary characters (> 65535, e.g. emoji like 😀) are encoded as surrogate pairs,
///   each surrogate occupying 3 bytes, producing a total of 6 bytes.
pub fn encode_modified_utf8(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    java_wire::encode_modified_utf8(s, &mut out);
    out
}

/// Decodes Java Modified UTF-8 bytes into a standard Rust UTF-8 String.
pub fn decode_modified_utf8(bytes: &[u8]) -> Result<String, BlockEntityError> {
    java_wire::decode_modified_utf8(bytes).map_err(map_wire_error)
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
            let encoded = encode_modified_utf8(v);
            out.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
            out.extend_from_slice(&encoded);
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
                let enc_name = encode_modified_utf8(name);
                out.extend_from_slice(&(enc_name.len() as u16).to_be_bytes());
                out.extend_from_slice(&enc_name);
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

/// Encodes a named root compound tag into standard NBT bytes.
pub fn write_named_network_nbt(name: &str, tag: &NbtTag, out: &mut Vec<u8>) {
    out.push(10); // TAG_Compound
    let enc_name = encode_modified_utf8(name);
    out.extend_from_slice(&(enc_name.len() as u16).to_be_bytes());
    out.extend_from_slice(&enc_name);
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
        let rest = self
            .data
            .get(self.pos..)
            .ok_or(BlockEntityError::UnexpectedEof)?;
        let (text, consumed) = java_wire::read_nbt_string(rest).map_err(map_wire_error)?;
        self.pos += consumed;
        Ok(text)
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
                if elem_type == 0 && len > 0 {
                    return Err(BlockEntityError::Malformed("nonempty TAG_End list"));
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

/// Decodes one anonymous root network NBT tag (without a root name), returning the tag
/// and the number of bytes consumed from `data`.
pub fn read_anonymous_network_nbt(data: &[u8]) -> Result<(NbtTag, usize), BlockEntityError> {
    let mut reader = NbtReader::new(data);
    let root_type = reader.read_u8()?;
    if root_type == 0 {
        return Err(BlockEntityError::Malformed(
            "unexpected TAG_End as root network NBT tag",
        ));
    }
    let tag = reader.read_payload(root_type, 0)?;
    Ok((tag, reader.pos))
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

    // Check if this is a named root compound (has u16 name_len + name bytes).
    // In anonymous network NBT, the next byte is either TAG_End (0) or a child tag type (1..=12).
    // If the next byte is 0 and there are 2+ bytes available with data following,
    // or if the first 2 bytes declare a name length, consume the root name.
    if reader.remaining() >= 2 && reader.data[reader.pos] == 0 {
        let name_len = reader.read_u16()? as usize;
        let _root_name = reader.read_bytes(name_len)?;
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

/// Decodes a named root compound tag from standard NBT bytes, returning the root name and tag.
pub fn read_named_network_nbt(data: &[u8]) -> Result<(String, NbtTag), BlockEntityError> {
    let mut reader = NbtReader::new(data);
    let root_type = reader.read_u8()?;
    if root_type == 0 {
        return Ok((String::new(), NbtTag::Compound(Vec::new())));
    }
    if root_type != 10 {
        return Err(BlockEntityError::Malformed(
            "root NBT tag must be Compound (10)",
        ));
    }
    let root_name = reader.read_string()?;
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
    Ok((root_name, NbtTag::Compound(entries)))
}

fn map_wire_error(err: WireError) -> BlockEntityError {
    match err {
        WireError::Truncated => BlockEntityError::UnexpectedEof,
        WireError::VarIntTooLong => BlockEntityError::Malformed("VarInt overflow"),
        WireError::NonCanonicalVarInt => BlockEntityError::Malformed("non-canonical VarInt"),
        WireError::TooLong { length, .. } => BlockEntityError::LineTooLong(length),
        WireError::InvalidText => BlockEntityError::Malformed("invalid text encoding"),
        WireError::BelowCompressionThreshold { .. }
        | WireError::InvalidCompression
        | WireError::DecompressedSizeMismatch { .. } => {
            BlockEntityError::Malformed("invalid compressed framing")
        }
    }
}

fn read_varint(input: &[u8]) -> Result<(u32, usize), BlockEntityError> {
    java_wire::read_varint(input)
        .map_err(map_wire_error)?
        .ok_or(BlockEntityError::UnexpectedEof)
}

/// Java 26.3 Clientbound `BlockEntityDataPacket` (`0x06`).
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
    pub fn decode(input: &[u8]) -> Result<Self, BlockEntityError> {
        let (pos_raw, mut input) =
            java_wire::read_i64_be(input).map_err(|_| BlockEntityError::UnexpectedEof)?;
        let pos = BlockPos::unpack(pos_raw);

        let (type_id, varint_len) = read_varint(input)?;
        input = &input[varint_len..];

        let nbt = read_network_nbt(input)?;

        Ok(Self {
            pos,
            block_entity_type: type_id,
            nbt,
        })
    }

    /// Decodes from a framed packet (skipping frame length and packet ID `0x06`).
    pub fn decode_framed(input: &[u8]) -> Result<Self, BlockEntityError> {
        let (frame_len, len_bytes) = read_varint(input)?;
        let frame_payload = &input[len_bytes..];
        if frame_payload.len() != frame_len as usize {
            return Err(BlockEntityError::Malformed("frame length mismatch"));
        }
        let (packet_id, id_bytes) = read_varint(frame_payload)?;
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

/// Java 26.3 Serverbound `UpdateSignPacket` (`0x3E`).
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
    pub fn decode(input: &[u8]) -> Result<Self, BlockEntityError> {
        let (pos_raw, rest) =
            java_wire::read_i64_be(input).map_err(|_| BlockEntityError::UnexpectedEof)?;
        let (&front_byte, mut input) = rest.split_first().ok_or(BlockEntityError::UnexpectedEof)?;
        let pos = BlockPos::unpack(pos_raw);
        if !pos.is_valid() {
            return Err(BlockEntityError::InvalidPosition(pos));
        }

        if front_byte > 1 {
            return Err(BlockEntityError::Malformed("is_front_text must be 0 or 1"));
        }
        let is_front_text = front_byte == 1;

        let mut lines = [String::new(), String::new(), String::new(), String::new()];
        for line in &mut lines {
            let (text, rest) =
                java_wire::read_string(input, MAX_LINE_BYTES).map_err(map_wire_error)?;
            *line = text.to_string();
            input = rest;
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

    /// Decodes from a framed packet (skipping frame length and packet ID `0x3E`).
    pub fn decode_framed(input: &[u8]) -> Result<Self, BlockEntityError> {
        let (frame_len, len_bytes) = read_varint(input)?;
        let frame_payload = &input[len_bytes..];
        if frame_payload.len() != frame_len as usize {
            return Err(BlockEntityError::Malformed("frame length mismatch"));
        }
        let (packet_id, id_bytes) = read_varint(frame_payload)?;
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
mod tests;
