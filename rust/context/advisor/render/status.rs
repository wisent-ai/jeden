//! What each context source answered: whether it could be read, what it
//! considered and how many recommendations it returned.

use std::time::Instant;

use serde::Serialize;

use super::Recommendation;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceStatus {
    pub(crate) source: String,
    pub(crate) available: bool,
    /// The observed state in one sentence: what was read, or what refused and
    /// why. Never empty.
    pub(crate) detail: String,
    /// How much the source read to answer; none when it read nothing it can
    /// count (a health route) or could not answer.
    pub(crate) considered: Option<usize>,
    /// How many recommendations the source returned; none when it was asked
    /// for none (a health probe) or could not answer.
    pub(crate) returned: Option<usize>,
    pub(crate) elapsed_ms: u128,
}

impl SourceStatus {
    pub(crate) fn unavailable(source: &str, detail: impl Into<String>, started: Instant) -> Self {
        Self {
            source: source.to_string(),
            available: false,
            detail: detail.into(),
            elapsed_ms: started.elapsed().as_millis(),
            ..Self::default()
        }
    }

    pub(crate) fn available(
        source: &str,
        detail: impl Into<String>,
        considered: usize,
        returned: usize,
        started: Instant,
    ) -> Self {
        Self {
            source: source.to_string(),
            available: true,
            detail: detail.into(),
            considered: Some(considered),
            returned: Some(returned),
            elapsed_ms: started.elapsed().as_millis(),
        }
    }

    /// A source that answered a health probe: it was asked for no
    /// recommendation, so it returned none, and it states what it counted
    /// when its health answer counts anything.
    pub(crate) fn probed(
        source: &str,
        detail: impl Into<String>,
        considered: Option<usize>,
        started: Instant,
    ) -> Self {
        Self {
            source: source.to_string(),
            available: true,
            detail: detail.into(),
            considered,
            returned: None,
            elapsed_ms: started.elapsed().as_millis(),
        }
    }
}

/// One source's answer: what it found and what state it was in.
pub(crate) struct SourceOutcome {
    pub(crate) hits: Vec<Recommendation>,
    pub(crate) status: SourceStatus,
}
