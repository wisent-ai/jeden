//! Naming a goal that has just been started.
//!
//! Split out of `goal_lifecycle/mod.rs`, which had grown past the module line
//! cap.

use serde_json::Value;
use std::env;
use std::path::{Path, PathBuf};

/// Resolve a title for a freshly started goal: `transcript-lake goal title
/// --stdin --json` when `transcript-lake` is on `PATH`, otherwise the
/// prompt's first line that holds text. An installed transcript-lake that
/// cannot be started, exits unsuccessfully or answers no goal is refused with
/// its cause, and a prompt with no text has no title: either one, replaced by
/// a placeholder, started a goal nobody named.
pub fn resolve_goal_title(prompt: &str) -> Result<String, String> {
    match find_transcript_lake() {
        Some(executable) => transcript_lake_title(&executable, prompt),
        None => prompt
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "the prompt holds no text to title the goal with".to_string()),
    }
}

fn transcript_lake_title(executable: &Path, prompt: &str) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let command_line = format!("{} goal title --stdin --json", executable.display());
    let failed = |operation: &str, cause: String| format!("{command_line}: {operation}: {cause}");
    let mut child = Command::new(executable)
        .args(["goal", "title", "--stdin", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| failed("cannot start", error.to_string()))?;
    // The prompt is written from its own thread while the answer is read:
    // written first, a child that answers before it reads all of a long
    // prompt filled its output pipe and both waited on each other.
    let mut stdin = child.stdin.take().ok_or_else(|| {
        failed(
            "cannot write the prompt",
            "the child has no stdin pipe".to_string(),
        )
    })?;
    let input = prompt.as_bytes().to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = stado_wait::child_output(child, &command_line)
        .map_err(|error| failed("cannot collect the answer", error.to_string()))?;
    match writer.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(failed("cannot write the prompt", error.to_string())),
        Err(panic) => {
            return Err(failed(
                "cannot write the prompt",
                crate::panicked::refusal("the prompt writer", &*panic),
            ))
        }
    }
    if !output.status.success() {
        return Err(failed(
            &output.status.to_string(),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| failed("answer is not JSON", error.to_string()))?;
    value
        .get("goal")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|goal| !goal.is_empty())
        .map(str::to_string)
        .ok_or_else(|| failed("answer names no goal", value.to_string()))
}

/// `transcript-lake` on this process's `PATH`, the one place executables
/// are declared for it.
fn find_transcript_lake() -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join("transcript-lake"))
        .find(|candidate| candidate.is_file())
}
