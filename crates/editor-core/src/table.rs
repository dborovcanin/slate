use std::hash::{Hash, Hasher};

use rustc_hash::{FxHashMap, FxHasher};

const TABLE_PARSE_CACHE_MAX_ENTRIES: usize = 512;

#[derive(Debug, Clone, Default)]
pub struct TableFormatCache {
    row_cells: Vec<Vec<String>>,
    delimiter_flags: Vec<bool>,
    widths: Vec<usize>,
    parsed_rows: FxHashMap<String, CachedTableRowParse>,
    parsed_rows_hits: usize,
    parsed_rows_misses: usize,
}

#[derive(Debug, Clone, Default)]
struct CachedTableRowParse {
    pipes: Vec<usize>,
    cells: Vec<String>,
}

impl TableFormatCache {
    fn cached_row_parse(&mut self, line: &str) -> &CachedTableRowParse {
        if self.parsed_rows.len() >= TABLE_PARSE_CACHE_MAX_ENTRIES {
            self.parsed_rows.clear();
        }
        match self.parsed_rows.entry(line.to_string()) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                self.parsed_rows_hits = self.parsed_rows_hits.saturating_add(1);
                entry.into_mut()
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                self.parsed_rows_misses = self.parsed_rows_misses.saturating_add(1);
                entry.insert(CachedTableRowParse {
                    pipes: table_pipe_positions(line),
                    cells: split_row_cells_raw(line)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|cell| cell.trim().to_string())
                        .collect(),
                })
            }
        }
    }

    pub fn invalidate_parsed_rows(&mut self) {
        self.parsed_rows.clear();
    }

    pub fn parsed_row_cache_stats(&self) -> (usize, usize) {
        (self.parsed_rows_hits, self.parsed_rows_misses)
    }

    pub fn parsed_row_cache_size(&self) -> usize {
        self.parsed_rows.len()
    }

    pub fn reset_parsed_row_cache_stats(&mut self) {
        self.parsed_rows_hits = 0;
        self.parsed_rows_misses = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAlign {
    Left,
    Center,
    Right,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableCellSpan {
    pub left_pipe: usize,
    pub right_pipe: usize,
    pub trim_start: usize,
    pub trim_end: usize,
}

impl TableCellSpan {
    pub fn is_empty(&self) -> bool {
        self.trim_end <= self.trim_start
    }

    pub fn edit_start(&self) -> usize {
        (self.left_pipe + 2).min(self.right_pipe)
    }

    pub fn navigation_anchor(&self) -> usize {
        if self.is_empty() {
            self.edit_start()
        } else {
            ((self.left_pipe + 1) + self.trim_end).min(self.right_pipe)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct TableCursorCellInfo {
    pub column_index: usize,
    pub column_count: usize,
    pub left_pipe: usize,
    pub right_pipe: usize,
    pub trim_start: usize,
    pub trim_end: usize,
    pub logical_row_index: Option<usize>,
    pub logical_row_count: usize,
    pub is_continuation_row: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableCursorMotionDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct TableCursorMotionTarget {
    pub line_index: isize,
    pub col: usize,
}

#[derive(Debug, Clone, Default)]
pub struct TableLogicalRowCache {
    block_start: usize,
    block_end: usize,
    logical_row_count: usize,
    delimiter_row: Option<usize>,
    line_hashes: Vec<u64>,
    logical_row_index_by_line: Vec<Option<usize>>,
}

impl TableLogicalRowCache {
    pub fn invalidate(&mut self) {
        self.block_start = 0;
        self.block_end = 0;
        self.logical_row_count = 0;
        self.delimiter_row = None;
        self.line_hashes.clear();
        self.logical_row_index_by_line.clear();
    }

    fn block_contains_line(&self, line_idx: usize) -> bool {
        !self.line_hashes.is_empty() && line_idx >= self.block_start && line_idx <= self.block_end
    }
}

impl TableCursorCellInfo {
    pub fn is_empty(&self) -> bool {
        self.trim_end <= self.trim_start
    }

    pub fn edit_start(&self) -> usize {
        (self.left_pipe + 1 + self.trim_start).min(self.right_pipe)
    }

    pub fn navigation_anchor(&self) -> usize {
        if self.is_empty() {
            self.edit_start()
        } else {
            ((self.left_pipe + 1) + self.trim_end).min(self.right_pipe)
        }
    }
}

fn table_cursor_motion_edit_start(cell: &TableCursorCellInfo) -> usize {
    (cell.left_pipe + 2).min(cell.right_pipe)
}

fn table_cursor_motion_navigation_anchor(cell: &TableCursorCellInfo) -> usize {
    if cell.is_empty() {
        table_cursor_motion_edit_start(cell)
    } else {
        ((cell.left_pipe + 1) + cell.trim_end).min(cell.right_pipe)
    }
}

fn table_cursor_motion_target_for_cell(
    lines: &[String],
    line_index: usize,
    cell_index: usize,
    at_edit_start: bool,
) -> Option<TableCursorMotionTarget> {
    let line = lines.get(line_index)?;
    let pipes = table_pipe_positions(line);
    if pipes.len() < 2 || cell_index + 1 >= pipes.len() {
        return None;
    }
    let probe_col = pipes[cell_index].saturating_add(1).min(line.len());
    let info = table_cell_cursor_info_in_document(lines, line_index, probe_col)?;
    let col = if at_edit_start {
        table_cursor_motion_edit_start(&info)
    } else {
        table_cursor_motion_navigation_anchor(&info)
    };
    Some(TableCursorMotionTarget {
        line_index: line_index as isize,
        col,
    })
}

pub fn plan_table_cursor_motion(
    lines: &[String],
    line_index: usize,
    col: usize,
    direction: TableCursorMotionDirection,
) -> Option<TableCursorMotionTarget> {
    let line = lines.get(line_index)?;
    if !is_table_line(line) {
        return None;
    }
    let current = table_cell_cursor_info_in_document(lines, line_index, col)?;
    let col = col.min(line.len());
    let edit_start = table_cursor_motion_edit_start(&current);
    let content_end = table_cursor_motion_navigation_anchor(&current);

    match direction {
        TableCursorMotionDirection::Left | TableCursorMotionDirection::Right
            if current.is_empty() || col > content_end =>
        {
            return Some(TableCursorMotionTarget {
                line_index: line_index as isize,
                col: content_end,
            });
        }
        TableCursorMotionDirection::Left => {
            if col > edit_start {
                return Some(TableCursorMotionTarget {
                    line_index: line_index as isize,
                    col: col.saturating_sub(1),
                });
            }
            if current.column_index > 0 {
                return table_cursor_motion_target_for_cell(
                    lines,
                    line_index,
                    current.column_index - 1,
                    false,
                );
            }
            for target_line in (0..line_index).rev() {
                let target = lines.get(target_line)?;
                if is_delimiter_line_in(lines, target_line) {
                    continue;
                }
                let pipes = table_pipe_positions(target);
                if pipes.len() < 2 {
                    continue;
                }
                return table_cursor_motion_target_for_cell(
                    lines,
                    target_line,
                    pipes.len().saturating_sub(2),
                    false,
                );
            }
            Some(TableCursorMotionTarget {
                line_index: -1,
                col: 0,
            })
        }
        TableCursorMotionDirection::Right => {
            if col < edit_start {
                return Some(TableCursorMotionTarget {
                    line_index: line_index as isize,
                    col: edit_start,
                });
            }
            if col < content_end {
                return Some(TableCursorMotionTarget {
                    line_index: line_index as isize,
                    col: col.saturating_add(1),
                });
            }
            if current.column_index + 1 < current.column_count {
                return table_cursor_motion_target_for_cell(
                    lines,
                    line_index,
                    current.column_index + 1,
                    true,
                );
            }
            for target_line in (line_index + 1)..lines.len() {
                if is_delimiter_line_in(lines, target_line) {
                    continue;
                }
                return table_cursor_motion_target_for_cell(lines, target_line, 0, true);
            }
            Some(TableCursorMotionTarget {
                line_index: lines.len() as isize,
                col: 0,
            })
        }
        TableCursorMotionDirection::Up => {
            for target_line in (0..line_index).rev() {
                if is_delimiter_line_in(lines, target_line) {
                    continue;
                }
                if let Some(target) = table_cursor_motion_target_for_cell(
                    lines,
                    target_line,
                    current.column_index,
                    false,
                ) {
                    return Some(target);
                }
            }
            Some(TableCursorMotionTarget {
                line_index: -1,
                col: 0,
            })
        }
        TableCursorMotionDirection::Down => {
            for target_line in (line_index + 1)..lines.len() {
                if is_delimiter_line_in(lines, target_line) {
                    continue;
                }
                if let Some(target) = table_cursor_motion_target_for_cell(
                    lines,
                    target_line,
                    current.column_index,
                    false,
                ) {
                    return Some(target);
                }
            }
            Some(TableCursorMotionTarget {
                line_index: lines.len() as isize,
                col: 0,
            })
        }
    }
}

pub fn is_table_line(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|')
}

pub fn is_table_continuation_line(text: &str) -> bool {
    text.trim_start().starts_with("|>")
}

fn pipe_is_escaped(bytes: &[u8], pipe_idx: usize) -> bool {
    if pipe_idx == 0 {
        return false;
    }
    let mut slash_count = 0usize;
    let mut idx = pipe_idx;
    while idx > 0 {
        idx -= 1;
        if bytes[idx] != b'\\' {
            break;
        }
        slash_count += 1;
    }
    slash_count % 2 == 1
}

pub fn table_pipe_positions(line: &str) -> Vec<usize> {
    let bytes = line.as_bytes();
    let mut pipes = Vec::new();
    for (idx, byte) in bytes.iter().enumerate() {
        if *byte == b'|' && !pipe_is_escaped(bytes, idx) {
            pipes.push(idx);
        }
    }
    pipes
}

pub fn table_cell_index_for_column(pipes: &[usize], col: usize) -> Option<usize> {
    if pipes.len() < 2 {
        return None;
    }
    for i in 0..pipes.len() - 1 {
        if col <= pipes[i + 1] {
            return Some(i);
        }
    }
    Some(pipes.len().saturating_sub(2))
}

pub fn first_non_space_offset(text: &str) -> usize {
    text.as_bytes()
        .iter()
        .position(|b| *b != b' ')
        .unwrap_or(text.len())
}

pub fn last_non_space_end_offset(text: &str) -> usize {
    text.as_bytes()
        .iter()
        .rposition(|b| *b != b' ')
        .map(|idx| idx + 1)
        .unwrap_or(0)
}

pub fn table_cell_span(
    line: &str,
    pipes: &[usize],
    left_pipe_index: usize,
) -> Option<TableCellSpan> {
    let &left_pipe = pipes.get(left_pipe_index)?;
    let &right_pipe = pipes.get(left_pipe_index + 1)?;
    if right_pipe <= left_pipe {
        return None;
    }
    let cell_start = left_pipe + 1;
    let raw = &line[cell_start..right_pipe];
    Some(TableCellSpan {
        left_pipe,
        right_pipe,
        trim_start: first_non_space_offset(raw),
        trim_end: last_non_space_end_offset(raw),
    })
}

pub fn table_cell_navigation_anchor(
    line_text: &str,
    pipes: &[usize],
    left_pipe_index: usize,
) -> usize {
    table_cell_span(line_text, pipes, left_pipe_index)
        .map(|cell| cell.navigation_anchor())
        .unwrap_or(0)
}

fn split_row_cells_raw(line: &str) -> Option<Vec<String>> {
    split_row_cells_raw_with_kind(line).map(|(cells, _)| cells)
}

fn strip_continuation_marker(raw: &str) -> String {
    let lead = raw.len().saturating_sub(raw.trim_start().len());
    let trimmed = raw.trim_start();
    let Some(rest) = trimmed.strip_prefix('>') else {
        return raw.to_string();
    };
    let mut out = String::new();
    out.push_str(&raw[..lead]);
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    out.push_str(rest);
    out
}

fn normalize_continuation_first_cell_content(raw: &str) -> String {
    let mut current = raw.to_string();
    loop {
        let next = strip_continuation_marker(&current);
        if next == current {
            break;
        }
        current = next;
    }
    current.trim().to_string()
}

fn split_row_cells_raw_with_kind(line: &str) -> Option<(Vec<String>, bool)> {
    if !is_table_line(line) {
        return None;
    }
    let continuation = is_table_continuation_line(line);
    let trimmed = line.trim();
    let pipes = table_pipe_positions(trimmed);
    if pipes.len() < 2 {
        return None;
    }
    let mut out = Vec::with_capacity(pipes.len().saturating_sub(1));
    for (idx, pair) in pipes.windows(2).enumerate() {
        let start = pair[0] + 1;
        let end = pair[1];
        let raw = trimmed[start..end].to_string();
        if continuation && idx == 0 {
            out.push(strip_continuation_marker(&raw));
        } else {
            out.push(raw);
        }
    }
    Some((out, continuation))
}

pub fn split_table_cells(line: &str) -> Vec<String> {
    split_row_cells_raw(line)
        .unwrap_or_default()
        .into_iter()
        .map(|cell| cell.trim().to_string())
        .collect()
}

pub fn split_table_cells_with_cache(line: &str, cache: &mut TableFormatCache) -> Vec<String> {
    cache.cached_row_parse(line).cells.clone()
}

pub fn table_pipe_positions_with_cache(line: &str, cache: &mut TableFormatCache) -> Vec<usize> {
    cache.cached_row_parse(line).pipes.clone()
}

pub fn split_table_cells_for_logical_row(line: &str) -> Vec<String> {
    split_table_cells(line)
}

pub fn serialize_table_continuation_row(cells: &[String]) -> String {
    if cells.is_empty() {
        return "|>".to_string();
    }
    let mut out = String::new();
    out.push_str("|> ");
    let mut first = true;
    for cell in cells {
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str(cell.trim());
        out.push(' ');
        out.push('|');
    }
    out
}

pub fn serialize_table_row_with_kind(cells: &[String], continuation: bool) -> String {
    if continuation {
        serialize_table_continuation_row(cells)
    } else {
        serialize_table_row(cells)
    }
}

fn logical_trim_offsets(raw: &str, continuation_first_cell: bool) -> (usize, usize) {
    let mut trim_start = first_non_space_offset(raw);
    let trim_end = last_non_space_end_offset(raw);
    if !continuation_first_cell {
        return (trim_start, trim_end);
    }
    let content = &raw[trim_start..trim_end.min(raw.len())];
    if let Some(rest) = content.strip_prefix('>') {
        let marker_ws = content.len().saturating_sub(rest.len());
        trim_start += marker_ws;
        if raw.as_bytes().get(trim_start).is_some_and(|b| *b == b' ') {
            trim_start += 1;
        }
    }
    (trim_start.min(trim_end), trim_end)
}

pub fn table_cell_cursor_info_in_document(
    lines: &[String],
    line_idx: usize,
    col: usize,
) -> Option<TableCursorCellInfo> {
    let mut cache = TableLogicalRowCache::default();
    table_cell_cursor_info_in_document_cached(lines, line_idx, col, &mut cache)
}

fn text_hash(text: &str) -> u64 {
    let mut hasher = FxHasher::default();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Structure of one table block shared by the formatter-facing rules and
/// front-end rendering: bounds, delimiter row, and per-column widths measured
/// with a caller-supplied cell width (raw text for the formatter, visible
/// text after marker collapsing for a renderer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableBlockLayout {
    /// First and last line index of the block (inclusive).
    pub start: usize,
    pub end: usize,
    /// Absolute line index of the delimiter row, if any.
    pub delimiter_row: Option<usize>,
    /// Column widths; at least 3 so `---` delimiters fit.
    pub col_widths: Vec<usize>,
}

impl TableBlockLayout {
    pub fn contains(&self, line_idx: usize) -> bool {
        (self.start..=self.end).contains(&line_idx)
    }

    pub fn is_delimiter(&self, line_idx: usize) -> bool {
        self.delimiter_row == Some(line_idx)
    }

    /// Builds the layout of block `start..=end` (see [`table_block_bounds`]).
    /// `cell_width` measures a trimmed cell.
    pub fn build(
        lines: &[String],
        start: usize,
        end: usize,
        cell_width: impl Fn(&str) -> usize,
    ) -> Self {
        let block = &lines[start..=end];
        let rows: Vec<Vec<String>> = block.iter().map(|line| split_table_cells(line)).collect();
        let continuation: Vec<bool> =
            block.iter().map(|line| is_table_continuation_line(line)).collect();
        let delimiter_row = table_block_delimiter_row(&rows, &continuation);
        let mut col_widths: Vec<usize> = Vec::new();
        for (idx, cells) in rows.iter().enumerate() {
            if Some(idx) == delimiter_row {
                continue;
            }
            if col_widths.len() < cells.len() {
                col_widths.resize(cells.len(), 3);
            }
            for (width, cell) in col_widths.iter_mut().zip(cells) {
                *width = (*width).max(cell_width(cell));
            }
        }
        Self {
            start,
            end,
            delimiter_row: delimiter_row.map(|idx| start + idx),
            col_widths,
        }
    }
}

/// Inclusive line bounds of the table block containing `line_idx`.
pub fn table_block_bounds(lines: &[String], line_idx: usize) -> Option<(usize, usize)> {
    let current = lines.get(line_idx)?;
    if !is_table_line(current) {
        return None;
    }

    let mut block_start = line_idx;
    while block_start > 0 && is_table_line(lines.get(block_start - 1)?) {
        block_start -= 1;
    }
    let mut block_end = line_idx;
    while block_end + 1 < lines.len() && is_table_line(lines.get(block_end + 1)?) {
        block_end += 1;
    }
    Some((block_start, block_end))
}

fn build_logical_row_cache(
    lines: &[String],
    block_start: usize,
    block_end: usize,
) -> Option<TableLogicalRowCache> {
    let mut delimiter_row: Option<usize> = None;
    let mut line_hashes = Vec::with_capacity(block_end.saturating_sub(block_start) + 1);
    let after_header = after_header_row(block_end + 1 - block_start, |i| {
        lines
            .get(block_start + i)
            .is_some_and(|line| is_table_continuation_line(line))
    })
    .map(|i| block_start + i);
    for idx in block_start..=block_end {
        let line = lines.get(idx)?;
        line_hashes.push(text_hash(line));
        if delimiter_row.is_none() {
            let row_cells = split_table_cells(line);
            if is_delimiter_row_at(&row_cells, Some(idx) == after_header) {
                delimiter_row = Some(idx);
            }
        }
    }

    let mut logical_row_count = 0usize;
    let mut logical_row_index_by_line = vec![None; block_end.saturating_sub(block_start) + 1];
    if let Some(delim) = delimiter_row {
        for idx in (delim + 1)..=block_end {
            let is_cont = is_table_continuation_line(lines.get(idx)?);
            if is_cont && logical_row_count > 0 {
                logical_row_index_by_line[idx - block_start] = Some(logical_row_count - 1);
                continue;
            }
            logical_row_index_by_line[idx - block_start] = Some(logical_row_count);
            logical_row_count += 1;
        }
    }
    Some(TableLogicalRowCache {
        block_start,
        block_end,
        logical_row_count,
        delimiter_row,
        line_hashes,
        logical_row_index_by_line,
    })
}

fn cache_matches_lines(cache: &TableLogicalRowCache, lines: &[String]) -> bool {
    if cache.line_hashes.is_empty() {
        return false;
    }
    if cache.block_end < cache.block_start {
        return false;
    }
    let len = cache.block_end - cache.block_start + 1;
    if len != cache.line_hashes.len() {
        return false;
    }
    for idx in 0..len {
        let line_no = cache.block_start + idx;
        let Some(line) = lines.get(line_no) else {
            return false;
        };
        if text_hash(line) != cache.line_hashes[idx] {
            return false;
        }
    }
    true
}

pub fn table_cell_cursor_info_in_document_cached(
    lines: &[String],
    line_idx: usize,
    col: usize,
    cache: &mut TableLogicalRowCache,
) -> Option<TableCursorCellInfo> {
    let current = lines.get(line_idx)?;
    if !is_table_line(current) {
        return None;
    }
    let (block_start, block_end) = table_block_bounds(lines, line_idx)?;
    let cache_can_reuse = cache.block_contains_line(line_idx)
        && cache.block_start == block_start
        && cache.block_end == block_end
        && cache_matches_lines(cache, lines);

    if !cache_can_reuse {
        *cache = build_logical_row_cache(lines, block_start, block_end)?;
    }
    let logical_row_index_for_line = if line_idx < cache.block_start || line_idx > cache.block_end {
        None
    } else {
        cache
            .logical_row_index_by_line
            .get(line_idx - cache.block_start)
            .copied()
            .flatten()
    };

    let pipes = table_pipe_positions(current);
    if pipes.len() < 2 {
        return None;
    }
    let col_in_line = col.min(current.len());
    if col_in_line > *pipes.last()? {
        // Past the closing pipe: the cursor is starting a new cell, not in one.
        return None;
    }
    let cell_index = table_cell_index_for_column(&pipes, col_in_line)?;
    let left_pipe = *pipes.get(cell_index)?;
    let right_pipe = *pipes.get(cell_index + 1)?;
    let raw = &current[left_pipe + 1..right_pipe];
    let (trim_start, trim_end) =
        logical_trim_offsets(raw, is_table_continuation_line(current) && cell_index == 0);
    Some(TableCursorCellInfo {
        column_index: cell_index,
        column_count: pipes.len().saturating_sub(1),
        left_pipe,
        right_pipe,
        trim_start,
        trim_end,
        logical_row_index: logical_row_index_for_line,
        logical_row_count: cache.logical_row_count,
        is_continuation_row: is_table_continuation_line(current),
    })
}

fn table_line_break_tag_len_at(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if *bytes.get(start)? != b'<' {
        return None;
    }
    let mut idx = start + 1;
    let b = bytes.get(idx)?.to_ascii_lowercase();
    if b != b'b' {
        return None;
    }
    idx += 1;
    let r = bytes.get(idx)?.to_ascii_lowercase();
    if r != b'r' {
        return None;
    }
    idx += 1;

    while let Some(ch) = bytes.get(idx) {
        if *ch == b' ' || *ch == b'\t' {
            idx += 1;
        } else {
            break;
        }
    }

    if matches!(bytes.get(idx), Some(b'/')) {
        idx += 1;
    }

    while let Some(ch) = bytes.get(idx) {
        if *ch == b' ' || *ch == b'\t' {
            idx += 1;
        } else {
            break;
        }
    }

    if matches!(bytes.get(idx), Some(b'>')) {
        return Some(idx + 1 - start);
    }
    None
}

pub fn table_cell_display_width(cell: &str) -> usize {
    if cell.is_empty() {
        return 0;
    }
    let mut max_width = 0usize;
    let mut segment_start = 0usize;
    let mut idx = 0usize;
    while idx < cell.len() {
        if let Some(tag_len) = table_line_break_tag_len_at(cell, idx) {
            let segment = cell[segment_start..idx].trim();
            max_width = max_width.max(segment.chars().count());
            idx += tag_len;
            segment_start = idx;
            continue;
        }
        idx += 1;
    }
    let tail = cell[segment_start..].trim();
    max_width.max(tail.chars().count())
}

pub fn table_column_count(line: &str) -> Option<usize> {
    Some(split_row_cells_raw(line)?.len().max(1))
}

/// Dash count of a delimiter-shaped cell (`:?-+:?`), or None.
fn delimiter_cell_dashes(cell: &str) -> Option<usize> {
    let trimmed = cell.trim();
    let core = trimmed.strip_prefix(':').unwrap_or(trimmed);
    let core = core.strip_suffix(':').unwrap_or(core);
    (!core.is_empty() && core.bytes().all(|b| b == b'-')).then_some(core.len())
}

/// A `---`-style delimiter cell (at least 3 dashes), recognized anywhere.
pub fn is_delimiter_cell(cell: &str) -> bool {
    delimiter_cell_dashes(cell).is_some_and(|dashes| dashes >= 3)
}

/// Whether `cells` form the table's delimiter row.
///
/// `---`-style rows count anywhere (empty cells allowed while one is being
/// typed). Short GFM delimiters (`-`, `--`, `:-:`) count only when
/// `after_header` — the row directly follows the header (and its `|>`
/// continuation rows), where GFM requires the delimiter. Elsewhere a row of
/// `-` cells is data, e.g. "n/a" placeholders.
pub fn is_delimiter_row_at(cells: &[String], after_header: bool) -> bool {
    is_delimiter_row(cells)
        || (after_header
            && !cells.is_empty()
            && cells.iter().all(|cell| delimiter_cell_dashes(cell).is_some()))
}

/// Whether a row of delimiter-shaped cells (short or `---`-style).
pub fn is_delimiter_shaped_row(cells: &[String]) -> bool {
    is_delimiter_row_at(cells, true)
}

/// Whether table line `idx` sits where a short GFM delimiter is allowed: right
/// after the block's header row and the header's continuation rows.
/// `line_at(i)` yields line `i`, or None past the document bounds.
pub fn follows_table_header<'a>(idx: usize, line_at: impl Fn(usize) -> Option<&'a str>) -> bool {
    let mut i = idx;
    while i > 0 {
        i -= 1;
        let Some(line) = line_at(i).filter(|line| is_table_line(line)) else {
            return false;
        };
        if !is_table_continuation_line(line) {
            return i == 0 || !line_at(i - 1).is_some_and(is_table_line);
        }
    }
    false
}

/// Position-aware delimiter check for line `idx` of `lines`.
pub fn is_delimiter_line_in(lines: &[String], idx: usize) -> bool {
    let Some(line) = lines.get(idx) else {
        return false;
    };
    is_table_line(line)
        && is_delimiter_row_at(
            &split_table_cells(line),
            follows_table_header(idx, |i| lines.get(i).map(String::as_str)),
        )
}

/// Index of the row that may hold a short delimiter: the first row after the
/// header that is not a `|>` continuation row.
fn after_header_row(row_count: usize, continuation: impl Fn(usize) -> bool) -> Option<usize> {
    (1..row_count).find(|&i| !continuation(i))
}

/// Index of the delimiter row among a whole table block's rows (cells plus
/// continuation flags, one entry per line).
pub fn table_block_delimiter_row(rows: &[Vec<String>], continuation: &[bool]) -> Option<usize> {
    let after_header =
        after_header_row(rows.len(), |i| continuation.get(i).copied().unwrap_or(false));
    rows.iter()
        .enumerate()
        .position(|(i, row)| is_delimiter_row_at(row, Some(i) == after_header))
}

pub fn parse_align(cell: &str) -> TableAlign {
    if delimiter_cell_dashes(cell).is_none() {
        return TableAlign::None;
    }
    let trimmed = cell.trim();
    let left = trimmed.starts_with(':');
    let right = trimmed.ends_with(':');
    match (left, right) {
        (true, true) => TableAlign::Center,
        (true, false) => TableAlign::Left,
        (false, true) => TableAlign::Right,
        (false, false) => TableAlign::None,
    }
}

fn min_delimiter_len(align: TableAlign) -> usize {
    match align {
        TableAlign::None => 3,
        TableAlign::Left | TableAlign::Right => 4,
        TableAlign::Center => 5,
    }
}

pub fn normalize_delimiter_cell_for_width(cell: &str, min_width: usize) -> String {
    let align = parse_align(cell);
    let target_width = min_width.max(min_delimiter_len(align));
    match align {
        TableAlign::Left => format!(":{}", "-".repeat(target_width.saturating_sub(1).max(3))),
        TableAlign::Right => format!("{}:", "-".repeat(target_width.saturating_sub(1).max(3))),
        TableAlign::Center => format!(":{}:", "-".repeat(target_width.saturating_sub(2).max(3))),
        TableAlign::None => "-".repeat(target_width.max(3)),
    }
}

pub fn normalize_delimiter_cell(cell: &str) -> String {
    let dashes = cell.trim().chars().filter(|ch| *ch == '-').count().max(3);
    let align = parse_align(cell);
    let min_width = match align {
        TableAlign::None => dashes,
        TableAlign::Left | TableAlign::Right => dashes + 1,
        TableAlign::Center => dashes + 2,
    };
    normalize_delimiter_cell_for_width(cell, min_width)
}

pub fn is_delimiter_row(cells: &[String]) -> bool {
    cells.iter().any(|cell| is_delimiter_cell(cell))
        && cells
            .iter()
            .all(|cell| is_delimiter_cell(cell) || cell.trim().is_empty())
}

pub fn serialize_table_row(cells: &[String]) -> String {
    let parts = cells
        .iter()
        .map(|cell| cell.trim().to_string())
        .collect::<Vec<_>>();
    format!("| {} |", parts.join(" | "))
}

pub fn format_table_lines(lines: &[String]) -> Vec<String> {
    let mut cache = TableFormatCache::default();
    format_table_lines_with_cache(lines, &mut cache)
}

pub fn format_table_lines_with_cache(
    lines: &[String],
    cache: &mut TableFormatCache,
) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }

    let mut rows: Vec<Vec<String>> = Vec::with_capacity(lines.len());
    let mut row_cont: Vec<bool> = Vec::with_capacity(lines.len());
    for line in lines {
        let Some((raw, continuation)) = split_row_cells_raw_with_kind(line) else {
            return lines.to_vec();
        };
        rows.push(raw);
        row_cont.push(continuation);
    }

    let column_count = rows.iter().map(|row| row.len()).max().unwrap_or(1).max(1);
    let normalized_rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|mut row| {
            while row.len() < column_count {
                row.push(String::new());
            }
            row
        })
        .collect();

    let mut normalized_rows = normalized_rows;
    let mut row_cont = row_cont;
    let mut after_header = after_header_row(normalized_rows.len(), |i| row_cont[i]);
    let has_delimiter_row = table_block_delimiter_row(&normalized_rows, &row_cont).is_some();
    if !has_delimiter_row && normalized_rows.len() >= 2 {
        normalized_rows.insert(1, vec!["---".to_string(); column_count]);
        row_cont.insert(1, false);
        after_header = Some(1);
    }

    let mut normalized_content: Vec<Vec<String>> = Vec::with_capacity(normalized_rows.len());
    let mut delimiter_flags: Vec<bool> = Vec::with_capacity(normalized_rows.len());
    for (row_idx, row) in normalized_rows.iter().enumerate() {
        let delimiter = is_delimiter_row_at(row, Some(row_idx) == after_header);
        delimiter_flags.push(delimiter);
        let continuation = *row_cont.get(row_idx).unwrap_or(&false);
        let mut out = Vec::with_capacity(column_count);
        for col in 0..column_count {
            let raw = row.get(col).map_or("", |cell| cell.as_str());
            let cell = if delimiter {
                if raw.trim().is_empty() {
                    "---".to_string()
                } else {
                    normalize_delimiter_cell(raw)
                }
            } else if continuation && col == 0 {
                normalize_continuation_first_cell_content(raw)
            } else {
                raw.trim().to_string()
            };
            out.push(cell);
        }
        normalized_content.push(out);
    }

    let compute_col_width = |col: usize| -> usize {
        let mut width = 0usize;
        for (row_idx, row) in normalized_content.iter().enumerate() {
            let cell = row.get(col).map_or("", String::as_str);
            if delimiter_flags.get(row_idx).copied().unwrap_or(false) {
                // Alignment colons widen the column (`:---:` needs 5).
                width = width.max(min_delimiter_len(parse_align(cell)));
            } else {
                width = width.max(table_cell_display_width(cell));
            }
        }
        width
    };

    let mut widths = {
        let cache_reusable = cache.row_cells.len() == normalized_content.len()
            && cache.delimiter_flags.len() == delimiter_flags.len()
            && cache.widths.len() == column_count
            && cache.row_cells.iter().all(|row| row.len() == column_count);

        if !cache_reusable {
            (0..column_count).map(compute_col_width).collect::<Vec<_>>()
        } else {
            let mut dirty_cols = vec![false; column_count];
            let mut any_change = false;
            for row_idx in 0..normalized_content.len() {
                if cache.delimiter_flags[row_idx] != delimiter_flags[row_idx] {
                    dirty_cols.fill(true);
                    any_change = true;
                    break;
                }
                for col in 0..column_count {
                    if cache.row_cells[row_idx][col] != normalized_content[row_idx][col] {
                        dirty_cols[col] = true;
                        any_change = true;
                    }
                }
            }

            if !any_change {
                cache.widths.clone()
            } else {
                let mut next_widths = cache.widths.clone();
                for col in 0..column_count {
                    if dirty_cols[col] {
                        next_widths[col] = compute_col_width(col);
                    }
                }
                next_widths
            }
        }
    };
    // Delimiter cells must render at least `---` (3 dashes), so enforce a
    // floor on every column width — but only when the table actually has
    // (or is about to get) a delimiter row.
    if delimiter_flags.contains(&true) {
        for w in widths.iter_mut() {
            *w = (*w).max(3);
        }
    }

    cache.row_cells = normalized_content.clone();
    cache.delimiter_flags = delimiter_flags.clone();
    cache.widths = widths.clone();

    normalized_rows
        .iter()
        .zip(normalized_content.iter())
        .enumerate()
        .map(|(row_idx, (raw_row, normalized))| {
            let delimiter = delimiter_flags[row_idx];
            let continuation = *row_cont.get(row_idx).unwrap_or(&false);
            let mut out = String::new();
            if continuation {
                out.push_str("|>");
            } else {
                out.push('|');
            }
            for (col, cell) in normalized.iter().enumerate() {
                let content = if delimiter {
                    normalize_delimiter_cell_for_width(
                        raw_row.get(col).map_or("---", |v| {
                            let trimmed = v.trim();
                            if trimmed.is_empty() {
                                "---"
                            } else {
                                trimmed
                            }
                        }),
                        widths[col],
                    )
                } else {
                    cell.clone()
                };
                let content_width = if delimiter {
                    content.chars().count()
                } else {
                    table_cell_display_width(&content)
                };
                let pad_right = widths[col].saturating_sub(content_width) + 1;
                if !(continuation && col == 0) {
                    out.push(' ');
                }
                out.push_str(&content);
                out.push_str(&" ".repeat(pad_right));
                out.push('|');
            }
            out
        })
        .collect()
}

/// Whether `line` is a `|>` continuation row with no content left, which
/// editing removes so a multiline cell does not leave blank rows behind.
pub fn is_empty_table_continuation_row(line: &str) -> bool {
    if !is_table_continuation_line(line) {
        return false;
    }
    split_table_cells(line).iter().enumerate().all(|(idx, cell)| {
        let cell = cell.trim();
        if idx == 0 {
            cell.trim_start_matches(|c: char| c == '>' || c.is_whitespace())
                .is_empty()
        } else {
            cell.is_empty()
        }
    })
}

/// Char column where deleting the word before `cursor` stops inside a table
/// cell whose editable text spans `edit_start..=edit_end` (char columns):
/// trailing non-word chars, then the word, never past the cell start. None
/// when nothing would be deleted.
pub fn table_cell_word_delete_start(
    line: &str,
    cursor: usize,
    edit_start: usize,
    edit_end: usize,
) -> Option<usize> {
    if cursor <= edit_start {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    let mut col = cursor.min(edit_end);
    while col > edit_start && chars.get(col - 1).is_some_and(|c| !c.is_alphanumeric()) {
        col -= 1;
    }
    while col > edit_start && chars.get(col - 1).is_some_and(|c| c.is_alphanumeric()) {
        col -= 1;
    }
    (col != cursor).then_some(col)
}

/// Per-char word classes of a table row for `w`/`b`: 0 for whitespace and
/// structural pipes (so motions step over cell borders), 1 for word chars,
/// 2 for other punctuation. Escaped `\|` stays punctuation.
fn table_row_word_classes(line: &str) -> Vec<u8> {
    let pipes = table_pipe_positions(line);
    line.char_indices()
        .map(|(byte, ch)| {
            if ch.is_whitespace() || pipes.binary_search(&byte).is_ok() {
                0
            } else if ch.is_alphanumeric() || ch == '_' {
                1
            } else {
                2
            }
        })
        .collect()
}

/// Char column of the next word start after `col` in table row `line`, or
/// None when no word follows on this row.
pub fn table_row_next_word_start(line: &str, col: usize) -> Option<usize> {
    let classes = table_row_word_classes(line);
    let len = classes.len();
    let mut c = col.min(len);
    if let Some(&start) = classes.get(c).filter(|&&class| class != 0) {
        while c < len && classes[c] == start {
            c += 1;
        }
    }
    while c < len && classes[c] == 0 {
        c += 1;
    }
    (c < len).then_some(c)
}

/// Char column of the word start before `col` in table row `line`, or None
/// when no word precedes it on this row.
pub fn table_row_prev_word_start(line: &str, col: usize) -> Option<usize> {
    let classes = table_row_word_classes(line);
    let mut c = col.min(classes.len());
    while c > 0 && classes[c - 1] == 0 {
        c -= 1;
    }
    let class = *classes.get(c.checked_sub(1)?)?;
    while c > 0 && classes[c - 1] == class {
        c -= 1;
    }
    Some(c)
}

/// Replacement of lines `start..=end` with `lines`, and where the cursor
/// lands (`cursor_line` absolute, `cursor_byte` within that line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableBlockEdit {
    pub start: usize,
    pub end: usize,
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_byte: usize,
}

/// Pastes multiline `text` into the table cell at (`line_idx`, `cursor_byte`):
/// the first line joins the cell before the cursor, each further line goes
/// into the same column of a new `|>` continuation row, and the text after
/// the cursor follows the last pasted line. None outside a data cell or for
/// single-line text.
pub fn plan_table_cell_multiline_paste(
    lines: &[String],
    line_idx: usize,
    cursor_byte: usize,
    text: &str,
    cache: &mut TableFormatCache,
) -> Option<TableBlockEdit> {
    let parts: Vec<&str> = text.split('\n').collect();
    if parts.len() < 2 || is_delimiter_line_in(lines, line_idx) {
        return None;
    }
    let (block_start, block_end) = table_block_bounds(lines, line_idx)?;
    let current_line = lines.get(line_idx)?;
    let pipes = table_pipe_positions(current_line);
    let column_index = table_cell_index_for_column(&pipes, cursor_byte)?;
    let span = table_cell_span(current_line, &pipes, column_index)?;

    let mut row_cells: Vec<Vec<String>> = lines[block_start..=block_end]
        .iter()
        .map(|line| split_table_cells(line))
        .collect();
    let mut row_continuations: Vec<bool> = lines[block_start..=block_end]
        .iter()
        .map(|line| is_table_continuation_line(line))
        .collect();
    let relative_row = line_idx - block_start;
    let column_count = row_cells[relative_row]
        .len()
        .max(pipes.len().saturating_sub(1))
        .max(column_index + 1);
    for row in &mut row_cells {
        row.resize(column_count, String::new());
    }

    let content_start = (span.left_pipe + 1 + span.trim_start).min(current_line.len());
    let content_end = (span.left_pipe + 1 + span.trim_end).min(current_line.len());
    let split = cursor_byte.clamp(content_start, content_end);
    let left_existing = &current_line[content_start..split];
    let right_existing = &current_line[split..content_end];

    row_cells[relative_row][column_index] = format!("{left_existing}{}", parts[0]);
    for (idx, part) in parts.iter().enumerate().skip(1) {
        let mut next_row = vec![String::new(); column_count];
        next_row[column_index] = if idx + 1 == parts.len() {
            format!("{part}{right_existing}")
        } else {
            (*part).to_string()
        };
        row_cells.insert(relative_row + idx, next_row);
        row_continuations.insert(relative_row + idx, true);
    }

    let raw_lines: Vec<String> = row_cells
        .iter()
        .zip(&row_continuations)
        .map(|(cells, &continuation)| serialize_table_row_with_kind(cells, continuation))
        .collect();
    let formatted = format_table_lines_with_cache(&raw_lines, cache);

    let target_relative_row = relative_row + parts.len() - 1;
    let cursor_line = (block_start + target_relative_row).min(block_start + formatted.len() - 1);
    let target_line = &formatted[cursor_line - block_start];
    let target_pipes = table_pipe_positions(target_line);
    let cursor_byte = table_cell_span(target_line, &target_pipes, column_index)
        .map(|target_span| {
            let anchor = target_span.navigation_anchor();
            let start = target_span.edit_start().min(anchor);
            let last_len = parts.last().map_or(0, |part| part.len());
            start.saturating_add(last_len).min(anchor)
        })
        .unwrap_or(0);
    Some(TableBlockEdit {
        start: block_start,
        end: block_end,
        lines: formatted,
        cursor_line,
        cursor_byte,
    })
}

pub fn build_empty_table_row_like(line_text: &str) -> Option<String> {
    let column_count = table_column_count(line_text)?;
    let cells = vec![String::new(); column_count];
    Some(serialize_table_row(&cells))
}

pub fn merge_cell_content(left: &str, right: &str) -> String {
    let left = left.trim();
    let right = right.trim();
    match (left.is_empty(), right.is_empty()) {
        (true, true) => String::new(),
        (true, false) => right.to_string(),
        (false, true) => left.to_string(),
        (false, false) => format!("{left} {right}"),
    }
}

pub fn map_table_cursor_column(source_line: &str, target_line: &str, source_col: usize) -> usize {
    let source_pipes = table_pipe_positions(source_line);
    let target_pipes = table_pipe_positions(target_line);
    if source_pipes.len() < 2 || target_pipes.len() < 2 {
        return source_col.min(target_line.len());
    }
    // Past the closing pipe (starting the next cell by hand): stay past it.
    if let (Some(&source_last), Some(&target_last)) = (source_pipes.last(), target_pipes.last()) {
        if source_col > source_last {
            return (target_last + (source_col - source_last)).min(target_line.len());
        }
    }

    let Some(source_cell_index) = table_cell_index_for_column(&source_pipes, source_col) else {
        return source_col.min(target_line.len());
    };
    let target_cell_index = source_cell_index.min(target_pipes.len().saturating_sub(2));

    let Some(source_cell) = table_cell_span(source_line, &source_pipes, source_cell_index) else {
        return source_col.min(target_line.len());
    };
    let source_left = source_cell.left_pipe + 1;
    let source_cont_first = is_table_continuation_line(source_line) && source_cell_index == 0;
    let target_cont_first = is_table_continuation_line(target_line) && target_cell_index == 0;
    let source_raw = &source_line[source_left..source_cell.right_pipe];
    let (source_trim_start, source_trim_end) = logical_trim_offsets(source_raw, source_cont_first);
    let source_in_cell = source_col
        .saturating_sub(source_left)
        .min(source_cell.right_pipe.saturating_sub(source_left));

    let source_content_len = source_trim_end.saturating_sub(source_trim_start);
    let semantic_offset = if source_content_len == 0 {
        0
    } else if source_in_cell <= source_trim_start {
        0
    } else if source_in_cell >= source_trim_end {
        source_content_len
    } else {
        source_in_cell.saturating_sub(source_trim_start)
    };

    let Some(target_cell) = table_cell_span(target_line, &target_pipes, target_cell_index) else {
        return source_col.min(target_line.len());
    };
    let target_left = target_cell.left_pipe + 1;
    let target_raw = &target_line[target_left..target_cell.right_pipe];
    let (target_trim_start, target_trim_end) = logical_trim_offsets(target_raw, target_cont_first);
    let target_content_len = target_trim_end.saturating_sub(target_trim_start);
    if target_content_len == 0 {
        let anchor = if target_cont_first {
            (target_cell.left_pipe + 2).min(target_cell.right_pipe)
        } else {
            target_cell.navigation_anchor()
        };
        return anchor.min(target_line.len());
    }
    let mapped_in_target = target_trim_start + semantic_offset.min(target_content_len);
    (target_left + mapped_in_target).min(target_line.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_table_lines_shrinks_column_with_trailing_pad_left_over() {
        // After delete: only the formerly-widest cell got shorter. The other
        // cells still carry the trailing padding from the previous format.
        let lines = vec![
            "| a | hi         |".to_string(),
            "| --- | ---------- |".to_string(),
            "| 1 | 22         |".to_string(),
        ];
        let f = format_table_lines(&lines);
        eprintln!("RES: {:?}", f);
        // Column 1 used to be ~10 wide because of the dashes in the
        // delimiter row. After the data shrank, the column should reflow to
        // fit the widest data cell (here "22"), padded by the `---` floor.
        // Expected layout: "| a   | hi  |" — len 13.
        assert_eq!(f[0].len(), 13, "lines: {:?}", f);
    }

    #[test]
    fn format_table_lines_shrinks_column_when_widest_cell_shrinks() {
        let wide = vec![
            "| a | bbbbbbbb |".to_string(),
            "| --- | --- |".to_string(),
            "| 1 | 22 |".to_string(),
        ];
        let formatted_wide = format_table_lines(&wide);
        let widest_col_len = formatted_wide[0].len();

        let narrow = vec![
            "| a | bb |".to_string(),
            "| --- | --- |".to_string(),
            "| 1 | 22 |".to_string(),
        ];
        let formatted_narrow = format_table_lines(&narrow);
        let narrow_col_len = formatted_narrow[0].len();

        assert!(
            narrow_col_len < widest_col_len,
            "expected narrow ({}) < wide ({})\nwide: {:?}\nnarrow: {:?}",
            narrow_col_len,
            widest_col_len,
            formatted_wide,
            formatted_narrow,
        );
    }

    #[test]
    fn table_cell_display_width_uses_longest_multiline_segment() {
        assert_eq!(table_cell_display_width("one"), 3);
        assert_eq!(table_cell_display_width("a<br>abcd"), 4);
        assert_eq!(table_cell_display_width("a <br/> abcd"), 4);
        assert_eq!(table_cell_display_width("x<BR />yy"), 2);
    }

    #[test]
    fn format_table_lines_handles_multiline_cell_width() {
        let lines = vec![
            "| h | body |".to_string(),
            "| --- | --- |".to_string(),
            "| a | short<br>very very long |".to_string(),
            "| b | tiny |".to_string(),
        ];
        let out = format_table_lines(&lines);
        assert_eq!(
            out[2], "| a   | short<br>very very long |",
            "multiline content should stay intact"
        );
        assert_eq!(
            out[3], "| b   | tiny           |",
            "column width should follow longest logical line in multiline cell"
        );
    }

    #[test]
    fn table_pipe_positions_ignores_escaped_pipes() {
        let line = "| a\\|b | c |";
        assert_eq!(table_pipe_positions(line), vec![0, 7, 11]);
    }

    #[test]
    fn split_table_cells_keeps_escaped_pipe_inside_cell() {
        let line = "| left \\| right | ok |";
        assert_eq!(
            split_table_cells(line),
            vec!["left \\| right".to_string(), "ok".to_string()]
        );
    }

    #[test]
    fn cached_table_row_parse_reuses_cells_and_pipe_positions() {
        let mut cache = TableFormatCache::default();
        let line = "| left \\| right | ok |";
        assert_eq!(
            split_table_cells_with_cache(line, &mut cache),
            vec!["left \\| right".to_string(), "ok".to_string()]
        );
        assert_eq!(
            table_pipe_positions_with_cache(line, &mut cache),
            vec![0, 16, 21]
        );

        // Re-read the same line to exercise cache reuse path.
        assert_eq!(
            split_table_cells_with_cache(line, &mut cache),
            vec!["left \\| right".to_string(), "ok".to_string()]
        );
    }

    #[test]
    fn cached_table_row_parse_invalidate_drops_previous_entries() {
        let mut cache = TableFormatCache::default();
        let line = "| a | b |";
        assert_eq!(
            split_table_cells_with_cache(line, &mut cache),
            vec!["a", "b"]
        );
        cache.invalidate_parsed_rows();
        assert_eq!(
            table_pipe_positions_with_cache(line, &mut cache),
            table_pipe_positions(line)
        );
    }

    #[test]
    fn continuation_row_detection_and_split() {
        let line = "|> left detail | right detail |";
        assert!(is_table_continuation_line(line));
        assert_eq!(
            split_table_cells_for_logical_row(line),
            vec!["left detail".to_string(), "right detail".to_string()]
        );
    }

    #[test]
    fn serialize_table_continuation_row_uses_canonical_prefix() {
        let row = serialize_table_continuation_row(&["left".to_string(), "right".to_string()]);
        assert_eq!(row, "|> left | right |");
    }

    #[test]
    fn table_cell_cursor_info_maps_continuation_to_same_logical_row() {
        let lines = vec![
            "| name | value |".to_string(),
            "| --- | --- |".to_string(),
            "| alpha | one |".to_string(),
            "|> beta | two |".to_string(),
            "| gamma | three |".to_string(),
        ];
        let info_first = table_cell_cursor_info_in_document(&lines, 2, 3).expect("row 1");
        let info_cont = table_cell_cursor_info_in_document(&lines, 3, 4).expect("cont row");
        let info_next = table_cell_cursor_info_in_document(&lines, 4, 3).expect("row 2");
        assert_eq!(info_first.logical_row_count, 2);
        assert_eq!(info_cont.logical_row_count, 2);
        assert_eq!(info_next.logical_row_count, 2);
        assert_eq!(info_first.logical_row_index, Some(0));
        assert_eq!(info_cont.logical_row_index, Some(0));
        assert_eq!(info_next.logical_row_index, Some(1));
        assert!(info_cont.is_continuation_row);
    }

    #[test]
    fn table_cursor_motion_moves_right_to_next_cell_edit_start() {
        let lines = vec!["| a | b |".to_string(), "| --- | --- |".to_string()];
        let target = plan_table_cursor_motion(&lines, 0, 3, TableCursorMotionDirection::Right)
            .expect("target");
        assert_eq!(target.line_index, 0);
        assert_eq!(target.col, 6);
    }

    #[test]
    fn table_cursor_motion_moves_down_to_content_anchor_and_skips_delimiter() {
        let lines = vec![
            "| longvalue | x |".to_string(),
            "| --------- | - |".to_string(),
            "| a         | y |".to_string(),
        ];
        let target = plan_table_cursor_motion(&lines, 0, 11, TableCursorMotionDirection::Down)
            .expect("target");
        assert_eq!(target.line_index, 2);
        assert_eq!(target.col, 3);
    }

    #[test]
    fn table_cursor_motion_exits_before_and_after_block() {
        let lines = vec![
            "| a | b |".to_string(),
            "| --- | --- |".to_string(),
            "| c | d |".to_string(),
        ];
        let before = plan_table_cursor_motion(&lines, 0, 2, TableCursorMotionDirection::Left)
            .expect("before");
        assert_eq!(before.line_index, -1);
        assert_eq!(before.col, 0);

        let after = plan_table_cursor_motion(&lines, 2, 7, TableCursorMotionDirection::Down)
            .expect("after");
        assert_eq!(after.line_index, 3);
        assert_eq!(after.col, 0);
    }

    fn owned(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn short_gfm_delimiter_counts_only_after_the_header() {
        let lines = owned(&["before", "| a | b |", "|-|:--:|", "| - | - |", "| 1 | 2 |"]);
        assert!(!is_delimiter_line_in(&lines, 1));
        assert!(is_delimiter_line_in(&lines, 2));
        assert!(!is_delimiter_line_in(&lines, 3), "`-` placeholders are data");

        let with_cont = owned(&["| a | b |", "|> a2 | |", "|--|--|"]);
        assert!(is_delimiter_line_in(&with_cont, 2), "header continuation rows are skipped");

        let dashes = |cells: &[&str]| cells.iter().map(|c| c.to_string()).collect::<Vec<_>>();
        assert!(is_delimiter_row_at(&dashes(&["---", ""]), false));
        assert!(!is_delimiter_row_at(&dashes(&["--", ""]), true), "short needs every cell");
        assert!(!is_delimiter_row_at(&dashes(&["", ""]), true));
    }

    #[test]
    fn format_table_lines_normalizes_short_delimiter_without_adding_another() {
        let out = format_table_lines(&owned(&["|a|b|c|", "|-|:-:|--:|", "|1|2|3|"]));
        assert_eq!(
            out,
            owned(&["| a   | b     | c    |", "| --- | :---: | ---: |", "| 1   | 2     | 3    |"])
        );
        let out = format_table_lines(&owned(&["| a | b |", "| - | - |", "| 1 | 2 |"]));
        assert_eq!(out[1], "| --- | --- |");
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn short_delimiter_keeps_later_dash_rows_as_data() {
        let out = format_table_lines(&owned(&["| a | b |", "| --- | --- |", "| - | - |"]));
        assert_eq!(out[2], "| -   | -   |");
    }

    #[test]
    fn vertical_motion_does_not_skip_empty_data_rows() {
        let lines = owned(&["| a   | b   |", "| --- | --- |", "| 1   | 2   |", "|     |     |", "| 3   | 4   |"]);
        let target =
            plan_table_cursor_motion(&lines, 2, 2, TableCursorMotionDirection::Down).expect("target");
        assert_eq!(target.line_index, 3);
        let target =
            plan_table_cursor_motion(&lines, 2, 2, TableCursorMotionDirection::Up).expect("target");
        assert_eq!(target.line_index, 0, "delimiter row is skipped");
    }

    #[test]
    fn table_row_word_motions_step_over_cell_borders() {
        let line = "| dusan | as dasd | y |";
        let starts: Vec<usize> = ["dusan", "as", "dasd", "y"]
            .iter()
            .map(|word| line.find(word).unwrap())
            .collect();
        assert_eq!(table_row_next_word_start(line, 0), Some(starts[0]));
        assert_eq!(table_row_next_word_start(line, starts[0]), Some(starts[1]));
        assert_eq!(table_row_next_word_start(line, starts[0] + 2), Some(starts[1]));
        assert_eq!(table_row_next_word_start(line, starts[1]), Some(starts[2]));
        assert_eq!(table_row_next_word_start(line, starts[2]), Some(starts[3]));
        assert_eq!(table_row_next_word_start(line, starts[3]), None);

        assert_eq!(table_row_prev_word_start(line, starts[3]), Some(starts[2]));
        assert_eq!(table_row_prev_word_start(line, starts[2]), Some(starts[1]));
        assert_eq!(table_row_prev_word_start(line, starts[1]), Some(starts[0]));
        assert_eq!(table_row_prev_word_start(line, starts[0] + 3), Some(starts[0]));
        assert_eq!(table_row_prev_word_start(line, starts[0]), None);

        let escaped = r"| a\|b | c |";
        assert_eq!(
            table_row_next_word_start(escaped, escaped.find('a').unwrap()),
            escaped.find('\\'),
            "escaped pipe is punctuation, not a border"
        );
    }

    #[test]
    fn empty_continuation_row_detection_ignores_marker_and_padding() {
        assert!(is_empty_table_continuation_row("|>   |    |"));
        assert!(is_empty_table_continuation_row("|> > |  |"));
        assert!(!is_empty_table_continuation_row("|> a |  |"));
        assert!(!is_empty_table_continuation_row("|    |  |"), "not a continuation row");
    }

    #[test]
    fn word_delete_start_stays_inside_the_cell() {
        let line = "| ab cd  | x |";
        assert_eq!(table_cell_word_delete_start(line, 7, 2, 7), Some(5));
        assert_eq!(table_cell_word_delete_start(line, 5, 2, 7), Some(2));
        assert_eq!(table_cell_word_delete_start(line, 2, 2, 7), None);
    }

    #[test]
    fn multiline_paste_fills_continuation_rows_in_the_same_column() {
        let lines = owned(&["| a   | b   |", "| --- | --- |", "| xy  | z   |"]);
        let cursor = lines[2].find('y').unwrap();
        let mut cache = TableFormatCache::default();
        let edit = plan_table_cell_multiline_paste(&lines, 2, cursor, "1\n2", &mut cache)
            .expect("paste plan");
        assert_eq!((edit.start, edit.end), (0, 2));
        assert_eq!(
            edit.lines,
            owned(&["| a   | b   |", "| --- | --- |", "| x1  | z   |", "|>2y  |     |"])
        );
        assert_eq!(edit.cursor_line, 3);
        assert_eq!(edit.cursor_byte, edit.lines[3].find('y').unwrap(), "after the pasted text");
        assert!(plan_table_cell_multiline_paste(&lines, 1, 3, "1\n2", &mut cache).is_none());
        assert!(plan_table_cell_multiline_paste(&lines, 2, cursor, "1", &mut cache).is_none());
    }

    #[test]
    fn map_table_cursor_column_keeps_cursor_after_closing_pipe() {
        assert_eq!(map_table_cursor_column("|sasa|", "| sasa |", 6), 8);
        assert_eq!(map_table_cursor_column("|a|b|", "| a | b |", 5), 9);
    }

    #[test]
    fn map_table_cursor_column_ignores_continuation_marker_for_first_cell() {
        let source = "|> abc |";
        let target = "|> abcde |";
        let source_col = source.find("b").expect("b");
        let mapped = map_table_cursor_column(source, target, source_col);
        assert_eq!(mapped, target.find("b").expect("b"));
    }

    #[test]
    fn table_cell_cursor_info_cached_reuses_and_invalidates_on_table_change() {
        let mut lines = vec![
            "before".to_string(),
            "| name | value |".to_string(),
            "| --- | --- |".to_string(),
            "| alpha | one |".to_string(),
            "|> beta | two |".to_string(),
            "after".to_string(),
        ];
        let mut cache = TableLogicalRowCache::default();
        let base =
            table_cell_cursor_info_in_document_cached(&lines, 3, 4, &mut cache).expect("base");
        let cont =
            table_cell_cursor_info_in_document_cached(&lines, 4, 4, &mut cache).expect("cont");
        assert_eq!(base.logical_row_index, Some(0));
        assert_eq!(cont.logical_row_index, Some(0));
        assert_eq!(cont.logical_row_count, 1);

        // Outside-block edits should not affect the cached logical mapping.
        lines[0].push('!');
        let cont_after_outside_edit =
            table_cell_cursor_info_in_document_cached(&lines, 4, 4, &mut cache).expect("cont");
        assert_eq!(cont_after_outside_edit.logical_row_index, Some(0));

        // In-block edits invalidate and recompute the mapping.
        lines[4] = "| gamma | three |".to_string();
        let next =
            table_cell_cursor_info_in_document_cached(&lines, 4, 4, &mut cache).expect("next");
        assert_eq!(next.logical_row_index, Some(1));
        assert_eq!(next.logical_row_count, 2);
        assert!(!next.is_continuation_row);
    }

    #[test]
    fn format_table_lines_preserves_continuation_prefix_and_alignment() {
        let lines = vec![
            "| name | value |".to_string(),
            "| --- | --- |".to_string(),
            "| alpha | one |".to_string(),
            "|> beta detail | two |".to_string(),
        ];
        let out = format_table_lines(&lines);
        assert_eq!(out[3], "|>beta detail | two   |");
        assert_eq!(out[2], "| alpha       | one   |");
        assert_eq!(
            table_pipe_positions(&out[2]),
            table_pipe_positions(&out[3]),
            "continuation row borders should align with base rows"
        );
    }

    #[test]
    fn format_table_lines_strips_repeated_continuation_marker_artifacts() {
        let lines = vec![
            "| name |".to_string(),
            "| --- |".to_string(),
            "| alpha |".to_string(),
            "|> > detail |".to_string(),
        ];
        let out = format_table_lines(&lines);
        assert_eq!(out[3], "|>detail |");
    }

    #[test]
    fn format_table_lines_keeps_alignment_with_error_literals() {
        let lines = vec![
            "| expr | value |".to_string(),
            "| --- | --- |".to_string(),
            "| :=(1,2) | !ERROR#non_numeric |".to_string(),
            "| ok | 1 |".to_string(),
        ];
        let out = format_table_lines(&lines);
        assert_eq!(out[0], "| expr    | value              |");
        assert_eq!(out[2], "| :=(1,2) | !ERROR#non_numeric |");
        assert_eq!(out[3], "| ok      | 1                  |");
    }
}
