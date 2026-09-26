use crate::terminal::markdown_view::collapse_inline_markers;
use crate::terminal::text_utils::byte_index;

/// Cell under a cursor; the logical-row fields are not filled in by
/// [`table_cell_info_at_char`].
pub(super) type TableCellInfo = crate::editor_core::table::TableCursorCellInfo;

/// 0-based table cell index containing `cursor_char` (a char offset into
/// `line`), using the same pipe-window rule as the reformatter. None if the
/// cursor is not inside a cell.
pub(super) fn table_cursor_cell_index(line: &str, cursor_char: usize) -> Option<usize> {
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
pub(super) fn display_cell_pipe_positions(
    display: &str,
    cell_idx: usize,
) -> Option<(usize, usize)> {
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

/// Forwards to the canonical `editor_core::table::is_table_line` so the
/// table-line predicate has a single source of truth shared with the rest of
/// the editor core (it previously held a byte-identical copy of that logic).
pub(super) fn is_markdown_table_line(line: &str) -> bool {
    crate::editor_core::table::is_table_line(line)
}

/// Cell of table row `lines[line_idx]` containing char column `col_char`.
/// Looks at that row alone, so it stays cheap on the per-key cursor path.
pub(super) fn table_cell_info_at_char(
    lines: &[String],
    line_idx: usize,
    col_char: usize,
) -> Option<TableCellInfo> {
    let line = lines.get(line_idx)?;
    if !is_markdown_table_line(line) {
        return None;
    }
    crate::editor_core::table::table_cell_info_in_line(line, byte_index(line, col_char))
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

// ── Table display reflow ──────────────────────────────────────────────────────
//
// The raw markdown table is stored with column widths based on raw char counts
// (including inline marker characters like backticks). In the terminal display,
// inline markers are collapsed (backticks hidden, bold/italic applied). To keep
// columns aligned after collapsing, we recompute column widths from visible
// content and re-render each row with the corrected widths.

/// Returns the visible display width of a trimmed cell string after collapsing
/// inline markdown markers (backticks, bold, italic, etc.).
pub(super) fn cell_visible_width(trimmed_cell: &str) -> usize {
    let (collapsed, _) = collapse_inline_markers(trimmed_cell, None);
    // Reuse the same <br>-aware width logic that the raw formatter uses.
    crate::editor_core::table::table_cell_display_width(&collapsed)
}

/// While spaces are typed after a cell's content, the cursor row draws them
/// (see `reformat_table_row_for_display`), so that cell is wider than its
/// column. Returns `(cell_index, width)` the column needs to keep the whole
/// table aligned, or None when the cursor is not past a cell's content.
pub(super) fn cursor_cell_typing_width(line: &str, cursor_char: usize) -> Option<(usize, usize)> {
    use crate::editor_core::table::{split_table_cells, table_pipe_positions};
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
pub(super) fn reformat_table_row_for_display(
    line: &str,
    col_widths: &[usize],
    delimiter: bool,
    cursor_col: Option<usize>,
) -> (String, Option<usize>, Option<(usize, usize)>) {
    reformat_table_row_impl(line, col_widths, delimiter, cursor_col, cursor_col.is_some())
}

/// Raw-marker reflow of the cursor row (markers kept for `render_line`
/// styling). Keeps the same cursor-side whitespace as the collapsed reflow so
/// both displays stay column-aligned.
pub(super) fn reformat_table_cursor_row_raw(
    line: &str,
    col_widths: &[usize],
    delimiter: bool,
    cursor_col: usize,
) -> String {
    reformat_table_row_impl(line, col_widths, delimiter, Some(cursor_col), false).0
}

fn reformat_table_row_impl(
    line: &str,
    col_widths: &[usize],
    is_sep: bool,
    cursor_col: Option<usize>,
    collapse: bool,
) -> (String, Option<usize>, Option<(usize, usize)>) {
    use crate::editor_core::table::{
        is_table_continuation_line, normalize_delimiter_cell_for_width,
        split_table_cells, table_pipe_positions,
    };

    // Work on the trimmed portion so pipe positions are predictable.
    let lead_bytes = line.len() - line.trim_start().len();
    let lead_chars = line[..lead_bytes].chars().count();
    let trimmed = &line[lead_bytes..];

    let pipes = table_pipe_positions(trimmed);
    if pipes.len() < 2 {
        return (line.to_string(), cursor_col, None);
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
    let mut mapped_cursor: Option<usize> = None;
    let mut out_chars = 0usize;

    // Preserve any leading whitespace from the original line.
    out.push_str(&line[..lead_bytes]);
    out_chars += lead_chars;

    // Track output char positions of each opening pipe (for cursor-at-pipe mapping
    // and for returning cursor cell pipe positions to the caller).
    let mut out_pipe_positions: Vec<usize> = Vec::with_capacity(pipes.len());

    for (ci, window) in pipes.windows(2).enumerate() {
        let left_pipe_byte = window[0];
        let right_pipe_byte = window[1];

        // Opening pipe (continuation rows use `|>` for the first column).
        out_pipe_positions.push(out_chars);
        if ci == 0 && is_cont {
            out.push_str("|>");
            out_chars += 2;
        } else {
            out.push('|');
            out_chars += 1;
        }
        out.push(' ');
        out_chars += 1;

        let col_w = col_widths.get(ci).copied().unwrap_or(3).max(1);

        if is_sep {
            // Render the delimiter cell (preserves `:---`, `---:`, `:---:`).
            let raw_cell = cells.get(ci).map(|s| s.as_str()).unwrap_or("---");
            let dashes = normalize_delimiter_cell_for_width(raw_cell, col_w);
            out_chars += dashes.chars().count();
            out.push_str(&dashes);
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

            if collapse && cursor_cell_idx == Some(ci) {
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
                    let (c, mc) = collapse_inline_markers(cell_content, Some(rel_char));
                    if let Some(rel) = mc {
                        mapped_cursor = Some(out_chars + rel);
                    }
                    let w = c.chars().count();
                    (c, w)
                };

                out.push_str(&collapsed);
                out_chars += cell_w;
                let pad = col_w.saturating_sub(cell_w);
                for _ in 0..pad {
                    out.push(' ');
                    out_chars += 1;
                }
            } else if !collapse {
                // Non-cursor line: keep raw cell content so render_line_full
                // can apply inline styling (code, bold, etc.) after hiding
                // markers. Pad based on visible width so columns align.
                let visible_w = cell_visible_width(cell_content) + cursor_trailing_ws;
                let raw_chars = cell_content.chars().count();
                out.push_str(cell_content);
                out_chars += raw_chars;
                for _ in 0..cursor_trailing_ws {
                    out.push(' ');
                    out_chars += 1;
                }
                let pad = col_w.saturating_sub(visible_w);
                for _ in 0..pad {
                    out.push(' ');
                    out_chars += 1;
                }
            } else {
                // Non-cursor cell on cursor line: collapse all markers so the
                // reformatted string can be used for terminal cursor positioning.
                let (collapsed, _) = collapse_inline_markers(cell_content, None);
                let cell_w = collapsed.chars().count();
                out.push_str(&collapsed);
                out_chars += cell_w;
                let pad = col_w.saturating_sub(cell_w);
                for _ in 0..pad {
                    out.push(' ');
                    out_chars += 1;
                }
            }
        }

        out.push(' ');
        out_chars += 1;
    }

    // Trailing pipe.
    out_pipe_positions.push(out_chars);
    out.push('|');
    out_chars += 1;

    // Cursor past the closing pipe (starting the next cell by hand): keep the
    // typed whitespace, and draw the caret at least one column past the pipe,
    // where the next cell's content starts (`| a | ▌`). The first typed
    // character there gets that pad space inserted in front of it.
    if let (Some(cb), Some(&last_pipe)) = (cursor_byte_in_trimmed, pipes.last()) {
        if cb > last_pipe {
            let shown = (cb - last_pipe - 1).max(1);
            out.extend(std::iter::repeat_n(' ', shown));
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

    (out, mapped_cursor, cursor_cell_pipes)
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
        crate::editor_core::table::TableBlockLayout::build(
            block,
            0,
            block.len() - 1,
            cell_visible_width,
        )
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
