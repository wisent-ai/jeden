//! Shared text work: which words of a query are searchable, how a snippet is
//! cut, and how a source process is run to completion.
//!
//! There is deliberately no list of words to ignore. A hand-written stop list
//! is a classifier nobody declared, and it is also unnecessary: the documents
//! themselves say which words carry information, because a word that occurs
//! in most of the corpus separates nothing. `files::weights` computes that
//! from the corpus it just read, so "the" and "jak" fall out by measurement
//! rather than by opinion.

use std::num::NonZeroUsize;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::tool_runtime::runtime_ops::CancellationToken;

static PROCESS_RUNTIME: Mutex<Option<Arc<tokio::runtime::Runtime>>> = Mutex::new(None);

pub(crate) fn process_runtime() -> Result<Arc<tokio::runtime::Runtime>, String> {
    let mut slot = PROCESS_RUNTIME
        .lock()
        .map_err(|error| format!("context process runtime lock failed: {error}"))?;
    if let Some(runtime) = &*slot {
        return Ok(Arc::clone(runtime));
    }
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("context process runtime creation failed: {error}"))?,
    );
    *slot = Some(Arc::clone(&runtime));
    Ok(runtime)
}

/// Searchable words of a query: lowercase, in first occurrence order. Which
/// of them carry information is measured against the corpus, not decided by
/// their length.
pub(crate) fn terms(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in query.split(|character: char| !character.is_alphanumeric()) {
        let word = raw.trim().to_lowercase();
        if word.is_empty() {
            continue;
        }
        if !out.contains(&word) {
            out.push(word);
        }
    }
    out
}

/// A term with its last two characters dropped, which is how one term
/// matches its own inflections: `routingu` finds `routing`, `signing` finds
/// `signed`. Short terms are returned unchanged, because two characters off a
/// four-letter word is noise rather than a stem.
pub(crate) fn stem(term: &str) -> String {
    const MIN_STEMMABLE: usize = 6;
    const DROPPED: usize = 2;
    let length = term.chars().count();
    if length < MIN_STEMMABLE {
        return term.to_string();
    }
    term.chars().take(length - DROPPED).collect()
}

/// The lines of `body` that carry a query term or its stem, whole; a body
/// that mentions none is represented by its first non-empty line.
pub(crate) fn snippet(body: &str, terms: &[String]) -> String {
    let lines = body.lines().map(str::trim).filter(|line| !line.is_empty());
    let mentioning: Vec<&str> = lines.clone().filter(|line| mentions(line, terms)).collect();
    if mentioning.is_empty() {
        return lines.take(NonZeroUsize::MIN.get()).collect();
    }
    mentioning.join("\n")
}

/// Whether any term, or its stem, occurs in `text`.
pub(crate) fn mentions(text: &str, terms: &[String]) -> bool {
    let lower = text.to_lowercase();
    terms
        .iter()
        .any(|term| lower.contains(term.as_str()) || lower.contains(stem(term).as_str()))
}

/// Terms that occur in `haystack`, exactly or as a stem, for
/// `Recommendation::matched`.
pub(crate) fn matched_terms(haystack: &str, terms: &[String]) -> Vec<String> {
    let lower = haystack.to_lowercase();
    terms
        .iter()
        .filter(|term| lower.contains(term.as_str()) || lower.contains(stem(term).as_str()))
        .cloned()
        .collect()
}

/// Run the declared command with its original environment and directory.
/// Caller cancellation ends the owned process group and reaps the child
/// through Stado before this returns; ordinary failures retain all stderr.
pub(crate) fn command_output(
    mut command: Command,
    cancellation: &CancellationToken,
) -> Result<String, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if cancellation.is_cancelled() {
        return Err("cancelled by its caller before the process started".to_string());
    }
    let runtime = process_runtime()?;
    let cancellation = cancellation.for_stado();
    let mut command = tokio::process::Command::from(command);
    let output = runtime
        .block_on(stado_wait::output_cancellable(
            &mut command,
            &cancellation.event,
        ))
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let detail = if detail.is_empty() {
            "no stderr"
        } else {
            detail
        };
        return Err(format!("exited with {}: {detail}", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
