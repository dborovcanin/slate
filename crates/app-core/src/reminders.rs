//! Keeping line reminders attached to their lines as a note is edited.
//!
//! Reminders are stored by line number together with the text the line had.
//! While a note is open, the editor moves them with each edit: [`LineEdit`]
//! and [`block_line_fates`] say where every old line went, from the edit
//! itself. [`place_reminders`] is only for text that changed elsewhere, when
//! a note is opened: it guesses from the text, and `Db::reconcile_reminders`
//! stores the result.

use crate::storage::Reminder;
use rustc_hash::{FxHashMap, FxHashSet};

/// One text edit in the coordinates of the text before it: the text from
/// (`from_line`, `from_col`) to (`to_line`, `to_col`) (byte columns, end
/// exclusive) was replaced by text holding `inserted_breaks` line breaks.
/// `from_line_len` and `to_line_len` are the old lengths of those lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineEdit {
    pub from_line: usize,
    pub from_col: usize,
    pub from_line_len: usize,
    pub to_line: usize,
    pub to_col: usize,
    pub to_line_len: usize,
    pub inserted_breaks: usize,
}

impl LineEdit {
    /// Where old line `line` is after the edit, or `None` when the edit
    /// deleted it. A line is kept while some of its text is: the first line
    /// keeps its start (so splitting a line keeps it on the first part), the
    /// last keeps its end, and lines wholly inside the edit are deleted. A
    /// line whose text was replaced entirely in place is edited, not deleted.
    /// A join keeps both ends on one line; [`LineEdit::fates`] decides which.
    pub fn line_after(&self, line: usize) -> Option<usize> {
        let (from, to, breaks) = (self.from_line, self.to_line, self.inserted_breaks);
        if line < from {
            return Some(line);
        }
        if line > to {
            return Some(line + breaks + from - to);
        }
        let keeps_start = self.from_col > 0;
        let keeps_end = self.to_col < self.to_line_len || (from < to && self.to_col == 0);
        if from == to {
            return Some(if keeps_start || !keeps_end {
                from
            } else {
                from + breaks
            });
        }
        if line == from {
            // Nothing of it is left but an empty line nobody else claims.
            let only_empty_line_left = !keeps_start && !keeps_end && breaks == 0;
            return (keeps_start || only_empty_line_left).then_some(from);
        }
        if line == to {
            return keeps_end.then_some(from + breaks);
        }
        None
    }

    /// [`LineEdit::line_after`] for each of `lines` (ascending), where two old
    /// lines joined onto one new line: the first keeps it, the later one is
    /// deleted.
    pub fn fates(&self, lines: &[usize]) -> Vec<Option<usize>> {
        let mut taken = FxHashSet::default();
        lines
            .iter()
            .map(|line| {
                self.line_after(*line)
                    .filter(|target| taken.insert(*target))
            })
            .collect()
    }
}

/// Above this many line pairs, [`block_line_fates`] stops matching lines
/// inside a block and keeps only its unchanged ends.
pub const BLOCK_MATCH_CAP: usize = 250_000;

