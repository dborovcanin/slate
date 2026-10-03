use crate::types::{EditOperation, OperationSelection, TextChange};
use similar::{capture_diff_slices_deadline, Algorithm, DiffTag};
use std::time::{Duration, Instant};

pub fn single_change(change: TextChange, selection: Option<OperationSelection>) -> EditOperation {
    EditOperation {
        changes: vec![change],
        selection,
    }
}

pub fn replace_range(
    from: usize,
    to: usize,
    insert: impl Into<String>,
    selection: Option<OperationSelection>,
) -> EditOperation {
    single_change(
        TextChange {
            from,
            to,
            insert: insert.into(),
        },
        selection,
    )
}

/// Bound on diff work for pathological inputs; past it the changes stay
/// correct, only less minimal.
const REPLACE_TEXT_DIFF_DEADLINE: Duration = Duration::from_millis(200);

/// The changes that turn `old` into `new`, from a line diff, each cut down to
/// the bytes that differ (at char boundaries); `None` when the texts are
/// equal. Used to take in text changed outside the editor as an ordinary
/// edit, so the cursor, undo and reminders follow the lines they were on.
pub fn replace_text(old: &str, new: &str) -> Option<EditOperation> {
    if old == new {
        return None;
    }
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let line_starts = |lines: &[&str]| {
        let mut starts = Vec::with_capacity(lines.len() + 1);
        let mut at = 0;
        starts.push(at);
        for line in lines {
            at += line.len();
            starts.push(at);
        }
        starts
    };
    let old_starts = line_starts(&old_lines);
    let new_starts = line_starts(&new_lines);
    let deadline = Some(Instant::now() + REPLACE_TEXT_DIFF_DEADLINE);
    let changes = capture_diff_slices_deadline(Algorithm::Myers, &old_lines, &new_lines, deadline)
        .into_iter()
        .filter(|op| op.tag() != DiffTag::Equal)
        .map(|op| {
            let (old_range, new_range) = (op.old_range(), op.new_range());
            let from = old_starts[old_range.start];
            let to = old_starts[old_range.end];
            let insert = &new[new_starts[new_range.start]..new_starts[new_range.end]];
            trimmed_change(&old[from..to], insert, from)
        })
        .collect();
    Some(EditOperation {
        changes,
        selection: None,
    })
}

/// The change replacing `old` (at byte `at`) with `new`, without their
/// common prefix and suffix.
fn trimmed_change(old: &str, new: &str, at: usize) -> TextChange {
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = old.len().min(new.len()) - prefix;
    let mut suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    TextChange {
        from: at + prefix,
        to: at + old.len() - suffix,
        insert: new[prefix..new.len() - suffix].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies the changes (in `old`'s coordinates) from the last one back.
    fn apply(old: &str, op: &EditOperation) -> String {
        let mut text = old.to_string();
        for change in op.changes.iter().rev() {
            text.replace_range(change.from..change.to, &change.insert);
        }
        text
    }

    #[test]
    fn replace_text_is_none_for_equal_texts() {
        assert_eq!(replace_text("same", "same"), None);
        assert_eq!(replace_text("", ""), None);
    }

    #[test]
    fn replace_text_covers_only_what_changed() {
        let op = replace_text("# Log\n- a\n", "# Log\n- a\n- b\n").expect("changed");
        assert_eq!(op.changes.len(), 1);
        let change = &op.changes[0];
        assert_eq!((change.from, change.to), (10, 10));
        assert_eq!(change.insert, "- b\n");
        assert_eq!(op.selection, None);

        let op = replace_text("keep this line", "keep that line").expect("changed");
        assert_eq!(op.changes[0].from, 7);
        assert_eq!(op.changes[0].to, 9);
        assert_eq!(op.changes[0].insert, "at");
    }

    #[test]
    fn replace_text_keeps_unchanged_lines_between_changes() {
        let old = "# Log\n- a\n";
        let new = "intro\n# Log\n- a\n- b\n";
        let op = replace_text(old, new).expect("changed");
        let spans = op
            .changes
            .iter()
            .map(|c| (c.from, c.to, c.insert.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(spans, [(0, 0, "intro\n"), (10, 10, "- b\n")]);
        assert_eq!(apply(old, &op), new);
    }

    #[test]
    fn replace_text_handles_repeats_and_empty_sides() {
        for (old, new) in [
            ("aaa", "aaaa"),
            ("aaaa", "aa"),
            ("", "new"),
            ("old", ""),
            ("x\ny\n", "x\nz\ny\n"),
            ("a\nb", "a\nb\nc"),
            ("a\nb\nc\nd", "b\na\nd\nc\ne"),
        ] {
            let op = replace_text(old, new).expect("changed");
            assert_eq!(apply(old, &op), new, "{old:?} -> {new:?}");
        }
    }

    #[test]
    fn replace_text_cuts_at_char_boundaries() {
        // "é" (c3 a9) and "è" (c3 a8) share their first byte; "ü" and "ö"
        // differ in the second byte only.
        for (old, new) in [("café", "cafè"), ("über", "öber"), ("aé b", "aè b")] {
            let op = replace_text(old, new).expect("changed");
            let change = &op.changes[0];
            assert!(old.is_char_boundary(change.from) && old.is_char_boundary(change.to));
            assert_eq!(apply(old, &op), new);
        }
    }
}
