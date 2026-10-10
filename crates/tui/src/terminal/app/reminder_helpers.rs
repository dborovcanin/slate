use super::{LineReminderGhost, ReminderMarks, TerminalApp};
use crate::storage::{Db, Note};
use app_core::storage::{NoteAccessMode, ReminderLine};
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
    if note.access_mode != NoteAccessMode::None && !note.is_unlocked {
        return Ok(FxHashMap::default());
    }
    Ok(db
        .reconcile_reminders(&note.id, lines)?
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
        self.session.reminder_ghosts =
            load_note_reminder_ghosts(db, &self.active_note, self.editor.lines())?;
        self.session.pending_line_edits.clear();
        self.session.reminders_generation = self.session.reminders_generation.wrapping_add(1);
        self.session.persisted_reminders_generation = self.session.reminders_generation;
        Ok(())
    }

    pub(super) fn reminder_marks(&self) -> ReminderMarks {
        reminder_marks_of(&self.session.reminder_ghosts)
    }

    /// Whether reminders changed since they were last stored.
    pub(super) fn reminders_unsaved(&self) -> bool {
        self.session.reminders_generation != self.session.persisted_reminders_generation
    }

    /// Reminders can be stored only with notes in the database.
    pub(super) fn active_note_holds_reminders(&self) -> bool {
        app_core::note_sources::markdown_file_path_from_note_id(&self.active_note.id).is_none()
    }

    /// The reminders to store with the current text.
    pub(super) fn reminder_lines(&self) -> Vec<ReminderLine> {
        let mut lines: Vec<ReminderLine> = self
            .session
            .reminder_ghosts
            .iter()
            .map(|(line_idx, ghost)| ReminderLine {
                line_number: *line_idx as i64 + 1,
                remind_at_ms: ghost.remind_at_ms,
                display_at: ghost.display_at.clone(),
                line_text: self
                    .editor
                    .lines()
                    .get(*line_idx)
                    .cloned()
                    .unwrap_or_else(|| ghost.line_text.clone()),
                reminded_at_ms: ghost.reminded_at_ms,
            })
            .collect();
        lines.sort_by_key(|line| line.line_number);
        lines
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
        let generation = self.session.reminders_generation;
        match db.replace_reminders_if(
            &self.active_note.id,
            Some(&self.active_note.updated_at),
            &self.reminder_lines(),
        ) {
            Ok(revision) => {
                // Our own checked write: its revision is the one we hold.
                self.active_note.updated_at = revision.updated_at;
                self.session.persisted_reminders_generation = generation;
            }
            Err(error) => {
                self.status = format!("reminders not saved yet: {error}");
            }
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

    /// The reminders to store with a save of the text: always while there
    /// are any (their line text follows edits), and after any change.
    pub(super) fn reminders_for_save(&self, saving_text: bool) -> Option<Vec<ReminderLine>> {
        let needed =
            self.reminders_unsaved() || (saving_text && !self.session.reminder_ghosts.is_empty());
        (needed && self.active_note_holds_reminders()).then(|| self.reminder_lines())
    }
}
