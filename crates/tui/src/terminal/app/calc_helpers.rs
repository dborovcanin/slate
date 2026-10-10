use crate::storage::Db;
#[cfg(test)]
use crate::terminal::text_utils::line_display_cols;
use app_core::calc::{CalcEngine, VariableIndexEntry};
use app_core::cross_note::CrossNoteVarIndex;
use std::sync::{Arc, Mutex};

#[cfg(test)]
pub(super) fn calc_ghost_prefix(text: &str, calc_ghost: Option<&str>) -> &'static str {
    if calc_ghost
        .map(|ghost| ghost.trim_start().starts_with('*'))
        .unwrap_or(false)
    {
        " "
    } else if contains_assignment_operator(text) {
        " = "
    } else {
        " → "
    }
}

#[cfg(test)]
pub(super) fn rendered_line_display_cols(text: &str, calc_ghost: Option<&str>) -> usize {
    let mut width = line_display_cols(text);
    if let Some(ghost) = calc_ghost {
        width += calc_ghost_prefix(text, Some(ghost)).chars().count();
        width += ghost.chars().count();
    }
    width
}

pub(super) use note_session::calc_eval::{compute_calc_data, CalcData};

#[cfg(test)]
pub(super) fn compute_calc_results(
    lines: &[String],
    variables_enabled: bool,
) -> Vec<Option<String>> {
    let engine = CalcEngine::new();
    compute_calc_data(
        &engine,
        lines,
        variables_enabled,
        true,
        true,
        None,
        Vec::new(),
    )
    .line_results
}

/// Decide whether an already-eligible line's trailing ` = <literal>` should
/// be rewritten to `new_result`. The caller is responsible for the eligibility
/// gate (line unchanged since last recompute AND previous backend result was
/// `None`, i.e. trailer was in sync). This helper only handles the per-line
/// mechanics: locate the trailer, short-circuit when already in sync, and
/// enforce the cursor guard so we never yank text from under the caret.
///
/// Returns `Some((eq_byte_idx, new_tail))` so the caller can run
/// `line.replace_range(eq_byte_idx.., &new_tail)`, or `None` to leave the
/// line untouched.
#[cfg(test)]
pub(super) fn compute_calc_trailer_refresh(
    line: &str,
    new_result: &str,
    is_cursor_line: bool,
    cursor_col: usize,
) -> Option<(usize, String)> {
    let refresh = crate::editor_core::calc_plan::compute_calc_trailer_refresh(
        line,
        new_result,
        is_cursor_line,
        cursor_col,
    )?;
    Some((refresh.eq_byte_idx, refresh.new_tail))
}

pub(super) fn find_calc_segment_range(text: &str) -> Option<(usize, usize)> {
    let segment = crate::editor_core::calc_plan::find_calc_segment(text)?;
    Some((segment.from_byte, segment.to_byte))
}

#[cfg(test)]
pub(super) use crate::editor_core::completion::{
    build_variable_suggestions, extract_variable_completion_prefix, variable_completion_candidates,
};

pub(super) fn contains_assignment_operator(text: &str) -> bool {
    crate::editor_core::calc_plan::contains_assignment_operator(text)
}

/// Returns variable name entries for `note_id` for autocomplete suggestions.
/// Uses a fast text scan (no expression evaluation) so the first call is cheap.
/// Values are NOT populated by this function — use `preload_cross_note_dep_value`
/// for ghost-eval correctness.
pub(super) fn cross_note_exports_for_autocomplete(
    note_id: &str,
    cross_note_var_index: &Arc<Mutex<CrossNoteVarIndex>>,
    _engine: &CalcEngine,
    db: &Db,
) -> Vec<VariableIndexEntry> {
    // Fast path: name scan already done for this note.
    if let Ok(index) = cross_note_var_index.lock() {
        if index.was_name_scan_attempted(note_id) {
            return index.exports_for_note(note_id).to_vec();
        }
    }

    // Slow path: load the note body and do a name-only scan (no eval).
    let note = match db.get_note(note_id) {
        Ok(Some(n)) => n,
        _ => {
            if let Ok(mut index) = cross_note_var_index.lock() {
                index.mark_name_scan_attempted(note_id);
            }
            return Vec::new();
        }
    };
    let lines: Vec<String> = note.body.split('\n').map(|l| l.to_string()).collect();
    let entries = app_core::calc::scan_variable_assignments(&lines);
    if let Ok(mut index) = cross_note_var_index.lock() {
        index.update_entries_only(note_id, &entries);
        index.mark_name_scan_attempted(note_id);
    }
    entries
}

/// Ensures `note_id`'s exported values are in the index, evaluating it
/// after the notes it references (`app_core::cross_note::load_note_exports`).
/// Called from `preload_cross_note_deps` before each recompute; a note already
/// evaluated costs one lookup.
pub(super) fn preload_cross_note_dep_value(
    note_id: &str,
    cross_note_var_index: &Arc<Mutex<CrossNoteVarIndex>>,
    engine: &CalcEngine,
    db: &Db,
) {
    app_core::cross_note::load_note_exports(db, engine, cross_note_var_index, note_id);
}

/// If the text before `cursor_col` ends with `[[ID]].partial`, return
/// `(note_id, bracket_col, from_col, partial)`.
/// `bracket_col` is the char column of the opening `[[` — used to anchor the
/// autocomplete popup visually under the full `[[id]].` expression.
/// `from_col` is the start of the partial var name — used for text replacement.
pub(super) use crate::editor_core::completion::extract_cross_note_completion_prefix;
