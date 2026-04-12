use super::context::ResolvedContext;
use super::operations::replace_range;
use super::types::{
    CommandExecutionResult, CommandMode, CommandSuggestion, EditOperation, EditorContextSnapshot,
    OperationSelection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandBehavior {
    Sum,
    Date,
    Format,
    Quit,
}

#[derive(Debug, Clone, Copy)]
struct CommandDefinition {
    value: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    modes: &'static [CommandMode],
    behavior: CommandBehavior,
}

const MODES_BOTH: [CommandMode; 2] = [CommandMode::Vim, CommandMode::Editor];
const MODES_VIM: [CommandMode; 1] = [CommandMode::Vim];

const COMMAND_DEFINITIONS: [CommandDefinition; 7] = [
    CommandDefinition {
        value: "sum",
        aliases: &[],
        description: "sum paragraph (placeholder)",
        modes: &MODES_BOTH,
        behavior: CommandBehavior::Sum,
    },
    CommandDefinition {
        value: "sum list",
        aliases: &[],
        description: "sum list at cursor (placeholder)",
        modes: &MODES_BOTH,
        behavior: CommandBehavior::Sum,
    },
    CommandDefinition {
        value: "sum table",
        aliases: &[],
        description: "sum table at cursor (placeholder)",
        modes: &MODES_BOTH,
        behavior: CommandBehavior::Sum,
    },
    CommandDefinition {
        value: "sum doc",
        aliases: &["sum_all", "sum all"],
        description: "sum whole document (placeholder)",
        modes: &MODES_BOTH,
        behavior: CommandBehavior::Sum,
    },
    CommandDefinition {
        value: "date",
        aliases: &[],
        description: "insert picked date (placeholder)",
        modes: &MODES_BOTH,
        behavior: CommandBehavior::Date,
    },
    CommandDefinition {
        value: "format",
        aliases: &["fmt"],
        description: "format markdown document (placeholder)",
        modes: &MODES_BOTH,
        behavior: CommandBehavior::Format,
    },
    CommandDefinition {
        value: "q",
        aliases: &["q!"],
        description: "quit",
        modes: &MODES_VIM,
        behavior: CommandBehavior::Quit,
    },
];

fn normalize_command(input: &str) -> String {
    let trimmed = input.trim();
    let without_colon = trimmed.strip_prefix(':').unwrap_or(trimmed);
    without_colon.to_lowercase()
}

fn command_matches(def: &CommandDefinition, normalized_input: &str) -> bool {
    def.value == normalized_input || def.aliases.iter().any(|alias| *alias == normalized_input)
}

fn available_commands(mode: CommandMode) -> Vec<&'static CommandDefinition> {
    COMMAND_DEFINITIONS
        .iter()
        .filter(|def| def.modes.contains(&mode))
        .collect()
}

fn parse_sum_numbers(text: &str) -> Vec<f64> {
    let cleaned = text.replace(',', "");
    cleaned
        .split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-' && c != '+')
        .filter_map(|w| {
            let w = w.trim();
            if w.is_empty() || matches!(w, "-" | "+" | ".") {
                return None;
            }
            w.parse::<f64>().ok().filter(|v| v.is_finite())
        })
        .collect()
}

