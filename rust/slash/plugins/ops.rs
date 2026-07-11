use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use super::fetch::{fetch_marketplace, read_marketplace_catalog};
use super::production::manifest::{MarketplaceEnvelopeV1, PluginDependency};
use super::production::service::MarketplaceService;
use super::production::trust::TrustRootV1;
use super::registry::{plugin_registry, save_plugin_registry};
use super::{marketplace_cache_dir, plugins_home};
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

fn artifact_bytes(cache: &Path, location: &str) -> Result<Vec<u8>, String> {
    if let Some(path) = location.strip_prefix("file://") {
        return fs::read(path).map_err(|error| error.to_string());
    }
    if location.starts_with("https://") {
        let response = reqwest::blocking::get(location).map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "artifact download failed with {}",
                response.status()
            ));
        }
        return response
            .bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(|error| error.to_string());
    }
    let path = cache.join(location);
    let canonical_cache = cache.canonicalize().map_err(|error| error.to_string())?;
    let canonical_path = path.canonicalize().map_err(|error| error.to_string())?;
    if !canonical_path.starts_with(canonical_cache) {
        return Err("artifact path escapes verified catalog cache".into());
    }
    fs::read(canonical_path).map_err(|error| error.to_string())
}

/// Resolve and transactionally activate a signed production catalog. Unsigned
/// legacy catalogs are deliberately rejected; local development uses dev-link.
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
        format!("Marketplace source not found: {mkt_name}. Add a signed source first.")
    })?;
    let cache = marketplace_cache_dir(mkt_name);
    if !cache.exists() {
        fetch_marketplace(cwd, mkt_name, &source)?;
    }
    let envelope: MarketplaceEnvelopeV1 = serde_json::from_value(read_marketplace_catalog(&cache)?).map_err(|error| format!("marketplace catalog is not a signed MarketplaceEnvelopeV1: {error}; use explicit dev-link for local development"))?;
    let scope_dir = registry_scope_dir(cwd, scope);
    let trust_path = scope_dir.join(".jeden/marketplace-trust-root.json");
    let trust: TrustRootV1 = serde_json::from_slice(&fs::read(&trust_path).map_err(|error| {
        format!(
            "cannot load marketplace trust root {}: {error}",
            trust_path.display()
        )
    })?)
    .map_err(|error| format!("invalid marketplace trust root: {error}"))?;
    let service = production_service(&scope_dir);
    if !force
        && service
            .active_packages()?
            .iter()
            .any(|record| record.id == plugin_name)
    {
        return Err(format!(
            "{id} is already active; use --force to replace it transactionally"
        ));
    }
    let requested = [PluginDependency {
        id: plugin_name.into(),
        requirement: "*".into(),
        features: Default::default(),
        optional: false,
    }];
    let previous = service
        .registry()?
        .catalog_sequence
        .checked_sub(0)
        .filter(|sequence| *sequence > 0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let active = service.install_and_activate(
        &trust,
        &envelope,
        previous,
        now,
        &requested,
        std::env::consts::OS,
        |location| artifact_bytes(&cache, location),
    )?;
    let record = active
        .packages
        .get(plugin_name)
        .ok_or_else(|| format!("resolved activation omitted requested plugin {plugin_name}"))?;
    Ok(format!(
        "activated signed {id} version {} at generation {} [scope: {scope}, digest {}]",
        record.version, active.generation, record.digest
    ))
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
