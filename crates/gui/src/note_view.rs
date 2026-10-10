//! The open note and the lines the window paints, without any GPUI types.
//!
//! Everything here comes from the shared crates: `NoteSession` owns the note
//! and its edits, keys go through `note_session::input`, calc runs through the
//! session's `NoteCalcProvider`, and line styling comes from
//! `note_session::display`. This module only groups that output into runs,
//! table cells and cursor/selection positions the window can lay out.
use app_core::cross_note::CrossNoteVarIndex;
use app_core::storage::{Db, Note, NoteModules, NoteSummary};
use editor_core::calc_plan::CalcFeatureMask;
use editor_core::command_catalog::CommandId;
use editor_core::history::policy::{UndoGrouping, UndoSession};
use editor_core::markdown_tokens::{self, FenceState};
use editor_core::types::{
    CommandMode, EditOperation, EditorContextSnapshot, SelectionSnapshot, TextRange,
};
use editor_core::vim::{VimAction, VimKey, VimMode};
use editor_core::vim_actions::VimRegisterValue;
use note_session::calc::CalcInputs;
use note_session::calc_provider::{
    CrossNoteSource, NoteCalcProvider, CALC_VIEWPORT_ONLY_MIN_LINES, LARGE_DOC_CALC_DEFER_LINES,
};
use note_session::calc_reset::CALC_ASYNC_MIN_LINES;
use note_session::calc_upkeep::{CalcEditInputs, CalcWork};
use note_session::display::mapping::Affinity;
use note_session::display::semantic::{
    LineDecorations, SemanticContext, SemanticLine, SemanticStyle,
};
use note_session::display::table::{format_formula_display_value, is_markdown_table_line};
use note_session::input::{InputContext, InputOptions, InputOutcome, InputState};
use note_session::save::{run_save, SaveCompletion, SaveContext};
use note_session::{Document, NoteSession};
use note_session::{EditContext, SessionEdit};
use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

/// Lines evaluated up front for notes that only evaluate what is visible.
const FIRST_SCREEN_LINES: usize = 200;

/// A stretch of one line with a single style.
#[derive(Clone, PartialEq)]
pub struct Run {
    pub text: String,
    pub style: SemanticStyle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub text: String,
    /// The cell holds a formula and `text` is its computed value.
    pub formula: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LineKind {
    Text,
    /// Markdown heading, level 1–6.
    Heading(usize),
    /// Checklist item; runs start after the `- [ ]` marker.
    Checklist {
        checked: bool,
    },
    TableRow {
        cells: Vec<TableCell>,
        header: bool,
    },
    /// The `| --- |` row under a table header; drawn as the header rule.
    TableDelimiter,
}

#[derive(Clone, PartialEq)]
pub struct LineView {
    pub index: usize,
    pub kind: LineKind,
    pub runs: Vec<Run>,
    /// Calc result shown after the text, with its ` = ` or ` → ` prefix.
    pub ghost: Option<String>,
    /// Cursor position in the runs' characters, on the cursor line.
    pub cursor: Option<usize>,
    /// Selected characters of the runs in Visual mode.
    pub selection: Option<Range<usize>>,
    /// The whole line is selected (Visual Line mode, or a selected table row).
    pub line_selected: bool,
}

/// What running a command-bar command did.
#[derive(Debug)]
pub enum CommandRun {
    Done {
        message: String,
        clipboard: Option<String>,
        quit: bool,
    },
    /// A command the window runs (browse, history, today, modules...).
    Host { id: Option<CommandId>, raw: String },
}

pub struct NoteHost {
    pub(crate) db: Db,
    index: Arc<Mutex<CrossNoteVarIndex>>,
    loaded: Condvar,
    pub doc: Document,
    pub session: NoteSession,
    pub input: InputState,
    pub title: String,
    pub modules: NoteModules,
    pub notes: Vec<NoteSummary>,
    last_edit: Option<Instant>,
}

/// The provider the terminal also passes to calc: modules from the note,
/// values of other notes from the shared index.
fn provider<'a>(
    modules: NoteModules,
    note_id: &'a str,
    index: &'a Arc<Mutex<CrossNoteVarIndex>>,
    db: &'a Db,
    loaded: &'a Condvar,
) -> NoteCalcProvider<'a> {
    NoteCalcProvider {
        base: CalcInputs {
            mask: CalcFeatureMask {
                math_enabled: modules.math,
                table_enabled: modules.table,
                variables_enabled: modules.variables,
            },
            math_enabled: modules.math,
            viewport_only: false,
        },
        cross_note_enabled: modules.cross_note,
        table_enabled: modules.table,
        cross_note: CrossNoteSource {
            note_id,
            index,
            db,
            loaded,
        },
        selection_range: None,
    }
}

