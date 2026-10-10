//! Text history restoration; presentation clamps and semantic action ordering stay with the host during extraction.
use crate::{Document, EditOutcome, NoteSession};
use editor_core::buffer::EditDelta;

impl NoteSession {
    pub fn undo(&mut self, doc: &mut Document) -> Option<EditOutcome> {
        let keep_cursor = self.history.undo_depth() == 1;
        self.restore_text(doc, false, keep_cursor)
    }

    pub fn redo(&mut self, doc: &mut Document) -> Option<EditOutcome> {
        self.restore_text(doc, true, false)
    }

    fn restore_text(
        &mut self,
        doc: &mut Document,
        redo: bool,
        keep_cursor: bool,
    ) -> Option<EditOutcome> {
        let old_span = doc.lines.len();
        let old_cursor = doc.cursor();
        let cursor = if redo {
            self.history.redo(&mut doc.lines)?
        } else {
            self.history.undo(&mut doc.lines)?
        };
        doc.cursor_line = if keep_cursor {
            old_cursor.line
        } else {
            cursor.line
        }
        .min(doc.lines.len().saturating_sub(1));
        doc.cursor_col = if keep_cursor {
            old_cursor.column
        } else {
            cursor.col
        };
        doc.text_generation = doc.text_generation.wrapping_add(1);
        doc.joined_text_cache = None;
        self.dirty = true;
        self.note_changed();
        self.pending_line_edits.clear();
        let marks = self.history.current_marks().clone();
        if *marks != *crate::reminders::reminder_marks_of(&self.reminder_ghosts) {
            self.reminder_ghosts = marks.iter().cloned().collect();
            self.reminders_generation = self.reminders_generation.wrapping_add(1);
        }
        Some(EditOutcome {
            delta: EditDelta {
                start_line: 0,
                old_span,
                new_span: doc.lines.len(),
            },
            first_changed_line: 0,
            title_changed: true,
            calc_splices: Vec::new(),
            fold_rescan: true,
            register: None,
            text_changed: true,
            calc_effect: Default::default(),
        })
    }
}
