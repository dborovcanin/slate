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

    // if let Some(op) = table_autoformat_rule(&ctx) { return Some(op); }

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
    if selection.head != line.to {
        return None;
    }

    // Try table continuation first
    if let Some(op) = table_continuation_rule(&line) {
        return Some(op);
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
    // A separator contains only |, -, :, and whitespace
    trimmed
        .chars()
        .all(|c| c == '|' || c == '-' || c == ':' || c == ' ')
}

fn table_continuation_rule(line: &crate::editor_core::types::LineContext) -> Option<EditOperation> {
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
    fn run_enter_rules_generates_next_item() {
        let doc = snapshot("- item", 6, 6);
        let op = run_enter_rules(&doc, TextRuleOptions::default()).expect("operation");
        assert_eq!(apply_operation(&doc.text, &op), "- item\n- ");
    }

    #[test]
    fn increment_ordered_marker_handles_top_and_nested_markers() {
        assert_eq!(increment_ordered_marker("1."), "2.");
        assert_eq!(increment_ordered_marker("1.1"), "1.2");
        assert_eq!(increment_ordered_marker("3.2.9"), "3.2.10");
    }
}
