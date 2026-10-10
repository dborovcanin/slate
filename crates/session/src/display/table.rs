use super::markdown::collapse_inline_markers;
fn byte_index(text: &str, column: usize) -> usize {
    text.char_indices()
        .nth(column)
        .map_or(text.len(), |(at, _)| at)
}

/// Cell under a cursor; the logical-row fields are not filled in by
/// [`table_cell_info_at_char`].
pub type TableCellInfo = editor_core::table::TableCursorCellInfo;

/// 0-based table cell index containing `cursor_char` (a char offset into
/// `line`), using the same pipe-window rule as the reformatter. None if the
/// cursor is not inside a cell.
pub fn table_cursor_cell_index(line: &str, cursor_char: usize) -> Option<usize> {
    let pipes: Vec<usize> = line
        .chars()
        .enumerate()
        .filter(|(_, c)| *c == '|')
        .map(|(i, _)| i)
        .collect();
    if pipes.len() < 2 {
        return None;
    }
    pipes
        .windows(2)
        .position(|w| cursor_char > w[0] && cursor_char <= w[1])
}

/// Output char positions of the (left, right) pipe bounding cell `cell_idx` in a
/// reformatted display row. None if the index is out of range.
pub fn display_cell_pipe_positions(display: &str, cell_idx: usize) -> Option<(usize, usize)> {
    let mut left = None;
    let mut seen = 0usize;
    for (i, c) in display.chars().enumerate() {
        if c != '|' {
            continue;
        }
        if seen == cell_idx {
            left = Some(i);
        } else if seen == cell_idx + 1 {
            return left.map(|l| (l, i));
        }
        seen += 1;
    }
    None
}

/// Char index where the trimmed content of cell `cell_idx` starts in a table
/// row. None if the index is out of range.
pub fn table_cell_content_start_char(line: &str, cell_idx: usize) -> Option<usize> {
    use editor_core::table::{is_table_continuation_line, table_pipe_positions};
    let lead = line.len() - line.trim_start().len();
    let trimmed = &line[lead..];
    let pipes = table_pipe_positions(trimmed);
    let (left, right) = (*pipes.get(cell_idx)?, *pipes.get(cell_idx + 1)?);
    let mut raw = &trimmed[left + 1..right];
    if cell_idx == 0 && is_table_continuation_line(trimmed) {
        raw = raw.strip_prefix('>').unwrap_or(raw);
    }
    let content_byte = lead + right - raw.trim_start().len();
    Some(line[..content_byte].chars().count())
}

/// Forwards to the canonical `editor_core::table::is_table_line` so the
/// table-line predicate has a single source of truth shared with the rest of
/// the editor core (it previously held a byte-identical copy of that logic).
pub fn is_markdown_table_line(line: &str) -> bool {
    editor_core::table::is_table_line(line)
}

/// Cell of table row `lines[line_idx]` containing char column `col_char`.
/// Looks at that row alone, so it stays cheap on the per-key cursor path.
pub fn table_cell_info_at_char(
    lines: &[String],
    line_idx: usize,
    col_char: usize,
) -> Option<TableCellInfo> {
    let line = lines.get(line_idx)?;
    if !is_markdown_table_line(line) {
        return None;
    }
    editor_core::table::table_cell_info_in_line(line, byte_index(line, col_char))
}

pub fn table_cell_is_empty(cell: &TableCellInfo) -> bool {
    cell.trim_end <= cell.trim_start
}

pub fn table_cell_edit_start(cell: &TableCellInfo) -> usize {
    (cell.left_pipe + 2).min(cell.right_pipe)
}

pub fn table_cell_navigation_anchor(line: &str, cell: &TableCellInfo) -> usize {
    let edit_start = table_cell_edit_start(cell);
    let anchor_byte = if table_cell_is_empty(cell) {
        edit_start
    } else {
        ((cell.left_pipe + 1) + cell.trim_end).min(cell.right_pipe)
    };
    line[..anchor_byte].chars().count()
}

#[derive(Debug, Clone)]
pub struct TableFormulaSegment {
    pub from_byte: usize,
    pub to_byte: usize,
    pub from_char: usize,
    pub to_char: usize,
    pub cell_from_char: usize,
    pub cell_to_char: usize,
    pub cell_index: usize,
    pub labels: Vec<String>,
}

pub fn builtin_formula_label(text: &str) -> Option<String> {
    editor_core::calc_plan::builtin_formula_label(text)
}

pub fn find_table_formula_segment(text: &str) -> Option<TableFormulaSegment> {
    find_table_formula_segments(text).into_iter().next()
}

