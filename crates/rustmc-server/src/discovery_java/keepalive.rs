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
//! - **Disconnect Framing**: Login disconnects carry a JSON text component; configuration
//!   and play disconnects carry a network NBT text component (26.3 protocol 777).

use std::fmt;

use super::put_varint;

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

/// Connection phase whose disconnect packet carries a reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectPhase {
    Login,
    Configuration,
    Play,
}

impl DisconnectPhase {
    /// Clientbound disconnect packet ID in the official 26.3 packet report:
    /// `login_disconnect` (login 0), `disconnect` (configuration 2, play 32).
    pub const fn packet_id(self) -> u32 {
        match self {
            Self::Login => 0,
            Self::Configuration => 2,
            Self::Play => 32,
        }
    }
}

/// NBT tag type of a string; a plain text component is a bare string tag.
const TAG_STRING: u8 = 8;
/// Largest modified UTF-8 payload an NBT string length prefix can describe.
const MAX_NBT_STRING_BYTES: usize = u16::MAX as usize;

/// Errors from encoding a disconnect reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectError {
    /// The reason exceeds the 65,535-byte modified UTF-8 limit of an NBT string.
    ReasonTooLong,
}

/// Disconnect reason packet encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisconnectPayload;

impl DisconnectPayload {
    /// Formats a plain text reason as a JSON text component, escaping every
    /// character JSON requires (quotes, backslash, and all control characters).
    pub fn format_reason_json(reason: &str) -> String {
        let mut escaped = String::with_capacity(reason.len() + 16);
        for c in reason.chars() {
            match c {
                '"' => escaped.push_str("\\\""),
                '\\' => escaped.push_str("\\\\"),
                '\n' => escaped.push_str("\\n"),
                '\r' => escaped.push_str("\\r"),
                '\t' => escaped.push_str("\\t"),
                c if c < ' ' => escaped.push_str(&format!("\\u{:04x}", u32::from(c))),
                _ => escaped.push(c),
            }
        }
        format!("{{\"text\":\"{escaped}\"}}")
    }

    /// Encodes a framed disconnect packet for `phase`.
    ///
    /// Login carries the reason as a JSON text component string. Configuration
    /// and play carry an anonymous network NBT text component; a plain text
    /// component is a single string tag in modified UTF-8.
    pub fn encode(phase: DisconnectPhase, reason: &str) -> Result<Vec<u8>, DisconnectError> {
        let mut payload = Vec::with_capacity(reason.len() + 8);
        put_varint(phase.packet_id(), &mut payload);
        match phase {
            DisconnectPhase::Login => {
                let json = Self::format_reason_json(reason);
                put_varint(json.len() as u32, &mut payload);
                payload.extend_from_slice(json.as_bytes());
            }
            DisconnectPhase::Configuration | DisconnectPhase::Play => {
                let text = modified_utf8(reason);
                if text.len() > MAX_NBT_STRING_BYTES {
                    return Err(DisconnectError::ReasonTooLong);
                }
                payload.push(TAG_STRING);
                payload.extend_from_slice(&(text.len() as u16).to_be_bytes());
                payload.extend_from_slice(&text);
            }
        }
        let mut framed = Vec::with_capacity(payload.len() + 5);
        put_varint(payload.len() as u32, &mut framed);
        framed.extend_from_slice(&payload);
        Ok(framed)
    }
}

/// Java's modified UTF-8: U+0000 becomes `C0 80` and supplementary characters
/// are written as two three-byte surrogate encodings.
fn modified_utf8(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for unit in text.encode_utf16() {
        match unit {
            0x0001..=0x007f => out.push(unit as u8),
            0x0000 | 0x0080..=0x07ff => {
                out.push(0xc0 | (unit >> 6) as u8);
                out.push(0x80 | (unit & 0x3f) as u8);
            }
            _ => {
                out.push(0xe0 | (unit >> 12) as u8);
                out.push(0x80 | ((unit >> 6) & 0x3f) as u8);
                out.push(0x80 | (unit & 0x3f) as u8);
            }
        }
    }
    out
}
