use app_core::calc::CalcEngine;

/// One replacement of old lines `[start, start + old_span)` by `new_span`
/// lines, applied to a line list of length `old_len`.
#[derive(Debug, Clone, Copy)]
pub struct ResultSplice {
    pub start: usize,
    pub old_span: usize,
    pub new_span: usize,
    pub old_len: usize,
    /// The removed lines assigned a variable or held a builtin formula.
    pub removed_affects_calc: bool,
}

pub struct CalcState {
    pub engine: CalcEngine,
    pub results: Vec<Option<String>>,
    pub cell_results: Vec<Vec<app_core::calc::TableCellEvaluation>>,
    pub variable_names: crate::variables::VariableNames,
    /// Whole-note calc preparation reused across viewport range evaluations.
    pub range_context: app_core::calc::NoteContextCache,
    /// Line splices applied to `line_metadata` since `results` last matched
    /// it, so viewport notes can shift results instead of re-evaluating.
    pub pending_result_splices: Vec<ResultSplice>,
    /// Cross-note refs from the last scan and the hashes of the lines they
    /// were scanned from, so later scans only read changed lines.
    pub cross_note_refs_scan: Option<(Vec<u64>, Vec<app_core::calc::CrossNoteRef>)>,
    /// The editor's `text_generation` that `cross_note_refs_scan` matches,
    /// if known; then it is reused without rehashing the note.
    pub cross_note_refs_generation: Option<u64>,
    pub calc_dependency_index: Option<editor_core::calc_plan::CalcDependencyIndex>,
    /// Metadata for current `lines`, incrementally patched on edits.
    pub line_metadata: Vec<editor_core::calc_plan::LineMetadata>,
    /// Snapshot aligned with `results` from the last recompute.
    pub prev_line_metadata: Vec<editor_core::calc_plan::LineMetadata>,
    pub stale: bool,
    pub cached_has_builtin_formula: bool,
    /// Some line looks like a calculation (see `CalcSignalFlags::has_expression`).
    pub cached_has_expression: bool,
    pub cached_has_variable_assignment: bool,
    pub pathological_window_streak: usize,
    pub forced_full_recompute_remaining: usize,
}