impl NoteHost {
    /// Open `note_id`, or the most recently edited note when none is given.
    pub fn open(db: Db, note_id: Option<&str>) -> Result<Self, String> {
        let notes = db.list_notes_meta()?;
        let id = match note_id {
            Some(id) => id.to_string(),
            None => notes
                .first()
                .map(|n| n.id.clone())
                .ok_or_else(|| "no notes yet; create one with `slate --new`".to_string())?,
        };
        let doc = Document::default();
        let history =
            note_session::lifecycle::build_history_for_note(doc.lines(), 0, 0, Default::default());
        let session = NoteSession::new(history, Default::default(), Default::default());
        let mut host = Self {
            db,
            index: Arc::new(Mutex::new(Default::default())),
            loaded: Condvar::new(),
            doc,
            session,
            input: InputState::default(),
            title: String::new(),
            modules: NoteModules::default(),
            notes,
            last_edit: None,
        };
        host.switch_to(&id)?;
        Ok(host)
    }

    pub fn note_id(&self) -> &str {
        self.session.note_id()
    }

    /// Save the open note first; a failed save keeps it open.
    pub fn switch_to(&mut self, id: &str) -> Result<(), String> {
        if !self.session.note_id().is_empty() {
            self.save()?;
        }
        let note: Note = self
            .db
            .get_note(id)?
            .ok_or_else(|| format!("note {id} not found"))?;
        self.open_note(note);
        Ok(())
    }

    /// Show `note` (already read, or just unlocked) as the open note.
    pub fn open_note(&mut self, note: Note) {
        self.session.open(&note, &mut self.doc);
        if let Ok(reminders) = self.db.list_reminders(&note.id) {
            self.session
                .install_reminders(reminder_ghosts(&reminders, self.doc.lines()));
        }
        self.input = InputState::default();
        self.modules = note.modules;
        self.title = self
            .notes
            .iter()
            .find(|n| n.id == note.id)
            .map(|n| n.title.clone())
            .unwrap_or_default();
        self.last_edit = None;
        self.recompute_calc();
    }

    /// The note is encrypted and its text is not available until unlocked.
    pub fn locked(&self) -> bool {
        !self.session.editable()
    }

    pub fn apply_vim_action(&mut self, action: &VimAction) -> InputOutcome {
        self.run_input(|session, doc, input, cx| session.apply_vim_action(doc, input, action, cx))
    }

    /// Re-read the note list, e.g. after creating or deleting notes.
    pub fn refresh_notes(&mut self) {
        self.refresh_titles();
    }

    /// Evaluate calc the way the terminal does after opening a note. Large
    /// viewport-only notes get their first screen evaluated.
    pub fn recompute_calc(&mut self) {
        let note_id = self.session.note_id().to_string();
        let provider = provider(self.modules, &note_id, &self.index, &self.db, &self.loaded);
        let reset = self.session.reset_calc_after_open(&mut self.doc, &provider);
        if reset.refresh_viewport {
            let to = self.doc.lines().len().min(FIRST_SCREEN_LINES);
            self.session
                .evaluate_calc_range(&self.doc, 0, to, &provider);
        }
    }

    /// One key through the shared input pipeline; calc stays current.
    pub fn handle_key(&mut self, key: VimKey) -> InputOutcome {
        self.run_input(|session, doc, input, cx| session.handle_key(doc, input, key, cx))
    }

