use crate::command_catalog::CommandId;
use crate::context::ResolvedContext;
use crate::engine::EditorEngine;
use crate::format::format_markdown;
use crate::operations::replace_range;
use crate::text_rules::{convert_line_to_list, convert_line_to_title, ListKind};
use crate::types::{
    CommandExecutionResult, CommandMode, CommandSuggestion, EditOperation, EditorContextSnapshot,
    OperationSelection, TextChange,
};

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
    Title,
    Checklist,
    Unordered,
    Ordered,
}

fn list_conversion_label(kind: ListConversionKind) -> &'static str {
    match kind {
        ListConversionKind::Title => "title",
        ListConversionKind::Checklist => "checklist",
        ListConversionKind::Unordered => "unordered list",
        ListConversionKind::Ordered => "ordered list",
    }
}

fn convert_line(line: &str, kind: ListConversionKind, ordered_index: usize) -> (String, bool) {
    if kind == ListConversionKind::Title {
        return convert_line_to_title(line);
    }
    let list_kind = match kind {
        ListConversionKind::Title => {
            unreachable!("title conversion is handled before list mapping")
        }
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
        let (next, _changed_line) = convert_line(source, kind, ordered_index);
        if kind == ListConversionKind::Ordered && !source.trim().is_empty() {
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

fn toggle_inline_wrap(
    snapshot: &EditorContextSnapshot,
    left: &str,
    right: &str,
    label: &str,
) -> CommandExecutionResult {
    let text = &snapshot.text;
    let max = text.len();
    let anchor = snapshot.selection.anchor.min(max);
    let head = snapshot.selection.head.min(max);
    let from = anchor.min(head);
    let to = anchor.max(head);

    if from == to {
        let insert = format!("{}{}", left, right);
        let new_pos = from + left.len();
        let op = EditOperation {
            changes: vec![TextChange { from, to, insert }],
            selection: Some(OperationSelection {
                anchor: new_pos,
                head: None,
            }),
        };
        let mut result = result_with_message(format!("{} markers inserted", label));
        result.operations.push(op);
        return result;
    }

    let is_wrapped = from >= left.len()
        && text.get(from - left.len()..from) == Some(left)
        && to + right.len() <= max
        && text.get(to..to + right.len()) == Some(right);

    if is_wrapped {
        let op = EditOperation {
            changes: vec![
                TextChange {
                    from: from - left.len(),
                    to: from,
                    insert: String::new(),
                },
                TextChange {
                    from: to,
                    to: to + right.len(),
                    insert: String::new(),
                },
            ],
            selection: Some(OperationSelection {
                anchor: from - left.len(),
                head: Some(to - left.len()),
            }),
        };
        let mut result = result_with_message(format!("{} removed", label));
        result.operations.push(op);
        return result;
    }

    let op = EditOperation {
        changes: vec![
            TextChange {
                from,
                to: from,
                insert: left.to_string(),
            },
            TextChange {
                from: to,
                to,
                insert: right.to_string(),
            },
        ],
        selection: Some(OperationSelection {
            anchor: from + left.len(),
            head: Some(to + left.len()),
        }),
    };
    let mut result = result_with_message(format!("{} applied", label));
    result.operations.push(op);
    result
}

fn strip_inline_formatting(text: &str) -> String {
    // Strip markers in order: longest first to avoid partial matches (** before *)
    let markers = ["**", "~~", "*", "`"];
    let mut result = text.to_string();
    for marker in markers {
        result = result.replace(marker, "");
    }
    result
}

fn run_format_clear(snapshot: &EditorContextSnapshot) -> CommandExecutionResult {
    let text = &snapshot.text;
    let max = text.len();
    let anchor = snapshot.selection.anchor.min(max);
    let head = snapshot.selection.head.min(max);
    let from = anchor.min(head);
    let to = anchor.max(head);

    if from == to {
        return result_with_message("no selection");
    }

    let selected = &text[from..to];
    let stripped = strip_inline_formatting(selected);
    if stripped == selected {
        return result_with_message("no inline formatting found");
    }

    let new_len = from + stripped.len();
    let op = EditOperation {
        changes: vec![TextChange {
            from,
            to,
            insert: stripped,
        }],
        selection: Some(OperationSelection {
            anchor: from,
            head: Some(new_len),
        }),
    };
    let mut result = result_with_message("inline formatting cleared");
    result.operations.push(op);
    result
}

pub fn list_command_suggestions(mode: CommandMode, raw_input: &str) -> Vec<CommandSuggestion> {
    EditorEngine::list_command_suggestions(mode, raw_input)
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
    if let Some(result) = crate::substitute::try_execute_vim_substitute(snapshot, raw_input, mode) {
        return result;
    }

    let normalized = EditorEngine::normalize_command(raw_input);
    if normalized.is_empty() {
        return result_with_message("");
    }

    let Some(command) = EditorEngine::resolve_command(mode, raw_input) else {
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
        | CommandId::SumDoc => crate::math_commands::execute_math_command(snapshot, command.id)
            .unwrap_or_else(|| result_with_message(format!("unknown command: {normalized}"))),
        CommandId::Avg
        | CommandId::AvgList
        | CommandId::AvgRow
        | CommandId::AvgColumn
        | CommandId::AvgDoc => crate::math_commands::execute_math_command(snapshot, command.id)
            .unwrap_or_else(|| result_with_message(format!("unknown command: {normalized}"))),
        CommandId::Date => result_with_message("date handled by host"),
        CommandId::Today => result_with_message("today handled by host"),
        CommandId::Browse => result_with_message("browse handled by host"),
        CommandId::History => result_with_message("history handled by host"),
        CommandId::Remind => result_with_message("remind handled by host"),
        CommandId::RemindToggle => result_with_message("remind toggle handled by host"),
        CommandId::ModuleStatus
        | CommandId::ModuleOnMath
        | CommandId::ModuleOffMath
        | CommandId::ModuleToggleMath
        | CommandId::ModuleOnTable
        | CommandId::ModuleOffTable
        | CommandId::ModuleToggleTable
        | CommandId::ModuleOnVariables
        | CommandId::ModuleOffVariables
        | CommandId::ModuleToggleVariables
        | CommandId::ModuleOnStyle
        | CommandId::ModuleOffStyle
        | CommandId::ModuleToggleStyle
        | CommandId::ModuleOnCrossNote
        | CommandId::ModuleOffCrossNote
        | CommandId::ModuleToggleCrossNote => result_with_message("module command handled by host"),
        CommandId::ClipWatch => result_with_message("clip-watch on handled by host"),
        CommandId::ClipWatchStop => result_with_message("clip-watch off handled by host"),
        CommandId::Fold => result_with_message("fold handled by host"),
        CommandId::Unfold => result_with_message("unfold handled by host"),
        CommandId::FoldToggle => result_with_message("fold-toggle handled by host"),
        CommandId::NoteEncrypt | CommandId::NoteDecrypt => {
            result_with_message("note security command handled by host")
        }
        CommandId::ExportPdf | CommandId::ExportMd | CommandId::ExportTxt => {
            result_with_message("export command handled by host")
        }
        CommandId::BackupExport | CommandId::BackupLoad => {
            result_with_message("backup command handled by host")
        }
        CommandId::WebSearch => result_with_message("web search command handled by host"),
        CommandId::ChooseCollection
        | CommandId::ClearCollection
        | CommandId::CreateCollection
        | CommandId::DeleteCollection
        | CommandId::UpdateCollection
        | CommandId::PurgeCollection
        | CommandId::AddToCollection
        | CommandId::RemoveFromCollection => {
            result_with_message("collection command handled by host")
        }
        CommandId::Write | CommandId::WriteQuit => {
            result_with_message("write command handled by host")
        }
        CommandId::Reload => result_with_message("reload command handled by host"),
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
        CommandId::ParagraphTitle
        | CommandId::Checklist
        | CommandId::UnorderedList
        | CommandId::OrderedList => {
            let kind = match command.id {
                CommandId::ParagraphTitle => ListConversionKind::Title,
                CommandId::Checklist => ListConversionKind::Checklist,
                CommandId::UnorderedList => ListConversionKind::Unordered,
                CommandId::OrderedList => ListConversionKind::Ordered,
                _ => unreachable!(),
            };
            run_list_convert_command(snapshot, kind, mode)
        }
        CommandId::FormatClear => run_format_clear(snapshot),
        CommandId::FormatBold => toggle_inline_wrap(snapshot, "**", "**", "bold"),
        CommandId::FormatItalic => toggle_inline_wrap(snapshot, "*", "*", "italic"),
        CommandId::FormatStrike => toggle_inline_wrap(snapshot, "~~", "~~", "strikethrough"),
        CommandId::FormatCode => toggle_inline_wrap(snapshot, "`", "`", "inline code"),
    }
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
                "today",
                "browse",
                "history",
                "date",
                "remind",
                "remind toggle",
                "module status",
                "module math on",
                "module math off",
                "module math toggle",
                "module table on",
                "module table off",
                "module table toggle",
                "module variables on",
                "module variables off",
                "module variables toggle",
                "module style on",
                "module style off",
                "module style toggle",
                "module cross_note on",
                "module cross_note off",
                "module cross_note toggle",
                "collection choose",
                "collection clear",
                "collection create",
                "collection delete",
                "collection update",
                "collection purge",
                "collection join",
                "collection leave",
                "format",
                "clip-watch on",
                "clip-watch off",
                "fold",
                "unfold",
                "fold-toggle",
                "format clear",
                "paragraph title",
                "paragraph clist",
                "paragraph olist",
                "paragraph ulist",
                "format bold",
                "format code",
                "format italic",
                "format strike",
                "note encrypt",
                "note decrypt",
                "export pdf",
                "export md",
                "export txt",
                "backup export",
                "backup load",
                "web",
                "e",
            ]
        );

        let vim_values = list_command_suggestions(CommandMode::Vim, "")
            .into_iter()
            .map(|entry| entry.value)
            .collect::<Vec<_>>();
        assert!(vim_values.contains(&"q".to_string()));
        assert!(vim_values.contains(&"w".to_string()));
        assert!(vim_values.contains(&"wq".to_string()));
        assert!(vim_values.contains(&"e".to_string()));
    }

    #[test]
    fn date_is_left_to_the_host() {
        // The host opens a date picker; the core never reads the clock.
        let result = execute_command(&snapshot("abc", 1, 1), "date", CommandMode::Editor);
        assert!(result.operations.is_empty());
        assert_eq!(result.message, "date handled by host");
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
    fn vim_substitute_replaces_first_match_on_current_line() {
        let doc = snapshot("alpha beta alpha\nalpha beta", 0, 0);
        let result = execute_command(&doc, "s/alpha/omega/", CommandMode::Vim);
        assert_eq!(result.operations.len(), 1);
        assert_eq!(result.operations[0].changes[0].from, 0);
        assert_eq!(result.operations[0].changes[0].to, "alpha beta alpha".len());
        assert_eq!(result.operations[0].changes[0].insert, "omega beta alpha");
        assert_eq!(result.message, "1 substitution on 1 line");
    }

    #[test]
    fn vim_substitute_percent_scope_replaces_first_match_per_line() {
        let doc = snapshot("alpha alpha\nalpha alpha", 0, 0);
        let result = execute_command(&doc, "%s/alpha/omega/", CommandMode::Vim);
        assert_eq!(result.operations.len(), 1);
        assert_eq!(result.operations[0].changes[0].from, 0);
        assert_eq!(result.operations[0].changes[0].to, doc.text.len());
        assert_eq!(
            result.operations[0].changes[0].insert,
            "omega alpha\nomega alpha"
        );
        assert_eq!(result.message, "2 substitutions on 2 lines");
    }

    #[test]
    fn vim_substitute_global_flag_replaces_all_matches() {
        let doc = snapshot("alpha alpha\nalpha", 0, 0);
        let result = execute_command(&doc, "%s/alpha/omega/g", CommandMode::Vim);
        assert_eq!(result.operations.len(), 1);
        assert_eq!(result.operations[0].changes[0].insert, "omega omega\nomega");
        assert_eq!(result.message, "3 substitutions on 2 lines");
    }

    #[test]
    fn vim_substitute_ignore_case_flag_replaces_case_insensitive_matches() {
        let doc = snapshot("Alpha alpha", 0, 0);
        let result = execute_command(&doc, "s/alpha/omega/gi", CommandMode::Vim);
        assert_eq!(result.operations.len(), 1);
        assert_eq!(result.operations[0].changes[0].insert, "omega omega");
        assert_eq!(result.message, "2 substitutions on 1 line");
    }

    #[test]
    fn vim_substitute_with_visual_selection_targets_selected_lines() {
        let text = "zero\nalpha alpha\nalpha\ntail";
        let start = text.find("alpha alpha").expect("line 2");
        let end = text.find("\ntail").expect("tail marker");
        let doc = snapshot(text, end, start);
        let result = execute_command(&doc, "s/alpha/omega/g", CommandMode::Vim);
        assert_eq!(result.operations.len(), 1);
        let change = &result.operations[0].changes[0];
        assert_eq!(change.from, start);
        assert_eq!(change.to, end);
        assert_eq!(change.insert, "omega omega\nomega");
        assert_eq!(result.message, "3 substitutions on 2 lines");
    }

    #[test]
    fn vim_substitute_reports_unsupported_flags_and_invalid_regex() {
        let doc = snapshot("alpha", 0, 0);
        let unsupported = execute_command(&doc, "s/alpha/beta/c", CommandMode::Vim);
        assert_eq!(
            unsupported.message,
            "substitute: unsupported substitute flag: c"
        );
        assert!(unsupported.operations.is_empty());

        let invalid = execute_command(&doc, "s/[alpha/beta/", CommandMode::Vim);
        assert!(invalid.message.starts_with("substitute: invalid pattern"));
        assert!(invalid.operations.is_empty());
    }

    #[test]
    fn vim_substitute_parser_does_not_intercept_sum_command() {
        let doc = snapshot("1\n2\n3", 0, 0);
        let result = execute_command(&doc, "sum", CommandMode::Vim);
        assert!(result.message.contains("sum(paragraph) = 6.00"));
        assert_eq!(result.operations.len(), 1);
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
        let result = execute_command(&doc, "sum column", CommandMode::Editor);
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

    #[test]
    fn title_converts_selected_lines_to_headings() {
        let text = "- task\n## follow-up\ntail";
        let tail_start = text.find("\ntail").expect("tail marker");
        let doc = snapshot(text, tail_start, 0);
        let result = execute_command(&doc, "title", CommandMode::Editor);
        assert_eq!(result.message, "converted 2 lines to title");
        assert_eq!(result.operations.len(), 1);
        let change = &result.operations[0].changes[0];
        assert_eq!(change.from, 0);
        assert_eq!(change.to, tail_start);
        assert_eq!(change.insert, "# task\n# follow-up");
    }

    #[test]
    fn format_clear_strips_inline_markers_from_selection() {
        let text = "**bold** and ~~strike~~ and `code`";
        let doc = snapshot(text, text.len(), 0);
        let result = execute_command(&doc, "format clear", CommandMode::Editor);
        assert_eq!(result.message, "inline formatting cleared");
        assert_eq!(result.operations.len(), 1);
        assert_eq!(
            result.operations[0].changes[0].insert,
            "bold and strike and code"
        );
    }

    #[test]
    fn format_clear_reports_no_selection_when_cursor_empty() {
        let text = "**bold**";
        let doc = snapshot(text, 0, 0);
        let result = execute_command(&doc, "format clear", CommandMode::Editor);
        assert_eq!(result.message, "no selection");
        assert!(result.operations.is_empty());
    }

    #[test]
    fn format_bold_wraps_selection() {
        let text = "hello world";
        let from = text.find("world").unwrap();
        let to = from + "world".len();
        let doc = snapshot(text, to, from);
        let result = execute_command(&doc, "bold", CommandMode::Editor);
        assert_eq!(result.message, "bold applied");
        assert_eq!(result.operations.len(), 1);
        let op = &result.operations[0];
        assert_eq!(op.changes.len(), 2);
        assert_eq!(
            op.changes[0],
            TextChange {
                from,
                to: from,
                insert: "**".to_string()
            }
        );
        assert_eq!(
            op.changes[1],
            TextChange {
                from: to,
                to,
                insert: "**".to_string()
            }
        );
    }

    #[test]
    fn format_bold_unwraps_already_wrapped_selection() {
        let text = "hello **world**";
        let from = text.find("world").unwrap();
        let to = from + "world".len();
        let doc = snapshot(text, to, from);
        let result = execute_command(&doc, "bold", CommandMode::Editor);
        assert_eq!(result.message, "bold removed");
        assert_eq!(result.operations.len(), 1);
        let op = &result.operations[0];
        assert_eq!(op.changes.len(), 2);
        assert_eq!(
            op.changes[0],
            TextChange {
                from: from - 2,
                to: from,
                insert: String::new()
            }
        );
        assert_eq!(
            op.changes[1],
            TextChange {
                from: to,
                to: to + 2,
                insert: String::new()
            }
        );
    }

    #[test]
    fn format_bold_empty_selection_inserts_markers_and_positions_cursor() {
        let text = "hello ";
        let cursor = text.len();
        let doc = snapshot(text, cursor, cursor);
        let result = execute_command(&doc, "bold", CommandMode::Editor);
        assert_eq!(result.message, "bold markers inserted");
        assert_eq!(result.operations.len(), 1);
        let op = &result.operations[0];
        assert_eq!(op.changes.len(), 1);
        assert_eq!(op.changes[0].insert, "****");
        assert_eq!(op.selection.unwrap().anchor, cursor + 2);
    }

    #[test]
    fn format_italic_wraps_selection() {
        let text = "hello world";
        let from = text.find("world").unwrap();
        let to = from + "world".len();
        let doc = snapshot(text, to, from);
        let result = execute_command(&doc, "italic", CommandMode::Editor);
        assert_eq!(result.message, "italic applied");
        let op = &result.operations[0];
        assert_eq!(
            op.changes[0],
            TextChange {
                from,
                to: from,
                insert: "*".to_string()
            }
        );
        assert_eq!(
            op.changes[1],
            TextChange {
                from: to,
                to,
                insert: "*".to_string()
            }
        );
    }

    #[test]
    fn format_strike_wraps_selection() {
        let text = "hello world";
        let from = text.find("world").unwrap();
        let to = from + "world".len();
        let doc = snapshot(text, to, from);
        let result = execute_command(&doc, "strike", CommandMode::Editor);
        assert_eq!(result.message, "strikethrough applied");
        let op = &result.operations[0];
        assert_eq!(
            op.changes[0],
            TextChange {
                from,
                to: from,
                insert: "~~".to_string()
            }
        );
        assert_eq!(
            op.changes[1],
            TextChange {
                from: to,
                to,
                insert: "~~".to_string()
            }
        );
    }

    #[test]
    fn format_code_wraps_selection() {
        let text = "hello world";
        let from = text.find("world").unwrap();
        let to = from + "world".len();
        let doc = snapshot(text, to, from);
        let result = execute_command(&doc, "format code", CommandMode::Editor);
        assert_eq!(result.message, "inline code applied");
        let op = &result.operations[0];
        assert_eq!(
            op.changes[0],
            TextChange {
                from,
                to: from,
                insert: "`".to_string()
            }
        );
        assert_eq!(
            op.changes[1],
            TextChange {
                from: to,
                to,
                insert: "`".to_string()
            }
        );
    }
}
