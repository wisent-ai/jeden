use std::io::{self, Write};

use super::{APP, ASSISTANT_TITLE, FrameOptions, Message, PRODUCT, PromptStatus, SLASH_COMMAND_HINTS, VERSION, WISENT_MARK};
use super::text::{clamp_visible, compact_path, pad_visible, paint, take_visible, visible_len, wrap_line};

pub(super) fn boxed(title: &str, rows: &[String], width: usize, color: bool) -> Vec<String> {
    let clean_title = format!(" {} ", title);
    let inner = width.saturating_sub(4).max(clean_title.chars().count() + 2).max(8);
    let mut normalized = Vec::new();
    for row in rows {
        for part in row.split('\n') {
            normalized.extend(wrap_line(part, inner));
        }
    }
    let top = format!(
        "{}{}{}{}",
        paint("╭", "cyan", color),
        paint(&clean_title, "bold", color),
        paint(&"─".repeat((inner + 2).saturating_sub(clean_title.chars().count())), "cyan", color),
        paint("╮", "cyan", color)
    );
    let mut out = vec![top];
    for row in normalized {
        out.push(format!("{} {} {}", paint("│", "cyan", color), pad_visible(&row, inner), paint("│", "cyan", color)));
    }
    out.push(format!("{}{}{}", paint("╰", "cyan", color), paint(&"─".repeat(inner + 2), "cyan", color), paint("╯", "cyan", color)));
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
    let title = if message.role == "assistant" { ASSISTANT_TITLE } else { message.role.as_str() };
    boxed(title, &message.text.split('\n').map(str::to_string).collect::<Vec<_>>(), width, color)
        .into_iter()
        .map(|line| paint(&line, role_color(&message.role), color))
        .collect()
}