    /// Paste `text` from the system clipboard for an action the session
    /// returned because its register was empty.
    pub fn paste_clipboard(&mut self, text: String, action: &VimAction) -> InputOutcome {
        self.input.register = if text.ends_with('\n') {
            VimRegisterValue::linewise(text)
        } else {
            VimRegisterValue::charwise(text)
        };
        self.run_input(|session, doc, input, cx| session.apply_vim_action(doc, input, action, cx))
    }

    fn run_input(
        &mut self,
        f: impl FnOnce(
            &mut NoteSession,
            &mut Document,
            &mut InputState,
            &InputContext<'_>,
        ) -> InputOutcome,
    ) -> InputOutcome {
        let note_id = self.session.note_id().to_string();
        let provider = provider(self.modules, &note_id, &self.index, &self.db, &self.loaded);
        let cx = InputContext {
            options: InputOptions {
                tables: self.modules.table,
                ..Default::default()
            },
            since_last_edit: self
                .last_edit
                .map_or(std::time::Duration::MAX, |t| t.elapsed()),
            calc: Some((
                CalcEditInputs {
                    base: provider.base,
                    key_in_progress: false,
                    async_min_lines: CALC_ASYNC_MIN_LINES,
                    defer_min_lines: LARGE_DOC_CALC_DEFER_LINES,
                },
                &provider,
            )),
        };
        let outcome = f(&mut self.session, &mut self.doc, &mut self.input, &cx);
        if outcome.text_changed {
            self.last_edit = Some(Instant::now());
        }
        // Work the terminal schedules for later runs now; large notes
        // evaluate the lines around the cursor.
        if outcome.calc_work != CalcWork::Done {
            let len = self.doc.lines().len();
            let (from, to) = if len >= CALC_VIEWPORT_ONLY_MIN_LINES {
                let c = self.doc.cursor_line;
                (
                    c.saturating_sub(FIRST_SCREEN_LINES),
                    (c + FIRST_SCREEN_LINES).min(len),
                )
            } else {
                (0, len)
            };
            self.session
                .evaluate_calc_range(&self.doc, from, to, &provider);
        }
        outcome
    }

    /// Write unsaved text through the session's save job; returns whether
    /// anything was written.
    pub fn save(&mut self) -> Result<bool, String> {
        let Some(job) = self.session.request_save(
            &mut self.doc,
            SaveContext {
                force: false,
                background: false,
            },
        ) else {
            return Ok(false);
        };
        let result = run_save(job, &self.db);
        match self.session.complete_save(&mut self.doc, result) {
            SaveCompletion::Saved { .. } => {
                self.refresh_titles();
                Ok(true)
            }
            SaveCompletion::Ignored => Ok(false),
            SaveCompletion::Failed(err) => Err(err),
        }
    }

    /// Byte offsets of `(line, col)` in the note joined with `\n`.
    fn byte_offset(&self, line: usize, col: usize) -> usize {
        let lines = self.doc.lines();
        let before: usize = lines[..line.min(lines.len())]
            .iter()
            .map(|l| l.len() + 1)
            .sum();
        before
            + lines.get(line).map_or(0, |l| {
                note_session::display::mapping::scalar_to_byte(l, col)
            })
    }

    /// The selection as byte offsets, end exclusive: Visual modes include
    /// the cursor's character (whole lines in Visual Line), standard editing
    /// ends before the cursor.
    pub fn selection_bytes(&self) -> Option<(usize, usize)> {
        let anchor = self.doc.selection_anchor?;
        let cursor = (self.doc.cursor_line, self.doc.cursor_col);
        let (start, end) = if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        let lines = self.doc.lines();
        let line_len = |l: usize| lines.get(l).map_or(0, |t| t.chars().count());
        let (from, to) = match self.input.mode() {
            VimMode::VisualLine => (
                self.byte_offset(start.0, 0),
                self.byte_offset(end.0, line_len(end.0)),
            ),
            VimMode::Visual => (
                self.byte_offset(start.0, start.1),
                self.byte_offset(end.0, (end.1 + 1).min(line_len(end.0))),
            ),
            _ => (
                self.byte_offset(start.0, start.1),
                self.byte_offset(end.0, end.1),
            ),
        };
        (from < to).then_some((from, to))
    }

