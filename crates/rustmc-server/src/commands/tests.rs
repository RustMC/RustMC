use super::*;

#[test]
fn test_coordinate_parsing_absolute() {
    assert_eq!(Coordinate::parse("0").unwrap(), Coordinate::Absolute(0.0));
    assert_eq!(
        Coordinate::parse("10.5").unwrap(),
        Coordinate::Absolute(10.5)
    );
    assert_eq!(
        Coordinate::parse("-100.25").unwrap(),
        Coordinate::Absolute(-100.25)
    );
    assert_eq!(
        Coordinate::parse("+42.0").unwrap(),
        Coordinate::Absolute(42.0)
    );
    assert!(Coordinate::Absolute(10.0).is_absolute());
    assert!(!Coordinate::Absolute(10.0).is_relative());
    assert!(!Coordinate::Absolute(10.0).is_local());
}

#[test]
fn test_coordinate_parsing_relative() {
    assert_eq!(Coordinate::parse("~").unwrap(), Coordinate::Relative(0.0));
    assert_eq!(Coordinate::parse("~0").unwrap(), Coordinate::Relative(0.0));
    assert_eq!(
        Coordinate::parse("~1.5").unwrap(),
        Coordinate::Relative(1.5)
    );
    assert_eq!(
        Coordinate::parse("~-2.0").unwrap(),
        Coordinate::Relative(-2.0)
    );
    assert_eq!(
        Coordinate::parse("~+5.25").unwrap(),
        Coordinate::Relative(5.25)
    );
    assert!(Coordinate::Relative(0.0).is_relative());
    assert!(!Coordinate::Relative(0.0).is_absolute());
    assert!(!Coordinate::Relative(0.0).is_local());
}

#[test]
fn test_coordinate_parsing_local() {
    assert_eq!(Coordinate::parse("^").unwrap(), Coordinate::Local(0.0));
    assert_eq!(Coordinate::parse("^0").unwrap(), Coordinate::Local(0.0));
    assert_eq!(Coordinate::parse("^1.0").unwrap(), Coordinate::Local(1.0));
    assert_eq!(Coordinate::parse("^-3.5").unwrap(), Coordinate::Local(-3.5));
    assert_eq!(Coordinate::parse("^+2.0").unwrap(), Coordinate::Local(2.0));
    assert!(Coordinate::Local(0.0).is_local());
    assert!(!Coordinate::Local(0.0).is_relative());
    assert!(!Coordinate::Local(0.0).is_absolute());
}

#[test]
fn test_coordinate_parsing_invalid() {
    assert!(Coordinate::parse("").is_err());
    assert!(Coordinate::parse("   ").is_err());
    assert!(Coordinate::parse("~abc").is_err());
    assert!(Coordinate::parse("^xyz").is_err());
    assert!(Coordinate::parse("abc").is_err());
    assert!(Coordinate::parse("NaN").is_err());
    assert!(Coordinate::parse("Infinity").is_err());
    assert!(Coordinate::parse("-Infinity").is_err());
}

#[test]
fn test_coordinate_resolution() {
    let abs = Coordinate::Absolute(100.0);
    assert_eq!(abs.resolve(50.0), 100.0);
    assert_eq!(abs.resolve(-20.0), 100.0);

    let rel = Coordinate::Relative(10.0);
    assert_eq!(rel.resolve(50.0), 60.0);
    assert_eq!(rel.resolve(-20.0), -10.0);

    let rel_neg = Coordinate::Relative(-5.0);
    assert_eq!(rel_neg.resolve(50.0), 45.0);

    let local = Coordinate::Local(15.0);
    assert_eq!(local.resolve(30.0), 45.0);
}

