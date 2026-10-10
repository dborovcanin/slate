//! Paste semantics. Hosts record exact line edits or block mappings before
//! mutation and retain their existing history/calc transaction boundaries.
use super::primitives::{byte_index, BufferCursor};
use super::{EditDelta, ExactTextEdit};
use crate::markdown_tokens::{advance_fence_state, FenceState};
use crate::table::{self, TableBlockEdit, TableFormatCache};

pub fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Retains the original paste allocations: split parts and one current-line
/// clone. Preparation leaves the buffer unchanged; parts borrow normalized text.
pub struct PreparedPaste<'a> {
    pub edit: ExactTextEdit,
    pub delta: EditDelta,
    pub cursor: BufferCursor,
    parts: Vec<&'a str>,
    current: String,
}

/// Prepare plain paste after normalization and table-cell handling. Cursor lines
/// clamp to the last line, including line zero when the buffer is empty.
pub fn prepare_plain_paste<'a>(
    lines: &[String],
    cursor: BufferCursor,
    normalized: &'a str,
) -> Option<PreparedPaste<'a>> {
    if normalized.is_empty() {
        return None;
    }
    let parts: Vec<&str> = normalized.split('\n').collect();
    let line = cursor.line.min(lines.len().saturating_sub(1));
    let current = lines.get(line).cloned().unwrap_or_default();
    let split = byte_index(&current, cursor.column);
    let after = if parts.len() == 1 {
        BufferCursor {
            line,
            column: cursor.column + parts[0].chars().count(),
        }
    } else {
        BufferCursor {
            line: line + parts.len() - 1,
            column: parts.last().unwrap().chars().count(),
        }
    };
    Some(PreparedPaste {
        edit: ExactTextEdit {
            from: (line, split),
            to: (line, split),
            from_line_len: current.len(),
            to_line_len: current.len(),
            inserted_breaks: parts.len() - 1,
        },
        delta: EditDelta {
            start_line: line,
            old_span: 1,
            new_span: parts.len(),
        },
        cursor: after,
        parts,
        current,
    })
}

/// Apply to the unchanged buffer used for preparation, after host metadata is
/// recorded. Preserve the existing per-line insertion path and cursor placement.
pub fn apply_plain_paste(lines: &mut Vec<String>, prepared: PreparedPaste<'_>) -> BufferCursor {
    if lines.is_empty() {
        lines.push(String::new());
    }
    let line = prepared.delta.start_line;
    let (left, right) = prepared.current.split_at(prepared.edit.from.1);
    if prepared.parts.len() == 1 {
        lines[line] = format!("{left}{}{right}", prepared.parts[0]);
    } else {
        lines[line] = format!("{left}{}", prepared.parts[0]);
        let mut insert_at = line + 1;
        for part in &prepared.parts[1..prepared.parts.len() - 1] {
            lines.insert(insert_at, (*part).to_string());
            insert_at += 1;
        }
        let tail = *prepared.parts.last().unwrap_or(&"");
        lines.insert(insert_at, format!("{tail}{right}"));
    }
    prepared.cursor
}

pub fn prepare_table_cell_paste(
    lines: &[String],
    cursor: BufferCursor,
    normalized: &str,
    tables_enabled: bool,
    cache: &mut TableFormatCache,
) -> Option<TableBlockEdit> {
    if !tables_enabled || lines.is_empty() {
        return None;
    }
    let line = cursor.line.min(lines.len().saturating_sub(1));
    let cursor_byte = byte_index(&lines[line], cursor.column);
    table::plan_table_cell_multiline_paste(lines, line, cursor_byte, normalized, cache)
}

/// The host must record the old/new block mapping before consuming the edit.
pub fn apply_table_cell_paste(lines: &mut Vec<String>, edit: TableBlockEdit) -> BufferCursor {
    lines.splice(edit.start..=edit.end, edit.lines);
    let target = &lines[edit.cursor_line];
    BufferCursor {
        line: edit.cursor_line,
        column: target[..edit.cursor_byte.min(target.len())].chars().count(),
    }
}

/// Parse a table only when table paste is enabled and the current line is not
/// already a table row. Fence lookup stays lazy in the host until parsing works.
pub fn parse_table_paste(
    text: &str,
    current_line: &str,
    tables_enabled: bool,
) -> Option<Vec<String>> {
    if !tables_enabled || table::is_table_line(current_line) {
        return None;
    }
    crate::table_import::delimited_text_to_table(text)
}

/// Opening and closing fence lines count as code for table insertion.
pub fn table_paste_outside_code(current_line: &str, mut fence_before: FenceState) -> bool {
    let before = fence_before.in_code_block;
    advance_fence_state(&mut fence_before, current_line);
    !before && !fence_before.in_code_block
}

