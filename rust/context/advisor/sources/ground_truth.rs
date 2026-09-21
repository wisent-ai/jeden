//! The ground-truth source: the Wisent cross-repository index, which answers
//! with repository, path, commit and line citations.
//!
//! `wisent-ground-truth-api` owns the index and the citation contract; this
//! module is a client of its `/search` route and nothing else. When no
//! endpoint is configured, that is reported as the configuration it is, not
//! as an empty result.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::context::advisor::text::{matched_terms, snippet};
use crate::context::advisor::{
    probe_value, Recommendation, Request, Settings, SourceOutcome, SourceStatus,
};

const SNIPPET_LINES: usize = 3;
const SNIPPET_CHARS: usize = 420;
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
    let response = match client(request.timeout) {
        Ok(client) => client
            .get(&url)
            .query(&[
                ("q", request.query.clone()),
                ("limit", request.limit.to_string()),
            ])
            .send(),
        Err(error) => {
            return SourceOutcome {
                hits: Vec::new(),
                status: SourceStatus::unavailable("ground-truth", error, started),
            }
        }
    };
    let document: Value = match response {
        Ok(response) if response.status().is_success() => match response.json() {
            Ok(document) => document,
            Err(error) => {
                return SourceOutcome {
                    hits: Vec::new(),
                    status: SourceStatus::unavailable(
                        "ground-truth",
                        format!("{url} answered unreadable JSON: {error}"),
                        started,
                    ),
                }
            }
        },
        Ok(response) => {
            let code = response.status();
            return SourceOutcome {
                hits: Vec::new(),
                status: SourceStatus::unavailable(
                    "ground-truth",
                    format!("{url} answered {code}"),
                    started,
                ),
            };
        }
        Err(error) => {
            return SourceOutcome {
                hits: Vec::new(),
                status: SourceStatus::unavailable(
                    "ground-truth",
                    format!("{url} is unreachable: {error}"),
                    started,
                ),
            }
        }
    };
    let results = document
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
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
    let heading = result.get("heading").and_then(Value::as_str).unwrap_or(path);
    Recommendation {
        source: "ground-truth".to_string(),
        title: heading.to_string(),
        locator: format!("{repo}/{path}@{reference}:{first}-{last}"),
        score: result
            .get("score")
            .and_then(Value::as_f64)
            .unwrap_or_default(),
        matched: matched_terms(content, terms),
        snippet: snippet(content, terms, SNIPPET_LINES, SNIPPET_CHARS),
    }
}

fn string_at<'a>(value: &'a Value, key: &str, declared: &'a str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or(declared)
}

/// Whether the endpoint is configured and whether it answers its own health
/// route right now.
pub(crate) fn probe(settings: &Settings) -> Value {
    let started = Instant::now();
    let status = if settings.ground_truth_url.is_empty() {
        SourceStatus::unavailable("ground-truth", NO_ENDPOINT, started)
    } else {
        let url = format!("{}/health", settings.ground_truth_url);
        match client(Duration::from_millis(settings.timeout_ms))
            .and_then(|client| client.get(&url).send().map_err(|error| error.to_string()))
        {
            Ok(response) if response.status().is_success() => SourceStatus::available(
                "ground-truth",
                format!("{url} answered {}", response.status()),
                usize::default(),
                usize::default(),
                started,
            ),
            Ok(response) => SourceStatus::unavailable(
                "ground-truth",
                format!("{url} answered {}", response.status()),
                started,
            ),
            Err(error) => SourceStatus::unavailable(
                "ground-truth",
                format!("{url} is unreachable: {error}"),
                started,
            ),
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

fn client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| format!("HTTP client could not be built: {error}"))
}
