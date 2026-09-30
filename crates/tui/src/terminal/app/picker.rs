use super::switcher::NoteMeta;
use super::{TerminalApp, UiMode};
use crate::terminal::browser::{collection_entry_row, plain_note_row, position_label, NoteEntry};
use crate::terminal::picker::{draw_picker, PickerView};
use crate::terminal::switcher;
use app_core::storage::{NoteAccessMode, NoteSearchResult};
use ratatui::buffer::Buffer;

fn note_entry_from_meta(meta: &NoteMeta, daily_prefix: &str) -> NoteEntry {
    NoteEntry {
        id: meta.id.clone(),
        title: meta.title.clone(),
        access_mode: meta.access_mode,
        is_unlocked: meta.is_unlocked,
        updated_at: meta.updated_at.clone(),
        is_daily: app_core::daily::is_daily_note_id(daily_prefix, &meta.id),
    }
}

fn note_entry_from_result(result: &NoteSearchResult, daily_prefix: &str) -> NoteEntry {
    NoteEntry {
        id: result.id.clone(),
        title: result.title.clone(),
        access_mode: NoteAccessMode::None,
        is_unlocked: true,
        updated_at: result.updated_at.clone(),
        is_daily: app_core::daily::is_daily_note_id(daily_prefix, &result.id),
    }
}

/// `Notes · in Work` / `Notes · all notes`.
fn scoped_title(base: &str, filter_id: Option<&String>, filter_name: Option<&str>) -> String {
    match filter_id
        .and(filter_name)
        .filter(|name| !name.trim().is_empty())
    {
        Some(name) => format!("{base} · in {name}"),
        None => format!("{base} · all notes"),
    }
}

// Ownership: drawing of the note switcher, its content search and the
// collection picker popups. Their keys stay in `command_search_switcher`.
impl TerminalApp {
    /// Status row text while a picker is open: a confirmation prompt, the
    /// edit dialog's keys, or the last status message.
    pub(super) fn picker_status_message(&self) -> Option<String> {
        if let Some(confirm) = self.switcher.open_confirm.as_ref() {
            return Some(format!(
                "Open {}: type password, Enter confirm, Esc cancel",
                Self::access_mode_prompt_label(confirm.access_mode)
            ));
        }
        if let Some(confirm) = self.switcher.delete_confirm.as_ref() {
            return Some(if confirm.requires_password {
                format!(
                    "Confirm delete {}: type password, Enter confirm, Esc cancel",
                    Self::access_mode_prompt_label(confirm.access_mode)
                )
            } else {
                "Confirm delete: Enter/Y confirm, Esc/N cancel".to_string()
            });
        }
        if self.collection_switcher.edit_dialog.is_some() {
            return Some("Tab/Shift+Tab field, Enter save, Esc cancel".to_string());
        }
        None
    }

    /// Draws the open picker popup and its dialogs over the editor.
    pub(super) fn draw_picker_popup(&self, buf: &mut Buffer, rows: usize, cols: usize) {
        let look = self.look();
        let prefix = self.daily_config.note_prefix.as_str();
        let working = self.working_collection_id.is_some();
        match self.mode {
            UiMode::Switcher => {
                let state = &self.switcher;
                let title = scoped_title(
                    "Notes",
                    state.collection_filter_id.as_ref(),
                    state.collection_filter_name.as_deref(),
                );
                let mut hints = vec![
                    ("Enter", "open"),
                    ("Tab", "search text"),
                    ("Ctrl+R", "history"),
                ];
                if working {
                    hints.push(("Ctrl+L", "all / working"));
                }
                hints.extend([("Ctrl+N", "new"), ("Del", "delete"), ("Esc", "close")]);
                let empty_hint = if state.query.trim().is_empty() {
                    "no notes yet · Ctrl+N creates one"
                } else {
                    "no match · Tab searches note text"
                };
                draw_picker(
                    &PickerView {
                        look,
                        title: &title,
                        footer: position_label(state.selected, state.matches.len()),
                        query: &state.query,
                        cursor: usize::MAX,
                        placeholder: "find a note by title",
                        len: state.matches.len(),
                        selected: state.selected,
                        row_at: |pos: usize| {
                            plain_note_row(
                                &look,
                                &note_entry_from_meta(&state.items[state.matches[pos]], prefix),
                            )
                        },
                        empty_hint,
                        hints: &hints,
                    },
                    buf,
                    rows,
                    cols,
                );
            }
            UiMode::ContentSearch => {
                let state = &self.content_search;
                let busy = state.pending || state.rx.is_some();
                let title = scoped_title(
                    "Search text",
                    state.collection_filter_id.as_ref(),
                    state.collection_filter_name.as_deref(),
                );
                let mut hints = vec![("Enter", "open at match"), ("Tab", "titles")];
                if working {
                    hints.push(("Ctrl+L", "all / working"));
                }
                hints.push(("Esc", "close"));
                let empty_hint = if state.query.trim().is_empty() {
                    "type to search note text"
                } else if busy {
                    "searching…"
                } else {
                    "no matches"
                };
                draw_picker(
                    &PickerView {
                        look,
                        title: &title,
                        footer: position_label(state.selected, state.results.len()),
                        query: &state.query,
                        cursor: state.cursor_col,
                        placeholder: "search note text",
                        len: state.results.len(),
                        selected: state.selected,
                        row_at: |pos: usize| {
                            let result = &state.results[pos];
                            let mut row =
                                plain_note_row(&look, &note_entry_from_result(result, prefix));
                            row.right = format!(":{}", result.line_number);
                            row
                        },
                        empty_hint,
                        hints: &hints,
                    },
                    buf,
                    rows,
                    cols,
                );
            }
            UiMode::CollectionSwitcher => {
                let state = &self.collection_switcher;
                draw_picker(
                    &PickerView {
                        look,
                        title: "Collections",
                        footer: position_label(Some(state.selected), state.matches.len()),
                        query: &state.query,
                        cursor: usize::MAX,
                        placeholder: "find a collection",
                        len: state.matches.len(),
                        selected: Some(state.selected),
                        row_at: |pos: usize| {
                            collection_entry_row(&look, &state.entries[state.matches[pos]], false)
                        },
                        empty_hint: "no match",
                        hints: &[("Enter", "work in"), ("Ctrl+E", "edit"), ("Esc", "close")],
                    },
                    buf,
                    rows,
                    cols,
                );
            }
            _ => return,
        }

        let palette = self.render_palette;
        if let Some(confirm) = self.switcher.delete_confirm.as_ref() {
            switcher::draw_delete_confirm(
                &confirm.note_title,
                confirm.requires_password,
                confirm.password.chars().count(),
                buf,
                rows,
                cols,
                palette,
            );
        }
        if let Some(confirm) = self.switcher.open_confirm.as_ref() {
            switcher::draw_open_confirm(
                &confirm.note_title,
                confirm.password.chars().count(),
                buf,
                rows,
                cols,
                palette,
            );
        }
        if let Some(dialog) = self.collection_switcher.edit_dialog.as_ref() {
            switcher::draw_collection_edit_dialog(
                &switcher::CollectionEditView {
                    name: &dialog.name,
                    description: &dialog.description,
                    default_tags: &dialog.default_tags,
                    selected_field: dialog.selected_field,
                },
                buf,
                rows,
                cols,
                palette,
            );
        }
    }
}
