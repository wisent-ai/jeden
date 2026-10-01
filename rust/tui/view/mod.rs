//! The interactive views a command can put on screen: a chooser, and the
//! confirmation in front of anything destructive.

const INITIAL_SELECTION: usize = usize::MIN;
const SELECTION_STEP: usize = true as usize;

mod confirm;
mod outcome;
mod picker;
// d183a11 moved view_render.rs here as render.rs without declaring it; tui
// re-exports it as `view_render`.
pub(crate) mod render;

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



