//! Searching many files for plain text at once, across the cores the machine
//! has.
//!
//! Split out of `tool_runtime/exec/search.rs`, which had grown past the module
//! line cap.

use super::walk::{check, discover};
use crate::tool_runtime::ToolRuntime;
use serde_json::Value;
use std::fs::File;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Mutex;
use crate::tool_runtime::exec::search::walk::MAX_SEARCH_FILE_BYTES;
use std::fs;

pub(super) fn text_files(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Vec<PathBuf>, String> {
    Ok(discover(runtime, input, false)?
        .into_iter()
        .filter_map(|(path, is_dir)| if is_dir { None } else { Some(path) })
        .collect())
}

pub(super) fn parallel_literal(
    runtime: &ToolRuntime<'_>,
    files: &[PathBuf],
    query: &str,
    case: bool,
    max_matches: usize,
) -> Result<Vec<(usize, usize, String)>, String> {
    let output = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2)
        .min(8);
    let chunk = files.len().max(1).div_ceil(workers);
    std::thread::scope(|scope| {
        for (chunk_index, part) in files.chunks(chunk).enumerate() {
            let output = &output;
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
                    if fs::metadata(path)
                        .map(|meta| meta.len() > MAX_SEARCH_FILE_BYTES)
                        .unwrap_or(true)
                    {
                        continue;
                    }
                    let Ok(content) = fs::read_to_string(path) else {
                        continue;
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
    Ok(values)
}
