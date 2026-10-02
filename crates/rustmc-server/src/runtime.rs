//! Bounded loopback discovery supervisor with an opt-in local Java terrain preview.

use crate::{ListenerConfig, discovery_bedrock, discovery_java, java_preview, preview_data};
use std::{
    fmt,
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const ACCEPT_BUDGET: usize = 32;
const MAX_PENDING_BYTES: usize = 1024 * 1024;

/// Observable development-process states. `Bound` is not protocol or world readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Configuring,
    Starting,
    Bound,
    Stopping,
    Stopped,
    Failed,
}

impl fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Configuring => "configuring",
            Self::Starting => "starting",
            Self::Bound => "bound",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        };
        f.write_str(label)
    }
}

/// One structured lifecycle or connection event, with no payload or secret fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeEvent {
    pub kind: &'static str,
    pub state: LifecycleState,
    pub elapsed_us: u128,
    pub elapsed_ms: u128,
    pub address: Option<SocketAddr>,
    pub active_connections: Option<usize>,
    pub reason: Option<&'static str>,
}

impl RuntimeEvent {
    /// Render stable key-value fields suitable for line-oriented logs.
    pub fn log_line(self) -> String {
        let mut line = format!(
            "event={} state={} elapsed_us={} elapsed_ms={}",
            self.kind, self.state, self.elapsed_us, self.elapsed_ms
        );
        if let Some(address) = self.address {
            line.push_str(&format!(" listen_addr={address}"));
        }
        if let Some(active) = self.active_connections {
            line.push_str(&format!(" active_connections={active}"));
        }
        if let Some(reason) = self.reason {
            line.push_str(&format!(" reason={reason}"));
        }
        if self.kind == "listener_bound" {
            line.push_str(" protocol_ready=false world_ready=false");
        }
        if self.kind == "discovery_bound" {
            line.push_str(" java_status_ready=true bedrock_discovery_ready=true login_ready=false world_ready=false");
        }
        line
    }
}

/// A listener startup or supervisor failure.
#[derive(Debug)]
pub enum RuntimeError {
    Bind(io::Error),
    Io(io::Error),
}

impl RuntimeError {
    /// Process exit code: 4 for startup/bind, 5 for a running listener failure.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Bind(_) => 4,
            Self::Io(_) => 5,
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind(error) => write!(f, "could not start loopback listener: {error}"),
            Self::Io(error) => write!(f, "development listener failed: {error}"),
        }
    }
}

struct Connection {
    stream: TcpStream,
    discovery: discovery_java::Session,
    pending: Vec<u8>,
    bytes_read: usize,
    last_activity: Instant,
    created: Instant,
    pending_chunk_since: Option<Instant>,
}

fn event(
    kind: &'static str,
    state: LifecycleState,
    started: Instant,
    address: Option<SocketAddr>,
    active_connections: Option<usize>,
    reason: Option<&'static str>,
) -> RuntimeEvent {
    let elapsed_us = started.elapsed().as_micros();
    RuntimeEvent {
        kind,
        state,
        elapsed_us,
        elapsed_ms: elapsed_us / 1000,
        address,
        active_connections,
        reason,
    }
}

fn stop<F: FnMut(RuntimeEvent)>(
    listener: Option<TcpListener>,
    mut connections: Vec<Connection>,
    started: Instant,
    observe: &mut F,
) {
    observe(event(
        "stopping",
        LifecycleState::Stopping,
        started,
        None,
        Some(connections.len()),
        None,
    ));
    for connection in connections.drain(..) {
        let _ = connection.stream.shutdown(Shutdown::Both);
    }
    drop(listener);
    observe(event(
        "stopped",
        LifecycleState::Stopped,
        started,
        None,
        Some(0),
        None,
    ));
}

