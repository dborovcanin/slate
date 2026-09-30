use super::{new_note_with_context, Db, Key, TerminalApp, UiMode};
use crate::terminal::browser::{
    BrowserState, BrowserView, ClipOp, Clipboard, CollectionEntry, Confirm, Level, MembershipUndo,
    NoteEntry, Preview, Prompt, PromptKind, Scope, SortKey, PREVIEW_BODY_CHARS,
};
use crate::terminal::canvas::{contrast_fg_for_bg, TextStyle};
use crate::terminal::icons::Icons;
use crate::terminal::input;
use crate::terminal::session::CursorPlacement;
use crate::terminal::text_input;
use app_core::note_sources::{NoteSourceService, SaveOptions};
use app_core::storage::NoteAccessMode;
use ratatui::buffer::Buffer;
use std::time::{SystemTime, UNIX_EPOCH};

/// Preview lines kept for the hovered note; more than any screen shows.
const PREVIEW_MAX_LINES: usize = 300;

const COLLECTIONS_HINT: &str =
    "l open  a new  r rename  D delete  w working  p paste  / filter  q close";
const NOTES_HINT: &str = "l open  space mark  y copy  x cut  p paste  d remove  D delete  a new  r rename  s sort  u undo  / filter  h back";

// Ownership: the collection browser's key handling and database work. View
// state and drawing live in `terminal::browser`.
impl TerminalApp {
    pub(super) fn open_browser(&mut self, db: &Db) -> Result<(), String> {
        self.dismiss_variable_autocomplete_popup();
        if self.autosave_enabled {
            self.save(db)?;
        }
        self.browser_return_mode = if matches!(self.mode, UiMode::Editor) {
            UiMode::Editor
        } else {
            UiMode::Normal
        };
        // The sort order and a pending yank survive closing the browser.
        self.browser = BrowserState {
            sort: self.browser.sort,
            clipboard: self.browser.clipboard.take(),
            ..Default::default()
        };
        self.browser_reload_collections(db)?;
        let scope = self
            .working_collection_id
            .clone()
            .map(Scope::Collection)
            .unwrap_or(Scope::All);
        let active = self.active_note.id.clone();
        self.browser_enter_scope(db, scope, Some(&active))?;
        self.mode = UiMode::Browser;
        Ok(())
    }

    fn close_browser(&mut self) {
        self.browser.prompt = None;
        self.browser.confirm = None;
        self.browser.scope_cache.clear();
        self.browser.preview = Preview::Empty;
        self.mode = self.browser_return_mode;
        self.status = if self.mode == UiMode::Normal {
            "-- NORMAL --".to_string()
        } else {
            format!("editing {}", self.active_note.id)
        };
    }

    pub(super) fn browser_status_line(&self) -> String {
        if let Some(message) = self.browser.message.as_ref() {
            return message.clone();
        }
        match self.browser.level() {
            Level::Collections => COLLECTIONS_HINT.to_string(),
            Level::Notes => NOTES_HINT.to_string(),
        }
    }

    fn browser_message(&mut self, message: impl Into<String>) {
        self.browser.message = Some(message.into());
    }

    fn browser_reload_collections(&mut self, db: &Db) -> Result<(), String> {
        let counts = db.collection_note_counts()?;
        let mut entries = vec![
            CollectionEntry {
                scope: Scope::All,
                name: "All notes".to_string(),
                description: "Every note".to_string(),
                count: counts.total,
            },
            CollectionEntry {
                scope: Scope::Unsorted,
                name: "Unsorted".to_string(),
                description: "Notes in no collection".to_string(),
                count: counts.unsorted,
            },
        ];
        entries.extend(db.list_collections()?.into_iter().map(|collection| {
            let count = counts
                .per_collection
                .get(&collection.id)
                .copied()
                .unwrap_or(0);
            CollectionEntry {
                scope: Scope::Collection(collection.id),
                name: collection.name,
                description: collection.description,
                count,
            }
        }));
        self.browser.collections = entries;
        self.browser.recompute_collection_matches();
        self.browser.scope_cache.clear();
        Ok(())
    }

