//! Finding what there is to load: which files are extension modules, which
//! declarative capabilities a root declares, and the fingerprint that says
//! whether anything under those roots has changed since the last generation.

use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::hash::{DefaultHasher, Hash};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::DeclarativeCapability;
use std::time::SystemTime;

pub(crate) mod host;
pub(crate) mod materialize;
pub(crate) mod roots;

pub(super) fn canonical_key(cwd: &Path) -> PathBuf {
    fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf())
}

/// The module formats the extension host loads, named by file extension.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ModuleExtension {
    Js,
    Mjs,
    Ts,
}

pub(crate) fn module_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                serde_json::from_value::<ModuleExtension>(Value::from(extension)).is_ok()
            })
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

pub(crate) fn scan_modules(root: &Path, recursive_children: bool) -> Vec<PathBuf> {
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
        }
    }
    files.sort();
    files.dedup();
    files
}

/// What a root may declare besides modules. Each kind is a directory of
/// files under the root, except hooks, which is one manifest.
#[derive(Clone, Copy)]
enum DeclarativeKind {
    Commands,
    Hooks,
    Skills,
    Agents,
    Rules,
}

impl DeclarativeKind {
    const EVERY: &'static [DeclarativeKind] = &[
        DeclarativeKind::Commands,
        DeclarativeKind::Hooks,
        DeclarativeKind::Skills,
        DeclarativeKind::Agents,
        DeclarativeKind::Rules,
    ];

    fn name(self) -> &'static str {
        match self {
            DeclarativeKind::Commands => "commands",
            DeclarativeKind::Hooks => "hooks",
            DeclarativeKind::Skills => "skills",
            DeclarativeKind::Agents => "agents",
            DeclarativeKind::Rules => "rules",
        }
    }

    fn path(self, root: &Path) -> PathBuf {
        match self {
            DeclarativeKind::Hooks => root.join("hooks.json"),
            _ => root.join(self.name()),
        }
    }

    fn health(self, path: &Path) -> Result<(), String> {
        match self {
            DeclarativeKind::Hooks => {
                let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
                let value =
                    serde_json::from_str::<Value>(&text).map_err(|error| error.to_string())?;
                if value.is_object() {
                    Ok(())
                } else {
                    Err("hooks manifest must be an object".into())
                }
            }
            _ => fs::read_dir(path)
                .map(|_| ())
                .map_err(|error| error.to_string()),
        }
    }
}

pub(super) fn declarative_paths(root: &Path, precedence: usize) -> Vec<DeclarativeCapability> {
    let mut values = Vec::new();
    for kind in DeclarativeKind::EVERY.iter().copied() {
        let path = kind.path(root);
        if path.exists() {
            let health = kind.health(&path);
            values.push(DeclarativeCapability {
                kind: kind.name(),
                path,
                healthy: health.is_ok(),
                error: health.err(),
                precedence,
            });
        }
    }
    values
}
pub(super) fn hash_path_tree(path: &Path, hasher: &mut DefaultHasher) {
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
            hash_path_tree(&child, hasher);
        }
    }
}

/// A JSON file the loader reads, such as a plugin registry: a file that does
/// not exist is `Value::Null`; one that cannot be read or does not parse is
/// refused naming the file, never read as empty.
pub(super) fn read_json(path: &Path) -> Result<Value, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Value::Null),
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    };
    serde_json::from_str(&text)
        .map_err(|error| format!("{} is not valid JSON: {error}", path.display()))
}

/// `key` as the configuration layers that apply here declare it, a later
/// layer (the project) overriding an earlier one (the user's files). A layer
/// that cannot be read or does not parse is refused naming the file: read as
/// empty, an invalid project configuration dropped `disabledExtensions` and
/// loaded every extension it switched off.
pub(super) fn config_value(cwd: &Path, key: &str) -> Result<Value, String> {
    let mut declared = Value::Null;
    for path in crate::cli::config::config_layer_paths(cwd) {
        let Some(layer) = crate::cli::config::read_config_layer(&path)? else {
            continue;
        };
        if let Some(value) = layer.get(key) {
            declared = value.clone();
        }
    }
    Ok(declared)
}
