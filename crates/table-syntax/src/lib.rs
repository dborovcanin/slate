//! Markdown table structure shared by the editor and the calc engine: row
//! and cell parsing, continuation rows, delimiter rows, and table blocks.
//! Pure functions over line text; no editing or formatting policy.

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

pub fn split_row_cells_raw(line: &str) -> Option<Vec<String>> {
    split_row_cells_raw_with_kind(line).map(|(cells, _)| cells)
}

pub fn strip_continuation_marker(raw: &str) -> String {
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

pub fn split_row_cells_raw_with_kind(line: &str) -> Option<(Vec<String>, bool)> {
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

/// Dash count of a delimiter-shaped cell (`:?-+:?`), or None.
pub fn delimiter_cell_dashes(cell: &str) -> Option<usize> {
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
pub fn after_header_row(row_count: usize, continuation: impl Fn(usize) -> bool) -> Option<usize> {
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

pub fn is_delimiter_row(cells: &[String]) -> bool {
    cells.iter().any(|cell| is_delimiter_cell(cell))
        && cells
            .iter()
            .all(|cell| is_delimiter_cell(cell) || cell.trim().is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn cells_drop_the_continuation_marker_and_keep_escaped_pipes() {
        assert_eq!(split_table_cells("| a | b |"), vec!["a", "b"]);
        assert_eq!(split_table_cells("|> more | b |"), vec!["more", "b"]);
        assert_eq!(split_table_cells(r"| a\|b | c |"), vec![r"a\|b", "c"]);
        assert!(split_table_cells("not a table").is_empty());
    }

    #[test]
    fn delimiter_rows_depend_on_position() {
        let lines = owned(&["| a | b |", "|-|:-:|", "| - | - |", "| --- | --- |"]);
        assert!(is_delimiter_line_in(&lines, 1));
        assert!(!is_delimiter_line_in(&lines, 2), "short dashes later are data");
        assert!(is_delimiter_line_in(&lines, 3), "`---` rows count anywhere");
        let rows: Vec<Vec<String>> = lines.iter().map(|line| split_table_cells(line)).collect();
        assert_eq!(table_block_delimiter_row(&rows, &[false; 4]), Some(1));
    }

    #[test]
    fn block_bounds_cover_contiguous_table_lines() {
        let lines = owned(&["text", "| a |", "|> b |", "| c |", "", "| d |"]);
        assert_eq!(table_block_bounds(&lines, 2), Some((1, 3)));
        assert_eq!(table_block_bounds(&lines, 5), Some((5, 5)));
        assert_eq!(table_block_bounds(&lines, 0), None);
    }
}