    fn browser_load_scope(&self, db: &Db, scope: &Scope) -> Result<Vec<NoteEntry>, String> {
        let summaries = match scope {
            Scope::All => db.list_notes_meta_filtered(None)?,
            Scope::Unsorted => db.list_notes_meta_unsorted()?,
            Scope::Collection(id) => db.list_notes_meta_filtered(Some(id))?,
        };
        let prefix = self.daily_config.note_prefix.as_str();
        let mut notes: Vec<NoteEntry> = summaries
            .into_iter()
            .map(|summary| NoteEntry::from_summary(summary, prefix))
            .collect();
        BrowserState::sort_notes(&mut notes, self.browser.sort);
        Ok(notes)
    }

    fn browser_enter_scope(
        &mut self,
        db: &Db,
        scope: Scope,
        focus_note: Option<&str>,
    ) -> Result<(), String> {
        let notes = self.browser_load_scope(db, &scope)?;
        let state = &mut self.browser;
        state.notes = notes;
        state.scope = Some(scope);
        state.level = Some(Level::Notes);
        state.note_filter.clear();
        state.marked.clear();
        state.note_matches.clear();
        state.note_cursor = 0;
        state.recompute_note_matches();
        if let Some(pos) = focus_note.and_then(|id| state.note_position(id)) {
            state.note_cursor = pos;
        }
        state.preview = Preview::Empty;
        self.browser_refresh_preview(db);
        Ok(())
    }

    fn browser_leave_scope(&mut self, db: &Db) {
        let state = &mut self.browser;
        if state.level() != Level::Notes {
            return;
        }
        state.level = Some(Level::Collections);
        state.marked.clear();
        if let Some(scope) = state.scope.clone() {
            let find = |state: &BrowserState| {
                state
                    .collection_matches
                    .iter()
                    .position(|idx| state.collections[*idx].scope == scope)
            };
            if find(state).is_none() {
                state.collection_filter.clear();
                state.recompute_collection_matches();
            }
            state.collection_cursor = find(state).unwrap_or(0);
        }
        state.preview = Preview::Empty;
        self.browser_refresh_preview(db);
    }

    /// Reloads counts and the open notes list after a change, keeping the
    /// filter and hovering `focus` (or the note hovered before).
    fn browser_reload(&mut self, db: &Db, focus: Option<String>) -> Result<(), String> {
        let focus = focus.or_else(|| self.browser.hovered_note().map(|note| note.id.clone()));
        self.browser_reload_collections(db)?;
        // Titles and note ids feed the switcher and wiki-link completion.
        self.refresh_switcher_items(db)?;
        if let (Level::Notes, Some(scope)) = (self.browser.level(), self.browser.scope.clone()) {
            let notes = self.browser_load_scope(db, &scope)?;
            let state = &mut self.browser;
            state.notes = notes;
            state
                .marked
                .retain(|id| state.notes.iter().any(|note| &note.id == id));
            let cursor = state.note_cursor;
            state.note_matches.clear();
            state.recompute_note_matches();
            state.note_cursor = focus
                .as_deref()
                .and_then(|id| state.note_position(id))
                .unwrap_or_else(|| cursor.min(state.note_matches.len().saturating_sub(1)));
        }
        self.browser.preview = Preview::Empty;
        self.browser_refresh_preview(db);
        Ok(())
    }

