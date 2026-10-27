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

    /// Show the line submitted before the one shown, starting from the latest;
    /// the first line stays shown once reached.
    pub fn history_previous(&mut self) {
        let step = usize::from(crate::tally::STEP);
        let index = match self.history_index {
            Some(shown) => shown.saturating_sub(step),
            None => {
                let Some(latest) = self.history.len().checked_sub(step) else {
                    return;
                };
                self.history_draft = Some(self.snapshot());
                latest
            }
        };
        self.load_history(index);
    }

    /// Show the line submitted after the one shown; past the latest, the
    /// draft that was being written comes back.
    pub fn history_next(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        let next = index + usize::from(crate::tally::STEP);
        if next < self.history.len() {
            self.load_history(next);
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
    fn put_back(
        &mut self,
        region: Region,
        present: &str,
        wanted: &str,
        place: (usize, Option<usize>),
    ) {
        match region {
            Region::Whole => wanted.clone_into(&mut self.text),
            Region::At(start) => self
                .text
                .replace_range(start..start + present.len(), wanted),
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
