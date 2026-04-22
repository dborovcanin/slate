use super::ansi::{
    contrast_fg_for_bg, draw_box_border, draw_row_at_styled, goto, pad_right, AnsiStyle,
};
use super::calc_cache::CalcCache;
use super::clipboard::{self, ClipboardWriteBackend};
use super::date_picker::{self, DatePickerAction, DatePickerView};
use super::folding::{self, FoldKind};
use super::folding_state::FoldingState;
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
use app_core::storage::{NoteAccessMode, NoteModules};
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
const CALC_VIEWPORT_ONLY_MIN_LINES: usize = 2_000;
const CALC_VIEWPORT_PREFETCH_MULTIPLIER: usize = 2;
const VARIABLE_AUTOCOMPLETE_MAX_SUGGESTIONS: usize = 3;
const COMMAND_COMPLETION_MAX_OPTIONS: usize = 8;
// Checkpoint every N lines for fence-state lookups in draw().
// Keeps the per-draw scan to at most INTERVAL line advances.
const FENCE_CHECKPOINT_INTERVAL: usize = 256;

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

fn trim_trailing_word(text: &mut String) {
    while text.chars().last().is_some_and(|c| !c.is_alphanumeric()) {
        text.pop();
    }
    while text.chars().last().is_some_and(|c| c.is_alphanumeric()) {
        text.pop();
    }
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
const MAX_COMMAND_HISTORY_ENTRIES: usize = 100;

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
    requires_password: bool,
    password: String,
}

#[derive(Debug, Clone)]
struct SwitcherOpenConfirm {
    note_id: String,
    note_title: String,
    password: String,
}

#[derive(Debug, Clone)]
struct VariableCompletionPrefix {
    from_col: usize,
    to_col: usize,
    query: String,
}

#[derive(Debug, Clone)]
struct VariableAutocompleteState {
    from_col: usize,
    to_col: usize,
    query: String,
    suggestions: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct VariableAutocompletePopupState {
    visible: bool,
    anchor_row: usize,
    anchor_col: usize,
    from_col: usize,
    to_col: usize,
    query: String,
    suggestions: Vec<String>,
    selected_index: usize,
    cursor_line: usize,
    cursor_col: usize,
}

#[derive(Debug, Clone, Default)]
struct CommandCompletionOption {
    token: String,
    has_more: bool,
}

#[derive(Debug, Clone, Default)]
struct CommandCompletionMenuState {
    visible: bool,
    prefix_tokens: Vec<String>,
    options: Vec<CommandCompletionOption>,
    selected_index: usize,
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
    switcher_open_confirm: Option<SwitcherOpenConfirm>,
    switcher_delete_confirm: Option<SwitcherDeleteConfirm>,
    dirty: bool,
    last_edit: Instant,
    status: String,
    command_input: String,
    command_completion: CommandCompletionMenuState,
    command_history: Vec<String>,
    command_history_index: Option<usize>,
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
    calc: CalcCache,
    calc_viewport_only: bool,
    calc_last_view_eval_range: Option<(usize, usize)>,
    reminder_ghosts: HashMap<usize, LineReminderGhost>, // 0-based line index
    reminders_dirty: bool,
    last_reminder_check: Instant,
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
    variable_autocomplete_min_chars: usize,
    variable_autocomplete_popup: VariableAutocompletePopupState,
    render_palette: render::RenderPalette,
    // Folding (real-line indexed, 0-based)
    folds: FoldingState,
    // Track which mode entered command bar from
    command_bar_from_normal: bool,
    // Clipboard watch
    clipboard_watch_enabled: bool,
    clipboard_watch_last_text: Option<String>,
    clipboard_watch_last_poll: Instant,
    // Undo/redo
    history: LineHistory,
    // Fence state checkpoints for draw(). Entry k = fence state BEFORE line
    // k * FENCE_CHECKPOINT_INTERVAL. fence_checkpoints_valid_through is the
    // highest index whose entry is current; all higher indices are stale.
    fence_checkpoints: Vec<(bool, Option<String>)>,
    fence_checkpoints_valid_through: usize,
    draw_buf: String,
}

impl TerminalApp {
    fn active_note_is_editable(&self) -> bool {
        self.active_note.access_mode == NoteAccessMode::None || self.active_note.is_unlocked
    }

    fn locked_note_status_message(&self) -> String {
        "note is locked; unlock first (:note unlock <password> or unlock-note <password>)"
            .to_string()
    }

    fn set_locked_note_status(&mut self) {
        self.status = self.locked_note_status_message();
    }

    fn is_locked_note_error(error: &str) -> bool {
        error.contains("unlock first") || error.contains("note is locked")
    }

    fn editor_key_may_edit_note(key: &Key) -> bool {
        matches!(
            key,
            Key::Ctrl('w')
                | Key::CtrlBackspace
                | Key::CtrlDelete
                | Key::Backspace
                | Key::Delete
                | Key::Enter
                | Key::Tab
                | Key::BackTab
                | Key::Paste(_)
                | Key::Char(_)
        )
    }

    fn vim_intent_mutates_document(intent: crate::editor_core::vim::VimIntent) -> bool {
        matches!(
            intent,
            crate::editor_core::vim::VimIntent::EnterInsert
                | crate::editor_core::vim::VimIntent::AppendInsert
                | crate::editor_core::vim::VimIntent::InsertLineStart
                | crate::editor_core::vim::VimIntent::AppendLineEnd
                | crate::editor_core::vim::VimIntent::OpenLineBelow
                | crate::editor_core::vim::VimIntent::OpenLineAbove
                | crate::editor_core::vim::VimIntent::DeleteLine
                | crate::editor_core::vim::VimIntent::DeleteToLineStart
                | crate::editor_core::vim::VimIntent::DeleteToLineEnd
                | crate::editor_core::vim::VimIntent::DeleteChar
                | crate::editor_core::vim::VimIntent::PasteAfter
                | crate::editor_core::vim::VimIntent::DeleteInsideWord
                | crate::editor_core::vim::VimIntent::DeleteAroundWord
                | crate::editor_core::vim::VimIntent::DeleteInsidePipe
                | crate::editor_core::vim::VimIntent::DeleteAroundPipe
                | crate::editor_core::vim::VimIntent::DeleteWordForward
                | crate::editor_core::vim::VimIntent::DeleteWordBackward
                | crate::editor_core::vim::VimIntent::Undo
                | crate::editor_core::vim::VimIntent::Redo
        )
    }

    fn require_startup_password_if_needed(&mut self) {
        if self.active_note.access_mode == NoteAccessMode::None || self.active_note.is_unlocked {
            return;
        }

        self.mode = UiMode::Switcher;
        self.switcher_query.clear();
        self.recompute_switcher_matches();

        let mut note_title = self.active_note.id.clone();
        if let Some((match_idx, switcher_idx)) = self
            .switcher_matches
            .iter()
            .enumerate()
            .find(|(_, idx)| self.switcher_items[**idx].id == self.active_note.id)
        {
            self.switcher_selected = match_idx;
            note_title = self.switcher_items[*switcher_idx].title.clone();
        } else {
            self.switcher_selected = 0;
        }

        self.switcher_open_confirm = Some(SwitcherOpenConfirm {
            note_id: self.active_note.id.clone(),
            note_title,
            password: String::new(),
        });
        self.switcher_delete_confirm = None;
        self.status = "password required to open protected note".to_string();
    }

