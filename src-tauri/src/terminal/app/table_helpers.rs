use crate::terminal::text_utils::byte_index;

pub(super) struct TableCellInfo {
    pub(super) left_pipe: usize,
    pub(super) right_pipe: usize,
    pub(super) trim_start: usize,
    pub(super) trim_end: usize,
}

pub(super) fn is_markdown_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|')
}

pub(super) fn table_cell_info_at_char(line: &str, col_char: usize) -> Option<TableCellInfo> {
    if !is_markdown_table_line(line) {
        return None;
    }

    let col_byte = byte_index(line, col_char);
    let pipes = crate::editor_core::table::table_pipe_positions(line);
    if pipes.len() < 2 {
        return None;
    }
    let cell_index = crate::editor_core::table::table_cell_index_for_column(&pipes, col_byte)?;
    let span = crate::editor_core::table::table_cell_span(line, &pipes, cell_index)?;
    Some(TableCellInfo {
        left_pipe: span.left_pipe,
        right_pipe: span.right_pipe,
        trim_start: span.trim_start,
        trim_end: span.trim_end,
    })
}

pub(super) fn table_cell_is_empty(cell: &TableCellInfo) -> bool {
    cell.trim_end <= cell.trim_start
}

pub(super) fn table_cell_edit_start(cell: &TableCellInfo) -> usize {
    (cell.left_pipe + 2).min(cell.right_pipe)
}

pub(super) fn table_cell_navigation_anchor(line: &str, cell: &TableCellInfo) -> usize {
    let edit_start = table_cell_edit_start(cell);
    let anchor_byte = if table_cell_is_empty(cell) {
        edit_start
    } else {
        ((cell.left_pipe + 1) + cell.trim_end).min(cell.right_pipe)
    };
    line[..anchor_byte].chars().count()
}

#[derive(Debug, Clone)]
pub(super) struct TableFormulaSegment {
    pub(super) from_byte: usize,
    pub(super) to_byte: usize,
    pub(super) from_char: usize,
    pub(super) to_char: usize,
    pub(super) cell_from_char: usize,
    pub(super) cell_to_char: usize,
    pub(super) cell_index: usize,
    #[cfg(test)]
    pub(super) labels: Vec<String>,
}

#[cfg(test)]
pub(super) fn builtin_formula_label(text: &str) -> Option<String> {
    crate::editor_core::calc_plan::builtin_formula_label(text)
}

#[cfg(test)]
pub(super) fn find_table_formula_segment(text: &str) -> Option<TableFormulaSegment> {
    find_table_formula_segments(text).into_iter().next()
}

pub(super) fn find_table_formula_segments(text: &str) -> Vec<TableFormulaSegment> {
    crate::editor_core::calc_plan::find_table_formula_segments(text)
        .into_iter()
        .map(|seg| TableFormulaSegment {
            from_byte: seg.from_byte,
            to_byte: seg.to_byte,
            from_char: seg.from_char,
            to_char: seg.to_char,
            cell_from_char: seg.cell_left_pipe_char + 1,
            cell_to_char: seg.cell_right_pipe_char,
            cell_index: seg.cell_index,
            #[cfg(test)]
            labels: seg.labels,
        })
        .collect()
}

pub(super) fn formula_marker_token(index: usize) -> String {
    "*".repeat(index + 1)
}

#[cfg(test)]
pub(super) fn should_mask_formula_cell(
    is_cursor_line: bool,
    cursor_col: usize,
    formula: &TableFormulaSegment,
) -> bool {
    !(is_cursor_line && cursor_col >= formula.cell_from_char && cursor_col < formula.cell_to_char)
}

pub(super) fn format_formula_display_value(raw: &str) -> String {
    crate::editor_core::calc_plan::format_formula_display_value(raw)
}