pub fn find_table_formula_segments(text: &str) -> Vec<TableFormulaSegment> {
    editor_core::calc_plan::find_table_formula_segments(text)
        .into_iter()
        .map(|seg| TableFormulaSegment {
            from_byte: seg.from_byte,
            to_byte: seg.to_byte,
            from_char: seg.from_char,
            to_char: seg.to_char,
            cell_from_char: seg.cell_left_pipe_char + 1,
            cell_to_char: seg.cell_right_pipe_char,
            cell_index: seg.cell_index,
            labels: seg.labels,
        })
        .collect()
}

pub fn formula_marker_token(index: usize) -> String {
    "*".repeat(index + 1)
}

pub fn should_mask_formula_cell(
    is_cursor_line: bool,
    cursor_col: usize,
    formula: &TableFormulaSegment,
) -> bool {
    !(is_cursor_line && cursor_col >= formula.cell_from_char && cursor_col < formula.cell_to_char)
}

pub fn format_formula_display_value(raw: &str) -> String {
    editor_core::calc_plan::format_formula_display_value(raw)
}

// ── Table display reflow ──────────────────────────────────────────────────────
//
// The raw markdown table is stored with column widths based on raw char counts
// (including inline marker characters like backticks). In the terminal display,
// inline markers are collapsed (backticks hidden, bold/italic applied). To keep
// columns aligned after collapsing, we recompute column widths from visible
// content and re-render each row with the corrected widths.

/// Returns the visible display width of a trimmed cell string after collapsing
/// inline markdown markers (backticks, bold, italic, etc.).
pub fn cell_visible_width(trimmed_cell: &str) -> usize {
    let (collapsed, _) = collapse_inline_markers(trimmed_cell, None);
    // Reuse the same <br>-aware width logic that the raw formatter uses.
    editor_core::table::table_cell_display_width(&collapsed)
}

/// While spaces are typed after a cell's content, the cursor row draws them
/// (see `reformat_table_row_for_display`), so that cell is wider than its
/// column. Returns `(cell_index, width)` the column needs to keep the whole
/// table aligned, or None when the cursor is not past a cell's content.
pub fn cursor_cell_typing_width(line: &str, cursor_char: usize) -> Option<(usize, usize)> {
    use editor_core::table::{split_table_cells, table_pipe_positions};
    let lead = line.len() - line.trim_start().len();
    let trimmed = &line[lead..];
    let pipes = table_pipe_positions(trimmed);
    let cb = byte_index(line, cursor_char).checked_sub(lead)?;
    let ci = pipes.windows(2).position(|w| cb > w[0] && cb <= w[1])?;
    let raw = &trimmed[pipes[ci] + 1..pipes[ci + 1]];
    let content_end = pipes[ci] + 1 + raw.trim_end().len();
    let trailing = cb.checked_sub(content_end).filter(|&n| n > 0)?;
    let cells = split_table_cells(trimmed);
    let content = cells.get(ci).filter(|cell| !cell.is_empty())?;
    let (collapsed, _) = collapse_inline_markers(content, Some(content.chars().count()));
    Some((ci, collapsed.chars().count() + trailing))
}

/// Re-renders a table row for display, adjusting column widths so cells align
/// to their visible widths (after inline markers are collapsed).
///
/// For the cursor line, pass `cursor_col` (raw char position in `line`); the
/// cell containing the cursor reveals its markers the same way regular text
/// does, and non-cursor cells are collapsed so `cursor_line_override` can be
/// used for accurate terminal cursor positioning. For non-cursor lines pass
/// `None` — cell content is kept raw so `render_line_full` can apply normal
/// inline styling (code spans, bold, etc.) after hiding markers.
///
/// Returns `(display_line, mapped_cursor_col, cursor_cell_pipe_out_positions)`.
/// The third element holds `(left_pipe_char, right_pipe_char)` in the output
/// string for the cursor cell; callers use it to redraw `focused_pipe_ranges`
/// from output positions rather than source positions.
///
/// Whitespace typed between the cursor cell's content and the cursor (or
/// after the closing pipe) is kept, so the caret sits where the next typed
/// character lands instead of snapping back onto content or a pipe.
///
/// `delimiter` says whether the row is the table's delimiter row; that depends
/// on the row's position in its block, which only the caller knows.
pub fn reformat_table_row_for_display(
    line: &str,
    col_widths: &[usize],
    delimiter: bool,
    cursor_col: Option<usize>,
) -> (String, Option<usize>, Option<(usize, usize)>) {
    reformat_table_row_impl(
        line,
        col_widths,
        delimiter,
        cursor_col,
        cursor_col.is_some(),
    )
}

