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

#[cfg(test)]
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
    parts.push(1);
    format_ordered_marker(&parts)
}

fn outdent_ordered_marker(marker: &str) -> String {
    let Some(mut parts) = parse_ordered_marker_segments(marker) else {
        return marker.to_string();
    };
    if parts.len() > 1 {
        parts.pop();
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

    // Table autoformat and other document-change rules are still placeholders.
    None
}

pub fn run_enter_rules(
    _snapshot: &EditorContextSnapshot,
    _options: TextRuleOptions,
) -> Option<EditOperation> {
    // Placeholder for Rust-native enter key text rules.
    None
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

pub fn run_tab_rules(
    snapshot: &EditorContextSnapshot,
    options: TabRuleOptions,
) -> Option<EditOperation> {
    if !options.markdown_autoformat {
        return None;
    }

    let ctx = ResolvedContext::new(snapshot.clone());
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
        assert_eq!(apply_operation(&indent_doc.text, &indent_op), "  1.1 parent");

        let outdent_doc = snapshot("  1.1.1 child", 13, 0);
        let outdent_op = run_tab_rules(
            &outdent_doc,
            TabRuleOptions {
                markdown_autoformat: true,
                outdent: true,
            },
        )
        .expect("outdent op");
        assert_eq!(apply_operation(&outdent_doc.text, &outdent_op), "1.1 child");
    }

    #[test]
    fn run_enter_rules_placeholder_is_noop() {
        let doc = snapshot("- item", 6, 6);
        assert_eq!(run_enter_rules(&doc, TextRuleOptions::default()), None);
    }

    #[test]
    fn increment_ordered_marker_handles_top_and_nested_markers() {
        assert_eq!(increment_ordered_marker("1."), "2.");
        assert_eq!(increment_ordered_marker("1.1"), "1.2");
        assert_eq!(increment_ordered_marker("3.2.9"), "3.2.10");
    }
}
