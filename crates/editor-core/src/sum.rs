use crate::context::ResolvedContext;
use crate::types::BlockLineRange;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SumScope {
    Paragraph,
    List,
    Row,
    Column,
    Doc,
}

pub fn parse_numbers(text: &str) -> Vec<f64> {
    let cleaned = text.replace(',', "");
    cleaned
        .split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-' && c != '+')
        .filter_map(|word| {
            let word = word.trim();
            if word.is_empty() || matches!(word, "-" | "+" | ".") {
                return None;
            }
            word.parse::<f64>().ok().filter(|value| value.is_finite())
        })
        .collect()
}

pub fn format_sum_result(value: f64) -> String {
    if !value.is_finite() {
        return "0.00".to_string();
    }
    format!("{value:.2}")
}

pub fn resolve_scope_range(ctx: &ResolvedContext<'_>, scope: SumScope) -> Option<BlockLineRange> {
    let current_line = ctx.current_line().number;
    match scope {
        SumScope::Paragraph => Some(ctx.paragraph_range_at_line(current_line)),
        SumScope::List => ctx.list_range_at_line(current_line),
        SumScope::Row | SumScope::Column => ctx.table_range_at_line(current_line, 1),
        SumScope::Doc => Some(BlockLineRange {
            start_line: 1,
            end_line: ctx.line_count(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EditorContextSnapshot, SelectionSnapshot};

    fn ctx(text: &str, cursor: usize) -> ResolvedContext<'static> {
        ResolvedContext::new(EditorContextSnapshot {
            text: text.to_string(),
            selection: SelectionSnapshot {
                anchor: cursor,
                head: cursor,
            },
            changed_range: None,
        })
    }

    #[test]
    fn parse_numbers_extracts_finite_values() {
        assert_eq!(
            parse_numbers("a 10 b -2.5 c 1,200 d +0.75"),
            vec![10.0, -2.5, 1200.0, 0.75]
        );
        assert_eq!(parse_numbers("no values"), Vec::<f64>::new());
    }

    #[test]
    fn resolve_scope_range_handles_paragraph_list_table_and_doc() {
        let text = "- one\n- two\n\n| a | 1 |\n| b | 2 |\n";
        let list_ctx = ctx(text, 0);
        assert_eq!(
            resolve_scope_range(&list_ctx, SumScope::List),
            Some(BlockLineRange {
                start_line: 1,
                end_line: 2,
            })
        );

        let table_cursor = text.find("| a | 1 |").unwrap_or(0);
        let table_ctx = ctx(text, table_cursor);
        assert_eq!(
            resolve_scope_range(&table_ctx, SumScope::Row),
            Some(BlockLineRange {
                start_line: 4,
                end_line: 5,
            })
        );

        assert_eq!(
            resolve_scope_range(&table_ctx, SumScope::Doc),
            Some(BlockLineRange {
                start_line: 1,
                end_line: 6,
            })
        );
    }
}
