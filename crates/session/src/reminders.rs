use app_core::reminders::{block_line_fates, LineEdit};
use editor_core::history::LineDelta;
use rustc_hash::FxHashMap;
use std::sync::Arc;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineReminderGhost {
    pub remind_at_ms: i64,
    pub display_at: String,
    pub line_text: String,
    pub reminded_at_ms: Option<i64>,
}

/// The open note's reminders by line, as undo history carries them.
pub type ReminderMarks = std::sync::Arc<Vec<(usize, LineReminderGhost)>>;

#[derive(Debug, Clone)]
pub struct ReminderUndoEntry {
    pub line_idx: usize,
    pub before: Option<LineReminderGhost>,
    pub after: Option<LineReminderGhost>,
}

/// A line change an edit reported before the buffer changed.
#[derive(Debug, Clone)]
pub enum PendingLineChange {
    /// A text edit with exact coordinates.
    Edit(LineEdit),
    /// `old_len` lines from `start` replaced as a block by `new_len` lines;
    /// `fates` says where each old line of the block went.
    Block {
        start: usize,
        old_len: usize,
        new_len: usize,
        fates: Vec<Option<usize>>,
    },
}

impl PendingLineChange {
    pub fn line_change(&self) -> isize {
        match self {
            Self::Edit(edit) => {
                edit.inserted_breaks as isize - (edit.to_line - edit.from_line) as isize
            }
            Self::Block {
                old_len, new_len, ..
            } => *new_len as isize - *old_len as isize,
        }
    }

    /// Where each of `lines` (ascending) goes.
    pub fn fates(&self, lines: &[usize]) -> Vec<Option<usize>> {
        match self {
            Self::Edit(edit) => edit.fates(lines),
            Self::Block {
                start,
                old_len,
                new_len,
                fates,
            } => lines
                .iter()
                .map(|line| {
                    if line < start {
                        Some(*line)
                    } else if *line >= start + old_len {
                        Some(line + new_len - old_len)
                    } else {
                        fates[line - start].map(|idx| start + idx)
                    }
                })
                .collect(),
        }
    }
}

/// `ghosts` as undo history marks: sorted by line.
pub fn reminder_marks_of(ghosts: &FxHashMap<usize, LineReminderGhost>) -> ReminderMarks {
    let mut marks: Vec<(usize, LineReminderGhost)> = ghosts
        .iter()
        .map(|(line, ghost)| (*line, ghost.clone()))
        .collect();
    marks.sort_by_key(|(line, _)| *line);
    Arc::new(marks)
}

/// `ghosts` keyed by where their lines are after `delta`.
pub fn move_lines(
    ghosts: &FxHashMap<usize, LineReminderGhost>,
    edits: &[PendingLineChange],
    delta: &LineDelta,
) -> FxHashMap<usize, LineReminderGhost> {
    let mut lines: Vec<usize> = ghosts.keys().copied().collect();
    lines.sort_unstable();
    let delta_change = delta.inserted.len() as isize - delta.removed.len() as isize;
    let edits_change: isize = edits.iter().map(PendingLineChange::line_change).sum();
    let targets: Vec<Option<usize>> = if !edits.is_empty() && edits_change == delta_change {
        // The edits applied in order; each keeps lines ascending.
        let mut current: Vec<Option<usize>> = lines.iter().map(|line| Some(*line)).collect();
        for edit in edits {
            let alive: Vec<usize> = current.iter().flatten().copied().collect();
            let mut fates = edit.fates(&alive).into_iter();
            for slot in current.iter_mut().filter(|slot| slot.is_some()) {
                *slot = fates.next().flatten();
            }
        }
        current
    } else {
        let block_end = delta.start + delta.removed.len();
        let fates = block_line_fates(&delta.removed, &delta.inserted);
        lines
            .iter()
            .map(|line| {
                if *line < delta.start {
                    Some(*line)
                } else if *line >= block_end {
                    Some((*line as isize + delta_change) as usize)
                } else {
                    fates[line - delta.start].map(|idx| delta.start + idx)
                }
            })
            .collect()
    };
    lines
        .iter()
        .zip(targets)
        .filter_map(|(line, target)| Some((target?, ghosts[line].clone())))
        .collect()
}

