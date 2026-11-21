//! Unit tests for `net::session_server`, including a local HTTP stand-in for
//! the session server so no test reaches the internet.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

use super::*;

#[test]
fn parses_a_profile_with_properties() {
    let body = r#"{"id":"b50ad385829d3141a2167e7d7539ba7f","name":"Notch",
        "properties":[{"name":"textures","value":"dmFsdWU=","signature":"c2ln"}]}"#;
    let profile = parse_profile(body, "notch").expect("profile");
    assert_eq!(profile.name, "Notch");
    assert_eq!(profile.uuid[0], 0xb5);
    assert_eq!(profile.properties.len(), 1);
    assert_eq!(profile.properties[0].signature.as_deref(), Some("c2ln"));
}

#[test]
fn rejects_profiles_for_other_players_or_malformed_bodies() {
    let other = r#"{"id":"b50ad385829d3141a2167e7d7539ba7f","name":"jeb_"}"#;
    assert_eq!(parse_profile(other, "Notch"), Err(VerifyError::BadResponse));
    assert_eq!(parse_profile("{}", "Notch"), Err(VerifyError::BadResponse));
    assert_eq!(
        parse_profile("not json", "Notch"),
        Err(VerifyError::BadResponse)
    );
    let bad_id = r#"{"id":"xyz","name":"Notch"}"#;
    assert_eq!(
        parse_profile(bad_id, "Notch"),
        Err(VerifyError::BadResponse)
    );
}

/// Serves one HTTP response and reports the request line it received.
fn serve_once(status: &'static str, body: &'static str) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).expect("request");
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("header") <= 2 {
                break;
            }
        }
        let mut stream = reader.into_inner();
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("respond");
        tx.send(request_line).expect("report");
    });
    (base, rx)
}

#[test]
fn has_joined_sends_the_documented_query_and_reads_the_profile() {
    let (base, request) = serve_once(
        "200 OK",
        r#"{"id":"b50ad385829d3141a2167e7d7539ba7f","name":"Notch","properties":[]}"#,
    );
    let profile = HttpSessionServer::new(&base)
        .has_joined("Notch", "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1")
        .expect("reachable")
        .expect("joined");
    assert_eq!(profile.name, "Notch");
    let line = request.recv().expect("request line");
    assert!(
        line.starts_with(
            "GET /session/minecraft/hasJoined?username=Notch&serverId=-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1 "
        ),
        "{line}"
    );
}

#[test]
fn no_content_means_not_joined_and_errors_are_unavailable() {
    let (base, _request) = serve_once("204 No Content", "");
    assert_eq!(
        HttpSessionServer::new(&base).has_joined("Notch", "00"),
        Ok(None)
    );
    let (base, _request) = serve_once("500 Internal Server Error", "");
    assert!(matches!(
        HttpSessionServer::new(&base).has_joined("Notch", "00"),
        Err(VerifyError::Unavailable(_))
    ));
}
