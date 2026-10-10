use crate::storage::Db;
#[cfg(test)]
use crate::terminal::text_utils::line_display_cols;
use app_core::calc::{CalcEngine, ExternVar, NoteEvaluationOptions, VariableIndexEntry};
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

pub(super) use note_session::calc_eval::{
    compute_calc_data, compute_calc_data_for_lines, CalcData,
};

/// Full-note eval that also reads/writes the shared cross-note variable index.
/// Use this instead of `compute_calc_data` for whole-document recomputes so that
/// cross-note variable references resolve correctly and exported variables stay
/// visible to other notes.
pub(super) fn compute_calc_data_for_note(
    engine: &CalcEngine,
    lines: &[String],
    variables_enabled: bool,
    cross_note_enabled: bool,
    table_enabled: bool,
    note_id: &str,
    cross_note_var_index: &Arc<Mutex<CrossNoteVarIndex>>,
) -> CalcData {
    // File notes cannot be linked, so they neither import nor export.
    let linkable =
        cross_note_enabled && crate::editor_core::markdown_tokens::is_note_link_id(note_id);
    let has_cross_note_syntax = linkable && lines.iter().any(|l| l.contains("[["));

    let (extern_vars, precomputed_refs) = if has_cross_note_syntax {
        // Scan outside the lock: TUI runs on a single event-loop thread so
        // no concurrent eval can race update_deps for the same note_id.
        let refs = app_core::calc::scan_cross_note_refs(lines);
        let extern_vars = if let Ok(mut index) = cross_note_var_index.lock() {
            index.update_deps(note_id, &refs);
            index.extern_vars_for(note_id)
        } else {
            Vec::new()
        };
        (extern_vars, Some(refs))
    } else {
        (Vec::new(), None)
    };

    let result = engine.evaluate_note_context(
        lines,
        NoteEvaluationOptions {
            variables_enabled,
            cross_note_enabled,
            table_enabled,
            eval_range: None,
            extern_vars,
            precomputed_refs,
            ..Default::default()
        },
    );

    if linkable {
        if let Ok(mut index) = cross_note_var_index.lock() {
            index.update_exports(note_id, &result.variables, &result.variable_values);
            index.update_deps(note_id, &result.cross_note_refs);
        }
    }

    let mut variable_names = result
        .variables
        .into_iter()
        .map(|entry| entry.normalized)
        .collect::<Vec<_>>();
    variable_names.sort();
    variable_names.dedup();

    CalcData {
        first_line: result.first_line,
        line_results: result.line_results,
        cell_results: result.table_cell_results,
        variable_names,
    }
}

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

/// Values for the `[[ID]].var` references in `lines`, loading each
/// linked note on first use, so the first calc pass after opening a note
/// already shows cross-note results.
pub(super) fn startup_cross_note_extern_vars(
    db: &Db,
    engine: &CalcEngine,
    cross_note_var_index: &Arc<Mutex<CrossNoteVarIndex>>,
    note_id: &str,
    lines: &[String],
) -> Vec<ExternVar> {
    let refs = app_core::calc::scan_cross_note_refs(lines);
    if refs.is_empty() {
        return Vec::new();
    }
    app_core::cross_note::refresh_referenced_notes(db, cross_note_var_index, lines);
    let dep_ids: rustc_hash::FxHashSet<&str> = refs.iter().map(|r| r.note_id.as_str()).collect();
    for dep_id in dep_ids {
        preload_cross_note_dep_value(dep_id, cross_note_var_index, engine, db);
    }
    match cross_note_var_index.lock() {
        Ok(mut index) => {
            index.update_deps(note_id, &refs);
            index.extern_vars_for(note_id)
        }
        Err(_) => Vec::new(),
    }
}

/// If the text before `cursor_col` ends with `[[ID]].partial`, return
/// `(note_id, bracket_col, from_col, partial)`.
/// `bracket_col` is the char column of the opening `[[` — used to anchor the
/// autocomplete popup visually under the full `[[id]].` expression.
/// `from_col` is the start of the partial var name — used for text replacement.
pub(super) use crate::editor_core::completion::extract_cross_note_completion_prefix;
