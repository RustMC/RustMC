//! Chat command parser, target selectors, coordinate resolvers, and execution dispatcher.

use crate::vanilla::random::RandomSource;
use std::{collections::HashMap, fmt, str::FromStr};

#[cfg(test)]
mod tests;

/// Maximum absolute coordinate value accepted by the coordinate parser/validator.
pub const MAX_COORDINATE: f64 = 30_000_000.0;

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

    /// Resolve all 3 coordinate components against an origin point `(x, y, z)`.
    pub fn resolve(&self, origin: (f64, f64, f64)) -> (f64, f64, f64) {
        self.resolve_with_rotation(origin, Some((0.0, 0.0)))
            .unwrap_or(origin)
    }

    /// Whether this coordinate triple uses local coordinates.
    pub fn is_local(&self) -> bool {
        self.x.is_local()
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
        *self.inventory.entry(key).or_insert(0) += count;
    }
}

/// Function signature for command execution handlers.
pub type CommandHandler =
    fn(&mut CommandDispatcher, &CommandSender, &[&str]) -> Result<String, CommandError>;

/// Metadata and callback for a registered command.
#[derive(Clone)]
pub struct CommandRegistration {
    pub name: String,
    pub permission_level: u8,
    pub handler: CommandHandler,
}

/// Dispatcher responsible for parsing and executing commands.
pub struct CommandDispatcher {
    pub seed: i64,
    pub time: u64,
    pub players: HashMap<String, PlayerTarget>,
    rng: RandomSource,
    commands: HashMap<String, CommandRegistration>,
}

impl Default for CommandDispatcher {
    fn default() -> Self {
        Self::with_seed(0)
    }
}

impl CommandDispatcher {
    /// Create a new command dispatcher with default seed 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new command dispatcher with an explicit world seed.
    pub fn with_seed(seed: i64) -> Self {
        let mut dispatcher = Self {
            seed,
            time: 0,
            players: HashMap::new(),
            rng: RandomSource::from_world_seed(seed as u64),
            commands: HashMap::new(),
        };
        dispatcher.register_standard_commands();
        dispatcher
    }

    /// Current world seed.
    pub fn seed(&self) -> i64 {
        self.seed
    }

    /// Update world seed and reseed PRNG.
    pub fn set_seed(&mut self, seed: i64) {
        self.seed = seed;
        self.rng = RandomSource::from_world_seed(seed as u64);
    }

    /// Current world time in ticks.
    pub fn time(&self) -> u64 {
        self.time
    }

    /// Set world time in ticks.
    pub fn set_time(&mut self, time: u64) {
        self.time = time;
    }

    /// Register a player in the simulation.
    pub fn add_player(&mut self, player: PlayerTarget) {
        self.players.insert(player.name.clone(), player);
    }

    /// Lookup a player reference by name.
    pub fn get_player(&self, name: &str) -> Option<&PlayerTarget> {
        self.players.get(name).or_else(|| {
            self.players
                .values()
                .find(|p| p.name.eq_ignore_ascii_case(name))
        })
    }

