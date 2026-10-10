//! Source execution keeps each wait and failure attached to its source.
use std::path::Path;
use std::time::Instant;

use super::sources::{files, ground_truth, memory, transcripts};
use super::{Request, Settings, SourceOutcome, SourceStatus, SOURCES};

pub(super) fn run(cwd: &Path, settings: &Settings, request: &Request, terms: &[String]) -> Vec<SourceOutcome> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = SOURCES.iter().copied()
            .filter(|source| request.sources.iter().any(|want| want == source))
            .map(|source| {
                let started = Instant::now();
                (source, started, scope.spawn(move || ask(source, cwd, settings, request, terms)))
            }).collect();
        handles.into_iter().map(|(source, started, handle)| {
            match handle.join() {
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
                        status: SourceStatus::unavailable(source,
                            format!("source execution panicked before answering: {cause}"), started),
                    }
                }
            }
        }).collect()
    })
}

fn ask(source: &str, cwd: &Path, settings: &Settings, request: &Request, terms: &[String]) -> SourceOutcome {
    let started = Instant::now();
    let waiting = stado_wait::begin(stado_wait::Kind::Process,
        format!("context advisor source {source}"), cwd.display());
    let outcome = if terms.is_empty() {
        SourceOutcome {
            hits: Vec::new(),
            status: SourceStatus::unavailable(source,
                "the query carries no searchable word of three characters or more", started),
        }
    } else {
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
    };
    if outcome.status.available {
        waiting.done();
    } else {
        waiting.failed(&outcome.status.detail);
    }
    outcome
}
