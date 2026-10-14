//! Source execution keeps each wait and failure attached to its source.
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::sources::{files, ground_truth, memory, transcripts};
use super::{Request, Settings, SourceOutcome, SourceStatus, SOURCES};

pub(super) fn run(
    cwd: &Path,
    settings: &Settings,
    request: &Request,
    terms: &[String],
) -> Vec<SourceOutcome> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = SOURCES
            .iter()
            .copied()
            .filter(|source| request.sources.iter().any(|want| want == source))
            .map(|source| {
                let started = Instant::now();
                (
                    source,
                    started,
                    scope.spawn(move || ask(source, cwd, settings, request, terms)),
                )
            })
            .collect();
        handles
            .into_iter()
            .map(|(source, started, handle)| match handle.join() {
                Ok(outcome) => outcome,
                Err(payload) => {
                    let cause = if let Some(message) = payload.downcast_ref::<String>() {
                        message.as_str()
                    } else if let Some(message) = payload.downcast_ref::<&str>() {
                        message
                    } else {
                        "non-text panic payload"
                    };
                    SourceOutcome {
                        hits: Vec::new(),
                        status: SourceStatus::unavailable(
                            source,
                            format!("source execution panicked before answering: {cause}"),
                            started,
                        ),
                    }
                }
            })
            .collect()
    })
}

fn ask(
    source: &str,
    cwd: &Path,
    settings: &Settings,
    request: &Request,
    terms: &[String],
) -> SourceOutcome {
    let started = Instant::now();
    let waiting = stado_wait::begin(
        stado_wait::Kind::Process,
        format!("context advisor source {source}"),
        cwd.display(),
    );
    let outcome = if let Some(cancelled) = request.cancelled_outcome(source, started) {
        cancelled
    } else if terms.is_empty() {
        SourceOutcome {
            hits: Vec::new(),
            status: SourceStatus::unavailable(
                source,
                "the query contains no alphanumeric search terms",
                started,
            ),
        }
    } else {
        match source {
            "files" => files::search(settings, request, terms),
            "memory" => ask_memory(cwd, request, terms),
            "transcripts" => transcripts::search(settings, request, terms),
            "ground-truth" => ground_truth::search(settings, request, terms),
            other => SourceOutcome {
                hits: Vec::new(),
                status: SourceStatus::unavailable(other, "no such source", started),
            },
        }
    };
    if outcome.status.available {
        waiting.done();
    } else {
        waiting.failed(&outcome.status.detail);
    }
    outcome
}

fn resolver_interval() -> Result<(Duration, String), String> {
    let path = match std::env::var_os("STADO_RESOLVER_STATE_FILE").filter(|value| !value.is_empty())
    {
        Some(explicit) => PathBuf::from(explicit),
        None => std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".stado/resolver-state.json"))
            .ok_or("HOME is unset and Stado's resolver state path was not declared")?,
    };
    let bytes = std::fs::read(&path).map_err(|error| {
        format!(
            "Stado resolver state {} cannot be read: {error}",
            path.display()
        )
    })?;
    let state: Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "Stado resolver state {} is invalid: {error}",
            path.display()
        )
    })?;
    let seconds = state
        .get("refresh_seconds")
        .and_then(Value::as_u64)
        .and_then(NonZeroU64::new)
        .ok_or_else(|| {
            format!(
                "Stado resolver state {} has no positive declared refresh_seconds",
                path.display()
            )
        })?;
    let target = state
        .get("target")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Stado resolver state {} has no target", path.display()))?;
    let phase = state
        .get("state")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Stado resolver state {} has no state", path.display()))?;
    let mut observation = format!("Stado resolver target {target}, state {phase}");
    if let Some(reason) = state
        .get("reason")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        observation.push_str(&format!(", reason {reason}"));
    }
    Ok((Duration::from_secs(seconds.get()), observation))
}

fn bounded_memory<T: Send + 'static>(
    cancellation: &crate::tool_runtime::runtime_ops::CancellationToken,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (interval, observation) = resolver_interval()
        .map_err(|cause| format!("stado database resolve jeden cannot be bounded: {cause}"))?;
    let token = cancellation.child();
    let worker_token = token.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::fleet::with_cancellation(&worker_token, work)
        }))
        .map_err(|payload| {
            let cause = if let Some(message) = payload.downcast_ref::<String>() {
                message.as_str()
            } else if let Some(message) = payload.downcast_ref::<&str>() {
                message
            } else {
                "non-text panic payload"
            };
            format!("memory source execution panicked before answering: {cause}")
        });
        let _ = sender.send(result);
    });
    match receiver.recv_timeout(interval) {
        Ok(outcome) => outcome,
        Err(RecvTimeoutError::Timeout) => {
            token.cancel();
            Err(format!("stado database resolve jeden did not answer within the resolver-declared {}s refresh interval; {observation}",
                interval.as_secs()))
        }
        Err(RecvTimeoutError::Disconnected) => Err(format!(
            "memory worker ended without an answer during stado database resolve jeden; {observation}"
        )),
    }
}

fn ask_memory(cwd: &Path, request: &Request, terms: &[String]) -> SourceOutcome {
    let started = Instant::now();
    let cwd = cwd.to_path_buf();
    let terms = terms.to_vec();
    let query = request.query.clone();
    let limit = request.limit;
    match bounded_memory(&request.cancellation, move || {
        memory::search(&cwd, &terms, &query, limit)
    }) {
        Ok(outcome) => outcome,
        Err(cause) => SourceOutcome {
            hits: Vec::new(),
            status: SourceStatus::unavailable("memory", cause, started),
        },
    }
}

pub(super) fn probe_memory(
    cancellation: &crate::tool_runtime::runtime_ops::CancellationToken,
) -> Value {
    let started = Instant::now();
    match bounded_memory(cancellation, memory::probe) {
        Ok(answer) => answer,
        Err(cause) => super::probe_value(
            &SourceStatus::unavailable("memory", cause, started),
            Vec::new(),
        ),
    }
}
