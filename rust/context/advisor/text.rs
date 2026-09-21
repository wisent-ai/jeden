//! Shared text work: which words of a query are searchable, how a snippet is
//! cut, and how a source process is run to completion.
//!
//! There is deliberately no list of words to ignore. A hand-written stop list
//! is a classifier nobody declared, and it is also unnecessary: the documents
//! themselves say which words carry information, because a word that occurs
//! in most of the corpus separates nothing. `files::weights` computes that
//! from the corpus it just read, so "the" and "jak" fall out by measurement
//! rather than by opinion.

use std::process::{Command, Stdio};

/// Searchable words of a query: lowercase, three characters or more, in first
/// occurrence order, bounded so a pasted paragraph cannot become a thousand
/// term scans.
pub(crate) fn terms(query: &str) -> Vec<String> {
    const MIN_WORD: usize = 3;
    const MAX_TERMS: usize = 24;
    let mut out: Vec<String> = Vec::new();
    for raw in query.split(|character: char| !character.is_alphanumeric()) {
        let word = raw.trim().to_lowercase();
        if word.chars().count() < MIN_WORD {
            continue;
        }
        if !out.contains(&word) {
            out.push(word);
        }
        if out.len() >= MAX_TERMS {
            break;
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

/// One snippet, bounded, preferring the lines that carry a query term or its
/// stem.
pub(crate) fn snippet(body: &str, terms: &[String], max_lines: usize, max_chars: usize) -> String {
    const MIN_TAIL: usize = 16;
    let mut lines: Vec<&str> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if mentions(trimmed, terms) {
            lines.push(trimmed);
        }
        if lines.len() >= max_lines {
            break;
        }
    }
    if lines.is_empty() {
        lines = body
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(max_lines)
            .collect();
    }
    let mut out = String::new();
    for line in lines {
        let remaining = max_chars.saturating_sub(out.chars().count());
        if remaining < MIN_TAIL {
            break;
        }
        let clipped: String = line.chars().take(remaining).collect();
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&clipped);
    }
    out
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
        .filter(|term| {
            lower.contains(term.as_str()) || lower.contains(stem(term).as_str())
        })
        .cloned()
        .collect()
}

/// Run `command` to completion and return its stdout.
///
/// Nothing here cuts the work short. A search that takes ten seconds takes
/// ten seconds and answers; a guessed interval would have reported nothing
/// and told the reader nothing about why. A failure is the command's own
/// failure, with the first line it wrote to stderr.
pub(crate) fn command_output(mut command: Command) -> Result<String, String> {
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("no stderr")
            .to_string();
        return Err(format!("exited with {}: {detail}", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