pub(super) fn welcome_panel(width: usize, model: &str, cwd: &str, write_status: &str, command_status: &str, color: bool) -> Vec<String> {
    let title = format!("{} {} {}", PRODUCT, APP, VERSION);
    let inner = width.saturating_sub(4).max(48);
    let left_width = (inner / 3).clamp(24, 34);
    let right_width = inner.saturating_sub(left_width + 3).max(24);
    let cwd_label = compact_path(cwd);
    let mut left = vec![
        String::new(),
        "Welcome back!".to_string(),
        String::new(),
    ];
    left.extend(WISENT_MARK.iter().map(|line| line.to_string()));
    left.extend([
        String::new(),
        if model.is_empty() { "default".to_string() } else { model.to_string() },
        "Jeden CLI".to_string(),
    ]);
    let right = [
        "Tips".to_string(),
        "Type a task and press Enter".to_string(),
        "/help for commands".to_string(),
        "/model to switch routes".to_string(),
        "/update runs automated self-update".to_string(),
        "! and $ shells are not wired yet".to_string(),
        "────────────────────────".to_string(),
        format!("Workspace: {}", cwd_label),
        format!("Tool gates: write {} · command {}", write_status, command_status),
        "CLI: jeden sessions".to_string(),
        "CLI: jeden artifacts <id>".to_string(),
    ];
    let mut rows = Vec::new();
    for index in 0..left.len().max(right.len()) {
        let left_cell = clamp_visible(&left.get(index).cloned().unwrap_or_default(), left_width);
        let right_cell = clamp_visible(&right.get(index).cloned().unwrap_or_default(), right_width);
        rows.push(format!(
            "{} │ {}",
            pad_visible(&left_cell, left_width),
            pad_visible(&right_cell, right_width)
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

pub(super) fn slash_matches(input_text: &str) -> Vec<(&'static str, &'static str)> {
    let Some(prefix) = slash_query(input_text) else {
        return Vec::new();
    };
    SLASH_COMMAND_HINTS
        .iter()
        .filter(|(name, _)| name.starts_with(&prefix))
        .take(6)
        .copied()
        .collect()
}

pub(super) fn complete_slash_input(input_text: &str, selected: usize) -> Option<String> {
    let matches = slash_matches(input_text);
    let (name, _) = matches.get(selected.min(matches.len().saturating_sub(1)))?;
    Some(format!("/{name} "))
}

pub(super) fn slash_hint_panel(input_text: &str, width: usize, color: bool, selected: usize) -> Vec<String> {
    let matches = slash_matches(input_text);
    if matches.is_empty() {
        return Vec::new();
    }
    let selected = selected.min(matches.len().saturating_sub(1));
    let rows: Vec<String> = matches
        .iter()
        .enumerate()
        .map(|(index, (name, description))| {
            let marker = if index == selected { "›" } else { " " };
            format!("{marker} /{:<15} — {}", name, description)
        })
        .collect();
    boxed("slash suggestions", &rows, width, color)
}

pub(super) fn compact_prompt(width: usize, status: &PromptStatus, input_text: &str, _busy: bool, color: bool) -> Vec<String> {
    let inner = width.saturating_sub(2).max(48);
    let model = if status.model.is_empty() { "default" } else { status.model.as_str() };
    let tier = if status.service_tier.is_empty() { "default" } else { status.service_tier.as_str() };
    let branch = status
        .branch
        .as_ref()
        .map(|branch| format!(" > ⑂ {}{}", branch, if status.dirty_count > 0 { format!(" ?{}", status.dirty_count) } else { String::new() }))
        .unwrap_or_default();
    let context = match (status.context_percent, status.context_limit.as_deref()) {
        (Some(percent), Some(limit)) => format!(" > ◫ {:.1}%/{}", percent, limit),
        (_, Some(limit)) => format!(" > ◫ {}", limit),
        _ => String::new(),
    };
    let cost = status.cost.as_ref().map(|cost| format!(" > {}", cost)).unwrap_or_default();
    let label = format!(
        " jeden > ⬢ {} > ↯ {} > 📁 {}{}{}{} ▶ ",
        model,
        tier,
        compact_path(&status.cwd),
        branch,
        context,
        cost
    );
    let safe_label = if visible_len(&label) > inner.saturating_sub(4) {
        format!("{}… ▶ ", take_visible(&label, inner.saturating_sub(7)))
    } else {
        label
    };
    let top = format!(
        "{}{}{}{}",
        paint("╭──", "cyan", color),
        safe_label,
        paint(&"─".repeat(inner.saturating_sub(visible_len(&safe_label) + 2)), "cyan", color),
        paint("╮", "cyan", color)
    );
    // Render the (possibly multiline) input: first line after the ╰─ caret,
    // continuation lines indented under it. Single-line input is unchanged.
    let mut out = vec![top];
    let input_lines: Vec<&str> = input_text.split('\n').collect();
    for (index, line) in input_lines.iter().enumerate() {
        if index == 0 {
            out.push(format!("{} {}", paint("╰─", "cyan", color), line));
        } else {
            out.push(format!("   {}", line));
        }
    }
    out
}

pub(super) fn frame_lines(options: &FrameOptions) -> Vec<String> {
    let width = options.columns.min(120).max(50);
    let prompt = compact_prompt(width, &options.status, &options.input_text, options.busy, options.color);
    let slash_hints = slash_hint_panel(&options.input_text, width, options.color, options.slash_selection);
    let reserved = prompt.len() + slash_hints.len() + 1;
    let available_rows = options.rows.saturating_sub(reserved).max(4);
    let message_lines: Vec<String> = options.messages.iter().flat_map(|message| format_message(message, width, options.color)).collect();
    let mut main_lines = if message_lines.is_empty() {
        welcome_panel(width, &options.status.model, &options.status.cwd, &options.status.write_status, &options.status.command_status, options.color)
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
    let mut lines = vec!["\x1b[2J\x1b[H".to_string()];
    lines.extend(frame_lines(options));
    lines.join("\n")
}

pub fn render_to_stdout(options: &FrameOptions) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(render_terminal_frame(options).as_bytes())?;
    stdout.write_all(b"\x1b[?25h")?;
    stdout.flush()
}
