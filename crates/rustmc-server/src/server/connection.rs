//! One client connection: a reader thread that runs the login sequence and
//! then forwards play packets to the game loop, and a writer thread that
//! frames, compresses and encrypts everything sent to the client.
//!
//! Why: blocking sockets on two threads per player keep each connection
//! simple and isolated; the game loop never touches a socket.
//! Done: the login hand-off, ordered framing switches, a read-rate limit,
//! a login deadline, bounded outbound queues (a full queue disconnects the
//! client instead of growing), clean close from either side.
//! Not done: nothing known for Phase 1 A.

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use super::config::ServerConfig;
use crate::net::crypto::SHARED_SECRET_BYTES;
use crate::net::framing::{FrameDecoder, FrameEncoder, Packet};
use crate::net::login::{Action, JoinRequest, Login, LoginContext};

/// Identifies one connection for the lifetime of the server.
pub type ConnectionId = u64;

/// Bytes read from the socket per call.
const READ_CHUNK: usize = 8 * 1024;
/// How long a blocking read waits before the reader re-checks its deadline.
const READ_POLL: Duration = Duration::from_millis(250);
/// Longest a write may block before the client counts as stalled.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Window over which the read-rate limit is applied.
const RATE_WINDOW: Duration = Duration::from_secs(1);

/// Something for the writer thread to do, in order.
#[derive(Debug)]
pub enum Outgoing {
    Packet(Packet),
    /// A packet shared by several players (a broadcast).
    Shared(Arc<Packet>),
    EnableCompression(usize),
    EnableEncryption([u8; SHARED_SECRET_BYTES]),
    /// Flush and close the socket.
    Close,
}

/// The game loop's way to reach one connection.
#[derive(Debug, Clone)]
pub struct ConnectionHandle {
    pub id: ConnectionId,
    pub peer: SocketAddr,
    outbound: SyncSender<Outgoing>,
}

impl ConnectionHandle {
    /// Queues a packet without waiting. `false` means the client is not
    /// keeping up or is gone; the caller should drop it.
    pub fn send(&self, outgoing: Outgoing) -> bool {
        match self.outbound.try_send(outgoing) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }

    /// Asks the writer to close the connection. A full queue means the
    /// writer is stuck; dropping the last sender then ends it instead.
    pub fn close(&self) {
        let _ = self.outbound.try_send(Outgoing::Close);
    }
}

/// Everything the game loop receives: connection events and finished work
/// from the chunk workers.
#[derive(Debug)]
pub enum Event {
    /// A worker finished generating and encoding a column.
    ChunkReady(Box<super::world::store::ReadyChunk>),
    Joined {
        handle: ConnectionHandle,
        join: Box<JoinRequest>,
    },
    Packet {
        id: ConnectionId,
        packet: Packet,
    },
    Left {
        id: ConnectionId,
        reason: String,
    },
}

/// Starts the reader and writer threads for an accepted socket. `guard` is
/// dropped when the reader ends, which lets the listener count open sockets.
pub fn spawn<G: Send + 'static>(
    stream: TcpStream,
    id: ConnectionId,
    config: Arc<ServerConfig>,
    context: Arc<LoginContext>,
    events: SyncSender<Event>,
    guard: G,
) -> io::Result<()> {
    let peer = stream.peer_addr()?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_POLL))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let write_stream = stream.try_clone()?;
    let (outbound, queue) = sync_channel(config.outbound_queue_packets);
    let handle = ConnectionHandle { id, peer, outbound };
    thread::Builder::new()
        .name(format!("conn-{id}-write"))
        .spawn(move || write_loop(write_stream, queue))?;
    thread::Builder::new()
        .name(format!("conn-{id}-read"))
        .spawn(move || {
            let reason = read_loop(stream, &handle, &config, context, &events)
                .err()
                .unwrap_or_else(|| "closed".to_owned());
            handle.close();
            let _ = events.send(Event::Left { id, reason });
            drop(guard);
        })?;
    Ok(())
}

