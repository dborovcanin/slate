//! Text history restoration and semantic undo ordering; IO stays with the host.
use crate::{Document, EditOutcome, NoteSession};
use editor_core::{buffer::EditDelta, history::policy::UndoAction};

#[derive(Clone, Copy, Default)]
pub struct UndoContext {
    pub normal_mode: bool,
    pub table_enabled: bool,
}
pub enum SessionUndoOutcome {
    Text(EditOutcome),
    Reminder { line_idx: usize },
    Exhausted,
    NotEditable,
}

impl NoteSession {
    /// Apply the next semantic action. Reminder-only actions return no text outcome.
    pub fn undo(&mut self, doc: &mut Document) -> Option<EditOutcome> {
        match self.undo_action(doc, UndoContext::default()) {
            SessionUndoOutcome::Text(outcome) => Some(outcome),
            _ => None,
        }
    }
    pub fn redo(&mut self, doc: &mut Document) -> Option<EditOutcome> {
        match self.redo_action(doc, UndoContext::default()) {
            SessionUndoOutcome::Text(outcome) => Some(outcome),
            _ => None,
        }
    }

    /// Shared semantic dispatch; callers must not acknowledge UndoPolicy a second time.
    pub fn undo_action(&mut self, doc: &mut Document, ctx: UndoContext) -> SessionUndoOutcome {
        self.restore_action(doc, false, ctx, None, None, None)
    }
    pub fn redo_action(&mut self, doc: &mut Document, ctx: UndoContext) -> SessionUndoOutcome {
        self.restore_action(doc, true, ctx, None, None, None)
    }
    pub fn undo_action_with_upkeep(
        &mut self,
        doc: &mut Document,
        ctx: UndoContext,
        calc_inputs: Option<crate::calc::CalcInputs>,
        provider: Option<&dyn crate::calc_upkeep::CalcProvider>,
        folds: Option<crate::folds::FoldInputs>,
    ) -> SessionUndoOutcome {
        self.restore_action(doc, false, ctx, calc_inputs, provider, folds)
    }
    pub fn redo_action_with_upkeep(
        &mut self,
        doc: &mut Document,
        ctx: UndoContext,
        calc_inputs: Option<crate::calc::CalcInputs>,
        provider: Option<&dyn crate::calc_upkeep::CalcProvider>,
        folds: Option<crate::folds::FoldInputs>,
    ) -> SessionUndoOutcome {
        self.restore_action(doc, true, ctx, calc_inputs, provider, folds)
    }
    fn restore_action(
        &mut self,
        doc: &mut Document,
        redo: bool,
        ctx: UndoContext,
        calc_inputs: Option<crate::calc::CalcInputs>,
        provider: Option<&dyn crate::calc_upkeep::CalcProvider>,
        folds: Option<crate::folds::FoldInputs>,
    ) -> SessionUndoOutcome {
        if !self.editable() {
            return SessionUndoOutcome::NotEditable;
        }
        let action = if redo {
            self.undo_policy.redo_action()
        } else {
            self.undo_policy.undo_action()
        }
        .cloned();
        let Some(action) = action else {
            return SessionUndoOutcome::Exhausted;
        };
        let result = match action {
            UndoAction::Text => {
                let result = if redo {
                    self.restore_text(doc, true, false)
                } else {
                    self.restore_text(doc, false, self.history.undo_depth() == 1)
                };
                if let Some(mut outcome) = result {
                    self.calc.pending_result_splices.clear();
                    if let Some(inputs) = calc_inputs {
                        if !inputs.math_enabled {
                            self.calc.clear(doc);
                        } else if inputs.viewport_only {
                            let (needs_eval, index_sync_pending) =
                                self.calc.refresh_viewport_calc_after_edit(doc, inputs);
                            outcome.calc_effect.index_sync_pending = index_sync_pending;
                            if needs_eval {
                                outcome.calc_effect.work =
                                    crate::calc_upkeep::CalcWork::RefreshViewport;
                            }
                        } else if let Some(provider) = provider {
                            let inputs = provider.inputs(&self.calc);
                            if inputs.cross_note_enabled {
                                let refs = app_core::calc::scan_cross_note_refs(doc.lines());
                                provider.preload_refs(&refs);
                            }
                            self.recompute_calc(doc, inputs, &mut |lines| {
                                provider.extern_vars(lines)
                            });
                        } else {
                            outcome.calc_effect.work = crate::calc_upkeep::CalcWork::Idle;
                        }
                    }
                    self.folds.rescan_pending = true;
                    if let Some(inputs) = folds {
                        outcome.fold_effect = self.folds.upkeep(
                            doc,
                            None,
                            doc.lines().len() > inputs.full_feature_line_limit,
                            inputs.has_collapsed,
                            inputs.view_line_count,
                        );
                    }

                    self.checkpoint_restored_cursor(doc, ctx);
                    SessionUndoOutcome::Text(outcome)
                } else {
                    SessionUndoOutcome::Exhausted
                }
            }
            UndoAction::Reminder(entry) => {
                let state = if redo { entry.after } else { entry.before };
                match state {
                    Some(mark) => {
                        self.reminder_ghosts.insert(entry.line_idx, mark);
                    }
                    None => {
                        self.reminder_ghosts.remove(&entry.line_idx);
                    }
                }
                self.reminders_generation = self.reminders_generation.wrapping_add(1);
                self.note_changed();
                self.history
                    .set_marks(crate::reminder_marks_of(&self.reminder_ghosts));
                SessionUndoOutcome::Reminder {
                    line_idx: entry.line_idx,
                }
            }
        };
        if redo {
            self.undo_policy.complete_redo();
        } else {
            self.undo_policy.complete_undo();
        }
        result
    }

