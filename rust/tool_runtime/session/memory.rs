use serde_json::{json, Value};

use crate::memory::{scope_from_value, FtsBackend, MemorySource, MemoryStore};
use crate::tool_runtime::shared::{count_input, string_input};
use crate::tool_runtime::ToolRuntime;

pub(crate) fn memory_tool(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let store = MemoryStore::open()?;
    let op = string_input(input, "op")
        .ok_or("memory requires op, one of remember, list, recall, context, forget or stats")?;
    let scope = scope_from_value(input.get("scope"), runtime.cwd)?;
    match op.as_str() {
        "remember" => {
            if !runtime.allow_write {
                return Err("memory remember requires --allow-write".into());
            }
            let text = string_input(input, "text").ok_or("memory remember requires text")?;
            let kind = string_input(input, "kind").ok_or(
                "memory remember requires kind, what the note is (such as note or decision)",
            )?;
            // How sure the note is, between 0 and 1 as the store keeps it: the
            // caller states it; no certainty is assumed for it.
            let confidence = input
                .get("confidence")
                .and_then(Value::as_f64)
                .ok_or("memory remember requires confidence, how sure the note is, from 0 to 1")?;
            // No tags is a note without tags; a tag that is not text is refused.
            let tags: Vec<String> = match input.get("tags") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map(str::to_string)
                            .ok_or_else(|| format!("memory remember tags must be text, not {item}"))
                    })
                    .collect::<Result<_, String>>()?,
                Some(other) => {
                    return Err(format!("memory remember tags must be a list, not {other}"))
                }
            };
            // A note the caller does not attribute is recorded as written by
            // this tool; a source that is not one is refused with its cause.
            let source = match input.get("source") {
                None | Some(Value::Null) => MemorySource {
                    origin: "rust_memory_tool".into(),
                    session_id: None,
                    entry_id: None,
                },
                Some(value) => serde_json::from_value(value.clone()).map_err(|error| {
                    format!("memory remember source is not a memory source: {error}")
                })?,
            };
            let entry = store.remember(&kind, &scope, &text, &tags, &source, confidence)?;
            Ok(json!({"entry":entry,"backend":"fleet-postgres"}))
        }
        "list" => {
            Ok(json!({"entries":store.list(count_input(input, "limit", "memory list")? as usize)?}))
        }
        "recall" => {
            let query = string_input(input, "query")
                .ok_or("memory recall requires query, the text the notes are recalled for")?;
            let hits = store.recall(
                &FtsBackend,
                &scope,
                &query,
                count_input(input, "limit", "memory recall")? as usize,
            )?;
            Ok(
                json!({"entries":hits.iter().map(|h|&h.record).collect::<Vec<_>>(),"hits":hits,"query":query,"backend":"postgres-fts"}),
            )
        }
        "context" => {
            let query = string_input(input, "query")
                .ok_or("memory context requires query, the text the context is gathered for")?;
            let max_chars = count_input(input, "maxChars", "memory context")? as usize;
            Ok(json!({"context":store.pre_compaction_context(&scope,&query,Some(max_chars))?}))
        }
        "forget" => {
            if !runtime.allow_write {
                return Err("memory forget requires --allow-write".into());
            }
            Ok(json!({"removed":store.forget_scope(&scope)?}))
        }
        "stats" => store.health(),
        other => Err(format!("unknown memory op: {other}")),
    }
}
