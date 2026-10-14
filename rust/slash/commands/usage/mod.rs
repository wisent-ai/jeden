//! What this workspace has used and what is left of the quotas that pay for
//! it.

use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::slash::common::{now_text, refusal_picker, split_head, write_json_value};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

mod quota;
pub(crate) mod totals;

use quota::quota_rows;
use totals::{read_ledger, Agg};

fn usage_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/usage.json")
}

pub(crate) fn usage_picker(context: &SlashContext<'_>) -> PickerSpec {
    let lang = crate::cli::i18n::lang_code(context.cwd);
    let path = usage_path(context.cwd);
    let ledger = match read_ledger(&path) {
        Ok(ledger) => ledger,
        Err(cause) => {
            return refusal_picker(
                &crate::cli::i18n::tr(&lang, "view.usage.title"),
                "Usage accounting cannot be read",
                cause,
            )
        }
    };
    let totals = Agg::of(&ledger.events);
    let total_tokens = totals.total;
    let total_cost = totals.cost.total;
    // A ledger never written has no update time, which the row says.
    let updated = match ledger.updated_at.as_deref() {
        Some(at) => at,
        None => "not recorded",
    };
    let mut items = quota_rows();
    items.extend([
        PickerItem::action("Show usage report", "/usage show")
            .detail(format!(
                "{total_tokens} tokens; recorded cost {total_cost}; updated {updated}"
            ))
            .badge(format!("{} events · project", ledger.events.len())),
        PickerItem::action("Show usage status", "/usage status")
            .detail(format!("Read accounting from {}", path.display()))
            .badge("status"),
        PickerItem::action("Reset usage accounting", "/usage reset")
            .detail(format!("Clear all recorded events in {}", path.display()))
            .badge("destructive")
            .disabled(ledger.events.is_empty()),
    ]);
    PickerSpec::new(crate::cli::i18n::tr(&lang, "view.usage.title"), items).localized(&lang)
}

/// The `/usage` verbs; `status` is a second name for `show`.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum UsageVerb {
    Show,
    Status,
    Reset,
}

pub(crate) fn handle_usage(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, _) = split_head(args);
    let verb = if verb.is_empty() { "show" } else { verb };
    let path = usage_path(context.cwd);
    let Ok(verb) = serde_json::from_value::<UsageVerb>(Value::from(verb)) else {
        return Err("Usage: /usage [show|reset]".into());
    };
    if let UsageVerb::Reset = verb {
        write_json_value(&path, &json!({"updatedAt": now_text()?, "events": []}))?;
        return Ok(format!("Reset usage accounting: {}", path.display()));
    }
    let ledger = read_ledger(&path)?;
    let mut by_model: BTreeMap<String, Agg> = BTreeMap::new();
    for event in &ledger.events {
        by_model.entry(event.model.clone()).or_default().add(event);
    }
    let by_model_value: Map<String, Value> = by_model
        .iter()
        .map(|(model, agg)| (model.clone(), agg.to_json()))
        .collect();
    let mut totals = Agg::of(&ledger.events).to_json();
    totals["byModel"] = Value::Object(by_model_value);
    // The prior "last-N events" cap was an unconsented numeric literal; recent
    // now carries every event in chronological order.
    let summary = json!({
        "file": path,
        "updatedAt": ledger.updated_at,
        "totals": totals,
        "recent": ledger.events,
    });
    serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())
}
