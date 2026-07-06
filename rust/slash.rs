use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};
use serde_json::Map;
use url::Url;

use crate::tools;

#[derive(Debug, Clone)]
pub struct SlashContext<'a> {
    pub cwd: &'a Path,
    pub model: Option<&'a str>,
    pub session_root: &'a Path,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ModeState {
    #[serde(default)]
    plan: PlanState,
    #[serde(default)]
    goal: GoalState,
    #[serde(rename = "guidedGoal", default)]
    guided_goal: GuidedGoalState,
    #[serde(default)]
    loop_mode: LoopState,
    #[serde(default)]
    fast: FastState,
    #[serde(default)]
    advisor: AdvisorState,
    #[serde(default)]
    force: Option<ForceState>,
    #[serde(rename = "lastFailedTask", default)]
    last_failed_task: String,
    #[serde(rename = "lastTask", default)]
    last_task: String,
    #[serde(default)]
    compact: bool,
    #[serde(default)]
    shake: String,
    #[serde(default)]
    todos: Vec<TodoState>,
    #[serde(default)]
    branches: Vec<BranchState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PlanState {
    #[serde(default)]
    enabled: bool,
    #[serde(rename = "latestPlan", default)]
    latest_plan: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct GuidedGoalState {
    #[serde(default)]
    active: bool,
    #[serde(rename = "roughObjective", default)]
    rough_objective: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct GoalState {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    objective: String,
    #[serde(default)]
    budget: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LoopState {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    remaining: Option<u64>,
    #[serde(default)]
    until: Option<u64>,
    #[serde(default)]
    prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FastState {
    #[serde(default)]
    enabled: bool,
    #[serde(rename = "serviceTier", default = "default_service_tier")]
    service_tier: String,
}

impl Default for FastState {
    fn default() -> Self { Self { enabled: false, service_tier: default_service_tier() } }
}

fn default_service_tier() -> String { "priority".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AdvisorState {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    model: String,
    #[serde(rename = "lastReview", default)]
    last_review: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ForceState {
    #[serde(default)]
    tool: String,
    #[serde(default)]
    prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TodoState {
    #[serde(default)]
    text: String,
    #[serde(default)]
    status: String,
    #[serde(rename = "createdAt", default)]
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct BranchState {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(rename = "createdAt", default)]
    created_at: String,
    #[serde(default)]
    path: String,
}

fn mode_state_path(cwd: &Path) -> PathBuf { cwd.join(".jeden/mode-state.json") }

fn read_mode_state(cwd: &Path) -> ModeState {
    fs::read_to_string(mode_state_path(cwd))
        .ok()
        .and_then(|text| serde_json::from_str::<ModeState>(&text).ok())
        .unwrap_or_default()
}

fn write_mode_state(cwd: &Path, state: &ModeState) -> Result<(), String> {
    let path = mode_state_path(cwd);
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())? + "\n";
    fs::write(path, text).map_err(|e| e.to_string())
}

fn dirs_home() -> PathBuf { env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")) }

fn project_config_path(cwd: &Path) -> PathBuf { cwd.join(".jeden/config.json") }

fn read_json_value(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}))
}

fn write_json_value(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())? + "\n";
    fs::write(path, text).map_err(|e| e.to_string())
}

fn merged_config(cwd: &Path) -> Value {
    let mut merged = match read_json_value(&dirs_home().join(".jeden/config.json")) {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if let Value::Object(project) = read_json_value(&project_config_path(cwd)) {
        for (key, value) in project { merged.insert(key, value); }
    }
    Value::Object(merged)
}

fn is_plain_object(value: &Value) -> bool {
    matches!(value, Value::Object(_))
}

fn browser_record_from(config: &Value) -> Value {
    let browser = config.get("browser").filter(|value| is_plain_object(value)).cloned().unwrap_or_else(|| json!({}));
    let mode = browser.get("mode").and_then(Value::as_str)
        .or_else(|| config.get("browserMode").and_then(Value::as_str))
        .filter(|mode| matches!(*mode, "headless" | "visible"))
        .unwrap_or("headless");
    json!({
        "mode": mode,
        "updatedAt": browser.get("updatedAt").and_then(Value::as_str),
        "launch": browser.get("launch").filter(|value| is_plain_object(value)).cloned().unwrap_or_else(|| json!({})),
        "profile": browser.get("profile").filter(|value| is_plain_object(value)).cloned().unwrap_or_else(|| json!({})),
    })
}

fn format_browser_settings(label: &str, value: &Value) -> String {
    if value.as_object().map(|map| map.is_empty()).unwrap_or(true) {
        format!("{label}: (none)")
    } else {
        format!("{label}: {}", value)
    }
}

fn browser_option_value(key: &str, value: &str) -> Value {
    if value == "true" { return json!(true); }
    if value == "false" { return json!(false); }
    if matches!(key, "slowMo" | "timeout") {
        if let Ok(number) = value.parse::<f64>() { return json!(number); }
    }
    if key == "args" {
        return json!(value.split(',').map(str::trim).filter(|part| !part.is_empty()).collect::<Vec<_>>());
    }
    json!(value)
}

fn insert_nested_object(target: &mut Value, path: &[&str], value: Value) {
    if !target.is_object() { *target = json!({}); }
    let mut cursor = target.as_object_mut().expect("object");
    for part in &path[..path.len().saturating_sub(1)] {
        let next = cursor.entry((*part).to_string()).or_insert_with(|| json!({}));
        if !next.is_object() { *next = json!({}); }
        cursor = next.as_object_mut().expect("nested object");
    }
    if let Some(key) = path.last() {
        cursor.insert((*key).to_string(), value);
    }
}

fn parse_browser_options(tokens: &[String]) -> Result<(Value, Value), String> {
    let mut launch = json!({});
    let mut profile = json!({});
    for token in tokens {
        let Some((raw_key, raw_value)) = token.split_once('=') else {
            return Err(format!("Expected key=value option, got \"{token}\"."));
        };
        let key_parts = raw_key.split('.').filter(|part| !part.is_empty()).collect::<Vec<_>>();
        if key_parts.is_empty() || key_parts.iter().any(|part| {
            matches!(*part, "__proto__" | "constructor" | "prototype")
                || !part.chars().enumerate().all(|(index, ch)| if index == 0 { ch.is_ascii_alphabetic() } else { ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' })
        }) {
            return Err(format!("Invalid browser option key \"{raw_key}\"."));
        }
        let (scope, rest) = (key_parts[0], &key_parts[1..]);
        if scope == "launch" && !rest.is_empty() {
            insert_nested_object(&mut launch, rest, browser_option_value(rest.last().copied().unwrap_or(""), raw_value));
        } else if scope == "profile" && !rest.is_empty() {
            insert_nested_object(&mut profile, rest, browser_option_value(rest.last().copied().unwrap_or(""), raw_value));
        } else if raw_key == "launch" {
            insert_nested_object(&mut launch, &["executablePath"], browser_option_value("executablePath", raw_value));
        } else if raw_key == "profile" {
            insert_nested_object(&mut profile, &["name"], browser_option_value("name", raw_value));
        } else if matches!(raw_key, "args" | "channel" | "devtools" | "executablePath" | "slowMo" | "timeout") {
            insert_nested_object(&mut launch, &[raw_key], browser_option_value(raw_key, raw_value));
        } else if matches!(raw_key, "name" | "profileDir" | "profile" | "userDataDir") {
            let normalized = if raw_key == "profileDir" { "userDataDir" } else { raw_key };
            insert_nested_object(&mut profile, &[normalized], browser_option_value(normalized, raw_value));
        } else {
            return Err(format!("Unknown browser option \"{raw_key}\". Use launch.<key>=value or profile.<key>=value."));
        }
    }
    Ok((launch, profile))
}

fn plugin_registry_path(cwd: &Path) -> PathBuf { cwd.join(".jeden/plugins.json") }

fn plugin_registry(cwd: &Path) -> Value {
    let raw = read_json_value(&plugin_registry_path(cwd));
    json!({
        "version": 1,
        "sources": raw.get("sources").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({})),
        "installed": raw.get("installed").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({})),
        "reload": raw.get("reload").filter(|value| value.is_object()).cloned().unwrap_or(Value::Null),
    })
}

fn save_plugin_registry(cwd: &Path, registry: &Value) -> Result<PathBuf, String> {
    let file = plugin_registry_path(cwd);
    let mut normalized = plugin_registry(cwd);
    if let Some(map) = registry.as_object() {
        if let Some(sources) = map.get("sources").filter(|value| value.is_object()) { normalized["sources"] = sources.clone(); }
        if let Some(installed) = map.get("installed").filter(|value| value.is_object()) { normalized["installed"] = installed.clone(); }
        if let Some(reload) = map.get("reload") { normalized["reload"] = reload.clone(); }
    }
    normalized["updatedAt"] = json!(now_text());
    write_json_value(&file, &normalized)?;
    Ok(file)
}


fn format_plugin_source(value: &Value) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        value.get("name").and_then(Value::as_str).unwrap_or("-"),
        value.get("type").and_then(Value::as_str).unwrap_or("-"),
        value.get("source").and_then(Value::as_str).unwrap_or("-"),
        if value.get("enabled").and_then(Value::as_bool) == Some(false) { "disabled" } else { "enabled" }
    )
}

fn format_plugin(value: &Value) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        value.get("id").and_then(Value::as_str).unwrap_or("-"),
        value.get("version").and_then(Value::as_str).unwrap_or("-"),
        if value.get("enabled").and_then(Value::as_bool) == Some(false) { "disabled" } else { "enabled" },
        value.get("source").and_then(Value::as_str).unwrap_or("-")
    )
}

fn sorted_object_values(value: &Value) -> Vec<Value> {
    let mut values = value.as_object().map(|map| map.values().cloned().collect::<Vec<_>>()).unwrap_or_default();
    values.sort_by(|a, b| format_plugin(a).cmp(&format_plugin(b)));
    values
}

fn sanitize_marketplace_name(value: &str, fallback: &str) -> String {
    let mut out = String::new();
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            out.push(ch);
        } else if matches!(ch, '@' | '/' | '\\') {
            out.push('-');
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() { fallback.to_string() } else { trimmed }
}

fn marketplace_source_name(source: &str) -> String {
    let text = source.trim().trim_end_matches('/');
    let tail = text.rsplit('/').next().unwrap_or(text);
    sanitize_marketplace_name(if tail.is_empty() { text } else { tail }, "source")
}

fn marketplace_source_type(source: &str) -> &'static str {
    let text = source.trim().to_ascii_lowercase();
    if text.starts_with("http://") || text.starts_with("https://") { "url" }
    else if text.starts_with("ssh://") || text.starts_with("git+ssh://") || text.starts_with("git@") { "git" }
    else { "local" }
}

// ---------------------------------------------------------------------------
// Marketplace plugin discovery / install / activation (OMP parity).
//
// On-disk layout under the user home (`~/.jeden`):
//   plugins/cache/marketplaces/<mktname>/   cloned repo / fetched catalog / copy
//   plugins/cache/plugins/<mkt>___<plugin>___<version>/   materialized plugin dir
// Installed records live in the existing plugin registry (`.jeden/plugins.json`,
// project scope, or `~/.jeden/plugins.json`, user scope) under `installed`, so
// `/marketplace installed` and `/plugins` keep working. Each installed entry now
// also carries the on-disk `path`, whether it ships `commands/` and `hooks.json`.
// ---------------------------------------------------------------------------

/// Home dir for plugin cache + user-scope registry. Overridable via
/// `JEDEN_PLUGINS_HOME` (used to keep tests hermetic); defaults to `~`.
fn plugins_home() -> PathBuf {
    env::var_os("JEDEN_PLUGINS_HOME").map(PathBuf::from).unwrap_or_else(dirs_home)
}
fn marketplace_cache_root() -> PathBuf { plugins_home().join(".jeden/plugins/cache/marketplaces") }
fn plugin_cache_root() -> PathBuf { plugins_home().join(".jeden/plugins/cache/plugins") }
fn marketplace_cache_dir(name: &str) -> PathBuf { marketplace_cache_root().join(name) }

/// Validate an OMP plugin/marketplace name: lowercase alphanumeric plus `-`/`.`,
/// first and last char alphanumeric, at most `max` chars.
fn valid_component_name(name: &str, max: usize) -> bool {
    let bytes = name.as_bytes();
    if name.is_empty() || name.len() > max { return false; }
    if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() { return false; }
    name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}
fn valid_plugin_name(name: &str) -> bool { valid_component_name(name, 64) }
fn valid_marketplace_name(name: &str) -> bool { valid_component_name(name, 64) }
/// A plugin id is `plugin@marketplace`, each part a valid name, total <= 128.
fn valid_plugin_id(id: &str) -> bool {
    if id.len() > 128 { return false; }
    match id.split_once('@') {
        Some((plugin, mkt)) => valid_plugin_name(plugin) && valid_marketplace_name(mkt),
        None => false,
    }
}

/// Reject anything unsafe to hand to `git` as an argument: empty, an option-like
/// leading `-`, control chars, or shell metacharacters. `git` is invoked via
/// `Command` (no shell) but this is defense in depth against injection through
/// catalog-controlled URLs/refs/paths.
fn git_arg_safe(value: &str) -> bool {
    if value.is_empty() || value.starts_with('-') { return false; }
    !value.chars().any(|c| c.is_control() || matches!(c,
        ' ' | '\t' | '\n' | '\r' | ';' | '&' | '|' | '`' | '$' | '(' | ')' | '{' | '}' |
        '<' | '>' | '*' | '?' | '[' | ']' | '!' | '\\' | '\'' | '"'))
}

enum FetchKind { Local, Github, GitUrl, JsonUrl }

