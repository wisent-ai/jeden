//! What the editor remembers: the lines already submitted, and the states to
//! step back to.
//!
//! Split out of `tui/editor/mod.rs`, which had grown past the module line cap.

use super::super::{Edit, EditorState, Region, Snapshot};

impl EditorState {
    pub fn push_history(&mut self, value: String) {
        if value.is_empty() || self.history.last() == Some(&value) {
            return;
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

    /// Replace the whole buffer as one undoable change; false when the text
    /// is already what the buffer holds.
    pub fn replace_all_transaction(&mut self, text: String) -> bool {
        if text == self.text {
            return false;
        }
        self.replace_whole(text);
        self.history_index = None;
        true
    }

    pub(in crate::tui::editor) fn record(&mut self, edit: Edit) {
        self.undo.push(edit);
        self.redo.clear();
    }

    pub(crate) fn undo(&mut self) {
        let Some(edit) = self.undo.pop() else {
            return;
        };
        self.put_back(edit.region, &edit.inserted, &edit.removed, edit.before);
        self.redo.push(edit);
    }

    pub(crate) fn redo(&mut self) {
        let Some(edit) = self.redo.pop() else {
            return;
        };
        self.put_back(edit.region, &edit.removed, &edit.inserted, edit.after);
        self.undo.push(edit);
    }

    /// Swap `present`, which `region` holds now, for `wanted`, and put the
    /// cursor and anchor where they stood with `wanted` in place.
    fn put_back(&mut self, region: Region, present: &str, wanted: &str, place: (usize, Option<usize>)) {
        match region {
            Region::Whole => wanted.clone_into(&mut self.text),
            Region::At(start) => self.text.replace_range(start..start + present.len(), wanted),
        }
        (self.cursor, self.anchor) = place;
        self.preferred_column = None;
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