    /// Normalize a restored source caret before updating its history checkpoint.
    /// A host may call again after moving a caret out of its collapsed view.
    pub fn checkpoint_restored_cursor(&mut self, doc: &mut Document, ctx: UndoContext) {
        doc.cursor_line = doc.cursor_line.min(doc.lines().len().saturating_sub(1));
        let Some(line) = doc.lines.get(doc.cursor_line) else {
            return;
        };
        let len = line.chars().count();
        doc.cursor_col = doc.cursor_col.min(len);
        if ctx.table_enabled {
            let byte = line
                .char_indices()
                .nth(doc.cursor_col)
                .map_or(line.len(), |(at, _)| at);
            if let Some(cell) = editor_core::table::table_cell_info_in_line(line, byte) {
                let anchor_byte = if cell.is_empty() {
                    (cell.left_pipe + 2).min(cell.right_pipe)
                } else {
                    (cell.left_pipe + 1 + cell.trim_end).min(cell.right_pipe)
                };
                let anchor = line[..anchor_byte].chars().count();
                let edit_start = line[..(cell.left_pipe + 2).min(cell.right_pipe)]
                    .chars()
                    .count();
                if cell.is_empty() || doc.cursor_col < edit_start || doc.cursor_col > anchor {
                    doc.cursor_col = anchor;
                }
            }
        }
        doc.cursor_col = doc.cursor_col.min(if ctx.normal_mode {
            len.saturating_sub(1)
        } else {
            len
        });
        self.history
            .checkpoint(doc.lines(), doc.cursor_line, doc.cursor_col);
    }

