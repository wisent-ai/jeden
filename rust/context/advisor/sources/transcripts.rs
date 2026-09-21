//! The transcript source: what was already said about this task in an
//! earlier session, read through the Transcript Lake CLI.
//!
//! Transcript Lake owns the masked canonical archive, so this module runs its
//! command rather than reading its files: one owner, one reader, and the
//! masking stays applied. The scan is seconds slow on a 16 GB archive, which
//! is why `transcripts` is opt-in for the automatic advisory and always
//! bounded by the configured deadline.

use std::process::Command;
use std::time::Instant;

use serde_json::{json, Value};

use crate::context::advisor::text::{bounded_output, matched_terms, snippet};
use crate::context::advisor::{
    probe_value, Recommendation, Request, Settings, SourceOutcome, SourceStatus,
};

const SNIPPET_LINES: usize = 2;
const SNIPPET_CHARS: usize = 300;

pub(crate) fn search(settings: &Settings, request: &Request, terms: &[String]) -> SourceOutcome {
    let started = Instant::now();
    let mut command = Command::new(&settings.transcript_lake_bin);
    command
        .arg("search")
        .arg(&request.query)
        .arg("--limit")
        .arg(request.limit.to_string())
        .arg("--json");
    let text = match bounded_output(command, request.timeout) {
        Ok(text) => text,
        Err(error) => {
            return SourceOutcome {
                hits: Vec::new(),
                status: SourceStatus::unavailable(
                    "transcripts",
                    format!("{} search {error}", settings.transcript_lake_bin),
                    started,
                ),
            }
        }
    };
    let rows = match json_tail(&text) {
        Some(Value::Array(rows)) => rows,
        _ => {
            return SourceOutcome {
                hits: Vec::new(),
                status: SourceStatus::unavailable(
                    "transcripts",
                    format!(
                        "{} search returned no JSON array",
                        settings.transcript_lake_bin
                    ),
                    started,
                ),
            }
        }
    };
    let considered = rows.len();
    let mut seen: Vec<String> = Vec::new();
    let mut hits = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let session = row
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if session.is_empty() || seen.contains(&session) {
            continue;
        }
        let body = row.get("text").and_then(Value::as_str).unwrap_or_default();
        let runtime = row
            .get("runtime")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let stamp = row.get("ts").and_then(Value::as_str).unwrap_or("undated");
        // The archive answers newest first, so position is recency, not
        // relevance. Naming it a rank keeps that visible in the output.
        let rank = considered.saturating_sub(index) as f64;
        hits.push(Recommendation {
            source: "transcripts".to_string(),
            title: format!("{runtime} session on {stamp}"),
            locator: format!("session:{session}"),
            score: rank,
            matched: matched_terms(body, terms),
            snippet: snippet(body, terms, SNIPPET_LINES, SNIPPET_CHARS),
        });
        seen.push(session);
        if hits.len() >= request.limit {
            break;
        }
    }
    let detail = format!(
        "{} search matched {} event(s) across {} session(s)",
        settings.transcript_lake_bin,
        considered,
        hits.len()
    );
    SourceOutcome {
        status: SourceStatus::available("transcripts", detail, considered, hits.len(), started),
        hits,
    }
}

/// The archive's own state: which runtimes it holds and how much of each.
pub(crate) fn probe(settings: &Settings) -> Value {
    let started = Instant::now();
    let mut command = Command::new(&settings.transcript_lake_bin);
    command.arg("status").arg("--json");
    let status = match bounded_output(command, std::time::Duration::from_millis(settings.timeout_ms))
    {
        Ok(text) => match json_tail(&text) {
            Some(document) => {
                let partitions = document
                    .get("partitions")
                    .and_then(Value::as_array)
                    .map(|rows| rows.len())
                    .unwrap_or_default();
                let parts: i64 = document
                    .get("partitions")
                    .and_then(Value::as_array)
                    .map(|rows| {
                        rows.iter()
                            .filter_map(|row| row.get("parts").and_then(Value::as_i64))
                            .sum()
                    })
                    .unwrap_or_default();
                if partitions > usize::default() {
                    SourceStatus::available(
                        "transcripts",
                        format!("{partitions} runtime partition(s), {parts} part file(s)"),
                        parts as usize,
                        usize::default(),
                        started,
                    )
                } else {
                    SourceStatus::unavailable(
                        "transcripts",
                        "the archive is reachable and holds no runtime partition",
                        started,
                    )
                }
            }
            None => SourceStatus::unavailable(
                "transcripts",
                format!(
                    "{} status returned no JSON document",
                    settings.transcript_lake_bin
                ),
                started,
            ),
        },
        Err(error) => SourceStatus::unavailable(
            "transcripts",
            format!("{} status {error}", settings.transcript_lake_bin),
            started,
        ),
    };
    probe_value(
        &status,
        vec![("command", json!(settings.transcript_lake_bin.clone()))],
    )
}

/// Transcript Lake prints a human freshness table before its JSON, so the
/// document starts at the first bracket rather than at byte zero.
fn json_tail(text: &str) -> Option<Value> {
    let start = text
        .char_indices()
        .find(|(_, character)| *character == '[' || *character == '{')
        .map(|(index, _)| index)?;
    serde_json::from_str(&text[start..]).ok()
}
