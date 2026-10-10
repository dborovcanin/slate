//! Word motions and backward deletion. Columns are character offsets; the host
//! supplies visible neighbors lazily, in motion order, without copying a note.
use super::primitives::{byte_index, BufferCursor};
use super::{EditDelta, ExactTextEdit};
use crate::table;

pub fn move_cursor_left_word(
    lines: &[String],
    cursor: BufferCursor,
    tables: bool,
    neighbors: impl Iterator<Item = usize>,
) -> BufferCursor {
    move_word(lines, cursor, tables, false, neighbors)
}

pub fn move_cursor_right_word(
    lines: &[String],
    cursor: BufferCursor,
    tables: bool,
    neighbors: impl Iterator<Item = usize>,
) -> BufferCursor {
    move_word(lines, cursor, tables, true, neighbors)
}

fn move_word(
    lines: &[String],
    cursor: BufferCursor,
    tables: bool,
    forward: bool,
    mut neighbors: impl Iterator<Item = usize>,
) -> BufferCursor {
    let line = lines.get(cursor.line).map(String::as_str).unwrap_or("");
    if tables && table::is_table_line(line) {
        let on_row = if forward {
            table::table_row_next_word_start(line, cursor.column)
        } else {
            table::table_row_prev_word_start(line, cursor.column)
        };
        if let Some(column) = on_row {
            return BufferCursor { column, ..cursor };
        }
        for line_idx in neighbors {
            let Some(line) = lines.get(line_idx) else {
                return cursor;
            };
            if !table::is_table_line(line) {
                return BufferCursor {
                    line: line_idx,
                    column: if forward { 0 } else { line.chars().count() },
                };
            }
            if table::is_delimiter_line_in(lines, line_idx) {
                continue;
            }
            let column = if forward {
                table::table_row_next_word_start(line, 0)
            } else {
                table::table_row_prev_word_start(line, line.chars().count())
            };
            // Empty table rows remain valid landing points; the host cursor
            // guard places the cursor in the first cell afterward.
            return BufferCursor {
                line: line_idx,
                column: column.unwrap_or(0),
            };
        }
        return cursor;
    }
    if !forward && cursor.column == 0 {
        return neighbors
            .next()
            .and_then(|idx| {
                lines.get(idx).map(|line| BufferCursor {
                    line: idx,
                    column: line.chars().count(),
                })
            })
            .unwrap_or(cursor);
    }
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    if forward && cursor.column >= len {
        return neighbors
            .next()
            .map(|line| BufferCursor { line, column: 0 })
            .unwrap_or(cursor);
    }
    let mut column = cursor.column.min(len);
    if forward {
        let start_class = chars.get(column).map_or(0, |&ch| word_class(ch));
        while column < len && word_class(chars[column]) == start_class {
            column += 1;
        }
        if start_class != 0 {
            while column < len && chars[column].is_whitespace() {
                column += 1;
            }
        }
    } else {
        if column == 0 {
            return BufferCursor {
                column: 0,
                ..cursor
            };
        }
        column -= 1;
        while column > 0 && chars[column].is_whitespace() {
            column -= 1;
        }
        let target_class = word_class(chars[column]);
        while column > 0 && word_class(chars[column - 1]) == target_class {
            column -= 1;
        }
    }
    BufferCursor { column, ..cursor }
}

fn word_class(ch: char) -> u8 {
    if ch.is_whitespace() {
        0
    } else if ch.is_alphanumeric() || ch == '_' {
        1
    } else {
        2
    }
}

/// A same-line deletion prepared against an unchanged line.
pub struct WordDeleteRange {
    pub edit: ExactTextEdit,
    pub delta: EditDelta,
    column: usize,
    start_byte: usize,
    end_byte: usize,
}

pub enum BackwardWordDelete {
    /// At column zero, retain the ordinary Backspace transaction and metadata
    /// path, joining the previous physical line even when it is folded.
    JoinPreviousLine,
    WithinLine(WordDeleteRange),
}

pub fn prepare_backward_word_delete(
    lines: &[String],
    cursor: BufferCursor,
    tables: bool,
) -> Option<BackwardWordDelete> {
    if cursor.column == 0 {
        return (cursor.line > 0).then_some(BackwardWordDelete::JoinPreviousLine);
    }
    let line = lines.get(cursor.line).map(String::as_str).unwrap_or("");
    let column = if let Some(cell) = tables
        .then(|| {
            if !table::is_table_line(line) {
                return None;
            }
            table::table_cell_info_in_line(line, byte_index(line, cursor.column))
        })
        .flatten()
    {
        // Preserve the current helper's fixed-padding edit bounds.
        let span = table::TableCellSpan {
            left_pipe: cell.left_pipe,
            right_pipe: cell.right_pipe,
            trim_start: cell.trim_start,
            trim_end: cell.trim_end,
        };
        table::table_cell_word_delete_start(
            line,
            cursor.column,
            span.edit_start(),
            line[..span.navigation_anchor()].chars().count(),
        )?
    } else {
        let chars: Vec<char> = line.chars().collect();
        let mut column = cursor.column;
        // Backward deletion intentionally treats underscores as punctuation,
        // unlike the Vim-like word motions above.
        while column > 0 && chars.get(column - 1).is_some_and(|c| !c.is_alphanumeric()) {
            column -= 1;
        }
        while column > 0 && chars.get(column - 1).is_some_and(|c| c.is_alphanumeric()) {
            column -= 1;
        }
        column
    };
    let start_byte = byte_index(line, column);
    let end_byte = byte_index(line, cursor.column);
    Some(BackwardWordDelete::WithinLine(WordDeleteRange {
        edit: ExactTextEdit {
            from: (cursor.line, start_byte),
            to: (cursor.line, end_byte),
            from_line_len: line.len(),
            to_line_len: line.len(),
            inserted_breaks: 0,
        },
        delta: EditDelta {
            start_line: cursor.line,
            old_span: 1,
            new_span: 1,
        },
        column,
        start_byte,
        end_byte,
    }))
}

pub fn apply_word_delete(line: &mut String, range: WordDeleteRange) -> usize {
    line.replace_range(range.start_byte..range.end_byte, "");
    range.column
}

#[cfg(test)]
mod tests;
