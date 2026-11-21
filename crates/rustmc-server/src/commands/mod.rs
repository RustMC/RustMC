//! Chat command parser, target selectors, coordinate resolvers, and execution dispatcher.

use crate::vanilla::random::RandomSource;
use std::{collections::HashMap, fmt, str::FromStr};

mod dispatcher;
pub use dispatcher::*;
mod handlers;
pub use handlers::*;

#[cfg(test)]
mod tests;

/// Maximum absolute coordinate value accepted by the coordinate parser/validator.
pub const MAX_COORDINATE: f64 = 30_000_000.0;

/// Ticks in one day-night cycle.
///
/// Source for this and the `/time set` presets below:
/// <https://minecraft.wiki/w/Commands/time>.
pub const TICKS_PER_DAY: u64 = 24_000;
/// `/time set day`.
pub const TIME_DAY: u64 = 1_000;
/// `/time set noon`.
pub const TIME_NOON: u64 = 6_000;
/// `/time set night`.
pub const TIME_NIGHT: u64 = 13_000;
/// `/time set midnight`.
pub const TIME_MIDNIGHT: u64 = 18_000;

/// Largest `/give` count this dispatcher accepts: 100 stacks of 64.
///
/// Source: <https://minecraft.wiki/w/Commands/give>.
pub const MAX_GIVE_COUNT: u32 = 6_400;

/// A 1D coordinate component that can be absolute, relative (`~`), or local (`^`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Coordinate {
    /// Absolute world coordinate, e.g. `10.5` or `-100.0`.
    Absolute(f64),
    /// Coordinate relative to execution origin, e.g. `~` (0.0) or `~1.5`.
    Relative(f64),
    /// Local coordinate relative to entity look direction, e.g. `^` (0.0) or `^1.0`.
    Local(f64),
}

impl Coordinate {
    /// Parse a single coordinate string component.
    pub fn parse(s: &str) -> Result<Self, CommandError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(CommandError::InvalidCoordinate(
                "Coordinate cannot be empty".to_string(),
            ));
        }

        if let Some(rest) = s.strip_prefix('~') {
            if rest.is_empty() {
                Ok(Self::Relative(0.0))
            } else {
                let offset = rest.parse::<f64>().map_err(|_| {
                    CommandError::InvalidCoordinate(format!("Invalid relative coordinate '{s}'"))
                })?;
                if !offset.is_finite() {
                    return Err(CommandError::InvalidCoordinate(format!(
                        "Coordinate '{s}' is not finite"
                    )));
                }
                Ok(Self::Relative(offset))
            }
        } else if let Some(rest) = s.strip_prefix('^') {
            if rest.is_empty() {
                Ok(Self::Local(0.0))
            } else {
                let offset = rest.parse::<f64>().map_err(|_| {
                    CommandError::InvalidCoordinate(format!("Invalid local coordinate '{s}'"))
                })?;
                if !offset.is_finite() {
                    return Err(CommandError::InvalidCoordinate(format!(
                        "Coordinate '{s}' is not finite"
                    )));
                }
                Ok(Self::Local(offset))
            }
        } else {
            let val = s.parse::<f64>().map_err(|_| {
                CommandError::InvalidCoordinate(format!("Invalid coordinate '{s}'"))
            })?;
            if !val.is_finite() {
                return Err(CommandError::InvalidCoordinate(format!(
                    "Coordinate '{s}' is not finite"
                )));
            }
            Ok(Self::Absolute(val))
        }
    }

    /// Resolve this coordinate against an origin value.
    ///
    /// A single axis cannot apply the rotated basis that `^` needs, so a local component
    /// is only an axis offset here. Destinations must go through
    /// [`Coordinates3::resolve_with_rotation`].
    pub fn resolve(&self, origin: f64) -> f64 {
        match *self {
            Self::Absolute(val) => val,
            Self::Relative(offset) => origin + offset,
            Self::Local(offset) => origin + offset,
        }
    }

    /// Whether this coordinate is a local coordinate (`^`).
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    /// Whether this coordinate is a relative coordinate (`~`).
    pub fn is_relative(&self) -> bool {
        matches!(self, Self::Relative(_))
    }

    /// Whether this coordinate is an absolute coordinate.
    pub fn is_absolute(&self) -> bool {
        matches!(self, Self::Absolute(_))
    }
}