/// Classify a marketplace source string for fetching (distinct from the display
/// `type` recorded by `/marketplace add`): local path, `owner/repo` github
/// shorthand, a direct `*.json` catalog URL, or a generic git URL.
fn classify_fetch(source: &str) -> FetchKind {
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
fn split_ref(source: &str) -> (String, Option<String>) {
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
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
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

fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
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

fn git_clone(url: &str, git_ref: Option<&str>, dest: &Path) -> Result<(), String> {
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

fn http_get_text(url: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client.get(url).send().map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() { return Err(format!("GET {url} failed: HTTP {}", status.as_u16())); }
    response.text().map_err(|e| e.to_string())
}

/// Fetch (or re-fetch) a marketplace source into its cache dir and return it.
fn fetch_marketplace(cwd: &Path, name: &str, source: &str) -> Result<PathBuf, String> {
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

/// Read a marketplace catalog, preferring `.omp-plugin/marketplace.json` then
/// `.claude-plugin/marketplace.json`.
fn read_marketplace_catalog(cache_dir: &Path) -> Result<Value, String> {
    for rel in [".omp-plugin/marketplace.json", ".claude-plugin/marketplace.json"] {
        let path = cache_dir.join(rel);
        if path.is_file() {
            let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            return serde_json::from_str(&text).map_err(|e| format!("{}: invalid JSON: {e}", path.display()));
        }
    }
    Err(format!("No marketplace.json in {} (.omp-plugin or .claude-plugin)", cache_dir.display()))
}

fn catalog_plugin_root(catalog: &Value) -> String {
    catalog.get("metadata").and_then(|m| m.get("pluginRoot")).and_then(Value::as_str).unwrap_or("").trim().trim_matches('/').to_string()
}
fn catalog_plugins(catalog: &Value) -> Vec<Value> {
    catalog.get("plugins").and_then(Value::as_array).cloned().unwrap_or_default()
}
fn catalog_find_plugin(catalog: &Value, name: &str) -> Option<Value> {
    catalog_plugins(catalog).into_iter().find(|p| p.get("name").and_then(Value::as_str) == Some(name))
}

/// Resolve a relative (`./…`) plugin source inside a marketplace cache, applying
/// `plugin_root` and rejecting path traversal outside the repo root.
fn resolve_relative_plugin_path(mkt_cache: &Path, plugin_root: &str, source: &str) -> Result<PathBuf, String> {
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

fn plugin_manifest_version(plugin_dir: &Path) -> Option<String> {
    read_json_value(&plugin_dir.join("package.json")).get("version").and_then(Value::as_str).map(str::to_string)
        .or_else(|| read_json_value(&plugin_dir.join(".claude-plugin/plugin.json")).get("version").and_then(Value::as_str).map(str::to_string))
}

struct Materialized { staging: PathBuf, source_desc: String, sha: Option<String> }

/// Materialize a plugin's directory (from its catalog `source`) into a staging
/// dir under the plugin cache. Caller renames it to the final versioned dir.
fn materialize_plugin(mkt_cache: &Path, catalog: &Value, entry: &Value) -> Result<Materialized, String> {
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

fn registry_scope_dir(cwd: &Path, scope: &str) -> PathBuf {
    if scope == "project" { cwd.to_path_buf() } else { plugins_home() }
}

/// Look up a marketplace source string by name across project then user scopes.
fn find_marketplace_source(cwd: &Path, name: &str) -> Option<String> {
    for dir in [cwd.to_path_buf(), plugins_home()] {
        if let Some(src) = plugin_registry(&dir).get("sources").and_then(Value::as_object)
            .and_then(|s| s.get(name)).and_then(|s| s.get("source")).and_then(Value::as_str) {
            return Some(src.to_string());
        }
    }
    None
}

/// All configured marketplace sources (name -> source) across both scopes.
fn all_marketplace_sources(cwd: &Path) -> Vec<(String, String)> {
    let mut out: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for dir in [cwd.to_path_buf(), plugins_home()] {
        if let Some(map) = plugin_registry(&dir).get("sources").and_then(Value::as_object) {
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
fn update_source_plugins(cwd: &Path, name: &str, plugins: &[Value]) {
    let summary: Vec<Value> = plugins.iter().map(|p| json!({
        "name": p.get("name").and_then(Value::as_str).unwrap_or(""),
        "description": p.get("description").and_then(Value::as_str).unwrap_or(""),
        "version": p.get("version").and_then(Value::as_str).unwrap_or(""),
    })).collect();
    for dir in [cwd.to_path_buf(), plugins_home()] {
        let mut registry = plugin_registry(&dir);
        let has = registry.get("sources").and_then(Value::as_object).map(|s| s.contains_key(name)).unwrap_or(false);
        if !has { continue; }
        if let Some(src) = registry.get_mut("sources").and_then(Value::as_object_mut).and_then(|s| s.get_mut(name)).and_then(Value::as_object_mut) {
            src.insert("plugins".into(), json!(summary));
            src.insert("updatedAt".into(), json!(now_text()));
        }
        let _ = save_plugin_registry(&dir, &registry);
    }
}

/// Merged installed-plugin records across scopes (project overrides user).
fn merged_installed_values(cwd: &Path) -> Vec<Value> {
    let mut map: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    for dir in [plugins_home(), cwd.to_path_buf()] {
        if let Some(installed) = plugin_registry(&dir).get("installed").and_then(Value::as_object) {
            for (id, entry) in installed { map.insert(id.clone(), entry.clone()); }
        }
    }
    map.into_values().collect()
}

fn split_plugin_id(id: &str) -> Result<(String, String), String> {
    match id.split_once('@') {
        Some((plugin, mkt)) if !plugin.is_empty() && !mkt.is_empty() => Ok((plugin.to_string(), mkt.to_string())),
        _ => Err(format!("Expected name@marketplace, got: {id}")),
    }
}

/// Parse `install`/`upgrade` flags: `--force`, `--scope <user|project>`, and the
/// remaining positional targets.
fn parse_marketplace_flags(argv: &[String]) -> (bool, Option<String>, Vec<String>) {
    let mut force = false;
    let mut scope = None;
    let mut rest = Vec::new();
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--force" | "-f" => force = true,
            "--scope" => scope = it.next().cloned(),
            s if s.starts_with("--scope=") => scope = Some(s.trim_start_matches("--scope=").to_string()),
            other => rest.push(other.to_string()),
        }
    }
    (force, scope, rest)
}

fn normalize_scope(scope: Option<String>) -> Result<String, String> {
    match scope.as_deref() {
        None | Some("user") => Ok("user".into()),
        Some("project") => Ok("project".into()),
        Some(other) => Err(format!("Invalid scope: {other}. Use user or project.")),
    }
}

/// Resolve, materialize, activate and record one plugin. Returns a report line.
fn install_one(cwd: &Path, mkt_name: &str, plugin_name: &str, scope: &str, force: bool) -> Result<String, String> {
    if !valid_marketplace_name(mkt_name) { return Err(format!("Invalid marketplace name: {mkt_name}")); }
    if !valid_plugin_name(plugin_name) { return Err(format!("Invalid plugin name: {plugin_name}")); }
    let id = format!("{plugin_name}@{mkt_name}");
    if !valid_plugin_id(&id) { return Err(format!("Invalid plugin id: {id}")); }
    let source = find_marketplace_source(cwd, mkt_name)
        .ok_or_else(|| format!("Marketplace source not found: {mkt_name}. Add it with /marketplace add <source>."))?;
    let mkt_cache = marketplace_cache_dir(mkt_name);
    if !mkt_cache.exists() { fetch_marketplace(cwd, mkt_name, &source)?; }
    let catalog = read_marketplace_catalog(&mkt_cache)?;
    let entry = catalog_find_plugin(&catalog, plugin_name)
        .ok_or_else(|| format!("Plugin {plugin_name} not found in marketplace {mkt_name}."))?;
    let mat = materialize_plugin(&mkt_cache, &catalog, &entry)?;
    let version = entry.get("version").and_then(Value::as_str).map(str::to_string)
        .or_else(|| plugin_manifest_version(&mat.staging))
        .or_else(|| mat.sha.clone())
        .unwrap_or_else(|| "0.0.0".into());
    let final_dir = plugin_cache_root().join(format!(
        "{}___{}___{}", mkt_name, plugin_name, sanitize_marketplace_name(&version, "0.0.0")));
    if final_dir.exists() {
        if force { fs::remove_dir_all(&final_dir).map_err(|e| e.to_string())?; }
        else {
            let _ = fs::remove_dir_all(&mat.staging);
            return Err(format!("{id} is already installed (version {version}). Use --force to reinstall."));
        }
    }
    if let Some(parent) = final_dir.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    if fs::rename(&mat.staging, &final_dir).is_err() {
        copy_dir_recursive(&mat.staging, &final_dir)?;
        let _ = fs::remove_dir_all(&mat.staging);
    }
    let commands_dir = final_dir.join("commands");
    let has_commands = commands_dir.is_dir();
    let command_count = if has_commands {
        fs::read_dir(&commands_dir).map(|rd| rd.flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md")).count()).unwrap_or(0)
    } else { 0 };
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
    registry.get_mut("installed").and_then(Value::as_object_mut).ok_or("invalid plugin registry")?.insert(id.clone(), record);
    save_plugin_registry(&reg_dir, &registry)?;
    Ok(format!(
        "installed {id} ({} command{}, hooks: {}) [scope: {scope}, {}]",
        command_count,
        if command_count == 1 { "" } else { "s" },
        if has_hooks { "yes" } else { "no" },
        final_dir.display(),
    ))
}

fn installed_entries_for_scope(dir: &Path) -> Vec<Value> {
    plugin_registry(dir).get("installed").and_then(Value::as_object)
        .map(|m| m.values().cloned().collect()).unwrap_or_default()
}

/// Command directories contributed by ENABLED installed plugins, across project
/// then user scope. Appended after the project/user `.jeden/commands` dirs so
/// local commands win; each plugin's `commands/` dir is a fallback source.
pub(crate) fn installed_plugin_command_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for dir in [cwd.to_path_buf(), plugins_home()] {
        for entry in installed_entries_for_scope(&dir) {
            if entry.get("enabled").and_then(Value::as_bool) == Some(false) { continue; }
            if let Some(path) = entry.get("path").and_then(Value::as_str) {
                let commands = Path::new(path).join("commands");
                if commands.is_dir() && !dirs.contains(&commands) { dirs.push(commands); }
            }
        }
    }
    dirs
}

/// Parsed `hooks.json` configs from ENABLED installed plugins. User-scope plugin
/// hooks always apply; project-scope plugin hooks only when `allow_project`
/// (mirrors the `.jeden/hooks.json` trust gate in [`crate::hooks`]).
pub(crate) fn installed_plugin_hook_configs(cwd: &Path, allow_project: bool) -> Vec<Value> {
    let mut configs = Vec::new();
    for (dir, include) in [(cwd.to_path_buf(), allow_project), (plugins_home(), true)] {
        if !include { continue; }
        for entry in installed_entries_for_scope(&dir) {
            if entry.get("enabled").and_then(Value::as_bool) == Some(false) { continue; }
            if let Some(path) = entry.get("path").and_then(Value::as_str) {
                let hooks_path = Path::new(path).join("hooks.json");
                if hooks_path.is_file() {
                    if let Ok(text) = fs::read_to_string(&hooks_path) {
                        if let Ok(value) = serde_json::from_str::<Value>(&text) { configs.push(value); }
                    }
                }
            }
        }
    }
    configs
}

fn handle_marketplace(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("help");
    let first = argv.get(1).map(String::as_str).unwrap_or("");
    let mut registry = plugin_registry(context.cwd);
    if verb == "help" {
        return Ok("Usage: /marketplace add <source> | remove <name> | list | update [name] | discover [marketplace] | install [--force] [--scope user|project] <name@marketplace> | upgrade [--scope user|project] [name@marketplace] | installed | uninstall <name@marketplace>.".into());
    }
    if verb == "add" {
        let source = argv.iter().skip(1).cloned().collect::<Vec<_>>().join(" ").trim().to_string();
        if source.is_empty() { return Err("Usage: /marketplace add <source>".into()); }
        let provisional = marketplace_source_name(&source);
        if !valid_marketplace_name(&provisional) {
            return Err(format!("Cannot derive a valid marketplace name from '{source}'."));
        }
        // OMP keys a marketplace by its catalog `name`. Fetch + read the catalog
        // authoritatively (errors surface), then rekey the cache to that name.
        let cache = fetch_marketplace(context.cwd, &provisional, &source)?;
        let catalog = read_marketplace_catalog(&cache)?;
        let cn = catalog.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let name = if valid_marketplace_name(&cn) {
            if cn != provisional {
                let to = marketplace_cache_dir(&cn);
                if let Some(parent) = to.parent() { let _ = fs::create_dir_all(parent); }
                let _ = fs::remove_dir_all(&to);
                fs::rename(&cache, &to).map_err(|e| format!("failed to key marketplace cache to '{cn}': {e}"))?;
            }
            cn
        } else {
            return Err(format!("Marketplace catalog at '{source}' has an invalid or missing name."));
        };
        let existing = registry.get("sources").and_then(Value::as_object).and_then(|sources| sources.get(&name)).cloned().unwrap_or_else(|| json!({}));
        let added_at = existing.get("addedAt").cloned().unwrap_or_else(|| json!(now_text()));
        registry.get_mut("sources").and_then(Value::as_object_mut).ok_or("invalid plugin registry")?.insert(name.clone(), json!({
            "name": name,
            "source": source.clone(),
            "type": marketplace_source_type(&source),
            "enabled": true,
            "addedAt": added_at,
            "updatedAt": now_text(),
            "plugins": existing.get("plugins").filter(|value| value.is_array()).cloned().unwrap_or_else(|| json!([])),
        }));
        let file = save_plugin_registry(context.cwd, &registry)?;
        return Ok(format!("Added marketplace source {} ({}) in {}.", name, source, file.display()));
    }
    if verb == "remove" {
        if first.is_empty() { return Err("Usage: /marketplace remove <name>".into()); }
        let sources = registry.get_mut("sources").and_then(Value::as_object_mut).ok_or("invalid plugin registry")?;
        if sources.remove(first).is_none() { return Err(format!("Marketplace source not found: {first}")); }
        let file = save_plugin_registry(context.cwd, &registry)?;
        return Ok(format!("Removed marketplace source {} from {}. Installed plugin records were kept; uninstall them explicitly if desired.", first, file.display()));
    }
    if verb == "list" {
        let mut sources = sorted_object_values(&registry["sources"]);
        sources.sort_by(|a, b| a.get("name").and_then(Value::as_str).unwrap_or("").cmp(b.get("name").and_then(Value::as_str).unwrap_or("")));
        return Ok(if sources.is_empty() { "No marketplace sources configured. Add one with /marketplace add <source>.".into() } else { ["Marketplace sources:".into()].into_iter().chain(sources.iter().map(format_plugin_source)).collect::<Vec<_>>().join("\n") });
    }
    if verb == "installed" {
        let mut installed = merged_installed_values(context.cwd);
        installed.sort_by(|a, b| a.get("id").and_then(Value::as_str).unwrap_or("").cmp(b.get("id").and_then(Value::as_str).unwrap_or("")));
        return Ok(if installed.is_empty() { "No plugins installed.".into() } else { ["Installed plugins:".into()].into_iter().chain(installed.iter().map(format_plugin)).collect::<Vec<_>>().join("\n") });
    }
    if verb == "uninstall" {
        if first.is_empty() { return Err("Usage: /marketplace uninstall <name@marketplace>".into()); }
        let mut removed_from = None;
        for dir in [context.cwd.to_path_buf(), dirs_home()] {
            let mut scoped = plugin_registry(&dir);
            let removed = scoped.get_mut("installed").and_then(Value::as_object_mut).map(|m| m.remove(first)).flatten();
            if removed.is_some() {
                let file = save_plugin_registry(&dir, &scoped)?;
                removed_from = Some(file);
            }
        }
        match removed_from {
            Some(file) => return Ok(format!("Uninstalled plugin {} from {}.", first, file.display())),
            None => return Err(format!("Installed plugin not found: {first}")),
        }
    }
    if verb == "discover" {
        let sources: Vec<(String, String)> = if first.is_empty() {
            all_marketplace_sources(context.cwd)
        } else {
            match find_marketplace_source(context.cwd, first) {
                Some(src) => vec![(first.to_string(), src)],
                None => return Err(format!("Marketplace source not found: {first}")),
            }
        };
        if sources.is_empty() { return Ok("No marketplace sources configured. Add one with /marketplace add <source>.".into()); }
        let mut lines = vec!["Available plugins:".to_string()];
        let mut errors = Vec::new();
        for (name, source) in sources {
            let cache = marketplace_cache_dir(&name);
            if !cache.exists() {
                if let Err(error) = fetch_marketplace(context.cwd, &name, &source) { errors.push(format!("- {name}: {error}")); continue; }
            }
            let catalog = match read_marketplace_catalog(&cache) { Ok(catalog) => catalog, Err(error) => { errors.push(format!("- {name}: {error}")); continue; } };
            let mut plugins = catalog_plugins(&catalog);
            plugins.sort_by(|a, b| a.get("name").and_then(Value::as_str).unwrap_or("").cmp(b.get("name").and_then(Value::as_str).unwrap_or("")));
            for plugin in plugins {
                let pname = plugin.get("name").and_then(Value::as_str).unwrap_or("-");
                let desc = plugin.get("description").and_then(Value::as_str).unwrap_or("");
                lines.push(format!("{pname}@{name}\t{desc}"));
            }
        }
        if lines.len() == 1 { lines.push("- none".into()); }
        lines.extend(errors);
        return Ok(lines.join("\n"));
    }
    if verb == "update" {
        let sources: Vec<(String, String)> = if first.is_empty() {
            all_marketplace_sources(context.cwd)
        } else {
            match find_marketplace_source(context.cwd, first) {
                Some(src) => vec![(first.to_string(), src)],
                None => return Err(format!("Marketplace source not found: {first}")),
            }
        };
        if sources.is_empty() { return Ok("No marketplace sources configured. Add one with /marketplace add <source>.".into()); }
        let mut updated = 0usize;
        let mut total_plugins = 0usize;
        let mut errors = Vec::new();
        for (name, source) in sources {
            match fetch_marketplace(context.cwd, &name, &source).and_then(|cache| read_marketplace_catalog(&cache)) {
                Ok(catalog) => {
                    let plugins = catalog_plugins(&catalog);
                    total_plugins += plugins.len();
                    update_source_plugins(context.cwd, &name, &plugins);
                    updated += 1;
                }
                Err(error) => errors.push(format!("- {name}: {error}")),
            }
        }
        let mut out = vec![format!("Updated {updated} marketplace source(s), {total_plugins} plugin(s) available.")];
        out.extend(errors);
        return Ok(out.join("\n"));
    }
    if verb == "install" {
        let (force, scope, targets) = parse_marketplace_flags(&argv[1..]);
        let scope = normalize_scope(scope)?;
        if targets.is_empty() { return Err("Usage: /marketplace install [--force] [--scope user|project] <name@marketplace>".into()); }
        let mut lines = Vec::new();
        for target in targets {
            let (plugin, mkt) = split_plugin_id(&target)?;
            lines.push(install_one(context.cwd, &mkt, &plugin, &scope, force)?);
        }
        return Ok(lines.join("\n"));
    }
    if verb == "upgrade" {
        let (_force, scope, targets) = parse_marketplace_flags(&argv[1..]);
        let scoped = match scope.as_deref() {
            Some(s) => vec![normalize_scope(Some(s.to_string()))?],
            None => vec!["project".to_string(), "user".to_string()],
        };
        let mut ids: Vec<(String, String)> = Vec::new();
        if targets.is_empty() {
            for scope in &scoped {
                for entry in installed_entries_for_scope(&registry_scope_dir(context.cwd, scope)) {
                    if let Some(id) = entry.get("id").and_then(Value::as_str) { ids.push((id.to_string(), scope.clone())); }
                }
            }
        } else {
            let scope = normalize_scope(scope)?;
            for target in targets { ids.push((target, scope.clone())); }
        }
        if ids.is_empty() { return Ok("No installed plugins to upgrade.".into()); }
        let mut lines = Vec::new();
        for (id, scope) in ids {
            let (plugin, mkt) = split_plugin_id(&id)?;
            lines.push(install_one(context.cwd, &mkt, &plugin, &scope, true)?);
        }
        return Ok(lines.join("\n"));
    }
    Err("Usage: /marketplace add <source> | remove <name> | list | update [name] | discover [marketplace] | install <name@marketplace> | upgrade [name@marketplace] | installed | uninstall <name@marketplace> | help".into())
}

fn clipboard_candidates() -> Vec<(&'static str, Vec<&'static str>)> {
    match env::consts::OS {
        "macos" => vec![("pbcopy", vec![])],
        "windows" => vec![("powershell.exe", vec!["-NoProfile", "-NonInteractive", "-Command", "Set-Clipboard -Value ([Console]::In.ReadToEnd())"]), ("clip.exe", vec![])],
        _ => vec![("wl-copy", vec![]), ("xclip", vec!["-selection", "clipboard"]), ("xsel", vec!["--clipboard", "--input"])],
    }
}

fn write_clipboard(payload: &str) -> Result<String, String> {
    let mut last_error = "no clipboard command was attempted".to_string();
    for (command, args) in clipboard_candidates() {
        match Command::new(command).args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn() {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut() {
                    if let Err(error) = stdin.write_all(payload.as_bytes()) {
                        last_error = error.to_string();
                        let _ = child.kill();
                        continue;
                    }
                }
                match child.wait_with_output() {
                    Ok(output) if output.status.success() => return Ok(command.to_string()),
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                        last_error = if stderr.is_empty() { format!("{command} exited with {}", output.status) } else { stderr };
                    }
                    Err(error) => last_error = error.to_string(),
                }
            }
            Err(error) => last_error = error.to_string(),
        }
    }
    Err(last_error)
}

fn handle_copy(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let payload = args.trim();
    if payload.is_empty() {
        return Err("/copy without text requires a live session recorder; pass text explicitly with /copy <text> in interactive Jeden.".into());
    }
    match write_clipboard(payload) {
        Ok(command) => Ok(format!("Copied provided text to the OS clipboard with {}.", command)),
        Err(error) => {
            let file = context.cwd.join(".jeden/copy.txt");
            if let Some(parent) = file.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
            fs::write(&file, payload).map_err(|e| e.to_string())?;
            Ok(format!("OS clipboard is unavailable ({}). Wrote provided text to fallback file: {}", error, file.display()))
        }
    }
}



fn collab_state_path(cwd: &Path) -> PathBuf { cwd.join(".jeden/collab.json") }

fn collab_default_relay(cwd: &Path) -> PathBuf { cwd.join(".jeden/collab-relay.jsonl") }

fn file_url(path: &Path) -> String {
    Url::from_file_path(path).map(|url| url.to_string()).unwrap_or_else(|_| format!("file://{}", path.display()))
}

fn collab_path(cwd: &Path, target: &str) -> Result<PathBuf, String> {
    let text = target.trim();
    if text.starts_with("http://") || text.starts_with("https://") {
        return Err("Rust collab currently supports durable file relays only; HTTP relay support remains JS-only.".into());
    }
    if text.starts_with("file://") {
        let url = Url::parse(text).map_err(|e| e.to_string())?;
        return url.to_file_path().map_err(|_| "Invalid file relay URL".to_string());
    }
    if text.is_empty() { return Ok(collab_default_relay(cwd)); }
    let path = PathBuf::from(text);
    Ok(if path.is_absolute() { path } else { cwd.join(path) })
}

fn append_collab_event(path: &Path, event_type: &str, cwd: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let line = serde_json::to_string(&json!({ "ts": now_text(), "type": event_type, "cwd": cwd })).map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| e.to_string())?;
    writeln!(file, "{}", line).map_err(|e| e.to_string())
}

fn read_collab_events(path: &Path) -> Vec<Value> {
    fs::read_to_string(path).unwrap_or_default().lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).collect()
}

fn collab_descriptor(entry: &Value) -> String {
    if let Some(file) = entry.get("relayFile").and_then(Value::as_str) {
        format!("durable file relay: {}", file)
    } else {
        "off".into()
    }
}

fn collab_role_status(role: &str, entry: &Value, view: bool) -> String {
    if entry.is_null() { return format!("Collab {role}: off."); }
    let relay_file = entry.get("relayFile").and_then(Value::as_str).unwrap_or("");
    let events = read_collab_events(Path::new(relay_file));
    let latest = events.last().and_then(|event| event.get("type")).and_then(Value::as_str).unwrap_or("none");
    let mut lines = vec![
        format!("Collab {role}: {}", collab_descriptor(entry)),
        format!("Relay URL: {}", entry.get("relayUrl").and_then(Value::as_str).unwrap_or("")),
        format!("Events: {}", events.len()),
        format!("Latest event: {}", latest),
    ];
    if view {
        if events.is_empty() {
            lines.push("Event log is empty.".into());
        } else {
            lines.push("Event log:".into());
            for (index, event) in events.iter().enumerate() {
                lines.push(format!("{}. {}", index + 1, serde_json::to_string(event).unwrap_or_else(|_| "{}".into())));
            }
        }
    }
    lines.join("\n")
}

fn save_collab_state(cwd: &Path, state: &Value) -> Result<PathBuf, String> {
    let file = collab_state_path(cwd);
    let host = state.get("host").cloned().unwrap_or(Value::Null);
    let guest = state.get("guest").cloned().unwrap_or(Value::Null);
    write_json_value(&file, &json!({ "version": 1, "updatedAt": now_text(), "host": host, "guest": guest }))?;
    Ok(file)
}

/// Encrypt a collab event under `key` and POST it to the HTTP relay. The relay
/// only ever sees the ciphertext; the key never leaves this process.
fn post_collab_http(base: &str, room: &str, key: &[u8; 32], event_type: &str, cwd: &Path) -> Result<(), String> {
    let event = json!({ "ts": now_text(), "type": event_type, "cwd": cwd });
    let plain = serde_json::to_vec(&event).map_err(|e| e.to_string())?;
    let blob = crate::collab::encrypt_blob(key, &plain)?;
    crate::collab::relay_post(base, room, &blob)?;
    Ok(())
}

/// Status for an HTTP-backed collab role. Shows the relay base + room + live
/// event count fetched from the relay (opaque blobs; contents stay encrypted).
fn collab_http_role_status(role: &str, entry: &Value) -> String {
    let base = entry.get("relayBase").and_then(Value::as_str).unwrap_or("");
    let room = entry.get("room").and_then(Value::as_str).unwrap_or("");
    let count = crate::collab::relay_get(base, room, 0).map(|(events, _)| events.len());
    let events_line = match count {
        Ok(n) => format!("Events: {} (encrypted)", n),
        Err(e) => format!("Events: unavailable ({})", e),
    };
    [
        format!("Collab {role}: HTTP relay {}", base),
        format!("Room: {}", room),
        events_line,
        "Payloads are end-to-end encrypted; the relay never sees plaintext or the key.".to_string(),
    ]
    .join("\n")
}

fn handle_collab(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, rest) = split_head(args);
    let verb = if verb.is_empty() { "status" } else { verb };
    let mut state = read_json_value(&collab_state_path(context.cwd));
    if !state.is_object() { state = json!({}); }
    if verb == "status" || verb == "view" {
        let host = state.get("host").unwrap_or(&Value::Null);
        let guest = state.get("guest").unwrap_or(&Value::Null);
        let file = collab_state_path(context.cwd);
        if host.is_null() && guest.is_null() {
            return Ok(format!("Collab off.\nRust backend: durable local file relay in .jeden/collab-relay.jsonl.\nState: {}", file.display()));
        }
        let mut sections = Vec::new();
        if !host.is_null() {
            sections.push(if host.get("backend").and_then(Value::as_str) == Some("http") { collab_http_role_status("host", host) } else { collab_role_status("host", host, verb == "view") });
        }
        if !guest.is_null() {
            sections.push(if guest.get("backend").and_then(Value::as_str) == Some("http") { collab_http_role_status("guest", guest) } else { collab_role_status("guest", guest, verb == "view") });
        }
        sections.push(format!("State: {}", file.display()));
        return Ok(sections.join("\n\n"));
    }
    if verb == "start" {
        let target = rest.trim();
        if target.starts_with("http://") || target.starts_with("https://") {
            let parsed = crate::collab::parse_relay_url(target)?;
            let (room, key) = if parsed.room.is_empty() {
                crate::collab::new_room_and_key()
            } else {
                (parsed.room.clone(), parsed.key.ok_or("HTTP relay start URL with a room must include #key=<k>")?)
            };
            post_collab_http(&parsed.base, &room, &key, "host-start", context.cwd)?;
            // Persist only the base + room; the key never touches disk.
            let entry = json!({ "backend": "http", "relayBase": parsed.base, "room": room, "startedAt": now_text(), "cwd": context.cwd });
            state["host"] = entry;
            save_collab_state(context.cwd, &state)?;
            let join_url = format!("{}/room/{}#key={}", parsed.base, room, crate::collab::encode_key(&key));
            return Ok(format!(
                "Collab started on HTTP relay {}.\nJoin with: /join {}\nBackend: HTTP relay (end-to-end encrypted).\nShare the join URL privately; its #key fragment is the decryption key and is never sent to the relay.",
                parsed.base, join_url
            ));
        }
        let relay = collab_path(context.cwd, rest)?;
        append_collab_event(&relay, "host-start", context.cwd)?;
        let entry = json!({ "backend": "file", "relayFile": relay, "relayUrl": file_url(&relay), "startedAt": now_text(), "cwd": context.cwd });
        state["host"] = entry;
        let file = save_collab_state(context.cwd, &state)?;
        return Ok(format!("Collab started with durable file relay: {}.\nJoin with: /join {}\nBackend: durable local file relay.\nState: {}", relay.display(), file_url(&relay), file.display()));
    }
    if verb == "stop" {
        let host = state.get("host").cloned().unwrap_or(Value::Null);
        if host.is_null() { return Ok("Collab hosting is already stopped.".into()); }
        if let Some(relay_file) = host.get("relayFile").and_then(Value::as_str) {
            append_collab_event(Path::new(relay_file), "host-stop", context.cwd)?;
        }
        state["host"] = Value::Null;
        let file = save_collab_state(context.cwd, &state)?;
        return Ok(format!("Collab hosting stopped.\nState: {}", file.display()));
    }
    Err("Usage: /collab [start|status|view|stop] [relay-file | http://relay-host[:port]]".into())
}

fn handle_join(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let target = args.trim();
    if target.is_empty() { return Err("Usage: /join <relay-file-or-file-url>".into()); }
    if target.starts_with("http://") || target.starts_with("https://") {
        let parsed = crate::collab::parse_relay_url(target)?;
        if parsed.room.is_empty() { return Err("Join URL must include a room: http://host/room/<id>#key=<k>".into()); }
        let key = parsed.key.ok_or("Join URL must include the #key=<k> fragment")?;
        // Fetch + decrypt existing events to prove the E2EE round-trip works.
        let (blobs, _) = crate::collab::relay_get(&parsed.base, &parsed.room, 0)?;
        if blobs.is_empty() {
            return Err("No events in that relay room yet — check the room id, or wait for the host to run /collab start.".into());
        }
        let mut decrypted = 0usize;
        for blob in &blobs {
            crate::collab::decrypt_blob(&key, blob).map_err(|e| format!("relay payload failed to decrypt (wrong key?): {}", e))?;
            decrypted += 1;
        }
        post_collab_http(&parsed.base, &parsed.room, &key, "guest-join", context.cwd)?;
        let mut state = read_json_value(&collab_state_path(context.cwd));
        if !state.is_object() { state = json!({}); }
        state["guest"] = json!({ "backend": "http", "relayBase": parsed.base, "room": parsed.room, "joinedAt": now_text(), "cwd": context.cwd });
        save_collab_state(context.cwd, &state)?;
        return Ok(format!(
            "Joined HTTP collab relay {} room {}.\nDecrypted {} existing event(s) end-to-end.\nBackend: HTTP relay (end-to-end encrypted); the key stays local and was never sent to the relay.",
            parsed.base, parsed.room, decrypted
        ));
    }
    let relay = collab_path(context.cwd, target)?;
    append_collab_event(&relay, "guest-join", context.cwd)?;
    let mut state = read_json_value(&collab_state_path(context.cwd));
    if !state.is_object() { state = json!({}); }
    state["guest"] = json!({ "backend": "file", "relayFile": relay, "relayUrl": file_url(&relay), "joinedAt": now_text(), "cwd": context.cwd });
    let file = save_collab_state(context.cwd, &state)?;
    Ok(format!("Joined collab via durable file relay: {}.\nRelay URL: {}\nState: {}", relay.display(), file_url(&relay), file.display()))
}

fn handle_leave(context: &SlashContext<'_>) -> Result<String, String> {
    let mut state = read_json_value(&collab_state_path(context.cwd));
    if !state.is_object() { state = json!({}); }
    let guest = state.get("guest").cloned().unwrap_or(Value::Null);
    if guest.is_null() {
        let host_note = if !state.get("host").unwrap_or(&Value::Null).is_null() { " Hosting is still active; use /collab stop to stop the host relay." } else { "" };
        return Ok(format!("No guest collab attachment is active.{}", host_note));
    }
    if let Some(relay_file) = guest.get("relayFile").and_then(Value::as_str) {
        append_collab_event(Path::new(relay_file), "guest-leave", context.cwd)?;
    }
    state["guest"] = Value::Null;
    let file = save_collab_state(context.cwd, &state)?;
    Ok(format!("Left collab relay.\nState: {}", file.display()))
}

fn slash_session_dir(context: &SlashContext<'_>, id_or_path: &str) -> Result<PathBuf, String> {
    let target = if id_or_path.trim().is_empty() {
        read_json_value(&mode_state_path(context.cwd))
            .get("lastSessionPath")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or("No current Rust session is recorded yet; pass a session id or path.")?
    } else {
        id_or_path.trim().to_string()
    };
    let raw_path = PathBuf::from(&target);
    let path = if target.contains('/') {
        if raw_path.is_absolute() { raw_path } else { context.cwd.join(raw_path) }
    } else {
        context.session_root.join(target)
    };
    if !path.exists() { return Err(format!("session not found: {}", path.display())); }
    Ok(path)
}

fn slash_session_events(dir: &Path) -> Vec<Value> {
    fs::read_to_string(dir.join("transcript.jsonl")).unwrap_or_default().lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).collect()
}

fn slash_session_value(context: &SlashContext<'_>, id_or_path: &str) -> Result<Value, String> {
    let dir = slash_session_dir(context, id_or_path)?;
    let state = read_json_value(&dir.join("state.json"));
    let id = dir.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_else(|| dir.display().to_string());
    Ok(json!({ "id": id, "path": dir, "state": state, "events": slash_session_events(&dir) }))
}

fn slash_session_text(session: &Value) -> String {
    let mut out = vec![format!("Session: {}", session.get("id").and_then(Value::as_str).unwrap_or("session")), format!("Path: {}", session.get("path").and_then(Value::as_str).unwrap_or("")), String::new()];
    for event in session.get("events").and_then(Value::as_array).cloned().unwrap_or_default() {
        out.push(format!("## {} {}", event.get("ts").and_then(Value::as_str).unwrap_or(""), event.get("type").and_then(Value::as_str).unwrap_or("")).trim().to_string());
        out.push(serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null)).unwrap_or_else(|_| "{}".into()));
        out.push(String::new());
    }
    out.join("\n")
}

