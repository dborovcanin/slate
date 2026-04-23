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
use super::{TerminalApp, TerminalOptions, UiMode, VimRegister, VimRegisterMode};
use crate::storage::Db;
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
    let snapshot = crate::editor_core::types::EditorContextSnapshot {
        text,
        selection: crate::editor_core::types::SelectionSnapshot {
            anchor: cursor,
            head: cursor,
        },
        changed_range: None,
    };
    let register = gui_shared_vim_register(state);
    let Some(result) = crate::editor_core::vim_actions::execute_vim_action(
        &snapshot,
        action.intent,
        action.count.max(1),
        register.as_ref(),
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

fn gui_markdown_context(state: &GuiParityState) -> crate::editor_core::context::ResolvedContext {
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

#[test]
fn vim_cross_frontend_parity_replay_cases() {
    let suite: VimParityReplaySuite =
        serde_json::from_str(include_str!("../tests/golden/vim_parity_replay.json"))
            .expect("parse vim parity replay fixture");
    for case in suite.cases {
        let tui = run_tui_parity_case(&case);
        let gui = run_gui_parity_case(&case);
        assert_eq!(tui, gui, "cross-frontend parity mismatch in {}", case.name);
    }
}

#[test]
fn markdown_cross_frontend_parity_replay_cases() {
    let suite: MarkdownParityReplaySuite =
        serde_json::from_str(include_str!("../tests/golden/markdown_parity_replay.json"))
            .expect("parse markdown parity replay fixture");
    for case in suite.cases {
        let tui = run_tui_markdown_parity_case(&case);
        let gui = run_gui_markdown_parity_case(&case);
        assert_eq!(tui, gui, "markdown parity mismatch in {}", case.name);
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
    let ranges = describe_fold_ranges(&["## parent", "### child", "details", "## sibling", "tail"]);

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
    let prefix = extract_variable_completion_prefix("total cost + tax", 10).expect("prefix exists");
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
fn module_style_off_keeps_table_autoformat_when_table_module_is_on() {
    let (db, mut app, path) = app_with_note("| a | b |\n| --- | --- |\n|1|2|");
    app.mode = UiMode::Editor;
    app.cursor_line = 2;
    app.cursor_col = 4; // before trailing pipe in "|1|2|"

    app.execute_terminal_command(&db, "module style off");
    app.handle_editor_key(&db, Key::Char('0'))
        .expect("typing still triggers table autoformat");

    assert_ne!(app.lines[2], "|1|20|");
    assert!(app.lines[2].contains("20"));
    assert!(app.lines[2].starts_with("| "));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_style_off_keeps_table_ctrl_navigation_when_table_module_is_on() {
    let (db, mut app, path) = app_with_note("| aaa | bb  |");
    app.mode = UiMode::Editor;
    app.cursor_col = 5; // first cell end anchor

    app.execute_terminal_command(&db, "module style off");

    app.handle_editor_key(&db, Key::CtrlArrowRight)
        .expect("ctrl-right jumps to next table cell");
    assert_eq!(app.cursor_col, 10);

    app.handle_editor_key(&db, Key::CtrlArrowLeft)
        .expect("ctrl-left jumps back to previous table cell");
    assert_eq!(app.cursor_col, 5);

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
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "bar ");

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

    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "one two ");
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
fn vim_d2w_deletes_two_words_forward() {
    let (db, mut app, path) = app_with_note("foo bar baz qux");
    app.mode = UiMode::Normal;
    app.cursor_col = 0;

    run_keys(
        &mut app,
        &db,
        &[Key::Char('d'), Key::Char('2'), Key::Char('w')],
    );

    assert_eq!(app.lines, vec!["baz qux".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "foo bar ");
    assert_eq!(app.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_d2b_deletes_two_words_backward() {
    let (db, mut app, path) = app_with_note("foo bar baz");
    app.mode = UiMode::Normal;
    app.cursor_col = app.current_line().find("baz").expect("baz");

    run_keys(
        &mut app,
        &db,
        &[Key::Char('d'), Key::Char('2'), Key::Char('b')],
    );

    assert_eq!(app.lines, vec!["baz".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "foo bar ");
    assert_eq!(app.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_de_uses_word_end_semantics_distinct_from_dw() {
    let (db, mut app, path) = app_with_note("foo bar baz");
    app.mode = UiMode::Normal;
    app.cursor_col = 0;

    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('e')]);
    assert_eq!(app.lines, vec![" bar baz".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "foo");
    assert_eq!(app.cursor_col, 0);

    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(app.lines, vec!["foo bar baz".to_string()]);

    app.cursor_col = 2;
    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('e')]);
    assert_eq!(app.lines, vec!["fo baz".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "o bar");
    assert_eq!(app.cursor_col, 2);

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
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "\n");
    assert_eq!(app.cursor_line, 0);
    assert_eq!(app.cursor_col, 5);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_p_after_yy_pastes_linewise_below_cursor_line() {
    let (db, mut app, path) = app_with_note("one\ntwo");
    app.mode = UiMode::Normal;
    app.cursor_line = 0;
    app.cursor_col = 0;

    run_keys(&mut app, &db, &[Key::Char('y'), Key::Char('y')]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Linewise);
    assert_eq!(app.clipboard.text, "one");

    run_keys(&mut app, &db, &[Key::Char('p')]);

    assert_eq!(
        app.lines,
        vec!["one".to_string(), "one".to_string(), "two".to_string()]
    );
    assert_eq!(app.cursor_line, 1);
    assert_eq!(app.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_counted_p_repeats_linewise_register_in_original_order() {
    let (db, mut app, path) = app_with_note("one\ntwo\nthree");
    app.mode = UiMode::Normal;
    app.cursor_line = 0;
    app.cursor_col = 0;

    run_keys(
        &mut app,
        &db,
        &[Key::Char('2'), Key::Char('y'), Key::Char('y')],
    );
    assert_eq!(app.clipboard.mode, VimRegisterMode::Linewise);
    assert_eq!(app.clipboard.text, "one\ntwo");

    run_keys(&mut app, &db, &[Key::Char('2'), Key::Char('p')]);
    assert_eq!(
        app.lines,
        vec![
            "one".to_string(),
            "one".to_string(),
            "two".to_string(),
            "one".to_string(),
            "two".to_string(),
            "two".to_string(),
            "three".to_string(),
        ]
    );
    assert_eq!(app.cursor_line, 1);
    assert_eq!(app.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_p_after_dollar_uses_charwise_register() {
    let (db, mut app, path) = app_with_note("alpha beta");
    app.mode = UiMode::Normal;
    app.cursor_col = 6;

    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('$')]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "beta");
    assert_eq!(app.lines, vec!["alpha ".to_string()]);

    run_keys(&mut app, &db, &[Key::Char('p')]);

    assert_eq!(app.lines, vec!["alpha beta".to_string()]);
    assert_eq!(app.cursor_line, 0);
    assert_eq!(app.cursor_col, 10);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_p_charwise_newline_splits_line_when_register_contains_newline() {
    let (db, mut app, path) = app_with_note("alpha\nbeta");
    app.mode = UiMode::Normal;
    app.cursor_line = 0;
    app.cursor_col = line_char_len(app.current_line());

    run_keys(&mut app, &db, &[Key::Char('y'), Key::Char('w')]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "\n");

    run_keys(&mut app, &db, &[Key::Char('p')]);
    assert_eq!(
        app.lines,
        vec!["alpha".to_string(), "".to_string(), "beta".to_string()]
    );
    assert_eq!(app.cursor_line, 1);
    assert_eq!(app.cursor_col, 0);

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
