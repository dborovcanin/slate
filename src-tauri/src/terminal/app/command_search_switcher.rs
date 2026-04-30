use super::{
    line_char_len, load_note_reminder_ghosts, new_note, trim_trailing_word,
    CommandCompletionMenuState, CommandCompletionOption, DatePickerAction, Db, Key, Note,
    NoteSearchResult, SwitcherDeleteConfirm, SwitcherOpenConfirm, TerminalApp, UiMode,
    CALC_VIEWPORT_ONLY_MIN_LINES, COMMAND_COMPLETION_MAX_OPTIONS, CONTENT_SEARCH_DEBOUNCE_MS,
    MAX_COMMAND_HISTORY_ENTRIES,
};
use crate::terminal::text_utils::{byte_index, join_lines, split_lines};
use crate::terminal::{notifications, switcher};
use app_core::storage::{NoteAccessMode, NoteModules};
use std::time::{Duration, Instant};

fn note_sources(db: &Db) -> app_core::note_sources::NoteSourceService {
    app_core::note_sources::NoteSourceService::new(db.clone())
}

// Ownership: switcher, command bar execution, and search workflows.
impl TerminalApp {
    pub(super) fn rebuild_wiki_link_prefix_index(&mut self) {
        self.wiki_link_prefix_index.clear();
        for note in &self.switcher_items {
            let short_id: String = note.id.chars().take(8).collect();
            if short_id.len() != 8 || !short_id.chars().all(|ch| ch.is_ascii_alphanumeric()) {
                continue;
            }
            let replace = self
                .wiki_link_prefix_index
                .get(&short_id)
                .map(|existing| {
                    note.updated_at > existing.updated_at
                        || (note.updated_at == existing.updated_at && note.id < existing.note_id)
                })
                .unwrap_or(true);
            if replace {
                self.wiki_link_prefix_index.insert(
                    short_id,
                    super::WikiLinkPrefixIndexEntry {
                        title: note.title.clone(),
                        updated_at: note.updated_at.clone(),
                        note_id: note.id.clone(),
                    },
                );
            }
        }
    }

