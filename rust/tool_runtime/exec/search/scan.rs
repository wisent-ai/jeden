//! Searching many files at once, across the cores the machine has. The
//! literal and the regular-expression search differ only in how they find
//! matches inside one file's text; the walk, the split across workers, the
//! files that could not be read and the ordering are this module's.

use super::walk::{check, discover, rel_path};
use crate::tool_runtime::shared::machine_parallelism;
use crate::tool_runtime::ToolRuntime;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

/// One match: the file's place in the searched list, its line number counted
/// from the first line, and the matching text.
pub(super) type Found = (usize, u64, String);

pub(super) fn text_files(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Vec<PathBuf>, String> {
    Ok(discover(runtime, input, false)?
        .into_iter()
        .filter_map(|(path, is_dir)| if is_dir { None } else { Some(path) })
        .collect())
}

/// Every match `find` reports in `files`, ordered by file and line, and every
/// file that could not be read named with its error: a search that silently
/// missed a file reads as "no match there". Each worker stops once it holds
/// `max_matches`, which is enough for any window the caller asked for since
/// its files come before the next worker's.
pub(super) fn parallel_scan<F>(
    runtime: &ToolRuntime<'_>,
    files: &[PathBuf],
    tool: &str,
    max_matches: usize,
    find: F,
) -> Result<(Vec<Found>, Vec<String>), String>
where
    F: Fn(&str) -> Vec<(u64, String)> + Sync,
{
    if files.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let output = Mutex::new(Vec::new());
    let unreadable = Mutex::new(Vec::new());
    let workers = machine_parallelism(tool)?;
    let chunk = files.len().div_ceil(workers);
    std::thread::scope(|scope| {
        for (chunk_index, part) in files.chunks(chunk).enumerate() {
            let (output, unreadable, find) = (&output, &unreadable, &find);
            let cwd = runtime.cwd;
            let cancellation = runtime.operation.cancellation().clone();
            scope.spawn(move || {
                let mut found = Vec::new();
                let mut missed = Vec::new();
                for (offset, path) in part.iter().enumerate() {
                    if found.len() >= max_matches || cancellation.is_cancelled() {
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
                            missed.push(format!("{}: {error}", rel_path(cwd, path)));
                            continue;
                        }
                    };
                    if content.contains('\0') {
                        continue;
                    }
                    let file = chunk_index * chunk + offset;
                    for (line, text) in find(&content) {
                        if found.len() >= max_matches {
                            break;
                        }
                        found.push((file, line, text));
                    }
                }
                // Each worker adds its whole result in one step, so a lock a
                // panicking worker poisoned still holds whole results.
                match output.lock() {
                    Ok(mut values) => values.extend(found),
                    Err(poisoned) => poisoned.into_inner().extend(found),
                }
                match unreadable.lock() {
                    Ok(mut names) => names.extend(missed),
                    Err(poisoned) => poisoned.into_inner().extend(missed),
                }
            });
        }
    });
    check(runtime)?;
    let mut values = output
        .into_inner()
        .map_err(|_| format!("{tool}: a search worker panicked"))?;
    values.sort_by_key(|found: &Found| (found.0, found.1));
    let mut unreadable: Vec<String> = unreadable
        .into_inner()
        .map_err(|_| format!("{tool}: a search worker panicked"))?;
    unreadable.sort();
    Ok((values, unreadable))
}
