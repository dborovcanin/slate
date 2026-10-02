use super::VariableCompletionPrefix;
use crate::storage::Db;
#[cfg(test)]
use crate::terminal::text_utils::line_display_cols;
use app_core::calc::{
    CalcEngine, ExternVar, NoteContextCache, NoteEvaluationOptions, NoteEvaluationResult,
    TableCellEvaluation, VariableIndexEntry,
};
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

pub(super) struct CalcData {
    /// The line `line_results[0]` and `cell_results[0]` belong to; nonzero
    /// for a range evaluation, which returns only its own lines.
    pub(super) first_line: usize,
    pub(super) line_results: Vec<Option<String>>,
    pub(super) cell_results: Vec<Vec<TableCellEvaluation>>,
    pub(super) variable_names: Vec<String>,
}

impl CalcData {
    pub(super) fn line_result(&self, line: usize) -> Option<String> {
        line.checked_sub(self.first_line)
            .and_then(|idx| self.line_results.get(idx))
            .cloned()
            .flatten()
    }

    pub(super) fn cell_result(&self, line: usize) -> Vec<TableCellEvaluation> {
        line.checked_sub(self.first_line)
            .and_then(|idx| self.cell_results.get(idx))
            .cloned()
            .unwrap_or_default()
    }
}

pub(super) fn compute_calc_data(
    engine: &CalcEngine,
    lines: &[String],
    variables_enabled: bool,
    cross_note_enabled: bool,
    table_enabled: bool,
    eval_range: Option<(usize, usize)>,
    extern_vars: Vec<ExternVar>,
) -> CalcData {
    let result = engine.evaluate_note_context(
        lines,
        NoteEvaluationOptions {
            variables_enabled,
            cross_note_enabled,
            table_enabled,
            eval_range,
            extern_vars,
            ..Default::default()
        },
    );
    calc_data_from_result(result)
}

/// Evaluates only `eval_lines`; table formula cells elsewhere read their
/// `table_cell_seeds` value instead of being evaluated again.
pub(super) fn compute_calc_data_for_lines(
    engine: &CalcEngine,
    lines: &[String],
    options: NoteEvaluationOptions,
    eval_lines: Vec<usize>,
    table_cell_seeds: rustc_hash::FxHashMap<(usize, usize), String>,
) -> CalcData {
    let result = engine.evaluate_note_context(
        lines,
        NoteEvaluationOptions {
            eval_lines: Some(eval_lines),
            table_cell_seeds,
            ..options
        },
    );
    calc_data_from_result(result)
}

/// `compute_calc_data` that reuses whole-note preparation from `cache` while
/// the note is unchanged, for repeated range evaluations such as scrolling.
#[allow(clippy::too_many_arguments)]
pub(super) fn compute_calc_data_cached(
    engine: &CalcEngine,
    lines: &[String],
    variables_enabled: bool,
    cross_note_enabled: bool,
    table_enabled: bool,
    eval_range: Option<(usize, usize)>,
    extern_vars: Vec<ExternVar>,
    cache: &mut NoteContextCache,
    text_generation: u64,
    omit_note_wide_results: bool,
) -> CalcData {
    let result = engine.evaluate_note_context_cached(
        lines,
        NoteEvaluationOptions {
            variables_enabled,
            cross_note_enabled,
            table_enabled,
            eval_range,
            extern_vars,
            text_generation: Some(text_generation),
            omit_note_wide_results,
            ..Default::default()
        },
        cache,
    );
    calc_data_from_result(result)
}

fn calc_data_from_result(result: NoteEvaluationResult) -> CalcData {
    let mut variable_names = result
        .variables
        .into_iter()
        .map(|entry| entry.normalized)
        .collect::<Vec<_>>();
    variable_names.sort();
    variable_names.dedup();

    let cell_results = result.table_cell_results;

    CalcData {
        first_line: result.first_line,
        line_results: result.line_results,
        cell_results,
        variable_names,
    }
}

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

pub(super) fn variable_query_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == ' '
}

pub(super) fn extract_variable_completion_prefix(
    line_text: &str,
    cursor_col: usize,
) -> Option<VariableCompletionPrefix> {
    let chars = line_text.chars().collect::<Vec<_>>();
    let col = cursor_col.min(chars.len());
    let mut from = col;

    while from > 0 && variable_query_char(chars[from - 1]) {
        from -= 1;
    }

    while from < col && chars[from] == ' ' {
        from += 1;
    }

    if from >= col {
        return None;
    }

    let query = chars[from..col].iter().collect::<String>();
    if query.is_empty() || query.ends_with(' ') {
        return None;
    }

    Some(VariableCompletionPrefix {
        from_col: from,
        to_col: col,
        query,
    })
}

/// Candidate queries for a completion prefix, longest first: the whole run
/// (variable names may contain spaces, e.g. `tax ra` -> `tax rate`) and then
/// each shorter run starting after a space (`then pri` -> `pri`).
pub(super) fn variable_completion_candidates(
    prefix: &VariableCompletionPrefix,
) -> Vec<VariableCompletionPrefix> {
    let chars: Vec<char> = prefix.query.chars().collect();
    let mut candidates = vec![VariableCompletionPrefix {
        from_col: prefix.from_col,
        to_col: prefix.to_col,
        query: prefix.query.clone(),
    }];
    for (idx, ch) in chars.iter().enumerate() {
        if *ch == ' ' && idx + 1 < chars.len() && chars[idx + 1] != ' ' {
            candidates.push(VariableCompletionPrefix {
                from_col: prefix.from_col + idx + 1,
                to_col: prefix.to_col,
                query: chars[idx + 1..].iter().collect(),
            });
        }
    }
    candidates
}

pub(super) fn build_variable_suggestions(
    variable_names: &[String],
    query: &str,
    min_chars: usize,
    max_suggestions: usize,
) -> Vec<String> {
    let normalized_query = query.trim().to_lowercase();
    if normalized_query.chars().count() < min_chars {
        return Vec::new();
    }

    let mut matches = variable_names
        .iter()
        .filter(|name| name.starts_with(&normalized_query) && **name != normalized_query)
        .cloned()
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    matches.truncate(max_suggestions.max(1));
    matches
}

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
pub(super) fn extract_cross_note_completion_prefix(
    line_text: &str,
    cursor_col: usize,
) -> Option<(String, usize, usize, String)> {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // Note ids as `markdown_tokens::is_note_link_id` accepts them.
        Regex::new(r"\[\[([A-Za-z0-9][A-Za-z0-9_-]{0,63})\]\]\.([A-Za-z0-9_][A-Za-z0-9_ ]*)?$")
            .unwrap()
    });

    let chars: Vec<char> = line_text.chars().collect();
    let col = cursor_col.min(chars.len());
    let text_before: String = chars[..col].iter().collect();
    let m = re.captures(&text_before)?;
    let full_match_start_byte = m.get(0)?.start();
    let bracket_col = text_before[..full_match_start_byte].chars().count();

    let note_id = m.get(1)?.as_str().to_string();
    let partial_raw = m.get(2).map(|g| g.as_str()).unwrap_or("");
    let partial = partial_raw.trim().to_lowercase();

    let partial_chars = partial_raw.chars().count();
    let from_col = col.saturating_sub(partial_chars);

    Some((note_id, bracket_col, from_col, partial))
}
