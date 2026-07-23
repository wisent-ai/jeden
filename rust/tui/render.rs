use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::text::{
    clamp_visible, compact_path, paint, sanitize_terminal_text, visible_len, wrap_line,
};
use super::{
    AttachmentTray, EditorState, FollowUpQueue, FrameOptions, Message, PromptStatus,
    RegistryUiRuntime, UiRuntimeAdapter, APP, ASSISTANT_TITLE, PRODUCT, VERSION,
};

const WISENT_MARK: &[&str] = &[
    "        ▄▄▄██▀▀▀▀▀▀██▄▄▄",
    "     ▄█▀▀             ▀▀▀█▄",
    "  ▄██▀                    ▀██",
    " ▄█▀▀▀█▄▄                   ▀█▄",
    "▄█▀     ▀▀▀█▄▄▄              ▀█▄",
    "██▄▄          ▀▀▀██▄▄▄        ██",
    "██▀▀██               ▀▀▀▀██▄▄▄██",
    "▀█▄ ██                       ▄█▀",
    " ▀█▄ ██              ▄▄▄    ▄█▀",
    "   ▀█▄██▄          ▄█▀▀▀▀▀███▀",
    "     ▀████▄    ▄▄██▀  ▄▄▄█▀",
    "        ▀▀▀██████▄▄██▀▀▀",
];

const TWO_COLUMN_WELCOME_MIN_WIDTH: usize = 76;

fn pad_visible(value: &str, width: usize) -> String {
    let mut padded = clamp_visible(value, width);
    padded.extend(std::iter::repeat(' ').take(width.saturating_sub(visible_len(&padded))));
    padded
}

fn framed_header(label: &str, width: usize, color: bool) -> String {
    let middle_width = width.saturating_sub(2);
    let label = clamp_visible(
        &format!(" {} ", sanitize_terminal_text(label)),
        middle_width,
    );
    let rule_width = middle_width.saturating_sub(visible_len(&label));
    format!(
        "{}{}{}{}",
        paint("╭", "cyan", color),
        paint(&label, "bold", color),
        paint(&"─".repeat(rule_width), "cyan", color),
        paint("╮", "cyan", color),
    )
}

fn input_prefix_width(width: usize) -> usize {
    width.saturating_sub(1).min(2)
}

pub(super) fn boxed(title: &str, rows: &[String], width: usize, color: bool) -> Vec<String> {
    let width = width.max(1);
    let title = sanitize_terminal_text(title);
    if width < 6 {
        let mut out = vec![paint(&clamp_visible(&title, width), "bold", color)];
        for row in rows {
            let safe = sanitize_terminal_text(row);
            for logical in safe.split('\n') {
                out.extend(wrap_line(logical, width));
            }
        }
        return out;
    }

    let inner_width = width - 4;
    let mut out = vec![framed_header(&title, width, color)];
    for row in rows {
        let safe = sanitize_terminal_text(row);
        for logical in safe.split('\n') {
            for part in wrap_line(logical, inner_width) {
                out.push(format!(
                    "{} {} {}",
                    paint("│", "cyan", color),
                    pad_visible(&part, inner_width),
                    paint("│", "cyan", color),
                ));
            }
        }
    }
    out.push(format!(
        "{}{}{}",
        paint("╰", "cyan", color),
        paint(&"─".repeat(width - 2), "cyan", color),
        paint("╯", "cyan", color),
    ));
    out
}

fn role_color(role: &str) -> &'static str {
    match role {
        "assistant" => "magenta",
        "error" => "red",
        "system" => "yellow",
        _ => "cyan",
    }
}

