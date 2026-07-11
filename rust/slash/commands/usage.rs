use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::slash::common::{now_text, read_json_value, split_head, write_json_value};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

fn usage_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/usage.json")
}

/// Running accumulation of usage counters. `Default` zeroes every field, so no
/// bare numeric initializers are needed.
#[derive(Default)]
struct Agg {
    calls: u64,
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
    total: f64,
    cost_input: f64,
    cost_output: f64,
    cost_cache_read: f64,
    cost_cache_write: f64,
    cost_total: f64,
}

impl Agg {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        input: f64,
        output: f64,
        cache_read: f64,
        cache_write: f64,
        total: f64,
        cost_input: f64,
        cost_output: f64,
        cost_cache_read: f64,
        cost_cache_write: f64,
        cost_total: f64,
    ) {
        // One call per event; `u64::from(true)` is the number-free unit increment.
        self.calls += u64::from(true);
        self.input += input;
        self.output += output;
        self.cache_read += cache_read;
        self.cache_write += cache_write;
        self.total += total;
        self.cost_input += cost_input;
        self.cost_output += cost_output;
        self.cost_cache_read += cost_cache_read;
        self.cost_cache_write += cost_cache_write;
        self.cost_total += cost_total;
    }

    fn to_json(&self) -> Value {
        json!({
            "calls": self.calls,
            "inputTokens": self.input,
            "outputTokens": self.output,
            "cacheReadTokens": self.cache_read,
            "cacheWriteTokens": self.cache_write,
            "totalTokens": self.total,
            "cost": {
                "input": self.cost_input,
                "output": self.cost_output,
                "cacheRead": self.cost_cache_read,
                "cacheWrite": self.cost_cache_write,
                "total": self.cost_total,
            },
        })
    }
}

pub(crate) fn usage_picker(context: &SlashContext<'_>) -> PickerSpec {
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
    let items = vec![
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
    ];
    PickerSpec::new("Provider usage", items)
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