fn slash_html_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn slash_session_export(session: &Value, format: &str) -> Result<String, String> {
    if format == "json" {
        return Ok(serde_json::to_string_pretty(session).map_err(|e| e.to_string())? + "\n");
    }
    if format == "markdown" || format == "md" {
        let mut out = format!("# Jeden session {}\n\n{}\n\n", session.get("id").and_then(Value::as_str).unwrap_or("session"), session.get("path").and_then(Value::as_str).unwrap_or(""));
        for event in session.get("events").and_then(Value::as_array).cloned().unwrap_or_default() {
            let label = format!("{} {}", event.get("ts").and_then(Value::as_str).unwrap_or(""), event.get("type").and_then(Value::as_str).unwrap_or("")).trim().to_string();
            let data = serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null)).unwrap_or_else(|_| "{}".into());
            out.push_str(&format!("## {}\n\n```json\n{}\n```\n\n", label, data));
        }
        return Ok(out);
    }
    if format == "html" {
        let id = slash_html_escape(session.get("id").and_then(Value::as_str).unwrap_or("session"));
        let path = slash_html_escape(session.get("path").and_then(Value::as_str).unwrap_or(""));
        let mut body = String::new();
        for event in session.get("events").and_then(Value::as_array).cloned().unwrap_or_default() {
            let label = slash_html_escape(&format!("{} {}", event.get("ts").and_then(Value::as_str).unwrap_or(""), event.get("type").and_then(Value::as_str).unwrap_or("")).trim().to_string());
            let data = slash_html_escape(&serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null)).unwrap_or_else(|_| "{}".into()));
            body.push_str(&format!("<section><h2>{}</h2><pre>{}</pre></section>\n", label, data));
        }
        return Ok(format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>Jeden session {}</title></head><body><h1>Jeden session {}</h1><p>{}</p>{}</body></html>\n", id, id, path, body));
    }
    Err(format!("unsupported session export format: {}", format))
}

