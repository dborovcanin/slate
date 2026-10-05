//! Help popup content (`:help`, `F1`): key bindings, and every command from
//! the catalog with its description. The popup filters it as you type.

use crate::editor_core::engine::EditorEngine;
use crate::editor_core::types::CommandMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpSection {
    General,
    Insert,
    Normal,
    Table,
    Search,
    Command,
}

impl HelpSection {
    /// One-cell tag in the icon column.
    pub fn tag(self) -> &'static str {
        match self {
            Self::General => "*",
            Self::Insert => "I",
            Self::Normal => "N",
            Self::Table => "T",
            Self::Search => "/",
            Self::Command => ":",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Insert => "insert",
            Self::Normal => "normal vim",
            Self::Table => "table",
            Self::Search => "search",
            Self::Command => "command",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpEntry {
    pub section: HelpSection,
    /// Keys, or `:command` for a command.
    pub keys: String,
    pub action: String,
}

impl HelpEntry {
    /// The command to prefill the command bar with, for a command entry.
    pub fn command(&self) -> Option<&str> {
        (self.section == HelpSection::Command).then(|| self.keys.trim_start_matches(':'))
    }

    /// Whether every word of `query` appears in the entry, ignoring case.
    fn matches(&self, words: &[String]) -> bool {
        let haystack =
            format!("{} {} {}", self.section.name(), self.keys, self.action).to_lowercase();
        words.iter().all(|word| haystack.contains(word.as_str()))
    }
}

use HelpSection::{General, Insert, Normal, Search, Table};

const KEYS: &[(HelpSection, &str, &str)] = &[
    (General, "F1, :help", "This help"),
    (
        General,
        "Ctrl+P",
        "Note switcher; Tab there searches note text",
    ),
    (General, "Ctrl+N", "New note"),
    (General, "Ctrl+G", "Collection picker"),
    (General, "Ctrl+B", "Collection browser"),
    (General, "Ctrl+S", "Save now"),
    (General, "Ctrl+Q", "Quit"),
    (General, "Ctrl+]", "Follow wiki link"),
    (Insert, "Enter", "New line, continue list, accept popup"),
    (
        Insert,
        "Tab / Shift+Tab",
        "Accept completion, apply result, next cell, indent",
    ),
    (
        Insert,
        "Ctrl+W, Ctrl+Backspace",
        "Delete word before the cursor",
    ),
    (Insert, "Ctrl+Delete", "Delete forward"),
    (Insert, "Ctrl+Left / Ctrl+Right", "Previous / next word"),
    (Insert, "[[", "Insert link, with note completion"),
    (Insert, "Ctrl+O", "Preview image (vim mode off)"),
    (Insert, "Esc", "Close popup, else Normal mode"),
    (
        Normal,
        "h j k l, w b, 0 $",
        "Move: char, line, word, line ends",
    ),
    (Normal, "gg / G, {n}G", "First / last line, line n"),
    (Normal, "gj / gk", "Down / up a screen row"),
    (Normal, "i a I A o O", "Enter Insert"),
    (
        Normal,
        "x, dd, yy, cc, C",
        "Delete char / line, yank, change",
    ),
    (
        Normal,
        "d y c + motion",
        "Operator with motion (d2w, y$, dt,)",
    ),
    (
        Normal,
        "diw ciw yi| ci( ...",
        "Word, cell and delimiter text objects",
    ),
    (
        Normal,
        "p / P",
        "Paste after / before (system clipboard if empty)",
    ),
    (Normal, "u / Ctrl+R", "Undo / redo"),
    (Normal, "v / V", "Visual / visual-line"),
    (Normal, ": , Ctrl+E", "Command bar"),
    (Normal, "/, Ctrl+F", "Search in note"),
    (Normal, "n / N", "Next / previous match"),
    (Normal, "za", "Toggle fold"),
    (Normal, "gd", "Follow link, or go to variable definition"),
    (Normal, "K", "Preview linked note"),
    (Normal, "gx", "Preview image"),
    (Normal, "?", "Web search"),
    (Normal, "q{r} / @{r}", "Record / replay macro"),
    (Table, "|", "Start a row / add a column"),
    (Table, "Tab / Shift+Tab", "Next / previous cell"),
    (Table, "Shift+Enter", "Split cell into a continuation row"),
    (
        Table,
        "Ctrl+Backspace, Ctrl+Delete",
        "Merge cells or delete a column",
    ),
    (Table, "paste CSV / TSV", "Becomes a table"),
    (Search, "Tab / Shift+Tab, arrows", "Next / previous match"),
    (Search, "Enter", "Keep the position"),
    (Search, "Esc", "Cancel and go back"),
];

/// Key bindings, then the commands available in `mode`.
pub fn help_entries(mode: CommandMode) -> Vec<HelpEntry> {
    let keys = KEYS.iter().map(|(section, keys, action)| HelpEntry {
        section: *section,
        keys: (*keys).to_string(),
        action: (*action).to_string(),
    });
    let commands = EditorEngine::list_command_suggestions(mode, "")
        .into_iter()
        .map(|suggestion| HelpEntry {
            section: HelpSection::Command,
            keys: format!(":{}", suggestion.value),
            action: suggestion.description,
        });
    keys.chain(commands).collect()
}

/// Indexes of the entries matching `query`, in order.
pub fn filter_help(entries: &[HelpEntry], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    (0..entries.len())
        .filter(|&idx| entries[idx].matches(&words))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_keys_then_every_command() {
        let entries = help_entries(CommandMode::Vim);
        assert_eq!(entries[0].keys, "F1, :help");
        assert!(entries.iter().any(|entry| entry.keys == ":paste-image"));
        assert!(entries.iter().any(|entry| entry.keys == ":help"));
        let command = entries
            .iter()
            .find(|entry| entry.keys == ":paste-image")
            .expect("paste-image listed");
        assert_eq!(command.command(), Some("paste-image"));
        assert_eq!(entries[0].command(), None);
    }

    #[test]
    fn filters_by_every_word_across_section_keys_and_action() {
        let entries = help_entries(CommandMode::Vim);
        let found = |query: &str| -> Vec<&str> {
            filter_help(&entries, query)
                .into_iter()
                .map(|idx| entries[idx].keys.as_str())
                .collect()
        };
        assert_eq!(found("UNDO"), vec!["u / Ctrl+R"]);
        assert!(found("table next cell").contains(&"Tab / Shift+Tab"));
        assert!(found("command clipboard").contains(&":paste-image"));
        assert_eq!(found("").len(), entries.len());
        assert!(found("no such thing anywhere").is_empty());
    }
}
