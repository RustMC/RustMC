//! Text components: colors, JSON and NBT encodings, and the chat
//! broadcast helper.

use super::*;

pub(super) const TAG_END: u8 = 0;
pub(super) const TAG_BYTE: u8 = 1;
pub(super) const TAG_STRING: u8 = NBT_TAG_STRING;
pub(super) const TAG_LIST: u8 = 9;
pub(super) const TAG_COMPOUND: u8 = 10;

/// Write a TAG_String payload: unsigned 16-bit byte length and modified UTF-8 via `java_wire`.
pub(super) fn put_nbt_string(value: &str, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
    let mut encoded = Vec::with_capacity(value.len());
    encode_modified_utf8(value, &mut encoded);
    let length = u16::try_from(encoded.len()).map_err(|_| {
        ProtocolError::Oversized(format!(
            "NBT string of {} bytes exceeds {MAX_NBT_STRING_BYTES}",
            encoded.len()
        ))
    })?;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend(encoded);
    Ok(())
}

pub(super) fn read_nbt_string(input: &[u8]) -> Result<(String, &[u8]), ProtocolError> {
    let (text, consumed) = java_wire::read_nbt_string(input).map_err(map_wire_error)?;
    Ok((text, &input[consumed..]))
}

pub(super) fn put_nbt_named(tag: u8, name: &str, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
    out.push(tag);
    put_nbt_string(name, out)
}

impl ChatComponent {
    /// Encode as a network Text Component: an anonymous NBT tag that is a
    /// TAG_String for plain text and a TAG_Compound for anything else.
    pub fn to_network_nbt(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut out = Vec::new();
        if self.is_plain() {
            out.push(TAG_STRING);
            put_nbt_string(&self.text, &mut out)?;
        } else {
            out.push(TAG_COMPOUND);
            self.write_compound(&mut out, 1)?;
        }
        if out.len() > MAX_TEXT_COMPONENT_BYTES {
            return Err(ProtocolError::Oversized(format!(
                "Text component of {} bytes exceeds {MAX_TEXT_COMPONENT_BYTES}",
                out.len()
            )));
        }
        Ok(out)
    }

    pub(super) fn is_plain(&self) -> bool {
        self.color.is_none()
            && self.bold.is_none()
            && self.italic.is_none()
            && self.underlined.is_none()
            && self.strikethrough.is_none()
            && self.obfuscated.is_none()
            && self.extra.is_empty()
    }

    pub(super) fn write_compound(
        &self,
        out: &mut Vec<u8>,
        depth: usize,
    ) -> Result<(), ProtocolError> {
        if depth > MAX_TEXT_COMPONENT_DEPTH {
            return Err(ProtocolError::Oversized(format!(
                "Text component nesting exceeds {MAX_TEXT_COMPONENT_DEPTH}"
            )));
        }
        put_nbt_named(TAG_STRING, "text", out)?;
        put_nbt_string(&self.text, out)?;
        if let Some(color) = &self.color {
            put_nbt_named(TAG_STRING, "color", out)?;
            put_nbt_string(color.as_str(), out)?;
        }
        for (name, value) in self.style_flags() {
            if let Some(value) = value {
                put_nbt_named(TAG_BYTE, name, out)?;
                out.push(u8::from(value));
            }
        }
        if !self.extra.is_empty() {
            put_nbt_named(TAG_LIST, "extra", out)?;
            out.push(TAG_COMPOUND);
            out.extend_from_slice(&(self.extra.len() as i32).to_be_bytes());
            for child in &self.extra {
                child.write_compound(out, depth + 1)?;
            }
        }
        out.push(TAG_END);
        Ok(())
    }