    pub fn selected_text(&mut self) -> Option<String> {
        let (from, to) = self.selection_bytes()?;
        self.doc.ensure_joined_text();
        self.doc
            .joined_text_cached()
            .map(|t| t[from..to].to_string())
    }

    /// The whole note with the selection (or cursor), as commands read it.
    fn snapshot(&mut self) -> EditorContextSnapshot {
        let cursor = self.byte_offset(self.doc.cursor_line, self.doc.cursor_col);
        let (anchor, head) = self.selection_bytes().unwrap_or((cursor, cursor));
        self.doc.ensure_joined_text();
        EditorContextSnapshot {
            text: self
                .doc
                .joined_text_cached()
                .unwrap_or_default()
                .to_string(),
            selection: SelectionSnapshot { anchor, head },
            changed_range: None,
        }
    }

    /// Apply a planned text operation as one undoable edit.
    pub fn apply_operation(&mut self, op: &EditOperation) -> InputOutcome {
        self.run_input(|session, doc, input, cx| {
            let session_kind = if input.mode() == VimMode::Insert {
                UndoSession::Insert
            } else {
                UndoSession::Command
            };
            let ctx = EditContext {
                grouping: UndoGrouping {
                    session: session_kind,
                    elapsed: cx.since_last_edit,
                },
                folds: None,
            };
            let applied = session.apply_with_upkeep(
                doc,
                SessionEdit::Operation(op),
                ctx,
                cx.calc.map(|(inputs, _)| inputs),
                cx.calc.map(|(_, provider)| provider),
            );
            let mut outcome = InputOutcome {
                handled: true,
                ..Default::default()
            };
            if let Some(applied) = applied {
                outcome.text_changed = applied.text_changed;
                outcome.first_changed_line = Some(applied.first_changed_line);
                outcome.calc_work = applied.calc_effect.work;
            }
            outcome
        })
    }

    /// Replace the selection (or insert at the cursor) with `text`.
    pub fn insert_text(&mut self, text: &str) -> InputOutcome {
        let snapshot = self.snapshot();
        let op = editor_core::commands::insert_value_at_selection(&snapshot, text);
        self.doc.selection_anchor = None;
        self.apply_operation(&op)
    }

    /// Delete the selection; returns its text.
    pub fn cut_selection(&mut self) -> Option<(String, InputOutcome)> {
        let text = self.selected_text()?;
        let outcome = self.insert_text("");
        Some((text, outcome))
    }

    /// Run a command-bar command through the editor core. Host commands come
    /// back as `CommandRun::Host` for the window to handle.
    pub fn run_command(&mut self, raw: &str) -> CommandRun {
        let raw = raw.trim().trim_start_matches(':').trim();
        let id =
            editor_core::engine::EditorEngine::resolve_command(CommandMode::Vim, raw).map(|c| c.id);
        let snapshot = self.snapshot();
        let result = editor_core::commands::execute_command(&snapshot, raw, CommandMode::Vim);
        if result.message.ends_with("handled by host") {
            return CommandRun::Host {
                id,
                raw: raw.to_string(),
            };
        }
        let mut changed = false;
        for op in &result.operations {
            changed |= self.apply_operation(op).text_changed;
        }
        if changed {
            self.doc.selection_anchor = None;
            if matches!(self.input.mode(), VimMode::Visual | VimMode::VisualLine) {
                self.input.vim = Default::default();
            }
        }
        CommandRun::Done {
            message: result.message,
            clipboard: result.clipboard_text,
            quit: result.quit_requested,
        }
    }

