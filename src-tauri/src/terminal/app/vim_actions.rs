use super::{
    byte_index, line_char_len, min, split_lines, Db, Key, TerminalApp, TerminalVimAdapter, UiMode,
    VimMacroStep, VimPipelineResult, VimRegister, VimRegisterMode,
};
use super::{clipboard, ClipboardWriteBackend};
use crate::terminal::text_utils::{is_word_char, join_lines};

const VIM_MACRO_REPLAY_STEP_BUDGET: usize = 10_000;

fn can_scope_shared_vim_intent(intent: crate::editor_core::vim::VimIntent) -> bool {
    matches!(
        intent,
        crate::editor_core::vim::VimIntent::DeleteLine
            | crate::editor_core::vim::VimIntent::YankLine
            | crate::editor_core::vim::VimIntent::DeleteToLineStart
            | crate::editor_core::vim::VimIntent::DeleteToLineEnd
            | crate::editor_core::vim::VimIntent::YankToLineStart
            | crate::editor_core::vim::VimIntent::YankToLineEnd
            | crate::editor_core::vim::VimIntent::DeleteWordForward
            | crate::editor_core::vim::VimIntent::DeleteWordBackward
            | crate::editor_core::vim::VimIntent::DeleteWordEnd
            | crate::editor_core::vim::VimIntent::YankWordForward
            | crate::editor_core::vim::VimIntent::YankWordBackward
            | crate::editor_core::vim::VimIntent::DeleteChar
            | crate::editor_core::vim::VimIntent::DeleteInsideWord
            | crate::editor_core::vim::VimIntent::DeleteAroundWord
            | crate::editor_core::vim::VimIntent::YankInsideWord
            | crate::editor_core::vim::VimIntent::YankAroundWord
            | crate::editor_core::vim::VimIntent::DeleteInsidePipe
            | crate::editor_core::vim::VimIntent::DeleteAroundPipe
            | crate::editor_core::vim::VimIntent::YankInsidePipe
            | crate::editor_core::vim::VimIntent::YankAroundPipe
            | crate::editor_core::vim::VimIntent::DeleteInsideParen
            | crate::editor_core::vim::VimIntent::DeleteInsideBracket
            | crate::editor_core::vim::VimIntent::DeleteInsideBrace
            | crate::editor_core::vim::VimIntent::DeleteInsideDoubleQuote
            | crate::editor_core::vim::VimIntent::DeleteInsideBacktick
            | crate::editor_core::vim::VimIntent::PasteAfter
            | crate::editor_core::vim::VimIntent::DeleteTillChar
    )
}

fn key_from_insert_macro_step(key: crate::editor_core::vim::VimKey) -> Option<Key> {
    match key {
        crate::editor_core::vim::VimKey::Esc => Some(Key::Esc),
        crate::editor_core::vim::VimKey::Enter => Some(Key::Enter),
        crate::editor_core::vim::VimKey::Tab => Some(Key::Tab),
        crate::editor_core::vim::VimKey::Backspace => Some(Key::Backspace),
        crate::editor_core::vim::VimKey::Delete => Some(Key::Delete),
        crate::editor_core::vim::VimKey::ArrowUp => Some(Key::ArrowUp),
        crate::editor_core::vim::VimKey::ArrowDown => Some(Key::ArrowDown),
        crate::editor_core::vim::VimKey::ArrowLeft => Some(Key::ArrowLeft),
        crate::editor_core::vim::VimKey::ArrowRight => Some(Key::ArrowRight),
        crate::editor_core::vim::VimKey::Char(ch) => Some(Key::Char(ch)),
        crate::editor_core::vim::VimKey::Ctrl(_) => None,
    }
}

fn is_recordable_macro_intent(intent: crate::editor_core::vim::VimIntent) -> bool {
    !matches!(
        intent,
        crate::editor_core::vim::VimIntent::StartMacroRecord
            | crate::editor_core::vim::VimIntent::StopMacroRecord
            | crate::editor_core::vim::VimIntent::PlayMacro
            | crate::editor_core::vim::VimIntent::OpenCommandBar
            | crate::editor_core::vim::VimIntent::OpenSearch
    )
}

