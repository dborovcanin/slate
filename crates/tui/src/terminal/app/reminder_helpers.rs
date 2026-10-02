use super::{LineReminderGhost, ReminderMarks, TerminalApp};
use crate::storage::{Db, Note};
use crate::terminal::history::LineDelta;
use app_core::reminders::{block_line_fates, LineEdit};
use app_core::storage::{NoteAccessMode, ReminderLine};
use rustc_hash::FxHashMap;
use std::sync::Arc;

/// A line change an edit reported before the buffer changed.
#[derive(Debug, Clone)]
pub(super) enum PendingLineChange {
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
    fn line_change(&self) -> isize {
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
    fn fates(&self, lines: &[usize]) -> Vec<Option<usize>> {
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

/// `ghosts` as undo history marks: sorted by line.
pub(super) fn reminder_marks_of(ghosts: &FxHashMap<usize, LineReminderGhost>) -> ReminderMarks {
    let mut marks: Vec<(usize, LineReminderGhost)> = ghosts
        .iter()
        .map(|(line, ghost)| (*line, ghost.clone()))
        .collect();
    marks.sort_by_key(|(line, _)| *line);
    Arc::new(marks)
}

// Ownership: the open note's reminders while it is edited. They live here,
// move with each edit, travel with undo history, and are stored with the
// note's text, never ahead of it.
impl TerminalApp {
    /// Loads the open note's reminders as stored with its text, dropping any
    /// unsaved reminder changes.
    pub(super) fn load_reminders(&mut self, db: &Db) -> Result<(), String> {
        self.reminder_ghosts =
            load_note_reminder_ghosts(db, &self.active_note, &self.editor.lines)?;
        self.pending_line_edits.clear();
        self.reminders_generation = self.reminders_generation.wrapping_add(1);
        self.persisted_reminders_generation = self.reminders_generation;
        Ok(())
    }

    pub(super) fn reminder_marks(&self) -> ReminderMarks {
        reminder_marks_of(&self.reminder_ghosts)
    }

    /// Whether reminders changed since they were last stored.
    pub(super) fn reminders_unsaved(&self) -> bool {
        self.reminders_generation != self.persisted_reminders_generation
    }

    /// Reminders can be stored only with notes in the database.
    pub(super) fn active_note_holds_reminders(&self) -> bool {
        app_core::note_sources::markdown_file_path_from_note_id(&self.active_note.id).is_none()
    }

    /// The reminders to store with the current text.
    pub(super) fn reminder_lines(&self) -> Vec<ReminderLine> {
        let mut lines: Vec<ReminderLine> = self
            .reminder_ghosts
            .iter()
            .map(|(line_idx, ghost)| ReminderLine {
                line_number: *line_idx as i64 + 1,
                remind_at_ms: ghost.remind_at_ms,
                display_at: ghost.display_at.clone(),
                line_text: self
                    .editor
                    .lines
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
        self.reminders_generation = self.reminders_generation.wrapping_add(1);
        // A change worth saving: a paused autosave tries again.
        self.last_edit = std::time::Instant::now();
        self.history.set_marks(self.reminder_marks());
        self.render_state.dirty = true;
        self.persist_reminders_if_text_saved(db);
    }

    /// Stores reminder changes on their own while the stored text is the
    /// buffer's, checked against its revision.
    pub(super) fn persist_reminders_if_text_saved(&mut self, db: &Db) {
        if self.dirty || !self.reminders_unsaved() || !self.active_note_holds_reminders() {
            return;
        }
        // An autosave in flight decides the stored revision first.
        self.poll_background_save(db, true);
        if self.dirty || !self.reminders_unsaved() {
            return;
        }
        let generation = self.reminders_generation;
        match db.replace_reminders_if(
            &self.active_note.id,
            Some(&self.active_note.updated_at),
            &self.reminder_lines(),
        ) {
            Ok(revision) => {
                // Our own checked write: its revision is the one we hold.
                self.active_note.updated_at = revision.updated_at;
                self.persisted_reminders_generation = generation;
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
        let before = self.reminder_ghosts.len();
        self.reminder_ghosts
            .retain(|line, _| *line < start || *line > end);
        if self.reminder_ghosts.len() != before {
            self.reminders_generation = self.reminders_generation.wrapping_add(1);
        }
    }

    /// Lines `start..=end` are about to be removed directly from the buffer
    /// (a linewise visual delete, the delete-line command), without a text
    /// change to describe it: drops their reminders and records where the
    /// remaining lines go, so equal lines elsewhere cannot be mistaken for
    /// the deleted ones. Call before the lines change.
    pub(super) fn note_deleted_lines(&mut self, start: usize, end: usize) {
        self.drop_reminders_on_deleted_lines(start, end);
        let last = self.editor.lines.len().saturating_sub(1);
        let len = |line: usize| self.editor.lines.get(line).map_or(0, String::len);
        let (from, to) = if end < last {
            ((start, 0), (end + 1, 0))
        } else if start > 0 {
            ((start - 1, len(start - 1)), (end, len(end)))
        } else {
            ((0, 0), (end, len(end)))
        };
        self.note_line_edit(from, to, 0);
    }

    /// The reminders to store with a save of the text: always while there
    /// are any (their line text follows edits), and after any change.
    pub(super) fn reminders_for_save(&self, saving_text: bool) -> Option<Vec<ReminderLine>> {
        let needed = self.reminders_unsaved() || (saving_text && !self.reminder_ghosts.is_empty());
        (needed && self.active_note_holds_reminders()).then(|| self.reminder_lines())
    }

    /// Records the coordinates of an edit about to change the buffer's lines,
    /// so reminders follow it exactly (see [`LineEdit::line_after`]). Call
    /// before the lines change; columns are bytes.
    pub(super) fn note_line_edit(
        &mut self,
        from: (usize, usize),
        to: (usize, usize),
        inserted_breaks: usize,
    ) {
        if self.reminder_ghosts.is_empty() {
            return;
        }
        let len = |line: usize| self.editor.lines.get(line).map_or(0, String::len);
        self.pending_line_edits
            .push(PendingLineChange::Edit(LineEdit {
                from_line: from.0,
                from_col: from.1,
                from_line_len: len(from.0),
                to_line: to.0,
                to_col: to.1,
                to_line_len: len(to.0),
                inserted_breaks,
            }));
    }

    /// Lines `start..start + old_len` are about to be replaced as a block by
    /// `new` (a structured rewrite such as a table paste): lines outside it
    /// move exactly, lines inside are matched ([`block_line_fates`]). Call
    /// before the lines change.
    pub(super) fn note_block_replace(&mut self, start: usize, old_len: usize, new: &[String]) {
        if self.reminder_ghosts.is_empty() {
            return;
        }
        let end = (start + old_len).min(self.editor.lines.len());
        let fates = block_line_fates(&self.editor.lines[start..end], new);
        self.pending_line_edits.push(PendingLineChange::Block {
            start,
            old_len: end - start,
            new_len: new.len(),
            fates,
        });
    }

    /// `count` lines are about to be inserted before line `at`.
    pub(super) fn note_lines_inserted(&mut self, at: usize, count: usize) {
        if at > 0 {
            let above = at - 1;
            let len = self.editor.lines.get(above).map_or(0, String::len);
            self.note_line_edit((above, len), (above, len), count);
        } else {
            self.note_line_edit((0, 0), (0, 0), count);
        }
    }

    /// Moves reminders through the edit history just recorded, and attaches
    /// the result to its undo step. The edit's own coordinates decide when
    /// they account for the whole change; otherwise its changed block is
    /// matched line by line ([`block_line_fates`]), never guessed.
    pub(super) fn move_reminders_with_recorded_edit(&mut self) {
        let edits = std::mem::take(&mut self.pending_line_edits);
        let delta = self.history.take_last_delta();
        if self.reminder_ghosts.is_empty() {
            if !self.history.current_marks().is_empty() {
                self.history.record_marks(self.reminder_marks());
            }
            return;
        }
        let Some(delta) = delta else {
            return;
        };
        let moved = move_lines(&self.reminder_ghosts, &edits, &delta);
        if moved.len() != self.reminder_ghosts.len()
            || moved
                .keys()
                .any(|line| !self.reminder_ghosts.contains_key(line))
        {
            self.reminder_ghosts = moved;
            self.reminders_generation = self.reminders_generation.wrapping_add(1);
        }
        self.history.record_marks(self.reminder_marks());
    }

    /// After undo or redo: the reminders the history step holds.
    pub(super) fn restore_reminders_from_history(&mut self) {
        self.pending_line_edits.clear();
        let marks = self.history.current_marks().clone();
        if *marks != *self.reminder_marks() {
            self.reminder_ghosts = marks.iter().cloned().collect();
            self.reminders_generation = self.reminders_generation.wrapping_add(1);
        }
    }
}

/// `ghosts` keyed by where their lines are after `delta`.
fn move_lines(
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
