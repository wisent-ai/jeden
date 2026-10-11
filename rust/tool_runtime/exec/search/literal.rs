//! Searching many files for plain text at once, across the cores the machine
//! has.
//!
//! Split out of `tool_runtime/exec/search.rs`, which had grown past the module
//! line cap.

use super::walk::{check, discover};
use crate::tool_runtime::shared::machine_parallelism;
use crate::tool_runtime::ToolRuntime;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

pub(super) fn text_files(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Vec<PathBuf>, String> {
    Ok(discover(runtime, input, false)?
        .into_iter()
        .filter_map(|(path, is_dir)| if is_dir { None } else { Some(path) })
        .collect())
}

/// The matches, and every file that could not be read named with its error:
/// a search that silently missed a file reads as "no match there".
pub(super) fn parallel_literal(
    runtime: &ToolRuntime<'_>,
    files: &[PathBuf],
    query: &str,
    case: bool,
    max_matches: usize,
) -> Result<(Vec<(usize, usize, String)>, Vec<String>), String> {
    let output = Mutex::new(Vec::new());
    let unreadable = Mutex::new(Vec::<String>::new());
    let workers = machine_parallelism("search_files")?;
    let chunk = files.len().max(1).div_ceil(workers);
    std::thread::scope(|scope| {
        for (chunk_index, part) in files.chunks(chunk).enumerate() {
            let output = &output;
            let unreadable = &unreadable;
            let cwd = runtime.cwd;
            let cancellation = runtime.operation.cancellation().clone();
            let needle = query.to_string();
            scope.spawn(move || {
                let mut collected = 0usize;
                for (offset, path) in part.iter().enumerate() {
                    if collected >= max_matches {
                        break;
                    }
                    if cancellation.is_cancelled() {
                        break;
                    }
                    let content = match fs::read_to_string(path) {
                        Ok(content) => content,
                        // Gone since listing, or not text: nothing to search.
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData
                            ) =>
                        {
                            continue
                        }
                        Err(error) => {
                            if let Ok(mut names) = unreadable.lock() {
                                names
                                    .push(format!("{}: {error}", super::walk::rel_path(cwd, path)));
                            }
                            continue;
                        }
                    };
                    if content.contains('\0') {
                        continue;
                    }
                    for (line_index, line) in content.lines().enumerate() {
                        let found = if case {
                            line.contains(&needle)
                        } else {
                            line.to_lowercase().contains(&needle)
                        };
                        if found && collected < max_matches {
                            if let Ok(mut values) = output.lock() {
                                values.push((
                                    chunk_index * chunk + offset,
                                    line_index + 1,
                                    line.to_string(),
                                ));
                                collected += 1;
                            }
                        }
                    }
                }
            });
        }
    });
    check(runtime)?;
    let mut values = output
        .into_inner()
        .map_err(|_| "search result lock poisoned")?;
    values.sort_by_key(|a| (a.0, a.1));
    let mut unreadable = unreadable
        .into_inner()
        .map_err(|_| "search unreadable-file lock poisoned")?;
    unreadable.sort();
    Ok((values, unreadable))
}
