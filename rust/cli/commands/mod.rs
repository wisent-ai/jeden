//! File-based custom slash-command discovery across provider directories with precedence.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{dirs_home, slash};

pub(crate) mod expand;

mod directories;

use directories::{add_claude_command_dir, add_flat_command_dir, add_frontmatter_name_dir};

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

/// Whether the command provider `commands.<key>` names is on, as the
/// configuration layers every run reads declare it (user files, then the
/// project's); `default` while no layer declares it. A hand-rolled reader
/// here took only `config.json`, read a file that did not parse as empty,
/// and parsed YAML line by line.
fn command_provider_enabled(cwd: &Path, key: &str, default: bool) -> bool {
    match crate::cli::config::merged_config_value(cwd)
        .get("commands")
        .and_then(|commands| commands.get(key))
        .and_then(Value::as_bool)
    {
        Some(enabled) => enabled,
        None => default,
    }
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
/// Callers run [`readable_file_command`] first, so a declared command whose
/// file cannot be read is refused rather than taken for no command.
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

/// Refuse a custom command a provider declares whose file cannot be read,
/// naming the file and the cause: taken for no command, `/name` went to the
/// model as a literal prompt. A name no provider declares is not refused.
pub(crate) fn readable_file_command(cwd: &Path, name: &str) -> Result<(), String> {
    let safe = name.trim().trim_start_matches('/');
    if !valid_file_command_name(safe) {
        return Ok(());
    }
    match discover_file_commands(cwd)
        .into_iter()
        .find(|command| command.name == safe)
    {
        Some(command) => fs::read_to_string(&command.path)
            .map(drop)
            .map_err(|error| {
                format!(
                    "custom command /{safe} is declared by {} but cannot be read: {error}",
                    command.path.display()
                )
            }),
        None => Ok(()),
    }
}
