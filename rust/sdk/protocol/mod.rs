//! The session protocol envelopes, and the tag each one carries on the wire.

use serde::{Deserialize, Serialize};

mod reply;
mod request;
mod validation;

pub use reply::{ErrorEnvelope, EventEnvelope, ResponseEnvelope};
pub use request::{ProtocolErrorBody, RequestEnvelope, RequestMeta};
pub use validation::{ReplayParams, ValidationError};

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
