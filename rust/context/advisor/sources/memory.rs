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

fn scope(cwd: &Path) -> MemoryScope {
    MemoryScope {
        kind: "repo".to_string(),
        id: cwd.display().to_string(),
    }
}

pub(crate) fn search(cwd: &Path, terms: &[String], query: &str, limit: usize) -> SourceOutcome {
    let started = Instant::now();
    let store = match MemoryStore::open() {
        Ok(store) => store,
        Err(error) => {
            return SourceOutcome {
                hits: Vec::new(),
                status: SourceStatus::unavailable(
                    "memory",
                    format!("memory could not be opened: {error}"),
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
            snippet: snippet(&hit.record.text, terms),
        })
        .collect();
    let detail = if hits.is_empty() {
        format!("no memory in scope repo:{} matched", scope.id)
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
    let location = "JEDEN_DATABASE_URL";
    let status = match MemoryStore::open() {
        Ok(store) => match store.health() {
            Ok(health) => match health.get("activeMemories").and_then(Value::as_u64) {
                Some(memories) => match usize::try_from(memories) {
                    Ok(considered) => SourceStatus::probed(
                        "memory",
                        format!("{memories} active memory record(s)"),
                        Some(considered),
                        started,
                    ),
                    Err(error) => SourceStatus::unavailable(
                        "memory",
                        format!("health activeMemories {memories} cannot be represented: {error}"),
                        started,
                    ),
                },
                None => SourceStatus::unavailable(
                    "memory",
                    format!(
                        "health reply requires a non-negative integer activeMemories; observed {:?}",
                        health.get("activeMemories")
                    ),
                    started,
                ),
            },
            Err(error) => {
                SourceStatus::unavailable("memory", format!("health read failed: {error}"), started)
            }
        },
        Err(error) => SourceStatus::unavailable(
            "memory",
            format!("memory could not be opened: {error}"),
            started,
        ),
    };
    probe_value(&status, vec![("store", json!(location))])
}
