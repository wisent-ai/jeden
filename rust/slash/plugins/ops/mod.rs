//! What the plugin commands do underneath: which scope a registry belongs to,
//! which marketplaces are configured, and what is actually installed.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::plugins_home;
use super::production::service::MarketplaceService;
use super::registry::{plugin_registry, save_plugin_registry};
use crate::slash::common::now_text;

mod install;

pub(crate) use install::install_one;

pub(crate) fn registry_scope_dir(cwd: &Path, scope: &str) -> PathBuf {
    if scope == "project" {
        cwd.to_path_buf()
    } else {
        plugins_home()
    }
}

/// Look up a marketplace source string by name across project then user
/// scopes; a registry that cannot be read or does not parse is refused.
pub(crate) fn find_marketplace_source(cwd: &Path, name: &str) -> Result<Option<String>, String> {
    for dir in [cwd.to_path_buf(), plugins_home()] {
        if let Some(src) = plugin_registry(&dir)?
            .get("sources")
            .and_then(Value::as_object)
            .and_then(|s| s.get(name))
            .and_then(|s| s.get("source"))
            .and_then(Value::as_str)
        {
            return Ok(Some(src.to_string()));
        }
    }
    Ok(None)
}

/// All configured marketplace sources (name -> source) across both scopes.
pub(crate) fn all_marketplace_sources(cwd: &Path) -> Result<Vec<(String, String)>, String> {
    let mut out: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for dir in [cwd.to_path_buf(), plugins_home()] {
        if let Some(map) = plugin_registry(&dir)?
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
    Ok(out.into_iter().collect())
}

/// Record the freshly fetched plugin list into whichever scope holds `name`;
/// a registry that cannot be read, parsed or written is refused naming it.
pub(crate) fn update_source_plugins(
    cwd: &Path,
    name: &str,
    plugins: &[Value],
) -> Result<(), String> {
    // A catalogue plugin is named; its description and version are its own
    // to state, kept as null when it states none.
    let summary = plugins
        .iter()
        .enumerate()
        .map(|(at, p)| {
            let name = p.get("name").and_then(Value::as_str).ok_or_else(|| {
                format!("the {name} catalogue lists plugin #{at} without a name: {p}")
            })?;
            Ok(json!({
                "name": name,
                "description": p.get("description").and_then(Value::as_str),
                "version": p.get("version").and_then(Value::as_str),
            }))
        })
        .collect::<Result<Vec<Value>, String>>()?;
    for dir in [cwd.to_path_buf(), plugins_home()] {
        let mut registry = plugin_registry(&dir)?;
        let has = registry
            .get("sources")
            .and_then(Value::as_object)
            .is_some_and(|s| s.contains_key(name));
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
            src.insert("updatedAt".into(), json!(now_text()?));
        }
        save_plugin_registry(&dir, &registry)?;
    }
    Ok(())
}

/// Merge only verified active records. Legacy `installed` entries are inventory,
/// never executable capability state.
pub(crate) fn merged_installed_values(cwd: &Path) -> Result<Vec<Value>, String> {
    let mut map = std::collections::BTreeMap::<String, Value>::new();
    for entry in installed_entries_for_scope(&plugins_home())? {
        if let Some(id) = entry.get("id").and_then(Value::as_str).map(str::to_string) {
            map.insert(id, entry);
        }
    }
    for entry in installed_entries_for_scope(cwd)? {
        if let Some(id) = entry.get("id").and_then(Value::as_str).map(str::to_string) {
            map.insert(id, entry);
        }
    }
    Ok(map.into_values().collect())
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
/// remaining positional targets. `--scope` without a value is refused, never
/// read as no scope.
pub(crate) fn parse_marketplace_flags(
    argv: &[String],
) -> Result<(bool, Option<String>, Vec<String>), String> {
    let mut force = false;
    let mut scope = None;
    let mut rest = Vec::new();
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--force" | "-f" => force = true,
            "--scope" => match it.next() {
                Some(named) => scope = Some(named.clone()),
                None => return Err("--scope takes user or project".into()),
            },
            s if s.starts_with("--scope=") => {
                scope = Some(s.trim_start_matches("--scope=").to_string())
            }
            other => rest.push(other.to_string()),
        }
    }
    Ok((force, scope, rest))
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

/// The active packages of the scope at `dir`; a package registry that
/// cannot be read is refused naming the scope, never read as none installed.
/// A package is enabled unless `/plugins disable` turned it off: the toggle
/// is kept in the scope's plugin registry, which is read here too, so a
/// disabled plugin's commands and hooks are not loaded.
pub(crate) fn installed_entries_for_scope(dir: &Path) -> Result<Vec<Value>, String> {
    let active = production_service(dir).active_packages().map_err(|cause| {
        format!(
            "the installed plugins of {} cannot be read: {cause}",
            dir.display()
        )
    })?;
    let toggles = plugin_registry(dir)?;
    Ok(active
        .into_iter()
        .map(|record| {
            let enabled = toggles["installed"][record.id.as_str()]["enabled"] != json!(false);
            json!({
                "id": record.id,
                "name": record.id,
                "version": record.version,
                "path": record.path,
                "source": record.trust,
                "state": "active",
                "enabled": enabled,
                "generation": record.generation,
            })
        })
        .collect())
}

/// Command directories contributed by ENABLED installed plugins, across project
/// then user scope. Appended after the project/user `.jeden/commands` dirs so
/// local commands win.
pub(crate) fn installed_plugin_command_dirs(cwd: &Path) -> Result<Vec<PathBuf>, String> {
    let mut dirs = Vec::new();
    for dir in [cwd.to_path_buf(), plugins_home()] {
        for entry in installed_entries_for_scope(&dir)? {
            if entry.get("enabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            if let Some(path) = entry.get("path").and_then(Value::as_str) {
                let commands = Path::new(path).join("commands");
                // A plugin's commands directory that cannot be examined is
                // refused: read as absent, its commands vanished from the list.
                if crate::tool_runtime::runtime_ops::platform::presence::present(&commands)?
                    && commands.is_dir()
                    && !dirs.contains(&commands)
                {
                    dirs.push(commands);
                }
            }
        }
    }
    for dir in crate::hooks::extension_command_dirs(cwd)? {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    Ok(dirs)
}

/// Parsed `hooks.json` configs from ENABLED installed plugins, each one or the
/// reason it could not be read. User-scope plugin hooks always apply;
/// project-scope plugin hooks only when `allow_project`. A plugin without a
/// `hooks.json` declares none; one whose file cannot be read or is not JSON
/// is answered with its path and cause instead of silently declaring none.
pub(crate) fn installed_plugin_hook_configs(
    cwd: &Path,
    allow_project: bool,
) -> Vec<Result<Value, String>> {
    let mut configs = Vec::new();
    for (dir, include) in [(cwd.to_path_buf(), allow_project), (plugins_home(), true)] {
        if !include {
            continue;
        }
        let entries = match installed_entries_for_scope(&dir) {
            Ok(entries) => entries,
            Err(cause) => {
                // The scope's plugins declare hooks this run cannot read:
                // answered as a config failure, never as no hooks.
                configs.push(Err(cause));
                continue;
            }
        };
        for entry in entries {
            if entry.get("enabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            if let Some(path) = entry.get("path").and_then(Value::as_str) {
                let hooks_path = Path::new(path).join("hooks.json");
                match crate::hooks::read_config(&hooks_path) {
                    Ok(Value::Null) => {}
                    loaded => configs.push(loaded),
                }
            }
        }
    }
    configs
}
