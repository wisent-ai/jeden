use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::slash::common::{dirs_home, now_millis, read_json_value};
use crate::slash::validate::{git_arg_safe, valid_marketplace_name};
use super::{marketplace_cache_dir, plugin_cache_root};

pub(crate) enum FetchKind { Local, Github, GitUrl, JsonUrl }

/// Classify a marketplace source string for fetching: local path, `owner/repo`
/// github shorthand, a direct `*.json` catalog URL, or a generic git URL.
pub(crate) fn classify_fetch(source: &str) -> FetchKind {
    let text = source.trim();
    let lower = text.to_ascii_lowercase();
    if text.starts_with("./") || text.starts_with("../") || text.starts_with("~/") || text.starts_with('/') {
        return FetchKind::Local;
    }
    if (lower.starts_with("http://") || lower.starts_with("https://")) && lower.ends_with(".json") {
        return FetchKind::JsonUrl;
    }
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("git@")
        || lower.starts_with("ssh://") || lower.starts_with("git+ssh://") {
        return FetchKind::GitUrl;
    }
    if text.contains('/') && !text.contains(':') { return FetchKind::Github; }
    FetchKind::Local
}

/// Split an optional `#ref` suffix off a source string.
pub(crate) fn split_ref(source: &str) -> (String, Option<String>) {
    match source.split_once('#') {
        Some((base, git_ref)) if !git_ref.trim().is_empty() => (base.trim().to_string(), Some(git_ref.trim().to_string())),
        _ => (source.trim().to_string(), None),
    }
}

fn expand_local_path(cwd: &Path, source: &str) -> PathBuf {
    let text = source.trim();
    if let Some(rest) = text.strip_prefix("~/") { return dirs_home().join(rest); }
    let path = Path::new(text);
    if path.is_absolute() { path.to_path_buf() } else { cwd.join(path) }
}

