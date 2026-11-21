//! Accepts TCP connections and starts a connection for each, up to a bound.
//!
//! Why: the listener must not let unauthenticated sockets exhaust threads.
//! Done: a cap on open connections (joined plus logging in), shutdown
//! polling. Not done: per-IP limits.

use std::io;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::thread;
use std::time::Duration;

use super::config::ServerConfig;
use super::connection::{self, Event};
use super::log;
use crate::net::login::LoginContext;

/// Sockets allowed to be logging in at once, beyond the player limit, so a
/// full server can still answer status pings and refuse logins politely.
pub const LOGIN_SLOTS: usize = 16;
/// How often the accept loop checks for shutdown when no client connects.
const ACCEPT_POLL: Duration = Duration::from_millis(50);

/// Counts one open connection until dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Accepts until `shutdown` is set.
pub fn accept_loop(
    listener: TcpListener,
    config: Arc<ServerConfig>,
    context: Arc<LoginContext>,
    events: SyncSender<Event>,
    shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    listener.set_nonblocking(true)?;
    let open = Arc::new(AtomicUsize::new(0));
    let next_id = AtomicU64::new(1);
    let limit = config.max_players + LOGIN_SLOTS;
    while !shutdown.load(Ordering::SeqCst) {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if open.load(Ordering::SeqCst) >= limit {
            log::event("connection_refused", &[("reason", &"capacity")]);
            continue;
        }
        stream.set_nonblocking(false)?;
        open.fetch_add(1, Ordering::SeqCst);
        let slot = Slot(Arc::clone(&open));
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        if let Err(error) = connection::spawn(
            stream,
            id,
            Arc::clone(&config),
            Arc::clone(&context),
            events.clone(),
            slot,
        ) {
            log::event("connection_failed", &[("error", &error)]);
        }
    }
    Ok(())
}