/// Prepare a parsed table's text and insertion cursor: replace a blank line or
/// insert below a nonblank line, retaining its existing text.
pub fn prepare_table_import(
    table: &[String],
    current_line: &str,
    mut cursor: BufferCursor,
) -> (String, BufferCursor) {
    let mut block = table.join("\n");
    if !current_line.trim().is_empty() {
        cursor.column = current_line.chars().count();
        block.insert(0, '\n');
    }
    (block, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paste_matches_text_insertion_at_unicode_boundaries() {
        let original = "é task";
        for column in 0..=original.chars().count() {
            let offset = byte_index(original, column);
            for raw in ["", "猫", "A\r\nB\r", "\n", "猫\nβ", "\r\n"] {
                let mut lines = vec![original.into(), "last".into()];
                let normalized = normalize_paste(raw);
                let Some(plan) =
                    prepare_plain_paste(&lines, BufferCursor { line: 0, column }, &normalized)
                else {
                    assert_eq!(raw, "");
                    continue;
                };
                assert_eq!(lines, vec![original, "last"]);
                assert_eq!(
                    plan.edit,
                    ExactTextEdit {
                        from: (0, offset),
                        to: (0, offset),
                        from_line_len: original.len(),
                        to_line_len: original.len(),
                        inserted_breaks: normalized.matches('\n').count(),
                    }
                );
                assert_eq!(
                    plan.delta,
                    EditDelta {
                        start_line: 0,
                        old_span: 1,
                        new_span: normalized.split('\n').count()
                    }
                );
                let expected_cursor = if normalized.contains('\n') {
                    BufferCursor {
                        line: normalized.matches('\n').count(),
                        column: normalized.rsplit('\n').next().unwrap().chars().count(),
                    }
                } else {
                    BufferCursor {
                        line: 0,
                        column: column + normalized.chars().count(),
                    }
                };
                assert_eq!(apply_plain_paste(&mut lines, plan), expected_cursor);
                assert_eq!(
                    lines.join("\n"),
                    format!(
                        "{}{normalized}{}\nlast",
                        &original[..offset],
                        &original[offset..]
                    )
                );
            }
        }
    }

    #[test]
    fn paste_clamps_line_and_initializes_an_empty_buffer() {
        let mut lines = Vec::new();
        let plan = prepare_plain_paste(
            &lines,
            BufferCursor {
                line: 99,
                column: 0,
            },
            "\n猫",
        )
        .unwrap();
        assert_eq!(plan.edit.from, (0, 0));
        assert_eq!(
            apply_plain_paste(&mut lines, plan),
            BufferCursor { line: 1, column: 1 }
        );
        assert_eq!(lines, vec!["", "猫"]);
        let plan = prepare_plain_paste(
            &lines,
            BufferCursor {
                line: 99,
                column: 3,
            },
            "β",
        )
        .unwrap();
        assert_eq!(
            apply_plain_paste(&mut lines, plan),
            BufferCursor { line: 1, column: 4 }
        );
        assert_eq!(lines, vec!["", "猫β"]);
    }

    #[test]
    fn table_cell_paste_applies_the_core_block_and_unicode_cursor() {
        let mut lines = table::format_table_lines(&[
            "| A | B |".into(),
            "| --- | --- |".into(),
            "| 猫β | z |".into(),
        ]);
        let column = lines[2][..lines[2].find('β').unwrap()].chars().count();
        let mut cache = TableFormatCache::default();
        assert!(prepare_table_cell_paste(
            &lines,
            BufferCursor { line: 2, column },
            "1\n2",
            false,
            &mut cache
        )
        .is_none());
        let plan = prepare_table_cell_paste(
            &lines,
            BufferCursor { line: 2, column },
            "1\n2",
            true,
            &mut cache,
        )
        .unwrap();
        let expected = plan.lines.clone();
        let expected_column = expected[3][..plan.cursor_byte].chars().count();
        let cursor = apply_table_cell_paste(&mut lines, plan);
        assert_eq!(lines, expected);
        assert_eq!(
            cursor,
            BufferCursor {
                line: 3,
                column: expected_column
            }
        );
        assert_eq!(table::split_table_cells(&lines[2])[0], "猫1");
        assert_eq!(table::split_table_cells(&lines[3])[0], "2β");
    }

    #[test]
    fn imported_tables_keep_module_row_and_fence_guards() {
        let text = "a,b\n1,2";
        assert!(parse_table_paste(text, "", true).is_some());
        assert!(parse_table_paste(text, "", false).is_none());
        assert!(parse_table_paste(text, "| a | b |", true).is_none());
        assert!(parse_table_paste("Hello, world\nBye, moon", "", true).is_none());
        assert!(table_paste_outside_code("plain", FenceState::default()));
        assert!(!table_paste_outside_code("```csv", FenceState::default()));
        let inside = FenceState {
            in_code_block: true,
            ..Default::default()
        };
        assert!(!table_paste_outside_code("plain", inside.clone()));
        assert!(!table_paste_outside_code("```", inside));
    }

    #[test]
    fn imported_table_placement_uses_character_columns_and_preserves_blank_line_cursor() {
        let table = parse_table_paste("a,b\n1,2", "", true).unwrap();
        let (block, cursor) =
            prepare_table_import(&table, "é text", BufferCursor { line: 3, column: 1 });
        assert_eq!(block, format!("\n{}", table.join("\n")));
        assert_eq!(cursor, BufferCursor { line: 3, column: 6 });
        let (block, cursor) =
            prepare_table_import(&table, " ", BufferCursor { line: 3, column: 1 });
        assert_eq!(block, table.join("\n"));
        assert_eq!(cursor, BufferCursor { line: 3, column: 1 });
    }
}