/// Raw-marker reflow of the cursor row (markers kept for `render_line`
/// styling). Keeps the same cursor-side whitespace as the collapsed reflow so
/// both displays stay column-aligned.
pub fn reformat_table_cursor_row_raw(
    line: &str,
    col_widths: &[usize],
    delimiter: bool,
    cursor_col: usize,
) -> (String, usize) {
    let (text, cursor, _) =
        reformat_table_row_impl(line, col_widths, delimiter, Some(cursor_col), false);
    (text, cursor.unwrap_or(cursor_col))
}

fn reformat_table_row_impl(
    line: &str,
    col_widths: &[usize],
    is_sep: bool,
    cursor_col: Option<usize>,
    collapse: bool,
) -> (String, Option<usize>, Option<(usize, usize)>) {
    let row = reformat_table_row_mapped(line, col_widths, is_sep, cursor_col, collapse);
    (row.text, row.cursor_col, row.cursor_cell_pipes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &[usize]) -> Vec<usize> {
        s.to_vec()
    }

    #[test]
    fn reformat_plain_row_pads_to_col_widths() {
        let line = "| a | bc |".to_string();
        let (out, cur, _) = reformat_table_row_for_display(&line, &w(&[5, 5]), false, None);
        assert_eq!(out, "| a     | bc    |");
        assert_eq!(cur, None);
    }

    #[test]
    fn reformat_keeps_backtick_markers_on_non_cursor_line() {
        // Non-cursor lines keep raw content so render_line_full can style it.
        let line = "| `code` | plain |".to_string();
        let (out, cur, _) = reformat_table_row_for_display(&line, &w(&[4, 5]), false, None);
        // col_w=4 (visible "code"), raw "`code`" = 6 chars; padding = 4-4 = 0
        assert!(
            out.contains("`code`"),
            "markers kept for styling by renderer"
        );
        assert_eq!(cur, None);
    }

    #[test]
    fn reformat_cursor_in_cell_reveals_markers() {
        // cursor at char 3 (inside `code`)
        let line = "| `code` | plain |".to_string();
        let (out, mc, pipes) = reformat_table_row_for_display(&line, &w(&[6, 5]), false, Some(3));
        assert!(
            out.contains("`code`"),
            "markers should be visible in cursor cell"
        );
        assert!(mc.is_some());
        assert!(pipes.is_some(), "cursor cell pipe positions returned");
    }

    #[test]
    fn reformat_cursor_at_leading_space_maps_to_edit_slot() {
        // cursor at char 1 (the space after the opening |, before cell content)
        let line = "| abc | def |".to_string();
        let (_, mc, _) = reformat_table_row_for_display(&line, &w(&[3, 3]), false, Some(1));
        assert_eq!(mc, Some(2), "cursor at leading space maps inside the cell");
    }

    #[test]
    fn reformat_cursor_in_empty_cell_maps_to_edit_slot() {
        let line = "|      |        |".to_string();
        let (_, mc, _) = reformat_table_row_for_display(&line, &w(&[6, 8]), false, Some(2));
        assert_eq!(mc, Some(2), "empty cell cursor maps after the pipe padding");
    }

    #[test]
    fn reformat_delimiter_row_preserves_alignment_markers() {
        let line = "| :--- | ---: |".to_string();
        let (out, _, _) = reformat_table_row_for_display(&line, &w(&[4, 4]), true, None);
        assert!(out.contains(":---"), "left-align marker preserved");
        assert!(out.contains("---:"), "right-align marker preserved");
    }

    fn visible_widths(block: &[String]) -> Vec<usize> {
        editor_core::table::TableBlockLayout::build(block, 0, block.len() - 1, cell_visible_width)
            .col_widths
    }

    #[test]
    fn table_layout_widths_skip_delimiter_rows() {
        let block = vec![
            "| Header | Long header |".to_string(),
            "| --- | --- |".to_string(),
            "| a | b |".to_string(),
        ];
        assert_eq!(visible_widths(&block), vec![6, 11]);
    }

    #[test]
    fn table_layout_widths_collapse_inline_markers() {
        let block = vec![
            "| `code` | plain |".to_string(),
            "| --- | --- |".to_string(),
            "| b | c |".to_string(),
        ];
        // `code` collapses to "code" (4 chars), not 6 raw chars
        assert_eq!(visible_widths(&block)[0], 4);
    }

    #[test]
    fn table_layout_widths_ignore_short_delimiter_but_not_dash_data() {
        let block = vec![
            "| a | b |".to_string(),
            "|--------|-|".to_string(),
            "| ------ | - |".to_string(),
        ];
        assert_eq!(visible_widths(&block), vec![6, 3]);
    }
}

