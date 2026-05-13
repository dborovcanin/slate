use super::adapter::TerminalVimAdapter;
use super::ansi::{
    contrast_fg_for_bg, draw_framed_surface, draw_row_at_styled, goto, pad_right, AnsiStyle,
};
use super::calc_cache::CalcCache;
use super::clipboard::{self, ClipboardWriteBackend};
use super::date_picker::DatePickerAction;
use super::folding::FoldKind;
use super::folding_state::FoldingState;
use super::history::LineHistory;
use super::input::{self, Key, TerminalGuard};
use super::render;
use super::switcher::{self, CollectionMeta, NoteMeta};
use super::text_utils::*;

use crate::config::ThemeConfig;
use crate::startup_log::append_startup_log_line;
use crate::storage::{Db, Note};
use app_core::calc::CalcEngine;
use app_core::storage::{NoteAccessMode, NoteModules, NoteSearchResult};
use rustc_hash::FxHashMap;
use std::cmp::min;
use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use ulid::Ulid;

const AUTOSAVE_DEBOUNCE_MS: u64 = 500;
const CALC_RECOMPUTE_DEBOUNCE_MS: u64 = 90;
const CALC_RECOMPUTE_PENDING_RETRY_MS: u64 = 35;
const CALC_IDLE_EVAL_BUDGET_MS: u64 = 6;
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
const WIKI_LINK_AUTOCOMPLETE_MAX_VISIBLE: usize = 16;
const COMMAND_COMPLETION_MAX_OPTIONS: usize = 16;
const CONTENT_SEARCH_DEBOUNCE_MS: u64 = 120;
const CONTENT_SEARCH_MAX_DETACHED_WORKERS: usize = 2;
const SEARCH_PREWARM_QUERY: &str = "slatewarmup";
const WIKI_LINK_RENDER_CACHE_MAX_ENTRIES: usize = 2048;
const WIKI_LINK_RENDER_CACHE_TTL_MS: u64 = 5 * 60 * 1000;
const WIKI_LINK_LINE_RENDER_CACHE_MAX_ENTRIES: usize = 1024;
const WIKI_LINK_LINE_RENDER_CACHE_TTL_MS: u64 = 60 * 1000;
const TABLE_FORMULA_SEGMENT_CACHE_MAX_ENTRIES: usize = 2048;
const TABLE_FORMULA_SEGMENT_CACHE_TTL_MS: u64 = 90 * 1000;
// Checkpoint every N lines for fence-state lookups in draw().
// Keeps the per-draw scan to at most INTERVAL line advances.
const FENCE_CHECKPOINT_INTERVAL: usize = 256;
const LARGE_NOTE_FULL_FEATURE_LINE_LIMIT: usize = 30_000;
const LARGE_NOTE_REDUCED_UNDO_LINES: usize = LARGE_NOTE_FULL_FEATURE_LINE_LIMIT + 1;
const LARGE_NOTE_LIGHTWEIGHT_FOLD_LINES: usize = LARGE_NOTE_FULL_FEATURE_LINE_LIMIT + 1;

type ContentSearchResponse = (String, Result<Vec<NoteSearchResult>, String>);

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

fn file_render_syntax_for_note_id(note_id: &str) -> (bool, Option<String>) {
    let Some(path) = app_core::note_sources::markdown_file_path_from_note_id(note_id) else {
        return (false, None);
    };
    if app_core::note_sources::is_supported_markdown_path(&path) {
        return (false, None);
    }
    (
        true,
        app_core::note_sources::syntax_language_for_path(&path),
    )
}

fn history_max_entries_for_line_count(line_count: usize) -> usize {
    if line_count >= LARGE_NOTE_REDUCED_UNDO_LINES {
        128
    } else {
        MAX_UNDO_ENTRIES
    }
}

