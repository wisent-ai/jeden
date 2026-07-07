use glob::Pattern;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::tool_runtime::shared::{bool_input, jail_path, string_input, u64_input};
use crate::tool_runtime::ToolRuntime;

const MAX_SEARCH_FILES: usize = 2_000;

pub(crate) fn search_text(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let query = string_input(input, "query").ok_or("search_text requires query")?;
    let path = string_input(input, "path").ok_or("search_text requires path")?;
    let case_sensitive = bool_input(input, "caseSensitive", false);
    let file = jail_path(runtime.cwd, &path)?;
    let content = fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let needle = if case_sensitive { query.clone() } else { query.to_lowercase() };
    let mut matches = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let hay = if case_sensitive { line.to_string() } else { line.to_lowercase() };
        if hay.contains(&needle) {
            matches.push(json!({"line": idx + 1, "text": line}));
            if matches.len() >= 50 { break; }
        }
    }
    Ok(json!({"ok": true, "path": path, "query": query, "matches": matches}))
}

fn rel_path(cwd: &Path, file: &Path) -> String {
    file.strip_prefix(cwd).unwrap_or(file).to_string_lossy().replace('\\', "/")
}

fn skip_discovery_dir(name: &str) -> bool {
    matches!(name, ".git" | "node_modules" | "dist" | "build" | "coverage" | ".next" | "target" | ".turbo" | ".vercel")
}

