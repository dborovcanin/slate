use super::ansi::{contrast_fg_for_bg, draw_row_at_styled, goto, pad_right, AnsiStyle};
use super::clipboard::{self, ClipboardWriteBackend};
use super::date_picker::{self, DatePickerAction, DatePickerView};
use super::folding::{self, FoldKind, FoldRange};
use super::history::LineHistory;
use super::input::{self, Key, TerminalGuard};
use super::notifications;
use super::render;
use super::switcher::{self, NoteMeta, SwitcherView};
use super::text_utils::*;

use crate::config::ThemeConfig;
use crate::startup_log::append_startup_log_line;
use crate::storage::{Db, Note};
use app_core::calc::CalcEngine;
use std::cmp::min;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use ulid::Ulid;

const AUTOSAVE_DEBOUNCE_MS: u64 = 500;
const UNDO_DEBOUNCE_MS: u64 = 300;
const CLIPBOARD_WATCH_POLL_MS: u64 = 350;
const FOLD_PREFIX_TIMEOUT_MS: u64 = 900;
const TITLE_ROW: usize = 1;
const EDITOR_TOP_ROW: usize = 2;
const GUTTER_WIDTH: usize = 6;
const HORIZONTAL_SCROLL_LEFT_CONTEXT: usize = 2;
const OVERFLOW_LEFT_MARKER: char = '<';
const OVERFLOW_RIGHT_MARKER: char = '>';
const LARGE_DOC_CALC_DEFER_LINES: usize = 20_000;

