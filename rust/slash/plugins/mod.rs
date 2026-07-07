use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::slash::common::{dirs_home, merged_config, now_text, read_json_value, resolve_cwd_path, split_args};
use crate::slash::SlashContext;
use crate::tools;

pub(crate) mod fetch;
pub(crate) mod marketplace;
pub(crate) mod ops;
pub(crate) mod registry;

use ops::{installed_entries_for_scope, merged_installed_values, normalize_scope, parse_marketplace_flags, registry_scope_dir};
use registry::{format_plugin, format_plugin_source, plugin_registry, plugin_registry_path, save_plugin_registry, sorted_object_values};

/// Plugin cache home. Honors `JEDEN_PLUGINS_HOME` (used to keep tests hermetic);
/// defaults to `~`.
pub(crate) fn plugins_home() -> PathBuf {
    env::var_os("JEDEN_PLUGINS_HOME").map(PathBuf::from).unwrap_or_else(dirs_home)
}
pub(crate) fn marketplace_cache_root() -> PathBuf { plugins_home().join(".jeden/plugins/cache/marketplaces") }
pub(crate) fn plugin_cache_root() -> PathBuf { plugins_home().join(".jeden/plugins/cache/plugins") }
pub(crate) fn marketplace_cache_dir(name: &str) -> PathBuf { marketplace_cache_root().join(name) }

fn is_extension_module_file(path: &Path) -> bool {
    path.is_file() && matches!(path.extension().and_then(|value| value.to_str()), Some("ts" | "js" | "mjs"))
}

