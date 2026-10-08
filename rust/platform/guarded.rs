//! Which folders a directory walk rooted at the user's home must not enter.
//!
//! On macOS the folders directly under the home that hold the user's own
//! things (photos, music, the desktop, downloads, other apps' data) are
//! guarded by a consent prompt, and the prompt names the process macOS holds
//! responsible: Oko, when Oko's broker started this Jeden. A walk rooted at
//! the home that descends opens every one of them, so a session whose working
//! directory is the home asked the person at the screen for Apple Music,
//! Photos, Desktop and Downloads on every turn. A walk rooted at the home
//! therefore reads only the home's own files; a folder under it is walked
//! when it is the walk's own root, because then it was asked for.

use std::path::Path;

/// True when `entry` is a folder directly under `home` and the walk was
/// rooted at `home` itself on macOS: the walk would open it only by passing
/// through, and opening it may raise a consent prompt nobody asked for.
pub(crate) fn walk_skips(home: &Path, root: &Path, entry: &Path, is_dir: bool) -> bool {
    cfg!(target_os = "macos") && is_dir && root == home && entry.parent() == Some(home)
}