    fn new_with_startup_metrics(
        db: &Db,
        opts: &TerminalOptions,
        vim_mode: bool,
        format_on_save: bool,
        markdown_autoformat: bool,
        checklist_auto_reorder: bool,
        _variables_enabled: bool,
        variable_autocomplete_min_chars: u8,
        render_palette: render::RenderPalette,
        date_format: String,
        date_time_format: String,
    ) -> Result<(Self, TerminalStartupMetrics), String> {
        let startup_begin = Instant::now();

        let note_begin = Instant::now();
        let mut active_note = select_note(db, opts, &crate::config::load_theme_config())?;
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
        let note_math_enabled = active_note.modules.math;
        let note_variables_enabled = active_note.modules.variables;
        let active_has_builtin_formula = note_math_enabled && initial_has_builtin_formula;
        let active_has_variable_assignment =
            note_math_enabled && note_variables_enabled && initial_has_variable_assignment;
        let calc_viewport_only = note_math_enabled
            && lines.len() >= CALC_VIEWPORT_ONLY_MIN_LINES
            && active_has_variable_assignment
            && !active_has_builtin_formula;
        let skip_initial_calc =
            calc_viewport_only || (!active_has_builtin_formula && !active_has_variable_assignment);
        let calc_begin = Instant::now();
        let calc_data = if skip_initial_calc {
            CalcData {
                line_results: vec![None; lines.len()],
                cell_results: vec![Vec::new(); lines.len()],
                variable_names: Vec::new(),
            }
        } else {
            compute_calc_data(&calc_engine, &lines, active_has_variable_assignment, None)
        };
        let loading_calc_engine = calc_begin.elapsed();

        let (prev_line_hashes, prev_line_has_assignment, prev_line_has_builtin_formula) =
            if skip_initial_calc {
                (Vec::new(), Vec::new(), Vec::new())
            } else {
                (
                    crate::editor_core::calc_plan::hash_lines(&lines),
                    lines
                        .iter()
                        .map(|line| {
                            crate::editor_core::calc_plan::contains_assignment_operator(line)
                        })
                        .collect::<Vec<_>>(),
                    lines
                        .iter()
                        .map(|line| {
                            crate::editor_core::calc_plan::contains_builtin_formula(
                                std::slice::from_ref(line),
                            )
                        })
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
            switcher_open_confirm: None,
            switcher_delete_confirm: None,
            dirty: false,
            last_edit: Instant::now(),
            status: initial_status,
            command_input: String::new(),
            command_completion: CommandCompletionMenuState::default(),
            command_history: Vec::new(),
            command_history_index: None,
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
            calc: CalcCache {
                engine: calc_engine,
                results: calc_data.line_results,
                cell_results: calc_data.cell_results,
                variable_names: calc_data.variable_names,
                prev_line_hashes,
                prev_line_has_assignment,
                prev_line_has_builtin_formula,
                stale: false,
                cached_has_builtin_formula: initial_has_builtin_formula,
                cached_has_variable_assignment: initial_has_variable_assignment,
            },
            calc_viewport_only,
            calc_last_view_eval_range: None,
            reminder_ghosts,
            reminders_dirty: false,
            last_reminder_check: Instant::now(),
            search_query: String::new(),
            search_matches: Vec::new(),
            search_current: 0,
            search_orig_line: 0,
            search_orig_col: 0,
            search_orig_scroll: 0,
            format_on_save,
            markdown_autoformat,
            checklist_auto_reorder,
            variable_autocomplete_min_chars: usize::from(
                variable_autocomplete_min_chars.clamp(1, 64),
            ),
            variable_autocomplete_popup: VariableAutocompletePopupState::default(),
            render_palette,
            folds: FoldingState::empty(line_has_fold_structure),
            command_bar_from_normal: false,
            clipboard_watch_enabled: false,
            clipboard_watch_last_text: None,
            clipboard_watch_last_poll: Instant::now(),
            history,
            fence_checkpoints: vec![(false, None)],
            fence_checkpoints_valid_through: 0,
            draw_buf: String::new(),
        };

        app.recompute_folding_from_cached_structure();
        app.adjust_cursor();
        app.adjust_scroll();
        app.require_startup_password_if_needed();
        if app.calc_viewport_only {
            let editor_height = app.editor_height();
            app.ensure_calc_for_viewport(editor_height, true);
        }

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
            if let Err(error) = self.save(db) {
                if !Self::is_locked_note_error(&error) {
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn maybe_autosave(&mut self, db: &Db) -> Result<(), String> {
        // Process any deferred fold recompute while the user is not typing.
        if self.folds.rescan_pending {
            self.folds.rescan_pending = false;
            self.recompute_folding();
        }
        if self.dirty && self.last_edit.elapsed() >= Duration::from_millis(AUTOSAVE_DEBOUNCE_MS) {
            match self.save(db) {
                Ok(()) => {
                    self.status = format!("autosaved {}", self.active_note.id);
                }
                Err(error) => {
                    if Self::is_locked_note_error(&error) {
                        self.set_locked_note_status();
                    } else {
                        return Err(error);
                    }
                }
            }
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
        if !self.active_note_is_editable() {
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
            self.folds.pending_prefix_until = None;
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
                self.save(db)?;
                self.status = format!("saved {}", self.active_note.id);
                return Ok(());
            }
            Key::Ctrl('n') => {
                self.save(db)?;
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
                if !self.move_variable_autocomplete_selection(-1) {
                    self.move_cursor_up(1);
                    moved_cursor = true;
                }
            }
            Key::ArrowDown => {
                if !self.move_variable_autocomplete_selection(1) {
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
                if self.apply_variable_autocomplete_popup_selection() {
                    should_autoformat = true;
                } else {
                    if !self.try_enter_rule() {
                        self.insert_newline();
                    }
                    refresh_variable_popup = true;
                    should_autoformat = true;
                }
            }
            Key::Tab => {
                if self.apply_variable_autocomplete_tab() {
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
                self.insert_paste(&text);
                // Pasted content should stay as-is; skip per-keystroke
                // autoformat pass that would otherwise scan the full document.
                should_autoformat = false;
                refresh_variable_popup = true;
            }
            Key::Char(ch) => {
                if ch == '|' && self.try_table_pipe_insert_column_rule() {
                    should_autoformat = false;
                    clamp_table_padding = false;
                } else {
                    self.insert_char(ch);
                    should_autoformat = true;
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
                if self.variable_autocomplete_popup.visible {
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
            } else if refresh_variable_popup {
                self.refresh_variable_autocomplete_popup();
            }
        } else {
            self.dismiss_variable_autocomplete_popup();
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

    fn line_col_lt(left_line: usize, left_col: usize, right_line: usize, right_col: usize) -> bool {
        left_line < right_line || (left_line == right_line && left_col < right_col)
    }

    fn slice_cols_range(
        &self,
        from_line: usize,
        from_col: usize,
        to_line: usize,
        to_col: usize,
    ) -> Option<String> {
        if !Self::line_col_lt(from_line, from_col, to_line, to_col) {
            return None;
        }
        if from_line == to_line {
            let line = self.lines.get(from_line)?;
            let start = byte_index(line, from_col);
            let end = byte_index(line, to_col);
            if start >= end || end > line.len() {
                return None;
            }
            return Some(line[start..end].to_string());
        }
        let text = join_lines(&self.lines);
        let start = self.byte_offset_for_line_col(from_line, from_col);
        let end = self.byte_offset_for_line_col(to_line, to_col);
        if start >= end || end > text.len() {
            return None;
        }
        Some(text[start..end].to_string())
    }

    fn delete_cols_range(
        &mut self,
        from_line: usize,
        from_col: usize,
        to_line: usize,
        to_col: usize,
    ) -> Option<String> {
        if !Self::line_col_lt(from_line, from_col, to_line, to_col) {
            return None;
        }
        if from_line == to_line {
            return self.delete_current_line_cols(from_col, to_col);
        }
        let mut text = join_lines(&self.lines);
        let start = self.byte_offset_for_line_col(from_line, from_col);
        let end = self.byte_offset_for_line_col(to_line, to_col);
        if start >= end || end > text.len() {
            return None;
        }
        let deleted = text[start..end].to_string();
        text.replace_range(start..end, "");
        self.lines = split_lines(&text);
        self.cursor_line = from_line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = from_col;
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
            if !self.active_note_is_editable() && Self::vim_intent_mutates_document(action.intent) {
                self.set_locked_note_status();
                continue;
            }
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
                crate::editor_core::vim::VimIntent::DeleteWordForward => {
                    let mut deleted = Vec::new();
                    for _ in 0..count {
                        let from_line = self.cursor_line;
                        let from_col = self.cursor_col;
                        self.move_cursor_right_word();
                        let to_line = self.cursor_line;
                        let to_col = self.cursor_col;
                        if Self::line_col_lt(from_line, from_col, to_line, to_col) {
                            if let Some(chunk) =
                                self.delete_cols_range(from_line, from_col, to_line, to_col)
                            {
                                deleted.push(chunk);
                            }
                        } else {
                            self.cursor_line = from_line;
                            self.cursor_col = from_col;
                            break;
                        }
                    }
                    if !deleted.is_empty() {
                        self.set_clipboard_lines(deleted);
                        self.status = self.with_clipboard_status("deleted word forward");
                        self.mark_edited();
                        self.adjust_cursor();
                    }
                }
                crate::editor_core::vim::VimIntent::DeleteWordBackward => {
                    let mut deleted = Vec::new();
                    for _ in 0..count {
                        let from_line = self.cursor_line;
                        let from_col = self.cursor_col;
                        self.move_cursor_left_word();
                        let to_line = self.cursor_line;
                        let to_col = self.cursor_col;
                        if Self::line_col_lt(to_line, to_col, from_line, from_col) {
                            if let Some(chunk) =
                                self.delete_cols_range(to_line, to_col, from_line, from_col)
                            {
                                deleted.push(chunk);
                            }
                        } else {
                            self.cursor_line = from_line;
                            self.cursor_col = from_col;
                            break;
                        }
                    }
                    if !deleted.is_empty() {
                        deleted.reverse();
                        self.set_clipboard_lines(deleted);
                        self.status = self.with_clipboard_status("deleted word backward");
                        self.mark_edited();
                        self.adjust_cursor();
                    }
                }
                crate::editor_core::vim::VimIntent::YankWordForward => {
                    let mut yanked = Vec::new();
                    let origin_line = self.cursor_line;
                    let origin_col = self.cursor_col;
                    for _ in 0..count {
                        let from_line = self.cursor_line;
                        let from_col = self.cursor_col;
                        self.move_cursor_right_word();
                        let to_line = self.cursor_line;
                        let to_col = self.cursor_col;
                        if Self::line_col_lt(from_line, from_col, to_line, to_col) {
                            if let Some(chunk) =
                                self.slice_cols_range(from_line, from_col, to_line, to_col)
                            {
                                yanked.push(chunk);
                            }
                        } else {
                            self.cursor_line = from_line;
                            self.cursor_col = from_col;
                            break;
                        }
                    }
                    self.cursor_line = origin_line;
                    self.cursor_col = origin_col;
                    if !yanked.is_empty() {
                        self.set_clipboard_lines(yanked);
                        self.status = self.with_clipboard_status("yanked word forward");
                    }
                }
                crate::editor_core::vim::VimIntent::YankWordBackward => {
                    let mut yanked = Vec::new();
                    let origin_line = self.cursor_line;
                    let origin_col = self.cursor_col;
                    for _ in 0..count {
                        let from_line = self.cursor_line;
                        let from_col = self.cursor_col;
                        self.move_cursor_left_word();
                        let to_line = self.cursor_line;
                        let to_col = self.cursor_col;
                        if Self::line_col_lt(to_line, to_col, from_line, from_col) {
                            if let Some(chunk) =
                                self.slice_cols_range(to_line, to_col, from_line, from_col)
                            {
                                yanked.push(chunk);
                            }
                        } else {
                            self.cursor_line = from_line;
                            self.cursor_col = from_col;
                            break;
                        }
                    }
                    self.cursor_line = origin_line;
                    self.cursor_col = origin_col;
                    if !yanked.is_empty() {
                        yanked.reverse();
                        self.set_clipboard_lines(yanked);
                        self.status = self.with_clipboard_status("yanked word backward");
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
                    self.dismiss_command_completion_menu();
                    self.command_history_index = None;
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
            let ctx = self.build_context();
            let options = crate::editor_core::text_rules::TextRuleOptions {
                markdown_autoformat: self.markdown_autoformat_enabled(),
                checklist_auto_reorder: self.checklist_auto_reorder_enabled(),
            };
            if let Some(op) = crate::editor_core::text_rules::run_doc_change_rules(&ctx, options) {
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
                self.vim_state = crate::editor_core::vim::VimState::default();
                self.selection_anchor = None;
                self.command_selection = None;
                self.status = "-- NORMAL --".to_string();
            }
            Key::Ctrl('e') | Key::Char(':') => {
                self.command_selection_linewise = self.mode == UiMode::VisualLine;
                self.command_selection = self.capture_visual_command_selection();
                self.vim_state = crate::editor_core::vim::VimState::default();
                self.command_input.clear();
                self.dismiss_command_completion_menu();
                self.command_history_index = None;
                self.command_bar_from_normal = true;
                self.mode = UiMode::CommandBar;
                self.status = ":".to_string();
            }
            Key::Char('y') | Key::Char('d') | Key::Char('x') => {
                let is_delete = matches!(key, Key::Char('d') | Key::Char('x'));
                if is_delete && !self.active_note_is_editable() {
                    self.set_locked_note_status();
                    return Ok(());
                }
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
                self.vim_state = crate::editor_core::vim::VimState::default();
                self.selection_anchor = None;
                self.command_selection = None;
                self.status = if is_delete {
                    self.with_clipboard_status("-- NORMAL --")
                } else {
                    self.with_clipboard_status("-- NORMAL -- (yanked)")
                };
                if is_delete {
                    self.mark_edited();
                }
            }
            _ => {
                let Some(vim_key) = Self::map_vim_key(&key) else {
                    self.adjust_cursor();
                    self.adjust_scroll();
                    return Ok(());
                };

                let context = crate::editor_core::vim::VimContext {
                    has_search_matches: !self.search_matches.is_empty(),
                    line_count: self.lines.len(),
                };
                let step = crate::editor_core::vim::step(&self.vim_state, vim_key, &context);
                self.vim_state = step.state;

                if step.handled {
                    self.apply_vim_actions(&step.actions);
                }
            }
        }

        self.adjust_cursor();
        self.adjust_scroll();
        Ok(())
    }

    fn handle_switcher_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
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
                        });
                    } else {
                        self.open_note_from_switcher(db, item.id.as_str(), None)?;
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

    fn handle_switcher_open_confirm_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
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

    fn open_note_from_switcher(
        &mut self,
        db: &Db,
        note_id: &str,
        password: Option<&str>,
    ) -> Result<(), String> {
        self.save(db)?;
        let note = if let Some(password) = password {
            db.unlock_note(note_id, password)?
        } else {
            let Some(note) = db.get_note(note_id)? else {
                self.status = format!("note missing {}", note_id);
                return Ok(());
            };
            note
        };
        self.set_active_note(db, note)?;
        self.close_switcher();
        self.mode = UiMode::Normal;
        self.vim_state.mode = crate::editor_core::vim::VimMode::Normal;
        self.selection_anchor = None;
        self.command_selection = None;
        self.status = "-- NORMAL --".to_string();
        Ok(())
    }

    fn request_switcher_delete_confirmation(&mut self, db: &Db) {
        if let Some(idx) = self.switcher_matches.get(self.switcher_selected).copied() {
            let item = &self.switcher_items[idx];
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

    fn delete_note_from_switcher(
        &mut self,
        db: &Db,
        note_id: &str,
        note_title: &str,
        password: Option<&str>,
    ) -> Result<(), String> {
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

    fn command_mode(&self) -> crate::editor_core::types::CommandMode {
        if self.command_bar_from_normal {
            crate::editor_core::types::CommandMode::Vim
        } else {
            crate::editor_core::types::CommandMode::Editor
        }
    }

    fn remember_command_in_history(&mut self, command: &str) {
        crate::editor_core::command_history::remember_command(
            &mut self.command_history,
            command,
            MAX_COMMAND_HISTORY_ENTRIES,
        );
        self.command_history_index = None;
    }

    fn dismiss_command_completion_menu(&mut self) {
        self.command_completion = CommandCompletionMenuState::default();
    }

    fn build_command_completion_menu(&self) -> Option<CommandCompletionMenuState> {
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

    fn open_command_completion_menu(&mut self) -> bool {
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

    fn move_command_completion_selection(&mut self, delta: isize) -> bool {
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

    fn apply_command_completion_selection(&mut self) -> bool {
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

    fn cycle_command_history_prev(&mut self) {
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

    fn cycle_command_history_next(&mut self) {
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

    fn handle_command_bar_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
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

    fn update_command_status(&mut self) {
        let hint = if self.command_completion.visible {
            self.command_completion
                .options
                .iter()
                .map(|option| option.token.clone())
                .collect::<Vec<_>>()
                .join("  ")
        } else {
            self.build_command_completion_menu()
                .map(|menu| {
                    menu.options
                        .into_iter()
                        .map(|option| option.token)
                        .collect::<Vec<_>>()
                        .join("  ")
                })
                .unwrap_or_default()
        };
        if hint.is_empty() {
            self.status = format!(":{}", self.command_input);
        } else {
            self.status = format!(":{}  [{}]", self.command_input, hint);
        }
    }

    fn draw_status_segment(
        buf: &mut String,
        row: usize,
        col: &mut usize,
        cols: usize,
        text: &str,
        style: AnsiStyle,
    ) {
        if *col > cols || text.is_empty() {
            return;
        }
        let remaining = cols.saturating_sub(*col).saturating_add(1);
        if remaining == 0 {
            return;
        }
        let clipped: String = text.chars().take(remaining).collect();
        if clipped.is_empty() {
            return;
        }
        buf.push_str(&goto(row, *col));
        style.write_to(buf);
        buf.push_str(&clipped);
        buf.push_str(render::RESET);
        *col += clipped.chars().count();
    }

    fn draw_command_completion_status_row(
        &self,
        buf: &mut String,
        row: usize,
        cols: usize,
        status_bg: u8,
    ) -> bool {
        if self.mode != UiMode::CommandBar || !self.command_completion.visible {
            return false;
        }
        if self.command_completion.options.is_empty() {
            return false;
        }
        let selected_idx = self
            .command_completion
            .selected_index
            .min(self.command_completion.options.len().saturating_sub(1));

        let base_fg = contrast_fg_for_bg(status_bg);
        let base_style = AnsiStyle {
            fg: Some(base_fg),
            bg: Some(status_bg),
            ..Default::default()
        };
        let selected_style = AnsiStyle {
            fg: Some(status_bg),
            bg: Some(base_fg),
            bold: true,
            ..Default::default()
        };

        draw_row_at_styled(buf, row, 1, cols, "", base_style);

        let mut col = 1usize;
        Self::draw_status_segment(
            buf,
            row,
            &mut col,
            cols,
            &format!(":{}  [", self.command_input),
            base_style,
        );
        for (idx, option) in self.command_completion.options.iter().enumerate() {
            if idx > 0 {
                Self::draw_status_segment(buf, row, &mut col, cols, "  ", base_style);
            }
            let style = if idx == selected_idx {
                selected_style
            } else {
                base_style
            };
            Self::draw_status_segment(buf, row, &mut col, cols, &option.token, style);
        }
        Self::draw_status_segment(buf, row, &mut col, cols, "]", base_style);
        true
    }

    fn format_active_note_modules_status(&self) -> String {
        let modules = self.active_note.modules;
        format!(
            "modules math={} table={} variables={} style={}",
            if modules.math { "on" } else { "off" },
            if modules.table { "on" } else { "off" },
            if modules.variables { "on" } else { "off" },
            if modules.style { "on" } else { "off" },
        )
    }

    fn handle_terminal_module_command(
        &mut self,
        db: &Db,
        command_id: crate::editor_core::command_catalog::CommandId,
    ) -> bool {
        use crate::editor_core::command_catalog::CommandId;

        let mut next_modules = self.active_note.modules;
        match command_id {
            CommandId::ModuleStatus => {
                self.status = self.format_active_note_modules_status();
                return true;
            }
            CommandId::ModuleOnMath => next_modules.math = true,
            CommandId::ModuleOffMath => next_modules.math = false,
            CommandId::ModuleToggleMath => next_modules.math = !next_modules.math,
            CommandId::ModuleOnTable => next_modules.table = true,
            CommandId::ModuleOffTable => next_modules.table = false,
            CommandId::ModuleToggleTable => next_modules.table = !next_modules.table,
            CommandId::ModuleOnVariables => next_modules.variables = true,
            CommandId::ModuleOffVariables => next_modules.variables = false,
            CommandId::ModuleToggleVariables => next_modules.variables = !next_modules.variables,
            CommandId::ModuleOnStyle => next_modules.style = true,
            CommandId::ModuleOffStyle => next_modules.style = false,
            CommandId::ModuleToggleStyle => next_modules.style = !next_modules.style,
            _ => return false,
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
                            self.recompute_calc_full();
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
                self.status = self.format_active_note_modules_status();
            }
            Err(error) => {
                self.status = format!("module update failed: {error}");
            }
        }
        true
    }

    fn execute_terminal_command(&mut self, db: &Db, cmd: &str) {
        if cmd == "q!" || cmd == "q" {
            self.force_quit = cmd == "q!";
            self.quit = true;
            return;
        }

        if let Some(parsed) = crate::editor_core::command_catalog::parse_note_security_command(cmd)
        {
            let action = parsed.action;
            let action_label = action.as_str();
            let password = parsed.password;
            if password.trim().is_empty() {
                self.status = format!("usage: note {action_label} <password>");
                return;
            }
            if self.dirty {
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

        if let Some(command) =
            crate::editor_core::command_catalog::resolve_command(self.command_mode(), cmd)
        {
            if self.handle_terminal_module_command(db, command.id) {
                return;
            }
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

    fn build_context(&self) -> crate::editor_core::context::ResolvedContext {
        crate::editor_core::context::ResolvedContext::new(self.build_snapshot())
    }

    fn open_date_picker(&mut self, action: DatePickerAction, require_time: bool) {
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
        self.dismiss_variable_autocomplete_popup();
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

    fn close_switcher(&mut self) {
        self.dismiss_variable_autocomplete_popup();
        self.mode = UiMode::Editor;
        self.switcher_query.clear();
        self.switcher_matches.clear();
        self.switcher_selected = 0;
        self.switcher_open_confirm = None;
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
            self.calc.prev_line_hashes.clear();
            self.calc.prev_line_has_assignment.clear();
            self.calc.prev_line_has_builtin_formula.clear();
            self.calc.stale = false;
        } else if self.should_defer_calc_recompute() {
            self.calc.results = vec![None; self.lines.len()];
            self.calc.cell_results = vec![Vec::new(); self.lines.len()];
            self.calc.variable_names.clear();
            self.calc.prev_line_hashes.clear();
            self.calc.prev_line_has_assignment.clear();
            self.calc.prev_line_has_builtin_formula.clear();
            self.calc.stale = true;
        } else {
            self.recompute_calc_full();
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
        self.calc.cached_has_builtin_formula =
            crate::editor_core::calc_plan::contains_builtin_formula(&self.lines);
        self.calc.cached_has_variable_assignment =
            crate::editor_core::calc_plan::contains_variable_assignment(&self.lines);
    }

    fn update_calc_flags_incremental(&mut self) {
        if self.lines.len() != self.calc.results.len() {
            // Line count changed (Enter, delete-at-boundary).
            // Flags can only go false → true here, never true → false.
            // On delete: the removed line may have been the only one with the
            // syntax, but accepting stale-true is safe — it just means we run
            // calc when not needed, which is correct.
            // On insert: check only the two affected lines (cursor and cursor-1).
            if self.lines.len() > self.calc.results.len() {
                let cl = self.cursor_line.min(self.lines.len().saturating_sub(1));
                for i in cl.saturating_sub(1)..=cl {
                    if let Some(text) = self.lines.get(i) {
                        if !self.calc.cached_has_variable_assignment
                            && crate::editor_core::calc_plan::contains_variable_assignment(
                                std::slice::from_ref(text),
                            )
                        {
                            self.calc.cached_has_variable_assignment = true;
                        }
                        if !self.calc.cached_has_builtin_formula
                            && crate::editor_core::calc_plan::contains_builtin_formula(
                                std::slice::from_ref(text),
                            )
                        {
                            self.calc.cached_has_builtin_formula = true;
                        }
                    }
                }
            }
            // Delete: accept stale-true; flags reset only via rescan_calc_flags
            // (called on note switch and explicit rescans).
            return;
        }
        // Same-line edit: check only the cursor line for new signals.
        let line = self.cursor_line;
        if !self.calc.cached_has_variable_assignment {
            if let Some(text) = self.lines.get(line) {
                if crate::editor_core::calc_plan::contains_variable_assignment(
                    std::slice::from_ref(text),
                ) {
                    self.calc.cached_has_variable_assignment = true;
                }
            }
        }
        if !self.calc.cached_has_builtin_formula {
            if let Some(text) = self.lines.get(line) {
                if crate::editor_core::calc_plan::contains_builtin_formula(std::slice::from_ref(
                    text,
                )) {
                    self.calc.cached_has_builtin_formula = true;
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

    fn note_math_module_enabled(&self) -> bool {
        self.active_note.modules.math
    }

    fn note_table_module_enabled(&self) -> bool {
        self.active_note.modules.table
    }

    fn note_variables_module_enabled(&self) -> bool {
        self.active_note.modules.variables
    }

    fn note_style_module_enabled(&self) -> bool {
        self.active_note.modules.style
    }

    fn markdown_autoformat_enabled(&self) -> bool {
        self.markdown_autoformat && self.note_style_module_enabled()
    }

    fn checklist_auto_reorder_enabled(&self) -> bool {
        self.checklist_auto_reorder && self.note_style_module_enabled()
    }

    fn active_has_variable_assignments(&self) -> bool {
        self.note_math_module_enabled()
            && self.note_variables_module_enabled()
            && self.calc.cached_has_variable_assignment
    }

    fn calc_variables_enabled(&self) -> bool {
        self.active_has_variable_assignments()
    }

    fn should_defer_calc_recompute(&self) -> bool {
        self.lines.len() >= LARGE_DOC_CALC_DEFER_LINES
            && !self.calc.cached_has_builtin_formula
            && !self.active_has_variable_assignments()
    }

    fn can_skip_calc_recompute(&self) -> bool {
        // If neither builtin formulas nor variable assignments exist anywhere
        // in the doc, `compute_calc_data` would produce all-None results for
        // every line — matching the current state. Safe to skip regardless of
        // doc size, which is the biggest input-latency win for notes that
        // don't use calc at all.
        !self.calc.cached_has_builtin_formula
            && !self.active_has_variable_assignments()
            && !self.calc.stale
    }

    fn clear_calc_cache(&mut self) {
        self.calc.results = vec![None; self.lines.len()];
        self.calc.cell_results = vec![Vec::new(); self.lines.len()];
        self.calc.variable_names.clear();
        self.calc.prev_line_hashes.clear();
        self.calc.prev_line_has_assignment.clear();
        self.calc.prev_line_has_builtin_formula.clear();
        self.calc.stale = false;
        self.calc_last_view_eval_range = None;
    }

    fn defer_calc_state_after_edit(&mut self) {
        // Large docs without explicit calc syntax should not recompute calc
        // state on every keystroke.
        // Clear the full cache so same-line-count multi-line edits cannot
        // leave stale calc ghosts on non-cursor lines.
        if self.calc.results.len() != self.lines.len() {
            self.calc.results = vec![None; self.lines.len()];
        } else {
            self.calc.results.fill(None);
        }
        if self.calc.cell_results.len() != self.lines.len() {
            self.calc.cell_results = vec![Vec::new(); self.lines.len()];
        } else {
            for row in &mut self.calc.cell_results {
                row.clear();
            }
        }
        self.calc.variable_names.clear();
        self.calc.stale = true;
    }

    fn recompute_folding_if_needed(&mut self) {
        // Process any pending deferred recompute first.
        if self.folds.rescan_pending {
            self.folds.rescan_pending = false;
            self.recompute_folding();
            return;
        }

        let line_count_changed = self.lines.len() != self.folds.line_has_structure.len();

        if line_count_changed {
            if self.folds.collapsed_starts.is_empty() {
                // No active folds — skip the expensive analyze_lines pass.
                // Update line_has_fold_structure incrementally (Vec::insert/remove)
                // instead of rebuilding 106k entries, then rebuild the sequential
                // view map and defer the full fold analysis to the next idle tick.
                let cl = self.cursor_line.min(self.lines.len().saturating_sub(1));
                let next_len = self.lines.len();
                let prev_len = self.folds.line_has_structure.len();
                if next_len == prev_len + 1 {
                    // One line inserted. Update the upper half of the split, insert new entry.
                    if cl > 0 {
                        if let Some(flag) = self.folds.line_has_structure.get_mut(cl - 1) {
                            *flag = self
                                .lines
                                .get(cl - 1)
                                .map(|l| Self::line_has_fold_structure(l))
                                .unwrap_or(false);
                        }
                    }
                    let new_flag = self
                        .lines
                        .get(cl)
                        .map(|l| Self::line_has_fold_structure(l))
                        .unwrap_or(false);
                    self.folds
                        .line_has_structure
                        .insert(cl.min(prev_len), new_flag);
                } else if next_len + 1 == prev_len {
                    // One line deleted. Remove the entry; update the merged line.
                    if cl < prev_len {
                        self.folds.line_has_structure.remove(cl);
                    }
                    let update_at = cl.min(next_len.saturating_sub(1));
                    if let Some(flag) = self.folds.line_has_structure.get_mut(update_at) {
                        *flag = self
                            .lines
                            .get(update_at)
                            .map(|l| Self::line_has_fold_structure(l))
                            .unwrap_or(false);
                    }
                } else {
                    // Bulk change (paste, format, etc.): rebuild entirely.
                    self.folds.line_has_structure = self
                        .lines
                        .iter()
                        .map(|l| Self::line_has_fold_structure(l))
                        .collect();
                }
                self.folds.ranges.clear();
                self.folds.range_by_start = vec![None; self.lines.len()];
                self.rebuild_fold_view_map();
                self.folds.rescan_pending = true;
            } else {
                // Active collapsed folds present: must recompute for correctness.
                self.recompute_folding();
            }
            return;
        }

        // Same-line edit: check whether the current line touches fold structure.
        let cl = self.cursor_line.min(self.lines.len().saturating_sub(1));
        let current_text = self.lines.get(cl).map(|s| s.as_str()).unwrap_or("");
        let next_flag = Self::line_has_fold_structure(current_text);
        let prev_flag = self
            .folds
            .line_has_structure
            .get(cl)
            .copied()
            .unwrap_or(false);

        if next_flag != prev_flag {
            if let Some(flag) = self.folds.line_has_structure.get_mut(cl) {
                *flag = next_flag;
            }
        }

        if next_flag || prev_flag {
            // Defer: fold analysis is O(N) and doesn't need to block typing.
            // The idle tick (100 ms with no keypress) will run recompute_folding.
            self.folds.rescan_pending = true;
        }
    }

    fn recompute_folding(&mut self) {
        self.folds.line_has_structure = self
            .lines
            .iter()
            .map(|line| Self::line_has_fold_structure(line))
            .collect();
        self.recompute_folding_from_cached_structure();
    }

    fn recompute_folding_from_cached_structure(&mut self) {
        self.folds.rescan_pending = false;
        self.folds.ranges = folding::build_fold_ranges(&self.lines);
        self.folds.range_by_start = vec![None; self.lines.len()];
        for range in &self.folds.ranges {
            if range.start_line < self.folds.range_by_start.len() {
                self.folds.range_by_start[range.start_line] = Some(*range);
            }
        }
        self.folds.collapsed_starts.retain(|line| {
            self.folds
                .range_by_start
                .get(*line)
                .is_some_and(|entry| entry.is_some())
        });
        self.rebuild_fold_view_map();
    }

    fn rebuild_fold_view_map(&mut self) {
        let line_count = self.lines.len();
        self.folds.visible_to_real.clear();
        self.folds.visible_to_real.reserve(line_count);
        self.folds.real_to_visible = vec![0; line_count];
        self.folds.hidden_owner = vec![None; line_count];
        self.folds.placeholder_hidden_lines = vec![None; line_count];

        if line_count == 0 {
            return;
        }

        let mut collapsed_ranges = self
            .folds
            .collapsed_starts
            .iter()
            .filter_map(|start| {
                self.folds
                    .range_by_start
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
            let visible_idx = self.folds.visible_to_real.len();
            self.folds.visible_to_real.push(real_line);
            self.folds.real_to_visible[real_line] = visible_idx;

            let collapse_here = effective
                .get(effective_idx)
                .copied()
                .filter(|range| range.start_line == real_line);
            if let Some(range) = collapse_here {
                let hidden_end = range.end_line.min(line_count.saturating_sub(1));
                if hidden_end > real_line {
                    self.folds.placeholder_hidden_lines[real_line] =
                        Some(hidden_end.saturating_sub(real_line));
                    for hidden_line in (real_line + 1)..=hidden_end {
                        self.folds.hidden_owner[hidden_line] = Some(real_line);
                        self.folds.real_to_visible[hidden_line] = visible_idx;
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

        if self.folds.visible_to_real.is_empty() {
            self.folds.visible_to_real.push(0);
        }
    }

    // ---- Fence-state checkpoint helpers ----

    // Returns the code-fence parse state that applies BEFORE line `target_line`
    // (0-based). Uses a sparse checkpoint array so the worst-case scan is at
    // most FENCE_CHECKPOINT_INTERVAL line advances regardless of doc size.
    fn fence_state_before_line(&mut self, target_line: usize) -> (bool, Option<String>) {
        if target_line == 0 {
            return (false, None);
        }
        let target = target_line.min(self.lines.len());
        let target_ck = target / FENCE_CHECKPOINT_INTERVAL;
        let start_ck = target_ck.min(self.fence_checkpoints_valid_through);
        let start_line = start_ck * FENCE_CHECKPOINT_INTERVAL;

        let (mut in_code_block, mut code_fence_lang) = if start_ck == 0 {
            (false, None)
        } else {
            self.fence_checkpoints
                .get(start_ck)
                .cloned()
                .unwrap_or((false, None))
        };

        let mut line_idx = start_line;
        while line_idx < target {
            // At each new checkpoint boundary, cache the current state.
            if line_idx > 0 && line_idx % FENCE_CHECKPOINT_INTERVAL == 0 {
                let ck = line_idx / FENCE_CHECKPOINT_INTERVAL;
                if ck > self.fence_checkpoints_valid_through {
                    while self.fence_checkpoints.len() <= ck {
                        self.fence_checkpoints.push((false, None));
                    }
                    self.fence_checkpoints[ck] = (in_code_block, code_fence_lang.clone());
                    self.fence_checkpoints_valid_through = ck;
                }
            }
            if let Some(line_text) = self.lines.get(line_idx) {
                let mut state = crate::editor_core::markdown_tokens::FenceState {
                    in_code_block,
                    code_fence_lang,
                };
                crate::editor_core::markdown_tokens::advance_fence_state(&mut state, line_text);
                in_code_block = state.in_code_block;
                code_fence_lang = state.code_fence_lang;
            }
            line_idx += 1;
        }
        (in_code_block, code_fence_lang)
    }

    // Invalidate all fence checkpoints that depend on content at or after
    // `line_idx`. Called whenever lines at or before a checkpoint boundary change.
    fn invalidate_fence_checkpoints_from_line(&mut self, line_idx: usize) {
        let keep_through = line_idx / FENCE_CHECKPOINT_INTERVAL;
        if self.fence_checkpoints_valid_through > keep_through {
            self.fence_checkpoints_valid_through = keep_through;
        }
    }

    fn visible_line_count(&self) -> usize {
        self.folds.visible_to_real.len().max(1)
    }

    fn current_virtual_line(&self) -> usize {
        self.folds
            .real_to_visible
            .get(self.cursor_line)
            .copied()
            .unwrap_or(0)
    }

    fn real_line_for_virtual(&self, virtual_line: usize) -> Option<usize> {
        self.folds.visible_to_real.get(virtual_line).copied()
    }

    fn fold_hidden_owner_for_line(&self, line: usize) -> Option<usize> {
        self.folds.hidden_owner.get(line).and_then(|owner| *owner)
    }

    fn fold_start_for_line(&self, line: usize) -> Option<usize> {
        if let Some(owner) = self.fold_hidden_owner_for_line(line) {
            return Some(owner);
        }
        if self
            .folds
            .range_by_start
            .get(line)
            .is_some_and(|entry| entry.is_some())
        {
            return Some(line);
        }

        let mut best_start = None;
        let mut best_span = usize::MAX;
        for range in &self.folds.ranges {
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
        let next_collapsed = !self.folds.collapsed_starts.contains(&start_line);
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
            .folds
            .range_by_start
            .get(start_line)
            .and_then(|entry| *entry)
        else {
            self.status = "fold: no foldable block at cursor".to_string();
            return false;
        };

        let was_collapsed = self.folds.collapsed_starts.contains(&start_line);
        if was_collapsed == collapsed {
            self.status = if collapsed {
                "fold: already folded".to_string()
            } else {
                "fold: already unfolded".to_string()
            };
            return false;
        }

        let action = if collapsed {
            self.folds.collapsed_starts.insert(start_line);
            "folded"
        } else {
            self.folds.collapsed_starts.remove(&start_line);
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

    fn mark_edited_from_line(&mut self, changed_from_line: usize) {
        let coalesce_undo = self.last_edit.elapsed() < Duration::from_millis(UNDO_DEBOUNCE_MS);
        self.dirty = true;
        if !self.reminder_ghosts.is_empty() {
            self.reminders_dirty = true;
        }
        let clamped_changed_line = if self.lines.is_empty() {
            0
        } else {
            changed_from_line.min(self.lines.len().saturating_sub(1))
        };
        self.invalidate_fence_checkpoints_from_line(clamped_changed_line);
        self.update_calc_flags_incremental();
        self.recompute_folding_if_needed();
        if self.can_skip_calc_recompute() {
            // No calc syntax anywhere in the doc and this edit didn't add any —
            // calc_results are already correct (all None). Skip the scan.
            // prev_line_hashes may drift from `lines` until the next real
            // recompute, but the planner falls back to full eval safely when
            // the diff looks large, so correctness holds.
        } else if self.should_defer_calc_recompute() {
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

    fn mark_edited(&mut self) {
        self.mark_edited_from_line(self.cursor_line);
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
            // Full lines replacement: invalidate all caches.
            self.fence_checkpoints.truncate(1);
            self.fence_checkpoints_valid_through = 0;
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
            self.fence_checkpoints.truncate(1);
            self.fence_checkpoints_valid_through = 0;
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
        if !self.note_math_module_enabled() {
            self.clear_calc_cache();
            return;
        }
        let calc_variables_enabled = self.calc_variables_enabled();
        if self.calc.stale {
            let calc_data =
                compute_calc_data(&self.calc.engine, &self.lines, calc_variables_enabled, None);
            self.calc.prev_line_hashes = crate::editor_core::calc_plan::hash_lines(&self.lines);
            self.calc.prev_line_has_assignment = self
                .lines
                .iter()
                .map(|line| crate::editor_core::calc_plan::contains_assignment_operator(line))
                .collect();
            self.calc.prev_line_has_builtin_formula = self
                .lines
                .iter()
                .map(|line| {
                    crate::editor_core::calc_plan::contains_builtin_formula(std::slice::from_ref(
                        line,
                    ))
                })
                .collect();
            self.calc.results = calc_data.line_results;
            self.calc.cell_results = calc_data.cell_results;
            self.calc.variable_names = calc_data.variable_names;
            self.calc.stale = false;
            return;
        }

        let next_hashes = crate::editor_core::calc_plan::hash_lines(&self.lines);
        let plan = crate::editor_core::calc_plan::plan_incremental_calc_from_hashes(
            &self.calc.prev_line_hashes,
            &self.calc.results,
            &self.lines,
            &next_hashes,
        );
        let has_prev = !self.calc.prev_line_hashes.is_empty();

        // Only scan the changed region for variable assignments and builtin
        // formulas (not all lines). Partial eval is safe as long as the edit
        // doesn't touch a formula/assignment — whole-doc presence of formulas
        // elsewhere doesn't force recomputation of unchanged lines.
        let suffix_len = self.lines.len().saturating_sub(plan.eval_to);
        let prev_changed_from = plan.eval_from.min(self.calc.prev_line_hashes.len());
        let prev_changed_to = self
            .calc
            .prev_line_hashes
            .len()
            .saturating_sub(suffix_len)
            .max(prev_changed_from);
        let prev_changed_had_assignment = self
            .calc
            .prev_line_has_assignment
            .get(prev_changed_from..prev_changed_to)
            .map(|slice| slice.iter().any(|&flag| flag))
            .unwrap_or(false);
        let prev_changed_had_builtin_formula = self
            .calc
            .prev_line_has_builtin_formula
            .get(prev_changed_from..prev_changed_to)
            .map(|slice| slice.iter().any(|&flag| flag))
            .unwrap_or(false);
        let touches_any_assignment = calc_variables_enabled
            && (crate::editor_core::calc_plan::contains_variable_assignment(&plan.eval_lines)
                || prev_changed_had_assignment);
        let touches_builtin_formula =
            crate::editor_core::calc_plan::contains_builtin_formula(&plan.eval_lines)
                || prev_changed_had_builtin_formula;
        let can_use_partial = has_prev && !touches_any_assignment && !touches_builtin_formula;

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
                    if let Some(cached) = self.calc.cell_results.get(entry.line_idx) {
                        *slot = cached.clone();
                    }
                }
            }

            if plan.eval_from < plan.eval_to {
                let calc_data = compute_calc_data(
                    &self.calc.engine,
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
                (
                    merged_results,
                    merged_cells,
                    self.calc.variable_names.clone(),
                )
            }
        } else {
            let calc_data =
                compute_calc_data(&self.calc.engine, &self.lines, calc_variables_enabled, None);
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
        let aligned = self.calc.prev_line_hashes.len() == self.lines.len()
            && self.calc.results.len() == self.lines.len();

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
                if self.calc.prev_line_hashes[i] != final_hashes[i] {
                    continue;
                }
                if self.calc.results[i].is_some() {
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

        self.calc.prev_line_hashes = final_hashes;
        self.calc.prev_line_has_assignment = self
            .lines
            .iter()
            .map(|line| crate::editor_core::calc_plan::contains_assignment_operator(line))
            .collect();
        self.calc.prev_line_has_builtin_formula = self
            .lines
            .iter()
            .map(|line| {
                crate::editor_core::calc_plan::contains_builtin_formula(std::slice::from_ref(line))
            })
            .collect();
        self.calc.results = new_results;
        self.calc.cell_results = new_cell_results;
        self.calc.variable_names = variable_names;
        self.calc.stale = false;
    }

    // --- Search ---

    fn open_search(&mut self) {
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
        let len = chars.len();

        let mut col = self.cursor_col;
        if col > len {
            col = len;
        }
        if col == 0 {
            self.cursor_col = 0;
            return;
        }

        col -= 1;
        while col > 0 && chars.get(col).map_or(false, |c| c.is_whitespace()) {
            col -= 1;
        }

        let target_class = chars.get(col).map_or(0, |c| {
            if c.is_alphanumeric() || *c == '_' {
                1
            } else {
                2
            }
        });
        while col > 0 {
            let prev_class = chars.get(col - 1).map_or(0, |c| {
                if c.is_whitespace() {
                    0
                } else if c.is_alphanumeric() || *c == '_' {
                    1
                } else {
                    2
                }
            });
            if prev_class == target_class {
                col -= 1;
            } else {
                break;
            }
        }
        self.cursor_col = col;
    }

    fn move_cursor_right_word(&mut self) {
        let line = self.current_line();
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        if self.cursor_col >= len {
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
        let start_class = chars.get(col).map_or(0, |c| {
            if c.is_whitespace() {
                0
            } else if c.is_alphanumeric() || *c == '_' {
                1
            } else {
                2
            }
        });

        while col < len {
            let current_class = chars.get(col).map_or(0, |c| {
                if c.is_whitespace() {
                    0
                } else if c.is_alphanumeric() || *c == '_' {
                    1
                } else {
                    2
                }
            });
            if current_class == start_class {
                col += 1;
            } else {
                break;
            }
        }

        if start_class != 0 {
            while col < len && chars.get(col).map_or(false, |c| c.is_whitespace()) {
                col += 1;
            }
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
        if self.note_table_module_enabled() {
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
            self.mark_edited_from_line(line_idx);
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
        self.mark_edited_from_line(line_idx);
    }

    fn insert_newline(&mut self) {
        let changed_from_line = self.cursor_line;
        let col = self.cursor_col;
        let idx = byte_index(self.current_line(), col);
        let right = self.lines[self.cursor_line][idx..].to_string();
        self.lines[self.cursor_line].truncate(idx);
        let insert_at = self.cursor_line + 1;
        self.lines.insert(insert_at, right);
        self.cursor_line += 1;
        self.cursor_col = 0;
        self.mark_edited_from_line(changed_from_line);
    }

    fn variable_autocomplete_state(&self) -> Option<VariableAutocompleteState> {
        if self.mode != UiMode::Editor {
            return None;
        }
        if !self.note_math_module_enabled()
            || !self.note_variables_module_enabled()
            || self.calc.variable_names.is_empty()
        {
            return None;
        }
        let line = self.current_line();
        let prefix = extract_variable_completion_prefix(line, self.cursor_col)?;
        let suggestions = build_variable_suggestions(
            &self.calc.variable_names,
            &prefix.query,
            self.variable_autocomplete_min_chars,
            VARIABLE_AUTOCOMPLETE_MAX_SUGGESTIONS,
        );
        if suggestions.is_empty() {
            return None;
        }
        Some(VariableAutocompleteState {
            from_col: prefix.from_col,
            to_col: prefix.to_col,
            query: prefix.query,
            suggestions,
        })
    }

    fn variable_popup_anchor(&self, anchor_col: usize) -> Option<(usize, usize)> {
        let (rows, cols) = input::terminal_size();
        if rows <= EDITOR_TOP_ROW || cols == 0 {
            return None;
        }
        let cursor_virtual = self.current_virtual_line();
        let row = EDITOR_TOP_ROW
            + cursor_virtual
                .saturating_sub(self.scroll_line)
                .min(rows.saturating_sub(2));
        let gutter_width = self.gutter_width();
        let available = cols.saturating_sub(gutter_width);
        if available == 0 {
            return None;
        }
        let line_text = self.current_line();
        let line_col = anchor_col.min(line_char_len(line_text));
        let display_col = display_cols_for_prefix(line_text, line_col);
        let line_width = line_display_cols(line_text);
        let visible_col =
            viewport_col_for_display_col(display_col, line_width, self.scroll_col, available);
        let col = (gutter_width + visible_col + 1).min(cols.max(1)).max(1);
        Some((row.max(EDITOR_TOP_ROW), col))
    }

    fn dismiss_variable_autocomplete_popup(&mut self) {
        self.variable_autocomplete_popup = VariableAutocompletePopupState::default();
    }

    fn refresh_variable_autocomplete_popup(&mut self) {
        let Some(state) = self.variable_autocomplete_state() else {
            self.dismiss_variable_autocomplete_popup();
            return;
        };
        let Some((anchor_row, anchor_col)) = self.variable_popup_anchor(state.from_col) else {
            self.dismiss_variable_autocomplete_popup();
            return;
        };
        let previous_selection = if self.variable_autocomplete_popup.visible
            && self.variable_autocomplete_popup.cursor_line == self.cursor_line
            && self.variable_autocomplete_popup.cursor_col <= self.cursor_col
            && self.variable_autocomplete_popup.query == state.query
        {
            self.variable_autocomplete_popup
                .suggestions
                .get(self.variable_autocomplete_popup.selected_index)
                .cloned()
        } else {
            None
        };
        let selected_index = previous_selection
            .as_ref()
            .and_then(|picked| state.suggestions.iter().position(|name| name == picked))
            .unwrap_or(0)
            .min(state.suggestions.len().saturating_sub(1));
        self.variable_autocomplete_popup = VariableAutocompletePopupState {
            visible: true,
            anchor_row,
            anchor_col,
            from_col: state.from_col,
            to_col: state.to_col,
            query: state.query,
            suggestions: state.suggestions,
            selected_index,
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        };
    }

    fn move_variable_autocomplete_selection(&mut self, delta: isize) -> bool {
        if !self.variable_autocomplete_popup.visible
            || self.variable_autocomplete_popup.suggestions.is_empty()
        {
            return false;
        }
        let len = self.variable_autocomplete_popup.suggestions.len();
        let current = self
            .variable_autocomplete_popup
            .selected_index
            .min(len.saturating_sub(1));
        let next = if delta >= 0 {
            (current + delta as usize) % len
        } else {
            (current + len - ((-delta) as usize % len)) % len
        };
        self.variable_autocomplete_popup.selected_index = next;
        true
    }

    fn apply_variable_autocomplete_pick(
        &mut self,
        from_col: usize,
        to_col: usize,
        pick: String,
    ) -> bool {
        let from_col = from_col.min(self.cursor_col);
        let to_col = to_col.min(line_char_len(self.current_line()));
        if from_col >= to_col {
            return false;
        }

        let from_byte = byte_index(self.current_line(), from_col);
        let to_byte = byte_index(self.current_line(), to_col);
        self.lines[self.cursor_line].replace_range(from_byte..to_byte, &pick);
        self.cursor_col = from_col + pick.chars().count();
        self.mark_edited();
        self.status = format!("autocomplete: {pick}");
        self.dismiss_variable_autocomplete_popup();
        true
    }

    fn apply_variable_autocomplete_popup_selection(&mut self) -> bool {
        if !self.variable_autocomplete_popup.visible
            || self.variable_autocomplete_popup.cursor_line != self.cursor_line
            || self.variable_autocomplete_popup.cursor_col > self.cursor_col
        {
            self.dismiss_variable_autocomplete_popup();
            return false;
        }
        let pick = self
            .variable_autocomplete_popup
            .suggestions
            .get(self.variable_autocomplete_popup.selected_index)
            .cloned();
        let Some(pick) = pick else {
            self.dismiss_variable_autocomplete_popup();
            return false;
        };
        self.apply_variable_autocomplete_pick(
            self.variable_autocomplete_popup.from_col,
            self.variable_autocomplete_popup.to_col,
            pick,
        )
    }

    fn variable_autocomplete_status_hint(&self) -> Option<String> {
        let (query, suggestions, selected) = if self.variable_autocomplete_popup.visible {
            (
                self.variable_autocomplete_popup.query.clone(),
                self.variable_autocomplete_popup.suggestions.clone(),
                Some(self.variable_autocomplete_popup.selected_index),
            )
        } else {
            let state = self.variable_autocomplete_state()?;
            (state.query, state.suggestions, None)
        };
        let picks = suggestions
            .iter()
            .take(VARIABLE_AUTOCOMPLETE_MAX_SUGGESTIONS)
            .enumerate()
            .map(|(idx, suggestion)| {
                if selected == Some(idx) {
                    format!(">{suggestion}<")
                } else {
                    suggestion.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("  ");
        if picks.is_empty() {
            None
        } else {
            Some(format!("var {query} -> {picks} (Tab/Enter)"))
        }
    }

    fn apply_variable_autocomplete_tab(&mut self) -> bool {
        if self.variable_autocomplete_popup.visible {
            return self.apply_variable_autocomplete_popup_selection();
        }
        let Some(state) = self.variable_autocomplete_state() else {
            return false;
        };
        let Some(pick) = state.suggestions.first().cloned() else {
            return false;
        };
        self.apply_variable_autocomplete_pick(state.from_col, state.to_col, pick)
    }

    fn apply_calc_tab(&mut self) -> bool {
        if !self.note_math_module_enabled() {
            return false;
        }
        let text = self.current_line().to_string();
        let Some(result) = self
            .calc
            .results
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
        if !self.note_style_module_enabled() {
            return;
        }
        if !Self::line_might_trigger_doc_change_rules(self.current_line()) {
            return;
        }

        let ctx = self.build_context();
        let options = crate::editor_core::text_rules::TextRuleOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            checklist_auto_reorder: self.checklist_auto_reorder_enabled(),
        };
        if let Some(op) = crate::editor_core::text_rules::run_doc_change_rules(&ctx, options) {
            self.apply_edit_operation(&op);
        }
    }

    fn try_enter_rule(&mut self) -> bool {
        let ctx = self.build_context();
        let options = crate::editor_core::text_rules::TextRuleOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            checklist_auto_reorder: self.checklist_auto_reorder_enabled(),
        };
        if let Some(op) = crate::editor_core::text_rules::run_enter_rules(&ctx, options) {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    fn try_tab_rule(&mut self, outdent: bool) -> bool {
        let ctx = self.build_context();
        let options = crate::editor_core::text_rules::TabRuleOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            outdent,
        };
        if let Some(op) = crate::editor_core::text_rules::run_tab_rules(&ctx, options) {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    fn try_table_navigation_rule(&mut self, outdent: bool) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let ctx = self.build_context();
        let options = crate::editor_core::text_rules::TabRuleOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            outdent,
        };
        if let Some(op) =
            crate::editor_core::text_rules::run_table_cell_navigation_rules(&ctx, options)
        {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    fn try_table_pipe_insert_column_rule(&mut self) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let ctx = self.build_context();
        if let Some(op) = crate::editor_core::text_rules::run_table_pipe_insert_column_rule(&ctx) {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    fn try_table_header_delete_column_rule(&mut self) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let ctx = self.build_context();
        if let Some(op) = crate::editor_core::text_rules::run_table_header_delete_column_rule(&ctx)
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
        if !self.note_table_module_enabled() {
            return None;
        }
        let ctx = self.build_context();
        let options = crate::editor_core::text_rules::TableBoundaryEditOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            backward,
            structural_merge,
        };
        let op = crate::editor_core::text_rules::run_table_boundary_edit_rules(&ctx, options)?;
        let changed = !op.changes.is_empty();
        self.apply_edit_operation(&op);
        Some(changed)
    }

    fn apply_edit_operation(&mut self, op: &crate::editor_core::types::EditOperation) {
        if !self.active_note_is_editable() && !op.changes.is_empty() {
            self.set_locked_note_status();
            return;
        }
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
        let changed_from_offset = op
            .changes
            .iter()
            .map(|change| change.from.min(text.len()))
            .min()
            .unwrap_or(0);
        let changed_from_line = text.as_bytes()[..changed_from_offset]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();

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
        self.folds.rescan_pending = true;
        self.mark_edited_from_line(changed_from_line);
        self.adjust_cursor();
        self.adjust_scroll();
    }

    fn backspace(&mut self) {
        if self.note_table_module_enabled() {
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
        if self.note_table_module_enabled() {
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
        let table_target_col = if self.note_table_module_enabled() {
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
        } else {
            None
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
        let table_target_col = if self.note_table_module_enabled() {
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
        } else {
            None
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
        if self.note_table_module_enabled() {
            if let Some(cell) = table_cell_info_at_char(self.current_line(), self.cursor_col) {
                self.cursor_col = table_cell_navigation_anchor(self.current_line(), &cell);
            }
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
        if self.note_table_module_enabled() {
            if let Some(cell) = table_cell_info_at_char(self.current_line(), self.cursor_col) {
                self.cursor_col = table_cell_navigation_anchor(self.current_line(), &cell);
            }
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
        let table_anchor = if self.note_table_module_enabled() {
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
        } else {
            None
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

    fn calc_eval_range_for_viewport(&self, editor_height: usize) -> Option<(usize, usize)> {
        if self.lines.is_empty() || editor_height == 0 {
            return None;
        }
        let visible_count = self.visible_line_count();
        if visible_count == 0 {
            return None;
        }
        let prefetch = editor_height.saturating_mul(CALC_VIEWPORT_PREFETCH_MULTIPLIER);
        let start_virtual = self.scroll_line.saturating_sub(prefetch);
        let end_virtual = self
            .scroll_line
            .saturating_add(editor_height)
            .saturating_add(prefetch)
            .min(visible_count.saturating_sub(1));
        let start_line = self.real_line_for_virtual(start_virtual).unwrap_or(0);
        let end_line_inclusive = self
            .real_line_for_virtual(end_virtual)
            .unwrap_or_else(|| self.lines.len().saturating_sub(1));
        let end_line_exclusive = end_line_inclusive.saturating_add(1).min(self.lines.len());
        if start_line >= end_line_exclusive {
            None
        } else {
            Some((start_line, end_line_exclusive))
        }
    }

    fn recompute_calc_range(&mut self, eval_from: usize, eval_to: usize) {
        if !self.note_math_module_enabled() {
            self.clear_calc_cache();
            return;
        }
        if eval_from >= eval_to || eval_to > self.lines.len() {
            return;
        }
        let calc_data = compute_calc_data(
            &self.calc.engine,
            &self.lines,
            self.calc_variables_enabled(),
            Some((eval_from, eval_to)),
        );
        if self.calc.results.len() != self.lines.len() {
            self.calc.results = vec![None; self.lines.len()];
        }
        if self.calc.cell_results.len() != self.lines.len() {
            self.calc.cell_results = vec![Vec::new(); self.lines.len()];
        }
        for line_idx in eval_from..eval_to {
            if let Some(slot) = self.calc.results.get_mut(line_idx) {
                *slot = calc_data
                    .line_results
                    .get(line_idx)
                    .cloned()
                    .unwrap_or(None);
            }
            if let Some(slot) = self.calc.cell_results.get_mut(line_idx) {
                *slot = calc_data
                    .cell_results
                    .get(line_idx)
                    .cloned()
                    .unwrap_or_default();
            }
        }
        self.calc.variable_names = calc_data.variable_names;
    }

    fn ensure_calc_for_viewport(&mut self, editor_height: usize, force: bool) {
        if !self.calc_viewport_only {
            return;
        }
        let Some(eval_range) = self.calc_eval_range_for_viewport(editor_height) else {
            return;
        };
        if !force && self.calc_last_view_eval_range == Some(eval_range) {
            return;
        }
        self.recompute_calc_range(eval_range.0, eval_range.1);
        self.calc_last_view_eval_range = Some(eval_range);
    }

    fn draw_variable_autocomplete_popup(&self, buf: &mut String, rows: usize, cols: usize) {
        if self.mode != UiMode::Editor
            || !self.variable_autocomplete_popup.visible
            || self.variable_autocomplete_popup.suggestions.is_empty()
            || cols == 0
            || rows <= EDITOR_TOP_ROW
        {
            return;
        }

        let max_editor_row = rows.saturating_sub(1);
        let available_editor_rows = max_editor_row.saturating_sub(EDITOR_TOP_ROW) + 1;
        if available_editor_rows < 3 {
            return;
        }

        let max_suggestions = available_editor_rows.saturating_sub(2).max(1);
        let visible_count = self
            .variable_autocomplete_popup
            .suggestions
            .len()
            .min(max_suggestions);
        let suggestions = &self.variable_autocomplete_popup.suggestions[..visible_count];
        let selected_index = self
            .variable_autocomplete_popup
            .selected_index
            .min(visible_count.saturating_sub(1));

        let inner_width = suggestions
            .iter()
            .map(|item| item.chars().count() + 2)
            .max()
            .unwrap_or(1)
            .min(cols.saturating_sub(2).max(1));
        let box_width = (inner_width + 2).min(cols.max(1));
        let box_height = visible_count + 2;

        let mut x = self.variable_autocomplete_popup.anchor_col.min(cols.max(1));
        if x + box_width > cols + 1 {
            x = cols.saturating_sub(box_width).saturating_add(1).max(1);
        }

        let preferred_top = self
            .variable_autocomplete_popup
            .anchor_row
            .saturating_add(1);
        let mut y = preferred_top;
        if y + box_height > max_editor_row + 1 {
            y = self
                .variable_autocomplete_popup
                .anchor_row
                .saturating_sub(box_height.saturating_sub(1));
        }
        y = y
            .max(EDITOR_TOP_ROW)
            .min(max_editor_row.saturating_sub(box_height.saturating_sub(1)));

        let border_style = AnsiStyle {
            fg: Some(self.render_palette.code_type),
            ..Default::default()
        };
        let row_style = AnsiStyle {
            fg: Some(self.render_palette.variable),
            ..Default::default()
        };
        let selected_bg = self.render_palette.primary();
        let selected_style = AnsiStyle {
            fg: Some(contrast_fg_for_bg(selected_bg)),
            bg: Some(selected_bg),
            bold: true,
            ..Default::default()
        };

        draw_box_border(buf, y, x, box_width, box_height, border_style);

        for (idx, suggestion) in suggestions.iter().enumerate() {
            let row = y + 1 + idx;
            let text = format!(" {suggestion}");
            if idx == selected_index {
                draw_row_at_styled(
                    buf,
                    row,
                    x + 1,
                    box_width.saturating_sub(2),
                    &text,
                    selected_style,
                );
            } else {
                draw_row_at_styled(
                    buf,
                    row,
                    x + 1,
                    box_width.saturating_sub(2),
                    &text,
                    row_style,
                );
            }
        }
    }

    fn draw(&mut self, out: &mut impl Write) -> Result<(), String> {
        let (rows, cols) = input::terminal_size();
        let editor_height = rows.saturating_sub(2).max(1);
        self.ensure_calc_for_viewport(editor_height, false);
        let gutter_width = self.gutter_width();
        let line_number_width = gutter_width.saturating_sub(2);
        let mut buf = std::mem::take(&mut self.draw_buf);
        buf.clear();

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
        let title_bg = self.render_palette.search_match;
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

        let first_real_line = self
            .real_line_for_virtual(self.scroll_line)
            .unwrap_or(self.lines.len());
        let (fence_in_code_block, fence_lang) = self.fence_state_before_line(first_real_line);
        let mut ctx = render::RenderContext::with_fence_state(
            fence_in_code_block,
            fence_lang,
            self.render_palette,
        );
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
                let mut calc_ghost = self.calc.results.get(line_idx).and_then(|r| r.as_deref());
                let mut calc_ghost_override: Option<String> = None;
                let mut reminder_ghost_override: Option<String> = None;
                let mut reminder_strikethrough = false;
                let mut ghost_dim_ranges: Vec<(usize, usize)> = Vec::new();
                let mut formula_segments: Vec<TableFormulaSegment> = Vec::new();
                let line_text = &self.lines[line_idx];
                let mut rendered_line = line_text.to_string();
                let collapsed_hidden_count = self
                    .folds
                    .placeholder_hidden_lines
                    .get(line_idx)
                    .and_then(|entry| *entry);
                let is_fold_placeholder = collapsed_hidden_count.is_some();

                if let Some(hidden_count) = collapsed_hidden_count {
                    let suffix = if hidden_count == 1 { "" } else { "s" };
                    rendered_line = "".to_string();
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
                            .calc
                            .cell_results
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
                                    self.calc
                                        .results
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

                if is_cursor_line && cursor_line_override.is_none() && !is_fold_placeholder {
                    let (collapsed_line, mapped_col) =
                        render::collapse_markdown_line_for_cursor(&rendered_line, self.cursor_col);
                    if collapsed_line != rendered_line || mapped_col != self.cursor_col {
                        cursor_line_override = Some((collapsed_line, mapped_col));
                    }
                }

                let effective_calc_ghost = calc_ghost_override.as_deref().or(calc_ghost);
                let effective_reminder_ghost = reminder_ghost_override.as_deref();
                let render_cursor_col = if is_cursor_line {
                    if !formula_segments.is_empty() {
                        cursor_line_override
                            .as_ref()
                            .map(|(_, mapped_col)| *mapped_col)
                            .or(Some(self.cursor_col))
                    } else {
                        Some(self.cursor_col)
                    }
                } else {
                    None
                };
                let line_scroll_col = self.scroll_col;
                let line_width = line_display_cols(&rendered_line);
                let viewport = compute_line_viewport(line_width, line_scroll_col, available);

                // Highlight the focused table cell's pipe characters in red so
                // the active cell is obvious. Pipe positions are taken from
                // the source `line_text` and translated to rendered char
                // positions using the formula-mask delta accumulated above.
                let mut focused_pipe_ranges: Vec<(usize, usize)> = Vec::new();
                if self.note_table_module_enabled() && is_cursor_line && !is_fold_placeholder {
                    if let Some(info) = table_cell_info_at_char(line_text, self.cursor_col) {
                        let left_pipe_char = line_text[..info.left_pipe].chars().count();
                        let right_pipe_char = line_text[..info.right_pipe].chars().count();
                        let translate = |src_col: usize| -> usize {
                            let mut delta: isize = 0;
                            for (fi, seg) in formula_segments.iter().enumerate() {
                                if seg.cell_to_char <= src_col {
                                    let value = self
                                        .calc
                                        .cell_results
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
                    ctx.render_line_window_with_reminder_cursor(
                        &rendered_line,
                        viewport.text_width,
                        viewport.text_window_col,
                        effective_calc_ghost,
                        effective_reminder_ghost,
                        reminder_strikethrough,
                        &search_ranges,
                        &current_search_ranges,
                        &self.calc.variable_names,
                        render_cursor_col,
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
                        &self.calc.variable_names,
                        &ghost_dim_ranges,
                        &visual_highlight_ranges,
                        &focused_pipe_ranges,
                        render_cursor_col,
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

        let status_owned = if self.mode == UiMode::Editor {
            self.variable_autocomplete_status_hint()
                .map(|hint| format!("{}  [{}]", self.status, hint))
        } else {
            None
        };
        let status = match self.mode {
            UiMode::Editor => status_owned.as_deref().unwrap_or(&self.status),
            UiMode::Normal
            | UiMode::CommandBar
            | UiMode::Search
            | UiMode::Visual
            | UiMode::VisualLine => &self.status,
            UiMode::Switcher => {
                if self.switcher_open_confirm.is_some() {
                    "Open note: type password, Enter confirm, Esc cancel"
                } else if let Some(confirm) = self.switcher_delete_confirm.as_ref() {
                    if confirm.requires_password {
                        "Confirm delete: type password, Enter confirm, Esc cancel"
                    } else {
                        "Confirm delete: Enter/Y confirm, Esc/N cancel"
                    }
                } else {
                    "Switcher: type to filter, Enter open, Delete/Ctrl+Backspace delete, Esc close"
                }
            }
            UiMode::DatePicker => {
                "Date picker: arrows navigate, Ctrl+arrows months, Enter insert, Esc cancel"
            }
        };
        let status_bg = self.render_palette.search_match;
        if !self.draw_command_completion_status_row(&mut buf, rows, cols, status_bg) {
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
        }

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
                    confirm.requires_password,
                    confirm.password.chars().count(),
                    &mut buf,
                    rows,
                    cols,
                    self.render_palette,
                );
            }
            if let Some(confirm) = self.switcher_open_confirm.as_ref() {
                switcher::draw_open_confirm(
                    &confirm.note_title,
                    confirm.password.chars().count(),
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
        self.draw_variable_autocomplete_popup(&mut buf, rows, cols);

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

        let result = out
            .write_all(buf.as_bytes())
            .and_then(|_| out.flush())
            .map_err(|e| format!("Failed to draw terminal UI: {e}"));
        self.draw_buf = buf;
        result
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
        config.variables_autocomplete_min_chars,
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

fn select_note(db: &Db, opts: &TerminalOptions, config: &ThemeConfig) -> Result<Note, String> {
    if opts.create_new {
        return new_note(db, config);
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

    new_note(db, config)
}

fn new_note(db: &Db, config: &ThemeConfig) -> Result<Note, String> {
    let id = Ulid::new().to_string();
    let security = crate::config::note_security_config_from_theme(config);
    let default_password = crate::config::resolve_default_note_encryption_password(&security)?;
    let modules = NoteModules {
        math: config.default_modules.math,
        table: config.default_modules.table,
        variables: config.default_modules.variables,
        style: config.default_modules.style,
    };
    db.create_note_with_defaults(&id, modules, default_password.as_deref())
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

fn variable_query_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == ' '
}

fn extract_variable_completion_prefix(
    line_text: &str,
    cursor_col: usize,
) -> Option<VariableCompletionPrefix> {
    let chars = line_text.chars().collect::<Vec<_>>();
    let col = cursor_col.min(chars.len());
    let mut from = col;

    while from > 0 && variable_query_char(chars[from - 1]) {
        from -= 1;
    }

    while from < col && chars[from] == ' ' {
        from += 1;
    }

    if from >= col {
        return None;
    }

    let query = chars[from..col].iter().collect::<String>();
    if query.is_empty() || query.ends_with(' ') {
        return None;
    }

    Some(VariableCompletionPrefix {
        from_col: from,
        to_col: col,
        query,
    })
}

fn build_variable_suggestions(
    variable_names: &[String],
    query: &str,
    min_chars: usize,
    max_suggestions: usize,
) -> Vec<String> {
    let normalized_query = query.trim().to_lowercase();
    if normalized_query.chars().count() < min_chars {
        return Vec::new();
    }

    let mut matches = variable_names
        .iter()
        .filter(|name| name.starts_with(&normalized_query) && **name != normalized_query)
        .cloned()
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    matches.truncate(max_suggestions.max(1));
    matches
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
        build_variable_suggestions, builtin_formula_label, compute_calc_results,
        compute_calc_trailer_refresh, extract_variable_completion_prefix, find_calc_segment_range,
        find_table_formula_segment, format_formula_display_value, rendered_line_display_cols,
        should_mask_formula_cell, table_cell_info_at_char, table_cell_is_empty,
        table_cell_navigation_anchor,
    };
    use super::{display_cols_for_prefix, line_char_len};
    use super::{TerminalApp, TerminalOptions, UiMode};
    use crate::storage::Db;
    use app_core::storage::NoteAccessMode;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
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
            3,
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

    fn note_modules(
        math: bool,
        table: bool,
        variables: bool,
        style: bool,
    ) -> app_core::storage::NoteModules {
        app_core::storage::NoteModules {
            math,
            table,
            variables,
            style,
        }
    }

    fn note_modules_with_variables(enabled: bool) -> app_core::storage::NoteModules {
        note_modules(true, true, enabled, true)
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
    fn display_cols_for_prefix_counts_wide_chars_as_two_columns() {
        // Each emoji occupies 2 terminal columns.
        assert_eq!(display_cols_for_prefix("😀", 1), 2);
        assert_eq!(display_cols_for_prefix("😀a", 1), 2);
        assert_eq!(display_cols_for_prefix("😀a", 2), 3);
        assert_eq!(display_cols_for_prefix("a😀b", 2), 3);
        assert_eq!(display_cols_for_prefix("a😀b", 3), 4);
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
    fn insert_newline_invalidates_fence_checkpoints_from_original_line() {
        let interval = super::FENCE_CHECKPOINT_INTERVAL;
        let fence_line = interval - 1;
        let mut lines = vec!["plain".to_string(); interval + 40];
        lines[fence_line] = "```".to_string();
        lines[fence_line + 2] = "```".to_string();
        let body = lines.join("\n");
        let (db, mut app, path) = app_with_note(&body);

        let _ = app.fence_state_before_line(interval + 20);
        assert!(app.fence_checkpoints_valid_through >= 1);

        app.cursor_line = fence_line;
        app.cursor_col = 0;
        app.insert_newline();

        assert_eq!(app.cursor_line, interval);
        assert_eq!(app.fence_checkpoints_valid_through, 0);

        let (in_code_block, _) = app.fence_state_before_line(interval);
        assert!(!in_code_block);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn insert_paste_multiline_invalidates_fence_checkpoints_from_original_line() {
        let interval = super::FENCE_CHECKPOINT_INTERVAL;
        let fence_line = interval - 1;
        let mut lines = vec!["plain".to_string(); interval + 40];
        lines[fence_line] = "```".to_string();
        lines[fence_line + 2] = "```".to_string();
        let body = lines.join("\n");
        let (db, mut app, path) = app_with_note(&body);

        let _ = app.fence_state_before_line(interval + 20);
        assert!(app.fence_checkpoints_valid_through >= 1);

        app.cursor_line = fence_line;
        app.cursor_col = 0;
        app.insert_paste("\n");

        assert_eq!(app.cursor_line, interval);
        assert_eq!(app.fence_checkpoints_valid_through, 0);

        let (in_code_block, _) = app.fence_state_before_line(interval);
        assert!(!in_code_block);

        drop(app);
        drop(db);
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
        assert_eq!(app.folds.real_to_visible.len(), app.lines.len());
        assert_eq!(app.folds.hidden_owner.len(), app.lines.len());
        assert_eq!(app.folds.placeholder_hidden_lines.len(), app.lines.len());
        assert_eq!(app.folds.range_by_start.len(), app.lines.len());
        assert!(app.scroll_line > 0);
        assert!(app.scroll_line >= insert_scroll_before.saturating_sub(1));

        app.mode = UiMode::Normal;
        app.cursor_line = app.lines.len().saturating_sub(2);
        app.cursor_col = 0;
        app.adjust_scroll();
        let delete_scroll_before = app.scroll_line;

        run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('d')]);

        assert_eq!(app.lines.len(), 100_000);
        assert_eq!(app.folds.real_to_visible.len(), app.lines.len());
        assert_eq!(app.folds.hidden_owner.len(), app.lines.len());
        assert_eq!(app.folds.placeholder_hidden_lines.len(), app.lines.len());
        assert_eq!(app.folds.range_by_start.len(), app.lines.len());
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
                app.folds.real_to_visible.len(),
                app.lines.len(),
                "step {step_idx}: fold_real_to_visible size mismatch after op {op}"
            );
            assert_eq!(
                app.folds.hidden_owner.len(),
                app.lines.len(),
                "step {step_idx}: fold_hidden_owner size mismatch after op {op}"
            );
            assert_eq!(
                app.folds.placeholder_hidden_lines.len(),
                app.lines.len(),
                "step {step_idx}: fold_placeholder_hidden_lines size mismatch after op {op}"
            );
            assert_eq!(
                app.folds.range_by_start.len(),
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

        assert!(app.calc.stale);
        assert!(!app.calc.cached_has_builtin_formula);
        assert!(!app.calc.cached_has_variable_assignment);

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

        assert!(app.calc.cached_has_variable_assignment);
        assert!(!app.calc.stale);
        assert_eq!(app.calc.prev_line_hashes.len(), app.lines.len());
        assert_eq!(app.calc.prev_line_has_assignment.len(), app.lines.len());
        assert!(app.calc.variable_names.iter().any(|name| name == "total"));

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

        assert!(app.folds.collapsed_starts.contains(&0));
        assert_eq!(app.folds.visible_to_real, vec![0, 3, 4]);
        assert_eq!(app.folds.placeholder_hidden_lines[0], Some(2));

        run_keys(&mut app, &db, &[Key::Char('j')]);
        assert_eq!(app.cursor_line, 3);
        assert_eq!(app.current_virtual_line(), 1);

        app.cursor_line = 1;
        app.adjust_cursor();
        assert_eq!(app.cursor_line, 0);

        run_keys(&mut app, &db, &[Key::Char('z'), Key::Char('a')]);
        assert!(!app.folds.collapsed_starts.contains(&0));

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
    fn variable_completion_prefix_resolves_current_query_span() {
        let prefix =
            extract_variable_completion_prefix("total cost + tax", 10).expect("prefix exists");
        assert_eq!(prefix.from_col, 0);
        assert_eq!(prefix.to_col, 10);
        assert_eq!(prefix.query, "total cost");
    }

    #[test]
    fn variable_completion_prefix_skips_leading_spaces_and_rejects_trailing_space() {
        let prefix = extract_variable_completion_prefix("   total", 8).expect("prefix exists");
        assert_eq!(prefix.from_col, 3);
        assert_eq!(prefix.to_col, 8);
        assert_eq!(prefix.query, "total");

        assert!(extract_variable_completion_prefix("total ", 6).is_none());
    }

    #[test]
    fn variable_suggestions_require_min_chars_and_exclude_exact_match() {
        let variables = vec![
            "total cost".to_string(),
            "tax".to_string(),
            "total revenue".to_string(),
        ];

        assert!(build_variable_suggestions(&variables, "to", 3, 8).is_empty());

        let picks = build_variable_suggestions(&variables, "tot", 3, 8);
        assert_eq!(
            picks,
            vec!["total cost".to_string(), "total revenue".to_string()]
        );

        assert!(build_variable_suggestions(&variables, "total cost", 3, 8).is_empty());
    }

    #[test]
    fn initial_open_without_calc_syntax_keeps_calc_cache_lightweight() {
        let (db, app, path) = app_with_note("plain line\nanother plain line");

        assert!(!app.calc.cached_has_builtin_formula);
        assert!(!app.calc.cached_has_variable_assignment);
        assert!(!app.calc.stale);
        assert_eq!(app.calc.results.len(), app.lines.len());
        assert!(app.calc.results.iter().all(|entry| entry.is_none()));
        assert!(app.calc.cell_results.iter().all(|row| row.is_empty()));
        assert!(app.calc.prev_line_hashes.is_empty());
        assert!(app.calc.prev_line_has_assignment.is_empty());
        assert!(app.calc.prev_line_has_builtin_formula.is_empty());

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn large_note_with_assignments_uses_viewport_calc_on_open() {
        let mut lines = Vec::new();
        lines.push("base := 1".to_string());
        lines.extend((0..2_500).map(|_| "base + 2".to_string()));
        let body = lines.join("\n");
        let (db, app, path) = app_with_note(&body);

        assert!(app.calc_viewport_only);
        assert!(app.calc_last_view_eval_range.is_some());
        assert_eq!(
            app.calc.results.get(1).and_then(|entry| entry.as_deref()),
            Some("3")
        );
        assert_eq!(
            app.calc.results.last().and_then(|entry| entry.as_deref()),
            None
        );

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn viewport_calc_evaluates_new_window_after_scroll() {
        let mut lines = Vec::new();
        lines.push("base := 1".to_string());
        lines.extend((0..2_500).map(|_| "base + 2".to_string()));
        let body = lines.join("\n");
        let (db, mut app, path) = app_with_note(&body);

        let last_idx = app.lines.len().saturating_sub(1);
        assert_eq!(
            app.calc
                .results
                .get(last_idx)
                .and_then(|entry| entry.as_deref()),
            None
        );

        app.cursor_line = last_idx;
        app.adjust_cursor();
        app.adjust_scroll();
        let mut out = Vec::new();
        app.draw(&mut out).expect("draw after scroll");

        assert_eq!(
            app.calc
                .results
                .get(last_idx)
                .and_then(|entry| entry.as_deref()),
            Some("3")
        );

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn set_active_note_without_calc_syntax_skips_full_calc_recompute() {
        let (db, mut app, path) = app_with_note("x := 4\nx + 2");
        assert!(app.calc.cached_has_variable_assignment);
        assert!(!app.calc.prev_line_hashes.is_empty());

        let plain_note = db
            .save_note("n2", "plain line\nstill plain")
            .expect("save plain note");
        app.set_active_note(&db, plain_note)
            .expect("switch to plain note");

        assert!(!app.calc.cached_has_builtin_formula);
        assert!(!app.calc.cached_has_variable_assignment);
        assert!(!app.calc.stale);
        assert_eq!(app.calc.results.len(), app.lines.len());
        assert!(app.calc.results.iter().all(|entry| entry.is_none()));
        assert!(app.calc.cell_results.iter().all(|row| row.is_empty()));
        assert!(app.calc.prev_line_hashes.is_empty());
        assert!(app.calc.prev_line_has_assignment.is_empty());
        assert!(app.calc.prev_line_has_builtin_formula.is_empty());

        drop(app);
        drop(db);
        cleanup_db_files(&path);
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
        app.calc.results = vec![
            Some("2".to_string()),
            Some("4".to_string()),
            Some("6".to_string()),
        ];
        app.calc.variable_names = vec!["total".to_string()];
        app.cursor_line = 1;

        app.defer_calc_state_after_edit();

        assert_eq!(app.calc.results, vec![None, None, None]);
        assert!(app.calc.variable_names.is_empty());
        assert!(app.calc.stale);

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
    fn variable_autocomplete_popup_appears_after_min_chars_and_supports_selection_keys() {
        let (db, mut app, path) = app_with_note("total cost := 10\ntotal revenue := 20\nto");
        app.cursor_line = 2;
        app.cursor_col = line_char_len(app.current_line());
        app.refresh_variable_autocomplete_popup();
        assert!(!app.variable_autocomplete_popup.visible);

        app.handle_editor_key(&db, Key::Char('t'))
            .expect("typing triggers popup refresh");
        assert!(app.variable_autocomplete_popup.visible);
        assert_eq!(app.variable_autocomplete_popup.selected_index, 0);
        assert_eq!(app.variable_autocomplete_popup.suggestions.len(), 2);

        app.handle_editor_key(&db, Key::ArrowDown)
            .expect("down picks next");
        assert_eq!(app.variable_autocomplete_popup.selected_index, 1);
        assert_eq!(app.cursor_line, 2);
        assert_eq!(app.cursor_col, 3);

        app.handle_editor_key(&db, Key::ArrowUp)
            .expect("up picks previous");
        assert_eq!(app.variable_autocomplete_popup.selected_index, 0);

        app.handle_editor_key(&db, Key::Enter)
            .expect("enter accepts selected suggestion");
        assert_eq!(app.lines[2], "total cost");
        assert_eq!(app.cursor_col, "total cost".chars().count());
        assert!(!app.variable_autocomplete_popup.visible);

        app.lines[2] = "tot".to_string();
        app.cursor_col = 3;
        app.refresh_variable_autocomplete_popup();
        app.handle_editor_key(&db, Key::ArrowDown)
            .expect("down selects second suggestion");
        app.handle_editor_key(&db, Key::Tab)
            .expect("tab accepts selected suggestion");
        assert_eq!(app.lines[2], "total revenue");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn autocomplete_popup_esc_dismisses_and_tab_fallback_still_accepts_variable() {
        let (db, mut app, path) = app_with_note("total cost := 10\ntotal revenue := 20\ntot");
        app.cursor_line = 2;
        app.cursor_col = line_char_len(app.current_line());
        app.refresh_variable_autocomplete_popup();
        assert!(app.variable_autocomplete_popup.visible);

        app.handle_editor_key(&db, Key::Esc)
            .expect("esc closes autocomplete popup");
        assert_eq!(app.mode, UiMode::Editor);
        assert!(!app.variable_autocomplete_popup.visible);

        app.handle_editor_key(&db, Key::Tab)
            .expect("tab fallback still applies variable autocomplete");
        assert_eq!(app.lines[2], "total cost");
        assert_eq!(app.status, "autocomplete: total cost");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn autocomplete_popup_closes_on_cursor_movement() {
        let (db, mut app, path) = app_with_note("total cost := 10\ntot");
        app.cursor_line = 1;
        app.cursor_col = line_char_len(app.current_line());
        app.refresh_variable_autocomplete_popup();
        assert!(app.variable_autocomplete_popup.visible);

        app.handle_editor_key(&db, Key::ArrowLeft)
            .expect("left moves cursor");
        assert!(!app.variable_autocomplete_popup.visible);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn variable_autocomplete_is_disabled_when_active_note_module_is_off() {
        let (db, mut app, path) = app_with_note("total cost := 10\ntot");
        db.set_note_modules("n1", note_modules_with_variables(false))
            .expect("disable variable module");
        let note = db
            .get_note("n1")
            .expect("note lookup")
            .expect("note exists");
        app.set_active_note(&db, note).expect("activate note");
        app.mode = UiMode::Editor;
        app.cursor_line = 1;
        app.cursor_col = line_char_len(app.current_line());
        app.refresh_variable_autocomplete_popup();
        assert!(!app.variable_autocomplete_popup.visible);
        assert!(!app.calc_variables_enabled());

        app.handle_editor_key(&db, Key::Tab)
            .expect("tab falls back when variable module is off");
        assert_eq!(app.lines[1], "tot  ");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn switching_notes_refreshes_variable_module_gating_immediately() {
        let (db, mut app, path) = app_with_note("total cost := 10\ntot");
        db.set_note_modules("n1", note_modules_with_variables(false))
            .expect("disable n1 variable module");
        let note1 = db.get_note("n1").expect("n1 lookup").expect("n1 exists");
        app.set_active_note(&db, note1).expect("activate n1");
        app.mode = UiMode::Editor;
        app.cursor_line = 1;
        app.cursor_col = line_char_len(app.current_line());
        app.refresh_variable_autocomplete_popup();
        assert!(!app.variable_autocomplete_popup.visible);

        db.save_note("n2", "total cost := 10\ntot")
            .expect("save n2");
        db.set_note_modules("n2", note_modules_with_variables(true))
            .expect("enable n2 variable module");
        let note2 = db.get_note("n2").expect("n2 lookup").expect("n2 exists");
        app.set_active_note(&db, note2).expect("activate n2");
        app.mode = UiMode::Editor;
        app.cursor_line = 1;
        app.cursor_col = line_char_len(app.current_line());
        app.refresh_variable_autocomplete_popup();
        assert!(app.variable_autocomplete_popup.visible);
        assert!(app.calc_variables_enabled());

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn tab_accepts_variable_autocomplete_for_active_prefix() {
        let (db, mut app, path) = app_with_note("total cost := 10\ntot");
        app.cursor_line = 1;
        app.cursor_col = line_char_len(app.current_line());
        assert!(!app.variable_autocomplete_popup.visible);

        let hint = app
            .variable_autocomplete_status_hint()
            .expect("autocomplete hint");
        assert!(hint.contains("total cost"));

        app.handle_editor_key(&db, Key::Tab)
            .expect("tab accepts autocomplete");

        assert_eq!(app.lines[1], "total cost");
        assert_eq!(app.cursor_col, "total cost".chars().count());
        assert_eq!(app.status, "autocomplete: total cost");

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
    fn module_commands_update_and_persist_note_modules() {
        let (db, mut app, path) = app_with_note("alpha := 10\nalp");
        app.mode = UiMode::Editor;
        app.cursor_line = 1;
        app.cursor_col = line_char_len(app.current_line());
        app.refresh_variable_autocomplete_popup();
        assert!(app.variable_autocomplete_popup.visible);

        app.execute_terminal_command(&db, "module status");
        assert_eq!(app.status, "modules math=on table=on variables=on style=on");

        app.execute_terminal_command(&db, "modules variables off");
        assert_eq!(
            app.status,
            "modules math=on table=on variables=off style=on"
        );
        assert!(!app.active_note.modules.variables);
        assert!(!app.variable_autocomplete_popup.visible);
        assert!(app.calc.variable_names.is_empty());

        let persisted = db
            .get_note("n1")
            .expect("note lookup")
            .expect("note exists");
        assert!(!persisted.modules.variables);

        app.execute_terminal_command(&db, "module variables toggle");
        assert_eq!(app.status, "modules math=on table=on variables=on style=on");
        assert!(app.active_note.modules.variables);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn module_math_toggle_disables_calc_tab_path() {
        let (db, mut app, path) = app_with_note("1 + 1");
        app.mode = UiMode::Editor;
        app.cursor_line = 0;
        app.cursor_col = line_char_len(app.current_line());

        app.execute_terminal_command(&db, "module math off");
        app.handle_editor_key(&db, Key::Tab)
            .expect("tab falls back when math module is off");
        assert_eq!(app.lines[0], "1 + 1  ");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn module_table_toggle_disables_table_cursor_clamping() {
        let (db, mut app, path) = app_with_note("| aaa |     | bb  |");
        app.mode = UiMode::Editor;
        app.cursor_col = 9;

        app.execute_terminal_command(&db, "module table off");
        app.adjust_cursor();
        assert_eq!(app.cursor_col, 9);

        app.execute_terminal_command(&db, "module table on");
        app.cursor_col = 9;
        app.adjust_cursor();
        assert_eq!(app.cursor_col, 8);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn module_style_toggle_disables_enter_autoformat_rules() {
        let (db, mut app, path) = app_with_note("- [ ] task");
        app.mode = UiMode::Editor;
        app.cursor_col = line_char_len(app.current_line());

        app.execute_terminal_command(&db, "module style off");
        app.handle_editor_key(&db, Key::Enter)
            .expect("enter uses plain newline when style module is off");
        assert_eq!(app.lines, vec!["- [ ] task".to_string(), String::new()]);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn note_unprotect_command_removes_lock() {
        let (db, mut app, path) = app_with_note("top secret");

        app.execute_terminal_command(&db, "note lock pass123");
        assert_eq!(app.active_note.access_mode, NoteAccessMode::Locked);
        assert_eq!(app.status, "note locked");

        app.execute_terminal_command(&db, "note unprotect pass123");
        assert_eq!(app.active_note.access_mode, NoteAccessMode::None);
        assert_eq!(app.lines, vec!["top secret".to_string()]);
        assert_eq!(app.status, "note unprotected");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn note_unprotect_command_removes_at_rest_encryption() {
        let (db, mut app, path) = app_with_note("classified");

        app.execute_terminal_command(&db, "note encrypt enc123");
        assert_eq!(app.active_note.access_mode, NoteAccessMode::Encrypted);
        assert_eq!(app.status, "note encrypted at rest");

        app.execute_terminal_command(&db, "note unprotect enc123");
        assert_eq!(app.active_note.access_mode, NoteAccessMode::None);
        assert_eq!(app.lines, vec!["classified".to_string()]);
        assert_eq!(app.status, "note unprotected");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn note_security_aliases_accept_password_arguments() {
        let (db, mut app, path) = app_with_note("top secret");

        app.execute_terminal_command(&db, "lock-note pass123");
        assert_eq!(app.active_note.access_mode, NoteAccessMode::Locked);
        assert!(!app.active_note.is_unlocked);
        assert_eq!(app.status, "note locked");

        app.execute_terminal_command(&db, "unlock-note pass123");
        assert_eq!(app.active_note.access_mode, NoteAccessMode::Locked);
        assert!(app.active_note.is_unlocked);
        assert_eq!(app.status, "note unlocked");

        app.execute_terminal_command(&db, "encrypt-note enc123");
        assert_eq!(app.active_note.access_mode, NoteAccessMode::Encrypted);
        assert!(app.active_note.is_unlocked);
        assert_eq!(app.status, "note encrypted at rest");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn locked_notes_block_editor_mutations_and_autosave_errors() {
        let (db, mut app, path) = app_with_note("top secret");

        app.execute_terminal_command(&db, "note lock pass123");
        assert_eq!(app.active_note.access_mode, NoteAccessMode::Locked);
        assert!(!app.active_note.is_unlocked);
        assert_eq!(app.lines, vec![String::new()]);
        assert!(!app.dirty);

        app.handle_editor_key(&db, Key::Char('x'))
            .expect("locked edit should not fail");
        assert_eq!(app.lines, vec![String::new()]);
        assert!(!app.dirty);
        assert!(app.status.contains("unlock first"));

        app.dirty = true;
        app.last_edit = Instant::now() - Duration::from_millis(super::AUTOSAVE_DEBOUNCE_MS + 5);
        app.maybe_autosave(&db)
            .expect("locked autosave should not terminate loop");
        assert!(app.status.contains("unlock first"));

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
    fn switcher_enter_prompts_password_for_locked_note_and_unlocks_on_confirm() {
        let (db, mut app, path) = app_with_note("first note");
        db.save_note("n2", "second note")
            .expect("second note saved");
        db.lock_note("n2", "pass123").expect("lock second note");
        app.refresh_switcher_items(&db)
            .expect("switcher items refreshed");
        app.mode = UiMode::Editor;

        run_keys(
            &mut app,
            &db,
            &[Key::Ctrl('p'), Key::Paste("second".to_string()), Key::Enter],
        );
        assert_eq!(app.mode, UiMode::Switcher);
        assert!(app.switcher_open_confirm.is_some());
        assert_eq!(app.active_note.id, "n1");

        run_keys(
            &mut app,
            &db,
            &[Key::Paste("wrong".to_string()), Key::Enter],
        );
        assert!(app.switcher_open_confirm.is_some());
        assert_eq!(app.active_note.id, "n1");

        run_keys(
            &mut app,
            &db,
            &[Key::Paste("pass123".to_string()), Key::Enter],
        );
        assert!(app.switcher_open_confirm.is_none());
        assert_eq!(app.active_note.id, "n2");
        assert_eq!(app.mode, UiMode::Normal);
        assert_eq!(app.vim_state.mode, crate::editor_core::vim::VimMode::Normal);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn startup_with_locked_recent_note_prompts_for_password() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("n1", "first note").expect("first note saved");
        db.save_note("n2", "second note")
            .expect("second note saved");
        db.lock_note("n2", "pass123").expect("lock second note");
        let opts = TerminalOptions {
            create_new: false,
            note_id: None,
            list_only: false,
        };

        let (app, _) = TerminalApp::new_with_startup_metrics(
            &db,
            &opts,
            true,
            false,
            true,
            true,
            true,
            3,
            super::render::RenderPalette::default(),
            "%Y-%m-%d".to_string(),
            "%Y-%m-%d %H:%M".to_string(),
        )
        .expect("terminal app");

        assert_eq!(app.active_note.id, "n2");
        assert_eq!(app.mode, UiMode::Switcher);
        assert_eq!(app.status, "password required to open protected note");
        let confirm = app
            .switcher_open_confirm
            .as_ref()
            .expect("startup should request password");
        assert_eq!(confirm.note_id, "n2");
        assert_eq!(confirm.password, "");

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
        assert!(app.folds.collapsed_starts.contains(&0));
        assert_eq!(app.folds.visible_to_real, vec![0, 3, 4]);

        app.execute_terminal_command(&db, "fold");
        assert_eq!(app.status, "fold: already folded");

        app.execute_terminal_command(&db, "unfold");
        assert!(!app.folds.collapsed_starts.contains(&0));

        app.execute_terminal_command(&db, "za");
        assert!(app.folds.collapsed_starts.contains(&0));

        app.execute_terminal_command(&db, "zo");
        assert!(!app.folds.collapsed_starts.contains(&0));

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn command_bar_arrow_history_cycles_latest_commands() {
        let (db, mut app, path) = app_with_note("alpha");
        app.mode = UiMode::Normal;

        run_keys(
            &mut app,
            &db,
            &[
                Key::Char(':'),
                Key::Char('f'),
                Key::Char('o'),
                Key::Char('o'),
                Key::Enter,
            ],
        );
        run_keys(
            &mut app,
            &db,
            &[
                Key::Char(':'),
                Key::Char('b'),
                Key::Char('a'),
                Key::Char('r'),
                Key::Enter,
            ],
        );

        run_keys(&mut app, &db, &[Key::Char(':')]);
        assert_eq!(app.mode, UiMode::CommandBar);
        assert_eq!(app.command_input, "");

        run_keys(&mut app, &db, &[Key::ArrowUp]);
        assert_eq!(app.command_input, "bar");
        run_keys(&mut app, &db, &[Key::ArrowUp]);
        assert_eq!(app.command_input, "foo");
        run_keys(&mut app, &db, &[Key::ArrowUp]);
        assert_eq!(app.command_input, "bar");

        run_keys(&mut app, &db, &[Key::ArrowDown]);
        assert_eq!(app.command_input, "foo");
        run_keys(&mut app, &db, &[Key::ArrowDown]);
        assert_eq!(app.command_input, "bar");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn command_bar_tab_autocompletes_single_option_then_opens_picker_for_multiple_options() {
        let (db, mut app, path) = app_with_note("alpha");

        run_keys(
            &mut app,
            &db,
            &[Key::Ctrl('e'), Key::Char('m'), Key::Char('o'), Key::Tab],
        );
        assert_eq!(app.mode, UiMode::CommandBar);
        assert!(!app.command_completion.visible);
        assert_eq!(app.command_input, "module ");

        run_keys(&mut app, &db, &[Key::Tab]);
        assert!(app.command_completion.visible);
        assert_eq!(
            app.command_completion
                .options
                .iter()
                .map(|entry| entry.token.as_str())
                .collect::<Vec<_>>(),
            vec!["math", "status", "style", "table", "variables"]
        );
        assert_eq!(app.command_completion.selected_index, 0);

        run_keys(&mut app, &db, &[Key::Tab]);
        assert!(app.command_completion.visible);
        assert_eq!(app.command_input, "module ");
        assert_eq!(app.command_completion.selected_index, 1);

        run_keys(&mut app, &db, &[Key::Tab, Key::Tab, Key::Tab]);
        assert!(app.command_completion.visible);
        assert_eq!(app.command_input, "module ");
        assert_eq!(app.command_completion.selected_index, 4);

        run_keys(&mut app, &db, &[Key::Enter]);
        assert!(!app.command_completion.visible);
        assert_eq!(app.command_input, "module variables ");

        run_keys(&mut app, &db, &[Key::Tab]);
        assert!(app.command_completion.visible);
        assert_eq!(
            app.command_completion
                .options
                .iter()
                .map(|entry| entry.token.as_str())
                .collect::<Vec<_>>(),
            vec!["off", "on", "toggle"]
        );
        assert_eq!(app.command_completion.selected_index, 0);

        run_keys(&mut app, &db, &[Key::Tab]);
        assert!(app.command_completion.visible);
        assert_eq!(app.command_input, "module variables ");
        assert_eq!(app.command_completion.selected_index, 1);

        run_keys(&mut app, &db, &[Key::Enter]);
        assert!(!app.command_completion.visible);
        assert_eq!(app.command_input, "module variables on");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn command_bar_enter_accepts_picker_selection_before_execute() {
        let (db, mut app, path) = app_with_note("alpha");

        run_keys(
            &mut app,
            &db,
            &[
                Key::Ctrl('e'),
                Key::Char('m'),
                Key::Char('o'),
                Key::Tab,
                Key::Tab,
                Key::Enter,
            ],
        );
        assert_eq!(app.mode, UiMode::CommandBar);
        assert_eq!(app.command_input, "module math ");
        assert!(!app.command_completion.visible);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn command_bar_ctrl_w_deletes_word_and_stays_in_command_mode() {
        let (db, mut app, path) = app_with_note("alpha");

        run_keys(
            &mut app,
            &db,
            &[Key::Ctrl('e'), Key::Paste("module table on".to_string())],
        );
        assert_eq!(app.mode, UiMode::CommandBar);
        assert_eq!(app.command_input, "module table on");

        run_keys(&mut app, &db, &[Key::Ctrl('w')]);
        assert_eq!(app.command_input, "module table ");
        assert_eq!(app.mode, UiMode::CommandBar);

        run_keys(&mut app, &db, &[Key::Ctrl('w')]);
        assert_eq!(app.command_input, "module ");
        assert_eq!(app.mode, UiMode::CommandBar);

        run_keys(&mut app, &db, &[Key::Ctrl('w')]);
        assert_eq!(app.command_input, "");
        assert_eq!(app.mode, UiMode::CommandBar);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_colon_substitute_replaces_current_line_only() {
        let (db, mut app, path) = app_with_note("alpha alpha\nalpha alpha");
        app.mode = UiMode::Normal;
        app.cursor_line = 1;
        app.cursor_col = 0;

        run_keys(
            &mut app,
            &db,
            &[
                Key::Char(':'),
                Key::Char('s'),
                Key::Char('/'),
                Key::Char('a'),
                Key::Char('l'),
                Key::Char('p'),
                Key::Char('h'),
                Key::Char('a'),
                Key::Char('/'),
                Key::Char('o'),
                Key::Char('m'),
                Key::Char('e'),
                Key::Char('g'),
                Key::Char('a'),
                Key::Char('/'),
                Key::Enter,
            ],
        );

        assert_eq!(
            app.lines,
            vec!["alpha alpha".to_string(), "omega alpha".to_string()]
        );
        assert_eq!(app.mode, UiMode::Normal);
        assert_eq!(app.status, "1 substitution on 1 line");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_colon_percent_substitute_global_replaces_whole_document() {
        let (db, mut app, path) = app_with_note("alpha alpha\nalpha alpha");
        app.mode = UiMode::Normal;

        run_keys(
            &mut app,
            &db,
            &[
                Key::Char(':'),
                Key::Char('%'),
                Key::Char('s'),
                Key::Char('/'),
                Key::Char('a'),
                Key::Char('l'),
                Key::Char('p'),
                Key::Char('h'),
                Key::Char('a'),
                Key::Char('/'),
                Key::Char('o'),
                Key::Char('m'),
                Key::Char('e'),
                Key::Char('g'),
                Key::Char('a'),
                Key::Char('/'),
                Key::Char('g'),
                Key::Enter,
            ],
        );

        assert_eq!(
            app.lines,
            vec!["omega omega".to_string(), "omega omega".to_string()]
        );
        assert_eq!(app.mode, UiMode::Normal);
        assert_eq!(app.status, "4 substitutions on 2 lines");

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
    fn visual_mode_supports_counted_navigation_and_doc_motions() {
        let body = (1..=40)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (db, mut app, path) = app_with_note(&body);
        app.mode = UiMode::Normal;

        run_keys(
            &mut app,
            &db,
            &[
                Key::Char('v'),
                Key::Char('1'),
                Key::Char('0'),
                Key::Char('j'),
            ],
        );
        assert_eq!(app.mode, UiMode::Visual);
        assert_eq!(app.cursor_line, 10);

        run_keys(
            &mut app,
            &db,
            &[Key::Char('3'), Key::Char('0'), Key::Char('k')],
        );
        assert_eq!(app.cursor_line, 0);

        run_keys(&mut app, &db, &[Key::Char('$')]);
        assert_eq!(
            app.cursor_col,
            line_char_len(app.current_line()).saturating_sub(1)
        );

        run_keys(&mut app, &db, &[Key::Char('G')]);
        assert_eq!(app.cursor_line, app.lines.len().saturating_sub(1));

        run_keys(&mut app, &db, &[Key::Char('g'), Key::Char('g')]);
        assert_eq!(app.cursor_line, 0);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn visual_line_mode_supports_counted_gg_and_g() {
        let body = (1..=40)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (db, mut app, path) = app_with_note(&body);
        app.mode = UiMode::Normal;

        run_keys(
            &mut app,
            &db,
            &[
                Key::Char('V'),
                Key::Char('3'),
                Key::Char('g'),
                Key::Char('g'),
            ],
        );
        assert_eq!(app.mode, UiMode::VisualLine);
        assert_eq!(app.cursor_line, 2);

        run_keys(
            &mut app,
            &db,
            &[Key::Char('3'), Key::Char('0'), Key::Char('G')],
        );
        assert_eq!(app.cursor_line, 29);

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
    fn move_cursor_left_word_clamps_empty_line_cursor_without_underflow() {
        let (db, mut app, path) = app_with_note("alpha\n\nbeta");
        app.mode = UiMode::Normal;
        app.cursor_line = 1;
        app.cursor_col = 4;

        app.move_cursor_left_word();

        assert_eq!(app.cursor_line, 1);
        assert_eq!(app.cursor_col, 0);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn counted_yank_word_forward_advances_across_words() {
        let (db, mut app, path) = app_with_note("one two three");
        app.mode = UiMode::Normal;
        app.cursor_col = 0;

        run_keys(
            &mut app,
            &db,
            &[Key::Char('2'), Key::Char('y'), Key::Char('w')],
        );

        assert_eq!(app.clipboard, vec!["one ".to_string(), "two ".to_string()]);
        assert_eq!(app.cursor_col, 0);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_dw_deletes_across_newline_when_motion_crosses_lines() {
        let (db, mut app, path) = app_with_note("alpha\nbeta");
        app.mode = UiMode::Normal;
        app.cursor_line = 0;
        app.cursor_col = line_char_len(app.current_line());

        run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('w')]);

        assert_eq!(app.lines, vec!["alphabeta".to_string()]);
        assert_eq!(app.cursor_line, 0);
        assert_eq!(app.cursor_col, 5);

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    #[test]
    fn vim_yw_yanks_across_newline_when_motion_crosses_lines() {
        let (db, mut app, path) = app_with_note("alpha\nbeta");
        app.mode = UiMode::Normal;
        app.cursor_line = 0;
        app.cursor_col = line_char_len(app.current_line());

        run_keys(&mut app, &db, &[Key::Char('y'), Key::Char('w')]);

        assert_eq!(app.lines, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(app.clipboard, vec!["\n".to_string()]);
        assert_eq!(app.cursor_line, 0);
        assert_eq!(app.cursor_col, 5);

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
