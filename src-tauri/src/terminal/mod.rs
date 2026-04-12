pub mod render;

use crate::config::ThemeConfig;
use crate::storage::{Db, Note};
use std::cmp::min;
use std::io::{self, Write};
use std::mem::MaybeUninit;
use std::time::{Duration, Instant};
use ulid::Ulid;

const AUTOSAVE_DEBOUNCE_MS: u64 = 500;
const TITLE_ROW: usize = 1;
const EDITOR_TOP_ROW: usize = 2;
const GUTTER_WIDTH: usize = 6;

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

#[derive(Debug)]
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
    date_month: u32,  // 1-12
    date_day: u32,
    // Vim state
    vim_buffer: String,
    clipboard: Vec<String>,
    selection_anchor: Option<(usize, usize)>, // (line, col)
}

impl TerminalApp {
    fn new(db: &Db, opts: &TerminalOptions) -> Result<Self, String> {
        let active_note = select_note(db, opts)?;
        let lines = split_lines(&active_note.body);
        let switcher_items = load_note_meta(db)?;

        Ok(Self {
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
            status: "-- NORMAL --  |  Ctrl+N new  Ctrl+P switch  Ctrl+S save  Ctrl+Q quit".to_string(),
            command_input: String::new(),
            quit: false,
            force_quit: false,
            date_year: 0,
            date_month: 0,
            date_day: 0,
            vim_buffer: String::new(),
            clipboard: Vec::new(),
            selection_anchor: None,
        })
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
            Key::CtrlArrowLeft => self.move_cursor_left_word(),
            Key::CtrlArrowRight => self.move_cursor_right_word(),
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
                if !self.try_tab_rule(false) {
                    let text = self.current_line().to_string();
                    let engine = crate::calc::engine::CalcEngine::new();
                    if let Some((from_byte, to_byte, result)) = find_calc_segment(&text, &engine) {
                        self.lines[self.cursor_line].replace_range(from_byte..to_byte, &result);
                        self.cursor_col = line_char_len(&self.lines[self.cursor_line]);
                        self.mark_edited();
                    } else if let Some(ghost) = get_calc_ghost_for_line(&text, &engine) {
                        self.insert_text(&format!(" = {ghost}"));
                    } else {
                        self.insert_text("  ");
                    }
                }
            }
            Key::BackTab => {
                self.try_tab_rule(true);
            }
            Key::Char(':') => {
                self.command_input.clear();
                self.mode = UiMode::CommandBar;
                self.status = ":".to_string();
                return Ok(());
            }
            Key::Char(ch) => self.insert_char(ch),
            Key::Esc => {
                self.mode = UiMode::Normal;
                self.status = "-- NORMAL --".to_string();
            }
            Key::Ctrl(_) => {}
        }

        self.adjust_cursor();
        self.adjust_scroll();

        // Run autoformat rules on doc edits (e.g., dynamically renumbering lists)
        let snapshot = self.build_snapshot();
        let options = crate::editor_core::text_rules::TextRuleOptions {
            markdown_autoformat: true,
        };
        if let Some(op) = crate::editor_core::text_rules::run_doc_change_rules(&snapshot, options) {
            self.apply_edit_operation(&op);
        }

        Ok(())
    }

    fn handle_normal_key(&mut self, _db: &Db, key: Key) -> Result<(), String> {
        match key {
            Key::Char('i') => {
                self.mode = UiMode::Editor;
                self.vim_buffer.clear();
                self.status = "-- INSERT --".to_string();
            }
            Key::Char('I') => {
                self.cursor_col = 0;
                self.mode = UiMode::Editor;
                self.vim_buffer.clear();
                self.status = "-- INSERT --".to_string();
            }
            Key::Char('a') => {
                self.move_cursor_right();
                self.mode = UiMode::Editor;
                self.vim_buffer.clear();
                self.status = "-- INSERT --".to_string();
            }
            Key::Char('A') => {
                self.cursor_col = line_char_len(self.current_line());
                self.mode = UiMode::Editor;
                self.vim_buffer.clear();
                self.status = "-- INSERT --".to_string();
            }
            Key::Char('o') => {
                self.cursor_col = line_char_len(self.current_line());
                self.insert_newline();
                self.mode = UiMode::Editor;
                self.vim_buffer.clear();
                self.status = "-- INSERT --".to_string();
            }
            Key::Char('O') => {
                self.cursor_col = 0;
                let current = self.cursor_line;
                self.lines.insert(current, String::new());
                self.mode = UiMode::Editor;
                self.vim_buffer.clear();
                self.status = "-- INSERT --".to_string();
            }
            Key::Char(':') => {
                self.command_input.clear();
                self.mode = UiMode::CommandBar;
                self.status = ":".to_string();
                self.vim_buffer.clear();
            }
            Key::Char(c) => {
                self.handle_normal_char(c);
            }
            Key::Esc => {
                self.vim_buffer.clear();
                self.status = "-- NORMAL --".to_string();
            }
            Key::ArrowUp => self.move_cursor_up(1),
            Key::ArrowDown => self.move_cursor_down(1),
            Key::ArrowLeft => self.move_cursor_left(),
            Key::ArrowRight => self.move_cursor_right(),
            _ => {}
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

    fn handle_normal_char(&mut self, c: char) {
        if c.is_ascii_digit() {
            if c == '0' && self.vim_buffer.is_empty() {
                self.cursor_col = 0;
                return;
            }
            self.vim_buffer.push(c);
            return;
        }

        // Parse any leading numbers as repeat count
        let mut count_str = String::new();
        let mut rest_str = String::new();
        for val in self.vim_buffer.chars() {
            if val.is_ascii_digit() && rest_str.is_empty() {
                count_str.push(val);
            } else {
                rest_str.push(val);
            }
        }
        let count: usize = count_str.parse().unwrap_or(1).max(1);

        match c {
            'h' => { for _ in 0..count { self.move_cursor_left(); } self.vim_buffer.clear(); }
            'j' => { self.move_cursor_down(count); self.vim_buffer.clear(); }
            'k' => { self.move_cursor_up(count); self.vim_buffer.clear(); }
            'l' => { for _ in 0..count { self.move_cursor_right(); } self.vim_buffer.clear(); }
            'w' => { for _ in 0..count { self.move_cursor_right_word(); } self.vim_buffer.clear(); }
            'b' => { for _ in 0..count { self.move_cursor_left_word(); } self.vim_buffer.clear(); }
            '$' => { self.cursor_col = line_char_len(self.current_line()); self.vim_buffer.clear(); }
            'G' => { 
                self.cursor_line = self.lines.len().saturating_sub(1);
                self.vim_buffer.clear(); 
            }
            'g' => {
                if rest_str == "g" {
                    self.cursor_line = 0;
                    self.vim_buffer.clear();
                } else {
                    self.vim_buffer.push('g');
                }
            }
            'y' => {
                if rest_str == "y" {
                    let mut yanked = Vec::new();
                    for i in 0..count {
                        if self.cursor_line + i < self.lines.len() {
                            yanked.push(self.lines[self.cursor_line + i].clone());
                        }
                    }
                    if !yanked.is_empty() {
                        if let Ok(mut ctx) = arboard::Clipboard::new() {
                            let _ = ctx.set_text(yanked.join("\n"));
                        }
                    }
                    self.clipboard = yanked;
                    self.status = format!("yanked {} lines", count);
                    self.vim_buffer.clear();
                } else {
                    self.vim_buffer.push('y');
                }
            }
            'd' => {
                if rest_str == "d" {
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
                        if let Ok(mut ctx) = arboard::Clipboard::new() {
                            let _ = ctx.set_text(deleted.join("\n"));
                        }
                    }
                    self.clipboard = deleted;
                    self.status = format!("deleted {} lines", count);
                    self.vim_buffer.clear();
                    self.mark_edited();
                    self.adjust_cursor();
                } else {
                    self.vim_buffer.push('d');
                }
            }
            'p' => {
                if let Ok(mut ctx) = arboard::Clipboard::new() {
                    if let Ok(text) = ctx.get_text() {
                        let sys_clip = text.split('\n').map(|s| s.to_string()).collect::<Vec<_>>();
                        if !sys_clip.is_empty() && (sys_clip != self.clipboard || self.clipboard.is_empty()) {
                            self.clipboard = sys_clip;
                        }
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
                    self.vim_buffer.clear();
                }
            }
            'v' => {
                self.mode = UiMode::Visual;
                self.selection_anchor = Some((self.cursor_line, self.cursor_col));
                self.status = "-- VISUAL --".to_string();
                self.vim_buffer.clear();
            }
            'V' => {
                self.mode = UiMode::VisualLine;
                self.selection_anchor = Some((self.cursor_line, self.cursor_col));
                self.status = "-- VISUAL LINE --".to_string();
                self.vim_buffer.clear();
            }
            _ => { self.vim_buffer.clear(); }
        }
    }

    fn handle_visual_key(&mut self, _db: &Db, key: Key) -> Result<(), String> {
        match key {
            Key::Esc | Key::Ctrl('c') => {
                self.mode = UiMode::Normal;
                self.selection_anchor = None;
                self.status = "-- NORMAL --".to_string();
            }
            Key::ArrowUp => self.move_cursor_up(1),
            Key::ArrowDown => self.move_cursor_down(1),
            Key::ArrowLeft => self.move_cursor_left(),
            Key::ArrowRight => self.move_cursor_right(),
            Key::Char(c) => {
                match c {
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
                        let anchor = self.selection_anchor.unwrap_or((self.cursor_line, self.cursor_col));
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
                                (min(anchor.1, self.cursor_col), std::cmp::max(anchor.1, self.cursor_col))
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
                            if let Ok(mut ctx) = arboard::Clipboard::new() {
                                let _ = ctx.set_text(yanked.join("\n"));
                            }
                            self.clipboard = yanked;
                        }
                        
                        self.mode = UiMode::Normal;
                        self.selection_anchor = None;
                        self.status = if is_delete { "-- NORMAL --".to_string() } else { "-- NORMAL -- (yanked)".to_string() };
                        if is_delete {
                            self.mark_edited();
                        }
                    }
                    _ => {}
                }
            }
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
                    if !c.is_alphanumeric() { self.switcher_query.pop(); } else { break; }
                }
                while let Some(c) = self.switcher_query.chars().last() {
                    if c.is_alphanumeric() { self.switcher_query.pop(); } else { break; }
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

    fn handle_command_bar_key(&mut self, key: Key) -> Result<(), String> {
        match key {
            Key::Esc => {
                self.mode = UiMode::Editor;
                self.command_input.clear();
                self.status = format!("editing {}", self.active_note.id);
            }
            Key::Enter => {
                let cmd = self.command_input.trim().to_string();
                self.mode = UiMode::Editor;
                self.command_input.clear();
                self.execute_terminal_command(&cmd);
            }
            Key::Tab => {
                let suggestions = crate::editor_core::commands::list_command_suggestions(
                    crate::editor_core::types::CommandMode::Editor,
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
                    self.mode = UiMode::Editor;
                    self.status = format!("editing {}", self.active_note.id);
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
            crate::editor_core::types::CommandMode::Editor,
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
        let result = crate::editor_core::commands::execute_command(
            &snapshot,
            cmd,
            crate::editor_core::types::CommandMode::Editor,
        );

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
        if !self.dirty {
            return Ok(());
        }
        let body = join_lines(&self.lines);
        let saved = db.save_note(&self.active_note.id, &body)?;
        self.active_note = saved;
        self.dirty = false;
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
        self.dirty = true;
        self.last_edit = Instant::now();
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
        for ch in text.chars() {
            self.insert_char(ch);
        }
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
        }.min(text.len());

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

        buf.push_str("\x1b[?25l\x1b[H\x1b[2J");

        let title = derive_title_from_lines(&self.lines);
        let title_line = format!(" note  [{}]  {}", self.active_note.id, title);
        draw_row(&mut buf, TITLE_ROW, cols, &title_line, true);

        let mut ctx = render::RenderContext::new();
        for line in self.lines.iter().take(self.scroll_line) {
            ctx.advance_line(line);
        }

        for i in 0..editor_height {
            let row = EDITOR_TOP_ROW + i;
            let line_idx = self.scroll_line + i;
            if line_idx < self.lines.len() {
                let line_no = line_idx + 1;
                let gutter = format!("{line_no:>4}  ");
                let available = cols.saturating_sub(GUTTER_WIDTH);
                let engine = crate::calc::engine::CalcEngine::new();
                let calc_ghost = get_calc_ghost_for_line(&self.lines[line_idx], &engine);
                let mut highlight_ranges = Vec::new();
                if let Some(anchor) = self.selection_anchor {
                    if self.mode == UiMode::Visual || self.mode == UiMode::VisualLine {
                        let start_line = min(anchor.0, self.cursor_line);
                        let end_line = std::cmp::max(anchor.0, self.cursor_line);
                        
                        if line_idx >= start_line && line_idx <= end_line {
                            if self.mode == UiMode::VisualLine {
                                let line_len = self.lines[line_idx].chars().count();
                                highlight_ranges.push((0, line_len.max(1)));
                            } else {
                                let (start_col, end_col) = if anchor.0 == self.cursor_line {
                                    (min(anchor.1, self.cursor_col), std::cmp::max(anchor.1, self.cursor_col))
                                } else if anchor.0 < self.cursor_line {
                                    (anchor.1, self.cursor_col)
                                } else {
                                    (self.cursor_col, anchor.1)
                                };
                                
                                if start_line == end_line {
                                    highlight_ranges.push((start_col, end_col + 1));
                                } else if line_idx == start_line {
                                    let line_len = self.lines[line_idx].chars().count();
                                    highlight_ranges.push((start_col, line_len.max(start_col + 1)));
                                } else if line_idx == end_line {
                                    highlight_ranges.push((0, end_col + 1));
                                } else {
                                    let line_len = self.lines[line_idx].chars().count();
                                    highlight_ranges.push((0, line_len.max(1)));
                                }
                            }
                        }
                    }
                }
                let rendered_text = ctx.render_line(&self.lines[line_idx], available, calc_ghost.as_deref(), &highlight_ranges);
                buf.push_str(&goto(row, 1));
                buf.push_str(&gutter);
                buf.push_str(&rendered_text);
            } else {
                draw_row(&mut buf, row, cols, "~", false);
            }
        }

        let status = match self.mode {
            UiMode::Editor | UiMode::Normal | UiMode::CommandBar | UiMode::Visual | UiMode::VisualLine => &self.status,
            UiMode::Switcher => "Switcher: type to filter, Enter open, Esc close",
            UiMode::DatePicker => "Date picker: arrows navigate, Ctrl+arrows months, Enter insert, Esc cancel",
        };
        draw_row(&mut buf, rows, cols, status, true);

        if self.mode == UiMode::Switcher {
            draw_switcher(self, &mut buf, rows, cols);
        }

        if self.mode == UiMode::DatePicker {
            draw_date_picker(self, &mut buf, rows, cols);
        }

        let (cursor_row, cursor_col) = self.cursor_position(rows, cols);
        buf.push_str(&goto(cursor_row, cursor_col));
        
        let cursor_style = match self.mode {
            UiMode::Editor | UiMode::CommandBar | UiMode::Switcher | UiMode::DatePicker => "\x1b[5 q", // Blinking Bar
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
                let visible = clip_text(line_text, cols.saturating_sub(GUTTER_WIDTH));
                let visible_col = min(clamped_col, visible.chars().count());
                let col = (GUTTER_WIDTH + visible_col + 1).min(cols.max(1));
                (row.max(1), col.max(1))
            }
            UiMode::Switcher => {
                let box_w = min(cols.saturating_sub(4).max(30), 72);
                let box_h = min(rows.saturating_sub(4).max(8), 14);
                let x = (cols.saturating_sub(box_w)) / 2 + 1;
                let y = (rows.saturating_sub(box_h)) / 2 + 1;
                let prompt = " search: ";
                let col = (x + prompt.chars().count() + self.switcher_query.chars().count())
                    .min(cols.max(1));
                (y + 1, col.max(1))
            }
        }
    }
}

pub fn run_terminal_session(
    db: &Db,
    _config: &ThemeConfig,
    opts: &TerminalOptions,
) -> Result<(), String> {
    if opts.list_only {
        print_note_list(db)?;
        return Ok(());
    }

    let mut app = TerminalApp::new(db, opts)?;
    app.run(db)
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

fn clip_text(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
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
    "January", "February", "March", "April", "May", "June",
    "July", "August", "September", "October", "November", "December",
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
    let selected = format!("{:04}-{:02}-{:02}", app.date_year, app.date_month, app.date_day);
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
        let Some(b) = read_byte()? else { break; };
        seq.push(b);
        if b.is_ascii_alphabetic() || b == b'~' { break; }
    }
    
    if seq.is_empty() { return Ok(Some(Key::Esc)); }
    
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
        if s == "1;5C" || s == "5C" { return Ok(Some(Key::CtrlArrowRight)); }
        if s == "1;5D" || s == "5D" { return Ok(Some(Key::CtrlArrowLeft)); }
        if s == "1~" || s == "7~" { return Ok(Some(Key::Home)); }
        if s == "4~" || s == "8~" { return Ok(Some(Key::End)); }
        if s == "3~" { return Ok(Some(Key::Delete)); }
        if s == "5~" { return Ok(Some(Key::PageUp)); }
        if s == "6~" { return Ok(Some(Key::PageDown)); }
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

fn get_calc_ghost_for_line(text: &str, engine: &crate::calc::engine::CalcEngine) -> Option<String> {
    if let Some((_, _, ghost)) = find_calc_segment(text, engine) {
        return Some(ghost);
    }
    
    let trimmed = text.trim();
    if trimmed.starts_with('|') && trimmed.ends_with('|') {
        return None;
    }
    
    engine.evaluate(text)
}

fn find_calc_segment(text: &str, engine: &crate::calc::engine::CalcEngine) -> Option<(usize, usize, String)> {
    let trimmed_text = text.trim();
    if trimmed_text.starts_with('|') && trimmed_text.ends_with('|') {
        let pipes: Vec<usize> = text.match_indices('|').map(|(i, _)| i).collect();
        if pipes.len() >= 2 {
            let mut candidates = Vec::new();
            for i in 0..pipes.len()-1 {
                let start = pipes[i] + 1;
                let end = pipes[i+1];
                if start >= end { continue; }
                let raw = &text[start..end];
                let trimmed = raw.trim();
                if let Some(ghost) = engine.evaluate(trimmed) {
                    let leading_ws = raw.len() - raw.trim_start().len();
                    let trailing_ws = raw.len() - raw.trim_end().len();
                    candidates.push((start + leading_ws, end - trailing_ws, ghost));
                }
            }
            if candidates.len() == 1 {
                return Some(candidates[0].clone());
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
        let trimmed = raw.trim();
        if let Some(ghost) = engine.evaluate(trimmed) {
            let leading_ws = raw.len() - raw.trim_start().len();
            let trailing_ws = raw.len() - raw.trim_end().len();
            return Some((start + leading_ws, text.len() - trailing_ws, ghost));
        }
    }
    
    None
}
