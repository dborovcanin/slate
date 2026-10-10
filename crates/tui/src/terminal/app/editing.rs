use super::{
    contains_assignment_operator, cross_note_exports_for_autocomplete, display_cols_for_prefix,
    extract_cross_note_completion_prefix, find_calc_segment_range, find_table_formula_segments,
    gutter_width_for_visible_lines, is_markdown_table_line, line_char_len, line_display_cols,
    preload_cross_note_dep_value, table_cell_edit_start, table_cell_info_at_char,
    table_cell_is_empty, table_cell_navigation_anchor, Db, FoldKind, LineReminderGhost,
    ReminderUndoEntry, TerminalApp, UiMode, UndoAction, VariableAutocompletePopupState,
    VariableAutocompleteState, WikiLinkAutocompletePopupState, WikiLinkSuggestion,
    CALC_ASYNC_MIN_LINES, CALC_IDLE_EVAL_BUDGET_MS, CALC_RECOMPUTE_DEBOUNCE_MS,
    CALC_RECOMPUTE_PENDING_RETRY_MS, CALC_VIEWPORT_PREFETCH_MULTIPLIER, EDITOR_TOP_ROW,
    FENCE_CHECKPOINT_INTERVAL, HORIZONTAL_SCROLL_LEFT_CONTEXT, LARGE_DOC_CALC_DEFER_LINES,
    VARIABLE_AUTOCOMPLETE_MAX_SUGGESTIONS,
};
use crate::editor_core::buffer::paste::{
    normalize_paste, parse_table_paste, table_paste_outside_code,
};
use crate::editor_core::buffer::primitives::PrimitiveEdit;
use crate::editor_core::buffer::words;
use crate::editor_core::buffer::EditDelta;
use crate::editor_core::history::policy::{UndoGrouping, UndoSession};
use crate::editor_core::history::HistoryCursor;
use crate::terminal::text_utils::{
    byte_index, char_col_at_byte, cursor_render_char_col, viewport_col_for_display_col,
};
use crate::terminal::{folding, input};
use app_core::calc::ExternVar;
use std::cmp::min;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif"];
const CALC_PATHOLOGICAL_WINDOW_MIN_LINES: usize = 2000;
const CALC_PATHOLOGICAL_WINDOW_PERCENT: usize = 85;
const CALC_PATHOLOGICAL_WINDOW_STREAK_THRESHOLD: usize = 3;
const CALC_FORCED_FULL_RECOMPUTE_CYCLES: usize = 2;

fn is_image_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    IMAGE_EXTENSIONS
        .iter()
        .any(|allowed| ext.eq_ignore_ascii_case(allowed))
}

fn tokenize_path_candidates(raw: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in raw.trim().chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && !in_single {
            escaped = true;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            continue;
        }
        if ch.is_whitespace() && !in_single && !in_double {
            if !current.is_empty() {
                tokens.push(current.clone());
                current.clear();
            }
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn normalize_pasted_path_token(token: &str) -> Option<PathBuf> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return None;
    }
    let raw = if let Some(rest) = trimmed.strip_prefix("file://") {
        let without_host = rest.strip_prefix("localhost/").unwrap_or(rest);
        let decoded = decode_percent_encoded(without_host);
        #[cfg(windows)]
        let decoded = if decoded.starts_with('/') && decoded.as_bytes().get(2) == Some(&b':') {
            decoded[1..].to_string()
        } else {
            decoded
        };
        decoded
    } else {
        trimmed.to_string()
    };
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    let resolved = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    if !resolved.exists() || !resolved.is_file() || !is_image_path(&resolved) {
        return None;
    }
    Some(resolved)
}

