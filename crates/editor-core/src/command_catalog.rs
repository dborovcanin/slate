use crate::types::{CommandMode, CommandSuggestion};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    Sum,
    SumList,
    SumRow,
    SumColumn,
    SumDoc,
    Avg,
    AvgList,
    AvgRow,
    AvgColumn,
    AvgDoc,
    Date,
    Format,
    Checklist,
    Quit,
}

#[derive(Debug, Clone, Copy)]
pub struct CommandDefinition {
    pub id: CommandId,
    pub value: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub modes: &'static [CommandMode],
}

const MODES_BOTH: [CommandMode; 2] = [CommandMode::Vim, CommandMode::Editor];
const MODES_VIM: [CommandMode; 1] = [CommandMode::Vim];

const COMMAND_DEFINITIONS: [CommandDefinition; 14] = [
    CommandDefinition {
        id: CommandId::Sum,
        value: "sum",
        aliases: &[],
        description: "sum paragraph (default scope)",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::SumList,
        value: "sum list",
        aliases: &[],
        description: "sum list at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::SumRow,
        value: "sum row",
        aliases: &["sum_row"],
        description: "sum markdown table per row at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::SumColumn,
        value: "sum column",
        aliases: &["sum_column"],
        description: "sum markdown table per column at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::SumDoc,
        value: "sum doc",
        aliases: &["sum_all", "sum all"],
        description: "sum whole document",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Avg,
        value: "avg",
        aliases: &[],
        description: "average paragraph (default scope)",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::AvgList,
        value: "avg list",
        aliases: &[],
        description: "average list at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::AvgRow,
        value: "avg row",
        aliases: &["avg_row"],
        description: "average markdown table per row at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::AvgColumn,
        value: "avg column",
        aliases: &["avg_column"],
        description: "average markdown table per column at cursor",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::AvgDoc,
        value: "avg doc",
        aliases: &["avg_all", "avg all"],
        description: "average whole document",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Date,
        value: "date",
        aliases: &[],
        description: "insert picked date",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Format,
        value: "format",
        aliases: &["fmt"],
        description: "format markdown document",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Checklist,
        value: "checklist",
        aliases: &["checkbox", "checkboxes", "todo"],
        description: "convert selected lines to checklist",
        modes: &MODES_BOTH,
    },
    CommandDefinition {
        id: CommandId::Quit,
        value: "q",
        aliases: &["q!"],
        description: "quit",
        modes: &MODES_VIM,
    },
];

pub fn normalize_command(input: &str) -> String {
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

pub fn resolve_command(mode: CommandMode, raw_input: &str) -> Option<&'static CommandDefinition> {
    let normalized = normalize_command(raw_input);
    if normalized.is_empty() {
        return None;
    }
    available_commands(mode)
        .into_iter()
        .find(|def| command_matches(def, &normalized))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_command_strips_colon_and_lowercases() {
        assert_eq!(normalize_command(":SUM LIST"), "sum list");
        assert_eq!(normalize_command("  format  "), "format");
    }

    #[test]
    fn resolve_command_is_mode_aware() {
        let editor_q = resolve_command(CommandMode::Editor, "q");
        assert!(editor_q.is_none());

        let vim_q = resolve_command(CommandMode::Vim, "q");
        assert_eq!(vim_q.map(|cmd| cmd.id), Some(CommandId::Quit));
    }

    #[test]
    fn resolve_command_handles_aliases() {
        assert_eq!(
            resolve_command(CommandMode::Editor, "sum_column").map(|cmd| cmd.id),
            Some(CommandId::SumColumn)
        );
        assert_eq!(
            resolve_command(CommandMode::Editor, "avg_all").map(|cmd| cmd.id),
            Some(CommandId::AvgDoc)
        );
    }

    #[test]
    fn suggestions_follow_prefix_then_contains_ordering() {
        let suggestions = list_command_suggestions(CommandMode::Editor, "sum");
        let values = suggestions
            .into_iter()
            .map(|entry| entry.value)
            .collect::<Vec<_>>();
        assert_eq!(values.first().map(|value| value.as_str()), Some("sum"));
        assert!(values.contains(&"sum row".to_string()));
        assert!(values.contains(&"sum column".to_string()));
    }
}
