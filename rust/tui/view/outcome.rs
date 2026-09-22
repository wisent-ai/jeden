//! What a command hands back to the terminal.
//!
//! Split out of `tui/view/mod.rs`, which had grown past the module line cap.

use super::PickerSpec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Text(String),
    Exit(String),
    Picker(PickerSpec),
}

impl CommandOutcome {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub fn into_text(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Exit(text) => text,
            Self::Picker(spec) => {
                let mut lines = vec![spec.title, spec.prompt];
                let render_item = |item: &PickerItem| {
                    let badge = item
                        .badge
                        .as_deref()
                        .map(|value| format!(" [{}]", value))
                        .unwrap_or_default();
                    let detail = if item.detail.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", item.detail)
                    };
                    format!("- {}{}{}", item.label, badge, detail)
                };
                if spec.tabs.len() > 1 {
                    // Shared rows (tab 0) first, then one `── tab (n) ──`
                    // section per non-empty category.
                    for item in spec.items.iter().filter(|item| item.tab == 0) {
                        lines.push(render_item(item));
                    }
                    for (tab, name) in spec.tabs.iter().enumerate().skip(1) {
                        let group: Vec<&PickerItem> =
                            spec.items.iter().filter(|item| item.tab == tab).collect();
                        if group.is_empty() {
                            continue;
                        }
                        lines.push(format!("── {name} ({}) ──", group.len()));
                        for item in group {
                            lines.push(render_item(item));
                        }
                    }
                } else {
                    for item in &spec.items {
                        lines.push(render_item(item));
                    }
                }
                lines.push(tr(&spec.lang, "picker.footer").to_string());
                lines.join("\n")
            }
        }
    }
}

impl From<String> for CommandOutcome {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}
