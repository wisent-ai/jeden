//! The ground-truth source: the Wisent cross-repository index, which answers
//! with repository, path, commit and line citations.
//!
//! `wisent-ground-truth-api` owns the index and the citation contract; this
//! module is a client of its `/search` route and nothing else. When no
//! endpoint is configured, that is reported as the configuration it is, not
//! as an empty result.

use std::time::Instant;

use serde_json::{json, Value};
use stado_wait::Kind;

use crate::context::advisor::text::{matched_terms, process_runtime, snippet};
use crate::context::advisor::{
    probe_value, Recommendation, Request, Settings, SourceOutcome, SourceStatus,
};
use crate::tool_runtime::runtime_ops::CancellationToken;

const NO_ENDPOINT: &str =
    "no endpoint: set context.advisor.groundTruthUrl or WISENT_GROUND_TRUTH_API";

pub(crate) fn search(settings: &Settings, request: &Request, terms: &[String]) -> SourceOutcome {
    let started = Instant::now();
    if settings.ground_truth_url.is_empty() {
        return SourceOutcome {
            hits: Vec::new(),
            status: SourceStatus::unavailable("ground-truth", NO_ENDPOINT, started),
        };
    }
    let url = format!("{}/search", settings.ground_truth_url);
    let query = [
        ("q", request.query.clone()),
        ("limit", request.limit.to_string()),
    ];
    let unavailable = |reason: String| SourceOutcome {
        hits: Vec::new(),
        status: SourceStatus::unavailable("ground-truth", reason, started),
    };
    let (status, body) = match get(&url, &query, &request.cancellation) {
        Ok(answer) => answer,
        Err(error) => return unavailable(error),
    };
    if !status.is_success() {
        return unavailable(format!("{url} answered {status}: {body}"));
    }
    let document: Value = match serde_json::from_str(&body) {
        Ok(document) => document,
        Err(error) => {
            return unavailable(format!(
                "{url} answered {status} with unreadable JSON: {error}; body: {body}"
            ));
        }
    };
    let Some(results) = document.get("results").and_then(Value::as_array) else {
        return SourceOutcome {
            hits: Vec::new(),
            status: SourceStatus::unavailable(
                "ground-truth",
                format!("{url} answered {status} without the required results array; body: {body}"),
                started,
            ),
        };
    };
    let considered = results.len();
    let hits: Vec<Recommendation> = results
        .iter()
        .take(request.limit)
        .map(|result| citation(result, terms))
        .collect();
    SourceOutcome {
        status: SourceStatus::available(
            "ground-truth",
            format!("{url} returned {considered} cited chunk(s)"),
            considered,
            hits.len(),
            started,
        ),
        hits,
    }
}

/// One `/search` result as a recommendation. The locator is the citation the
/// index returned — repository, path, commit and lines — so it can be read
/// without trusting this process's working copy.
fn citation(result: &Value, terms: &[String]) -> Recommendation {
    let repo = string_at(result, "repo", "unknown-repo");
    let path = string_at(result, "path", "unknown-path");
    let reference = result
        .get("commitSha")
        .and_then(Value::as_str)
        .or_else(|| result.get("ref").and_then(Value::as_str))
        .unwrap_or("unknown-ref");
    let first = result
        .get("lineStart")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let last = result
        .get("lineEnd")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let content = string_at(result, "content", "");
    let heading = result
        .get("heading")
        .and_then(Value::as_str)
        .unwrap_or(path);
    Recommendation {
        source: "ground-truth".to_string(),
        title: heading.to_string(),
        locator: format!("{repo}/{path}@{reference}:{first}-{last}"),
        score: result
            .get("score")
            .and_then(Value::as_f64)
            .unwrap_or_default(),
        matched: matched_terms(content, terms),
        snippet: snippet(content, terms),
    }
}

fn string_at<'a>(value: &'a Value, key: &str, declared: &'a str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or(declared)
}

/// Whether the endpoint is configured and whether it answers its own health
/// route right now; a caller that cancels ends the read where it stands.
pub(crate) fn probe(settings: &Settings, cancellation: &CancellationToken) -> Value {
    let started = Instant::now();
    let status = if settings.ground_truth_url.is_empty() {
        SourceStatus::unavailable("ground-truth", NO_ENDPOINT, started)
    } else {
        let url = format!("{}/health", settings.ground_truth_url);
        match get(&url, &[], cancellation) {
            Ok((status, _)) if status.is_success() => SourceStatus::available(
                "ground-truth",
                format!("{url} answered {status}"),
                usize::default(),
                usize::default(),
                started,
            ),
            Ok((status, body)) => SourceStatus::unavailable(
                "ground-truth",
                format!("{url} answered {status}: {body}"),
                started,
            ),
            Err(error) => SourceStatus::unavailable("ground-truth", error, started),
        }
    };
    probe_value(
        &status,
        vec![
            ("endpoint", json!(settings.ground_truth_url.clone())),
            ("origin", json!(settings.ground_truth_origin.clone())),
        ],
    )
}

/// One GET to the index, answered as its status and body. Each network wait
/// is announced through Stado's wait lines and raced against the caller's
/// cancellation event: a caller that cancels drops the request where it
/// stands, so nothing of it keeps running, and the cancellation is the answer.
fn get(
    url: &str,
    query: &[(&str, String)],
    cancellation: &CancellationToken,
) -> Result<(reqwest::StatusCode, String), String> {
    if cancellation.is_cancelled() {
        return Err(format!(
            "{url}: cancelled by its caller before the request was sent"
        ));
    }
    let client = reqwest::Client::builder().build().map_err(|error| {
        format!(
            "{url}: HTTP client creation failed: {}",
            stado_wait::chain(&error)
        )
    })?;
    let request = client.get(url).query(query);
    let scope = cancellation.for_stado();
    let cancelled = || format!("{url}: cancelled by its caller while waiting for the answer");
    process_runtime()?.block_on(async {
        let sent = request.send();
        let response = match stado_wait::until_cancellable(
            Kind::Network,
            format!("GET {url}"),
            url,
            &scope.event,
            sent,
        )
        .await
        {
            Some(Ok(response)) => response,
            Some(Err(error)) => {
                return Err(format!(
                    "{url} is unreachable: {}",
                    stado_wait::chain(&error)
                ));
            }
            None => return Err(cancelled()),
        };
        let status = response.status();
        let read = response.text();
        match stado_wait::until_cancellable(
            Kind::Network,
            format!("body of GET {url}"),
            url,
            &scope.event,
            read,
        )
        .await
        {
            Some(Ok(body)) => Ok((status, body)),
            Some(Err(error)) => Err(format!(
                "{url} answered {status} with an unreadable body: {}",
                stado_wait::chain(&error)
            )),
            None => Err(cancelled()),
        }
    })
}