fn build_history_for_note(lines: &[String], cursor_line: usize, cursor_col: usize) -> LineHistory {
    LineHistory::new(
        history_max_entries_for_line_count(lines.len()),
        lines,
        cursor_line,
        cursor_col,
    )
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
    CollectionSwitcher,
    ContentSearch,
    CommandBar,
    Search,
    DatePicker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VimPipelineResult {
    NoIntent,
    Unhandled,
    Applied { doc_mutated: bool },
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum VimMacroStep {
    Action(crate::editor_core::vim::VimAction),
    InsertKey(crate::editor_core::vim::VimKey),
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
    line_number: Option<usize>,
}

#[derive(Debug, Clone)]
struct CollectionEditDialogState {
    collection_id: String,
    selected_field: usize,
    name: String,
    description: String,
    default_tags: String,
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

#[derive(Debug, Clone)]
pub(super) struct WikiLinkSuggestion {
    pub short_id: String,
    pub title: String,
    pub title_lower: String,
    pub heading: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct WikiLinkAutocompletePopupState {
    visible: bool,
    anchor_row: usize,
    anchor_col: usize,
    from_col: usize,
    query: String,
    pending_heading_short_id: Option<String>,
    note_suggestions: Vec<WikiLinkSuggestion>,
    heading_cache: FxHashMap<String, Vec<WikiLinkSuggestion>>,
    suggestions: Vec<WikiLinkSuggestion>,
    selected_index: usize,
    cursor_line: usize,
}

#[derive(Debug, Clone)]
struct WikiLinkRenderCacheEntry {
    display: String,
    broken: bool,
    cached_at: Instant,
}

#[derive(Debug, Clone)]
struct WikiLinkLineRenderCacheEntry {
    rendered_line: String,
    underline_ranges: Vec<(usize, usize)>,
    cached_at: Instant,
}

#[derive(Debug, Clone)]
struct TableFormulaSegmentCacheEntry {
    segments: Vec<TableFormulaSegment>,
    cached_at: Instant,
}

#[derive(Debug, Clone)]
struct WikiLinkPrefixIndexEntry {
    title: String,
    updated_at: String,
    note_id: String,
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

#[derive(Debug, Clone, Default)]
struct PerfBucket {
    samples_ms: Vec<f64>,
}

#[derive(Debug, Clone)]
struct PerfTraceState {
    enabled: bool,
    capacity: usize,
    buckets: FxHashMap<(String, String), PerfBucket>,
}

impl Default for PerfTraceState {
    fn default() -> Self {
        Self {
            enabled: false,
            capacity: 256,
            buckets: FxHashMap::default(),
        }
    }
}

struct TerminalApp {
    active_note: Note,
    lines: Vec<String>,
    joined_text_cache: Option<String>,
    cursor_line: usize,
    cursor_col: usize, // char index
    scroll_line: usize,
    scroll_col: usize,
    mode: UiMode,
    vim_enabled: bool,
    switcher_query: String,
    switcher_items: Vec<NoteMeta>,
    switcher_matches: Vec<usize>,
    switcher_selected: usize,
    switcher_open_confirm: Option<SwitcherOpenConfirm>,
    switcher_delete_confirm: Option<SwitcherDeleteConfirm>,
    collection_switcher_query: String,
    collection_switcher_items: Vec<CollectionMeta>,
    collection_switcher_matches: Vec<usize>,
    collection_switcher_selected: usize,
    collection_edit_dialog: Option<CollectionEditDialogState>,
    content_search_query: String,
    content_search_cursor_col: usize,
    content_search_results: Vec<NoteSearchResult>,
    content_search_selected: usize,
    content_search_pending: bool,
    content_search_debounce_until: Option<Instant>,
    content_search_rx: Option<std::sync::mpsc::Receiver<ContentSearchResponse>>,
    content_search_detached_rxs: Vec<std::sync::mpsc::Receiver<ContentSearchResponse>>,
    working_collection_id: Option<String>,
    working_collection_name: Option<String>,
    switcher_collection_filter_id: Option<String>,
    switcher_collection_filter_name: Option<String>,
    content_search_collection_filter_id: Option<String>,
    content_search_collection_filter_name: Option<String>,
    switcher_prewarm_pending: bool,
    search_index_prewarm_pending: bool,
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
    vim_macro_recording: Option<char>,
    vim_macro_registers: FxHashMap<char, Vec<VimMacroStep>>,
    vim_macro_replaying: bool,
    clipboard: VimRegister,
    last_clipboard_backend: Option<ClipboardWriteBackend>,
    selection_anchor: Option<(usize, usize)>, // (line, col)
    command_selection: Option<crate::editor_core::types::SelectionSnapshot>,
    command_selection_linewise: bool,
    // Calc ghost cache
    calc: CalcCache,
    calc_recompute_pending: bool,
    calc_recompute_due_at: Option<Instant>,
    calc_pending_viewport_pass: bool,
    calc_pending_full_pass: bool,
    calc_viewport_only: bool,
    calc_last_view_eval_range: Option<(usize, usize)>,
    reminder_ghosts: FxHashMap<usize, LineReminderGhost>, // 0-based line index
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
    autosave_enabled: bool,
    format_on_save: bool,
    markdown_autoformat: bool,
    checklist_auto_reorder: bool,
    // Calc/variables behavior
    variable_autocomplete_min_chars: usize,
    variable_autocomplete_popup: VariableAutocompletePopupState,
    wiki_link_autocomplete_popup: WikiLinkAutocompletePopupState,
    wiki_link_note_suggestions_cache: Vec<WikiLinkSuggestion>,
    wiki_link_prefix_index: FxHashMap<String, WikiLinkPrefixIndexEntry>,
    wiki_link_render_cache: FxHashMap<String, WikiLinkRenderCacheEntry>,
    wiki_link_line_render_cache: FxHashMap<String, WikiLinkLineRenderCacheEntry>,
    table_formula_segment_cache: FxHashMap<String, TableFormulaSegmentCacheEntry>,
    table_format_cache: crate::editor_core::table::TableFormatCache,
    render_palette: render::RenderPalette,
    render_plain_text_file: bool,
    render_file_language: Option<String>,
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
    last_draw_had_overlay: bool,
    perf_trace: PerfTraceState,
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
    fn large_note_reduced_features(&self) -> bool {
        self.lines.len() > LARGE_NOTE_FULL_FEATURE_LINE_LIMIT
    }

    fn maybe_compact_buffers_after_note_switch(&mut self) {
        // Best-effort memory trimming when switching from very large notes.
        // This doesn't guarantee RSS drops immediately (allocator-dependent),
        // but it releases large vector capacities held by app structures.
        self.lines.shrink_to_fit();
        self.calc.results.shrink_to_fit();
        self.calc.cell_results.shrink_to_fit();
        self.calc.variable_names.shrink_to_fit();
        self.calc.line_metadata.shrink_to_fit();
        self.calc.prev_line_metadata.shrink_to_fit();
        self.folds.line_has_structure.shrink_to_fit();
        self.folds.line_text_snapshot.shrink_to_fit();
        self.folds.range_by_start.shrink_to_fit();
        self.folds.visible_to_real.shrink_to_fit();
        self.folds.real_to_visible.shrink_to_fit();
        self.folds.hidden_owner.shrink_to_fit();
        self.folds.placeholder_hidden_lines.shrink_to_fit();
        self.history.compact();
    }

    pub(super) fn working_collection_status_suffix(&self) -> String {
        match self.working_collection_name.as_deref() {
            Some(name) if !name.trim().is_empty() => format!("  |  collection:{name}"),
            _ => String::new(),
        }
    }

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
                | crate::editor_core::vim::VimIntent::DeleteTillChar
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
            line_number: None,
        });
        self.switcher_delete_confirm = None;
        self.status = "password required to open protected note".to_string();
    }

    fn new_with_startup_metrics(
        db: &Db,
        opts: &TerminalOptions,
        vim_mode: bool,
        autosave_enabled: bool,
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
        let (render_plain_text_file, render_file_language) =
            file_render_syntax_for_note_id(&active_note.id);
        let loading_note = note_begin.elapsed();

        let lines = split_lines(&active_note.body);
        // The body has been split into `lines`; release the contiguous copy
        // to avoid carrying ~N bytes twice for large documents.
        active_note.body = String::new();
        let reminder_ghosts = load_note_reminder_ghosts(db, &active_note.id, &lines)?;

        // Keep startup memory lean: load switcher/wiki metadata lazily on
        // first explicit switcher/wiki-autocomplete use.
        let switcher_items = Vec::new();
        let loading_switcher = Duration::default();

        let calc_engine = CalcEngine::new();
        let note_math_enabled = active_note.modules.math;
        let note_table_enabled = active_note.modules.table;
        let note_variables_enabled = active_note.modules.variables;
        let initial_calc_signals =
            crate::editor_core::calc_plan::detect_calc_signal_flags_with_mask(
                &lines,
                crate::editor_core::calc_plan::CalcFeatureMask {
                    math_enabled: note_math_enabled,
                    table_enabled: note_table_enabled,
                    variables_enabled: note_variables_enabled,
                },
            );
        let initial_has_builtin_formula = initial_calc_signals.has_builtin_formula;
        let initial_has_variable_assignment = initial_calc_signals.has_variable_assignment;
        let active_has_builtin_formula = note_math_enabled && initial_has_builtin_formula;
        let active_has_variable_assignment =
            note_math_enabled && note_variables_enabled && initial_has_variable_assignment;
        let calc_viewport_only = note_math_enabled
            && lines.len() >= CALC_VIEWPORT_ONLY_MIN_LINES
            && active_has_variable_assignment
            && !active_has_builtin_formula;
        let skip_initial_calc =
            calc_viewport_only || (!active_has_builtin_formula && !active_has_variable_assignment);
        // Keep startup responsive for larger notes by deferring full calc
        // evaluation to the first idle ticks after initial paint.
        let defer_initial_full_calc = note_math_enabled
            && lines.len() >= CALC_ASYNC_MIN_LINES
            && active_has_builtin_formula
            && active_has_variable_assignment;
        let calc_begin = Instant::now();
        let calc_data = if skip_initial_calc || defer_initial_full_calc {
            CalcData {
                line_results: vec![None; lines.len()],
                cell_results: vec![Vec::new(); lines.len()],
                variable_names: Vec::new(),
            }
        } else {
            compute_calc_data(
                &calc_engine,
                &lines,
                active_has_variable_assignment,
                note_table_enabled,
                None,
            )
        };
        let loading_calc_engine = calc_begin.elapsed();

        let line_metadata = if skip_initial_calc || defer_initial_full_calc {
            Vec::new()
        } else {
            crate::editor_core::calc_plan::line_metadata_for_lines_with_mask(
                &lines,
                crate::editor_core::calc_plan::CalcFeatureMask {
                    math_enabled: note_math_enabled,
                    table_enabled: note_table_enabled,
                    variables_enabled: note_variables_enabled,
                },
            )
        };
        let calc_dependency_index = if skip_initial_calc || defer_initial_full_calc {
            None
        } else {
            crate::editor_core::calc_plan::build_calc_dependency_index(
                &lines,
                crate::editor_core::calc_plan::CalcFeatureMask {
                    math_enabled: note_math_enabled,
                    table_enabled: note_table_enabled,
                    variables_enabled: note_variables_enabled,
                },
            )
        };
        let history = build_history_for_note(&lines, 0, 0);
        let initial_mode = if vim_mode {
            UiMode::Normal
        } else {
            UiMode::Editor
        };
        let perf_enabled = crate::config::load_perf_config().enabled;
        let initial_status = if vim_mode {
            "-- NORMAL --  |  :cmd  Ctrl+F find  Ctrl+N new  Ctrl+P notes  Ctrl+G collections  Ctrl+Q quit".to_string()
        } else {
            format!("editing {}", active_note.id)
        };

        let mut app = Self {
            active_note,
            lines,
            joined_text_cache: None,
            cursor_line: 0,
            cursor_col: 0,
            scroll_line: 0,
            scroll_col: 0,
            mode: initial_mode,
            vim_enabled: vim_mode,
            switcher_query: String::new(),
            switcher_items,
            switcher_matches: Vec::new(),
            switcher_selected: 0,
            switcher_open_confirm: None,
            switcher_delete_confirm: None,
            collection_switcher_query: String::new(),
            collection_switcher_items: Vec::new(),
            collection_switcher_matches: Vec::new(),
            collection_switcher_selected: 0,
            collection_edit_dialog: None,
            content_search_query: String::new(),
            content_search_cursor_col: 0,
            content_search_results: Vec::new(),
            content_search_selected: 0,
            content_search_pending: false,
            content_search_debounce_until: None,
            content_search_rx: None,
            content_search_detached_rxs: Vec::new(),
            working_collection_id: None,
            working_collection_name: None,
            switcher_collection_filter_id: None,
            switcher_collection_filter_name: None,
            content_search_collection_filter_id: None,
            content_search_collection_filter_name: None,
            switcher_prewarm_pending: true,
            search_index_prewarm_pending: true,
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
            vim_macro_recording: None,
            vim_macro_registers: FxHashMap::default(),
            vim_macro_replaying: false,
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
                calc_dependency_index,
                line_metadata: line_metadata.clone(),
                prev_line_metadata: line_metadata,
                stale: defer_initial_full_calc,
                cached_has_builtin_formula: initial_has_builtin_formula,
                cached_has_variable_assignment: initial_has_variable_assignment,
                pathological_window_streak: 0,
                forced_full_recompute_remaining: 0,
            },
            calc_recompute_pending: defer_initial_full_calc,
            calc_recompute_due_at: None,
            calc_pending_viewport_pass: defer_initial_full_calc,
            calc_pending_full_pass: defer_initial_full_calc,
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
            autosave_enabled,
            format_on_save,
            markdown_autoformat,
            checklist_auto_reorder,
            variable_autocomplete_min_chars: usize::from(
                variable_autocomplete_min_chars.clamp(1, 64),
            ),
            variable_autocomplete_popup: VariableAutocompletePopupState::default(),
            wiki_link_autocomplete_popup: WikiLinkAutocompletePopupState::default(),
            wiki_link_note_suggestions_cache: Vec::new(),
            wiki_link_prefix_index: FxHashMap::default(),
            wiki_link_render_cache: FxHashMap::default(),
            wiki_link_line_render_cache: FxHashMap::default(),
            table_formula_segment_cache: FxHashMap::default(),
            table_format_cache: crate::editor_core::table::TableFormatCache::default(),
            render_palette,
            render_plain_text_file,
            render_file_language,
            folds: FoldingState::empty(Vec::new(), Vec::new()),
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
            last_draw_had_overlay: false,
            perf_trace: PerfTraceState {
                enabled: perf_enabled,
                ..PerfTraceState::default()
            },
        };

        app.bootstrap_folding_for_startup();
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
            let draw_start = Instant::now();
            self.draw(&mut stdout)?;
            self.record_perf_duration("tui.render.frame", "draw", draw_start.elapsed());
            if self.quit {
                break;
            }

            match input::read_key()? {
                Some(key) => {
                    let handle_start = Instant::now();
                    self.handle_key(db, key)?;
                    self.record_perf_duration("tui.key.dispatch", "input", handle_start.elapsed());
                }
                None => {
                    let idle_start = Instant::now();
                    self.maybe_autosave(db)?;
                    self.record_perf_duration(
                        "tui.idle.dispatch",
                        "autosave_tick",
                        idle_start.elapsed(),
                    );
                }
            }

            self.maybe_clipboard_watch();
            self.maybe_collect_search_results(db);
            self.sync_reminder_ghosts_if_dirty(db)?;
            self.maybe_dispatch_due_reminders(db);
        }

        if !self.force_quit && self.autosave_enabled {
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
        self.maybe_dispatch_content_search(db);
        self.maybe_prewarm_search_surfaces(db);
        if !self.autosave_enabled {
            return Ok(());
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

    fn maybe_prewarm_search_surfaces(&mut self, db: &Db) {
        if self.switcher_prewarm_pending {
            if self.switcher_items.is_empty() {
                let started = Instant::now();
                if self.refresh_switcher_items(db).is_ok() {
                    self.record_perf_duration(
                        "tui.idle.dispatch",
                        "switcher_prewarm",
                        started.elapsed(),
                    );
                }
            }
            self.switcher_prewarm_pending = false;
        }

        if self.search_index_prewarm_pending {
            let started = Instant::now();
            if db
                .search_notes_content_filtered(SEARCH_PREWARM_QUERY, 1, None)
                .is_ok()
            {
                self.record_perf_duration(
                    "tui.idle.dispatch",
                    "content_search_index_prewarm",
                    started.elapsed(),
                );
            }
            self.search_index_prewarm_pending = false;
        }
    }

    pub(super) fn record_perf_duration(&mut self, name: &str, reason: &str, duration: Duration) {
        if !self.perf_trace.enabled {
            return;
        }
        let key = (name.to_string(), reason.to_string());
        let bucket = self
            .perf_trace
            .buckets
            .entry(key)
            .or_insert_with(PerfBucket::default);
        let ms = duration.as_secs_f64() * 1000.0;
        bucket.samples_ms.push(ms);
        if bucket.samples_ms.len() > self.perf_trace.capacity {
            let drop = bucket.samples_ms.len() - self.perf_trace.capacity;
            bucket.samples_ms.drain(0..drop);
        }
    }

    fn perf_percentile(samples: &[f64], p: f64) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let max_idx = sorted.len().saturating_sub(1) as f64;
        let rank = (p.clamp(0.0, 100.0) / 100.0) * max_idx;
        let lo = rank.floor() as usize;
        let hi = rank.ceil() as usize;
        if lo == hi {
            return sorted[lo];
        }
        let frac = rank - lo as f64;
        sorted[lo] + (sorted[hi] - sorted[lo]) * frac
    }

    pub(super) fn perf_status_summary(&self) -> String {
        let sample_count: usize = self
            .perf_trace
            .buckets
            .values()
            .map(|bucket| bucket.samples_ms.len())
            .sum();
        format!(
            "perf {} buckets={} samples={} cap={}",
            if self.perf_trace.enabled { "on" } else { "off" },
            self.perf_trace.buckets.len(),
            sample_count,
            self.perf_trace.capacity
        )
    }

    pub(super) fn perf_dump_report(&self, top: usize) -> String {
        let mut rows = self
            .perf_trace
            .buckets
            .iter()
            .filter_map(|((name, reason), bucket)| {
                if bucket.samples_ms.is_empty() {
                    return None;
                }
                let count = bucket.samples_ms.len();
                let total: f64 = bucket.samples_ms.iter().sum();
                let avg = total / count as f64;
                let max = bucket.samples_ms.iter().copied().fold(0.0_f64, f64::max);
                let p95 = Self::perf_percentile(&bucket.samples_ms, 95.0);
                Some((name.clone(), reason.clone(), count, avg, p95, max))
            })
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
        let limit = top.max(1).min(rows.len());
        let mut out = Vec::with_capacity(limit + 1);
        out.push(format!("[tui-profiler] {}", self.perf_status_summary()));
        for (idx, (name, reason, count, avg, p95, max)) in rows.into_iter().take(limit).enumerate()
        {
            out.push(format!(
                "{}. {} reason={} count={} avg={:.2}ms p95={:.2}ms max={:.2}ms",
                idx + 1,
                name,
                reason,
                count,
                avg,
                p95,
                max
            ));
        }
        out.join("\n")
    }

    fn maybe_dispatch_content_search(&mut self, db: &Db) {
        if self.mode != UiMode::ContentSearch {
            return;
        }
        if !self.content_search_pending || self.content_search_rx.is_some() {
            return;
        }
        if let Some(until) = self.content_search_debounce_until {
            if Instant::now() < until {
                return;
            }
            self.content_search_debounce_until = None;
        }
        let query = self.content_search_query.trim().to_string();
        if query.is_empty() {
            self.content_search_pending = false;
            self.content_search_debounce_until = None;
            self.content_search_results.clear();
            self.content_search_selected = 0;
            return;
        }

        self.content_search_pending = false;
        self.content_search_debounce_until = None;
        let (tx, rx) = std::sync::mpsc::channel();
        let search_db = db.clone();
        let collection_filter = self.content_search_collection_filter_id.clone();
        std::thread::spawn(move || {
            let result =
                search_db.search_notes_content_filtered(&query, 60, collection_filter.as_deref());
            tx.send((query, result)).ok();
        });
        self.content_search_rx = Some(rx);
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

    fn maybe_collect_search_results(&mut self, db: &Db) {
        if let Some(rx) = &self.content_search_rx {
            match rx.try_recv() {
                Ok((query, Ok(results))) => {
                    if self.content_search_query.trim() == query {
                        self.content_search_results = results;
                        self.content_search_selected = 0;
                    }
                    self.content_search_rx = None;
                }
                Ok((_, Err(error))) => {
                    self.status = format!("content search failed: {error}");
                    self.content_search_results.clear();
                    self.content_search_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.content_search_rx = None;
                }
            }
        }

        self.content_search_detached_rxs
            .retain(|rx| matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));

        self.maybe_dispatch_content_search(db);
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
        config.autosave,
        config.format_on_save,
        config.markdown_autoformat,
        config.checklist_auto_reorder,
        config.variables_autocomplete_min_chars,
        render::RenderPalette::for_theme(&config.color_scheme, &config.accent),
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
    let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
    if opts.create_new {
        return new_note(db, config);
    }

    if let Some(id) = &opts.note_id {
        return note_sources
            .open_note_by_id(id)?
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
    new_note_with_context(db, config, None)
}

fn new_note_with_context(
    db: &Db,
    config: &ThemeConfig,
    working_collection_id: Option<&str>,
) -> Result<Note, String> {
    let id = Ulid::new().to_string();
    let security = crate::config::note_security_config_from_theme(config);
    let default_password = crate::config::resolve_default_note_encryption_password(&security)?;
    let modules = NoteModules {
        math: config.default_modules.math,
        table: config.default_modules.table,
        variables: config.default_modules.variables,
        style: config.default_modules.style,
    };
    db.create_note_with_context(
        &id,
        modules,
        default_password.as_deref(),
        working_collection_id,
    )
}
