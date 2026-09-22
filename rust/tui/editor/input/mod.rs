//! How the prompt editor takes input: the key bindings, movement through
//! text, and what it remembers.

mod cursor;
mod history;
mod keymap;

pub(crate) use cursor::{
    byte_at_display_column, line_end, line_start, next_boundary, normalize_paste, ordered,
    previous_boundary,
};
pub use keymap::{ActionKeyMap, EditorAction};
