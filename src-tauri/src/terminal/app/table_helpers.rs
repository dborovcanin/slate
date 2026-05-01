use crate::terminal::text_utils::byte_index;
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct TableCellInfo {
    pub(super) column_index: usize,
    pub(super) column_count: usize,
    pub(super) logical_row_index: Option<usize>,
    pub(super) logical_row_count: usize,
    pub(super) is_continuation_row: bool,
    pub(super) left_pipe: usize,
    pub(super) right_pipe: usize,
    pub(super) trim_start: usize,
    pub(super) trim_end: usize,
}

#[derive(Debug, Clone)]
struct TableCellInfoCacheEntry {
    line_idx: usize,
    col_char: usize,
    cur_hash: u64,
    prev_hash: u64,
    next_hash: u64,
    info: Option<TableCellInfo>,
}

const TABLE_CELL_INFO_CACHE_CAP: usize = 256;

thread_local! {
    static TABLE_CELL_INFO_CACHE: RefCell<VecDeque<TableCellInfoCacheEntry>> = const { RefCell::new(VecDeque::new()) };
}

fn line_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn is_markdown_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|')
}

pub(super) fn table_cell_info_at_char(
    lines: &[String],
    line_idx: usize,
    col_char: usize,
) -> Option<TableCellInfo> {
    let line = lines.get(line_idx)?;
    if !is_markdown_table_line(line) {
        return None;
    }

    let cur_hash = line_hash(line);
    let prev_hash = line_idx
        .checked_sub(1)
        .and_then(|idx| lines.get(idx))
        .map(|line| line_hash(line))
        .unwrap_or(0);
    let next_hash = lines
        .get(line_idx.saturating_add(1))
        .map(|line| line_hash(line))
        .unwrap_or(0);

    if let Some(cached) = TABLE_CELL_INFO_CACHE.with(|cache| {
        cache
            .borrow()
            .iter()
            .rev()
            .find(|entry| {
                entry.line_idx == line_idx
                    && entry.col_char == col_char
                    && entry.cur_hash == cur_hash
                    && entry.prev_hash == prev_hash
                    && entry.next_hash == next_hash
            })
            .cloned()
    }) {
        return cached.info;
    }

    let col_byte = byte_index(line, col_char);
    let info =
        crate::editor_core::table::table_cell_cursor_info_in_document(lines, line_idx, col_byte)?;
    let resolved = Some(TableCellInfo {
        column_index: info.column_index,
        column_count: info.column_count,
        logical_row_index: info.logical_row_index,
        logical_row_count: info.logical_row_count,
        is_continuation_row: info.is_continuation_row,
        left_pipe: info.left_pipe,
        right_pipe: info.right_pipe,
        trim_start: info.trim_start,
        trim_end: info.trim_end,
    });

    TABLE_CELL_INFO_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.push_back(TableCellInfoCacheEntry {
            line_idx,
            col_char,
            cur_hash,
            prev_hash,
            next_hash,
            info: resolved.clone(),
        });
        while cache.len() > TABLE_CELL_INFO_CACHE_CAP {
            cache.pop_front();
        }
    });

    resolved
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
