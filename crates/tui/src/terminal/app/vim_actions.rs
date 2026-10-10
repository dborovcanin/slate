use super::{clipboard, ClipboardWriteBackend};
use super::{
    line_char_len, Db, Key, TerminalApp, TerminalVimAdapter, UiMode, VimMacroStep,
    VimPipelineResult, VimRegister, VimRegisterMode,
};
use crate::terminal::text_utils::join_lines;

const VIM_MACRO_REPLAY_STEP_BUDGET: usize = 10_000;

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

// Vertical line motions whose desired text column should survive the move,
// rather than snapping to a table cell. Mirrors the `MoveUp`/`MoveDown` intents
// emitted for j/k and the arrow keys.
fn vim_intent_preserves_vertical_text_column(intent: crate::editor_core::vim::VimIntent) -> bool {
    matches!(
        intent,
        crate::editor_core::vim::VimIntent::MoveUp | crate::editor_core::vim::VimIntent::MoveDown
    )
}

fn vim_intent_mirrors_register_to_system_clipboard(
    intent: crate::editor_core::vim::VimIntent,
) -> bool {
    matches!(
        intent,
        crate::editor_core::vim::VimIntent::YankLine
            | crate::editor_core::vim::VimIntent::YankToLineStart
            | crate::editor_core::vim::VimIntent::YankToLineEnd
            | crate::editor_core::vim::VimIntent::YankWordForward
            | crate::editor_core::vim::VimIntent::YankWordBackward
            | crate::editor_core::vim::VimIntent::YankInsideWord
            | crate::editor_core::vim::VimIntent::YankAroundWord
            | crate::editor_core::vim::VimIntent::YankInsidePipe
            | crate::editor_core::vim::VimIntent::YankAroundPipe
            | crate::editor_core::vim::VimIntent::YankVisualSelection
    )
}