use super::mapping::{Provenance, Segment, SourceDisplayMap};
#[derive(Clone)]
pub struct MappedTableRow {
    pub text: String,
    pub cursor_col: Option<usize>,
    pub cursor_cell_pipes: Option<(usize, usize)>,
    pub map: SourceDisplayMap,
}
/// Reflow from source syntax with character-column provenance. The caller
/// composes this intermediate source map with formula/wiki substitutions.
pub fn reformat_table_row_mapped(
    line: &str,
    widths: &[usize],
    delimiter: bool,
    cursor: Option<usize>,
    collapse: bool,
) -> MappedTableRow {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    (line, widths, delimiter, cursor, collapse).hash(&mut hasher);
    let key = hasher.finish();
    TABLE_TRANSFORMS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(entry) = cache.entries.get(&key) {
            if entry.source == line
                && entry.widths == widths
                && entry.flags == (delimiter, cursor, collapse)
            {
                return entry.row.clone();
            }
        }
        let row = reformat_table_row_mapped_impl(line, widths, delimiter, cursor, collapse);
        let bytes = line.len()
            + row.text.len()
            + std::mem::size_of_val(widths)
            + row.map.segments.len() * std::mem::size_of::<Segment>();
        if bytes <= TRANSFORM_CACHE_BYTES / 4 {
            if let Some(old) = cache.entries.remove(&key) {
                cache.bytes -= old.bytes;
                cache.order.retain(|old_key| *old_key != key);
            }
            while cache.entries.len() >= TRANSFORM_CACHE_LINES
                || cache.bytes + bytes > TRANSFORM_CACHE_BYTES
            {
                let Some(old_key) = cache.order.pop_front() else {
                    break;
                };
                if let Some(old) = cache.entries.remove(&old_key) {
                    cache.bytes -= old.bytes;
                }
            }
            cache.order.push_back(key);
            cache.bytes += bytes;
            cache.entries.insert(
                key,
                TransformEntry {
                    source: line.to_owned(),
                    widths: widths.to_vec(),
                    flags: (delimiter, cursor, collapse),
                    row: row.clone(),
                    bytes,
                },
            );
        }
        row
    })
}
const TRANSFORM_CACHE_LINES: usize = 128;
const TRANSFORM_CACHE_BYTES: usize = 2 * 1024 * 1024;
struct TransformEntry {
    source: String,
    widths: Vec<usize>,
    flags: (bool, Option<usize>, bool),
    row: MappedTableRow,
    bytes: usize,
}
#[derive(Default)]
struct TransformCache {
    entries: rustc_hash::FxHashMap<u64, TransformEntry>,
    order: std::collections::VecDeque<u64>,
    bytes: usize,
}
thread_local! {
    static TABLE_TRANSFORMS: std::cell::RefCell<TransformCache> = Default::default();
}

pub fn reformat_table_cursor_row_raw_mapped(
    line: &str,
    widths: &[usize],
    delimiter: bool,
    cursor: usize,
) -> (String, usize, SourceDisplayMap) {
    let row = reformat_table_row_mapped(line, widths, delimiter, Some(cursor), false);
    (row.text, row.cursor_col.unwrap_or(cursor), row.map)
}
pub fn reformat_table_row_for_display_mapped(
    line: &str,
    widths: &[usize],
    delimiter: bool,
    cursor: Option<usize>,
) -> (
    String,
    Option<usize>,
    Option<(usize, usize)>,
    SourceDisplayMap,
) {
    let row = reformat_table_row_mapped(line, widths, delimiter, cursor, cursor.is_some());
    (row.text, row.cursor_col, row.cursor_cell_pipes, row.map)
}

// Typed ASCII spaces are exact source copies. Unicode whitespace retains the
// old byte-count display expansion, so its emitted spaces own the source span.
fn map_typed_spaces(
    map: &mut SourceDisplayMap,
    line: &str,
    offsets: &[usize],
    from: usize,
    to: usize,
    shown: usize,
) {
    let source = &line[offsets[from]..offsets[to]];
    if source.bytes().all(|byte| byte == b' ') {
        let copied = (to - from).min(shown);
        if copied > 0 {
            map_emit(map, copied, Provenance::Copied(from..from + copied));
        }
        if shown > copied {
            map_emit(map, shown - copied, Provenance::Owned(to..to));
        }
    } else {
        map_emit(map, shown, Provenance::Owned(from..to));
    }
}