#[test]
fn test_coordinates3_parsing_and_resolution() {
    // Absolute
    let c = Coordinates3::parse("10", "20", "30").unwrap();
    assert_eq!(c.resolve((0.0, 0.0, 0.0)), (10.0, 20.0, 30.0));
    assert_eq!(c.resolve((50.0, 60.0, 70.0)), (10.0, 20.0, 30.0));

    // Relative
    let c_rel = Coordinates3::parse("~5", "~-2", "~").unwrap();
    assert_eq!(c_rel.resolve((100.0, 64.0, 200.0)), (105.0, 62.0, 200.0));

    // Local
    let c_loc = Coordinates3::parse("^1", "^2", "^3").unwrap();
    assert_eq!(c_loc.resolve((10.0, 20.0, 30.0)), (11.0, 22.0, 33.0));

    // Mixed relative and absolute
    let c_mixed = Coordinates3::parse("~5", "64", "~-5").unwrap();
    assert_eq!(c_mixed.resolve((10.0, 20.0, 30.0)), (15.0, 64.0, 25.0));

    // FromStr
    let from_str: Coordinates3 = "~1.5 ~2.5 ~3.5".parse().unwrap();
    assert_eq!(from_str.resolve((0.0, 0.0, 0.0)), (1.5, 2.5, 3.5));

    // Reject mixing local and world coordinates
    assert!(Coordinates3::parse("^1", "~2", "3").is_err());
    assert!(Coordinates3::parse("^1", "64", "^3").is_err());
    assert!(Coordinates3::parse("~1", "^2", "~3").is_err());
    assert!("^1 ~2 3".parse::<Coordinates3>().is_err());
    assert!("1 2".parse::<Coordinates3>().is_err());
    assert!("1 2 3 4".parse::<Coordinates3>().is_err());
}

#[test]
fn test_target_selector_parsing() {
    assert_eq!(
        TargetSelector::parse("@a").unwrap(),
        TargetSelector::AllPlayers
    );
    assert_eq!(
        TargetSelector::parse("@p").unwrap(),
        TargetSelector::NearestPlayer
    );
    assert_eq!(TargetSelector::parse("@s").unwrap(), TargetSelector::Self_);
    assert_eq!(
        TargetSelector::parse("@r").unwrap(),
        TargetSelector::RandomPlayer
    );
    assert_eq!(
        TargetSelector::parse("Steve").unwrap(),
        TargetSelector::Named("Steve".to_string())
    );
    assert_eq!(
        TargetSelector::parse("Alex_99").unwrap(),
        TargetSelector::Named("Alex_99".to_string())
    );

    // Invalid selectors
    assert!(TargetSelector::parse("@e").is_err());
    assert!(TargetSelector::parse("@all").is_err());
    assert!(TargetSelector::parse("").is_err());
    assert!(TargetSelector::parse("   ").is_err());
    // Name too long (> 16 chars)
    assert!(TargetSelector::parse("PlayerNameTooLongForMinecraft").is_err());
    // Invalid characters
    assert!(TargetSelector::parse("Bad Name").is_err());
    assert!(TargetSelector::parse("Player!").is_err());
}

#[test]
fn test_target_selector_resolution() {
    let mut dispatcher = CommandDispatcher::with_seed(42);
    dispatcher.add_player(PlayerTarget::new("Alice", (0.0, 64.0, 0.0)));
    dispatcher.add_player(PlayerTarget::new("Bob", (100.0, 64.0, 100.0)));
    dispatcher.add_player(PlayerTarget::new("Charlie", (5.0, 64.0, 5.0)));

    // @a
    let all = dispatcher.resolve_targets(&TargetSelector::AllPlayers, None, None);
    assert_eq!(all, vec!["Alice", "Bob", "Charlie"]);

    // @p relative to (4.0, 64.0, 4.0) -> Charlie is nearest
    let nearest =
        dispatcher.resolve_targets(&TargetSelector::NearestPlayer, Some((4.0, 64.0, 4.0)), None);
    assert_eq!(nearest, vec!["Charlie"]);

    // @p relative to (99.0, 64.0, 99.0) -> Bob is nearest
    let nearest_bob = dispatcher.resolve_targets(
        &TargetSelector::NearestPlayer,
        Some((99.0, 64.0, 99.0)),
        None,
    );
    assert_eq!(nearest_bob, vec!["Bob"]);

    // @s
    let self_match = dispatcher.resolve_targets(&TargetSelector::Self_, None, Some("Alice"));
    assert_eq!(self_match, vec!["Alice"]);

    let self_console = dispatcher.resolve_targets(&TargetSelector::Self_, None, None);
    assert!(self_console.is_empty());

    // @r
    let random = dispatcher.resolve_targets(&TargetSelector::RandomPlayer, None, None);
    assert_eq!(random.len(), 1);
    assert!(["Alice", "Bob", "Charlie"].contains(&random[0].as_str()));

    // Named
    let named = dispatcher.resolve_targets(&TargetSelector::Named("bob".to_string()), None, None);
    assert_eq!(named, vec!["Bob"]);

    let non_existent =
        dispatcher.resolve_targets(&TargetSelector::Named("Unknown".to_string()), None, None);
    assert!(non_existent.is_empty());
}