fn decimal_digit_count(mut value: usize) -> usize {
    let mut digits = 1usize;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

fn gutter_width_for_visible_lines(visible_lines: usize) -> usize {
    // Keep the legacy 4-digit gutter (+2 spaces), but expand once line numbers
    // outgrow it so rendering/cursor math stay aligned at 10k+ lines.
    (decimal_digit_count(visible_lines.max(1)) + 2).max(GUTTER_WIDTH)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalOptions {
    pub create_new: bool,
    pub note_id: Option<String>,
    pub list_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Editor,
    Normal,
    Visual,
    VisualLine,
    Switcher,
    CommandBar,
    Search,
    DatePicker,
}

#[derive(Debug, Clone, Copy, Default)]
struct TerminalStartupMetrics {
    loading_note: Duration,
    loading_switcher: Duration,
    loading_calc_engine: Duration,
    loading_screen: Duration,
}

const MAX_UNDO_ENTRIES: usize = 500;

#[derive(Debug, Clone)]
struct LineReminderGhost {
    remind_at_ms: i64,
    display_at: String,
    line_text: String,
    notified_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct SwitcherDeleteConfirm {
    note_id: String,
    note_title: String,
}

struct TerminalApp {
    active_note: Note,
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize, // char index
    scroll_line: usize,
    scroll_col: usize,
    mode: UiMode,
    switcher_query: String,
    switcher_items: Vec<NoteMeta>,
    switcher_matches: Vec<usize>,
    switcher_selected: usize,
    switcher_delete_confirm: Option<SwitcherDeleteConfirm>,
    dirty: bool,
    last_edit: Instant,
    status: String,
    command_input: String,
    quit: bool,
    force_quit: bool,
    // Date picker state
    date_year: i32,
    date_month: u32, // 1-12
    date_day: u32,
    date_hour: u32,   // 0-23
    date_minute: u32, // 0-59
    date_include_time: bool,
    date_require_time: bool,
    date_picker_action: DatePickerAction,
    date_picker_return_mode: UiMode,
    date_format: String,
    date_time_format: String,
    // Vim state
    vim_state: crate::editor_core::vim::VimState,
    clipboard: Vec<String>,
    last_clipboard_backend: Option<ClipboardWriteBackend>,
    selection_anchor: Option<(usize, usize)>, // (line, col)
    command_selection: Option<crate::editor_core::types::SelectionSnapshot>,
    command_selection_linewise: bool,
    // Calc ghost cache
    calc_engine: CalcEngine,
    calc_results: Vec<Option<String>>,
    /// Per-line list of `(cell_index, formatted_value)` for table rows
    /// containing one or more `=…` formula cells. Parallel to `calc_results`
    /// (`calc_results[i]` carries the *first* formula value for backward
    /// compatibility); `cell_calc_results[i]` carries every formula cell in
    /// that row in left-to-right order.
    cell_calc_results: Vec<Vec<(usize, String)>>,
    reminder_ghosts: HashMap<usize, LineReminderGhost>, // 0-based line index
    reminders_dirty: bool,
    last_reminder_check: Instant,
    variable_names: Vec<String>,
    // Per-line hashes of `lines` taken at the end of the previous
    // `recompute_calc_full`. Used to detect changed regions (instead of
    // keeping a full clone of lines — saves ~1 String per line) and to gate
    // the committed-trailer auto-refresh: a line is eligible only if its hash
    // matches this snapshot and its previous calc result was `None` (meaning
    // the trailer was in sync with the backend last time).
    prev_line_hashes: Vec<u64>,
    // Parallel to `prev_line_hashes`: tracks which lines in the previous
    // snapshot contained `:=`. Needed to decide if incremental calc is safe
    // without holding the full prev lines.
    prev_line_has_assignment: Vec<bool>,
    calc_state_stale: bool,
    // Search state
    search_query: String,
    search_matches: Vec<(usize, usize, usize)>, // (line_idx, start_col, end_col)
    search_current: usize,
    search_orig_line: usize,
    search_orig_col: usize,
    search_orig_scroll: usize,
    // Auto format
    format_on_save: bool,
    markdown_autoformat: bool,
    checklist_auto_reorder: bool,
    // Calc/variables behavior
    variables_enabled: bool,
    render_palette: render::RenderPalette,
    // Folding (real-line indexed, 0-based)
    fold_ranges: Vec<FoldRange>,
    fold_range_by_start: Vec<Option<FoldRange>>,
    collapsed_fold_starts: HashSet<usize>,
    fold_visible_to_real: Vec<usize>,
    fold_real_to_visible: Vec<usize>,
    fold_hidden_owner: Vec<Option<usize>>,
    fold_placeholder_hidden_lines: Vec<Option<usize>>,
    line_has_fold_structure: Vec<bool>,
    fold_rescan_pending: bool,
    pending_fold_prefix_until: Option<Instant>,
    // Track which mode entered command bar from
    command_bar_from_normal: bool,
    // Clipboard watch
    clipboard_watch_enabled: bool,
    clipboard_watch_last_text: Option<String>,
    clipboard_watch_last_poll: Instant,
    // Undo/redo
    history: LineHistory,
    // Cached calc flags — avoid O(n) full-doc scans on every keystroke
    cached_has_builtin_formula: bool,
    cached_has_variable_assignment: bool,
}

impl TerminalApp {
    fn new_with_startup_metrics(
        db: &Db,
        opts: &TerminalOptions,
        vim_mode: bool,
        format_on_save: bool,
        markdown_autoformat: bool,
        checklist_auto_reorder: bool,
        variables_enabled: bool,
        render_palette: render::RenderPalette,
        date_format: String,
        date_time_format: String,
    ) -> Result<(Self, TerminalStartupMetrics), String> {
        let startup_begin = Instant::now();

        let note_begin = Instant::now();
        let mut active_note = select_note(db, opts)?;
        let loading_note = note_begin.elapsed();

        let lines = split_lines(&active_note.body);
        // The body has been split into `lines`; release the contiguous copy
        // to avoid carrying ~N bytes twice for large documents.
        active_note.body = String::new();
        let reminder_ghosts = load_note_reminder_ghosts(db, &active_note.id, &lines)?;

        let switcher_begin = Instant::now();
        let switcher_items = switcher::load_note_meta(db)?;
        let loading_switcher = switcher_begin.elapsed();

        let calc_engine = CalcEngine::new();
        let initial_has_builtin_formula =
            crate::editor_core::calc_plan::contains_builtin_formula(&lines);
        let initial_has_variable_assignment =
            crate::editor_core::calc_plan::contains_variable_assignment(&lines);
        let defer_initial_calc = lines.len() >= LARGE_DOC_CALC_DEFER_LINES
            && !initial_has_builtin_formula
            && !initial_has_variable_assignment;
        let calc_begin = Instant::now();
        let calc_data = if defer_initial_calc {
            CalcData {
                line_results: vec![None; lines.len()],
                cell_results: vec![Vec::new(); lines.len()],
                variable_names: Vec::new(),
            }
        } else {
            compute_calc_data(
                &calc_engine,
                &lines,
                variables_enabled && initial_has_variable_assignment,
                None,
            )
        };
        let loading_calc_engine = calc_begin.elapsed();

        let (prev_line_hashes, prev_line_has_assignment) = if defer_initial_calc {
            (Vec::new(), Vec::new())
        } else {
            (
                crate::editor_core::calc_plan::hash_lines(&lines),
                lines
                    .iter()
                    .map(|line| crate::editor_core::calc_plan::contains_assignment_operator(line))
                    .collect::<Vec<_>>(),
            )
        };
        let line_has_fold_structure = lines
            .iter()
            .map(|line| Self::line_has_fold_structure(line))
            .collect::<Vec<_>>();
        let history = LineHistory::new(MAX_UNDO_ENTRIES, &lines, 0, 0);
        let initial_mode = if vim_mode {
            UiMode::Normal
        } else {
            UiMode::Editor
        };
        let initial_status = if vim_mode {
            "-- NORMAL --  |  :cmd  Ctrl+F find  Ctrl+N new  Ctrl+P switch  Ctrl+Q quit".to_string()
        } else {
            format!("editing {}", active_note.id)
        };

        let mut app = Self {
            active_note,
            lines,
            cursor_line: 0,
            cursor_col: 0,
            scroll_line: 0,
            scroll_col: 0,
            mode: initial_mode,
            switcher_query: String::new(),
            switcher_items,
            switcher_matches: Vec::new(),
            switcher_selected: 0,
            switcher_delete_confirm: None,
            dirty: false,
            last_edit: Instant::now(),
            status: initial_status,
            command_input: String::new(),
            quit: false,
            force_quit: false,
            date_year: 0,
            date_month: 0,
            date_day: 0,
            date_hour: 0,
            date_minute: 0,
            date_include_time: false,
            date_require_time: false,
            date_picker_action: DatePickerAction::InsertDate,
            date_picker_return_mode: UiMode::Editor,
            date_format,
            date_time_format,
            vim_state: crate::editor_core::vim::VimState::default(),
            clipboard: Vec::new(),
            last_clipboard_backend: None,
            selection_anchor: None,
            command_selection: None,
            command_selection_linewise: false,
            calc_engine,
            calc_results: calc_data.line_results,
            cell_calc_results: calc_data.cell_results,
            reminder_ghosts,
            reminders_dirty: false,
            last_reminder_check: Instant::now(),
            variable_names: calc_data.variable_names,
            prev_line_hashes,
            prev_line_has_assignment,
            calc_state_stale: defer_initial_calc,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_current: 0,
            search_orig_line: 0,
            search_orig_col: 0,
            search_orig_scroll: 0,
            format_on_save,
            markdown_autoformat,
            checklist_auto_reorder,
            variables_enabled,
            render_palette,
            fold_ranges: Vec::new(),
            fold_range_by_start: Vec::new(),
            collapsed_fold_starts: HashSet::new(),
            fold_visible_to_real: Vec::new(),
            fold_real_to_visible: Vec::new(),
            fold_hidden_owner: Vec::new(),
            fold_placeholder_hidden_lines: Vec::new(),
            line_has_fold_structure,
            fold_rescan_pending: false,
            pending_fold_prefix_until: None,
            command_bar_from_normal: false,
            clipboard_watch_enabled: false,
            clipboard_watch_last_text: None,
            clipboard_watch_last_poll: Instant::now(),
            history,
            cached_has_builtin_formula: initial_has_builtin_formula,
            cached_has_variable_assignment: initial_has_variable_assignment,
        };

        app.recompute_folding();
        app.adjust_cursor();
        app.adjust_scroll();

        let metrics = TerminalStartupMetrics {
            loading_note,
            loading_switcher,
            loading_calc_engine,
            loading_screen: startup_begin.elapsed(),
        };

        Ok((app, metrics))
    }

    fn run(&mut self, db: &Db) -> Result<(), String> {
        let _guard = TerminalGuard::enter()?;
        let mut stdout = io::stdout();

        loop {
            self.draw(&mut stdout)?;
            if self.quit {
                break;
            }

            match input::read_key()? {
                Some(key) => self.handle_key(db, key)?,
                None => self.maybe_autosave(db)?,
            }

            self.maybe_clipboard_watch();
            self.sync_reminder_ghosts_if_dirty(db)?;
            self.maybe_dispatch_due_reminders(db);
        }

        if !self.force_quit {
            self.save(db)?;
        }
        Ok(())
    }

    fn maybe_autosave(&mut self, db: &Db) -> Result<(), String> {
        if self.dirty && self.last_edit.elapsed() >= Duration::from_millis(AUTOSAVE_DEBOUNCE_MS) {
            self.save(db)?;
            self.status = format!("autosaved {}", self.active_note.id);
        }
        Ok(())
    }

    fn start_clipboard_watch(&mut self) -> bool {
        if self.clipboard_watch_enabled {
            return false;
        }
        self.clipboard_watch_enabled = true;
        self.clipboard_watch_last_text = clipboard::read_clipboard_via_commands();
        self.clipboard_watch_last_poll =
            Instant::now() - Duration::from_millis(CLIPBOARD_WATCH_POLL_MS);
        true
    }

    fn stop_clipboard_watch(&mut self) -> bool {
        if !self.clipboard_watch_enabled {
            return false;
        }
        self.clipboard_watch_enabled = false;
        true
    }

    fn maybe_clipboard_watch(&mut self) {
        if !self.clipboard_watch_enabled {
            return;
        }
        if !matches!(self.mode, UiMode::Editor | UiMode::Normal) {
            return;
        }
        if self.clipboard_watch_last_poll.elapsed() < Duration::from_millis(CLIPBOARD_WATCH_POLL_MS)
        {
            return;
        }
        self.clipboard_watch_last_poll = Instant::now();

        let Some(text) = clipboard::read_clipboard_via_commands() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if self.clipboard_watch_last_text.as_deref() == Some(text.as_str()) {
            return;
        }

        self.clipboard_watch_last_text = Some(text.clone());
        let mut pasted = text;
        if !pasted.ends_with('\n') {
            pasted.push('\n');
        }
        self.insert_paste(&pasted);
        self.adjust_cursor();
        self.adjust_scroll();
        self.status = "clip-watch pasted".to_string();
    }

    fn handle_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        if self.mode != UiMode::Normal {
            self.pending_fold_prefix_until = None;
        }
        match self.mode {
            UiMode::DatePicker => self.handle_date_picker_key(db, key)?,
            UiMode::Editor => self.handle_editor_key(db, key)?,
            UiMode::Normal => self.handle_normal_key(db, key)?,
            UiMode::Visual | UiMode::VisualLine => self.handle_visual_key(db, key)?,
            UiMode::Switcher => self.handle_switcher_key(db, key)?,
            UiMode::CommandBar => self.handle_command_bar_key(db, key)?,
            UiMode::Search => self.handle_search_key(key)?,
        }
        Ok(())
    }

    fn handle_editor_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        let mut should_autoformat = false;
        let mut clamp_table_padding = true;
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
                self.save(db)?;
                self.status = format!("saved {}", self.active_note.id);
                return Ok(());
            }
            Key::Ctrl('n') => {
                self.save(db)?;
                let id = Ulid::new().to_string();
                let note = db.save_note(&id, "")?;
                self.set_active_note(db, note)?;
                self.refresh_switcher_items(db)?;
                self.status = format!("new note {}", self.active_note.id);
                return Ok(());
            }
            Key::Ctrl('p') => {
                self.open_switcher(db)?;
                return Ok(());
            }
            Key::ArrowUp => self.move_cursor_up(1),
            Key::ArrowDown => self.move_cursor_down(1),
            Key::ArrowLeft => self.move_cursor_left(),
            Key::ArrowRight => self.move_cursor_right(),
            Key::CtrlArrowLeft => {
                if !self.try_table_navigation_rule(true)
                    && !is_markdown_table_line(self.current_line())
                {
                    self.move_cursor_left_word();
                }
            }
            Key::CtrlArrowRight => {
                if !self.try_table_navigation_rule(false)
                    && !is_markdown_table_line(self.current_line())
                {
                    self.move_cursor_right_word();
                }
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
            }
            Key::PageUp => self.move_cursor_up(self.editor_height().saturating_sub(1)),
            Key::PageDown => self.move_cursor_down(self.editor_height().saturating_sub(1)),
            Key::Home => self.cursor_col = 0,
            Key::End => self.cursor_col = line_char_len(self.current_line()),
            Key::Backspace => {
                if let Some(changed) = self.try_table_boundary_edit_rule(true, false) {
                    should_autoformat = changed;
                } else {
                    self.backspace();
                    should_autoformat = true;
                }
            }
            Key::Delete => {
                if let Some(changed) = self.try_table_boundary_edit_rule(false, false) {
                    should_autoformat = changed;
                } else {
                    self.delete_forward();
                    should_autoformat = true;
                }
            }
            Key::Enter => {
                if !self.try_enter_rule() {
                    self.insert_newline();
                }
                should_autoformat = true;
            }
            Key::Tab => {
                if self.apply_calc_tab() {
                    should_autoformat = true;
                    // handled
                } else if !self.try_table_navigation_rule(false) && !self.try_tab_rule(false) {
                    self.insert_text("  ");
                    should_autoformat = true;
                }
            }
            Key::BackTab => {
                if !self.try_table_navigation_rule(true) && self.try_tab_rule(true) {
                    should_autoformat = true;
                }
            }
            Key::Ctrl('e') => {
                self.command_input.clear();
                self.command_bar_from_normal = false;
                self.command_selection = None;
                self.command_selection_linewise = false;
                self.mode = UiMode::CommandBar;
                self.status = ":".to_string();
                return Ok(());
            }
            Key::Ctrl('f') => {
                self.open_search();
                return Ok(());
            }
            Key::Paste(text) => {
                self.insert_paste(&text);
                // Pasted content should stay as-is; skip per-keystroke
                // autoformat pass that would otherwise scan the full document.
                should_autoformat = false;
            }
            Key::Char(ch) => {
                if ch == '|' && self.try_table_pipe_insert_column_rule() {
                    should_autoformat = false;
                    clamp_table_padding = false;
                } else {
                    self.insert_char(ch);
                    should_autoformat = true;
                }
                if ch == ' ' && is_markdown_table_line(self.current_line()) {
                    // Let users type multi-word table cell content without
                    // instant trim/realign fighting the cursor.
                    should_autoformat = false;
                    // Keep right-padding clamp relaxed for this keystroke so
                    // the next word can continue after the inserted space.
                    clamp_table_padding = false;
                }
            }
            Key::Esc => {
                self.mode = UiMode::Normal;
                self.vim_state = crate::editor_core::vim::VimState::default();
                self.status = "-- NORMAL --".to_string();
            }
            Key::Ctrl(_) => {}
        }

        self.adjust_cursor_with_table_padding_guard(clamp_table_padding);
        self.adjust_scroll();

        if should_autoformat {
            self.try_autoformat_rules();
        }

        Ok(())
    }

    fn map_vim_key(key: &Key) -> Option<crate::editor_core::vim::VimKey> {
        match key {
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
            Key::Ctrl(ch) => Some(crate::editor_core::vim::VimKey::Ctrl(*ch)),
            _ => None,
        }
    }

    fn set_clipboard_lines(&mut self, lines: Vec<String>) -> Option<ClipboardWriteBackend> {
        if lines.is_empty() {
            return None;
        }
        let joined = lines.join("\n");
        self.clipboard_watch_last_text = Some(joined.clone());
        let backend = clipboard::copy_text_to_clipboard(&joined);
        self.last_clipboard_backend = backend;
        self.clipboard = lines;
        backend
    }

    fn with_clipboard_status(&self, base: impl Into<String>) -> String {
        let base = base.into();
        match self.last_clipboard_backend {
            Some(backend) => format!("{base} [clipboard: {}]", backend.label()),
            None => format!("{base} [clipboard: local only]"),
        }
    }

    fn read_system_clipboard_lines(&self) -> Option<Vec<String>> {
        let text = clipboard::read_clipboard_via_commands().or_else(|| {
            if let Ok(mut ctx) = arboard::Clipboard::new() {
                ctx.get_text().ok()
            } else {
                None
            }
        })?;
        let lines = text.split('\n').map(|s| s.to_string()).collect::<Vec<_>>();
        if lines.is_empty() {
            None
        } else {
            Some(lines)
        }
    }

    fn slice_current_line_cols(&self, start_col: usize, end_col: usize) -> Option<String> {
        if start_col >= end_col {
            return None;
        }
        let line = self.current_line();
        let start = byte_index(line, start_col);
        let end = byte_index(line, end_col);
        if start >= end || end > line.len() {
            return None;
        }
        Some(line[start..end].to_string())
    }

    fn delete_current_line_cols(&mut self, start_col: usize, end_col: usize) -> Option<String> {
        if start_col >= end_col {
            return None;
        }
        let line = self.current_line().to_string();
        let start = byte_index(&line, start_col);
        let end = byte_index(&line, end_col);
        if start >= end || end > line.len() {
            return None;
        }
        let deleted = line[start..end].to_string();
        let mut updated = line;
        updated.replace_range(start..end, "");
        self.lines[self.cursor_line] = updated;
        self.cursor_col = start_col;
        Some(deleted)
    }

    fn find_word_object_bounds(&self, around: bool) -> Option<(usize, usize)> {
        let line = self.current_line();
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        if len == 0 {
            return None;
        }

        let mut idx = self.cursor_col.min(len.saturating_sub(1));
        if !is_word_char(chars[idx]) {
            if idx > 0 && is_word_char(chars[idx - 1]) {
                idx -= 1;
            } else {
                while idx < len && !is_word_char(chars[idx]) {
                    idx += 1;
                }
                if idx >= len {
                    return None;
                }
            }
        }

        let mut start = idx;
        while start > 0 && is_word_char(chars[start - 1]) {
            start -= 1;
        }
        let mut end = idx + 1;
        while end < len && is_word_char(chars[end]) {
            end += 1;
        }

        if around {
            let mut astart = start;
            let mut aend = end;
            while aend < len && chars[aend].is_whitespace() {
                aend += 1;
            }
            if aend == end {
                while astart > 0 && chars[astart - 1].is_whitespace() {
                    astart -= 1;
                }
            }
            start = astart;
            end = aend;
        }

        if start >= end {
            None
        } else {
            Some((start, end))
        }
    }

    fn find_pipe_object_bounds(&self, around: bool) -> Option<(usize, usize)> {
        let line = self.current_line();
        let chars: Vec<char> = line.chars().collect();
        if chars.len() < 2 {
            return None;
        }
        let pipes: Vec<usize> = chars
            .iter()
            .enumerate()
            .filter_map(|(idx, ch)| if *ch == '|' { Some(idx) } else { None })
            .collect();
        if pipes.len() < 2 {
            return None;
        }

        let cursor = self.cursor_col.min(chars.len());
        let mut pair = None;
        for window in pipes.windows(2) {
            let left = window[0];
            let right = window[1];
            if cursor == left || (cursor > left && cursor <= right) {
                pair = Some((left, right));
                break;
            }
        }
        let (left, right) = pair?;
        let start = if around { left } else { left + 1 };
        let end = if around { right + 1 } else { right };
        if start >= end {
            None
        } else {
            Some((start, end))
        }
    }

    fn apply_word_text_object(&mut self, around: bool, delete: bool, count: usize) -> usize {
        let mut chunks = Vec::new();
        let mut changed = false;
        let mut applied = 0usize;

        for _ in 0..count.max(1) {
            let Some((start, end)) = self.find_word_object_bounds(around) else {
                break;
            };
            if delete {
                if let Some(deleted) = self.delete_current_line_cols(start, end) {
                    chunks.push(deleted);
                    changed = true;
                    applied += 1;
                } else {
                    break;
                }
            } else if let Some(yanked) = self.slice_current_line_cols(start, end) {
                self.cursor_col = end.min(line_char_len(self.current_line()));
                chunks.push(yanked);
                applied += 1;
            } else {
                break;
            }
        }

        if chunks.is_empty() {
            return 0;
        }

        self.set_clipboard_lines(chunks);
        if changed {
            self.mark_edited();
            self.adjust_cursor();
        }
        applied
    }

    fn apply_pipe_text_object(&mut self, around: bool, delete: bool, count: usize) -> usize {
        let mut chunks = Vec::new();
        let mut changed = false;
        let mut applied = 0usize;

        for _ in 0..count.max(1) {
            let Some((start, end)) = self.find_pipe_object_bounds(around) else {
                break;
            };
            if delete {
                if let Some(deleted) = self.delete_current_line_cols(start, end) {
                    chunks.push(deleted);
                    changed = true;
                    applied += 1;
                } else {
                    break;
                }
            } else if let Some(yanked) = self.slice_current_line_cols(start, end) {
                self.cursor_col = end.min(line_char_len(self.current_line()));
                chunks.push(yanked);
                applied += 1;
            } else {
                break;
            }
        }

        if chunks.is_empty() {
            return 0;
        }

        self.set_clipboard_lines(chunks);
        if changed {
            self.mark_edited();
            self.adjust_cursor();
        }
        applied
    }

    fn apply_vim_actions(&mut self, actions: &[crate::editor_core::vim::VimAction]) {
        for action in actions {
            let count = action.count.max(1);
            match action.intent {
                crate::editor_core::vim::VimIntent::MoveLeft => {
                    for _ in 0..count {
                        self.move_cursor_left();
                    }
                }
                crate::editor_core::vim::VimIntent::MoveRight => {
                    for _ in 0..count {
                        self.move_cursor_right();
                    }
                }
                crate::editor_core::vim::VimIntent::MoveUp => self.move_cursor_up(count),
                crate::editor_core::vim::VimIntent::MoveDown => self.move_cursor_down(count),
                crate::editor_core::vim::VimIntent::MoveWordForward => {
                    for _ in 0..count {
                        self.move_cursor_right_word();
                    }
                }
                crate::editor_core::vim::VimIntent::MoveWordBackward => {
                    for _ in 0..count {
                        self.move_cursor_left_word();
                    }
                }
                crate::editor_core::vim::VimIntent::MoveLineStart => self.cursor_col = 0,
                crate::editor_core::vim::VimIntent::MoveLineEnd => {
                    let line_len = line_char_len(self.current_line());
                    self.cursor_col = line_len.saturating_sub(1);
                }
                crate::editor_core::vim::VimIntent::MoveDocStart => self.cursor_line = 0,
                crate::editor_core::vim::VimIntent::MoveDocEnd => {
                    self.cursor_line = self
                        .real_line_for_virtual(self.visible_line_count().saturating_sub(1))
                        .unwrap_or_else(|| self.lines.len().saturating_sub(1))
                }
                crate::editor_core::vim::VimIntent::MoveToLine => {
                    let target_virtual = count.max(1).min(self.visible_line_count()) - 1;
                    self.cursor_line = self
                        .real_line_for_virtual(target_virtual)
                        .unwrap_or_else(|| self.lines.len().saturating_sub(1));
                }
                crate::editor_core::vim::VimIntent::EnterInsert => {
                    self.mode = UiMode::Editor;
                    self.status = "-- INSERT --".to_string();
                }
                crate::editor_core::vim::VimIntent::AppendInsert => {
                    let line_len = line_char_len(self.current_line());
                    if self.cursor_col < line_len {
                        self.cursor_col += 1;
                    }
                    self.mode = UiMode::Editor;
                    self.status = "-- INSERT --".to_string();
                }
                crate::editor_core::vim::VimIntent::InsertLineStart => {
                    self.cursor_col = 0;
                    self.mode = UiMode::Editor;
                    self.status = "-- INSERT --".to_string();
                }
                crate::editor_core::vim::VimIntent::AppendLineEnd => {
                    self.cursor_col = line_char_len(self.current_line());
                    self.mode = UiMode::Editor;
                    self.status = "-- INSERT --".to_string();
                }
                crate::editor_core::vim::VimIntent::OpenLineBelow => {
                    self.cursor_col = line_char_len(self.current_line());
                    self.insert_newline();
                    self.mode = UiMode::Editor;
                    self.status = "-- INSERT --".to_string();
                }
                crate::editor_core::vim::VimIntent::OpenLineAbove => {
                    self.cursor_col = 0;
                    let current = self.cursor_line;
                    self.lines.insert(current, String::new());
                    self.mode = UiMode::Editor;
                    self.status = "-- INSERT --".to_string();
                }
                crate::editor_core::vim::VimIntent::EnterVisual => {
                    self.mode = UiMode::Visual;
                    self.selection_anchor = Some((self.cursor_line, self.cursor_col));
                    self.status = "-- VISUAL --".to_string();
                }
                crate::editor_core::vim::VimIntent::EnterVisualLine => {
                    self.mode = UiMode::VisualLine;
                    self.selection_anchor = Some((self.cursor_line, self.cursor_col));
                    self.status = "-- VISUAL LINE --".to_string();
                }
                crate::editor_core::vim::VimIntent::ExitVisual => {
                    if self.mode == UiMode::Visual || self.mode == UiMode::VisualLine {
                        self.mode = UiMode::Normal;
                        self.selection_anchor = None;
                        self.status = "-- NORMAL --".to_string();
                    }
                }
                crate::editor_core::vim::VimIntent::DeleteLine => {
                    let mut deleted = Vec::new();
                    for _ in 0..count {
                        if self.cursor_line < self.lines.len() {
                            deleted.push(self.lines.remove(self.cursor_line));
                        }
                    }
                    if self.lines.is_empty() {
                        self.lines.push(String::new());
                    }
                    if !deleted.is_empty() {
                        self.set_clipboard_lines(deleted);
                        self.status =
                            self.with_clipboard_status(format!("deleted {} lines", count));
                        self.mark_edited();
                        self.adjust_cursor();
                    }
                }
                crate::editor_core::vim::VimIntent::YankLine => {
                    let mut yanked = Vec::new();
                    for i in 0..count {
                        if self.cursor_line + i < self.lines.len() {
                            yanked.push(self.lines[self.cursor_line + i].clone());
                        }
                    }
                    if !yanked.is_empty() {
                        self.set_clipboard_lines(yanked);
                        self.status = self.with_clipboard_status(format!("yanked {} lines", count));
                    }
                }
                crate::editor_core::vim::VimIntent::DeleteToLineStart => {
                    let mut chunks = Vec::new();
                    for _ in 0..count {
                        if self.cursor_col == 0 {
                            break;
                        }
                        if let Some(deleted) = self.delete_current_line_cols(0, self.cursor_col) {
                            chunks.push(deleted);
                        } else {
                            break;
                        }
                    }
                    if !chunks.is_empty() {
                        self.set_clipboard_lines(chunks);
                        self.status = self.with_clipboard_status("deleted to line start");
                        self.mark_edited();
                        self.adjust_cursor();
                    }
                }
                crate::editor_core::vim::VimIntent::DeleteToLineEnd => {
                    let mut chunks = Vec::new();
                    for _ in 0..count {
                        let end_col = line_char_len(self.current_line());
                        if self.cursor_col >= end_col {
                            break;
                        }
                        if let Some(deleted) =
                            self.delete_current_line_cols(self.cursor_col, end_col)
                        {
                            chunks.push(deleted);
                        } else {
                            break;
                        }
                    }
                    if !chunks.is_empty() {
                        self.set_clipboard_lines(chunks);
                        self.status = self.with_clipboard_status("deleted to line end");
                        self.mark_edited();
                        self.adjust_cursor();
                    }
                }
                crate::editor_core::vim::VimIntent::YankToLineStart => {
                    let mut chunks = Vec::new();
                    for _ in 0..count {
                        if self.cursor_col == 0 {
                            break;
                        }
                        if let Some(yanked) = self.slice_current_line_cols(0, self.cursor_col) {
                            chunks.push(yanked);
                        } else {
                            break;
                        }
                    }
                    if !chunks.is_empty() {
                        self.set_clipboard_lines(chunks);
                        self.status = self.with_clipboard_status("yanked to line start");
                    }
                }
                crate::editor_core::vim::VimIntent::YankToLineEnd => {
                    let mut chunks = Vec::new();
                    for _ in 0..count {
                        let end_col = line_char_len(self.current_line());
                        if self.cursor_col >= end_col {
                            break;
                        }
                        if let Some(yanked) = self.slice_current_line_cols(self.cursor_col, end_col)
                        {
                            chunks.push(yanked);
                        } else {
                            break;
                        }
                    }
                    if !chunks.is_empty() {
                        self.set_clipboard_lines(chunks);
                        self.status = self.with_clipboard_status("yanked to line end");
                    }
                }
                crate::editor_core::vim::VimIntent::DeleteChar => {
                    for _ in 0..count {
                        self.delete_forward();
                    }
                }
                crate::editor_core::vim::VimIntent::PasteAfter => {
                    if let Some(sys_clip) = self.read_system_clipboard_lines() {
                        if sys_clip != self.clipboard || self.clipboard.is_empty() {
                            self.clipboard = sys_clip;
                        }
                    }
                    if !self.clipboard.is_empty() {
                        for _ in 0..count {
                            let mut insert_at = self.cursor_line;
                            if !self.lines[self.cursor_line].is_empty() {
                                insert_at += 1;
                            }
                            for (i, line) in self.clipboard.iter().enumerate() {
                                self.lines.insert(insert_at + i, line.clone());
                            }
                            self.cursor_line = insert_at + self.clipboard.len().saturating_sub(1);
                            self.cursor_col = 0;
                        }
                        self.mark_edited();
                    }
                }
                crate::editor_core::vim::VimIntent::DeleteInsideWord => {
                    let applied = self.apply_word_text_object(false, true, count);
                    if applied > 0 {
                        let msg = if applied == 1 {
                            "deleted inside word".to_string()
                        } else {
                            format!("deleted inside {} words", applied)
                        };
                        self.status = self.with_clipboard_status(msg);
                    }
                }
                crate::editor_core::vim::VimIntent::DeleteAroundWord => {
                    let applied = self.apply_word_text_object(true, true, count);
                    if applied > 0 {
                        let msg = if applied == 1 {
                            "deleted around word".to_string()
                        } else {
                            format!("deleted around {} words", applied)
                        };
                        self.status = self.with_clipboard_status(msg);
                    }
                }
                crate::editor_core::vim::VimIntent::YankInsideWord => {
                    let applied = self.apply_word_text_object(false, false, count);
                    if applied > 0 {
                        let msg = if applied == 1 {
                            "yanked inside word".to_string()
                        } else {
                            format!("yanked inside {} words", applied)
                        };
                        self.status = self.with_clipboard_status(msg);
                    }
                }
                crate::editor_core::vim::VimIntent::YankAroundWord => {
                    let applied = self.apply_word_text_object(true, false, count);
                    if applied > 0 {
                        let msg = if applied == 1 {
                            "yanked around word".to_string()
                        } else {
                            format!("yanked around {} words", applied)
                        };
                        self.status = self.with_clipboard_status(msg);
                    }
                }
                crate::editor_core::vim::VimIntent::DeleteInsidePipe => {
                    let applied = self.apply_pipe_text_object(false, true, count);
                    if applied > 0 {
                        let msg = if applied == 1 {
                            "deleted inside | |".to_string()
                        } else {
                            format!("deleted inside {} pipe ranges", applied)
                        };
                        self.status = self.with_clipboard_status(msg);
                    }
                }
                crate::editor_core::vim::VimIntent::DeleteAroundPipe => {
                    let applied = self.apply_pipe_text_object(true, true, count);
                    if applied > 0 {
                        let msg = if applied == 1 {
                            "deleted around | |".to_string()
                        } else {
                            format!("deleted around {} pipe ranges", applied)
                        };
                        self.status = self.with_clipboard_status(msg);
                    }
                }
                crate::editor_core::vim::VimIntent::YankInsidePipe => {
                    let applied = self.apply_pipe_text_object(false, false, count);
                    if applied > 0 {
                        let msg = if applied == 1 {
                            "yanked inside | |".to_string()
                        } else {
                            format!("yanked inside {} pipe ranges", applied)
                        };
                        self.status = self.with_clipboard_status(msg);
                    }
                }
                crate::editor_core::vim::VimIntent::YankAroundPipe => {
                    let applied = self.apply_pipe_text_object(true, false, count);
                    if applied > 0 {
                        let msg = if applied == 1 {
                            "yanked around | |".to_string()
                        } else {
                            format!("yanked around {} pipe ranges", applied)
                        };
                        self.status = self.with_clipboard_status(msg);
                    }
                }
                crate::editor_core::vim::VimIntent::Undo => {
                    for _ in 0..count {
                        self.undo();
                    }
                }
                crate::editor_core::vim::VimIntent::Redo => {
                    for _ in 0..count {
                        self.redo();
                    }
                }
                crate::editor_core::vim::VimIntent::OpenCommandBar => {
                    self.command_input.clear();
                    self.command_bar_from_normal = true;
                    self.command_selection = None;
                    self.command_selection_linewise = false;
                    self.mode = UiMode::CommandBar;
                    self.status = ":".to_string();
                }
                crate::editor_core::vim::VimIntent::OpenSearch => self.open_search(),
                crate::editor_core::vim::VimIntent::SearchNext => self.search_next(),
                crate::editor_core::vim::VimIntent::SearchPrev => self.search_prev(),
                crate::editor_core::vim::VimIntent::Swallow => {}
            }
        }
    }

    fn handle_normal_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        if key == Key::Ctrl('q') {
            self.quit = true;
            return Ok(());
        }

        if key == Key::Ctrl('p') {
            self.open_switcher(db)?;
            return Ok(());
        }

        let now = Instant::now();
        if self
            .pending_fold_prefix_until
            .is_some_and(|until| now > until)
        {
            self.pending_fold_prefix_until = None;
        }

        if self.pending_fold_prefix_until.take().is_some() {
            if key == Key::Char('a') {
                self.toggle_fold_at_cursor();
                return Ok(());
            }
        }

        if key == Key::Char('z') {
            self.pending_fold_prefix_until =
                Some(now + Duration::from_millis(FOLD_PREFIX_TIMEOUT_MS));
            return Ok(());
        }

        let Some(vim_key) = Self::map_vim_key(&key) else {
            return Ok(());
        };

        let context = crate::editor_core::vim::VimContext {
            has_search_matches: !self.search_matches.is_empty(),
            line_count: self.lines.len(),
        };
        let step = crate::editor_core::vim::step(&self.vim_state, vim_key, &context);
        self.vim_state = step.state;

        if !step.handled {
            return Ok(());
        }

        self.apply_vim_actions(&step.actions);
        if key == Key::Esc {
            self.search_matches.clear();
            self.search_query.clear();
            self.status = "-- NORMAL --".to_string();
        }

        self.adjust_cursor();
        self.adjust_scroll();

        if Self::line_might_trigger_doc_change_rules(self.current_line()) {
            let snapshot = self.build_snapshot();
            let options = crate::editor_core::text_rules::TextRuleOptions {
                markdown_autoformat: self.markdown_autoformat,
                checklist_auto_reorder: self.checklist_auto_reorder,
            };
            if let Some(op) =
                crate::editor_core::text_rules::run_doc_change_rules(&snapshot, options)
            {
                self.apply_edit_operation(&op);
            }
        }

        Ok(())
    }

    fn handle_visual_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        if key == Key::Ctrl('p') {
            self.open_switcher(db)?;
            return Ok(());
        }

        match key {
            Key::Esc | Key::Ctrl('c') => {
                self.mode = UiMode::Normal;
                self.vim_state.mode = crate::editor_core::vim::VimMode::Normal;
                self.selection_anchor = None;
                self.command_selection = None;
                self.status = "-- NORMAL --".to_string();
            }
            Key::Ctrl('e') => {
                self.command_selection_linewise = self.mode == UiMode::VisualLine;
                self.command_selection = self.capture_visual_command_selection();
                self.vim_state = crate::editor_core::vim::VimState::default();
                self.command_input.clear();
                self.command_bar_from_normal = true;
                self.mode = UiMode::CommandBar;
                self.status = ":".to_string();
            }
            Key::ArrowUp => self.move_cursor_up(1),
            Key::ArrowDown => self.move_cursor_down(1),
            Key::ArrowLeft => self.move_cursor_left(),
            Key::ArrowRight => self.move_cursor_right(),
            Key::Char(c) => match c {
                'h' => self.move_cursor_left(),
                'j' => self.move_cursor_down(1),
                'k' => self.move_cursor_up(1),
                'l' => self.move_cursor_right(),
                ':' => {
                    self.command_selection_linewise = self.mode == UiMode::VisualLine;
                    self.command_selection = self.capture_visual_command_selection();
                    self.vim_state = crate::editor_core::vim::VimState::default();
                    self.command_input.clear();
                    self.command_bar_from_normal = true;
                    self.mode = UiMode::CommandBar;
                    self.status = ":".to_string();
                }
                'w' => self.move_cursor_right_word(),
                'b' => self.move_cursor_left_word(),
                '$' => self.cursor_col = line_char_len(self.current_line()).saturating_sub(1),
                '0' => self.cursor_col = 0,
                'y' | 'd' | 'x' => {
                    let is_delete = c == 'd' || c == 'x';
                    let anchor = self
                        .selection_anchor
                        .unwrap_or((self.cursor_line, self.cursor_col));
                    let start_line = min(anchor.0, self.cursor_line);
                    let end_line = std::cmp::max(anchor.0, self.cursor_line);

                    let mut yanked = Vec::new();

                    if self.mode == UiMode::VisualLine {
                        for i in start_line..=end_line {
                            if i < self.lines.len() {
                                yanked.push(self.lines[i].clone());
                            }
                        }
                        if is_delete {
                            for _ in start_line..=end_line {
                                if start_line < self.lines.len() {
                                    self.lines.remove(start_line);
                                }
                            }
                            if self.lines.is_empty() {
                                self.lines.push(String::new());
                            }
                            self.cursor_line = start_line.min(self.lines.len().saturating_sub(1));
                            self.cursor_col = 0;
                        }
                    } else {
                        let (start_col, end_col) = if anchor.0 == self.cursor_line {
                            (
                                min(anchor.1, self.cursor_col),
                                std::cmp::max(anchor.1, self.cursor_col),
                            )
                        } else if anchor.0 < self.cursor_line {
                            (anchor.1, self.cursor_col)
                        } else {
                            (self.cursor_col, anchor.1)
                        };

                        if start_line == end_line {
                            let line = &self.lines[start_line];
                            let chars: Vec<char> = line.chars().collect();
                            let c_start = min(start_col, chars.len());
                            let c_end = min(end_col + 1, chars.len());

                            yanked.push(chars[c_start..c_end].iter().collect::<String>());

                            if is_delete {
                                let mut new_line: String = chars[..c_start].iter().collect();
                                let tail: String = chars[c_end..].iter().collect();
                                new_line.push_str(&tail);
                                self.lines[start_line] = new_line;
                                self.cursor_col = c_start;
                            }
                        } else {
                            let l1_chars: Vec<char> = self.lines[start_line].chars().collect();
                            let l1_start = min(start_col, l1_chars.len());
                            yanked.push(l1_chars[l1_start..].iter().collect::<String>());

                            for i in (start_line + 1)..end_line {
                                if i < self.lines.len() {
                                    yanked.push(self.lines[i].clone());
                                }
                            }

                            let ln_chars: Vec<char> = self.lines[end_line].chars().collect();
                            let ln_end = min(end_col + 1, ln_chars.len());
                            yanked.push(ln_chars[..ln_end].iter().collect::<String>());

                            if is_delete {
                                let mut new_l1: String = l1_chars[..l1_start].iter().collect();
                                let tail: String = ln_chars[ln_end..].iter().collect();
                                new_l1.push_str(&tail);

                                for _ in start_line..=end_line {
                                    if start_line < self.lines.len() {
                                        self.lines.remove(start_line);
                                    }
                                }
                                self.lines.insert(start_line, new_l1);
                                self.cursor_line = start_line;
                                self.cursor_col = l1_start;
                            }
                        }
                    }

                    if !yanked.is_empty() {
                        self.set_clipboard_lines(yanked);
                    }

                    self.mode = UiMode::Normal;
                    self.vim_state.mode = crate::editor_core::vim::VimMode::Normal;
                    self.selection_anchor = None;
                    self.status = if is_delete {
                        self.with_clipboard_status("-- NORMAL --")
                    } else {
                        self.with_clipboard_status("-- NORMAL -- (yanked)")
                    };
                    if is_delete {
                        self.mark_edited();
                    }
                }
                _ => {}
            },
            _ => {}
        }

        self.adjust_cursor();
        self.adjust_scroll();
        Ok(())
    }

    fn handle_switcher_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
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
                // simple word deletion for switcher
                while let Some(c) = self.switcher_query.chars().last() {
                    if !c.is_alphanumeric() {
                        self.switcher_query.pop();
                    } else {
                        break;
                    }
                }
                while let Some(c) = self.switcher_query.chars().last() {
                    if c.is_alphanumeric() {
                        self.switcher_query.pop();
                    } else {
                        break;
                    }
                }
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
                self.request_switcher_delete_confirmation();
            }
            Key::Enter => {
                if let Some(idx) = self.switcher_matches.get(self.switcher_selected).copied() {
                    let id = self.switcher_items[idx].id.clone();
                    let mut opened = false;
                    self.save(db)?;
                    if let Some(note) = db.get_note(&id)? {
                        self.set_active_note(db, note)?;
                        self.status = format!("opened {}", id);
                        opened = true;
                    } else {
                        self.status = format!("note missing {}", id);
                    }
                    self.close_switcher();
                    if opened {
                        self.mode = UiMode::Normal;
                        self.vim_state.mode = crate::editor_core::vim::VimMode::Normal;
                        self.selection_anchor = None;
                        self.command_selection = None;
                        self.status = "-- NORMAL --".to_string();
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
            Key::Tab
            | Key::CtrlDelete
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

    fn handle_switcher_delete_confirm_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        match key {
            Key::Ctrl('q') => {
                self.quit = true;
            }
            Key::Ctrl('p') => {
                self.close_switcher();
            }
            Key::Esc | Key::Char('n') | Key::Char('N') => {
                self.switcher_delete_confirm = None;
            }
            Key::Enter | Key::Char('y') | Key::Char('Y') => {
                if let Some(confirm) = self.switcher_delete_confirm.take() {
                    self.delete_note_from_switcher(db, &confirm.note_id, &confirm.note_title)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn request_switcher_delete_confirmation(&mut self) {
        if let Some(idx) = self.switcher_matches.get(self.switcher_selected).copied() {
            let item = &self.switcher_items[idx];
            self.switcher_delete_confirm = Some(SwitcherDeleteConfirm {
                note_id: item.id.clone(),
                note_title: item.title.clone(),
            });
        }
    }

    fn delete_note_from_switcher(
        &mut self,
        db: &Db,
        note_id: &str,
        note_title: &str,
    ) -> Result<(), String> {
        let deleting_active = self.active_note.id == note_id;
        let deleted = db.delete_note(note_id)?;
        if !deleted {
            self.status = format!("note missing {}", note_id);
            self.refresh_switcher_items(db)?;
            return Ok(());
        }

        if deleting_active {
            if let Some(note) = db.get_most_recent_note()? {
                self.set_active_note(db, note)?;
            } else {
                let id = Ulid::new().to_string();
                let note = db.save_note(&id, "")?;
                self.set_active_note(db, note)?;
            }
        } else {
            self.refresh_switcher_items(db)?;
        }

        self.status = format!("deleted {}", note_title);
        Ok(())
    }

    fn command_mode(&self) -> crate::editor_core::types::CommandMode {
        if self.command_bar_from_normal {
            crate::editor_core::types::CommandMode::Vim
        } else {
            crate::editor_core::types::CommandMode::Editor
        }
    }

    fn handle_command_bar_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        match key {
            Key::Esc => {
                self.mode = if self.command_bar_from_normal {
                    UiMode::Normal
                } else {
                    UiMode::Editor
                };
                self.command_input.clear();
                self.command_selection = None;
                self.command_selection_linewise = false;
                self.status = if self.command_bar_from_normal {
                    "-- NORMAL --".to_string()
                } else {
                    format!("editing {}", self.active_note.id)
                };
            }
            Key::Enter => {
                let cmd = self.command_input.trim().to_string();
                let return_to = if self.command_bar_from_normal {
                    UiMode::Normal
                } else {
                    UiMode::Editor
                };
                self.mode = return_to;
                self.command_input.clear();
                self.execute_terminal_command(db, &cmd);
                self.command_selection = None;
                self.command_selection_linewise = false;
            }
            Key::Tab => {
                let suggestions = crate::editor_core::commands::list_command_suggestions(
                    self.command_mode(),
                    &self.command_input,
                );
                if let Some(top) = suggestions.first() {
                    self.command_input = top.value.clone();
                    self.update_command_status();
                }
            }
            Key::Backspace => {
                self.command_input.pop();
                if self.command_input.is_empty() {
                    self.mode = if self.command_bar_from_normal {
                        UiMode::Normal
                    } else {
                        UiMode::Editor
                    };
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
            Key::Char(ch) => {
                self.command_input.push(ch);
                self.update_command_status();
            }
            Key::Paste(text) => {
                for ch in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                    self.command_input.push(ch);
                }
                self.update_command_status();
            }
            _ => {}
        }
        Ok(())
    }

    fn update_command_status(&mut self) {
        let suggestions = crate::editor_core::commands::list_command_suggestions(
            self.command_mode(),
            &self.command_input,
        );
        let hint = suggestions
            .iter()
            .take(3)
            .map(|s| s.value.as_str())
            .collect::<Vec<_>>()
            .join("  ");
        if hint.is_empty() {
            self.status = format!(":{}", self.command_input);
        } else {
            self.status = format!(":{}  [{}]", self.command_input, hint);
        }
    }

    fn execute_terminal_command(&mut self, db: &Db, cmd: &str) {
        if cmd == "q!" || cmd == "q" {
            self.force_quit = cmd == "q!";
            self.quit = true;
            return;
        }

        if let Some(command) =
            crate::editor_core::command_catalog::resolve_command(self.command_mode(), cmd)
        {
            match command.id {
                crate::editor_core::command_catalog::CommandId::Date => {
                    self.open_date_picker(DatePickerAction::InsertDate, false);
                    return;
                }
                crate::editor_core::command_catalog::CommandId::Notify => {
                    self.open_date_picker(DatePickerAction::SetNotify, true);
                    return;
                }
                crate::editor_core::command_catalog::CommandId::NotifyDelete => {
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
                crate::editor_core::command_catalog::CommandId::ClipWatch => {
                    if self.start_clipboard_watch() {
                        self.status = "clip-watch started".to_string();
                    } else {
                        self.status = "clip-watch already active".to_string();
                    }
                    return;
                }
                crate::editor_core::command_catalog::CommandId::ClipWatchStop => {
                    if self.stop_clipboard_watch() {
                        self.status = "clip-watch stopped".to_string();
                    } else {
                        self.status = "clip-watch not active".to_string();
                    }
                    return;
                }
                crate::editor_core::command_catalog::CommandId::Fold => {
                    self.set_fold_collapsed_at_cursor(true);
                    return;
                }
                crate::editor_core::command_catalog::CommandId::Unfold => {
                    self.set_fold_collapsed_at_cursor(false);
                    return;
                }
                crate::editor_core::command_catalog::CommandId::FoldToggle => {
                    self.toggle_fold_at_cursor();
                    return;
                }
                _ => {}
            }
        }

        let snapshot = self.build_snapshot();
        let result =
            crate::editor_core::commands::execute_command(&snapshot, cmd, self.command_mode());

        if result.quit_requested {
            self.quit = true;
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

    fn byte_offset_for_line_col(&self, line_idx: usize, col: usize) -> usize {
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

    fn capture_visual_command_selection(
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

    fn build_snapshot(&self) -> crate::editor_core::types::EditorContextSnapshot {
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

    fn open_date_picker(&mut self, action: DatePickerAction, require_time: bool) {
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

    fn close_date_picker(&mut self) {
        self.mode = self.date_picker_return_mode;
        self.status = if self.mode == UiMode::Normal {
            "-- NORMAL --".to_string()
        } else {
            format!("editing {}", self.active_note.id)
        };
    }

    fn adjust_picker_hour(&mut self, delta: i32) {
        let mut next = self.date_hour as i32 + delta;
        while next < 0 {
            next += 24;
        }
        while next >= 24 {
            next -= 24;
        }
        self.date_hour = next as u32;
    }

    fn adjust_picker_minute(&mut self, delta: i32) {
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

    fn handle_date_picker_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
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

    fn open_switcher(&mut self, db: &Db) -> Result<(), String> {
        self.refresh_switcher_items(db)?;
        self.mode = UiMode::Switcher;
        self.switcher_query.clear();
        self.recompute_switcher_matches();
        self.switcher_delete_confirm = None;
        self.status =
            "Switcher: type to filter, Enter open, Delete/Ctrl+Backspace delete, Esc close"
                .to_string();
        Ok(())
    }

    fn close_switcher(&mut self) {
        self.mode = UiMode::Editor;
        self.switcher_query.clear();
        self.switcher_matches.clear();
        self.switcher_selected = 0;
        self.switcher_delete_confirm = None;
        self.status = format!("editing {}", self.active_note.id);
    }

    fn recompute_switcher_matches(&mut self) {
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

    fn save(&mut self, db: &Db) -> Result<(), String> {
        if self.format_on_save {
            self.execute_terminal_command(db, "format");
        }
        if !self.dirty {
            return Ok(());
        }
        self.sync_reminder_ghosts_if_dirty(db)?;
        let body = join_lines(&self.lines);
        let mut saved = db.save_note(&self.active_note.id, &body)?;
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

    fn sync_reminder_ghosts_if_dirty(&mut self, db: &Db) -> Result<(), String> {
        if !self.reminders_dirty {
            return Ok(());
        }
        self.reminder_ghosts = load_note_reminder_ghosts(db, &self.active_note.id, &self.lines)?;
        self.reminders_dirty = false;
        Ok(())
    }

    fn maybe_dispatch_due_reminders(&mut self, db: &Db) {
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

    fn refresh_switcher_items(&mut self, db: &Db) -> Result<(), String> {
        self.switcher_items = switcher::load_note_meta(db)?;
        if self.mode == UiMode::Switcher {
            self.recompute_switcher_matches();
        }
        Ok(())
    }

    fn set_active_note(&mut self, db: &Db, note: Note) -> Result<(), String> {
        self.active_note = note;
        self.lines = split_lines(&self.active_note.body);
        self.active_note.body = String::new();
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
        self.history
            .reset(&self.lines, self.cursor_line, self.cursor_col);
        self.rescan_calc_flags();
        if self.should_defer_calc_recompute() {
            self.calc_results = vec![None; self.lines.len()];
            self.cell_calc_results = vec![Vec::new(); self.lines.len()];
            self.variable_names.clear();
            self.prev_line_hashes.clear();
            self.prev_line_has_assignment.clear();
            self.calc_state_stale = true;
        } else {
            self.recompute_calc_full();
        }
        self.recompute_folding();
        self.adjust_cursor();
        self.adjust_scroll();
        self.history
            .checkpoint(&self.lines, self.cursor_line, self.cursor_col);
        Ok(())
    }

    fn current_line(&self) -> &str {
        self.lines
            .get(self.cursor_line)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    fn current_line_mut(&mut self) -> &mut String {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        &mut self.lines[self.cursor_line]
    }

    fn rescan_calc_flags(&mut self) {
        self.cached_has_builtin_formula =
            crate::editor_core::calc_plan::contains_builtin_formula(&self.lines);
        self.cached_has_variable_assignment =
            crate::editor_core::calc_plan::contains_variable_assignment(&self.lines);
    }

    fn update_calc_flags_incremental(&mut self) {
        // If a flag is already true, only a full rescan can turn it off. We
        // only rescan when line count changes (structural edit), since
        // single-line edits that remove the last `:=` or formula are rare
        // and the flag being stale-true just means we fall back to full calc
        // (correct, slightly slower) until the next structural edit.
        if self.lines.len() != self.calc_results.len() {
            self.rescan_calc_flags();
            return;
        }
        // Flag is false — check only the edited line for a new signal.
        let line = self.cursor_line;
        if !self.cached_has_variable_assignment {
            if let Some(text) = self.lines.get(line) {
                if crate::editor_core::calc_plan::contains_variable_assignment(
                    std::slice::from_ref(text),
                ) {
                    self.cached_has_variable_assignment = true;
                }
            }
        }
        if !self.cached_has_builtin_formula {
            if let Some(text) = self.lines.get(line) {
                if crate::editor_core::calc_plan::contains_builtin_formula(std::slice::from_ref(
                    text,
                )) {
                    self.cached_has_builtin_formula = true;
                }
            }
        }
    }

    fn line_has_fold_structure(text: &str) -> bool {
        let trimmed = text.trim_start();
        trimmed.starts_with('#')
            || trimmed.starts_with("```")
            || trimmed.starts_with("~~~")
            || (trimmed.starts_with('|') && trimmed.ends_with('|'))
            || crate::editor_core::markdown_tokens::list_marker_end(text).is_some()
    }

    fn calc_variables_enabled(&self) -> bool {
        self.variables_enabled && self.cached_has_variable_assignment
    }

    fn should_defer_calc_recompute(&self) -> bool {
        self.lines.len() >= LARGE_DOC_CALC_DEFER_LINES
            && !self.cached_has_builtin_formula
            && !self.cached_has_variable_assignment
    }

    fn defer_calc_state_after_edit(&mut self) {
        // Large docs without explicit calc syntax should not recompute calc
        // state on every keystroke.
        // Clear the full cache so same-line-count multi-line edits cannot
        // leave stale calc ghosts on non-cursor lines.
        if self.calc_results.len() != self.lines.len() {
            self.calc_results = vec![None; self.lines.len()];
        } else {
            self.calc_results.fill(None);
        }
        if self.cell_calc_results.len() != self.lines.len() {
            self.cell_calc_results = vec![Vec::new(); self.lines.len()];
        } else {
            for row in &mut self.cell_calc_results {
                row.clear();
            }
        }
        self.variable_names.clear();
        self.calc_state_stale = true;
    }

    fn recompute_folding_if_needed(&mut self) {
        if self.fold_rescan_pending || self.lines.len() != self.line_has_fold_structure.len() {
            self.fold_rescan_pending = false;
            self.recompute_folding();
            return;
        }

        // Fast path: only the cursor line can affect folding for single-line edits.
        let cl = self.cursor_line.min(self.lines.len().saturating_sub(1));
        let Some(current_line) = self.lines.get(cl) else {
            self.recompute_folding();
            return;
        };
        let next_flag = Self::line_has_fold_structure(current_line);
        let prev_flag = self
            .line_has_fold_structure
            .get(cl)
            .copied()
            .unwrap_or(false);
        if next_flag || prev_flag {
            // Edits within fold-relevant lines (e.g. heading level changes)
            // can alter fold ranges even when the flag itself doesn't change.
            self.recompute_folding();
        }
    }

    fn recompute_folding(&mut self) {
        self.fold_rescan_pending = false;
        self.line_has_fold_structure = self
            .lines
            .iter()
            .map(|line| Self::line_has_fold_structure(line))
            .collect();
        self.fold_ranges = folding::build_fold_ranges(&self.lines);
        self.fold_range_by_start = vec![None; self.lines.len()];
        for range in &self.fold_ranges {
            if range.start_line < self.fold_range_by_start.len() {
                self.fold_range_by_start[range.start_line] = Some(*range);
            }
        }
        self.collapsed_fold_starts.retain(|line| {
            self.fold_range_by_start
                .get(*line)
                .is_some_and(|entry| entry.is_some())
        });
        self.rebuild_fold_view_map();
    }

    fn rebuild_fold_view_map(&mut self) {
        let line_count = self.lines.len();
        self.fold_visible_to_real.clear();
        self.fold_visible_to_real.reserve(line_count);
        self.fold_real_to_visible = vec![0; line_count];
        self.fold_hidden_owner = vec![None; line_count];
        self.fold_placeholder_hidden_lines = vec![None; line_count];

        if line_count == 0 {
            return;
        }

        let mut collapsed_ranges = self
            .collapsed_fold_starts
            .iter()
            .filter_map(|start| {
                self.fold_range_by_start
                    .get(*start)
                    .and_then(|entry| *entry)
            })
            .collect::<Vec<_>>();
        collapsed_ranges.sort_by_key(|range| (range.start_line, range.end_line));

        let mut effective = Vec::new();
        let mut covered_to: Option<usize> = None;
        for range in collapsed_ranges {
            if range.end_line <= range.start_line {
                continue;
            }
            if covered_to.is_some_and(|last_end| range.start_line <= last_end) {
                continue;
            }
            covered_to = Some(range.end_line);
            effective.push(range);
        }

        let mut effective_idx = 0usize;
        let mut real_line = 0usize;
        while real_line < line_count {
            let visible_idx = self.fold_visible_to_real.len();
            self.fold_visible_to_real.push(real_line);
            self.fold_real_to_visible[real_line] = visible_idx;

            let collapse_here = effective
                .get(effective_idx)
                .copied()
                .filter(|range| range.start_line == real_line);
            if let Some(range) = collapse_here {
                let hidden_end = range.end_line.min(line_count.saturating_sub(1));
                if hidden_end > real_line {
                    self.fold_placeholder_hidden_lines[real_line] =
                        Some(hidden_end.saturating_sub(real_line));
                    for hidden_line in (real_line + 1)..=hidden_end {
                        self.fold_hidden_owner[hidden_line] = Some(real_line);
                        self.fold_real_to_visible[hidden_line] = visible_idx;
                    }
                    real_line = hidden_end + 1;
                } else {
                    real_line += 1;
                }
                effective_idx += 1;
            } else {
                real_line += 1;
            }
        }

        if self.fold_visible_to_real.is_empty() {
            self.fold_visible_to_real.push(0);
        }
    }

    fn visible_line_count(&self) -> usize {
        self.fold_visible_to_real.len().max(1)
    }

    fn current_virtual_line(&self) -> usize {
        self.fold_real_to_visible
            .get(self.cursor_line)
            .copied()
            .unwrap_or(0)
    }

    fn real_line_for_virtual(&self, virtual_line: usize) -> Option<usize> {
        self.fold_visible_to_real.get(virtual_line).copied()
    }

    fn fold_hidden_owner_for_line(&self, line: usize) -> Option<usize> {
        self.fold_hidden_owner.get(line).and_then(|owner| *owner)
    }

    fn fold_start_for_line(&self, line: usize) -> Option<usize> {
        if let Some(owner) = self.fold_hidden_owner_for_line(line) {
            return Some(owner);
        }
        if self
            .fold_range_by_start
            .get(line)
            .is_some_and(|entry| entry.is_some())
        {
            return Some(line);
        }

        let mut best_start = None;
        let mut best_span = usize::MAX;
        for range in &self.fold_ranges {
            if range.start_line < line && line <= range.end_line {
                let span = range.end_line.saturating_sub(range.start_line);
                if span < best_span {
                    best_span = span;
                    best_start = Some(range.start_line);
                }
            }
        }
        best_start
    }

    fn toggle_fold_at_cursor(&mut self) -> bool {
        let line = self.cursor_line.min(self.lines.len().saturating_sub(1));
        let Some(start_line) = self.fold_start_for_line(line) else {
            self.status = "fold: no foldable block at cursor".to_string();
            return false;
        };
        let next_collapsed = !self.collapsed_fold_starts.contains(&start_line);
        self.set_fold_collapsed_at_line(start_line, next_collapsed)
    }

    fn set_fold_collapsed_at_cursor(&mut self, collapsed: bool) -> bool {
        let line = self.cursor_line.min(self.lines.len().saturating_sub(1));
        let Some(start_line) = self.fold_start_for_line(line) else {
            self.status = "fold: no foldable block at cursor".to_string();
            return false;
        };
        self.set_fold_collapsed_at_line(start_line, collapsed)
    }

    fn set_fold_collapsed_at_line(&mut self, start_line: usize, collapsed: bool) -> bool {
        let Some(range) = self
            .fold_range_by_start
            .get(start_line)
            .and_then(|entry| *entry)
        else {
            self.status = "fold: no foldable block at cursor".to_string();
            return false;
        };

        let was_collapsed = self.collapsed_fold_starts.contains(&start_line);
        if was_collapsed == collapsed {
            self.status = if collapsed {
                "fold: already folded".to_string()
            } else {
                "fold: already unfolded".to_string()
            };
            return false;
        }

        let action = if collapsed {
            self.collapsed_fold_starts.insert(start_line);
            "folded"
        } else {
            self.collapsed_fold_starts.remove(&start_line);
            "unfolded"
        };

        self.rebuild_fold_view_map();
        self.adjust_cursor();
        self.adjust_scroll();

        let kind = match range.kind {
            FoldKind::Heading => "heading",
            FoldKind::Fence => "code block",
            FoldKind::List => "list",
            FoldKind::Table => "table",
            FoldKind::Paragraph => "paragraph",
        };
        let hidden = range.end_line.saturating_sub(range.start_line);
        self.status = format!("fold: {action} {kind} ({hidden} lines)");
        true
    }

    fn mark_edited(&mut self) {
        let coalesce_undo = self.last_edit.elapsed() < Duration::from_millis(UNDO_DEBOUNCE_MS);
        self.dirty = true;
        if !self.reminder_ghosts.is_empty() {
            self.reminders_dirty = true;
        }
        self.update_calc_flags_incremental();
        self.recompute_folding_if_needed();
        if self.should_defer_calc_recompute() {
            self.defer_calc_state_after_edit();
        } else {
            self.recompute_calc_full();
        }
        self.history.record_edit(
            &self.lines,
            self.cursor_line,
            self.cursor_col,
            coalesce_undo,
        );
        self.last_edit = Instant::now();
    }

    fn undo(&mut self) {
        let keep_cursor_on_exhaust = self.history.undo_depth() == 1;
        let cursor_before_undo = (self.cursor_line, self.cursor_col);
        if let Some(cursor) = self.history.undo(&mut self.lines) {
            if keep_cursor_on_exhaust {
                self.cursor_line = cursor_before_undo.0.min(self.lines.len().saturating_sub(1));
                self.cursor_col = cursor_before_undo.1;
            } else {
                self.cursor_line = cursor.line.min(self.lines.len().saturating_sub(1));
                self.cursor_col = cursor.col;
            }
            self.dirty = true;
            self.last_edit = Instant::now();
            if !self.reminder_ghosts.is_empty() {
                self.reminders_dirty = true;
            }
            self.recompute_calc_full();
            self.recompute_folding();
            self.adjust_cursor();
            self.adjust_scroll();
            self.history
                .checkpoint(&self.lines, self.cursor_line, self.cursor_col);
            self.status = format!("undo ({} left)", self.history.undo_depth());
        } else {
            self.status = "already at oldest change".to_string();
        }
    }

    fn redo(&mut self) {
        if let Some(cursor) = self.history.redo(&mut self.lines) {
            self.cursor_line = cursor.line.min(self.lines.len().saturating_sub(1));
            self.cursor_col = cursor.col;
            self.dirty = true;
            self.last_edit = Instant::now();
            if !self.reminder_ghosts.is_empty() {
                self.reminders_dirty = true;
            }
            self.recompute_calc_full();
            self.recompute_folding();
            self.adjust_cursor();
            self.adjust_scroll();
            self.history
                .checkpoint(&self.lines, self.cursor_line, self.cursor_col);
            self.status = format!("redo ({} left)", self.history.redo_depth());
        } else {
            self.status = "already at newest change".to_string();
        }
    }

    fn recompute_calc_full(&mut self) {
        let calc_variables_enabled = self.calc_variables_enabled();
        if self.calc_state_stale {
            let calc_data =
                compute_calc_data(&self.calc_engine, &self.lines, calc_variables_enabled, None);
            self.prev_line_hashes = crate::editor_core::calc_plan::hash_lines(&self.lines);
            self.prev_line_has_assignment = self
                .lines
                .iter()
                .map(|line| crate::editor_core::calc_plan::contains_assignment_operator(line))
                .collect();
            self.calc_results = calc_data.line_results;
            self.cell_calc_results = calc_data.cell_results;
            self.variable_names = calc_data.variable_names;
            self.calc_state_stale = false;
            return;
        }

        let next_hashes = crate::editor_core::calc_plan::hash_lines(&self.lines);
        let plan = crate::editor_core::calc_plan::plan_incremental_calc_from_hashes(
            &self.prev_line_hashes,
            &self.calc_results,
            &self.lines,
            &next_hashes,
        );
        let has_prev = !self.prev_line_hashes.is_empty();
        let has_builtin_formula = self.cached_has_builtin_formula;

        // Only scan the changed region for variable assignments (not all lines).
        let suffix_len = self.lines.len().saturating_sub(plan.eval_to);
        let prev_changed_from = plan.eval_from.min(self.prev_line_hashes.len());
        let prev_changed_to = self
            .prev_line_hashes
            .len()
            .saturating_sub(suffix_len)
            .max(prev_changed_from);
        let prev_changed_had_assignment = self
            .prev_line_has_assignment
            .get(prev_changed_from..prev_changed_to)
            .map(|slice| slice.iter().any(|&flag| flag))
            .unwrap_or(false);
        let touches_any_assignment = calc_variables_enabled
            && (crate::editor_core::calc_plan::contains_variable_assignment(&plan.eval_lines)
                || prev_changed_had_assignment);
        let can_use_partial = has_prev && !touches_any_assignment && !has_builtin_formula;

        let (mut new_results, mut new_cell_results, variable_names) = if can_use_partial {
            let mut merged_results = vec![None; self.lines.len()];
            for entry in &plan.base_results {
                if let Some(slot) = merged_results.get_mut(entry.line_idx) {
                    *slot = Some(entry.result.clone());
                }
            }
            // Carry forward cached cell results for unchanged lines (same
            // alignment as base_results, which the planner already validated).
            let mut merged_cells: Vec<Vec<(usize, String)>> = vec![Vec::new(); self.lines.len()];
            for entry in &plan.base_results {
                if let Some(slot) = merged_cells.get_mut(entry.line_idx) {
                    if let Some(cached) = self.cell_calc_results.get(entry.line_idx) {
                        *slot = cached.clone();
                    }
                }
            }

            if plan.eval_from < plan.eval_to {
                let calc_data = compute_calc_data(
                    &self.calc_engine,
                    &self.lines,
                    calc_variables_enabled,
                    Some((plan.eval_from, plan.eval_to)),
                );
                for idx in plan.eval_from..plan.eval_to {
                    if let Some(slot) = merged_results.get_mut(idx) {
                        *slot = calc_data.line_results.get(idx).cloned().unwrap_or(None);
                    }
                    if let Some(slot) = merged_cells.get_mut(idx) {
                        *slot = calc_data.cell_results.get(idx).cloned().unwrap_or_default();
                    }
                }
                (merged_results, merged_cells, calc_data.variable_names)
            } else {
                (merged_results, merged_cells, self.variable_names.clone())
            }
        } else {
            let calc_data =
                compute_calc_data(&self.calc_engine, &self.lines, calc_variables_enabled, None);
            (
                calc_data.line_results,
                calc_data.cell_results,
                calc_data.variable_names,
            )
        };

        // Auto-refresh committed-style trailers. Eligibility is deliberately
        // conservative — it requires that the line is byte-identical to the
        // snapshot taken at the end of the previous recompute AND that the
        // previous recompute returned `None` for the line. A `None` result
        // from the calc engine means "the trailing ` = <literal>` already
        // matches what the left side evaluates to", so prev-None is the
        // signal that the trailer was in sync. When a subsequent recompute
        // reports `Some(new_result)` for the same untouched line, the left
        // side has drifted (typically because of an upstream variable
        // change) and we rewrite the trailer in place.
        //
        // Length mismatches (note switch, undo/redo, Enter, paste, line
        // delete) invalidate per-index alignment; we skip the pass and
        // reseed the snapshot below, so eligibility returns on the next
        // recompute once the user resumes normal in-line editing.
        let aligned = self.prev_line_hashes.len() == self.lines.len()
            && self.calc_results.len() == self.lines.len();

        let mut final_hashes = next_hashes;
        if aligned {
            let cursor_line = self.cursor_line;
            let cursor_col = self.cursor_col;
            let selection_range: Option<(usize, usize)> = if matches!(
                self.mode,
                UiMode::Visual | UiMode::VisualLine | UiMode::CommandBar
            ) {
                self.selection_anchor.map(|(anchor_line, _)| {
                    let a = anchor_line.min(cursor_line);
                    let b = anchor_line.max(cursor_line);
                    (a, b)
                })
            } else {
                None
            };

            for i in 0..self.lines.len() {
                let Some(new_result) = new_results[i].as_deref() else {
                    continue;
                };
                if self.prev_line_hashes[i] != final_hashes[i] {
                    continue;
                }
                if self.calc_results[i].is_some() {
                    // Previous recompute already considered this line stale;
                    // not eligible for auto-refresh (user hand-typed or
                    // otherwise never-synced trailer).
                    continue;
                }
                if let Some((a, b)) = selection_range {
                    if a <= i && i <= b {
                        continue;
                    }
                }
                let refresh = compute_calc_trailer_refresh(
                    &self.lines[i],
                    new_result,
                    cursor_line == i,
                    cursor_col,
                );
                if let Some((eq_idx, new_tail)) = refresh {
                    self.lines[i].replace_range(eq_idx.., &new_tail);
                    // Line is back in sync with the backend, reflect it in
                    // the cached result so the ghost widget disappears and
                    // the next eligibility round still sees prev-None here.
                    new_results[i] = None;
                    if let Some(slot) = new_cell_results.get_mut(i) {
                        slot.clear();
                    }
                    // Trailer rewrite changed the line bytes; rehash so the
                    // snapshot stays in sync for the next recompute.
                    final_hashes[i] = crate::editor_core::calc_plan::hash_line(&self.lines[i]);
                }
            }
        }

        self.prev_line_hashes = final_hashes;
        self.prev_line_has_assignment = self
            .lines
            .iter()
            .map(|line| crate::editor_core::calc_plan::contains_assignment_operator(line))
            .collect();
        self.calc_results = new_results;
        self.cell_calc_results = new_cell_results;
        self.variable_names = variable_names;
        self.calc_state_stale = false;
    }

    // --- Search ---

    fn open_search(&mut self) {
        self.search_query.clear();
        self.search_matches.clear();
        self.search_current = 0;
        self.search_orig_line = self.cursor_line;
        self.search_orig_col = self.cursor_col;
        self.search_orig_scroll = self.scroll_line;
        self.mode = UiMode::Search;
        self.status = "/".to_string();
    }

    fn handle_search_key(&mut self, key: Key) -> Result<(), String> {
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
                while self
                    .search_query
                    .chars()
                    .last()
                    .is_some_and(|c| !c.is_alphanumeric())
                {
                    self.search_query.pop();
                }
                while self
                    .search_query
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_alphanumeric())
                {
                    self.search_query.pop();
                }
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

    fn recompute_search(&mut self) {
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

    fn update_search_status(&mut self) {
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

    fn jump_to_nearest_match(&mut self) {
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

    fn jump_to_current_match(&mut self) {
        if let Some(&(line, col, _)) = self.search_matches.get(self.search_current) {
            self.cursor_line = line;
            self.cursor_col = col;
            self.adjust_cursor();
            self.adjust_scroll();
        }
    }

    fn search_next(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_current = (self.search_current + 1) % self.search_matches.len();
        self.jump_to_current_match();
        self.update_search_status();
    }

    fn search_prev(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_current =
            (self.search_current + self.search_matches.len() - 1) % self.search_matches.len();
        self.jump_to_current_match();
        self.update_search_status();
    }

    fn search_highlights_for_line(
        &self,
        line_idx: usize,
    ) -> (Vec<(usize, usize)>, Vec<(usize, usize)>) {
        let mut matches = Vec::new();
        let mut current = Vec::new();
        for (idx, &(line, start, end)) in self.search_matches.iter().enumerate() {
            if line != line_idx {
                continue;
            }
            if idx == self.search_current {
                current.push((start, end));
            } else {
                matches.push((start, end));
            }
        }
        (matches, current)
    }

    fn line_is_in_visual_selection(&self, line_idx: usize) -> bool {
        let Some(anchor) = self.selection_anchor else {
            return false;
        };
        let has_visual_selection = self.mode == UiMode::Visual
            || self.mode == UiMode::VisualLine
            || (self.mode == UiMode::CommandBar && self.command_selection.is_some());
        if !has_visual_selection {
            return false;
        }

        let start_line = min(anchor.0, self.cursor_line);
        let end_line = std::cmp::max(anchor.0, self.cursor_line);
        line_idx >= start_line && line_idx <= end_line
    }

    fn append_visual_highlights(&self, line_idx: usize, ranges: &mut Vec<(usize, usize)>) {
        let Some(anchor) = self.selection_anchor else {
            return;
        };
        let has_visual_selection = self.mode == UiMode::Visual
            || self.mode == UiMode::VisualLine
            || (self.mode == UiMode::CommandBar && self.command_selection.is_some());
        if !has_visual_selection {
            return;
        }

        let start_line = min(anchor.0, self.cursor_line);
        let end_line = std::cmp::max(anchor.0, self.cursor_line);

        if line_idx < start_line || line_idx > end_line {
            return;
        }

        let linewise = self.mode == UiMode::VisualLine
            || (self.mode == UiMode::CommandBar && self.command_selection_linewise);
        if linewise {
            let line_len = self.lines[line_idx].chars().count();
            ranges.push((0, line_len.max(1)));
            return;
        }

        let (start_col, end_col) = if anchor.0 == self.cursor_line {
            (
                min(anchor.1, self.cursor_col),
                std::cmp::max(anchor.1, self.cursor_col),
            )
        } else if anchor.0 < self.cursor_line {
            (anchor.1, self.cursor_col)
        } else {
            (self.cursor_col, anchor.1)
        };

        if start_line == end_line {
            ranges.push((start_col, end_col + 1));
        } else if line_idx == start_line {
            let line_len = self.lines[line_idx].chars().count();
            ranges.push((start_col, line_len.max(start_col + 1)));
        } else if line_idx == end_line {
            ranges.push((0, end_col + 1));
        } else {
            let line_len = self.lines[line_idx].chars().count();
            ranges.push((0, line_len.max(1)));
        }
    }

    fn move_cursor_left_word(&mut self) {
        if self.cursor_col == 0 {
            let current_virtual = self.current_virtual_line();
            if current_virtual > 0 {
                if let Some(prev_real) = self.real_line_for_virtual(current_virtual - 1) {
                    self.cursor_line = prev_real;
                    self.cursor_col = line_char_len(self.current_line());
                }
            }
            return;
        }
        let line = self.current_line();
        let chars: Vec<char> = line.chars().collect();
        let mut col = self.cursor_col;
        while col > 0 && chars.get(col - 1).map_or(false, |c| !c.is_alphanumeric()) {
            col -= 1;
        }
        while col > 0 && chars.get(col - 1).map_or(false, |c| c.is_alphanumeric()) {
            col -= 1;
        }
        self.cursor_col = col;
    }

    fn move_cursor_right_word(&mut self) {
        let line = self.current_line();
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        if self.cursor_col == len {
            let current_virtual = self.current_virtual_line();
            if current_virtual + 1 < self.visible_line_count() {
                if let Some(next_real) = self.real_line_for_virtual(current_virtual + 1) {
                    self.cursor_line = next_real;
                    self.cursor_col = 0;
                }
            }
            return;
        }
        let mut col = self.cursor_col;
        while col < len && chars.get(col).map_or(false, |c| c.is_alphanumeric()) {
            col += 1;
        }
        while col < len && chars.get(col).map_or(false, |c| !c.is_alphanumeric()) {
            col += 1;
        }
        self.cursor_col = col;
    }

    fn delete_word_backward(&mut self) -> bool {
        if self.cursor_col == 0 {
            if self.cursor_line > 0 {
                self.backspace();
                return true;
            }
            return false;
        }
        if let Some(cell) = table_cell_info_at_char(self.current_line(), self.cursor_col) {
            let edit_start = table_cell_edit_start(&cell);
            let edit_end = table_cell_navigation_anchor(self.current_line(), &cell);
            if self.cursor_col <= edit_start {
                return false;
            }
            let mut col = self.cursor_col.min(edit_end);
            let line = self.current_line();
            let chars: Vec<char> = line.chars().collect();
            while col > edit_start && chars.get(col - 1).is_some_and(|c| !c.is_alphanumeric()) {
                col -= 1;
            }
            while col > edit_start && chars.get(col - 1).is_some_and(|c| c.is_alphanumeric()) {
                col -= 1;
            }
            if col == self.cursor_col {
                return false;
            }
            let start_byte = byte_index(self.current_line(), col);
            let end_byte = byte_index(self.current_line(), self.cursor_col);
            let text = self.current_line_mut();
            text.replace_range(start_byte..end_byte, "");
            self.cursor_col = col;
            self.mark_edited();
            return true;
        }
        let line = self.current_line();
        let chars: Vec<char> = line.chars().collect();
        let mut col = self.cursor_col;
        while col > 0 && chars.get(col - 1).map_or(false, |c| !c.is_alphanumeric()) {
            col -= 1;
        }
        while col > 0 && chars.get(col - 1).map_or(false, |c| c.is_alphanumeric()) {
            col -= 1;
        }

        let start_byte = byte_index(self.current_line(), col);
        let end_byte = byte_index(self.current_line(), self.cursor_col);
        let text = self.current_line_mut();
        text.replace_range(start_byte..end_byte, "");
        self.cursor_col = col;
        self.mark_edited();
        true
    }

    fn insert_char(&mut self, ch: char) {
        if ch.is_control() {
            return;
        }
        let col = self.cursor_col;
        let line = self.current_line_mut();
        let idx = byte_index(line, col);
        line.insert(idx, ch);
        self.cursor_col += 1;
        self.mark_edited();
    }

    fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let col = self.cursor_col;
        let line = self.current_line_mut();
        let idx = byte_index(line, col);
        line.insert_str(idx, text);
        self.cursor_col += text.chars().count();
        self.mark_edited();
    }

    fn insert_paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        if self.lines.is_empty() {
            self.lines.push(String::new());
        }

        // Normalize line endings to keep cursor/line mapping predictable.
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let parts: Vec<&str> = normalized.split('\n').collect();
        if parts.is_empty() {
            return;
        }

        let line_idx = self.cursor_line.min(self.lines.len().saturating_sub(1));
        let col = self.cursor_col;
        let current = self.lines[line_idx].clone();
        let split_idx = byte_index(&current, col);
        let (left, right) = current.split_at(split_idx);

        if parts.len() == 1 {
            self.lines[line_idx] = format!("{left}{}{right}", parts[0]);
            self.cursor_line = line_idx;
            self.cursor_col = col + parts[0].chars().count();
            self.mark_edited();
            return;
        }

        self.lines[line_idx] = format!("{left}{}", parts[0]);
        let mut insert_at = line_idx + 1;
        for part in &parts[1..parts.len() - 1] {
            self.lines.insert(insert_at, (*part).to_string());
            insert_at += 1;
        }

        let tail = *parts.last().unwrap_or(&"");
        self.lines.insert(insert_at, format!("{tail}{right}"));
        self.cursor_line = insert_at;
        self.cursor_col = tail.chars().count();
        self.mark_edited();
    }

    fn insert_newline(&mut self) {
        let col = self.cursor_col;
        let idx = byte_index(self.current_line(), col);
        let right = self.lines[self.cursor_line][idx..].to_string();
        self.lines[self.cursor_line].truncate(idx);
        let insert_at = self.cursor_line + 1;
        self.lines.insert(insert_at, right);
        self.cursor_line += 1;
        self.cursor_col = 0;
        self.mark_edited();
    }

    fn apply_calc_tab(&mut self) -> bool {
        let text = self.current_line().to_string();
        let Some(result) = self
            .calc_results
            .get(self.cursor_line)
            .and_then(|value| value.clone())
        else {
            return false;
        };
        if contains_assignment_operator(&text) {
            return false;
        }

        if let Some((from_byte, to_byte)) = find_calc_segment_range(&text) {
            self.lines[self.cursor_line].replace_range(from_byte..to_byte, &result);
            self.cursor_col = self.lines[self.cursor_line]
                [..from_byte.saturating_add(result.len())]
                .chars()
                .count();
            self.mark_edited();
            return true;
        }

        self.insert_text(&format!(" = {result}"));
        true
    }

    fn line_might_trigger_doc_change_rules(line: &str) -> bool {
        let trimmed = line.trim_start();
        let might_be_list = trimmed.starts_with('-')
            || trimmed.starts_with('*')
            || trimmed.starts_with('+')
            || trimmed.starts_with("->")
            || trimmed.chars().next().is_some_and(|c| c.is_ascii_digit());
        let might_be_table = trimmed.starts_with('|') && line.trim_end().ends_with('|');
        might_be_list || might_be_table
    }

    fn try_autoformat_rules(&mut self) {
        if !Self::line_might_trigger_doc_change_rules(self.current_line()) {
            return;
        }

        let snapshot = self.build_snapshot();
        let options = crate::editor_core::text_rules::TextRuleOptions {
            markdown_autoformat: self.markdown_autoformat,
            checklist_auto_reorder: self.checklist_auto_reorder,
        };
        if let Some(op) = crate::editor_core::text_rules::run_doc_change_rules(&snapshot, options) {
            self.apply_edit_operation(&op);
        }
    }

    fn try_enter_rule(&mut self) -> bool {
        let snapshot = self.build_snapshot();
        let options = crate::editor_core::text_rules::TextRuleOptions {
            markdown_autoformat: self.markdown_autoformat,
            checklist_auto_reorder: self.checklist_auto_reorder,
        };
        if let Some(op) = crate::editor_core::text_rules::run_enter_rules(&snapshot, options) {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    fn try_tab_rule(&mut self, outdent: bool) -> bool {
        let snapshot = self.build_snapshot();
        let options = crate::editor_core::text_rules::TabRuleOptions {
            markdown_autoformat: self.markdown_autoformat,
            outdent,
        };
        if let Some(op) = crate::editor_core::text_rules::run_tab_rules(&snapshot, options) {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    fn try_table_navigation_rule(&mut self, outdent: bool) -> bool {
        let snapshot = self.build_snapshot();
        let options = crate::editor_core::text_rules::TabRuleOptions {
            markdown_autoformat: self.markdown_autoformat,
            outdent,
        };
        if let Some(op) =
            crate::editor_core::text_rules::run_table_cell_navigation_rules(&snapshot, options)
        {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    fn try_table_pipe_insert_column_rule(&mut self) -> bool {
        let snapshot = self.build_snapshot();
        if let Some(op) =
            crate::editor_core::text_rules::run_table_pipe_insert_column_rule(&snapshot)
        {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    fn try_table_header_delete_column_rule(&mut self) -> bool {
        let snapshot = self.build_snapshot();
        if let Some(op) =
            crate::editor_core::text_rules::run_table_header_delete_column_rule(&snapshot)
        {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    fn try_table_boundary_edit_rule(
        &mut self,
        backward: bool,
        structural_merge: bool,
    ) -> Option<bool> {
        let snapshot = self.build_snapshot();
        let options = crate::editor_core::text_rules::TableBoundaryEditOptions {
            markdown_autoformat: self.markdown_autoformat,
            backward,
            structural_merge,
        };
        let op = crate::editor_core::text_rules::run_table_boundary_edit_rules(&snapshot, options)?;
        let changed = !op.changes.is_empty();
        self.apply_edit_operation(&op);
        Some(changed)
    }

    fn apply_edit_operation(&mut self, op: &crate::editor_core::types::EditOperation) {
        if op.changes.is_empty() {
            if let Some(sel) = &op.selection {
                let mut offset = 0usize;
                let target = sel.anchor.min(join_lines(&self.lines).len());
                for (i, line) in self.lines.iter().enumerate() {
                    let line_end = offset + line.len();
                    if target <= line_end {
                        self.cursor_line = i;
                        self.cursor_col = line[..target.saturating_sub(offset)].chars().count();
                        break;
                    }
                    offset = line_end + 1;
                }
                self.adjust_cursor();
                self.adjust_scroll();
            }
            return;
        }

        let mut text = join_lines(&self.lines);

        // Track initial cursor byte offset
        let mut mapped_anchor = 0;
        for (i, line) in self.lines.iter().enumerate() {
            if i == self.cursor_line {
                mapped_anchor += byte_index(line, self.cursor_col);
                break;
            }
            mapped_anchor += line.len() + 1;
        }

        let mut changes = op.changes.clone();
        changes.sort_by(|a, b| b.from.cmp(&a.from));
        for change in &changes {
            let from = change.from.min(text.len());
            let to = change.to.min(text.len());
            text.replace_range(from..to, &change.insert);

            // Map cursor through change
            if from <= mapped_anchor {
                if to <= mapped_anchor {
                    let removed = to - from;
                    let added = change.insert.len();
                    mapped_anchor = mapped_anchor + added - removed;
                } else {
                    // Keep cursor stable relative to the replacement start
                    // when it falls inside the replaced span.
                    let inside = mapped_anchor.saturating_sub(from);
                    mapped_anchor = from + inside.min(change.insert.len());
                }
            }
        }
        self.lines = split_lines(&text);

        let final_anchor = if let Some(sel) = &op.selection {
            sel.anchor
        } else {
            mapped_anchor
        }
        .min(text.len());

        let mut offset = 0;
        for (i, line) in self.lines.iter().enumerate() {
            let line_end = offset + line.len();
            if final_anchor <= line_end {
                self.cursor_line = i;
                self.cursor_col = line[..final_anchor.saturating_sub(offset)].chars().count();
                break;
            }
            offset = line_end + 1;
        }
        self.fold_rescan_pending = true;
        self.mark_edited();
        self.adjust_cursor();
        self.adjust_scroll();
    }

    fn backspace(&mut self) {
        if let Some(cell) = table_cell_info_at_char(self.current_line(), self.cursor_col) {
            let edit_start = table_cell_edit_start(&cell);
            let edit_end = table_cell_navigation_anchor(self.current_line(), &cell);
            if self.cursor_col <= edit_start {
                return;
            }
            if self.cursor_col > edit_end {
                self.cursor_col = edit_end;
                return;
            }
            let new_col = self.cursor_col - 1;
            if new_col < edit_start {
                return;
            }
            remove_char_at(&mut self.lines[self.cursor_line], new_col);
            self.cursor_col = new_col;
            self.mark_edited();
            return;
        }

        if self.cursor_col > 0 {
            let new_col = self.cursor_col - 1;
            remove_char_at(&mut self.lines[self.cursor_line], new_col);
            self.cursor_col = new_col;
            self.mark_edited();
            return;
        }

        if self.cursor_line == 0 {
            return;
        }

        let removed = self.lines.remove(self.cursor_line);
        self.cursor_line -= 1;
        let prev_len = line_char_len(&self.lines[self.cursor_line]);
        self.lines[self.cursor_line].push_str(&removed);
        self.cursor_col = prev_len;
        self.mark_edited();
    }

    fn delete_forward(&mut self) {
        if let Some(cell) = table_cell_info_at_char(self.current_line(), self.cursor_col) {
            let edit_start = table_cell_edit_start(&cell);
            let edit_end = table_cell_navigation_anchor(self.current_line(), &cell);
            if self.cursor_col < edit_start {
                self.cursor_col = edit_start;
                return;
            }
            if self.cursor_col >= edit_end {
                return;
            }
            let col = self.cursor_col;
            remove_char_at(&mut self.lines[self.cursor_line], col);
            self.mark_edited();
            return;
        }

        let line_len = line_char_len(self.current_line());
        if self.cursor_col < line_len {
            let col = self.cursor_col;
            remove_char_at(&mut self.lines[self.cursor_line], col);
            self.mark_edited();
            return;
        }

        if self.cursor_line + 1 >= self.lines.len() {
            return;
        }

        let next = self.lines.remove(self.cursor_line + 1);
        self.lines[self.cursor_line].push_str(&next);
        self.mark_edited();
    }

    fn move_cursor_left(&mut self) {
        let table_target_col = {
            let line_text = self.current_line();
            if let Some(current_cell) = table_cell_info_at_char(line_text, self.cursor_col) {
                let anchor = table_cell_navigation_anchor(line_text, &current_cell);
                let edit_start = table_cell_edit_start(&current_cell);
                if table_cell_is_empty(&current_cell) {
                    Some(anchor)
                } else if self.cursor_col > anchor {
                    // Entering left/right padding is not allowed; snap back to content anchor.
                    Some(anchor)
                } else if self.cursor_col <= edit_start {
                    // Regular arrows do not cross cell boundaries.
                    Some(edit_start)
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(target_col) = table_target_col {
            self.cursor_col = target_col;
            return;
        }

        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            return;
        }
        let current_virtual = self.current_virtual_line();
        if current_virtual > 0 {
            if let Some(prev_real) = self.real_line_for_virtual(current_virtual - 1) {
                self.cursor_line = prev_real;
                self.cursor_col = line_char_len(self.current_line());
            }
        }
    }

    fn move_cursor_right(&mut self) {
        let table_target_col = {
            let line_text = self.current_line();
            if let Some(current_cell) = table_cell_info_at_char(line_text, self.cursor_col) {
                let anchor = table_cell_navigation_anchor(line_text, &current_cell);
                let edit_start = table_cell_edit_start(&current_cell);
                if table_cell_is_empty(&current_cell) {
                    Some(anchor)
                } else if self.cursor_col < edit_start {
                    Some(edit_start)
                } else if self.cursor_col > anchor {
                    // Entering padding is not allowed; snap back.
                    Some(anchor)
                } else if self.cursor_col == anchor {
                    // Regular arrows do not cross cell boundaries.
                    Some(anchor)
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(target_col) = table_target_col {
            self.cursor_col = target_col;
            return;
        }

        let line_len = line_char_len(self.current_line());
        if self.cursor_col < line_len {
            self.cursor_col += 1;
            return;
        }
        let current_virtual = self.current_virtual_line();
        if current_virtual + 1 < self.visible_line_count() {
            if let Some(next_real) = self.real_line_for_virtual(current_virtual + 1) {
                self.cursor_line = next_real;
                self.cursor_col = 0;
            }
        }
    }

    fn move_cursor_up(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let current_virtual = self.current_virtual_line();
        let target_virtual = current_virtual.saturating_sub(count);
        self.cursor_line = self.real_line_for_virtual(target_virtual).unwrap_or(0);
        if let Some(cell) = table_cell_info_at_char(self.current_line(), self.cursor_col) {
            self.cursor_col = table_cell_navigation_anchor(self.current_line(), &cell);
        }
    }

    fn move_cursor_down(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let current_virtual = self.current_virtual_line();
        let target_virtual = min(
            current_virtual.saturating_add(count),
            self.visible_line_count().saturating_sub(1),
        );
        self.cursor_line = self
            .real_line_for_virtual(target_virtual)
            .unwrap_or_else(|| self.lines.len().saturating_sub(1));
        if let Some(cell) = table_cell_info_at_char(self.current_line(), self.cursor_col) {
            self.cursor_col = table_cell_navigation_anchor(self.current_line(), &cell);
        }
    }

    fn adjust_cursor_with_table_padding_guard(&mut self, clamp_table_padding: bool) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        if self.cursor_line >= self.lines.len() {
            self.cursor_line = self.lines.len() - 1;
        }
        if let Some(owner) = self.fold_hidden_owner_for_line(self.cursor_line) {
            self.cursor_line = owner.min(self.lines.len().saturating_sub(1));
        }
        let len = line_char_len(self.current_line());
        if self.cursor_col > len {
            self.cursor_col = len;
        }
        let table_anchor = {
            let line_text = self.current_line();
            if let Some(cell) = table_cell_info_at_char(line_text, self.cursor_col) {
                let anchor = table_cell_navigation_anchor(line_text, &cell);
                let edit_start = table_cell_edit_start(&cell);
                if table_cell_is_empty(&cell) {
                    Some(anchor)
                } else if self.cursor_col < edit_start
                    || (clamp_table_padding && self.cursor_col > anchor)
                {
                    // Keep the cursor inside content; right padding is
                    // reserved for alignment only.
                    Some(anchor)
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(anchor) = table_anchor {
            self.cursor_col = anchor;
        }
    }

    fn adjust_cursor(&mut self) {
        self.adjust_cursor_with_table_padding_guard(true);
    }

    fn editor_height(&self) -> usize {
        let (rows, _) = input::terminal_size();
        rows.saturating_sub(2).max(1)
    }

    fn gutter_width(&self) -> usize {
        gutter_width_for_visible_lines(self.visible_line_count())
    }

    fn adjust_scroll(&mut self) {
        let height = self.editor_height();
        let cursor_virtual = self.current_virtual_line();
        if cursor_virtual < self.scroll_line {
            self.scroll_line = cursor_virtual;
        } else if cursor_virtual >= self.scroll_line + height {
            self.scroll_line = cursor_virtual + 1 - height;
        }
        self.scroll_line = self
            .scroll_line
            .min(self.visible_line_count().saturating_sub(1));

        let (_, cols) = input::terminal_size();
        let available = cols.saturating_sub(self.gutter_width());
        if available == 0 {
            self.scroll_col = 0;
            return;
        }

        let (cursor_display_col, max_scroll) = {
            let line_text = self.current_line();
            let line_len = line_char_len(line_text);
            let logical_col = min(self.cursor_col, line_len);
            let render_col = cursor_render_char_col(
                line_text,
                self.cursor_col,
                matches!(
                    self.mode,
                    UiMode::Normal | UiMode::Visual | UiMode::VisualLine
                ),
            );
            let line_width = line_display_cols(line_text);
            let end_slot = usize::from(
                self.mode == UiMode::Editor && logical_col == line_len && line_width > available,
            );
            let target_col = if self.mode == UiMode::Editor {
                logical_col
            } else {
                render_col
            };
            (
                display_cols_for_prefix(line_text, target_col),
                line_width
                    .saturating_sub(available)
                    .saturating_add(end_slot),
            )
        };

        if cursor_display_col < self.scroll_col {
            self.scroll_col = cursor_display_col.saturating_sub(HORIZONTAL_SCROLL_LEFT_CONTEXT);
        } else if cursor_display_col >= self.scroll_col + available {
            self.scroll_col = cursor_display_col + 1 - available;
        }

        self.scroll_col = self.scroll_col.min(max_scroll);
    }

    fn draw(&self, out: &mut impl Write) -> Result<(), String> {
        let (rows, cols) = input::terminal_size();
        let editor_height = rows.saturating_sub(2).max(1);
        let gutter_width = self.gutter_width();
        let line_number_width = gutter_width.saturating_sub(2);
        let mut buf = String::with_capacity(rows.saturating_mul(cols.saturating_add(8)));

        // Hide cursor, move home. No \x1b[2J — we overwrite every row to full width.
        buf.push_str("\x1b[?25l\x1b[H");

        let title = derive_title_from_lines(&self.lines);
        let dirty_mark = if self.dirty { " [+]" } else { "" };
        let mode_label = match self.mode {
            UiMode::Normal => "",
            UiMode::Editor => " INSERT",
            UiMode::Visual => " VISUAL",
            UiMode::VisualLine => " V-LINE",
            UiMode::CommandBar => " CMD",
            UiMode::Search => " SEARCH",
            UiMode::Switcher => " SWITCH",
            UiMode::DatePicker => " DATE",
        };
        let title_line = format!(
            " note  {}  {}{}{}",
            self.active_note.id, title, dirty_mark, mode_label
        );
        let title_bg = self.render_palette.search_current;
        draw_row_at_styled(
            &mut buf,
            TITLE_ROW,
            1,
            cols,
            &title_line,
            AnsiStyle {
                fg: Some(contrast_fg_for_bg(title_bg)),
                bg: Some(title_bg),
                bold: true,
                ..Default::default()
            },
        );

        let mut ctx = render::RenderContext::new_with_palette(self.render_palette);
        let first_real_line = self
            .real_line_for_virtual(self.scroll_line)
            .unwrap_or(self.lines.len());
        ctx.advance_lines(&self.lines[..first_real_line.min(self.lines.len())]);
        let mut last_rendered_real = if first_real_line > 0 {
            Some(first_real_line - 1)
        } else {
            None
        };
        let mut cursor_line_override: Option<(String, usize)> = None;
        let now_ms = notifications::now_epoch_ms();

        for i in 0..editor_height {
            let row = EDITOR_TOP_ROW + i;
            let virtual_line = self.scroll_line + i;
            if let Some(line_idx) = self.real_line_for_virtual(virtual_line) {
                if let Some(prev_real) = last_rendered_real {
                    if line_idx > prev_real + 1 {
                        ctx.advance_lines(&self.lines[(prev_real + 1)..line_idx]);
                    }
                }
                last_rendered_real = Some(line_idx);

                let line_no = virtual_line + 1;
                let available = cols.saturating_sub(gutter_width);
                let is_cursor_line = line_idx == self.cursor_line;
                let mut calc_ghost = self.calc_results.get(line_idx).and_then(|r| r.as_deref());
                let mut calc_ghost_override: Option<String> = None;
                let mut reminder_ghost_override: Option<String> = None;
                let mut reminder_strikethrough = false;
                let mut ghost_dim_ranges: Vec<(usize, usize)> = Vec::new();
                let mut formula_segments: Vec<TableFormulaSegment> = Vec::new();
                let line_text = &self.lines[line_idx];
                let mut rendered_line = line_text.to_string();
                let collapsed_hidden_count = self
                    .fold_placeholder_hidden_lines
                    .get(line_idx)
                    .and_then(|entry| *entry);
                let is_fold_placeholder = collapsed_hidden_count.is_some();

                if let Some(hidden_count) = collapsed_hidden_count {
                    let suffix = if hidden_count == 1 { "" } else { "s" };
                    rendered_line = "▶".to_string();
                    calc_ghost = None;
                    reminder_ghost_override = Some(format!("{hidden_count} line{suffix} folded"));
                } else {
                    if let Some(reminder) = self.reminder_ghosts.get(&line_idx) {
                        reminder_ghost_override = Some(format!("⏰ {}", reminder.display_at));
                        reminder_strikethrough = reminder.remind_at_ms <= now_ms;
                    }

                    formula_segments = find_table_formula_segments(line_text);
                    if !formula_segments.is_empty() {
                        // Formula rows render a marker in-cell (`value*`,
                        // `value**`, …) and keep the detailed per-formula
                        // explanation as a line-end ghost. The first formula
                        // value in the row is also stored in `calc_results`
                        // for backward compatibility; per-cell values come
                        // from `cell_calc_results`.
                        calc_ghost = None;

                        let cell_results = self
                            .cell_calc_results
                            .get(line_idx)
                            .cloned()
                            .unwrap_or_default();
                        let value_for_cell = |cell_index: usize| -> Option<String> {
                            cell_results
                                .iter()
                                .find(|(idx, _)| *idx == cell_index)
                                .map(|(_, v)| format_formula_display_value(v))
                                .or_else(|| {
                                    // Fallback: legacy single-result path.
                                    self.calc_results
                                        .get(line_idx)
                                        .and_then(|r| r.as_deref())
                                        .map(format_formula_display_value)
                                })
                        };

                        let mut out = String::with_capacity(line_text.len() + 16);
                        let mut last_byte = 0usize;
                        let mut char_delta: isize = 0;
                        let mut trailer_parts: Vec<String> = Vec::new();
                        // Char position of the cursor in the rendered line; we
                        // collect this only when the cursor sits inside a
                        // focused (un-masked) formula cell.
                        let mut focused_cursor_col: Option<usize> = None;

                        for (fi, seg) in formula_segments.iter().enumerate() {
                            let marker = formula_marker_token(fi);
                            let value =
                                value_for_cell(seg.cell_index).unwrap_or_else(|| String::from("…"));
                            let source_text =
                                line_text[seg.from_byte..seg.to_byte].trim().to_string();

                            let is_focused = is_cursor_line
                                && self.cursor_col >= seg.cell_from_char
                                && self.cursor_col < seg.cell_to_char;

                            // Ghost trailer: focused cell shows the value
                            // (so the user can see the result while editing),
                            // resting cells show the formula source.
                            let trailer_text = if is_focused {
                                value.clone()
                            } else {
                                source_text
                            };
                            if !trailer_text.is_empty() {
                                trailer_parts.push(format!("{marker} ➜ {trailer_text}"));
                            }

                            out.push_str(&line_text[last_byte..seg.from_byte]);

                            if is_focused {
                                out.push_str(&line_text[seg.from_byte..seg.to_byte]);
                                let mapped =
                                    (self.cursor_col as isize + char_delta).max(0) as usize;
                                focused_cursor_col = Some(mapped);
                            } else {
                                let mut replacement = format!("{value}{marker}");
                                let old_chars = seg.to_char.saturating_sub(seg.from_char);
                                let new_chars = replacement.chars().count();
                                if new_chars < old_chars {
                                    replacement.push_str(&" ".repeat(old_chars - new_chars));
                                }
                                let rendered_chars = replacement.chars().count();
                                let marker_char = ((seg.from_char as isize) + char_delta) as usize
                                    + value.chars().count();
                                let marker_end = marker_char + marker.chars().count();
                                ghost_dim_ranges.push((marker_char, marker_end));
                                char_delta += rendered_chars as isize - old_chars as isize;
                                out.push_str(&replacement);
                            }
                            last_byte = seg.to_byte;
                        }
                        out.push_str(&line_text[last_byte..]);
                        rendered_line = out;

                        calc_ghost_override = Some(trailer_parts.join("  "));

                        if is_cursor_line {
                            let mapped_col = focused_cursor_col.unwrap_or_else(|| {
                                // Cursor is outside every formula cell. Walk
                                // the segments that lie entirely before the
                                // cursor and accumulate their rendered-vs-
                                // source char delta.
                                let mut delta: isize = 0;
                                for (fi, seg) in formula_segments.iter().enumerate() {
                                    if seg.cell_to_char <= self.cursor_col {
                                        let value = value_for_cell(seg.cell_index)
                                            .unwrap_or_else(|| String::from("…"));
                                        let marker = formula_marker_token(fi);
                                        let mut rep = format!("{value}{marker}");
                                        let old_chars = seg.to_char.saturating_sub(seg.from_char);
                                        let new_chars = rep.chars().count();
                                        if new_chars < old_chars {
                                            rep.push_str(&" ".repeat(old_chars - new_chars));
                                        }
                                        delta += rep.chars().count() as isize - old_chars as isize;
                                    }
                                }
                                ((self.cursor_col as isize) + delta).max(0) as usize
                            });
                            cursor_line_override = Some((rendered_line.clone(), mapped_col));
                        }
                    }
                }

                let (search_ranges, current_search_ranges) = if is_fold_placeholder {
                    (Vec::new(), Vec::new())
                } else {
                    self.search_highlights_for_line(line_idx)
                };
                let mut visual_highlight_ranges = Vec::new();
                if is_fold_placeholder {
                    if self.line_is_in_visual_selection(line_idx) {
                        visual_highlight_ranges.push((0, rendered_line.chars().count().max(1)));
                    }
                } else {
                    self.append_visual_highlights(line_idx, &mut visual_highlight_ranges);
                }
                let effective_calc_ghost = calc_ghost_override.as_deref().or(calc_ghost);
                let effective_reminder_ghost = reminder_ghost_override.as_deref();
                let line_scroll_col = self.scroll_col;
                let line_width = line_display_cols(&rendered_line);
                let viewport = compute_line_viewport(line_width, line_scroll_col, available);

                // Highlight the focused table cell's pipe characters in red so
                // the active cell is obvious. Pipe positions are taken from
                // the source `line_text` and translated to rendered char
                // positions using the formula-mask delta accumulated above.
                let mut focused_pipe_ranges: Vec<(usize, usize)> = Vec::new();
                if is_cursor_line && !is_fold_placeholder {
                    if let Some(info) = table_cell_info_at_char(line_text, self.cursor_col) {
                        let left_pipe_char = line_text[..info.left_pipe].chars().count();
                        let right_pipe_char = line_text[..info.right_pipe].chars().count();
                        let translate = |src_col: usize| -> usize {
                            let mut delta: isize = 0;
                            for (fi, seg) in formula_segments.iter().enumerate() {
                                if seg.cell_to_char <= src_col {
                                    let value = self
                                        .cell_calc_results
                                        .get(line_idx)
                                        .and_then(|row| {
                                            row.iter()
                                                .find(|(idx, _)| *idx == seg.cell_index)
                                                .map(|(_, v)| format_formula_display_value(v))
                                        })
                                        .unwrap_or_else(|| String::from("…"));
                                    let marker = formula_marker_token(fi);
                                    let mut rep = format!("{value}{marker}");
                                    let old_chars = seg.to_char.saturating_sub(seg.from_char);
                                    let new_chars = rep.chars().count();
                                    if new_chars < old_chars {
                                        rep.push_str(&" ".repeat(old_chars - new_chars));
                                    }
                                    delta += rep.chars().count() as isize - old_chars as isize;
                                }
                            }
                            ((src_col as isize) + delta).max(0) as usize
                        };
                        let lp = translate(left_pipe_char);
                        let rp = translate(right_pipe_char);
                        focused_pipe_ranges.push((lp, lp + 1));
                        focused_pipe_ranges.push((rp, rp + 1));
                    }
                }

                let rendered_text = if ghost_dim_ranges.is_empty()
                    && visual_highlight_ranges.is_empty()
                    && focused_pipe_ranges.is_empty()
                {
                    ctx.render_line_window_with_reminder(
                        &rendered_line,
                        viewport.text_width,
                        viewport.text_window_col,
                        effective_calc_ghost,
                        effective_reminder_ghost,
                        reminder_strikethrough,
                        &search_ranges,
                        &current_search_ranges,
                        &self.variable_names,
                    )
                } else {
                    ctx.render_line_full(
                        &rendered_line,
                        viewport.text_width,
                        viewport.text_window_col,
                        effective_calc_ghost,
                        effective_reminder_ghost,
                        reminder_strikethrough,
                        &search_ranges,
                        &current_search_ranges,
                        &self.variable_names,
                        &ghost_dim_ranges,
                        &visual_highlight_ranges,
                        &focused_pipe_ranges,
                    )
                };
                buf.push_str(&goto(row, 1));
                let gutter_style = if is_cursor_line {
                    AnsiStyle {
                        fg: Some(self.render_palette.variable),
                        bold: true,
                        ..Default::default()
                    }
                } else {
                    AnsiStyle {
                        fg: Some(self.render_palette.code_comment),
                        dim: true,
                        ..Default::default()
                    }
                };
                gutter_style.write_to(&mut buf);
                buf.push_str(&format!("{line_no:>line_number_width$}  "));
                buf.push_str(render::RESET);
                if viewport.has_left_overflow {
                    let indicator_style = AnsiStyle {
                        fg: Some(self.render_palette.code_comment),
                        dim: true,
                        ..Default::default()
                    };
                    indicator_style.write_to(&mut buf);
                    buf.push(OVERFLOW_LEFT_MARKER);
                    buf.push_str(render::RESET);
                }
                buf.push_str(&rendered_text);
                if viewport.has_right_overflow {
                    let indicator_style = AnsiStyle {
                        fg: Some(self.render_palette.code_comment),
                        dim: true,
                        ..Default::default()
                    };
                    indicator_style.write_to(&mut buf);
                    buf.push(OVERFLOW_RIGHT_MARKER);
                    buf.push_str(render::RESET);
                }
            } else {
                buf.push_str(&goto(row, 1));
                AnsiStyle {
                    fg: Some(self.render_palette.code_comment),
                    dim: true,
                    ..Default::default()
                }
                .write_to(&mut buf);
                buf.push_str(&pad_right("~", cols));
                buf.push_str(render::RESET);
            }
        }

        let status = match self.mode {
            UiMode::Editor
            | UiMode::Normal
            | UiMode::CommandBar
            | UiMode::Search
            | UiMode::Visual
            | UiMode::VisualLine => &self.status,
            UiMode::Switcher => {
                if self.switcher_delete_confirm.is_some() {
                    "Confirm delete: Enter/Y confirm, Esc/N cancel"
                } else {
                    "Switcher: type to filter, Enter open, Delete/Ctrl+Backspace delete, Esc close"
                }
            }
            UiMode::DatePicker => {
                "Date picker: arrows navigate, Ctrl+arrows months, Enter insert, Esc cancel"
            }
        };
        let status_bg = self.render_palette.search_match;
        draw_row_at_styled(
            &mut buf,
            rows,
            1,
            cols,
            status,
            AnsiStyle {
                fg: Some(contrast_fg_for_bg(status_bg)),
                bg: Some(status_bg),
                ..Default::default()
            },
        );

        if self.mode == UiMode::Switcher {
            switcher::draw_switcher(
                &SwitcherView {
                    query: &self.switcher_query,
                    items: &self.switcher_items,
                    matches: &self.switcher_matches,
                    selected: self.switcher_selected,
                },
                &mut buf,
                rows,
                cols,
                self.render_palette,
            );
            if let Some(confirm) = self.switcher_delete_confirm.as_ref() {
                switcher::draw_delete_confirm(
                    &confirm.note_title,
                    &mut buf,
                    rows,
                    cols,
                    self.render_palette,
                );
            }
        }

        if self.mode == UiMode::DatePicker {
            date_picker::draw_date_picker(
                &DatePickerView {
                    year: self.date_year,
                    month: self.date_month,
                    day: self.date_day,
                    hour: self.date_hour,
                    minute: self.date_minute,
                    include_time: self.date_include_time,
                    require_time: self.date_require_time,
                    is_notify: self.date_picker_action == DatePickerAction::SetNotify,
                    date_format: &self.date_format,
                    date_time_format: &self.date_time_format,
                },
                &mut buf,
                rows,
                cols,
                self.render_palette,
            );
        }

        let (cursor_row, mut cursor_col) = self.cursor_position(rows, cols);
        if let Some((line_text, mapped_col)) = cursor_line_override {
            if matches!(
                self.mode,
                UiMode::Editor | UiMode::Normal | UiMode::Visual | UiMode::VisualLine
            ) {
                let available = cols.saturating_sub(gutter_width);
                let display_char_col = cursor_render_char_col(
                    &line_text,
                    mapped_col,
                    matches!(
                        self.mode,
                        UiMode::Normal | UiMode::Visual | UiMode::VisualLine
                    ),
                );
                let display_col = display_cols_for_prefix(&line_text, display_char_col);
                let line_width = line_display_cols(&line_text);
                let visible_col = viewport_col_for_display_col(
                    display_col,
                    line_width,
                    self.scroll_col,
                    available,
                );
                cursor_col = (gutter_width + visible_col + 1).min(cols.max(1)).max(1);
            }
        }
        buf.push_str(&goto(cursor_row, cursor_col));

        let cursor_style = match self.mode {
            UiMode::Editor
            | UiMode::CommandBar
            | UiMode::Search
            | UiMode::Switcher
            | UiMode::DatePicker => {
                "\x1b[5 q" // Blinking Bar
            }
            UiMode::Normal | UiMode::Visual | UiMode::VisualLine => "\x1b[1 q", // Blinking Block
        };
        buf.push_str(cursor_style);
        buf.push_str("\x1b[?25h");

        out.write_all(buf.as_bytes())
            .and_then(|_| out.flush())
            .map_err(|e| format!("Failed to draw terminal UI: {e}"))?;
        Ok(())
    }

    fn cursor_position(&self, rows: usize, cols: usize) -> (usize, usize) {
        match self.mode {
            UiMode::CommandBar => {
                let col = (1 + 1 + self.command_input.chars().count()).min(cols.max(1));
                (rows, col.max(1))
            }
            UiMode::Search => {
                let col = (1 + 1 + self.search_query.chars().count()).min(cols.max(1));
                (rows, col.max(1))
            }
            UiMode::DatePicker => {
                // Hide cursor inside the date picker
                (1, 1)
            }
            UiMode::Editor | UiMode::Normal | UiMode::Visual | UiMode::VisualLine => {
                let cursor_virtual = self.current_virtual_line();
                let row = EDITOR_TOP_ROW
                    + cursor_virtual
                        .saturating_sub(self.scroll_line)
                        .min(rows.saturating_sub(2));
                let line_text = self.current_line();
                let display_char_col = cursor_render_char_col(
                    line_text,
                    self.cursor_col,
                    matches!(
                        self.mode,
                        UiMode::Normal | UiMode::Visual | UiMode::VisualLine
                    ),
                );
                let gutter_width = self.gutter_width();
                let available = cols.saturating_sub(gutter_width);
                let display_col = display_cols_for_prefix(line_text, display_char_col);
                let line_width = line_display_cols(line_text);
                let visible_col = viewport_col_for_display_col(
                    display_col,
                    line_width,
                    self.scroll_col,
                    available,
                );
                let col = (gutter_width + visible_col + 1).min(cols.max(1));
                (row.max(1), col.max(1))
            }
            UiMode::Switcher => {
                let box_w = min(cols.saturating_sub(4).max(30), 72);
                let box_h = min(rows.saturating_sub(4).max(8), 14);
                let x = (cols.saturating_sub(box_w)) / 2 + 1;
                let y = (rows.saturating_sub(box_h)) / 2 + 1;
                let prompt = " search: ";
                let col = (x + 1 + prompt.chars().count() + self.switcher_query.chars().count())
                    .min(cols.max(1));
                (y + 1, col.max(1))
            }
        }
    }
}

pub fn run_terminal_session(
    db: &Db,
    config: &ThemeConfig,
    opts: &TerminalOptions,
) -> Result<(), String> {
    let startup_ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    if opts.list_only {
        switcher::print_note_list(db)?;
        let line = format!("time:{startup_ts_ms} loading_screen:0ms list_notes:0ms");
        if let Err(err) = append_startup_log_line("tui", &line) {
            eprintln!("Startup diagnostics: {err}");
        }
        return Ok(());
    }

    let (mut app, metrics) = TerminalApp::new_with_startup_metrics(
        db,
        opts,
        config.vim_mode,
        config.format_on_save,
        config.markdown_autoformat,
        config.checklist_auto_reorder,
        config.variables_enabled,
        render::RenderPalette::for_color_scheme(&config.color_scheme),
        config.date_format.clone(),
        config.date_time_format.clone(),
    )?;
    let line = format!(
        "time:{startup_ts_ms} loading_screen:{} loading_note:{} loading_switcher:{} loading_calc_engine:{}",
        format_startup_duration(metrics.loading_screen),
        format_startup_duration(metrics.loading_note),
        format_startup_duration(metrics.loading_switcher),
        format_startup_duration(metrics.loading_calc_engine),
    );
    if let Err(err) = append_startup_log_line("tui", &line) {
        eprintln!("Startup diagnostics: {err}");
    }

    app.run(db)
}

fn format_startup_duration(duration: Duration) -> String {
    let ms = duration.as_secs_f64() * 1000.0;
    if ms >= 1000.0 {
        let seconds = ms / 1000.0;
        if seconds >= 10.0 {
            format!("{seconds:.1}s")
        } else {
            format!("{seconds:.2}s")
        }
    } else {
        format!("{}ms", ms.round() as u64)
    }
}

fn select_note(db: &Db, opts: &TerminalOptions) -> Result<Note, String> {
    if opts.create_new {
        return new_note(db);
    }

    if let Some(id) = &opts.note_id {
        return db
            .get_note(id)?
            .ok_or_else(|| format!("Note not found: {id}"));
    }

    let special = crate::config::load_special_notes_config();
    if let Some(note) = db.get_most_recent_note_excluding_prefix(&special.email_note_prefix)? {
        return Ok(note);
    }

    if let Some(note) = db.get_most_recent_note()? {
        return Ok(note);
    }

    new_note(db)
}

fn new_note(db: &Db) -> Result<Note, String> {
    let id = Ulid::new().to_string();
    db.save_note(&id, "")
}

fn load_note_reminder_ghosts(
    db: &Db,
    note_id: &str,
    lines: &[String],
) -> Result<HashMap<usize, LineReminderGhost>, String> {
    #[derive(Debug, Clone)]
    struct PlannedReminder {
        source_line: i64,
        source_text: String,
        target_line: i64,
        target_text: String,
        remind_at_ms: i64,
        display_at: String,
        notified_at_ms: Option<i64>,
    }

    fn nearest_available_line(
        candidates: &[usize],
        preferred: usize,
        used_lines: &HashSet<usize>,
    ) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut best_distance = usize::MAX;
        for &line_number in candidates {
            if used_lines.contains(&line_number) {
                continue;
            }
            let distance = line_number.abs_diff(preferred);
            if distance < best_distance
                || (distance == best_distance
                    && best.map(|current| line_number < current).unwrap_or(true))
            {
                best = Some(line_number);
                best_distance = distance;
            }
        }
        best
    }

    fn nearest_free_line(
        preferred: usize,
        line_count: usize,
        used_lines: &HashSet<usize>,
    ) -> Option<usize> {
        if line_count == 0 {
            return None;
        }
        let clamped = preferred.clamp(1, line_count);
        if !used_lines.contains(&clamped) {
            return Some(clamped);
        }
        for distance in 1..=line_count {
            let down = clamped.saturating_add(distance);
            if down <= line_count && !used_lines.contains(&down) {
                return Some(down);
            }
            let up = clamped.saturating_sub(distance);
            if up >= 1 && !used_lines.contains(&up) {
                return Some(up);
            }
        }
        None
    }

    let mut reminders = db.list_reminders(note_id)?;
    reminders.sort_by_key(|reminder| reminder.line_number);

    let mut text_to_lines: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, line) in lines.iter().enumerate() {
        text_to_lines.entry(line.clone()).or_default().push(idx + 1);
    }

    let mut used_lines: HashSet<usize> = HashSet::new();
    let mut planned = Vec::with_capacity(reminders.len());

    for reminder in reminders {
        let old_line = reminder.line_number;
        let old_line_usize = usize::try_from(old_line).ok().filter(|line| *line >= 1);
        let old_line_idx = old_line_usize
            .and_then(|line| line.checked_sub(1))
            .filter(|idx| *idx < lines.len());
        let old_text_matches = old_line_idx
            .and_then(|idx| lines.get(idx))
            .map(|line| line == &reminder.line_text)
            .unwrap_or(false);

        let preferred_line = old_line_usize.unwrap_or(1);
        let mut target_line = None;

        if old_text_matches {
            if let Some(old_line_num) = old_line_usize {
                if !used_lines.contains(&old_line_num) {
                    target_line = Some(old_line_num);
                }
            }
        }

        if target_line.is_none() {
            if let Some(candidates) = text_to_lines.get(&reminder.line_text) {
                target_line = nearest_available_line(candidates, preferred_line, &used_lines);
            }
        }

        if target_line.is_none() {
            if let Some(old_line_num) = old_line_usize {
                if old_line_num >= 1
                    && old_line_num <= lines.len()
                    && !used_lines.contains(&old_line_num)
                {
                    target_line = Some(old_line_num);
                }
            }
        }

        if target_line.is_none() {
            target_line = nearest_free_line(preferred_line, lines.len(), &used_lines);
        }

        let Some(target_line) = target_line else {
            continue;
        };
        used_lines.insert(target_line);

        let Some(target_text) = lines.get(target_line.saturating_sub(1)).cloned() else {
            continue;
        };

        planned.push(PlannedReminder {
            source_line: old_line,
            source_text: reminder.line_text,
            target_line: i64::try_from(target_line).unwrap_or(old_line),
            target_text,
            remind_at_ms: reminder.remind_at_ms,
            display_at: reminder.display_at,
            notified_at_ms: reminder.notified_at_ms,
        });
    }

    if !planned.is_empty() {
        let max_existing_line = planned
            .iter()
            .flat_map(|entry| [entry.source_line, entry.target_line])
            .filter(|line| *line > 0)
            .max()
            .unwrap_or_else(|| i64::try_from(lines.len()).unwrap_or(1));
        let temp_base = max_existing_line.saturating_add(10);
        let mut temp_moves = Vec::new();

        for (idx, entry) in planned.iter().enumerate() {
            if entry.source_line == entry.target_line {
                continue;
            }
            let temp_line = temp_base.saturating_add(i64::try_from(idx).unwrap_or(0) + 1);
            if db.move_reminder_line(note_id, entry.source_line, temp_line, &entry.source_text)? {
                temp_moves.push((idx, temp_line));
            }
        }

        for (idx, temp_line) in temp_moves {
            let entry = &planned[idx];
            let _ =
                db.move_reminder_line(note_id, temp_line, entry.target_line, &entry.target_text)?;
        }

        for entry in &planned {
            if entry.source_line == entry.target_line && entry.source_text != entry.target_text {
                let _ = db.move_reminder_line(
                    note_id,
                    entry.source_line,
                    entry.target_line,
                    &entry.target_text,
                )?;
            }
        }
    }

    let mut by_line = HashMap::with_capacity(planned.len());
    for entry in planned {
        let Some(line_idx) = entry
            .target_line
            .checked_sub(1)
            .and_then(|line| usize::try_from(line).ok())
        else {
            continue;
        };
        by_line.insert(
            line_idx,
            LineReminderGhost {
                remind_at_ms: entry.remind_at_ms,
                display_at: entry.display_at,
                line_text: entry.target_text,
                notified_at_ms: entry.notified_at_ms,
            },
        );
    }

    Ok(by_line)
}

#[cfg(test)]
fn calc_ghost_prefix(text: &str, calc_ghost: Option<&str>) -> &'static str {
    if calc_ghost
        .map(|ghost| ghost.trim_start().starts_with('*'))
        .unwrap_or(false)
    {
        " "
    } else if contains_assignment_operator(text) {
        " = "
    } else {
        " → "
    }
}

#[cfg(test)]
fn rendered_line_display_cols(text: &str, calc_ghost: Option<&str>) -> usize {
    let mut width = line_display_cols(text);
    if let Some(ghost) = calc_ghost {
        width += calc_ghost_prefix(text, Some(ghost)).chars().count();
        width += ghost.chars().count();
    }
    width
}

struct CalcData {
    line_results: Vec<Option<String>>,
    cell_results: Vec<Vec<(usize, String)>>,
    variable_names: Vec<String>,
}

fn compute_calc_data(
    engine: &CalcEngine,
    lines: &[String],
    variables_enabled: bool,
    eval_range: Option<(usize, usize)>,
) -> CalcData {
    let result = engine.evaluate_note_context(
        lines,
        app_core::calc::NoteEvaluationOptions {
            variables_enabled,
            eval_range,
        },
    );
    let mut variable_names = result
        .variables
        .into_iter()
        .map(|entry| entry.normalized)
        .collect::<Vec<_>>();
    variable_names.sort();
    variable_names.dedup();

    let cell_results = result
        .table_cell_results
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|entry| (entry.cell_index, entry.value))
                .collect()
        })
        .collect();

    CalcData {
        line_results: result.line_results,
        cell_results,
        variable_names,
    }
}

#[cfg(test)]
fn compute_calc_results(lines: &[String], variables_enabled: bool) -> Vec<Option<String>> {
    let engine = CalcEngine::new();
    compute_calc_data(&engine, lines, variables_enabled, None).line_results
}

/// Decide whether an already-eligible line's trailing ` = <literal>` should
/// be rewritten to `new_result`. The caller is responsible for the eligibility
/// gate (line unchanged since last recompute AND previous backend result was
/// `None`, i.e. trailer was in sync). This helper only handles the per-line
/// mechanics: locate the trailer, short-circuit when already in sync, and
/// enforce the cursor guard so we never yank text from under the caret.
///
/// Returns `Some((eq_byte_idx, new_tail))` so the caller can run
/// `line.replace_range(eq_byte_idx.., &new_tail)`, or `None` to leave the
/// line untouched.
fn compute_calc_trailer_refresh(
    line: &str,
    new_result: &str,
    is_cursor_line: bool,
    cursor_col: usize,
) -> Option<(usize, String)> {
    let refresh = crate::editor_core::calc_plan::compute_calc_trailer_refresh(
        line,
        new_result,
        is_cursor_line,
        cursor_col,
    )?;
    Some((refresh.eq_byte_idx, refresh.new_tail))
}

fn find_calc_segment_range(text: &str) -> Option<(usize, usize)> {
    let segment = crate::editor_core::calc_plan::find_calc_segment(text)?;
    Some((segment.from_byte, segment.to_byte))
}

#[derive(Clone, Copy)]
struct TableCellInfo {
    left_pipe: usize,
    right_pipe: usize,
    trim_start: usize,
    trim_end: usize,
}

fn is_markdown_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|')
}

fn table_cell_info_at_char(line: &str, col_char: usize) -> Option<TableCellInfo> {
    if !is_markdown_table_line(line) {
        return None;
    }

    let col_byte = byte_index(line, col_char);
    let pipes = crate::editor_core::table::table_pipe_positions(line);
    if pipes.len() < 2 {
        return None;
    }
    let cell_index = crate::editor_core::table::table_cell_index_for_column(&pipes, col_byte)?;
    let span = crate::editor_core::table::table_cell_span(line, &pipes, cell_index)?;
    Some(TableCellInfo {
        left_pipe: span.left_pipe,
        right_pipe: span.right_pipe,
        trim_start: span.trim_start,
        trim_end: span.trim_end,
    })
}

fn table_cell_is_empty(cell: &TableCellInfo) -> bool {
    cell.trim_end <= cell.trim_start
}

fn table_cell_edit_start(cell: &TableCellInfo) -> usize {
    (cell.left_pipe + 2).min(cell.right_pipe)
}

fn table_cell_navigation_anchor(line: &str, cell: &TableCellInfo) -> usize {
    let edit_start = table_cell_edit_start(cell);
    let anchor_byte = if table_cell_is_empty(cell) {
        edit_start
    } else {
        ((cell.left_pipe + 1) + cell.trim_end).min(cell.right_pipe)
    };
    line[..anchor_byte].chars().count()
}

struct TableFormulaSegment {
    from_byte: usize,
    to_byte: usize,
    from_char: usize,
    to_char: usize,
    cell_from_char: usize,
    cell_to_char: usize,
    cell_index: usize,
    #[cfg(test)]
    labels: Vec<String>,
}

#[cfg(test)]
fn builtin_formula_label(text: &str) -> Option<String> {
    crate::editor_core::calc_plan::builtin_formula_label(text)
}

#[cfg(test)]
fn find_table_formula_segment(text: &str) -> Option<TableFormulaSegment> {
    find_table_formula_segments(text).into_iter().next()
}

fn find_table_formula_segments(text: &str) -> Vec<TableFormulaSegment> {
    crate::editor_core::calc_plan::find_table_formula_segments(text)
        .into_iter()
        .map(|seg| TableFormulaSegment {
            from_byte: seg.from_byte,
            to_byte: seg.to_byte,
            from_char: seg.from_char,
            to_char: seg.to_char,
            cell_from_char: seg.cell_left_pipe_char + 1,
            cell_to_char: seg.cell_right_pipe_char,
            cell_index: seg.cell_index,
            #[cfg(test)]
            labels: seg.labels,
        })
        .collect()
}

fn formula_marker_token(index: usize) -> String {
    "*".repeat(index + 1)
}

#[cfg(test)]
fn should_mask_formula_cell(
    is_cursor_line: bool,
    cursor_col: usize,
    formula: &TableFormulaSegment,
) -> bool {
    !(is_cursor_line && cursor_col >= formula.cell_from_char && cursor_col < formula.cell_to_char)
}

fn format_formula_display_value(raw: &str) -> String {
    crate::editor_core::calc_plan::format_formula_display_value(raw)
}

fn contains_assignment_operator(text: &str) -> bool {
    crate::editor_core::calc_plan::contains_assignment_operator(text)
}

#[cfg(test)]
mod tests {
    use super::folding::describe_fold_ranges;
    use super::input::Key;
    use super::{
        builtin_formula_label, compute_calc_results, compute_calc_trailer_refresh,
        find_calc_segment_range, find_table_formula_segment, format_formula_display_value,
        rendered_line_display_cols, should_mask_formula_cell, table_cell_info_at_char,
        table_cell_is_empty, table_cell_navigation_anchor,
    };
    use super::{display_cols_for_prefix, line_char_len};
    use super::{TerminalApp, TerminalOptions, UiMode};
    use crate::storage::Db;
    use std::fs;
    use std::path::PathBuf;
    use ulid::Ulid;

    fn temp_db_path() -> PathBuf {
        std::env::temp_dir().join(format!("note-terminal-test-{}.db", Ulid::new()))
    }

    fn cleanup_db_files(path: &PathBuf) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(format!("{}-wal", path.display()));
        let _ = fs::remove_file(format!("{}-shm", path.display()));
    }

    fn app_with_note(body: &str) -> (Db, TerminalApp, PathBuf) {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let note_id = "n1";
        db.save_note(note_id, body).expect("note saved");
        let opts = TerminalOptions {
            create_new: false,
            note_id: Some(note_id.to_string()),
            list_only: false,
        };
        let (mut app, _) = TerminalApp::new_with_startup_metrics(
            &db,
            &opts,
            true,
            false,
            true,
            true,
            true,
            super::render::RenderPalette::default(),
            "%Y-%m-%d".to_string(),
            "%Y-%m-%d %H:%M".to_string(),
        )
        .expect("terminal app");
        app.mode = UiMode::Editor;
        (db, app, path)
    }

    fn run_keys(app: &mut TerminalApp, db: &Db, keys: &[Key]) {
        for key in keys {
            app.handle_key(db, key.clone())
                .expect("key sequence should apply");
        }
    }

    #[test]
    fn load_note_reminder_ghosts_reconciles_shift_without_dropping_adjacent_reminders() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let note_id = "n1";
        db.save_note(note_id, "a\nb\nc").expect("note saved");
        db.upsert_reminder(note_id, 1, 1_900_000_000_000, "2030-03-10 09:00", "a")
            .expect("reminder a");
        db.upsert_reminder(note_id, 2, 1_900_000_100_000, "2030-03-10 09:05", "b")
            .expect("reminder b");

        let lines = vec![
            "x".to_string(),
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
        ];
        let ghosts = super::load_note_reminder_ghosts(&db, note_id, &lines).expect("load ghosts");
        assert!(ghosts.contains_key(&1));
        assert!(ghosts.contains_key(&2));

        let persisted = db.list_reminders(note_id).expect("list reminders");
        let persisted_lines = persisted
            .iter()
            .map(|entry| entry.line_number)
            .collect::<Vec<_>>();
        assert_eq!(persisted_lines, vec![2, 3]);

        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn load_note_reminder_ghosts_updates_line_text_when_line_changes_in_place() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let note_id = "n1";
        db.save_note(note_id, "alpha").expect("note saved");
        db.upsert_reminder(note_id, 1, 1_900_000_000_000, "2030-03-10 09:00", "alpha")
            .expect("reminder");

        let lines = vec!["alpha updated".to_string()];
        let ghosts = super::load_note_reminder_ghosts(&db, note_id, &lines).expect("load ghosts");
        let ghost = ghosts.get(&0).expect("ghost on first line");
        assert_eq!(ghost.line_text, "alpha updated");

        let persisted = db.list_reminders(note_id).expect("list reminders");
        assert_eq!(persisted[0].line_number, 1);
        assert_eq!(persisted[0].line_text, "alpha updated");

        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn display_cols_for_prefix_expands_tabs_without_clamping() {
        assert_eq!(display_cols_for_prefix("\tabc", 1), 4);
        assert_eq!(display_cols_for_prefix("\tabc", 4), 7);
    }

    #[test]
    fn gutter_width_expands_after_four_digit_line_numbers() {
        assert_eq!(super::gutter_width_for_visible_lines(1), 6);
        assert_eq!(super::gutter_width_for_visible_lines(9_999), 6);
        assert_eq!(super::gutter_width_for_visible_lines(10_000), 7);
        assert_eq!(super::gutter_width_for_visible_lines(100_000), 8);
    }

    #[test]
    fn cursor_position_respects_expanded_gutter_width() {
        let body = vec!["x"; 10_000].join("\n");
        let (_db, mut app, path) = app_with_note(&body);
        app.cursor_line = 9_999;
        app.cursor_col = 0;
        app.adjust_scroll();

        let (rows, cols) = super::input::terminal_size();
        let (_row, cursor_col) = app.cursor_position(rows, cols);
        let expected = super::gutter_width_for_visible_lines(app.visible_line_count()) + 1;

        assert_eq!(expected, 8);
        assert_eq!(cursor_col, expected);

        drop(app);
        cleanup_db_files(&path);
    }

    #[test]
    #[ignore = "Temporarily disabled heavy large-file load tests"]
    fn large_doc_structural_edits_near_eof_keep_fold_maps_and_scroll_stable() {
        let body = vec!["alpha"; 100_000].join("\n");
        let (db, mut app, path) = app_with_note(&body);

        app.mode = UiMode::Editor;
        app.cursor_line = app.lines.len().saturating_sub(1);
        app.cursor_col = line_char_len(app.current_line());
        app.adjust_scroll();
        let insert_scroll_before = app.scroll_line;

        run_keys(&mut app, &db, &[Key::Enter]);

        assert_eq!(app.lines.len(), 100_001);
        assert_eq!(app.fold_real_to_visible.len(), app.lines.len());
        assert_eq!(app.fold_hidden_owner.len(), app.lines.len());
        assert_eq!(app.fold_placeholder_hidden_lines.len(), app.lines.len());
        assert_eq!(app.fold_range_by_start.len(), app.lines.len());
        assert!(app.scroll_line > 0);
        assert!(app.scroll_line >= insert_scroll_before.saturating_sub(1));

        app.mode = UiMode::Normal;
        app.cursor_line = app.lines.len().saturating_sub(2);
        app.cursor_col = 0;
        app.adjust_scroll();
        let delete_scroll_before = app.scroll_line;

        run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('d')]);

        assert_eq!(app.lines.len(), 100_000);
        assert_eq!(app.fold_real_to_visible.len(), app.lines.len());
        assert_eq!(app.fold_hidden_owner.len(), app.lines.len());
        assert_eq!(app.fold_placeholder_hidden_lines.len(), app.lines.len());
        assert_eq!(app.fold_range_by_start.len(), app.lines.len());
        assert!(app.scroll_line > 0);
        assert!(app.scroll_line >= delete_scroll_before.saturating_sub(2));

        let mut out = Vec::new();
        app.draw(&mut out).expect("draw after large-file EOF edits");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    #[ignore = "Temporarily disabled heavy large-file load tests"]
    fn large_doc_random_tail_edit_stress_keeps_state_consistent() {
        let body = vec!["tail"; 100_000].join("\n");
        let (db, mut app, path) = app_with_note(&body);

        let mut seed = 0xA11CE5EED_u64;
        let mut next_u64 = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            seed
        };

        for step_idx in 0..24usize {
            let len = app.lines.len().max(1);
            let tail_window = 500usize.min(len.saturating_sub(1)).max(1);
            let tail_start = len.saturating_sub(tail_window);
            let span = len.saturating_sub(tail_start).max(1);
            let target = tail_start + (next_u64() as usize % span);

            app.cursor_line = target.min(app.lines.len().saturating_sub(1));
            app.cursor_col = 0;
            app.mode = UiMode::Normal;
            app.vim_state = crate::editor_core::vim::VimState::default();
            app.adjust_cursor();
            app.adjust_scroll();

            let op = (next_u64() % 8) as usize;
            match op {
                0 => run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('d')]),
                1 => run_keys(&mut app, &db, &[Key::Char('o'), Key::Char('x'), Key::Esc]),
                2 => run_keys(&mut app, &db, &[Key::Char('O'), Key::Char('x'), Key::Esc]),
                3 => run_keys(&mut app, &db, &[Key::Char('A'), Key::Char('z'), Key::Esc]),
                4 => run_keys(&mut app, &db, &[Key::Char('x')]),
                5 => run_keys(&mut app, &db, &[Key::Char('i'), Key::Enter, Key::Esc]),
                6 => run_keys(&mut app, &db, &[Key::Char('j')]),
                _ => run_keys(&mut app, &db, &[Key::Char('k')]),
            }

            assert!(
                !app.lines.is_empty(),
                "step {step_idx}: lines unexpectedly empty after op {op}"
            );
            assert!(
                app.cursor_line < app.lines.len(),
                "step {step_idx}: cursor_line {} out of bounds {} after op {op}",
                app.cursor_line,
                app.lines.len()
            );
            assert_eq!(
                app.fold_real_to_visible.len(),
                app.lines.len(),
                "step {step_idx}: fold_real_to_visible size mismatch after op {op}"
            );
            assert_eq!(
                app.fold_hidden_owner.len(),
                app.lines.len(),
                "step {step_idx}: fold_hidden_owner size mismatch after op {op}"
            );
            assert_eq!(
                app.fold_placeholder_hidden_lines.len(),
                app.lines.len(),
                "step {step_idx}: fold_placeholder_hidden_lines size mismatch after op {op}"
            );
            assert_eq!(
                app.fold_range_by_start.len(),
                app.lines.len(),
                "step {step_idx}: fold_range_by_start size mismatch after op {op}"
            );
            assert!(
                app.scroll_line < app.visible_line_count(),
                "step {step_idx}: scroll_line {} out of visible range {} after op {op}",
                app.scroll_line,
                app.visible_line_count()
            );

            if step_idx % 4 == 0 {
                let mut out = Vec::new();
                app.draw(&mut out)
                    .expect("draw during large random tail edit stress");
            }
        }

        let mut out = Vec::new();
        app.draw(&mut out)
            .expect("draw after large random tail edit stress");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    #[ignore = "Temporarily disabled heavy large-file load tests"]
    fn large_doc_deferred_calc_reactivates_when_assignment_is_typed() {
        let body = vec!["plain"; 25_000].join("\n");
        let (db, mut app, path) = app_with_note(&body);

        assert!(app.calc_state_stale);
        assert!(!app.cached_has_builtin_formula);
        assert!(!app.cached_has_variable_assignment);

        app.mode = UiMode::Editor;
        app.cursor_line = app.lines.len().saturating_sub(1);
        app.cursor_col = 0;
        run_keys(
            &mut app,
            &db,
            &[
                Key::Char('t'),
                Key::Char('o'),
                Key::Char('t'),
                Key::Char('a'),
                Key::Char('l'),
                Key::Char(' '),
                Key::Char(':'),
                Key::Char('='),
                Key::Char(' '),
                Key::Char('2'),
            ],
        );

        assert!(app.cached_has_variable_assignment);
        assert!(!app.calc_state_stale);
        assert_eq!(app.prev_line_hashes.len(), app.lines.len());
        assert_eq!(app.prev_line_has_assignment.len(), app.lines.len());
        assert!(app.variable_names.iter().any(|name| name == "total"));

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn rendered_line_display_cols_accounts_for_calc_ghost() {
        assert_eq!(rendered_line_display_cols("2 + 2", Some("4")), 9);
        assert_eq!(rendered_line_display_cols("x := 1", Some("2")), 10);
    }

    #[test]
    fn fold_range_builder_detects_heading_fence_list_table_and_paragraph_blocks() {
        let ranges = describe_fold_ranges(&[
            "# top",
            "para one",
            "para two",
            "## sub",
            "```rs",
            "let total = 1;",
            "```",
            "- first",
            "- second",
            "| a |",
            "| - |",
            "| b |",
            "plain one",
            "plain two",
            "",
        ]);

        assert!(ranges.contains(&(0, 14, "heading")));
        assert!(ranges.contains(&(3, 14, "heading")));
        assert!(ranges.contains(&(4, 6, "fence")));
        assert!(ranges.contains(&(7, 8, "list")));
        assert!(ranges.contains(&(9, 11, "table")));
        assert!(ranges.contains(&(1, 2, "paragraph")));
        assert!(ranges.contains(&(12, 13, "paragraph")));
    }

    #[test]
    fn heading_folds_stop_at_same_level_only() {
        let ranges =
            describe_fold_ranges(&["## parent", "### child", "details", "## sibling", "tail"]);

        assert!(ranges.contains(&(0, 2, "heading")));
        assert!(ranges.contains(&(1, 4, "heading")));
        assert!(ranges.contains(&(3, 4, "heading")));
    }

    #[test]
    fn normal_mode_za_toggles_fold_and_vertical_navigation_uses_virtual_lines() {
        let (db, mut app, path) = app_with_note("# h1\none\ntwo\n# h2\nthree");
        app.mode = UiMode::Normal;
        app.cursor_line = 0;

        run_keys(&mut app, &db, &[Key::Char('z'), Key::Char('a')]);

        assert!(app.collapsed_fold_starts.contains(&0));
        assert_eq!(app.fold_visible_to_real, vec![0, 3, 4]);
        assert_eq!(app.fold_placeholder_hidden_lines[0], Some(2));

        run_keys(&mut app, &db, &[Key::Char('j')]);
        assert_eq!(app.cursor_line, 3);
        assert_eq!(app.current_virtual_line(), 1);

        app.cursor_line = 1;
        app.adjust_cursor();
        assert_eq!(app.cursor_line, 0);

        run_keys(&mut app, &db, &[Key::Char('z'), Key::Char('a')]);
        assert!(!app.collapsed_fold_starts.contains(&0));

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn horizontal_scroll_clamps_to_last_visible_window_at_line_end_in_normal_mode() {
        let long = "a".repeat(200);
        let (_db, mut app, path) = app_with_note(&long);
        app.mode = UiMode::Normal;
        app.cursor_line = 0;
        app.cursor_col = line_char_len(app.current_line());
        app.adjust_scroll();

        let (_rows, cols) = super::input::terminal_size();
        let available = cols.saturating_sub(super::GUTTER_WIDTH);
        let expected = super::line_display_cols(app.current_line()).saturating_sub(available);

        assert_eq!(app.scroll_col, expected);

        drop(app);
        cleanup_db_files(&path);
    }

    #[test]
    fn horizontal_scroll_allows_insert_end_slot_on_overflow_line_end() {
        let long = "a".repeat(200);
        let (_db, mut app, path) = app_with_note(&long);
        app.mode = UiMode::Editor;
        app.cursor_line = 0;
        app.cursor_col = line_char_len(app.current_line());
        app.adjust_scroll();

        let (_rows, cols) = super::input::terminal_size();
        let available = cols.saturating_sub(super::GUTTER_WIDTH);
        let expected = super::line_display_cols(app.current_line())
            .saturating_sub(available)
            .saturating_add(1);

        assert_eq!(app.scroll_col, expected);

        let line_width = super::line_display_cols(app.current_line());
        let viewport = super::compute_line_viewport(line_width, app.scroll_col, available);
        assert!(!viewport.has_right_overflow);
        assert_eq!(
            viewport.text_window_col + viewport.text_width,
            line_width + 1
        );

        drop(app);
        cleanup_db_files(&path);
    }

    #[test]
    fn sample_overflow_line_places_cursor_on_last_screen_cell_in_insert_and_normal() {
        let sample = "- [ ] Automatic link handling in the form of [link](link) with optional [link] text update. Show only [link] by default. dfsasjf hsjabshga sfhdghksaghkdfgbghb ahgbsadfhgkbfa ghbf";
        let (_db, mut app, path) = app_with_note(sample);
        let (rows, cols) = super::input::terminal_size();
        let available = cols.saturating_sub(super::GUTTER_WIDTH);
        let line_width = super::line_display_cols(app.current_line());

        app.cursor_line = 0;
        app.cursor_col = line_char_len(app.current_line());
        app.mode = UiMode::Editor;
        app.adjust_scroll();
        let (_row, col_insert) = app.cursor_position(rows, cols);
        assert_eq!(col_insert, cols);
        assert!(line_width <= app.scroll_col.saturating_add(available));

        app.mode = UiMode::Normal;
        app.adjust_scroll();
        let (_row, col_normal) = app.cursor_position(rows, cols);
        assert_eq!(col_normal, cols);
        assert!(line_width <= app.scroll_col.saturating_add(available));

        drop(app);
        cleanup_db_files(&path);
    }

    #[test]
    fn normal_mode_cursor_at_logical_line_end_renders_on_last_character_cell() {
        let (_db, mut app, path) = app_with_note("UI settings page");
        let (rows, cols) = super::input::terminal_size();
        app.mode = UiMode::Normal;
        app.cursor_line = 0;
        app.cursor_col = line_char_len(app.current_line());
        app.adjust_scroll();

        let (_row, cursor_col) = app.cursor_position(rows, cols);
        let expected = super::GUTTER_WIDTH + line_char_len(app.current_line());
        assert_eq!(cursor_col, expected);

        drop(app);
        cleanup_db_files(&path);
    }

    #[test]
    fn append_line_end_on_overflow_keeps_last_character_visible_and_cursor_at_screen_edge() {
        let sample = "- [ ] Automatic link handling in the form of [link](link) with optional [link] text update. Show only [link] by default. dfsasjf hsjabshga sfhdghksaghkdfgbghb ahgbsadfhgkbfa ghbf";
        let (db, mut app, path) = app_with_note(sample);
        app.mode = UiMode::Normal;
        app.vim_state = crate::editor_core::vim::VimState::default();
        app.cursor_line = 0;
        app.cursor_col = 0;
        app.scroll_col = 0;

        run_keys(&mut app, &db, &[Key::Char('A')]);

        let (rows, cols) = super::input::terminal_size();
        let available = cols.saturating_sub(super::GUTTER_WIDTH);
        let line_width = super::line_display_cols(app.current_line());
        let viewport = super::compute_line_viewport(line_width, app.scroll_col, available);
        let (_row, cursor_col) = app.cursor_position(rows, cols);

        assert_eq!(app.mode, UiMode::Editor);
        assert_eq!(app.cursor_col, line_char_len(app.current_line()));
        assert_eq!(cursor_col, cols);
        assert_eq!(
            viewport.text_window_col + viewport.text_width,
            line_width + 1,
            "insert mode should reserve one visual end-slot past final character",
        );

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn normal_mode_dollar_then_a_stays_on_same_line_and_enters_insert_at_line_end() {
        let (db, mut app, path) = app_with_note("alpha\nbeta");
        app.mode = UiMode::Normal;
        app.cursor_line = 0;
        app.cursor_col = 0;

        run_keys(&mut app, &db, &[Key::Char('$'), Key::Char('a')]);

        assert_eq!(app.mode, UiMode::Editor);
        assert_eq!(app.cursor_line, 0);
        assert_eq!(app.cursor_col, line_char_len("alpha"));

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn enter_from_overflowing_checklist_repositions_cursor_and_resets_horizontal_scroll() {
        let long = format!("- [ ] {}", "a".repeat(200));
        let (db, mut app, path) = app_with_note(&long);
        app.cursor_line = 0;
        app.cursor_col = line_char_len(app.current_line());
        app.adjust_scroll();
        assert!(app.scroll_col > 0);

        app.handle_editor_key(&db, Key::Enter)
            .expect("enter applies");

        assert_eq!(app.cursor_line, 1);
        assert_eq!(app.lines[1], "- [ ] ");
        assert_eq!(app.cursor_col, line_char_len("- [ ] "));
        assert_eq!(app.scroll_col, 0);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn enter_from_overflowing_unordered_list_repositions_cursor_and_resets_horizontal_scroll() {
        let long = format!("- {}", "a".repeat(200));
        let (db, mut app, path) = app_with_note(&long);
        app.cursor_line = 0;
        app.cursor_col = line_char_len(app.current_line());
        app.adjust_scroll();
        assert!(app.scroll_col > 0);

        app.handle_editor_key(&db, Key::Enter)
            .expect("enter applies");

        assert_eq!(app.cursor_line, 1);
        assert_eq!(app.lines[1], "- ");
        assert_eq!(app.cursor_col, line_char_len("- "));
        assert_eq!(app.scroll_col, 0);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn enter_from_overflowing_ordered_list_repositions_cursor_and_resets_horizontal_scroll() {
        let long = format!("9. {}", "a".repeat(200));
        let (db, mut app, path) = app_with_note(&long);
        app.cursor_line = 0;
        app.cursor_col = line_char_len(app.current_line());
        app.adjust_scroll();
        assert!(app.scroll_col > 0);

        app.handle_editor_key(&db, Key::Enter)
            .expect("enter applies");

        assert_eq!(app.cursor_line, 1);
        assert_eq!(app.lines[1], "10. ");
        assert_eq!(app.cursor_col, line_char_len("10. "));
        assert_eq!(app.scroll_col, 0);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn enter_from_overflowing_table_row_repositions_cursor_and_resets_horizontal_scroll() {
        let long_cell = "a".repeat(180);
        let row = format!("| col |\n| --- |\n| {} |", long_cell);
        let (db, mut app, path) = app_with_note(&row);
        app.cursor_line = 2;
        app.cursor_col = line_char_len(app.current_line());
        app.adjust_scroll();
        assert!(app.scroll_col > 0);

        app.handle_editor_key(&db, Key::Enter)
            .expect("enter applies");

        assert_eq!(app.cursor_line, 3);
        assert!(app.lines[3].starts_with("| "));
        assert!(app.lines[3].ends_with(" |"));
        assert_eq!(app.cursor_col, 2);
        assert_eq!(app.scroll_col, 0);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn find_calc_segment_range_detects_single_table_expression_cell() {
        let line = "| name | 4+2 |";
        let Some((from, to)) = find_calc_segment_range(line) else {
            panic!("expected table segment");
        };
        assert_eq!(&line[from..to], "4+2");
    }

    #[test]
    fn find_calc_segment_range_detects_builtin_formula_cell() {
        let line = "| name | =avg_col() | 1.91 |";
        let Some((from, to)) = find_calc_segment_range(line) else {
            panic!("expected formula segment");
        };
        assert_eq!(&line[from..to], "=avg_col()");
    }

    #[test]
    fn builtin_formula_label_normalizes_aliases() {
        assert_eq!(
            builtin_formula_label("=avg_col()").as_deref(),
            Some("avg_col()")
        );
        assert_eq!(
            builtin_formula_label(" sum_column ( ) ").as_deref(),
            Some("sum_col()")
        );
        assert_eq!(builtin_formula_label("2+2"), None);
    }

    #[test]
    fn format_formula_display_value_rounds_and_strips_approximation_text() {
        assert_eq!(format_formula_display_value("6.666666"), "6.67");
        assert_eq!(format_formula_display_value("≈ 6.666666"), "6.67");
        assert_eq!(format_formula_display_value("approximately 12.000"), "12");
        assert_eq!(format_formula_display_value("5.555 m"), "5.56 m");
    }

    #[test]
    fn find_table_formula_segment_extracts_cell_bounds_and_label() {
        let line = "| a | =sum_column() | 9 |";
        let Some(seg) = find_table_formula_segment(line) else {
            panic!("expected table formula segment");
        };
        assert_eq!(&line[seg.from_byte..seg.to_byte], "=sum_column()");
        assert_eq!(seg.labels, vec!["sum_col()"]);
        assert!(seg.from_char < seg.to_char);
    }

    #[test]
    fn find_table_formula_segment_extracts_chained_formula_labels_in_order() {
        let line = "| sum_col() * a + avg_col() |";
        let Some(seg) = find_table_formula_segment(line) else {
            panic!("expected table formula segment");
        };
        assert_eq!(
            &line[seg.from_byte..seg.to_byte],
            "sum_col() * a + avg_col()"
        );
        assert_eq!(seg.labels, vec!["sum_col()", "avg_col()"]);
    }

    #[test]
    fn find_table_formula_segment_tracks_full_cell_bounds() {
        let line = "| a | =sum_col() | 9 |";
        let Some(seg) = find_table_formula_segment(line) else {
            panic!("expected table formula segment");
        };
        assert!(seg.cell_from_char < seg.from_char);
        assert!(seg.to_char < seg.cell_to_char);
    }

    #[test]
    fn should_mask_formula_cell_reveals_when_cursor_is_anywhere_in_formula_cell() {
        let line = "| a | =sum_col() |";
        let seg = find_table_formula_segment(line).expect("formula segment");
        assert!(should_mask_formula_cell(false, seg.from_char, &seg));
        assert!(!should_mask_formula_cell(true, seg.cell_from_char, &seg));
        assert!(!should_mask_formula_cell(
            true,
            seg.cell_to_char.saturating_sub(1),
            &seg
        ));
        assert!(should_mask_formula_cell(true, seg.cell_to_char, &seg));
    }

    #[test]
    fn table_cell_navigation_anchor_uses_padding_for_empty_and_word_end_for_non_empty() {
        let line = "| aaa |     | bb  |";
        let first_cell = table_cell_info_at_char(line, 2).expect("first cell");
        assert!(!table_cell_is_empty(&first_cell));
        assert_eq!(table_cell_navigation_anchor(line, &first_cell), 5);

        let empty_cell = table_cell_info_at_char(line, 8).expect("empty cell");
        assert!(table_cell_is_empty(&empty_cell));
        assert_eq!(table_cell_navigation_anchor(line, &empty_cell), 8);

        let third_cell = table_cell_info_at_char(line, 14).expect("third cell");
        assert!(!table_cell_is_empty(&third_cell));
        assert_eq!(table_cell_navigation_anchor(line, &third_cell), 16);
    }

    #[test]
    fn find_calc_segment_range_detects_list_body() {
        let line = "- [ ] subtotal + tax";
        let Some((from, to)) = find_calc_segment_range(line) else {
            panic!("expected list segment");
        };
        assert_eq!(&line[from..to], "subtotal + tax");
    }

    #[test]
    fn compute_calc_results_resolves_reactive_variables() {
        let lines = vec!["x := 4".to_string(), "x + 2".to_string()];
        let results = compute_calc_results(&lines, true);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_deref(), None);
        assert_eq!(results[1].as_deref(), Some("6"));
    }

    #[test]
    fn compute_calc_results_resolves_variables_when_assignment_has_trailer_literal() {
        let lines = vec![
            "total := 12 = 12".to_string(),
            "value := total + 3 = 15".to_string(),
            "value + 1".to_string(),
        ];
        let results = compute_calc_results(&lines, true);
        assert_eq!(
            results,
            vec![None, Some("15".to_string()), Some("16".to_string())]
        );
    }

    #[test]
    fn tab_applies_table_calc_with_variables_and_positions_cursor_at_insert_end() {
        let (db, mut app, path) = app_with_note("x := 4\n| value | x + 2 |");
        app.cursor_line = 1;
        app.cursor_col = line_char_len(app.current_line());

        app.handle_editor_key(&db, Key::Tab).expect("tab applies");

        assert_eq!(app.lines[1], "| value | 6 |");
        let expected_byte = app.lines[1].find("6").expect("result exists") + "6".len();
        let expected_col = app.lines[1][..expected_byte].chars().count();
        assert_eq!(app.cursor_col, expected_col);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn adjust_cursor_snaps_empty_table_cells_to_padding_start() {
        let (db, mut app, path) = app_with_note("| aaa |     | bb  |");
        app.cursor_col = 9;
        app.adjust_cursor();
        assert_eq!(app.cursor_col, 8);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn arrow_navigation_does_not_jump_across_empty_table_cells() {
        let (db, mut app, path) = app_with_note("| aaa |     | bb  |");
        app.cursor_col = 9;
        app.handle_editor_key(&db, Key::ArrowRight)
            .expect("move to empty anchor");
        assert_eq!(app.cursor_col, 8);

        app.handle_editor_key(&db, Key::ArrowRight)
            .expect("regular right stays in current cell");
        assert_eq!(app.cursor_col, 8);

        app.cursor_col = 8;
        app.handle_editor_key(&db, Key::ArrowLeft)
            .expect("regular left stays in current cell");
        assert_eq!(app.cursor_col, 8);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn arrow_right_from_content_end_does_not_jump_to_next_cell() {
        let (db, mut app, path) = app_with_note("| aaa | bb  |");
        app.cursor_col = 5; // end of first cell content
        app.handle_editor_key(&db, Key::ArrowRight)
            .expect("stay at current cell content end");
        assert_eq!(app.cursor_col, 5);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn ctrl_arrow_moves_between_table_cells() {
        let (db, mut app, path) = app_with_note("| aaa | bb  |");
        app.cursor_col = 5; // first cell end
        app.handle_editor_key(&db, Key::CtrlArrowRight)
            .expect("ctrl-right jumps to next cell");
        assert_eq!(app.cursor_col, 10);

        app.handle_editor_key(&db, Key::CtrlArrowLeft)
            .expect("ctrl-left jumps to previous cell");
        assert_eq!(app.cursor_col, 5);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn ctrl_arrow_does_not_fallback_to_word_motion_inside_table() {
        let (db, mut app, path) = app_with_note("| aaa | bb  |");
        app.cursor_col = 5; // first cell end
        app.handle_editor_key(&db, Key::CtrlArrowLeft)
            .expect("ctrl-left in first cell is constrained");
        assert_eq!(app.cursor_col, 5);

        app.cursor_col = 10; // last cell end
        app.handle_editor_key(&db, Key::CtrlArrowRight)
            .expect("ctrl-right in last cell is constrained");
        assert_eq!(app.cursor_col, 10);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn backspace_and_delete_are_isolated_within_table_cell() {
        let (db, mut app, path) = app_with_note("| aaa |     | bb  |");
        let original = app.lines[0].clone();
        app.cursor_col = 8; // empty middle cell anchor
        app.handle_editor_key(&db, Key::Backspace)
            .expect("backspace in empty cell");
        assert_eq!(app.lines[0], original);
        assert_eq!(app.cursor_col, 8);

        app.handle_editor_key(&db, Key::Delete)
            .expect("delete in empty cell");
        assert_eq!(app.lines[0], original);
        assert_eq!(app.cursor_col, 8);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn ctrl_w_removes_table_column_when_header_cell_empty() {
        let text = "| a |  | c |\n| --- | --- | --- |\n| 1 | 2 | 3 |";
        let (db, mut app, path) = app_with_note(text);
        app.cursor_line = 0;
        app.cursor_col = app.lines[0].find("|  |").expect("empty header cell") + 2;

        app.handle_editor_key(&db, Key::Ctrl('w'))
            .expect("ctrl-w removes empty header column");

        assert_eq!(app.lines.len(), 3);
        for line in &app.lines {
            assert_eq!(line.matches('|').count(), 3, "line: {:?}", line);
        }
        assert!(app.lines[2].contains("1"));
        assert!(app.lines[2].contains("3"));
        assert!(!app.lines[2].contains("2"));
        assert_eq!(app.cursor_line, 0);
        assert_eq!(app.cursor_col, 3);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn ctrl_w_deletes_word_outside_table() {
        let (db, mut app, path) = app_with_note("alpha beta");
        app.cursor_col = app.lines[0].len();

        app.handle_editor_key(&db, Key::Ctrl('w'))
            .expect("ctrl-w deletes previous word");

        assert_eq!(app.lines[0], "alpha ");
        assert_eq!(app.cursor_col, "alpha ".len());

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn ctrl_backspace_and_ctrl_delete_merge_adjacent_table_cells() {
        let (db, mut app, path) = app_with_note("| aaa | bb |");

        app.cursor_col = 8; // start of second cell content
        app.handle_editor_key(&db, Key::CtrlBackspace)
            .expect("ctrl-backspace merges with previous cell");
        assert_eq!(app.lines[0], "| aaa bb |");

        app.lines[0] = "| aaa | bb |".to_string();
        app.cursor_col = 5; // end of first cell content
        app.handle_editor_key(&db, Key::CtrlDelete)
            .expect("ctrl-delete merges with next cell");
        assert_eq!(app.lines[0], "| aaa bb |");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn ctrl_backspace_and_ctrl_delete_remove_table_column_when_header_cell_empty() {
        let text = "| a |  | c |\n| --- | --- | --- |\n| 1 | 2 | 3 |";
        let assert_middle_column_removed = |lines: &[String]| {
            assert_eq!(lines.len(), 3);
            for line in lines {
                assert_eq!(line.matches('|').count(), 3, "line: {:?}", line);
            }
            assert!(lines[2].contains("1"));
            assert!(lines[2].contains("3"));
            assert!(!lines[2].contains("2"));
        };

        let (db, mut app, path) = app_with_note(text);
        app.cursor_line = 0;
        app.cursor_col = app.lines[0].find("|  |").expect("empty header cell") + 2;
        app.handle_editor_key(&db, Key::CtrlBackspace)
            .expect("ctrl-backspace removes empty header column");
        assert_middle_column_removed(&app.lines);
        assert_eq!(app.cursor_line, 0);
        assert_eq!(app.cursor_col, 3);
        drop(app);
        drop(db);
        cleanup_db_files(&path);

        let (db, mut app, path) = app_with_note(text);
        app.cursor_line = 0;
        app.cursor_col = app.lines[0].find("|  |").expect("empty header cell") + 2;
        app.handle_editor_key(&db, Key::CtrlDelete)
            .expect("ctrl-delete removes empty header column");
        assert_middle_column_removed(&app.lines);
        assert_eq!(app.cursor_line, 0);
        assert_eq!(app.cursor_col, 3);
        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vertical_movement_into_table_cell_snaps_to_cell_end() {
        let (db, mut app, path) = app_with_note("plain\n| aaa | bb  |");
        app.cursor_line = 0;
        app.cursor_col = 0;
        app.handle_editor_key(&db, Key::ArrowDown)
            .expect("move into table row");
        assert_eq!(app.cursor_line, 1);
        assert_eq!(app.cursor_col, 5);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn typing_space_in_table_cell_allows_followup_word_input() {
        let (db, mut app, path) = app_with_note("| aaa |");
        app.cursor_col = 5; // end of content
        app.handle_editor_key(&db, Key::Char(' '))
            .expect("insert space");
        app.handle_editor_key(&db, Key::Char('b'))
            .expect("insert next word char");
        assert_eq!(app.lines[0], "| aaa b |");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn typing_in_table_cell_reflows_column_when_cell_becomes_widest() {
        let (db, mut app, path) = app_with_note("| a | b |\n| --- | --- |\n| 1 | 2 |");
        app.cursor_line = 2;
        app.cursor_col = 3; // end of first cell content in row 3

        run_keys(
            &mut app,
            &db,
            &[
                Key::Char('2'),
                Key::Char('3'),
                Key::Char('4'),
                Key::Char('5'),
            ],
        );

        assert_eq!(app.lines[0], "| a     | b   |");
        assert_eq!(app.lines[1], "| ----- | --- |");
        assert_eq!(app.lines[2], "| 12345 | 2   |");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn compute_calc_trailer_refresh_rewrites_stale_literal() {
        let out = compute_calc_trailer_refresh("1 + 1 = 2", "3", false, 0);
        let Some((eq_idx, tail)) = out else {
            panic!("expected refresh");
        };
        assert_eq!(eq_idx, 5);
        assert_eq!(tail, " = 3");
    }

    #[test]
    fn compute_calc_trailer_refresh_skips_when_missing_trailer() {
        let out = compute_calc_trailer_refresh("1 + 1", "3", false, 0);
        assert_eq!(out, None);
    }

    #[test]
    fn compute_calc_trailer_refresh_already_in_sync() {
        // Caller passed the eligibility gate but the line's trailer already
        // equals the new result — nothing to do.
        let out = compute_calc_trailer_refresh("1 + 1 = 3", "3", false, 0);
        assert_eq!(out, None);
    }

    #[test]
    fn compute_calc_trailer_refresh_skips_when_cursor_inside_trailer() {
        // Cursor sits on the space before `=`; treat the whole trailer as
        // off-limits so we don't yank text out from under the caret.
        let out = compute_calc_trailer_refresh("1 + 1 = 2", "3", true, 5);
        assert_eq!(out, None);
    }

    #[test]
    fn compute_calc_trailer_refresh_runs_when_cursor_is_before_trailer() {
        let out = compute_calc_trailer_refresh("1 + 1 = 2", "3", true, 0);
        assert!(out.is_some());
    }

    #[test]
    fn recompute_calc_refreshes_stale_trailer_after_variable_change() {
        let (db, mut app, path) = app_with_note("rate := 10\n2 * rate = 20\nother line");
        // Move the cursor out of the trailer so the refresh is not guarded.
        app.cursor_line = 0;
        app.cursor_col = 0;
        // Seed: a recompute now should leave the trailer alone (already in sync).
        app.recompute_calc_full();
        assert_eq!(app.lines[1], "2 * rate = 20");

        // Change the variable definition.
        app.lines[0] = "rate := 15".to_string();
        app.recompute_calc_full();

        // Trailer should have been refreshed from `= 20` to `= 30`.
        assert_eq!(app.lines[1], "2 * rate = 30");
        assert_eq!(app.lines[2], "other line");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn recompute_calc_does_not_refresh_hand_typed_trailer() {
        // The user typed `= FOO` by hand; it never matched a backend result,
        // so subsequent recomputes must not clobber it even when the left
        // side becomes reactively different.
        let (db, mut app, path) = app_with_note("rate := 10\n2 * rate = FOO");
        app.cursor_line = 0;
        app.cursor_col = 0;
        app.recompute_calc_full();
        assert_eq!(app.lines[1], "2 * rate = FOO");

        app.lines[0] = "rate := 15".to_string();
        app.recompute_calc_full();
        assert_eq!(app.lines[1], "2 * rate = FOO");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn recompute_calc_skips_refresh_when_cursor_in_trailer() {
        let (db, mut app, path) = app_with_note("rate := 10\n2 * rate = 20");
        app.cursor_line = 0;
        app.cursor_col = 0;
        app.recompute_calc_full();

        // Park the cursor inside the trailer on line 1.
        app.cursor_line = 1;
        app.cursor_col = app.lines[1].chars().count(); // end of line, inside trailer
        app.lines[0] = "rate := 15".to_string();
        app.recompute_calc_full();

        // Untouched because cursor is in the trailer region.
        assert_eq!(app.lines[1], "2 * rate = 20");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn defer_calc_state_after_edit_clears_full_cached_results_vector() {
        let (db, mut app, path) = app_with_note("1 + 1\n2 + 2\n3 + 3");
        app.calc_results = vec![
            Some("2".to_string()),
            Some("4".to_string()),
            Some("6".to_string()),
        ];
        app.variable_names = vec!["total".to_string()];
        app.cursor_line = 1;

        app.defer_calc_state_after_edit();

        assert_eq!(app.calc_results, vec![None, None, None]);
        assert!(app.variable_names.is_empty());
        assert!(app.calc_state_stale);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn tab_applies_checklist_calc_with_variables_and_positions_cursor_at_insert_end() {
        let (db, mut app, path) = app_with_note("base := 10\n- [ ] base + 5");
        app.cursor_line = 1;
        app.cursor_col = line_char_len(app.current_line());

        app.handle_editor_key(&db, Key::Tab).expect("tab applies");

        assert_eq!(app.lines[1], "- [ ] 15");
        assert_eq!(app.cursor_col, app.lines[1].chars().count());

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn editor_paste_multiline_inserts_as_single_bulk_edit() {
        let (db, mut app, path) = app_with_note("start end");
        app.mode = UiMode::Editor;
        app.cursor_line = 0;
        app.cursor_col = 6; // after "start "

        app.handle_editor_key(&db, Key::Paste("a\nb\n".to_string()))
            .expect("paste applies");

        assert_eq!(
            app.lines,
            vec!["start a".to_string(), "b".to_string(), "end".to_string()]
        );
        assert_eq!(app.cursor_line, 2);
        assert_eq!(app.cursor_col, 0);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_golden_j_dd_deletes_current_line() {
        let (db, mut app, path) = app_with_note("one\ntwo\nthree");
        app.mode = UiMode::Normal;

        run_keys(
            &mut app,
            &db,
            &[Key::Char('j'), Key::Char('d'), Key::Char('d')],
        );
        assert_eq!(app.lines, vec!["one".to_string(), "three".to_string()]);
        assert_eq!(app.cursor_line, 1);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn ctrl_q_quits_from_normal_mode() {
        let (db, mut app, path) = app_with_note("one\ntwo\nthree");
        app.mode = UiMode::Normal;

        run_keys(&mut app, &db, &[Key::Ctrl('q')]);
        assert!(app.quit);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn clip_watch_commands_toggle_terminal_watcher() {
        let (db, mut app, path) = app_with_note("alpha");
        app.mode = UiMode::Normal;

        app.execute_terminal_command(&db, "clip-watch");
        assert!(app.clipboard_watch_enabled);
        assert_eq!(app.status, "clip-watch started");

        app.execute_terminal_command(&db, "clip-watch");
        assert!(app.clipboard_watch_enabled);
        assert_eq!(app.status, "clip-watch already active");

        app.execute_terminal_command(&db, "clip-watch-stop");
        assert!(!app.clipboard_watch_enabled);
        assert_eq!(app.status, "clip-watch stopped");

        app.execute_terminal_command(&db, "clip-watch-stop");
        assert!(!app.clipboard_watch_enabled);
        assert_eq!(app.status, "clip-watch not active");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn switcher_delete_cancel_keeps_note() {
        let (db, mut app, path) = app_with_note("first note");
        db.save_note("n2", "second note")
            .expect("second note saved");
        app.refresh_switcher_items(&db)
            .expect("switcher items refreshed");

        run_keys(
            &mut app,
            &db,
            &[Key::Ctrl('p'), Key::Char('s'), Key::Delete],
        );
        assert_eq!(app.mode, UiMode::Switcher);
        assert!(app.switcher_delete_confirm.is_some());

        run_keys(&mut app, &db, &[Key::Char('n')]);
        assert!(app.switcher_delete_confirm.is_none());
        assert!(db.get_note("n2").expect("lookup works").is_some());

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn switcher_ctrl_backspace_delete_removes_active_after_confirmation() {
        let (db, mut app, path) = app_with_note("first note");
        db.save_note("n2", "second note")
            .expect("second note saved");
        app.refresh_switcher_items(&db)
            .expect("switcher items refreshed");

        run_keys(
            &mut app,
            &db,
            &[
                Key::Ctrl('p'),
                Key::Paste("first".to_string()),
                Key::CtrlBackspace,
            ],
        );
        assert_eq!(app.mode, UiMode::Switcher);
        let pending = app
            .switcher_delete_confirm
            .as_ref()
            .expect("delete confirmation requested");
        assert_eq!(pending.note_id, "n1");

        run_keys(&mut app, &db, &[Key::Enter]);
        assert!(app.switcher_delete_confirm.is_none());
        assert!(db.get_note("n1").expect("lookup works").is_none());
        assert_ne!(app.active_note.id, "n1");
        assert!(db
            .get_note(&app.active_note.id)
            .expect("active note lookup")
            .is_some());

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn switcher_enter_opens_note_in_normal_mode() {
        let (db, mut app, path) = app_with_note("first note");
        db.save_note("n2", "second note")
            .expect("second note saved");
        app.refresh_switcher_items(&db)
            .expect("switcher items refreshed");
        app.mode = UiMode::Editor;

        run_keys(
            &mut app,
            &db,
            &[Key::Ctrl('p'), Key::Paste("second".to_string()), Key::Enter],
        );

        assert_eq!(app.active_note.id, "n2");
        assert_eq!(app.mode, UiMode::Normal);
        assert_eq!(app.vim_state.mode, crate::editor_core::vim::VimMode::Normal);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn fold_commands_toggle_terminal_folds_and_aliases() {
        let (db, mut app, path) = app_with_note("# h1\none\ntwo\n# h2\nthree");
        app.mode = UiMode::Normal;
        app.cursor_line = 0;

        app.execute_terminal_command(&db, "fold");
        assert!(app.collapsed_fold_starts.contains(&0));
        assert_eq!(app.fold_visible_to_real, vec![0, 3, 4]);

        app.execute_terminal_command(&db, "fold");
        assert_eq!(app.status, "fold: already folded");

        app.execute_terminal_command(&db, "unfold");
        assert!(!app.collapsed_fold_starts.contains(&0));

        app.execute_terminal_command(&db, "za");
        assert!(app.collapsed_fold_starts.contains(&0));

        app.execute_terminal_command(&db, "zo");
        assert!(!app.collapsed_fold_starts.contains(&0));

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_golden_counted_delete_2dd_deletes_two_lines() {
        let (db, mut app, path) = app_with_note("alpha\nbeta\ngamma\ndelta");
        app.mode = UiMode::Normal;

        run_keys(
            &mut app,
            &db,
            &[Key::Char('2'), Key::Char('d'), Key::Char('d')],
        );
        assert_eq!(app.lines, vec!["gamma".to_string(), "delta".to_string()]);
        assert_eq!(app.cursor_line, 0);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn visual_colon_runs_command_on_preserved_selection() {
        let (db, mut app, path) = app_with_note("alpha\nbeta\ngamma");
        app.mode = UiMode::Normal;

        run_keys(
            &mut app,
            &db,
            &[
                Key::Char('v'),
                Key::Char('j'),
                Key::Char(':'),
                Key::Char('c'),
                Key::Char('l'),
                Key::Char('i'),
                Key::Char('s'),
                Key::Char('t'),
                Key::Enter,
            ],
        );

        assert_eq!(app.mode, UiMode::Normal);
        assert_eq!(app.lines[0], "- [ ] alpha");
        assert_eq!(app.lines[1], "- [ ] beta");
        assert_eq!(app.lines[2], "gamma");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn visual_colon_keeps_selection_while_command_bar_is_open() {
        let (db, mut app, path) = app_with_note("alpha\nbeta\ngamma");
        app.mode = UiMode::Normal;

        run_keys(
            &mut app,
            &db,
            &[Key::Char('v'), Key::Char('j'), Key::Char(':')],
        );

        assert_eq!(app.mode, UiMode::CommandBar);
        assert!(app.selection_anchor.is_some());
        assert!(app.command_selection.is_some());
        assert!(!app.command_selection_linewise);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn visual_line_colon_runs_command_on_preserved_selection() {
        let (db, mut app, path) = app_with_note("alpha\nbeta\ngamma");
        app.mode = UiMode::Normal;

        run_keys(
            &mut app,
            &db,
            &[
                Key::Char('V'),
                Key::Char('j'),
                Key::Char(':'),
                Key::Char('o'),
                Key::Char('l'),
                Key::Char('i'),
                Key::Char('s'),
                Key::Char('t'),
                Key::Enter,
            ],
        );

        assert_eq!(app.mode, UiMode::Normal);
        assert_eq!(app.lines[0], "1. alpha");
        assert_eq!(app.lines[1], "2. beta");
        assert_eq!(app.lines[2], "gamma");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn visual_line_colon_keeps_linewise_selection_while_command_bar_is_open() {
        let (db, mut app, path) = app_with_note("alpha\nbeta\ngamma");
        app.mode = UiMode::Normal;

        run_keys(
            &mut app,
            &db,
            &[Key::Char('V'), Key::Char('j'), Key::Char(':')],
        );

        assert_eq!(app.mode, UiMode::CommandBar);
        assert!(app.selection_anchor.is_some());
        assert!(app.command_selection.is_some());
        assert!(app.command_selection_linewise);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_di_pipe_deletes_cell_contents() {
        let (db, mut app, path) = app_with_note("| one | two |");
        app.mode = UiMode::Normal;
        app.cursor_col = 3;

        run_keys(
            &mut app,
            &db,
            &[Key::Char('d'), Key::Char('i'), Key::Char('|')],
        );

        assert_eq!(app.lines, vec!["|  | two |".to_string()]);
        assert_eq!(app.cursor_col, 2);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_daw_deletes_word_with_padding() {
        let (db, mut app, path) = app_with_note("foo bar baz");
        app.mode = UiMode::Normal;
        app.cursor_col = 5;

        run_keys(
            &mut app,
            &db,
            &[Key::Char('d'), Key::Char('a'), Key::Char('w')],
        );

        assert_eq!(app.lines, vec!["foo baz".to_string()]);
        assert_eq!(app.cursor_col, 4);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_yaw_yanks_word_with_padding() {
        let (db, mut app, path) = app_with_note("foo bar baz");
        app.mode = UiMode::Normal;
        app.cursor_col = 5;

        run_keys(
            &mut app,
            &db,
            &[Key::Char('y'), Key::Char('a'), Key::Char('w')],
        );

        assert_eq!(app.lines, vec!["foo bar baz".to_string()]);
        assert_eq!(app.clipboard, vec!["bar ".to_string()]);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_dollar_and_d0_delete_line_ranges() {
        let (db, mut app, path) = app_with_note("alpha beta");
        app.mode = UiMode::Normal;
        app.cursor_col = 6;

        run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('$')]);
        assert_eq!(app.lines, vec!["alpha ".to_string()]);

        run_keys(&mut app, &db, &[Key::Char('u')]);
        assert_eq!(app.lines, vec!["alpha beta".to_string()]);

        app.cursor_col = 6;
        run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('0')]);
        assert_eq!(app.lines, vec!["beta".to_string()]);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_normal_mode_undo_redo_roundtrip() {
        let (db, mut app, path) = app_with_note("one\ntwo\nthree");
        app.mode = UiMode::Normal;

        run_keys(
            &mut app,
            &db,
            &[Key::Char('j'), Key::Char('d'), Key::Char('d')],
        );
        assert_eq!(app.lines, vec!["one".to_string(), "three".to_string()]);

        run_keys(&mut app, &db, &[Key::Char('u')]);
        assert_eq!(
            app.lines,
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );

        run_keys(&mut app, &db, &[Key::Ctrl('r')]);
        assert_eq!(app.lines, vec!["one".to_string(), "three".to_string()]);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_redo_is_cleared_after_new_edit() {
        let (db, mut app, path) = app_with_note("one\ntwo\nthree");
        app.mode = UiMode::Normal;

        run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('d')]);
        assert_eq!(app.lines, vec!["two".to_string(), "three".to_string()]);

        run_keys(&mut app, &db, &[Key::Char('u')]);
        assert_eq!(
            app.lines,
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );

        run_keys(
            &mut app,
            &db,
            &[Key::Char('j'), Key::Char('d'), Key::Char('d')],
        );
        assert_eq!(app.lines, vec!["one".to_string(), "three".to_string()]);

        // New edit after undo should invalidate redo history.
        run_keys(&mut app, &db, &[Key::Ctrl('r')]);
        assert_eq!(app.lines, vec!["one".to_string(), "three".to_string()]);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_undo_still_works_after_save() {
        let (db, mut app, path) = app_with_note("one\ntwo");
        app.mode = UiMode::Normal;

        run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('d')]);
        assert_eq!(app.lines, vec!["two".to_string()]);

        app.save(&db).expect("save succeeds");
        run_keys(&mut app, &db, &[Key::Char('u')]);
        assert_eq!(app.lines, vec!["one".to_string(), "two".to_string()]);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn undo_exhaustion_keeps_latest_cursor_location() {
        let (db, mut app, path) = app_with_note("one\ntwo");
        app.mode = UiMode::Editor;
        app.cursor_line = 1;
        app.cursor_col = 1;

        app.handle_editor_key(&db, Key::Char('x'))
            .expect("insert char");
        assert_eq!(app.lines[1], "txwo");
        assert_eq!(app.cursor_line, 1);
        assert_eq!(app.cursor_col, 2);

        app.undo();

        assert_eq!(app.lines, vec!["one".to_string(), "two".to_string()]);
        assert_eq!(app.cursor_line, 1);
        assert_eq!(app.cursor_col, 2);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_n_and_shift_n_cycle_last_search_matches() {
        let (db, mut app, path) = app_with_note("alpha\nbeta alpha\nalpha");
        app.mode = UiMode::Normal;

        run_keys(
            &mut app,
            &db,
            &[
                Key::Char('/'),
                Key::Char('a'),
                Key::Char('l'),
                Key::Char('p'),
                Key::Char('h'),
                Key::Char('a'),
                Key::Enter,
            ],
        );
        assert_eq!(app.search_query, "alpha");
        assert!(!app.search_matches.is_empty());
        assert_eq!((app.cursor_line, app.cursor_col), (0, 0));

        run_keys(&mut app, &db, &[Key::Char('n')]);
        assert_eq!((app.cursor_line, app.cursor_col), (1, 5));

        run_keys(&mut app, &db, &[Key::Char('N')]);
        assert_eq!((app.cursor_line, app.cursor_col), (0, 0));

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }
}
