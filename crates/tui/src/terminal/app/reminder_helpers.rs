use super::LineReminderGhost;
use crate::storage::{Db, Note};
use app_core::storage::NoteAccessMode;
use rustc_hash::FxHashMap;

/// Reminder ghosts for `note` shown as `lines`, after moving its stored
/// reminders along with their lines (`Db::reconcile_reminders`). A locked
/// note's buffer is a placeholder rather than its text, so its reminders are
/// left alone until it is unlocked.
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
