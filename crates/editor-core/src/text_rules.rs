use crate::context::ResolvedContext;
use crate::operations::replace_range;
use crate::table;
use crate::types::{EditOperation, EditorContextSnapshot, OperationSelection, TextChange};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRuleOptions {
    pub markdown_autoformat: bool,
    pub checklist_auto_reorder: bool,
}

impl Default for TextRuleOptions {
    fn default() -> Self {
        Self {
            markdown_autoformat: true,
            checklist_auto_reorder: true,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableBoundaryEditOptions {
    pub markdown_autoformat: bool,
    pub backward: bool,
    pub structural_merge: bool,
}

impl Default for TableBoundaryEditOptions {
    fn default() -> Self {
        Self {
            markdown_autoformat: true,
            backward: true,
            structural_merge: false,
        }
    }
}

fn is_digits(segment: &str) -> bool {
    !segment.is_empty() && segment.as_bytes().iter().all(|b| b.is_ascii_digit())
}

fn looks_like_month_name(content: &str) -> bool {
    let first = content
        .trim_start()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|ch: char| !ch.is_ascii_alphabetic())
        .to_ascii_lowercase();
    matches!(
        first.as_str(),
        "jan"
            | "january"
            | "feb"
            | "february"
            | "mar"
            | "march"
            | "apr"
            | "april"
            | "may"
            | "jun"
            | "june"
            | "jul"
            | "july"
            | "aug"
            | "august"
            | "sep"
            | "sept"
            | "september"
            | "oct"
            | "october"
            | "nov"
            | "november"
            | "dec"
            | "december"
    )
}

fn is_probable_numeric_date_marker(marker: &str) -> bool {
    let base = marker.strip_suffix('.').unwrap_or(marker);
    let mut iter = base.split('.');
    let (Some(day_s), Some(month_s), Some(year_s), None) =
        (iter.next(), iter.next(), iter.next(), iter.next())
    else {
        return false;
    };
    if !is_digits(day_s) || !is_digits(month_s) || !is_digits(year_s) {
        return false;
    }
    let day = day_s.parse::<u32>().ok().unwrap_or(0);
    let month = month_s.parse::<u32>().ok().unwrap_or(0);
    if !(1..=31).contains(&day) || !(1..=12).contains(&month) {
        return false;
    }
    marker.ends_with('.') || year_s.len() >= 4
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
    let mut marker_is_ordered = false;
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
        marker_is_ordered = true;
        if is_probable_numeric_date_marker(token) {
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

    let marker = &line[marker_start..marker_end];
    let content = &line[i..];
    if marker_is_ordered
        && marker.strip_suffix('.').is_some_and(is_digits)
        && looks_like_month_name(content)
    {
        return None;
    }

    Some(ListLineParts {
        indent: &line[..indent_end],
        marker,
        content,
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

#[derive(Debug, Clone, Copy)]
struct ChecklistLineMeta<'a> {
    parts: ListLineParts<'a>,
    checked: bool,
}

fn parse_checklist_line_meta(line: &str) -> Option<ChecklistLineMeta<'_>> {
    let parts = parse_list_line_parts(line)?;
    let (checked, _) = parse_checklist_after_prefix(parts.content)?;
    Some(ChecklistLineMeta { parts, checked })
}

fn checklist_marker_offset(line: &str) -> Option<usize> {
    let parts = parse_list_line_parts(line)?;
    let rest = &line[parts.prefix_end..];
    parse_checklist_after_prefix(rest)?;
    Some(parts.prefix_end + 1)
}

fn is_checklist_line_with_depth(line: &str, depth: usize) -> bool {
    let Some(meta) = parse_checklist_line_meta(line) else {
        return false;
    };
    marker_depth(meta.parts.indent) == depth
}

fn is_empty_checklist_content(content: &str) -> bool {
    matches!(content.trim(), "[ ]" | "[x]" | "[X]")
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

fn checklist_toggle_from_suffix(
    ctx: &ResolvedContext,
) -> Option<(usize, usize, usize, String, bool)> {
    let selection = ctx.selection();
    if !selection.empty {
        return None;
    }

    let line = ctx.current_line();
    if selection.head != line.to {
        return None;
    }

    let replacement = rewrite_line_with_checklist_toggle_suffix(&line.text)?;
    let checked = parse_checklist_line_meta(&replacement)?.checked;
    Some((line.number, line.from, line.to, replacement, checked))
}

fn checklist_toggle_from_marker_change(
    ctx: &ResolvedContext,
) -> Option<(usize, usize, usize, String, bool)> {
    let changed = ctx.changed_range()?;
    if changed.to <= changed.from {
        return None;
    }

    let line = ctx.line_at(changed.from);
    if changed.from < line.from || changed.to > line.to {
        return None;
    }

    let marker_offset = checklist_marker_offset(&line.text)?;
    let marker_from = line.from + marker_offset;
    let marker_to = marker_from + 1;
    if changed.from > marker_from || changed.to < marker_to {
        return None;
    }

    let checked = parse_checklist_line_meta(&line.text)?.checked;
    Some((line.number, line.from, line.to, line.text.clone(), checked))
}

fn reorder_checklist_toggle(
    ctx: &ResolvedContext,
    line_number: usize,
    replacement: &str,
    move_to_bottom: bool,
) -> Option<EditOperation> {
    let meta = parse_checklist_line_meta(replacement)?;
    let depth = marker_depth(meta.parts.indent);
    let line_count = ctx.line_count();

    let mut start_line = line_number;
    while start_line > 1 && is_checklist_line_with_depth(ctx.line_text(start_line - 1), depth) {
        start_line -= 1;
    }

    let mut end_line = line_number;
    while end_line < line_count && is_checklist_line_with_depth(ctx.line_text(end_line + 1), depth)
    {
        end_line += 1;
    }

    let mut updated_lines = Vec::with_capacity(end_line - start_line + 1);
    let mut original_lines = Vec::with_capacity(end_line - start_line + 1);
    for current in start_line..=end_line {
        let original = ctx.line_text(current).to_string();
        original_lines.push(original.clone());
        if current == line_number {
            updated_lines.push(replacement.to_string());
        } else {
            updated_lines.push(original);
        }
    }

    let source_idx = line_number - start_line;
    let target_idx = if move_to_bottom {
        updated_lines.len().saturating_sub(1)
    } else {
        0
    };

    if source_idx != target_idx {
        let moved = updated_lines.remove(source_idx);
        updated_lines.insert(target_idx, moved);
    }

    let new_text = updated_lines.join("\n");
    let old_text = original_lines.join("\n");
    if new_text == old_text {
        return None;
    }

    let start = ctx.line(start_line);
    let end = ctx.line(end_line);
    let mut anchor = start.from;
    for line_text in updated_lines.iter().take(target_idx) {
        anchor += line_text.len() + 1;
    }
    anchor += updated_lines[target_idx].len();

    Some(replace_range(
        start.from,
        end.to,
        new_text,
        Some(OperationSelection { anchor, head: None }),
    ))
}

fn checklist_toggle_rule(ctx: &ResolvedContext, options: TextRuleOptions) -> Option<EditOperation> {
    let (line_number, line_from, line_to, replacement, checked) =
        checklist_toggle_from_suffix(ctx).or_else(|| checklist_toggle_from_marker_change(ctx))?;

    if options.checklist_auto_reorder {
        if let Some(op) = reorder_checklist_toggle(ctx, line_number, &replacement, checked) {
            return Some(op);
        }
    }

    let original_line = ctx.line_text(line_number);
    if replacement == original_line {
        return None;
    }

    Some(replace_range(
        line_from,
        line_to,
        replacement.clone(),
        Some(OperationSelection {
            anchor: line_from + replacement.len(),
            head: None,
        }),
    ))
}

pub fn format_table_lines(lines: &[String]) -> Vec<String> {
    table::format_table_lines(lines)
}

fn table_autoformat_rule(ctx: &ResolvedContext) -> Option<EditOperation> {
    let mut target_lines: Vec<usize> = Vec::new();
    if let Some(changed) = ctx.changed_range() {
        let start_line = ctx.line_at(changed.from).number;
        let end_line = ctx.line_at(changed.to).number;
        for line_no in start_line..=end_line {
            target_lines.push(line_no);
        }
    } else {
        target_lines.push(ctx.current_line().number);
    }
    target_lines.sort_unstable();
    target_lines.dedup();

    let mut blocks: Vec<(usize, usize)> = Vec::new();
    for line_no in target_lines {
        if !is_table_line(ctx.line_text(line_no)) {
            continue;
        }
        let Some(block) = ctx.table_range_at_line(line_no, 1) else {
            continue;
        };
        blocks.push((block.start_line, block.end_line));
    }
    blocks.sort_unstable();
    blocks.dedup();
    if blocks.is_empty() {
        return None;
    }

    let selection = ctx.selection();
    let cursor_line_no = ctx.line_at(selection.head).number;
    let cursor_line = ctx.line(cursor_line_no);
    let cursor_col = selection
        .head
        .saturating_sub(cursor_line.from)
        .min(cursor_line.text.len());

    let mut changes = Vec::new();
    let mut mapped_selection: Option<OperationSelection> = None;

    for (start_line, end_line) in blocks {
        let original_lines = (start_line..=end_line)
            .map(|line_no| ctx.line_text(line_no).to_string())
            .collect::<Vec<_>>();
        let formatted_lines = table::format_table_lines(&original_lines);
        let formatted_text = formatted_lines.join("\n");
        let original_text = original_lines.join("\n");
        if formatted_text == original_text {
            continue;
        }

        let start = ctx.line(start_line).from;
        let end = ctx.line(end_line).to;
        changes.push(TextChange {
            from: start,
            to: end,
            insert: formatted_text,
        });

        if mapped_selection.is_none()
            && selection.empty
            && cursor_line_no >= start_line
            && cursor_line_no <= end_line
        {
            let source_idx = cursor_line_no.saturating_sub(start_line);
            let had_delimiter = original_lines.iter().any(|line| is_table_separator(line));
            let inserted_delimiter = !had_delimiter
                && original_lines.len() >= 2
                && formatted_lines.len() == original_lines.len() + 1;
            let target_idx = if inserted_delimiter && source_idx >= 1 {
                source_idx + 1
            } else {
                source_idx
            }
            .min(formatted_lines.len().saturating_sub(1));

            let source_line_text = &original_lines[source_idx.min(original_lines.len() - 1)];
            let target_line_text = &formatted_lines[target_idx];
            let mapped_col =
                table::map_table_cursor_column(source_line_text, target_line_text, cursor_col);

            let mut anchor = start;
            for line_text in formatted_lines.iter().take(target_idx) {
                anchor += line_text.len() + 1;
            }
            anchor += mapped_col;
            mapped_selection = Some(OperationSelection { anchor, head: None });
        }
    }

    if changes.is_empty() {
        return None;
    }

    Some(EditOperation {
        changes,
        selection: mapped_selection,
    })
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

    if let Some(op) = checklist_toggle_rule(&ctx, options) {
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
    options: TextRuleOptions,
) -> Option<EditOperation> {
    if !options.markdown_autoformat {
        return None;
    }

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
    table::is_table_line(text)
}

fn is_table_separator(text: &str) -> bool {
    if !table::is_table_line(text) {
        return false;
    }
    let cells = table::split_table_cells(text);
    table::is_delimiter_row(&cells)
}

fn table_continuation_rule(
    line: &crate::types::LineContext,
    selection: &crate::types::SelectionContext,
) -> Option<EditOperation> {
    if !is_table_line(&line.text) {
        return None;
    }
    if is_table_separator(&line.text) {
        return None;
    }

    // Check if the row is empty (only pipes and whitespace)
    let inner: String = line
        .text
        .split('|')
        .skip(1)
        .take(line.text.matches('|').count().saturating_sub(1).max(1))
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
        let pipes = table::table_pipe_positions(&line.text);
        if pipes.len() < 2 {
            return None;
        }
        let head_col = selection
            .head
            .saturating_sub(line.from)
            .min(line.text.len());
        let Some(current_cell) = table::table_cell_index_for_column(&pipes, head_col) else {
            return None;
        };
        let last_cell = pipes.len().saturating_sub(2);
        if current_cell != last_cell {
            return None;
        }
        let last_anchor = table::table_cell_navigation_anchor(&line.text, &pipes, last_cell);
        if head_col < last_anchor {
            return None;
        }
    }

    let empty_row = table::build_empty_table_row_like(&line.text)?;
    let insert = format!("\n{}", empty_row);
    let empty_pipes = table::table_pipe_positions(&empty_row);
    let first_cell_anchor = table::table_cell_navigation_anchor(&empty_row, &empty_pipes, 0);
    let anchor = line.to + 1 + first_cell_anchor; // \n + in-row anchor
    Some(replace_range(
        line.to,
        line.to,
        insert,
        Some(OperationSelection { anchor, head: None }),
    ))
}

fn list_continuation_rule(
    _ctx: &ResolvedContext,
    line: &crate::types::LineContext,
    _selection: &crate::types::SelectionContext,
) -> Option<EditOperation> {
    let parts = parse_list_line_parts(&line.text)?;

    let checklist_empty = parse_checklist_after_prefix(parts.content)
        .map(|(_, content_start)| parts.content[content_start..].trim().is_empty())
        .unwrap_or_else(|| is_empty_checklist_content(parts.content));

    if parts.content.trim().is_empty() || checklist_empty {
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
    if parse_checklist_after_prefix(parts.content).is_some()
        || is_empty_checklist_content(parts.content)
    {
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

fn table_tab_rule(ctx: &ResolvedContext, options: &TabRuleOptions) -> Option<EditOperation> {
    let selection = ctx.selection();
    if !selection.empty {
        return None;
    }

    let start_line_idx = ctx.current_line().number;
    let mut current_line_idx = start_line_idx;
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

        if is_table_separator(&line.text) && current_line_idx != start_line_idx {
            if outdent {
                current_line_idx = current_line_idx.saturating_sub(1);
            } else {
                current_line_idx += 1;
            }
            continue;
        }

        let pipes = table::table_pipe_positions(&line.text);
        if pipes.len() < 2 {
            break;
        }
        let Some(current_cell) = table::table_cell_index_for_column(&pipes, head_col) else {
            break;
        };

        if outdent {
            if current_cell > 0 {
                let pos = table::table_cell_navigation_anchor(&line.text, &pipes, current_cell - 1);
                target_anchor = line.from + pos;
                found_target = true;
                break;
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
            if current_cell + 1 < pipes.len().saturating_sub(1) {
                let pos = table::table_cell_navigation_anchor(&line.text, &pipes, current_cell + 1);
                target_anchor = line.from + pos;
                found_target = true;
                break;
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

pub fn run_table_cell_navigation_rules(
    snapshot: &EditorContextSnapshot,
    options: TabRuleOptions,
) -> Option<EditOperation> {
    if !options.markdown_autoformat {
        return None;
    }
    let ctx = ResolvedContext::new(snapshot.clone());
    table_tab_rule(&ctx, &options)
}

fn prev_char_start(text: &str, at: usize) -> Option<usize> {
    if at == 0 || at > text.len() {
        return None;
    }
    text[..at].char_indices().next_back().map(|(idx, _)| idx)
}

fn next_char_end(text: &str, at: usize) -> Option<usize> {
    if at >= text.len() {
        return None;
    }
    let mut iter = text[at..].char_indices();
    iter.next()?;
    Some(
        iter.next()
            .map(|(idx, _)| at + idx)
            .unwrap_or_else(|| text.len()),
    )
}

fn table_noop_at(line_from: usize, col: usize) -> EditOperation {
    EditOperation {
        changes: vec![],
        selection: Some(OperationSelection {
            anchor: line_from + col,
            head: None,
        }),
    }
}

fn merge_cells_on_line(
    line: &crate::types::LineContext,
    current_cell: usize,
    backward: bool,
) -> Option<EditOperation> {
    let mut cells = table::split_table_cells(&line.text);
    if cells.is_empty() || table::is_delimiter_row(&cells) {
        return None;
    }

    if backward {
        if current_cell == 0 || current_cell >= cells.len() {
            return None;
        }
        let merged = table::merge_cell_content(&cells[current_cell - 1], &cells[current_cell]);
        cells[current_cell - 1] = merged;
        cells.remove(current_cell);
        let new_line = table::serialize_table_row(&cells);
        let pipes = table::table_pipe_positions(&new_line);
        let anchor_col =
            table::table_cell_navigation_anchor(&new_line, &pipes, current_cell.saturating_sub(1));
        return Some(replace_range(
            line.from,
            line.to,
            new_line,
            Some(OperationSelection {
                anchor: line.from + anchor_col,
                head: None,
            }),
        ));
    }

    if current_cell + 1 >= cells.len() {
        return None;
    }
    let merged = table::merge_cell_content(&cells[current_cell], &cells[current_cell + 1]);
    cells[current_cell] = merged;
    cells.remove(current_cell + 1);
    let new_line = table::serialize_table_row(&cells);
    let pipes = table::table_pipe_positions(&new_line);
    let anchor_col = table::table_cell_navigation_anchor(&new_line, &pipes, current_cell);
    Some(replace_range(
        line.from,
        line.to,
        new_line,
        Some(OperationSelection {
            anchor: line.from + anchor_col,
            head: None,
        }),
    ))
}

pub fn run_table_boundary_edit_rules(
    snapshot: &EditorContextSnapshot,
    options: TableBoundaryEditOptions,
) -> Option<EditOperation> {
    if !options.markdown_autoformat {
        return None;
    }

    let ctx = ResolvedContext::new(snapshot.clone());
    let selection = ctx.selection();
    if !selection.empty {
        return None;
    }
    let line = ctx.current_line();
    if !is_table_line(&line.text) {
        return None;
    }

    let pipes = table::table_pipe_positions(&line.text);
    if pipes.len() < 2 {
        return Some(table_noop_at(line.from, 0));
    }

    let head_col = selection
        .head
        .saturating_sub(line.from)
        .min(line.text.len());
    let Some(current_cell) = table::table_cell_index_for_column(&pipes, head_col) else {
        return Some(table_noop_at(line.from, 0));
    };
    let Some(cell) = table::table_cell_span(&line.text, &pipes, current_cell) else {
        return Some(table_noop_at(line.from, head_col));
    };
    let edit_start = cell.edit_start();
    let anchor_col = cell.navigation_anchor();

    if options.structural_merge {
        if options.backward {
            if head_col > edit_start {
                return Some(table_noop_at(line.from, head_col));
            }
            return Some(
                merge_cells_on_line(&line, current_cell, true)
                    .unwrap_or_else(|| table_noop_at(line.from, edit_start)),
            );
        }

        if head_col < anchor_col {
            return Some(table_noop_at(line.from, head_col));
        }
        return Some(
            merge_cells_on_line(&line, current_cell, false)
                .unwrap_or_else(|| table_noop_at(line.from, anchor_col)),
        );
    }

    if options.backward {
        if head_col <= edit_start {
            return Some(table_noop_at(line.from, edit_start));
        }
        if head_col > anchor_col {
            return Some(table_noop_at(line.from, anchor_col));
        }

        let Some(prev_start) = prev_char_start(&line.text, head_col) else {
            return Some(table_noop_at(line.from, edit_start));
        };
        if prev_start < edit_start {
            return Some(table_noop_at(line.from, edit_start));
        }
        return Some(replace_range(
            line.from + prev_start,
            line.from + head_col,
            "",
            Some(OperationSelection {
                anchor: line.from + prev_start,
                head: None,
            }),
        ));
    }

    if head_col < edit_start {
        return Some(table_noop_at(line.from, edit_start));
    }
    if head_col >= anchor_col {
        return Some(table_noop_at(line.from, anchor_col));
    }

    let Some(next_end) = next_char_end(&line.text, head_col) else {
        return Some(table_noop_at(line.from, anchor_col));
    };
    if next_end > anchor_col {
        return Some(table_noop_at(line.from, anchor_col));
    }
    Some(replace_range(
        line.from + head_col,
        line.from + next_end,
        "",
        Some(OperationSelection {
            anchor: line.from + head_col,
            head: None,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SelectionSnapshot, TextRange};

    fn snapshot(text: &str, head: usize, anchor: usize) -> EditorContextSnapshot {
        EditorContextSnapshot {
            text: text.to_string(),
            selection: SelectionSnapshot { anchor, head },
            changed_range: None,
        }
    }

    fn snapshot_with_changed_range(
        text: &str,
        head: usize,
        anchor: usize,
        from: usize,
        to: usize,
    ) -> EditorContextSnapshot {
        EditorContextSnapshot {
            text: text.to_string(),
            selection: SelectionSnapshot { anchor, head },
            changed_range: Some(TextRange { from, to }),
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
    fn run_doc_change_rules_moves_checked_item_to_bottom() {
        let text = "- [ ] first /x\n- [ ] second\n- [x] done";
        let first_line_end = text.find('\n').expect("newline");
        let doc = snapshot(text, first_line_end, first_line_end);
        let op = run_doc_change_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(
            apply_operation(&doc.text, &op),
            "- [ ] second\n- [x] done\n- [x] first"
        );
    }

    #[test]
    fn run_doc_change_rules_moves_unchecked_item_to_top() {
        let text = "- [ ] first\n- [x] second /x\n- [x] third";
        let second_line_end = text.find("\n- [x] third").expect("line suffix");
        let doc = snapshot(text, second_line_end, second_line_end);
        let op = run_doc_change_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(
            apply_operation(&doc.text, &op),
            "- [ ] second\n- [ ] first\n- [x] third"
        );
    }

    #[test]
    fn run_doc_change_rules_can_disable_checklist_reordering() {
        let text = "- [ ] first /x\n- [ ] second\n- [x] done";
        let first_line_end = text.find('\n').expect("newline");
        let doc = snapshot(text, first_line_end, first_line_end);
        let op = run_doc_change_rules(
            &doc,
            TextRuleOptions {
                checklist_auto_reorder: false,
                ..TextRuleOptions::default()
            },
        )
        .expect("operation");
        assert_eq!(
            apply_operation(&doc.text, &op),
            "- [x] first\n- [ ] second\n- [x] done"
        );
    }

    #[test]
    fn run_doc_change_rules_reorders_when_checkbox_marker_is_clicked() {
        let text = "- [x] first\n- [ ] second";
        let marker_from = 3;
        let marker_to = marker_from + 1;
        let doc =
            snapshot_with_changed_range(text, marker_from, marker_from, marker_from, marker_to);
        let op = run_doc_change_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(apply_operation(&doc.text, &op), "- [ ] second\n- [x] first");
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
    fn run_doc_change_rules_normalizes_table_row_spacing_when_enabled() {
        let text = "| a | b |\n| --- | --- |\n|1|2|";
        let head = text.len();
        let doc = snapshot_with_changed_range(text, head, head, head.saturating_sub(1), head);
        let op = run_doc_change_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(
            apply_operation(&doc.text, &op),
            "| a   | b   |\n| --- | --- |\n| 1   | 2   |"
        );
    }

    #[test]
    fn run_doc_change_rules_reflows_table_block_when_cell_becomes_widest() {
        let text = "| a | b |\n| --- | --- |\n| 12345 | 2 |";
        let head = text.find("12345").unwrap() + "12345".len() + 1;
        let doc = snapshot_with_changed_range(
            text,
            head,
            head,
            text.find("| 12345").unwrap(),
            text.len(),
        );
        let op = run_doc_change_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(
            apply_operation(&doc.text, &op),
            "| a     | b   |\n| ----- | --- |\n| 12345 | 2   |"
        );
        assert!(op.selection.is_some());
    }

    #[test]
    fn run_doc_change_rules_inserts_missing_table_delimiter_row() {
        let text = "| test | count |\n| bro | 5 |";
        let head = text.len();
        let doc = snapshot_with_changed_range(text, head, head, text.find("| bro").unwrap(), head);
        let op = run_doc_change_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(
            apply_operation(&doc.text, &op),
            "| test | count |\n| ---- | ----- |\n| bro  | 5     |"
        );
    }

    #[test]
    fn run_doc_change_rules_keeps_empty_cells_with_single_padding() {
        let text = "| first | value |\n| x | |";
        let head = text.len();
        let doc = snapshot_with_changed_range(text, head, head, text.find("| x ").unwrap(), head);
        let op = run_doc_change_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(
            apply_operation(&doc.text, &op),
            "| first | value |\n| ----- | ----- |\n| x     |       |"
        );
    }

    #[test]
    fn run_doc_change_rules_inserts_delimiter_without_overriding_selection() {
        let text = "| test | count |\n|      |       |";
        let input_row_start = text.find("\n|      |       |").unwrap() + 1;
        let head = input_row_start + 2;
        let doc = snapshot_with_changed_range(text, head, head, input_row_start, text.len());
        let op = run_doc_change_rules(&doc, TextRuleOptions::default()).expect("operation");
        let formatted = apply_operation(&doc.text, &op);
        assert_eq!(
            formatted,
            "| test | count |\n| ---- | ----- |\n|      |       |"
        );
        assert!(op.selection.is_some());
    }

    #[test]
    fn run_tab_rules_places_cursor_after_mandatory_left_padding_for_empty_table_cell() {
        let text = "| a   |     |";
        let head = text.find('a').unwrap() + 1;
        let doc = snapshot(text, head, head);
        let op = run_tab_rules(&doc, TabRuleOptions::default()).expect("operation");
        assert_eq!(op.selection.expect("selection").anchor, 8);
    }

    #[test]
    fn run_tab_rules_lands_after_last_word_for_non_empty_table_cells() {
        let text = "| aaa | bb  |";
        let head = text.find('a').unwrap() + 1;
        let doc = snapshot(text, head, head);
        let op = run_tab_rules(&doc, TabRuleOptions::default()).expect("operation");
        assert_eq!(op.selection.expect("selection").anchor, 10);
    }

    #[test]
    fn run_table_boundary_rules_keep_backspace_inside_cell() {
        let text = "| aaa | bb |";
        let doc = snapshot(text, 8, 8); // start of second cell content
        let op = run_table_boundary_edit_rules(
            &doc,
            TableBoundaryEditOptions {
                backward: true,
                ..TableBoundaryEditOptions::default()
            },
        )
        .expect("operation");
        assert!(op.changes.is_empty());
        assert_eq!(op.selection.expect("selection").anchor, 8);

        let doc = snapshot(text, 9, 9); // inside second cell content
        let op = run_table_boundary_edit_rules(
            &doc,
            TableBoundaryEditOptions {
                backward: true,
                ..TableBoundaryEditOptions::default()
            },
        )
        .expect("operation");
        assert_eq!(apply_operation(&doc.text, &op), "| aaa | b |");
    }

    #[test]
    fn run_table_boundary_rules_keep_delete_inside_cell() {
        let text = "| aaa | bb |";
        let doc = snapshot(text, 10, 10); // second cell content end anchor
        let op = run_table_boundary_edit_rules(
            &doc,
            TableBoundaryEditOptions {
                backward: false,
                ..TableBoundaryEditOptions::default()
            },
        )
        .expect("operation");
        assert!(op.changes.is_empty());
        assert_eq!(op.selection.expect("selection").anchor, 10);
    }

    #[test]
    fn run_table_boundary_rules_merge_cells_when_structural_merge_enabled() {
        let text = "| aaa | bb |";
        let merge_prev_doc = snapshot(text, 8, 8); // at start of second cell
        let merge_prev = run_table_boundary_edit_rules(
            &merge_prev_doc,
            TableBoundaryEditOptions {
                backward: true,
                structural_merge: true,
                ..TableBoundaryEditOptions::default()
            },
        )
        .expect("operation");
        assert_eq!(
            apply_operation(&merge_prev_doc.text, &merge_prev),
            "| aaa bb |"
        );

        let merge_next_doc = snapshot(text, 5, 5); // at end of first cell
        let merge_next = run_table_boundary_edit_rules(
            &merge_next_doc,
            TableBoundaryEditOptions {
                backward: false,
                structural_merge: true,
                ..TableBoundaryEditOptions::default()
            },
        )
        .expect("operation");
        assert_eq!(
            apply_operation(&merge_next_doc.text, &merge_next),
            "| aaa bb |"
        );
    }

    #[test]
    fn run_enter_rules_generates_next_item() {
        let doc = snapshot("- item", 6, 6);
        let op = run_enter_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(apply_operation(&doc.text, &op), "- item\n- ");
    }

    #[test]
    fn run_enter_rules_exits_empty_checklist_without_trailing_space() {
        let doc = snapshot("- [ ]", 5, 5);
        let op = run_enter_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(apply_operation(&doc.text, &op), "");
    }

    #[test]
    fn run_enter_rules_exits_empty_checklist_with_trailing_space() {
        let doc = snapshot("- [ ] ", 6, 6);
        let op = run_enter_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(apply_operation(&doc.text, &op), "");
    }

    #[test]
    fn run_enter_rules_continues_non_empty_checklist_item() {
        let doc = snapshot("- [ ] task", 10, 10);
        let op = run_enter_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(apply_operation(&doc.text, &op), "- [ ] task\n- [ ] ");
    }

    #[test]
    fn run_enter_rules_is_disabled_when_markdown_autoformat_is_off() {
        let doc = snapshot("- item", 6, 6);
        let op = run_enter_rules(
            &doc,
            TextRuleOptions {
                markdown_autoformat: false,
                ..TextRuleOptions::default()
            },
        );
        assert!(op.is_none());
    }

    #[test]
    fn run_enter_rules_exits_empty_table_row_when_cursor_is_inside_row() {
        let text = "| a | b |\n| |";
        let doc = snapshot(text, text.len() - 1, text.len() - 1);
        let op = run_enter_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(apply_operation(&doc.text, &op), "| a | b |\n");
    }

    #[test]
    fn run_enter_rules_inserts_empty_table_placeholders_with_single_padding() {
        let text = "| a   | bbbb |\n| --- | ---- |\n| cc  | d    |";
        let doc = snapshot(text, text.len(), text.len());
        let op = run_enter_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(
            apply_operation(&doc.text, &op),
            "| a   | bbbb |\n| --- | ---- |\n| cc  | d    |\n|  |  |"
        );
        assert_eq!(op.selection.expect("selection").anchor, text.len() + 3);
    }

    #[test]
    fn run_enter_rules_inserts_table_row_from_last_cell_anchor_position() {
        let text = "| a   | bbbb |";
        let head = text.find("bbbb").unwrap() + "bbbb".len();
        let doc = snapshot(text, head, head);
        let op = run_enter_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(apply_operation(&doc.text, &op), "| a   | bbbb |\n|  |  |");
        assert_eq!(op.selection.expect("selection").anchor, text.len() + 3);
    }

    #[test]
    fn increment_ordered_marker_handles_top_and_nested_markers() {
        assert_eq!(increment_ordered_marker("1."), "2.");
        assert_eq!(increment_ordered_marker("1.1"), "1.2");
        assert_eq!(increment_ordered_marker("3.2.9"), "3.2.10");
    }

    #[test]
    fn parse_list_line_parts_ignores_date_like_prefixes() {
        assert!(parse_list_line_parts("22.04.2026. 14:34").is_none());
        assert!(parse_list_line_parts("22. April 2026.").is_none());
        assert!(parse_list_line_parts("22. task").is_some());
    }
}
