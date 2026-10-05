use super::{TerminalApp, UiMode};
use crate::terminal::browser::{position_label, Row};
use crate::terminal::help::{filter_help, help_entries, HelpEntry};
use crate::terminal::input::Key;
use crate::terminal::picker::{draw_picker, picker_query_cursor, PickerView};
use ratatui::buffer::Buffer;

/// The help popup over the editor: its query and the entries matching it.
pub(super) struct HelpState {
    query: String,
    entries: Vec<HelpEntry>,
    matches: Vec<usize>,
    selected: usize,
}

const HELP_PAGE: usize = 10;

// Ownership: the help popup (`:help`, `F1`). Its content lives in
// `terminal::help`.
impl TerminalApp {
    pub(super) fn open_help(&mut self) {
        let entries = help_entries(self.command_mode());
        let matches = (0..entries.len()).collect();
        self.help = Some(HelpState {
            query: String::new(),
            entries,
            matches,
            selected: 0,
        });
    }

    pub(super) fn handle_help_key(&mut self, key: Key) {
        let Some(help) = self.help.as_mut() else {
            return;
        };
        let last = help.matches.len().saturating_sub(1);
        match key {
            Key::Esc | Key::F1 => self.help = None,
            Key::Ctrl('q') => self.quit = true,
            Key::ArrowUp | Key::Ctrl('p') => help.selected = help.selected.saturating_sub(1),
            Key::ArrowDown | Key::Ctrl('n') => help.selected = (help.selected + 1).min(last),
            Key::PageUp => help.selected = help.selected.saturating_sub(HELP_PAGE),
            Key::PageDown => help.selected = (help.selected + HELP_PAGE).min(last),
            Key::Enter => {
                let command = help
                    .matches
                    .get(help.selected)
                    .and_then(|&idx| help.entries[idx].command())
                    .map(str::to_string);
                self.help = None;
                if let Some(command) = command {
                    self.open_command_bar_with(&command);
                }
            }
            Key::Backspace => {
                help.query.pop();
                Self::refilter_help(help);
            }
            Key::Ctrl('u') => {
                help.query.clear();
                Self::refilter_help(help);
            }
            Key::Char(ch) => {
                help.query.push(ch);
                Self::refilter_help(help);
            }
            Key::Paste(text) => {
                help.query.push_str(text.lines().next().unwrap_or_default());
                Self::refilter_help(help);
            }
            _ => {}
        }
    }

    fn refilter_help(help: &mut HelpState) {
        help.matches = filter_help(&help.entries, &help.query);
        help.selected = 0;
    }

    /// Opens the command bar holding `command`, ready to run or complete.
    fn open_command_bar_with(&mut self, command: &str) {
        if self.mode == UiMode::Normal {
            self.open_command_bar_from_vim_action();
        } else {
            self.command_input.clear();
            self.dismiss_command_completion_menu();
            self.command_history_index = None;
            self.command_bar_from_normal = false;
            self.command_selection = None;
            self.command_selection_linewise = false;
            self.mode = UiMode::CommandBar;
        }
        self.command_input = command.to_string();
        self.command_cursor = usize::MAX;
        self.update_command_status();
    }

    /// Draws the help popup when open; returns the screen cell of its query
    /// cursor.
    pub(super) fn draw_help(
        &self,
        buf: &mut Buffer,
        rows: usize,
        cols: usize,
    ) -> Option<(usize, usize)> {
        let help = self.help.as_ref()?;
        let look = self.look();
        let palette = look.palette;
        draw_picker(
            &PickerView {
                look,
                title: "Help",
                footer: position_label(Some(help.selected), help.matches.len()),
                query: &help.query,
                cursor: usize::MAX,
                placeholder: "find a key or command",
                len: help.matches.len(),
                selected: Some(help.selected),
                row_at: |pos: usize| {
                    let entry = &help.entries[help.matches[pos]];
                    Row {
                        marker: None,
                        icon: entry.section.tag(),
                        icon_fg: palette.primary(),
                        text: entry.action.clone(),
                        text_fg: palette.text_fg(),
                        bold: false,
                        right: entry.keys.clone(),
                    }
                },
                empty_hint: "no match",
                hints: &[("Enter", "use command"), ("↑↓", "move"), ("Esc", "close")],
            },
            buf,
            rows,
            cols,
        );
        Some(picker_query_cursor(rows, cols, &help.query, usize::MAX))
    }
}