fn handle_dump(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    Ok(slash_session_text(&slash_session_value(context, args.trim())?))
}

fn handle_export(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let mut id = String::new();
    let mut format = "json".to_string();
    let mut output: Option<String> = None;
    for arg in argv {
        if arg == "--html" { format = "html".into(); }
        else if arg == "--markdown" || arg == "--md" { format = "markdown".into(); }
        else if id.is_empty() && !arg.starts_with("--") && slash_session_dir(context, &arg).is_ok() { id = arg; }
        else { output = Some(arg); }
    }
    let payload = slash_session_export(&slash_session_value(context, &id)?, &format)?;
    if let Some(path) = output {
        let target = resolve_cwd_path(context.cwd, &path);
        if let Some(parent) = target.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        fs::write(&target, &payload).map_err(|e| e.to_string())?;
        Ok(target.display().to_string())
    } else {
        Ok(payload)
    }
}

fn handle_share(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let copy_link = argv.iter().any(|arg| matches!(arg.as_str(), "copy" | "--copy" | "--clipboard"));
    let session = slash_session_value(context, "")?;
    let id = session.get("id").and_then(Value::as_str).unwrap_or("session");
    let created_at = now_text();
    let plain = serde_json::to_vec_pretty(&json!({ "version": 1, "kind": "jeden-session", "createdAt": created_at, "session": session })).map_err(|e| e.to_string())?;
    let mut key = [0u8; 32];
    let mut iv = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut key);
    rand::thread_rng().fill_bytes(&mut iv);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
    let encrypted = cipher.encrypt(Nonce::from_slice(&iv), plain.as_ref()).map_err(|e| e.to_string())?;
    if encrypted.len() < 16 { return Err("encrypted share payload is unexpectedly short".into()); }
    let split = encrypted.len() - 16;
    let (ciphertext, tag) = encrypted.split_at(split);
    let session_dir = slash_session_dir(context, "")?;
    let artifact_dir = session_dir.join("artifacts");
    fs::create_dir_all(&artifact_dir).map_err(|e| e.to_string())?;
    let file = artifact_dir.join(format!("share-{}-{}.jeden-share", sanitize_marketplace_name(id, "session"), created_at));
    let bundle = json!({
        "version": 1,
        "kind": "jeden-encrypted-share",
        "backend": "file",
        "durable": true,
        "algorithm": "AES-256-GCM",
        "createdAt": created_at,
        "sessionId": id,
        "iv": URL_SAFE_NO_PAD.encode(iv),
        "tag": URL_SAFE_NO_PAD.encode(tag),
        "ciphertext": URL_SAFE_NO_PAD.encode(ciphertext),
        "note": "Durable encrypted session bundle. The decryption key is carried only in the returned URL fragment; keep the fragment private."
    });
    fs::write(&file, serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
    let url = format!("{}#key={}", file_url(&file), URL_SAFE_NO_PAD.encode(key));
    let copy_status = if copy_link {
        match write_clipboard(&url) {
            Ok(command) => format!("Copied share URL to clipboard with {}.", command),
            Err(error) => format!("Could not copy share URL to clipboard: {}", error),
        }
    } else {
        "Add `copy`, `--copy`, or `--clipboard` to copy the share URL.".into()
    };
    Ok(format!(
        "Encrypted durable share bundle written to {}\nShare URL with decryption key: {}\n{}\nBackend: durable local file bundle. Move or sync the file anywhere you trust; the URL fragment/key is never written into the bundle.",
        file.display(),
        url,
        copy_status
    ))
}

fn handle_omfg(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let complaint = args.trim();
    if complaint.is_empty() { return Err("Usage: /omfg <complaint>".into()); }
    let file = context.cwd.join(".jeden/rules.jsonl");
    if let Some(parent) = file.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let id = format!("rule-{}", now_text());
    let record = json!({
        "id": id,
        "kind": "omfg-rule",
        "createdAt": now_text(),
        "cwd": context.cwd,
        "complaint": complaint,
        "rule": format!("When this situation recurs, avoid the behavior described here: {}", complaint),
        "source": "/omfg"
    });
    let mut out = fs::OpenOptions::new().create(true).append(true).open(&file).map_err(|e| e.to_string())?;
    writeln!(out, "{}", serde_json::to_string(&record).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(format!("Forged local rule {}.\nRules file: {}\nRule: {}", id, file.display(), record.get("rule").and_then(Value::as_str).unwrap_or("")))
}


fn handle_tan(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let task = args.trim();
    if task.is_empty() { return Err("Usage: /tan <work>".into()); }
    let session_dir = slash_session_dir(context, "")?;
    let dir = session_dir.join("artifacts/tan-jobs");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let job_id = format!("tan-{}", now_text());
    let stdout_path = dir.join(format!("{}.stdout.log", job_id));
    let stderr_path = dir.join(format!("{}.stderr.log", job_id));
    let metadata_path = dir.join(format!("{}.json", job_id));
    let stdout = fs::File::create(&stdout_path).map_err(|e| e.to_string())?;
    let stderr = fs::File::create(&stderr_path).map_err(|e| e.to_string())?;
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command.arg("run").arg(task).arg("--cwd").arg(context.cwd).arg("--json");
    if let Some(model) = context.model.filter(|model| !model.trim().is_empty()) {
        command.arg("--model").arg(model);
    }
    let child = command.stdin(Stdio::null()).stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr)).spawn().map_err(|e| e.to_string())?;
    let pid = child.id();
    let metadata = json!({
        "id": job_id,
        "kind": "tan",
        "status": "running",
        "pid": pid,
        "task": task,
        "cwd": context.cwd,
        "sessionPath": session_dir,
        "stdout": stdout_path,
        "stderr": stderr_path,
        "startedAt": now_text()
    });
    write_json_value(&metadata_path, &metadata)?;
    let mut mode = read_json_value(&mode_state_path(context.cwd));
    if !mode.is_object() { mode = json!({}); }
    mode.as_object_mut().expect("mode object").insert("tanJobsSessionPath".into(), json!(session_dir));
    write_json_value(&mode_state_path(context.cwd), &mode)?;
    std::mem::forget(child);
    Ok(format!("Started detached tan job {}.\nPID: {}\nMetadata: {}\nStdout: {}\nStderr: {}", job_id, pid, metadata_path.display(), stdout_path.display(), stderr_path.display()))
}

