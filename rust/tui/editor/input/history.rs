//! What the editor remembers: the lines already submitted, and the states to
//! step back to.
//!
//! Split out of `tui/editor/mod.rs`, which had grown past the module line cap.

use super::super::{EditorState, Snapshot, MAX_HISTORY_ITEMS, MAX_UNDO_STEPS};
use crate::tui::editor::EditorLimitError;
use crate::tui::editor::MAX_BUFFER_BYTES;

impl EditorState {
    pub fn push_history(&mut self, value: String) {
        if value.is_empty() || self.history.last() == Some(&value) {
            return;
        }
        if self.history.len() == MAX_HISTORY_ITEMS {
            self.history.remove(0);
        }
        self.history.push(value);
        self.history_index = None;
        self.history_draft = None;
    }

    pub fn history_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_index.is_none() {
            self.history_draft = Some(self.snapshot());
        }
        let index = self
            .history_index
            .map_or(self.history.len() - 1, |i| i.saturating_sub(1));
        self.load_history(index);
    }

    pub fn history_next(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            self.load_history(index + 1);
        } else if let Some(draft) = self.history_draft.take() {
            self.restore(draft);
            self.history_index = None;
        }
    }

    pub fn replace_all_transaction(&mut self, text: String) -> Result<bool, EditorLimitError> {
        if text.len() > MAX_BUFFER_BYTES {
            let error = EditorLimitError {
                limit_bytes: MAX_BUFFER_BYTES,
            };
            self.last_error = Some(error);
            return Err(error);
        }
        if text == self.text {
            return Ok(false);
        }
        self.record_undo();
        self.text = text;
        self.cursor = self.text.len();
        self.anchor = None;
        self.preferred_column = None;
        self.history_index = None;
        Ok(true)
    }

    pub(crate) fn record_undo(&mut self) {
        if self.undo.len() == MAX_UNDO_STEPS {
            self.undo.remove(0);
        }
        self.undo.push(self.snapshot());
        self.redo.clear();
    }

    pub(crate) fn undo(&mut self) {
        let Some(snapshot) = self.undo.pop() else {
            return;
        };
        if self.redo.len() == MAX_UNDO_STEPS {
            self.redo.remove(0);
        }
        self.redo.push(self.snapshot());
        self.restore(snapshot);
    }

    pub(crate) fn redo(&mut self) {
        let Some(snapshot) = self.redo.pop() else {
            return;
        };
        if self.undo.len() == MAX_UNDO_STEPS {
            self.undo.remove(0);
        }
        self.undo.push(self.snapshot());
        self.restore(snapshot);
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.text = snapshot.text;
        self.cursor = snapshot.cursor;
        self.anchor = snapshot.anchor;
        self.preferred_column = None;
    }

    fn load_history(&mut self, index: usize) {
        self.text.clone_from(&self.history[index]);
        self.cursor = self.text.len();
        self.anchor = None;
        self.preferred_column = None;
        self.history_index = Some(index);
    }
}
