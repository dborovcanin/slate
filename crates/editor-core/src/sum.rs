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

/// Running count / sum / min / max over the numbers in some text, used for
/// live selection statistics. Feed it selected text piece by piece.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NumberStats {
    pub count: usize,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
}

impl NumberStats {
    /// Adds the numbers of one line of text. A leading list marker (`1.`,
    /// `2)`, `-`) is skipped so ordered-list numbering does not count.
    pub fn add_text(&mut self, line: &str) {
        let body = match crate::markdown_tokens::list_marker_end(line) {
            Some(end) => line.get(end..).unwrap_or(""),
            None => line,
        };
        for value in parse_numbers(body) {
            if self.count == 0 {
                self.min = value;
                self.max = value;
            } else {
                self.min = self.min.min(value);
                self.max = self.max.max(value);
            }
            self.count += 1;
            self.sum += value;
        }
    }

    pub fn average(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
    }
}

/// Compact number for status display: at most two decimals, trailing zeros
/// dropped (`12`, `12.5`, `0.33`).
pub fn format_stat_value(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_string()
    } else {
        text.to_string()
    }
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
    fn number_stats_accumulate_across_lines_and_skip_list_markers() {
        let mut stats = NumberStats::default();
        stats.add_text("1. rent 1,200");
        stats.add_text("2. food 350.5");
        stats.add_text("- misc -50");
        stats.add_text("no numbers here");
        assert_eq!(stats.count, 3);
        assert_eq!(stats.sum, 1500.5);
        assert_eq!((stats.min, stats.max), (-50.0, 1200.0));
        assert_eq!(stats.average(), Some(1500.5 / 3.0));
        assert_eq!(NumberStats::default().average(), None);
    }

    #[test]
    fn format_stat_value_trims_trailing_zeros() {
        assert_eq!(format_stat_value(12.0), "12");
        assert_eq!(format_stat_value(12.5), "12.5");
        assert_eq!(format_stat_value(1.0 / 3.0), "0.33");
        assert_eq!(format_stat_value(-0.001), "0");
        assert_eq!(format_stat_value(f64::NAN), "0");
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
