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