    fn restore_text(
        &mut self,
        doc: &mut Document,
        redo: bool,
        keep_cursor: bool,
    ) -> Option<EditOutcome> {
        if !self.editable() {
            return None;
        }
        let old_span = doc.lines.len();
        let old_cursor = doc.cursor();
        let cursor = if redo {
            self.history.redo(&mut doc.lines)?
        } else {
            self.history.undo(&mut doc.lines)?
        };
        doc.cursor_line = if keep_cursor {
            old_cursor.line
        } else {
            cursor.line
        }
        .min(doc.lines.len().saturating_sub(1));
        doc.cursor_col = if keep_cursor {
            old_cursor.column
        } else {
            cursor.col
        };
        doc.text_generation = doc.text_generation.wrapping_add(1);
        doc.joined_text_cache = None;
        self.dirty = true;
        self.note_changed();
        self.pending_line_edits.clear();
        let marks = self.history.current_marks().clone();
        if *marks != *crate::reminders::reminder_marks_of(&self.reminder_ghosts) {
            self.reminder_ghosts = marks.iter().cloned().collect();
            self.reminders_generation = self.reminders_generation.wrapping_add(1);
        }
        Some(EditOutcome {
            delta: EditDelta {
                start_line: 0,
                old_span,
                new_span: doc.lines.len(),
            },
            first_changed_line: 0,
            title_changed: true,
            calc_splices: Vec::new(),
            fold_rescan: true,
            register: None,
            text_changed: true,
            calc_effect: Default::default(),
            fold_effect: Default::default(),
        })
    }
}
#[cfg(test)]
mod semantic_tests {
    use super::*;
    use crate::{EditContext, LineReminderGhost, ReminderUndoEntry, SessionEdit};
    use editor_core::{
        buffer::primitives::PrimitiveEdit,
        history::policy::{UndoGrouping, UndoSession},
    };
    use std::time::Duration;
    fn fixture() -> (Document, NoteSession) {
        let doc = Document::from_text("éx");
        let history =
            crate::lifecycle::build_history_for_note(doc.lines(), 0, 0, Default::default());
        (
            doc,
            NoteSession::new(history, Default::default(), Default::default()),
        )
    }
    fn add(doc: &mut Document, session: &mut NoteSession, ch: char) {
        session.history.break_coalescing();
        session
            .apply(
                doc,
                SessionEdit::Primitive(PrimitiveEdit::InsertChar(ch)),
                EditContext {
                    grouping: UndoGrouping {
                        session: UndoSession::Command,
                        elapsed: Duration::from_secs(1),
                    },
                    folds: None,
                },
            )
            .unwrap();
    }
    #[test]
    fn public_wrappers_follow_text_reminder_text_order() {
        let (mut doc, mut session) = fixture();
        add(&mut doc, &mut session, 'a');
        let mark = LineReminderGhost {
            remind_at_ms: 1,
            display_at: "soon".into(),
            line_text: "aéx".into(),
            reminded_at_ms: None,
        };
        session.reminder_ghosts.insert(0, mark.clone());
        session
            .history
            .set_marks(crate::reminder_marks_of(&session.reminder_ghosts));
        session.undo_policy.record_reminder(ReminderUndoEntry {
            line_idx: 0,
            before: None,
            after: Some(mark),
        });
        add(&mut doc, &mut session, 'b');
        assert!(session.undo(&mut doc).is_some());
        assert_eq!(doc.lines(), &["aéx".to_owned()]);
        let text_generation = doc.text_generation;
        assert!(session.undo(&mut doc).is_none());
        assert!(session.reminder_ghosts.is_empty());
        assert_eq!(doc.text_generation, text_generation);
        assert!(session.undo(&mut doc).is_some());
        assert_eq!(doc.lines(), &["éx".to_owned()]);
        assert!(session.redo(&mut doc).is_some());
        assert!(session.redo(&mut doc).is_none());
        assert!(session.reminder_ghosts.contains_key(&0));
        assert!(session.redo(&mut doc).is_some());
        assert_eq!(doc.lines(), &["abéx".to_owned()]);
        assert_eq!(session.undo_policy.undo_depth(), 3);
    }
    #[test]
    fn shared_undo_upkeep_clears_disabled_calc_and_rebuilds_fold_structure() {
        let (mut doc, mut session) = fixture();
        add(&mut doc, &mut session, 'a');
        session.calc.results = vec![Some("stale".into())];
        let inputs = crate::calc::CalcInputs {
            mask: Default::default(),
            math_enabled: false,
            viewport_only: false,
        };
        let folds = crate::folds::FoldInputs {
            full_feature_line_limit: 30_000,
            has_collapsed: true,
            view_line_count: 1,
        };
        let SessionUndoOutcome::Text(outcome) = session.undo_action_with_upkeep(
            &mut doc,
            UndoContext {
                normal_mode: true,
                ..Default::default()
            },
            Some(inputs),
            None,
            Some(folds),
        ) else {
            panic!("text undo")
        };
        assert_eq!(session.calc.results, vec![None]);
        assert_eq!(outcome.calc_effect.work, crate::calc_upkeep::CalcWork::Done);
        assert!(session.folds.analysis_ready);
        assert!(outcome.fold_effect.rebuild_view);
        assert!(!session.folds.rescan_pending);
    }
    #[test]
    fn locked_session_restoration_is_inert() {
        let (mut doc, mut session) = fixture();
        add(&mut doc, &mut session, 'a');
        session.access_mode = app_core::storage::NoteAccessMode::Encrypted;
        session.is_unlocked = false;
        let identity = (
            session.edit_seq,
            doc.text_generation,
            session.history.undo_depth(),
            session.undo_policy.undo_depth(),
        );
        assert!(session.undo(&mut doc).is_none());
        assert!(session.redo(&mut doc).is_none());
        assert!(matches!(
            session.undo_action(&mut doc, UndoContext::default()),
            SessionUndoOutcome::NotEditable
        ));
        assert_eq!(doc.lines(), &["aéx".to_owned()]);
        assert_eq!(
            (
                session.edit_seq,
                doc.text_generation,
                session.history.undo_depth(),
                session.undo_policy.undo_depth()
            ),
            identity
        );
    }
    #[test]
    fn normal_cursor_is_clamped_before_checkpoint_on_unicode_and_empty_lines() {
        let (mut doc, mut session) = fixture();
        doc.cursor_col = 2;
        add(&mut doc, &mut session, '!');
        assert!(matches!(
            session.undo_action(
                &mut doc,
                UndoContext {
                    normal_mode: true,
                    ..Default::default()
                }
            ),
            SessionUndoOutcome::Text(_)
        ));
        assert_eq!(doc.cursor_col, 1);
        assert!(matches!(
            session.redo_action(
                &mut doc,
                UndoContext {
                    normal_mode: true,
                    ..Default::default()
                }
            ),
            SessionUndoOutcome::Text(_)
        ));
        assert!(doc.cursor_col < doc.lines()[0].chars().count());
        let mut doc = Document::from_text("");
        let history =
            crate::lifecycle::build_history_for_note(doc.lines(), 0, 0, Default::default());
        let mut session = NoteSession::new(history, Default::default(), Default::default());
        add(&mut doc, &mut session, 'x');
        session.undo_action(
            &mut doc,
            UndoContext {
                normal_mode: true,
                ..Default::default()
            },
        );
        assert_eq!(doc.cursor_col, 0);
    }
}
