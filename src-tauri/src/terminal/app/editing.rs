use super::{
    build_variable_suggestions, compute_calc_data, compute_calc_trailer_refresh,
    contains_assignment_operator, display_cols_for_prefix, extract_variable_completion_prefix,
    find_calc_segment_range, gutter_width_for_visible_lines, line_char_len, line_display_cols,
    split_lines, table_cell_edit_start, table_cell_info_at_char, table_cell_is_empty,
    table_cell_navigation_anchor, FoldKind, TerminalApp, UiMode, VariableAutocompletePopupState,
    VariableAutocompleteState, WikiLinkAutocompletePopupState, WikiLinkSuggestion,
    CALC_ASYNC_MIN_LINES, CALC_RECOMPUTE_DEBOUNCE_MS, CALC_VIEWPORT_PREFETCH_MULTIPLIER,
    EDITOR_TOP_ROW, FENCE_CHECKPOINT_INTERVAL, HORIZONTAL_SCROLL_LEFT_CONTEXT,
    LARGE_DOC_CALC_DEFER_LINES, UNDO_DEBOUNCE_MS, VARIABLE_AUTOCOMPLETE_MAX_SUGGESTIONS,
};
use crate::terminal::text_utils::{
    byte_index, cursor_render_char_col, join_lines, remove_char_at, viewport_col_for_display_col,
};
use crate::terminal::{folding, input};
use std::cmp::min;
use std::time::{Duration, Instant};

fn map_offset_through_changes(
    mut offset: usize,
    changes_desc: &[crate::editor_core::types::TextChange],
) -> usize {
    for change in changes_desc {
        let from = change.from;
        let to = change.to.max(from);
        let added = change.insert.len();
        let removed = to.saturating_sub(from);
        if from <= offset {
            if to <= offset {
                offset = offset.saturating_add(added).saturating_sub(removed);
            } else {
                let inside = offset.saturating_sub(from);
                offset = from.saturating_add(inside.min(added));
            }
        }
    }
    offset
}

// Ownership: editor mutations, cursor movement, folding, and calc state updates.
impl TerminalApp {
    pub(super) fn bootstrap_folding_for_startup(&mut self) {
        // Keep startup cheap for very large notes: build a plain 1:1 visible
        // map and defer expensive fold structure analysis until needed.
        self.folds.ranges.clear();
        self.folds.range_by_start = vec![None; self.lines.len()];
        self.folds.collapsed_starts.clear();
        self.folds.line_has_structure = vec![false; self.lines.len()];
        // Preserve length invariants expected by incremental fold remap logic
        // without cloning full line content at startup.
        self.folds.line_text_snapshot = vec![String::new(); self.lines.len()];
        self.folds.rescan_pending = false;
        self.folds.analysis_ready = false;
        self.rebuild_fold_view_map();
    }

    fn ensure_fold_analysis_ready_for_command(&mut self) {
        if !self.folds.analysis_ready
            || self.folds.rescan_pending
            || self.folds.range_by_start.len() != self.lines.len()
            || self.folds.line_has_structure.len() != self.lines.len()
            || self.folds.line_text_snapshot.len() != self.lines.len()
        {
            self.recompute_folding();
        }
    }

