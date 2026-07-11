use std::io::{self, IsTerminal};
use std::path::Path;

use super::text::{
    clamp_visible, compact_path, paint, sanitize_terminal_text, visible_len, wrap_line,
};
use super::{
    AttachmentTray, EditorState, FollowUpQueue, FrameOptions, Message, PromptStatus,
    RegistryUiRuntime, UiRuntimeAdapter, ASSISTANT_TITLE, PRODUCT, VERSION,
};

pub(super) fn boxed(title: &str, rows: &[String], width: usize, color: bool) -> Vec<String> {
    let width = width.max(1);
    let mut out = vec![paint(&sanitize_terminal_text(title), "bold", color)];
    for row in rows {
        let safe = sanitize_terminal_text(row);
        for part in safe.split('\n') {
            out.extend(wrap_line(part, width));
        }
    }
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
    let rows = safe
        .split('\n')
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>();
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
    let model = sanitize_terminal_text(if model.is_empty() { "default" } else { model });
    let write_status = sanitize_terminal_text(write_status);
    let command_status = sanitize_terminal_text(command_status);
    let brand = paint(PRODUCT, "bold", color);
    let version = paint(VERSION, "dim", color);
    let workspace = sanitize_terminal_text(&compact_path(cwd));
    vec![
        clamp_visible(&format!("{brand}  {version}"), width),
        clamp_visible(&format!("model {model} | workspace {workspace}"), width),
        clamp_visible(
            &format!("permissions: write {write_status} | command {command_status}"),
            width,
        ),
        paint(
            "[Enter] send  [Alt+Enter] newline  [Ctrl+C] exit",
            "dim",
            color,
        ),
        String::new(),
    ]
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

pub(super) fn compact_prompt(
    width: usize,
    status: &PromptStatus,
    input_text: &str,
    _busy: bool,
    color: bool,
) -> Vec<String> {
    let width = width.max(1);
    let content_width = width.saturating_sub(2).max(1);
    let model = sanitize_terminal_text(if status.model.is_empty() {
        "default"
    } else {
        status.model.as_str()
    });
    let mut segments = vec![
        format!("jeden. {model}"),
        sanitize_terminal_text(&compact_path(&status.cwd)),
    ];
    if !status.service_tier.is_empty() {
        segments.push(format!(
            "route {}",
            sanitize_terminal_text(&status.service_tier)
        ));
    }
    if let Some(branch) = &status.branch {
        segments.push(format!(
            "{}{}",
            sanitize_terminal_text(branch),
            if status.dirty_count > 0 { " dirty" } else { "" }
        ));
    }
    match (status.context_percent, status.context_limit.as_deref()) {
        (Some(percent), Some(limit)) => segments.push(format!(
            "context {percent:.1}% {}",
            sanitize_terminal_text(limit)
        )),
        (_, Some(limit)) => segments.push(format!("context {}", sanitize_terminal_text(limit))),
        (Some(percent), None) => segments.push(format!("context {percent:.1}%")),
        _ => {}
    }
    if let Some(cost) = &status.cost {
        segments.push(format!("cost {}", sanitize_terminal_text(cost)));
    }
    let runtime = RegistryUiRuntime.runtime_status(Path::new(&status.cwd));
    if let Some(route_health) = runtime.route_health {
        segments.push(format!("route {route_health}"));
    }
    if let Some(active_jobs) = runtime.active_jobs {
        segments.push(format!("jobs {active_jobs}"));
    }
    if runtime.services_degraded > 0 || runtime.services_unavailable > 0 {
        segments.push(format!(
            "services {}/{}",
            runtime.services_degraded, runtime.services_unavailable
        ));
    }
    if !status.write_status.is_empty() {
        segments.push(format!(
            "write {}",
            sanitize_terminal_text(&status.write_status)
        ));
    }
    let status_line = clamp_visible(&segments.join(" | "), width);
    let mut out = vec![paint(&status_line, "dim", color)];
    let safe_input = sanitize_terminal_text(input_text);
    let mut first = true;
    for logical in safe_input.split('\n') {
        let wrapped = wrap_line(logical, content_width);
        for line in wrapped {
            out.push(format!("{}{}", if first { "> " } else { "  " }, line));
            first = false;
        }
        if logical.is_empty() && !first {
            continue;
        }
    }
    if first {
        out.push("> ".to_string());
    }
    out
}

pub(super) fn place_editor_cursor(lines: &mut [String], input: &str, cursor: usize, width: usize) {
    let rendered_input_rows = lines.len().saturating_sub(1);
    let content_width = width.saturating_sub(2).max(1);
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
    let up = rendered_input_rows.saturating_sub(cursor_row + 1);
    let Some(last) = lines.last_mut() else {
        return;
    };
    if up > 0 {
        last.push_str(&format!("\x1b[{up}A"));
    }
    last.push('\r');
    let column = cursor_column + 2;
    if column > 0 {
        last.push_str(&format!("\x1b[{column}C"));
    }
}

pub(super) fn attachment_lines(tray: &AttachmentTray, width: usize, color: bool) -> Vec<String> {
    if tray.items().is_empty() {
        return Vec::new();
    }
    let mut lines = vec![paint(
        &format!("Attachments ({})", tray.items().len()),
        "bold",
        color,
    )];
    for attachment in tray.items() {
        lines.push(clamp_visible(
            &format!("  {}", attachment.fallback_label()),
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
    let content_width = width.saturating_sub(2).max(1);
    let label = if queue.is_empty() {
        "Follow-up".to_string()
    } else {
        format!("Follow-up ({} queued)", queue.len())
    };
    let mut lines = vec![
        paint(
            "[Enter] queue  [Ctrl+Enter] steer  [Alt+Up] recall",
            "dim",
            color,
        ),
        paint(&label, "dim", color),
    ];
    let safe = sanitize_terminal_text(editor.text());
    let mut first = true;
    for logical in safe.split('\n') {
        for part in wrap_line(logical, content_width) {
            lines.push(format!("{}{}", if first { "> " } else { "  " }, part));
            first = false;
        }
    }
    if first {
        lines.push("> ".into());
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
