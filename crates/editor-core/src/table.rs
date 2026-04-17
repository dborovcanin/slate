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

pub fn is_table_line(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|')
}

pub fn table_pipe_positions(line: &str) -> Vec<usize> {
    line.match_indices('|').map(|(idx, _)| idx).collect()
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
    if !is_table_line(line) {
        return None;
    }
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('|')?.strip_suffix('|')?;
    Some(inner.split('|').map(|cell| cell.to_string()).collect())
}

pub fn split_table_cells(line: &str) -> Vec<String> {
    split_row_cells_raw(line)
        .unwrap_or_default()
        .into_iter()
        .map(|cell| cell.trim().to_string())
        .collect()
}

pub fn table_column_count(line: &str) -> Option<usize> {
    Some(split_row_cells_raw(line)?.len().max(1))
}

pub fn is_delimiter_cell(cell: &str) -> bool {
    let bytes = cell.trim().as_bytes();
    if bytes.is_empty() {
        return false;
    }

    let mut i = 0usize;
    if bytes[i] == b':' {
        i += 1;
    }

    let dash_start = i;
    while i < bytes.len() && bytes[i] == b'-' {
        i += 1;
    }
    if i.saturating_sub(dash_start) < 3 {
        return false;
    }

    if i < bytes.len() && bytes[i] == b':' {
        i += 1;
    }

    i == bytes.len()
}

pub fn parse_align(cell: &str) -> TableAlign {
    if !is_delimiter_cell(cell) {
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

pub fn normalize_table_row(line: &str) -> Option<String> {
    let raw_cells = split_row_cells_raw(line)?;
    let normalized_cells: Vec<String> = if is_delimiter_row(&raw_cells) {
        raw_cells
            .iter()
            .map(|cell| {
                if cell.trim().is_empty() {
                    "---".to_string()
                } else {
                    normalize_delimiter_cell(cell)
                }
            })
            .collect()
    } else {
        raw_cells
            .iter()
            .map(|cell| cell.trim().to_string())
            .collect()
    };
    Some(serialize_table_row(&normalized_cells))
}

pub fn format_table_lines(lines: &[String]) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }

    let mut rows: Vec<Vec<String>> = Vec::with_capacity(lines.len());
    for line in lines {
        let Some(raw) = split_row_cells_raw(line) else {
            return lines.to_vec();
        };
        rows.push(raw);
    }

    let column_count = rows.iter().map(|row| row.len()).max().unwrap_or(1).max(1);
    let mut normalized_rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|mut row| {
            while row.len() < column_count {
                row.push(String::new());
            }
            row
        })
        .collect();

    let has_delimiter_row = normalized_rows.iter().any(|row| is_delimiter_row(row));
    if !has_delimiter_row && normalized_rows.len() >= 2 {
        normalized_rows.insert(1, vec!["---".to_string(); column_count]);
    }

    let mut normalized_content: Vec<Vec<String>> = Vec::with_capacity(normalized_rows.len());
    let mut widths = vec![0usize; column_count];

    for row in &normalized_rows {
        let delimiter = is_delimiter_row(row);
        let mut out = Vec::with_capacity(column_count);
        for col in 0..column_count {
            let raw = row.get(col).map_or("", |cell| cell.as_str());
            let cell = if delimiter {
                if raw.trim().is_empty() {
                    "---".to_string()
                } else {
                    normalize_delimiter_cell(raw)
                }
            } else {
                raw.trim().to_string()
            };
            // Delimiter dashes auto-stretch to the column width — they must
            // not themselves drive that width or the column can never shrink.
            // Their alignment markers (`:`) still need to fit, so use the
            // marker count (0, 1, or 2) as the minimum delimiter contribution.
            if delimiter {
                let marker_len = cell.chars().filter(|c| *c == ':').count();
                widths[col] = widths[col].max(marker_len);
            } else {
                widths[col] = widths[col].max(cell.len());
            }
            out.push(cell);
        }
        normalized_content.push(out);
    }
    // Delimiter cells must render at least `---` (3 dashes), so enforce a
    // floor on every column width.
    for w in widths.iter_mut() {
        *w = (*w).max(3);
    }

    normalized_rows
        .iter()
        .zip(normalized_content.iter())
        .map(|(raw_row, normalized)| {
            let delimiter = is_delimiter_row(raw_row);
            let mut out = String::new();
            out.push('|');
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
                let pad_right = widths[col].saturating_sub(content.len()) + 1;
                out.push(' ');
                out.push_str(&content);
                out.push_str(&" ".repeat(pad_right));
                out.push('|');
            }
            out
        })
        .collect()
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

    let Some(source_cell_index) = table_cell_index_for_column(&source_pipes, source_col) else {
        return source_col.min(target_line.len());
    };
    let target_cell_index = source_cell_index.min(target_pipes.len().saturating_sub(2));

    let Some(source_cell) = table_cell_span(source_line, &source_pipes, source_cell_index) else {
        return source_col.min(target_line.len());
    };
    let source_left = source_cell.left_pipe + 1;
    let source_in_cell = source_col
        .saturating_sub(source_left)
        .min(source_cell.right_pipe.saturating_sub(source_left));

    let source_content_len = source_cell.trim_end.saturating_sub(source_cell.trim_start);
    let semantic_offset = if source_content_len == 0 {
        0
    } else if source_in_cell <= source_cell.trim_start {
        0
    } else if source_in_cell >= source_cell.trim_end {
        source_content_len
    } else {
        source_in_cell.saturating_sub(source_cell.trim_start)
    };

    let Some(target_cell) = table_cell_span(target_line, &target_pipes, target_cell_index) else {
        return source_col.min(target_line.len());
    };
    let target_left = target_cell.left_pipe + 1;
    let target_content_len = target_cell.trim_end.saturating_sub(target_cell.trim_start);
    if target_content_len == 0 {
        return target_cell.navigation_anchor().min(target_line.len());
    }
    let mapped_in_target = target_cell.trim_start + semantic_offset.min(target_content_len);
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
}
