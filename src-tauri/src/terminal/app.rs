use super::render;

use crate::config::ThemeConfig;
use crate::startup_log::append_startup_log_line;
use crate::storage::{Db, Note};
use app_core::calc::CalcEngine;
use base64::Engine as _;
use std::cmp::min;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::{self, IsTerminal as _, Write};
use std::mem::MaybeUninit;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use ulid::Ulid;

const AUTOSAVE_DEBOUNCE_MS: u64 = 500;
const CLIPBOARD_WATCH_POLL_MS: u64 = 350;
const FOLD_PREFIX_TIMEOUT_MS: u64 = 900;
const TITLE_ROW: usize = 1;
const EDITOR_TOP_ROW: usize = 2;
const GUTTER_WIDTH: usize = 6;
const HORIZONTAL_SCROLL_LEFT_CONTEXT: usize = 2;
const OVERFLOW_LEFT_MARKER: char = '<';
const OVERFLOW_RIGHT_MARKER: char = '>';

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardWriteBackend {
    Arboard,
    Tmux,
    WlCopy,
    Xclip,
    Xsel,
    Pbcopy,
    ClipExe,
    Osc52,
}

impl ClipboardWriteBackend {
    fn label(self) -> &'static str {
        match self {
            Self::Arboard => "native",
            Self::Tmux => "tmux",
            Self::WlCopy => "wl-copy",
            Self::Xclip => "xclip",
            Self::Xsel => "xsel",
            Self::Pbcopy => "pbcopy",
            Self::ClipExe => "clip.exe",
            Self::Osc52 => "osc52",
        }
    }
}

fn copy_text_to_clipboard(text: &str) -> Option<ClipboardWriteBackend> {
    if text.is_empty() {
        return None;
    }

    // In terminal mode prefer explicit system/terminal clipboard transports.
    // `arboard` can report success in environments where the desktop clipboard
    // is not actually reachable from this terminal session.
    if let Some(backend) = write_clipboard_via_commands(text) {
        return Some(backend);
    }

    // Fallback for terminal environments where native/system providers are
    // unavailable. Many terminals support OSC 52 copy sequences.
    if write_clipboard_via_osc52(text) {
        return Some(ClipboardWriteBackend::Osc52);
    }

    if let Ok(mut ctx) = arboard::Clipboard::new() {
        if ctx.set_text(text.to_string()).is_ok() {
            return Some(ClipboardWriteBackend::Arboard);
        }
    }

    None
}

fn write_terminal_sequence(sequence: &str) -> bool {
    if io::stdout().is_terminal() {
        let mut out = io::stdout();
        if write!(out, "{sequence}").and_then(|_| out.flush()).is_ok() {
            return true;
        }
    }

    #[cfg(unix)]
    {
        if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
            if tty
                .write_all(sequence.as_bytes())
                .and_then(|_| tty.flush())
                .is_ok()
            {
                return true;
            }
        }
    }

    false
}

fn run_clipboard_write_command(bin: &str, args: &[&str], text: &str) -> bool {
    let mut child = match Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };

    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
    } else {
        return false;
    }

    matches!(child.wait(), Ok(status) if status.success())
}

fn run_clipboard_write_command_with_arg(bin: &str, args: &[&str], text: &str) -> bool {
    matches!(
        Command::new(bin)
            .args(args)
            .arg(text)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status(),
        Ok(status) if status.success()
    )
}