fn handle_jobs(context: &SlashContext<'_>) -> Result<String, String> {
    let mode = read_json_value(&mode_state_path(context.cwd));
    let session_dir = if let Some(path) = mode.get("tanJobsSessionPath").and_then(Value::as_str) {
        PathBuf::from(path)
    } else {
        match slash_session_dir(context, "") {
            Ok(dir) => dir,
            Err(_) => return Ok("No background jobs are tracked for a Rust session yet.".into()),
        }
    };
    let dir = session_dir.join("artifacts/tan-jobs");
    let mut jobs = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") { continue; }
            let mut job = read_json_value(&path);
            if !job.is_object() { continue; }
            if let Some(map) = job.as_object_mut() {
                map.insert("metadata".into(), json!(path));
                for key in ["stdout", "stderr"] {
                    if let Some(log_path) = map.get(key).and_then(Value::as_str) {
                        if let Ok(meta) = fs::metadata(log_path) {
                            map.insert(format!("{key}Bytes"), json!(meta.len()));
                        }
                    }
                }
            }
            jobs.push(job);
        }
    }
    jobs.sort_by(|a, b| {
        a.get("startedAt").and_then(Value::as_str).unwrap_or("")
            .cmp(b.get("startedAt").and_then(Value::as_str).unwrap_or(""))
            .then_with(|| a.get("id").and_then(Value::as_str).unwrap_or("").cmp(b.get("id").and_then(Value::as_str).unwrap_or("")))
    });
    if jobs.is_empty() {
        Ok(format!("No background jobs are tracked in {}.", dir.display()))
    } else {
        serde_json::to_string_pretty(&jobs).map_err(|e| e.to_string())
    }
}

/// Render recent release notes from the source repo's git history — the real
/// changelog for this package (no bundled CHANGELOG file exists).
fn handle_changelog() -> Result<String, String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("git")
        .args(["log", "-20", "--pretty=format:%h  %ad  %s", "--date=short"])
        .current_dir(&root)
        .output()
        .map_err(|e| format!("git log failed: {e}"))?;
    if !output.status.success() {
        return Ok("No git history available for a changelog in this source tree.".into());
    }
    let log = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if log.is_empty() {
        return Ok("No commits found for a changelog.".into());
    }
    Ok(format!("Recent changes (git history, {}):\n{}", root.display(), log))
}

fn handle_extensions(context: &SlashContext<'_>) -> Result<String, String> {
    let registry = plugin_registry(context.cwd);
    let mut sources = sorted_object_values(&registry["sources"]);
    sources.sort_by(|a, b| a.get("name").and_then(Value::as_str).unwrap_or("").cmp(b.get("name").and_then(Value::as_str).unwrap_or("")));
    let mut installed = sorted_object_values(&registry["installed"]);
    installed.sort_by(|a, b| a.get("id").and_then(Value::as_str).unwrap_or("").cmp(b.get("id").and_then(Value::as_str).unwrap_or("")));
    let mut lines = vec![format!("Extension registry: {}", plugin_registry_path(context.cwd).display()), format!("Sources: {}", sources.len())];
    if sources.is_empty() { lines.push("- none".into()); } else { lines.extend(sources.iter().map(format_plugin_source)); }
    lines.push(format!("Installed plugins: {}", installed.len()));
    if installed.is_empty() { lines.push("- none".into()); } else { lines.extend(installed.iter().map(format_plugin)); }
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
fn handle_reload_plugins(context: &SlashContext<'_>) -> Result<String, String> {
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
        format!("Plugin reload scanned {} custom tool file(s).", registry["reload"]["customToolFiles"].as_array().map(Vec::len).unwrap_or(0)),
        format!("Visible Rust tool definitions: {}.", registry["reload"]["loadedTools"].as_array().map(Vec::len).unwrap_or(0)),
        format!("Reload marker: {}", file.display()),
        "The active tool registry is rebuilt on the next Jeden run; this Rust command records the reload request durably.".into(),
    ].join("\n"))
}

fn handle_plugins(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("list");
    let target = argv.get(1).map(String::as_str).unwrap_or("");
    let mut registry = plugin_registry(context.cwd);
    if verb == "list" {
        let installed = sorted_object_values(&registry["installed"]);
        return Ok(if installed.is_empty() { "No plugins installed. Use /marketplace discover and /marketplace install <name@marketplace>.".into() } else { ["Installed plugins:".into()].into_iter().chain(installed.iter().map(format_plugin)).collect::<Vec<_>>().join("\n") });
    }
    if verb == "enable" || verb == "disable" {
        if target.is_empty() { return Err(format!("Usage: /plugins {verb} <name@marketplace>")); }
        let installed = registry.get_mut("installed").and_then(Value::as_object_mut).ok_or("invalid plugin registry")?;
        let plugin = installed.get_mut(target).ok_or_else(|| format!("Installed plugin not found: {target}"))?;
        if !plugin.is_object() { *plugin = json!({}); }
        let plugin_obj = plugin.as_object_mut().expect("plugin object");
        plugin_obj.insert("enabled".into(), json!(verb == "enable"));
        plugin_obj.insert("updatedAt".into(), json!(now_text()));
        let file = save_plugin_registry(context.cwd, &registry)?;
        return Ok(format!("{} plugin {} in {}.", if verb == "enable" { "Enabled" } else { "Disabled" }, target, file.display()));
    }
    Err("Usage: /plugins list | enable <name@marketplace> | disable <name@marketplace>".into())
}


fn split_head(args: &str) -> (&str, &str) {
    let text = args.trim();
    if text.is_empty() { return ("", ""); }
    match text.find(char::is_whitespace) {
        Some(index) => (&text[..index], text[index..].trim()),
        None => (text, ""),
    }
}

fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn now_text() -> String { now_millis().to_string() }

fn split_args(value: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in value.trim().chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active { quote = None; } else { current.push(ch); }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() { args.push(current); }
    args
}

fn parse_duration_ms(value: &str) -> Option<u64> {
    let lower = value.trim().to_ascii_lowercase();
    let split = lower.find(|ch: char| !ch.is_ascii_digit())?;
    let amount = lower[..split].parse::<u64>().ok()?;
    match &lower[split..] {
        "ms" => Some(amount),
        "s" => Some(amount * 1_000),
        "m" => Some(amount * 60_000),
        "h" => Some(amount * 3_600_000),
        _ => None,
    }
}

fn format_goal_status(goal: &GoalState) -> String {
    if goal.objective.is_empty() { return "Goal mode has no objective. Use /goal set <objective>.".into(); }
    let state = if goal.enabled { if goal.paused { "paused" } else { "active" } } else { "disabled" };
    let budget = goal.budget.map(|v| if v.fract() == 0.0 { format!("{}", v as i64) } else { v.to_string() }).unwrap_or_else(|| "off".into());
    format!("Goal mode: {}\nObjective: {}\nBudget: {}", state, goal.objective, budget)
}

fn format_loop_status(loop_state: &LoopState) -> String {
    if !loop_state.enabled { return "Loop mode is disabled.".into(); }
    let mut limits = Vec::new();
    if let Some(remaining) = loop_state.remaining { limits.push(format!("{} resubmission(s) remaining", remaining)); }
    if let Some(until) = loop_state.until { limits.push(format!("until epoch-ms {}", until)); }
    if !loop_state.prompt.is_empty() { limits.push(format!("prompt: {}", loop_state.prompt)); }
    if limits.is_empty() { "Loop mode is enabled.".into() } else { format!("Loop mode is enabled ({}).", limits.join(", ")) }
}

fn current_model_route(context: &SlashContext<'_>) -> String {
    context.model
        .map(ToString::to_string)
        .or_else(|| env::var("JEDEN_MODEL").ok())
        .or_else(|| env::var("MODEL").ok())
        .unwrap_or_else(|| "default".into())
}

fn advisor_model_label(advisor: &AdvisorState, context: &SlashContext<'_>) -> String {
    if advisor.model.is_empty() { current_model_route(context) } else { advisor.model.clone() }
}

fn format_advisor_status(advisor: &AdvisorState, context: &SlashContext<'_>) -> String {
    [
        format!("Advisor reviewer is {}.", if advisor.enabled { "enabled" } else { "disabled" }),
        "Review backend: second model-router call after each successful agent result.".to_string(),
        format!("Configured reviewer route: {}.", advisor_model_label(advisor, context)),
        if advisor.last_review.is_some() { "Last advisor notes are available with /advisor dump.".to_string() } else { "No advisor notes have been recorded yet.".to_string() },
    ].join("\n")
}

fn handle_plan(args: &str, state: &mut ModeState) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = head.to_ascii_lowercase();
    if args.trim().is_empty() {
        state.plan.enabled = !state.plan.enabled;
        return Ok(format!("Plan mode {}.", if state.plan.enabled { "enabled" } else { "disabled" }));
    }
    match verb.as_str() {
        "on" => { state.plan.enabled = true; Ok("Plan mode enabled.".into()) },
        "off" => { state.plan.enabled = false; Ok("Plan mode disabled.".into()) },
        "status" => Ok(format!("Plan mode is {}.{}", if state.plan.enabled { "enabled" } else { "disabled" }, if state.plan.latest_plan.is_empty() { "" } else { "\nLatest plan is available for /plan-review." })),
        "run" if !rest.is_empty() => { state.plan.enabled = true; Ok("Plan mode enabled for this prompt.".into()) },
        _ => { state.plan.enabled = true; Ok("Plan mode enabled for this prompt.".into()) },
    }
}

fn handle_plan_review(state: &ModeState) -> Result<String, String> {
    if !state.plan.enabled && state.plan.latest_plan.trim().is_empty() {
        return Ok("Warning: Plan mode is not active.".into());
    }
    if state.plan.latest_plan.trim().is_empty() {
        return Ok("No plan review is available yet.".into());
    }
    Ok(state.plan.latest_plan.clone())
}

fn handle_goal(args: &str, state: &mut ModeState) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = head.to_ascii_lowercase();
    if args.trim().is_empty() || verb == "show" || verb == "status" { return Ok(format_goal_status(&state.goal)); }
    match verb.as_str() {
        "set" => {
            if rest.is_empty() { return Err("Usage: /goal set <objective>".into()); }
            state.goal.objective = rest.to_string();
            state.goal.enabled = true;
            state.goal.paused = false;
            Ok(format!("Goal mode enabled.\nObjective: {}", state.goal.objective))
        },
        "pause" => { state.goal.paused = true; Ok("Goal mode paused.".into()) },
        "resume" => {
            if state.goal.objective.is_empty() { return Err("No goal objective is set. Use /goal set <objective>.".into()); }
            state.goal.enabled = true;
            state.goal.paused = false;
            Ok("Goal mode resumed.".into())
        },
        "drop" | "off" => {
            state.goal.enabled = false;
            state.goal.paused = false;
            state.goal.objective.clear();
            state.goal.budget = None;
            Ok("Goal mode dropped.".into())
        },
        "budget" => {
            let budget = rest.trim().to_ascii_lowercase();
            if budget.is_empty() || budget == "off" {
                state.goal.budget = None;
                return Ok("Goal budget disabled.".into());
            }
            let parsed = budget.parse::<f64>().map_err(|_| "Usage: /goal budget <positive-number|off>".to_string())?;
            if !parsed.is_finite() || parsed <= 0.0 { return Err("Usage: /goal budget <positive-number|off>".into()); }
            state.goal.budget = Some(parsed);
            Ok(format!("Goal budget set to {}.", if parsed.fract() == 0.0 { format!("{}", parsed as i64) } else { parsed.to_string() }))
        },
        _ => {
            state.goal.objective = args.trim().to_string();
            state.goal.enabled = true;
            state.goal.paused = false;
            Ok(format!("Goal mode enabled.\nObjective: {}", state.goal.objective))
        },
    }
}

fn handle_loop(args: &str, state: &mut ModeState) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = head.to_ascii_lowercase();
    if verb == "off" || verb == "stop" {
        state.loop_mode = LoopState::default();
        return Ok("Loop mode disabled.".into());
    }
    if verb == "status" { return Ok(format_loop_status(&state.loop_mode)); }
    let mut prompt = args.trim();
    state.loop_mode.remaining = None;
    state.loop_mode.until = None;
    if !head.is_empty() && head.chars().all(|ch| ch.is_ascii_digit()) {
        state.loop_mode.remaining = head.parse::<u64>().ok();
        prompt = rest;
    } else if let Some(duration) = parse_duration_ms(head) {
        state.loop_mode.until = Some(now_millis() + duration);
        prompt = rest;
    }
    state.loop_mode.enabled = true;
    state.loop_mode.prompt = prompt.to_string();
    let qualifier = if let Some(remaining) = state.loop_mode.remaining {
        format!(" for {} resubmission(s)", remaining)
    } else if state.loop_mode.until.is_some() {
        " until the duration expires".to_string()
    } else {
        String::new()
    };
    Ok(format!("Loop mode enabled{}.", qualifier))
}

fn handle_fast(args: &str, state: &mut ModeState) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = head.to_ascii_lowercase();
    match verb.as_str() {
        "" => state.fast.enabled = !state.fast.enabled,
        "on" => state.fast.enabled = true,
        "off" => state.fast.enabled = false,
        "tier" => {
            if rest.is_empty() { return Err("Usage: /fast tier <service-tier>".into()); }
            state.fast.service_tier = rest.to_string();
            state.fast.enabled = true;
        },
        "status" => {},
        _ => return Err("Usage: /fast [on|off|status|tier <service-tier>]".into()),
    }
    let tier = if state.fast.service_tier.is_empty() { "priority" } else { &state.fast.service_tier };
    Ok(format!("Fast mode is {}. Model-router service_tier for future requests: {}.", if state.fast.enabled { "enabled" } else { "disabled" }, if state.fast.enabled { tier } else { "(default)" }))
}

fn handle_advisor(args: &str, state: &mut ModeState, context: &SlashContext<'_>) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = if head.is_empty() { "status".to_string() } else { head.to_ascii_lowercase() };
    match verb.as_str() {
        "on" => {
            state.advisor.enabled = true;
            Ok(format!("Advisor reviewer enabled.\n{}", format_advisor_status(&state.advisor, context)))
        },
        "off" => {
            state.advisor.enabled = false;
            Ok("Advisor reviewer disabled.".into())
        },
        "status" => Ok(format_advisor_status(&state.advisor, context)),
        "dump" => {
            let Some(review) = &state.advisor.last_review else { return Err("No advisor notes are available yet. Enable /advisor and complete an agent turn first.".into()); };
            if rest.trim().eq_ignore_ascii_case("raw") { return serde_json::to_string_pretty(review).map_err(|e| e.to_string()); }
            Ok(review.get("text").and_then(Value::as_str).unwrap_or("Advisor review is empty.").to_string())
        },
        "configure" => {
            let config_text = rest.trim();
            if config_text.is_empty() { return Ok(format_advisor_status(&state.advisor, context)); }
            let (key, value_rest) = split_head(config_text);
            let mut model = config_text.to_string();
            if key.eq_ignore_ascii_case("model") { model = value_rest.trim().to_string(); }
            else if let Some((left, right)) = key.split_once('=') {
                if left.eq_ignore_ascii_case("model") { model = right.to_string(); }
            }
            if model.is_empty() { return Err("Usage: /advisor configure [model <route>|model=<route>|<route>]".into()); }
            state.advisor.model = model;
            Ok(format!("Advisor reviewer route set to {}.\n{}", state.advisor.model, format_advisor_status(&state.advisor, context)))
        },
        _ => Err("Usage: /advisor [on|off|status|dump [raw]|configure [model <route>|model=<route>|<route>]]".into()),
    }
}

