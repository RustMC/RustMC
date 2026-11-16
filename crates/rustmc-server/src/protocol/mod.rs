//! Protocol decoders, encoders, and state tracking for Java Edition.
//!
//! Submodules are reached by path (`protocol::movement::...`); they are not
//! glob re-exported because several define helpers with the same names.

pub mod movement;
pub mod player_info;
