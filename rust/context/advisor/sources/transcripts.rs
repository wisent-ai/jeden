//! The transcript source: what was already said about this task in an
//! earlier session, read through the Transcript Lake CLI.
//!
//! Transcript Lake owns the masked canonical archive, so this module runs its
//! command rather than reading its files: one owner, one reader, and the
//! masking stays applied. The scan is seconds slow on a 16 GB archive, which
//! is why `transcripts` is opt-in for the automatic advisory: the source
//! selection, not a deadline, is what decides whether a turn pays for it.

use std::process::Command;
use std::time::Instant;

use serde_json::{json, Value};

use crate::context::advisor::text::{command_output, matched_terms, snippet};
use crate::context::advisor::{
    probe_value, Recommendation, Request, Settings, SourceOutcome, SourceStatus,
};

pub(crate) fn search(settings: &Settings, request: &Request, terms: &[String]) -> SourceOutcome {
    let started = Instant::now();
    let refused = |detail: String| SourceOutcome {
        hits: Vec::new(),
        status: SourceStatus::unavailable("transcripts", detail, started),
    };
    let mut command = Command::new(&settings.transcript_lake_bin);
    command
        .arg("search")
        .arg(&request.query)
        .arg("--limit")
        .arg(request.limit.to_string())
        .arg("--json");
    let text = match command_output(command, &request.cancellation) {
        Ok(text) => text,
        Err(error) => return refused(format!("{} search {error}", settings.transcript_lake_bin)),
    };
    let rows = match json_tail(&text) {
        Ok(Value::Array(rows)) => rows,
        Ok(other) => {
            return refused(format!(
                "{} search returned no JSON array: {other}",
                settings.transcript_lake_bin
            ))
        }
        Err(error) => return refused(format!("{} search {error}", settings.transcript_lake_bin)),
    };
    let considered = rows.len();
    let mut seen: Vec<String> = Vec::new();
    let mut hits = Vec::new();
    // An event the archive keeps without a session has no locator to read it
    // by; it is counted in the detail, not recommended.
    let mut sessionless = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let Some(session) = row
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|session| !session.is_empty())
        else {
            sessionless.push(index);
            continue;
        };
        if seen.iter().any(|kept| kept == session) {
            continue;
        }
        let runtime = row.get("runtime").and_then(Value::as_str);
        let stamp = match row.get("ts").and_then(Value::as_str) {
            Some(stamp) => format!("on {stamp}"),
            None => "at no recorded time".to_string(),
        };
        // The archive answers newest first, so position is recency, not
        // relevance. Naming it a rank keeps that visible in the output.
        let rank = considered.saturating_sub(index) as f64;
        let (matched, excerpt) = match row.get("text").and_then(Value::as_str) {
            Some(body) => (matched_terms(body, terms), snippet(body, terms)),
            None => (Vec::new(), "the event carries no text".to_string()),
        };
        hits.push(Recommendation {
            source: "transcripts".to_string(),
            title: match runtime {
                Some(runtime) => format!("{runtime} session {stamp}"),
                None => format!("session of an unreported runtime {stamp}"),
            },
            locator: format!("session:{session}"),
            score: rank,
            matched,
            snippet: excerpt,
        });
        seen.push(session.to_string());
        if hits.len() >= request.limit {
            break;
        }
    }
    let mut detail = format!(
        "{} search matched {} event(s) across {} session(s)",
        settings.transcript_lake_bin,
        considered,
        hits.len()
    );
    if !sessionless.is_empty() {
        detail.push_str(&format!(
            "; {} event(s) name no session and cannot be read by locator",
            sessionless.len()
        ));
    }
    SourceOutcome {
        status: SourceStatus::available("transcripts", detail, considered, hits.len(), started),
        hits,
    }
}

/// The archive's own state: which runtimes it holds and how much of each.
pub(crate) fn probe(
    settings: &Settings,
    cancellation: &crate::tool_runtime::runtime_ops::CancellationToken,
) -> Value {
    let started = Instant::now();
    let bin = &settings.transcript_lake_bin;
    let unavailable = |detail: String| SourceStatus::unavailable("transcripts", detail, started);
    let mut command = Command::new(bin);
    command.arg("status").arg("--json");
    let status = match command_output(command, cancellation).map(|text| json_tail(&text)) {
        Err(error) | Ok(Err(error)) => unavailable(format!("{bin} status {error}")),
        Ok(Ok(document)) => match document.get("partitions").and_then(Value::as_array) {
            None => unavailable(format!("{bin} status returned no partitions array")),
            Some(rows) if rows.is_empty() => {
                unavailable("the archive is reachable and holds no runtime partition".into())
            }
            // Every partition states its whole part count; one that does not
            // is a reply this probe cannot count, not a partition of none.
            Some(rows) => match rows
                .iter()
                .map(|row| {
                    row.get("parts")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| format!("a partition states no part count: {row}"))
                })
                .sum::<Result<u64, String>>()
                .and_then(|parts| usize::try_from(parts).map_err(|error| error.to_string()))
            {
                Ok(parts) => SourceStatus::probed(
                    "transcripts",
                    format!("{} runtime partition(s), {parts} part file(s)", rows.len()),
                    Some(parts),
                    started,
                ),
                Err(error) => unavailable(format!("{bin} status: {error}")),
            },
        },
    };
    probe_value(&status, vec![("command", json!(bin.clone()))])
}

/// Transcript Lake prints a human freshness table before its JSON, so the
/// document starts at the first bracket rather than at byte zero.
fn json_tail(text: &str) -> Result<Value, String> {
    let start = text
        .char_indices()
        .find(|(_, character)| *character == '[' || *character == '{')
        .map(|(index, _)| index)
        .ok_or("returned no JSON document")?;
    serde_json::from_str(&text[start..])
        .map_err(|error| format!("returned a JSON document that cannot be read: {error}"))
}
