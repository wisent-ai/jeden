//! Persisted completion protocol values, not operator-tunable execution limits.

pub const SCHEMA_VERSION: u32 = 1;
/// No native mutation has been committed before the first revision.
pub(crate) const INITIAL_REVISION: u64 = 0;
/// HTTP responses at or above this protocol boundary are failures, not evidence of success.
pub(crate) const HTTP_ERROR_STATUS: u64 = 400;
/// A preview bounds prompt copying; task_evidence retains access to the complete recorded result.
pub(crate) const EVIDENCE_PREVIEW_CHARS: usize = 2048;
/// The output budget one completion inspection is asked for. A provider's own
/// default cut an acceptance review off mid-array — the answer parsed as
/// truncated JSON, was corrected once, and was cut again — so the controller
/// asks for a budget wide enough to carry every task, criterion and evidence
/// reference of a retained request instead of inheriting a provider default.
pub(crate) const INSPECTION_OUTPUT_TOKENS: u32 = 16_384;
