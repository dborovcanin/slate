use super::{
    line_char_len, load_note_reminder_ghosts, new_note_with_context, trim_trailing_word,
    CollectionEditDialogState, CommandCompletionMenuState, CommandCompletionOption,
    ContentSearchResponse, DatePickerAction, Db, Key, Note, NoteSearchResult,
    SwitcherDeleteConfirm, SwitcherOpenConfirm, TerminalApp, UiMode, WebSearchResponse,
    WebSearchState, CALC_VIEWPORT_ONLY_MIN_LINES, COMMAND_COMPLETION_MAX_OPTIONS,
    CONTENT_SEARCH_DEBOUNCE_MS, CONTENT_SEARCH_MAX_DETACHED_WORKERS, MAX_COMMAND_HISTORY_ENTRIES,
};
use crate::terminal::text_utils::{byte_index, join_lines, split_lines};
use crate::terminal::{notifications, switcher, text_input};
use app_core::storage::{NoteAccessMode, NoteModules};
use std::time::{Duration, Instant};

/// Char ranges in `line` of non-overlapping matches of `query_lower` (an
/// already lowercased query), compared case-insensitively.
pub(super) fn case_insensitive_matches(line: &str, query_lower: &str) -> Vec<(usize, usize)> {
    let mut matches = Vec::new();
    if query_lower.is_empty() {
        return matches;
    }
    if line.is_ascii() && query_lower.is_ascii() {
        // ASCII lowercasing keeps byte offsets, which are char offsets here.
        let lower = line.to_ascii_lowercase();
        let mut start = 0;
        while let Some(pos) = lower[start..].find(query_lower) {
            let from = start + pos;
            matches.push((from, from + query_lower.len()));
            start = from + query_lower.len();
        }
        return matches;
    }
    // Lowercasing can change a char's length (`İ` becomes two chars), so
    // keep, for every lowered char, the index of the char it came from.
    let query: Vec<char> = query_lower.chars().collect();
    let lowered: Vec<(char, usize)> = line
        .chars()
        .enumerate()
        .flat_map(|(idx, ch)| ch.to_lowercase().map(move |lower| (lower, idx)))
        .collect();
    let mut start = 0;
    while start + query.len() <= lowered.len() {
        let window = &lowered[start..start + query.len()];
        if window.iter().map(|(ch, _)| *ch).eq(query.iter().copied()) {
            matches.push((window[0].1, window[query.len() - 1].1 + 1));
            start += query.len();
        } else {
            start += 1;
        }
    }
    matches
}

fn note_sources(db: &Db) -> app_core::note_sources::NoteSourceService {
    app_core::note_sources::NoteSourceService::new(db.clone())
}

const TERMINAL_PERF_COMMAND_SUGGESTIONS: [(&str, &str); 8] = [
    ("perf status", "show terminal perf tracing status"),
    ("perf where", "show terminal perf log location"),
    ("perf dump", "show terminal perf dump hint"),
    (
        "perf on",
        "alias for perf status (runtime tracing always on-demand)",
    ),
    (
        "perf off",
        "alias for perf status (runtime tracing always on-demand)",
    ),
    ("perf clear", "alias for perf status"),
    ("perf toggle", "toggle terminal perf tracing"),
    ("perf cap", "set sample cap per bucket"),
];

fn list_terminal_perf_command_suggestions(
    raw_input: &str,
) -> Vec<crate::editor_core::types::CommandSuggestion> {
    let normalized = crate::editor_core::command_catalog::normalize_command(raw_input);
    if normalized.is_empty() {
        return Vec::new();
    }
    if !normalized.starts_with("perf")
        && !normalized.starts_with("profile")
        && !normalized.starts_with("profiler")
    {
        return Vec::new();
    }
    TERMINAL_PERF_COMMAND_SUGGESTIONS
        .iter()
        .filter(|(value, _)| value.starts_with(&normalized))
        .map(
            |(value, description)| crate::editor_core::types::CommandSuggestion {
                value: (*value).to_string(),
                description: (*description).to_string(),
            },
        )
        .collect()
}

fn list_terminal_command_suggestions(
    mode: crate::editor_core::types::CommandMode,
    raw_input: &str,
) -> Vec<crate::editor_core::types::CommandSuggestion> {
    let mut core = crate::editor_core::commands::list_command_suggestions(mode, raw_input);
    let perf = list_terminal_perf_command_suggestions(raw_input);
    if perf.is_empty() {
        return core;
    }
    let seen = core
        .iter()
        .map(|entry| entry.value.clone())
        .collect::<rustc_hash::FxHashSet<_>>();
    core.extend(
        perf.into_iter()
            .filter(|entry| !seen.contains(&entry.value)),
    );
    core
}

fn parse_collection_default_tags_input(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::<String>::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = trimmed.to_lowercase();
        if seen.insert(normalized) {
            out.push(trimmed.to_string());
        }
    }
    out
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

fn insert_str_at_char_col(target: &mut String, char_col: usize, insert: &str) {
    let byte_col = byte_index(target, char_col);
    target.insert_str(byte_col, insert);
}

fn remove_char_before_char_col(target: &mut String, char_col: usize) -> bool {
    if char_col == 0 {
        return false;
    }
    let from = byte_index(target, char_col - 1);
    let to = byte_index(target, char_col);
    target.replace_range(from..to, "");
    true
}

fn remove_char_at_char_col(target: &mut String, char_col: usize) -> bool {
    if char_col >= char_len(target) {
        return false;
    }
    let from = byte_index(target, char_col);
    let to = byte_index(target, char_col + 1);
    target.replace_range(from..to, "");
    true
}