/// Recursively copy `src` into `dst` (skipping any `.git` directory).
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if name == ".git" { continue; }
        let from = entry.path();
        let to = dst.join(&name);
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_symlink() {
            match fs::metadata(&from) {
                Ok(meta) if meta.is_dir() => copy_dir_recursive(&from, &to)?,
                Ok(_) => { fs::copy(&from, &to).map_err(|e| e.to_string())?; }
                Err(_) => {} // dangling symlink: skip
            }
        } else {
            fs::copy(&from, &to).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub(crate) fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(args).stdin(Stdio::null()).env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never");
    if let Some(dir) = cwd { command.current_dir(dir); }
    let output = command.output().map_err(|e| format!("git failed to start: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()))
    }
}

pub(crate) fn git_clone(url: &str, git_ref: Option<&str>, dest: &Path) -> Result<(), String> {
    if !git_arg_safe(url) { return Err(format!("Unsafe git URL rejected: {url}")); }
    if let Some(git_ref) = git_ref {
        if !git_arg_safe(git_ref) { return Err(format!("Unsafe git ref rejected: {git_ref}")); }
    }
    if dest.exists() { fs::remove_dir_all(dest).map_err(|e| e.to_string())?; }
    if let Some(parent) = dest.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let dest_str = dest.to_string_lossy().to_string();
    let mut args: Vec<&str> = vec!["clone", "--depth", "1"];
    if let Some(git_ref) = git_ref { args.push("--branch"); args.push(git_ref); }
    args.push(url);
    args.push(&dest_str);
    run_git(&args, None).map(|_| ())
}

/// Fetch one HTTP(S) text body. No client-side deadline is configured.
fn http_get_text(url: &str) -> Result<String, String> {
    let response = reqwest::blocking::get(url).map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() { return Err(format!("GET {url} failed: {status}")); }
    response.text().map_err(|e| e.to_string())
}

/// Fetch (or re-fetch) a marketplace source into its cache dir and return it.
pub(crate) fn fetch_marketplace(cwd: &Path, name: &str, source: &str) -> Result<PathBuf, String> {
    if !valid_marketplace_name(name) { return Err(format!("Invalid marketplace name: {name}")); }
    let dest = marketplace_cache_dir(name);
    match classify_fetch(source) {
        FetchKind::Local => {
            let src = expand_local_path(cwd, source);
            if !src.is_dir() { return Err(format!("Local marketplace path not found: {}", src.display())); }
            if dest.exists() { fs::remove_dir_all(&dest).map_err(|e| e.to_string())?; }
            copy_dir_recursive(&src, &dest)?;
        }
        FetchKind::JsonUrl => {
            let body = http_get_text(source)?;
            serde_json::from_str::<Value>(&body).map_err(|e| format!("catalog is not valid JSON: {e}"))?;
            if dest.exists() { fs::remove_dir_all(&dest).map_err(|e| e.to_string())?; }
            fs::create_dir_all(dest.join(".omp-plugin")).map_err(|e| e.to_string())?;
            fs::write(dest.join(".omp-plugin/marketplace.json"), body).map_err(|e| e.to_string())?;
        }
        FetchKind::Github => {
            let (repo, git_ref) = split_ref(source);
            let url = format!("https://github.com/{}.git", repo.trim_end_matches(".git"));
            git_clone(&url, git_ref.as_deref(), &dest)?;
        }
        FetchKind::GitUrl => {
            let (url, git_ref) = split_ref(source);
            git_clone(&url, git_ref.as_deref(), &dest)?;
        }
    }
    Ok(dest)
}

/// Read a marketplace catalog, preferring `.omp-plugin` then `.claude-plugin`.
pub(crate) fn read_marketplace_catalog(cache_dir: &Path) -> Result<Value, String> {
    for rel in [".omp-plugin/marketplace.json", ".claude-plugin/marketplace.json"] {
        let path = cache_dir.join(rel);
        if path.is_file() {
            let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            return serde_json::from_str(&text).map_err(|e| format!("{}: invalid JSON: {e}", path.display()));
        }
    }
    Err(format!("No marketplace.json in {} (.omp-plugin or .claude-plugin)", cache_dir.display()))
}

pub(crate) fn catalog_plugin_root(catalog: &Value) -> String {
    catalog.get("metadata").and_then(|m| m.get("pluginRoot")).and_then(Value::as_str).unwrap_or("").trim().trim_matches('/').to_string()
}
pub(crate) fn catalog_plugins(catalog: &Value) -> Vec<Value> {
    catalog.get("plugins").and_then(Value::as_array).cloned().unwrap_or_default()
}
pub(crate) fn catalog_find_plugin(catalog: &Value, name: &str) -> Option<Value> {
    catalog_plugins(catalog).into_iter().find(|p| p.get("name").and_then(Value::as_str) == Some(name))
}

/// Resolve a relative (`./…`) plugin source inside a marketplace cache, applying
/// `plugin_root` and rejecting path traversal outside the repo root.
pub(crate) fn resolve_relative_plugin_path(mkt_cache: &Path, plugin_root: &str, source: &str) -> Result<PathBuf, String> {
    let rel = source.trim();
    let stripped = rel.strip_prefix("./").ok_or_else(|| format!("relative plugin source must start with ./: {rel}"))?;
    if stripped.is_empty() { return Err("empty relative plugin source".into()); }
    for comp in stripped.split('/') {
        if comp.is_empty() || comp == "." || comp == ".." {
            return Err(format!("path traversal rejected in plugin source: {rel}"));
        }
    }
    let mut path = mkt_cache.to_path_buf();
    let root = plugin_root.trim().trim_matches('/');
    if !root.is_empty() {
        for comp in root.split('/') {
            if comp.is_empty() || comp == "." || comp == ".." { return Err(format!("invalid pluginRoot: {plugin_root}")); }
            path.push(comp);
        }
    }
    for comp in stripped.split('/') { path.push(comp); }
    Ok(path)
}

pub(crate) fn plugin_manifest_version(plugin_dir: &Path) -> Option<String> {
    read_json_value(&plugin_dir.join("package.json")).get("version").and_then(Value::as_str).map(str::to_string)
        .or_else(|| read_json_value(&plugin_dir.join(".claude-plugin/plugin.json")).get("version").and_then(Value::as_str).map(str::to_string))
}

pub(crate) struct Materialized { pub(crate) staging: PathBuf, pub(crate) source_desc: String, pub(crate) sha: Option<String> }

/// Materialize a plugin's directory (from its catalog `source`) into a staging
/// dir under the plugin cache. Caller renames it to the final versioned dir.
pub(crate) fn materialize_plugin(mkt_cache: &Path, catalog: &Value, entry: &Value) -> Result<Materialized, String> {
    let staging = plugin_cache_root().join(format!("staging-{}", now_millis()));
    if staging.exists() { let _ = fs::remove_dir_all(&staging); }
    if let Some(parent) = staging.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let plugin_root = catalog_plugin_root(catalog);
    let source = entry.get("source").cloned().unwrap_or(Value::Null);
    let result = (|| -> Result<Materialized, String> {
        match &source {
            Value::String(rel) => {
                let src = resolve_relative_plugin_path(mkt_cache, &plugin_root, rel)?;
                if !src.is_dir() { return Err(format!("plugin path not found in marketplace: {}", src.display())); }
                copy_dir_recursive(&src, &staging)?;
                Ok(Materialized { staging: staging.clone(), source_desc: rel.clone(), sha: None })
            }
            Value::Object(map) => {
                let kind = map.get("source").and_then(Value::as_str).unwrap_or("");
                match kind {
                    "github" => {
                        let repo = map.get("repo").and_then(Value::as_str).ok_or("github plugin source missing repo")?;
                        let git_ref = map.get("ref").and_then(Value::as_str);
                        let url = format!("https://github.com/{}.git", repo.trim_end_matches(".git"));
                        git_clone(&url, git_ref, &staging)?;
                        let sha = run_git(&["rev-parse", "HEAD"], Some(&staging)).ok();
                        let _ = fs::remove_dir_all(staging.join(".git"));
                        Ok(Materialized { staging: staging.clone(), source_desc: format!("github:{repo}"), sha })
                    }
                    "url" => {
                        let url = map.get("url").and_then(Value::as_str).ok_or("url plugin source missing url")?;
                        let git_ref = map.get("ref").and_then(Value::as_str);
                        git_clone(url, git_ref, &staging)?;
                        let sha = run_git(&["rev-parse", "HEAD"], Some(&staging)).ok();
                        let _ = fs::remove_dir_all(staging.join(".git"));
                        Ok(Materialized { staging: staging.clone(), source_desc: format!("url:{url}"), sha })
                    }
                    "git-subdir" => {
                        let url = map.get("url").and_then(Value::as_str).ok_or("git-subdir plugin source missing url")?;
                        let subpath = map.get("path").and_then(Value::as_str).ok_or("git-subdir plugin source missing path")?;
                        let git_ref = map.get("ref").and_then(Value::as_str);
                        let tmp = plugin_cache_root().join(format!("clone-{}", now_millis()));
                        git_clone(url, git_ref, &tmp)?;
                        let sha = run_git(&["rev-parse", "HEAD"], Some(&tmp)).ok();
                        let normalized = format!("./{}", subpath.trim_start_matches("./").trim_start_matches('/'));
                        let sub = match resolve_relative_plugin_path(&tmp, "", &normalized) {
                            Ok(sub) => sub,
                            Err(e) => { let _ = fs::remove_dir_all(&tmp); return Err(e); }
                        };
                        if !sub.is_dir() { let _ = fs::remove_dir_all(&tmp); return Err(format!("git-subdir path not found: {subpath}")); }
                        copy_dir_recursive(&sub, &staging)?;
                        let _ = fs::remove_dir_all(&tmp);
                        Ok(Materialized { staging: staging.clone(), source_desc: format!("git-subdir:{url}#{subpath}"), sha })
                    }
                    "npm" => Err("npm plugin sources are not yet supported".into()),
                    other => Err(format!("unknown plugin source type: {other}")),
                }
            }
            _ => Err("plugin entry missing source".into()),
        }
    })();
    if result.is_err() { let _ = fs::remove_dir_all(&staging); }
    result
}
