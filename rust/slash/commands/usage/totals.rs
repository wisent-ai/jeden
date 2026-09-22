//! Adding up what a workspace has spent.
//!
//! Split out of `slash/commands/usage.rs`, which had grown past the module
//! line cap.
use serde_json::Value;
use serde_json::json;

/// Running accumulation of usage counters. `Default` zeroes every field, so no
/// bare numeric initializers are needed.
#[derive(Default)]
pub(super) struct Agg {
    pub(super) calls: u64,
    pub(super) input: f64,
    pub(super) output: f64,
    pub(super) cache_read: f64,
    pub(super) cache_write: f64,
    pub(super) total: f64,
    pub(super) cost_input: f64,
    pub(super) cost_output: f64,
    pub(super) cost_cache_read: f64,
    pub(super) cost_cache_write: f64,
    pub(super) cost_total: f64,
}

impl Agg {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn add(
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
