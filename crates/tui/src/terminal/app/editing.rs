use super::{
    build_variable_suggestions, compute_calc_data, compute_calc_data_for_note,
    compute_calc_trailer_refresh, contains_assignment_operator,
    cross_note_exports_for_autocomplete, display_cols_for_prefix,
    extract_cross_note_completion_prefix, extract_variable_completion_prefix,
    variable_completion_candidates,
    find_calc_segment_range, find_table_formula_segments, gutter_width_for_visible_lines,
    is_markdown_table_line, line_char_len, line_display_cols, preload_cross_note_dep_value,
    table_cell_edit_start, table_cell_info_at_char,
    table_cell_is_empty, table_cell_navigation_anchor, tui_note_short_id, Db, FoldKind,
    LineReminderGhost, ReminderUndoEntry, TerminalApp, UiMode, UndoAction,
    VariableAutocompletePopupState, VariableAutocompleteState, WikiLinkAutocompletePopupState,
    WikiLinkSuggestion, CALC_ASYNC_MIN_LINES, CALC_IDLE_EVAL_BUDGET_MS, CALC_RECOMPUTE_DEBOUNCE_MS,
    CALC_RECOMPUTE_PENDING_RETRY_MS, CALC_VIEWPORT_PREFETCH_MULTIPLIER, EDITOR_TOP_ROW,
    FENCE_CHECKPOINT_INTERVAL, HORIZONTAL_SCROLL_LEFT_CONTEXT, LARGE_DOC_CALC_DEFER_LINES,
    UNDO_DEBOUNCE_MS, VARIABLE_AUTOCOMPLETE_MAX_SUGGESTIONS,
};
use crate::terminal::text_utils::{
    byte_index, cursor_render_char_col, remove_char_at, viewport_col_for_display_col,
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

fn document_text_len(lines: &[String]) -> usize {
    if lines.len() == 1 && lines.first().is_some_and(String::is_empty) {
        0
    } else {
        let line_bytes: usize = lines.iter().map(|line| line.len()).sum();
        line_bytes.saturating_add(lines.len().saturating_sub(1))
    }
}

fn line_and_byte_for_offset(lines: &[String], target: usize) -> (usize, usize) {
    if lines.is_empty() {
        return (0, 0);
    }
    let mut offset = 0usize;
    for (idx, line) in lines.iter().enumerate() {
        let line_end = offset + line.len();
        if target <= line_end {
            return (idx, target.saturating_sub(offset));
        }
        offset = line_end + 1;
    }
    let last = lines.len().saturating_sub(1);
    (last, lines[last].len())
}

fn apply_text_change_in_place(
    lines: &mut Vec<String>,
    change: &crate::editor_core::types::TextChange,
    doc_len: usize,
) -> (usize, usize, usize) {
    let from = change.from.min(doc_len);
    let to = change.to.min(doc_len);
    let (from_line, from_byte) = line_and_byte_for_offset(lines, from);
    let (to_line, to_byte) = line_and_byte_for_offset(lines, to);

    let from_text = lines.get(from_line).cloned().unwrap_or_default();
    let to_text = lines.get(to_line).cloned().unwrap_or_default();
    let prefix = &from_text[..from_byte.min(from_text.len())];
    let suffix = &to_text[to_byte.min(to_text.len())..];
    let insert_parts = change.insert.split('\n').collect::<Vec<_>>();
    let mut replacement = Vec::with_capacity(insert_parts.len().max(1));

    if insert_parts.len() <= 1 {
        replacement.push(format!(
            "{prefix}{}{suffix}",
            insert_parts.first().copied().unwrap_or("")
        ));
    } else {
        replacement.push(format!("{prefix}{}", insert_parts[0]));
        for part in &insert_parts[1..insert_parts.len() - 1] {
            replacement.push((*part).to_string());
        }
        replacement.push(format!(
            "{}{suffix}",
            insert_parts.last().copied().unwrap_or("")
        ));
    }

    let old_line_span = to_line.saturating_sub(from_line).saturating_add(1);
    let new_line_span = replacement.len().max(1);
    if from_line <= to_line && from_line < lines.len() {
        let end = to_line.min(lines.len().saturating_sub(1));
        lines.splice(from_line..=end, replacement);
    } else {
        *lines = replacement;
    }
    if lines.is_empty() {
        lines.push(String::new());
    }

    (from_line, old_line_span, new_line_span)
}

// Ownership: editor mutations, cursor movement, folding, and calc state updates.
impl TerminalApp {
    pub(super) fn push_undo_action(&mut self, action: UndoAction) {
        if self.undo_action_pos < self.undo_actions.len() {
            self.undo_actions.truncate(self.undo_action_pos);
        }
        self.undo_actions.push(action);
        self.undo_action_pos = self.undo_actions.len();
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
        self.push_undo_action(UndoAction::Reminder(ReminderUndoEntry {
            line_idx,
            before,
            after,
        }));
    }

    fn prefer_span_history_fast_path(&self) -> bool {
        // Above COALESCE_ANCHOR_MAX_LINES the generic `record_edit` path no longer
        // coalesces (its coalesce anchor is only retained while the doc fits within
        // that cap), so it already emits one undo entry per edit. The span fast path
        // is then behavior-equivalent but O(changed lines) instead of O(doc) per
        // keystroke, because `record_edit`'s prefix/suffix diff scans from the
        // document ends. Gate on the coalesce cap rather than the much larger
        // lightweight-fold threshold so mid-size notes (5k–30k lines) stop paying
        // the per-keystroke full-document diff.
        self.editor.lines.len() > crate::terminal::history::COALESCE_ANCHOR_MAX_LINES
    }

    fn record_history_after_edit(
        &mut self,
        coalesce_undo: bool,
        history_span: Option<(usize, usize, usize)>,
    ) {
        let undo_depth_before = self.history.undo_depth();
        let history_changed = if let Some((start_line, old_line_span, new_line_span)) = history_span
        {
            if self.prefer_span_history_fast_path() {
                let history_changed = self.history.record_edit_span(
                    &self.editor.lines,
                    self.editor.cursor_line,
                    self.editor.cursor_col,
                    start_line,
                    old_line_span,
                    new_line_span,
                );
                if history_changed {
                    let undo_depth_after = self.history.undo_depth();
                    if !coalesce_undo || undo_depth_after > undo_depth_before {
                        self.push_undo_action(UndoAction::Text);
                    }
                }
                return;
            }
            self.history.record_edit(
                &self.editor.lines,
                self.editor.cursor_line,
                self.editor.cursor_col,
                coalesce_undo,
            )
        } else {
            self.history.record_edit(
                &self.editor.lines,
                self.editor.cursor_line,
                self.editor.cursor_col,
                coalesce_undo,
            )
        };
        if history_changed {
            let undo_depth_after = self.history.undo_depth();
            if !coalesce_undo || undo_depth_after > undo_depth_before {
                self.push_undo_action(UndoAction::Text);
            }
        }
    }

    pub(super) fn bootstrap_folding_for_startup(&mut self) {
        // Keep startup cheap for very large notes: build a plain 1:1 visible
        // map and defer expensive fold structure analysis until needed.
        self.folds.ranges.clear();
        self.folds.range_by_start = vec![None; self.editor.lines.len()];
        self.folds.collapsed_starts.clear();
        self.folds.line_has_structure = vec![false; self.editor.lines.len()];
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
            || self.folds.range_by_start.len() != self.editor.lines.len()
            || self.folds.line_has_structure.len() != self.editor.lines.len()
            || self.folds.line_text_snapshot.len() != self.editor.lines.len()
        {
            self.recompute_folding();
        }
    }

    pub(super) fn current_line(&self) -> &str {
        self.editor
            .lines
            .get(self.editor.cursor_line)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    pub(super) fn current_line_mut(&mut self) -> &mut String {
        if self.editor.lines.is_empty() {
            self.editor.lines.push(String::new());
        }
        &mut self.editor.lines[self.editor.cursor_line]
    }

    pub(super) fn rescan_calc_flags(&mut self) {
        let flags = crate::editor_core::calc_plan::detect_calc_signal_flags_with_mask(
            &self.editor.lines,
            self.calc_feature_mask(),
        );
        self.calc.cached_has_builtin_formula = flags.has_builtin_formula;
        self.calc.cached_has_variable_assignment = flags.has_variable_assignment;
        self.calc.cached_has_expression = flags.has_expression;
    }

    pub(super) fn update_calc_flags_incremental(&mut self) {
        // Incremental signal-detection semantics live in the shared core; the
        // TUI owns the cached flags and `results`-length bookkeeping.
        let mut flags = crate::editor_core::calc_plan::CalcSignalFlags {
            has_variable_assignment: self.calc.cached_has_variable_assignment,
            has_builtin_formula: self.calc.cached_has_builtin_formula,
            has_expression: self.calc.cached_has_expression,
        };
        crate::editor_core::calc_plan::merge_incremental_signal_flags(
            &mut flags,
            &self.editor.lines,
            self.calc.results.len(),
            self.editor.cursor_line,
            self.calc_feature_mask(),
        );
        self.calc.cached_has_variable_assignment = flags.has_variable_assignment;
        self.calc.cached_has_builtin_formula = flags.has_builtin_formula;
        self.calc.cached_has_expression = flags.has_expression;
    }

    fn rebuild_calc_line_metadata(&mut self) {
        self.calc.line_metadata = crate::editor_core::calc_plan::line_metadata_for_lines_with_mask(
            &self.editor.lines,
            self.calc_feature_mask(),
        );
    }

    fn ensure_calc_line_metadata(&mut self) {
        if self.calc.line_metadata.len() != self.editor.lines.len() {
            self.rebuild_calc_line_metadata();
        }
    }

    fn refresh_calc_line_metadata_at(&mut self, line_idx: usize) {
        if self.calc.line_metadata.is_empty() && self.editor.lines.is_empty() {
            return;
        }
        self.ensure_calc_line_metadata();
        if line_idx >= self.editor.lines.len() || line_idx >= self.calc.line_metadata.len() {
            return;
        }
        self.calc.line_metadata[line_idx] = crate::editor_core::calc_plan::line_metadata_with_mask(
            &self.editor.lines[line_idx],
            self.calc_feature_mask(),
        );
    }

    fn splice_calc_line_metadata(
        &mut self,
        start_line: usize,
        old_line_span: usize,
        new_line_span: usize,
    ) {
        if self.calc.line_metadata.is_empty() && self.editor.lines.is_empty() {
            return;
        }
        // Core recomputes metadata only for the replaced lines, or returns false
        // when dimensions are inconsistent so we fall back to a full rebuild
        // rather than corrupting the cache.
        let mask = self.calc_feature_mask();
        let spliced = crate::editor_core::calc_plan::splice_line_metadata(
            &mut self.calc.line_metadata,
            &self.editor.lines,
            start_line,
            old_line_span,
            new_line_span,
            mask,
        );
        if !spliced {
            self.rebuild_calc_line_metadata();
        }
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
            && self.calc.cached_has_variable_assignment
    }

    pub(super) fn calc_variables_enabled(&self) -> bool {
        self.active_has_variable_assignments()
    }

    pub(super) fn calc_cross_note_enabled(&self) -> bool {
        self.note_math_module_enabled() && self.note_cross_note_module_enabled()
    }

    pub(super) fn should_defer_calc_recompute(&self) -> bool {
        self.editor.lines.len() >= LARGE_DOC_CALC_DEFER_LINES
            && !self.calc.cached_has_builtin_formula
            && !self.active_has_variable_assignments()
    }

    pub(super) fn can_skip_calc_recompute(&self) -> bool {
        // If no line holds a formula, an assignment, or anything that looks
        // like a calculation, `compute_calc_data` would produce all-None
        // results — matching the current state. Safe to skip regardless of
        // doc size, which is the biggest input-latency win for notes that
        // don't use calc at all.
        !self.calc.cached_has_builtin_formula
            && !self.active_has_variable_assignments()
            && !self.calc.cached_has_expression
            && !self.calc.stale
    }

    fn calc_recompute_debounce_duration(&self) -> Duration {
        Duration::from_millis(CALC_RECOMPUTE_DEBOUNCE_MS)
    }

    fn schedule_calc_recompute(&mut self, viewport_pass: bool, full_pass: bool) {
        self.calc_runtime.recompute_pending = true;
        self.calc_runtime.pending_viewport_pass |= viewport_pass;
        self.calc_runtime.pending_full_pass |= full_pass;
        self.calc_runtime.recompute_due_at = None;
    }

    fn shared_prefix_len_hashes(prev_hashes: &[u64], next_hashes: &[u64]) -> usize {
        let max = prev_hashes.len().min(next_hashes.len());
        let mut idx = 0usize;
        while idx < max && prev_hashes[idx] == next_hashes[idx] {
            idx += 1;
        }
        idx
    }

    fn shared_suffix_len_hashes(
        prev_hashes: &[u64],
        next_hashes: &[u64],
        prefix_len: usize,
    ) -> usize {
        let max = prev_hashes
            .len()
            .min(next_hashes.len())
            .saturating_sub(prefix_len);
        let mut idx = 0usize;
        while idx < max
            && prev_hashes[prev_hashes.len() - 1 - idx] == next_hashes[next_hashes.len() - 1 - idx]
        {
            idx += 1;
        }
        idx
    }

    fn try_remap_calc_results_after_structural_edit(&mut self) -> bool {
        if !self.note_math_module_enabled() || self.calc.stale {
            return false;
        }
        self.ensure_calc_line_metadata();

        let prev_len = self.calc.results.len();
        let next_len = self.editor.lines.len();
        if prev_len == 0
            || self.calc.cell_results.len() != prev_len
            || self.calc.prev_line_metadata.len() != prev_len
            || self.calc.line_metadata.len() != next_len
        {
            return false;
        }

        let prev_hashes = self
            .calc
            .prev_line_metadata
            .iter()
            .map(|entry| entry.hash)
            .collect::<Vec<_>>();
        let next_hashes = self
            .calc
            .line_metadata
            .iter()
            .map(|entry| entry.hash)
            .collect::<Vec<_>>();
        let prefix = Self::shared_prefix_len_hashes(&prev_hashes, &next_hashes);
        let suffix = Self::shared_suffix_len_hashes(&prev_hashes, &next_hashes, prefix);
        let changed_from = prefix.min(next_len);
        let changed_to_next = next_len.saturating_sub(suffix).max(changed_from);
        let prev_changed_from = prefix.min(prev_len);
        let prev_changed_to = prev_len.saturating_sub(suffix).max(prev_changed_from);
        let prev_changed_slice = self
            .calc
            .prev_line_metadata
            .get(prev_changed_from..prev_changed_to)
            .unwrap_or(&[]);

        let prev_changed_had_assignment =
            prev_changed_slice.iter().any(|entry| entry.has_assignment);
        let prev_changed_had_builtin_formula = prev_changed_slice
            .iter()
            .any(|entry| entry.has_builtin_formula);
        let mask = self.calc_feature_mask();
        let changed_lines = self
            .editor
            .lines
            .get(changed_from..changed_to_next)
            .unwrap_or(&[]);
        let changed_touches_table = mask.table_enabled
            && changed_lines
                .iter()
                .any(|line| is_markdown_table_line(line));
        let changed_has_assignment =
            crate::editor_core::calc_plan::contains_variable_assignment_with_mask(
                changed_lines,
                mask,
            );
        let changed_has_builtin_formula =
            crate::editor_core::calc_plan::contains_builtin_formula_with_mask(changed_lines, mask);
        let changed_has_calc_expression = changed_lines.iter().any(|line| {
            let eval_target =
                crate::editor_core::calc_plan::line_for_calc_evaluation_with_mask(line, mask);
            let trimmed = eval_target.trim();
            !trimmed.is_empty() && crate::editor_core::calc_plan::has_calc_signal(trimmed)
        });

        // Safe remap-only path: only line index shifting happened, and changed
        // lines don't participate in calc semantics.
        if changed_touches_table
            || prev_changed_had_assignment
            || changed_has_assignment
            || prev_changed_had_builtin_formula
            || changed_has_builtin_formula
            || changed_has_calc_expression
        {
            return false;
        }

        let mut remapped_results = vec![None; next_len];
        let mut remapped_cell_results = vec![Vec::new(); next_len];

        let shared_prefix = prefix.min(prev_len).min(next_len);
        for line_idx in 0..shared_prefix {
            remapped_results[line_idx] = self.calc.results[line_idx].clone();
            remapped_cell_results[line_idx] = self.calc.cell_results[line_idx].clone();
        }

        let shared_suffix = suffix
            .min(prev_len.saturating_sub(shared_prefix))
            .min(next_len.saturating_sub(shared_prefix));
        for offset in 0..shared_suffix {
            let prev_idx = prev_len - shared_suffix + offset;
            let next_idx = next_len - shared_suffix + offset;
            remapped_results[next_idx] = self.calc.results[prev_idx].clone();
            remapped_cell_results[next_idx] = self.calc.cell_results[prev_idx].clone();
        }

        self.calc.results = remapped_results;
        self.calc.cell_results = remapped_cell_results;
        self.calc.prev_line_metadata = self.calc.line_metadata.clone();
        self.calc.stale = false;
        self.calc_runtime.recompute_pending = false;
        self.calc_runtime.recompute_due_at = None;
        self.calc_runtime.pending_viewport_pass = false;
        self.calc_runtime.pending_full_pass = false;
        true
    }

    pub(super) fn clear_calc_cache(&mut self) {
        self.calc.results = vec![None; self.editor.lines.len()];
        self.calc.cell_results = vec![Vec::new(); self.editor.lines.len()];
        self.calc.variable_names.clear();
        self.calc.calc_dependency_index = None;
        self.calc.line_metadata.clear();
        self.calc.prev_line_metadata.clear();
        self.calc.stale = false;
        self.calc.pathological_window_streak = 0;
        self.calc.forced_full_recompute_remaining = 0;
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
        if self.calc.results.len() != self.editor.lines.len() {
            self.calc.results = vec![None; self.editor.lines.len()];
        } else {
            self.calc.results.fill(None);
        }
        if self.calc.cell_results.len() != self.editor.lines.len() {
            self.calc.cell_results = vec![Vec::new(); self.editor.lines.len()];
        } else {
            for row in &mut self.calc.cell_results {
                row.clear();
            }
        }
        self.calc.variable_names.clear();
        self.calc.stale = true;
    }

    pub(super) fn recompute_folding_if_needed(&mut self) {
        if self.large_note_reduced_features() {
            if !self.folds.ranges.is_empty() {
                self.folds.ranges.clear();
            }
            if self.folds.range_by_start.len() != self.editor.lines.len() {
                self.folds.range_by_start = vec![None; self.editor.lines.len()];
            }
            if !self.folds.collapsed_starts.is_empty() {
                self.folds.collapsed_starts.clear();
            }
            if self.folds.visible_to_real.len() != self.editor.lines.len()
                || self.folds.real_to_visible.len() != self.editor.lines.len()
                || self.folds.hidden_owner.len() != self.editor.lines.len()
                || self.folds.placeholder_hidden_lines.len() != self.editor.lines.len()
            {
                self.rebuild_fold_view_map();
            }
            if self.folds.line_has_structure.len() != self.editor.lines.len() {
                self.folds.line_has_structure = vec![false; self.editor.lines.len()];
            }
            self.folds.line_text_snapshot.clear();
            self.folds.rescan_pending = false;
            self.folds.analysis_ready = false;
            return;
        }
        // Process any pending deferred recompute first.
        if self.folds.rescan_pending {
            self.folds.rescan_pending = false;
            self.recompute_folding();
            return;
        }

        if self.editor.lines.is_empty() {
            self.folds.line_has_structure.clear();
            self.folds.line_text_snapshot.clear();
            self.apply_fold_ranges(Vec::new());
            self.folds.analysis_ready = true;
            return;
        }

        let cl = self
            .editor
            .cursor_line
            .min(self.editor.lines.len().saturating_sub(1));
        let line_count_changed = self.editor.lines.len() != self.folds.line_has_structure.len();

        if line_count_changed {
            let next_len = self.editor.lines.len();
            let prev_len = self.folds.line_has_structure.len();
            // Safety guard: incremental insert/remove remap assumes snapshot and
            // structure vectors are aligned. If a prior mode change/reset left
            // them out of sync, use full recompute instead of risking panic on
            // Vec::insert/remove indexes during Enter/Delete edits.
            if self.folds.line_text_snapshot.len() != prev_len {
                self.recompute_folding();
                return;
            }
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
                let new_line_text = self
                    .editor
                    .lines
                    .get(insert_at)
                    .cloned()
                    .unwrap_or_default();
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
                            .editor
                            .lines
                            .get(cl - 1)
                            .map(|l| Self::line_has_fold_structure(l))
                            .unwrap_or(false);
                    }
                    if let Some(text) = self.folds.line_text_snapshot.get_mut(cl - 1) {
                        *text = self.editor.lines.get(cl - 1).cloned().unwrap_or_default();
                    }
                }
                let new_flag = self
                    .editor
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
                let new_line_text = self
                    .editor
                    .lines
                    .get(old_start_line)
                    .cloned()
                    .unwrap_or_default();
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
                        .editor
                        .lines
                        .get(update_at)
                        .map(|l| Self::line_has_fold_structure(l))
                        .unwrap_or(false);
                }
                if let Some(text) = self.folds.line_text_snapshot.get_mut(update_at) {
                    *text = self
                        .editor
                        .lines
                        .get(update_at)
                        .cloned()
                        .unwrap_or_default();
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
                self.folds.range_by_start = vec![None; self.editor.lines.len()];
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
        let current_text = self.editor.lines.get(cl).map(|s| s.as_str()).unwrap_or("");
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
            .editor
            .lines
            .iter()
            .map(|line| Self::line_has_fold_structure(line))
            .collect();
        self.folds.line_text_snapshot = self.editor.lines.clone();
        self.recompute_folding_from_cached_structure();
    }

    pub(super) fn recompute_folding_from_cached_structure(&mut self) {
        self.folds.rescan_pending = false;
        let ranges = folding::build_fold_ranges(&self.editor.lines);
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
            self.editor.lines.len().max(1),
        );
        self.folds.rescan_pending = false;
        self.apply_fold_ranges(mapped);
        self.folds.analysis_ready = true;
        true
    }

    fn apply_fold_ranges(&mut self, ranges: Vec<crate::editor_core::folding::FoldRange>) {
        self.folds.ranges = ranges;
        self.folds.range_by_start = vec![None; self.editor.lines.len()];
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
        let line_count = self.editor.lines.len();
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
        let target = target_line.min(self.editor.lines.len());
        let target_ck = target / FENCE_CHECKPOINT_INTERVAL;
        let start_ck = target_ck.min(self.render_state.fence_checkpoints_valid_through);
        let start_line = start_ck * FENCE_CHECKPOINT_INTERVAL;

        let (mut in_code_block, mut code_fence_lang) = if start_ck == 0 {
            (false, None)
        } else {
            self.render_state
                .fence_checkpoints
                .get(start_ck)
                .cloned()
                .unwrap_or((false, None))
        };

        let mut line_idx = start_line;
        while line_idx < target {
            // At each new checkpoint boundary, cache the current state.
            if line_idx > 0 && line_idx % FENCE_CHECKPOINT_INTERVAL == 0 {
                let ck = line_idx / FENCE_CHECKPOINT_INTERVAL;
                if ck > self.render_state.fence_checkpoints_valid_through {
                    while self.render_state.fence_checkpoints.len() <= ck {
                        self.render_state.fence_checkpoints.push((false, None));
                    }
                    self.render_state.fence_checkpoints[ck] =
                        (in_code_block, code_fence_lang.clone());
                    self.render_state.fence_checkpoints_valid_through = ck;
                }
            }
            if let Some(line_text) = self.editor.lines.get(line_idx) {
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
            .min(self.editor.lines.len().saturating_sub(1));
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
            .min(self.editor.lines.len().saturating_sub(1));
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
        history_span: Option<(usize, usize, usize)>,
    ) {
        let coalesce_undo = self.last_edit.elapsed() < Duration::from_millis(UNDO_DEBOUNCE_MS);
        let line_count_changed = self.editor.lines.len() != self.calc.results.len();
        self.invalidate_joined_text_cache();
        self.render_caches.table_formula_segment_cache.clear();
        self.dirty = true;
        if changed_from_line == 0 {
            self.switcher.needs_title_refresh = true;
        }
        if !self.reminder_ghosts.is_empty() {
            self.reminders_dirty = true;
        }
        let clamped_changed_line = if self.editor.lines.is_empty() {
            0
        } else {
            changed_from_line.min(self.editor.lines.len().saturating_sub(1))
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
            self.calc_runtime.recompute_pending = false;
            self.calc_runtime.recompute_due_at = None;
            self.calc_runtime.pending_viewport_pass = false;
            self.calc_runtime.pending_full_pass = false;
        } else if self.should_defer_calc_recompute() {
            self.defer_calc_state_after_edit();
            self.calc_runtime.recompute_pending = false;
            self.calc_runtime.recompute_due_at = None;
            self.calc_runtime.pending_viewport_pass = false;
            self.calc_runtime.pending_full_pass = false;
        } else {
            if self.editor.lines.len() >= CALC_ASYNC_MIN_LINES {
                if line_count_changed {
                    // Structural edits (Enter/join/delete-at-boundary): try a
                    // cheap remap-only path first, and recompute only when
                    // changed lines may affect calc semantics.
                    if !self.try_remap_calc_results_after_structural_edit() {
                        self.run_calc_recompute();
                    }
                } else {
                    // Keep large-note typing non-blocking: schedule calc for
                    // the next idle tick and clear only the edited line's
                    // cached result so we don't show stale ghosts while
                    // pending.
                    if let Some(slot) = self.calc.results.get_mut(self.editor.cursor_line) {
                        *slot = None;
                    }
                    if let Some(slot) = self.calc.cell_results.get_mut(self.editor.cursor_line) {
                        slot.clear();
                    }
                    self.schedule_calc_recompute(true, true);
                }
            } else {
                self.run_calc_recompute();
            }
        }
        self.record_history_after_edit(coalesce_undo, history_span);
        self.last_edit = Instant::now();
    }

    pub(super) fn mark_edited_from_line(&mut self, changed_from_line: usize) {
        self.mark_edited_from_line_with_span(changed_from_line, None);
    }

    pub(super) fn mark_edited(&mut self) {
        self.mark_edited_from_line_with_span(self.editor.cursor_line, None);
    }

    pub(super) fn mark_edited_current_line(&mut self) {
        let changed_line = self
            .editor
            .cursor_line
            .min(self.editor.lines.len().saturating_sub(1));
        self.mark_edited_from_line_with_span(changed_line, Some((changed_line, 1, 1)));
    }

    fn apply_reminder_state(
        &mut self,
        db: &Db,
        line_idx: usize,
        state: Option<LineReminderGhost>,
    ) -> Result<(), String> {
        let line_number = (line_idx + 1) as i64;
        match state {
            Some(reminder) => {
                db.upsert_reminder(
                    &self.active_note.id,
                    line_number,
                    reminder.remind_at_ms,
                    &reminder.display_at,
                    &reminder.line_text,
                )?;
                if let Some(reminded_at_ms) = reminder.reminded_at_ms {
                    let _ = db.mark_reminder_reminded(
                        &self.active_note.id,
                        line_number,
                        reminded_at_ms,
                    );
                }
                self.reminder_ghosts.insert(line_idx, reminder);
            }
            None => {
                db.delete_reminder(&self.active_note.id, line_number)?;
                self.reminder_ghosts.remove(&line_idx);
            }
        }
        Ok(())
    }

    fn undo_text_action(&mut self) {
        let keep_cursor_on_exhaust = self.history.undo_depth() == 1;
        let cursor_before_undo = (self.editor.cursor_line, self.editor.cursor_col);
        if let Some(cursor) = self.history.undo(&mut self.editor.lines) {
            self.invalidate_joined_text_cache();
            if keep_cursor_on_exhaust {
                self.editor.cursor_line = cursor_before_undo
                    .0
                    .min(self.editor.lines.len().saturating_sub(1));
                self.editor.cursor_col = cursor_before_undo.1;
            } else {
                self.editor.cursor_line =
                    cursor.line.min(self.editor.lines.len().saturating_sub(1));
                self.editor.cursor_col = cursor.col;
            }
            self.dirty = true;
            self.last_edit = Instant::now();
            if !self.reminder_ghosts.is_empty() {
                self.reminders_dirty = true;
            }
            // Full lines replacement: invalidate all caches.
            self.render_state.fence_checkpoints.truncate(1);
            self.render_state.fence_checkpoints_valid_through = 0;
            self.run_calc_recompute();
            self.recompute_folding();
            self.adjust_cursor();
            self.adjust_scroll();
            self.history.checkpoint(
                &self.editor.lines,
                self.editor.cursor_line,
                self.editor.cursor_col,
            );
            self.status = format!("undo ({} left)", self.undo_action_pos.saturating_sub(1));
        } else {
            self.status = "already at oldest change".to_string();
        }
    }

    fn redo_text_action(&mut self) {
        if let Some(cursor) = self.history.redo(&mut self.editor.lines) {
            self.invalidate_joined_text_cache();
            self.editor.cursor_line = cursor.line.min(self.editor.lines.len().saturating_sub(1));
            self.editor.cursor_col = cursor.col;
            self.dirty = true;
            self.last_edit = Instant::now();
            if !self.reminder_ghosts.is_empty() {
                self.reminders_dirty = true;
            }
            self.render_state.fence_checkpoints.truncate(1);
            self.render_state.fence_checkpoints_valid_through = 0;
            self.run_calc_recompute();
            self.recompute_folding();
            self.adjust_cursor();
            self.adjust_scroll();
            self.history.checkpoint(
                &self.editor.lines,
                self.editor.cursor_line,
                self.editor.cursor_col,
            );
            self.status = format!("redo ({} left)", self.history.redo_depth());
        } else {
            self.status = "already at newest change".to_string();
        }
    }

    pub(super) fn undo(&mut self, db: &Db) {
        if self.undo_action_pos == 0 {
            self.status = "already at oldest change".to_string();
            return;
        }
        let action = self.undo_actions[self.undo_action_pos - 1].clone();
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
        self.undo_action_pos = self.undo_action_pos.saturating_sub(1);
    }

    pub(super) fn redo(&mut self, db: &Db) {
        if self.undo_action_pos >= self.undo_actions.len() {
            self.status = "already at newest change".to_string();
            return;
        }
        let action = self.undo_actions[self.undo_action_pos].clone();
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
        self.undo_action_pos += 1;
    }

    pub(super) fn run_calc_recompute(&mut self) {
        let started = Instant::now();
        self.calc_runtime.recompute_due_at = None;
        self.calc_runtime.pending_viewport_pass = false;
        self.calc_runtime.pending_full_pass = false;
        if !self.note_math_module_enabled() {
            self.clear_calc_cache();
            self.calc_runtime.recompute_pending = false;
            self.record_perf_duration("tui.calc.recompute", "math_disabled", started.elapsed());
            return;
        }
        self.ensure_calc_line_metadata();
        if !self.editor.lines.is_empty() {
            let cursor_line = self
                .editor
                .cursor_line
                .min(self.editor.lines.len().saturating_sub(1));
            self.refresh_calc_line_metadata_at(cursor_line);
        }
        let calc_variables_enabled = self.calc_variables_enabled();
        let calc_cross_note_enabled = self.calc_cross_note_enabled();
        let calc_table_enabled = self.note_table_module_enabled();

        // Pre-load f64 values for any referenced dep notes before any eval path,
        // including the stale (first-open) path. Guarded internally so O(1) after
        // the first call per dep per session.
        if calc_cross_note_enabled {
            self.preload_cross_note_deps();
        }

        if self.calc.stale {
            let note_id = self.active_note.id.clone();
            let short_id = tui_note_short_id(&note_id).to_string();
            let calc_data = compute_calc_data_for_note(
                &self.calc.engine,
                &self.editor.lines,
                calc_variables_enabled,
                calc_cross_note_enabled,
                calc_table_enabled,
                &note_id,
                &short_id,
                &self.cross_note_var_index,
            );
            let calc_mask = self.calc_feature_mask();
            self.calc.calc_dependency_index =
                crate::editor_core::calc_plan::build_calc_dependency_index(
                    &self.editor.lines,
                    calc_mask,
                );
            self.calc.prev_line_metadata = self.calc.line_metadata.clone();
            self.calc.results = calc_data.line_results;
            self.calc.cell_results = calc_data.cell_results;
            let variable_names =
                crate::editor_core::calc_plan::variable_names_from_calc_dependency_index(
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
            self.calc_runtime.recompute_pending = false;
            self.calc_runtime.recompute_due_at = None;
            self.calc_runtime.pending_viewport_pass = false;
            self.calc_runtime.pending_full_pass = false;
            self.record_perf_duration("tui.calc.recompute", "stale_full", started.elapsed());
            return;
        }

        // Snapshot extern vars once for the whole incremental recompute.
        // Update deps from a live scan so refs added since the last full eval are picked up.
        let incremental_extern_vars: Vec<ExternVar> = if calc_cross_note_enabled {
            let note_id = self.active_note.id.clone();
            let refs = app_core::calc::scan_cross_note_refs(&self.editor.lines);
            if let Ok(mut index) = self.cross_note_var_index.lock() {
                index.update_deps(&note_id, &refs);
                index.extern_vars_for(&note_id)
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let plan = crate::editor_core::calc_plan::plan_incremental_calc_from_line_metadata(
            &self.calc.prev_line_metadata,
            &self.calc.results,
            &self.editor.lines,
            &self.calc.line_metadata,
        );
        let has_prev = !self.calc.prev_line_metadata.is_empty();

        // Only scan the changed region for variable assignments and builtin
        // formulas (not all lines). Partial eval is safe as long as the edit
        // doesn't touch a formula/assignment — whole-doc presence of formulas
        // elsewhere doesn't force recomputation of unchanged lines.
        let suffix_len = self.editor.lines.len().saturating_sub(plan.eval_to);
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
        let calc_mask = self.calc_feature_mask();
        crate::editor_core::calc_plan::sync_calc_dependency_index(
            &mut self.calc.calc_dependency_index,
            &self.editor.lines,
            plan.eval_from,
            plan.eval_to,
            calc_mask,
        );
        let eval_window = crate::editor_core::calc_plan::decide_eval_window(
            &crate::editor_core::calc_plan::DecideEvalWindowParams {
                lines: &self.editor.lines,
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

        let line_count = self.editor.lines.len();
        let eval_span = eval_to.saturating_sub(eval_from);
        let is_pathological_window = can_use_partial
            && line_count >= CALC_PATHOLOGICAL_WINDOW_MIN_LINES
            && eval_span.saturating_mul(100)
                >= line_count.saturating_mul(CALC_PATHOLOGICAL_WINDOW_PERCENT);
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
        if self.calc.pathological_window_streak >= CALC_PATHOLOGICAL_WINDOW_STREAK_THRESHOLD {
            self.calc.pathological_window_streak = 0;
            self.calc.forced_full_recompute_remaining = CALC_FORCED_FULL_RECOMPUTE_CYCLES;
            force_full_now = true;
        }
        if force_full_now {
            can_use_partial = false;
            eval_from = 0;
            eval_to = line_count;
        }

        let prev_results = std::mem::take(&mut self.calc.results);
        let prev_results_snapshot = prev_results.clone();
        let prev_cell_results = std::mem::take(&mut self.calc.cell_results);
        let same_shape_cache = prev_results.len() == self.editor.lines.len()
            && prev_cell_results.len() == self.editor.lines.len();

        let (mut new_results, mut new_cell_results) = if can_use_partial && same_shape_cache {
            let mut merged_results = prev_results;
            let mut merged_cells = prev_cell_results;
            if eval_from < eval_to {
                let calc_data = compute_calc_data(
                    &self.calc.engine,
                    &self.editor.lines,
                    calc_variables_enabled,
                    calc_cross_note_enabled,
                    calc_table_enabled,
                    Some((eval_from, eval_to)),
                    incremental_extern_vars.clone(),
                );
                for idx in eval_from..eval_to {
                    if let Some(slot) = merged_results.get_mut(idx) {
                        *slot = calc_data.line_results.get(idx).cloned().unwrap_or(None);
                    }
                    if let Some(slot) = merged_cells.get_mut(idx) {
                        *slot = calc_data.cell_results.get(idx).cloned().unwrap_or_default();
                    }
                }
            }
            (merged_results, merged_cells)
        } else if can_use_partial {
            let mut merged_results = vec![None; self.editor.lines.len()];
            for entry in &plan.base_results {
                if let Some(slot) = merged_results.get_mut(entry.line_idx) {
                    *slot = Some(entry.result.clone());
                }
            }
            let mut merged_cells: Vec<Vec<app_core::calc::TableCellEvaluation>> =
                vec![Vec::new(); self.editor.lines.len()];
            for entry in &plan.base_results {
                if let Some(slot) = merged_cells.get_mut(entry.line_idx) {
                    if let Some(cached) = prev_cell_results.get(entry.line_idx) {
                        *slot = cached.clone();
                    }
                }
            }
            if eval_from < eval_to {
                let calc_data = compute_calc_data(
                    &self.calc.engine,
                    &self.editor.lines,
                    calc_variables_enabled,
                    calc_cross_note_enabled,
                    calc_table_enabled,
                    Some((eval_from, eval_to)),
                    incremental_extern_vars.clone(),
                );
                for idx in eval_from..eval_to {
                    if let Some(slot) = merged_results.get_mut(idx) {
                        *slot = calc_data.line_results.get(idx).cloned().unwrap_or(None);
                    }
                    if let Some(slot) = merged_cells.get_mut(idx) {
                        *slot = calc_data.cell_results.get(idx).cloned().unwrap_or_default();
                    }
                }
            }
            (merged_results, merged_cells)
        } else {
            let note_id = self.active_note.id.clone();
            let short_id = tui_note_short_id(&note_id).to_string();
            let calc_data = compute_calc_data_for_note(
                &self.calc.engine,
                &self.editor.lines,
                calc_variables_enabled,
                calc_cross_note_enabled,
                calc_table_enabled,
                &note_id,
                &short_id,
                &self.cross_note_var_index,
            );
            (calc_data.line_results, calc_data.cell_results)
        };

        let variable_names =
            crate::editor_core::calc_plan::variable_names_from_calc_dependency_index(
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
        let aligned = self.calc.prev_line_metadata.len() == self.editor.lines.len()
            && prev_results_snapshot.len() == self.editor.lines.len();
        let mut trailer_rewritten_lines: Vec<usize> = Vec::new();

        if aligned {
            let cursor_line = self.editor.cursor_line;
            let cursor_col = self.editor.cursor_col;
            let selection_range: Option<(usize, usize)> = if matches!(
                self.mode,
                UiMode::Visual | UiMode::VisualLine | UiMode::CommandBar
            ) {
                self.editor.selection_anchor.map(|(anchor_line, _)| {
                    let a = anchor_line.min(cursor_line);
                    let b = anchor_line.max(cursor_line);
                    (a, b)
                })
            } else {
                None
            };

            for i in 0..self.editor.lines.len() {
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
                    prev_results_snapshot[i].as_deref(),
                    line_is_selected,
                ) {
                    continue;
                }
                let refresh = compute_calc_trailer_refresh(
                    &self.editor.lines[i],
                    new_result,
                    cursor_line == i,
                    cursor_col,
                );
                if let Some((eq_idx, new_tail)) = refresh {
                    self.editor.lines[i].replace_range(eq_idx.., &new_tail);
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
                            &self.editor.lines[i],
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
        let had_trailer_rewrites = !trailer_rewritten_lines.is_empty();
        for line_idx in trailer_rewritten_lines {
            if line_idx < self.calc.prev_line_metadata.len()
                && line_idx < self.calc.line_metadata.len()
            {
                self.calc.prev_line_metadata[line_idx] = self.calc.line_metadata[line_idx].clone();
            }
        }
        if had_trailer_rewrites {
            self.invalidate_joined_text_cache();
        }
        self.calc.results = new_results;
        self.calc.cell_results = new_cell_results;
        self.calc.variable_names.set(variable_names);
        self.calc.stale = false;
        self.calc_runtime.recompute_pending = false;
        self.calc_runtime.recompute_due_at = None;
        self.calc_runtime.pending_viewport_pass = false;
        self.calc_runtime.pending_full_pass = false;
        self.record_perf_duration("tui.calc.recompute", "incremental", started.elapsed());
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
        if self.editor.lines.len() >= CALC_ASYNC_MIN_LINES {
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

    /// `w`/`b` on a table row: cell borders count as whitespace, so the
    /// motion steps into the neighbouring cell, and past the row's first or
    /// last word it continues on the adjacent row (skipping the delimiter).
    /// Returns false when the cursor is not on a table row.
    fn move_cursor_word_in_table(&mut self, forward: bool) -> bool {
        use crate::editor_core::table::{
            is_delimiter_line_in, table_row_next_word_start, table_row_prev_word_start,
        };
        if !self.note_table_module_enabled() || !is_markdown_table_line(self.current_line()) {
            return false;
        }
        let on_row = if forward {
            table_row_next_word_start(self.current_line(), self.editor.cursor_col)
        } else {
            table_row_prev_word_start(self.current_line(), self.editor.cursor_col)
        };
        if let Some(col) = on_row {
            self.editor.cursor_col = col;
            return true;
        }

        let mut virtual_line = self.current_virtual_line();
        loop {
            let next_virtual = if forward {
                virtual_line + 1
            } else {
                let Some(prev) = virtual_line.checked_sub(1) else {
                    return true;
                };
                prev
            };
            if next_virtual >= self.visible_line_count() {
                return true;
            }
            let Some(line_idx) = self.real_line_for_virtual(next_virtual) else {
                return true;
            };
            virtual_line = next_virtual;
            let line = &self.editor.lines[line_idx];
            if !is_markdown_table_line(line) {
                self.editor.cursor_line = line_idx;
                self.editor.cursor_col = if forward { 0 } else { line_char_len(line) };
                return true;
            }
            if is_delimiter_line_in(&self.editor.lines, line_idx) {
                continue;
            }
            let col = if forward {
                table_row_next_word_start(line, 0)
            } else {
                table_row_prev_word_start(line, line_char_len(line))
            };
            self.editor.cursor_line = line_idx;
            // An empty row has no word; land on it and let the cursor guard
            // place the cursor in its first cell.
            self.editor.cursor_col = col.unwrap_or(0);
            return true;
        }
    }

    pub(super) fn move_cursor_left_word(&mut self) {
        if self.move_cursor_word_in_table(false) {
            return;
        }
        if self.editor.cursor_col == 0 {
            let current_virtual = self.current_virtual_line();
            if current_virtual > 0 {
                if let Some(prev_real) = self.real_line_for_virtual(current_virtual - 1) {
                    self.editor.cursor_line = prev_real;
                    self.editor.cursor_col = line_char_len(self.current_line());
                }
            }
            return;
        }
        let line = self.current_line();
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();

        let mut col = self.editor.cursor_col;
        if col > len {
            col = len;
        }
        if col == 0 {
            self.editor.cursor_col = 0;
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
        self.editor.cursor_col = col;
    }

    pub(super) fn move_cursor_right_word(&mut self) {
        if self.move_cursor_word_in_table(true) {
            return;
        }
        let line = self.current_line();
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        if self.editor.cursor_col >= len {
            let current_virtual = self.current_virtual_line();
            if current_virtual + 1 < self.visible_line_count() {
                if let Some(next_real) = self.real_line_for_virtual(current_virtual + 1) {
                    self.editor.cursor_line = next_real;
                    self.editor.cursor_col = 0;
                }
            }
            return;
        }
        let mut col = self.editor.cursor_col;
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

        self.editor.cursor_col = col;
    }

    pub(super) fn delete_word_backward(&mut self) -> bool {
        if self.editor.cursor_col == 0 {
            if self.editor.cursor_line > 0 {
                self.backspace();
                return true;
            }
            return false;
        }
        if self.note_table_module_enabled() {
            if let Some(cell) = table_cell_info_at_char(
                &self.editor.lines,
                self.editor.cursor_line,
                self.editor.cursor_col,
            ) {
                let Some(col) = crate::editor_core::table::table_cell_word_delete_start(
                    self.current_line(),
                    self.editor.cursor_col,
                    table_cell_edit_start(&cell),
                    table_cell_navigation_anchor(self.current_line(), &cell),
                ) else {
                    return false;
                };
                let start_byte = byte_index(self.current_line(), col);
                let end_byte = byte_index(self.current_line(), self.editor.cursor_col);
                let text = self.current_line_mut();
                text.replace_range(start_byte..end_byte, "");
                self.editor.cursor_col = col;
                self.refresh_calc_line_metadata_at(self.editor.cursor_line);
                self.mark_edited_current_line();
                self.prune_empty_table_continuation_row_at_cursor();
                return true;
            }
        }
        let line = self.current_line();
        let chars: Vec<char> = line.chars().collect();
        let mut col = self.editor.cursor_col;
        while col > 0 && chars.get(col - 1).map_or(false, |c| !c.is_alphanumeric()) {
            col -= 1;
        }
        while col > 0 && chars.get(col - 1).map_or(false, |c| c.is_alphanumeric()) {
            col -= 1;
        }

        let start_byte = byte_index(self.current_line(), col);
        let end_byte = byte_index(self.current_line(), self.editor.cursor_col);
        let text = self.current_line_mut();
        text.replace_range(start_byte..end_byte, "");
        self.editor.cursor_col = col;
        self.refresh_calc_line_metadata_at(self.editor.cursor_line);
        self.mark_edited_current_line();
        self.prune_empty_table_continuation_row_at_cursor();
        true
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        if ch.is_control() {
            return;
        }
        let col = self.editor.cursor_col;
        let line = self.current_line_mut();
        let idx = byte_index(line, col);
        line.insert(idx, ch);
        self.editor.cursor_col += 1;
        self.refresh_calc_line_metadata_at(self.editor.cursor_line);
        self.mark_edited_current_line();
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let col = self.editor.cursor_col;
        let line = self.current_line_mut();
        let idx = byte_index(line, col);
        line.insert_str(idx, text);
        self.editor.cursor_col += text.chars().count();
        self.refresh_calc_line_metadata_at(self.editor.cursor_line);
        self.mark_edited_current_line();
    }

    fn try_insert_table_cell_multiline_paste(&mut self, normalized: &str) -> bool {
        if !self.note_table_module_enabled() || self.editor.lines.is_empty() {
            return false;
        }
        let line_idx = self
            .editor
            .cursor_line
            .min(self.editor.lines.len().saturating_sub(1));
        let cursor_byte = byte_index(&self.editor.lines[line_idx], self.editor.cursor_col);
        let Some(edit) = crate::editor_core::table::plan_table_cell_multiline_paste(
            &self.editor.lines,
            line_idx,
            cursor_byte,
            normalized,
            &mut self.table_format_cache,
        ) else {
            return false;
        };

        let replaced_count = edit.end - edit.start + 1;
        let inserted_count = edit.lines.len();
        self.editor.lines.splice(edit.start..=edit.end, edit.lines);
        self.editor.cursor_line = edit.cursor_line;
        let target_line = &self.editor.lines[edit.cursor_line];
        self.editor.cursor_col = target_line[..edit.cursor_byte.min(target_line.len())]
            .chars()
            .count();

        self.splice_calc_line_metadata(edit.start, replaced_count, inserted_count);
        self.mark_edited_from_line(edit.start);
        true
    }

    pub(super) fn insert_paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        if self.editor.lines.is_empty() {
            self.editor.lines.push(String::new());
        }

        // Normalize line endings to keep cursor/line mapping predictable.
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        if self.try_insert_table_cell_multiline_paste(&normalized) {
            return;
        }
        let parts: Vec<&str> = normalized.split('\n').collect();
        if parts.is_empty() {
            return;
        }

        let line_idx = self
            .editor
            .cursor_line
            .min(self.editor.lines.len().saturating_sub(1));
        let col = self.editor.cursor_col;
        let current = self.editor.lines[line_idx].clone();
        let split_idx = byte_index(&current, col);
        let (left, right) = current.split_at(split_idx);

        if parts.len() == 1 {
            self.editor.lines[line_idx] = format!("{left}{}{right}", parts[0]);
            self.editor.cursor_line = line_idx;
            self.editor.cursor_col = col + parts[0].chars().count();
            self.splice_calc_line_metadata(line_idx, 1, 1);
            self.mark_edited_from_line(line_idx);
            return;
        }

        self.editor.lines[line_idx] = format!("{left}{}", parts[0]);
        let mut insert_at = line_idx + 1;
        for part in &parts[1..parts.len() - 1] {
            self.editor.lines.insert(insert_at, (*part).to_string());
            insert_at += 1;
        }

        let tail = *parts.last().unwrap_or(&"");
        self.editor
            .lines
            .insert(insert_at, format!("{tail}{right}"));
        self.editor.cursor_line = insert_at;
        self.editor.cursor_col = tail.chars().count();
        self.splice_calc_line_metadata(line_idx, 1, parts.len());
        self.mark_edited_from_line(line_idx);
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

    pub(super) fn insert_newline(&mut self) {
        let changed_from_line = self.editor.cursor_line;
        let col = self.editor.cursor_col;
        let idx = byte_index(self.current_line(), col);
        let right = self.editor.lines[self.editor.cursor_line][idx..].to_string();
        self.editor.lines[self.editor.cursor_line].truncate(idx);
        let insert_at = self.editor.cursor_line + 1;
        self.editor.lines.insert(insert_at, right);
        self.editor.cursor_line += 1;
        self.editor.cursor_col = 0;
        self.splice_calc_line_metadata(changed_from_line, 1, 2);
        self.mark_edited_from_line(changed_from_line);
    }

    pub(super) fn variable_autocomplete_state(&self) -> Option<VariableAutocompleteState> {
        if self.mode != UiMode::Editor {
            return None;
        }
        if !self.note_math_module_enabled() || !self.note_variables_module_enabled() {
            return None;
        }

        // Cross-note prefix takes priority: [[SHORTID]].partial
        let line = self.current_line();
        if let Some((short_id, bracket_col, from_col, partial)) =
            extract_cross_note_completion_prefix(line, self.editor.cursor_col)
        {
            let exports = cross_note_exports_for_autocomplete(
                &short_id,
                &self.cross_note_var_index,
                &self.calc.engine,
                &self.cross_note_db,
            );
            // Eagerly preload dep values in the background so that when the
            // user confirms the selection and a recompute fires, the index
            // already has f64 values — avoiding a blocking DB load at that point.
            let needs_preload = self
                .cross_note_var_index
                .lock()
                .ok()
                .map(|mut idx| idx.try_claim_eval(&short_id))
                .unwrap_or(false);
            if needs_preload {
                let bg_short_id = short_id.clone();
                let bg_var_index = std::sync::Arc::clone(&self.cross_note_var_index);
                let bg_condvar = std::sync::Arc::clone(&self.cross_note_eval_condvar);
                let bg_db = self.cross_note_db.clone();
                std::thread::spawn(move || {
                    let engine = app_core::calc::CalcEngine::new();
                    preload_cross_note_dep_value(&bg_short_id, &bg_var_index, &engine, &bg_db);
                    bg_condvar.notify_all();
                });
            }
            let suggestions: Vec<String> = exports
                .iter()
                .filter(|e| e.normalized.starts_with(&partial) && e.normalized != partial)
                .map(|e| e.name.clone())
                .collect();
            if !suggestions.is_empty() {
                return Some(VariableAutocompleteState {
                    popup_anchor_col: bracket_col,
                    from_col,
                    to_col: self.editor.cursor_col,
                    query: partial,
                    suggestions,
                });
            }
        }

        if self.calc.variable_names.is_empty() {
            return None;
        }
        let run = extract_variable_completion_prefix(line, self.editor.cursor_col)?;
        let (prefix, suggestions) = variable_completion_candidates(&run)
            .into_iter()
            .map(|candidate| {
                let suggestions = build_variable_suggestions(
                    &self.calc.variable_names,
                    &candidate.query,
                    self.variable_autocomplete_min_chars,
                    VARIABLE_AUTOCOMPLETE_MAX_SUGGESTIONS,
                );
                (candidate, suggestions)
            })
            .find(|(_, suggestions)| !suggestions.is_empty())?;
        Some(VariableAutocompleteState {
            popup_anchor_col: prefix.from_col,
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
                .saturating_sub(self.editor.scroll_line)
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
        let visible_col = viewport_col_for_display_col(
            display_col,
            line_width,
            self.editor.scroll_col,
            available,
        );
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

        let from_byte = byte_index(self.current_line(), from_col);
        let to_byte = byte_index(self.current_line(), to_col);
        self.editor.lines[self.editor.cursor_line].replace_range(from_byte..to_byte, &pick);
        self.editor.cursor_col = from_col + pick.chars().count();
        self.refresh_calc_line_metadata_at(self.editor.cursor_line);
        self.mark_edited();
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
            self.editor.lines[self.editor.cursor_line].replace_range(from_byte..to_byte, &result);
            self.editor.cursor_col = self.editor.lines[self.editor.cursor_line]
                [..from_byte.saturating_add(result.len())]
                .chars()
                .count();
            if should_reflow_table {
                self.try_autoformat_rules();
            }
            self.refresh_calc_line_metadata_at(self.editor.cursor_line);
            self.mark_edited();
            return true;
        }

        if should_reflow_table {
            let cursor_col = self.editor.cursor_col;
            let formula = find_table_formula_segments(&text)
                .into_iter()
                .find(|seg| cursor_col >= seg.cell_from_char && cursor_col <= seg.cell_to_char)
                .or_else(|| find_table_formula_segments(&text).into_iter().next());
            if let Some(seg) = formula {
                self.editor.lines[self.editor.cursor_line]
                    .replace_range(seg.from_byte..seg.to_byte, &result);
                self.editor.cursor_col = self.editor.lines[self.editor.cursor_line]
                    [..seg.from_byte.saturating_add(result.len())]
                    .chars()
                    .count();
                self.try_autoformat_rules();
                self.refresh_calc_line_metadata_at(self.editor.cursor_line);
                self.mark_edited_current_line();
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
        if self.editor.lines.is_empty() {
            return (0, 0);
        }
        let center = center_line.min(self.editor.lines.len().saturating_sub(1));
        if self.note_table_module_enabled() {
            if let Some(bounds) =
                crate::editor_core::table::table_block_bounds(&self.editor.lines, center)
            {
                return bounds;
            }
        }
        let window = 96usize;
        (
            center.saturating_sub(window),
            center
                .saturating_add(window)
                .min(self.editor.lines.len().saturating_sub(1)),
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
        rule: fn(&crate::editor_core::context::ResolvedContext<'_>) -> Option<crate::editor_core::types::EditOperation>,
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
        let fired = self
            .try_scoped_table_rule(crate::editor_core::text_rules::run_table_duplicate_delimiter_rule);
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
        if op.changes.is_empty() {
            if let Some(sel) = &op.selection {
                let target = sel.anchor.min(document_text_len(&self.editor.lines));
                let (line_idx, line_byte) = line_and_byte_for_offset(&self.editor.lines, target);
                if let Some(line) = self.editor.lines.get(line_idx) {
                    self.editor.cursor_line = line_idx;
                    self.editor.cursor_col = line[..line_byte.min(line.len())].chars().count();
                }
                self.adjust_cursor_after_operation(op);
                self.adjust_scroll();
            }
            return;
        }

        if op.changes.len() == 1 {
            let change = &op.changes[0];
            let doc_len = document_text_len(&self.editor.lines);
            let from = change.from.min(doc_len);
            let to = change.to.min(doc_len);
            let (from_line, from_byte) = line_and_byte_for_offset(&self.editor.lines, from);
            let (to_line, to_byte) = line_and_byte_for_offset(&self.editor.lines, to);

            let mut mapped_anchor =
                self.byte_offset_for_line_col(self.editor.cursor_line, self.editor.cursor_col);
            if from <= mapped_anchor {
                if to <= mapped_anchor {
                    let removed = to.saturating_sub(from);
                    let added = change.insert.len();
                    mapped_anchor = mapped_anchor.saturating_add(added).saturating_sub(removed);
                } else {
                    let inside = mapped_anchor.saturating_sub(from);
                    mapped_anchor = from.saturating_add(inside.min(change.insert.len()));
                }
            }

            let from_text = self
                .editor
                .lines
                .get(from_line)
                .cloned()
                .unwrap_or_default();
            let to_text = self.editor.lines.get(to_line).cloned().unwrap_or_default();
            let prefix = &from_text[..from_byte.min(from_text.len())];
            let suffix = &to_text[to_byte.min(to_text.len())..];
            let insert_parts = change.insert.split('\n').collect::<Vec<_>>();
            let mut replacement = Vec::with_capacity(insert_parts.len().max(1));

            if insert_parts.len() <= 1 {
                replacement.push(format!(
                    "{prefix}{}{suffix}",
                    insert_parts.first().copied().unwrap_or("")
                ));
            } else {
                replacement.push(format!("{prefix}{}", insert_parts[0]));
                for part in &insert_parts[1..insert_parts.len() - 1] {
                    replacement.push((*part).to_string());
                }
                replacement.push(format!(
                    "{}{suffix}",
                    insert_parts.last().copied().unwrap_or("")
                ));
            }

            let old_line_span = to_line.saturating_sub(from_line).saturating_add(1);
            let new_line_span = replacement.len().max(1);
            if from_line <= to_line && from_line < self.editor.lines.len() {
                let end = to_line.min(self.editor.lines.len().saturating_sub(1));
                self.editor.lines.splice(from_line..=end, replacement);
            } else {
                self.editor.lines = replacement;
            }
            if self.editor.lines.is_empty() {
                self.editor.lines.push(String::new());
            }

            self.splice_calc_line_metadata(from_line, old_line_span, new_line_span);

            let new_doc_len = doc_len
                .saturating_add(change.insert.len())
                .saturating_sub(to.saturating_sub(from));
            let final_anchor = op
                .selection
                .as_ref()
                .map_or(mapped_anchor, |selection| selection.anchor)
                .min(new_doc_len);
            let (line_idx, line_byte) = line_and_byte_for_offset(&self.editor.lines, final_anchor);
            if let Some(line) = self.editor.lines.get(line_idx) {
                self.editor.cursor_line = line_idx;
                self.editor.cursor_col = line[..line_byte.min(line.len())].chars().count();
            }
            // A rewrite of the cursor line alone (e.g. a table row reformatted
            // after a keystroke) is covered by the incremental fold update in
            // `mark_edited_*`; anything wider needs the O(N) rescan.
            if !(old_line_span == 1 && new_line_span == 1 && from_line == self.editor.cursor_line) {
                self.folds.rescan_pending = true;
            }
            self.mark_edited_from_line_with_span(
                from_line,
                Some((from_line, old_line_span, new_line_span)),
            );
            self.adjust_cursor_after_operation(op);
            self.adjust_scroll();
            return;
        }

        let old_doc_len = document_text_len(&self.editor.lines);
        let changed_from_offset = op
            .changes
            .iter()
            .map(|change| change.from.min(old_doc_len))
            .min()
            .unwrap_or(0);
        let changed_to_offset_old = op
            .changes
            .iter()
            .map(|change| change.to.min(old_doc_len))
            .max()
            .unwrap_or(changed_from_offset);
        let changed_from_line = line_and_byte_for_offset(&self.editor.lines, changed_from_offset).0;
        let old_changed_to_line_exclusive =
            line_and_byte_for_offset(&self.editor.lines, changed_to_offset_old).0 + 1;

        let original_anchor =
            self.byte_offset_for_line_col(self.editor.cursor_line, self.editor.cursor_col);
        let mut changes = op.changes.clone();
        changes.sort_by(|a, b| b.from.cmp(&a.from));
        let mapped_anchor = map_offset_through_changes(original_anchor, &changes);

        let mut current_doc_len = old_doc_len;
        for change in &changes {
            let from = change.from.min(current_doc_len);
            let to = change.to.min(current_doc_len);
            let removed = to.saturating_sub(from);
            let (from_line, old_line_span, new_line_span) =
                apply_text_change_in_place(&mut self.editor.lines, change, current_doc_len);
            self.splice_calc_line_metadata(from_line, old_line_span, new_line_span);
            current_doc_len = current_doc_len
                .saturating_add(change.insert.len())
                .saturating_sub(removed);
        }

        let mapped_from =
            map_offset_through_changes(changed_from_offset, &changes).min(current_doc_len);
        let mapped_to =
            map_offset_through_changes(changed_to_offset_old, &changes).min(current_doc_len);
        let mapped_changed_to = mapped_from.max(mapped_to);
        let new_changed_to_line_exclusive =
            line_and_byte_for_offset(&self.editor.lines, mapped_changed_to).0 + 1;
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
        .min(current_doc_len);
        let (line_idx, line_byte) = line_and_byte_for_offset(&self.editor.lines, final_anchor);
        if let Some(line) = self.editor.lines.get(line_idx) {
            self.editor.cursor_line = line_idx;
            self.editor.cursor_col = line[..line_byte.min(line.len())].chars().count();
        }
        self.folds.rescan_pending = true;
        self.mark_edited_from_line_with_span(
            changed_from_line,
            Some((changed_from_line, old_line_span, new_line_span)),
        );
        self.adjust_cursor_after_operation(op);
        self.adjust_scroll();
    }

    fn prune_empty_table_continuation_row_at_cursor(&mut self) -> bool {
        if !self.note_table_module_enabled() || self.editor.lines.is_empty() {
            return false;
        }
        if self.editor.cursor_line >= self.editor.lines.len() {
            return false;
        }
        if !crate::editor_core::table::is_empty_table_continuation_row(self.current_line()) {
            return false;
        }

        let remove_line = self.editor.cursor_line;
        self.editor.lines.remove(remove_line);
        if self.editor.lines.is_empty() {
            self.editor.lines.push(String::new());
            self.editor.cursor_line = 0;
            self.editor.cursor_col = 0;
        } else {
            self.editor.cursor_line = remove_line
                .saturating_sub(1)
                .min(self.editor.lines.len() - 1);
            self.editor.cursor_col = self
                .editor
                .cursor_col
                .min(line_char_len(self.current_line()));
            if self.note_table_module_enabled() {
                if let Some(cell) = table_cell_info_at_char(
                    &self.editor.lines,
                    self.editor.cursor_line,
                    self.editor.cursor_col,
                ) {
                    self.editor.cursor_col =
                        table_cell_navigation_anchor(self.current_line(), &cell);
                }
            }
        }

        self.splice_calc_line_metadata(remove_line, 1, 0);
        self.mark_edited_from_line(remove_line.saturating_sub(1));
        true
    }

    /// Backspace/Delete inside a table cell: removes only the cell's text
    /// (see `table::plan_table_char_delete`). False when not in a cell.
    fn try_table_char_delete(&mut self, backward: bool) -> bool {
        use crate::editor_core::table::{plan_table_char_delete, TableCharDelete};
        if !self.note_table_module_enabled() {
            return false;
        }
        let Some(plan) = plan_table_char_delete(self.current_line(), self.editor.cursor_col, backward)
        else {
            return false;
        };
        match plan {
            TableCharDelete::Stay { cursor } => self.editor.cursor_col = cursor,
            TableCharDelete::Remove { at, cursor } => {
                remove_char_at(&mut self.editor.lines[self.editor.cursor_line], at);
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

        if self.editor.cursor_col > 0 {
            let new_col = self.editor.cursor_col - 1;
            remove_char_at(&mut self.editor.lines[self.editor.cursor_line], new_col);
            self.editor.cursor_col = new_col;
            self.refresh_calc_line_metadata_at(self.editor.cursor_line);
            self.mark_edited_current_line();
            self.prune_empty_table_continuation_row_at_cursor();
            return;
        }

        if self.editor.cursor_line == 0 {
            return;
        }

        let removed = self.editor.lines.remove(self.editor.cursor_line);
        self.editor.cursor_line -= 1;
        let prev_len = line_char_len(&self.editor.lines[self.editor.cursor_line]);
        self.editor.lines[self.editor.cursor_line].push_str(&removed);
        self.editor.cursor_col = prev_len;
        self.splice_calc_line_metadata(self.editor.cursor_line, 2, 1);
        self.mark_edited_from_line_with_span(
            self.editor.cursor_line,
            Some((self.editor.cursor_line, 2, 1)),
        );
    }

    pub(super) fn delete_forward(&mut self) {
        if self.try_table_char_delete(false) {
            return;
        }

        let line_len = line_char_len(self.current_line());
        if self.editor.cursor_col < line_len {
            let col = self.editor.cursor_col;
            remove_char_at(&mut self.editor.lines[self.editor.cursor_line], col);
            self.refresh_calc_line_metadata_at(self.editor.cursor_line);
            self.mark_edited_current_line();
            self.prune_empty_table_continuation_row_at_cursor();
            return;
        }

        if self.editor.cursor_line + 1 >= self.editor.lines.len() {
            return;
        }

        let next = self.editor.lines.remove(self.editor.cursor_line + 1);
        self.editor.lines[self.editor.cursor_line].push_str(&next);
        self.splice_calc_line_metadata(self.editor.cursor_line, 2, 1);
        self.mark_edited_from_line_with_span(
            self.editor.cursor_line,
            Some((self.editor.cursor_line, 2, 1)),
        );
    }

    pub(super) fn try_shared_table_cursor_motion(
        &mut self,
        direction: crate::editor_core::table::TableCursorMotionDirection,
    ) -> bool {
        if !self.note_table_module_enabled() {
            return false;
        }
        let Some((block_start, block_end)) =
            crate::editor_core::table::table_block_bounds(&self.editor.lines, self.editor.cursor_line)
        else {
            return false;
        };
        let block_lines = &self.editor.lines[block_start..=block_end];
        let Some(target) = crate::editor_core::table::plan_table_cursor_motion(
            block_lines,
            self.editor.cursor_line.saturating_sub(block_start),
            self.editor.cursor_col,
            direction,
        ) else {
            return false;
        };

        self.editor.markdown_formatting_right_boundary_exit = None;
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
        if self.editor.markdown_formatting_right_boundary_exit
            == Some((self.editor.cursor_line, self.editor.cursor_col))
        {
            self.editor.markdown_formatting_right_boundary_exit = None;
            return;
        }
        self.editor.markdown_formatting_right_boundary_exit = None;

        if self.try_shared_table_cursor_motion(
            crate::editor_core::table::TableCursorMotionDirection::Left,
        ) {
            return;
        }

        if self.editor.cursor_col > 0 {
            self.editor.cursor_col -= 1;
            return;
        }
        let current_virtual = self.current_virtual_line();
        if current_virtual > 0 {
            if let Some(prev_real) = self.real_line_for_virtual(current_virtual - 1) {
                self.editor.cursor_line = prev_real;
                self.editor.cursor_col = line_char_len(self.current_line());
            }
        }
    }

    pub(super) fn move_cursor_right(&mut self) {
        let consumed_boundary_exit = self.editor.markdown_formatting_right_boundary_exit
            == Some((self.editor.cursor_line, self.editor.cursor_col));
        if consumed_boundary_exit {
            self.editor.markdown_formatting_right_boundary_exit = None;
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

        let line_len = line_char_len(self.current_line());
        if self.editor.cursor_col < line_len {
            self.editor.cursor_col += 1;
            self.editor.markdown_formatting_right_boundary_exit = boundary_exit_anchor;
            return;
        }
        let current_virtual = self.current_virtual_line();
        if current_virtual + 1 < self.visible_line_count() {
            if let Some(next_real) = self.real_line_for_virtual(current_virtual + 1) {
                self.editor.cursor_line = next_real;
                self.editor.cursor_col = 0;
                self.editor.markdown_formatting_right_boundary_exit = None;
                return;
            }
        }

        self.editor.markdown_formatting_right_boundary_exit = boundary_exit_anchor;
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
        if display.text != self.editor.lines[line_idx] {
            return None;
        }
        let (_, cols) = input::terminal_size();
        let width = cols.saturating_sub(self.gutter_width()).max(1);
        let mut ctx = self.render_context_at(line_idx);
        let (positions, _) = ctx.wrap_char_positions(
            &display.text,
            width,
            &display.decorations(&self.calc.variable_names),
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
            let rows = positions.iter().flatten().map(|(r, _)| r + 1).max().unwrap_or(1);
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
        self.editor.markdown_formatting_right_boundary_exit = None;
        self.clamp_cursor_to_line_bounds();
    }

    pub(super) fn move_cursor_up(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.editor.markdown_formatting_right_boundary_exit = None;
        let current_virtual = self.current_virtual_line();
        let target_virtual = current_virtual.saturating_sub(count);
        self.editor.cursor_line = self.real_line_for_virtual(target_virtual).unwrap_or(0);
        self.clamp_cursor_to_line_bounds();
    }

    pub(super) fn move_cursor_down(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.editor.markdown_formatting_right_boundary_exit = None;
        let current_virtual = self.current_virtual_line();
        let target_virtual = min(
            current_virtual.saturating_add(count),
            self.visible_line_count().saturating_sub(1),
        );
        self.editor.cursor_line = self
            .real_line_for_virtual(target_virtual)
            .unwrap_or_else(|| self.editor.lines.len().saturating_sub(1));
        self.clamp_cursor_to_line_bounds();
    }

    fn adjust_cursor_line_and_col_bounds(&mut self) {
        if self.editor.lines.is_empty() {
            self.editor.lines.push(String::new());
        }
        if self.editor.cursor_line >= self.editor.lines.len() {
            self.editor.cursor_line = self.editor.lines.len() - 1;
        }
        if let Some(owner) = self.fold_hidden_owner_for_line(self.editor.cursor_line) {
            self.editor.cursor_line = owner.min(self.editor.lines.len().saturating_sub(1));
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
                &self.editor.lines,
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
        if cursor_virtual < self.editor.scroll_line {
            self.editor.scroll_line = cursor_virtual;
        } else if cursor_virtual >= self.editor.scroll_line + height {
            self.editor.scroll_line = cursor_virtual + 1 - height;
        }
        self.editor.scroll_line = self
            .editor
            .scroll_line
            .min(self.visible_line_count().saturating_sub(1));

        let (_, cols) = input::terminal_size();
        let available = cols.saturating_sub(self.gutter_width());
        if available == 0 || self.cursor_line_wraps() {
            self.editor.scroll_col = 0;
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

        if cursor_display_col < self.editor.scroll_col {
            self.editor.scroll_col =
                cursor_display_col.saturating_sub(HORIZONTAL_SCROLL_LEFT_CONTEXT);
        } else if cursor_display_col >= self.editor.scroll_col + available {
            self.editor.scroll_col = cursor_display_col + 1 - available;
        }

        self.editor.scroll_col = self.editor.scroll_col.min(max_scroll);
    }

    pub(super) fn calc_eval_range_for_viewport(
        &self,
        editor_height: usize,
    ) -> Option<(usize, usize)> {
        if self.editor.lines.is_empty() || editor_height == 0 {
            return None;
        }
        let visible_count = self.visible_line_count();
        if visible_count == 0 {
            return None;
        }
        let prefetch = editor_height.saturating_mul(CALC_VIEWPORT_PREFETCH_MULTIPLIER);
        let start_virtual = self.editor.scroll_line.saturating_sub(prefetch);
        let end_virtual = self
            .editor
            .scroll_line
            .saturating_add(editor_height)
            .saturating_add(prefetch)
            .min(visible_count.saturating_sub(1));
        let start_line = self.real_line_for_virtual(start_virtual).unwrap_or(0);
        let end_line_inclusive = self
            .real_line_for_virtual(end_virtual)
            .unwrap_or_else(|| self.editor.lines.len().saturating_sub(1));
        let end_line_exclusive = end_line_inclusive
            .saturating_add(1)
            .min(self.editor.lines.len());
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
        let refs = app_core::calc::scan_cross_note_refs(&self.editor.lines);
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
            let short_ids: rustc_hash::FxHashSet<String> =
                refs.iter().map(|r| r.note_short_id.clone()).collect();
            match self.cross_note_var_index.lock() {
                Ok(index) => {
                    let mut missing = Vec::new();
                    let mut in_flight = Vec::new();
                    for sid in short_ids {
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
        for short_id in missing {
            preload_cross_note_dep_value(
                &short_id,
                &self.cross_note_var_index,
                &self.calc.engine,
                &self.cross_note_db,
            );
        }
    }

    pub(super) fn recompute_calc_range(&mut self, eval_from: usize, eval_to: usize) {
        if !self.note_math_module_enabled() {
            self.clear_calc_cache();
            return;
        }
        if eval_from >= eval_to || eval_to > self.editor.lines.len() {
            return;
        }
        let calc_mask = self.calc_feature_mask();
        crate::editor_core::calc_plan::sync_calc_dependency_index(
            &mut self.calc.calc_dependency_index,
            &self.editor.lines,
            eval_from,
            eval_to,
            calc_mask,
        );
        let vars_enabled = self.calc_variables_enabled();
        let cross_note_enabled = self.calc_cross_note_enabled();
        let extern_vars: Vec<ExternVar> = if cross_note_enabled {
            let note_id = self.active_note.id.clone();
            let refs = app_core::calc::scan_cross_note_refs(&self.editor.lines);
            self.preload_cross_note_deps_for_refs(&refs);
            if let Ok(mut index) = self.cross_note_var_index.lock() {
                index.update_deps(&note_id, &refs);
                index.extern_vars_for(&note_id)
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let calc_data = compute_calc_data(
            &self.calc.engine,
            &self.editor.lines,
            vars_enabled,
            cross_note_enabled,
            self.note_table_module_enabled(),
            Some((eval_from, eval_to)),
            extern_vars,
        );
        if self.calc.results.len() != self.editor.lines.len() {
            self.calc.results = vec![None; self.editor.lines.len()];
        }
        if self.calc.cell_results.len() != self.editor.lines.len() {
            self.calc.cell_results = vec![Vec::new(); self.editor.lines.len()];
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
        let variable_names =
            crate::editor_core::calc_plan::variable_names_from_calc_dependency_index(
                self.calc.calc_dependency_index.as_ref(),
            );
        self.calc.variable_names.set(if variable_names.is_empty() {
            calc_data.variable_names
        } else {
            variable_names
        });
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
        let Ok(Some(note)) = note_sources.resolve_wiki_link_note(short_id) else {
            return Vec::new();
        };
        crate::editor_core::markdown_tokens::extract_markdown_headings(&note.body)
            .into_iter()
            .map(|heading| WikiLinkSuggestion {
                short_id: short_id.to_string(),
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
                    short_id: n.id[..8.min(n.id.len())].to_string(),
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
            self.rebuild_wiki_link_prefix_index();
            self.rebuild_wiki_link_note_suggestions_cache();
        }
    }

    pub(super) fn dismiss_wiki_link_autocomplete(&mut self) {
        self.wiki_link_autocomplete_popup = WikiLinkAutocompletePopupState::default();
    }

    fn cleanup_pending_wiki_link_heading_prompt(&mut self) {
        let Some(short_id) = self
            .wiki_link_autocomplete_popup
            .pending_heading_short_id
            .clone()
        else {
            return;
        };
        let line_idx = self.wiki_link_autocomplete_popup.cursor_line;
        if line_idx >= self.editor.lines.len() {
            return;
        }
        let from_col = self.wiki_link_autocomplete_popup.from_col;
        let line = self.editor.lines[line_idx].clone();
        let chars: Vec<char> = line.chars().collect();
        let hash_col = from_col + 2 + short_id.chars().count();
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
        self.editor.lines[line_idx].replace_range(from_byte..to_byte, "");
        if self.editor.cursor_line == line_idx {
            self.editor.cursor_col = hash_col;
        }
        self.refresh_calc_line_metadata_at(line_idx);
        self.mark_edited();
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
                .saturating_sub(self.editor.scroll_line)
                + super::EDITOR_TOP_ROW,
            from_col.saturating_add(1),
        ));
        self.wiki_link_autocomplete_popup = WikiLinkAutocompletePopupState {
            visible: true,
            anchor_row,
            anchor_col,
            from_col,
            query: String::new(),
            pending_heading_short_id: None,
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
        if Self::parse_wiki_link_query(&query).is_none() {
            return false;
        }

        let note_suggestions = self.wiki_link_note_suggestions_cache.clone();
        let suggestions = note_suggestions.clone();
        let (anchor_row, anchor_col) = self.variable_popup_anchor(from_col).unwrap_or((
            self.editor
                .cursor_line
                .saturating_sub(self.editor.scroll_line)
                + super::EDITOR_TOP_ROW,
            from_col.saturating_add(1),
        ));
        self.wiki_link_autocomplete_popup = WikiLinkAutocompletePopupState {
            visible: true,
            anchor_row,
            anchor_col,
            from_col,
            query,
            pending_heading_short_id: None,
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
        let Some((_, _heading_query)) = Self::parse_wiki_link_query(&query) else {
            self.cancel_wiki_link_autocomplete();
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
                    if !self
                        .wiki_link_autocomplete_popup
                        .heading_cache
                        .contains_key(short_id)
                    {
                        let loaded = Self::load_wiki_link_heading_suggestions(db, short_id);
                        self.wiki_link_autocomplete_popup
                            .heading_cache
                            .insert(short_id.to_string(), loaded);
                    }
                    if let Some(cached) = self
                        .wiki_link_autocomplete_popup
                        .heading_cache
                        .get(short_id)
                    {
                        if value.is_empty() {
                            cached.clone()
                        } else {
                            let query = value.to_lowercase();
                            cached
                                .iter()
                                .filter(|suggestion| suggestion.title_lower.contains(&query))
                                .cloned()
                                .collect()
                        }
                    } else {
                        Vec::new()
                    }
                }
                None => {
                    if short_id.is_empty() {
                        self.wiki_link_autocomplete_popup.note_suggestions.clone()
                    } else {
                        let query = short_id.to_lowercase();
                        self.wiki_link_autocomplete_popup
                            .note_suggestions
                            .iter()
                            .filter(|suggestion| suggestion.title_lower.contains(&query))
                            .cloned()
                            .collect()
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
        let short_id = pick.short_id;
        let title = pick.title;
        let heading = pick.heading;
        let from_col = self.wiki_link_autocomplete_popup.from_col;
        let replacement = if let Some(heading) = heading.as_deref() {
            format!("[[{}#{}]]", short_id, heading)
        } else {
            format!("[[{}#]]", short_id)
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

        let from_byte = byte_index(&line, from_col);
        let to_byte = byte_index(&line, end_col);
        self.editor.lines[self.editor.cursor_line].replace_range(from_byte..to_byte, &replacement);
        if heading.is_some() {
            self.editor.cursor_col = from_col + replacement.chars().count();
        } else {
            // Keep caret right after the auto-added # to filter heading picks.
            self.editor.cursor_col = from_col + 2 + short_id.chars().count() + 1;
        }
        self.refresh_calc_line_metadata_at(self.editor.cursor_line);
        self.mark_edited();
        if heading.is_some() {
            self.wiki_link_autocomplete_popup.pending_heading_short_id = None;
            self.dismiss_wiki_link_autocomplete();
            self.status = format!("link heading: {title}");
        } else {
            self.wiki_link_autocomplete_popup.query = format!("{short_id}#");
            self.wiki_link_autocomplete_popup.pending_heading_short_id = Some(short_id);
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
        let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
        match note_sources.resolve_wiki_link(&link.short_id) {
            Ok(Some(summary)) => match db.get_note(&summary.id) {
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

    pub(super) fn open_wiki_link_preview(&mut self, db: &crate::storage::Db) {
        let line = self.current_line().to_string();
        let Some(link) =
            crate::editor_core::markdown_tokens::wiki_link_at_cursor(&line, self.editor.cursor_col)
        else {
            self.status = "no wiki-link at cursor".to_string();
            return;
        };
        let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
        match note_sources.resolve_wiki_link(&link.short_id) {
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
        for (idx, line) in self.editor.lines.iter().enumerate() {
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
        let Some(eval_range) = self.calc_eval_range_for_viewport(editor_height) else {
            return;
        };
        if !force && self.calc_runtime.last_view_eval_range == Some(eval_range) {
            return;
        }
        self.recompute_calc_range(eval_range.0, eval_range.1);
        self.calc_runtime.last_view_eval_range = Some(eval_range);
        self.record_perf_duration("tui.calc.viewport_eval", "eval", started.elapsed());
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
