//! Network layer of the playable server: connection framing, encryption,
//! player identity and the login sequence.
//!
//! Why: the preview session (`discovery_java`) serves one isolated player
//! per connection; the server needs shared, authenticated sessions.
//! Done: see each submodule. Not done: proxy protocols, transfer packets.

pub mod crypto;
pub mod identity;