#[test]
fn test_gamemode_parsing() {
    assert_eq!(GameMode::parse("survival").unwrap(), GameMode::Survival);
    assert_eq!(GameMode::parse("s").unwrap(), GameMode::Survival);
    assert_eq!(GameMode::parse("0").unwrap(), GameMode::Survival);

    assert_eq!(GameMode::parse("creative").unwrap(), GameMode::Creative);
    assert_eq!(GameMode::parse("c").unwrap(), GameMode::Creative);
    assert_eq!(GameMode::parse("1").unwrap(), GameMode::Creative);

    assert_eq!(GameMode::parse("adventure").unwrap(), GameMode::Adventure);
    assert_eq!(GameMode::parse("a").unwrap(), GameMode::Adventure);
    assert_eq!(GameMode::parse("2").unwrap(), GameMode::Adventure);

    assert_eq!(GameMode::parse("spectator").unwrap(), GameMode::Spectator);
    assert_eq!(GameMode::parse("sp").unwrap(), GameMode::Spectator);
    assert_eq!(GameMode::parse("3").unwrap(), GameMode::Spectator);

    // Case insensitivity
    assert_eq!(GameMode::parse("CREATIVE").unwrap(), GameMode::Creative);
    assert_eq!(GameMode::parse("SuRvIvAl").unwrap(), GameMode::Survival);

    // Invalid
    assert!(GameMode::parse("invalid").is_err());
    assert!(GameMode::parse("4").is_err());
    assert!(GameMode::parse("").is_err());
}

#[test]
fn test_tp_execution() {
    let mut dispatcher = CommandDispatcher::new();
    dispatcher.add_player(PlayerTarget::new("Steve", (0.0, 64.0, 0.0)));
    dispatcher.add_player(PlayerTarget::new("Alex", (100.0, 70.0, 100.0)));

    let steve_sender = CommandSender::player("Steve", (0.0, 64.0, 0.0), 2);

    // 1. Teleport self to absolute coordinates
    let res = dispatcher.execute(&steve_sender, "/tp 10 20 30");
    assert!(res.is_success());
    assert_eq!(
        dispatcher.get_player("Steve").unwrap().pos,
        (10.0, 20.0, 30.0)
    );

    // 2. Teleport self to relative coordinates
    let res = dispatcher.execute(
        &CommandSender::player("Steve", (10.0, 20.0, 30.0), 2),
        "/tp ~ ~10 ~",
    );
    assert!(res.is_success());
    assert_eq!(
        dispatcher.get_player("Steve").unwrap().pos,
        (10.0, 30.0, 30.0)
    );

    // 3. Teleport self with local coordinates
    let res = dispatcher.execute(
        &CommandSender::player("Steve", (10.0, 30.0, 30.0), 2),
        "/tp ^ ^5 ^",
    );
    assert!(res.is_success());
    assert_eq!(
        dispatcher.get_player("Steve").unwrap().pos,
        (10.0, 35.0, 30.0)
    );

    // 4. Teleport self to another player
    let res = dispatcher.execute(
        &CommandSender::player("Steve", (10.0, 35.0, 30.0), 2),
        "/tp Alex",
    );
    assert!(res.is_success());
    assert_eq!(
        dispatcher.get_player("Steve").unwrap().pos,
        (100.0, 70.0, 100.0)
    );

    // 5. Teleport another target to absolute coordinates (via Console)
    let res = dispatcher.execute(&CommandSender::console(), "/tp Alex 50 80 50");
    assert!(res.is_success());
    assert_eq!(
        dispatcher.get_player("Alex").unwrap().pos,
        (50.0, 80.0, 50.0)
    );

    // 6. Teleport target to relative coordinates (relative to target's position)
    let res = dispatcher.execute(&CommandSender::console(), "/tp Alex ~ ~5 ~-10");
    assert!(res.is_success());
    assert_eq!(
        dispatcher.get_player("Alex").unwrap().pos,
        (50.0, 85.0, 40.0)
    );

    // 7. Teleport target to another player
    let res = dispatcher.execute(&CommandSender::console(), "/tp Alex Steve");
    assert!(res.is_success());
    assert_eq!(
        dispatcher.get_player("Alex").unwrap().pos,
        dispatcher.get_player("Steve").unwrap().pos
    );

    // 8. Teleport all players (@a) to a position
    let res = dispatcher.execute(&CommandSender::console(), "/tp @a 0 128 0");
    assert!(res.is_success());
    assert_eq!(
        dispatcher.get_player("Steve").unwrap().pos,
        (0.0, 128.0, 0.0)
    );
    assert_eq!(
        dispatcher.get_player("Alex").unwrap().pos,
        (0.0, 128.0, 0.0)
    );

    // 9. Command without leading slash
    let res = dispatcher.execute(&CommandSender::console(), "tp Steve 1 2 3");
    assert!(res.is_success());
    assert_eq!(dispatcher.get_player("Steve").unwrap().pos, (1.0, 2.0, 3.0));
}