fn list_sessions(session_root: &Path, limit: usize) -> String {
    let mut rows = Vec::new();
    if let Ok(entries) = fs::read_dir(session_root) {
        for entry in entries.flatten().take(limit) { rows.push(entry.file_name().to_string_lossy().to_string()); }
    }
    if rows.is_empty() { "No sessions found.".into() } else { rows.join("\n") }
}

fn session_path(session_root: &Path, id_or_path: &str) -> PathBuf {
    if id_or_path.contains('/') { PathBuf::from(id_or_path) } else { session_root.join(id_or_path) }
}

fn handle_session(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, _) = split_head(args);
    if verb.is_empty() || verb == "info" {
        return Ok(format!("Session: rust one-shot slash invocation\nWorkspace: {}\nSession root: {}\nRecorder: not active in this non-interactive Rust command", context.cwd.display(), context.session_root.display()));
    }
    if verb == "delete" { return Err("Refusing to delete the active session from inside itself. Exit Jeden, then remove the session directory explicitly if you still want this destructive action.".into()); }
    Err("Usage: /session [info|delete]".into())
}

fn handle_lifecycle(command: &str, args: &str, state: &mut ModeState, context: &SlashContext<'_>) -> Option<Result<String, String>> {
    match command {
        "/new" | "/fresh" => Some(Ok("Started a fresh logical turn context. Provider stream state is reset for the next prompt in this Jeden process.".into())),
        "/drop" => Some(Err("Refusing to delete the active session from inside itself. Use /new for a fresh context or exit and remove the session directory explicitly.".into())),
        "/shake" => {
            state.shake = if args.trim().is_empty() { "elide".into() } else { args.trim().into() };
            Some(Ok(format!("Shake mode applied locally: {}. Subsequent prompts will instruct the model to avoid relying on heavy prior artifacts unless re-read.", state.shake)))
        },
        "/resume" => {
            let (id, _) = split_head(args);
            if id.is_empty() { Some(Ok(list_sessions(context.session_root, 10))) }
            else {
                let path = session_path(context.session_root, id);
                if path.exists() { Some(Ok(format!("Session {} exists at {}. Full in-place interactive resume is available through CLI: jeden resume {} \"<task>\"", path.file_name().map(|v| v.to_string_lossy()).unwrap_or_default(), path.display(), path.display()))) }
                else { Some(Err(format!("session not found: {}", path.display()))) }
            }
        },
        "/rename" => Some(Ok(format!("Session title set to: {}", if args.trim().is_empty() { "rust one-shot slash invocation" } else { args.trim() }))),
        "/move" => Some(Err("/move requires an active interactive session recorder; Rust one-shot slash commands cannot move a live recorder in this pass.".into())),
        _ => None,
    }
}

fn handle_mcp(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, rest) = split_head(args);
    let verb = if verb.is_empty() { "list" } else { verb };
    match verb {
        "list" => {
            let names = tools::configured_mcp_server_names(context.cwd);
            Ok(if names.is_empty() { "No MCP servers configured.".into() } else { names.join("\n") })
        },
        "tools" | "resources" | "prompts" | "test" => {
            let (server, _) = split_head(rest);
            if server.is_empty() { return Err(format!("Usage: /mcp {} <server>", verb)); }
            let result = match verb {
                "tools" | "test" => crate::mcp::list_tools(context.cwd, server, 10_000),
                "resources" => crate::mcp::list_resources(context.cwd, server, 10_000),
                "prompts" => crate::mcp::list_prompts(context.cwd, server, 10_000),
                _ => unreachable!(),
            }?;
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
        },
        "reload" => {
            // Stateless client: re-read config and health-probe every server by
            // spawning it and listing tools. This is the real reconnect for a
            // one-shot MCP client — no persistent handle to recycle.
            let names = tools::configured_mcp_server_names(context.cwd);
            if names.is_empty() {
                return Ok("No MCP servers configured.".into());
            }
            let mut lines = vec![format!("Reloaded MCP config: {} server(s).", names.len())];
            for name in &names {
                match crate::mcp::list_tools(context.cwd, name, 10_000) {
                    Ok(result) => {
                        let count = result.get("tools").and_then(Value::as_array).map(|t| t.len()).unwrap_or(0);
                        lines.push(format!("- {}: ok ({} tools)", name, count));
                    }
                    Err(error) => lines.push(format!("- {}: unreachable ({})", name, error.lines().next().unwrap_or("error"))),
                }
            }
            Ok(lines.join("\n"))
        }
        "reconnect" => {
            let (server, _) = split_head(rest);
            if server.is_empty() { return Err("Usage: /mcp reconnect <server>".into()); }
            match crate::mcp::list_tools(context.cwd, server, 10_000) {
                Ok(result) => {
                    let count = result.get("tools").and_then(Value::as_array).map(|t| t.len()).unwrap_or(0);
                    Ok(format!("Reconnected to {} by re-spawning it: ok ({} tools).", server, count))
                }
                Err(error) => Err(format!("Reconnect to {} failed: {}", server, error)),
            }
        }
        _ => Err("Usage: /mcp list | tools <server> | resources <server> | prompts <server> | test <server> | reload | reconnect".into()),
    }
}
fn handle_browser(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("");
    let config = merged_config(context.cwd);
    let current = browser_record_from(&config);
    let file = project_config_path(context.cwd);
    if verb.is_empty() || verb == "status" {
        return Ok([
            format!("Browser runtime preference: {}", current.get("mode").and_then(Value::as_str).unwrap_or("headless")),
            format!("Updated: {}", current.get("updatedAt").and_then(Value::as_str).unwrap_or("not set locally")),
            format!("Config: {}", file.display()),
            format_browser_settings("Launch settings", current.get("launch").unwrap_or(&Value::Null)),
            format_browser_settings("Profile settings", current.get("profile").unwrap_or(&Value::Null)),
            "Scope: configures the browser tool/controller backend selected by local Jeden config.".into(),
        ].join("\n"));
    }
    if !matches!(verb, "headless" | "visible") {
        return Err("Usage: /browser [status|headless|visible] [launch.<key>=value] [profile.<key>=value]".into());
    }
    let (launch, profile) = parse_browser_options(if argv.len() > 1 { &argv[1..] } else { &[] })
        .map_err(|error| format!("Usage: /browser [status|headless|visible] [launch.<key>=value] [profile.<key>=value]\n{error}"))?;
    let mut merged_launch = current.get("launch").cloned().unwrap_or_else(|| json!({}));
    let mut merged_profile = current.get("profile").cloned().unwrap_or_else(|| json!({}));
    if let (Some(target), Some(source)) = (merged_launch.as_object_mut(), launch.as_object()) {
        for (key, value) in source { target.insert(key.clone(), value.clone()); }
    }
    if let (Some(target), Some(source)) = (merged_profile.as_object_mut(), profile.as_object()) {
        for (key, value) in source { target.insert(key.clone(), value.clone()); }
    }
    let mut project = read_json_value(&file);
    if !project.is_object() { project = json!({}); }
    let object = project.as_object_mut().expect("project object");
    let mut browser = Map::new();
    browser.insert("mode".into(), json!(verb));
    browser.insert("updatedAt".into(), json!(now_text()));
    if merged_launch.as_object().map(|map| !map.is_empty()).unwrap_or(false) { browser.insert("launch".into(), merged_launch.clone()); }
    if merged_profile.as_object().map(|map| !map.is_empty()).unwrap_or(false) { browser.insert("profile".into(), merged_profile.clone()); }
    object.insert("browser".into(), Value::Object(browser));
    object.remove("browserMode");
    write_json_value(&file, &project)?;
    let mut lines = vec![format!("Browser runtime preference set to {verb}."), format!("Config: {}", file.display())];
    if merged_launch.as_object().map(|map| !map.is_empty()).unwrap_or(false) { lines.push(format_browser_settings("Launch settings", &merged_launch)); }
    if merged_profile.as_object().map(|map| !map.is_empty()).unwrap_or(false) { lines.push(format_browser_settings("Profile settings", &merged_profile)); }
    lines.push("Honest scope: this configures Jeden browser-tool/controller preference only; browser availability still depends on installed local tools or MCP adapters.".into());
    Ok(lines.join("\n"))
}


fn usage_path(cwd: &Path) -> PathBuf { cwd.join(".jeden/usage.json") }

fn handle_usage(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, _) = split_head(args);
    let verb = if verb.is_empty() { "show" } else { verb };
    let path = usage_path(context.cwd);
    if verb == "reset" {
        write_json_value(&path, &json!({"version": 1, "updatedAt": now_text(), "events": []}))?;
        return Ok(format!("Reset usage accounting: {}", path.display()));
    }
    if verb != "show" && verb != "status" {
        return Err("Usage: /usage [show|reset]".into());
    }
    let usage = read_json_value(&path);
    let events = usage.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut by_model = Map::new();
    let mut input_tokens = 0.0;
    let mut output_tokens = 0.0;
    let mut total_tokens = 0.0;
    for event in &events {
        let input = event.get("inputTokens").and_then(Value::as_f64).unwrap_or(0.0);
        let output = event.get("outputTokens").and_then(Value::as_f64).unwrap_or(0.0);
        let total = event.get("totalTokens").and_then(Value::as_f64).unwrap_or(0.0);
        input_tokens += input;
        output_tokens += output;
        total_tokens += total;
        let model = event.get("model").and_then(Value::as_str).unwrap_or("default").to_string();
        let entry = by_model.entry(model).or_insert_with(|| json!({"calls": 0, "inputTokens": 0.0, "outputTokens": 0.0, "totalTokens": 0.0}));
        if let Value::Object(map) = entry {
            let calls = map.get("calls").and_then(Value::as_u64).unwrap_or(0) + 1;
            let model_input = map.get("inputTokens").and_then(Value::as_f64).unwrap_or(0.0) + input;
            let model_output = map.get("outputTokens").and_then(Value::as_f64).unwrap_or(0.0) + output;
            let model_total = map.get("totalTokens").and_then(Value::as_f64).unwrap_or(0.0) + total;
            map.insert("calls".into(), json!(calls));
            map.insert("inputTokens".into(), json!(model_input));
            map.insert("outputTokens".into(), json!(model_output));
            map.insert("totalTokens".into(), json!(model_total));
        }
    }
    let recent = events.iter().rev().take(10).cloned().collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>();
    let summary = json!({
        "file": path,
        "updatedAt": usage.get("updatedAt").cloned().unwrap_or(Value::Null),
        "totals": {
            "calls": events.len(),
            "inputTokens": input_tokens,
            "outputTokens": output_tokens,
            "totalTokens": total_tokens,
            "byModel": by_model,
        },
        "recent": recent,
    });
    serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())
}

fn memory_file_path() -> PathBuf {
    env::var_os("JEDEN_MEMORY_FILE").map(PathBuf::from).unwrap_or_else(|| dirs_home().join(".jeden/memory.jsonl"))
}

fn tool_values(context: &SlashContext<'_>) -> Vec<Value> {
    tools::list_tools(context.cwd)
        .into_iter()
        .map(|tool| json!({"name": tool.name, "description": tool.description, "input": {}}))
        .collect()
}

fn handle_doctor(context: &SlashContext<'_>) -> Result<String, String> {
    let all = tool_values(context);
    let report = json!({
        "ok": true,
        "cwd": context.cwd,
        "model": current_model_route(context),
        "checks": [
            {"id": "filesystem.cwd.readable", "ok": context.cwd.is_dir(), "fatal": true, "path": context.cwd},
            {"id": "tools.static.load", "ok": true, "fatal": false},
        ],
        "tools": {
            "total": all.len(),
        },
        "memory": {
            "backend": "local-jsonl",
            "path": memory_file_path(),
        },
    });
    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
}

fn ssh_hosts_from(config: &Value) -> Option<&Map<String, Value>> {
    config.get("sshHosts").and_then(Value::as_object)
        .or_else(|| config.get("ssh").and_then(|ssh| ssh.get("hosts")).and_then(Value::as_object))
        .or_else(|| config.get("ssh").and_then(Value::as_object))
}

fn ssh_host_value(target: &str, options: &[String]) -> Option<Value> {
    if options.is_empty() { return Some(Value::String(target.to_string())); }
    let mut host = Map::new();
    host.insert("host".into(), Value::String(target.to_string()));
    for option in options {
        let (key, value) = option.split_once('=')?;
        if key.is_empty() { return None; }
        host.insert(key.to_string(), Value::String(value.to_string()));
    }
    Some(Value::Object(host))
}