    /// Insert whole lines before line `at`.
    pub fn insert_lines(&mut self, at: usize, lines: Vec<String>) -> InputOutcome {
        self.run_input(|session, doc, input, cx| {
            let mut outcome = InputOutcome {
                handled: true,
                ..Default::default()
            };
            let ctx = EditContext {
                grouping: UndoGrouping {
                    session: UndoSession::Command,
                    elapsed: cx.since_last_edit,
                },
                folds: None,
            };
            let _ = input;
            let applied = session.apply_with_upkeep(
                doc,
                SessionEdit::InsertLines {
                    at,
                    lines: lines.into(),
                },
                ctx,
                cx.calc.map(|(inputs, _)| inputs),
                cx.calc.map(|(_, provider)| provider),
            );
            if let Some(applied) = applied {
                outcome.text_changed = applied.text_changed;
                outcome.first_changed_line = Some(applied.first_changed_line);
                outcome.calc_work = applied.calc_effect.work;
            }
            outcome
        })
    }

    /// Add an empty cell at the right end of every row of the table in
    /// lines `start..=end`; the delimiter row gets a `---` cell.
    pub fn append_table_column(&mut self, start: usize, end: usize) -> InputOutcome {
        let lines = self.doc.lines();
        let mut changes = Vec::new();
        for (i, line) in lines.iter().enumerate().take(end + 1).skip(start) {
            let trimmed = line.trim_end();
            if !trimmed.ends_with('|') {
                continue;
            }
            let cell = if table_syntax::is_delimiter_line_in(lines, i) {
                " --- |"
            } else {
                "  |"
            };
            let at = self.byte_offset(i, 0) + trimmed.len();
            changes.push(editor_core::types::TextChange {
                from: at,
                to: at,
                insert: cell.to_string(),
            });
        }
        if changes.is_empty() {
            return InputOutcome::default();
        }
        let op = EditOperation {
            changes,
            selection: None,
        };
        self.apply_operation(&op)
    }

    /// A script request for the current selection or note, with the ticket
    /// that guards where its result may land. `None` when the script needs
    /// a selection and there is none.
    pub fn script_request(
        &mut self,
        script: &app_core::scripts::ScriptDefinition,
        args: Vec<String>,
    ) -> Option<(
        note_session::scripts::ScriptTicket,
        app_core::scripts::ScriptRequest,
    )> {
        use app_core::scripts::{ScriptInput, ScriptOutput};
        let selection = self.selection_bytes();
        let cursor = self.byte_offset(self.doc.cursor_line, self.doc.cursor_col);
        self.doc.ensure_joined_text();
        let full = self.doc.joined_text_cached().unwrap_or_default();
        let text = match script.input {
            ScriptInput::None => String::new(),
            ScriptInput::Selection => {
                let (a, b) = selection?;
                full[a..b].to_string()
            }
            ScriptInput::Note => full.to_string(),
        };
        if text.len() > app_core::scripts::MAX_INPUT_BYTES {
            return None;
        }
        let range = match (script.output, selection) {
            (ScriptOutput::ReplaceSelection, Some((from, to))) => TextRange { from, to },
            (ScriptOutput::ReplaceSelection, None) => return None,
            _ => TextRange {
                from: cursor,
                to: cursor,
            },
        };
        let ticket = self.session.script_ticket(&self.doc, range, script.output);
        let request = app_core::scripts::ScriptRequest {
            version: 1,
            args,
            text,
            note_id: Some(self.note_id().to_string()),
        };
        Some((ticket, request))
    }

    /// Apply a finished script's result unless the note changed since it
    /// started.
    pub fn apply_script(
        &mut self,
        ticket: &note_session::scripts::ScriptTicket,
        response: &app_core::scripts::ScriptResponse,
    ) -> Result<InputOutcome, String> {
        let edit = self
            .session
            .accept_script_result(&self.doc, ticket, response)
            .map_err(|r| match r {
                note_session::scripts::ScriptRejection::Lifetime => {
                    "the note was closed".to_string()
                }
                note_session::scripts::ScriptRejection::TextChanged => {
                    "the text changed while it ran; result dropped".to_string()
                }
                note_session::scripts::ScriptRejection::NotEditable => {
                    "the note is locked".to_string()
                }
            })?;
        let Some(edit) = edit else {
            return Ok(InputOutcome::default());
        };
        let ctx = EditContext {
            grouping: UndoGrouping {
                session: UndoSession::Command,
                elapsed: std::time::Duration::MAX,
            },
            folds: None,
        };
        let applied = self.session.apply(&mut self.doc, edit, ctx);
        let mut outcome = InputOutcome {
            handled: true,
            ..Default::default()
        };
        if let Some(applied) = applied {
            outcome.text_changed = applied.text_changed;
            outcome.first_changed_line = Some(applied.first_changed_line);
        }
        self.last_edit = Some(Instant::now());
        self.recompute_calc_after_script();
        Ok(outcome)
    }