// Ownership: vim intent pipeline, text objects, and vim action application.
impl TerminalApp {
    pub(super) fn build_vim_context(&self) -> crate::editor_core::vim::VimContext {
        crate::editor_core::vim::VimContext {
            has_search_matches: !self.search.matches.is_empty(),
            line_count: self.editor.lines.len(),
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
        let preserve_vertical_column = !doc_mutated
            && step
                .actions
                .iter()
                .any(|action| vim_intent_preserves_vertical_text_column(action.intent));
        self.apply_vim_actions(db, &step.actions);
        self.record_perf_duration(
            "tui.vim.step",
            if doc_mutated { "mutating" } else { "movement" },
            perf_start.elapsed(),
        );
        VimPipelineResult::Applied {
            doc_mutated,
            preserve_vertical_column,
        }
    }

    pub(super) fn set_clipboard_register(
        &mut self,
        register: VimRegister,
    ) -> Option<ClipboardWriteBackend> {
        if register.is_empty() {
            return None;
        }
        let backend = clipboard::copy_text_to_clipboard(&register.text);
        if backend.is_some() {
            self.clipboard_watch.last_text = Some(register.text.clone());
        }
        self.last_clipboard_backend = backend;
        self.clipboard = register;
        backend
    }

    pub(super) fn set_vim_register(&mut self, register: VimRegister) -> bool {
        if register.is_empty() {
            return false;
        }
        self.last_clipboard_backend = None;
        self.clipboard = register;
        true
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
        Some(self.clipboard.clone())
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
        // Large notes run the action on just the lines it can touch.
        let scope = crate::editor_core::vim_actions::scoped_line_range(
            intent,
            count,
            &self.editor.lines,
            self.editor.cursor_line,
        )
        .filter(|_| self.editor.lines.len() >= 2048);
        if let Some((start, end)) = scope {
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
            return Some((result, scope_start_offset));
        }
        if self.editor.joined_text_cache.is_none() {
            self.editor.joined_text_cache = Some(join_lines(&self.editor.lines));
        }
        let fallback_cursor =
            self.byte_offset_for_line_col(self.editor.cursor_line, self.editor.cursor_col);
        let selection =
            self.command_selection
                .unwrap_or(crate::editor_core::types::SelectionSnapshot {
                    anchor: fallback_cursor,
                    head: fallback_cursor,
                });
        let register = self.shared_vim_register();
        let text: &str = self.editor.joined_text_cache.as_deref().unwrap();
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

    pub(super) fn apply_shared_vim_action_result(
        &mut self,
        result: crate::editor_core::vim_actions::VimActionExecutionResult,
        mirror_register_to_system_clipboard: bool,
    ) {
        for operation in &result.operations {
            self.apply_edit_operation(operation);
        }
        if let Some(register) = result.register {
            if mirror_register_to_system_clipboard {
                let _ = self.set_clipboard_register(register);
            } else {
                let _ = self.set_vim_register(register);
            }
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
        clipboard::read_clipboard_text()
    }

    /// The system clipboard as a register: CSV or TSV becomes table rows,
    /// pasted as whole lines; anything else is text.
    fn read_system_clipboard_register(&mut self) -> Option<VimRegister> {
        let text = self.read_system_clipboard_text()?;
        Some(match self.pasted_table(&text) {
            Some(table) => VimRegister::linewise(table.join("\n")),
            None => VimRegister::charwise(text),
        })
    }

    /// Imports a clipboard image into the note and returns its markdown,
    /// or none with the failure in the status line.
    pub(super) fn import_clipboard_image(&mut self, db: &Db, image: &[u8]) -> Option<String> {
        match self.import_image_bytes(db, image) {
            Ok(markdown) => {
                self.status = "image inserted".to_string();
                Some(markdown)
            }
            Err(error) => {
                self.status = format!("image paste failed: {error}");
                None
            }
        }
    }

    pub(super) fn apply_visual_selection_action(&mut self, delete: bool) -> bool {
        if !matches!(self.mode, UiMode::Visual | UiMode::VisualLine) {
            return false;
        }
        if self.editor.lines.is_empty() {
            self.editor.lines.push(String::new());
        }
        use crate::editor_core::buffer::primitives::BufferCursor;
        let anchor = self
            .editor
            .selection_anchor
            .unwrap_or((self.editor.cursor_line, self.editor.cursor_col));
        let Some(plan) = crate::editor_core::vim_actions::buffer::prepare_visual_selection(
            &self.editor.lines,
            self.editor.cursor(),
            BufferCursor {
                line: anchor.0,
                column: anchor.1,
            },
            self.mode == UiMode::VisualLine,
            delete,
        ) else {
            return false;
        };
        let history_delta = plan.text_changed.then_some(plan.delta);
        if let Some((start, end)) = plan.deleted_lines {
            self.note_deleted_lines(start, end);
        }
        if let Some(edit) = plan.exact_edit {
            self.note_line_edit(edit.from, edit.to, edit.inserted_breaks);
        }
        let (register, cursor) = plan.apply(&mut self.editor.lines);
        self.editor.set_cursor(cursor);
        if delete {
            let _ = self.set_vim_register(register);
        } else {
            let _ = self.set_clipboard_register(register);
        }
        self.mode = UiMode::Normal;
        self.editor.selection_anchor = None;
        self.command_selection = None;
        self.command_selection_linewise = false;
        self.status = self.with_clipboard_status(if delete {
            "-- NORMAL --"
        } else {
            "-- NORMAL -- (yanked)"
        });
        if delete {
            self.mark_edited_from_line_with_span(
                history_delta.map_or(self.editor.cursor_line, |delta| delta.start_line),
                history_delta,
            );
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
        self.update_command_status();
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
                if action.intent == crate::editor_core::vim::VimIntent::DeleteLine
                    && !shared.operations.is_empty()
                {
                    let start = self.editor.cursor_line;
                    self.drop_reminders_on_deleted_lines(start, start + count.max(1) - 1);
                }
                let had_register = shared.register.is_some();
                let mapped = crate::editor_core::vim_actions::VimActionExecutionResult {
                    operations: shared
                        .operations
                        .iter()
                        .map(|op| Self::remap_operation_from_scope(op, scope_start_offset))
                        .collect(),
                    register: shared.register,
                };
                self.apply_shared_vim_action_result(
                    mapped,
                    vim_intent_mirrors_register_to_system_clipboard(action.intent),
                );
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
                        crate::editor_core::vim::VimIntent::DeleteInsideAsterisk => {
                            Some(self.with_clipboard_status("deleted inside * *"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteInsideTilde => {
                            Some(self.with_clipboard_status("deleted inside ~ ~"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteInsideUnderscore => {
                            Some(self.with_clipboard_status("deleted inside _ _"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteAroundParen => {
                            Some(self.with_clipboard_status("deleted around ( )"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteAroundBracket => {
                            Some(self.with_clipboard_status("deleted around [ ]"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteAroundBrace => {
                            Some(self.with_clipboard_status("deleted around { }"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteAroundDoubleQuote => {
                            Some(self.with_clipboard_status("deleted around \" \""))
                        }
                        crate::editor_core::vim::VimIntent::DeleteAroundBacktick => {
                            Some(self.with_clipboard_status("deleted around ` `"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteAroundAsterisk => {
                            Some(self.with_clipboard_status("deleted around * *"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteAroundTilde => {
                            Some(self.with_clipboard_status("deleted around ~ ~"))
                        }
                        crate::editor_core::vim::VimIntent::DeleteAroundUnderscore => {
                            Some(self.with_clipboard_status("deleted around _ _"))
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
                crate::editor_core::vim::VimIntent::MoveScreenUp => {
                    self.move_cursor_screen(true, count)
                }
                crate::editor_core::vim::VimIntent::MoveScreenDown => {
                    self.move_cursor_screen(false, count)
                }
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
                crate::editor_core::vim::VimIntent::MoveLineStart => {
                    self.editor.cursor_col =
                        crate::editor_core::vim_actions::buffer::insert_entry_column(
                            self.editor.cursor_col,
                            0,
                            action.intent,
                        )
                }
                crate::editor_core::vim::VimIntent::MoveLineEnd => {
                    let line_len = line_char_len(self.current_line());
                    self.editor.cursor_col =
                        crate::editor_core::vim_actions::buffer::insert_entry_column(
                            self.editor.cursor_col,
                            line_len,
                            action.intent,
                        );
                }
                crate::editor_core::vim::VimIntent::MoveDocStart => self.editor.cursor_line = 0,
                crate::editor_core::vim::VimIntent::MoveDocEnd => {
                    self.editor.cursor_line = self
                        .real_line_for_virtual(self.visible_line_count().saturating_sub(1))
                        .unwrap_or_else(|| self.editor.lines.len().saturating_sub(1))
                }
                crate::editor_core::vim::VimIntent::MoveToLine => {
                    let target_virtual = count.max(1).min(self.visible_line_count()) - 1;
                    self.editor.cursor_line = self
                        .real_line_for_virtual(target_virtual)
                        .unwrap_or_else(|| self.editor.lines.len().saturating_sub(1));
                }
                crate::editor_core::vim::VimIntent::EnterInsert => {
                    self.mode = UiMode::Editor;
                    self.status = "-- INSERT --".to_string();
                }
                crate::editor_core::vim::VimIntent::AppendInsert => {
                    let line_len = line_char_len(self.current_line());
                    self.editor.cursor_col =
                        crate::editor_core::vim_actions::buffer::insert_entry_column(
                            self.editor.cursor_col,
                            line_len,
                            action.intent,
                        );
                    self.mode = UiMode::Editor;
                    self.status = "-- INSERT --".to_string();
                }
                crate::editor_core::vim::VimIntent::InsertLineStart => {
                    self.editor.cursor_col =
                        crate::editor_core::vim_actions::buffer::insert_entry_column(
                            self.editor.cursor_col,
                            0,
                            action.intent,
                        );
                    self.mode = UiMode::Editor;
                    self.status = "-- INSERT --".to_string();
                }
                crate::editor_core::vim::VimIntent::AppendLineEnd => {
                    self.editor.cursor_col =
                        crate::editor_core::vim_actions::buffer::insert_entry_column(
                            self.editor.cursor_col,
                            line_char_len(self.current_line()),
                            action.intent,
                        );
                    self.mode = UiMode::Editor;
                    self.status = "-- INSERT --".to_string();
                }
                crate::editor_core::vim::VimIntent::OpenLineBelow => {
                    self.editor.cursor_col =
                        crate::editor_core::vim_actions::buffer::insert_entry_column(
                            self.editor.cursor_col,
                            line_char_len(self.current_line()),
                            action.intent,
                        );
                    self.insert_newline();
                    self.mode = UiMode::Editor;
                    self.status = "-- INSERT --".to_string();
                }
                crate::editor_core::vim::VimIntent::OpenLineAbove => {
                    self.editor.cursor_col =
                        crate::editor_core::vim_actions::buffer::insert_entry_column(
                            self.editor.cursor_col,
                            0,
                            action.intent,
                        );
                    let current = self.editor.cursor_line;
                    self.note_lines_inserted(current, 1);
                    crate::editor_core::vim_actions::buffer::insert_empty_line_above(
                        &mut self.editor.lines,
                        current,
                    );
                    self.mode = UiMode::Editor;
                    self.status = "-- INSERT --".to_string();
                }
                crate::editor_core::vim::VimIntent::EnterVisual => {
                    self.mode = UiMode::Visual;
                    self.editor.selection_anchor =
                        Some((self.editor.cursor_line, self.editor.cursor_col));
                    self.status = "-- VISUAL --".to_string();
                }
                crate::editor_core::vim::VimIntent::EnterVisualLine => {
                    self.mode = UiMode::VisualLine;
                    self.editor.selection_anchor =
                        Some((self.editor.cursor_line, self.editor.cursor_col));
                    self.status = "-- VISUAL LINE --".to_string();
                }
                crate::editor_core::vim::VimIntent::ExitVisual => {
                    if self.mode == UiMode::Visual || self.mode == UiMode::VisualLine {
                        self.mode = UiMode::Normal;
                        self.editor.selection_anchor = None;
                        self.status = "-- NORMAL --".to_string();
                    }
                }
                crate::editor_core::vim::VimIntent::PasteAfter => {
                    if self.clipboard.is_empty() {
                        // An image is imported once; its markdown then pastes
                        // like clipboard text, so a count repeats the link.
                        let pasted = match clipboard::read_clipboard_image_via_commands(true) {
                            Some(image) => self
                                .import_clipboard_image(db, &image)
                                .map(VimRegister::charwise),
                            None => self.read_system_clipboard_register(),
                        };
                        if let Some(register) = pasted {
                            self.clipboard = register;
                        }
                    }
                    if !self.clipboard.is_empty() {
                        match self.clipboard.mode {
                            VimRegisterMode::Linewise => {
                                let repeated =
                                    crate::editor_core::vim_actions::buffer::linewise_paste_lines(
                                        &self.clipboard.text,
                                        count,
                                    );
                                if !repeated.is_empty() {
                                    let insert_at = self.editor.cursor_line + 1;
                                    self.note_lines_inserted(insert_at, repeated.len());
                                    for (offset, line) in repeated.iter().enumerate() {
                                        self.editor.lines.insert(insert_at + offset, line.clone());
                                    }
                                    self.editor.cursor_line = insert_at;
                                    self.editor.cursor_col = 0;
                                    self.mark_edited();
                                }
                            }
                            VimRegisterMode::Charwise => {
                                let text = self.clipboard.text.clone();
                                let mut inserted = false;
                                for _ in 0..count {
                                    let line_len = line_char_len(self.current_line());
                                    self.editor.cursor_col =
                                        crate::editor_core::vim_actions::buffer::paste_after_column(
                                            self.editor.cursor_col,
                                            line_len,
                                        );
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
                crate::editor_core::vim::VimIntent::PasteBefore => {
                    // Reached only with an empty register: paste the system
                    // clipboard, like `p` does.
                    let pasted = match clipboard::read_clipboard_image_via_commands(true) {
                        Some(image) => self
                            .import_clipboard_image(db, &image)
                            .map(VimRegister::charwise),
                        None => self.read_system_clipboard_register(),
                    };
                    if let Some(register) = pasted {
                        self.clipboard = register;
                        if let Some((shared, scope_start_offset)) =
                            self.try_execute_shared_vim_action(action.intent, count, None)
                        {
                            let mapped =
                                crate::editor_core::vim_actions::VimActionExecutionResult {
                                    operations: shared
                                        .operations
                                        .iter()
                                        .map(|op| {
                                            Self::remap_operation_from_scope(op, scope_start_offset)
                                        })
                                        .collect(),
                                    register: None,
                                };
                            self.apply_shared_vim_action_result(mapped, false);
                        }
                    }
                }
                crate::editor_core::vim::VimIntent::Undo => {
                    for _ in 0..count {
                        self.undo(db);
                    }
                }
                crate::editor_core::vim::VimIntent::Redo => {
                    for _ in 0..count {
                        self.redo(db);
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
                // These intents are all resolved by the shared core path
                // (`try_execute_shared_vim_action` / `supports_intent`), which
                // runs before this match and `continue`s, so reaching one here is
                // unreachable in practice (core always returns an action for them —
                // see `execute_vim_action_with_target`). They are listed only to
                // keep this match exhaustive — deliberately no wildcard, so a new
                // intent forces a compile error here. `PasteAfter` and
                // `PasteBefore` are the `supports_intent` exceptions handled
                // above, because they can fall through with an empty register and
                // then pull from the system clipboard.
                crate::editor_core::vim::VimIntent::DeleteLine
                | crate::editor_core::vim::VimIntent::ChangeLine
                | crate::editor_core::vim::VimIntent::YankLine
                | crate::editor_core::vim::VimIntent::DeleteToLineStart
                | crate::editor_core::vim::VimIntent::DeleteToLineEnd
                | crate::editor_core::vim::VimIntent::YankToLineStart
                | crate::editor_core::vim::VimIntent::YankToLineEnd
                | crate::editor_core::vim::VimIntent::DeleteChar
                | crate::editor_core::vim::VimIntent::DeleteWordForward
                | crate::editor_core::vim::VimIntent::DeleteWordBackward
                | crate::editor_core::vim::VimIntent::DeleteWordEnd
                | crate::editor_core::vim::VimIntent::YankWordForward
                | crate::editor_core::vim::VimIntent::YankWordBackward
                | crate::editor_core::vim::VimIntent::DeleteTillChar
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
                | crate::editor_core::vim::VimIntent::DeleteAroundUnderscore => {}
                crate::editor_core::vim::VimIntent::Swallow => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::vim_intent_preserves_vertical_text_column as preserves;
    use crate::editor_core::vim::VimIntent;

    #[test]
    fn only_vertical_line_motions_preserve_the_text_column() {
        assert!(preserves(VimIntent::MoveUp));
        assert!(preserves(VimIntent::MoveDown));
        // Horizontal / jump / mutating intents must fall through to the table
        // padding guard instead of keeping the raw column.
        for intent in [
            VimIntent::MoveLeft,
            VimIntent::MoveRight,
            VimIntent::MoveToLine,
            VimIntent::MoveDocStart,
            VimIntent::MoveDocEnd,
            VimIntent::DeleteLine,
            VimIntent::PasteAfter,
            VimIntent::PasteBefore,
        ] {
            assert!(!preserves(intent), "{intent:?} should not preserve column");
        }
    }
}
