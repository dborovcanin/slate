use crate::reminders::*;
use editor_core::history::policy::UndoPolicy;
use editor_core::history::LineHistory;
use rustc_hash::FxHashMap;
/// State and policy for one open note. Fields are crate-private: front ends
/// read through accessors and change state only through session methods, so
/// edit, history, reminder, revision and access rules have a single owner.
pub struct NoteSession {
    pub(crate) stored_revision: String,
    pub(crate) access_mode: app_core::storage::NoteAccessMode,
    pub(crate) is_unlocked: bool,
    pub(crate) leave_refused_at: Option<u64>,
    pub(crate) autosave_paused_at: Option<u64>,
    pub(crate) outside_change_reported: Option<String>,
    pub(crate) folds: crate::folds::FoldStructure,
    pub(crate) session_id: u64,
    pub(crate) note_id: String,
    pub(crate) calc: crate::calc::CalcState,
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

    /// Calc results, line metadata and signals, read by renderers and popups.
    pub fn calc(&self) -> &crate::calc::CalcState {
        &self.calc
    }
    /// Fold ranges and structure the front end maps into its view.
    pub fn folds(&self) -> &crate::folds::FoldStructure {
        &self.folds
    }

    pub fn bootstrap_folds(&mut self, line_count: usize) {
        self.folds.bootstrap(line_count);
    }
    /// Keep fold structure current after an edit the front end applied itself.
    pub fn fold_upkeep(
        &mut self,
        doc: &crate::Document,
        delta: Option<editor_core::buffer::EditDelta>,
        reduced_features: bool,
        has_collapsed: bool,
        view_line_count: usize,
    ) -> crate::folds::FoldEffect {
        self.folds
            .upkeep(doc, delta, reduced_features, has_collapsed, view_line_count)
    }
    pub fn recompute_folds(&mut self, doc: &crate::Document) -> crate::folds::FoldEffect {
        self.folds.recompute(doc)
    }
    /// Fold commands need a full analysis of the current text first.
    pub fn fold_analysis_stale(&self, doc: &crate::Document) -> bool {
        let len = doc.lines().len();
        !self.folds.analysis_ready
            || self.folds.rescan_pending
            || self.folds.range_by_start.len() != len
            || self.folds.line_has_structure.len() != len
            || self.folds.line_text_snapshot.len() != len
    }
    /// A deferred fold rescan is due; the caller runs it now.
    pub fn take_fold_rescan(&mut self) -> bool {
        std::mem::take(&mut self.folds.rescan_pending)
    }

    pub fn rebuild_calc_metadata(
        &mut self,
        doc: &crate::Document,
        inputs: crate::calc::CalcInputs,
    ) {
        self.calc.rebuild_calc_line_metadata(doc, inputs);
    }
    /// Returns whether the dependency index now needs an idle sync.
    pub fn refresh_calc_metadata_at(
        &mut self,
        doc: &crate::Document,
        inputs: crate::calc::CalcInputs,
        line: usize,
    ) -> bool {
        self.calc.refresh_calc_line_metadata_at(doc, inputs, line)
    }
    /// Returns whether the dependency index now needs an idle sync.
    pub fn splice_calc_metadata(
        &mut self,
        doc: &crate::Document,
        inputs: crate::calc::CalcInputs,
        start_line: usize,
        old_span: usize,
        new_span: usize,
    ) -> bool {
        self.calc
            .splice_calc_line_metadata(doc, inputs, start_line, old_span, new_span)
    }
    /// Bring the dependency index up to date while idle; true when names changed.
    pub fn sync_calc_index_after_idle(
        &mut self,
        doc: &crate::Document,
        mask: editor_core::calc_plan::CalcFeatureMask,
        viewport_only: bool,
    ) -> bool {
        self.calc.sync_index_after_idle(doc, mask, viewport_only)
    }
    /// Release capacity held for a large note after switching away from it.
    pub fn compact_derived_state(&mut self) {
        self.calc.results.shrink_to_fit();
        self.calc.cell_results.shrink_to_fit();
        self.calc.variable_names.shrink_to_fit();
        self.calc.line_metadata.shrink_to_fit();
        self.calc.prev_line_metadata.shrink_to_fit();
        self.folds.line_has_structure.shrink_to_fit();
        self.folds.line_text_snapshot.shrink_to_fit();
        self.folds.range_by_start.shrink_to_fit();
    }

    #[cfg(feature = "test-support")]
    pub fn calc_mut_for_tests(&mut self) -> &mut crate::calc::CalcState {
        &mut self.calc
    }
    #[cfg(feature = "test-support")]
    pub fn folds_mut_for_tests(&mut self) -> &mut crate::folds::FoldStructure {
        &mut self.folds
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
