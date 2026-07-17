use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::fmt;

/// The canonical session protocol identifier carried by every request.
pub const PROTOCOL_VERSION: &str = "jeden.session.v1";
/// The canonical replay method.
pub const SESSION_REPLAY_METHOD: &str = "session.replay";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum RequestTag {
    #[serde(rename = "request")]
    Request,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ResponseTag {
    #[serde(rename = "response")]
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum EventTag {
    #[serde(rename = "event")]
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ErrorTag {
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestMeta {
    pub protocol_version: String,
    pub idempotency_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

impl RequestMeta {
    pub fn new(idempotency_key: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            idempotency_key: idempotency_key.into(),
            deadline: None,
            trace_id: None,
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ValidationError::new(
                "meta.protocolVersion",
                "must equal jeden.session.v1",
            ));
        }
        require_non_empty("meta.idempotencyKey", &self.idempotency_key)?;
        if let Some(deadline) = &self.deadline {
            require_non_empty("meta.deadline", deadline)?;
        }
        if let Some(trace_id) = &self.trace_id {
            require_non_empty("meta.traceId", trace_id)?;
        }
        Ok(())
    }

    pub fn validate_mutating(&self) -> Result<(), ValidationError> {
        self.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub details: Value,
}

impl ProtocolErrorBody {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
        details: Value,
    ) -> Result<Self, ValidationError> {
        let body = Self {
            code: code.into(),
            message: message.into(),
            retryable,
            details,
        };
        body.validate()?;
        Ok(body)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestEnvelope {
    #[serde(rename = "type")]
    envelope_type: RequestTag,
    pub id: String,
    pub method: String,
    pub params: Value,
    pub meta: RequestMeta,
}

impl RequestEnvelope {
    pub fn new(
        id: impl Into<String>,
        method: impl Into<String>,
        params: Value,
        meta: RequestMeta,
    ) -> Result<Self, ValidationError> {
        let request = Self {
            envelope_type: RequestTag::Request,
            id: id.into(),
            method: method.into(),
            params,
            meta,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn new_mutating(
        id: impl Into<String>,
        method: impl Into<String>,
        params: Value,
        meta: RequestMeta,
    ) -> Result<Self, ValidationError> {
        let request = Self::new(id, method, params, meta)?;
        request.validate_mutating()?;
        Ok(request)
    }

    pub fn replay(
        id: impl Into<String>,
        session_id: impl Into<String>,
        cursor: Option<String>,
        limit: Option<u64>,
        meta: RequestMeta,
    ) -> Result<Self, ValidationError> {
        let params = ReplayParams {
            session_id: session_id.into(),
            cursor,
            limit,
        };
        params.validate()?;
        Self::new(
            id,
            SESSION_REPLAY_METHOD,
            serde_json::to_value(params).expect("ReplayParams serialization cannot fail"),
            meta,
        )
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        require_non_empty("id", &self.id)?;
        require_non_empty("method", &self.method)?;
        self.meta.validate()?;
        if self.method == SESSION_REPLAY_METHOD {
            let params: ReplayParams = serde_json::from_value(self.params.clone())
                .map_err(|error| ValidationError::new("params", error.to_string()))?;
            params.validate()?;
        }
        Ok(())
    }

    pub fn validate_mutating(&self) -> Result<(), ValidationError> {
        self.validate()?;
        self.meta.validate_mutating()
    }
}

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

/// A session protocol envelope, discriminated on the wire by its `type` field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Envelope {
    Request(RequestEnvelope),
    Response(ResponseEnvelope),
    Event(EventEnvelope),
    Error(ErrorEnvelope),
}

impl Envelope {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Request(value) => value.validate(),
            Self::Response(value) => value.validate(),
            Self::Event(value) => value.validate(),
            Self::Error(value) => value.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplayParams {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

impl ReplayParams {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require_non_empty("params.sessionId", &self.session_id)?;
        if let Some(cursor) = &self.cursor {
            require_non_empty("params.cursor", cursor)?;
        }
        if self.limit == Some(0) {
            return Err(ValidationError::new("params.limit", "must be at least 1"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub field: &'static str,
    pub message: String,
}

impl ValidationError {
    fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.field, self.message)
    }
}

impl Error for ValidationError {}

fn require_non_empty(field: &'static str, value: &str) -> Result<(), ValidationError> {
    if value.is_empty() {
        Err(ValidationError::new(field, "must be a non-empty string"))
    } else {
        Ok(())
    }
}
