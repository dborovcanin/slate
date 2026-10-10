//! Key input front ends share: vim stepping, motions and insert-mode editing
//! over one note session.
//!
//! A front end maps its platform keys to `VimKey`, keeps an `InputState` per
//! window, and acts on the returned `HostRequest`s (clipboard, command bar,
//! search). Text changes go through `NoteSession::apply_with_upkeep`, so undo,
//! reminders, folds and calc stay consistent with every other edit.
//!
//! The terminal app still runs its own copy of this dispatch
//! (`crates/tui/src/terminal/app/vim_actions.rs`, `input_modes.rs`), with
//! view-dependent extras this module leaves to the host: fold-aware and
//! soft-wrapped vertical motion, table-cell motion, formatting-boundary
//! cursor exits, autocomplete popups, macros and autoformat after typing.
use crate::calc_upkeep::{CalcEditInputs, CalcProvider, CalcWork};
use crate::display::mapping::scalar_to_byte;
use crate::{Document, EditContext, EditOutcome, NoteSession, SessionEdit, SessionUndoOutcome};
use editor_core::buffer::primitives::{BufferCursor, PrimitiveEdit};
use editor_core::buffer::words;
use editor_core::context::ResolvedContext;
use editor_core::history::policy::{UndoGrouping, UndoSession};
use editor_core::text_rules::{self, TabRuleOptions, TextRuleOptions};
use editor_core::types::{EditOperation, EditorContextSnapshot, SelectionSnapshot};
use editor_core::vim::{self, VimAction, VimContext, VimIntent, VimKey, VimMode, VimState};
use editor_core::vim_actions::{self, buffer as vim_buffer, VimRegisterValue};
use std::time::Duration;

/// Notes at least this long run edit rules on the lines around the cursor.
const SCOPED_RULE_MIN_LINES: usize = 2048;
/// Lines on each side of the cursor that Enter and Tab rules may read.
const RULE_WINDOW: usize = 96;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputState {
    pub vim: VimState,
    /// The unnamed register: yanks and deletes land here, `p` pastes it.
    pub register: VimRegisterValue,
    /// Column kept across vertical moves through shorter lines.
    desired_col: Option<usize>,
}

