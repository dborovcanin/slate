use crate::reminders::*;
use editor_core::history::policy::UndoPolicy;
use editor_core::history::LineHistory;
use rustc_hash::FxHashMap;
pub struct NoteSession {
    pub history: LineHistory<ReminderMarks>,
    pub undo_policy: UndoPolicy<ReminderUndoEntry>,
    pub dirty: bool,
    pub reminder_ghosts: FxHashMap<usize, LineReminderGhost>,
    pub pending_line_edits: Vec<PendingLineChange>,
    pub reminders_generation: u64,
    pub persisted_reminders_generation: u64,
    pub edit_seq: u64,
}
impl NoteSession {
    pub fn new(
        history: LineHistory<ReminderMarks>,
        reminder_ghosts: FxHashMap<usize, LineReminderGhost>,
    ) -> Self {
        Self {
            history,
            reminder_ghosts,
            undo_policy: UndoPolicy::default(),
            dirty: false,
            pending_line_edits: Vec::new(),
            reminders_generation: 0,
            persisted_reminders_generation: 0,
            edit_seq: 0,
        }
    }
    pub fn edit_seq(&self) -> u64 {
        self.edit_seq
    }

    pub fn note_changed(&mut self) {
        self.edit_seq = self.edit_seq.wrapping_add(1);
    }
}
