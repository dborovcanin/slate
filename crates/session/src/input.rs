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
use editor_core::table::{TableCursorMotionDirection, TableTypingCursor};
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
    /// The register a macro is being recorded into (`q{r}` ... `q`).
    macro_recording: Option<char>,
    macros: rustc_hash::FxHashMap<char, Vec<MacroStep>>,
    /// A macro is replaying: its steps are not recorded again.
    replaying: bool,
}

/// One recorded step of a macro: a Normal-mode action, or a key typed in
/// Insert mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroStep {
    Action(VimAction),
    InsertKey(VimKey),
}

/// Most steps one `@{r}` may run.
const MACRO_STEP_BUDGET: usize = 10_000;

fn recordable_intent(intent: VimIntent) -> bool {
    !matches!(
        intent,
        VimIntent::StartMacroRecord
            | VimIntent::StopMacroRecord
            | VimIntent::PlayMacro
            | VimIntent::OpenCommandBar
            | VimIntent::OpenSearch
    )
}

impl InputState {
    /// The register being recorded, if any.
    pub fn macro_recording(&self) -> Option<char> {
        self.macro_recording
    }
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
    /// `[[` was typed: `]]` is already added after the cursor; open the
    /// note picker for the link.
    OpenWikiCompletion,
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
    /// A message for the status line (macro recording and replay).
    pub notice: Option<String>,
}

