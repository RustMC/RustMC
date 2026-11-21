//! Handlers for the built-in commands and their argument validation.

use super::*;

/// Tokenize command input string by whitespace, respecting quotes.
pub fn tokenize_command(input: &str) -> Result<Vec<String>, CommandError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    // Separate from `current` being empty, so `""` still yields an empty argument.
    let mut token_started = false;
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
                    token_started = true;
                }
            }
            ch if ch.is_whitespace() => {
                if in_quote.is_some() {
                    current.push(ch);
                } else if token_started {
                    tokens.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            ch => {
                current.push(ch);
                token_started = true;
            }
        }
    }

    if in_quote.is_some() {
        return Err(CommandError::InvalidSyntax(
            "Unclosed quote in command".to_string(),
        ));
    }
    if token_started {
        tokens.push(current);
    }

    Ok(tokens)
}

/// Validate that coordinates are finite and bounded within world border limits.
pub(super) fn validate_coordinate_bounds(pos: (f64, f64, f64)) -> Result<(), CommandError> {
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

/// Resolve a `/tp` destination once from the command source, never from a target.
///
/// The console has no position or rotation in this dispatcher, so it may only use absolute
/// coordinates: its real execution origin depends on world spawn, which is not modelled here.
pub(super) fn resolve_source_destination(
    coords: &Coordinates3,
    sender: &CommandSender,
) -> Result<(f64, f64, f64), CommandError> {
    let origin = match sender.position() {
        Some(pos) => pos,
        None if coords.is_absolute() => (0.0, 0.0, 0.0),
        None => {
            return Err(CommandError::ConsoleNotAllowed(
                "Relative and local coordinates require an in-world sender".to_string(),
            ));
        }
    };
    let dest_pos = coords.resolve_with_rotation(origin, sender.rotation())?;
    validate_coordinate_bounds(dest_pos)?;
    Ok(dest_pos)
}

/// Validate and normalize a namespaced Minecraft item identifier.
pub(super) fn validate_item_id(item: &str) -> Result<String, CommandError> {
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

pub(super) fn handle_tp(
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
            let (dest_pos, dest_yaw, dest_pitch) = dispatcher
                .players
                .get(dest_name)
                .map(|dest| (dest.pos, dest.yaw, dest.pitch))
                .ok_or_else(|| {
                    CommandError::PlayerNotFound(format!(
                        "Destination player not found: {dest_name}"
                    ))
                })?;

            if let Some(player) = dispatcher.players.get_mut(&sender_name) {
                player.pos = dest_pos;
                player.yaw = dest_yaw;
                player.pitch = dest_pitch;
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
            let (dest_pos, dest_yaw, dest_pitch) = dispatcher
                .players
                .get(dest_name)
                .map(|dest| (dest.pos, dest.yaw, dest.pitch))
                .ok_or_else(|| {
                    CommandError::PlayerNotFound(format!(
                        "Destination player not found: {dest_name}"
                    ))
                })?;

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
                    player.yaw = dest_yaw;
                    player.pitch = dest_pitch;
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

            // One destination for every target, validated before anyone moves. Targets keep
            // their own rotation.
            let dest_pos = resolve_source_destination(&coords, sender)?;
            for name in &targets {
                if let Some(player) = dispatcher.players.get_mut(name) {
                    player.pos = dest_pos;
                }
            }
            if targets.len() == 1 {
                Ok(format!(
                    "Teleported {} to {}, {}, {}",
                    targets[0], dest_pos.0, dest_pos.1, dest_pos.2
                ))
            } else {
                Ok(format!(
                    "Teleported {} players to {}, {}, {}",
                    targets.len(),
                    dest_pos.0,
                    dest_pos.1,
                    dest_pos.2
                ))
            }
        }
        _ => Err(CommandError::InvalidArgument(
            "Too many arguments for /tp".to_string(),
        )),
    }
}

pub(super) fn handle_gamemode(
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

pub(super) fn handle_seed(
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

pub(super) fn handle_time(
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
                "day" => TIME_DAY,
                "noon" => TIME_NOON,
                "night" => TIME_NIGHT,
                "midnight" => TIME_MIDNIGHT,
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
                    let val = dispatcher.time % TICKS_PER_DAY;
                    Ok(format!("The time is {val}"))
                }
                "gametime" => {
                    let val = dispatcher.game_time;
                    Ok(format!("The time is {val}"))
                }
                "day" => {
                    let val = dispatcher.time / TICKS_PER_DAY;
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

pub(super) fn handle_kill(
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

pub(super) fn handle_give(
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
        if parsed > i64::from(MAX_GIVE_COUNT) {
            return Err(CommandError::InvalidArgument(format!(
                "Count must not be larger than {MAX_GIVE_COUNT}"
            )));
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

pub(super) fn is_world_mutating_command(cmd_name: &str, args: &[&str]) -> bool {
    match cmd_name {
        "tp" | "teleport" | "gamemode" | "give" | "kill" => true,
        "time" => args
            .first()
            .is_some_and(|sub| sub.eq_ignore_ascii_case("set") || sub.eq_ignore_ascii_case("add")),
        _ => false,
    }
}

pub(super) fn handle_help(
    dispatcher: &mut CommandDispatcher,
    _sender: &CommandSender,
    args: &[&str],
) -> Result<String, CommandError> {
    if args.is_empty() {
        let mut names: Vec<&str> = dispatcher
            .commands
            .values()
            .map(|reg| reg.name.as_str())
            .collect();
        names.sort_unstable();
        let list = names
            .into_iter()
            .map(|name| format!("/{name}"))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!("Available commands: {list}"))
    } else {
        let topic = args[0]
            .strip_prefix('/')
            .unwrap_or(args[0])
            .to_ascii_lowercase();
        if dispatcher.commands.contains_key(&topic) {
            Ok(format!("/{topic}"))
        } else {
            Err(CommandError::InvalidArgument(format!(
                "Help topic '/{topic}' not found"
            )))
        }
    }
}
