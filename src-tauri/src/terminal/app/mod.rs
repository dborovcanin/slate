use super::adapter::TerminalVimAdapter;
use super::ansi::{
    contrast_fg_for_bg, draw_box_border, draw_row_at_styled, goto, pad_right, AnsiStyle,
};
use super::calc_cache::CalcCache;
use super::clipboard::{self, ClipboardWriteBackend};
use super::date_picker::DatePickerAction;
use super::folding::FoldKind;
use super::folding_state::FoldingState;
use super::history::LineHistory;
use super::input::{self, Key, TerminalGuard};
use super::render;
use super::switcher::{self, NoteMeta};
use super::text_utils::*;

use crate::config::ThemeConfig;
use crate::startup_log::append_startup_log_line;
use crate::storage::{Db, Note};
use app_core::calc::CalcEngine;
use app_core::storage::{NoteAccessMode, NoteModules};
use std::cmp::min;
use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use ulid::Ulid;

const AUTOSAVE_DEBOUNCE_MS: u64 = 500;
const CALC_RECOMPUTE_DEBOUNCE_MS: u64 = 90;
const CALC_ASYNC_MIN_LINES: usize = 2_000;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VimPipelineResult {
    NoIntent,
    Unhandled,
    Applied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VimRegisterMode {
    Charwise,
    Linewise,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VimRegister {
    text: String,
    mode: VimRegisterMode,
}

impl Default for VimRegister {
    fn default() -> Self {
        Self {
            text: String::new(),
            mode: VimRegisterMode::Charwise,
        }
    }
}

impl VimRegister {
    fn charwise(text: String) -> Self {
        Self {
            text,
            mode: VimRegisterMode::Charwise,
        }
    }

    fn linewise(text: String) -> Self {
        Self {
            text,
            mode: VimRegisterMode::Linewise,
        }
    }

    fn is_empty(&self) -> bool {
        self.mode == VimRegisterMode::Charwise && self.text.is_empty()
    }
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
    clipboard: VimRegister,
    last_clipboard_backend: Option<ClipboardWriteBackend>,
    selection_anchor: Option<(usize, usize)>, // (line, col)
    command_selection: Option<crate::editor_core::types::SelectionSnapshot>,
    command_selection_linewise: bool,
    // Calc ghost cache
    calc: CalcCache,
    calc_recompute_pending: bool,
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
    last_drawn_rows: Vec<String>,
    last_drawn_rows_dim: (usize, usize),
    last_cursor_row: usize,
    last_cursor_col: usize,
    last_cursor_block: bool,
}

mod calc_helpers;
mod command_search_switcher;
mod editing;
mod input_modes;
mod reminder_helpers;
mod rendering;
mod table_helpers;
#[cfg(test)]
mod tests;
mod vim_actions;

use calc_helpers::*;
use reminder_helpers::*;
use table_helpers::*;

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
                | crate::editor_core::vim::VimIntent::DeleteWordEnd
                | crate::editor_core::vim::VimIntent::DeleteVisualSelection
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

        let line_metadata = if skip_initial_calc {
            Vec::new()
        } else {
            crate::editor_core::calc_plan::line_metadata_for_lines(&lines)
        };
        let line_has_fold_structure = lines
            .iter()
            .map(|line| Self::line_has_fold_structure(line))
            .collect::<Vec<_>>();
        let line_text_snapshot = lines.clone();
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
            clipboard: VimRegister::default(),
            last_clipboard_backend: None,
            selection_anchor: None,
            command_selection: None,
            command_selection_linewise: false,
            calc: CalcCache {
                engine: calc_engine,
                results: calc_data.line_results,
                cell_results: calc_data.cell_results,
                variable_names: calc_data.variable_names,
                line_metadata: line_metadata.clone(),
                prev_line_metadata: line_metadata,
                stale: false,
                cached_has_builtin_formula: initial_has_builtin_formula,
                cached_has_variable_assignment: initial_has_variable_assignment,
            },
            calc_recompute_pending: false,
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
            folds: FoldingState::empty(line_has_fold_structure, line_text_snapshot),
            command_bar_from_normal: false,
            clipboard_watch_enabled: false,
            clipboard_watch_last_text: None,
            clipboard_watch_last_poll: Instant::now(),
            history,
            fence_checkpoints: vec![(false, None)],
            fence_checkpoints_valid_through: 0,
            draw_buf: String::new(),
            last_drawn_rows: Vec::new(),
            last_drawn_rows_dim: (0, 0),
            last_cursor_row: 0,
            last_cursor_col: 0,
            last_cursor_block: false,
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
        self.maybe_recompute_calc_after_idle();
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
