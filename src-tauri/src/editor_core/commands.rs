use super::command_catalog::{self, CommandId};
use super::context::ResolvedContext;
use super::format::format_markdown;
use super::operations::replace_range;
use super::sum::{
    format_sum_result as format_numeric_result, parse_numbers as parse_numeric_values,
    resolve_scope_range, SumScope,
};
use super::text_rules::{convert_line_to_list, ListKind};
use super::types::{
    BlockLineRange, CommandExecutionResult, CommandMode, CommandSuggestion, EditOperation,
    EditorContextSnapshot, OperationSelection,
};
use regex::Regex;

fn parse_sum_numbers(text: &str) -> Vec<f64> {
    parse_numeric_values(text)
}

fn format_sum_result(value: f64) -> String {
    format_numeric_result(value)
}

struct NoInterrupt;

impl fend_core::Interrupt for NoInterrupt {
    fn should_interrupt(&self) -> bool {
        false
    }
}

static NO_INTERRUPT: NoInterrupt = NoInterrupt;

fn new_fend_context() -> fend_core::Context {
    let mut ctx = fend_core::Context::new();
    ctx.set_random_u32_fn(|| {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        RandomState::new().build_hasher().finish() as u32
    });
    ctx
}

fn evaluate_expression(expr: &str) -> Option<String> {
    let mut ctx = new_fend_context();
    match fend_core::evaluate_with_interrupt(expr, &mut ctx, &NO_INTERRUPT) {
        Ok(result) => Some(result.get_main_result().to_string()),
        Err(_) => None,
    }
}

fn normalize_evaluated_value(raw: &str) -> Option<String> {
    let cleaned = raw
        .replace("approximately", "")
        .replace("approx.", "")
        .replace("approx", "")
        .replace('≈', "")
        .replace('~', "");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return None;
    }

    let matcher = Regex::new(r"[-+]?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?|[-+]?\.\d+").ok()?;
    let found = matcher.find(cleaned)?;
    let numeric_raw = &cleaned[found.start()..found.end()];
    let numeric = numeric_raw.replace(',', "").parse::<f64>().ok()?;
    if !numeric.is_finite() {
        return None;
    }

    let suffix = cleaned[found.end()..].trim();
    let formatted = format_sum_result(numeric);
    if suffix.is_empty() {
        Some(formatted)
    } else {
        Some(format!("{formatted} {suffix}"))
    }
}

fn split_table_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let inner = trimmed.trim_start_matches('|').trim_end_matches('|');
    inner
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn is_table_delimiter_cell(cell: &str) -> bool {
    let trimmed = cell.trim();
    if trimmed.is_empty() {
        return true;
    }

    let without_left = trimmed.strip_prefix(':').unwrap_or(trimmed);
    let core = without_left.strip_suffix(':').unwrap_or(without_left);
    core.len() >= 3 && core.bytes().all(|byte| byte == b'-')
}

fn is_table_delimiter_row(cells: &[String]) -> bool {
    !cells.is_empty() && cells.iter().all(|cell| is_table_delimiter_cell(cell))
}

fn evaluate_cell_term(cell: &str) -> Option<String> {
    let trimmed = cell.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.bytes().any(|byte| byte.is_ascii_digit()) {
        return None;
    }

    if let Some(value) = evaluate_expression(trimmed) {
        if let Some(normalized) = normalize_evaluated_value(&value) {
            return Some(normalized);
        }
    }

    let numbers = parse_sum_numbers(trimmed);
    if numbers.is_empty() {
        return None;
    }
    Some(format_sum_result(numbers.iter().sum()))
}

