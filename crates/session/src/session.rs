use crate::reminders::*;
use editor_core::history::policy::UndoPolicy;
use editor_core::history::LineHistory;
use rustc_hash::FxHashMap;
/// State and policy for one open note. Fields are crate-private: front ends
/// read through accessors and change state only through session methods, so
/// edit, history, reminder, revision and access rules have a single owner.
/// `calc` and `folds` stay public while front ends still drive viewport
/// evaluation and fold rescans; see "Follow-ups" in `roadmap/arch-refactor.md`.
pub struct NoteSession {
    pub(crate) stored_revision: String,
    pub(crate) access_mode: app_core::storage::NoteAccessMode,
    pub(crate) is_unlocked: bool,
    pub(crate) leave_refused_at: Option<u64>,
    pub(crate) autosave_paused_at: Option<u64>,
    pub(crate) outside_change_reported: Option<String>,
    pub folds: crate::folds::FoldStructure,
    pub(crate) session_id: u64,
    pub(crate) note_id: String,
    pub calc: crate::calc::CalcState,
    pub(crate) history: LineHistory<ReminderMarks>,
    pub(crate) undo_policy: UndoPolicy<ReminderUndoEntry>,
    pub(crate) dirty: bool,
    pub(crate) reminder_ghosts: FxHashMap<usize, LineReminderGhost>,
    pub(crate) pending_line_edits: Vec<PendingLineChange>,
    pub(crate) reminders_generation: u64,
    pub(crate) persisted_reminders_generation: u64,
    pub(crate) edit_seq: u64,
}
impl NoteSession {
    pub fn new(
        history: LineHistory<ReminderMarks>,
        reminder_ghosts: FxHashMap<usize, LineReminderGhost>,
        calc: crate::calc::CalcState,
    ) -> Self {
        Self {
            stored_revision: String::new(),
            access_mode: Default::default(),
            is_unlocked: true,
            leave_refused_at: None,
            autosave_paused_at: None,
            outside_change_reported: None,
            folds: crate::folds::FoldStructure::default(),
            session_id: 0,
            note_id: String::new(),
            calc,
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

    pub(crate) fn note_changed(&mut self) {
        self.edit_seq = self.edit_seq.wrapping_add(1);
    }

    pub fn stored_revision(&self) -> &str {
        &self.stored_revision
    }
    pub fn access_mode(&self) -> app_core::storage::NoteAccessMode {
        self.access_mode
    }
    pub fn is_unlocked(&self) -> bool {
        self.is_unlocked
    }
    pub fn note_id(&self) -> &str {
        &self.note_id
    }
    pub fn session_id(&self) -> u64 {
        self.session_id
    }
    pub fn dirty(&self) -> bool {
        self.dirty
    }
    pub fn reminders(&self) -> &FxHashMap<usize, LineReminderGhost> {
        &self.reminder_ghosts
    }
    pub fn pending_line_edits(&self) -> &[PendingLineChange] {
        &self.pending_line_edits
    }
    pub fn history(&self) -> &LineHistory<ReminderMarks> {
        &self.history
    }
    pub fn undo_depth(&self) -> usize {
        self.undo_policy.undo_depth()
    }
    pub fn redo_depth(&self) -> usize {
        self.history.redo_depth()
    }

    /// Normal/visual commands begin a new undo step.
    pub fn begin_input(&mut self, session: editor_core::history::policy::UndoSession) {
        self.history.begin_input(session);
    }
    /// The next edit starts its own undo step.
    pub fn break_undo_coalescing(&mut self) {
        self.history.break_coalescing();
    }
    /// The current text becomes the undo baseline, e.g. after opening a note.
    pub fn checkpoint_history(&mut self, doc: &crate::Document) {
        self.history
            .checkpoint(doc.lines(), doc.cursor_line, doc.cursor_col);
    }
    /// Release undo memory after switching away from a large note.
    pub fn compact_history(&mut self) {
        self.history.compact();
    }

    /// Whole lines `start..=end` are about to be deleted by a linewise command:
    /// their reminders go with them, even where an empty line remains.
    pub fn drop_reminders_on_lines(&mut self, start: usize, end: usize) {
        let before = self.reminder_ghosts.len();
        self.reminder_ghosts
            .retain(|line, _| *line < start || *line > end);
        if self.reminder_ghosts.len() != before {
            self.reminders_generation = self.reminders_generation.wrapping_add(1);
        }
    }

    #[cfg(feature = "test-support")]
    pub fn note_changed_for_tests(&mut self) {
        self.note_changed();
    }
    #[cfg(feature = "test-support")]
    pub fn set_dirty_for_tests(&mut self, dirty: bool) {
        self.dirty = dirty;
    }
    #[cfg(feature = "test-support")]
    pub fn history_mut_for_tests(&mut self) -> &mut LineHistory<ReminderMarks> {
        &mut self.history
    }
    #[cfg(feature = "test-support")]
    pub fn reminders_mut_for_tests(&mut self) -> &mut FxHashMap<usize, LineReminderGhost> {
        &mut self.reminder_ghosts
    }
}