pub(super) fn format_message(message: &Message, width: usize, color: bool) -> Vec<String> {
    let width = if io::stdout().is_terminal() {
        crossterm::terminal::size()
            .map(|(columns, _)| width.min(usize::from(columns)).max(1))
            .unwrap_or(width.max(1))
    } else {
        width.max(1)
    };
    let title = sanitize_terminal_text(if message.role == "assistant" {
        ASSISTANT_TITLE
    } else {
        message.role.as_str()
    });
    let safe = sanitize_terminal_text(&message.text);
    let rows = safe.split('\n').map(str::to_string).collect::<Vec<_>>();
    boxed(&title, &rows, width.max(1), color)
        .into_iter()
        .map(|line| paint(&line, role_color(&message.role), color))
        .collect()
}

pub(super) fn welcome_panel(
    width: usize,
    model: &str,
    cwd: &str,
    write_status: &str,
    command_status: &str,
    color: bool,
) -> Vec<String> {
    let width = width.max(1);
    let title = format!("{PRODUCT} {APP} {VERSION}");
    let model = sanitize_terminal_text(if model.is_empty() { "default" } else { model });
    let workspace = sanitize_terminal_text(&compact_path(cwd));
    let write_status = sanitize_terminal_text(write_status);
    let command_status = sanitize_terminal_text(command_status);

    if width < TWO_COLUMN_WELCOME_MIN_WIDTH {
        let rows = vec![
            "Welcome back!".to_string(),
            format!("Model: {model}"),
            format!("Workspace: {workspace}"),
            format!("Permissions: write {write_status} · command {command_status}"),
            String::new(),
            "Tips".to_string(),
            "Type a task and press Enter".to_string(),
            "/help commands · /model routes".to_string(),
            "Enter send · Alt+Enter newline · Ctrl+C exit".to_string(),
        ];
        return boxed(&title, &rows, width, color);
    }

    let inner_width = width - 4;
    let left_width = 34;
    let right_width = inner_width.saturating_sub(left_width + 3);
    let mut left = vec![String::new(), "Welcome back!".to_string(), String::new()];
    left.extend(WISENT_MARK.iter().map(|line| (*line).to_string()));
    left.extend([String::new(), model.clone(), "Jeden CLI".to_string()]);
    let right = [
        "Tips".to_string(),
        "Type a task and press Enter".to_string(),
        "/help for commands".to_string(),
        "/model to switch routes".to_string(),
        "/update runs automated self-update".to_string(),
        "Ctrl+V pastes text or adds an attachment".to_string(),
        "Alt+Backspace removes the last attachment".to_string(),
        "────────────────────────".to_string(),
        format!("Workspace: {workspace}"),
        format!("Permissions: write {write_status}"),
        format!("             command {command_status}"),
        "────────────────────────".to_string(),
        "Enter send · Alt+Enter newline".to_string(),
        "Tab complete · ↑↓ history/select".to_string(),
        "Esc clear · Ctrl+C exit".to_string(),
        "CLI: jeden sessions".to_string(),
        "CLI: jeden artifacts <id>".to_string(),
    ];
    let mut rows = Vec::with_capacity(left.len().max(right.len()));
    for index in 0..left.len().max(right.len()) {
        let left_cell = left.get(index).map(String::as_str).unwrap_or_default();
        let right_cell = right.get(index).map(String::as_str).unwrap_or_default();
        rows.push(format!(
            "{} │ {}",
            pad_visible(left_cell, left_width),
            clamp_visible(right_cell, right_width),
        ));
    }
    boxed(&title, &rows, width, color)
}

fn slash_query(input_text: &str) -> Option<String> {
    let text = input_text.trim_start();
    if !text.starts_with('/') || text.contains('\n') {
        return None;
    }
    let query = text.trim_start_matches('/');
    if query.contains(char::is_whitespace) {
        return None;
    }
    Some(query.to_ascii_lowercase())
}

