//! The status line the terminal draws every frame, built without ever
//! blocking the frame.
//!
//! Split out of `cli/run/interactive.rs`, which had grown past the module line
//! cap.

use serde_json::Value;
use std::env;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The branch and the count of uncommitted changes the status line shows.
/// Outside a git work tree there is no branch and nothing uncommitted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GitPrompt {
    pub(crate) branch: Option<String>,
    pub(crate) dirty_count: usize,
}

/// Git's view of the working directory, read when the directory or the
/// conversation's size changes and kept between frames, so a keypress never
/// starts a process. A change made outside Jeden shows after the next turn.
#[derive(Default)]
pub(super) struct GitWatch {
    read_for: Option<(PathBuf, Option<usize>)>,
    prompt: GitPrompt,
}

impl GitWatch {
    /// `tokens` is the conversation's size when it could be read without
    /// waiting; while a turn holds it the last reading stands.
    pub(super) fn current(&mut self, cwd: &Path, tokens: Option<usize>) -> GitPrompt {
        let stale = match (&self.read_for, tokens) {
            (None, _) => true,
            (Some((read_cwd, _)), _) if read_cwd != cwd => true,
            (Some((_, read_tokens)), Some(tokens)) => *read_tokens != Some(tokens),
            (Some(_), None) => false,
        };
        if stale {
            self.prompt = git_prompt(cwd);
            self.read_for = Some((cwd.to_path_buf(), tokens));
        }
        self.prompt.clone()
    }
}

/// One `git status --porcelain --branch` of `cwd`. A directory with no `.git`
/// above it starts no process; a `.git` that cannot be examined is not read
/// as absent, so git runs and says why. A git that cannot run or refuses
/// inside a work tree puts its cause where the branch would be.
pub(crate) fn git_prompt(cwd: &Path) -> GitPrompt {
    let absent = |dir: &Path| matches!(dir.join(".git").try_exists(), Ok(false));
    if cwd.ancestors().all(absent) {
        return GitPrompt::default();
    }
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(cwd)
        .args(["status", "--porcelain", "--branch"]);
    let output = match stado_wait::output(&mut command) {
        Ok(output) => output,
        Err(error) => return failed_prompt(format!("git cannot run: {error}")),
    };
    if !output.status.success() {
        return failed_prompt(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = text.lines().collect();
    match lines.split_first() {
        Some((header, changes)) => GitPrompt {
            branch: branch_of(header),
            dirty_count: changes.len(),
        },
        None => failed_prompt("git status printed no branch header".into()),
    }
}

fn failed_prompt(cause: String) -> GitPrompt {
    GitPrompt {
        branch: Some(cause),
        ..GitPrompt::default()
    }
}

/// The branch a `## ` header names; a detached HEAD names none.
fn branch_of(header: &str) -> Option<String> {
    let named = header.strip_prefix("## ")?;
    let named = match named.strip_prefix("No commits yet on ") {
        Some(unborn) => unborn,
        None => named,
    };
    if named.starts_with("HEAD (no branch)") {
        return None;
    }
    let name = match named.split_once("...") {
        Some((name, _upstream)) => name,
        None => named,
    };
    let name = match name.split_once(' ') {
        Some((name, _tracking)) => name,
        None => name,
    };
    Some(name.to_owned())
}

/// The service tier the status line shows, chosen in the order a model call
/// chooses it. No tier stated shows no route segment. A mode state that
/// cannot be read, or a tier variable that is not Unicode, is named on the
/// line itself.
pub(super) fn service_tier_prompt(cwd: &Path) -> String {
    let mode = match crate::slash::state::read_mode_state_value(cwd) {
        Ok(mode) => mode,
        Err(error) => return format!("mode state unreadable: {error}"),
    };
    let fast_tier = match (
        mode.pointer("/fast/enabled").and_then(Value::as_bool),
        mode.pointer("/fast/serviceTier").and_then(Value::as_str),
    ) {
        (Some(true), Some(tier)) if !tier.trim().is_empty() => Some(tier.to_owned()),
        _ => None,
    };
    match crate::agent::selected_service_tier(fast_tier) {
        Ok(Some(tier)) => tier,
        Ok(None) => String::new(),
        Err(cause) => cause,
    }
}

/// `JEDEN_CONTEXT_LIMIT`: `None` when it is not set, the number of tokens when
/// it names a whole number above zero, refused naming its value otherwise.
/// Read by the session setup for the status line and by `/context`.
pub(super) fn context_limit() -> Result<Option<NonZeroUsize>, String> {
    match env::var("JEDEN_CONTEXT_LIMIT") {
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(value)) => Err(format!(
            "JEDEN_CONTEXT_LIMIT {value:?} is not Unicode; state a whole number of tokens above zero"
        )),
        Ok(value) => value.trim().parse::<NonZeroUsize>().map(Some).map_err(|error| {
            format!("JEDEN_CONTEXT_LIMIT {value:?} is not a whole number of tokens above zero: {error}")
        }),
    }
}
