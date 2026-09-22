//! The memory source: what earlier Jeden sessions wrote down for this
//! repository.
//!
//! Ranking belongs to `crate::memory`, which owns the store's FTS index and
//! its own scoring. This module only turns recalled records into
//! recommendations and reports the store's observed state.

use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};

use crate::context::advisor::text::{matched_terms, snippet};
use crate::context::advisor::{probe_value, Recommendation, SourceOutcome, SourceStatus};
use crate::memory::{FtsBackend, MemoryScope, MemoryStore};

const SNIPPET_LINES: usize = 3;
const SNIPPET_CHARS: usize = 420;

fn scope(cwd: &Path) -> MemoryScope {
    MemoryScope {
        kind: "repo".to_string(),
        id: cwd.display().to_string(),
    }
}

pub(crate) fn search(cwd: &Path, terms: &[String], query: &str, limit: usize) -> SourceOutcome {
    let started = Instant::now();
    let store = match MemoryStore::open(MemoryStore::default_path()) {
        Ok(store) => store,
        Err(error) => {
            return SourceOutcome {
                hits: Vec::new(),
                status: SourceStatus::unavailable(
                    "memory",
                    format!(
                        "{} could not be opened: {error}",
                        MemoryStore::default_path().display()
                    ),
                    started,
                ),
            }
        }
    };
    let scope = scope(cwd);
    let recalled = match store.recall(&FtsBackend, &scope, query, limit) {
        Ok(hits) => hits,
        Err(error) => {
            return SourceOutcome {
                hits: Vec::new(),
                status: SourceStatus::unavailable(
                    "memory",
                    format!("recall failed for scope repo:{}: {error}", scope.id),
                    started,
                ),
            }
        }
    };
    let considered = recalled.len();
    let hits: Vec<Recommendation> = recalled
        .into_iter()
        .map(|hit| Recommendation {
            source: "memory".to_string(),
            title: format!("{} ({})", hit.record.kind, hit.record.source.origin),
            locator: format!("memory:{}", hit.record.id),
            score: hit.score,
            matched: matched_terms(&hit.record.text, terms),
            snippet: snippet(&hit.record.text, terms, SNIPPET_LINES, SNIPPET_CHARS),
        })
        .collect();
    let detail = if hits.is_empty() {
        format!(
            "no memory in scope repo:{} matched; the store holds {}",
            scope.id,
            active_count(&store)
        )
    } else {
        format!("recalled from scope repo:{}", scope.id)
    };
    SourceOutcome {
        status: SourceStatus::available("memory", detail, considered, hits.len(), started),
        hits,
    }
}

/// The store's own health, so an empty recommendation list can be told apart
/// from an empty store.
pub(crate) fn probe() -> Value {
    let started = Instant::now();
    let path = MemoryStore::default_path();
    let status = match MemoryStore::open(&path) {
        Ok(store) => match store.health() {
            Ok(health) => {
                let memories = health
                    .get("memories")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                if memories > i64::default() {
                    SourceStatus::available(
                        "memory",
                        format!("{memories} active memory record(s)"),
                        memories as usize,
                        usize::default(),
                        started,
                    )
                } else {
                    SourceStatus::unavailable(
                        "memory",
                        "the store is reachable and holds no active memory record",
                        started,
                    )
                }
            }
            Err(error) => {
                SourceStatus::unavailable("memory", format!("health read failed: {error}"), started)
            }
        },
        Err(error) => SourceStatus::unavailable(
            "memory",
            format!("{} could not be opened: {error}", path.display()),
            started,
        ),
    };
    probe_value(&status, vec![("store", json!(path.display().to_string()))])
}

fn active_count(store: &MemoryStore) -> String {
    match store.health() {
        Ok(health) => format!(
            "{} active record(s)",
            health
                .get("memories")
                .and_then(Value::as_i64)
                .unwrap_or_default()
        ),
        Err(error) => format!("an unreadable health state ({error})"),
    }
}
