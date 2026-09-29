use super::input::Key;
use crate::terminal::session::CursorPlacement;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;
use super::{
    build_variable_suggestions, builtin_formula_label, compute_calc_results,
    compute_calc_trailer_refresh, extract_variable_completion_prefix, find_calc_segment_range,
    find_table_formula_segment, format_formula_display_value, rendered_line_display_cols,
    should_mask_formula_cell, table_cell_info_at_char, table_cell_is_empty,
    table_cell_navigation_anchor,
};
use super::{display_cols_for_prefix, line_char_len};
use super::{TerminalApp, TerminalOptions, UiMode, VimRegisterMode};
use crate::storage::Db;
use crate::terminal::folding::describe_fold_ranges;
use app_core::storage::NoteAccessMode;
use serde::Deserialize;
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
        open_switcher: false,
        open_at_end: false,
    };
    let (mut app, _) = TerminalApp::new_with_startup_metrics(
        &db,
        &opts,
        crate::config::ThemeConfig::default(),
        true,
        true,
        false,
        true,
        true,
        3,
        super::render::RenderPalette::default(),
        "%Y-%m-%d".to_string(),
        "%Y-%m-%d %H:%M".to_string(),
        std::sync::Arc::new(std::sync::Mutex::new(
            app_core::cross_note::CrossNoteVarIndex::default(),
        )),
    )
    .expect("terminal app");
    app.mode = UiMode::Editor;
    app.perf_trace.enabled = false;
    (db, app, path)
}

/// Renders the app into an off-screen ratatui buffer sized like the terminal.
fn render_screen(app: &mut TerminalApp) -> (Vec<String>, CursorPlacement) {
    let (rows, cols) = super::input::terminal_size();
    let mut buf = Buffer::empty(Rect::new(0, 0, cols as u16, rows as u16));
    let cursor = app.render_to_buffer(&mut buf);
    (buffer_rows(&buf), cursor)
}

fn buffer_rows(buf: &Buffer) -> Vec<String> {
    let area = buf.area;
    (area.top()..area.bottom())
        .map(|y| {
            let mut row = String::new();
            let mut x = area.left();
            while x < area.right() {
                let symbol = buf[(x, y)].symbol();
                row.push_str(symbol);
                x += (UnicodeWidthStr::width(symbol) as u16).max(1);
            }
            row
        })
        .collect()
}

fn screen_rows(app: &mut TerminalApp) -> Vec<String> {
    render_screen(app).0
}

fn screen_text(app: &mut TerminalApp) -> String {
    screen_rows(app).join("\n")
}

fn first_editor_row_for(rows: &[String], line_no: usize) -> String {
    let prefix = format!("{line_no}  ");
    rows.iter()
        .find(|row| row.trim_start().starts_with(&prefix))
        .cloned()
        .unwrap_or_default()
}