    pub(super) fn current_line(&self) -> &str {
        self.lines
            .get(self.cursor_line)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    pub(super) fn current_line_mut(&mut self) -> &mut String {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        &mut self.lines[self.cursor_line]
    }

    pub(super) fn rescan_calc_flags(&mut self) {
        let flags = crate::editor_core::calc_plan::detect_calc_signal_flags_with_mask(
            &self.lines,
            self.calc_feature_mask(),
        );
        self.calc.cached_has_builtin_formula = flags.has_builtin_formula;
        self.calc.cached_has_variable_assignment = flags.has_variable_assignment;
    }

    pub(super) fn update_calc_flags_incremental(&mut self) {
        if self.lines.len() != self.calc.results.len() {
            // Line count changed (Enter, delete-at-boundary).
            // Flags can only go false → true here, never true → false.
            // On delete: the removed line may have been the only one with the
            // syntax, but accepting stale-true is safe — it just means we run
            // calc when not needed, which is correct.
            // On insert: check only the two affected lines (cursor and cursor-1).
            if self.lines.len() > self.calc.results.len() {
                let cl = self.cursor_line.min(self.lines.len().saturating_sub(1));
                for i in cl.saturating_sub(1)..=cl {
                    if let Some(text) = self.lines.get(i) {
                        let mask = self.calc_feature_mask();
                        if !self.calc.cached_has_variable_assignment
                            && crate::editor_core::calc_plan::contains_variable_assignment_with_mask(
                                std::slice::from_ref(text),
                                mask,
                            )
                        {
                            self.calc.cached_has_variable_assignment = true;
                        }
                        if !self.calc.cached_has_builtin_formula
                            && crate::editor_core::calc_plan::contains_builtin_formula_with_mask(
                                std::slice::from_ref(text),
                                mask,
                            )
                        {
                            self.calc.cached_has_builtin_formula = true;
                        }
                    }
                }
            }
            // Delete: accept stale-true; flags reset only via rescan_calc_flags
            // (called on note switch and explicit rescans).
            return;
        }
        // Same-line edit: check only the cursor line for new signals.
        let line = self.cursor_line;
        if !self.calc.cached_has_variable_assignment {
            if let Some(text) = self.lines.get(line) {
                if crate::editor_core::calc_plan::contains_variable_assignment_with_mask(
                    std::slice::from_ref(text),
                    self.calc_feature_mask(),
                ) {
                    self.calc.cached_has_variable_assignment = true;
                }
            }
        }
        if !self.calc.cached_has_builtin_formula {
            if let Some(text) = self.lines.get(line) {
                if crate::editor_core::calc_plan::contains_builtin_formula_with_mask(
                    std::slice::from_ref(text),
                    self.calc_feature_mask(),
                ) {
                    self.calc.cached_has_builtin_formula = true;
                }
            }
        }
    }

    fn rebuild_calc_line_metadata(&mut self) {
        self.calc.line_metadata = crate::editor_core::calc_plan::line_metadata_for_lines_with_mask(
            &self.lines,
            self.calc_feature_mask(),
        );
    }

    fn ensure_calc_line_metadata(&mut self) {
        if self.calc.line_metadata.len() != self.lines.len() {
            self.rebuild_calc_line_metadata();
        }
    }

    fn refresh_calc_line_metadata_at(&mut self, line_idx: usize) {
        if self.calc.line_metadata.is_empty() && self.lines.is_empty() {
            return;
        }
        self.ensure_calc_line_metadata();
        if line_idx >= self.lines.len() || line_idx >= self.calc.line_metadata.len() {
            return;
        }
        self.calc.line_metadata[line_idx] = crate::editor_core::calc_plan::line_metadata_with_mask(
            &self.lines[line_idx],
            self.calc_feature_mask(),
        );
    }

    fn splice_calc_line_metadata(
        &mut self,
        start_line: usize,
        old_line_span: usize,
        new_line_span: usize,
    ) {
        if self.calc.line_metadata.is_empty() && self.lines.is_empty() {
            return;
        }
        // If dimensions are inconsistent, fall back to a full rebuild rather than corrupting state.
        let expected_prev_len = self
            .lines
            .len()
            .saturating_add(old_line_span)
            .saturating_sub(new_line_span);
        if self.calc.line_metadata.len() != expected_prev_len {
            self.rebuild_calc_line_metadata();
            return;
        }
        let start = start_line.min(self.calc.line_metadata.len());
        let old_end = start
            .saturating_add(old_line_span)
            .min(self.calc.line_metadata.len());
        let new_end = start_line
            .saturating_add(new_line_span)
            .min(self.lines.len());
        let replacement = self
            .lines
            .get(start_line.min(self.lines.len())..new_end)
            .unwrap_or(&[])
            .iter()
            .map(|line| {
                crate::editor_core::calc_plan::line_metadata_with_mask(
                    line,
                    self.calc_feature_mask(),
                )
            })
            .collect::<Vec<_>>();
        self.calc.line_metadata.splice(start..old_end, replacement);
    }

    pub(super) fn line_has_fold_structure(text: &str) -> bool {
        let trimmed = text.trim_start();
        trimmed.starts_with('#')
            || trimmed.starts_with("```")
            || trimmed.starts_with("~~~")
            || (trimmed.starts_with('|') && trimmed.ends_with('|'))
            || crate::editor_core::markdown_tokens::list_marker_end(text).is_some()
    }

    pub(super) fn note_math_module_enabled(&self) -> bool {
        self.active_note.modules.math
    }

    pub(super) fn note_table_module_enabled(&self) -> bool {
        self.active_note.modules.table
    }

    pub(super) fn note_variables_module_enabled(&self) -> bool {
        self.active_note.modules.variables
    }

    pub(super) fn note_style_module_enabled(&self) -> bool {
        self.active_note.modules.style
    }

    fn calc_feature_mask(&self) -> crate::editor_core::calc_plan::CalcFeatureMask {
        crate::editor_core::calc_plan::CalcFeatureMask {
            math_enabled: self.note_math_module_enabled(),
            table_enabled: self.note_table_module_enabled(),
            variables_enabled: self.note_variables_module_enabled(),
        }
    }

    pub(super) fn markdown_autoformat_enabled(&self) -> bool {
        self.markdown_autoformat && self.note_style_module_enabled()
    }

    pub(super) fn checklist_auto_reorder_enabled(&self) -> bool {
        self.checklist_auto_reorder && self.note_style_module_enabled()
    }

    pub(super) fn active_has_variable_assignments(&self) -> bool {
        self.note_math_module_enabled()
            && self.note_variables_module_enabled()
            && self.calc.cached_has_variable_assignment
    }

    pub(super) fn calc_variables_enabled(&self) -> bool {
        self.active_has_variable_assignments()
    }

    pub(super) fn should_defer_calc_recompute(&self) -> bool {
        self.lines.len() >= LARGE_DOC_CALC_DEFER_LINES
            && !self.calc.cached_has_builtin_formula
            && !self.active_has_variable_assignments()
    }

    pub(super) fn can_skip_calc_recompute(&self) -> bool {
        // If neither builtin formulas nor variable assignments exist anywhere
        // in the doc, `compute_calc_data` would produce all-None results for
        // every line — matching the current state. Safe to skip regardless of
        // doc size, which is the biggest input-latency win for notes that
        // don't use calc at all.
        !self.calc.cached_has_builtin_formula
            && !self.active_has_variable_assignments()
            && !self.calc.stale
    }

    pub(super) fn clear_calc_cache(&mut self) {
        self.calc.results = vec![None; self.lines.len()];
        self.calc.cell_results = vec![Vec::new(); self.lines.len()];
        self.calc.variable_names.clear();
        self.calc.line_metadata.clear();
        self.calc.prev_line_metadata.clear();
        self.calc.stale = false;
        self.calc_last_view_eval_range = None;
        self.calc_recompute_pending = false;
    }

    pub(super) fn defer_calc_state_after_edit(&mut self) {
        // Large docs without explicit calc syntax should not recompute calc
        // state on every keystroke.
        // Clear the full cache so same-line-count multi-line edits cannot
        // leave stale calc ghosts on non-cursor lines.
        if self.calc.results.len() != self.lines.len() {
            self.calc.results = vec![None; self.lines.len()];
        } else {
            self.calc.results.fill(None);
        }
        if self.calc.cell_results.len() != self.lines.len() {
            self.calc.cell_results = vec![Vec::new(); self.lines.len()];
        } else {
            for row in &mut self.calc.cell_results {
                row.clear();
            }
        }
        self.calc.variable_names.clear();
        self.calc.stale = true;
    }

    pub(super) fn recompute_folding_if_needed(&mut self) {
        // Process any pending deferred recompute first.
        if self.folds.rescan_pending {
            self.folds.rescan_pending = false;
            self.recompute_folding();
            return;
        }

        if self.lines.is_empty() {
            self.folds.line_has_structure.clear();
            self.folds.line_text_snapshot.clear();
            self.apply_fold_ranges(Vec::new());
            self.folds.analysis_ready = true;
            return;
        }

        let cl = self.cursor_line.min(self.lines.len().saturating_sub(1));
        let line_count_changed = self.lines.len() != self.folds.line_has_structure.len();

        if line_count_changed {
            let next_len = self.lines.len();
            let prev_len = self.folds.line_has_structure.len();
            let mut remap_edits: Vec<crate::editor_core::folding::FoldLineEdit> = Vec::new();
            if next_len == prev_len + 1 {
                // One line inserted near cursor.
                let insert_at = cl.min(prev_len);
                let old_line_text = self
                    .folds
                    .line_text_snapshot
                    .get(insert_at)
                    .cloned()
                    .unwrap_or_default();
                let new_line_text = self.lines.get(insert_at).cloned().unwrap_or_default();
                remap_edits.push(crate::editor_core::folding::FoldLineEdit {
                    old_start_line: insert_at,
                    old_line_span: 1,
                    new_line_span: 2,
                    old_line_text,
                    new_line_text: new_line_text.clone(),
                });

                if cl > 0 {
                    if let Some(flag) = self.folds.line_has_structure.get_mut(cl - 1) {
                        *flag = self
                            .lines
                            .get(cl - 1)
                            .map(|l| Self::line_has_fold_structure(l))
                            .unwrap_or(false);
                    }
                    if let Some(text) = self.folds.line_text_snapshot.get_mut(cl - 1) {
                        *text = self.lines.get(cl - 1).cloned().unwrap_or_default();
                    }
                }
                let new_flag = self
                    .lines
                    .get(insert_at)
                    .map(|l| Self::line_has_fold_structure(l))
                    .unwrap_or(false);
                self.folds.line_has_structure.insert(insert_at, new_flag);
                self.folds
                    .line_text_snapshot
                    .insert(insert_at, new_line_text);
            } else if next_len + 1 == prev_len {
                // One line deleted near cursor.
                let remove_at = cl.min(prev_len.saturating_sub(1));
                let old_start_line = remove_at.min(prev_len.saturating_sub(2));
                let old_line_text = self
                    .folds
                    .line_text_snapshot
                    .get(old_start_line)
                    .cloned()
                    .unwrap_or_default();
                let new_line_text = self.lines.get(old_start_line).cloned().unwrap_or_default();
                remap_edits.push(crate::editor_core::folding::FoldLineEdit {
                    old_start_line,
                    old_line_span: 2,
                    new_line_span: 1,
                    old_line_text,
                    new_line_text: new_line_text.clone(),
                });

                if remove_at < prev_len {
                    self.folds.line_has_structure.remove(remove_at);
                }
                if remove_at < self.folds.line_text_snapshot.len() {
                    self.folds.line_text_snapshot.remove(remove_at);
                }
                let update_at = remove_at.min(next_len.saturating_sub(1));
                if let Some(flag) = self.folds.line_has_structure.get_mut(update_at) {
                    *flag = self
                        .lines
                        .get(update_at)
                        .map(|l| Self::line_has_fold_structure(l))
                        .unwrap_or(false);
                }
                if let Some(text) = self.folds.line_text_snapshot.get_mut(update_at) {
                    *text = self.lines.get(update_at).cloned().unwrap_or_default();
                }
            } else {
                // Bulk change (paste, format, etc.): rebuild entirely.
                self.recompute_folding();
                return;
            }

            if self.try_incremental_fold_remap(&remap_edits) {
                return;
            }

            if self.folds.collapsed_starts.is_empty() {
                self.folds.ranges.clear();
                self.folds.range_by_start = vec![None; self.lines.len()];
                self.rebuild_fold_view_map();
                self.folds.rescan_pending = true;
                self.folds.analysis_ready = false;
            } else {
                self.recompute_folding();
            }
            return;
        }

        // Same-line edit: check whether the current line touches fold structure.
        let old_text = self
            .folds
            .line_text_snapshot
            .get(cl)
            .cloned()
            .unwrap_or_default();
        let current_text = self.lines.get(cl).map(|s| s.as_str()).unwrap_or("");
        let new_text = current_text.to_string();
        let next_flag = Self::line_has_fold_structure(current_text);
        let prev_flag = self
            .folds
            .line_has_structure
            .get(cl)
            .copied()
            .unwrap_or(false);

        if next_flag != prev_flag {
            if let Some(flag) = self.folds.line_has_structure.get_mut(cl) {
                *flag = next_flag;
            }
        }
        if let Some(text) = self.folds.line_text_snapshot.get_mut(cl) {
            *text = new_text.clone();
        }

        let remap_edits = [crate::editor_core::folding::FoldLineEdit {
            old_start_line: cl,
            old_line_span: 1,
            new_line_span: 1,
            old_line_text: old_text,
            new_line_text: new_text,
        }];
        if self.try_incremental_fold_remap(&remap_edits) {
            return;
        }

        if next_flag || prev_flag {
            if self.folds.collapsed_starts.is_empty() {
                // Defer: fold analysis is O(N) and doesn't need to block typing.
                // The idle tick (100 ms with no keypress) will run recompute_folding.
                self.folds.rescan_pending = true;
                self.folds.analysis_ready = false;
            } else {
                self.recompute_folding();
            }
        }
    }

    pub(super) fn recompute_folding(&mut self) {
        self.folds.line_has_structure = self
            .lines
            .iter()
            .map(|line| Self::line_has_fold_structure(line))
            .collect();
        self.folds.line_text_snapshot = self.lines.clone();
        self.recompute_folding_from_cached_structure();
    }

    pub(super) fn recompute_folding_from_cached_structure(&mut self) {
        self.folds.rescan_pending = false;
        let ranges = folding::build_fold_ranges(&self.lines);
        self.apply_fold_ranges(ranges);
        self.folds.analysis_ready = true;
    }

    fn try_incremental_fold_remap(
        &mut self,
        edits: &[crate::editor_core::folding::FoldLineEdit],
    ) -> bool {
        if edits.is_empty() {
            return false;
        }
        if crate::editor_core::folding::edits_require_rebuild(edits) {
            return false;
        }
        let mapped = crate::editor_core::folding::map_ranges_through_line_edits(
            &self.folds.ranges,
            edits,
            self.lines.len().max(1),
        );
        self.folds.rescan_pending = false;
        self.apply_fold_ranges(mapped);
        self.folds.analysis_ready = true;
        true
    }

    fn apply_fold_ranges(&mut self, ranges: Vec<crate::editor_core::folding::FoldRange>) {
        self.folds.ranges = ranges;
        self.folds.range_by_start = vec![None; self.lines.len()];
        for range in &self.folds.ranges {
            if range.start_line < self.folds.range_by_start.len() {
                self.folds.range_by_start[range.start_line] = Some(*range);
            }
        }
        self.folds.collapsed_starts.retain(|line| {
            self.folds
                .range_by_start
                .get(*line)
                .is_some_and(|entry| entry.is_some())
        });
        self.rebuild_fold_view_map();
    }

    pub(super) fn rebuild_fold_view_map(&mut self) {
        let line_count = self.lines.len();
        self.folds.visible_to_real.clear();
        self.folds.visible_to_real.reserve(line_count);
        self.folds.real_to_visible = vec![0; line_count];
        self.folds.hidden_owner = vec![None; line_count];
        self.folds.placeholder_hidden_lines = vec![None; line_count];

        if line_count == 0 {
            return;
        }

        let mut collapsed_ranges = self
            .folds
            .collapsed_starts
            .iter()
            .filter_map(|start| {
                self.folds
                    .range_by_start
                    .get(*start)
                    .and_then(|entry| *entry)
            })
            .collect::<Vec<_>>();
        collapsed_ranges.sort_by_key(|range| (range.start_line, range.end_line));

        let mut effective = Vec::new();
        let mut covered_to: Option<usize> = None;
        for range in collapsed_ranges {
            if range.end_line <= range.start_line {
                continue;
            }
            if covered_to.is_some_and(|last_end| range.start_line <= last_end) {
                continue;
            }
            covered_to = Some(range.end_line);
            effective.push(range);
        }

        let mut effective_idx = 0usize;
        let mut real_line = 0usize;
        while real_line < line_count {
            let visible_idx = self.folds.visible_to_real.len();
            self.folds.visible_to_real.push(real_line);
            self.folds.real_to_visible[real_line] = visible_idx;

            let collapse_here = effective
                .get(effective_idx)
                .copied()
                .filter(|range| range.start_line == real_line);
            if let Some(range) = collapse_here {
                let hidden_end = range.end_line.min(line_count.saturating_sub(1));
                if hidden_end > real_line {
                    self.folds.placeholder_hidden_lines[real_line] =
                        Some(hidden_end.saturating_sub(real_line));
                    for hidden_line in (real_line + 1)..=hidden_end {
                        self.folds.hidden_owner[hidden_line] = Some(real_line);
                        self.folds.real_to_visible[hidden_line] = visible_idx;
                    }
                    real_line = hidden_end + 1;
                } else {
                    real_line += 1;
                }
                effective_idx += 1;
            } else {
                real_line += 1;
            }
        }

        if self.folds.visible_to_real.is_empty() {
            self.folds.visible_to_real.push(0);
        }
    }

    // ---- Fence-state checkpoint helpers ----

    // Returns the code-fence parse state that applies BEFORE line `target_line`
    // (0-based). Uses a sparse checkpoint array so the worst-case scan is at
    // most FENCE_CHECKPOINT_INTERVAL line advances regardless of doc size.
    pub(super) fn fence_state_before_line(&mut self, target_line: usize) -> (bool, Option<String>) {
        if target_line == 0 {
            return (false, None);
        }
        let target = target_line.min(self.lines.len());
        let target_ck = target / FENCE_CHECKPOINT_INTERVAL;
        let start_ck = target_ck.min(self.fence_checkpoints_valid_through);
        let start_line = start_ck * FENCE_CHECKPOINT_INTERVAL;

        let (mut in_code_block, mut code_fence_lang) = if start_ck == 0 {
            (false, None)
        } else {
            self.fence_checkpoints
                .get(start_ck)
                .cloned()
                .unwrap_or((false, None))
        };

        let mut line_idx = start_line;
        while line_idx < target {
            // At each new checkpoint boundary, cache the current state.
            if line_idx > 0 && line_idx % FENCE_CHECKPOINT_INTERVAL == 0 {
                let ck = line_idx / FENCE_CHECKPOINT_INTERVAL;
                if ck > self.fence_checkpoints_valid_through {
                    while self.fence_checkpoints.len() <= ck {
                        self.fence_checkpoints.push((false, None));
                    }
                    self.fence_checkpoints[ck] = (in_code_block, code_fence_lang.clone());
                    self.fence_checkpoints_valid_through = ck;
                }
            }
            if let Some(line_text) = self.lines.get(line_idx) {
                let mut state = crate::editor_core::markdown_tokens::FenceState {
                    in_code_block,
                    code_fence_lang,
                };
                crate::editor_core::markdown_tokens::advance_fence_state(&mut state, line_text);
                in_code_block = state.in_code_block;
                code_fence_lang = state.code_fence_lang;
            }
            line_idx += 1;
        }
        (in_code_block, code_fence_lang)
    }

    // Invalidate all fence checkpoints that depend on content at or after
    // `line_idx`. Called whenever lines at or before a checkpoint boundary change.
    pub(super) fn invalidate_fence_checkpoints_from_line(&mut self, line_idx: usize) {
        let keep_through = line_idx / FENCE_CHECKPOINT_INTERVAL;
        if self.fence_checkpoints_valid_through > keep_through {
            self.fence_checkpoints_valid_through = keep_through;
        }
    }

    pub(super) fn visible_line_count(&self) -> usize {
        self.folds.visible_to_real.len().max(1)
    }

    pub(super) fn current_virtual_line(&self) -> usize {
        self.folds
            .real_to_visible
            .get(self.cursor_line)
            .copied()
            .unwrap_or(0)
    }

    pub(super) fn real_line_for_virtual(&self, virtual_line: usize) -> Option<usize> {
        self.folds.visible_to_real.get(virtual_line).copied()
    }

    pub(super) fn fold_hidden_owner_for_line(&self, line: usize) -> Option<usize> {
        self.folds.hidden_owner.get(line).and_then(|owner| *owner)
    }

    pub(super) fn fold_start_for_line(&self, line: usize) -> Option<usize> {
        if let Some(owner) = self.fold_hidden_owner_for_line(line) {
            return Some(owner);
        }
        if self
            .folds
            .range_by_start
            .get(line)
            .is_some_and(|entry| entry.is_some())
        {
            return Some(line);
        }

        let mut best_start = None;
        let mut best_span = usize::MAX;
        for range in &self.folds.ranges {
            if range.start_line < line && line <= range.end_line {
                let span = range.end_line.saturating_sub(range.start_line);
                if span < best_span {
                    best_span = span;
                    best_start = Some(range.start_line);
                }
            }
        }
        best_start
    }

    pub(super) fn toggle_fold_at_cursor(&mut self) -> bool {
        self.ensure_fold_analysis_ready_for_command();
        let line = self.cursor_line.min(self.lines.len().saturating_sub(1));
        let Some(start_line) = self.fold_start_for_line(line) else {
            self.status = "fold: no foldable block at cursor".to_string();
            return false;
        };
        let next_collapsed = !self.folds.collapsed_starts.contains(&start_line);
        self.set_fold_collapsed_at_line(start_line, next_collapsed)
    }

    pub(super) fn set_fold_collapsed_at_cursor(&mut self, collapsed: bool) -> bool {
        self.ensure_fold_analysis_ready_for_command();
        let line = self.cursor_line.min(self.lines.len().saturating_sub(1));
        let Some(start_line) = self.fold_start_for_line(line) else {
            self.status = "fold: no foldable block at cursor".to_string();
            return false;
        };
        self.set_fold_collapsed_at_line(start_line, collapsed)
    }

    pub(super) fn set_fold_collapsed_at_line(
        &mut self,
        start_line: usize,
        collapsed: bool,
    ) -> bool {
        let Some(range) = self
            .folds
            .range_by_start
            .get(start_line)
            .and_then(|entry| *entry)
        else {
            self.status = "fold: no foldable block at cursor".to_string();
            return false;
        };

        let was_collapsed = self.folds.collapsed_starts.contains(&start_line);
        if was_collapsed == collapsed {
            self.status = if collapsed {
                "fold: already folded".to_string()
            } else {
                "fold: already unfolded".to_string()
            };
            return false;
        }

        let action = if collapsed {
            self.folds.collapsed_starts.insert(start_line);
            "folded"
        } else {
            self.folds.collapsed_starts.remove(&start_line);
            "unfolded"
        };

        self.rebuild_fold_view_map();
        self.adjust_cursor();
        self.adjust_scroll();

        let kind = match range.kind {
            FoldKind::Heading => "heading",
            FoldKind::Fence => "code block",
            FoldKind::List => "list",
            FoldKind::Table => "table",
            FoldKind::Paragraph => "paragraph",
        };
        let hidden = range.end_line.saturating_sub(range.start_line);
        self.status = format!("fold: {action} {kind} ({hidden} lines)");
        true
    }

    pub(super) fn mark_edited_from_line(&mut self, changed_from_line: usize) {
        let coalesce_undo = self.last_edit.elapsed() < Duration::from_millis(UNDO_DEBOUNCE_MS);
        self.dirty = true;
        if !self.reminder_ghosts.is_empty() {
            self.reminders_dirty = true;
        }
        let clamped_changed_line = if self.lines.is_empty() {
            0
        } else {
            changed_from_line.min(self.lines.len().saturating_sub(1))
        };
        self.invalidate_fence_checkpoints_from_line(clamped_changed_line);
        self.update_calc_flags_incremental();
        self.recompute_folding_if_needed();
        if self.can_skip_calc_recompute() {
            // No calc syntax anywhere in the doc and this edit didn't add any —
            // calc_results are already correct (all None). Skip the scan.
            // prev_line_metadata may drift from `lines` until the next real
            // recompute, but the planner falls back to full eval safely when
            // the diff looks large, so correctness holds.
            self.calc_recompute_pending = false;
        } else if self.should_defer_calc_recompute() {
            self.defer_calc_state_after_edit();
            self.calc_recompute_pending = false;
        } else {
            if self.lines.len() >= CALC_ASYNC_MIN_LINES {
                // Keep large-note typing non-blocking: schedule calc for the
                // next idle tick and clear only the edited line's cached
                // result so we don't show stale ghosts while pending.
                if let Some(slot) = self.calc.results.get_mut(self.cursor_line) {
                    *slot = None;
                }
                if let Some(slot) = self.calc.cell_results.get_mut(self.cursor_line) {
                    slot.clear();
                }
                self.calc_recompute_pending = true;
            } else {
                self.run_calc_recompute();
            }
        }
        self.history.record_edit(
            &self.lines,
            self.cursor_line,
            self.cursor_col,
            coalesce_undo,
        );
        self.last_edit = Instant::now();
    }

    pub(super) fn mark_edited(&mut self) {
        self.mark_edited_from_line(self.cursor_line);
    }

    pub(super) fn undo(&mut self) {
        let keep_cursor_on_exhaust = self.history.undo_depth() == 1;
        let cursor_before_undo = (self.cursor_line, self.cursor_col);
        if let Some(cursor) = self.history.undo(&mut self.lines) {
            if keep_cursor_on_exhaust {
                self.cursor_line = cursor_before_undo.0.min(self.lines.len().saturating_sub(1));
                self.cursor_col = cursor_before_undo.1;
            } else {
                self.cursor_line = cursor.line.min(self.lines.len().saturating_sub(1));
                self.cursor_col = cursor.col;
            }
            self.dirty = true;
            self.last_edit = Instant::now();
            if !self.reminder_ghosts.is_empty() {
                self.reminders_dirty = true;
            }
            // Full lines replacement: invalidate all caches.
            self.fence_checkpoints.truncate(1);
            self.fence_checkpoints_valid_through = 0;
            self.run_calc_recompute();
            self.recompute_folding();
            self.adjust_cursor();
            self.adjust_scroll();
            self.history
                .checkpoint(&self.lines, self.cursor_line, self.cursor_col);
            self.status = format!("undo ({} left)", self.history.undo_depth());
        } else {
            self.status = "already at oldest change".to_string();
        }
    }

    pub(super) fn redo(&mut self) {
        if let Some(cursor) = self.history.redo(&mut self.lines) {
            self.cursor_line = cursor.line.min(self.lines.len().saturating_sub(1));
            self.cursor_col = cursor.col;
            self.dirty = true;
            self.last_edit = Instant::now();
            if !self.reminder_ghosts.is_empty() {
                self.reminders_dirty = true;
            }
            self.fence_checkpoints.truncate(1);
            self.fence_checkpoints_valid_through = 0;
            self.run_calc_recompute();
            self.recompute_folding();
            self.adjust_cursor();
            self.adjust_scroll();
            self.history
                .checkpoint(&self.lines, self.cursor_line, self.cursor_col);
            self.status = format!("redo ({} left)", self.history.redo_depth());
        } else {
            self.status = "already at newest change".to_string();
        }
    }

    pub(super) fn run_calc_recompute(&mut self) {
        if !self.note_math_module_enabled() {
            self.clear_calc_cache();
            self.calc_recompute_pending = false;
            return;
        }
        self.ensure_calc_line_metadata();
        if !self.lines.is_empty() {
            let cursor_line = self.cursor_line.min(self.lines.len().saturating_sub(1));
            self.refresh_calc_line_metadata_at(cursor_line);
        }
        let calc_variables_enabled = self.calc_variables_enabled();
        let calc_table_enabled = self.note_table_module_enabled();
        if self.calc.stale {
            let calc_data = compute_calc_data(
                &self.calc.engine,
                &self.lines,
                calc_variables_enabled,
                calc_table_enabled,
                None,
            );
            self.calc.prev_line_metadata = self.calc.line_metadata.clone();
            self.calc.results = calc_data.line_results;
            self.calc.cell_results = calc_data.cell_results;
            self.calc.variable_names = calc_data.variable_names;
            self.calc.stale = false;
            self.calc_recompute_pending = false;
            return;
        }

        let plan = crate::editor_core::calc_plan::plan_incremental_calc_from_line_metadata(
            &self.calc.prev_line_metadata,
            &self.calc.results,
            &self.lines,
            &self.calc.line_metadata,
        );
        let has_prev = !self.calc.prev_line_metadata.is_empty();

        // Only scan the changed region for variable assignments and builtin
        // formulas (not all lines). Partial eval is safe as long as the edit
        // doesn't touch a formula/assignment — whole-doc presence of formulas
        // elsewhere doesn't force recomputation of unchanged lines.
        let suffix_len = self.lines.len().saturating_sub(plan.eval_to);
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
        let eval_window = crate::editor_core::calc_plan::decide_eval_window_with_flags(
            &self.lines,
            plan.eval_from,
            plan.eval_to,
            &prev_changed_assignment_names,
            prev_changed_had_assignment,
            prev_changed_had_builtin_formula,
            has_prev,
            calc_variables_enabled,
            calc_table_enabled,
        );
        let can_use_partial = eval_window.can_use_partial;
        let eval_from = eval_window.eval_from;
        let eval_to = eval_window.eval_to;

        let (mut new_results, mut new_cell_results, variable_names) = if can_use_partial {
            let mut merged_results = vec![None; self.lines.len()];
            for entry in &plan.base_results {
                if let Some(slot) = merged_results.get_mut(entry.line_idx) {
                    *slot = Some(entry.result.clone());
                }
            }
            // Carry forward cached cell results for unchanged lines (same
            // alignment as base_results, which the planner already validated).
            let mut merged_cells: Vec<Vec<(usize, String)>> = vec![Vec::new(); self.lines.len()];
            for entry in &plan.base_results {
                if let Some(slot) = merged_cells.get_mut(entry.line_idx) {
                    if let Some(cached) = self.calc.cell_results.get(entry.line_idx) {
                        *slot = cached.clone();
                    }
                }
            }

            if eval_from < eval_to {
                let calc_data = compute_calc_data(
                    &self.calc.engine,
                    &self.lines,
                    calc_variables_enabled,
                    calc_table_enabled,
                    Some((eval_from, eval_to)),
                );
                for idx in eval_from..eval_to {
                    if let Some(slot) = merged_results.get_mut(idx) {
                        *slot = calc_data.line_results.get(idx).cloned().unwrap_or(None);
                    }
                    if let Some(slot) = merged_cells.get_mut(idx) {
                        *slot = calc_data.cell_results.get(idx).cloned().unwrap_or_default();
                    }
                }
                (merged_results, merged_cells, calc_data.variable_names)
            } else {
                (
                    merged_results,
                    merged_cells,
                    self.calc.variable_names.clone(),
                )
            }
        } else {
            let calc_data = compute_calc_data(
                &self.calc.engine,
                &self.lines,
                calc_variables_enabled,
                calc_table_enabled,
                None,
            );
            (
                calc_data.line_results,
                calc_data.cell_results,
                calc_data.variable_names,
            )
        };

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
        let aligned = self.calc.prev_line_metadata.len() == self.lines.len()
            && self.calc.results.len() == self.lines.len();
        let mut trailer_rewritten_lines: Vec<usize> = Vec::new();

        if aligned {
            let cursor_line = self.cursor_line;
            let cursor_col = self.cursor_col;
            let selection_range: Option<(usize, usize)> = if matches!(
                self.mode,
                UiMode::Visual | UiMode::VisualLine | UiMode::CommandBar
            ) {
                self.selection_anchor.map(|(anchor_line, _)| {
                    let a = anchor_line.min(cursor_line);
                    let b = anchor_line.max(cursor_line);
                    (a, b)
                })
            } else {
                None
            };

            for i in 0..self.lines.len() {
                let Some(new_result) = new_results[i].as_deref() else {
                    continue;
                };
                let line_is_selected = if let Some((a, b)) = selection_range {
                    a <= i && i <= b
                } else {
                    false
                };
                if !crate::editor_core::calc_plan::should_attempt_calc_trailer_refresh(
                    self.calc.prev_line_metadata[i].hash,
                    self.calc.line_metadata[i].hash,
                    self.calc.results[i].as_deref(),
                    line_is_selected,
                ) {
                    continue;
                }
                let refresh = compute_calc_trailer_refresh(
                    &self.lines[i],
                    new_result,
                    cursor_line == i,
                    cursor_col,
                );
                if let Some((eq_idx, new_tail)) = refresh {
                    self.lines[i].replace_range(eq_idx.., &new_tail);
                    // Line is back in sync with the backend, reflect it in
                    // the cached result so the ghost widget disappears and
                    // the next eligibility round still sees prev-None here.
                    new_results[i] = None;
                    if let Some(slot) = new_cell_results.get_mut(i) {
                        slot.clear();
                    }
                    // Trailer rewrite changed the line bytes; rehash so the
                    // snapshot stays in sync for the next recompute.
                    self.calc.line_metadata[i] =
                        crate::editor_core::calc_plan::line_metadata_with_mask(
                            &self.lines[i],
                            self.calc_feature_mask(),
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
        for line_idx in trailer_rewritten_lines {
            if line_idx < self.calc.prev_line_metadata.len()
                && line_idx < self.calc.line_metadata.len()
            {
                self.calc.prev_line_metadata[line_idx] = self.calc.line_metadata[line_idx].clone();
            }
        }
        self.calc.results = new_results;
        self.calc.cell_results = new_cell_results;
        self.calc.variable_names = variable_names;
        self.calc.stale = false;
        self.calc_recompute_pending = false;
    }

    pub(super) fn maybe_recompute_calc_after_idle(&mut self) {
        if !self.calc_recompute_pending {
            return;
        }
        if self.last_edit.elapsed() < Duration::from_millis(CALC_RECOMPUTE_DEBOUNCE_MS) {
            return;
        }
        self.run_calc_recompute();
    }

    // --- Search ---

    pub(super) fn move_cursor_left_word(&mut self) {
        if self.cursor_col == 0 {
            let current_virtual = self.current_virtual_line();
            if current_virtual > 0 {
                if let Some(prev_real) = self.real_line_for_virtual(current_virtual - 1) {
                    self.cursor_line = prev_real;
                    self.cursor_col = line_char_len(self.current_line());
                }
            }
            return;
        }
        let line = self.current_line();
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();

        let mut col = self.cursor_col;
        if col > len {
            col = len;
        }
        if col == 0 {
            self.cursor_col = 0;
            return;
        }

        col -= 1;
        while col > 0 && chars.get(col).map_or(false, |c| c.is_whitespace()) {
            col -= 1;
        }

        let target_class = chars.get(col).map_or(0, |c| {
            if c.is_alphanumeric() || *c == '_' {
                1
            } else {
                2
            }
        });
        while col > 0 {
            let prev_class = chars.get(col - 1).map_or(0, |c| {
                if c.is_whitespace() {
                    0
                } else if c.is_alphanumeric() || *c == '_' {
                    1
                } else {
                    2
                }
            });
            if prev_class == target_class {
                col -= 1;
            } else {
                break;
            }
        }
        self.cursor_col = col;
    }

    pub(super) fn move_cursor_right_word(&mut self) {
        let line = self.current_line();
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        if self.cursor_col >= len {
            let current_virtual = self.current_virtual_line();
            if current_virtual + 1 < self.visible_line_count() {
                if let Some(next_real) = self.real_line_for_virtual(current_virtual + 1) {
                    self.cursor_line = next_real;
                    self.cursor_col = 0;
                }
            }
            return;
        }
        let mut col = self.cursor_col;
        let start_class = chars.get(col).map_or(0, |c| {
            if c.is_whitespace() {
                0
            } else if c.is_alphanumeric() || *c == '_' {
                1
            } else {
                2
            }
        });

        while col < len {
            let current_class = chars.get(col).map_or(0, |c| {
                if c.is_whitespace() {
                    0
                } else if c.is_alphanumeric() || *c == '_' {
                    1
                } else {
                    2
                }
            });
            if current_class == start_class {
                col += 1;
            } else {
                break;
            }
        }

        if start_class != 0 {
            while col < len && chars.get(col).map_or(false, |c| c.is_whitespace()) {
                col += 1;
            }
        }

        self.cursor_col = col;
    }

    pub(super) fn delete_word_backward(&mut self) -> bool {
        if self.cursor_col == 0 {
            if self.cursor_line > 0 {
                self.backspace();
                return true;
            }
            return false;
        }
        if self.note_table_module_enabled() {
            if let Some(cell) = table_cell_info_at_char(self.current_line(), self.cursor_col) {
                let edit_start = table_cell_edit_start(&cell);
                let edit_end = table_cell_navigation_anchor(self.current_line(), &cell);
                if self.cursor_col <= edit_start {
                    return false;
                }
                let mut col = self.cursor_col.min(edit_end);
                let line = self.current_line();
                let chars: Vec<char> = line.chars().collect();
                while col > edit_start && chars.get(col - 1).is_some_and(|c| !c.is_alphanumeric()) {
                    col -= 1;
                }
                while col > edit_start && chars.get(col - 1).is_some_and(|c| c.is_alphanumeric()) {
                    col -= 1;
                }
                if col == self.cursor_col {
                    return false;
                }
                let start_byte = byte_index(self.current_line(), col);
                let end_byte = byte_index(self.current_line(), self.cursor_col);
                let text = self.current_line_mut();
                text.replace_range(start_byte..end_byte, "");
                self.cursor_col = col;
                self.refresh_calc_line_metadata_at(self.cursor_line);
                self.mark_edited();
                return true;
            }
        }
        let line = self.current_line();
        let chars: Vec<char> = line.chars().collect();
        let mut col = self.cursor_col;
        while col > 0 && chars.get(col - 1).map_or(false, |c| !c.is_alphanumeric()) {
            col -= 1;
        }
        while col > 0 && chars.get(col - 1).map_or(false, |c| c.is_alphanumeric()) {
            col -= 1;
        }

        let start_byte = byte_index(self.current_line(), col);
        let end_byte = byte_index(self.current_line(), self.cursor_col);
        let text = self.current_line_mut();
        text.replace_range(start_byte..end_byte, "");
        self.cursor_col = col;
        self.refresh_calc_line_metadata_at(self.cursor_line);
        self.mark_edited();
        true
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        if ch.is_control() {
            return;
        }
        let col = self.cursor_col;
        let line = self.current_line_mut();
        let idx = byte_index(line, col);
        line.insert(idx, ch);
        self.cursor_col += 1;
        self.refresh_calc_line_metadata_at(self.cursor_line);
        self.mark_edited();
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let col = self.cursor_col;
        let line = self.current_line_mut();
        let idx = byte_index(line, col);
        line.insert_str(idx, text);
        self.cursor_col += text.chars().count();
        self.refresh_calc_line_metadata_at(self.cursor_line);
        self.mark_edited();
    }

    pub(super) fn insert_paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        if self.lines.is_empty() {
            self.lines.push(String::new());
        }

        // Normalize line endings to keep cursor/line mapping predictable.
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let parts: Vec<&str> = normalized.split('\n').collect();
        if parts.is_empty() {
            return;
        }

        let line_idx = self.cursor_line.min(self.lines.len().saturating_sub(1));
        let col = self.cursor_col;
        let current = self.lines[line_idx].clone();
        let split_idx = byte_index(&current, col);
        let (left, right) = current.split_at(split_idx);

        if parts.len() == 1 {
            self.lines[line_idx] = format!("{left}{}{right}", parts[0]);
            self.cursor_line = line_idx;
            self.cursor_col = col + parts[0].chars().count();
            self.splice_calc_line_metadata(line_idx, 1, 1);
            self.mark_edited_from_line(line_idx);
            return;
        }

        self.lines[line_idx] = format!("{left}{}", parts[0]);
        let mut insert_at = line_idx + 1;
        for part in &parts[1..parts.len() - 1] {
            self.lines.insert(insert_at, (*part).to_string());
            insert_at += 1;
        }

        let tail = *parts.last().unwrap_or(&"");
        self.lines.insert(insert_at, format!("{tail}{right}"));
        self.cursor_line = insert_at;
        self.cursor_col = tail.chars().count();
        self.splice_calc_line_metadata(line_idx, 1, parts.len());
        self.mark_edited_from_line(line_idx);
    }

    pub(super) fn insert_newline(&mut self) {
        let changed_from_line = self.cursor_line;
        let col = self.cursor_col;
        let idx = byte_index(self.current_line(), col);
        let right = self.lines[self.cursor_line][idx..].to_string();
        self.lines[self.cursor_line].truncate(idx);
        let insert_at = self.cursor_line + 1;
        self.lines.insert(insert_at, right);
        self.cursor_line += 1;
        self.cursor_col = 0;
        self.splice_calc_line_metadata(changed_from_line, 1, 2);
        self.mark_edited_from_line(changed_from_line);
    }

    pub(super) fn variable_autocomplete_state(&self) -> Option<VariableAutocompleteState> {
        if self.mode != UiMode::Editor {
            return None;
        }
        if !self.note_math_module_enabled()
            || !self.note_variables_module_enabled()
            || self.calc.variable_names.is_empty()
        {
            return None;
        }
        let line = self.current_line();
        let prefix = extract_variable_completion_prefix(line, self.cursor_col)?;
        let suggestions = build_variable_suggestions(
            &self.calc.variable_names,
            &prefix.query,
            self.variable_autocomplete_min_chars,
            VARIABLE_AUTOCOMPLETE_MAX_SUGGESTIONS,
        );
        if suggestions.is_empty() {
            return None;
        }
        Some(VariableAutocompleteState {
            from_col: prefix.from_col,
            to_col: prefix.to_col,
            query: prefix.query,
            suggestions,
        })
    }

    pub(super) fn variable_popup_anchor(&self, anchor_col: usize) -> Option<(usize, usize)> {
        let (rows, cols) = input::terminal_size();
        if rows <= EDITOR_TOP_ROW || cols == 0 {
            return None;
        }
        let cursor_virtual = self.current_virtual_line();
        let row = EDITOR_TOP_ROW
            + cursor_virtual
                .saturating_sub(self.scroll_line)
                .min(rows.saturating_sub(2));
        let gutter_width = self.gutter_width();
        let available = cols.saturating_sub(gutter_width);
        if available == 0 {
            return None;
        }
        let line_text = self.current_line();
        let line_col = anchor_col.min(line_char_len(line_text));
        let display_col = display_cols_for_prefix(line_text, line_col);
        let line_width = line_display_cols(line_text);
        let visible_col =
            viewport_col_for_display_col(display_col, line_width, self.scroll_col, available);
        let col = (gutter_width + visible_col + 1).min(cols.max(1)).max(1);
        Some((row.max(EDITOR_TOP_ROW), col))
    }

    pub(super) fn dismiss_variable_autocomplete_popup(&mut self) {
        self.variable_autocomplete_popup = VariableAutocompletePopupState::default();
    }

    pub(super) fn refresh_variable_autocomplete_popup(&mut self) {
        let Some(state) = self.variable_autocomplete_state() else {
            self.dismiss_variable_autocomplete_popup();
            return;
        };
        let Some((anchor_row, anchor_col)) = self.variable_popup_anchor(state.from_col) else {
            self.dismiss_variable_autocomplete_popup();
            return;
        };
        let previous_selection = if self.variable_autocomplete_popup.visible
            && self.variable_autocomplete_popup.cursor_line == self.cursor_line
            && self.variable_autocomplete_popup.cursor_col <= self.cursor_col
            && self.variable_autocomplete_popup.query == state.query
        {
            self.variable_autocomplete_popup
                .suggestions
                .get(self.variable_autocomplete_popup.selected_index)
                .cloned()
        } else {
            None
        };
        let selected_index = previous_selection
            .as_ref()
            .and_then(|picked| state.suggestions.iter().position(|name| name == picked))
            .unwrap_or(0)
            .min(state.suggestions.len().saturating_sub(1));
        self.variable_autocomplete_popup = VariableAutocompletePopupState {
            visible: true,
            anchor_row,
            anchor_col,
            from_col: state.from_col,
            to_col: state.to_col,
            query: state.query,
            suggestions: state.suggestions,
            selected_index,
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        };
    }

    pub(super) fn move_variable_autocomplete_selection(&mut self, delta: isize) -> bool {
        if !self.variable_autocomplete_popup.visible
            || self.variable_autocomplete_popup.suggestions.is_empty()
        {
            return false;
        }
        let len = self.variable_autocomplete_popup.suggestions.len();
        let current = self
            .variable_autocomplete_popup
            .selected_index
            .min(len.saturating_sub(1));
        let next = if delta >= 0 {
            (current + delta as usize) % len
        } else {
            (current + len - ((-delta) as usize % len)) % len
        };
        self.variable_autocomplete_popup.selected_index = next;
        true
    }

    pub(super) fn apply_variable_autocomplete_pick(
        &mut self,
        from_col: usize,
        to_col: usize,
        pick: String,
    ) -> bool {
        let from_col = from_col.min(self.cursor_col);
        let to_col = to_col.min(line_char_len(self.current_line()));
        if from_col >= to_col {
            return false;
        }

        let from_byte = byte_index(self.current_line(), from_col);
        let to_byte = byte_index(self.current_line(), to_col);
        self.lines[self.cursor_line].replace_range(from_byte..to_byte, &pick);
        self.cursor_col = from_col + pick.chars().count();
        self.refresh_calc_line_metadata_at(self.cursor_line);
        self.mark_edited();
        self.status = format!("autocomplete: {pick}");
        self.dismiss_variable_autocomplete_popup();
        true
    }

    pub(super) fn apply_variable_autocomplete_popup_selection(&mut self) -> bool {
        if !self.variable_autocomplete_popup.visible
            || self.variable_autocomplete_popup.cursor_line != self.cursor_line
            || self.variable_autocomplete_popup.cursor_col > self.cursor_col
        {
            self.dismiss_variable_autocomplete_popup();
            return false;
        }
        let pick = self
            .variable_autocomplete_popup
            .suggestions
            .get(self.variable_autocomplete_popup.selected_index)
            .cloned();
        let Some(pick) = pick else {
            self.dismiss_variable_autocomplete_popup();
            return false;
        };
        self.apply_variable_autocomplete_pick(
            self.variable_autocomplete_popup.from_col,
            self.variable_autocomplete_popup.to_col,
            pick,
        )
    }

    pub(super) fn variable_autocomplete_status_hint(&self) -> Option<String> {
        let (query, suggestions, selected) = if self.variable_autocomplete_popup.visible {
            (
                self.variable_autocomplete_popup.query.clone(),
                self.variable_autocomplete_popup.suggestions.clone(),
                Some(self.variable_autocomplete_popup.selected_index),
            )
        } else {
            let state = self.variable_autocomplete_state()?;
            (state.query, state.suggestions, None)
        };
        let picks = suggestions
            .iter()
            .take(VARIABLE_AUTOCOMPLETE_MAX_SUGGESTIONS)
            .enumerate()
            .map(|(idx, suggestion)| {
                if selected == Some(idx) {
                    format!(">{suggestion}<")
                } else {
                    suggestion.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("  ");
        if picks.is_empty() {
            None
        } else {
            Some(format!("var {query} -> {picks} (Tab/Enter)"))
        }
    }

    pub(super) fn apply_variable_autocomplete_tab(&mut self) -> bool {
        if self.variable_autocomplete_popup.visible {
            return self.apply_variable_autocomplete_popup_selection();
        }
        let Some(state) = self.variable_autocomplete_state() else {
            return false;
        };
        let Some(pick) = state.suggestions.first().cloned() else {
            return false;
        };
        self.apply_variable_autocomplete_pick(state.from_col, state.to_col, pick)
    }

    pub(super) fn apply_calc_tab(&mut self) -> bool {
        if !self.note_math_module_enabled() {
            return false;
        }
        if self.calc_recompute_pending {
            self.run_calc_recompute();
        }
        let text = self.current_line().to_string();
        let Some(result) = self
            .calc
            .results
            .get(self.cursor_line)
            .and_then(|value| value.clone())
        else {
            return false;
        };
        if contains_assignment_operator(&text) {
            return false;
        }

        if let Some((from_byte, to_byte)) = find_calc_segment_range(&text) {
            self.lines[self.cursor_line].replace_range(from_byte..to_byte, &result);
            self.cursor_col = self.lines[self.cursor_line]
                [..from_byte.saturating_add(result.len())]
                .chars()
                .count();
            self.refresh_calc_line_metadata_at(self.cursor_line);
            self.mark_edited();
            return true;
        }

        self.insert_text(&format!(" = {result}"));
        true
    }

    pub(super) fn line_might_trigger_doc_change_rules(line: &str) -> bool {
        let trimmed = line.trim_start();
        let might_be_list = trimmed.starts_with('-')
            || trimmed.starts_with('*')
            || trimmed.starts_with('+')
            || trimmed.starts_with("->")
            || trimmed.chars().next().is_some_and(|c| c.is_ascii_digit());
        let might_be_table = trimmed.starts_with('|') && line.trim_end().ends_with('|');
        might_be_list || might_be_table
    }

    pub(super) fn try_autoformat_rules(&mut self) {
        if !self.note_style_module_enabled() && !self.note_table_module_enabled() {
            return;
        }
        if !Self::line_might_trigger_doc_change_rules(self.current_line()) {
            return;
        }

        let ctx = self.build_context();
        let options = crate::editor_core::text_rules::TextRuleOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            checklist_auto_reorder: self.checklist_auto_reorder_enabled(),
            table_enabled: self.note_table_module_enabled(),
        };
        if let Some(op) = crate::editor_core::text_rules::run_doc_change_rules(&ctx, options) {
            self.apply_edit_operation(&op);
        }
    }

    pub(super) fn try_enter_rule(&mut self) -> bool {
        let ctx = self.build_context();
        let options = crate::editor_core::text_rules::TextRuleOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            checklist_auto_reorder: self.checklist_auto_reorder_enabled(),
            table_enabled: self.note_table_module_enabled(),
        };
        if let Some(op) = crate::editor_core::text_rules::run_enter_rules(&ctx, options) {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    pub(super) fn try_tab_rule(&mut self, outdent: bool) -> bool {
        let ctx = self.build_context();
        let options = crate::editor_core::text_rules::TabRuleOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            outdent,
            table_enabled: self.note_table_module_enabled(),
        };
        if let Some(op) = crate::editor_core::text_rules::run_tab_rules(&ctx, options) {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    pub(super) fn try_table_navigation_rule(&mut self, outdent: bool) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let ctx = self.build_context();
        let options = crate::editor_core::text_rules::TabRuleOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            outdent,
            table_enabled: true,
        };
        if let Some(op) =
            crate::editor_core::text_rules::run_table_cell_navigation_rules(&ctx, options)
        {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    pub(super) fn try_table_pipe_insert_column_rule(&mut self) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let ctx = self.build_context();
        if let Some(op) = crate::editor_core::text_rules::run_table_pipe_insert_column_rule(&ctx) {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    pub(super) fn try_table_header_delete_column_rule(&mut self) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let ctx = self.build_context();
        if let Some(op) = crate::editor_core::text_rules::run_table_header_delete_column_rule(&ctx)
        {
            self.apply_edit_operation(&op);
            return true;
        }
        false
    }

    pub(super) fn try_table_boundary_edit_rule(
        &mut self,
        backward: bool,
        structural_merge: bool,
    ) -> Option<bool> {
        if !self.note_table_module_enabled() {
            return None;
        }
        let ctx = self.build_context();
        let options = crate::editor_core::text_rules::TableBoundaryEditOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            backward,
            structural_merge,
            table_enabled: true,
        };
        let op = crate::editor_core::text_rules::run_table_boundary_edit_rules(&ctx, options)?;
        let changed = !op.changes.is_empty();
        self.apply_edit_operation(&op);
        Some(changed)
    }

    pub(super) fn apply_edit_operation(&mut self, op: &crate::editor_core::types::EditOperation) {
        if !self.active_note_is_editable() && !op.changes.is_empty() {
            self.set_locked_note_status();
            return;
        }
        if op.changes.is_empty() {
            if let Some(sel) = &op.selection {
                let mut offset = 0usize;
                let target = sel.anchor.min(join_lines(&self.lines).len());
                for (i, line) in self.lines.iter().enumerate() {
                    let line_end = offset + line.len();
                    if target <= line_end {
                        self.cursor_line = i;
                        self.cursor_col = line[..target.saturating_sub(offset)].chars().count();
                        break;
                    }
                    offset = line_end + 1;
                }
                self.adjust_cursor();
                self.adjust_scroll();
            }
            return;
        }

        let mut text = join_lines(&self.lines);
        let changed_from_offset = op
            .changes
            .iter()
            .map(|change| change.from.min(text.len()))
            .min()
            .unwrap_or(0);
        let changed_to_offset_old = op
            .changes
            .iter()
            .map(|change| change.to.min(text.len()))
            .max()
            .unwrap_or(changed_from_offset);
        let changed_from_line = text.as_bytes()[..changed_from_offset]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        let old_changed_to_line_exclusive = text.as_bytes()[..changed_to_offset_old]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1;

        // Track initial cursor byte offset
        let mut mapped_anchor = 0;
        for (i, line) in self.lines.iter().enumerate() {
            if i == self.cursor_line {
                mapped_anchor += byte_index(line, self.cursor_col);
                break;
            }
            mapped_anchor += line.len() + 1;
        }

        let mut changes = op.changes.clone();
        changes.sort_by(|a, b| b.from.cmp(&a.from));
        for change in &changes {
            let from = change.from.min(text.len());
            let to = change.to.min(text.len());
            text.replace_range(from..to, &change.insert);

            // Map cursor through change
            if from <= mapped_anchor {
                if to <= mapped_anchor {
                    let removed = to - from;
                    let added = change.insert.len();
                    mapped_anchor = mapped_anchor + added - removed;
                } else {
                    // Keep cursor stable relative to the replacement start
                    // when it falls inside the replaced span.
                    let inside = mapped_anchor.saturating_sub(from);
                    mapped_anchor = from + inside.min(change.insert.len());
                }
            }
        }
        self.lines = split_lines(&text);
        let mapped_from = map_offset_through_changes(changed_from_offset, &changes).min(text.len());
        let mapped_to = map_offset_through_changes(changed_to_offset_old, &changes).min(text.len());
        let mapped_changed_to = mapped_from.max(mapped_to);
        let new_changed_to_line_exclusive = text.as_bytes()[..mapped_changed_to]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1;
        let old_line_span = old_changed_to_line_exclusive
            .saturating_sub(changed_from_line)
            .max(1);
        let new_line_span = new_changed_to_line_exclusive
            .saturating_sub(changed_from_line)
            .max(1);
        self.splice_calc_line_metadata(changed_from_line, old_line_span, new_line_span);

        let final_anchor = if let Some(sel) = &op.selection {
            sel.anchor
        } else {
            mapped_anchor
        }
        .min(text.len());

        let mut offset = 0;
        for (i, line) in self.lines.iter().enumerate() {
            let line_end = offset + line.len();
            if final_anchor <= line_end {
                self.cursor_line = i;
                self.cursor_col = line[..final_anchor.saturating_sub(offset)].chars().count();
                break;
            }
            offset = line_end + 1;
        }
        self.folds.rescan_pending = true;
        self.mark_edited_from_line(changed_from_line);
        self.adjust_cursor();
        self.adjust_scroll();
    }

    pub(super) fn backspace(&mut self) {
        if self.note_table_module_enabled() {
            if let Some(cell) = table_cell_info_at_char(self.current_line(), self.cursor_col) {
                let edit_start = table_cell_edit_start(&cell);
                let edit_end = table_cell_navigation_anchor(self.current_line(), &cell);
                if self.cursor_col <= edit_start {
                    return;
                }
                if self.cursor_col > edit_end {
                    self.cursor_col = edit_end;
                    return;
                }
                let new_col = self.cursor_col - 1;
                if new_col < edit_start {
                    return;
                }
                remove_char_at(&mut self.lines[self.cursor_line], new_col);
                self.cursor_col = new_col;
                self.refresh_calc_line_metadata_at(self.cursor_line);
                self.mark_edited();
                return;
            }
        }

        if self.cursor_col > 0 {
            let new_col = self.cursor_col - 1;
            remove_char_at(&mut self.lines[self.cursor_line], new_col);
            self.cursor_col = new_col;
            self.refresh_calc_line_metadata_at(self.cursor_line);
            self.mark_edited();
            return;
        }

        if self.cursor_line == 0 {
            return;
        }

        let removed = self.lines.remove(self.cursor_line);
        self.cursor_line -= 1;
        let prev_len = line_char_len(&self.lines[self.cursor_line]);
        self.lines[self.cursor_line].push_str(&removed);
        self.cursor_col = prev_len;
        self.splice_calc_line_metadata(self.cursor_line, 2, 1);
        self.mark_edited();
    }

    pub(super) fn delete_forward(&mut self) {
        if self.note_table_module_enabled() {
            if let Some(cell) = table_cell_info_at_char(self.current_line(), self.cursor_col) {
                let edit_start = table_cell_edit_start(&cell);
                let edit_end = table_cell_navigation_anchor(self.current_line(), &cell);
                if self.cursor_col < edit_start {
                    self.cursor_col = edit_start;
                    return;
                }
                if self.cursor_col >= edit_end {
                    return;
                }
                let col = self.cursor_col;
                remove_char_at(&mut self.lines[self.cursor_line], col);
                self.refresh_calc_line_metadata_at(self.cursor_line);
                self.mark_edited();
                return;
            }
        }

        let line_len = line_char_len(self.current_line());
        if self.cursor_col < line_len {
            let col = self.cursor_col;
            remove_char_at(&mut self.lines[self.cursor_line], col);
            self.refresh_calc_line_metadata_at(self.cursor_line);
            self.mark_edited();
            return;
        }

        if self.cursor_line + 1 >= self.lines.len() {
            return;
        }

        let next = self.lines.remove(self.cursor_line + 1);
        self.lines[self.cursor_line].push_str(&next);
        self.splice_calc_line_metadata(self.cursor_line, 2, 1);
        self.mark_edited();
    }

    pub(super) fn move_cursor_left(&mut self) {
        let table_target_col = if self.note_table_module_enabled() {
            let line_text = self.current_line();
            if let Some(current_cell) = table_cell_info_at_char(line_text, self.cursor_col) {
                let anchor = table_cell_navigation_anchor(line_text, &current_cell);
                let edit_start = table_cell_edit_start(&current_cell);
                if table_cell_is_empty(&current_cell) {
                    Some(anchor)
                } else if self.cursor_col > anchor {
                    // Entering left/right padding is not allowed; snap back to content anchor.
                    Some(anchor)
                } else if self.cursor_col <= edit_start {
                    // Regular arrows do not cross cell boundaries.
                    Some(edit_start)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some(target_col) = table_target_col {
            self.cursor_col = target_col;
            return;
        }

        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            return;
        }
        let current_virtual = self.current_virtual_line();
        if current_virtual > 0 {
            if let Some(prev_real) = self.real_line_for_virtual(current_virtual - 1) {
                self.cursor_line = prev_real;
                self.cursor_col = line_char_len(self.current_line());
            }
        }
    }

    pub(super) fn move_cursor_right(&mut self) {
        let table_target_col = if self.note_table_module_enabled() {
            let line_text = self.current_line();
            if let Some(current_cell) = table_cell_info_at_char(line_text, self.cursor_col) {
                let anchor = table_cell_navigation_anchor(line_text, &current_cell);
                let edit_start = table_cell_edit_start(&current_cell);
                if table_cell_is_empty(&current_cell) {
                    Some(anchor)
                } else if self.cursor_col < edit_start {
                    Some(edit_start)
                } else if self.cursor_col > anchor {
                    // Entering padding is not allowed; snap back.
                    Some(anchor)
                } else if self.cursor_col == anchor {
                    // Regular arrows do not cross cell boundaries.
                    Some(anchor)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some(target_col) = table_target_col {
            self.cursor_col = target_col;
            return;
        }

        let line_len = line_char_len(self.current_line());
        if self.cursor_col < line_len {
            self.cursor_col += 1;
            return;
        }
        let current_virtual = self.current_virtual_line();
        if current_virtual + 1 < self.visible_line_count() {
            if let Some(next_real) = self.real_line_for_virtual(current_virtual + 1) {
                self.cursor_line = next_real;
                self.cursor_col = 0;
            }
        }
    }

    pub(super) fn move_cursor_up(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let current_virtual = self.current_virtual_line();
        let target_virtual = current_virtual.saturating_sub(count);
        self.cursor_line = self.real_line_for_virtual(target_virtual).unwrap_or(0);
        if self.note_table_module_enabled() {
            if let Some(cell) = table_cell_info_at_char(self.current_line(), self.cursor_col) {
                self.cursor_col = table_cell_navigation_anchor(self.current_line(), &cell);
            }
        }
    }

    pub(super) fn move_cursor_down(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let current_virtual = self.current_virtual_line();
        let target_virtual = min(
            current_virtual.saturating_add(count),
            self.visible_line_count().saturating_sub(1),
        );
        self.cursor_line = self
            .real_line_for_virtual(target_virtual)
            .unwrap_or_else(|| self.lines.len().saturating_sub(1));
        if self.note_table_module_enabled() {
            if let Some(cell) = table_cell_info_at_char(self.current_line(), self.cursor_col) {
                self.cursor_col = table_cell_navigation_anchor(self.current_line(), &cell);
            }
        }
    }

    pub(super) fn adjust_cursor_with_table_padding_guard(&mut self, clamp_table_padding: bool) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        if self.cursor_line >= self.lines.len() {
            self.cursor_line = self.lines.len() - 1;
        }
        if let Some(owner) = self.fold_hidden_owner_for_line(self.cursor_line) {
            self.cursor_line = owner.min(self.lines.len().saturating_sub(1));
        }
        let len = line_char_len(self.current_line());
        if self.cursor_col > len {
            self.cursor_col = len;
        }
        let table_anchor = if self.note_table_module_enabled() {
            let line_text = self.current_line();
            if let Some(cell) = table_cell_info_at_char(line_text, self.cursor_col) {
                let anchor = table_cell_navigation_anchor(line_text, &cell);
                let edit_start = table_cell_edit_start(&cell);
                if table_cell_is_empty(&cell) {
                    Some(anchor)
                } else if self.cursor_col < edit_start
                    || (clamp_table_padding && self.cursor_col > anchor)
                {
                    // Keep the cursor inside content; right padding is
                    // reserved for alignment only.
                    Some(anchor)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some(anchor) = table_anchor {
            self.cursor_col = anchor;
        }
    }

    pub(super) fn adjust_cursor(&mut self) {
        self.adjust_cursor_with_table_padding_guard(true);
    }

    pub(super) fn editor_height(&self) -> usize {
        let (rows, _) = input::terminal_size();
        rows.saturating_sub(2).max(1)
    }

    pub(super) fn gutter_width(&self) -> usize {
        gutter_width_for_visible_lines(self.visible_line_count())
    }

    pub(super) fn adjust_scroll(&mut self) {
        let height = self.editor_height();
        let cursor_virtual = self.current_virtual_line();
        if cursor_virtual < self.scroll_line {
            self.scroll_line = cursor_virtual;
        } else if cursor_virtual >= self.scroll_line + height {
            self.scroll_line = cursor_virtual + 1 - height;
        }
        self.scroll_line = self
            .scroll_line
            .min(self.visible_line_count().saturating_sub(1));

        let (_, cols) = input::terminal_size();
        let available = cols.saturating_sub(self.gutter_width());
        if available == 0 {
            self.scroll_col = 0;
            return;
        }

        let (cursor_display_col, max_scroll) = {
            let line_text = self.current_line();
            let line_len = line_char_len(line_text);
            let logical_col = min(self.cursor_col, line_len);
            let render_col = cursor_render_char_col(
                line_text,
                self.cursor_col,
                matches!(
                    self.mode,
                    UiMode::Normal | UiMode::Visual | UiMode::VisualLine
                ),
            );
            let line_width = line_display_cols(line_text);
            let end_slot = usize::from(
                self.mode == UiMode::Editor && logical_col == line_len && line_width > available,
            );
            let target_col = if self.mode == UiMode::Editor {
                logical_col
            } else {
                render_col
            };
            (
                display_cols_for_prefix(line_text, target_col),
                line_width
                    .saturating_sub(available)
                    .saturating_add(end_slot),
            )
        };

        if cursor_display_col < self.scroll_col {
            self.scroll_col = cursor_display_col.saturating_sub(HORIZONTAL_SCROLL_LEFT_CONTEXT);
        } else if cursor_display_col >= self.scroll_col + available {
            self.scroll_col = cursor_display_col + 1 - available;
        }

        self.scroll_col = self.scroll_col.min(max_scroll);
    }

    pub(super) fn calc_eval_range_for_viewport(
        &self,
        editor_height: usize,
    ) -> Option<(usize, usize)> {
        if self.lines.is_empty() || editor_height == 0 {
            return None;
        }
        let visible_count = self.visible_line_count();
        if visible_count == 0 {
            return None;
        }
        let prefetch = editor_height.saturating_mul(CALC_VIEWPORT_PREFETCH_MULTIPLIER);
        let start_virtual = self.scroll_line.saturating_sub(prefetch);
        let end_virtual = self
            .scroll_line
            .saturating_add(editor_height)
            .saturating_add(prefetch)
            .min(visible_count.saturating_sub(1));
        let start_line = self.real_line_for_virtual(start_virtual).unwrap_or(0);
        let end_line_inclusive = self
            .real_line_for_virtual(end_virtual)
            .unwrap_or_else(|| self.lines.len().saturating_sub(1));
        let end_line_exclusive = end_line_inclusive.saturating_add(1).min(self.lines.len());
        if start_line >= end_line_exclusive {
            None
        } else {
            Some((start_line, end_line_exclusive))
        }
    }

    pub(super) fn recompute_calc_range(&mut self, eval_from: usize, eval_to: usize) {
        if !self.note_math_module_enabled() {
            self.clear_calc_cache();
            return;
        }
        if eval_from >= eval_to || eval_to > self.lines.len() {
            return;
        }
        let calc_data = compute_calc_data(
            &self.calc.engine,
            &self.lines,
            self.calc_variables_enabled(),
            self.note_table_module_enabled(),
            Some((eval_from, eval_to)),
        );
        if self.calc.results.len() != self.lines.len() {
            self.calc.results = vec![None; self.lines.len()];
        }
        if self.calc.cell_results.len() != self.lines.len() {
            self.calc.cell_results = vec![Vec::new(); self.lines.len()];
        }
        for line_idx in eval_from..eval_to {
            if let Some(slot) = self.calc.results.get_mut(line_idx) {
                *slot = calc_data
                    .line_results
                    .get(line_idx)
                    .cloned()
                    .unwrap_or(None);
            }
            if let Some(slot) = self.calc.cell_results.get_mut(line_idx) {
                *slot = calc_data
                    .cell_results
                    .get(line_idx)
                    .cloned()
                    .unwrap_or_default();
            }
        }
        self.calc.variable_names = calc_data.variable_names;
    }

    // --- Wiki-link autocomplete ---

    fn parse_wiki_link_query(query: &str) -> Option<(&str, Option<&str>)> {
        if query.contains(']') || query.contains('|') {
            return None;
        }
        if let Some(hash_idx) = query.find('#') {
            let short_id = &query[..hash_idx];
            if short_id.len() != 8 || !short_id.chars().all(|ch| ch.is_ascii_alphanumeric()) {
                return None;
            }
            return Some((short_id, Some(&query[hash_idx + 1..])));
        }
        Some((query, None))
    }

    fn load_wiki_link_heading_suggestions(
        db: &crate::storage::Db,
        short_id: &str,
    ) -> Vec<WikiLinkSuggestion> {
        let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
        let Ok(Some(summary)) = note_sources.resolve_wiki_link(short_id) else {
            return Vec::new();
        };
        let Ok(Some(note)) = note_sources.open_note_by_id(&summary.id) else {
            return Vec::new();
        };
        crate::editor_core::markdown_tokens::extract_markdown_headings(&note.body)
            .into_iter()
            .take(32)
            .map(|heading| WikiLinkSuggestion {
                short_id: short_id.to_string(),
                title: heading.clone(),
                heading: Some(heading),
            })
            .collect()
    }

    fn load_wiki_link_note_suggestions(db: &crate::storage::Db) -> Vec<WikiLinkSuggestion> {
        let notes = match crate::terminal::switcher::load_note_meta(db, None) {
            Ok(n) => n,
            Err(_) => return Vec::new(),
        };
        notes
            .into_iter()
            .filter(|n| n.access_mode == app_core::storage::NoteAccessMode::None || n.is_unlocked)
            .map(|n| WikiLinkSuggestion {
                short_id: n.id[..8.min(n.id.len())].to_string(),
                title: if n.title.is_empty() { "Untitled".to_string() } else { n.title },
                heading: None,
            })
            .collect()
    }

    pub(super) fn dismiss_wiki_link_autocomplete(&mut self) {
        self.wiki_link_autocomplete_popup = WikiLinkAutocompletePopupState::default();
    }

    pub(super) fn open_wiki_link_autocomplete(&mut self, db: &crate::storage::Db) {
        let from_col = self.cursor_col.saturating_sub(2);
        let note_suggestions = Self::load_wiki_link_note_suggestions(db);
        let suggestions = note_suggestions.clone();
        let (anchor_row, anchor_col) = self.variable_popup_anchor(from_col).unwrap_or((
            self.cursor_line.saturating_sub(self.scroll_line) + super::EDITOR_TOP_ROW,
            from_col.saturating_add(1),
        ));
        self.wiki_link_autocomplete_popup = WikiLinkAutocompletePopupState {
            visible: true,
            anchor_row,
            anchor_col,
            from_col,
            query: String::new(),
            note_suggestions,
            heading_cache: std::collections::HashMap::new(),
            suggestions,
            selected_index: 0,
            cursor_line: self.cursor_line,
        };
    }

    pub(super) fn refresh_wiki_link_autocomplete(&mut self, db: &crate::storage::Db) {
        if !self.wiki_link_autocomplete_popup.visible {
            return;
        }
        if self.wiki_link_autocomplete_popup.cursor_line != self.cursor_line {
            self.dismiss_wiki_link_autocomplete();
            return;
        }
        let line = self.current_line();
        let from_col = self.wiki_link_autocomplete_popup.from_col;
        // Cursor must stay to the right of [[ and line must still have ]] ahead.
        if self.cursor_col < from_col + 2 {
            self.dismiss_wiki_link_autocomplete();
            return;
        }
        let query: String = line.chars().skip(from_col + 2).take(self.cursor_col - from_col - 2).collect();
        let Some((_, _heading_query)) = Self::parse_wiki_link_query(&query) else {
            self.dismiss_wiki_link_autocomplete();
            return;
        };
        self.wiki_link_autocomplete_popup.query = query;
        if let Some((anchor_row, anchor_col)) = self.variable_popup_anchor(from_col) {
            self.wiki_link_autocomplete_popup.anchor_row = anchor_row;
            self.wiki_link_autocomplete_popup.anchor_col = anchor_col;
        }
        if let Some((short_id, heading_query)) =
            Self::parse_wiki_link_query(self.wiki_link_autocomplete_popup.query.as_str())
        {
            self.wiki_link_autocomplete_popup.suggestions = match heading_query {
                Some(value) => {
                    if !self.wiki_link_autocomplete_popup.heading_cache.contains_key(short_id) {
                        let loaded = Self::load_wiki_link_heading_suggestions(db, short_id);
                        self.wiki_link_autocomplete_popup
                            .heading_cache
                            .insert(short_id.to_string(), loaded);
                    }
                    if let Some(cached) = self.wiki_link_autocomplete_popup.heading_cache.get(short_id) {
                        if value.is_empty() {
                            cached.clone()
                        } else {
                            let query = value.to_lowercase();
                            cached
                                .iter()
                                .filter(|suggestion| suggestion.title.to_lowercase().contains(&query))
                                .cloned()
                                .collect()
                        }
                    } else {
                        Vec::new()
                    }
                }
                None => self.wiki_link_autocomplete_popup.note_suggestions.clone(),
            };
            self.wiki_link_autocomplete_popup.selected_index = 0;
        }
    }

    pub(super) fn filtered_wiki_link_suggestions(&self) -> Vec<&WikiLinkSuggestion> {
        if self
            .wiki_link_autocomplete_popup
            .suggestions
            .iter()
            .any(|s| s.heading.is_some())
        {
            return self
                .wiki_link_autocomplete_popup
                .suggestions
                .iter()
                .take(16)
                .collect();
        }
        let query = self.wiki_link_autocomplete_popup.query.to_lowercase();
        self.wiki_link_autocomplete_popup
            .suggestions
            .iter()
            .filter(|s| query.is_empty() || s.title.to_lowercase().contains(&query))
            .take(16)
            .collect()
    }

    pub(super) fn move_wiki_link_selection(&mut self, delta: isize) -> bool {
        if !self.wiki_link_autocomplete_popup.visible {
            return false;
        }
        let count = self.filtered_wiki_link_suggestions().len();
        if count == 0 {
            return false;
        }
        let current = self.wiki_link_autocomplete_popup.selected_index.min(count.saturating_sub(1));
        let next = if delta >= 0 {
            (current + delta as usize) % count
        } else {
            (current + count - ((-delta) as usize % count)) % count
        };
        self.wiki_link_autocomplete_popup.selected_index = next;
        true
    }

    pub(super) fn apply_wiki_link_selection(&mut self) -> bool {
        if !self.wiki_link_autocomplete_popup.visible {
            return false;
        }
        let suggestions = self.filtered_wiki_link_suggestions();
        let idx = self.wiki_link_autocomplete_popup.selected_index.min(suggestions.len().saturating_sub(1));
        let Some(pick) = suggestions.get(idx) else {
            self.dismiss_wiki_link_autocomplete();
            return false;
        };
        let short_id = pick.short_id.clone();
        let title = pick.title.clone();
        let heading = pick.heading.clone();
        let from_col = self.wiki_link_autocomplete_popup.from_col;
        let replacement = if let Some(heading) = heading.as_deref() {
            format!("[[{}#{}]]", short_id, heading)
        } else {
            format!("[[{}]]", short_id)
        };

        // Find end of [[...]] span: scan forward from from_col for ]]
        let line = self.current_line().to_string();
        let chars: Vec<char> = line.chars().collect();
        let mut end_col = self.cursor_col;
        while end_col + 1 < chars.len() {
            if chars[end_col] == ']' && chars[end_col + 1] == ']' {
                end_col += 2;
                break;
            }
            end_col += 1;
        }

        let from_byte = byte_index(&line, from_col);
        let to_byte = byte_index(&line, end_col);
        self.lines[self.cursor_line].replace_range(from_byte..to_byte, &replacement);
        if heading.is_some() {
            self.cursor_col = from_col + replacement.chars().count();
        } else {
            // Keep caret before closing markers so users can continue with #heading or |alt.
            self.cursor_col = from_col + 2 + short_id.chars().count();
        }
        self.refresh_calc_line_metadata_at(self.cursor_line);
        self.mark_edited();
        if heading.is_some() {
            self.dismiss_wiki_link_autocomplete();
            self.status = format!("link heading: {title}");
        } else {
            self.wiki_link_autocomplete_popup.query = short_id;
            self.wiki_link_autocomplete_popup.suggestions =
                self.wiki_link_autocomplete_popup.note_suggestions.clone();
            self.wiki_link_autocomplete_popup.selected_index = 0;
            self.wiki_link_autocomplete_popup.cursor_line = self.cursor_line;
            if let Some((anchor_row, anchor_col)) =
                self.variable_popup_anchor(self.wiki_link_autocomplete_popup.from_col)
            {
                self.wiki_link_autocomplete_popup.anchor_row = anchor_row;
                self.wiki_link_autocomplete_popup.anchor_col = anchor_col;
            }
            self.status = format!("link: {title}");
        }
        true
    }

    pub(super) fn navigate_wiki_link_at_cursor(&mut self, db: &crate::storage::Db) -> bool {
        let line = self.current_line().to_string();
        let Some(link) = crate::editor_core::markdown_tokens::wiki_link_at_cursor(
            &line,
            self.cursor_col,
        ) else {
            return false;
        };
        let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
        match note_sources.resolve_wiki_link(&link.short_id) {
            Ok(Some(summary)) => {
                match db.get_note(&summary.id) {
                    Ok(Some(note)) => {
                        let heading_text = link.heading.clone();
                        if let Err(e) = self.set_active_note(db, note) {
                            self.status = format!("wiki-link error: {e}");
                        } else {
                            if let Some(ref h) = heading_text {
                                self.jump_to_heading(h);
                            }
                            let dest = match &heading_text {
                                Some(h) => format!("→ {}#{}", summary.title, h),
                                None => format!("→ {}", summary.title),
                            };
                            self.status = dest;
                        }
                        return true;
                    }
                    _ => {
                        self.status = "wiki-link: note not found".to_string();
                    }
                }
            }
            Ok(None) => {
                self.status = "wiki-link: broken (note deleted)".to_string();
            }
            Err(e) => {
                self.status = format!("wiki-link error: {e}");
            }
        }
        true
    }

    fn jump_to_heading(&mut self, heading: &str) {
        let normalize = |value: &str| {
            value
                .trim()
                .trim_end_matches('#')
                .trim()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let needle = normalize(heading);
        if needle.is_empty() {
            return;
        }
        for (idx, line) in self.lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with('#') {
                continue;
            }
            let content = trimmed.trim_start_matches('#').trim();
            if normalize(content) == needle {
                self.cursor_line = idx;
                self.cursor_col = 0;
                self.adjust_scroll();
                return;
            }
        }
    }

    pub(super) fn wiki_link_autocomplete_status_hint(&self) -> Option<String> {
        if !self.wiki_link_autocomplete_popup.visible {
            return None;
        }
        let suggestions = self.filtered_wiki_link_suggestions();
        if suggestions.is_empty() {
            return Some(format!("[[{}… (no matches)", self.wiki_link_autocomplete_popup.query));
        }
        let idx = self.wiki_link_autocomplete_popup.selected_index.min(suggestions.len().saturating_sub(1));
        let picks: Vec<String> = suggestions
            .iter()
            .enumerate()
            .map(|(i, s)| if i == idx { format!(">{}<", s.title) } else { s.title.clone() })
            .collect();
        Some(format!("[[{} → {} (Tab/Enter)", self.wiki_link_autocomplete_popup.query, picks.join("  ")))
    }

    pub(super) fn ensure_calc_for_viewport(&mut self, editor_height: usize, force: bool) {
        if !self.calc_viewport_only {
            return;
        }
        let Some(eval_range) = self.calc_eval_range_for_viewport(editor_height) else {
            return;
        };
        if !force && self.calc_last_view_eval_range == Some(eval_range) {
            return;
        }
        self.recompute_calc_range(eval_range.0, eval_range.1);
        self.calc_last_view_eval_range = Some(eval_range);
    }
}
