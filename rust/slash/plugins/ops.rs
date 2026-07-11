use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use super::fetch::{
    catalog_find_plugin, copy_dir_recursive, fetch_marketplace, materialize_plugin,
    plugin_manifest_version, read_marketplace_catalog,
};
use super::marketplace::sanitize_marketplace_name;
use super::registry::{plugin_registry, save_plugin_registry};
use super::{marketplace_cache_dir, plugin_cache_root, plugins_home};
use crate::slash::common::now_text;
use crate::slash::validate::{valid_marketplace_name, valid_plugin_id, valid_plugin_name};

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

/// Merged installed-plugin records across scopes. Enabled project records shadow
/// user records; disabled project records do not hide an enabled user install.
pub(crate) fn merged_installed_values(cwd: &Path) -> Vec<Value> {
    let mut map: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    if let Some(installed) = plugin_registry(&plugins_home())
        .get("installed")
        .and_then(Value::as_object)
    {
        for (id, entry) in installed {
            map.insert(id.clone(), entry.clone());
        }
    }
    if let Some(installed) = plugin_registry(cwd)
        .get("installed")
        .and_then(Value::as_object)
    {
        for (id, entry) in installed {
            if entry.get("enabled").and_then(Value::as_bool) == Some(false) && map.contains_key(id)
            {
                continue;
            }
            map.insert(id.clone(), entry.clone());
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

/// Resolve, materialize, activate and record one plugin. Returns a report line.
pub(crate) fn install_one(
    cwd: &Path,
    mkt_name: &str,
    plugin_name: &str,
    scope: &str,
    force: bool,
) -> Result<String, String> {
    if !valid_marketplace_name(mkt_name) {
        return Err(format!("Invalid marketplace name: {mkt_name}"));
    }
    if !valid_plugin_name(plugin_name) {
        return Err(format!("Invalid plugin name: {plugin_name}"));
    }
    let id = format!("{plugin_name}@{mkt_name}");
    if !valid_plugin_id(&id) {
        return Err(format!("Invalid plugin id: {id}"));
    }
    let source = find_marketplace_source(cwd, mkt_name).ok_or_else(|| {
        format!("Marketplace source not found: {mkt_name}. Add it with /marketplace add <source>.")
    })?;
    let mkt_cache = marketplace_cache_dir(mkt_name);
    if !mkt_cache.exists() {
        fetch_marketplace(cwd, mkt_name, &source)?;
    }
    let catalog = read_marketplace_catalog(&mkt_cache)?;
    let entry = catalog_find_plugin(&catalog, plugin_name)
        .ok_or_else(|| format!("Plugin {plugin_name} not found in marketplace {mkt_name}."))?;
    let mat = materialize_plugin(&mkt_cache, &catalog, &entry)?;
    let version = entry
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| plugin_manifest_version(&mat.staging))
        .or_else(|| mat.sha.clone())
        .unwrap_or_else(|| "unversioned".into());
    let final_dir = plugin_cache_root().join(format!(
        "{}___{}___{}",
        mkt_name,
        plugin_name,
        sanitize_marketplace_name(&version)
    ));
    if final_dir.exists() {
        if force {
            fs::remove_dir_all(&final_dir).map_err(|e| e.to_string())?;
        } else {
            let _ = fs::remove_dir_all(&mat.staging);
            return Err(format!(
                "{id} is already installed (version {version}). Use --force to reinstall."
            ));
        }
    }
    if let Some(parent) = final_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if fs::rename(&mat.staging, &final_dir).is_err() {
        copy_dir_recursive(&mat.staging, &final_dir)?;
        let _ = fs::remove_dir_all(&mat.staging);
    }
    let commands_dir = final_dir.join("commands");
    let has_commands = commands_dir.is_dir();
    let command_files: Vec<PathBuf> = if has_commands {
        fs::read_dir(&commands_dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let command_count = command_files.len();
    let has_hooks = final_dir.join("hooks.json").is_file();
    let reg_dir = registry_scope_dir(cwd, scope);
    let mut registry = plugin_registry(&reg_dir);
    let record = json!({
        "id": id,
        "name": plugin_name,
        "marketplace": mkt_name,
        "version": version,
        "source": mat.source_desc,
        "path": final_dir.to_string_lossy(),
        "commands": has_commands,
        "commandCount": command_count,
        "hooks": has_hooks,
        "scope": scope,
        "enabled": true,
        "installedAt": now_text(),
        "updatedAt": now_text(),
    });
    registry
        .get_mut("installed")
        .and_then(Value::as_object_mut)
        .ok_or("invalid plugin registry")?
        .insert(id.clone(), record);
    save_plugin_registry(&reg_dir, &registry)?;
    let plural = match command_files.as_slice() {
        [_] => "",
        _ => "s",
    };
    Ok(format!(
        "installed {id} ({command_count} command{plural}, hooks: {}) [scope: {scope}, {}]",
        if has_hooks { "yes" } else { "no" },
        final_dir.display(),
    ))
}

pub(crate) fn installed_entries_for_scope(dir: &Path) -> Vec<Value> {
    plugin_registry(dir)
        .get("installed")
        .and_then(Value::as_object)
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default()
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
