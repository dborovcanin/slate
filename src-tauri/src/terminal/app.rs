use super::render;

use crate::config::ThemeConfig;
use crate::startup_log::append_startup_log_line;
use crate::storage::{Db, Note};
use app_core::calc::CalcEngine;
use base64::Engine as _;
use std::cmp::min;
use std::io::{self, IsTerminal as _, Write};
use std::mem::MaybeUninit;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use ulid::Ulid;

const AUTOSAVE_DEBOUNCE_MS: u64 = 500;
const TITLE_ROW: usize = 1;
const EDITOR_TOP_ROW: usize = 2;
const GUTTER_WIDTH: usize = 6;

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
enum Key {
    Char(char),
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

#[derive(Debug, Clone, Copy, Default)]
struct TerminalStartupMetrics {
    loading_note: Duration,
    loading_switcher: Duration,
    loading_calc_engine: Duration,
    loading_screen: Duration,
}

struct TerminalApp {
    active_note: Note,
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize, // char index
    scroll_line: usize,
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
    // Vim state
    vim_state: crate::editor_core::vim::VimState,
    clipboard: Vec<String>,
    last_clipboard_backend: Option<ClipboardWriteBackend>,
    selection_anchor: Option<(usize, usize)>, // (line, col)
    // Calc ghost cache
    calc_engine: CalcEngine,
    calc_results: Vec<Option<String>>,
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
    // Calc/variables behavior
    variables_enabled: bool,
    // Track which mode entered command bar from
    command_bar_from_normal: bool,
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
        variables_enabled: bool,
    ) -> Result<(Self, TerminalStartupMetrics), String> {
        let startup_begin = Instant::now();

        let note_begin = Instant::now();
        let active_note = select_note(db, opts)?;
        let loading_note = note_begin.elapsed();

        let lines = split_lines(&active_note.body);

        let switcher_begin = Instant::now();
        let switcher_items = load_note_meta(db)?;
        let loading_switcher = switcher_begin.elapsed();

        let calc_engine = CalcEngine::new();
        let calc_begin = Instant::now();
        let calc_data = compute_calc_data(&calc_engine, &lines, variables_enabled);
        let loading_calc_engine = calc_begin.elapsed();

        let prev_lines_snapshot = lines.clone();
        let undo_seed = lines.clone();

        let app = Self {
            active_note,
            lines,
            cursor_line: 0,
            cursor_col: 0,
            scroll_line: 0,
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
            vim_state: crate::editor_core::vim::VimState::default(),
            clipboard: Vec::new(),
            last_clipboard_backend: None,
            selection_anchor: None,
            calc_engine,
            calc_results: calc_data.line_results,
            variable_names: calc_data.variable_names,
            prev_lines: prev_lines_snapshot,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_current: 0,
            search_orig_line: 0,
            search_orig_col: 0,
            search_orig_scroll: 0,
            format_on_save,
            variables_enabled,
            command_bar_from_normal: false,
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

    fn handle_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
        match self.mode {
            UiMode::DatePicker => self.handle_date_picker_key(key)?,
            UiMode::Editor => self.handle_editor_key(db, key)?,
            UiMode::Normal => self.handle_normal_key(db, key)?,
            UiMode::Visual | UiMode::VisualLine => self.handle_visual_key(db, key)?,
            UiMode::Switcher => self.handle_switcher_key(db, key)?,
            UiMode::CommandBar => self.handle_command_bar_key(key)?,
            UiMode::Search => self.handle_search_key(key)?,
        }
        Ok(())
    }

    fn handle_editor_key(&mut self, db: &Db, key: Key) -> Result<(), String> {
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
                self.set_active_note(note);
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
                self.mode = UiMode::CommandBar;
                self.status = ":".to_string();
                return Ok(());
            }
            Key::Ctrl('f') => {
                self.open_search();
                return Ok(());
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

        self.try_autoformat_rules();

        Ok(())
    }

    fn map_vim_key(key: Key) -> Option<crate::editor_core::vim::VimKey> {
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
            Key::Char(ch) => Some(crate::editor_core::vim::VimKey::Char(ch)),
            Key::Ctrl(ch) => Some(crate::editor_core::vim::VimKey::Ctrl(ch)),
            _ => None,
        }
    }

