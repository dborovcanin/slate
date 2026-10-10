use note_session::{reminder_marks_of, LineReminderGhost, ReminderMarks, ReminderUndoEntry};
mod currency;
mod scripts;
use super::adapter::TerminalVimAdapter;
use super::calc_cache::CalcCache;
use super::canvas::{contrast_fg_for_bg, draw_framed_surface, draw_row_at_styled, TextStyle};
use super::clipboard::{self, ClipboardWriteBackend};
use super::date_picker::DatePickerAction;
use super::folding::FoldKind;
use super::folding_state::FoldingState;
use super::input::{self, Key};
use super::render;
use super::session::TerminalSession;
use super::switcher::{self, CollectionMeta, NoteMeta};
use super::text_utils::*;
use crate::editor_core::completion::VariableAutocompleteState;
use crate::editor_core::history::policy::UndoAction;
use crate::editor_core::history::LineHistory;
use crate::editor_core::vim_actions::{VimRegisterMode, VimRegisterValue as VimRegister};

use crate::config::ThemeConfig;
use crate::startup_log::append_startup_log_line;
use crate::storage::{Db, Note};
use app_core::calc::CalcEngine;
use app_core::cross_note::CrossNoteVarIndex;
use app_core::storage::{NoteAccessMode, NoteModules, NoteSearchResult};
use rustc_hash::FxHashMap;
use std::cmp::min;
use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use ulid::Ulid;

const AUTOSAVE_DEBOUNCE_MS: u64 = 500;
/// Longest a pending repaint waits while queued keys are handled first, so a
/// key burst (held key, wheel scroll) paints once per frame, not once per key.
const MAX_FRAME_DEFER: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Copy, Default)]
enum BackupAnimOp {
    #[default]
    Export,
    Load,
}

enum BackupThreadResult {
    ExportDone(Result<String, String>),
    LoadStageDone(Result<(), String>),
}
const CALC_RECOMPUTE_DEBOUNCE_MS: u64 = 90;
const CALC_RECOMPUTE_PENDING_RETRY_MS: u64 = 35;
const CALC_IDLE_EVAL_BUDGET_MS: u64 = 6;
const CALC_ASYNC_MIN_LINES: usize = 2_000;
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
/// Viewport notes this long prepare calc off the input thread when opened;
/// below it the preparation takes a few milliseconds and runs inline.
#[cfg(not(test))]
const CALC_BACKGROUND_PREPARE_MIN_LINES: usize = 20_000;
/// Lower in tests so the background path runs on notes debug builds handle
/// quickly; it stays above the 2,500-line viewport notes other tests use.
#[cfg(test)]
const CALC_BACKGROUND_PREPARE_MIN_LINES: usize = 3_000;
const CALC_VIEWPORT_PREFETCH_MULTIPLIER: usize = 2;
const VARIABLE_AUTOCOMPLETE_MAX_SUGGESTIONS: usize = 3;
const WIKI_LINK_AUTOCOMPLETE_MAX_VISIBLE: usize = 16;
const COMMAND_COMPLETION_MAX_OPTIONS: usize = 32;
const CONTENT_SEARCH_DEBOUNCE_MS: u64 = 120;
const CONTENT_SEARCH_MAX_DETACHED_WORKERS: usize = 2;
const SEARCH_PREWARM_QUERY: &str = "slatewarmup";
const STARTUP_PREWARM_IDLE_BUDGET_MS: u64 = 4;
const WIKI_LINK_RENDER_CACHE_MAX_ENTRIES: usize = 512;
const WIKI_LINK_RENDER_CACHE_TTL_MS: u64 = 5 * 60 * 1000;
const WIKI_LINK_LINE_RENDER_CACHE_MAX_ENTRIES: usize = 256;
const WIKI_LINK_LINE_RENDER_CACHE_TTL_MS: u64 = 60 * 1000;
const TABLE_FORMULA_SEGMENT_CACHE_MAX_ENTRIES: usize = 512;
const TABLE_FORMULA_SEGMENT_CACHE_TTL_MS: u64 = 90 * 1000;
// Checkpoint every N lines for fence-state lookups in draw().
// Keeps the per-draw scan to at most INTERVAL line advances.
const FENCE_CHECKPOINT_INTERVAL: usize = 256;
const LARGE_NOTE_FULL_FEATURE_LINE_LIMIT: usize = 30_000;
const LARGE_NOTE_REDUCED_UNDO_LINES: usize = LARGE_NOTE_FULL_FEATURE_LINE_LIMIT + 1;

type ContentSearchResponse = (String, Result<Vec<NoteSearchResult>, String>);
type BrowserSearchResponse = (String, Result<Vec<super::browser::SearchHit>, String>);

