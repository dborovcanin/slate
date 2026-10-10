//! Primitive editing plans preserve the in-place typing path without allocating
//! replacement lines. Hosts record structural metadata before applying a plan.
use super::{EditDelta, ExactTextEdit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferCursor {
    pub line: usize,
    /// Character column, unlike the byte columns in `ExactTextEdit`.
    pub column: usize,
}

pub enum PrimitiveEdit<'a> {
    InsertChar(char),
    /// Literal insertion into one line; newline and paste handle line splits.
    InsertText(&'a str),
    Newline,
    Backspace,
    DeleteForward,
}

enum Mutation<'a> {
    InsertChar(char),
    InsertText(&'a str),
    Split,
    RemoveChar,
    Join,
}

pub struct PreparedPrimitiveEdit<'a> {
    pub edit: ExactTextEdit,
    pub delta: EditDelta,
    pub cursor: BufferCursor,
    mutation: Mutation<'a>,
}

pub(super) fn byte_index(text: &str, column: usize) -> usize {
    if column == 0 {
        return 0;
    }
    text.char_indices()
        .nth(column)
        .map_or(text.len(), |(idx, _)| idx)
}

/// Prepare an edit on the current line without changing or cloning any text.
/// The cursor must refer to an existing line (or line zero of an empty buffer
/// for insertion). Preserve the buffer until `apply_primitive_edit` consumes
/// this plan. `None` means the primitive has no effect at this boundary.
pub fn prepare_primitive_edit<'a>(
    lines: &[String],
    cursor: BufferCursor,
    primitive: PrimitiveEdit<'a>,
) -> Option<PreparedPrimitiveEdit<'a>> {
    let line = lines.get(cursor.line).map_or("", String::as_str);
    let mut after = cursor;
    let (from, to, mutation) = match primitive {
        PrimitiveEdit::InsertChar(ch) => {
            if ch.is_control() {
                return None;
            }
            let at = (cursor.line, byte_index(line, cursor.column));
            after.column += 1;
            (at, at, Mutation::InsertChar(ch))
        }
        PrimitiveEdit::InsertText(text) => {
            if text.is_empty() {
                return None;
            }
            let at = (cursor.line, byte_index(line, cursor.column));
            after.column += text.chars().count();
            (at, at, Mutation::InsertText(text))
        }
        PrimitiveEdit::Newline => {
            let at = (cursor.line, byte_index(line, cursor.column));
            after.line += 1;
            after.column = 0;
            (at, at, Mutation::Split)
        }
        PrimitiveEdit::Backspace if cursor.column > 0 => {
            after.column -= 1;
            (
                (cursor.line, byte_index(line, after.column)),
                (cursor.line, byte_index(line, cursor.column)),
                Mutation::RemoveChar,
            )
        }
        PrimitiveEdit::Backspace => {
            if cursor.line == 0 {
                return None;
            }
            let previous = &lines[cursor.line - 1];
            after.line -= 1;
            after.column = previous.chars().count();
            (
                (after.line, previous.len()),
                (cursor.line, 0),
                Mutation::Join,
            )
        }
        PrimitiveEdit::DeleteForward => {
            if cursor.column < line.chars().count() {
                (
                    (cursor.line, byte_index(line, cursor.column)),
                    (cursor.line, byte_index(line, cursor.column + 1)),
                    Mutation::RemoveChar,
                )
            } else {
                if cursor.line + 1 >= lines.len() {
                    return None;
                }
                (
                    (cursor.line, line.len()),
                    (cursor.line + 1, 0),
                    Mutation::Join,
                )
            }
        }
    };
    let (old_span, new_span, inserted_breaks) = match mutation {
        Mutation::Split => (1, 2, 1),
        Mutation::Join => (2, 1, 0),
        _ => (1, 1, 0),
    };
    Some(PreparedPrimitiveEdit {
        edit: ExactTextEdit {
            from,
            to,
            from_line_len: lines.get(from.0).map_or(0, String::len),
            to_line_len: lines.get(to.0).map_or(0, String::len),
            inserted_breaks,
        },
        delta: EditDelta {
            start_line: from.0,
            old_span: if lines.is_empty() { 0 } else { old_span },
            new_span,
        },
        cursor: after,
        mutation,
    })
}

