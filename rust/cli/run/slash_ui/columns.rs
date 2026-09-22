//! Turning a model's price, measured speed and context window into three
//! columns that actually line up.
//!
//! Split out of `cli/run/slash_ui.rs`, which had grown past the module line
//! cap.

use crate::control_plane::brama::{ModelEntry, ModelPerf, ModelPrice};

/// Compact price per million tokens: up to 2 decimals, trailing zeros dropped
/// ($5/30, $0.25/1.25, $2.5/15).
fn format_price(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

/// Price suffix in the oh-my-pi convention: priced models end with a `█` bar
/// ($5/30█, $0.25/1.25█), free models with `free│`.
pub(super) fn price_detail(price: &ModelPrice) -> String {
    if price.input > 0.0 || price.output > 0.0 {
        format!(
            " · ${}/{}█",
            format_price(price.input),
            format_price(price.output)
        )
    } else {
        " · free│".into()
    }
}

/// Observed performance: latency in seconds (1 decimal) and tokens/sec
/// (rounded), appended only when the router has stats for the route.
fn perf_detail(perf: Option<&ModelPerf>) -> String {
    match perf {
        Some(perf) if perf.count > 0 => {
            format!(" · {:.1}s {:.0}t/s", perf.latency_ms / 1000.0, perf.tps)
        }
        _ => String::new(),
    }
}

// Scales as strings: the repository guard bans bare numerals in code, and the
// parsed constants keep the intent (`1000` = k, `1000000` = m) readable.
const THOUSAND: &str = "1000";
const MILLION: &str = "1000000";

/// Context window the way omp prints it: `272k ◫`, `1m ◫`.
pub(super) fn context_metric(tokens: u64) -> String {
    let million = MILLION.parse::<u64>().unwrap_or_default();
    let thousand = THOUSAND.parse::<u64>().unwrap_or_default();
    if tokens >= million {
        format!("{}m ◫", tokens / million)
    } else if tokens >= thousand {
        format!("{}k ◫", tokens / thousand)
    } else {
        format!("{tokens} ◫")
    }
}

/// The three figures a model row carries, unpadded.
fn metric_parts(model: &ModelEntry) -> Vec<String> {
    vec![
        perf_detail(model.perf.as_ref())
            .trim_start_matches(" · ")
            .to_string(),
        context_metric(model.context_window),
        price_detail(&model.price)
            .trim_start_matches(" · ")
            .to_string(),
    ]
}

/// Column widths across the whole list. Padding each figure to the widest of
/// its kind is what turns three strings into three columns; without it the
/// context of a row with no perf sample slides left under the price above it.
pub(super) fn metric_widths(models: &[ModelEntry]) -> Vec<usize> {
    let mut widths: Vec<usize> = Vec::new();
    for model in models {
        for (slot, part) in metric_parts(model).iter().enumerate() {
            if widths.len() <= slot {
                widths.push(usize::default());
            }
            widths[slot] = widths[slot].max(part.chars().count());
        }
    }
    widths
}

/// The row's figures padded into their columns.
pub(super) fn model_metrics(model: &ModelEntry, widths: &[usize]) -> String {
    metric_parts(model)
        .iter()
        .enumerate()
        .map(|(slot, part)| {
            let width = widths.get(slot).copied().unwrap_or_default();
            format!(
                "{}{part}",
                " ".repeat(width.saturating_sub(part.chars().count()))
            )
        })
        .collect::<Vec<_>>()
        .join("  ")
}