#[test]
fn editor_right_arrow_exits_inline_formatting_boundary_before_advancing() {
    let (db, mut app, path) = app_with_note("**bold** tail");
    app.editor.cursor_col = 8;

    app.handle_editor_key(&db, Key::ArrowRight)
        .expect("right advances while exiting boundary");
    assert_eq!(app.editor.cursor_col, 9);
    assert_eq!(
        app.editor.markdown_formatting_right_boundary_exit,
        Some((0, 8))
    );

    app.handle_editor_key(&db, Key::ArrowRight)
        .expect("second right advances normally");
    assert_eq!(app.editor.cursor_col, 10);
    assert_eq!(app.editor.markdown_formatting_right_boundary_exit, None);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn editor_right_arrow_snaps_at_formatting_boundary_when_no_forward_motion_exists() {
    let (db, mut app, path) = app_with_note("**bold**");
    app.editor.cursor_col = 8;

    app.handle_editor_key(&db, Key::ArrowRight)
        .expect("right snaps boundary in no-move edge case");
    assert_eq!(app.editor.cursor_col, 8);
    assert_eq!(
        app.editor.markdown_formatting_right_boundary_exit,
        Some((0, 8))
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn editor_right_boundary_exit_snaps_closing_strong_markers_before_next_right() {
    let line = "Capability is **an action** linked to the entity type or resource type";
    let (db, mut app, path) = app_with_note(line);
    app.editor.cursor_col = "Capability is **an action**".chars().count();

    let revealed = first_editor_row_for(&screen_rows(&mut app), 1);
    assert!(
        revealed.contains("**an action**"),
        "expected right boundary reveal before exit, got:\n{revealed}"
    );

    app.handle_editor_key(&db, Key::ArrowRight)
        .expect("right exits boundary");
    let snapped_after_first = first_editor_row_for(&screen_rows(&mut app), 1);
    assert!(
        snapped_after_first.contains("an action linked"),
        "expected markers snapped after first right, got:\n{snapped_after_first}"
    );
    assert!(
        !snapped_after_first.contains("**an action**"),
        "closing strong markers stayed revealed after first right:\n{snapped_after_first}"
    );

    let col_after_first = app.editor.cursor_col;
    app.handle_editor_key(&db, Key::ArrowRight)
        .expect("second right continues forward");
    assert!(
        app.editor.cursor_col > col_after_first,
        "second right should advance source cursor forward"
    );
    let snapped_after_second = first_editor_row_for(&screen_rows(&mut app), 1);
    assert!(
        snapped_after_second.contains("an action linked"),
        "markers should stay snapped after second right, got:\n{snapped_after_second}"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn editor_left_arrow_restores_inline_formatting_boundary_reveal() {
    let (db, mut app, path) = app_with_note("**bold**");
    app.editor.cursor_col = 8;
    app.handle_editor_key(&db, Key::ArrowRight)
        .expect("right snaps boundary in no-move edge case");
    app.handle_editor_key(&db, Key::ArrowLeft)
        .expect("left restores boundary reveal");
    assert_eq!(app.editor.cursor_col, 8);
    assert_eq!(app.editor.markdown_formatting_right_boundary_exit, None);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

fn app_with_note_and_modules(
    body: &str,
    modules: app_core::storage::NoteModules,
) -> (Db, TerminalApp, PathBuf) {
    let path = temp_db_path();
    let db = Db::open(path.clone()).expect("db opens");
    let note_id = "n1";
    db.create_note_with_defaults(note_id, modules, None)
        .expect("create note with modules");
    db.save_note(note_id, body).expect("note saved");
    let opts = TerminalOptions {
        create_new: false,
        note_id: Some(note_id.to_string()),
        list_only: false,
        open_switcher: false,
        open_at_end: false,
    };
    let (mut app, _) = TerminalApp::new_with_startup_metrics(
        &db,
        &opts,
        crate::config::ThemeConfig::default(),
        true,
        true,
        false,
        true,
        true,
        3,
        super::render::RenderPalette::default(),
        "%Y-%m-%d".to_string(),
        "%Y-%m-%d %H:%M".to_string(),
        std::sync::Arc::new(std::sync::Mutex::new(
            app_core::cross_note::CrossNoteVarIndex::default(),
        )),
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
        cross_note: true,
    }
}

fn note_modules_with_variables(enabled: bool) -> app_core::storage::NoteModules {
    note_modules(true, true, enabled, true)
}

#[derive(Debug, Deserialize)]
struct VimReplaySuite {
    cases: Vec<VimReplayCase>,
}

#[derive(Debug, Deserialize)]
struct VimReplayCase {
    name: String,
    initial_text: String,
    #[serde(default)]
    initial_state: crate::editor_core::vim::VimState,
    #[serde(default)]
    initial_cursor_line: usize,
    #[serde(default)]
    initial_cursor_col: usize,
    keys: Vec<String>,
    expected: Option<VimReplayExpected>,
}

#[derive(Debug, Deserialize)]
struct VimReplayExpected {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
    mode: String,
    vim_state: crate::editor_core::vim::VimState,
    selection_anchor: Option<(usize, usize)>,
    clipboard_text: String,
    clipboard_mode: String,
}

#[derive(Debug, Deserialize)]
struct MarkdownReplaySuite {
    cases: Vec<MarkdownReplayCase>,
}

#[derive(Debug, Deserialize)]
struct MarkdownReplayCase {
    name: String,
    initial_text: String,
    #[serde(default)]
    initial_cursor_line: usize,
    #[serde(default)]
    initial_cursor_col: usize,
    keys: Vec<String>,
    expected: MarkdownReplaySnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VimReplaySnapshot {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
    mode: UiMode,
    vim_state: crate::editor_core::vim::VimState,
    selection_anchor: Option<(usize, usize)>,
    clipboard_text: String,
    clipboard_mode: VimRegisterMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct MarkdownReplaySnapshot {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

fn ui_mode_from_vim_mode(mode: crate::editor_core::vim::VimMode) -> UiMode {
    match mode {
        crate::editor_core::vim::VimMode::Insert => UiMode::Editor,
        crate::editor_core::vim::VimMode::Normal => UiMode::Normal,
        crate::editor_core::vim::VimMode::Visual => UiMode::Visual,
        crate::editor_core::vim::VimMode::VisualLine => UiMode::VisualLine,
    }
}

fn parse_terminal_key_token(token: &str) -> Key {
    let key = crate::editor_core::vim::parse_key_token(token)
        .unwrap_or_else(|| panic!("invalid replay key token: {token}"));
    match key {
        crate::editor_core::vim::VimKey::Esc => Key::Esc,
        crate::editor_core::vim::VimKey::Enter => Key::Enter,
        crate::editor_core::vim::VimKey::Tab => Key::Tab,
        crate::editor_core::vim::VimKey::Backspace => Key::Backspace,
        crate::editor_core::vim::VimKey::Delete => Key::Delete,
        crate::editor_core::vim::VimKey::ArrowUp => Key::ArrowUp,
        crate::editor_core::vim::VimKey::ArrowDown => Key::ArrowDown,
        crate::editor_core::vim::VimKey::ArrowLeft => Key::ArrowLeft,
        crate::editor_core::vim::VimKey::ArrowRight => Key::ArrowRight,
        crate::editor_core::vim::VimKey::Char(ch) => Key::Char(ch),
        crate::editor_core::vim::VimKey::Ctrl(ch) => Key::Ctrl(ch),
    }
}

fn parse_markdown_key_token(token: &str) -> Key {
    if let Some(raw) = token.strip_prefix("char:") {
        let ch = if raw.eq_ignore_ascii_case("space") {
            ' '
        } else {
            let mut chars = raw.chars();
            let ch = chars
                .next()
                .unwrap_or_else(|| panic!("missing char token in {token}"));
            assert!(
                chars.next().is_none(),
                "markdown replay char token must be a single character: {token}"
            );
            ch
        };
        return Key::Char(ch);
    }

    match token {
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "backtab" | "shift-tab" => Key::BackTab,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "ctrl-backspace" => Key::CtrlBackspace,
        "ctrl-delete" => Key::CtrlDelete,
        "ctrl-w" => Key::Ctrl('w'),
        _ => panic!("invalid markdown replay key token: {token}"),
    }
}

fn run_vim_replay_case(case: &VimReplayCase) -> VimReplaySnapshot {
    let (db, mut app, path) = app_with_note(&case.initial_text);
    app.mode = ui_mode_from_vim_mode(case.initial_state.mode);
    app.vim_state = case.initial_state.clone();
    if app.editor.lines.is_empty() {
        app.editor.lines.push(String::new());
    }
    app.editor.cursor_line = case
        .initial_cursor_line
        .min(app.editor.lines.len().saturating_sub(1));
    app.editor.cursor_col = case.initial_cursor_col;
    if matches!(app.mode, UiMode::Visual | UiMode::VisualLine) {
        app.editor.selection_anchor = Some((app.editor.cursor_line, app.editor.cursor_col));
    }
    app.adjust_cursor();

    for token in &case.keys {
        let key = parse_terminal_key_token(token);
        app.handle_key(&db, key).unwrap_or_else(|err| {
            panic!(
                "vim replay case '{}' failed on key '{}': {}",
                case.name, token, err
            )
        });
    }
    app.adjust_cursor();

    let snapshot = VimReplaySnapshot {
        lines: app.editor.lines.clone(),
        cursor_line: app.editor.cursor_line,
        cursor_col: app.editor.cursor_col,
        mode: app.mode,
        vim_state: app.vim_state.clone(),
        selection_anchor: app.editor.selection_anchor,
        clipboard_text: app.clipboard.text.clone(),
        clipboard_mode: app.clipboard.mode,
    };

    drop(app);
    drop(db);
    cleanup_db_files(&path);
    snapshot
}

fn run_markdown_replay_case(case: &MarkdownReplayCase) -> MarkdownReplaySnapshot {
    let (db, mut app, path) = app_with_note(&case.initial_text);
    app.mode = UiMode::Editor;
    if app.editor.lines.is_empty() {
        app.editor.lines.push(String::new());
    }
    app.editor.cursor_line = case
        .initial_cursor_line
        .min(app.editor.lines.len().saturating_sub(1));
    app.editor.cursor_col = case.initial_cursor_col;
    app.adjust_cursor();

    for token in &case.keys {
        let key = parse_markdown_key_token(token);
        app.handle_key(&db, key).unwrap_or_else(|err| {
            panic!(
                "markdown replay case '{}' failed on key '{}': {}",
                case.name, token, err
            )
        });
    }
    app.adjust_cursor();

    let snapshot = MarkdownReplaySnapshot {
        lines: app.editor.lines.clone(),
        cursor_line: app.editor.cursor_line,
        cursor_col: app.editor.cursor_col,
    };

    drop(app);
    drop(db);
    cleanup_db_files(&path);
    snapshot
}

#[cfg(test)]
#[path = "tests/calc_table.rs"]
mod calc_table;
#[cfg(test)]
#[path = "tests/command_security_switcher.rs"]
mod command_security_switcher;
#[cfg(test)]
#[path = "tests/layout_folding.rs"]
mod layout_folding;
#[cfg(test)]
#[path = "tests/replay.rs"]
mod replay;
#[cfg(test)]
#[path = "tests/vim.rs"]
mod vim;

#[cfg(test)]
#[path = "tests/wrap.rs"]
mod wrap;

#[path = "tests/large_note_perf.rs"]
mod large_note_perf;