fn handle_ssh(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("list");
    let name = argv.get(1).map(String::as_str).unwrap_or("");
    let target = argv.get(2).map(String::as_str).unwrap_or("");
    let options = if argv.len() > 3 { &argv[3..] } else { &[] };
    let project_file = project_config_path(context.cwd);
    if verb == "list" {
        let config = merged_config(context.cwd);
        let Some(hosts) = ssh_hosts_from(&config) else {
            return Ok("No SSH hosts configured in ~/.jeden/config.json or <cwd>/.jeden/config.json (sshHosts).".into());
        };
        let mut names = hosts.keys().cloned().collect::<Vec<_>>();
        names.sort();
        if names.is_empty() {
            return Ok("No SSH hosts configured in ~/.jeden/config.json or <cwd>/.jeden/config.json (sshHosts).".into());
        }
        return Ok(names.into_iter().map(|host| {
            let value = hosts.get(&host).cloned().unwrap_or(Value::Null);
            let rendered = value.as_str().map(ToString::to_string).unwrap_or_else(|| serde_json::to_string(&value).unwrap_or_else(|_| "null".into()));
            format!("{}\t{}", host, rendered)
        }).collect::<Vec<_>>().join("\n"));
    }
    if verb == "help" {
        return Ok("Usage: /ssh list | add <name> <target> [key=value ...] | remove <name>. Hosts are stored in <cwd>/.jeden/config.json under sshHosts.".into());
    }
    if verb == "add" {
        if name.is_empty() || target.is_empty() { return Err("Usage: /ssh add <name> <target> [key=value ...]".into()); }
        let value = ssh_host_value(target, options).ok_or_else(|| "Usage: /ssh add <name> <target> [key=value ...]".to_string())?;
        let mut project = read_json_value(&project_file);
        if !project.is_object() { project = json!({}); }
        let object = project.as_object_mut().expect("project config object");
        let hosts = object.entry("sshHosts").or_insert_with(|| Value::Object(Map::new()));
        if !hosts.is_object() { *hosts = Value::Object(Map::new()); }
        hosts.as_object_mut().expect("sshHosts object").insert(name.to_string(), value);
        write_json_value(&project_file, &project)?;
        return Ok(format!("Added SSH host {} to {}.", name, project_file.display()));
    }
    if verb == "remove" {
        if name.is_empty() { return Err("Usage: /ssh remove <name>".into()); }
        let effective = merged_config(context.cwd);
        let mut project = read_json_value(&project_file);
        let in_project = project.get("sshHosts").and_then(Value::as_object).and_then(|hosts| hosts.get(name)).is_some();
        if !in_project {
            if ssh_hosts_from(&effective).and_then(|hosts| hosts.get(name)).is_some() {
                return Err(format!("SSH host {} is not in <cwd>/.jeden/config.json. Remove it from ~/.jeden/config.json or the config file that defines it.", name));
            }
            return Err(format!("SSH host not found: {}", name));
        }
        if let Some(hosts) = project.get_mut("sshHosts").and_then(Value::as_object_mut) { hosts.remove(name); }
        write_json_value(&project_file, &project)?;
        return Ok(format!("Removed SSH host {} from {}.", name, project_file.display()));
    }
    Err("Usage: /ssh list | add <name> <target> [key=value ...] | remove <name> | help".into())
}

fn resolve_cwd_path(cwd: &Path, target: &str) -> PathBuf {
    let path = PathBuf::from(target);
    if path.is_absolute() { path } else { cwd.join(path) }
}


fn load_memory_lines() -> Result<Vec<Value>, String> {
    let file = memory_file_path();
    let raw = match fs::read_to_string(&file) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).map_err(|e| e.to_string()))
        .collect()
}

fn save_memory_lines(records: &[Value]) -> Result<(), String> {
    let file = memory_file_path();
    if let Some(parent) = file.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let body = records.iter().map(|record| serde_json::to_string(record).map_err(|e| e.to_string())).collect::<Result<Vec<_>, _>>()?.join("\n");
    fs::write(&file, if body.is_empty() { String::new() } else { format!("{body}\n") }).map_err(|e| e.to_string())
}

fn handle_memory(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("view");
    let records = load_memory_lines()?;
    let file = memory_file_path();
    if matches!(verb, "stats" | "diagnose") {
        return Ok(format!("Memory file: {}\nRecords: {}\nScope: {}", file.display(), records.len(), context.cwd.display()));
    }
    if matches!(verb, "clear" | "reset") {
        save_memory_lines(&[])?;
        return Ok(format!("Cleared memory file: {}", file.display()));
    }
    if matches!(verb, "view" | "list" | "") {
        if records.is_empty() { return Ok("No memory records.".into()); }
        let query = argv.iter().skip(1).cloned().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
        let mut shown = records;
        if !query.is_empty() {
            shown.retain(|record| record.get("text").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase().contains(&query));
        }
        let start = shown.len().saturating_sub(20);
        return Ok(shown[start..].iter().map(|record| {
            let id = record.get("id").and_then(Value::as_str).unwrap_or("-");
            let kind = record.get("kind").and_then(Value::as_str).unwrap_or("-");
            let text = record.get("text").and_then(Value::as_str).or_else(|| record.get("content").and_then(Value::as_str)).unwrap_or("");
            format!("{id}\t{kind}\t{text}")
        }).collect::<Vec<_>>().join("\n"));
    }
    Err("Usage: /memory [view [query]|stats|diagnose|clear|reset]".into())
}

fn handle_todo(args: &str, state: &mut ModeState, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("list");
    let text = argv.iter().skip(1).cloned().collect::<Vec<_>>().join(" ");
    if verb.is_empty() || verb == "list" {
        if state.todos.is_empty() { return Ok("Todo list is empty.".into()); }
        return Ok(state.todos.iter().enumerate().map(|(index, todo)| format!("{}. [{}] {}", index + 1, todo.status, todo.text)).collect::<Vec<_>>().join("\n"));
    }
    if verb == "add" || verb == "start" {
        if text.is_empty() { return Err(format!("Usage: /todo {} <task>", verb)); }
        state.todos.push(TodoState { text: text.clone(), status: if verb == "start" { "in_progress".into() } else { "pending".into() }, created_at: now_text() });
        return Ok(format!("Todo added: {}", text));
    }
    if verb == "done" || verb == "drop" || verb == "rm" {
        let needle = text.to_ascii_lowercase();
        let Some(index) = state.todos.iter().position(|todo| todo.text.to_ascii_lowercase().contains(&needle)).or_else(|| text.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).filter(|&n| n < state.todos.len())) else {
            return Err(format!("Todo not found: {}", if text.is_empty() { "(missing)" } else { &text }));
        };
        let todo_text = state.todos[index].text.clone();
        if verb == "rm" { state.todos.remove(index); }
        else { state.todos[index].status = if verb == "done" { "done".into() } else { "dropped".into() }; }
        return Ok(format!("{} todo: {}", if verb == "rm" { "Removed" } else { "Updated" }, todo_text));
    }
    if verb == "copy" || verb == "export" {
        let md = if state.todos.is_empty() { "- [ ]".into() } else { state.todos.iter().map(|todo| format!("- [{}] {}", if todo.status == "done" { "x" } else { " " }, todo.text)).collect::<Vec<_>>().join("\n") };
        if verb == "copy" { return Ok(md); }
        let target = resolve_cwd_path(context.cwd, if text.is_empty() { "TODO.md" } else { &text });
        if let Some(parent) = target.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        fs::write(&target, format!("{}\n", md)).map_err(|e| e.to_string())?;
        return Ok(format!("Todos exported to {}", target.display()));
    }
    if verb == "import" {
        let target = resolve_cwd_path(context.cwd, if text.is_empty() { "TODO.md" } else { &text });
        let raw = fs::read_to_string(&target).map_err(|e| e.to_string())?;
        state.todos = raw.lines().filter_map(|line| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("- [") { return None; }
            let status_mark = trimmed.chars().nth(3)?;
            let text = trimmed.get(6..)?.trim().to_string();
            if text.is_empty() { return None; }
            Some(TodoState { text, status: if status_mark == 'x' || status_mark == 'X' { "done".into() } else { "pending".into() }, created_at: now_text() })
        }).collect();
        return Ok(format!("Imported {} todos from {}", state.todos.len(), target.display()));
    }
    Err("Usage: /todo [list|add|start|done|drop|rm|copy|export|import]".into())
}

fn handle_guided_goal(args: &str, state: &mut ModeState) -> Result<String, String> {
    let objective = args.trim();
    if objective.is_empty() { return Err("Usage: /guided-goal <rough objective>".into()); }
    state.guided_goal.active = true;
    state.guided_goal.rough_objective = objective.to_string();
    Ok("Guided goal drafting started. Jeden will use the next turn to refine the objective instead of pretending to open an overlay.".into())
}

fn handle_force(args: &str, state: &mut ModeState, context: &SlashContext<'_>) -> Result<String, String> {
    let (tool, _prompt) = split_head(args);
    if tool.is_empty() { return Err("Usage: /force <tool-name> [prompt]".into()); }
    let names = tools::list_tools(context.cwd).into_iter().map(|tool| tool.name).collect::<Vec<_>>();
    if !names.is_empty() && !names.iter().any(|name| name == tool) {
        return Err(format!("Unknown or unavailable tool: {}. Visible tools: {}", tool, names.iter().take(20).cloned().collect::<Vec<_>>().join(", ")));
    }
    state.force = Some(ForceState { tool: tool.to_string(), prompt: String::new() });
    Ok(format!("The next agent turn will be instructed to use {} first.", tool))
}

