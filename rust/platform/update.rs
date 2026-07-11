use super::{native, PlatformError};
use std::path::Path;

/// Commits a fully written update artifact with the native atomic replacement primitive.
/// When requested, `backup` is a durable copy/link of the prior destination suitable for rollback.
pub fn commit_staged_binary(
    staged: &Path,
    destination: &Path,
    backup: Option<&Path>,
) -> Result<(), PlatformError> {
    native().atomic_replace(staged, destination, backup)
}
