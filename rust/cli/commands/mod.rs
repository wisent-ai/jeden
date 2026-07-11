//! File-based custom slash-command discovery across provider directories with precedence.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{dirs_home, read_json, slash};

pub(crate) mod expand;

#[derive(Debug, Clone)]
pub(crate) struct DiscoveredCommand {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) source: String,
}

pub(crate) fn valid_file_command_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
}

fn push_discovered_command(
    out: &mut Vec<DiscoveredCommand>,
    seen: &mut std::collections::BTreeSet<String>,
    name: String,
    path: PathBuf,
    source: &str,
) {
    if valid_file_command_name(&name) && seen.insert(name.clone()) {
        out.push(DiscoveredCommand {
            name,
            path,
            source: source.to_string(),
        });
    }
}

fn md_files(dir: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        let mut paths = entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if recursive && path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                out.push(path);
            }
        }
    }
    out
}

fn frontmatter_field(text: &str, key: &str) -> Option<String> {
    let trimmed = text.trim_start_matches("\u{feff}");
    let rest = trimmed.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    for line in rest[..end].lines() {
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        if field.trim() == key {
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn add_flat_command_dir(
    out: &mut Vec<DiscoveredCommand>,
    seen: &mut std::collections::BTreeSet<String>,
    dir: PathBuf,
    source: &str,
) {
    for path in md_files(&dir, false) {
        let Some(name) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(ToString::to_string)
        else {
            continue;
        };
        push_discovered_command(out, seen, name, path, source);
    }
}

fn add_frontmatter_name_dir(
    out: &mut Vec<DiscoveredCommand>,
    seen: &mut std::collections::BTreeSet<String>,
    dir: PathBuf,
    source: &str,
) {
    for path in md_files(&dir, false) {
        let name = fs::read_to_string(&path)
            .ok()
            .and_then(|text| frontmatter_field(&text, "name"))
            .or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(ToString::to_string)
            });
        let Some(name) = name else { continue };
        push_discovered_command(out, seen, name, path, source);
    }
}

fn add_claude_command_dir(
    out: &mut Vec<DiscoveredCommand>,
    seen: &mut std::collections::BTreeSet<String>,
    dir: PathBuf,
    source: &str,
) {
    for path in md_files(&dir, true) {
        let Some(stem) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(ToString::to_string)
        else {
            continue;
        };
        push_discovered_command(out, seen, stem.clone(), path.clone(), source);
        if let Ok(relative) = path.strip_prefix(&dir) {
            let mut parts = relative
                .iter()
                .filter_map(|part| part.to_str())
                .collect::<Vec<_>>();
            if matches!(parts.as_slice(), [_, _, ..]) {
                if let Some(last) = parts.last_mut() {
                    *last = stem.as_str();
                }
                push_discovered_command(out, seen, parts.join(":"), path, source);
            }
        }
    }
}

fn raw_config_bool(value: &Value, path: &[&str]) -> Option<bool> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_bool()
}

fn parse_bool_text(value: &str) -> Option<bool> {
    match value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_ascii_lowercase()
        .as_str()
    {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn read_command_bool(path: &Path, key: &str) -> Option<bool> {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "json" => raw_config_bool(&read_json::<Value>(path), &["commands", key]),
        "yml" | "yaml" => {
            let text = fs::read_to_string(path).ok()?;
            let dotted = format!("commands.{key}");
            let mut in_commands = false;
            for line in text.lines() {
                let no_comment = line.split_once('#').map(|(head, _)| head).unwrap_or(line);
                if no_comment.trim().is_empty() {
                    continue;
                }
                let top_level = !no_comment.starts_with(|ch: char| ch.is_whitespace());
                let trimmed = no_comment.trim();
                if let Some((field, value)) = trimmed.split_once(':') {
                    if field.trim() == dotted {
                        return parse_bool_text(value);
                    }
                    if top_level && field.trim() == "commands" {
                        in_commands = true;
                        continue;
                    }
                    if in_commands && top_level {
                        in_commands = false;
                    }
                    if in_commands && field.trim() == key {
                        return parse_bool_text(value);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn command_provider_enabled(cwd: &Path, key: &str, default: bool) -> bool {
    let mut enabled = default;
    for path in [
        dirs_home().join(".jeden/config.json"),
        cwd.join(".jeden/config.json"),
    ] {
        if let Some(next) = read_command_bool(&path, key) {
            enabled = next;
        }
    }
    enabled
}

/// Discover file-based custom slash commands in provider precedence order.
/// The first discovered command name wins across providers.
pub(crate) fn discover_file_commands(cwd: &Path) -> Vec<DiscoveredCommand> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    add_flat_command_dir(
        &mut out,
        &mut seen,
        cwd.join(".jeden/commands"),
        "Jeden Project",
    );
    add_flat_command_dir(
        &mut out,
        &mut seen,
        dirs_home().join(".jeden/commands"),
        "Jeden User",
    );
    for dir in slash::installed_plugin_command_dirs(cwd) {
        add_flat_command_dir(&mut out, &mut seen, dir, "Installed Plugin");
    }
    if command_provider_enabled(cwd, "enableClaudeUser", true) {
        add_claude_command_dir(
            &mut out,
            &mut seen,
            dirs_home().join(".claude/commands"),
            "Claude User",
        );
    }
    if command_provider_enabled(cwd, "enableClaudeProject", true) {
        add_claude_command_dir(
            &mut out,
            &mut seen,
            cwd.join(".claude/commands"),
            "Claude Project",
        );
    }
    add_frontmatter_name_dir(
        &mut out,
        &mut seen,
        dirs_home().join(".codex/commands"),
        "Codex User",
    );
    add_frontmatter_name_dir(
        &mut out,
        &mut seen,
        cwd.join(".codex/commands"),
        "Codex Project",
    );
    if command_provider_enabled(cwd, "enableOpencodeUser", true) {
        add_frontmatter_name_dir(
            &mut out,
            &mut seen,
            dirs_home().join(".config/opencode/commands"),
            "OpenCode User",
        );
    }
    if command_provider_enabled(cwd, "enableOpencodeProject", true) {
        add_frontmatter_name_dir(
            &mut out,
            &mut seen,
            cwd.join(".opencode/commands"),
            "OpenCode Project",
        );
    }
    out
}

/// Resolve a file-based custom command `<name>` to its template body (frontmatter
/// stripped). Returns None if no provider contributes a matching command.
pub(crate) fn find_file_command(cwd: &Path, name: &str) -> Option<String> {
    let safe = name.trim().trim_start_matches('/');
    if !valid_file_command_name(safe) {
        return None;
    }
    discover_file_commands(cwd)
        .into_iter()
        .find(|command| command.name == safe)
        .and_then(|command| fs::read_to_string(command.path).ok())
        .map(|text| expand::strip_frontmatter(&text))
}