fn handle_branching(command: &str, _args: &str, state: &ModeState) -> Result<String, String> {
    if command == "/tree" {
        if state.branches.is_empty() {
            return Ok("No branches yet. Create one in an interactive session with /branch <title>.".into());
        }
        return Ok(state
            .branches
            .iter()
            .map(|branch| format!("{}\t{}\t{}\t{}", branch.id, branch.title, branch.created_at, branch.path))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    // /branch and /fork need a live conversation to fork; the interactive loop
    // handles them directly. The one-shot CLI has no live conversation.
    Err(format!("{} requires an interactive session (it forks the live conversation). Start `jeden` and run {} there.", command, command))
}




pub fn handle_local(context: &SlashContext<'_>, input: &str) -> Option<Result<String, String>> {
    let trimmed = input.trim();
    let (command, args) = split_head(trimmed);
    let command = command.to_ascii_lowercase();
    let mut state = read_mode_state(context.cwd);
    let mut changed = false;
    let result = match command.as_str() {
        "/plan" => { changed = args.trim() != "status"; Some(handle_plan(args, &mut state)) },
        "/plan-review" => Some(handle_plan_review(&state)),
        "/goal" => { changed = !matches!(split_head(args).0, "" | "show" | "status"); Some(handle_goal(args, &mut state)) },
        "/guided-goal" => { changed = true; Some(handle_guided_goal(args, &mut state)) },
        "/loop" => { changed = split_head(args).0 != "status"; Some(handle_loop(args, &mut state)) },
        "/fast" => { changed = split_head(args).0 != "status"; Some(handle_fast(args, &mut state)) },
        "/advisor" => { changed = !matches!(split_head(args).0, "" | "status" | "dump"); Some(handle_advisor(args, &mut state, context)) },
        "/tools" => Some(Ok(tools::tools_slash_text(context.cwd))),
        "/stats" | "/debug" => Some(handle_doctor(context)),
        "/usage" => Some(handle_usage(args, context)),
        "/session" => Some(handle_session(args, context)),
        "/todo" => { changed = !matches!(split_head(args).0, "" | "list" | "copy" | "export"); Some(handle_todo(args, &mut state, context)) },
        "/mcp" => Some(handle_mcp(args, context)),
        "/ssh" => Some(handle_ssh(args, context)),
        "/browser" => Some(handle_browser(args, context)),
        "/extensions" | "/status" => Some(handle_extensions(context)),
        "/plugins" => Some(handle_plugins(args, context)),
        "/hooks" => Some(Ok(crate::hooks::describe_hooks(context.cwd))),
        "/reload-plugins" => Some(handle_reload_plugins(context)),
        "/marketplace" => Some(handle_marketplace(args, context)),
        "/copy" => Some(handle_copy(args, context)),
        "/collab" => Some(handle_collab(args, context)),
        "/join" => Some(handle_join(args, context)),
        "/leave" => Some(handle_leave(context)),
        "/dump" => Some(handle_dump(args, context)),
        "/export" => Some(handle_export(args, context)),
        "/share" => Some(handle_share(args, context)),
        "/omfg" => Some(handle_omfg(args, context)),
        "/force" | "/force:" => { changed = true; Some(handle_force(args, &mut state, context)) },
        "/retry" => Some(Err("/retry must be executed through the agent runner so it can replay lastFailedTask.".into())),
        "/btw" => Some(Err("/btw must be executed through the agent runner so it can run the side question.".into())),
        "/memory" => Some(handle_memory(args, context)),
        "/branch" | "/fork" | "/tree" => Some(handle_branching(command.as_str(), args, &state)),
        "/new" | "/fresh" | "/drop" | "/shake" | "/resume" | "/rename" | "/move" => {
            changed = command == "/shake";
            handle_lifecycle(command.as_str(), args, &mut state, context)
        },
        "/agents" => Some(Ok("Agent controls:\n- /tan <work> starts a detached local agent job tracked in session artifacts.\n- /advisor manages second-pass reviewer mode.\n- /jobs shows locally tracked background jobs.".into())),
        "/jobs" => Some(handle_jobs(context)),
        "/changelog" => Some(handle_changelog()),
        "/hotkeys" => Some(Ok("Jeden input:\nType a prompt on the `jeden >` line and press Enter.\nSlash commands such as /help and /update run from the same line.\nCtrl-C exits.".into())),
        "/tan" => Some(handle_tan(args, context)),
        _ => None,
    };
    if changed {
        if let Some(Ok(_)) = &result {
            if let Err(error) = write_mode_state(context.cwd, &state) { return Some(Err(error)); }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("jeden-slash-{}-{}", name, now_millis()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn test_context<'a>(cwd: &'a Path, session_root: &'a Path) -> SlashContext<'a> {
        SlashContext { cwd, model: Some("test-model"), session_root }
    }

    #[test]
    fn ssh_add_list_remove_roundtrip_updates_project_config() {
        let cwd = temp_workspace("ssh");
        let sessions = cwd.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let context = test_context(&cwd, &sessions);

        let added = handle_local(&context, "/ssh add lab example.test user=me").unwrap().unwrap();
        assert!(added.contains("Added SSH host lab"));
        let listed = handle_local(&context, "/ssh list").unwrap().unwrap();
        assert!(listed.contains("lab"));
        assert!(listed.contains("example.test"));
        assert!(listed.contains("\"user\":\"me\""));
        let removed = handle_local(&context, "/ssh remove lab").unwrap().unwrap();
        assert!(removed.contains("Removed SSH host lab"));
        let listed = handle_local(&context, "/ssh list").unwrap().unwrap();
        assert!(listed.contains("No SSH hosts configured"));
    }

    #[test]
    fn usage_summary_and_reset_match_js_shape() {
        let cwd = temp_workspace("usage");
        let sessions = cwd.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(cwd.join(".jeden")).unwrap();
        fs::write(cwd.join(".jeden/usage.json"), r#"{"updatedAt":"then","events":[{"model":"a","inputTokens":2,"outputTokens":3,"totalTokens":5},{"model":"a","inputTokens":4,"outputTokens":1,"totalTokens":5}]}"#).unwrap();
        let context = test_context(&cwd, &sessions);

        let summary = handle_local(&context, "/usage show").unwrap().unwrap();
        let value: Value = serde_json::from_str(&summary).unwrap();
        assert_eq!(value["totals"]["calls"], 2);
        assert_eq!(value["totals"]["totalTokens"], 10.0);
        assert_eq!(value["totals"]["byModel"]["a"]["calls"], 2);

        let reset = handle_local(&context, "/usage reset").unwrap().unwrap();
        assert!(reset.contains("Reset usage accounting"));
        let reset_file = read_json_value(&cwd.join(".jeden/usage.json"));
        assert_eq!(reset_file["events"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn memory_view_stats_clear_use_jsonl_file() {
        let cwd = temp_workspace("memory");
        let sessions = cwd.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let memory_file = cwd.join("memory.jsonl");
        env::set_var("JEDEN_MEMORY_FILE", &memory_file);
        fs::write(&memory_file, "{\"id\":\"m1\",\"kind\":\"note\",\"text\":\"alpha durable\"}\n").unwrap();
        let context = test_context(&cwd, &sessions);

        let stats = handle_local(&context, "/memory stats").unwrap().unwrap();
        assert!(stats.contains("Records: 1"));
        let view = handle_local(&context, "/memory view alpha").unwrap().unwrap();
        assert!(view.contains("alpha durable"));
        let cleared = handle_local(&context, "/memory clear").unwrap().unwrap();
        assert!(cleared.contains("Cleared memory file"));
        assert_eq!(fs::read_to_string(&memory_file).unwrap(), "");
        env::remove_var("JEDEN_MEMORY_FILE");
    }

    // --- Marketplace plugin discovery / install / activation tests ---------

    /// Serializes tests that mutate process-global env (`JEDEN_PLUGINS_HOME`,
    /// `HOME`) so parallel runs stay hermetic.
    static ENV_LOCK: std::sync::LazyLock<parking_lot::Mutex<()>> =
        std::sync::LazyLock::new(|| parking_lot::Mutex::new(()));

    struct EnvGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let prev = env::var_os(key);
            env::set_var(key, value);
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => env::set_var(self.key, v),
                None => env::remove_var(self.key),
            }
        }
    }

    fn write_registry(dir: &Path, value: &Value) {
        write_json_value(&dir.join(".jeden/plugins.json"), value).unwrap();
    }

    /// Build a local marketplace `mkt/` under `root` with a `demo` plugin that
    /// ships one command (`hello.md`) and a `UserPromptSubmit` hook.
    fn make_local_marketplace(root: &Path) -> PathBuf {
        let mkt = root.join("mkt");
        let plugin = mkt.join("plugins/demo");
        fs::create_dir_all(mkt.join(".omp-plugin")).unwrap();
        fs::create_dir_all(plugin.join("commands")).unwrap();
        fs::write(
            mkt.join(".omp-plugin/marketplace.json"),
            r#"{"name":"mkt","owner":{"name":"tester"},"plugins":[{"name":"demo","source":"./plugins/demo","description":"Demo plugin"}]}"#,
        )
        .unwrap();
        fs::write(plugin.join("commands/hello.md"), "Say hi to $ARGUMENTS").unwrap();
        fs::write(
            plugin.join("hooks.json"),
            r#"{"version":1,"hooks":{"UserPromptSubmit":[{"command":"echo hi"}]}}"#,
        )
        .unwrap();
        mkt
    }

    #[test]
    fn marketplace_name_validation_accepts_valid_and_rejects_invalid() {
        for ok in ["demo", "a", "my-plugin", "plugin.v2", "a1b2", &"a".repeat(64)] {
            assert!(valid_plugin_name(ok), "expected valid: {ok}");
        }
        for bad in ["", "-lead", "trail-", ".dot", "dot.", "Upper", "has_underscore", "sp ace", &"a".repeat(65)] {
            assert!(!valid_plugin_name(bad), "expected invalid: {bad}");
        }
        assert!(valid_plugin_id("demo@mkt"));
        assert!(!valid_plugin_id("demo"));
        assert!(!valid_plugin_id("Demo@mkt"));
        assert!(!valid_plugin_id("demo@"));
        assert!(!valid_plugin_id(&format!("{}@{}", "a".repeat(64), "b".repeat(70))));
    }

    #[test]
    fn git_arg_safety_rejects_metachars_and_options() {
        for ok in ["https://github.com/o/r.git", "o/r", "v1.2.3", "main"] {
            assert!(git_arg_safe(ok), "expected safe: {ok}");
        }
        for bad in ["", "-x", "--upload-pack=evil", "a;b", "a b", "a|b", "$(x)", "a`b`", "a&b", "a>b"] {
            assert!(!git_arg_safe(bad), "expected unsafe: {bad}");
        }
    }

    #[test]
    fn catalog_parse_prefers_omp_plugin_over_claude_plugin() {
        let cwd = temp_workspace("catalog-pref");
        let cache = cwd.join("cache");
        fs::create_dir_all(cache.join(".omp-plugin")).unwrap();
        fs::create_dir_all(cache.join(".claude-plugin")).unwrap();
        fs::write(cache.join(".omp-plugin/marketplace.json"), r#"{"name":"omp","plugins":[]}"#).unwrap();
        fs::write(cache.join(".claude-plugin/marketplace.json"), r#"{"name":"claude","plugins":[]}"#).unwrap();
        let catalog = read_marketplace_catalog(&cache).unwrap();
        assert_eq!(catalog.get("name").and_then(Value::as_str), Some("omp"));

        // Falls back to .claude-plugin when .omp-plugin is absent.
        fs::remove_file(cache.join(".omp-plugin/marketplace.json")).unwrap();
        let catalog = read_marketplace_catalog(&cache).unwrap();
        assert_eq!(catalog.get("name").and_then(Value::as_str), Some("claude"));
    }

    #[test]
    fn relative_plugin_source_applies_plugin_root_and_rejects_traversal() {
        let cache = Path::new("/tmp/mkt-cache");
        // pluginRoot is prepended.
        let resolved = resolve_relative_plugin_path(cache, "plugins", "./demo").unwrap();
        assert_eq!(resolved, cache.join("plugins").join("demo"));
        // No pluginRoot.
        let resolved = resolve_relative_plugin_path(cache, "", "./demo").unwrap();
        assert_eq!(resolved, cache.join("demo"));
        // Traversal is rejected.
        assert!(resolve_relative_plugin_path(cache, "", "./../evil").is_err());
        assert!(resolve_relative_plugin_path(cache, "", "./a/../../b").is_err());
        // Must start with ./ .
        assert!(resolve_relative_plugin_path(cache, "", "demo").is_err());
        // Malicious pluginRoot is rejected.
        assert!(resolve_relative_plugin_path(cache, "../escape", "./demo").is_err());
    }

    #[test]
    fn installed_plugin_command_dirs_respects_enabled_flag() {
        let _lock = ENV_LOCK.lock();
        let cwd = temp_workspace("cmd-dirs");
        let empty_home = temp_workspace("cmd-dirs-home");
        let _home = EnvGuard::set("JEDEN_PLUGINS_HOME", &empty_home);
        let on = cwd.join("plugins/on");
        let off = cwd.join("plugins/off");
        fs::create_dir_all(on.join("commands")).unwrap();
        fs::create_dir_all(off.join("commands")).unwrap();
        write_registry(&cwd, &json!({
            "installed": {
                "on@mkt": {"id":"on@mkt","enabled":true,"path": on.to_string_lossy()},
                "off@mkt": {"id":"off@mkt","enabled":false,"path": off.to_string_lossy()},
            }
        }));
        let dirs = installed_plugin_command_dirs(&cwd);
        assert!(dirs.contains(&on.join("commands")), "enabled plugin dir must be present: {dirs:?}");
        assert!(!dirs.contains(&off.join("commands")), "disabled plugin dir must be absent: {dirs:?}");
    }

    #[test]
    fn installed_plugin_hook_configs_returns_enabled_plugin_hooks() {
        let _lock = ENV_LOCK.lock();
        let cwd = temp_workspace("hook-cfgs");
        let empty_home = temp_workspace("hook-cfgs-home");
        let _home = EnvGuard::set("JEDEN_PLUGINS_HOME", &empty_home);
        let plugin = cwd.join("plugins/demo");
        fs::create_dir_all(&plugin).unwrap();
        fs::write(plugin.join("hooks.json"), r#"{"version":1,"hooks":{"UserPromptSubmit":[{"command":"echo hi"}]}}"#).unwrap();
        write_registry(&cwd, &json!({
            "installed": { "demo@mkt": {"id":"demo@mkt","enabled":true,"path": plugin.to_string_lossy()} }
        }));
        // Project-scope hooks require allow_project.
        assert!(installed_plugin_hook_configs(&cwd, false).is_empty());
        let configs = installed_plugin_hook_configs(&cwd, true);
        assert_eq!(configs.len(), 1);
        let hooks = crate::hooks::parse_event_hooks(&configs[0], "UserPromptSubmit");
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].command, "echo hi");
    }

    #[test]
    fn marketplace_install_activates_commands_and_hooks_end_to_end() {
        let _lock = ENV_LOCK.lock();
        let cwd = temp_workspace("install-e2e");
        let sessions = cwd.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        // Isolate both the plugin cache/user-registry and the user commands dir.
        let home = temp_workspace("install-e2e-home");
        let _plugins_home = EnvGuard::set("JEDEN_PLUGINS_HOME", &home);
        let _home_env = EnvGuard::set("HOME", &home);
        make_local_marketplace(&cwd);
        let context = test_context(&cwd, &sessions);

        let added = handle_local(&context, "/marketplace add ./mkt").unwrap().unwrap();
        assert!(added.contains("Added marketplace source mkt"), "add output: {added}");

        let discover = handle_local(&context, "/marketplace discover").unwrap().unwrap();
        assert!(discover.contains("demo@mkt"), "discover output: {discover}");
        assert!(discover.contains("Demo plugin"), "discover output: {discover}");

        let install = handle_local(&context, "/marketplace install demo@mkt").unwrap().unwrap();
        assert!(install.contains("installed demo@mkt"), "install output: {install}");
        assert!(install.contains("1 command"), "install output: {install}");
        assert!(install.contains("hooks: yes"), "install output: {install}");

        // (a) The installed plugin's commands dir is now an active command dir,
        //     and /hello resolves + expands through the exact CLI resolver.
        let dirs = installed_plugin_command_dirs(&cwd);
        assert!(dirs.iter().any(|d| d.join("hello.md").is_file()), "plugin commands dir missing: {dirs:?}");
        let expanded = crate::resolve_file_command(&cwd, "/hello", "World");
        assert_eq!(expanded, Some("Say hi to World".to_string()), "/hello did not expand from installed plugin");

        // (b) The plugin's hooks.json is merged into the hook runtime.
        let configs = installed_plugin_hook_configs(&cwd, true);
        assert_eq!(configs.len(), 1);
        assert_eq!(crate::hooks::parse_event_hooks(&configs[0], "UserPromptSubmit").len(), 1);

        // Re-install without --force is rejected; --force succeeds.
        let dup = handle_local(&context, "/marketplace install demo@mkt");
        assert!(dup.unwrap().is_err(), "duplicate install should be rejected");
        let forced = handle_local(&context, "/marketplace install --force demo@mkt").unwrap().unwrap();
        assert!(forced.contains("installed demo@mkt"), "force reinstall output: {forced}");

        // installed lists the plugin; uninstall removes it.
        let installed = handle_local(&context, "/marketplace installed").unwrap().unwrap();
        assert!(installed.contains("demo@mkt"), "installed output: {installed}");
        let removed = handle_local(&context, "/marketplace uninstall demo@mkt").unwrap().unwrap();
        assert!(removed.contains("Uninstalled plugin demo@mkt"), "uninstall output: {removed}");
        assert_eq!(installed_plugin_command_dirs(&cwd).len(), 0, "command dirs must be empty after uninstall");
    }

    /// Regression: `/marketplace add <local-dir>` must key the marketplace by
    /// the catalog's declared `name`, NOT the source directory basename (OMP
    /// parity). Here the source dir basename (`mktsrc`) differs from the catalog
    /// `name` (`acme`); every downstream surface must use `acme`.
    #[test]
    fn marketplace_add_keys_by_catalog_name_not_dir_basename() {
        let _lock = ENV_LOCK.lock();
        let cwd = temp_workspace("keying-e2e");
        let sessions = cwd.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let home = temp_workspace("keying-e2e-home");
        let _plugins_home = EnvGuard::set("JEDEN_PLUGINS_HOME", &home);
        let _home_env = EnvGuard::set("HOME", &home);

        // Marketplace whose DIRECTORY basename (`mktsrc`) != catalog `name`
        // (`acme`). If these were equal, basename-keying would masquerade as
        // catalog-name keying and the regression would slip through.
        let mkt = cwd.join("mktsrc");
        let plugin = mkt.join("plugins/demo");
        fs::create_dir_all(mkt.join(".omp-plugin")).unwrap();
        fs::create_dir_all(plugin.join("commands")).unwrap();
        fs::write(
            mkt.join(".omp-plugin/marketplace.json"),
            r#"{"name":"acme","owner":{"name":"tester"},"plugins":[{"name":"demo","source":"./plugins/demo","description":"Demo plugin"}]}"#,
        )
        .unwrap();
        fs::write(plugin.join("commands/hello.md"), "Say hi to $ARGUMENTS").unwrap();
        assert_ne!(
            mkt.file_name().and_then(|n| n.to_str()),
            Some("acme"),
            "test invariant: source dir basename must differ from catalog name",
        );
        let context = test_context(&cwd, &sessions);

        // (1) add keys by catalog name `acme`, not basename `mktsrc`.
        let added = handle_local(&context, "/marketplace add ./mktsrc").unwrap().unwrap();
        assert!(added.contains("Added marketplace source acme"), "add output: {added}");
        let registry = plugin_registry(&cwd);
        let sources = registry.get("sources").and_then(Value::as_object).unwrap();
        assert!(sources.contains_key("acme"), "registry sources must be keyed by catalog name: {sources:?}");
        assert!(!sources.contains_key("mktsrc"), "registry must not be keyed by dir basename: {sources:?}");

        // (2) discover lists the plugin under the catalog name.
        let discover = handle_local(&context, "/marketplace discover").unwrap().unwrap();
        assert!(discover.contains("demo@acme"), "discover output: {discover}");
        assert!(!discover.contains("demo@mktsrc"), "discover must not use basename: {discover}");

        // (3) install by the catalog-name id succeeds.
        let install = handle_local(&context, "/marketplace install demo@acme").unwrap().unwrap();
        assert!(install.contains("demo@acme"), "install output: {install}");

        // (4) activation is keyed correctly: /hello resolves + expands.
        let expanded = crate::resolve_file_command(&cwd, "/hello", "World");
        assert_eq!(expanded, Some("Say hi to World".to_string()), "/hello did not expand from catalog-name-keyed install");
    }
}