impl FromStr for Coordinate {
    type Err = CommandError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for Coordinate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absolute(val) => write!(f, "{val}"),
            Self::Relative(offset) => {
                if *offset == 0.0 {
                    write!(f, "~")
                } else {
                    write!(f, "~{offset}")
                }
            }
            Self::Local(offset) => {
                if *offset == 0.0 {
                    write!(f, "^")
                } else {
                    write!(f, "^{offset}")
                }
            }
        }
    }
}

/// A 3D coordinate triple containing X, Y, and Z components.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coordinates3 {
    pub x: Coordinate,
    pub y: Coordinate,
    pub z: Coordinate,
}

impl Coordinates3 {
    /// Construct a new coordinate triple and validate that local coordinates
    /// are not mixed with world coordinates.
    pub fn new(x: Coordinate, y: Coordinate, z: Coordinate) -> Result<Self, CommandError> {
        let x_loc = x.is_local();
        let y_loc = y.is_local();
        let z_loc = z.is_local();
        let any_local = x_loc || y_loc || z_loc;
        let all_local = x_loc && y_loc && z_loc;
        if any_local && !all_local {
            return Err(CommandError::InvalidSyntax(
                "Cannot mix world and local coordinates (everything must either use ^ or not)"
                    .to_string(),
            ));
        }
        Ok(Self { x, y, z })
    }

    /// Parse 3 coordinate component strings into a `Coordinates3`.
    pub fn parse(x_str: &str, y_str: &str, z_str: &str) -> Result<Self, CommandError> {
        let x = Coordinate::parse(x_str)?;
        let y = Coordinate::parse(y_str)?;
        let z = Coordinate::parse(z_str)?;
        Self::new(x, y, z)
    }

    /// Resolve all 3 coordinate components against an origin point `(x, y, z)`
    /// and optional player rotation `(yaw, pitch)` in degrees.
    ///
    /// For local coordinates (`^left ^up ^forward`), computes the local basis
    /// vectors from rotation using standard vanilla trigonometry:
    /// - Forward: (-sin(yaw) * cos(pitch), -sin(pitch), cos(yaw) * cos(pitch))
    /// - Left: (cos(yaw), 0, sin(yaw))
    /// - Up: (-sin(pitch) * sin(yaw), cos(pitch), sin(pitch) * cos(yaw))
    ///
    /// If local coordinates are resolved without rotation (e.g. console sender),
    /// returns an error.
    pub fn resolve_with_rotation(
        &self,
        origin: (f64, f64, f64),
        rotation: Option<(f32, f32)>,
    ) -> Result<(f64, f64, f64), CommandError> {
        if self.is_local() {
            let (yaw, pitch) = rotation.ok_or_else(|| {
                CommandError::ConsoleNotAllowed(
                    "Local coordinates (^) require a sender or target with rotation".to_string(),
                )
            })?;

            let left = match self.x {
                Coordinate::Local(v) => v,
                _ => 0.0,
            };
            let up = match self.y {
                Coordinate::Local(v) => v,
                _ => 0.0,
            };
            let forward = match self.z {
                Coordinate::Local(v) => v,
                _ => 0.0,
            };

            let rad_yaw = (yaw as f64).to_radians();
            let rad_pitch = (pitch as f64).to_radians();

            let cos_yaw = rad_yaw.cos();
            let sin_yaw = rad_yaw.sin();
            let cos_pitch = rad_pitch.cos();
            let sin_pitch = rad_pitch.sin();

            // Forward unit vector
            let fx = -sin_yaw * cos_pitch;
            let fy = -sin_pitch;
            let fz = cos_yaw * cos_pitch;

            // Left unit vector (horizontal perpendicular to forward)
            let lx = cos_yaw;
            let ly = 0.0;
            let lz = sin_yaw;

            // Up unit vector (F x L)
            let ux = -sin_pitch * sin_yaw;
            let uy = cos_pitch;
            let uz = sin_pitch * cos_yaw;

            let dest_x = origin.0 + left * lx + up * ux + forward * fx;
            let dest_y = origin.1 + left * ly + up * uy + forward * fy;
            let dest_z = origin.2 + left * lz + up * uz + forward * fz;

            Ok((dest_x, dest_y, dest_z))
        } else {
            Ok((
                self.x.resolve(origin.0),
                self.y.resolve(origin.1),
                self.z.resolve(origin.2),
            ))
        }
    }

    /// Resolve all 3 coordinate components against an origin point `(x, y, z)`,
    /// assuming zero rotation. Use [`Self::resolve_with_rotation`] for a real executor.
    pub fn resolve(&self, origin: (f64, f64, f64)) -> (f64, f64, f64) {
        self.resolve_with_rotation(origin, Some((0.0, 0.0)))
            .unwrap_or(origin)
    }