impl Default for CalcState {
    fn default() -> Self {
        Self {
            engine: CalcEngine::new(),
            results: Default::default(),
            cell_results: Default::default(),
            variable_names: Default::default(),
            range_context: Default::default(),
            pending_result_splices: Default::default(),
            cross_note_refs_scan: Default::default(),
            cross_note_refs_generation: Default::default(),
            calc_dependency_index: Default::default(),
            line_metadata: Default::default(),
            prev_line_metadata: Default::default(),
            stale: Default::default(),
            cached_has_builtin_formula: Default::default(),
            cached_has_expression: Default::default(),
            cached_has_variable_assignment: Default::default(),
            pathological_window_streak: Default::default(),
            forced_full_recompute_remaining: Default::default(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct CalcInputs {
    pub mask: editor_core::calc_plan::CalcFeatureMask,
    pub math_enabled: bool,
    pub viewport_only: bool,
}
impl CalcState {
    pub fn rescan_calc_flags(&mut self, doc: &crate::Document, inputs: CalcInputs) {
        let flags =
            editor_core::calc_plan::detect_calc_signal_flags_with_mask(doc.lines(), inputs.mask);
        self.cached_has_builtin_formula = flags.has_builtin_formula;
        self.cached_has_variable_assignment = flags.has_variable_assignment;
        self.cached_has_expression = flags.has_expression;
    }
    pub fn update_calc_flags_incremental(&mut self, doc: &crate::Document, inputs: CalcInputs) {
        // Incremental signal-detection semantics live in the shared core; the
        // session owns the cached flags and result-length bookkeeping.
        let mut flags = editor_core::calc_plan::CalcSignalFlags {
            has_variable_assignment: self.cached_has_variable_assignment,
            has_builtin_formula: self.cached_has_builtin_formula,
            has_expression: self.cached_has_expression,
        };
        editor_core::calc_plan::merge_incremental_signal_flags(
            &mut flags,
            doc.lines(),
            self.results.len(),
            doc.cursor_line,
            inputs.mask,
        );
        self.cached_has_variable_assignment = flags.has_variable_assignment;
        self.cached_has_builtin_formula = flags.has_builtin_formula;
        self.cached_has_expression = flags.has_expression;
    }
    pub fn rebuild_calc_line_metadata(&mut self, doc: &crate::Document, inputs: CalcInputs) {
        self.line_metadata =
            editor_core::calc_plan::line_metadata_for_lines_with_mask(doc.lines(), inputs.mask);
    }
    pub fn ensure_calc_line_metadata(&mut self, doc: &crate::Document, inputs: CalcInputs) {
        let mask = inputs.mask;
        editor_core::calc_plan::sync_line_metadata(&mut self.line_metadata, doc.lines(), mask);
    }
    pub fn refresh_calc_line_metadata_at(
        &mut self,
        doc: &crate::Document,
        inputs: CalcInputs,
        line_idx: usize,
    ) -> bool {
        if self.line_metadata.is_empty() && doc.lines().is_empty() {
            return false;
        }
        if self.line_metadata.is_empty() && inputs.viewport_only {
            // Viewport notes build metadata off the input thread; the build
            // re-derives any line edited before it lands.
            return true;
        }
        self.ensure_calc_line_metadata(doc, inputs);
        if line_idx >= doc.lines().len() || line_idx >= self.line_metadata.len() {
            return false;
        }
        self.line_metadata[line_idx] =
            editor_core::calc_plan::line_metadata_with_mask(&doc.lines()[line_idx], inputs.mask);

        false
    }
    pub fn splice_calc_line_metadata(
        &mut self,
        doc: &crate::Document,
        inputs: CalcInputs,
        start_line: usize,
        old_line_span: usize,
        new_line_span: usize,
    ) -> bool {
        if self.line_metadata.is_empty() && doc.lines().is_empty() {
            return false;
        }
        if self.line_metadata.is_empty() && inputs.viewport_only {
            // Nothing to splice yet; the background build covers this edit.
            return true;
        }
        let old_len = self.line_metadata.len();
        if start_line + old_line_span <= old_len {
            let removed_affects_calc = self.line_metadata[start_line..start_line + old_line_span]
                .iter()
                .any(|meta| meta.has_assignment || meta.has_builtin_formula);
            self.pending_result_splices.push(ResultSplice {
                start: start_line,
                old_span: old_line_span,
                new_span: new_line_span,
                old_len,
                removed_affects_calc,
            });
        }
        // Core recomputes metadata only for the replaced lines, or returns false
        // when dimensions are inconsistent so we fall back to a full rebuild
        // rather than corrupting the cache.
        let mask = inputs.mask;
        let spliced = editor_core::calc_plan::splice_line_metadata(
            &mut self.line_metadata,
            doc.lines(),
            start_line,
            old_line_span,
            new_line_span,
            mask,
        );
        if !spliced {
            self.rebuild_calc_line_metadata(doc, inputs);
        }

        false
    }
    pub fn try_remap_calc_results_after_structural_edit(
        &mut self,
        doc: &crate::Document,
        inputs: CalcInputs,
    ) -> bool {
        if !inputs.math_enabled || self.stale {
            return false;
        }
        self.ensure_calc_line_metadata(doc, inputs);

        let prev_len = self.results.len();
        let next_len = doc.lines().len();
        if prev_len == 0
            || self.cell_results.len() != prev_len
            || self.prev_line_metadata.len() != prev_len
            || self.line_metadata.len() != next_len
        {
            return false;
        }

        let Some(plan) = editor_core::calc_plan::plan_result_remap(
            &self.prev_line_metadata,
            &self.line_metadata,
            doc.lines(),
            inputs.mask,
        ) else {
            return false;
        };
        let prefix = plan.prefix;
        let suffix = plan.suffix;

        let mut remapped_results = vec![None; next_len];
        let mut remapped_cell_results = vec![Vec::new(); next_len];

        let shared_prefix = prefix.min(prev_len).min(next_len);
        for line_idx in 0..shared_prefix {
            remapped_results[line_idx] = self.results[line_idx].clone();
            remapped_cell_results[line_idx] = self.cell_results[line_idx].clone();
        }

        let shared_suffix = suffix
            .min(prev_len.saturating_sub(shared_prefix))
            .min(next_len.saturating_sub(shared_prefix));
        for offset in 0..shared_suffix {
            let prev_idx = prev_len - shared_suffix + offset;
            let next_idx = next_len - shared_suffix + offset;
            remapped_results[next_idx] = self.results[prev_idx].clone();
            remapped_cell_results[next_idx] = self.cell_results[prev_idx].clone();
        }

        self.results = remapped_results;
        self.cell_results = remapped_cell_results;
        self.prev_line_metadata = self.line_metadata.clone();
        self.stale = false;
        true
    }
    pub fn refresh_viewport_calc_after_edit(
        &mut self,
        doc: &crate::Document,
        inputs: CalcInputs,
    ) -> (bool, bool) {
        let mut index_sync_pending = false;
        let next_len = doc.lines().len();
        let splices = std::mem::take(&mut self.pending_result_splices);
        let new_hashes = editor_core::calc_plan::hash_lines(doc.lines());
        let mut needs_eval = true;
        if !splices.is_empty() && self.cell_results.len() == self.results.len() {
            // The edit already spliced the metadata; replay it on the results.
            needs_eval = splices.len() > 1;
            for splice in &splices {
                if self.results.len() != splice.old_len {
                    needs_eval = true;
                    break;
                }
                let old_range = splice.start..splice.start + splice.old_span;
                self.results
                    .splice(old_range.clone(), vec![None; splice.new_span]);
                self.cell_results
                    .splice(old_range, vec![Vec::new(); splice.new_span]);
                let new_lines = doc
                    .lines()
                    .get(splice.start..splice.start + splice.new_span)
                    .unwrap_or(&[]);
                needs_eval |= splice.removed_affects_calc
                    || editor_core::calc_plan::lines_affect_calc(new_lines, inputs.mask);
            }
        } else if !self.line_metadata.is_empty()
            && self.line_metadata.len() == self.results.len()
            && self.cell_results.len() == self.results.len()
        {
            let old_hashes: Vec<u64> = self.line_metadata.iter().map(|m| m.hash).collect();
            match editor_core::calc_plan::changed_line_span(&old_hashes, &new_hashes) {
                None => needs_eval = false,
                Some((from, old_to, new_to)) => {
                    let removed_mattered = self.line_metadata[from..old_to]
                        .iter()
                        .any(|meta| meta.has_assignment || meta.has_builtin_formula);
                    needs_eval = removed_mattered
                        || editor_core::calc_plan::lines_affect_calc(
                            &doc.lines()[from..new_to],
                            inputs.mask,
                        );
                    let span = new_to - from;
                    self.results.splice(from..old_to, vec![None; span]);
                    self.cell_results
                        .splice(from..old_to, vec![Vec::new(); span]);
                }
            }
        }
        if self.results.len() != next_len || self.cell_results.len() != next_len {
            self.results = vec![None; next_len];
            self.cell_results = vec![Vec::new(); next_len];
            needs_eval = true;
        }

        let mask = inputs.mask;
        if self.line_metadata.is_empty() {
            // Building metadata reads every line; the idle tick does it.
            index_sync_pending = true;
        } else if self.line_metadata.len() == next_len {
            for (idx, hash) in new_hashes.iter().enumerate() {
                if self.line_metadata[idx].hash != *hash {
                    self.line_metadata[idx] =
                        editor_core::calc_plan::line_metadata_with_mask(&doc.lines()[idx], mask);
                }
            }
        } else {
            editor_core::calc_plan::sync_line_metadata(&mut self.line_metadata, doc.lines(), mask);
        }

        (needs_eval, index_sync_pending)
    }
    pub fn cross_note_refs(
        &mut self,
        doc: &crate::Document,
    ) -> (Vec<u64>, Vec<app_core::calc::CrossNoteRef>) {
        let generation = doc.text_generation;
        let (line_hashes, refs) = match self.cross_note_refs_scan.take() {
            // Unchanged text: the last scan still holds.
            Some(scan) if self.cross_note_refs_generation == Some(generation) => scan,
            Some((scanned, refs)) => {
                let line_hashes = editor_core::calc_plan::hash_lines(doc.lines());
                let refs = match editor_core::calc_plan::changed_line_span(&scanned, &line_hashes) {
                    None => refs,
                    Some(span) => splice_cross_note_refs(refs, doc.lines(), span),
                };
                (line_hashes, refs)
            }
            None => (
                editor_core::calc_plan::hash_lines(doc.lines()),
                app_core::calc::scan_cross_note_refs(doc.lines()),
            ),
        };
        (line_hashes, refs)
    }
    pub fn evaluate_range(
        &mut self,
        doc: &crate::Document,
        eval_from: usize,
        eval_to: usize,
        options: app_core::calc::NoteEvaluationOptions,
    ) {
        if eval_from >= eval_to || eval_to > doc.lines().len() {
            return;
        }
        let calc_data = crate::calc_eval::compute_calc_data_cached(
            &self.engine,
            doc.lines(),
            options.variables_enabled,
            options.cross_note_enabled,
            options.table_enabled,
            Some((eval_from, eval_to)),
            options.extern_vars,
            &mut self.range_context,
            doc.text_generation,
            // Names come from the dependency index once it exists; before
            // that, the first evaluation's names do until the idle tick.
            self.calc_dependency_index.is_some() || !self.variable_names.is_empty(),
        );
        if self.results.len() != doc.lines().len() {
            self.results = vec![None; doc.lines().len()];
        }
        if self.cell_results.len() != doc.lines().len() {
            self.cell_results = vec![Vec::new(); doc.lines().len()];
        }
        for line_idx in eval_from..eval_to {
            if let Some(slot) = self.results.get_mut(line_idx) {
                *slot = calc_data.line_result(line_idx);
            }
            if let Some(slot) = self.cell_results.get_mut(line_idx) {
                *slot = calc_data.cell_result(line_idx);
            }
        }
        if self.calc_dependency_index.is_none() && self.variable_names.is_empty() {
            // Until the idle tick builds the index, take names from the eval.
            self.variable_names.set(calc_data.variable_names);
        }
    }
}
pub fn splice_cross_note_refs(
    refs: Vec<app_core::calc::CrossNoteRef>,
    lines: &[String],
    (from, old_to, new_to): (usize, usize, usize),
) -> Vec<app_core::calc::CrossNoteRef> {
    let mut spliced = Vec::with_capacity(refs.len());
    let mut after = Vec::new();
    for reference in refs {
        let line_idx = reference.line - 1;
        if line_idx < from {
            spliced.push(reference);
        } else if line_idx >= old_to {
            after.push(app_core::calc::CrossNoteRef {
                line: line_idx - old_to + new_to + 1,
                ..reference
            });
        }
    }
    spliced.extend(
        app_core::calc::scan_cross_note_refs(&lines[from..new_to])
            .into_iter()
            .map(|reference| app_core::calc::CrossNoteRef {
                line: reference.line + from,
                ..reference
            }),
    );
    spliced.append(&mut after);
    spliced
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inputs() -> CalcInputs {
        CalcInputs {
            mask: Default::default(),
            math_enabled: true,
            viewport_only: false,
        }
    }
    #[test]
    fn range_evaluation_uses_absolute_line_slots_and_keeps_untouched_results() {
        let doc = crate::Document::from_text("2 + 2\n3 + 3\n4 + 4");
        let mut calc = CalcState {
            results: vec![Some("old".into()); 3],
            ..Default::default()
        };
        calc.evaluate_range(&doc, 1, 2, Default::default());
        assert_eq!(
            calc.results,
            vec![Some("old".into()), Some("6".into()), Some("old".into())]
        );
        calc.evaluate_range(&doc, 4, 5, Default::default());
        assert_eq!(calc.results[1].as_deref(), Some("6"));
    }
    #[test]
    fn structural_prose_remap_preserves_calc_values_in_shifted_suffix() {
        let mut doc = crate::Document::from_text("intro\n2 + 2");
        let mut calc = CalcState {
            results: vec![None, Some("4".into())],
            cell_results: vec![vec![]; 2],
            ..Default::default()
        };
        calc.rebuild_calc_line_metadata(&doc, inputs());
        calc.prev_line_metadata = calc.line_metadata.clone();
        doc.set_text("intro\nextra prose\n2 + 2");
        calc.splice_calc_line_metadata(&doc, inputs(), 1, 0, 1);
        assert!(calc.try_remap_calc_results_after_structural_edit(&doc, inputs()));
        assert_eq!(calc.results, vec![None, None, Some("4".into())]);
    }
}