    /// Lookup a mutable player reference by name.
    pub fn get_player_mut(&mut self, name: &str) -> Option<&mut PlayerTarget> {
        if self.players.contains_key(name) {
            return self.players.get_mut(name);
        }
        self.players
            .values_mut()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// Remove a player by name.
    pub fn remove_player(&mut self, name: &str) -> Option<PlayerTarget> {
        self.players.remove(name)
    }

    /// Register a custom command.
    pub fn register(&mut self, name: &str, permission_level: u8, handler: CommandHandler) {
        let key = name.to_ascii_lowercase();
        self.commands.insert(
            key,
            CommandRegistration {
                name: name.to_string(),
                permission_level,
                handler,
            },
        );
    }

    /// Whether a command name is registered.
    pub fn is_command_registered(&self, name: &str) -> bool {
        self.commands.contains_key(&name.to_ascii_lowercase())
    }

    /// Register all standard vanilla commands required by RustMC:
    /// `/tp`, `/gamemode`, `/seed`, `/time`, `/kill`, `/give`.
    pub fn register_standard_commands(&mut self) {
        self.register("tp", 2, handle_tp);
        self.register("teleport", 2, handle_tp);
        self.register("gamemode", 2, handle_gamemode);
        self.register("seed", 2, handle_seed);
        self.register("time", 2, handle_time);
        self.register("kill", 2, handle_kill);
        self.register("give", 2, handle_give);
    }

    /// Resolve a `TargetSelector` against the currently tracked players.
    pub fn resolve_targets(
        &mut self,
        selector: &TargetSelector,
        origin: Option<(f64, f64, f64)>,
        sender_name: Option<&str>,
    ) -> Vec<String> {
        match selector {
            TargetSelector::AllPlayers => {
                let mut names: Vec<String> = self.players.keys().cloned().collect();
                names.sort();
                names
            }
            TargetSelector::NearestPlayer => {
                if self.players.is_empty() {
                    return Vec::new();
                }
                let org = origin.unwrap_or((0.0, 0.0, 0.0));
                let mut best_name = None;
                let mut best_dist_sq = f64::MAX;

                let mut sorted_players: Vec<&PlayerTarget> = self.players.values().collect();
                sorted_players.sort_by(|a, b| a.name.cmp(&b.name));

                for p in sorted_players {
                    let dx = p.pos.0 - org.0;
                    let dy = p.pos.1 - org.1;
                    let dz = p.pos.2 - org.2;
                    let dist_sq = dx * dx + dy * dy + dz * dz;
                    if dist_sq < best_dist_sq {
                        best_dist_sq = dist_sq;
                        best_name = Some(p.name.clone());
                    }
                }
                best_name.into_iter().collect()
            }
            TargetSelector::Self_ => {
                if let Some(name) = sender_name
                    && self.players.contains_key(name)
                {
                    return vec![name.to_string()];
                }
                Vec::new()
            }
            TargetSelector::RandomPlayer => {
                if self.players.is_empty() {
                    return Vec::new();
                }
                let mut names: Vec<String> = self.players.keys().cloned().collect();
                names.sort();
                let idx = self.rng.next_int_bounded(names.len() as u32) as usize;
                vec![names[idx].clone()]
            }
            TargetSelector::Named(name) => {
                if let Some(p) = self.get_player(name) {
                    vec![p.name.clone()]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Parse and execute a command string from a sender.
    pub fn execute(&mut self, sender: &CommandSender, input: &str) -> CommandResult {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return CommandResult::error("Empty command");
        }

        let input_without_slash = trimmed.strip_prefix('/').unwrap_or(trimmed).trim();
        if input_without_slash.is_empty() {
            return CommandResult::error("Empty command");
        }

        let tokens = match tokenize_command(input_without_slash) {
            Ok(tokens) => tokens,
            Err(err) => return CommandResult::Error(err.to_string()),
        };

        if tokens.is_empty() {
            return CommandResult::error("Empty command");
        }

        let cmd_name = tokens[0].to_ascii_lowercase();
        let registration = match self.commands.get(&cmd_name).cloned() {
            Some(reg) => reg,
            None => return CommandResult::error(format!("Unknown command '{cmd_name}'")),
        };

        // Permission check
        if sender.permission_level() < registration.permission_level {
            return CommandResult::error(format!(
                "Permission denied: you do not have permission to use /{} (requires level {})",
                registration.name, registration.permission_level
            ));
        }

        // Auto-register or sync sender if it is an in-world player
        if let CommandSender::Player { name, pos, .. } = sender {
            if let Some(player) = self.players.get_mut(name) {
                player.pos = *pos;
            } else {
                self.players
                    .insert(name.clone(), PlayerTarget::new(name.clone(), *pos));
            }
        }

        let arg_strs: Vec<&str> = tokens[1..].iter().map(String::as_str).collect();
        match (registration.handler)(self, sender, &arg_strs) {
            Ok(msg) => CommandResult::Success(msg),
            Err(err) => CommandResult::Error(err.to_string()),
        }
    }
}

/// Tokenize command input string by whitespace, respecting quotes.
pub fn tokenize_command(input: &str) -> Result<Vec<String>, CommandError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = None;

    for ch in input.chars() {
        match ch {
            '"' | '\'' => {
                if let Some(q) = in_quote {
                    if q == ch {
                        in_quote = None;
                    } else {
                        current.push(ch);
                    }
                } else {
                    in_quote = Some(ch);
                }
            }
            ch if ch.is_whitespace() => {
                if in_quote.is_some() {
                    current.push(ch);
                } else if !current.is_empty() {
                    tokens.push(current);
                    current = String::new();
                }
            }
            ch => {
                current.push(ch);
            }
        }
    }

    if in_quote.is_some() {
        return Err(CommandError::InvalidSyntax(
            "Unclosed quote in command".to_string(),
        ));
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

/// Validate that coordinates are finite and bounded within world border limits.
fn validate_coordinate_bounds(pos: (f64, f64, f64)) -> Result<(), CommandError> {
    if !pos.0.is_finite() || !pos.1.is_finite() || !pos.2.is_finite() {
        return Err(CommandError::InvalidArgument(
            "Coordinates must be finite".to_string(),
        ));
    }
    if pos.0 < -MAX_COORDINATE
        || pos.0 > MAX_COORDINATE
        || pos.1 < -MAX_COORDINATE
        || pos.1 > MAX_COORDINATE
        || pos.2 < -MAX_COORDINATE
        || pos.2 > MAX_COORDINATE
    {
        return Err(CommandError::InvalidArgument(format!(
            "Coordinate ({}, {}, {}) is out of bounds [-{MAX_COORDINATE}, {MAX_COORDINATE}]",
            pos.0, pos.1, pos.2
        )));
    }
    Ok(())
}

/// Validate and normalize a namespaced Minecraft item identifier.
fn validate_item_id(item: &str) -> Result<String, CommandError> {
    if item.is_empty() {
        return Err(CommandError::InvalidArgument(
            "Item identifier cannot be empty".to_string(),
        ));
    }
    let (namespace, path) = if let Some((ns, p)) = item.split_once(':') {
        (ns, p)
    } else {
        ("minecraft", item)
    };
    let is_valid_char =
        |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.' || c == '-';
    if namespace.is_empty() || !namespace.chars().all(is_valid_char) {
        return Err(CommandError::InvalidArgument(format!(
            "Invalid item namespace in '{item}'"
        )));
    }
    if path.is_empty() || !path.chars().all(is_valid_char) {
        return Err(CommandError::InvalidArgument(format!(
            "Invalid item path in '{item}'"
        )));
    }
    Ok(format!("{namespace}:{path}"))
}

// ---------------------------------------------------------------------------
// Standard Command Handlers
// ---------------------------------------------------------------------------

fn handle_tp(
    dispatcher: &mut CommandDispatcher,
    sender: &CommandSender,
    args: &[&str],
) -> Result<String, CommandError> {
    match args.len() {
        0 => Err(CommandError::MissingArgument(
            "Expected destination or target".to_string(),
        )),
        1 => {
            // /tp <destination_player> (teleports self to destination player)
            let (sender_name, _) = match sender {
                CommandSender::Player { name, pos, .. } => (name.clone(), *pos),
                CommandSender::Console => {
                    return Err(CommandError::ConsoleNotAllowed(
                        "Console must specify both a target and a destination".to_string(),
                    ));
                }
            };
            let dest_selector = TargetSelector::parse(args[0])?;
            let dest_targets =
                dispatcher.resolve_targets(&dest_selector, sender.position(), Some(&sender_name));
            if dest_targets.is_empty() {
                return Err(CommandError::PlayerNotFound(format!(
                    "Destination player not found: {}",
                    args[0]
                )));
            }
            if dest_targets.len() > 1 {
                return Err(CommandError::InvalidArgument(
                    "Destination selector matched more than one player".to_string(),
                ));
            }
            let dest_name = &dest_targets[0];
            let dest_pos = dispatcher
                .players
                .get(dest_name)
                .ok_or_else(|| {
                    CommandError::PlayerNotFound(format!(
                        "Destination player not found: {dest_name}"
                    ))
                })?
                .pos;

            if let Some(player) = dispatcher.players.get_mut(&sender_name) {
                player.pos = dest_pos;
            }
            Ok(format!(
                "Teleported {sender_name} to {dest_name} ({}, {}, {})",
                dest_pos.0, dest_pos.1, dest_pos.2
            ))
        }
        2 => {
            // /tp <target> <destination_player>
            let sender_name = match sender {
                CommandSender::Player { name, .. } => Some(name.as_str()),
                CommandSender::Console => None,
            };
            let dest_selector = TargetSelector::parse(args[1])?;
            let dest_targets =
                dispatcher.resolve_targets(&dest_selector, sender.position(), sender_name);
            if dest_targets.is_empty() {
                return Err(CommandError::PlayerNotFound(format!(
                    "Destination player not found: {}",
                    args[1]
                )));
            }
            if dest_targets.len() > 1 {
                return Err(CommandError::InvalidArgument(
                    "Destination selector matched more than one player".to_string(),
                ));
            }
            let dest_name = &dest_targets[0];
            let dest_pos = dispatcher
                .players
                .get(dest_name)
                .ok_or_else(|| {
                    CommandError::PlayerNotFound(format!(
                        "Destination player not found: {dest_name}"
                    ))
                })?
                .pos;

            let target_selector = TargetSelector::parse(args[0])?;
            let targets =
                dispatcher.resolve_targets(&target_selector, sender.position(), sender_name);
            if targets.is_empty() {
                return Err(CommandError::PlayerNotFound(format!(
                    "No player found matching selector '{}'",
                    args[0]
                )));
            }
            for name in &targets {
                if let Some(player) = dispatcher.players.get_mut(name) {
                    player.pos = dest_pos;
                }
            }
            if targets.len() == 1 {
                Ok(format!(
                    "Teleported {} to {dest_name} ({}, {}, {})",
                    targets[0], dest_pos.0, dest_pos.1, dest_pos.2
                ))
            } else {
                Ok(format!(
                    "Teleported {} players to {dest_name} ({}, {}, {})",
                    targets.len(),
                    dest_pos.0,
                    dest_pos.1,
                    dest_pos.2
                ))
            }
        }
        3 => {
            // /tp <x> <y> <z> (teleports self to coordinates)
            let (sender_name, sender_pos) = match sender {
                CommandSender::Player { name, pos, .. } => (name.clone(), *pos),
                CommandSender::Console => {
                    return Err(CommandError::ConsoleNotAllowed(
                        "Console cannot be teleported; specify a target player".to_string(),
                    ));
                }
            };
            let coords = Coordinates3::parse(args[0], args[1], args[2])?;
            let dest_pos = coords.resolve_with_rotation(sender_pos, sender.rotation())?;
            validate_coordinate_bounds(dest_pos)?;

            if let Some(player) = dispatcher.players.get_mut(&sender_name) {
                player.pos = dest_pos;
            }
            Ok(format!(
                "Teleported {sender_name} to {}, {}, {}",
                dest_pos.0, dest_pos.1, dest_pos.2
            ))
        }
        4 => {
            // /tp <target> <x> <y> <z>
            let sender_name = match sender {
                CommandSender::Player { name, .. } => Some(name.as_str()),
                CommandSender::Console => None,
            };
            let target_selector = TargetSelector::parse(args[0])?;
            let targets =
                dispatcher.resolve_targets(&target_selector, sender.position(), sender_name);
            if targets.is_empty() {
                return Err(CommandError::PlayerNotFound(format!(
                    "No player found matching selector '{}'",
                    args[0]
                )));
            }
            let coords = Coordinates3::parse(args[1], args[2], args[3])?;

            // Transactional update: compute and validate destinations for ALL targets before applying mutations.
            // If any destination is invalid, abort immediately without mutating ANY player.
            let mut pending_updates: Vec<(String, (f64, f64, f64))> =
                Vec::with_capacity(targets.len());
            for name in &targets {
                let (current_pos, rot) = match dispatcher.players.get(name) {
                    Some(p) => (p.pos, Some((p.yaw, p.pitch))),
                    None => ((0.0, 0.0, 0.0), sender.rotation()),
                };
                let dest_pos =
                    coords.resolve_with_rotation(current_pos, rot.or_else(|| sender.rotation()))?;
                validate_coordinate_bounds(dest_pos)?;
                pending_updates.push((name.clone(), dest_pos));
            }

            // Commit all pre-validated mutations
            let mut last_pos = (0.0, 0.0, 0.0);
            for (name, dest_pos) in pending_updates {
                if let Some(player) = dispatcher.players.get_mut(&name) {
                    player.pos = dest_pos;
                }
                last_pos = dest_pos;
            }
            if targets.len() == 1 {
                Ok(format!(
                    "Teleported {} to {}, {}, {}",
                    targets[0], last_pos.0, last_pos.1, last_pos.2
                ))
            } else {
                Ok(format!(
                    "Teleported {} players to {}, {}, {}",
                    targets.len(),
                    last_pos.0,
                    last_pos.1,
                    last_pos.2
                ))
            }
        }
        _ => Err(CommandError::InvalidArgument(
            "Too many arguments for /tp".to_string(),
        )),
    }
}

fn handle_gamemode(
    dispatcher: &mut CommandDispatcher,
    sender: &CommandSender,
    args: &[&str],
) -> Result<String, CommandError> {
    if args.is_empty() {
        return Err(CommandError::MissingArgument(
            "Expected game mode".to_string(),
        ));
    }
    if args.len() > 2 {
        return Err(CommandError::InvalidArgument(
            "Too many arguments for /gamemode".to_string(),
        ));
    }
    let mode = GameMode::parse(args[0])?;
    let sender_name = match sender {
        CommandSender::Player { name, .. } => Some(name.as_str()),
        CommandSender::Console => None,
    };

    let targets = if args.len() == 1 {
        let name = sender_name.ok_or_else(|| {
            CommandError::ConsoleNotAllowed(
                "Console must specify a target player for /gamemode".to_string(),
            )
        })?;
        vec![name.to_string()]
    } else {
        let selector = TargetSelector::parse(args[1])?;
        dispatcher.resolve_targets(&selector, sender.position(), sender_name)
    };

    if targets.is_empty() {
        return Err(CommandError::PlayerNotFound(format!(
            "No player found matching selector '{}'",
            args.get(1).copied().unwrap_or("@s")
        )));
    }

    for name in &targets {
        if let Some(player) = dispatcher.players.get_mut(name) {
            player.gamemode = mode;
        }
    }

    if targets.len() == 1 {
        if sender_name == Some(&targets[0]) {
            Ok(format!("Set own game mode to {}", mode.display_name()))
        } else {
            Ok(format!(
                "Set {}'s game mode to {}",
                targets[0],
                mode.display_name()
            ))
        }
    } else {
        Ok(format!(
            "Set {} players' game mode to {}",
            targets.len(),
            mode.display_name()
        ))
    }
}

fn handle_seed(
    dispatcher: &mut CommandDispatcher,
    _sender: &CommandSender,
    args: &[&str],
) -> Result<String, CommandError> {
    if !args.is_empty() {
        return Err(CommandError::InvalidArgument(
            "Too many arguments for /seed".to_string(),
        ));
    }
    Ok(format!("Seed: [{}]", dispatcher.seed))
}

fn handle_time(
    dispatcher: &mut CommandDispatcher,
    _sender: &CommandSender,
    args: &[&str],
) -> Result<String, CommandError> {
    if args.is_empty() {
        return Err(CommandError::MissingArgument(
            "Expected 'set', 'add', or 'query'".to_string(),
        ));
    }
    match args[0].to_ascii_lowercase().as_str() {
        "set" => {
            if args.len() < 2 {
                return Err(CommandError::MissingArgument(
                    "Expected time value for /time set".to_string(),
                ));
            }
            if args.len() > 2 {
                return Err(CommandError::InvalidArgument(
                    "Too many arguments for /time set".to_string(),
                ));
            }
            let new_time = match args[1].to_ascii_lowercase().as_str() {
                "day" => 1000,
                "noon" => 6000,
                "night" => 13000,
                "midnight" => 18000,
                other => {
                    let val: i64 = other.parse().map_err(|_| {
                        CommandError::InvalidArgument(format!("Invalid time value '{other}'"))
                    })?;
                    if val < 0 {
                        return Err(CommandError::InvalidArgument(
                            "Time value cannot be negative".to_string(),
                        ));
                    }
                    val as u64
                }
            };
            dispatcher.time = new_time;
            Ok(format!("Set the time to {}", dispatcher.time))
        }
        "add" => {
            if args.len() < 2 {
                return Err(CommandError::MissingArgument(
                    "Expected time value for /time add".to_string(),
                ));
            }
            if args.len() > 2 {
                return Err(CommandError::InvalidArgument(
                    "Too many arguments for /time add".to_string(),
                ));
            }
            let val: i64 = args[1].parse().map_err(|_| {
                CommandError::InvalidArgument(format!("Invalid time value '{}'", args[1]))
            })?;
            if val < 0 {
                return Err(CommandError::InvalidArgument(
                    "Time value cannot be negative".to_string(),
                ));
            }
            dispatcher.time = dispatcher.time.wrapping_add(val as u64);
            Ok(format!("Added {val} to the time"))
        }
        "query" => {
            if args.len() < 2 {
                return Err(CommandError::MissingArgument(
                    "Expected 'daytime', 'gametime', or 'day'".to_string(),
                ));
            }
            if args.len() > 2 {
                return Err(CommandError::InvalidArgument(
                    "Too many arguments for /time query".to_string(),
                ));
            }
            match args[1].to_ascii_lowercase().as_str() {
                "daytime" => {
                    let val = dispatcher.time % 24000;
                    Ok(format!("The time is {val}"))
                }
                "gametime" => {
                    let val = dispatcher.time;
                    Ok(format!("The time is {val}"))
                }
                "day" => {
                    let val = dispatcher.time / 24000;
                    Ok(format!("The time is {val}"))
                }
                other => Err(CommandError::InvalidArgument(format!(
                    "Unknown time query target '{other}', expected 'daytime', 'gametime', or 'day'"
                ))),
            }
        }
        other => Err(CommandError::InvalidArgument(format!(
            "Unknown subcommand '{other}', expected 'set', 'add', or 'query'"
        ))),
    }
}

fn handle_kill(
    dispatcher: &mut CommandDispatcher,
    sender: &CommandSender,
    args: &[&str],
) -> Result<String, CommandError> {
    if args.len() > 1 {
        return Err(CommandError::InvalidArgument(
            "Too many arguments for /kill".to_string(),
        ));
    }
    let sender_name = match sender {
        CommandSender::Player { name, .. } => Some(name.as_str()),
        CommandSender::Console => None,
    };

    let targets = if args.is_empty() {
        let name = sender_name.ok_or_else(|| {
            CommandError::ConsoleNotAllowed(
                "Console cannot be killed; specify a target player".to_string(),
            )
        })?;
        vec![name.to_string()]
    } else {
        let selector = TargetSelector::parse(args[0])?;
        dispatcher.resolve_targets(&selector, sender.position(), sender_name)
    };

    if targets.is_empty() {
        return Err(CommandError::PlayerNotFound(format!(
            "No player found matching selector '{}'",
            args.first().copied().unwrap_or("@s")
        )));
    }

    for name in &targets {
        if let Some(player) = dispatcher.players.get_mut(name) {
            player.is_alive = false;
        }
    }

    if targets.len() == 1 {
        Ok(format!("Killed {}", targets[0]))
    } else {
        Ok(format!("Killed {} players", targets.len()))
    }
}

fn handle_give(
    dispatcher: &mut CommandDispatcher,
    sender: &CommandSender,
    args: &[&str],
) -> Result<String, CommandError> {
    if args.len() < 2 {
        return Err(CommandError::MissingArgument(
            "Usage: /give <target> <item> [count]".to_string(),
        ));
    }
    if args.len() > 3 {
        return Err(CommandError::InvalidArgument(
            "Too many arguments for /give".to_string(),
        ));
    }

    let sender_name = match sender {
        CommandSender::Player { name, .. } => Some(name.as_str()),
        CommandSender::Console => None,
    };

    let target_selector = TargetSelector::parse(args[0])?;
    let targets = dispatcher.resolve_targets(&target_selector, sender.position(), sender_name);
    if targets.is_empty() {
        return Err(CommandError::PlayerNotFound(format!(
            "No player found matching selector '{}'",
            args[0]
        )));
    }

    let item = validate_item_id(args[1])?;

    let count: u32 = if args.len() == 3 {
        let parsed: i64 = args[2]
            .parse()
            .map_err(|_| CommandError::InvalidArgument(format!("Invalid count '{}'", args[2])))?;
        if parsed < 1 {
            return Err(CommandError::InvalidArgument(
                "Count must be at least 1".to_string(),
            ));
        }
        if parsed > 6400 {
            return Err(CommandError::InvalidArgument(
                "Count must not be larger than 6400".to_string(),
            ));
        }
        parsed as u32
    } else {
        1
    };

    for name in &targets {
        if let Some(player) = dispatcher.players.get_mut(name) {
            player.give_item(&item, count);
        }
    }

    if targets.len() == 1 {
        Ok(format!("Gave {count} [{item}] to {}", targets[0]))
    } else {
        Ok(format!(
            "Gave {count} [{item}] to {} players",
            targets.len()
        ))
    }
}
