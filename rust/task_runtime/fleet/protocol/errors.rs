//! Every way the worker protocol can refuse, said so the reader knows whose
//! fault it is and what to do.
//!
//! Split out of `task_runtime/fleet/protocol.rs`, which had grown past the
//! module line cap.

use super::ProtocolVersion;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    UnsupportedVersion {
        minimum: ProtocolVersion,
        maximum: ProtocolVersion,
    },
    Invalid(String),
    NotFound(String),
    NoPlacement(String),
    LeaseLost(String),
    StaleFence {
        expected: u64,
        actual: u64,
    },
    Conflict(String),
    Cancelled(String),
    Storage(String),
    Transport(String),
}
impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion { minimum, maximum } => write!(
                f,
                "unsupported worker protocol range {}.{}..={}.{}",
                minimum.major, minimum.minor, maximum.major, maximum.minor
            ),
            Self::Invalid(v)
            | Self::NotFound(v)
            | Self::NoPlacement(v)
            | Self::LeaseLost(v)
            | Self::Conflict(v)
            | Self::Cancelled(v)
            | Self::Storage(v)
            | Self::Transport(v) => f.write_str(v),
            Self::StaleFence { expected, actual } => write!(
                f,
                "stale fencing token {actual}; current token is {expected}"
            ),
        }
    }
}
impl std::error::Error for ProtocolError {}
