use serde_json::{json, Value};
use std::path::PathBuf;

use crate::memory::{scope_from_value, FtsBackend, MemoryStore};
use crate::slash::common::split_args;
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

const MEMORY_USAGE: &str = "Usage: /memory [view [query]|stats|queue [status|run [limit]]|enqueue [reindex]|rebuild|clear]";

pub(crate) fn memory_file_path() -> PathBuf {
    MemoryStore::default_path()
}

pub(crate) fn memory_picker() -> Result<PickerSpec, String> {
    let store = MemoryStore::open(memory_file_path())?;
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
                .detail(format!(
                    "{count} active records in {}",
                    store.path().display()
                ))
                .badge("SQLite WAL"),
            PickerItem::action("Show memory health", "/memory stats")
                .detail(format!("{pending} pending jobs; {failed} failed jobs"))
                .badge("FTS5"),
            PickerItem::action("Inspect maintenance queue", "/memory queue")
                .detail("Show durable extraction and reindex jobs with explicit states")
                .badge("durable"),
            PickerItem::action("Rebuild search index now", "/memory rebuild")
                .detail("Rebuild and optimize the SQLite FTS5 index, then verify integrity")
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
    let store = MemoryStore::open(memory_file_path())?;
    let scope = scope_from_value(None, context.cwd);
    match verb {
        "stats" | "diagnose" | "health" if rest.is_empty() => {
            serde_json::to_string_pretty(&store.health()?).map_err(|e| e.to_string())
        }
        "clear" | "reset" if rest.is_empty() => Ok(format!(
            "Deleted {} stored memory revision row(s) from {}. The durable job queue was not changed.",
            store.clear()?,
            store.path().display()
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
        &json!({"requestedBy":"slash","scope":scope,"operation":"fts-rebuild-optimize"}),
    )?;
    Ok(format!(
        "Enqueued durable FTS5 rebuild job {id}. Inspect it with /memory queue; execute queued work with /memory queue run."
    ))
}

fn handle_queue(store: &MemoryStore, args: &[String]) -> Result<String, String> {
    let action = args.first().map(String::as_str).unwrap_or("status");
    match action {
        "status" if args.len() == 1 || args.is_empty() => {
            serde_json::to_string_pretty(&store.queue_status(100)?).map_err(|e| e.to_string())
        }
        "run" | "drain" if args.len() <= 2 => {
            let limit = args
                .get(1)
                .map(|value| {
                    value.parse::<usize>().map_err(|_| {
                        "memory queue run limit must be a positive integer".to_string()
                    })
                })
                .transpose()?
                .unwrap_or(1);
            if !(1..=100).contains(&limit) {
                return Err("memory queue run limit must be between 1 and 100".into());
            }
            let worker = format!("slash-memory-{}", std::process::id());
            let mut processed = 0;
            while processed < limit {
                match store.process_one(&worker) {
                    Ok(true) => processed += 1,
                    Ok(false) => break,
                    Err(error) => {
                        return Err(format!(
                            "Memory queue processing failed after {processed} completed job(s): {error}. Inspect /memory queue for retry state."
                        ));
                    }
                }
            }
            let status = store.queue_status(20)?;
            Ok(format!(
                "Processed {processed} memory job(s); {} pending, {} failed.",
                status.pending, status.failed
            ))
        }
        _ => Err("Usage: /memory queue [status|run [limit]]".into()),
    }
}