impl crate::NoteSession {
    /// Update one reminder and its semantic history entry as a single session change.
    /// Notification acknowledgements use `record_undo = false`.
    pub fn set_reminder(
        &mut self,
        doc: &crate::Document,
        line: usize,
        state: Option<LineReminderGhost>,
        record_undo: bool,
    ) -> bool {
        if !self.editable() || !self.holds_reminders() || line >= doc.lines().len() {
            return false;
        }
        let before = self.reminder_ghosts.get(&line).cloned();
        if before == state {
            return false;
        }
        match &state {
            Some(mark) => {
                self.reminder_ghosts.insert(line, mark.clone());
            }
            None => {
                self.reminder_ghosts.remove(&line);
            }
        }
        if record_undo {
            self.history.break_coalescing();
            self.undo_policy.record_reminder(ReminderUndoEntry {
                line_idx: line,
                before,
                after: state,
            });
        }
        self.reminder_changed_outside_text();
        true
    }

    /// Compatibility boundary for reconciliation that already changed the mark map.
    /// Hosts retain persistence and debounce clocks; the session owns change identity.
    pub fn reminder_changed_outside_text(&mut self) -> bool {
        if !self.editable() || !self.holds_reminders() {
            return false;
        }
        self.reminders_generation = self.reminders_generation.wrapping_add(1);
        self.note_changed();
        self.history
            .set_marks(reminder_marks_of(&self.reminder_ghosts));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (crate::Document, crate::NoteSession) {
        let doc = crate::Document::from_text("first\nsecond");
        let history =
            crate::lifecycle::build_history_for_note(doc.lines(), 0, 0, Default::default());
        let mut session = crate::NoteSession::new(history, Default::default(), Default::default());
        session.start_lifetime("a");
        (doc, session)
    }
    fn mark() -> LineReminderGhost {
        LineReminderGhost {
            remind_at_ms: 1,
            display_at: "soon".into(),
            line_text: "second".into(),
            reminded_at_ms: None,
        }
    }
    #[test]
    fn reminder_mutation_updates_marks_identity_and_semantic_history_atomically() {
        let (doc, mut session) = fixture();
        assert!(session.set_reminder(&doc, 1, Some(mark()), true));
        assert_eq!((session.reminders_generation, session.edit_seq), (1, 1));
        assert_eq!(session.undo_policy.undo_depth(), 1);
        assert_eq!(
            **session.history.current_marks(),
            *reminder_marks_of(&session.reminder_ghosts)
        );
        assert!(!session.dirty);
        assert_eq!(doc.text_generation, 0);
        let editor_core::history::policy::UndoAction::Reminder(entry) =
            session.undo_policy.undo_action().unwrap()
        else {
            panic!("reminder marker")
        };
        assert_eq!(entry.line_idx, 1);
        assert_eq!(entry.before, None);
        assert_eq!(entry.after, Some(mark()));
        assert!(!session.set_reminder(&doc, 1, Some(mark()), true));
        assert_eq!(
            (
                session.reminders_generation,
                session.edit_seq,
                session.undo_policy.undo_depth()
            ),
            (1, 1, 1)
        );
        let mut notified = mark();
        notified.reminded_at_ms = Some(20);
        assert!(session.set_reminder(&doc, 1, Some(notified), false));
        assert_eq!(session.undo_policy.undo_depth(), 1);
        assert_eq!((session.reminders_generation, session.edit_seq), (2, 2));
        assert!(session.set_reminder(&doc, 1, None, true));
        assert_eq!(session.undo_policy.undo_depth(), 2);
        assert!(session.reminder_ghosts.is_empty());
    }
    #[test]
    fn invalid_locked_and_file_backed_requests_do_not_change_reminders_or_identity() {
        let (doc, mut session) = fixture();
        assert!(!session.set_reminder(&doc, 99, Some(mark()), true));
        assert!(!session.set_reminder(&doc, 0, None, true));
        session.access_mode = app_core::storage::NoteAccessMode::Encrypted;
        session.is_unlocked = false;
        assert!(!session.set_reminder(&doc, 0, Some(mark()), true));
        assert!(!session.reminder_changed_outside_text());
        session.is_unlocked = true;
        session.note_id =
            app_core::note_sources::note_id_for_markdown_file(std::path::Path::new("/tmp/note.md"));
        assert!(!session.set_reminder(&doc, 0, Some(mark()), true));
        assert!(!session.reminder_changed_outside_text());
        assert!(session.reminder_ghosts.is_empty());
        assert_eq!(
            (
                session.reminders_generation,
                session.edit_seq,
                session.undo_policy.undo_depth()
            ),
            (0, 0, 0)
        );
    }
}
