//! The playable multiplayer server: shared world, game loop and the
//! connections that feed it.
//!
//! Why: the discovery listener and preview session serve isolated, read-only
//! players; this module is the server players actually share.
//! Done: see submodules. Not done: see `docs/STATUS.md`.

pub mod join;
