use serde_json::{json, Value};

use crate::memory::{scope_from_value, FtsBackend, MemorySource, MemoryStore};
use crate::tool_runtime::shared::{count_input, string_input};
use crate::tool_runtime::ToolRuntime;

pub(crate) fn memory_tool(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let store = MemoryStore::open()?;
    let op = string_input(input, "op").unwrap_or_else(|| "recall".into());
    let scope = scope_from_value(input.get("scope"), runtime.cwd);
    match op.as_str() {
        "remember" => {
            if !runtime.allow_write {
                return Err("memory remember requires --allow-write".into());
            }
            let text = string_input(input, "text").ok_or("memory remember requires text")?;
            let tags: Vec<String> = input
                .get("tags")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let source = input
                .get("source")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or(MemorySource {
                    origin: "rust_memory_tool".into(),
                    session_id: None,
                    entry_id: None,
                });
            let entry = store.remember(
                &string_input(input, "kind").unwrap_or_else(|| "note".into()),
                &scope,
                &text,
                &tags,
                &source,
                input
                    .get("confidence")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.5),
            )?;
            Ok(json!({"entry":entry,"backend":"fleet-postgres"}))
        }
        "list" => {
            Ok(json!({"entries":store.list(count_input(input, "limit", "memory list")? as usize)?}))
        }
        "recall" => {
            let query = string_input(input, "query").unwrap_or_default();
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
        "health" | "stats" => store.health(),
        other => Err(format!("unknown memory op: {other}")),
    }
}
