use super::input::Key;
use super::{
    build_variable_suggestions, builtin_formula_label, compute_calc_results,
    compute_calc_trailer_refresh, extract_variable_completion_prefix, find_calc_segment_range,
    find_table_formula_segment, format_formula_display_value, rendered_line_display_cols,
    should_mask_formula_cell, table_cell_info_at_char, table_cell_is_empty,
    table_cell_navigation_anchor,
};
use super::{display_cols_for_prefix, line_char_len};
use super::{TerminalApp, TerminalOptions, UiMode, VimRegister, VimRegisterMode};
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
    )
    .expect("terminal app");
    app.mode = UiMode::Editor;
    app.perf_trace.enabled = false;
    (db, app, path)
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

#[derive(Debug, Deserialize)]
struct VimParityReplaySuite {
    cases: Vec<VimParityReplayCase>,
}

#[derive(Debug, Deserialize)]
struct VimParityReplayCase {
    name: String,
    initial_text: String,
    #[serde(default)]
    initial_state: crate::editor_core::vim::VimState,
    #[serde(default)]
    initial_cursor_line: usize,
    #[serde(default)]
    initial_cursor_col: usize,
    keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MarkdownParityReplaySuite {
    cases: Vec<MarkdownParityReplayCase>,
}

#[derive(Debug, Deserialize)]
struct MarkdownParityReplayCase {
    name: String,
    initial_text: String,
    #[serde(default)]
    initial_cursor_line: usize,
    #[serde(default)]
    initial_cursor_col: usize,
    keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParitySnapshot {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
    mode: UiMode,
    vim_state: crate::editor_core::vim::VimState,
    selection_anchor: Option<(usize, usize)>,
    clipboard_text: String,
    clipboard_mode: VimRegisterMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkdownParitySnapshot {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

#[derive(Debug, Clone)]
struct GuiParityState {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
    mode: UiMode,
    selection_anchor: Option<(usize, usize)>,
    vim_state: crate::editor_core::vim::VimState,
    clipboard: VimRegister,
}

fn ui_mode_from_vim_mode(mode: crate::editor_core::vim::VimMode) -> UiMode {
    match mode {
        crate::editor_core::vim::VimMode::Insert => UiMode::Editor,
        crate::editor_core::vim::VimMode::Normal => UiMode::Normal,
        crate::editor_core::vim::VimMode::Visual => UiMode::Visual,
        crate::editor_core::vim::VimMode::VisualLine => UiMode::VisualLine,
    }
}

fn split_replay_lines(text: &str) -> Vec<String> {
    let mut lines = text
        .split('\n')
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn parse_terminal_key_token(token: &str) -> Key {
    let key = crate::editor_core::vim::parse_key_token(token)
        .unwrap_or_else(|| panic!("invalid parity key token: {token}"));
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
                "markdown parity char token must be a single character: {token}"
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
        _ => panic!("invalid markdown parity key token: {token}"),
    }
}

fn clamp_gui_cursor(state: &mut GuiParityState) {
    if state.lines.is_empty() {
        state.lines.push(String::new());
    }
    if state.cursor_line >= state.lines.len() {
        state.cursor_line = state.lines.len().saturating_sub(1);
    }
    let line_len = line_char_len(&state.lines[state.cursor_line]);
    state.cursor_col = state.cursor_col.min(line_len);
}

fn replace_char_range(line: &mut String, from_col: usize, to_col: usize) -> bool {
    if from_col >= to_col {
        return false;
    }
    let from = super::byte_index(line, from_col);
    let to = super::byte_index(line, to_col);
    if from >= to || to > line.len() {
        return false;
    }
    line.replace_range(from..to, "");
    true
}

fn gui_set_clipboard_charwise(state: &mut GuiParityState, text: String) {
    state.clipboard = VimRegister::charwise(text);
}

fn gui_set_clipboard_linewise(state: &mut GuiParityState, lines: Vec<String>) {
    if lines.is_empty() {
        return;
    }
    state.clipboard = VimRegister::linewise(lines.join("\n"));
}

fn gui_line_col_lt(left_line: usize, left_col: usize, right_line: usize, right_col: usize) -> bool {
    left_line < right_line || (left_line == right_line && left_col < right_col)
}

fn gui_byte_offset_for_line_col(state: &GuiParityState, line_idx: usize, col: usize) -> usize {
    let mut offset = 0usize;
    for (idx, line) in state.lines.iter().enumerate() {
        if idx == line_idx {
            offset += super::byte_index(line, col);
            break;
        }
        offset += line.len() + 1;
    }
    offset
}

fn gui_cursor_from_anchor(state: &mut GuiParityState, target: usize) {
    let mut offset = 0usize;
    for (idx, line) in state.lines.iter().enumerate() {
        let line_end = offset + line.len();
        if target <= line_end {
            state.cursor_line = idx;
            state.cursor_col = line[..target.saturating_sub(offset)].chars().count();
            return;
        }
        offset = line_end + 1;
    }
}

fn gui_apply_edit_operation(
    state: &mut GuiParityState,
    op: &crate::editor_core::types::EditOperation,
) {
    if op.changes.is_empty() {
        if let Some(selection) = &op.selection {
            let max = super::join_lines(&state.lines).len();
            gui_cursor_from_anchor(state, selection.anchor.min(max));
            clamp_gui_cursor(state);
        }
        return;
    }

    let mut text = super::join_lines(&state.lines);
    let mut mapped_anchor =
        gui_byte_offset_for_line_col(state, state.cursor_line, state.cursor_col);

    let mut changes = op.changes.clone();
    changes.sort_by(|a, b| b.from.cmp(&a.from));
    for change in &changes {
        let from = change.from.min(text.len());
        let to = change.to.min(text.len());
        text.replace_range(from..to, &change.insert);
        if from <= mapped_anchor {
            if to <= mapped_anchor {
                let removed = to - from;
                let added = change.insert.len();
                mapped_anchor = mapped_anchor + added - removed;
            } else {
                let inside = mapped_anchor.saturating_sub(from);
                mapped_anchor = from + inside.min(change.insert.len());
            }
        }
    }

    state.lines = super::split_lines(&text);
    let final_anchor = op
        .selection
        .as_ref()
        .map_or(mapped_anchor, |selection| selection.anchor);
    gui_cursor_from_anchor(state, final_anchor.min(text.len()));
    clamp_gui_cursor(state);
}

fn gui_shared_vim_register(
    state: &GuiParityState,
) -> Option<crate::editor_core::vim_actions::VimRegisterValue> {
    if state.clipboard.is_empty() {
        return None;
    }
    Some(crate::editor_core::vim_actions::VimRegisterValue {
        text: state.clipboard.text.clone(),
        mode: match state.clipboard.mode {
            VimRegisterMode::Charwise => crate::editor_core::vim_actions::VimRegisterMode::Charwise,
            VimRegisterMode::Linewise => crate::editor_core::vim_actions::VimRegisterMode::Linewise,
        },
    })
}

fn gui_try_apply_shared_vim_action(
    state: &mut GuiParityState,
    action: &crate::editor_core::vim::VimAction,
) -> bool {
    let text = super::join_lines(&state.lines);
    let cursor = gui_byte_offset_for_line_col(state, state.cursor_line, state.cursor_col);
    let selection = crate::editor_core::types::SelectionSnapshot {
        anchor: cursor,
        head: cursor,
    };
    let register = gui_shared_vim_register(state);
    let Some(result) = crate::editor_core::vim_actions::execute_vim_action_with_target(
        &text,
        selection,
        action.intent,
        action.count.max(1),
        register.as_ref(),
        action.target_char,
    ) else {
        return false;
    };
    for operation in &result.operations {
        gui_apply_edit_operation(state, operation);
    }
    if let Some(register) = result.register {
        state.clipboard = VimRegister {
            text: register.text,
            mode: match register.mode {
                crate::editor_core::vim_actions::VimRegisterMode::Charwise => {
                    VimRegisterMode::Charwise
                }
                crate::editor_core::vim_actions::VimRegisterMode::Linewise => {
                    VimRegisterMode::Linewise
                }
            },
        };
    }
    true
}

fn gui_slice_cols_range(
    state: &GuiParityState,
    from_line: usize,
    from_col: usize,
    to_line: usize,
    to_col: usize,
) -> Option<String> {
    if !gui_line_col_lt(from_line, from_col, to_line, to_col) {
        return None;
    }
    if from_line == to_line {
        let line = state.lines.get(from_line)?;
        let start = super::byte_index(line, from_col);
        let end = super::byte_index(line, to_col);
        if start >= end || end > line.len() {
            return None;
        }
        return Some(line[start..end].to_string());
    }
    let text = super::join_lines(&state.lines);
    let start = gui_byte_offset_for_line_col(state, from_line, from_col);
    let end = gui_byte_offset_for_line_col(state, to_line, to_col);
    if start >= end || end > text.len() {
        return None;
    }
    Some(text[start..end].to_string())
}

fn gui_move_cursor_right_word(state: &mut GuiParityState) {
    let line = &state.lines[state.cursor_line];
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    if state.cursor_col >= len {
        if state.cursor_line + 1 < state.lines.len() {
            state.cursor_line += 1;
            state.cursor_col = 0;
        }
        return;
    }

    let mut col = state.cursor_col;
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
        while col < len && chars.get(col).is_some_and(|c| c.is_whitespace()) {
            col += 1;
        }
    }

    state.cursor_col = col;
}

fn gui_insert_paste(state: &mut GuiParityState, text: &str) {
    if text.is_empty() {
        return;
    }
    if state.lines.is_empty() {
        state.lines.push(String::new());
    }

    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let parts: Vec<&str> = normalized.split('\n').collect();
    if parts.is_empty() {
        return;
    }

    let line_idx = state.cursor_line.min(state.lines.len().saturating_sub(1));
    let col = state.cursor_col;
    let current = state.lines[line_idx].clone();
    let split_idx = super::byte_index(&current, col);
    let (left, right) = current.split_at(split_idx);

    if parts.len() == 1 {
        state.lines[line_idx] = format!("{left}{}{right}", parts[0]);
        state.cursor_line = line_idx;
        state.cursor_col = col + parts[0].chars().count();
        return;
    }

    state.lines[line_idx] = format!("{left}{}", parts[0]);
    let mut insert_at = line_idx + 1;
    for part in &parts[1..parts.len() - 1] {
        state.lines.insert(insert_at, (*part).to_string());
        insert_at += 1;
    }

    let tail = *parts.last().unwrap_or(&"");
    state.lines.insert(insert_at, format!("{tail}{right}"));
    state.cursor_line = insert_at;
    state.cursor_col = tail.chars().count();
}

fn gui_paste_after(state: &mut GuiParityState, count: usize) {
    if state.clipboard.is_empty() {
        return;
    }
    match state.clipboard.mode {
        VimRegisterMode::Linewise => {
            let normalized = state
                .clipboard
                .text
                .strip_suffix('\n')
                .unwrap_or_else(|| state.clipboard.text.as_str());
            let lines = if normalized.is_empty() {
                vec![String::new()]
            } else {
                normalized
                    .split('\n')
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>()
            };
            let mut repeated = Vec::with_capacity(lines.len() * count.max(1));
            for _ in 0..count.max(1) {
                repeated.extend(lines.iter().cloned());
            }
            if repeated.is_empty() {
                return;
            }
            let insert_at = state.cursor_line + 1;
            for (offset, line) in repeated.iter().enumerate() {
                state.lines.insert(insert_at + offset, line.clone());
            }
            state.cursor_line = insert_at;
            state.cursor_col = 0;
        }
        VimRegisterMode::Charwise => {
            let text = state.clipboard.text.clone();
            for _ in 0..count.max(1) {
                let line_len = line_char_len(&state.lines[state.cursor_line]);
                if state.cursor_col < line_len {
                    state.cursor_col += 1;
                } else {
                    state.cursor_col = line_len;
                }
                gui_insert_paste(state, &text);
            }
        }
    }
}

fn gui_apply_visual_selection_action(
    state: &mut GuiParityState,
    source_mode: UiMode,
    delete: bool,
) -> bool {
    if !matches!(source_mode, UiMode::Visual | UiMode::VisualLine) {
        return false;
    }

    let linewise = source_mode == UiMode::VisualLine;
    let anchor = state
        .selection_anchor
        .unwrap_or((state.cursor_line, state.cursor_col));
    let mut start_line = std::cmp::min(anchor.0, state.cursor_line);
    let mut end_line = std::cmp::max(anchor.0, state.cursor_line);
    if state.lines.is_empty() {
        state.lines.push(String::new());
    }
    start_line = start_line.min(state.lines.len().saturating_sub(1));
    end_line = end_line.min(state.lines.len().saturating_sub(1));

    let mut yanked = Vec::new();
    if linewise {
        for i in start_line..=end_line {
            if i < state.lines.len() {
                yanked.push(state.lines[i].clone());
            }
        }
        if delete {
            for _ in start_line..=end_line {
                if start_line < state.lines.len() {
                    state.lines.remove(start_line);
                }
            }
            if state.lines.is_empty() {
                state.lines.push(String::new());
            }
            state.cursor_line = start_line.min(state.lines.len().saturating_sub(1));
            state.cursor_col = 0;
        }
    } else {
        let (start_col, end_col) = if anchor.0 == state.cursor_line {
            (
                std::cmp::min(anchor.1, state.cursor_col),
                std::cmp::max(anchor.1, state.cursor_col),
            )
        } else if anchor.0 < state.cursor_line {
            (anchor.1, state.cursor_col)
        } else {
            (state.cursor_col, anchor.1)
        };

        if start_line == end_line {
            let line = &state.lines[start_line];
            let chars: Vec<char> = line.chars().collect();
            let c_start = std::cmp::min(start_col, chars.len());
            let c_end = std::cmp::min(end_col + 1, chars.len());
            yanked.push(chars[c_start..c_end].iter().collect::<String>());
            if delete {
                let mut new_line: String = chars[..c_start].iter().collect();
                let tail: String = chars[c_end..].iter().collect();
                new_line.push_str(&tail);
                state.lines[start_line] = new_line;
                state.cursor_col = c_start;
            }
        } else {
            let l1_chars: Vec<char> = state.lines[start_line].chars().collect();
            let l1_start = std::cmp::min(start_col, l1_chars.len());
            yanked.push(l1_chars[l1_start..].iter().collect::<String>());
            for i in (start_line + 1)..end_line {
                if i < state.lines.len() {
                    yanked.push(state.lines[i].clone());
                }
            }
            let ln_chars: Vec<char> = state.lines[end_line].chars().collect();
            let ln_end = std::cmp::min(end_col + 1, ln_chars.len());
            yanked.push(ln_chars[..ln_end].iter().collect::<String>());
            if delete {
                let mut new_l1: String = l1_chars[..l1_start].iter().collect();
                let tail: String = ln_chars[ln_end..].iter().collect();
                new_l1.push_str(&tail);
                for _ in start_line..=end_line {
                    if start_line < state.lines.len() {
                        state.lines.remove(start_line);
                    }
                }
                state.lines.insert(start_line, new_l1);
                state.cursor_line = start_line;
                state.cursor_col = l1_start;
            }
        }
    }

    if yanked.is_empty() {
        return false;
    }
    if linewise {
        gui_set_clipboard_linewise(state, yanked);
    } else {
        gui_set_clipboard_charwise(state, yanked.join("\n"));
    }
    state.mode = UiMode::Normal;
    state.selection_anchor = None;
    true
}

fn apply_gui_vim_action(
    state: &mut GuiParityState,
    action: &crate::editor_core::vim::VimAction,
    source_mode: UiMode,
    case_name: &str,
) {
    use crate::editor_core::vim::VimIntent;
    if gui_try_apply_shared_vim_action(state, action) {
        return;
    }
    let count = action.count.max(1);
    match action.intent {
        VimIntent::MoveLeft => {
            for _ in 0..count {
                if state.cursor_col > 0 {
                    state.cursor_col -= 1;
                } else if state.cursor_line > 0 {
                    state.cursor_line -= 1;
                    state.cursor_col = line_char_len(&state.lines[state.cursor_line]);
                }
            }
        }
        VimIntent::MoveRight => {
            for _ in 0..count {
                let line_len = line_char_len(&state.lines[state.cursor_line]);
                if state.cursor_col < line_len {
                    state.cursor_col += 1;
                } else if state.cursor_line + 1 < state.lines.len() {
                    state.cursor_line += 1;
                    state.cursor_col = 0;
                }
            }
        }
        VimIntent::MoveUp => {
            state.cursor_line = state.cursor_line.saturating_sub(count);
        }
        VimIntent::MoveDown => {
            state.cursor_line =
                (state.cursor_line + count).min(state.lines.len().saturating_sub(1));
        }
        VimIntent::MoveLineStart => state.cursor_col = 0,
        VimIntent::MoveLineEnd => {
            let line_len = line_char_len(&state.lines[state.cursor_line]);
            state.cursor_col = line_len.saturating_sub(1);
        }
        VimIntent::MoveDocStart => {
            state.cursor_line = 0;
            state.cursor_col = 0;
        }
        VimIntent::MoveDocEnd => {
            state.cursor_line = state.lines.len().saturating_sub(1);
            state.cursor_col = 0;
        }
        VimIntent::MoveToLine => {
            state.cursor_line = count.max(1).min(state.lines.len()) - 1;
            state.cursor_col = 0;
        }
        VimIntent::EnterInsert => state.mode = UiMode::Editor,
        VimIntent::AppendInsert => {
            let line_len = line_char_len(&state.lines[state.cursor_line]);
            if state.cursor_col < line_len {
                state.cursor_col += 1;
            }
            state.mode = UiMode::Editor;
        }
        VimIntent::InsertLineStart => {
            state.cursor_col = 0;
            state.mode = UiMode::Editor;
        }
        VimIntent::AppendLineEnd => {
            state.cursor_col = line_char_len(&state.lines[state.cursor_line]);
            state.mode = UiMode::Editor;
        }
        VimIntent::OpenLineBelow => {
            let insert_at = state.cursor_line + 1;
            state.lines.insert(insert_at, String::new());
            state.cursor_line = insert_at;
            state.cursor_col = 0;
            state.mode = UiMode::Editor;
        }
        VimIntent::OpenLineAbove => {
            state.lines.insert(state.cursor_line, String::new());
            state.cursor_col = 0;
            state.mode = UiMode::Editor;
        }
        VimIntent::EnterVisual => {
            state.mode = UiMode::Visual;
            state.selection_anchor = Some((state.cursor_line, state.cursor_col));
        }
        VimIntent::EnterVisualLine => {
            state.mode = UiMode::VisualLine;
            state.selection_anchor = Some((state.cursor_line, state.cursor_col));
        }
        VimIntent::ExitVisual => {
            state.mode = UiMode::Normal;
            state.selection_anchor = None;
        }
        VimIntent::DeleteLine => {
            let mut deleted = Vec::new();
            let mut removed = 0usize;
            for _ in 0..count {
                if state.cursor_line < state.lines.len() {
                    deleted.push(state.lines.remove(state.cursor_line));
                    removed += 1;
                }
            }
            if removed > 0 && state.lines.is_empty() {
                state.lines.push(String::new());
            }
            if !deleted.is_empty() {
                gui_set_clipboard_linewise(state, deleted);
            }
        }
        VimIntent::YankLine => {
            let mut yanked = Vec::new();
            for i in 0..count {
                if state.cursor_line + i < state.lines.len() {
                    yanked.push(state.lines[state.cursor_line + i].clone());
                }
            }
            gui_set_clipboard_linewise(state, yanked);
        }
        VimIntent::DeleteToLineStart => {
            let mut chunks = Vec::new();
            for _ in 0..count {
                if state.cursor_col == 0 {
                    break;
                }
                let deleted = state.lines[state.cursor_line]
                    .chars()
                    .take(state.cursor_col)
                    .collect::<String>();
                let line = &mut state.lines[state.cursor_line];
                if replace_char_range(line, 0, state.cursor_col) {
                    state.cursor_col = 0;
                    chunks.push(deleted);
                } else {
                    break;
                }
            }
            if !chunks.is_empty() {
                gui_set_clipboard_charwise(state, chunks.join("\n"));
            }
        }
        VimIntent::DeleteToLineEnd => {
            let mut chunks = Vec::new();
            for _ in 0..count {
                let line_len = line_char_len(&state.lines[state.cursor_line]);
                if state.cursor_col >= line_len {
                    break;
                }
                let deleted = state.lines[state.cursor_line]
                    .chars()
                    .skip(state.cursor_col)
                    .collect::<String>();
                let line = &mut state.lines[state.cursor_line];
                if replace_char_range(line, state.cursor_col, line_len) {
                    chunks.push(deleted);
                } else {
                    break;
                }
            }
            if !chunks.is_empty() {
                gui_set_clipboard_charwise(state, chunks.join("\n"));
            }
        }
        VimIntent::DeleteChar => {
            for _ in 0..count {
                let line_len = line_char_len(&state.lines[state.cursor_line]);
                if state.cursor_col < line_len {
                    let line = &mut state.lines[state.cursor_line];
                    let _ = replace_char_range(line, state.cursor_col, state.cursor_col + 1);
                } else if state.cursor_line + 1 < state.lines.len() {
                    let next = state.lines.remove(state.cursor_line + 1);
                    state.lines[state.cursor_line].push_str(&next);
                }
            }
        }
        VimIntent::YankWordForward => {
            let mut yanked = Vec::new();
            let origin_line = state.cursor_line;
            let origin_col = state.cursor_col;
            for _ in 0..count {
                let from_line = state.cursor_line;
                let from_col = state.cursor_col;
                gui_move_cursor_right_word(state);
                let to_line = state.cursor_line;
                let to_col = state.cursor_col;
                if gui_line_col_lt(from_line, from_col, to_line, to_col) {
                    if let Some(chunk) =
                        gui_slice_cols_range(state, from_line, from_col, to_line, to_col)
                    {
                        yanked.push(chunk);
                    }
                } else {
                    state.cursor_line = from_line;
                    state.cursor_col = from_col;
                    break;
                }
            }
            state.cursor_line = origin_line;
            state.cursor_col = origin_col;
            if !yanked.is_empty() {
                gui_set_clipboard_charwise(state, yanked.join(""));
            }
        }
        VimIntent::YankVisualSelection => {
            let _ = gui_apply_visual_selection_action(state, source_mode, false);
        }
        VimIntent::DeleteVisualSelection => {
            let _ = gui_apply_visual_selection_action(state, source_mode, true);
        }
        VimIntent::OpenCommandBar => {
            state.mode = UiMode::CommandBar;
        }
        VimIntent::PasteAfter => {
            gui_paste_after(state, count);
        }
        VimIntent::DeleteTillChar => {}
        VimIntent::Swallow => {}
        other => panic!(
            "unsupported GUI parity action {:?} in replay case {}",
            other, case_name
        ),
    }
}

fn run_gui_parity_case(case: &VimParityReplayCase) -> ParitySnapshot {
    let mut state = GuiParityState {
        lines: split_replay_lines(&case.initial_text),
        cursor_line: case.initial_cursor_line,
        cursor_col: case.initial_cursor_col,
        mode: ui_mode_from_vim_mode(case.initial_state.mode),
        selection_anchor: None,
        vim_state: case.initial_state.clone(),
        clipboard: VimRegister::default(),
    };
    if matches!(state.mode, UiMode::Visual | UiMode::VisualLine) {
        state.selection_anchor = Some((state.cursor_line, state.cursor_col));
    }
    clamp_gui_cursor(&mut state);

    for token in &case.keys {
        let vim_key = crate::editor_core::vim::parse_key_token(token)
            .unwrap_or_else(|| panic!("invalid GUI parity key token: {token}"));
        let ctx = crate::editor_core::vim::VimContext {
            has_search_matches: false,
            line_count: state.lines.len(),
            macro_recording: false,
        };
        let step =
            crate::editor_core::engine::EditorEngine::step_vim(&state.vim_state, vim_key, &ctx);
        let source_mode = state.mode;
        state.vim_state = step.state.clone();
        state.mode = ui_mode_from_vim_mode(step.state.mode);

        if !step.handled {
            continue;
        }
        for action in &step.actions {
            apply_gui_vim_action(&mut state, action, source_mode, &case.name);
        }
        if !matches!(
            state.mode,
            UiMode::Visual | UiMode::VisualLine | UiMode::CommandBar
        ) {
            state.selection_anchor = None;
        }
        clamp_gui_cursor(&mut state);
    }

    ParitySnapshot {
        lines: state.lines,
        cursor_line: state.cursor_line,
        cursor_col: state.cursor_col,
        mode: state.mode,
        vim_state: state.vim_state,
        selection_anchor: state.selection_anchor,
        clipboard_text: state.clipboard.text,
        clipboard_mode: state.clipboard.mode,
    }
}

fn run_tui_parity_case(case: &VimParityReplayCase) -> ParitySnapshot {
    let (db, mut app, path) = app_with_note(&case.initial_text);
    app.mode = ui_mode_from_vim_mode(case.initial_state.mode);
    app.vim_state = case.initial_state.clone();
    if app.lines.is_empty() {
        app.lines.push(String::new());
    }
    app.cursor_line = case
        .initial_cursor_line
        .min(app.lines.len().saturating_sub(1));
    app.cursor_col = case.initial_cursor_col;
    if matches!(app.mode, UiMode::Visual | UiMode::VisualLine) {
        app.selection_anchor = Some((app.cursor_line, app.cursor_col));
    }
    app.adjust_cursor();

    for token in &case.keys {
        let key = parse_terminal_key_token(token);
        app.handle_key(&db, key).unwrap_or_else(|err| {
            panic!(
                "parity replay case '{}' failed on key '{}': {}",
                case.name, token, err
            )
        });
    }
    app.adjust_cursor();

    let snapshot = ParitySnapshot {
        lines: app.lines.clone(),
        cursor_line: app.cursor_line,
        cursor_col: app.cursor_col,
        mode: app.mode,
        vim_state: app.vim_state.clone(),
        selection_anchor: app.selection_anchor,
        clipboard_text: app.clipboard.text.clone(),
        clipboard_mode: app.clipboard.mode,
    };

    drop(app);
    drop(db);
    cleanup_db_files(&path);
    snapshot
}

fn gui_markdown_context(
    state: &GuiParityState,
) -> crate::editor_core::context::ResolvedContext<'static> {
    let text = super::join_lines(&state.lines);
    let anchor = gui_byte_offset_for_line_col(state, state.cursor_line, state.cursor_col);
    crate::editor_core::context::ResolvedContext::new(
        crate::editor_core::types::EditorContextSnapshot {
            text,
            selection: crate::editor_core::types::SelectionSnapshot {
                anchor,
                head: anchor,
            },
            changed_range: None,
        },
    )
}

fn gui_current_line(state: &GuiParityState) -> &str {
    state
        .lines
        .get(state.cursor_line)
        .map(String::as_str)
        .unwrap_or("")
}

fn gui_insert_char(state: &mut GuiParityState, ch: char) {
    if ch.is_control() {
        return;
    }
    let idx = super::byte_index(&state.lines[state.cursor_line], state.cursor_col);
    state.lines[state.cursor_line].insert(idx, ch);
    state.cursor_col += 1;
}

fn gui_insert_text(state: &mut GuiParityState, text: &str) {
    if text.is_empty() {
        return;
    }
    let idx = super::byte_index(&state.lines[state.cursor_line], state.cursor_col);
    state.lines[state.cursor_line].insert_str(idx, text);
    state.cursor_col += text.chars().count();
}

fn gui_insert_newline(state: &mut GuiParityState) {
    let idx = super::byte_index(&state.lines[state.cursor_line], state.cursor_col);
    let right = state.lines[state.cursor_line][idx..].to_string();
    state.lines[state.cursor_line].truncate(idx);
    let insert_at = state.cursor_line + 1;
    state.lines.insert(insert_at, right);
    state.cursor_line = insert_at;
    state.cursor_col = 0;
}

fn gui_backspace(state: &mut GuiParityState) {
    if state.cursor_col > 0 {
        let new_col = state.cursor_col - 1;
        crate::terminal::text_utils::remove_char_at(&mut state.lines[state.cursor_line], new_col);
        state.cursor_col = new_col;
        return;
    }
    if state.cursor_line == 0 {
        return;
    }
    let removed = state.lines.remove(state.cursor_line);
    state.cursor_line -= 1;
    let prev_len = line_char_len(&state.lines[state.cursor_line]);
    state.lines[state.cursor_line].push_str(&removed);
    state.cursor_col = prev_len;
}

fn gui_delete_forward(state: &mut GuiParityState) {
    let line_len = line_char_len(&state.lines[state.cursor_line]);
    if state.cursor_col < line_len {
        crate::terminal::text_utils::remove_char_at(
            &mut state.lines[state.cursor_line],
            state.cursor_col,
        );
        return;
    }
    if state.cursor_line + 1 >= state.lines.len() {
        return;
    }
    let next = state.lines.remove(state.cursor_line + 1);
    state.lines[state.cursor_line].push_str(&next);
}

fn gui_delete_word_backward(state: &mut GuiParityState) -> bool {
    if state.cursor_col == 0 {
        if state.cursor_line > 0 {
            gui_backspace(state);
            return true;
        }
        return false;
    }
    let chars: Vec<char> = state.lines[state.cursor_line].chars().collect();
    let mut col = state.cursor_col;
    while col > 0 && chars.get(col - 1).is_some_and(|c| !c.is_alphanumeric()) {
        col -= 1;
    }
    while col > 0 && chars.get(col - 1).is_some_and(|c| c.is_alphanumeric()) {
        col -= 1;
    }
    let start = super::byte_index(&state.lines[state.cursor_line], col);
    let end = super::byte_index(&state.lines[state.cursor_line], state.cursor_col);
    state.lines[state.cursor_line].replace_range(start..end, "");
    state.cursor_col = col;
    true
}

fn gui_line_might_trigger_doc_change_rules(line: &str) -> bool {
    let trimmed = line.trim_start();
    let might_be_list = trimmed.starts_with('-')
        || trimmed.starts_with('*')
        || trimmed.starts_with('+')
        || trimmed.starts_with("->")
        || trimmed.chars().next().is_some_and(|c| c.is_ascii_digit());
    let might_be_table = trimmed.starts_with('|') && line.trim_end().ends_with('|');
    might_be_list || might_be_table
}

fn gui_try_markdown_doc_change_rule(state: &mut GuiParityState) {
    if !gui_line_might_trigger_doc_change_rules(gui_current_line(state)) {
        return;
    }
    let ctx = gui_markdown_context(state);
    let options = crate::editor_core::text_rules::TextRuleOptions {
        markdown_autoformat: true,
        checklist_auto_reorder: true,
        table_enabled: true,
    };
    if let Some(op) = crate::editor_core::text_rules::run_doc_change_rules(&ctx, options) {
        gui_apply_edit_operation(state, &op);
    }
}

fn gui_try_markdown_enter_rule(state: &mut GuiParityState) -> bool {
    let ctx = gui_markdown_context(state);
    let options = crate::editor_core::text_rules::TextRuleOptions {
        markdown_autoformat: true,
        checklist_auto_reorder: true,
        table_enabled: true,
    };
    if let Some(op) = crate::editor_core::text_rules::run_enter_rules(&ctx, options) {
        gui_apply_edit_operation(state, &op);
        return true;
    }
    false
}

fn gui_try_markdown_tab_rule(state: &mut GuiParityState, outdent: bool) -> bool {
    let ctx = gui_markdown_context(state);
    let options = crate::editor_core::text_rules::TabRuleOptions {
        markdown_autoformat: true,
        outdent,
        table_enabled: true,
    };
    if let Some(op) = crate::editor_core::text_rules::run_tab_rules(&ctx, options) {
        gui_apply_edit_operation(state, &op);
        return true;
    }
    false
}

fn gui_try_markdown_table_navigation_rule(state: &mut GuiParityState, outdent: bool) -> bool {
    let ctx = gui_markdown_context(state);
    let options = crate::editor_core::text_rules::TabRuleOptions {
        markdown_autoformat: true,
        outdent,
        table_enabled: true,
    };
    if let Some(op) = crate::editor_core::text_rules::run_table_cell_navigation_rules(&ctx, options)
    {
        gui_apply_edit_operation(state, &op);
        return true;
    }
    false
}

fn gui_try_markdown_table_pipe_insert_column_rule(state: &mut GuiParityState) -> bool {
    let ctx = gui_markdown_context(state);
    if let Some(op) = crate::editor_core::text_rules::run_table_pipe_insert_column_rule(&ctx) {
        gui_apply_edit_operation(state, &op);
        return true;
    }
    false
}

fn gui_try_markdown_table_header_delete_column_rule(state: &mut GuiParityState) -> bool {
    let ctx = gui_markdown_context(state);
    if let Some(op) = crate::editor_core::text_rules::run_table_header_delete_column_rule(&ctx) {
        gui_apply_edit_operation(state, &op);
        return true;
    }
    false
}

fn gui_try_markdown_table_boundary_edit_rule(
    state: &mut GuiParityState,
    backward: bool,
    structural_merge: bool,
) -> Option<bool> {
    let ctx = gui_markdown_context(state);
    let options = crate::editor_core::text_rules::TableBoundaryEditOptions {
        markdown_autoformat: true,
        backward,
        structural_merge,
        table_enabled: true,
    };
    let op = crate::editor_core::text_rules::run_table_boundary_edit_rules(&ctx, options)?;
    let changed = !op.changes.is_empty();
    gui_apply_edit_operation(state, &op);
    Some(changed)
}

fn apply_gui_markdown_key(state: &mut GuiParityState, key: Key, case_name: &str, token: &str) {
    let mut should_autoformat = false;
    match key {
        Key::Enter => {
            if !gui_try_markdown_enter_rule(state) {
                gui_insert_newline(state);
            }
            should_autoformat = true;
        }
        Key::Tab => {
            if !gui_try_markdown_table_navigation_rule(state, false)
                && !gui_try_markdown_tab_rule(state, false)
            {
                gui_insert_text(state, "  ");
                should_autoformat = true;
            }
        }
        Key::BackTab => {
            if !gui_try_markdown_table_navigation_rule(state, true)
                && gui_try_markdown_tab_rule(state, true)
            {
                should_autoformat = true;
            }
        }
        Key::Backspace => {
            if let Some(changed) = gui_try_markdown_table_boundary_edit_rule(state, true, false) {
                should_autoformat = changed;
            } else {
                gui_backspace(state);
                should_autoformat = true;
            }
        }
        Key::Delete => {
            if let Some(changed) = gui_try_markdown_table_boundary_edit_rule(state, false, false) {
                should_autoformat = changed;
            } else {
                gui_delete_forward(state);
                should_autoformat = true;
            }
        }
        Key::Ctrl('w') | Key::CtrlBackspace => {
            if gui_try_markdown_table_header_delete_column_rule(state) {
                should_autoformat = false;
            } else if let Some(changed) =
                gui_try_markdown_table_boundary_edit_rule(state, true, true)
            {
                should_autoformat = changed;
            } else {
                should_autoformat = gui_delete_word_backward(state);
            }
        }
        Key::CtrlDelete => {
            if gui_try_markdown_table_header_delete_column_rule(state) {
                should_autoformat = false;
            } else if let Some(changed) =
                gui_try_markdown_table_boundary_edit_rule(state, false, true)
            {
                should_autoformat = changed;
            } else {
                gui_delete_forward(state);
                should_autoformat = true;
            }
        }
        Key::Char(ch) => {
            if ch == '|' && gui_try_markdown_table_pipe_insert_column_rule(state) {
                should_autoformat = false;
            } else {
                gui_insert_char(state, ch);
                should_autoformat = true;
            }
            if ch == ' ' && super::is_markdown_table_line(gui_current_line(state)) {
                should_autoformat = false;
            }
        }
        other => {
            panic!(
                "unsupported GUI markdown parity key {:?} in case {} token {}",
                other, case_name, token
            );
        }
    }

    if should_autoformat {
        gui_try_markdown_doc_change_rule(state);
    }
    clamp_gui_cursor(state);
}

fn run_gui_markdown_parity_case(case: &MarkdownParityReplayCase) -> MarkdownParitySnapshot {
    let mut state = GuiParityState {
        lines: split_replay_lines(&case.initial_text),
        cursor_line: case.initial_cursor_line,
        cursor_col: case.initial_cursor_col,
        mode: UiMode::Editor,
        selection_anchor: None,
        vim_state: crate::editor_core::vim::VimState::default(),
        clipboard: VimRegister::default(),
    };
    clamp_gui_cursor(&mut state);

    for token in &case.keys {
        let key = parse_markdown_key_token(token);
        apply_gui_markdown_key(&mut state, key, &case.name, token);
    }

    MarkdownParitySnapshot {
        lines: state.lines,
        cursor_line: state.cursor_line,
        cursor_col: state.cursor_col,
    }
}

fn run_tui_markdown_parity_case(case: &MarkdownParityReplayCase) -> MarkdownParitySnapshot {
    let (db, mut app, path) = app_with_note(&case.initial_text);
    app.mode = UiMode::Editor;
    if app.lines.is_empty() {
        app.lines.push(String::new());
    }
    app.cursor_line = case
        .initial_cursor_line
        .min(app.lines.len().saturating_sub(1));
    app.cursor_col = case.initial_cursor_col;
    app.adjust_cursor();

    for token in &case.keys {
        let key = parse_markdown_key_token(token);
        app.handle_key(&db, key).unwrap_or_else(|err| {
            panic!(
                "markdown parity case '{}' failed on key '{}': {}",
                case.name, token, err
            )
        });
    }
    app.adjust_cursor();

    let snapshot = MarkdownParitySnapshot {
        lines: app.lines.clone(),
        cursor_line: app.cursor_line,
        cursor_col: app.cursor_col,
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
#[path = "tests/parity.rs"]
mod parity;
#[cfg(test)]
#[path = "tests/vim.rs"]
mod vim;