#[test]
fn test_gamemode_execution() {
    let mut dispatcher = CommandDispatcher::new();
    dispatcher.add_player(PlayerTarget::new("Steve", (0.0, 64.0, 0.0)));
    dispatcher.add_player(PlayerTarget::new("Alex", (10.0, 64.0, 10.0)));

    let steve_sender = CommandSender::player("Steve", (0.0, 64.0, 0.0), 2);

    // 1. Set own gamemode
    let res = dispatcher.execute(&steve_sender, "/gamemode creative");
    assert!(res.is_success());
    assert_eq!(
        dispatcher.get_player("Steve").unwrap().gamemode,
        GameMode::Creative
    );

    // 2. Set another player's gamemode
    let res = dispatcher.execute(&steve_sender, "/gamemode adventure Alex");
    assert!(res.is_success());
    assert_eq!(
        dispatcher.get_player("Alex").unwrap().gamemode,
        GameMode::Adventure
    );

    // 3. Shorthand mode
    let res = dispatcher.execute(&steve_sender, "/gamemode sp Steve");
    assert!(res.is_success());
    assert_eq!(
        dispatcher.get_player("Steve").unwrap().gamemode,
        GameMode::Spectator
    );

    // 4. Set all players gamemode
    let res = dispatcher.execute(&CommandSender::console(), "/gamemode survival @a");
    assert!(res.is_success());
    assert_eq!(
        dispatcher.get_player("Steve").unwrap().gamemode,
        GameMode::Survival
    );
    assert_eq!(
        dispatcher.get_player("Alex").unwrap().gamemode,
        GameMode::Survival
    );
}

#[test]
fn test_seed_execution() {
    let mut dispatcher = CommandDispatcher::with_seed(123456789);
    let res = dispatcher.execute(&CommandSender::console(), "/seed");
    assert!(res.is_success());
    assert_eq!(res.message(), "Seed: [123456789]");

    let mut negative_seed = CommandDispatcher::with_seed(-987654321);
    let res = negative_seed.execute(&CommandSender::console(), "/seed");
    assert!(res.is_success());
    assert_eq!(res.message(), "Seed: [-987654321]");

    // Player with permission level 2
    let sender = CommandSender::player("Operator", (0.0, 64.0, 0.0), 2);
    let res = dispatcher.execute(&sender, "/seed");
    assert!(res.is_success());
    assert_eq!(res.message(), "Seed: [123456789]");
}

