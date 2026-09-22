//! Finding what there is to load: which files are extension modules, which
//! declarative capabilities a root declares, and the fingerprint that says
//! whether anything under those roots has changed since the last generation.

use serde_json::Value;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use super::{DeclarativeCapability, MAX_DESCRIPTOR_BYTES, MAX_EXTENSION_FILES};

pub(super) mod host;
pub(super) mod materialize;
pub(super) mod roots;

pub(super) fn canonical_key(cwd: &Path) -> PathBuf {
    fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf())
}

pub(super) fn module_file(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("js" | "mjs" | "ts")
        )
}

pub(super) fn package_entries(root: &Path) -> Vec<PathBuf> {
    let manifest = fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    let entries = manifest
        .pointer("/jeden/extensions")
        .and_then(Value::as_array)
        .or_else(|| manifest.pointer("/pi/extensions").and_then(Value::as_array));
    entries
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|path| root.join(path))
        .filter(|path| module_file(path))
        .collect()
}

pub(super) fn scan_modules(root: &Path, recursive_children: bool) -> Vec<PathBuf> {
    if module_file(root) {
        return vec![root.to_path_buf()];
    }
    if !root.is_dir() {
        return Vec::new();
    }
    let manifest = package_entries(root);
    if !manifest.is_empty() {
        return manifest;
    }
    for name in ["index.ts", "index.js", "index.mjs"] {
        let path = root.join(name);
        if module_file(&path) {
            return vec![path];
        }
    }
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if module_file(&path) {
                files.push(path);
            } else if recursive_children && path.is_dir() {
                files.extend(scan_modules(&path, false));
            }
            if files.len() >= MAX_EXTENSION_FILES {
                break;
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

pub(super) fn declarative_paths(root: &Path, precedence: usize) -> Vec<DeclarativeCapability> {
    let mut values = Vec::new();
    for kind in ["commands", "hooks", "skills", "agents", "rules"] {
        let path = if kind == "hooks" {
            root.join("hooks.json")
        } else {
            root.join(kind)
        };
        if path.exists() {
            let health = if kind == "hooks" {
                fs::read_to_string(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|text| {
                        serde_json::from_str::<Value>(&text).map_err(|error| error.to_string())
                    })
                    .map(|value| value.is_object())
                    .and_then(|valid| {
                        if valid {
                            Ok(())
                        } else {
                            Err("hooks manifest must be an object".into())
                        }
                    })
            } else {
                fs::read_dir(&path)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            };
            values.push(DeclarativeCapability {
                kind,
                path,
                healthy: health.is_ok(),
                error: health.err(),
                precedence,
            });
        }
    }
    values
}
pub(super) fn hash_path_tree(path: &Path, hasher: &mut DefaultHasher, remaining: &mut usize) {
    if *remaining == 0 {
        return;
    }
    *remaining -= 1;
    path.hash(hasher);
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    metadata.len().hash(hasher);
    metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .hash(hasher);
    if metadata.is_dir() {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        let mut children = entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            hash_path_tree(&child, hasher, remaining);
            if *remaining == 0 {
                break;
            }
        }
    }
}

pub(super) fn read_json(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

pub(super) fn config_value(cwd: &Path, key: &str) -> Value {
    let project = read_json(&cwd.join(".jeden/config.json"));
    if let Some(value) = project.get(key) {
        return value.clone();
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| read_json(&home.join(".jeden/config.json")))
        .and_then(|config| config.get(key).cloned())
        .unwrap_or(Value::Null)
}

