//! The prompt: the input line, the status line beneath it, and what happens
//! to both as the terminal narrows.
//!
//! Split out of `tui/render/mod.rs`, which had grown past the module line cap.

use super::panels::boxes::{frame_inner, prompt_corner, INPUT_PREFIX};
use super::panels::{framed_header, input_prefix_width};
use crate::tui::text::{
    clamp_visible, compact_path, paint, sanitize_terminal_text, visible_len, wrap_line,
};

mod status;

use crate::tui::integration::{RegistryUiRuntime, UiRuntimeAdapter};
use crate::tui::PromptStatus;
use crate::tui::APP;
use crate::tui::PRODUCT;
use crate::tui::VERSION;
use status::{cost_segment, mode_badges, quota_percent_free_cached, status_extras};
use std::path::Path;

pub(crate) fn compact_prompt(
    width: usize,
    status: &PromptStatus,
    input_text: &str,
    busy: bool,
    color: bool,
) -> Vec<String> {
    let one = usize::from(crate::tally::STEP);
    let width = width.max(one);
    let prefix_width = input_prefix_width(width);
    let content_width = width.saturating_sub(prefix_width).max(one);
    let model = match status.model.as_str() {
        "" => "model not chosen".to_string(),
        chosen => format!("model {}", sanitize_terminal_text(chosen)),
    };
    let mut head = vec![
        format!("{PRODUCT} {APP} {VERSION}"),
        model,
        sanitize_terminal_text(&compact_path(&status.cwd)),
    ];
    if busy {
        head.push("busy".to_string());
    }
    if !status.service_tier.is_empty() {
        head.push(format!(
            "route {}",
            sanitize_terminal_text(&status.service_tier)
        ));
    }
    // Live segments: mode badges slot in after the route segment; the cost
    // and quota segments follow the context segment.
    let extras = status_extras(Path::new(&status.cwd));
    let badges: Vec<String> = extras
        .as_ref()
        .map(mode_badges)
        .into_iter()
        .flatten()
        .collect();
    let cost_text = extras
        .as_ref()
        .and_then(|extras| cost_segment(status, extras));
    let quota_text = quota_percent_free_cached().map(|percent| format!("quota {percent}%"));
    let mut mid = Vec::new();
    if let Some(branch) = &status.branch {
        mid.push(format!(
            "{}{}",
            sanitize_terminal_text(branch),
            if status.dirty_count != crate::tally::ORIGIN {
                " dirty"
            } else {
                ""
            }
        ));
    }
    match (status.context_percent, status.context_limit.as_deref()) {
        (Some(percent), Some(limit)) => mid.push(format!(
            "context {percent:.1}% {}",
            sanitize_terminal_text(limit)
        )),
        (_, Some(limit)) => mid.push(format!("context {}", sanitize_terminal_text(limit))),
        (Some(percent), None) => mid.push(format!("context {percent:.1}%")),
        _ => {}
    }
    let mut tail = Vec::new();
    let runtime = RegistryUiRuntime.runtime_status(Path::new(&status.cwd));
    if let Some(route_health) = runtime.route_health {
        tail.push(format!("route {route_health}"));
    }
    if let Some(active_jobs) = runtime.active_jobs {
        tail.push(format!("jobs {active_jobs}"));
    }
    if runtime.services_degraded != crate::tally::ORIGIN
        || runtime.services_unavailable != crate::tally::ORIGIN
    {
        tail.push(format!(
            "services {}/{}",
            runtime.services_degraded, runtime.services_unavailable
        ));
    }
    if !status.write_status.is_empty() {
        tail.push(format!(
            "write {}",
            sanitize_terminal_text(&status.write_status)
        ));
    }
    let compose = |with_badges: bool, with_cost: bool, with_quota: bool| {
        let mut segments = head.clone();
        if with_badges {
            segments.extend(badges.iter().cloned());
        }
        segments.extend(mid.iter().cloned());
        if with_cost {
            segments.extend(cost_text.iter().cloned());
        }
        if with_quota {
            segments.extend(quota_text.iter().cloned());
        }
        segments.extend(tail.iter().cloned());
        segments.join(" | ")
    };
    let mut status_line = compose(true, true, true);
    // Narrow terminals shed the live segments first — quota, then cost, then
    // the mode badges; framed_header still clamps whatever remains.
    let framed = frame_inner(width);
    let overflows = |line: &str| framed.is_some_and(|inner| visible_len(line) > inner);
    if overflows(&status_line) {
        status_line = compose(true, true, false);
    }
    if overflows(&status_line) {
        status_line = compose(true, false, false);
    }
    if overflows(&status_line) {
        status_line = compose(false, false, false);
    }
    let mut out = match framed {
        Some(_) => vec![framed_header(&status_line, width, color)],
        None => vec![paint(&clamp_visible(&status_line, width), "dim", color)],
    };
    // A framed prompt opens on the frame's corner; an unframed one on as much
    // of the input prefix as fits. The rows after the first are indented as far.
    let first_prefix: String = match framed {
        Some(_) => prompt_corner(),
        None => INPUT_PREFIX.chars().take(prefix_width).collect(),
    };
    let indent = " ".repeat(prefix_width);
    let safe_input = sanitize_terminal_text(input_text);
    let mut first = true;
    for logical in safe_input.split('\n') {
        for line in wrap_line(logical, content_width) {
            let prefix = if first { &first_prefix } else { &indent };
            out.push(format!("{prefix}{line}"));
            first = false;
        }
    }
    if first {
        out.push(first_prefix);
    }
    out
}