fn map_emit(map: &mut SourceDisplayMap, chars: usize, provenance: Provenance) {
    let start = map.display_len;
    map.display_len += chars;
    if let Some(last) = map.segments.last_mut() {
        if chars > 0
            && !last.display.is_empty()
            && last.display.end == start
            && last.provenance == provenance
        {
            last.display.end += chars;
            return;
        }
    }
    map.segments.push(Segment {
        display: start..start + chars,
        provenance,
    });
}
fn map_inline_cell(
    map: &mut SourceDisplayMap,
    text: &str,
    source: usize,
    cursor: Option<usize>,
) -> usize {
    let hidden = super::markdown::hidden_ranges_for_markdown_line(text, cursor);
    let len = text.chars().count();
    let mut next = 0;
    let before = map.display_len;
    for (from, to) in hidden {
        if from > next {
            map_emit(
                map,
                from - next,
                Provenance::Copied(source + next..source + from),
            );
        }
        map_emit(map, 0, Provenance::Owned(source + from..source + to));
        next = to;
    }
    if next < len {
        map_emit(
            map,
            len - next,
            Provenance::Copied(source + next..source + len),
        );
    }
    map.display_len - before
}
fn reformat_table_row_mapped_impl(
    line: &str,
    col_widths: &[usize],
    is_sep: bool,
    cursor_col: Option<usize>,
    collapse: bool,
) -> MappedTableRow {
    use editor_core::table::{
        is_table_continuation_line, normalize_delimiter_cell_for_width, split_table_cells,
        table_pipe_positions,
    };

    // Work on the trimmed portion so pipe positions are predictable.
    let lead_bytes = line.len() - line.trim_start().len();
    let lead_chars = line[..lead_bytes].chars().count();
    let trimmed = &line[lead_bytes..];

    let pipes = table_pipe_positions(trimmed);
    if pipes.len() < 2 {
        return MappedTableRow {
            text: line.to_owned(),
            cursor_col,
            cursor_cell_pipes: None,
            map: SourceDisplayMap::identity(line),
        };
    }

    let is_cont = is_table_continuation_line(trimmed);
    let cells = split_table_cells(trimmed);

    // Map cursor byte position into `trimmed`.
    let cursor_byte_in_trimmed: Option<usize> = cursor_col.map(|col| {
        let col_in_trimmed = col.saturating_sub(lead_chars);
        byte_index(trimmed, col_in_trimmed)
    });

    // Find which cell (pipe window index) the cursor falls in.
    let cursor_cell_idx: Option<usize> = cursor_byte_in_trimmed
        .and_then(|cb| pipes.windows(2).position(|w| cb > w[0] && cb <= w[1]));

    let mut out = String::with_capacity(line.len() + 16);
    let source_len = line.chars().count();
    let mut map = SourceDisplayMap {
        source_len,
        ..Default::default()
    };
    let offsets: Vec<usize> = line
        .char_indices()
        .map(|(at, _)| at)
        .chain(std::iter::once(line.len()))
        .collect();
    let source_column = |byte: usize| offsets.binary_search(&byte).expect("UTF-8 source boundary");
    let mut mapped_cursor: Option<usize> = None;
    let mut out_chars = 0usize;

    // Preserve any leading whitespace from the original line.
    out.push_str(&line[..lead_bytes]);
    map_emit(&mut map, lead_chars, Provenance::Copied(0..lead_chars));
    out_chars += lead_chars;

    // Track output char positions of each opening pipe (for cursor-at-pipe mapping
    // and for returning cursor cell pipe positions to the caller).
    let mut out_pipe_positions: Vec<usize> = Vec::with_capacity(pipes.len());

    for (ci, window) in pipes.windows(2).enumerate() {
        let left_pipe_byte = window[0];
        let right_pipe_byte = window[1];
        let left_source = source_column(lead_bytes + left_pipe_byte);
        let right_source = source_column(lead_bytes + right_pipe_byte);
        let raw_source = &trimmed[left_pipe_byte + 1..right_pipe_byte];
        let mut source_content = raw_source.trim_start();
        let mut continuation_source = None;
        if ci == 0 && is_cont {
            if let Some(rest) = source_content.strip_prefix('>') {
                continuation_source = Some(source_column(
                    lead_bytes + left_pipe_byte + 1 + raw_source.len() - source_content.len(),
                ));
                source_content = rest.strip_prefix(' ').unwrap_or(rest).trim_start();
            }
        }
        let content_byte = left_pipe_byte + 1 + raw_source.len() - source_content.len();
        let content_source = source_column(lead_bytes + content_byte);
        let cell_anchor = content_source;

        // Opening pipe (continuation rows use `|>` for the first column).
        out_pipe_positions.push(out_chars);
        if ci == 0 && is_cont {
            out.push_str("|>");
            map_emit(
                &mut map,
                1,
                Provenance::Copied(left_source..left_source + 1),
            );
            if let Some(marker) = continuation_source {
                map_emit(&mut map, 0, Provenance::Owned(left_source + 1..marker));
                map_emit(&mut map, 1, Provenance::Copied(marker..marker + 1));
                map_emit(&mut map, 0, Provenance::Owned(marker + 1..content_source));
            } else {
                map_emit(&mut map, 1, Provenance::Owned(cell_anchor..cell_anchor));
            }

            out_chars += 2;
        } else {
            out.push('|');
            map_emit(
                &mut map,
                1,
                Provenance::Copied(left_source..left_source + 1),
            );
            map_emit(
                &mut map,
                0,
                Provenance::Owned(left_source + 1..content_source),
            );
            out_chars += 1;
        }
        out.push(' ');
        map_emit(&mut map, 1, Provenance::Owned(cell_anchor..cell_anchor));
        out_chars += 1;

        let col_w = col_widths.get(ci).copied().unwrap_or(3).max(1);

        let mut trailing_end = content_source;
        if is_sep {
            // Render the delimiter cell (preserves `:---`, `---:`, `:---:`).
            let raw_cell = cells.get(ci).map(|s| s.as_str()).unwrap_or("---");
            let dashes = normalize_delimiter_cell_for_width(raw_cell, col_w);
            out_chars += dashes.chars().count();
            out.push_str(&dashes);
            map_emit(
                &mut map,
                dashes.chars().count(),
                Provenance::Owned(content_source..right_source),
            );
        } else {
            let cell_content = cells.get(ci).map(|s| s.as_str()).unwrap_or("");
            // Spaces typed after the cell's content, up to the cursor.
            let cursor_trailing_ws = match (cursor_cell_idx == Some(ci), cursor_byte_in_trimmed) {
                (true, Some(cb)) if !cell_content.is_empty() => {
                    let raw = &trimmed[left_pipe_byte + 1..right_pipe_byte];
                    let content_end_byte = left_pipe_byte + 1 + raw.trim_end().len();
                    cb.saturating_sub(content_end_byte)
                }
                _ => 0,
            };

            let content_end_source = content_source + cell_content.chars().count();
            trailing_end = content_end_source;
            let typed_end = if cursor_trailing_ws > 0 {
                source_column(lead_bytes + cursor_byte_in_trimmed.unwrap()).min(right_source)
            } else {
                content_end_source
            };
            if collapse && cursor_cell_idx == Some(ci) {
                let mut map_cursor = None;
                // Cursor cell: collapse with cursor-aware marker revealing.
                let cb = cursor_byte_in_trimmed.unwrap_or(left_pipe_byte + 1);
                // Byte offset of the trimmed cell content start within `trimmed`.
                let raw_in_trimmed = &trimmed[left_pipe_byte + 1..right_pipe_byte];
                let trim_lead = raw_in_trimmed.len() - raw_in_trimmed.trim_start().len();
                let content_start_byte = left_pipe_byte + 1 + trim_lead;

                let (collapsed, cell_w) = if cb < content_start_byte {
                    // Cursor is in the cell's leading padding. Keep the
                    // rendered caret in the editable cell slot instead of
                    // snapping it back onto the border pipe.
                    mapped_cursor = Some(out_chars);
                    let (c, _) = collapse_inline_markers(cell_content, None);
                    let w = c.chars().count();
                    (c, w)
                } else if cursor_trailing_ws > 0 {
                    let rel_char = cell_content.chars().count();
                    map_cursor = Some(rel_char);
                    let (mut c, _) = collapse_inline_markers(cell_content, Some(rel_char));
                    c.extend(std::iter::repeat_n(' ', cursor_trailing_ws));
                    let w = c.chars().count();
                    mapped_cursor = Some(out_chars + w);
                    (c, w)
                } else {
                    let rel_byte = cb - content_start_byte;
                    let rel_char = cell_content[..rel_byte.min(cell_content.len())]
                        .chars()
                        .count();
                    map_cursor = Some(rel_char);
                    let (c, mc) = collapse_inline_markers(cell_content, Some(rel_char));
                    if let Some(rel) = mc {
                        mapped_cursor = Some(out_chars + rel);
                    }
                    let w = c.chars().count();
                    (c, w)
                };

                out.push_str(&collapsed);
                let mapped_content_len =
                    map_inline_cell(&mut map, cell_content, content_source, map_cursor);
                map_typed_spaces(
                    &mut map,
                    line,
                    &offsets,
                    content_end_source,
                    typed_end,
                    cell_w.saturating_sub(mapped_content_len),
                );
                trailing_end = typed_end;
                out_chars += cell_w;
                let pad = col_w.saturating_sub(cell_w);
                for _ in 0..pad {
                    out.push(' ');
                    map_emit(
                        &mut map,
                        1,
                        Provenance::Owned(content_end_source..content_end_source),
                    );
                    out_chars += 1;
                }
            } else if !collapse {
                // Non-cursor line: keep raw cell content so render_line_full
                // can apply inline styling (code, bold, etc.) after hiding
                // markers. Pad based on visible width so columns align.
                let visible_w = cell_visible_width(cell_content) + cursor_trailing_ws;
                let raw_chars = cell_content.chars().count();
                if cursor_cell_idx == Some(ci) {
                    let raw = &trimmed[left_pipe_byte + 1..right_pipe_byte];
                    let content_start = left_pipe_byte + 1 + raw.len() - raw.trim_start().len();
                    let cb = cursor_byte_in_trimmed.unwrap_or(content_start);
                    let before_cursor = &trimmed[content_start..cb.max(content_start)];
                    mapped_cursor = Some(out_chars + before_cursor.chars().count());
                }
                out.push_str(cell_content);
                map_emit(
                    &mut map,
                    raw_chars,
                    Provenance::Copied(content_source..content_end_source),
                );
                out_chars += raw_chars;
                for _ in 0..cursor_trailing_ws {
                    out.push(' ');
                    out_chars += 1;
                }
                map_typed_spaces(
                    &mut map,
                    line,
                    &offsets,
                    content_end_source,
                    typed_end,
                    cursor_trailing_ws,
                );
                trailing_end = typed_end;
                let pad = col_w.saturating_sub(visible_w);
                for _ in 0..pad {
                    out.push(' ');
                    map_emit(
                        &mut map,
                        1,
                        Provenance::Owned(content_end_source..content_end_source),
                    );
                    out_chars += 1;
                }
            } else {
                // Non-cursor cell on cursor line: collapse all markers so the
                // reformatted string can be used for terminal cursor positioning.
                let (collapsed, _) = collapse_inline_markers(cell_content, None);
                let cell_w = collapsed.chars().count();
                out.push_str(&collapsed);
                let mapped_content_len =
                    map_inline_cell(&mut map, cell_content, content_source, None);
                map_emit(
                    &mut map,
                    cell_w.saturating_sub(mapped_content_len),
                    Provenance::Owned(content_end_source..content_end_source),
                );
                out_chars += cell_w;
                let pad = col_w.saturating_sub(cell_w);
                for _ in 0..pad {
                    out.push(' ');
                    map_emit(
                        &mut map,
                        1,
                        Provenance::Owned(content_end_source..content_end_source),
                    );
                    out_chars += 1;
                }
            }
        }

        if !is_sep {
            map_emit(&mut map, 0, Provenance::Owned(trailing_end..right_source));
        }
        out.push(' ');
        let end = content_source + cells.get(ci).map_or(0, |cell| cell.chars().count());
        map_emit(&mut map, 1, Provenance::Owned(end..end));
        out_chars += 1;
    }

    // Trailing pipe.
    out_pipe_positions.push(out_chars);
    out.push('|');
    let last_source = source_column(lead_bytes + *pipes.last().unwrap());
    map_emit(
        &mut map,
        1,
        Provenance::Copied(last_source..last_source + 1),
    );
    out_chars += 1;

    // Cursor past the closing pipe (starting the next cell by hand): keep the
    // typed whitespace, and draw the caret at least one column past the pipe,
    // where the next cell's content starts (`| a | ▌`). The first typed
    // character there gets that pad space inserted in front of it.
    let mut suffix_end = last_source + 1;
    if let (Some(cb), Some(&last_pipe)) = (cursor_byte_in_trimmed, pipes.last()) {
        if cb > last_pipe {
            let shown = (cb - last_pipe - 1).max(1);
            out.extend(std::iter::repeat_n(' ', shown));
            suffix_end = source_column(lead_bytes + cb);
            map_typed_spaces(&mut map, line, &offsets, last_source + 1, suffix_end, shown);
            mapped_cursor = Some(out_chars + shown);
        }
    }

    // If cursor wasn't mapped (cursor is at a pipe or leading space), find the
    // nearest pipe position in the output.
    if cursor_col.is_some() && mapped_cursor.is_none() {
        let cb = cursor_byte_in_trimmed.unwrap_or(0);
        // Find the source pipe index closest to the cursor byte.
        let src_pipe_idx = pipes
            .iter()
            .position(|&p| p >= cb)
            .unwrap_or(pipes.len().saturating_sub(1));
        mapped_cursor = out_pipe_positions.get(src_pipe_idx).copied();
    }

    // Return the output pipe positions for the cursor cell so the caller can
    // update focused_pipe_ranges using reformatted positions.
    let cursor_cell_pipes = cursor_cell_idx.and_then(|ci| {
        let lp = out_pipe_positions.get(ci).copied()?;
        let rp = out_pipe_positions.get(ci + 1).copied()?;
        Some((lp, rp))
    });

    if suffix_end < map.source_len {
        map_emit(&mut map, 0, Provenance::Owned(suffix_end..source_len));
    }
    debug_assert_eq!(map.display_len, out.chars().count());
    MappedTableRow {
        text: out,
        cursor_col: mapped_cursor,
        cursor_cell_pipes,
        map,
    }
}

