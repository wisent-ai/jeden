//! Naming a goal that has just been started.
//!
//! Split out of `goal_lifecycle/mod.rs`, which had grown past the module line
//! cap.

use serde_json::Value;
use std::env;
use std::path::Path;
use std::path::PathBuf;

/// Resolve a title for a freshly started goal: `transcript-lake goal title
/// --stdin --json` when the executable is available, otherwise the prompt's
/// first line trimmed to 100 characters.
pub fn resolve_goal_title(prompt: &str) -> String {
    if let Some(title) = transcript_lake_title(prompt) {
        return title;
    }
    let first_line = prompt.lines().next().unwrap_or("").trim();
    let mut title: String = first_line.chars().take(100).collect();
    if title.is_empty() {
        title = "New goal".to_string();
    }
    title
}

fn transcript_lake_title(prompt: &str) -> Option<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let executable = find_transcript_lake()?;
    let mut child = Command::new(executable)
        .args(["goal", "title", "--stdin", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(prompt.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value: Value = serde_json::from_slice(&output.stdout).ok()?;
    value
        .get("goal")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|goal| !goal.is_empty())
        .map(str::to_string)
}

fn find_transcript_lake() -> Option<PathBuf> {
    let mut directories: Vec<PathBuf> = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = env::var_os("HOME") {
        directories.push(Path::new(&home).join(".local/bin"));
    }
    directories.push(PathBuf::from("/opt/homebrew/bin"));
    directories.push(PathBuf::from("/usr/local/bin"));
    directories
        .into_iter()
        .map(|directory| directory.join("transcript-lake"))
        .find(|candidate| candidate.is_file())
}
