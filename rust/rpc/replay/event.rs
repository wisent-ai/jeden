//! What the replay journal stores and how a reader names its place in it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventV1 {
    pub session_id: String,
    pub stream_id: String,
    pub sequence: u64,
    pub event_id: String,
    pub request_id: String,
    pub kind: String,
    pub payload: Value,
    pub terminal: bool,
}

/// An event before the store numbers it: the store gives it the next
/// sequence of its stream and the event id that sequence makes.
#[derive(Debug, Clone, PartialEq)]
pub struct EventDraft {
    pub session_id: String,
    pub stream_id: String,
    pub request_id: String,
    pub kind: String,
    pub payload: Value,
    pub terminal: bool,
}

/// The sequence of the last event a reader has seen. The default cursor stands
/// before a stream's first event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventCursor(pub u64);

impl EventCursor {
    pub fn parse(token: &str) -> Result<Self, ReplayError> {
        let value = token
            .strip_prefix("cursor-")
            .ok_or(ReplayError::InvalidCursor)?;
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ReplayError::InvalidCursor);
        }
        value
            .parse::<u64>()
            .map(Self)
            .map_err(|_overflow| ReplayError::InvalidCursor)
    }

    /// The sequence padded to the digits of the largest one, so tokens order
    /// as text exactly as their sequences order as numbers.
    pub fn token(self) -> String {
        let width = u64::MAX.to_string().len();
        format!("cursor-{:0>width$}", self.0)
    }
}

impl SessionEventV1 {
    pub fn cursor(&self) -> String {
        EventCursor(self.sequence).token()
    }

    pub fn wire_value(&self) -> Value {
        serde_json::json!({
            "type": "event",
            "sessionId": self.session_id,
            "streamId": self.stream_id,
            "sequence": self.sequence,
            "cursor": self.cursor(),
            "eventId": self.event_id,
            "requestId": self.request_id,
            "kind": self.kind,
            "payload": self.payload,
            "terminal": self.terminal,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayError {
    InvalidStream,
    InvalidCursor,
    CursorAhead { latest: u64 },
    CursorTooOld { earliest: u64 },
    TerminalAlreadyRecorded,
    CorruptLog(String),
    Storage(String),
}
