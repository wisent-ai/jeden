//! What can go wrong between a caller and a session, said in terms the caller
//! can act on, and the transport an implementation must provide.
//!
//! Split out of `sdk/client.rs`, which had grown past the module line cap.

use super::super::protocol::{Envelope, ErrorEnvelope, ValidationError};
use futures::future::BoxFuture;
use std::error::Error;
use std::fmt;

/// An I/O failure reported by an injected session transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    message: String,
}

impl TransportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for TransportError {}

/// Object-safe, asynchronous envelope transport injected into [`SessionClient`].
///
/// Implementations must permit one `recv` to be in flight concurrently with `send` calls.
pub trait SessionTransport: Send + Sync + 'static {
    fn send(&self, envelope: Envelope) -> BoxFuture<'_, Result<(), TransportError>>;
    fn recv(&self) -> BoxFuture<'_, Result<Envelope, TransportError>>;
}

/// A failure produced by the asynchronous session client.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    Validation(ValidationError),
    Transport(TransportError),
    Protocol(ErrorEnvelope),
    DuplicateRequestId(String),
    Disposed,
    EventStreamLagged,
    UnexpectedEnvelope(&'static str),
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => write!(formatter, "invalid request: {error}"),
            Self::Transport(error) => write!(formatter, "transport failed: {error}"),
            Self::Protocol(error) => write!(
                formatter,
                "protocol error {}: {}",
                error.error.code, error.error.message
            ),
            Self::DuplicateRequestId(id) => write!(formatter, "duplicate request id: {id}"),
            Self::Disposed => formatter.write_str("session client is disposed"),
            Self::EventStreamLagged => formatter.write_str("event subscriber lagged"),
            Self::UnexpectedEnvelope(kind) => {
                write!(formatter, "unexpected inbound {kind} envelope")
            }
        }
    }
}

impl Error for ClientError {}

impl From<ValidationError> for ClientError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}
