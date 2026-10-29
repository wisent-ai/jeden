//! Turning a model's price, measured speed and context window into three
//! columns that actually line up.
//!
//! Split out of `cli/run/slash_ui.rs`, which had grown past the module line
//! cap.

use std::time::Duration;

use crate::control_plane::brama::{ModelEntry, ModelPerf, ModelPrice};

/// The minor-unit digits of the US dollar, the currency the catalogue prices
/// in. https://www.iso.org/iso-4217-currency-codes.html
const USD_MINOR_DIGITS: usize = 2;

/// The SI prefix kilo. https://www.bipm.org/en/measurement-units/si-prefixes
const KILO: u64 = 1_000;

/// The SI prefix mega. https://www.bipm.org/en/measurement-units/si-prefixes
const MEGA: u64 = 1_000_000;

/// Compact price per million tokens: to the cent, trailing zeros dropped
/// ($5/30, $0.25/1.25, $2.5/15).
fn format_price(value: f64) -> String {
    format!("{value:.precision$}", precision = USD_MINOR_DIGITS)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

/// Price suffix in the oh-my-pi convention: priced models end with a `█` bar
/// ($5/30█, $0.25/1.25█), free models with `free│`, and a route the
/// catalogue states no input or output price for says so instead of
/// reading as free.
pub(super) fn price_detail(price: Option<&ModelPrice>) -> String {
    let free: f64 = Default::default();
    match price.map(|price| (price.input, price.output)) {
        Some((Some(input), Some(output))) if input > free || output > free => {
            format!(" · ${}/{}█", format_price(input), format_price(output))
        }
        Some((Some(_), Some(_))) => " · free│".into(),
        Some((None, Some(_))) => " · input not priced".into(),
        Some((Some(_), None)) => " · output not priced".into(),
        Some((None, None)) | None => " · not priced".into(),
    }
}

/// Observed performance: latency in seconds (one decimal) and tokens/sec
/// (rounded), appended only when the router has measured the route.
fn perf_detail(perf: Option<&ModelPerf>) -> String {
    match perf {
        Some(perf) if perf.count > Default::default() => {
            let latency = Duration::from_millis(perf.latency_ms.round() as u64);
            format!(" · {:.1}s {:.0}t/s", latency.as_secs_f64(), perf.tps)
        }
        _ => String::new(),
    }
}

/// Context window the way omp prints it: `272k ◫`, `1m ◫`; a route that
/// declares none shows `? ◫`.
pub(super) fn context_metric(tokens: Option<u64>) -> String {
    match tokens {
        None => "? ◫".to_string(),
        Some(tokens) if tokens >= MEGA => format!("{}m ◫", tokens / MEGA),
        Some(tokens) if tokens >= KILO => format!("{}k ◫", tokens / KILO),
        Some(tokens) => format!("{tokens} ◫"),
    }
}

/// The three figures a model row carries, unpadded.
fn metric_parts(model: &ModelEntry) -> Vec<String> {
    vec![
        perf_detail(model.perf.as_ref())
            .trim_start_matches(" · ")
            .to_string(),
        context_metric(model.context_window),
        price_detail(model.price.as_ref())
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
            let measured = part.chars().count();
            match widths.get_mut(slot) {
                Some(width) => *width = (*width).max(measured),
                None => widths.push(measured),
            }
        }
    }
    widths
}

/// The row's figures padded into their columns. A figure in a slot the
/// widths did not measure (they were taken over another list) stands as it
/// is rather than padded to a width nobody measured.
pub(super) fn model_metrics(model: &ModelEntry, widths: &[usize]) -> String {
    metric_parts(model)
        .iter()
        .enumerate()
        .map(|(slot, part)| match widths.get(slot) {
            Some(width) => format!(
                "{}{part}",
                " ".repeat(width.saturating_sub(part.chars().count()))
            ),
            None => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join("  ")
}
