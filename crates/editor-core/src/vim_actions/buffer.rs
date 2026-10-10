//! Line-buffer Vim plans. Coordinates are character columns, except exact edit maps.
use super::{VimRegisterMode, VimRegisterValue};
use crate::buffer::primitives::BufferCursor;
use crate::buffer::{EditDelta, ExactTextEdit};

pub struct VisualSelectionPlan {
    pub register: VimRegisterValue,
    pub cursor: BufferCursor,
    pub delta: EditDelta,
    pub text_changed: bool,
    pub exact_edit: Option<ExactTextEdit>,
    pub deleted_lines: Option<(usize, usize)>,
    replacement: Option<Vec<String>>,
}
impl VisualSelectionPlan {
    pub fn apply(self, lines: &mut Vec<String>) -> (VimRegisterValue, BufferCursor) {
        if let Some(replacement) = self.replacement {
            let end = self.delta.start_line + self.delta.old_span;
            lines.splice(self.delta.start_line..end, replacement);
            if lines.is_empty() {
                lines.push(String::new());
            }
        }
        (self.register, self.cursor)
    }
}
/// Prepare only selected lines; no joined document or whole-buffer copy.
pub fn prepare_visual_selection(
    lines: &[String],
    cursor: BufferCursor,
    anchor: BufferCursor,
    linewise: bool,
    delete: bool,
) -> Option<VisualSelectionPlan> {
    if lines.is_empty() {
        return None;
    }
    let start_line = anchor.line.min(cursor.line).min(lines.len() - 1);
    let end_line = anchor.line.max(cursor.line).min(lines.len() - 1);
    let (start_col, end_col) = if anchor.line == cursor.line {
        (
            anchor.column.min(cursor.column),
            anchor.column.max(cursor.column),
        )
    } else if anchor.line < cursor.line {
        (anchor.column, cursor.column)
    } else {
        (cursor.column, anchor.column)
    };
    let mut resulting_cursor = cursor;
    let mut exact_edit = None;
    let mut deleted_lines = None;
    let (yanked, replacement) = if linewise {
        let yanked = lines[start_line..=end_line].join("\n");
        if delete {
            resulting_cursor = BufferCursor {
                line: start_line.min(lines.len().saturating_sub(end_line - start_line + 2)),
                column: 0,
            };
            deleted_lines = Some((start_line, end_line));
        }
        (yanked, delete.then(Vec::new))
    } else {
        let first = &lines[start_line];
        let last = &lines[end_line];
        let first_chars: Vec<char> = first.chars().collect();
        let last_chars_storage;
        let last_chars = if start_line == end_line {
            &first_chars
        } else {
            last_chars_storage = last.chars().collect::<Vec<_>>();
            &last_chars_storage
        };
        let from = start_col.min(first_chars.len());
        let to = end_col.saturating_add(1).min(last_chars.len());
        let mut selected = Vec::new();
        if start_line == end_line {
            selected.push(first_chars[from..to].iter().collect::<String>());
        } else {
            selected.push(first_chars[from..].iter().collect());
            selected.extend(lines[start_line + 1..end_line].iter().cloned());
            selected.push(last_chars[..to].iter().collect());
        }
        let replacement = if delete {
            let mut line: String = first_chars[..from].iter().collect();
            line.extend(last_chars[to..].iter());
            resulting_cursor.column = from;
            if start_line != end_line {
                resulting_cursor.line = start_line;
                exact_edit = Some(ExactTextEdit {
                    from: (
                        start_line,
                        first_chars[..from].iter().map(|c| c.len_utf8()).sum(),
                    ),
                    to: (
                        end_line,
                        last_chars[..to].iter().map(|c| c.len_utf8()).sum(),
                    ),
                    from_line_len: first.len(),
                    to_line_len: last.len(),
                    inserted_breaks: 0,
                });
            }
            Some(vec![line])
        } else {
            None
        };
        (selected.join("\n"), replacement)
    };
    let text_changed = delete && (!yanked.is_empty() || (linewise && lines.len() > 1));
    Some(VisualSelectionPlan {
        register: VimRegisterValue {
            text: yanked,
            mode: if linewise {
                VimRegisterMode::Linewise
            } else {
                VimRegisterMode::Charwise
            },
        },
        cursor: resulting_cursor,
        delta: EditDelta {
            start_line,
            old_span: end_line - start_line + 1,
            new_span: if !delete {
                end_line - start_line + 1
            } else if linewise {
                usize::from(end_line - start_line + 1 == lines.len())
            } else {
                1
            },
        },
        text_changed,
        exact_edit,
        deleted_lines,
        replacement,
    })
}

/// Cursor placement for insert-entry and line-boundary intents.
pub fn insert_entry_column(column: usize, line_len: usize, intent: crate::vim::VimIntent) -> usize {
    use crate::vim::VimIntent::*;
    match intent {
        AppendInsert => {
            if column < line_len {
                column + 1
            } else {
                column
            }
        }
        InsertLineStart | OpenLineAbove | MoveLineStart => 0,
        AppendLineEnd | OpenLineBelow => line_len,
        MoveLineEnd => line_len.saturating_sub(1),
        _ => column,
    }
}

