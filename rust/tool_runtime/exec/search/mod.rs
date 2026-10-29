//! The search tools: text in one file, text across many, paths by pattern,
//! and a regular expression across the tree.

use crate::tally::FIRST;
use glob::Pattern;
use serde_json::{json, Value};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use crate::tool_runtime::shared::{amount_input, count_input, flag_input, jail_path, string_input};
use crate::tool_runtime::ToolRuntime;

mod scan;
mod walk;

use scan::{parallel_scan, text_files, Found};
use walk::{check, discover, rel_path};

/// Whether `line` holds `needle`; without case sensitivity `needle` is
/// already lower case.
fn holds(line: &str, needle: &str, case: bool) -> bool {
    if case {
        line.contains(needle)
    } else {
        line.to_lowercase().contains(needle)
    }
}

fn needle_of(query: &str, case: bool) -> String {
    if case {
        query.to_owned()
    } else {
        query.to_lowercase()
    }
}

/// The matches the caller's window covers, as the answer lists them.
fn window(
    runtime: &ToolRuntime<'_>,
    files: &[PathBuf],
    found: Vec<Found>,
    skip: usize,
    limit: usize,
) -> Vec<Value> {
    found
        .into_iter()
        .skip(skip)
        .take(limit)
        .map(|(file, line, text)| {
            json!({"path": rel_path(runtime.cwd, &files[file]), "line": line, "text": text})
        })
        .collect()
}

pub(crate) fn search_text(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let query = string_input(input, "query").ok_or("search_text requires query")?;
    let label = string_input(input, "path").ok_or("search_text requires path")?;
    let case = flag_input(input, "caseSensitive", "search_text")?;
    let file = jail_path(runtime.cwd, &label)?;
    let mut matches = Vec::new();
    let reader = BufReader::new(File::open(file).map_err(|value| value.to_string())?);
    let needle = needle_of(&query, case);
    for (number, line) in (FIRST..).zip(reader.lines()) {
        check(runtime)?;
        let line = line.map_err(|value| value.to_string())?;
        // Every matching line of the one file is answered; no count is
        // chosen for the model.
        if holds(&line, &needle, case) {
            matches.push(json!({"line": number, "text": line}));
        }
    }
    Ok(json!({"ok":true,"path":label,"query":query,"matches":matches}))
}

pub(crate) fn search_files(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let query = string_input(input, "query").ok_or("search_files requires query")?;
    let case = flag_input(input, "caseSensitive", "search_files")?;
    let skip = amount_input(input, "skip", "search_files")?.0;
    let limit = count_input(input, "limit", "search_files")? as usize;
    let files = text_files(runtime, input)?;
    let needle = needle_of(&query, case);
    let (found, unreadable) = parallel_scan(
        runtime,
        &files,
        "search_files",
        skip.saturating_add(limit),
        |content| {
            (FIRST..)
                .zip(content.lines())
                .filter(|(_, line)| holds(line, &needle, case))
                .map(|(number, line)| (number, line.to_owned()))
                .collect()
        },
    )?;
    let matches = window(runtime, &files, found, skip, limit);
    Ok(
        json!({"searchedFiles":files.len(),"skip":skip,"limit":limit,"matches":matches,"unreadable":unreadable}),
    )
}

/// The patterns a path search matches: a list, one pattern, or every path
/// under the roots when none is stated. A pattern that is not a string is
/// refused by name.
fn glob_patterns(input: &Value) -> Result<Vec<String>, String> {
    match input.get("patterns") {
        None | Some(Value::Null) => Ok(vec!["**".to_owned()]),
        Some(Value::String(pattern)) => Ok(vec![pattern.clone()]),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| match value.as_str() {
                Some(pattern) => Ok(pattern.to_owned()),
                None => Err(format!("glob_paths patterns must be strings, not {value}")),
            })
            .collect(),
        Some(other) => Err(format!(
            "glob_paths patterns must be a string or a list of strings, not {other}"
        )),
    }
}

pub(crate) fn glob_paths(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let limit = count_input(input, "limit", "glob_paths")? as usize;
    let skip = amount_input(input, "skip", "glob_paths")?.0;
    let patterns = glob_patterns(input)?
        .iter()
        .map(|value| {
            Pattern::new(value).map_err(|error| format!("glob_paths pattern {value:?}: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let matches = discover(runtime, input, true)?
        .into_iter()
        .filter_map(|(path, is_dir)| {
            let label = rel_path(runtime.cwd, &path);
            patterns
                .iter()
                .any(|pattern| pattern.matches(&label))
                .then(|| json!({"path":label,"type":if is_dir{"directory"}else{"file"}}))
        })
        .skip(skip)
        .take(limit)
        .collect::<Vec<_>>();
    Ok(json!({"skip":skip,"limit":limit,"matches":matches}))
}

pub(crate) fn grep_regex(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let expr = string_input(input, "expr")
        .or_else(|| string_input(input, "pattern"))
        .ok_or("grep_regex requires expr")?;
    let case = flag_input(input, "caseSensitive", "grep_regex")?;
    let multiline = flag_input(input, "multiline", "grep_regex")? || expr.contains('\n');
    let matcher = regex::RegexBuilder::new(&expr)
        .case_insensitive(!case)
        .dot_matches_new_line(multiline)
        .build()
        .map_err(|error| error.to_string())?;
    let files = text_files(runtime, input)?;
    let skip = amount_input(input, "skip", "grep_regex")?.0;
    let limit = count_input(input, "limit", "grep_regex")? as usize;
    let (found, unreadable) = parallel_scan(
        runtime,
        &files,
        "grep_regex",
        skip.saturating_add(limit),
        |content| {
            if multiline {
                matcher
                    .find_iter(content)
                    .map(|found| {
                        let breaks_before = content[..found.start()]
                            .bytes()
                            .filter(|byte| *byte == b'\n')
                            .count() as u64;
                        let text = found
                            .as_str()
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        (FIRST + breaks_before, text)
                    })
                    .collect()
            } else {
                (FIRST..)
                    .zip(content.lines())
                    .filter(|(_, line)| matcher.is_match(line))
                    .map(|(number, line)| (number, line.to_owned()))
                    .collect()
            }
        },
    )?;
    let matches = window(runtime, &files, found, skip, limit);
    Ok(
        json!({"searchedFiles":files.len(),"skip":skip,"limit":limit,"matches":matches,"unreadable":unreadable}),
    )
}
