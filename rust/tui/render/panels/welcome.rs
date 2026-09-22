//! The panel shown when the terminal opens.
//!
//! Split out of `tui/render/mod.rs`, which had grown past the module line cap.

use super::boxes::{boxed, pad_visible};
use crate::tui::text::clamp_visible;
use crate::tui::text::{compact_path, sanitize_terminal_text};
use crate::tui::{APP, PRODUCT, VERSION};
use std::path::Path;

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

pub(crate) fn welcome_panel(
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
    // First-run hint, shown only while no Brama router is configured.
    let setup_tip = !crate::slash::setup::brama_router_configured(Path::new(cwd));

    if width < TWO_COLUMN_WELCOME_MIN_WIDTH {
        let mut rows = vec![
            "Welcome back!".to_string(),
            format!("Model: {model}"),
            format!("Workspace: {workspace}"),
            format!("Permissions: write {write_status} · command {command_status}"),
            String::new(),
            "Tips".to_string(),
            "Type a task and press Enter".to_string(),
            "/help commands · /model routes".to_string(),
        ];
        if setup_tip {
            rows.push("run /setup to connect a model router".to_string());
        }
        rows.push("Enter send · Alt+Enter newline · Ctrl+C exit".to_string());
        return boxed(&title, &rows, width, color);
    }

    let inner_width = width - 4;
    let left_width = 34;
    let right_width = inner_width.saturating_sub(left_width + 3);
    let mut left = vec![String::new(), "Welcome back!".to_string(), String::new()];
    left.extend(WISENT_MARK.iter().map(|line| (*line).to_string()));
    left.extend([String::new(), model.clone(), "Jeden CLI".to_string()]);
    let mut right = vec![
        "Tips".to_string(),
        "Type a task and press Enter".to_string(),
        "/help for commands".to_string(),
        "/model to switch routes".to_string(),
    ];
    if setup_tip {
        right.push("run /setup to connect a model router".to_string());
    }
    right.extend([
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
    ]);
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