fn collect_files(root: &Path, hidden: bool, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if out.len() >= MAX_SEARCH_FILES { return Ok(()); }
    let entries = fs::read_dir(root).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        if out.len() >= MAX_SEARCH_FILES { break; }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !hidden && name.starts_with('.') { continue; }
        let meta = match entry.metadata() { Ok(meta) => meta, Err(_) => continue };
        if meta.is_dir() {
            if skip_discovery_dir(&name) { continue; }
            collect_files(&path, hidden, out)?;
        } else if meta.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn collect_entries(root: &Path, hidden: bool, out: &mut Vec<(PathBuf, &'static str)>) -> Result<(), String> {
    if out.len() >= MAX_SEARCH_FILES { return Ok(()); }
    let entries = fs::read_dir(root).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        if out.len() >= MAX_SEARCH_FILES { break; }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !hidden && name.starts_with('.') { continue; }
        let meta = match entry.metadata() { Ok(meta) => meta, Err(_) => continue };
        if meta.is_dir() {
            if skip_discovery_dir(&name) { continue; }
            out.push((path.clone(), "directory"));
            collect_entries(&path, hidden, out)?;
        } else if meta.is_file() {
            out.push((path, "file"));
        }
    }
    Ok(())
}

fn input_roots(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Vec<PathBuf>, String> {
    if let Some(paths) = input.get("paths").and_then(Value::as_array) {
        return paths.iter().filter_map(Value::as_str).map(|path| jail_path(runtime.cwd, path)).collect();
    }
    let path = string_input(input, "path").unwrap_or_else(|| ".".into());
    Ok(vec![jail_path(runtime.cwd, &path)?])
}

fn text_files_for_input(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Vec<PathBuf>, String> {
    let hidden = bool_input(input, "hidden", false);
    let mut files = Vec::new();
    for root in input_roots(runtime, input)? {
        let meta = fs::metadata(&root).map_err(|e| e.to_string())?;
        if meta.is_file() { files.push(root); }
        else if meta.is_dir() { collect_files(&root, hidden, &mut files)?; }
    }
    files.sort();
    Ok(files)
}

pub(crate) fn search_files(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let query = string_input(input, "query").ok_or("search_files requires query")?;
    let limit = u64_input(input, "limit", 500).clamp(1, 500) as usize;
    let skip = u64_input(input, "skip", 0) as usize;
    let case_sensitive = bool_input(input, "caseSensitive", false);
    let needle = if case_sensitive { query.clone() } else { query.to_lowercase() };
    let files = text_files_for_input(runtime, input)?;
    let mut seen = 0usize;
    let mut matches = Vec::new();
    for file in &files {
        if matches.len() >= limit { break; }
        let Ok(content) = fs::read_to_string(file) else { continue };
        if content.contains('\0') { continue; }
        for (idx, line) in content.lines().enumerate() {
            let hay = if case_sensitive { line.to_string() } else { line.to_lowercase() };
            if hay.contains(&needle) {
                seen += 1;
                if seen <= skip { continue; }
                matches.push(json!({"path": rel_path(runtime.cwd, file), "line": idx + 1, "text": line}));
                if matches.len() >= limit { break; }
            }
        }
    }
    Ok(json!({"searchedFiles": files.len(), "skip": skip, "limit": limit, "matches": matches}))
}

pub(crate) fn glob_paths(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let root = jail_path(runtime.cwd, &string_input(input, "path").unwrap_or_else(|| ".".into()))?;
    let limit = u64_input(input, "limit", 200).clamp(1, 2_000) as usize;
    let skip = u64_input(input, "skip", 0) as usize;
    let hidden = bool_input(input, "hidden", false);
    let raw_patterns = if let Some(patterns) = input.get("patterns").and_then(Value::as_array) {
        patterns.iter().filter_map(Value::as_str).map(ToString::to_string).collect::<Vec<_>>()
    } else {
        vec![string_input(input, "patterns").unwrap_or_else(|| "**".into())]
    };
    let patterns = raw_patterns.iter().filter_map(|pattern| Pattern::new(pattern).ok()).collect::<Vec<_>>();
    let mut paths = Vec::new();
    collect_entries(&root, hidden, &mut paths)?;
    let mut entries = paths.into_iter().filter_map(|(file, kind)| {
        let path = rel_path(runtime.cwd, &file);
        if patterns.iter().any(|pattern| pattern.matches(&path)) { Some(json!({"path": path, "type": kind})) } else { None }
    }).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.get("path").and_then(Value::as_str).unwrap_or("").to_string());
    let matches = entries.into_iter().skip(skip).take(limit).collect::<Vec<_>>();
    Ok(json!({"skip": skip, "limit": limit, "matches": matches}))
}

pub(crate) fn grep_regex(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let expr = string_input(input, "expr").ok_or("grep_regex requires expr")?;
    let limit = u64_input(input, "limit", 500).clamp(1, 500) as usize;
    let skip = u64_input(input, "skip", 0) as usize;
    let case_sensitive = bool_input(input, "caseSensitive", false);
    let multiline = bool_input(input, "multiline", false) || expr.contains('\n');
    let matcher = regex::RegexBuilder::new(&expr).case_insensitive(!case_sensitive).dot_matches_new_line(multiline).build().map_err(|e| e.to_string())?;
    let files = text_files_for_input(runtime, input)?;
    let mut seen = 0usize;
    let mut matches = Vec::new();
    for file in &files {
        if matches.len() >= limit { break; }
        let Ok(content) = fs::read_to_string(file) else { continue };
        if content.contains('\0') { continue; }
        if multiline {
            for mat in matcher.find_iter(&content) {
                seen += 1;
                if seen <= skip { continue; }
                let line = content[..mat.start()].lines().count() + 1;
                matches.push(json!({"path": rel_path(runtime.cwd, file), "line": line, "text": mat.as_str().split_whitespace().collect::<Vec<_>>().join(" ").chars().take(500).collect::<String>()}));
                if matches.len() >= limit { break; }
            }
        } else {
            for (idx, line) in content.lines().enumerate() {
                if matcher.is_match(line) {
                    seen += 1;
                    if seen <= skip { continue; }
                    matches.push(json!({"path": rel_path(runtime.cwd, file), "line": idx + 1, "text": line}));
                    if matches.len() >= limit { break; }
                }
            }
        }
    }
    Ok(json!({"searchedFiles": files.len(), "skip": skip, "limit": limit, "matches": matches}))
}