// Ownership: vim intent pipeline, text objects, and vim action application.
impl TerminalApp {
    pub(super) fn build_vim_context(&self) -> crate::editor_core::vim::VimContext {
        crate::editor_core::vim::VimContext {
            has_search_matches: !self.search_matches.is_empty(),
            line_count: self.lines.len(),
            macro_recording: self.vim_macro_recording.is_some(),
        }
    }

    pub(super) fn run_vim_pipeline(&mut self, db: &Db, key: &Key) -> VimPipelineResult {
        let perf_start = std::time::Instant::now();
        // Pipeline: terminal input -> vim intent translation -> shared-core
        // engine step -> terminal action rendering.
        let context = self.build_vim_context();
        let Some(step) = TerminalVimAdapter::step(&self.vim_state, key, &context) else {
            self.record_perf_duration("tui.vim.step", "no_intent", perf_start.elapsed());
            return VimPipelineResult::NoIntent;
        };
        self.vim_state = step.state;
        if !step.handled {
            self.record_perf_duration("tui.vim.step", "unhandled", perf_start.elapsed());
            return VimPipelineResult::Unhandled;
        }
        let doc_mutated = step
            .actions
            .iter()
            .any(|action| Self::vim_intent_mutates_document(action.intent));
        self.apply_vim_actions(db, &step.actions);
        self.record_perf_duration(
            "tui.vim.step",
            if doc_mutated { "mutating" } else { "movement" },
            perf_start.elapsed(),
        );
        VimPipelineResult::Applied { doc_mutated }
    }

    pub(super) fn set_clipboard_register(
        &mut self,
        register: VimRegister,
    ) -> Option<ClipboardWriteBackend> {
        if register.mode == VimRegisterMode::Charwise && register.text.is_empty() {
            return None;
        }
        self.clipboard_watch_last_text = Some(register.text.clone());
        let backend = clipboard::copy_text_to_clipboard(&register.text);
        self.last_clipboard_backend = backend;
        self.clipboard = register;
        backend
    }

    pub(super) fn set_clipboard_linewise(
        &mut self,
        lines: Vec<String>,
    ) -> Option<ClipboardWriteBackend> {
        if lines.is_empty() {
            return None;
        }
        self.set_clipboard_register(VimRegister::linewise(lines.join("\n")))
    }

    pub(super) fn set_clipboard_charwise(&mut self, text: String) -> Option<ClipboardWriteBackend> {
        self.set_clipboard_register(VimRegister::charwise(text))
    }

    pub(super) fn shared_vim_register(
        &self,
    ) -> Option<crate::editor_core::vim_actions::VimRegisterValue> {
        if self.clipboard.is_empty() {
            return None;
        }
        Some(crate::editor_core::vim_actions::VimRegisterValue {
            text: self.clipboard.text.clone(),
            mode: match self.clipboard.mode {
                VimRegisterMode::Charwise => {
                    crate::editor_core::vim_actions::VimRegisterMode::Charwise
                }
                VimRegisterMode::Linewise => {
                    crate::editor_core::vim_actions::VimRegisterMode::Linewise
                }
            },
        })
    }

    pub(super) fn try_execute_shared_vim_action(
        &mut self,
        intent: crate::editor_core::vim::VimIntent,
        count: usize,
        target_char: Option<char>,
    ) -> Option<(
        crate::editor_core::vim_actions::VimActionExecutionResult,
        usize,
    )> {
        if !crate::editor_core::vim_actions::supports_intent(intent) {
            return None;
        }
        if self.lines.len() >= 2048 && can_scope_shared_vim_intent(intent) {
            let center = self.cursor_line.min(self.lines.len().saturating_sub(1));
            let start = center.saturating_sub(96);
            let end = center
                .saturating_add(96)
                .min(self.lines.len().saturating_sub(1));
            let (snapshot, scope_start_offset) =
                self.build_scoped_snapshot_for_line_span(start, end, None);
            let register = self.shared_vim_register();
            let result = crate::editor_core::vim_actions::execute_vim_action_with_target(
                &snapshot.text,
                snapshot.selection,
                intent,
                count.max(1),
                register.as_ref(),
                target_char,
            )?;
            Some((result, scope_start_offset))
        } else {
            if self.joined_text_cache.is_none() {
                self.joined_text_cache = Some(join_lines(&self.lines));
            }
            let fallback_cursor = self.byte_offset_for_line_col(self.cursor_line, self.cursor_col);
            let selection = self.command_selection.unwrap_or(
                crate::editor_core::types::SelectionSnapshot {
                    anchor: fallback_cursor,
                    head: fallback_cursor,
                },
            );
            let register = self.shared_vim_register();
            let text: &str = self.joined_text_cache.as_deref().unwrap();
            let result = crate::editor_core::vim_actions::execute_vim_action_with_target(
                text,
                selection,
                intent,
                count.max(1),
                register.as_ref(),
                target_char,
            )?;
            Some((result, 0))
        }
    }

