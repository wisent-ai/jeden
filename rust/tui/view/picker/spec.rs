//! What a chooser on screen is made of: the rows, and the description of the
//! chooser itself.
//!
//! Split out of `tui/view/mod.rs`, which had grown past the module line cap.

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