    fn recompute_calc_after_script(&mut self) {
        let note_id = self.session.note_id().to_string();
        let provider = provider(self.modules, &note_id, &self.index, &self.db, &self.loaded);
        let len = self.doc.lines().len();
        self.session
            .evaluate_calc_range(&self.doc, 0, len, &provider);
    }

    /// Unsaved edits older than the autosave delay.
    pub fn autosave_due(&self, delay: std::time::Duration) -> bool {
        self.session.dirty() && self.last_edit.is_some_and(|t| t.elapsed() >= delay)
    }

    fn refresh_titles(&mut self) {
        if let Ok(notes) = self.db.list_notes_meta() {
            self.notes = notes;
        }
        if let Some(note) = self.notes.iter().find(|n| n.id == self.session.note_id()) {
            self.title = note.title.clone();
        }
    }

    /// Move the cursor to the start of `line`, clamped to the note.
    pub fn set_cursor_line(&mut self, line: usize) {
        let last = self.doc.lines().len().saturating_sub(1);
        self.doc.cursor_line = line.min(last);
        self.doc.cursor_col = 0;
    }

    /// Code-fence state before each line, so any line can be styled alone.
    pub fn fence_starts(&self) -> Vec<FenceState> {
        let mut fence = FenceState::default();
        self.doc
            .lines()
            .iter()
            .map(|text| {
                let before = fence.clone();
                markdown_tokens::advance_fence_state(&mut fence, text);
                before
            })
            .collect()
    }

    /// Lines `from..to` ready to paint.
    pub fn lines(&self, from: usize, to: usize) -> Vec<LineView> {
        let fences = self.fence_starts();
        (from..to.min(fences.len()))
            .map(|ix| self.line_view(ix, &fences[ix]))
            .collect()
    }

    /// Visual selection on `line` in source columns, end exclusive, or
    /// `None` when the line is outside it. `Some(None)` selects the line.
    fn selection_on(&self, line: usize, len: usize) -> Option<Option<Range<usize>>> {
        let anchor = self.doc.selection_anchor?;
        let cursor = (self.doc.cursor_line, self.doc.cursor_col);
        let (start, end) = if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        if line < start.0 || line > end.0 {
            return None;
        }
        match self.input.mode() {
            VimMode::VisualLine => Some(None),
            VimMode::Visual => {
                let from = if line == start.0 { start.1 } else { 0 };
                let to = if line == end.0 {
                    (end.1 + 1).min(len)
                } else {
                    len
                };
                Some(Some(from..to.max(from)))
            }
            // Standard editing: the selection ends before the cursor.
            VimMode::Insert => {
                let from = if line == start.0 { start.1 } else { 0 };
                let to = if line == end.0 { end.1.min(len) } else { len };
                Some(Some(from..to.max(from)))
            }
            VimMode::Normal => None,
        }
    }

