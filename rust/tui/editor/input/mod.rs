//! How the prompt editor takes input: the key bindings, movement through
//! text, and what it remembers.

mod cursor;
mod history;
mod keymap;

pub(super) use cursor::{
    byte_at_display_column, line_end, line_start, next_boundary, normalize_paste, ordered,
    previous_boundary, word_left, word_right,
};
pub use keymap::{ActionKeyMap, EditorAction, KeyBinding};