impl InputState {
    pub fn mode(&self) -> VimMode {
        self.vim.mode
    }
    /// Characters typed towards an unfinished command, e.g. `2d` or `g`.
    pub fn pending_keys(&self) -> String {
        let mut keys = self.vim.count_buffer.clone();
        if let Some(pending) = self.vim.pending {
            keys.push_str(match pending {
                vim::VimPending::Delete
                | vim::VimPending::DeleteTill
                | vim::VimPending::DeleteInner
                | vim::VimPending::DeleteAround => "d",
                vim::VimPending::Yank
                | vim::VimPending::YankInner
                | vim::VimPending::YankAround => "y",
                vim::VimPending::Change
                | vim::VimPending::ChangeTill
                | vim::VimPending::ChangeInner
                | vim::VimPending::ChangeAround => "c",
                vim::VimPending::Go => "g",
                vim::VimPending::MacroRecord => "q",
                vim::VimPending::MacroPlay => "@",
            });
        }
        keys
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InputOptions {
    pub tables: bool,
    pub markdown_autoformat: bool,
    pub checklist_auto_reorder: bool,
}

impl Default for InputOptions {
    fn default() -> Self {
        let rules = TextRuleOptions::default();
        Self {
            tables: rules.table_enabled,
            markdown_autoformat: rules.markdown_autoformat,
            checklist_auto_reorder: rules.checklist_auto_reorder,
        }
    }
}

pub struct InputContext<'a> {
    pub options: InputOptions,
    /// Time since the previous edit; close edits share an undo step.
    pub since_last_edit: Duration,
    /// Keep calc current through each edit; `None` leaves it to the host.
    pub calc: Option<(CalcEditInputs, &'a dyn CalcProvider)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRequest {
    /// A yank the host mirrors to the system clipboard.
    CopyToClipboard(String),
    /// `p`/`P` with an empty register: the host reads the system clipboard,
    /// stores it in `InputState::register` and replays the action.
    PasteFromClipboard(VimAction),
    OpenCommandBar,
    OpenSearch,
    SearchNext,
    SearchPrev,
    /// Recognized but not run by this module (macros).
    Unsupported(VimIntent),
}

#[derive(Debug, Default)]
pub struct InputOutcome {
    /// The key meant something in the current mode.
    pub handled: bool,
    pub text_changed: bool,
    /// First line whose text changed; lines after it may have moved.
    pub first_changed_line: Option<usize>,
    /// Calc work the host still schedules (viewport or idle passes).
    pub calc_work: CalcWork,
    pub requests: Vec<HostRequest>,
}

impl InputOutcome {
    fn absorb(&mut self, outcome: &EditOutcome) {
        if outcome.text_changed {
            self.text_changed = true;
            let line = outcome.first_changed_line;
            self.first_changed_line = Some(self.first_changed_line.map_or(line, |l| l.min(line)));
        }
        if outcome.calc_effect.work != CalcWork::Done {
            self.calc_work = outcome.calc_effect.work;
        }
    }
}

fn yank_mirrors_to_clipboard(intent: VimIntent) -> bool {
    matches!(
        intent,
        VimIntent::YankLine
            | VimIntent::YankToLineStart
            | VimIntent::YankToLineEnd
            | VimIntent::YankWordForward
            | VimIntent::YankWordBackward
            | VimIntent::YankInsideWord
            | VimIntent::YankAroundWord
            | VimIntent::YankInsidePipe
            | VimIntent::YankAroundPipe
            | VimIntent::YankVisualSelection
    )
}

fn char_len(line: &str) -> usize {
    line.chars().count()
}

/// Byte offset of `(line, col)` in the lines joined with `\n`.
fn byte_offset(lines: &[String], line: usize, col: usize) -> usize {
    let before: usize = lines[..line.min(lines.len())]
        .iter()
        .map(|l| l.len() + 1)
        .sum();
    before + lines.get(line).map_or(0, |l| scalar_to_byte(l, col))
}

/// The text of lines `start..=end` with the cursor as an empty selection,
/// and the byte offset of `start` in the whole note.
fn scoped_snapshot(doc: &Document, start: usize, end: usize) -> (EditorContextSnapshot, usize) {
    let lines = doc.lines();
    let scope_start = byte_offset(lines, start, 0);
    let text = lines[start..=end].join("\n");
    let cursor = byte_offset(lines, doc.cursor_line, doc.cursor_col)
        .saturating_sub(scope_start)
        .min(text.len());
    let snapshot = EditorContextSnapshot {
        text,
        selection: SelectionSnapshot {
            anchor: cursor,
            head: cursor,
        },
        changed_range: None,
    };
    (snapshot, scope_start)
}

fn shift_operation(op: &EditOperation, offset: usize) -> EditOperation {
    let mut mapped = op.clone();
    for change in &mut mapped.changes {
        change.from += offset;
        change.to += offset;
    }
    if let Some(selection) = mapped.selection.as_mut() {
        selection.anchor += offset;
        if let Some(head) = selection.head.as_mut() {
            *head += offset;
        }
    }
    mapped
}

/// Lines Enter and Tab rules may read: the table around the cursor, or a
/// window of lines in large notes.
fn rule_span(doc: &Document, tables: bool) -> (usize, usize) {
    let lines = doc.lines();
    let last = lines.len().saturating_sub(1);
    let center = doc.cursor_line.min(last);
    if tables {
        if let Some(bounds) = editor_core::table::table_block_bounds(lines, center) {
            return bounds;
        }
    }
    if lines.len() < SCOPED_RULE_MIN_LINES {
        return (0, last);
    }
    (
        center.saturating_sub(RULE_WINDOW),
        (center + RULE_WINDOW).min(last),
    )
}

impl NoteSession {
    /// Handle one key in the current vim mode.
    pub fn handle_key(
        &mut self,
        doc: &mut Document,
        state: &mut InputState,
        key: VimKey,
        cx: &InputContext<'_>,
    ) -> InputOutcome {
        if doc.lines.is_empty() {
            doc.lines.push(String::new());
        }
        if state.vim.mode == VimMode::Insert {
            return self.handle_insert_key(doc, state, key, cx);
        }
        let mode_before = state.vim.mode;
        let context = VimContext {
            has_search_matches: false,
            line_count: doc.lines().len(),
            macro_recording: false,
        };
        let step = vim::step(&state.vim, key, &context);
        state.vim = step.state;
        let mut outcome = InputOutcome {
            handled: step.handled,
            ..Default::default()
        };
        for action in &step.actions {
            self.run_action(doc, state, action, mode_before, cx, &mut outcome);
        }
        if state.vim.mode != VimMode::Insert {
            clamp_normal_cursor(doc);
        }
        outcome
    }

    /// Run one vim action outside key stepping, e.g. a paste the host
    /// replays after filling the register from the system clipboard.
    pub fn apply_vim_action(
        &mut self,
        doc: &mut Document,
        state: &mut InputState,
        action: &VimAction,
        cx: &InputContext<'_>,
    ) -> InputOutcome {
        let mut outcome = InputOutcome {
            handled: true,
            ..Default::default()
        };
        let mode = state.vim.mode;
        self.run_action(doc, state, action, mode, cx, &mut outcome);
        if state.vim.mode != VimMode::Insert {
            clamp_normal_cursor(doc);
        }
        outcome
    }

    fn edit(
        &mut self,
        doc: &mut Document,
        state: &InputState,
        edit: SessionEdit<'_>,
        cx: &InputContext<'_>,
        outcome: &mut InputOutcome,
    ) -> Option<EditOutcome> {
        let session = if state.vim.mode == VimMode::Insert {
            UndoSession::Insert
        } else {
            UndoSession::Command
        };
        let ctx = EditContext {
            grouping: UndoGrouping {
                session,
                elapsed: cx.since_last_edit,
            },
            folds: None,
        };
        let applied = self.apply_with_upkeep(
            doc,
            edit,
            ctx,
            cx.calc.map(|(inputs, _)| inputs),
            cx.calc.map(|(_, provider)| provider),
        )?;
        outcome.absorb(&applied);
        Some(applied)
    }

    fn handle_insert_key(
        &mut self,
        doc: &mut Document,
        state: &mut InputState,
        key: VimKey,
        cx: &InputContext<'_>,
    ) -> InputOutcome {
        let mut outcome = InputOutcome {
            handled: true,
            ..Default::default()
        };
        let options = cx.options;
        match key {
            VimKey::Esc => {
                let step = vim::step(&state.vim, key, &VimContext::default());
                state.vim = step.state;
                state.desired_col = None;
                clamp_normal_cursor(doc);
            }
            VimKey::Char(ch) => {
                let plan = if options.tables {
                    editor_core::table::plan_table_typed_char(
                        doc.lines(),
                        doc.cursor_line,
                        doc.cursor_col,
                        ch,
                    )
                } else {
                    Default::default()
                };
                if plan.pad_before {
                    self.edit(
                        doc,
                        state,
                        SessionEdit::Primitive(PrimitiveEdit::InsertChar(' ')),
                        cx,
                        &mut outcome,
                    );
                }
                self.edit(
                    doc,
                    state,
                    SessionEdit::Primitive(PrimitiveEdit::InsertChar(ch)),
                    cx,
                    &mut outcome,
                );
            }
            VimKey::Enter => {
                let rules = TextRuleOptions {
                    markdown_autoformat: options.markdown_autoformat,
                    checklist_auto_reorder: options.checklist_auto_reorder,
                    table_enabled: options.tables,
                };
                let (start, end) = rule_span(doc, options.tables);
                let (snapshot, offset) = scoped_snapshot(doc, start, end);
                let ruled = text_rules::run_enter_rules(&ResolvedContext::new(snapshot), rules);
                match ruled {
                    Some(op) => {
                        let op = shift_operation(&op, offset);
                        self.edit(doc, state, SessionEdit::Operation(&op), cx, &mut outcome);
                    }
                    None => {
                        self.edit(
                            doc,
                            state,
                            SessionEdit::Primitive(PrimitiveEdit::Newline),
                            cx,
                            &mut outcome,
                        );
                    }
                }
            }
            VimKey::Tab => {
                let rules = TabRuleOptions {
                    markdown_autoformat: options.markdown_autoformat,
                    outdent: false,
                    table_enabled: options.tables,
                };
                let (start, end) = rule_span(doc, options.tables);
                let (snapshot, offset) = scoped_snapshot(doc, start, end);
                match text_rules::run_tab_rules(&ResolvedContext::new(snapshot), rules) {
                    Some(op) => {
                        let op = shift_operation(&op, offset);
                        self.edit(doc, state, SessionEdit::Operation(&op), cx, &mut outcome);
                    }
                    None => {
                        self.edit(
                            doc,
                            state,
                            SessionEdit::Primitive(PrimitiveEdit::InsertText("  ")),
                            cx,
                            &mut outcome,
                        );
                    }
                }
            }
            VimKey::Backspace => {
                self.edit(
                    doc,
                    state,
                    SessionEdit::Primitive(PrimitiveEdit::Backspace),
                    cx,
                    &mut outcome,
                );
            }
            VimKey::Delete => {
                self.edit(
                    doc,
                    state,
                    SessionEdit::Primitive(PrimitiveEdit::DeleteForward),
                    cx,
                    &mut outcome,
                );
            }
            VimKey::ArrowLeft | VimKey::ArrowRight | VimKey::ArrowUp | VimKey::ArrowDown => {
                let intent = match key {
                    VimKey::ArrowLeft => VimIntent::MoveLeft,
                    VimKey::ArrowRight => VimIntent::MoveRight,
                    VimKey::ArrowUp => VimIntent::MoveUp,
                    _ => VimIntent::MoveDown,
                };
                move_cursor(doc, state, intent, 1, options.tables);
            }
            VimKey::Ctrl(_) => outcome.handled = false,
        }
        if outcome.text_changed {
            state.desired_col = None;
        }
        outcome
    }

    fn run_action(
        &mut self,
        doc: &mut Document,
        state: &mut InputState,
        action: &VimAction,
        mode_before: VimMode,
        cx: &InputContext<'_>,
        outcome: &mut InputOutcome,
    ) {
        let count = action.count.max(1);
        let intent = action.intent;
        let options = cx.options;
        match intent {
            VimIntent::MoveLeft
            | VimIntent::MoveRight
            | VimIntent::MoveUp
            | VimIntent::MoveDown
            | VimIntent::MoveScreenUp
            | VimIntent::MoveScreenDown
            | VimIntent::MoveWordForward
            | VimIntent::MoveWordBackward
            | VimIntent::MoveLineStart
            | VimIntent::MoveLineEnd
            | VimIntent::MoveDocStart
            | VimIntent::MoveDocEnd
            | VimIntent::MoveToLine => move_cursor(doc, state, intent, count, options.tables),
            VimIntent::EnterInsert
            | VimIntent::AppendInsert
            | VimIntent::InsertLineStart
            | VimIntent::AppendLineEnd => {
                let len = char_len(&doc.lines()[doc.cursor_line]);
                let len = if intent == VimIntent::InsertLineStart {
                    0
                } else {
                    len
                };
                doc.cursor_col = vim_buffer::insert_entry_column(doc.cursor_col, len, intent);
                state.vim.mode = VimMode::Insert;
            }
            VimIntent::OpenLineBelow => {
                let len = char_len(&doc.lines()[doc.cursor_line]);
                doc.cursor_col = vim_buffer::insert_entry_column(doc.cursor_col, len, intent);
                state.vim.mode = VimMode::Insert;
                self.edit(
                    doc,
                    state,
                    SessionEdit::Primitive(PrimitiveEdit::Newline),
                    cx,
                    outcome,
                );
            }
            VimIntent::OpenLineAbove => {
                doc.cursor_col = 0;
                state.vim.mode = VimMode::Insert;
                let at = doc.cursor_line;
                self.edit(
                    doc,
                    state,
                    SessionEdit::InsertLines {
                        at,
                        lines: vec![String::new()].into(),
                    },
                    cx,
                    outcome,
                );
                doc.cursor_line = at;
                doc.cursor_col = 0;
            }
            VimIntent::EnterVisual | VimIntent::EnterVisualLine => {
                doc.selection_anchor = Some((doc.cursor_line, doc.cursor_col));
            }
            VimIntent::ExitVisual => doc.selection_anchor = None,
            VimIntent::YankVisualSelection | VimIntent::DeleteVisualSelection => {
                let delete = intent == VimIntent::DeleteVisualSelection;
                let linewise = mode_before == VimMode::VisualLine;
                if let Some(applied) = self.edit(
                    doc,
                    state,
                    SessionEdit::Visual { linewise, delete },
                    cx,
                    outcome,
                ) {
                    if let Some(register) = applied.register {
                        if !delete {
                            outcome
                                .requests
                                .push(HostRequest::CopyToClipboard(register.text.clone()));
                        }
                        state.register = register;
                    }
                }
                doc.selection_anchor = None;
            }
            VimIntent::Undo | VimIntent::Redo => {
                let undo_cx = crate::UndoContext {
                    normal_mode: true,
                    table_enabled: options.tables,
                };
                for _ in 0..count {
                    let calc_inputs = cx.calc.map(|(inputs, _)| inputs.base);
                    let provider = cx.calc.map(|(_, provider)| provider);
                    let restored = if intent == VimIntent::Undo {
                        self.undo_action_with_upkeep(doc, undo_cx, calc_inputs, provider, None)
                    } else {
                        self.redo_action_with_upkeep(doc, undo_cx, calc_inputs, provider, None)
                    };
                    match restored {
                        SessionUndoOutcome::Text(applied) => outcome.absorb(&applied),
                        SessionUndoOutcome::Reminder { .. } => {}
                        SessionUndoOutcome::Exhausted | SessionUndoOutcome::NotEditable => break,
                    }
                }
            }
            VimIntent::PasteAfter | VimIntent::PasteBefore if state.register.is_empty() => {
                outcome
                    .requests
                    .push(HostRequest::PasteFromClipboard(action.clone()));
            }
            VimIntent::OpenCommandBar => outcome.requests.push(HostRequest::OpenCommandBar),
            VimIntent::OpenSearch => outcome.requests.push(HostRequest::OpenSearch),
            VimIntent::SearchNext => outcome.requests.push(HostRequest::SearchNext),
            VimIntent::SearchPrev => outcome.requests.push(HostRequest::SearchPrev),
            VimIntent::StartMacroRecord | VimIntent::StopMacroRecord | VimIntent::PlayMacro => {
                outcome.requests.push(HostRequest::Unsupported(intent))
            }
            VimIntent::Swallow => {}
            _ if vim_actions::supports_intent(intent) => {
                self.run_shared_action(doc, state, action, cx, outcome);
            }
            _ => outcome.requests.push(HostRequest::Unsupported(intent)),
        }
        if !matches!(
            intent,
            VimIntent::MoveUp
                | VimIntent::MoveDown
                | VimIntent::MoveScreenUp
                | VimIntent::MoveScreenDown
        ) {
            state.desired_col = None;
        }
    }

    /// Text edits planned by `editor_core::vim_actions` from a snapshot.
    fn run_shared_action(
        &mut self,
        doc: &mut Document,
        state: &mut InputState,
        action: &VimAction,
        cx: &InputContext<'_>,
        outcome: &mut InputOutcome,
    ) {
        let count = action.count.max(1);
        let lines = doc.lines();
        let last = lines.len() - 1;
        let (start, end) =
            vim_actions::scoped_line_range(action.intent, count, lines, doc.cursor_line)
                .filter(|_| lines.len() >= SCOPED_RULE_MIN_LINES)
                .unwrap_or((0, last));
        let (snapshot, offset) = scoped_snapshot(doc, start, end);
        let register = (!state.register.is_empty()).then_some(&state.register);
        let Some(result) = vim_actions::execute_vim_action_with_target(
            &snapshot.text,
            snapshot.selection,
            action.intent,
            count,
            register,
            action.target_char,
        ) else {
            return;
        };
        let mut deleted_lines = (action.intent == VimIntent::DeleteLine)
            .then(|| (doc.cursor_line, doc.cursor_line + count - 1));
        for op in &result.operations {
            let op = shift_operation(op, offset);
            let edit = match deleted_lines.take().filter(|_| !op.changes.is_empty()) {
                Some(deleted_lines) => SessionEdit::LinewiseOperation {
                    operation: &op,
                    deleted_lines,
                },
                None => SessionEdit::Operation(&op),
            };
            self.edit(doc, state, edit, cx, outcome);
        }
        if let Some(register) = result.register.filter(|r| !r.is_empty()) {
            if yank_mirrors_to_clipboard(action.intent) {
                outcome
                    .requests
                    .push(HostRequest::CopyToClipboard(register.text.clone()));
            }
            state.register = register;
        }
        if action.intent == VimIntent::ChangeLine {
            state.vim.mode = VimMode::Insert;
        }
    }
}

/// Keep the cursor inside the note. Like the terminal app, Normal mode may
/// rest at the line's end, where word motion continues onto the next line.
fn clamp_normal_cursor(doc: &mut Document) {
    let last = doc.lines().len().saturating_sub(1);
    doc.cursor_line = doc.cursor_line.min(last);
    let len = doc.lines().get(doc.cursor_line).map_or(0, |l| char_len(l));
    doc.cursor_col = doc.cursor_col.min(len);
}

/// Cursor motion over logical lines. Hosts with folds or soft wrap map
/// vertical motion through their view before calling into the session.
fn move_cursor(
    doc: &mut Document,
    state: &mut InputState,
    intent: VimIntent,
    count: usize,
    tables: bool,
) {
    let lines = doc.lines();
    let last = lines.len().saturating_sub(1);
    let cursor = doc.cursor();
    let len = |line: usize| lines.get(line).map_or(0, |l| char_len(l));
    let vertical = |state: &mut InputState, line: usize| {
        let want = *state.desired_col.get_or_insert(cursor.column);
        BufferCursor {
            line,
            column: want.min(len(line)),
        }
    };
    let after = match intent {
        VimIntent::MoveLeft | VimIntent::MoveRight => {
            let mut c = cursor;
            for _ in 0..count {
                c = vim_buffer::horizontal_motion(lines, c, intent == VimIntent::MoveRight, None);
            }
            c
        }
        VimIntent::MoveUp | VimIntent::MoveScreenUp => {
            vertical(state, cursor.line.saturating_sub(count))
        }
        VimIntent::MoveDown | VimIntent::MoveScreenDown => {
            vertical(state, (cursor.line + count).min(last))
        }
        VimIntent::MoveWordForward | VimIntent::MoveWordBackward => {
            let mut c = cursor;
            for _ in 0..count {
                c = if intent == VimIntent::MoveWordForward {
                    words::move_cursor_right_word(lines, c, tables, c.line + 1..lines.len())
                } else {
                    words::move_cursor_left_word(lines, c, tables, (0..c.line).rev())
                };
            }
            c
        }
        VimIntent::MoveLineStart | VimIntent::MoveLineEnd => BufferCursor {
            line: cursor.line,
            column: vim_buffer::insert_entry_column(cursor.column, len(cursor.line), intent),
        },
        VimIntent::MoveDocStart => BufferCursor { line: 0, column: 0 },
        VimIntent::MoveDocEnd => BufferCursor {
            line: last,
            column: 0,
        },
        VimIntent::MoveToLine => BufferCursor {
            line: (count - 1).min(last),
            column: 0,
        },
        _ => cursor,
    };
    doc.set_cursor(after);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(doc: &Document) -> NoteSession {
        NoteSession::new(
            crate::lifecycle::build_history_for_note(doc.lines(), 0, 0, Default::default()),
            Default::default(),
            Default::default(),
        )
    }

    struct Editor {
        doc: Document,
        session: NoteSession,
        state: InputState,
        requests: Vec<HostRequest>,
    }

    impl Editor {
        fn new(text: &str) -> Self {
            let doc = Document::from_text(text);
            let session = session(&doc);
            Self {
                doc,
                session,
                state: InputState::default(),
                requests: Vec::new(),
            }
        }
        fn keys(&mut self, keys: &str) -> &mut Self {
            let cx = InputContext {
                options: InputOptions::default(),
                since_last_edit: Duration::from_secs(5),
                calc: None,
            };
            let mut chars = keys.chars().peekable();
            while let Some(ch) = chars.next() {
                let key = match ch {
                    '⎋' => VimKey::Esc,
                    '⏎' => VimKey::Enter,
                    '⌫' => VimKey::Backspace,
                    '⇥' => VimKey::Tab,
                    '^' if chars.peek().is_some_and(|c| c.is_ascii_lowercase()) => {
                        VimKey::Ctrl(chars.next().unwrap())
                    }
                    ch => VimKey::Char(ch),
                };
                let outcome = self
                    .session
                    .handle_key(&mut self.doc, &mut self.state, key, &cx);
                self.requests.extend(outcome.requests);
            }
            self
        }
        fn text(&self) -> String {
            self.doc.lines().join("\n")
        }
        fn cursor(&self) -> (usize, usize) {
            (self.doc.cursor_line, self.doc.cursor_col)
        }
    }

    #[test]
    fn insert_typing_and_escape() {
        let mut e = Editor::new("world");
        e.keys("iHello, ⎋");
        assert_eq!(e.text(), "Hello, world");
        assert_eq!(e.state.mode(), VimMode::Normal);
        assert_eq!(e.cursor(), (0, 7));
    }

    #[test]
    fn motions_keep_the_desired_column_through_short_lines() {
        let mut e = Editor::new("abcdef\nab\nabcdef");
        e.keys("$");
        assert_eq!(e.cursor(), (0, 5));
        e.keys("j");
        assert_eq!(e.cursor(), (1, 2));
        e.keys("j");
        assert_eq!(e.cursor(), (2, 5));
        e.keys("gg");
        assert_eq!(e.cursor(), (0, 0));
        e.keys("G$0");
        assert_eq!(e.cursor(), (2, 0));
    }

    #[test]
    fn word_motion_crosses_lines() {
        let mut e = Editor::new("one two\nthree");
        e.keys("w");
        assert_eq!(e.cursor(), (0, 4));
        // The last word of a line stops at its end before the next line.
        e.keys("ww");
        assert_eq!(e.cursor(), (1, 0));
        // At a line start, `b` first goes to the end of the line above.
        e.keys("b");
        assert_eq!(e.cursor(), (0, 7));
        e.keys("b");
        assert_eq!(e.cursor(), (0, 4));
    }

    #[test]
    fn delete_line_fills_the_register_and_pastes_back() {
        let mut e = Editor::new("a\nb\nc");
        e.keys("dd");
        assert_eq!(e.text(), "b\nc");
        e.keys("p");
        assert_eq!(e.text(), "b\na\nc");
    }

    #[test]
    fn yank_is_mirrored_to_the_clipboard() {
        let mut e = Editor::new("keep me");
        e.keys("yy");
        assert_eq!(
            e.requests,
            vec![HostRequest::CopyToClipboard("keep me".into())]
        );
    }

    #[test]
    fn paste_with_an_empty_register_asks_the_host() {
        let mut e = Editor::new("x");
        e.keys("p");
        assert!(matches!(
            e.requests.as_slice(),
            [HostRequest::PasteFromClipboard(VimAction {
                intent: VimIntent::PasteAfter,
                ..
            })]
        ));
    }

    #[test]
    fn enter_continues_a_list() {
        let mut e = Editor::new("- milk");
        e.keys("A⏎eggs⎋");
        assert_eq!(e.text(), "- milk\n- eggs");
    }

    #[test]
    fn undo_and_redo_restore_text() {
        let mut e = Editor::new("abc");
        e.keys("x");
        assert_eq!(e.text(), "bc");
        e.keys("u");
        assert_eq!(e.text(), "abc");
        e.keys("^r");
        assert_eq!(e.text(), "bc");
    }

    #[test]
    fn open_line_above_and_below() {
        let mut e = Editor::new("mid");
        e.keys("Otop⎋joend⎋");
        assert_eq!(e.text(), "top\nmid\nend");
    }

    #[test]
    fn visual_line_delete_takes_whole_lines() {
        let mut e = Editor::new("1\n2\n3");
        e.keys("Vjd");
        assert_eq!(e.text(), "3");
        assert_eq!(e.state.mode(), VimMode::Normal);
        assert_eq!(e.doc.selection_anchor, None);
    }

    #[test]
    fn pending_keys_show_counts_and_operators() {
        let mut e = Editor::new("a");
        e.keys("2d");
        assert_eq!(e.state.pending_keys(), "d");
        e.keys("⎋g");
        assert_eq!(e.state.pending_keys(), "g");
    }
}
