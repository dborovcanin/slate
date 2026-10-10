//! One preparation/application boundary for document edits and their bookkeeping.
use crate::{Document, NoteSession, PendingLineChange};
use app_core::reminders::LineEdit;
use editor_core::buffer::{
    lines::{apply_insert_lines, apply_remove_lines, prepare_insert_lines, prepare_remove_lines},
    paste::{apply_plain_paste, prepare_plain_paste},
    primitives::{apply_primitive_edit, prepare_primitive_edit, PrimitiveEdit},
    replace::{apply_line_replace, prepare_line_replace},
    words::{apply_word_delete, prepare_backward_word_delete, BackwardWordDelete},
    EditDelta, ExactTextEdit,
};
use editor_core::history::{policy::UndoGrouping, HistoryCursor};
use std::{borrow::Cow, ops::Range};

/// Text is borrowed through preparation and application, never stored by a session.
pub enum SessionEdit<'a> {
    /// Canonicalize an empty storage buffer without changing its text.
    EnsureBuffer,
    Operation(&'a editor_core::types::EditOperation),
    TableCellPaste {
        text: &'a str,
        tables: bool,
        cache: &'a mut editor_core::table::TableFormatCache,
    },
    TableImport(&'a [String]),
    Visual {
        linewise: bool,
        delete: bool,
    },
    Primitive(PrimitiveEdit<'a>),
    LineReplace {
        line: usize,
        range: Range<usize>,
        text: &'a str,
        preserve_cursor: bool,
        cursor_after: Option<editor_core::buffer::primitives::BufferCursor>,
    },
    /// The caller normalizes CRLF before requesting this paste.
    PlainPaste(&'a str),
    InsertLines {
        at: usize,
        lines: Cow<'a, [String]>,
    },
    PruneEmptyTableContinuation,
    RemoveLines {
        start: usize,
        end: usize,
    },
    BackwardWordDelete {
        tables: bool,
    },
}

#[derive(Clone, Copy)]
pub struct EditContext {
    pub grouping: UndoGrouping,
    pub folds: Option<crate::folds::FoldInputs>,
}

#[derive(Debug)]
pub struct EditOutcome {
    pub delta: EditDelta,
    pub first_changed_line: usize,
    pub title_changed: bool,
    pub calc_splices: Vec<EditDelta>,
    pub fold_rescan: bool,
    pub register: Option<editor_core::vim_actions::VimRegisterValue>,
    pub text_changed: bool,
    pub calc_effect: crate::calc_upkeep::CalcEffect,
    pub fold_effect: crate::folds::FoldEffect,
}

impl NoteSession {
    /// Prepare against the current buffer and consume that plan immediately.
    /// Boundary/no-op requests return None without advancing edit identity.
    pub fn apply(
        &mut self,
        doc: &mut Document,
        edit: SessionEdit<'_>,
        ctx: EditContext,
    ) -> Option<EditOutcome> {
        self.apply_with_upkeep(doc, edit, ctx, None, None)
    }

    /// Apply text, derived calc state and history as one synchronous transaction.
    /// The borrowed provider supplies lazy IO inputs to the shared evaluator.
    pub fn apply_with_upkeep(
        &mut self,
        doc: &mut Document,
        edit: SessionEdit<'_>,
        ctx: EditContext,
        inputs: Option<crate::calc_upkeep::CalcEditInputs>,
        provider: Option<&dyn crate::calc_upkeep::CalcProvider>,
    ) -> Option<EditOutcome> {
        let finalize_empty_delete = matches!(edit, SessionEdit::Visual { delete: true, .. });
        let cursor_before = doc.cursor();
        let mut calc_splices = Vec::new();
        let mut fold_rescan = false;
        let mut register = None;
        let mut text_changed = true;
        let delta = match edit {
            SessionEdit::EnsureBuffer => {
                if doc.lines.is_empty() {
                    doc.lines.push(String::new());
                }
                return None;
            }
            SessionEdit::Operation(op) => {
                let outcome = self.apply_operation_text(doc, op)?;
                calc_splices = outcome.calc_splices;
                fold_rescan = outcome.fold_rescan;
                outcome.delta
            }
            SessionEdit::TableCellPaste {
                text,
                tables,
                cache,
            } => {
                let plan = editor_core::buffer::paste::prepare_table_cell_paste(
                    &doc.lines,
                    doc.cursor(),
                    text,
                    tables,
                    cache,
                )?;
                let delta = EditDelta {
                    start_line: plan.start,
                    old_span: plan.end - plan.start + 1,
                    new_span: plan.lines.len(),
                };
                if !self.reminder_ghosts.is_empty() {
                    let fates = app_core::reminders::block_line_fates(
                        &doc.lines[plan.start..=plan.end],
                        &plan.lines,
                    );
                    self.record_block(delta, fates);
                }
                let cursor =
                    editor_core::buffer::paste::apply_table_cell_paste(&mut doc.lines, plan);
                doc.set_cursor(cursor);
                delta
            }
            SessionEdit::TableImport(lines) => {
                let current = doc.lines.get(doc.cursor_line).map_or("", String::as_str);
                let (text, cursor) =
                    editor_core::buffer::paste::prepare_table_import(lines, current, doc.cursor());
                doc.set_cursor(cursor);
                return self.apply_with_upkeep(
                    doc,
                    SessionEdit::PlainPaste(&text),
                    ctx,
                    inputs,
                    provider,
                );
            }
            SessionEdit::Visual { linewise, delete } => {
                if doc.lines.is_empty() {
                    doc.lines.push(String::new());
                }
                let anchor = doc
                    .selection_anchor
                    .unwrap_or((doc.cursor_line, doc.cursor_col));
                let plan = editor_core::vim_actions::buffer::prepare_visual_selection(
                    &doc.lines,
                    doc.cursor(),
                    editor_core::buffer::primitives::BufferCursor {
                        line: anchor.0,
                        column: anchor.1,
                    },
                    linewise,
                    delete,
                )?;
                let delta = plan.delta;
                text_changed = plan.text_changed;
                if let Some((start, end)) = plan.deleted_lines {
                    // Deletion discards ownership even when a matching empty slot survives.
                    let before = self.reminder_ghosts.len();
                    self.reminder_ghosts
                        .retain(|line, _| *line < start || *line > end);
                    if self.reminder_ghosts.len() != before {
                        self.reminders_generation = self.reminders_generation.wrapping_add(1);
                    }
                    self.record_block(delta, vec![None; delta.old_span]);
                }
                if let Some(edit) = plan.exact_edit {
                    self.record_exact(edit);
                }
                let (value, cursor) = plan.apply(&mut doc.lines);
                register = Some(value);
                doc.set_cursor(cursor);
                delta
            }

            SessionEdit::Primitive(edit) => {
                let plan = prepare_primitive_edit(&doc.lines, doc.cursor(), edit)?;
                let delta = plan.delta;
                if delta.old_span != delta.new_span {
                    self.record_exact(plan.edit);
                }
                let cursor = apply_primitive_edit(&mut doc.lines, plan);
                doc.set_cursor(cursor);
                delta
            }
            SessionEdit::LineReplace {
                line,
                range,
                text,
                preserve_cursor,
                cursor_after,
            } => {
                let plan = prepare_line_replace(&doc.lines, line, range, text)?;
                let delta = plan.delta;
                let cursor = apply_line_replace(&mut doc.lines, plan);
                doc.set_cursor(cursor_after.unwrap_or(if preserve_cursor {
                    cursor_before
                } else {
                    cursor
                }));
                delta
            }
            SessionEdit::PlainPaste(text) => {
                let plan = prepare_plain_paste(&doc.lines, doc.cursor(), text)?;
                let delta = plan.delta;
                if plan.edit.inserted_breaks > 0 {
                    self.record_exact(plan.edit);
                }
                let cursor = apply_plain_paste(&mut doc.lines, plan);
                doc.set_cursor(cursor);
                delta
            }
            SessionEdit::InsertLines { at, lines } => {
                let plan = prepare_insert_lines(&doc.lines, at, lines)?;
                let delta = plan.delta;
                self.record_block(delta, Vec::new());
                let cursor = apply_insert_lines(&mut doc.lines, plan);
                doc.set_cursor(cursor);
                delta
            }
            SessionEdit::PruneEmptyTableContinuation => {
                let line = doc.lines.get(doc.cursor_line)?;
                if !editor_core::table::is_empty_table_continuation_row(line) {
                    return None;
                }
                let at = doc.cursor_line;
                let old_col = doc.cursor_col;
                let mut plan = prepare_remove_lines(&doc.lines, at, at + 1)?;
                let delta = plan.delta;
                self.record_block(delta, std::mem::take(&mut plan.fates));
                apply_remove_lines(&mut doc.lines, plan);
                doc.cursor_line = at.saturating_sub(1).min(doc.lines.len() - 1);
                let line = &doc.lines[doc.cursor_line];
                doc.cursor_col = old_col.min(line.chars().count());
                let byte = line
                    .char_indices()
                    .nth(doc.cursor_col)
                    .map_or(line.len(), |(at, _)| at);
                if let Some(cell) = editor_core::table::table_cell_info_in_line(line, byte) {
                    let anchor_byte = if cell.is_empty() {
                        (cell.left_pipe + 2).min(cell.right_pipe)
                    } else {
                        ((cell.left_pipe + 1) + cell.trim_end).min(cell.right_pipe)
                    };
                    doc.cursor_col = line[..anchor_byte].chars().count();
                }
                delta
            }
            SessionEdit::RemoveLines { start, end } => {
                let mut plan = prepare_remove_lines(&doc.lines, start, end)?;
                let delta = plan.delta;
                self.record_block(delta, std::mem::take(&mut plan.fates));
                let cursor = apply_remove_lines(&mut doc.lines, plan);
                doc.set_cursor(cursor);
                delta
            }
            SessionEdit::BackwardWordDelete { tables } => {
                match prepare_backward_word_delete(&doc.lines, doc.cursor(), tables)? {
                    BackwardWordDelete::JoinPreviousLine => {
                        return self.apply_with_upkeep(
                            doc,
                            SessionEdit::Primitive(PrimitiveEdit::Backspace),
                            ctx,
                            inputs,
                            provider,
                        );
                    }
                    BackwardWordDelete::WithinLine(plan) => {
                        if plan.edit.from == plan.edit.to {
                            return None;
                        }
                        let delta = plan.delta;
                        doc.cursor_col = apply_word_delete(&mut doc.lines[doc.cursor_line], plan);
                        delta
                    }
                }
            }
        };
        if text_changed || finalize_empty_delete {
            doc.text_generation = doc.text_generation.wrapping_add(1);
            doc.joined_text_cache = None;
            self.dirty = true;
            self.edit_seq = self.edit_seq.wrapping_add(1);
        }
        let calc_effect = if text_changed || finalize_empty_delete {
            inputs.map_or_else(Default::default, |inputs| {
                self.calc_after_edit(doc, Some(delta), !calc_splices.is_empty(), inputs, provider)
            })
        } else {
            Default::default()
        };
        let fold_effect = if text_changed || finalize_empty_delete {
            ctx.folds.map_or_else(Default::default, |inputs| {
                self.folds.rescan_pending |= fold_rescan;
                self.folds.upkeep(
                    doc,
                    Some(delta),
                    doc.lines().len() > inputs.full_feature_line_limit,
                    inputs.has_collapsed,
                    inputs.view_line_count,
                )
            })
        } else {
            Default::default()
        };
        let outcome = EditOutcome {
            delta,
            first_changed_line: delta.start_line,
            title_changed: delta.start_line == 0,
            calc_splices,
            fold_rescan,
            register,
            text_changed,
            calc_effect,
            fold_effect,
        };
        if text_changed || finalize_empty_delete {
            self.finish_edit(doc, ctx, text_changed.then_some(outcome.delta));
        }
        Some(outcome)
    }

    pub(crate) fn record_exact(&mut self, edit: ExactTextEdit) {
        if self.reminder_ghosts.is_empty() {
            return;
        }
        self.pending_line_edits
            .push(PendingLineChange::Edit(LineEdit {
                from_line: edit.from.0,
                from_col: edit.from.1,
                from_line_len: edit.from_line_len,
                to_line: edit.to.0,
                to_col: edit.to.1,
                to_line_len: edit.to_line_len,
                inserted_breaks: edit.inserted_breaks,
            }));
    }

    fn record_block(&mut self, delta: EditDelta, fates: Vec<Option<usize>>) {
        if self.reminder_ghosts.is_empty() {
            return;
        }
        self.pending_line_edits.push(PendingLineChange::Block {
            start: delta.start_line,
            old_len: delta.old_span,
            new_len: delta.new_span,
            fates,
        });
    }

    pub(crate) fn finish_edit(
        &mut self,
        doc: &Document,
        ctx: EditContext,
        delta: Option<EditDelta>,
    ) {
        self.undo_policy.record_text(
            &mut self.history,
            &doc.lines,
            HistoryCursor {
                line: doc.cursor_line,
                col: doc.cursor_col,
            },
            delta,
            ctx.grouping,
        );
        let edits = std::mem::take(&mut self.pending_line_edits);
        let delta = self.history.take_last_delta();
        if self.reminder_ghosts.is_empty() {
            if !self.history.current_marks().is_empty() {
                self.history
                    .record_marks(crate::reminders::reminder_marks_of(&self.reminder_ghosts));
            }
            return;
        }
        let Some(delta) = delta else {
            return;
        };
        let moved = crate::reminders::move_lines(&self.reminder_ghosts, &edits, &delta);
        if moved.len() != self.reminder_ghosts.len()
            || moved
                .keys()
                .any(|line| !self.reminder_ghosts.contains_key(line))
        {
            self.reminder_ghosts = moved;
            self.reminders_generation = self.reminders_generation.wrapping_add(1);
        }
        self.history
            .record_marks(crate::reminders::reminder_marks_of(&self.reminder_ghosts));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc_upkeep::CalcProvider;
    use editor_core::history::{policy::UndoSession, LineHistory};
    use std::time::Duration;
    fn setup() -> (Document, NoteSession, EditContext) {
        let doc = Document {
            lines: vec!["éx".into(), "second".into()],
            ..Default::default()
        };
        let session = NoteSession::new(
            LineHistory::new(32, &doc.lines, 0, 0, Default::default()),
            Default::default(),
            Default::default(),
        );
        let ctx = EditContext {
            folds: None,
            grouping: UndoGrouping {
                session: UndoSession::Command,
                elapsed: Duration::from_secs(1),
            },
        };
        (doc, session, ctx)
    }
    #[test]
    fn boundary_noop_preserves_identity_and_history() {
        let (mut doc, mut session, ctx) = setup();
        doc.joined_text_cache = Some("éx\nsecond".into());
        assert!(session
            .apply(
                &mut doc,
                SessionEdit::Primitive(PrimitiveEdit::Backspace),
                ctx
            )
            .is_none());
        assert_eq!(session.edit_seq, 0);
        assert_eq!(doc.text_generation, 0);
        assert!(!session.dirty);
        assert!(doc.joined_text_cache.is_some());
        assert_eq!(session.history.undo_depth(), 0);
    }
    #[test]
    fn direct_unicode_replacement_records_text_and_preserves_requested_cursor() {
        let (mut doc, mut session, ctx) = setup();
        session
            .apply(
                &mut doc,
                SessionEdit::LineReplace {
                    line: 0,
                    range: 0..1,
                    text: "λ",
                    preserve_cursor: true,
                    cursor_after: None,
                },
                ctx,
            )
            .unwrap();
        assert_eq!(doc.lines[0], "λx");
        assert_eq!(doc.cursor_col, 0);
        assert_eq!(session.edit_seq, 1);
        assert_eq!(doc.text_generation, 1);
        session.history.undo(&mut doc.lines).unwrap();
        assert_eq!(doc.lines[0], "éx");
    }
    #[derive(Default)]
    struct TestCalcProvider {
        index: std::sync::Arc<std::sync::Mutex<app_core::cross_note::CrossNoteVarIndex>>,
        calls: std::cell::Cell<usize>,
    }
    impl crate::calc_upkeep::CalcProvider for TestCalcProvider {
        fn inputs(
            &self,
            _: &crate::calc::CalcState,
        ) -> crate::calc_recompute::CalcRecomputeInputs<'_> {
            self.calls.set(self.calls.get() + 1);
            crate::calc_recompute::CalcRecomputeInputs {
                base: crate::calc::CalcInputs {
                    mask: Default::default(),
                    math_enabled: true,
                    viewport_only: false,
                },
                variables_enabled: true,
                cross_note_enabled: false,
                table_enabled: true,
                note_id: "test",
                index: &self.index,
                selection_range: None,
                thresholds: crate::calc_recompute::CalcThresholds {
                    calc_pathological_window_min_lines: 2000,
                    calc_pathological_window_percent: 85,
                    calc_pathological_window_streak_threshold: 3,
                    calc_forced_full_recompute_cycles: 2,
                },
            }
        }
        fn preload_refs(&self, _: &[app_core::calc::CrossNoteRef]) {
            panic!("cross-note disabled");
        }
        fn extern_vars(&self, _: &[String]) -> Vec<app_core::calc::ExternVar> {
            panic!("cross-note disabled");
        }
    }
    #[test]
    fn pruning_an_interior_continuation_records_the_final_cursor_for_redo() {
        let (_, _, ctx) = setup();
        let mut doc = Document::from_text("| a    | b     |\n| ---- | ----- |\n| base | value |\n|>     |       |\n| next | row   |");
        doc.cursor_line = 3;
        doc.cursor_col = 12;
        let history = LineHistory::new(32, doc.lines(), 3, 12, Default::default());
        let mut session = NoteSession::new(history, Default::default(), Default::default());
        session
            .apply(&mut doc, SessionEdit::PruneEmptyTableContinuation, ctx)
            .unwrap();
        let after = doc.cursor();
        assert_eq!(after.line, 2);
        assert_eq!(doc.lines().len(), 4);
        session.undo(&mut doc).unwrap();
        assert_eq!(doc.lines().len(), 5);
        session.redo(&mut doc).unwrap();
        assert_eq!(doc.cursor(), after);
    }
    #[test]
    fn table_import_keeps_calc_upkeep_in_the_transaction() {
        let (mut doc, mut session, ctx) = setup();
        session.calc.stale = true;
        let inputs = crate::calc_upkeep::CalcEditInputs {
            base: crate::calc::CalcInputs {
                mask: Default::default(),
                math_enabled: true,
                viewport_only: false,
            },
            key_in_progress: false,
            async_min_lines: usize::MAX,
            defer_min_lines: usize::MAX,
        };
        let table = vec!["| a |".into(), "| - |".into(), "| 2 + 2 |".into()];
        let provider = TestCalcProvider::default();
        session
            .apply_with_upkeep(
                &mut doc,
                SessionEdit::TableImport(&table),
                ctx,
                Some(inputs),
                Some(&provider),
            )
            .unwrap();
        assert_eq!(provider.calls.get(), 1);
        assert_eq!(session.calc.line_metadata.len(), doc.lines().len());
        assert_eq!(session.calc.results.len(), doc.lines().len());
        assert_eq!(session.history.undo_depth(), 1);
    }
    #[test]
    fn synchronous_upkeep_finishes_history_once() {
        let (_, _, ctx) = setup();
        let mut doc = Document::from_text("base := 2\nbase * 2 = 4");
        let history = LineHistory::new(32, doc.lines(), 0, 0, Default::default());
        let mut session = NoteSession::new(
            history,
            Default::default(),
            crate::calc::CalcState {
                stale: true,
                ..Default::default()
            },
        );
        let provider = TestCalcProvider::default();
        session.recompute_calc(&mut doc, provider.inputs(&session.calc), &mut |_| {
            panic!("cross-note disabled")
        });
        let inputs = crate::calc_upkeep::CalcEditInputs {
            base: crate::calc::CalcInputs {
                mask: Default::default(),
                math_enabled: true,
                viewport_only: false,
            },
            key_in_progress: false,
            async_min_lines: usize::MAX,
            defer_min_lines: usize::MAX,
        };
        session
            .apply_with_upkeep(
                &mut doc,
                SessionEdit::LineReplace {
                    line: 0,
                    range: 8..9,
                    text: "3",
                    preserve_cursor: true,
                    cursor_after: None,
                },
                ctx,
                Some(inputs),
                Some(&provider),
            )
            .unwrap();
        assert_eq!(doc.lines[1], "base * 2 = 6");
        assert_eq!(session.history.undo_depth(), 1);
        session.undo(&mut doc).unwrap();
        assert_eq!(doc.lines, vec!["base := 2", "base * 2 = 4"]);
    }

    #[test]
    fn synchronous_edit_with_math_disabled_clears_results_without_evaluation() {
        let (mut doc, mut session, ctx) = setup();
        session.calc.stale = true;
        let inputs = crate::calc_upkeep::CalcEditInputs {
            base: crate::calc::CalcInputs {
                mask: Default::default(),
                math_enabled: false,
                viewport_only: false,
            },
            key_in_progress: false,
            async_min_lines: usize::MAX,
            defer_min_lines: usize::MAX,
        };
        session
            .apply_with_upkeep(
                &mut doc,
                SessionEdit::Primitive(PrimitiveEdit::InsertChar('x')),
                ctx,
                Some(inputs),
                None,
            )
            .unwrap();
        assert!(!session.calc.stale);
        assert!(session.calc.results.iter().all(Option::is_none));
        assert!(session.calc.line_metadata.is_empty());
    }

    #[test]
    fn fold_policy_uses_line_count_after_the_edit_in_both_directions() {
        let (mut doc, mut session, mut ctx) = setup();
        session.folds.recompute(&doc);
        ctx.folds = Some(crate::folds::FoldInputs {
            full_feature_line_limit: doc.lines().len(),
            has_collapsed: false,
            view_line_count: doc.lines().len(),
        });
        let inserted = session
            .apply(
                &mut doc,
                SessionEdit::Primitive(PrimitiveEdit::Newline),
                ctx,
            )
            .unwrap();
        assert!(inserted.fold_effect.clear_collapsed);
        assert!(!session.folds.analysis_ready);
        let removed = session
            .apply(&mut doc, SessionEdit::RemoveLines { start: 0, end: 1 }, ctx)
            .unwrap();
        assert!(!removed.fold_effect.clear_collapsed);
        assert!(session.folds.analysis_ready);
        assert_eq!(session.folds.line_text_snapshot, doc.lines());
    }

    #[test]
    fn split_and_whole_line_removal_map_and_restore_reminders() {
        let (mut doc, mut session, ctx) = setup();
        session.reminder_ghosts.insert(
            1,
            crate::LineReminderGhost {
                remind_at_ms: 1,
                display_at: "later".into(),
                line_text: "second".into(),
                reminded_at_ms: None,
            },
        );
        session
            .history
            .set_marks(crate::reminders::reminder_marks_of(
                &session.reminder_ghosts,
            ));
        session
            .apply(
                &mut doc,
                SessionEdit::Primitive(PrimitiveEdit::Newline),
                ctx,
            )
            .unwrap();
        assert!(session.reminder_ghosts.contains_key(&2));
        session
            .apply(&mut doc, SessionEdit::RemoveLines { start: 2, end: 3 }, ctx)
            .unwrap();
        assert!(session.reminder_ghosts.is_empty());
        session.undo(&mut doc).unwrap();
        assert_eq!(session.history.current_marks()[0].0, 2);
        assert!(session.reminder_ghosts.contains_key(&2));
        session.redo(&mut doc).unwrap();
        assert!(session.reminder_ghosts.is_empty());
    }
}