#[test]
fn test_time_execution() {
    let mut dispatcher = CommandDispatcher::new();
    let console = CommandSender::console();

    // time set day (1000)
    let res = dispatcher.execute(&console, "/time set day");
    assert!(res.is_success());
    assert_eq!(dispatcher.time(), 1000);

    // time set noon (6000)
    let res = dispatcher.execute(&console, "/time set noon");
    assert!(res.is_success());
    assert_eq!(dispatcher.time(), 6000);

    // time set night (13000)
    let res = dispatcher.execute(&console, "/time set night");
    assert!(res.is_success());
    assert_eq!(dispatcher.time(), 13000);

    // time set midnight (18000)
    let res = dispatcher.execute(&console, "/time set midnight");
    assert!(res.is_success());
    assert_eq!(dispatcher.time(), 18000);

    // time set numeric
    let res = dispatcher.execute(&console, "/time set 24000");
    assert!(res.is_success());
    assert_eq!(dispatcher.time(), 24000);

    // time add
    let res = dispatcher.execute(&console, "/time add 1000");
    assert!(res.is_success());
    assert_eq!(dispatcher.time(), 25000);

    // time query daytime (25000 % 24000 = 1000)
    let res = dispatcher.execute(&console, "/time query daytime");
    assert!(res.is_success());
    assert_eq!(res.message(), "The time is 1000");

    // time query gametime (25000)
    let res = dispatcher.execute(&console, "/time query gametime");
    assert!(res.is_success());
    assert_eq!(res.message(), "The time is 25000");

    // time query day (25000 / 24000 = 1)
    let res = dispatcher.execute(&console, "/time query day");
    assert!(res.is_success());
    assert_eq!(res.message(), "The time is 1");
}

#[test]
fn test_kill_execution() {
    let mut dispatcher = CommandDispatcher::new();
    dispatcher.add_player(PlayerTarget::new("Steve", (0.0, 64.0, 0.0)));
    dispatcher.add_player(PlayerTarget::new("Alex", (10.0, 64.0, 10.0)));

    let steve_sender = CommandSender::player("Steve", (0.0, 64.0, 0.0), 2);

    // Kill self
    let res = dispatcher.execute(&steve_sender, "/kill");
    assert!(res.is_success());
    assert!(!dispatcher.get_player("Steve").unwrap().is_alive);
    assert!(dispatcher.get_player("Alex").unwrap().is_alive);

    // Kill specific target
    let res = dispatcher.execute(&CommandSender::console(), "/kill Alex");
    assert!(res.is_success());
    assert!(!dispatcher.get_player("Alex").unwrap().is_alive);

    // Kill all players
    dispatcher.get_player_mut("Steve").unwrap().is_alive = true;
    dispatcher.get_player_mut("Alex").unwrap().is_alive = true;
    let res = dispatcher.execute(&CommandSender::console(), "/kill @a");
    assert!(res.is_success());
    assert!(!dispatcher.get_player("Steve").unwrap().is_alive);
    assert!(!dispatcher.get_player("Alex").unwrap().is_alive);
}

#[test]
fn test_give_execution() {
    let mut dispatcher = CommandDispatcher::new();
    dispatcher.add_player(PlayerTarget::new("Steve", (0.0, 64.0, 0.0)));
    dispatcher.add_player(PlayerTarget::new("Alex", (10.0, 64.0, 10.0)));

    // Default count 1
    let res = dispatcher.execute(&CommandSender::console(), "/give Steve diamond");
    assert!(res.is_success());
    assert_eq!(
        dispatcher
            .get_player("Steve")
            .unwrap()
            .item_count("diamond"),
        1
    );
    assert_eq!(
        dispatcher
            .get_player("Steve")
            .unwrap()
            .item_count("minecraft:diamond"),
        1
    );

    // Explicit count
    let res = dispatcher.execute(
        &CommandSender::console(),
        "/give Steve minecraft:diamond 64",
    );
    assert!(res.is_success());
    assert_eq!(
        dispatcher
            .get_player("Steve")
            .unwrap()
            .item_count("diamond"),
        65
    );

    // Give to all players
    let res = dispatcher.execute(&CommandSender::console(), "/give @a iron_ingot 16");
    assert!(res.is_success());
    assert_eq!(
        dispatcher
            .get_player("Steve")
            .unwrap()
            .item_count("iron_ingot"),
        16
    );
    assert_eq!(
        dispatcher
            .get_player("Alex")
            .unwrap()
            .item_count("iron_ingot"),
        16
    );
}

