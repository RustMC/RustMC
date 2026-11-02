//! Java Edition keep-alive challenge-response tracking and disconnect framing.
//!
//! Implements latency measurement, timeout detection, and graceful disconnection
//! protocols matching Minecraft Java 26.3 client specifications.
//!
//! # Protocol Specification
//!
//! - **Keep Alive Challenge**: Server periodically emits a 64-bit integer challenge (`KeepAliveId`).
//!   Only one keep-alive challenge may be in-flight at any time per connection.
//! - **Timeout Window**: If a client does not acknowledge the pending challenge within
//!   15 seconds (300 ticks at 20 TPS), the connection is terminated with a timeout error.
//! - **Disconnect Framing**: Disconnect packets carry formatted Chat Component JSON payloads
//!   explaining the termination reason (e.g. timeout, kick, server shutdown).

use std::fmt;

/// Default keep-alive timeout in server ticks (15 seconds = 300 ticks).
pub const DEFAULT_TIMEOUT_TICKS: u64 = 300;

/// Assumed milliseconds per server tick for latency estimation.
pub const MS_PER_TICK: u32 = 50;

/// Errors arising during keep-alive tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepAliveError {
    ChallengeAlreadyPending,
    NoPendingChallenge,
    MismatchedChallenge { expected: i64, received: i64 },
    TimedOut,
}

impl fmt::Display for KeepAliveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChallengeAlreadyPending => {
                write!(f, "a keep-alive challenge is already in-flight")
            }
            Self::NoPendingChallenge => {
                write!(f, "received keep-alive response with no challenge pending")
            }
            Self::MismatchedChallenge { expected, received } => {
                write!(
                    f,
                    "mismatched keep-alive challenge ID: expected {expected}, got {received}"
                )
            }
            Self::TimedOut => write!(f, "keep-alive response timed out"),
        }
    }
}

impl std::error::Error for KeepAliveError {}

/// Tracks in-flight keep-alive challenges and computes connection latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeepAliveTracker {
    pending_challenge: Option<(i64, u64)>, // (challenge_id, sent_tick)
    last_ping_ms: Option<u32>,
    timeout_ticks: u64,
}

impl Default for KeepAliveTracker {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT_TICKS)
    }
}

impl KeepAliveTracker {
    pub const fn new(timeout_ticks: u64) -> Self {
        Self {
            pending_challenge: None,
            last_ping_ms: None,
            timeout_ticks: if timeout_ticks == 0 {
                DEFAULT_TIMEOUT_TICKS
            } else {
                timeout_ticks
            },
        }
    }

    #[inline]
    pub const fn has_pending_challenge(&self) -> bool {
        self.pending_challenge.is_some()
    }

    #[inline]
    pub const fn pending_challenge_id(&self) -> Option<i64> {
        match self.pending_challenge {
            Some((id, _)) => Some(id),
            None => None,
        }
    }

    #[inline]
    pub const fn last_ping_ms(&self) -> Option<u32> {
        self.last_ping_ms
    }

    #[inline]
    pub const fn timeout_ticks(&self) -> u64 {
        self.timeout_ticks
    }

    /// Issues a new keep-alive challenge to be sent to the client.
    ///
    /// Fails with `ChallengeAlreadyPending` if a previous challenge is awaiting response.
    pub fn issue_challenge(
        &mut self,
        current_tick: u64,
        challenge_id: i64,
    ) -> Result<i64, KeepAliveError> {
        if self.pending_challenge.is_some() {
            return Err(KeepAliveError::ChallengeAlreadyPending);
        }
        self.pending_challenge = Some((challenge_id, current_tick));
        Ok(challenge_id)
    }

    /// Processes a client's response to a keep-alive challenge.
    ///
    /// Validates challenge ID, calculates latency, and clears the pending challenge.
    pub fn handle_response(
        &mut self,
        current_tick: u64,
        received_id: i64,
    ) -> Result<u32, KeepAliveError> {
        let (expected_id, sent_tick) = self
            .pending_challenge
            .take()
            .ok_or(KeepAliveError::NoPendingChallenge)?;

        if expected_id != received_id {
            return Err(KeepAliveError::MismatchedChallenge {
                expected: expected_id,
                received: received_id,
            });
        }

        let elapsed_ticks = current_tick.saturating_sub(sent_tick);
        if elapsed_ticks > self.timeout_ticks {
            return Err(KeepAliveError::TimedOut);
        }

        let latency_ms = (elapsed_ticks as u32).saturating_mul(MS_PER_TICK);
        self.last_ping_ms = Some(latency_ms);
        Ok(latency_ms)
    }

    /// Checks whether the in-flight challenge has exceeded the timeout threshold.
    pub fn is_timed_out(&self, current_tick: u64) -> bool {
        match self.pending_challenge {
            Some((_, sent_tick)) => current_tick.saturating_sub(sent_tick) > self.timeout_ticks,
            None => false,
        }
    }

    /// Resets tracker state.
    pub fn reset(&mut self) {
        self.pending_challenge = None;
        self.last_ping_ms = None;
    }
}

/// Disconnect reason packet framing generator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisconnectPayload;

impl DisconnectPayload {
    /// Formats a plain text reason into a canonical JSON Chat Component payload.
    pub fn format_reason_json(reason: &str) -> String {
        // Escape special JSON characters
        let mut escaped = String::with_capacity(reason.len() + 16);
        for c in reason.chars() {
            match c {
                '"' => escaped.push_str("\\\""),
                '\\' => escaped.push_str("\\\\"),
                '\n' => escaped.push_str("\\n"),
                '\r' => escaped.push_str("\\r"),
                '\t' => escaped.push_str("\\t"),
                _ => escaped.push(c),
            }
        }
        format!("{{\"text\":\"{escaped}\"}}")
    }

    /// Encodes a framed disconnect packet body: VarInt length + UTF-8 payload.
    pub fn encode_disconnect_packet(packet_id: u32, reason: &str) -> Vec<u8> {
        let json_reason = Self::format_reason_json(reason);
        let reason_bytes = json_reason.as_bytes();

        let mut payload = Vec::with_capacity(reason_bytes.len() + 10);
        put_varint(packet_id, &mut payload);
        put_varint(reason_bytes.len() as u32, &mut payload);
        payload.extend_from_slice(reason_bytes);

        let mut framed = Vec::with_capacity(payload.len() + 5);
        put_varint(payload.len() as u32, &mut framed);
        framed.extend_from_slice(&payload);
        framed
    }
}

/// Writes a VarInt to a byte buffer.
fn put_varint(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}
