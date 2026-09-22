//! The interactive views a command can put on screen: a chooser, and the
//! confirmation in front of anything destructive.

const INITIAL_SELECTION: usize = usize::MIN;
const SELECTION_STEP: usize = true as usize;

mod confirm;
mod outcome;
mod picker;

pub use confirm::{ConfirmEvent, ConfirmState};
pub use outcome::CommandOutcome;
pub use picker::{PickerItem, PickerSpec};

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