fn write_loop(mut stream: TcpStream, queue: Receiver<Outgoing>) {
    let mut encoder = FrameEncoder::new();
    let mut buffer = Vec::new();
    while let Ok(outgoing) = queue.recv() {
        buffer.clear();
        let written = match outgoing {
            Outgoing::Packet(packet) => encoder.encode(packet.id, &packet.body, &mut buffer),
            Outgoing::Shared(packet) => encoder.encode(packet.id, &packet.body, &mut buffer),
            Outgoing::EnableCompression(threshold) => {
                encoder.enable_compression(threshold);
                Ok(())
            }
            Outgoing::EnableEncryption(secret) => {
                encoder.enable_encryption(&secret);
                Ok(())
            }
            Outgoing::Close => break,
        };
        if written.is_err() || stream.write_all(&buffer).is_err() {
            break;
        }
    }
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Both);
}

/// Limits how many bytes a client may send per window.
struct RateLimit {
    budget: usize,
    window_start: Instant,
    used: usize,
}

impl RateLimit {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            window_start: Instant::now(),
            used: 0,
        }
    }

    /// Bytes the next read may take; waits for the next window when the
    /// budget is spent, so a flooding client is slowed by TCP backpressure.
    fn allowance(&mut self) -> usize {
        let elapsed = self.window_start.elapsed();
        if elapsed >= RATE_WINDOW {
            self.window_start = Instant::now();
            self.used = 0;
        } else if self.used >= self.budget {
            thread::sleep(RATE_WINDOW - elapsed);
            self.window_start = Instant::now();
            self.used = 0;
        }
        (self.budget - self.used).min(READ_CHUNK)
    }
}

fn read_loop(
    mut stream: TcpStream,
    handle: &ConnectionHandle,
    config: &ServerConfig,
    context: Arc<LoginContext>,
    events: &SyncSender<Event>,
) -> Result<(), String> {
    let mut decoder = FrameDecoder::new(config.max_packet_bytes);
    let mut login = Some(Login::new(context));
    let deadline = Instant::now() + Duration::from_millis(config.login_timeout_ms);
    let mut rate = RateLimit::new(config.read_bytes_per_second);
    let mut buffer = [0u8; READ_CHUNK];
    loop {
        if login.is_some() && Instant::now() > deadline {
            return Err("login timed out".to_owned());
        }
        let allowance = rate.allowance();
        let read = match stream.read(&mut buffer[..allowance]) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error.to_string()),
        };
        rate.used += read;
        decoder.push(&buffer[..read]).map_err(|e| e.to_string())?;
        while let Some(packet) = decoder.next_packet().map_err(|e| e.to_string())? {
            let Some(state) = login.as_mut() else {
                events
                    .send(Event::Packet {
                        id: handle.id,
                        packet,
                    })
                    .map_err(|_| "server stopped".to_owned())?;
                continue;
            };
            for action in state.handle(packet).map_err(|e| e.to_string())? {
                match action {
                    Action::Send(packet) => queue(handle, Outgoing::Packet(packet))?,
                    Action::EnableCompression(threshold) => {
                        decoder.enable_compression(threshold);
                        queue(handle, Outgoing::EnableCompression(threshold))?;
                    }
                    Action::EnableEncryption(secret) => {
                        decoder.enable_encryption(&secret);
                        queue(handle, Outgoing::EnableEncryption(secret))?;
                    }
                    Action::Close => {
                        handle.close();
                        return Ok(());
                    }
                    Action::Join(join) => {
                        login = None;
                        events
                            .send(Event::Joined {
                                handle: handle.clone(),
                                join,
                            })
                            .map_err(|_| "server stopped".to_owned())?;
                    }
                }
            }
        }
    }
}

/// Queues a login packet; during login the queue only fills if the client
/// stops reading, which ends the connection.
fn queue(handle: &ConnectionHandle, outgoing: Outgoing) -> Result<(), String> {
    if handle.send(outgoing) {
        Ok(())
    } else {
        Err("client is not reading".to_owned())
    }
}
