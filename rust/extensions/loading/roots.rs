//! Where extensions come from: the plugin roots this installation has
//! installed, and the whole source set one working directory offers - its own
//! extension directory, the configured roots, and the installed plugins.

use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::super::{InstalledPluginRoot, SourceSet};
use super::{config_value, declarative_paths, hash_path_tree, read_json, scan_modules};
use crate::hooks::extensions::loading::package_entries;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::SystemTime;

pub(super) fn installed_plugin_roots(cwd: &Path) -> Vec<InstalledPluginRoot> {
    let home = env::var_os("JEDEN_PLUGINS_HOME")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf());
    let mut roots = Vec::new();
    for scope in [cwd.to_path_buf(), home] {
        let registry = read_json(&scope.join(".jeden/plugins.json"));
        let Some(installed) = registry.get("installed").and_then(Value::as_object) else {
            continue;
        };
        for (registry_id, value) in installed {
            let Some(path) = value.get("path").and_then(Value::as_str) else {
                continue;
            };
            roots.push(InstalledPluginRoot {
                id: value
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(registry_id)
                    .to_string(),
                version: value
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("unversioned")
                    .to_string(),
                path: PathBuf::from(path),
                enabled: value.get("enabled").and_then(Value::as_bool) != Some(false),
            });
        }
    }
    roots.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| left.path.cmp(&right.path))
    });
    roots
}

pub(in crate::hooks::extensions) fn source_set(cwd: &Path) -> Result<SourceSet, String> {
    let mut modules = Vec::new();
    let mut declarative = Vec::new();
    let installed_plugins = installed_plugin_roots(cwd);
    // Precedence rises layer by layer, a later layer overriding an earlier
    // one: installed plugins in their listed order, then the user's
    // ~/.jeden, then the workspace with the extensions its config names.
    let mut layer = installed_plugins.len()..;
    let user_rank = layer.start;
    layer.next();
    let project_rank = layer.start;
    let mut roots = vec![cwd.join(".jeden/extensions")];
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join(".jeden/extensions"));
        declarative.extend(declarative_paths(&home.join(".jeden"), user_rank));
        roots.push(home.join(".jeden/tools"));
    }
    declarative.extend(declarative_paths(&cwd.join(".jeden"), project_rank));
    roots.push(cwd.join(".jeden/tools"));
    let configured_extensions = config_value(cwd, "extensions");
    if let Some(configured) = configured_extensions.as_array() {
        for value in configured {
            if let Some(path) = value.as_str() {
                let path = PathBuf::from(path);
                roots.push(if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                });
            }
        }
    }
    for (index, plugin) in installed_plugins.iter().enumerate() {
        if !plugin.enabled {
            continue;
        }
        roots.push(plugin.path.join("extensions"));
        roots.push(plugin.path.join("tools"));
        modules.extend(package_entries(&plugin.path));
        modules.extend(scan_modules(&plugin.path, false));
        declarative.extend(declarative_paths(&plugin.path, index));
    }
    for root in roots {
        modules.extend(scan_modules(&root, true));
    }
    modules.sort();
    modules.dedup();
    let disabled_extensions = config_value(cwd, "disabledExtensions");
    let disabled: BTreeSet<String> = disabled_extensions
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    modules.retain(|path| {
        let text = path.to_string_lossy();
        !disabled
            .iter()
            .any(|id| text.ends_with(id) || text.contains(&format!("/{id}/")))
    });
    let home_jeden = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".jeden"));
    let mut module_precedence = BTreeMap::new();
    for module in &modules {
        let plugin = installed_plugins
            .iter()
            .position(|plugin| plugin.enabled && module.starts_with(&plugin.path));
        let in_user_layer = home_jeden
            .as_ref()
            .is_some_and(|root| module.starts_with(root));
        let precedence = match (plugin, in_user_layer) {
            (Some(index), _) => index,
            (None, true) => user_rank,
            (None, false) => project_rank,
        };
        module_precedence.insert(module.clone(), precedence);
    }
    let mut hasher = DefaultHasher::new();
    for path in &modules {
        path.hash(&mut hasher);
        if let Ok(metadata) = fs::metadata(path) {
            metadata.len().hash(&mut hasher);
            metadata
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .hash(&mut hasher);
        }
    }
    for plugin in &installed_plugins {
        plugin.id.hash(&mut hasher);
        plugin.version.hash(&mut hasher);
        plugin.enabled.hash(&mut hasher);
    }
    for item in &declarative {
        item.kind.hash(&mut hasher);
        item.path.hash(&mut hasher);
        item.precedence.hash(&mut hasher);
        hash_path_tree(&item.path, &mut hasher);
    }
    Ok(SourceSet {
        modules,
        declarative,
        installed_plugins,
        module_precedence,
        fingerprint: hasher.finish(),
    })
}
