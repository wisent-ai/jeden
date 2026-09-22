//! Everything the content-addressed store can refuse, and the sentence each
//! refusal prints.
//!
//! Split out of `cas/store.rs`, which had grown past the module line cap.

use super::super::digest::Digest;
use std::fmt;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub enum CasError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    DigestMismatch {
        expected: Digest,
        actual: Digest,
    },
    CorruptObject {
        expected: Digest,
        actual: Digest,
    },
    InvalidOffset {
        expected: u64,
        actual: u64,
    },
    InvalidPath(String),
    UnsupportedEntry(PathBuf),
    CaseCollision {
        directory: PathBuf,
        first: String,
        second: String,
    },
    InvalidSnapshot(String),
    Serialization(String),
}

impl CasError {
    pub(crate) fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}
impl fmt::Display for CasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(f, "{operation} {}: {source}", path.display()),
            Self::DigestMismatch { expected, actual } => {
                write!(f, "digest mismatch: expected {expected}, got {actual}")
            }
            Self::CorruptObject { expected, actual } => write!(
                f,
                "corrupt CAS object {expected}: content hashes to {actual}"
            ),
            Self::InvalidOffset { expected, actual } => write!(
                f,
                "invalid upload offset: expected {expected}, got {actual}"
            ),
            Self::InvalidPath(message) => write!(f, "invalid snapshot path: {message}"),
            Self::UnsupportedEntry(path) => write!(
                f,
                "snapshot entry is not a regular file or directory: {}",
                path.display()
            ),
            Self::CaseCollision {
                directory,
                first,
                second,
            } => write!(
                f,
                "case-folding collision in {}: {first:?} and {second:?}",
                directory.display()
            ),
            Self::InvalidSnapshot(message) => write!(f, "invalid snapshot: {message}"),
            Self::Serialization(message) => write!(f, "snapshot serialization failed: {message}"),
        }
    }
}
impl std::error::Error for CasError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
