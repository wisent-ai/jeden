//! Where extensions come from: the plugin roots this installation has
//! installed, and the whole source set one working directory offers - its own
//! extension directory, the configured roots, and the installed plugins.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use super::super::{InstalledPluginRoot, SourceSet};
use super::{config_value, declarative_paths, hash_path_tree, scan_modules};
use crate::hooks::extensions::loading::package_entries;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};

/// The installed plugins of the working directory's scope and the user's,
/// read from the installed-package registry every plugin reader shares
/// (`slash::installed_entries_for_scope`): its record says where each
/// package is and which version, and the scope's toggle whether it is
/// enabled. A record that misses its id, version or path is refused
/// naming it, never loaded under a made-up one.
pub(super) fn installed_plugin_roots(cwd: &Path) -> Result<Vec<InstalledPluginRoot>, String> {
    let mut roots = Vec::new();
    for scope in [cwd.to_path_buf(), crate::slash::plugins_home()] {
        for entry in crate::slash::installed_entries_for_scope(&scope)? {
            let field = |name: &str| {
                entry.get(name).and_then(Value::as_str).map(str::to_string).ok_or_else(|| {
                    format!(
                        "an installed plugin of {} records no {name}, so it cannot be loaded: {entry}",
                        scope.display()
                    )
                })
            };
            roots.push(InstalledPluginRoot {
                id: field("id")?,
                version: field("version")?,
                path: PathBuf::from(field("path")?),
                enabled: entry.get("enabled").and_then(Value::as_bool) != Some(false),
            });
        }
    }
    roots.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(roots)
}

pub(in crate::hooks::extensions) fn source_set(cwd: &Path) -> Result<SourceSet, String> {
    let mut modules = Vec::new();
    let mut declarative = Vec::new();
    let installed_plugins = installed_plugin_roots(cwd)?;
    // Precedence rises layer by layer, a later layer overriding an earlier
    // one: installed plugins in their listed order, then the user's
    // ~/.jeden, then the workspace with the extensions its config names.
    let mut layer = installed_plugins.len()..;
    let user_rank = layer.start;
    layer.next();
    let project_rank = layer.start;
    let mut roots = vec![cwd.join(".jeden/extensions")];
    // The user layer under the one home lookup: skipped when HOME was unset,
    // every user extension and tool vanished from the session.
    let home_jeden = crate::slash::common::dirs_home().join(".jeden");
    roots.push(home_jeden.join("extensions"));
    declarative.extend(declarative_paths(&home_jeden, user_rank));
    roots.push(home_jeden.join("tools"));
    declarative.extend(declarative_paths(&cwd.join(".jeden"), project_rank));
    roots.push(cwd.join(".jeden/tools"));
    let configured_extensions = config_value(cwd, "extensions")?;
    for path in crate::cli::config::config_texts(&configured_extensions, "extensions")? {
        let path = PathBuf::from(path);
        roots.push(if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        });
    }
    for (index, plugin) in installed_plugins.iter().enumerate() {
        if !plugin.enabled {
            continue;
        }
        roots.push(plugin.path.join("extensions"));
        roots.push(plugin.path.join("tools"));
        modules.extend(package_entries(&plugin.path)?);
        modules.extend(scan_modules(&plugin.path, false)?);
        declarative.extend(declarative_paths(&plugin.path, index));
    }
    for root in roots {
        modules.extend(scan_modules(&root, true)?);
    }
    modules.sort();
    modules.dedup();
    let disabled_extensions = config_value(cwd, "disabledExtensions")?;
    let disabled: BTreeSet<String> =
        crate::cli::config::config_texts(&disabled_extensions, "disabledExtensions")?
            .into_iter()
            .map(str::to_string)
            .collect();
    modules.retain(|path| {
        let text = path.to_string_lossy();
        !disabled
            .iter()
            .any(|id| text.ends_with(id) || text.contains(&format!("/{id}/")))
    });
    let mut module_precedence = BTreeMap::new();
    for module in &modules {
        let plugin = installed_plugins
            .iter()
            .position(|plugin| plugin.enabled && module.starts_with(&plugin.path));
        let in_user_layer = module.starts_with(&home_jeden);
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
        let metadata = fs::metadata(path).map_err(|error| {
            format!(
                "extension module {} cannot be examined: {error}",
                path.display()
            )
        })?;
        metadata.len().hash(&mut hasher);
        let modified = metadata.modified().map_err(|error| {
            format!(
                "extension module {} has no readable modification time: {error}",
                path.display()
            )
        })?;
        modified.hash(&mut hasher);
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
