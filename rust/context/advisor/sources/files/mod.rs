//! The files source: everything readable under the declared roots —
//! documentation, source code, configuration and manifests — cut into chunks
//! and ranked against the query.

mod corpus;
mod rank;

use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};

use crate::context::advisor::text::{matched_terms, snippet};
use crate::context::advisor::{
    probe_value, Recommendation, Request, Settings, SourceOutcome, SourceStatus,
};
use corpus::Corpus;
use rank::NO_MATCH;

pub(crate) const DEFAULT_DEPTH: usize = 6;
const SNIPPET_LINES: usize = 3;
const SNIPPET_CHARS: usize = 420;

pub(crate) fn search(settings: &Settings, request: &Request, terms: &[String]) -> SourceOutcome {
    let started = Instant::now();
    if settings.roots.is_empty() {
        return SourceOutcome {
            hits: Vec::new(),
            status: SourceStatus::unavailable(
                "files",
                "no root is declared in context.advisor.roots",
                started,
            ),
        };
    }
    let corpus = Corpus::read(settings);
    if corpus.sections.is_empty() {
        return SourceOutcome {
            hits: Vec::new(),
            status: SourceStatus::unavailable("files", corpus.empty_detail(), started),
        };
    }
    let weighed = rank::weigh(&corpus.sections, terms);
    let informative = rank::informative_terms(&weighed);
    let mut scored: Vec<(f64, Recommendation)> = Vec::new();
    for section in &corpus.sections {
        let score = rank::score_section(section, &weighed);
        if score <= NO_MATCH {
            continue;
        }
        let haystack = format!(
            "{} {} {}",
            section.path.display(),
            section.title,
            section.body
        );
        scored.push((
            score,
            Recommendation {
                source: "files".to_string(),
                title: section.title.clone(),
                locator: format!(
                    "{}:{}-{}",
                    display_path(&section.path),
                    section.first_line,
                    section.last_line
                ),
                score,
                matched: matched_terms(&haystack, terms),
                snippet: snippet(&section.body, &informative, SNIPPET_LINES, SNIPPET_CHARS),
            },
        ));
    }
    scored.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.1.locator.cmp(&right.1.locator))
    });
    let considered = corpus.sections.len();
    let hits: Vec<Recommendation> = scored
        .into_iter()
        .map(|(_, hit)| hit)
        .take(request.limit)
        .collect();
    SourceOutcome {
        status: SourceStatus::available("files", detail(&corpus), considered, hits.len(), started),
        hits,
    }
}

fn detail(corpus: &Corpus) -> String {
    let mut detail = format!(
        "read {} chunk(s) from {} file(s) under {} root(s)",
        corpus.sections.len(),
        corpus.files,
        corpus.existing_roots.len()
    );
    if corpus.truncated {
        detail.push_str("; the walk stopped at its cap, so this is a partial corpus");
    }
    if !corpus.missing_roots.is_empty() {
        detail.push_str(&format!(
            "; missing root(s): {}",
            corpus.missing_roots.join(", ")
        ));
    }
    detail
}

/// What the source is, without a query: which roots exist and how much text
/// they actually carry.
pub(crate) fn probe(settings: &Settings) -> Value {
    let started = Instant::now();
    let corpus = Corpus::read(settings);
    let status = if corpus.sections.is_empty() {
        SourceStatus::unavailable("files", corpus.empty_detail(), started)
    } else {
        SourceStatus::available(
            "files",
            detail(&corpus),
            corpus.sections.len(),
            usize::default(),
            started,
        )
    };
    probe_value(
        &status,
        vec![
            ("roots", json!(settings.roots)),
            ("extensions", json!(settings.file_extensions)),
            ("existingRoots", json!(corpus.existing_roots)),
            ("missingRoots", json!(corpus.missing_roots)),
            ("files", json!(corpus.files)),
            ("truncated", json!(corpus.truncated)),
        ],
    )
}

/// Shortest honest way to name the file: relative to the working directory
/// when it is inside it, `~`-prefixed when it is under the home directory.
fn display_path(path: &Path) -> String {
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(relative) = path.strip_prefix(&cwd) {
            return relative.display().to_string();
        }
    }
    if let Ok(relative) = path.strip_prefix(crate::dirs_home()) {
        return format!("~/{}", relative.display());
    }
    path.display().to_string()
}
