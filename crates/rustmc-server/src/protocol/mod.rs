//! Protocol decoders, encoders, and state tracking for Java Edition.
//!
//! Submodules are reached by path (`protocol::movement::...`); they are not
//! glob re-exported because several define helpers with the same names.

pub mod chat;
pub mod entity_spawn;
pub mod ids;
pub mod movement;
pub mod player_info;
pub mod slot;