/// Repeated linewise register contents, with Vim's single trailing newline removed.
pub fn linewise_paste_lines(text: &str, count: usize) -> Vec<String> {
    let normalized = text.strip_suffix('\n').unwrap_or(text);
    let lines: Vec<String> = normalized.split('\n').map(str::to_string).collect();
    let mut repeated = Vec::with_capacity(lines.len() * count);
    for _ in 0..count {
        repeated.extend(lines.iter().cloned());
    }
    repeated
}

/// Prose horizontal movement; the host resolves visible neighbours and formatting exits.
pub fn horizontal_motion(
    lines: &[String],
    cursor: BufferCursor,
    forward: bool,
    neighbour: Option<usize>,
) -> BufferCursor {
    if forward {
        let len = lines
            .get(cursor.line)
            .map_or(0, |line| line.chars().count());
        if cursor.column < len {
            BufferCursor {
                column: cursor.column + 1,
                ..cursor
            }
        } else {
            neighbour.map_or(cursor, |line| BufferCursor { line, column: 0 })
        }
    } else if cursor.column > 0 {
        BufferCursor {
            column: cursor.column - 1,
            ..cursor
        }
    } else {
        neighbour.map_or(cursor, |line| BufferCursor {
            line,
            column: lines.get(line).map_or(0, |s| s.chars().count()),
        })
    }
}

pub fn insert_empty_line_above(lines: &mut Vec<String>, line: usize) {
    lines.insert(line, String::new());
}

/// Paste-after insertion starts after the current character, clamped at line end.
pub fn paste_after_column(column: usize, line_len: usize) -> usize {
    if column < line_len {
        column + 1
    } else {
        line_len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cursor(line: usize, column: usize) -> BufferCursor {
        BufferCursor { line, column }
    }
    #[test]
    fn reversed_unicode_visual_span_preserves_boundaries() {
        let mut lines = vec!["é🙂abc".into(), "tail🙂".into()];
        let plan =
            prepare_visual_selection(&lines, cursor(0, 1), cursor(1, 1), false, true).unwrap();
        assert_eq!(plan.register.text, "🙂abc\nta");
        assert_eq!(plan.exact_edit.unwrap().from, (0, 2));
        let (_, pos) = plan.apply(&mut lines);
        assert_eq!(lines, ["éil🙂"]);
        assert_eq!(pos, cursor(0, 1));
    }
    #[test]
    fn whole_document_linewise_delete_retains_empty_line() {
        let mut lines = vec!["one".into(), "two".into()];
        let plan =
            prepare_visual_selection(&lines, cursor(1, 0), cursor(0, 0), true, true).unwrap();
        assert_eq!(plan.deleted_lines, Some((0, 1)));
        assert_eq!(
            plan.delta,
            EditDelta {
                start_line: 0,
                old_span: 2,
                new_span: 1
            }
        );
        let (register, pos) = plan.apply(&mut lines);
        assert_eq!(register.text, "one\ntwo");
        assert_eq!(lines, [""]);
        assert_eq!(pos, cursor(0, 0));
    }
    #[test]
    fn yank_keeps_buffer_and_cursor() {
        let mut lines = vec!["é🙂abc".into()];
        let plan =
            prepare_visual_selection(&lines, cursor(0, 2), cursor(0, 0), false, false).unwrap();
        assert_eq!(plan.delta.old_span, plan.delta.new_span);
        let (register, pos) = plan.apply(&mut lines);
        assert_eq!(register.text, "é🙂a");
        assert_eq!(lines, ["é🙂abc"]);
        assert_eq!(pos, cursor(0, 2));
    }
    #[test]
    fn horizontal_motion_uses_host_visible_neighbour() {
        let lines = vec!["é🙂".into(), "hidden".into(), "tail".into()];
        assert_eq!(
            horizontal_motion(&lines, cursor(0, 2), true, Some(2)),
            cursor(2, 0)
        );
        assert_eq!(
            horizontal_motion(&lines, cursor(2, 0), false, Some(0)),
            cursor(0, 2)
        );
        assert_eq!(
            horizontal_motion(&lines, cursor(0, 0), false, None),
            cursor(0, 0)
        );
    }
    #[test]
    fn linewise_register_repetition_removes_one_trailing_newline() {
        assert_eq!(linewise_paste_lines("é\n🙂\n", 2), ["é", "🙂", "é", "🙂"]);
        assert_eq!(linewise_paste_lines("\n", 2), ["", ""]);
    }
}

#[cfg(test)]
mod visual_metadata_tests {
    use super::*;
    #[test]
    fn empty_visual_delete_is_not_a_text_change_and_multiline_yank_has_no_delta() {
        let lines = vec![String::new()];
        let cursor = BufferCursor { line: 0, column: 0 };
        for linewise in [false, true] {
            let plan = prepare_visual_selection(&lines, cursor, cursor, linewise, true).unwrap();
            assert!(!plan.text_changed);
            assert_eq!(plan.delta.old_span, plan.delta.new_span);
        }
        let lines = vec!["one".into(), "two".into()];
        let plan = prepare_visual_selection(
            &lines,
            BufferCursor { line: 1, column: 1 },
            cursor,
            true,
            false,
        )
        .unwrap();
        assert!(!plan.text_changed);
        assert_eq!(plan.delta.old_span, plan.delta.new_span);
    }
}
