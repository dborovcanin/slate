//! Note lifetime and revision policy, independent of host IO and clocks.
use crate::{Document, LineReminderGhost, NoteSession};
use app_core::storage::{Note, NoteAccessMode};
use editor_core::history::LineHistory;
use rustc_hash::FxHashMap;
/// History capacity policy is shared by startup and subsequent opens.
pub fn build_history_for_note(
    lines: &[String],
    cursor_line: usize,
    cursor_col: usize,
    marks: crate::ReminderMarks,
) -> LineHistory<crate::ReminderMarks> {
    LineHistory::new(
        if lines.len() > 30_000 { 128 } else { 500 },
        lines,
        cursor_line,
        cursor_col,
        marks,
    )
}
#[derive(Default, Debug, Clone, Copy)]
pub struct Effects {
    pub repaint: bool,
    pub title_changed: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutsideChange {
    Unchanged,
    Deleted,
    Conflict,
    Reload,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaveDecision {
    Allow,
    Refuse,
}
impl NoteSession {
    /// Keep protection metadata current after a host-side unlock/encryption operation.
    pub fn set_note_metadata(&mut self, note: &Note) {
        self.stored_revision = note.updated_at.clone();
        self.set_protection(note.access_mode, note.is_unlocked);
    }
    /// Encryption or unlock state changed outside the editor; the text is kept.
    pub fn set_protection(&mut self, access_mode: NoteAccessMode, is_unlocked: bool) {
        self.access_mode = access_mode;
        self.is_unlocked = is_unlocked;
    }
    /// A still-locked note has no text to compare with; track its stored
    /// revision so the change is not reported again.
    pub fn acknowledge_locked_revision(&mut self, revision: String) {
        self.stored_revision = revision;
    }
    pub fn editable(&self) -> bool {
        self.access_mode == NoteAccessMode::None || self.is_unlocked
    }
    /// Open is a new lifetime, including reopening the same ID. Host reads note/reminders first.
    pub fn open(&mut self, note: &Note, doc: &mut Document) -> Effects {
        self.start_lifetime(&note.id);
        self.set_note_metadata(note);
        doc.set_text(&note.body);
        doc.cursor_line = 0;
        doc.cursor_col = 0;
        doc.selection_anchor = None;
        self.dirty = false;
        self.note_changed();
        self.reminder_ghosts.clear();
        self.pending_line_edits.clear();
        self.reminders_generation = self.reminders_generation.wrapping_add(1);
        self.persisted_reminders_generation = self.reminders_generation;
        self.history = build_history_for_note(
            doc.lines(),
            0,
            0,
            crate::reminder_marks_of(&self.reminder_ghosts),
        );
        self.undo_policy.clear();
        self.calc = Default::default();
        self.folds = Default::default();
        self.leave_refused_at = None;
        self.autosave_paused_at = None;
        self.outside_change_reported = None;
        Effects {
            repaint: true,
            title_changed: true,
        }
    }
    pub fn install_reminders(&mut self, ghosts: FxHashMap<usize, LineReminderGhost>) {
        self.reminder_ghosts = ghosts;
        self.pending_line_edits.clear();
        self.reminders_generation = self.reminders_generation.wrapping_add(1);
        self.persisted_reminders_generation = self.reminders_generation;
        self.history
            .set_marks(crate::reminder_marks_of(&self.reminder_ghosts));
    }
    pub fn reminders_unsaved(&self) -> bool {
        self.reminders_generation != self.persisted_reminders_generation
    }
    /// None means deleted. Repeated conflict/deletion notifications are suppressed.
    pub fn outside_change(&mut self, revision: Option<&str>) -> OutsideChange {
        let Some(revision) = revision else {
            if self.outside_change_reported.as_deref() == Some("") {
                return OutsideChange::Unchanged;
            }
            self.outside_change_reported = Some(String::new());
            return OutsideChange::Deleted;
        };
        if revision == self.stored_revision {
            return OutsideChange::Unchanged;
        }
        if self.dirty || self.reminders_unsaved() {
            if self.outside_change_reported.as_deref() == Some(revision) {
                return OutsideChange::Unchanged;
            }
            self.outside_change_reported = Some(revision.to_owned());
            return OutsideChange::Conflict;
        }
        OutsideChange::Reload
    }
    /// Called after any required host save/drain; repeat at the same edit identity permits discard.
    pub fn leave_decision(&mut self, save_failed: bool) -> LeaveDecision {
        if !save_failed && !self.dirty && !self.reminders_unsaved() {
            return LeaveDecision::Allow;
        }
        if self.leave_refused_at == Some(self.edit_seq) {
            self.leave_refused_at = None;
            return LeaveDecision::Allow;
        }
        self.leave_refused_at = Some(self.edit_seq);
        LeaveDecision::Refuse
    }
    /// Plan one undoable replacement, without copying an existing joined cache.
    pub fn prepare_stored_replacement(
        &mut self,
        doc: &Document,
        body: &str,
    ) -> Option<editor_core::types::EditOperation> {
        let text = doc
            .joined_text_cache
            .as_deref()
            .map(std::borrow::Cow::Borrowed)
            .unwrap_or_else(|| std::borrow::Cow::Owned(doc.lines().join("\n")));
        let op = editor_core::operations::replace_text(&text, body);
        if op.is_some() {
            self.history.break_coalescing();
        }
        op
    }
    /// A stored replacement stays undoable; acknowledgement does not alter its history entry.
    pub fn acknowledge_reload(&mut self, doc: &Document, revision: String, replaced: bool) {
        self.stored_revision = revision;
        self.outside_change_reported = None;
        self.dirty = false;
        if replaced {
            self.history.break_coalescing();
            self.history
                .checkpoint(doc.lines(), doc.cursor_line, doc.cursor_col);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditContext, SessionEdit};
    use editor_core::{
        buffer::primitives::PrimitiveEdit,
        history::policy::{UndoGrouping, UndoSession},
    };
    use std::time::Duration;
    fn note(id: &str, body: &str) -> Note {
        Note {
            id: id.into(),
            body: body.into(),
            pinned_title: None,
            modules: Default::default(),
            access_mode: NoteAccessMode::None,
            is_unlocked: true,
            created_at: "created".into(),
            updated_at: "revision-1".into(),
        }
    }
    fn fixture() -> (Document, NoteSession) {
        let mut doc = Document::default();
        let history = build_history_for_note(doc.lines(), 0, 0, Default::default());
        let mut session = NoteSession::new(history, Default::default(), Default::default());
        session.open(&note("a", "original"), &mut doc);
        (doc, session)
    }
    fn ctx() -> EditContext {
        EditContext {
            grouping: UndoGrouping {
                session: UndoSession::Command,
                elapsed: Duration::from_secs(1),
            },
            folds: None,
        }
    }
    #[test]
    fn reopen_is_a_new_lifetime_and_resets_document_and_session_state() {
        let (mut doc, mut session) = fixture();
        let first_lifetime = session.session_id;
        session
            .apply(
                &mut doc,
                SessionEdit::Primitive(PrimitiveEdit::InsertChar('x')),
                ctx(),
            )
            .unwrap();
        session.reminder_ghosts.insert(
            0,
            LineReminderGhost {
                remind_at_ms: 1,
                display_at: "tomorrow".into(),
                line_text: "original".into(),
                reminded_at_ms: None,
            },
        );
        session.reminders_generation += 1;
        session.calc.results = vec![Some("stale".into())];
        session.folds.rescan_pending = true;
        session.folds.analysis_ready = true;
        session.leave_refused_at = Some(session.edit_seq);
        session.autosave_paused_at = Some(session.edit_seq);
        session.outside_change_reported = Some("old".into());
        doc.selection_anchor = Some((0, 1));
        session.open(&note("b", "other"), &mut doc);
        let second_lifetime = session.session_id;
        let mut locked = note("a", "locked placeholder");
        locked.access_mode = NoteAccessMode::Encrypted;
        locked.is_unlocked = false;
        let effects = session.open(&locked, &mut doc);
        assert!(session.session_id > second_lifetime && second_lifetime > first_lifetime);
        assert_eq!(session.note_id, "a");
        assert_eq!(session.stored_revision, "revision-1");
        assert!(!session.editable());
        assert_eq!(session.access_mode, NoteAccessMode::Encrypted);
        assert_eq!(doc.lines(), &["locked placeholder".to_owned()]);
        assert_eq!(
            (doc.cursor_line, doc.cursor_col, doc.selection_anchor),
            (0, 0, None)
        );
        assert!(!session.dirty);
        assert_eq!(session.history.undo_depth(), 0);
        assert!(session.reminder_ghosts.is_empty() && session.pending_line_edits.is_empty());
        assert!(!session.reminders_unsaved());
        assert!(session.calc.results.is_empty());
        assert!(!session.folds.analysis_ready && !session.folds.rescan_pending);
        assert_eq!(
            (
                session.leave_refused_at,
                session.autosave_paused_at,
                session.outside_change_reported
            ),
            (None, None, None)
        );
        assert!(effects.repaint && effects.title_changed);
    }
    #[test]
    fn outside_changes_deduplicate_conflicts_and_deletions_and_protect_reminder_edits() {
        let (_, mut session) = fixture();
        assert_eq!(
            session.outside_change(Some("revision-1")),
            OutsideChange::Unchanged
        );
        assert_eq!(
            session.outside_change(Some("revision-2")),
            OutsideChange::Reload
        );
        session.reminders_generation += 1;
        assert_eq!(
            session.outside_change(Some("revision-2")),
            OutsideChange::Conflict
        );
        assert_eq!(
            session.outside_change(Some("revision-2")),
            OutsideChange::Unchanged
        );
        assert_eq!(
            session.outside_change(Some("revision-3")),
            OutsideChange::Conflict
        );
        assert_eq!(session.outside_change(None), OutsideChange::Deleted);
        assert_eq!(session.outside_change(None), OutsideChange::Unchanged);
        session.persisted_reminders_generation = session.reminders_generation;
        session.dirty = true;
        assert_eq!(
            session.outside_change(Some("revision-4")),
            OutsideChange::Conflict
        );
    }
    #[test]
    fn repeat_leave_discards_only_without_an_intervening_edit() {
        let (_, mut session) = fixture();
        assert_eq!(session.leave_decision(false), LeaveDecision::Allow);
        session.dirty = true;
        assert_eq!(session.leave_decision(false), LeaveDecision::Refuse);
        session.note_changed();
        assert_eq!(session.leave_decision(false), LeaveDecision::Refuse);
        assert_eq!(session.leave_decision(false), LeaveDecision::Allow);
        session.dirty = false;
        session.reminders_generation += 1;
        session.note_changed();
        assert_eq!(session.leave_decision(false), LeaveDecision::Refuse);
        assert_eq!(session.leave_decision(false), LeaveDecision::Allow);
        assert_eq!(session.leave_decision(true), LeaveDecision::Refuse);
    }
    #[test]
    fn stored_replacement_acknowledgement_preserves_undo_and_redo() {
        let (mut doc, mut session) = fixture();
        let op = session
            .prepare_stored_replacement(&doc, "é replacement\nsecond")
            .unwrap();
        session
            .apply(&mut doc, SessionEdit::Operation(&op), ctx())
            .unwrap();
        session.acknowledge_reload(&doc, "revision-2".into(), true);
        assert!(!session.dirty);
        assert_eq!(session.stored_revision, "revision-2");
        session.undo(&mut doc).unwrap();
        assert_eq!(doc.lines(), &["original".to_owned()]);
        assert!(session.dirty);
        session.redo(&mut doc).unwrap();
        assert_eq!(
            doc.lines(),
            &["é replacement".to_owned(), "second".to_owned()]
        );
        assert!(session
            .prepare_stored_replacement(&doc, "é replacement\nsecond")
            .is_none());
    }
}
