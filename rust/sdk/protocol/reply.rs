//! What comes back from a session: a response, an event, or an error.
//!
//! Split out of `sdk/protocol.rs`, which had grown past the module line cap.

use super::request::ProtocolErrorBody;
use super::validation::{require_non_empty, ValidationError};
use super::{ErrorTag, EventTag, ResponseTag};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseEnvelope {
    #[serde(rename = "type")]
    envelope_type: ResponseTag,
    pub id: String,
    pub result: Value,
}

impl ResponseEnvelope {
    pub fn new(id: impl Into<String>, result: Value) -> Result<Self, ValidationError> {
        let response = Self {
            envelope_type: ResponseTag::Response,
            id: id.into(),
            result,
        };
        response.validate()?;
        Ok(response)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        require_non_empty("id", &self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventEnvelope {
    #[serde(rename = "type")]
    envelope_type: EventTag,
    pub session_id: String,
    pub stream_id: String,
    pub sequence: u64,
    pub cursor: String,
    pub event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub kind: String,
    pub payload: Value,
}

impl EventEnvelope {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: impl Into<String>,
        stream_id: impl Into<String>,
        sequence: u64,
        cursor: impl Into<String>,
        event_id: impl Into<String>,
        request_id: Option<String>,
        kind: impl Into<String>,
        payload: Value,
    ) -> Result<Self, ValidationError> {
        let event = Self {
            envelope_type: EventTag::Event,
            session_id: session_id.into(),
            stream_id: stream_id.into(),
            sequence,
            cursor: cursor.into(),
            event_id: event_id.into(),
            request_id,
            kind: kind.into(),
            payload,
        };
        event.validate()?;
        Ok(event)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        require_non_empty("sessionId", &self.session_id)?;
        require_non_empty("streamId", &self.stream_id)?;
        require_non_empty("cursor", &self.cursor)?;
        require_non_empty("eventId", &self.event_id)?;
        if let Some(request_id) = &self.request_id {
            require_non_empty("requestId", request_id)?;
        }
        require_non_empty("kind", &self.kind)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorEnvelope {
    #[serde(rename = "type")]
    envelope_type: ErrorTag,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub error: ProtocolErrorBody,
}

impl ErrorEnvelope {
    pub fn new(id: Option<String>, error: ProtocolErrorBody) -> Result<Self, ValidationError> {
        let envelope = Self {
            envelope_type: ErrorTag::Error,
            id,
            error,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(id) = &self.id {
            require_non_empty("id", id)?;
        }
        self.error.validate()
    }
}