impl InputOutcome {
    fn merge(&mut self, other: InputOutcome) {
        self.handled |= other.handled;
        if other.text_changed {
            self.text_changed = true;
            self.first_changed_line = match (self.first_changed_line, other.first_changed_line) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
        if other.calc_work != CalcWork::Done {
            self.calc_work = other.calc_work;
        }
        self.requests.extend(other.requests);
    }

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

/// Like [`scoped_snapshot`], marking the character before the cursor as
/// the change the doc-change rules react to.
fn scoped_snapshot_changed(
    doc: &Document,
    start: usize,
    end: usize,
) -> (EditorContextSnapshot, usize) {
    let (mut snapshot, offset) = scoped_snapshot(doc, start, end);
    let cursor = snapshot.selection.head;
    snapshot.changed_range = Some(editor_core::types::TextRange {
        from: cursor.saturating_sub(1),
        to: cursor,
    });
    (snapshot, offset)
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

/// Might an edit on `line` change what the autoformat rules would do?
fn line_may_trigger_rules(line: &str) -> bool {
    let trimmed = line.trim_start();
    let list = trimmed.starts_with(['-', '*', '+'])
        || trimmed.starts_with("->")
        || trimmed.chars().next().is_some_and(|c| c.is_ascii_digit());
    let table = trimmed.starts_with('|') && line.trim_end().ends_with('|');
    list || table
}

/// Move one step through table cells; `false` when the cursor is not in a
/// table or the plan does not apply. Rows and columns follow the core's
/// planner, so the cursor skips cell padding and crosses cells like the
/// terminal app does.
fn table_cursor_motion(doc: &mut Document, direction: TableCursorMotionDirection) -> bool {
    use editor_core::table::{plan_table_cursor_motion, table_block_bounds};
    let lines = doc.lines();
    let Some((block_start, block_end)) = table_block_bounds(lines, doc.cursor_line) else {
        return false;
    };
    let Some(target) = plan_table_cursor_motion(
        &lines[block_start..=block_end],
        doc.cursor_line - block_start,
        doc.cursor_col,
        direction,
    ) else {
        return false;
    };
    let vertical = matches!(
        direction,
        TableCursorMotionDirection::Up | TableCursorMotionDirection::Down
    );
    let (line, col) = if target.line_index < 0 {
        match block_start.checked_sub(1) {
            Some(prev) => (prev, if vertical { doc.cursor_col } else { target.col }),
            None => return true,
        }
    } else if target.line_index as usize > block_end - block_start {
        if block_end + 1 >= lines.len() {
            return true;
        }
        (
            block_end + 1,
            if vertical { doc.cursor_col } else { target.col },
        )
    } else {
        (block_start + target.line_index as usize, target.col)
    };
    let len = lines.get(line).map_or(0, |l| char_len(l));
    doc.cursor_line = line;
    doc.cursor_col = col.min(len);
    true
}

/// Keep the cursor in the content of a table cell: right padding exists for
/// alignment only (`clamp_right`), and the left padding is skipped.
fn clamp_table_cursor(doc: &mut Document, tables: bool, clamp_right: bool) {
    use crate::display::table::{
        table_cell_edit_start, table_cell_info_at_char, table_cell_is_empty,
        table_cell_navigation_anchor,
    };
    if !tables {
        return;
    }
    let line = doc.cursor_line;
    let Some(cell) = table_cell_info_at_char(doc.lines(), line, doc.cursor_col) else {
        return;
    };
    let text = &doc.lines()[line];
    let anchor = table_cell_navigation_anchor(text, &cell);
    let anchor = if table_cell_is_empty(&cell)
        || doc.cursor_col < char_len_of(text, table_cell_edit_start(&cell))
        || (clamp_right && doc.cursor_col > anchor)
    {
        Some(anchor)
    } else {
        None
    };
    if let Some(anchor) = anchor {
        doc.cursor_col = anchor;
    }
}

fn char_len_of(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].chars().count()
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
        // Same boundary as the terminal: a command starts its own undo step
        // instead of merging into the typing before it.
        self.begin_input(if state.vim.mode == VimMode::Insert {
            UndoSession::Insert
        } else {
            UndoSession::Command
        });
        if state.vim.mode == VimMode::Insert {
            if let (Some(register), false) = (state.macro_recording, state.replaying) {
                if !matches!(key, VimKey::Ctrl(_)) {
                    state
                        .macros
                        .entry(register)
                        .or_default()
                        .push(MacroStep::InsertKey(key));
                }
            }
            return self.handle_insert_key(doc, state, key, cx);
        }
        let mode_before = state.vim.mode;
        let context = VimContext {
            has_search_matches: false,
            line_count: doc.lines().len(),
            macro_recording: state.macro_recording.is_some(),
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

    /// Replace characters `range` of `line` with `text` and put the cursor
    /// at `cursor_col` (autocomplete picks).
    pub fn replace_line_chars(
        &mut self,
        doc: &mut Document,
        state: &InputState,
        line: usize,
        range: std::ops::Range<usize>,
        text: &str,
        cursor_col: usize,
        cx: &InputContext<'_>,
    ) -> InputOutcome {
        let mut outcome = InputOutcome {
            handled: true,
            ..Default::default()
        };
        let cursor_after = BufferCursor {
            line,
            column: cursor_col,
        };
        self.edit(
            doc,
            state,
            SessionEdit::LineReplace {
                line,
                range,
                text,
                preserve_cursor: line != doc.cursor_line,
                cursor_after: Some(cursor_after),
            },
            cx,
            &mut outcome,
        );
        doc.cursor_col = cursor_col;
        outcome
    }

    /// After typing: reformat tables and lists around the cursor.
    fn autoformat(
        &mut self,
        doc: &mut Document,
        state: &InputState,
        cx: &InputContext<'_>,
        outcome: &mut InputOutcome,
    ) {
        let options = cx.options;
        if !(options.tables || options.markdown_autoformat)
            || !line_may_trigger_rules(&doc.lines()[doc.cursor_line])
        {
            return;
        }
        let rules = TextRuleOptions {
            markdown_autoformat: options.markdown_autoformat,
            checklist_auto_reorder: options.checklist_auto_reorder,
            table_enabled: options.tables,
        };
        let (start, end) = rule_span(doc, options.tables);
        let (snapshot, offset) = scoped_snapshot_changed(doc, start, end);
        if let Some(op) = text_rules::run_doc_change_rules_with_table_cache(
            &ResolvedContext::new(snapshot),
            rules,
            &mut editor_core::table::TableFormatCache::default(),
        ) {
            let op = shift_operation(&op, offset);
            self.edit(doc, state, SessionEdit::Operation(&op), cx, outcome);
        }
    }

    /// `Backspace`/`Delete` in a table: the header's column goes with its
    /// last character, cells merge at their edges, and pipes stay put.
    /// `false` when no table rule applies and the plain delete should run.
    fn table_delete(
        &mut self,
        doc: &mut Document,
        state: &InputState,
        backward: bool,
        cx: &InputContext<'_>,
        outcome: &mut InputOutcome,
    ) -> bool {
        use editor_core::text_rules::{
            run_table_boundary_edit_rules, run_table_header_delete_column_rule_with_table_cache,
            TableBoundaryEditOptions,
        };
        if !cx.options.tables || !editor_core::table::is_table_line(&doc.lines()[doc.cursor_line]) {
            return false;
        }
        let (start, end) = rule_span(doc, true);
        let (snapshot, offset) = scoped_snapshot(doc, start, end);
        let ctx = ResolvedContext::new(snapshot);
        let op = run_table_header_delete_column_rule_with_table_cache(
            &ctx,
            &mut editor_core::table::TableFormatCache::default(),
        )
        .or_else(|| {
            run_table_boundary_edit_rules(
                &ctx,
                TableBoundaryEditOptions {
                    markdown_autoformat: cx.options.markdown_autoformat,
                    backward,
                    structural_merge: true,
                    table_enabled: true,
                },
            )
        });
        let Some(op) = op else {
            return false;
        };
        if !op.changes.is_empty() {
            let op = shift_operation(&op, offset);
            self.edit(doc, state, SessionEdit::Operation(&op), cx, outcome);
        }
        true
    }

    /// `Shift+Tab` in Insert mode: the previous table cell, or outdent the
    /// list item. Does nothing when neither applies.
    pub fn back_tab(
        &mut self,
        doc: &mut Document,
        state: &mut InputState,
        cx: &InputContext<'_>,
    ) -> InputOutcome {
        self.begin_input(UndoSession::Insert);
        let mut outcome = InputOutcome {
            handled: true,
            ..Default::default()
        };
        let options = cx.options;
        let rules = TabRuleOptions {
            markdown_autoformat: options.markdown_autoformat,
            outdent: true,
            table_enabled: options.tables,
        };
        let (start, end) = rule_span(doc, options.tables);
        let (snapshot, offset) = scoped_snapshot(doc, start, end);
        if let Some(op) = text_rules::run_tab_rules(&ResolvedContext::new(snapshot), rules) {
            let op = shift_operation(&op, offset);
            self.edit(doc, state, SessionEdit::Operation(&op), cx, &mut outcome);
            self.autoformat(doc, state, cx, &mut outcome);
            clamp_table_cursor(doc, options.tables, true);
        }
        outcome
    }

    /// Move the cursor to the previous or next word start, across lines;
    /// table rows move by cell content.
    pub fn move_word(&self, doc: &mut Document, forward: bool, tables: bool) {
        let cursor = doc.cursor();
        let lines = doc.lines();
        let after = if forward {
            words::move_cursor_right_word(lines, cursor, tables, cursor.line + 1..lines.len())
        } else {
            words::move_cursor_left_word(lines, cursor, tables, (0..cursor.line).rev())
        };
        doc.set_cursor(after);
    }

    /// Delete the word before the cursor (`Ctrl+Backspace`); at the start of
    /// a line this joins it to the previous one.
    pub fn delete_word_backward(
        &mut self,
        doc: &mut Document,
        state: &mut InputState,
        cx: &InputContext<'_>,
    ) -> InputOutcome {
        self.begin_input(UndoSession::Insert);
        let mut outcome = InputOutcome {
            handled: true,
            ..Default::default()
        };
        let edit = if doc.cursor_col == 0 {
            SessionEdit::Primitive(PrimitiveEdit::Backspace)
        } else {
            SessionEdit::BackwardWordDelete {
                tables: cx.options.tables,
            }
        };
        self.edit(doc, state, edit, cx, &mut outcome);
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
                if plan.autoformat {
                    self.autoformat(doc, state, cx, &mut outcome);
                }
                match plan.cursor {
                    TableTypingCursor::InCellContent => {
                        clamp_table_cursor(doc, options.tables, true)
                    }
                    TableTypingCursor::InCellPadding => {
                        clamp_table_cursor(doc, options.tables, false)
                    }
                    TableTypingCursor::PastRowEnd => {}
                }
                // `[[` closes itself and asks for the link target.
                if ch == '['
                    && doc.cursor_col >= 2
                    && doc.lines()[doc.cursor_line].chars().nth(doc.cursor_col - 2) == Some('[')
                {
                    self.edit(
                        doc,
                        state,
                        SessionEdit::Primitive(PrimitiveEdit::InsertText("]]")),
                        cx,
                        &mut outcome,
                    );
                    doc.cursor_col -= 2;
                    outcome.requests.push(HostRequest::OpenWikiCompletion);
                }
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
                self.autoformat(doc, state, cx, &mut outcome);
                clamp_table_cursor(doc, options.tables, true);
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
                self.autoformat(doc, state, cx, &mut outcome);
                clamp_table_cursor(doc, options.tables, true);
            }
            VimKey::Backspace | VimKey::Delete => {
                let backward = key == VimKey::Backspace;
                if !self.table_delete(doc, state, backward, cx, &mut outcome) {
                    self.edit(
                        doc,
                        state,
                        SessionEdit::Primitive(if backward {
                            PrimitiveEdit::Backspace
                        } else {
                            PrimitiveEdit::DeleteForward
                        }),
                        cx,
                        &mut outcome,
                    );
                }
                self.autoformat(doc, state, cx, &mut outcome);
                clamp_table_cursor(doc, options.tables, true);
            }
            VimKey::ArrowLeft | VimKey::ArrowRight | VimKey::ArrowUp | VimKey::ArrowDown => {
                let intent = match key {
                    VimKey::ArrowLeft => VimIntent::MoveLeft,
                    VimKey::ArrowRight => VimIntent::MoveRight,
                    VimKey::ArrowUp => VimIntent::MoveUp,
                    _ => VimIntent::MoveDown,
                };
                move_cursor(doc, state, intent, 1, options.tables);
                clamp_table_cursor(doc, options.tables, true);
            }
            VimKey::Ctrl(_) => outcome.handled = false,
        }
        if outcome.text_changed {
            state.desired_col = None;
        }
        outcome
    }

    /// `@{r}`: run the register's steps `count` times.
    fn replay_macro(
        &mut self,
        doc: &mut Document,
        state: &mut InputState,
        action: &VimAction,
        cx: &InputContext<'_>,
        outcome: &mut InputOutcome,
    ) {
        let count = action.count.max(1);
        let Some(register) = action.target_char.map(|ch| ch.to_ascii_lowercase()) else {
            outcome.notice = Some("macro register required".to_string());
            return;
        };
        let steps = state.macros.get(&register).cloned().unwrap_or_default();
        if steps.is_empty() {
            outcome.notice = Some(format!("macro @{register} is empty"));
            return;
        }
        if count
            .checked_mul(steps.len())
            .is_none_or(|n| n > MACRO_STEP_BUDGET)
        {
            outcome.notice = Some(format!(
                "macro @{register} replay aborted: step budget exceeded (>{MACRO_STEP_BUDGET})"
            ));
            return;
        }
        state.replaying = true;
        for _ in 0..count {
            for step in &steps {
                match step {
                    MacroStep::Action(a) => {
                        let mode = state.vim.mode;
                        self.run_action(doc, state, a, mode, cx, outcome);
                    }
                    MacroStep::InsertKey(key) => {
                        if state.vim.mode == VimMode::Insert {
                            let step_outcome = self.handle_insert_key(doc, state, *key, cx);
                            outcome.merge(step_outcome);
                        }
                    }
                }
            }
        }
        state.replaying = false;
        outcome.notice = Some(format!("replayed @{register} x{count}"));
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
        if let (Some(register), false) = (state.macro_recording, state.replaying) {
            if recordable_intent(intent) {
                state
                    .macros
                    .entry(register)
                    .or_default()
                    .push(MacroStep::Action(action.clone()));
            }
        }
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
            VimIntent::StartMacroRecord => {
                outcome.notice = Some(match action.target_char {
                    Some(ch) => {
                        let register = ch.to_ascii_lowercase();
                        state.macro_recording = Some(register);
                        state.macros.insert(register, Vec::new());
                        format!("recording @{register}")
                    }
                    None => "macro register required".to_string(),
                });
            }
            VimIntent::StopMacroRecord => {
                outcome.notice = Some(match state.macro_recording.take() {
                    Some(register) => {
                        let steps = state.macros.get(&register).map_or(0, Vec::len);
                        format!("recorded @{register} ({steps} steps)")
                    }
                    None => "no active macro recording".to_string(),
                });
            }
            VimIntent::PlayMacro => {
                if !state.replaying {
                    self.replay_macro(doc, state, action, cx, outcome);
                }
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
    if tables {
        let direction = match intent {
            VimIntent::MoveLeft => Some(TableCursorMotionDirection::Left),
            VimIntent::MoveRight => Some(TableCursorMotionDirection::Right),
            VimIntent::MoveUp => Some(TableCursorMotionDirection::Up),
            VimIntent::MoveDown => Some(TableCursorMotionDirection::Down),
            _ => None,
        };
        if let Some(direction) = direction {
            if table_cursor_motion(doc, direction) {
                for _ in 1..count {
                    table_cursor_motion(doc, direction);
                }
                return;
            }
        }
    }
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
        since_last_edit: Duration,
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
                since_last_edit: Duration::from_secs(5),
            }
        }
        fn keys(&mut self, keys: &str) -> &mut Self {
            let cx = InputContext {
                options: InputOptions::default(),
                since_last_edit: self.since_last_edit,
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
    fn word_motion_and_deletion_cross_lines() {
        let mut e = Editor::new("one two\nthree");
        e.doc.cursor_line = 1;
        e.doc.cursor_col = 5;
        e.session.move_word(&mut e.doc, false, false);
        assert_eq!(e.cursor(), (1, 0));
        e.session.move_word(&mut e.doc, false, false);
        assert_eq!(e.cursor(), (0, 7));
        e.session.move_word(&mut e.doc, false, false);
        assert_eq!(e.cursor(), (0, 4));
        e.session.move_word(&mut e.doc, true, false);
        assert_eq!(e.cursor(), (0, 7));
        e.doc.cursor_line = 0;
        e.doc.cursor_col = 7;
        let cx = InputContext {
            options: InputOptions::default(),
            since_last_edit: Duration::from_secs(5),
            calc: None,
        };
        e.state.vim.mode = VimMode::Insert;
        e.session
            .delete_word_backward(&mut e.doc, &mut e.state, &cx);
        assert_eq!(e.text(), "one \nthree");
    }

    #[test]
    fn double_bracket_closes_and_requests_the_link_picker() {
        let mut e = Editor::new("see ");
        e.doc.cursor_col = 4;
        e.keys("i[[");
        assert_eq!(e.text(), "see [[]]");
        assert_eq!(e.cursor(), (0, 6));
        assert!(e
            .requests
            .iter()
            .any(|r| matches!(r, HostRequest::OpenWikiCompletion)));
    }

    #[test]
    fn replace_line_chars_swaps_text_and_moves_the_cursor() {
        let mut e = Editor::new("sal + 1");
        e.state.vim.mode = VimMode::Insert;
        e.doc.cursor_col = 3;
        let cx = InputContext {
            options: InputOptions::default(),
            since_last_edit: Duration::from_secs(5),
            calc: None,
        };
        e.session
            .replace_line_chars(&mut e.doc, &e.state, 0, 0..3, "salary", 6, &cx);
        assert_eq!(e.text(), "salary + 1");
        assert_eq!(e.cursor(), (0, 6));
    }

    #[test]
    fn macros_record_and_replay_commands_and_typing() {
        let mut e = Editor::new("a\nb\nc\nd");
        // Record: append "!" to the line and move down.
        e.keys("qaA!⎋jq");
        assert_eq!(e.text(), "a!\nb\nc\nd");
        assert!(e.state.macro_recording().is_none());
        // Replay twice.
        e.keys("2@a");
        assert_eq!(e.text(), "a!\nb!\nc!\nd");
        // An unknown register does nothing.
        e.keys("@z");
        assert_eq!(e.text(), "a!\nb!\nc!\nd");
    }

    #[test]
    fn back_tab_goes_to_the_previous_cell_and_outdents_lists() {
        let cx = InputContext {
            options: InputOptions::default(),
            since_last_edit: Duration::from_secs(5),
            calc: None,
        };
        let mut e = Editor::new("| A | B |\n| --- | --- |\n| 1 | 2 |");
        e.state.vim.mode = VimMode::Insert;
        e.doc.cursor_line = 2;
        e.doc.cursor_col = 8;
        e.session.back_tab(&mut e.doc, &mut e.state, &cx);
        assert_eq!(e.cursor().0, 2);
        assert!(e.cursor().1 < 6, "moved back into the first cell: {:?}", e.cursor());
        let mut e = Editor::new("- a\n  - b");
        e.state.vim.mode = VimMode::Insert;
        e.doc.cursor_line = 1;
        e.doc.cursor_col = 7;
        e.session.back_tab(&mut e.doc, &mut e.state, &cx);
        assert_eq!(e.text(), "- a\n- b");
    }

    #[test]
    fn a_command_starts_its_own_undo_step_even_when_typed_quickly() {
        let mut e = Editor::new("abc\ndef");
        e.since_last_edit = Duration::ZERO;
        e.keys("iX⎋dd");
        assert_eq!(e.text(), "def");
        e.keys("u");
        assert_eq!(e.text(), "Xabc\ndef");
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