    fn set_clipboard_lines(&mut self, lines: Vec<String>) -> Option<ClipboardWriteBackend> {
        if lines.is_empty() {
            return None;
        }
        let joined = lines.join("\n");
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
                    self.cursor_col = line_char_len(self.current_line())
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
                    self.move_cursor_right();
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
        if key == Key::Ctrl('p') {
            self.open_switcher(db)?;
            return Ok(());
        }

        let Some(vim_key) = Self::map_vim_key(key) else {
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
            markdown_autoformat: true,
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
                self.status = "-- NORMAL --".to_string();
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
                'w' => self.move_cursor_right_word(),
                'b' => self.move_cursor_left_word(),
                '$' => self.cursor_col = line_char_len(self.current_line()),
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
                        self.set_active_note(note);
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

    fn handle_command_bar_key(&mut self, key: Key) -> Result<(), String> {
        match key {
            Key::Esc => {
                self.mode = if self.command_bar_from_normal {
                    UiMode::Normal
                } else {
                    UiMode::Editor
                };
                self.command_input.clear();
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
                self.execute_terminal_command(&cmd);
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

    fn execute_terminal_command(&mut self, cmd: &str) {
        if cmd == "q!" || cmd == "q" {
            self.force_quit = cmd == "q!";
            self.quit = true;
            return;
        }

        if cmd == "date" {
            self.open_date_picker();
            return;
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

    fn build_snapshot(&self) -> crate::editor_core::types::EditorContextSnapshot {
        let text = join_lines(&self.lines);
        // Convert cursor_line/cursor_col to byte offset
        let mut offset = 0;
        for (i, line) in self.lines.iter().enumerate() {
            if i == self.cursor_line {
                offset += byte_index(line, self.cursor_col);
                break;
            }
            offset += line.len() + 1; // +1 for \n
        }
        crate::editor_core::types::EditorContextSnapshot {
            text,
            selection: crate::editor_core::types::SelectionSnapshot {
                anchor: offset,
                head: offset,
            },
            changed_range: None,
        }
    }

    fn open_date_picker(&mut self) {
        let now = time::OffsetDateTime::now_utc().date();
        self.date_year = now.year();
        self.date_month = now.month() as u32;
        self.date_day = now.day() as u32;
        self.mode = UiMode::DatePicker;
        self.status = "Date picker: arrows navigate, Enter insert, Esc cancel".to_string();
    }

    fn handle_date_picker_key(&mut self, key: Key) -> Result<(), String> {
        match key {
            Key::Esc => {
                self.mode = UiMode::Editor;
                self.status = format!("editing {}", self.active_note.id);
            }
            Key::Enter => {
                let date_str = format!(
                    "{:04}-{:02}-{:02}",
                    self.date_year, self.date_month, self.date_day
                );
                self.mode = UiMode::Editor;
                self.insert_text(&date_str);
                self.status = "Date inserted".to_string();
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
            self.execute_terminal_command("format");
        }
        if !self.dirty {
            return Ok(());
        }
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

    fn refresh_switcher_items(&mut self, db: &Db) -> Result<(), String> {
        self.switcher_items = load_note_meta(db)?;
        if self.mode == UiMode::Switcher {
            self.recompute_switcher_matches();
        }
        Ok(())
    }

    fn set_active_note(&mut self, note: Note) {
        self.active_note = note;
        self.lines = split_lines(&self.active_note.body);
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.scroll_line = 0;
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
        let calc_data = compute_calc_data(&self.calc_engine, &self.lines, self.variables_enabled);
        let mut new_results = calc_data.line_results;

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
            let selection_range: Option<(usize, usize)> =
                self.selection_anchor.map(|(anchor_line, _)| {
                    let a = anchor_line.min(cursor_line);
                    let b = anchor_line.max(cursor_line);
                    (a, b)
                });

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
        self.variable_names = calc_data.variable_names;
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

    fn search_highlights_for_line(&self, line_idx: usize) -> Vec<(usize, usize)> {
        self.search_matches
            .iter()
            .filter(|&&(l, _, _)| l == line_idx)
            .map(|&(_, s, e)| (s, e))
            .collect()
    }

    fn append_visual_highlights(&self, line_idx: usize, ranges: &mut Vec<(usize, usize)>) {
        let Some(anchor) = self.selection_anchor else {
            return;
        };
        if self.mode != UiMode::Visual && self.mode != UiMode::VisualLine {
            return;
        }

        let start_line = min(anchor.0, self.cursor_line);
        let end_line = std::cmp::max(anchor.0, self.cursor_line);

        if line_idx < start_line || line_idx > end_line {
            return;
        }

        if self.mode == UiMode::VisualLine {
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
            markdown_autoformat: true,
        };
        if let Some(op) = crate::editor_core::text_rules::run_doc_change_rules(&snapshot, options) {
            self.apply_edit_operation(&op);
        }
    }

    fn try_enter_rule(&mut self) -> bool {
        let snapshot = self.build_snapshot();
        let options = crate::editor_core::text_rules::TextRuleOptions {
            markdown_autoformat: true,
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
            markdown_autoformat: true,
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
            markdown_autoformat: true,
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
        draw_row(&mut buf, TITLE_ROW, cols, &title_line, true);

        let mut ctx = render::RenderContext::new();
        ctx.advance_lines(&self.lines[..self.scroll_line.min(self.lines.len())]);
        let mut cursor_line_override: Option<(String, usize)> = None;

        for i in 0..editor_height {
            let row = EDITOR_TOP_ROW + i;
            let line_idx = self.scroll_line + i;
            if line_idx < self.lines.len() {
                let line_no = line_idx + 1;
                let available = cols.saturating_sub(GUTTER_WIDTH);
                let is_cursor_line = line_idx == self.cursor_line;
                let mut calc_ghost = self.calc_results.get(line_idx).and_then(|r| r.as_deref());
                let mut calc_ghost_override: Option<String> = None;
                let mut ghost_dim_ranges: Vec<(usize, usize)> = Vec::new();
                let line_text = &self.lines[line_idx];
                let mut rendered_line = line_text.to_string();

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

                let mut highlight_ranges = self.search_highlights_for_line(line_idx);
                self.append_visual_highlights(line_idx, &mut highlight_ranges);

                let rendered_text = if ghost_dim_ranges.is_empty() {
                    ctx.render_line(
                        &rendered_line,
                        available,
                        calc_ghost_override.as_deref().or(calc_ghost),
                        &highlight_ranges,
                        &self.variable_names,
                    )
                } else {
                    ctx.render_line_with_dim_ranges(
                        &rendered_line,
                        available,
                        calc_ghost_override.as_deref().or(calc_ghost),
                        &highlight_ranges,
                        &self.variable_names,
                        &ghost_dim_ranges,
                    )
                };
                buf.push_str(&goto(row, 1));
                // Dim gutter
                if is_cursor_line {
                    buf.push_str(render::BOLD);
                } else {
                    buf.push_str(render::DIM);
                }
                buf.push_str(&format!("{line_no:>4}  "));
                buf.push_str(render::RESET);
                buf.push_str(&rendered_text);
            } else {
                buf.push_str(&goto(row, 1));
                buf.push_str(render::DIM);
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
        draw_row(&mut buf, rows, cols, status, true);

        if self.mode == UiMode::Switcher {
            draw_switcher(self, &mut buf, rows, cols);
        }

        if self.mode == UiMode::DatePicker {
            draw_date_picker(self, &mut buf, rows, cols);
        }

        let (cursor_row, mut cursor_col) = self.cursor_position(rows, cols);
        if let Some((line_text, mapped_col)) = cursor_line_override {
            if matches!(
                self.mode,
                UiMode::Editor | UiMode::Normal | UiMode::Visual | UiMode::VisualLine
            ) {
                let visible_col = visible_display_cols_for_prefix(
                    &line_text,
                    mapped_col,
                    cols.saturating_sub(GUTTER_WIDTH),
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
                let clamped_col = min(self.cursor_col, line_char_len(line_text));
                let visible_col = visible_display_cols_for_prefix(
                    line_text,
                    clamped_col,
                    cols.saturating_sub(GUTTER_WIDTH),
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
        config.variables_enabled,
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

fn visible_display_cols_for_prefix(text: &str, prefix_chars: usize, max_cols: usize) -> usize {
    let mut visible = 0usize;
    for ch in text.chars().take(prefix_chars) {
        if visible >= max_cols {
            break;
        }
        if ch == '\t' {
            let tab = render::TAB_WIDTH - (visible % render::TAB_WIDTH);
            visible = (visible + tab).min(max_cols);
        } else {
            visible += 1;
        }
    }
    visible.min(max_cols)
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

fn draw_row_at(buf: &mut String, row: usize, col: usize, width: usize, text: &str, inverted: bool) {
    buf.push_str(&goto(row, col));
    if inverted {
        buf.push_str("\x1b[7m");
    }
    buf.push_str(&pad_right(text, width));
    if inverted {
        buf.push_str("\x1b[0m");
    }
}

fn draw_row(buf: &mut String, row: usize, width: usize, text: &str, inverted: bool) {
    draw_row_at(buf, row, 1, width, text, inverted);
}

fn draw_switcher(app: &TerminalApp, buf: &mut String, rows: usize, cols: usize) {
    let box_w = min(cols.saturating_sub(4).max(30), 72);
    let box_h = min(rows.saturating_sub(4).max(8), 14);
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;

    // Border
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

    let prompt = format!(" search: {}", app.switcher_query);
    draw_row_at(buf, y + 1, x + 1, box_w.saturating_sub(2), &prompt, false);
    draw_row_at(
        buf,
        y + 2,
        x + 1,
        box_w.saturating_sub(2),
        " results:",
        false,
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
            draw_row_at(
                buf,
                row,
                x + 1,
                box_w.saturating_sub(2),
                &text,
                start + i == app.switcher_selected,
            );
        } else {
            draw_row_at(buf, row, x + 1, box_w.saturating_sub(2), "", false);
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

fn draw_date_picker(app: &TerminalApp, buf: &mut String, rows: usize, cols: usize) {
    let box_w: usize = 30;
    let box_h: usize = 12;
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;

    // Clear box area
    for dy in 0..box_h {
        draw_row_at(buf, y + dy, x, box_w, "", false);
    }

    // Border
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

    // Title: month + year
    let month_name = MONTH_NAMES[app.date_month.saturating_sub(1).min(11) as usize];
    let title = format!("< {} {} >", month_name, app.date_year);
    let title_x = x + 1 + (box_w.saturating_sub(2).saturating_sub(title.len())) / 2;
    draw_row_at(buf, y + 1, title_x, title.len(), &title, false);

    // Day headers
    let header = " Mo Tu We Th Fr Sa Su ";
    let inner_w = box_w.saturating_sub(2);
    let hdr_text: String = header.chars().take(inner_w).collect();
    buf.push_str(&goto(y + 2, x + 1));
    buf.push_str("\x1b[2m");
    buf.push_str(&hdr_text);
    buf.push_str("\x1b[0m");

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
                buf.push_str("\x1b[7m"); // reverse
            }
            buf.push_str(&format!("{:>2}", day));
            if day == app.date_day {
                buf.push_str("\x1b[0m");
            }
        }

        col_idx += 1;
        if col_idx >= 7 {
            col_idx = 0;
            row_idx += 1;
        }
    }

    // Footer
    let selected = format!(
        "{:04}-{:02}-{:02}",
        app.date_year, app.date_month, app.date_day
    );
    let footer_x = x + 1 + (inner_w.saturating_sub(selected.len())) / 2;
    buf.push_str(&goto(y + box_h - 2, footer_x));
    buf.push_str("\x1b[1m");
    buf.push_str(&selected);
    buf.push_str("\x1b[0m");
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
        out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[H\x1b[2J")
            .and_then(|_| out.flush())
            .map_err(|e| format!("Failed to initialize terminal screen: {e}"))?;

        Ok(Self { original })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original) };
        let mut out = io::stdout();
        let _ = out.write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l\x1b[0 q");
        let _ = out.flush();
    }
}

struct CalcData {
    line_results: Vec<Option<String>>,
    variable_names: Vec<String>,
}

fn compute_calc_data(engine: &CalcEngine, lines: &[String], variables_enabled: bool) -> CalcData {
    let result = engine.evaluate_note_context(
        lines,
        app_core::calc::NoteEvaluationOptions {
            variables_enabled,
            eval_range: None,
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
    compute_calc_data(&engine, lines, variables_enabled).line_results
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
    let eq_idx = line.rfind(" = ")?;
    let current_literal = &line[eq_idx + 3..];
    if current_literal == new_result {
        return None;
    }
    if is_cursor_line {
        let eq_char_idx = line[..eq_idx].chars().count();
        if cursor_col >= eq_char_idx {
            return None;
        }
    }
    Some((eq_idx, format!(" = {new_result}")))
}

fn find_calc_segment_range(text: &str) -> Option<(usize, usize)> {
    let trimmed_text = text.trim();
    if trimmed_text.starts_with('|') && trimmed_text.ends_with('|') {
        let pipes: Vec<usize> = text.match_indices('|').map(|(i, _)| i).collect();
        if pipes.len() >= 2 {
            let mut formula_candidates = Vec::new();
            let mut candidates = Vec::new();
            for i in 0..pipes.len() - 1 {
                let start = pipes[i] + 1;
                let end = pipes[i + 1];
                if start >= end {
                    continue;
                }
                let raw = &text[start..end];
                let trimmed = raw.trim();
                if trimmed.is_empty() || !has_calc_signal(trimmed) {
                    continue;
                }
                let leading_ws = raw.len() - raw.trim_start().len();
                let trailing_ws = raw.len() - raw.trim_end().len();
                let span = (start + leading_ws, end - trailing_ws);
                if is_builtin_formula(trimmed) {
                    formula_candidates.push(span);
                } else {
                    candidates.push(span);
                }
            }
            if formula_candidates.len() == 1 {
                return Some(formula_candidates[0]);
            }
            if !formula_candidates.is_empty() {
                return None;
            }
            if candidates.len() == 1 {
                return Some(candidates[0]);
            }
        }
        return None;
    }

    let mut prefix_end = None;
    if let Some((marker_end, _)) = render::checklist_marker_end(text) {
        prefix_end = Some(marker_end);
    } else if let Some(marker_end) = render::list_marker_end(text) {
        prefix_end = Some(marker_end);
    }

    if let Some(start) = prefix_end {
        let raw = &text[start..];
        let leading_ws = raw.len() - raw.trim_start().len();
        let trailing_ws = raw.len() - raw.trim_end().len();
        let from = start + leading_ws;
        let to = text.len().saturating_sub(trailing_ws);
        if from < to {
            return Some((from, to));
        }
    }

    None
}

struct TableFormulaSegment {
    from_byte: usize,
    to_byte: usize,
    from_char: usize,
    to_char: usize,
    label: String,
}

fn builtin_formula_label(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }

    let without_equals = text.strip_prefix('=').unwrap_or(text).trim();
    if without_equals.is_empty() {
        return None;
    }

    let compact = without_equals
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let token = compact.strip_suffix("()").unwrap_or(compact.as_str());

    let normalized = match token {
        "sum_row" => "sum_row",
        "avg_row" => "avg_row",
        "sum_col" | "sum_column" => "sum_col",
        "avg_col" | "avg_column" => "avg_col",
        _ => return None,
    };

    Some(format!("{normalized}()"))
}

fn find_table_formula_segment(text: &str) -> Option<TableFormulaSegment> {
    let trimmed = text.trim();
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return None;
    }

    let (from_byte, to_byte) = find_calc_segment_range(text)?;
    let expr = text.get(from_byte..to_byte)?;
    let label = builtin_formula_label(expr)?;

    Some(TableFormulaSegment {
        from_byte,
        to_byte,
        from_char: text[..from_byte].chars().count(),
        to_char: text[..to_byte].chars().count(),
        label,
    })
}

fn format_formula_display_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut cleaned = trimmed.to_string();
    if let Some(rest) = cleaned.strip_prefix('≈') {
        cleaned = rest.trim_start().to_string();
    }
    if let Some(rest) = cleaned.strip_prefix('~') {
        cleaned = rest.trim_start().to_string();
    }
    let lowered = cleaned.to_ascii_lowercase();
    for prefix in ["approximately ", "approx. ", "approx ", "about "] {
        if lowered.starts_with(prefix) {
            cleaned = cleaned[prefix.len()..].trim_start().to_string();
            break;
        }
    }
    if cleaned.is_empty() {
        cleaned = trimmed.to_string();
    }

    let mut parts = cleaned.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim_start();
    let numeric = first.replace(',', "");

    let Ok(value) = numeric.parse::<f64>() else {
        return cleaned;
    };
    if !value.is_finite() {
        return cleaned;
    }

    let rounded = (value * 100.0).round() / 100.0;
    let mut out = format!("{rounded:.2}");
    while out.contains('.') && out.ends_with('0') {
        out.pop();
    }
    if out.ends_with('.') {
        out.pop();
    }

    if rest.is_empty() {
        out
    } else {
        format!("{out} {rest}")
    }
}

fn has_calc_signal(text: &str) -> bool {
    let trimmed = text.trim();
    if is_builtin_formula(trimmed) {
        return true;
    }

    if looks_like_date(text) {
        return false;
    }

    text.bytes().any(|b| {
        matches!(
            b,
            b'+' | b'-' | b'*' | b'/' | b'^' | b'%' | b'(' | b'0'..=b'9'
        )
    }) || text.contains(" to ")
        || text.contains(" in ")
}

fn is_builtin_formula(text: &str) -> bool {
    builtin_formula_label(text).is_some()
}

fn contains_assignment_operator(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 2 {
        return false;
    }

    for i in 0..bytes.len() - 1 {
        if bytes[i] != b':' || bytes[i + 1] != b'=' {
            continue;
        }
        if i > 0 && matches!(bytes[i - 1], b':' | b'!' | b'<' | b'>' | b'=') {
            continue;
        }
        if i + 2 < bytes.len() && bytes[i + 2] == b'=' {
            continue;
        }
        return true;
    }

    false
}

fn looks_like_date_with_delim(text: &str, delim: char) -> bool {
    let mut parts = text.split(delim);
    let (Some(a), Some(b), Some(c)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }

    let parse_len = |part: &str, min_len: usize, max_len: usize| -> Option<u32> {
        if part.len() < min_len
            || part.len() > max_len
            || !part.as_bytes().iter().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        part.parse::<u32>().ok()
    };

    if parse_len(a, 4, 4).is_some() {
        if let (Some(month), Some(day)) = (parse_len(b, 1, 2), parse_len(c, 1, 2)) {
            return (1..=12).contains(&month) && (1..=31).contains(&day);
        }
    }

    if let (Some(day), Some(month), Some(year)) =
        (parse_len(a, 1, 2), parse_len(b, 1, 2), parse_len(c, 2, 4))
    {
        return year > 0 && (1..=12).contains(&month) && (1..=31).contains(&day);
    }

    false
}

fn looks_like_date(text: &str) -> bool {
    looks_like_date_with_delim(text, '-')
        || looks_like_date_with_delim(text, '.')
        || looks_like_date_with_delim(text, '/')
}

#[cfg(test)]
mod tests {
    use super::{
        builtin_formula_label, compute_calc_results, compute_calc_trailer_refresh,
        find_calc_segment_range, find_table_formula_segment, format_formula_display_value,
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
        let (mut app, _) =
            TerminalApp::new_with_startup_metrics(&db, &opts, false, true).expect("terminal app");
        app.mode = UiMode::Editor;
        (db, app, path)
    }

    fn run_keys(app: &mut TerminalApp, db: &Db, keys: &[Key]) {
        for key in keys {
            app.handle_key(db, *key).expect("key sequence should apply");
        }
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
        assert_eq!(results[0].as_deref(), Some("4"));
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
            vec![
                Some("12".to_string()),
                Some("15".to_string()),
                Some("16".to_string())
            ]
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
