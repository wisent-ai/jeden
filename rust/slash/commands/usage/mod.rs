//! What this workspace has used and what is left of the quotas that pay for
//! it.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::slash::common::{now_text, read_json_value, split_head, write_json_value};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

mod quota;
mod totals;

use quota::quota_rows;
use totals::Agg;

fn usage_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/usage.json")
}

pub(crate) fn usage_picker(context: &SlashContext<'_>) -> PickerSpec {
    let lang = crate::cli::i18n::lang_code(context.cwd);
    let path = usage_path(context.cwd);
    let usage = read_json_value(&path);
    let events = usage
        .get("events")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let total_tokens = events
        .iter()
        .map(|event| {
            event
                .get("totalTokens")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    let direct = ["inputTokens", "outputTokens"]
                        .iter()
                        .map(|key| event.get(key).and_then(Value::as_f64).unwrap_or_default())
                        .sum::<f64>();
                    let cache_read = event
                        .get("cacheReadTokens")
                        .or_else(|| event.get("cacheRead"))
                        .and_then(Value::as_f64)
                        .unwrap_or_default();
                    let cache_write = event
                        .get("cacheWriteTokens")
                        .or_else(|| event.get("cacheWrite"))
                        .and_then(Value::as_f64)
                        .unwrap_or_default();
                    direct + cache_read + cache_write
                })
        })
        .sum::<f64>();
    let total_cost = events
        .iter()
        .map(|event| {
            event
                .pointer("/cost/total")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    ["input", "output", "cacheRead", "cacheWrite"]
                        .iter()
                        .map(|key| {
                            event
                                .pointer(&format!("/cost/{key}"))
                                .and_then(Value::as_f64)
                                .unwrap_or_default()
                        })
                        .sum::<f64>()
                })
        })
        .sum::<f64>();
    let updated = usage
        .get("updatedAt")
        .and_then(Value::as_str)
        .unwrap_or("not recorded");
    let mut items = quota_rows();
    items.extend([
        PickerItem::action("Show usage report", "/usage show")
            .detail(format!(
                "{total_tokens} tokens; recorded cost {total_cost}; updated {updated}"
            ))
            .badge(format!("{} events · project", events.len())),
        PickerItem::action("Show usage status", "/usage status")
            .detail(format!("Read accounting from {}", path.display()))
            .badge("status"),
        PickerItem::action("Reset usage accounting", "/usage reset")
            .detail(format!("Clear all recorded events in {}", path.display()))
            .badge("destructive")
            .disabled(events.is_empty()),
    ]);
    PickerSpec::new(crate::cli::i18n::tr(&lang, "view.usage.title"), items).localized(&lang)
}

pub(crate) fn handle_usage(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, _) = split_head(args);
    let verb = if verb.is_empty() { "show" } else { verb };
    let path = usage_path(context.cwd);
    if verb == "reset" {
        write_json_value(&path, &json!({"updatedAt": now_text(), "events": []}))?;
        return Ok(format!("Reset usage accounting: {}", path.display()));
    }
    if verb != "show" && verb != "status" {
        return Err("Usage: /usage [show|reset]".into());
    }
    let usage = read_json_value(&path);
    let events = usage
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut by_model: BTreeMap<String, Agg> = BTreeMap::new();
    let mut totals = Agg::default();
    for event in &events {
        let input = event
            .get("inputTokens")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let output = event
            .get("outputTokens")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let cache_read = event
            .get("cacheReadTokens")
            .or_else(|| event.get("cacheRead"))
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let cache_write = event
            .get("cacheWriteTokens")
            .or_else(|| event.get("cacheWrite"))
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let total = event
            .get("totalTokens")
            .and_then(Value::as_f64)
            .unwrap_or(input + output + cache_read + cache_write);
        let event_cost_input = event
            .pointer("/cost/input")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let event_cost_output = event
            .pointer("/cost/output")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let event_cost_cache_read = event
            .pointer("/cost/cacheRead")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let event_cost_cache_write = event
            .pointer("/cost/cacheWrite")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let event_cost_total = event
            .pointer("/cost/total")
            .and_then(Value::as_f64)
            .unwrap_or(
                event_cost_input
                    + event_cost_output
                    + event_cost_cache_read
                    + event_cost_cache_write,
            );
        totals.add(
            input,
            output,
            cache_read,
            cache_write,
            total,
            event_cost_input,
            event_cost_output,
            event_cost_cache_read,
            event_cost_cache_write,
            event_cost_total,
        );
        let model = event
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("default")
            .to_string();
        by_model.entry(model).or_default().add(
            input,
            output,
            cache_read,
            cache_write,
            total,
            event_cost_input,
            event_cost_output,
            event_cost_cache_read,
            event_cost_cache_write,
            event_cost_total,
        );
    }
    let mut by_model_value = Map::new();
    for (model, agg) in &by_model {
        by_model_value.insert(model.clone(), agg.to_json());
    }
    // The prior "last-N events" cap was an unconsented numeric literal; recent
    // now carries every event in chronological order.
    let recent = events.clone();
    let summary = json!({
        "file": path,
        "updatedAt": usage.get("updatedAt").cloned().unwrap_or(Value::Null),
        "totals": {
            "calls": events.len(),
            "inputTokens": totals.input,
            "outputTokens": totals.output,
            "cacheReadTokens": totals.cache_read,
            "cacheWriteTokens": totals.cache_write,
            "totalTokens": totals.total,
            "cost": {
                "input": totals.cost_input,
                "output": totals.cost_output,
                "cacheRead": totals.cost_cache_read,
                "cacheWrite": totals.cost_cache_write,
                "total": totals.cost_total,
            },
            "byModel": Value::Object(by_model_value),
        },
        "recent": recent,
    });
    serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())
}