    /// One line ready to paint, given the fence state before it.
    pub fn line_view(&self, index: usize, fence: &FenceState) -> LineView {
        let text = self.doc.lines()[index].as_str();
        let calc = self.session.calc();
        let variables = (!calc.variable_names.is_empty()).then_some(&calc.variable_names);
        let is_cursor = index == self.doc.cursor_line;
        let in_code = fence.in_code_block;
        let len = text.chars().count();
        let selection = self.selection_on(index, len);
        if !in_code && is_markdown_table_line(text) {
            let mut view = self.table_line(index, text);
            view.line_selected = selection.is_some();
            return view;
        }
        let result = calc.results.get(index).and_then(|r| r.as_deref());
        let mut ctx = SemanticContext {
            fence: fence.clone(),
            render_as_plain_code: false,
            forced_code_lang: None,
        };
        let deco = LineDecorations {
            calc_ghost: result,
            variable_names: variables,
            active_cursor_col: is_cursor.then_some(self.doc.cursor_col),
            ..Default::default()
        };
        let styled = ctx.style_line(text, &deco);
        let info = (!in_code).then(|| markdown_tokens::classify_markdown_line(text));
        let mut kind = LineKind::Text;
        let mut start = 0;
        if let Some(info) = &info {
            if let Some(level) = info.heading_level {
                kind = LineKind::Heading(level);
            } else if let (Some(content), false) = (info.checklist_content_start, is_cursor) {
                kind = LineKind::Checklist {
                    checked: info.checklist_checked,
                };
                start = content;
            }
        }
        // Source columns to positions in the runs, which drop hidden
        // markers and anything before `start`.
        let map = &styled.source_map;
        let shown = |col: usize, affinity| {
            let skipped = map.source_to_display(start, Affinity::After).unwrap_or(0);
            map.source_to_display(col, affinity)
                .map(|d| d.saturating_sub(skipped))
        };
        let (selection, line_selected) = match selection {
            Some(Some(range)) => (
                shown(range.start, Affinity::After)
                    .zip(shown(range.end, Affinity::Before))
                    .map(|(a, b)| a..b.max(a)),
                false,
            ),
            Some(None) => (None, true),
            None => (None, false),
        };
        LineView {
            index,
            kind,
            runs: runs(&styled, start),
            ghost: result.map(|r| format!("{}{}", styled.calc_prefix, r)),
            cursor: is_cursor
                .then(|| shown(self.doc.cursor_col, Affinity::After))
                .flatten(),
            selection,
            line_selected,
        }
    }

    fn table_line(&self, index: usize, text: &str) -> LineView {
        let lines = self.doc.lines();
        let mut view = LineView {
            index,
            kind: LineKind::TableDelimiter,
            runs: Vec::new(),
            ghost: None,
            cursor: None,
            selection: None,
            line_selected: false,
        };
        if table_syntax::is_delimiter_line_in(lines, index) {
            return view;
        }
        let header = table_syntax::is_delimiter_line_in(lines, index + 1);
        let results = self.session.calc().cell_results.get(index);
        let is_cursor = index == self.doc.cursor_line;
        let cells = table_syntax::split_table_cells(text)
            .into_iter()
            .enumerate()
            .map(|(cell_index, raw)| {
                let value = results.and_then(|r| r.iter().find(|e| e.cell_index == cell_index));
                match value {
                    // The cursor row keeps its formulas visible for editing.
                    Some(eval) if !is_cursor => TableCell {
                        text: format_formula_display_value(&eval.value),
                        formula: true,
                    },
                    _ => TableCell {
                        formula: raw.starts_with(":="),
                        text: raw,
                    },
                }
            })
            .collect();
        view.kind = LineKind::TableRow { cells, header };
        view
    }
}

/// Reminder marks for the lines the stored reminders now sit on.
fn reminder_ghosts(
    reminders: &[app_core::storage::Reminder],
    lines: &[String],
) -> rustc_hash::FxHashMap<usize, note_session::LineReminderGhost> {
    app_core::reminders::place_reminders(reminders, lines)
        .into_iter()
        .zip(reminders)
        .filter_map(|(line, r)| {
            let line = line?;
            Some((
                line,
                note_session::LineReminderGhost {
                    remind_at_ms: r.remind_at_ms,
                    display_at: r.display_at.clone(),
                    line_text: lines[line].clone(),
                    reminded_at_ms: r.reminded_at_ms,
                },
            ))
        })
        .collect()
}