    pub(super) fn apply_shared_vim_action_result(
        &mut self,
        result: crate::editor_core::vim_actions::VimActionExecutionResult,
    ) {
        for operation in &result.operations {
            self.apply_edit_operation(operation);
        }
        if let Some(register) = result.register {
            let mode = match register.mode {
                crate::editor_core::vim_actions::VimRegisterMode::Charwise => {
                    VimRegisterMode::Charwise
                }
                crate::editor_core::vim_actions::VimRegisterMode::Linewise => {
                    VimRegisterMode::Linewise
                }
            };
            let _ = self.set_clipboard_register(VimRegister {
                text: register.text,
                mode,
            });
        }
    }

    pub(super) fn with_clipboard_status(&self, base: impl Into<String>) -> String {
        let base = base.into();
        match self.last_clipboard_backend {
            Some(backend) => format!("{base} [clipboard: {}]", backend.label()),
            None => format!("{base} [clipboard: local only]"),
        }
    }

    pub(super) fn read_system_clipboard_text(&self) -> Option<String> {
        clipboard::read_clipboard_via_commands().or_else(|| {
            if let Ok(mut ctx) = arboard::Clipboard::new() {
                ctx.get_text().ok()
            } else {
                None
            }
        })
    }

    pub(super) fn slice_current_line_cols(
        &self,
        start_col: usize,
        end_col: usize,
    ) -> Option<String> {
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

    pub(super) fn delete_current_line_cols(
        &mut self,
        start_col: usize,
        end_col: usize,
    ) -> Option<String> {
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

    pub(super) fn line_col_lt(
        left_line: usize,
        left_col: usize,
        right_line: usize,
        right_col: usize,
    ) -> bool {
        left_line < right_line || (left_line == right_line && left_col < right_col)
    }

    pub(super) fn slice_cols_range(
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

    pub(super) fn delete_cols_range(
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

    pub(super) fn find_word_object_bounds(&self, around: bool) -> Option<(usize, usize)> {
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

    pub(super) fn find_pipe_object_bounds(&self, around: bool) -> Option<(usize, usize)> {
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

    pub(super) fn apply_word_text_object(
        &mut self,
        around: bool,
        delete: bool,
        count: usize,
    ) -> usize {
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

        self.set_clipboard_charwise(chunks.join("\n"));
        if changed {
            self.mark_edited();
            self.adjust_cursor();
        }
        applied
    }

    pub(super) fn apply_pipe_text_object(
        &mut self,
        around: bool,
        delete: bool,
        count: usize,
    ) -> usize {
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

        self.set_clipboard_charwise(chunks.join("\n"));
        if changed {
            self.mark_edited();
            self.adjust_cursor();
        }
        applied
    }

    pub(super) fn apply_visual_selection_action(&mut self, delete: bool) -> bool {
        if !matches!(self.mode, UiMode::Visual | UiMode::VisualLine) {
            return false;
        }

        let anchor = self
            .selection_anchor
            .unwrap_or((self.cursor_line, self.cursor_col));
        let mut start_line = min(anchor.0, self.cursor_line);
        let mut end_line = std::cmp::max(anchor.0, self.cursor_line);

        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        start_line = start_line.min(self.lines.len().saturating_sub(1));
        end_line = end_line.min(self.lines.len().saturating_sub(1));

        let mut yanked = Vec::new();
        if self.mode == UiMode::VisualLine {
            for i in start_line..=end_line {
                if i < self.lines.len() {
                    yanked.push(self.lines[i].clone());
                }
            }
            if delete {
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

                if delete {
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

                if delete {
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

        if yanked.is_empty() {
            return false;
        }

        if self.mode == UiMode::VisualLine {
            self.set_clipboard_linewise(yanked);
        } else {
            self.set_clipboard_charwise(yanked.join("\n"));
        }

        self.mode = UiMode::Normal;
        self.selection_anchor = None;
        self.command_selection = None;
        self.command_selection_linewise = false;
        self.status = if delete {
            self.with_clipboard_status("-- NORMAL --")
        } else {
            self.with_clipboard_status("-- NORMAL -- (yanked)")
        };
        if delete {
            self.mark_edited();
        }
        true
    }

    pub(super) fn open_command_bar_from_vim_action(&mut self) {
        if self.mode == UiMode::Visual || self.mode == UiMode::VisualLine {
            self.command_selection_linewise = self.mode == UiMode::VisualLine;
            self.command_selection = self.capture_visual_command_selection();
        } else {
            self.command_selection = None;
            self.command_selection_linewise = false;
        }
        self.command_input.clear();
        self.dismiss_command_completion_menu();
        self.command_history_index = None;
        self.command_bar_from_normal = true;
        self.mode = UiMode::CommandBar;
        self.status = ":".to_string();
    }

    pub(super) fn apply_vim_actions(
        &mut self,
        db: &Db,
        actions: &[crate::editor_core::vim::VimAction],
    ) {
        for action in actions {
            let count = action.count.max(1);
            if !self.vim_macro_replaying {
                if let Some(register) = self.vim_macro_recording {
                    if is_recordable_macro_intent(action.intent) {
                        self.vim_macro_registers
                            .entry(register)
                            .or_default()
                            .push(VimMacroStep::Action(action.clone()));
                    }
                }
            }
            match action.intent {
                crate::editor_core::vim::VimIntent::StartMacroRecord => {
                    if let Some(register) = action.target_char.map(|ch| ch.to_ascii_lowercase()) {
                        self.vim_macro_recording = Some(register);
                        self.vim_macro_registers.insert(register, Vec::new());
                        self.status = format!("recording @{}", register);
                    } else {
                        self.status = "macro register required".to_string();
                    }
                    continue;
                }
                crate::editor_core::vim::VimIntent::StopMacroRecord => {
                    if let Some(register) = self.vim_macro_recording.take() {
                        let steps = self
                            .vim_macro_registers
                            .get(&register)
                            .map(|items| items.len())
                            .unwrap_or(0);
                        self.status = format!("recorded @{} ({} steps)", register, steps);
                    } else {
                        self.status = "no active macro recording".to_string();
                    }
                    continue;
                }
                crate::editor_core::vim::VimIntent::PlayMacro => {
                    if self.vim_macro_replaying {
                        continue;
                    }
                    let Some(register) = action.target_char.map(|ch| ch.to_ascii_lowercase())
                    else {
                        self.status = "macro register required".to_string();
                        continue;
                    };
                    let Some(sequence_len) = self.vim_macro_registers.get(&register).map(Vec::len)
                    else {
                        self.status = format!("macro @{} is empty", register);
                        continue;
                    };
                    if sequence_len == 0 {
                        self.status = format!("macro @{} is empty", register);
                        continue;
                    }
                    let Some(total_steps) = count.checked_mul(sequence_len) else {
                        self.status = format!(
                            "macro @{} replay aborted: step budget exceeded (>{})",
                            register, VIM_MACRO_REPLAY_STEP_BUDGET
                        );
                        continue;
                    };
                    if total_steps > VIM_MACRO_REPLAY_STEP_BUDGET {
                        self.status = format!(
                            "macro @{} replay aborted: step budget exceeded (>{})",
                            register, VIM_MACRO_REPLAY_STEP_BUDGET
                        );
                        continue;
                    }
                    self.vim_macro_replaying = true;
                    let mut executed_steps = 0usize;
                    let mut aborted = false;
                    for _ in 0..count {
                        for idx in 0..sequence_len {
                            if executed_steps >= VIM_MACRO_REPLAY_STEP_BUDGET {
                                aborted = true;
                                break;
                            }
                            let Some(step) = self
                                .vim_macro_registers
                                .get(&register)
                                .and_then(|sequence| sequence.get(idx))
                                .cloned()
                            else {
                                aborted = true;
                                break;
                            };
                            match step {
                                VimMacroStep::Action(vim_action) => {
                                    self.apply_vim_actions(db, &[vim_action]);
                                }
                                VimMacroStep::InsertKey(insert_key) => {
                                    if self.mode == UiMode::Editor {
                                        let key = key_from_insert_macro_step(insert_key);
                                        if let Some(key) = key {
                                            let _ = self.handle_editor_key(db, key);
                                        }
                                    }
                                }
                            }
                            executed_steps += 1;
                        }
                        if aborted {
                            break;
                        }
                    }
                    self.vim_macro_replaying = false;
                    if aborted {
                        self.status = format!(
                            "macro @{} replay aborted: step budget exceeded (>{})",
                            register, VIM_MACRO_REPLAY_STEP_BUDGET
                        );
                    } else {
                        self.status = format!("replayed @{} x{}", register, count);
                    }
                    continue;
                }
                _ => {}
            }
            if !self.active_note_is_editable() && Self::vim_intent_mutates_document(action.intent) {
                self.set_locked_note_status();
                continue;
            }
            if let Some((shared, scope_start_offset)) =
                self.try_execute_shared_vim_action(action.intent, count, action.target_char)
            {
                let had_register = shared.register.is_some();
                let mapped = crate::editor_core::vim_actions::VimActionExecutionResult {
                    operations: shared
                        .operations
                        .iter()
                        .map(|op| Self::remap_operation_from_scope(op, scope_start_offset))
                        .collect(),
                    register: shared.register,
                };
                self.apply_shared_vim_action_result(mapped);
                if had_register {
                    let status = match action.intent {
                        crate::editor_core::vim::VimIntent::DeleteLine => {
                            Some(self.with_clipboard_status(format!("deleted {} lines", count)))
                        }
                        crate::editor_core::vim::VimIntent::YankLine => {
                            Some(self.with_clipboard_status(format!("yanked {} lines", count)))
                        }
                        crate::editor_core::vim::VimIntent::DeleteToLineStart => {
                            Some(self.with_clipboard_status("deleted to line start"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteToLineEnd => {
                            Some(self.with_clipboard_status("deleted to line end"))
                        }
                        crate::editor_core::vim::VimIntent::YankToLineStart => {
                            Some(self.with_clipboard_status("yanked to line start"))
                        }
                        crate::editor_core::vim::VimIntent::YankToLineEnd => {
                            Some(self.with_clipboard_status("yanked to line end"))
                        }
                        crate::editor_core::vim::VimIntent::YankWordForward => {
                            Some(self.with_clipboard_status("yanked word forward"))
                        }
                        crate::editor_core::vim::VimIntent::YankWordBackward => {
                            Some(self.with_clipboard_status("yanked word backward"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteWordForward => {
                            Some(self.with_clipboard_status("deleted word forward"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteWordBackward => {
                            Some(self.with_clipboard_status("deleted word backward"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteWordEnd => {
                            Some(self.with_clipboard_status("deleted to word end"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteTillChar => {
                            Some(self.with_clipboard_status("deleted till char"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteInsideWord => {
                            Some(self.with_clipboard_status("deleted inside word"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteAroundWord => {
                            Some(self.with_clipboard_status("deleted around word"))
                        }
                        crate::editor_core::vim::VimIntent::YankInsideWord => {
                            Some(self.with_clipboard_status("yanked inside word"))
                        }
                        crate::editor_core::vim::VimIntent::YankAroundWord => {
                            Some(self.with_clipboard_status("yanked around word"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteInsidePipe => {
                            Some(self.with_clipboard_status("deleted inside | |"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteAroundPipe => {
                            Some(self.with_clipboard_status("deleted around | |"))
                        }
                        crate::editor_core::vim::VimIntent::YankInsidePipe => {
                            Some(self.with_clipboard_status("yanked inside | |"))
                        }
                        crate::editor_core::vim::VimIntent::YankAroundPipe => {
                            Some(self.with_clipboard_status("yanked around | |"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteInsideParen => {
                            Some(self.with_clipboard_status("deleted inside ( )"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteInsideBracket => {
                            Some(self.with_clipboard_status("deleted inside [ ]"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteInsideBrace => {
                            Some(self.with_clipboard_status("deleted inside { }"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteInsideDoubleQuote => {
                            Some(self.with_clipboard_status("deleted inside \" \""))
                        }
                        crate::editor_core::vim::VimIntent::DeleteInsideBacktick => {
                            Some(self.with_clipboard_status("deleted inside ` `"))
                        }
                        _ => None,
                    };
                    if let Some(status) = status {
                        self.status = status;
                    }
                }
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
                        self.set_clipboard_linewise(deleted);
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
                        self.set_clipboard_linewise(yanked);
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
                        self.set_clipboard_charwise(chunks.join("\n"));
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
                        self.set_clipboard_charwise(chunks.join("\n"));
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
                        self.set_clipboard_charwise(chunks.join("\n"));
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
                        self.set_clipboard_charwise(chunks.join("\n"));
                        self.status = self.with_clipboard_status("yanked to line end");
                    }
                }
                crate::editor_core::vim::VimIntent::DeleteChar => {
                    for _ in 0..count {
                        self.delete_forward();
                    }
                }
                crate::editor_core::vim::VimIntent::PasteAfter => {
                    if let Some(sys_clip_text) = self.read_system_clipboard_text() {
                        if self.clipboard.is_empty() || self.clipboard.text != sys_clip_text {
                            self.clipboard = VimRegister::charwise(sys_clip_text);
                        }
                    }
                    if !self.clipboard.is_empty() {
                        match self.clipboard.mode {
                            VimRegisterMode::Linewise => {
                                let normalized = self
                                    .clipboard
                                    .text
                                    .strip_suffix('\n')
                                    .unwrap_or_else(|| self.clipboard.text.as_str());
                                let lines = if normalized.is_empty() {
                                    vec![String::new()]
                                } else {
                                    normalized
                                        .split('\n')
                                        .map(|line| line.to_string())
                                        .collect::<Vec<_>>()
                                };
                                let mut repeated = Vec::with_capacity(lines.len() * count);
                                for _ in 0..count {
                                    repeated.extend(lines.iter().cloned());
                                }
                                if !repeated.is_empty() {
                                    let insert_at = self.cursor_line + 1;
                                    for (offset, line) in repeated.iter().enumerate() {
                                        self.lines.insert(insert_at + offset, line.clone());
                                    }
                                    self.cursor_line = insert_at;
                                    self.cursor_col = 0;
                                    self.mark_edited();
                                }
                            }
                            VimRegisterMode::Charwise => {
                                let text = self.clipboard.text.clone();
                                let mut inserted = false;
                                for _ in 0..count {
                                    let line_len = line_char_len(self.current_line());
                                    if self.cursor_col < line_len {
                                        self.cursor_col += 1;
                                    } else {
                                        self.cursor_col = line_len;
                                    }
                                    self.insert_paste(&text);
                                    inserted = true;
                                }
                                if inserted {
                                    self.adjust_cursor();
                                }
                            }
                        };
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
                        self.set_clipboard_charwise(deleted.join(""));
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
                        self.set_clipboard_charwise(deleted.join(""));
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
                        self.set_clipboard_charwise(yanked.join(""));
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
                        self.set_clipboard_charwise(yanked.join(""));
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
                    self.open_command_bar_from_vim_action();
                }
                crate::editor_core::vim::VimIntent::OpenSearch => self.open_search(),
                crate::editor_core::vim::VimIntent::SearchNext => self.search_next(),
                crate::editor_core::vim::VimIntent::SearchPrev => self.search_prev(),
                crate::editor_core::vim::VimIntent::YankVisualSelection => {
                    let _ = self.apply_visual_selection_action(false);
                }
                crate::editor_core::vim::VimIntent::DeleteVisualSelection => {
                    let _ = self.apply_visual_selection_action(true);
                }
                crate::editor_core::vim::VimIntent::StartMacroRecord => {}
                crate::editor_core::vim::VimIntent::StopMacroRecord => {}
                crate::editor_core::vim::VimIntent::PlayMacro => {}
                crate::editor_core::vim::VimIntent::DeleteWordEnd => {}
                crate::editor_core::vim::VimIntent::DeleteTillChar => {}
                crate::editor_core::vim::VimIntent::DeleteInsideParen => {}
                crate::editor_core::vim::VimIntent::DeleteInsideBracket => {}
                crate::editor_core::vim::VimIntent::DeleteInsideBrace => {}
                crate::editor_core::vim::VimIntent::DeleteInsideDoubleQuote => {}
                crate::editor_core::vim::VimIntent::DeleteInsideBacktick => {}
                crate::editor_core::vim::VimIntent::Swallow => {}
            }
        }
    }
}
