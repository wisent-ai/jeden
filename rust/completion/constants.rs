//! Persisted completion protocol values, not operator-tunable execution limits.

/// Version three retains each request's time-to-completion estimate and the
/// moment it was verified complete; version two added typed defects. An older
/// writer must refuse a newer file rather than drop what it cannot model.
pub const SCHEMA_VERSION: u32 = 3;
/// Versions a reader upgrades in place. Their files hold nothing version three
/// dropped, so reading one loses nothing; the next write records version three.
pub(crate) const UPGRADED_SCHEMA_VERSIONS: [u32; 2] = [1, 2];
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
/// The budget a cut-off inspection is asked again with, and the reason the
/// single correction is worth spending. An answer that did not fit in
/// `INSPECTION_OUTPUT_TOKENS` cannot fit in it when the refusal is quoted back
/// as well: on 2026-09-15 two real journeys ended as `Work remains open
/// (acceptance_review)` after being cut at 2387 and then 4850 bytes, with the
/// files written and the review unread.
pub(crate) const INSPECTION_RETRY_OUTPUT_TOKENS: u32 = 2 * INSPECTION_OUTPUT_TOKENS;
/// How much of an unreadable model answer a refusal quotes on each side of the
/// parser's position. Wide enough to show the object that failed, short enough
/// to stay one readable sentence in a session ledger and a terminal.
pub(crate) const REFUSAL_EXCERPT_CHARS: usize = 120;