    pub(super) fn style_flags(&self) -> [(&'static str, Option<bool>); 5] {
        [
            ("bold", self.bold),
            ("italic", self.italic),
            ("underlined", self.underlined),
            ("strikethrough", self.strikethrough),
            ("obfuscated", self.obfuscated),
        ]
    }

    /// Read one anonymous network Text Component, returning the remaining
    /// bytes. A field this model cannot represent is rejected, never dropped.
    pub fn read_network_nbt(input: &[u8]) -> Result<(Self, &[u8]), ProtocolError> {
        let (tag, rest) = input.split_first().ok_or(ProtocolError::UnexpectedEof)?;
        let (component, rest) = match *tag {
            TAG_STRING => {
                let (text, rest) = read_nbt_string(rest)?;
                (Self::text(text), rest)
            }
            TAG_COMPOUND => Self::read_compound(rest, 1)?,
            other => {
                return Err(ProtocolError::Malformed(format!(
                    "Unsupported text component root tag {other}"
                )));
            }
        };
        if input.len() - rest.len() > MAX_TEXT_COMPONENT_BYTES {
            return Err(ProtocolError::Oversized(format!(
                "Text component exceeds {MAX_TEXT_COMPONENT_BYTES} bytes"
            )));
        }
        Ok((component, rest))
    }

    pub(super) fn read_compound(
        mut input: &[u8],
        depth: usize,
    ) -> Result<(Self, &[u8]), ProtocolError> {
        if depth > MAX_TEXT_COMPONENT_DEPTH {
            return Err(ProtocolError::Oversized(format!(
                "Text component nesting exceeds {MAX_TEXT_COMPONENT_DEPTH}"
            )));
        }
        let mut component = Self::default();
        loop {
            let (tag, rest) = input.split_first().ok_or(ProtocolError::UnexpectedEof)?;
            if *tag == TAG_END {
                return Ok((component, rest));
            }
            let (name, rest) = read_nbt_string(rest)?;
            input = match (*tag, name.as_str()) {
                (TAG_STRING, "text") => {
                    let (text, rest) = read_nbt_string(rest)?;
                    component.text = text;
                    rest
                }
                (TAG_STRING, "color") => {
                    let (color, rest) = read_nbt_string(rest)?;
                    component.color = Some(ChatColor::parse(&color).ok_or_else(|| {
                        ProtocolError::Malformed(format!("Unknown text color '{color}'"))
                    })?);
                    rest
                }
                (TAG_BYTE, "bold" | "italic" | "underlined" | "strikethrough" | "obfuscated") => {
                    let (value, rest) = rest.split_first().ok_or(ProtocolError::UnexpectedEof)?;
                    let value = Some(*value != 0);
                    match name.as_str() {
                        "bold" => component.bold = value,
                        "italic" => component.italic = value,
                        "underlined" => component.underlined = value,
                        "strikethrough" => component.strikethrough = value,
                        _ => component.obfuscated = value,
                    }
                    rest
                }
                (TAG_LIST, "extra") => {
                    let (children, rest) = Self::read_extra(rest, depth)?;
                    component.extra = children;
                    rest
                }
                _ => {
                    return Err(ProtocolError::Malformed(format!(
                        "Unsupported text component field '{name}' with tag {tag}"
                    )));
                }
            };
        }
    }

    pub(super) fn read_extra(
        input: &[u8],
        depth: usize,
    ) -> Result<(Vec<Self>, &[u8]), ProtocolError> {
        let (element, rest) = input.split_first().ok_or(ProtocolError::UnexpectedEof)?;
        let (count, mut rest) = rest
            .split_first_chunk::<4>()
            .ok_or(ProtocolError::UnexpectedEof)?;
        let count = i32::from_be_bytes(*count);
        if count < 0 {
            return Err(ProtocolError::Malformed("Negative NBT list length".into()));
        }
        // Every element takes at least one byte, so a false count fails early
        // without reserving memory for it.
        let mut children = Vec::new();
        for _ in 0..count {
            let (child, after) = match *element {
                TAG_STRING => {
                    let (text, after) = read_nbt_string(rest)?;
                    (Self::text(text), after)
                }
                TAG_COMPOUND => Self::read_compound(rest, depth + 1)?,
                other => {
                    return Err(ProtocolError::Malformed(format!(
                        "Unsupported extra element tag {other}"
                    )));
                }
            };
            children.push(child);
            rest = after;
        }
        Ok((children, rest))
    }
}

// ============================================================================
// Chat Colors & Chat Component Builder
// ============================================================================

/// Standard Minecraft text colors.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChatColor {
    Black,
    DarkBlue,
    DarkGreen,
    DarkAqua,
    DarkRed,
    DarkPurple,
    Gold,
    Gray,
    DarkGray,
    Blue,
    Green,
    Aqua,
    Red,
    LightPurple,
    Yellow,
    White,
    Custom(String),
}

