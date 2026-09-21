//! The advisor's resolved configuration. Every default is applied here once,
//! so no caller re-derives one and `jeden context sources` can print exactly
//! what the next recommendation will use.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{
    sources::docs, DEFAULT_DOC_EXTENSIONS, DEFAULT_LIMIT, DEFAULT_MAX_CHARS, DEFAULT_SOURCES,
    DEFAULT_TIMEOUT_MS, SOURCES,
};
use crate::cli::config::{AdvisorConfig, Config};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Settings {
    pub(crate) enabled: bool,
    pub(crate) limit: usize,
    pub(crate) max_chars: usize,
    pub(crate) timeout_ms: u64,
    pub(crate) sources: Vec<String>,
    pub(crate) roots: Vec<DocRoot>,
    pub(crate) doc_extensions: Vec<String>,
    /// Empty when no ground-truth endpoint is configured anywhere.
    pub(crate) ground_truth_url: String,
    pub(crate) ground_truth_origin: String,
    pub(crate) transcript_lake_bin: String,
}

/// A documentation root and how deep the walk may go from it. Depth is part
/// of the declaration because a root like `$HOME` is only usable bounded.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DocRoot {
    pub(crate) path: PathBuf,
    pub(crate) depth: usize,
}

pub(crate) fn settings(cwd: &Path, config: &Config) -> Settings {
    let advisor: &AdvisorConfig = &config.context.advisor;
    let (ground_truth_url, ground_truth_origin) = resolve_ground_truth(advisor);
    Settings {
        enabled: advisor.enabled,
        limit: bounded_limit(advisor.limit),
        max_chars: bounded_max_chars(advisor.max_chars),
        timeout_ms: bounded_timeout_ms(advisor.timeout_ms),
        sources: parse_sources(&advisor.sources),
        roots: parse_roots(&advisor.roots, cwd),
        doc_extensions: parse_extensions(&advisor.doc_extensions),
        ground_truth_url,
        ground_truth_origin,
        transcript_lake_bin: declared_or(&advisor.transcript_lake_bin, "transcript-lake"),
    }
}

fn declared_or(value: &str, declared_name: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        declared_name.to_string()
    } else {
        value.to_string()
    }
}

pub(crate) fn bounded_limit(limit: usize) -> usize {
    const MAX: usize = 50;
    if limit == 0 {
        DEFAULT_LIMIT
    } else {
        limit.min(MAX)
    }
}

pub(crate) fn bounded_max_chars(max_chars: usize) -> usize {
    const MIN: usize = 400;
    const MAX: usize = 60_000;
    if max_chars == 0 {
        DEFAULT_MAX_CHARS
    } else {
        max_chars.clamp(MIN, MAX)
    }
}

pub(crate) fn bounded_timeout_ms(timeout_ms: u64) -> u64 {
    const MIN: u64 = 100;
    const MAX: u64 = 120_000;
    if timeout_ms == 0 {
        DEFAULT_TIMEOUT_MS
    } else {
        timeout_ms.clamp(MIN, MAX)
    }
}

/// The endpoint plus where it came from, so `jeden context sources` can say
/// "config" or "WISENT_GROUND_TRUTH_API" rather than only printing a URL.
/// The two environment variables are the ones the fleet's existing
/// ground-truth consumers already read.
fn resolve_ground_truth(advisor: &AdvisorConfig) -> (String, String) {
    let declared = advisor.ground_truth_url.trim();
    if !declared.is_empty() {
        return (
            declared.trim_end_matches('/').to_string(),
            "config context.advisor.groundTruthUrl".to_string(),
        );
    }
    for name in ["WISENT_GROUND_TRUTH_API", "GROUND_TRUTH_API"] {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim();
            if !value.is_empty() {
                return (value.trim_end_matches('/').to_string(), name.to_string());
            }
        }
    }
    (String::new(), "unset".to_string())
}

/// `docs,memory` and `docs, memory` and `all` all name source sets. Unknown
/// names are dropped here and reported by `unknown_sources`.
pub(crate) fn parse_sources(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return parse_sources(DEFAULT_SOURCES);
    }
    if raw.eq_ignore_ascii_case("all") {
        return SOURCES.iter().map(|source| source.to_string()).collect();
    }
    let mut chosen: Vec<String> = Vec::new();
    for part in raw.split([',', ' ']) {
        let name = canonical_source(part);
        if name.is_empty() {
            continue;
        }
        if SOURCES.contains(&name.as_str()) && !chosen.contains(&name) {
            chosen.push(name);
        }
    }
    chosen
}

fn canonical_source(part: &str) -> String {
    match part.trim().to_ascii_lowercase().as_str() {
        "" => String::new(),
        "groundtruth" | "ground_truth" => "ground-truth".to_string(),
        other => other.to_string(),
    }
}

/// Declared documentation extensions, without their dots and lowercased, so
/// `.MD` and `md` are the same declaration.
fn parse_extensions(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    let declared = if raw.is_empty() {
        DEFAULT_DOC_EXTENSIONS
    } else {
        raw
    };
    let mut out: Vec<String> = Vec::new();
    for part in declared.split([',', ' ']) {
        let name = part.trim().trim_start_matches('.').to_ascii_lowercase();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Names in `raw` that are not sources, so an operator typo is refused rather
/// than silently narrowing the answer.
pub(crate) fn unknown_sources(raw: &str) -> Vec<String> {
    if raw.trim().is_empty() || raw.trim().eq_ignore_ascii_case("all") {
        return Vec::new();
    }
    raw.split([',', ' '])
        .map(canonical_source)
        .filter(|part| !part.is_empty())
        .filter(|part| !SOURCES.contains(&part.as_str()))
        .collect()
}

/// `path` or `path@depth`, colon-separated, `~` expanded, relative entries
/// resolved against the working directory. The declared default pair is the
/// project itself and the operator's own Jeden instructions.
fn parse_roots(raw: &str, cwd: &Path) -> Vec<DocRoot> {
    const MAX_DEPTH: usize = 32;
    let raw = raw.trim();
    let mut roots: Vec<DocRoot> = Vec::new();
    let declared: Vec<&str> = if raw.is_empty() {
        vec![".", "~/.jeden"]
    } else {
        raw.split(':').collect()
    };
    for entry in declared {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (path_part, depth) = split_root_depth(entry);
        let expanded = expand_home(path_part);
        let path = if expanded.is_absolute() {
            expanded
        } else {
            cwd.join(expanded)
        };
        let path = path.canonicalize().unwrap_or(path);
        if roots.iter().any(|root| root.path == path) {
            continue;
        }
        roots.push(DocRoot {
            path,
            depth: depth.clamp(1, MAX_DEPTH),
        });
    }
    roots
}

fn split_root_depth(entry: &str) -> (&str, usize) {
    match entry.rsplit_once('@') {
        Some((path, depth))
            if !depth.is_empty() && depth.chars().all(|character| character.is_ascii_digit()) =>
        {
            (path, depth.parse().unwrap_or(docs::DEFAULT_DEPTH))
        }
        _ => (entry, docs::DEFAULT_DEPTH),
    }
}

fn expand_home(value: &str) -> PathBuf {
    if value == "~" {
        return crate::dirs_home();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return crate::dirs_home().join(rest);
    }
    PathBuf::from(value)
}
