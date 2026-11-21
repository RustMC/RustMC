//! The game loop's persistence: recovering journaled edits at startup,
//! autosaves, saving on shutdown and per-player state.
//!
//! Why: players must find their world and themselves as they left them,
//! even after a crash.
//! Done: startup replay of the journal onto saved or generated columns,
//! followed by a save; saving changed columns, every player and the level;
//! the journal is emptied only after a save fully succeeded; per-tick journal
//! sync. Not done: saving off the game thread (a save of many changed
//! columns pauses the tick it runs in).

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use super::{Game, Player};
use crate::net::identity::{GameProfile, uuid_text};
use crate::server::log;
use crate::server::world::store::ReadyChunk;
use crate::storage::journal::BlockEdit;
use crate::storage::world_files::{LevelData, PlayerData, StorageError};

impl Game {
    /// Applies edits recovered from the journal, then saves them so the
    /// journal can start empty.
    pub fn recover(&mut self, edits: &[BlockEdit]) -> Result<(), StorageError> {
        for edit in edits {
            let position = (edit.x.div_euclid(16), edit.z.div_euclid(16));
            if !self.store.is_known(position) {
                let column = self.source.column(position.0, position.1)?;
                self.store.insert(ReadyChunk {
                    column,
                    packet: None,
                    damaged: false,
                });
            }
            self.store.set_block(edit.x, edit.y, edit.z, edit.state);
        }
        if !edits.is_empty() {
            log::event("journal_recovered", &[("edits", &edits.len())]);
            self.save_all();
        }
        Ok(())
    }

    /// Makes this tick's journaled edits durable.
    pub(super) fn sync_journal(&mut self) {
        if let Err(error) = self.journal.sync() {
            log::event("journal_sync_failed", &[("error", &error)]);
        }
    }

    /// Saves changed columns, every player and the level; empties the
    /// journal only when all of it succeeded.
    pub(super) fn save_all(&mut self) {
        let started = Instant::now();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() as u32);
        let dirty: Vec<(i32, i32)> = self.store.dirty_columns().map(|c| (c.x, c.z)).collect();
        let columns = self.source.files.save_columns(
            self.store.dirty_columns(),
            &self.source.tables,
            self.tick,
            timestamp,
        );
        let mut complete = match columns {
            Ok(_) => {
                self.store.mark_saved(&dirty);
                true
            }
            Err(error) => {
                log::event("save_failed", &[("what", &"chunks"), ("error", &error)]);
                false
            }
        };
        for player in self.players.values() {
            complete &= self.save_player(player);
        }
        let level = LevelData {
            seed: self.seed,
            world_age: self.tick,
            day_time: self.day_time,
        };
        if let Err(error) = self.source.files.save_level(&level) {
            log::event("save_failed", &[("what", &"level"), ("error", &error)]);
            complete = false;
        }
        if complete && let Err(error) = self.journal.reset() {
            log::event("journal_reset_failed", &[("error", &error)]);
        }
        log::event(
            "world_saved",
            &[
                ("chunks", &dirty.len()),
                ("players", &self.players.len()),
                ("complete", &complete),
                ("ms", &started.elapsed().as_millis()),
            ],
        );
    }

    /// Saves one player's state; `false` when it could not be written.
    pub(super) fn save_player(&self, player: &Player) -> bool {
        let data = PlayerData {
            position: player.position,
            yaw: player.yaw,
            pitch: player.pitch,
            game_mode: player.game_mode,
        };
        match self.source.files.save_player(&player.profile.uuid, &data) {
            Ok(()) => true,
            Err(error) => {
                log::event(
                    "save_failed",
                    &[
                        ("what", &"player"),
                        ("name", &player.profile.name),
                        ("error", &error),
                    ],
                );
                false
            }
        }
    }

    /// A returning player's saved state. A damaged file is logged and the
    /// player starts at spawn; the file is replaced at their next save.
    pub(super) fn load_player(&self, profile: &GameProfile) -> Option<PlayerData> {
        match self.source.files.load_player(&profile.uuid) {
            Ok(saved) => saved,
            Err(error) => {
                log::event(
                    "player_load_failed",
                    &[("uuid", &uuid_text(&profile.uuid)), ("error", &error)],
                );
                None
            }
        }
    }
}