impl ChatColor {
    /// Return the canonical Minecraft identifier for this color.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Black => "black",
            Self::DarkBlue => "dark_blue",
            Self::DarkGreen => "dark_green",
            Self::DarkAqua => "dark_aqua",
            Self::DarkRed => "dark_red",
            Self::DarkPurple => "dark_purple",
            Self::Gold => "gold",
            Self::Gray => "gray",
            Self::DarkGray => "dark_gray",
            Self::Blue => "blue",
            Self::Green => "green",
            Self::Aqua => "aqua",
            Self::Red => "red",
            Self::LightPurple => "light_purple",
            Self::Yellow => "yellow",
            Self::White => "white",
            Self::Custom(hex) => hex.as_str(),
        }
    }

    /// Parse a color name or `#RRGGBB` hex string into a ChatColor.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "black" => Some(Self::Black),
            "dark_blue" => Some(Self::DarkBlue),
            "dark_green" => Some(Self::DarkGreen),
            "dark_aqua" => Some(Self::DarkAqua),
            "dark_red" => Some(Self::DarkRed),
            "dark_purple" => Some(Self::DarkPurple),
            "gold" => Some(Self::Gold),
            "gray" => Some(Self::Gray),
            "dark_gray" => Some(Self::DarkGray),
            "blue" => Some(Self::Blue),
            "green" => Some(Self::Green),
            "aqua" => Some(Self::Aqua),
            "red" => Some(Self::Red),
            "light_purple" => Some(Self::LightPurple),
            "yellow" => Some(Self::Yellow),
            "white" => Some(Self::White),
            hex if hex.starts_with('#') && hex.len() == 7 => Some(Self::Custom(hex.to_string())),
            _ => None,
        }
    }
}

/// JSON Chat Component adhering to Minecraft Java Edition text component specification.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatComponent {
    /// The base text content.
    pub text: String,
    /// Color of this component.
    pub color: Option<ChatColor>,
    /// Bold styling flag.
    pub bold: Option<bool>,
    /// Italic styling flag.
    pub italic: Option<bool>,
    /// Underlined styling flag.
    pub underlined: Option<bool>,
    /// Strikethrough styling flag.
    pub strikethrough: Option<bool>,
    /// Obfuscated styling flag.
    pub obfuscated: Option<bool>,
    /// Extra sibling components.
    pub extra: Vec<ChatComponent>,
}

impl ChatComponent {
    /// Create a plain text chat component.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    /// Create a plain text chat component (alias).
    pub fn plain(text: impl Into<String>) -> Self {
        Self::text(text)
    }

    /// Create a colored text chat component.
    pub fn colored(text: impl Into<String>, color: ChatColor) -> Self {
        Self::text(text).with_color(color)
    }

    /// Format a sender prefix and message, e.g. `<Player> hello`.
    pub fn player_message(sender: &str, message: &str) -> Self {
        Self::text(format!("<{sender}> {message}"))
    }

    /// Format a sender prefix and message with custom sender and message colors.
    pub fn player_message_styled(
        sender: &str,
        sender_color: ChatColor,
        message: &str,
        message_color: Option<ChatColor>,
    ) -> Self {
        let mut msg_comp = Self::text(message);
        if let Some(col) = message_color {
            msg_comp = msg_comp.with_color(col);
        }
        Self::text(format!("<{sender}> "))
            .with_color(sender_color)
            .add_extra(msg_comp)
    }

    /// Set the color for this component.
    pub fn with_color(mut self, color: ChatColor) -> Self {
        self.color = Some(color);
        self
    }

    /// Set bold styling.
    pub fn with_bold(mut self, bold: bool) -> Self {
        self.bold = Some(bold);
        self
    }

    /// Set italic styling.
    pub fn with_italic(mut self, italic: bool) -> Self {
        self.italic = Some(italic);
        self
    }

    /// Set underlined styling.
    pub fn with_underlined(mut self, underlined: bool) -> Self {
        self.underlined = Some(underlined);
        self
    }