#[test]
fn test_error_handling_invalid_syntax_and_unknown() {
    let mut dispatcher = CommandDispatcher::new();
    let console = CommandSender::console();

    // Empty input
    assert!(dispatcher.execute(&console, "").is_error());
    assert!(dispatcher.execute(&console, "   ").is_error());
    assert!(dispatcher.execute(&console, "/").is_error());
    assert!(dispatcher.execute(&console, " / ").is_error());

    // Unknown command
    let res = dispatcher.execute(&console, "/unknowncommand");
    assert!(res.is_error());
    assert!(res.message().contains("Unknown command 'unknowncommand'"));

    // Unclosed quote
    let res = dispatcher.execute(&console, "say \"unclosed string");
    assert!(res.is_error());
    assert!(res.message().contains("Unclosed quote"));

    // Mixing world and local coordinates in /tp
    dispatcher.add_player(PlayerTarget::new("Steve", (0.0, 64.0, 0.0)));
    let res = dispatcher.execute(&console, "/tp Steve ^1 ~2 3");
    assert!(res.is_error());
    assert!(
        res.message()
            .contains("Cannot mix world and local coordinates")
    );
}

#[test]
fn test_error_handling_missing_arguments() {
    let mut dispatcher = CommandDispatcher::new();
    let console = CommandSender::console();

    // /tp without args
    assert!(dispatcher.execute(&console, "/tp").is_error());

    // /gamemode without args
    assert!(dispatcher.execute(&console, "/gamemode").is_error());

    // /time without args
    assert!(dispatcher.execute(&console, "/time").is_error());
    assert!(dispatcher.execute(&console, "/time set").is_error());
    assert!(dispatcher.execute(&console, "/time add").is_error());
    assert!(dispatcher.execute(&console, "/time query").is_error());

    // /give with insufficient args
    assert!(dispatcher.execute(&console, "/give").is_error());
    assert!(dispatcher.execute(&console, "/give Steve").is_error());
}

#[test]
fn test_error_handling_out_of_range() {
    let mut dispatcher = CommandDispatcher::new();
    dispatcher.add_player(PlayerTarget::new("Steve", (0.0, 64.0, 0.0)));
    let console = CommandSender::console();

    // /give count < 1
    let res = dispatcher.execute(&console, "/give Steve diamond 0");
    assert!(res.is_error());
    assert!(res.message().contains("Count must be at least 1"));

    let res = dispatcher.execute(&console, "/give Steve diamond -5");
    assert!(res.is_error());
    assert!(res.message().contains("Count must be at least 1"));

    // /give count > 6400
    let res = dispatcher.execute(&console, "/give Steve diamond 6401");
    assert!(res.is_error());
    assert!(res.message().contains("Count must not be larger than 6400"));

    // /time negative
    let res = dispatcher.execute(&console, "/time set -100");
    assert!(res.is_error());
    assert!(res.message().contains("Time value cannot be negative"));

    let res = dispatcher.execute(&console, "/time add -50");
    assert!(res.is_error());
    assert!(res.message().contains("Time value cannot be negative"));

    // /tp coordinate beyond world limits (> 30,000,000)
    let res = dispatcher.execute(&console, "/tp Steve 30000001 64 0");
    assert!(res.is_error());
    assert!(res.message().contains("out of bounds"));

    let res = dispatcher.execute(&console, "/tp Steve -30000001 64 0");
    assert!(res.is_error());
    assert!(res.message().contains("out of bounds"));
}

