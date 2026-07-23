use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::control_plane::billing::{QuotaBucket, QuotaState};
use crate::control_plane::now_ms;
use crate::control_plane::quota::{
    fetch_subscription_quotas, percent_free, QuotaEntry, SubscriptionQuotas,
};
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

/// One display row derived from a Weles quota bucket.
struct QuotaRow {
    label: String,
    badge: Option<String>,
    detail: String,
    remaining: Option<u64>,
    exhausted: bool,
}

/// Non-action display row; the picker skips disabled, command-less rows.
fn display_row(label: impl Into<String>) -> PickerItem {
    let mut item = PickerItem::action(label, "");
    item.command = None;
    item.disabled = true;
    item
}

/// Compact 20-cell bar: `█` cells for the used share, `░` for the free share.
fn quota_bar(remaining: u64, limit: u64) -> String {
    const CELLS: u128 = 20;
    let free = ((u128::from(remaining) * CELLS + u128::from(limit) / 2) / u128::from(limit))
        as usize;
    let free = free.min(CELLS as usize);
    format!("{}{}", "█".repeat(CELLS as usize - free), "░".repeat(free))
}

/// ` · resets in <duration>` when the bucket reports a reset timestamp.
fn reset_suffix(now: u64, resets_at_ms: Option<u64>) -> String {
    let Some(reset) = resets_at_ms else {
        return String::new();
    };
    let delta = reset.saturating_sub(now);
    if delta == 0 {
        return " · resets soon".into();
    }
    let minutes = (delta / 60_000).max(1);
    let text = if minutes >= 1_440 {
        format!("{}d{}h", minutes / 1_440, minutes % 1_440 / 60)
    } else if minutes >= 60 {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    };
    format!(" · resets in {text}")
}

/// Badge (percent free) and amount text for one bucket; buckets without a
/// limit render `unmetered` instead of a bar.
fn bucket_amount(bucket: &QuotaBucket) -> (Option<String>, String) {
    match (bucket.remaining, bucket.limit) {
        (Some(remaining), Some(limit)) if limit > 0 => (
            Some(format!("{}%", percent_free(remaining, limit))),
            format!("{} {remaining}/{limit} remaining", quota_bar(remaining, limit)),
        ),
        (Some(remaining), _) => (None, format!("unmetered · {remaining} remaining")),
        (None, Some(limit)) => (None, format!("limit {limit}")),
        (None, None) if bucket.state == QuotaState::Unmetered => (None, "unmetered".into()),
        (None, None) => (None, "amount not reported".into()),
    }
}

fn unavailable_row(label: String, error: impl std::fmt::Display) -> QuotaRow {
    QuotaRow {
        label,
        badge: None,
        detail: format!("quota unavailable: {error}"),
        remaining: None,
        exhausted: false,
    }
}

/// Live quota rows from the shared Weles subscription quota fetch (same
/// client path as `/login`). Any health/account-level failure collapses into
/// a single disabled info row so the local accounting actions below always
/// render.
fn quota_rows() -> Vec<PickerItem> {
    let accounts = match fetch_subscription_quotas() {
        SubscriptionQuotas::Unavailable(reason) => {
            return vec![display_row(format!("quota unavailable: {reason}"))];
        }
        SubscriptionQuotas::Accounts(accounts) => accounts,
    };
    let now = now_ms();
    let mut groups: BTreeMap<String, Vec<QuotaRow>> = BTreeMap::new();
    for account in accounts {
        for entry in account.entries {
            let row = match entry {
                QuotaEntry::Bucket(labeled) => {
                    let (badge, amount) = bucket_amount(&labeled.bucket);
                    QuotaRow {
                        label: labeled.label,
                        badge,
                        detail: format!(
                            "{amount}{}",
                            reset_suffix(now, labeled.bucket.resets_at_ms)
                        ),
                        remaining: labeled.bucket.remaining,
                        exhausted: labeled.bucket.state == QuotaState::Exhausted,
                    }
                }
                QuotaEntry::Unavailable { label, error } => unavailable_row(label, error),
            };
            groups.entry(account.provider.clone()).or_default().push(row);
        }
    }
    let mut items = Vec::new();
    for (provider, mut rows) in groups {
        // Most constrained subscription first; unreported amounts sort last.
        rows.sort_by(|left, right| {
            left.remaining
                .unwrap_or(u64::MAX)
                .cmp(&right.remaining.unwrap_or(u64::MAX))
                .then_with(|| left.label.cmp(&right.label))
        });
        let exhausted = rows.iter().any(|row| row.exhausted);
        let mut header = display_row(format!("── {provider} ({}) ──", rows.len()));
        header.badge = Some(if exhausted { "✖".into() } else { "✔".into() });
        items.push(header);
        items.extend(rows.into_iter().map(|row| {
            let mut item = display_row(row.label).detail(row.detail);
            if let Some(badge) = row.badge {
                item = item.badge(badge);
            }
            item
        }));
    }
    if items.is_empty() {
        items.push(display_row("quota: no subscription quota reported"));
    }
    items
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
