use crate::{Document, NoteSession};
use editor_core::buffer::EditDelta;
use editor_core::calc_plan::{AfterEditFlags, CalcAfterEdit, CalcSignalFlags};
#[derive(Clone, Copy)]
pub struct CalcEditInputs {
    pub base: crate::calc::CalcInputs,
    pub key_in_progress: bool,
    pub async_min_lines: usize,
    pub defer_min_lines: usize,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CalcWork {
    #[default]
    Done,
    RefreshViewport,
    AfterKey,
    Idle,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct CalcEffect {
    pub work: CalcWork,
    pub index_sync_pending: bool,
}
/// Borrowed IO sources for calc; implementations cannot mutate session text.
/// Lookups are called only on paths that actually evaluate cross-note refs.
pub trait CalcProvider {
    fn inputs(
        &self,
        calc: &crate::calc::CalcState,
    ) -> crate::calc_recompute::CalcRecomputeInputs<'_>;
    fn preload_refs(&self, refs: &[app_core::calc::CrossNoteRef]);
    fn extern_vars(&self, lines: &[String]) -> Vec<app_core::calc::ExternVar>;
}
impl crate::calc::CalcState {
    /// Reset derived math state when the note's math module is disabled.
    pub fn clear(&mut self, doc: &Document) {
        self.results = vec![None; doc.lines().len()];
        self.cell_results = vec![Vec::new(); doc.lines().len()];
        self.variable_names.clear();
        self.calc_dependency_index = None;
        self.line_metadata.clear();
        self.prev_line_metadata.clear();
        self.stale = false;
        self.pathological_window_streak = 0;
        self.forced_full_recompute_remaining = 0;
    }
    pub fn defer_calc_state_after_edit(&mut self, doc: &crate::Document) {
        // Large docs without explicit calc syntax should not recompute calc
        // state on every keystroke.
        // Clear the full cache so same-line-count multi-line edits cannot
        // leave stale calc ghosts on non-cursor lines.
        if self.results.len() != doc.lines().len() {
            self.results = vec![None; doc.lines().len()];
        } else {
            self.results.fill(None);
        }
        if self.cell_results.len() != doc.lines().len() {
            self.cell_results = vec![Vec::new(); doc.lines().len()];
        } else {
            for row in &mut self.cell_results {
                row.clear();
            }
        }
        self.variable_names.clear();
        self.stale = true;
    }
}
impl NoteSession {
    pub(crate) fn calc_after_edit(
        &mut self,
        doc: &mut Document,
        delta: Option<EditDelta>,
        compound: bool,
        inputs: CalcEditInputs,
        provider: Option<&dyn CalcProvider>,
    ) -> CalcEffect {
        let mut effect = CalcEffect::default();
        if compound {
            self.calc.rebuild_calc_line_metadata(doc, inputs.base);
        } else if let Some(delta) = delta {
            effect.index_sync_pending |= if delta.old_span == delta.new_span {
                let mut pending = false;
                for line in delta.start_line..delta.start_line + delta.new_span {
                    pending |= self
                        .calc
                        .refresh_calc_line_metadata_at(doc, inputs.base, line);
                }
                pending
            } else {
                self.calc.splice_calc_line_metadata(
                    doc,
                    inputs.base,
                    delta.start_line,
                    delta.old_span,
                    delta.new_span,
                )
            };
        }
        self.calc.update_calc_flags_incremental(doc, inputs.base);
        let plan = editor_core::calc_plan::plan_after_edit(AfterEditFlags {
            line_count: doc.lines().len(),
            previous_line_count: self.calc.results.len(),
            signals: CalcSignalFlags {
                has_builtin_formula: self.calc.cached_has_builtin_formula,
                has_variable_assignment: inputs.base.math_enabled
                    && inputs.base.mask.variables_enabled
                    && self.calc.cached_has_variable_assignment,
                has_expression: self.calc.cached_has_expression,
            },
            stale: self.calc.stale,
            viewport_only: inputs.base.viewport_only,
            async_min_lines: inputs.async_min_lines,
            defer_min_lines: inputs.defer_min_lines,
        });
        match plan {
            CalcAfterEdit::Skip => {}
            CalcAfterEdit::Defer => self.calc.defer_calc_state_after_edit(doc),
            CalcAfterEdit::ViewportRefresh => {
                let (needs_eval, pending) =
                    self.calc.refresh_viewport_calc_after_edit(doc, inputs.base);
                effect.index_sync_pending |= pending;
                if needs_eval {
                    effect.work = CalcWork::RefreshViewport;
                }
            }
            CalcAfterEdit::RemapOrRecompute | CalcAfterEdit::RecomputeRange => {
                let remapped = plan == CalcAfterEdit::RemapOrRecompute
                    && self
                        .calc
                        .try_remap_calc_results_after_structural_edit(doc, inputs.base);
                if !remapped {
                    if inputs.key_in_progress {
                        self.calc.ensure_calc_line_metadata(doc, inputs.base);
                        if let Some(delta) = delta {
                            if delta.old_span == delta.new_span {
                                for line in delta.start_line..delta.start_line + delta.new_span {
                                    effect.index_sync_pending |= self
                                        .calc
                                        .refresh_calc_line_metadata_at(doc, inputs.base, line);
                                }
                            }
                        }
                        effect.index_sync_pending |= self.calc.refresh_calc_line_metadata_at(
                            doc,
                            inputs.base,
                            doc.cursor_line,
                        );
                        effect.work = CalcWork::AfterKey;
                    } else {
                        if !inputs.base.math_enabled {
                            self.calc.clear(doc);
                            self.calc.pending_result_splices.clear();
                            return effect;
                        }
                        let provider = provider.expect("synchronous calc needs evaluation inputs");
                        let recompute_inputs = provider.inputs(&self.calc);
                        if recompute_inputs.cross_note_enabled {
                            let refs = app_core::calc::scan_cross_note_refs(doc.lines());
                            provider.preload_refs(&refs);
                        }
                        self.recompute_calc(doc, recompute_inputs, &mut |lines| {
                            provider.extern_vars(lines)
                        });
                    }
                }
            }
            CalcAfterEdit::ScheduleIdle => {
                if let Some(slot) = self.calc.results.get_mut(doc.cursor_line) {
                    *slot = None;
                }
                if let Some(slot) = self.calc.cell_results.get_mut(doc.cursor_line) {
                    slot.clear();
                }
                let (start, span) = match delta {
                    Some(delta) if delta.old_span == delta.new_span => {
                        (delta.start_line, delta.new_span)
                    }
                    _ => (doc.cursor_line, 1),
                };
                if !self.calc.line_metadata.is_empty() {
                    for line in start..start + span {
                        effect.index_sync_pending |=
                            self.calc
                                .refresh_calc_line_metadata_at(doc, inputs.base, line);
                    }
                    if !(start..start + span).contains(&doc.cursor_line) {
                        effect.index_sync_pending |= self.calc.refresh_calc_line_metadata_at(
                            doc,
                            inputs.base,
                            doc.cursor_line,
                        );
                    }
                }
                effect.work = CalcWork::Idle;
            }
        }
        self.calc.pending_result_splices.clear();
        effect
    }
    /// Adopt an externally replaced snapshot; primarily used by host fixtures.
    /// Live text edits should use apply_with_upkeep instead.
    pub fn record_external_edit(
        &mut self,
        doc: &mut Document,
        ctx: crate::EditContext,
        delta: Option<EditDelta>,
        inputs: CalcEditInputs,
        provider: Option<&dyn CalcProvider>,
    ) -> CalcEffect {
        doc.joined_text_cache = None;
        doc.text_generation = doc.text_generation.wrapping_add(1);
        self.dirty = true;
        self.note_changed();
        let effect = self.calc_after_edit(doc, delta, false, inputs, provider);
        self.finish_edit(doc, ctx, delta);
        effect
    }
}