    fn browser_refresh_preview(&mut self, db: &Db) {
        match self.browser.level() {
            Level::Collections => {
                let Some(entry) = self.browser.hovered_collection().cloned() else {
                    self.browser.preview = Preview::Empty;
                    return;
                };
                let notes = match self.browser.scope_cache.get(&entry.scope) {
                    Some(notes) => notes.clone(),
                    None => {
                        let notes = std::sync::Arc::new(
                            self.browser_load_scope(db, &entry.scope)
                                .unwrap_or_default(),
                        );
                        self.browser
                            .scope_cache
                            .insert(entry.scope.clone(), notes.clone());
                        notes
                    }
                };
                self.browser.preview = Preview::Notes {
                    description: entry.description,
                    notes,
                };
            }
            Level::Notes => {
                let Some(note) = self.browser.hovered_note().cloned() else {
                    self.browser.preview = Preview::Empty;
                    return;
                };
                if matches!(&self.browser.preview, Preview::Note { note_id, .. } if *note_id == note.id)
                {
                    return;
                }
                let (lines, locked) = if note.id == self.active_note.id {
                    let lines = self
                        .editor
                        .lines
                        .iter()
                        .take(PREVIEW_MAX_LINES)
                        .cloned()
                        .collect();
                    (lines, !self.active_note_is_editable())
                } else {
                    let body = db
                        .get_note_body_head(&note.id, PREVIEW_BODY_CHARS)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    if body == "[locked]" {
                        (Vec::new(), true)
                    } else {
                        let lines = body
                            .lines()
                            .take(PREVIEW_MAX_LINES)
                            .map(str::to_string)
                            .collect();
                        (lines, false)
                    }
                };
                let collections = db
                    .get_note_collection_ids(&note.id)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|id| self.browser.collection_name(id).map(str::to_string))
                    .collect();
                let tags = db.list_note_tags(&note.id).unwrap_or_default();
                self.browser.preview = Preview::Note {
                    note_id: note.id,
                    lines,
                    collections,
                    tags,
                    locked,
                };
            }
        }
    }

    pub(super) fn handle_browser_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        self.browser.message = None;
        if key == Key::Ctrl('q') {
            self.quit = true;
            return Ok(());
        }
        if self.browser.confirm.is_some() {
            return self.handle_browser_confirm_key(db, key);
        }
        if self.browser.prompt.is_some() {
            return self.handle_browser_prompt_key(db, key);
        }
        let pending_g = std::mem::take(&mut self.browser.pending_g);
        let page = (input::terminal_size().0.saturating_sub(2) / 2).max(1) as isize;
        let level = self.browser.level();
        match key {
            Key::Char('j') | Key::ArrowDown => self.browser.move_cursor(1),
            Key::Char('k') | Key::ArrowUp => self.browser.move_cursor(-1),
            Key::Ctrl('d') | Key::PageDown => self.browser.move_cursor(page),
            Key::Ctrl('u') | Key::PageUp => self.browser.move_cursor(-page),
            Key::Char('g') if pending_g => self.browser.move_to_end(false),
            Key::Char('g') => self.browser.pending_g = true,
            Key::Home => self.browser.move_to_end(false),
            Key::Char('G') | Key::End => self.browser.move_to_end(true),
            Key::Char('l') | Key::Char('o') | Key::ArrowRight | Key::Enter => {
                return self.browser_open_hovered(db);
            }
            Key::Char('h') | Key::Char('-') | Key::ArrowLeft | Key::Backspace => {
                self.browser_leave_scope(db);
            }
            Key::Esc => {
                if self.browser.has_filter() {
                    self.browser.set_active_filter(String::new());
                } else if !self.browser.marked.is_empty() {
                    self.browser.marked.clear();
                } else {
                    self.close_browser();
                    return Ok(());
                }
            }
            Key::Char('q') | Key::Ctrl('b') => {
                self.close_browser();
                return Ok(());
            }
            Key::Ctrl('p') => {
                self.close_browser();
                return self.open_switcher(db);
            }
            Key::Ctrl('g') => {
                self.close_browser();
                return self.open_collection_switcher(db);
            }
            Key::Char('/') => {
                let text = match level {
                    Level::Collections => self.browser.collection_filter.clone(),
                    Level::Notes => self.browser.note_filter.clone(),
                };
                self.browser_open_prompt(PromptKind::Filter, text);
            }
            Key::Char('a') => match level {
                Level::Collections => self.browser_open_prompt(PromptKind::NewCollection, ""),
                Level::Notes => self.browser_open_prompt(PromptKind::NewNote, ""),
            },
            Key::Char('r') => self.browser_request_rename(),
            Key::Char(' ') if level == Level::Notes => {
                self.browser.toggle_mark_hovered();
                self.browser.move_cursor(1);
            }
            Key::Ctrl('a') if level == Level::Notes => self.browser.mark_all_visible(),
            Key::Char('y') => self.browser_yank(ClipOp::Copy),
            Key::Char('x') => self.browser_yank(ClipOp::Cut),
            Key::Char('p') => self.browser_paste(db)?,
            Key::Char('d') => self.browser_remove_from_collection(db)?,
            Key::Char('D') | Key::Delete => self.browser_request_delete(),
            Key::Char('u') => self.browser_undo(db)?,
            Key::Char('s') if level == Level::Notes => self.browser_toggle_sort(db),
            Key::Char('w') => self.browser_set_working_collection(),
            Key::Char('R') => {
                self.browser_reload(db, None)?;
                self.browser_message("reloaded");
            }
            _ => {}
        }
        self.browser_refresh_preview(db);
        Ok(())
    }

    fn browser_open_prompt(&mut self, kind: PromptKind, text: impl Into<String>) {
        self.browser.prompt = Some(Prompt {
            kind,
            text: text.into(),
            cursor: usize::MAX,
        });
    }

    fn browser_open_hovered(&mut self, db: &Db) -> Result<(), String> {
        match self.browser.level() {
            Level::Collections => {
                if let Some(scope) = self.browser.hovered_collection().map(|e| e.scope.clone()) {
                    self.browser_enter_scope(db, scope, None)?;
                }
            }
            Level::Notes => {
                let Some(note) = self.browser.hovered_note().cloned() else {
                    return Ok(());
                };
                if note.is_locked() {
                    self.browser_open_prompt(
                        PromptKind::Unlock {
                            note_id: note.id,
                            title: note.title,
                        },
                        "",
                    );
                    return Ok(());
                }
                self.close_browser();
                self.open_note_from_switcher(db, &note.id, None, None)?;
            }
        }
        Ok(())
    }

    fn handle_browser_prompt_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        let Some(mut prompt) = self.browser.prompt.take() else {
            return Ok(());
        };
        match key {
            Key::Esc => {
                if prompt.kind == PromptKind::Filter {
                    self.browser.set_active_filter(String::new());
                    self.browser_refresh_preview(db);
                }
                return Ok(());
            }
            Key::Enter => return self.browser_submit_prompt(db, prompt),
            other => {
                text_input::apply_key(&mut prompt.text, &mut prompt.cursor, &other);
                if prompt.kind == PromptKind::Filter {
                    self.browser.set_active_filter(prompt.text.clone());
                    self.browser_refresh_preview(db);
                }
            }
        }
        self.browser.prompt = Some(prompt);
        Ok(())
    }

    fn browser_submit_prompt(&mut self, db: &Db, prompt: Prompt) -> Result<(), String> {
        let text = prompt.text.trim().to_string();
        if text.is_empty() && !matches!(prompt.kind, PromptKind::Filter) {
            self.browser_message("nothing entered");
            return Ok(());
        }
        let outcome = match prompt.kind {
            PromptKind::Filter => Ok(()),
            PromptKind::NewNote => self.browser_create_note(db, &text),
            PromptKind::NewCollection => self.browser_create_collection(db, &text),
            PromptKind::RenameNote { note_id } => self.browser_rename_note(db, &note_id, &text),
            PromptKind::RenameCollection { collection_id } => {
                self.browser_rename_collection(db, &collection_id, &text)
            }
            PromptKind::Unlock { note_id, title } => {
                // The password is used as typed, spaces included.
                match self.open_note_from_switcher(db, &note_id, Some(&prompt.text), None) {
                    Ok(()) => {
                        self.status = format!("unlocked {title}");
                        Ok(())
                    }
                    Err(error) => {
                        self.browser_open_prompt(PromptKind::Unlock { note_id, title }, "");
                        Err(format!("unlock failed: {error}"))
                    }
                }
            }
        };
        if let Err(error) = outcome {
            self.browser_message(error);
        }
        Ok(())
    }

    fn browser_create_note(&mut self, db: &Db, title: &str) -> Result<(), String> {
        let collection_id = self
            .browser
            .scope
            .as_ref()
            .and_then(Scope::collection_id)
            .map(str::to_string);
        let note = new_note_with_context(db, &self.note_creation_theme, collection_id.as_deref())?;
        NoteSourceService::new(db.clone()).save_note_revision_by_id(
            &note.id,
            &format!("# {title}"),
            SaveOptions::default(),
        )?;
        self.browser_reload(db, Some(note.id))?;
        self.browser_message(format!("created {title} · Enter to open"));
        Ok(())
    }

    fn browser_create_collection(&mut self, db: &Db, name: &str) -> Result<(), String> {
        let collection = db.create_collection(name, "")?;
        self.browser_reload_collections(db)?;
        let scope = Scope::Collection(collection.id);
        if let Some(pos) = self
            .browser
            .collection_matches
            .iter()
            .position(|idx| self.browser.collections[*idx].scope == scope)
        {
            self.browser.collection_cursor = pos;
        }
        self.browser_refresh_preview(db);
        self.browser_message(format!("created collection {}", collection.name));
        Ok(())
    }

    fn browser_request_rename(&mut self) {
        match self.browser.level() {
            Level::Collections => match self.browser.hovered_collection() {
                Some(CollectionEntry {
                    scope: Scope::Collection(id),
                    name,
                    ..
                }) => {
                    let kind = PromptKind::RenameCollection {
                        collection_id: id.clone(),
                    };
                    let name = name.clone();
                    self.browser_open_prompt(kind, name);
                }
                _ => self.browser_message("built-in groups cannot be renamed"),
            },
            Level::Notes => {
                let Some(note) = self.browser.hovered_note().cloned() else {
                    return;
                };
                if note.is_locked() {
                    self.browser_message("unlock the note before renaming it");
                    return;
                }
                if crate::file_path_from_note_id(&note.id).is_some() {
                    self.browser_message("file-backed notes are renamed in the editor");
                    return;
                }
                self.browser_open_prompt(PromptKind::RenameNote { note_id: note.id }, note.title);
            }
        }
    }

    fn browser_rename_note(&mut self, db: &Db, note_id: &str, title: &str) -> Result<(), String> {
        if note_id == self.active_note.id {
            // The open note is renamed in its buffer so the editor and the
            // next autosave agree on the text.
            self.rename_active_note_title(title);
            self.save(db)?;
        } else {
            NoteSourceService::new(db.clone()).rename_note(note_id, title)?;
        }
        self.browser_reload(db, Some(note_id.to_string()))?;
        self.browser_message(format!("renamed to {title}"));
        Ok(())
    }

    fn rename_active_note_title(&mut self, title: &str) {
        let lines = &self.editor.lines;
        let change =
            match app_core::note_sources::title_line_index(lines.iter().map(String::as_str)) {
                Some(idx) => {
                    let from = self.byte_offset_for_line_col(idx, 0);
                    crate::editor_core::types::TextChange {
                        from,
                        to: from + lines[idx].len(),
                        insert: app_core::note_sources::retitle_line(&lines[idx], title),
                    }
                }
                None => crate::editor_core::types::TextChange {
                    from: 0,
                    to: 0,
                    insert: format!("# {}\n", title.trim()),
                },
            };
        self.apply_edit_operation(&crate::editor_core::types::EditOperation {
            changes: vec![change],
            selection: None,
        });
    }

    fn browser_rename_collection(
        &mut self,
        db: &Db,
        collection_id: &str,
        name: &str,
    ) -> Result<(), String> {
        let collection = db.rename_collection(collection_id, name)?;
        if self.working_collection_id.as_deref() == Some(collection_id) {
            self.working_collection_name = Some(collection.name.clone());
        }
        self.browser_reload_collections(db)?;
        self.browser_message(format!("renamed collection to {}", collection.name));
        Ok(())
    }

    fn browser_yank(&mut self, op: ClipOp) {
        if self.browser.level() != Level::Notes {
            self.browser_message("open a collection to copy or cut notes");
            return;
        }
        let note_ids = self.browser.selected_note_ids();
        let Some(source) = self.browser.scope.clone() else {
            return;
        };
        if note_ids.is_empty() {
            return;
        }
        let verb = match op {
            ClipOp::Copy => "copied",
            ClipOp::Cut => "cut",
        };
        self.browser_message(format!(
            "{verb} {} · p pastes into a collection",
            plural(note_ids.len(), "note")
        ));
        self.browser.clipboard = Some(Clipboard {
            op,
            note_ids,
            source,
        });
        self.browser.marked.clear();
    }

    fn browser_paste(&mut self, db: &Db) -> Result<(), String> {
        let Some(clip) = self.browser.clipboard.clone() else {
            self.browser_message("nothing to paste · y copies, x cuts");
            return Ok(());
        };
        let target = match self.browser.level() {
            Level::Notes => self.browser.scope.clone(),
            Level::Collections => self.browser.hovered_collection().map(|e| e.scope.clone()),
        };
        let Some(Scope::Collection(target_id)) = target else {
            self.browser_message("paste into a collection");
            return Ok(());
        };
        // Only notes that were not members yet are recorded, so undo never
        // removes a membership that existed before the paste.
        let new_members: Vec<String> = clip
            .note_ids
            .iter()
            .filter(|id| {
                !db.get_note_collection_ids(id)
                    .unwrap_or_default()
                    .contains(&target_id)
            })
            .cloned()
            .collect();
        db.add_notes_to_collection(&target_id, &new_members)?;
        let mut undo = MembershipUndo {
            added: vec![(target_id.clone(), new_members)],
            ..Default::default()
        };
        let target_name = self
            .browser
            .scope_name(&Scope::Collection(target_id.clone()));
        let verb = match (&clip.op, &clip.source) {
            (ClipOp::Cut, Scope::Collection(source_id)) if *source_id != target_id => {
                db.remove_notes_from_collection(source_id, &clip.note_ids)?;
                undo.removed = vec![(source_id.clone(), clip.note_ids.clone())];
                "moved"
            }
            _ => "added",
        };
        let label = format!(
            "{verb} {} to {target_name}",
            plural(clip.note_ids.len(), "note")
        );
        undo.label = label.clone();
        self.browser.undo = Some(undo);
        if clip.op == ClipOp::Cut {
            self.browser.clipboard = None;
        }
        self.browser_reload(db, None)?;
        self.browser_message(format!("{label} · u undo"));
        Ok(())
    }

    fn browser_remove_from_collection(&mut self, db: &Db) -> Result<(), String> {
        if self.browser.level() != Level::Notes {
            self.browser_message("D deletes a collection");
            return Ok(());
        }
        let Some(Scope::Collection(collection_id)) = self.browser.scope.clone() else {
            self.browser_message("not a collection · D deletes notes");
            return Ok(());
        };
        let note_ids = self.browser.selected_note_ids();
        if note_ids.is_empty() {
            return Ok(());
        }
        let removed = db.remove_notes_from_collection(&collection_id, &note_ids)?;
        let name = self
            .browser
            .collection_name(&collection_id)
            .unwrap_or("collection")
            .to_string();
        let label = format!("removed {} from {name}", plural(removed, "note"));
        self.browser.undo = Some(MembershipUndo {
            removed: vec![(collection_id, note_ids)],
            label: label.clone(),
            ..Default::default()
        });
        self.browser.marked.clear();
        self.browser_reload(db, None)?;
        self.browser_message(format!("{label} · u undo"));
        Ok(())
    }

    fn browser_undo(&mut self, db: &Db) -> Result<(), String> {
        let Some(undo) = self.browser.undo.take() else {
            self.browser_message("nothing to undo");
            return Ok(());
        };
        for (collection_id, note_ids) in &undo.added {
            db.remove_notes_from_collection(collection_id, note_ids)?;
        }
        for (collection_id, note_ids) in &undo.removed {
            db.add_notes_to_collection(collection_id, note_ids)?;
        }
        self.browser_reload(db, None)?;
        self.browser_message(format!("undid: {}", undo.label));
        Ok(())
    }

    fn browser_request_delete(&mut self) {
        match self.browser.level() {
            Level::Collections => match self.browser.hovered_collection() {
                Some(CollectionEntry {
                    scope: Scope::Collection(id),
                    name,
                    ..
                }) => {
                    self.browser.confirm = Some(Confirm::DeleteCollection {
                        collection_id: id.clone(),
                        name: name.clone(),
                    });
                }
                _ => self.browser_message("built-in groups cannot be deleted"),
            },
            Level::Notes => {
                let note_ids = self.browser.selected_note_ids();
                let protected = self.browser.notes.iter().any(|note| {
                    note_ids.contains(&note.id) && note.access_mode != NoteAccessMode::None
                });
                if protected {
                    self.browser_message(
                        "protected notes need a password: delete them from Ctrl+P",
                    );
                    return;
                }
                let Some(first) = note_ids.first() else {
                    return;
                };
                let label = self
                    .browser
                    .notes
                    .iter()
                    .find(|note| &note.id == first)
                    .map(|note| note.title.clone())
                    .unwrap_or_default();
                self.browser.confirm = Some(Confirm::DeleteNotes { note_ids, label });
            }
        }
    }

    fn handle_browser_confirm_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        match key {
            Key::Char('y' | 'Y') | Key::Enter => {}
            Key::Char('n' | 'N') | Key::Esc => {
                self.browser.confirm = None;
                return Ok(());
            }
            _ => return Ok(()),
        }
        let Some(confirm) = self.browser.confirm.take() else {
            return Ok(());
        };
        match confirm {
            Confirm::DeleteNotes { note_ids, label } => {
                let mut deleted = 0usize;
                for note_id in &note_ids {
                    match self.delete_note_from_switcher(db, note_id, &label, None) {
                        Ok(()) => deleted += 1,
                        Err(error) => {
                            self.browser_message(format!("delete failed: {error}"));
                            break;
                        }
                    }
                }
                if let Some(clip) = self.browser.clipboard.as_mut() {
                    clip.note_ids.retain(|id| !note_ids.contains(id));
                }
                self.browser.undo = None;
                self.browser.marked.clear();
                self.browser_reload(db, None)?;
                if deleted == note_ids.len() {
                    self.browser_message(if deleted == 1 {
                        format!("deleted {label}")
                    } else {
                        format!("deleted {}", plural(deleted, "note"))
                    });
                }
            }
            Confirm::DeleteCollection {
                collection_id,
                name,
            } => {
                db.delete_collection(&collection_id)?;
                if self.working_collection_id.as_deref() == Some(collection_id.as_str()) {
                    self.working_collection_id = None;
                    self.working_collection_name = None;
                }
                self.browser.undo = None;
                self.browser_reload_collections(db)?;
                self.browser.move_cursor(0);
                self.browser.preview = Preview::Empty;
                self.browser_refresh_preview(db);
                self.browser_message(format!("deleted collection {name}; its notes stay"));
            }
        }
        Ok(())
    }

    fn browser_toggle_sort(&mut self, db: &Db) {
        self.browser.sort = match self.browser.sort {
            SortKey::Modified => SortKey::Title,
            SortKey::Title => SortKey::Modified,
        };
        let hovered = self.browser.hovered_note().map(|note| note.id.clone());
        BrowserState::sort_notes(&mut self.browser.notes, self.browser.sort);
        self.browser.scope_cache.clear();
        self.browser.note_matches.clear();
        self.browser.recompute_note_matches();
        if let Some(pos) = hovered.and_then(|id| self.browser.note_position(&id)) {
            self.browser.note_cursor = pos;
        }
        self.browser_refresh_preview(db);
        self.browser_message(format!("sorted by {}", self.browser.sort.label()));
    }

    fn browser_set_working_collection(&mut self) {
        let scope = match self.browser.level() {
            Level::Collections => self.browser.hovered_collection().map(|e| e.scope.clone()),
            Level::Notes => self.browser.scope.clone(),
        };
        match scope {
            Some(Scope::Collection(id)) => {
                let name = self.browser.scope_name(&Scope::Collection(id.clone()));
                self.working_collection_id = Some(id);
                self.working_collection_name = Some(name.clone());
                self.browser_message(format!("working collection: {name} · new notes go here"));
            }
            Some(Scope::All) => {
                self.working_collection_id = None;
                self.working_collection_name = None;
                self.browser_message("working collection cleared");
            }
            _ => self.browser_message("pick a collection to work in"),
        }
    }

    /// Paints the browser screen and its status row.
    pub(super) fn render_browser_screen(
        &mut self,
        buf: &mut Buffer,
        rows: usize,
        cols: usize,
    ) -> CursorPlacement {
        let now_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let cursor = crate::terminal::browser::draw_browser(
            &BrowserView {
                state: &self.browser,
                palette: self.render_palette,
                icons: Icons::for_style(self.note_creation_theme.icons),
                working_collection_id: self.working_collection_id.as_deref(),
                active_note_id: &self.active_note.id,
                now_epoch,
            },
            buf,
            rows,
            cols,
        );
        let status_bg = self.render_palette.primary();
        let status_style = TextStyle {
            fg: Some(contrast_fg_for_bg(status_bg)),
            bg: Some(status_bg),
            ..Default::default()
        };
        let status = format!(" {}", self.browser_status_line());
        let sticky = self.working_collection_status_suffix();
        self.draw_status_row_with_right_sticky(buf, rows, cols, &status, &sticky, status_style);
        let (row, col) = cursor.unwrap_or((rows, cols));
        CursorPlacement {
            row: u16::try_from(row.saturating_sub(1)).unwrap_or(u16::MAX),
            col: u16::try_from(col.saturating_sub(1)).unwrap_or(u16::MAX),
            block: false,
            visible: cursor.is_some(),
        }
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}