#[test]
fn test_error_handling_invalid_types_and_targets() {
    let mut dispatcher = CommandDispatcher::new();
    dispatcher.add_player(PlayerTarget::new("Steve", (0.0, 64.0, 0.0)));
    let console = CommandSender::console();

    // Invalid gamemode
    let res = dispatcher.execute(&console, "/gamemode godmode Steve");
    assert!(res.is_error());
    assert!(res.message().contains("Unknown game mode"));

    // Invalid time subcommands and values
    let res = dispatcher.execute(&console, "/time invalid 100");
    assert!(res.is_error());
    assert!(res.message().contains("Unknown subcommand"));

    let res = dispatcher.execute(&console, "/time set daytime_not_a_num");
    assert!(res.is_error());
    assert!(res.message().contains("Invalid time value"));

    let res = dispatcher.execute(&console, "/time query unknown_query");
    assert!(res.is_error());
    assert!(res.message().contains("Unknown time query target"));

    // Non-existent target
    let res = dispatcher.execute(&console, "/tp UnknownPlayer 0 64 0");
    assert!(res.is_error());
    assert!(res.message().contains("No player found matching selector"));

    let res = dispatcher.execute(&console, "/gamemode survival UnknownPlayer");
    assert!(res.is_error());
    assert!(res.message().contains("No player found matching selector"));

    let res = dispatcher.execute(&console, "/kill UnknownPlayer");
    assert!(res.is_error());
    assert!(res.message().contains("No player found matching selector"));

    let res = dispatcher.execute(&console, "/give UnknownPlayer diamond 1");
    assert!(res.is_error());
    assert!(res.message().contains("No player found matching selector"));

    // Invalid item identifier
    let res = dispatcher.execute(&console, "/give Steve \"Bad Item Name!\" 1");
    assert!(res.is_error());
    assert!(res.message().contains("Invalid item"));

    // Console restrictions
    let res = dispatcher.execute(&console, "/tp 0 64 0");
    assert!(res.is_error());
    assert!(res.message().contains("Console cannot be teleported"));

    let res = dispatcher.execute(&console, "/tp Steve");
    assert!(res.is_error());
    assert!(
        res.message()
            .contains("Console must specify both a target and a destination")
    );

    let res = dispatcher.execute(&console, "/gamemode survival");
    assert!(res.is_error());
    assert!(
        res.message()
            .contains("Console must specify a target player")
    );

    let res = dispatcher.execute(&console, "/kill");
    assert!(res.is_error());
    assert!(res.message().contains("Console cannot be killed"));
}

#[test]
fn test_permission_checks() {
    let mut dispatcher = CommandDispatcher::with_seed(1234);
    dispatcher.add_player(PlayerTarget::new("RegularPlayer", (0.0, 64.0, 0.0)));
    dispatcher.add_player(PlayerTarget::new("OperatorPlayer", (0.0, 64.0, 0.0)));

    let regular = CommandSender::player("RegularPlayer", (0.0, 64.0, 0.0), 0);
    let operator = CommandSender::player("OperatorPlayer", (0.0, 64.0, 0.0), 2);
    let console = CommandSender::console();

    // Regular player (level 0) is denied for all operator commands
    assert!(dispatcher.execute(&regular, "/tp 10 20 30").is_error());
    assert!(
        dispatcher
            .execute(&regular, "/gamemode creative")
            .is_error()
    );
    assert!(dispatcher.execute(&regular, "/seed").is_error());
    assert!(dispatcher.execute(&regular, "/time set day").is_error());
    assert!(dispatcher.execute(&regular, "/kill").is_error());
    assert!(
        dispatcher
            .execute(&regular, "/give RegularPlayer diamond")
            .is_error()
    );

    // Operator (level 2) is permitted
    assert!(dispatcher.execute(&operator, "/tp 10 20 30").is_success());
    assert!(
        dispatcher
            .execute(&operator, "/gamemode creative")
            .is_success()
    );
    assert!(dispatcher.execute(&operator, "/seed").is_success());
    assert!(dispatcher.execute(&operator, "/time set day").is_success());
    assert!(
        dispatcher
            .execute(&operator, "/give OperatorPlayer diamond 1")
            .is_success()
    );
    assert!(dispatcher.execute(&operator, "/kill").is_success());

    // Console (level 4) is permitted
    assert!(dispatcher.execute(&console, "/seed").is_success());
}