// Mirrors app_core::calc::engine::parse_plain_numeric_literal.
fn parse_plain_numeric_literal(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let cleaned: String = trimmed
        .chars()
        .filter(|ch| !matches!(ch, ',' | '_'))
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    let mut seen_digit = false;
    let mut seen_dot = false;
    for (idx, ch) in cleaned.chars().enumerate() {
        if ch.is_ascii_digit() {
            seen_digit = true;
            continue;
        }
        if ch == '.' && !seen_dot {
            seen_dot = true;
            continue;
        }
        if matches!(ch, '+' | '-') && idx == 0 {
            continue;
        }
        return None;
    }
    if !seen_digit {
        return None;
    }
    cleaned.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn sum_term_values(terms: &[String]) -> Option<String> {
    if terms.is_empty() {
        return None;
    }
    if terms.len() == 1 {
        return Some(terms[0].clone());
    }

    // Fast path: all plain numerics — sum as f64 directly without fend.
    let nums: Option<Vec<f64>> = terms
        .iter()
        .map(|t| parse_plain_numeric_literal(t))
        .collect();
    if let Some(ns) = nums {
        let sum: f64 = ns.iter().sum();
        return Some(format_sum_result(sum));
    }

    let mut acc = terms[0].clone();
    for term in terms.iter().skip(1) {
        let expr = format!("({acc}) + ({term})");
        let next = evaluate_expression(&expr)?;
        let normalized = normalize_evaluated_value(&next)?;
        acc = normalized;
    }
    Some(acc)
}

fn average_term_values(terms: &[String]) -> Option<String> {
    if terms.is_empty() {
        return None;
    }

    let summed = sum_term_values(terms)?;
    if terms.len() == 1 {
        return Some(summed);
    }

    let expr = format!("({summed}) / {}", terms.len());
    if let Some(value) = evaluate_expression(&expr) {
        if let Some(normalized) = normalize_evaluated_value(&value) {
            return Some(normalized);
        }
    }

    let numeric = parse_sum_numbers(&summed).first().copied()?;
    Some(format_sum_result(numeric / terms.len() as f64))
}

/// Returns the 0-indexed table column the cursor is currently in, or `None` if the
/// cursor is not inside a table row.  Column index is determined by counting `|`
/// separators to the left of the cursor within the current line.
fn cursor_table_column(ctx: &ResolvedContext) -> Option<usize> {
    let line = ctx.current_line();
    if !line.text.trim_start().starts_with('|') {
        return None;
    }
    let col_in_line = ctx
        .cursor_pos()
        .saturating_sub(line.from)
        .min(line.text.len());
    let pipes_before = line.text[..col_in_line]
        .chars()
        .filter(|&c| c == '|')
        .count();
    if pipes_before == 0 {
        return None; // cursor is before the opening '|'
    }
    Some(pipes_before - 1)
}

/// Collects the evaluated cell values to be summed/averaged for a row or column
/// operation.
///
/// - `SumScope::Row`: all cells in the current row that are to the **left** of
///   the cursor column (non-numeric cells are skipped).
/// - `SumScope::Column`: all cells in the cursor column in rows **above** the
///   cursor row (delimiter rows and non-numeric cells are skipped).
///
/// Returns an empty vec if the cursor is not in a table.
fn collect_table_terms(
    ctx: &ResolvedContext,
    scope: SumScope,
    range: BlockLineRange,
) -> Vec<String> {
    let Some(cursor_col) = cursor_table_column(ctx) else {
        return Vec::new();
    };

    if scope == SumScope::Row {
        let cells = split_table_cells(ctx.current_line().text.as_str());
        cells[..cursor_col.min(cells.len())]
            .iter()
            .filter_map(|cell| evaluate_cell_term(cell))
            .collect()
    } else {
        // Column: rows above cursor within the same table.
        let cursor_line = ctx.current_line().number;
        let mut terms = Vec::new();
        for line_no in range.start_line..cursor_line {
            let cells = split_table_cells(ctx.line_text(line_no));
            if is_table_delimiter_row(&cells) {
                continue;
            }
            if let Some(cell) = cells.get(cursor_col) {
                if let Some(val) = evaluate_cell_term(cell) {
                    terms.push(val);
                }
            }
        }
        terms
    }
}

/// Computes the sum or average of `terms`.  Returns an error string if `terms`
/// is empty or if the units are incompatible (fend cannot add them).
fn compute_table_terms(
    terms: Vec<String>,
    op_name: &str,
    scope_name: &str,
    use_avg: bool,
) -> Result<String, String> {
    if terms.is_empty() {
        return Err(format!("{op_name}({scope_name}): no numbers"));
    }
    let result = if use_avg {
        average_term_values(&terms)
    } else {
        sum_term_values(&terms)
    };
    result.ok_or_else(|| format!("{op_name}({scope_name}): incompatible units"))
}

fn scope_for_command_id(command_id: CommandId) -> Option<SumScope> {
    match command_id {
        CommandId::Sum | CommandId::Avg => Some(SumScope::Paragraph),
        CommandId::SumList | CommandId::AvgList => Some(SumScope::List),
        CommandId::SumRow | CommandId::AvgRow => Some(SumScope::Row),
        CommandId::SumColumn | CommandId::AvgColumn => Some(SumScope::Column),
        CommandId::SumDoc | CommandId::AvgDoc => Some(SumScope::Doc),
        _ => None,
    }
}

fn scope_label(scope: SumScope) -> &'static str {
    match scope {
        SumScope::Paragraph => "paragraph",
        SumScope::List => "list",
        SumScope::Row => "row",
        SumScope::Column => "column",
        SumScope::Doc => "doc",
    }
}

