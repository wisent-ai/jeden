//! Saying no to a malformed envelope in a way the caller can act on.
//!
//! Split out of `sdk/protocol.rs`, which had grown past the module line cap.

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

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
    pub(super) fn new(field: &'static str, message: impl Into<String>) -> Self {
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

pub(super) fn require_non_empty(field: &'static str, value: &str) -> Result<(), ValidationError> {
    if value.is_empty() {
        Err(ValidationError::new(field, "must be a non-empty string"))
    } else {
        Ok(())
    }
}
