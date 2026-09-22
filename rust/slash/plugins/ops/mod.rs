//! What the plugin commands do underneath: which scope a registry belongs to,
//! which marketplaces are configured, and what is actually installed.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::production::service::MarketplaceService;
use super::registry::{plugin_registry, save_plugin_registry};
use super::plugins_home;
use crate::slash::common::now_text;

mod install;

pub(crate) use install::install_one;
use std::fs;

pub(crate) fn registry_scope_dir(cwd: &Path, scope: &str) -> PathBuf {
    if scope == "project" {
        cwd.to_path_buf()
    } else {
        plugins_home()
    }
}

/// Look up a marketplace source string by name across project then user scopes.
pub(crate) fn find_marketplace_source(cwd: &Path, name: &str) -> Option<String> {
    for dir in [cwd.to_path_buf(), plugins_home()] {
        if let Some(src) = plugin_registry(&dir)
            .get("sources")
            .and_then(Value::as_object)
            .and_then(|s| s.get(name))
            .and_then(|s| s.get("source"))
            .and_then(Value::as_str)
        {
            return Some(src.to_string());
        }
    }
    None
}

/// All configured marketplace sources (name -> source) across both scopes.
pub(crate) fn all_marketplace_sources(cwd: &Path) -> Vec<(String, String)> {
    let mut out: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for dir in [cwd.to_path_buf(), plugins_home()] {
        if let Some(map) = plugin_registry(&dir)
            .get("sources")
            .and_then(Value::as_object)
        {
            for (name, value) in map {
                if let Some(src) = value.get("source").and_then(Value::as_str) {
                    out.entry(name.clone()).or_insert_with(|| src.to_string());
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Record the freshly fetched plugin list into whichever scope holds `name`.
pub(crate) fn update_source_plugins(cwd: &Path, name: &str, plugins: &[Value]) {
    let summary: Vec<Value> = plugins
        .iter()
        .map(|p| {
            json!({
                "name": p.get("name").and_then(Value::as_str).unwrap_or(""),
                "description": p.get("description").and_then(Value::as_str).unwrap_or(""),
                "version": p.get("version").and_then(Value::as_str).unwrap_or(""),
            })
        })
        .collect();
    for dir in [cwd.to_path_buf(), plugins_home()] {
        let mut registry = plugin_registry(&dir);
        let has = registry
            .get("sources")
            .and_then(Value::as_object)
            .map(|s| s.contains_key(name))
            .unwrap_or(false);
        if !has {
            continue;
        }
        if let Some(src) = registry
            .get_mut("sources")
            .and_then(Value::as_object_mut)
            .and_then(|s| s.get_mut(name))
            .and_then(Value::as_object_mut)
        {
            src.insert("plugins".into(), json!(summary));
            src.insert("updatedAt".into(), json!(now_text()));
        }
        let _ = save_plugin_registry(&dir, &registry);
    }
}

/// Merge only verified active records. Legacy `installed` entries are inventory,
/// never executable capability state.
pub(crate) fn merged_installed_values(cwd: &Path) -> Vec<Value> {
    let mut map = std::collections::BTreeMap::<String, Value>::new();
    for entry in installed_entries_for_scope(&plugins_home()) {
        if let Some(id) = entry.get("id").and_then(Value::as_str).map(str::to_string) {
            map.insert(id, entry);
        }
    }
    for entry in installed_entries_for_scope(cwd) {
        if let Some(id) = entry.get("id").and_then(Value::as_str).map(str::to_string) {
            map.insert(id, entry);
        }
    }
    map.into_values().collect()
}

pub(crate) fn split_plugin_id(id: &str) -> Result<(String, String), String> {
    match id.split_once('@') {
        Some((plugin, mkt)) if !plugin.is_empty() && !mkt.is_empty() => {
            Ok((plugin.to_string(), mkt.to_string()))
        }
        _ => Err(format!("Expected name@marketplace, got: {id}")),
    }
}

/// Parse `install`/`upgrade` flags: `--force`, `--scope <user|project>`, and the
/// remaining positional targets.
pub(crate) fn parse_marketplace_flags(argv: &[String]) -> (bool, Option<String>, Vec<String>) {
    let mut force = false;
    let mut scope = None;
    let mut rest = Vec::new();
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--force" | "-f" => force = true,
            "--scope" => scope = it.next().cloned(),
            s if s.starts_with("--scope=") => {
                scope = Some(s.trim_start_matches("--scope=").to_string())
            }
            other => rest.push(other.to_string()),
        }
    }
    (force, scope, rest)
}

pub(crate) fn normalize_scope(scope: Option<String>) -> Result<String, String> {
    match scope.as_deref() {
        None | Some("user") => Ok("user".into()),
        Some("project") => Ok("project".into()),
        Some(other) => Err(format!("Invalid scope: {other}. Use user or project.")),
    }
}

pub(crate) fn production_service(scope_dir: &Path) -> MarketplaceService {
    MarketplaceService::new(scope_dir.join(".jeden/plugins/v2"))
}


pub(crate) fn installed_entries_for_scope(dir: &Path) -> Vec<Value> {
    production_service(dir)
        .active_packages()
        .unwrap_or_default()
        .into_iter()
        .map(|record| {
            json!({
                "id": record.id,
                "name": record.id,
                "version": record.version,
                "path": record.path,
                "source": record.trust,
                "state": "active",
                "enabled": true,
                "generation": record.generation,
            })
        })
        .collect()
}

/// Command directories contributed by ENABLED installed plugins, across project
/// then user scope. Appended after the project/user `.jeden/commands` dirs so
/// local commands win.
pub(crate) fn installed_plugin_command_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for dir in [cwd.to_path_buf(), plugins_home()] {
        for entry in installed_entries_for_scope(&dir) {
            if entry.get("enabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            if let Some(path) = entry.get("path").and_then(Value::as_str) {
                let commands = Path::new(path).join("commands");
                if commands.is_dir() && !dirs.contains(&commands) {
                    dirs.push(commands);
                }
            }
        }
    }
    if let Ok(extension_dirs) = crate::hooks::extension_command_dirs(cwd) {
        for dir in extension_dirs {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    dirs
}

/// Parsed `hooks.json` configs from ENABLED installed plugins. User-scope plugin
/// hooks always apply; project-scope plugin hooks only when `allow_project`.
pub(crate) fn installed_plugin_hook_configs(cwd: &Path, allow_project: bool) -> Vec<Value> {
    let mut configs = Vec::new();
    for (dir, include) in [(cwd.to_path_buf(), allow_project), (plugins_home(), true)] {
        if !include {
            continue;
        }
        for entry in installed_entries_for_scope(&dir) {
            if entry.get("enabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            if let Some(path) = entry.get("path").and_then(Value::as_str) {
                let hooks_path = Path::new(path).join("hooks.json");
                if hooks_path.is_file() {
                    if let Ok(text) = fs::read_to_string(&hooks_path) {
                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                            configs.push(value);
                        }
                    }
                }
            }
        }
    }
    configs
}
