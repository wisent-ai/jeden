//! The prompt: the input line, the status line beneath it, and what happens
//! to both as the terminal narrows.
//!
//! Split out of `tui/render/mod.rs`, which had grown past the module line cap.

use super::panels::{framed_header, input_prefix_width, pad_visible};
use crate::tui::text::{
    clamp_visible, compact_path, paint, sanitize_terminal_text, visible_len, wrap_line,
};

mod status;

use status::{cost_segment, mode_badges, quota_percent_free_cached, status_extras};
use crate::tui::PromptStatus;
use std::path::Path;
use crate::tui::APP;
use crate::tui::PRODUCT;
use crate::tui::VERSION;
use crate::tui::integration::RegistryUiRuntime;

pub(crate) fn compact_prompt(
    width: usize,
    status: &PromptStatus,
    input_text: &str,
    busy: bool,
    color: bool,
) -> Vec<String> {
    let width = width.max(1);
    let prefix_width = input_prefix_width(width);
    let content_width = width.saturating_sub(prefix_width).max(1);
    let model = sanitize_terminal_text(if status.model.is_empty() {
        "default"
    } else {
        status.model.as_str()
    });
    let mut head = vec![
        format!("{PRODUCT} {APP} {VERSION}"),
        format!("model {model}"),
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
    let badges = extras.as_ref().map(mode_badges).unwrap_or_default();
    let cost_text = extras
        .as_ref()
        .and_then(|extras| cost_segment(status, extras));
    let quota_text = quota_percent_free_cached().map(|percent| format!("quota {percent}%"));
    let mut mid = Vec::new();
    if let Some(branch) = &status.branch {
        mid.push(format!(
            "{}{}",
            sanitize_terminal_text(branch),
            if status.dirty_count > 0 { " dirty" } else { "" }
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
    if runtime.services_degraded > 0 || runtime.services_unavailable > 0 {
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
    let overflows = |line: &str| width >= 6 && visible_len(line) + 4 > width;
    if overflows(&status_line) {
        status_line = compose(true, true, false);
    }
    if overflows(&status_line) {
        status_line = compose(true, false, false);
    }
    if overflows(&status_line) {
        status_line = compose(false, false, false);
    }
    let mut out = if width >= 6 {
        vec![framed_header(&status_line, width, color)]
    } else {
        vec![paint(&clamp_visible(&status_line, width), "dim", color)]
    };
    let safe_input = sanitize_terminal_text(input_text);
    let mut first = true;
    for logical in safe_input.split('\n') {
        let wrapped = wrap_line(logical, content_width);
        for line in wrapped {
            let prefix = if first && width >= 6 {
                "╰─"
            } else if first && prefix_width == 2 {
                "> "
            } else if first && prefix_width == 1 {
                ">"
            } else if first {
                ""
            } else if prefix_width == 2 {
                "  "
            } else if prefix_width == 1 {
                " "
            } else {
                ""
            };
            out.push(format!("{prefix}{line}"));
            first = false;
        }
        if logical.is_empty() && !first {
            continue;
        }
    }
    if first {
        out.push(if width >= 6 {
            "╰─".to_string()
        } else if prefix_width == 2 {
            "> ".to_string()
        } else if prefix_width == 1 {
            ">".to_string()
        } else {
            String::new()
        });
    }
    out
}