/// Where each line of `old` went when the block was replaced by `new` (as a
/// whole, by a structured edit such as a table or list rewrite), as an index
/// into `new`, or `None` when it was deleted. Equal lines are matched
/// (unchanged ends first, then a longest common sequence up to
/// [`BLOCK_MATCH_CAP`] pairs); between matched lines, as many old as new
/// lines pair up in order as edits, and any other mismatch counts as deleted
/// rather than guessed.
pub fn block_line_fates(old: &[String], new: &[String]) -> Vec<Option<usize>> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let mut fates: Vec<Option<usize>> = vec![None; old.len()];
    for (idx, fate) in fates.iter_mut().enumerate().take(prefix) {
        *fate = Some(idx);
    }
    for idx in 0..suffix {
        fates[old.len() - 1 - idx] = Some(new.len() - 1 - idx);
    }
    let old_mid = prefix..old.len() - suffix;
    let new_mid = prefix..new.len() - suffix;
    // Matched (old, new) pairs inside the middle, ascending.
    let mut anchors: Vec<(usize, usize)> = Vec::new();
    if old_mid.len().saturating_mul(new_mid.len()) <= BLOCK_MATCH_CAP {
        anchors = longest_common_lines(&old[old_mid.clone()], &new[new_mid.clone()])
            .into_iter()
            .map(|(o, n)| (o + prefix, n + prefix))
            .collect();
    }
    let mut old_at = old_mid.start;
    let mut new_at = new_mid.start;
    for (o, n) in anchors
        .iter()
        .copied()
        .chain(std::iter::once((old_mid.end, new_mid.end)))
    {
        if o - old_at == n - new_at {
            for offset in 0..o - old_at {
                fates[old_at + offset] = Some(new_at + offset);
            }
        }
        if o < old_mid.end {
            fates[o] = Some(n);
        }
        old_at = o + 1;
        new_at = n + 1;
    }
    fates
}

