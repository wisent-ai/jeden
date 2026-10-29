use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::marketplace_cache_dir;
use crate::slash::common::dirs_home;
use crate::slash::validate::{git_arg_safe, valid_marketplace_name};

pub(crate) enum FetchKind {
    Local,
    Github,
    GitUrl,
    JsonUrl,
}

/// Classify a marketplace source string for fetching: local path, `owner/repo`
/// github shorthand, a direct `*.json` catalog URL, or a generic git URL.
pub(crate) fn classify_fetch(source: &str) -> FetchKind {
    let text = source.trim();
    let lower = text.to_ascii_lowercase();
    if text.starts_with("./")
        || text.starts_with("../")
        || text.starts_with("~/")
        || text.starts_with('/')
    {
        return FetchKind::Local;
    }
    if (lower.starts_with("http://") || lower.starts_with("https://")) && lower.ends_with(".json") {
        return FetchKind::JsonUrl;
    }
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("git@")
        || lower.starts_with("ssh://")
        || lower.starts_with("git+ssh://")
    {
        return FetchKind::GitUrl;
    }
    if text.contains('/') && !text.contains(':') {
        return FetchKind::Github;
    }
    FetchKind::Local
}

/// Split an optional `#ref` suffix off a source string.
pub(crate) fn split_ref(source: &str) -> (String, Option<String>) {
    match source.split_once('#') {
        Some((base, git_ref)) if !git_ref.trim().is_empty() => {
            (base.trim().to_string(), Some(git_ref.trim().to_string()))
        }
        _ => (source.trim().to_string(), None),
    }
}

fn expand_local_path(cwd: &Path, source: &str) -> PathBuf {
    let text = source.trim();
    if let Some(rest) = text.strip_prefix("~/") {
        return dirs_home().join(rest);
    }
    let path = Path::new(text);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Recursively copy `src` into `dst` (skipping any `.git` directory). Every
/// failure names the path it was copying, so a refused plugin copy says which
/// file stopped it.
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    let copy_file = |from: &Path, to: &Path| {
        fs::copy(from, to).map(drop).map_err(|error| {
            format!(
                "{} cannot be copied to {}: {error}",
                from.display(),
                to.display()
            )
        })
    };
    fs::create_dir_all(dst)
        .map_err(|error| format!("{} cannot be created: {error}", dst.display()))?;
    let entries = fs::read_dir(src)
        .map_err(|error| format!("{} cannot be listed: {error}", src.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("an entry of {} cannot be read: {error}", src.display()))?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let file_type = entry
            .file_type()
            .map_err(|error| format!("the type of {} cannot be read: {error}", from.display()))?;
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_symlink() {
            match fs::metadata(&from) {
                Ok(meta) if meta.is_dir() => copy_dir_recursive(&from, &to)?,
                Ok(_) => copy_file(&from, &to)?,
                Err(error) => {
                    return Err(format!(
                        "{} is a symbolic link whose target cannot be read, so the plugin cannot be copied whole: {error}",
                        from.display()
                    ))
                }
            }
        } else {
            copy_file(&from, &to)?;
        }
    }
    Ok(())
}

pub(crate) fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never");
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let output =
        stado_wait::output(&mut command).map_err(|e| format!("git failed to start: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

pub(crate) fn git_clone(url: &str, git_ref: Option<&str>, dest: &Path) -> Result<(), String> {
    if !git_arg_safe(url) {
        return Err(format!("Unsafe git URL rejected: {url}"));
    }
    if let Some(git_ref) = git_ref {
        if !git_arg_safe(git_ref) {
            return Err(format!("Unsafe git ref rejected: {git_ref}"));
        }
    }
    clear_cache(dest)?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("{} cannot be created: {error}", parent.display()))?;
    }
    let dest_str = dest.to_string_lossy().to_string();
    let mut args: Vec<&str> = vec!["clone", "--depth", "1"];
    if let Some(git_ref) = git_ref {
        args.push("--branch");
        args.push(git_ref);
    }
    args.push(url);
    args.push(&dest_str);
    run_git(&args, None).map(|_clone_output| ())
}

/// Fetch one HTTP(S) text body.
///
/// The comment here used to claim no client-side deadline while calling
/// `reqwest::blocking::get`, which builds a client carrying the library's own
/// thirty seconds. The claim is now true.
fn http_get_text(url: &str) -> Result<String, String> {
    let client = crate::net::blocking_builder()
        .build()
        .map_err(|error| format!("the HTTP client for GET {url} cannot be built: {error}"))?;
    let response = client
        .get(url)
        .send()
        .map_err(|error| format!("GET {url} could not be sent: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("GET {url} failed: {status}"));
    }
    response.text().map_err(|error| {
        format!("GET {url} answered {status} with a body that cannot be read: {error}")
    })
}

/// Clear a marketplace's cache directory before it is fetched again.
fn clear_cache(dest: &Path) -> Result<(), String> {
    match fs::remove_dir_all(dest) {
        Ok(()) => {}
        // Only a cache that is not there needs no clearing.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "the marketplace cache {} cannot be cleared: {error}",
                dest.display()
            ))
        }
    }
    Ok(())
}

/// Fetch (or re-fetch) a marketplace source into its cache dir and return it.
pub(crate) fn fetch_marketplace(cwd: &Path, name: &str, source: &str) -> Result<PathBuf, String> {
    if !valid_marketplace_name(name) {
        return Err(format!("Invalid marketplace name: {name}"));
    }
    let dest = marketplace_cache_dir(name);
    match classify_fetch(source) {
        FetchKind::Local => {
            let src = expand_local_path(cwd, source);
            if !src.is_dir() {
                return Err(format!(
                    "Local marketplace path not found: {}",
                    src.display()
                ));
            }
            clear_cache(&dest)?;
            copy_dir_recursive(&src, &dest)?;
        }
        FetchKind::JsonUrl => {
            let body = http_get_text(source)?;
            serde_json::from_str::<Value>(&body)
                .map_err(|e| format!("the catalog at {source} is not valid JSON: {e}"))?;
            clear_cache(&dest)?;
            let manifest_dir = dest.join(".jeden-plugin");
            fs::create_dir_all(&manifest_dir).map_err(|error| {
                format!("{} cannot be created: {error}", manifest_dir.display())
            })?;
            let manifest = manifest_dir.join("marketplace.json");
            fs::write(&manifest, body)
                .map_err(|error| format!("{} cannot be written: {error}", manifest.display()))?;
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

/// Read a marketplace catalog, preferring `.jeden-plugin` then `.claude-plugin`.
pub(crate) fn read_marketplace_catalog(cache_dir: &Path) -> Result<Value, String> {
    for rel in [
        ".jeden-plugin/marketplace.json",
        ".claude-plugin/marketplace.json",
    ] {
        let path = cache_dir.join(rel);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!("{} cannot be read: {error}", path.display()));
            }
        };
        return serde_json::from_str(&text)
            .map_err(|error| format!("{}: invalid JSON: {error}", path.display()));
    }
    Err(format!(
        "No marketplace.json in {} (.jeden-plugin or .claude-plugin)",
        cache_dir.display()
    ))
}

/// The plugins a marketplace catalogue lists, or why it lists none.
pub(crate) fn catalog_plugins(catalog: &Value) -> Result<Vec<Value>, String> {
    catalog
        .get("plugins")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "the marketplace catalogue holds no `plugins` list".to_string())
}
