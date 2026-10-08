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
