//! What a criterion means when it names a place.
//!
//! An acceptance criterion usually names the file it is about. Reading the
//! prose is the independent reviewer's job. The place is not, because a place
//! is decidable, and on 2026-09-10 the reviewer twice accepted a file that sat
//! next to the named path instead of at it: a run whose criterion asked for
//! `alpha.txt` in the workspace root was accepted by a write that landed in
//! `workspace/alpha.txt` underneath it.
//!
//! So the controller answers the decidable half itself. It reads the paths a
//! criterion names, the paths an accepted observation touched, and refuses the
//! verdict when no observation happened at the named place. A bare file name
//! is a claim about the workspace root and is matched there exactly; a path of
//! several parts is matched by an observation whose own path ends with those
//! parts, because the same file is spelled from different roots in a receipt.

use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// Longest extension still read as a file name rather than prose.
const MAX_EXTENSION: usize = 8;
/// Last labels that make a dotted word a host instead of a file.
const HOSTS: &[&str] = &["com", "org", "net", "io", "ai", "gov", "edu", "pl", "eu"];
/// Trimmed from both ends of a word before it is read as a path.
const WRAPPERS: &[char] = &[
    '`', '\'', '"', '(', ')', '[', ']', '{', '}', '<', '>', ',', ';', ':', '!', '?', '*', '=',
    '\u{201c}', '\u{201d}', '\u{2018}', '\u{2019}',
];

/// Every path this text names, in the spelling the text used.
pub(super) fn named(text: &str) -> BTreeSet<String> {
    text.split_whitespace().filter_map(token).collect()
}

/// Every path a receipt mentions, wherever the string sits inside it.
pub(super) fn touched(receipt: &Value) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    collect(receipt, &mut found);
    found
}

/// The first named path no observation touched, in its original spelling.
pub(super) fn unmatched(
    wanted: &BTreeSet<String>,
    touched: &BTreeSet<String>,
    workspace: &Path,
) -> Option<String> {
    wanted
        .iter()
        .find(|place| {
            !touched
                .iter()
                .any(|observed| same_place(place, observed, workspace))
        })
        .cloned()
}

fn collect(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => found.extend(named(text)),
        Value::Array(items) => items.iter().for_each(|item| collect(item, found)),
        Value::Object(fields) => fields.values().for_each(|field| collect(field, found)),
        _ => {}
    }
}

/// One word read as a path, or nothing when it is prose, a version or a host.
fn token(word: &str) -> Option<String> {
    let word = word.trim_matches(|character| WRAPPERS.contains(&character));
    let word = word.trim_end_matches('.');
    let word = match word.split_once("://") {
        Some((_, rest)) => rest.find('/').map(|cut| &rest[cut..])?,
        None => word,
    };
    if word.is_empty() || word.contains('*') || word == "/" || word.trim_matches('.').is_empty() {
        return None;
    }
    let word = strip_host(word);
    let directory = word.ends_with('/');
    let trimmed = word.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.contains('/') && !names_file(trimmed) {
        return None;
    }
    Some(if directory {
        format!("{trimmed}/")
    } else {
        trimmed.to_owned()
    })
}

/// `jeden.wisent.com/docs/cli` is a page under `/docs/cli`, not a directory
/// called after the host, and a bare host names no path at all.
fn strip_host(word: &str) -> &str {
    let (head, rest) = match word.split_once('/') {
        Some((head, rest)) => (head, rest),
        None => (word, ""),
    };
    if !is_host(head) {
        return word;
    }
    if rest.is_empty() {
        return "";
    }
    &word[head.len()..]
}

/// A head is a machine, not a directory: `jeden.wisent.com`, `127.0.0.1:17601`
/// and `localhost:8080` all address a service whose path follows the slash.
fn is_host(head: &str) -> bool {
    let head = match head.split_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(char::is_numeric) => host,
        _ => head,
    };
    if head == "localhost" {
        return true;
    }
    let labels: Vec<_> = head.split('.').collect();
    let named_labels = labels.len() > 1
        && labels.iter().all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        });
    let literal = labels.len() == "0.0.0.0".split('.').count()
        && labels
            .iter()
            .all(|label| label.chars().all(char::is_numeric) && !label.is_empty());
    named_labels && (literal || labels.last().is_some_and(|last| HOSTS.contains(last)))
}

/// True for `alpha.txt` and `package.json`, false for `0.1.1` or `127.0.0.1`.
fn names_file(word: &str) -> bool {
    let Some((stem, extension)) = word.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && !extension.is_empty()
        && extension.len() <= MAX_EXTENSION
        && extension
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
        && extension
            .chars()
            .any(|character| character.is_ascii_alphabetic())
}

fn same_place(wanted: &str, observed: &str, workspace: &Path) -> bool {
    let directory = wanted.ends_with('/');
    let wanted = wanted.trim_end_matches('/');
    let here = anchored(wanted, workspace);
    let there = anchored(observed, workspace);
    if inside(&there, &here, directory) {
        return true;
    }
    let spelled_from_a_root = Path::new(wanted).components().count() > 1;
    if spelled_from_a_root && inside(Path::new(observed), Path::new(wanted), directory) {
        return true;
    }
    if spelled_from_a_root
        && Path::new(observed)
            .components()
            .count()
            .ge(&Path::new(wanted).components().count())
        && ends_with(Path::new(observed), Path::new(wanted))
    {
        return true;
    }
    match (std::fs::canonicalize(&here), std::fs::canonicalize(&there)) {
        (Ok(here), Ok(there)) => inside(&there, &here, directory),
        _ => false,
    }
}

/// The observation's own path, read from the workspace the request named.
fn anchored(token: &str, workspace: &Path) -> PathBuf {
    let path = Path::new(token);
    if path.is_absolute() {
        lexical(path)
    } else {
        lexical(&workspace.join(path))
    }
}

fn lexical(path: &Path) -> PathBuf {
    let mut walked = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                walked.pop();
            }
            kept => walked.push(kept.as_os_str()),
        }
    }
    walked
}

fn inside(there: &Path, here: &Path, directory: bool) -> bool {
    there == here || (directory && there.starts_with(here))
}

fn ends_with(observed: &Path, wanted: &Path) -> bool {
    let observed: Vec<_> = observed.components().collect();
    let wanted: Vec<_> = wanted.components().collect();
    observed.len() >= wanted.len() && observed[observed.len() - wanted.len()..] == wanted[..]
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
