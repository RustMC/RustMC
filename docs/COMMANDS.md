# Commands

What RustMC's command dispatcher accepts today, and where it runs.

**Where commands run.** On `main`, commands run only inside the opt-in Java
preview session. There they are parsed, checked against the sender's
permission level and answered with a chat message; they do not change the
world, because the preview has no shared, mutable world. The playable server
(Phase 1) does not accept commands yet.

**Permissions.** Each command has a required permission level. In the preview
every player has level 2, so every command below is available; `help` needs
level 0.

| Command | Level | Arguments | What it checks |
| --- | --- | --- | --- |
| `help` | 0 | none | Lists the commands the sender may use |
| `tp`, `teleport` | 2 | a destination (coordinates, `~` relative or a target selector), optionally a target first | Coordinates are finite and inside the world border; the console may only use absolute coordinates |
| `gamemode` | 2 | `survival`, `creative`, `adventure` or `spectator`, optionally a target | The mode name and the target |
| `seed` | 2 | none | Replies with the world seed |
| `time` | 2 | `set` with ticks or `day` (1000), `noon` (6000), `night` (13000), `midnight` (18000); `add` with ticks; `query` with `daytime`, `gametime` or `day` | Values are in range |
| `kill` | 2 | optionally a target | The target |
| `give` | 2 | a target, an item id and optionally a count | The item id is a valid namespaced identifier and the count is at most 6,400 |

The source of truth is `crates/rustmc-server/src/commands/`.
