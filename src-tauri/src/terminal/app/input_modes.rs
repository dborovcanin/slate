use super::{
    is_markdown_table_line, new_note, DatePickerAction, Db, Key, LineReminderGhost, TerminalApp,
    UiMode, VimPipelineResult, FOLD_PREFIX_TIMEOUT_MS,
};
use crate::terminal::date_picker;
use crate::terminal::text_utils::line_char_len;
use std::time::{Duration, Instant};

// Ownership: key dispatch and per-mode key handling entry points.
impl TerminalApp {
    pub(super) fn handle_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        if self.mode != UiMode::Normal {
            self.folds.pending_prefix_until = None;
        }
        match self.mode {
            UiMode::DatePicker => self.handle_date_picker_key(db, key)?,
            UiMode::Editor => self.handle_editor_key(db, key)?,
            UiMode::Normal => self.handle_normal_key(db, key)?,
            UiMode::Visual | UiMode::VisualLine => self.handle_visual_key(db, key)?,
            UiMode::Switcher => self.handle_switcher_key(db, key)?,
            UiMode::ContentSearch => self.handle_content_search_key(db, key)?,
            UiMode::CommandBar => self.handle_command_bar_key(db, key)?,
            UiMode::Search => self.handle_search_key(key)?,
        }
        Ok(())
    }

    pub(super) fn handle_editor_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        let mut should_autoformat = false;
        let mut clamp_table_padding = true;
        let mut moved_cursor = false;
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
            Key::Ctrl('w') => {
                if self.try_table_header_delete_column_rule() {
                    should_autoformat = false;
                    clamp_table_padding = false;
                } else if let Some(changed) = self.try_table_boundary_edit_rule(true, true) {
                    should_autoformat = changed;
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
                let note = new_note(db, &crate::config::load_theme_config())?;
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
            Key::ArrowUp => {
                if self.wiki_link_autocomplete_popup.visible {
                    self.move_wiki_link_selection(-1);
                } else if !self.move_variable_autocomplete_selection(-1) {
                    self.move_cursor_up(1);
                    moved_cursor = true;
                }
            }
            Key::ArrowDown => {
                if self.wiki_link_autocomplete_popup.visible {
                    self.move_wiki_link_selection(1);
                } else if !self.move_variable_autocomplete_selection(1) {
                    self.move_cursor_down(1);
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
            }
            Key::PageDown => {
                self.move_cursor_down(self.editor_height().saturating_sub(1));
                moved_cursor = true;
            }
            Key::Home => {
                self.cursor_col = 0;
                moved_cursor = true;
            }
            Key::End => {
                self.cursor_col = line_char_len(self.current_line());
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
                if self.apply_wiki_link_selection() {
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
                if self.apply_wiki_link_selection() {
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
                self.dismiss_wiki_link_autocomplete();
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
                if ch == '|' && self.try_table_pipe_insert_column_rule() {
                    should_autoformat = false;
                    clamp_table_padding = false;
                } else {
                    self.insert_char(ch);
                    should_autoformat = true;
                    // Auto-close [[ → [[]] and open wiki-link picker.
                    if ch == '[' && self.cursor_col >= 2 {
                        let prev = self
                            .current_line()
                            .chars()
                            .nth(self.cursor_col.saturating_sub(2));
                        if prev == Some('[') {
                            self.insert_text("]]");
                            self.cursor_col = self.cursor_col.saturating_sub(2);
                            self.dismiss_variable_autocomplete_popup();
                            self.open_wiki_link_autocomplete(db);
                        }
                    }
                    if ch == '#' {
                        self.maybe_open_wiki_link_autocomplete_at_cursor(db);
                    }
                }
                refresh_variable_popup = true;
                if ch == ' '
                    && self.note_table_module_enabled()
                    && is_markdown_table_line(self.current_line())
                {
                    // Let users type multi-word table cell content without
                    // instant trim/realign fighting the cursor.
                    should_autoformat = false;
                    // Keep right-padding clamp relaxed for this keystroke so
                    // the next word can continue after the inserted space.
                    clamp_table_padding = false;
                }
            }
            Key::Esc => {
                if self.wiki_link_autocomplete_popup.visible {
                    self.dismiss_wiki_link_autocomplete();
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

        self.adjust_cursor_with_table_padding_guard(clamp_table_padding);
        self.adjust_scroll();

        if should_autoformat {
            self.try_autoformat_rules();
        }

        if self.mode == UiMode::Editor {
            if moved_cursor {
                self.dismiss_variable_autocomplete_popup();
                self.dismiss_wiki_link_autocomplete();
            } else if refresh_variable_popup {
                self.refresh_variable_autocomplete_popup();
                self.refresh_wiki_link_autocomplete(db);
            }
        } else {
            self.dismiss_variable_autocomplete_popup();
            self.dismiss_wiki_link_autocomplete();
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

        if key == Key::Ctrl(']') {
            self.navigate_wiki_link_at_cursor(db);
            return Ok(());
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

        let doc_mutated = match self.run_vim_pipeline(&key) {
            VimPipelineResult::NoIntent | VimPipelineResult::Unhandled => return Ok(()),
            VimPipelineResult::Applied { doc_mutated } => doc_mutated,
        };
        if key == Key::Esc {
            self.search_matches.clear();
            self.search_query.clear();
            self.status = "-- NORMAL --".to_string();
        }

        self.adjust_cursor();
        self.adjust_scroll();

        if doc_mutated && Self::line_might_trigger_doc_change_rules(self.current_line()) {
            let (start_line, end_line) = self.scoped_rule_line_span(self.cursor_line);
            let (ctx, scope_start_offset) =
                self.build_scoped_context_for_line_span(start_line, end_line, None);
            let options = crate::editor_core::text_rules::TextRuleOptions {
                markdown_autoformat: self.markdown_autoformat_enabled(),
                checklist_auto_reorder: self.checklist_auto_reorder_enabled(),
                table_enabled: self.note_table_module_enabled(),
            };
            if let Some(op) = crate::editor_core::text_rules::run_doc_change_rules(&ctx, options) {
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

        let normalized_key = if key == Key::Ctrl('c') { Key::Esc } else { key };
        if matches!(
            self.run_vim_pipeline(&normalized_key),
            VimPipelineResult::NoIntent
        ) {
            self.adjust_cursor();
            self.adjust_scroll();
            return Ok(());
        }

        self.adjust_cursor();
        self.adjust_scroll();
        Ok(())
    }

    pub(super) fn open_date_picker(&mut self, action: DatePickerAction, require_time: bool) {
        self.dismiss_variable_autocomplete_popup();
        if let Some((year, month, day, hour, minute)) = date_picker::current_local_datetime_parts()
        {
            self.date_year = year;
            self.date_month = month;
            self.date_day = day;
            self.date_hour = hour;
            self.date_minute = minute;
        } else {
            let now = time::OffsetDateTime::now_utc();
            let date = now.date();
            let tod = now.time();
            self.date_year = date.year();
            self.date_month = date.month() as u32;
            self.date_day = date.day() as u32;
            self.date_hour = u32::from(tod.hour());
            self.date_minute = u32::from(tod.minute());
        }
        self.date_require_time = require_time;
        self.date_include_time = require_time;
        self.date_picker_action = action;
        self.date_picker_return_mode = self.mode;
        self.mode = UiMode::DatePicker;
        self.status =
            "Date picker: arrows days, Ctrl+arrows months, h/l hour, j/k minute, Tab time, Enter confirm"
                .to_string();
    }

    pub(super) fn close_date_picker(&mut self) {
        self.mode = self.date_picker_return_mode;
        self.status = if self.mode == UiMode::Normal {
            "-- NORMAL --".to_string()
        } else {
            format!("editing {}", self.active_note.id)
        };
    }

    pub(super) fn adjust_picker_hour(&mut self, delta: i32) {
        let mut next = self.date_hour as i32 + delta;
        while next < 0 {
            next += 24;
        }
        while next >= 24 {
            next -= 24;
        }
        self.date_hour = next as u32;
    }

    pub(super) fn adjust_picker_minute(&mut self, delta: i32) {
        let mut next = self.date_minute as i32 + delta;
        while next < 0 {
            next += 60;
            self.adjust_picker_hour(-1);
        }
        while next >= 60 {
            next -= 60;
            self.adjust_picker_hour(1);
        }
        self.date_minute = next as u32;
    }

    pub(super) fn handle_date_picker_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        match key {
            Key::Esc => {
                self.close_date_picker();
            }
            Key::Enter => {
                if self.date_picker_action == DatePickerAction::InsertDate {
                    let inserted = date_picker::format_datetime_with_pattern(
                        self.date_year,
                        self.date_month,
                        self.date_day,
                        self.date_hour,
                        self.date_minute,
                        if self.date_include_time {
                            &self.date_time_format
                        } else {
                            &self.date_format
                        },
                    );
                    self.close_date_picker();
                    self.insert_text(&inserted);
                    self.status = format!("Date inserted: {inserted}");
                    return Ok(());
                }

                let remind_at_ms = date_picker::local_datetime_to_epoch_ms(
                    self.date_year,
                    self.date_month,
                    self.date_day,
                    self.date_hour,
                    self.date_minute,
                )
                .ok_or_else(|| "failed to convert reminder time".to_string())?;
                let display_at = date_picker::format_datetime_with_pattern(
                    self.date_year,
                    self.date_month,
                    self.date_day,
                    self.date_hour,
                    self.date_minute,
                    &self.date_time_format,
                );
                let line_number = (self.cursor_line + 1) as i64;
                let line_text = self.current_line().to_string();
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
                    self.reminder_ghosts.insert(
                        line_idx,
                        LineReminderGhost {
                            remind_at_ms,
                            display_at: display_at.clone(),
                            line_text: line_text.clone(),
                            notified_at_ms: None,
                        },
                    );
                }
                self.close_date_picker();
                self.status = format!("notify set ⏰ {display_at}");
            }
            Key::ArrowLeft => {
                if self.date_day > 1 {
                    self.date_day -= 1;
                }
            }
            Key::ArrowRight => {
                let max = date_picker::days_in_month(self.date_year, self.date_month);
                if self.date_day < max {
                    self.date_day += 1;
                }
            }
            Key::ArrowUp => {
                if self.date_day > 7 {
                    self.date_day -= 7;
                } else {
                    // Go to previous month
                    if self.date_month == 1 {
                        self.date_month = 12;
                        self.date_year -= 1;
                    } else {
                        self.date_month -= 1;
                    }
                    let max = date_picker::days_in_month(self.date_year, self.date_month);
                    self.date_day = max.min(self.date_day);
                }
            }
            Key::ArrowDown => {
                let max = date_picker::days_in_month(self.date_year, self.date_month);
                if self.date_day + 7 <= max {
                    self.date_day += 7;
                } else {
                    // Go to next month
                    if self.date_month == 12 {
                        self.date_month = 1;
                        self.date_year += 1;
                    } else {
                        self.date_month += 1;
                    }
                    let new_max = date_picker::days_in_month(self.date_year, self.date_month);
                    self.date_day = new_max.min(self.date_day);
                }
            }
            Key::CtrlArrowLeft => {
                // Previous month
                if self.date_month == 1 {
                    self.date_month = 12;
                    self.date_year -= 1;
                } else {
                    self.date_month -= 1;
                }
                let max = date_picker::days_in_month(self.date_year, self.date_month);
                self.date_day = self.date_day.min(max);
            }
            Key::CtrlArrowRight => {
                // Next month
                if self.date_month == 12 {
                    self.date_month = 1;
                    self.date_year += 1;
                } else {
                    self.date_month += 1;
                }
                let max = date_picker::days_in_month(self.date_year, self.date_month);
                self.date_day = self.date_day.min(max);
            }
            Key::Home | Key::Char('h') => self.adjust_picker_hour(-1),
            Key::End | Key::Char('l') => self.adjust_picker_hour(1),
            Key::PageUp | Key::Char('j') => self.adjust_picker_minute(-1),
            Key::PageDown | Key::Char('k') => self.adjust_picker_minute(1),
            Key::Tab | Key::Char('t') | Key::Char('T') => {
                if !self.date_require_time {
                    self.date_include_time = !self.date_include_time;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
