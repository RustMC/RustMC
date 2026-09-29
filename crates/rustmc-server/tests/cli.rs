#![forbid(unsafe_code)]

use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustmc-test-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, contents: &str) -> PathBuf {
        let path = self.0.join("config.toml");
        fs::write(&path, contents).unwrap();
        path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rustmc-server"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_version_and_bootstrap_only() {
    let help = run(&["--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&help.stdout).contains("--check-config <path>"));
    let version = run(&["--version"]);
    assert_eq!(version.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&version.stdout).contains(env!("CARGO_PKG_VERSION")));
    let default = run(&[]);
    assert_eq!(default.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&default.stdout).contains("no listener or world started"));
}

#[test]
fn valid_and_invalid_configurations() {
    let dir = TempDir::new();
    let cases = [
        ("schema_version = 1\nlog_level = 'info'", 0, "valid"),
        ("schema_version = 1\nlog_level =", 3, "malformed TOML"),
        (
            "schema_version = 1\nlog_level = 'info'\nextra = 4",
            3,
            "unknown",
        ),
        ("schema_version = 9\nlog_level = 'info'", 3, "unsupported"),
        (
            "schema_version = 1\nlog_level = 'private-token'",
            3,
            "invalid",
        ),
        ("log_level = 'info'", 3, "missing"),
        (
            "schema_version = 1\nlog_level = 'info'\n[listener]\nbind_address = '0.0.0.0'",
            3,
            "loopback",
        ),
        (
            "schema_version = 1\nlog_level = 'info'\n[listener]\nport = 70000",
            3,
            "listener.port",
        ),
        (
            "schema_version = 1\nlog_level = 'info'\n[listener]\nunknown = 1",
            3,
            "unknown",
        ),
        (
            "schema_version = 1\nlog_level = 'info'\n[listener]\nidle_timeout_ms = 200\nmax_connection_lifetime_ms = 100",
            3,
            "cannot exceed",
        ),
    ];
    for (contents, code, expected) in cases {
        let path = dir.file(contents);
        let output = run(&["--check-config", path.to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(code), "{contents}");
        let message = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(message.contains(expected), "{message}");
        assert!(!message.contains("private-token"));
        assert!(!message.contains("panicked"));
        assert_eq!(fs::read_to_string(path).unwrap(), contents);
    }
    let missing = dir.0.join("missing.toml");
    let output = run(&["--check-config", missing.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not read"));
}

#[test]
fn usage_errors() {
    for args in [
        vec!["--check-config"],
        vec!["--bad"],
        vec!["--help", "extra"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("usage error"));
    }
}

#[cfg(unix)]
mod runtime_cli {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::{Ipv4Addr, TcpListener, TcpStream, UdpSocket},
        process::{Child, Stdio},
        sync::mpsc::{self, Receiver},
        thread,
        time::Duration,
    };

    struct Running {
        child: Child,
        lines: Receiver<String>,
        reader: Option<thread::JoinHandle<()>>,
    }

    impl Running {
        fn start(path: &std::path::Path) -> Self {
            let mut child = Command::new(env!("CARGO_BIN_EXE_rustmc-server"))
                .args(["--run", path.to_str().unwrap()])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let (tx, lines) = mpsc::channel();
            let reader = thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    if tx.send(line.unwrap()).is_err() {
                        break;
                    }
                }
            });
            Self {
                child,
                lines,
                reader: Some(reader),
            }
        }

        fn wait_for(&self, text: &str) -> String {
            loop {
                let line = self.lines.recv_timeout(Duration::from_secs(3)).unwrap();
                if line.contains(text) {
                    return line;
                }
            }
        }

        fn stop(self) {
            self.stop_with_signal("-TERM");
        }

        fn stop_with_signal(mut self, signal: &str) {
            let status = Command::new("kill")
                .args([signal, &self.child.id().to_string()])
                .status()
                .unwrap();
            assert!(status.success());
            self.wait_for("event=stopping");
            self.wait_for("event=stopped");
            assert_eq!(self.child.wait().unwrap().code(), Some(0));
            self.reader.take().unwrap().join().unwrap();
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    fn config(port: u16) -> String {
        format!(
            "schema_version = 1\nlog_level = 'debug'\n[listener]\nbind_address = '127.0.0.1'\nport = {port}\nmax_connections = 1\nmax_bytes_per_connection = 4\nidle_timeout_ms = 500\nmax_connection_lifetime_ms = 2000\n"
        )
    }

    #[test]
    fn java_and_bedrock_discovery_share_loopback_port_and_stop() {
        let dir = TempDir::new();
        let path = dir.file(&config(0).replace(
            "max_bytes_per_connection = 4",
            "max_bytes_per_connection = 4096",
        ));
        let server = Running::start(&path);
        let bound = server.wait_for("event=listener_bound");
        let ready = server.wait_for("event=discovery_bound");
        assert!(ready.contains("java_status_ready=true"));
        assert!(ready.contains("bedrock_discovery_ready=true"));
        assert!(ready.contains("login_ready=false"));
        let address = bound
            .split_whitespace()
            .find_map(|s| s.strip_prefix("listen_addr="))
            .unwrap()
            .parse::<std::net::SocketAddr>()
            .unwrap();
        assert!(bound.contains("protocol_ready=false"));
        let mut java = TcpStream::connect(address).unwrap();
        java.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        // Handshake: packet id 0, protocol 777, host "x", port 25565, status state 1.
        java.write_all(&[8, 0, 0x89, 0x06, 1, b'x', 0x63, 0xdd, 1, 1, 0])
            .unwrap();
        let mut buf = [0u8; 512];
        let n = java.read(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("\"protocol\":777"));
        java.write_all(&[9, 1, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(java.read(&mut buf).unwrap(), 10);
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut ping = vec![1];
        ping.extend([7u8; 8]);
        ping.extend([
            0, 255, 255, 0, 254, 254, 254, 254, 253, 253, 253, 253, 18, 52, 86, 120,
        ]);
        ping.extend([0u8; 8]);
        udp.send_to(&ping, address).unwrap();
        let n = udp.recv(&mut buf).unwrap();
        assert_eq!(buf[0], 0x1c);
        assert!(String::from_utf8_lossy(&buf[..n]).contains(";2193;1.26.51;"));
        let mut bad_ping = ping.clone();
        bad_ping[13] = 0;
        udp.send_to(&bad_ping, address).unwrap();
        udp.set_read_timeout(Some(Duration::from_millis(150)))
            .unwrap();
        assert!(udp.recv(&mut buf).is_err());
        udp.send_to(&vec![0u8; 513], address).unwrap();
        assert!(udp.recv(&mut buf).is_err());
        server.stop();
        let rebound = UdpSocket::bind(address).unwrap();
        drop(rebound);
    }

    #[test]
    fn malformed_java_frame_is_closed_without_response() {
        let dir = TempDir::new();
        let path = dir.file(&config(0).replace(
            "max_bytes_per_connection = 4",
            "max_bytes_per_connection = 4096",
        ));
        let server = Running::start(&path);
        let bound = server.wait_for("event=listener_bound");
        let address = bound
            .split_whitespace()
            .find_map(|s| s.strip_prefix("listen_addr="))
            .unwrap()
            .parse::<std::net::SocketAddr>()
            .unwrap();
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket.write_all(&[0x81, 0x20]).unwrap(); // 4097-byte frame exceeds the codec cap.
        assert_eq!(socket.read(&mut [0u8; 1]).unwrap(), 0);
        server.stop();
    }

    #[test]
    fn local_java_preview_requires_opt_in_and_advances_to_configuration() {
        let dir = TempDir::new();
        let path = dir.file(&config(0).replace(
            "max_bytes_per_connection = 4",
            "max_bytes_per_connection = 4096\nlocal_java_preview = true",
        ));
        let server = Running::start(&path);
        let bound = server.wait_for("event=listener_bound");
        let address = bound
            .split_whitespace()
            .find_map(|s| s.strip_prefix("listen_addr="))
            .unwrap()
            .parse::<std::net::SocketAddr>()
            .unwrap();
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        // The independently constructed handshake switches to login state 2.
        socket
            .write_all(&[8, 0, 0x89, 0x06, 1, b'x', 0x63, 0xdd, 2])
            .unwrap();
        let mut hello = vec![22, 0, 4];
        hello.extend_from_slice(b"Test");
        hello.extend_from_slice(&[7; 16]);
        socket.write_all(&hello).unwrap();
        let mut reply = [0u8; 64];
        let count = socket.read(&mut reply).unwrap();
        assert_eq!(reply[0], 39); // 26.3 also carries a connection-session UUID.
        assert_eq!(reply[1], 2);
        assert_eq!(&reply[2..18], &[7; 16]);
        assert_eq!(&reply[19..23], b"Test");
        assert_eq!(reply[30] & 0xf0, 0x40); // RFC 9562 version 4.
        assert_eq!(reply[32] & 0xc0, 0x80); // RFC variant.
        assert_eq!(count, 40);
        socket.write_all(&[1, 3]).unwrap(); // Login Acknowledged.
        let count = socket.read(&mut reply).unwrap();
        assert_eq!(count, 23);
        assert_eq!(&reply[..3], &[22, 15, 1]); // One known pack.
        assert!(String::from_utf8_lossy(&reply[..count]).contains("minecraft"));
        socket.write_all(&[1, 127]).unwrap(); // Unsupported configuration packet.
        assert_eq!(socket.read(&mut reply).unwrap(), 0);
        server.stop();
    }

    #[test]
    fn binds_limits_times_out_and_stops_cleanly() {
        let dir = TempDir::new();
        let path = dir.file(&config(0));
        let server = Running::start(&path);
        server.wait_for("event=process_started state=configuring");
        server.wait_for("event=config_validated state=configuring");
        server.wait_for("event=starting state=starting");
        let bound = server.wait_for("event=listener_bound state=bound");
        assert!(bound.contains("elapsed_ms="));
        assert!(bound.contains("protocol_ready=false"));
        assert!(bound.contains("world_ready=false"));
        let address = bound
            .split_whitespace()
            .find_map(|field| field.strip_prefix("listen_addr="))
            .unwrap()
            .parse::<std::net::SocketAddr>()
            .unwrap();
        assert!(address.ip().is_loopback());
        assert_ne!(address.port(), 0);

        let mut first = TcpStream::connect(address).unwrap();
        first
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        server.wait_for("event=connection_accepted");
        let mut excess = TcpStream::connect(address).unwrap();
        excess
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        server.wait_for("event=connection_rejected");
        let mut byte = [0u8; 1];
        assert_eq!(excess.read(&mut byte).unwrap(), 0);

        first.write_all(b"test").unwrap();
        server.wait_for("reason=read_limit");
        assert_eq!(first.read(&mut byte).unwrap(), 0);

        let mut idle = TcpStream::connect(address).unwrap();
        idle.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        server.wait_for("event=connection_accepted");
        server.wait_for("reason=idle_timeout");
        assert_eq!(idle.read(&mut byte).unwrap(), 0);

        let mut active = TcpStream::connect(address).unwrap();
        active
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        server.wait_for("event=connection_accepted");
        server.stop();
        assert_eq!(active.read(&mut byte).unwrap(), 0);
        let rebound = TcpListener::bind(address).unwrap();
        drop(rebound);
    }

    #[test]
    fn sigint_stops_cleanly() {
        let dir = TempDir::new();
        let path = dir.file(&config(0));
        let server = Running::start(&path);
        server.wait_for("event=listener_bound");
        server.stop_with_signal("-INT");
    }

    #[test]
    fn total_lifetime_closes_idle_socket() {
        let dir = TempDir::new();
        let contents = config(0)
            .replace("idle_timeout_ms = 500", "idle_timeout_ms = 100")
            .replace(
                "max_connection_lifetime_ms = 2000",
                "max_connection_lifetime_ms = 100",
            );
        let path = dir.file(&contents);
        let server = Running::start(&path);
        let bound = server.wait_for("event=listener_bound");
        let address = bound
            .split_whitespace()
            .find_map(|field| field.strip_prefix("listen_addr="))
            .unwrap()
            .parse::<std::net::SocketAddr>()
            .unwrap();
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        server.wait_for("event=connection_accepted");
        server.wait_for("reason=lifetime_timeout");
        assert_eq!(socket.read(&mut [0u8; 1]).unwrap(), 0);
        server.stop();
    }

    #[test]
    fn occupied_port_is_a_startup_error() {
        let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let dir = TempDir::new();
        let path = dir.file(&config(occupied.local_addr().unwrap().port()));
        let output = run(&["--run", path.to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(4));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("event=failed state=failed"));
        assert!(!stdout.contains("event=listener_bound"));
    }
}