fn result_with_message(message: impl Into<String>) -> CommandExecutionResult {
    CommandExecutionResult {
        message: message.into(),
        operations: Vec::new(),
        clipboard_text: None,
        quit_requested: false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListConversionKind {
    Checklist,
    Unordered,
    Ordered,
}

fn list_conversion_label(kind: ListConversionKind) -> &'static str {
    match kind {
        ListConversionKind::Checklist => "checklist",
        ListConversionKind::Unordered => "unordered list",
        ListConversionKind::Ordered => "ordered list",
    }
}

fn convert_line(line: &str, kind: ListConversionKind, ordered_index: usize) -> (String, bool) {
    let list_kind = match kind {
        ListConversionKind::Checklist => ListKind::Checklist,
        ListConversionKind::Unordered => ListKind::Unordered,
        ListConversionKind::Ordered => ListKind::Ordered,
    };
    convert_line_to_list(line, list_kind, ordered_index)
}

fn run_list_convert_command(
    snapshot: &EditorContextSnapshot,
    kind: ListConversionKind,
    mode: CommandMode,
) -> CommandExecutionResult {
    let ctx = ResolvedContext::new(snapshot.clone());
    let selection = ctx.selection();
    let (start_line, end_line) = if selection.empty {
        let line = ctx.line_at(selection.head).number;
        (line, line)
    } else if mode == CommandMode::Vim {
        let anchor_line = ctx.line_at(selection.anchor).number;
        let head_line = ctx.line_at(selection.head).number;
        (anchor_line.min(head_line), anchor_line.max(head_line))
    } else {
        let start = ctx.line_at(selection.from).number;
        let end_cursor = selection.from.max(selection.to.saturating_sub(1));
        let end = ctx.line_at(end_cursor).number;
        (start, end)
    };

    let mut converted = Vec::new();
    let mut changed = 0usize;
    let mut ordered_index = 1usize;
    for line_no in start_line..=end_line {
        let source = ctx.line_text(line_no);
        let (next, converted_line) = convert_line(source, kind, ordered_index);
        if kind == ListConversionKind::Ordered && converted_line {
            ordered_index += 1;
        }
        if next != source {
            changed += 1;
        }
        converted.push(next);
    }

    if changed == 0 {
        return result_with_message(format!("already {}", list_conversion_label(kind)));
    }

    let from = ctx.line(start_line).from;
    let to = ctx.line(end_line).to;
    let insert = converted.join("\n");
    let anchor = from + insert.len();
    let op = replace_range(
        from,
        to,
        insert,
        Some(OperationSelection { anchor, head: None }),
    );
    let label = list_conversion_label(kind);
    let message = if changed == 1 {
        format!("converted 1 line to {label}")
    } else {
        format!("converted {changed} lines to {label}")
    };
    let mut result = result_with_message(message);
    result.operations.push(op);
    result
}

pub fn list_command_suggestions(mode: CommandMode, raw_input: &str) -> Vec<CommandSuggestion> {
    command_catalog::list_command_suggestions(mode, raw_input)
}

/// Replaces the content of the table cell the cursor is currently in with `value`,
/// padded with a single space on each side.  Falls back to a plain insert at the
/// cursor if the cursor is not between two `|` separators on the current line.
fn replace_table_cell_at_cursor(snapshot: &EditorContextSnapshot, value: &str) -> EditOperation {
    let ctx = ResolvedContext::new(snapshot.clone());
    let line = ctx.current_line();
    let cursor_in_line = snapshot
        .selection
        .anchor
        .min(snapshot.selection.head)
        .saturating_sub(line.from)
        .min(line.text.len());

    let left_pipe = line.text[..cursor_in_line].rfind('|');
    let right_pipe = line.text[cursor_in_line..]
        .find('|')
        .map(|i| cursor_in_line + i);

    if let (Some(lp), Some(rp)) = (left_pipe, right_pipe) {
        let cell_start = line.from + lp + 1; // byte after the left '|'
        let cell_end = line.from + rp; // byte of the right '|'
        let formatted = format!(" {} ", value);
        let next_anchor = cell_start + formatted.len();
        return replace_range(
            cell_start,
            cell_end,
            &formatted,
            Some(OperationSelection {
                anchor: next_anchor,
                head: None,
            }),
        );
    }

    // Fallback: no surrounding pipes found — just insert at cursor.
    insert_value_at_selection(snapshot, value)
}

pub fn insert_value_at_selection(snapshot: &EditorContextSnapshot, value: &str) -> EditOperation {
    let max = snapshot.text.len();
    let anchor = snapshot.selection.anchor.min(max);
    let head = snapshot.selection.head.min(max);
    let from = anchor.min(head);
    let to = anchor.max(head);
    let next_anchor = from + value.len();
    replace_range(
        from,
        to,
        value,
        Some(OperationSelection {
            anchor: next_anchor,
            head: None,
        }),
    )
}

pub fn execute_command(
    snapshot: &EditorContextSnapshot,
    raw_input: &str,
    mode: CommandMode,
) -> CommandExecutionResult {
    let normalized = command_catalog::normalize_command(raw_input);
    if normalized.is_empty() {
        return result_with_message("");
    }

    let Some(command) = command_catalog::resolve_command(mode, raw_input) else {
        return result_with_message(format!("unknown command: {normalized}"));
    };

    match command.id {
        CommandId::Quit => CommandExecutionResult {
            message: "quit".to_string(),
            operations: Vec::new(),
            clipboard_text: None,
            quit_requested: true,
        },
        CommandId::Sum
        | CommandId::SumList
        | CommandId::SumRow
        | CommandId::SumColumn
        | CommandId::SumDoc => {
            let ctx = ResolvedContext::new(snapshot.clone());
            let Some(scope) = scope_for_command_id(command.id) else {
                return result_with_message(format!("unknown command: {normalized}"));
            };
            let scope_name = scope_label(scope);
            let Some(range) = resolve_scope_range(&ctx, scope) else {
                return result_with_message(format!("sum({scope_name}): no block at cursor"));
            };

            if scope == SumScope::Row || scope == SumScope::Column {
                let terms = collect_table_terms(&ctx, scope, range);
                match compute_table_terms(terms, "sum", scope_name, false) {
                    Ok(total) => {
                        let op = replace_table_cell_at_cursor(snapshot, &total);
                        let mut result =
                            result_with_message(format!("sum({scope_name}) = {total}"));
                        result.operations.push(op);
                        result.clipboard_text = Some(total);
                        return result;
                    }
                    Err(msg) => return result_with_message(msg),
                }
            }

            let text = ctx.text_for_line_range(range);
            let numbers = parse_sum_numbers(&text);
            if numbers.is_empty() {
                return result_with_message(format!("sum({scope_name}): no numbers"));
            }
            let sum: f64 = numbers.iter().sum();
            let formatted = format_sum_result(sum);
            let msg = format!(
                "sum({scope_name}) = {formatted} ({} values, inserted + copied)",
                numbers.len()
            );
            let op = insert_value_at_selection(snapshot, &formatted);
            let mut result = result_with_message(msg);
            result.operations.push(op);
            result.clipboard_text = Some(formatted);
            result
        }
        CommandId::Avg
        | CommandId::AvgList
        | CommandId::AvgRow
        | CommandId::AvgColumn
        | CommandId::AvgDoc => {
            let ctx = ResolvedContext::new(snapshot.clone());
            let Some(scope) = scope_for_command_id(command.id) else {
                return result_with_message(format!("unknown command: {normalized}"));
            };
            let scope_name = scope_label(scope);
            let Some(range) = resolve_scope_range(&ctx, scope) else {
                return result_with_message(format!("avg({scope_name}): no block at cursor"));
            };

            if scope == SumScope::Row || scope == SumScope::Column {
                let terms = collect_table_terms(&ctx, scope, range);
                match compute_table_terms(terms, "avg", scope_name, true) {
                    Ok(avg) => {
                        let op = replace_table_cell_at_cursor(snapshot, &avg);
                        let mut result = result_with_message(format!("avg({scope_name}) = {avg}"));
                        result.operations.push(op);
                        result.clipboard_text = Some(avg);
                        return result;
                    }
                    Err(msg) => return result_with_message(msg),
                }
            }

            let text = ctx.text_for_line_range(range);
            let numbers = parse_sum_numbers(&text);
            if numbers.is_empty() {
                return result_with_message(format!("avg({scope_name}): no numbers"));
            }
            let avg: f64 = numbers.iter().sum::<f64>() / numbers.len() as f64;
            let formatted = format_sum_result(avg);
            let msg = format!(
                "avg({scope_name}) = {formatted} ({} values, inserted + copied)",
                numbers.len()
            );
            let op = insert_value_at_selection(snapshot, &formatted);
            let mut result = result_with_message(msg);
            result.operations.push(op);
            result.clipboard_text = Some(formatted);
            result
        }
        CommandId::Date => {
            let date_str = time::OffsetDateTime::now_utc().date().to_string();
            let mut result = result_with_message("Date inserted");
            result
                .operations
                .push(insert_value_at_selection(snapshot, &date_str));
            result
        }
        CommandId::Notify => result_with_message("notify handled by host"),
        CommandId::NotifyDelete => result_with_message("notify-delete handled by host"),
        CommandId::ClipWatch => result_with_message("clip-watch handled by host"),
        CommandId::ClipWatchStop => result_with_message("clip-watch-stop handled by host"),
        CommandId::Fold => result_with_message("fold handled by host"),
        CommandId::Unfold => result_with_message("unfold handled by host"),
        CommandId::FoldToggle => result_with_message("fold-toggle handled by host"),
        CommandId::Format => {
            let mut formatted = format_markdown(&snapshot.text);
            if snapshot.text.ends_with('\n') && !formatted.ends_with('\n') {
                formatted.push('\n');
            }
            let op = replace_range(
                0,
                snapshot.text.len(),
                formatted,
                Some(OperationSelection {
                    anchor: snapshot.selection.anchor,
                    head: Some(snapshot.selection.head),
                }),
            );
            let mut result = result_with_message("Document formatted");
            result.operations.push(op);
            result
        }
        CommandId::Checklist | CommandId::UnorderedList | CommandId::OrderedList => {
            let kind = match command.id {
                CommandId::Checklist => ListConversionKind::Checklist,
                CommandId::UnorderedList => ListConversionKind::Unordered,
                CommandId::OrderedList => ListConversionKind::Ordered,
                _ => unreachable!(),
            };
            run_list_convert_command(snapshot, kind, mode)
        }
    }
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

    #[test]
    fn suggestions_are_mode_aware() {
        let editor_values = list_command_suggestions(CommandMode::Editor, "")
            .into_iter()
            .map(|entry| entry.value)
            .collect::<Vec<_>>();
        assert_eq!(
            editor_values,
            vec![
                "sum",
                "sum list",
                "sum row",
                "sum column",
                "sum doc",
                "avg",
                "avg list",
                "avg row",
                "avg column",
                "avg doc",
                "date",
                "notify",
                "notify-delete",
                "format",
                "clip-watch",
                "clip-watch-stop",
                "fold",
                "unfold",
                "fold-toggle",
                "clist",
                "ulist",
                "olist",
            ]
        );

        let vim_values = list_command_suggestions(CommandMode::Vim, "")
            .into_iter()
            .map(|entry| entry.value)
            .collect::<Vec<_>>();
        assert!(vim_values.contains(&"q".to_string()));
    }

    #[test]
    fn execute_command_handles_unknown_and_mode_gated_quit() {
        let doc = snapshot("", 0, 0);
        let unknown = execute_command(&doc, "missing", CommandMode::Editor);
        assert_eq!(unknown.message, "unknown command: missing");
        assert!(unknown.operations.is_empty());

        let editor_q = execute_command(&doc, "q", CommandMode::Editor);
        assert_eq!(editor_q.message, "unknown command: q");

        let vim_q = execute_command(&doc, "q", CommandMode::Vim);
        assert_eq!(vim_q.message, "quit");
        assert!(vim_q.quit_requested);
    }

    #[test]
    fn insert_value_replaces_selection_and_moves_cursor_to_end() {
        let doc = snapshot("hello world", 11, 6);
        let op = insert_value_at_selection(&doc, "planet");
        assert_eq!(op.changes.len(), 1);
        assert_eq!(op.changes[0].from, 6);
        assert_eq!(op.changes[0].to, 11);
        assert_eq!(op.changes[0].insert, "planet");
        assert_eq!(
            op.selection,
            Some(OperationSelection {
                anchor: 12,
                head: None
            })
        );
    }

    #[test]
    fn parse_sum_numbers_extracts_values() {
        assert_eq!(
            parse_sum_numbers("a 10 b -2.5 c 1,200 d +0.75"),
            vec![10.0, -2.5, 1200.0, 0.75]
        );
        assert_eq!(parse_sum_numbers("no numbers here"), Vec::<f64>::new());
        assert_eq!(parse_sum_numbers("3"), vec![3.0]);
    }

    #[test]
    fn format_sum_result_clean_output() {
        assert_eq!(format_sum_result(4.0), "4.00");
        assert_eq!(format_sum_result(3.5), "3.50");
        assert_eq!(format_sum_result(0.0), "0.00");
        assert_eq!(format_sum_result(1234.0), "1234.00");
    }

    #[test]
    fn normalize_evaluated_value_strips_approx_and_rounds() {
        assert_eq!(
            normalize_evaluated_value("approximately 2.004 km"),
            Some("2.00 km".to_string())
        );
        assert_eq!(
            normalize_evaluated_value("≈ 7.006m"),
            Some("7.01 m".to_string())
        );
    }

    #[test]
    fn sum_command_inserts_at_selection() {
        let doc = snapshot("item 10\nitem 20\nitem 30", 0, 0);
        let result = execute_command(&doc, "sum", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        let op = &result.operations[0];
        assert_eq!(op.changes[0].insert, "60.00");
        assert!(result.message.contains("60.00"));
        assert!(result.message.contains("3 values"));
        assert_eq!(result.clipboard_text, Some("60.00".to_string()));
    }

    #[test]
    fn sum_doc_sums_entire_document() {
        let doc = snapshot("1\n2\n3", 0, 0);
        let result = execute_command(&doc, "sum doc", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        assert!(result.message.contains("6.00"));
    }

    #[test]
    fn avg_command_inserts_at_selection() {
        let doc = snapshot("item 10\nitem 20\nitem 30", 0, 0);
        let result = execute_command(&doc, "avg", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        let op = &result.operations[0];
        assert_eq!(op.changes[0].insert, "20.00");
        assert!(result.message.contains("avg(paragraph) = 20.00"));
        assert_eq!(result.clipboard_text, Some("20.00".to_string()));
    }

    // --- sum row / avg row ---
    //
    // Cursor is placed inside an empty trailing cell; all data cells to the left
    // are summed and the result replaces the empty cell content.

    #[test]
    fn sum_row_sums_cells_left_of_cursor() {
        // Cursor inside the empty trailing cell — data cells are item (skip), 2m, 2km.
        let table = "| item | 2m  | 2km |  |";
        let cursor = table.rfind("|  |").unwrap() + 1; // inside the empty last cell
        let doc = snapshot(table, cursor, cursor);
        let result = execute_command(&doc, "sum row", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        assert!(result.message.contains("sum(row)"));
        // 2m + 2km = 2002m; cell is replaced so insert is trimmed value with padding
        assert_eq!(
            result.operations[0].changes[0].insert.replace(' ', ""),
            "2002.00m"
        );
    }

    #[test]
    fn avg_row_averages_cells_left_of_cursor() {
        // Cursor in empty trailing cell; data cells x (skip), 3m, 4m → avg = 3.5m.
        let table = "| x | 3m | 4m |  |";
        let cursor = table.rfind("|  |").unwrap() + 1;
        let doc = snapshot(table, cursor, cursor);
        let result = execute_command(&doc, "avg row", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        assert!(result.message.contains("avg(row)"));
        assert_eq!(
            result.operations[0].changes[0].insert.replace(' ', ""),
            "3.50m"
        );
    }

    #[test]
    fn sum_row_skips_non_numeric_cells() {
        // Cursor in empty trailing cell; label (skip), 10, 20 → sum = 30.
        let table = "| label | 10 | 20 |  |";
        let cursor = table.rfind("|  |").unwrap() + 1;
        let doc = snapshot(table, cursor, cursor);
        let result = execute_command(&doc, "sum row", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        assert_eq!(result.operations[0].changes[0].insert.trim(), "30.00");
    }

    #[test]
    fn sum_row_reports_incompatible_units() {
        // Plain number + time duration → incompatible units error, no edit applied.
        let table = "| 34 | 3h |  |";
        let cursor = table.rfind("|  |").unwrap() + 1;
        let doc = snapshot(table, cursor, cursor);
        let result = execute_command(&doc, "sum row", CommandMode::Editor);
        assert!(result.operations.is_empty());
        assert!(
            result.message.contains("incompatible units"),
            "got: {}",
            result.message
        );
    }

    // --- sum column / avg column ---
    //
    // Cursor is inside a specific cell; only cells ABOVE the cursor row (in the
    // same column) are summed and the result is inserted at the cursor position.

    #[test]
    fn sum_column_sums_cells_above_cursor() {
        // Table:
        //   | 2m  | 3m  |
        //   | --- | --- |
        //   | 4m  | 5m  |  ← cursor in col 1 (second numeric column)
        let table = "| 2m  | 3m  |\n| --- | --- |\n| 4m  | 5m  |";
        // Place cursor inside "5m" cell (col 1, row 3) — col 1 means after two '|' on that line.
        // Row 3 starts at offset: len("| 2m  | 3m  |\n| --- | --- |\n") = 14 + 14 = 28
        let row3_start = table.rfind("| 4m").unwrap();
        let cursor = row3_start + table[row3_start..].find("5m").unwrap();
        let doc = snapshot(table, cursor, cursor);
        let result = execute_command(&doc, "sum_column", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        assert!(result.message.contains("sum(column)"));
        // Only "3m" from row 1 is above the cursor in col 1 (row 2 is delimiter)
        assert_eq!(
            result.operations[0].changes[0].insert.replace(' ', ""),
            "3.00m"
        );
    }

    #[test]
    fn avg_column_ignores_non_numeric_cells() {
        // | header | number |
        // | ------ | ------ |
        // | a      | 3      |
        // | b      | 4      |   ← cursor in col 1
        let table =
            "| header | number |\n| ------ | ------ |\n| a      | 3      |\n| b      | 4      |";
        // Place cursor in last row, col 1 (the "4" cell)
        let cursor = table.rfind("4 ").unwrap() + 1; // inside "4" cell
        let doc = snapshot(table, cursor, cursor);
        let result = execute_command(&doc, "avg column", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        assert!(result.message.contains("avg(column)"));
        // col 1 above cursor: "number"(non-numeric, skipped), "3" → avg = 3.00
        assert_eq!(result.operations[0].changes[0].insert.trim(), "3.00");
    }

    #[test]
    fn clist_converts_selected_lines() {
        let text = "alpha\n- beta\n1. gamma\ntail";
        let tail_start = text.find("\ntail").expect("tail marker");
        let doc = snapshot(text, tail_start, 0);
        let result = execute_command(&doc, "clist", CommandMode::Editor);
        assert_eq!(result.message, "converted 3 lines to checklist");
        assert_eq!(result.operations.len(), 1);
        let change = &result.operations[0].changes[0];
        assert_eq!(change.from, 0);
        assert_eq!(change.to, tail_start);
        assert_eq!(change.insert, "- [ ] alpha\n- [ ] beta\n1. [ ] gamma");
    }

    #[test]
    fn ulist_converts_only_current_line_without_selection() {
        let text = "alpha\n1. beta\ngamma";
        let cursor = text.find("beta").expect("cursor");
        let doc = snapshot(text, cursor, cursor);
        let result = execute_command(&doc, "ulist", CommandMode::Editor);
        assert_eq!(result.message, "converted 1 line to unordered list");
        assert_eq!(result.operations.len(), 1);
        let change = &result.operations[0].changes[0];
        let beta_line_from = text.find("\n1. beta").expect("line start") + 1;
        let beta_line_to = beta_line_from + "1. beta".len();
        assert_eq!(change.from, beta_line_from);
        assert_eq!(change.to, beta_line_to);
        assert_eq!(change.insert, "- beta");
    }

    #[test]
    fn olist_converts_selection_to_numbered_items() {
        let text = "alpha\n- [x] beta\n- gamma\ntail";
        let tail_start = text.find("\ntail").expect("tail marker");
        let doc = snapshot(text, tail_start, 0);
        let result = execute_command(&doc, "olist", CommandMode::Editor);
        assert_eq!(result.message, "converted 3 lines to ordered list");
        assert_eq!(result.operations.len(), 1);
        let change = &result.operations[0].changes[0];
        assert_eq!(change.from, 0);
        assert_eq!(change.to, tail_start);
        assert_eq!(change.insert, "1. alpha\n2. beta\n3. gamma");
    }

    #[test]
    fn clist_in_vim_mode_treats_endpoint_lines_as_selected() {
        let text = "alpha\nbeta\ngamma";
        let beta_start = text.find("beta").expect("beta");
        let doc = snapshot(text, beta_start, 0);
        let result = execute_command(&doc, "clist", CommandMode::Vim);
        assert_eq!(result.message, "converted 2 lines to checklist");
        assert_eq!(result.operations.len(), 1);
        let change = &result.operations[0].changes[0];
        assert_eq!(change.from, 0);
        assert_eq!(change.to, beta_start + "beta".len());
        assert_eq!(change.insert, "- [ ] alpha\n- [ ] beta");
    }
}
