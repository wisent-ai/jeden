use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::cli::i18n::tr;

const INITIAL_SELECTION: usize = usize::MIN;
const SELECTION_STEP: usize = true as usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    pub label: String,
    pub detail: String,
    pub badge: Option<String>,
    pub command: Option<String>,
    pub disabled: bool,
    pub destructive: bool,
    pub prefill: bool,
    /// Index into `PickerSpec::tabs`; 0 = shown in every tab view (the "All"
    /// tab and non-tab pickers). Only meaningful when the spec has tabs.
    pub tab: usize,
}

impl PickerItem {
    pub fn action(label: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: String::new(),
            badge: None,
            command: Some(command.into()),
            disabled: false,
            destructive: false,
            prefill: false,
            tab: 0,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        let badge = badge.into();
        self.destructive = badge.eq_ignore_ascii_case("DESTRUCTIVE");
        self.badge = Some(badge);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn prefill(mut self) -> Self {
        self.prefill = true;
        self.disabled = false;
        self
    }
    pub fn tab(mut self, tab: usize) -> Self {
        self.tab = tab;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerSpec {
    pub title: String,
    pub prompt: String,
    pub empty_message: String,
    pub items: Vec<PickerItem>,
    /// Optional category bar. `tabs[0]` is always the "show everything" entry;
    /// items with `tab == 0` belong to it, others to their 1-based category.
    /// Empty = no tab bar (the common case).
    pub tabs: Vec<String>,
    /// Resolved `ui.language` code; render-only chrome (footer, confirm title,
    /// text export) follows it. Defaults to English for pickers built without
    /// config in scope.
    pub lang: String,
}

impl PickerSpec {
    pub fn new(title: impl Into<String>, items: Vec<PickerItem>) -> Self {
        Self {
            title: title.into(),
            prompt: tr("en", "picker.search_placeholder").into(),
            empty_message: "No matching items".into(),
            items,
            tabs: Vec::new(),
            lang: "en".into(),
        }
    }

    /// Enable the category bar. `tabs[0]` should be the catch-all label
    /// ("All"); categories with no items are skipped in text export.
    pub fn with_tabs(mut self, tabs: Vec<String>) -> Self {
        self.tabs = tabs;
        self
    }

    /// Record the resolved chrome language and localize the spec-carried
    /// search placeholder, so interactive and text rendering follow
    /// `ui.language`.
    pub fn localized(mut self, lang: &str) -> Self {
        self.prompt = tr(lang, "picker.search_placeholder").into();
        self.lang = lang.to_string();
        self
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerState {
    pub spec: PickerSpec,
    pub query: String,
    pub selected: usize,
    /// Active category when the spec has tabs; 0 = the catch-all "All" view.
    pub active_tab: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerEvent {
    Pending,
    Cancelled,
    Submit(String),
    Prefill(String),
    Confirm {
        label: String,
        detail: String,
        command: String,
    },
}
impl PickerState {
    pub fn new(spec: PickerSpec) -> Self {
        Self {
            spec,
            query: String::new(),
            selected: INITIAL_SELECTION,
            active_tab: 0,
        }
    }

    pub fn filtered_indices(&self) -> Vec<usize> {
        let query = self.query.trim().to_ascii_lowercase();
        self.spec
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                // Tab browsing applies only while not searching; a query
                // always scans every category. Tab 0 is the catch-all view.
                if query.is_empty() && self.active_tab > 0 && item.tab != self.active_tab {
                    return false;
                }
                if query.is_empty() {
                    return true;
                }
                item.label.to_ascii_lowercase().contains(&query)
                    || item.detail.to_ascii_lowercase().contains(&query)
                    || item
                        .badge
                        .as_deref()
                        .map(|badge| badge.to_ascii_lowercase().contains(&query))
                        .unwrap_or(false)
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn clamp_selection(&mut self) {
        let count = self.filtered_indices().len();
        self.selected = self.selected.min(count.saturating_sub(SELECTION_STEP));
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PickerEvent {
        match key.code {
            KeyCode::Esc => PickerEvent::Cancelled,
            KeyCode::Backspace => {
                self.query.pop();
                self.clamp_selection();
                PickerEvent::Pending
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.clear();
                self.selected = INITIAL_SELECTION;
                PickerEvent::Pending
            }
            KeyCode::Tab if !self.spec.tabs.is_empty() => {
                self.active_tab = (self.active_tab + SELECTION_STEP) % self.spec.tabs.len();
                self.selected = INITIAL_SELECTION;
                PickerEvent::Pending
            }
            KeyCode::BackTab if !self.spec.tabs.is_empty() => {
                self.active_tab = (self.active_tab + self.spec.tabs.len() - SELECTION_STEP)
                    % self.spec.tabs.len();
                self.selected = INITIAL_SELECTION;
                PickerEvent::Pending
            }
            KeyCode::Home | KeyCode::PageUp => {
                self.selected = INITIAL_SELECTION;
                PickerEvent::Pending
            }
            KeyCode::End | KeyCode::PageDown => {
                self.selected = self.filtered_indices().len().saturating_sub(SELECTION_STEP);
                PickerEvent::Pending
            }
            KeyCode::Up => {
                let count = self.filtered_indices().len();
                if count > usize::MIN {
                    self.selected = if self.selected == INITIAL_SELECTION {
                        count - SELECTION_STEP
                    } else {
                        self.selected - SELECTION_STEP
                    };
                }
                PickerEvent::Pending
            }
            KeyCode::Down => {
                let count = self.filtered_indices().len();
                if count > usize::MIN {
                    self.selected = (self.selected + SELECTION_STEP) % count;
                }
                PickerEvent::Pending
            }
            KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n') => {
                let Some(index) = self.filtered_indices().get(self.selected).copied() else {
                    return PickerEvent::Pending;
                };
                let item = &self.spec.items[index];
                if item.disabled {
                    return PickerEvent::Pending;
                }
                let Some(command) = item.command.clone() else {
                    return PickerEvent::Pending;
                };
                if item.destructive {
                    return PickerEvent::Confirm {
                        label: item.label.clone(),
                        detail: item.detail.clone(),
                        command,
                    };
                }
                if item.prefill {
                    return PickerEvent::Prefill(command);
                }
                PickerEvent::Submit(command)
            }
            KeyCode::Char(ch)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.query.push(ch);
                self.selected = INITIAL_SELECTION;
                PickerEvent::Pending
            }
            _ => PickerEvent::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmState {
    pub label: String,
    pub detail: String,
    pub command: String,
    pub confirmed: bool,
    /// Chrome language inherited from the picker that raised the confirmation.
    pub lang: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmEvent {
    Pending,
    Cancelled,
    Submit(String),
}

impl ConfirmState {
    pub fn new(label: String, detail: String, command: String, lang: String) -> Self {
        Self {
            label,
            detail,
            command,
            confirmed: false,
            lang,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ConfirmEvent {
        match key.code {
            KeyCode::Esc => ConfirmEvent::Cancelled,
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                self.confirmed = !self.confirmed;
                ConfirmEvent::Pending
            }
            KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n') => {
                if self.confirmed {
                    ConfirmEvent::Submit(self.command.clone())
                } else {
                    ConfirmEvent::Cancelled
                }
            }
            _ => ConfirmEvent::Pending,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn tabbed_state() -> PickerState {
        let spec = PickerSpec::new(
            "test",
            vec![
                PickerItem::action("shared", "/shared"),
                PickerItem::action("alpha-1", "/a1").tab(1),
                PickerItem::action("alpha-2", "/a2").tab(1),
                PickerItem::action("beta-1", "/b1").tab(2),
            ],
        )
        .with_tabs(vec!["All".into(), "alpha".into(), "beta".into()]);
        PickerState::new(spec)
    }

    #[test]
    fn tab_zero_shows_everything() {
        let state = tabbed_state();
        assert_eq!(state.filtered_indices().len(), 4);
    }

    #[test]
    fn active_tab_filters_rows() {
        let mut state = tabbed_state();
        state.active_tab = 1;
        let labels: Vec<&str> = state
            .filtered_indices()
            .iter()
            .map(|index| state.spec.items[*index].label.as_str())
            .collect();
        assert_eq!(labels, ["alpha-1", "alpha-2"]);
    }

    #[test]
    fn query_searches_across_tabs() {
        let mut state = tabbed_state();
        state.active_tab = 1;
        state.query = "beta".into();
        let labels: Vec<&str> = state
            .filtered_indices()
            .iter()
            .map(|index| state.spec.items[*index].label.as_str())
            .collect();
        assert_eq!(labels, ["beta-1"]);
    }

    #[test]
    fn tab_key_cycles_categories() {
        let mut state = tabbed_state();
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        state.handle_key(tab);
        assert_eq!(state.active_tab, 1);
        state.handle_key(tab);
        assert_eq!(state.active_tab, 2);
        state.handle_key(tab);
        assert_eq!(state.active_tab, 0);
        state.handle_key(backtab);
        assert_eq!(state.active_tab, 2);
    }

    #[test]
    fn tab_key_is_ignored_without_tabs() {
        let mut state = PickerState::new(PickerSpec::new(
            "plain",
            vec![PickerItem::action("one", "/one")],
        ));
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.active_tab, 0);
    }

    #[test]
    fn text_export_groups_by_tab() {
        let outcome = CommandOutcome::Picker(tabbed_state().spec);
        let text = outcome.into_text();
        let shared = text.find("- shared").expect("shared row");
        let alpha = text.find("── alpha (2) ──").expect("alpha section");
        let beta = text.find("── beta (1) ──").expect("beta section");
        assert!(shared < alpha && alpha < beta, "ordering: {text}");
    }
}
