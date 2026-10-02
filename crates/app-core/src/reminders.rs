//! Keeping line reminders attached to their lines as a note is edited.
//!
//! Reminders are stored by line number together with the text the line had.
//! After an edit, [`place_reminders`] finds where each one belongs in the
//! note's current lines; `Db::reconcile_reminders` stores the result.

use crate::storage::Reminder;
use rustc_hash::{FxHashMap, FxHashSet};

/// The 1-based line each reminder belongs on in `lines`, in the order of
/// `reminders`, or `None` when its line is gone.
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
