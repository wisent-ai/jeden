//! The prompt editor: the buffer a person types into, and what it will accept.

use crossterm::event::KeyEvent;

pub const EDITOR_KEYMAP_NAMESPACE: &str = "editor";
pub const EXTERNAL_EDITOR_ACTION_ID: &str = "editor.external";

// d183a11 moved attachments.rs and text.rs from tui/ into editor/ without
// declaring them here, so the compiler never read them; tui/mod.rs
// re-exports both for the rest of the crate.
pub(crate) mod attachments;
mod input;
pub(crate) mod text;

use input::{
    byte_at_display_column, line_end, line_start, next_boundary, normalize_paste, ordered,
    previous_boundary,
};
pub use input::{ActionKeyMap, EditorAction};
use unicode_width::UnicodeWidthStr;

/// The buffer, cursor and selection anchor as they stood, kept for the draft
/// a history walk returns to.
#[derive(Debug, Clone)]
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
}

/// Where a change landed: the whole buffer, or the bytes from an offset.
#[derive(Debug, Clone, Copy)]
enum Region {
    Whole,
    At(usize),
}

/// One change to the buffer and how to take it back: what was `removed`
/// from `region` and what was `inserted` there, with the cursor and anchor
/// before and after. Undo keeps changes, not copies of the buffer, so its
/// memory grows with what was typed rather than with the buffer's length,
/// and every step stays reachable.
#[derive(Debug, Clone)]
struct Edit {
    region: Region,
    removed: String,
    inserted: String,
    before: (usize, Option<usize>),
    after: (usize, Option<usize>),
}

#[derive(Debug, Clone)]
pub struct EditorState {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    preferred_column: Option<usize>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<Snapshot>,
    keymap: ActionKeyMap,
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new(ActionKeyMap::default())
    }
}

impl EditorState {
    pub fn new(keymap: ActionKeyMap) -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            anchor: None,
            preferred_column: None,
            undo: Vec::new(),
            redo: Vec::new(),
            history: Vec::new(),
            history_index: None,
            history_draft: None,
            keymap,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    pub fn action_for(&self, event: KeyEvent) -> Option<EditorAction> {
        self.keymap.action_for(event)
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.text = text;
        self.cursor = self.text.len();
        self.anchor = None;
        self.preferred_column = None;
        self.history_index = None;
    }

    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.anchor = None;
        self.preferred_column = None;
        self.history_index = None;
        self.history_draft = None;
        self.undo.clear();
        self.redo.clear();
        text
    }

    pub fn clear(&mut self) {
        if !self.text.is_empty() {
            self.replace_whole(String::new());
        }
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            None
        } else {
            Some(ordered(anchor, self.cursor))
        }
    }

    pub fn insert(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        self.replace_selection(value);
    }

    pub fn paste(&mut self, value: &str) {
        let normalized = normalize_paste(value);
        if normalized.is_empty() {
            return;
        }
        self.replace_selection(&normalized);
    }

    pub fn delete_backward(&mut self) {
        if self.selection().is_some() {
            self.replace_selection("");
            return;
        }
        let start = previous_boundary(&self.text, self.cursor);
        if start != self.cursor {
            self.splice(start, self.cursor, "", start);
        }
    }

    pub fn delete_forward(&mut self) {
        if self.selection().is_some() {
            self.replace_selection("");
            return;
        }
        let end = next_boundary(&self.text, self.cursor);
        if end != self.cursor {
            self.splice(self.cursor, end, "", self.cursor);
        }
    }

    /// Replace `start..end` with `value`, leave the cursor at `cursor` with no
    /// selection, and record the change so undo can take it back.
    fn splice(&mut self, start: usize, end: usize, value: &str, cursor: usize) {
        let before = (self.cursor, self.anchor);
        let removed = self.text[start..end].to_string();
        self.text.replace_range(start..end, value);
        self.cursor = cursor;
        self.anchor = None;
        self.preferred_column = None;
        self.record(Edit {
            region: Region::At(start),
            removed,
            inserted: value.to_string(),
            before,
            after: (cursor, None),
        });
    }

    /// Replace the whole buffer with `value`, cursor at its end, recorded so
    /// undo can take it back.
    fn replace_whole(&mut self, value: String) {
        let before = (self.cursor, self.anchor);
        let removed = std::mem::replace(&mut self.text, value);
        self.cursor = self.text.len();
        self.anchor = None;
        self.preferred_column = None;
        self.record(Edit {
            region: Region::Whole,
            removed,
            inserted: self.text.clone(),
            before,
            after: (self.cursor, None),
        });
    }

    fn replace_selection(&mut self, value: &str) {
        let (start, end) = self.selection().unwrap_or((self.cursor, self.cursor));
        self.splice(start, end, value, start + value.len());
        self.history_index = None;
    }

    fn move_to(&mut self, cursor: usize, selecting: bool) {
        if selecting {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = cursor;
        self.preferred_column = None;
    }

    fn move_vertical(&mut self, direction: isize) {
        let start = line_start(&self.text, self.cursor);
        let column = self
            .preferred_column
            .unwrap_or_else(|| UnicodeWidthStr::width(&self.text[start..self.cursor]));
        let target_start = if direction < 0 {
            if start == 0 {
                return;
            }
            line_start(&self.text, start.saturating_sub(1))
        } else {
            let end = line_end(&self.text, self.cursor);
            if end == self.text.len() {
                return;
            }
            end + 1
        };
        let target_end = line_end(&self.text, target_start);
        self.cursor = byte_at_display_column(&self.text, target_start, target_end, column);
        self.anchor = None;
        self.preferred_column = Some(column);
    }
}