/// Index pairs of a longest common subsequence of equal lines.
fn longest_common_lines(old: &[String], new: &[String]) -> Vec<(usize, usize)> {
    let (rows, cols) = (old.len(), new.len());
    // lengths[i][j]: LCS of old[i..] and new[j..].
    let mut lengths = vec![0u32; (rows + 1) * (cols + 1)];
    let at = |i: usize, j: usize| i * (cols + 1) + j;
    for i in (0..rows).rev() {
        for j in (0..cols).rev() {
            lengths[at(i, j)] = if old[i] == new[j] {
                lengths[at(i + 1, j + 1)] + 1
            } else {
                lengths[at(i + 1, j)].max(lengths[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut pairs = Vec::new();
    while i < rows && j < cols {
        if old[i] == new[j] {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if lengths[at(i + 1, j)] >= lengths[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// The 1-based line each reminder belongs on in `lines`, in the order of
/// `reminders`, or `None` when its line is gone. For text changed outside
/// the editor, when no edit says what happened.
///
/// A reminder stays on its line while the text there is unchanged, follows
/// its exact text to the nearest free line when lines moved, and stays on
/// its line number when that line was edited in place (the old and new text
/// share at least half of the shorter one as a common prefix or suffix).
/// Anything else means the line was deleted: the reminder is not moved onto
/// whatever text took its place.
pub fn place_reminders(reminders: &[Reminder], lines: &[String]) -> Vec<Option<usize>> {
    let mut placed: Vec<Option<usize>> = vec![None; reminders.len()];
    let mut used: FxHashSet<usize> = FxHashSet::default();
    let line_at = |line: usize| line.checked_sub(1).and_then(|idx| lines.get(idx));
    let stored_line = |reminder: &Reminder| usize::try_from(reminder.line_number).ok();

    // Unchanged lines first, so a moved reminder never takes their place.
    for (slot, reminder) in placed.iter_mut().zip(reminders) {
        let Some(line) = stored_line(reminder) else {
            continue;
        };
        if line_at(line) == Some(&reminder.line_text) && used.insert(line) {
            *slot = Some(line);
        }
    }

    // Lines that moved: the same text elsewhere, nearest first.
    let mut by_text: Option<FxHashMap<&str, Vec<usize>>> = None;
    for (slot, reminder) in placed.iter_mut().zip(reminders) {
        if slot.is_some() {
            continue;
        }
        let by_text = by_text.get_or_insert_with(|| {
            let mut index: FxHashMap<&str, Vec<usize>> = FxHashMap::default();
            for (idx, line) in lines.iter().enumerate() {
                index.entry(line.as_str()).or_default().push(idx + 1);
            }
            index
        });
        let preferred = stored_line(reminder).unwrap_or(1);
        let nearest = by_text
            .get(reminder.line_text.as_str())
            .into_iter()
            .flatten()
            .copied()
            .filter(|line| !used.contains(line))
            .min_by_key(|line| (line.abs_diff(preferred), *line));
        if let Some(line) = nearest {
            used.insert(line);
            *slot = Some(line);
        }
    }

    // Lines edited in place.
    for (slot, reminder) in placed.iter_mut().zip(reminders) {
        if slot.is_some() {
            continue;
        }
        let Some(line) = stored_line(reminder) else {
            continue;
        };
        let Some(text) = line_at(line) else {
            continue;
        };
        if is_same_line_edited(&reminder.line_text, text) && used.insert(line) {
            *slot = Some(line);
        }
    }
    placed
}

/// Whether `new` reads as an edit of `old` rather than a different line.
fn is_same_line_edited(old: &str, new: &str) -> bool {
    let (old, new) = (old.trim(), new.trim());
    // A blank line carries nothing to recognise it by.
    if old.is_empty() {
        return true;
    }
    if new.is_empty() {
        return false;
    }
    let shorter = old.chars().count().min(new.chars().count());
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old
        .chars()
        .rev()
        .zip(new.chars().rev())
        .take_while(|(a, b)| a == b)
        .count();
    prefix.max(suffix) * 2 >= shorter
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reminder(line_number: i64, line_text: &str) -> Reminder {
        Reminder {
            note_id: "n1".to_string(),
            line_number,
            remind_at_ms: 0,
            display_at: String::new(),
            line_text: line_text.to_string(),
            reminded_at_ms: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn unchanged_lines_keep_their_reminders() {
        let placed = place_reminders(
            &[reminder(1, "a"), reminder(3, "c")],
            &lines(&["a", "b", "c"]),
        );
        assert_eq!(placed, vec![Some(1), Some(3)]);
    }

    #[test]
    fn reminders_follow_moved_lines() {
        // A line inserted above shifts both reminded lines down.
        let placed = place_reminders(
            &[reminder(1, "buy milk"), reminder(2, "call Ana")],
            &lines(&["new", "buy milk", "call Ana"]),
        );
        assert_eq!(placed, vec![Some(2), Some(3)]);
    }

    #[test]
    fn deleted_line_does_not_pass_its_reminder_on() {
        let placed = place_reminders(
            &[reminder(1, "buy milk"), reminder(3, "pay rent")],
            &lines(&["call Ana", "x", "pay rent"]),
        );
        assert_eq!(placed, vec![None, Some(3)]);
    }

    #[test]
    fn line_edited_in_place_keeps_its_reminder() {
        let placed = place_reminders(
            &[
                reminder(1, "buy milk"),
                reminder(2, "call Ana"),
                reminder(3, "- [ ] pay rent"),
            ],
            &lines(&["buy milk and eggs", "call Anna", "- [x] pay rent"]),
        );
        assert_eq!(placed, vec![Some(1), Some(2), Some(3)]);
    }

    /// The edit replacing `from..to` (line, byte col) of `old` with `insert`.
    fn edit(old: &[&str], from: (usize, usize), to: (usize, usize), insert: &str) -> LineEdit {
        LineEdit {
            from_line: from.0,
            from_col: from.1,
            from_line_len: old[from.0].len(),
            to_line: to.0,
            to_col: to.1,
            to_line_len: old[to.0].len(),
            inserted_breaks: insert.matches('\n').count(),
        }
    }

    #[test]
    fn deleting_a_line_deletes_only_that_line() {
        let old = ["- [ ] buy milk", "- [ ] buy eggs", "end"];
        // dd on line 0 removes "- [ ] buy milk\n".
        let dd = edit(&old, (0, 0), (1, 0), "");
        assert_eq!(dd.fates(&[0, 1, 2]), vec![None, Some(0), Some(1)]);
        // dd on the last line removes the break before it.
        let last = edit(&old, (1, 14), (2, 3), "");
        assert_eq!(last.fates(&[0, 1, 2]), vec![Some(0), Some(1), None]);
    }

    #[test]
    fn splitting_a_line_keeps_it_with_its_start() {
        let old = ["abcdef", "next"];
        let middle = edit(&old, (0, 3), (0, 3), "\n");
        assert_eq!(middle.fates(&[0, 1]), vec![Some(0), Some(2)]);
        let at_start = edit(&old, (0, 0), (0, 0), "\n");
        assert_eq!(at_start.fates(&[0, 1]), vec![Some(1), Some(2)]);
        let at_end = edit(&old, (0, 6), (0, 6), "\n");
        assert_eq!(at_end.fates(&[0, 1]), vec![Some(0), Some(2)]);
        // `O` above line 1.
        let open_above = edit(&old, (1, 0), (1, 0), "x\n");
        assert_eq!(open_above.fates(&[0, 1]), vec![Some(0), Some(2)]);
    }

    #[test]
    fn a_join_keeps_the_first_line_and_drops_the_second() {
        let old = ["call Ana", "  about lunch", "end"];
        // J: the break and indent become one space.
        let join = edit(&old, (0, 8), (1, 2), " ");
        assert_eq!(join.fates(&[0, 1, 2]), vec![Some(0), None, Some(1)]);
        // Only the second line reminded: it moves onto the joined line.
        assert_eq!(join.fates(&[1]), vec![Some(0)]);
        // Backspace at the start of line 1.
        let backspace = edit(&old, (0, 8), (1, 0), "");
        assert_eq!(backspace.fates(&[0, 1]), vec![Some(0), None]);
    }

    #[test]
    fn replacing_a_whole_line_edits_it_in_place() {
        let old = ["buy milk", "end"];
        let cc = edit(&old, (0, 0), (0, 8), "buy bread");
        assert_eq!(cc.fates(&[0, 1]), vec![Some(0), Some(1)]);
    }

    #[test]
    fn block_rewrites_match_equal_lines_and_never_guess() {
        let lines = |text: &[&str]| -> Vec<String> { text.iter().map(|s| s.to_string()).collect() };
        // Every line changed, none deleted: edited line by line.
        assert_eq!(
            block_line_fates(&lines(&["a", "b"]), &lines(&["A", "B"])),
            vec![Some(0), Some(1)]
        );
        // milk is gone, eggs moved up and bread is new.
        assert_eq!(
            block_line_fates(
                &lines(&["buy milk", "buy eggs"]),
                &lines(&["buy eggs", "buy bread"])
            ),
            vec![None, Some(0)]
        );
        // After the match, one old and one new line pair up as an edit.
        assert_eq!(
            block_line_fates(
                &lines(&["buy milk", "buy eggs", "x"]),
                &lines(&["buy eggs", "buy bread"])
            ),
            vec![None, Some(0), Some(1)]
        );
        // An unchanged interior line anchors both sides.
        assert_eq!(
            block_line_fates(
                &lines(&["a", "keep", "b", "c"]),
                &lines(&["A", "keep", "B"])
            ),
            vec![Some(0), Some(1), None, None]
        );
    }

    #[test]
    fn block_rewrites_past_the_cap_keep_only_unchanged_ends() {
        let old: Vec<String> = (0..600).map(|i| format!("old {i}")).collect();
        let mut new: Vec<String> = (0..601).map(|i| format!("new {i}")).collect();
        new[300] = "old 299".to_string();
        let fates = block_line_fates(&old, &new);
        // Past the cap the interior match is not searched: nothing is guessed.
        assert!(old.len() * new.len() > BLOCK_MATCH_CAP);
        assert!(fates.iter().all(Option::is_none));
    }

    #[test]
    fn empty_buffer_places_nothing() {
        let placed = place_reminders(&[reminder(2, "buy milk")], &lines(&[""]));
        assert_eq!(placed, vec![None]);
    }

    #[test]
    fn duplicate_text_goes_to_the_nearest_free_line() {
        let placed = place_reminders(
            &[reminder(2, "todo"), reminder(5, "todo")],
            &lines(&["x", "y", "todo", "z", "w", "todo"]),
        );
        assert_eq!(placed, vec![Some(3), Some(6)]);
    }
}