#[test]
fn test_tp_local_coordinates_with_rotation() {
    let mut dispatcher = CommandDispatcher::new();

    // Facing South (yaw = 0.0, pitch = 0.0): forward is +Z
    dispatcher
        .add_player(PlayerTarget::new("SouthFacing", (0.0, 64.0, 0.0)).with_rotation(0.0, 0.0));
    let sender_south =
        CommandSender::player_with_rotation("SouthFacing", (0.0, 64.0, 0.0), 2, 0.0, 0.0);
    let res = dispatcher.execute(&sender_south, "/tp ^ ^ ^10");
    assert!(res.is_success());
    let pos = dispatcher.get_player("SouthFacing").unwrap().pos;
    assert!((pos.0 - 0.0).abs() < 1e-4);
    assert!((pos.1 - 64.0).abs() < 1e-4);
    assert!((pos.2 - 10.0).abs() < 1e-4);

    // Facing West (yaw = 90.0, pitch = 0.0): forward is -X
    dispatcher
        .add_player(PlayerTarget::new("WestFacing", (0.0, 64.0, 0.0)).with_rotation(90.0, 0.0));
    let sender_west =
        CommandSender::player_with_rotation("WestFacing", (0.0, 64.0, 0.0), 2, 90.0, 0.0);
    let res = dispatcher.execute(&sender_west, "/tp ^ ^ ^10");
    assert!(res.is_success());
    let pos = dispatcher.get_player("WestFacing").unwrap().pos;
    assert!((pos.0 - (-10.0)).abs() < 1e-4);
    assert!((pos.1 - 64.0).abs() < 1e-4);
    assert!((pos.2 - 0.0).abs() < 1e-4);

    // Looking Up (yaw = 0.0, pitch = -90.0): forward is +Y
    dispatcher
        .add_player(PlayerTarget::new("UpFacing", (0.0, 64.0, 0.0)).with_rotation(0.0, -90.0));
    let sender_up =
        CommandSender::player_with_rotation("UpFacing", (0.0, 64.0, 0.0), 2, 0.0, -90.0);
    let res = dispatcher.execute(&sender_up, "/tp ^ ^ ^5");
    assert!(res.is_success());
    let pos = dispatcher.get_player("UpFacing").unwrap().pos;
    assert!((pos.0 - 0.0).abs() < 1e-4);
    assert!((pos.1 - 69.0).abs() < 1e-4);
    assert!((pos.2 - 0.0).abs() < 1e-4);

    // Local coordinates executed by console without target rotation must fail cleanly
    let res_console = dispatcher.execute(&CommandSender::console(), "/tp ^ ^ ^5");
    assert!(res_console.is_error());
}

#[test]
fn test_tp_multi_target_transactional_rollback() {
    let mut dispatcher = CommandDispatcher::new();
    dispatcher.add_player(PlayerTarget::new("PlayerA", (0.0, 100.0, 0.0)));
    dispatcher.add_player(PlayerTarget::new("PlayerB", (29_999_990.0, 100.0, 0.0)));

    // PlayerA's relative destination would be valid (X=20 <= 30,000,000),
    // but PlayerB's relative destination would exceed world boundaries (X=30,000,010 > 30,000,000).
    let res = dispatcher.execute(&CommandSender::console(), "/tp @a ~20 ~ ~");
    assert!(res.is_error());

    // Transactional rollback guarantee: PlayerA MUST NOT have moved!
    assert_eq!(
        dispatcher.get_player("PlayerA").unwrap().pos,
        (0.0, 100.0, 0.0),
        "PlayerA position must be rolled back on mixed-validity failure"
    );
    assert_eq!(
        dispatcher.get_player("PlayerB").unwrap().pos,
        (29_999_990.0, 100.0, 0.0),
        "PlayerB position must remain untouched"
    );
}
