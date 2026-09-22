//! The drawn panels: the bordered box everything else is built from, the
//! welcome screen, and the slash suggestion list.

pub(crate) mod boxes;
mod slash_hints;
mod welcome;

pub(crate) use boxes::{boxed, boxed_split};
pub(crate) use boxes::{framed_header, input_prefix_width};
pub(crate) use slash_hints::{complete_slash_input, slash_hint_panel, slash_matches};
pub(crate) use welcome::welcome_panel;