/// Visible characters from `start`, grouped by style; hidden markers are dropped.
fn runs(line: &SemanticLine, start: usize) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let mut hidden = line.hidden_ranges.iter().peekable();
    for (i, (&ch, &style)) in line.chars.iter().zip(&line.styles).enumerate().skip(start) {
        while hidden.peek().is_some_and(|&&(_, end)| end <= i) {
            hidden.next();
        }
        if hidden.peek().is_some_and(|&&(from, _)| from <= i) {
            continue;
        }
        match out.last_mut() {
            Some(run) if run.style == style => run.text.push(ch),
            _ => out.push(Run {
                text: ch.to_string(),
                style,
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    const LISBON: &str = "# Lisbon trip\n\
        ## Budget\n\
        flights := 2 * 189\n\
        hotel := 4 * 96\n\
        flights + hotel\n\
        - [x] Book flights\n\
        - [ ] Renew passport\n\
        \n\
        | Item | Qty | Price | Total |\n\
        | --- | --- | --- | --- |\n\
        | Tram pass | 2 | 6.60 | :=(1,2)*(1,3) |\n\
        | Museum | 2 | 12 | :=(2,2)*(2,3) |";

    struct Fixture {
        host: NoteHost,
        dir: std::path::PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    fn fixture(body: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "slate-gui-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(dir.join("notes.db")).unwrap();
        db.create_note_with_context("lisbon", Default::default(), None, None)
            .unwrap();
        db.save_note("lisbon", body).unwrap();
        let host = NoteHost::open(db, Some("lisbon")).unwrap();
        Fixture { host, dir }
    }
    fn text(line: &LineView) -> String {
        line.runs.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn headings_hide_their_marker_away_from_the_cursor() {
        let mut f = fixture(LISBON);
        f.host.set_cursor_line(2);
        let lines = f.host.lines(0, 2);
        assert_eq!(lines[0].kind, LineKind::Heading(1));
        assert_eq!(text(&lines[0]), "Lisbon trip");
        assert_eq!(lines[1].kind, LineKind::Heading(2));
        assert_eq!(text(&lines[1]), "Budget");
    }

    #[test]
    fn expressions_carry_their_calc_result() {
        let f = fixture(LISBON);
        let line = &f.host.lines(4, 5)[0];
        let ghost = line.ghost.as_deref().expect("calc result");
        assert!(ghost.contains("762"), "{ghost}");
        assert_eq!(text(line), "flights + hotel");
    }

    #[test]
    fn checklists_report_state_and_drop_the_marker() {
        let f = fixture(LISBON);
        let lines = f.host.lines(5, 7);
        assert_eq!(lines[0].kind, LineKind::Checklist { checked: true });
        assert_eq!(text(&lines[0]), "Book flights");
        assert_eq!(lines[1].kind, LineKind::Checklist { checked: false });
    }

    #[test]
    fn tables_show_formula_values_except_on_the_cursor_row() {
        let mut f = fixture(LISBON);
        let lines = f.host.lines(8, 12);
        let LineKind::TableRow { cells, header } = &lines[0].kind else {
            panic!("header row")
        };
        assert!(*header);
        assert_eq!(cells[0].text, "Item");
        assert_eq!(lines[1].kind, LineKind::TableDelimiter);
        let LineKind::TableRow { cells, header } = &lines[2].kind else {
            panic!("body row")
        };
        assert!(!*header);
        assert!(cells[3].formula);
        assert_eq!(cells[3].text, "13.2");

        f.host.set_cursor_line(10);
        let LineKind::TableRow { cells, .. } = &f.host.lines(10, 11)[0].kind else {
            panic!("cursor row")
        };
        assert_eq!(cells[3].text, ":=(1,2)*(1,3)");
    }

    #[test]
    fn switching_notes_reloads_text_and_calc() {
        let mut f = fixture(LISBON);
        f.host
            .db
            .create_note_with_context("other", Default::default(), None, None)
            .unwrap();
        f.host.db.save_note("other", "2 + 3").unwrap();
        f.host.switch_to("other").unwrap();
        assert_eq!(f.host.note_id(), "other");
        let line = &f.host.lines(0, 1)[0];
        assert_eq!(line.ghost.as_deref(), Some(" → 5"));
    }
}