#[cfg(test)]
mod mapped_tests {
    use super::*;
    #[test]
    fn copied_runs_match_exact_unicode_source_and_padding_is_generated() {
        for (line, widths, delimiter, cursor, collapse) in [
            ("  | é **λ** | z |", vec![9, 3], false, None, false),
            ("| é **λ** | z |", vec![9, 3], false, Some(1), true),
            ("|> é **λ** | z |", vec![9, 3], false, None, false),
            ("| :--- | ---: |", vec![9, 3], true, None, false),
            ("| é | z |  ", vec![5, 3], false, Some(12), false),
        ] {
            let mapped = reformat_table_row_mapped(line, &widths, delimiter, cursor, collapse);
            assert_eq!(mapped.map.display_len, mapped.text.chars().count());
            let source: Vec<char> = line.chars().collect();
            let displayed: Vec<char> = mapped.text.chars().collect();
            for segment in &mapped.map.segments {
                if let Provenance::Copied(range) = &segment.provenance {
                    assert_eq!(
                        &source[range.clone()],
                        &displayed[segment.display.clone()],
                        "copied provenance for {line}"
                    );
                }
            }
            assert!(mapped
                .map
                .segments
                .iter()
                .any(|segment| matches!(segment.provenance, Provenance::Owned(_))
                    && !segment.display.is_empty()));
        }
    }
    #[test]
    fn typed_spaces_retain_exact_source_spans_in_both_reflows() {
        for collapse in [false, true] {
            let line = "| é   |  ";
            let row = reformat_table_row_mapped(line, &[7], false, Some(6), collapse);
            assert!(row.map.segments.iter().any(|segment|
                matches!(&segment.provenance, Provenance::Copied(range) if range == &(3..6))));
            let row = reformat_table_row_mapped(line, &[7], false, Some(9), collapse);
            assert!(row.map.segments.iter().any(|segment|
                matches!(&segment.provenance, Provenance::Copied(range) if range == &(7..9))));
        }
    }

