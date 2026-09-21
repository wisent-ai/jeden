//! The four places an answer can come from. Each module owns one source,
//! answers with `SourceOutcome`, and never lets a failure of its own become
//! the advisor's failure: an unreachable source is a reported status.

pub(super) mod files;
pub(super) mod ground_truth;
pub(super) mod memory;
pub(super) mod transcripts;
