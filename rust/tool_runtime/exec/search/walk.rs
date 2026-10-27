//! Deciding which files a search is allowed to look at, before it looks at
//! any of them.
//!
//! Split out of `tool_runtime/exec/search.rs`, which had grown past the module
//! line cap.

use crate::tool_runtime::shared::{
    flag_input, jail_path, machine_parallelism, optional_bool_input, string_input,
};
use crate::tool_runtime::ToolRuntime;
use ignore::{WalkBuilder, WalkState};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(super) fn check(runtime: &ToolRuntime<'_>) -> Result<(), String> {
    if runtime.operation.cancellation().is_cancelled() {
        return Err("search cancelled".into());
    }
    Ok(())
}

/// `file` as the search reports it: relative to the working directory, or
/// its full path when it lies outside it (a root given as an absolute path
/// the jail admits), so a hit is never shown under a path it does not have.
pub(super) fn rel_path(cwd: &Path, file: &Path) -> String {
    let shown = match file.strip_prefix(cwd) {
        Ok(relative) => relative,
        Err(_outside) => file,
    };
    shown.to_string_lossy().replace('\\', "/")
}

/// The directories a search walks: each of `paths`, else `path`, else the
/// workspace root. A `paths` entry that is not a string is refused by name.
pub(super) fn roots(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Vec<PathBuf>, String> {
    if let Some(paths) = input.get("paths").and_then(Value::as_array) {
        return paths
            .iter()
            .map(|path| match path.as_str() {
                Some(path) => jail_path(runtime.cwd, path),
                None => Err(format!("search paths must be strings, not {path}")),
            })
            .collect();
    }
    let root = match string_input(input, "path") {
        Some(path) => jail_path(runtime.cwd, &path)?,
        None => jail_path(runtime.cwd, ".")?,
    };
    Ok(vec![root])
}

pub(super) fn discover(
    runtime: &ToolRuntime<'_>,
    input: &Value,
    include_dirs: bool,
) -> Result<Vec<(PathBuf, bool)>, String> {
    check(runtime)?;
    let hidden = flag_input(input, "hidden", "search")?;
    // Ignore files are honoured unless the caller turns them off.
    let gitignore = optional_bool_input(input, "gitignore", "search")? != Some(false);
    let output = Arc::new(Mutex::new(Vec::<(PathBuf, bool)>::new()));
    let workers = machine_parallelism("search")?;
    let error = Arc::new(Mutex::new(None::<String>));
    for root in roots(runtime, input)? {
        let metadata = fs::metadata(&root).map_err(|value| value.to_string())?;
        if metadata.is_file() {
            // Results and the first walk error change by one push or one set,
            // so a worker that panicked left them whole; skipped on poison, a
            // search answered without the files it had found.
            crate::panicked::held(&output).push((root, false));
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
            .threads(workers)
            .filter_entry({
                let home = crate::dirs_home();
                let root = root.clone();
                move |entry| {
                    !crate::tool_runtime::runtime_ops::platform::guarded::walk_skips(
                        &home,
                        &root,
                        entry.path(),
                        entry.file_type().is_some_and(|kind| kind.is_dir()),
                    )
                }
            });
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
                        let mut slot = crate::panicked::held(&error);
                        if slot.is_none() {
                            *slot = Some(value.to_string());
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
                crate::panicked::held(&output).push((entry.into_path(), is_dir));
                WalkState::Continue
            })
        });
        check(runtime)?;
    }
    if let Some(error) = crate::panicked::held(&error).take() {
        return Err(error);
    }
    let output = Arc::try_unwrap(output).map_err(|_shared| "search workers still active")?;
    let mut values = match output.into_inner() {
        Ok(values) => values,
        Err(poisoned) => poisoned.into_inner(),
    };
    values.sort_by(|left, right| left.0.cmp(&right.0));
    values.dedup_by(|left, right| left.0 == right.0);
    Ok(values)
}