fn extension_manifest_entries(dir: &Path) -> Vec<PathBuf> {
    let manifest = read_json_value(&dir.join("package.json"));
    let entries = manifest
        .pointer("/omp/extensions")
        .and_then(Value::as_array)
        .or_else(|| manifest.pointer("/pi/extensions").and_then(Value::as_array));
    let mut out = Vec::new();
    if let Some(entries) = entries {
        for entry in entries {
            let Some(raw) = entry.as_str() else { continue; };
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

fn discover_extension_module_files(root: &Path) -> Vec<PathBuf> {
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

fn native_extension_roots(cwd: &Path) -> Vec<PathBuf> {
    let mut roots = vec![cwd.join(".jeden/extensions"), cwd.join(".omp/extensions")];
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join(".jeden/extensions"));
        roots.push(home.join(".omp/agent/extensions"));
    }
    roots
}

fn configured_extension_paths(cwd: &Path) -> Vec<PathBuf> {
    merged_config(cwd)
        .get("extensions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|raw| resolve_cwd_path(cwd, raw))
        .collect()
}

fn disabled_extension_ids(cwd: &Path) -> Vec<String> {
    merged_config(cwd)
        .get("disabledExtensions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn installed_plugin_extension_files(cwd: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in [cwd.to_path_buf(), plugins_home()] {
        for entry in installed_entries_for_scope(&dir) {
            if entry.get("enabled").and_then(Value::as_bool) == Some(false) { continue; }
            if let Some(path) = entry.get("path").and_then(Value::as_str) {
                for file in discover_extension_module_files(Path::new(path)) {
                    if !files.contains(&file) {
                        files.push(file);
                    }
                }
            }
        }
    }
    files.sort();
    files
}

pub(crate) fn handle_extensions(context: &SlashContext<'_>) -> Result<String, String> {
    let registry = plugin_registry(context.cwd);
    let mut sources = sorted_object_values(&registry["sources"]);
    sources.sort_by(|a, b| a.get("name").and_then(Value::as_str).unwrap_or("").cmp(b.get("name").and_then(Value::as_str).unwrap_or("")));
    let mut installed = sorted_object_values(&registry["installed"]);
    installed.sort_by(|a, b| a.get("id").and_then(Value::as_str).unwrap_or("").cmp(b.get("id").and_then(Value::as_str).unwrap_or("")));
    let native = native_extension_roots(context.cwd)
        .into_iter()
        .flat_map(|root| discover_extension_module_files(&root))
        .map(|path| path.display().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let configured = configured_extension_paths(context.cwd)
        .into_iter()
        .flat_map(|path| discover_extension_module_files(&path))
        .map(|path| path.display().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let plugin_extensions = installed_plugin_extension_files(context.cwd)
        .into_iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let disabled = disabled_extension_ids(context.cwd);
    let mut lines = vec![
        format!("Extension registry: {}", plugin_registry_path(context.cwd).display()),
        format!("Native extension modules: {}", native.len()),
    ];
    if native.is_empty() { lines.push("- none".into()); } else { lines.extend(native.iter().map(|path| format!("- {}", path))); }
    lines.push(format!("Configured extension modules: {}", configured.len()));
    if configured.is_empty() { lines.push("- none".into()); } else { lines.extend(configured.iter().map(|path| format!("- {}", path))); }
    lines.push(format!("Disabled extension ids: {}", disabled.len()));
    if disabled.is_empty() { lines.push("- none".into()); } else { lines.extend(disabled.iter().map(|id| format!("- {}", id))); }
    lines.push(format!("Marketplace sources: {}", sources.len()));
    if sources.is_empty() { lines.push("- none".into()); } else { lines.extend(sources.iter().map(format_plugin_source)); }
    lines.push(format!("Installed plugins: {}", installed.len()));
    if installed.is_empty() { lines.push("- none".into()); } else { lines.extend(installed.iter().map(format_plugin)); }
    lines.push(format!("Installed plugin extension modules: {}", plugin_extensions.len()));
    if plugin_extensions.is_empty() { lines.push("- none".into()); } else { lines.extend(plugin_extensions.iter().map(|path| format!("- {}", path))); }
    lines.push("Runtime note: Rust currently reports JS/TS extension candidates; command/tool activation still comes from installed plugin commands, hooks, and custom tools.".into());
    Ok(lines.join("\n"))
}

fn discover_custom_tool_files(cwd: &Path) -> Vec<String> {
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
                    let ext = path.extension().and_then(|value| value.to_str()).unwrap_or("");
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

pub(crate) fn handle_reload_plugins(context: &SlashContext<'_>) -> Result<String, String> {
    let mut registry = plugin_registry(context.cwd);
    let files = discover_custom_tool_files(context.cwd);
    let loaded_tools = tools::list_tools(context.cwd)
        .into_iter()
        .map(|tool| json!({ "name": tool.name, "description": tool.description }))
        .collect::<Vec<_>>();
    registry["reload"] = json!({
        "requestedAt": now_text(),
        "customToolFiles": files,
        "loadedTools": loaded_tools,
        "checkedBy": "rust"
    });
    let file = save_plugin_registry(context.cwd, &registry)?;
    Ok([
        format!("Plugin reload scanned {} custom tool file(s).", registry["reload"]["customToolFiles"].as_array().map(Vec::len).unwrap_or_default()),
        format!("Visible Rust tool definitions: {}.", registry["reload"]["loadedTools"].as_array().map(Vec::len).unwrap_or_default()),
        format!("Reload marker: {}", file.display()),
        "The active tool registry is rebuilt on the next Jeden run; this Rust command records the reload request durably.".into(),
    ].join("\n"))
}

pub(crate) fn handle_plugins(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("list");
    if verb == "list" {
        let mut installed = merged_installed_values(context.cwd);
        installed.sort_by(|a, b| a.get("id").and_then(Value::as_str).unwrap_or("").cmp(b.get("id").and_then(Value::as_str).unwrap_or("")));
        return Ok(if installed.is_empty() { "No plugins installed. Use /marketplace discover and /marketplace install <name@marketplace>.".into() } else { ["Installed plugins:".into()].into_iter().chain(installed.iter().map(format_plugin)).collect::<Vec<_>>().join("\n") });
    }
    if verb == "enable" || verb == "disable" {
        let rest = argv.split_first().map(|(_, r)| r).unwrap_or(&[]);
        let (_force, scope, targets) = parse_marketplace_flags(rest);
        let scope = normalize_scope(scope)?;
        let target = targets.first().map(String::as_str).unwrap_or("");
        if target.is_empty() { return Err(format!("Usage: /plugins {verb} [--scope user|project] <name@marketplace>")); }
        let reg_dir = registry_scope_dir(context.cwd, &scope);
        let mut registry = plugin_registry(&reg_dir);
        let installed = registry.get_mut("installed").and_then(Value::as_object_mut).ok_or("invalid plugin registry")?;
        let plugin = installed.get_mut(target).ok_or_else(|| format!("Installed plugin not found in {scope} scope: {target}"))?;
        if !plugin.is_object() { *plugin = json!({}); }
        let plugin_obj = plugin.as_object_mut().expect("plugin object");
        plugin_obj.insert("enabled".into(), json!(verb == "enable"));
        plugin_obj.insert("scope".into(), json!(scope.clone()));
        plugin_obj.insert("updatedAt".into(), json!(now_text()));
        let file = save_plugin_registry(&reg_dir, &registry)?;
        return Ok(format!("{} plugin {} in {} scope ({}).", if verb == "enable" { "Enabled" } else { "Disabled" }, target, scope, file.display()));
    }
    Err("Usage: /plugins list | enable [--scope user|project] <name@marketplace> | disable [--scope user|project] <name@marketplace>".into())
}
