//! What a caller sends, and what has to be true of it before it leaves.
//!
//! Split out of `sdk/protocol.rs`, which had grown past the module line cap.

use super::validation::{require_non_empty, ReplayParams, ValidationError};
use super::{RequestTag, PROTOCOL_VERSION, SESSION_REPLAY_METHOD};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestMeta {
    pub protocol_version: String,
    pub idempotency_key: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

impl RequestMeta {
    pub fn new(idempotency_key: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            idempotency_key: idempotency_key.into(),

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
