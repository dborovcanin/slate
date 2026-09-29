use super::{
    is_markdown_table_line, new_note_with_context, DatePickerAction, Db, Key, LineReminderGhost,
    TerminalApp, UiMode, VimPipelineResult, FOLD_PREFIX_TIMEOUT_MS,
};
use crate::terminal::date_picker;
use crate::terminal::text_utils::line_char_len;
use std::time::{Duration, Instant};

// Ownership: key dispatch and per-mode key handling entry points.
impl TerminalApp {
    fn maybe_record_vim_insert_macro_key(&mut self, key: &Key) {
        if self.vim_macro_replaying {
            return;
        }
        let Some(register) = self.vim_macro_recording else {
            return;
        };
        let recorded = match key {
            Key::Esc => Some(crate::editor_core::vim::VimKey::Esc),
            Key::Enter => Some(crate::editor_core::vim::VimKey::Enter),
            Key::Tab => Some(crate::editor_core::vim::VimKey::Tab),
            Key::Backspace => Some(crate::editor_core::vim::VimKey::Backspace),
            Key::Delete => Some(crate::editor_core::vim::VimKey::Delete),
            Key::ArrowUp => Some(crate::editor_core::vim::VimKey::ArrowUp),
            Key::ArrowDown => Some(crate::editor_core::vim::VimKey::ArrowDown),
            Key::ArrowLeft => Some(crate::editor_core::vim::VimKey::ArrowLeft),
            Key::ArrowRight => Some(crate::editor_core::vim::VimKey::ArrowRight),
            Key::Char(ch) => Some(crate::editor_core::vim::VimKey::Char(*ch)),
            _ => None,
        };
        if let Some(vim_key) = recorded {
            self.vim_macro_registers
                .entry(register)
                .or_default()
                .push(super::VimMacroStep::InsertKey(vim_key));
        }
    }