    /// Set strikethrough styling.
    pub fn with_strikethrough(mut self, strikethrough: bool) -> Self {
        self.strikethrough = Some(strikethrough);
        self
    }

    /// Set obfuscated styling.
    pub fn with_obfuscated(mut self, obfuscated: bool) -> Self {
        self.obfuscated = Some(obfuscated);
        self
    }

    /// Add an extra sibling component.
    pub fn add_extra(mut self, extra: ChatComponent) -> Self {
        self.extra.push(extra);
        self
    }

    /// Convert this component into a `serde_json::Value` adhering strictly to Minecraft text JSON schema.
    pub fn to_value(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert(
            "text".to_string(),
            serde_json::Value::String(self.text.clone()),
        );
        if let Some(ref color) = self.color {
            map.insert(
                "color".to_string(),
                serde_json::Value::String(color.as_str().to_string()),
            );
        }
        if let Some(bold) = self.bold {
            map.insert("bold".to_string(), serde_json::Value::Bool(bold));
        }
        if let Some(italic) = self.italic {
            map.insert("italic".to_string(), serde_json::Value::Bool(italic));
        }
        if let Some(underlined) = self.underlined {
            map.insert(
                "underlined".to_string(),
                serde_json::Value::Bool(underlined),
            );
        }
        if let Some(strikethrough) = self.strikethrough {
            map.insert(
                "strikethrough".to_string(),
                serde_json::Value::Bool(strikethrough),
            );
        }
        if let Some(obfuscated) = self.obfuscated {
            map.insert(
                "obfuscated".to_string(),
                serde_json::Value::Bool(obfuscated),
            );
        }
        if !self.extra.is_empty() {
            let extra_vals: Vec<serde_json::Value> =
                self.extra.iter().map(|e| e.to_value()).collect();
            map.insert("extra".to_string(), serde_json::Value::Array(extra_vals));
        }
        serde_json::Value::Object(map)
    }

    /// Serialize this component to a standard Minecraft JSON text string (`{"text":"..."}`).
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.to_value())
            .unwrap_or_else(|_| format!("{{\"text\":\"{}\"}}", self.text))
    }

    /// Parse a JSON string conforming to Minecraft text component specification into a `ChatComponent`.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| format!("Failed to parse chat component JSON: {e}"))?;
        Self::from_value(&value)
    }

    /// Parse a `serde_json::Value` into a `ChatComponent`.
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        match value {
            serde_json::Value::String(s) => Ok(Self::text(s.clone())),
            serde_json::Value::Object(map) => {
                let text = map
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let color = map
                    .get("color")
                    .and_then(|v| v.as_str())
                    .and_then(ChatColor::parse);
                let bold = map.get("bold").and_then(|v| v.as_bool());
                let italic = map.get("italic").and_then(|v| v.as_bool());
                let underlined = map.get("underlined").and_then(|v| v.as_bool());
                let strikethrough = map.get("strikethrough").and_then(|v| v.as_bool());
                let obfuscated = map.get("obfuscated").and_then(|v| v.as_bool());
                let mut extra = Vec::new();
                if let Some(serde_json::Value::Array(arr)) = map.get("extra") {
                    for item in arr {
                        extra.push(Self::from_value(item)?);
                    }
                }
                Ok(Self {
                    text,
                    color,
                    bold,
                    italic,
                    underlined,
                    strikethrough,
                    obfuscated,
                    extra,
                })
            }
            _ => Err("Invalid chat component JSON type, expected object or string".into()),
        }
    }

    /// Extract concatenated plain text stripped of JSON formatting.
    pub fn to_plain_text(&self) -> String {
        let mut out = self.text.clone();
        for child in &self.extra {
            out.push_str(&child.to_plain_text());
        }
        out
    }
}

impl fmt::Display for ChatComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_json())
    }
}

/// Create a system chat broadcast packet for a player message `<sender> message`.
pub fn broadcast_chat(
    sender: &str,
    message: &str,
) -> Result<SystemChatMessagePacket, ProtocolError> {
    SystemChatMessagePacket::chat(ChatComponent::player_message(sender, message))
}

// ============================================================================
// Unit Tests
// ============================================================================
