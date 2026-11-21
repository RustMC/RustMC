//! The playable multiplayer server: shared world, game loop and the
//! connections that feed it.
//!
//! Why: the discovery listener and preview session serve isolated, read-only
//! players; this module is the server players actually share.
//! Done: see submodules. Not done: see `docs/STATUS.md`.

pub mod config;
pub mod connection;
pub mod game;
pub mod join;
pub mod listener;
pub mod log;
pub mod world;

use std::fmt;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::mpsc::sync_channel;
use std::thread;

use config::ServerConfig;
use game::Game;
use world::store::ChunkStore;
use world::terrain::TerrainPalette;
use world::workers::ChunkWorkers;

use crate::chunk_adapter::RegistryTables;
use crate::world::Generator;

use crate::net::crypto::ServerKey;
use crate::net::login::LoginContext;
use crate::net::session_server::{HttpSessionServer, SessionVerifier};

/// Events each player may have queued for the game loop before its reader
/// waits; the waiting is TCP backpressure on that client alone.
pub const EVENTS_PER_PLAYER: usize = 64;

/// Why the server could not start or stopped with an error.
#[derive(Debug)]
pub enum ServerError {
    Manifest(String),
    /// The registry table is missing, invalid or lacks a default-terrain name.
    Registry(String),
    Key(crate::net::crypto::CryptoError),
    Bind(std::io::Error),
    Accept(std::io::Error),
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(error) => write!(f, "registry manifest: {error}"),
            Self::Registry(error) => write!(f, "registry table: {error}"),
            Self::Key(error) => write!(f, "server key: {error}"),
            Self::Bind(error) => write!(f, "cannot listen: {error}"),
            Self::Accept(error) => write!(f, "accept failed: {error}"),
        }
    }
}

impl std::error::Error for ServerError {}

/// Runs the server until `shutdown` is set. `on_bound` receives the
/// listening address once the socket is open.
pub fn run(
    config: ServerConfig,
    shutdown: Arc<AtomicBool>,
    verifier: Option<Arc<dyn SessionVerifier>>,
    on_bound: impl FnOnce(SocketAddr),
) -> Result<(), ServerError> {
    let manifest = Arc::new(
        crate::preview_data::load(&config.registry_manifest).map_err(ServerError::Manifest)?,
    );
    let table = std::fs::read_to_string(&config.registry_table)
        .map_err(|error| ServerError::Registry(error.to_string()))?;
    let tables = Arc::new(
        RegistryTables::from_provisioned(&table)
            .map_err(|error| ServerError::Registry(error.to_string()))?,
    );
    let palette = Arc::new(
        TerrainPalette::resolve(&tables)
            .map_err(|error| ServerError::Registry(error.to_string()))?,
    );
    // The configuration stores the signed seed; the generator hashes its bits.
    let generator = Generator::new(config.seed as u64);
    let key = Arc::new(ServerKey::generate().map_err(ServerError::Key)?);
    let verifier =
        verifier.unwrap_or_else(|| Arc::new(HttpSessionServer::new(&config.session_server)));
    let online = Arc::new(AtomicUsize::new(0));
    let config = Arc::new(config);
    let context = Arc::new(LoginContext {
        online_mode: config.online_mode,
        compression_threshold: config.compression_threshold,
        motd: config.motd.clone(),
        max_players: config.max_players,
        online_players: Arc::clone(&online),
        manifest: Arc::clone(&manifest),
        key,
        verifier,
    });
    let listener = TcpListener::bind(config.address).map_err(ServerError::Bind)?;
    let address = listener.local_addr().map_err(ServerError::Bind)?;
    log::event(
        "server_started",
        &[
            ("address", &address),
            ("online_mode", &config.online_mode),
            ("max_players", &config.max_players),
        ],
    );
    on_bound(address);
    let (events, queue) = sync_channel(config.max_players * EVENTS_PER_PLAYER);
    let results = events.clone();
    let accept = {
        let config = Arc::clone(&config);
        let shutdown = Arc::clone(&shutdown);
        thread::Builder::new()
            .name("accept".to_owned())
            .spawn(move || listener::accept_loop(listener, config, context, events, shutdown))
            .map_err(ServerError::Bind)?
    };
    let workers = ChunkWorkers::spawn(
        config.generation_workers,
        generator,
        palette,
        tables.clone(),
        results,
    )
    .map_err(ServerError::Bind)?;
    let store = ChunkStore::new(workers, tables);
    Game::new(Arc::clone(&config), manifest, online, generator, store).run(&queue, &shutdown);
    let result = accept
        .join()
        .unwrap_or_else(|_| Err(std::io::Error::other("accept thread panicked")));
    log::event("server_stopped", &[]);
    result.map_err(ServerError::Accept)
}

#[cfg(test)]
mod tests;