// Ownership: switcher, command bar execution, and search workflows.
impl TerminalApp {
    pub(super) fn rebuild_wiki_link_prefix_index(&mut self) {
        self.wiki_link_prefix_index.clear();
        for note in &self.switcher.items {
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

        let mut ordered = Vec::with_capacity(self.switcher.items.len());
        if let Some(active_idx) = self
            .switcher
            .items
            .iter()
            .position(|note| note.id == self.active_note.id)
        {
            ordered.push((0usize, &self.switcher.items[active_idx]));
        }
        for (idx, note) in self.switcher.items.iter().enumerate() {
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
        if let Some(rx) = self.content_search.rx.take() {
            self.push_detached_content_search_rx(rx);
        }
        self.content_search.query.clear();
        self.content_search.cursor_col = 0;
        self.content_search.results.clear();
        self.content_search.selected = 0;
        self.content_search.pending = false;
        self.content_search.debounce_until = None;
    }

    fn push_detached_content_search_rx(
        &mut self,
        rx: std::sync::mpsc::Receiver<ContentSearchResponse>,
    ) {
        if self.content_search.detached_rxs.len() >= CONTENT_SEARCH_MAX_DETACHED_WORKERS {
            self.content_search.detached_rxs.remove(0);
        }
        self.content_search.detached_rxs.push(rx);
    }

    fn refresh_content_search_preview(&mut self) {
        self.content_search.cursor_col = self
            .content_search
            .cursor_col
            .min(char_len(&self.content_search.query));
        let query = self.content_search.query.trim().to_string();
        // Supersede any stale in-flight worker immediately when the query/filter
        // changes so the next debounced dispatch is never blocked.
        if let Some(rx) = self.content_search.rx.take() {
            self.push_detached_content_search_rx(rx);
        }
        self.content_search.results = self.content_search_title_fallback_results(&query);
        self.content_search.selected = 0;
        self.content_search.pending = !query.is_empty();
        self.content_search.debounce_until = if query.is_empty() {
            None
        } else {
            Some(Instant::now() + Duration::from_millis(CONTENT_SEARCH_DEBOUNCE_MS))
        };
    }

    fn reset_content_search_filter_to_working_collection(&mut self) {
        self.content_search.collection_filter_id = self.working_collection_id.clone();
        self.content_search.collection_filter_name = self.working_collection_name.clone();
    }

    fn toggle_content_search_collection_filter(&mut self) {
        let Some(working_id) = self.working_collection_id.clone() else {
            return;
        };
        let use_working =
            self.content_search.collection_filter_id.as_deref() != Some(working_id.as_str());
        if use_working {
            self.content_search.collection_filter_id = Some(working_id);
            self.content_search.collection_filter_name = self.working_collection_name.clone();
        } else {
            self.content_search.collection_filter_id = None;
            self.content_search.collection_filter_name = None;
        }
    }

    fn reset_switcher_filter_to_working_collection(&mut self) {
        self.switcher.collection_filter_id = self.working_collection_id.clone();
        self.switcher.collection_filter_name = self.working_collection_name.clone();
    }

    fn toggle_switcher_collection_filter(&mut self) {
        let Some(working_id) = self.working_collection_id.clone() else {
            return;
        };
        let use_working =
            self.switcher.collection_filter_id.as_deref() != Some(working_id.as_str());
        if use_working {
            self.switcher.collection_filter_id = Some(working_id);
            self.switcher.collection_filter_name = self.working_collection_name.clone();
        } else {
            self.switcher.collection_filter_id = None;
            self.switcher.collection_filter_name = None;
        }
    }

    fn switcher_collection_filter_label(&self) -> &str {
        self.switcher
            .collection_filter_name
            .as_deref()
            .unwrap_or("All")
    }

    fn content_search_collection_filter_label(&self) -> &str {
        self.content_search
            .collection_filter_name
            .as_deref()
            .unwrap_or("All")
    }

    fn update_switcher_status_hint(&mut self) {
        self.status = format!(
            "Switcher: type to filter, Enter open, Tab content search, Ctrl+G collections, Ctrl+L toggle collection ({}), Delete/Ctrl+Backspace delete, Esc close",
            self.switcher_collection_filter_label()
        );
    }

    fn update_content_search_status_hint(&mut self) {
        self.status = format!(
            "Content search: type to search, Ctrl+L toggle collection ({}), Enter open, Tab title search, Esc close",
            self.content_search_collection_filter_label()
        );
    }

    fn refresh_switcher_items_for_filter(&mut self, db: &Db) -> Result<(), String> {
        self.refresh_switcher_items(db)?;
        if self.mode == UiMode::Switcher {
            self.update_switcher_status_hint();
        }
        if self.mode == UiMode::ContentSearch {
            self.update_content_search_status_hint();
        }
        Ok(())
    }

    fn toggle_switcher_collection_filter_and_refresh(&mut self, db: &Db) -> Result<(), String> {
        self.toggle_switcher_collection_filter();
        self.refresh_switcher_items_for_filter(db)?;
        Ok(())
    }

    fn toggle_content_search_collection_filter_and_refresh(
        &mut self,
        db: &Db,
    ) -> Result<(), String> {
        self.toggle_content_search_collection_filter();
        self.refresh_switcher_items_for_filter(db)?;
        self.refresh_content_search_preview();
        Ok(())
    }

    pub(super) fn handle_switcher_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        if self.switcher.open_confirm.is_some() {
            return self.handle_switcher_open_confirm_key(db, key);
        }
        if self.switcher.delete_confirm.is_some() {
            return self.handle_switcher_delete_confirm_key(db, key);
        }

        match key {
            Key::Esc | Key::Ctrl('p') => {
                self.close_switcher();
            }
            Key::Ctrl('q') => {
                self.quit = true;
            }
            Key::Ctrl('l') => {
                if self.working_collection_id.is_some() {
                    self.toggle_switcher_collection_filter_and_refresh(db)?;
                }
            }
            Key::Ctrl('w') => {
                trim_trailing_word(&mut self.switcher.query);
                self.recompute_switcher_matches();
            }
            Key::Ctrl('n') => {
                self.close_switcher();
                self.handle_editor_key(db, Key::Ctrl('n'))?;
            }
            Key::ArrowUp => {
                if self.switcher.selected > 0 {
                    self.switcher.selected -= 1;
                }
            }
            Key::ArrowDown => {
                if self.switcher.selected + 1 < self.switcher.matches.len() {
                    self.switcher.selected += 1;
                }
            }
            Key::Backspace => {
                self.switcher.query.pop();
                self.recompute_switcher_matches();
            }
            Key::Delete | Key::CtrlBackspace => {
                self.request_switcher_delete_confirmation(db);
            }
            Key::Enter => {
                if let Some(idx) = self.switcher.matches.get(self.switcher.selected).copied() {
                    let item = self.switcher.items[idx].clone();
                    if item.access_mode != NoteAccessMode::None && !item.is_unlocked {
                        self.switcher.open_confirm = Some(SwitcherOpenConfirm {
                            note_id: item.id,
                            note_title: item.title,
                            access_mode: item.access_mode,
                            password: String::new(),
                            line_number: None,
                        });
                    } else {
                        self.open_note_from_switcher(db, item.id.as_str(), None, None)?;
                    }
                }
            }
            Key::Char(ch) => {
                self.switcher.query.push(ch);
                self.recompute_switcher_matches();
            }
            Key::Paste(text) => {
                for ch in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                    self.switcher.query.push(ch);
                }
                self.recompute_switcher_matches();
            }
            Key::Tab => {
                self.open_content_search(db)?;
            }
            Key::Ctrl('g') => {
                self.open_collection_switcher(db)?;
            }
            Key::CtrlDelete
            | Key::ShiftEnter
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

    pub(super) fn handle_collection_switcher_key(
        &mut self,
        db: &Db,
        key: Key,
    ) -> Result<(), String> {
        if self.collection_switcher.edit_dialog.is_some() {
            match key {
                Key::Esc => {
                    self.collection_switcher.edit_dialog = None;
                    self.status = "collection update canceled".to_string();
                }
                Key::Enter => {
                    self.save_collection_edit_dialog(db)?;
                }
                Key::Tab => {
                    if let Some(dialog) = self.collection_switcher.edit_dialog.as_mut() {
                        dialog.selected_field = (dialog.selected_field + 1) % 3;
                    }
                }
                Key::BackTab => {
                    if let Some(dialog) = self.collection_switcher.edit_dialog.as_mut() {
                        dialog.selected_field = (dialog.selected_field + 2) % 3;
                    }
                }
                Key::Ctrl('w') | Key::CtrlBackspace => {
                    if let Some(dialog) = self.collection_switcher.edit_dialog.as_mut() {
                        match dialog.selected_field {
                            0 => trim_trailing_word(&mut dialog.name),
                            1 => trim_trailing_word(&mut dialog.description),
                            _ => trim_trailing_word(&mut dialog.default_tags),
                        }
                    }
                }
                Key::Backspace => {
                    if let Some(dialog) = self.collection_switcher.edit_dialog.as_mut() {
                        match dialog.selected_field {
                            0 => {
                                dialog.name.pop();
                            }
                            1 => {
                                dialog.description.pop();
                            }
                            _ => {
                                dialog.default_tags.pop();
                            }
                        }
                    }
                }
                Key::Char(ch) => {
                    if let Some(dialog) = self.collection_switcher.edit_dialog.as_mut() {
                        match dialog.selected_field {
                            0 => dialog.name.push(ch),
                            1 => dialog.description.push(ch),
                            _ => dialog.default_tags.push(ch),
                        }
                    }
                }
                Key::Paste(text) => {
                    if let Some(dialog) = self.collection_switcher.edit_dialog.as_mut() {
                        for ch in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                            match dialog.selected_field {
                                0 => dialog.name.push(ch),
                                1 => dialog.description.push(ch),
                                _ => dialog.default_tags.push(ch),
                            }
                        }
                    }
                }
                Key::Ctrl('q') => {
                    self.quit = true;
                }
                _ => {}
            }
            return Ok(());
        }

        match key {
            Key::Esc | Key::Ctrl('p') | Key::Ctrl('g') => {
                self.close_collection_switcher();
            }
            Key::Ctrl('q') => {
                self.quit = true;
            }
            Key::Ctrl('w') => {
                trim_trailing_word(&mut self.collection_switcher.query);
                self.recompute_collection_switcher_matches();
            }
            Key::ArrowUp => {
                if self.collection_switcher.selected > 0 {
                    self.collection_switcher.selected -= 1;
                }
            }
            Key::ArrowDown => {
                if self.collection_switcher.selected + 1 < self.collection_switcher.matches.len() {
                    self.collection_switcher.selected += 1;
                }
            }
            Key::Backspace => {
                self.collection_switcher.query.pop();
                self.recompute_collection_switcher_matches();
            }
            Key::Enter => {
                if let Some(idx) = self
                    .collection_switcher
                    .matches
                    .get(self.collection_switcher.selected)
                    .copied()
                {
                    if let Some(item) = self.collection_switcher.items.get(idx) {
                        let is_clear = item.is_clear || item.id.is_none();
                        let selected_name = item.name.clone();
                        let selected_id = item.id.clone();
                        if is_clear {
                            self.working_collection_id = None;
                            self.working_collection_name = None;
                            self.refresh_switcher_items(db)?;
                            self.close_collection_switcher();
                            self.status = "working collection cleared".to_string();
                        } else if let Some(collection_id) = selected_id {
                            self.working_collection_id = Some(collection_id);
                            self.working_collection_name = Some(selected_name.clone());
                            self.refresh_switcher_items(db)?;
                            self.close_collection_switcher();
                            self.status = format!("working collection: {}", selected_name);
                        }
                    }
                }
            }
            Key::Ctrl('e') => {
                self.open_collection_edit_dialog_for_selected(db)?;
            }
            Key::Char(ch) => {
                self.collection_switcher.query.push(ch);
                self.recompute_collection_switcher_matches();
            }
            Key::Paste(text) => {
                for ch in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                    self.collection_switcher.query.push(ch);
                }
                self.recompute_collection_switcher_matches();
            }
            _ => {}
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
                self.switcher.delete_confirm = None;
            }
            Key::Enter => {
                if let Some(confirm) = self.switcher.delete_confirm.clone() {
                    if confirm.requires_password && confirm.password.trim().is_empty() {
                        self.status = format!(
                            "password required to delete {}",
                            Self::access_mode_prompt_label(confirm.access_mode)
                        );
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
                            self.switcher.delete_confirm = None;
                        }
                        Err(error) => {
                            self.status = format!("delete failed: {error}");
                            if let Some(current) = self.switcher.delete_confirm.as_mut() {
                                current.password.clear();
                            }
                        }
                    }
                }
            }
            Key::Char('y') => {
                let requires_password = self
                    .switcher
                    .delete_confirm
                    .as_ref()
                    .map(|confirm| confirm.requires_password)
                    .unwrap_or(false);
                if !requires_password {
                    if let Some(confirm) = self.switcher.delete_confirm.clone() {
                        match self.delete_note_from_switcher(
                            db,
                            &confirm.note_id,
                            &confirm.note_title,
                            None,
                        ) {
                            Ok(()) => {
                                self.switcher.delete_confirm = None;
                            }
                            Err(error) => {
                                self.status = format!("delete failed: {error}");
                            }
                        }
                    }
                } else if let Some(confirm) = self.switcher.delete_confirm.as_mut() {
                    confirm.password.push('y');
                }
            }
            Key::Char('Y') => {
                let requires_password = self
                    .switcher
                    .delete_confirm
                    .as_ref()
                    .map(|confirm| confirm.requires_password)
                    .unwrap_or(false);
                if !requires_password {
                    if let Some(confirm) = self.switcher.delete_confirm.clone() {
                        match self.delete_note_from_switcher(
                            db,
                            &confirm.note_id,
                            &confirm.note_title,
                            None,
                        ) {
                            Ok(()) => {
                                self.switcher.delete_confirm = None;
                            }
                            Err(error) => {
                                self.status = format!("delete failed: {error}");
                            }
                        }
                    }
                } else if let Some(confirm) = self.switcher.delete_confirm.as_mut() {
                    confirm.password.push('Y');
                }
            }
            Key::Char('n') => {
                let requires_password = self
                    .switcher
                    .delete_confirm
                    .as_ref()
                    .map(|confirm| confirm.requires_password)
                    .unwrap_or(false);
                if !requires_password {
                    self.switcher.delete_confirm = None;
                } else if let Some(confirm) = self.switcher.delete_confirm.as_mut() {
                    confirm.password.push('n');
                }
            }
            Key::Char('N') => {
                let requires_password = self
                    .switcher
                    .delete_confirm
                    .as_ref()
                    .map(|confirm| confirm.requires_password)
                    .unwrap_or(false);
                if !requires_password {
                    self.switcher.delete_confirm = None;
                } else if let Some(confirm) = self.switcher.delete_confirm.as_mut() {
                    confirm.password.push('N');
                }
            }
            Key::Backspace | Key::CtrlBackspace => {
                if let Some(confirm) = self.switcher.delete_confirm.as_mut() {
                    if confirm.requires_password {
                        confirm.password.pop();
                    }
                }
            }
            Key::Paste(text) => {
                if let Some(confirm) = self.switcher.delete_confirm.as_mut() {
                    if confirm.requires_password {
                        for ch in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                            confirm.password.push(ch);
                        }
                    }
                }
            }
            Key::Char(ch) => {
                if let Some(confirm) = self.switcher.delete_confirm.as_mut() {
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
                self.switcher.open_confirm = None;
            }
            Key::Enter => {
                if let Some(confirm) = self.switcher.open_confirm.clone() {
                    if confirm.password.trim().is_empty() {
                        self.status = format!(
                            "password required to open {}",
                            Self::access_mode_prompt_label(confirm.access_mode)
                        );
                        return Ok(());
                    }
                    match self.open_note_from_switcher(
                        db,
                        &confirm.note_id,
                        Some(confirm.password.as_str()),
                        confirm.line_number,
                    ) {
                        Ok(()) => {
                            self.switcher.open_confirm = None;
                        }
                        Err(error) => {
                            self.status = format!("open failed: {error}");
                            if let Some(current) = self.switcher.open_confirm.as_mut() {
                                current.password.clear();
                            }
                        }
                    }
                }
            }
            Key::Backspace | Key::CtrlBackspace => {
                if let Some(confirm) = self.switcher.open_confirm.as_mut() {
                    confirm.password.pop();
                }
            }
            Key::Paste(text) => {
                if let Some(confirm) = self.switcher.open_confirm.as_mut() {
                    for ch in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                        confirm.password.push(ch);
                    }
                }
            }
            Key::Char(ch) => {
                if let Some(confirm) = self.switcher.open_confirm.as_mut() {
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
            let max_line = self.editor.lines.len().saturating_sub(1);
            self.editor.cursor_line = line_number.saturating_sub(1).min(max_line);
            self.editor.cursor_col = 0;
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
        self.editor.selection_anchor = None;
        self.command_selection = None;
        self.status = "-- NORMAL --".to_string();
        Ok(())
    }

    pub(super) fn request_switcher_delete_confirmation(&mut self, db: &Db) {
        if let Some(idx) = self.switcher.matches.get(self.switcher.selected).copied() {
            let item = &self.switcher.items[idx];
            let capabilities = note_sources(db).capabilities_for_note_id(&item.id);
            if !capabilities.can_delete {
                self.status = "file-backed notes are not deleted via switcher".to_string();
                self.switcher.delete_confirm = None;
                return;
            }
            let access_mode = match db.get_note_meta(&item.id) {
                Ok(Some(note)) => note.access_mode,
                Ok(None) => NoteAccessMode::None,
                Err(error) => {
                    self.status = format!("delete check failed: {error}");
                    NoteAccessMode::None
                }
            };
            self.switcher.delete_confirm = Some(SwitcherDeleteConfirm {
                note_id: item.id.clone(),
                note_title: item.title.clone(),
                requires_password: matches!(
                    access_mode,
                    NoteAccessMode::Locked | NoteAccessMode::Encrypted
                ),
                access_mode,
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
                let note = new_note_with_context(
                    db,
                    &self.note_creation_theme,
                    self.working_collection_id.as_deref(),
                )?;
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
        let suggestions =
            list_terminal_command_suggestions(self.command_mode(), &self.command_input);
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
        options.sort_by(|a, b| a.token.cmp(&b.token));
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
        self.command_cursor = usize::MAX;
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
        self.command_cursor = usize::MAX;
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
        self.command_cursor = usize::MAX;
        self.update_command_status();
    }

    pub(super) fn handle_command_bar_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        match key {
            Key::Esc => {
                self.mode = if self.vim_enabled {
                    UiMode::Normal
                } else if self.command_bar_from_normal {
                    UiMode::Normal
                } else {
                    UiMode::Editor
                };
                self.command_input.clear();
                self.dismiss_command_completion_menu();
                self.command_history_index = None;
                self.command_selection = None;
                self.command_selection_linewise = false;
                self.status = if self.mode == UiMode::Normal {
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
            Key::BackTab => {
                self.command_history_index = None;
                if self.command_completion.visible {
                    self.move_command_completion_selection(-1);
                } else {
                    self.open_command_completion_menu();
                }
            }
            Key::ArrowLeft if self.command_completion.visible => {
                self.move_command_completion_selection(-1);
            }
            Key::ArrowRight if self.command_completion.visible => {
                self.move_command_completion_selection(1);
            }
            Key::Backspace => {
                self.command_history_index = None;
                self.dismiss_command_completion_menu();
                text_input::apply_key(&mut self.command_input, &mut self.command_cursor, &key);
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
            Key::ArrowUp => {
                self.cycle_command_history_prev();
            }
            Key::ArrowDown => {
                self.cycle_command_history_next();
            }
            other => {
                if text_input::apply_key(&mut self.command_input, &mut self.command_cursor, &other)
                {
                    self.command_history_index = None;
                    self.dismiss_command_completion_menu();
                    self.update_command_status();
                }
            }
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
            cross_note: self.active_note.modules.cross_note,
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
            cross_note: plan.next.cross_note,
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
                    || previous_modules.variables != self.active_note.modules.variables
                    || previous_modules.cross_note != self.active_note.modules.cross_note;
                if calc_module_changed {
                    self.calc_runtime.viewport_only = self.note_math_module_enabled()
                        && self.editor.lines.len() >= CALC_VIEWPORT_ONLY_MIN_LINES
                        && self.active_has_variable_assignments()
                        && !self.calc.cached_has_builtin_formula;
                    if self.note_math_module_enabled() {
                        self.calc.stale = true;
                        if self.calc_runtime.viewport_only {
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

    fn handle_choose_working_collection(
        &mut self,
        db: &Db,
        collection_name: Option<String>,
    ) -> Result<String, String> {
        let Some(raw_name) = collection_name.map(|value| value.trim().to_string()) else {
            return Ok("usage: collection choose <collection|none>".to_string());
        };
        if raw_name.is_empty() {
            return Ok("usage: collection choose <collection|none>".to_string());
        }
        if raw_name.eq_ignore_ascii_case("none") {
            self.working_collection_id = None;
            self.working_collection_name = None;
            self.refresh_switcher_items(db)?;
            return Ok("working collection cleared".to_string());
        }
        let Some(collection) = db.get_collection_by_name(&raw_name)? else {
            return Ok(format!("collection not found: {raw_name}"));
        };
        self.working_collection_id = Some(collection.id);
        self.working_collection_name = Some(collection.name.clone());
        self.refresh_switcher_items(db)?;
        Ok(format!("working collection: {}", collection.name))
    }

    fn handle_add_remove_collection_membership(
        &mut self,
        db: &Db,
        collection_name: Option<String>,
        add: bool,
    ) -> Result<String, String> {
        let capabilities = note_sources(db).capabilities_for_note_id(&self.active_note.id);
        if !capabilities.can_module_persist {
            return Ok("collections are not supported for file-backed notes".to_string());
        }
        let Some(raw_name) = collection_name.map(|value| value.trim().to_string()) else {
            return Ok(if add {
                "usage: collection join <name>".to_string()
            } else {
                "usage: collection leave <name>".to_string()
            });
        };
        if raw_name.is_empty() {
            return Ok(if add {
                "usage: collection join <name>".to_string()
            } else {
                "usage: collection leave <name>".to_string()
            });
        }
        let Some(collection) = db.get_collection_by_name(&raw_name)? else {
            return Ok(format!("collection not found: {raw_name}"));
        };

        let mut current = db.get_note_collection_ids(&self.active_note.id)?;
        let has_membership = current.iter().any(|id| id == &collection.id);
        if add {
            if has_membership {
                return Ok(format!("note already in collection {}", collection.name));
            }
            current.push(collection.id.clone());
            db.set_note_collections(&self.active_note.id, &current)?;
            self.refresh_switcher_items(db)?;
            return Ok(format!("added to collection {}", collection.name));
        }

        if !has_membership {
            return Ok(format!("note is not in collection {}", collection.name));
        }
        current.retain(|id| id != &collection.id);
        db.set_note_collections(&self.active_note.id, &current)?;
        self.refresh_switcher_items(db)?;
        Ok(format!("removed from collection {}", collection.name))
    }

    fn handle_create_collection(
        &mut self,
        db: &Db,
        collection_name: Option<String>,
    ) -> Result<String, String> {
        let Some(raw_name) = collection_name.map(|value| value.trim().to_string()) else {
            return Ok("usage: collection create <name>".to_string());
        };
        if raw_name.is_empty() {
            return Ok("usage: collection create <name>".to_string());
        }
        let created = db.create_collection(&raw_name, "")?;
        self.refresh_switcher_items(db)?;
        Ok(format!("collection created: {}", created.name))
    }

    fn handle_delete_or_purge_collection(
        &mut self,
        db: &Db,
        collection_name: Option<String>,
        purge: bool,
    ) -> Result<String, String> {
        let Some(raw_name) = collection_name.map(|value| value.trim().to_string()) else {
            return Ok(if purge {
                "usage: collection purge <name>".to_string()
            } else {
                "usage: collection delete <name>".to_string()
            });
        };
        if raw_name.is_empty() {
            return Ok(if purge {
                "usage: collection purge <name>".to_string()
            } else {
                "usage: collection delete <name>".to_string()
            });
        }
        let Some(collection) = db.get_collection_by_name(&raw_name)? else {
            return Ok(format!("collection not found: {raw_name}"));
        };

        let active_note_in_target_collection = if purge {
            db.get_note_collection_ids(&self.active_note.id)?
                .iter()
                .any(|id| id == &collection.id)
        } else {
            false
        };
        let deleted_notes = if purge {
            Some(db.purge_collection(&collection.id)?)
        } else {
            db.delete_collection(&collection.id)?;
            None
        };

        if self.working_collection_id.as_deref() == Some(collection.id.as_str()) {
            self.working_collection_id = None;
            self.working_collection_name = None;
        }
        if self.switcher.collection_filter_id.as_deref() == Some(collection.id.as_str()) {
            self.switcher.collection_filter_id = self.working_collection_id.clone();
            self.switcher.collection_filter_name = self.working_collection_name.clone();
        }
        if self.content_search.collection_filter_id.as_deref() == Some(collection.id.as_str()) {
            self.content_search.collection_filter_id = self.working_collection_id.clone();
            self.content_search.collection_filter_name = self.working_collection_name.clone();
        }
        self.refresh_switcher_items(db)?;
        if active_note_in_target_collection {
            if let Some(next) = self.switcher.items.first().cloned() {
                if let Some(note) = db.get_note(&next.id)? {
                    self.set_active_note(db, note)?;
                }
            } else {
                let created = new_note_with_context(
                    db,
                    &self.note_creation_theme,
                    self.working_collection_id.as_deref(),
                )?;
                self.set_active_note(db, created)?;
                self.refresh_switcher_items(db)?;
            }
        }
        Ok(if let Some(count) = deleted_notes {
            format!(
                "collection purged: {} ({} notes deleted)",
                collection.name, count
            )
        } else {
            format!("collection deleted: {}", collection.name)
        })
    }

    pub(super) fn execute_terminal_command(&mut self, db: &Db, cmd: &str) {
        if let Some(message) = self.try_execute_terminal_perf_command(cmd) {
            self.status = message;
            return;
        }

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
                                    "note session-locked; not encrypted at rest".to_string()
                                }
                                crate::editor_core::command_catalog::NoteSecurityAction::Unlock => {
                                    "note unlocked for this session".to_string()
                                }
                                crate::editor_core::command_catalog::NoteSecurityAction::Encrypt => {
                                    "note encrypted at rest".to_string()
                                }
                                crate::editor_core::command_catalog::NoteSecurityAction::Decrypt => {
                                    "note decrypted; stored without at-rest encryption".to_string()
                                }
                                crate::editor_core::command_catalog::NoteSecurityAction::Unprotect => {
                                    "note decrypted; at-rest encryption removed".to_string()
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
                crate::editor_core::engine::HostCommandPlan::Collection { action, collection } => {
                    self.status = match action {
                        crate::editor_core::engine::HostCollectionAction::Choose => {
                            self.handle_choose_working_collection(db, collection)
                        }
                        crate::editor_core::engine::HostCollectionAction::Clear => {
                            self.handle_choose_working_collection(db, Some("none".to_string()))
                        }
                        crate::editor_core::engine::HostCollectionAction::Create => {
                            self.handle_create_collection(db, collection)
                        }
                        crate::editor_core::engine::HostCollectionAction::Delete => {
                            self.handle_delete_or_purge_collection(db, collection, false)
                        }
                        crate::editor_core::engine::HostCollectionAction::Update => {
                            self.open_collection_edit_dialog_by_name(db, collection)
                        }
                        crate::editor_core::engine::HostCollectionAction::Purge => {
                            self.handle_delete_or_purge_collection(db, collection, true)
                        }
                        crate::editor_core::engine::HostCollectionAction::Add => {
                            self.handle_add_remove_collection_membership(db, collection, true)
                        }
                        crate::editor_core::engine::HostCollectionAction::Remove => {
                            self.handle_add_remove_collection_membership(db, collection, false)
                        }
                    }
                    .unwrap_or_else(|error| format!("collection command failed: {error}"));
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::Today => {
                    if let Err(error) = self.open_today_note(db) {
                        self.status = format!("today: {error}");
                    }
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::Date => {
                    self.open_date_picker(DatePickerAction::InsertDate, false);
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::Remind => {
                    self.open_date_picker(DatePickerAction::SetRemind, true);
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::RemindToggle => {
                    let line_number = (self.editor.cursor_line + 1) as i64;
                    let before_reminder =
                        self.reminder_ghosts.get(&self.editor.cursor_line).cloned();
                    match db.delete_reminder(&self.active_note.id, line_number) {
                        Ok(true) => {
                            self.reminder_ghosts.remove(&self.editor.cursor_line);
                            if before_reminder.is_some() {
                                self.push_reminder_undo_entry(
                                    self.editor.cursor_line,
                                    before_reminder,
                                    None,
                                );
                            }
                            self.status =
                                format!("remind removed on line {}", self.editor.cursor_line + 1);
                        }
                        Ok(false) => {
                            self.open_date_picker(DatePickerAction::SetRemind, true);
                        }
                        Err(error) => {
                            self.status = format!("remind toggle failed: {error}");
                        }
                    }
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::Export { format, path } => {
                    let content = join_lines(&self.editor.lines);
                    self.status = match (format, path) {
                        (crate::editor_core::command_catalog::ExportFormat::Pdf, None) => {
                            "usage: export pdf <path>".to_string()
                        }
                        (
                            crate::editor_core::command_catalog::ExportFormat::Md
                            | crate::editor_core::command_catalog::ExportFormat::Txt,
                            None,
                        ) => {
                            let _ = self.set_clipboard_charwise(content);
                            self.with_clipboard_status(format!(
                                "exported {} to clipboard",
                                format.as_str()
                            ))
                        }
                        (
                            crate::editor_core::command_catalog::ExportFormat::Md
                            | crate::editor_core::command_catalog::ExportFormat::Txt,
                            Some(path),
                        ) => {
                            match crate::commands::export::export_to_file_blocking(&path, &content)
                            {
                                Ok(()) => format!("exported {} to {}", format.as_str(), path),
                                Err(error) => format!("export failed: {error}"),
                            }
                        }
                        (crate::editor_core::command_catalog::ExportFormat::Pdf, Some(path)) => {
                            match crate::commands::export::export_markdown_to_pdf_file(
                                &note_sources(db),
                                &self.active_note.id,
                                &path,
                                &content,
                                &crate::commands::export::PdfExportPalette::default(),
                            ) {
                                Ok(()) => format!("exported {} to {}", format.as_str(), path),
                                Err(error) => format!("export failed: {error}"),
                            }
                        }
                    };
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::BackupExport { path } => {
                    let Some(path) = path else {
                        self.status = "usage: backup export <path.zip>".to_string();
                        return;
                    };
                    if self.dirty {
                        if let Err(error) = self.save(db) {
                            self.status = format!("backup failed: save failed: {error}");
                            return;
                        }
                    }
                    let (tx, rx) = std::sync::mpsc::channel();
                    let db_for_thread = db.clone();
                    std::thread::spawn(move || {
                        let result = crate::commands::backup::backup_notes_database_blocking(
                            &db_for_thread,
                            &path,
                        )
                        .map(|r| format!("backed up notes to {}", r.path));
                        let _ = tx.send(super::BackupThreadResult::ExportDone(result));
                    });
                    self.backup.rx = Some(rx);
                    self.backup.anim_op = super::BackupAnimOp::Export;
                    self.backup.anim_dots = 1;
                    self.backup.anim_last_tick = Some(std::time::Instant::now());
                    self.status = "exporting backup.".to_string();
                    return;
                }
                crate::editor_core::engine::HostCommandPlan::BackupLoad { path } => {
                    let Some(path) = path else {
                        self.status = "usage: backup load <path.zip>".to_string();
                        return;
                    };
                    if self.dirty {
                        if let Err(error) = self.save(db) {
                            self.status = format!("backup load failed: save failed: {error}");
                            return;
                        }
                    }
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let result =
                            crate::commands::backup::stage_restore_from_zip(&path).map(|_| ());
                        let _ = tx.send(super::BackupThreadResult::LoadStageDone(result));
                    });
                    self.backup.rx = Some(rx);
                    self.backup.anim_op = super::BackupAnimOp::Load;
                    self.backup.anim_dots = 1;
                    self.backup.anim_last_tick = Some(std::time::Instant::now());
                    self.status = "loading backup.".to_string();
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
                crate::editor_core::engine::HostCommandPlan::WebSearch { query } => {
                    self.open_web_search(query);
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

    fn try_execute_terminal_perf_command(&mut self, cmd: &str) -> Option<String> {
        let normalized = crate::editor_core::command_catalog::normalize_command(cmd);
        if normalized.is_empty() {
            return None;
        }
        let tokens = normalized.split_whitespace().collect::<Vec<_>>();
        let root = tokens.first().copied().unwrap_or_default();
        if root != "perf" && root != "profile" && root != "profiler" {
            return None;
        }

        let subcommand = tokens.get(1).copied().unwrap_or("status");
        let path = crate::startup_log::startup_log_path("tui_perf");
        let path_text = path.display().to_string();
        match subcommand {
            "where" => Some(format!("perf logs: {path_text}")),
            "status" => Some(self.perf_status_summary()),
            "on" | "enable" | "1" => {
                self.perf_trace.enabled = true;
                Some(self.perf_status_summary())
            }
            "off" | "disable" | "0" => {
                self.perf_trace.enabled = false;
                Some(self.perf_status_summary())
            }
            "toggle" => {
                self.perf_trace.enabled = !self.perf_trace.enabled;
                Some(self.perf_status_summary())
            }
            "clear" | "reset" => {
                self.perf_trace.buckets.clear();
                Some(self.perf_status_summary())
            }
            "capacity" | "cap" => {
                let parsed = tokens
                    .get(2)
                    .and_then(|raw| raw.parse::<usize>().ok())
                    .unwrap_or(0);
                if parsed == 0 {
                    return Some("perf usage: perf cap <positive-number>".to_string());
                }
                self.perf_trace.capacity = parsed;
                for bucket in self.perf_trace.buckets.values_mut() {
                    if bucket.samples_ms.len() > parsed {
                        let drop = bucket.samples_ms.len() - parsed;
                        bucket.samples_ms.drain(0..drop);
                    }
                }
                Some(self.perf_status_summary())
            }
            "dump" => {
                let top = tokens
                    .get(2)
                    .and_then(|raw| raw.parse::<usize>().ok())
                    .unwrap_or(12);
                let report = self.perf_dump_report(top);
                let mut write_errors = 0usize;
                for line in report.lines() {
                    if crate::startup_log::append_startup_log_line("tui_perf", line).is_err() {
                        write_errors += 1;
                    }
                }
                if write_errors > 0 {
                    Some(format!(
                        "perf dump write issue ({write_errors} lines) -> {path_text}"
                    ))
                } else {
                    Some(format!("perf dump logged -> {path_text}"))
                }
            }
            _ => Some(
                "perf usage: perf [status|on|off|toggle|dump [top]|where|clear|cap <n>]"
                    .to_string(),
            ),
        }
    }

    pub(super) fn byte_offset_for_line_col(&self, line_idx: usize, col: usize) -> usize {
        let mut offset = 0;
        for (i, line) in self.editor.lines.iter().enumerate() {
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
        let anchor = self.editor.selection_anchor?;
        match self.mode {
            UiMode::Visual => {
                let anchor_offset = self.byte_offset_for_line_col(anchor.0, anchor.1);
                let head_offset =
                    self.byte_offset_for_line_col(self.editor.cursor_line, self.editor.cursor_col);
                Some(crate::editor_core::types::SelectionSnapshot {
                    anchor: anchor_offset,
                    head: head_offset,
                })
            }
            UiMode::VisualLine => {
                let anchor_line = anchor.0.min(self.editor.lines.len().saturating_sub(1));
                let head_line = self
                    .editor
                    .cursor_line
                    .min(self.editor.lines.len().saturating_sub(1));
                let anchor_line_len = line_char_len(&self.editor.lines[anchor_line]);
                let head_line_len = line_char_len(&self.editor.lines[head_line]);

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

    pub(super) fn invalidate_joined_text_cache(&mut self) {
        self.editor.joined_text_cache = None;
    }

    fn joined_text_cached_ref(&mut self) -> &str {
        if self.editor.joined_text_cache.is_none() {
            self.editor.joined_text_cache = Some(join_lines(&self.editor.lines));
        }
        self.editor.joined_text_cache.as_deref().unwrap()
    }

    pub(super) fn build_snapshot(&mut self) -> crate::editor_core::types::EditorContextSnapshot {
        let text = self.joined_text_cached_ref().to_owned();
        let fallback_cursor =
            self.byte_offset_for_line_col(self.editor.cursor_line, self.editor.cursor_col);
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

    pub(super) fn remap_operation_from_scope(
        op: &crate::editor_core::types::EditOperation,
        scope_start_offset: usize,
    ) -> crate::editor_core::types::EditOperation {
        let mut mapped = op.clone();
        for change in &mut mapped.changes {
            change.from = change.from.saturating_add(scope_start_offset);
            change.to = change.to.saturating_add(scope_start_offset);
        }
        if let Some(selection) = mapped.selection.as_mut() {
            selection.anchor = selection.anchor.saturating_add(scope_start_offset);
            if let Some(head) = selection.head.as_mut() {
                *head = head.saturating_add(scope_start_offset);
            }
        }
        mapped
    }

    pub(super) fn build_scoped_snapshot_for_line_span(
        &self,
        start_line: usize,
        end_line: usize,
        changed_range_abs: Option<crate::editor_core::types::TextRange>,
    ) -> (crate::editor_core::types::EditorContextSnapshot, usize) {
        if self.editor.lines.is_empty() {
            let snapshot = crate::editor_core::types::EditorContextSnapshot {
                text: String::new(),
                selection: crate::editor_core::types::SelectionSnapshot { anchor: 0, head: 0 },
                changed_range: None,
            };
            return (snapshot, 0);
        }

        let clamped_start = start_line.min(self.editor.lines.len().saturating_sub(1));
        let clamped_end = end_line
            .min(self.editor.lines.len().saturating_sub(1))
            .max(clamped_start);
        let scope_start_offset = self.byte_offset_for_line_col(clamped_start, 0);
        let scope_text = join_lines(&self.editor.lines[clamped_start..=clamped_end]);
        let scope_len = scope_text.len();

        let fallback_cursor =
            self.byte_offset_for_line_col(self.editor.cursor_line, self.editor.cursor_col);
        let selection_abs =
            self.command_selection
                .unwrap_or(crate::editor_core::types::SelectionSnapshot {
                    anchor: fallback_cursor,
                    head: fallback_cursor,
                });
        let selection = crate::editor_core::types::SelectionSnapshot {
            anchor: selection_abs
                .anchor
                .saturating_sub(scope_start_offset)
                .min(scope_len),
            head: selection_abs
                .head
                .saturating_sub(scope_start_offset)
                .min(scope_len),
        };

        let changed_range = changed_range_abs.and_then(|changed| {
            let scope_end = scope_start_offset.saturating_add(scope_len);
            if changed.to < scope_start_offset || changed.from > scope_end {
                return None;
            }
            let from_abs = changed.from.max(scope_start_offset).min(scope_end);
            let to_abs = changed.to.max(scope_start_offset).min(scope_end);
            Some(crate::editor_core::types::TextRange {
                from: from_abs.saturating_sub(scope_start_offset),
                to: to_abs.saturating_sub(scope_start_offset),
            })
        });

        let snapshot = crate::editor_core::types::EditorContextSnapshot {
            text: scope_text,
            selection,
            changed_range,
        };
        (snapshot, scope_start_offset)
    }

    pub(super) fn build_scoped_context_for_line_span(
        &self,
        start_line: usize,
        end_line: usize,
        changed_range_abs: Option<crate::editor_core::types::TextRange>,
    ) -> (crate::editor_core::context::ResolvedContext<'static>, usize) {
        let (snapshot, scope_start_offset) =
            self.build_scoped_snapshot_for_line_span(start_line, end_line, changed_range_abs);
        (
            crate::editor_core::context::ResolvedContext::new(snapshot),
            scope_start_offset,
        )
    }

    pub(super) fn refresh_collection_switcher_items(&mut self, db: &Db) -> Result<(), String> {
        self.collection_switcher.items = switcher::load_collection_meta(db)?;
        self.recompute_collection_switcher_matches();
        Ok(())
    }

    pub(super) fn recompute_collection_switcher_matches(&mut self) {
        let query = self.collection_switcher.query.trim();
        if query.is_empty() {
            self.collection_switcher.matches = (0..self.collection_switcher.items.len()).collect();
            self.collection_switcher.selected = 0;
            return;
        }

        let mut scored: Vec<(usize, i32)> = Vec::new();
        for (idx, item) in self.collection_switcher.items.iter().enumerate() {
            let haystack = if item.description.trim().is_empty() {
                item.name.clone()
            } else {
                format!("{} {}", item.name, item.description)
            };
            if let Some(score) = switcher::fuzzy_score(query, &haystack) {
                scored.push((idx, score));
            }
        }
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        self.collection_switcher.matches = scored.into_iter().map(|(idx, _)| idx).collect();
        self.collection_switcher.selected = 0;
    }

    pub(super) fn open_collection_switcher(&mut self, db: &Db) -> Result<(), String> {
        self.dismiss_variable_autocomplete_popup();
        if self.mode == UiMode::ContentSearch {
            self.clear_content_search_session();
        }
        self.refresh_collection_switcher_items(db)?;
        self.mode = UiMode::CollectionSwitcher;
        self.collection_switcher.query.clear();
        self.recompute_collection_switcher_matches();
        self.collection_switcher.edit_dialog = None;
        self.status =
            "Collections: type to filter, Enter choose, Ctrl+E edit, Esc close".to_string();
        Ok(())
    }

    pub(super) fn close_collection_switcher(&mut self) {
        self.dismiss_variable_autocomplete_popup();
        self.mode = UiMode::Editor;
        self.collection_switcher.query.clear();
        self.collection_switcher.matches.clear();
        self.collection_switcher.selected = 0;
        self.collection_switcher.edit_dialog = None;
        self.status = format!("editing {}", self.active_note.id);
    }

    fn open_collection_edit_dialog_for_selected(&mut self, db: &Db) -> Result<(), String> {
        let Some(match_idx) = self
            .collection_switcher
            .matches
            .get(self.collection_switcher.selected)
            .copied()
        else {
            return Ok(());
        };
        let Some(item) = self.collection_switcher.items.get(match_idx) else {
            return Ok(());
        };
        let Some(collection_id) = item.id.clone() else {
            self.status = "cannot edit All collections".to_string();
            return Ok(());
        };

        let default_tags = db.list_collection_default_tags(&collection_id)?;
        self.collection_switcher.edit_dialog = Some(CollectionEditDialogState {
            collection_id,
            selected_field: 0,
            name: item.name.clone(),
            description: item.description.clone(),
            default_tags: default_tags.join(", "),
        });
        self.status = format!("editing collection {}", item.name);
        Ok(())
    }

    fn open_collection_edit_dialog_by_name(
        &mut self,
        db: &Db,
        collection_name: Option<String>,
    ) -> Result<String, String> {
        let Some(raw_name) = collection_name.map(|value| value.trim().to_string()) else {
            return Ok("usage: collection update <name>".to_string());
        };
        if raw_name.is_empty() {
            return Ok("usage: collection update <name>".to_string());
        }
        let Some(collection) = db.get_collection_by_name(&raw_name)? else {
            return Ok(format!("collection not found: {raw_name}"));
        };

        self.refresh_collection_switcher_items(db)?;
        self.mode = UiMode::CollectionSwitcher;
        self.collection_switcher.query.clear();
        self.recompute_collection_switcher_matches();
        if let Some(position) = self
            .collection_switcher
            .items
            .iter()
            .position(|entry| entry.id.as_deref() == Some(collection.id.as_str()))
        {
            if let Some(match_position) = self
                .collection_switcher
                .matches
                .iter()
                .position(|idx| *idx == position)
            {
                self.collection_switcher.selected = match_position;
            }
        }
        self.open_collection_edit_dialog_for_selected(db)?;
        Ok(format!(
            "collection update: editing {} (Enter save, Esc cancel)",
            collection.name
        ))
    }

    fn save_collection_edit_dialog(&mut self, db: &Db) -> Result<(), String> {
        let Some(dialog) = self.collection_switcher.edit_dialog.clone() else {
            return Ok(());
        };
        let name = dialog.name.trim();
        if name.is_empty() {
            self.status = "collection name is required".to_string();
            return Ok(());
        }
        let mut current = db
            .get_collection(&dialog.collection_id)?
            .ok_or_else(|| "collection no longer exists".to_string())?;
        if current.name != name {
            current = db.rename_collection(&current.id, name)?;
        }
        let description = dialog.description.trim();
        if current.description != description {
            current = db.update_collection_description(&current.id, description)?;
        }
        let tags = parse_collection_default_tags_input(&dialog.default_tags);
        db.set_collection_default_tags(&current.id, &tags)?;
        if self.working_collection_id.as_deref() == Some(current.id.as_str()) {
            self.working_collection_name = Some(current.name.clone());
        }
        self.refresh_collection_switcher_items(db)?;
        self.refresh_switcher_items(db)?;
        self.collection_switcher.edit_dialog = None;
        self.status = format!("collection updated: {}", current.name);
        Ok(())
    }

    pub(super) fn open_switcher(&mut self, db: &Db) -> Result<(), String> {
        self.dismiss_variable_autocomplete_popup();
        if self.mode == UiMode::ContentSearch {
            self.clear_content_search_session();
        }
        self.reset_switcher_filter_to_working_collection();
        self.refresh_switcher_items(db)?;
        self.mode = UiMode::Switcher;
        self.switcher.query.clear();
        self.recompute_switcher_matches();
        self.switcher.open_confirm = None;
        self.switcher.delete_confirm = None;
        self.update_switcher_status_hint();
        Ok(())
    }

    pub(super) fn close_switcher(&mut self) {
        self.dismiss_variable_autocomplete_popup();
        self.mode = UiMode::Editor;
        self.switcher.query.clear();
        self.switcher.matches.clear();
        self.switcher.selected = 0;
        self.switcher.open_confirm = None;
        self.switcher.delete_confirm = None;
        self.status = format!("editing {}", self.active_note.id);
    }

    pub(super) fn recompute_switcher_matches(&mut self) {
        let query = self.switcher.query.trim();
        if query.is_empty() {
            self.switcher.matches = (0..self.switcher.items.len()).collect();
            self.switcher.selected = 0;
            return;
        }

        self.switcher.score_scratch.clear();
        for (idx, item) in self.switcher.items.iter().enumerate() {
            if let Some(score) = switcher::fuzzy_score(query, &item.title) {
                self.switcher.score_scratch.push((idx, score));
            }
        }
        self.switcher
            .score_scratch
            .sort_unstable_by(|a, b| b.1.cmp(&a.1));
        self.switcher.matches = self
            .switcher
            .score_scratch
            .iter()
            .map(|(idx, _)| *idx)
            .collect();
        self.switcher.selected = 0;
    }

    pub(super) fn save(&mut self, db: &Db) -> Result<(), String> {
        self.save_with_options(db, false)
    }

    pub(super) fn save_with_options(&mut self, db: &Db, force: bool) -> Result<(), String> {
        // Never race an in-flight autosave on the note's revision.
        self.poll_background_save(db, true)?;
        let started = Instant::now();
        if self.format_on_save {
            self.execute_terminal_command(db, "format");
        }
        if !self.dirty {
            self.record_perf_duration("tui.save", "noop", started.elapsed());
            return Ok(());
        }
        self.sync_reminder_ghosts_if_dirty(db)?;
        let body = self
            .editor
            .joined_text_cache
            .take()
            .unwrap_or_else(|| join_lines(&self.editor.lines));
        let saved = note_sources(db).save_note_revision_by_id(
            &self.active_note.id,
            &body,
            app_core::note_sources::SaveOptions {
                expected_revision: Some(self.active_note.updated_at.clone()),
                force,
            },
        )?;
        // Only the revision moves on; the document itself stays in
        // `self.editor.lines` and is never round-tripped through the store.
        self.active_note.id = saved.id;
        self.active_note.updated_at = saved.updated_at;
        self.editor.joined_text_cache = Some(body);
        self.dirty = false;
        self.history.checkpoint(
            &self.editor.lines,
            self.editor.cursor_line,
            self.editor.cursor_col,
        );
        self.render_state.dirty = true;
        // Only do a full DB scan when the first line (note title) changed.
        // Body-only saves don't affect the prefix index or wiki link caches.
        if self.switcher.needs_title_refresh {
            self.refresh_switcher_items(db)?;
        } else {
            self.update_switcher_item_after_body_save();
        }
        self.record_perf_duration(
            "tui.save",
            if force { "forced" } else { "normal" },
            started.elapsed(),
        );
        Ok(())
    }

    /// Autosave without blocking input: the write runs on a thread and
    /// `poll_background_save` applies the result.
    pub(super) fn start_background_autosave(&mut self, db: &Db) -> Result<(), String> {
        if self.background_save.is_some() {
            return Ok(());
        }
        if self.format_on_save {
            self.execute_terminal_command(db, "format");
        }
        if !self.dirty {
            return Ok(());
        }
        if !self.active_note_is_editable() {
            self.set_locked_note_status();
            return Ok(());
        }
        self.sync_reminder_ghosts_if_dirty(db)?;
        let body = self
            .editor
            .joined_text_cache
            .clone()
            .unwrap_or_else(|| join_lines(&self.editor.lines));
        let note_id = self.active_note.id.clone();
        let options = app_core::note_sources::SaveOptions {
            expected_revision: Some(self.active_note.updated_at.clone()),
            force: false,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let db = db.clone();
        let thread_note_id = note_id.clone();
        std::thread::spawn(move || {
            let result =
                note_sources(&db).save_note_revision_by_id(&thread_note_id, &body, options);
            let _ = tx.send(result);
        });
        self.background_save = Some(super::BackgroundSave {
            rx,
            note_id,
            edit_mark: self.last_edit,
        });
        Ok(())
    }

    /// Applies a finished background autosave; with `wait`, blocks until the
    /// in-flight one finishes. Errors other than a locked note propagate,
    /// as they did when autosave ran inline.
    pub(super) fn poll_background_save(&mut self, db: &Db, wait: bool) -> Result<(), String> {
        let Some(job) = self.background_save.as_ref() else {
            return Ok(());
        };
        let result = if wait {
            job.rx
                .recv()
                .unwrap_or_else(|_| Err("autosave stopped unexpectedly".to_string()))
        } else {
            match job.rx.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(()),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Err("autosave stopped unexpectedly".to_string())
                }
            }
        };
        let job = self.background_save.take().expect("save in flight");
        let saved = match result {
            Ok(saved) => saved,
            Err(error) if Self::is_locked_note_error(&error) => {
                self.set_locked_note_status();
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if job.note_id != self.active_note.id {
            return Ok(());
        }
        self.active_note.id = saved.id;
        self.active_note.updated_at = saved.updated_at;
        if self.last_edit == job.edit_mark {
            self.dirty = false;
            self.history.checkpoint(
                &self.editor.lines,
                self.editor.cursor_line,
                self.editor.cursor_col,
            );
        }
        self.status = format!("autosaved {}", self.active_note.id);
        self.render_state.dirty = true;
        if self.switcher.needs_title_refresh {
            self.refresh_switcher_items(db)?;
        } else {
            self.update_switcher_item_after_body_save();
        }
        Ok(())
    }

    fn update_switcher_item_after_body_save(&mut self) {
        let note_id = &self.active_note.id;
        let updated_at = self.active_note.updated_at.clone();
        if let Some(item) = self.switcher.items.iter_mut().find(|i| &i.id == note_id) {
            item.updated_at = updated_at;
        }
        // Keep the list sorted by updated_at DESC (DB order); the saved note
        // just became the most-recently-modified so it floats to the top.
        self.switcher
            .items
            .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    }

    pub(super) fn sync_reminder_ghosts_if_dirty(&mut self, db: &Db) -> Result<(), String> {
        if !self.reminders_dirty {
            return Ok(());
        }
        self.reminder_ghosts =
            load_note_reminder_ghosts(db, &self.active_note.id, &self.editor.lines)?;
        self.render_state.dirty = true;
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
                if reminder.reminded_at_ms.is_none() && reminder.remind_at_ms <= now_ms {
                    Some(*line_idx)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        due_lines.sort_unstable();
        if !due_lines.is_empty() {
            self.render_state.dirty = true;
        }

        for line_idx in due_lines {
            let Some(reminder) = self.reminder_ghosts.get(&line_idx).cloned() else {
                continue;
            };
            let body = self
                .editor
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
            match db.mark_reminder_reminded(&self.active_note.id, line_number, now_ms) {
                Ok(updated) => {
                    let reminded_at = updated
                        .and_then(|entry| entry.reminded_at_ms)
                        .unwrap_or(now_ms);
                    if let Some(entry) = self.reminder_ghosts.get_mut(&line_idx) {
                        entry.reminded_at_ms = Some(reminded_at);
                        entry.line_text = self
                            .editor
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
        // Pre-load switcher items so the empty query can fall back to titles.
        if self.switcher.items.is_empty() {
            self.refresh_switcher_items(db)?;
        }
        self.mode = UiMode::ContentSearch;
        self.clear_content_search_session();
        self.reset_content_search_filter_to_working_collection();
        self.content_search.results = self.content_search_title_fallback_results("");
        self.switcher.open_confirm = None;
        self.switcher.delete_confirm = None;
        self.update_content_search_status_hint();
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
            Key::Ctrl('l') => {
                self.toggle_content_search_collection_filter_and_refresh(db)?;
            }
            Key::Tab => {
                self.open_switcher(db)?;
            }
            Key::Ctrl('w') | Key::CtrlBackspace => {
                if self.content_search.cursor_col == char_len(&self.content_search.query) {
                    trim_trailing_word(&mut self.content_search.query);
                    self.content_search.cursor_col = char_len(&self.content_search.query);
                } else {
                    // Keep behavior predictable away from end-of-line by deleting one char.
                    if remove_char_before_char_col(
                        &mut self.content_search.query,
                        self.content_search.cursor_col,
                    ) {
                        self.content_search.cursor_col =
                            self.content_search.cursor_col.saturating_sub(1);
                    }
                }
                self.refresh_content_search_preview();
            }
            Key::ArrowUp => {
                if self.content_search.selected > 0 {
                    self.content_search.selected -= 1;
                }
            }
            Key::ArrowDown => {
                if self.content_search.selected + 1 < self.content_search.results.len() {
                    self.content_search.selected += 1;
                }
            }
            Key::ArrowLeft => {
                self.content_search.cursor_col = self.content_search.cursor_col.saturating_sub(1);
            }
            Key::ArrowRight => {
                self.content_search.cursor_col =
                    (self.content_search.cursor_col + 1).min(char_len(&self.content_search.query));
            }
            Key::Home => {
                self.content_search.cursor_col = 0;
            }
            Key::End => {
                self.content_search.cursor_col = char_len(&self.content_search.query);
            }
            Key::Backspace => {
                if remove_char_before_char_col(
                    &mut self.content_search.query,
                    self.content_search.cursor_col,
                ) {
                    self.content_search.cursor_col =
                        self.content_search.cursor_col.saturating_sub(1);
                    self.refresh_content_search_preview();
                }
            }
            Key::Delete => {
                if remove_char_at_char_col(
                    &mut self.content_search.query,
                    self.content_search.cursor_col,
                ) {
                    self.refresh_content_search_preview();
                }
            }
            Key::Enter => {
                if let Some(result) = self
                    .content_search
                    .results
                    .get(self.content_search.selected)
                    .cloned()
                {
                    // Check if the note is protected
                    let access_mode = self
                        .switcher
                        .items
                        .iter()
                        .find(|n| n.id == result.id)
                        .map(|n| n.access_mode)
                        .unwrap_or(NoteAccessMode::None);
                    let is_unlocked = self
                        .switcher
                        .items
                        .iter()
                        .find(|n| n.id == result.id)
                        .map(|n| n.is_unlocked)
                        .unwrap_or(false);
                    if access_mode != NoteAccessMode::None && !is_unlocked {
                        self.switcher.open_confirm = Some(SwitcherOpenConfirm {
                            note_id: result.id.clone(),
                            note_title: result.title.clone(),
                            access_mode,
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
                let insert = ch.to_string();
                insert_str_at_char_col(
                    &mut self.content_search.query,
                    self.content_search.cursor_col,
                    &insert,
                );
                self.content_search.cursor_col += 1;
                self.refresh_content_search_preview();
            }
            Key::Paste(text) => {
                let sanitized = text
                    .chars()
                    .filter(|c| *c != '\n' && *c != '\r')
                    .collect::<String>();
                if !sanitized.is_empty() {
                    let added = char_len(&sanitized);
                    insert_str_at_char_col(
                        &mut self.content_search.query,
                        self.content_search.cursor_col,
                        &sanitized,
                    );
                    self.content_search.cursor_col += added;
                    self.refresh_content_search_preview();
                }
            }
            _ => {}
        }
        self.maybe_dispatch_content_search(db);
        Ok(())
    }

    pub(super) fn refresh_switcher_items(&mut self, db: &Db) -> Result<(), String> {
        self.switcher.needs_title_refresh = false;
        if let Some(working_id) = self.working_collection_id.clone() {
            if db.get_collection(&working_id)?.is_none() {
                self.working_collection_id = None;
                self.working_collection_name = None;
                if self.switcher.collection_filter_id.as_deref() == Some(working_id.as_str()) {
                    self.switcher.collection_filter_id = None;
                    self.switcher.collection_filter_name = None;
                }
                if self.content_search.collection_filter_id.as_deref() == Some(working_id.as_str())
                {
                    self.content_search.collection_filter_id = None;
                    self.content_search.collection_filter_name = None;
                }
            }
        }
        let previous_prefix_index = self.wiki_link_prefix_index.clone();
        let switcher_collection_filter = match self.mode {
            UiMode::Switcher => self.switcher.collection_filter_id.as_deref(),
            UiMode::ContentSearch => self.content_search.collection_filter_id.as_deref(),
            _ => self.working_collection_id.as_deref(),
        };
        self.switcher.items = switcher::load_note_meta_filtered(
            db,
            Some(&self.active_note.id),
            switcher_collection_filter,
        )?;
        self.rebuild_wiki_link_prefix_index();
        self.rebuild_wiki_link_note_suggestions_cache();
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
        } else if self.mode == UiMode::CollectionSwitcher {
            self.refresh_collection_switcher_items(db)?;
        }
        Ok(())
    }

    /// Opens today's daily note (creating it from the `[daily]` template)
    /// with the cursor at the end, ready to type.
    pub(super) fn open_today_note(&mut self, db: &Db) -> Result<(), String> {
        if self.autosave_enabled {
            self.save(db)?;
        }
        let stamp = crate::terminal::local_stamp();
        let label = crate::terminal::daily_date_label(stamp, &self.note_creation_theme.date_format);
        let note = app_core::daily::ensure_daily_note(db, &self.daily_config, stamp, &label)?;
        self.set_active_note(db, note)?;
        self.editor.cursor_line = self.editor.lines.len().saturating_sub(1);
        self.editor.cursor_col = line_char_len(self.current_line());
        self.adjust_cursor();
        self.adjust_scroll();
        self.refresh_switcher_items(db)?;
        self.status = format!("today {}", self.active_note.id);
        Ok(())
    }

    pub(super) fn set_active_note(&mut self, db: &Db, note: Note) -> Result<(), String> {
        self.active_note = note;
        let (render_plain_text_file, render_file_language) =
            super::file_render_syntax_for_note_id(&self.active_note.id);
        self.render_state.plain_text_file = render_plain_text_file;
        self.render_state.file_language = render_file_language;
        self.editor.lines = split_lines(&self.active_note.body);
        self.editor.joined_text_cache = None;
        self.active_note.body = String::new();
        self.dismiss_variable_autocomplete_popup();
        self.reminder_ghosts =
            load_note_reminder_ghosts(db, &self.active_note.id, &self.editor.lines)?;
        self.reminders_dirty = false;
        self.last_reminder_check = Instant::now();
        self.editor.cursor_line = 0;
        self.editor.cursor_col = 0;
        self.editor.scroll_line = 0;
        self.editor.scroll_col = 0;
        self.dirty = false;
        self.last_edit = Instant::now();
        self.search.query.clear();
        self.search.matches.clear();
        self.rebuild_wiki_link_prefix_index();
        self.rebuild_wiki_link_note_suggestions_cache();
        self.render_caches.wiki_link_render_cache.clear();
        self.render_caches.wiki_link_line_render_cache.clear();
        self.history = super::build_history_for_note(
            &self.editor.lines,
            self.editor.cursor_line,
            self.editor.cursor_col,
        );
        self.undo_actions.clear();
        self.undo_action_pos = 0;
        self.render_state.fence_checkpoints.truncate(1);
        self.render_state.fence_checkpoints_valid_through = 0;
        self.rescan_calc_flags();
        self.calc_runtime.viewport_only = self.editor.lines.len() >= CALC_VIEWPORT_ONLY_MIN_LINES
            && self.active_has_variable_assignments()
            && !self.calc.cached_has_builtin_formula;
        self.calc_runtime.last_view_eval_range = None;
        if self.calc_runtime.viewport_only
            || (!self.calc.cached_has_builtin_formula
                && !self.active_has_variable_assignments()
                && !self.calc.cached_has_expression)
        {
            self.calc.results = vec![None; self.editor.lines.len()];
            self.calc.cell_results = vec![Vec::new(); self.editor.lines.len()];
            self.calc.variable_names.clear();
            self.calc.calc_dependency_index = None;
            self.calc.line_metadata.clear();
            self.calc.prev_line_metadata.clear();
            self.calc.stale = false;
            self.calc.pathological_window_streak = 0;
            self.calc.forced_full_recompute_remaining = 0;
            self.calc_runtime.recompute_pending = false;
            self.calc_runtime.recompute_due_at = None;
            self.calc_runtime.pending_viewport_pass = false;
            self.calc_runtime.pending_full_pass = false;
        } else if self.should_defer_calc_recompute() {
            self.calc.results = vec![None; self.editor.lines.len()];
            self.calc.cell_results = vec![Vec::new(); self.editor.lines.len()];
            self.calc.variable_names.clear();
            self.calc.calc_dependency_index = None;
            self.calc.line_metadata.clear();
            self.calc.prev_line_metadata.clear();
            self.calc.stale = true;
            self.calc.pathological_window_streak = 0;
            self.calc.forced_full_recompute_remaining = 0;
            self.calc_runtime.recompute_pending = false;
            self.calc_runtime.recompute_due_at = None;
            self.calc_runtime.pending_viewport_pass = false;
            self.calc_runtime.pending_full_pass = false;
        } else {
            self.run_calc_recompute();
        }
        self.recompute_folding();
        self.adjust_cursor();
        self.adjust_scroll();
        if self.calc_runtime.viewport_only && !self.start_viewport_calc_preparation() {
            let editor_height = self.editor_height();
            self.ensure_calc_for_viewport(editor_height, true);
        }
        self.history.checkpoint(
            &self.editor.lines,
            self.editor.cursor_line,
            self.editor.cursor_col,
        );
        self.maybe_compact_buffers_after_note_switch();
        Ok(())
    }

    pub(super) fn open_search(&mut self) {
        self.dismiss_variable_autocomplete_popup();
        self.search.query.clear();
        self.search.matches.clear();
        self.search.current = 0;
        self.search.orig_line = self.editor.cursor_line;
        self.search.orig_col = self.editor.cursor_col;
        self.search.orig_scroll = self.editor.scroll_line;
        self.mode = UiMode::Search;
        self.status = "/".to_string();
    }

    pub(super) fn handle_search_key(&mut self, key: Key) -> Result<(), String> {
        match key {
            Key::Esc => {
                self.editor.cursor_line = self.search.orig_line;
                self.editor.cursor_col = self.search.orig_col;
                self.editor.scroll_line = self.search.orig_scroll;
                self.adjust_cursor();
                self.editor.scroll_line = self
                    .editor
                    .scroll_line
                    .min(self.visible_line_count().saturating_sub(1));
                self.mode = UiMode::Normal;
                self.search.query.clear();
                self.search.matches.clear();
                self.status = "-- NORMAL --".to_string();
            }
            Key::Enter => {
                self.mode = UiMode::Normal;
                self.status = if self.search.matches.is_empty() {
                    "no matches".to_string()
                } else {
                    format!(
                        "/{} ({}/{})",
                        self.search.query,
                        self.search.current + 1,
                        self.search.matches.len()
                    )
                };
            }
            Key::ArrowDown | Key::Ctrl('n') | Key::Tab => {
                self.search_next();
            }
            Key::ArrowUp | Key::Ctrl('p') | Key::BackTab => {
                self.search_prev();
            }
            other => {
                let before = self.search.query.clone();
                if text_input::apply_key(&mut self.search.query, &mut self.search.cursor, &other)
                    && self.search.query != before
                {
                    self.recompute_search();
                }
            }
        }
        Ok(())
    }

    pub(super) fn recompute_search(&mut self) {
        self.search.matches.clear();
        self.search.current = 0;

        let query = self.search.query.to_lowercase();
        if query.is_empty() {
            self.status = "/".to_string();
            return;
        }

        for (line_idx, line) in self.editor.lines.iter().enumerate() {
            for (char_start, char_end) in case_insensitive_matches(line, &query) {
                self.search.matches.push((line_idx, char_start, char_end));
            }
        }

        if !self.search.matches.is_empty() {
            self.jump_to_nearest_match();
        }
        self.update_search_status();
    }

    pub(super) fn update_search_status(&mut self) {
        if self.search.matches.is_empty() {
            self.status = format!("/{} (no matches)", self.search.query);
        } else {
            self.status = format!(
                "/{} ({}/{})",
                self.search.query,
                self.search.current + 1,
                self.search.matches.len()
            );
        }
    }

    pub(super) fn jump_to_nearest_match(&mut self) {
        for (i, &(line, _, _)) in self.search.matches.iter().enumerate() {
            if line >= self.search.orig_line {
                self.search.current = i;
                self.jump_to_current_match();
                return;
            }
        }
        self.search.current = 0;
        self.jump_to_current_match();
    }

    pub(super) fn jump_to_current_match(&mut self) {
        if let Some(&(line, col, _)) = self.search.matches.get(self.search.current) {
            self.editor.cursor_line = line;
            self.editor.cursor_col = col;
            self.adjust_cursor();
            self.adjust_scroll();
        }
    }

    pub(super) fn search_next(&mut self) {
        if self.search.matches.is_empty() {
            return;
        }
        self.search.current = (self.search.current + 1) % self.search.matches.len();
        self.jump_to_current_match();
        self.update_search_status();
    }

    pub(super) fn search_prev(&mut self) {
        if self.search.matches.is_empty() {
            return;
        }
        self.search.current =
            (self.search.current + self.search.matches.len() - 1) % self.search.matches.len();
        self.jump_to_current_match();
        self.update_search_status();
    }

    // ── Web search overlay ──────────────────────────────────────────────

    pub(super) fn open_web_search(&mut self, prefill_query: Option<String>) {
        self.dismiss_variable_autocomplete_popup();
        self.web_search = WebSearchState::default();
        if let Some(q) = prefill_query {
            self.web_search.query = q.clone();
            self.web_search.cursor_col = self.web_search.query.chars().count();
            self.fire_web_search(&q);
        }
        self.mode = UiMode::WebSearch;
        self.status = "?web search".to_string();
    }

    fn fire_web_search(&mut self, query: &str) {
        let q = query.trim().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        self.web_search.pending = true;
        self.web_search.error = None;
        self.web_search.results.clear();
        self.web_search.answer = None;
        self.web_search.summary = None;
        self.web_search.answer_card = None;
        self.web_search.selected = 0;
        self.web_search.rx = Some(rx);
        std::thread::spawn(move || {
            let config = app_core::config::load_web_search_config();
            let result = app_core::web_search::search_web(&q, &config);
            let _ = tx.send(WebSearchResponse { result });
        });
    }

    pub(super) fn poll_web_search(&mut self) {
        let response = match self.web_search.rx.as_ref() {
            Some(rx) => match rx.try_recv() {
                Ok(resp) => resp.result,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Err("web search worker stopped before returning a result".to_string())
                }
            },
            None => return,
        };

        match response {
            Ok(search_result) => {
                self.web_search.answer = search_result.answer;
                self.web_search.summary = search_result.summary;
                self.web_search.answer_card = search_result.answer_card;
                self.web_search.results = search_result.items;
                self.web_search.pending = false;
                self.web_search.error = None;
                self.status = format!(
                    "?{} ({} results)",
                    self.web_search.query,
                    self.web_search.results.len()
                );
            }
            Err(e) => {
                self.web_search.pending = false;
                self.web_search.error = Some(e.clone());
                self.status = format!("?web search error: {e}");
            }
        }
        self.web_search.rx = None;
        self.render_state.dirty = true;
    }

    pub(super) fn handle_web_search_key(&mut self, key: Key) {
        match key {
            Key::Esc => {
                self.web_search = WebSearchState::default();
                self.mode = if self.vim_enabled {
                    UiMode::Normal
                } else {
                    UiMode::Editor
                };
                self.status.clear();
            }
            Key::Enter => {
                // Submit query if no results yet, or open selected link in browser
                if self.web_search.pending {
                    return;
                }
                if self.web_search.results.is_empty()
                    && self.web_search.answer.is_none()
                    && self.web_search.summary.is_none()
                    && self.web_search.answer_card.is_none()
                    && !self.web_search.query.trim().is_empty()
                {
                    let query = self.web_search.query.clone();
                    self.fire_web_search(&query);
                } else if let Some(item) = self.web_search.results.get(self.web_search.selected) {
                    let url = item.url.clone();
                    self.status = match open_browser_url(&url) {
                        Ok(()) => "web result opened".to_string(),
                        Err(err) => format!("web result open failed: {err}"),
                    };
                }
            }
            Key::ShiftEnter => {
                // Insert markdown link at cursor
                if let Some(item) = self.web_search.results.get(self.web_search.selected) {
                    let link = item.markdown_link.clone();
                    self.web_search = WebSearchState::default();
                    self.mode = if self.vim_enabled {
                        UiMode::Normal
                    } else {
                        UiMode::Editor
                    };
                    self.status.clear();
                    self.insert_text(&link);
                }
            }
            Key::ArrowUp => {
                if self.web_search.selected > 0 {
                    self.web_search.selected -= 1;
                }
            }
            Key::ArrowDown => {
                if !self.web_search.results.is_empty() {
                    self.web_search.selected =
                        (self.web_search.selected + 1).min(self.web_search.results.len() - 1);
                }
            }
            Key::ArrowLeft => {
                self.web_search.cursor_col = self.web_search.cursor_col.saturating_sub(1);
            }
            Key::ArrowRight => {
                self.web_search.cursor_col =
                    (self.web_search.cursor_col + 1).min(char_len(&self.web_search.query));
            }
            Key::Home => {
                self.web_search.cursor_col = 0;
            }
            Key::End => {
                self.web_search.cursor_col = char_len(&self.web_search.query);
            }
            Key::Ctrl('w') | Key::CtrlBackspace => {
                if self.web_search.cursor_col == char_len(&self.web_search.query) {
                    let old_len = self.web_search.query.len();
                    trim_trailing_word(&mut self.web_search.query);
                    if self.web_search.query.len() != old_len {
                        self.invalidate_web_search_results();
                        self.web_search.cursor_col = char_len(&self.web_search.query);
                    }
                } else if remove_char_before_char_col(
                    &mut self.web_search.query,
                    self.web_search.cursor_col,
                ) {
                    self.invalidate_web_search_results();
                    self.web_search.cursor_col = self.web_search.cursor_col.saturating_sub(1);
                }
            }
            Key::Backspace => {
                if remove_char_before_char_col(
                    &mut self.web_search.query,
                    self.web_search.cursor_col,
                ) {
                    self.invalidate_web_search_results();
                    self.web_search.cursor_col = self.web_search.cursor_col.saturating_sub(1);
                }
            }
            Key::Delete => {
                if remove_char_at_char_col(&mut self.web_search.query, self.web_search.cursor_col) {
                    self.invalidate_web_search_results();
                }
            }
            Key::Char(ch) => {
                self.invalidate_web_search_results();
                insert_str_at_char_col(
                    &mut self.web_search.query,
                    self.web_search.cursor_col,
                    &ch.to_string(),
                );
                self.web_search.cursor_col += 1;
            }
            Key::Paste(text) => {
                let sanitized = text
                    .chars()
                    .filter(|ch| *ch != '\n' && *ch != '\r')
                    .collect::<String>();
                if !sanitized.is_empty() {
                    self.invalidate_web_search_results();
                    let added = char_len(&sanitized);
                    insert_str_at_char_col(
                        &mut self.web_search.query,
                        self.web_search.cursor_col,
                        &sanitized,
                    );
                    self.web_search.cursor_col += added;
                }
            }
            _ => {}
        }
    }

    fn invalidate_web_search_results(&mut self) {
        self.web_search.results.clear();
        self.web_search.answer = None;
        self.web_search.summary = None;
        self.web_search.answer_card = None;
        self.web_search.selected = 0;
        self.web_search.pending = false;
        self.web_search.error = None;
        self.web_search.rx = None;
    }
}

fn open_browser_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|err| format!("invalid URL: {err}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("only HTTP(S) URLs can be opened".to_string());
    }
    crate::terminal::external_open::open_with_default_app(parsed.as_str().as_ref())
}