    fn content_search_title_fallback_results(&self, query: &str) -> Vec<NoteSearchResult> {
        const CONTENT_SEARCH_FALLBACK_LIMIT: usize = 60;

        let mut ordered = Vec::with_capacity(self.switcher_items.len());
        if let Some(active_idx) = self
            .switcher_items
            .iter()
            .position(|note| note.id == self.active_note.id)
        {
            ordered.push((0usize, &self.switcher_items[active_idx]));
        }
        for (idx, note) in self.switcher_items.iter().enumerate() {
            if note.id != self.active_note.id {
                ordered.push((idx + 1, note));
            }
        }

        let trimmed = query.trim();
        if trimmed.is_empty() {
            return ordered
                .into_iter()
                .take(CONTENT_SEARCH_FALLBACK_LIMIT)
                .map(|(_, note)| NoteSearchResult {
                    id: note.id.clone(),
                    title: note.title.clone(),
                    snippet: String::new(),
                    line_number: 1,
                    rank: 0.0,
                    updated_at: String::new(),
                })
                .collect();
        }

        let mut matches = ordered
            .into_iter()
            .filter_map(|(idx, note)| {
                switcher::fuzzy_score(trimmed, &note.title).map(|score| (score, idx, note))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left_score, left_idx, _), (right_score, right_idx, _)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_idx.cmp(right_idx))
        });

        matches
            .into_iter()
            .take(CONTENT_SEARCH_FALLBACK_LIMIT)
            .map(|(score, _, note)| NoteSearchResult {
                id: note.id.clone(),
                title: note.title.clone(),
                snippet: String::new(),
                line_number: 1,
                rank: -(score as f64),
                updated_at: String::new(),
            })
            .collect()
    }

    fn clear_content_search_session(&mut self) {
        // Move any in-flight receiver into the detached pool so the stale worker
        // can drain without blocking the next dialog session.
        if let Some(rx) = self.content_search_rx.take() {
            self.content_search_detached_rxs.push(rx);
        }
        self.content_search_query.clear();
        self.content_search_results.clear();
        self.content_search_selected = 0;
        self.content_search_pending = false;
        self.content_search_debounce_until = None;
    }

    fn refresh_content_search_preview(&mut self) {
        let query = self.content_search_query.trim().to_string();
        // Keep at most one active worker per visible dialog. Query changes are
        // coalesced via `content_search_pending` and dispatched once the active
        // worker resolves.
        self.content_search_results = self.content_search_title_fallback_results(&query);
        self.content_search_selected = 0;
        self.content_search_pending = !query.is_empty();
        self.content_search_debounce_until = if query.is_empty() {
            None
        } else {
            Some(Instant::now() + Duration::from_millis(CONTENT_SEARCH_DEBOUNCE_MS))
        };
    }

    pub(super) fn handle_switcher_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        if self.switcher_open_confirm.is_some() {
            return self.handle_switcher_open_confirm_key(db, key);
        }
        if self.switcher_delete_confirm.is_some() {
            return self.handle_switcher_delete_confirm_key(db, key);
        }

        match key {
            Key::Esc | Key::Ctrl('p') => {
                self.close_switcher();
            }
            Key::Ctrl('q') => {
                self.quit = true;
            }
            Key::Ctrl('w') => {
                trim_trailing_word(&mut self.switcher_query);
                self.recompute_switcher_matches();
            }
            Key::Ctrl('n') => {
                self.close_switcher();
                self.handle_editor_key(db, Key::Ctrl('n'))?;
            }
            Key::ArrowUp => {
                if self.switcher_selected > 0 {
                    self.switcher_selected -= 1;
                }
            }
            Key::ArrowDown => {
                if self.switcher_selected + 1 < self.switcher_matches.len() {
                    self.switcher_selected += 1;
                }
            }
            Key::Backspace => {
                self.switcher_query.pop();
                self.recompute_switcher_matches();
            }
            Key::Delete | Key::CtrlBackspace => {
                self.request_switcher_delete_confirmation(db);
            }
            Key::Enter => {
                if let Some(idx) = self.switcher_matches.get(self.switcher_selected).copied() {
                    let item = self.switcher_items[idx].clone();
                    if item.access_mode != NoteAccessMode::None && !item.is_unlocked {
                        self.switcher_open_confirm = Some(SwitcherOpenConfirm {
                            note_id: item.id,
                            note_title: item.title,
                            password: String::new(),
                            line_number: None,
                        });
                    } else {
                        self.open_note_from_switcher(db, item.id.as_str(), None, None)?;
                    }
                }
            }
            Key::Char(ch) => {
                self.switcher_query.push(ch);
                self.recompute_switcher_matches();
            }
            Key::Paste(text) => {
                for ch in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                    self.switcher_query.push(ch);
                }
                self.recompute_switcher_matches();
            }
            Key::Tab => {
                self.open_content_search(db)?;
            }
            Key::CtrlDelete
            | Key::BackTab
            | Key::ArrowLeft
            | Key::ArrowRight
            | Key::CtrlArrowLeft
            | Key::CtrlArrowRight
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown
            | Key::Ctrl(_) => {}
        }
        Ok(())
    }

    pub(super) fn handle_switcher_delete_confirm_key(
        &mut self,
        db: &Db,
        key: Key,
    ) -> Result<(), String> {
        match key {
            Key::Ctrl('q') => {
                self.quit = true;
            }
            Key::Ctrl('p') => {
                self.close_switcher();
            }
            Key::Esc => {
                self.switcher_delete_confirm = None;
            }
            Key::Enter => {
                if let Some(confirm) = self.switcher_delete_confirm.clone() {
                    if confirm.requires_password && confirm.password.trim().is_empty() {
                        self.status = "password required to delete protected note".to_string();
                        return Ok(());
                    }
                    let password = if confirm.requires_password {
                        Some(confirm.password.as_str())
                    } else {
                        None
                    };
                    match self.delete_note_from_switcher(
                        db,
                        &confirm.note_id,
                        &confirm.note_title,
                        password,
                    ) {
                        Ok(()) => {
                            self.switcher_delete_confirm = None;
                        }
                        Err(error) => {
                            self.status = format!("delete failed: {error}");
                            if let Some(current) = self.switcher_delete_confirm.as_mut() {
                                current.password.clear();
                            }
                        }
                    }
                }
            }
            Key::Char('y') => {
                let requires_password = self
                    .switcher_delete_confirm
                    .as_ref()
                    .map(|confirm| confirm.requires_password)
                    .unwrap_or(false);
                if !requires_password {
                    if let Some(confirm) = self.switcher_delete_confirm.clone() {
                        match self.delete_note_from_switcher(
                            db,
                            &confirm.note_id,
                            &confirm.note_title,
                            None,
                        ) {
                            Ok(()) => {
                                self.switcher_delete_confirm = None;
                            }
                            Err(error) => {
                                self.status = format!("delete failed: {error}");
                            }
                        }
                    }
                } else if let Some(confirm) = self.switcher_delete_confirm.as_mut() {
                    confirm.password.push('y');
                }
            }
            Key::Char('Y') => {
                let requires_password = self
                    .switcher_delete_confirm
                    .as_ref()
                    .map(|confirm| confirm.requires_password)
                    .unwrap_or(false);
                if !requires_password {
                    if let Some(confirm) = self.switcher_delete_confirm.clone() {
                        match self.delete_note_from_switcher(
                            db,
                            &confirm.note_id,
                            &confirm.note_title,
                            None,
                        ) {
                            Ok(()) => {
                                self.switcher_delete_confirm = None;
                            }
                            Err(error) => {
                                self.status = format!("delete failed: {error}");
                            }
                        }
                    }
                } else if let Some(confirm) = self.switcher_delete_confirm.as_mut() {
                    confirm.password.push('Y');
                }
            }
            Key::Char('n') => {
                let requires_password = self
                    .switcher_delete_confirm
                    .as_ref()
                    .map(|confirm| confirm.requires_password)
                    .unwrap_or(false);
                if !requires_password {
                    self.switcher_delete_confirm = None;
                } else if let Some(confirm) = self.switcher_delete_confirm.as_mut() {
                    confirm.password.push('n');
                }
            }
            Key::Char('N') => {
                let requires_password = self
                    .switcher_delete_confirm
                    .as_ref()
                    .map(|confirm| confirm.requires_password)
                    .unwrap_or(false);
                if !requires_password {
                    self.switcher_delete_confirm = None;
                } else if let Some(confirm) = self.switcher_delete_confirm.as_mut() {
                    confirm.password.push('N');
                }
            }
            Key::Backspace | Key::CtrlBackspace => {
                if let Some(confirm) = self.switcher_delete_confirm.as_mut() {
                    if confirm.requires_password {
                        confirm.password.pop();
                    }
                }
            }
            Key::Paste(text) => {
                if let Some(confirm) = self.switcher_delete_confirm.as_mut() {
                    if confirm.requires_password {
                        for ch in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                            confirm.password.push(ch);
                        }
                    }
                }
            }
            Key::Char(ch) => {
                if let Some(confirm) = self.switcher_delete_confirm.as_mut() {
                    if confirm.requires_password {
                        confirm.password.push(ch);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_switcher_open_confirm_key(
        &mut self,
        db: &Db,
        key: Key,
    ) -> Result<(), String> {
        match key {
            Key::Ctrl('q') => {
                self.quit = true;
            }
            Key::Ctrl('p') => {
                self.close_switcher();
            }
            Key::Esc => {
                self.switcher_open_confirm = None;
            }
            Key::Enter => {
                if let Some(confirm) = self.switcher_open_confirm.clone() {
                    if confirm.password.trim().is_empty() {
                        self.status = "password required to open protected note".to_string();
                        return Ok(());
                    }
                    match self.open_note_from_switcher(
                        db,
                        &confirm.note_id,
                        Some(confirm.password.as_str()),
                        confirm.line_number,
                    ) {
                        Ok(()) => {
                            self.switcher_open_confirm = None;
                        }
                        Err(error) => {
                            self.status = format!("open failed: {error}");
                            if let Some(current) = self.switcher_open_confirm.as_mut() {
                                current.password.clear();
                            }
                        }
                    }
                }
            }
            Key::Backspace | Key::CtrlBackspace => {
                if let Some(confirm) = self.switcher_open_confirm.as_mut() {
                    confirm.password.pop();
                }
            }
            Key::Paste(text) => {
                if let Some(confirm) = self.switcher_open_confirm.as_mut() {
                    for ch in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                        confirm.password.push(ch);
                    }
                }
            }
            Key::Char(ch) => {
                if let Some(confirm) = self.switcher_open_confirm.as_mut() {
                    confirm.password.push(ch);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn open_note_from_switcher(
        &mut self,
        db: &Db,
        note_id: &str,
        password: Option<&str>,
        line_number: Option<usize>,
    ) -> Result<(), String> {
        if self.autosave_enabled {
            self.save(db)?;
        }
        let note = if let Some(password) = password {
            db.unlock_note(note_id, password)?
        } else {
            let note = note_sources(db).open_note_by_id(note_id)?;
            let Some(note) = note else {
                self.status = format!("note missing {}", note_id);
                return Ok(());
            };
            note
        };
        self.set_active_note(db, note)?;
        if let Some(line_number) = line_number {
            let max_line = self.lines.len().saturating_sub(1);
            self.cursor_line = line_number.saturating_sub(1).min(max_line);
            self.cursor_col = 0;
            self.adjust_cursor();
            self.adjust_scroll();
        }
        // Opening a note finalizes the previous content-search session.
        // Drop any stale async receiver/results so the next content search
        // always starts from a clean state.
        self.clear_content_search_session();
        self.close_switcher();
        self.mode = UiMode::Normal;
        self.vim_state.mode = crate::editor_core::vim::VimMode::Normal;
        self.selection_anchor = None;
        self.command_selection = None;
        self.status = "-- NORMAL --".to_string();
        Ok(())
    }

    pub(super) fn request_switcher_delete_confirmation(&mut self, db: &Db) {
        if let Some(idx) = self.switcher_matches.get(self.switcher_selected).copied() {
            let item = &self.switcher_items[idx];
            let capabilities = note_sources(db).capabilities_for_note_id(&item.id);
            if !capabilities.can_delete {
                self.status = "file-backed notes are not deleted via switcher".to_string();
                self.switcher_delete_confirm = None;
                return;
            }
            let requires_password = match db.get_note_meta(&item.id) {
                Ok(Some(note)) => matches!(
                    note.access_mode,
                    NoteAccessMode::Locked | NoteAccessMode::Encrypted
                ),
                Ok(None) => false,
                Err(error) => {
                    self.status = format!("delete check failed: {error}");
                    false
                }
            };
            self.switcher_delete_confirm = Some(SwitcherDeleteConfirm {
                note_id: item.id.clone(),
                note_title: item.title.clone(),
                requires_password,
                password: String::new(),
            });
        }
    }

    pub(super) fn delete_note_from_switcher(
        &mut self,
        db: &Db,
        note_id: &str,
        note_title: &str,
        password: Option<&str>,
    ) -> Result<(), String> {
        let capabilities = note_sources(db).capabilities_for_note_id(note_id);
        if !capabilities.can_delete {
            self.status = "file-backed notes are not deleted via switcher".to_string();
            self.refresh_switcher_items(db)?;
            return Ok(());
        }
        let deleting_active = self.active_note.id == note_id;
        let deleted = db.delete_note(note_id, password)?;
        if !deleted {
            self.status = format!("note missing {}", note_id);
            self.refresh_switcher_items(db)?;
            return Ok(());
        }

        if deleting_active {
            if let Some(note) = db.get_most_recent_note()? {
                self.set_active_note(db, note)?;
            } else {
                let note = new_note(db, &crate::config::load_theme_config())?;
                self.set_active_note(db, note)?;
            }
        } else {
            self.refresh_switcher_items(db)?;
        }

        self.status = format!("deleted {}", note_title);
        Ok(())
    }

    pub(super) fn command_mode(&self) -> crate::editor_core::types::CommandMode {
        if self.command_bar_from_normal {
            crate::editor_core::types::CommandMode::Vim
        } else {
            crate::editor_core::types::CommandMode::Editor
        }
    }

    pub(super) fn remember_command_in_history(&mut self, command: &str) {
        crate::editor_core::command_history::remember_command(
            &mut self.command_history,
            command,
            MAX_COMMAND_HISTORY_ENTRIES,
        );
        self.command_history_index = None;
    }

    pub(super) fn dismiss_command_completion_menu(&mut self) {
        self.command_completion = CommandCompletionMenuState::default();
    }

    pub(super) fn build_command_completion_menu(&self) -> Option<CommandCompletionMenuState> {
        let suggestions = crate::editor_core::commands::list_command_suggestions(
            self.command_mode(),
            &self.command_input,
        );
        if suggestions.is_empty() {
            return None;
        }

        let normalized_input =
            crate::editor_core::command_catalog::normalize_command(&self.command_input);
        let ends_with_space = self
            .command_input
            .chars()
            .last()
            .is_some_and(char::is_whitespace);
        let typed_tokens = if normalized_input.is_empty() {
            Vec::new()
        } else {
            normalized_input
                .split_whitespace()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };

        let (prefix_tokens, token_prefix) = if ends_with_space {
            (typed_tokens, String::new())
        } else if let Some((last, prefix)) = typed_tokens.split_last() {
            (prefix.to_vec(), last.to_string())
        } else {
            (Vec::new(), String::new())
        };
        let token_index = prefix_tokens.len();

        let mut options: Vec<CommandCompletionOption> = Vec::new();
        for suggestion in suggestions {
            let suggestion_tokens = suggestion.value.split_whitespace().collect::<Vec<_>>();
            if suggestion_tokens.len() <= token_index {
                continue;
            }
            if !prefix_tokens.iter().enumerate().all(|(idx, token)| {
                suggestion_tokens
                    .get(idx)
                    .is_some_and(|candidate| *candidate == token.as_str())
            }) {
                continue;
            }
            let token = suggestion_tokens[token_index];
            if !token.starts_with(&token_prefix) {
                continue;
            }
            let has_more = suggestion_tokens.len() > token_index + 1;
            if let Some(existing) = options.iter_mut().find(|entry| entry.token == token) {
                existing.has_more |= has_more;
                continue;
            }
            options.push(CommandCompletionOption {
                token: token.to_string(),
                has_more,
            });
        }

        if options.is_empty() {
            return None;
        }
        if options.len() > COMMAND_COMPLETION_MAX_OPTIONS {
            options.truncate(COMMAND_COMPLETION_MAX_OPTIONS);
        }

        Some(CommandCompletionMenuState {
            visible: true,
            prefix_tokens,
            options,
            selected_index: 0,
        })
    }

    pub(super) fn open_command_completion_menu(&mut self) -> bool {
        let Some(menu) = self.build_command_completion_menu() else {
            self.dismiss_command_completion_menu();
            self.update_command_status();
            return false;
        };
        if menu.options.len() == 1 {
            self.command_completion = menu;
            return self.apply_command_completion_selection();
        }
        self.command_completion = menu;
        self.update_command_status();
        true
    }

    pub(super) fn move_command_completion_selection(&mut self, delta: isize) -> bool {
        if !self.command_completion.visible || self.command_completion.options.is_empty() {
            return false;
        }
        let len = self.command_completion.options.len();
        let selected = self
            .command_completion
            .selected_index
            .min(len.saturating_sub(1)) as isize;
        let next = (selected + delta).rem_euclid(len as isize) as usize;
        self.command_completion.selected_index = next;
        self.update_command_status();
        true
    }

    pub(super) fn apply_command_completion_selection(&mut self) -> bool {
        if !self.command_completion.visible || self.command_completion.options.is_empty() {
            return false;
        }
        let selected_idx = self
            .command_completion
            .selected_index
            .min(self.command_completion.options.len().saturating_sub(1));
        let selected = self.command_completion.options[selected_idx].clone();
        let mut tokens = self.command_completion.prefix_tokens.clone();
        tokens.push(selected.token);
        self.command_input = tokens.join(" ");
        if selected.has_more {
            self.command_input.push(' ');
        }
        self.command_history_index = None;
        self.dismiss_command_completion_menu();
        self.update_command_status();
        true
    }

    pub(super) fn cycle_command_history_prev(&mut self) {
        let Some(step) = crate::editor_core::command_history::cycle_prev(
            &self.command_history,
            self.command_history_index,
        ) else {
            return;
        };
        self.dismiss_command_completion_menu();
        self.command_history_index = Some(step.index);
        self.command_input = step.command;
        self.update_command_status();
    }

    pub(super) fn cycle_command_history_next(&mut self) {
        let Some(step) = crate::editor_core::command_history::cycle_next(
            &self.command_history,
            self.command_history_index,
        ) else {
            return;
        };
        self.dismiss_command_completion_menu();
        self.command_history_index = Some(step.index);
        self.command_input = step.command;
        self.update_command_status();
    }

    pub(super) fn handle_command_bar_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        match key {
            Key::Esc => {
                if self.command_completion.visible {
                    self.dismiss_command_completion_menu();
                    self.update_command_status();
                    return Ok(());
                }
                self.mode = if self.command_bar_from_normal {
                    UiMode::Normal
                } else {
                    UiMode::Editor
                };
                self.command_input.clear();
                self.dismiss_command_completion_menu();
                self.command_history_index = None;
                self.command_selection = None;
                self.command_selection_linewise = false;
                self.status = if self.command_bar_from_normal {
                    "-- NORMAL --".to_string()
                } else {
                    format!("editing {}", self.active_note.id)
                };
            }
            Key::Enter => {
                if self.apply_command_completion_selection() {
                    return Ok(());
                }
                let cmd = self.command_input.clone();
                let return_to = if self.command_bar_from_normal {
                    UiMode::Normal
                } else {
                    UiMode::Editor
                };
                self.mode = return_to;
                self.command_input.clear();
                self.dismiss_command_completion_menu();
                self.command_history_index = None;
                if !cmd.trim().is_empty() {
                    self.remember_command_in_history(&cmd);
                    self.execute_terminal_command(db, &cmd);
                }
                self.command_selection = None;
                self.command_selection_linewise = false;
            }
            Key::Tab => {
                self.command_history_index = None;
                if self.command_completion.visible {
                    self.move_command_completion_selection(1);
                } else {
                    self.open_command_completion_menu();
                }
            }
            Key::ArrowLeft => {
                self.move_command_completion_selection(-1);
            }
            Key::ArrowRight => {
                self.move_command_completion_selection(1);
            }
            Key::Backspace => {
                self.command_history_index = None;
                self.dismiss_command_completion_menu();
                self.command_input.pop();
                if self.command_input.is_empty() {
                    self.mode = if self.command_bar_from_normal {
                        UiMode::Normal
                    } else {
                        UiMode::Editor
                    };
                    self.dismiss_command_completion_menu();
                    self.command_selection = None;
                    self.command_selection_linewise = false;
                    self.status = if self.command_bar_from_normal {
                        "-- NORMAL --".to_string()
                    } else {
                        format!("editing {}", self.active_note.id)
                    };
                } else {
                    self.update_command_status();
                }
            }
            Key::Ctrl('w') => {
                self.command_history_index = None;
                self.dismiss_command_completion_menu();
                trim_trailing_word(&mut self.command_input);
                self.update_command_status();
            }
            Key::ArrowUp => {
                self.cycle_command_history_prev();
            }
            Key::ArrowDown => {
                self.cycle_command_history_next();
            }
            Key::Char(ch) => {
                self.command_history_index = None;
                self.dismiss_command_completion_menu();
                self.command_input.push(ch);
                self.update_command_status();
            }
            Key::Paste(text) => {
                self.command_history_index = None;
                self.dismiss_command_completion_menu();
                for ch in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                    self.command_input.push(ch);
                }
                self.update_command_status();
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_terminal_module_command(
        &mut self,
        db: &Db,
        command_id: crate::editor_core::command_catalog::CommandId,
    ) -> bool {
        let current = crate::editor_core::engine::ModuleState {
            math: self.active_note.modules.math,
            table: self.active_note.modules.table,
            variables: self.active_note.modules.variables,
            style: self.active_note.modules.style,
        };
        let Some(plan) =
            crate::editor_core::engine::EditorEngine::plan_module_command(command_id, current)
        else {
            return false;
        };
        if !plan.changed {
            self.status = plan.message;
            return true;
        }

        let next_modules = NoteModules {
            math: plan.next.math,
            table: plan.next.table,
            variables: plan.next.variables,
            style: plan.next.style,
        };
        let capabilities = note_sources(db).capabilities_for_note_id(&self.active_note.id);
        if !capabilities.can_module_persist {
            self.status = "module updates are not supported for file-backed notes".to_string();
            return true;
        }

        let previous_modules = self.active_note.modules;
        match db.set_note_modules(&self.active_note.id, next_modules) {
            Ok(saved_note) => {
                self.active_note.modules = saved_note.modules;
                self.active_note.updated_at = saved_note.updated_at;
                let calc_module_changed = previous_modules.math != self.active_note.modules.math
                    || previous_modules.variables != self.active_note.modules.variables;
                if calc_module_changed {
                    self.calc_viewport_only = self.note_math_module_enabled()
                        && self.lines.len() >= CALC_VIEWPORT_ONLY_MIN_LINES
                        && self.active_has_variable_assignments()
                        && !self.calc.cached_has_builtin_formula;
                    if self.note_math_module_enabled() {
                        self.calc.stale = true;
                        if self.calc_viewport_only {
                            self.clear_calc_cache();
                            let editor_height = self.editor_height();
                            self.ensure_calc_for_viewport(editor_height, true);
                        } else {
                            self.run_calc_recompute();
                        }
                    } else {
                        self.clear_calc_cache();
                    }

                    if self.mode == UiMode::Editor
                        && self.note_math_module_enabled()
                        && self.note_variables_module_enabled()
                    {
                        self.refresh_variable_autocomplete_popup();
                    } else {
                        self.dismiss_variable_autocomplete_popup();
                    }
                }

                if previous_modules.table != self.active_note.modules.table {
                    self.adjust_cursor();
                    self.adjust_scroll();
                }
                self.status = plan.message;
            }
            Err(error) => {
                self.status = format!("module update failed: {error}");
            }
        }
        true
    }

    pub(super) fn execute_terminal_command(&mut self, db: &Db, cmd: &str) {
        if let Some(plan) =
            crate::editor_core::engine::EditorEngine::plan_host_command(self.command_mode(), cmd)
        {
            match plan {
                crate::editor_core::engine::HostCommandPlan::Write { quit, force } => {
                    match self.save_with_options(db, force) {
                        Ok(()) => {
                            self.status = "written".to_string();
                            if quit {
                                self.quit = true;
                            }
                        }
                        Err(error) => {
                            self.status = format!("write failed: {error}");
                        }
                    }
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::Quit { force } => {
                    self.force_quit = force;
                    self.quit = true;
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::NoteSecurity { action, password } => {
                    let action_label = action.as_str();
                    if password.trim().is_empty() {
                        self.status = format!("usage: note {action_label} <password>");
                        return;
                    }
                    let capabilities =
                        note_sources(db).capabilities_for_note_id(&self.active_note.id);
                    let supported = match action {
                        crate::editor_core::command_catalog::NoteSecurityAction::Lock
                        | crate::editor_core::command_catalog::NoteSecurityAction::Unlock => {
                            capabilities.can_lock
                        }
                        crate::editor_core::command_catalog::NoteSecurityAction::Encrypt
                        | crate::editor_core::command_catalog::NoteSecurityAction::Decrypt
                        | crate::editor_core::command_catalog::NoteSecurityAction::Unprotect => {
                            capabilities.can_encrypt
                        }
                    };
                    if !supported {
                        self.status =
                            "note security commands are not supported for file-backed notes"
                                .to_string();
                        return;
                    }
                    if self.autosave_enabled && self.dirty {
                        if let Err(error) = self.save(db) {
                            self.status = format!("save failed: {error}");
                            return;
                        }
                    }
                    let result = match action {
                        crate::editor_core::command_catalog::NoteSecurityAction::Lock => {
                            db.lock_note(&self.active_note.id, &password)
                        }
                        crate::editor_core::command_catalog::NoteSecurityAction::Unlock => {
                            db.unlock_note(&self.active_note.id, &password)
                        }
                        crate::editor_core::command_catalog::NoteSecurityAction::Encrypt => {
                            db.encrypt_note(&self.active_note.id, &password)
                        }
                        crate::editor_core::command_catalog::NoteSecurityAction::Decrypt => {
                            db.decrypt_note(&self.active_note.id, &password)
                        }
                        crate::editor_core::command_catalog::NoteSecurityAction::Unprotect => {
                            db.decrypt_note(&self.active_note.id, &password)
                        }
                    };
                    match result {
                        Ok(note) => {
                            if let Err(error) = self.set_active_note(db, note) {
                                self.status = format!("note {action_label} failed: {error}");
                                return;
                            }
                            self.status = match action {
                                crate::editor_core::command_catalog::NoteSecurityAction::Lock => {
                                    "note locked".to_string()
                                }
                                crate::editor_core::command_catalog::NoteSecurityAction::Unlock => {
                                    "note unlocked".to_string()
                                }
                                crate::editor_core::command_catalog::NoteSecurityAction::Encrypt => {
                                    "note encrypted at rest".to_string()
                                }
                                crate::editor_core::command_catalog::NoteSecurityAction::Decrypt => {
                                    "note decrypted".to_string()
                                }
                                crate::editor_core::command_catalog::NoteSecurityAction::Unprotect => {
                                    "note unprotected".to_string()
                                }
                            };
                        }
                        Err(error) => {
                            self.status = format!("note {action_label} failed: {error}");
                        }
                    }
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::Module { command_id } => {
                    let _ = self.handle_terminal_module_command(db, command_id);
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::Date => {
                    self.open_date_picker(DatePickerAction::InsertDate, false);
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::Notify => {
                    self.open_date_picker(DatePickerAction::SetNotify, true);
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::NotifyDelete => {
                    let line_number = (self.cursor_line + 1) as i64;
                    match db.delete_reminder(&self.active_note.id, line_number) {
                        Ok(true) => {
                            self.reminder_ghosts.remove(&self.cursor_line);
                            self.status =
                                format!("notify deleted on line {}", self.cursor_line + 1);
                        }
                        Ok(false) => {
                            self.status = format!(
                                "notify-delete: no reminder on line {}",
                                self.cursor_line + 1
                            );
                        }
                        Err(error) => {
                            self.status = format!("notify-delete failed: {error}");
                        }
                    }
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::ClipWatch { action } => {
                    match action {
                        crate::editor_core::engine::HostClipWatchAction::Start => {
                            if self.start_clipboard_watch() {
                                self.status = "clip-watch started".to_string();
                            } else {
                                self.status = "clip-watch already active".to_string();
                            }
                        }
                        crate::editor_core::engine::HostClipWatchAction::Stop => {
                            if self.stop_clipboard_watch() {
                                self.status = "clip-watch stopped".to_string();
                            } else {
                                self.status = "clip-watch not active".to_string();
                            }
                        }
                    }
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::Fold { action } => {
                    match action {
                        crate::editor_core::engine::HostFoldAction::Fold => {
                            self.set_fold_collapsed_at_cursor(true);
                        }
                        crate::editor_core::engine::HostFoldAction::Unfold => {
                            self.set_fold_collapsed_at_cursor(false);
                        }
                        crate::editor_core::engine::HostFoldAction::Toggle => {
                            self.toggle_fold_at_cursor();
                        }
                    }
                    return;
                }
            }
        }

        let snapshot = self.build_snapshot();
        let result =
            crate::editor_core::commands::execute_command(&snapshot, cmd, self.command_mode());

        if result.quit_requested {
            self.quit = true;
            return;
        }
        if !self.active_note_is_editable() && !result.operations.is_empty() {
            self.set_locked_note_status();
            return;
        }

        for op in &result.operations {
            self.apply_edit_operation(op);
        }

        self.status = if result.message.is_empty() {
            format!("editing {}", self.active_note.id)
        } else {
            result.message
        };
        self.adjust_cursor();
        self.adjust_scroll();
    }

    pub(super) fn byte_offset_for_line_col(&self, line_idx: usize, col: usize) -> usize {
        let mut offset = 0;
        for (i, line) in self.lines.iter().enumerate() {
            if i == line_idx {
                offset += byte_index(line, col);
                break;
            }
            offset += line.len() + 1; // +1 for \n
        }
        offset
    }

    pub(super) fn capture_visual_command_selection(
        &self,
    ) -> Option<crate::editor_core::types::SelectionSnapshot> {
        let anchor = self.selection_anchor?;
        match self.mode {
            UiMode::Visual => {
                let anchor_offset = self.byte_offset_for_line_col(anchor.0, anchor.1);
                let head_offset = self.byte_offset_for_line_col(self.cursor_line, self.cursor_col);
                Some(crate::editor_core::types::SelectionSnapshot {
                    anchor: anchor_offset,
                    head: head_offset,
                })
            }
            UiMode::VisualLine => {
                let anchor_line = anchor.0.min(self.lines.len().saturating_sub(1));
                let head_line = self.cursor_line.min(self.lines.len().saturating_sub(1));
                let anchor_line_len = line_char_len(&self.lines[anchor_line]);
                let head_line_len = line_char_len(&self.lines[head_line]);

                let (anchor_offset, head_offset) = if head_line >= anchor_line {
                    (
                        self.byte_offset_for_line_col(anchor_line, 0),
                        self.byte_offset_for_line_col(head_line, head_line_len),
                    )
                } else {
                    (
                        self.byte_offset_for_line_col(anchor_line, anchor_line_len),
                        self.byte_offset_for_line_col(head_line, 0),
                    )
                };

                Some(crate::editor_core::types::SelectionSnapshot {
                    anchor: anchor_offset,
                    head: head_offset,
                })
            }
            _ => None,
        }
    }

    pub(super) fn build_snapshot(&self) -> crate::editor_core::types::EditorContextSnapshot {
        let text = join_lines(&self.lines);
        let fallback_cursor = self.byte_offset_for_line_col(self.cursor_line, self.cursor_col);
        let selection =
            self.command_selection
                .unwrap_or(crate::editor_core::types::SelectionSnapshot {
                    anchor: fallback_cursor,
                    head: fallback_cursor,
                });
        crate::editor_core::types::EditorContextSnapshot {
            text,
            selection,
            changed_range: None,
        }
    }

    pub(super) fn build_context(&self) -> crate::editor_core::context::ResolvedContext<'static> {
        crate::editor_core::context::ResolvedContext::new(self.build_snapshot())
    }

    pub(super) fn open_switcher(&mut self, db: &Db) -> Result<(), String> {
        self.dismiss_variable_autocomplete_popup();
        if self.mode == UiMode::ContentSearch {
            self.clear_content_search_session();
        }
        self.refresh_switcher_items(db)?;
        self.mode = UiMode::Switcher;
        self.switcher_query.clear();
        self.recompute_switcher_matches();
        self.switcher_open_confirm = None;
        self.switcher_delete_confirm = None;
        self.status =
            "Switcher: type to filter, Enter open, Delete/Ctrl+Backspace delete, Esc close"
                .to_string();
        Ok(())
    }

    pub(super) fn close_switcher(&mut self) {
        self.dismiss_variable_autocomplete_popup();
        self.mode = UiMode::Editor;
        self.switcher_query.clear();
        self.switcher_matches.clear();
        self.switcher_selected = 0;
        self.switcher_open_confirm = None;
        self.switcher_delete_confirm = None;
        self.status = format!("editing {}", self.active_note.id);
    }

    pub(super) fn recompute_switcher_matches(&mut self) {
        let query = self.switcher_query.trim();
        if query.is_empty() {
            self.switcher_matches = (0..self.switcher_items.len()).collect();
            self.switcher_selected = 0;
            return;
        }

        let mut scored: Vec<(usize, i32)> = Vec::new();
        for (idx, item) in self.switcher_items.iter().enumerate() {
            if let Some(score) = switcher::fuzzy_score(query, &item.title) {
                scored.push((idx, score));
            }
        }
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        self.switcher_matches = scored.into_iter().map(|(idx, _)| idx).collect();
        self.switcher_selected = 0;
    }

    pub(super) fn save(&mut self, db: &Db) -> Result<(), String> {
        self.save_with_options(db, false)
    }

    pub(super) fn save_with_options(&mut self, db: &Db, force: bool) -> Result<(), String> {
        if self.format_on_save {
            self.execute_terminal_command(db, "format");
        }
        if !self.dirty {
            return Ok(());
        }
        self.sync_reminder_ghosts_if_dirty(db)?;
        let body = join_lines(&self.lines);
        let mut saved = note_sources(db).save_note_by_id(
            &self.active_note.id,
            &body,
            app_core::note_sources::SaveOptions {
                expected_revision: Some(self.active_note.updated_at.clone()),
                force,
            },
        )?;
        // The returned body duplicates what we already hold in `self.lines`;
        // drop it to keep memory usage flat.
        saved.body = String::new();
        self.active_note = saved;
        self.dirty = false;
        self.history
            .checkpoint(&self.lines, self.cursor_line, self.cursor_col);
        self.refresh_switcher_items(db)?;
        Ok(())
    }

    pub(super) fn sync_reminder_ghosts_if_dirty(&mut self, db: &Db) -> Result<(), String> {
        if !self.reminders_dirty {
            return Ok(());
        }
        self.reminder_ghosts = load_note_reminder_ghosts(db, &self.active_note.id, &self.lines)?;
        self.reminders_dirty = false;
        Ok(())
    }

    pub(super) fn maybe_dispatch_due_reminders(&mut self, db: &Db) {
        if self.last_reminder_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_reminder_check = Instant::now();

        if self.reminder_ghosts.is_empty() {
            return;
        }

        let now_ms = notifications::now_epoch_ms();
        let mut due_lines = self
            .reminder_ghosts
            .iter()
            .filter_map(|(line_idx, reminder)| {
                if reminder.notified_at_ms.is_none() && reminder.remind_at_ms <= now_ms {
                    Some(*line_idx)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        due_lines.sort_unstable();

        for line_idx in due_lines {
            let Some(reminder) = self.reminder_ghosts.get(&line_idx).cloned() else {
                continue;
            };
            let body = self
                .lines
                .get(line_idx)
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .map(|line| line.to_string())
                .or_else(|| {
                    let trimmed = reminder.line_text.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                })
                .unwrap_or_else(|| "Reminder".to_string());

            if let Err(error) = notifications::send_system_notification("Note reminder", &body) {
                eprintln!("Reminder notification failed: {error}");
                continue;
            }

            let line_number = i64::try_from(line_idx + 1).unwrap_or(i64::MAX);
            match db.mark_reminder_notified(&self.active_note.id, line_number, now_ms) {
                Ok(updated) => {
                    let notified_at = updated
                        .and_then(|entry| entry.notified_at_ms)
                        .unwrap_or(now_ms);
                    if let Some(entry) = self.reminder_ghosts.get_mut(&line_idx) {
                        entry.notified_at_ms = Some(notified_at);
                        entry.line_text = self
                            .lines
                            .get(line_idx)
                            .cloned()
                            .unwrap_or_else(|| entry.line_text.clone());
                    }
                }
                Err(error) => {
                    eprintln!("Failed to persist reminder notification: {error}");
                }
            }
        }
    }

    pub(super) fn open_content_search(&mut self, db: &Db) -> Result<(), String> {
        self.dismiss_variable_autocomplete_popup();
        // Pre-load switcher items so title fallback can mirror UI behavior.
        if self.switcher_items.is_empty() {
            self.refresh_switcher_items(db)?;
        }
        self.mode = UiMode::ContentSearch;
        self.clear_content_search_session();
        self.content_search_results = self.content_search_title_fallback_results("");
        self.switcher_open_confirm = None;
        self.switcher_delete_confirm = None;
        self.status =
            "Content search: type to search, Enter open, Tab title search, Esc close".to_string();
        Ok(())
    }

    pub(super) fn close_content_search(&mut self) {
        self.dismiss_variable_autocomplete_popup();
        self.mode = UiMode::Editor;
        self.clear_content_search_session();
        self.status = format!("editing {}", self.active_note.id);
    }

    pub(super) fn handle_content_search_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        match key {
            Key::Esc | Key::Ctrl('p') => {
                self.close_content_search();
            }
            Key::Ctrl('q') => {
                self.quit = true;
            }
            Key::Tab => {
                self.open_switcher(db)?;
            }
            Key::Ctrl('w') | Key::CtrlBackspace => {
                trim_trailing_word(&mut self.content_search_query);
                self.refresh_content_search_preview();
            }
            Key::ArrowUp => {
                if self.content_search_selected > 0 {
                    self.content_search_selected -= 1;
                }
            }
            Key::ArrowDown => {
                if self.content_search_selected + 1 < self.content_search_results.len() {
                    self.content_search_selected += 1;
                }
            }
            Key::Backspace => {
                self.content_search_query.pop();
                self.refresh_content_search_preview();
            }
            Key::Enter => {
                if let Some(result) = self
                    .content_search_results
                    .get(self.content_search_selected)
                    .cloned()
                {
                    // Check if the note is protected
                    let access_mode = self
                        .switcher_items
                        .iter()
                        .find(|n| n.id == result.id)
                        .map(|n| n.access_mode)
                        .unwrap_or(NoteAccessMode::None);
                    let is_unlocked = self
                        .switcher_items
                        .iter()
                        .find(|n| n.id == result.id)
                        .map(|n| n.is_unlocked)
                        .unwrap_or(false);
                    if access_mode != NoteAccessMode::None && !is_unlocked {
                        self.switcher_open_confirm = Some(SwitcherOpenConfirm {
                            note_id: result.id.clone(),
                            note_title: result.title.clone(),
                            password: String::new(),
                            line_number: Some(result.line_number),
                        });
                        self.mode = UiMode::Switcher;
                        self.recompute_switcher_matches();
                    } else {
                        self.open_note_from_switcher(
                            db,
                            &result.id.clone(),
                            None,
                            Some(result.line_number),
                        )?;
                    }
                }
            }
            Key::Char(ch) => {
                self.content_search_query.push(ch);
                self.refresh_content_search_preview();
            }
            Key::Paste(text) => {
                for ch in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                    self.content_search_query.push(ch);
                }
                self.refresh_content_search_preview();
            }
            _ => {}
        }
        self.maybe_dispatch_content_search(db);
        Ok(())
    }

    pub(super) fn refresh_switcher_items(&mut self, db: &Db) -> Result<(), String> {
        let previous_prefix_index = self.wiki_link_prefix_index.clone();
        self.switcher_items = switcher::load_note_meta(db, Some(&self.active_note.id))?;
        self.rebuild_wiki_link_prefix_index();
        let mut changed_short_ids: Vec<String> = Vec::new();
        for (short_id, next) in self.wiki_link_prefix_index.iter() {
            let changed = match previous_prefix_index.get(short_id) {
                Some(prev) => prev.note_id != next.note_id || prev.updated_at != next.updated_at,
                None => true,
            };
            if changed {
                changed_short_ids.push(short_id.clone());
            }
        }
        for short_id in previous_prefix_index.keys() {
            if !self.wiki_link_prefix_index.contains_key(short_id) {
                changed_short_ids.push(short_id.clone());
            }
        }
        changed_short_ids.sort();
        changed_short_ids.dedup();
        for short_id in changed_short_ids {
            self.invalidate_wiki_link_render_cache_for_short_id(&short_id);
        }
        if self.mode == UiMode::Switcher {
            self.recompute_switcher_matches();
        }
        Ok(())
    }

    pub(super) fn set_active_note(&mut self, db: &Db, note: Note) -> Result<(), String> {
        self.active_note = note;
        self.lines = split_lines(&self.active_note.body);
        self.active_note.body = String::new();
        self.dismiss_variable_autocomplete_popup();
        self.reminder_ghosts = load_note_reminder_ghosts(db, &self.active_note.id, &self.lines)?;
        self.reminders_dirty = false;
        self.last_reminder_check = Instant::now();
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.scroll_line = 0;
        self.scroll_col = 0;
        self.dirty = false;
        self.last_edit = Instant::now();
        self.search_query.clear();
        self.search_matches.clear();
        self.rebuild_wiki_link_prefix_index();
        self.wiki_link_render_cache.clear();
        self.wiki_link_line_render_cache.clear();
        self.history
            .reset(&self.lines, self.cursor_line, self.cursor_col);
        self.fence_checkpoints.truncate(1);
        self.fence_checkpoints_valid_through = 0;
        self.rescan_calc_flags();
        self.calc_viewport_only = self.lines.len() >= CALC_VIEWPORT_ONLY_MIN_LINES
            && self.active_has_variable_assignments()
            && !self.calc.cached_has_builtin_formula;
        self.calc_last_view_eval_range = None;
        if self.calc_viewport_only
            || (!self.calc.cached_has_builtin_formula && !self.active_has_variable_assignments())
        {
            self.calc.results = vec![None; self.lines.len()];
            self.calc.cell_results = vec![Vec::new(); self.lines.len()];
            self.calc.variable_names.clear();
            self.calc.line_metadata.clear();
            self.calc.prev_line_metadata.clear();
            self.calc.stale = false;
            self.calc_recompute_pending = false;
        } else if self.should_defer_calc_recompute() {
            self.calc.results = vec![None; self.lines.len()];
            self.calc.cell_results = vec![Vec::new(); self.lines.len()];
            self.calc.variable_names.clear();
            self.calc.line_metadata.clear();
            self.calc.prev_line_metadata.clear();
            self.calc.stale = true;
            self.calc_recompute_pending = false;
        } else {
            self.run_calc_recompute();
        }
        self.recompute_folding();
        self.adjust_cursor();
        self.adjust_scroll();
        if self.calc_viewport_only {
            let editor_height = self.editor_height();
            self.ensure_calc_for_viewport(editor_height, true);
        }
        self.history
            .checkpoint(&self.lines, self.cursor_line, self.cursor_col);
        Ok(())
    }

    pub(super) fn open_search(&mut self) {
        self.dismiss_variable_autocomplete_popup();
        self.search_query.clear();
        self.search_matches.clear();
        self.search_current = 0;
        self.search_orig_line = self.cursor_line;
        self.search_orig_col = self.cursor_col;
        self.search_orig_scroll = self.scroll_line;
        self.mode = UiMode::Search;
        self.status = "/".to_string();
    }

    pub(super) fn handle_search_key(&mut self, key: Key) -> Result<(), String> {
        match key {
            Key::Esc => {
                self.cursor_line = self.search_orig_line;
                self.cursor_col = self.search_orig_col;
                self.scroll_line = self.search_orig_scroll;
                self.adjust_cursor();
                self.scroll_line = self
                    .scroll_line
                    .min(self.visible_line_count().saturating_sub(1));
                self.mode = UiMode::Normal;
                self.search_query.clear();
                self.search_matches.clear();
                self.status = "-- NORMAL --".to_string();
            }
            Key::Enter => {
                self.mode = UiMode::Normal;
                self.status = if self.search_matches.is_empty() {
                    "no matches".to_string()
                } else {
                    format!(
                        "/{} ({}/{})",
                        self.search_query,
                        self.search_current + 1,
                        self.search_matches.len()
                    )
                };
            }
            Key::ArrowDown | Key::Ctrl('n') | Key::Tab => {
                self.search_next();
            }
            Key::ArrowUp | Key::Ctrl('p') | Key::BackTab => {
                self.search_prev();
            }
            Key::Backspace => {
                self.search_query.pop();
                self.recompute_search();
            }
            Key::Ctrl('w') => {
                trim_trailing_word(&mut self.search_query);
                self.recompute_search();
            }
            Key::Char(ch) => {
                self.search_query.push(ch);
                self.recompute_search();
            }
            Key::Paste(text) => {
                for ch in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                    self.search_query.push(ch);
                }
                self.recompute_search();
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn recompute_search(&mut self) {
        self.search_matches.clear();
        self.search_current = 0;

        let query = self.search_query.to_lowercase();
        if query.is_empty() {
            self.status = "/".to_string();
            return;
        }

        let query_chars = query.chars().count();
        for (line_idx, line) in self.lines.iter().enumerate() {
            let lower = line.to_lowercase();
            let mut byte_start = 0;
            while let Some(pos) = lower[byte_start..].find(&query) {
                let abs_byte = byte_start + pos;
                let char_start = line[..abs_byte].chars().count();
                self.search_matches
                    .push((line_idx, char_start, char_start + query_chars));
                byte_start = abs_byte + query.len();
            }
        }

        if !self.search_matches.is_empty() {
            self.jump_to_nearest_match();
        }
        self.update_search_status();
    }

    pub(super) fn update_search_status(&mut self) {
        if self.search_matches.is_empty() {
            self.status = format!("/{} (no matches)", self.search_query);
        } else {
            self.status = format!(
                "/{} ({}/{})",
                self.search_query,
                self.search_current + 1,
                self.search_matches.len()
            );
        }
    }

    pub(super) fn jump_to_nearest_match(&mut self) {
        for (i, &(line, _, _)) in self.search_matches.iter().enumerate() {
            if line >= self.search_orig_line {
                self.search_current = i;
                self.jump_to_current_match();
                return;
            }
        }
        self.search_current = 0;
        self.jump_to_current_match();
    }

    pub(super) fn jump_to_current_match(&mut self) {
        if let Some(&(line, col, _)) = self.search_matches.get(self.search_current) {
            self.cursor_line = line;
            self.cursor_col = col;
            self.adjust_cursor();
            self.adjust_scroll();
        }
    }

    pub(super) fn search_next(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_current = (self.search_current + 1) % self.search_matches.len();
        self.jump_to_current_match();
        self.update_search_status();
    }

    pub(super) fn search_prev(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_current =
            (self.search_current + self.search_matches.len() - 1) % self.search_matches.len();
        self.jump_to_current_match();
        self.update_search_status();
    }
}