    #[test]
    fn cache_keys_include_all_format_inputs_and_stay_bounded() {
        TABLE_TRANSFORMS.with(|cache| *cache.borrow_mut() = TransformCache::default());
        let line = "| **é** |";
        let raw = reformat_table_row_mapped(line, &[4], false, None, false);
        let hidden = reformat_table_row_mapped(line, &[4], false, None, true);
        assert_ne!(raw.text, hidden.text);
        assert_ne!(
            raw.text,
            reformat_table_row_mapped(line, &[12], false, None, false).text
        );
        assert_ne!(
            raw.text,
            reformat_table_row_mapped(line, &[4], true, None, false).text
        );
        assert_ne!(
            hidden.text,
            reformat_table_row_mapped(line, &[4], false, Some(4), true).text
        );
        for index in 0..TRANSFORM_CACHE_LINES * 2 {
            reformat_table_row_mapped(&format!("| {index} |"), &[4], false, None, false);
        }
        TABLE_TRANSFORMS.with(|cache| {
            let cache = cache.borrow();
            assert!(cache.entries.len() <= TRANSFORM_CACHE_LINES);
            assert!(cache.bytes <= TRANSFORM_CACHE_BYTES);
            assert_eq!(cache.order.len(), cache.entries.len());
        });
        let (text, cursor, map) = reformat_table_cursor_row_raw_mapped(line, &[4], false, 4);
        assert_eq!(
            (text, cursor),
            reformat_table_cursor_row_raw(line, &[4], false, 4)
        );
        assert_eq!(map.source_len, line.chars().count());
    }

    #[test]
    fn hidden_inline_markers_have_empty_display_spans() {
        let row = reformat_table_row_mapped("| **é** |", &[6], false, None, true);
        assert!(row.map.segments.iter().any(|s| s.display.is_empty()
            && matches!(&s.provenance,Provenance::Owned(r) if r.end-r.start==2)));
        assert!(row.text.contains('é'));
        assert!(!row.text.contains('*'));
    }
}