    pub(super) fn handle_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        if self.image_preview.is_some() {
            match key {
                Key::Esc => self.image_preview = None,
                Key::Char('o') | Key::Char('O') => {
                    self.image_preview = None;
                    self.open_image_externally_at_cursor(db);
                }
                Key::Ctrl('q') => self.quit = true,
                _ => {}
            }
            return Ok(());
        }
        if self.mode != UiMode::Normal {
            self.folds.pending_prefix_until = None;
        }
        match self.mode {
            UiMode::DatePicker => self.handle_date_picker_key(db, key)?,
            UiMode::Editor => self.handle_editor_key(db, key)?,
            UiMode::Normal => self.handle_normal_key(db, key)?,
            UiMode::Visual | UiMode::VisualLine => self.handle_visual_key(db, key)?,
            UiMode::Switcher => self.handle_switcher_key(db, key)?,
            UiMode::CollectionSwitcher => self.handle_collection_switcher_key(db, key)?,
            UiMode::ContentSearch => self.handle_content_search_key(db, key)?,
            UiMode::CommandBar => self.handle_command_bar_key(db, key)?,
            UiMode::Search => self.handle_search_key(key)?,
            UiMode::WebSearch => self.handle_web_search_key(key),
        }
        Ok(())
    }

    pub(super) fn handle_editor_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        if self.wiki_link_preview.visible {
            self.close_wiki_link_preview();
        }
        self.maybe_record_vim_insert_macro_key(&key);
        let mut should_autoformat = false;
        let mut clamp_table_padding = true;
        // Set when a typed `|` closes a table row at the end of the line: the
        // cursor stays after the pipe so the next cell can be typed.
        let mut cursor_after_row_end = false;
        let mut moved_cursor = false;
        let mut preserve_table_column = false;
        let mut refresh_variable_popup = false;
        if !self.active_note_is_editable() && Self::editor_key_may_edit_note(&key) {
            self.set_locked_note_status();
            self.dismiss_variable_autocomplete_popup();
            return Ok(());
        }
        match key {
            Key::Ctrl('q') => {
                self.quit = true;
                return Ok(());
            }
            Key::Ctrl('o') if !self.vim_enabled => {
                self.preview_image_at_cursor(db);
                return Ok(());
            }
            Key::Ctrl('w') => {
                if self.try_table_header_delete_column_rule() {
                    should_autoformat = false;
                    clamp_table_padding = false;
                } else if let Some(changed) = self.try_table_boundary_edit_rule(true, true) {
                    if changed {
                        should_autoformat = true;
                    } else {
                        should_autoformat = self.delete_word_backward();
                    }
                } else {
                    should_autoformat = self.delete_word_backward();
                }
            }
            Key::Ctrl('s') => {
                if self.autosave_enabled {
                    self.save(db)?;
                    self.status = format!("saved {}", self.active_note.id);
                } else {
                    self.status = "autosave off; use :w".to_string();
                }
                return Ok(());
            }
            Key::Ctrl('n') => {
                if self.autosave_enabled {
                    self.save(db)?;
                }
                let note = new_note_with_context(
                    db,
                    &self.note_creation_theme,
                    self.working_collection_id.as_deref(),
                )?;
                self.set_active_note(db, note)?;
                self.refresh_switcher_items(db)?;
                self.status = format!("new note {}", self.active_note.id);
                return Ok(());
            }
            Key::Ctrl('p') => {
                self.dismiss_variable_autocomplete_popup();
                self.open_switcher(db)?;
                return Ok(());
            }
            Key::Ctrl('g') => {
                self.dismiss_variable_autocomplete_popup();
                self.open_collection_switcher(db)?;
                return Ok(());
            }
            Key::ArrowUp => {
                if self.wiki_link_autocomplete_popup.visible {
                    self.move_wiki_link_selection(-1);
                } else if !self.move_variable_autocomplete_selection(-1) {
                    if !self.try_shared_table_cursor_motion(
                        crate::editor_core::table::TableCursorMotionDirection::Up,
                    ) {
                        if self.cursor_line_wraps() {
                            self.move_cursor_screen(true, 1);
                        } else {
                            self.move_cursor_up(1);
                            preserve_table_column = true;
                        }
                    }
                    moved_cursor = true;
                }
            }
            Key::ArrowDown => {
                if self.wiki_link_autocomplete_popup.visible {
                    self.move_wiki_link_selection(1);
                } else if !self.move_variable_autocomplete_selection(1) {
                    if !self.try_shared_table_cursor_motion(
                        crate::editor_core::table::TableCursorMotionDirection::Down,
                    ) {
                        if self.cursor_line_wraps() {
                            self.move_cursor_screen(false, 1);
                        } else {
                            self.move_cursor_down(1);
                            preserve_table_column = true;
                        }
                    }
                    moved_cursor = true;
                }
            }
            Key::ArrowLeft => {
                self.move_cursor_left();
                moved_cursor = true;
            }
            Key::ArrowRight => {
                self.move_cursor_right();
                moved_cursor = true;
            }
            Key::CtrlArrowLeft => {
                if !self.try_table_navigation_rule(true)
                    && (!self.note_table_module_enabled()
                        || !is_markdown_table_line(self.current_line()))
                {
                    self.move_cursor_left_word();
                }
                moved_cursor = true;
            }
            Key::CtrlArrowRight => {
                if !self.try_table_navigation_rule(false)
                    && (!self.note_table_module_enabled()
                        || !is_markdown_table_line(self.current_line()))
                {
                    self.move_cursor_right_word();
                }
                moved_cursor = true;
            }
            Key::CtrlBackspace => {
                if self.try_table_header_delete_column_rule() {
                    should_autoformat = false;
                    clamp_table_padding = false;
                } else if let Some(changed) = self.try_table_boundary_edit_rule(true, true) {
                    should_autoformat = changed;
                } else {
                    should_autoformat = self.delete_word_backward();
                }
                refresh_variable_popup = true;
            }
            Key::CtrlDelete => {
                if self.try_table_header_delete_column_rule() {
                    should_autoformat = false;
                    clamp_table_padding = false;
                } else if let Some(changed) = self.try_table_boundary_edit_rule(false, true) {
                    should_autoformat = changed;
                } else {
                    self.delete_forward();
                    should_autoformat = true;
                }
                refresh_variable_popup = true;
            }
            Key::PageUp => {
                self.move_cursor_up(self.editor_height().saturating_sub(1));
                moved_cursor = true;
                preserve_table_column = true;
            }
            Key::PageDown => {
                self.move_cursor_down(self.editor_height().saturating_sub(1));
                moved_cursor = true;
                preserve_table_column = true;
            }
            Key::Home => {
                self.editor.cursor_col = 0;
                moved_cursor = true;
            }
            Key::End => {
                self.editor.cursor_col = line_char_len(self.current_line());
                moved_cursor = true;
            }
            Key::Backspace => {
                if let Some(changed) = self.try_table_boundary_edit_rule(true, false) {
                    should_autoformat = changed;
                } else {
                    self.backspace();
                    should_autoformat = true;
                }
                refresh_variable_popup = true;
            }
            Key::Delete => {
                if let Some(changed) = self.try_table_boundary_edit_rule(false, false) {
                    should_autoformat = changed;
                } else {
                    self.delete_forward();
                    should_autoformat = true;
                }
                refresh_variable_popup = true;
            }
            Key::Enter => {
                if self.apply_wiki_link_selection(db) {
                    should_autoformat = true;
                } else if self.apply_variable_autocomplete_popup_selection() {
                    should_autoformat = true;
                } else {
                    if !self.try_enter_rule() {
                        self.insert_newline();
                    }
                    refresh_variable_popup = true;
                    should_autoformat = true;
                }
            }
            Key::ShiftEnter => {
                if self.try_table_multiline_break_rule() {
                    should_autoformat = true;
                    refresh_variable_popup = true;
                    clamp_table_padding = false;
                } else if !self.try_enter_rule() {
                    self.insert_newline();
                    refresh_variable_popup = true;
                    should_autoformat = true;
                } else {
                    refresh_variable_popup = true;
                    should_autoformat = true;
                }
            }
            Key::Tab => {
                if self.apply_wiki_link_selection(db) {
                    should_autoformat = true;
                } else if self.apply_variable_autocomplete_tab() {
                    should_autoformat = true;
                    // handled
                } else if self.apply_calc_tab() {
                    should_autoformat = true;
                    // handled
                } else if !self.try_table_navigation_rule(false) && !self.try_tab_rule(false) {
                    self.insert_text("  ");
                    should_autoformat = true;
                }
                refresh_variable_popup = true;
            }
            Key::BackTab => {
                if !self.try_table_navigation_rule(true) && self.try_tab_rule(true) {
                    should_autoformat = true;
                }
            }
            Key::Ctrl(']') => {
                self.cancel_wiki_link_autocomplete();
                self.navigate_wiki_link_at_cursor(db);
                return Ok(());
            }
            Key::Ctrl('e') => {
                self.command_input.clear();
                self.dismiss_command_completion_menu();
                self.command_history_index = None;
                self.command_bar_from_normal = false;
                self.command_selection = None;
                self.command_selection_linewise = false;
                self.mode = UiMode::CommandBar;
                self.status = ":".to_string();
                self.dismiss_variable_autocomplete_popup();
                return Ok(());
            }
            Key::Ctrl('f') => {
                self.dismiss_variable_autocomplete_popup();
                self.open_search();
                return Ok(());
            }
            Key::Paste(text) => {
                if self.try_import_image_paste(db, &text)? {
                    should_autoformat = false;
                    refresh_variable_popup = true;
                } else {
                    self.insert_paste(&text);
                    // Pasted content should stay as-is; skip per-keystroke
                    // autoformat pass that would otherwise scan the full document.
                    should_autoformat = false;
                    refresh_variable_popup = true;
                }
            }
            Key::Char(ch) => {
                let plan = if self.note_table_module_enabled() {
                    crate::editor_core::table::plan_table_typed_char(
                        &self.editor.lines,
                        self.editor.cursor_line,
                        self.editor.cursor_col,
                        ch,
                    )
                } else {
                    crate::editor_core::table::TableTypingPlan::default()
                };
                if ch == '|' && self.try_table_manual_row_start_rule() {
                    // Blank row replaced by `|`: keep composing the row by hand.
                    should_autoformat = false;
                    cursor_after_row_end = true;
                } else if ch == '|' && self.try_table_pipe_insert_column_rule() {
                    should_autoformat = false;
                    clamp_table_padding = false;
                } else {
                    use crate::editor_core::table::TableTypingCursor;
                    if plan.pad_before {
                        self.insert_char(' ');
                    }
                    self.insert_char(ch);
                    should_autoformat = plan.autoformat;
                    match plan.cursor {
                        TableTypingCursor::InCellContent => {}
                        TableTypingCursor::InCellPadding => clamp_table_padding = false,
                        TableTypingCursor::PastRowEnd => cursor_after_row_end = true,
                    }
                    if plan.closes_row && self.try_table_duplicate_delimiter_rule() {
                        should_autoformat = false;
                    }
                    // Auto-close [[ → [[]] and open wiki-link picker.
                    if ch == '[' && self.editor.cursor_col >= 2 {
                        let prev = self
                            .current_line()
                            .chars()
                            .nth(self.editor.cursor_col.saturating_sub(2));
                        if prev == Some('[') {
                            self.insert_text("]]");
                            self.editor.cursor_col = self.editor.cursor_col.saturating_sub(2);
                            self.dismiss_variable_autocomplete_popup();
                            self.open_wiki_link_autocomplete(db);
                        }
                    }
                    if ch == '#' {
                        self.maybe_open_wiki_link_autocomplete_at_cursor(db);
                    }
                }
                refresh_variable_popup = true;
            }
            Key::Esc => {
                if self.wiki_link_autocomplete_popup.visible {
                    self.cancel_wiki_link_autocomplete();
                } else if self.variable_autocomplete_popup.visible {
                    self.dismiss_variable_autocomplete_popup();
                } else {
                    self.mode = UiMode::Normal;
                    self.vim_state = crate::editor_core::vim::VimState::default();
                    self.status = "-- NORMAL --".to_string();
                    self.dismiss_variable_autocomplete_popup();
                }
            }
            Key::Ctrl(_) => {}
        }

        if preserve_table_column || cursor_after_row_end {
            self.clamp_cursor_to_line_bounds();
        } else {
            self.adjust_cursor_with_table_padding_guard(clamp_table_padding);
        }
        self.adjust_scroll();

        if should_autoformat {
            self.try_autoformat_rules();
        }

        if self.mode == UiMode::Editor {
            if moved_cursor {
                self.dismiss_variable_autocomplete_popup();
                self.cancel_wiki_link_autocomplete();
            } else if refresh_variable_popup {
                self.refresh_variable_autocomplete_popup();
                self.refresh_wiki_link_autocomplete(db);
            }
        } else {
            self.dismiss_variable_autocomplete_popup();
            self.cancel_wiki_link_autocomplete();
        }

        Ok(())
    }

    pub(super) fn handle_normal_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        if key == Key::Ctrl('q') {
            self.quit = true;
            return Ok(());
        }

        if key == Key::Ctrl('s') {
            self.save(db)?;
            self.status = format!("saved {}", self.active_note.id);
            return Ok(());
        }

        if key == Key::Ctrl('p') {
            self.open_switcher(db)?;
            return Ok(());
        }

        if key == Key::Ctrl('g') {
            self.open_collection_switcher(db)?;
            return Ok(());
        }

        if key == Key::Char('?') {
            self.open_web_search(None);
            return Ok(());
        }

        if key == Key::Ctrl(']') {
            self.navigate_wiki_link_at_cursor(db);
            return Ok(());
        }

        if key == Key::Char('x')
            && matches!(
                self.vim_state.pending,
                Some(crate::editor_core::vim::VimPending::Go)
            )
        {
            self.vim_state.pending = None;
            self.preview_image_at_cursor(db);
            return Ok(());
        }

        if key == Key::Char('K') {
            if self.wiki_link_preview.visible {
                self.close_wiki_link_preview();
            } else {
                self.open_wiki_link_preview(db);
            }
            return Ok(());
        }

        if self.wiki_link_preview.visible {
            self.close_wiki_link_preview();
        }

        let now = Instant::now();
        if self
            .folds
            .pending_prefix_until
            .is_some_and(|until| now > until)
        {
            self.folds.pending_prefix_until = None;
        }

        if self.folds.pending_prefix_until.take().is_some() {
            if key == Key::Char('a') {
                self.toggle_fold_at_cursor();
                return Ok(());
            }
        }

        if key == Key::Char('z') {
            self.folds.pending_prefix_until =
                Some(now + Duration::from_millis(FOLD_PREFIX_TIMEOUT_MS));
            return Ok(());
        }

        // gd: navigate wiki link. Check vim state's pending Go before running the pipeline
        // so 'gg' still reaches the pipeline unimpeded.
        if key == Key::Char('d')
            && matches!(
                self.vim_state.pending,
                Some(crate::editor_core::vim::VimPending::Go)
            )
        {
            self.vim_state.pending = None;
            self.navigate_wiki_link_at_cursor(db);
            return Ok(());
        }

        let (doc_mutated, preserve_vertical_column) = match self.run_vim_pipeline(db, &key) {
            VimPipelineResult::NoIntent | VimPipelineResult::Unhandled => return Ok(()),
            VimPipelineResult::Applied {
                doc_mutated,
                preserve_vertical_column,
            } => (doc_mutated, preserve_vertical_column),
        };
        if key == Key::Esc {
            self.search.matches.clear();
            self.search.query.clear();
            self.status = "-- NORMAL --".to_string();
        }

        // A change operator (`cw`, `ciw`, ...) that leaves insert mode on a
        // table row keeps the cell as is: the cursor may sit in the space left
        // by the deleted word, and the row reformats once typing starts.
        let changing_table_cell = self.mode == UiMode::Editor
            && self.note_table_module_enabled()
            && is_markdown_table_line(self.current_line());
        if preserve_vertical_column {
            self.clamp_cursor_to_line_bounds();
        } else {
            self.adjust_cursor_with_table_padding_guard(!changing_table_cell);
        }
        self.adjust_scroll();

        if doc_mutated
            && !changing_table_cell
            && Self::line_might_trigger_doc_change_rules(self.current_line())
        {
            let (start_line, end_line) = self.scoped_rule_line_span(self.editor.cursor_line);
            let (ctx, scope_start_offset) =
                self.build_scoped_context_for_line_span(start_line, end_line, None);
            let options = crate::editor_core::text_rules::TextRuleOptions {
                markdown_autoformat: self.markdown_autoformat_enabled(),
                checklist_auto_reorder: self.checklist_auto_reorder_enabled(),
                table_enabled: self.note_table_module_enabled(),
            };
            if let Some(op) = crate::editor_core::text_rules::run_doc_change_rules_with_table_cache(
                &ctx,
                options,
                &mut self.table_format_cache,
            ) {
                let mapped = Self::remap_operation_from_scope(&op, scope_start_offset);
                self.apply_edit_operation(&mapped);
            }
        }

        Ok(())
    }

    pub(super) fn handle_visual_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        if key == Key::Ctrl('p') {
            self.open_switcher(db)?;
            return Ok(());
        }

        if key == Key::Ctrl('g') {
            self.open_collection_switcher(db)?;
            return Ok(());
        }

        let normalized_key = if key == Key::Ctrl('c') { Key::Esc } else { key };
        let (doc_mutated, preserve_vertical_column) =
            match self.run_vim_pipeline(db, &normalized_key) {
                VimPipelineResult::NoIntent => {
                    self.adjust_cursor();
                    self.adjust_scroll();
                    return Ok(());
                }
                VimPipelineResult::Unhandled => (false, false),
                VimPipelineResult::Applied {
                    doc_mutated,
                    preserve_vertical_column,
                } => (doc_mutated, preserve_vertical_column),
            };

        if preserve_vertical_column {
            self.clamp_cursor_to_line_bounds();
        } else {
            self.adjust_cursor();
        }
        self.adjust_scroll();

        if doc_mutated && Self::line_might_trigger_doc_change_rules(self.current_line()) {
            let (start_line, end_line) = self.scoped_rule_line_span(self.editor.cursor_line);
            let (ctx, scope_start_offset) =
                self.build_scoped_context_for_line_span(start_line, end_line, None);
            let options = crate::editor_core::text_rules::TextRuleOptions {
                markdown_autoformat: self.markdown_autoformat_enabled(),
                checklist_auto_reorder: self.checklist_auto_reorder_enabled(),
                table_enabled: self.note_table_module_enabled(),
            };
            if let Some(op) = crate::editor_core::text_rules::run_doc_change_rules_with_table_cache(
                &ctx,
                options,
                &mut self.table_format_cache,
            ) {
                let mapped = Self::remap_operation_from_scope(&op, scope_start_offset);
                self.apply_edit_operation(&mapped);
            }
        }

        Ok(())
    }

    pub(super) fn open_date_picker(&mut self, action: DatePickerAction, require_time: bool) {
        self.dismiss_variable_autocomplete_popup();
        if let Some((year, month, day, hour, minute)) = date_picker::current_local_datetime_parts()
        {
            self.date_picker.year = year;
            self.date_picker.month = month;
            self.date_picker.day = day;
            self.date_picker.hour = hour;
            self.date_picker.minute = minute;
        } else {
            let now = time::OffsetDateTime::now_utc();
            let date = now.date();
            let tod = now.time();
            self.date_picker.year = date.year();
            self.date_picker.month = date.month() as u32;
            self.date_picker.day = date.day() as u32;
            self.date_picker.hour = u32::from(tod.hour());
            self.date_picker.minute = u32::from(tod.minute());
        }
        self.date_picker.require_time = require_time;
        self.date_picker.include_time = require_time;
        self.date_picker.action = action;
        self.date_picker.return_mode = self.mode;
        self.mode = UiMode::DatePicker;
        self.status =
            "Date picker: arrows days, Ctrl+arrows months, h/l hour, j/k minute, Tab time, Enter confirm"
                .to_string();
    }

    pub(super) fn close_date_picker(&mut self) {
        self.mode = self.date_picker.return_mode;
        self.status = if self.mode == UiMode::Normal {
            "-- NORMAL --".to_string()
        } else {
            format!("editing {}", self.active_note.id)
        };
    }

    pub(super) fn adjust_picker_hour(&mut self, delta: i32) {
        let mut next = self.date_picker.hour as i32 + delta;
        while next < 0 {
            next += 24;
        }
        while next >= 24 {
            next -= 24;
        }
        self.date_picker.hour = next as u32;
    }

    pub(super) fn adjust_picker_minute(&mut self, delta: i32) {
        let mut next = self.date_picker.minute as i32 + delta;
        while next < 0 {
            next += 60;
            self.adjust_picker_hour(-1);
        }
        while next >= 60 {
            next -= 60;
            self.adjust_picker_hour(1);
        }
        self.date_picker.minute = next as u32;
    }

    pub(super) fn handle_date_picker_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        match key {
            Key::Esc => {
                self.close_date_picker();
            }
            Key::Enter => {
                if self.date_picker.action == DatePickerAction::InsertDate {
                    let inserted = date_picker::format_datetime_with_pattern(
                        self.date_picker.year,
                        self.date_picker.month,
                        self.date_picker.day,
                        self.date_picker.hour,
                        self.date_picker.minute,
                        if self.date_picker.include_time {
                            &self.date_picker.time_format
                        } else {
                            &self.date_picker.format
                        },
                    );
                    self.close_date_picker();
                    self.insert_text(&inserted);
                    self.status = format!("Date inserted: {inserted}");
                    return Ok(());
                }

                let remind_at_ms = date_picker::local_datetime_to_epoch_ms(
                    self.date_picker.year,
                    self.date_picker.month,
                    self.date_picker.day,
                    self.date_picker.hour,
                    self.date_picker.minute,
                )
                .ok_or_else(|| "failed to convert reminder time".to_string())?;
                let display_at = date_picker::format_datetime_with_pattern(
                    self.date_picker.year,
                    self.date_picker.month,
                    self.date_picker.day,
                    self.date_picker.hour,
                    self.date_picker.minute,
                    &self.date_picker.time_format,
                );
                let line_number = (self.editor.cursor_line + 1) as i64;
                let line_text = self.current_line().to_string();
                let before_reminder = self.reminder_ghosts.get(&self.editor.cursor_line).cloned();
                db.upsert_reminder(
                    &self.active_note.id,
                    line_number,
                    remind_at_ms,
                    &display_at,
                    &line_text,
                )?;
                if let Some(line_idx) = line_number
                    .checked_sub(1)
                    .and_then(|line| usize::try_from(line).ok())
                {
                    let entry = LineReminderGhost {
                        remind_at_ms,
                        display_at: display_at.clone(),
                        line_text: line_text.clone(),
                        reminded_at_ms: None,
                    };
                    self.reminder_ghosts.insert(line_idx, entry.clone());
                    self.push_reminder_undo_entry(line_idx, before_reminder, Some(entry));
                }
                self.close_date_picker();
                self.status = format!("remind set ⏰ {display_at}");
            }
            Key::ArrowLeft => {
                if self.date_picker.day > 1 {
                    self.date_picker.day -= 1;
                }
            }
            Key::ArrowRight => {
                let max = date_picker::days_in_month(self.date_picker.year, self.date_picker.month);
                if self.date_picker.day < max {
                    self.date_picker.day += 1;
                }
            }
            Key::ArrowUp => {
                if self.date_picker.day > 7 {
                    self.date_picker.day -= 7;
                } else {
                    // Go to previous month
                    if self.date_picker.month == 1 {
                        self.date_picker.month = 12;
                        self.date_picker.year -= 1;
                    } else {
                        self.date_picker.month -= 1;
                    }
                    let max =
                        date_picker::days_in_month(self.date_picker.year, self.date_picker.month);
                    self.date_picker.day = max.min(self.date_picker.day);
                }
            }
            Key::ArrowDown => {
                let max = date_picker::days_in_month(self.date_picker.year, self.date_picker.month);
                if self.date_picker.day + 7 <= max {
                    self.date_picker.day += 7;
                } else {
                    // Go to next month
                    if self.date_picker.month == 12 {
                        self.date_picker.month = 1;
                        self.date_picker.year += 1;
                    } else {
                        self.date_picker.month += 1;
                    }
                    let new_max =
                        date_picker::days_in_month(self.date_picker.year, self.date_picker.month);
                    self.date_picker.day = new_max.min(self.date_picker.day);
                }
            }
            Key::CtrlArrowLeft => {
                // Previous month
                if self.date_picker.month == 1 {
                    self.date_picker.month = 12;
                    self.date_picker.year -= 1;
                } else {
                    self.date_picker.month -= 1;
                }
                let max = date_picker::days_in_month(self.date_picker.year, self.date_picker.month);
                self.date_picker.day = self.date_picker.day.min(max);
            }
            Key::CtrlArrowRight => {
                // Next month
                if self.date_picker.month == 12 {
                    self.date_picker.month = 1;
                    self.date_picker.year += 1;
                } else {
                    self.date_picker.month += 1;
                }
                let max = date_picker::days_in_month(self.date_picker.year, self.date_picker.month);
                self.date_picker.day = self.date_picker.day.min(max);
            }
            Key::Home | Key::Char('h') => self.adjust_picker_hour(-1),
            Key::End | Key::Char('l') => self.adjust_picker_hour(1),
            Key::PageUp | Key::Char('j') => self.adjust_picker_minute(-1),
            Key::PageDown | Key::Char('k') => self.adjust_picker_minute(1),
            Key::Tab | Key::Char('t') | Key::Char('T') => {
                if !self.date_picker.require_time {
                    self.date_picker.include_time = !self.date_picker.include_time;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl TerminalApp {
    fn preview_image_at_cursor(&mut self, db: &Db) {
        if !self.graphics.is_enabled() {
            self.open_image_externally_at_cursor(db);
            return;
        }
        let Some(image) = crate::editor_core::markdown_tokens::find_markdown_image_at_cursor(
            self.current_line(),
            self.editor.cursor_col,
        ) else {
            self.status = "no image at cursor".to_string();
            return;
        };
        self.image_preview = Some(super::ImagePreviewState {
            src: image.src,
            alt: image.alt,
        });
        self.close_wiki_link_preview();
    }

    fn open_image_externally_at_cursor(&mut self, db: &Db) {
        use base64::Engine as _;
        let Some(image) = crate::editor_core::markdown_tokens::find_markdown_image_at_cursor(
            self.current_line(),
            self.editor.cursor_col,
        ) else {
            self.status = "no image at cursor".to_string();
            return;
        };
        let sources = app_core::note_sources::NoteSourceService::new(db.clone());
        let resolved = match sources
            .resolve_image_markdown_source_by_id(&self.active_note.id, &image.src)
        {
            Ok(Some(value)) => value,
            Ok(None) => {
                self.status = "image source is missing".to_string();
                return;
            }
            Err(error) => {
                self.status = format!("cannot resolve image: {error}");
                return;
            }
        };
        let mut temporary_path = false;
        let path = if let Some(data) = resolved.strip_prefix("data:") {
            let Some((header, encoded)) = data.split_once(',') else {
                self.status = "invalid image data".to_string();
                return;
            };
            if !header.ends_with(";base64") || encoded.len() > 24 * 1024 * 1024 {
                self.status = "unsupported image data".to_string();
                return;
            }
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
                self.status = "invalid image data".to_string();
                return;
            };
            let extension = match header.split(';').next().unwrap_or_default() {
                "image/jpeg" => "jpg",
                "image/gif" => "gif",
                "image/webp" => "webp",
                "image/bmp" => "bmp",
                _ => "png",
            };
            let path = match create_private_image_file(&bytes, extension) {
                Ok(path) => path,
                Err(error) => {
                    self.status = format!("cannot prepare image: {error}");
                    return;
                }
            };
            temporary_path = true;
            path
        } else if resolved.contains("://") {
            self.status = "remote images are not opened".to_string();
            return;
        } else {
            std::path::PathBuf::from(resolved)
        };

        match open_local_file(&path) {
            Ok(()) => {
                if temporary_path {
                    self.open_image_temp_paths.push(path);
                }
                self.status = format!("opened image {}", image.alt);
            }
            Err(error) => {
                if temporary_path {
                    let _ = std::fs::remove_file(path);
                }
                self.status = format!("cannot open image: {error}");
            }
        }
    }
}

fn open_local_file(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    let (program, args) = ("xdg-open", vec![path.as_os_str()]);
    #[cfg(target_os = "macos")]
    let (program, args) = ("open", vec![path.as_os_str()]);
    #[cfg(target_os = "windows")]
    let (program, args) = (
        "rundll32.exe",
        vec![
            std::ffi::OsStr::new("url.dll,FileProtocolHandler"),
            path.as_os_str(),
        ],
    );
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return Err("external image viewers are unsupported on this platform".to_string());

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    std::process::Command::new(program)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn create_private_image_file(bytes: &[u8], extension: &str) -> Result<std::path::PathBuf, String> {
    let directory = std::env::temp_dir().join(format!("slate-open-image-{}", ulid::Ulid::new()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(&directory)
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir(&directory).map_err(|error| error.to_string())?;

    let path = directory.join(format!("image.{extension}"));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
        }
        std::io::Write::write_all(&mut file, bytes).map_err(|error| error.to_string())?;
        Ok(path.clone())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&directory);
    }
    result
}

impl Drop for TerminalApp {
    fn drop(&mut self) {
        for path in self.open_image_temp_paths.drain(..) {
            if let Some(directory) = path.parent() {
                let _ = std::fs::remove_dir_all(directory);
            }
        }
    }
}

#[cfg(test)]
mod image_open_tests {
    use super::*;

    #[test]
    fn database_image_temp_file_is_private_and_removable() {
        let path = create_private_image_file(b"pixels", "png").expect("create private image");
        assert_eq!(std::fs::read(&path).expect("read image"), b"pixels");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&path)
                    .expect("image metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(path.parent().expect("private dir"))
                    .expect("dir metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        std::fs::remove_dir_all(path.parent().expect("private dir")).expect("cleanup image");
    }
}
