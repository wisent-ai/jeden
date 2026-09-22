//! The suggestion panel shown while an operator is typing a slash command.
//!
//! Split out of `tui/render/mod.rs`, which had grown past the module line cap.

use super::boxes::boxed;
use crate::tui::text::sanitize_terminal_text;

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

pub(crate) fn slash_matches(input_text: &str) -> Vec<(String, String)> {
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

pub(crate) fn complete_slash_input(input_text: &str, selected: usize) -> Option<String> {
    let matches = slash_matches(input_text);
    let (name, _) = matches.get(selected.min(matches.len().saturating_sub(1)))?;
    Some(format!("/{name} "))
}

pub(crate) fn slash_hint_panel(
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