fn format_sum_result(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    if (value - value.round()).abs() < 1e-9 {
        return format!("{}", value.round() as i64);
    }
    format!("{:.10}", value)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn sum_scope_label(command_value: &str) -> &str {
    let rest = command_value.strip_prefix("sum").unwrap_or("").trim();
    if rest.is_empty() {
        "paragraph"
    } else {
        rest
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
    let query = normalize_command(raw_input);
    let available = available_commands(mode);

    if query.is_empty() {
        return available
            .into_iter()
            .map(|command| CommandSuggestion {
                value: command.value.to_string(),
                description: command.description.to_string(),
            })
            .collect();
    }

    let mut matches = available
        .into_iter()
        .filter_map(|command| {
            let lower_value = command.value.to_lowercase();
            let score = if lower_value.starts_with(&query) {
                0
            } else if lower_value.contains(&query) {
                1
            } else {
                2
            };

            if score < 2 {
                Some((score, command))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    matches.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.value.cmp(right.1.value))
    });

    matches
        .into_iter()
        .map(|(_, command)| CommandSuggestion {
            value: command.value.to_string(),
            description: command.description.to_string(),
        })
        .collect()
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

fn format_markdown(text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(|s| s.trim_end().to_string()).collect();

    // Auto-format tables
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim().starts_with('|') && lines[i].trim().ends_with('|') {
            let start = i;
            while i < lines.len()
                && lines[i].trim().starts_with('|')
                && lines[i].trim().ends_with('|')
            {
                i += 1;
            }
            let end = i;

            let mut rows: Vec<Vec<String>> = Vec::new();
            for r in start..end {
                let row = lines[r].trim();
                let cols: Vec<String> = row
                    .trim_start_matches('|')
                    .trim_end_matches('|')
                    .split('|')
                    .map(|s| s.trim().to_string())
                    .collect();
                rows.push(cols);
            }

            if !rows.is_empty() {
                let max_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                let mut widths = vec![0; max_cols];
                for r in &rows {
                    for (c, col) in r.iter().enumerate() {
                        if c < max_cols && col.len() > widths[c] {
                            widths[c] = col.len();
                        }
                    }
                }

                for r in start..end {
                    let mut new_row = String::from("|");
                    for c in 0..max_cols {
                        let col = rows[r - start].get(c).unwrap_or(&String::new()).clone();
                        let is_separator = rows[r - start].iter().all(|x| {
                            x.chars()
                                .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
                        });

                        let width = widths[c].max(3);
                        if is_separator {
                            new_row.push_str(&format!(" {} |", "-".repeat(width)));
                        } else {
                            if col.is_empty() {
                                new_row.push_str(&format!(" {:width$} |", "", width = width));
                            } else {
                                new_row.push_str(&format!(" {:width$} |", col, width = width));
                            }
                        }
                    }
                    lines[r] = new_row;
                }
            }
        } else {
            i += 1;
        }
    }

    // Formatting other elements
    for line in &mut lines {
        // Headings
        if line.starts_with('#') {
            let hashes = line.chars().take_while(|&c| c == '#').count();
            if hashes > 0 && hashes <= 6 {
                let rest = line[hashes..].trim_start();
                if !rest.is_empty() {
                    *line = format!("{} {}", "#".repeat(hashes), rest);
                }
            }
        }

        // Block quotes
        if line.starts_with('>') {
            let rest = line[1..].trim_start();
            *line = format!("> {}", rest);
        }

        // Lists
        if let Some(ch) = line.chars().next() {
            if ch == '-' || ch == '*' || ch == '+' {
                if line.len() > 1 && !line[1..].starts_with(' ') && !line[1..].starts_with(ch) {
                    let rest = line[1..].trim_start();
                    *line = format!("{} {}", ch, rest);
                }
            }
        }
    }

    lines.join("\n")
}

pub fn execute_command(
    snapshot: &EditorContextSnapshot,
    raw_input: &str,
    mode: CommandMode,
) -> CommandExecutionResult {
    let normalized = normalize_command(raw_input);
    if normalized.is_empty() {
        return result_with_message("");
    }

    let Some(command) = available_commands(mode)
        .into_iter()
        .find(|def| command_matches(def, &normalized))
    else {
        return result_with_message(format!("unknown command: {normalized}"));
    };

    match command.behavior {
        CommandBehavior::Quit => CommandExecutionResult {
            message: "quit".to_string(),
            operations: Vec::new(),
            clipboard_text: None,
            quit_requested: true,
        },
        CommandBehavior::Sum => {
            let ctx = ResolvedContext::new(snapshot.clone());
            let current_line = ctx.current_line().number;

            let range = match command.value {
                "sum doc" => Some(crate::editor_core::types::BlockLineRange {
                    start_line: 1,
                    end_line: ctx.line_count(),
                }),
                "sum" => Some(ctx.paragraph_range_at_line(current_line)),
                "sum list" => ctx.list_range_at_line(current_line),
                "sum table" => ctx.table_range_at_line(current_line, 1),
                _ => None,
            };

            let scope = sum_scope_label(command.value);

            if let Some(r) = range {
                let text = ctx.text_for_line_range(r);
                let numbers = parse_sum_numbers(&text);
                if numbers.is_empty() {
                    return result_with_message(format!("sum({scope}): no numbers"));
                }
                let sum: f64 = numbers.iter().sum();
                let formatted = format_sum_result(sum);
                let msg = format!(
                    "sum({scope}) = {formatted} ({} values, inserted + copied)",
                    numbers.len()
                );

                let end_pos = ctx.line(r.end_line).to;
                let insert = format!("\n{formatted}");
                let new_anchor = end_pos + insert.len();
                let op = replace_range(
                    end_pos,
                    end_pos,
                    insert,
                    Some(OperationSelection {
                        anchor: new_anchor,
                        head: None,
                    }),
                );

                let mut result = result_with_message(msg);
                result.operations.push(op);
                result.clipboard_text = Some(formatted);
                result
            } else {
                result_with_message(format!("sum({scope}): no block at cursor"))
            }
        }
        CommandBehavior::Date => {
            let date_str = time::OffsetDateTime::now_utc().date().to_string();
            let mut result = result_with_message("Date inserted");
            result
                .operations
                .push(insert_value_at_selection(snapshot, &date_str));
            result
        }
        CommandBehavior::Format => {
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
            vec!["sum", "sum list", "sum table", "sum doc", "date", "format"]
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
        assert_eq!(format_sum_result(4.0), "4");
        assert_eq!(format_sum_result(3.5), "3.5");
        assert_eq!(format_sum_result(0.0), "0");
        assert_eq!(format_sum_result(1234.0), "1234");
    }

    #[test]
    fn sum_command_inserts_after_range() {
        let doc = snapshot("item 10\nitem 20\nitem 30", 0, 0);
        let result = execute_command(&doc, "sum", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        let op = &result.operations[0];
        assert_eq!(op.changes[0].insert, "\n60");
        assert!(result.message.contains("60"));
        assert!(result.message.contains("3 values"));
        assert_eq!(result.clipboard_text, Some("60".to_string()));
    }

    #[test]
    fn sum_doc_sums_entire_document() {
        let doc = snapshot("1\n2\n3", 0, 0);
        let result = execute_command(&doc, "sum doc", CommandMode::Editor);
        assert_eq!(result.operations.len(), 1);
        assert!(result.message.contains("6"));
    }
}
