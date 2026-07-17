use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerSpec {
    pub title: String,
    pub prompt: String,
    pub empty_message: String,
    pub items: Vec<PickerItem>,
}

impl PickerSpec {
    pub fn new(title: impl Into<String>, items: Vec<PickerItem>) -> Self {
        Self {
            title: title.into(),
            prompt: "Type to search".into(),
            empty_message: "No matching items".into(),
            items,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Text(String),
    Picker(PickerSpec),
}

impl CommandOutcome {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub fn into_text(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Picker(spec) => {
                let mut lines = vec![spec.title];
                for item in spec.items {
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
                    lines.push(format!("- {}{}{}", item.label, badge, detail));
                }
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
        }
    }

    pub fn filtered_indices(&self) -> Vec<usize> {
        let query = self.query.trim().to_ascii_lowercase();
        self.spec
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmEvent {
    Pending,
    Cancelled,
    Submit(String),
}

impl ConfirmState {
    pub fn new(label: String, detail: String, command: String) -> Self {
        Self {
            label,
            detail,
            command,
            confirmed: false,
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