    /// Whether this coordinate triple uses local coordinates.
    pub fn is_local(&self) -> bool {
        self.x.is_local()
    }

    /// Whether every component is absolute, so no execution origin is needed.
    pub fn is_absolute(&self) -> bool {
        self.x.is_absolute() && self.y.is_absolute() && self.z.is_absolute()
    }
}

impl FromStr for Coordinates3 {
    type Err = CommandError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(CommandError::InvalidSyntax(format!(
                "Expected 3 coordinate components, found {}",
                parts.len()
            )));
        }
        Self::parse(parts[0], parts[1], parts[2])
    }
}

impl fmt::Display for Coordinates3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.x, self.y, self.z)
    }
}

/// Entity target selectors used in Minecraft commands.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TargetSelector {
    /// Target all players (`@a`).
    AllPlayers,
    /// Target the nearest player (`@p`).
    NearestPlayer,
    /// Target executing entity / sender (`@s`).
    Self_,
    /// Target a random player (`@r`).
    RandomPlayer,
    /// Target a player by literal username.
    Named(String),
}

impl TargetSelector {
    /// Parse a target selector string.
    pub fn parse(s: &str) -> Result<Self, CommandError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(CommandError::InvalidSelector(
                "Target selector cannot be empty".to_string(),
            ));
        }

        match s {
            "@a" => Ok(Self::AllPlayers),
            "@p" => Ok(Self::NearestPlayer),
            "@s" => Ok(Self::Self_),
            "@r" => Ok(Self::RandomPlayer),
            _ if s.starts_with('@') => Err(CommandError::InvalidSelector(format!(
                "Unknown target selector '{s}'"
            ))),
            _ => {
                if s.len() > 16 {
                    return Err(CommandError::InvalidSelector(format!(
                        "Player name '{s}' exceeds maximum length of 16 characters"
                    )));
                }
                if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(CommandError::InvalidSelector(format!(
                        "Player name '{s}' contains invalid characters"
                    )));
                }
                Ok(Self::Named(s.to_string()))
            }
        }
    }
}

impl FromStr for TargetSelector {
    type Err = CommandError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for TargetSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AllPlayers => write!(f, "@a"),
            Self::NearestPlayer => write!(f, "@p"),
            Self::Self_ => write!(f, "@s"),
            Self::RandomPlayer => write!(f, "@r"),
            Self::Named(name) => write!(f, "{name}"),
        }
    }
}

/// Standard player game modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    /// Parse a game mode from name, abbreviation, or numeric id.
    pub fn parse(s: &str) -> Result<Self, CommandError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "survival" | "s" | "0" => Ok(Self::Survival),
            "creative" | "c" | "1" => Ok(Self::Creative),
            "adventure" | "a" | "2" => Ok(Self::Adventure),
            "spectator" | "sp" | "3" => Ok(Self::Spectator),
            _ => Err(CommandError::InvalidGameMode(format!(
                "Unknown game mode '{s}'"
            ))),
        }
    }

    /// Lowercase canonical identifier.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Survival => "survival",
            Self::Creative => "creative",
            Self::Adventure => "adventure",
            Self::Spectator => "spectator",
        }
    }

    /// Human-readable display label.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Survival => "Survival Mode",
            Self::Creative => "Creative Mode",
            Self::Adventure => "Adventure Mode",
            Self::Spectator => "Spectator Mode",
        }
    }
}

impl FromStr for GameMode {
    type Err = CommandError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for GameMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Context of the command executor.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandSender {
    Console,
    Player {
        name: String,
        pos: (f64, f64, f64),
        permission_level: u8,
        yaw: f32,
        pitch: f32,
    },
}

impl CommandSender {
    /// Create a console command sender.
    pub fn console() -> Self {
        Self::Console
    }

    /// Create a player command sender.
    pub fn player(name: impl Into<String>, pos: (f64, f64, f64), permission_level: u8) -> Self {
        Self::Player {
            name: name.into(),
            pos,
            permission_level,
            yaw: 0.0,
            pitch: 0.0,
        }
    }

    /// Create a player command sender with explicit yaw and pitch rotation in degrees.
    pub fn player_with_rotation(
        name: impl Into<String>,
        pos: (f64, f64, f64),
        permission_level: u8,
        yaw: f32,
        pitch: f32,
    ) -> Self {
        Self::Player {
            name: name.into(),
            pos,
            permission_level,
            yaw,
            pitch,
        }
    }

