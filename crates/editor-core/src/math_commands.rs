use crate::command_catalog::CommandId;
use crate::context::ResolvedContext;
use crate::operations::replace_range;
use crate::sum::{
    format_sum_result as format_numeric_result, parse_numbers as parse_numeric_values,
    resolve_scope_range, SumScope,
};
use crate::table;
use crate::types::{
    BlockLineRange, CommandExecutionResult, EditOperation, EditorContextSnapshot,
    OperationSelection,
};
use regex::Regex;
use std::sync::OnceLock;

static NUMERIC_VALUE_RE: OnceLock<Regex> = OnceLock::new();

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

    let matcher = NUMERIC_VALUE_RE.get_or_init(|| {
        Regex::new(r"[-+]?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?|[-+]?\.\d+")
            .expect("numeric value regex is valid")
    });
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

    let nums: Option<Vec<f64>> = terms
        .iter()
        .map(|term| parse_plain_numeric_literal(term))
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

fn cursor_table_column(ctx: &ResolvedContext<'_>) -> Option<usize> {
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
        return None;
    }
    Some(pipes_before - 1)
}

fn collect_table_terms(
    ctx: &ResolvedContext<'_>,
    scope: SumScope,
    range: BlockLineRange,
) -> Vec<String> {
    let Some(cursor_col) = cursor_table_column(ctx) else {
        return Vec::new();
    };

    if scope == SumScope::Row {
        let cells = table::split_table_cells(ctx.current_line().text.as_str());
        cells[..cursor_col.min(cells.len())]
            .iter()
            .filter_map(|cell| evaluate_cell_term(cell))
            .collect()
    } else {
        let cursor_line = ctx.current_line().number;
        let mut terms = Vec::new();
        for line_no in range.start_line..cursor_line {
            let cells = table::split_table_cells(ctx.line_text(line_no));
            if table::is_delimiter_row_at(&cells, crate::text_rules::follows_table_header(ctx, line_no)) {
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

fn insert_value_at_selection(snapshot: &EditorContextSnapshot, value: &str) -> EditOperation {
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
        .map(|idx| cursor_in_line + idx);

    if let (Some(lp), Some(rp)) = (left_pipe, right_pipe) {
        let cell_start = line.from + lp + 1;
        let cell_end = line.from + rp;
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

    insert_value_at_selection(snapshot, value)
}

pub fn execute_math_command(
    snapshot: &EditorContextSnapshot,
    command_id: CommandId,
) -> Option<CommandExecutionResult> {
    let (op_name, use_avg) = match command_id {
        CommandId::Sum
        | CommandId::SumList
        | CommandId::SumRow
        | CommandId::SumColumn
        | CommandId::SumDoc => ("sum", false),
        CommandId::Avg
        | CommandId::AvgList
        | CommandId::AvgRow
        | CommandId::AvgColumn
        | CommandId::AvgDoc => ("avg", true),
        _ => return None,
    };

    let ctx = ResolvedContext::new(snapshot.clone());
    let scope = scope_for_command_id(command_id)?;
    let scope_name = scope_label(scope);
    let Some(range) = resolve_scope_range(&ctx, scope) else {
        return Some(result_with_message(format!(
            "{op_name}({scope_name}): no block at cursor"
        )));
    };

    if scope == SumScope::Row || scope == SumScope::Column {
        let terms = collect_table_terms(&ctx, scope, range);
        return Some(
            match compute_table_terms(terms, op_name, scope_name, use_avg) {
                Ok(value) => {
                    let op = replace_table_cell_at_cursor(snapshot, &value);
                    let mut result =
                        result_with_message(format!("{op_name}({scope_name}) = {value}"));
                    result.operations.push(op);
                    result.clipboard_text = Some(value);
                    result
                }
                Err(message) => result_with_message(message),
            },
        );
    }

    let text = ctx.text_for_line_range(range);
    let numbers = parse_sum_numbers(&text);
    if numbers.is_empty() {
        return Some(result_with_message(format!(
            "{op_name}({scope_name}): no numbers"
        )));
    }
    let value = if use_avg {
        numbers.iter().sum::<f64>() / numbers.len() as f64
    } else {
        numbers.iter().sum::<f64>()
    };
    let formatted = format_sum_result(value);
    let msg = format!(
        "{op_name}({scope_name}) = {formatted} ({} values, inserted + copied)",
        numbers.len()
    );
    let op = insert_value_at_selection(snapshot, &formatted);
    let mut result = result_with_message(msg);
    result.operations.push(op);
    result.clipboard_text = Some(formatted);
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SelectionSnapshot;

    fn snapshot(text: &str, head: usize, anchor: usize) -> EditorContextSnapshot {
        EditorContextSnapshot {
            text: text.to_string(),
            selection: SelectionSnapshot { anchor, head },
            changed_range: None,
        }
    }

    #[test]
    fn sum_row_replaces_active_table_cell() {
        let table = "| item | 2m  | 2km |  |";
        let cursor = table.rfind("|  |").expect("table cell") + 1;
        let doc = snapshot(table, cursor, cursor);
        let result = execute_math_command(&doc, CommandId::SumRow).expect("math command");
        assert_eq!(result.operations.len(), 1);
        assert!(result.message.contains("sum(row)"));
        assert_eq!(
            result.operations[0].changes[0].insert.replace(' ', ""),
            "2002.00m"
        );
    }

    // Behavioral contract for `:sum` / `:avg`.
    #[test]
    fn sum_paragraph_inserts_total_at_the_cursor() {
        let doc = snapshot("10\n20", 0, 0);
        let result = execute_math_command(&doc, CommandId::Sum).expect("math command");
        assert!(
            result.message.contains("sum(paragraph) = 30.00"),
            "got: {}",
            result.message
        );
        assert_eq!(result.operations.len(), 1);
        let change = &result.operations[0].changes[0];
        assert_eq!((change.from, change.to, change.insert.as_str()), (0, 0, "30.00"));
        assert_eq!(
            result.operations[0].selection.map(|s| s.anchor),
            Some("30.00".len())
        );
    }

    #[test]
    fn avg_paragraph_inserts_mean_at_the_cursor() {
        let doc = snapshot("10\n20\n30", 0, 0);
        let result = execute_math_command(&doc, CommandId::Avg).expect("math command");
        assert!(
            result.message.contains("avg(paragraph) = 20"),
            "got: {}",
            result.message
        );
        assert_eq!(result.operations.len(), 1);
        assert_eq!(result.operations[0].changes[0].insert, "20.00");
    }

    #[test]
    fn avg_row_averages_unit_bearing_cells() {
        let table = "| x | 3m | 4m |  |";
        let cursor = table.rfind("|  |").expect("table cell") + 1;
        let doc = snapshot(table, cursor, cursor);
        let result = execute_math_command(&doc, CommandId::AvgRow).expect("math command");
        assert_eq!(result.message, "avg(row) = 3.50 m");
        assert_eq!(
            result.operations[0].changes[0].insert.replace(' ', ""),
            "3.50m"
        );
    }

    #[test]
    fn sum_column_skips_non_numeric_header_cells() {
        let table =
            "| header | number |\n| ------ | ------ |\n| a      | 3      |\n| b      | 4      |";
        let cursor = table.rfind("4 ").expect("last value") + 1;
        let doc = snapshot(table, cursor, cursor);
        let result = execute_math_command(&doc, CommandId::SumColumn).expect("math command");
        assert_eq!(result.message, "sum(column) = 3.00");
        assert_eq!(result.operations[0].changes[0].insert.trim(), "3.00");
    }

    #[test]
    fn normalize_evaluated_value_strips_approximation_wording_and_rounds() {
        assert_eq!(
            normalize_evaluated_value("approximately 2.004 km").as_deref(),
            Some("2.00 km")
        );
        assert_eq!(
            normalize_evaluated_value("approx. 1.239").as_deref(),
            Some("1.24")
        );
        assert_eq!(normalize_evaluated_value("≈ 5 m").as_deref(), Some("5.00 m"));
        assert_eq!(normalize_evaluated_value("not a number"), None);
    }

    #[test]
    fn avg_column_skips_non_numeric_header_cells() {
        let table =
            "| header | number |\n| ------ | ------ |\n| a      | 3      |\n| b      | 4      |";
        let cursor = table.rfind("4 ").expect("last value") + 1;
        let doc = snapshot(table, cursor, cursor);
        let result = execute_math_command(&doc, CommandId::AvgColumn).expect("math command");
        assert_eq!(result.operations.len(), 1);
        assert_eq!(result.operations[0].changes[0].insert.trim(), "3.00");
    }
}
