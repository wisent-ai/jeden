use std::num::{NonZeroI64, NonZeroUsize};

use serde_json::{json, Value};

use crate::memory::{scope_from_value, FtsBackend, MemoryStore};
use crate::slash::common::split_args;
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

const MEMORY_USAGE: &str =
    "Usage: /memory [view [query]|stats|queue [status|run LIMIT LEASE_MS|drain LIMIT LEASE_MS]|enqueue [reindex]|rebuild|clear]";

pub(crate) fn memory_picker() -> Result<PickerSpec, String> {
    let store = MemoryStore::open()?;
    let health = store.health()?;
    let count = health
        .get("activeMemories")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let pending = health
        .get("pendingJobs")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let failed = health
        .get("failedJobs")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    Ok(PickerSpec::new(
        "Durable memory",
        vec![
            PickerItem::action("View durable memory", "/memory view")
                .detail(format!("{count} active records in {}", store.location()))
                .badge("fleet"),
            PickerItem::action("Show memory health", "/memory stats")
                .detail(format!("{pending} pending jobs; {failed} failed jobs"))
                .badge("full-text"),
            PickerItem::action("Inspect maintenance queue", "/memory queue")
                .detail("Show durable extraction and reindex jobs with explicit states")
                .badge("durable"),
            PickerItem::action("Rebuild search index now", "/memory rebuild")
                .detail(
                    "Rebuild the full-text index memories_search and report the stored row count",
                )
                .badge("maintenance"),
            PickerItem::action("Clear durable memory", "/memory clear")
                .detail("Delete stored memory revisions; queued work is retained")
                .badge("destructive")
                .disabled(count == 0),
        ],
    ))
}

pub(crate) fn handle_memory(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let (verb, rest) = argv
        .split_first()
        .map(|(value, tail)| (value.as_str(), tail))
        .unwrap_or(("view", &[][..]));
    let store = MemoryStore::open()?;
    let scope = scope_from_value(None, context.cwd);
    match verb {
        "stats" | "diagnose" | "health" if rest.is_empty() => {
            serde_json::to_string_pretty(&store.health()?).map_err(|e| e.to_string())
        }
        "clear" | "reset" if rest.is_empty() => Ok(format!(
            "Deleted {} stored memory revision row(s) from {}. The durable job queue was not changed.",
            store.clear()?,
            store.location()
        )),
        "view" | "list" | "" => {
            let query = rest.join(" ");
            let hits = store.recall(&FtsBackend, &scope, &query, 200)?;
            if hits.is_empty() {
                return Ok("No matching active memory records.".into());
            }
            Ok(hits
                .into_iter()
                .map(|hit| {
                    format!(
                        "{}\t{}\t{}\t{:.3}\t{}",
                        hit.record.id,
                        hit.record.kind,
                        hit.provenance.backend,
                        hit.score,
                        hit.record.text
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "enqueue" => enqueue_maintenance(&store, rest, &scope),
        "queue" | "jobs" => handle_queue(&store, rest),
        "rebuild" | "reindex" if rest.is_empty() => {
            serde_json::to_string_pretty(&store.rebuild_fts()?).map_err(|e| e.to_string())
        }
        _ => Err(MEMORY_USAGE.into()),
    }
}

fn enqueue_maintenance(
    store: &MemoryStore,
    args: &[String],
    scope: &crate::memory::MemoryScope,
) -> Result<String, String> {
    let operation = args.first().map(String::as_str).unwrap_or("reindex");
    if args.len() > 1 || !matches!(operation, "reindex" | "rebuild") {
        return Err("Usage: /memory enqueue [reindex]".into());
    }
    let id = store.enqueue(
        "reindex",
        &json!({"requestedBy":"slash","scope":scope,"operation":"reindex"}),
    )?;
    Ok(format!(
        "Enqueued durable full-text reindex job {id}. Inspect it with /memory queue; execute queued work with /memory queue run LIMIT LEASE_MS."
    ))
}

/// The queue verbs, parsed by their declaration rather than compared as words.
#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum QueueVerb {
    Status,
    Run,
    Drain,
}

const QUEUE_USAGE: &str =
    "Usage: /memory queue [status | run LIMIT LEASE_MS | drain LIMIT LEASE_MS]";

/// `run` and `drain` process at most LIMIT jobs, each claimed for LEASE_MS
/// milliseconds: the time this worker promises to hold a job before another
/// worker may reclaim it. Both are the caller's; neither has a default.
fn handle_queue(store: &MemoryStore, args: &[String]) -> Result<String, String> {
    let Some((action, inputs)) = args.split_first() else {
        return serde_json::to_string_pretty(&store.queue_status()?).map_err(|e| e.to_string());
    };
    let verb: QueueVerb = serde_json::from_value(Value::String(action.clone()))
        .map_err(|_| format!("unknown memory queue action {action:?}; {QUEUE_USAGE}"))?;
    match (verb, inputs) {
        (QueueVerb::Status, []) => {
            serde_json::to_string_pretty(&store.queue_status()?).map_err(|e| e.to_string())
        }
        (QueueVerb::Run | QueueVerb::Drain, [limit, lease]) => {
            let limit: NonZeroUsize = limit.parse().map_err(|error| {
                format!("memory queue LIMIT {limit:?} must be a positive integer: {error}; {QUEUE_USAGE}")
            })?;
            let lease_ms: NonZeroI64 = lease.parse().map_err(|error| {
                format!("memory queue LEASE_MS {lease:?} must be a positive integer: {error}; {QUEUE_USAGE}")
            })?;
            if !lease_ms.get().is_positive() {
                return Err(format!(
                    "memory queue LEASE_MS {lease} must be positive: it is how long this worker holds a claimed job before another worker may reclaim it"
                ));
            }
            let worker = format!("slash-memory-{}", std::process::id());
            let mut processed = 0;
            while processed < limit.get() {
                match store.process_one(&worker, lease_ms.get()) {
                    Ok(true) => processed += 1,
                    Ok(false) => break,
                    Err(error) => {
                        return Err(format!(
                            "Memory queue processing failed after {processed} completed job(s): {error}. Inspect /memory queue for retry state."
                        ));
                    }
                }
            }
            let status = store.queue_status()?;
            Ok(format!(
                "Processed {processed} memory job(s); {} pending, {} failed.",
                status.pending, status.failed
            ))
        }
        _ => Err(QUEUE_USAGE.into()),
    }
}