fn decode_percent_encoded(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(10 + (byte - b'a')),
        b'A'..=b'F' => Some(10 + (byte - b'A')),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use ulid::Ulid;

    #[test]
    fn decode_percent_encoded_decodes_ascii_hex_sequences() {
        assert_eq!(decode_percent_encoded("a%20b%2Fc.png"), "a b/c.png");
        assert_eq!(decode_percent_encoded("plain.png"), "plain.png");
    }

    #[test]
    fn normalize_pasted_path_token_accepts_file_url_with_percent_encoding() {
        let dir = std::env::temp_dir().join(format!("slate-image-paste-{}", Ulid::new()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("my image.png");
        fs::write(&path, b"img").expect("seed image");
        let raw = format!("file://{}", path.to_string_lossy().replace(' ', "%20"));
        let resolved = normalize_pasted_path_token(&raw).expect("resolved image path");
        assert_eq!(resolved, path);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalize_pasted_path_token_rejects_non_image_paths() {
        let dir = std::env::temp_dir().join(format!("slate-image-paste-{}", Ulid::new()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("note.txt");
        fs::write(&path, b"txt").expect("seed file");
        assert!(normalize_pasted_path_token(path.to_string_lossy().as_ref()).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}

// Ownership: editor mutations, cursor movement, folding, and calc state updates.
impl TerminalApp {
    fn calc_inputs(&self) -> note_session::calc::CalcInputs {
        note_session::calc::CalcInputs {
            mask: self.calc_feature_mask(),
            math_enabled: self.note_math_module_enabled(),
            viewport_only: self.calc_runtime.viewport_only,
        }
    }

    pub(super) fn undo_session(&self) -> UndoSession {
        if !self.vim_enabled {
            return UndoSession::Other;
        }
        match self.mode {
            UiMode::Editor => UndoSession::Insert,
            UiMode::Normal | UiMode::Visual | UiMode::VisualLine => UndoSession::Command,
            _ => UndoSession::Other,
        }
    }

    pub(super) fn push_reminder_undo_entry(
        &mut self,
        line_idx: usize,
        before: Option<LineReminderGhost>,
        after: Option<LineReminderGhost>,
    ) {
        if before == after {
            return;
        }
        self.session.undo_policy.record_reminder(ReminderUndoEntry {
            line_idx,
            before,
            after,
        });
    }

    fn record_history_after_edit(&mut self, grouping: UndoGrouping, delta: Option<EditDelta>) {
        self.session.undo_policy.record_text(
            &mut self.session.history,
            self.editor.lines(),
            HistoryCursor {
                line: self.editor.cursor_line,
                col: self.editor.cursor_col,
            },
            delta,
            grouping,
        );
        self.move_reminders_with_recorded_edit();
    }

    pub(super) fn bootstrap_folding_for_startup(&mut self) {
        // Keep startup cheap for very large notes: build a plain 1:1 visible
        // map and defer expensive fold structure analysis until needed.
        self.folds.ranges.clear();
        self.folds.range_by_start = vec![None; self.editor.lines().len()];
        self.folds.collapsed_starts.clear();
        self.folds.line_has_structure = vec![false; self.editor.lines().len()];
        // Keep memory lean at startup; full snapshots are only needed once
        // fold analysis actually runs.
        self.folds.line_text_snapshot.clear();
        self.folds.rescan_pending = false;
        self.folds.analysis_ready = false;
        self.rebuild_fold_view_map();
    }

    fn ensure_fold_analysis_ready_for_command(&mut self) {
        if !self.folds.analysis_ready
            || self.folds.rescan_pending
            || self.folds.range_by_start.len() != self.editor.lines().len()
            || self.folds.line_has_structure.len() != self.editor.lines().len()
            || self.folds.line_text_snapshot.len() != self.editor.lines().len()
        {
            self.recompute_folding();
        }
    }

    pub(super) fn current_line(&self) -> &str {
        self.editor
            .lines()
            .get(self.editor.cursor_line)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    pub(super) fn rescan_calc_flags(&mut self) {
        let inputs = self.calc_inputs();
        self.session.calc.rescan_calc_flags(&self.editor, inputs);
    }

    pub(super) fn update_calc_flags_incremental(&mut self) {
        let inputs = self.calc_inputs();
        self.session
            .calc
            .update_calc_flags_incremental(&self.editor, inputs);
    }

    fn rebuild_calc_line_metadata(&mut self) {
        let inputs = self.calc_inputs();
        self.session
            .calc
            .rebuild_calc_line_metadata(&self.editor, inputs);
    }

    fn ensure_calc_line_metadata(&mut self) {
        let inputs = self.calc_inputs();
        self.session
            .calc
            .ensure_calc_line_metadata(&self.editor, inputs);
    }

    fn refresh_calc_line_metadata_at(&mut self, line_idx: usize) {
        let inputs = self.calc_inputs();
        self.calc_runtime.index_sync_pending |=
            self.session
                .calc
                .refresh_calc_line_metadata_at(&self.editor, inputs, line_idx);
    }

    pub(super) fn splice_calc_line_metadata(
        &mut self,
        start_line: usize,
        old_line_span: usize,
        new_line_span: usize,
    ) {
        let inputs = self.calc_inputs();
        self.calc_runtime.index_sync_pending |= self.session.calc.splice_calc_line_metadata(
            &self.editor,
            inputs,
            start_line,
            old_line_span,
            new_line_span,
        );
    }

    pub(super) fn line_has_fold_structure(text: &str) -> bool {
        crate::editor_core::folding::upkeep::line_has_fold_structure(text)
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
        self.active_note.modules.style && !self.large_note_reduced_features()
    }

    pub(super) fn note_cross_note_module_enabled(&self) -> bool {
        self.active_note.modules.cross_note
    }

    pub(super) fn calc_feature_mask(&self) -> crate::editor_core::calc_plan::CalcFeatureMask {
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
            && self.session.calc.cached_has_variable_assignment
    }

    pub(super) fn calc_variables_enabled(&self) -> bool {
        self.active_has_variable_assignments()
    }

    pub(super) fn calc_cross_note_enabled(&self) -> bool {
        self.note_math_module_enabled() && self.note_cross_note_module_enabled()
    }

    fn calc_signal_flags(&self) -> crate::editor_core::calc_plan::CalcSignalFlags {
        crate::editor_core::calc_plan::CalcSignalFlags {
            has_builtin_formula: self.session.calc.cached_has_builtin_formula,
            has_variable_assignment: self.active_has_variable_assignments(),
            has_expression: self.session.calc.cached_has_expression,
        }
    }
    pub(super) fn should_defer_calc_recompute(&self) -> bool {
        crate::editor_core::calc_plan::should_defer_after_edit(
            self.editor.lines().len(),
            self.calc_signal_flags(),
            LARGE_DOC_CALC_DEFER_LINES,
        )
    }
    fn calc_recompute_debounce_duration(&self) -> Duration {
        Duration::from_millis(CALC_RECOMPUTE_DEBOUNCE_MS)
    }

    /// Viewport-only notes after an edit that may insert, delete or replace
    /// lines: shifts cached results so unchanged lines keep their values and
    /// re-evaluates only the viewport, and only when the changed lines take
    /// part in calc. The whole note is never evaluated on the keystroke.
    fn refresh_viewport_calc_after_edit(&mut self) {
        let inputs = self.calc_inputs();
        let (needs_eval, index_sync_pending) = self
            .session
            .calc
            .refresh_viewport_calc_after_edit(&self.editor, inputs);
        self.calc_runtime.index_sync_pending |= index_sync_pending;
        if needs_eval {
            let height = self.editor_height();
            self.ensure_calc_for_viewport(height, true);
        }
        self.calc_runtime.recompute_pending = false;
        self.calc_runtime.recompute_due_at = None;
        self.calc_runtime.pending_viewport_pass = false;
        self.calc_runtime.pending_full_pass = false;
    }

    fn schedule_calc_recompute(&mut self, viewport_pass: bool, full_pass: bool) {
        self.calc_runtime.recompute_pending = true;
        self.calc_runtime.pending_viewport_pass |= viewport_pass;
        self.calc_runtime.pending_full_pass |= full_pass;
        self.calc_runtime.recompute_due_at = None;
    }

    fn try_remap_calc_results_after_structural_edit(&mut self) -> bool {
        let inputs = self.calc_inputs();
        if !self
            .session
            .calc
            .try_remap_calc_results_after_structural_edit(&self.editor, inputs)
        {
            return false;
        }
        self.calc_runtime.recompute_pending = false;
        self.calc_runtime.recompute_due_at = None;
        self.calc_runtime.pending_viewport_pass = false;
        self.calc_runtime.pending_full_pass = false;
        true
    }

    /// Re-evaluates every calc result after something note-wide changed,
    /// such as the calc modules or the exchange rates.
    pub(super) fn recompute_calc_whole_note(&mut self) {
        self.calc_runtime.viewport_only = self.note_math_module_enabled()
            && self.editor.lines().len() >= super::CALC_VIEWPORT_ONLY_MIN_LINES
            && self.active_has_variable_assignments()
            && !self.session.calc.cached_has_builtin_formula;
        if self.note_math_module_enabled() {
            self.session.calc.stale = true;
            if self.calc_runtime.viewport_only {
                self.clear_calc_cache();
                let editor_height = self.editor_height();
                self.ensure_calc_for_viewport(editor_height, true);
            } else {
                self.run_calc_recompute();
            }
        } else {
            self.clear_calc_cache();
        }
    }

    pub(super) fn clear_calc_cache(&mut self) {
        self.session.calc.results = vec![None; self.editor.lines().len()];
        self.session.calc.cell_results = vec![Vec::new(); self.editor.lines().len()];
        self.session.calc.variable_names.clear();
        self.session.calc.calc_dependency_index = None;
        self.session.calc.line_metadata.clear();
        self.session.calc.prev_line_metadata.clear();
        self.session.calc.stale = false;
        self.session.calc.pathological_window_streak = 0;
        self.session.calc.forced_full_recompute_remaining = 0;
        self.calc_runtime.last_view_eval_range = None;
        self.calc_runtime.recompute_pending = false;
        self.calc_runtime.recompute_due_at = None;
        self.calc_runtime.pending_viewport_pass = false;
        self.calc_runtime.pending_full_pass = false;
    }

    pub(super) fn defer_calc_state_after_edit(&mut self) {
        // Large docs without explicit calc syntax should not recompute calc
        // state on every keystroke.
        // Clear the full cache so same-line-count multi-line edits cannot
        // leave stale calc ghosts on non-cursor lines.
        if self.session.calc.results.len() != self.editor.lines().len() {
            self.session.calc.results = vec![None; self.editor.lines().len()];
        } else {
            self.session.calc.results.fill(None);
        }
        if self.session.calc.cell_results.len() != self.editor.lines().len() {
            self.session.calc.cell_results = vec![Vec::new(); self.editor.lines().len()];
        } else {
            for row in &mut self.session.calc.cell_results {
                row.clear();
            }
        }
        self.session.calc.variable_names.clear();
        self.session.calc.stale = true;
    }

    pub(super) fn recompute_folding_if_needed(&mut self, delta: Option<EditDelta>) {
        use crate::editor_core::folding::upkeep::{FoldUpkeep, FoldUpkeepFlags};
        let flags = FoldUpkeepFlags {
            reduced_features: self.large_note_reduced_features(),
            rescan_pending: self.folds.rescan_pending,
            has_collapsed: !self.folds.collapsed_starts.is_empty(),
            view_line_count: self.folds.real_to_visible.len(),
        };
        let plan = crate::editor_core::folding::upkeep::plan_fold_upkeep_with_delta(
            self.editor.lines(),
            self.editor.cursor_line,
            delta,
            &mut self.folds.line_has_structure,
            &mut self.folds.line_text_snapshot,
            &self.folds.ranges,
            flags,
        );
        match plan {
            FoldUpkeep::Disabled => {
                if !self.folds.ranges.is_empty() {
                    self.folds.ranges.clear();
                }
                if self.folds.range_by_start.len() != self.editor.lines().len() {
                    self.folds.range_by_start = vec![None; self.editor.lines().len()];
                }
                if !self.folds.collapsed_starts.is_empty() {
                    self.folds.collapsed_starts.clear();
                }
                if self.folds.visible_to_real.len() != self.editor.lines().len()
                    || self.folds.real_to_visible.len() != self.editor.lines().len()
                    || self.folds.hidden_owner.len() != self.editor.lines().len()
                    || self.folds.placeholder_hidden_lines.len() != self.editor.lines().len()
                {
                    self.rebuild_fold_view_map();
                }
                if self.folds.line_has_structure.len() != self.editor.lines().len() {
                    self.folds.line_has_structure = vec![false; self.editor.lines().len()];
                }
                self.folds.line_text_snapshot.clear();
                self.folds.rescan_pending = false;
                self.folds.analysis_ready = false;
            }
            FoldUpkeep::PendingVisible { reset_map } | FoldUpkeep::Defer { reset_map } => {
                if reset_map {
                    self.folds.ranges.clear();
                    self.folds.range_by_start = vec![None; self.editor.lines().len()];
                    self.rebuild_fold_view_map();
                }
                self.folds.rescan_pending = true;
                self.folds.analysis_ready = false;
            }
            FoldUpkeep::Recompute => {
                self.folds.rescan_pending = false;
                self.recompute_folding();
            }
            FoldUpkeep::Empty => {
                self.apply_fold_ranges(Vec::new());
                self.folds.analysis_ready = true;
            }
            FoldUpkeep::Mapped(ranges) => {
                self.folds.rescan_pending = false;
                self.apply_fold_ranges(ranges);
                self.folds.analysis_ready = true;
            }
            FoldUpkeep::Unchanged => {}
        }
    }

    pub(super) fn recompute_folding(&mut self) {
        self.folds.line_has_structure = self
            .editor
            .lines()
            .iter()
            .map(|line| Self::line_has_fold_structure(line))
            .collect();
        self.folds.line_text_snapshot = self.editor.lines().to_vec();
        self.recompute_folding_from_cached_structure();
    }

    pub(super) fn recompute_folding_from_cached_structure(&mut self) {
        self.folds.rescan_pending = false;
        let ranges = folding::build_fold_ranges(self.editor.lines());
        self.apply_fold_ranges(ranges);
        self.folds.analysis_ready = true;
    }

    fn apply_fold_ranges(&mut self, ranges: Vec<crate::editor_core::folding::FoldRange>) {
        self.folds.ranges = ranges;
        self.folds.range_by_start = vec![None; self.editor.lines().len()];
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
        let line_count = self.editor.lines().len();
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
    pub(super) fn fence_state_before_line(
        &mut self,
        target_line: usize,
    ) -> crate::editor_core::markdown_tokens::FenceState {
        use crate::editor_core::markdown_tokens::{advance_fence_state, FenceState};
        if target_line == 0 {
            return FenceState::default();
        }
        let target = target_line.min(self.editor.lines().len());
        let target_ck = target / FENCE_CHECKPOINT_INTERVAL;
        let start_ck = target_ck.min(self.render_state.fence_checkpoints_valid_through);
        let start_line = start_ck * FENCE_CHECKPOINT_INTERVAL;

        let mut state = if start_ck == 0 {
            FenceState::default()
        } else {
            self.render_state
                .fence_checkpoints
                .get(start_ck)
                .cloned()
                .unwrap_or_default()
        };

        let mut line_idx = start_line;
        while line_idx < target {
            // At each new checkpoint boundary, cache the current state.
            if line_idx > 0 && line_idx % FENCE_CHECKPOINT_INTERVAL == 0 {
                let ck = line_idx / FENCE_CHECKPOINT_INTERVAL;
                if ck > self.render_state.fence_checkpoints_valid_through {
                    while self.render_state.fence_checkpoints.len() <= ck {
                        self.render_state
                            .fence_checkpoints
                            .push(FenceState::default());
                    }
                    self.render_state.fence_checkpoints[ck] = state.clone();
                    self.render_state.fence_checkpoints_valid_through = ck;
                }
            }
            if let Some(line_text) = self.editor.lines().get(line_idx) {
                advance_fence_state(&mut state, line_text);
            }
            line_idx += 1;
        }
        state
    }

    // Invalidate all fence checkpoints that depend on content at or after
    // `line_idx`. Called whenever lines at or before a checkpoint boundary change.
    pub(super) fn invalidate_fence_checkpoints_from_line(&mut self, line_idx: usize) {
        let keep_through = line_idx / FENCE_CHECKPOINT_INTERVAL;
        if self.render_state.fence_checkpoints_valid_through > keep_through {
            self.render_state.fence_checkpoints_valid_through = keep_through;
        }
    }

    pub(super) fn visible_line_count(&self) -> usize {
        self.folds.visible_to_real.len().max(1)
    }

    pub(super) fn current_virtual_line(&self) -> usize {
        self.folds
            .real_to_visible
            .get(self.editor.cursor_line)
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
        if self.large_note_reduced_features() {
            self.status = format!(
                "fold disabled for notes above {} lines",
                super::LARGE_NOTE_FULL_FEATURE_LINE_LIMIT
            );
            return false;
        }
        self.ensure_fold_analysis_ready_for_command();
        let line = self
            .editor
            .cursor_line
            .min(self.editor.lines().len().saturating_sub(1));
        let Some(start_line) = self.fold_start_for_line(line) else {
            self.status = "fold: no foldable block at cursor".to_string();
            return false;
        };
        let next_collapsed = !self.folds.collapsed_starts.contains(&start_line);
        self.set_fold_collapsed_at_line(start_line, next_collapsed)
    }

    pub(super) fn set_fold_collapsed_at_cursor(&mut self, collapsed: bool) -> bool {
        if self.large_note_reduced_features() {
            self.status = format!(
                "fold disabled for notes above {} lines",
                super::LARGE_NOTE_FULL_FEATURE_LINE_LIMIT
            );
            return false;
        }
        self.ensure_fold_analysis_ready_for_command();
        let line = self
            .editor
            .cursor_line
            .min(self.editor.lines().len().saturating_sub(1));
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

    pub(super) fn mark_edited_from_line_with_span(
        &mut self,
        changed_from_line: usize,
        delta: Option<EditDelta>,
    ) {
        let grouping = UndoGrouping {
            session: self.undo_session(),
            elapsed: self.last_edit.elapsed(),
        };
        let session_edit = std::mem::take(&mut self.pending_session_edit);
        if !session_edit {
            self.invalidate_joined_text_cache();
        }
        self.render_caches.table_formula_segment_cache.clear();
        self.session.dirty = true;
        if changed_from_line == 0 {
            self.switcher.needs_title_refresh = true;
        }
        let clamped_changed_line = if self.editor.lines().is_empty() {
            0
        } else {
            changed_from_line.min(self.editor.lines().len().saturating_sub(1))
        };
        self.invalidate_fence_checkpoints_from_line(clamped_changed_line);
        self.update_calc_flags_incremental();
        self.recompute_folding_if_needed(delta);
        use crate::editor_core::calc_plan::{AfterEditFlags, CalcAfterEdit};
        let plan = crate::editor_core::calc_plan::plan_after_edit(AfterEditFlags {
            line_count: self.editor.lines().len(),
            previous_line_count: self.session.calc.results.len(),
            signals: self.calc_signal_flags(),
            stale: self.session.calc.stale,
            viewport_only: self.calc_runtime.viewport_only,
            async_min_lines: CALC_ASYNC_MIN_LINES,
            defer_min_lines: LARGE_DOC_CALC_DEFER_LINES,
        });
        match plan {
            CalcAfterEdit::Skip | CalcAfterEdit::Defer => {
                if plan == CalcAfterEdit::Defer {
                    self.defer_calc_state_after_edit();
                }
                self.calc_runtime.recompute_pending = false;
                self.calc_runtime.recompute_due_at = None;
                self.calc_runtime.pending_viewport_pass = false;
                self.calc_runtime.pending_full_pass = false;
            }
            CalcAfterEdit::ViewportRefresh => self.refresh_viewport_calc_after_edit(),
            CalcAfterEdit::RemapOrRecompute => {
                if !self.try_remap_calc_results_after_structural_edit() {
                    self.recompute_calc_after_edit(delta);
                }
            }
            CalcAfterEdit::RecomputeRange => self.recompute_calc_after_edit(delta),
            CalcAfterEdit::ScheduleIdle => {
                // Keep large-note typing non-blocking: schedule calc for
                // the next idle tick and clear only the edited line's
                // cached result so we don't show stale ghosts while
                // pending.
                if let Some(slot) = self.session.calc.results.get_mut(self.editor.cursor_line) {
                    *slot = None;
                }
                if let Some(slot) = self
                    .session
                    .calc
                    .cell_results
                    .get_mut(self.editor.cursor_line)
                {
                    slot.clear();
                }
                // Keep the edited lines' metadata current so a later
                // structural edit diffs against what is actually there.
                let (start, span) = match delta {
                    Some(delta) if delta.old_span == delta.new_span => {
                        (delta.start_line, delta.new_span)
                    }
                    _ => (self.editor.cursor_line, 1),
                };
                // Without metadata yet (viewport notes build it at idle),
                // there is nothing to keep current.
                if !self.session.calc.line_metadata.is_empty() {
                    for line_idx in start..start + span {
                        self.refresh_calc_line_metadata_at(line_idx);
                    }
                    if !(start..start + span).contains(&self.editor.cursor_line) {
                        self.refresh_calc_line_metadata_at(self.editor.cursor_line);
                    }
                }
                self.schedule_calc_recompute(true, true);
            }
        }
        // Splices are only meaningful for the edit that recorded them.
        self.session.calc.pending_result_splices.clear();
        if session_edit {
            self.session.finish_edit(
                &self.editor,
                note_session::EditContext {
                    grouping,
                    defer_history: true,
                },
                delta.expect("session edit span"),
            );
        } else {
            self.record_history_after_edit(grouping, delta);
            self.session.note_changed();
        }
        self.last_edit = Instant::now();
    }

    /// Recomputes calc now, or once the key being handled is done when it may
    /// edit again (autoformat after the typed text). The edited lines'
    /// metadata is kept current so that recompute sees every edit.
    fn recompute_calc_after_edit(&mut self, delta: Option<EditDelta>) {
        if self.key_depth == 0 {
            self.run_calc_recompute();
            return;
        }
        self.ensure_calc_line_metadata();
        if let Some(delta) = delta {
            if delta.old_span == delta.new_span {
                for line_idx in delta.start_line..delta.start_line + delta.new_span {
                    self.refresh_calc_line_metadata_at(line_idx);
                }
            }
        }
        self.refresh_calc_line_metadata_at(self.editor.cursor_line);
        self.calc_recompute_after_key = true;
    }

    pub(super) fn mark_edited_with_delta(&mut self, delta: EditDelta) {
        self.mark_edited_from_line_with_span(delta.start_line, Some(delta));
    }

    #[cfg(test)]
    pub(super) fn mark_edited_from_line(&mut self, changed_from_line: usize) {
        self.mark_edited_from_line_with_span(changed_from_line, None);
    }

    #[cfg(test)]
    pub(super) fn mark_edited(&mut self) {
        self.mark_edited_from_line_with_span(self.editor.cursor_line, None);
    }

    pub(super) fn mark_edited_current_line(&mut self) {
        let changed_line = self
            .editor
            .cursor_line
            .min(self.editor.lines().len().saturating_sub(1));
        self.mark_edited_with_delta(EditDelta {
            start_line: changed_line,
            old_span: 1,
            new_span: 1,
        });
    }

    fn apply_reminder_state(
        &mut self,
        db: &Db,
        line_idx: usize,
        state: Option<LineReminderGhost>,
    ) -> Result<(), String> {
        match state {
            Some(reminder) => {
                self.session.reminder_ghosts.insert(line_idx, reminder);
            }
            None => {
                self.session.reminder_ghosts.remove(&line_idx);
            }
        }
        self.reminders_changed_outside_text(db);
        Ok(())
    }

    fn undo_text_action(&mut self) {
        if self.session.undo(&mut self.editor).is_some() {
            self.last_edit = Instant::now();
            // Full lines replacement: invalidate all caches.
            self.render_state.fence_checkpoints.truncate(1);
            self.render_state.fence_checkpoints_valid_through = 0;
            if self.calc_runtime.viewport_only {
                // History swaps lines without splicing metadata.
                self.session.calc.pending_result_splices.clear();
                self.refresh_viewport_calc_after_edit();
            } else {
                self.run_calc_recompute();
            }
            // History may replace any lines; rescan, deferred to idle time
            // unless a collapsed fold depends on it.
            self.folds.rescan_pending = true;
            self.recompute_folding_if_needed(None);
            self.adjust_cursor();
            if self.mode == UiMode::Normal {
                self.editor.cursor_col = self
                    .editor
                    .cursor_col
                    .min(line_char_len(self.current_line()).saturating_sub(1));
            }
            self.adjust_scroll();
            self.session.history.checkpoint(
                self.editor.lines(),
                self.editor.cursor_line,
                self.editor.cursor_col,
            );
            self.status = format!(
                "undo ({} left)",
                self.session.undo_policy.undo_depth().saturating_sub(1)
            );
        } else {
            self.status = "already at oldest change".to_string();
        }
    }

    fn redo_text_action(&mut self) {
        if self.session.redo(&mut self.editor).is_some() {
            self.last_edit = Instant::now();
            self.render_state.fence_checkpoints.truncate(1);
            self.render_state.fence_checkpoints_valid_through = 0;
            if self.calc_runtime.viewport_only {
                // History swaps lines without splicing metadata.
                self.session.calc.pending_result_splices.clear();
                self.refresh_viewport_calc_after_edit();
            } else {
                self.run_calc_recompute();
            }
            // History may replace any lines; rescan, deferred to idle time
            // unless a collapsed fold depends on it.
            self.folds.rescan_pending = true;
            self.recompute_folding_if_needed(None);
            self.adjust_cursor();
            if self.mode == UiMode::Normal {
                self.editor.cursor_col = self
                    .editor
                    .cursor_col
                    .min(line_char_len(self.current_line()).saturating_sub(1));
            }
            self.adjust_scroll();
            self.session.history.checkpoint(
                self.editor.lines(),
                self.editor.cursor_line,
                self.editor.cursor_col,
            );
            self.status = format!("redo ({} left)", self.session.history.redo_depth());
        } else {
            self.status = "already at newest change".to_string();
        }
    }

    pub(super) fn undo(&mut self, db: &Db) {
        let Some(action) = self.session.undo_policy.undo_action().cloned() else {
            self.status = "already at oldest change".to_string();
            return;
        };
        match action {
            UndoAction::Text => self.undo_text_action(),
            UndoAction::Reminder(entry) => {
                if let Err(error) = self.apply_reminder_state(db, entry.line_idx, entry.before) {
                    self.status = format!("undo reminder failed: {error}");
                    return;
                }
                self.status = format!("undo reminder on line {}", entry.line_idx + 1);
            }
        }
        self.session.undo_policy.complete_undo();
    }

    pub(super) fn redo(&mut self, db: &Db) {
        let Some(action) = self.session.undo_policy.redo_action().cloned() else {
            self.status = "already at newest change".to_string();
            return;
        };
        match action {
            UndoAction::Text => self.redo_text_action(),
            UndoAction::Reminder(entry) => {
                if let Err(error) = self.apply_reminder_state(db, entry.line_idx, entry.after) {
                    self.status = format!("redo reminder failed: {error}");
                    return;
                }
                self.status = format!("redo reminder on line {}", entry.line_idx + 1);
            }
        }
        self.session.undo_policy.complete_redo();
    }

    pub(super) fn run_calc_recompute(&mut self) {
        let started = Instant::now();
        self.calc_runtime.recompute_due_at = None;
        self.calc_runtime.pending_viewport_pass = false;
        self.calc_runtime.pending_full_pass = false;
        if !self.note_math_module_enabled() {
            self.clear_calc_cache();
            self.calc_runtime.recompute_pending = false;
            self.record_perf_duration(
                "tui.session.calc.recompute",
                "math_disabled",
                started.elapsed(),
            );
            return;
        }
        let variables_enabled = self.calc_variables_enabled();
        let cross_note_enabled = self.calc_cross_note_enabled();
        let table_enabled = self.note_table_module_enabled();
        if cross_note_enabled {
            self.preload_cross_note_deps();
        }
        let selection_range = if matches!(
            self.mode,
            UiMode::Visual | UiMode::VisualLine | UiMode::CommandBar
        ) {
            self.editor.selection_anchor.map(|(line, _)| {
                (
                    line.min(self.editor.cursor_line),
                    line.max(self.editor.cursor_line),
                )
            })
        } else {
            None
        };
        let index = std::sync::Arc::clone(&self.cross_note_var_index);
        let note_id = self.active_note.id.clone();
        let inputs = note_session::calc_recompute::CalcRecomputeInputs {
            base: self.calc_inputs(),
            variables_enabled,
            cross_note_enabled,
            table_enabled,
            note_id: &note_id,
            index: &index,
            selection_range,
            thresholds: note_session::calc_recompute::CalcThresholds {
                calc_pathological_window_min_lines: CALC_PATHOLOGICAL_WINDOW_MIN_LINES,
                calc_pathological_window_percent: CALC_PATHOLOGICAL_WINDOW_PERCENT,
                calc_pathological_window_streak_threshold:
                    CALC_PATHOLOGICAL_WINDOW_STREAK_THRESHOLD,
                calc_forced_full_recompute_cycles: CALC_FORCED_FULL_RECOMPUTE_CYCLES,
            },
        };
        let outcome = self
            .session
            .recompute_calc(&mut self.editor, inputs, &mut |lines| {
                let refs = app_core::calc::scan_cross_note_refs(lines);
                if let Ok(mut index) = index.lock() {
                    index.update_deps(&note_id, &refs);
                    index.extern_vars_for(&note_id)
                } else {
                    Vec::new()
                }
            });
        self.calc_runtime.recompute_pending = false;
        self.calc_runtime.recompute_due_at = None;
        self.calc_runtime.pending_viewport_pass = false;
        self.calc_runtime.pending_full_pass = false;
        let label = match outcome {
            note_session::calc_recompute::CalcRecompute::StaleFull => "stale_full",
            note_session::calc_recompute::CalcRecompute::Incremental => "incremental",
        };
        self.record_perf_duration("tui.session.calc.recompute", label, started.elapsed());
    }

    pub(super) fn maybe_recompute_calc_after_idle(&mut self) {
        if !self.calc_runtime.recompute_pending {
            return;
        }
        if self
            .calc_runtime
            .recompute_due_at
            .is_some_and(|due| Instant::now() < due)
        {
            return;
        }
        if self.last_edit.elapsed() < self.calc_recompute_debounce_duration() {
            return;
        }
        self.render_state.dirty = true;
        if self.calc_runtime.viewport_only {
            let editor_height = self.editor_height();
            self.ensure_calc_for_viewport(editor_height, true);
            self.calc_runtime.recompute_pending = false;
            self.calc_runtime.recompute_due_at = None;
            self.calc_runtime.pending_viewport_pass = false;
            self.calc_runtime.pending_full_pass = false;
            return;
        }
        if self.editor.lines().len() >= CALC_ASYNC_MIN_LINES {
            let budget = Duration::from_millis(CALC_IDLE_EVAL_BUDGET_MS);
            let tick_started = Instant::now();
            if self.calc_runtime.pending_viewport_pass {
                let editor_height = self.editor_height();
                self.ensure_calc_for_viewport(editor_height, true);
                self.calc_runtime.pending_viewport_pass = false;
                if tick_started.elapsed() >= budget {
                    self.calc_runtime.recompute_due_at = Some(
                        Instant::now() + Duration::from_millis(CALC_RECOMPUTE_PENDING_RETRY_MS),
                    );
                    return;
                }
            }
        }
        self.run_calc_recompute();
    }

    // --- Search ---

    fn move_cursor_word(&mut self, forward: bool) {
        let current_virtual = self.current_virtual_line();
        let cursor = self.editor.cursor();
        let tables = self.note_table_module_enabled();
        let after = if forward {
            let neighbors = (current_virtual + 1..self.visible_line_count())
                .map_while(|line| self.real_line_for_virtual(line));
            words::move_cursor_right_word(self.editor.lines(), cursor, tables, neighbors)
        } else {
            let neighbors = (0..current_virtual)
                .rev()
                .map_while(|line| self.real_line_for_virtual(line));
            words::move_cursor_left_word(self.editor.lines(), cursor, tables, neighbors)
        };
        self.editor.set_cursor(after);
    }

    pub(super) fn move_cursor_left_word(&mut self) {
        self.move_cursor_word(false);
    }

    pub(super) fn move_cursor_right_word(&mut self) {
        self.move_cursor_word(true);
    }

    pub(super) fn delete_word_backward(&mut self) -> bool {
        if self.editor.cursor_col == 0 {
            if self.editor.cursor_line == 0 {
                return false;
            }
            self.backspace();
            return true;
        }
        let Some(outcome) =
            self.apply_session_edit(note_session::SessionEdit::BackwardWordDelete {
                tables: self.note_table_module_enabled(),
            })
        else {
            return false;
        };
        self.refresh_calc_line_metadata_at(outcome.delta.start_line);
        self.mark_edited_with_delta(outcome.delta);
        self.prune_empty_table_continuation_row_at_cursor();
        true
    }

    fn session_edit_context(&self) -> note_session::EditContext {
        note_session::EditContext {
            grouping: UndoGrouping {
                session: UndoSession::Other,
                elapsed: Duration::ZERO,
            },
            defer_history: true,
        }
    }

    pub(super) fn apply_session_edit(
        &mut self,
        edit: note_session::SessionEdit<'_>,
    ) -> Option<note_session::EditOutcome> {
        let ctx = self.session_edit_context();
        let outcome = self.session.apply(&mut self.editor, edit, ctx)?;
        self.pending_session_edit = outcome.text_changed;
        Some(outcome)
    }

    fn apply_buffer_primitive(&mut self, primitive: PrimitiveEdit<'_>) -> Option<EditDelta> {
        self.apply_session_edit(note_session::SessionEdit::Primitive(primitive))
            .map(|outcome| outcome.delta)
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        if self
            .apply_buffer_primitive(PrimitiveEdit::InsertChar(ch))
            .is_none()
        {
            return;
        }
        self.refresh_calc_line_metadata_at(self.editor.cursor_line);
        self.mark_edited_current_line();
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        if self
            .apply_buffer_primitive(PrimitiveEdit::InsertText(text))
            .is_none()
        {
            return;
        }
        self.refresh_calc_line_metadata_at(self.editor.cursor_line);
        self.mark_edited_current_line();
    }

    fn try_insert_table_cell_multiline_paste(&mut self, normalized: &str) -> bool {
        let ctx = self.session_edit_context();
        let tables = self.note_table_module_enabled();
        let Some(outcome) = self.session.apply(
            &mut self.editor,
            note_session::SessionEdit::TableCellPaste {
                text: normalized,
                tables,
                cache: &mut self.table_format_cache,
            },
            ctx,
        ) else {
            return false;
        };
        self.pending_session_edit = true;
        let delta = outcome.delta;
        self.splice_calc_line_metadata(delta.start_line, delta.old_span, delta.new_span);
        self.mark_edited_with_delta(delta);
        true
    }

    pub(super) fn insert_paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let normalized = normalize_paste(text);
        if self.try_insert_table_cell_multiline_paste(&normalized) {
            return;
        }
        let Some(outcome) =
            self.apply_session_edit(note_session::SessionEdit::PlainPaste(&normalized))
        else {
            return;
        };
        let delta = outcome.delta;
        self.splice_calc_line_metadata(delta.start_line, delta.old_span, delta.new_span);
        self.mark_edited_with_delta(delta);
    }

    pub(super) fn try_import_image_paste(
        &mut self,
        db: &crate::storage::Db,
        pasted: &str,
    ) -> Result<bool, String> {
        let tokens = tokenize_path_candidates(pasted);
        if tokens.is_empty() {
            return Ok(false);
        }
        let mut paths = Vec::new();
        for token in tokens {
            if let Some(path) = normalize_pasted_path_token(&token) {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            return Ok(false);
        }

        let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
        let mut snippets = Vec::new();
        for path in paths {
            let imported = note_sources.import_image_path_by_id(&self.active_note.id, &path)?;
            let alt = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|text| text.replace(['_', '-'], " "))
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "Image".to_string())
                .replace(']', "\\]");
            snippets.push(format!("![{alt}]({})", imported.markdown_path));
        }
        if snippets.is_empty() {
            return Ok(false);
        }
        self.insert_paste(&snippets.join("\n"));
        self.status = if snippets.len() == 1 {
            "image inserted".to_string()
        } else {
            format!("inserted {} images", snippets.len())
        };
        Ok(true)
    }

    /// Imports image bytes, such as an image read off the system clipboard,
    /// into the note and returns the markdown that shows it.
    pub(super) fn import_image_bytes(
        &self,
        db: &crate::storage::Db,
        bytes: &[u8],
    ) -> Result<String, String> {
        let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
        let imported =
            note_sources.import_image_bytes_by_id(&self.active_note.id, None, None, bytes)?;
        Ok(format!("![Image]({})", imported.markdown_path))
    }

    /// Pasted CSV or TSV as markdown table lines, unless the note has tables
    /// off, or the cursor is in a fenced code block or already in a table
    /// (where a paste fills cells).
    pub(super) fn pasted_table(&mut self, text: &str) -> Option<Vec<String>> {
        let table = parse_table_paste(text, self.current_line(), self.note_table_module_enabled())?;
        let fence = self.fence_state_before_line(self.editor.cursor_line);
        table_paste_outside_code(self.current_line(), fence).then_some(table)
    }

    /// Pastes CSV or TSV as a table: in place of a blank line, or on the
    /// lines below the cursor's.
    pub(super) fn try_paste_as_table(&mut self, text: &str) -> bool {
        let Some(table) = self.pasted_table(text) else {
            return false;
        };
        let Some(outcome) = self.apply_session_edit(note_session::SessionEdit::TableImport(&table))
        else {
            return false;
        };
        let delta = outcome.delta;
        self.splice_calc_line_metadata(delta.start_line, delta.old_span, delta.new_span);
        self.mark_edited_with_delta(delta);
        self.status = "pasted as table".to_string();
        true
    }

    pub(super) fn insert_newline(&mut self) {
        let Some(delta) = self.apply_buffer_primitive(PrimitiveEdit::Newline) else {
            return;
        };
        self.splice_calc_line_metadata(delta.start_line, delta.old_span, delta.new_span);
        self.mark_edited_with_delta(delta);
    }

    pub(super) fn variable_autocomplete_state(&self) -> Option<VariableAutocompleteState> {
        if self.mode != UiMode::Editor || !self.note_math_module_enabled() {
            return None;
        }
        let line = self.current_line();
        // Cross-note prefix takes priority: [[ID]].partial
        if let Some(crate::editor_core::completion::CrossNoteCompletionPrefix {
            note_id: dep_id,
            bracket_col,
            from_col,
            partial,
        }) = extract_cross_note_completion_prefix(line, self.editor.cursor_col)
        {
            if !self.note_variables_module_enabled() {
                return None;
            }
            let exports = cross_note_exports_for_autocomplete(
                &dep_id,
                &self.cross_note_var_index,
                &self.session.calc.engine,
                &self.cross_note_db,
            );
            // Eagerly preload dep values in the background so that when the
            // user confirms the selection and a recompute fires, the index
            // already has f64 values — avoiding a blocking DB load at that point.
            let needs_preload = self
                .cross_note_var_index
                .lock()
                .ok()
                .map(|mut idx| idx.try_claim_eval(&dep_id))
                .unwrap_or(false);
            if needs_preload {
                let bg_dep_id = dep_id.clone();
                let bg_var_index = std::sync::Arc::clone(&self.cross_note_var_index);
                let bg_condvar = std::sync::Arc::clone(&self.cross_note_eval_condvar);
                let bg_db = self.cross_note_db.clone();
                std::thread::spawn(move || {
                    let engine = app_core::calc::CalcEngine::new();
                    preload_cross_note_dep_value(&bg_dep_id, &bg_var_index, &engine, &bg_db);
                    bg_condvar.notify_all();
                });
            }
            let suggestions = crate::editor_core::completion::cross_note_suggestions(
                exports
                    .iter()
                    .map(|e| (e.normalized.as_str(), e.name.as_str())),
                &partial,
            );
            if !suggestions.is_empty() {
                return Some(VariableAutocompleteState {
                    popup_anchor_col: bracket_col,
                    from_col,
                    to_col: self.editor.cursor_col,
                    query: partial,
                    suggestions,
                });
            }
            return None;
        }

        crate::editor_core::completion::local_variable_completion(
            line,
            self.editor.cursor_col,
            self.note_table_module_enabled(),
            self.note_variables_module_enabled(),
            &self.session.calc.variable_names,
            self.variable_autocomplete_min_chars,
            VARIABLE_AUTOCOMPLETE_MAX_SUGGESTIONS,
        )
    }

    pub(super) fn variable_popup_anchor(&self, anchor_col: usize) -> Option<(usize, usize)> {
        let (rows, cols) = input::terminal_size();
        if rows <= EDITOR_TOP_ROW || cols == 0 {
            return None;
        }
        let cursor_virtual = self.current_virtual_line();
        let row = EDITOR_TOP_ROW
            + cursor_virtual
                .saturating_sub(self.view.scroll_line)
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
            viewport_col_for_display_col(display_col, line_width, self.view.scroll_col, available);
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
        let Some((anchor_row, anchor_col)) = self.variable_popup_anchor(state.popup_anchor_col)
        else {
            self.dismiss_variable_autocomplete_popup();
            return;
        };
        let previous_selection = if self.variable_autocomplete_popup.visible
            && self.variable_autocomplete_popup.cursor_line == self.editor.cursor_line
            && self.variable_autocomplete_popup.cursor_col <= self.editor.cursor_col
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
            cursor_line: self.editor.cursor_line,
            cursor_col: self.editor.cursor_col,
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

    fn replace_line_chars(
        &mut self,
        line: usize,
        range: std::ops::Range<usize>,
        text: &str,
    ) -> Option<EditDelta> {
        self.apply_session_edit(note_session::SessionEdit::LineReplace {
            line,
            range,
            text,
            preserve_cursor: true,
        })
        .map(|outcome| outcome.delta)
    }

    pub(super) fn apply_variable_autocomplete_pick(
        &mut self,
        from_col: usize,
        to_col: usize,
        pick: String,
    ) -> bool {
        let from_col = from_col.min(self.editor.cursor_col);
        let to_col = to_col.min(line_char_len(self.current_line()));
        if from_col > to_col {
            return false;
        }

        let delta = self.replace_line_chars(self.editor.cursor_line, from_col..to_col, &pick);
        self.editor.cursor_col = from_col + pick.chars().count();
        self.refresh_calc_line_metadata_at(self.editor.cursor_line);
        if let Some(delta) = delta {
            self.mark_edited_with_delta(delta);
        }
        self.status = format!("autocomplete: {pick}");
        self.dismiss_variable_autocomplete_popup();
        true
    }

    pub(super) fn apply_variable_autocomplete_popup_selection(&mut self) -> bool {
        if !self.variable_autocomplete_popup.visible
            || self.variable_autocomplete_popup.cursor_line != self.editor.cursor_line
            || self.variable_autocomplete_popup.cursor_col > self.editor.cursor_col
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

    /// Status-bar hint for the completion Tab would insert. The open popup
    /// already lists the suggestions, so the hint shows only once it is
    /// dismissed.
    pub(super) fn variable_autocomplete_status_hint(&self) -> Option<String> {
        if self.variable_autocomplete_popup.visible {
            return None;
        }
        let state = self.variable_autocomplete_state()?;
        let pick = state.suggestions.first()?;
        Some(format!("Tab: {pick}"))
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
        if self.calc_runtime.recompute_pending {
            if self.calc_runtime.viewport_only {
                let editor_height = self.editor_height();
                self.ensure_calc_for_viewport(editor_height, true);
                self.calc_runtime.recompute_pending = false;
                self.calc_runtime.recompute_due_at = None;
                self.calc_runtime.pending_viewport_pass = false;
                self.calc_runtime.pending_full_pass = false;
            } else {
                self.run_calc_recompute();
            }
        }
        let text = self.current_line().to_string();
        let Some(result) = self
            .session
            .calc
            .results
            .get(self.editor.cursor_line)
            .and_then(|value| value.clone())
        else {
            return false;
        };
        if contains_assignment_operator(&text) && !is_markdown_table_line(&text) {
            return false;
        }
        let should_reflow_table = self.note_table_module_enabled() && is_markdown_table_line(&text);

        if let Some((from_byte, to_byte)) = find_calc_segment_range(&text) {
            let delta = self.replace_line_chars(
                self.editor.cursor_line,
                char_col_at_byte(&text, from_byte)..char_col_at_byte(&text, to_byte),
                &result,
            );
            self.editor.cursor_col = self.editor.lines()[self.editor.cursor_line]
                [..from_byte.saturating_add(result.len())]
                .chars()
                .count();
            if should_reflow_table {
                self.try_autoformat_rules();
            }
            self.refresh_calc_line_metadata_at(self.editor.cursor_line);
            if let Some(delta) = delta {
                self.mark_edited_with_delta(delta);
            }
            return true;
        }

        if should_reflow_table {
            let cursor_col = self.editor.cursor_col;
            let formula = find_table_formula_segments(&text)
                .into_iter()
                .find(|seg| cursor_col >= seg.cell_from_char && cursor_col <= seg.cell_to_char)
                .or_else(|| find_table_formula_segments(&text).into_iter().next());
            if let Some(seg) = formula {
                let delta = self.replace_line_chars(
                    self.editor.cursor_line,
                    char_col_at_byte(&text, seg.from_byte)..char_col_at_byte(&text, seg.to_byte),
                    &result,
                );
                self.editor.cursor_col = self.editor.lines()[self.editor.cursor_line]
                    [..seg.from_byte.saturating_add(result.len())]
                    .chars()
                    .count();
                self.try_autoformat_rules();
                self.refresh_calc_line_metadata_at(self.editor.cursor_line);
                if let Some(delta) = delta {
                    self.mark_edited_with_delta(delta);
                }
                return true;
            }
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

    pub(super) fn scoped_rule_line_span(&self, center_line: usize) -> (usize, usize) {
        if self.editor.lines().is_empty() {
            return (0, 0);
        }
        let center = center_line.min(self.editor.lines().len().saturating_sub(1));
        if self.note_table_module_enabled() {
            if let Some(bounds) =
                crate::editor_core::table::table_block_bounds(self.editor.lines(), center)
            {
                return bounds;
            }
        }
        let window = 96usize;
        (
            center.saturating_sub(window),
            center
                .saturating_add(window)
                .min(self.editor.lines().len().saturating_sub(1)),
        )
    }

    pub(super) fn try_autoformat_rules(&mut self) {
        if !self.note_style_module_enabled() && !self.note_table_module_enabled() {
            return;
        }
        if !Self::line_might_trigger_doc_change_rules(self.current_line()) {
            return;
        }

        let (start_line, end_line) = self.scoped_rule_line_span(self.editor.cursor_line);
        let cursor_offset =
            self.byte_offset_for_line_col(self.editor.cursor_line, self.editor.cursor_col);
        let changed = crate::editor_core::types::TextRange {
            from: cursor_offset.saturating_sub(1),
            to: cursor_offset,
        };
        let (ctx, scope_start_offset) =
            self.build_scoped_context_for_line_span(start_line, end_line, Some(changed));
        let options = crate::editor_core::text_rules::TextRuleOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            checklist_auto_reorder: self.checklist_auto_reorder_enabled(),
            table_enabled: self.note_table_module_enabled(),
        };
        let started = Instant::now();
        if let Some(op) = crate::editor_core::text_rules::run_doc_change_rules_with_table_cache(
            &ctx,
            options,
            &mut self.table_format_cache,
        ) {
            let mapped = Self::remap_operation_from_scope(&op, scope_start_offset);
            self.apply_edit_operation(&mapped);
            self.record_perf_duration("tui.doc_change_rules", "applied", started.elapsed());
        } else {
            self.record_perf_duration("tui.doc_change_rules", "noop", started.elapsed());
        }
    }

    pub(super) fn try_enter_rule(&mut self) -> bool {
        let (start_line, end_line) = self.scoped_rule_line_span(self.editor.cursor_line);
        let (ctx, scope_start_offset) =
            self.build_scoped_context_for_line_span(start_line, end_line, None);
        let options = crate::editor_core::text_rules::TextRuleOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            checklist_auto_reorder: self.checklist_auto_reorder_enabled(),
            table_enabled: self.note_table_module_enabled(),
        };
        let started = Instant::now();
        if let Some(op) = crate::editor_core::text_rules::run_enter_rules(&ctx, options) {
            let mapped = Self::remap_operation_from_scope(&op, scope_start_offset);
            self.apply_edit_operation(&mapped);
            self.record_perf_duration("tui.enter_rules", "applied", started.elapsed());
            return true;
        }
        self.record_perf_duration("tui.enter_rules", "noop", started.elapsed());
        false
    }

    pub(super) fn try_tab_rule(&mut self, outdent: bool) -> bool {
        let (start_line, end_line) = self.scoped_rule_line_span(self.editor.cursor_line);
        let (ctx, scope_start_offset) =
            self.build_scoped_context_for_line_span(start_line, end_line, None);
        let options = crate::editor_core::text_rules::TabRuleOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            outdent,
            table_enabled: self.note_table_module_enabled(),
        };
        let started = Instant::now();
        if let Some(op) = crate::editor_core::text_rules::run_tab_rules(&ctx, options) {
            let mapped = Self::remap_operation_from_scope(&op, scope_start_offset);
            self.apply_edit_operation(&mapped);
            self.record_perf_duration("tui.tab_rules", "applied", started.elapsed());
            return true;
        }
        self.record_perf_duration("tui.tab_rules", "noop", started.elapsed());
        false
    }

    pub(super) fn try_table_navigation_rule(&mut self, outdent: bool) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let (start_line, end_line) = self.scoped_rule_line_span(self.editor.cursor_line);
        let (ctx, scope_start_offset) =
            self.build_scoped_context_for_line_span(start_line, end_line, None);
        let options = crate::editor_core::text_rules::TabRuleOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            outdent,
            table_enabled: true,
        };
        if let Some(op) =
            crate::editor_core::text_rules::run_table_cell_navigation_rules(&ctx, options)
        {
            let mapped = Self::remap_operation_from_scope(&op, scope_start_offset);
            self.apply_edit_operation(&mapped);
            return true;
        }
        false
    }

    pub(super) fn try_table_multiline_break_rule(&mut self) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let (start_line, end_line) = self.scoped_rule_line_span(self.editor.cursor_line);
        let (ctx, scope_start_offset) =
            self.build_scoped_context_for_line_span(start_line, end_line, None);
        let op = crate::editor_core::text_rules::run_table_multiline_break_rule_with_table_cache(
            &ctx,
            true,
            &mut self.table_format_cache,
        );
        let Some(op) = op else {
            return false;
        };
        let mapped = Self::remap_operation_from_scope(&op, scope_start_offset);
        self.apply_edit_operation(&mapped);
        true
    }

    /// Runs a table text rule scoped to the cursor line's block and applies
    /// its edit. Returns whether the rule fired.
    fn try_scoped_table_rule(
        &mut self,
        rule: fn(
            &crate::editor_core::context::ResolvedContext<'_>,
        ) -> Option<crate::editor_core::types::EditOperation>,
    ) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let (start_line, end_line) = self.scoped_rule_line_span(self.editor.cursor_line);
        let (ctx, scope_start_offset) =
            self.build_scoped_context_for_line_span(start_line, end_line, None);
        let Some(op) = rule(&ctx) else {
            return false;
        };
        let mapped = Self::remap_operation_from_scope(&op, scope_start_offset);
        self.apply_edit_operation(&mapped);
        true
    }

    pub(super) fn try_table_manual_row_start_rule(&mut self) -> bool {
        self.try_scoped_table_rule(crate::editor_core::text_rules::run_table_manual_row_start_rule)
    }

    pub(super) fn try_table_duplicate_delimiter_rule(&mut self) -> bool {
        let fired = self.try_scoped_table_rule(
            crate::editor_core::text_rules::run_table_duplicate_delimiter_rule,
        );
        if fired {
            // End of the kept delimiter row, so Enter starts the first data row
            // (the padding guard in apply would pull it into the last cell).
            self.editor.cursor_col = line_char_len(self.current_line());
        }
        fired
    }

    pub(super) fn try_table_pipe_insert_column_rule(&mut self) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let (start_line, end_line) = self.scoped_rule_line_span(self.editor.cursor_line);
        let (ctx, scope_start_offset) =
            self.build_scoped_context_for_line_span(start_line, end_line, None);
        if let Some(op) =
            crate::editor_core::text_rules::run_table_pipe_insert_column_rule_with_table_cache(
                &ctx,
                &mut self.table_format_cache,
            )
        {
            let mapped = Self::remap_operation_from_scope(&op, scope_start_offset);
            self.apply_edit_operation(&mapped);
            return true;
        }
        false
    }

    pub(super) fn try_table_header_delete_column_rule(&mut self) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let (start_line, end_line) = self.scoped_rule_line_span(self.editor.cursor_line);
        let (ctx, scope_start_offset) =
            self.build_scoped_context_for_line_span(start_line, end_line, None);
        if let Some(op) =
            crate::editor_core::text_rules::run_table_header_delete_column_rule_with_table_cache(
                &ctx,
                &mut self.table_format_cache,
            )
        {
            let mapped = Self::remap_operation_from_scope(&op, scope_start_offset);
            self.apply_edit_operation(&mapped);
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
        let (start_line, end_line) = self.scoped_rule_line_span(self.editor.cursor_line);
        let (ctx, scope_start_offset) =
            self.build_scoped_context_for_line_span(start_line, end_line, None);
        let options = crate::editor_core::text_rules::TableBoundaryEditOptions {
            markdown_autoformat: self.markdown_autoformat_enabled(),
            backward,
            structural_merge,
            table_enabled: true,
        };
        let op = crate::editor_core::text_rules::run_table_boundary_edit_rules(&ctx, options)?;
        let changed = !op.changes.is_empty();
        let mapped = Self::remap_operation_from_scope(&op, scope_start_offset);
        self.apply_edit_operation(&mapped);
        if changed {
            self.prune_empty_table_continuation_row_at_cursor();
        }
        Some(changed)
    }

    /// Clamps the cursor after applying `op`. An explicit selection is where
    /// the operation meant the cursor to be (e.g. the gap `ciw` leaves in a
    /// table cell), so only an inferred position is kept out of cell padding.
    fn adjust_cursor_after_operation(&mut self, op: &crate::editor_core::types::EditOperation) {
        self.adjust_cursor_with_table_padding_guard(op.selection.is_none());
    }

    pub(super) fn apply_edit_operation(&mut self, op: &crate::editor_core::types::EditOperation) {
        if !self.active_note_is_editable() && !op.changes.is_empty() {
            self.set_locked_note_status();
            return;
        }
        let outcome = self.apply_session_edit(note_session::SessionEdit::Operation(op));
        if let Some(outcome) = outcome {
            let delta = outcome.delta;
            if outcome.calc_splices.is_empty() {
                self.splice_calc_line_metadata(delta.start_line, delta.old_span, delta.new_span);
            } else {
                // Compound changes already own an allocated plan; derive final metadata
                // once rather than reading intermediate buffers after the transaction.
                self.rebuild_calc_line_metadata();
            }
            self.folds.rescan_pending |= outcome.fold_rescan;
            self.mark_edited_with_delta(delta);
        }
        self.adjust_cursor_after_operation(op);
        self.adjust_scroll();
    }

    fn prune_empty_table_continuation_row_at_cursor(&mut self) -> bool {
        if !self.note_table_module_enabled() || self.editor.lines().is_empty() {
            return false;
        }
        if self.editor.cursor_line >= self.editor.lines().len() {
            return false;
        }
        if !crate::editor_core::table::is_empty_table_continuation_row(self.current_line()) {
            return false;
        }

        let remove_line = self.editor.cursor_line;
        let previous_col = self.editor.cursor_col;
        let delta = self
            .apply_session_edit(note_session::SessionEdit::RemoveLines {
                start: remove_line,
                end: remove_line + 1,
            })
            .expect("existing continuation row")
            .delta;
        if delta.new_span == 1 {
            self.editor.cursor_line = 0;
            self.editor.cursor_col = 0;
        } else {
            self.editor.cursor_line = remove_line
                .saturating_sub(1)
                .min(self.editor.lines().len() - 1);
            self.editor.cursor_col = previous_col.min(line_char_len(self.current_line()));
            if self.note_table_module_enabled() {
                if let Some(cell) = table_cell_info_at_char(
                    self.editor.lines(),
                    self.editor.cursor_line,
                    self.editor.cursor_col,
                ) {
                    self.editor.cursor_col =
                        table_cell_navigation_anchor(self.current_line(), &cell);
                }
            }
        }

        self.splice_calc_line_metadata(delta.start_line, delta.old_span, delta.new_span);
        self.mark_edited_from_line_with_span(remove_line.saturating_sub(1), Some(delta));
        true
    }

    /// Backspace/Delete inside a table cell: removes only the cell's text
    /// (see `table::plan_table_char_delete`). False when not in a cell.
    fn try_table_char_delete(&mut self, backward: bool) -> bool {
        use crate::editor_core::table::{plan_table_char_delete, TableCharDelete};
        if !self.note_table_module_enabled() {
            return false;
        }
        let Some(plan) =
            plan_table_char_delete(self.current_line(), self.editor.cursor_col, backward)
        else {
            return false;
        };
        match plan {
            TableCharDelete::Stay { cursor } => self.editor.cursor_col = cursor,
            TableCharDelete::Remove { at, cursor } => {
                self.replace_line_chars(self.editor.cursor_line, at..at + 1, "");
                self.editor.cursor_col = cursor;
                self.refresh_calc_line_metadata_at(self.editor.cursor_line);
                self.mark_edited_current_line();
            }
        }
        self.prune_empty_table_continuation_row_at_cursor();
        true
    }

    pub(super) fn backspace(&mut self) {
        if self.try_table_char_delete(true) {
            return;
        }
        let Some(delta) = self.apply_buffer_primitive(PrimitiveEdit::Backspace) else {
            return;
        };
        if delta.old_span == delta.new_span {
            self.refresh_calc_line_metadata_at(self.editor.cursor_line);
            self.mark_edited_current_line();
            self.prune_empty_table_continuation_row_at_cursor();
            return;
        }
        self.splice_calc_line_metadata(delta.start_line, delta.old_span, delta.new_span);
        self.mark_edited_with_delta(delta);
    }

    pub(super) fn delete_forward(&mut self) {
        if self.try_table_char_delete(false) {
            return;
        }
        let Some(delta) = self.apply_buffer_primitive(PrimitiveEdit::DeleteForward) else {
            return;
        };
        if delta.old_span == delta.new_span {
            self.refresh_calc_line_metadata_at(self.editor.cursor_line);
            self.mark_edited_current_line();
            self.prune_empty_table_continuation_row_at_cursor();
            return;
        }
        self.splice_calc_line_metadata(delta.start_line, delta.old_span, delta.new_span);
        self.mark_edited_with_delta(delta);
    }

    pub(super) fn try_shared_table_cursor_motion(
        &mut self,
        direction: crate::editor_core::table::TableCursorMotionDirection,
    ) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let Some((block_start, block_end)) = crate::editor_core::table::table_block_bounds(
            self.editor.lines(),
            self.editor.cursor_line,
        ) else {
            return false;
        };
        let block_lines = &self.editor.lines()[block_start..=block_end];
        let Some(target) = crate::editor_core::table::plan_table_cursor_motion(
            block_lines,
            self.editor.cursor_line.saturating_sub(block_start),
            self.editor.cursor_col,
            direction,
        ) else {
            return false;
        };

        self.view.markdown_formatting_right_boundary_exit = None;
        if target.line_index < 0 {
            if let Some(prev_line) = self.visible_line_before(block_start) {
                self.editor.cursor_line = prev_line;
                let exit_col = self.table_cursor_motion_exit_col(direction, target.col);
                self.editor.cursor_col = exit_col.min(line_char_len(self.current_line()));
            }
            return true;
        }

        let block_len = block_end.saturating_sub(block_start).saturating_add(1);
        let Ok(target_index) = usize::try_from(target.line_index) else {
            return false;
        };
        if target_index >= block_len {
            if let Some(next_line) = self.visible_line_after(block_end) {
                self.editor.cursor_line = next_line;
                let exit_col = self.table_cursor_motion_exit_col(direction, target.col);
                self.editor.cursor_col = exit_col.min(line_char_len(self.current_line()));
            }
            return true;
        }

        self.editor.cursor_line = block_start + target_index;
        self.editor.cursor_col = target.col.min(line_char_len(self.current_line()));
        self.clamp_cursor_to_line_bounds();
        true
    }

    fn table_cursor_motion_exit_col(
        &self,
        direction: crate::editor_core::table::TableCursorMotionDirection,
        planner_col: usize,
    ) -> usize {
        match direction {
            crate::editor_core::table::TableCursorMotionDirection::Up
            | crate::editor_core::table::TableCursorMotionDirection::Down => self.editor.cursor_col,
            crate::editor_core::table::TableCursorMotionDirection::Left
            | crate::editor_core::table::TableCursorMotionDirection::Right => planner_col,
        }
    }

    fn visible_line_before(&self, real_line: usize) -> Option<usize> {
        let visible = self
            .folds
            .real_to_visible
            .get(real_line)
            .copied()
            .unwrap_or_else(|| self.current_virtual_line());
        visible
            .checked_sub(1)
            .and_then(|virtual_line| self.real_line_for_virtual(virtual_line))
    }

    fn visible_line_after(&self, real_line: usize) -> Option<usize> {
        let mut visible = self
            .folds
            .real_to_visible
            .get(real_line)
            .copied()
            .unwrap_or_else(|| self.current_virtual_line());
        while visible + 1 < self.visible_line_count() {
            visible += 1;
            if let Some(next_real) = self.real_line_for_virtual(visible) {
                if next_real > real_line {
                    return Some(next_real);
                }
            }
        }
        None
    }

    pub(super) fn move_cursor_left(&mut self) {
        if self.view.markdown_formatting_right_boundary_exit
            == Some((self.editor.cursor_line, self.editor.cursor_col))
        {
            self.view.markdown_formatting_right_boundary_exit = None;
            return;
        }
        self.view.markdown_formatting_right_boundary_exit = None;

        if self.try_shared_table_cursor_motion(
            crate::editor_core::table::TableCursorMotionDirection::Left,
        ) {
            return;
        }

        let current_virtual = self.current_virtual_line();
        let neighbour = current_virtual
            .checked_sub(1)
            .and_then(|idx| self.real_line_for_virtual(idx));
        let cursor = crate::editor_core::vim_actions::buffer::horizontal_motion(
            self.editor.lines(),
            self.editor.cursor(),
            false,
            neighbour,
        );
        self.editor.set_cursor(cursor);
    }

    pub(super) fn move_cursor_right(&mut self) {
        let consumed_boundary_exit = self.view.markdown_formatting_right_boundary_exit
            == Some((self.editor.cursor_line, self.editor.cursor_col));
        if consumed_boundary_exit {
            self.view.markdown_formatting_right_boundary_exit = None;
        }

        let boundary_exit_anchor = if !consumed_boundary_exit
            && crate::terminal::markdown_view::formatting_component_right_boundary_at(
                self.current_line(),
                self.editor.cursor_col,
            ) {
            Some((self.editor.cursor_line, self.editor.cursor_col))
        } else {
            None
        };

        if self.try_shared_table_cursor_motion(
            crate::editor_core::table::TableCursorMotionDirection::Right,
        ) {
            return;
        }

        let current_virtual = self.current_virtual_line();
        let neighbour = self.real_line_for_virtual(current_virtual + 1);
        let cursor = crate::editor_core::vim_actions::buffer::horizontal_motion(
            self.editor.lines(),
            self.editor.cursor(),
            true,
            neighbour,
        );
        self.view.markdown_formatting_right_boundary_exit =
            if cursor.line == self.editor.cursor_line {
                boundary_exit_anchor
            } else {
                None
            };
        self.editor.set_cursor(cursor);
    }

    /// Cell positions of the cursor line's chars (see
    /// `RenderContext::wrap_char_positions`) when it soft-wraps and its display
    /// text is the source text; `None` when screen rows cannot be mapped back.
    fn cursor_line_wrap_positions(&mut self) -> Option<Vec<Option<(usize, usize)>>> {
        if !self.cursor_line_wraps() {
            return None;
        }
        let line_idx = self.editor.cursor_line;
        let display =
            self.prepare_display_line(line_idx, crate::terminal::notifications::now_epoch_ms());
        if display.text != self.editor.lines()[line_idx] {
            return None;
        }
        let (_, cols) = input::terminal_size();
        let width = cols.saturating_sub(self.gutter_width()).max(1);
        let mut ctx = self.render_context_at(line_idx);
        let (positions, _) = ctx.wrap_char_positions(
            &display.text,
            width,
            &display.decorations(&self.session.calc.variable_names),
        );
        Some(positions)
    }

    /// Moves the cursor `count` screen rows up or down (`gk` / `gj`, and
    /// arrow keys on wrapped lines), keeping its screen column. Falls back to
    /// logical line moves where lines do not wrap.
    pub(super) fn move_cursor_screen(&mut self, up: bool, count: usize) {
        let allow_end = self.mode == UiMode::Editor;
        for _ in 0..count {
            let Some(positions) = self.cursor_line_wrap_positions() else {
                if up {
                    self.move_cursor_up(1);
                } else {
                    self.move_cursor_down(1);
                }
                continue;
            };
            let col = self.editor.cursor_col.min(positions.len() - 1);
            let (row, screen_col) = positions[col..]
                .iter()
                .flatten()
                .next()
                .copied()
                .unwrap_or((0, 0));
            let rows = positions
                .iter()
                .flatten()
                .map(|(r, _)| r + 1)
                .max()
                .unwrap_or(1);
            let target_row = if up {
                row.checked_sub(1)
            } else {
                (row + 1 < rows).then_some(row + 1)
            };
            if let Some(target_row) = target_row {
                self.editor.cursor_col =
                    column_on_screen_row(&positions, target_row, screen_col, allow_end);
                continue;
            }
            let before = self.editor.cursor_line;
            if up {
                self.move_cursor_up(1);
            } else {
                self.move_cursor_down(1);
            }
            if self.editor.cursor_line == before {
                break;
            }
            match self.cursor_line_wrap_positions() {
                Some(next) => {
                    let next_rows = next.iter().flatten().map(|(r, _)| r + 1).max().unwrap_or(1);
                    let row = if up { next_rows - 1 } else { 0 };
                    self.editor.cursor_col =
                        column_on_screen_row(&next, row, screen_col, allow_end);
                }
                None => self.editor.cursor_col = screen_col,
            }
        }
        self.view.markdown_formatting_right_boundary_exit = None;
        self.clamp_cursor_to_line_bounds();
    }

    pub(super) fn move_cursor_up(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.view.markdown_formatting_right_boundary_exit = None;
        let current_virtual = self.current_virtual_line();
        let target_virtual = current_virtual.saturating_sub(count);
        self.editor.cursor_line = self.real_line_for_virtual(target_virtual).unwrap_or(0);
        self.clamp_cursor_to_line_bounds();
    }

    pub(super) fn move_cursor_down(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.view.markdown_formatting_right_boundary_exit = None;
        let current_virtual = self.current_virtual_line();
        let target_virtual = min(
            current_virtual.saturating_add(count),
            self.visible_line_count().saturating_sub(1),
        );
        self.editor.cursor_line = self
            .real_line_for_virtual(target_virtual)
            .unwrap_or_else(|| self.editor.lines().len().saturating_sub(1));
        self.clamp_cursor_to_line_bounds();
    }

    fn adjust_cursor_line_and_col_bounds(&mut self) {
        if self.editor.lines().is_empty() {
            self.apply_session_edit(note_session::SessionEdit::EnsureBuffer);
        }
        if self.editor.cursor_line >= self.editor.lines().len() {
            self.editor.cursor_line = self.editor.lines().len() - 1;
        }
        if let Some(owner) = self.fold_hidden_owner_for_line(self.editor.cursor_line) {
            self.editor.cursor_line = owner.min(self.editor.lines().len().saturating_sub(1));
        }
        let len = line_char_len(self.current_line());
        if self.editor.cursor_col > len {
            self.editor.cursor_col = len;
        }
    }

    pub(super) fn clamp_cursor_to_line_bounds(&mut self) {
        self.adjust_cursor_line_and_col_bounds();
    }

    /// True when the cursor line is a table row with fewer cells than the
    /// table's header row, i.e. a row still being typed by hand.
    pub(super) fn adjust_cursor_with_table_padding_guard(&mut self, clamp_table_padding: bool) {
        self.adjust_cursor_line_and_col_bounds();
        let table_anchor = if self.note_table_module_enabled() {
            let line_text = self.current_line();
            if let Some(cell) = table_cell_info_at_char(
                self.editor.lines(),
                self.editor.cursor_line,
                self.editor.cursor_col,
            ) {
                let anchor = table_cell_navigation_anchor(line_text, &cell);
                let edit_start = table_cell_edit_start(&cell);
                if table_cell_is_empty(&cell) {
                    Some(anchor)
                } else if self.editor.cursor_col < edit_start
                    || (clamp_table_padding && self.editor.cursor_col > anchor)
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
            self.editor.cursor_col = anchor;
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

    /// True when soft wrap is on and the cursor line wraps rather than
    /// scrolling horizontally.
    pub(super) fn cursor_line_wraps(&mut self) -> bool {
        if !self.render_state.wrap_lines {
            return false;
        }
        let ctx = self.render_context_at(self.editor.cursor_line);
        let line = self.current_line().to_string();
        super::rendering::line_wraps(&ctx, &line)
    }

    pub(super) fn adjust_scroll(&mut self) {
        let height = self.editor_height();
        let cursor_virtual = self.current_virtual_line();
        if cursor_virtual < self.view.scroll_line {
            self.view.scroll_line = cursor_virtual;
        } else if cursor_virtual >= self.view.scroll_line + height {
            self.view.scroll_line = cursor_virtual + 1 - height;
        }
        self.view.scroll_line = self
            .view
            .scroll_line
            .min(self.visible_line_count().saturating_sub(1));

        let (_, cols) = input::terminal_size();
        let available = cols.saturating_sub(self.gutter_width());
        if available == 0 || self.cursor_line_wraps() {
            self.view.scroll_col = 0;
            return;
        }

        let (cursor_display_col, max_scroll) = {
            let line_text = self.current_line();
            let line_len = line_char_len(line_text);
            let logical_col = min(self.editor.cursor_col, line_len);
            let render_col = cursor_render_char_col(
                line_text,
                self.editor.cursor_col,
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

        if cursor_display_col < self.view.scroll_col {
            self.view.scroll_col =
                cursor_display_col.saturating_sub(HORIZONTAL_SCROLL_LEFT_CONTEXT);
        } else if cursor_display_col >= self.view.scroll_col + available {
            self.view.scroll_col = cursor_display_col + 1 - available;
        }

        self.view.scroll_col = self.view.scroll_col.min(max_scroll);
    }

    pub(super) fn calc_eval_range_for_viewport(
        &self,
        editor_height: usize,
    ) -> Option<(usize, usize)> {
        if self.editor.lines().is_empty() || editor_height == 0 {
            return None;
        }
        let visible_count = self.visible_line_count();
        if visible_count == 0 {
            return None;
        }
        let prefetch = editor_height.saturating_mul(CALC_VIEWPORT_PREFETCH_MULTIPLIER);
        let start_virtual = self.view.scroll_line.saturating_sub(prefetch);
        let end_virtual = self
            .view
            .scroll_line
            .saturating_add(editor_height)
            .saturating_add(prefetch)
            .min(visible_count.saturating_sub(1));
        let start_line = self.real_line_for_virtual(start_virtual).unwrap_or(0);
        let end_line_inclusive = self
            .real_line_for_virtual(end_virtual)
            .unwrap_or_else(|| self.editor.lines().len().saturating_sub(1));
        let end_line_exclusive = end_line_inclusive
            .saturating_add(1)
            .min(self.editor.lines().len());
        if start_line >= end_line_exclusive {
            None
        } else {
            Some((start_line, end_line_exclusive))
        }
    }

    /// Ensure f64 export values for every cross-note dep in the current note
    /// are in the index. Guarded by `was_full_eval_attempted` so it's O(1)
    /// after the first call per dep per session.
    fn preload_cross_note_deps(&self) {
        let refs = app_core::calc::scan_cross_note_refs(self.editor.lines());
        self.preload_cross_note_deps_for_refs(&refs);
    }

    fn preload_cross_note_deps_for_refs(&self, refs: &[app_core::calc::CrossNoteRef]) {
        if refs.is_empty() {
            return;
        }
        // Partition deps: truly missing (need sync load) vs. in-flight (bg thread
        // is already loading them). For in-flight deps we do a short bounded wait
        // so the recompute following a Tab press can still get correct values when
        // the autocomplete background thread is nearly done.
        let (missing, in_flight): (Vec<String>, Vec<String>) = {
            let dep_ids: rustc_hash::FxHashSet<String> =
                refs.iter().map(|r| r.note_id.clone()).collect();
            match self.cross_note_var_index.lock() {
                Ok(index) => {
                    let mut missing = Vec::new();
                    let mut in_flight = Vec::new();
                    for sid in dep_ids {
                        if index.was_full_eval_attempted(&sid) {
                            // already done
                        } else if index.is_eval_done_or_in_flight(&sid) {
                            in_flight.push(sid);
                        } else {
                            missing.push(sid);
                        }
                    }
                    (missing, in_flight)
                }
                Err(_) => (Vec::new(), Vec::new()),
            }
        };

        // Park the event-loop thread until all in-flight background evals signal
        // completion (or until the 200 ms deadline). The background thread calls
        // cross_note_eval_condvar.notify_all() after mark_full_eval_attempted fires,
        // so we wake up as soon as the data is ready instead of burning fixed intervals.
        if !in_flight.is_empty() {
            if let Ok(lock) = self.cross_note_var_index.lock() {
                let _ = self.cross_note_eval_condvar.wait_timeout_while(
                    lock,
                    Duration::from_millis(200),
                    |index| {
                        in_flight
                            .iter()
                            .any(|sid| !index.was_full_eval_attempted(sid))
                    },
                );
            }
        }

        // Sync-load any deps that have no background thread covering them.
        for dep_id in missing {
            preload_cross_note_dep_value(
                &dep_id,
                &self.cross_note_var_index,
                &self.session.calc.engine,
                &self.cross_note_db,
            );
        }
    }

    pub(super) fn recompute_calc_range(&mut self, eval_from: usize, eval_to: usize) {
        if !self.note_math_module_enabled() {
            self.clear_calc_cache();
            return;
        }
        if eval_from >= eval_to || eval_to > self.editor.lines().len() {
            return;
        }
        // The dependency index only supplies variable names here; syncing it
        // walks the whole note, so leave that to the idle tick.
        self.calc_runtime.index_sync_pending = true;
        let vars_enabled = self.calc_variables_enabled();
        let cross_note_enabled = self.calc_cross_note_enabled();
        let extern_vars: Vec<ExternVar> = if cross_note_enabled {
            let note_id = self.active_note.id.clone();
            // Viewport evaluations repeat on every scroll step and edit;
            // rescan only the lines that changed since the last scan.
            let generation = self.editor.text_generation;
            let (line_hashes, refs) = self.session.calc.cross_note_refs(&self.editor);
            self.preload_cross_note_deps_for_refs(&refs);
            let extern_vars = if let Ok(mut index) = self.cross_note_var_index.lock() {
                index.update_deps(&note_id, &refs);
                index.extern_vars_for(&note_id)
            } else {
                Vec::new()
            };
            self.session.calc.cross_note_refs_scan = Some((line_hashes, refs));
            self.session.calc.cross_note_refs_generation = Some(generation);
            extern_vars
        } else {
            Vec::new()
        };
        self.session.calc.evaluate_range(
            &self.editor,
            eval_from,
            eval_to,
            app_core::calc::NoteEvaluationOptions {
                variables_enabled: vars_enabled,
                cross_note_enabled,
                table_enabled: self.note_table_module_enabled(),
                extern_vars,
                ..Default::default()
            },
        );
    }

    /// Starts preparing a large viewport note's calc off the input thread:
    /// cross-note refs and values, then the whole-note calc context. The
    /// note paints without calc ghosts until `poll_viewport_calc_preparation`
    /// installs it. False when the note is small enough to prepare inline.
    pub(super) fn start_viewport_calc_preparation(&mut self) -> bool {
        if !self.calc_runtime.viewport_only
            || !self.note_math_module_enabled()
            || self.editor.lines().len() < super::CALC_BACKGROUND_PREPARE_MIN_LINES
        {
            return false;
        }
        let lines = self.editor.lines().to_vec();
        let note_id = self.active_note.id.clone();
        let variables_enabled = self.calc_variables_enabled();
        let cross_note_enabled = self.calc_cross_note_enabled();
        let table_enabled = self.note_table_module_enabled();
        let index = std::sync::Arc::clone(&self.cross_note_var_index);
        let index_epoch = index.lock().map(|index| index.epoch()).unwrap_or_default();
        let db = self.cross_note_db.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let engine = app_core::calc::CalcEngine::new();
            let (extern_vars, refs_scan) = if cross_note_enabled {
                let refs = app_core::calc::scan_cross_note_refs(&lines);
                let dep_ids: rustc_hash::FxHashSet<&str> =
                    refs.iter().map(|r| r.note_id.as_str()).collect();
                for dep_id in dep_ids {
                    preload_cross_note_dep_value(dep_id, &index, &engine, &db);
                }
                let extern_vars = match index.lock() {
                    // Not after a restore reset the index: these lines are
                    // from the replaced database.
                    Ok(mut index) if index.epoch() == index_epoch => {
                        index.update_deps(&note_id, &refs);
                        index.extern_vars_for(&note_id)
                    }
                    _ => Vec::new(),
                };
                let hashes = crate::editor_core::calc_plan::hash_lines(&lines);
                (extern_vars, Some((hashes, refs)))
            } else {
                (Vec::new(), None)
            };
            let options = app_core::calc::NoteEvaluationOptions {
                variables_enabled,
                cross_note_enabled,
                table_enabled,
                extern_vars,
                ..Default::default()
            };
            let context = engine.prepare_note_context(&lines, &options);
            let _ = tx.send(crate::terminal::calc_cache::RangeContextBuild {
                note_id,
                context,
                refs_scan,
            });
        });
        self.calc_workers.range_context_build = Some(rx);
        true
    }

    /// Takes over a finished background calc preparation; false while it
    /// still runs. A build for a note that is no longer open is dropped.
    fn install_viewport_calc_preparation(&mut self) -> bool {
        let Some(rx) = self.calc_workers.range_context_build.as_ref() else {
            return true;
        };
        let build = match rx.try_recv() {
            Ok(build) => build,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.calc_workers.range_context_build = None;
                return true;
            }
        };
        self.calc_workers.range_context_build = None;
        if build.note_id == self.active_note.id {
            self.session.calc.range_context = build.context;
            if build.refs_scan.is_some() {
                self.session.calc.cross_note_refs_scan = build.refs_scan;
                // Built from a snapshot; check it against the text once.
                self.session.calc.cross_note_refs_generation = None;
            }
        }
        self.calc_runtime.last_view_eval_range = None;
        true
    }

    /// Event-loop hook: once the background preparation lands, evaluates
    /// the viewport (now cheap) and repaints.
    pub(super) fn poll_viewport_calc_preparation(&mut self) {
        if self.calc_workers.range_context_build.is_none()
            || !self.install_viewport_calc_preparation()
        {
            return;
        }
        let editor_height = self.editor_height();
        self.ensure_calc_for_viewport(editor_height, true);
        self.render_state.dirty = true;
    }

    /// Catches the dependency index up with the note after viewport
    /// evaluations skipped it, and refreshes the variable names it supplies.
    pub(super) fn maybe_sync_calc_index_after_idle(&mut self) {
        if !self.calc_runtime.index_sync_pending {
            return;
        }
        if self.calc_workers.index_build.is_some() {
            if !self.install_background_calc_index() {
                return;
            }
        } else if self.last_edit.elapsed() < self.calc_recompute_debounce_duration() {
            return;
        }
        if !self.note_math_module_enabled() {
            self.calc_runtime.index_sync_pending = false;
            return;
        }
        let mask = self.calc_feature_mask();
        if self.calc_runtime.viewport_only
            && (self.session.calc.calc_dependency_index.is_none()
                || self.session.calc.line_metadata.is_empty())
        {
            // The first build reads every line; do it off the input thread
            // and catch up with any edits made meanwhile when it lands.
            let lines = self.editor.lines().to_vec();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let index =
                    crate::editor_core::calc_plan::build_calc_dependency_index(&lines, mask);
                let metadata =
                    crate::editor_core::calc_plan::line_metadata_for_lines_with_mask(&lines, mask);
                let _ = tx.send((index, metadata));
            });
            self.calc_workers.index_build = Some(rx);
            return;
        }
        self.calc_runtime.index_sync_pending = false;
        if self.calc_runtime.viewport_only
            && self.session.calc.results.len() == self.editor.lines().len()
        {
            // Lets later structural edits shift results instead of clearing them.
            crate::editor_core::calc_plan::sync_line_metadata(
                &mut self.session.calc.line_metadata,
                self.editor.lines(),
                mask,
            );
        }
        crate::editor_core::calc_plan::sync_calc_dependency_index(
            &mut self.session.calc.calc_dependency_index,
            self.editor.lines(),
            0,
            self.editor.lines().len(),
            mask,
        );
        let Some(revision) = self
            .session
            .calc
            .calc_dependency_index
            .as_ref()
            .map(|index| index.revision())
        else {
            return;
        };
        if self.session.calc.variable_names.source_revision() == Some(revision) {
            return;
        }
        let variable_names =
            crate::editor_core::calc_plan::variable_names_from_calc_dependency_index(
                self.session.calc.calc_dependency_index.as_ref(),
            );
        // An empty list is applied too: deleting the last assignment must
        // drop its name.
        self.session
            .calc
            .variable_names
            .set_from_revision(variable_names, revision);
        self.render_state.dirty = true;
    }

    /// Installs a finished background index build; false while it runs.
    /// Anything built meanwhile on the input thread is kept instead, and
    /// metadata for lines edited since the build started is re-derived.
    fn install_background_calc_index(&mut self) -> bool {
        let Some(rx) = self.calc_workers.index_build.as_ref() else {
            return true;
        };
        let (index, metadata) = match rx.try_recv() {
            Ok(built) => built,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.calc_workers.index_build = None;
                return true;
            }
        };
        self.calc_workers.index_build = None;
        if self.session.calc.calc_dependency_index.is_none() {
            self.session.calc.calc_dependency_index = index;
        }
        if self.session.calc.line_metadata.is_empty() {
            self.session.calc.line_metadata = metadata;
            let mask = self.calc_feature_mask();
            crate::editor_core::calc_plan::sync_line_metadata(
                &mut self.session.calc.line_metadata,
                self.editor.lines(),
                mask,
            );
            for (idx, line) in self.editor.lines().iter().enumerate() {
                let hash = crate::editor_core::calc_plan::hash_line(line);
                if self.session.calc.line_metadata[idx].hash != hash {
                    self.session.calc.line_metadata[idx] =
                        crate::editor_core::calc_plan::line_metadata_with_mask(line, mask);
                }
            }
        }
        true
    }

    // --- Wiki-link autocomplete ---

    fn load_wiki_link_heading_suggestions(
        db: &crate::storage::Db,
        note_id: &str,
    ) -> Vec<WikiLinkSuggestion> {
        let Ok(Some(note)) = db.get_note(note_id) else {
            return Vec::new();
        };
        crate::editor_core::markdown_tokens::extract_markdown_headings(&note.body)
            .into_iter()
            .map(|heading| WikiLinkSuggestion {
                note_id: note_id.to_string(),
                title: heading.clone(),
                title_lower: heading.to_lowercase(),
                heading: Some(heading),
            })
            .collect()
    }

    pub(super) fn rebuild_wiki_link_note_suggestions_cache(&mut self) {
        self.wiki_link_note_suggestions_cache = self
            .switcher
            .items
            .iter()
            .cloned()
            .filter(|n| n.access_mode == app_core::storage::NoteAccessMode::None || n.is_unlocked)
            .map(|n| {
                let title = if n.title.is_empty() {
                    "Untitled".to_string()
                } else {
                    n.title
                };
                let title_lower = title.to_lowercase();
                WikiLinkSuggestion {
                    note_id: n.id,
                    title,
                    title_lower,
                    heading: None,
                }
            })
            .collect();
    }

    fn ensure_wiki_link_sources_loaded(&mut self, db: &crate::storage::Db) {
        if !self.switcher.items.is_empty() {
            return;
        }
        if let Ok(items) = crate::terminal::switcher::load_note_meta_filtered(
            db,
            Some(&self.active_note.id),
            self.working_collection_id.as_deref(),
        ) {
            self.switcher.items = items;
            self.rebuild_wiki_link_index();
            self.rebuild_wiki_link_note_suggestions_cache();
        }
    }

    pub(super) fn dismiss_wiki_link_autocomplete(&mut self) {
        self.wiki_link_autocomplete_popup = WikiLinkAutocompletePopupState::default();
    }

    fn cleanup_pending_wiki_link_heading_prompt(&mut self) {
        let Some(note_id) = self
            .wiki_link_autocomplete_popup
            .pending_heading_note_id
            .clone()
        else {
            return;
        };
        let line_idx = self.wiki_link_autocomplete_popup.cursor_line;
        if line_idx >= self.editor.lines().len() {
            return;
        }
        let from_col = self.wiki_link_autocomplete_popup.from_col;
        let line = self.editor.lines()[line_idx].clone();
        let chars: Vec<char> = line.chars().collect();
        let hash_col = from_col + 2 + note_id.chars().count();
        if hash_col >= chars.len() || chars[hash_col] != '#' {
            return;
        }
        let mut end_col = hash_col;
        while end_col + 1 < chars.len() {
            if chars[end_col] == ']' && chars[end_col + 1] == ']' {
                break;
            }
            end_col += 1;
        }
        if end_col + 1 >= chars.len() || chars[end_col] != ']' || chars[end_col + 1] != ']' {
            return;
        }
        let from_byte = byte_index(&line, hash_col);
        let to_byte = byte_index(&line, end_col);
        if from_byte == to_byte {
            return;
        }
        let delta = self.replace_line_chars(line_idx, hash_col..end_col, "");
        if self.editor.cursor_line == line_idx {
            self.editor.cursor_col = hash_col;
        }
        self.refresh_calc_line_metadata_at(line_idx);
        if let Some(delta) = delta {
            self.mark_edited_with_delta(delta);
        }
    }

    pub(super) fn cancel_wiki_link_autocomplete(&mut self) {
        self.cleanup_pending_wiki_link_heading_prompt();
        self.dismiss_wiki_link_autocomplete();
    }

    pub(super) fn open_wiki_link_autocomplete(&mut self, db: &crate::storage::Db) {
        self.ensure_wiki_link_sources_loaded(db);
        let from_col = self.editor.cursor_col.saturating_sub(2);
        let note_suggestions = self.wiki_link_note_suggestions_cache.clone();
        let suggestions = note_suggestions.clone();
        let (anchor_row, anchor_col) = self.variable_popup_anchor(from_col).unwrap_or((
            self.editor
                .cursor_line
                .saturating_sub(self.view.scroll_line)
                + super::EDITOR_TOP_ROW,
            from_col.saturating_add(1),
        ));
        self.wiki_link_autocomplete_popup = WikiLinkAutocompletePopupState {
            visible: true,
            anchor_row,
            anchor_col,
            from_col,
            query: String::new(),
            pending_heading_note_id: None,
            note_suggestions,
            heading_cache: rustc_hash::FxHashMap::default(),
            suggestions,
            selected_index: 0,
            cursor_line: self.editor.cursor_line,
        };
    }

    pub(super) fn maybe_open_wiki_link_autocomplete_at_cursor(
        &mut self,
        db: &crate::storage::Db,
    ) -> bool {
        self.ensure_wiki_link_sources_loaded(db);
        if self.wiki_link_autocomplete_popup.visible {
            return false;
        }
        let line = self.current_line().to_string();
        let Some(link) =
            crate::editor_core::markdown_tokens::wiki_link_at_cursor(&line, self.editor.cursor_col)
        else {
            return false;
        };
        if self.editor.cursor_col < link.from + 2 {
            return false;
        }
        let from_col = link.from;
        let query: String = line
            .chars()
            .skip(from_col + 2)
            .take(self.editor.cursor_col.saturating_sub(from_col + 2))
            .collect();
        if crate::editor_core::completion::parse_wiki_link_query(&query).is_none() {
            return false;
        }

        let note_suggestions = self.wiki_link_note_suggestions_cache.clone();
        let suggestions = note_suggestions.clone();
        let (anchor_row, anchor_col) = self.variable_popup_anchor(from_col).unwrap_or((
            self.editor
                .cursor_line
                .saturating_sub(self.view.scroll_line)
                + super::EDITOR_TOP_ROW,
            from_col.saturating_add(1),
        ));
        self.wiki_link_autocomplete_popup = WikiLinkAutocompletePopupState {
            visible: true,
            anchor_row,
            anchor_col,
            from_col,
            query,
            pending_heading_note_id: None,
            note_suggestions,
            heading_cache: rustc_hash::FxHashMap::default(),
            suggestions,
            selected_index: 0,
            cursor_line: self.editor.cursor_line,
        };
        self.refresh_wiki_link_autocomplete(db);
        true
    }

    pub(super) fn refresh_wiki_link_autocomplete(&mut self, db: &crate::storage::Db) {
        if !self.wiki_link_autocomplete_popup.visible {
            return;
        }
        if self.wiki_link_autocomplete_popup.cursor_line != self.editor.cursor_line {
            self.cancel_wiki_link_autocomplete();
            return;
        }
        let line = self.current_line();
        let from_col = self.wiki_link_autocomplete_popup.from_col;
        // Cursor must stay to the right of [[ and line must still have ]] ahead.
        if self.editor.cursor_col < from_col + 2 {
            self.cancel_wiki_link_autocomplete();
            return;
        }
        let query: String = line
            .chars()
            .skip(from_col + 2)
            .take(self.editor.cursor_col - from_col - 2)
            .collect();
        let Some((_, _heading_query)) =
            crate::editor_core::completion::parse_wiki_link_query(&query)
        else {
            self.cancel_wiki_link_autocomplete();
            return;
        };
        self.wiki_link_autocomplete_popup.query = query;
        if let Some((anchor_row, anchor_col)) = self.variable_popup_anchor(from_col) {
            self.wiki_link_autocomplete_popup.anchor_row = anchor_row;
            self.wiki_link_autocomplete_popup.anchor_col = anchor_col;
        }
        if let Some((target, heading_query)) = crate::editor_core::completion::parse_wiki_link_query(
            self.wiki_link_autocomplete_popup.query.as_str(),
        ) {
            self.wiki_link_autocomplete_popup.suggestions = match heading_query {
                Some(value) => {
                    if !self
                        .wiki_link_autocomplete_popup
                        .heading_cache
                        .contains_key(target)
                    {
                        let loaded = Self::load_wiki_link_heading_suggestions(db, target);
                        self.wiki_link_autocomplete_popup
                            .heading_cache
                            .insert(target.to_string(), loaded);
                    }
                    if let Some(cached) =
                        self.wiki_link_autocomplete_popup.heading_cache.get(target)
                    {
                        if value.is_empty() {
                            cached.clone()
                        } else {
                            crate::editor_core::completion::filter_wiki_suggestions(
                                cached,
                                value,
                                |suggestion| &suggestion.title_lower,
                            )
                        }
                    } else {
                        Vec::new()
                    }
                }
                None => {
                    if target.is_empty() {
                        self.wiki_link_autocomplete_popup.note_suggestions.clone()
                    } else {
                        crate::editor_core::completion::filter_wiki_suggestions(
                            &self.wiki_link_autocomplete_popup.note_suggestions,
                            target,
                            |suggestion| &suggestion.title_lower,
                        )
                    }
                }
            };
            self.wiki_link_autocomplete_popup.selected_index = 0;
        }
    }

    #[cfg(test)]
    pub(super) fn filtered_wiki_link_suggestions(&self) -> Vec<&WikiLinkSuggestion> {
        let (start, end) = self.wiki_link_visible_window(super::WIKI_LINK_AUTOCOMPLETE_MAX_VISIBLE);
        self.wiki_link_autocomplete_popup
            .suggestions
            .iter()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect()
    }

    pub(super) fn wiki_link_visible_window(&self, max_items: usize) -> (usize, usize) {
        let total = self.wiki_link_autocomplete_popup.suggestions.len();
        if total == 0 || max_items == 0 {
            return (0, 0);
        }
        let selected = self
            .wiki_link_autocomplete_popup
            .selected_index
            .min(total.saturating_sub(1));
        let visible = total.min(max_items);
        let start = selected.saturating_add(1).saturating_sub(visible);
        (start, start + visible)
    }

    pub(super) fn move_wiki_link_selection(&mut self, delta: isize) -> bool {
        if !self.wiki_link_autocomplete_popup.visible {
            return false;
        }
        let count = self.wiki_link_autocomplete_popup.suggestions.len();
        if count == 0 {
            return false;
        }
        let current = self
            .wiki_link_autocomplete_popup
            .selected_index
            .min(count.saturating_sub(1));
        let next = if delta >= 0 {
            (current + delta as usize) % count
        } else {
            (current + count - ((-delta) as usize % count)) % count
        };
        self.wiki_link_autocomplete_popup.selected_index = next;
        true
    }

    pub(super) fn apply_wiki_link_selection(&mut self, db: &crate::storage::Db) -> bool {
        if !self.wiki_link_autocomplete_popup.visible {
            return false;
        }
        let len = self.wiki_link_autocomplete_popup.suggestions.len();
        if len == 0 {
            self.cancel_wiki_link_autocomplete();
            return false;
        }
        let idx = self
            .wiki_link_autocomplete_popup
            .selected_index
            .min(len.saturating_sub(1));
        let Some(pick) = self
            .wiki_link_autocomplete_popup
            .suggestions
            .get(idx)
            .cloned()
        else {
            self.dismiss_wiki_link_autocomplete();
            return false;
        };
        let note_id = pick.note_id;
        let title = pick.title;
        let heading = pick.heading;
        let from_col = self.wiki_link_autocomplete_popup.from_col;
        let replacement = if let Some(heading) = heading.as_deref() {
            format!("[[{}#{}]]", note_id, heading)
        } else {
            format!("[[{}#]]", note_id)
        };

        // Find end of [[...]] span: scan forward from from_col for ]]
        let line = self.current_line().to_string();
        let chars: Vec<char> = line.chars().collect();
        let mut end_col = self.editor.cursor_col;
        while end_col + 1 < chars.len() {
            if chars[end_col] == ']' && chars[end_col + 1] == ']' {
                end_col += 2;
                break;
            }
            end_col += 1;
        }

        let delta =
            self.replace_line_chars(self.editor.cursor_line, from_col..end_col, &replacement);
        if heading.is_some() {
            self.editor.cursor_col = from_col + replacement.chars().count();
        } else {
            // Keep caret right after the auto-added # to filter heading picks.
            self.editor.cursor_col = from_col + 2 + note_id.chars().count() + 1;
        }
        self.refresh_calc_line_metadata_at(self.editor.cursor_line);
        if let Some(delta) = delta {
            self.mark_edited_with_delta(delta);
        }
        if heading.is_some() {
            self.wiki_link_autocomplete_popup.pending_heading_note_id = None;
            self.dismiss_wiki_link_autocomplete();
            self.status = format!("link heading: {title}");
        } else {
            self.wiki_link_autocomplete_popup.query = format!("{note_id}#");
            self.wiki_link_autocomplete_popup.pending_heading_note_id = Some(note_id);
            self.wiki_link_autocomplete_popup.selected_index = 0;
            self.wiki_link_autocomplete_popup.cursor_line = self.editor.cursor_line;
            if let Some((anchor_row, anchor_col)) =
                self.variable_popup_anchor(self.wiki_link_autocomplete_popup.from_col)
            {
                self.wiki_link_autocomplete_popup.anchor_row = anchor_row;
                self.wiki_link_autocomplete_popup.anchor_col = anchor_col;
            }
            self.refresh_wiki_link_autocomplete(db);
            self.status = format!("link: {title}");
        }
        true
    }

    pub(super) fn navigate_wiki_link_at_cursor(&mut self, db: &crate::storage::Db) -> bool {
        let line = self.current_line().to_string();
        let Some(link) =
            crate::editor_core::markdown_tokens::wiki_link_at_cursor(&line, self.editor.cursor_col)
        else {
            return false;
        };
        match db.get_note_meta(&link.note_id) {
            Ok(Some(summary)) if summary.id == self.active_note.id => {
                // Reloading would replace unsaved edits and undo history
                // with the stored body; the open text is already this note.
                match &link.heading {
                    Some(h) => {
                        self.jump_to_heading(h);
                        self.status = format!("→ {}#{}", summary.title, h);
                    }
                    None => self.status = format!("→ {}", summary.title),
                }
            }
            Ok(Some(summary)) => match db.get_note(&summary.id) {
                Ok(Some(_)) if !self.can_leave_note(db) => {}
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
            },
            Ok(None) => {
                self.status = "wiki-link: broken (note deleted)".to_string();
            }
            Err(e) => {
                self.status = format!("wiki-link error: {e}");
            }
        }
        true
    }

    pub(super) fn go_to_variable_definition_at_cursor(&mut self) {
        let target = if self.note_math_module_enabled() {
            crate::editor_core::calc_plan::variable_definition_at(
                self.editor.lines(),
                self.session.calc.calc_dependency_index.as_ref(),
                self.editor.cursor_line,
                self.editor.cursor_col,
                self.calc_feature_mask(),
            )
        } else {
            None
        };
        let Some(target) = target else {
            self.status = "no link or variable at cursor".to_string();
            return;
        };
        self.editor.cursor_line = target.line;
        self.editor.cursor_col = target.col;
        self.adjust_cursor();
        self.adjust_scroll();
        self.status = format!("definition: {}", target.name);
    }

    /// For `[[id]].name` at the cursor, opens that note at the assignment of
    /// `name`, or at its top when the note no longer assigns it. False when
    /// the cursor is not on such a name.
    pub(super) fn go_to_qualified_variable_at_cursor(&mut self, db: &crate::storage::Db) -> bool {
        if !self.note_math_module_enabled() {
            return false;
        }
        let Some(reference) = crate::editor_core::calc_plan::qualified_variable_at(
            self.current_line(),
            self.editor.cursor_col,
        ) else {
            return false;
        };
        if reference.note_id == self.active_note.id {
            // Reloading would replace unsaved edits and undo history with
            // the stored body; the definition is in the open text.
            let target = crate::editor_core::calc_plan::variable_definition_named(
                self.editor.lines(),
                &reference.name,
                self.calc_feature_mask(),
            );
            match target {
                Some(target) => {
                    self.editor.cursor_line = target.line;
                    self.editor.cursor_col = target.col;
                    self.adjust_cursor();
                    self.adjust_scroll();
                    self.status = format!("definition: {}", target.name);
                }
                None => self.status = format!("no {} in this note", reference.name),
            }
            return true;
        }
        let found = db
            .get_note_meta(&reference.note_id)
            .and_then(|summary| match summary {
                Some(summary) => Ok(db.get_note(&summary.id)?.map(|note| (summary.title, note))),
                None => Ok(None),
            });
        let (title, note) = match found {
            Ok(Some(found)) => found,
            Ok(None) => {
                self.status = "wiki-link: broken (note deleted)".to_string();
                return true;
            }
            Err(e) => {
                self.status = format!("wiki-link error: {e}");
                return true;
            }
        };
        if !self.can_leave_note(db) {
            return true;
        }
        if let Err(e) = self.set_active_note(db, note) {
            self.status = format!("wiki-link error: {e}");
            return true;
        }
        let target = crate::editor_core::calc_plan::variable_definition_named(
            self.editor.lines(),
            &reference.name,
            self.calc_feature_mask(),
        );
        let Some(target) = target else {
            self.status = format!("→ {title} (no {} there)", reference.name);
            return true;
        };
        self.editor.cursor_line = target.line;
        self.editor.cursor_col = target.col;
        self.adjust_cursor();
        self.adjust_scroll();
        self.status = format!("→ {title}: {}", target.name);
        true
    }

    pub(super) fn open_wiki_link_preview(&mut self, db: &crate::storage::Db) {
        let line = self.current_line().to_string();
        let Some(link) =
            crate::editor_core::markdown_tokens::wiki_link_at_cursor(&line, self.editor.cursor_col)
        else {
            self.status = "no wiki-link at cursor".to_string();
            return;
        };
        match db.get_note_meta(&link.note_id) {
            Ok(Some(summary)) => {
                let body = db
                    .get_note_body_preview(&summary.id, link.heading.as_deref())
                    .unwrap_or_default()
                    .unwrap_or_default();
                self.wiki_link_preview.title = summary.title;
                self.wiki_link_preview.body = extract_preview_content(&body, 5);
                self.wiki_link_preview.visible = true;
            }
            Ok(None) => {
                self.status = "wiki-link: note not found".to_string();
            }
            Err(e) => {
                self.status = format!("wiki-link error: {e}");
            }
        }
    }

    pub(super) fn close_wiki_link_preview(&mut self) {
        self.wiki_link_preview.visible = false;
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
        for (idx, line) in self.editor.lines().iter().enumerate() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with('#') {
                continue;
            }
            let content = trimmed.trim_start_matches('#').trim();
            if normalize(content) == needle {
                self.editor.cursor_line = idx;
                self.editor.cursor_col = 0;
                self.adjust_scroll();
                return;
            }
        }
    }

    pub(super) fn ensure_calc_for_viewport(&mut self, editor_height: usize, force: bool) {
        let started = Instant::now();
        if !self.calc_runtime.viewport_only {
            return;
        }
        // Preparing inline would redo, on this keystroke, the whole-note work
        // the background build is already doing; its install evaluates.
        if self.calc_workers.range_context_build.is_some()
            && !self.install_viewport_calc_preparation()
        {
            return;
        }
        let Some(eval_range) = self.calc_eval_range_for_viewport(editor_height) else {
            return;
        };
        if !force && self.calc_runtime.last_view_eval_range == Some(eval_range) {
            return;
        }
        self.recompute_calc_range(eval_range.0, eval_range.1);
        self.calc_runtime.last_view_eval_range = Some(eval_range);
        self.record_perf_duration("tui.session.calc.viewport_eval", "eval", started.elapsed());
    }
}

fn extract_preview_content(body: &str, max_lines: usize) -> String {
    let mut result = Vec::new();
    let mut skipped_title = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !skipped_title && trimmed.starts_with('#') && !trimmed.starts_with("##") {
            skipped_title = true;
            continue;
        }
        result.push(trimmed.to_string());
        if result.len() >= max_lines {
            break;
        }
    }
    result.join("\n")
}

/// Char index on screen `row` whose column is the closest at or before
/// `screen_col` (the first char of the row when none is). The end-of-line
/// slot is only a candidate when `allow_end` (insert mode).
/// Cross-note refs of `lines` given the refs of their previous version, where
/// old lines `[from, old_to)` became `[from, new_to)`: only the changed lines
/// are scanned and later refs are shifted.
#[cfg(test)]
pub(super) use note_session::calc::splice_cross_note_refs;
fn column_on_screen_row(
    positions: &[Option<(usize, usize)>],
    row: usize,
    screen_col: usize,
    allow_end: bool,
) -> usize {
    let end_slot = positions.len() - 1;
    let mut best: Option<(usize, usize)> = None;
    let mut first: Option<usize> = None;
    for (idx, pos) in positions.iter().enumerate() {
        let Some((r, c)) = *pos else { continue };
        if r != row || (idx == end_slot && !allow_end) {
            continue;
        }
        first.get_or_insert(idx);
        if c <= screen_col && best.is_none_or(|(_, best_col)| c >= best_col) {
            best = Some((idx, c));
        }
    }
    best.map(|(idx, _)| idx).or(first).unwrap_or(0)
}