pub(super) fn slash_matches(input_text: &str) -> Vec<(String, String)> {
    let Some(prefix) = slash_query(input_text) else {
        return Vec::new();
    };
    crate::capability::snapshot()
        .executable_kind(crate::capability::CapabilityKind::SlashCommand)
        .filter_map(|descriptor| {
            let action = descriptor.ui.action.as_deref()?.strip_prefix('/')?;
            action
                .starts_with(&prefix)
                .then(|| (action.to_string(), descriptor.ui.description.clone()))
        })
        .take(6)
        .collect()
}

pub(super) fn complete_slash_input(input_text: &str, selected: usize) -> Option<String> {
    let matches = slash_matches(input_text);
    let (name, _) = matches.get(selected.min(matches.len().saturating_sub(1)))?;
    Some(format!("/{name} "))
}

pub(super) fn slash_hint_panel(
    input_text: &str,
    width: usize,
    color: bool,
    selected: usize,
) -> Vec<String> {
    let matches = slash_matches(input_text);
    if matches.is_empty() {
        return Vec::new();
    }
    let selected = selected.min(matches.len().saturating_sub(1));
    let rows: Vec<String> = matches
        .iter()
        .enumerate()
        .map(|(index, (name, description))| {
            let marker = if index == selected { ">" } else { " " };
            format!("{marker} /{:<15}  {}", name, description)
        })
        .collect();
    boxed("slash suggestions", &rows, width, color)
}

/// Live status-line extras read from project state files (`.jeden/usage.json`
/// counters and `.jeden/mode-state.json`). Reads are cached for a few seconds
/// so per-keystroke renders stay cheap.
#[derive(Clone)]
struct StatusExtras {
    tokens: f64,
    cost: f64,
    plan: bool,
    goal: bool,
    loop_mode: bool,
    advisor: bool,
}

struct ExtrasCache {
    cwd: PathBuf,
    fetched_at: Instant,
    extras: StatusExtras,
}

static EXTRAS_CACHE: Mutex<Option<ExtrasCache>> = Mutex::new(None);
const EXTRAS_TTL: Duration = Duration::from_secs(3);

