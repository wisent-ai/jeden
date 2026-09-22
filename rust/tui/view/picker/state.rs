//! A chooser while it is open: what narrows it, what is selected, and what
//! each key does to that.
//!
//! Split out of `tui/view/mod.rs`, which had grown past the module line cap.

use super::super::{PickerEvent, PickerFocus, PickerState, INITIAL_SELECTION, SELECTION_STEP};
use super::PickerSpec;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
