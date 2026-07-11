use serde_json::Value;
use std::path::PathBuf;

use crate::memory::{scope_from_value, FtsBackend, MemoryStore};
use crate::slash::common::split_args;
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

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
    Ok(PickerSpec::new(
        "Durable memory",
        vec![
            PickerItem::action("View durable memory", "/memory view")
                .detail(format!("{count} records in {}", store.path().display()))
                .badge("SQLite WAL"),
            PickerItem::action("Show memory health", "/memory stats")
                .detail(format!("{pending} extraction/consolidation jobs pending"))
                .badge("FTS5"),
            PickerItem::action("Clear durable memory", "/memory clear")
                .detail("Forget all durable records; queued work is retained")
                .badge("destructive")
                .disabled(count == 0),
        ],
    ))
}

pub(crate) fn handle_memory(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let (verb, rest) = argv
        .split_first()
        .map(|(v, r)| (v.as_str(), r))
        .unwrap_or(("view", &[][..]));
    let store = MemoryStore::open(memory_file_path())?;
    let scope = scope_from_value(None, context.cwd);
    match verb {
        "stats"|"diagnose"|"health"=>serde_json::to_string_pretty(&store.health()?).map_err(|e|e.to_string()),
        "clear"|"reset"=>Ok(format!("Cleared {} active memory record(s) from {}.",store.clear()?,store.path().display())),
        "view"|"list"|""=>{
            let query=rest.join(" "); let hits=store.recall(&FtsBackend,&scope,&query,200)?;
            if hits.is_empty(){return Ok("No matching memory records.".into())}
            Ok(hits.into_iter().map(|hit|format!("{}\t{}\t{}\t{:.3}\t{}",hit.record.id,hit.record.kind,hit.provenance.backend,hit.score,hit.record.text)).collect::<Vec<_>>().join("\n"))
        }
        "enqueue"|"queue"|"rebuild"=>Err("Memory extraction and FTS maintenance are automatic; manual queue/rebuild commands were removed.".into()),
        _=>Err("Usage: /memory [view [query]|stats|diagnose|clear|reset]".into()),
    }
}
