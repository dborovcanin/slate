use crate::calc_eval::{
    compute_calc_data, compute_calc_data_for_lines, compute_calc_data_for_note,
};
use app_core::calc::ExternVar;

#[derive(Clone, Copy)]
pub struct CalcThresholds {
    pub calc_pathological_window_min_lines: usize,
    pub calc_pathological_window_percent: usize,
    pub calc_pathological_window_streak_threshold: usize,
    pub calc_forced_full_recompute_cycles: usize,
}
pub struct CalcRecomputeInputs<'a> {
    pub base: crate::calc::CalcInputs,
    pub variables_enabled: bool,
    pub cross_note_enabled: bool,
    pub table_enabled: bool,
    pub note_id: &'a str,
    pub index: &'a std::sync::Arc<std::sync::Mutex<app_core::cross_note::CrossNoteVarIndex>>,
    pub selection_range: Option<(usize, usize)>,
    pub thresholds: CalcThresholds,
}
#[derive(Clone, Copy)]
pub enum CalcRecompute {
    Disabled,
    StaleFull,
    Incremental,
}
impl crate::NoteSession {
    pub fn recompute_calc(
        &mut self,
        doc: &mut crate::Document,
        inputs: CalcRecomputeInputs<'_>,
        extern_vars: &mut dyn FnMut(&[String]) -> Vec<ExternVar>,
    ) -> CalcRecompute {
        if !inputs.base.math_enabled {
            self.calc.clear(doc);
            self.calc.range_context = Default::default();
            self.calc.cross_note_refs_scan = None;
            self.calc.cross_note_refs_generation = None;
            self.calc.pending_result_splices.clear();
            return CalcRecompute::Disabled;
        }
        self.calc.ensure_calc_line_metadata(doc, inputs.base);
        if !doc.lines().is_empty() {
            let cursor_line = doc.cursor_line.min(doc.lines().len().saturating_sub(1));
            self.calc
                .refresh_calc_line_metadata_at(doc, inputs.base, cursor_line);
        }
        let calc_variables_enabled = inputs.variables_enabled;
        let calc_cross_note_enabled = inputs.cross_note_enabled;
        let calc_table_enabled = inputs.table_enabled;

        if self.calc.stale {
            let note_id = inputs.note_id.to_owned();
            let calc_data = compute_calc_data_for_note(
                &self.calc.engine,
                doc.lines(),
                calc_variables_enabled,
                calc_cross_note_enabled,
                calc_table_enabled,
                &note_id,
                inputs.index,
            );
            let calc_mask = inputs.base.mask;
            self.calc.calc_dependency_index =
                editor_core::calc_plan::build_calc_dependency_index(doc.lines(), calc_mask);
            self.calc.prev_line_metadata = self.calc.line_metadata.clone();
            self.calc.results = calc_data.line_results;
            self.calc.cell_results = calc_data.cell_results;
            let variable_names = editor_core::calc_plan::variable_names_from_calc_dependency_index(
                self.calc.calc_dependency_index.as_ref(),
            );
            self.calc.variable_names.set(if variable_names.is_empty() {
                calc_data.variable_names
            } else {
                variable_names
            });
            self.calc.pathological_window_streak = 0;
            self.calc.forced_full_recompute_remaining = 0;
            self.calc.stale = false;
            return CalcRecompute::StaleFull;
        }

        // Snapshot extern vars once for the whole incremental recompute.
        // Update deps from a live scan so refs added since the last full eval are picked up.
        let incremental_extern_vars = if calc_cross_note_enabled {
            extern_vars(doc.lines())
        } else {
            Vec::new()
        };

        let plan = editor_core::calc_plan::plan_incremental_calc_from_line_metadata(
            &self.calc.prev_line_metadata,
            &self.calc.results,
            doc.lines(),
            &self.calc.line_metadata,
        );
        let has_prev = !self.calc.prev_line_metadata.is_empty();

        // Only scan the changed region for variable assignments and builtin
        // formulas (not all lines). Partial eval is safe as long as the edit
        // doesn't touch a formula/assignment — whole-doc presence of formulas
        // elsewhere doesn't force recomputation of unchanged lines.
        let suffix_len = doc.lines().len().saturating_sub(plan.eval_to);
        let prev_changed_from = plan.eval_from.min(self.calc.prev_line_metadata.len());
        let prev_changed_to = self
            .calc
            .prev_line_metadata
            .len()
            .saturating_sub(suffix_len)
            .max(prev_changed_from);
        let prev_changed_slice = self
            .calc
            .prev_line_metadata
            .get(prev_changed_from..prev_changed_to)
            .unwrap_or(&[]);
        let prev_changed_had_assignment = prev_changed_slice.iter().any(|meta| meta.has_assignment);
        let prev_changed_assignment_names = prev_changed_slice
            .iter()
            .filter_map(|meta| meta.assignment_name.clone())
            .collect::<Vec<_>>();
        let prev_changed_had_builtin_formula = prev_changed_slice
            .iter()
            .any(|meta| meta.has_builtin_formula);
        let calc_mask = inputs.base.mask;
        // Where the changed lines sat in their table before this edit.
        let table_rows_before = editor_core::calc_plan::table_rows_placement(
            self.calc.calc_dependency_index.as_ref(),
            plan.eval_from,
            plan.eval_to,
        );
        editor_core::calc_plan::sync_calc_dependency_index(
            &mut self.calc.calc_dependency_index,
            doc.lines(),
            plan.eval_from,
            plan.eval_to,
            calc_mask,
        );
        let eval_window = editor_core::calc_plan::decide_eval_window(
            &editor_core::calc_plan::DecideEvalWindowParams {
                lines: doc.lines(),
                changed_from: plan.eval_from,
                changed_to: plan.eval_to,
                has_prev,
                mask: calc_mask,
                prev_changed_assignment_names: &prev_changed_assignment_names,
                prev_changed_had_assignment,
                prev_changed_had_builtin_formula,
                variable_graph: None,
                table_formula_index: None,
            }
            .with_calc_dependency_index(self.calc.calc_dependency_index.as_ref()),
        );
        let mut can_use_partial = eval_window.can_use_partial;
        let mut eval_from = eval_window.eval_from;
        let mut eval_to = eval_window.eval_to;

        let line_count = doc.lines().len();
        let eval_span = eval_to.saturating_sub(eval_from);
        let is_pathological_window = can_use_partial
            && line_count >= inputs.thresholds.calc_pathological_window_min_lines
            && eval_span.saturating_mul(100)
                >= line_count.saturating_mul(inputs.thresholds.calc_pathological_window_percent);
        if is_pathological_window {
            self.calc.pathological_window_streak =
                self.calc.pathological_window_streak.saturating_add(1);
        } else {
            self.calc.pathological_window_streak = 0;
        }

        let mut force_full_now = false;
        if self.calc.forced_full_recompute_remaining > 0 {
            self.calc.forced_full_recompute_remaining -= 1;
            force_full_now = true;
        }
        if self.calc.pathological_window_streak
            >= inputs.thresholds.calc_pathological_window_streak_threshold
        {
            self.calc.pathological_window_streak = 0;
            self.calc.forced_full_recompute_remaining =
                inputs.thresholds.calc_forced_full_recompute_cycles;
            force_full_now = true;
        }
        if force_full_now {
            can_use_partial = false;
            eval_from = 0;
            eval_to = line_count;
        }

        let prev_results = std::mem::take(&mut self.calc.results);
        let prev_results_snapshot = prev_results.clone();
        let mut prev_cell_results = std::mem::take(&mut self.calc.cell_results);
        let same_shape_cache =
            prev_results.len() == doc.lines().len() && prev_cell_results.len() == doc.lines().len();

        // An in-place edit of table rows evaluates just the formula lines it
        // reaches; the table's other formula cells keep their last values.
        let table_eval_set = match &table_rows_before {
            Some(before)
                if can_use_partial
                    && same_shape_cache
                    && !force_full_now
                    && !inputs.base.viewport_only
                    && !eval_window.touches_any_assignment =>
            {
                editor_core::calc_plan::table_formula_eval_set(
                    self.calc.calc_dependency_index.as_ref(),
                    plan.eval_from,
                    plan.eval_to,
                    before,
                )
            }
            _ => None,
        };

        let (mut new_results, mut new_cell_results) = if let Some(set) = table_eval_set {
            let mut merged_results = prev_results;
            let mut merged_cells = prev_cell_results;
            let seeds = set
                .resting_cells
                .iter()
                .map(|&(line, cell)| {
                    let value = merged_cells
                        .get(line)
                        .and_then(|cells| cells.iter().find(|entry| entry.cell_index == cell))
                        .map(|entry| entry.value.clone())
                        .unwrap_or_default();
                    ((line, cell), value)
                })
                .collect();
            let calc_data = compute_calc_data_for_lines(
                &self.calc.engine,
                doc.lines(),
                app_core::calc::NoteEvaluationOptions {
                    variables_enabled: calc_variables_enabled,
                    cross_note_enabled: calc_cross_note_enabled,
                    table_enabled: calc_table_enabled,
                    extern_vars: incremental_extern_vars.clone(),
                    ..Default::default()
                },
                set.lines.clone(),
                seeds,
            );
            for idx in set.lines {
                if let Some(slot) = merged_results.get_mut(idx) {
                    *slot = calc_data.line_result(idx);
                }
                if let Some(slot) = merged_cells.get_mut(idx) {
                    *slot = calc_data.cell_result(idx);
                }
            }
            (merged_results, merged_cells)
        } else if can_use_partial && same_shape_cache {
            let mut merged_results = prev_results;
            let mut merged_cells = prev_cell_results;
            if eval_from < eval_to {
                let calc_data = compute_calc_data(
                    &self.calc.engine,
                    doc.lines(),
                    calc_variables_enabled,
                    calc_cross_note_enabled,
                    calc_table_enabled,
                    Some((eval_from, eval_to)),
                    incremental_extern_vars.clone(),
                );
                for idx in eval_from..eval_to {
                    if let Some(slot) = merged_results.get_mut(idx) {
                        *slot = calc_data.line_result(idx);
                    }
                    if let Some(slot) = merged_cells.get_mut(idx) {
                        *slot = calc_data.cell_result(idx);
                    }
                }
            }
            (merged_results, merged_cells)
        } else if can_use_partial {
            let mut merged_results = vec![None; doc.lines().len()];
            for entry in &plan.base_results {
                if let Some(slot) = merged_results.get_mut(entry.line_idx) {
                    *slot = Some(entry.result.clone());
                }
            }
            // Lines before the change keep their index; lines after it moved
            // by the change in line count.
            let line_count = doc.lines().len();
            let mut merged_cells: Vec<Vec<app_core::calc::TableCellEvaluation>> =
                vec![Vec::new(); line_count];
            let suffix_len = line_count.saturating_sub(plan.eval_to);
            let suffix_len = if prev_cell_results.len() >= suffix_len {
                suffix_len
            } else {
                0
            };
            let prev_suffix_start = prev_cell_results.len() - suffix_len;
            let unchanged = (0..plan.eval_from.min(prev_cell_results.len()))
                .map(|idx| (idx, idx))
                .chain((0..suffix_len).map(|i| (plan.eval_to + i, prev_suffix_start + i)));
            for (idx, prev_idx) in unchanged {
                if let (Some(slot), Some(cached)) = (
                    merged_cells.get_mut(idx),
                    prev_cell_results.get_mut(prev_idx),
                ) {
                    *slot = std::mem::take(cached);
                }
            }
            if eval_from < eval_to {
                let calc_data = compute_calc_data(
                    &self.calc.engine,
                    doc.lines(),
                    calc_variables_enabled,
                    calc_cross_note_enabled,
                    calc_table_enabled,
                    Some((eval_from, eval_to)),
                    incremental_extern_vars.clone(),
                );
                for idx in eval_from..eval_to {
                    if let Some(slot) = merged_results.get_mut(idx) {
                        *slot = calc_data.line_result(idx);
                    }
                    if let Some(slot) = merged_cells.get_mut(idx) {
                        *slot = calc_data.cell_result(idx);
                    }
                }
            }
            (merged_results, merged_cells)
        } else {
            let note_id = inputs.note_id.to_owned();
            let calc_data = compute_calc_data_for_note(
                &self.calc.engine,
                doc.lines(),
                calc_variables_enabled,
                calc_cross_note_enabled,
                calc_table_enabled,
                &note_id,
                inputs.index,
            );
            (calc_data.line_results, calc_data.cell_results)
        };

        let variable_names = editor_core::calc_plan::variable_names_from_calc_dependency_index(
            self.calc.calc_dependency_index.as_ref(),
        );

        // Auto-refresh committed-style trailers. Eligibility is deliberately
        // conservative — it requires that the line is byte-identical to the
        // snapshot taken at the end of the previous recompute AND that the
        // previous recompute returned `None` for the line. A `None` result
        // from the calc engine means "the trailing ` = <literal>` already
        // matches what the left side evaluates to", so prev-None is the
        // signal that the trailer was in sync. When a subsequent recompute
        // reports `Some(new_result)` for the same untouched line, the left
        // side has drifted (typically because of an upstream variable
        // change) and we rewrite the trailer in place.
        //
        // Length mismatches (note switch, undo/redo, Enter, paste, line
        // delete) invalidate per-index alignment; we skip the pass and
        // reseed the snapshot below, so eligibility returns on the next
        // recompute once the user resumes normal in-line editing.
        let aligned = self.calc.prev_line_metadata.len() == doc.lines().len()
            && prev_results_snapshot.len() == doc.lines().len();
        let mut trailer_rewritten_lines: Vec<usize> = Vec::new();

        if aligned {
            let cursor_line = doc.cursor_line;
            let cursor_col = doc.cursor_col;
            let selection_range = inputs.selection_range;

            for i in 0..doc.lines().len() {
                let Some(new_result) = new_results[i].as_deref() else {
                    continue;
                };
                let line_is_selected = if let Some((a, b)) = selection_range {
                    a <= i && i <= b
                } else {
                    false
                };
                if !editor_core::calc_plan::should_attempt_calc_trailer_refresh(
                    self.calc.prev_line_metadata[i].hash,
                    self.calc.line_metadata[i].hash,
                    prev_results_snapshot[i].as_deref(),
                    line_is_selected,
                ) {
                    continue;
                }
                let refresh =
                    calc_trailer_refresh(&doc.lines()[i], new_result, cursor_line == i, cursor_col);
                if let Some((eq_idx, new_tail)) = refresh {
                    // Keep this rewrite inside the originating calc/edit transaction.
                    // Calling mark_edited here would recursively start calc.
                    let from_col = doc.lines()[i][..eq_idx].chars().count();
                    let to_col = doc.lines()[i].chars().count();
                    if let Some(plan) = editor_core::buffer::replace::prepare_line_replace(
                        doc.lines(),
                        i,
                        from_col..to_col,
                        &new_tail,
                    ) {
                        editor_core::buffer::replace::apply_line_replace(&mut doc.lines, plan);
                    }
                    // Line is back in sync with the backend, reflect it in
                    // the cached result so the ghost widget disappears and
                    // the next eligibility round still sees prev-None here.
                    new_results[i] = None;
                    if let Some(slot) = new_cell_results.get_mut(i) {
                        slot.clear();
                    }
                    // Trailer rewrite changed the line bytes; rehash so the
                    // snapshot stays in sync for the next recompute.
                    self.calc.line_metadata[i] = editor_core::calc_plan::line_metadata_with_mask(
                        &doc.lines()[i],
                        inputs.base.mask,
                    );
                    trailer_rewritten_lines.push(i);
                }
            }
        }

        // Sync only the changed window into prev_line_metadata. prev_line_metadata tracks
        // the hash state that `results` was computed against; the unchanged prefix and suffix
        // are already correct, so only the eval window needs to be brought forward.
        let prev_len = self.calc.prev_line_metadata.len();
        let next_len = self.calc.line_metadata.len();
        let suffix_len = next_len.saturating_sub(plan.eval_to.min(next_len));
        let changed_from = plan.eval_from.min(prev_len).min(next_len);
        let prev_changed_to = prev_len.saturating_sub(suffix_len).max(changed_from);
        let next_changed_to = next_len.saturating_sub(suffix_len).max(changed_from);
        let replacement = self
            .calc
            .line_metadata
            .get(changed_from..next_changed_to)
            .unwrap_or(&[])
            .to_vec();
        self.calc
            .prev_line_metadata
            .splice(changed_from..prev_changed_to, replacement);
        let had_trailer_rewrites = !trailer_rewritten_lines.is_empty();
        for line_idx in trailer_rewritten_lines {
            if line_idx < self.calc.prev_line_metadata.len()
                && line_idx < self.calc.line_metadata.len()
            {
                self.calc.prev_line_metadata[line_idx] = self.calc.line_metadata[line_idx].clone();
            }
        }
        if had_trailer_rewrites {
            doc.joined_text_cache = None;
            doc.text_generation = doc.text_generation.wrapping_add(1);
        }
        self.calc.results = new_results;
        self.calc.cell_results = new_cell_results;
        self.calc.variable_names.set(variable_names);
        self.calc.stale = false;
        CalcRecompute::Incremental
    }
}
fn calc_trailer_refresh(
    line: &str,
    result: &str,
    cursor_line: bool,
    cursor_col: usize,
) -> Option<(usize, String)> {
    let refresh = editor_core::calc_plan::compute_calc_trailer_refresh(
        line,
        result,
        cursor_line,
        cursor_col,
    )?;
    Some((refresh.eq_byte_idx, refresh.new_tail))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    fn inputs<'a>(
        index: &'a Arc<Mutex<app_core::cross_note::CrossNoteVarIndex>>,
        selection_range: Option<(usize, usize)>,
    ) -> CalcRecomputeInputs<'a> {
        CalcRecomputeInputs {
            base: crate::calc::CalcInputs {
                mask: Default::default(),
                math_enabled: true,
                viewport_only: false,
            },
            variables_enabled: true,
            cross_note_enabled: false,
            table_enabled: true,
            note_id: "0123456789abcdef",
            index,
            selection_range,
            thresholds: CalcThresholds {
                calc_pathological_window_min_lines: 2000,
                calc_pathological_window_percent: 85,
                calc_pathological_window_streak_threshold: 3,
                calc_forced_full_recompute_cycles: 2,
            },
        }
    }
    #[test]
    fn trailer_upkeep_uses_shared_text_and_skips_selected_lines() {
        for selected in [false, true] {
            let mut doc = crate::Document::from_text("base := 2\nbase * 2 = 4");
            let history =
                editor_core::history::LineHistory::new(32, doc.lines(), 0, 0, Default::default());
            let mut session = crate::NoteSession::new(
                history,
                Default::default(),
                crate::calc::CalcState {
                    stale: true,
                    ..Default::default()
                },
            );
            let index = Arc::new(Mutex::new(
                app_core::cross_note::CrossNoteVarIndex::default(),
            ));
            session.recompute_calc(&mut doc, inputs(&index, None), &mut |_| {
                panic!("disabled cross-note lookup")
            });
            doc.set_text("base := 3\nbase * 2 = 4");
            let generation = doc.text_generation;
            session.recompute_calc(
                &mut doc,
                inputs(&index, selected.then_some((1, 1))),
                &mut |_| panic!("disabled cross-note lookup"),
            );
            assert_eq!(
                doc.lines()[1],
                if selected {
                    "base * 2 = 4"
                } else {
                    "base * 2 = 6"
                }
            );
            assert_eq!(doc.text_generation, generation + u64::from(!selected));
            assert_eq!(
                session.calc.results[1].as_deref(),
                if selected { Some("6") } else { None }
            );
        }
    }
    #[test]
    fn disabled_math_clears_results_without_io_or_text_mutation() {
        let mut doc = crate::Document::from_text("base := 3\nbase * 2 = 4");
        doc.joined_text_cache = Some(doc.lines().join("\n"));
        let generation = doc.text_generation;
        let history =
            editor_core::history::LineHistory::new(32, doc.lines(), 0, 0, Default::default());
        let mut session = crate::NoteSession::new(history, Default::default(), Default::default());
        session.calc.results = vec![Some("3".into()), Some("6".into())];
        session.calc.variable_names.set(vec!["base".into()]);
        session.calc.stale = true;
        let index = Arc::new(Mutex::new(
            app_core::cross_note::CrossNoteVarIndex::default(),
        ));
        let mut options = inputs(&index, None);
        options.base.math_enabled = false;
        options.cross_note_enabled = true;
        let result = session.recompute_calc(&mut doc, options, &mut |_| {
            panic!("disabled math must not read external values")
        });
        assert!(matches!(result, CalcRecompute::Disabled));
        assert_eq!(doc.lines()[1], "base * 2 = 4");
        assert_eq!(doc.text_generation, generation);
        assert!(doc.joined_text_cache.is_some());
        assert_eq!(session.calc.results, vec![None, None]);
        assert!(session.calc.variable_names.is_empty());
        assert!(!session.calc.stale);
        assert!(session.calc.line_metadata.is_empty());
        assert!(!session.dirty);
    }
}
