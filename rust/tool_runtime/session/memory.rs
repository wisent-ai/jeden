use serde_json::{json, Value};

use crate::memory::{FtsBackend, MemorySource, MemoryStore, scope_from_value};
use crate::tool_runtime::shared::{string_input, u64_input};
use crate::tool_runtime::ToolRuntime;

pub(crate) fn memory_tool(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let store = MemoryStore::open(MemoryStore::default_path())?;
    let op = string_input(input, "op").unwrap_or_else(|| "recall".into());
    let scope = scope_from_value(input.get("scope"), runtime.cwd);
    match op.as_str() {
        "remember" => {
            if !runtime.allow_write { return Err("memory remember requires --allow-write".into()); }
            let text = string_input(input, "text").ok_or("memory remember requires text")?;
            let tags: Vec<String> = input.get("tags").and_then(Value::as_array).map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            let source = input.get("source").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or(MemorySource { origin: "rust_memory_tool".into(), session_id: None, entry_id: None });
            let entry = store.remember(&string_input(input,"kind").unwrap_or_else(||"note".into()), &scope, &text, &tags, &source, input.get("confidence").and_then(Value::as_f64).unwrap_or(0.5))?;
            Ok(json!({"entry":entry,"backend":"sqlite-wal"}))
        }
        "list" => Ok(json!({"entries":store.list(u64_input(input,"limit",20).clamp(1,200) as usize)?})),
        "recall" => {
            let query=string_input(input,"query").unwrap_or_default();
            let hits=store.recall(&FtsBackend,&scope,&query,u64_input(input,"limit",10).clamp(1,100) as usize)?;
            Ok(json!({"entries":hits.iter().map(|h|&h.record).collect::<Vec<_>>(),"hits":hits,"query":query,"backend":"sqlite-fts5"}))
        }
        "context" => Ok(json!({"context":store.pre_compaction_context(&scope,&string_input(input,"query").unwrap_or_default(),u64_input(input,"maxChars",12_000).clamp(256,12_000) as usize)?})),
        "forget" => {
            if !runtime.allow_write { return Err("memory forget requires --allow-write".into()); }
            Ok(json!({"removed":store.forget_scope(&scope)?}))
        }
        "health" | "stats" => store.health(),
        other => Err(format!("unknown memory op: {other}")),
    }
}
