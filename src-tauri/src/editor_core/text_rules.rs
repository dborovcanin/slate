use super::context::ResolvedContext;
use super::operations::replace_range;
use super::types::{EditOperation, EditorContextSnapshot, OperationSelection, TextChange};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRuleOptions {
    pub markdown_autoformat: bool,
}

impl Default for TextRuleOptions {
    fn default() -> Self {
        Self {
            markdown_autoformat: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabRuleOptions {
    pub markdown_autoformat: bool,
    pub outdent: bool,
}

impl Default for TabRuleOptions {
    fn default() -> Self {
        Self {
            markdown_autoformat: true,
            outdent: false,
        }
    }
}

fn is_digits(segment: &str) -> bool {
    !segment.is_empty() && segment.as_bytes().iter().all(|b| b.is_ascii_digit())
}

fn is_ordered_marker_token(marker: &str) -> bool {
    if let Some(stripped) = marker.strip_suffix('.') {
        return is_digits(stripped)
            || (stripped.contains('.') && stripped.split('.').all(is_digits));
    }
    marker.contains('.') && marker.split('.').all(is_digits)
}

fn parse_ordered_marker_segments(marker: &str) -> Option<Vec<u32>> {
    if !is_ordered_marker_token(marker) {
        return None;
    }
    let base = marker.strip_suffix('.').unwrap_or(marker);
    let mut parts = Vec::new();
    for segment in base.split('.') {
        parts.push(segment.parse::<u32>().ok()?);
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts)
}

fn format_ordered_marker(parts: &[u32]) -> String {
    if parts.len() <= 1 {
        return format!("{}.", parts.first().copied().unwrap_or(1));
    }
    parts
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

fn increment_ordered_marker(marker: &str) -> String {
    let Some(mut parts) = parse_ordered_marker_segments(marker) else {
        return marker.to_string();
    };
    let last = parts.len() - 1;
    parts[last] = parts[last].saturating_add(1);
    format_ordered_marker(&parts)
}

fn indent_ordered_marker(marker: &str) -> String {
    let Some(mut parts) = parse_ordered_marker_segments(marker) else {
        return marker.to_string();
    };
    // When indenting 3. into a child, it becomes 2.1 (child of parent 2)
    let last = parts.len() - 1;
    if parts[last] > 1 {
        parts[last] = parts[last].saturating_sub(1);
    }
    parts.push(1);
    format_ordered_marker(&parts)
}

fn outdent_ordered_marker(marker: &str) -> String {
    let Some(mut parts) = parse_ordered_marker_segments(marker) else {
        return marker.to_string();
    };
    if parts.len() > 1 {
        parts.pop();
        // Reverse of indent's decrement: 2.1 outdents to 3.
        let last = parts.len() - 1;
        parts[last] = parts[last].saturating_add(1);
    }
    format_ordered_marker(&parts)
}

fn is_unordered_marker(marker: &str) -> bool {
    matches!(marker, "-" | "*" | "+" | "->")
}

fn unordered_marker_for_depth(depth: usize) -> &'static str {
    if depth == 0 {
        "-"
    } else if depth == 1 {
        "*"
    } else {
        "->"
    }
}

#[derive(Debug, Clone, Copy)]
struct ListLineParts<'a> {
    indent: &'a str,
    marker: &'a str,
    content: &'a str,
    prefix_end: usize,
}

fn parse_list_line_parts(line: &str) -> Option<ListLineParts<'_>> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let indent_end = i;
    if i >= bytes.len() {
        return None;
    }

    let marker_start = i;
    let marker_end = if bytes[i] == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'>' {
        i += 2;
        i
    } else if matches!(bytes[i], b'-' | b'*' | b'+') {
        i += 1;
        i
    } else {
        let mut j = i;
        while j < bytes.len() && !bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j == i {
            return None;
        }
        let token = &line[i..j];
        if !is_ordered_marker_token(token) {
            return None;
        }
        i = j;
        j
    };

    if i >= bytes.len() || !bytes[i].is_ascii_whitespace() {
        return None;
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    Some(ListLineParts {
        indent: &line[..indent_end],
        marker: &line[marker_start..marker_end],
        content: &line[i..],
        prefix_end: i,
    })
}

fn parse_list_prefix_end(line: &str) -> Option<usize> {
    parse_list_line_parts(line).map(|parts| parts.prefix_end)
}

fn parse_checklist_after_prefix(rest: &str) -> Option<(bool, usize)> {
    let bytes = rest.as_bytes();
    if bytes.len() < 4 {
        return None;
    }
    if bytes[0] != b'[' || bytes[2] != b']' {
        return None;
    }
    if !matches!(bytes[1], b' ' | b'x' | b'X') {
        return None;
    }

    let mut i = 3;
    if i >= bytes.len() || !bytes[i].is_ascii_whitespace() {
        return None;
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    Some((matches!(bytes[1], b'x' | b'X'), i))
}

fn strip_checklist_toggle_suffix(content: &str) -> Option<String> {
    let trimmed = content.trim_end();
    if trimmed.len() < 2 {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !lower.ends_with("/x") {
        return None;
    }

    let slash_pos = trimmed.len() - 2;
    if slash_pos > 0 {
        let before = trimmed[..slash_pos].chars().next_back();
        if before.is_some_and(|ch| !ch.is_whitespace()) {
            return None;
        }
    }
    Some(trimmed[..slash_pos].trim_end().to_string())
}

pub fn rewrite_line_with_checklist_toggle_suffix(line_text: &str) -> Option<String> {
    let prefix_end = parse_list_prefix_end(line_text)?;
    let prefix = &line_text[..prefix_end];
    let rest = &line_text[prefix_end..];

    if let Some((checked, content_start)) = parse_checklist_after_prefix(rest) {
        let content = &rest[content_start..];
        let next_content = strip_checklist_toggle_suffix(content)?;
        let next_marker = if checked { " " } else { "x" };
        return Some(format!("{prefix}[{next_marker}] {next_content}"));
    }

    let next_content = strip_checklist_toggle_suffix(rest)?;
    Some(format!("{prefix}[x] {next_content}"))
}

fn checklist_toggle_rule(ctx: &ResolvedContext) -> Option<EditOperation> {
    let selection = ctx.selection();
    if !selection.empty {
        return None;
    }

    let line = ctx.current_line();
    if selection.head != line.to {
        return None;
    }

    let replacement = rewrite_line_with_checklist_toggle_suffix(&line.text)?;
    if replacement == line.text {
        return None;
    }

    Some(replace_range(
        line.from,
        line.to,
        replacement.clone(),
        Some(OperationSelection {
            anchor: line.from + replacement.len(),
            head: None,
        }),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
    None,
}

fn split_table_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let inner = trimmed.trim_start_matches('|').trim_end_matches('|');
    inner.split('|').map(|cell| cell.trim().to_string()).collect()
}

fn is_delimiter_cell(cell: &str) -> bool {
    let bytes = cell.as_bytes();
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

fn parse_align(cell: &str) -> Align {
    if !is_delimiter_cell(cell) {
        return Align::None;
    }
    let left = cell.starts_with(':');
    let right = cell.ends_with(':');
    match (left, right) {
        (true, true) => Align::Center,
        (true, false) => Align::Left,
        (false, true) => Align::Right,
        (false, false) => Align::None,
    }
}

fn delimiter_for_width(width: usize, align: Align) -> String {
    let w = width.max(3);
    match align {
        Align::Left => format!(":{}", "-".repeat(w.saturating_sub(1).max(3))),
        Align::Right => format!("{}:", "-".repeat(w.saturating_sub(1).max(3))),
        Align::Center => format!(":{}:", "-".repeat(w.saturating_sub(2).max(3))),
        Align::None => "-".repeat(w),
    }
}

fn is_delimiter_row(row: &[String]) -> bool {
    row.iter().any(|cell| is_delimiter_cell(cell))
        && row
            .iter()
            .all(|cell| is_delimiter_cell(cell) || cell.is_empty())
}

fn format_table_lines(lines: &[String]) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }

    let rows: Vec<Vec<String>> = lines.iter().map(|line| split_table_cells(line)).collect();
    let column_count = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    if column_count == 0 {
        return lines.to_vec();
    }

    let normalized_rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|mut row| {
            while row.len() < column_count {
                row.push(String::new());
            }
            row
        })
        .collect();

    let mut align = vec![Align::None; column_count];
    for row in &normalized_rows {
        if !is_delimiter_row(row) {
            continue;
        }
        for i in 0..column_count {
            if is_delimiter_cell(&row[i]) {
                align[i] = parse_align(&row[i]);
            }
        }
        break;
    }

    let mut widths = vec![3usize; column_count];
    for row in &normalized_rows {
        if is_delimiter_row(row) {
            continue;
        }
        for i in 0..column_count {
            widths[i] = widths[i].max(row[i].len());
        }
    }

    normalized_rows
        .iter()
        .map(|row| {
            let delimiter = is_delimiter_row(row);
            let mut parts = Vec::with_capacity(column_count);
            for i in 0..column_count {
                if delimiter {
                    parts.push(delimiter_for_width(widths[i], align[i]));
                } else {
                    parts.push(format!("{:<width$}", row[i], width = widths[i]));
                }
            }
            format!("| {} |", parts.join(" | "))
        })
        .collect()
}

fn table_pipe_positions(line: &str) -> Vec<usize> {
    line.match_indices('|').map(|(idx, _)| idx).collect()
}

fn table_cell_index_for_column(pipes: &[usize], col: usize) -> Option<usize> {
    if pipes.len() < 2 {
        return None;
    }
    for i in 0..pipes.len() - 1 {
        if col <= pipes[i + 1] {
            return Some(i);
        }
    }
    Some(pipes.len() - 2)
}

fn first_non_space_offset(text: &str) -> usize {
    text.as_bytes()
        .iter()
        .position(|b| *b != b' ')
        .unwrap_or(text.len())
}

fn last_non_space_end_offset(text: &str) -> usize {
    text.as_bytes()
        .iter()
        .rposition(|b| *b != b' ')
        .map(|idx| idx + 1)
        .unwrap_or(0)
}

fn map_table_cursor_column(source_line: &str, target_line: &str, source_col: usize) -> usize {
    let source_pipes = table_pipe_positions(source_line);
    let target_pipes = table_pipe_positions(target_line);
    if source_pipes.len() < 2 || target_pipes.len() < 2 {
        return source_col.min(target_line.len());
    }

    let Some(source_cell_index) = table_cell_index_for_column(&source_pipes, source_col) else {
        return source_col.min(target_line.len());
    };
    let target_cell_index = source_cell_index.min(target_pipes.len().saturating_sub(2));

    let source_left = source_pipes[source_cell_index] + 1;
    let source_right = source_pipes[source_cell_index + 1];
    let source_raw = &source_line[source_left..source_right];
    let source_trim_start = first_non_space_offset(source_raw);
    let source_trim_end = last_non_space_end_offset(source_raw);
    let source_content_len = source_trim_end.saturating_sub(source_trim_start);
    let source_in_cell = source_col.saturating_sub(source_left).min(source_raw.len());

    let mut semantic_offset = 0usize;
    if source_content_len > 0 {
        semantic_offset = if source_in_cell <= source_trim_start {
            0
        } else if source_in_cell >= source_trim_end {
            source_content_len
        } else {
            source_in_cell - source_trim_start
        };
    }

    let target_left = target_pipes[target_cell_index] + 1;
    let target_right = target_pipes[target_cell_index + 1];
    let target_raw = &target_line[target_left..target_right];
    let target_trim_start = first_non_space_offset(target_raw);
    let target_trim_end = last_non_space_end_offset(target_raw);
    let target_content_len = target_trim_end.saturating_sub(target_trim_start);

    if target_content_len == 0 {
        return (target_left + 1).min(target_right);
    }

    let mapped_in_target = target_trim_start + semantic_offset.min(target_content_len);
    (target_left + mapped_in_target).min(target_line.len())
}

fn table_autoformat_rule(ctx: &ResolvedContext) -> Option<EditOperation> {
    let line = ctx.current_line();
    let block = ctx.table_range_at_line(line.number, 2)?;

    let lines: Vec<String> = (block.start_line..=block.end_line)
        .map(|n| ctx.line_text(n).to_string())
        .collect();
    let formatted = format_table_lines(&lines);
    if formatted
        .iter()
        .zip(lines.iter())
        .all(|(formatted_line, source_line)| formatted_line == source_line)
    {
        return None;
    }

    let head = ctx.selection().head;
    let head_line = ctx.line_at(head).number;
    let head_col = head.saturating_sub(ctx.line(head_line).from);
    let relative_line = head_line
        .saturating_sub(block.start_line)
        .min(formatted.len().saturating_sub(1));
    let source_line = lines
        .get(relative_line)
        .map(String::as_str)
        .unwrap_or_default();
    let target_line = formatted
        .get(relative_line)
        .map(String::as_str)
        .unwrap_or_default();
    let mapped_head_col = if is_table_line(source_line) && is_table_line(target_line) {
        map_table_cursor_column(source_line, target_line, head_col)
    } else {
        head_col.min(target_line.len())
    };

    let start_line = ctx.line(block.start_line);
    let end_line = ctx.line(block.end_line);
    let mut new_head = start_line.from;
    for i in 0..relative_line {
        new_head += formatted[i].len() + 1;
    }
    new_head += mapped_head_col.min(formatted[relative_line].len());

    Some(replace_range(
        start_line.from,
        end_line.to,
        formatted.join("\n"),
        Some(OperationSelection {
            anchor: new_head,
            head: None,
        }),
    ))
}

fn list_autoformat_rule(ctx: &ResolvedContext) -> Option<EditOperation> {
    let line = ctx.current_line();
    let block = ctx.list_range_at_line(line.number)?;

    let mut lines = Vec::new();
    for n in block.start_line..=block.end_line {
        lines.push(ctx.line_text(n));
    }

    let mut formatted = Vec::new();
    let mut expected_hierarchy: Vec<u32> = Vec::new();
    let mut initialized = false;

    for text in &lines {
        if let Some(parts) = parse_list_line_parts(text) {
            let depth = marker_depth(parts.indent);

            if let Some(parsed) = parse_ordered_marker_segments(parts.marker) {
                if !initialized {
                    initialized = true;
                    if expected_hierarchy.len() <= depth {
                        expected_hierarchy.resize(depth + 1, 1);
                    }
                    for (j, &val) in parsed.iter().enumerate().take(depth + 1) {
                        expected_hierarchy[j] = val;
                    }
                    expected_hierarchy[depth] = expected_hierarchy[depth].saturating_sub(1);
                }

                if expected_hierarchy.len() <= depth {
                    expected_hierarchy.resize(depth + 1, 0);
                }
                expected_hierarchy[depth] += 1;
                expected_hierarchy.truncate(depth + 1);

                for j in 0..depth {
                    if expected_hierarchy[j] == 0 {
                        expected_hierarchy[j] = 1;
                    }
                }

                let next_marker = format_ordered_marker(&expected_hierarchy);
                formatted.push(format!("{}{next_marker} {}", parts.indent, parts.content));
            } else {
                formatted.push(text.to_string());
            }
        } else {
            formatted.push(text.to_string());
        }
    }

    if formatted.iter().zip(lines.iter()).all(|(a, b)| a == b) {
        return None;
    }

    let start_line = ctx.line(block.start_line);
    let end_line = ctx.line(block.end_line);

    let head = ctx.selection().head;
    let head_line = ctx.line_at(head).number;
    let head_col = head.saturating_sub(ctx.line(head_line).from);
    let relative_line = head_line
        .saturating_sub(block.start_line)
        .min(formatted.len().saturating_sub(1));

    let mut new_head = start_line.from;
    for i in 0..relative_line {
        new_head += formatted[i].len() + 1; // +1 for newline
    }
    new_head += head_col.min(formatted[relative_line].len());

    Some(replace_range(
        start_line.from,
        end_line.to,
        formatted.join("\n"),
        Some(OperationSelection {
            anchor: new_head,
            head: None,
        }),
    ))
}

pub fn run_doc_change_rules(
    snapshot: &EditorContextSnapshot,
    options: TextRuleOptions,
) -> Option<EditOperation> {
    let ctx = ResolvedContext::new(snapshot.clone());

    if let Some(op) = checklist_toggle_rule(&ctx) {
        return Some(op);
    }

    if !options.markdown_autoformat {
        return None;
    }

    if let Some(op) = table_autoformat_rule(&ctx) {
        return Some(op);
    }

    list_autoformat_rule(&ctx)
}

pub fn run_enter_rules(
    snapshot: &EditorContextSnapshot,
    _options: TextRuleOptions,
) -> Option<EditOperation> {
    let ctx = ResolvedContext::new(snapshot.clone());
    let selection = ctx.selection();
    if !selection.empty {
        return None;
    }

    let line = ctx.current_line();

    // Try table continuation first
    if let Some(op) = table_continuation_rule(&line, &selection) {
        return Some(op);
    }

    if selection.head != line.to {
        return None;
    }

    // Then list continuation
    list_continuation_rule(&ctx, &line, &selection)
}

fn is_table_line(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|')
}

fn is_table_separator(text: &str) -> bool {
    let trimmed = text.trim();
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return false;
    }
    // A separator contains only |, -, :, and whitespace and must contain at least one '-'.
    let mut has_dash = false;
    for ch in trimmed.chars() {
        match ch {
            '|' | ':' | ' ' => {}
            '-' => has_dash = true,
            _ => return false,
        }
    }
    has_dash
}

fn table_continuation_rule(
    line: &crate::editor_core::types::LineContext,
    selection: &crate::editor_core::types::SelectionContext,
) -> Option<EditOperation> {
    if !is_table_line(&line.text) {
        return None;
    }
    if is_table_separator(&line.text) {
        return None;
    }

    let column_count = line.text.matches('|').count().saturating_sub(1).max(1);

    // Check if the row is empty (only pipes and whitespace)
    let inner: String = line
        .text
        .split('|')
        .skip(1)
        .take(column_count)
        .collect::<Vec<_>>()
        .join("");
    if inner.trim().is_empty() {
        return Some(replace_range(
            line.from,
            line.to,
            "",
            Some(OperationSelection {
                anchor: line.from,
                head: None,
            }),
        ));
    }

    if selection.head != line.to {
        return None;
    }

    // Build empty row with matching column count
    let empty_row = format!("|{}", " |".repeat(column_count));
    let insert = format!("\n{}", empty_row);
    let anchor = line.to + 1 + 2; // \n + | + space
    Some(replace_range(
        line.to,
        line.to,
        insert,
        Some(OperationSelection { anchor, head: None }),
    ))
}

fn list_continuation_rule(
    _ctx: &ResolvedContext,
    line: &crate::editor_core::types::LineContext,
    _selection: &crate::editor_core::types::SelectionContext,
) -> Option<EditOperation> {
    let parts = parse_list_line_parts(&line.text)?;

    if parts.content.trim().is_empty() {
        return Some(replace_range(
            line.from,
            line.to,
            "",
            Some(OperationSelection {
                anchor: line.from,
                head: None,
            }),
        ));
    }

    let next_marker = if parse_ordered_marker_segments(parts.marker).is_some() {
        increment_ordered_marker(parts.marker)
    } else if is_unordered_marker(parts.marker) {
        parts.marker.to_string()
    } else {
        return None;
    };

    let mut content_prefix = "";
    if parse_checklist_after_prefix(parts.content).is_some() {
        content_prefix = "[ ] ";
    }

    let insert = format!("\n{}{} {}", parts.indent, next_marker, content_prefix);

    Some(replace_range(
        line.to,
        line.to,
        insert.clone(),
        Some(OperationSelection {
            anchor: line.to + insert.len(),
            head: None,
        }),
    ))
}

fn selection_line_span(ctx: &ResolvedContext) -> (usize, usize) {
    let selection = ctx.selection();
    let start_line = ctx.line_at(selection.from).number;
    let mut end_line = ctx.line_at(selection.to).number;
    if !selection.empty {
        let end_line_ctx = ctx.line_at(selection.to);
        if selection.to == end_line_ctx.from && end_line_ctx.number > start_line {
            end_line = end_line_ctx.number - 1;
        }
    } else {
        end_line = start_line;
    }
    (start_line, end_line)
}

fn marker_depth(indent: &str) -> usize {
    indent
        .as_bytes()
        .iter()
        .take_while(|b| b.is_ascii_whitespace())
        .count()
        / 2
}

fn ensure_cell_anchor_keeps_leading_space(
    line_text: &str,
    pipes: &[usize],
    left_pipe_index: usize,
    anchor_in_line: usize,
) -> usize {
    let Some(&left_pipe) = pipes.get(left_pipe_index) else {
        return anchor_in_line;
    };
    let Some(&right_pipe) = pipes.get(left_pipe_index + 1) else {
        return anchor_in_line;
    };

    let cell_start = left_pipe + 1;
    if anchor_in_line != cell_start || right_pipe <= cell_start {
        return anchor_in_line;
    }
    if line_text.as_bytes().get(cell_start).copied() == Some(b' ') {
        (cell_start + 1).min(right_pipe)
    } else {
        anchor_in_line
    }
}

fn table_tab_rule(ctx: &ResolvedContext, options: &TabRuleOptions) -> Option<EditOperation> {
    let selection = ctx.selection();
    if !selection.empty {
        return None;
    }

    let mut current_line_idx = ctx.current_line().number;
    let mut head_col = selection
        .head
        .saturating_sub(ctx.line(current_line_idx).from);
    let outdent = options.outdent;

    let mut found_target = false;
    let mut target_anchor = 0;

    let line_count = ctx.line_count();
    while current_line_idx >= 1 && current_line_idx <= line_count {
        let line = ctx.line(current_line_idx);
        if !is_table_line(&line.text) {
            break;
        }

        if is_table_separator(&line.text) && current_line_idx != ctx.current_line().number {
            if outdent {
                current_line_idx = current_line_idx.saturating_sub(1);
            } else {
                current_line_idx += 1;
            }
            continue;
        }

        let pipes: Vec<usize> = line.text.match_indices('|').map(|(i, _)| i).collect();
        if pipes.len() < 2 {
            break;
        }

        if outdent {
            let mut left_pipe: Option<usize> = None;
            for i in (0..pipes.len()).rev() {
                if pipes[i] < head_col {
                    left_pipe = Some(i);
                    break;
                }
            }
            if let Some(left) = left_pipe {
                if left > 0 {
                    let target_pipe_index = pipes[left];
                    let mut pos = target_pipe_index;
                    while pos > pipes[left - 1] + 1
                        && line.text.as_bytes().get(pos - 1).copied() == Some(b' ')
                    {
                        pos -= 1;
                    }
                    pos = ensure_cell_anchor_keeps_leading_space(&line.text, &pipes, left - 1, pos);
                    target_anchor = line.from + pos;
                    found_target = true;
                    break;
                }
            }
            current_line_idx = current_line_idx.saturating_sub(1);
            if current_line_idx >= 1 {
                let prev_line = ctx.line(current_line_idx);
                if is_table_line(&prev_line.text) {
                    head_col = prev_line.text.len();
                    continue;
                }
            }
            break;
        } else {
            let mut right_pipe: Option<usize> = None;
            for i in 0..pipes.len() {
                if pipes[i] > head_col {
                    right_pipe = Some(i);
                    break;
                }
            }
            if let Some(right) = right_pipe {
                if right + 1 < pipes.len() {
                    let target_pipe_index = pipes[right + 1];
                    let mut pos = target_pipe_index;
                    while pos > pipes[right] + 1
                        && line.text.as_bytes().get(pos - 1).copied() == Some(b' ')
                    {
                        pos -= 1;
                    }
                    pos = ensure_cell_anchor_keeps_leading_space(&line.text, &pipes, right, pos);
                    target_anchor = line.from + pos;
                    found_target = true;
                    break;
                }
            }
            current_line_idx += 1;
            if current_line_idx <= line_count {
                let next_line = ctx.line(current_line_idx);
                if is_table_line(&next_line.text) {
                    head_col = 0;
                    continue;
                }
            }
            break;
        }
    }

    if found_target {
        return Some(EditOperation {
            changes: vec![],
            selection: Some(OperationSelection {
                anchor: target_anchor,
                head: None,
            }),
        });
    }

    None
}

pub fn run_tab_rules(
    snapshot: &EditorContextSnapshot,
    options: TabRuleOptions,
) -> Option<EditOperation> {
    if !options.markdown_autoformat {
        return None;
    }

    let ctx = ResolvedContext::new(snapshot.clone());

    if let Some(op) = table_tab_rule(&ctx, &options) {
        return Some(op);
    }

    let (start_line, end_line) = selection_line_span(&ctx);
    let mut changes = Vec::new();

    for line_no in start_line..=end_line {
        let line = ctx.line(line_no);
        let Some(parts) = parse_list_line_parts(&line.text) else {
            continue;
        };

        let current_depth = marker_depth(parts.indent);
        let next_depth = if options.outdent {
            current_depth.saturating_sub(1)
        } else {
            current_depth.saturating_add(1)
        };
        let next_indent = " ".repeat(next_depth * 2);

        let next_marker = if parse_ordered_marker_segments(parts.marker).is_some() {
            if options.outdent {
                outdent_ordered_marker(parts.marker)
            } else {
                indent_ordered_marker(parts.marker)
            }
        } else if is_unordered_marker(parts.marker) {
            unordered_marker_for_depth(next_depth).to_string()
        } else {
            parts.marker.to_string()
        };

        let replacement = format!("{next_indent}{next_marker} {}", parts.content);
        if replacement == line.text {
            continue;
        }

        changes.push(TextChange {
            from: line.from,
            to: line.to,
            insert: replacement,
        });
    }

    if changes.is_empty() {
        return None;
    }

    Some(EditOperation {
        changes,
        selection: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_core::types::SelectionSnapshot;

    fn snapshot(text: &str, head: usize, anchor: usize) -> EditorContextSnapshot {
        EditorContextSnapshot {
            text: text.to_string(),
            selection: SelectionSnapshot { anchor, head },
            changed_range: None,
        }
    }

    fn apply_operation(source: &str, operation: &EditOperation) -> String {
        let mut changes = operation.changes.clone();
        changes.sort_by(|a, b| b.from.cmp(&a.from));

        let mut out = source.to_string();
        for change in changes {
            out = format!(
                "{}{}{}",
                &out[..change.from],
                change.insert,
                &out[change.to..]
            );
        }
        out
    }

    #[test]
    fn rewrite_line_with_checklist_toggle_suffix_toggles_and_converts_lists() {
        assert_eq!(
            rewrite_line_with_checklist_toggle_suffix("- [ ] task /x"),
            Some("- [x] task".to_string())
        );
        assert_eq!(
            rewrite_line_with_checklist_toggle_suffix("- [x] task /x"),
            Some("- [ ] task".to_string())
        );
        assert_eq!(
            rewrite_line_with_checklist_toggle_suffix("1.1 task /x"),
            Some("1.1 [x] task".to_string())
        );
        assert_eq!(rewrite_line_with_checklist_toggle_suffix("- path/x"), None);
    }

    #[test]
    fn run_doc_change_rules_applies_checklist_toggle() {
        let text = "- [ ] task /x";
        let doc = snapshot(text, text.len(), text.len());
        let op = run_doc_change_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(apply_operation(&doc.text, &op), "- [x] task");
    }

    #[test]
    fn run_tab_rules_indents_and_outdents_unordered_list_markers() {
        let doc = snapshot("- parent\n  - child\nplain", 16, 0);
        let indent_op = run_tab_rules(&doc, TabRuleOptions::default()).expect("indent op");
        assert_eq!(
            apply_operation(&doc.text, &indent_op),
            "  * parent\n    -> child\nplain"
        );

        let outdent_doc = snapshot("  * parent\n    -> child", 21, 0);
        let outdent_op = run_tab_rules(
            &outdent_doc,
            TabRuleOptions {
                markdown_autoformat: true,
                outdent: true,
            },
        )
        .expect("outdent op");
        assert_eq!(
            apply_operation(&outdent_doc.text, &outdent_op),
            "- parent\n  * child"
        );
    }

    #[test]
    fn run_tab_rules_uses_hierarchical_ordered_markers() {
        let text = "1. parent";
        let indent_doc = snapshot(text, text.len(), text.len());
        let indent_op = run_tab_rules(&indent_doc, TabRuleOptions::default()).expect("indent op");
        assert_eq!(
            apply_operation(&indent_doc.text, &indent_op),
            "  1.1 parent"
        );

        let outdent_doc = snapshot("  1.1.1 child", 13, 0);
        let outdent_op = run_tab_rules(
            &outdent_doc,
            TabRuleOptions {
                markdown_autoformat: true,
                outdent: true,
            },
        )
        .expect("outdent op");
        assert_eq!(apply_operation(&outdent_doc.text, &outdent_op), "1.2 child");
    }

    #[test]
    fn run_doc_change_rules_formats_markdown_tables_when_enabled() {
        let text = "| a | b |\n| --- | --- |\n| 1 | 2 |";
        let doc = snapshot(text, text.len(), text.len());
        let op = run_doc_change_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(
            apply_operation(&doc.text, &op),
            "| a   | b   |\n| --- | --- |\n| 1   | 2   |"
        );
    }

    #[test]
    fn run_tab_rules_keeps_one_leading_space_when_entering_empty_table_cell() {
        let text = "| a   |     |";
        let head = text.find('a').unwrap() + 1;
        let doc = snapshot(text, head, head);
        let op = run_tab_rules(&doc, TabRuleOptions::default()).expect("operation");
        assert_eq!(op.selection.expect("selection").anchor, 8);
    }

    #[test]
    fn run_enter_rules_generates_next_item() {
        let doc = snapshot("- item", 6, 6);
        let op = run_enter_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(apply_operation(&doc.text, &op), "- item\n- ");
    }

    #[test]
    fn run_enter_rules_exits_empty_table_row_when_cursor_is_inside_row() {
        let text = "| a | b |\n| |";
        let doc = snapshot(text, text.len() - 1, text.len() - 1);
        let op = run_enter_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(apply_operation(&doc.text, &op), "| a | b |\n");
    }

    #[test]
    fn increment_ordered_marker_handles_top_and_nested_markers() {
        assert_eq!(increment_ordered_marker("1."), "2.");
        assert_eq!(increment_ordered_marker("1.1"), "1.2");
        assert_eq!(increment_ordered_marker("3.2.9"), "3.2.10");
    }
}
