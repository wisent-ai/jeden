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
    /// Compact figures rendered right-aligned at the end of the row (perf,
    /// context, price). Kept apart from `detail` so the columns line up
    /// instead of drifting with the description length.
    pub metrics: String,
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
            metrics: String::new(),
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
    pub fn metrics(mut self, metrics: impl Into<String>) -> Self {
        self.metrics = metrics.into();
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
    /// Per-tab reachability, parallel to `tabs`. A marked category is one the
    /// user can actually use right now (a subscription they hold); unmarked
    /// ones are visible but out of reach (the public catalog). The brands
    /// pane draws them as ● and ○ and rules a line between the two groups.
    pub tab_marks: Vec<bool>,
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
            tab_marks: Vec::new(),
            lang: "en".into(),
        }
    }

    /// Enable the category bar. `tabs[0]` should be the catch-all label
    /// ("All"); categories with no items are skipped in text export.
    pub fn with_tabs(mut self, tabs: Vec<String>) -> Self {
        self.tabs = tabs;
        self
    }

    /// Mark which categories are reachable; see `PickerSpec::tab_marks`.
    pub fn with_tab_marks(mut self, marks: Vec<bool>) -> Self {
        self.tab_marks = marks;
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

/// Which pane the arrow keys drive in a two-pane picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerFocus {
    /// Left pane: ↑↓ walk the brands, → steps into their items.
    Categories,
    /// Right pane: ↑↓ walk the items, ← steps back to the brands.
    Items,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerState {
    pub spec: PickerSpec,
    pub query: String,
    pub selected: usize,
    /// Active category when the spec has tabs; 0 = the catch-all "All" view.
    pub active_tab: usize,
    /// Focused pane. A picker without categories is always on its items.
    pub focus: PickerFocus,
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
        let focus = if spec.tabs.is_empty() {
            PickerFocus::Items
        } else {
            // Two-pane pickers open on the brands column, the way omp does:
            // you pick the provider first, then step right into its models.
            PickerFocus::Categories
        };
        let mut state = Self {
            spec,
            query: String::new(),
            selected: INITIAL_SELECTION,
            active_tab: 0,
            focus,
        };
        state.select_first_enabled();
        state
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

    /// Move `position` in `direction` until it lands on an enabled item
    /// (summary rows and section headers are disabled dead-ends; navigation
    /// should skip them, not park on them). Falls back to the clamped input
    /// when every row is disabled.
    fn skip_disabled(&self, position: usize, direction: isize) -> usize {
        let indices = self.filtered_indices();
        if indices.is_empty() {
            return usize::MIN;
        }
        if indices.iter().all(|index| self.spec.items[*index].disabled) {
            return position.min(indices.len() - SELECTION_STEP);
        }
        let mut current = position.min(indices.len() - SELECTION_STEP);
        for _ in usize::MIN..indices.len() {
            if !self.spec.items[indices[current]].disabled {
                return current;
            }
            current = (current as isize + direction).rem_euclid(indices.len() as isize) as usize;
        }
        current
    }

    fn select_first_enabled(&mut self) {
        self.selected = self.skip_disabled(INITIAL_SELECTION, SELECTION_STEP as isize);
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
                self.select_first_enabled();
                PickerEvent::Pending
            }
            KeyCode::Tab if !self.spec.tabs.is_empty() => {
                self.active_tab = (self.active_tab + SELECTION_STEP) % self.spec.tabs.len();
                self.select_first_enabled();
                PickerEvent::Pending
            }
            KeyCode::BackTab if !self.spec.tabs.is_empty() => {
                self.active_tab = (self.active_tab + self.spec.tabs.len() - SELECTION_STEP)
                    % self.spec.tabs.len();
                self.select_first_enabled();
                PickerEvent::Pending
            }
            // →/← cross between the panes; a picker with no brands column
            // ignores them, exactly as it ignores Tab.
            KeyCode::Right if !self.spec.tabs.is_empty() => {
                self.focus = PickerFocus::Items;
                PickerEvent::Pending
            }
            KeyCode::Left if !self.spec.tabs.is_empty() => {
                self.focus = PickerFocus::Categories;
                PickerEvent::Pending
            }
            KeyCode::Home | KeyCode::PageUp => {
                self.select_first_enabled();
                PickerEvent::Pending
            }
            KeyCode::End | KeyCode::PageDown => {
                let last = self.filtered_indices().len().saturating_sub(SELECTION_STEP);
                self.selected = self.skip_disabled(last, -(SELECTION_STEP as isize));
                PickerEvent::Pending
            }
            // On the brands pane the arrows walk categories; on the items
            // pane they walk items. The footer promises exactly this.
            KeyCode::Up if self.focus == PickerFocus::Categories => {
                self.active_tab = (self.active_tab + self.spec.tabs.len() - SELECTION_STEP)
                    % self.spec.tabs.len();
                self.select_first_enabled();
                PickerEvent::Pending
            }
            KeyCode::Down if self.focus == PickerFocus::Categories => {
                self.active_tab = (self.active_tab + SELECTION_STEP) % self.spec.tabs.len();
                self.select_first_enabled();
                PickerEvent::Pending
            }
            KeyCode::Up => {
                let count = self.filtered_indices().len();
                if count > usize::MIN {
                    let previous = if self.selected == INITIAL_SELECTION {
                        count - SELECTION_STEP
                    } else {
                        self.selected - SELECTION_STEP
                    };
                    self.selected = self.skip_disabled(previous, -(SELECTION_STEP as isize));
                }
                PickerEvent::Pending
            }
            KeyCode::Down => {
                let count = self.filtered_indices().len();
                if count > usize::MIN {
                    let next = (self.selected + SELECTION_STEP) % count;
                    self.selected = self.skip_disabled(next, SELECTION_STEP as isize);
                }
                PickerEvent::Pending
            }
            KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n') => {
                // On the brands pane Enter steps right, like →: there is
                // nothing to submit in a category, only models behind it.
                if self.focus == PickerFocus::Categories {
                    self.focus = PickerFocus::Items;
                    return PickerEvent::Pending;
                }
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
                // Searching is an item operation: a query spans every
                // category, so the arrows must land on results, not brands.
                self.focus = PickerFocus::Items;
                self.select_first_enabled();
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

    fn disabled_heavy_state() -> PickerState {
        PickerState::new(PickerSpec::new(
            "test",
            vec![
                PickerItem::action("header", "").disabled(true),
                PickerItem::action("first", "/first"),
                PickerItem::action("middle", "").disabled(true),
                PickerItem::action("last", "/last"),
            ],
        ))
    }

    #[test]
    fn initial_selection_skips_disabled_rows() {
        let state = disabled_heavy_state();
        assert_eq!(state.selected, 1, "first enabled row should be selected");
    }

    #[test]
    fn navigation_skips_disabled_rows() {
        let mut state = disabled_heavy_state();
        state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.selected, 3, "down from 1 skips the disabled row at 2");
        state.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(state.selected, 1);
        state.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(state.selected, 1);
        state.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(state.selected, 3);
    }
}
