//! Deciding which files a search is allowed to look at, before it looks at
//! any of them.
//!
//! Split out of `tool_runtime/exec/search.rs`, which had grown past the module
//! line cap.

use crate::tool_runtime::shared::{bool_input, jail_path, string_input, u64_input};
use crate::tool_runtime::ToolRuntime;
use glob::Pattern;
use ignore::{WalkBuilder, WalkState};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_SEARCH_FILES: usize = 20_000;
pub(super) const MAX_SEARCH_FILE_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn check(runtime: &ToolRuntime<'_>) -> Result<(), String> {
    if runtime.operation.cancellation().is_cancelled() {
        return Err("search cancelled".into());
    }
    Ok(())
}

pub(super) fn rel_path(cwd: &Path, file: &Path) -> String {
    file.strip_prefix(cwd)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

pub(super) fn roots(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Vec<PathBuf>, String> {
    if let Some(paths) = input.get("paths").and_then(Value::as_array) {
        return paths
            .iter()
            .filter_map(Value::as_str)
            .map(|path| jail_path(runtime.cwd, path))
            .collect();
    }
    Ok(vec![jail_path(
        runtime.cwd,
        &string_input(input, "path").unwrap_or_else(|| ".".into()),
    )?])
}

pub(super) fn discover(
    runtime: &ToolRuntime<'_>,
    input: &Value,
    include_dirs: bool,
) -> Result<Vec<(PathBuf, bool)>, String> {
    check(runtime)?;
    let hidden = bool_input(input, "hidden", false);
    let gitignore = bool_input(input, "gitignore", true);
    let output = Arc::new(Mutex::new(Vec::<(PathBuf, bool)>::new()));
    let error = Arc::new(Mutex::new(None::<String>));
    for root in roots(runtime, input)? {
        let metadata = fs::metadata(&root).map_err(|value| value.to_string())?;
        if metadata.is_file() {
            output
                .lock()
                .map_err(|_| "search result lock poisoned")?
                .push((root, false));
            continue;
        }
        let mut builder = WalkBuilder::new(&root);
        builder
            .hidden(!hidden)
            .git_ignore(gitignore)
            .git_exclude(gitignore)
            .ignore(gitignore)
            .parents(gitignore)
            .require_git(false)
            .threads(
                std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(2)
                    .min(8),
            );
        let output = Arc::clone(&output);
        let error = Arc::clone(&error);
        let cancellation = runtime.operation.cancellation().clone();
        builder.build_parallel().run(|| {
            let output = Arc::clone(&output);
            let error = Arc::clone(&error);
            let cancellation = cancellation.clone();
            let root = root.clone();
            Box::new(move |entry| {
                if cancellation.is_cancelled() {
                    return WalkState::Quit;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(value) => {
                        if let Ok(mut slot) = error.lock() {
                            if slot.is_none() {
                                *slot = Some(value.to_string());
                            }
                        }
                        return WalkState::Continue;
                    }
                };
                if entry.path() == root {
                    return WalkState::Continue;
                }
                let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
                if !include_dirs && is_dir {
                    return WalkState::Continue;
                }
                if let Ok(mut values) = output.lock() {
                    if values.len() >= MAX_SEARCH_FILES {
                        return WalkState::Quit;
                    }
                    values.push((entry.into_path(), is_dir));
                }
                WalkState::Continue
            })
        });
        check(runtime)?;
    }
    if let Some(error) = error
        .lock()
        .map_err(|_| "search error lock poisoned")?
        .take()
    {
        return Err(error);
    }
    let mut values = Arc::try_unwrap(output)
        .map_err(|_| "search workers still active")?
        .into_inner()
        .map_err(|_| "search result lock poisoned")?;
    values.sort_by(|left, right| left.0.cmp(&right.0));
    values.dedup_by(|left, right| left.0 == right.0);
    values.truncate(MAX_SEARCH_FILES);
    Ok(values)
}
