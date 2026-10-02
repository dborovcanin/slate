use super::{LineReminderGhost, TerminalApp};
use crate::storage::{Db, Note};
use app_core::reminders::ReminderHints;
use app_core::storage::NoteAccessMode;
use rustc_hash::{FxHashMap, FxHashSet};

/// Reminder ghosts for `note` shown as `lines`, after moving its stored
/// reminders along with their lines (`Db::reconcile_reminders`). A locked
/// note's buffer is a placeholder rather than its text, so its reminders are
/// left alone until it is unlocked.
pub(super) fn load_note_reminder_ghosts(
    db: &Db,
    note: &Note,
    lines: &[String],
    hints: &ReminderHints,
) -> Result<FxHashMap<usize, LineReminderGhost>, String> {
    if note.access_mode != NoteAccessMode::None && !note.is_unlocked {
        return Ok(FxHashMap::default());
    }
    Ok(db
        .reconcile_reminders(&note.id, lines, hints)?
        .into_iter()
        .map(|reminder| {
            (
                reminder.line_number as usize - 1,
                LineReminderGhost {
                    remind_at_ms: reminder.remind_at_ms,
                    display_at: reminder.display_at,
                    line_text: reminder.line_text,
                    reminded_at_ms: reminder.reminded_at_ms,
                    stored_line: reminder.line_number,
                },
            )
        })
        .collect())
}

impl TerminalApp {
    /// Reconciles the open note's stored reminders with its buffer, telling
    /// the core where edits moved them since the last reconcile while that
    /// tracking still describes the buffer.
    pub(super) fn reconcile_reminder_ghosts(
        &mut self,
        db: &Db,
    ) -> Result<FxHashMap<usize, LineReminderGhost>, String> {
        let mut hints = ReminderHints::default();
        if self.reminder_tracked_len == Some(self.editor.lines.len()) {
            for (line_idx, ghost) in &self.reminder_ghosts {
                hints.insert(ghost.stored_line, Some(line_idx + 1));
            }
            for line in &self.reminder_deleted_lines {
                hints.insert(*line, None);
            }
        }
        let ghosts = load_note_reminder_ghosts(db, &self.active_note, &self.editor.lines, &hints)?;
        self.reminder_deleted_lines.clear();
        self.reminder_tracked_len = Some(self.editor.lines.len());
        Ok(ghosts)
    }

    /// Moves reminder ghosts through an edit that replaced lines
    /// `[start, start + old_span)` with `new_span` lines (already in the
    /// buffer). Lines after the block shift. Inside it, lines the edit left
    /// unchanged at either end keep their reminders; of the rest, the first
    /// old lines pair with the new ones as edits, and old lines beyond them
    /// were deleted. Without the old lines' hashes, a reminder follows its
    /// text or else keeps its place.
    pub(super) fn track_reminder_splice(&mut self, start: usize, old_span: usize, new_span: usize) {
        if let Some(len) = self.reminder_tracked_len.as_mut() {
            *len = (*len + new_span).saturating_sub(old_span);
        }
        if self.reminder_ghosts.is_empty() || old_span == new_span && old_span <= 1 {
            // Nothing to move (an edit within one line keeps its reminder).
            return;
        }
        let new_end = (start + new_span).min(self.editor.lines.len());
        let old_line_count = (self.editor.lines.len() + old_span).saturating_sub(new_span);
        // Calc metadata still describes the lines before this edit.
        let old_hashes: Option<Vec<u64>> = (self.calc.line_metadata.len() == old_line_count
            && start + old_span <= old_line_count)
            .then(|| {
                self.calc.line_metadata[start..start + old_span]
                    .iter()
                    .map(|meta| meta.hash)
                    .collect()
            });
        let block_target = |offset: usize, lines: &[String]| -> Option<usize> {
            let old_hashes = old_hashes.as_ref()?;
            let new_hashes: Vec<u64> = lines[start..new_end]
                .iter()
                .map(|line| crate::editor_core::calc_plan::hash_line(line))
                .collect();
            let shortest = old_hashes.len().min(new_hashes.len());
            let prefix = old_hashes
                .iter()
                .zip(&new_hashes)
                .take_while(|(a, b)| a == b)
                .count();
            let suffix = old_hashes
                .iter()
                .rev()
                .zip(new_hashes.iter().rev())
                .take(shortest - prefix)
                .take_while(|(a, b)| a == b)
                .count();
            Some(if offset < prefix {
                start + offset
            } else if offset >= old_hashes.len() - suffix {
                start + offset + new_hashes.len() - old_hashes.len()
            } else if offset - prefix < new_hashes.len() - prefix - suffix {
                start + offset
            } else {
                return Some(usize::MAX);
            })
        };
        let old_end = start + old_span;
        let mut moved =
            FxHashMap::with_capacity_and_hasher(self.reminder_ghosts.len(), Default::default());
        let mut taken: FxHashSet<usize> = FxHashSet::default();
        let mut inside = Vec::new();
        for (line_idx, ghost) in self.reminder_ghosts.drain() {
            if line_idx < start {
                taken.insert(line_idx);
                moved.insert(line_idx, ghost);
            } else if line_idx >= old_end {
                let shifted = line_idx + new_span - old_span;
                taken.insert(shifted);
                moved.insert(shifted, ghost);
            } else {
                inside.push((line_idx, ghost));
            }
        }
        inside.sort_by_key(|(line_idx, _)| *line_idx);
        for (line_idx, ghost) in inside {
            let target = match block_target(line_idx - start, &self.editor.lines) {
                Some(usize::MAX) => None,
                Some(target) => Some(target),
                None => (start..new_end)
                    .find(|idx| !taken.contains(idx) && self.editor.lines[*idx] == ghost.line_text)
                    .or(Some(line_idx).filter(|idx| *idx < new_end)),
            }
            .filter(|target| !taken.contains(target));
            match target {
                Some(target) => {
                    taken.insert(target);
                    moved.insert(target, ghost);
                }
                None => self.reminder_deleted_lines.push(ghost.stored_line),
            }
        }
        self.reminder_ghosts = moved;
    }
}
