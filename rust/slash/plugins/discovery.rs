//! Finding the extension and custom tool files a workspace actually has,
//! rather than the ones a configuration claims.
//!
//! Split out of `slash/plugins/mod.rs`, which had grown past the module line
//! cap.

use crate::cli::config::merged_config_value;
use crate::hooks::extensions::loading::module_file;
use crate::slash::common::{dirs_home, resolve_cwd_path};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// The project's and the user's extension directories, the user's under the
/// one home lookup: skipped when HOME was unset, the picker hid every user
/// extension the loader still loaded.
pub(super) fn native_extension_roots(cwd: &Path) -> Vec<PathBuf> {
    vec![
        cwd.join(".jeden/extensions"),
        dirs_home().join(".jeden/extensions"),
    ]
}

/// The `extensions` the configuration layers that apply here declare, read
/// as every run reads them (user files, then the project's).
pub(super) fn configured_extension_paths(cwd: &Path) -> Vec<PathBuf> {
    merged_config_value(cwd)
        .get("extensions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|raw| resolve_cwd_path(cwd, raw))
        .collect()
}

pub(super) fn discover_custom_tool_files(cwd: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut dirs = vec![dirs_home().join(".jeden/tools"), cwd.join(".jeden/tools")];
    dirs.sort();
    dirs.dedup();
    for dir in dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if module_file(&path) {
                    out.push(path.display().to_string());
                }
            }
        }
    }
    out.sort();
    out
}