/// Total tokens and recorded cost across all events, summed exactly like the
/// `/usage` report does over the same `.jeden/usage.json` file.
fn usage_totals(cwd: &Path) -> (f64, f64) {
    let usage: serde_json::Value = std::fs::read_to_string(cwd.join(".jeden/usage.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);
    let events = usage
        .get("events")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut tokens = 0.0;
    let mut cost = 0.0;
    for event in events {
        let number = |key: &str| event.get(key).and_then(serde_json::Value::as_f64);
        tokens += event
            .get("totalTokens")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_else(|| {
                let direct = ["inputTokens", "outputTokens"]
                    .iter()
                    .filter_map(|key| number(key))
                    .sum::<f64>();
                let cache_read = number("cacheReadTokens")
                    .or_else(|| number("cacheRead"))
                    .unwrap_or_default();
                let cache_write = number("cacheWriteTokens")
                    .or_else(|| number("cacheWrite"))
                    .unwrap_or_default();
                direct + cache_read + cache_write
            });
        cost += event
            .pointer("/cost/total")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_else(|| {
                ["input", "output", "cacheRead", "cacheWrite"]
                    .iter()
                    .map(|key| {
                        event
                            .pointer(&format!("/cost/{key}"))
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or_default()
                    })
                    .sum::<f64>()
            });
    }
    (tokens, cost)
}

fn status_extras(cwd: &Path) -> Option<StatusExtras> {
    let mut guard = EXTRAS_CACHE.lock().ok()?;
    let fresh = guard
        .as_ref()
        .is_some_and(|cache| cache.cwd == cwd && cache.fetched_at.elapsed() < EXTRAS_TTL);
    if !fresh {
        let (tokens, cost) = usage_totals(cwd);
        let state = crate::slash::read_mode_state(cwd);
        *guard = Some(ExtrasCache {
            cwd: cwd.to_path_buf(),
            fetched_at: Instant::now(),
            extras: StatusExtras {
                tokens,
                cost,
                plan: state.plan.enabled,
                goal: state.goal.enabled || state.guided_goal.active,
                loop_mode: state.loop_mode.enabled,
                advisor: state.advisor.enabled,
            },
        });
    }
    guard.as_ref().map(|cache| cache.extras.clone())
}

/// Live session spend: recorded cost when nonzero, otherwise the token form
/// (`~N tok`); nothing when no usage has been recorded yet.
fn cost_segment(status: &PromptStatus, extras: &StatusExtras) -> Option<String> {
    if let Some(cost) = &status.cost {
        return Some(format!("cost {}", sanitize_terminal_text(cost)));
    }
    if extras.cost > 0.0 {
        Some(format!("${:.2}", extras.cost))
    } else if extras.tokens > 0.0 {
        Some(format!("~{} tok", extras.tokens.round() as u64))
    } else {
        None
    }
}

/// Compact badges for the active modes, ordered after the route segment.
fn mode_badges(extras: &StatusExtras) -> Vec<String> {
    let mut badges = Vec::new();
    if extras.plan {
        badges.push("plan".to_string());
    }
    if extras.goal {
        badges.push("goal".to_string());
    }
    if extras.loop_mode {
        badges.push("loop".to_string());
    }
    if extras.advisor {
        badges.push("adv".to_string());
    }
    badges
}

struct QuotaCache {
    fetched_at: Instant,
    percent_free: Option<u64>,
    refreshing: bool,
}

static QUOTA_CACHE: Mutex<Option<QuotaCache>> = Mutex::new(None);
const QUOTA_TTL: Duration = Duration::from_secs(60);

/// Most-constrained Weles subscription quota (percent free), refreshed on a
/// detached thread so renders never block on the network. `None` until the
/// first refresh lands and whenever Weles is unconfigured or unreachable, so
/// the status line just skips the segment.
fn quota_percent_free_cached() -> Option<u64> {
    let mut guard = QUOTA_CACHE.lock().ok()?;
    let refresh = match guard.as_ref() {
        Some(cache) => cache.fetched_at.elapsed() >= QUOTA_TTL && !cache.refreshing,
        None => true,
    };
    if refresh {
        if let Some(cache) = guard.as_mut() {
            cache.refreshing = true;
        } else {
            *guard = Some(QuotaCache {
                fetched_at: Instant::now(),
                percent_free: None,
                refreshing: true,
            });
        }
        std::thread::spawn(|| {
            let percent_free =
                crate::control_plane::quota::fetch_subscription_quotas().min_percent_free();
            if let Ok(mut guard) = QUOTA_CACHE.lock() {
                *guard = Some(QuotaCache {
                    fetched_at: Instant::now(),
                    percent_free,
                    refreshing: false,
                });
            }
        });
    }
    guard.as_ref().and_then(|cache| cache.percent_free)
}

pub(super) fn compact_prompt(
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

pub(super) fn place_editor_cursor(
    lines: &mut [String],
    input: &str,
    cursor: usize,
    width: usize,
    trailing_rows: usize,
) -> usize {
    let rendered_input_rows = lines.len().saturating_sub(trailing_rows + 1);
    let prefix_width = input_prefix_width(width.max(1));
    let content_width = width.saturating_sub(prefix_width).max(1);
    let safe_prefix = sanitize_terminal_text(&input[..cursor.min(input.len())]);
    let mut cursor_row = 0usize;
    let mut cursor_column = 0usize;
    for logical in safe_prefix.split('\n') {
        if cursor_row > 0 || cursor_column > 0 {
            cursor_row += 1;
        }
        let logical_width = visible_len(logical);
        if logical_width == 0 {
            cursor_column = 0;
        } else {
            cursor_row += (logical_width - 1) / content_width;
            cursor_column = ((logical_width - 1) % content_width) + 1;
        }
    }
    let up = trailing_rows + rendered_input_rows.saturating_sub(cursor_row + 1);
    let Some(last) = lines.last_mut() else {
        return 0;
    };
    if up > 0 {
        last.push_str(&format!("\x1b[{up}A"));
    }
    last.push('\r');
    let column = cursor_column + prefix_width;
    if column > 0 {
        last.push_str(&format!("\x1b[{column}C"));
    }
    up
}

pub(super) fn attachment_lines(tray: &AttachmentTray, width: usize, color: bool) -> Vec<String> {
    if tray.items().is_empty() {
        return Vec::new();
    }
    let heading = clamp_visible(
        &format!("Attachments ({})", tray.items().len()),
        width.max(1),
    );
    let mut lines = vec![paint(&heading, "bold", color)];
    for attachment in tray.items() {
        lines.push(clamp_visible(
            &format!("  {}", sanitize_terminal_text(&attachment.fallback_label())),
            width.max(1),
        ));
    }
    lines
}

pub(super) fn busy_editor_lines(
    editor: &EditorState,
    queue: &FollowUpQueue,
    width: usize,
    color: bool,
) -> Vec<String> {
    let width = width.max(1);
    let prefix_width = input_prefix_width(width);
    let content_width = width.saturating_sub(prefix_width).max(1);
    let label = if queue.is_empty() {
        "Follow-up".to_string()
    } else {
        format!("Follow-up ({} queued)", queue.len())
    };
    let hotkeys = clamp_visible("[Enter] queue  [Ctrl+Enter] steer  [Alt+Up] recall", width);
    let lines_label = clamp_visible(&label, width);
    let mut lines = vec![
        paint(&hotkeys, "dim", color),
        paint(&lines_label, "dim", color),
    ];
    let safe = sanitize_terminal_text(editor.text());
    let mut first = true;
    for logical in safe.split('\n') {
        for part in wrap_line(logical, content_width) {
            let prefix = if first && prefix_width == 2 {
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
            lines.push(format!("{prefix}{part}"));
            first = false;
        }
    }
    if first {
        lines.push(if prefix_width == 2 {
            "> ".into()
        } else if prefix_width == 1 {
            ">".into()
        } else {
            String::new()
        });
    }
    lines
}

pub(super) fn frame_lines(options: &FrameOptions) -> Vec<String> {
    let width = options.columns.min(112).max(1);
    let prompt = compact_prompt(
        width,
        &options.status,
        &options.input_text,
        options.busy,
        options.color,
    );
    let slash_hints = slash_hint_panel(
        &options.input_text,
        width,
        options.color,
        options.slash_selection,
    );
    let reserved = prompt.len() + slash_hints.len() + 1;
    let available_rows = options.rows.saturating_sub(reserved).max(4);
    let message_lines: Vec<String> = options
        .messages
        .iter()
        .flat_map(|message| format_message(message, width, options.color))
        .collect();
    let mut main_lines = if message_lines.is_empty() {
        welcome_panel(
            width,
            &options.status.model,
            &options.status.cwd,
            &options.status.write_status,
            &options.status.command_status,
            options.color,
        )
    } else {
        message_lines
    };
    if main_lines.len() > available_rows {
        main_lines = main_lines.split_off(main_lines.len() - available_rows);
    }
    let mut lines = Vec::new();
    lines.extend(main_lines);
    lines.extend(std::iter::repeat(String::new()).take(available_rows.saturating_sub(lines.len())));
    lines.extend(slash_hints);
    lines.extend(prompt);
    // Never let an embedded newline reach the absolute-positioned diff renderer.
    lines
        .into_iter()
        .flat_map(|line| line.split('\n').map(str::to_string).collect::<Vec<_>>())
        .collect()
}

pub fn render_terminal_frame(options: &FrameOptions) -> String {
    let lines = frame_lines(options);
    if options.color {
        format!("\x1b[2J\x1b[H{}", lines.join("\n"))
    } else {
        lines
            .into_iter()
            .map(|line| sanitize_terminal_text(&line))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
