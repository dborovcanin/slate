use super::command_catalog::{self, CommandId};
use super::context::ResolvedContext;
use super::format::format_markdown;
use super::operations::replace_range;
use super::sum::{
    format_sum_result as format_numeric_result, parse_numbers as parse_numeric_values,
    resolve_scope_range, SumScope,
};
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

fn table_data_rows(ctx: &ResolvedContext, range: BlockLineRange) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for line_no in range.start_line..=range.end_line {
        let cells = split_table_cells(ctx.line_text(line_no));
        if is_table_delimiter_row(&cells) {
            continue;
        }
        rows.push(cells);
    }
    rows
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

fn sum_term_values(terms: &[String]) -> Option<String> {
    if terms.is_empty() {
        return None;
    }
    if terms.len() == 1 {
        return Some(terms[0].clone());
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

fn sum_row(ctx: &ResolvedContext, range: BlockLineRange) -> Vec<String> {
    let rows = table_data_rows(ctx, range);
    let mut out = Vec::new();

    for row in rows {
        let terms = row
            .iter()
            .filter_map(|cell| evaluate_cell_term(cell))
            .collect::<Vec<_>>();
        if let Some(total) = sum_term_values(&terms) {
            out.push(total);
        }
    }

    out
}

fn sum_column(ctx: &ResolvedContext, range: BlockLineRange) -> Vec<String> {
    let rows = table_data_rows(ctx, range);
    let column_count = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    let mut out = Vec::new();

    for col in 0..column_count {
        let terms = rows
            .iter()
            .filter_map(|row| row.get(col))
            .filter_map(|cell| evaluate_cell_term(cell))
            .collect::<Vec<_>>();
        if let Some(total) = sum_term_values(&terms) {
            out.push(total);
        }
    }

    out
}

fn avg_row(ctx: &ResolvedContext, range: BlockLineRange) -> Vec<String> {
    let rows = table_data_rows(ctx, range);
    let mut out = Vec::new();

    for row in rows {
        let terms = row
            .iter()
            .filter_map(|cell| evaluate_cell_term(cell))
            .collect::<Vec<_>>();
        if let Some(avg) = average_term_values(&terms) {
            out.push(avg);
        }
    }

    out
}

fn avg_column(ctx: &ResolvedContext, range: BlockLineRange) -> Vec<String> {
    let rows = table_data_rows(ctx, range);
    let column_count = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    let mut out = Vec::new();

    for col in 0..column_count {
        let terms = rows
            .iter()
            .filter_map(|row| row.get(col))
            .filter_map(|cell| evaluate_cell_term(cell))
            .collect::<Vec<_>>();
        if let Some(avg) = average_term_values(&terms) {
            out.push(avg);
        }
    }

    out
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

pub fn list_command_suggestions(mode: CommandMode, raw_input: &str) -> Vec<CommandSuggestion> {
    command_catalog::list_command_suggestions(mode, raw_input)
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

            let (formatted, msg) = if scope == SumScope::Row || scope == SumScope::Column {
                let totals = if scope == SumScope::Row {
                    sum_row(&ctx, range)
                } else {
                    sum_column(&ctx, range)
                };
                if totals.is_empty() {
                    return result_with_message(format!("sum({scope_name}): no numbers"));
                }
                let formatted = totals.join("\n");
                let msg = format!(
                    "sum({scope_name}) = [{}] ({} totals, inserted + copied)",
                    totals.join(", "),
                    totals.len()
                );
                (formatted, msg)
            } else {
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
                (formatted, msg)
            };

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

            let (formatted, msg) = if scope == SumScope::Row || scope == SumScope::Column {
                let averages = if scope == SumScope::Row {
                    avg_row(&ctx, range)
                } else {
                    avg_column(&ctx, range)
                };
                if averages.is_empty() {
                    return result_with_message(format!("avg({scope_name}): no numbers"));
                }
                let formatted = averages.join("\n");
                let msg = format!(
                    "avg({scope_name}) = [{}] ({} averages, inserted + copied)",
                    averages.join(", "),
                    averages.len()
                );
                (formatted, msg)
            } else {
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
                (formatted, msg)
            };

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
                "format"
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
    fn sum_row_supports_unit_aware_totals() {
        let doc = snapshot(
            "| item | a  | b   |\n| ---- | -- | --- |\n| x    | 2m | 2km |\n| y    | 3m | 4m  |",
            0,
            0,
        );
        let result = execute_command(&doc, "sum row", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        assert!(result.message.contains("sum(row)"));
        let inserted = result.operations[0].changes[0].insert.replace(' ', "");
        assert_eq!(inserted, "2002.00m\n7.00m");
    }

    #[test]
    fn sum_column_supports_unit_aware_totals_and_alias() {
        let doc = snapshot(
            "| item | a  | b   |\n| ---- | -- | --- |\n| x    | 2m | 2km |\n| y    | 3m | 4m  |",
            0,
            0,
        );
        let result = execute_command(&doc, "sum_column", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        assert!(result.message.contains("sum(column)"));
        let inserted = result.operations[0].changes[0].insert.replace(' ', "");
        assert_eq!(inserted, "5.00m\n2.00km");
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

    #[test]
    fn avg_row_supports_unit_aware_values() {
        let doc = snapshot(
            "| item | a  | b   |\n| ---- | -- | --- |\n| x    | 2m | 2km |\n| y    | 3m | 4m  |",
            0,
            0,
        );
        let result = execute_command(&doc, "avg row", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        assert!(result.message.contains("avg(row)"));
        let inserted = result.operations[0].changes[0].insert.replace(' ', "");
        assert_eq!(inserted, "1001.00m\n3.50m");
    }

    #[test]
    fn avg_column_supports_unit_aware_values_and_alias() {
        let doc = snapshot(
            "| item | a  | b   |\n| ---- | -- | --- |\n| x    | 2m | 2km |\n| y    | 3m | 4m  |",
            0,
            0,
        );
        let result = execute_command(&doc, "avg_column", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        assert!(result.message.contains("avg(column)"));
        let inserted = result.operations[0].changes[0].insert.replace(' ', "");
        assert_eq!(inserted, "2.50m\n1.00km");
    }

    #[test]
    fn avg_column_ignores_non_numeric_header_cells() {
        let doc = snapshot(
            "| header | number |\n| ------ | ------ |\n| a      | 3      |\n| b      | 4      |",
            0,
            0,
        );
        let result = execute_command(&doc, "avg column", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        assert!(result.message.contains("avg(column)"));
        assert_eq!(result.operations[0].changes[0].insert, "3.50");
    }
}
