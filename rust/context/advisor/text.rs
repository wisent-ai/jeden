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

/// Run `command` to completion and return its stdout.
///
/// Nothing here cuts the work short. A search that takes ten seconds takes
/// ten seconds and answers; a guessed interval would have reported nothing
/// and told the reader nothing about why. The shared wait diagnostic names
/// the child before waiting; a failure retains all of its stderr, including
/// the cause after progress messages.
pub(crate) fn command_output(mut command: Command) -> Result<String, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = stado_wait::output(&mut command).map_err(|error| error.to_string())?;
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
