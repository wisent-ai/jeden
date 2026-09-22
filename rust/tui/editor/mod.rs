//! The prompt editor: the buffer a person types into, and what it will accept.

use crossterm::event::KeyEvent;

const MAX_UNDO_STEPS: usize = 64;
const MAX_HISTORY_ITEMS: usize = 100;
const MAX_BUFFER_BYTES: usize = 1024 * 1024;

pub const EDITOR_KEYMAP_NAMESPACE: &str = "editor";
pub const EXTERNAL_EDITOR_ACTION_ID: &str = "editor.external";

mod input;

use input::{
    byte_at_display_column, line_end, line_start, next_boundary, normalize_paste, ordered,
    previous_boundary, word_left, word_right,
};
pub use input::{ActionKeyMap, EditorAction, KeyBinding};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorLimitError {
    pub limit_bytes: usize,
}

impl std::fmt::Display for EditorLimitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Editor input limit exceeded ({} bytes)",
            self.limit_bytes
        )
    }
}

impl std::error::Error for EditorLimitError {}

#[derive(Debug, Clone)]
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct EditorState {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    preferred_column: Option<usize>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<Snapshot>,
    keymap: ActionKeyMap,
    last_error: Option<EditorLimitError>,
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
            last_error: None,
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
    pub fn take_error(&mut self) -> Option<EditorLimitError> {
        self.last_error.take()
    }
    pub fn action_for(&self, event: KeyEvent) -> Option<EditorAction> {
        self.keymap.action_for(event)
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        if text.len() > MAX_BUFFER_BYTES {
            self.last_error = Some(EditorLimitError {
                limit_bytes: MAX_BUFFER_BYTES,
            });
            return;
        }
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
            self.record_undo();
            self.text.clear();
            self.cursor = 0;
            self.anchor = None;
            self.preferred_column = None;
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
        if !self.can_replace_selection_with(value.len()) {
            self.last_error = Some(EditorLimitError {
                limit_bytes: MAX_BUFFER_BYTES,
            });
            return;
        }
        self.record_undo();
        self.replace_selection(value);
    }

    pub fn paste(&mut self, value: &str) {
        if !self.can_replace_selection_with(value.len()) {
            self.last_error = Some(EditorLimitError {
                limit_bytes: MAX_BUFFER_BYTES,
            });
            return;
        }
        let normalized = normalize_paste(value);
        if normalized.is_empty() {
            return;
        }
        self.record_undo();
        self.replace_selection(&normalized);
    }

    pub fn delete_backward(&mut self) {
        if self.selection().is_some() {
            self.record_undo();
            self.replace_selection("");
            return;
        }
        let start = previous_boundary(&self.text, self.cursor);
        if start != self.cursor {
            self.record_undo();
            self.text.replace_range(start..self.cursor, "");
            self.cursor = start;
            self.preferred_column = None;
        }
    }

    pub fn delete_forward(&mut self) {
        if self.selection().is_some() {
            self.record_undo();
            self.replace_selection("");
            return;
        }
        let end = next_boundary(&self.text, self.cursor);
        if end != self.cursor {
            self.record_undo();
            self.text.replace_range(self.cursor..end, "");
            self.preferred_column = None;
        }
    }

    fn can_replace_selection_with(&self, bytes: usize) -> bool {
        let removed = self.selection().map_or(0, |(start, end)| end - start);
        self.text
            .len()
            .saturating_sub(removed)
            .saturating_add(bytes)
            <= MAX_BUFFER_BYTES
    }

    fn replace_selection(&mut self, value: &str) {
        let (start, end) = self.selection().unwrap_or((self.cursor, self.cursor));
        self.text.replace_range(start..end, value);
        self.cursor = start + value.len();
        self.anchor = None;
        self.preferred_column = None;
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
