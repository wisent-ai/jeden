//! Finding the extension and custom tool files a workspace actually has,
//! rather than the ones a configuration claims.
//!
//! Split out of `slash/plugins/mod.rs`, which had grown past the module line
//! cap.

use crate::slash::common::{merged_config, read_json_value, resolve_cwd_path};
use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn is_extension_module_file(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("ts" | "js" | "mjs")
        )
}

fn extension_manifest_entries(dir: &Path) -> Vec<PathBuf> {
    let manifest = read_json_value(&dir.join("package.json"));
    let entries = manifest
        .pointer("/jeden/extensions")
        .and_then(Value::as_array)
        .or_else(|| manifest.pointer("/pi/extensions").and_then(Value::as_array));
    let mut out = Vec::new();
    if let Some(entries) = entries {
        for entry in entries {
            let Some(raw) = entry.as_str() else {
                continue;
            };
            let path = dir.join(raw);
            if is_extension_module_file(&path) {
                out.push(path);
            }
        }
    }
    out
}

fn extension_index_entry(dir: &Path) -> Option<PathBuf> {
    for name in ["index.ts", "index.js", "index.mjs"] {
        let path = dir.join(name);
        if is_extension_module_file(&path) {
            return Some(path);
        }
    }
    None
}

pub(super) fn discover_extension_module_files(root: &Path) -> Vec<PathBuf> {
    if is_extension_module_file(root) {
        return vec![root.to_path_buf()];
    }
    if !root.is_dir() {
        return Vec::new();
    }
    let manifest = extension_manifest_entries(root);
    if !manifest.is_empty() {
        return manifest;
    }
    if let Some(index) = extension_index_entry(root) {
        return vec![index];
    }
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_extension_module_file(&path) {
                out.push(path);
            } else if path.is_dir() {
                let manifest = extension_manifest_entries(&path);
                if !manifest.is_empty() {
                    out.extend(manifest);
                } else if let Some(index) = extension_index_entry(&path) {
                    out.push(index);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

pub(super) fn native_extension_roots(cwd: &Path) -> Vec<PathBuf> {
    let mut roots = vec![cwd.join(".jeden/extensions")];
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join(".jeden/extensions"));
    }
    roots
}

pub(super) fn configured_extension_paths(cwd: &Path) -> Vec<PathBuf> {
    merged_config(cwd)
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
    let mut dirs = Vec::new();
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".jeden/tools"));
    }
    dirs.push(cwd.join(".jeden/tools"));
    dirs.sort();
    dirs.dedup();
    for dir in dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let ext = path
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("");
                    if matches!(ext, "js" | "mjs") {
                        out.push(path.display().to_string());
                    }
                }
            }
        }
    }
    out.sort();
    out
}