fn run_clipboard_read_command(bin: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim_end_matches('\n').trim_end_matches('\r');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn write_clipboard_via_tmux(text: &str) -> bool {
    if run_clipboard_write_command("tmux", &["load-buffer", "-w", "-"], text) {
        return true;
    }
    if run_clipboard_write_command_with_arg("tmux", &["set-buffer", "-w", "--"], text) {
        return true;
    }
    if run_clipboard_write_command("tmux", &["load-buffer", "-"], text) {
        return true;
    }
    run_clipboard_write_command_with_arg("tmux", &["set-buffer", "--"], text)
}

fn write_clipboard_via_commands(text: &str) -> Option<ClipboardWriteBackend> {
    if run_clipboard_write_command("wl-copy", &[], text) {
        return Some(ClipboardWriteBackend::WlCopy);
    }
    if run_clipboard_write_command("xclip", &["-selection", "clipboard"], text) {
        return Some(ClipboardWriteBackend::Xclip);
    }
    if run_clipboard_write_command("xsel", &["--clipboard", "--input"], text) {
        return Some(ClipboardWriteBackend::Xsel);
    }
    if run_clipboard_write_command("pbcopy", &[], text) {
        return Some(ClipboardWriteBackend::Pbcopy);
    }
    if run_clipboard_write_command("clip.exe", &[], text) {
        return Some(ClipboardWriteBackend::ClipExe);
    }
    if run_clipboard_write_command("clip", &[], text) {
        return Some(ClipboardWriteBackend::ClipExe);
    }
    if std::env::var_os("TMUX").is_some() && write_clipboard_via_tmux(text) {
        return Some(ClipboardWriteBackend::Tmux);
    }

    None
}

fn build_osc52_sequence(encoded: &str, terminator: &str) -> String {
    // tmux/screen usually require DCS passthrough for OSC sequences.
    if std::env::var_os("TMUX").is_some() {
        return format!("\x1bPtmux;\x1b\x1b]52;c;{encoded}{terminator}\x1b\\");
    }
    if std::env::var_os("STY").is_some() {
        return format!("\x1bP\x1b]52;c;{encoded}{terminator}\x1b\\");
    }
    format!("\x1b]52;c;{encoded}{terminator}")
}

fn write_clipboard_via_osc52(text: &str) -> bool {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let bel = build_osc52_sequence(&encoded, "\x07");
    let st = build_osc52_sequence(&encoded, "\x1b\\");

    // Emit both BEL- and ST-terminated forms for wider terminal compatibility.
    let wrote_bel = write_terminal_sequence(&bel);
    let wrote_st = write_terminal_sequence(&st);
    wrote_bel || wrote_st
}

fn read_clipboard_via_commands() -> Option<String> {
    if let Some(text) = run_clipboard_read_command("wl-paste", &["-n"]) {
        return Some(text);
    }
    if let Some(text) = run_clipboard_read_command("xclip", &["-selection", "clipboard", "-o"]) {
        return Some(text);
    }
    if let Some(text) = run_clipboard_read_command("xsel", &["--clipboard", "--output"]) {
        return Some(text);
    }
    if let Some(text) = run_clipboard_read_command("pbpaste", &[]) {
        return Some(text);
    }
    if let Some(text) = run_clipboard_read_command(
        "powershell",
        &["-NoProfile", "-Command", "Get-Clipboard -Raw"],
    ) {
        return Some(text);
    }
    if let Some(text) =
        run_clipboard_read_command("pwsh", &["-NoProfile", "-Command", "Get-Clipboard -Raw"])
    {
        return Some(text);
    }
    if std::env::var_os("TMUX").is_some() {
        if let Some(text) = run_clipboard_read_command("tmux", &["save-buffer", "-"]) {
            return Some(text);
        }
    }
    None
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
enum DatePickerAction {
    InsertDate,
    SetNotify,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    Char(char),
    Paste(String),
    Enter,
    Backspace,
    Delete,
    Tab,
    BackTab,
    Esc,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    CtrlArrowLeft,
    CtrlArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Ctrl(char),
}

#[derive(Debug, Clone)]
struct NoteMeta {
    id: String,
    title: String,
}

const MAX_UNDO_ENTRIES: usize = 500;

#[derive(Debug, Clone)]
struct UndoEntry {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FoldKind {
    Heading,
    Fence,
    List,
    Table,
    Paragraph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FoldRange {
    start_line: usize,
    end_line: usize,
    kind: FoldKind,
}

#[derive(Debug, Clone, Copy, Default)]
struct TerminalStartupMetrics {
    loading_note: Duration,
    loading_switcher: Duration,
    loading_calc_engine: Duration,
    loading_screen: Duration,
}

#[derive(Debug, Clone)]
struct LineReminderGhost {
    remind_at_ms: i64,
    display_at: String,
    line_text: String,
    notified_at_ms: Option<i64>,
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
    reminder_ghosts: HashMap<usize, LineReminderGhost>, // 0-based line index
    reminders_dirty: bool,
    last_reminder_check: Instant,
    variable_names: Vec<String>,
    // Snapshot of `lines` taken at the end of the previous `recompute_calc_full`.
    // Used to gate the committed-trailer auto-refresh: a line is eligible only
    // if it is byte-identical to this snapshot and its previous calc result
    // was `None` (meaning the trailer was in sync with the backend last time).
    prev_lines: Vec<String>,
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
    // Track which mode entered command bar from
    command_bar_from_normal: bool,
    // Clipboard watch
    clipboard_watch_enabled: bool,
    clipboard_watch_last_text: Option<String>,
    clipboard_watch_last_poll: Instant,
    // Undo/redo
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    last_undo_snapshot: UndoEntry,
}

impl TerminalApp {
    fn new_with_startup_metrics(
        db: &Db,
        opts: &TerminalOptions,
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
        let active_note = select_note(db, opts)?;
        let loading_note = note_begin.elapsed();

        let lines = split_lines(&active_note.body);
        let reminder_ghosts = load_note_reminder_ghosts(db, &active_note.id, &lines)?;

        let switcher_begin = Instant::now();
        let switcher_items = load_note_meta(db)?;
        let loading_switcher = switcher_begin.elapsed();

        let calc_engine = CalcEngine::new();
        let calc_begin = Instant::now();
        let calc_data = compute_calc_data(&calc_engine, &lines, variables_enabled, None);
        let loading_calc_engine = calc_begin.elapsed();

        let prev_lines_snapshot = lines.clone();
        let undo_seed = lines.clone();

        let app = Self {
            active_note,
            lines,
            cursor_line: 0,
            cursor_col: 0,
            scroll_line: 0,
            scroll_col: 0,
            mode: UiMode::Normal,
            switcher_query: String::new(),
            switcher_items,
            switcher_matches: Vec::new(),
            switcher_selected: 0,
            dirty: false,
            last_edit: Instant::now(),
            status: "-- NORMAL --  |  :cmd  Ctrl+F find  Ctrl+N new  Ctrl+P switch  Ctrl+Q quit"
                .to_string(),
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
            reminder_ghosts,
            reminders_dirty: false,
            last_reminder_check: Instant::now(),
            variable_names: calc_data.variable_names,
            prev_lines: prev_lines_snapshot,
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
            command_bar_from_normal: false,
            clipboard_watch_enabled: false,
            clipboard_watch_last_text: None,
            clipboard_watch_last_poll: Instant::now(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_undo_snapshot: UndoEntry {
                lines: undo_seed,
                cursor_line: 0,
                cursor_col: 0,
            },
        };

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

            match read_key()? {
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
        self.clipboard_watch_last_text = read_clipboard_via_commands();
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

        let Some(text) = read_clipboard_via_commands() else {
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
        let mut should_autoformat = true;
        match key {
            Key::Ctrl('q') => {
                self.quit = true;
                return Ok(());
            }
            Key::Ctrl('w') => {
                self.delete_word_backward();
                return Ok(());
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
                if !self.try_table_navigation_rule(true) {
                    self.move_cursor_left_word();
                }
            }
            Key::CtrlArrowRight => {
                if !self.try_table_navigation_rule(false) {
                    self.move_cursor_right_word();
                }
            }
            Key::PageUp => self.move_cursor_up(self.editor_height().saturating_sub(1)),
            Key::PageDown => self.move_cursor_down(self.editor_height().saturating_sub(1)),
            Key::Home => self.cursor_col = 0,
            Key::End => self.cursor_col = line_char_len(self.current_line()),
            Key::Backspace => self.backspace(),
            Key::Delete => self.delete_forward(),
            Key::Enter => {
                if !self.try_enter_rule() {
                    self.insert_newline();
                }
            }
            Key::Tab => {
                if self.apply_calc_tab() {
                    // handled
                } else if !self.try_tab_rule(false) {
                    self.insert_text("  ");
                }
            }
            Key::BackTab => {
                self.try_tab_rule(true);
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
            Key::Char(ch) => self.insert_char(ch),
            Key::Esc => {
                self.mode = UiMode::Normal;
                self.vim_state = crate::editor_core::vim::VimState::default();
                self.status = "-- NORMAL --".to_string();
            }
            Key::Ctrl(_) => {}
        }

        self.adjust_cursor();
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
        let backend = copy_text_to_clipboard(&joined);
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
        let text = read_clipboard_via_commands().or_else(|| {
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
                    self.cursor_line = self.lines.len().saturating_sub(1)
                }
                crate::editor_core::vim::VimIntent::MoveToLine => {
                    let line = count.max(1).min(self.lines.len().max(1));
                    self.cursor_line = line.saturating_sub(1);
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

        let snapshot = self.build_snapshot();
        let options = crate::editor_core::text_rules::TextRuleOptions {
            markdown_autoformat: self.markdown_autoformat,
            checklist_auto_reorder: self.checklist_auto_reorder,
        };
        if let Some(op) = crate::editor_core::text_rules::run_doc_change_rules(&snapshot, options) {
            self.apply_edit_operation(&op);
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
            Key::Enter => {
                if let Some(idx) = self.switcher_matches.get(self.switcher_selected).copied() {
                    let id = self.switcher_items[idx].id.clone();
                    self.save(db)?;
                    if let Some(note) = db.get_note(&id)? {
                        self.set_active_note(db, note)?;
                        self.status = format!("opened {}", id);
                    } else {
                        self.status = format!("note missing {}", id);
                    }
                    self.close_switcher();
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
            | Key::Delete
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
        if let Some((year, month, day, hour, minute)) = current_local_datetime_parts() {
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
                    let inserted = format_datetime_with_pattern(
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

                let remind_at_ms = local_datetime_to_epoch_ms(
                    self.date_year,
                    self.date_month,
                    self.date_day,
                    self.date_hour,
                    self.date_minute,
                )
                .ok_or_else(|| "failed to convert reminder time".to_string())?;
                let display_at = format_datetime_with_pattern(
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
                let max = days_in_month(self.date_year, self.date_month);
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
                    let max = days_in_month(self.date_year, self.date_month);
                    self.date_day = max.min(self.date_day);
                }
            }
            Key::ArrowDown => {
                let max = days_in_month(self.date_year, self.date_month);
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
                    let new_max = days_in_month(self.date_year, self.date_month);
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
                let max = days_in_month(self.date_year, self.date_month);
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
                let max = days_in_month(self.date_year, self.date_month);
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
        self.status = "Switcher: type to filter, Enter open, Esc close".to_string();
        Ok(())
    }

    fn close_switcher(&mut self) {
        self.mode = UiMode::Editor;
        self.switcher_query.clear();
        self.switcher_matches.clear();
        self.switcher_selected = 0;
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
            if let Some(score) = fuzzy_score(query, &item.title) {
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
        let saved = db.save_note(&self.active_note.id, &body)?;
        self.active_note = saved;
        self.dirty = false;
        self.last_undo_snapshot = UndoEntry {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        };
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

        let now_ms = now_epoch_ms();
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

            if let Err(error) = send_system_notification("Note reminder", &body) {
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
        self.switcher_items = load_note_meta(db)?;
        if self.mode == UiMode::Switcher {
            self.recompute_switcher_matches();
        }
        Ok(())
    }

    fn set_active_note(&mut self, db: &Db, note: Note) -> Result<(), String> {
        self.active_note = note;
        self.lines = split_lines(&self.active_note.body);
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
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_undo_snapshot = UndoEntry {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        };
        self.recompute_calc_full();
        self.adjust_cursor();
        self.adjust_scroll();
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

    fn mark_edited(&mut self) {
        // Push undo snapshot if enough time elapsed since last edit (debounce).
        // The snapshot represents the state *before* the current mutation.
        self.redo_stack.clear();
        if self.last_edit.elapsed() >= Duration::from_millis(300) || self.undo_stack.is_empty() {
            self.undo_stack.push(self.last_undo_snapshot.clone());
            if self.undo_stack.len() > MAX_UNDO_ENTRIES {
                self.undo_stack.remove(0);
            }
        }
        self.dirty = true;
        self.last_edit = Instant::now();
        if !self.reminder_ghosts.is_empty() {
            self.reminders_dirty = true;
        }
        self.recompute_calc_full();
        self.last_undo_snapshot = UndoEntry {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        };
    }

    fn undo(&mut self) {
        if let Some(entry) = self.undo_stack.pop() {
            self.redo_stack.push(UndoEntry {
                lines: self.lines.clone(),
                cursor_line: self.cursor_line,
                cursor_col: self.cursor_col,
            });
            self.lines = entry.lines;
            self.cursor_line = entry.cursor_line.min(self.lines.len().saturating_sub(1));
            self.cursor_col = entry.cursor_col;
            self.dirty = true;
            self.last_edit = Instant::now();
            if !self.reminder_ghosts.is_empty() {
                self.reminders_dirty = true;
            }
            self.recompute_calc_full();
            self.adjust_cursor();
            self.adjust_scroll();
            self.last_undo_snapshot = UndoEntry {
                lines: self.lines.clone(),
                cursor_line: self.cursor_line,
                cursor_col: self.cursor_col,
            };
            self.status = format!("undo ({} left)", self.undo_stack.len());
        } else {
            self.status = "already at oldest change".to_string();
        }
    }

    fn redo(&mut self) {
        if let Some(entry) = self.redo_stack.pop() {
            self.undo_stack.push(UndoEntry {
                lines: self.lines.clone(),
                cursor_line: self.cursor_line,
                cursor_col: self.cursor_col,
            });
            self.lines = entry.lines;
            self.cursor_line = entry.cursor_line.min(self.lines.len().saturating_sub(1));
            self.cursor_col = entry.cursor_col;
            self.dirty = true;
            self.last_edit = Instant::now();
            if !self.reminder_ghosts.is_empty() {
                self.reminders_dirty = true;
            }
            self.recompute_calc_full();
            self.adjust_cursor();
            self.adjust_scroll();
            self.last_undo_snapshot = UndoEntry {
                lines: self.lines.clone(),
                cursor_line: self.cursor_line,
                cursor_col: self.cursor_col,
            };
            self.status = format!("redo ({} left)", self.redo_stack.len());
        } else {
            self.status = "already at newest change".to_string();
        }
    }

    fn recompute_calc_full(&mut self) {
        let plan = crate::editor_core::calc_plan::plan_incremental_calc(
            &self.prev_lines,
            &self.calc_results,
            &self.lines,
        );
        let has_prev = !self.prev_lines.is_empty();
        let has_builtin_formula =
            crate::editor_core::calc_plan::contains_builtin_formula(&self.lines)
                || crate::editor_core::calc_plan::contains_builtin_formula(&self.prev_lines);

        // Prev-side changed slice mirrors the next-side plan by preserving
        // the shared suffix length.
        let suffix_len = self.lines.len().saturating_sub(plan.eval_to);
        let prev_changed_from = plan.eval_from.min(self.prev_lines.len());
        let prev_changed_to = self
            .prev_lines
            .len()
            .saturating_sub(suffix_len)
            .max(prev_changed_from);
        let prev_changed_lines = &self.prev_lines[prev_changed_from..prev_changed_to];
        let touches_any_assignment =
            crate::editor_core::calc_plan::contains_variable_assignment(&plan.eval_lines)
                || crate::editor_core::calc_plan::contains_variable_assignment(prev_changed_lines);
        let can_use_partial = has_prev && !touches_any_assignment && !has_builtin_formula;

        let (mut new_results, variable_names) = if can_use_partial {
            let mut merged_results = vec![None; self.lines.len()];
            for entry in &plan.base_results {
                if let Some(slot) = merged_results.get_mut(entry.line_idx) {
                    *slot = Some(entry.result.clone());
                }
            }

            if plan.eval_from < plan.eval_to {
                let calc_data = compute_calc_data(
                    &self.calc_engine,
                    &self.lines,
                    self.variables_enabled,
                    Some((plan.eval_from, plan.eval_to)),
                );
                for idx in plan.eval_from..plan.eval_to {
                    if let Some(slot) = merged_results.get_mut(idx) {
                        *slot = calc_data.line_results.get(idx).cloned().unwrap_or(None);
                    }
                }
                (merged_results, calc_data.variable_names)
            } else {
                (merged_results, self.variable_names.clone())
            }
        } else {
            let calc_data =
                compute_calc_data(&self.calc_engine, &self.lines, self.variables_enabled, None);
            (calc_data.line_results, calc_data.variable_names)
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
        let aligned = self.prev_lines.len() == self.lines.len()
            && self.calc_results.len() == self.lines.len();

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
                if self.prev_lines[i] != self.lines[i] {
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
                }
            }
        }

        self.prev_lines = self.lines.clone();
        self.calc_results = new_results;
        self.variable_names = variable_names;
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
            if self.cursor_line > 0 {
                self.cursor_line -= 1;
                self.cursor_col = line_char_len(self.current_line());
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
            if self.cursor_line + 1 < self.lines.len() {
                self.cursor_line += 1;
                self.cursor_col = 0;
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

    fn delete_word_backward(&mut self) {
        if self.cursor_col == 0 {
            if self.cursor_line > 0 {
                self.backspace();
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

        let start_byte = byte_index(self.current_line(), col);
        let end_byte = byte_index(self.current_line(), self.cursor_col);
        let text = self.current_line_mut();
        text.replace_range(start_byte..end_byte, "");
        self.cursor_col = col;
        self.mark_edited();
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
        let line = self.current_line_mut();
        let idx = byte_index(line, col);
        let right = line[idx..].to_string();
        line.truncate(idx);
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

    fn try_autoformat_rules(&mut self) {
        // Fast path: skip the expensive build_snapshot/parse round trip
        // when the current line can't trigger any doc-change rules.
        let line = self.current_line();
        let trimmed = line.trim_start();
        let might_be_list = trimmed.starts_with('-')
            || trimmed.starts_with('*')
            || trimmed.starts_with('+')
            || trimmed.starts_with("->")
            || trimmed.chars().next().is_some_and(|c| c.is_ascii_digit());
        let might_be_table = trimmed.starts_with('|') && line.trim_end().ends_with('|');
        let might_be_checklist = might_be_list && line.contains("/x");
        if !might_be_list && !might_be_checklist && !might_be_table {
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

    fn apply_edit_operation(&mut self, op: &crate::editor_core::types::EditOperation) {
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
                    mapped_anchor = from + change.insert.len();
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
        self.mark_edited();
        self.adjust_cursor();
        self.adjust_scroll();
    }

    fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let new_col = self.cursor_col - 1;
            let line = self.current_line_mut();
            remove_char_at(line, new_col);
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
        let line_len = line_char_len(self.current_line());
        if self.cursor_col < line_len {
            let col = self.cursor_col;
            let line = self.current_line_mut();
            remove_char_at(line, col);
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
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            return;
        }
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = line_char_len(self.current_line());
        }
    }

    fn move_cursor_right(&mut self) {
        let line_len = line_char_len(self.current_line());
        if self.cursor_col < line_len {
            self.cursor_col += 1;
            return;
        }
        if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
    }

    fn move_cursor_up(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.cursor_line = self.cursor_line.saturating_sub(count);
    }

    fn move_cursor_down(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.cursor_line = min(self.cursor_line + count, self.lines.len().saturating_sub(1));
    }

    fn adjust_cursor(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        if self.cursor_line >= self.lines.len() {
            self.cursor_line = self.lines.len() - 1;
        }
        let len = line_char_len(self.current_line());
        if self.cursor_col > len {
            self.cursor_col = len;
        }
    }

    fn editor_height(&self) -> usize {
        let (rows, _) = terminal_size();
        rows.saturating_sub(2).max(1)
    }

    fn adjust_scroll(&mut self) {
        let height = self.editor_height();
        if self.cursor_line < self.scroll_line {
            self.scroll_line = self.cursor_line;
        } else if self.cursor_line >= self.scroll_line + height {
            self.scroll_line = self.cursor_line + 1 - height;
        }

        let (_, cols) = terminal_size();
        let available = cols.saturating_sub(GUTTER_WIDTH);
        if available == 0 {
            self.scroll_col = 0;
            return;
        }

        let (cursor_display_col, max_scroll) = {
            let line_text = self.current_line();
            let line_len = line_char_len(line_text);
            let logical_col = min(self.cursor_col, line_len);
            let render_col = cursor_render_char_col(line_text, self.cursor_col, self.mode);
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
        let (rows, cols) = terminal_size();
        let editor_height = rows.saturating_sub(2).max(1);
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
        ctx.advance_lines(&self.lines[..self.scroll_line.min(self.lines.len())]);
        let mut cursor_line_override: Option<(String, usize)> = None;
        let now_ms = now_epoch_ms();

        for i in 0..editor_height {
            let row = EDITOR_TOP_ROW + i;
            let line_idx = self.scroll_line + i;
            if line_idx < self.lines.len() {
                let line_no = line_idx + 1;
                let available = cols.saturating_sub(GUTTER_WIDTH);
                let is_cursor_line = line_idx == self.cursor_line;
                let mut calc_ghost = self.calc_results.get(line_idx).and_then(|r| r.as_deref());
                let mut calc_ghost_override: Option<String> = None;
                let mut reminder_ghost_override: Option<String> = None;
                let mut reminder_strikethrough = false;
                let mut ghost_dim_ranges: Vec<(usize, usize)> = Vec::new();
                let line_text = &self.lines[line_idx];
                let mut rendered_line = line_text.to_string();

                if let Some(reminder) = self.reminder_ghosts.get(&line_idx) {
                    reminder_ghost_override = Some(format!("⏰ {}", reminder.display_at));
                    reminder_strikethrough = reminder.remind_at_ms <= now_ms;
                }

                if let Some(formula) = find_table_formula_segment(line_text) {
                    // Formula rows render a marker in-cell (`value*`) and keep
                    // the detailed explanation as a line-end ghost.
                    calc_ghost = None;

                    if let Some(result) = self.calc_results.get(line_idx).and_then(|r| r.as_deref())
                    {
                        if !(is_cursor_line
                            && self.cursor_col >= formula.from_char
                            && self.cursor_col <= formula.to_char)
                        {
                            let formatted = format_formula_display_value(result);
                            let marker_char = formula.from_char + formatted.chars().count();
                            let mut replacement = format!("{formatted}*");
                            let old_len = formula.to_char.saturating_sub(formula.from_char);
                            let new_len = replacement.chars().count();
                            if new_len < old_len {
                                replacement.push_str(&" ".repeat(old_len - new_len));
                            }
                            calc_ghost_override = Some(format!("* ➜ {}", formula.label));
                            ghost_dim_ranges.push((marker_char, marker_char + 1));

                            let mut out = String::with_capacity(
                                line_text
                                    .len()
                                    .saturating_sub(formula.to_byte - formula.from_byte)
                                    + replacement.len(),
                            );
                            out.push_str(&line_text[..formula.from_byte]);
                            out.push_str(&replacement);
                            out.push_str(&line_text[formula.to_byte..]);
                            rendered_line = out;

                            if is_cursor_line {
                                let mapped_col = if self.cursor_col <= formula.from_char {
                                    self.cursor_col
                                } else if self.cursor_col >= formula.to_char {
                                    if new_len >= old_len {
                                        self.cursor_col + (new_len - old_len)
                                    } else {
                                        self.cursor_col.saturating_sub(old_len - new_len)
                                    }
                                } else {
                                    self.cursor_col
                                };
                                cursor_line_override = Some((rendered_line.clone(), mapped_col));
                            }
                        }
                    }
                }

                let (search_ranges, current_search_ranges) =
                    self.search_highlights_for_line(line_idx);
                let mut visual_highlight_ranges = Vec::new();
                self.append_visual_highlights(line_idx, &mut visual_highlight_ranges);
                let effective_calc_ghost = calc_ghost_override.as_deref().or(calc_ghost);
                let effective_reminder_ghost = reminder_ghost_override.as_deref();
                let line_scroll_col = self.scroll_col;
                let line_width = line_display_cols(&rendered_line);
                let viewport = compute_line_viewport(line_width, line_scroll_col, available);

                let rendered_text =
                    if ghost_dim_ranges.is_empty() && visual_highlight_ranges.is_empty() {
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
                        ctx.render_line_with_dim_ranges_window_with_reminder(
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
                buf.push_str(&format!("{line_no:>4}  "));
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
            UiMode::Switcher => "Switcher: type to filter, Enter open, Esc close",
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
            draw_switcher(self, &mut buf, rows, cols, self.render_palette);
        }

        if self.mode == UiMode::DatePicker {
            draw_date_picker(self, &mut buf, rows, cols, self.render_palette);
        }

        let (cursor_row, mut cursor_col) = self.cursor_position(rows, cols);
        if let Some((line_text, mapped_col)) = cursor_line_override {
            if matches!(
                self.mode,
                UiMode::Editor | UiMode::Normal | UiMode::Visual | UiMode::VisualLine
            ) {
                let available = cols.saturating_sub(GUTTER_WIDTH);
                let display_char_col = cursor_render_char_col(&line_text, mapped_col, self.mode);
                let display_col = display_cols_for_prefix(&line_text, display_char_col);
                let line_width = line_display_cols(&line_text);
                let visible_col = viewport_col_for_display_col(
                    display_col,
                    line_width,
                    self.scroll_col,
                    available,
                );
                cursor_col = (GUTTER_WIDTH + visible_col + 1).min(cols.max(1)).max(1);
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
                let row = EDITOR_TOP_ROW
                    + self
                        .cursor_line
                        .saturating_sub(self.scroll_line)
                        .min(rows.saturating_sub(2));
                let line_text = self.current_line();
                let display_char_col =
                    cursor_render_char_col(line_text, self.cursor_col, self.mode);
                let available = cols.saturating_sub(GUTTER_WIDTH);
                let display_col = display_cols_for_prefix(line_text, display_char_col);
                let line_width = line_display_cols(line_text);
                let visible_col = viewport_col_for_display_col(
                    display_col,
                    line_width,
                    self.scroll_col,
                    available,
                );
                let col = (GUTTER_WIDTH + visible_col + 1).min(cols.max(1));
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
        print_note_list(db)?;
        let line = format!("time:{startup_ts_ms} loading_screen:0ms list_notes:0ms");
        if let Err(err) = append_startup_log_line("tui", &line) {
            eprintln!("Startup diagnostics: {err}");
        }
        return Ok(());
    }

    let (mut app, metrics) = TerminalApp::new_with_startup_metrics(
        db,
        opts,
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

fn split_lines(body: &str) -> Vec<String> {
    if body.is_empty() {
        vec![String::new()]
    } else {
        body.split('\n').map(|l| l.to_string()).collect()
    }
}

fn join_lines(lines: &[String]) -> String {
    if lines.len() == 1 && lines[0].is_empty() {
        String::new()
    } else {
        lines.join("\n")
    }
}

fn line_char_len(text: &str) -> usize {
    text.chars().count()
}

fn display_cols_for_prefix(text: &str, prefix_chars: usize) -> usize {
    let mut visible = 0usize;
    for ch in text.chars().take(prefix_chars) {
        if ch == '\t' {
            let tab = render::TAB_WIDTH - (visible % render::TAB_WIDTH);
            visible += tab;
        } else {
            visible += 1;
        }
    }
    visible
}

fn line_display_cols(text: &str) -> usize {
    display_cols_for_prefix(text, line_char_len(text))
}

fn cursor_render_char_col(text: &str, cursor_col: usize, mode: UiMode) -> usize {
    let line_len = line_char_len(text);
    let clamped_col = min(cursor_col, line_len);
    if matches!(mode, UiMode::Normal | UiMode::Visual | UiMode::VisualLine)
        && line_len > 0
        && clamped_col == line_len
    {
        line_len - 1
    } else {
        clamped_col
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct LineViewport {
    has_left_overflow: bool,
    has_right_overflow: bool,
    text_window_col: usize,
    text_width: usize,
}

fn compute_line_viewport(
    line_width: usize,
    scroll_col: usize,
    available_cols: usize,
) -> LineViewport {
    if available_cols == 0 {
        return LineViewport::default();
    }

    let has_left_overflow = scroll_col > 0;
    let has_right_overflow = line_width > scroll_col.saturating_add(available_cols);
    let reserved = (has_left_overflow as usize) + (has_right_overflow as usize);
    let text_width = available_cols.saturating_sub(reserved);
    let text_window_col = scroll_col.saturating_add(has_left_overflow as usize);

    LineViewport {
        has_left_overflow,
        has_right_overflow,
        text_window_col,
        text_width,
    }
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

fn viewport_col_for_display_col(
    display_col: usize,
    line_width: usize,
    scroll_col: usize,
    available_cols: usize,
) -> usize {
    let viewport = compute_line_viewport(line_width, scroll_col, available_cols);
    if viewport.text_width == 0 {
        return 0;
    }

    let text_rel = display_col
        .saturating_sub(viewport.text_window_col)
        .min(viewport.text_width.saturating_sub(1));
    (viewport.has_left_overflow as usize).saturating_add(text_rel)
}

fn byte_index(text: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

fn remove_char_at(text: &mut String, char_idx: usize) {
    let start = byte_index(text, char_idx);
    let end = byte_index(text, char_idx + 1);
    if start < end && end <= text.len() {
        text.replace_range(start..end, "");
    }
}

fn derive_title_from_lines(lines: &[String]) -> String {
    let line = lines
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|s| s.trim())
        .unwrap_or("Untitled");
    if line.chars().count() > 70 {
        let truncated: String = line.chars().take(70).collect();
        format!("{truncated}...")
    } else {
        line.to_string()
    }
}

fn load_note_meta(db: &Db) -> Result<Vec<NoteMeta>, String> {
    Ok(db
        .list_notes()?
        .into_iter()
        .map(|n| NoteMeta {
            id: n.id.clone(),
            title: note_title(&n),
        })
        .collect())
}

fn note_title(note: &Note) -> String {
    let first = note
        .body
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("Untitled")
        .trim();
    if first.chars().count() > 60 {
        let truncated: String = first.chars().take(60).collect();
        format!("{truncated}...")
    } else {
        first.to_string()
    }
}

fn print_note_list(db: &Db) -> Result<(), String> {
    let notes = db.list_notes()?;
    if notes.is_empty() {
        println!("No notes");
        return Ok(());
    }
    for (idx, note) in notes.iter().enumerate() {
        println!("{:>3}. {}  {}", idx + 1, note.id, note_title(note));
    }
    Ok(())
}

fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q = query.to_lowercase();
    let t = text.to_lowercase();
    let q_chars: Vec<char> = q.chars().collect();
    let t_chars: Vec<char> = t.chars().collect();
    let raw_chars: Vec<char> = text.chars().collect();

    let mut qi = 0usize;
    let mut score = 0i32;
    let mut prev_match = -2isize;
    for (ti, ch) in t_chars.iter().enumerate() {
        if qi >= q_chars.len() {
            break;
        }
        if *ch == q_chars[qi] {
            score += if prev_match == ti as isize - 1 { 2 } else { 1 };
            if ti == 0
                || raw_chars
                    .get(ti - 1)
                    .map(|c| c.is_whitespace())
                    .unwrap_or(false)
            {
                score += 1;
            }
            prev_match = ti as isize;
            qi += 1;
        }
    }

    if qi == q_chars.len() {
        Some(score)
    } else {
        None
    }
}

fn pad_right(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    let current = out.chars().count();
    if current < width {
        out.push_str(&" ".repeat(width - current));
    }
    out
}

#[derive(Clone, Copy, Default)]
struct AnsiStyle {
    fg: Option<u8>,
    bg: Option<u8>,
    bold: bool,
    dim: bool,
    reverse: bool,
}

impl AnsiStyle {
    fn write_to(self, buf: &mut String) {
        buf.push_str("\x1b[0");
        if self.bold {
            buf.push_str(";1");
        }
        if self.dim {
            buf.push_str(";2");
        }
        if self.reverse {
            buf.push_str(";7");
        }
        if let Some(fg) = self.fg {
            let _ = write!(buf, ";38;5;{fg}");
        }
        if let Some(bg) = self.bg {
            let _ = write!(buf, ";48;5;{bg}");
        }
        buf.push('m');
    }
}

fn ansi_256_rgb(index: u8) -> (u8, u8, u8) {
    if index < 16 {
        const ANSI16: [(u8, u8, u8); 16] = [
            (0, 0, 0),
            (128, 0, 0),
            (0, 128, 0),
            (128, 128, 0),
            (0, 0, 128),
            (128, 0, 128),
            (0, 128, 128),
            (192, 192, 192),
            (128, 128, 128),
            (255, 0, 0),
            (0, 255, 0),
            (255, 255, 0),
            (0, 0, 255),
            (255, 0, 255),
            (0, 255, 255),
            (255, 255, 255),
        ];
        return ANSI16[index as usize];
    }

    if index <= 231 {
        let idx = index - 16;
        let r = idx / 36;
        let g = (idx % 36) / 6;
        let b = idx % 6;
        let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
        return (level(r), level(g), level(b));
    }

    let gray = 8 + (index - 232) * 10;
    (gray, gray, gray)
}

fn contrast_fg_for_bg(bg: u8) -> u8 {
    let (r, g, b) = ansi_256_rgb(bg);
    // Relative luminance approximation in integer space.
    let luminance = (299u32 * r as u32 + 587u32 * g as u32 + 114u32 * b as u32) / 1000u32;
    if luminance >= 140 {
        16 // dark text on light background
    } else {
        231 // light text on dark background
    }
}

fn draw_row_at_styled(
    buf: &mut String,
    row: usize,
    col: usize,
    width: usize,
    text: &str,
    style: AnsiStyle,
) {
    buf.push_str(&goto(row, col));
    style.write_to(buf);
    buf.push_str(&pad_right(text, width));
    buf.push_str(render::RESET);
}

fn draw_switcher(
    app: &TerminalApp,
    buf: &mut String,
    rows: usize,
    cols: usize,
    palette: render::RenderPalette,
) {
    let box_w = min(cols.saturating_sub(4).max(30), 72);
    let box_h = min(rows.saturating_sub(4).max(8), 14);
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;

    let border_style = AnsiStyle {
        fg: Some(palette.code_type),
        ..Default::default()
    };
    let prompt_style = AnsiStyle {
        fg: Some(palette.code_keyword),
        bold: true,
        ..Default::default()
    };
    let label_style = AnsiStyle {
        fg: Some(palette.code_comment),
        dim: true,
        ..Default::default()
    };
    let row_style = AnsiStyle {
        fg: Some(palette.variable),
        ..Default::default()
    };
    let selected_bg = palette.search_current;
    let selected_style = AnsiStyle {
        fg: Some(contrast_fg_for_bg(selected_bg)),
        bg: Some(selected_bg),
        bold: true,
        ..Default::default()
    };

    // Border
    border_style.write_to(buf);
    for dx in 0..box_w {
        let ch_top = if dx == 0 || dx + 1 == box_w { '+' } else { '-' };
        buf.push_str(&goto(y, x + dx));
        buf.push(ch_top);
        buf.push_str(&goto(y + box_h - 1, x + dx));
        buf.push(ch_top);
    }
    for dy in 1..box_h.saturating_sub(1) {
        buf.push_str(&goto(y + dy, x));
        buf.push('|');
        buf.push_str(&goto(y + dy, x + box_w - 1));
        buf.push('|');
    }
    buf.push_str(render::RESET);

    let prompt = format!(" search: {}", app.switcher_query);
    draw_row_at_styled(
        buf,
        y + 1,
        x + 1,
        box_w.saturating_sub(2),
        &prompt,
        prompt_style,
    );
    draw_row_at_styled(
        buf,
        y + 2,
        x + 1,
        box_w.saturating_sub(2),
        " results:",
        label_style,
    );

    let max_rows = box_h.saturating_sub(4);
    let mut start = 0usize;
    if app.switcher_selected >= max_rows {
        start = app.switcher_selected + 1 - max_rows;
    }

    for i in 0..max_rows {
        let row = y + 3 + i;
        if let Some(match_idx) = app.switcher_matches.get(start + i).copied() {
            let item = &app.switcher_items[match_idx];
            let marker = if start + i == app.switcher_selected {
                ">"
            } else {
                " "
            };
            let text = format!("{marker} {}  {}", item.id, item.title);
            if start + i == app.switcher_selected {
                draw_row_at_styled(
                    buf,
                    row,
                    x + 1,
                    box_w.saturating_sub(2),
                    &text,
                    selected_style,
                );
            } else {
                draw_row_at_styled(buf, row, x + 1, box_w.saturating_sub(2), &text, row_style);
            }
        } else {
            draw_row_at_styled(buf, row, x + 1, box_w.saturating_sub(2), "", row_style);
        }
    }
}

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const MONTH_NAMES_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Zeller-style day of week: 0=Mon, 1=Tue, ..., 6=Sun
fn day_of_week(year: i32, month: u32, day: u32) -> u32 {
    let (y, m) = if month <= 2 {
        (year - 1, month + 12)
    } else {
        (year, month)
    };
    let q = day as i32;
    let k = y % 100;
    let j = y / 100;
    let m = m as i32;
    let h = (q + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 - 2 * j) % 7;
    // h: 0=Sat, 1=Sun, 2=Mon, ...
    let dow = ((h + 5) % 7 + 7) % 7;
    dow as u32
}

fn now_epoch_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

#[cfg(target_os = "macos")]
fn escape_applescript(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

fn send_system_notification(title: &str, body: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let status = Command::new("notify-send")
            .args(["--", title, body])
            .status()
            .map_err(|e| format!("notify-send unavailable: {e}"))?;
        if status.success() {
            return Ok(());
        }
        return Err(format!("notify-send exited with status {status}"));
    }

    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            escape_applescript(body),
            escape_applescript(title)
        );
        let status = Command::new("osascript")
            .args(["-e", script.as_str()])
            .status()
            .map_err(|e| format!("osascript unavailable: {e}"))?;
        if status.success() {
            return Ok(());
        }
        return Err(format!("osascript exited with status {status}"));
    }

    #[cfg(target_os = "windows")]
    {
        let escaped_title = title.replace('\'', "''");
        let escaped_body = body.replace('\'', "''");
        let command = format!(
            "$null=[Windows.UI.Notifications.ToastNotificationManager,Windows.UI.Notifications,ContentType=WindowsRuntime];\
             $null=[Windows.Data.Xml.Dom.XmlDocument,Windows.Data.Xml.Dom.XmlDocument,ContentType=WindowsRuntime];\
             $xml=New-Object Windows.Data.Xml.Dom.XmlDocument;\
             $xml.LoadXml(\"<toast><visual><binding template='ToastGeneric'><text>{escaped_title}</text><text>{escaped_body}</text></binding></visual></toast>\");\
             $toast=[Windows.UI.Notifications.ToastNotification]::new($xml);\
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('slate').Show($toast);"
        );
        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                command.as_str(),
            ])
            .status()
            .map_err(|e| format!("powershell unavailable: {e}"))?;
        if status.success() {
            return Ok(());
        }
        return Err(format!("powershell exited with status {status}"));
    }

    #[allow(unreachable_code)]
    Err("system notifications are not supported on this platform".to_string())
}

fn current_local_datetime_parts() -> Option<(i32, u32, u32, u32, u32)> {
    let epoch_seconds: libc::time_t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs()
        .try_into()
        .ok()?;
    let mut local_tm = unsafe { std::mem::zeroed::<libc::tm>() };
    let ptr = unsafe { libc::localtime_r(&epoch_seconds, &mut local_tm as *mut libc::tm) };
    if ptr.is_null() {
        return None;
    }
    Some((
        local_tm.tm_year + 1900,
        (local_tm.tm_mon + 1) as u32,
        local_tm.tm_mday as u32,
        local_tm.tm_hour as u32,
        local_tm.tm_min as u32,
    ))
}

fn local_datetime_to_epoch_ms(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
) -> Option<i64> {
    if !(1..=12).contains(&month) {
        return None;
    }
    if day == 0 || day > days_in_month(year, month) {
        return None;
    }
    if hour > 23 || minute > 59 {
        return None;
    }

    let mut local_tm = unsafe { std::mem::zeroed::<libc::tm>() };
    local_tm.tm_year = year - 1900;
    local_tm.tm_mon = i32::try_from(month).ok()? - 1;
    local_tm.tm_mday = i32::try_from(day).ok()?;
    local_tm.tm_hour = i32::try_from(hour).ok()?;
    local_tm.tm_min = i32::try_from(minute).ok()?;
    local_tm.tm_sec = 0;
    local_tm.tm_isdst = -1;

    let epoch_seconds = unsafe { libc::mktime(&mut local_tm as *mut libc::tm) };
    if epoch_seconds < 0 {
        return None;
    }
    i64::try_from(i128::from(epoch_seconds) * 1000).ok()
}

fn format_datetime_with_pattern(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    pattern: &str,
) -> String {
    let month_idx = month.saturating_sub(1).min(11) as usize;
    let yyyy = format!("{year:04}");
    let yy = format!("{:02}", year.rem_euclid(100));
    let mm = format!("{month:02}");
    let m = month.to_string();
    let dd = format!("{day:02}");
    let d = day.to_string();
    let hh = format!("{hour:02}");
    let h = hour.to_string();
    let min2 = format!("{minute:02}");
    let mmm = MONTH_NAMES_SHORT[month_idx];
    let mmmm = MONTH_NAMES[month_idx];

    let mut out = if pattern.trim().is_empty() {
        "%Y-%m-%d".to_string()
    } else {
        pattern.to_string()
    };

    if out.contains('%') {
        for (token, value) in [
            ("%Y", yyyy.as_str()),
            ("%y", yy.as_str()),
            ("%m", mm.as_str()),
            ("%d", dd.as_str()),
            ("%H", hh.as_str()),
            ("%M", min2.as_str()),
            ("%b", mmm),
            ("%B", mmmm),
        ] {
            out = out.replace(token, value);
        }
        return out;
    }

    for (token, value) in [
        ("YYYY", yyyy.as_str()),
        ("MMMM", mmmm),
        ("MMM", mmm),
        ("MM", mm.as_str()),
        ("DD", dd.as_str()),
        ("HH", hh.as_str()),
        ("mm", min2.as_str()),
        ("YY", yy.as_str()),
        ("M", m.as_str()),
        ("D", d.as_str()),
        ("H", h.as_str()),
    ] {
        out = out.replace(token, value);
    }
    out
}

fn draw_date_picker(
    app: &TerminalApp,
    buf: &mut String,
    rows: usize,
    cols: usize,
    palette: render::RenderPalette,
) {
    let box_w: usize = 38;
    let box_h: usize = 15;
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;

    let border_style = AnsiStyle {
        fg: Some(palette.code_type),
        ..Default::default()
    };
    let title_style = AnsiStyle {
        fg: Some(palette.code_keyword),
        bold: true,
        ..Default::default()
    };
    let header_style = AnsiStyle {
        fg: Some(palette.code_comment),
        dim: true,
        ..Default::default()
    };
    let day_style = AnsiStyle {
        fg: Some(palette.variable),
        ..Default::default()
    };
    let selected_bg = palette.search_current;
    let selected_day_style = AnsiStyle {
        fg: Some(contrast_fg_for_bg(selected_bg)),
        bg: Some(selected_bg),
        bold: true,
        ..Default::default()
    };
    let footer_style = AnsiStyle {
        fg: Some(palette.search_match),
        bold: true,
        ..Default::default()
    };
    let time_style = AnsiStyle {
        fg: Some(palette.code_string),
        ..Default::default()
    };
    let hint_style = AnsiStyle {
        fg: Some(palette.code_comment),
        dim: true,
        ..Default::default()
    };

    // Clear box area
    for dy in 0..box_h {
        draw_row_at_styled(buf, y + dy, x, box_w, "", AnsiStyle::default());
    }

    // Border
    border_style.write_to(buf);
    for dx in 0..box_w {
        let ch = if dx == 0 || dx + 1 == box_w { '+' } else { '-' };
        buf.push_str(&goto(y, x + dx));
        buf.push(ch);
        buf.push_str(&goto(y + box_h - 1, x + dx));
        buf.push(ch);
    }
    for dy in 1..box_h.saturating_sub(1) {
        buf.push_str(&goto(y + dy, x));
        buf.push('|');
        buf.push_str(&goto(y + dy, x + box_w - 1));
        buf.push('|');
    }
    buf.push_str(render::RESET);

    // Title: month + year
    let month_name = MONTH_NAMES[app.date_month.saturating_sub(1).min(11) as usize];
    let title = format!("< {} {} >", month_name, app.date_year);
    let title_x = x + 1 + (box_w.saturating_sub(2).saturating_sub(title.len())) / 2;
    draw_row_at_styled(buf, y + 1, title_x, title.len(), &title, title_style);

    // Day headers
    let header = " Mo Tu We Th Fr Sa Su ";
    let inner_w = box_w.saturating_sub(2);
    let hdr_text: String = header.chars().take(inner_w).collect();
    buf.push_str(&goto(y + 2, x + 1));
    header_style.write_to(buf);
    buf.push_str(&hdr_text);
    buf.push_str(render::RESET);

    // Calendar grid
    let first_dow = day_of_week(app.date_year, app.date_month, 1);
    let max_days = days_in_month(app.date_year, app.date_month);

    let mut row_idx = 0;
    let mut col_idx = first_dow as usize;

    for day in 1..=max_days {
        let grid_row = y + 3 + row_idx;
        let grid_col = x + 1 + col_idx * 3;

        if grid_row < y + box_h - 1 {
            buf.push_str(&goto(grid_row, grid_col));
            if day == app.date_day {
                selected_day_style.write_to(buf);
            } else {
                day_style.write_to(buf);
            }
            buf.push_str(&format!("{:>2}", day));
            buf.push_str(render::RESET);
        }

        col_idx += 1;
        if col_idx >= 7 {
            col_idx = 0;
            row_idx += 1;
        }
    }

    let action = if app.date_picker_action == DatePickerAction::SetNotify {
        "notify"
    } else {
        "date"
    };
    let time_label = if app.date_require_time {
        format!(
            "Time {:02}:{:02} (required)",
            app.date_hour, app.date_minute
        )
    } else {
        let state = if app.date_include_time { "on" } else { "off" };
        format!(
            "Time {:02}:{:02} ({state}, Tab toggle)",
            app.date_hour, app.date_minute
        )
    };
    let selected = if app.date_include_time {
        format_datetime_with_pattern(
            app.date_year,
            app.date_month,
            app.date_day,
            app.date_hour,
            app.date_minute,
            &app.date_time_format,
        )
    } else {
        format_datetime_with_pattern(
            app.date_year,
            app.date_month,
            app.date_day,
            app.date_hour,
            app.date_minute,
            &app.date_format,
        )
    };
    draw_row_at_styled(
        buf,
        y + box_h - 5,
        x + 2,
        inner_w.saturating_sub(2),
        &time_label,
        time_style,
    );
    let hint = format!("{action}: h/l hour  j/k minute  Enter confirm");
    draw_row_at_styled(
        buf,
        y + box_h - 4,
        x + 2,
        inner_w.saturating_sub(2),
        &hint,
        hint_style,
    );
    let footer_x = x + 1 + (inner_w.saturating_sub(selected.chars().count())) / 2;
    buf.push_str(&goto(y + box_h - 2, footer_x));
    footer_style.write_to(buf);
    buf.push_str(&selected);
    buf.push_str(render::RESET);
}

fn goto(row: usize, col: usize) -> String {
    format!("\x1b[{};{}H", row.max(1), col.max(1))
}

fn terminal_size() -> (usize, usize) {
    let mut ws = MaybeUninit::<libc::winsize>::zeroed();
    let ok = unsafe {
        libc::ioctl(
            libc::STDOUT_FILENO,
            libc::TIOCGWINSZ,
            ws.as_mut_ptr() as *mut libc::c_void,
        )
    };
    if ok == 0 {
        let ws = unsafe { ws.assume_init() };
        let rows = usize::from(ws.ws_row.max(1));
        let cols = usize::from(ws.ws_col.max(1));
        (rows, cols)
    } else {
        (24, 80)
    }
}

fn read_key() -> Result<Option<Key>, String> {
    let Some(first) = read_byte()? else {
        return Ok(None);
    };

    if first == b'\x1b' {
        return parse_escape_sequence();
    }
    if first == b'\r' || first == b'\n' {
        return Ok(Some(Key::Enter));
    }
    if first == b'\t' {
        return Ok(Some(Key::Tab));
    }
    if first == 127 || first == 8 {
        return Ok(Some(Key::Backspace));
    }
    if (1..=26).contains(&first) {
        let c = (b'a' + (first - 1)) as char;
        return Ok(Some(Key::Ctrl(c)));
    }
    if first.is_ascii() {
        return Ok(Some(Key::Char(first as char)));
    }

    let needed = utf8_continuation_count(first);
    if needed == 0 {
        return Ok(None);
    }

    let mut bytes = vec![first];
    for _ in 0..needed {
        if let Some(b) = read_byte()? {
            bytes.push(b);
        } else {
            return Ok(None);
        }
    }
    if let Ok(text) = std::str::from_utf8(&bytes) {
        if let Some(ch) = text.chars().next() {
            return Ok(Some(Key::Char(ch)));
        }
    }
    Ok(None)
}

fn utf8_continuation_count(first: u8) -> usize {
    if first & 0b1110_0000 == 0b1100_0000 {
        1
    } else if first & 0b1111_0000 == 0b1110_0000 {
        2
    } else if first & 0b1111_1000 == 0b1111_0000 {
        3
    } else {
        0
    }
}

fn read_bracketed_paste_payload() -> Result<String, String> {
    const END: &[u8] = b"\x1b[201~";
    let mut payload = Vec::new();
    let mut idle_ticks = 0usize;

    loop {
        match read_byte()? {
            Some(byte) => {
                idle_ticks = 0;
                payload.push(byte);
                if payload.len() >= END.len() && payload.ends_with(END) {
                    payload.truncate(payload.len() - END.len());
                    break;
                }
            }
            None => {
                // VTIME=1 means 100ms per empty read; bail out after a short
                // idle window so malformed/partial sequences don't hang input.
                idle_ticks += 1;
                if idle_ticks >= 8 {
                    break;
                }
            }
        }
    }

    Ok(String::from_utf8_lossy(&payload).into_owned())
}

fn parse_escape_sequence() -> Result<Option<Key>, String> {
    let Some(second) = read_byte()? else {
        return Ok(Some(Key::Esc));
    };
    if second != b'[' && second != b'O' {
        return Ok(Some(Key::Esc));
    }

    let mut seq = Vec::new();
    loop {
        let Some(b) = read_byte()? else {
            break;
        };
        seq.push(b);
        if b.is_ascii_alphabetic() || b == b'~' {
            break;
        }
    }

    if seq.is_empty() {
        return Ok(Some(Key::Esc));
    }

    let last = seq[seq.len() - 1];
    if seq.len() == 1 {
        match last {
            b'A' => return Ok(Some(Key::ArrowUp)),
            b'B' => return Ok(Some(Key::ArrowDown)),
            b'C' => return Ok(Some(Key::ArrowRight)),
            b'D' => return Ok(Some(Key::ArrowLeft)),
            b'H' => return Ok(Some(Key::Home)),
            b'F' => return Ok(Some(Key::End)),
            b'Z' => return Ok(Some(Key::BackTab)),
            _ => return Ok(Some(Key::Esc)),
        }
    } else {
        let s = std::str::from_utf8(&seq).unwrap_or("");
        if s == "200~" {
            let pasted = read_bracketed_paste_payload()?;
            return Ok(Some(Key::Paste(pasted)));
        }
        if s == "201~" {
            return Ok(None);
        }
        if s == "1;5C" || s == "5C" {
            return Ok(Some(Key::CtrlArrowRight));
        }
        if s == "1;5D" || s == "5D" {
            return Ok(Some(Key::CtrlArrowLeft));
        }
        if s == "1~" || s == "7~" {
            return Ok(Some(Key::Home));
        }
        if s == "4~" || s == "8~" {
            return Ok(Some(Key::End));
        }
        if s == "3~" {
            return Ok(Some(Key::Delete));
        }
        if s == "5~" {
            return Ok(Some(Key::PageUp));
        }
        if s == "6~" {
            return Ok(Some(Key::PageDown));
        }
    }

    Ok(Some(Key::Esc))
}

fn read_byte() -> Result<Option<u8>, String> {
    let mut buf = [0u8; 1];
    let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, 1) };
    if n == 0 {
        return Ok(None);
    }
    if n < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::WouldBlock {
            return Ok(None);
        }
        return Err(format!("Failed to read stdin: {err}"));
    }
    Ok(Some(buf[0]))
}

struct TerminalGuard {
    original: libc::termios,
}

impl TerminalGuard {
    fn enter() -> Result<Self, String> {
        let mut term = MaybeUninit::<libc::termios>::zeroed();
        let ok = unsafe { libc::tcgetattr(libc::STDIN_FILENO, term.as_mut_ptr()) };
        if ok != 0 {
            return Err(format!(
                "Failed to read terminal attributes: {}",
                io::Error::last_os_error()
            ));
        }
        let original = unsafe { term.assume_init() };
        let mut raw = original;

        raw.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        raw.c_oflag &= !(libc::OPOST);
        raw.c_cflag |= libc::CS8;
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 1;

        let ok = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) };
        if ok != 0 {
            return Err(format!(
                "Failed to enable raw terminal mode: {}",
                io::Error::last_os_error()
            ));
        }

        let mut out = io::stdout();
        out.write_all(b"\x1b[?1049h\x1b[?2004h\x1b[?25l\x1b[H\x1b[2J")
            .and_then(|_| out.flush())
            .map_err(|e| format!("Failed to initialize terminal screen: {e}"))?;

        Ok(Self { original })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original) };
        let mut out = io::stdout();
        let _ = out.write_all(b"\x1b[0m\x1b[?2004l\x1b[?25h\x1b[?1049l\x1b[0 q");
        let _ = out.flush();
    }
}

struct CalcData {
    line_results: Vec<Option<String>>,
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

    CalcData {
        line_results: result.line_results,
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

struct TableFormulaSegment {
    from_byte: usize,
    to_byte: usize,
    from_char: usize,
    to_char: usize,
    label: String,
}

#[cfg(test)]
fn builtin_formula_label(text: &str) -> Option<String> {
    crate::editor_core::calc_plan::builtin_formula_label(text)
}

fn find_table_formula_segment(text: &str) -> Option<TableFormulaSegment> {
    let segment = crate::editor_core::calc_plan::find_table_formula_segment(text)?;
    Some(TableFormulaSegment {
        from_byte: segment.from_byte,
        to_byte: segment.to_byte,
        from_char: segment.from_char,
        to_char: segment.to_char,
        label: segment.label,
    })
}

fn format_formula_display_value(raw: &str) -> String {
    crate::editor_core::calc_plan::format_formula_display_value(raw)
}

fn contains_assignment_operator(text: &str) -> bool {
    crate::editor_core::calc_plan::contains_assignment_operator(text)
}

#[cfg(test)]
mod tests {
    use super::{
        builtin_formula_label, compute_calc_results, compute_calc_trailer_refresh,
        display_cols_for_prefix, find_calc_segment_range, find_table_formula_segment,
        format_formula_display_value, rendered_line_display_cols,
    };
    use super::{line_char_len, Key, TerminalApp, TerminalOptions, UiMode};
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
    fn rendered_line_display_cols_accounts_for_calc_ghost() {
        assert_eq!(rendered_line_display_cols("2 + 2", Some("4")), 9);
        assert_eq!(rendered_line_display_cols("x := 1", Some("2")), 10);
    }

    #[test]
    fn horizontal_scroll_clamps_to_last_visible_window_at_line_end_in_normal_mode() {
        let long = "a".repeat(200);
        let (_db, mut app, path) = app_with_note(&long);
        app.mode = UiMode::Normal;
        app.cursor_line = 0;
        app.cursor_col = line_char_len(app.current_line());
        app.adjust_scroll();

        let (_rows, cols) = super::terminal_size();
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

        let (_rows, cols) = super::terminal_size();
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
        let (rows, cols) = super::terminal_size();
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
        let (rows, cols) = super::terminal_size();
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

        let (rows, cols) = super::terminal_size();
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
        assert_eq!(seg.label, "sum_col()");
        assert!(seg.from_char < seg.to_char);
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

        assert_eq!(app.lines, vec!["|| two |".to_string()]);
        assert_eq!(app.cursor_col, 1);

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
