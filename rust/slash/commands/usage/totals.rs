//! Adding up what a workspace has spent, from the usage ledger the agent
//! writes (`agent::runtime::routing::usage::append_usage_event`).
//!
//! The ledger is read as the shape that writer gives every event, so `/usage`,
//! its picker and the status line add up the same fields and no reader guesses
//! at another spelling of a field the writer never wrote.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

/// What one call cost, priced from the model catalogue; absent when the
/// catalogue named no price for the model.
#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventCost {
    pub(crate) input: f64,
    pub(crate) output: f64,
    pub(crate) cache_read: f64,
    pub(crate) cache_write: f64,
    pub(crate) total: f64,
}

/// One model call as the ledger records it.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageEvent {
    pub(crate) model: String,
    pub(crate) input_tokens: f64,
    pub(crate) output_tokens: f64,
    pub(crate) cache_read_tokens: f64,
    pub(crate) cache_write_tokens: f64,
    pub(crate) total_tokens: f64,
    pub(crate) cost: Option<EventCost>,
}

/// The ledger: when it last changed and every call it records.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Ledger {
    pub(crate) updated_at: Option<String>,
    pub(crate) events: Vec<UsageEvent>,
}

/// The ledger at `path`. One that does not exist holds no call and was never
/// updated; one that cannot be read, or whose events are not the shape the
/// writer gives them, is refused naming the file and the field.
pub(crate) fn read_ledger(path: &Path) -> Result<Ledger, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Ledger {
                updated_at: None,
                events: Vec::new(),
            })
        }
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    };
    serde_json::from_str::<Ledger>(&text)
        .map_err(|error| format!("{} is not a usage ledger: {error}", path.display()))
}

/// Running accumulation of usage counters. `Default` zeroes every field, so no
/// bare numeric initializers are needed.
#[derive(Default)]
pub(crate) struct Agg {
    pub(crate) calls: u64,
    pub(crate) input: f64,
    pub(crate) output: f64,
    pub(crate) cache_read: f64,
    pub(crate) cache_write: f64,
    pub(crate) total: f64,
    pub(crate) cost: EventCost,
}

impl Agg {
    /// Every event of `events`, added up.
    pub(crate) fn of(events: &[UsageEvent]) -> Self {
        let mut agg = Self::default();
        for event in events {
            agg.add(event);
        }
        agg
    }

    pub(crate) fn add(&mut self, event: &UsageEvent) {
        // One call per event.
        self.calls += u64::from(crate::tally::STEP);
        self.input += event.input_tokens;
        self.output += event.output_tokens;
        self.cache_read += event.cache_read_tokens;
        self.cache_write += event.cache_write_tokens;
        self.total += event.total_tokens;
        if let Some(cost) = event.cost {
            self.cost.input += cost.input;
            self.cost.output += cost.output;
            self.cost.cache_read += cost.cache_read;
            self.cost.cache_write += cost.cache_write;
            self.cost.total += cost.total;
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "calls": self.calls,
            "inputTokens": self.input,
            "outputTokens": self.output,
            "cacheReadTokens": self.cache_read,
            "cacheWriteTokens": self.cache_write,
            "totalTokens": self.total,
            "cost": self.cost,
        })
    }
}