fn decimal_digit_count(mut value: usize) -> usize {
    let mut digits = 1usize;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

fn gutter_width_for_visible_lines(visible_lines: usize) -> usize {
    // A 4-digit gutter (+2 spaces) that widens once line numbers
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

fn build_history_for_note(
    lines: &[String],
    cursor_line: usize,
    cursor_col: usize,
    reminders: ReminderMarks,
) -> LineHistory<ReminderMarks> {
    LineHistory::new(
        history_max_entries_for_line_count(lines.len()),
        lines,
        cursor_line,
        cursor_col,
        reminders,
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalOptions {
    pub create_new: bool,
    pub note_id: Option<String>,
    pub list_only: bool,
    /// Open the note switcher immediately on startup instead of the last-used note.
    pub open_switcher: bool,
    /// Start with the cursor at the end of the note (daily notes).
    pub open_at_end: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Editor,
    Normal,
    Visual,
    VisualLine,
    Switcher,
    CollectionSwitcher,
    Browser,
    ContentSearch,
    CommandBar,
    Search,
    WebSearch,
    DatePicker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VimPipelineResult {
    NoIntent,
    Unhandled,
    Applied {
        doc_mutated: bool,
        // True when the resolved motion was a vertical line move (j/k/arrows ->
        // MoveUp/MoveDown). Lets the caller keep the desired text column instead
        // of snapping to a table cell, without inspecting the raw key.
        preserve_vertical_column: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VimMacroStep {
    Action(crate::editor_core::vim::VimAction),
    InsertKey(crate::editor_core::vim::VimKey),
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
struct SwitcherDeleteConfirm {
    note_id: String,
    note_title: String,
    requires_password: bool,
    access_mode: NoteAccessMode,
    password: String,
}

#[derive(Debug, Clone)]
struct SwitcherOpenConfirm {
    note_id: String,
    note_title: String,
    /// Encrypted collection whose password unlocks the note.
    collection: Option<String>,
    access_mode: NoteAccessMode,
    password: String,
    line_number: Option<usize>,
}

/// Masked password dialog for `:note encrypt` and `:note decrypt`, so a
/// password is never typed on the command line. Encrypting asks twice.
#[derive(Debug, Clone)]
struct NotePasswordDialog {
    action: crate::editor_core::command_catalog::NoteSecurityAction,
    /// The first entry while it is repeated.
    first: Option<String>,
    password: String,
}

#[derive(Debug, Clone)]
struct CollectionEditDialogState {
    collection_id: String,
    selected_field: usize,
    name: String,
    description: String,
    default_tags: String,
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
    pub note_id: String,
    pub title: String,
    pub title_lower: String,
    pub heading: Option<String>,
}

#[derive(Debug, Default)]
struct WikiLinkPreviewState {
    visible: bool,
    title: String,
    body: String,
}

#[derive(Debug, Clone)]
struct ImagePreviewState {
    src: String,
    alt: String,
}

#[derive(Debug, Clone, Default)]
struct WikiLinkAutocompletePopupState {
    visible: bool,
    anchor_row: usize,
    anchor_col: usize,
    from_col: usize,
    query: String,
    pending_heading_note_id: Option<String>,
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
struct WikiLinkIndexEntry {
    title: String,
    updated_at: String,
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

/// Render-time caches pulled out of the `TerminalApp` monolith. These have only
/// render-path read/write sites and don't cross subsystem boundaries, so they
/// group cleanly. Each `*_cache` map has a parallel `*_order` `VecDeque` giving
/// O(1) FIFO eviction once the cap is hit.
#[derive(Default)]
struct RenderCaches {
    wiki_link_render_cache: FxHashMap<String, WikiLinkRenderCacheEntry>,
    wiki_link_render_cache_order: VecDeque<String>,
    wiki_link_line_render_cache: FxHashMap<String, WikiLinkLineRenderCacheEntry>,
    wiki_link_line_render_cache_order: VecDeque<String>,
    table_formula_segment_cache: FxHashMap<String, TableFormulaSegmentCacheEntry>,
    table_formula_segment_cache_order: VecDeque<String>,
    /// Table block start line → incrementally updated layout, so an edit
    /// re-measures only the changed rows. Bounded by `TABLE_LAYOUT_CACHE_CAP`.
    table_layout_cache: FxHashMap<usize, crate::editor_core::table::TableLayoutCache>,
    /// Layouts resolved during the current render pass (`Some` only inside
    /// `render_to_buffer`, where lines cannot change): each visible table
    /// block is bounded and hashed once per frame, not once per line.
    table_layout_pass: Option<Vec<crate::editor_core::table::TableBlockLayout>>,
}

/// Background-backup thread handle + the status-bar dot animation that runs
/// while an export/load is in flight.
#[derive(Default)]
struct BackupState {
    rx: Option<mpsc::Receiver<BackupThreadResult>>,
    anim_op: BackupAnimOp,
    anim_dots: u8,
    anim_last_tick: Option<Instant>,
}

/// Clipboard-watch polling state (the `:clipwatch` feature). Self-contained;
/// no `Default` because `last_poll` seeds from `Instant::now()`.
struct ClipboardWatch {
    enabled: bool,
    last_text: Option<String>,
    last_poll: Instant,
}

/// Date-picker overlay state (`:date` insert flow). Modal/input-and-render only;
/// no hot-path or partial-borrow coupling with `lines`/`calc`. `format` and
/// `time_format` are seeded from config at construction; the rest reset to the
/// `Default` below each time the picker opens.
struct DatePickerState {
    year: i32,
    month: u32,  // 1-12
    day: u32,    // 1-31
    hour: u32,   // 0-23
    minute: u32, // 0-59
    include_time: bool,
    require_time: bool,
    action: DatePickerAction,
    return_mode: UiMode,
    format: String,
    time_format: String,
}

impl Default for DatePickerState {
    fn default() -> Self {
        Self {
            year: 0,
            month: 0,
            day: 0,
            hour: 0,
            minute: 0,
            include_time: false,
            require_time: false,
            action: DatePickerAction::InsertDate,
            return_mode: UiMode::Editor,
            format: String::new(),
            time_format: String::new(),
        }
    }
}

/// An autosave running on a background thread.
struct BackgroundSave {
    /// The new revision, and whether the text was saved (or only reminders).
    rx: mpsc::Receiver<Result<(app_core::storage::NoteRevision, bool), String>>,
    /// The reminder version stored with it, if any.
    reminders_generation: Option<u64>,
    note_id: String,
    /// `last_edit` when the saved text was taken: later edits keep the note
    /// dirty once the save lands.
    edit_mark: u64,
    /// Revision the save was checked against. When the note's revision has
    /// moved on meanwhile (e.g. a module change), the saved one is stale.
    expected_revision: String,
}

/// Calc recompute scheduling/runtime flags (distinct from `calc: CalcCache`,
/// which holds the ghost results). These coordinate the debounced viewport/full
/// eval passes; all written on the edit hot path but read/written independently
/// of the cache, so they split cleanly off the monolith.
#[derive(Default)]
struct CalcRuntime {
    recompute_pending: bool,
    recompute_due_at: Option<Instant>,
    pending_viewport_pass: bool,
    pending_full_pass: bool,
    viewport_only: bool,
    last_view_eval_range: Option<(usize, usize)>,
    /// Viewport evaluation skipped syncing the dependency index (it only
    /// feeds variable names there); the idle tick catches it up.
    index_sync_pending: bool,
}

/// Draw-output and render-bookkeeping state: the reused draw buffer, the
/// last-frame diff snapshot, cached cursor placement, the fence-state
/// checkpoints used during `draw()`, and the file-render flags. Render-path
/// only; disjoint from the document/calc fields it reads.
/// Identifies a visual selection for `RenderState::selection_stats`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionStatsKey {
    linewise: bool,
    anchor: (usize, usize),
    cursor: (usize, usize),
    edit_seq: u64,
}

struct RenderState {
    /// Soft-wrap long lines (tables and code stay horizontally scrolled).
    wrap_lines: bool,
    /// Screen cell (1-based row, col) of the editor cursor in the last frame.
    editor_cursor_cell: Option<(usize, usize)>,
    /// Number statistics for the current visual selection, keyed by the
    /// selection and the last edit so large selections are scanned once.
    selection_stats: Option<(
        SelectionStatsKey,
        Option<crate::editor_core::sum::NumberStats>,
    )>,
    /// Status message currently displayed and when it first appeared; the
    /// editor status bar hides it after `STATUS_MESSAGE_TTL`.
    status_shown: Option<(String, Instant)>,
    /// Whether the last drawn frame showed the status message.
    status_visible: bool,
    plain_text_file: bool,
    file_language: Option<String>,
    /// Fence state checkpoints for draw(). Entry k = fence state BEFORE line
    /// k * FENCE_CHECKPOINT_INTERVAL. `fence_checkpoints_valid_through` is the
    /// highest index whose entry is current; all higher indices are stale.
    fence_checkpoints: Vec<crate::editor_core::markdown_tokens::FenceState>,
    fence_checkpoints_valid_through: usize,
    dirty: bool,
}

/// Terminal viewport and cursor-dependent presentation state.
#[derive(Default)]
struct ViewState {
    scroll_line: usize,
    /// Rows of the soft-wrapped top line scrolled past.
    scroll_row_offset: usize,
    scroll_col: usize,
    markdown_formatting_right_boundary_exit: Option<(usize, usize)>,
}

/// Note-switcher overlay state: the fuzzy query + result list, the reused
/// scoring scratch buffer, selection cursor, open/delete confirmation prompts,
/// the active collection filter, and the prewarm / title-refresh flags.
#[derive(Default)]
struct SwitcherState {
    query: String,
    items: Vec<NoteMeta>,
    matches: Vec<usize>,
    score_scratch: Vec<(usize, i32)>,
    /// Match picked with the arrows; editing the query clears it.
    selected: Option<usize>,
    open_confirm: Option<SwitcherOpenConfirm>,
    delete_confirm: Option<SwitcherDeleteConfirm>,
    collection_filter_id: Option<String>,
    collection_filter_name: Option<String>,
    prewarm_pending: bool,
    needs_title_refresh: bool,
}

/// Collection-switcher overlay state: the fuzzy query + collection list,
/// selection cursor, and the inline collection edit dialog.
#[derive(Default)]
struct CollectionSwitcherState {
    query: String,
    items: Vec<CollectionMeta>,
    /// `items` with note counts, for drawing.
    entries: Vec<super::browser::CollectionEntry>,
    matches: Vec<usize>,
    selected: usize,
    edit_dialog: Option<CollectionEditDialogState>,
}

/// Content-search overlay state: cross-note full-text search query/results,
/// the async worker receivers (primary + detached), debounce timer, and the
/// optional collection filter scoping the search.
#[derive(Default)]
struct ContentSearchState {
    query: String,
    cursor_col: usize,
    results: Vec<NoteSearchResult>,
    /// Result picked with the arrows; editing the query clears it.
    selected: Option<usize>,
    pending: bool,
    debounce_until: Option<Instant>,
    rx: Option<std::sync::mpsc::Receiver<ContentSearchResponse>>,
    detached_rxs: Vec<std::sync::mpsc::Receiver<ContentSearchResponse>>,
    collection_filter_id: Option<String>,
    collection_filter_name: Option<String>,
}

/// In-note `/`-search overlay state: the query, the match spans, the current
/// match cursor, and the pre-search origin used to restore position on cancel.
#[derive(Default)]
struct SearchState {
    query: String,
    /// Char index of the query cursor; `usize::MAX` means at the end.
    cursor: usize,
    matches: Vec<(usize, usize, usize)>, // (line_idx, start_col, end_col)
    current: usize,
    orig_line: usize,
    orig_col: usize,
    orig_scroll: usize,
}

/// Web search overlay state: the user query, async result receiver, fetched
/// results, and the selected result index.
#[derive(Default)]
pub(super) struct WebSearchState {
    pub(super) query: String,
    pub(super) cursor_col: usize,
    pub(super) results: Vec<app_core::web_search::WebSearchItem>,
    pub(super) answer: Option<String>,
    pub(super) summary: Option<String>,
    pub(super) answer_card: Option<app_core::web_search::WebSearchAnswerCard>,
    pub(super) selected: usize,
    pub(super) pending: bool,
    pub(super) error: Option<String>,
    pub(super) rx: Option<std::sync::mpsc::Receiver<WebSearchResponse>>,
}

pub(super) struct WebSearchResponse {
    pub(super) result: Result<app_core::web_search::WebSearchResult, String>,
}

struct TerminalApp {
    session: note_session::NoteSession,
    pending_session_edit: bool,
    active_note: Note,
    /// Encrypted collection whose password unlocks the open note.
    active_note_key_collection: Option<String>,
    // Shared document text, cache, cursor and selection; terminal view is separate.
    editor: note_session::Document,
    view: ViewState,
    scripts: scripts::ScriptState,
    currency: currency::CurrencyState,
    mode: UiMode,
    vim_enabled: bool,
    /// Keys being handled (nested for macro replay). While above zero, edits
    /// set `calc_recompute_after_key` instead of recomputing, so a key that
    /// edits several times (text, then autoformat) recomputes once.
    key_depth: usize,
    calc_recompute_after_key: bool,
    // Note switcher overlay
    switcher: SwitcherState,
    // Collection switcher overlay
    collection_switcher: CollectionSwitcherState,
    // Collection browser (full screen) and the mode it returns to
    browser: super::browser::BrowserState,
    browser_return_mode: UiMode,
    /// Running browser content search: the query and its hits.
    browser_search_rx: Option<mpsc::Receiver<BrowserSearchResponse>>,
    browser_search_due: Option<Instant>,
    // Content-search overlay (cross-note full-text search)
    content_search: ContentSearchState,
    working_collection_id: Option<String>,
    working_collection_name: Option<String>,
    search_index_prewarm_pending: bool,
    search_index_prewarm_rx: Option<std::sync::mpsc::Receiver<bool>>,
    search_index_prewarm_started_at: Option<Instant>,
    startup_fold_hydration_pending: bool,
    startup_reminder_hydration_pending: bool,
    startup_reminder_hydration_retry_at: Option<Instant>,
    startup_reminder_hydration_retry_count: u32,
    background_tasks_enabled: bool,
    note_creation_theme: ThemeConfig,
    /// `[daily]` settings used by `:today`.
    daily_config: app_core::config::DailyNotesConfig,
    last_edit: Instant,
    status: String,
    command_input: String,
    /// Char index of the command-bar cursor; `usize::MAX` means at the end.
    command_cursor: usize,
    command_completion: CommandCompletionMenuState,
    command_history: Vec<String>,
    command_history_index: Option<usize>,
    quit: bool,
    force_quit: bool,
    /// Edit mark of a buffer whose autosave failed; autosave waits for the
    /// next edit (or an explicit save) instead of retrying in a loop.
    autosave_paused_at: Option<u64>,
    /// Edit mark of a buffer `can_leave_note` refused to leave; leaving
    /// again without editing in between discards its unsaved changes.
    leave_refused_at: Option<u64>,
    backup: BackupState,
    /// Autosave writing on a background thread, if one is in flight.
    background_save: Option<BackgroundSave>,
    // Date picker overlay
    date_picker: DatePickerState,
    // Vim state
    vim_state: crate::editor_core::vim::VimState,
    vim_macro_recording: Option<char>,
    vim_macro_registers: FxHashMap<char, Vec<VimMacroStep>>,
    vim_macro_replaying: bool,
    clipboard: VimRegister,
    last_clipboard_backend: Option<ClipboardWriteBackend>,
    command_selection: Option<crate::editor_core::types::SelectionSnapshot>,
    command_selection_linewise: bool,
    // Shared cross-note variable index (also held by AppCore).
    cross_note_var_index: Arc<Mutex<CrossNoteVarIndex>>,
    // Notified by background dep-eval threads when mark_full_eval_attempted fires.
    // Allows preload_cross_note_deps to park instead of spin-sleep.
    cross_note_eval_condvar: Arc<Condvar>,
    // DB handle for on-demand cross-note export loading (cheap Arc clone).
    cross_note_db: Db,
    // Calc ghost cache
    calc: CalcCache,
    calc_runtime: CalcRuntime,
    /// Coordinates of edits applied since the last history record, for
    /// moving reminders with them (`note_line_edit`).
    /// Bumped whenever the reminders change; equal to the persisted one when
    /// they are stored as they are.
    last_reminder_check: Instant,
    /// When the stored note was last checked for changes made outside this
    /// session (`maybe_take_outside_change`).
    outside_change_checked_at: Instant,
    /// Outside revision the unsaved buffer was warned about, so the warning
    /// is shown once per change.
    outside_change_reported: Option<String>,
    // In-note search overlay
    search: SearchState,
    // Web search overlay
    web_search: WebSearchState,
    // Auto format
    autosave_enabled: bool,
    format_on_save: bool,
    markdown_autoformat: bool,
    checklist_auto_reorder: bool,
    // Calc/variables behavior
    variable_autocomplete_min_chars: usize,
    variable_autocomplete_popup: VariableAutocompletePopupState,
    wiki_link_preview: WikiLinkPreviewState,
    image_preview: Option<ImagePreviewState>,
    /// The help popup (`:help`, `F1`), over any editing mode.
    help: Option<help::HelpState>,
    note_password_dialog: Option<NotePasswordDialog>,
    wiki_link_autocomplete_popup: WikiLinkAutocompletePopupState,
    wiki_link_note_suggestions_cache: Vec<WikiLinkSuggestion>,
    wiki_link_index: FxHashMap<String, WikiLinkIndexEntry>,
    render_caches: RenderCaches,
    table_format_cache: crate::editor_core::table::TableFormatCache,
    render_palette: render::RenderPalette,
    // Draw output + render bookkeeping (buffer, last-frame diff, fence checkpoints)
    render_state: RenderState,
    // Folding (real-line indexed, 0-based)
    folds: FoldingState,
    // Track which mode entered command bar from
    command_bar_from_normal: bool,
    // Clipboard watch
    clipboard_watch: ClipboardWatch,
    // Undo/redo
    perf_trace: PerfTraceState,
    /// Terminal graphics support for the image preview. `None` until the
    /// first preview, which queries the terminal, so startup never pays for it.
    graphics: Option<super::graphics::GraphicsContext>,
    image_renderer: super::graphics::ImageRenderer,
    open_image_temp_paths: Vec<std::path::PathBuf>,
}

mod browser;
mod calc_helpers;
mod command_search_switcher;
mod editing;
mod help;
mod input_modes;
mod picker;
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
        self.editor.lines().len() > LARGE_NOTE_FULL_FEATURE_LINE_LIMIT
    }

    fn maybe_compact_buffers_after_note_switch(&mut self) {
        // Best-effort memory trimming when switching from very large notes.
        // This doesn't guarantee RSS drops immediately (allocator-dependent),
        // but it releases large vector capacities held by app structures.
        self.editor.compact();
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
        self.session.history.compact();
    }

    pub(super) fn working_collection_status_suffix(&self) -> String {
        match self.working_collection_name.as_deref() {
            Some(name) if !name.trim().is_empty() => format!(" | {name} "),
            _ => String::new(),
        }
    }

    fn active_note_is_editable(&self) -> bool {
        self.active_note.access_mode == NoteAccessMode::None || self.active_note.is_unlocked
    }

    fn access_mode_prompt_label(mode: NoteAccessMode) -> &'static str {
        match mode {
            NoteAccessMode::None => "note",
            NoteAccessMode::Encrypted => "encrypted-at-rest note",
        }
    }

    /// The open note is encrypted and locked, for example because its unlock
    /// expired: ask for the password while editing, otherwise say so.
    fn set_locked_note_status(&mut self) {
        let editing = matches!(
            self.mode,
            UiMode::Editor | UiMode::Normal | UiMode::Visual | UiMode::VisualLine
        );
        if editing {
            self.prompt_active_note_password();
        } else if self.mode != UiMode::Switcher {
            self.status = "note is encrypted; open it to enter its password".to_string();
        }
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
                | crate::editor_core::vim::VimIntent::ChangeLine
                | crate::editor_core::vim::VimIntent::DeleteToLineStart
                | crate::editor_core::vim::VimIntent::DeleteToLineEnd
                | crate::editor_core::vim::VimIntent::DeleteChar
                | crate::editor_core::vim::VimIntent::PasteAfter
                | crate::editor_core::vim::VimIntent::PasteBefore
                | crate::editor_core::vim::VimIntent::DeleteInsideWord
                | crate::editor_core::vim::VimIntent::DeleteAroundWord
                | crate::editor_core::vim::VimIntent::DeleteInsidePipe
                | crate::editor_core::vim::VimIntent::DeleteAroundPipe
                | crate::editor_core::vim::VimIntent::DeleteInsideParen
                | crate::editor_core::vim::VimIntent::DeleteInsideBracket
                | crate::editor_core::vim::VimIntent::DeleteInsideBrace
                | crate::editor_core::vim::VimIntent::DeleteInsideDoubleQuote
                | crate::editor_core::vim::VimIntent::DeleteInsideBacktick
                | crate::editor_core::vim::VimIntent::DeleteInsideAsterisk
                | crate::editor_core::vim::VimIntent::DeleteInsideTilde
                | crate::editor_core::vim::VimIntent::DeleteInsideUnderscore
                | crate::editor_core::vim::VimIntent::DeleteAroundParen
                | crate::editor_core::vim::VimIntent::DeleteAroundBracket
                | crate::editor_core::vim::VimIntent::DeleteAroundBrace
                | crate::editor_core::vim::VimIntent::DeleteAroundDoubleQuote
                | crate::editor_core::vim::VimIntent::DeleteAroundBacktick
                | crate::editor_core::vim::VimIntent::DeleteAroundAsterisk
                | crate::editor_core::vim::VimIntent::DeleteAroundTilde
                | crate::editor_core::vim::VimIntent::DeleteAroundUnderscore
                | crate::editor_core::vim::VimIntent::DeleteWordForward
                | crate::editor_core::vim::VimIntent::DeleteWordBackward
                | crate::editor_core::vim::VimIntent::DeleteWordEnd
                | crate::editor_core::vim::VimIntent::DeleteTillChar
                | crate::editor_core::vim::VimIntent::DeleteVisualSelection
                | crate::editor_core::vim::VimIntent::Undo
                | crate::editor_core::vim::VimIntent::Redo
        )
    }

    fn require_startup_password_if_needed(&mut self, db: &Db) {
        if self.active_note.access_mode == NoteAccessMode::None || self.active_note.is_unlocked {
            return;
        }
        self.active_note_key_collection = db
            .note_key_collection_name(&self.active_note.id)
            .ok()
            .flatten();
        self.prompt_active_note_password();
    }

    /// Opens the password prompt for the open note over the note switcher.
    fn prompt_active_note_password(&mut self) {
        self.mode = UiMode::Switcher;
        self.switcher.query.clear();
        self.recompute_switcher_matches();

        // The note list may not be loaded yet (at startup).
        let mut note_title = self
            .active_note
            .pinned_title
            .clone()
            .unwrap_or_else(|| app_core::storage::ENCRYPTED_NOTE_TITLE.to_string());
        if let Some((match_idx, switcher_idx)) = self
            .switcher
            .matches
            .iter()
            .enumerate()
            .find(|(_, idx)| self.switcher.items[**idx].id == self.active_note.id)
        {
            self.switcher.selected = Some(match_idx);
            note_title = self.switcher.items[*switcher_idx].title.clone();
        }

        self.switcher.open_confirm = Some(SwitcherOpenConfirm {
            note_id: self.active_note.id.clone(),
            note_title,
            collection: self.active_note_key_collection.clone(),
            access_mode: self.active_note.access_mode,
            password: String::new(),
            line_number: None,
        });
        self.switcher.delete_confirm = None;
        self.status = format!(
            "password required to open {}",
            Self::access_mode_prompt_label(self.active_note.access_mode)
        );
    }

    fn new_with_startup_metrics(
        db: &Db,
        opts: &TerminalOptions,
        note_creation_theme: ThemeConfig,
        vim_mode: bool,
        autosave_enabled: bool,
        format_on_save: bool,
        markdown_autoformat: bool,
        checklist_auto_reorder: bool,
        variable_autocomplete_min_chars: u8,
        render_palette: render::RenderPalette,
        date_format: String,
        date_time_format: String,
        cross_note_var_index: Arc<Mutex<CrossNoteVarIndex>>,
    ) -> Result<(Self, TerminalStartupMetrics), String> {
        let startup_begin = Instant::now();

        let note_begin = Instant::now();
        let default_collection = note_creation_theme
            .default_collection
            .as_deref()
            .map(|name| db.get_collection_by_name(name))
            .transpose()?;
        let missing_collection = matches!(default_collection, Some(None));
        let working_collection = default_collection.flatten();
        let mut active_note = select_note(
            db,
            opts,
            &note_creation_theme,
            working_collection
                .as_ref()
                .map(|collection| collection.id.as_str()),
        )?;
        let (render_plain_text_file, render_file_language) =
            file_render_syntax_for_note_id(&active_note.id);
        let loading_note = note_begin.elapsed();

        let lines = split_lines(&active_note.body);
        // The body has been split into `lines`; release the contiguous copy
        // to avoid carrying ~N bytes twice for large documents.
        active_note.body = String::new();
        let background_tasks_enabled = note_creation_theme.background_tasks_enabled;
        // Keep first frame/edit available quickly; reminders are hydrated on
        // the first idle ticks after initial paint.
        let reminder_ghosts = if background_tasks_enabled {
            FxHashMap::default()
        } else {
            load_note_reminder_ghosts(db, &active_note, &lines)?
        };

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
        let active_has_expression = note_math_enabled && initial_calc_signals.has_expression;
        let active_has_builtin_formula = note_math_enabled && initial_has_builtin_formula;
        let active_has_variable_assignment =
            note_math_enabled && note_variables_enabled && initial_has_variable_assignment;
        let calc_viewport_only = note_math_enabled
            && lines.len() >= CALC_VIEWPORT_ONLY_MIN_LINES
            && active_has_variable_assignment
            && !active_has_builtin_formula;
        let skip_initial_calc = calc_viewport_only
            || (!active_has_builtin_formula
                && !active_has_variable_assignment
                && !active_has_expression);
        // Keep startup responsive for larger notes by deferring full calc
        // evaluation to the first idle ticks after initial paint.
        let defer_initial_full_calc = note_math_enabled
            && lines.len() >= CALC_ASYNC_MIN_LINES
            && active_has_builtin_formula
            && active_has_variable_assignment;
        let calc_begin = Instant::now();
        let calc_data = if skip_initial_calc || defer_initial_full_calc {
            CalcData {
                first_line: 0,
                line_results: vec![None; lines.len()],
                cell_results: vec![Vec::new(); lines.len()],
                variable_names: Default::default(),
            }
        } else {
            let extern_vars = if active_note.modules.cross_note {
                startup_cross_note_extern_vars(
                    db,
                    &calc_engine,
                    &cross_note_var_index,
                    &active_note.id,
                    &lines,
                )
            } else {
                Vec::new()
            };
            compute_calc_data(
                &calc_engine,
                &lines,
                active_has_variable_assignment,
                active_note.modules.cross_note,
                note_table_enabled,
                None,
                extern_vars,
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
        let history = build_history_for_note(&lines, 0, 0, reminder_marks_of(&reminder_ghosts));
        let initial_mode = if vim_mode {
            UiMode::Normal
        } else {
            UiMode::Editor
        };
        let perf_enabled = crate::config::load_perf_config().enabled;
        let initial_status = if missing_collection {
            format!(
                "default collection not found: {}",
                note_creation_theme
                    .default_collection
                    .as_deref()
                    .unwrap_or_default()
            )
        } else if vim_mode {
            "-- NORMAL --  |  :cmd  Ctrl+F find  Ctrl+N new  Ctrl+P notes  Ctrl+G collections  Ctrl+Q quit".to_string()
        } else {
            format!("editing {}", active_note.id)
        };

        let mut app = Self {
            session: note_session::NoteSession::new(history, reminder_ghosts),
            pending_session_edit: false,
            active_note,
            active_note_key_collection: None,
            scripts: scripts::ScriptState::new(app_core::config::load_script_config()),
            currency: Default::default(),
            editor: note_session::Document::from_lines(lines),
            view: ViewState::default(),
            mode: initial_mode,
            vim_enabled: vim_mode,
            key_depth: 0,
            calc_recompute_after_key: false,
            switcher: SwitcherState {
                items: switcher_items,
                prewarm_pending: background_tasks_enabled,
                ..Default::default()
            },
            collection_switcher: CollectionSwitcherState::default(),
            browser: super::browser::BrowserState::default(),
            browser_return_mode: UiMode::Normal,
            browser_search_rx: None,
            browser_search_due: None,
            content_search: ContentSearchState::default(),
            working_collection_id: working_collection
                .as_ref()
                .map(|collection| collection.id.clone()),
            working_collection_name: working_collection.map(|collection| collection.name),
            search_index_prewarm_pending: background_tasks_enabled,
            search_index_prewarm_rx: None,
            search_index_prewarm_started_at: None,
            startup_fold_hydration_pending: background_tasks_enabled,
            startup_reminder_hydration_pending: background_tasks_enabled,
            startup_reminder_hydration_retry_at: None,
            startup_reminder_hydration_retry_count: 0,
            background_tasks_enabled,
            note_creation_theme,
            daily_config: app_core::config::DailyNotesConfig::default(),
            last_edit: Instant::now(),
            status: initial_status,
            command_input: String::new(),
            command_cursor: usize::MAX,
            command_completion: CommandCompletionMenuState::default(),
            command_history: Vec::new(),
            command_history_index: None,
            quit: false,
            force_quit: false,
            autosave_paused_at: None,
            leave_refused_at: None,
            backup: BackupState::default(),
            background_save: None,
            date_picker: DatePickerState {
                format: date_format,
                time_format: date_time_format,
                ..DatePickerState::default()
            },
            vim_state: crate::editor_core::vim::VimState::default(),
            vim_macro_recording: None,
            vim_macro_registers: FxHashMap::default(),
            vim_macro_replaying: false,
            clipboard: VimRegister::default(),
            last_clipboard_backend: None,
            command_selection: None,
            command_selection_linewise: false,
            cross_note_var_index,
            cross_note_eval_condvar: Arc::new(Condvar::new()),
            cross_note_db: db.clone(),
            calc: CalcCache {
                engine: calc_engine,
                results: calc_data.line_results,
                cell_results: calc_data.cell_results,
                variable_names: calc_data.variable_names.into(),
                range_context: Default::default(),
                pending_result_splices: Vec::new(),
                cross_note_refs_scan: None,
                cross_note_refs_generation: None,
                index_build: None,
                range_context_build: None,
                calc_dependency_index,
                line_metadata: line_metadata.clone(),
                prev_line_metadata: line_metadata,
                stale: defer_initial_full_calc,
                cached_has_builtin_formula: initial_has_builtin_formula,
                cached_has_variable_assignment: initial_has_variable_assignment,
                cached_has_expression: initial_calc_signals.has_expression,
                pathological_window_streak: 0,
                forced_full_recompute_remaining: 0,
            },
            calc_runtime: CalcRuntime {
                recompute_pending: defer_initial_full_calc,
                recompute_due_at: None,
                pending_viewport_pass: defer_initial_full_calc,
                pending_full_pass: defer_initial_full_calc,
                viewport_only: calc_viewport_only,
                last_view_eval_range: None,
                index_sync_pending: false,
            },
            last_reminder_check: Instant::now(),
            outside_change_checked_at: Instant::now(),
            outside_change_reported: None,
            search: SearchState::default(),
            web_search: WebSearchState::default(),
            autosave_enabled,
            format_on_save,
            markdown_autoformat,
            checklist_auto_reorder,
            variable_autocomplete_min_chars: usize::from(
                variable_autocomplete_min_chars.clamp(1, 64),
            ),
            variable_autocomplete_popup: VariableAutocompletePopupState::default(),
            wiki_link_preview: WikiLinkPreviewState::default(),
            image_preview: None,
            help: None,
            note_password_dialog: None,
            wiki_link_autocomplete_popup: WikiLinkAutocompletePopupState::default(),
            wiki_link_note_suggestions_cache: Vec::new(),
            wiki_link_index: FxHashMap::default(),
            render_caches: RenderCaches::default(),
            table_format_cache: crate::editor_core::table::TableFormatCache::default(),
            render_palette,
            render_state: RenderState {
                plain_text_file: render_plain_text_file,
                file_language: render_file_language,
                fence_checkpoints: vec![Default::default()],
                fence_checkpoints_valid_through: 0,
                wrap_lines: false,
                editor_cursor_cell: None,
                selection_stats: None,
                status_shown: None,
                status_visible: false,
                dirty: true,
            },
            folds: FoldingState::empty(Vec::new(), Vec::new()),
            command_bar_from_normal: false,
            clipboard_watch: ClipboardWatch {
                enabled: false,
                last_text: None,
                last_poll: Instant::now(),
            },
            perf_trace: PerfTraceState {
                enabled: perf_enabled,
                ..PerfTraceState::default()
            },
            graphics: None,
            image_renderer: super::graphics::ImageRenderer::new(),
            open_image_temp_paths: Vec::new(),
        };

        if let Err(error) = &app.scripts.config {
            app.status = format!("script/keybinding config: {error}");
        }
        app.bootstrap_folding_for_startup();
        app.adjust_cursor();
        app.adjust_scroll();
        app.require_startup_password_if_needed(db);
        if app.calc_runtime.viewport_only && !app.start_viewport_calc_preparation() {
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
        let mut session = TerminalSession::enter()?;
        let mut last_draw = Instant::now();

        loop {
            if self.render_state.dirty
                && (last_draw.elapsed() >= MAX_FRAME_DEFER || !input::input_ready()?)
            {
                let draw_start = Instant::now();
                self.render_state.dirty = false;
                session.draw(|frame| self.render(frame, db))?;
                last_draw = Instant::now();
                self.record_perf_duration("tui.render.frame", "draw", draw_start.elapsed());
            }
            if self.quit {
                if self.force_quit || self.can_leave_note(db) {
                    break;
                }
                self.quit = false;
                self.render_state.dirty = true;
            }

            self.poll_script_result();
            self.poll_currency_refresh();
            self.poll_script_binding_timeout(db)?;
            match input::read_key()? {
                Some(key) => {
                    let handle_start = Instant::now();
                    // A failed action is reported; it never closes the editor
                    // with unsaved text.
                    if let Err(error) = self.handle_key(db, key) {
                        self.status = format!("error: {error}");
                    }
                    self.render_state.dirty = true;
                    self.record_perf_duration("tui.key.dispatch", "input", handle_start.elapsed());
                }
                None => {
                    if input::take_resize() {
                        self.render_state.dirty = true;
                    }
                    if self.status_message_expired() {
                        self.render_state.dirty = true;
                    }
                    let idle_start = Instant::now();
                    if let Err(error) = self.maybe_autosave(db) {
                        self.status = format!("error: {error}");
                    }
                    self.maybe_take_outside_change(db);
                    self.record_perf_duration(
                        "tui.idle.dispatch",
                        "autosave_tick",
                        idle_start.elapsed(),
                    );
                    // Advance the dot animation (~1s per step) while a backup op is running.
                    if self.backup.rx.is_some() {
                        let now = Instant::now();
                        let elapsed = self
                            .backup
                            .anim_last_tick
                            .map(|t| now.duration_since(t))
                            .unwrap_or(Duration::from_secs(1));
                        if elapsed >= Duration::from_millis(1000) {
                            self.backup.anim_dots = self.backup.anim_dots % 3 + 1;
                            self.backup.anim_last_tick = Some(now);
                            let label = match self.backup.anim_op {
                                BackupAnimOp::Export => "exporting backup",
                                BackupAnimOp::Load => "loading backup",
                            };
                            self.status =
                                format!("{label}{}", ".".repeat(self.backup.anim_dots as usize));
                            self.render_state.dirty = true;
                        }
                    }
                }
            }
            // Poll for backup thread completion on every iteration so the result
            // is applied promptly whether or not the user is pressing keys.
            self.maybe_finish_backup_op(db);
            self.poll_background_save(db, false);
            self.poll_viewport_calc_preparation();

            if self.image_renderer.poll() {
                self.render_state.dirty = true;
            }

            self.poll_web_search();
            self.maybe_clipboard_watch();
            self.maybe_collect_search_results(db);
            self.poll_browser_search(db);
            self.maybe_dispatch_due_reminders(db);
        }
        Ok(())
    }

    fn maybe_finish_backup_op(&mut self, db: &Db) {
        let result = match &self.backup.rx {
            Some(rx) => match rx.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.backup.rx = None;
                    self.status = "backup operation failed unexpectedly".to_string();
                    self.render_state.dirty = true;
                    return;
                }
            },
            None => return,
        };
        self.backup.rx = None;
        match result {
            BackupThreadResult::ExportDone(Ok(msg)) => {
                self.status = msg;
            }
            BackupThreadResult::ExportDone(Err(e)) => {
                self.status = format!("backup failed: {e}");
            }
            BackupThreadResult::LoadStageDone(Ok(())) => {
                self.status = match self.apply_staged_restore(db) {
                    Ok(status) => status,
                    Err(e) => {
                        crate::commands::backup::discard_staged_restore();
                        format!("backup load failed: {e}")
                    }
                };
            }
            BackupThreadResult::LoadStageDone(Err(e)) => {
                self.status = format!("backup load failed: {e}");
            }
        }
        self.render_state.dirty = true;
    }

    /// Swaps the staged backup in and reloads everything read from the
    /// replaced database. Edits made while the backup was staging are saved
    /// first, so they stay in the database the restore sets aside.
    fn apply_staged_restore(&mut self, db: &Db) -> Result<String, String> {
        self.poll_background_save(db, true);
        if self.session.dirty {
            self.save(db)?;
        }
        if !crate::commands::backup::apply_restore_in_session(db)? {
            return Err("staged file missing".to_string());
        }

        // Nothing read from the old database stays valid.
        if let Ok(mut index) = self.cross_note_var_index.lock() {
            index.reset();
        }
        self.clear_content_search_session();
        self.working_collection_id = None;
        let note = match db.get_note(&self.active_note.id)? {
            Some(note) => note,
            None => new_note_with_context(db, &self.note_creation_theme, None)?,
        };
        self.set_active_note(db, note)?;
        self.open_switcher(db)?;
        Ok(
            "backup loaded — select a note (previous notes kept as notes.db.before-restore)"
                .to_string(),
        )
    }

    fn maybe_autosave(&mut self, db: &Db) -> Result<(), String> {
        if self.background_tasks_enabled {
            self.maybe_hydrate_startup_state(db);
        }

        // Process any deferred fold recompute while the user is not typing.
        if self.folds.rescan_pending {
            self.folds.rescan_pending = false;
            self.recompute_folding_for_note_size();
            self.render_state.dirty = true;
        }
        self.poll_viewport_calc_preparation();
        let calc_was_pending = self.calc_runtime.recompute_pending;
        self.maybe_recompute_calc_after_idle();
        // One heavy calc task per tick, so input gets a turn in between.
        let calc_ran = calc_was_pending && !self.calc_runtime.recompute_pending;
        if !calc_ran {
            self.maybe_sync_calc_index_after_idle();
        }
        self.maybe_dispatch_content_search(db);
        self.maybe_prewarm_search_surfaces(db);
        self.poll_background_save(db, false);
        if !self.autosave_enabled || self.autosave_paused_at == Some(self.session.edit_seq()) {
            return Ok(());
        }
        if (self.session.dirty || self.reminders_unsaved())
            && self.last_edit.elapsed() >= Duration::from_millis(AUTOSAVE_DEBOUNCE_MS)
        {
            self.start_background_autosave(db)?;
        }
        Ok(())
    }

    /// Full fold rescan, or the cheap reset when the note is large enough
    /// that folding is off.
    fn recompute_folding_for_note_size(&mut self) {
        if self.large_note_reduced_features() {
            self.recompute_folding_if_needed(None);
        } else {
            self.recompute_folding();
        }
    }

    fn maybe_hydrate_startup_state(&mut self, db: &Db) {
        if self.startup_fold_hydration_pending {
            let started = Instant::now();
            self.recompute_folding_for_note_size();
            self.record_perf_duration(
                "tui.idle.dispatch",
                "startup_fold_hydration",
                started.elapsed(),
            );
            self.startup_fold_hydration_pending = false;
            self.render_state.dirty = true;
        }

        self.hydrate_startup_reminders(db, false);
    }

    /// Loads the open note's reminders deferred at startup. Runs on an idle
    /// tick, or before the first key (`now`) so no edit can happen before
    /// its reminders are there to move with it.
    pub(super) fn hydrate_startup_reminders(&mut self, db: &Db, now: bool) {
        if self.startup_reminder_hydration_pending {
            if let Some(retry_at) = self.startup_reminder_hydration_retry_at {
                if !now && Instant::now() < retry_at {
                    return;
                }
            }
            // Placed against the stored text: wait for unsaved edits to land.
            if self.session.dirty {
                return;
            }
            let started = Instant::now();
            match self.load_reminders(db) {
                Ok(()) => {
                    self.session.history.set_marks(self.reminder_marks());
                    self.record_perf_duration(
                        "tui.idle.dispatch",
                        "startup_reminder_hydration",
                        started.elapsed(),
                    );
                    self.startup_reminder_hydration_pending = false;
                    self.startup_reminder_hydration_retry_at = None;
                    self.startup_reminder_hydration_retry_count = 0;
                    self.render_state.dirty = true;
                }
                Err(error) => {
                    self.startup_reminder_hydration_retry_count = self
                        .startup_reminder_hydration_retry_count
                        .saturating_add(1);
                    self.startup_reminder_hydration_retry_at =
                        Some(Instant::now() + Duration::from_secs(2));
                    eprintln!(
                        "startup reminder hydration retry #{} failed: {}",
                        self.startup_reminder_hydration_retry_count, error
                    );
                }
            }
        }
    }

    fn maybe_prewarm_search_surfaces(&mut self, db: &Db) {
        if !self.background_tasks_enabled {
            return;
        }
        let tick_started = Instant::now();
        let tick_budget = Duration::from_millis(STARTUP_PREWARM_IDLE_BUDGET_MS);

        if let Some(rx) = self.search_index_prewarm_rx.take() {
            match rx.try_recv() {
                Ok(ok) => {
                    if ok {
                        if let Some(started) = self.search_index_prewarm_started_at.take() {
                            self.record_perf_duration(
                                "tui.idle.dispatch",
                                "content_search_index_prewarm",
                                started.elapsed(),
                            );
                        }
                    } else {
                        self.search_index_prewarm_started_at = None;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.search_index_prewarm_rx = Some(rx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.search_index_prewarm_started_at = None;
                }
            }
        }

        if self.switcher.prewarm_pending {
            if tick_started.elapsed() >= tick_budget {
                return;
            }
            if self.switcher.items.is_empty() {
                let started = Instant::now();
                if self.refresh_switcher_items(db).is_ok() {
                    self.record_perf_duration(
                        "tui.idle.dispatch",
                        "switcher_prewarm",
                        started.elapsed(),
                    );
                }
            }
            self.switcher.prewarm_pending = false;
        }

        if self.search_index_prewarm_pending && self.search_index_prewarm_rx.is_none() {
            if tick_started.elapsed() >= tick_budget {
                return;
            }
            let db_clone = db.clone();
            let (tx, rx) = std::sync::mpsc::channel::<bool>();
            std::thread::spawn(move || {
                let ok = db_clone
                    .search_notes_content_filtered(SEARCH_PREWARM_QUERY, 1, None)
                    .is_ok();
                let _ = tx.send(ok);
            });
            self.search_index_prewarm_rx = Some(rx);
            self.search_index_prewarm_started_at = Some(Instant::now());
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
        if !self.content_search.pending || self.content_search.rx.is_some() {
            return;
        }
        if let Some(until) = self.content_search.debounce_until {
            if Instant::now() < until {
                return;
            }
            self.content_search.debounce_until = None;
        }
        let query = self.content_search.query.trim().to_string();
        if query.is_empty() {
            self.content_search.pending = false;
            self.content_search.debounce_until = None;
            self.content_search.results.clear();
            self.content_search.selected = None;
            return;
        }

        self.content_search.pending = false;
        self.content_search.debounce_until = None;
        let (tx, rx) = std::sync::mpsc::channel();
        let search_db = db.clone();
        let collection_filter = self.content_search.collection_filter_id.clone();
        std::thread::spawn(move || {
            let result =
                search_db.search_notes_content_filtered(&query, 100, collection_filter.as_deref());
            tx.send((query, result)).ok();
        });
        self.content_search.rx = Some(rx);
    }

    fn start_clipboard_watch(&mut self) -> bool {
        if self.clipboard_watch.enabled {
            return false;
        }
        self.clipboard_watch.enabled = true;
        self.clipboard_watch.last_text = clipboard::read_clipboard_text();
        self.clipboard_watch.last_poll =
            Instant::now() - Duration::from_millis(CLIPBOARD_WATCH_POLL_MS);
        true
    }

    fn stop_clipboard_watch(&mut self) -> bool {
        if !self.clipboard_watch.enabled {
            return false;
        }
        self.clipboard_watch.enabled = false;
        true
    }

    fn maybe_collect_search_results(&mut self, db: &Db) {
        if let Some(rx) = &self.content_search.rx {
            match rx.try_recv() {
                Ok((query, Ok(results))) => {
                    if self.content_search.query.trim() == query {
                        self.content_search.results = results;
                        self.content_search.selected = None;
                    }
                    self.content_search.rx = None;
                    self.render_state.dirty = true;
                }
                Ok((_, Err(error))) => {
                    self.status = format!("content search failed: {error}");
                    self.content_search.results.clear();
                    self.content_search.rx = None;
                    self.render_state.dirty = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.content_search.rx = None;
                }
            }
        }

        self.content_search
            .detached_rxs
            .retain(|rx| matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));

        self.maybe_dispatch_content_search(db);
    }

    fn maybe_clipboard_watch(&mut self) {
        if !self.clipboard_watch.enabled {
            return;
        }
        if !self.active_note_is_editable() {
            return;
        }
        if !matches!(self.mode, UiMode::Editor | UiMode::Normal) {
            return;
        }
        if self.clipboard_watch.last_poll.elapsed() < Duration::from_millis(CLIPBOARD_WATCH_POLL_MS)
        {
            return;
        }
        self.clipboard_watch.last_poll = Instant::now();

        let Some(text) = clipboard::read_clipboard_text() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if self.clipboard_watch.last_text.as_deref() == Some(text.as_str()) {
            return;
        }

        self.clipboard_watch.last_text = Some(text.clone());
        let mut pasted = text;
        if !pasted.ends_with('\n') {
            pasted.push('\n');
        }
        self.insert_paste(&pasted);
        self.adjust_cursor();
        self.adjust_scroll();
        self.status = "clip-watch pasted".to_string();
        self.render_state.dirty = true;
    }
}

pub fn run_terminal_session(
    db: &Db,
    config: &ThemeConfig,
    opts: &TerminalOptions,
    cross_note_var_index: Arc<Mutex<CrossNoteVarIndex>>,
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

    clipboard::set_write_mode(crate::config::load_terminal_clipboard_mode());
    // Before the app evaluates the note, so cached rates apply from the start.
    let (currency, currency_problem) =
        currency::CurrencyState::start(config.background_tasks_enabled);
    let (mut app, metrics) = TerminalApp::new_with_startup_metrics(
        db,
        opts,
        config.clone(),
        config.vim_mode,
        config.autosave,
        config.format_on_save,
        config.markdown_autoformat,
        config.checklist_auto_reorder,
        config.variables_autocomplete_min_chars,
        render::RenderPalette::for_theme(&config.color_scheme, &config.accent, &config.colors),
        config.date_format.clone(),
        config.date_time_format.clone(),
        cross_note_var_index,
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

    app.render_state.wrap_lines = config.wrap;
    app.currency = currency;
    if let Some(problem) = currency_problem {
        app.status = problem;
    }
    app.daily_config = crate::config::load_daily_notes_config();
    if opts.open_at_end {
        app.editor.cursor_line = app.editor.lines().len().saturating_sub(1);
        app.editor.cursor_col = crate::terminal::text_utils::line_char_len(app.current_line());
        app.adjust_cursor();
        app.adjust_scroll();
    }
    if opts.open_switcher {
        app.open_switcher(db)?;
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

fn select_note(
    db: &Db,
    opts: &TerminalOptions,
    config: &ThemeConfig,
    working_collection_id: Option<&str>,
) -> Result<Note, String> {
    let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
    if opts.create_new {
        return new_note_with_context(db, config, working_collection_id);
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

    new_note_with_context(db, config, working_collection_id)
}

fn new_note_with_context(
    db: &Db,
    config: &ThemeConfig,
    working_collection_id: Option<&str>,
) -> Result<Note, String> {
    let id = Ulid::new().to_string();
    let default_password =
        crate::config::resolve_default_note_encryption_password(&config.security)?;
    let modules = NoteModules {
        math: config.default_modules.math,
        table: config.default_modules.table,
        variables: config.default_modules.variables,
        style: config.default_modules.style,
        cross_note: config.default_modules.cross_note,
    };
    db.create_note_with_context(
        &id,
        modules,
        default_password.as_deref(),
        working_collection_id,
    )
}
