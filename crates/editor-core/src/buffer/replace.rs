//! Borrowed same-line replacement plans with character input and byte metadata.
use super::primitives::{byte_index, BufferCursor};
use super::{EditDelta, ExactTextEdit};
use std::ops::Range;

pub struct LineReplacePlan<'a> {
    pub edit: ExactTextEdit,
    pub delta: EditDelta,
    pub cursor: BufferCursor,
    text: &'a str,
}

/// Clamp character bounds to the line end. Reversed ranges, missing lines,
/// multiline replacements and unchanged replacements have no effect.
pub fn prepare_line_replace<'a>(
    lines: &[String],
    line: usize,
    char_range: Range<usize>,
    text: &'a str,
) -> Option<LineReplacePlan<'a>> {
    if char_range.start > char_range.end || text.contains(['\n', '\r']) {
        return None;
    }
    let current = lines.get(line)?;
    let from = byte_index(current, char_range.start);
    let to = byte_index(current, char_range.end);
    if &current[from..to] == text {
        return None;
    }
    Some(LineReplacePlan {
        edit: ExactTextEdit {
            from: (line, from),
            to: (line, to),
            from_line_len: current.len(),
            to_line_len: current.len(),
            inserted_breaks: 0,
        },
        delta: EditDelta {
            start_line: line,
            old_span: 1,
            new_span: 1,
        },
        cursor: BufferCursor {
            line,
            column: current[..from].chars().count() + text.chars().count(),
        },
        text,
    })
}

/// Apply to the unchanged buffer used for preparation, after recording metadata.
pub fn apply_line_replace(lines: &mut [String], plan: LineReplacePlan<'_>) -> BufferCursor {
    let current = &mut lines[plan.edit.from.0];
    debug_assert_eq!(current.len(), plan.edit.from_line_len);
    current.replace_range(plan.edit.from.1..plan.edit.to.1, plan.text);
    plan.cursor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_replacement_records_bytes_and_returns_character_cursor() {
        let mut lines = vec!["aé🙂z".into()];
        let plan = prepare_line_replace(&lines, 0, 1..3, "λ").unwrap();
        assert_eq!(plan.edit.from, (0, 1));
        assert_eq!(plan.edit.to, (0, 7));
        assert_eq!(
            plan.delta,
            EditDelta {
                start_line: 0,
                old_span: 1,
                new_span: 1
            }
        );
        assert_eq!(
            apply_line_replace(&mut lines, plan),
            BufferCursor { line: 0, column: 2 }
        );
        assert_eq!(lines[0], "aλz");
    }

    #[test]
    fn empty_replacement_and_past_end_ranges_clamp() {
        let mut lines = vec!["é🙂".into()];
        let plan = prepare_line_replace(&lines, 0, 1..99, "").unwrap();
        assert_eq!(apply_line_replace(&mut lines, plan).column, 1);
        assert_eq!(lines[0], "é");
        let plan = prepare_line_replace(&lines, 0, 99..100, "β").unwrap();
        assert_eq!(apply_line_replace(&mut lines, plan).column, 2);
        assert_eq!(lines[0], "éβ");
    }

    #[test]
    fn invalid_and_unchanged_replacements_are_noops() {
        let lines = vec!["é🙂".into()];
        for (line, range, text) in [
            (0, 0..1, "é"),
            (0, 99..100, ""),
            (0, Range { start: 2, end: 1 }, "x"),
            (1, 0..0, "x"),
            (0, 0..0, "\n"),
            (0, 0..0, "\r"),
        ] {
            assert!(prepare_line_replace(&lines, line, range, text).is_none());
        }
        assert!(prepare_line_replace(&[], 0, 0..0, "x").is_none());
    }
}
