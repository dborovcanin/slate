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

pub fn execute_command(
    _snapshot: &EditorContextSnapshot,
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
        CommandBehavior::Sum => result_with_message("sum unavailable in rust placeholder"),
        CommandBehavior::Date => result_with_message("date unavailable in rust placeholder"),
        CommandBehavior::Format => result_with_message("format unavailable in rust placeholder"),
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
}
