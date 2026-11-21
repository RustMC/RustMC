//! The shared world of the playable server: columns of block-state ids,
//! default terrain generation and chunk streaming.
//!
//! Why: every player reads and changes the same blocks.
//! Done: see submodules. Not done: persistence (track C).

pub mod column;
pub mod store;
pub mod terrain;
pub mod view;
pub mod workers;