/// Run a bounded local development listener until shutdown is requested.
///
/// The observer receives events in process order; no event announces protocol or world readiness.
/// A pre-set shutdown flag stops before binding, allowing deterministic startup cancellation tests.
pub fn run_listener<F: FnMut(RuntimeEvent)>(
    config: &ListenerConfig,
    shutdown: &AtomicBool,
    started: Instant,
    mut observe: F,
) -> Result<(), RuntimeError> {
    observe(event(
        "starting",
        LifecycleState::Starting,
        started,
        None,
        None,
        None,
    ));
    if shutdown.load(Ordering::SeqCst) {
        stop(None, Vec::new(), started, &mut observe);
        return Ok(());
    }
    if !(2..=32).contains(&config.preview_view_distance)
        || (config.preview_registry_manifest.is_some() && !config.local_java_preview)
        || config.max_connections == 0
        || config.max_connections > 64
        || config.max_bytes_per_connection == 0
        || config.max_bytes_per_connection > 65536
        || !(10..=60000).contains(&config.idle_timeout_ms)
        || !(10..=60000).contains(&config.max_connection_lifetime_ms)
        || config.idle_timeout_ms > config.max_connection_lifetime_ms
    {
        observe(event(
            "failed",
            LifecycleState::Failed,
            started,
            None,
            None,
            Some("invalid_limits"),
        ));
        return Err(RuntimeError::Bind(io::Error::new(
            io::ErrorKind::InvalidInput,
            "development listener limits are invalid",
        )));
    }
    if !config.bind_address.is_loopback() {
        observe(event(
            "failed",
            LifecycleState::Failed,
            started,
            None,
            None,
            Some("non_loopback"),
        ));
        return Err(RuntimeError::Bind(io::Error::new(
            io::ErrorKind::InvalidInput,
            "M1 requires a loopback bind address",
        )));
    }
    if config.vanilla_data_root.is_some() != config.vanilla_registry_table.is_some()
        || (config.vanilla_data_root.is_some()
            && (!config.local_java_preview || config.max_connections != 1))
    {
        return Err(RuntimeError::Bind(io::Error::new(
            io::ErrorKind::InvalidInput,
            "local vanilla preview requires both data paths and max_connections = 1",
        )));
    }
    let registry_manifest = match &config.preview_registry_manifest {
        Some(path) => match preview_data::load(path) {
            Ok(manifest) => Some(Arc::new(manifest)),
            Err(message) => {
                observe(event(
                    "failed",
                    LifecycleState::Failed,
                    started,
                    None,
                    None,
                    Some("invalid_preview_manifest"),
                ));
                return Err(RuntimeError::Bind(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    message,
                )));
            }
        },
        None => None,
    };
    let vanilla_source = if let (Some(data_root), Some(registry_table)) =
        (&config.vanilla_data_root, &config.vanilla_registry_table)
    {
        let seed = i64::try_from(config.preview_seed).map_err(|_| {
            RuntimeError::Bind(io::Error::new(
                io::ErrorKind::InvalidInput,
                "preview seed exceeds i64",
            ))
        })?;
        let generator = crate::vanilla::generator::VanillaGenerator::new(
            data_root,
            seed,
            "minecraft:overworld",
        )
        .map_err(|error| {
            RuntimeError::Bind(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid operator-local worldgen data: {error}"),
            ))
        })?;
        let table_text = std::fs::read_to_string(registry_table).map_err(RuntimeError::Bind)?;
        let _tables = crate::chunk_adapter::registry::RegistryTables::from_provisioned(&table_text)
            .map_err(|error| {
                RuntimeError::Bind(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid local 26.3 registry table: {error}"),
                ))
            })?;
        Some(java_preview::VanillaSource {
            data_root: data_root.clone(),
            registry_table: registry_table.clone(),
            seed,
            spawn_y: generator.surface_height(0, 0),
        })
    } else {
        None
    };
    let bind_address = SocketAddr::new(config.bind_address, config.port);
    let listener = match TcpListener::bind(bind_address) {
        Ok(listener) => listener,
        Err(error) => {
            observe(event(
                "failed",
                LifecycleState::Failed,
                started,
                None,
                None,
                Some("bind_failed"),
            ));
            return Err(RuntimeError::Bind(error));
        }
    };
    if let Err(error) = listener.set_nonblocking(true) {
        observe(event(
            "failed",
            LifecycleState::Failed,
            started,
            None,
            None,
            Some("nonblocking_failed"),
        ));
        return Err(RuntimeError::Bind(error));
    }
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => {
            observe(event(
                "failed",
                LifecycleState::Failed,
                started,
                None,
                None,
                Some("local_address_failed"),
            ));
            return Err(RuntimeError::Bind(error));
        }
    };
    let udp = match UdpSocket::bind(address) {
        Ok(socket) => socket,
        Err(error) => {
            observe(event(
                "failed",
                LifecycleState::Failed,
                started,
                None,
                None,
                Some("udp_bind_failed"),
            ));
            return Err(RuntimeError::Bind(error));
        }
    };
    if let Err(error) = udp.set_nonblocking(true) {
        observe(event(
            "failed",
            LifecycleState::Failed,
            started,
            None,
            None,
            Some("udp_nonblocking_failed"),
        ));
        return Err(RuntimeError::Bind(error));
    }
    observe(event(
        "listener_bound",
        LifecycleState::Bound,
        started,
        Some(address),
        Some(0),
        None,
    ));
    observe(event(
        "discovery_bound",
        LifecycleState::Bound,
        started,
        Some(address),
        Some(0),
        None,
    ));
    let mut connections: Vec<Connection> = Vec::with_capacity(config.max_connections);
    while !shutdown.load(Ordering::SeqCst) {
        for _ in 0..ACCEPT_BUDGET {
            let mut buffer = [0u8; discovery_bedrock::MAX_DATAGRAM + 1];
            match udp.recv_from(&mut buffer) {
                Ok((length, peer)) => {
                    if let Some(reply) =
                        discovery_bedrock::pong(&buffer[..length], 0x525553544d43, address.port())
                    {
                        let _ = udp.send_to(&reply, peer);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    observe(event(
                        "failed",
                        LifecycleState::Failed,
                        started,
                        None,
                        Some(connections.len()),
                        Some("udp_receive_failed"),
                    ));
                    return Err(RuntimeError::Io(error));
                }
            }
        }
        for _ in 0..ACCEPT_BUDGET {
            match listener.accept() {
                Ok((stream, _peer)) => {
                    if connections.len() >= config.max_connections {
                        let _ = stream.shutdown(Shutdown::Both);
                        observe(event(
                            "connection_rejected",
                            LifecycleState::Bound,
                            started,
                            None,
                            Some(connections.len()),
                            Some("capacity"),
                        ));
                        continue;
                    }
                    if stream.set_nonblocking(true).is_err() {
                        let _ = stream.shutdown(Shutdown::Both);
                        observe(event(
                            "connection_closed",
                            LifecycleState::Bound,
                            started,
                            None,
                            Some(connections.len()),
                            Some("socket_setup"),
                        ));
                        continue;
                    }
                    let mut discovery = discovery_java::Session::new(config.local_java_preview)
                        .with_world(
                            config.preview_seed,
                            config.preview_view_distance,
                            config.preview_terrain,
                        );
                    if let Some(source) = &vanilla_source {
                        discovery = discovery.with_vanilla_source(source.clone());
                    }
                    if let Some(manifest) = &registry_manifest {
                        discovery = discovery.with_registry_manifest(Arc::clone(manifest));
                    }
                    connections.push(Connection {
                        stream,
                        discovery,
                        pending: Vec::new(),
                        bytes_read: 0,
                        last_activity: Instant::now(),
                        created: Instant::now(),
                        pending_chunk_since: None,
                    });
                    observe(event(
                        "connection_accepted",
                        LifecycleState::Bound,
                        started,
                        None,
                        Some(connections.len()),
                        None,
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    observe(event(
                        "failed",
                        LifecycleState::Failed,
                        started,
                        None,
                        Some(connections.len()),
                        Some("accept_failed"),
                    ));
                    for connection in connections {
                        let _ = connection.stream.shutdown(Shutdown::Both);
                    }
                    return Err(RuntimeError::Io(error));
                }
            }
        }
        let mut index = 0;
        while index < connections.len() {
            let connection = &mut connections[index];
            let playing = connection.discovery.state() == discovery_java::State::Play;
            let byte_limit = if playing {
                1024 * 1024
            } else {
                config.max_bytes_per_connection
            };
            let lifetime_ms = if playing {
                600_000
            } else {
                config.max_connection_lifetime_ms
            };
            let remaining = byte_limit.saturating_sub(connection.bytes_read);
            let mut buffer = [0u8; 4096];
            let read_len = remaining.min(buffer.len());
            let reason = if connection.created.elapsed() >= Duration::from_millis(lifetime_ms) {
                Some("lifetime_timeout")
            } else {
                match connection.stream.read(&mut buffer[..read_len]) {
                    Ok(0) => Some("peer_closed"),
                    Ok(count) => {
                        connection.bytes_read += count;
                        connection.last_activity = Instant::now();
                        let state_before = connection.discovery.state();
                        match connection.discovery.receive(&buffer[..count], byte_limit) {
                            Ok(replies) => {
                                let state_after = connection.discovery.state();
                                if state_before != state_after {
                                    let milestone = match state_after {
                                        discovery_java::State::Configuration => {
                                            Some("java_configuration_started")
                                        }
                                        discovery_java::State::ConfigurationData => {
                                            Some("java_known_pack_acknowledged")
                                        }
                                        discovery_java::State::Play => {
                                            Some("java_configuration_acknowledged")
                                        }
                                        _ => None,
                                    };
                                    if let Some(kind) = milestone {
                                        observe(event(
                                            kind,
                                            LifecycleState::Bound,
                                            started,
                                            None,
                                            None,
                                            None,
                                        ));
                                    }
                                }
                                for reply in replies {
                                    connection.pending.extend(reply);
                                }
                                if connection.pending.len() > MAX_PENDING_BYTES {
                                    Some("write_limit")
                                } else if connection.bytes_read >= byte_limit {
                                    Some("read_limit")
                                } else {
                                    None
                                }
                            }
                            Err(_) if connection.bytes_read >= byte_limit => Some("read_limit"),
                            Err(_)
                                if matches!(
                                    connection.discovery.state(),
                                    discovery_java::State::Configuration
                                        | discovery_java::State::ConfigurationData
                                        | discovery_java::State::Play
                                ) =>
                            {
                                Some("unsupported_java_configuration_or_play")
                            }
                            Err(_) => Some("invalid_java_packet"),
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        (connection.last_activity.elapsed()
                            >= Duration::from_millis(config.idle_timeout_ms))
                        .then_some("idle_timeout")
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => None,
                    Err(_) => Some("read_error"),
                }
            };
            if reason.is_none()
                && connection.pending.is_empty()
                && let Some(preview) = &mut connection.discovery.preview
            {
                if let Some(packet) = preview.keepalive() {
                    connection.pending.extend(packet);
                }
                if let Some((packet, generation_us, encoding_us)) = preview.next_chunk() {
                    connection.pending_chunk_since = Some(Instant::now());
                    eprintln!(
                        "event=preview_chunk_encoded generation_us={generation_us} encoding_us={encoding_us} bytes={}",
                        packet.len()
                    );
                    connection.pending.extend(packet);
                }
            }
            let reason = reason.or_else(|| {
                connection
                    .discovery
                    .preview
                    .as_ref()
                    .and_then(|preview| preview.failed.then_some("vanilla_chunk_failure"))
            });
            let reason = if reason.is_none() && !connection.pending.is_empty() {
                match connection.stream.write(&connection.pending) {
                    Ok(0) => Some("write_closed"),
                    Ok(n) => {
                        connection.pending.drain(..n);
                        if connection.pending.is_empty()
                            && let Some(since) = connection.pending_chunk_since.take()
                        {
                            eprintln!(
                                "event=preview_chunk_socket_flushed queue_and_write_us={}",
                                since.elapsed().as_micros()
                            );
                        }
                        None
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => None,
                    Err(_) => Some("write_error"),
                }
            } else {
                reason
            };
            let reason = reason.or_else(|| {
                (connection.discovery.state() == discovery_java::State::Done
                    && connection.pending.is_empty())
                .then_some("status_complete")
            });
            if let Some(reason) = reason {
                let connection = connections.swap_remove(index);
                let _ = connection.stream.shutdown(Shutdown::Both);
                observe(event(
                    "connection_closed",
                    LifecycleState::Bound,
                    started,
                    None,
                    Some(connections.len()),
                    Some(reason),
                ));
            } else {
                index += 1;
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
    stop(Some(listener), connections, started, &mut observe);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prebind_shutdown_never_opens_socket() {
        let flag = AtomicBool::new(true);
        let mut events = Vec::new();
        run_listener(&ListenerConfig::default(), &flag, Instant::now(), |event| {
            events.push(event)
        })
        .unwrap();
        assert_eq!(
            events.iter().map(|e| e.kind).collect::<Vec<_>>(),
            ["starting", "stopping", "stopped"]
        );
    }

    #[test]
    fn direct_invalid_limits_fail_before_binding() {
        let config = ListenerConfig {
            max_connections: usize::MAX,
            ..ListenerConfig::default()
        };
        let flag = AtomicBool::new(false);
        let mut events = Vec::new();
        let error =
            run_listener(&config, &flag, Instant::now(), |event| events.push(event)).unwrap_err();
        assert_eq!(error.exit_code(), 4);
        assert!(
            events
                .iter()
                .any(|event| event.reason == Some("invalid_limits"))
        );
    }

    #[test]
    fn runtime_failure_has_distinct_exit_code() {
        let error = RuntimeError::Io(io::Error::other("test failure"));
        assert_eq!(error.exit_code(), 5);
    }

    #[test]
    fn occupied_port_fails_with_startup_code() {
        let occupied = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let config = ListenerConfig {
            port: occupied.local_addr().unwrap().port(),
            ..ListenerConfig::default()
        };
        let flag = AtomicBool::new(false);
        let mut events = Vec::new();
        let error =
            run_listener(&config, &flag, Instant::now(), |event| events.push(event)).unwrap_err();
        assert_eq!(error.exit_code(), 4);
        assert!(
            events
                .iter()
                .any(|event| event.state == LifecycleState::Failed)
        );
    }
}
