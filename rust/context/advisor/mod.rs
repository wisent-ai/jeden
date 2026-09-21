//! The context advisor: what an agent should read before it starts working.
//!
//! Jeden already injects discovered context files and remembered notes, but
//! both answer "what is always true here", never "what is relevant to this
//! task". The advisor answers the second question from sources that already
//! hold the answer — the documentation corpus, Jeden's own memory, the
//! Transcript Lake archive, and the Wisent ground-truth index — and returns
//! locators an agent can read directly instead of searching for them.
//!
//! Every source reports its own availability with the exact reason it failed,
//! because a recommendation list that is silently short is indistinguishable
//! from one that is complete.

mod render;
mod settings;
mod sources;
mod text;

use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::cli::config::Config;

pub(crate) use render::{availability_word, prompt_section, probe_value, render_text};
pub(crate) use settings::{
    bounded_limit, bounded_timeout_ms, parse_sources, settings, unknown_sources, Settings,
};
use sources::{files, ground_truth, memory, transcripts};

/// Source order is the presentation order: local and cited sources first,
/// scans last. Interleaving walks this list, so it decides which source wins
/// a tie for the first recommendation.
pub(crate) const SOURCES: &[&str] = &["files", "ground-truth", "memory", "transcripts"];

pub(crate) const DEFAULT_LIMIT: usize = 6;
pub(crate) const DEFAULT_MAX_CHARS: usize = 6_000;
pub(crate) const DEFAULT_TIMEOUT_MS: u64 = 3_000;
/// The deadline the automatic per-turn block uses. Shorter than the one a
/// person waiting at a prompt accepts, because every turn pays it: the local
/// sources finish inside it and a slow archive reports that it did not.
pub(crate) const DEFAULT_PROMPT_TIMEOUT_MS: u64 = 1_000;
/// Every source answers unless the operator narrows them. Sources run
/// concurrently and each one is bounded by the same deadline, so a turn
/// waits for the slowest source rather than for their sum.
pub(crate) const DEFAULT_SOURCES: &str = "all";
/// Which file types the walk reads. Empty means every readable text file —
/// code, configuration and prose alike; a list narrows it.
pub(crate) const DEFAULT_FILE_EXTENSIONS: &str = "";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Recommendation {
    /// Which source produced it: one of `SOURCES`.
    pub(crate) source: String,
    /// What the material is called where it lives — a heading, a session
    /// title, a memory kind.
    pub(crate) title: String,
    /// Exactly what to read: `path:first-last`, `session:<id>`,
    /// `repo/path@ref:first-last`, or `memory:<id>`.
    pub(crate) locator: String,
    pub(crate) score: f64,
    /// Query terms this hit actually matched, so a wrong recommendation can
    /// be explained rather than guessed at.
    pub(crate) matched: Vec<String>,
    pub(crate) snippet: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceStatus {
    pub(crate) source: String,
    pub(crate) available: bool,
    /// The observed state in one sentence: what was read, or what refused and
    /// why. Never empty.
    pub(crate) detail: String,
    pub(crate) considered: usize,
    pub(crate) returned: usize,
    pub(crate) elapsed_ms: u128,
}

impl SourceStatus {
    pub(super) fn unavailable(source: &str, detail: impl Into<String>, started: Instant) -> Self {
        Self {
            source: source.to_string(),
            available: false,
            detail: detail.into(),
            elapsed_ms: started.elapsed().as_millis(),
            ..Self::default()
        }
    }

    pub(super) fn available(
        source: &str,
        detail: impl Into<String>,
        considered: usize,
        returned: usize,
        started: Instant,
    ) -> Self {
        Self {
            source: source.to_string(),
            available: true,
            detail: detail.into(),
            considered,
            returned,
            elapsed_ms: started.elapsed().as_millis(),
        }
    }
}

/// One source's answer: what it found and what state it was in.
pub(crate) struct SourceOutcome {
    pub(crate) hits: Vec<Recommendation>,
    pub(crate) status: SourceStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Advice {
    pub(crate) query: String,
    pub(crate) limit: usize,
    pub(crate) recommendations: Vec<Recommendation>,
    pub(crate) sources: Vec<SourceStatus>,
}

impl Advice {
    pub(crate) fn unavailable_sources(&self) -> Vec<&SourceStatus> {
        self.sources
            .iter()
            .filter(|status| !status.available)
            .collect()
    }
}

pub(crate) struct Request {
    pub(crate) query: String,
    pub(crate) limit: usize,
    pub(crate) sources: Vec<String>,
    pub(crate) timeout: Duration,
}

impl Request {
    pub(crate) fn from_settings(query: &str, settings: &Settings) -> Self {
        Self {
            query: query.trim().to_string(),
            limit: settings.limit,
            sources: settings.sources.clone(),
            timeout: Duration::from_millis(settings.timeout_ms),
        }
    }
}

/// Ask every requested source and interleave what they found.
///
/// Sources run on their own threads because they are independent and
/// unevenly slow: the archive scan takes seconds while memory answers in
/// milliseconds, and a turn should wait for the slowest one rather than for
/// all of them in a row.
pub(crate) fn recommend(cwd: &Path, config: &Config, request: &Request) -> Advice {
    let settings = settings(cwd, config);
    let terms = text::terms(&request.query);
    let selected: Vec<&&str> = SOURCES
        .iter()
        .filter(|source| request.sources.iter().any(|want| want == *source))
        .collect();
    let outcomes: Vec<SourceOutcome> = std::thread::scope(|scope| {
        let handles: Vec<_> = selected
            .iter()
            .map(|source| {
                let source = **source;
                let settings = &settings;
                let terms = &terms;
                scope.spawn(move || ask(source, cwd, settings, request, terms))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| SourceOutcome {
                    hits: Vec::new(),
                    status: SourceStatus::unavailable(
                        "unknown",
                        "the source thread ended without an answer",
                        Instant::now(),
                    ),
                })
            })
            .collect()
    });
    let mut statuses = Vec::with_capacity(outcomes.len());
    let mut per_source = Vec::with_capacity(outcomes.len());
    for outcome in outcomes {
        statuses.push(outcome.status);
        per_source.push(outcome.hits);
    }
    Advice {
        query: request.query.clone(),
        limit: request.limit,
        recommendations: interleave(per_source, request.limit),
        sources: statuses,
    }
}

fn ask(
    source: &str,
    cwd: &Path,
    settings: &Settings,
    request: &Request,
    terms: &[String],
) -> SourceOutcome {
    let started = Instant::now();
    if terms.is_empty() {
        return SourceOutcome {
            hits: Vec::new(),
            status: SourceStatus::unavailable(
                source,
                "the query carries no searchable word of three characters or more",
                started,
            ),
        };
    }
    match source {
        "files" => files::search(settings, request, terms),
        "memory" => memory::search(cwd, terms, &request.query, request.limit),
        "transcripts" => transcripts::search(settings, request, terms),
        "ground-truth" => ground_truth::search(settings, request, terms),
        other => SourceOutcome {
            hits: Vec::new(),
            status: SourceStatus::unavailable(other, "no such source", started),
        },
    }
}

/// Round-robin across sources in `SOURCES` order. Scores are comparable
/// inside one source and not across them, so a global sort would let a
/// verbose document outrank a cited answer for arithmetic reasons.
fn interleave(per_source: Vec<Vec<Recommendation>>, limit: usize) -> Vec<Recommendation> {
    let mut out = Vec::new();
    let mut round = 0usize;
    loop {
        let mut added = false;
        for hits in per_source.iter() {
            if let Some(hit) = hits.get(round).cloned() {
                out.push(hit);
                added = true;
                if out.len() >= limit {
                    return out;
                }
            }
        }
        if !added {
            return out;
        }
        round += 1;
    }
}

/// The advisor's own state, with no query: what each source is configured to
/// be and whether it answers right now.
pub(crate) fn sources_report(cwd: &Path, config: &Config) -> Value {
    let settings = settings(cwd, config);
    let probes = vec![
        files::probe(&settings),
        ground_truth::probe(&settings),
        memory::probe(),
        transcripts::probe(&settings),
    ];
    json!({
        "settings": settings,
        "selected": settings.sources,
        "sources": probes,
    })
}

/// What a turn injects for `task`, or `None` when the advisor is disabled,
/// configured to no source, or has nothing to offer. Never fails a turn: a
/// broken source becomes a reported unavailability, not an error.
pub(crate) fn advice_for_prompt(cwd: &Path, config: &Config, task: &str) -> Option<String> {
    let settings = settings(cwd, config);
    if !settings.enabled || settings.sources.is_empty() {
        return None;
    }
    let mut request = Request::from_settings(task, &settings);
    request.timeout = Duration::from_millis(settings.prompt_timeout_ms);
    if request.query.is_empty() {
        return None;
    }
    prompt_section(&recommend(cwd, config, &request), settings.max_chars)
}