/// Mutate only the affected lines using the original insertion/removal paths.
/// No document joins or replacement-line clones are added to typing.
pub fn apply_primitive_edit(
    lines: &mut Vec<String>,
    prepared: PreparedPrimitiveEdit<'_>,
) -> BufferCursor {
    let from = prepared.edit.from;
    let to = prepared.edit.to;
    debug_assert!(
        lines.is_empty() || lines.get(from.0).map(String::len) == Some(prepared.edit.from_line_len),
        "primitive edit applied to a buffer that changed after preparation"
    );
    match prepared.mutation {
        Mutation::InsertChar(ch) => {
            if lines.is_empty() {
                lines.push(String::new());
            }
            lines[from.0].insert(from.1, ch);
        }
        Mutation::InsertText(text) => {
            if lines.is_empty() {
                lines.push(String::new());
            }
            lines[from.0].insert_str(from.1, text);
        }
        Mutation::Split => {
            if lines.is_empty() {
                lines.push(String::new());
            }
            let right = lines[from.0][from.1..].to_string();
            lines[from.0].truncate(from.1);
            lines.insert(from.0 + 1, right);
        }
        Mutation::RemoveChar => {
            if from.1 < to.1 {
                lines[from.0].replace_range(from.1..to.1, "");
            }
        }
        Mutation::Join => {
            let removed = lines.remove(to.0);
            lines[from.0].push_str(&removed);
        }
    }
    prepared.cursor
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &mut Vec<String>, cursor: BufferCursor, edit: PrimitiveEdit<'_>) -> BufferCursor {
        let plan = prepare_primitive_edit(lines, cursor, edit).expect("edit");
        apply_primitive_edit(lines, plan)
    }

    #[test]
    fn unicode_insertion_and_deletion_use_character_cursor_and_byte_metadata() {
        let mut lines = vec!["é β".into()];
        let cursor = BufferCursor { line: 0, column: 1 };
        let plan = prepare_primitive_edit(&lines, cursor, PrimitiveEdit::InsertChar('λ')).unwrap();
        assert_eq!(plan.edit.from, (0, 2));
        assert_eq!(plan.edit.from_line_len, 5);
        let cursor = apply_primitive_edit(&mut lines, plan);
        assert_eq!(cursor.column, 2);
        let cursor = run(&mut lines, cursor, PrimitiveEdit::InsertText("猫"));
        assert_eq!(lines[0], "éλ猫 β");
        let plan = prepare_primitive_edit(&lines, cursor, PrimitiveEdit::Backspace).unwrap();
        assert_eq!(plan.edit.from, (0, 4));
        assert_eq!(plan.edit.to, (0, 7));
        let cursor = apply_primitive_edit(&mut lines, plan);
        let cursor = run(&mut lines, cursor, PrimitiveEdit::DeleteForward);
        assert_eq!(lines[0], "éλβ");
        assert_eq!(cursor.column, 2);
    }

    #[test]
    fn splits_report_exact_pre_edit_coordinates_and_line_spans() {
        for column in [0, 1, 2] {
            let mut lines = vec!["éβ".into(), "end".into()];
            let plan = prepare_primitive_edit(
                &lines,
                BufferCursor { line: 0, column },
                PrimitiveEdit::Newline,
            )
            .unwrap();
            assert_eq!(plan.edit.from, (0, column * 2));
            assert_eq!(plan.edit.from_line_len, 4);
            assert_eq!(plan.edit.to_line_len, 4);
            assert_eq!(plan.edit.inserted_breaks, 1);
            assert_eq!(
                plan.delta,
                EditDelta {
                    start_line: 0,
                    old_span: 1,
                    new_span: 2
                }
            );
            let cursor = apply_primitive_edit(&mut lines, plan);
            assert_eq!(cursor, BufferCursor { line: 1, column: 0 });
            assert_eq!(lines[0].chars().count(), column);
            assert_eq!(lines[1].chars().count(), 2 - column);
            assert_eq!(lines[2], "end");
        }
    }

    #[test]
    fn joins_preserve_character_cursor_and_both_old_line_lengths() {
        for (cursor, action) in [
            (
                BufferCursor { line: 1, column: 0 },
                PrimitiveEdit::Backspace,
            ),
            (
                BufferCursor { line: 0, column: 1 },
                PrimitiveEdit::DeleteForward,
            ),
        ] {
            let mut lines = vec!["é".into(), "β".into()];
            let plan = prepare_primitive_edit(&lines, cursor, action).unwrap();
            assert_eq!(
                plan.edit,
                ExactTextEdit {
                    from: (0, 2),
                    to: (1, 0),
                    from_line_len: 2,
                    to_line_len: 2,
                    inserted_breaks: 0
                }
            );
            assert_eq!(
                plan.delta,
                EditDelta {
                    start_line: 0,
                    old_span: 2,
                    new_span: 1
                }
            );
            assert_eq!(
                apply_primitive_edit(&mut lines, plan),
                BufferCursor { line: 0, column: 1 }
            );
            assert_eq!(lines, vec!["éβ"]);
        }
    }

    #[test]
    fn boundaries_and_ignored_insertions_leave_the_buffer_untouched() {
        let lines = vec![String::new()];
        for edit in [
            PrimitiveEdit::InsertChar('\n'),
            PrimitiveEdit::InsertText(""),
            PrimitiveEdit::Backspace,
            PrimitiveEdit::DeleteForward,
        ] {
            assert!(
                prepare_primitive_edit(&lines, BufferCursor { line: 0, column: 0 }, edit).is_none()
            );
        }
        let mut lines = Vec::new();
        let cursor = run(
            &mut lines,
            BufferCursor { line: 0, column: 0 },
            PrimitiveEdit::InsertChar('é'),
        );
        assert_eq!(lines, vec!["é"]);
        assert_eq!(cursor.column, 1);
    }

    #[test]
    fn literal_text_and_out_of_range_columns_keep_existing_insertion_behavior() {
        let mut lines = vec!["é".into()];
        let cursor = run(
            &mut lines,
            BufferCursor { line: 0, column: 3 },
            PrimitiveEdit::InsertText("a\nb"),
        );
        assert_eq!(lines, vec!["éa\nb"]);
        assert_eq!(cursor.column, 6);
    }

    #[test]
    fn newline_initializes_an_empty_buffer() {
        let mut lines = Vec::new();
        let plan = prepare_primitive_edit(
            &lines,
            BufferCursor { line: 0, column: 0 },
            PrimitiveEdit::Newline,
        )
        .unwrap();
        assert_eq!(
            plan.delta,
            EditDelta {
                start_line: 0,
                old_span: 0,
                new_span: 2
            }
        );
        let cursor = apply_primitive_edit(&mut lines, plan);
        assert_eq!(lines, ["", ""]);
        assert_eq!(cursor, BufferCursor { line: 1, column: 0 });
    }
}
