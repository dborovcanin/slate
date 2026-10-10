use super::{LineReminderGhost, ReminderMarks, TerminalApp};
use crate::storage::{Db, Note};
use app_core::storage::NoteAccessMode;
use note_session::reminder_marks_of;
use rustc_hash::FxHashMap;

/// Reminder ghosts for `note` shown as `lines`, after placing its stored
/// reminders on the text (`Db::reconcile_reminders`, for text that may have
/// changed elsewhere). A locked note's buffer is a placeholder rather than
/// its text, so its reminders are left alone until it is unlocked.
pub(super) fn load_note_reminder_ghosts(
    db: &Db,
    note: &Note,
    lines: &[String],
) -> Result<FxHashMap<usize, LineReminderGhost>, String> {
    load_reminder_ghosts(
        db,
        &note.id,
        lines,
        note.access_mode == NoteAccessMode::None || note.is_unlocked,
    )
}
fn load_reminder_ghosts(
    db: &Db,
    note_id: &str,
    lines: &[String],
    editable: bool,
) -> Result<FxHashMap<usize, LineReminderGhost>, String> {
    if !editable {
        return Ok(FxHashMap::default());
    }
    Ok(db
        .reconcile_reminders(note_id, lines)?
        .into_iter()
        .map(|reminder| {
            (
                reminder.line_number as usize - 1,
                LineReminderGhost {
                    remind_at_ms: reminder.remind_at_ms,
                    display_at: reminder.display_at,
                    line_text: reminder.line_text,
                    reminded_at_ms: reminder.reminded_at_ms,
                },
            )
        })
        .collect())
}

// Ownership: the open note's reminders while it is edited. They live here,
// move with each edit, travel with undo history, and are stored with the
// note's text, never ahead of it.
impl TerminalApp {
    /// Loads the open note's reminders as stored with its text, dropping any
    /// unsaved reminder changes.
    pub(super) fn load_reminders(&mut self, db: &Db) -> Result<(), String> {
        let ghosts = load_reminder_ghosts(
            db,
            &self.session.note_id,
            self.editor.lines(),
            self.session.editable(),
        )?;
        self.session.install_reminders(ghosts);
        Ok(())
    }

    pub(super) fn reminder_marks(&self) -> ReminderMarks {
        reminder_marks_of(&self.session.reminder_ghosts)
    }

    /// Whether reminders changed since they were last stored.
    pub(super) fn reminders_unsaved(&self) -> bool {
        self.session.reminders_unsaved()
    }

    /// Reminders can be stored only with notes in the database.
    pub(super) fn active_note_holds_reminders(&self) -> bool {
        self.session.holds_reminders()
    }

    /// A reminder change that is not a text edit (set, removed, notified,
    /// undone): it is stored right away when the text is saved, else with
    /// the next save of the text.
    pub(super) fn reminders_changed_outside_text(&mut self, db: &Db) {
        self.session.reminders_generation = self.session.reminders_generation.wrapping_add(1);
        // A change worth saving: a paused autosave tries again.
        self.session.note_changed();
        self.last_edit = std::time::Instant::now();
        self.session.history.set_marks(self.reminder_marks());
        self.render_state.dirty = true;
        self.persist_reminders_if_text_saved(db);
    }

    /// Stores reminder changes on their own while the stored text is the
    /// buffer's, checked against its revision.
    pub(super) fn persist_reminders_if_text_saved(&mut self, db: &Db) {
        if self.session.dirty || !self.reminders_unsaved() || !self.active_note_holds_reminders() {
            return;
        }
        // An autosave in flight decides the stored revision first.
        self.poll_background_save(db, true);
        if self.session.dirty || !self.reminders_unsaved() {
            return;
        }
        let Some(job) = self
            .session
            .request_save(&mut self.editor, Default::default())
        else {
            return;
        };
        let result = note_session::jobs::run_save(job, db);
        if let note_session::save::SaveCompletion::Failed(error) =
            self.session.complete_save(&mut self.editor, result)
        {
            self.status = format!("reminders not saved yet: {error}");
        }
    }

    /// Lines `start..=end` are about to go by a command that deletes whole
    /// lines (`dd`, a linewise visual delete): their reminders go with them,
    /// even where an empty line is left behind, as for the note's only line.
    /// The text change alone cannot tell that from emptying a line.
    pub(super) fn drop_reminders_on_deleted_lines(&mut self, start: usize, end: usize) {
        let before = self.session.reminder_ghosts.len();
        self.session
            .reminder_ghosts
            .retain(|line, _| *line < start || *line > end);
        if self.session.reminder_ghosts.len() != before {
            self.session.reminders_generation = self.session.reminders_generation.wrapping_add(1);
        }
    }
}
