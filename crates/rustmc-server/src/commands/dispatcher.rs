//! The command registry and dispatcher: permission checks, routing to
//! handlers, and tokenizing.

use super::*;

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
///
/// `/time` covers only `set`, `add`, and the `daytime`, `gametime`, and `day` queries over
/// one day clock. The community wiki describes Java 26.1 and later as built on world clocks
/// with different query forms, which are not implemented, so this is not a 26.3 parity claim.
pub struct CommandDispatcher {
    pub seed: i64,
    /// Mutable day clock, changed by `/time set` and `/time add`.
    pub time: u64,
    /// Elapsed world ticks. Only [`Self::advance_ticks`] changes this.
    pub(super) game_time: u64,
    pub players: HashMap<String, PlayerTarget>,
    pub(super) rng: RandomSource,
    pub(super) commands: HashMap<String, CommandRegistration>,
    pub(super) read_only: bool,
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
            game_time: 0,
            players: HashMap::new(),
            rng: RandomSource::from_world_seed(seed as u64),
            commands: HashMap::new(),
            read_only: false,
        };
        dispatcher.register_standard_commands();
        dispatcher
    }

    /// Create a dispatcher for a read-only preview session.
    ///
    /// Registers `/help` alongside standard commands, while [`Self::execute`] refuses
    /// commands that would mutate world or player state.
    pub fn read_only_preview(seed: i64) -> Self {
        let mut dispatcher = Self::with_seed(seed);
        dispatcher.read_only = true;
        dispatcher.register("help", 0, handle_help);
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

    /// Current day clock in ticks.
    pub fn time(&self) -> u64 {
        self.time
    }

    /// Set the day clock in ticks. Elapsed game time is unaffected.
    pub fn set_time(&mut self, time: u64) {
        self.time = time;
    }

    /// Elapsed world ticks, independent of the day clock.
    pub fn game_time(&self) -> u64 {
        self.game_time
    }

    /// Record `ticks` elapsed world ticks, moving the day clock with them.
    pub fn advance_ticks(&mut self, ticks: u64) {
        self.game_time = self.game_time.saturating_add(ticks);
        self.time = self.time.wrapping_add(ticks);
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
        let arg_strs: Vec<&str> = tokens[1..].iter().map(String::as_str).collect();
        if self.read_only && is_world_mutating_command(&cmd_name, &arg_strs) {
            return CommandResult::error(
                "This preview session is read-only, so the command was not applied",
            );
        }

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
        if !self.read_only
            && let CommandSender::Player {
                name,
                pos,
                yaw,
                pitch,
                ..
            } = sender
        {
            if let Some(player) = self.players.get_mut(name) {
                player.pos = *pos;
                player.yaw = *yaw;
                player.pitch = *pitch;
            } else {
                self.players.insert(
                    name.clone(),
                    PlayerTarget::new(name.clone(), *pos).with_rotation(*yaw, *pitch),
                );
            }
        }

        match (registration.handler)(self, sender, &arg_strs) {
            Ok(msg) => CommandResult::Success(msg),
            Err(err) => CommandResult::Error(err.to_string()),
        }
    }
}