    /// Name of the sender.
    pub fn name(&self) -> &str {
        match self {
            Self::Console => "Server",
            Self::Player { name, .. } => name.as_str(),
        }
    }

    /// Position of the sender, if an in-world entity.
    pub fn position(&self) -> Option<(f64, f64, f64)> {
        match self {
            Self::Console => None,
            Self::Player { pos, .. } => Some(*pos),
        }
    }

    /// Rotation `(yaw, pitch)` of the sender in degrees, if available.
    pub fn rotation(&self) -> Option<(f32, f32)> {
        match self {
            Self::Console => None,
            Self::Player { yaw, pitch, .. } => Some((*yaw, *pitch)),
        }
    }

    /// Numerical permission level (0 = regular player, 2 = cheat commands, 4 = console/owner).
    pub fn permission_level(&self) -> u8 {
        match self {
            Self::Console => 4,
            Self::Player {
                permission_level, ..
            } => *permission_level,
        }
    }
}

/// Outcome of executing a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandResult {
    Success(String),
    Error(String),
}

impl CommandResult {
    pub fn success(msg: impl Into<String>) -> Self {
        Self::Success(msg.into())
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self::Error(msg.into())
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success(_))
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Success(msg) | Self::Error(msg) => msg.as_str(),
        }
    }
}

/// Command parsing or execution failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    EmptyCommand,
    UnknownCommand(String),
    MissingArgument(String),
    InvalidArgument(String),
    InvalidCoordinate(String),
    InvalidSelector(String),
    InvalidGameMode(String),
    InvalidSyntax(String),
    PlayerNotFound(String),
    PermissionDenied(String),
    ConsoleNotAllowed(String),
    ExecutionError(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCommand => write!(f, "Empty command"),
            Self::UnknownCommand(cmd) => write!(f, "Unknown command '{cmd}'"),
            Self::MissingArgument(arg) => write!(f, "Missing argument: {arg}"),
            Self::InvalidArgument(msg) => write!(f, "Invalid argument: {msg}"),
            Self::InvalidCoordinate(msg) => write!(f, "Invalid coordinate: {msg}"),
            Self::InvalidSelector(msg) => write!(f, "Invalid target selector: {msg}"),
            Self::InvalidGameMode(msg) => write!(f, "Invalid game mode: {msg}"),
            Self::InvalidSyntax(msg) => write!(f, "Invalid syntax: {msg}"),
            Self::PlayerNotFound(name) => write!(f, "Player not found: {name}"),
            Self::PermissionDenied(msg) => write!(f, "Permission denied: {msg}"),
            Self::ConsoleNotAllowed(msg) => write!(f, "Console cannot execute this: {msg}"),
            Self::ExecutionError(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Tracked player target state inside the dispatcher simulation.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerTarget {
    pub name: String,
    pub pos: (f64, f64, f64),
    pub yaw: f32,
    pub pitch: f32,
    pub gamemode: GameMode,
    pub is_alive: bool,
    pub inventory: HashMap<String, u32>,
}

impl PlayerTarget {
    pub fn new(name: impl Into<String>, pos: (f64, f64, f64)) -> Self {
        Self {
            name: name.into(),
            pos,
            yaw: 0.0,
            pitch: 0.0,
            gamemode: GameMode::Survival,
            is_alive: true,
            inventory: HashMap::new(),
        }
    }

    pub fn with_rotation(mut self, yaw: f32, pitch: f32) -> Self {
        self.yaw = yaw;
        self.pitch = pitch;
        self
    }

    pub fn with_gamemode(mut self, gamemode: GameMode) -> Self {
        self.gamemode = gamemode;
        self
    }

    /// Retrieve count of an item from inventory.
    pub fn item_count(&self, item: &str) -> u32 {
        let key = if item.contains(':') {
            item.to_string()
        } else {
            format!("minecraft:{item}")
        };
        self.inventory.get(&key).copied().unwrap_or(0)
    }

    /// Add an item count to inventory.
    pub fn give_item(&mut self, item: &str, count: u32) {
        let key = if item.contains(':') {
            item.to_string()
        } else {
            format!("minecraft:{item}")
        };
        // Repeated gives must not overflow the tracked count.
        let held = self.inventory.entry(key).or_insert(0);
        *held = held.saturating_add(count);
    }
}
